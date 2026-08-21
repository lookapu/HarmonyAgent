//! Durable lifecycle for user interactions that temporarily suspend an Agent run.
//!
//! The actual reply channel is intentionally process-local. SQLite records what the
//! run was waiting for and why the wait ended, so restart recovery never treats a
//! lost channel as a live approval and can explain the safe next action.

use rusqlite::{Connection, params};

const WAIT_TIMEOUT_MS: i64 = 5 * 60 * 1000;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn redact_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => {
            serde_json::Value::String(crate::utils::redact::redact_text(text))
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(redact_value).collect())
        }
        serde_json::Value::Object(fields) => serde_json::Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), redact_value(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

pub fn begin(
    request_id: &str,
    conversation_id: &str,
    run_id: Option<&str>,
    kind: &str,
    payload: &serde_json::Value,
) -> Result<(), String> {
    let Some(db) = crate::db::global() else {
        // Unit tests and headless tool processes intentionally run without the app DB.
        return Ok(());
    };
    let conn = db.lock().map_err(|e| e.to_string())?;
    begin_with_conn(&conn, request_id, conversation_id, run_id, kind, payload)
}

pub fn begin_with_conn(
    conn: &Connection,
    request_id: &str,
    conversation_id: &str,
    run_id: Option<&str>,
    kind: &str,
    payload: &serde_json::Value,
) -> Result<(), String> {
    let at = now_ms();
    let payload_json = redact_value(payload).to_string();
    conn.execute(
        "INSERT INTO pending_interactions
         (request_id,conversation_id,run_id,kind,state,payload_json,owner_worker_id,
          expires_at,created_at,updated_at)
         VALUES (?1,?2,?3,?4,'pending',?5,?6,?7,?8,?8)",
        params![
            request_id,
            conversation_id,
            run_id.filter(|id| !id.is_empty()),
            kind,
            payload_json,
            crate::agent::scheduler::current_worker_id(),
            at.saturating_add(WAIT_TIMEOUT_MS),
            at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn finish(request_id: &str, state: &str, response: serde_json::Value) -> Result<bool, String> {
    let Some(db) = crate::db::global() else {
        return Ok(false);
    };
    let conn = db.lock().map_err(|e| e.to_string())?;
    finish_with_conn(&conn, request_id, state, &response)
}

pub fn finish_with_conn(
    conn: &Connection,
    request_id: &str,
    state: &str,
    response: &serde_json::Value,
) -> Result<bool, String> {
    let at = now_ms();
    let response_json = redact_value(response).to_string();
    let changed = conn
        .execute(
            "UPDATE pending_interactions SET state=?1,response_json=?2,updated_at=?3,resolved_at=?3
             WHERE request_id=?4 AND state='pending'",
            params![state, response_json, at, request_id],
        )
        .map_err(|e| e.to_string())?;
    Ok(changed > 0)
}

pub fn cancel_conversation(conversation_id: &str, reason: &str) {
    let Some(db) = crate::db::global() else {
        return;
    };
    let Ok(conn) = db.lock() else { return };
    let at = now_ms();
    let _ = conn.execute(
        "UPDATE pending_interactions SET state='cancelled',response_json=?1,
         updated_at=?2,resolved_at=?2 WHERE conversation_id=?3 AND state='pending'
         AND owner_worker_id=?4",
        params![
            serde_json::json!({ "reason": reason }).to_string(),
            at,
            conversation_id,
            crate::agent::scheduler::current_worker_id(),
        ],
    );
}

/// Startup recovery only closes waits whose durable run has already been declared
/// interrupted/recovery-required. Live interactions owned by another healthy worker
/// remain untouched.
pub fn recover_orphaned(conn: &Connection) -> Result<usize, String> {
    let at = now_ms();
    conn.execute(
        "UPDATE pending_interactions SET state='interrupted',
         response_json=?1,updated_at=?2,resolved_at=?2
         WHERE state='pending' AND (
           run_id IS NULL OR run_id='' OR
           NOT EXISTS(SELECT 1 FROM agent_runs r WHERE r.run_id=pending_interactions.run_id) OR
           EXISTS(SELECT 1 FROM agent_runs r WHERE r.run_id=pending_interactions.run_id
                  AND r.state IN ('completed','failed','cancelled','interrupted')) OR
           EXISTS(SELECT 1 FROM agent_task_queue q WHERE q.run_id=pending_interactions.run_id
                  AND q.state='recovery_required')
         )",
        params![
            serde_json::json!({
                "reason": "reply_channel_lost",
                "recovery": "review_and_continue_from_checkpoint"
            })
            .to_string(),
            at,
        ],
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE conversations(id TEXT PRIMARY KEY);
             CREATE TABLE agent_runs(run_id TEXT PRIMARY KEY,state TEXT);
             CREATE TABLE agent_task_queue(run_id TEXT PRIMARY KEY,state TEXT);
             INSERT INTO conversations VALUES('c1');
             CREATE TABLE pending_interactions(
               request_id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
               run_id TEXT,kind TEXT NOT NULL,state TEXT NOT NULL,payload_json TEXT NOT NULL,response_json TEXT,
               owner_worker_id TEXT,expires_at INTEGER,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,resolved_at INTEGER);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn lifecycle_is_single_resolution() {
        let conn = db();
        begin_with_conn(
            &conn,
            "i1",
            "c1",
            None,
            "ask_user",
            &serde_json::json!({"q":"?"}),
        )
        .unwrap();
        assert!(
            finish_with_conn(&conn, "i1", "answered", &serde_json::json!({"answer":"ok"})).unwrap()
        );
        assert!(!finish_with_conn(&conn, "i1", "cancelled", &serde_json::json!({})).unwrap());
        let state: String = conn
            .query_row(
                "SELECT state FROM pending_interactions WHERE request_id='i1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "answered");
    }

    #[test]
    fn persisted_payload_and_response_are_redacted() {
        let conn = db();
        begin_with_conn(
            &conn,
            "secret",
            "c1",
            None,
            "ask_user",
            &serde_json::json!({ "token": "api_key=sk-abc1234567890abcdef" }),
        )
        .unwrap();
        finish_with_conn(
            &conn,
            "secret",
            "answered",
            &serde_json::json!({ "answer": "Bearer AbCdEf1234567890XyZ" }),
        )
        .unwrap();
        let (payload, response): (String, String) = conn
            .query_row(
                "SELECT payload_json,response_json FROM pending_interactions WHERE request_id='secret'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(!payload.contains("abc1234567890abcdef"));
        assert!(!response.contains("AbCdEf1234567890XyZ"));
    }

    #[test]
    fn recovery_preserves_live_worker_and_interrupts_terminal_run() {
        let conn = db();
        conn.execute("INSERT INTO agent_runs VALUES('live','running')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO agent_runs VALUES('dead','waiting_approval')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO agent_task_queue VALUES('live','running')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO agent_task_queue VALUES('dead','recovery_required')",
            [],
        )
        .unwrap();
        begin_with_conn(
            &conn,
            "i-live",
            "c1",
            Some("live"),
            "plan_review",
            &serde_json::json!({}),
        )
        .unwrap();
        begin_with_conn(
            &conn,
            "i-dead",
            "c1",
            Some("dead"),
            "plan_review",
            &serde_json::json!({}),
        )
        .unwrap();
        assert_eq!(recover_orphaned(&conn).unwrap(), 1);
        let live: String = conn
            .query_row(
                "SELECT state FROM pending_interactions WHERE request_id='i-live'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let dead: String = conn
            .query_row(
                "SELECT state FROM pending_interactions WHERE request_id='i-dead'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, "pending");
        assert_eq!(dead, "interrupted");
    }
}
