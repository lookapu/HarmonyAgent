use crate::db::models::{ChatMessage, Conversation, Project, ProjectInspect};
use crate::db::DbState;
use crate::utils::path::normalize_path;
use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn row_to_project(row: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        // 路径统一规范化：兼容历史数据中 canonicalize 残留的 \\?\ verbatim 前缀
        path: normalize_path(&row.get::<_, String>(2)?),
        kind: row.get(3)?,
        trusted: row.get::<_, i64>(4)? != 0,
        default_provider_id: row.get(5)?,
        default_model_id: row.get(6)?,
        index_state: row.get(7)?,
        rules: row.get(8)?,
        last_opened_at: row.get(9)?,
        created_at: row.get(10)?,
        worktree_path: row.get::<_, Option<String>>(11)?.map(|s| normalize_path(&s)),
        harmony_subprojects: row.get::<_, Option<String>>(12)?,
        workspace_modules: row.get::<_, Option<String>>(13)?,
        harmony_project_path: row.get::<_, Option<String>>(14)?,
        conversation_count: row.get(15)?,
        pinned: row.get::<_, i64>(16)? != 0,
    })
}

fn row_to_conversation(row: &Row) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        provider_id: row.get(3)?,
        model_id: row.get(4)?,
        system_prompt_version: row.get(5)?,
        is_pinned: row.get::<_, i64>(6)? != 0,
        archived: row.get::<_, i64>(7)? != 0,
        tags: row.get::<_, String>(10).unwrap_or_default(),
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        work_mode: row.get::<_, String>(11).unwrap_or_else(|_| "local".into()),
        worktree_path: row.get::<_, Option<String>>(12)?,
        worktree_branch: row.get::<_, Option<String>>(13)?,
    })
}

fn row_to_message(row: &Row) -> rusqlite::Result<ChatMessage> {
    Ok(ChatMessage {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        role: row.get(2)?,
        content: row.get(3)?,
        references_json: row.get(4)?,
        model: row.get(5)?,
        tokens_in: row.get(6)?,
        tokens_out: row.get(7)?,
        created_at: row.get(8)?,
        reasoning: row.get(9)?,
        queued: row.get(10)?,
        agent_owned: row.get(11)?,
        modified_files_json: row.get(12)?,
        duration_ms: row.get(13)?,
    })
}

pub(crate) fn get_project_by_id(state: &State<DbState>, id: &str) -> Result<Project, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT p.id, p.name, p.path, p.kind, p.trusted, p.default_provider_id, p.default_model_id,
                p.index_state, p.rules, p.last_opened_at, p.created_at, p.worktree_path, p.harmony_subprojects, p.workspace_modules,
                p.harmony_project_path,
                (SELECT COUNT(*) FROM conversations c WHERE c.project_id = p.id) AS conversation_count,
                p.pinned
         FROM projects p WHERE p.id = ?1",
        [id],
        row_to_project,
    )
    .map_err(|e| e.to_string())
}

/// 项目列表（按最近打开排序）
#[tauri::command]
pub fn list_projects(state: State<DbState>) -> Result<Vec<Project>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT p.id, p.name, p.path, p.kind, p.trusted, p.default_provider_id, p.default_model_id,
                    p.index_state, p.rules, p.last_opened_at, p.created_at, p.worktree_path, p.harmony_subprojects, p.workspace_modules,
                    p.harmony_project_path,
                    (SELECT COUNT(*) FROM conversations c WHERE c.project_id = p.id) AS conversation_count,
                    p.pinned
             FROM projects p
                 ORDER BY p.pinned DESC,
                          CASE WHEN p.kind = 'global' THEN 1 ELSE 0 END,
                          COALESCE(p.last_opened_at, p.created_at) DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], row_to_project)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// 置顶/取消置顶项目（列表排序优先），返回更新后的项目
#[tauri::command]
pub fn set_project_pinned(id: String, pinned: bool, state: State<DbState>) -> Result<Project, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE projects SET pinned = ?1 WHERE id = ?2",
        params![pinned as i64, id],
    )
    .map_err(|e| e.to_string())?;
    get_project_by_id(&state, &id)
}

/// 添加项目前的目录探测（信任对话框展示用，不落库）
#[tauri::command]
pub fn inspect_project(path: String, state: State<DbState>) -> Result<ProjectInspect, String> {
    let p = Path::new(&path);
    if !p.is_dir() {
        return Err("目录不存在或不可访问".into());
    }
    let canon = fs::canonicalize(p).map_err(|e| format!("路径解析失败: {e}"))?;
    // canonicalize 在 Windows 上返回 \\?\ verbatim 前缀，入库前还原为普通路径（I:\xxx）
    let canon_str = normalize_path(&canon.to_string_lossy());
    let name = canon
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| canon_str.clone());

    let is_harmony = find_harmony_marker(&canon);
    let (app_name, bundle_name) = if is_harmony {
        let info = crate::services::harmony::parse_project(&canon);
        (info.app_label, info.bundle_name)
    } else {
        (None, None)
    };

    // 轻量文件计数（上限 20000，防大目录卡死）
    let file_count = count_files(&canon, 0, 20_000);

    let already_added = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM projects WHERE path = ?1",
                [&canon_str],
                |row| row.get(0),
            )
            .unwrap_or(false);
        exists
    };

    Ok(ProjectInspect {
        path: canon_str,
        name,
        is_harmony,
        file_count,
        has_git: canon.join(".git").is_dir(),
        already_added,
        app_name,
        bundle_name,
    })
}

/// 添加项目（trusted=0，随后走信任流程）
///
/// 注意：不在此处同步扫描工作区模块——大仓库的递归扫描会阻塞 UI。
/// 入库时仅写入空的 workspace_modules，由前端在添加成功后异步调用
/// `rescan_workspace_modules` 在后台线程完成扫描并刷新。
#[tauri::command]
pub fn add_project(app: AppHandle, path: String, state: State<DbState>) -> Result<Project, String> {
    let inspected = inspect_project(path, state.clone())?;
    if inspected.already_added {
        return Err("该项目已在列表中".into());
    }
    let id = Uuid::new_v4().to_string();
    let ts = now();
    let kind = if inspected.is_harmony { "harmony" } else { "generic" };

    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO projects (id, name, path, kind, trusted, index_state, last_opened_at, created_at, harmony_subprojects, workspace_modules)
         VALUES (?1, ?2, ?3, ?4, 0, 'pending', ?5, ?6, '[]', '[]')",
        params![id, inspected.name, inspected.path, kind, ts, ts],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);

    // 通知前端刷新项目列表（项目创建/删除等均会 emit，各页据此重新拉取）
    let _ = app.emit("projects-changed", serde_json::json!({ "project_id": id }));

    get_project_by_id(&state, &id)
}

/// 信任项目（信任流程确认按钮）
#[tauri::command]
pub fn trust_project(id: String, state: State<DbState>) -> Result<Project, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE projects SET trusted = 1, last_opened_at = ?1 WHERE id = ?2",
        params![now(), id],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
    get_project_by_id(&state, &id)
}

/// 删除项目（级联删除会话/消息/权限/索引缓存；清理项目级 MCP 服务器与技能配置）
/// 磁盘上的项目目录移入系统回收站（可恢复），不做物理删除；失败则中止删除保持数据一致。
#[tauri::command]
pub fn delete_project(
    app: AppHandle,
    id: String,
    state: State<DbState>,
    manager: State<'_, crate::services::mcp_manager::McpManager>,
) -> Result<(), String> {
    // 先取项目路径：删除 DB 前把磁盘目录移入回收站（目录不存在时跳过，视为用户已手动删除）
    let path: Option<String> = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT path FROM projects WHERE id = ?1", [&id], |r| r.get::<_, String>(0))
            .ok()
    };
    if let Some(p) = &path {
        let norm = normalize_path(p);
        let dir = Path::new(&norm);
        if dir.is_dir() {
            move_to_recycle_bin(dir)?;
        }
    }
    // 项目级配置清理信息，删除后清理符号磁盘缓存与 MCP 连接
    let (mcp_ids, skill_dirs, repo_roots) = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        // 项目级 MCP 服务器 id：删除后断开已缓存连接
        let mut stmt = conn
            .prepare("SELECT id FROM mcp_servers WHERE project_id = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([&id], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        let mcp_ids: Vec<String> = rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
        // 项目级技能：目录与仓库根（删除后做引用计数清理，共享目录不误删）
        let mut stmt = conn
            .prepare("SELECT directory, repo_owner, repo_name FROM skills WHERE project_id = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([&id], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut skill_dirs: Vec<String> = Vec::new();
        let mut repo_roots: Vec<String> = Vec::new();
        for row in rows {
            let (dir, owner, name) = row.map_err(|e| e.to_string())?;
            if let Some(d) = dir {
                skill_dirs.push(d);
            }
            if let (Some(o), Some(n)) = (owner, name) {
                repo_roots.push(format!("{o}__{n}"));
            }
        }
        conn.execute("DELETE FROM mcp_servers WHERE project_id = ?1", [&id])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM skills WHERE project_id = ?1", [&id])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM projects WHERE id = ?1", [&id])
            .map_err(|e| e.to_string())?;
        (mcp_ids, skill_dirs, repo_roots)
    };
    // 断开项目级 MCP 连接（子进程不残留到应用退出）
    manager.disconnect(&mcp_ids);
    // 技能目录引用计数清理：行已删，仅当无其他作用域引用同一目录时才删磁盘文件
    for d in skill_dirs {
        let in_use = state
            .0
            .lock()
            .map(|conn| crate::db::queries::skill_directory_in_use(&conn, &d).unwrap_or(true))
            .unwrap_or(true);
        if !in_use {
            let _ = std::fs::remove_dir_all(&d);
        }
    }
    // 仓库根目录若已空，一并清理（{owner}__{name}，与 remove_skill 行为一致）
    if let Ok(skills_dir) = app.path().app_data_dir().map(|d| d.join("skills")) {
        for r in repo_roots {
            let repo_dir = skills_dir.join(&r);
            let empty = std::fs::read_dir(&repo_dir)
                .map(|mut it| it.next().is_none())
                .unwrap_or(false);
            if empty {
                let _ = std::fs::remove_dir_all(&repo_dir);
            }
        }
    }
    if let Some(p) = path {
        let root = normalize_path(&p);
        if !root.is_empty() {
            crate::services::symbol_index::invalidate_cache(Path::new(&root));
        }
    }
    Ok(())
}

/// 将目录移入系统回收站（Windows 回收站 / macOS 废纸篓），可恢复、不弹任何 UI。
/// 失败返回 Err（如文件被占用/权限不足）；目录不存在由调用方提前判空跳过。
/// 使用 trash crate 统一实现，避免各平台手写系统 API。
#[cfg(any(windows, target_os = "macos"))]
fn move_to_recycle_bin(dir: &Path) -> Result<(), String> {
    trash::delete(dir).map_err(|e| format!("移入系统回收站失败：{e}"))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn move_to_recycle_bin(_dir: &Path) -> Result<(), String> {
    // 非 Windows/macOS 平台暂无回收站实现：保持"不动磁盘文件"的既有行为
    Ok(())
}

/// 各项目的项目级专属配置数量（MCP 服务器、技能），用于项目列表徽标。
/// 返回 JSON 对象：{ "<project_id>": { "mcp": n, "skills": m } }
#[tauri::command]
pub fn project_scoped_counts(state: State<DbState>) -> Result<std::collections::HashMap<String, serde_json::Value>, String> {
    use std::collections::HashMap;
    let conn = state.0.lock().map_err(|e| e.to_string())?;

    let count = |sql: &str| -> Result<HashMap<String, i64>, String> {
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| e.to_string())?;
        let mut map = HashMap::new();
        for row in rows {
            let (pid, n) = row.map_err(|e| e.to_string())?;
            map.insert(pid, n);
        }
        Ok(map)
    };

    let mcp = count("SELECT project_id, COUNT(*) FROM mcp_servers WHERE project_id IS NOT NULL GROUP BY project_id")?;
    let skills = count("SELECT project_id, COUNT(*) FROM skills WHERE project_id IS NOT NULL GROUP BY project_id")?;

    let mut all: HashMap<String, serde_json::Value> = HashMap::new();
    for pid in mcp.keys().chain(skills.keys()) {
        if !all.contains_key(pid) {
            all.insert(
                pid.clone(),
                serde_json::json!({
                    "mcp": mcp.get(pid).copied().unwrap_or(0),
                    "skills": skills.get(pid).copied().unwrap_or(0),
                }),
            );
        }
    }
    Ok(all)
}

// ---------- 工作区多类型模块识别 ----------
//
// 一个根目录（工作区）可能混合包含 Vue/React/Java/Go/鸿蒙等多种工程。工具始终把根目录
// 作为"一个项目"添加，同时扫描并记录其中各子目录的模块类型，供对话与工具联动。

use crate::services::workspace::{self, ModuleKind, WorkspaceModule};

/// 扫描结果项：模块信息 + 该子目录的探测信息
#[derive(Debug, Serialize)]
pub struct ScannedModule {
    #[serde(flatten)]
    pub module: WorkspaceModule,
    pub inspect: ProjectInspect,
}

/// 预览扫描：列出所选目录下识别到的所有模块（不落库）
#[tauri::command]
pub fn scan_workspace_modules(
    path: String,
    state: State<DbState>,
) -> Result<Vec<ScannedModule>, String> {
    let p = Path::new(&path);
    if !p.is_dir() {
        return Err("目录不存在或不可访问".into());
    }
    let root = fs::canonicalize(p).map_err(|e| format!("路径解析失败: {e}"))?;

    let modules = workspace::scan(&root, None);
    let mut results = Vec::with_capacity(modules.len());
    for m in modules {
        let cand = root.join(&m.rel_path);
        let canon_str = normalize_path(&cand.to_string_lossy());
        let inspect = inspect_project(canon_str, state.clone())?;
        results.push(ScannedModule { module: m, inspect });
    }
    Ok(results)
}

/// 重新扫描已添加项目的工作区模块（保留手动绑定项），更新记录并返回最新 project。
///
/// Tauri 命令在独立线程上执行，不会阻塞 UI 线程；前端可在添加项目成功后异步调用，
/// 或在项目结构变化时手动刷新。扫描本身为递归目录遍历，大仓库可能耗时数百毫秒。
#[tauri::command]
pub fn rescan_workspace_modules(
    project_id: String,
    state: State<DbState>,
) -> Result<Project, String> {
    let (path, existing_json) = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT path, workspace_modules FROM projects WHERE id = ?1",
            [&project_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .map_err(|e| e.to_string())?
    };
    let root = PathBuf::from(&path);
    let existing = workspace::parse(existing_json.as_deref());
    let modules = workspace::scan(&root, Some(&existing));
    save_modules(&state, &project_id, &modules)?;
    get_project_by_id(&state, &project_id)
}

/// 手动设置工作区模块列表（增删改、修改类型）。manual=true 的项在后续重新扫描时被保留。
#[tauri::command]
pub fn set_workspace_modules(
    project_id: String,
    modules: Vec<WorkspaceModule>,
    state: State<DbState>,
) -> Result<Project, String> {
    // 规范化：统一正斜杠、去空/去 "."、去前导 "./"，校验 kind
    let cleaned: Vec<WorkspaceModule> = modules
        .into_iter()
        .map(|mut m| {
            m.rel_path = m
                .rel_path
                .replace('\\', "/")
                .trim_start_matches("./")
                .trim_end_matches('/')
                .to_string();
            m
        })
        .filter(|m| !m.rel_path.is_empty() && m.rel_path != ".")
        .collect();
    save_modules(&state, &project_id, &cleaned)?;
    get_project_by_id(&state, &project_id)
}

/// 持久化模块列表：同时写 workspace_modules 与 harmony_subprojects（后者作冗余，便于旧逻辑/查询）。
/// 自动兜底：未配置"鸿蒙主工程"且工作区恰好只有一个鸿蒙模块时，自动将其设为主工程。
fn save_modules(
    state: &State<DbState>,
    project_id: &str,
    modules: &[WorkspaceModule],
) -> Result<(), String> {
    let modules_json = workspace::stringify(modules);
    let harmony_relpaths: Vec<&str> = modules
        .iter()
        .filter(|m| m.kind == ModuleKind::Harmony)
        .map(|m| m.rel_path.as_str())
        .collect();
    let harmony_json = serde_json::to_string(&harmony_relpaths).map_err(|e| e.to_string())?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    // 项目根（用于工程根级校验；查询失败时跳过自动兜底判定）
    let project_root: PathBuf = conn
        .query_row(
            "SELECT path FROM projects WHERE id = ?1",
            [project_id],
            |r| r.get::<_, String>(0),
        )
        .map(PathBuf::from)
        .unwrap_or_default();
    // 用户手动配置过则不覆盖；仅当未配置且唯一鸿蒙模块时自动设置
    let existing_cfg: Option<String> = conn
        .query_row(
            "SELECT harmony_project_path FROM projects WHERE id = ?1",
            [project_id],
            |r| r.get(0),
        )
        .unwrap_or(None);
    let auto: Option<String> = if existing_cfg
        .as_deref()
        .map(|s| s.trim())
        .unwrap_or("")
        .is_empty()
        && harmony_relpaths.len() == 1
        && !project_root.as_os_str().is_empty()
        // 仅当该模块是工程根级（AppScope/build-profile 存在）才自动设为主工程，
        // 避免 entry 等纯模块目录（仅 oh-package.json5）被误设导致 Bundle 名/SDK 识别失败
        && crate::services::harmony::is_project_root(&project_root.join(harmony_relpaths[0].replace('\\', "/")))
    {
        Some(harmony_relpaths[0].to_string())
    } else {
        None
    };
    conn.execute(
        "UPDATE projects SET workspace_modules = ?1, harmony_subprojects = ?2,
                harmony_project_path = COALESCE(?3, harmony_project_path) WHERE id = ?4",
        params![modules_json, harmony_json, auto, project_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 会话"鸿蒙主工程"解析结果
#[derive(Debug, Clone, Serialize)]
pub struct HarmonyRootInfo {
    /// 解析后的鸿蒙主工程根（绝对路径；未配置且无唯一候选时为项目根）
    pub root: String,
    /// 已配置的鸿蒙主工程（相对项目根或绝对路径）；None=未配置
    pub configured: Option<String>,
    /// 候选鸿蒙子工程（绝对路径列表，不含项目根本身）
    pub candidates: Vec<String>,
    /// 是否自动兜底（未配置但工作区仅一个鸿蒙模块）
    pub auto: bool,
}

/// 解析项目的"鸿蒙主工程"根（命令与 Agent 工具共用）：
/// 1) 已配置 harmony_project_path 且为工程根级 → 直接解析；
/// 2) 项目根本身是鸿蒙工程根 → 用项目根（常见单工程场景，优先于子模块候选）；
/// 3) 未配置 → 若工作区恰好一个工程根级鸿蒙子工程，自动兜底使用它；
/// 4) 否则回退到项目根本身。
///    工程根级判定见 harmony::is_project_root：AppScope/app.json5 存在，或 build-profile.json5 顶层含 "app" 键，
///    避免 entry 等纯模块目录（模块级 build-profile.json5 无 "app" 键）被误设为主工程导致 Bundle 名/SDK 识别失败。
pub fn resolve_harmony_root(
    conn: &rusqlite::Connection,
    project_id: &str,
    root: Option<&str>,
) -> Result<HarmonyRootInfo, String> {
    let (db_path, configured, harmony_json) = conn
        .query_row(
            "SELECT path, harmony_project_path, harmony_subprojects FROM projects WHERE id = ?1",
            [project_id],
            |r| {
                Ok((
                    normalize_path(&r.get::<_, String>(0)?),
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .map_err(|e| e.to_string())?;
    // worktree 覆盖优先，否则回退项目主路径（与文件树/符号命令的 resolve_root 同口径）
    let root_path = match root.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(r) => normalize_path(r),
        None => db_path,
    };
    let root = PathBuf::from(&root_path);
    // 候选：工作区扫描出的鸿蒙子工程（绝对路径，跳过项目根自身）
    let candidates: Vec<String> =
        serde_json::from_str::<Vec<String>>(harmony_json.as_deref().unwrap_or("[]"))
            .unwrap_or_default()
            .into_iter()
            .filter(|p| !p.is_empty() && p != ".")
            .map(|p| normalize_path(&root.join(p.replace('\\', "/")).to_string_lossy()))
            .filter(|p| p != &root_path)
            .collect();

    // 1) 显式配置优先（校验为工程根级；自动兜底误写入的纯模块目录会被回退纠正）
    if let Some(cfg) = configured.as_deref().filter(|s| !s.trim().is_empty()) {
        let joined = if Path::new(cfg).is_absolute() {
            PathBuf::from(cfg)
        } else {
            root.join(cfg.replace('\\', "/"))
        };
        if joined.is_dir() && crate::services::harmony::is_project_root(&joined) {
            return Ok(HarmonyRootInfo {
                root: normalize_path(&joined.to_string_lossy()),
                configured: Some(cfg.to_string()),
                candidates,
                auto: false,
            });
        }
        // 配置的目录不存在或非工程根级：继续兜底（保留 configured 供前端修正）
    }
    // 2) 项目根本身是鸿蒙工程根 → 直接用（常见单工程场景，优先于子模块候选）
    if crate::services::harmony::is_project_root(&root) {
        return Ok(HarmonyRootInfo {
            root: root_path,
            configured: None,
            candidates,
            auto: false,
        });
    }
    // 3) 自动兜底：恰好一个工程根级鸿蒙子工程
    if candidates.len() == 1 && crate::services::harmony::is_project_root(Path::new(&candidates[0])) {
        return Ok(HarmonyRootInfo {
            root: candidates[0].clone(),
            configured: None,
            candidates,
            auto: true,
        });
    }
    // 4) 回退项目根
    Ok(HarmonyRootInfo {
        root: root_path,
        configured: None,
        candidates,
        auto: false,
    })
}

/// 查询项目的"鸿蒙主工程"解析结果（工程分析面板选择器与 Agent 工具共用）
#[tauri::command]
pub fn get_harmony_root(
    state: State<DbState>,
    project_id: String,
    root: Option<String>,
) -> Result<HarmonyRootInfo, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    resolve_harmony_root(&conn, &project_id, root.as_deref())
}

/// 设置会话"鸿蒙主工程"（空串=清除，回退项目根本身）；返回更新后的项目
#[tauri::command]
pub fn set_harmony_project_path(
    state: State<DbState>,
    project_id: String,
    path: String,
    root: Option<String>,
) -> Result<Project, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let db_path: String = conn
        .query_row(
            "SELECT path FROM projects WHERE id = ?1",
            [&project_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    // worktree 覆盖优先：相对路径相对会话工作目录存储（结构与主仓库一致，跨会话仍有效）
    let root_path = match root.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(r) => normalize_path(r),
        None => normalize_path(&db_path),
    };
    let trimmed = path.trim();
    let stored: Option<String> = if trimmed.is_empty() {
        None
    } else {
        // 规范化：绝对路径位于项目根下则存相对（正斜杠），否则存绝对；相对路径去 ./ 与尾部 /
        let p = Path::new(trimmed);
        let normalized = if p.is_absolute() {
            let abs = normalize_path(trimmed);
            match Path::new(&abs).strip_prefix(Path::new(&root_path)) {
                Ok(rel) if !rel.as_os_str().is_empty() => {
                    Some(rel.to_string_lossy().replace('\\', "/"))
                }
                _ => Some(abs),
            }
        } else {
            Some(
                trimmed
                    .replace('\\', "/")
                    .trim_start_matches("./")
                    .trim_end_matches('/')
                    .to_string(),
            )
        };
        // 校验目标目录存在（避免配置死路径）
        let target = match &normalized {
            Some(rel) if Path::new(rel).is_absolute() => PathBuf::from(rel),
            Some(rel) => Path::new(&root_path).join(rel),
            None => PathBuf::from(&root_path),
        };
        if !target.is_dir() {
            return Err("目标目录不存在或不可访问".into());
        }
        normalized
    };
    conn.execute(
        "UPDATE projects SET harmony_project_path = ?1 WHERE id = ?2",
        params![stored, project_id],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
    get_project_by_id(&state, &project_id)
}

/// Git 分支信息
#[derive(Debug, Serialize)]
pub struct GitBranchInfo {
    pub has_git: bool,
    pub current: Option<String>,
    pub branches: Vec<String>,
    pub error: Option<String>,
}

/// 在指定目录执行 git 命令（无窗口，捕获输出）
fn run_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("git 执行失败（未安装或不在 PATH）: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// 在 blocking 线程池执行 git 命令，避免子进程阻塞 IPC 主循环
async fn run_git_async(dir: PathBuf, args: Vec<String>) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        run_git(&dir, &refs)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 采集指定目录的 git 分支信息（git 子进程在 blocking 线程池执行）
async fn collect_git_branch_info(dir: &Path) -> Result<GitBranchInfo, String> {
    let dir = dir.to_path_buf();
    let current = run_git_async(dir.clone(), vec!["symbolic-ref".into(), "--short".into(), "HEAD".into()])
        .await
        .ok()
        .filter(|s| !s.is_empty());
    let out = run_git_async(dir, vec!["branch".into(), "--format".into(), "%(refname:short)".into()]).await?;
    let branches: Vec<String> = out
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Ok(GitBranchInfo {
        has_git: true,
        current,
        branches,
        error: None,
    })
}

/// 获取项目的 Git 分支列表与当前分支
#[tauri::command]
pub async fn get_git_branches(project_id: String, state: State<'_, DbState>) -> Result<GitBranchInfo, String> {
    let project = get_project_by_id(&state, &project_id)?;
    if project.path.is_empty() || !Path::new(&project.path).join(".git").is_dir() {
        return Ok(GitBranchInfo {
            has_git: false,
            current: None,
            branches: vec![],
            error: None,
        });
    }
    collect_git_branch_info(Path::new(&project.path)).await
}

/// 切换 Git 分支，返回切换后的最新分支状态
#[tauri::command]
pub async fn switch_git_branch(
    project_id: String,
    branch: String,
    state: State<'_, DbState>,
) -> Result<GitBranchInfo, String> {
    let project = get_project_by_id(&state, &project_id)?;
    if project.path.is_empty() {
        return Err("当前项目没有 Git 仓库".into());
    }
    let msg = run_git_async(PathBuf::from(project.path.clone()), vec!["switch".into(), branch]).await?;
    let info = collect_git_branch_info(Path::new(&project.path)).await?;
    if let Ok(conn) = state.0.lock() {
        let _ = crate::agent::context::invalidate_project_facts(
            &conn,
            &project_id,
            "git_branch_changed",
        );
        let _ = crate::agent::context::invalidate_project_memories(
            &conn,
            &project_id,
            "git_branch_changed",
            &[],
        );
    }
    Ok(GitBranchInfo {
        error: if msg.is_empty() { None } else { Some(msg) },
        ..info
    })
}

/// 列出会话：按项目过滤；keyword 非空时按标题/首条消息内容模糊搜索（LIKE）
#[tauri::command]
pub fn list_conversations(
    project_id: String,
    state: State<DbState>,
    include_archived: Option<bool>,
    keyword: Option<String>,
) -> Result<Vec<Conversation>, String> {
    let include_archived = include_archived.unwrap_or(false);
    let keyword = keyword.unwrap_or_default().trim().to_string();
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    // 搜索命中规则：标题 LIKE 或 首条 user 消息内容 LIKE（首条消息 = 会话最早用户消息）
    let (sql, params) = if keyword.is_empty() {
        (
            "SELECT id, project_id, title, provider_id, model_id, system_prompt_version,
                    is_pinned, archived, created_at, updated_at, tags, work_mode, worktree_path, worktree_branch
             FROM conversations WHERE project_id = ?1 AND archived = ?2
             ORDER BY is_pinned DESC, updated_at DESC"
                .to_string(),
            rusqlite::params![project_id, include_archived as i64],
        )
    } else {
        (
            "SELECT c.id, c.project_id, c.title, c.provider_id, c.model_id, c.system_prompt_version,
                    c.is_pinned, c.archived, c.created_at, c.updated_at, c.tags, c.work_mode, c.worktree_path, c.worktree_branch
             FROM conversations c
             WHERE c.project_id = ?1 AND c.archived = ?2
               AND (c.title LIKE ?3 OR EXISTS (
                    SELECT 1 FROM messages m
                    WHERE m.conversation_id = c.id AND m.role = 'user' AND m.queued = 0
                      AND m.content LIKE ?3
                    ORDER BY m.created_at ASC, m.rowid ASC LIMIT 1))
             ORDER BY c.is_pinned DESC, c.updated_at DESC"
                .to_string(),
            rusqlite::params![project_id, include_archived as i64, format!("%{keyword}%")],
        )
    };
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params, row_to_conversation)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// 按 id 查询单个会话（不区分归档状态）。搜索命中跳转时目标会话可能不在当前可见列表
/// （如已归档），前端用它兜底打开；查不到返回 None。
#[tauri::command]
pub fn get_conversation(
    id: String,
    state: State<DbState>,
) -> Result<Option<Conversation>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT id, project_id, title, provider_id, model_id, system_prompt_version,
                is_pinned, archived, created_at, updated_at, tags, work_mode, worktree_path, worktree_branch
         FROM conversations WHERE id = ?1",
        [&id],
        row_to_conversation,
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// 更新会话：置顶 / 归档 / 绑定模型（空串清除绑定，None 不改）
#[tauri::command]
pub fn update_conversation(
    id: String,
    state: State<DbState>,
    is_pinned: Option<bool>,
    archived: Option<bool>,
    model_id: Option<String>,
    tags: Option<String>,
) -> Result<Conversation, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(pin) = is_pinned {
        conn.execute(
            "UPDATE conversations SET is_pinned = ?1 WHERE id = ?2",
            params![pin as i64, id],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(a) = archived {
        conn.execute(
            "UPDATE conversations SET archived = ?1 WHERE id = ?2",
            params![a as i64, id],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(m) = model_id {
        conn.execute(
            "UPDATE conversations SET model_id = NULLIF(?1, '') WHERE id = ?2",
            params![m, id],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(t) = tags {
        // 标签 normalize：去空、trim、dedup、限制 10 个；后端兜底，前端主要责任
        let normalized: Vec<String> = t
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .enumerate()
            .fold(Vec::<String>::new(), |mut acc, (i, s)| {
                if i < 10 && !acc.iter().any(|x| x == s) {
                    acc.push(s.to_string());
                }
                acc
            });
        let joined = normalized.join(",");
        conn.execute(
            "UPDATE conversations SET tags = ?1 WHERE id = ?2",
            params![joined, id],
        )
        .map_err(|e| e.to_string())?;
    }
    conn.query_row(
        "SELECT id, project_id, title, provider_id, model_id, system_prompt_version,
                is_pinned, archived, created_at, updated_at, tags, work_mode, worktree_path, worktree_branch
         FROM conversations WHERE id = ?1",
        [id],
        row_to_conversation,
    )
    .map_err(|e| e.to_string())
}

/// 新建会话（默认标题"新会话"）。worktree 模式时传入 worktree_path/worktree_branch。
#[tauri::command]
pub fn create_conversation(
    project_id: String,
    title: Option<String>,
    work_mode: Option<String>,
    worktree_path: Option<String>,
    worktree_branch: Option<String>,
    state: State<DbState>,
) -> Result<Conversation, String> {
    let id = Uuid::new_v4().to_string();
    let ts = now();
    let is_worktree = work_mode.as_deref() == Some("worktree");
    // 本地模式强制清空 worktree 字段，避免脏数据
    let wt_path = if is_worktree {
        worktree_path.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty())
    } else {
        None
    };
    let wt_branch = if is_worktree {
        worktree_branch.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty())
    } else {
        None
    };
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO conversations (id, project_id, title, created_at, updated_at, work_mode, worktree_path, worktree_branch)
         VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7)",
        params![id, project_id, title.unwrap_or_else(|| "新会话".into()), ts, if is_worktree { "worktree" } else { "local" }, wt_path, wt_branch],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
    let conversations = list_conversations(project_id, state.clone(), None, None)?;
    conversations
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| "会话创建失败".into())
}

/// 会话 Fork：从既有会话非破坏性派生新会话（探索分支不影响原会话）。
/// 复制 messages（截至 until_message_id 含该条；None=全部）与对应范围的 session_events
/// （审计轨迹随行）。消息 ID 全部重新生成；queued/agent_owned 清零（fork 的是静态历史）。
#[tauri::command]
pub fn fork_conversation(
    from_id: String,
    until_message_id: Option<String>,
    anchor_kind: Option<String>,
    anchor_ref: Option<String>,
    state: State<DbState>,
) -> Result<Conversation, String> {
    let new_id = Uuid::new_v4().to_string();
    let ts = now();
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let tx = conn
        .transaction()
        .map_err(|e| e.to_string())?;

    // 1) 源会话信息 + 新会话落库（worktree 绑定随 fork 继承，保持在同一工作目录）
    let (project_id, title, work_mode, worktree_path, worktree_branch): (String, String, String, Option<String>, Option<String>) = tx
        .query_row(
            "SELECT project_id, title, work_mode, worktree_path, worktree_branch FROM conversations WHERE id = ?1",
            [&from_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .map_err(|e| format!("源会话不存在: {e}"))?;
    tx.execute(
        "INSERT INTO conversations (id, project_id, title, created_at, updated_at, work_mode, worktree_path, worktree_branch)
         VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7)",
        params![new_id, project_id, format!("Fork·{title}"), ts, work_mode, worktree_path, worktree_branch],
    )
    .map_err(|e| e.to_string())?;

    // 2) 截止锚点：until 消息的 (created_at, rowid)，按序截断（同秒多条时不误带后续消息）
    let requested_anchor = anchor_kind.as_deref().unwrap_or(if until_message_id.is_some() { "message" } else { "latest" });
    if !matches!(requested_anchor, "latest" | "message" | "checkpoint" | "build_failure" | "git_commit") {
        return Err("anchor_kind 仅支持 latest|message|checkpoint|build_failure|git_commit".into());
    }
    let effective_ref = anchor_ref.as_ref().or(until_message_id.as_ref());
    let until = resolve_conversation_branch_anchor(&tx, &from_id, requested_anchor, effective_ref.map(String::as_str))?;

    // 3) 复制 messages（新 UUID；按 (created_at, rowid) 升序保持原顺序）
    let copied: usize = {
        let mut stmt = tx
            .prepare(
                "SELECT role, content, references_json, model, tokens_in, tokens_out,
                        created_at, reasoning, modified_files_json, duration_ms
                 FROM messages
                 WHERE conversation_id = ?1
                   AND (?2 IS NULL OR created_at < ?2 OR (created_at = ?2 AND rowid <= ?3))
                 ORDER BY created_at ASC, rowid ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows: Vec<(String, String, Option<String>, Option<String>, Option<i64>, Option<i64>, i64, Option<String>, Option<String>, Option<i64>)> = stmt
            .query_map(
                params![from_id, until.map(|u| u.0), until.map(|u| u.1)],
                |r| {
                    Ok((
                        r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?,
                        r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?,
                    ))
                },
            )
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        let mut n = 0usize;
        for (role, content, refs_json, model, tin, tout, created, reasoning, mod_files, dur) in rows {
            tx.execute(
                "INSERT INTO messages (id, conversation_id, role, content, references_json, model,
                    tokens_in, tokens_out, created_at, reasoning, queued, agent_owned,
                    modified_files_json, duration_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, 0, ?11, ?12)",
                params![Uuid::new_v4().to_string(), new_id, role, content, refs_json, model, tin, tout, created, reasoning, mod_files, dur],
            )
            .map_err(|e| e.to_string())?;
            n += 1;
        }
        n
    };

    // 4) 复制 session_events（按截止消息时间过滤；seq/trace_id 保留，事件为审计轨迹）
    tx.execute(
        "INSERT INTO session_events (conversation_id, seq, event_type, payload, created_at, trace_id)
         SELECT ?2, seq, event_type, payload, created_at, trace_id
         FROM session_events
         WHERE conversation_id = ?1 AND (?3 IS NULL OR created_at <= ?3)",
        params![from_id, new_id, until.map(|u| u.0)],
    )
    .map_err(|e| e.to_string())?;

    tx.execute(
        "INSERT INTO conversation_branches
         (id,source_conversation_id,branch_conversation_id,anchor_kind,anchor_ref,anchor_message_rowid,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![Uuid::new_v4().to_string(), from_id, new_id, requested_anchor,
            effective_ref, until.map(|item| item.1), ts],
    ).map_err(|e| e.to_string())?;
    let _ = crate::agent::enterprise::audit(
        &tx,
        None,
        Some(&new_id),
        "user",
        "conversation.fork",
        "conversation",
        "created",
        &serde_json::json!({
            "source_conversation_id": from_id,
            "branch_conversation_id": new_id,
            "anchor_kind": requested_anchor,
            "anchor_ref": effective_ref,
            "anchor_message_rowid": until.map(|item| item.1),
        }),
    );

    tx.commit().map_err(|e| e.to_string())?;
    drop(conn);
    // 空会话 fork 等价于新建，同样允许
    let _ = copied;
    list_conversations(project_id, state.clone(), None, None)?
        .into_iter()
        .find(|c| c.id == new_id)
        .ok_or_else(|| "会话派生失败".into())
}

fn resolve_conversation_branch_anchor(
    conn: &rusqlite::Connection,
    from_id: &str,
    requested_anchor: &str,
    effective_ref: Option<&str>,
) -> Result<Option<(i64, i64)>, String> {
    let resolved = match requested_anchor {
        "message" => {
            let mid = effective_ref.ok_or_else(|| "消息锚点缺少 anchor_ref".to_string())?;
            let row: Option<(i64, i64)> = conn
                .query_row(
                    "SELECT created_at, rowid FROM messages WHERE id = ?1 AND conversation_id = ?2",
                    params![mid, from_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            Some(row.ok_or_else(|| "截止消息不存在".to_string())?)
        }
        "checkpoint" => {
            let snapshot_id = effective_ref.ok_or_else(|| "检查点锚点缺少 anchor_ref".to_string())?;
            let rowid: i64 = conn.query_row(
                "SELECT msg_rowid FROM conversation_snapshots WHERE id=?1 AND conversation_id=?2",
                params![snapshot_id, from_id], |row| row.get(0),
            ).map_err(|_| "检查点不存在或不属于该会话".to_string())?;
            let created_at = conn.query_row(
                "SELECT created_at FROM messages WHERE conversation_id=?1 AND rowid=?2",
                params![from_id, rowid], |row| row.get(0),
            ).unwrap_or(0);
            Some((created_at, rowid))
        }
        "build_failure" | "git_commit" => {
            let (tool_filter, status_filter) = if requested_anchor == "build_failure" {
                ("tool_name IN ('build_project','build_generic','build_hap','hvigor_build','run_tests','test_project')", "status NOT IN ('ok','completed')")
            } else {
                ("tool_name='git_commit'", "status IN ('ok','completed')")
            };
            let ref_filter = if effective_ref.is_some() { " AND (id=?2 OR COALESCE(result_json,'') LIKE '%'||?2||'%')" } else { "" };
            let sql = format!("SELECT created_at,id FROM tool_runs WHERE conversation_id=?1 AND {tool_filter} AND {status_filter}{ref_filter} ORDER BY created_at DESC LIMIT 1");
            let tool_anchor: Option<(i64, String)> = if let Some(reference) = effective_ref {
                conn.query_row(&sql, params![from_id, reference], |row| Ok((row.get(0)?, row.get(1)?))).optional()
            } else {
                conn.query_row(&sql, [from_id], |row| Ok((row.get(0)?, row.get(1)?))).optional()
            }.map_err(|e| e.to_string())?;
            let (created_at, _) = tool_anchor.ok_or_else(|| format!("未找到可用的{requested_anchor}锚点"))?;
            let message: Option<(i64, i64)> = conn.query_row(
                "SELECT created_at,rowid FROM messages WHERE conversation_id=?1 AND created_at<=?2 ORDER BY created_at DESC,rowid DESC LIMIT 1",
                params![from_id, created_at], |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional().map_err(|e| e.to_string())?;
            Some(message.unwrap_or((0, 0)))
        }
        _ => None,
    };
    Ok(resolved)
}

#[derive(Clone, Serialize)]
pub struct BranchMergeResult {
    pub merge_id: String,
    pub decisions_merged: i64,
    pub artifacts_merged: i64,
    pub evidence_merged: i64,
}

#[tauri::command]
pub fn get_conversation_branch_parent(
    branch_id: String,
    state: State<DbState>,
) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT source_conversation_id FROM conversation_branches WHERE branch_conversation_id=?1",
        [branch_id], |row| row.get(0),
    ).optional().map_err(|e| e.to_string())
}

/// Merge only structured, source-backed branch output. Messages, summaries and
/// free-form assistant text are intentionally excluded.
#[tauri::command]
pub fn merge_conversation_branch(
    source_id: String,
    target_id: String,
    state: State<DbState>,
) -> Result<BranchMergeResult, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    merge_conversation_branch_conn(&mut conn, &source_id, &target_id)
}

fn merge_conversation_branch_conn(
    conn: &mut rusqlite::Connection,
    source_id: &str,
    target_id: &str,
) -> Result<BranchMergeResult, String> {
    if source_id == target_id { return Err("不能把会话分支合并到自身".into()); }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let (source_project, target_project): (String, String) = (
        tx.query_row("SELECT project_id FROM conversations WHERE id=?1", [source_id], |row| row.get(0)).map_err(|_| "源分支不存在".to_string())?,
        tx.query_row("SELECT project_id FROM conversations WHERE id=?1", [target_id], |row| row.get(0)).map_err(|_| "目标会话不存在".to_string())?,
    );
    if source_project != target_project { return Err("只能合并同一项目内的会话分支".into()); }
    let lineage: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversation_branches WHERE branch_conversation_id=?1 AND source_conversation_id=?2)",
        params![source_id, target_id], |row| row.get(0),
    ).map_err(|e| e.to_string())?;
    if !lineage { return Err("源会话不是目标会话的直接分支".into()); }
    let now = now();
    let decisions = tx.execute(
        "INSERT OR IGNORE INTO conversation_context_pins
         (id,conversation_id,project_id,pin_kind,source_ref,label,content,created_at,updated_at)
         SELECT lower(hex(randomblob(16))),?2,project_id,pin_kind,'branch:'||?1||':'||source_ref,
                label,content,?3,?3 FROM conversation_context_pins
         WHERE conversation_id=?1 AND pin_kind IN ('decision','acceptance')",
        params![source_id, target_id, now],
    ).map_err(|e| e.to_string())? as i64;
    let artifacts = tx.execute(
        "INSERT OR IGNORE INTO conversation_context_artifacts
         (id,conversation_id,run_id,artifact_kind,uri,label,digest,metadata_json,source_ref,valid,created_at,updated_at)
         SELECT lower(hex(randomblob(16))),?2,NULL,artifact_kind,uri,label,digest,metadata_json,
                'branch:'||?1||':'||source_ref,valid,?3,?3
         FROM conversation_context_artifacts WHERE conversation_id=?1 AND valid=1",
        params![source_id, target_id, now],
    ).map_err(|e| e.to_string())? as i64;
    let evidence = tx.execute(
        "INSERT OR IGNORE INTO conversation_context_facts
         (id,conversation_id,project_id,run_id,fact_kind,fact_key,value_json,source_kind,source_ref,
          scope,confidence,version,observed_at,created_at,updated_at)
         SELECT lower(hex(randomblob(16))),?2,project_id,NULL,fact_kind,
                'branch:'||?1||':'||fact_key,value_json,'branch_merge',source_ref,scope,confidence,1,?3,?3,?3
         FROM conversation_context_facts
         WHERE conversation_id=?1 AND invalidated_at IS NULL AND fact_kind IN ('verification','workspace','device')",
        params![source_id, target_id, now],
    ).map_err(|e| e.to_string())? as i64;
    let merge_id = Uuid::new_v4().to_string();
    let manifest = serde_json::json!({
        "source": source_id, "target": target_id,
        "included": ["decision_pins", "acceptance_pins", "artifacts", "verification_facts"],
        "excluded": ["messages", "summaries", "free_form_output"]
    });
    tx.execute(
        "INSERT INTO conversation_branch_merges
         (id,source_conversation_id,target_conversation_id,decisions_merged,artifacts_merged,evidence_merged,manifest_json,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![merge_id, source_id, target_id, decisions, artifacts, evidence, manifest.to_string(), now],
    ).map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE conversation_context_state SET invalidation_epoch=invalidation_epoch+1,updated_at=?1
         WHERE conversation_id=?2",
        params![now, target_id],
    ).map_err(|e| e.to_string())?;
    let _ = crate::agent::enterprise::audit(
        &tx,
        None,
        Some(target_id),
        "user",
        "conversation.branch_merge",
        "conversation_context",
        "merged",
        &manifest,
    );
    tx.commit().map_err(|e| e.to_string())?;
    Ok(BranchMergeResult { merge_id, decisions_merged: decisions, artifacts_merged: artifacts, evidence_merged: evidence })
}

/// 按标签筛选会话（精确匹配某个标签；项目内；含/不含归档）
/// tags 为空字符串时退化为 list_conversations 的语义
#[tauri::command]
pub fn list_conversations_by_tag(
    project_id: String,
    tag: String,
    include_archived: Option<bool>,
    state: State<DbState>,
) -> Result<Vec<Conversation>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let include_archived = include_archived.unwrap_or(false);
    // 标签字符串里包含 tag 即命中（用 LIKE %tag% 简单实现，前端传 tag 时已加边界）
    // 为避免子串误匹配，前后缀加逗号判等
    let pattern = format!("%{}%", tag);
    let mut stmt = conn
        .prepare(
            "SELECT id, project_id, title, provider_id, model_id, system_prompt_version,
                    is_pinned, archived, created_at, updated_at, tags, work_mode, worktree_path, worktree_branch
             FROM conversations
             WHERE project_id = ?1 AND archived = ?2 AND tags LIKE ?3
             ORDER BY is_pinned DESC, updated_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(
            rusqlite::params![project_id, include_archived as i64, pattern],
            row_to_conversation,
        )
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// 列出某项目下所有出现过的标签（去重 + 频次），用于筛选下拉
#[tauri::command]
pub fn list_conversation_tags(
    project_id: String,
    state: State<DbState>,
) -> Result<Vec<TagCount>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    // 一次性查询 + Rust 端 split 聚合：避免 N 次子串查询
    let mut stmt = conn
        .prepare(
            "SELECT tags FROM conversations
             WHERE project_id = ?1 AND tags != ''",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<String> = stmt
        .query_map([project_id], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    let mut counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for tags_str in rows {
        for tag in tags_str.split(',') {
            let tag = tag.trim();
            if tag.is_empty() {
                continue;
            }
            *counts.entry(tag.to_string()).or_insert(0) += 1;
        }
    }
    let mut out: Vec<TagCount> = counts
        .into_iter()
        .map(|(tag, count)| TagCount { tag, count })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then(a.tag.cmp(&b.tag)));
    Ok(out)
}

/// 标签 + 出现次数（前端筛选下拉展示）
#[derive(Debug, Clone, serde::Serialize)]
pub struct TagCount {
    pub tag: String,
    pub count: i64,
}

/// 消息列表（正序）
#[tauri::command]
pub fn list_messages(conversation_id: String, state: State<DbState>) -> Result<Vec<ChatMessage>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, conversation_id, role, content, references_json, model,
                    tokens_in, tokens_out, created_at, reasoning, queued, agent_owned, modified_files_json, duration_ms
             FROM messages WHERE conversation_id = ?1 AND hidden = 0 ORDER BY created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([conversation_id], row_to_message)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// 消息分页结果（messages 正序返回，has_more 表示游标之前是否还有更早消息）
#[derive(Debug, Clone, serde::Serialize)]
pub struct MessagePage {
    pub messages: Vec<ChatMessage>,
    pub has_more: bool,
}

/// 消息游标分页：返回游标（before_id 所指消息）之前最近的 limit 条，正序返回；
/// before_id 为空时返回该会话最近 limit 条。多取 1 条探测 has_more。
/// 用途：会话打开只加载最近一页，向上滚动时加载更早历史，避免长会话一次性全量加载渲染。
#[tauri::command]
pub fn list_messages_page(
    conversation_id: String,
    before_id: Option<String>,
    limit: Option<usize>,
    state: State<DbState>,
) -> Result<MessagePage, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list_messages_page_impl(&conn, &conversation_id, before_id.as_deref(), limit)
}

/// 分页实现（独立于 tauri State，便于单元测试）
fn list_messages_page_impl(
    conn: &rusqlite::Connection,
    conversation_id: &str,
    before_id: Option<&str>,
    limit: Option<usize>,
) -> Result<MessagePage, String> {
    let page_size = limit.unwrap_or(60).clamp(1, 500);
    // 游标定位：以 (created_at, id) 行值比较定位更早消息（同秒多条时 id 兜底保证全序）
    let cursor: Option<(i64, String)> = match before_id {
        Some(bid) => conn
            .query_row(
                "SELECT created_at, id FROM messages WHERE id = ?1",
                [bid],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?,
        None => None,
    };
    // before_id 指定但游标查不到（消息已被删除）：返回空页终止分页，
    // 避免前端把重复消息 prepend 到已有列表
    if before_id.is_some() && cursor.is_none() {
        return Ok(MessagePage { messages: Vec::new(), has_more: false });
    }
    let fetch = page_size + 1; // 多取 1 条探测是否仍有更早
    let sql = "SELECT id, conversation_id, role, content, references_json, model,
                      tokens_in, tokens_out, created_at, reasoning, queued, agent_owned, modified_files_json, duration_ms
               FROM messages";
    let mut out: Vec<ChatMessage> = if let Some((ts, bid)) = &cursor {
        let sql = format!(
            "{sql} WHERE conversation_id = ?1 AND hidden = 0 AND (created_at, id) < (?2, ?3)\n               ORDER BY created_at DESC, id DESC LIMIT ?4"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![conversation_id, ts, bid, fetch], row_to_message)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
    } else {
        let sql = format!(
            "{sql} WHERE conversation_id = ?1 AND hidden = 0\n               ORDER BY created_at DESC, id DESC LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![conversation_id, fetch], row_to_message)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
    };
    let has_more = out.len() > page_size;
    if has_more {
        out.truncate(page_size);
    }
    out.reverse(); // 倒序取回后反转，保持正序（旧 → 新）
    Ok(MessagePage { messages: out, has_more })
}

/// 消息全文搜索命中：消息基本信息 + 会话标题 + 内容片段（命中位置上下文）
#[derive(Debug, serde::Serialize)]
pub struct MessageSearchHit {
    pub conversation_id: String,
    pub conversation_title: String,
    pub message_id: String,
    pub role: String,
    pub created_at: i64,
    pub snippet: String,
    pub match_start: usize,
    /// 所属项目（跨项目搜索时填充，单项目搜索时为 None）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
}

/// 在项目内（或指定会话）对消息内容做 LIKE 全文检索，返回命中片段。
/// 每条命中截取匹配位置前后各 60 字符作为 snippet，最多返回 100 条。
#[tauri::command]
pub fn search_messages(
    project_id: String,
    query: String,
    conversation_id: Option<String>,
    state: State<DbState>,
) -> Result<Vec<MessageSearchHit>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let like = format!("%{q}%");
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut sql = String::from(
        "SELECT m.id, m.conversation_id, c.title, m.role, m.content, m.created_at
         FROM messages m JOIN conversations c ON c.id = m.conversation_id
         WHERE c.project_id = ?1 AND m.queued = 0 AND m.content LIKE ?2",
    );
    if conversation_id.is_some() {
        sql.push_str(" AND m.conversation_id = ?3");
    }
    sql.push_str(" ORDER BY m.created_at DESC LIMIT 100");

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let qlower = q.to_lowercase();
    let mapper = |row: &Row| -> rusqlite::Result<MessageSearchHit> {
        let id: String = row.get(0)?;
        let conv_id: String = row.get(1)?;
        let title: String = row.get(2)?;
        let role: String = row.get(3)?;
        let content: String = row.get(4)?;
        let created_at: i64 = row.get(5)?;
        let lower = content.to_lowercase();
        let pos = lower.find(&qlower).unwrap_or(0);
        let bytes_start = content.char_indices().nth(pos.saturating_sub(60)).map(|(i, _)| i).unwrap_or(0);
        let bytes_end = content
            .char_indices()
            .nth(pos + q.chars().count() + 60)
            .map(|(i, _)| i)
            .unwrap_or(content.len());
        let snippet = content[bytes_start..bytes_end].trim().to_string();
        Ok(MessageSearchHit {
            conversation_id: conv_id,
            conversation_title: title,
            message_id: id,
            role,
            created_at,
            snippet,
            match_start: pos,
            project_id: None,
            project_name: None,
        })
    };

    let hits = match conversation_id.as_deref() {
        Some(cid) => stmt
            .query_map(params![project_id, like, cid], mapper)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?,
        None => stmt
            .query_map(params![project_id, like], mapper)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?,
    };
    Ok(hits)
}

/// 跨项目消息全文检索：搜所有已添加项目的消息，结果按项目分组。
/// 用途：5+ 项目时找"哪个项目的哪个会话讲过某话题"。限制 100 条。
#[tauri::command]
pub fn search_messages_all_projects(
    query: String,
    state: State<DbState>,
) -> Result<Vec<MessageSearchHit>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let like = format!("%{q}%");
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    // JOIN projects 拿项目名；结果按时间倒序
    let mut stmt = conn
        .prepare(
            "SELECT m.id, m.conversation_id, c.title, m.role, m.content, m.created_at,
                    c.project_id, p.name
             FROM messages m
             JOIN conversations c ON c.id = m.conversation_id
             JOIN projects p ON p.id = c.project_id
             WHERE m.queued = 0 AND m.content LIKE ?1
             ORDER BY m.created_at DESC
             LIMIT 100",
        )
        .map_err(|e| e.to_string())?;
    let qlower = q.to_lowercase();
    let mapper = |row: &Row| -> rusqlite::Result<MessageSearchHit> {
        let id: String = row.get(0)?;
        let conv_id: String = row.get(1)?;
        let title: String = row.get(2)?;
        let role: String = row.get(3)?;
        let content: String = row.get(4)?;
        let created_at: i64 = row.get(5)?;
        let project_id: String = row.get(6)?;
        let project_name: String = row.get(7)?;
        let lower = content.to_lowercase();
        let pos = lower.find(&qlower).unwrap_or(0);
        let bytes_start = content
            .char_indices()
            .nth(pos.saturating_sub(60))
            .map(|(i, _)| i)
            .unwrap_or(0);
        let bytes_end = content
            .char_indices()
            .nth(pos + q.chars().count() + 60)
            .map(|(i, _)| i)
            .unwrap_or(content.len());
        let snippet = content[bytes_start..bytes_end].trim().to_string();
        Ok(MessageSearchHit {
            conversation_id: conv_id,
            conversation_title: title,
            message_id: id,
            role,
            created_at,
            snippet,
            match_start: pos,
            project_id: Some(project_id),
            project_name: Some(project_name),
        })
    };
    let rows = stmt
        .query_map([like], mapper)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// 发送消息（M0 仅入库展示；Agent 响应链路后续里程碑接入）
#[tauri::command]
pub fn send_message(
    conversation_id: String,
    content: String,
    state: State<DbState>,
) -> Result<ChatMessage, String> {
    let id = Uuid::new_v4().to_string();
    let ts = now();
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO messages (id, conversation_id, role, content, created_at)
         VALUES (?1, ?2, 'user', ?3, ?4)",
        params![id, conversation_id, content, ts],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
        params![ts, conversation_id],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);

    conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT id, conversation_id, role, content, references_json, model,
                tokens_in, tokens_out, created_at, reasoning, queued, agent_owned, modified_files_json, duration_ms
         FROM messages WHERE id = ?1",
        [id],
        row_to_message,
    )
    .map_err(|e| e.to_string())
}

// ---------- 目录探测辅助 ----------

/// 识别鸿蒙工程：查找 app.json5 / module.json5 / build-profile.json5
fn find_harmony_marker(root: &Path) -> bool {
    if root.join("app.json5").is_file() {
        return true;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir()
            && (p.join("module.json5").is_file()
                || p.join("build-profile.json5").is_file()
                || p.join("oh-package.json5").is_file())
        {
            return true;
        }
    }
    false
}

/// 文件计数（含子目录，上限保护）
fn count_files(dir: &Path, depth: u32, max: i64) -> i64 {
    if depth > 8 || max <= 0 {
        return 0;
    }
    let mut count = 0i64;
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        if count >= max {
            return count;
        }
        let p = entry.path();
        if p.is_dir() {
            count += count_files(&p, depth + 1, max - count);
        } else {
            count += 1;
        }
    }
    count
}

/// 判定相对路径是否命中"项目标识文件"（框架标志文件）。
/// 命中意味着项目身份/类型可能变化：新增 build-profile.json5 → 成为鸿蒙工程；
/// 删除 package.json → 不再是 Node 工程。排除构建/依赖目录下的同名文件。
fn is_project_identity_file(rel: &str) -> bool {
    let lower = rel.replace('\\', "/").to_lowercase();
    if [
        "node_modules/", "oh_modules/", ".hvigor/", "/build/", "/dist/", "/target/", ".git/", ".preview/",
    ]
    .iter()
    .any(|d| lower.contains(d))
    {
        return false;
    }
    let name = lower.rsplit('/').next().unwrap_or(&lower).to_string();
    const IDENTITY_FILES: &[&str] = &[
        "build-profile.json5",
        "oh-package.json5",
        "hvigorfile.ts",
        "hvigorfile.js",
        "package.json",
        "go.mod",
        "cargo.toml",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
        "pyproject.toml",
        "requirements.txt",
        "setup.py",
        "pipfile",
        "composer.json",
        "gemfile",
        "cmakelists.txt",
        "makefile",
        "pubspec.yaml",
        "pnpm-lock.yaml",
        "package-lock.json",
        "yarn.lock",
    ];
    IDENTITY_FILES.contains(&name.as_str())
        || (lower.contains("appscope/") && name == "app.json5")
        || (lower.contains("src/main/") && name == "module.json5")
        || lower.ends_with(".sln")
        || lower.ends_with(".csproj")
        || lower.ends_with(".xcodeproj")
        || lower.ends_with(".xcworkspace")
}

/// Agent 修改了项目标识文件后调用：重新分类项目类型、更新 DB 类型标签、
/// 广播 project-meta-changed 让前端刷新各处（对话框顶部徽标、概览、右侧栏）。
/// 智能化语义：删除框架标志文件 = 项目不再是该类型（如鸿蒙工程删掉
/// build-profile.json5/oh-package.json5 → 降级为普通目录，各处随之刷新）。
pub fn on_project_meta_files_changed(
    app: &AppHandle,
    project_id: &str,
    changed_paths: &[String],
    _roots: &[String],
    state: &DbState,
) {
    if project_id.is_empty() || !changed_paths.iter().any(|p| is_project_identity_file(p)) {
        return;
    }
    // 项目根以 DB 记录为准（roots 里可能混入 path_hints 的无关目录）
    let root: Option<PathBuf> = {
        let Ok(conn) = state.0.lock() else { return };
        conn.query_row("SELECT path FROM projects WHERE id = ?1", [project_id], |r| r.get::<_, String>(0))
            .ok()
            .map(PathBuf::from)
    };
    let Some(root) = root else { return };
    if !root.is_dir() {
        return;
    }
    let new_kind = crate::services::workspace::classify(&root);
    let db_kind = if new_kind == Some(crate::services::workspace::ModuleKind::Harmony) {
        "harmony"
    } else {
        "generic"
    };
    let old_kind: Option<String> = {
        let Ok(conn) = state.0.lock() else { return };
        conn.query_row("SELECT kind FROM projects WHERE id = ?1", [project_id], |r| r.get(0)).ok()
    };
    let identity_changed = old_kind.as_deref() != Some(db_kind);
    if identity_changed {
        let Ok(conn) = state.0.lock() else { return };
        let _ = conn.execute(
            "UPDATE projects SET kind = ?1 WHERE id = ?2",
            rusqlite::params![db_kind, project_id],
        );
        let _ = crate::agent::context::invalidate_project_facts(
            &conn,
            project_id,
            "project_identity_changed",
        );
        let _ = crate::agent::context::invalidate_project_memories(
            &conn,
            project_id,
            "project_identity_changed",
            &[],
        );
    }
    // 通知前端刷新：kind 变化时附带旧/新类型供提示
    let _ = app.emit(
        "project-meta-changed",
        serde_json::json!({
            "project_id": project_id,
            "old_kind": old_kind.unwrap_or_default(),
            "new_kind": db_kind,
            "classify": serde_json::to_string(&new_kind).unwrap_or_default(),
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 并行测试共用同 pid 时会争抢同一临时库文件，用原子序号唯一化
    static DB_SEQ: AtomicU32 = AtomicU32::new(0);

    fn test_conn() -> rusqlite::Connection {
        let mut dir = std::env::temp_dir();
        dir.push("deveco-switch-paging-test");
        std::fs::create_dir_all(&dir).unwrap();
        let db_path: PathBuf = dir.join(format!("paging-{}-{}.db", std::process::id(), DB_SEQ.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_file(&db_path);
        let m = init(&db_path).unwrap();
        let conn = m.into_inner().unwrap();
        std::fs::remove_file(&db_path).ok();
        std::fs::remove_file(db_path.with_extension("db-wal")).ok();
        std::fs::remove_file(db_path.with_extension("db-shm")).ok();
        conn
    }

    fn seed(conn: &rusqlite::Connection, conv_id: &str, count: i64) {
        conn.execute(
            "INSERT INTO projects (id, name, path, kind, trusted, index_state, created_at)
             VALUES ('p1', 't', 't', 'harmony', 1, 'ready', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO conversations (id, project_id, title, created_at, updated_at)
             VALUES (?1, 'p1', 't', 1, 1)",
            [conv_id],
        )
        .unwrap();
        for i in 1..=count {
            // id 前缀补零保证字典序 = 时间序；created_at 故意每 3 条同秒，验证行值游标
            let id = format!("m-{i:03}");
            let ts = 1000 + i / 3;
            conn.execute(
                "INSERT INTO messages (id, conversation_id, role, content, created_at)
                 VALUES (?1, ?2, 'user', ?3, ?4)",
                params![id, conv_id, format!("msg {i}"), ts],
            )
            .unwrap();
        }
    }

    #[test]
    fn first_page_returns_latest() {
        let conn = test_conn();
        seed(&conn, "c1", 20);
        let page = list_messages_page_impl(&conn, "c1", None, Some(10)).unwrap();
        assert!(page.has_more);
        let ids: Vec<&str> = page.messages.iter().map(|m| m.id.as_str()).collect();
        // 最近 10 条，正序（旧 → 新）
        assert_eq!(ids, vec!["m-011", "m-012", "m-013", "m-014", "m-015", "m-016", "m-017", "m-018", "m-019", "m-020"]);
    }

    #[test]
    fn cursor_pages_earlier() {
        let conn = test_conn();
        seed(&conn, "c1", 20);
        // 第一页游标 = 最早一条 m-011
        let page = list_messages_page_impl(&conn, "c1", Some("m-011"), Some(10)).unwrap();
        assert!(!page.has_more);
        let ids: Vec<&str> = page.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["m-001", "m-002", "m-003", "m-004", "m-005", "m-006", "m-007", "m-008", "m-009", "m-010"]);
    }

    #[test]
    fn same_second_cursor_no_dup_no_loss() {
        let conn = test_conn();
        seed(&conn, "c1", 30);
        // 同秒多条（每 3 条同秒）：逐页翻完，验证不重不漏
        let mut all: Vec<String> = Vec::new();
        let mut before: Option<String> = None;
        loop {
            let page = list_messages_page_impl(&conn, "c1", before.as_deref(), Some(10)).unwrap();
            for m in &page.messages {
                all.push(m.id.clone());
            }
            if !page.has_more {
                break;
            }
            before = page.messages.first().map(|m| m.id.clone());
        }
        assert_eq!(all.len(), 30);
        // 每页内部正序 + 块序（最新的页在前，即前端 prepend 顺序）验证：完整且不重不漏
        let mut uniq = all.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), 30, "不应有重复消息");
        assert_eq!(all[0], "m-021", "第一块应为最新的消息");
        assert_eq!(all[29], "m-010", "最旧页的最后一条应排在末尾");
        // 块内正序检查
        for chunk in all.chunks(10) {
            let mut sorted = chunk.to_vec();
            sorted.sort();
            assert_eq!(chunk.to_vec(), sorted, "块内应正序");
        }
    }

    #[test]
    fn empty_and_missing_cursor() {
        let conn = test_conn();
        seed(&conn, "c1", 5);
        // 空会话
        let page = list_messages_page_impl(&conn, "c-none", None, Some(10)).unwrap();
        assert!(page.messages.is_empty());
        assert!(!page.has_more);
        // 游标不存在（消息已删除）
        let page = list_messages_page_impl(&conn, "c1", Some("m-gone"), Some(10)).unwrap();
        assert!(page.messages.is_empty());
        assert!(!page.has_more);
    }

    #[test]
    fn page_size_clamped() {
        let conn = test_conn();
        seed(&conn, "c1", 5);
        let page = list_messages_page_impl(&conn, "c1", None, Some(0)).unwrap();
        assert_eq!(page.messages.len(), 1);
        let page = list_messages_page_impl(&conn, "c1", None, Some(9999)).unwrap();
        assert_eq!(page.messages.len(), 5);
    }

    #[test]
    fn branch_merge_copies_only_structured_whitelist() {
        let mut conn = test_conn();
        conn.execute(
            "INSERT INTO projects (id,name,path,kind,trusted,index_state,created_at) VALUES ('merge-p','p','p','harmony',1,'ready',1)",
            [],
        ).unwrap();
        conn.execute_batch(
            "INSERT INTO conversations(id,project_id,title,created_at,updated_at) VALUES ('parent','merge-p','parent',1,1);
             INSERT INTO conversations(id,project_id,title,created_at,updated_at) VALUES ('branch','merge-p','branch',1,1);
             INSERT INTO conversation_branches(id,source_conversation_id,branch_conversation_id,anchor_kind,created_at)
               VALUES ('lineage','parent','branch','latest',1);
             INSERT INTO conversation_context_pins(id,conversation_id,project_id,pin_kind,source_ref,label,content,created_at,updated_at)
               VALUES ('decision','branch','merge-p','decision','d','decision','use api',1,1),
                      ('message','branch','merge-p','message','m','message','free form',1,1);
             INSERT INTO conversation_context_artifacts(id,conversation_id,artifact_kind,uri,label,metadata_json,source_ref,valid,created_at,updated_at)
               VALUES ('artifact','branch','file','src/a.ets','a','{}','tool:a',1,1,1);
             INSERT INTO conversation_context_facts(id,conversation_id,project_id,fact_kind,fact_key,value_json,source_kind,source_ref,scope,confidence,version,observed_at,created_at,updated_at)
               VALUES ('verified','branch','merge-p','verification','tests','{\"passed\":true}','tool_run','tool:t','project',1,1,1,1,1),
                      ('other','branch','merge-p','note','summary','\"text\"','summary','message:m','conversation',0.5,1,1,1,1);",
        ).unwrap();
        let result = merge_conversation_branch_conn(&mut conn, "branch", "parent").unwrap();
        assert_eq!((result.decisions_merged, result.artifacts_merged, result.evidence_merged), (1, 1, 1));
        let pin_kinds: Vec<String> = conn.prepare(
            "SELECT pin_kind FROM conversation_context_pins WHERE conversation_id='parent' ORDER BY pin_kind",
        ).unwrap().query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(pin_kinds, vec!["decision"]);
        let fact_kinds: Vec<String> = conn.prepare(
            "SELECT fact_kind FROM conversation_context_facts WHERE conversation_id='parent' ORDER BY fact_kind",
        ).unwrap().query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(fact_kinds, vec!["verification"]);
        let manifest: String = conn.query_row(
            "SELECT manifest_json FROM conversation_branch_merges WHERE id=?1",
            [&result.merge_id], |row| row.get(0),
        ).unwrap();
        assert!(manifest.contains("free_form_output"));
    }

    #[test]
    fn branch_anchors_resolve_checkpoint_build_failure_and_git_commit() {
        let conn = test_conn();
        seed(&conn, "anchors", 20);
        let checkpoint_rowid: i64 = conn.query_row(
            "SELECT rowid FROM messages WHERE id='m-010'", [], |row| row.get(0),
        ).unwrap();
        conn.execute(
            "INSERT INTO conversation_snapshots(id,conversation_id,msg_rowid,label,tool_count,created_at)
             VALUES ('snap','anchors',?1,'checkpoint',1,1)",
            [checkpoint_rowid],
        ).unwrap();
        conn.execute(
            "INSERT INTO tool_runs(id,conversation_id,tool_name,result_json,status,created_at)
             VALUES ('build-fail','anchors','build_project','failed','error',1004)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO tool_runs(id,conversation_id,tool_name,result_json,status,created_at)
             VALUES ('commit-tool','anchors','git_commit','commit abc123','ok',1005)",
            [],
        ).unwrap();

        assert_eq!(
            resolve_conversation_branch_anchor(&conn, "anchors", "checkpoint", Some("snap")).unwrap().unwrap().1,
            checkpoint_rowid,
        );
        let build = resolve_conversation_branch_anchor(&conn, "anchors", "build_failure", Some("build-fail"))
            .unwrap().unwrap();
        let commit = resolve_conversation_branch_anchor(&conn, "anchors", "git_commit", Some("abc123"))
            .unwrap().unwrap();
        assert!(build.0 <= 1004);
        assert!(commit.0 <= 1005);
        assert!(commit.1 >= build.1);
    }
}
