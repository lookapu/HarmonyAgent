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
    /// 工具审批决议（payload: { tool, approved, remember?, scope? }）——与沙箱升级、
    /// 工具调用同源进入统一审计链，可回放/进入 eval trajectory。
    ToolApproval,
    /// 系统说明（payload: { text }）
    SystemNote,
    /// 上下文压缩（payload: { trigger, old_limit?, new_limit?, keep? }）——LC-33：
    /// 压缩预警与执行写入事件流，度量预警后用户固定行为与“无预兆压缩”体验
    ContextCompress,
}

impl SessionEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UserMessage => "user_message",
            Self::AssistantMessage => "assistant_message",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
            Self::ToolApproval => "tool_approval",
            Self::SystemNote => "system_note",
            Self::ContextCompress => "context_compress",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "user_message" => Self::UserMessage,
            "assistant_message" => Self::AssistantMessage,
            "tool_call" => Self::ToolCall,
            "tool_result" => Self::ToolResult,
            "tool_approval" => Self::ToolApproval,
            "context_compress" => Self::ContextCompress,
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

/// 统一审计时间线的一条事件：把会话事件与运行事件合并到同一可查询链。
#[derive(Clone, Debug, serde::Serialize)]
pub struct AuditEvent {
    /// "session"（消息生命周期/审批）| "run"（沙箱升级等运行级事件）
    pub source: String,
    pub event_type: String,
    pub payload: Value,
    pub created_at: i64,
    pub run_id: Option<String>,
    pub trace_id: Option<String>,
}

/// 统一审计链：把 `session_events`（含审批决议）与 `run_events`（含沙箱升级）合并为
/// 一条按时间排序的审计时间线。只读，不改动任何写入路径。
pub fn audit_timeline(conn: &Connection, conversation_id: &str) -> Result<Vec<AuditEvent>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT 'session' AS source, event_type, payload, created_at, NULL AS run_id, trace_id
             FROM session_events WHERE conversation_id = ?1
             UNION ALL
             SELECT 'run' AS source, event_type, payload, created_at, run_id, NULL AS trace_id
             FROM run_events WHERE conversation_id = ?1
             ORDER BY created_at ASC, source ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([conversation_id], |row| {
            let payload: String = row.get(2)?;
            Ok(AuditEvent {
                source: row.get(0)?,
                event_type: row.get(1)?,
                payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                created_at: row.get(3)?,
                run_id: row.get(4)?,
                trace_id: row.get(5)?,
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
            // 压缩事件不进消息历史投影（摘要/裁剪由 conversations 表水位承载）
            SessionEventType::ContextCompress => {}
            // 审批决议只进审计链，不进消息历史投影
            SessionEventType::ToolApproval => {}
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
    fn audit_timeline_merges_session_and_run_events() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id TEXT NOT NULL, seq INTEGER NOT NULL,
                event_type TEXT NOT NULL, payload TEXT NOT NULL DEFAULT '{}',
                trace_id TEXT, created_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE TABLE agent_runs(run_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL,
                goal TEXT NOT NULL DEFAULT '', state TEXT NOT NULL, phase TEXT NOT NULL,
                attempt INTEGER NOT NULL DEFAULT 1, last_event_seq INTEGER NOT NULL DEFAULT 0,
                recovery_count INTEGER NOT NULL DEFAULT 0, resume_policy TEXT NOT NULL DEFAULT 'continue',
                acceptance_json TEXT, metadata_json TEXT NOT NULL DEFAULT '{}', error TEXT,
                started_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, finished_at INTEGER,
                parent_run_id TEXT, recovery_plan_json TEXT, recovery_mode TEXT NOT NULL DEFAULT 'fresh',
                goal_contract_json TEXT, remediation_count INTEGER NOT NULL DEFAULT 0,
                heartbeat_at INTEGER, lease_expires_at INTEGER, quality_json TEXT);
            CREATE TABLE run_events(event_id TEXT PRIMARY KEY, run_id TEXT NOT NULL,
                conversation_id TEXT NOT NULL, seq INTEGER NOT NULL, event_type TEXT NOT NULL,
                payload TEXT NOT NULL, created_at INTEGER NOT NULL, UNIQUE(run_id,seq));",
        )
        .unwrap();
        append_event(&conn, "c1", SessionEventType::ToolApproval, serde_json::json!({"tool": "git_push", "approved": true}), None).unwrap();
        conn.execute(
            "INSERT INTO agent_runs(run_id, conversation_id, goal, state, phase, started_at, updated_at)
             VALUES('r1','c1','','running','execute',100,101)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO run_events VALUES('e1','r1','c1',1,'sandbox_started','{}',200)",
            [],
        ).unwrap();

        let timeline = audit_timeline(&conn, "c1").unwrap();
        assert_eq!(timeline.len(), 2);
        // 两条来源的事件都进入统一时间线，且按 created_at 升序
        let sources: Vec<&str> = timeline.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"session") && sources.contains(&"run"));
        let created: Vec<i64> = timeline.iter().map(|e| e.created_at).collect();
        assert!(created.windows(2).all(|w| w[0] <= w[1]));
        let session_ev = timeline.iter().find(|e| e.source == "session").unwrap();
        assert_eq!(session_ev.event_type, "tool_approval");
        let run_ev = timeline.iter().find(|e| e.source == "run").unwrap();
        assert_eq!(run_ev.event_type, "sandbox_started");
        assert_eq!(run_ev.run_id.as_deref(), Some("r1"));
        // 会话隔离
        assert!(audit_timeline(&conn, "c2").unwrap().is_empty());
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
