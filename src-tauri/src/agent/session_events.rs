//! 会话事件日志（事件溯源 Session 的补充真源）
//!
//! 事件溯源（Event Sourcing）：把会话状态变迁建模为**仅追加的事件日志**，
//! 事件是事实（事实不可变、只追加），消息历史等视图从事件**派生**（投影）。
//! 参考 deepseek-harness 的 SessionEventMap：本模块提供
//! 消息生命周期的类型化事件（用户消息 / 助手消息 / 工具调用 / 工具结果 / 系统说明）。
//!
//! 落地方式（务实渐进）：现有 `messages` 表仍是主存储，`session_events`
//! 在消息落库路径旁追加事件记录，提供 `replay`（回放）与 `derive_messages`
//! （事件 → 消息历史投影）能力；后续可平滑升级为唯一真源。

use rusqlite::Connection;
use serde_json::Value;

/// 会话事件类型：消息生命周期的类型化事实。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEventType {
    /// 用户消息（payload: { content, references? }）
    UserMessage,
    /// 助手消息完成（payload: { content, model?, reasoning?, tokens_in?, tokens_out? }）
    AssistantMessage,
    /// 工具调用（payload: { name, args }）
    ToolCall,
    /// 工具结果（payload: { ok, output }）
    ToolResult,
    /// 系统说明（payload: { text }）
    SystemNote,
}

impl SessionEventType {
    fn as_str(&self) -> &'static str {
        match self {
            Self::UserMessage => "user_message",
            Self::AssistantMessage => "assistant_message",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
            Self::SystemNote => "system_note",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "user_message" => Self::UserMessage,
            "assistant_message" => Self::AssistantMessage,
            "tool_call" => Self::ToolCall,
            "tool_result" => Self::ToolResult,
            _ => Self::SystemNote,
        }
    }
}

/// 一条会话事件（追加后不可变）。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SessionEvent {
    pub id: i64,
    pub conversation_id: String,
    /// 会话内单调递增序号（回放顺序）
    pub seq: i64,
    pub event_type: SessionEventType,
    pub payload: Value,
    /// 任务级 Trace ID：一次任务（一次用户消息触发的完整执行）的所有事件共享同一 ID，
    /// 全链路可 grep / 前端 timeline 按它折叠
    pub trace_id: Option<String>,
    pub created_at: i64,
}

/// 从事件日志派生的消息历史条目（事件投影视图）。
#[derive(Clone, Debug, serde::Serialize)]
pub struct DerivedMessage {
    /// user | assistant | tool
    pub role: String,
    pub content: String,
    /// role=tool 时的工具名
    pub tool_name: Option<String>,
    pub created_at: i64,
}

/// 追加一条事件（seq 自动递增）。返回新事件的 seq。
/// `trace_id`：任务级全链路 ID（None 表示不属于任何任务，如系统事件）。
pub fn append_event(
    conn: &Connection,
    conversation_id: &str,
    event_type: SessionEventType,
    payload: Value,
    trace_id: Option<&str>,
) -> Result<i64, String> {
    let seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM session_events WHERE conversation_id = ?1",
            [conversation_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO session_events (conversation_id, seq, event_type, payload, trace_id) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![conversation_id, seq, event_type.as_str(), payload.to_string(), trace_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(seq)
}

/// 回放某会话的全部事件（按 seq 升序）。
pub fn replay(conn: &Connection, conversation_id: &str) -> Result<Vec<SessionEvent>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, conversation_id, seq, event_type, payload, trace_id, created_at
             FROM session_events WHERE conversation_id = ?1 ORDER BY seq ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([conversation_id], |row| {
            let raw: String = row.get(3)?;
            let payload: String = row.get(4)?;
            Ok(SessionEvent {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                seq: row.get(2)?,
                event_type: SessionEventType::from_str(&raw),
                payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                trace_id: row.get(5)?,
                created_at: row.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// 事件 → 消息历史投影（核心派生：消息历史从事件日志重建）。
/// 工具调用/结果配对为 assistant 视角的 tool 条目，保持与消息表的语义对齐。
pub fn derive_messages(conn: &Connection, conversation_id: &str) -> Result<Vec<DerivedMessage>, String> {
    let events = replay(conn, conversation_id)?;
    let mut out: Vec<DerivedMessage> = Vec::new();
    for ev in events {
        match ev.event_type {
            SessionEventType::UserMessage => {
                let content = ev.payload.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                out.push(DerivedMessage { role: "user".into(), content, tool_name: None, created_at: ev.created_at });
            }
            SessionEventType::AssistantMessage => {
                let content = ev.payload.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                out.push(DerivedMessage { role: "assistant".into(), content, tool_name: None, created_at: ev.created_at });
            }
            SessionEventType::ToolCall => {
                let name = ev.payload.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let args = ev.payload.get("args").map(|a| a.to_string()).unwrap_or_default();
                let output = ev.payload.get("output").and_then(|v| v.as_str()).unwrap_or("");
                let mut content = format!("工具调用：{name} {args}");
                if !output.is_empty() {
                    content.push_str(&format!("\n{output}"));
                }
                out.push(DerivedMessage {
                    role: "tool".into(),
                    content,
                    tool_name: Some(name),
                    created_at: ev.created_at,
                });
            }
            SessionEventType::ToolResult => {
                let ok = ev.payload.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                let output = ev.payload.get("output").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let prefix = if ok { "工具结果（成功）：" } else { "工具结果（失败）：" };
                out.push(DerivedMessage { role: "tool".into(), content: format!("{prefix}{output}"), tool_name: None, created_at: ev.created_at });
            }
            SessionEventType::SystemNote => {
                let text = ev.payload.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                out.push(DerivedMessage { role: "assistant".into(), content: text, tool_name: None, created_at: ev.created_at });
            }
        }
    }
    Ok(out)
}

/// 统计某会话的事件条数（成本/诊断用）。
pub fn count_events(conn: &Connection, conversation_id: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM session_events WHERE conversation_id = ?1",
        [conversation_id],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// 删除某会话的全部事件（会话删除时级联清理）。
pub fn delete_conversation_events(conn: &Connection, conversation_id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM session_events WHERE conversation_id = ?1", [conversation_id])
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
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
        conn
    }

    #[test]
    fn append_replay_roundtrip() {
        let conn = mem_conn();
        append_event(&conn, "c1", SessionEventType::UserMessage, serde_json::json!({"content": "hi"}), None).unwrap();
        append_event(&conn, "c1", SessionEventType::ToolCall, serde_json::json!({"name": "read_file", "args": {}}), Some("tr-1")).unwrap();
        append_event(&conn, "c1", SessionEventType::ToolResult, serde_json::json!({"ok": true, "output": "file content"}), Some("tr-1")).unwrap();
        append_event(&conn, "c1", SessionEventType::AssistantMessage, serde_json::json!({"content": "done"}), Some("tr-1")).unwrap();
        // seq 单调递增
        let events = replay(&conn, "c1").unwrap();
        assert_eq!(events.len(), 4);
        assert_eq!(events.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        assert_eq!(count_events(&conn, "c1"), 4);
        // trace_id 落库：任务内事件共享同一 ID，用户消息无 trace
        assert_eq!(events[0].trace_id, None);
        assert_eq!(events[1].trace_id.as_deref(), Some("tr-1"));
        assert_eq!(events[2].trace_id.as_deref(), Some("tr-1"));
        assert_eq!(events[3].trace_id.as_deref(), Some("tr-1"));
        // 会话隔离
        assert_eq!(count_events(&conn, "c2"), 0);
    }

    #[test]
    fn derive_projects_messages_from_events() {
        let conn = mem_conn();
        append_event(&conn, "c1", SessionEventType::UserMessage, serde_json::json!({"content": "读一下"}), None).unwrap();
        append_event(&conn, "c1", SessionEventType::ToolCall, serde_json::json!({"name": "read_file", "args": {"path": "a.txt"}}), Some("t1")).unwrap();
        append_event(&conn, "c1", SessionEventType::ToolResult, serde_json::json!({"ok": false, "output": "not found"}), Some("t1")).unwrap();
        append_event(&conn, "c1", SessionEventType::AssistantMessage, serde_json::json!({"content": "文件不存在"}), Some("t1")).unwrap();
        let msgs = derive_messages(&conn, "c1").unwrap();
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "读一下");
        assert_eq!(msgs[1].role, "tool");
        assert_eq!(msgs[1].tool_name.as_deref(), Some("read_file"));
        assert!(msgs[1].content.contains("read_file"));
        assert!(msgs[2].content.contains("失败"));
        assert_eq!(msgs[3].role, "assistant");
    }

    #[test]
    fn delete_conversation_cleans_events() {
        let conn = mem_conn();
        append_event(&conn, "c1", SessionEventType::SystemNote, serde_json::json!({"text": "note"}), None).unwrap();
        delete_conversation_events(&conn, "c1").unwrap();
        assert_eq!(count_events(&conn, "c1"), 0);
    }
}
