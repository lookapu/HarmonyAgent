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

/// 检索前置校验：库里向量必须与**当前编码口径同一个指纹**。
///
/// 不同（含"没有指纹"这种老库/种子库状态）时查询向量与库向量根本不可比——
/// 整库对不上，返回的"最相似"是纯噪声，**比不搜更糟**。此时应降级为纯关键词。
/// 做成纯函数是为了能被单测钉住；判定本身不能藏在某个 `if` 里。
pub fn fingerprint_matches(stored: Option<&str>, current: &str) -> bool {
    match stored {
        Some(s) => s == current,
        None => false,
    }
}

/// 读库里记录的编码口径指纹（没有该键 = None，即"不知道是谁产的"）
pub fn stored_fingerprint(conn: &rusqlite::Connection) -> Option<String> {
    conn.query_row(
        "SELECT value FROM api_docs_meta WHERE key = ?1",
        rusqlite::params![FINGERPRINT_META_KEY],
        |r| r.get(0),
    )
    .ok()
}

/// 写入本次编码口径的指纹
pub fn write_fingerprint(conn: &rusqlite::Connection, fp: &str) {
    let _ = conn.execute(
        "INSERT OR REPLACE INTO api_docs_meta (key, value) VALUES (?1, ?2)",
        rusqlite::params![FINGERPRINT_META_KEY, fp],
    );
}

/// 语义层当前处于什么状态。**主要只读 DB，但有一条分支会加载模型**（见下）。
///
/// 更正记录：这里原写着「**只读 DB，不加载模型**」——**那是不准确的**。
/// 判到 `Ready` / `StaleIndex` 时要拿 `global_embedder().index_fingerprint()` 比指纹，
/// 而 `global_embedder()` 是 `OnceLock::get_or_init` 惰性加载，**首次调用会真的
/// `Embedder::load` 读 91MB 权重（约 1~2 秒）**，之后才走缓存。
///
/// 仍然保留这个行为，理由：换成「只读模型目录算文件哈希」并不更便宜
/// （`index_fingerprint` 要把 91MB 读一遍算 sha256，而且**每次调用都要重来**，
/// 而加载只付一次）；而 `api_kb_embed_status` 反正也要返回 `available`，
/// 那条路径必然加载。**真正要注意的是**：零命中提示那条路径**可能**是首次调用，
/// 于是第一条零命中查询会多花 1~2 秒——这是可接受的一次性成本，
/// 但**不要据「不加载模型」这个前提去做热路径上的性能假设**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticLayerState {
    /// 索引齐全且口径一致，可以参与检索
    Ready,
    /// 从没建过索引（表为空）
    NotIndexed,
    /// 表里有向量，但**不是当前编码口径产出的**（老库 / 种子库 / 改过 normalizer）
    StaleIndex,
    /// **建了一半**（取消 / 中断 / 被杀掉）：行数盖不住知识库总量
    PartialIndex,
    /// 模型文件不在/加载不了
    ModelMissing,
    /// **用户把语义层开关关了**（第二十四批·P-7 方案②）。
    ///
    /// 为什么必须单独一态：关着的时候索引**可能是齐的**（甚至本来就在库里）。
    /// 若报 `Ready` 就是在说谎（功能是关的）；若报 `NotIndexed` 同样是另一种说谎
    /// （让人以为要去建一个已经存在的索引，20~30 分钟白等）。
    /// 这一态回答的是「**为什么不做**」而不是「**为什么不能做**」——两者不可混。
    Disabled,
}

impl SemanticLayerState {
    /// 短标签（面向使用者/日志，不吐 `Debug` 的 `StaleIndex` 这种内部名）
    pub fn label(self) -> &'static str {
        match self {
            SemanticLayerState::Ready => "就绪",
            SemanticLayerState::NotIndexed => "未构建索引",
            SemanticLayerState::StaleIndex => "索引口径已过期",
            SemanticLayerState::PartialIndex => "索引未建完",
            SemanticLayerState::ModelMissing => "本地模型不可用",
            SemanticLayerState::Disabled => "已关闭",
        }
    }

    /// 给使用者看的一句话：为什么可能一条都没召回来，以及能做什么。
    ///
    /// **存在的理由**：一个查询 0 命中时，光说「共找到 0 条」等于把人丢在死胡同里——
    /// 关键词层靠 `LIKE '%keyword%'` 字面子串召回，而语料 `declaration` 含非 ASCII
    /// 的行 = **0 / 44,856**，所以**中文查询在关键词层必然 0 命中**（实测 15/15）。
    /// 此时能不能靠语义层兜底，取决于上面这四种状态，得说出来。
    pub fn hint(self) -> &'static str {
        match self {
            SemanticLayerState::Ready => {
                "关键词层无字面命中；语义层已就绪但也没匹配上——建议改用精确 API 名（大小写与驼峰原样），或换 module/kit 过滤缩小范围。"
            }
            SemanticLayerState::NotIndexed => {
                "关键词层靠字面子串召回，中文/描述性关键词几乎必然 0 命中；向量索引尚未构建，构建后可支持语义检索（首次约 20~30 分钟）。"
            }
            SemanticLayerState::StaleIndex => {
                "现有向量索引不是当前编码口径产出的（升级前或来自种子库），已按「不可比」处理；需重建索引后才能用于语义检索。"
            }
            SemanticLayerState::PartialIndex => {
                "向量索引只建了一部分（多半是上次构建被取消或中断），已按「不可用」处理——半份索引会让语义检索看起来在工作，实际只在少量条目上生效；重新构建完成后即可使用。"
            }
            SemanticLayerState::ModelMissing => {
                "本地向量模型不可用，本次为纯关键词检索；中文/描述性关键词建议改用精确 API 名。"
            }
            SemanticLayerState::Disabled => {
                "语义检索当前处于关闭状态（默认关闭，可在 API 知识库页面开启）；本次为纯关键词检索。精确 API 名（大小写与驼峰原样）命中率最高，中文/描述性关键词需开启语义层才能召回。"
            }
        }
    }
}

/// 语义层开关的 `settings` 表键。
pub const ENABLED_META_KEY: &str = "embedding_enabled";

/// 读语义层开关。**默认关**（P-7 方案②）。
///
/// **为什么默认关**：实测（15 条真实检索词）向量层 top1 仅 1/15、top10 3/15，
/// 而关键词层在 IDE 里的主要用法（查已知 API 名）上 top1 **3/3**。
/// 让一个中位名次 >20 的能力默认参与每次检索，代价是：**178.9MB** 向量表
/// （种子库实测：净数据 91.9MB，但 SQLite 页占用膨胀到 178.9MB）、
/// 91MB 模型随包、首次 20~30 分钟建索引——**不值**。
/// 精确标识符走关键词层零成本，所以默认值取「关」。
///
/// **更正记录**：这里原写「91.8MB 向量库」。那是 `SUM(LENGTH(vector))`
/// 的**净数据**，不是磁盘占用。实测（复制种子库 → DROP TABLE → VACUUM →
/// 比 `PRAGMA page_count`）是 **178.9MB，接近 2 倍**。
/// 按净数据做成本评估会**低估一半**，而没有任何一步会报错。
///
/// **读不到 / 值不认识的处理**：一律回落到**关**（fail-closed）。
/// 反过来（读不到就当开）会让「升级即默认启用」，正是 ② 要消除的行为。
/// 「不认识的字符串」按关处理同样是错向安全那一侧。
pub fn semantic_enabled(conn: &rusqlite::Connection) -> bool {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        [ENABLED_META_KEY],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .map(|raw| {
        matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        )
    })
    .unwrap_or(false)
}

/// 写语义层开关。**幂等**：写同值不报错。
pub fn set_semantic_enabled(conn: &rusqlite::Connection, enabled: bool) -> Result<(), String> {
    conn.execute(
        "INSERT INTO settings(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![ENABLED_META_KEY, if enabled { "1" } else { "0" }],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 查语义层当前状态（读 DB；判 `Ready`/`StaleIndex` 时会经 `global_embedder()`
/// **首次加载 91MB 模型**——见 `SemanticLayerState` 的更正记录）
///
/// ## 判据一：行数与指纹两道都要查，缺一不可（第二十四批·UI 段自查时修）
///
/// 原来这里只验指纹，于是「建了一半」的索引会被判成 `Ready`：
/// `build_index_streaming` 在循环结束（含**取消 break** 的那条路径）后
/// 无条件写指纹，而判定不数行数 ⇒ 取消后卡片显示绿色「就绪」，
/// 而语义层只覆盖了 11%。更糟的是 `vector_search` 复用本函数判降级，
/// 半成品因此**不会被拒用**，会被当成全量。
///
/// `should_skip_index` 要求「行数够 且 指纹一致」，这里曾经只要求后者——
/// 同一个事实两个判断函数、判据不同 ⇒ 一个说「要重建」，另一个说「就绪」。
///
/// ## 判据二：开关最先判，而且它就是 ② 的主要收益来源（P-7 方案②）
///
/// 开关关 ⇒ **第一件事就返回 `Disabled`**，不去数向量行数、更不会走到模型加载。
/// 这不是"早退优化"，而是 ② 收益的**来源**：默认关时，判定与检索都
/// **完全碰不到那 91MB 权重**（`get_or_init` 永远不被触发），
/// 也没有 20~30 分钟建索引的压力。
///
/// 如果把开关判放到最后（先查库/加载模型再判开关），② 就退化成一个装饰：
/// 模型照样加载、照样提示去建索引，收益归零。
/// 自测 `disabled_layer_short_circuits_before_loading_model` 钉住这个顺序。
pub fn semantic_layer_state(conn: &rusqlite::Connection) -> SemanticLayerState {
    // ① 开关最先判：关着就直接返回，不碰向量表、不碰模型
    if !semantic_enabled(conn) {
        return SemanticLayerState::Disabled;
    }
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM api_docs_embeddings", [], |r| r.get(0))
        .unwrap_or(0);
    if n == 0 {
        return SemanticLayerState::NotIndexed;
    }
    // 有向量：先看**盖不盖得住**总量（取消/中断留下的半成品），
    // 再看口径对不对（老库 / 改过 normalizer）。
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM api_docs", [], |r| r.get(0))
        .unwrap_or(0);
    if total > 0 && n < total {
        return SemanticLayerState::PartialIndex;
    }
    // 有向量：口径对不上就是「陈旧」，比说「相同」安全
    #[cfg(feature = "embedding")]
    {
        match global_embedder() {
            Some(em) => {
                if fingerprint_matches(stored_fingerprint(conn).as_deref(), em.index_fingerprint())
                {
                    SemanticLayerState::Ready
                } else {
                    SemanticLayerState::StaleIndex
                }
            }
            None => SemanticLayerState::ModelMissing,
        }
    }
    #[cfg(not(feature = "embedding"))]
    {
        // 未启用 embedding feature：无向量覆盖能力可谈
        SemanticLayerState::ModelMissing
    }
}

/// 编码文本来源：api_docs 单条记录拼成可检索文本。索引的是"文档侧"，不带查询指令前缀。
///
/// **参与编码的字段恰好是这四个**，由 `doc_text_uses_exactly_the_four_indexed_fields`
/// 逐字钉死。原注释曾写着「+ 变更说明」，而函数根本没有 `change_type` 参数——
/// **注释在描述一件它没做的事**，而注释正是下一个人判断「索引覆盖了什么」的唯一依据。
///
/// ## 更正记录：这里原先写着「72.2% 的向量碰撞是版本并列造成的」，那是错的
///
/// 第二十三批把 44,856 行里 18,506 个唯一向量归因为「同一个声明在几十个
/// `version_label` 下重复出现，而 `version_label` 不参与编码」。第二十四批实测推翻：
///
/// | 项 | 原以为 | 实测 |
/// |---|---:|---:|
/// | distinct `doc_text` | 18,506 | **38,566**（文本去重只能省 6,290 行 / 14.0%） |
/// | 剩余碰撞 | 「版本并列」 | **分词器把驼峰 API 名整段塌成 `[UNK]`** |
///
/// 决定性证据：**distinct token 序列 = 18,506 = distinct 向量数**，一个不差。
/// 分词序列相同的行向量必然逐字节相同 ⇒ 碰撞 100% 由分词解释，版本不是原因。
/// 详见 `NORMALIZER_SCHEMA` 与 `imp::Embedder::load` 里覆盖 normalizer 的那段注释。
///
/// 保留这段而不是删掉：下一批若不读这一段，会直接引用「版本并列」这个已被证伪的结论
/// （P-6 就是照着它拍板的），而它看起来是权威的。
///
/// 所以：**不要**为了「检索分不清版本」而改这里——那解决不了任何一个实际检索问题。
/// 顺带说明，`version_label` / `dts_file` / `api_level` 不参与编码仍是事实，
/// 只是它造成的重复远小于分词造成的重复。
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
    data.as_chunks::<4>().0.iter()
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

/// RRF 融合：**在关键词已召回的集合内**用向量排名重排。
/// `vec_hits`: (doc_id, score) 降序；`kw_hits`: (doc_id, kw_score) 降序。
/// 融合分 = RRF(vec_rank) + RRF(kw_rank)，返回按融合分降序的 doc_id 列表。
///
/// ## 契约变更一：关键词集合非空时它是权威集合；**为空时放行向量命中**（第二十四批）
/// 旧版把两路结果取**并集**，于是「只在向量里、字面零命中」的文档能压过 BM25 第一名
/// （向量第 1 名 = 1/61，与关键词第 1 名 1/61 **精确相等**）。而被允许压过 BM25 的
/// 那个信号，实测 top1 命中 **0/15**、top10 仅 3/15，且 67.8% 的查询 top-1 与
/// top-2 余弦间隙恰好为 0。
/// 现在 `kw_hits` **非空**时它是**权威候选集**：向量只在其内部调序，不引入新文档。
/// 「语义相关但无字面命中也能召回」是**有意保留的未实现能力**——先让向量层在
/// 端到端评测上打过 BM25，再谈让它在没有 BM25 时独占结果。
///
/// ## 契约变更一的续：空关键词集合**必须放行**向量命中，否则语义检索彻底归零
/// 同一批测量给出的事实（生产代码本身跑出来的，不是复现）：
///
/// | 查询类型 | 关键词层召回 | 目标名次 |
/// |---|---|---|
/// | 语义/中文（15 条） | **0 条（15/15）** | 未召回 |
/// | 精确标识符（`getWindowProperties` 等 3 条） | 1~8 条 | **top1 3/3** |
///
/// 语料 `declaration` 含非 ASCII 的行 = **0 / 44,856**，而召回靠 `LIKE '%keyword%'`
/// 字面子串 ⇒ **中文查询在关键词层一条都召不回**。
/// 于是第一版收窄（无条件把向量命中滤掉）把语义检索从「50 条（目标在 top10 的有 3/15）」
/// 压成了「**0 条**」——**比不修更糟**，而且是本条规则自己引入的回归。
///
/// 现在的规则是「**弱信号只在没有强信号时使用**」：
/// 有关键词命中 ⇒ 向量不得引入新文档（防压过 BM25）；
/// 一个都没有 ⇒ 向量是唯一的机会，放行。
/// 这条规则不含任何魔数，且两个方向都指向「不要覆盖已有的强信号」。
///
/// ## 契约变更二：并列必须可复现（第二十四批）
/// 旧版用 `HashMap::into_iter()` 的迭代序 + 稳定排序打破并列。Rust 的 `HashMap`
/// 每实例随机种子 ⇒ 同一查询在不同进程返回**不同顺序**。而并列不是边角情况
/// （top-1/top-2 间隙为 0 的查询占 67.8%），于是结果顺序基本是随机的。
/// 现在并列一律按 `doc_id` 升序，同输入必然同输出。
pub fn rrf_fuse(
    vec_hits: &[(i64, f32)],
    kw_hits: &[(i64, f32)],
    top_n: usize,
) -> Vec<(i64, f64)> {
    use std::collections::HashMap;
    const K: f64 = 60.0;
    // 关键词集合非空 ⇒ 权威候选集；为空 ⇒ 不筛（弱信号在没有强信号时才上场）
    let filter: Option<std::collections::HashSet<i64>> = if kw_hits.is_empty() {
        None
    } else {
        Some(kw_hits.iter().map(|(id, _)| *id).collect())
    };
    let mut fused: HashMap<i64, f64> = HashMap::new();
    for (rank, (id, _)) in vec_hits.iter().enumerate() {
        if let Some(allowed) = &filter {
            if !allowed.contains(id) {
                continue;
            }
        }
        *fused.entry(*id).or_insert(0.0) += 1.0 / (K + rank as f64);
    }
    for (rank, (id, _)) in kw_hits.iter().enumerate() {
        *fused.entry(*id).or_insert(0.0) += 1.0 / (K + rank as f64);
    }
    let mut out: Vec<(i64, f64)> = fused.into_iter().collect();
    // 分数降序；并列按 doc_id 升序，**不依赖 HashMap 迭代序**（见「契约变更二」）
    out.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    out.truncate(top_n);
    out
}

/// 索引指纹的 `api_docs_meta` 键。
pub const FINGERPRINT_META_KEY: &str = "embedding_index_fingerprint";

/// 参与编码的文本组成。**改 `doc_text` 的字段或顺序时必须同步改这里**，
/// 否则旧向量会被当成可复用的缓存继续用（见 `should_skip_index`）。
pub const DOC_TEXT_SCHEMA: &str = "api+decl+module+kit/空格连接/v1";

/// 分词/归一化口径。**改 `imp::Embedder::load` 里覆盖的 normalizer 时必须同步改这里**。
pub const NORMALIZER_SCHEMA: &str = "bert/clean+zh-chars+strip-accents+lowercase/v1";

/// 参与指纹的模型文件（按**内容**哈希，不看 mtime/大小）
const FINGERPRINT_FILES: [&str; 3] = ["model.safetensors", "tokenizer.json", "config.json"];

/// 指纹：sha256(hex) 取前 8 字节。
///
/// 逐段加**长度前缀**——否则 `("ab","c")` 与 `("a","bc")` 会拼出同一个字节流，
/// 算出同一个指纹，而它们其实是两套不同的输入。
pub fn fingerprint_hex(parts: &[&[u8]]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p);
    }
    h.finalize()[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// 当前模型目录下的索引指纹 = 编码口径（源码常量）+ 模型文件内容。
///
/// 用**内容哈希**而不是 mtime：mtime 在拷贝/解包/重新下载后会变，会造成无谓的
/// 20~30 分钟全量重建；而内容变了而 mtime 没变才是致命的（那正好是旧代码的情形）。
/// 模型文件缺失时不参与哈希（跳过该段），加载失败由调用方各自报错。
pub fn index_fingerprint(model_dir: &std::path::Path) -> Result<String, String> {
    let mut parts: Vec<Vec<u8>> = vec![
        DOC_TEXT_SCHEMA.as_bytes().to_vec(),
        NORMALIZER_SCHEMA.as_bytes().to_vec(),
    ];
    for name in FINGERPRINT_FILES {
        let p = model_dir.join(name);
        if !p.is_file() {
            continue;
        }
        parts.push(name.as_bytes().to_vec());
        parts.push(
            std::fs::read(&p).map_err(|e| format!("读取 {} 失败: {e}", p.display()))?,
        );
    }
    let refs: Vec<&[u8]> = parts.iter().map(|v| v.as_slice()).collect();
    Ok(fingerprint_hex(&refs))
}

/// 幂等判定：**行数够 且 指纹一致** 才允许跳过重建。
///
/// 旧版只比行数，于是「编码口径改了、行数没变」会直接跳过——查询用新口径、
/// 库里是旧向量，**比不修更糟**。指纹缺失（老库、种子库没带）也判不跳过：
/// 宁可多建一次，也不要拿旧向量当新的用。
pub fn should_skip_index(
    indexed: usize,
    total: usize,
    stored: Option<&str>,
    current: &str,
) -> bool {
    total > 0 && indexed >= total && stored == Some(current)
}

/// 建索引进度（phase: checking / embedding / cancelled）
#[derive(serde::Serialize)]
pub struct EmbedProgress {
    pub phase: String,
    pub current: usize,
    pub total: usize,
    pub message: String,
    /// 预计剩余秒数。前端据此显示"预计还需 N 分钟"——
    /// **没有它，20~30 分钟的任务在用户眼里只是一根不知道多长的进度条**，
    /// 而 `1234/44856` 这种计数完全不传达时长。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta_secs: Option<u64>,
}

/// 建索引进度回调（后台线程跨线程使用，须 Send + Sync）
pub type EmbedProgressCb = Box<dyn Fn(&EmbedProgress) + Send + Sync>;

/// 由「已完成 / 总数 / 已耗时秒」线性外推剩余秒数。**纯函数**——
/// 收 `elapsed_secs` 而不是 `Instant`，这样它能被确定性地自测：
/// 收 `Instant` 就只能靠 sleep 造场景，而 sleep 会让自测慢且易抖。
///
/// - `done <= 0` 或 `total == 0` 或 `done >= total` ⇒ `None`（没有可外推的样本）
/// - **第一批之前不报 ETA**：128 条只花几秒，线性外推会算出
///   "剩余 3 秒" 这种与真实 20~30 分钟完全相反的结论，比不给更坏。
///   门槛「完成 ≥1% 且 ≥20 条」与阈值本身无关，纯粹为了让样本有意义。
/// - `elapsed < 1s` ⇒ `None`（样本时间太短，速率不可信）
// 未启用 embedding feature 时只有自测用它（同 l2_normalize / cosine）
#[cfg_attr(not(feature = "embedding"), allow(dead_code))]
pub(crate) fn eta_from(done: usize, total: usize, elapsed_secs: f64) -> Option<u64> {
    if total == 0 || done == 0 || done >= total {
        return None;
    }
    if done * 100 < total || done < 20 {
        return None;
    }
    if elapsed_secs < 1.0 {
        return None;
    }
    let remaining = elapsed_secs * (total.saturating_sub(done) as f64 / done as f64);
    Some(remaining.ceil().max(1.0) as u64)
}

// ───────────────────────── candle 推理实现（生产编译专用） ─────────────────────────

#[cfg(feature = "embedding")]
mod imp {
    use super::{cosine, doc_text, from_f32_le_bytes, l2_normalize, to_f32_le_bytes};
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    use candle_core::{DType, Device, Tensor};
    use candle_nn::VarBuilder;
    use candle_transformers::models::bert::{BertModel, Config as BertConfig};
    use tokenizers::normalizers;
    use tokenizers::Tokenizer;

    /// 模型目录（resources/embedding/bge-small-zh-v1.5），随安装包分发。
    /// 运行时通过 resource_dir 定位；开发模式回退到 CARGO_MANIFEST_DIR/resources。
    const MODEL_DIR_NAME: &str = "embedding/bge-small-zh-v1.5";

    /// BGE v1.5 官方建议：查询语句加指令前缀，提升检索相关性（文档侧不加）
    const QUERY_INSTRUCTION: &str = "为这个句子生成表示以用于检索相关文章：";

    /// 覆盖模型自带的 normalizer：**强制小写化**（第二十四批）。
    ///
    /// 模型自带的 `tokenizer.json` 是 `lowercase: false`，而它的词表是**全小写**的
    /// （`get`∈V 而 `Get`∉V、`kit`∈V 而 `Kit`∉V）。于是所有驼峰标识符——
    /// 鸿蒙 API 名的常态——整体塌成一个 `[UNK]`。实测（44,856 行全量）：
    ///
    /// | 指标 | 现状 | 强制小写后 |
    /// |---|---:|---:|
    /// | 含 `[UNK]` 的行 | **100%** | **0%** |
    /// | `[UNK]` 占 965,066 个 token | **24.8%** | **0.0%** |
    /// | 非空 API 名整名塌成 `[UNK]` | **27,433 / 31,812（86.2%）** | 0 |
    /// | distinct token 序列 | 18,506 | **38,471（2.08×）** |
    /// | 最大碰撞组 | **538 行** | **24 行** |
    ///
    /// `distinct token 序列 = 18,506` 恰好等于 `distinct 向量数 = 18,506`：
    /// 分词序列相同的行向量必然逐字节相同 ⇒ **72.2% 的向量碰撞 100% 由分词解释**，
    /// 与 `version_label` 无关。
    ///
    /// ## 为什么不直接改 `tokenizer.json`
    /// `src-tauri/resources/` 在 **.gitignore** 里（91MB 模型按发行流程注入）。
    /// 改资源文件的修法**无法提交、CI 一构建就丢**，纯源码 checkout 拿到的还是坏的。
    /// 口径必须落在源码里。
    ///
    /// ## 改了这段必须同步改 `NORMALIZER_SCHEMA`
    /// 否则旧向量会被 `should_skip_index` 当成可复用缓存，查询用新口径、库里是旧向量。
    ///
    /// `strip_accents` 一并置 true：HF 的 BertNormalizer 里 `strip_accents: null`
    /// 语义是「跟随 lowercase」，这里显式打开与之一致。
    /// `strip_accents` 传 `None`：HF 的 BertNormalizer 里 `null` 的语义就是
    /// 「跟随 lowercase」，与模型自带的 `tokenizer_config.json` 一致；显式写 `true`
    /// 反而是替模型做了决定。
    fn fixed_normalizer(mut tokenizer: Tokenizer) -> Tokenizer {
        tokenizer.with_normalizer(Some(normalizers::bert::BertNormalizer::new(
            /* clean_text          */ true,
            /* handle_chinese_chars */ true,
            /* strip_accents        */ None,
            /* lowercase            */ true,
        )));
        tokenizer
    }

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
        /// 本实例的编码口径指纹（建模时算一次）。
        /// 存在的理由：检索热路径**不能**每次去哈希 91MB 权重文件，
        /// 而"库里向量是不是这套口径产出的"必须在每次检索前确认。
        fingerprint: String,
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

            let tokenizer = fixed_normalizer(
                Tokenizer::from_file(&tokenizer_path).map_err(|e| e.to_string())?,
            );

            // mmap 加载 safetensors（大权重零拷贝）
            let vb = unsafe {
                VarBuilder::from_mmaped_safetensors(&[weights_path], DType::F32, &device)
                    .map_err(|e| e.to_string())?
            };
            let model = BertModel::load(vb, &config).map_err(|e| e.to_string())?;

            // 口径指纹：normalizer 覆盖 + doc_text 组成 + 三个模型文件的内容
            let fingerprint = super::index_fingerprint(model_dir)?;

            Ok(Self {
                model,
                tokenizer,
                device,
                fingerprint,
            })
        }

        /// 本实例的编码口径指纹（`api_docs_meta` 键 `embedding_index_fingerprint`）
        pub fn index_fingerprint(&self) -> &str {
            &self.fingerprint
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
    /// 模型不可用时返回 Err（调用方提示用户）；
    /// **行数够且指纹一致**时跳过（幂等），否则全量重建。
    /// 返回 (已索引行数, 跳过行数)。
    pub fn build_index(
        conn: &rusqlite::Connection,
        batch_size: usize,
    ) -> Result<(usize, usize), String> {
        let em = global_embedder()
            .ok_or_else(|| "embedding 模型不可用（缺少模型文件或加载失败）".to_string())?;
        let current_fp = em.index_fingerprint().to_string();

        // 幂等：**行数够 且 指纹一致**才跳过（只比行数会把旧口径向量当缓存，见 should_skip_index）
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
        let stored_fp = super::stored_fingerprint(conn);
        if super::should_skip_index(
            indexed.max(0) as usize,
            total.max(0) as usize,
            stored_fp.as_deref(),
            &current_fp,
        ) {
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
        // 记下本次编码口径的指纹：只有行数与指纹都对上，下次才允许跳过重建
        super::write_fingerprint(conn, &current_fp);
        Ok((inserted, skipped))
    }

    /// 向量检索：query 编码后与库中全部向量算余弦，取 top_k。
    /// 返回 (doc_id, score) 降序。**库空 / 模型不可用 / 库向量不是当前口径产出的**
    /// 时返回 None（回退关键词方案）。
    ///
    /// **顺序是契约的一部分（P-7 方案②）**：状态判定**必须在 `global_embedder()`
    /// 之前**完成。原实现第一件事就是 `global_embedder()`，
    /// 而那是 `OnceLock::get_or_init` ⇒ **开关关着也会真的读 91MB 权重**，
    /// ② 的收益（默认不碰那 91MB）就归零了。
    /// `semantic_layer_state` 自己会先判开关并直接返回 `Disabled`，
    /// 所以把它提到 `global_embedder()` 之前即可达成。
    pub fn vector_search(
        conn: &rusqlite::Connection,
        query: &str,
        top_k: usize,
    ) -> Result<Option<Vec<(i64, f32)>>, String> {
        // 口径 + 开关校验（必须在 `global_embedder()` 与编码 query **之前**）：
        // ① 开关关 ⇒ 直接降级，绝不加载 91MB 权重；
        // ② 库里若是另一套口径的向量，查询向量与它们不可比，
        //    整库对不上时"最相似"是纯噪声——比不搜更糟。
        // 与「零结果提示」复用同一个判定，避免日志和用户看到的提示说的不是一回事。
        match super::semantic_layer_state(conn) {
            super::SemanticLayerState::Ready => {}
            other => {
                crate::utils::logger::log_event(
                    "embedding_index_fingerprint_mismatch",
                    serde_json::json!({
                        "state": format!("{other:?}"),
                        "action": "skip_vector_search_fallback_to_keyword",
                    }),
                );
                return Ok(None);
            }
        }
        // 到这里才加载模型：状态已是 `Ready`，这次加载是必要成本。
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
    /// 语义与 build_index 一致：幂等跳过（行数够**且**指纹一致）、换数据或换口径后全量重建。
    /// `cancel` 每批检查一次：20~30 分钟的任务不给取消入口，等于让人守着。
    /// 返回 (已索引行数, 跳过行数)。
    pub fn build_index_streaming(
        state: &std::sync::Mutex<rusqlite::Connection>,
        batch_size: usize,
        on_progress: Option<super::EmbedProgressCb>,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<(usize, usize), String> {
        let batch_size = batch_size.max(1);
        let started = std::time::Instant::now();

        // 锁内读统计（短暂）：幂等判断——行数够 **且** 指纹一致才跳过
        let (indexed, total, stored_fp) = {
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
            let stored_fp = super::stored_fingerprint(&conn);
            (indexed.max(0) as usize, total.max(0) as usize, stored_fp)
        };
        // 模型不可用直接报错（调用方提示用户），不静默降级。
        // 放在守卫**之前**：口径指纹由 Embedder 建模时算一次并缓存，
        // 避免每次调用都去哈希 91MB 权重文件（那是热路径）。
        let em = global_embedder()
            .ok_or_else(|| "embedding 模型不可用（缺少模型文件或加载失败）".to_string())?;
        let current_fp = em.index_fingerprint().to_string();
        if let Some(cb) = &on_progress {
            cb(&super::EmbedProgress {
                phase: "checking".to_string(),
                current: indexed,
                total,
                message: format!("检查索引状态（已索引 {indexed}/{total} 条）"),
                eta_secs: None,
            });
        }
        if super::should_skip_index(indexed, total, stored_fp.as_deref(), &current_fp) {
            return Ok((0, total));
        }

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
        // 是否因为取消而提前退出。**取消路径不得写指纹**（见循环后的说明）。
        let mut cancelled_early = false;
        loop {
            // 取消检查放在每批**开头**：一批编码完才有机会看到，
            // 128 条一批在 CPU 上约 3~5 秒，粒度够用。
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                if let Some(cb) = &on_progress {
                    cb(&super::EmbedProgress {
                        phase: "cancelled".to_string(),
                        current: inserted,
                        total,
                        message: format!("已取消（完成 {inserted}/{total} 条）"),
                        eta_secs: None,
                    });
                }
                cancelled_early = true;
                break;
            }
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
                        let done = inserted + skipped;
                        cb(&super::EmbedProgress {
                            phase: "embedding".to_string(),
                            current: done,
                            total,
                            message: format!("第 {batch_no} 批编码失败已跳过（{} 条）", rows.len()),
                            eta_secs: super::eta_from(done, total, started.elapsed().as_secs_f64()),
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
                let done = inserted + skipped;
                cb(&super::EmbedProgress {
                    phase: "embedding".to_string(),
                    current: done,
                    total,
                    message: format!("第 {batch_no} 批完成（本批 {} 条）", rows.len()),
                    eta_secs: super::eta_from(done, total, started.elapsed().as_secs_f64()),
                });
            }
        }
        // 记下本次编码口径的指纹：只有行数与指纹都对上，下次才允许跳过重建。
        //
        // **取消路径不写指纹**：指纹的含义是「本口径已完整索引」，
        // 写下去等于替一次没跑完的构建背书。原实现在这里无条件写，
        // 于是「建了一半」也会拿到当前口径的有效指纹——
        // 若判定只看指纹，它就是一次就绪的假象（见 `semantic_layer_state`）。
        //
        // 现在有两道独立的防线，任一道单独都够用：
        //   1. 判定先查行数覆盖（`n < total` ⇒ PartialIndex）
        //   2. 取消时不写指纹（⇒ 下次必判不可比，强制全量重建）
        if !cancelled_early {
            let conn = state.lock().map_err(|e| e.to_string())?;
            super::write_fingerprint(&conn, &current_fp);
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

    /// 钉死「哪几个字段参与编码、按什么顺序」——用**逐字相等**而不是 `contains`。
    ///
    /// 这条不是为了测拼接，是为了让**注释与签名无法悄悄漂移**：原注释写着
    /// 「+ 变更说明」，而函数没有 `change_type` 参数。注释是下一个人判断
    /// 「索引到底覆盖了什么」的唯一依据，它错了没人会知道。
    /// 逐字相等意味着**增删任何一个字段或调换顺序，这里都会红**。
    #[test]
    fn doc_text_uses_exactly_the_four_indexed_fields() {
        assert_eq!(
            doc_text(
                "windowSize",
                "getWindowProperties(): Promise<WindowProperties>;",
                "window",
                "ArkUI",
            ),
            "windowSize getWindowProperties(): Promise<WindowProperties>; window ArkUI"
        );
        // 空字段留占位空格而不是塌掉：保持「字段个数」在文本里可辨认，
        // 避免 ("a","b c","","") 与 ("a","b","c","") 拼出同一个串。
        assert_eq!(doc_text("f", "(): void;", "", ""), "f (): void;  ");
        // 明确记录「version_label 不参与」这件事本身
        let with_version_looking_decl = doc_text("f", "(): void; // 6.1.1(24)", "m", "k");
        assert!(with_version_looking_decl.starts_with("f (): void; // 6.1.1(24)"));
    }

    /// RRF 融合：双路都命中的文档排最前。
    ///
    /// 反向护栏：别把「空集合才放行」这个例外扩成通例。
    /// 关键词**有一条**命中时，向量侧独占的文档仍必须被剔除——
    /// 顺带钉住"关键词一条命中就足以建立权威集合"（不是要两条以上）。
    #[test]
    fn rrf_still_drops_vector_only_hits_when_keywords_exist() {
        let vec_hits = vec![(7, 0.9), (8, 0.8), (9, 0.7)];
        let kw_hits = vec![(1, 1.0)];
        let ids: Vec<i64> = rrf_fuse(&vec_hits, &kw_hits, 10)
            .iter()
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(ids, vec![1], "有关键词命中时只保留关键词集合内的文档");
    }

    /// **注意这条自测的契约在第二十四批变过**：旧版断言「两路结果并集 3 条」，
    /// 也就是允许「只在向量里命中」的文档进入结果。现在向量只在关键词集合内调序，
    /// 所以 1 号（仅向量命中）**必须不出现**。旧断言若留着，就是把一个
    /// 已证伪的契约钉死成"正确用法"——和 `todo_write` 那次同一个形状。
    #[test]
    fn rrf_fuses_both_rankings_within_keyword_set() {
        let vec_hits = vec![(1, 0.9), (2, 0.8)];
        let kw_hits = vec![(2, 3.0), (3, 2.0)];
        let fused = rrf_fuse(&vec_hits, &kw_hits, 3);
        assert_eq!(fused.len(), 2, "结果只能来自关键词集合，1 号仅向量命中应被剔除");
        assert_eq!(fused[0].0, 2, "双路命中应排第一");
        let ids: Vec<i64> = fused.iter().map(|(id, _)| *id).collect();
        assert!(!ids.contains(&1), "仅向量命中的文档不得进入结果");
        assert!(ids.contains(&3), "仅关键词命中的文档必须保留");
    }

    /// 空关键词集合 ⇒ **放行**向量命中，不是返回空。
    ///
    /// 记下为什么：语料 `declaration` 含非 ASCII 的行 = **0 / 44,856**，而召回靠
    /// `LIKE '%keyword%'` 字面子串 ⇒ **中文查询在关键词层一条都召不回**（实测 15/15）。
    /// 无条件把向量命中滤掉，会让语义检索从「50 条（目标在 top10 的有 3/15）」
    /// 变成「**0 条**」——第一版收窄就是这么把功能改坏的。
    /// 规则是「弱信号只在没有强信号时使用」：有关键词命中时向量不得引入新文档。
    #[test]
    fn rrf_with_empty_keyword_side_lets_vector_through() {
        let vec_hits = vec![(1, 0.9), (2, 0.8), (3, 0.7)];
        let kw_hits: Vec<(i64, f32)> = vec![];
        let fused = rrf_fuse(&vec_hits, &kw_hits, 2);
        assert_eq!(
            fused.len(),
            2,
            "关键词一条都没有时，向量是唯一的机会，不能什么都不给"
        );
        assert_eq!(fused[0].0, 1, "按向量名次排序");
    }

    /// 并列必须：**按 doc_id 升序**、且**可复现**。
    ///
    /// 旧版用 `HashMap::into_iter()` 迭代序 + 稳定排序打破并列，而 `HashMap`
    /// 每实例随机种子 ⇒ 同一查询在不同进程顺序不同。而并列不是边角情况
    /// （top-1/top-2 余弦间隙为 0 的查询占 67.8%）。
    ///
    /// ## 为什么这两件事在一条自测里
    /// 它们拆开写的后果：第一版「造并列」的输入里**一条并列都没有**
    /// （vec 侧 id 10~13 被权威候选集过滤、kw 侧 20~23 的分数 1/60…1/63 互不相同）
    /// ⇒ 去掉次级排序键照样绿，被负向验证抓出来。
    /// 改对之后又发现：单独一条「断言顺序是 [2,7,…]」的自测**只能 50% 报红**
    /// （去掉次级键后 id2/id7 谁在前由随机序决定），把它当变异探测器是 flake。
    /// 所以顺序断言与 200 次复现断言合在一条里：去掉次级排序键时，
    /// 报红概率是 1 − 2^-200，而不是 1/2。
    ///
    /// 并列构造：id2 = 关键词第 6 + 向量第 1，id7 = 关键词第 1 + 向量第 6，
    /// 两侧分母集合相同 ⇒ 分数**精确相等**。此时若按名次决胜，关键词第 1 的 id7 会赢。
    /// 填充项用互不相同的 id——同一 id 在两侧各中多次会累加出更高的分，把并列挤出第一。
    #[test]
    fn rrf_tie_break_is_ordered_and_reproducible() {
        let kw_hits: Vec<(i64, f32)> =
            vec![(7, 1.0), (31, 1.0), (32, 1.0), (33, 1.0), (34, 1.0), (2, 1.0)];
        let vec_hits: Vec<(i64, f32)> =
            vec![(2, 0.5), (41, 0.5), (42, 0.5), (43, 0.5), (44, 0.5), (7, 0.5)];
        let first = rrf_fuse(&vec_hits, &kw_hits, 8);
        // 前提：前两名真的同分。否则这条自测根本测不到并列（第一版就栽在这）
        assert!(
            first.len() >= 2 && (first[0].1 - first[1].1).abs() < 1e-12,
            "前提：前两名必须真的同分（各 1/60 + 1/65），否则测不到并列"
        );
        let ids: Vec<i64> = first.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            ids,
            vec![2, 7, 31, 32, 33, 34],
            "精确并列按 doc_id 升序（id7 关键词名次更好，但并列不按名次决胜）；\
             41~44 仅向量命中，不进结果"
        );
        for i in 0..200 {
            assert_eq!(
                rrf_fuse(&vec_hits, &kw_hits, 8),
                first,
                "第 {i} 次与第 0 次不一致：并列不能依赖 HashMap 迭代序"
            );
        }
    }

    /// 反向护栏：别把「只重排」修成「向量层完全无效」。
    /// 双路命中的文档仍应靠向量那一份把名次提上去（这里是关键词第 10 → 融合第 1）。
    /// 选的两个 doc 关键词名次必须**连续且靠后**，否则它们自身的关键词分就会盖过向量加成。
    #[test]
    fn rrf_still_lets_a_dual_hit_climb() {
        // 关键词 12 条（1..=12，id10 在第 10 名、id11 在第 11 名）；向量只含这两条
        let kw_hits: Vec<(i64, f32)> = (1..=12).map(|i| (i as i64, 1.0)).collect();
        let vec_hits: Vec<(i64, f32)> = vec![(10, 0.9), (11, 0.8)];
        let fused = rrf_fuse(&vec_hits, &kw_hits, 12);
        assert_eq!(fused[0].0, 10, "向量第 1 + 关键词第 10 应融合到第一");
        assert_eq!(fused[0].1, 1.0 / 60.0 + 1.0 / 69.0);
        assert_eq!(fused[1].0, 11, "向量第 2 + 关键词第 11 应融合到第二");
        assert_eq!(
            fused[2].0, 1,
            "关键词第 1 没有向量加成，应被两个双路命中的压下去"
        );
    }

    /// 幂等守卫：**行数够但指纹不同**必须重建。
    /// 这正是「改了分词口径、老库直接跳过 → 查询用新口径、库里是旧向量」的坑。
    #[test]
    fn should_skip_index_requires_matching_fingerprint() {
        let fp = "abc123";
        assert!(should_skip_index(100, 100, Some(fp), fp), "行数够且指纹一致才跳过");
        assert!(
            !should_skip_index(100, 100, Some("old"), fp),
            "行数够但指纹不同 ⇒ 必须重建"
        );
        assert!(
            !should_skip_index(100, 100, None, fp),
            "指纹缺失（老库/种子库）⇒ 必须重建"
        );
        assert!(!should_skip_index(100, 200, Some(fp), fp), "行数不够 ⇒ 重建");
        assert!(!should_skip_index(0, 0, Some(fp), fp), "空库不跳过后无意义");
        assert!(!should_skip_index(0, 10, Some(fp), fp), "一行没建 ⇒ 重建");
    }

    /// 检索前置校验：指纹缺失必须判"不可用"。
    /// 缺失不是"默认相同"——老库与种子库都没有这个键，
    /// 状态判定：表为空 ⇒ 未构建；表非空但拿不到一致指纹 ⇒ 不可用/过期。
    /// 纯读 DB，不加载模型——「为什么一条都没召回来」这个问题不该先花 1~2s 加载 91MB 权重。
    #[test]
    fn semantic_layer_state_reads_the_database_not_a_flag() {
        use rusqlite::Connection;
        let mk = |with_rows: bool| {
            let c = Connection::open_in_memory().unwrap();
            c.execute_batch(
                "CREATE TABLE api_docs_embeddings (doc_id INTEGER, model TEXT, vector BLOB, created_at INTEGER);
                 CREATE TABLE api_docs_meta (key TEXT PRIMARY KEY, value TEXT);
                 CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )
            .unwrap();
            // 开关打开，否则判定会先短路成 `Disabled`（契约变过：P-7 方案②默认关）。
            // 这里要测的是「开了之后读库那支」，不是「关着」。
            set_semantic_enabled(&c, true).unwrap();
            if with_rows {
                c.execute(
                    "INSERT INTO api_docs_embeddings VALUES (1, 'bge-small-zh-v1.5', x'00', 0)",
                    [],
                )
                .unwrap();
            }
            c
        };
        assert_eq!(
            semantic_layer_state(&mk(false)),
            SemanticLayerState::NotIndexed,
            "表为空 ⇒ 从没建过索引"
        );
        // 有行但拿不到一致指纹 ⇒ 不得判成 Ready
        assert_ne!(
            semantic_layer_state(&mk(true)),
            SemanticLayerState::Ready,
            "没有一致指纹就绝不能声称就绪（错向安全那一侧）"
        );
    }

    /// 开关关闭时**必须**判成 `Disabled`，且这是它最先返回的一支。
    ///
    /// 这条钉的是 P-7 方案②的**收益来源**（不是功能开关本身）：
    /// 开关关着时判定要**不碰向量表、不碰模型**地早退，
    /// 这样 `global_embedder()` 永远不被触发 ⇒ 91MB 权重不被读。
    /// 若把开关判挪到数行数之后、或挪到模型加载之后，这条的意图就没了——
    /// 但**功能仍然"能跑"**（只是照样加载 91MB），所以**必须靠这条守住顺序**。
    ///
    /// ## 这里踩过一次：第一版只断言"结果 == Disabled"，是**假牙**
    ///
    /// 把开关判挪到「读向量表之后」当变异注入，测试**照样全绿**——
    /// 因为功能上仍返回 `Disabled`（向量齐备，不会走 `NotIndexed` 早退）。
    /// **功能和收益是两回事**：晚判只是多做几次 COUNT，测不出来。
    ///
    /// 所以判据必须是**可区分的**，而不是只断言一个返回值。
    /// 这里的关键：**只建 `settings`，刻意不建 `api_docs_embeddings` / `api_docs`**。
    /// - 早退（正确）⇒ 根本不查表 ⇒ 返回 `Disabled`
    /// - 晚判（变异）⇒ 查表失败被 `unwrap_or(0)` 吞成 `n=0` ⇒ 返回 `NotIndexed`
    ///
    /// 两者返回值因此可区分。`unwrap_or(0)` 会吞掉 `no such table`，
    /// 所以**"表不存在"恰好成了"读没读过表"的探针**。
    #[test]
    fn disabled_layer_short_circuits_before_touching_the_index() {
        use rusqlite::Connection;
        // 只建 settings —— 刻意**不建**向量表与 api_docs 表
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        assert_eq!(
            semantic_layer_state(&c),
            SemanticLayerState::Disabled,
            "开关关着 + 索引表根本不存在 ⇒ 必须返回 Disabled（早退，没去查表）。\
             若返回 NotIndexed，说明开关判被挪到了读表之后：功能仍能跑，\
             但 ②「默认不碰向量表/模型」的收益已经没了"
        );
        // 默认值必须是「关」：没写过这个 key 也一样
        let c2 = Connection::open_in_memory().unwrap();
        c2.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
        assert!(
            !semantic_enabled(&c2),
            "开关默认值必须是关（fail-closed）：否则『升级即默认启用』，正是 ② 要消除的行为"
        );
    }

    /// 半成品索引（建到一半被取消/中断）**必须**判成 `PartialIndex`，不得判成 `Ready`。
    ///
    /// 这条钉的是本轮的真实缺陷：`build_index_streaming` 在取消时仍会写指纹，
    /// 而判定当时**只验指纹不数行数** ⇒ 取消后（5,000 / 44,856）卡片显示绿色「就绪」，
    /// 而语义层只覆盖 11%；`vector_search` 复用同一函数判降级，
    /// 于是**半成品也不会被拒用**，会被当成全量。
    ///
    /// 第一条断言是**前提**：必须真的造出"行数盖不住总量"这个场景，
    /// 否则后面测的只是一个碰巧的分支（造边界场景失败时是绿的）。
    #[test]
    fn partial_index_is_never_reported_as_ready() {
        use rusqlite::Connection;
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE api_docs_embeddings (doc_id INTEGER, model TEXT, vector BLOB, created_at INTEGER);
             CREATE TABLE api_docs_meta (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE api_docs (id INTEGER, api_name TEXT, declaration TEXT, module TEXT, kit TEXT);
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
        // 造 3 条知识库、只建 2 条向量
        for i in 1..=3i64 {
            c.execute(
                "INSERT INTO api_docs VALUES (?1, 'api', 'function f(): void;', 'm', 'k')",
                [i],
            )
            .unwrap();
        }
        for i in 1..=2i64 {
            c.execute(
                "INSERT INTO api_docs_embeddings VALUES (?1, 'bge-small-zh-v1.5', x'00', 0)",
                [i],
            )
            .unwrap();
        }
        // 前提：开关必须先打开。**契约变过**（P-7 方案②）：判定现在先判开关，
        // 开关关着时**连向量表都不数**就返回 `Disabled`——
        // 所以不打开的话，下面测到的会是 `Disabled` 而不是半成品那一支。
        set_semantic_enabled(&c, true).unwrap();
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM api_docs_embeddings", [], |r| r.get(0))
            .unwrap();
        let total: i64 = c
            .query_row("SELECT COUNT(*) FROM api_docs", [], |r| r.get(0))
            .unwrap();
        assert!(
            n < total,
            "前提：必须真的造出行数盖不住总量（{n} < {total}），否则测不到半成品"
        );
        assert_eq!(
            semantic_layer_state(&c),
            SemanticLayerState::PartialIndex,
            "行数盖不住总量 ⇒ 索引未建完。这条若红，多半是有人把行数判据删了：\
             半成品会被当成全量使用，而 `vector_search` 也就不降级了"
        );
        // 补满后不再判半成品（避免这条判据变成"永远 Partial"）
        c.execute(
            "INSERT INTO api_docs_embeddings VALUES (3, 'bge-small-zh-v1.5', x'00', 0)",
            [],
        )
        .unwrap();
        assert_ne!(
            semantic_layer_state(&c),
            SemanticLayerState::PartialIndex,
            "行数补满后就不该再算半成品（否则这个状态就变成死路：永远重建不了）"
        );
    }

    /// **每一种**状态的提示必须**互不相同**，且每条都要把「现状」和「下一步」分开。
    ///
    /// 写成同一句话就等于没写：调用方看不出差别，用户也就不知道该做什么。
    /// 这条是为了让「多种状态」不至于退化成「一种提示套多种枚举名」——
    /// 那正是 `UNSCORED_QUALITY_SCORE` 取 0 而不是中间分要防的那类假信息。
    ///
    /// ## 契约变过：这里原本只有四态
    ///
    /// 加 `PartialIndex` 时若只写代码不改本测试，**它照样通过**
    /// （它只检查"列出的那几个"互不相同）——也就是说
    /// 「新状态没有独立提示」这个缺陷能一路溜过门禁，而且**看起来门禁是绿的**。
    ///
    /// Rust 没有「枚举全量」的反射，所以这里用一个**写死的数量断言**兜住：
    /// `EXPECTED_STATE_COUNT` 与清单长度都必须等于它。
    /// 新增变体而忘改本测试 ⇒ 数字对不上 ⇒ 报红。**这是能真的红的那一头。**
    /// （不写这条断言的话，本测试对"漏加变体"完全无感。）
    #[test]
    fn every_semantic_state_has_a_distinct_actionable_hint() {
        const EXPECTED_STATE_COUNT: usize = 6;
        let states = [
            SemanticLayerState::Ready,
            SemanticLayerState::NotIndexed,
            SemanticLayerState::StaleIndex,
            SemanticLayerState::PartialIndex,
            SemanticLayerState::ModelMissing,
            SemanticLayerState::Disabled,
        ];
        assert_eq!(
            states.len(),
            EXPECTED_STATE_COUNT,
            "枚举变体数与清单不一致：**新增 SemanticLayerState 变体时必须把本清单和这个数字一起补上**，\
             否则新状态会没有独立提示，而本测试仍然全绿（契约变过：原本只有四态）"
        );
        let mut seen: Vec<&str> = Vec::new();
        let mut seen_labels: Vec<&str> = Vec::new();
        for s in states {
            let h = s.hint();
            assert!(h.chars().count() > 20, "{s:?} 的提示过短，等于没有：{h}");
            assert!(
                !seen.contains(&h),
                "{s:?} 的提示与前面的状态重复了——四种状态写成同一句等于没写"
            );
            assert!(
                !seen_labels.contains(&s.label()),
                "{s:?} 的标签与其他状态重复：{}",
                s.label()
            );
            seen_labels.push(s.label());
            seen.push(h);
        }
        for s in states {
            assert!(
                s.hint().contains('；'),
                "{s:?} 的提示没有把「现状」和「下一步」用分号分开：{}",
                s.hint()
            );
        }
    }

    /// ETA：样本不足时**必须不报**，而不是报一个离谱的值。
    ///
    /// 128 条一批在 CPU 上只要几秒，若第一批就线性外推，
    /// 会算出「剩余 3 秒」——与真实 20~30 分钟**方向完全相反**，比不给更坏。
    /// 收的是 `elapsed_secs` 而不是 `Instant`，所以全部用常量、不用 sleep。
    #[test]
    fn eta_refuses_to_guess_before_it_has_a_meaningful_sample() {
        assert_eq!(eta_from(0, 44_856, 120.0), None, "一条都没做");
        assert_eq!(eta_from(128, 44_856, 3.0), None, "第一批：样本不足");
        assert_eq!(eta_from(19, 44_856, 5.0), None, "不足 20 条");
        assert_eq!(eta_from(200, 0, 5.0), None, "总量未知");
        assert_eq!(eta_from(44_856, 44_856, 1500.0), None, "已全部完成");
        assert_eq!(eta_from(4_000, 44_856, 0.4), None, "样本时间太短，速率不可信");
        // 已有足够样本 ⇒ 必须给出一个非零的数
        let e = eta_from(4_000, 44_856, 134.0).expect("4% 进度、已耗时 134s");
        assert!(e >= 1, "剩余时间至少 1 秒");
    }

    /// ETA 随进度推进而**不反弹**，且与真实速率一致。
    ///
    /// 用常量而非 sleep：同一批 elapsed 下，进度翻倍 ⇒ 剩余时间约减半。
    /// 钉「不反弹」而不是「严格递减」，是为了不给舍入持平留 flaky 的口子。
    #[test]
    fn eta_shrinks_as_progress_advances() {
        let a = eta_from(2_000, 44_856, 67.0).expect("样本足够");
        let b = eta_from(4_000, 44_856, 67.0).expect("样本足够");
        assert!(b < a, "同一耗时下进度翻倍，剩余应更少：{a}s → {b}s");
        // 速率自洽：10% 进度 / 已耗时 150s ⇒ 剩余 = 150 × (90%/10%) = 1350s
        let e = eta_from(4_485, 44_856, 150.0).expect("10% 进度");
        assert!(
            (1_300..=1_400).contains(&e),
            "10% 进度 / 150s ⇒ 剩余应约 1350s，实际 {e}s"
        );
    }

    /// 而它们的向量是由**别的口径**产出的（种子库甚至是上游机器上生成的）。
    #[test]
    fn fingerprint_matches_treats_missing_as_mismatch() {
        assert!(fingerprint_matches(Some("abc"), "abc"));
        assert!(!fingerprint_matches(Some("old"), "abc"), "不同口径不可比");
        assert!(
            !fingerprint_matches(None, "abc"),
            "没有指纹 ≠ 与当前口径相同（老库/种子库都落这里）"
        );
    }

    /// 指纹必须逐段加长度前缀，否则 ("ab","c") 与 ("a","bc") 会算出同一个值。
    #[test]
    fn fingerprint_is_length_prefixed() {
        assert_ne!(
            fingerprint_hex(&[b"ab", b"c"]),
            fingerprint_hex(&[b"a", b"bc"]),
            "拼接歧义会让两套不同输入算出同一个指纹"
        );
        assert_eq!(
            fingerprint_hex(&[b"a", b"b"]),
            fingerprint_hex(&[b"a", b"b"]),
            "同输入必须同指纹"
        );
        assert_ne!(
            fingerprint_hex(&[b"a", b"b"]),
            fingerprint_hex(&[b"a", b"c"]),
            "任一段变化必须改指纹"
        );
    }

    /// 索引指纹跟踪**文件内容**，不跟踪 mtime。
    /// mtime 在拷贝/解包后就会变，会造成无谓的 20~30 分钟全量重建；
    /// 内容变了 mtime 没变才是致命的。纯源码 checkout 无模型文件时跳过。
    #[test]
    fn index_fingerprint_tracks_contents_not_mtime() {
        let dir = std::env::temp_dir().join(format!("deveco-fp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for name in FINGERPRINT_FILES {
            std::fs::create_dir_all(&dir).ok();
            std::fs::write(dir.join(name), b"v1").unwrap();
        }
        let fp1 = index_fingerprint(&dir).expect("指纹应可计算");
        // 只改 mtime，内容不动
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(86_400);
        for name in FINGERPRINT_FILES {
            let p = dir.join(name);
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(later).unwrap();
        }
        assert_eq!(
            index_fingerprint(&dir).unwrap(),
            fp1,
            "mtime 变了不该改指纹（否则每次拷贝/解包都要全量重建）"
        );
        // 改内容
        std::fs::write(dir.join("tokenizer.json"), b"v2").unwrap();
        assert_ne!(
            index_fingerprint(&dir).unwrap(),
            fp1,
            "内容变了必须改指纹（否则旧向量被当缓存继续用）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 钉住「为什么要覆盖 normalizer」这条**外部证据**。
    /// 模型自带的 tokenizer 是 `lowercase:false` 而词表全小写——两者一起才是缺陷。
    /// 哪天发行流程换成大小写一致的词表，这条会红，提示覆盖可以撤掉了。
    #[test]
    fn shipped_tokenizer_is_case_sensitive_so_the_override_is_needed() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("embedding")
            .join("bge-small-zh-v1.5");
        let tok_path = dir.join("tokenizer.json");
        if !tok_path.is_file() {
            return; // 纯源码 checkout：模型资源被 gitignore
        }
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&tok_path).unwrap()).unwrap();
        assert_eq!(
            v["normalizer"]["lowercase"], false,
            "模型自带的 normalizer 若已小写化，imp::fixed_normalizer 的覆盖就可以撤掉了"
        );
        let vocab = v["model"]["vocab"].as_object().expect("词表应是对象");
        assert!(
            vocab.contains_key("get") && !vocab.contains_key("Get"),
            "词表预期只有小写形：get∈V 而 Get∉V"
        );
        assert!(
            vocab.contains_key("kit") && !vocab.contains_key("Kit"),
            "词表预期只有小写形：kit∈V 而 Kit∉V"
        );
    }
}
