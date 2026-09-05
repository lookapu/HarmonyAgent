//! 真实 Agent 评测 trajectory 事件流（docs/AGENT_EVAL_HARNESS.md §4 `trajectory.jsonl`）。
//!
//! 事件采用统一信封（时间戳 + kind + 任意 JSON 字段），逐行 JSONL 落盘，边写边算 SHA-256，
//! 供 `report.json` 的 `trajectory_digest` 引用。事件源（AgentEventSink）待抽取后接入，
//! 但信封与落盘契约先固定，避免 sink 落地时再改 schema。

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct TrajectoryEvent {
    /// RFC 3339 或 unix 秒；由调用方写入，保证跨进程可排序。
    pub ts: String,
    pub kind: String,
    #[serde(flatten)]
    pub fields: serde_json::Value,
}

pub struct TrajectoryWriter {
    file: File,
    hasher: Sha256,
    lines: u64,
}

impl TrajectoryWriter {
    pub fn create(path: &Path) -> Result<Self, String> {
        let file = File::create(path).map_err(|error| format!("创建 trajectory 文件失败：{error}"))?;
        Ok(Self {
            file,
            hasher: Sha256::new(),
            lines: 0,
        })
    }

    pub fn append(&mut self, event: &TrajectoryEvent) -> Result<(), String> {
        let mut line =
            serde_json::to_string(event).map_err(|error| format!("序列化 trajectory 事件失败：{error}"))?;
        line.push('\n');
        self.file
            .write_all(line.as_bytes())
            .map_err(|error| format!("写入 trajectory 失败：{error}"))?;
        self.hasher.update(line.as_bytes());
        self.lines += 1;
        Ok(())
    }

    pub fn finish(self) -> Result<(u64, String), String> {
        self.file
            .sync_all()
            .map_err(|error| format!("同步 trajectory 失败：{error}"))?;
        let digest = format!("{:x}", self.hasher.finalize());
        Ok((self.lines, digest))
    }
}

/// 追加模式，供同一次 trial 恢复续写。返回的 writer 在继续前不会重算已有内容，
/// digest 从本进程续写开始累计；如需整文件 digest 应在续写结束后对文件整体重算。
pub fn append_existing(path: &Path) -> Result<TrajectoryWriter, String> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("打开 trajectory 文件失败：{error}"))?;
    Ok(TrajectoryWriter {
        file,
        hasher: Sha256::new(),
        lines: 0,
    })
}

/// 把已有会话事件日志（`agent::session_events`）回放进 trajectory 写入器，
/// 复用真实事件源而不是另造一套。返回写入的事件条数。
pub fn session_events_to_trajectory(
    conn: &rusqlite::Connection,
    conversation_id: &str,
    writer: &mut TrajectoryWriter,
) -> Result<usize, String> {
    let events = crate::agent::session_events::replay(conn, conversation_id)?;
    let mut written = 0usize;
    for event in events {
        writer.append(&TrajectoryEvent {
            ts: event.created_at.to_string(),
            kind: event.event_type.as_str().to_string(),
            fields: event.payload,
        })?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn temp_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("deveco-eval-traj-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn writer_appends_jsonl_and_computes_digest() {
        let path = temp_path();
        let mut writer = TrajectoryWriter::create(&path).unwrap();
        writer
            .append(&TrajectoryEvent {
                ts: "2026-09-05T00:00:00Z".into(),
                kind: "tool_call".into(),
                fields: serde_json::json!({ "tool": "search_symbols", "ok": true }),
            })
            .unwrap();
        writer
            .append(&TrajectoryEvent {
                ts: "2026-09-05T00:00:01Z".into(),
                kind: "tool_result".into(),
                fields: serde_json::json!({ "tool": "search_symbols", "chars": 123 }),
            })
            .unwrap();
        let (lines, digest) = writer.finish().unwrap();
        assert_eq!(lines, 2);
        assert_eq!(digest.len(), 64);

        let mut contents = String::new();
        File::open(&path)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        let events: Vec<serde_json::Value> = contents
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["kind"], "tool_call");
        assert_eq!(events[0]["tool"], "search_symbols");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn append_existing_keeps_prior_lines() {
        let path = temp_path();
        let mut writer = TrajectoryWriter::create(&path).unwrap();
        writer
            .append(&TrajectoryEvent {
                ts: "0".into(),
                kind: "first".into(),
                fields: serde_json::Value::Null,
            })
            .unwrap();
        drop(writer.finish().unwrap());

        let mut resumed = append_existing(&path).unwrap();
        resumed
            .append(&TrajectoryEvent {
                ts: "1".into(),
                kind: "second".into(),
                fields: serde_json::Value::Null,
            })
            .unwrap();
        let (appended, _) = resumed.finish().unwrap();
        assert_eq!(appended, 1);

        let mut contents = String::new();
        File::open(&path)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents.lines().count(), 2);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn replays_session_events_into_trajectory() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                payload TEXT NOT NULL DEFAULT '{}',
                trace_id TEXT,
                created_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE INDEX idx_session_events_conv_seq ON session_events(conversation_id, seq);",
        )
        .unwrap();
        crate::agent::session_events::append_event(
            &conn,
            "eval-1",
            crate::agent::session_events::SessionEventType::UserMessage,
            serde_json::json!({ "content": "fix it" }),
            None,
        )
        .unwrap();
        crate::agent::session_events::append_event(
            &conn,
            "eval-1",
            crate::agent::session_events::SessionEventType::ToolCall,
            serde_json::json!({ "name": "read_file", "args": {} }),
            Some("tr-1"),
        )
        .unwrap();

        let path = temp_path();
        let mut writer = TrajectoryWriter::create(&path).unwrap();
        let written = session_events_to_trajectory(&conn, "eval-1", &mut writer).unwrap();
        assert_eq!(written, 2);
        drop(writer.finish().unwrap());

        let mut contents = String::new();
        File::open(&path)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        let events: Vec<serde_json::Value> = contents
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["kind"], "user_message");
        assert_eq!(events[1]["kind"], "tool_call");
        assert_eq!(events[1]["name"], "read_file");
        std::fs::remove_file(path).ok();
    }
}
