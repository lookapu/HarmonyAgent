use crate::db::models::{ChatMessage, Conversation, Project, ProjectInspect};
use crate::db::DbState;
use crate::utils::path::normalize_path;
use rusqlite::{params, Row};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State};
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
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
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
        "SELECT id, name, path, kind, trusted, default_provider_id, default_model_id,
                index_state, rules, last_opened_at, created_at, worktree_path, harmony_subprojects, workspace_modules,
                harmony_project_path
         FROM projects WHERE id = ?1",
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
            "SELECT id, name, path, kind, trusted, default_provider_id, default_model_id,
                    index_state, rules, last_opened_at, created_at, worktree_path, harmony_subprojects, workspace_modules,
                    harmony_project_path
         FROM projects
             ORDER BY CASE WHEN kind = 'global' THEN 1 ELSE 0 END,
                      COALESCE(last_opened_at, created_at) DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], row_to_project)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
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
pub fn add_project(path: String, state: State<DbState>) -> Result<Project, String> {
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
#[tauri::command]
pub fn delete_project(
    app: AppHandle,
    id: String,
    state: State<DbState>,
    manager: State<'_, crate::services::mcp_manager::McpManager>,
) -> Result<(), String> {
    // 先取出项目路径与项目级配置清理信息，删除后清理符号磁盘缓存与 MCP 连接
    let (path, mcp_ids, skill_dirs, repo_roots) = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        let p: Option<String> = conn
            .query_row("SELECT path FROM projects WHERE id = ?1", [&id], |r| r.get::<_, String>(0))
            .ok();
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
        (p, mcp_ids, skill_dirs, repo_roots)
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
/// 1) 已配置 harmony_project_path → 直接解析（相对项目根拼接，校验目录存在）；
/// 2) 未配置 → 若工作区仅扫描到一个鸿蒙模块，自动兜底使用它；
/// 3) 否则回退到项目根本身。
pub fn resolve_harmony_root(
    conn: &rusqlite::Connection,
    project_id: &str,
) -> Result<HarmonyRootInfo, String> {
    let (root_path, configured, harmony_json) = conn
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

    // 1) 显式配置优先
    if let Some(cfg) = configured.as_deref().filter(|s| !s.trim().is_empty()) {
        let joined = if Path::new(cfg).is_absolute() {
            PathBuf::from(cfg)
        } else {
            root.join(cfg.replace('\\', "/"))
        };
        if joined.is_dir() {
            return Ok(HarmonyRootInfo {
                root: normalize_path(&joined.to_string_lossy()),
                configured: Some(cfg.to_string()),
                candidates,
                auto: false,
            });
        }
        // 配置的目录已失效：回退自动兜底（保留 configured 供前端修正）
    }
    // 2) 自动兜底：恰好一个鸿蒙模块
    if candidates.len() == 1 {
        return Ok(HarmonyRootInfo {
            root: candidates[0].clone(),
            configured: None,
            candidates,
            auto: true,
        });
    }
    // 3) 回退项目根
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
) -> Result<HarmonyRootInfo, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    resolve_harmony_root(&conn, &project_id)
}

/// 设置会话"鸿蒙主工程"（空串=清除，回退项目根本身）；返回更新后的项目
#[tauri::command]
pub fn set_harmony_project_path(
    state: State<DbState>,
    project_id: String,
    path: String,
) -> Result<Project, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let root_path: String = conn
        .query_row(
            "SELECT path FROM projects WHERE id = ?1",
            [&project_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
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

/// 获取项目的 Git 分支列表与当前分支
#[tauri::command]
pub fn get_git_branches(project_id: String, state: State<DbState>) -> Result<GitBranchInfo, String> {
    let project = get_project_by_id(&state, &project_id)?;
    if project.path.is_empty() || !Path::new(&project.path).join(".git").is_dir() {
        return Ok(GitBranchInfo {
            has_git: false,
            current: None,
            branches: vec![],
            error: None,
        });
    }
    let dir = Path::new(&project.path);
    let current = run_git(dir, &["symbolic-ref", "--short", "HEAD"])
        .ok()
        .filter(|s| !s.is_empty());
    let out = run_git(dir, &["branch", "--format", "%(refname:short)"])?;
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

/// 切换 Git 分支，返回切换后的最新分支状态
#[tauri::command]
pub fn switch_git_branch(
    project_id: String,
    branch: String,
    state: State<DbState>,
) -> Result<GitBranchInfo, String> {
    let project = get_project_by_id(&state, &project_id)?;
    if project.path.is_empty() {
        return Err("当前项目没有 Git 仓库".into());
    }
    let dir = Path::new(&project.path);
    let msg = run_git(dir, &["switch", &branch])?;
    let info = get_git_branches(project_id, state)?;
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
                    is_pinned, archived, created_at, updated_at
             FROM conversations WHERE project_id = ?1 AND archived = ?2
             ORDER BY is_pinned DESC, updated_at DESC"
                .to_string(),
            rusqlite::params![project_id, include_archived as i64],
        )
    } else {
        (
            "SELECT c.id, c.project_id, c.title, c.provider_id, c.model_id, c.system_prompt_version,
                    c.is_pinned, c.archived, c.created_at, c.updated_at
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

/// 更新会话：置顶 / 归档 / 绑定模型（空串清除绑定，None 不改）
#[tauri::command]
pub fn update_conversation(
    id: String,
    state: State<DbState>,
    is_pinned: Option<bool>,
    archived: Option<bool>,
    model_id: Option<String>,
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
    conn.query_row(
        "SELECT id, project_id, title, provider_id, model_id, system_prompt_version,
                is_pinned, archived, created_at, updated_at
         FROM conversations WHERE id = ?1",
        [id],
        row_to_conversation,
    )
    .map_err(|e| e.to_string())
}

/// 新建会话（默认标题"新会话"）
#[tauri::command]
pub fn create_conversation(
    project_id: String,
    title: Option<String>,
    state: State<DbState>,
) -> Result<Conversation, String> {
    let id = Uuid::new_v4().to_string();
    let ts = now();
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO conversations (id, project_id, title, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)",
        params![id, project_id, title.unwrap_or_else(|| "新会话".into()), ts],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
    let conversations = list_conversations(project_id, state.clone(), None, None)?;
    conversations
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| "会话创建失败".into())
}

/// 消息列表（正序）
#[tauri::command]
pub fn list_messages(conversation_id: String, state: State<DbState>) -> Result<Vec<ChatMessage>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, conversation_id, role, content, references_json, model,
                    tokens_in, tokens_out, created_at, reasoning, queued, agent_owned, modified_files_json, duration_ms
             FROM messages WHERE conversation_id = ?1 ORDER BY created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([conversation_id], row_to_message)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
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
