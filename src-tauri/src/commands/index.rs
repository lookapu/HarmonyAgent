use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path};
use tauri::{AppHandle, Manager, State};

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

/// 文本预览最大字节数（超出提示文件过大）
const MAX_PREVIEW_BYTES: u64 = 5 * 1024 * 1024;

/// 递归扫描目录，生成文件树（按 目录→文件、名称 排序）
fn scan_dir(dir: &Path, rel: &str, depth: u32, count: &mut usize) -> Option<FileTreeNode> {
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
            if let Some(node) = scan_dir(&entry.path(), &child_rel, depth + 1, count) {
                children.push(node);
            }
        } else {
            *count += 1;
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

/// 将项目目录注册为 asset protocol 可访问范围（供前端 convertFileSrc 预览本地媒体）
fn register_asset_scope(app: &AppHandle, project_path: &Path) -> Result<(), String> {
    app.asset_protocol_scope()
        .allow_directory(project_path, true)
        .map_err(|e| format!("注册资源访问范围失败: {e}"))
}

/// 构建（或重建）项目文件树索引，存入缓存并更新 index_state
#[tauri::command]
pub fn build_project_index(
    project_id: String,
    state: State<DbState>,
    app: AppHandle,
) -> Result<FileTreeNode, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        get_project_path(&conn, &project_id)?
    };
    if project_path.is_empty() {
        return Err("全局项目没有文件目录".into());
    }
    let root = Path::new(&project_path);
    if !root.is_dir() {
        return Err("项目目录不存在".into());
    }
    register_asset_scope(&app, root)?;

    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE projects SET index_state = 'building' WHERE id = ?1",
        [&project_id],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);

    let mut count = 0usize;
    let tree = scan_dir(root, "", 0, &mut count).ok_or_else(|| "索引超限或目录不可读".to_string())?;
    let json = serde_json::to_string(&tree).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();

    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO project_index_cache (project_id, kind, data_json, built_at)
         VALUES (?1, 'filetree', ?2, ?3)",
        rusqlite::params![project_id, json, now],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE projects SET index_state = 'ready' WHERE id = ?1",
        [&project_id],
    )
    .map_err(|e| e.to_string())?;

    Ok(tree)
}

/// 读取项目文件树缓存（未构建时返回 None）
#[tauri::command]
pub fn get_project_file_tree(
    project_id: String,
    state: State<DbState>,
    app: AppHandle,
) -> Result<Option<FileTreeNode>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let project_path = get_project_path(&conn, &project_id)?;
    drop(conn);
    if !project_path.is_empty() {
        register_asset_scope(&app, Path::new(&project_path))?;
    }
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let row: Option<String> = conn
        .query_row(
            "SELECT data_json FROM project_index_cache
             WHERE project_id = ?1 AND kind = 'filetree'",
            [&project_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    match row {
        Some(json) => serde_json::from_str(&json)
            .map(Some)
            .map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

/// 读取单层目录内容（文件树懒加载：先读根目录，展开时逐级按需请求，无深度/数量上限）
/// 安全约束：仅允许项目目录内的相对路径（防路径穿越）；path 为空表示项目根。
#[tauri::command]
pub fn list_project_dir(
    project_id: String,
    path: String,
    state: State<DbState>,
    app: AppHandle,
) -> Result<Vec<FileTreeNode>, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        get_project_path(&conn, &project_id)?
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
}

/// 读取项目内文件文本内容（UTF-8），供预览面板使用。
/// 安全约束：仅允许项目目录内的相对路径（防路径穿越），且文件 ≤ 5MB。
#[tauri::command]
pub fn read_project_file(
    project_id: String,
    path: String,
    state: State<DbState>,
    app: AppHandle,
) -> Result<String, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        get_project_path(&conn, &project_id)?
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
    if len > MAX_PREVIEW_BYTES {
        return Err(format!("文件过大（{:.1}MB），仅支持预览 {}MB 以内的文本", len as f64 / 1024.0 / 1024.0, MAX_PREVIEW_BYTES / 1024 / 1024));
    }
    fs::read_to_string(&canonical).map_err(|e| format!("仅支持 UTF-8 文本预览: {e}"))
}

/// 扫描项目符号（组件/类/函数/路由等），返回全部符号定义（含文件与行号）。
/// 供前端符号跳转面板与 Agent 工具使用。
/// 异步命令：扫描在 blocking 线程池执行，首次全量扫描（可能数秒）不阻塞 IPC 主循环，
/// 前端切换 Tab/其它操作不受影响。
#[tauri::command]
pub async fn index_project_symbols(
    project_id: String,
    state: State<'_, DbState>,
) -> Result<Vec<crate::services::symbol_index::Symbol>, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        get_project_path(&conn, &project_id)?
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
    state: State<'_, DbState>,
) -> Result<Vec<crate::services::symbol_index::Symbol>, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        get_project_path(&conn, &project_id)?
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
    state: State<'_, DbState>,
) -> Result<crate::services::symbol_index::ProjectOutline, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        get_project_path(&conn, &project_id)?
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
    state: State<'_, DbState>,
) -> Result<Vec<crate::services::symbol_index::Symbol>, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        get_project_path(&conn, &project_id)?
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
    state: State<'_, DbState>,
) -> Result<(), String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        get_project_path(&conn, &project_id)?
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
    state: State<'_, DbState>,
) -> Result<crate::services::symbol_index::SymbolIndexMeta, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        get_project_path(&conn, &project_id)?
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
    state: State<'_, DbState>,
) -> Result<Vec<crate::services::symbol_index::SymbolCount>, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        get_project_path(&conn, &project_id)?
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

