pub mod ids;
pub mod models;
pub mod queries;

use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

/// 应用全局数据库状态：Arc 允许后台线程（如向量索引重建）短暂持锁分批读写。
pub struct DbState(pub Arc<Mutex<Connection>>);

/// 全局 DB 单例：供无 tauri State 上下文的模块（todo 持久化、诊断缓存等）访问同一连接。
/// 由 lib.rs 在启动时经 register_global 注册；未注册时返回 None（如 full_fetch 工具进程）。
static GLOBAL_DB: OnceLock<Arc<Mutex<Connection>>> = OnceLock::new();

pub fn register_global(conn: Arc<Mutex<Connection>>) {
    let _ = GLOBAL_DB.set(conn);
}

/// 获取全局 DB 连接（Arc 克隆，锁语义与 tauri 托管 DbState 一致）。
pub fn global() -> Option<Arc<Mutex<Connection>>> {
    GLOBAL_DB.get().cloned()
}

pub fn init(path: &Path) -> Result<Mutex<Connection>, rusqlite::Error> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    run_migrations(&conn)?;
    crate::agent::coordinator::recover_interrupted_steps(&conn)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(e))))?;
    recover_interrupted_tool_runs(&conn)?;
    // 上次进程已经不存在，非终态 durable run 不得继续伪装为运行中。
    // 恢复失败不应静默启动；映射回 rusqlite 错误较笨重，因此使用 SQL 侧同等收敛，
    // 详细 run.recovered 事件由 runtime 恢复函数在正常路径补齐。
    crate::agent::runtime::recover_interrupted_runs(&conn)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(e))))?;
    Ok(Mutex::new(conn))
}

/// 应用异常退出时，执行前已落库但未写终态的工具会残留 running。启动恢复将其明确
/// 标记为 interrupted，并按副作用策略说明后续动作；绝不把未知副作用调用自动当作失败重跑。
fn recover_interrupted_tool_runs(conn: &Connection) -> Result<usize, rusqlite::Error> {
    let prepared = conn.execute(
        "UPDATE tool_runs SET status='cancelled', finished_at=unixepoch(),
         result_json=COALESCE(NULLIF(result_json, ''), '应用退出时工具尚未开始执行')
         WHERE status='prepared'",
        [],
    )?;
    let running = conn.execute(
        "UPDATE tool_runs SET status='interrupted', finished_at=unixepoch(),
         result_json=COALESCE(NULLIF(result_json, ''),
           CASE recovery_policy
             WHEN 'replay' THEN '应用退出导致调用中断；该工具为只读，可安全重新执行'
             WHEN 'verify' THEN '应用退出导致调用中断，实际状态未知；恢复前必须核验副作用'
             ELSE '应用退出导致调用中断，实际状态未知；需要用户确认后处理'
           END)
         WHERE status='running'",
        [],
    )?;
    Ok(prepared + running)
}

/// 全部迁移清单（id, 名称, SQL）。启动迁移与 db_migrate 工具共用同一清单。
pub static MIGRATIONS: &[(i64, &str, &str)] = &[
    (1, "001_initial", include_str!("../../migrations/001_initial.sql")),
    (2, "002_request_logs", include_str!("../../migrations/002_request_logs.sql")),
    (3, "003_mcp_skills", include_str!("../../migrations/003_mcp_skills.sql")),
    (4, "004_agent", include_str!("../../migrations/004_agent.sql")),
    (5, "005_provider_proxy", include_str!("../../migrations/005_provider_proxy.sql")),
    (6, "006_conversation_skills", include_str!("../../migrations/006_conversation_skills.sql")),
    (7, "007_model_enabled", include_str!("../../migrations/007_model_enabled.sql")),
    (8, "008_project_memories", include_str!("../../migrations/008_project_memories.sql")),
    (9, "009_provider_endpoints", include_str!("../../migrations/009_provider_endpoints.sql")),
    (10, "010_task_runs", include_str!("../../migrations/010_task_runs.sql")),
    (11, "011_proxy_auto_start", include_str!("../../migrations/011_proxy_auto_start.sql")),
    (12, "012_message_feedback_versions", include_str!("../../migrations/012_message_feedback_versions.sql")),
    (13, "013_message_queue", include_str!("../../migrations/013_message_queue.sql")),
    (14, "014_conversation_summary", include_str!("../../migrations/014_conversation_summary.sql")),
    (15, "015_message_modified_files", include_str!("../../migrations/015_message_modified_files.sql")),
    (16, "016_project_worktree", include_str!("../../migrations/016_project_worktree.sql")),
    (17, "017_mcp_server_health", include_str!("../../migrations/017_mcp_server_health.sql")),
    (18, "018_message_duration", include_str!("../../migrations/018_message_duration.sql")),
    (19, "019_drop_endpoint_health", include_str!("../../migrations/019_drop_endpoint_health.sql")),
    (20, "020_cleanup_ghosts", include_str!("../../migrations/020_cleanup_ghosts.sql")),
    (21, "021_harmony_subprojects", include_str!("../../migrations/021_harmony_subprojects.sql")),
    (22, "022_workspace_modules", include_str!("../../migrations/022_workspace_modules.sql")),
    (23, "023_tool_approval_whitelist", include_str!("../../migrations/023_tool_approval_whitelist.sql")),
    (24, "024_compact_keep", include_str!("../../migrations/024_compact_keep.sql")),
    (25, "025_scope_project", include_str!("../../migrations/025_scope_project.sql")),
    (26, "026_knowledge_entries", include_str!("../../migrations/026_knowledge_entries.sql")),
    (27, "027_knowledge_hit_count", include_str!("../../migrations/027_knowledge_hit_count.sql")),
    (28, "028_api_docs", include_str!("../../migrations/028_api_docs.sql")),
    (29, "029_api_details", include_str!("../../migrations/029_api_details.sql")),
    (30, "030_project_harmony_root", include_str!("../../migrations/030_project_harmony_root.sql")),
    (31, "031_api_docs_embeddings", include_str!("../../migrations/031_api_docs_embeddings.sql")),
    (32, "032_session_events", include_str!("../../migrations/032_session_events.sql")),
    (33, "033_conversation_todos", include_str!("../../migrations/033_conversation_todos.sql")),
    (34, "034_session_events_trace", include_str!("../../migrations/034_session_events_trace.sql")),
    (35, "035_skill_repo_host", include_str!("../../migrations/035_skill_repo_host.sql")),
    (36, "036_snippets", include_str!("../../migrations/036_snippets.sql")),
    (37, "037_request_logs_tool", include_str!("../../migrations/037_request_logs_tool.sql")),
    (38, "038_conversation_tags", include_str!("../../migrations/038_conversation_tags.sql")),
    (39, "039_message_paging_index", include_str!("../../migrations/039_message_paging_index.sql")),
    (40, "040_conversation_worktree", include_str!("../../migrations/040_conversation_worktree.sql")),
    (41, "041_skill_usage", include_str!("../../migrations/041_skill_usage.sql")),
    (42, "042_lan_config", include_str!("../../migrations/042_lan_config.sql")),
    (43, "043_lan_tokens", include_str!("../../migrations/043_lan_tokens.sql")),
    (44, "044_lan_sessions", include_str!("../../migrations/044_lan_sessions.sql")),
    (45, "045_model_sort_order", include_str!("../../migrations/045_model_sort_order.sql")),
    (46, "046_lan_token_plain", include_str!("../../migrations/046_lan_token_plain.sql")),
    (47, "047_model_default_dedupe", include_str!("../../migrations/047_model_default_dedupe.sql")),
    (48, "048_ohpm_landscape", include_str!("../../migrations/048_ohpm_landscape.sql")),
    (49, "049_ohpm_landscape_sort", include_str!("../../migrations/049_ohpm_landscape_sort.sql")),
    (50, "050_task_ledger", include_str!("../../migrations/050_task_ledger.sql")),
    (51, "051_conversation_snapshots", include_str!("../../migrations/051_conversation_snapshots.sql")),
    (52, "052_reminders_feedback_terms", include_str!("../../migrations/052_reminders_feedback_terms.sql")),
    (53, "053_tool_run_lifecycle", include_str!("../../migrations/053_tool_run_lifecycle.sql")),
    (54, "054_agent_runtime", include_str!("../../migrations/054_agent_runtime.sql")),
    (55, "055_execution_steps", include_str!("../../migrations/055_execution_steps.sql")),
    (56, "056_recovery_orchestrator", include_str!("../../migrations/056_recovery_orchestrator.sql")),
];

fn run_migrations(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at INTEGER NOT NULL
        );"
    )?;

    for (id, name, sql) in MIGRATIONS {
        let applied: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM _migrations WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !applied {
            // 每条迁移独立事务：执行失败回滚，避免部分应用造成 schema 不一致
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO _migrations (id, name, applied_at) VALUES (?1, ?2, unixepoch())",
                rusqlite::params![id, name],
            )?;
            tx.commit()?;
        }
    }

    Ok(())
}

/// 查询迁移状态：返回 (id, name, 是否已应用, 应用时间)
pub fn migration_status(conn: &Connection) -> Result<Vec<(i64, String, bool, Option<i64>)>, rusqlite::Error> {
    let mut out = Vec::with_capacity(MIGRATIONS.len());
    for (id, name, _) in MIGRATIONS {
        let (applied, at): (bool, Option<i64>) = conn
            .query_row(
                "SELECT COUNT(*) > 0, MAX(applied_at) FROM _migrations WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((false, None));
        out.push((*id, name.to_string(), applied, at));
    }
    Ok(out)
}

/// 应用所有未执行的迁移，返回本次应用的数量（db_migrate 工具入口）
pub fn apply_pending_migrations(conn: &Connection) -> Result<usize, rusqlite::Error> {
    let before: usize = conn
        .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
        .unwrap_or(0);
    run_migrations(conn)?;
    let after: usize = conn
        .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
        .unwrap_or(0);
    Ok(after.saturating_sub(before))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 迁移清单必须完整注册：验证关键后期迁移列，防止新增迁移文件却未登记。
    #[test]
    fn migrations_apply_ohpm_sort_columns() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(ohpm_landscape)")
            .unwrap()
            .query_map([], |r| r.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for c in ["likes", "popularity", "latest_publish_time"] {
            assert!(cols.iter().any(|x| x == c), "迁移后缺少列 {c}: {cols:?}");
        }
        let tool_cols: Vec<String> = conn
            .prepare("PRAGMA table_info(tool_runs)")
            .unwrap()
            .query_map([], |r| r.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for c in ["trace_id", "call_id", "idempotency_key", "effect_kind", "recovery_policy"] {
            assert!(tool_cols.iter().any(|x| x == c), "tool_runs 迁移后缺少列 {c}: {tool_cols:?}");
        }
        let run_tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('agent_runs','run_events')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(run_tables, 2);
        let step_tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='execution_steps'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(step_tables, 1);
        let run_cols: Vec<String> = conn
            .prepare("PRAGMA table_info(agent_runs)")
            .unwrap()
            .query_map([], |r| r.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for c in ["parent_run_id", "recovery_plan_json", "recovery_mode"] {
            assert!(
                run_cols.iter().any(|x| x == c),
                "agent_runs 迁移后缺少列 {c}: {run_cols:?}"
            );
        }
    }

    #[test]
    fn startup_recovers_unfinished_tool_audit_rows() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO projects (id,name,path,kind,trusted,created_at) VALUES ('p','p','/tmp/p','other',0,0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO conversations (id,project_id,title,created_at,updated_at) VALUES ('c','p','c',0,0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tool_runs (id,conversation_id,tool_name,status,created_at,trace_id,call_id)
             VALUES ('call','c','run_command','running',0,'trace','call')",
            [],
        )
        .unwrap();
        assert_eq!(recover_interrupted_tool_runs(&conn).unwrap(), 1);
        let row: (String, String) = conn
            .query_row(
                "SELECT status,result_json FROM tool_runs WHERE id='call'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row.0, "interrupted");
        assert!(row.1.contains("中断"));
    }
}
