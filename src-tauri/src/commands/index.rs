use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::db::DbState;

/// 文件树节点（相对路径 + 递归 children）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTreeNode {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub node_type: String, // dir | file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>, // 文件字节数（目录为 None）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<FileTreeNode>>,
}

/// 索引时排除的目录（体积大/无意义）
const EXCLUDED_DIRS: &[&str] = &[
    "node_modules", ".git", "build", ".hvigor", "oh_modules", ".idea", "dist", ".cxx",
    ".preview", ".test", ".ohpm", ".arkui-x", "coverage", ".venv",
];
const MAX_DEPTH: u32 = 12;
const MAX_NODES: usize = 30000;

/// 文本完整渲染最大字节数（≤此值完整返回，不截断）
const MAX_PREVIEW_BYTES: u64 = 5 * 1024 * 1024;

/// 文本/文档可预览最大字节数（超出拒绝；与 Agent read_document 工具一致）
const MAX_BIG_BYTES: u64 = 50 * 1024 * 1024;

/// 预览文本最大字符数（超出后保头保尾截断，防止超大文件拖垮渲染）
const MAX_TEXT_CHARS: usize = 200_000;

/// 预览结果：content 为渲染文本；truncated 表示是否因过大截断；
/// total_chars 为截断前总字符数（仅截断时有值，供前端提示）
#[derive(Debug, Clone, Serialize)]
pub struct PreviewResult {
    content: String,
    truncated: bool,
    total_chars: Option<usize>,
}

/// 保头保尾截断（用户视角文案）：前 60% 保留开头（标题/摘要），后 40% 保留结尾（结论/签名）
fn truncate_head_tail_user(s: &str, n: usize) -> String {
    let total = s.chars().count();
    if total <= n {
        return s.to_string();
    }
    let head = n * 3 / 5;
    let tail = n - head;
    let mut out: String = s.chars().take(head).collect();
    out.push_str(&format!(
        "\n\n……（内容过长：中间 {} 字符已省略，共 {total} 字符，仅展示开头与结尾）\n\n",
        total - head - tail
    ));
    out.push_str(&s.chars().skip(total - tail).collect::<String>());
    out
}

/// 索引进度发射器：节流地向前端推送已扫描项数，供进度展示
struct IndexProgress {
    app: AppHandle,
    project_id: String,
    last_emit: usize,
    last_time: std::time::Instant,
}

impl IndexProgress {
    /// 每 200 项或每 200ms 发一次进度（避免高频事件刷屏）
    fn tick(&mut self, count: usize) {
        let by_count = count - self.last_emit >= 200;
        let by_time = self.last_time.elapsed().as_millis() >= 200;
        if count > 0 && (by_count || by_time) {
            let _ = self.app.emit("file-tree-index-progress", serde_json::json!({
                "projectId": self.project_id,
                "scanned": count,
            }));
            self.last_emit = count;
            self.last_time = std::time::Instant::now();
        }
    }

    fn done(&self, count: usize) {
        let _ = self.app.emit("file-tree-index-done", serde_json::json!({
            "projectId": self.project_id,
            "scanned": count,
        }));
    }
}

/// 递归扫描目录，生成文件树（按 目录→文件、名称 排序）
fn scan_dir(
    dir: &Path,
    rel: &str,
    depth: u32,
    count: &mut usize,
    progress: &mut IndexProgress,
) -> Option<FileTreeNode> {
    if depth > MAX_DEPTH || *count >= MAX_NODES {
        return None;
    }
    let mut children = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(dir).ok()?.flatten().collect();
    entries.sort_by_key(|e| (e.path().is_file(), e.file_name()));

    for entry in entries {
        if *count >= MAX_NODES {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.path().is_dir();
        if is_dir {
            if EXCLUDED_DIRS.contains(&name.as_str()) {
                continue;
            }
            let child_rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if let Some(node) = scan_dir(&entry.path(), &child_rel, depth + 1, count, progress) {
                children.push(node);
            }
        } else {
            *count += 1;
            progress.tick(*count);
            let child_rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let size = entry.metadata().ok().map(|m| m.len());
            children.push(FileTreeNode {
                name,
                path: child_rel,
                node_type: "file".into(),
                size,
                children: None,
            });
        }
    }

    *count += 1;
    progress.tick(*count);
    let root_name = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| rel.to_string());
    Some(FileTreeNode {
        name: root_name,
        path: rel.to_string(),
        node_type: "dir".into(),
        size: None,
        children: Some(children),
    })
}

/// 获取项目的基本信息（路径）
fn get_project_path(conn: &rusqlite::Connection, project_id: &str) -> Result<String, String> {
    conn.query_row(
        "SELECT path FROM projects WHERE id = ?1",
        [project_id],
        |r| Ok(crate::utils::path::normalize_path(&r.get::<_, String>(0)?)),
    )
    .map_err(|e| e.to_string())
}

/// 解析项目工作目录：root 覆盖（worktree 会话）优先，否则回退到项目主路径。
/// 各文件树/预览/符号命令据此在「主仓库 or worktree」之间切换。
fn resolve_root(
    conn: &rusqlite::Connection,
    project_id: &str,
    root: Option<&str>,
) -> Result<String, String> {
    if let Some(r) = root.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        return Ok(crate::utils::path::normalize_path(r));
    }
    get_project_path(conn, project_id)
}

/// 将项目目录注册为 asset protocol 可访问范围（供前端 convertFileSrc 预览本地媒体）
fn register_asset_scope(app: &AppHandle, project_path: &Path) -> Result<(), String> {
    app.asset_protocol_scope()
        .allow_directory(project_path, true)
        .map_err(|e| format!("注册资源访问范围失败: {e}"))
}

/// 构建（或重建）项目文件树索引，存入缓存并更新 index_state。
/// 异步命令：全量递归扫描在 blocking 线程池执行（大项目/含大量二进制目录可能数秒），
/// 不阻塞 IPC 主循环——同步 scan_dir 会整窗冻结数秒。
#[tauri::command]
pub async fn build_project_index(
    project_id: String,
    root: Option<String>,
    state: State<'_, DbState>,
    app: AppHandle,
) -> Result<FileTreeNode, String> {
    let (main_path, project_path) = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        let main = get_project_path(&conn, &project_id)?;
        let eff = resolve_root(&conn, &project_id, root.as_deref())?;
        (main, eff)
    };
    if project_path.is_empty() {
        return Err("全局项目没有文件目录".into());
    }
    let root = Path::new(&project_path);
    if !root.is_dir() {
        return Err("项目目录不存在".into());
    }
    register_asset_scope(&app, root)?;

    {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE projects SET index_state = 'building' WHERE id = ?1",
            [&project_id],
        )
        .map_err(|e| e.to_string())?;
    }
    // 重建索引 = 目录结构可能已变化：清空文件名检索快照，避免搜索读到过期列表
    invalidate_file_search_cache();

    // 全量递归扫描（每个文件多次 metadata 系统调用）在 blocking 线程池执行，避免阻塞主线程
    let scan_root = root.to_path_buf();
    let app_for_progress = app.clone();
    let pid_for_progress = project_id.clone();
    let tree = tauri::async_runtime::spawn_blocking(move || {
        let mut count = 0usize;
        let mut progress = IndexProgress {
            app: app_for_progress,
            project_id: pid_for_progress,
            last_emit: 0,
            last_time: std::time::Instant::now(),
        };
        let result = scan_dir(&scan_root, "", 0, &mut count, &mut progress);
        // 无论成败都发 done，前端据此清除进度指示，避免失败时进度卡在最后值
        progress.done(count);
        result.ok_or_else(|| "索引超限或目录不可读".to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    let json = serde_json::to_string(&tree).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();

    let conn = state.0.lock().map_err(|e| e.to_string())?;
    // 仅主仓库写入项目级文件树缓存（worktree 会话的全量树不落库，避免与主仓库互相覆盖）
    if project_path == main_path {
        conn.execute(
            "INSERT OR REPLACE INTO project_index_cache (project_id, kind, data_json, built_at)
             VALUES (?1, 'filetree', ?2, ?3)",
            rusqlite::params![project_id, json, now],
        )
        .map_err(|e| e.to_string())?;
    }
    conn.execute(
        "UPDATE projects SET index_state = 'ready' WHERE id = ?1",
        [&project_id],
    )
    .map_err(|e| e.to_string())?;

    Ok(tree)
}

/// 读取项目文件树缓存（未构建时返回 None）
#[tauri::command]
pub async fn get_project_file_tree(
    project_id: String,
    state: State<'_, DbState>,
    app: AppHandle,
) -> Result<Option<FileTreeNode>, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        get_project_path(&conn, &project_id)?
    };
    if !project_path.is_empty() {
        register_asset_scope(&app, Path::new(&project_path))?;
    }
    let row: Option<String> = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT data_json FROM project_index_cache
             WHERE project_id = ?1 AND kind = 'filetree'",
            [&project_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
    };

    // 反序列化整棵文件树 JSON 在 blocking 线程池执行，且不再持有 DB 锁
    match row {
        Some(json) => tauri::async_runtime::spawn_blocking(move || {
            serde_json::from_str(&json).map(Some).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?,
        None => Ok(None),
    }
}

/// 读取单层目录内容（文件树懒加载：先读根目录，展开时逐级按需请求，无深度/数量上限）
/// 安全约束：仅允许项目目录内的相对路径（防路径穿越）；path 为空表示项目根。
#[tauri::command]
pub async fn list_project_dir(
    project_id: String,
    path: String,
    root: Option<String>,
    state: State<'_, DbState>,
    app: AppHandle,
) -> Result<Vec<FileTreeNode>, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        resolve_root(&conn, &project_id, root.as_deref())?
    };
    if project_path.is_empty() {
        return Err("全局项目没有文件目录".into());
    }
    let root = Path::new(&project_path);
    if !root.is_dir() {
        return Err("项目目录不存在".into());
    }
    register_asset_scope(&app, root)?;

    // 路径安全：仅相对路径、拒绝 .. 等越界组件
    let rel = Path::new(&path);
    if !path.is_empty() {
        if rel.is_absolute() {
            return Err("仅支持项目内相对路径".into());
        }
        if rel.components()
            .any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
        {
            return Err("路径越界".into());
        }
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("项目目录不可访问: {e}"))?;
    let target = if path.is_empty() {
        canonical_root.clone()
    } else {
        root.join(rel)
            .canonicalize()
            .map_err(|e| format!("目录不可读: {e}"))?
    };
    if !target.starts_with(&canonical_root) {
        return Err("路径越界".into());
    }
    if !target.is_dir() {
        return Err("目标不是目录".into());
    }

    // 扫描单层（目录→文件、名称排序；目录 children 留空由前端按需加载）
    // 在 blocking 线程池执行（每项 is_dir/metadata 系统调用），避免阻塞 IPC 主循环
    tauri::async_runtime::spawn_blocking(move || {
        let mut children = Vec::new();
        let mut entries: Vec<_> = fs::read_dir(&target)
            .map_err(|e| format!("读取目录失败: {e}"))?
            .flatten()
            .collect();
        entries.sort_by_key(|e| (e.path().is_file(), e.file_name()));
        for entry in entries {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.path().is_dir();
            let child_rel = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}/{name}")
            };
            if is_dir {
                if EXCLUDED_DIRS.contains(&name.as_str()) {
                    continue;
                }
                children.push(FileTreeNode {
                    name,
                    path: child_rel,
                    node_type: "dir".into(),
                    size: None,
                    children: None,
                });
            } else {
                let size = entry.metadata().ok().map(|m| m.len());
                children.push(FileTreeNode {
                    name,
                    path: child_rel,
                    node_type: "file".into(),
                    size,
                    children: None,
                });
            }
        }
        Ok(children)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 文件搜索结果项（仅文件名匹配，目录名不参与匹配）
#[derive(Debug, Clone, Serialize)]
pub struct FileSearchHit {
    pub name: String,
    /// 相对项目根的路径（正斜杠）
    pub path: String,
}

/// 文件名检索的目录快照缓存：首次搜索全量遍历后缓存扁平文件列表，
/// TTL 内后续搜索直接内存过滤（万级文件子串过滤 <1ms），避免每次击键重复全盘扫描。
/// 快照上限 MAX_NODES，超出部分不参与搜索（与文件树索引同口径）。
struct FileSearchSnapshot {
    /// 规范化后的项目根（主仓库或 worktree）
    key: String,
    scanned_at: std::time::Instant,
    files: Vec<FileSearchHit>,
}

/// 目录快照 TTL：文件增删改后最多延迟这么久可见（重建索引会立即清空）
const FILE_SEARCH_TTL: std::time::Duration = std::time::Duration::from_secs(10);

static FILE_SEARCH_CACHE: Mutex<Option<FileSearchSnapshot>> = Mutex::new(None);

/// 清空文件名检索快照缓存（文件树重建索引/刷新时调用）
fn invalidate_file_search_cache() {
    if let Ok(mut guard) = FILE_SEARCH_CACHE.lock() {
        *guard = None;
    }
}

/// 递归收集目录下全部文件名（仅文件；跳过排除目录；深度/数量上限与文件树索引一致）。
/// 使用 DirEntry::file_type() 判断目录（Windows 上来自 FindFirstFile 枚举数据，
/// 无额外系统调用），不做逐文件 metadata，大目录下显著快于 is_dir() + metadata()。
fn collect_file_names(dir: &Path, rel: &str, depth: u32, out: &mut Vec<FileSearchHit>) {
    if depth > MAX_DEPTH || out.len() >= MAX_NODES {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if out.len() >= MAX_NODES {
            return;
        }
        let Ok(ft) = entry.file_type() else { continue };
        let name = entry.file_name().to_string_lossy().to_string();
        if ft.is_dir() {
            if EXCLUDED_DIRS.contains(&name.as_str()) {
                continue;
            }
            let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            collect_file_names(&entry.path(), &child_rel, depth + 1, out);
        } else if ft.is_file() {
            let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            out.push(FileSearchHit { name, path: child_rel });
        }
    }
}

/// 内存过滤 + 排序：浅层目录优先，同层按路径字典序，截断到 limit
fn filter_and_rank(files: &[FileSearchHit], qlower: &str, limit: usize) -> Vec<FileSearchHit> {
    let mut hits: Vec<FileSearchHit> = files
        .iter()
        .filter(|f| f.name.to_lowercase().contains(qlower))
        .cloned()
        .collect();
    hits.sort_by(|a, b| {
        let da = a.path.matches('/').count();
        let db = b.path.matches('/').count();
        da.cmp(&db).then(a.path.cmp(&b.path))
    });
    hits.truncate(limit);
    hits
}

/// 按文件名（不含目录路径）做不区分大小写子串搜索，返回最多 limit 条。
/// 性能：目录快照缓存（10s TTL）+ 内存过滤；冷启动首次搜索在 blocking 线程池全量遍历
/// （DirEntry::file_type 免 metadata 调用），不阻塞 IPC 主循环。
#[tauri::command]
pub async fn search_project_files(
    project_id: String,
    query: String,
    root: Option<String>,
    limit: Option<usize>,
    state: State<'_, DbState>,
) -> Result<Vec<FileSearchHit>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        resolve_root(&conn, &project_id, root.as_deref())?
    };
    if project_path.is_empty() {
        return Err("全局项目没有文件目录".into());
    }
    let limit = limit.unwrap_or(200).clamp(1, 1000);
    let qlower = q.to_lowercase();

    // 快照缓存命中：直接内存过滤，秒回
    if let Ok(guard) = FILE_SEARCH_CACHE.lock() {
        if let Some(snap) = guard.as_ref() {
            if snap.key == project_path && snap.scanned_at.elapsed() <= FILE_SEARCH_TTL {
                return Ok(filter_and_rank(&snap.files, &qlower, limit));
            }
        }
    }

    // 缓存未命中：全量遍历构建快照（blocking 线程池，不阻塞 IPC 主循环）
    let scan_root = PathBuf::from(&project_path);
    let snapshot = tauri::async_runtime::spawn_blocking(move || -> Result<FileSearchSnapshot, String> {
        if !scan_root.is_dir() {
            return Err("项目目录不存在".into());
        }
        let mut files = Vec::new();
        collect_file_names(&scan_root, "", 0, &mut files);
        Ok(FileSearchSnapshot {
            key: project_path,
            scanned_at: std::time::Instant::now(),
            files,
        })
    })
    .await
    .map_err(|e| e.to_string())??;

    let result = filter_and_rank(&snapshot.files, &qlower, limit);
    if let Ok(mut guard) = FILE_SEARCH_CACHE.lock() {
        *guard = Some(snapshot);
    }
    Ok(result)
}

/// 读取项目内文件内容供预览面板使用：文本（UTF-8/GBK 自适应）完整或智能截断返回，
/// office/pdf 文档解析为纯文本。安全约束：仅允许项目目录内的相对路径（防路径穿越）。
#[tauri::command]
pub async fn read_project_file(
    project_id: String,
    path: String,
    root: Option<String>,
    state: State<'_, DbState>,
    app: AppHandle,
) -> Result<PreviewResult, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        resolve_root(&conn, &project_id, root.as_deref())?
    };
    if project_path.is_empty() {
        return Err("全局项目没有文件目录".into());
    }
    let rel = Path::new(&path);
    if path.is_empty() || rel.is_absolute() {
        return Err("仅支持项目内相对路径".into());
    }
    // 拒绝 .. 等越界组件
    if rel.components().any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_))) {
        return Err("路径越界".into());
    }
    let root = Path::new(&project_path);
    register_asset_scope(&app, root)?;
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("项目目录不可访问: {e}"))?;
    let canonical = root
        .join(rel)
        .canonicalize()
        .map_err(|e| format!("文件不可读: {e}"))?;
    if !canonical.starts_with(&canonical_root) {
        return Err("路径越界".into());
    }
    if !canonical.is_file() {
        return Err("目标不是文件".into());
    }
    let len = canonical
        .metadata()
        .map_err(|e| e.to_string())?
        .len();
    // office/pdf 文档走专用解析（提取为纯文本），其余按文本读取（UTF-8 → GBK 回退）
    let ext = canonical
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    let is_doc = matches!(ext.as_str(), "docx" | "pptx" | "xlsx" | "pdf");
    if len > MAX_BIG_BYTES {
        return Err(format!(
            "文件过大（{:.1}MB）：预览仅支持 50MB 以内的{}文件，可用 Agent 工具读取或改用外部编辑器打开",
            len as f64 / 1024.0 / 1024.0,
            if is_doc { "文档" } else { "文本" },
        ));
    }
    // office/pdf 文档解析 / 大文件读取在 blocking 线程池执行，避免阻塞 IPC 主循环
    let canonical_for_read = canonical.clone();
    let raw = tauri::async_runtime::spawn_blocking(move || {
        if is_doc {
            crate::agent::tools::doc_tools::extract_document_text(&canonical_for_read)
        } else {
            crate::agent::tools::doc_tools::read_text_doc(&canonical_for_read)
        }
    })
    .await
    .map_err(|e| e.to_string())??;
    // 文本 ≤5MB 完整返回；大文本/文档超 20 万字符时保头保尾截断（开头标题 + 结尾结论/签名）
    let total = raw.chars().count();
    if (!is_doc && len <= MAX_PREVIEW_BYTES) || total <= MAX_TEXT_CHARS {
        return Ok(PreviewResult {
            content: raw,
            truncated: false,
            total_chars: None,
        });
    }
    Ok(PreviewResult {
        content: truncate_head_tail_user(&raw, MAX_TEXT_CHARS),
        truncated: true,
        total_chars: Some(total),
    })
}

/// 解析项目内文件绝对路径（校验：相对 + 无越界组件 + canonical 根内 + 是文件）。
/// 返回 (文件绝对路径, 项目根绝对路径)，供下载/删除命令复用 read_project_file 的安全口径。
fn resolve_project_file(
    project_id: &str,
    path: &str,
    root: Option<&str>,
    state: &State<DbState>,
) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        resolve_root(&conn, project_id, root)?
    };
    if project_path.is_empty() {
        return Err("全局项目没有文件目录".into());
    }
    let rel = Path::new(path);
    if path.is_empty() || rel.is_absolute() {
        return Err("仅支持项目内相对路径".into());
    }
    // 拒绝 .. 等越界组件
    if rel.components().any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_))) {
        return Err("路径越界".into());
    }
    let root = Path::new(&project_path);
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("项目目录不可访问: {e}"))?;
    let canonical = root
        .join(rel)
        .canonicalize()
        .map_err(|e| format!("文件不可访问: {e}"))?;
    if !canonical.starts_with(&canonical_root) {
        return Err("路径越界".into());
    }
    if !canonical.is_file() {
        return Err("目标不是文件".into());
    }
    Ok((canonical, canonical_root))
}

/// 预览窗口下载：把项目内文件复制到用户选择的保存位置（dest 为 dialog.save 结果）。
/// 返回复制字节数。安全约束与 read_project_file 一致（仅项目内相对路径）。
#[tauri::command]
pub async fn save_project_file(
    project_id: String,
    path: String,
    dest: String,
    root: Option<String>,
    state: State<'_, DbState>,
) -> Result<u64, String> {
    let (src, _) = resolve_project_file(&project_id, &path, root.as_deref(), &state)?;
    if dest.trim().is_empty() {
        return Err("未指定保存位置".into());
    }
    let dest_path = Path::new(&dest).to_path_buf();
    if let Some(parent) = dest_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| format!("创建保存目录失败: {e}"))?;
        }
    }
    // 大文件复制在 blocking 线程池执行，避免阻塞 IPC 主循环
    tauri::async_runtime::spawn_blocking(move || {
        fs::copy(&src, &dest_path).map_err(|e| format!("保存文件失败: {e}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 预览窗口删除：把项目内文件移入系统回收站（Windows 回收站 / macOS 废纸篓，可恢复）。
/// 非 win/mac 平台回退：移到项目内 .deveco-agent/trash（与 Agent delete_file 同口径）。
/// 安全约束：仅项目内文件；项目根与保护目录天然不可达（文件树索引已排除）。
/// 异步命令：回收站/移动操作在 blocking 线程池执行，避免阻塞 IPC 主循环。
#[tauri::command]
pub async fn delete_project_file(
    project_id: String,
    path: String,
    root: Option<String>,
    state: State<'_, DbState>,
) -> Result<String, String> {
    // 非 win/mac 平台需要项目根拼回收目录；win/mac 直接进系统回收站
    #[cfg(not(any(windows, target_os = "macos")))]
    let (src, canonical_root) = resolve_project_file(&project_id, &path, root.as_deref(), &state)?;
    #[cfg(any(windows, target_os = "macos"))]
    let (src, _) = resolve_project_file(&project_id, &path, root.as_deref(), &state)?;

    #[cfg(any(windows, target_os = "macos"))]
    {
        let msg = format!("已移入系统回收站（可恢复）：{}", src.display());
        tauri::async_runtime::spawn_blocking(move || {
            trash::delete(&src).map_err(|e| format!("移入系统回收站失败: {e}"))
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok(msg)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        // 无系统回收站的平台：移到项目内回收目录，保证可恢复
        let trash_root = canonical_root.join(".deveco-agent").join("trash");
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let name = src.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        let dest = trash_root.join(ts.to_string()).join(&name);
        let parent = dest.parent().map(|p| p.to_path_buf());
        let msg = format!("已移至项目回收站（可恢复）：{}", dest.display());
        tauri::async_runtime::spawn_blocking(move || {
            if let Some(parent) = parent {
                fs::create_dir_all(&parent).map_err(|e| format!("创建回收目录失败: {e}"))?;
            }
            fs::rename(&src, &dest).map_err(|e| format!("移入回收目录失败: {e}"))?;
            Ok::<(), String>(())
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok(msg)
    }
}

/// 扫描项目符号（组件/类/函数/路由等），返回全部符号定义（含文件与行号）。
/// 供前端符号跳转面板与 Agent 工具使用。
/// 异步命令：扫描在 blocking 线程池执行，首次全量扫描（可能数秒）不阻塞 IPC 主循环，
/// 前端切换 Tab/其它操作不受影响。
#[tauri::command]
pub async fn index_project_symbols(
    project_id: String,
    root: Option<String>,
    state: State<'_, DbState>,
) -> Result<Vec<crate::services::symbol_index::Symbol>, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        resolve_root(&conn, &project_id, root.as_deref())?
    };
    if project_path.is_empty() {
        return Err("全局项目没有文件目录".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let root = Path::new(&project_path);
        if !root.is_dir() {
            return Err("项目目录不存在".into());
        }
        Ok(crate::services::symbol_index::index_project_cached(root))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 强制刷新项目符号索引：失效内存缓存后重新全量扫描。
#[tauri::command]
pub async fn refresh_project_symbols(
    project_id: String,
    root: Option<String>,
    state: State<'_, DbState>,
) -> Result<Vec<crate::services::symbol_index::Symbol>, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        resolve_root(&conn, &project_id, root.as_deref())?
    };
    if project_path.is_empty() {
        return Err("全局项目没有文件目录".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let root = Path::new(&project_path);
        if !root.is_dir() {
            return Err("项目目录不存在".into());
        }
        crate::services::symbol_index::invalidate_cache(root);
        Ok(crate::services::symbol_index::index_project_cached(root))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 项目结构摘要：组件清单、页面路由、符号总数。帮助 Agent 快速建立工程心智模型。
#[tauri::command]
pub async fn project_outline(
    project_id: String,
    root: Option<String>,
    state: State<'_, DbState>,
) -> Result<crate::services::symbol_index::ProjectOutline, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        resolve_root(&conn, &project_id, root.as_deref())?
    };
    if project_path.is_empty() {
        return Err("全局项目没有文件目录".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let root = Path::new(&project_path);
        if !root.is_dir() {
            return Err("项目目录不存在".into());
        }
        Ok(crate::services::symbol_index::build_outline(root))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 检索符号：按关键字（符号名/文件路径）和可选类型过滤，最多 200 条。
#[tauri::command]
pub async fn search_symbols(
    project_id: String,
    query: String,
    kind: Option<String>,
    root: Option<String>,
    state: State<'_, DbState>,
) -> Result<Vec<crate::services::symbol_index::Symbol>, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        resolve_root(&conn, &project_id, root.as_deref())?
    };
    if project_path.is_empty() {
        return Err("全局项目没有文件目录".into());
    }
    // 检索在 blocking 线程池执行（首次可能触发全量扫描），不阻塞 IPC 主循环
    tauri::async_runtime::spawn_blocking(move || {
        let root = Path::new(&project_path);
        if !root.is_dir() {
            return Err("项目目录不存在".into());
        }
        let syms = crate::services::symbol_index::index_project_cached(root);
        let found = crate::services::symbol_index::filter_symbols(&syms, &query, kind.as_deref());
        Ok(found.into_iter().cloned().collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 跨项目检索符号：遍历所有项目目录（上限 8 个），每个项目最多返回 60 条，附项目名。
/// 供符号面板「全部项目」范围使用；query 为空时直接返回空列表。
#[derive(Debug, Clone, Serialize)]
pub struct CrossProjectSymbol {
    #[serde(flatten)]
    pub sym: crate::services::symbol_index::Symbol,
    pub project_name: String,
}

#[tauri::command]
pub async fn search_symbols_all(
    query: String,
    kind: Option<String>,
    state: State<'_, DbState>,
) -> Result<Vec<CrossProjectSymbol>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let projects: Vec<(String, String)> = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT name, path FROM projects ORDER BY name")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        rows.filter_map(|x| x.ok()).collect()
    };
    // 并行扫描各项目（每项目走独立磁盘/内存缓存，互不阻塞）：
    // 首次冷启动多个项目同时全量扫描时收益最大。整体在 blocking 线程池执行，不阻塞 IPC。
    tauri::async_runtime::spawn_blocking(move || {
        let mut out: Vec<CrossProjectSymbol> = Vec::new();
        std::thread::scope(|s| {
            let mut handles = Vec::new();
            for (name, path) in projects.into_iter().take(8) {
                let p = crate::utils::path::normalize_path(&path);
                if p.is_empty() {
                    continue;
                }
                let root = std::path::PathBuf::from(p);
                if !root.is_dir() {
                    continue;
                }
                let q = query.clone();
                let k = kind.clone();
                handles.push(s.spawn(move || -> (String, Vec<crate::services::symbol_index::Symbol>) {
                    let syms = crate::services::symbol_index::index_project_cached(&root);
                    let found = crate::services::symbol_index::filter_symbols(&syms, &q, k.as_deref());
                    (name, found.into_iter().take(60).cloned().collect())
                }));
            }
            for h in handles {
                if let Ok((name, syms)) = h.join() {
                    for s in syms {
                        out.push(CrossProjectSymbol { sym: s, project_name: name.clone() });
                    }
                }
            }
        });
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 后台预热符号索引：磁盘缓存命中 + 增量校正，不返回符号列表。
/// 供前端在项目加载后调用，让符号面板与首轮对话构建概要时秒出结果。
#[tauri::command]
pub async fn warmup_symbol_index(
    project_id: String,
    root: Option<String>,
    state: State<'_, DbState>,
) -> Result<(), String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        resolve_root(&conn, &project_id, root.as_deref())?
    };
    if project_path.is_empty() || !Path::new(&project_path).is_dir() {
        // 全局项目/目录不存在：静默跳过，不算错误
        return Ok(());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let _ = crate::services::symbol_index::index_project_cached(Path::new(&project_path));
    })
    .await
    .map_err(|e| e.to_string())
}

/// 符号索引元信息：符号/文件数量与数据来源（供面板展示缓存状态）
#[tauri::command]
pub async fn symbol_index_meta(
    project_id: String,
    root: Option<String>,
    state: State<'_, DbState>,
) -> Result<crate::services::symbol_index::SymbolIndexMeta, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        resolve_root(&conn, &project_id, root.as_deref())?
    };
    if project_path.is_empty() {
        return Err("全局项目没有文件目录".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let root = Path::new(&project_path);
        if !root.is_dir() {
            return Err("项目目录不存在".into());
        }
        Ok(crate::services::symbol_index::index_meta(root))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 文件级符号数量（供文件树面板徽标展示）：返回 (相对路径, 符号数) 列表
#[tauri::command]
pub async fn symbol_counts(
    project_id: String,
    root: Option<String>,
    state: State<'_, DbState>,
) -> Result<Vec<crate::services::symbol_index::SymbolCount>, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        resolve_root(&conn, &project_id, root.as_deref())?
    };
    if project_path.is_empty() {
        return Err("全局项目没有文件目录".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let root = Path::new(&project_path);
        if !root.is_dir() {
            return Err("项目目录不存在".into());
        }
        Ok(crate::services::symbol_index::symbol_counts(root))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static DIR_SEQ: AtomicU32 = AtomicU32::new(0);

    /// 并行测试共用临时目录时用原子序号唯一化
    fn tmp_project() -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "deveco-switch-search-test-{}-{}",
            std::process::id(),
            DIR_SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn collects_and_filters_by_filename_only() {
        let root = tmp_project();
        // 目录名含关键字不参与匹配；node_modules 被排除；大小写不敏感
        std::fs::create_dir_all(root.join("src/PageModel")).unwrap();
        std::fs::write(root.join("src/App.ets"), "").unwrap();
        std::fs::write(root.join("src/PageModel/Model.ets"), "").unwrap();
        std::fs::write(root.join("src/page.ts"), "").unwrap();
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        std::fs::write(root.join("node_modules/Page.js"), "").unwrap();

        let mut files = Vec::new();
        collect_file_names(&root, "", 0, &mut files);

        let mut names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["App.ets", "Model.ets", "page.ts"]);

        // "page" 仅命中文件名 page.ts（PageModel 目录与 node_modules 内的文件都不算）
        let hits = filter_and_rank(&files, "page", 50);
        let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(paths, vec!["src/page.ts"]);

        // 同深度按路径字典序
        let hits = filter_and_rank(&files, "ets", 50);
        let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(paths, vec!["src/App.ets", "src/PageModel/Model.ets"]);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rank_shallow_first_and_limit() {
        let root = tmp_project();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        for i in 0..5 {
            std::fs::write(root.join(format!("x{i}.ts")), "").unwrap();
            std::fs::write(root.join(format!("a/x{i}.ts")), "").unwrap();
            std::fs::write(root.join(format!("a/b/x{i}.ts")), "").unwrap();
        }
        let mut files = Vec::new();
        collect_file_names(&root, "", 0, &mut files);
        let hits = filter_and_rank(&files, "x", 3);
        assert_eq!(hits.len(), 3);
        // 截断后前三条应全部来自根目录（深度 0，浅层优先）
        assert!(hits.iter().all(|h| !h.path.contains('/')));

        // 不设上限时 15 条全量返回
        let all = filter_and_rank(&files, "x", 1000);
        assert_eq!(all.len(), 15);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn empty_query_or_no_match_returns_empty() {
        let root = tmp_project();
        std::fs::write(root.join("main.ts"), "").unwrap();
        let mut files = Vec::new();
        collect_file_names(&root, "", 0, &mut files);
        assert!(filter_and_rank(&files, "zzz_not_exist", 50).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }
}

