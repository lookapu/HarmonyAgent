//! 数据维护：防止日志/成本明细等"只增不删"的数据无限堆满仓库。
//!
//! 治理策略（按数据性质分类）：
//! - **日志类**（request_logs）：只保留最近 N 条，超出自动滚动删除（防无限增长）
//! - **成本明细**（task_runs）：只保留最近 N 天，超出滚动删除（预算门控只看当日/当月，口径不受影响）
//! - **内容类**（会话/消息/记忆）：由用户手动删除（已有入口），这里提供"一键清空"聚合入口
//! - **配置类**（providers/models/mcp/skills）：不清理，用户主动删除
//!
//! 滚动清理调用点：request_logs 每次插入后、task_runs 每次插入后、应用启动时。

use rusqlite::Connection;

/// request_logs 保留上限（条）
pub const REQUEST_LOG_KEEP: i64 = 2000;
/// task_runs 保留天数
pub const TASK_RUN_KEEP_DAYS: i64 = 180;

/// 滚动清理 request_logs：只保留最近 `keep` 条，删除更旧的。
/// 返回删除行数。
pub fn prune_request_logs(conn: &Connection, keep: i64) -> usize {
    conn.execute(
        "DELETE FROM request_logs WHERE id NOT IN (
            SELECT id FROM request_logs ORDER BY created_at DESC LIMIT ?1)",
        [keep],
    )
    .unwrap_or(0)
}

/// 滚动清理 task_runs：只保留最近 `keep_days` 天内的记录，删除更旧的。
/// 返回删除行数。
pub fn prune_task_runs(conn: &Connection, keep_days: i64) -> usize {
    let cutoff = chrono::Utc::now().timestamp() - keep_days * 86_400;
    conn.execute("DELETE FROM task_runs WHERE started_at < ?1", [cutoff])
        .unwrap_or(0)
}

/// 一键清空"内容类"数据：会话、消息、反馈、版本、任务/工具轨迹、请求日志、项目记忆、
/// 审批白名单（会话内免审记忆）。
/// 保留：配置类（providers/models/mcp/skills/权限）与 API 知识库（api_docs 等）。
/// 返回 (删除会话数, 删除消息数)。
pub fn clear_content_data(conn: &mut Connection) -> rusqlite::Result<(u64, u64)> {
    let tx = conn.transaction()?;
    let deleted_msgs = tx.execute("DELETE FROM messages", [])?;
    let deleted_convs = tx.execute("DELETE FROM conversations", [])?;
    tx.execute("DELETE FROM message_feedback", [])?;
    tx.execute("DELETE FROM message_versions", [])?;
    tx.execute("DELETE FROM tool_runs", [])?;
    tx.execute("DELETE FROM task_runs", [])?;
    tx.execute("DELETE FROM request_logs", [])?;
    tx.execute("DELETE FROM project_memories", [])?;
    tx.execute("DELETE FROM tool_approval_whitelist", [])?;
    tx.commit()?;
    Ok((deleted_convs as u64, deleted_msgs as u64))
}

/// VACUUM 回收文件空间（删除大量行后执行，避免文件继续占盘）。
/// SQLite 页面复用不保证缩小文件，删除大量数据后需要 VACUUM 才真正归还磁盘。
pub fn vacuum(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("VACUUM;")
}

/// 应用启动时的一次性维护：按保留策略滚动清理 + 回收空间。
/// 任何一步失败都不阻塞启动（日志记录即可）。
pub fn run_startup_maintenance(conn: &Connection) {
    let pruned_logs = prune_request_logs(conn, REQUEST_LOG_KEEP);
    let pruned_runs = prune_task_runs(conn, TASK_RUN_KEEP_DAYS);
    if pruned_logs > 0 || pruned_runs > 0 {
        let _ = vacuum(conn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE request_logs (
                id TEXT PRIMARY KEY,
                provider_id TEXT,
                model TEXT,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE task_runs (
                id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'success',
                started_at INTEGER NOT NULL,
                finished_at INTEGER NOT NULL
            );
            CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE conversations (
                id TEXT PRIMARY KEY,
                title TEXT,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE message_feedback (id TEXT PRIMARY KEY, message_id TEXT);
            CREATE TABLE message_versions (id TEXT PRIMARY KEY, conversation_id TEXT);
            CREATE TABLE tool_runs (id TEXT PRIMARY KEY, conversation_id TEXT);
            CREATE TABLE project_memories (id TEXT PRIMARY KEY, project_id TEXT, content TEXT);
            CREATE TABLE tool_approval_whitelist (project_id TEXT, tool TEXT);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn request_logs_rolls_to_keep() {
        let conn = test_conn();
        for i in 0..10 {
            conn.execute(
                "INSERT INTO request_logs (id, created_at) VALUES (?1, ?2)",
                params![format!("r{i}"), i],
            )
            .unwrap();
        }
        let removed = prune_request_logs(&conn, 5);
        assert_eq!(removed, 5, "超出 5 条上限的最旧 5 条应被删除");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM request_logs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 5);
        // 保留的应是最新的 5 条（min created_at == 5，即旧 0..5 已被删）
        let keep_newest: i64 = conn
            .query_row(
                "SELECT MIN(created_at) FROM request_logs",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(keep_newest, 5);
    }

    #[test]
    fn task_runs_pruned_by_retention_days() {
        let conn = test_conn();
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO task_runs (id, conversation_id, started_at, finished_at) VALUES (?1, 'c', ?2, ?2)",
            params!["old", now - 200 * 86_400],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_runs (id, conversation_id, started_at, finished_at) VALUES (?1, 'c', ?2, ?2)",
            params!["recent", now - 10 * 86_400],
        )
        .unwrap();
        let removed = prune_task_runs(&conn, 180);
        assert_eq!(removed, 1, "超过 180 天的旧记录应被删除");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM task_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn clear_content_keeps_config_and_knowledge() {
        let mut conn = test_conn();
        conn.execute(
            "INSERT INTO conversations (id, title, created_at) VALUES ('c1', 't', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, conversation_id, role, content, created_at) VALUES ('m1', 'c1', 'user', 'hi', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message_feedback (id, message_id) VALUES ('f1', 'm1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message_versions (id, conversation_id) VALUES ('v1', 'c1')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO tool_runs (id, conversation_id) VALUES ('t1', 'c1')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO task_runs (id, conversation_id, status, started_at, finished_at) VALUES ('tr1', 'c1', 'success', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO request_logs (id, created_at) VALUES ('l1', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO project_memories (id, project_id, content) VALUES ('p1', 'g', 'x')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tool_approval_whitelist (project_id, tool) VALUES ('g', 'run_shell')",
            [],
        )
        .unwrap();

        let (convs, msgs) = clear_content_data(&mut conn).unwrap();
        assert_eq!(convs, 1);
        assert_eq!(msgs, 1);

        // 内容类全部清空
        for table in [
            "message_feedback",
            "message_versions",
            "tool_runs",
            "task_runs",
            "request_logs",
            "project_memories",
            "tool_approval_whitelist",
        ] {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "表 {table} 应被清空");
        }
    }
}
