//! 本地语义向量（embedding）模块：bge-small-zh-v1.5 + candle 纯 Rust 推理。
//!
//! 选型理由（贴合"离线、发给任何人可用、Win/mac 双端"）：
//! - **candle 纯 Rust**：不依赖 onnxruntime 平台库（Win 的 dll / mac 的 dylib 及签名问题），
//!   一份代码两端编译即用，CPU 推理即可（模型仅 4 层 BERT，单条编码约 20-40ms）
//! - **bge-small-zh-v1.5**：中英双语（鸿蒙 API 名为英文、描述为中文），512 维，
//!   fp32 权重约 91MB，随安装包 resources/embedding/ 分发
//! - **CLS pooling + L2 归一化**：与 sentence-transformers 官方推理口径一致，
//!   保证查询与文档向量可比（余弦相似度）
//!
//! 降级策略：模型文件缺失 / 加载失败 → 模块不可用，调用方自动回退到 TF 关键词打分，
//! 向量检索只是增强层，不阻塞主功能。
//!
//! 结构说明：candle 推理相关代码全部放在 `#[cfg(feature = "embedding")]` 的 `imp` 子模块中。
//! 原因：candle 的 gemm/libm 数学函数（fma/exp/log2 等）在 Windows 的单元测试链接下会
//! 触发 0xc0000139（STATUS_ENTRYPOINT_NOT_FOUND）——但主程序 / full_fetch 等生产二进制
//! 链接正常。故 candle 依赖被拆到 `embedding` feature 下（默认不启用），单元测试仅覆盖
//! 不依赖 candle 的纯函数（归一化、字节转换、RRF 融合等）；模型推理正确性由
//! full_fetch --embed 建索引与运行时向量检索共同保证。

/// 向量维度（bge-small-zh-v1.5 = 512）
pub const EMBED_DIM: usize = 512;

/// L2 归一化（余弦相似度等价）；未启用 embedding feature 时暂无调用方
#[cfg_attr(not(feature = "embedding"), allow(dead_code))]
fn l2_normalize(v: &[f32]) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        v.to_vec()
    } else {
        v.iter().map(|x| x / norm).collect()
    }
}

/// 编码文本来源：api_docs 单条记录拼成可检索文本（API 名 + 声明 + 模块/Kit + 变更说明）。
/// 索引的是"文档侧"，不带查询指令前缀。
pub fn doc_text(api_name: &str, declaration: &str, module: &str, kit: &str) -> String {
    format!(
        "{api_name} {declaration} {} {}",
        if module.is_empty() { "" } else { module },
        if kit.is_empty() { "" } else { kit },
    )
}

/// 编码为 f32 LE 字节（BLOB 存储）
pub fn to_f32_le_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// 从 BLOB 解码 f32 向量
pub fn from_f32_le_bytes(data: &[u8]) -> Vec<f32> {
    data.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// 单个查询向量与某条存储向量的余弦相似度（两者均已 L2 归一化 → 点积）；
/// 未启用 embedding feature 时暂无调用方
#[cfg_attr(not(feature = "embedding"), allow(dead_code))]
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// RRF 融合：向量检索得分 + 关键词命中，综合排序（防止纯向量把 TF 结果挤出）。
/// `vec_hits`: (doc_id, score)；`kw_hits`: (doc_id, kw_score)。
/// 融合分 = RRF(vec_rank) + RRF(kw_rank)，返回按融合分降序的 doc_id 列表。
pub fn rrf_fuse(
    vec_hits: &[(i64, f32)],
    kw_hits: &[(i64, f32)],
    top_n: usize,
) -> Vec<(i64, f64)> {
    use std::collections::HashMap;
    let mut fused: HashMap<i64, f64> = HashMap::new();
    const K: f64 = 60.0;
    for (rank, (id, _)) in vec_hits.iter().enumerate() {
        *fused.entry(*id).or_insert(0.0) += 1.0 / (K + rank as f64);
    }
    for (rank, (id, _)) in kw_hits.iter().enumerate() {
        *fused.entry(*id).or_insert(0.0) += 1.0 / (K + rank as f64);
    }
    let mut out: Vec<(i64, f64)> = fused.into_iter().collect();
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(top_n);
    out
}

/// 建索引进度（phase: checking / embedding）
#[derive(serde::Serialize)]
pub struct EmbedProgress {
    pub phase: String,
    pub current: usize,
    pub total: usize,
    pub message: String,
}

/// 建索引进度回调（后台线程跨线程使用，须 Send + Sync）
pub type EmbedProgressCb = Box<dyn Fn(&EmbedProgress) + Send + Sync>;

// ───────────────────────── candle 推理实现（生产编译专用） ─────────────────────────

#[cfg(feature = "embedding")]
mod imp {
    use super::{cosine, doc_text, from_f32_le_bytes, l2_normalize, to_f32_le_bytes};
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    use candle_core::{DType, Device, Tensor};
    use candle_nn::VarBuilder;
    use candle_transformers::models::bert::{BertModel, Config as BertConfig};
    use tokenizers::Tokenizer;

    /// 模型目录（resources/embedding/bge-small-zh-v1.5），随安装包分发。
    /// 运行时通过 resource_dir 定位；开发模式回退到 CARGO_MANIFEST_DIR/resources。
    const MODEL_DIR_NAME: &str = "embedding/bge-small-zh-v1.5";

    /// BGE v1.5 官方建议：查询语句加指令前缀，提升检索相关性（文档侧不加）
    const QUERY_INSTRUCTION: &str = "为这个句子生成表示以用于检索相关文章：";

    /// 编码器实例：懒加载（首次编码时初始化模型，约 1-2s）
    static EMBEDDER: OnceLock<Option<Embedder>> = OnceLock::new();

    /// 运行时资源目录（打包后由 tauri setup 注入；CLI/开发模式用默认回退）
    static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();

    /// 注入资源目录（tauri setup 阶段调用一次；full_fetch 等 CLI 可跳过，走开发回退路径）
    pub fn set_resource_dir(dir: PathBuf) {
        let _ = RESOURCE_DIR.set(dir);
    }

    /// 选择推理设备：GPU 优先，探测失败自动回退 CPU。
    /// - 启用 cuda feature（Windows/Linux 构建）：NVIDIA CUDA；驱动缺失/初始化失败 → CPU
    /// - 未启用任何加速 feature：恒为 CPU
    fn pick_device() -> Device {
        #[cfg(feature = "cuda")]
        {
            match Device::cuda_if_available(0) {
                Ok(d) => return d,
                Err(e) => crate::utils::logger::log_event(
                    "embedding_device_cuda_fallback",
                    serde_json::json!({ "error": e.to_string() }),
                ),
            }
        }
        Device::Cpu
    }

    /// 线程安全的编码器（内部不可变，纯推理）
    pub struct Embedder {
        model: BertModel,
        tokenizer: Tokenizer,
        device: Device,
    }

    impl Embedder {
        /// 从模型目录加载。失败返回 Err（调用方决定回退策略）。
        pub fn load(model_dir: &Path) -> Result<Self, String> {
            let config_path = model_dir.join("config.json");
            let tokenizer_path = model_dir.join("tokenizer.json");
            let weights_path = model_dir.join("model.safetensors");

            for p in [&config_path, &tokenizer_path, &weights_path] {
                if !p.is_file() {
                    return Err(format!("embedding 模型文件缺失: {}", p.display()));
                }
            }

            // 设备：GPU 优先（cuda feature 时探测），探测失败自动回退 CPU，保证跨端一致可用
            let device = pick_device();

            let config: BertConfig = serde_json::from_str(
                &std::fs::read_to_string(&config_path).map_err(|e| e.to_string())?,
            )
            .map_err(|e| format!("解析 bert config 失败: {e}"))?;

            let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(|e| e.to_string())?;

            // mmap 加载 safetensors（大权重零拷贝）
            let vb = unsafe {
                VarBuilder::from_mmaped_safetensors(&[weights_path], DType::F32, &device)
                    .map_err(|e| e.to_string())?
            };
            let model = BertModel::load(vb, &config).map_err(|e| e.to_string())?;

            Ok(Self {
                model,
                tokenizer,
                device,
            })
        }

        /// 单条文本 → 归一化向量（CLS pooling）。
        pub fn embed(&self, text: &str, as_query: bool) -> Result<Vec<f32>, String> {
            // 模型最大序列长度（bge-small-zh-v1.5 的 max_position_embeddings）
            const MAX_SEQ: usize = 512;

            let full = if as_query {
                format!("{QUERY_INSTRUCTION}{text}")
            } else {
                text.to_string()
            };

            let encoding = self
                .tokenizer
                .encode(full, true)
                .map_err(|e| format!("tokenize 失败: {e}"))?;
            // 截断到最大序列长度：超长文本（如超长 declaration）会让 position_ids/词表 gather 越界
            // （CPU 路径会报错、CUDA 路径触发 kernel 断言），截尾保留 [CLS] 头部语义
            let input_ids = encoding.get_ids().iter().copied().take(MAX_SEQ).collect::<Vec<_>>();
            let attention_mask = encoding
                .get_attention_mask()
                .iter()
                .copied()
                .take(MAX_SEQ)
                .collect::<Vec<_>>();

            let ids = Tensor::new(vec![input_ids], &self.device).map_err(|e| e.to_string())?;
            let mask = Tensor::new(vec![attention_mask], &self.device).map_err(|e| e.to_string())?;
            let type_ids = ids.zeros_like().map_err(|e| e.to_string())?;

            let out = self
                .model
                .forward(&ids, &type_ids, Some(&mask))
                .map_err(|e| e.to_string())?;
            // out: [1, seq, hidden] → 取 [0, 0, :]（CLS 向量）
            let cls = out
                .get(0)
                .map_err(|e| e.to_string())?
                .get(0)
                .map_err(|e| e.to_string())?
                .to_vec1::<f32>()
                .map_err(|e| e.to_string())?;

            Ok(l2_normalize(&cls))
        }

        /// 批量编码（建索引用）：循环单条编码。模型只跑 4 层小 BERT，
        /// 单条 CPU 约 20-40ms，批量逐条足够（4.6 万条约 20~30 分钟，后台任务）。
        pub fn embed_batch(
            &self,
            texts: &[String],
            as_query: bool,
        ) -> Result<Vec<Vec<f32>>, String> {
            texts.iter().map(|t| self.embed(t, as_query)).collect()
        }
    }

    /// 获取全局编码器（懒加载）。不可用时返回 None（回退 TF 关键词方案）。
    pub fn global_embedder() -> Option<&'static Embedder> {
        EMBEDDER
            .get_or_init(|| {
                let dir = model_dir()?;
                match Embedder::load(&dir) {
                    Ok(em) => {
                        crate::utils::logger::log_event(
                            "embedding_device",
                            serde_json::json!({ "device": format!("{:?}", em.device) }),
                        );
                        Some(em)
                    }
                    Err(e) => {
                        crate::utils::logger::log_event(
                            "embedding_model_unavailable",
                            serde_json::json!({ "error": e }),
                        );
                        None
                    }
                }
            })
            .as_ref()
    }

    /// 定位模型目录：优先资源目录（打包环境），回退开发目录。
    pub fn model_dir() -> Option<PathBuf> {
        // 打包运行时：resource_dir/embedding/bge-small-zh-v1.5（tauri setup 注入）
        if let Some(dir) = RESOURCE_DIR.get() {
            let p = dir.join(MODEL_DIR_NAME);
            if p.join("model.safetensors").is_file() {
                return Some(p);
            }
        }
        // 开发/CLI 环境：src-tauri/resources/embedding/bge-small-zh-v1.5
        let dev = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join(MODEL_DIR_NAME);
        if dev.join("model.safetensors").is_file() {
            return Some(dev);
        }
        None
    }

    /// 重置全局编码器（测试用 / 模型更新后重建）
    pub fn reset() {
        // OnceLock 无法直接清空，这里仅提供重建语义的占位（实际由进程生命周期保证）
        let _ = EMBEDDER.get();
    }

    /// 构建/重建向量索引：遍历 api_docs，编码并写入 api_docs_embeddings。
    /// 模型不可用时返回 Err（调用方提示用户）；已存在且 model 匹配时跳过（幂等）。
    /// 返回 (已索引行数, 跳过行数)。
    pub fn build_index(
        conn: &rusqlite::Connection,
        batch_size: usize,
    ) -> Result<(usize, usize), String> {
        let em = global_embedder()
            .ok_or_else(|| "embedding 模型不可用（缺少模型文件或加载失败）".to_string())?;

        // 幂等：已按当前模型建过索引则跳过
        let indexed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM api_docs_embeddings WHERE model = 'bge-small-zh-v1.5'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM api_docs", [], |r| r.get(0))
            .unwrap_or(0);
        if indexed >= total && total > 0 {
            return Ok((0, total as usize));
        }

        // 清掉旧模型/旧版本向量，避免脏数据干扰
        let _ = conn.execute("DELETE FROM api_docs_embeddings", []);

        let mut stmt = conn
            .prepare(
                "SELECT id, COALESCE(api_name,''), declaration, COALESCE(module,''), COALESCE(kit,'')
                 FROM api_docs ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows: Vec<(i64, String, String, String, String)> = stmt
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        drop(stmt);

        let mut inserted = 0usize;
        let mut skipped = 0usize;
        let now = chrono::Utc::now().timestamp();

        // 分批编码 + 事务写入（避免大事务锁库过久）
        for chunk in rows.chunks(batch_size.max(1)) {
            let texts: Vec<String> = chunk
                .iter()
                .map(|(_, name, decl, module, kit)| doc_text(name, decl, module, kit))
                .collect();
            let vecs = match em.embed_batch(&texts, false) {
                Ok(v) => v,
                Err(e) => {
                    crate::utils::logger::log_event(
                        "embedding_index_batch_error",
                        serde_json::json!({ "error": e, "batch": chunk.len() }),
                    );
                    skipped += chunk.len();
                    continue;
                }
            };
            let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
            for (row, vec) in chunk.iter().zip(vecs.iter()) {
                let bytes = to_f32_le_bytes(vec);
                if let Err(e) = tx.execute(
                    "INSERT OR REPLACE INTO api_docs_embeddings (doc_id, model, vector, created_at)
                     VALUES (?1, 'bge-small-zh-v1.5', ?2, ?3)",
                    rusqlite::params![row.0, bytes, now],
                ) {
                    crate::utils::logger::log_event(
                        "embedding_index_insert_error",
                        serde_json::json!({ "error": e.to_string(), "doc_id": row.0 }),
                    );
                }
            }
            tx.commit().map_err(|e| e.to_string())?;
            inserted += vecs.len();
        }
        Ok((inserted, skipped))
    }

    /// 向量检索：query 编码后与库中全部向量算余弦，取 top_k。
    /// 返回 (doc_id, score) 降序。库空或模型不可用时返回 None（回退关键词方案）。
    pub fn vector_search(
        conn: &rusqlite::Connection,
        query: &str,
        top_k: usize,
    ) -> Result<Option<Vec<(i64, f32)>>, String> {
        let em = match global_embedder() {
            Some(e) => e,
            None => return Ok(None),
        };
        let qv = em.embed(query, true).map_err(|e| e.to_string())?;

        // 独立作用域：stmt 与 rows 同生共死，避免借用冲突
        let mut scored: Vec<(i64, f32)> = {
            let mut stmt = conn
                .prepare("SELECT doc_id, vector FROM api_docs_embeddings")
                .map_err(|e| e.to_string())?;
            let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
            let mut scored = Vec::new();
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                let doc_id: i64 = row.get(0).map_err(|e| e.to_string())?;
                let blob: Vec<u8> = row.get(1).map_err(|e| e.to_string())?;
                let v = from_f32_le_bytes(&blob);
                scored.push((doc_id, cosine(&qv, &v)));
            }
            scored
        };

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(Some(scored))
    }

    /// 流式构建/重建向量索引（应用内后台线程调用）：
    /// 按 id 游标分批，每批“锁→读→解锁→编码→锁→写→解锁”，
    /// 编码耗时（CPU 推理）不持库锁，避免 20~30 分钟的长任务阻塞其他数据库操作。
    /// 语义与 build_index 一致：幂等跳过（已按当前模型全量建过）、换数据后全量重建。
    /// 返回 (已索引行数, 跳过行数)。
    pub fn build_index_streaming(
        state: &std::sync::Mutex<rusqlite::Connection>,
        batch_size: usize,
        on_progress: Option<super::EmbedProgressCb>,
    ) -> Result<(usize, usize), String> {
        let batch_size = batch_size.max(1);

        // 锁内读统计（短暂）：幂等判断——已按当前模型建过全量索引则跳过
        let (indexed, total) = {
            let conn = state.lock().map_err(|e| e.to_string())?;
            let indexed: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM api_docs_embeddings WHERE model = 'bge-small-zh-v1.5'",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let total: i64 = conn
                .query_row("SELECT COUNT(*) FROM api_docs", [], |r| r.get(0))
                .unwrap_or(0);
            (indexed.max(0) as usize, total.max(0) as usize)
        };
        if let Some(cb) = &on_progress {
            cb(&super::EmbedProgress {
                phase: "checking".to_string(),
                current: indexed,
                total,
                message: format!("检查索引状态（已索引 {indexed}/{total} 条）"),
            });
        }
        if indexed >= total && total > 0 {
            return Ok((0, total));
        }

        // 模型不可用直接报错（调用方提示用户），不静默降级
        let em = global_embedder()
            .ok_or_else(|| "embedding 模型不可用（缺少模型文件或加载失败）".to_string())?;

        // 清掉旧模型/旧版本向量，避免脏数据干扰（短暂锁）
        {
            let conn = state.lock().map_err(|e| e.to_string())?;
            conn.execute("DELETE FROM api_docs_embeddings", [])
                .map_err(|e| e.to_string())?;
        }

        let now = chrono::Utc::now().timestamp();
        let mut last_id: i64 = 0;
        let mut inserted = 0usize;
        let mut skipped = 0usize;
        let mut batch_no = 0usize;
        loop {
            // 锁内读一批（游标增量，避免一次拉全表）
            let rows: Vec<(i64, String, String, String, String)> = {
                let conn = state.lock().map_err(|e| e.to_string())?;
                let mut stmt = conn
                    .prepare(
                        "SELECT id, COALESCE(api_name,''), declaration, COALESCE(module,''), COALESCE(kit,'')
                         FROM api_docs WHERE id > ?1 ORDER BY id LIMIT ?2",
                    )
                    .map_err(|e| e.to_string())?;
                let collected = stmt
                    .query_map([last_id, batch_size as i64], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                    })
                    .map_err(|e| e.to_string())?
                    .collect::<Result<_, _>>()
                    .map_err(|e| e.to_string())?;
                collected
            };
            if rows.is_empty() {
                break;
            }

            // 锁外编码（CPU 推理，不持库锁）
            let texts: Vec<String> = rows
                .iter()
                .map(|(_, name, decl, module, kit)| super::doc_text(name, decl, module, kit))
                .collect();
            let vecs = match em.embed_batch(&texts, false) {
                Ok(v) => v,
                Err(e) => {
                    crate::utils::logger::log_event(
                        "embedding_index_batch_error",
                        serde_json::json!({ "error": e, "batch": rows.len() }),
                    );
                    skipped += rows.len();
                    last_id = rows.last().map(|r| r.0).unwrap_or(last_id);
                    batch_no += 1;
                    if let Some(cb) = &on_progress {
                        cb(&super::EmbedProgress {
                            phase: "embedding".to_string(),
                            current: inserted + skipped,
                            total,
                            message: format!("第 {batch_no} 批编码失败已跳过（{} 条）", rows.len()),
                        });
                    }
                    continue;
                }
            };

            // 锁内事务写入（短暂锁）
            {
                let conn = state.lock().map_err(|e| e.to_string())?;
                let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
                for (row, vec) in rows.iter().zip(vecs.iter()) {
                    let bytes = super::to_f32_le_bytes(vec);
                    if let Err(e) = tx.execute(
                        "INSERT OR REPLACE INTO api_docs_embeddings (doc_id, model, vector, created_at)
                         VALUES (?1, 'bge-small-zh-v1.5', ?2, ?3)",
                        rusqlite::params![row.0, bytes, now],
                    ) {
                        crate::utils::logger::log_event(
                            "embedding_index_insert_error",
                            serde_json::json!({ "error": e.to_string(), "doc_id": row.0 }),
                        );
                    }
                }
                tx.commit().map_err(|e| e.to_string())?;
            }
            inserted += vecs.len();
            last_id = rows.last().map(|r| r.0).unwrap_or(last_id);
            batch_no += 1;
            if let Some(cb) = &on_progress {
                cb(&super::EmbedProgress {
                    phase: "embedding".to_string(),
                    current: inserted + skipped,
                    total,
                    message: format!("第 {batch_no} 批完成（本批 {} 条）", rows.len()),
                });
            }
        }
        Ok((inserted, skipped))
    }
}

#[cfg(feature = "embedding")]
pub use imp::{
    build_index, build_index_streaming, global_embedder, model_dir, reset, set_resource_dir,
    vector_search, Embedder,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn l2_normalize_unit_vector() {
        let v = vec![3.0, 4.0];
        let n = l2_normalize(&v);
        assert!((n[0] - 0.6).abs() < 1e-6, "3-4-5 直角归一化为 0.6/0.8");
        assert!((n[1] - 0.8).abs() < 1e-6);
        let norm = (n[0] * n[0] + n[1] * n[1]).sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
    }

    #[test]
    fn l2_normalize_zero_vector() {
        let v = vec![0.0, 0.0];
        let n = l2_normalize(&v);
        assert_eq!(n, v, "零向量原样返回，避免除零");
    }

    #[test]
    fn model_files_present() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("embedding")
            .join("bge-small-zh-v1.5");
        let files = ["config.json", "tokenizer.json", "model.safetensors"];
        // 91MB 模型资源按发行流程注入且被 gitignore；纯源码 checkout 不携带它。
        // 一旦目录中出现任一模型文件，就必须是完整集合，防止打出半残安装包。
        if !files.iter().any(|f| dir.join(f).is_file()) {
            return;
        }
        for f in files {
            assert!(dir.join(f).is_file(), "缺少模型文件 {f}");
        }
    }

    #[test]
    fn f32_bytes_roundtrip() {
        let v = vec![1.0, -2.5, 0.25, 512.0];
        let bytes = to_f32_le_bytes(&v);
        assert_eq!(bytes.len(), v.len() * 4);
        assert_eq!(from_f32_le_bytes(&bytes), v);
    }

    #[test]
    fn doc_text_concatenates() {
        let s = doc_text(
            "windowSize",
            "getWindowProperties(): Promise<WindowProperties>;",
            "window",
            "ArkUI",
        );
        assert!(s.contains("windowSize"));
        assert!(s.contains("getWindowProperties"));
        assert!(s.contains("ArkUI"));
    }

    #[test]
    fn rrf_fuses_both_rankings() {
        // 向量检索和关键词结果各有一个独家命中 + 一个共有命中
        let vec_hits = vec![(1, 0.9), (2, 0.8)];
        let kw_hits = vec![(2, 3.0), (3, 2.0)];
        let fused = rrf_fuse(&vec_hits, &kw_hits, 3);
        assert_eq!(fused.len(), 3, "两路结果并集 3 条");
        // doc 2 两路都中 → 融合分最高
        assert_eq!(fused[0].0, 2, "双路命中应排第一");
        // 单路命中者按各自排名
        let ids: Vec<i64> = fused.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&1) && ids.contains(&3));
    }

    #[test]
    fn rrf_top_n_limits() {
        let vec_hits = vec![(1, 0.9), (2, 0.8), (3, 0.7)];
        let kw_hits: Vec<(i64, f32)> = vec![];
        let fused = rrf_fuse(&vec_hits, &kw_hits, 2);
        assert_eq!(fused.len(), 2);
    }
}
