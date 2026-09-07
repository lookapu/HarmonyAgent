//! Headless 工具运行时：集中管理 workspace、数据库、MCP 和取消边界。

use crate::agent::exec_ctx::ToolCtx;
use crate::agent::tools::run_tool;
use crate::db::DbState;
use crate::services::mcp_manager::McpManager;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct HeadlessToolPolicy;

impl HeadlessToolPolicy {
    pub fn allows(self, name: &str) -> bool {
        matches!(
            name,
            "list_dir"
                | "read_file"
                | "write_file"
                | "preview_edit"
                | "edit_file"
                | "find_files"
                | "grep_files"
                | "search_symbols"
                | "repo_query"
                | "git_status"
                | "git_diff"
        )
    }
}

pub struct HeadlessToolRuntime {
    pub db: DbState,
    pub project_root: PathBuf,
    pub mcp: McpManager,
    pub policy: HeadlessToolPolicy,
    cancelled: Arc<AtomicBool>,
}

impl HeadlessToolRuntime {
    pub fn new(project_root: &Path) -> Result<Self, String> {
        let root = project_root
            .canonicalize()
            .map_err(|e| format!("headless workspace 不可访问：{e}"))?;
        if !root.is_dir() {
            return Err("headless workspace 必须是目录".into());
        }
        let conn =
            Connection::open_in_memory().map_err(|e| format!("创建 headless 工具库失败：{e}"))?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")
            .map_err(|e| format!("配置 headless 工具库失败：{e}"))?;
        crate::db::run_migrations(&conn)
            .map_err(|e| format!("初始化 headless 工具库 schema 失败：{e}"))?;
        Ok(Self {
            db: DbState(Arc::new(Mutex::new(conn))),
            project_root: root,
            mcp: McpManager::default(),
            policy: HeadlessToolPolicy,
            cancelled: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub async fn execute(&self, name: &str, args: &str) -> Result<String, String> {
        if self.is_cancelled() {
            return Err("headless 工具执行已取消".into());
        }
        if !self.policy.allows(name) {
            return Err(format!("headless policy rejected tool: {name}"));
        }
        run_tool(
            name,
            args,
            &self.project_root.to_string_lossy(),
            &[],
            "headless",
            &self.db,
            &self.mcp,
            &ToolCtx::empty(),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_cancellation_is_sticky() {
        let runtime = HeadlessToolRuntime::new(Path::new(".")).unwrap();
        assert!(!runtime.is_cancelled());
        runtime.cancel();
        assert!(runtime.is_cancelled());
    }

    #[test]
    fn runtime_rejects_unknown_tools_before_dispatch() {
        let runtime = HeadlessToolRuntime::new(Path::new(".")).unwrap();
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(runtime.execute("run_command", "{}"));
        assert!(result.unwrap_err().contains("rejected"));
    }

    #[test]
    fn runtime_database_has_full_application_schema() {
        let runtime = HeadlessToolRuntime::new(Path::new(".")).unwrap();
        let conn = runtime.db.0.lock().unwrap();
        let applied: usize = conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(applied, crate::db::MIGRATIONS.len());
        let session_events_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='session_events'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(session_events_exists);
    }
}
