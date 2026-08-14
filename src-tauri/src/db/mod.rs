pub mod ids;
pub mod models;
pub mod queries;

use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// 应用全局数据库状态：Arc 允许后台线程（如向量索引重建）短暂持锁分批读写。
pub struct DbState(pub Arc<Mutex<Connection>>);

pub fn init(path: &Path) -> Result<Mutex<Connection>, rusqlite::Error> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    run_migrations(&conn)?;
    Ok(Mutex::new(conn))
}

fn run_migrations(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at INTEGER NOT NULL
        );"
    )?;

    let migrations = [
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
    ];

    for (id, name, sql) in migrations {
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
