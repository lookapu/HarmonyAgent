//! Headless 工具运行时：集中管理 workspace、数据库、MCP 和取消边界。

use crate::agent::exec_ctx::ToolCtx;
use crate::agent::tools::contracts::{ApprovalPolicy, ToolContract};
use crate::agent::tools::{is_retryable_err, run_tool_boxed};
use crate::db::DbState;
use crate::services::mcp_manager::McpManager;
use crate::services::permissions::{self, Level};
use crate::utils::retry::{retry_with_backoff, RetryResult, TOOL_POLICY};
use rusqlite::Connection;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

/// 与桌面 UI tool loop 相同的自动重试语义：契约 retry_safe + 可恢复错误白名单 +
/// 指数退避。生产路径传 `TOOL_POLICY`；`attempt` 负责一次执行（含取消与剩余
/// wall time 检查）。
pub(crate) async fn run_tool_with_retry<F, Fut>(
    contract: &ToolContract,
    policy: &crate::utils::retry::RetryPolicy,
    mut attempt: F,
) -> RetryResult<String, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    retry_with_backoff(
        policy,
        &mut attempt,
        |error: &String| contract.retry_safe && is_retryable_err(error),
        |_| None,
    )
    .await
}

/// 重试成功后的模型可见提示，与 UI 的措辞保持一致。
pub(crate) fn retry_notice(output: String, attempts: usize) -> String {
    if attempts > 1 {
        format!("（首次执行超时/网络错误，已自动重试 {} 次）\n{output}", attempts - 1)
    } else {
        output
    }
}

fn audit_preview(value: &str) -> String {
    serde_json::from_str::<serde_json::Value>(value)
        .map(|json| crate::utils::redact::redact_json_value(&json).to_string())
        .unwrap_or_else(|_| crate::utils::redact::redact_text(value))
        .chars()
        .take(4_000)
        .collect()
}

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

    pub fn check(self, name: &str, args: &str) -> Result<ToolContract, String> {
        if !self.allows(name) {
            return Err(format!("headless allowlist 未授权工具：{name}"));
        }
        let parsed: serde_json::Value =
            serde_json::from_str(if args.trim().is_empty() { "{}" } else { args })
                .map_err(|error| format!("工具 {name} 参数不是合法 JSON：{error}"))?;
        if !parsed.is_object() {
            return Err(format!("工具 {name} 参数必须是 JSON object"));
        }
        if permissions::requires_fresh_explicit_approval(name, &parsed) {
            return Err(format!(
                "工具 {name} 的本次参数要求逐次显式审批，headless 模式拒绝执行"
            ));
        }
        if permissions::tool_level(name) == Level::L2 {
            return Err(format!("工具 {name} 属于 L2，headless 模式拒绝执行"));
        }
        let contract = crate::agent::tools::contracts::contract(name);
        if contract.approval == ApprovalPolicy::Always {
            return Err(format!(
                "工具 {name} 的契约要求每次审批，headless 模式拒绝执行"
            ));
        }
        Ok(contract)
    }
}

pub struct HeadlessToolRuntime {
    pub db: DbState,
    pub project_root: PathBuf,
    pub project_id: String,
    pub conversation_id: String,
    pub trace_id: String,
    pub mcp: McpManager,
    pub policy: HeadlessToolPolicy,
    cancelled: Arc<AtomicBool>,
}

impl HeadlessToolRuntime {
    pub fn new(project_root: &Path) -> Result<Self, String> {
        Self::new_scoped(
            project_root,
            format!("headless-conversation-{}", uuid::Uuid::new_v4()),
            format!("headless-trace-{}", uuid::Uuid::new_v4()),
        )
    }

    pub fn new_scoped(
        project_root: &Path,
        conversation_id: String,
        trace_id: String,
    ) -> Result<Self, String> {
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
        let project_id = format!("headless-project-{}", uuid::Uuid::new_v4());
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO projects (id,name,path,kind,trusted,index_state,created_at)
             VALUES (?1,'Headless Eval',?2,'generic',1,'pending',?3)",
            rusqlite::params![project_id, root.to_string_lossy(), now],
        )
        .map_err(|e| format!("创建 headless 项目记录失败：{e}"))?;
        conn.execute(
            "INSERT INTO conversations (id,project_id,title,created_at,updated_at)
             VALUES (?1,?2,'Headless Eval',?3,?3)",
            rusqlite::params![conversation_id, project_id, now],
        )
        .map_err(|e| format!("创建 headless 会话记录失败：{e}"))?;
        Ok(Self {
            db: DbState(Arc::new(Mutex::new(conn))),
            project_root: root,
            project_id,
            conversation_id,
            trace_id,
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
        self.execute_bounded(name, args, Duration::from_secs(180))
            .await
    }

    pub async fn execute_bounded(
        &self,
        name: &str,
        args: &str,
        remaining_wall_time: Duration,
    ) -> Result<String, String> {
        let call_id = format!("headless-call-{}", uuid::Uuid::new_v4());
        self.execute_observed(name, args, &call_id, remaining_wall_time)
            .await
    }

    pub async fn execute_observed(
        &self,
        name: &str,
        args: &str,
        call_id: &str,
        remaining_wall_time: Duration,
    ) -> Result<String, String> {
        if self.is_cancelled() {
            return Err("headless 工具执行已取消".into());
        }
        let contract = self.policy.check(name, args)?;
        let created_at = chrono::Utc::now().timestamp();
        let input = audit_preview(args);
        let idempotency_key =
            crate::agent::tool_runtime::idempotency_key(&self.trace_id, call_id, name, args);
        {
            let conn = self.db.0.lock().map_err(|error| error.to_string())?;
            conn.execute(
                "INSERT INTO tool_runs
                 (id,conversation_id,tool_name,input_json,status,duration_ms,created_at,trace_id,
                  call_id,idempotency_key,effect_kind,recovery_policy,prepared_at,project_id,producer_version)
                 VALUES (?1,?2,?3,?4,'running',0,?5,?6,?1,?7,?8,?9,?5,?10,?11)",
                rusqlite::params![
                    call_id,
                    self.conversation_id,
                    name,
                    input,
                    created_at,
                    self.trace_id,
                    idempotency_key,
                    contract.effect.as_str(),
                    contract.recovery.as_str(),
                    self.project_id,
                    env!("CARGO_PKG_VERSION"),
                ],
            )
            .map_err(|error| format!("记录 headless 工具开始失败：{error}"))?;
        }
        let started = std::time::Instant::now();
        let retried = run_tool_with_retry(&contract, &TOOL_POLICY, || async move {
            if self.is_cancelled() {
                return Err("headless 工具执行已取消".to_string());
            }
            let remaining = remaining_wall_time.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err("headless 工具执行没有剩余 wall time".to_string());
            }
            let timeout = remaining.min(Duration::from_millis(contract.timeout_ms));
            tokio::time::timeout(
                timeout,
                run_tool_boxed(
                    name,
                    args,
                    &self.project_root.to_string_lossy(),
                    &[],
                    "headless",
                    &self.db,
                    &self.mcp,
                    &ToolCtx::empty(),
                ),
            )
            .await
            .unwrap_or_else(|_| {
                Err(format!(
                    "headless 工具 {name} 超过 {} ms 超时",
                    timeout.as_millis()
                ))
            })
        })
        .await;
        let attempts = retried.attempts;
        let retry_count = attempts.saturating_sub(1) as i64;
        let result = retried.value.map(|output| retry_notice(output, attempts));
        let duration_ms = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
        let status = if result.is_ok() { "ok" } else { "error" };
        let raw_output = match &result {
            Ok(output) | Err(output) => output,
        };
        let output = audit_preview(raw_output);
        let structured =
            crate::agent::structured_result::ToolResultEnvelope::from_execution_with_metrics(
                name,
                &input,
                &output,
                status,
                duration_ms,
            );
        let structured_json =
            serde_json::to_string(&structured).map_err(|error| error.to_string())?;
        let evidence_digest = structured.digest();
        {
            let conn = self.db.0.lock().map_err(|error| error.to_string())?;
            conn.execute(
                "UPDATE tool_runs SET result_json=?1,status=?2,duration_ms=?3,finished_at=?4,
                 structured_result_json=?5,evidence_digest=?6,protocol_version=2,error_code=?7,
                 compensation_json=?8,metrics_json=?9,outcome_committed_at=?10,retry_count=?11
                 WHERE id=?12 AND status='running'",
                rusqlite::params![
                    output,
                    status,
                    duration_ms,
                    chrono::Utc::now().timestamp(),
                    structured_json,
                    evidence_digest,
                    structured.error.as_ref().map(|error| error.code.as_str()),
                    serde_json::to_string(&structured.compensation).ok(),
                    serde_json::to_string(&structured.metrics).ok(),
                    chrono::Utc::now().timestamp_millis(),
                    retry_count,
                    call_id,
                ],
            )
            .map_err(|error| format!("记录 headless 工具终态失败：{error}"))?;
        }
        result
    }

    pub fn quality_summary(
        &self,
    ) -> Result<crate::agent::tool_metrics::ToolQualitySummary, String> {
        let conn = self.db.0.lock().map_err(|error| error.to_string())?;
        crate::agent::tool_metrics::summary(&conn, 0)
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
        assert!(result.unwrap_err().contains("未授权"));
    }

    #[test]
    fn policy_uses_shared_contracts_and_strict_arguments() {
        let policy = HeadlessToolPolicy;
        let read = policy
            .check("read_file", r#"{"path":"README.md"}"#)
            .unwrap();
        assert_eq!(
            read.effect,
            crate::agent::tools::contracts::EffectKind::Read
        );
        let write = policy
            .check("write_file", r#"{"path":"a.txt","content":"x"}"#)
            .unwrap();
        assert_eq!(
            write.effect,
            crate::agent::tools::contracts::EffectKind::Write
        );
        assert!(policy
            .check("write_file", "not-json")
            .unwrap_err()
            .contains("合法 JSON"));
        assert!(policy
            .check("write_file", "[]")
            .unwrap_err()
            .contains("JSON object"));
        assert!(policy
            .check("run_command", r#"{"command":"true"}"#)
            .is_err());
    }

    #[tokio::test]
    async fn runtime_records_failed_tool_quality() {
        let workspace =
            std::env::temp_dir().join(format!("harmony-headless-metrics-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        let runtime = HeadlessToolRuntime::new(&workspace).unwrap();

        let error = runtime
            .execute_observed(
                "read_file",
                r#"{"path":"missing.txt"}"#,
                "missing-call",
                Duration::from_secs(1),
            )
            .await
            .unwrap_err();
        assert!(!error.is_empty());
        let quality = runtime.quality_summary().unwrap();
        assert_eq!(quality.total_calls, 1);
        assert_eq!(quality.successful_calls, 0);
        assert_eq!(quality.success_rate, 0.0);
        std::fs::remove_dir_all(workspace).ok();
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
        let scope: (String, String) = conn
            .query_row(
                "SELECT p.id,c.id FROM projects p JOIN conversations c ON c.project_id=p.id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(scope.0, runtime.project_id);
        assert_eq!(scope.1, runtime.conversation_id);
    }

    fn zero_delay_policy() -> crate::utils::retry::RetryPolicy {
        crate::utils::retry::RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 0,
            max_delay_ms: 0,
        }
    }

    #[tokio::test]
    async fn tool_retry_retries_only_retryable_errors_on_retry_safe_tools() {
        let contract = crate::agent::tools::contracts::contract("read_file");
        assert!(contract.retry_safe);
        let mut calls = 0usize;
        let retried = run_tool_with_retry(&contract, &zero_delay_policy(), || {
            calls += 1;
            async move {
                if calls < 3 {
                    Err("连接被拒绝，请稍后重试".to_string())
                } else {
                    Ok("ok".to_string())
                }
            }
        })
        .await;
        assert_eq!(retried.attempts, 3);
        assert_eq!(retried.value.unwrap(), "ok");
        assert_eq!(calls, 3);
    }

    #[tokio::test]
    async fn tool_retry_stops_after_first_non_retryable_error() {
        let contract = crate::agent::tools::contracts::contract("read_file");
        let mut calls = 0usize;
        let retried = run_tool_with_retry(&contract, &zero_delay_policy(), || {
            calls += 1;
            async move { Err::<String, String>("文件不存在：missing.txt".to_string()) }
        })
        .await;
        assert_eq!(retried.attempts, 1);
        assert!(retried.value.unwrap_err().contains("不存在"));
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn tool_retry_skips_non_retry_safe_contracts() {
        let contract = crate::agent::tools::contracts::contract("write_file");
        assert!(!contract.retry_safe);
        let mut calls = 0usize;
        let retried = run_tool_with_retry(&contract, &zero_delay_policy(), || {
            calls += 1;
            async move { Err::<String, String>("连接超时".to_string()) }
        })
        .await;
        assert_eq!(retried.attempts, 1);
        assert_eq!(calls, 1);
    }

    #[test]
    fn retry_notice_only_wraps_after_retries() {
        assert_eq!(retry_notice("out".into(), 1), "out");
        let noticed = retry_notice("out".into(), 3);
        assert!(noticed.starts_with("（首次执行超时/网络错误，已自动重试 2 次）"));
        assert!(noticed.ends_with("\nout"));
    }

    #[tokio::test]
    async fn failed_tool_records_zero_retry_count_in_scoped_db() {
        let workspace =
            std::env::temp_dir().join(format!("harmony-headless-retry-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        let runtime = HeadlessToolRuntime::new(&workspace).unwrap();
        let error = runtime
            .execute_observed(
                "read_file",
                r#"{"path":"missing.txt"}"#,
                "no-retry-call",
                Duration::from_secs(2),
            )
            .await
            .unwrap_err();
        assert!(!error.is_empty());
        let conn = runtime.db.0.lock().unwrap();
        let retry_count: i64 = conn
            .query_row(
                "SELECT retry_count FROM tool_runs WHERE id='no-retry-call'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retry_count, 0);
        drop(conn);
        std::fs::remove_dir_all(workspace).ok();
    }
}
