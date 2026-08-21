use deveco_switch::agent::tool_runtime;
use rusqlite::Connection;
use std::process::Command;

const DB_ENV: &str = "HARMONY_TOOL_WORKER_E2E_DB";

fn create_schema(conn: &Connection) {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;
         CREATE TABLE agent_task_queue(task_id TEXT);
         CREATE TABLE agent_workers(worker_id TEXT);
         CREATE TABLE tool_runs(id TEXT PRIMARY KEY,status TEXT,recovery_policy TEXT,trace_id TEXT,
           idempotency_key TEXT,effect_kind TEXT,result_json TEXT,execution_worker_id TEXT,
           lease_token TEXT,attempt INTEGER DEFAULT 0,heartbeat_at INTEGER,lease_expires_at INTEGER,
           verification_state TEXT DEFAULT 'none',recovery_count INTEGER DEFAULT 0,finished_at INTEGER,
           outcome_committed_at INTEGER,error_code TEXT);
         CREATE TABLE tool_execution_workers(worker_id TEXT PRIMARY KEY,process_worker_id TEXT,pid INTEGER,
           platform TEXT,state TEXT,capacity INTEGER,active_tools INTEGER,started_at INTEGER,
           last_heartbeat_at INTEGER,stopped_at INTEGER,metadata_json TEXT);
         CREATE TABLE tool_execution_attempts(call_id TEXT,attempt INTEGER,worker_id TEXT,lease_token TEXT,
           state TEXT,started_at INTEGER,last_heartbeat_at INTEGER,finished_at INTEGER,outcome_digest TEXT,
           error TEXT,PRIMARY KEY(call_id,attempt));",
    ).unwrap();
}

#[test]
fn crashed_tool_worker_leaves_effect_for_verification() {
    let path = std::env::temp_dir().join(format!(
        "harmony-tool-worker-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let conn = Connection::open(&path).unwrap();
    create_schema(&conn);
    conn.execute(
        "INSERT INTO tool_runs(id,status,recovery_policy,trace_id,idempotency_key,effect_kind)
         VALUES('call','prepared','verify','run','key','write')",
        [],
    )
    .unwrap();
    drop(conn);

    let status = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("crash_tool_worker_child")
        .arg("--nocapture")
        .env(DB_ENV, &path)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(87));

    let conn = Connection::open(&path).unwrap();
    conn.execute("UPDATE tool_execution_workers SET last_heartbeat_at=0", [])
        .unwrap();
    conn.execute("UPDATE tool_runs SET lease_expires_at=0", [])
        .unwrap();
    assert_eq!(tool_runtime::recover_stale(&conn, 30_000).unwrap(), 1);
    let (state, verification, recoveries): (String, String, i64) = conn
        .query_row(
            "SELECT status,verification_state,recovery_count FROM tool_runs WHERE id='call'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(state, "verification_required");
    assert_eq!(verification, "required");
    assert_eq!(recoveries, 1);
    let attempt: String = conn
        .query_row(
            "SELECT state FROM tool_execution_attempts WHERE call_id='call'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(attempt, "interrupted");
    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn crash_tool_worker_child() {
    let Ok(path) = std::env::var(DB_ENV) else {
        return;
    };
    let conn = Connection::open(path).unwrap();
    tool_runtime::start_attempt(&conn, "call", 30_000)
        .unwrap()
        .unwrap();
    std::process::exit(87);
}
