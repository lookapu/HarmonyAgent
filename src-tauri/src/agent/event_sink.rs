//! Agent 事件输出边界。
//!
//! headless 与后续 UI/kernel adapter 共享这一层：一次事实写入 session_events，
//! 同时产生可审计的 trajectory 事件，避免两套日志手写后发生漂移。

use crate::agent::eval_trajectory::TrajectoryEvent;
use crate::agent::session_events::{append_event, SessionEventType};
use crate::db::DbState;
use rusqlite::Connection;
use serde_json::Value;
use std::sync::{Arc, Mutex};

pub trait AgentEventSink: Send {
    fn append(
        &mut self,
        event_type: SessionEventType,
        payload: Value,
        trajectory_kind: &str,
        trajectory_fields: Value,
    ) -> Result<(), String>;
}

pub struct SessionTrajectorySink {
    conn: Arc<Mutex<Connection>>,
    conversation_id: String,
    trace_id: String,
    trajectory: Vec<TrajectoryEvent>,
}

impl SessionTrajectorySink {
    pub fn in_memory(conversation_id: String, trace_id: String) -> Result<Self, String> {
        let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE session_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                payload TEXT NOT NULL DEFAULT '{}',
                trace_id TEXT,
                created_at INTEGER NOT NULL DEFAULT (unixepoch())
            );",
        )
        .map_err(|e| format!("创建 headless 事件库失败：{e}"))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            conversation_id,
            trace_id,
            trajectory: Vec::new(),
        })
    }

    pub fn from_db(
        db: &DbState,
        conversation_id: String,
        trace_id: String,
    ) -> Result<Self, String> {
        let conn = db.0.lock().map_err(|error| error.to_string())?;
        let has_events: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='session_events'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| format!("检查 headless 事件 schema 失败：{error}"))?;
        if !has_events {
            return Err("headless 事件库缺少 session_events schema".into());
        }
        drop(conn);
        Ok(Self {
            conn: db.0.clone(),
            conversation_id,
            trace_id,
            trajectory: Vec::new(),
        })
    }

    pub fn connection(&self) -> Arc<Mutex<Connection>> {
        self.conn.clone()
    }

    pub fn into_trajectory(self) -> Vec<TrajectoryEvent> {
        self.trajectory
    }
}

impl AgentEventSink for SessionTrajectorySink {
    fn append(
        &mut self,
        event_type: SessionEventType,
        payload: Value,
        trajectory_kind: &str,
        trajectory_fields: Value,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|error| error.to_string())?;
        append_event(
            &conn,
            &self.conversation_id,
            event_type,
            payload,
            Some(&self.trace_id),
        )?;
        drop(conn);
        self.trajectory.push(TrajectoryEvent {
            ts: chrono::Utc::now().to_rfc3339(),
            kind: trajectory_kind.to_string(),
            fields: trajectory_fields,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::session_events::replay;

    #[test]
    fn writes_session_event_and_trajectory_together() {
        let mut sink = SessionTrajectorySink::in_memory("conv".into(), "trace".into()).unwrap();
        sink.append(
            SessionEventType::SystemNote,
            serde_json::json!({"text":"x"}),
            "system_note",
            serde_json::json!({"text":"x"}),
        )
        .unwrap();
        assert_eq!(sink.trajectory.len(), 1);
        let conn = sink.connection();
        assert_eq!(replay(&conn.lock().unwrap(), "conv").unwrap().len(), 1);
    }
}
