//! Broker 审批凭据：绑定已准备的工具调用，不接受会话白名单作为显式审批。
//!
//! v4 起凭据与能力无关：任何宿主能力工具都能签发与复核，OTA 只是 scope 取文件内容摘要的
//! 一种特例（`ApprovalScope::Ota`），其他能力绑定请求幂等键与 canonical 工作区
//! （`ApprovalScope::Request`）。因此新增能力不需要再复制一套凭据逻辑。
//!
//! 可撤销判定同样不看工具名，而是「该调用仍处于活动状态，且签发过持久凭据」——自描述、
//! 不需要维护工具白名单，OTA 自动被覆盖。旧 v3 凭据一律失败关闭，要求重新审批。
use rusqlite::{Connection, OptionalExtension};

const EVENT: &str = "host_capability.explicit_approval";
/// 仍可撤销的活动状态（与 tool_runs 生命周期一致）。
const ACTIVE_STATUSES: &str =
    "'prepared','running','verifying','recovery_required','stuck'";
/// 审批凭据有效期；同时是签发/复核两侧共同校验的固定时长。
const TTL_MS: i64 = 30 * 60 * 1000;

/// 凭据绑定的作用域：签发前冻结，执行前复核，防止审批后替换参数或输入内容。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ApprovalScope {
    /// OTA 打包：输入文件内容摘要 + 工作区。
    Ota(super::ota_scope::OtaScope),
    /// 其他宿主能力：请求幂等键（含参数） + canonical 工作区。
    Request {
        request_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace: Option<String>,
    },
}

/// 仅撤销指定调用，不发送会话级停止信号或影响其他工具。
pub(crate) fn revoke_call(conn: &Connection, call: &str) -> Result<(), String> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let eligible: bool = tx
        .query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM tool_runs t WHERE t.id=?1 AND t.status IN ({ACTIVE_STATUSES})
                 AND EXISTS(SELECT 1 FROM run_events e WHERE e.run_id=t.trace_id
                     AND e.conversation_id=t.conversation_id AND e.event_type=?2
                     AND json_extract(e.payload,'$.tool_call_id')=t.id))"
            ),
            rusqlite::params![call, EVENT],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !eligible {
        return Err(
            "目标不是可撤销的宿主能力调用：它不在活动状态，或没有可撤销的持久审批凭据".into(),
        );
    }
    tx.execute(
        "INSERT OR IGNORE INTO ota_approval_revocations(call_id,run_id,conversation_id,revoked_at,reason,tool)
         SELECT id,trace_id,conversation_id,?2,'user_revoke_call',tool_name FROM tool_runs WHERE id=?1",
        rusqlite::params![call, chrono::Utc::now().timestamp_millis()],
    )
    .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())
}

#[derive(Clone, Copy)]
pub(crate) enum StopReason {
    User,
    Timeout,
    Watchdog,
    ConversationDeleted,
}

impl StopReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user_stop",
            Self::Timeout => "tool_timeout",
            Self::Watchdog => "watchdog",
            Self::ConversationDeleted => "conversation_deleted",
        }
    }
}

/// 先发本地停止，再持久撤销；数据库失败不能阻止本地停止信号。
pub(crate) fn stop_and_revoke(
    db: &crate::db::DbState,
    conversation: &str,
    reason: StopReason,
) -> Result<(), String> {
    crate::agent::exec_ctx::request_stop_tool(conversation);
    let conn =
        db.0.try_lock()
            .map_err(|_| "停止已请求，但审批数据库忙或锁损坏，持久撤销未完成")?;
    revoke_with_reason(&conn, conversation, reason)?;
    Ok(())
}

#[cfg(test)]
fn revoke_conversation(conn: &Connection, conversation: &str) -> Result<usize, String> {
    revoke_with_reason(conn, conversation, StopReason::User)
}

/// 停止会话时撤销该会话内所有活动且签发过凭据的能力调用。
fn revoke_with_reason(
    conn: &Connection,
    conversation: &str,
    reason: StopReason,
) -> Result<usize, String> {
    conn.execute(
        &format!(
            "INSERT OR IGNORE INTO ota_approval_revocations(call_id,run_id,conversation_id,revoked_at,reason,tool)
             SELECT t.id,t.trace_id,t.conversation_id,?2,?3,t.tool_name FROM tool_runs t
             WHERE t.conversation_id=?1 AND t.status IN ({ACTIVE_STATUSES})
             AND EXISTS(SELECT 1 FROM run_events e WHERE e.run_id=t.trace_id
                 AND e.conversation_id=t.conversation_id AND e.event_type=?4
                 AND json_extract(e.payload,'$.tool_call_id')=t.id)"
        ),
        rusqlite::params![
            conversation,
            chrono::Utc::now().timestamp_millis(),
            reason.as_str(),
            EVENT
        ],
    )
    .map_err(|error| format!("审批撤销持久化失败：{error}"))
}

pub(crate) fn is_revoked(conn: &Connection, call: &str) -> Result<bool, String> {
    let revoked: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM ota_approval_revocations WHERE call_id=?1)",
            [call],
            |row| row.get(0),
        )
        .map_err(|error| format!("无法读取审批撤销状态：{error}"))?;
    Ok(revoked)
}

fn require_not_revoked(conn: &Connection, call: &str) -> Result<(), String> {
    if is_revoked(conn, call)? {
        return Err("本次调用已被持久撤销，必须发起新的工具调用并重新审批".into());
    }
    Ok(())
}

fn approval_epoch() -> &'static str {
    static EPOCH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    EPOCH.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

fn validate_lifecycle(
    payload: &serde_json::Value,
    conversation: &str,
    now: i64,
) -> Result<(), String> {
    let issued = payload["issued_at_ms"].as_i64().ok_or("审批缺少签发时间")?;
    let expires = payload["expires_at_ms"]
        .as_i64()
        .ok_or("审批缺少过期时间")?;
    if payload["process_epoch"].as_str() != Some(approval_epoch())
        || payload["stop_generation"].as_u64()
            != Some(crate::agent::exec_ctx::stop_generation(conversation))
        || issued > now
        || expires <= now
        || expires.checked_sub(issued) != Some(TTL_MS)
    {
        return Err("审批已因停止、应用重启或超时失效，请重新审批".into());
    }
    Ok(())
}

/// 按工具名匹配当前状态下的工具调用，返回其幂等键；同时校验撤销状态。
fn tool_key(
    conn: &Connection,
    run: &str,
    conversation: &str,
    call: &str,
    tool: &str,
    status: &str,
) -> Result<String, String> {
    if run.is_empty() || conversation.is_empty() || call.is_empty() {
        return Err("Broker 审批缺少 Run/会话/工具调用身份".into());
    }
    require_not_revoked(conn, call)?;
    conn.query_row(
        "SELECT idempotency_key FROM tool_runs WHERE id=?1 AND trace_id=?2
         AND conversation_id=?3 AND tool_name=?4 AND status=?5",
        rusqlite::params![call, run, conversation, tool, status],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|error| error.to_string())?
    .filter(|key| !key.is_empty())
    .ok_or_else(|| "Broker 审批未匹配到当前状态下的工具调用".into())
}

/// 为任意宿主能力工具签发审批凭据。调用方在展示审批之前冻结 `scope`。
pub(crate) fn record_capability_approval(
    ctx: &crate::agent::exec_ctx::ToolCtx,
    tool: &str,
    args_raw: &str,
    scope: &ApprovalScope,
    stop_generation: u64,
) -> Result<(), String> {
    if stop_generation != crate::agent::exec_ctx::stop_generation(&ctx.conversation_id) {
        return Err("审批等待期间已收到停止请求，拒绝签发审批凭据".into());
    }
    let call = ctx
        .tool_call_id
        .as_deref()
        .ok_or("审批缺少工具调用 ID")?;
    let app = ctx.app.as_ref().ok_or("审批缺少持久数据库")?;
    let db: tauri::State<crate::db::DbState> = tauri::Manager::state(app);
    let conn = db.0.lock().map_err(|_| "审批数据库锁损坏")?;
    let key = tool_key(&conn, &ctx.run_id, &ctx.conversation_id, call, tool, "prepared")?;
    let expected = crate::agent::tool_runtime::idempotency_key(&ctx.run_id, call, tool, args_raw);
    if key != expected {
        return Err(format!("{tool} 审批参数与已登记工具调用不一致"));
    }
    let issued_at = chrono::Utc::now().timestamp_millis();
    crate::agent::runtime::append_event(
        &conn,
        &ctx.run_id,
        &ctx.conversation_id,
        EVENT,
        serde_json::json!({
            "version": 4, "tool_call_id": call, "tool": tool, "scope": scope,
            "process_epoch": approval_epoch(), "stop_generation": stop_generation,
            "issued_at_ms": issued_at, "expires_at_ms": issued_at + TTL_MS,
            "tool_request_key": key, "decision": "explicitly_approved",
        }),
    )?;
    Ok(())
}

/// 复核任意宿主能力工具的审批凭据，返回冻结的作用域。
pub(crate) fn verify_capability_approval(
    conn: &Connection,
    run: &str,
    conversation: &str,
    call: &str,
    tool: &str,
) -> Result<ApprovalScope, String> {
    let key = tool_key(conn, run, conversation, call, tool, "running")?;
    let payload: Option<String> = conn
        .query_row(
            "SELECT payload FROM run_events WHERE run_id=?1 AND conversation_id=?2
         AND event_type=?3 AND json_extract(payload,'$.tool_call_id')=?4
         ORDER BY seq DESC LIMIT 1",
            rusqlite::params![run, conversation, EVENT, call],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("Broker 审批读取失败：{error}"))?;
    let payload: serde_json::Value =
        serde_json::from_str(&payload.ok_or("Broker 缺少本次调用的显式审批凭据")?)
            .map_err(|error| format!("Broker 审批凭据损坏：{error}"))?;
    // 旧版本凭据（v3 及更早）没有通用 scope，一律失败关闭要求重新审批
    if payload["version"] != 4
        || payload["tool"].as_str() != Some(tool)
        || payload["decision"] != "explicitly_approved"
        || payload["tool_request_key"].as_str() != Some(key.as_str())
    {
        return Err("Broker 显式审批凭据与当前工具请求不匹配".into());
    }
    validate_lifecycle(
        &payload,
        conversation,
        chrono::Utc::now().timestamp_millis(),
    )?;
    serde_json::from_value(payload["scope"].clone())
        .map_err(|_| "Broker 审批缺少有效请求作用域，需重新审批".into())
}

/// OTA 专用复核：在通用凭据之上要求作用域确为文件内容摘要。
pub(crate) fn verify_ota_approval(
    conn: &Connection,
    run: &str,
    conversation: &str,
    call: &str,
) -> Result<super::ota_scope::OtaScope, String> {
    match verify_capability_approval(conn, run, conversation, call, "ota_pack")? {
        ApprovalScope::Ota(scope) => Ok(scope),
        ApprovalScope::Request { .. } => {
            Err("Broker OTA 审批缺少文件/能力作用域，需重新审批".into())
        }
    }
}

/// 在路径解析/创建暂存目录之前，再核对实际交给工具的参数，避免审批后替换参数。
pub(crate) fn verify_ota_arguments(
    ctx: &crate::agent::exec_ctx::ToolCtx,
    args: &serde_json::Value,
    roots: &[String],
) -> Result<(), String> {
    let call = ctx.tool_call_id.as_deref().ok_or("OTA 缺少工具调用 ID")?;
    let app = ctx.app.as_ref().ok_or("OTA 缺少持久审批数据库")?;
    let db: tauri::State<crate::db::DbState> = tauri::Manager::state(app);
    let conn = db.0.lock().map_err(|_| "OTA 审批数据库锁损坏")?;
    let approved = verify_ota_approval(&conn, &ctx.run_id, &ctx.conversation_id, call)?;
    let key = tool_key(
        &conn,
        &ctx.run_id,
        &ctx.conversation_id,
        call,
        "ota_pack",
        "running",
    )?;
    let actual = crate::agent::tool_runtime::idempotency_key(
        &ctx.run_id,
        call,
        "ota_pack",
        &args.to_string(),
    );
    if key != actual {
        return Err("OTA 执行参数已偏离本次显式审批，拒绝执行".into());
    }
    drop(conn);
    if approved != super::ota_scope::argument_scope(roots, args)? {
        return Err("OTA 文件内容或路径已偏离审批快照，需重新审批".into());
    }
    Ok(())
}

/// 产物发布前再次验证停止/重启/过期边界；不重新读取可能已被编辑的原输入。
pub(crate) fn verify_ota_publication(ctx: &crate::agent::exec_ctx::ToolCtx) -> Result<(), String> {
    let call = ctx.tool_call_id.as_deref().ok_or("OTA 缺少工具调用 ID")?;
    let app = ctx.app.as_ref().ok_or("OTA 缺少审批数据库")?;
    let db: tauri::State<crate::db::DbState> = tauri::Manager::state(app);
    let conn = db.0.lock().map_err(|_| "OTA 审批数据库锁损坏")?;
    verify_ota_approval(&conn, &ctx.run_id, &ctx.conversation_id, call)?;
    Ok(())
}

pub(crate) fn verify_ota_capability(
    ctx: &crate::agent::exec_ctx::ToolCtx,
    capability: &super::capability_broker::HostCapability,
    workspace: &std::path::Path,
) -> Result<super::ota_scope::OtaScope, String> {
    let call = ctx.tool_call_id.as_deref().ok_or("OTA 缺少工具调用 ID")?;
    let app = ctx.app.as_ref().ok_or("OTA 缺少持久审批数据库")?;
    let db: tauri::State<crate::db::DbState> = tauri::Manager::state(app);
    let approved = {
        let conn = db.0.lock().map_err(|_| "OTA 审批数据库锁损坏")?;
        verify_ota_approval(&conn, &ctx.run_id, &ctx.conversation_id, call)?
    };
    if approved != super::ota_scope::capability_scope(capability, workspace)? {
        return Err("Broker 拒绝执行：OTA 能力参数或输入内容与审批快照不一致".into());
    }
    Ok(approved)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_SCHEMA: &str = "CREATE TABLE tool_runs(id TEXT,trace_id TEXT,conversation_id TEXT,
            tool_name TEXT,status TEXT,idempotency_key TEXT);
            CREATE TABLE run_events(run_id TEXT,conversation_id TEXT,event_type TEXT,seq INTEGER,payload TEXT);
            CREATE TABLE ota_approval_revocations(call_id TEXT PRIMARY KEY,run_id TEXT,conversation_id TEXT,
            revoked_at INTEGER,reason TEXT,tool TEXT NOT NULL DEFAULT 'ota_pack');";

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(FIXTURE_SCHEMA).unwrap();
        conn.execute_batch(
            "INSERT INTO tool_runs VALUES('call','run','conversation','ota_pack','running','key');",
        )
        .unwrap();
        conn
    }

    fn ota_scope() -> serde_json::Value {
        serde_json::json!({"kind":"ota","paths_sha256":"paths","hap_sha256":"hap","profile_sha256":null})
    }

    fn receipt(conn: &Connection, call: &str, key: &str, decision: &str, seq: i64) {
        receipt_for(conn, call, "ota_pack", key, decision, seq, ota_scope())
    }

    fn receipt_for(
        conn: &Connection,
        call: &str,
        tool: &str,
        key: &str,
        decision: &str,
        seq: i64,
        scope: serde_json::Value,
    ) {
        let now = chrono::Utc::now().timestamp_millis();
        let payload = serde_json::json!({"version":4,"tool_call_id":call,"tool":tool,
            "process_epoch":approval_epoch(),"stop_generation":crate::agent::exec_ctx::stop_generation("conversation"),
            "issued_at_ms":now,"expires_at_ms":now+TTL_MS,
            "scope":scope,
            "tool_request_key":key,"decision":decision});
        conn.execute(
            "INSERT INTO run_events VALUES('run','conversation',?1,?2,?3)",
            rusqlite::params![EVENT, seq, payload.to_string()],
        )
        .unwrap();
    }

    #[test]
    fn missing_or_other_call_approval_is_rejected() {
        let conn = fixture();
        assert!(verify_ota_approval(&conn, "run", "conversation", "call").is_err());
        receipt(&conn, "other", "key", "explicitly_approved", 1);
        assert!(verify_ota_approval(&conn, "run", "conversation", "call").is_err());
        receipt(&conn, "call", "key", "explicitly_approved", 2);
        assert!(verify_ota_approval(&conn, "run", "conversation", "call").is_ok());
        assert!(verify_ota_approval(&conn, "other", "conversation", "call").is_err());
        assert!(verify_ota_approval(&conn, "run", "other", "call").is_err());
    }

    #[test]
    fn changed_request_and_latest_rejection_do_not_fall_back() {
        let conn = fixture();
        receipt(&conn, "call", "key", "explicitly_approved", 1);
        receipt(&conn, "call", "changed", "explicitly_approved", 2);
        assert!(verify_ota_approval(&conn, "run", "conversation", "call").is_err());
        receipt(&conn, "call", "key", "rejected", 3);
        assert!(verify_ota_approval(&conn, "run", "conversation", "call").is_err());
    }

    #[test]
    fn inactive_or_wrong_tool_cannot_reuse_approval() {
        let conn = fixture();
        receipt(&conn, "call", "key", "explicitly_approved", 1);
        for status in ["prepared", "succeeded", "failed", "stuck"] {
            conn.execute("UPDATE tool_runs SET status=?1", [status])
                .unwrap();
            assert!(verify_ota_approval(&conn, "run", "conversation", "call").is_err());
        }
        conn.execute_batch("UPDATE tool_runs SET status='running',tool_name='run_command'")
            .unwrap();
        assert!(verify_ota_approval(&conn, "run", "conversation", "call").is_err());
    }

    #[test]
    fn malformed_latest_receipt_fails_closed() {
        let conn = fixture();
        receipt(&conn, "call", "key", "explicitly_approved", 1);
        conn.execute(
            "INSERT INTO run_events VALUES('run','conversation',?1,2,'not json')",
            [EVENT],
        )
        .unwrap();
        assert!(verify_ota_approval(&conn, "run", "conversation", "call").is_err());
    }

    #[test]
    fn old_or_missing_scope_receipts_require_reapproval() {
        let conn = fixture();
        receipt(&conn, "call", "key", "explicitly_approved", 1);
        conn.execute_batch("UPDATE run_events SET payload=json_set(payload,'$.version',3)")
            .unwrap();
        assert!(verify_ota_approval(&conn, "run", "conversation", "call").is_err());
        conn.execute_batch(
            "UPDATE run_events SET payload=json_remove(json_set(payload,'$.version',4),'$.scope')",
        )
        .unwrap();
        assert!(verify_ota_approval(&conn, "run", "conversation", "call").is_err());
    }

    #[test]
    fn lifecycle_rejects_restart_expiry_future_and_malformed_duration() {
        let conversation = uuid::Uuid::new_v4().to_string();
        let now = 2_000_000;
        let valid = serde_json::json!({"process_epoch":approval_epoch(),"stop_generation":0,
            "issued_at_ms":now-1,"expires_at_ms":now-1+TTL_MS});
        assert!(validate_lifecycle(&valid, &conversation, now).is_ok());
        for (key, value) in [
            ("process_epoch", serde_json::json!("previous-process")),
            ("expires_at_ms", serde_json::json!(now)),
            ("issued_at_ms", serde_json::json!(now + 1)),
            ("expires_at_ms", serde_json::json!(i64::MAX)),
            ("stop_generation", serde_json::Value::Null),
        ] {
            let mut changed = valid.clone();
            changed[key] = value;
            assert!(
                validate_lifecycle(&changed, &conversation, now).is_err(),
                "{key}"
            );
        }
    }

    /// 通用能力凭据：非 OTA 工具也能签发/复核，且工具名不匹配即拒绝。
    #[test]
    fn generic_capability_receipt_is_tool_scoped() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(FIXTURE_SCHEMA).unwrap();
        conn.execute_batch(
            "INSERT INTO tool_runs VALUES('install','run','conversation','device_deploy','running','ik');
             INSERT INTO tool_runs VALUES('other','run','conversation','device_debug','running','ik2');",
        )
        .unwrap();
        receipt_for(
            &conn,
            "install",
            "device_deploy",
            "ik",
            "explicitly_approved",
            1,
            serde_json::json!({"kind":"request","request_key":"ik","workspace":"/ws"}),
        );
        assert_eq!(
            verify_capability_approval(&conn, "run", "conversation", "install", "device_deploy")
                .unwrap(),
            ApprovalScope::Request {
                request_key: "ik".into(),
                workspace: Some("/ws".into())
            }
        );
        // 同一凭据不能被另一个工具复用；OTA 复核也不能消费请求型作用域
        assert!(
            verify_capability_approval(&conn, "run", "conversation", "install", "device_debug")
                .is_err()
        );
        assert!(verify_ota_approval(&conn, "run", "conversation", "install").is_err());
    }

    /// 撤销资格自描述：活动状态但没有凭据的调用不可撤销。
    #[test]
    fn revocation_requires_an_active_call_with_receipt() {
        let conn = fixture();
        conn.execute_batch("INSERT INTO tool_runs VALUES('plain','run','conversation','device_deploy','running','ik');")
            .unwrap();
        assert!(revoke_call(&conn, "plain").is_err());
        receipt_for(
            &conn,
            "plain",
            "device_deploy",
            "ik",
            "explicitly_approved",
            1,
            serde_json::json!({"kind":"request","request_key":"ik"}),
        );
        revoke_call(&conn, "plain").unwrap();
        let tool: String = conn
            .query_row(
                "SELECT tool FROM ota_approval_revocations WHERE call_id='plain'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tool, "device_deploy");
    }

    #[test]
    fn durable_revocation_blocks_signing_execution_and_same_call_recovery() {
        let conn = fixture();
        receipt(&conn, "call", "key", "explicitly_approved", 1);
        assert_eq!(revoke_conversation(&conn, "other").unwrap(), 0);
        assert_eq!(revoke_conversation(&conn, "conversation").unwrap(), 1);
        assert_eq!(revoke_conversation(&conn, "conversation").unwrap(), 0);
        assert!(verify_ota_approval(&conn, "run", "conversation", "call").is_err());
        conn.execute_batch("UPDATE tool_runs SET status='prepared'")
            .unwrap();
        assert!(tool_key(&conn, "run", "conversation", "call", "ota_pack", "prepared").is_err());
        conn.execute_batch("UPDATE tool_runs SET id='new-call'")
            .unwrap();
        assert!(tool_key(&conn, "run", "conversation", "new-call", "ota_pack", "prepared").is_ok());
    }

    #[test]
    fn stop_signal_survives_database_failure() {
        let conversation = uuid::Uuid::new_v4().to_string();
        let db = crate::db::DbState(std::sync::Arc::new(std::sync::Mutex::new(
            Connection::open_in_memory().unwrap(),
        )));
        let before = crate::agent::exec_ctx::stop_generation(&conversation);
        assert!(stop_and_revoke(&db, &conversation, StopReason::Timeout).is_err());
        assert_ne!(
            crate::agent::exec_ctx::stop_generation(&conversation),
            before
        );
    }

    #[test]
    fn occupied_database_does_not_block_local_stop() {
        let conversation = uuid::Uuid::new_v4().to_string();
        let db = crate::db::DbState(std::sync::Arc::new(std::sync::Mutex::new(fixture())));
        let _guard = db.0.lock().unwrap();
        let before = crate::agent::exec_ctx::stop_generation(&conversation);
        assert!(stop_and_revoke(&db, &conversation, StopReason::Watchdog).is_err());
        assert_ne!(
            crate::agent::exec_ctx::stop_generation(&conversation),
            before
        );
    }

    #[test]
    fn targeted_revocation_is_exact_and_rejects_finished_or_unrelated_tools() {
        let conn = fixture();
        receipt(&conn, "call", "key", "explicitly_approved", 1);
        conn.execute_batch("INSERT INTO tool_runs VALUES('other','run','conversation','ota_pack','running','key');
            INSERT INTO tool_runs VALUES('read','run','conversation','read_file','running','key');
            INSERT INTO tool_runs VALUES('done','run','conversation','ota_pack','succeeded','key');").unwrap();
        revoke_call(&conn, "call").unwrap();
        revoke_call(&conn, "call").unwrap();
        assert!(require_not_revoked(&conn, "call").is_err());
        assert!(require_not_revoked(&conn, "other").is_ok());
        for call in ["missing", "read", "done"] {
            assert!(revoke_call(&conn, call).is_err());
        }
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ota_approval_revocations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn internal_stop_reasons_are_persisted_and_first_cause_is_preserved() {
        for reason in [
            StopReason::User,
            StopReason::Timeout,
            StopReason::Watchdog,
            StopReason::ConversationDeleted,
        ] {
            let conn = fixture();
            receipt(&conn, "call", "key", "explicitly_approved", 1);
            revoke_with_reason(&conn, "conversation", reason).unwrap();
            revoke_with_reason(&conn, "conversation", StopReason::User).unwrap();
            let recorded: String = conn
                .query_row(
                    "SELECT reason FROM ota_approval_revocations WHERE call_id='call'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(recorded, reason.as_str());
        }
    }

    #[test]
    fn revocation_is_visible_to_separate_database_connections() {
        let path = std::env::temp_dir().join(format!(
            "ota-revocation-test-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        {
            let writer = Connection::open(&path).unwrap();
            writer.execute_batch("CREATE TABLE conversations(id TEXT PRIMARY KEY);
                CREATE TABLE tool_runs(id TEXT,trace_id TEXT,conversation_id TEXT,tool_name TEXT,status TEXT);
                INSERT INTO conversations VALUES('conversation');
                INSERT INTO tool_runs VALUES('call','run','conversation','ota_pack','prepared');").unwrap();
            writer
                .execute_batch(include_str!(
                    "../../migrations/083_ota_approval_revocations.sql"
                ))
                .unwrap();
            // 生产库会先跑完全部迁移再到撤销表；这里补齐 084 的 tool 列
            writer
                .execute_batch(include_str!(
                    "../../migrations/084_host_approval_revocation_tool.sql"
                ))
                .unwrap();
            let reader = Connection::open(&path).unwrap();
            assert!(require_not_revoked(&reader, "call").is_ok());
            // 直接写入撤销记录（该场景验证跨连接可见性，不经过凭据资格判定）
            writer
                .execute_batch(
                    "INSERT INTO ota_approval_revocations(call_id,run_id,conversation_id,revoked_at,reason,tool)
                     VALUES('call','run','conversation',1,'user_revoke_call','ota_pack');",
                )
                .unwrap();
            assert!(require_not_revoked(&reader, "call").is_err());
            drop(writer);
            assert!(require_not_revoked(&reader, "call").is_err());
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn stop_revokes_old_receipt_but_new_approval_can_proceed() {
        let conversation = uuid::Uuid::new_v4().to_string();
        let mut receipt = serde_json::json!({"process_epoch":approval_epoch(),"stop_generation":0,
            "issued_at_ms":100,"expires_at_ms":100+TTL_MS});
        crate::agent::exec_ctx::request_stop_tool(&conversation);
        assert!(validate_lifecycle(&receipt, &conversation, 101).is_err());
        receipt["stop_generation"] =
            serde_json::json!(crate::agent::exec_ctx::stop_generation(&conversation));
        assert!(validate_lifecycle(&receipt, &conversation, 101).is_ok());
    }
}
