//! Broker 审批凭据 P0：绑定已准备的工具调用，不接受会话白名单作为显式审批。
use rusqlite::{Connection, OptionalExtension};

const EVENT: &str = "host_capability.explicit_approval";

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

fn revoke_with_reason(
    conn: &Connection,
    conversation: &str,
    reason: StopReason,
) -> Result<usize, String> {
    conn.execute(
        "INSERT OR IGNORE INTO ota_approval_revocations(call_id,run_id,conversation_id,revoked_at,reason)
         SELECT id,trace_id,conversation_id,?2,?3 FROM tool_runs
         WHERE conversation_id=?1 AND tool_name='ota_pack'
         AND status IN ('prepared','running','verifying','recovery_required','stuck')",
        rusqlite::params![conversation, chrono::Utc::now().timestamp_millis(), reason.as_str()],
    ).map_err(|error| format!("OTA 审批撤销持久化失败：{error}"))
}

fn require_not_revoked(conn: &Connection, call: &str) -> Result<(), String> {
    let revoked: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM ota_approval_revocations WHERE call_id=?1)",
            [call],
            |row| row.get(0),
        )
        .map_err(|error| format!("无法读取 OTA 审批撤销状态：{error}"))?;
    if revoked {
        return Err("本次 OTA 调用已被持久撤销，必须发起新的工具调用并重新审批".into());
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
        || expires.checked_sub(issued) != Some(30 * 60 * 1000)
    {
        return Err("OTA 审批已因停止、应用重启或超时失效，请重新审批".into());
    }
    Ok(())
}

fn tool_key(
    conn: &Connection,
    run: &str,
    conversation: &str,
    call: &str,
    status: &str,
) -> Result<String, String> {
    if run.is_empty() || conversation.is_empty() || call.is_empty() {
        return Err("Broker 审批缺少 Run/会话/工具调用身份".into());
    }
    require_not_revoked(conn, call)?;
    conn.query_row(
        "SELECT idempotency_key FROM tool_runs WHERE id=?1 AND trace_id=?2
         AND conversation_id=?3 AND tool_name='ota_pack' AND status=?4",
        rusqlite::params![call, run, conversation, status],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|error| error.to_string())?
    .filter(|key| !key.is_empty())
    .ok_or_else(|| "Broker 审批未匹配到当前状态下的 OTA 工具调用".into())
}

pub(crate) fn record_ota_approval(
    ctx: &crate::agent::exec_ctx::ToolCtx,
    args_raw: &str,
    scope: &super::ota_scope::OtaScope,
    stop_generation: u64,
) -> Result<(), String> {
    if stop_generation != crate::agent::exec_ctx::stop_generation(&ctx.conversation_id) {
        return Err("审批等待期间已收到停止请求，拒绝签发 OTA 凭据".into());
    }
    let call = ctx
        .tool_call_id
        .as_deref()
        .ok_or("OTA 审批缺少工具调用 ID")?;
    let app = ctx.app.as_ref().ok_or("OTA 审批缺少持久数据库")?;
    let db: tauri::State<crate::db::DbState> = tauri::Manager::state(app);
    let conn = db.0.lock().map_err(|_| "OTA 审批数据库锁损坏")?;
    let key = tool_key(&conn, &ctx.run_id, &ctx.conversation_id, call, "prepared")?;
    let expected =
        crate::agent::tool_runtime::idempotency_key(&ctx.run_id, call, "ota_pack", args_raw);
    if key != expected {
        return Err("OTA 审批参数与已登记工具调用不一致".into());
    }
    let issued_at = chrono::Utc::now().timestamp_millis();
    crate::agent::runtime::append_event(
        &conn,
        &ctx.run_id,
        &ctx.conversation_id,
        EVENT,
        serde_json::json!({
            "version": 3, "tool_call_id": call, "tool": "ota_pack", "scope": scope,
            "process_epoch": approval_epoch(), "stop_generation": stop_generation,
            "issued_at_ms": issued_at, "expires_at_ms": issued_at + 30 * 60 * 1000,
            "tool_request_key": key, "decision": "explicitly_approved",
        }),
    )?;
    Ok(())
}

pub(crate) fn verify_ota_approval(
    conn: &Connection,
    run: &str,
    conversation: &str,
    call: &str,
) -> Result<super::ota_scope::OtaScope, String> {
    let key = tool_key(conn, run, conversation, call, "running")?;
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
        serde_json::from_str(&payload.ok_or("Broker 缺少本次 OTA 调用的显式审批凭据")?)
            .map_err(|error| format!("Broker 审批凭据损坏：{error}"))?;
    if payload["version"] != 3
        || payload["tool"] != "ota_pack"
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
        .map_err(|_| "Broker OTA 审批缺少有效文件/能力作用域，需重新审批".into())
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
    let key = tool_key(&conn, &ctx.run_id, &ctx.conversation_id, call, "running")?;
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

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE tool_runs(id TEXT,trace_id TEXT,conversation_id TEXT,
            tool_name TEXT,status TEXT,idempotency_key TEXT);
            CREATE TABLE run_events(run_id TEXT,conversation_id TEXT,event_type TEXT,seq INTEGER,payload TEXT);
            CREATE TABLE ota_approval_revocations(call_id TEXT PRIMARY KEY,run_id TEXT,conversation_id TEXT,revoked_at INTEGER,reason TEXT);
            INSERT INTO tool_runs VALUES('call','run','conversation','ota_pack','running','key');").unwrap();
        conn
    }

    fn receipt(conn: &Connection, call: &str, key: &str, decision: &str, seq: i64) {
        let now = chrono::Utc::now().timestamp_millis();
        let payload = serde_json::json!({"version":3,"tool_call_id":call,"tool":"ota_pack",
            "process_epoch":approval_epoch(),"stop_generation":crate::agent::exec_ctx::stop_generation("conversation"),
            "issued_at_ms":now,"expires_at_ms":now+30*60*1000,
            "scope":{"paths_sha256":"paths","hap_sha256":"hap","profile_sha256":null},
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
        conn.execute_batch("UPDATE run_events SET payload=json_set(payload,'$.version',1)")
            .unwrap();
        assert!(verify_ota_approval(&conn, "run", "conversation", "call").is_err());
        conn.execute_batch(
            "UPDATE run_events SET payload=json_remove(json_set(payload,'$.version',3),'$.scope')",
        )
        .unwrap();
        assert!(verify_ota_approval(&conn, "run", "conversation", "call").is_err());
    }

    #[test]
    fn lifecycle_rejects_restart_expiry_future_and_malformed_duration() {
        let conversation = uuid::Uuid::new_v4().to_string();
        let now = 2_000_000;
        let valid = serde_json::json!({"process_epoch":approval_epoch(),"stop_generation":0,
            "issued_at_ms":now-1,"expires_at_ms":now-1+30*60*1000});
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
        assert!(tool_key(&conn, "run", "conversation", "call", "prepared").is_err());
        conn.execute_batch("UPDATE tool_runs SET id='new-call'")
            .unwrap();
        assert!(tool_key(&conn, "run", "conversation", "new-call", "prepared").is_ok());
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
    fn internal_stop_reasons_are_persisted_and_first_cause_is_preserved() {
        for reason in [
            StopReason::User,
            StopReason::Timeout,
            StopReason::Watchdog,
            StopReason::ConversationDeleted,
        ] {
            let conn = fixture();
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
            let reader = Connection::open(&path).unwrap();
            assert!(require_not_revoked(&reader, "call").is_ok());
            revoke_conversation(&writer, "conversation").unwrap();
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
            "issued_at_ms":100,"expires_at_ms":100+30*60*1000});
        crate::agent::exec_ctx::request_stop_tool(&conversation);
        assert!(validate_lifecycle(&receipt, &conversation, 101).is_err());
        receipt["stop_generation"] =
            serde_json::json!(crate::agent::exec_ctx::stop_generation(&conversation));
        assert!(validate_lifecycle(&receipt, &conversation, 101).is_ok());
    }
}
