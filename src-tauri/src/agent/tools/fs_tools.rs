//! 文件系统域工具：目录浏览 / 文件读取 / 搜索 / 写入 / 编辑 / 删除 / 移动 / 复制 / 撤销 / 批量编辑。
//! 共享辅助函数（truncate_out / stamps / run_cmd 等）在父模块 mod.rs，通过 `use super::*` 继承。

use super::*;

/// 面向工具协议的稳定内容版本。与进程内冲突检测使用的 FNV 指纹分离：
/// 该值会暴露给 Agent，后续可直接作为 expected_hash 乐观锁使用。
fn file_content_version(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 读文件请求（宽松）：字段均可选；路径归一化/基础文件校验集中在 `resolve()` 显式落地。
#[derive(serde::Deserialize, Default)]
pub(super) struct ReadFileRequest {
    /// 文件路径（相对工程根或绝对路径，resolve 校验非空）
    pub path: Option<String>,
    /// 骨架模式：只输出结构定义行（import/类/函数等），快速了解大文件
    pub outline: Option<bool>,
    /// 骨架分页（1 起，缺省第 1 页；每页 200 条，大文件结构项多时翻页查看）
    pub outline_page: Option<u32>,
    /// 骨架类型过滤（如 "函数"/"类型"/"组件"）：只显示标签含该词的条目，分页在过滤后集合上进行
    pub outline_filter: Option<String>,
    /// 起始行号（1 起，缺省 1）
    pub start: Option<u64>,
    /// 读取行数（缺省读到文件尾，单次最多 2000 行）
    pub lines: Option<u64>,
}

impl ReadFileRequest {
    /// 从工具入参解析宽松请求。
    pub(super) fn from_args(args: &Value) -> Result<Self, String> {
        serde_json::from_value(args.clone()).map_err(|e| format!("read_file 参数解析失败：{e}"))
    }

    /// 显式 resolve：路径非空/归一化/文件类型/大小上限校验，产出严格规范。
    pub(super) fn resolve(self, roots: &[String]) -> Result<ReadFileSpec, String> {
        let raw = self.path.as_deref().unwrap_or("");
        if raw.is_empty() {
            return Err("read_file 需要参数 {\"path\":\"<文件路径>\"}".into());
        }
        let p = resolve_readable(roots, raw)?;
        if !p.is_file() {
            return Err(format!("路径不是文件: {}", p.display()));
        }
        let meta = std::fs::metadata(&p).map_err(|e| e.to_string())?;
        let stream_large = meta.len() > 1024 * 1024;
        if stream_large
            && (self.outline.unwrap_or(false)
                || self.lines.unwrap_or(0) == 0
                || self.lines.unwrap_or(0) > 2_000)
        {
            return Err(format!(
                "文件较大（{}，>1MB），请显式传 start/lines 进行流式窗口读取（单次最多 2000 行，不会把整文件加载进内存）；大文件 outline 将在行偏移索引上线后支持",
                human_size(meta.len())
            ));
        }
        Ok(ReadFileSpec {
            path: p,
            outline: self.outline.unwrap_or(false),
            outline_page: self.outline_page.unwrap_or(1).max(1),
            outline_filter: self.outline_filter.filter(|f| !f.trim().is_empty()),
            start: self.start.unwrap_or(1).max(1),
            lines: self.lines.unwrap_or(0),
            stream_large,
        })
    }
}

/// 读文件规范（严格）：由 `ReadFileRequest::resolve()` 产出。
pub(super) struct ReadFileSpec {
    /// 已归一化且已验证的文件路径（≤1MB 的文本文件）
    pub path: PathBuf,
    /// 是否骨架模式
    pub outline: bool,
    /// 骨架分页（≥1，缺省 1）
    pub outline_page: u32,
    /// 骨架类型过滤（None = 不过滤）
    pub outline_filter: Option<String>,
    /// 起始行号（≥1）
    pub start: u64,
    /// 读取行数（0 表示读到文件尾）
    pub lines: u64,
    /// 超过 1 MiB 时走固定内存的流式行窗口，不加载整文件。
    pub stream_large: bool,
}

/// 写文件请求（宽松）：默认值/校验在 `resolve()` 显式落地。
#[derive(serde::Deserialize, Default)]
pub(super) struct WriteFileRequest {
    /// 目标路径（相对工程根或绝对路径）
    pub path: Option<String>,
    /// 要写入的内容（resolve 校验非空且 ≤1MB）
    pub content: Option<String>,
}

impl WriteFileRequest {
    pub(super) fn from_args(args: &Value) -> Result<Self, String> {
        serde_json::from_value(args.clone()).map_err(|e| format!("write_file 参数解析失败：{e}"))
    }

    /// 显式 resolve：路径/内容非空校验、大小上限、敏感文件保护、写入目标归一化。
    pub(super) fn resolve(self, roots: &[String]) -> Result<WriteFileSpec, String> {
        let raw = self.path.as_deref().unwrap_or("");
        if raw.is_empty() {
            return Err("write_file 需要参数 {\"path\":\"<文件路径>\",\"content\":\"<内容>\"}".into());
        }
        let content = self.content.ok_or("write_file 缺少 content 参数（要写入的内容）")?;
        if content.len() > 1024 * 1024 {
            return Err("内容超过 1MB，write_file 单次仅支持小文件，请拆分写入".into());
        }
        let p = resolve_for_write(roots, raw)?;
        if let Some(reason) = is_protected_file(&p) {
            return Err(format!("写入被安全策略拒绝：{reason}"));
        }
        if p.is_dir() {
            return Err(format!("目标是目录，无法写入: {}", p.display()));
        }
        Ok(WriteFileSpec { path: p, content })
    }
}

/// 写文件规范（严格）：由 `WriteFileRequest::resolve()` 产出。
pub(super) struct WriteFileSpec {
    /// 已归一化且通过安全校验的目标路径
    pub path: PathBuf,
    /// 已校验的内容（≤1MB）
    pub content: String,
}

/// 编辑文件请求（宽松）：默认值/校验在 `resolve()` 显式落地。
#[derive(serde::Deserialize, Default)]
pub(super) struct EditFileRequest {
    /// 目标路径
    pub path: Option<String>,
    /// 被替换的原文（resolve 校验非空；与 start 互斥）
    pub old: Option<String>,
    /// 替换成的新文（缺省空串）
    pub new: Option<String>,
    /// 是否全部替换（缺省 false，仅替换第一处）
    pub replace_all: Option<bool>,
    /// 代码块模式：按该行号定位所在完整方法/函数/代码块并整体替换（new 为空 = 整块删除）。
    /// 语言感知成对匹配：不固定行数，块有多长就替换多长，杜绝漏掉结束符。
    pub start: Option<u64>,
    /// 块锚签名（可选，start 模式）：期望的块定义行内容片段（如 "fn parse"）。
    /// 行号因先前编辑漂移时不致改错块：定位到的块首行不含 anchor 时，
    /// 在 ±100 行内搜索含 anchor 的块根行重新定位；找不到则拒绝。
    pub anchor: Option<String>,
    /// 批量块模式：多个行号一次定位多个完整块整体替换（与 old/start 互斥）。
    /// 与 news 一一对应；全部块先在原文上定位（行号互不漂移）再统一拼接。
    pub starts: Option<Vec<u64>>,
    /// 批量模式的各块新内容（空串 = 整块删除），与 starts 等长
    pub news: Option<Vec<String>>,
    /// 批量模式各块的锚签名（可选，与 starts 等长；元素可为 null）
    pub anchors: Option<Vec<Option<String>>>,
}

impl EditFileRequest {
    pub(super) fn from_args(args: &Value) -> Result<Self, String> {
        serde_json::from_value(args.clone()).map_err(|e| format!("edit_file 参数解析失败：{e}"))
    }

    /// 显式 resolve：路径归一化/文件校验/敏感保护 + old 非空校验（start 模式按代码块替换）。
    pub(super) fn resolve(self, roots: &[String]) -> Result<EditFileSpec, String> {
        let raw = self.path.as_deref().unwrap_or("");
        if raw.is_empty() {
            return Err("edit_file 需要参数 {\"path\":\"<文件路径>\",\"old\":\"<原文>\",\"new\":\"<新文>\"}".into());
        }
        let batch = self.starts.is_some();
        if batch && (self.old.is_some() || self.start.is_some()) {
            return Err("edit_file 的 starts（批量块替换）与 old/start 参数互斥：批量模式只认 starts/news/anchors".into());
        }
        if self.old.is_some() && self.start.is_some() {
            return Err("edit_file 的 old 与 start 参数互斥：old=精确文本替换；start=按代码块整体替换（start 定位所在完整方法/块，不固定行数）".into());
        }
        // 批量模式参数校验：starts 非空、news 等长、anchors 可选但须等长
        let starts = self.starts.clone();
        let news = self.news.clone();
        let anchors = self.anchors.clone();
        if batch {
            let st = starts.as_deref().unwrap_or(&[]);
            if st.is_empty() {
                return Err("edit_file 批量模式 starts 数组不能为空".into());
            }
            let ns = news.as_deref().unwrap_or(&[]);
            if ns.len() != st.len() {
                return Err(format!(
                    "edit_file 批量模式 starts（{} 项）与 news（{} 项）长度必须一致",
                    st.len(),
                    ns.len()
                ));
            }
            if let Some(an) = anchors.as_deref() {
                if an.len() != st.len() {
                    return Err(format!(
                        "edit_file 批量模式 anchors（{} 项）与 starts（{} 项）长度必须一致（无需锚的项传 null）",
                        an.len(),
                        st.len()
                    ));
                }
            }
        }
        let old = self.old.unwrap_or_default();
        if old.is_empty() && self.start.is_none() && !batch {
            return Err("edit_file 需要 old 参数（原文片段），或用 start 参数按代码块整体替换，或用 starts/news 批量替换多个块".into());
        }
        let p = resolve_in_roots(roots, raw)?;
        if let Some(reason) = is_protected_file(&p) {
            return Err(format!("编辑被安全策略拒绝：{reason}"));
        }
        if !p.is_file() {
            return Err(format!("路径不是文件: {}", p.display()));
        }
        let meta = std::fs::metadata(&p).map_err(|e| e.to_string())?;
        if meta.len() > 1024 * 1024 {
            return Err("文件超过 1MB，请用 run_command 执行脚本处理或拆分修改".into());
        }
        Ok(EditFileSpec {
            path: p,
            old,
            new: self.new.unwrap_or_default(),
            replace_all: self.replace_all.unwrap_or(false),
            start: self.start,
            anchor: self.anchor.filter(|a| !a.trim().is_empty()),
            starts: if batch { starts } else { None },
            news: if batch { news.unwrap_or_default() } else { Vec::new() },
            anchors: if batch {
                anchors.unwrap_or_default()
                    .into_iter()
                    .map(|a| a.filter(|s| !s.trim().is_empty()))
                    .collect()
            } else {
                Vec::new()
            },
        })
    }
}

/// 编辑文件规范（严格）：由 `EditFileRequest::resolve()` 产出。
pub(super) struct EditFileSpec {
    /// 已归一化且通过校验的目标文件
    pub path: PathBuf,
    /// 被替换的原文（非空）
    pub old: String,
    /// 替换成的新文
    pub new: String,
    /// 是否全部替换
    pub replace_all: bool,
    /// 代码块模式定位行（Some = 按完整代码块替换，忽略 old/replace_all）
    pub start: Option<u64>,
    /// 块锚签名（start 模式防行号漂移；见 EditFileRequest.anchor）
    pub anchor: Option<String>,
    /// 批量块模式行号组（Some = 批量替换，忽略 old/start）
    pub starts: Option<Vec<u64>>,
    /// 批量模式各块新内容（与 starts 等长；空串 = 整块删除）
    pub news: Vec<String>,
    /// 批量模式各块锚签名（与 starts 等长；None = 该块不用锚）
    pub anchors: Vec<Option<String>>,
}

pub(super) async fn delete_file(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录，无法删除".into());
    }
    let rel = args["path"].as_str().unwrap_or("").trim();
    if rel.is_empty() {
        return Err("缺少 path 参数".into());
    }
    let root = Path::new(project_path);
    let target = root.join(rel);
    // 防越权：必须在项目根内
    let canon_target = target.canonicalize().map_err(|e| format!("路径无效: {e}"))?;
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if !canon_target.starts_with(&canon_root) {
        return Err("禁止删除项目目录之外的文件".into());
    }
    // 禁止删除受保护目录（与工具描述一致：版本库/依赖/产物/IDE 配置目录一律禁止）
    let name = canon_target.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let protected = [
        ".git", "oh_modules", "node_modules", ".ohpm", ".deveco-agent",
        "build", ".hvigor", ".idea", ".arkui-x",
    ];
    if protected.contains(&name) && canon_target.is_dir() {
        return Err(format!("受保护目录 {name} 不允许通过工具删除"));
    }
    if canon_target == canon_root {
        return Err("不能删除项目根目录".into());
    }
    if is_protected_file(&canon_target).is_some() {
        return Err("敏感文件（密钥/证书/迁移）不允许删除".into());
    }
    if !target.exists() {
        return Err(format!("文件不存在: {rel}"));
    }
    // [58] dry-run：预览将删除的路径，不落盘、不移动
    if args["dry_run"].as_bool().unwrap_or(false) {
        return Ok(format!(
            "【dry_run 预览】将删除（移入回收站可恢复）：{}（{}，{}）",
            target.display(),
            if target.is_dir() { "目录" } else { "文件" },
            if let Ok(m) = std::fs::metadata(&target) { super::ui_tools::format_bytes(m.len()) } else { "未知大小".to_string() }
        ));
    }
    // 移到回收站（保留相对路径结构以便恢复）
    let trash_root = root.join(".deveco-agent").join("trash");
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dest = trash_root
        .join(ts.to_string())
        .join(rel.trim_start_matches(['/', '\\']));
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // 跨设备移动兜底：先 rename，失败则 copy + remove
    match std::fs::rename(&target, &dest) {
        Ok(_) => Ok(format!("已移至回收站: {}（可恢复）", dest.display())),
        Err(_) => {
            if target.is_dir() {
                copy_dir_recursive(&target, &dest)?;
                std::fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
            } else {
                std::fs::copy(&target, &dest).map_err(|e| e.to_string())?;
                std::fs::remove_file(&target).map_err(|e| e.to_string())?;
            }
            Ok(format!("已移至回收站: {}（可恢复）", dest.display()))
        }
    }
}

pub(super) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for e in std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
        let p = e.path();
        let t = dst.join(e.file_name());
        if p.is_dir() {
            copy_dir_recursive(&p, &t)?;
        } else {
            std::fs::copy(&p, &t).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// move_file：移动/重命名项目内文件或目录（不覆盖目标，禁止受保护路径）
pub(super) async fn move_file(args: &Value, roots: &[String]) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法移动文件".into());
    }
    let from_raw = args["from"].as_str().ok_or("move_file 需要参数 {\"from\":\"<源路径>\",\"to\":\"<目标路径>\"}")?.trim();
    let to_raw = args["to"].as_str().ok_or("move_file 缺少 to 参数（目标路径）")?.trim();
    if from_raw.is_empty() || to_raw.is_empty() {
        return Err("from/to 参数不能为空".into());
    }
    let src = resolve_in_roots(roots, from_raw)?;
    // 目标允许不存在（移动=重命名到新路径），用 resolve_for_write 解析：
    // 逐级校验最近存在祖先在根内 + 拼接剩余段，防 .. 越界与指向项目外；
    // 目标已存在时 resolve_for_write 内部先走 resolve_in_roots 返回原路径，由下方拒绝覆盖。
    let dst = resolve_for_write(roots, to_raw)?;
    if let Some(reason) = is_protected_file(&src) {
        return Err(format!("移动被安全策略拒绝：{reason}"));
    }
    if !src.exists() {
        return Err(format!("源路径不存在: {from_raw}"));
    }
    // 项目根本身不可移动（与 delete_file 同规则）
    let canon_src = src.canonicalize().map_err(|e| format!("路径无效: {e}"))?;
    for r in roots {
        let rc = std::fs::canonicalize(r).unwrap_or_else(|_| PathBuf::from(r));
        if canon_src == rc {
            return Err("不能移动项目根目录".into());
        }
    }
    // 受保护目录（版本库/依赖/产物/IDE 配置）不可移动，目标也不可落入其中
    const PROTECTED: [&str; 9] = [
        ".git", "oh_modules", "node_modules", ".ohpm", ".deveco-agent",
        "build", ".hvigor", ".idea", ".arkui-x",
    ];
    let sname = src.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if PROTECTED.contains(&sname) && src.is_dir() {
        return Err(format!("受保护目录 {sname} 不允许移动"));
    }
    for comp in dst.ancestors().skip(1) {
        if let Some(n) = comp.file_name().and_then(|s| s.to_str()) {
            if PROTECTED.contains(&n) && comp.is_dir() {
                return Err(format!("目标落入受保护目录 {n}，拒绝移动"));
            }
        }
    }
    if dst.exists() {
        return Err(format!(
            "目标已存在，拒绝覆盖: {}\n源路径: {}\n请先 list_dir 核对实际目录结构：若源路径拼错（如 entry/entry/src 出现嵌套重复），按真实结构重发；确认确实需要替换/合并时，先删除或清空旧目标再移动。",
            dst.display(),
            src.display()
        ));
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目标父目录失败 {}: {e}", parent.display()))?;
    }
    // [58] dry-run：预览移动目标（校验全部通过后，仅报告不执行）
    if args["dry_run"].as_bool().unwrap_or(false) {
        return Ok(format!(
            "【dry_run 预览】将移动 {}\n             → {}\n（校验已通过：非受保护路径、目标不存在）",
            src.display(),
            dst.display()
        ));
    }
    // 跨设备移动兜底：先 rename，失败则 copy + remove（与 delete_file 同模式）
    match std::fs::rename(&src, &dst) {
        Ok(_) => Ok(format!("已移动 {} → {}", src.display(), dst.display())),
        Err(_) => {
            if src.is_dir() {
                copy_dir_recursive(&src, &dst)?;
                std::fs::remove_dir_all(&src).map_err(|e| e.to_string())?;
            } else {
                std::fs::copy(&src, &dst).map_err(|e| e.to_string())?;
                std::fs::remove_file(&src).map_err(|e| e.to_string())?;
            }
            Ok(format!("已移动 {} → {}（跨设备复制方案）", src.display(), dst.display()))
        }
    }
}

/// undo_edit：按栈序（LIFO）恢复最近一次文件修改前的快照；
/// preview=true 时只展示将恢复的 diff 不落盘（撤销预览）。
pub(super) async fn undo_edit(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    let count = args["count"].as_u64().unwrap_or(1).clamp(1, 10) as usize;
    let preview = args["preview"].as_bool().unwrap_or(false);
    if preview {
        return undo_preview(conversation_id, roots, count);
    }
    let mut restored: Vec<String> = Vec::new();
    for _ in 0..count {
        let Some(s) = crate::agent::undo::pop_undo(conversation_id) else {
            break;
        };
        // 恢复前校验路径仍在会话可见根内（跨项目快照不可恢复）。
        // 注意：canonicalize 在 Windows 返回 \\?\ 前缀路径，而快照路径已 normalize
        // 剥掉前缀——直接 starts_with 会永远失配（undo 静默失效），须同口径归一化。
        let allowed = roots.iter().any(|r| {
            let rc = std::fs::canonicalize(r).unwrap_or_else(|_| PathBuf::from(r));
            let rc = PathBuf::from(crate::utils::path::normalize_path(&rc.to_string_lossy()));
            crate::utils::path::path_within(&s.path, &rc)
        });
        if !allowed {
            continue;
        }
        if let Some(parent) = s.path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        // 恢复写盘为 IO 操作，放 spawn_blocking 避免钉死 tokio worker
        let path_buf = s.path.clone();
        let content_buf = s.content.clone();
        let restored_path = tokio::task::spawn_blocking(move || {
            std::fs::write(&path_buf, &content_buf)
                .map_err(|e| format!("恢复 {} 失败: {e}", path_buf.display()))?;
            let meta = std::fs::metadata(&path_buf).ok();
            Ok::<(String, Option<std::fs::Metadata>), String>((path_buf.display().to_string(), meta))
        })
        .await
        .map_err(|e| format!("撤销恢复任务异常: {e}"))??;
        if let Some(meta) = &restored_path.1 {
            stamp_put(&s.path, meta, &s.content);
        }
        restored.push(restored_path.0);
    }
    if restored.is_empty() {
        return Ok("没有可撤销的修改（本会话尚无 Agent 文件写入记录）".into());
    }
    let remain = crate::agent::undo::undo_count(conversation_id);
    let mut out = format!(
        "已撤销 {} 处修改（剩余可撤销 {remain} 步）：\n",
        restored.len()
    );
    for p in &restored {
        out.push_str(&format!("- {p}\n"));
    }
    Ok(out)
}

/// 撤销预览：不弹出快照，逐条展示将恢复的改动（行级 diff），供模型/用户确认后再真正撤销。
fn undo_preview(conversation_id: &str, roots: &[String], count: usize) -> Result<String, String> {
    let mut parts: Vec<String> = Vec::new();
    for i in 0..count {
        let Some(s) = crate::agent::undo::peek_at(conversation_id, i) else { break };
        let allowed = roots.iter().any(|r| {
            let rc = std::fs::canonicalize(r).unwrap_or_else(|_| PathBuf::from(r));
            s.path.starts_with(&rc)
        });
        if !allowed {
            continue;
        }
        parts.push(format_undo_diff(&s));
    }
    if parts.is_empty() {
        return Ok("没有可撤销的修改（本会话尚无 Agent 文件写入记录）".into());
    }
    let remain = crate::agent::undo::undo_count(conversation_id);
    let mut out = format!(
        "撤销预览（将恢复 {} 处，剩余可撤销 {remain} 步；确认后去掉 preview 参数重新调用即可真正恢复）：\n",
        parts.len()
    );
    for p in parts {
        out.push_str(&p);
    }
    Ok(out)
}

/// 生成单文件撤销 diff：修改时间 + 行数对比 + 中间变化段（- 旧行 / + 新行 / 空格上下文行）。
fn format_undo_diff(s: &crate::agent::undo::Snapshot) -> String {
    let cur = std::fs::read(&s.path).unwrap_or_default();
    let old_lines: Vec<String> = String::from_utf8_lossy(&s.content).lines().map(String::from).collect();
    let cur_lines: Vec<String> = String::from_utf8_lossy(&cur).lines().map(String::from).collect();
    let at = chrono::DateTime::from_timestamp(s.at, 0)
        .map(|d| d.format("%H:%M:%S").to_string())
        .unwrap_or_default();
    // 裁剪共同前缀/后缀，只对比中间变化段（大文件/小幅改动时输出最小化）
    let mut i = 0usize;
    while i < old_lines.len() && i < cur_lines.len() && old_lines[i] == cur_lines[i] {
        i += 1;
    }
    let mut j_old = old_lines.len();
    let mut j_cur = cur_lines.len();
    while j_old > i && j_cur > i && old_lines[j_old - 1] == cur_lines[j_cur - 1] {
        j_old -= 1;
        j_cur -= 1;
    }
    let mut out = format!(
        "\n- {}（修改于 {at}，旧 {} 行 → 当前 {} 行）\n",
        s.path.display(),
        old_lines.len(),
        cur_lines.len()
    );
    const MAX_SHOWN: usize = 40;
    let mut o = i;
    let mut c = i;
    let mut shown = 0;
    while o < j_old || c < j_cur {
        if shown >= MAX_SHOWN {
            out.push_str(&format!(
                "  …（其余 {} 行变化省略）\n",
                (j_old - o) + (j_cur - c)
            ));
            break;
        }
        if o < j_old && (c >= j_cur || old_lines[o] != cur_lines[c]) {
            out.push_str(&format!("- {}\n", trunc_line(&old_lines[o])));
            o += 1;
        } else if c < j_cur && (o >= j_old || old_lines[o] != cur_lines[c]) {
            out.push_str(&format!("+ {}\n", trunc_line(&cur_lines[c])));
            c += 1;
        } else {
            out.push_str(&format!("  {}\n", trunc_line(&cur_lines[c])));
            o += 1;
            c += 1;
        }
        shown += 1;
    }
    out
}

/// 单行截断（预览护栏：每行最多 120 字符）
fn trunc_line(s: &str) -> String {
    let t: String = s.chars().take(120).collect();
    if t.chars().count() < s.chars().count() {
        format!("{t}…")
    } else {
        t
    }
}

pub(super) fn should_skip_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

/// 人类可读文件大小
pub(super) fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n}B")
    } else {
        format!("{v:.1}{}", UNITS[u])
    }
}

pub(super) fn fmt_time(ts: std::time::SystemTime) -> String {
    let d: chrono::DateTime<chrono::Local> = ts.into();
    d.format("%Y-%m-%d %H:%M").to_string()
}

/// 简单 glob 匹配（支持 * 单层、** 任意层级、? 单字符；不区分大小写；* 不跨 /）
pub(super) fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    fn m(p: &[char], t: &[char]) -> bool {
        if p.is_empty() {
            return t.is_empty();
        }
        match p[0] {
            '*' => {
                if p.len() > 1 && p[1] == '*' {
                    if p.len() > 2 && p[2] == '/' {
                        // **/：匹配零个或多个路径段
                        if m(&p[3..], t) {
                            return true;
                        }
                        for i in 0..t.len() {
                            if t[i] == '/' && m(&p[3..], &t[i + 1..]) {
                                return true;
                            }
                        }
                        return false;
                    }
                    // **：匹配任意内容（含 /）
                    for i in 0..=t.len() {
                        if m(&p[2..], &t[i..]) {
                            return true;
                        }
                    }
                    return false;
                }
                // *：不跨 /
                for i in 0..=t.len() {
                    if t.get(i) == Some(&'/') {
                        break;
                    }
                    if m(&p[1..], &t[i..]) {
                        return true;
                    }
                }
                false
            }
            '?' => !t.is_empty() && t[0] != '/' && m(&p[1..], &t[1..]),
            c => !t.is_empty() && t[0].eq_ignore_ascii_case(&c) && m(&p[1..], &t[1..]),
        }
    }
    m(&p, &t)
}

/// 单目录最多展示条目数：超出折叠为一行汇总，避免静态资源/生成物把输出刷爆后整体截断（截断对模型无用）
const DIR_SHOW_MAX: usize = 30;
/// . 开头目录白名单（默认隐藏目录全部跳过，仅保留对 AI 有用的）
const HIDDEN_ALLOW: [&str; 1] = [".github"];
/// 单壳目录穿透预算：只有单个子目录且无文件的目录链（如 entry/src/main/ets）自动展开，不消耗 depth
const SHELL_PENETRATE_MAX: usize = 3;

/// .gitignore 规则（简化子集：! 取反、结尾 / 仅目录、/ 开头锚定根、含 / 按相对路径匹配，否则按条目名匹配任意层级）
#[derive(Clone)]
struct IgnoreRule {
    pattern: String,
    negate: bool,
    dir_only: bool,
    anchored: bool,
    /// 规则基准：相对项目根的目录（如 "entry"，规则仅作用于该目录及以下）；空 = 项目根
    base: String,
}

/// 解析 .gitignore：仅支持本项目需要的子集（完整 git 语义过于复杂，够用即可）
fn parse_gitignore(content: &str) -> Vec<IgnoreRule> {
    parse_gitignore_at(content, "")
}

/// 解析某子目录下的 .gitignore：规则以 base（相对项目根）为基准
fn parse_gitignore_at(content: &str, base: &str) -> Vec<IgnoreRule> {
    let mut rules = Vec::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut pat = line;
        let mut negate = false;
        if let Some(rest) = pat.strip_prefix('!') {
            negate = true;
            pat = rest;
        }
        if pat.is_empty() {
            continue;
        }
        let dir_only = pat.ends_with('/');
        if dir_only {
            pat = pat.trim_end_matches('/');
        }
        // / 开头：锚定到规则所在目录（按相对路径匹配，防止 "/build" 误伤子层同名目录）
        let anchored = pat.starts_with('/');
        if anchored {
            pat = pat.trim_start_matches('/');
        }
        if pat.is_empty() {
            continue;
        }
        rules.push(IgnoreRule {
            pattern: pat.to_string(),
            negate,
            dir_only,
            anchored,
            base: base.to_string(),
        });
    }
    rules
}

/// 相对路径与规则基准的关系：base 为空 → 全匹配；否则 rel 必须是 base 自身或 base 的子路径
/// （子目录 .gitignore 只作用于其目录及以下；返回剥离基准后的相对路径用于匹配）
fn rel_vs_base<'a>(rel: &'a str, base: &str) -> Option<&'a str> {
    if base.is_empty() {
        return Some(rel);
    }
    if rel == base {
        return Some("");
    }
    rel.strip_prefix(base).and_then(|rest| rest.strip_prefix('/'))
}

/// 判断条目是否被 gitignore 规则忽略（后置规则覆盖前置；目录与文件共用条目名匹配）
fn gitignore_ignored(rules: &[IgnoreRule], name: &str, rel: &str, is_dir: bool) -> bool {
    let mut ignored = false;
    for r in rules {
        if r.dir_only && !is_dir {
            continue;
        }
        // 子目录规则先验基准：不在 base 子树内的条目直接跳过
        let Some(eff_rel) = rel_vs_base(rel, &r.base) else { continue };
        // 锚定或含 / 的规则按相对路径匹配，否则按条目名匹配（任意层级）
        let hit = if r.anchored || r.pattern.contains('/') {
            glob_match(&r.pattern, eff_rel)
        } else {
            glob_match(&r.pattern, name)
        };
        if hit {
            ignored = !r.negate;
        }
    }
    ignored
}

/// 查找包含 root 的项目根（roots 中首个包含者），加载其 .gitignore 规则；
/// 返回 (规则, root 相对项目根的路径，作为相对路径匹配基准)
fn load_project_ignore(root: &Path, roots: &[String]) -> (Vec<IgnoreRule>, String) {
    let mut proj_root: Option<PathBuf> = None;
    for r in roots {
        let Ok(rc) = std::fs::canonicalize(r) else { continue };
        // canonicalize 在 Windows 带 \?\ 前缀，与 resolve_in_roots 返回的规范化路径不一致，
        // 不归一化会导致 path_within 字符串比较失败 → gitignore 规则静默失效
        let rc = PathBuf::from(crate::utils::path::normalize_path(&rc.to_string_lossy()));
        if crate::utils::path::path_within(root, &rc) {
            proj_root = Some(rc);
            break;
        }
    }
    let mut rules = Vec::new();
    if let Some(pr) = &proj_root {
        if let Ok(content) = std::fs::read_to_string(pr.join(".gitignore")) {
            rules = parse_gitignore(&content);
        }
    }
    let start_rel = proj_root
        .as_ref()
        .and_then(|pr| root.strip_prefix(pr).ok())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    (rules, start_rel)
}

/// 子目录 .gitignore 合并：本目录的规则以 rel 为基准，只对其子树生效，与父规则合并后向下传递
fn load_child_rules(rules: &[IgnoreRule], dir: &Path, rel: &str) -> Vec<IgnoreRule> {
    if rel.is_empty() {
        return rules.to_vec();
    }
    match std::fs::read_to_string(dir.join(".gitignore")) {
        Ok(content) => {
            let mut v = rules.to_vec();
            v.extend(parse_gitignore_at(&content, rel));
            v
        }
        Err(_) => rules.to_vec(),
    }
}

/// 注释折叠阈值：连续注释行达到该值才折叠为一行摘要（短注释块信息密度高，保留原文）
const COMMENT_FOLD_MIN: usize = 8;

/// 代码语言扩展名（注释识别/折叠只对代码生效；文本/数据文件如 .md 的 # 是标题、.json 无注释，误伤代价高）
fn is_code_ext(ext: &str) -> bool {
    matches!(
        ext,
        "ets" | "ts" | "tsx" | "js" | "jsx" | "rs" | "py" | "java" | "kt" | "swift"
            | "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "go" | "sql" | "lua"
            | "sh" | "bash" | "bat" | "cmd" | "ps1" | "css" | "scss" | "less"
            | "vue" | "svelte" | "php" | "rb" | "cs" | "dart" | "m" | "mm"
    )
}

/// 判断某行是否为注释行（C 系 // /* */ *，Python/Shell #，SQL/Lua --）
fn is_comment_line(line: &str, ext: &str) -> bool {
    if !is_code_ext(ext) {
        return false;
    }
    let t = line.trim_start();
    if t.is_empty() {
        return false;
    }
    if t.starts_with("//") || t.starts_with("/*") || t.starts_with('*') || t.starts_with("*/") {
        return true;
    }
    if (ext == "py" || ext == "sh" || ext == "bash" || ext == "ps1" || ext == "rb")
        && t.starts_with('#')
    {
        return true;
    }
    if (ext == "sql" || ext == "lua") && t.starts_with("--") {
        return true;
    }
    false
}

/// 低价值文件聚合：扩展名 → 类别（这类文件逐条列出对理解结构无益，按类别汇总一行即可）
fn agg_category(name: &str) -> Option<&'static str> {
    const EXT: &[(&str, &str)] = &[
        ("png", "图片"), ("jpg", "图片"), ("jpeg", "图片"), ("gif", "图片"),
        ("webp", "图片"), ("svg", "图片"), ("ico", "图片"), ("bmp", "图片"),
        ("avif", "图片"), ("heic", "图片"),
        ("ttf", "字体"), ("otf", "字体"), ("woff", "字体"), ("woff2", "字体"), ("eot", "字体"),
        ("mp3", "媒体"), ("mp4", "媒体"), ("avi", "媒体"), ("mkv", "媒体"),
        ("mov", "媒体"), ("wav", "媒体"), ("flac", "媒体"), ("webm", "媒体"),
        ("zip", "压缩包"), ("rar", "压缩包"), ("7z", "压缩包"), ("tar", "压缩包"),
        ("gz", "压缩包"), ("jar", "压缩包"), ("har", "压缩包"),
        ("bin", "二进制"), ("exe", "二进制"), ("dll", "二进制"), ("so", "二进制"),
        ("apk", "安装包"), ("hap", "安装包"), ("hsp", "安装包"),
        ("db", "数据库"), ("sqlite", "数据库"), ("sqlite3", "数据库"),
        ("log", "日志"), ("map", "构建产物"),
    ];
    let lower = name.to_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    EXT.iter().find(|(e, _)| *e == ext).map(|(_, c)| *c)
}

/// 标志文件（★ 标注：配置/清单/说明类，帮模型快速定位关键文件）
fn is_mark_file(name: &str) -> bool {
    matches!(
        name,
        "pom.xml" | "package.json" | "build-profile.json5" | "oh-package.json5"
            | "Cargo.toml" | "go.mod" | "build.gradle" | "build.gradle.kts"
            | "settings.gradle" | "settings.gradle.kts" | "pyproject.toml"
            | "requirements.txt" | "README.md" | "AGENTS.md" | ".gitignore"
            | "hvigorfile.ts" | "hvigorfile.js" | "tsconfig.json" | "vite.config.ts"
            | "vite.config.js" | "webpack.config.js" | "Dockerfile" | "docker-compose.yml"
    )
}

/// 项目类型探测：根目录标志文件（HarmonyOS 优先，避免与 Node 误判）
fn detect_project_type(root: &Path) -> Option<(String, String)> {
    const MARKS: &[(&str, &str)] = &[
        ("HarmonyOS 工程（DevEco）", "build-profile.json5"),
        ("Java 工程（Maven）", "pom.xml"),
        ("Gradle 工程", "build.gradle"),
        ("Node.js 工程", "package.json"),
        ("Rust 工程（Cargo）", "Cargo.toml"),
        ("Go 工程", "go.mod"),
        ("Python 工程", "pyproject.toml"),
    ];
    for (kind, mark) in MARKS {
        if root.join(mark).is_file() {
            return Some((kind.to_string(), mark.to_string()));
        }
    }
    // .NET 工程以解决方案/项目文件命名（无法用固定文件名，扫一级）
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.ends_with(".sln") || n.ends_with(".csproj") {
                return Some((".NET 工程".to_string(), n));
            }
        }
    }
    None
}

/// 目录浏览统计（跨层累计）
#[derive(Default)]
struct ListStats {
    dirs: usize,
    files: usize,
    bytes: u64,
    /// 聚合类别 → (数量, 字节)
    agg: std::collections::BTreeMap<&'static str, (usize, u64)>,
}

pub(super) async fn list_dir(args: &Value, roots: &[String]) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法浏览文件".into());
    }
    let raw = args["path"].as_str().unwrap_or(".");
    let depth = args["depth"].as_u64().unwrap_or(1).clamp(1, 3) as u32;
    let root = resolve_readable(roots, raw)?;
    if !root.is_dir() {
        return Err(format!("路径不是目录: {}", root.display()));
    }
    // 目录遍历 + metadata 为 IO 密集操作，放 spawn_blocking 避免钉死 tokio worker
    let root_buf = root.clone();
    let roots_owned: Vec<String> = roots.to_vec();
    let (mut out, stats, skipped) = tokio::task::spawn_blocking(move || {
        // .gitignore 规则：项目根规则 + 浏览目标自身规则
        // （root 非项目根时，其自身 .gitignore 由 walk 首层按子规则加载，基准为 root 自身，语义等价）
        let (ignore_rules, start_rel) = load_project_ignore(&root_buf, &roots_owned);
        let mut out = String::new();
        // 浏览项目根时附带项目类型识别，帮助模型快速定位技术栈；
        // Git 仓库提示：list_dir 只反映文件系统现状，查变更/历史应转用 git 工具（自 root 向上查找 .git）
        let in_git = root_buf.ancestors().any(|a| a.join(".git").exists());
        if let Some((kind, mark)) = detect_project_type(&root_buf) {
            let hint = if in_git {
                "；目录在 Git 仓库内，可用 git status 查变更、git log 查历史"
            } else {
                ""
            };
            out.push_str(&format!("（项目类型：{kind}，依据：{mark}{hint}）\n"));
        } else if in_git {
            out.push_str("（目录在 Git 仓库内，可用 git status 查变更、git log 查历史）\n");
        }
        let mut skipped = 0u32;
        let mut stats = ListStats::default();
        fn walk(
            dir: &Path,
            rel: &str,
            depth: u32,
            max_depth: u32,
            shell_left: usize,
            chain: String,
            rules: &[IgnoreRule],
            stats: &mut ListStats,
            skipped: &mut u32,
            out: &mut String,
        ) -> Result<(), String> {
            // 本目录的 .gitignore（多模块工程/子仓库常见）：规则只对其所在子树生效，
            // 与父规则合并后向下传递（项目根已在外层加载，rel 为空时跳过）
            let child_rules = load_child_rules(rules, dir, rel);
            let entries =
                std::fs::read_dir(dir).map_err(|e| format!("读取目录失败 {}: {e}", dir.display()))?;
            let mut items: Vec<(String, bool, u64, std::time::SystemTime)> = Vec::new();
            let mut agg: std::collections::BTreeMap<&'static str, (usize, u64)> =
                std::collections::BTreeMap::new();
            let mut folded = 0usize; // 超出 DIR_SHOW_MAX 后折叠的条目数
            let mut folded_dirs: Vec<String> = Vec::new(); // 被折叠的目录名（前几个，提示模型可继续深入）
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                let Ok(ft) = e.file_type() else { continue };
                let is_dir = ft.is_dir();
                // 隐藏目录（. 开头，白名单除外）+ 已知忽略目录 + .gitignore 命中 → 跳过
                // 注意：含 / 的规则（如 src/main/）按条目自身相对路径匹配，而不是父目录路径
                let hidden =
                    is_dir && name.starts_with('.') && !HIDDEN_ALLOW.contains(&name.as_str());
                let entry_rel = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                if hidden || should_skip_dir(&name)
                    || gitignore_ignored(&child_rules, &name, &entry_rel, is_dir)
                {
                    *skipped += 1;
                    continue;
                }
                if is_dir {
                    stats.dirs += 1;
                    if items.len() < DIR_SHOW_MAX {
                        items.push((name, true, 0, std::time::UNIX_EPOCH));
                    } else {
                        folded += 1;
                        if folded_dirs.len() < 5 {
                            folded_dirs.push(name);
                        }
                    }
                } else if let Some(cat) = agg_category(&name) {
                    // 低价值文件：只聚合统计，不逐条列出（大图/字体/压缩包对结构理解无益）
                    let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                    let ent = agg.entry(cat).or_insert((0, 0));
                    ent.0 += 1;
                    ent.1 += size;
                    stats.files += 1;
                    stats.bytes += size;
                } else if is_mark_file(&name) || items.len() < DIR_SHOW_MAX {
                    // 关键配置/清单文件（★）不受折叠限制：结构信息永远可见
                    let Ok(meta) = e.metadata() else { continue };
                    let size = meta.len();
                    let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                    stats.files += 1;
                    stats.bytes += size;
                    items.push((name, false, size, mtime));
                } else {
                    folded += 1;
                    // 折叠文件也计入统计，保证汇总数字真实
                    let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                    stats.files += 1;
                    stats.bytes += size;
                }
            }
            for (k, (n, sz)) in agg {
                let ent = stats.agg.entry(k).or_insert((0, 0));
                ent.0 += n;
                ent.1 += sz;
            }
            // 单壳目录穿透：仅 1 个子目录且无文件/聚合/折叠时，合并路径继续深入（不消耗 depth）
            if shell_left > 0 && items.len() == 1 && items[0].1 && folded == 0 {
                let sub = items[0].0.clone();
                let child_rel = if rel.is_empty() {
                    sub.clone()
                } else {
                    format!("{rel}/{sub}")
                };
                // 先借用 sub 构造路径，再 move 进 new_chain（顺序敏感：sub 随后被转移）
                let joined = dir.join(&sub);
                let new_chain = if chain.is_empty() {
                    sub
                } else {
                    format!("{chain}/{sub}")
                };
                return walk(
                    &joined,
                    &child_rel,
                    depth,
                    max_depth,
                    shell_left - 1,
                    new_chain,
                    &child_rules,
                    stats,
                    skipped,
                    out,
                );
            }
            // 目录优先，同级按名称排序
            items.sort_by(|a, b| {
                b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
            });
            let indent = "  ".repeat(depth as usize);
            for (name, is_dir, size, mtime) in &items {
                if *is_dir {
                    let disp = if chain.is_empty() {
                        name.clone()
                    } else {
                        format!("{chain}/{name}")
                    };
                    out.push_str(&format!("{indent}[目录] {disp}/\n"));
                    // 目录超限（folded>0）时结构不完整，不再深入，避免输出缺块误导模型
                    if depth < max_depth && folded == 0 {
                        let child_rel = if rel.is_empty() {
                            name.clone()
                        } else {
                            format!("{rel}/{name}")
                        };
                        walk(
                            &dir.join(name),
                            &child_rel,
                            depth + 1,
                            max_depth,
                            SHELL_PENETRATE_MAX,
                            String::new(),
                            &child_rules,
                            stats,
                            skipped,
                            out,
                        )?;
                    }
                } else {
                    let star = if is_mark_file(name) { "★" } else { "" };
                    out.push_str(&format!(
                        "{indent}{star}[文件] {name}  {}  {}\n",
                        human_size(*size),
                        fmt_time(*mtime)
                    ));
                }
            }
            if folded > 0 {
                let dir_hint = if folded_dirs.is_empty() {
                    String::new()
                } else {
                    format!(
                        "（含目录：{} 等）",
                        folded_dirs.iter().map(|d| format!("{d}/")).collect::<Vec<_>>().join("、")
                    )
                };
                out.push_str(&format!(
                    "{indent}…（该目录共 {} 项，其余 {folded} 项省略{dir_hint}；如需查看请直接对该目录再调 list_dir）\n",
                    items.len() + folded
                ));
            }
            Ok(())
        }
        walk(
            &root_buf,
            &start_rel,
            0,
            depth,
            SHELL_PENETRATE_MAX,
            String::new(),
            &ignore_rules,
            &mut stats,
            &mut skipped,
            &mut out,
        )?;
        Ok::<(String, ListStats, u32), String>((out, stats, skipped))
    })
    .await
    .map_err(|e| format!("目录遍历任务异常: {e}"))??;
    if out.is_empty() {
        out.push_str("（空目录）\n");
    }
    // 汇总：计数 + 聚合类别（低价值文件不逐条列出但明确告知，避免模型误以为目录里没有）
    out.push_str(&format!(
        "（目录: {}，共 {} 个文件、{} 个目录，总大小 {}；已跳过 {skipped} 个忽略项）\n",
        root.display(),
        stats.files,
        stats.dirs,
        human_size(stats.bytes)
    ));
    if !stats.agg.is_empty() {
        let parts: Vec<String> = stats
            .agg
            .iter()
            .map(|(c, (n, sz))| format!("{c} {n} 个/{}", human_size(*sz)))
            .collect();
        out.push_str(&format!(
            "（聚合未逐一列出：{}；如需查看请对该目录单独 list_dir）\n",
            parts.join("、")
        ));
    }
    // 保尾截断：统计汇总在尾部，超长时纯截头会让模型拿到残缺结构认知
    Ok(truncate_out_head_tail(&out, 3000))
}

fn append_large_line_preview(
    out: &mut String,
    output_chars: &mut usize,
    line_no: u64,
    preview: &[u8],
    line_truncated: bool,
) -> bool {
    const OUTPUT_CHARS: usize = 14_000;
    let decoded = String::from_utf8_lossy(preview);
    let snippet: String = decoded.trim_end_matches('\r').chars().take(240).collect();
    let suffix = if line_truncated || decoded.chars().count() > 240 {
        " …(本行已截断)"
    } else {
        ""
    };
    let rendered = format!("{line_no:>5} │ {snippet}{suffix}\n");
    let chars = rendered.chars().count();
    if output_chars.saturating_add(chars) > OUTPUT_CHARS {
        return false;
    }
    out.push_str(&rendered);
    *output_chars += chars;
    true
}

/// 大文本文件的固定内存窗口读取。扫描到目标行只需要 O(1) 额外内存；单行预览也有
/// 独立上限，避免无换行的病理文件把进程内存撑满。行偏移 sidecar 上线后可把深页扫描
/// 从 O(n) 降为 seek + O(window)。
fn read_large_file_window(path: &Path, start: u64, limit: u64) -> Result<String, String> {
    use std::io::Read;

    const READ_BUFFER: usize = 64 * 1024;
    const LINE_PREVIEW_BYTES: usize = 4 * 1024;
    let file = std::fs::File::open(path).map_err(|e| format!("读取文件失败: {e}"))?;
    let meta = file.metadata().map_err(|e| format!("读取文件元数据失败: {e}"))?;
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let mut reader = std::io::BufReader::with_capacity(READ_BUFFER, file);
    let mut chunk = [0u8; READ_BUFFER];
    let mut line_no = 1u64;
    let mut selected = 0u64;
    let mut preview = Vec::with_capacity(LINE_PREVIEW_BYTES);
    let mut line_truncated = false;
    let mut scanned = 0usize;
    let mut out = String::new();
    let mut output_chars = 0usize;
    let mut next_start: Option<u64> = None;

    'scan: loop {
        let read = reader.read(&mut chunk).map_err(|e| format!("读取文件失败: {e}"))?;
        if read == 0 {
            break;
        }
        for byte in &chunk[..read] {
            if scanned < 8192 && *byte == 0 {
                return Err(format!("文件是二进制（{}），无法以文本方式读取", human_size(meta.len())));
            }
            scanned = scanned.saturating_add(1);
            let selected_line = line_no >= start;
            if *byte == b'\n' {
                if selected_line {
                    if selected >= limit {
                        next_start = Some(line_no);
                        break 'scan;
                    }
                    if !append_large_line_preview(
                        &mut out,
                        &mut output_chars,
                        line_no,
                        &preview,
                        line_truncated,
                    ) {
                        next_start = Some(line_no);
                        break 'scan;
                    }
                    selected += 1;
                }
                line_no = line_no.saturating_add(1);
                preview.clear();
                line_truncated = false;
            } else if selected_line {
                if preview.len() < LINE_PREVIEW_BYTES {
                    preview.push(*byte);
                } else {
                    line_truncated = true;
                }
            }
        }
    }

    if next_start.is_none() && !preview.is_empty() && line_no >= start {
        if selected >= limit
            || !append_large_line_preview(
                &mut out,
                &mut output_chars,
                line_no,
                &preview,
                line_truncated,
            )
        {
            next_start = Some(line_no);
        } else {
            selected += 1;
        }
    }
    if out.is_empty() {
        out.push_str("（目标窗口为空；start 可能已超过文件末尾）\n");
    }
    let window_end = if selected == 0 {
        0
    } else {
        start.saturating_add(selected).saturating_sub(1)
    };
    let cursor = next_start.map_or_else(|| "end".into(), |line| line.to_string());
    Ok(format!(
        "文件 {}（{}；file_version=stat:{mtime_ns}:{}；流式窗口=L{start}-L{window_end}；next_start={cursor}；总行数=未预计算）：\n{out}",
        path.display(),
        human_size(meta.len()),
        meta.len(),
    ))
}

pub(super) async fn read_file(args: &Value, roots: &[String]) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法读取文件".into());
    }
    // Request/Spec 分离：宽松参数 ReadFileRequest → 显式 resolve() 产出严格规范 ReadFileSpec
    let spec = ReadFileRequest::from_args(args)?.resolve(roots)?;
    let p = &spec.path;
    if spec.stream_large {
        let path = p.clone();
        let start = spec.start;
        let lines = spec.lines;
        return tokio::task::spawn_blocking(move || read_large_file_window(&path, start, lines))
            .await
            .map_err(|e| format!("流式读取文件任务异常: {e}"))?;
    }
    // 整文件读取 + metadata 为 IO 操作（大文件/日志可达数 MB~数十 MB），
    // 放 spawn_blocking 避免钉死 tokio worker
    let p_buf = p.clone();
    let (meta, bytes) = tokio::task::spawn_blocking(move || -> Result<(std::fs::Metadata, Vec<u8>), String> {
        let meta = std::fs::metadata(&p_buf).map_err(|e| e.to_string())?;
        let bytes = std::fs::read(&p_buf).map_err(|e| format!("读取文件失败: {e}"))?;
        Ok((meta, bytes))
    })
    .await
    .map_err(|e| format!("读取文件任务异常: {e}"))??;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err(format!(
            "文件是二进制（{}），无法以文本方式读取",
            human_size(meta.len())
        ));
    }
    // 记录文件指纹（外部修改检测基线）
    stamp_put(p, &meta, &bytes);
    let file_version = file_content_version(&bytes);
    // 编码：UTF-8 严格校验；非 UTF-8 用 GBK 解码展示（Windows 老项目常见），并标注编码避免误导
    let (text, enc_note) = match std::str::from_utf8(&bytes) {
        Ok(s) => {
            // BOM 剥离展示：BOM 是不可见字符，模型拼接 old 时不会带 BOM，
            // 展示中保留会让首行看起来与原文不同；剥离并标注。
            if let Some(b) = s.strip_prefix('\u{feff}') {
                (b.to_string(), "（UTF-8 含 BOM，编辑时自动兼容）".to_string())
            } else {
                (s.to_string(), String::new())
            }
        }
        Err(_) => {
            let (t, _, had_err) = encoding_rs::GBK.decode(&bytes);
            if had_err {
                (String::from_utf8_lossy(&bytes).to_string(), "（编码：非 UTF-8/GBK，展示可能失真）".to_string())
            } else {
                (t.into_owned(), "（编码：GBK，非 UTF-8，只读展示；如需编辑请先转换为 UTF-8）".to_string())
            }
        }
    };
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();

    // 骨架模式：只提取结构（导入/类/函数/接口/组件/装饰器等），帮助 Agent 先了解大文件
    if spec.outline {
        return Ok(render_outline(p, &lines, meta.len(), spec.outline_page, spec.outline_filter.as_deref()));
    }

    let start = spec.start as usize;
    let limit = spec.lines as usize;
    // start 超出总行数：明确提示，避免误输出“文件为空”误导模型
    if total > 0 && start > total {
        return Ok(format!(
            "start={start} 超出文件总行数（共 {total} 行），请调小 start，或传 lines 参数从文件头部读取"
        ));
    }
    let begin = if start > total { total } else { start - 1 };
    let end = if limit > 0 {
        (begin + limit).min(total)
    } else {
        total
    };

    // ---- 语言感知的代码块对齐 ----
    // 原则：读取窗口绝不把代码块截断在中间。窗口起点若落在方法/函数内部 → 上扩到完整块首行；
    // 窗口末尾若仍在块内 → 补齐到块尾（结束符一定可见）。
    // 依据扩展名识别语言：花括号语言按 {}()[] 成对匹配，Python 按 def/class 缩进块。
    // 文本/数据类文件（.md/.json/.txt 等）无结构化块，行为与旧版一致。
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mut win_begin = begin;
    let mut win_end = end;
    // 1) 起始行上扩：begin 在方法级根块内部 → 从完整块首行开始展示
    if win_begin < total {
        if let Some((o, _)) = find_root_block(&lines, win_begin, &ext) {
            if o < win_begin {
                win_begin = o;
            }
        }
    }
    // 2) 结束行补齐：窗口最后一行仍处块内 → 延伸到块尾（锚定 win_begin，
    //    与窗口起点同一层级的块，避免跳进内层块而漏掉外层方法尾部）
    if win_begin < total && win_end > win_begin && win_end < total {
        let last = win_end - 1;
        if let Some((_, c)) = find_block_anchored(&lines, last, win_begin, &ext) {
            if c + 1 > win_end {
                win_end = (c + 1).min(total);
            }
        }
    }
    let block_note = if win_begin != begin || win_end != end {
        format!(
            "（已按语言代码块自动对齐 L{}-L{}：保证方法/块完整、不丢结束符）\n",
            win_begin + 1, win_end
        )
    } else {
        String::new()
    };

    // 单次输出上限：普通模式 2000 行 / 15000 字符；代码块补齐场景预算放大到 40000 字符，
    // 尽量完整输出整个方法。病理级超大块由下方给出完整行区间与结束符位置提示。
    const MAX_LINES: usize = 2000;
    const MAX_CHARS: usize = 15000;
    const BLOCK_CHARS: usize = 40000;
    let block_expanded = !block_note.is_empty();
    let char_budget = if block_expanded { BLOCK_CHARS } else { MAX_CHARS };
    let line_budget = if block_expanded { MAX_LINES * 2 } else { MAX_LINES };
    // 注释清洗：文件注释占比高时，连续长注释块折叠为一行摘要。
    // 痛点：license/文件头大段注释会吃掉字符预算，代码本身还没读到就被截断，
    // 喂给模型的是一堆注释而看不到代码。短注释块信息密度高，保留原文。
    let comment_ratio = if is_code_ext(&ext) {
        lines.iter().filter(|l| is_comment_line(l, &ext)).count() as f64 / total.max(1) as f64
    } else {
        0.0
    };
    let mut folded_runs = 0usize; // 折叠的注释块数
    let mut folded_lines = 0usize; // 折叠的注释总行数
    fn flush_comment_buf(
        out: &mut String,
        buf: &[usize],
        lines: &[&str],
        shown_lines: &mut usize,
        folded_runs: &mut usize,
        folded_lines: &mut usize,
    ) {
        if buf.is_empty() {
            return;
        }
        let run = buf.len();
        if run >= COMMENT_FOLD_MIN {
            let first = buf[0] + 1;
            let last = buf[run - 1] + 1;
            out.push_str(&format!(
                "{first:>5} │ …(L{first}-L{last}：{run} 行注释已折叠；start={first}/lines={run} 可看原文)…\n"
            ));
            *folded_runs += 1;
            *folded_lines += run;
        } else {
            // 短注释块：逐行展示
            for abs in buf {
                let s: String = lines[*abs].chars().take(240).collect();
                out.push_str(&format!("{:>5} │ {s}\n", abs + 1));
            }
        }
        *shown_lines += run;
    }
    let mut out = String::new();
    let mut shown_lines = 0usize; // 已展示的真实行数（截断时用于提示续读位置）
    let mut cut_block: Option<(usize, usize)> = None; // 字符/行数截断命中的块（用于完整区间提示）
    let mut comment_buf: Vec<usize> = Vec::new(); // 累积的连续注释行（绝对行号）
    for (i, line) in lines[win_begin..win_end].iter().enumerate() {
        if i >= line_budget || out.chars().count() >= char_budget {
            // 命中预算：若窗口结束符仍不可见，记录截断处所在块，给出完整区间与结束符位置
            let cut_abs = (win_begin + i).saturating_sub(1).min(win_end - 1);
            if block_expanded && cut_abs >= win_begin {
                cut_block = find_enclosing_block(&lines, cut_abs, &ext);
            }
            break;
        }
        let abs = win_begin + i;
        // 注释行先累积（判断是否成块），代码行先 flush 注释块再输出
        if comment_ratio > 0.35 && is_comment_line(line, &ext) {
            comment_buf.push(abs);
            continue;
        }
        flush_comment_buf(
            &mut out,
            &comment_buf,
            &lines,
            &mut shown_lines,
            &mut folded_runs,
            &mut folded_lines,
        );
        comment_buf.clear();
        let snippet: String = line.chars().take(240).collect();
        out.push_str(&format!("{:>5} │ {snippet}\n", abs + 1));
        shown_lines += 1;
    }
    // 窗口结尾的注释块同样处理
    flush_comment_buf(
        &mut out,
        &comment_buf,
        &lines,
        &mut shown_lines,
        &mut folded_runs,
        &mut folded_lines,
    );
    // 截断提示：优先给出“块没读完”的完整区间与结束符行号（防模型基于残缺片段编辑）；
    // 普通截断则告知已读到第几行、从哪继续。
    // 截断时把完整窗口落盘（对齐 opencode tool-output-store），模型可 read_file 读回全量。
    // read 豁免（对齐 deepseek-harness spill-policy）：目标文件本身就在 tool-output 目录内
    // （模型读回落盘产物的场景）时不再落盘——否则会形成 read → spill → read 死循环
    // （读回 5000 行落盘文件 → 又截断又落盘新文件），直接给截断提示让模型用 start/lines 续读。
    let p_str = p.to_string_lossy();
    let in_tool_output =
        p_str.contains("\\.deveco-agent\\tool-output\\") || p_str.contains("/.deveco-agent/tool-output/");
    let overflow_path: Option<String> = if !in_tool_output && shown_lines < win_end - win_begin {
        // 完整窗口（带行号）落盘到 .deveco-agent/tool-output/，路径标记喂给模型
        let full_win: String = lines[win_begin..win_end]
            .iter()
            .enumerate()
            .map(|(i, l)| format!("{:>5} │ {}\n", win_begin + i + 1, l))
            .collect();
        let dir = Path::new(roots.first().map(String::as_str).unwrap_or(""))
            .join(".deveco-agent")
            .join("tool-output");
        std::fs::create_dir_all(&dir).ok();
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S%3f");
        let file = dir.join(format!("read-{ts}.txt"));
        if std::fs::write(&file, full_win).is_ok() {
            Some(file.to_string_lossy().to_string())
        } else {
            None
        }
    } else {
        None
    };
    if shown_lines < win_end - win_begin {
        if let Some((o, c)) = cut_block.filter(|(_, c)| *c >= (win_begin + shown_lines).saturating_sub(1)) {
            let block_lines = c - o + 1;
            let block_chars: usize = lines[o..=c].iter().map(|l| l.chars().count()).sum();
            out.push_str(&format!(
                "…(代码块过大：所在块跨 L{}-L{}（{} 行、约 {} 字符），超出单次输出上限 {char_budget} 字符；结束符在第 {} 行。如需完整读取请传 start={}/lines={})\n",
                o + 1,
                c + 1,
                block_lines,
                block_chars,
                c + 1,
                o + 1,
                block_lines
            ));
        } else {
            let path_hint = overflow_path
                .as_deref()
                .map(|p| format!("；完整内容已保存到 {p}，可 read_file 读取"))
                .unwrap_or_default();
            out.push_str(&format!(
                "…(已截断，共 {total} 行，仅显示到第 {} 行{path_hint}；可传 start={} 继续读取)\n",
                win_begin + shown_lines,
                win_begin + shown_lines + 1
            ));
        }
    }
    if out.is_empty() {
        out.push_str("（文件为空或没有可显示的内容）\n");
    }
    // 注释清洗提示：折叠发生时说明原文结构（模型可据此决定是否 outline 或精读注释区）
    let comment_note = if folded_lines > 0 {
        format!(
            "（注释占 {:.0}%，长注释块已折叠 {folded_runs} 处/共 {folded_lines} 行；可用 outline=true 查看结构骨架）\n",
            comment_ratio * 100.0
        )
    } else if comment_ratio >= 0.4 {
        format!(
            "（注释占 {:.0}%，文件注释较多；可用 outline=true 查看结构骨架）\n",
            comment_ratio * 100.0
        )
    } else {
        String::new()
    };
    let processed_end = (win_begin + shown_lines).min(win_end);
    let window_start = if total == 0 { 0 } else { win_begin + 1 };
    let next_start = if processed_end < total {
        (processed_end + 1).to_string()
    } else {
        "end".to_string()
    };
    Ok(truncate_out_max(
        &format!(
            "文件 {}{enc_note}（{}，共 {total} 行；file_version=sha256:{file_version}；窗口=L{}-L{}；next_start={next_start}）：\n{block_note}{comment_note}{out}",
            p.display(),
            human_size(meta.len()),
            window_start,
            processed_end,
        ),
        if block_expanded { BLOCK_CHARS + 2000 } else { 15000 },
    ))
}

/// outline 条目的块区间：定义行 idx 所开块的首尾行（0 起，含）。
/// 花括号语言：从定义行起找第一个「深度 0 的 `{`」（支持多行签名 fn foo(\\n..\\n) -> Bar {），
/// 再取其配对闭合行；Python：def/class 缩进块。找不到（声明 `;`、属性等）返回 None。
/// bindex：预构建的块索引（Some 时闭行查 open_close 表 O(1)，替代逐次 find_matching_close O(n)；
/// None 时回退逐次扫描。两者语义等价：都取 b 行「最后一个未闭合开括号」的闭合行）。
fn outline_block_range(
    lines: &[&str],
    idx: usize,
    ext: &str,
    bindex: Option<&BlockIndex>,
) -> Option<(usize, usize)> {
    // 声明（`;` 结尾）/属性/装饰器行无块体：直接无区间，
    // 否则向前搜 `{` 会误吞下一个定义的块
    let t = lines.get(idx)?.trim();
    if t.ends_with(';') || t.starts_with("#[") || t.starts_with('@') {
        return None;
    }
    match block_style(ext) {
        BlockStyle::Indent => find_indent_block(lines, idx),
        BlockStyle::None => None,
        BlockStyle::Brace => {
            // 从定义行起做括号深度扫描，找方法体 `{`（签名内 () 的深度已计入）
            let mut depth: i32 = 0;
            let mut sc = LineScanner::default();
            let mut brace_line: Option<usize> = None;
            let search_end = (idx + 40).min(lines.len());
            for (i, line) in lines.iter().enumerate().take(search_end).skip(idx) {
                let mut found = false;
                sc.scan(line, ext, |c| {
                    if found {
                        return;
                    }
                    match c {
                        '{' if depth == 0 => {
                            brace_line = Some(i);
                            found = true;
                        }
                        '{' | '(' | '[' => depth += 1,
                        '}' | ')' | ']' => depth -= 1,
                        _ => {}
                    }
                });
                if found {
                    break;
                }
            }
            let b = brace_line?;
            let close = match bindex {
                Some(bi) => BlockIndex::get(&bi.open_close, b)?,
                None => find_matching_close(lines, b, ext)?,
            };
            Some((idx.min(b), close))
        }
    }
}

/// 生成代码文件骨架：仅保留结构定义行（导入、类/接口/struct/enum、函数/方法、组件、装饰器），
/// 帮助 Agent 在不读取全文的情况下快速了解大文件结构，再决定精读哪段。
/// 行号列为「定义行-块尾行」区间（语言感知块对齐联动）：可直接按区间整读/整块编辑。
pub(super) fn render_outline(
    p: &Path,
    lines: &[&str],
    byte_len: u64,
    page: u32,
    filter: Option<&str>,
) -> String {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mut out = String::new();
    let mut last_was_import = false;
    let mut import_count = 0usize;
    let mut total_entries = 0usize; // 全量结构项数（分页统计用，不随页截断；filter 时只计匹配项）
    let mut shown = 0usize; // 本页已显示条数
    const MAX_ENTRIES: usize = 200; // 每页条数
    let skip = (page as usize - 1) * MAX_ENTRIES;
    // 块索引复用：花括号语言一次 O(n) 构建，200 条区间的闭行查询从 O(200n) 降为 O(n)+O(1)
    let style = block_style(&ext);
    let bindex = if style == BlockStyle::Brace {
        Some(BlockIndex::build(lines, &ext))
    } else {
        None
    };
    // 层级跟踪：花括号语言按未闭括号深度，Python 按行首缩进/4；本行处理前的深度 = 该行定义的结构层级
    let mut depth: usize = 0;
    let mut dsc = LineScanner::default();

    for (idx, raw) in lines.iter().enumerate() {
        let depth_here = match style {
            BlockStyle::Brace => depth.min(10),
            BlockStyle::Indent => (raw.chars().take_while(|&c| c == ' ').count() / 4).min(10),
            BlockStyle::None => 0,
        };
        if style == BlockStyle::Brace {
            dsc.scan(raw, &ext, |c| {
                if matches!(c, '{' | '(' | '[') {
                    depth += 1;
                } else {
                    depth = depth.saturating_sub(1);
                }
            });
        }
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('*') || line.starts_with("/*") {
            continue;
        }
        let lineno = idx + 1;

        // 导入语句：折叠显示（只给首尾与数量），避免 import 区占用大量篇幅
        let is_import = line.starts_with("import ")
            || line.starts_with("import\t")
            || line.starts_with("use ")
            || line.starts_with("const ") && line.contains("require(")
            || line.starts_with("#import");
        if is_import {
            import_count += 1;
            last_was_import = true;
            continue;
        } else if last_was_import {
            if skip == 0 {
                // 导入折叠摘要只在第 1 页显示（后续页的导入统计意义不大）
                out.push_str(&format!("  … ({import_count} 条导入已折叠)…\n"));
            }
            last_was_import = false;
        }

        let kind = outline_kind(line, &ext);
        if let Some(label) = kind {
            // kind 过滤：不匹配的条目不渲染也不计数（分页在过滤后的集合上进行）
            if let Some(f) = filter {
                if !label.contains(f) {
                    continue;
                }
            }
            // 分页：先数全量序号，再决定本页是否渲染
            if total_entries >= skip && shown < MAX_ENTRIES {
                let snippet: String = raw.chars().take(160).collect();
                // 块区间联动：定义行 → 完整块首尾（声明/属性等由 helper 判无块）
                let loc = match outline_block_range(lines, idx, &ext, bindex.as_ref()) {
                    Some((o, c)) if c > o => format!("{:>5}-{}", o + 1, c + 1),
                    _ => format!("{lineno:>5}"),
                };
                // 层级缩进：嵌套定义（类内方法等）按深度缩进，骨架结构一眼可读
                let indent = if depth_here > 0 { "  ".repeat(depth_here) } else { String::new() };
                out.push_str(&format!("{loc} │ {indent}{label} {}\n", snippet.trim()));
                shown += 1;
            }
            total_entries += 1;
        }
    }
    if last_was_import && skip == 0 {
        out.push_str(&format!("  … ({import_count} 条导入已折叠)…\n"));
    }

    let pages = total_entries.div_ceil(MAX_ENTRIES);
    let mut header = format!(
        "文件 {}（{}，共 {} 行）骨架（行号列为 定义行-块尾行 区间）：\n",
        p.display(),
        human_size(byte_len),
        lines.len()
    );
    if pages > 1 {
        let page = page.min(pages as u32); // 超界页码钳到末页
        header.push_str(&format!("共 {total_entries} 条结构项 / {pages} 页，当前第 {page} 页（每页 {MAX_ENTRIES} 条）。\n"));
    }
    if let Some(f) = filter {
        header.push_str(&format!("（已按类型过滤：仅显示「{f}」类条目，共 {total_entries} 条；去掉 outline_filter 查看全部）\n"));
    }
    header.push_str("提示：按区间整块操作——read_file {\"start\":区间起点,\"lines\":区间长度} 精读整个方法；edit_file {\"start\":区间起点} 整块替换/删除。\n");
    if out.is_empty() {
        if total_entries == 0 {
            header.push_str("（未能识别出结构化定义，可能是非代码文件或使用了非常规语法；请用 start/lines 直接读取）\n");
        } else {
            header.push_str(&format!(
                "outline_page={page} 超出总页数（共 {pages} 页）；请传 1–{pages} 之间的页码\n"
            ));
        }
    } else {
        header.push_str(&out);
        if pages > 1 && (page as usize) < pages {
            header.push_str(&format!(
                "…（还有下一页：read_file 传 outline=true,outline_page={} 查看后续 {MAX_ENTRIES} 条）\n",
                page + 1
            ));
        }
    }
    truncate_out_max(&header, 12000)
}

/// 判断某行是否为结构定义，返回其分类标签。
pub(super) fn outline_kind(line: &str, ext: &str) -> Option<&'static str> {
    // ArkTS/TS/JS：装饰器、类/接口/枚举、函数/方法、组件
    let is_ts = matches!(ext, "ets" | "ts" | "tsx" | "js" | "jsx");
    if is_ts {
        if line.starts_with('@')
            && (line.starts_with("@Entry")
                || line.starts_with("@Component")
                || line.starts_with("@Builder")
                || line.starts_with("@CustomDialog")
                || line.starts_with("@State")
                || line.starts_with("@Prop")
                || line.starts_with("@Link")
                || line.starts_with("@Provide")
                || line.starts_with("@Consume")
                || line.starts_with("@Watch")
                || line.starts_with("@StorageLink")
                || line.starts_with("@Router"))
        {
            return Some("装饰器");
        }
        if starts_with_any(line, &["export ", "declare "]) {
            let body = line
                .trim_start_matches("export ")
                .trim_start_matches("default ")
                .trim_start_matches("declare ");
            return outline_kind(body.trim(), ext);
        }
        if starts_with_word(line, "class") || starts_with_word(line, "interface")
            || starts_with_word(line, "enum") || starts_with_word(line, "type")
            || starts_with_word(line, "abstract class")
        {
            return Some("类型");
        }
        if starts_with_word(line, "function") || starts_with_word(line, "async function") {
            return Some("函数");
        }
        // 方法/箭头函数：name(...) {  或  name = (...) =>
        if (line.contains('(') && line.ends_with('{') && looks_like_method(line))
            || (line.contains("=>") && looks_like_arrow(line))
        {
            return Some("方法");
        }
        if starts_with_word(line, "struct") && ext == "ets" {
            return Some("组件");
        }
        if line.starts_with("build(") || line == "build() {" {
            return Some("构建");
        }
        return None;
    }

    // Rust
    if ext == "rs" {
        if starts_with_word(line, "fn") || starts_with_word(line, "async fn")
            || starts_with_word(line, "pub fn") || starts_with_word(line, "pub async fn")
        {
            return Some("函数");
        }
        if starts_with_any(line, &["struct ", "enum ", "trait ", "type ", "mod ", "impl ", "pub struct ", "pub enum ", "pub trait ", "pub mod "]) {
            return Some("类型");
        }
        if line.starts_with("#[") {
            return Some("属性");
        }
        return None;
    }

    // Python
    if ext == "py" {
        if starts_with_word(line, "def") || starts_with_word(line, "async def") {
            return Some("函数");
        }
        if starts_with_word(line, "class") {
            return Some("类型");
        }
        return None;
    }

    // C/C++/Java/Kotlin/Swift
    if matches!(ext, "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "java" | "kt" | "swift") {
        if starts_with_word(line, "class") || starts_with_word(line, "interface")
            || starts_with_word(line, "struct") || starts_with_word(line, "enum")
        {
            return Some("类型");
        }
        if line.contains('(') && (line.ends_with('{') || line.ends_with(';')) && looks_like_method(line) {
            return Some("函数");
        }
        if line.starts_with("@IBAction") || line.starts_with("@IBOutlet") || line.starts_with("@Override") {
            return Some("注解");
        }
        return None;
    }

    // Go
    if ext == "go" {
        // 注意用 starts_with（"func " 自带空格已保证整词）：starts_with_word 要求
        // word 后非标识符字符，带空格调用会因函数名紧跟而恒 false（历史 bug）
        if line.starts_with("func ") {
            return Some("函数");
        }
        if line.starts_with("type ") {
            return Some("类型");
        }
        return None;
    }

    // Dart/C#/PHP/Scala/Groovy（block_style 认识但此前 outline 不识别 → 骨架为空）
    if matches!(ext, "dart" | "cs" | "php" | "scala" | "groovy") {
        // 剥离修饰符前缀后按定义关键字判定
        let mut t = line;
        loop {
            let stripped = ["public ", "private ", "protected ", "internal ", "static ",
                "final ", "abstract ", "open ", "override ", "async ", "sealed ", "case ", "extern "]
                .iter()
                .find_map(|p| t.strip_prefix(p));
            match stripped {
                Some(s) => t = s,
                None => break,
            }
        }
        if starts_with_any(t, &["class ", "interface ", "struct ", "enum ", "object ", "trait ", "mixin "]) {
            return Some("类型");
        }
        if t.starts_with("def ") || t.starts_with("function ") || t.starts_with("func ") {
            return Some("函数");
        }
        // 通用方法签名：含 ( 且以 { 结尾（void main() { / Future<void> f() { / void F() {）
        if t.contains('(') && line.ends_with('{') && looks_like_method(line) {
            return Some("函数");
        }
        return None;
    }

    None
}

pub(super) fn starts_with_word(s: &str, word: &str) -> bool {
    if !s.starts_with(word) {
        return false;
    }
    s.as_bytes().get(word.len()).is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_')
}

pub(super) fn starts_with_any(s: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| s.starts_with(p))
}

/// 粗略判断是否为方法定义（含括号、以 { 结尾，且像 `name(...)` 开头），排除控制流语句。
pub(super) fn looks_like_method(line: &str) -> bool {
    let first = line.split('(').next().unwrap_or("").trim();
    if first.is_empty() {
        return false;
    }
    let name = first.split_whitespace().last().unwrap_or("");
    if name.is_empty() {
        return false;
    }
    // 排除 if/for/while/switch/catch 等控制流
    if matches!(name, "if" | "for" | "while" | "switch" | "catch" | "when" | "return" | "else") {
        return false;
    }
    name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
}

pub(super) fn looks_like_arrow(line: &str) -> bool {
    let first = line.split('=').next().unwrap_or("").trim();
    if first.is_empty() {
        return false;
    }
    let name = first.split_whitespace().last().unwrap_or("");
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
}

// ---------- 语言感知的成对代码块 ----------
// 原则：文件操作（读取截取/编辑/查找/删除）绝不把代码块截断在中间。
// 依据扩展名识别语言结构——花括号语言按 `{}` `()` `[]` 成对匹配（字符串/注释感知），
// Python 按 def/class 缩进块——从而整块操作：一个方法行数再多，也给出完整方法，
// 避免固定行数漏掉块结束符导致后续编辑基于残缺片段。

/// 代码块结构风格（语言感知）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum BlockStyle {
    /// 花括号语言：`{}` 成对（`()`/`[]` 作为同组括号参与配对）
    Brace,
    /// 缩进语言（Python）：块由 `def/class ...:` + 缩进界定
    Indent,
    /// 无结构化块（文本/数据/样式等）：不做块级操作
    None,
}

pub(super) fn block_style(ext: &str) -> BlockStyle {
    match ext.to_lowercase().as_str() {
        "py" | "pyw" => BlockStyle::Indent,
        // 花括号语言（含 ArkTS/TS/JS/Rust/C 系/Go/Kotlin/Swift/C#/Dart/PHP/Shell/Vue 等）
        "rs" | "c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "java" | "kt" | "kts"
        | "swift" | "cs" | "dart" | "scala" | "groovy" | "go" | "php" | "ts" | "tsx"
        | "js" | "jsx" | "mjs" | "cjs" | "ets" | "vue" | "svelte" | "sh" | "bash"
        | "zsh" | "fish" => BlockStyle::Brace,
        _ => BlockStyle::None,
    }
}

/// 跨行扫描状态：块注释（/* */）与多行字符串（` 模板串、三引号等）跨行持续
#[derive(Default)]
pub(super) struct LineScanner {
    in_block_comment: bool,
    in_str: Option<char>,
    /// Rust 原始字符串（r"…" / r#"…"# / r##"…"##，含 br/cr 前缀）：
    /// 记录开头的 `#` 数量，闭合 = `"` 后跟同等数量 `#`；无转义语义。
    in_raw: Option<u32>,
    /// 三引号字符串（Kotlin/Dart/Scala/Groovy 的 `"""`、Python 的 `"""`/`'''`）：
    /// 记录定界引号字符，闭合 = 同种三连引号；无转义语义，可跨行。
    in_triple: Option<char>,
    /// JS 系正则字面量（/…/）：字符类 [ ] 内的 `/` 不闭合。
    /// 正则字面量不允许裸换行，行尾自动退出（防止一行误判吞掉后续行）。
    in_regex: bool,
    in_regex_class: bool,
    /// 上一个非空白字符（正则/除法消歧启发式用，跨行保留）
    prev_char: Option<char>,
}

impl LineScanner {
    /// 逐字符扫描一行，把「有意义的括号字符」按出现顺序交给 cb；
    /// 字符串（" ' `）、三引号字符串、Rust 原始字符串、正则字面量、转义、
    /// 行注释（//，Python/Shell 的 #）、块注释（/* */）内的字符全部跳过。
    pub(super) fn scan(&mut self, line: &str, ext: &str, mut cb: impl FnMut(char)) {
        let hash_comment = matches!(ext, "py" | "pyw" | "sh" | "bash" | "zsh" | "fish");
        let raw_str = ext == "rs";
        // 三引号字符串语言：Python 双/三单引号，JVM/Dart 系仅三双引号
        let triple: &[u8] = match ext {
            "py" | "pyw" => b"\"'",
            "kt" | "kts" | "dart" | "scala" | "groovy" => b"\"",
            _ => b"",
        };
        // 正则字面量语言（JS 系）：`/` 可为除法或正则开头，用前驱字符消歧
        let regex_lang = matches!(ext, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "ets" | "vue" | "svelte");
        let mut chars = line.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            // 原始字符串内：无转义，唯一出口是 `"` + N 个 `#`
            if let Some(n) = self.in_raw {
                if c == '"' {
                    let rest = &line[i + 1..];
                    let hashes: String = rest.chars().take(n as usize).collect();
                    if hashes.len() == n as usize && hashes.chars().all(|h| h == '#') {
                        for _ in 0..n {
                            chars.next(); // 消费闭合的 #
                        }
                        self.in_raw = None;
                    }
                }
                continue;
            }
            // 三引号字符串内：无转义，唯一出口是同种三连引号
            if let Some(q) = self.in_triple {
                if c == q && line[i..].starts_with(&format!("{q}{q}{q}")) {
                    chars.next();
                    chars.next(); // 消费闭合的其余两个引号
                    self.in_triple = None;
                }
                continue;
            }
            if let Some(q) = self.in_str {
                if c == '\\' {
                    chars.next(); // 转义：跳过下一字符
                    continue;
                }
                if c == q {
                    self.in_str = None;
                }
                continue;
            }
            if self.in_regex {
                match c {
                    '\\' => {
                        chars.next(); // 转义：跳过下一字符
                        continue;
                    }
                    '[' => self.in_regex_class = true,
                    ']' => self.in_regex_class = false,
                    '/' if !self.in_regex_class => self.in_regex = false,
                    _ => {}
                }
                continue;
            }
            if self.in_block_comment {
                if c == '*' && chars.peek().map(|(_, n)| *n) == Some('/') {
                    chars.next();
                    self.in_block_comment = false;
                }
                continue;
            }
            // Rust 原始字符串开头：r 后跟 N 个 # 再跟 "（br#"/cr#" 的 b/c 作普通字符略过）。
            // 检测时把开头的 # 与 " 一并消费（否则 n=0 时开头 " 会被误判为闭合）。
            if raw_str && c == 'r' {
                let rest = &line[i + 1..];
                let n = rest.chars().take_while(|&h| h == '#').count() as u32;
                if rest.chars().nth(n as usize) == Some('"') {
                    for _ in 0..n {
                        chars.next(); // 消费开头的 #
                    }
                    chars.next(); // 消费开头的 "
                    self.in_raw = Some(n);
                    continue;
                }
                // r#ident（原始标识符，如 r#type）不由此处理，落入普通字符
            }
            // 三引号字符串开头：`"""` / `'''` 整体进入（先于单引号字符串判定，
            // 否则三连引号被拆成 开+闭+开，内容含单引号即泄漏为代码）
            if triple.contains(&(c as u8)) && line[i..].starts_with(&format!("{c}{c}{c}")) {
                chars.next();
                chars.next(); // 消费开头的其余两个引号
                self.in_triple = Some(c);
                continue;
            }
            // JS 系 `/` 消歧：前驱是 = ( , : [ ! & | ? ; { } 或行首 → 正则字面量
            // （除法两侧 prev 必是标识符/数字/`)`；`/[{]/` 等字符类内括号不参与配对）
            if regex_lang && c == '/' {
                let next_is_comment = chars.peek().map(|(_, n)| *n) == Some('/') || chars.peek().map(|(_, n)| *n) == Some('*');
                if !next_is_comment
                    && self.prev_char.is_none_or(|p| matches!(p, '=' | '(' | ',' | ':' | '[' | '!' | '&' | '|' | '?' | ';' | '{' | '}' | '\n' | '+' | '-' | '*' | '%' | '<' | '>' | '~' | '^'))
                {
                    self.in_regex = true;
                    self.in_regex_class = false;
                    self.prev_char = Some('/');
                    continue;
                }
            }
            let is_sig = !c.is_whitespace();
            match c {
                // 单引号歧义消解：Rust 里 ' 只用于字符字面量（'x'/'\\x'，必闭合），
                // `'ident` 后不跟 ' 的是生命周期（&'a str）→ 跳过；非 Rust 语言行内
                // 找不到后续 ' 的是撇号（JSX 文本 don't）→ 跳过。否则按字符串开引号。
                '\'' if ext == "rs" => {
                    let rest = &line[i + 1..];
                    let run: usize = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .map(|c| c.len_utf8())
                        .sum();
                    let after = rest[run..].chars().next();
                    if !(run > 0 && after != Some('\'')) {
                        self.in_str = Some(c);
                    }
                }
                '\'' => {
                    if line[i + 1..].contains('\'') {
                        self.in_str = Some(c);
                    }
                }
                '"' | '`' => self.in_str = Some(c),
                '/' if chars.peek().map(|(_, n)| *n) == Some('/') => break, // 行注释
                '/' if chars.peek().map(|(_, n)| *n) == Some('*') => {
                    chars.next();
                    self.in_block_comment = true;
                }
                '#' if hash_comment => break,
                '{' | '}' | '(' | ')' | '[' | ']' => cb(c),
                _ => {}
            }
            if is_sig {
                self.prev_char = Some(c);
            }
        }
        // 正则字面量不允许裸换行：行尾强制退出，防误判吞掉后续行
        self.in_regex = false;
        self.in_regex_class = false;
    }
}

/// 行缩进宽度（空格+制表）
fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

/// 判断一行是否为「块根」：方法/函数/类型等定义行（非控制流）。
/// 用于在嵌套块中选「要完整操作的那个方法」，避免只补到 if/for 内部。
pub(super) fn is_block_root(line: &str, ext: &str) -> bool {
    let t = line.trim();
    if outline_kind(t, ext).is_some() {
        return true;
    }
    // 兜底：语言无关的常见定义关键字（outline_kind 未覆盖的语言/写法）
    starts_with_word(t, "fn")
        || starts_with_word(t, "function")
        || starts_with_word(t, "def")
        || starts_with_word(t, "class")
        || starts_with_word(t, "struct")
        || starts_with_word(t, "interface")
        || starts_with_word(t, "enum")
        || starts_with_word(t, "trait")
        || starts_with_word(t, "impl")
        || starts_with_word(t, "type")
        || starts_with_word(t, "func")
        || starts_with_word(t, "pub fn")
        || starts_with_word(t, "export default")
        || looks_like_method(t)
        || looks_like_arrow(t)
}

/// 自上而下收集「包含 idx 行」的所有未配对开块行（外层在前、内层在后）。
/// 空列表 = idx 不在任何块内（文本/数据区域或块间）。
pub(super) fn enclosing_opens(lines: &[&str], idx: usize, ext: &str) -> Vec<usize> {
    let mut stack: Vec<usize> = Vec::new();
    let mut sc = LineScanner::default();
    for (i, line) in lines.iter().enumerate().take(idx + 1) {
        sc.scan(line, ext, |c| match c {
            '{' | '(' | '[' => stack.push(i),
            _ => {
                stack.pop();
            }
        });
    }
    stack
}

/// 从 open_idx 行向下扫描，找到与开块括号配对闭合的那一行（0 起，含）。
/// 显式栈配对：每次 `})]` 弹出最近未闭合的开括号；目标 = 开行最后一个未闭合开括号，
/// 被弹出即找到配对行——比扁平深度计数更稳：`} else {`、单行内嵌块不会误判，
/// 行首闭合符（如 `} else {` 的 `}`）也不会干扰。找不到返回 None。
pub(super) fn find_matching_close(lines: &[&str], open_idx: usize, ext: &str) -> Option<usize> {
    let mut stack: Vec<usize> = Vec::new();
    let mut target: Option<usize> = None;
    let mut sc = LineScanner::default();
    for (i, line) in lines.iter().enumerate().skip(open_idx) {
        let mut closed_target = false;
        sc.scan(line, ext, |c| match c {
            '{' | '(' | '[' => stack.push(i),
            '}' | ')' | ']' => {
                if let Some(popped) = stack.pop() {
                    if target == Some(popped) {
                        closed_target = true;
                    }
                }
            }
            _ => {}
        });
        if target.is_none() {
            // 开行处理完毕：目标 = 该行最后未闭合的开括号（若无 → 该行未真正开块）
            target = stack.last().copied();
            target?;
        } else if closed_target {
            return Some(i);
        }
    }
    None
}

/// 去掉 Python 行内注释（# 起，字符串内的 # 不算）——判定签名结束 `:` 时用
fn strip_py_comment(line: &str) -> &str {
    let mut in_str: Option<char> = None;
    let mut chars = line.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if let Some(q) = in_str {
            if c == '\\' {
                chars.next();
                continue;
            }
            if c == q {
                in_str = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => in_str = Some(c),
            '#' => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Python 缩进块：向上找最近的 def/class 定义行，签名（含跨行参数表）闭合后，
/// 向下到缩进归位前的最后一行。
fn find_indent_block(lines: &[&str], idx: usize) -> Option<(usize, usize)> {
    if lines.is_empty() {
        return None;
    }
    let mut open = idx.min(lines.len() - 1);
    loop {
        let t = lines[open].trim();
        if t.is_empty() || t.starts_with('#') {
            if open == 0 {
                return None;
            }
            open -= 1;
            continue;
        }
        if t.starts_with("def ") || t.starts_with("async def ") || t.starts_with("class ") {
            break;
        }
        if open == 0 {
            return None;
        }
        open -= 1;
    }
    let base_indent = indent_of(lines[open]);
    // 签名结束行：从定义行起做括号配对（字符串/注释感知），平衡归零且
    // （去行尾注释后）以 : 结尾 → 签名完整；兼容单行体 def foo(): return 1。
    // 多行签名（def foo(\\n  参数\\n):）必须先找到 `):` 行再量主体缩进，
    // 否则 `):` 行缩进归位会被误当块尾、函数体被截掉。
    let mut sig_end = open;
    {
        let mut depth: i32 = 0;
        let mut sc = LineScanner::default();
        let mut closed = false;
        for (i, line) in lines.iter().enumerate().skip(open) {
            sc.scan(line, "py", |c| match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                _ => {}
            });
            if depth <= 0 {
                let t = strip_py_comment(line).trim_end();
                if t.ends_with(':') {
                    sig_end = i;
                    closed = true;
                    break;
                }
                // 单行体：签名已闭合且同行即函数体（def foo(): return 1）
                if i == open && t.contains("):") {
                    return Some((open, open));
                }
            }
        }
        if !closed {
            return None;
        }
    }
    // 主体：sig_end 之后缩进 > base 的连续行；块尾 = 缩进归位前最后一个非空行
    let mut last_body = sig_end;
    let mut close = sig_end + 1;
    while close < lines.len() {
        let t = lines[close].trim();
        if t.is_empty() || t.starts_with('#') {
            close += 1;
            continue;
        }
        if indent_of(lines[close]) <= base_indent {
            break;
        }
        last_body = close;
        close += 1;
    }
    Some((open, last_body))
}

/// 找到 idx 所在的方法/类型级根块（最内层根块；Python 为 def/class 块）。
pub(super) fn find_root_block(lines: &[&str], idx: usize, ext: &str) -> Option<(usize, usize)> {
    if idx >= lines.len() {
        return None;
    }
    match block_style(ext) {
        BlockStyle::Indent => return find_indent_block(lines, idx),
        BlockStyle::None => return None,
        BlockStyle::Brace => {}
    }
    let opens = enclosing_opens(lines, idx, ext);
    let root = opens.iter().rev().find(|&&o| is_block_root(lines[o], ext)).copied()?;
    let close = find_matching_close(lines, root, ext)?;
    Some((root, close))
}

/// 单遍块索引：一次 O(n) 扫描预计算每行所属块，供多查询场景（grep block 模式
/// 每命中一次 find_enclosing_block 都要从头 O(n) 重扫，50 命中 = O(50n)）。
/// 语义与 find_enclosing_block 完全一致：最内层根块优先，无根块回退最外层块。
/// 内存紧凑编码：行号存 u32（文件上限 1MB → 行数远小于 u32::MAX），
/// u32::MAX 作 None 哨兵，每个数组元素 4B（Option<usize> 是 16B，省 4 倍）。
pub(super) struct BlockIndex {
    /// 每行处理完后的「栈内最近块根开行」（NONE = 无根块在栈中）
    line_root: Vec<u32>,
    /// 每行处理完后的「栈底（最外层未闭开行）」
    line_outer: Vec<u32>,
    /// 开行 → 闭行（NONE = 未闭合，查询返回 None 与 find_matching_close 一致）
    open_close: Vec<u32>,
}

/// u32 行号编码的 None 哨兵
const BI_NONE: u32 = u32::MAX;
/// 行数超过此值不再构建索引（回退逐次 find_enclosing_block，防 u32 溢出）
const BI_MAX_LINES: usize = u32::MAX as usize - 1;

impl BlockIndex {
    pub(super) fn build(lines: &[&str], ext: &str) -> Self {
        let n = lines.len();
        let mut idx = BlockIndex {
            line_root: vec![BI_NONE; n],
            line_outer: vec![BI_NONE; n],
            open_close: vec![BI_NONE; n],
        };
        if block_style(ext) != BlockStyle::Brace || n > BI_MAX_LINES {
            return idx; // Python（Indent）/None/超限：查询走回退路径
        }
        // 栈存唯一 id（同行的多个开括号各有独立 id），对齐 find_matching_close
        // 的 target 语义：open_close 只记「开行最后一个未闭合括号」的闭合行，
        // 同行配平的 ()（如 `fn foo() {`）不污染，未闭合到文件尾保持哨兵。
        let mut stack: Vec<usize> = Vec::new();
        let mut line_of: Vec<usize> = Vec::new(); // id → 开行
        let mut target_of: Vec<Option<usize>> = vec![None; n]; // 行 → 该行最后未闭括号 id
        let mut sc = LineScanner::default();
        for (i, line) in lines.iter().enumerate() {
            sc.scan(line, ext, |c| match c {
                '{' | '(' | '[' => {
                    line_of.push(i);
                    stack.push(line_of.len() - 1);
                }
                '}' | ')' | ']' => {
                    if let Some(popped) = stack.pop() {
                        let ln = line_of[popped];
                        if target_of[ln] == Some(popped) {
                            idx.open_close[ln] = i as u32;
                        }
                    }
                }
                _ => {}
            });
            idx.line_outer[i] = stack.first().map(|&id| line_of[id] as u32).unwrap_or(BI_NONE);
            idx.line_root[i] = stack
                .iter()
                .rev()
                .map(|&id| line_of[id])
                .find(|&o| is_block_root(lines[o], ext))
                .map(|o| o as u32)
                .unwrap_or(BI_NONE);
            target_of[i] = stack.iter().rev().find(|&&id| line_of[id] == i).copied();
        }
        idx
    }

    /// 紧凑编码取值：哨兵 → None
    fn get(arr: &[u32], i: usize) -> Option<usize> {
        match arr.get(i) {
            Some(&v) if v != BI_NONE => Some(v as usize),
            _ => None,
        }
    }

    /// 查询 idx 行所在的完整块（与 find_enclosing_block 等价）。
    /// Python 回退 find_indent_block（缩进制无栈概念，O(n) 可接受）。
    pub(super) fn enclosing(&self, lines: &[&str], idx: usize, ext: &str) -> Option<(usize, usize)> {
        if idx >= lines.len() {
            return None;
        }
        if block_style(ext) == BlockStyle::Indent {
            return find_indent_block(lines, idx);
        }
        let open = Self::get(&self.line_root, idx).or_else(|| Self::get(&self.line_outer, idx))?;
        let close = Self::get(&self.open_close, open)?;
        Some((open, close))
    }
}

/// 找到 idx 所在的完整块：最内层根块优先；无根块则最外层块（括号组）。
/// 保证返回的块是「完整的」——成对结束符一定包含在内。
pub(super) fn find_enclosing_block(lines: &[&str], idx: usize, ext: &str) -> Option<(usize, usize)> {
    if idx >= lines.len() {
        return None;
    }
    match block_style(ext) {
        BlockStyle::Indent => return find_indent_block(lines, idx),
        BlockStyle::None => return None,
        BlockStyle::Brace => {}
    }
    let opens = enclosing_opens(lines, idx, ext);
    if opens.is_empty() {
        return None;
    }
    let open = opens
        .iter()
        .rev()
        .find(|&&o| is_block_root(lines[o], ext))
        .copied()
        .unwrap_or(opens[0]);
    let close = find_matching_close(lines, open, ext)?;
    Some((open, close))
}

/// 以 anchor 为锚点的块查找：在 idx 的 enclosing 候选里，选「开行 ≤ anchor」的最内层块
/// （即与窗口起点同一层级的那块），保证窗口终点不跳进内层块而漏掉外层方法尾部。
pub(super) fn find_block_anchored(
    lines: &[&str],
    idx: usize,
    anchor: usize,
    ext: &str,
) -> Option<(usize, usize)> {
    if idx >= lines.len() {
        return None;
    }
    match block_style(ext) {
        BlockStyle::Indent => return find_indent_block(lines, idx),
        BlockStyle::None => return None,
        BlockStyle::Brace => {}
    }
    let opens = enclosing_opens(lines, idx, ext);
    if opens.is_empty() {
        return None;
    }
    let chosen = opens
        .iter()
        .rev()
        .find(|&&o| o <= anchor)
        .copied()
        .unwrap_or(opens[0]);
    let close = find_matching_close(lines, chosen, ext)?;
    Some((chosen, close))
}


/// find_files：按文件名 glob 搜索（最多 100 条）
pub(super) async fn find_files(args: &Value, roots: &[String]) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法搜索文件".into());
    }
    let pattern = args["pattern"].as_str().unwrap_or("").trim();
    if pattern.is_empty() {
        return Err("find_files 需要参数 {\"pattern\":\"<glob 模式，如 *.ets 或 **/*.json>\"}".into());
    }
    let raw = args["path"].as_str().unwrap_or(".");
    let root = resolve_in_roots(roots, raw)?;
    if !root.is_dir() {
        return Err(format!("路径不是目录: {}", root.display()));
    }
    let state = args["state"].as_str().map(str::trim).filter(|value| !value.is_empty());
    if state.is_some_and(|value| {
        !matches!(
            value,
            "indexed" | "deferred" | "oversized" | "unsupported" | "symlink" | "unreadable"
        )
    }) {
        return Err("state 仅支持 indexed|deferred|oversized|unsupported|symlink|unreadable".into());
    }
    let page = args["page"].as_u64().unwrap_or(1) as usize;
    let limit = args["limit"].as_u64().unwrap_or(100) as usize;
    let project_root = PathBuf::from(&roots[0]);
    let prefix = root
        .strip_prefix(&project_root)
        .ok()
        .map(|value| value.to_string_lossy().replace('\\', "/"));
    if let Some(prefix) = prefix {
        let project_root_for_query = project_root.clone();
        let pattern_for_query = pattern.to_string();
        let state_for_query = state.map(str::to_string);
        let catalog = tokio::task::spawn_blocking(move || {
            crate::services::symbol_index::query_catalog_files(
                &project_root_for_query,
                &pattern_for_query,
                Some(&prefix),
                state_for_query.as_deref(),
                page,
                limit,
            )
        })
        .await
        .map_err(|error| error.to_string())?;
        if let Some(result) = catalog {
            let result = result?;
            let mut out = format!(
                "全库目录匹配 {} 个文件，第 {} 页（每页 {}）：\n",
                result.total_matches, result.page, result.page_size
            );
            for file in result.items {
                out.push_str(&format!(
                    "{}  [{}; {} bytes; shard={}]\n",
                    project_root.join(&file.path).display(),
                    file.state,
                    file.size,
                    file.shard,
                ));
            }
            if let Some(next_page) = result.next_page {
                out.push_str(&format!("还有结果：使用 page={next_page} 读取下一页。\n"));
            }
            return Ok(truncate_out(&out));
        }
    }
    // 全树递归遍历为 IO 密集操作，放 spawn_blocking 避免钉死 tokio worker
    let root_buf = root.clone();
    let roots_owned: Vec<String> = roots.to_vec();
    let pattern_owned = pattern.to_string();
    let (hits, skipped) = tokio::task::spawn_blocking(move || {
        let (ignore_rules, start_rel) = load_project_ignore(&root_buf, &roots_owned);
        let mut hits: Vec<PathBuf> = Vec::new();
        let mut skipped = 0u32;
        fn walk(
            dir: &Path,
            root: &Path,
            rel: &str,
            rules: &[IgnoreRule],
            pattern: &str,
            hits: &mut Vec<PathBuf>,
            skipped: &mut u32,
        ) {
            // 子目录 .gitignore（与 list_dir 同口径）
            let child_rules = load_child_rules(rules, dir, rel);
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for e in entries.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if p.is_dir() {
                    let entry_rel = if rel.is_empty() {
                        name.clone()
                    } else {
                        format!("{rel}/{name}")
                    };
                    if should_skip_dir(&name)
                        || gitignore_ignored(&child_rules, &name, &entry_rel, true)
                    {
                        *skipped += 1;
                        continue;
                    }
                    walk(&p, root, &entry_rel, &child_rules, pattern, hits, skipped);
                    if hits.len() >= 100 {
                        return;
                    }
                } else {
                    // pattern 可匹配文件名（*.ets）或相对路径（src/**/*.ets）
                    let rel = p
                        .strip_prefix(root)
                        .map(|r| r.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_else(|_| name.clone());
                    if glob_match(pattern, &name) || glob_match(pattern, &rel) {
                        hits.push(p);
                        if hits.len() >= 100 {
                            return;
                        }
                    }
                }
            }
        }
        walk(&root_buf, &root_buf, &start_rel, &ignore_rules, &pattern_owned, &mut hits, &mut skipped);
        // 排序：按路径字典序，同级目录聚拢、结果可预测（read_dir 顺序是随机的）
        hits.sort();
        (hits, skipped)
    })
    .await
    .unwrap_or_default();
    if hits.is_empty() {
        return Ok(format!(
            "未找到匹配 {pattern} 的文件（已跳过 {skipped} 个忽略目录）"
        ));
    }
    let mut out = format!("找到 {} 个文件（已跳过 {skipped} 个忽略目录）：\n", hits.len());
    for p in &hits {
        out.push_str(&format!("{}\n", p.display()));
    }
    Ok(truncate_out(&out))
}

/// grep_files：按文本内容搜索（缺省不区分大小写，可指定 case_sensitive；跳过忽略目录、
/// 二进制与超大文件，最多 50 条）
pub(super) async fn grep_files(args: &Value, roots: &[String]) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法搜索内容".into());
    }
    let pattern = args["pattern"].as_str().unwrap_or("").trim();
    if pattern.is_empty() {
        return Err("grep_files 需要参数 {\"pattern\":\"<搜索关键词>\"}".into());
    }
    let raw = args["path"].as_str().unwrap_or(".");
    let root = resolve_in_roots(roots, raw)?;
    let glob = args["glob"].as_str().unwrap_or("").trim().to_string();
    let case_sensitive = args["case_sensitive"].as_bool().unwrap_or(false);
    // block=true：命中时给出所在「完整代码块」（方法/函数整体，语言感知成对匹配），
    // 便于直接了解/编辑整个方法而不只看单行；最多展开前 5 条防输出爆炸
    let block_mode = args["block"].as_bool().unwrap_or(false);
    const MAX_BLOCK_HITS: usize = 5;
    let pattern = pattern.to_string();
    let lower = pattern.to_lowercase();
    // regex=true：pattern 按正则解释（如 foo\s*\(、Vec<\w+>），
    // 大小写敏感性由 case_sensitive 统一控制；非法正则提前报错
    let re = if args["regex"].as_bool().unwrap_or(false) {
        Some(
            regex::RegexBuilder::new(&pattern)
                .case_insensitive(!case_sensitive)
                .build()
                .map_err(|e| format!("正则表达式无效（{pattern:?}）：{e}。常见问题：反斜杠在 JSON 参数中需双重转义（\\d 写作 \\\\d）；或去掉 regex 参数用纯文本搜索"))?,
        )
    } else {
        None
    };
    // 全树递归遍历 + 逐文件正则匹配为 CPU/IO 密集操作（大项目可达数百 ms~秒级），
    // 整体放 spawn_blocking，避免钉死 tokio worker（timer driver 停转 → 流式超时全部失效）。
    let root_buf = root.clone();
    let pattern_buf = pattern.clone();
    let roots_owned: Vec<String> = roots.to_vec();
    let (hits, files_checked, skipped, block_shown) = tokio::task::spawn_blocking(move || {
        let (ignore_rules, start_rel) = load_project_ignore(&root_buf, &roots_owned);
        let mut hits: Vec<String> = Vec::new();
        let mut files_checked = 0u32;
        let mut skipped = 0u32;
        let mut block_shown = 0usize;
        // 单文件搜索大小上限：超大文本文件跳过（防读入内存爆炸）
        const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
        fn walk(
            dir: &Path,
            rel: &str,
            rules: &[IgnoreRule],
            pattern: &str,
            lower: &str,
            case_sensitive: bool,
            re: Option<&regex::Regex>,
            glob: &str,
            block_mode: bool,
            block_shown: &mut usize,
            hits: &mut Vec<String>,
            files_checked: &mut u32,
            skipped: &mut u32,
        ) {
            // 子目录 .gitignore（与 list_dir 同口径）：规则只对其子树生效
            let child_rules = load_child_rules(rules, dir, rel);
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for e in entries.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if p.is_dir() {
                    let entry_rel = if rel.is_empty() {
                        name.clone()
                    } else {
                        format!("{rel}/{name}")
                    };
                    if should_skip_dir(&name)
                        || gitignore_ignored(&child_rules, &name, &entry_rel, true)
                    {
                        *skipped += 1;
                        continue;
                    }
                    walk(
                        &p,
                        &entry_rel,
                        &child_rules,
                        pattern,
                        lower,
                        case_sensitive,
                        re,
                        glob,
                        block_mode,
                        block_shown,
                        hits,
                        files_checked,
                        skipped,
                    );
                    if hits.len() >= 50 {
                        return;
                    }
                } else {
                    if !glob.is_empty() && !glob_match(glob, &name) {
                        continue;
                    }
                    let Ok(meta) = std::fs::metadata(&p) else { continue };
                    if meta.len() > MAX_FILE_BYTES {
                        continue;
                    }
                    let Ok(bytes) = std::fs::read(&p) else { continue };
                    if bytes[..bytes.len().min(4096)].contains(&0) {
                        continue;
                    }
                    *files_checked += 1;
                    let text = smart_decode(&bytes);
                    let flines: Vec<&str> = text.lines().collect();
                    let ext = p
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    // 同一文件内已展开过的块区间（同一方法多条命中不重复整块展开）
                    let mut seen_blocks: Vec<(usize, usize)> = Vec::new();
                    // 单遍块索引：50 命中场景从 O(50n) 降为 O(n)，语义与逐次 find_enclosing_block 等价
                    let bindex_built = if block_mode { Some(BlockIndex::build(&flines, &ext)) } else { None };
                    let bindex = bindex_built.as_ref();
                    for (i, line) in flines.iter().enumerate() {
                        let matched = match re {
                            Some(r) => r.is_match(line),
                            None => {
                                if case_sensitive {
                                    line.contains(pattern)
                                } else {
                                    line.to_lowercase().contains(lower)
                                }
                            }
                        };
                        if matched {
                            // block 模式：展开所在完整代码块（成对 {}() 语言感知，整方法不截断）
                            if block_mode && *block_shown < MAX_BLOCK_HITS {
                                let blk = match bindex {
                                    Some(bi) => bi.enclosing(&flines, i, &ext),
                                    None => find_enclosing_block(&flines, i, &ext),
                                };
                                if let Some((o, c)) = blk {
                                    if seen_blocks.contains(&(o, c)) {
                                        continue;
                                    }
                                    seen_blocks.push((o, c));
                                    *block_shown += 1;
                                    let mut b = String::new();
                                    b.push_str(&format!("{}（完整代码块 L{}-L{}）\n", p.display(), o + 1, c + 1));
                                    for (li, l) in flines[o..=c].iter().enumerate() {
                                        let s: String = l.trim_end().chars().take(200).collect();
                                        b.push_str(&format!("{:>5} │ {s}\n", o + li + 1));
                                    }
                                    b.push_str(&format!("…块结束 L{}\n", c + 1));
                                    hits.push(b);
                                    if hits.len() >= 50 {
                                        return;
                                    }
                                    continue;
                                }
                            }
                            let snippet: String = line.trim().chars().take(160).collect();
                            // 注释命中标注：模型据此区分代码逻辑位置与说明性位置，避免误读
                            let tag = if is_comment_line(line, &ext) { "[注释] " } else { "" };
                            hits.push(format!("{}:{}: {tag}{snippet}", p.display(), i + 1));
                            if hits.len() >= 50 {
                                return;
                            }
                        }
                    }
                }
            }
        }
        walk(
            &root_buf,
            &start_rel,
            &ignore_rules,
            &pattern_buf,
            &lower,
            case_sensitive,
            re.as_ref(),
            &glob,
            block_mode,
            &mut block_shown,
            &mut hits,
            &mut files_checked,
            &mut skipped,
        );
        (hits, files_checked, skipped, block_shown)
    })
    .await
    .unwrap_or_default();
    if hits.is_empty() {
        return Ok(format!(
            "未找到包含「{pattern}」的内容（检查了 {files_checked} 个文件，跳过 {skipped} 个忽略目录）"
        ));
    }
    let mut out = format!(
        "找到 {} 条命中（检查 {files_checked} 个文件，跳过 {skipped} 个忽略目录）：\n",
        hits.len()
    );
    if block_mode {
        out.push_str(&format!(
            "（block=true：前 {} 条命中已展开所在完整代码块，其余仅显示单行）\n",
            block_shown.min(MAX_BLOCK_HITS)
        ));
    }
    for h in &hits {
        out.push_str(&format!("{h}\n"));
    }
    // block 模式需要容纳整块代码，输出上限放大；普通模式沿用 3000 字符
    Ok(truncate_out_max(&out, if block_mode { 20000 } else { 3000 }))
}

// ---------- 写文件 / 编辑文件 ----------

/// 检测"连续重复目录段"（如 entry/entry/、src/src/）：模块目录嵌套的典型错误，
/// 根因是工程根被误判为模块目录时路径多拼了一层模块名。
/// 命中时返回错误并给出鸿蒙标准结构指引，避免把测试/源码写进 <模块>/<模块>/src 后难以清理。
fn check_nested_module_path(p: &Path, roots: &[String]) -> Result<(), String> {
    // Windows canonicalize 会带 \\?\ verbatim 前缀，strip_prefix 前统一去前缀
    let p_clean = PathBuf::from(crate::utils::path::normalize_path(&p.to_string_lossy()));
    for r in roots {
        // macOS 的 /var 与 /private/var、Windows 的 verbatim 前缀都可能让“同一路径”
        // 出现两种文本形式。原始根与规范根都检查，防止尚不存在的待写路径绕过检测。
        let raw_root = PathBuf::from(r);
        let canonical_root = std::fs::canonicalize(&raw_root).unwrap_or_else(|_| raw_root.clone());
        for root in [raw_root, canonical_root] {
            let rc = PathBuf::from(crate::utils::path::normalize_path(&root.to_string_lossy()));
            let Ok(rel) = p_clean.strip_prefix(&rc) else {
                continue;
            };
            let segs: Vec<String> = rel
                .components()
                .filter_map(|c| match c {
                    std::path::Component::Normal(s) => Some(s.to_string_lossy().to_lowercase()),
                    _ => None,
                })
                .collect();
            if let Some(i) = (1..segs.len()).find(|&i| segs[i] == segs[i - 1]) {
                return Err(format!(
                    "路径疑似模块目录嵌套：{}（目录段 \"{}\" 连续重复）。\n鸿蒙标准结构：模块根 = <工程根>/<模块名>，配置与源码直接位于模块根下，测试代码在 <模块根>/src/test 与 <模块根>/src/ohosTest。若目标模块是 entry，正确路径形如 <工程根>/entry/src/...，而不是 entry/entry/src/...。\n请核对路径后重试。",
                    p.display(),
                    segs[i]
                ));
            }
        }
    }
    Ok(())
}

/// 写入/覆盖文本文件（UTF-8，单次 ≤1MB，自动创建父目录）
pub(super) async fn write_file(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法写入文件".into());
    }
    // Request/Spec 分离：宽松参数 WriteFileRequest → 显式 resolve() 产出严格规范 WriteFileSpec
    let spec = WriteFileRequest::from_args(args)?.resolve(roots)?;
    check_nested_module_path(&spec.path, roots)?;
    let p = &spec.path;
    let content = spec.content.as_str();
    let existed = p.exists();
    let mut content_out = content.to_string();
    if existed {
        // 冲突保护：文件自上次读取后被外部修改（IDE/用户/其他会话）→ 拒绝覆盖，要求重读确认。
        // 整文件读取在 spawn_blocking 中执行，避免大文件读钉死 tokio worker。
        let p_buf = p.clone();
        let old_bytes = tokio::task::spawn_blocking(move || std::fs::read(&p_buf))
            .await
            .ok()
            .and_then(|r| r.ok());
        if let Some(bytes) = old_bytes {
            if has_external_change(p, &bytes) {
                return Err(format!(
                    "写入冲突：文件 {} 自上次读取后被修改（可能被外部编辑器/IDE、其他会话或命令间接改动）。\n请先 read_file 查看最新内容、确认意图后再写入（重新读取会解除冲突保护）。",
                    p.display()
                ));
            }
            // 换行风格保持：原文件 CRLF（Windows 项目常见）且新内容纯 LF → 统一转 CRLF，
            // 避免覆盖后整个文件 diff 全部变化
            if bytes.windows(2).any(|w| w == b"\r\n") && !content.contains('\r') {
                content_out = content.replace('\n', "\r\n");
            }
            // BOM 保留：原文件带 UTF-8 BOM（EF BB BF）时新内容也前置 BOM（与 edit_file 同口径），
            // 避免覆盖后 .bat/带 BOM 文本文件的首行字节变化导致乱码
            if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) && !content_out.starts_with('\u{feff}') {
                content_out.insert(0, '\u{feff}');
            }
            // 撤销快照：覆盖前记录旧内容（必须记旧字节，记新内容会导致 undo 失效）
            crate::agent::undo::snapshot(conversation_id, p, &bytes);
        }
    }
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录失败 {}: {e}", parent.display()))?;
    }
    // 配平守卫（与 edit_file 同口径）：代码文件写入后必须配平——
    // 新文件不存在（旧内容为空串，视为配平基准），内容缺结束符（漏 } 等）→ 拒绝落盘
    {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let old_text = std::fs::read_to_string(p).unwrap_or_default(); // 不存在/读取失败 → 空串
        balance_guard(&old_text, &content_out, &ext)?;
    }
    // [58] dry-run：预览将写入的内容，不落盘、不写 undo
    if args["dry_run"].as_bool().unwrap_or(false) {
        let preview: String = {
            let n = content_out.chars().count();
            if n > 1200 {
                format!("{}…（共 {n} 字符）", content_out.chars().take(1200).collect::<String>())
            } else {
                content_out.clone()
            }
        };
        return Ok(format!(
            "【dry_run 预览】将{}文件 {}（{} 字节，未落盘）\n{}",
            if existed { "覆盖" } else { "创建" },
            p.display(),
            content_out.len(),
            preview
        ));
    }
    // 落盘 + 指纹登记：文件写入为 IO 操作，放 spawn_blocking 避免钉死 tokio worker
    let p_buf = p.clone();
    let content_buf = content_out.clone();
    let (wmeta, _) = tokio::task::spawn_blocking(move || {
        std::fs::write(&p_buf, content_buf.as_bytes()).map_err(|e| format!("写入文件失败: {e}"))?;
        let meta = std::fs::metadata(&p_buf).ok();
        Ok::<(Option<std::fs::Metadata>, ()), String>((meta, ()))
    })
    .await
    .map_err(|e| format!("写入文件任务异常: {e}"))??;
    if let Some(meta) = wmeta {
        stamp_put(p, &meta, content_out.as_bytes());
    }
    Ok(format!(
        "已{}文件 {}（{} 字节）",
        if existed { "覆盖" } else { "创建" },
        p.display(),
        content.len()
    ))
}

/// 全文括号配平检查（字符串/注释/正则感知）：净栈为零且无悬空字符串态。
/// 用于编辑落盘前的完整性守卫——「原本配平的文件被改失衡」说明替换内容
/// 残缺（如漏块结束符），拒绝写回。病态文件（原本就失衡）不由此判断。
fn is_balanced(text: &str, ext: &str) -> bool {
    let mut depth: i64 = 0;
    let mut sc = LineScanner::default();
    for line in text.lines() {
        sc.scan(line, ext, |c| {
            depth += match c {
                '{' | '(' | '[' => 1,
                _ => -1,
            };
        });
    }
    depth == 0 && sc.in_str.is_none() && sc.in_raw.is_none() && sc.in_triple.is_none() && !sc.in_block_comment
}

/// start 模式块定位（edit_file / preview_edit 共用口径）：
/// 超界显式报错（绝不静默钳到末行改错块）→ 语言感知定位完整块（成对 {}()，
/// Python 按缩进）→ anchor 块锚签名校验（行号漂移时 ±100 行内按签名重定位，
/// 找不到则拒绝）。返回 (块首行, 块尾行)（0 起，含两端）。
fn locate_edit_block(
    body_lines: &[&str],
    start_line: u64,
    anchor: Option<&str>,
    ext: &str,
) -> Result<(usize, usize), String> {
    let total = body_lines.len();
    if total == 0 {
        return Err("文件为空，没有可替换的代码块".into());
    }
    // 超界显式报错：绝不静默钳制到末行（会改到无关块）
    if (start_line as usize) > total {
        return Err(format!(
            "start={start_line} 超出文件总行数（共 {total} 行）。行号可能已因先前编辑漂移：请重新 read_file/outline 确认，或提供 anchor 参数（块定义行内容片段）自动重定位"
        ));
    }
    let idx = (start_line as usize - 1).min(total - 1);
    let (mut o, mut c) = find_enclosing_block(body_lines, idx, ext).ok_or_else(|| {
        format!(
            "无法识别第 {start_line} 行所在的代码块：未找到成对的 {{}}/()（该行可能在字符串/注释中，或文件不是结构化代码；请改用 old 参数精确替换）"
        )
    })?;
    // 块锚签名：定位到的块与预期不符（行号漂移）→ ±100 行内按签名重定位
    if let Some(anchor) = anchor {
        if !body_lines[o].contains(anchor) {
            let from = idx.saturating_sub(100);
            let to = (idx + 100).min(total - 1);
            let relocated = (from..=to)
                .find(|&i| is_block_root(body_lines[i], ext) && body_lines[i].contains(anchor))
                .and_then(|i| find_enclosing_block(body_lines, i, ext).filter(|&(no, _)| no == i));
            match relocated {
                Some((no, nc)) => {
                    o = no;
                    c = nc;
                }
                None => {
                    return Err(format!(
                        "anchor 定位失败：第 {start_line} 行所在块首行不含 {:?}（实际：{:?}），且 ±100 行内未找到含该签名的块根行。\n可能行号漂移过大或签名不符，请重新 read_file/outline 确认",
                        anchor,
                        body_lines[o].trim()
                    ));
                }
            }
        }
    }
    Ok((o, c))
}

/// 批量块编辑计划（edit_file / preview_edit 共用）：
/// 所有块先在「原文」上定位（locate_edit_block 同口径：超界报错 + anchor 重定位），
/// 校验互不重叠后按行序一次性拼接——各块行号基于同一份原文，不存在先后漂移问题。
/// 返回 (最终正文, 各块区间按 starts 原顺序的 (开行, 闭行) 列表)（0 起，含两端）。
fn plan_batch_blocks(
    body: &str,
    starts: &[u64],
    news: &[String],
    anchors: &[Option<String>],
    ext: &str,
) -> Result<(String, Vec<(usize, usize)>), String> {
    let body_lines: Vec<&str> = body.split('\n').collect();
    // 1. 逐块定位（原文坐标系）
    let mut located: Vec<((usize, usize), usize)> = Vec::with_capacity(starts.len()); // (区间, starts 下标)
    for (k, &s) in starts.iter().enumerate() {
        let a = anchors.get(k).and_then(|x| x.as_deref());
        let (o, c) = locate_edit_block(&body_lines, s, a, ext)?;
        located.push(((o, c), k));
    }
    // 2. 重叠校验：同一块重复或区间相交 → 拒绝（拼接语义不明确）
    let mut sorted = located.clone();
    sorted.sort_by_key(|((o, _), _)| *o);
    for w in sorted.windows(2) {
        let ((o1, c1), k1) = w[0];
        let ((o2, _), k2) = w[1];
        if o2 <= c1 {
            return Err(format!(
                "批量编辑的块重叠：starts[{}] 定位到 L{}-L{}，starts[{}] 定位到 L{} 起，两者相交（同一块只需出现一次）",
                k1,
                o1 + 1,
                c1 + 1,
                k2,
                o2 + 1
            ));
        }
    }
    // 3. 行序拼接（CRLF 保留：按原文字节边界切）
    let mut line_starts: Vec<usize> = Vec::with_capacity(body_lines.len());
    let mut off = 0usize;
    for l in body.split_inclusive('\n') {
        line_starts.push(off);
        off += l.len();
    }
    let mut out = String::with_capacity(body.len());
    let mut cursor = 0usize;
    for ((o, c), k) in &sorted {
        let s_off = line_starts[*o];
        out.push_str(&body[cursor..s_off]);
        out.push_str(&news[*k]);
        cursor = if c + 1 < line_starts.len() { line_starts[c + 1] } else { body.len() };
    }
    out.push_str(&body[cursor..]);
    // 返回区间按 starts 原顺序（报告与参数一一对应）
    let ranges = located.iter().map(|(r, _)| *r).collect();
    Ok((out, ranges))
}

/// 编辑落盘守卫：原文件配平而新内容失衡 → 拒绝（返回 Err 带定位提示）。
/// 原文件本就失衡（病态/片段文件）时放行（允许修复），仅代码类扩展名适用。
fn balance_guard(old_text: &str, new_text: &str, ext: &str) -> Result<(), String> {
    if !is_code_ext(ext) {
        return Ok(());
    }
    if !is_balanced(old_text, ext) {
        return Ok(()); // 原本就不配平：不拦修复
    }
    if is_balanced(new_text, ext) {
        return Ok(());
    }
    // 定位失衡位置：逐行累计深度，首个越界负值或末尾未归零点
    let mut depth: i64 = 0;
    let mut sc = LineScanner::default();
    let mut first_bad: Option<usize> = None;
    for (i, line) in new_text.lines().enumerate() {
        sc.scan(line, ext, |c| {
            depth += match c {
                '{' | '(' | '[' => 1,
                _ => -1,
            };
        });
        if depth < 0 && first_bad.is_none() {
            first_bad = Some(i + 1);
        }
    }
    let dangling = sc.in_str.is_some() || sc.in_raw.is_some() || sc.in_triple.is_some() || sc.in_block_comment;
    Err(format!(
        "替换被配平守卫拒绝：原文件括号配平，替换后失衡（净差 {depth:+}{}{}）。\n新内容疑似残缺（漏块结束符/引号未闭合），请补全后重试；如确要写入病态片段，请先确认原文。",
        first_bad.map(|l| format!("，第 {l} 行首次越界")).unwrap_or_default(),
        if dangling { "，存在未闭合字符串/注释" } else { "" },
    ))
}

/// 编辑文件：精确文本替换（old → new，可选全部替换），返回替换处数
pub(super) fn apply_edit(text: &str, old: &str, new: &str, replace_all: bool) -> Result<(String, usize), String> {
    // CRLF/LF 兼容匹配：Windows 项目文件常见 CRLF，而 read_file 展示时行尾 \r 被剥离，
    // 模型拼的 old 用 \n（或单行无换行），直接匹配 CRLF 文件必然失败。
    // 策略：原样匹配失败时，按文件实际换行风格归一 old/new 重试；写回保持文件原风格。
    let mut eff_old = old.to_string();
    let mut eff_new = new.to_string();
    let mut occurrences = text.matches(&eff_old).count();
    if occurrences == 0 {
        // 文件 CRLF、old 用 LF：把 old/new 的 \n 换成 \r\n 重试
        if text.contains("\r\n") && !old.contains('\r') && old.contains('\n') {
            let c_old = old.replace('\n', "\r\n");
            let n = text.matches(&c_old).count();
            if n > 0 {
                eff_old = c_old;
                eff_new = new.replace('\n', "\r\n");
                occurrences = n;
            }
        }
        // 反向：文件 LF、old 带 CRLF（罕见，模型直接拼了原文含 \r 的情况）
        if occurrences == 0 && !text.contains('\r') && old.contains("\r\n") {
            let l_old = old.replace("\r\n", "\n");
            let n = text.matches(&l_old).count();
            if n > 0 {
                eff_old = l_old;
                eff_new = new.replace("\r\n", "\n");
                occurrences = n;
            }
        }
    }
    if occurrences == 0 {
        return Err(format!("old 内容在文件中未找到（{old:?}），请先 read_file 确认原文（注意缩进/引号/空白；若含反斜杠或正则如 \\n，确认 JSON 参数中已双重转义为 \\\\n）"));
    }
    let count = if replace_all { occurrences } else { 1 };
    let replaced = if replace_all {
        text.replace(&eff_old, &eff_new)
    } else {
        text.replacen(&eff_old, &eff_new, 1)
    };
    Ok((replaced, count))
}

/// edit_file：精确文本替换修改文件（≤1MB）
pub(super) async fn edit_file(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法编辑文件".into());
    }
    // Request/Spec 分离：宽松参数 EditFileRequest → 显式 resolve() 产出严格规范 EditFileSpec
    let spec = EditFileRequest::from_args(args)?.resolve(roots)?;
    check_nested_module_path(&spec.path, roots)?;
    let p = &spec.path;
    let old = spec.old.as_str();
    let new = spec.new.as_str();
    let replace_all = spec.replace_all;
    // 整文件读取为 IO 操作，放 spawn_blocking 避免钉死 tokio worker
    let p_buf = p.clone();
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(&p_buf))
        .await
        .map_err(|e| format!("读取文件任务异常: {e}"))?
        .map_err(|e| format!("读取文件失败: {e}"))?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err("文件是二进制，无法以文本方式编辑".into());
    }
    // 冲突保护：文件自上次读取后被外部修改 → 提前拦截（比 old 匹配失败的报错更明确）
    if has_external_change(p, &bytes) {
        return Err(format!(
            "编辑冲突：文件 {} 自上次读取后被修改（可能被外部编辑器/IDE、其他会话或命令间接改动）。\n请先 read_file 查看最新内容后重新编辑（重新读取会解除冲突保护）。",
            p.display()
        ));
    }
    // 严格 UTF-8 校验：GBK 文件若用 from_utf8_lossy 读入再写回，中文会被替换字符永久破坏
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| {
            format!(
                "文件 {} 非 UTF-8 编码（可能是 GBK/GB2312），为避免中文被写坏已拒绝编辑。请先用 iconv 等工具转换为 UTF-8 后再编辑",
                p.display()
            )
        })?
        .to_string();
    // BOM 处理：剥离后匹配（模型看不到 BOM 字符，不会在 old 里携带），写回时保留，
    // 避免「首行内容永远匹配不上」
    let (has_bom, body) = match text.strip_prefix('\u{feff}') {
        Some(b) => (true, b),
        None => (false, text.as_str()),
    };

    // ---- starts 批量模式：多个完整块一次定位、统一拼接（重构场景减少往返） ----
    // 所有块都在同一份原文上定位（行号互不漂移），互不重叠校验后按行序拼接；
    // 与单块模式同口径：超界报错、anchor 重定位、配平守卫、undo 快照、dry_run。
    if let Some(starts) = spec.starts.as_deref() {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let (final_body, ranges) = plan_batch_blocks(body, starts, &spec.news, &spec.anchors, &ext)?;
        balance_guard(body, &final_body, &ext)?;
        let final_text = if has_bom { format!("\u{feff}{final_body}") } else { final_body.clone() };
        if args["dry_run"].as_bool().unwrap_or(false) {
            let old_lines: Vec<&str> = body.split('\n').collect();
            let new_lines: Vec<&str> = final_body.split('\n').collect();
            let (diff, add_l, del_l) = build_unified_diff(&old_lines, &new_lines, &p.display().to_string(), 0, 0);
            let mut sum = String::new();
            for (k, (o, c)) in ranges.iter().enumerate() {
                sum.push_str(&format!(
                    "  starts[{k}]：L{}-L{}（{} 行）→ {}\n",
                    o + 1,
                    c + 1,
                    c - o + 1,
                    if spec.news[k].is_empty() { "删除" } else { "替换" }
                ));
            }
            return Ok(format!(
                "【dry_run 预览】将批量编辑 {} 个块，预计 +{add_l} −{del_l} 行（未落盘）\n{sum}{diff}",
                ranges.len()
            ));
        }
        crate::agent::undo::snapshot(conversation_id, p, &bytes);
        // 落盘为 IO 操作，放 spawn_blocking 避免钉死 tokio worker
        let p_buf = p.clone();
        let final_buf = final_text.clone();
        let (wmeta, _) = tokio::task::spawn_blocking(move || {
            std::fs::write(&p_buf, final_buf.as_bytes()).map_err(|e| format!("写入文件失败: {e}"))?;
            Ok::<(Option<std::fs::Metadata>, ()), String>((std::fs::metadata(&p_buf).ok(), ()))
        })
        .await
        .map_err(|e| format!("写入文件任务异常: {e}"))??;
        if let Some(meta) = wmeta {
            stamp_put(p, &meta, final_text.as_bytes());
        }
        let mut report = String::new();
        for (k, (o, c)) in ranges.iter().enumerate() {
            report.push_str(&format!(
                "  starts[{k}]：L{}-L{}（{} 行）→ {}（新内容 {} 行）\n",
                o + 1,
                c + 1,
                c - o + 1,
                if spec.news[k].is_empty() { "已删除" } else { "已替换" },
                spec.news[k].split('\n').count()
            ));
        }
        return Ok(format!("已批量编辑 {} 个块：\n{report}文件：{}", ranges.len(), p.display()));
    }

    // ---- start 模式：按语言感知的「完整代码块」整体替换/删除 ----
    // 不固定行数：块有多长就操作多长（{}() 成对匹配，Python 按缩进），
    // 从块首行到结束符一整套替换/删除，杜绝漏掉块结束符。
    if let Some(start_line) = spec.start {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let body_lines: Vec<&str> = body.split('\n').collect();
        let (o, c) = locate_edit_block(&body_lines, start_line, spec.anchor.as_deref(), &ext)?;
        // 按字节边界切出完整块（split('\n') 保留 CRLF 的 \r，替换后保持原换行风格）
        let mut line_starts: Vec<usize> = Vec::with_capacity(body_lines.len());
        let mut off = 0usize;
        for l in body.split_inclusive('\n') {
            line_starts.push(off);
            off += l.len();
        }
        let start_off = line_starts[o];
        let end_off = if c + 1 < line_starts.len() { line_starts[c + 1] } else { body.len() };
        let block_lines = c - o + 1;
        let final_body = format!("{}{}{}", &body[..start_off], spec.new, &body[end_off..]);
        // 先构造 final_text 时不再 move final_body（dry_run 分支仍需借用）
        let final_text = if has_bom { format!("\u{feff}{final_body}") } else { final_body.clone() };
        // [58] dry-run：内存 diff 预览，不落盘、不写 undo
        if args["dry_run"].as_bool().unwrap_or(false) {
            let old_lines: Vec<&str> = body.split('\n').collect();
            let new_lines: Vec<&str> = final_body.split('\n').collect();
            let (diff, add_l, del_l) = build_unified_diff(&old_lines, &new_lines, &p.display().to_string(), 0, 0);
            return Ok(format!(
                "【dry_run 预览】将{}完整代码块（第 {}–{} 行，共 {} 行）预计 +{add_l} −{del_l} 行（未落盘）\n{diff}",
                if spec.new.is_empty() { "删除" } else { "替换" },
                o + 1,
                c + 1,
                block_lines
            ));
        }
        // 配平守卫：原文件配平而替换后失衡 → 拒绝落盘（新内容残缺，如漏结束符）
        balance_guard(body, &final_body, &ext)?;
        // 撤销快照：落盘前记录旧内容（会话级，undo_edit 工具按栈序恢复）
        crate::agent::undo::snapshot(conversation_id, p, &bytes);
        // 落盘为 IO 操作，放 spawn_blocking 避免钉死 tokio worker
        let p_buf = p.clone();
        let final_buf = final_text.clone();
        let (wmeta, _) = tokio::task::spawn_blocking(move || {
            std::fs::write(&p_buf, final_buf.as_bytes()).map_err(|e| format!("写入文件失败: {e}"))?;
            Ok::<(Option<std::fs::Metadata>, ()), String>((std::fs::metadata(&p_buf).ok(), ()))
        })
        .await
        .map_err(|e| format!("写入文件任务异常: {e}"))??;
        if let Some(meta) = wmeta {
            stamp_put(p, &meta, final_text.as_bytes());
        }
        let show = |s: &str| -> String {
            let n = s.chars().count();
            if n > 200 {
                format!("{}…（共 {n} 字符）", s.chars().take(200).collect::<String>())
            } else {
                s.to_string()
            }
        };
        let mut report = format!(
            "已{}完整代码块（第 {}–{} 行，共 {} 行）\n",
            if spec.new.is_empty() { "删除" } else { "替换" },
            o + 1,
            c + 1,
            block_lines
        );
        report.push_str(&format!("块首行：{}\n", show(body_lines[o].trim())));
        if !spec.new.is_empty() {
            report.push_str(&format!("新内容：{}\n", show(&spec.new)));
        }
        report.push_str(&format!(
            "（语言感知成对匹配：块结束符已包含在操作范围内）\n文件：{}",
            p.display()
        ));
        return Ok(report);
    }

    let (replaced, count) = apply_edit(body, old, new, replace_all)
        .map_err(|e| with_advice("edit_file", e))?;
    // 配平守卫：原文件配平而替换后失衡 → 拒绝落盘（新内容残缺，如漏结束符）
    {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        balance_guard(body, &replaced, &ext)?;
    }
    // 先构造 final_text 时不再 move replaced（dry_run 分支仍需借用）
    let final_text = if has_bom { format!("\u{feff}{replaced}") } else { replaced.clone() };
    // [58] dry-run：内存 diff 预览，不落盘、不写 undo
    if args["dry_run"].as_bool().unwrap_or(false) {
        let old_lines: Vec<&str> = body.split('\n').collect();
        let new_lines: Vec<&str> = replaced.split('\n').collect();
        let (diff, add_l, del_l) = build_unified_diff(&old_lines, &new_lines, &p.display().to_string(), 0, 0);
        return Ok(format!(
            "【dry_run 预览】将替换 {count} 处（{}）预计 +{add_l} −{del_l} 行（未落盘）\n{diff}",
            if replace_all { "全部替换" } else { "仅第一处" }
        ));
    }
    // 撤销快照：落盘前记录旧内容（会话级，undo_edit 工具按栈序恢复）
    crate::agent::undo::snapshot(conversation_id, p, &bytes);
    // 落盘为 IO 操作，放 spawn_blocking 避免钉死 tokio worker
    let p_buf = p.clone();
    let final_buf = final_text.clone();
    let (wmeta, _) = tokio::task::spawn_blocking(move || {
        std::fs::write(&p_buf, final_buf.as_bytes()).map_err(|e| format!("写入文件失败: {e}"))?;
        Ok::<(Option<std::fs::Metadata>, ()), String>((std::fs::metadata(&p_buf).ok(), ()))
    })
    .await
    .map_err(|e| format!("写入文件任务异常: {e}"))??;
    if let Some(meta) = wmeta {
        stamp_put(p, &meta, final_text.as_bytes());
    }
    // 原文/新文各截 200 字符展示，避免大段替换把结果撑爆上下文
    let show = |s: &str| -> String {
        let n = s.chars().count();
        if n > 200 {
            format!("{}…（共 {n} 字符）", s.chars().take(200).collect::<String>())
        } else {
            s.to_string()
        }
    };
    Ok(format!(
        "已替换 {} 处（{}）\n原文：{}\n新文：{}\n文件：{}",
        count,
        if replace_all { "全部替换" } else { "仅第一处" },
        show(old),
        show(new),
        p.display()
    ))
}

// ---------- diff 预览（preview_edit） ----------

/// 全量行级 LCS 的行数上限：超过则只 diff 变化区域（防大文件 O(n×m) 内存爆）
const MAX_DIFF_LINES: usize = 600;
/// unified diff 上下文行数
const DIFF_CONTEXT: usize = 3;

/// 行级 LCS 回溯 → unified diff 文本。
/// `old_base / new_base`：窗口切片的起始行偏移（大文件窗口 diff 时行号对齐用）。
/// 返回 (diff 文本, 新增行数, 删除行数)。
pub(crate) fn build_unified_diff(
    old_lines: &[&str],
    new_lines: &[&str],
    path: &str,
    old_base: usize,
    new_base: usize,
) -> (String, usize, usize) {
    let n = old_lines.len();
    let m = new_lines.len();
    // LCS DP（行相等判定；调用方保证窗口 ≤ MAX_DIFF_LINES）
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if old_lines[i] == new_lines[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    // 回溯 → 编辑序列（' ' 相同 / '-' 删除 / '+' 新增）
    let mut ops: Vec<(char, usize, usize)> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let mut add = 0usize;
    let mut del = 0usize;
    while i < n && j < m {
        if old_lines[i] == new_lines[j] {
            ops.push((' ', i, j));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            ops.push(('-', i, usize::MAX));
            del += 1;
            i += 1;
        } else {
            ops.push(('+', usize::MAX, j));
            add += 1;
            j += 1;
        }
    }
    while i < n {
        ops.push(('-', i, usize::MAX));
        del += 1;
        i += 1;
    }
    while j < m {
        ops.push(('+', usize::MAX, j));
        add += 1;
        j += 1;
    }
    if ops.is_empty() {
        return (String::new(), 0, 0); // 无变化
    }
    // 每步操作前的行号游标（@@ 头计算用）
    let mut old_pos = vec![0usize; ops.len() + 1];
    let mut new_pos = vec![0usize; ops.len() + 1];
    for (idx, op) in ops.iter().enumerate() {
        old_pos[idx + 1] = old_pos[idx] + usize::from(op.0 != '+');
        new_pos[idx + 1] = new_pos[idx] + usize::from(op.0 != '-');
    }
    let mut out = String::new();
    out.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
    let mut k = 0usize;
    while k < ops.len() {
        if ops[k].0 == ' ' {
            k += 1;
            continue;
        }
        // hunk 起点：往前补 context 行
        let start = k.saturating_sub(DIFF_CONTEXT);
        // hunk 终点：从 k 向后，连续空格超过 2×context 视为上下文断层（尾部留给下一 hunk）
        let mut end = k;
        let mut run = 0usize;
        while end < ops.len() {
            if ops[end].0 == ' ' {
                run += 1;
                if run > DIFF_CONTEXT * 2 {
                    break;
                }
            } else {
                run = 0;
            }
            end += 1;
        }
        if run > DIFF_CONTEXT * 2 {
            // 断点落在 run 个连续空格内：hunk 尾部只保留 context 行
            end -= run - DIFF_CONTEXT;
        }
        // @@ 行号：hunk 内第一条 old/new 行（全新增/全删除块用 0 计数写法）
        let has_old = ops[start..end].iter().any(|o| o.0 != '+');
        let has_new = ops[start..end].iter().any(|o| o.0 != '-');
        let old_start = if has_old {
            let fi = ops[start..end].iter().position(|o| o.0 != '+').unwrap() + start;
            old_pos[fi] + old_base + 1
        } else {
            old_pos[start] + old_base
        };
        let new_start = if has_new {
            let fi = ops[start..end].iter().position(|o| o.0 != '-').unwrap() + start;
            new_pos[fi] + new_base + 1
        } else {
            new_pos[start] + new_base
        };
        let old_cnt = old_pos[end] - old_pos[start];
        let new_cnt = new_pos[end] - new_pos[start];
        out.push_str(&format!("@@ -{},{} +{},{} @@\n", old_start, old_cnt, new_start, new_cnt));
        for op in &ops[start..end] {
            match op.0 {
                ' ' => out.push_str(&format!(" {}\n", old_lines[op.1])),
                '-' => out.push_str(&format!("-{}\n", old_lines[op.1])),
                _ => out.push_str(&format!("+{}\n", new_lines[op.2])),
            }
        }
        k = end;
    }
    (out, add, del)
}

/// 生成新旧文本的 unified diff：大文件（任一侧 > MAX_DIFF_LINES）只 diff 变化区域。
fn limited_diff_text(old_text: &str, new_text: &str, path: &str) -> (String, usize, usize) {
    let old_lines: Vec<&str> = old_text.split('\n').collect();
    let new_lines: Vec<&str> = new_text.split('\n').collect();
    if old_lines.len() <= MAX_DIFF_LINES && new_lines.len() <= MAX_DIFF_LINES {
        return build_unified_diff(&old_lines, &new_lines, path, 0, 0);
    }
    // 大文件：定位首尾差异，取 ±10 行窗口做 diff
    let n = old_lines.len();
    let m = new_lines.len();
    let mut first = 0usize;
    while first < n.min(m) && old_lines[first] == new_lines[first] {
        first += 1;
    }
    let mut last = 0usize;
    while last < n.min(m) && old_lines[n - 1 - last] == new_lines[m - 1 - last] {
        last += 1;
    }
    let ws = first.saturating_sub(10);
    let we_old = (n - last + 10).min(n);
    let we_new = (m - last + 10).min(m);
    let (diff, add, del) =
        build_unified_diff(&old_lines[ws..we_old], &new_lines[ws..we_new], path, ws, ws);
    (
        format!("（文件行数超 {MAX_DIFF_LINES}，仅展示变化区域）\n{diff}"),
        add,
        del,
    )
}

/// preview_edit：与 edit_file 相同的参数与校验，但只计算并返回 unified diff（不落盘）。
/// 模型先预览改动给用户确认，确认后再调用 edit_file 应用同一修改；
/// 配合审批流水线（approval_mode=ask）可形成"危险编辑先预览、人工 OK 再落盘"闭环。
pub(super) async fn preview_edit(args: &Value, roots: &[String], _conversation_id: &str) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法预览编辑".into());
    }
    let spec = EditFileRequest::from_args(args)?.resolve(roots)?;
    let p = &spec.path;
    if !p.is_file() {
        return Err(format!(
            "文件不存在（preview_edit 只预览已有文件的编辑；新建文件请直接用 write_file）：{}",
            p.display()
        ));
    }
    let bytes = std::fs::read(p).map_err(|e| format!("读取文件失败: {e}"))?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err("文件是二进制，无法以文本方式编辑".into());
    }
    // 严格 UTF-8 校验（与 edit_file 同口径，防 GBK 文件预览后写坏）
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| {
            format!(
                "文件 {} 非 UTF-8 编码（可能是 GBK/GB2312），为避免中文被写坏已拒绝预览。请先转 UTF-8 再编辑",
                p.display()
            )
        })?
        .to_string();
    // BOM 剥离比较（写回时保留，与 edit_file 一致）
    let body = match text.strip_prefix('\u{feff}') {
        Some(b) => b,
        None => text.as_str(),
    };
    // 计算编辑后的正文（不落盘；与 edit_file 同口径：old 精确替换 / start 语言感知块替换 /
    // starts 批量块替换，块定位走共用 locate_edit_block / plan_batch_blocks：
    // 超界显式报错 + anchor 漂移重定位 + 重叠校验，预览即拦截）
    let final_body: String = if let Some(starts) = spec.starts.as_deref() {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        plan_batch_blocks(body, starts, &spec.news, &spec.anchors, &ext)?.0
    } else if let Some(start_line) = spec.start {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let body_lines: Vec<&str> = body.split('\n').collect();
        let (o, c) = locate_edit_block(&body_lines, start_line, spec.anchor.as_deref(), &ext)?;
        let mut line_starts: Vec<usize> = Vec::with_capacity(body_lines.len());
        let mut off = 0usize;
        for l in body.split_inclusive('\n') {
            line_starts.push(off);
            off += l.len();
        }
        let start_off = line_starts[o];
        let end_off = if c + 1 < line_starts.len() { line_starts[c + 1] } else { body.len() };
        format!("{}{}{}", &body[..start_off], spec.new, &body[end_off..])
    } else {
        let (replaced, _count) = apply_edit(body, &spec.old, &spec.new, spec.replace_all)
            .map_err(|e| with_advice("preview_edit", e))?;
        replaced
    };
    if final_body == body {
        return Ok(format!("【edit 预览】文件 {}：无实际变化（old 与 new 内容相同）", p.display()));
    }
    // 生成 unified diff（大文件只 diff 变化区域）
    let (diff, add, del) = limited_diff_text(body, &final_body, &p.display().to_string());
    Ok(format!(
        "【edit 预览】（未落盘，文件：{}，预计 +{add} −{del} 行）\n{diff}\n确认无误后用 edit_file 应用同一修改（old/new 参数保持不变）；如需调整请重新调用 preview_edit。",
        p.display()
    ))
}

// ---------- 命令执行工具 ----------

/// 敏感文件保护（write_file/edit_file 等写路径代码级拦截）：命中返回拒绝原因。
/// 委托不变式注册表（crate::agent::invariants）：新增约束只注册不散改调用点。
pub(super) fn is_protected_file(p: &std::path::Path) -> Option<&'static str> {
    crate::agent::invariants::check_write(p).map(|(_, reason)| reason)
}

pub(super) async fn multi_edit(
    args: &Value,
    roots: &[String],
    conversation_id: &str,
) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法编辑文件".into());
    }
    let edits = args["edits"].as_array().ok_or(
        "multi_edit 需要参数 {\"edits\":[{\"path\":\"<文件>\",\"old\":\"<原文>\",\"new\":\"<新文>\",\"replace_all\":<可选布尔>}]}",
    )?;
    if edits.is_empty() {
        return Err("edits 数组不能为空".into());
    }
    if edits.len() > 10 {
        return Err("单次最多批量编辑 10 个文件，请拆分为多次调用".into());
    }
    let mut report: Vec<String> = Vec::new();
    let mut ok_count = 0usize;
    for (i, e) in edits.iter().enumerate() {
        let raw = match e.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                report.push(format!("{}. ❌ 缺少 path 参数，跳过", i + 1));
                continue;
            }
        };
        let old = e.get("old").and_then(|v| v.as_str()).unwrap_or("");
        let new = e.get("new").and_then(|v| v.as_str()).unwrap_or("");
        let replace_all = e.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
        match apply_single_edit(raw, old, new, replace_all, roots, conversation_id) {
            Ok(msg) => {
                ok_count += 1;
                report.push(format!("{}. ✅ {msg}", i + 1));
            }
            Err(err) => report.push(format!("{}. ❌ {err}", i + 1)),
        }
    }
    Ok(format!(
        "批量编辑完成：{ok_count}/{} 项成功\n\n{}",
        edits.len(),
        report.join("\n")
    ))
}

/// 单文件替换核心（multi_edit 复用；校验/冲突保护/撤销快照/落盘与 edit_file 同口径）
pub(super) fn apply_single_edit(
    raw: &str,
    old: &str,
    new: &str,
    replace_all: bool,
    roots: &[String],
    conversation_id: &str,
) -> Result<String, String> {
    if old.is_empty() {
        return Err(format!("{raw}: old 参数不能为空"));
    }
    let p = resolve_in_roots(roots, raw)?;
    check_nested_module_path(&p, roots)?;
    if let Some(reason) = is_protected_file(&p) {
        return Err(format!("{raw}: 被安全策略拒绝：{reason}"));
    }
    if !p.is_file() {
        return Err(format!("{raw}: 路径不是文件"));
    }
    let meta = std::fs::metadata(&p).map_err(|e| e.to_string())?;
    if meta.len() > 1024 * 1024 {
        return Err(format!("{raw}: 超过 1MB，请用 run_command 处理"));
    }
    let bytes = std::fs::read(&p).map_err(|e| format!("读取失败: {e}"))?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err(format!("{raw}: 二进制文件无法文本编辑"));
    }
    if has_external_change(&p, &bytes) {
        return Err(format!(
            "{raw}: 自上次读取后被修改，请先 read_file 后重试（重新读取解除冲突保护）"
        ));
    }
    // 严格 UTF-8 校验：GBK 文件用 lossy 读入再写回会把中文永久写坏（与 edit_file 同口径拒绝）
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| format!("{raw}: 文件非 UTF-8 编码（可能是 GBK/GB2312），为避免中文被写坏已拒绝编辑，请先转换为 UTF-8"))?
        .to_string();
    // BOM 剥离匹配、写回保留（与 edit_file 同口径）
    let (has_bom, body) = match text.strip_prefix('\u{feff}') {
        Some(b) => (true, b),
        None => (false, text.as_str()),
    };
    let (replaced, count) = apply_edit(body, old, new, replace_all)?;
    let final_text = if has_bom { format!("\u{feff}{replaced}") } else { replaced };
    crate::agent::undo::snapshot(conversation_id, &p, &bytes);
    std::fs::write(&p, final_text.as_bytes()).map_err(|e| format!("写入失败: {e}"))?;
    if let Ok(meta) = std::fs::metadata(&p) {
        stamp_put(&p, &meta, final_text.as_bytes());
    }
    Ok(format!("{}（替换 {count} 处）", p.display()))
}

/// copy_file：复制项目内文件/目录（不覆盖目标，禁止受保护路径）
pub(super) async fn copy_file(args: &Value, roots: &[String]) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法复制文件".into());
    }
    let from_raw = args["from"].as_str().ok_or("copy_file 需要参数 {\"from\":\"<源路径>\",\"to\":\"<目标路径>\"}")?.trim();
    let to_raw = args["to"].as_str().ok_or("copy_file 缺少 to 参数（目标路径）")?.trim();
    if from_raw.is_empty() || to_raw.is_empty() {
        return Err("from/to 参数不能为空".into());
    }
    let src = resolve_in_roots(roots, from_raw)?;
    // 目标允许不存在（复制到新路径），用 resolve_for_write 解析（防越界语义同 move_file）；
    // 目标已存在时由下方拒绝覆盖。
    let dst = resolve_for_write(roots, to_raw)?;
    if let Some(reason) = is_protected_file(&src) {
        return Err(format!("复制被安全策略拒绝：{reason}"));
    }
    if !src.exists() {
        return Err(format!("源路径不存在: {from_raw}"));
    }
    if dst.exists() {
        return Err(format!("目标已存在，拒绝覆盖: {}", dst.display()));
    }
    const PROTECTED: [&str; 9] = [
        ".git", "oh_modules", "node_modules", ".ohpm", ".deveco-agent",
        "build", ".hvigor", ".idea", ".arkui-x",
    ];
    let sname = src.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if PROTECTED.contains(&sname) && src.is_dir() {
        return Err(format!("受保护目录 {sname} 不允许复制"));
    }
    for comp in dst.ancestors().skip(1) {
        if let Some(n) = comp.file_name().and_then(|s| s.to_str()) {
            if PROTECTED.contains(&n) && comp.is_dir() {
                return Err(format!("目标落入受保护目录 {n}，拒绝复制"));
            }
        }
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目标父目录失败 {}: {e}", parent.display()))?;
    }
    if src.is_dir() {
        copy_dir_recursive(&src, &dst)?;
    } else {
        std::fs::copy(&src, &dst).map_err(|e| e.to_string())?;
    }
    Ok(format!("已复制 {} → {}", src.display(), dst.display()))
}

/// get_file_info：文件元信息（大小/修改时间/行数/编码/类型）
pub(super) async fn get_file_info(args: &Value, roots: &[String]) -> Result<String, String> {
    let raw = args["path"].as_str().ok_or("get_file_info 需要参数 {\"path\":\"<文件路径>\"}")?.trim();
    if raw.is_empty() {
        return Err("get_file_info 需要参数 {\"path\":\"<文件路径>\"}".into());
    }
    let p = if roots.is_empty() {
        PathBuf::from(raw)
    } else {
        resolve_in_roots(roots, raw)?
    };
    let meta = std::fs::metadata(&p).map_err(|e| format!("无法读取 {}: {e}", p.display()))?;
    let mut out = String::new();
    out.push_str(&format!("路径: {}\n", p.display()));
    out.push_str(&format!("类型: {}\n", if meta.is_dir() { "目录" } else if meta.is_file() { "文件" } else { "其他" }));
    out.push_str(&format!("大小: {} 字节", meta.len()));
    if meta.len() >= 1024 {
        out.push_str(&format!("（约 {:.1} KB）", meta.len() as f64 / 1024.0));
    }
    out.push('\n');
    if let Ok(mtime) = meta.modified() {
        if let Ok(dt) = mtime.duration_since(std::time::UNIX_EPOCH) {
            out.push_str(&format!("修改时间: {}\n", dt.as_secs()));
        }
    }
    // 行数 + 编码探测（单次读取）：严格 UTF-8 → GBK 验证 → 未知
    // （from_utf8_lossy 会把 GBK 误报为 UTF-8，必须严格校验）
    if let Ok(bytes) = std::fs::read(&p) {
        let bin = bytes.iter().take(2048).any(|b| *b == 0);
        let lines = if bin { 0 } else { smart_decode(&bytes).lines().count() };
        out.push_str(&format!("行数: {lines}\n"));
        let enc = if bin {
            "二进制（含 NUL 字节）"
        } else if std::str::from_utf8(&bytes).is_ok() {
            "UTF-8 文本"
        } else {
            let (_, _, had_err) = encoding_rs::GBK.decode(&bytes);
            if had_err {
                "非 UTF-8（未知编码，可能是 Shift-JIS 等）"
            } else {
                "GBK/GB2312（非 UTF-8，编辑前需转换）"
            }
        };
        out.push_str(&format!("编码: {enc}\n"));
        out.push_str(&format!("只读: {}\n", meta.permissions().readonly()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同步构造 tokio runtime 跑 async 工具（与 mod.rs 测试同款）
    fn block_on_rt<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Runtime::new().unwrap().block_on(f)
    }

    /// 在独立临时目录创建测试文件，返回 (文件路径, 会话根目录列表)
    fn tmp_file(tag: &str, content: &str, ext: &str) -> (std::path::PathBuf, Vec<String>) {
        let dir = std::env::temp_dir().join(format!(
            "agent_block_test_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(format!("t.{ext}"));
        std::fs::write(&f, content).unwrap();
        (f, vec![dir.to_string_lossy().to_string()])
    }

    #[test]
    fn nested_module_path_detected() {
        // 回归：模块目录嵌套（entry/entry/）必须被拦截并给出标准结构指引
        let dir = std::env::temp_dir().join(format!(
            "nested_path_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("entry/src")).unwrap();
        let roots = vec![dir.to_string_lossy().to_string()];
        // 正常模块路径放行
        let ok = dir.join("entry/src/test/List.test.ets");
        assert!(check_nested_module_path(&ok, &roots).is_ok(), "正常模块路径应放行");
        // 嵌套路径拦截（目录不存在也应拦截——检查的是路径拼接而非磁盘状态）
        let bad = dir.join("entry/entry/src/test/List.test.ets");
        let err = check_nested_module_path(&bad, &roots).unwrap_err();
        assert!(err.contains("嵌套") && err.contains("entry/entry"), "错误应说明嵌套段: {err}");
        assert!(err.contains("entry/src"), "错误应给出正确路径示例: {err}");
        // 任意连续重复段都拦截
        let weird = dir.join("a/b/b/c.txt");
        assert!(check_nested_module_path(&weird, &roots).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------- 成对代码块核心 ----------

    #[test]
    fn block_whole_method_detected() {
        let src = "fn a() {\n  let x = 1;\n  if x {\n    foo();\n  }\n  y();\n}\n\nfn b() {\n  z();\n}\n";
        let l: Vec<&str> = src.lines().collect();
        // 方法 a 内部（含 if 内部）→ 整方法 0-6
        assert_eq!(find_enclosing_block(&l, 3, "rs"), Some((0, 6)));
        assert_eq!(find_root_block(&l, 3, "rs"), Some((0, 6)));
        // 方法 b（头行与体内行一致）
        assert_eq!(find_enclosing_block(&l, 8, "rs"), Some((8, 10)));
        assert_eq!(find_enclosing_block(&l, 9, "rs"), Some((8, 10)));
        // 方法之间（空行）→ 无块
        assert_eq!(find_enclosing_block(&l, 7, "rs"), None);
    }

    #[test]
    fn block_ignores_strings_and_comments() {
        let src = "fn a() {\n  let s = \"{\";\n  let t = \"}\";\n  // }\n  let u = `x`;\n  foo();\n}\n";
        let l: Vec<&str> = src.lines().collect();
        // 字符串/注释里的括号不参与配对 → 结束符是第 6 行 }
        assert_eq!(find_enclosing_block(&l, 5, "ts"), Some((0, 6)));
    }

    #[test]
    fn block_else_falls_back_to_outermost() {
        let src = "if (a) {\n  foo();\n} else {\n  bar();\n}\n";
        let l: Vec<&str> = src.lines().collect();
        // 无方法级根 → 退回最外层块（else 块）
        assert_eq!(find_enclosing_block(&l, 3, "ts"), Some((2, 4)));
        // 行首 } 属前一块：if 块 0..=2
        assert_eq!(find_matching_close(&l, 0, "ts"), Some(2));
    }

    #[test]
    fn block_nested_arrow_prefers_innermost_root() {
        let src = "fn a() {\n  const inner = () => {\n    x();\n  };\n  y();\n}\n";
        let l: Vec<&str> = src.lines().collect();
        // 内层箭头内部 → 最内层根块是箭头（1..=3）
        assert_eq!(find_enclosing_block(&l, 2, "ts"), Some((1, 3)));
        // 外层方法体（箭头外）→ 整个方法 a（0..=5）
        assert_eq!(find_enclosing_block(&l, 4, "ts"), Some((0, 5)));
        // 锚定：窗口起点在方法 a 首行 → 末尾不跳进箭头，仍补齐整个方法
        assert_eq!(find_block_anchored(&l, 2, 0, "ts"), Some((0, 5)));
        assert_eq!(find_block_anchored(&l, 2, 1, "ts"), Some((1, 3)));
    }

    #[test]
    fn block_matching_close_skips_leading_close() {
        let src = "fn a() {\n  if x {\n    y();\n  } else {\n    z();\n  }\n}\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(find_matching_close(&l, 1, "ts"), Some(3)); // if 块 1..=3
        assert_eq!(find_matching_close(&l, 3, "ts"), Some(5)); // else 块 3..=5
    }

    #[test]
    fn block_python_indent() {
        let src = "def a():\n    x = 1\n    if x:\n        y()\n    return 2\n\ndef b():\n    z()\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(find_enclosing_block(&l, 3, "py"), Some((0, 4)));
        assert_eq!(find_enclosing_block(&l, 7, "py"), Some((6, 7)));
        assert_eq!(find_root_block(&l, 3, "py"), Some((0, 4)));
    }

    #[test]
    fn block_python_multiline_signature() {
        // 多行参数表：`):` 行缩进归位，不得被误当块尾截掉函数体
        let src = "def foo(\n    a,\n    b,\n):\n    return a + b\n\nx = 1\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(find_root_block(&l, 1, "py"), Some((0, 4)));
        assert_eq!(find_root_block(&l, 4, "py"), Some((0, 4)));
    }

    #[test]
    fn block_python_signature_trailing_comment() {
        // def 行带行尾注释：`:` 判定须去注释后再看行尾
        let src = "def foo():  # note\n    return 1\n\nx = 2\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(find_root_block(&l, 1, "py"), Some((0, 1)));
    }

    #[test]
    fn block_python_oneliner_def() {
        let src = "def add(a, b): return a + b\n\nz = add(1, 2)\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(find_root_block(&l, 0, "py"), Some((0, 0)));
    }

    #[test]
    fn block_rust_lifetime_quotes_ignored() {
        // 生命周期 'a（奇数个）不得被当字符串开引号吞掉方法开 {
        let src = "fn foo<'a>(x: &'a str) -> &'a str {\n    x\n}\nfn bar() {\n}\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(find_root_block(&l, 1, "rs"), Some((0, 2)));
        assert_eq!(find_matching_close(&l, 0, "rs"), Some(2));
    }

    #[test]
    fn block_jsx_apostrophe_in_text_ignored() {
        // JSX 文本撇号（It's）不得开启字符串模式吞掉后续行的 }
        let src = "function Greet() {\n  return <p>It's fine</p>;\n}\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(find_root_block(&l, 1, "tsx"), Some((0, 2)));
        assert_eq!(find_matching_close(&l, 0, "tsx"), Some(2));
    }

    #[test]
    fn scanner_rust_char_literals_still_strings() {
        // 字符字面量里的括号仍按字符串内容跳过
        let mut sc = LineScanner::default();
        let mut out = String::new();
        sc.scan("let c = '{'; let d = '\\'';", "rs", |c| out.push(c));
        assert_eq!(out, "", "字符字面量内的 {{ 与 '' 不参与配对");
        // 生命周期行：(){} 正常参与配对
        let mut sc2 = LineScanner::default();
        let mut out2 = String::new();
        sc2.scan("fn f<'a>(x: &'a str) -> &'a str {", "rs", |c| out2.push(c));
        assert_eq!(out2, "(){");
    }

    #[test]
    fn scanner_rust_raw_strings() {
        // 三种原始字符串内的括号都不参与配对；检测时消费开头 " 后正常闭合
        let mut sc = LineScanner::default();
        let mut out = String::new();
        sc.scan("let a = r\"{(}\"; let b = r#\"{[\"#; let c = r##\"a\"#b\"##; foo();", "rs", |ch| out.push(ch));
        assert_eq!(out, "()", "r\"..\"/r#\"..\"#/r##\"..\"## 内括号跳过，仅 foo() 参与");
        // r#"..."# 内的引号不闭合（旧逻辑会当普通字符串开引号而错乱）；r"..." 遇 " 即闭合是正确语义
        let mut sc2 = LineScanner::default();
        let mut out2 = String::new();
        sc2.scan("let s = r#\"he said \"hi\" ok\"#; g();", "rs", |ch| out2.push(ch));
        assert_eq!(out2, "()");
    }

    #[test]
    fn block_rust_raw_string_multiline() {
        // 跨行原始字符串含未配对 { " ( ：不得污染后续行配对
        let src = "fn f() {\n    let s = r#\"\n        { \" (\n    \"#;\n    g();\n}\nfn next() {\n}\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(find_matching_close(&l, 0, "rs"), Some(5));
        assert_eq!(find_root_block(&l, 4, "rs"), Some((0, 5)));
        assert_eq!(find_root_block(&l, 6, "rs"), Some((6, 7)), "fn next 块不受上方原始字符串影响");
    }

    #[test]
    fn scanner_rust_raw_identifier_not_raw_string() {
        // 原始标识符 r#type：r 后 # 后不是 " → 不按原始字符串处理
        let mut sc = LineScanner::default();
        let mut out = String::new();
        sc.scan("let r#type = r#\"}\"#; call();", "rs", |ch| out.push(ch));
        assert_eq!(out, "()", "r#type 中的字符不影响配对，r#\"}}\"# 内 }} 跳过");
    }

    #[test]
    fn outline_block_range_multiline_sig() {
        // 多行签名：fn 定义行 → 完整方法体区间（含签名与闭合 }）
        let src = "fn foo(\n    a: u32,\n    b: u32,\n) -> u32 {\n    a + b\n}\nfn bar() {\n}\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(outline_block_range(&l, 0, "rs", None), Some((0, 5)));
        assert_eq!(outline_block_range(&l, 6, "rs", None), Some((6, 7)));
    }

    #[test]
    fn outline_block_range_trait_decl_and_attrs_none() {
        // trait 声明（; 结尾）与属性行无块区间
        let src = "pub trait T {\n    fn a(&self);\n    fn b(&self) {\n        self.a();\n    }\n}\n#[test]\nfn t() {\n}\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(outline_block_range(&l, 1, "rs", None), None, "fn a(&self); 无块");
        assert_eq!(outline_block_range(&l, 2, "rs", None), Some((2, 4)));
        assert_eq!(outline_block_range(&l, 0, "rs", None), Some((0, 5)), "trait 块整体");
    }

    #[test]
    fn outline_block_range_python_def() {
        let src = "def foo(\n    a,\n):\n    return a\n\nx = 1\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(outline_block_range(&l, 0, "py", None), Some((0, 3)));
    }

    #[test]
    fn outline_renders_block_ranges() {
        let src = "fn a() {\n    x();\n}\nfn b(\n    p: u32,\n) -> u32 {\n    p\n}\nfn c(&self);\n";
        let l: Vec<&str> = src.lines().collect();
        let out = render_outline(std::path::Path::new("t.rs"), &l, 128, 1, None);
        assert!(out.contains("1-3") || out.contains("    1-3"), "fn a 区间 L1-L3: {out}");
        assert!(out.contains("4-8"), "fn b 多行签名区间 L4-L8: {out}");
        assert!(out.contains("区间"), "头部应说明区间用法: {out}");
        assert!(!out.contains("9-"), "fn c(&self); 声明不显示区间: {out}");
    }

    // ---------- 核验组：先证明薄弱点存在，再修复转绿 ----------

    #[test]
    fn scanner_kotlin_triple_quote_string() {
        // Kotlin 三引号字符串内的 { 与内嵌 " 不参与配对
        let src = "fun f() {\n    val s = \"\"\"a \"b\" { c\"\"\".length\n    g()\n}\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(find_matching_close(&l, 0, "kt"), Some(3), "三引号内 {{ 不应推栈");
        // 跨行三引号
        let src2 = "fun h() {\n    val t = \"\"\"\n        { \" (\n    \"\"\"\n    g()\n}\n";
        let l2: Vec<&str> = src2.lines().collect();
        assert_eq!(find_matching_close(&l2, 0, "kt"), Some(5), "跨行三引号不污染配对");
    }

    #[test]
    fn scanner_js_regex_char_class() {
        // JS 正则字符类内的单个 { 不参与配对：/[{]/ 若按块字符处理，{ 入栈后方法的 }
        // 弹的是字符类的 {，目标永不闭合 → 整个文件块结构错位
        let src = "function f() {\n  const re = /[{]/;\n  g();\n}\nfunction next() {\n}\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(find_matching_close(&l, 0, "ts"), Some(3), "正则字符类内 {{ 应跳过");
        assert_eq!(find_matching_close(&l, 4, "ts"), Some(5), "后续方法不受污染");
        // 除法上下文（a / b）不得误判为正则
        let mut sc = LineScanner::default();
        let mut out = String::new();
        sc.scan("let x = a / b[0] / c;", "ts", |ch| out.push(ch));
        assert_eq!(out, "[]", "除法两侧的 [] 正常参与，/ 不触发正则模式");
    }

    #[test]
    fn python_docstring_with_quotes_safe() {
        // 免修证明：Python 为缩进制块，docstring 内引号/花括号不影响块识别
        let src = "def f():\n    \"\"\"He said \"hi\" {ok}\"\"\"\n    return 1\n\nx = 2\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(find_root_block(&l, 1, "py"), Some((0, 2)));
    }

    #[test]
    fn edit_start_out_of_range_errors() {
        // 无尾随换行：start=999 钳到末行 `}`（fn b 块内）→ 会静默删掉 fn b，必须报错
        let content = "fn a() {\n    x();\n}\nfn b() {\n    y();\n}";
        let (f, roots) = tmp_file("edit_oor", content, "rs");
        let rel = f.to_string_lossy().to_string();
        let args = serde_json::json!({"path": rel, "start": 999, "new": ""});
        let out = block_on_rt(edit_file(&args, &roots, "conv"));
        assert!(out.is_err(), "start 超界应报错: {out:?}");
        let after = std::fs::read_to_string(&f).unwrap();
        assert_eq!(after, content, "文件不应被改动");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn edit_anchor_relocates_drifted_lineno() {
        // 行号漂移：旧 start=4 本指向 fn b，但 fn a 上方插了 2 行 → start=4 落在 fn a 内。
        // anchor="fn b" 应重定位到真正的 fn b 块（从第 6 行起），删掉的是 fn b 不是 fn a。
        let content = "// note1\n// note2\nfn a() {\n    x();\n}\nfn b() {\n    y();\n    z();\n}\n";
        let (f, roots) = tmp_file("edit_anchor", content, "rs");
        let rel = f.to_string_lossy().to_string();
        let args = serde_json::json!({"path": rel, "start": 4, "new": "", "anchor": "fn b"});
        let out = block_on_rt(edit_file(&args, &roots, "conv")).expect("anchor 重定位应成功");
        assert!(out.contains("fn b() {"), "报告应含被删块首行: {out}");
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(after.contains("fn a()"), "fn a 不应被误删: {after}");
        assert!(!after.contains("fn b()"), "fn b 应被删除: {after}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn edit_anchor_mismatch_rejected() {
        // anchor 与实际块不符且附近找不到 → 拒绝（防改错块）
        let content = "fn a() {\n    x();\n}\nfn b() {\n    y();\n}\n";
        let (f, roots) = tmp_file("edit_anchor_bad", content, "rs");
        let rel = f.to_string_lossy().to_string();
        let args = serde_json::json!({"path": rel, "start": 4, "new": "", "anchor": "fn nonexistent"});
        let out = block_on_rt(edit_file(&args, &roots, "conv"));
        assert!(out.is_err(), "anchor 不符应拒绝: {out:?}");
        let after = std::fs::read_to_string(&f).unwrap();
        assert_eq!(after, content, "文件不应被改动");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn edit_balance_guard_rejects_broken_new() {
        // 原文件配平、替换后失衡 → 拒绝落盘（防模型生成残缺块直接写坏文件）
        let content = "fn a() {\n    x();\n}\nfn b() {\n    y();\n}\n";
        let (f, roots) = tmp_file("edit_bal", content, "rs");
        let rel = f.to_string_lossy().to_string();
        // start=4（fn b 行），new 缺闭合 }
        let args = serde_json::json!({"path": rel, "start": 4, "new": "fn b() {\n    y();\n"});
        let out = block_on_rt(edit_file(&args, &roots, "conv"));
        assert!(out.is_err(), "失衡替换应被拒绝: {out:?}");
        let after = std::fs::read_to_string(&f).unwrap();
        assert_eq!(after, content, "文件不应被写坏");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn edit_balance_guard_allows_balanced_and_none_style() {
        // 正常配平替换放行；非结构化文件（txt）不做配平校验
        let content = "fn a() {\n    x();\n}\n";
        let (f, roots) = tmp_file("edit_ok", content, "rs");
        let rel = f.to_string_lossy().to_string();
        let args = serde_json::json!({"path": rel, "start": 1, "new": "fn a() {\n    z();\n}\n"});
        block_on_rt(edit_file(&args, &roots, "conv")).expect("配平替换应成功");
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(after.contains("z();"));
        std::fs::remove_dir_all(f.parent().unwrap()).ok();

        let (f2, roots2) = tmp_file("edit_txt", "a\nb\n", "txt");
        let rel2 = f2.to_string_lossy().to_string();
        let args2 = serde_json::json!({"path": rel2, "old": "a", "new": "A["});
        block_on_rt(edit_file(&args2, &roots2, "conv")).expect("txt 不做配平校验");
        std::fs::remove_dir_all(f2.parent().unwrap()).ok();
    }

    #[test]
    fn outline_kinds_dart_cs_php_scala() {
        assert_eq!(outline_kind("void main() {", "dart"), Some("函数"));
        assert_eq!(outline_kind("class Foo {", "dart"), Some("类型"));
        assert_eq!(outline_kind("void F() {", "cs"), Some("函数"));
        assert_eq!(outline_kind("public class Foo {", "cs"), Some("类型"));
        assert_eq!(outline_kind("function foo() {", "php"), Some("函数"));
        assert_eq!(outline_kind("class Foo {", "php"), Some("类型"));
        assert_eq!(outline_kind("def main() = {", "scala"), Some("函数"));
        assert_eq!(outline_kind("class Foo {", "scala"), Some("类型"));
        // Go 原有覆盖不回退
        assert_eq!(outline_kind("func main() {", "go"), Some("函数"));
    }

    #[test]
    fn outline_kinds_py_cfamily_word_boundary() {
        // 回归锁定：starts_with_word 带尾随空格调用恒 false 的历史 bug
        //（Python def/class、C-family class 等此前 outline 全为空）
        assert_eq!(outline_kind("def main():", "py"), Some("函数"));
        assert_eq!(outline_kind("async def fetch(x):", "py"), Some("函数"));
        assert_eq!(outline_kind("class Foo:", "py"), Some("类型"));
        assert_eq!(outline_kind("class Foo {", "java"), Some("类型"));
        assert_eq!(outline_kind("public interface Bar {", "java"), None, "public 前缀不剥（c-family 原语义）");
        // render_outline 全链路：Python 文件骨架不再为空
        let src = "import os\n\ndef a():\n    return 1\n\nclass B:\n    pass\n";
        let l: Vec<&str> = src.lines().collect();
        let out = render_outline(std::path::Path::new("t.py"), &l, 64, 1, None);
        assert!(out.contains("函数") && out.contains("类型"), "py outline 应识别 def/class: {out}");
        assert!(out.contains("3-4") && out.contains("6-7"), "py 块区间: {out}");
    }

    #[test]
    fn block_index_equivalent_to_find_enclosing() {
        // 索引查询与 find_enclosing_block 全行等价（嵌套/else/字符串/未闭合/顶层）
        let src = "fn a() {\n    let s = \"{\";\n    if x { y(); }\n    g();\n}\nif (a) {\n  foo();\n} else {\n  bar();\n}\nfn unclosed() {\n    x();\n";
        let l: Vec<&str> = src.lines().collect();
        let bi = BlockIndex::build(&l, "rs");
        for i in 0..l.len() {
            assert_eq!(bi.enclosing(&l, i, "rs"), find_enclosing_block(&l, i, "rs"), "行 {i} 结果应一致");
        }
        // Python 回退路径
        let pysrc = "def a():\n    x = 1\n\ndef b():\n    z()\n";
        let pl: Vec<&str> = pysrc.lines().collect();
        let pbi = BlockIndex::build(&pl, "py");
        for i in 0..pl.len() {
            assert_eq!(pbi.enclosing(&pl, i, "py"), find_enclosing_block(&pl, i, "py"), "py 行 {i}");
        }
    }

    #[test]
    fn block_index_compact_encoding_equivalence() {
        // 紧凑编码（u32+哨兵）在多页场景下仍与 find_enclosing_block 全行等价：
        // 深嵌套、行号超 255（验证非 u8 也能存）、同文件多方法
        let mut src = String::new();
        for m in 0..60 {
            src.push_str(&format!("fn m{m}() {{\n    if x {{\n        y();\n    }}\n}}\n"));
        }
        let l: Vec<&str> = src.lines().collect();
        assert!(l.len() > 255, "行数应超过 255 验证 u32 编码非平凡");
        let bi = BlockIndex::build(&l, "rs");
        for i in 0..l.len() {
            assert_eq!(
                bi.enclosing(&l, i, "rs"),
                find_enclosing_block(&l, i, "rs"),
                "行 {} 结果应一致",
                i + 1
            );
        }
    }

    #[test]
    fn outline_pagination_pages_and_hints() {
        // 250 个函数 → 2 页；第 1 页含前 200 条 + 翻页提示，第 2 页含后 50 条
        let mut src = String::new();
        for m in 0..250 {
            src.push_str(&format!("fn f{m}() {{\n    x();\n}}\n"));
        }
        let l: Vec<&str> = src.lines().collect();
        let p1 = render_outline(std::path::Path::new("t.rs"), &l, 4096, 1, None);
        assert!(p1.contains("共 250 条结构项 / 2 页"), "分页统计: {p1}");
        assert!(p1.contains("当前第 1 页"), "页码标注: {p1}");
        assert!(p1.contains("fn f199"), "第 1 页末条 f199: {p1}");
        assert!(!p1.contains("fn f200"), "第 1 页不含 f200: {p1}");
        assert!(p1.contains("outline_page=2"), "翻页提示: {p1}");
        let p2 = render_outline(std::path::Path::new("t.rs"), &l, 4096, 2, None);
        assert!(p2.contains("当前第 2 页"), "第 2 页页码: {p2}");
        assert!(p2.contains("fn f200"), "第 2 页首条 f200: {p2}");
        assert!(p2.contains("fn f249"), "第 2 页末条 f249: {p2}");
        assert!(!p2.contains("fn f199()"), "第 2 页不含 f199: {p2}");
        // 小文件单页：不显示分页信息（与旧版输出一致）
        let small = "fn a() {\n}\n";
        let sl: Vec<&str> = small.lines().collect();
        let ps = render_outline(std::path::Path::new("t.rs"), &sl, 16, 1, None);
        assert!(!ps.contains("页"), "单页无分页字样: {ps}");
    }

    #[test]
    fn locate_edit_block_shared_semantics() {
        // edit_file / preview_edit 共用口径：超界报错、anchor 重定位、anchor 失败拒绝
        let src = "fn a() {\n    x();\n}\nfn b() {\n    y();\n}\n";
        let l: Vec<&str> = src.lines().collect();
        // 常规定位：start 行所在块
        assert_eq!(locate_edit_block(&l, 1, None, "rs").unwrap(), (0, 2));
        assert_eq!(locate_edit_block(&l, 5, None, "rs").unwrap(), (3, 5));
        // 超界显式报错（含 anchor 提示）
        let err = locate_edit_block(&l, 99, None, "rs").unwrap_err();
        assert!(err.contains("超出文件总行数") && err.contains("anchor"), "{err}");
        // anchor 校验通过：块首行含签名
        assert_eq!(locate_edit_block(&l, 1, Some("fn a"), "rs").unwrap(), (0, 2));
        // anchor 重定位：start 漂移到 b 内但签名是 fn a → ±100 行内重定位回 a 块
        assert_eq!(locate_edit_block(&l, 5, Some("fn a"), "rs").unwrap(), (0, 2));
        // anchor 失败：签名不存在 → 拒绝（不会静默改错块）
        let err2 = locate_edit_block(&l, 1, Some("fn not_exist"), "rs").unwrap_err();
        assert!(err2.contains("anchor 定位失败"), "{err2}");
        // Python 缩进块同口径
        let pysrc = "def a():\n    x = 1\n\ndef b():\n    z()\n";
        let pl: Vec<&str> = pysrc.lines().collect();
        assert_eq!(locate_edit_block(&pl, 2, Some("def a"), "py").unwrap(), (0, 1));
    }

    #[test]
    fn write_file_balance_guard_rejects_unbalanced() {
        // 新建文件：残缺代码（漏 }）拒绝落盘；完整代码正常创建
        let dir = std::env::temp_dir().join(format!("wf_bal_new_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let roots = vec![dir.to_string_lossy().to_string()];
        let f = dir.join("t.rs");
        let args = serde_json::json!({"path": f.to_string_lossy(), "content": "fn a() {\n    x();\n"});
        let out = block_on_rt(write_file(&args, &roots, "conv"));
        assert!(out.is_err(), "新建残缺 rs 应拒绝: {out:?}");
        assert!(!f.exists(), "残缺文件不应落盘");
        let args_ok = serde_json::json!({"path": f.to_string_lossy(), "content": "fn a() {\n    x();\n}\n"});
        block_on_rt(write_file(&args_ok, &roots, "conv")).expect("完整代码应可创建");
        // 覆盖已有配平文件为残缺内容 → 拒绝且原文件不变
        let args_bad = serde_json::json!({"path": f.to_string_lossy(), "content": "fn a() {\n    y();\n"});
        let out2 = block_on_rt(write_file(&args_bad, &roots, "conv"));
        assert!(out2.is_err(), "覆盖为残缺应拒绝: {out2:?}");
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(after.contains('}'), "原文件不应被改坏: {after}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edit_file_batch_replace_and_delete() {
        // 批量模式：一次替换 fn a、删除 fn c，中间 fn b 不受影响
        let dir = std::env::temp_dir().join(format!("ef_batch_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let roots = vec![dir.to_string_lossy().to_string()];
        let f = dir.join("t.rs");
        std::fs::write(&f, "fn a() {\n    old_a();\n}\nfn b() {\n    keep();\n}\nfn c() {\n    old_c();\n}\n").unwrap();
        // 先 read_file 解除外部修改保护；会话 id 用唯一值防并行测试互相污染 undo 栈
        let conv = format!("conv_batch_{}", std::process::id());
        let rd = serde_json::json!({"path": f.to_string_lossy()});
        block_on_rt(read_file(&rd, &roots)).expect("read ok");
        let args = serde_json::json!({
            "path": f.to_string_lossy(),
            "starts": [1, 7],
            "news": ["fn a() {\n    new_a();\n}", ""]
        });
        let out = block_on_rt(edit_file(&args, &roots, &conv)).expect("批量编辑应成功");
        assert!(out.contains("已批量编辑 2 个块"), "报告: {out}");
        assert!(out.contains("starts[0]：L1-L3") && out.contains("已替换"), "{out}");
        assert!(out.contains("starts[1]：L7-L9") && out.contains("已删除"), "{out}");
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(after.contains("new_a();") && !after.contains("old_a();"), "a 已替换: {after}");
        assert!(after.contains("fn b() {\n    keep();\n}"), "b 保持原样: {after}");
        assert!(!after.contains("fn c") && !after.contains("old_c();"), "c 已删除: {after}");
        // undo 一次恢复全部（批量只写一个快照）
        let undo = serde_json::json!({});
        block_on_rt(undo_edit(&undo, &roots, &conv)).expect("undo ok");
        let restored = std::fs::read_to_string(&f).unwrap();
        assert!(restored.contains("old_a();") && restored.contains("old_c();"), "undo 恢复原样: {restored}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edit_file_batch_validation_errors() {
        let dir = std::env::temp_dir().join(format!("ef_bve_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let roots = vec![dir.to_string_lossy().to_string()];
        let f = dir.join("t.rs");
        let src = "fn a() {\n    x();\n}\nfn b() {\n    y();\n}\n";
        std::fs::write(&f, src).unwrap();
        let rd = serde_json::json!({"path": f.to_string_lossy()});
        block_on_rt(read_file(&rd, &roots)).expect("read ok");
        // starts 与 old 互斥
        let e1 = block_on_rt(edit_file(&serde_json::json!({"path": f.to_string_lossy(), "old": "x", "new": "z", "starts": [1], "news": [""]}), &roots, "conv")).unwrap_err();
        assert!(e1.contains("互斥"), "{e1}");
        // starts/news 长度不一致
        let e2 = block_on_rt(edit_file(&serde_json::json!({"path": f.to_string_lossy(), "starts": [1, 5], "news": [""]}), &roots, "conv")).unwrap_err();
        assert!(e2.contains("长度必须一致"), "{e2}");
        // anchors 长度不一致
        let e3 = block_on_rt(edit_file(&serde_json::json!({"path": f.to_string_lossy(), "starts": [1], "news": [""], "anchors": []}), &roots, "conv")).unwrap_err();
        assert!(e3.contains("anchors"), "{e3}");
        // 同一块出现两次 → 重叠拒绝
        let e4 = block_on_rt(edit_file(&serde_json::json!({"path": f.to_string_lossy(), "starts": [1, 2], "news": ["", ""]}), &roots, "conv")).unwrap_err();
        assert!(e4.contains("重叠"), "{e4}");
        // anchors 校验失败（签名不存在）
        let e5 = block_on_rt(edit_file(&serde_json::json!({"path": f.to_string_lossy(), "starts": [1], "news": [""], "anchors": ["fn not_exist"]}), &roots, "conv")).unwrap_err();
        assert!(e5.contains("anchor 定位失败"), "{e5}");
        // 超界
        let e6 = block_on_rt(edit_file(&serde_json::json!({"path": f.to_string_lossy(), "starts": [99], "news": [""]}), &roots, "conv")).unwrap_err();
        assert!(e6.contains("超出文件总行数"), "{e6}");
        // 配平守卫：批量替换后失衡 → 拒绝
        let e7 = block_on_rt(edit_file(&serde_json::json!({"path": f.to_string_lossy(), "starts": [1], "news": ["fn a() {\n    x();\n"]}), &roots, "conv")).unwrap_err();
        assert!(e7.contains("配平") || e7.contains("失衡"), "{e7}");
        let after = std::fs::read_to_string(&f).unwrap();
        assert_eq!(after, src, "所有失败路径都不应改动文件");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edit_file_batch_dry_run_and_preview() {
        // dry_run 不落盘；preview_edit 批量同口径出 diff
        let dir = std::env::temp_dir().join(format!("ef_bdr_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let roots = vec![dir.to_string_lossy().to_string()];
        let f = dir.join("t.rs");
        let src = "fn a() {\n    x();\n}\nfn b() {\n    y();\n}\n";
        std::fs::write(&f, src).unwrap();
        let rd = serde_json::json!({"path": f.to_string_lossy()});
        block_on_rt(read_file(&rd, &roots)).expect("read ok");
        let args = serde_json::json!({"path": f.to_string_lossy(), "starts": [1, 4], "news": ["fn a() {\n    x2();\n}", "fn b() {\n    y2();\n}"], "dry_run": true});
        let out = block_on_rt(edit_file(&args, &roots, "conv")).expect("dry_run ok");
        assert!(out.contains("【dry_run 预览】") && out.contains("2 个块"), "{out}");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), src, "dry_run 不落盘");
        let pv = block_on_rt(preview_edit(&serde_json::json!({"path": f.to_string_lossy(), "starts": [1, 4], "news": ["fn a() {\n    x2();\n}", "fn b() {\n    y2();\n}"]}), &roots, "conv")).expect("preview ok");
        assert!(pv.contains("+") && pv.contains("-") || pv.contains("x2"), "diff 内容: {pv}");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), src, "preview 不落盘");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn grep_regex_ci_debug() {
        let r = regex::RegexBuilder::new("foo\\(").case_insensitive(true).build().unwrap();
        assert!(r.is_match("FOO("), "ci literal");
        let r2 = regex::RegexBuilder::new("foo\\s*\\(").case_insensitive(true).build().unwrap();
        assert!(r2.is_match("fn foo() {"));
    }

    #[test]
    fn grep_files_regex_mode() {
        let dir = std::env::temp_dir().join(format!("gp_re_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let roots = vec![dir.to_string_lossy().to_string()];
        std::fs::write(dir.join("a.rs"), "fn foo() {\n    call_foo (x);\n}\nlet v: Vec<String> = Vec::new();\n").unwrap();
        std::fs::write(dir.join("b.txt"), "foo (not code)\nplain text\n").unwrap();
        // 正则匹配：foo 后跟可选空格再 (（命中 .rs 的 L1/L2；.txt 也会命中行，先不 glob 过滤）
        let out = block_on_rt(grep_files(&serde_json::json!({"pattern": "foo\\s*\\(", "path": dir.to_string_lossy(), "glob": "*.rs", "regex": true}), &roots)).expect("regex grep ok");
        assert!(out.contains("fn foo()") && out.contains("call_foo (x)"), "两行命中: {out}");
        // 类型模式：Vec<\w+>
        let out2 = block_on_rt(grep_files(&serde_json::json!({"pattern": "Vec<\\w+>", "path": dir.to_string_lossy(), "glob": "*.rs", "regex": true}), &roots)).expect("regex grep 2 ok");
        assert!(out2.contains("Vec<String>"), "泛型命中: {out2}");
        // 大小写不敏感（缺省）：FOO 也命中
        std::fs::write(dir.join("c.rs"), "FOO(\n").unwrap();
        let out3 = block_on_rt(grep_files(&serde_json::json!({"pattern": "foo\\(", "path": dir.to_string_lossy(), "glob": "c.rs", "regex": true}), &roots)).expect("ci ok");
        assert!(out3.contains("FOO("), "大小写不敏感: {out3}");
        // case_sensitive=true 时不再命中
        let out4 = block_on_rt(grep_files(&serde_json::json!({"pattern": "foo\\(", "path": dir.to_string_lossy(), "glob": "c.rs", "regex": true, "case_sensitive": true}), &roots)).expect("cs ok");
        assert!(out4.contains("未找到"), "大小写敏感应无命中: {out4}");
        // 非法正则：明确报错并提示转义
        let e = block_on_rt(grep_files(&serde_json::json!({"pattern": "foo(", "path": dir.to_string_lossy(), "regex": true}), &roots)).unwrap_err();
        assert!(e.contains("正则表达式无效") && e.contains("双重转义"), "{e}");
        // 不传 regex：纯文本语义，foo\s*\( 找不到字面量
        let out5 = block_on_rt(grep_files(&serde_json::json!({"pattern": "foo\\s*\\(", "path": dir.to_string_lossy(), "glob": "*.rs"}), &roots)).expect("plain ok");
        assert!(out5.contains("未找到"), "纯文本模式不应正则命中: {out5}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn outline_block_range_bindex_matches_fallback() {
        // 快路径（BlockIndex O(1) 查询）与回退路径（逐次 find_matching_close）全定义行等价
        let src = "struct S {\n    x: u32,\n}\nimpl S {\n    fn a(&self) {\n        self.x();\n    }\n    fn b(\n        p: u32,\n    ) -> u32 {\n        p\n    }\n}\nfn top() {\n    z();\n}\n";
        let l: Vec<&str> = src.lines().collect();
        let bi = BlockIndex::build(&l, "rs");
        for i in 0..l.len() {
            assert_eq!(
                outline_block_range(&l, i, "rs", Some(&bi)),
                outline_block_range(&l, i, "rs", None),
                "定义行 {} 两条路径结果应一致",
                i + 1
            );
        }
    }

    #[test]
    fn outline_filter_and_hierarchy_indent() {
        // 类型过滤：filter="函数" 只显示 fn 条目并标注过滤信息
        let src = "struct S {\n    x: u32,\n}\nimpl S {\n    fn a(&self) {\n        g();\n    }\n}\nfn top() {\n    z();\n}\n";
        let l: Vec<&str> = src.lines().collect();
        let out = render_outline(std::path::Path::new("t.rs"), &l, 128, 1, Some("函数"));
        assert!(out.contains("已按类型过滤"), "过滤标注: {out}");
        assert!(out.contains("fn a") && out.contains("fn top"), "函数条目保留: {out}");
        assert!(!out.contains("struct S"), "类型条目被过滤: {out}");
        assert!(!out.contains("impl S"), "impl 被过滤: {out}");
        // 层级缩进：impl 内的 fn a 缩进 2 格，顶层 fn top 不缩进
        let full = render_outline(std::path::Path::new("t.rs"), &l, 128, 1, None);
        let fn_a_line = full.lines().find(|s| s.contains("fn a")).unwrap();
        let fn_top_line = full.lines().find(|s| s.contains("fn top")).unwrap();
        assert!(fn_a_line.contains("│   函数 fn a"), "类内方法应缩进: {fn_a_line}");
        assert!(fn_top_line.contains("│ 函数 fn top"), "顶层函数不缩进: {fn_top_line}");
        // Python 层级：类内 def 按缩进层级显示
        let pysrc = "class B:\n    def m(self):\n        return 1\n\ndef top():\n    pass\n";
        let pl: Vec<&str> = pysrc.lines().collect();
        let pyout = render_outline(std::path::Path::new("t.py"), &pl, 64, 1, None);
        let def_m = pyout.lines().find(|s| s.contains("def m")).unwrap();
        assert!(def_m.contains("│   函数 def m"), "py 类内方法应缩进: {def_m}");
    }

    #[test]
    fn block_single_line_inner_blocks() {
        // 单行内嵌块闭合时不得误判外层方法已结束（栈配对核心回归）
        let src = "fn a() {\n  if (x) { foo(); }\n  bar();\n}\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(find_matching_close(&l, 0, "rs"), Some(3)); // fn a 结束在第 3 行 }
        assert_eq!(find_enclosing_block(&l, 2, "rs"), Some((0, 3))); // bar(); 仍属 fn a
        assert_eq!(find_enclosing_block(&l, 1, "rs"), Some((0, 3)));
    }

    #[test]
    fn block_else_if_chain_is_one_construct() {
        let src = "if (a) {\n  x();\n} else if (b) {\n  y();\n} else {\n  z();\n}\n";
        let l: Vec<&str> = src.lines().collect();
        // 顶层 if-else-if-else：每个分支是独立完整块（各自成对括号）
        assert_eq!(find_enclosing_block(&l, 1, "ts"), Some((0, 2))); // if 分支
        assert_eq!(find_enclosing_block(&l, 3, "ts"), Some((2, 4))); // else-if 分支
        assert_eq!(find_enclosing_block(&l, 5, "ts"), Some((4, 6))); // else 分支
        assert_eq!(find_matching_close(&l, 0, "ts"), Some(2));
        // 若链在方法内 → 任意分支都归到整个方法（不会只补到分支块）
        let src2 = "fn a() {\n  if (a) {\n    x();\n  } else if (b) {\n    y();\n  }\n  z();\n}\n";
        let l2: Vec<&str> = src2.lines().collect();
        assert_eq!(find_enclosing_block(&l2, 4, "ts"), Some((0, 7)));
    }

    #[test]
    fn block_non_code_file_returns_none() {
        let src = "title\n\nplain text { not code } more\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(block_style("md"), BlockStyle::None);
        assert_eq!(find_enclosing_block(&l, 0, "md"), None);
        assert_eq!(block_style("json"), BlockStyle::None, "数据文件不做块操作");
    }

    // ---------- read_file 块补齐 ----------

    #[test]
    fn read_file_completes_method_block() {
        // 读取窗口结束位置在方法内部 → 自动补齐到方法结束符（整方法）
        let content = "fn a() {\n  let x = 1;\n}\nfn b() {\n  let y = 2;\n  let z = 3;\n}\nfn c() {\n  let w = 4;\n}\n";
        let (f, roots) = tmp_file("read_block", content, "rs");
        let rel = f.to_string_lossy().to_string();
        // start=5（fn b 内部）lines=2 → 上扩到 fn b 首行、补齐到结束符
        let args = serde_json::json!({"path": rel, "start": 5, "lines": 2});
        let out = block_on_rt(read_file(&args, &roots)).expect("读取应成功");
        assert!(out.contains("fn b() {"), "应上扩到方法首行: {out}");
        assert!(out.contains("let y = 2") && out.contains("let z = 3"), "应包含完整方法体: {out}");
        assert!(!out.contains("let w = 4"), "不应越界到方法 c: {out}");
        assert!(out.contains("自动对齐"), "应有块对齐提示: {out}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn read_file_plain_text_keeps_fixed_window() {
        let content = "line1\nline2\nline3\nline4\n";
        let (f, roots) = tmp_file("read_plain", content, "txt");
        let rel = f.to_string_lossy().to_string();
        let args = serde_json::json!({"path": rel, "start": 2, "lines": 2});
        let out = block_on_rt(read_file(&args, &roots)).unwrap();
        assert!(!out.contains("代码块"), "非代码文件不应做块对齐: {out}");
        assert!(out.contains("line2") && out.contains("line3"), "{out}");
        assert!(!out.contains("line1"), "应严格按 lines=2: {out}");
        assert!(out.contains("file_version=sha256:"), "应返回稳定文件版本: {out}");
        assert!(out.contains("窗口=L2-L3") && out.contains("next_start=4"), "应返回续读游标: {out}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn read_file_last_window_reports_terminal_cursor() {
        let (f, roots) = tmp_file("read_cursor_end", "line1\nline2\n", "txt");
        let args = serde_json::json!({
            "path": f.to_string_lossy(),
            "start": 2,
            "lines": 20
        });
        let out = block_on_rt(read_file(&args, &roots)).unwrap();
        assert!(out.contains("窗口=L2-L2"), "{out}");
        assert!(out.contains("next_start=end"), "{out}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn large_text_requires_and_supports_bounded_stream_window() {
        let mut content = String::with_capacity(1_200_000);
        for line in 1..=12_000 {
            content.push_str(&format!("line-{line:05}-{}\n", "x".repeat(90)));
        }
        let (f, roots) = tmp_file("read_large_stream", &content, "txt");
        assert!(std::fs::metadata(&f).unwrap().len() > 1024 * 1024);

        let without_window = block_on_rt(read_file(
            &serde_json::json!({"path": f.to_string_lossy()}),
            &roots,
        ))
        .unwrap_err();
        assert!(without_window.contains("显式传 start/lines"), "{without_window}");

        let out = block_on_rt(read_file(
            &serde_json::json!({"path": f.to_string_lossy(), "start": 10_000, "lines": 2}),
            &roots,
        ))
        .unwrap();
        assert!(out.contains("流式窗口=L10000-L10001"), "{out}");
        assert!(out.contains("line-10000") && out.contains("line-10001"), "{out}");
        assert!(!out.contains("line-09999"), "不应泄露窗口外内容: {out}");
        assert!(out.contains("next_start=10002"), "{out}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    // ---------- edit_file start 模式（整块替换/删除） ----------

    #[test]
    fn edit_block_mode_replaces_whole_method() {
        let content = "fn a() {\n  let x = 1;\n}\nfn b() {\n  let y = 2;\n  let z = 3;\n}\nfn c() {\n  let w = 4;\n}\n";
        let (f, roots) = tmp_file("edit_block", content, "rs");
        let rel = f.to_string_lossy().to_string();
        // 先读建立指纹基线（避免冲突保护拦截）
        block_on_rt(read_file(&serde_json::json!({"path": rel.clone()}), &roots)).unwrap();
        // start 定位方法 b 内部（第 5 行）→ 整块替换
        let args = serde_json::json!({"path": rel.clone(), "start": 5, "new": "fn b2() {\n  let q = 9;\n}\n"});
        let out = block_on_rt(edit_file(&args, &roots, "t_edit_block")).expect("块替换应成功");
        assert!(out.contains("完整代码块"), "{out}");
        assert!(out.contains("共 4 行"), "方法 b 应整体 4 行: {out}");
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains("fn b2()"), "应整块替换: {text}");
        assert!(!text.contains("let z = 3"), "旧方法体应消失: {text}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn edit_block_mode_deletes_whole_method() {
        let content = "fn a() {\n  let x = 1;\n}\nfn b() {\n  let y = 2;\n  let z = 3;\n}\nfn c() {\n  let w = 4;\n}\n";
        let (f, roots) = tmp_file("edit_del", content, "rs");
        let rel = f.to_string_lossy().to_string();
        block_on_rt(read_file(&serde_json::json!({"path": rel.clone()}), &roots)).unwrap();
        // start 定位 fn c 首行，new 为空 → 整块删除
        let args = serde_json::json!({"path": rel, "start": 8, "new": ""});
        let out = block_on_rt(edit_file(&args, &roots, "t_edit_del")).expect("块删除应成功");
        assert!(out.contains("删除"), "{out}");
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(!text.contains("fn c"), "方法 c 应被整块删除: {text}");
        assert!(text.contains("fn b"), "其余方法应保留: {text}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn edit_old_start_mutually_exclusive() {
        let content = "fn a() {\n  let x = 1;\n}\n";
        let (f, roots) = tmp_file("edit_mutex", content, "rs");
        let args = serde_json::json!({"path": f.to_string_lossy().to_string(), "old": "x", "start": 1});
        let err = block_on_rt(edit_file(&args, &roots, "t_edit_mutex")).unwrap_err();
        assert!(err.contains("互斥"), "{err}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    // ---------- grep_files block 模式 ----------

    #[test]
    fn grep_block_mode_shows_whole_method() {
        let content = "fn a() {\n  let x = 1;\n}\nfn b() {\n  let y = 2;\n}\n";
        let (f, roots) = tmp_file("grep_block", content, "rs");
        let args = serde_json::json!({"path": ".", "pattern": "let y", "block": true});
        let out = block_on_rt(grep_files(&args, &roots)).expect("搜索应成功");
        assert!(out.contains("完整代码块 L4-L6"), "应显示完整方法块: {out}");
        assert!(out.contains("let y = 2"), "{out}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn grep_plain_mode_keeps_single_line() {
        let content = "fn a() {\n  let x = 1;\n}\nfn b() {\n  let y = 2;\n}\n";
        let (f, roots) = tmp_file("grep_plain", content, "rs");
        let args = serde_json::json!({"path": ".", "pattern": "let y"});
        let out = block_on_rt(grep_files(&args, &roots)).expect("搜索应成功");
        assert!(out.contains("let y = 2"), "{out}");
        assert!(!out.contains("完整代码块"), "默认单行模式不应展开块: {out}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    // ---------- 注释识别 / 折叠清洗 ----------

    #[test]
    fn comment_line_detection() {
        assert!(is_comment_line("// x", "rs"));
        assert!(is_comment_line("  // x", "ts"));
        assert!(is_comment_line("/* x", "c"));
        assert!(is_comment_line(" * doc", "java"));
        assert!(is_comment_line("*/", "go"));
        assert!(is_comment_line("# x", "py"));
        assert!(is_comment_line("-- x", "sql"));
        assert!(!is_comment_line("let a = 1; // 行尾注释", "ts"), "行内注释不算整行注释");
        assert!(!is_comment_line("# 标题", "md"), "非代码语言不识别");
        assert!(!is_comment_line("// 字符串", "json"), "数据文件不识别");
    }

    #[test]
    fn read_file_folds_long_comment_blocks() {
        // 10 行头注释 + 3 行代码：注释占比 77% → 长注释块折叠，代码可见
        let content = [
                "// 文件头说明",
                "// 文件头说明",
                "// 文件头说明",
                "// 文件头说明",
                "// 文件头说明",
                "// 文件头说明",
                "// 文件头说明",
                "// 文件头说明",
                "// 文件头说明",
                "// 文件头说明",
                "fn a() {",
                "  let x = 1;",
                "}",
            ]
            .join("\n").to_string();
        let (f, roots) = tmp_file("read_fold", &content, "rs");
        let rel = f.to_string_lossy().to_string();
        let args = serde_json::json!({"path": rel});
        let out = block_on_rt(read_file(&args, &roots)).expect("读取应成功");
        assert!(out.contains("行注释已折叠"), "长注释块应折叠: {out}");
        assert!(out.contains("注释占 77%"), "应标注注释占比: {out}");
        assert!(out.contains("fn a() {") && out.contains("let x = 1"), "代码必须完整可见: {out}");
        assert!(!out.contains("文件头说明"), "折叠后不应再逐行展示注释: {out}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn read_file_keeps_short_comment_blocks() {
        // 3 行短注释块（< 8 行）即使注释占比高也保留原文
        let content = "// 简介\n// 简介2\n// 简介3\nfn a() {\n  let x = 1;\n}\n";
        let (f, roots) = tmp_file("read_keep", content, "rs");
        let rel = f.to_string_lossy().to_string();
        let args = serde_json::json!({"path": rel});
        let out = block_on_rt(read_file(&args, &roots)).expect("读取应成功");
        assert!(out.contains("简介2"), "短注释块应保留原文: {out}");
        assert!(!out.contains("行注释已折叠"), "短注释块不应折叠: {out}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn grep_marks_comment_hits() {
        let content = "// let y = 2\nfn b() {\n  let y = 2;\n}\n";
        let (f, roots) = tmp_file("grep_cmt", content, "rs");
        let args = serde_json::json!({"path": ".", "pattern": "let y"});
        let out = block_on_rt(grep_files(&args, &roots)).expect("搜索应成功");
        assert!(out.contains("[注释] // let y = 2"), "注释命中应标注: {out}");
        assert!(out.contains("let y = 2;"), "代码命中不应标注: {out}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn grep_respects_gitignore() {
        // .gitignore 忽略的目录不进入搜索结果
        let dir = std::env::temp_dir().join(format!("agent_gi_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(dir.join("ignored")).unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::write(dir.join("kept.txt"), "needle here\n").unwrap();
        std::fs::write(dir.join("ignored").join("secret.txt"), "needle here\n").unwrap();
        let roots = vec![dir.to_string_lossy().to_string()];
        let args = serde_json::json!({"path": ".", "pattern": "needle"});
        let out = block_on_rt(grep_files(&args, &roots)).expect("搜索应成功");
        assert!(out.contains("kept.txt"), "应命中保留文件: {out}");
        assert!(!out.contains("secret.txt"), "gitignore 目录不应命中: {out}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_dir_respects_gitignore_and_hidden() {
        // gitignore 目录 + 隐藏目录跳过；普通文件可见；★ 关键文件标注
        let dir = std::env::temp_dir().join(format!("agent_ld_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(dir.join("ignored")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::write(dir.join("keep.txt"), "x").unwrap();
        std::fs::write(dir.join("README.md"), "# t").unwrap();
        std::fs::write(dir.join("ignored").join("secret.txt"), "x").unwrap();
        let roots = vec![dir.to_string_lossy().to_string()];
        let args = serde_json::json!({"path": "."});
        let out = block_on_rt(list_dir(&args, &roots)).expect("浏览应成功");
        assert!(out.contains("keep.txt"), "普通文件应可见: {out}");
        assert!(out.contains("README.md"), "关键文件应可见: {out}");
        assert!(!out.contains("secret.txt") && !out.contains("ignored/"), "gitignore 目录应跳过: {out}");
        assert!(!out.contains(".hidden"), "隐藏目录应跳过: {out}");
        assert!(out.contains("跳过 2 个忽略项"), "应统计跳过数: {out}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
