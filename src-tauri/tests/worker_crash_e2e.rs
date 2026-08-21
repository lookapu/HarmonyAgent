//! 真实跨进程故障恢复：子进程认领任务后直接异常退出，父进程回收失联 Owner，
//! 第二个 Worker 接管；旧 fencing token 必须无法再写检查点。

use deveco_switch::agent::scheduler::{self, EnqueueSpec};
use rusqlite::Connection;
use std::process::Command;

const CHILD_DB_ENV: &str = "HARMONY_AGENT_WORKER_E2E_DB";
const CHILD_MODE_ENV: &str = "HARMONY_AGENT_WORKER_E2E_MODE";

fn open(path: &std::path::Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; CREATE TABLE IF NOT EXISTS conversations(id TEXT PRIMARY KEY); CREATE TABLE IF NOT EXISTS agent_runs(run_id TEXT PRIMARY KEY,scheduler_task_id TEXT,budget_json TEXT); CREATE TABLE IF NOT EXISTS agent_task_queue(task_id TEXT PRIMARY KEY,run_id TEXT UNIQUE,conversation_id TEXT,goal TEXT,state TEXT,priority INTEGER,worker_id TEXT,attempt INTEGER,max_attempts INTEGER,lease_expires_at INTEGER,budget_json TEXT,checkpoint_json TEXT,error TEXT,created_at INTEGER,updated_at INTEGER,finished_at INTEGER,payload_json TEXT NOT NULL DEFAULT '{}',resume_token TEXT,claimed_at INTEGER,last_checkpoint_at INTEGER,next_attempt_at INTEGER,concurrency_key TEXT,tenant_id TEXT NOT NULL DEFAULT 'local',lease_token TEXT,claim_epoch INTEGER NOT NULL DEFAULT 0,last_worker_id TEXT,recovery_count INTEGER NOT NULL DEFAULT 0); CREATE TABLE IF NOT EXISTS agent_workers(worker_id TEXT PRIMARY KEY,worker_kind TEXT,pid INTEGER,hostname TEXT,version TEXT,state TEXT,capacity INTEGER,active_tasks INTEGER,started_at INTEGER,last_heartbeat_at INTEGER,draining_at INTEGER,stopped_at INTEGER,metadata_json TEXT); CREATE TABLE IF NOT EXISTS agent_task_attempts(task_id TEXT,attempt INTEGER,worker_id TEXT,lease_token TEXT,state TEXT,checkpoint_json TEXT,error TEXT,started_at INTEGER,last_heartbeat_at INTEGER,finished_at INTEGER,PRIMARY KEY(task_id,attempt));").unwrap();
    conn
}

#[test]
fn crash_worker_claims_task() {
    let Some(path) = std::env::var_os(CHILD_DB_ENV) else {
        return;
    };
    let mode = std::env::var(CHILD_MODE_ENV).unwrap_or_else(|_| "crash".into());
    let worker_id = if mode == "hold" {
        "process-worker-live"
    } else {
        "process-worker-crashed"
    };
    let conn = open(std::path::Path::new(&path));
    scheduler::register_worker(&conn, worker_id, "e2e", 1).unwrap();
    let claimed = scheduler::claim_next_for_worker(&conn, worker_id, 10_000)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.run_id, "run-e2e");
    if mode == "hold" {
        std::thread::sleep(std::time::Duration::from_secs(5));
        return;
    }
    // 模拟 kill -9 / TerminateProcess：不执行 Worker stop，也不走 Rust Drop。
    std::process::exit(86);
}

#[test]
fn live_process_owner_is_not_stolen_by_another_startup() {
    let path = std::env::temp_dir().join(format!(
        "harmony-agent-worker-live-e2e-{}.db",
        uuid::Uuid::new_v4()
    ));
    {
        let conn = open(&path);
        conn.execute("INSERT INTO conversations(id) VALUES('c')", []).unwrap();
        conn.execute("INSERT INTO agent_runs(run_id) VALUES('run-e2e')", []).unwrap();
        scheduler::enqueue(&conn, &EnqueueSpec {
            run_id:"run-e2e".into(), conversation_id:"c".into(), goal:"keep live owner".into(),
            priority:90, max_attempts:3, concurrency_key:None,
            payload:serde_json::json!({}), budget:serde_json::json!({}),
        }).unwrap();
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--exact").arg("crash_worker_claims_task").arg("--nocapture")
        .env(CHILD_DB_ENV, &path).env(CHILD_MODE_ENV, "hold").spawn().unwrap();
    let conn = open(&path);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if scheduler::get(&conn, "run-e2e").unwrap().is_some_and(|task| task.state == "running") {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "子 Worker 未及时认领任务");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    scheduler::register_worker(&conn, "process-worker-observer", "e2e", 1).unwrap();
    assert_eq!(scheduler::recover_stale_owners(&conn, 60_000).unwrap(), 0);
    let task = scheduler::get(&conn, "run-e2e").unwrap().unwrap();
    assert_eq!(task.state, "running");
    assert_eq!(task.worker_id.as_deref(), Some("process-worker-live"));
    child.kill().unwrap();
    let _ = child.wait();
    drop(conn);
    for candidate in [path.clone(), path.with_extension("db-wal"), path.with_extension("db-shm")] {
        let _ = std::fs::remove_file(candidate);
    }
}

#[test]
fn crashed_process_is_recovered_and_old_owner_is_fenced() {
    let path = std::env::temp_dir().join(format!(
        "harmony-agent-worker-e2e-{}.db",
        uuid::Uuid::new_v4()
    ));
    {
        let conn = open(&path);
        conn.execute("INSERT INTO conversations(id) VALUES('c')", [])
            .unwrap();
        conn.execute("INSERT INTO agent_runs(run_id) VALUES('run-e2e')", [])
            .unwrap();
        scheduler::enqueue(
            &conn,
            &EnqueueSpec {
                run_id: "run-e2e".into(),
                conversation_id: "c".into(),
                goal: "survive process crash".into(),
                priority: 90,
                max_attempts: 3,
                concurrency_key: Some("workspace:e2e".into()),
                payload: serde_json::json!({"e2e":true}),
                budget: serde_json::json!({}),
            },
        )
        .unwrap();
    }

    let status = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("crash_worker_claims_task")
        .arg("--nocapture")
        .env(CHILD_DB_ENV, &path)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(86));

    let conn = open(&path);
    let old = scheduler::get(&conn, "run-e2e").unwrap().unwrap();
    let old_token = old.lease_token.clone().unwrap();
    assert_eq!(old.worker_id.as_deref(), Some("process-worker-crashed"));
    conn.execute(
        "UPDATE agent_workers SET last_heartbeat_at=0 WHERE worker_id='process-worker-crashed'",
        [],
    )
    .unwrap();
    assert_eq!(scheduler::recover_stale_owners(&conn, 0).unwrap(), 1);
    scheduler::register_worker(&conn, "process-worker-takeover", "e2e", 1).unwrap();
    let takeover = scheduler::claim_next_for_worker(&conn, "process-worker-takeover", 10_000)
        .unwrap()
        .unwrap();
    assert_eq!(takeover.claim_epoch, old.claim_epoch + 1);
    assert_eq!(takeover.recovery_count, 1);
    assert!(scheduler::checkpoint_owned(
        &conn,
        "run-e2e",
        "process-worker-crashed",
        &old_token,
        &serde_json::json!({"stale":true}),
        10_000
    )
    .is_err());
    scheduler::checkpoint_owned(
        &conn,
        "run-e2e",
        "process-worker-takeover",
        takeover.lease_token.as_deref().unwrap(),
        &serde_json::json!({"recovered":true}),
        10_000,
    )
    .unwrap();
    let attempts: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM agent_task_attempts WHERE task_id='task:run-e2e'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(attempts, 2);
    drop(conn);
    for candidate in [
        path.clone(),
        path.with_extension("db-wal"),
        path.with_extension("db-shm"),
    ] {
        let _ = std::fs::remove_file(candidate);
    }
}
