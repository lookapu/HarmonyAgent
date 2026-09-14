//! Broker 审批凭据 P0：绑定已准备的工具调用，不接受会话白名单作为显式审批。
use rusqlite::{Connection, OptionalExtension};

const EVENT: &str = "host_capability.explicit_approval";

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
) -> Result<(), String> {
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
    crate::agent::runtime::append_event(
        &conn,
        &ctx.run_id,
        &ctx.conversation_id,
        EVENT,
        serde_json::json!({
            "version": 2, "tool_call_id": call, "tool": "ota_pack", "scope": scope,
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
    if payload["version"] != 2
        || payload["tool"] != "ota_pack"
        || payload["decision"] != "explicitly_approved"
        || payload["tool_request_key"].as_str() != Some(key.as_str())
    {
        return Err("Broker 显式审批凭据与当前工具请求不匹配".into());
    }
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
            INSERT INTO tool_runs VALUES('call','run','conversation','ota_pack','running','key');").unwrap();
        conn
    }

    fn receipt(conn: &Connection, call: &str, key: &str, decision: &str, seq: i64) {
        let payload = serde_json::json!({"version":2,"tool_call_id":call,"tool":"ota_pack",
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
            "UPDATE run_events SET payload=json_remove(json_set(payload,'$.version',2),'$.scope')",
        )
        .unwrap();
        assert!(verify_ota_approval(&conn, "run", "conversation", "call").is_err());
    }
}
