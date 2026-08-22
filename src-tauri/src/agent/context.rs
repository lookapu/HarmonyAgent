//! Conversation Context V2。
//!
//! 长会话上下文是 messages/session_events/agent_runs 等真实数据的可重建投影：
//! - 热上下文由最近消息实时组装；
//! - 任务状态来自 Durable Run、目标契约和执行步骤；
//! - 项目/环境事实必须携带来源与失效条件；
//! - 历史摘要只负责导航，并记录覆盖游标，不能替代原始证据。

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CONTEXT_SCHEMA_VERSION: i64 = 2;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextLayer {
    Hot,
    Task,
    Project,
    Archive,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextSource {
    pub kind: String,
    pub reference: String,
    pub observed_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextFactV2 {
    pub id: String,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub run_id: Option<String>,
    pub fact_kind: String,
    pub fact_key: String,
    pub value: serde_json::Value,
    pub source: ContextSource,
    pub scope: String,
    pub confidence: f64,
    pub version: i64,
    pub invalidated_at: Option<i64>,
    pub invalidation_reason: Option<String>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextFactInput {
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub run_id: Option<String>,
    pub fact_kind: String,
    pub fact_key: String,
    pub value: serde_json::Value,
    pub source_kind: String,
    pub source_ref: String,
    pub scope: String,
    pub confidence: f64,
    pub observed_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskSnapshotV2 {
    pub run_id: Option<String>,
    pub goal: String,
    pub state: String,
    pub phase: String,
    pub required_conditions: Vec<String>,
    pub constraints: Vec<String>,
    pub completed_steps: Vec<String>,
    pub open_steps: Vec<String>,
    pub blocked_steps: Vec<String>,
    pub next_action: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextArtifactRef {
    pub id: String,
    pub conversation_id: String,
    pub run_id: Option<String>,
    pub artifact_kind: String,
    pub uri: String,
    pub label: String,
    pub digest: Option<String>,
    pub metadata: serde_json::Value,
    pub source_ref: String,
    pub valid: bool,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HotMessageV2 {
    pub role: String,
    pub content: String,
    pub source_ref: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingInteractionV2 {
    pub request_id: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub expires_at: Option<i64>,
    pub source_ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextPinV2 {
    pub id: String,
    pub pin_kind: String,
    pub source_ref: String,
    pub label: String,
    pub content: String,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HotContextV2 {
    pub recent_messages: Vec<HotMessageV2>,
    pub current_errors: Vec<String>,
    pub active_files: Vec<String>,
    pub pending_interactions: Vec<PendingInteractionV2>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextBudgetV2 {
    pub total_tokens: i64,
    pub reserved_output_tokens: i64,
    pub system_tokens: i64,
    pub task_tokens: i64,
    pub project_tokens: i64,
    pub archive_tokens: i64,
    pub hot_tokens: i64,
}

impl ContextBudgetV2 {
    pub fn allocate(total_tokens: i64) -> Self {
        let total_tokens = total_tokens.max(4_096);
        let reserved_output_tokens = (total_tokens / 8).clamp(1_024, 16_384);
        let input = total_tokens.saturating_sub(reserved_output_tokens);
        let system_tokens = input * 15 / 100;
        let task_tokens = input * 12 / 100;
        let project_tokens = input * 13 / 100;
        let archive_tokens = input * 15 / 100;
        let hot_tokens = input
            .saturating_sub(system_tokens)
            .saturating_sub(task_tokens)
            .saturating_sub(project_tokens)
            .saturating_sub(archive_tokens);
        Self {
            total_tokens,
            reserved_output_tokens,
            system_tokens,
            task_tokens,
            project_tokens,
            archive_tokens,
            hot_tokens,
        }
    }

    pub fn input_tokens(&self) -> i64 {
        self.total_tokens
            .saturating_sub(self.reserved_output_tokens)
    }
}

impl Default for ContextBudgetV2 {
    fn default() -> Self {
        Self::allocate(200_000)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversationContextV2 {
    pub schema_version: i64,
    pub conversation_id: String,
    pub summary: Option<String>,
    pub summary_from_message_rowid: i64,
    pub summary_to_message_rowid: i64,
    pub summary_event_seq: i64,
    pub task: TaskSnapshotV2,
    pub hot: HotContextV2,
    pub facts: Vec<ContextFactV2>,
    pub artifacts: Vec<ContextArtifactRef>,
    pub pins: Vec<ContextPinV2>,
    pub budget: ContextBudgetV2,
    pub facts_digest: Option<String>,
    pub invalidation_epoch: i64,
    pub reconciliation: ContextReconciliationState,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SummaryReconciliation {
    pub summary: String,
    pub status: String,
    pub conflicts: Vec<String>,
    pub authoritative_block: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContextReconciliationState {
    pub count: i64,
    pub latest_status: Option<String>,
    pub latest_conflicts: Vec<String>,
    pub latest_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct ContextCheckpoint<'a> {
    pub conversation_id: &'a str,
    pub run_id: Option<&'a str>,
    pub summary: Option<&'a str>,
    pub summary_from_message_rowid: i64,
    pub summary_to_message_rowid: i64,
    pub summary_event_seq: i64,
    pub task: &'a TaskSnapshotV2,
    pub budget: &'a ContextBudgetV2,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn set_context_pin(
    conn: &Connection,
    conversation_id: &str,
    pin_kind: &str,
    source_ref: &str,
    label: &str,
    content: &str,
    pinned: bool,
) -> Result<Option<ContextPinV2>, String> {
    if !matches!(pin_kind, "message" | "decision" | "file" | "acceptance") {
        return Err("pin_kind 仅支持 message|decision|file|acceptance".into());
    }
    let source_ref = source_ref.trim();
    if source_ref.is_empty() || source_ref.chars().count() > 500 {
        return Err("固定项来源为空或过长".into());
    }
    let project_id = conn
        .query_row(
            "SELECT project_id FROM conversations WHERE id=?1",
            [conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "会话不存在".to_string())?;
    if !pinned {
        conn.execute(
            "DELETE FROM conversation_context_pins
             WHERE conversation_id=?1 AND pin_kind=?2 AND source_ref=?3",
            params![conversation_id, pin_kind, source_ref],
        )
        .map_err(|e| e.to_string())?;
        return Ok(None);
    }
    let mut content = content.trim().chars().take(4_000).collect::<String>();
    let mut label = label.trim().chars().take(200).collect::<String>();
    if pin_kind == "message" {
        let message_id = source_ref.strip_prefix("message:").unwrap_or(source_ref);
        let message = conn
            .query_row(
                "SELECT role,content FROM messages WHERE id=?1 AND conversation_id=?2 AND hidden=0",
                params![message_id, conversation_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "消息不存在或不属于当前会话".to_string())?;
        if label.is_empty() {
            label = format!("{} message", message.0);
        }
        content = message.1.chars().take(4_000).collect();
    }
    if content.is_empty() {
        return Err("固定项内容不能为空".into());
    }
    if label.is_empty() {
        label = pin_kind.to_string();
    }
    let now = now_ms();
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO conversation_context_pins
         (id,conversation_id,project_id,pin_kind,source_ref,label,content,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)
         ON CONFLICT(conversation_id,pin_kind,source_ref) DO UPDATE SET
           label=excluded.label,content=excluded.content,updated_at=excluded.updated_at",
        params![id, conversation_id, project_id, pin_kind, source_ref, label, content, now],
    )
    .map_err(|e| e.to_string())?;
    load_context_pins(conn, conversation_id, 500).map(|pins| {
        pins.into_iter()
            .find(|pin| pin.pin_kind == pin_kind && pin.source_ref == source_ref)
    })
}

fn load_context_pins(
    conn: &Connection,
    conversation_id: &str,
    limit: usize,
) -> Result<Vec<ContextPinV2>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id,pin_kind,source_ref,label,content,updated_at
             FROM conversation_context_pins WHERE conversation_id=?1
             ORDER BY updated_at DESC LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![conversation_id, limit.clamp(1, 500) as i64], |row| {
            Ok(ContextPinV2 {
                id: row.get(0)?,
                pin_kind: row.get(1)?,
                source_ref: row.get(2)?,
                label: row.get(3)?,
                content: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

fn json_or_default<T>(raw: &str) -> T
where
    T: serde::de::DeserializeOwned + Default,
{
    serde_json::from_str(raw).unwrap_or_default()
}

pub fn capture_task_snapshot(
    conn: &Connection,
    conversation_id: &str,
) -> Result<TaskSnapshotV2, String> {
    let run = conn
        .query_row(
            "SELECT run_id,goal,state,phase,goal_contract_json,error,updated_at
             FROM agent_runs WHERE conversation_id=?1 ORDER BY started_at DESC LIMIT 1",
            [conversation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;

    let Some((run_id, goal, state, phase, contract_json, error, updated_at)) = run else {
        return snapshot_from_legacy_ledger(conn, conversation_id);
    };

    let contract = contract_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<crate::agent::acceptance::GoalContract>(raw).ok());
    let required_conditions = contract
        .as_ref()
        .map(|contract| {
            contract.criteria.iter()
                .filter(|criterion| criterion.required)
                .map(|criterion| criterion.label.clone())
                .collect()
        })
        .unwrap_or_default();
    let constraints = contract.map(|contract| contract.constraints).unwrap_or_default();

    let mut stmt = conn
        .prepare(
            "SELECT title,state,result_summary FROM execution_steps
             WHERE run_id=?1 ORDER BY ordinal,updated_at",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut completed_steps = Vec::new();
    let mut open_steps = Vec::new();
    let mut blocked_steps = Vec::new();
    for row in rows {
        let (title, step_state, result) = row.map_err(|e| e.to_string())?;
        let detail = result
            .filter(|text| !text.trim().is_empty())
            .map(|text| format!("{title}: {text}"))
            .unwrap_or(title);
        match step_state.as_str() {
            "completed" => completed_steps.push(detail),
            "failed" | "blocked" | "cancelled" => blocked_steps.push(detail),
            _ => open_steps.push(detail),
        }
    }
    completed_steps.truncate(20);
    open_steps.truncate(20);
    blocked_steps.truncate(10);

    Ok(TaskSnapshotV2 {
        run_id: Some(run_id),
        goal,
        state,
        phase,
        required_conditions,
        constraints,
        completed_steps,
        open_steps,
        blocked_steps,
        next_action: None,
        last_error: error,
        updated_at,
    })
}

fn snapshot_from_legacy_ledger(
    conn: &Connection,
    conversation_id: &str,
) -> Result<TaskSnapshotV2, String> {
    let ledger = conn
        .query_row(
            "SELECT ledger,updated_at FROM conversations WHERE id=?1",
            [conversation_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((Some(raw), updated_at)) = ledger else {
        return Ok(TaskSnapshotV2::default());
    };
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
    let list = |key: &str| {
        value[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let tool = item["tool"].as_str().unwrap_or("");
                let text = item["text"].as_str().unwrap_or("");
                (!tool.is_empty() || !text.is_empty()).then(|| format!("[{tool}] {text}"))
            })
            .collect::<Vec<_>>()
    };
    Ok(TaskSnapshotV2 {
        run_id: None,
        goal: value["goal"].as_str().unwrap_or("").to_string(),
        state: "legacy".into(),
        phase: "ledger".into(),
        required_conditions: Vec::new(),
        constraints: Vec::new(),
        completed_steps: list("verified"),
        open_steps: list("open"),
        blocked_steps: Vec::new(),
        next_action: value["next"].as_str().map(str::to_string),
        last_error: None,
        updated_at,
    })
}

pub fn upsert_fact(conn: &Connection, input: &ContextFactInput) -> Result<ContextFactV2, String> {
    let now = now_ms();
    let observed_at = input.observed_at.unwrap_or(now);
    let value_json = serde_json::to_string(&input.value).map_err(|e| e.to_string())?;
    let confidence = input.confidence.clamp(0.0, 1.0);
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let current = tx
        .query_row(
            "SELECT id,value_json,source_kind,source_ref,version,created_at
             FROM conversation_context_facts
             WHERE conversation_id=?1 AND fact_kind=?2 AND fact_key=?3 AND invalidated_at IS NULL",
            params![input.conversation_id, input.fact_kind, input.fact_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;

    let (id, version, created_at) = match current {
        Some((id, old_value, old_source_kind, old_source_ref, version, created_at))
            if old_value == value_json
                && old_source_kind == input.source_kind
                && old_source_ref == input.source_ref =>
        {
            tx.execute(
                "UPDATE conversation_context_facts SET project_id=?1,run_id=?2,scope=?3,
                 confidence=?4,observed_at=?5,updated_at=?6 WHERE id=?7",
                params![
                    input.project_id,
                    input.run_id,
                    input.scope,
                    confidence,
                    observed_at,
                    now,
                    id,
                ],
            )
            .map_err(|e| e.to_string())?;
            (id, version, created_at)
        }
        Some((old_id, _, _, _, version, _)) => {
            tx.execute(
                "UPDATE conversation_context_facts SET invalidated_at=?1,
                 invalidation_reason='superseded',updated_at=?1 WHERE id=?2",
                params![now, old_id],
            )
            .map_err(|e| e.to_string())?;
            let id = uuid::Uuid::new_v4().to_string();
            insert_fact(
                &tx,
                &id,
                input,
                &value_json,
                confidence,
                version + 1,
                observed_at,
                now,
            )?;
            (id, version + 1, now)
        }
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            insert_fact(
                &tx,
                &id,
                input,
                &value_json,
                confidence,
                1,
                observed_at,
                now,
            )?;
            (id, 1, now)
        }
    };
    tx.commit().map_err(|e| e.to_string())?;
    Ok(ContextFactV2 {
        id,
        conversation_id: input.conversation_id.clone(),
        project_id: input.project_id.clone(),
        run_id: input.run_id.clone(),
        fact_kind: input.fact_kind.clone(),
        fact_key: input.fact_key.clone(),
        value: input.value.clone(),
        source: ContextSource {
            kind: input.source_kind.clone(),
            reference: input.source_ref.clone(),
            observed_at,
        },
        scope: input.scope.clone(),
        confidence,
        version,
        invalidated_at: None,
        invalidation_reason: None,
        updated_at: now.max(created_at),
    })
}

fn insert_fact(
    conn: &Connection,
    id: &str,
    input: &ContextFactInput,
    value_json: &str,
    confidence: f64,
    version: i64,
    observed_at: i64,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO conversation_context_facts
         (id,conversation_id,project_id,run_id,fact_kind,fact_key,value_json,source_kind,
          source_ref,scope,confidence,version,observed_at,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14)",
        params![
            id,
            input.conversation_id,
            input.project_id,
            input.run_id,
            input.fact_kind,
            input.fact_key,
            value_json,
            input.source_kind,
            input.source_ref,
            input.scope,
            confidence,
            version,
            observed_at,
            now,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn invalidate_facts(
    conn: &Connection,
    conversation_id: &str,
    scope: Option<&str>,
    reason: &str,
) -> Result<usize, String> {
    let now = now_ms();
    let changed = match scope {
        Some(scope) => conn.execute(
            "UPDATE conversation_context_facts SET invalidated_at=?1,invalidation_reason=?2,
             updated_at=?1 WHERE conversation_id=?3 AND scope=?4 AND invalidated_at IS NULL",
            params![now, reason, conversation_id, scope],
        ),
        None => conn.execute(
            "UPDATE conversation_context_facts SET invalidated_at=?1,invalidation_reason=?2,
             updated_at=?1 WHERE conversation_id=?3 AND invalidated_at IS NULL",
            params![now, reason, conversation_id],
        ),
    }
    .map_err(|e| e.to_string())?;
    if changed > 0 {
        conn.execute(
            "INSERT INTO conversation_context_state
             (conversation_id,schema_version,invalidation_epoch,created_at,updated_at)
             VALUES (?1,?2,1,?3,?3)
             ON CONFLICT(conversation_id) DO UPDATE SET
               invalidation_epoch=invalidation_epoch+1,updated_at=excluded.updated_at",
            params![conversation_id, CONTEXT_SCHEMA_VERSION, now],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(changed)
}

pub fn invalidate_project_facts(
    conn: &Connection,
    project_id: &str,
    reason: &str,
) -> Result<usize, String> {
    let mut stmt = conn
        .prepare("SELECT id FROM conversations WHERE project_id=?1")
        .map_err(|e| e.to_string())?;
    let ids = stmt
        .query_map([project_id], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(stmt);
    let mut changed = 0;
    for conversation_id in ids {
        changed += invalidate_facts(conn, &conversation_id, Some("project"), reason)?;
    }
    Ok(changed)
}

/// Invalidate durable project memories only when their declared condition
/// matches the observed event or one of its concrete references. Stable user
/// preferences and decisions with no condition are intentionally preserved.
pub fn invalidate_project_memories(
    conn: &Connection,
    project_id: &str,
    event: &str,
    references: &[String],
) -> Result<usize, String> {
    let event = event.trim().to_lowercase();
    let aliases: &[&str] = match event.as_str() {
        "project_changed" | "project_identity_changed" => &["project", "worktree", "项目", "工程"],
        "branch_changed" | "git_branch_changed" => &["branch", "git", "分支"],
        "file_changed" => &["file_changed", "file change", "文件变更", "文件变化"],
        "device_changed" => &["device", "hdc", "设备", "系统版本", "权限", "安装状态"],
        _ => &[],
    };
    let refs = references
        .iter()
        .flat_map(|reference| {
            let normalized = reference.trim().to_lowercase();
            let name = std::path::Path::new(&normalized)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_string();
            [normalized, name]
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let mut stmt = conn
        .prepare(
            "SELECT id,invalidation_condition FROM project_memories
             WHERE project_id=?1 AND enabled=1 AND invalidated_at IS NULL
               AND TRIM(invalidation_condition)!=''",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([project_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(stmt);
    let ids = rows
        .into_iter()
        .filter_map(|(id, condition)| {
            let condition = condition.to_lowercase();
            (condition.contains(&event)
                || aliases.iter().any(|alias| condition.contains(alias))
                || refs.iter().any(|reference| condition.contains(reference)))
            .then_some(id)
        })
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(0);
    }
    let now = now_ms();
    let reason = if references.is_empty() {
        event.clone()
    } else {
        format!("{}:{}", event, references.iter().take(5).cloned().collect::<Vec<_>>().join(","))
    };
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    for id in &ids {
        tx.execute(
            "UPDATE project_memories SET invalidated_at=?1,invalidation_reason=?2,updated_at=?1
             WHERE id=?3 AND invalidated_at IS NULL",
            params![now, reason, id],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.execute(
        "INSERT INTO conversation_context_state
         (conversation_id,schema_version,invalidation_epoch,created_at,updated_at)
         SELECT id,?1,1,?2,?2 FROM conversations WHERE project_id=?3
         ON CONFLICT(conversation_id) DO UPDATE SET
           invalidation_epoch=invalidation_epoch+1,updated_at=excluded.updated_at",
        params![CONTEXT_SCHEMA_VERSION, now, project_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(ids.len())
}

fn tool_file_references(args: &str) -> Vec<String> {
    fn collect(value: &serde_json::Value, key: Option<&str>, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (name, child) in map {
                    collect(child, Some(name), out);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    collect(item, key, out);
                }
            }
            serde_json::Value::String(text)
                if matches!(key, Some("path" | "file" | "target" | "dest" | "old_path" | "new_path")) =>
            {
                out.push(text.clone());
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Ok(value) = serde_json::from_str(args) {
        collect(&value, None, &mut out);
    }
    out.sort();
    out.dedup();
    out.truncate(20);
    out
}

fn invalidate_fact_kinds(
    conn: &Connection,
    conversation_id: &str,
    kinds: &[&str],
    reason: &str,
) -> Result<usize, String> {
    if kinds.is_empty() {
        return Ok(0);
    }
    let now = now_ms();
    let placeholders = std::iter::repeat_n("?", kinds.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "UPDATE conversation_context_facts SET invalidated_at=?1,invalidation_reason=?2,
         updated_at=?1 WHERE conversation_id=?3 AND invalidated_at IS NULL
         AND fact_kind IN ({placeholders})"
    );
    let mut values: Vec<rusqlite::types::Value> = vec![
        now.into(),
        reason.to_string().into(),
        conversation_id.to_string().into(),
    ];
    values.extend(kinds.iter().map(|kind| kind.to_string().into()));
    let changed = conn
        .execute(&sql, rusqlite::params_from_iter(values))
        .map_err(|e| e.to_string())?;
    if changed > 0 {
        conn.execute(
            "INSERT INTO conversation_context_state
             (conversation_id,schema_version,invalidation_epoch,created_at,updated_at)
             VALUES (?1,?2,1,?3,?3)
             ON CONFLICT(conversation_id) DO UPDATE SET
               invalidation_epoch=invalidation_epoch+1,updated_at=excluded.updated_at",
            params![conversation_id, CONTEXT_SCHEMA_VERSION, now],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(changed)
}

pub fn record_artifact(conn: &Connection, artifact: &ContextArtifactRef) -> Result<(), String> {
    conn.execute(
        "INSERT INTO conversation_context_artifacts
         (id,conversation_id,run_id,artifact_kind,uri,label,digest,metadata_json,source_ref,
          valid,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11)
         ON CONFLICT(conversation_id,artifact_kind,uri) DO UPDATE SET
           run_id=excluded.run_id,label=excluded.label,digest=excluded.digest,
           metadata_json=excluded.metadata_json,source_ref=excluded.source_ref,
           valid=excluded.valid,updated_at=excluded.updated_at",
        params![
            artifact.id,
            artifact.conversation_id,
            artifact.run_id,
            artifact.artifact_kind,
            artifact.uri,
            artifact.label,
            artifact.digest,
            artifact.metadata.to_string(),
            artifact.source_ref,
            artifact.valid as i64,
            artifact.updated_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 将工具协议 V2 中的产物和关键状态投影为可追溯上下文。
/// 原始输出仍保存在 tool_runs/messages；这里仅保存有界摘要、digest 和 URI。
pub fn record_tool_evidence(
    conn: &Connection,
    conversation_id: &str,
    run_id: &str,
    tool: &str,
    args: &str,
    output: &str,
    succeeded: bool,
) -> Result<(), String> {
    let status = if succeeded { "ok" } else { "error" };
    let envelope = crate::agent::structured_result::ToolResultEnvelope::from_execution(
        tool, args, output, status,
    );
    let digest = envelope.digest();
    let source_ref = format!("tool:{run_id}:{tool}:{}", &digest[..12]);
    let project_id = conn
        .query_row(
            "SELECT project_id FROM conversations WHERE id=?1",
            [conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let observed_at = now_ms();

    if succeeded {
        let invalidated_kinds: &[&str] = if matches!(
            tool,
            "write_file" | "edit_file" | "multi_edit" | "apply_patch" | "delete_file"
        ) {
            &["verification", "workspace"]
        } else if matches!(
            tool,
            "git_branch" | "git_pull" | "git_merge" | "git_restore"
        ) {
            &["workspace", "verification"]
        } else if matches!(
            tool,
            "deploy" | "deploy_all" | "uninstall_app" | "clear_app_data"
        ) {
            &["device"]
        } else {
            &[]
        };
        let _ = invalidate_fact_kinds(
            conn,
            conversation_id,
            invalidated_kinds,
            &format!("tool_mutation:{tool}"),
        );
        if let Some(project_id) = project_id.as_deref() {
            let (event, references) = if matches!(
                tool,
                "write_file" | "edit_file" | "multi_edit" | "apply_patch" | "delete_file"
            ) {
                (Some("file_changed"), tool_file_references(args))
            } else if matches!(tool, "git_branch" | "git_pull" | "git_merge" | "git_restore") {
                (Some("git_branch_changed"), Vec::new())
            } else if matches!(
                tool,
                "deploy" | "deploy_all" | "uninstall_app" | "clear_app_data" | "grant_permission"
            ) {
                (Some("device_changed"), Vec::new())
            } else {
                (None, Vec::new())
            };
            if let Some(event) = event {
                let _ = invalidate_project_memories(conn, project_id, event, &references);
            }
        }
    }

    for artifact in &envelope.artifacts {
        record_artifact(
            conn,
            &ContextArtifactRef {
                id: uuid::Uuid::new_v4().to_string(),
                conversation_id: conversation_id.to_string(),
                run_id: Some(run_id.to_string()),
                artifact_kind: artifact.kind.clone(),
                uri: artifact.path.clone(),
                label: format!("{}: {}", tool, artifact.operation),
                digest: Some(digest.clone()),
                metadata: serde_json::json!({
                    "operation": artifact.operation,
                    "tool_status": envelope.status,
                }),
                source_ref: source_ref.clone(),
                valid: true,
                updated_at: observed_at,
            },
        )?;
    }

    let fact_kind = if matches!(
        tool,
        "git_status" | "git_diff" | "git_log" | "git_branch" | "git_commit"
    ) {
        Some("workspace")
    } else if matches!(
        tool,
        "build_project"
            | "build_generic"
            | "build_hap"
            | "hvigor_build"
            | "run_tests"
            | "test_project"
            | "run_lint"
            | "check_signature"
    ) || !envelope.verification.is_empty()
    {
        Some("verification")
    } else if matches!(
        tool,
        "list_devices"
            | "get_app_info"
            | "deploy"
            | "deploy_all"
            | "start_ability"
            | "read_runtime_logs"
            | "search_hilog"
    ) {
        Some("device")
    } else {
        None
    };

    if let Some(fact_kind) = fact_kind {
        upsert_fact(
            conn,
            &ContextFactInput {
                conversation_id: conversation_id.to_string(),
                project_id,
                run_id: Some(run_id.to_string()),
                fact_kind: fact_kind.into(),
                fact_key: tool.to_string(),
                value: serde_json::json!({
                    "passed": succeeded,
                    "outcome": envelope.outcome,
                    "summary": envelope.summary,
                    "evidence_digest": digest,
                }),
                source_kind: "tool_run".into(),
                source_ref,
                scope: if fact_kind == "device" {
                    "environment".into()
                } else {
                    "project".into()
                },
                confidence: 1.0,
                observed_at: Some(observed_at),
            },
        )?;
    }
    Ok(())
}

pub fn persist_checkpoint(
    conn: &Connection,
    checkpoint: &ContextCheckpoint<'_>,
) -> Result<(), String> {
    let now = now_ms();
    let task_json = serde_json::to_string(checkpoint.task).map_err(|e| e.to_string())?;
    let budget_json = serde_json::to_string(checkpoint.budget).map_err(|e| e.to_string())?;
    let facts_digest = active_facts_digest(conn, checkpoint.conversation_id)?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO conversation_context_state
         (conversation_id,schema_version,summary,summary_from_message_rowid,
          summary_to_message_rowid,summary_event_seq,task_snapshot_json,budget_json,
          facts_digest,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?10)
         ON CONFLICT(conversation_id) DO UPDATE SET
           schema_version=excluded.schema_version,summary=excluded.summary,
           summary_from_message_rowid=excluded.summary_from_message_rowid,
           summary_to_message_rowid=excluded.summary_to_message_rowid,
           summary_event_seq=excluded.summary_event_seq,
           task_snapshot_json=excluded.task_snapshot_json,budget_json=excluded.budget_json,
           facts_digest=excluded.facts_digest,updated_at=excluded.updated_at",
        params![
            checkpoint.conversation_id,
            CONTEXT_SCHEMA_VERSION,
            checkpoint.summary,
            checkpoint.summary_from_message_rowid.max(0),
            checkpoint.summary_to_message_rowid.max(0),
            checkpoint.summary_event_seq.max(0),
            task_json,
            budget_json,
            facts_digest,
            now,
        ],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO conversation_context_snapshots
         (id,conversation_id,run_id,schema_version,message_rowid,event_seq,summary,
          task_snapshot_json,budget_json,facts_digest,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            uuid::Uuid::new_v4().to_string(),
            checkpoint.conversation_id,
            checkpoint.run_id,
            CONTEXT_SCHEMA_VERSION,
            checkpoint.summary_to_message_rowid.max(0),
            checkpoint.summary_event_seq.max(0),
            checkpoint.summary,
            task_json,
            budget_json,
            facts_digest,
            now,
        ],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM conversation_context_snapshots WHERE conversation_id=?1 AND id NOT IN
         (SELECT id FROM conversation_context_snapshots WHERE conversation_id=?1
          ORDER BY created_at DESC LIMIT 80)",
        [checkpoint.conversation_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

fn active_facts_digest(conn: &Connection, conversation_id: &str) -> Result<Option<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT fact_kind,fact_key,value_json,version FROM conversation_context_facts
             WHERE conversation_id=?1 AND invalidated_at IS NULL ORDER BY fact_kind,fact_key",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut count = 0;
    for row in rows {
        let (kind, key, value, version) = row.map_err(|e| e.to_string())?;
        hasher.update(kind);
        hasher.update([0]);
        hasher.update(key);
        hasher.update([0]);
        hasher.update(value);
        hasher.update(version.to_le_bytes());
        count += 1;
    }
    drop(stmt);
    let project_id = conn
        .query_row(
            "SELECT project_id FROM conversations WHERE id=?1",
            [conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(project_id) = project_id {
        let mut memories = conn
            .prepare(
                "SELECT id,category,title,content,version,updated_at FROM project_memories
                 WHERE project_id=?1 AND enabled=1 AND confirmed=1 AND invalidated_at IS NULL
                 ORDER BY pinned DESC,updated_at DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = memories
            .query_map([project_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (id, category, title, content, version, updated_at) = row.map_err(|e| e.to_string())?;
            for value in [id, category, title, content] {
                hasher.update(value);
                hasher.update([0]);
            }
            hasher.update(version.to_le_bytes());
            hasher.update(updated_at.to_le_bytes());
            count += 1;
        }
    }
    let mut pins = conn
        .prepare(
            "SELECT pin_kind,source_ref,label,content,updated_at FROM conversation_context_pins
             WHERE conversation_id=?1 ORDER BY pin_kind,source_ref",
        )
        .map_err(|e| e.to_string())?;
    let rows = pins
        .query_map([conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (kind, reference, label, content, updated_at) = row.map_err(|e| e.to_string())?;
        for value in [kind, reference, label, content] {
            hasher.update(value);
            hasher.update([0]);
        }
        hasher.update(updated_at.to_le_bytes());
        count += 1;
    }
    Ok((count > 0).then(|| format!("{:x}", hasher.finalize())))
}

/// Reconcile a model-written summary against durable facts and append a bounded,
/// machine-generated block. Natural language can provide navigation, but this block
/// is authoritative whenever the two disagree.
pub fn reconcile_summary(
    conn: &Connection,
    conversation_id: &str,
    summary: &str,
) -> Result<SummaryReconciliation, String> {
    const MARKER: &str = "【结构化事实对账（机器生成，以此为准）】";
    let base = summary
        .split(MARKER)
        .next()
        .unwrap_or(summary)
        .trim()
        .chars()
        .take(1_600)
        .collect::<String>();
    let task = capture_task_snapshot(conn, conversation_id)?;
    let facts = load_active_facts(conn, conversation_id, 40)?;
    let artifacts = load_valid_artifacts(conn, conversation_id, 20)?;
    let pending = load_hot_context(conn, conversation_id)?.pending_interactions;
    let pins = load_context_pins(conn, conversation_id, 30)?;

    let mut lines = Vec::new();
    if !task.goal.trim().is_empty() {
        lines.push(format!("任务状态：{} / {}；目标：{}", task.state, task.phase, task.goal));
    }
    if !task.constraints.is_empty() {
        lines.push(format!("不可丢失约束：{}", task.constraints.join("；")));
    }
    if !pending.is_empty() {
        lines.push(format!(
            "仍待用户确认：{}",
            pending
                .iter()
                .map(|item| format!("{}:{}", item.kind, item.request_id))
                .collect::<Vec<_>>()
                .join("；")
        ));
    }
    if !pins.is_empty() {
        lines.push(format!(
            "用户固定上下文：{}",
            pins.iter()
                .map(|pin| format!("{}:{}={}", pin.pin_kind, pin.label, pin.content))
                .collect::<Vec<_>>()
                .join("；")
        ));
    }
    for fact in facts.iter().take(20) {
        let value: String = fact.value.to_string().chars().take(300).collect();
        lines.push(format!(
            "事实 {}/{}={}（{}:{}，v{}）",
            fact.fact_kind,
            fact.fact_key,
            value,
            fact.source.kind,
            fact.source.reference,
            fact.version
        ));
    }
    if !artifacts.is_empty() {
        lines.push(format!(
            "有效产物：{}",
            artifacts
                .iter()
                .take(12)
                .map(|item| format!("{}:{}", item.artifact_kind, item.uri))
                .collect::<Vec<_>>()
                .join("；")
        ));
    }
    let authoritative_block = lines.join("\n").chars().take(2_400).collect::<String>();

    let lower = base.to_lowercase();
    let contains_any = |needles: &[&str]| needles.iter().any(|needle| lower.contains(needle));
    let mut conflicts = Vec::new();
    if !task.goal.trim().is_empty()
        && task.state != "completed"
        && contains_any(&["任务已完成", "全部完成", "fully completed", "task completed"])
    {
        conflicts.push("summary_claims_completed_but_run_is_non_terminal".into());
    }
    if !pending.is_empty()
        && contains_any(&["已批准", "无需审批", "无需确认", "approval completed", "no approval"])
    {
        conflicts.push("summary_claims_approval_resolved_but_interaction_is_pending".into());
    }
    if !task.constraints.is_empty()
        && !task.constraints.iter().any(|constraint| base.contains(constraint))
    {
        conflicts.push("summary_omits_durable_constraints".into());
    }
    for fact in &facts {
        if fact.value["passed"].as_bool() != Some(false) {
            continue;
        }
        let positive_claim = if fact.fact_key.contains("build") || fact.fact_key.contains("hvigor") {
            contains_any(&["构建成功", "编译通过", "build passed", "build succeeded"])
        } else if fact.fact_key.contains("test") || fact.fact_key.contains("lint") {
            contains_any(&["测试通过", "检查通过", "tests passed", "lint passed"])
        } else if fact.fact_kind == "device" {
            contains_any(&["部署成功", "安装成功", "设备验证通过", "deploy succeeded"])
        } else if fact.fact_kind == "workspace" {
            contains_any(&["工作区干净", "无未提交", "working tree clean"])
        } else {
            contains_any(&["全部验证通过", "验证均成功", "all checks passed"])
        };
        if positive_claim {
            conflicts.push(format!(
                "summary_positive_claim_conflicts_with_failed_fact:{}/{}",
                fact.fact_kind, fact.fact_key
            ));
        }
    }
    conflicts.sort();
    conflicts.dedup();
    let status = if conflicts.is_empty() { "consistent" } else { "corrected" };
    let summary = if authoritative_block.is_empty() {
        base
    } else {
        format!("{base}\n\n{MARKER}\n{authoritative_block}")
    };
    let summary_digest = format!("{:x}", Sha256::digest(summary.as_bytes()));
    let run_id = task.run_id.as_deref();
    conn.execute(
        "INSERT INTO conversation_context_reconciliations
         (id,conversation_id,run_id,summary_digest,facts_digest,status,conflicts_json,
          authoritative_block,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            uuid::Uuid::new_v4().to_string(),
            conversation_id,
            run_id,
            summary_digest,
            active_facts_digest(conn, conversation_id)?,
            status,
            serde_json::to_string(&conflicts).map_err(|e| e.to_string())?,
            authoritative_block,
            now_ms(),
        ],
    )
    .map_err(|e| e.to_string())?;
    if !conflicts.is_empty()
        && matches!(
            task.state.as_str(),
            "queued" | "running" | "delegated_running" | "waiting_approval" | "waiting_user" | "verifying"
        )
    {
        if let Some(run_id) = run_id {
            let _ = crate::agent::runtime::append_event(
                conn,
                run_id,
                conversation_id,
                "context.summary_reconciled",
                serde_json::json!({
                    "status": status,
                    "conflicts": conflicts,
                    "facts_digest": active_facts_digest(conn, conversation_id)?,
                }),
            );
        }
    }
    Ok(SummaryReconciliation {
        summary,
        status: status.into(),
        conflicts,
        authoritative_block,
    })
}

pub fn load_context_v2(
    conn: &Connection,
    conversation_id: &str,
    context_limit: i64,
) -> Result<ConversationContextV2, String> {
    let state = conn
        .query_row(
            "SELECT schema_version,summary,summary_from_message_rowid,summary_to_message_rowid,
             summary_event_seq,task_snapshot_json,budget_json,facts_digest,invalidation_epoch,updated_at
             FROM conversation_context_state WHERE conversation_id=?1",
            [conversation_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;

    let mut task = capture_task_snapshot(conn, conversation_id)?;
    let (
        schema_version,
        summary,
        from_rowid,
        to_rowid,
        event_seq,
        budget,
        facts_digest,
        epoch,
        updated_at,
    ) = if let Some((
        version,
        summary,
        from,
        to,
        seq,
        task_json,
        budget_json,
        digest,
        epoch,
        updated,
    )) = state
    {
        let persisted_task: TaskSnapshotV2 = json_or_default(&task_json);
        if task.run_id.is_none() && !persisted_task.goal.is_empty() {
            task = persisted_task;
        }
        (
            version,
            summary,
            from,
            to,
            seq,
            serde_json::from_str(&budget_json)
                .unwrap_or_else(|_| ContextBudgetV2::allocate(context_limit)),
            digest,
            epoch,
            updated,
        )
    } else {
        let legacy_summary = conn
            .query_row(
                "SELECT summary FROM conversations WHERE id=?1",
                [conversation_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .flatten();
        (
            CONTEXT_SCHEMA_VERSION,
            legacy_summary,
            0,
            0,
            0,
            ContextBudgetV2::allocate(context_limit),
            active_facts_digest(conn, conversation_id)?,
            0,
            task.updated_at,
        )
    };

    Ok(ConversationContextV2 {
        schema_version,
        conversation_id: conversation_id.to_string(),
        summary,
        summary_from_message_rowid: from_rowid,
        summary_to_message_rowid: to_rowid,
        summary_event_seq: event_seq,
        task,
        hot: load_hot_context(conn, conversation_id)?,
        facts: load_active_facts(conn, conversation_id, 100)?,
        artifacts: load_valid_artifacts(conn, conversation_id, 100)?,
        pins: load_context_pins(conn, conversation_id, 100)?,
        budget,
        facts_digest,
        invalidation_epoch: epoch,
        reconciliation: load_reconciliation_state(conn, conversation_id)?,
        updated_at,
    })
}

fn load_reconciliation_state(
    conn: &Connection,
    conversation_id: &str,
) -> Result<ContextReconciliationState, String> {
    let count = conn
        .query_row(
            "SELECT COUNT(*) FROM conversation_context_reconciliations WHERE conversation_id=?1",
            [conversation_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    let latest = conn
        .query_row(
            "SELECT status,conflicts_json,created_at FROM conversation_context_reconciliations
             WHERE conversation_id=?1 ORDER BY created_at DESC,rowid DESC LIMIT 1",
            [conversation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((status, conflicts, at)) = latest else {
        return Ok(ContextReconciliationState { count, ..Default::default() });
    };
    Ok(ContextReconciliationState {
        count,
        latest_status: Some(status),
        latest_conflicts: serde_json::from_str(&conflicts).unwrap_or_default(),
        latest_at: Some(at),
    })
}

fn load_hot_context(conn: &Connection, conversation_id: &str) -> Result<HotContextV2, String> {
    let mut message_stmt = conn
        .prepare(
            "SELECT id,role,content,created_at FROM messages
             WHERE conversation_id=?1 AND queued=0 AND hidden=0
             ORDER BY created_at DESC,rowid DESC LIMIT 12",
        )
        .map_err(|e| e.to_string())?;
    let mut recent_messages = message_stmt
        .query_map([conversation_id], |row| {
            let message_id = row.get::<_, String>(0)?;
            let content = row.get::<_, String>(2)?;
            Ok(HotMessageV2 {
                role: row.get(1)?,
                content: content.chars().take(1_200).collect(),
                source_ref: format!("message:{message_id}"),
                created_at: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    recent_messages.reverse();

    let mut current_errors = Vec::new();
    if let Some(error) = conn
        .query_row(
            "SELECT error FROM agent_runs WHERE conversation_id=?1 AND error IS NOT NULL
             ORDER BY started_at DESC LIMIT 1",
            [conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .filter(|value| !value.trim().is_empty())
    {
        current_errors.push(error.chars().take(1_000).collect());
    }
    let mut fact_stmt = conn
        .prepare(
            "SELECT value_json FROM conversation_context_facts
             WHERE conversation_id=?1 AND invalidated_at IS NULL
               AND json_extract(value_json,'$.passed')=0
             ORDER BY updated_at DESC LIMIT 4",
        )
        .map_err(|e| e.to_string())?;
    for raw in fact_stmt
        .query_map([conversation_id], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
    {
        let raw = raw.map_err(|e| e.to_string())?;
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
        if let Some(summary) = value["summary"].as_str().filter(|value| !value.trim().is_empty()) {
            current_errors.push(summary.chars().take(1_000).collect());
        }
    }
    current_errors.dedup();
    let mut interrupted_stmt = conn
        .prepare(
            "SELECT kind FROM pending_interactions
             WHERE conversation_id=?1 AND state='interrupted'
             ORDER BY resolved_at DESC LIMIT 3",
        )
        .map_err(|e| e.to_string())?;
    for kind in interrupted_stmt
        .query_map([conversation_id], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
    {
        current_errors.push(format!(
            "{} 等待因运行中断而关闭；继续前必须从安全检查点重新确认",
            kind.map_err(|e| e.to_string())?
        ));
    }

    let mut active_files = load_valid_artifacts(conn, conversation_id, 40)?
        .into_iter()
        .filter(|item| matches!(item.artifact_kind.as_str(), "file" | "config" | "source"))
        .map(|item| item.uri)
        .collect::<Vec<_>>();
    active_files.dedup();
    active_files.truncate(12);

    let mut interaction_stmt = conn
        .prepare(
            "SELECT request_id,kind,payload_json,expires_at FROM pending_interactions
             WHERE conversation_id=?1 AND state='pending'
             ORDER BY created_at DESC LIMIT 10",
        )
        .map_err(|e| e.to_string())?;
    let pending_interactions = interaction_stmt
        .query_map([conversation_id], |row| {
            let request_id = row.get::<_, String>(0)?;
            let raw = row.get::<_, String>(2)?;
            Ok(PendingInteractionV2 {
                source_ref: format!("interaction:{request_id}"),
                request_id,
                kind: row.get(1)?,
                payload: serde_json::from_str(&raw).unwrap_or_default(),
                expires_at: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(HotContextV2 {
        recent_messages,
        current_errors,
        active_files,
        pending_interactions,
    })
}

/// 读取 Context V2 摘要；状态尚未建立时由 `load_context_v2` 自动兼容旧摘要。
pub fn load_summary(
    conn: &Connection,
    conversation_id: &str,
    context_limit: i64,
) -> Option<String> {
    load_context_v2(conn, conversation_id, context_limit)
        .ok()
        .and_then(|context| context.summary)
        .filter(|summary| !summary.trim().is_empty())
}

/// 将当前 Run/步骤、摘要覆盖范围与预算保存为 Context V2 检查点。
///
/// `keep_recent` 与聊天历史裁剪口径一致：摘要覆盖游标指向被最近 N 条消息窗口排除的
/// 最后一条消息。查询失败或尚无摘要时游标保持 0，不会伪造覆盖范围。
pub fn persist_runtime_checkpoint(
    conn: &Connection,
    conversation_id: &str,
    run_id: Option<&str>,
    summary: Option<&str>,
    keep_recent: usize,
    context_limit: i64,
) -> Result<(), String> {
    let task = capture_task_snapshot(conn, conversation_id)?;
    let budget = ContextBudgetV2::allocate(context_limit);
    let (summary_from_message_rowid, summary_to_message_rowid) =
        if summary.is_some_and(|value| !value.trim().is_empty()) {
            conn.query_row(
                "SELECT COALESCE(MIN(rowid),0),COALESCE(MAX(rowid),0) FROM messages
             WHERE conversation_id=?1 AND role IN ('user','assistant','tool') AND queued=0
               AND hidden=0 AND rowid NOT IN (
                 SELECT rowid FROM messages WHERE conversation_id=?1
                   AND role IN ('user','assistant','tool') AND queued=0 AND hidden=0
                 ORDER BY created_at DESC,rowid DESC LIMIT ?2
               )",
                params![conversation_id, keep_recent.max(1) as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((0, 0))
        } else {
            (0, 0)
        };
    let summary_event_seq = conn
        .query_row(
            "SELECT COALESCE(MAX(seq),0) FROM session_events WHERE conversation_id=?1",
            [conversation_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    persist_checkpoint(
        conn,
        &ContextCheckpoint {
            conversation_id,
            run_id,
            summary,
            summary_from_message_rowid,
            summary_to_message_rowid,
            summary_event_seq,
            task: &task,
            budget: &budget,
        },
    )
}

fn load_active_facts(
    conn: &Connection,
    conversation_id: &str,
    limit: usize,
) -> Result<Vec<ContextFactV2>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id,project_id,run_id,fact_kind,fact_key,value_json,source_kind,source_ref,
             scope,confidence,version,observed_at,invalidated_at,invalidation_reason,updated_at
             FROM conversation_context_facts WHERE conversation_id=?1 AND invalidated_at IS NULL
             ORDER BY updated_at DESC LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(
            params![conversation_id, limit.clamp(1, 500) as i64],
            |row| {
                let raw: String = row.get(5)?;
                Ok(ContextFactV2 {
                    id: row.get(0)?,
                    conversation_id: conversation_id.to_string(),
                    project_id: row.get(1)?,
                    run_id: row.get(2)?,
                    fact_kind: row.get(3)?,
                    fact_key: row.get(4)?,
                    value: serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw)),
                    source: ContextSource {
                        kind: row.get(6)?,
                        reference: row.get(7)?,
                        observed_at: row.get(11)?,
                    },
                    scope: row.get(8)?,
                    confidence: row.get(9)?,
                    version: row.get(10)?,
                    invalidated_at: row.get(12)?,
                    invalidation_reason: row.get(13)?,
                    updated_at: row.get(14)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    let mut facts = rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    drop(stmt);

    // Project memories are the durable project layer of Context V2. They are
    // queried at read time so edits, disabling and invalidation are visible in
    // every conversation without copying stale rows into each conversation.
    let project_id = conn
        .query_row(
            "SELECT project_id FROM conversations WHERE id=?1",
            [conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(project_id) = project_id {
        let remaining = limit.saturating_sub(facts.len());
        if remaining > 0 {
            let mut memories = conn
                .prepare(
                    "SELECT id,category,title,content,source_kind,source_ref,scope,confidence,
                     version,updated_at,pinned,invalidation_condition
                     FROM project_memories
                     WHERE project_id=?1 AND enabled=1 AND confirmed=1 AND invalidated_at IS NULL
                     ORDER BY pinned DESC,updated_at DESC LIMIT ?2",
                )
                .map_err(|e| e.to_string())?;
            let rows = memories
                .query_map(params![project_id, remaining as i64], |row| {
                    let id: String = row.get(0)?;
                    let category: String = row.get(1)?;
                    let title: String = row.get(2)?;
                    let source_ref: String = row.get(5)?;
                    let updated_at: i64 = row.get(9)?;
                    Ok(ContextFactV2 {
                        id: format!("memory:{id}"),
                        conversation_id: conversation_id.to_string(),
                        project_id: Some(project_id.clone()),
                        run_id: None,
                        fact_kind: "project_memory".into(),
                        fact_key: format!("{category}:{id}"),
                        value: serde_json::json!({
                            "category": category,
                            "title": title,
                            "content": row.get::<_, String>(3)?,
                            "pinned": row.get::<_, i64>(10)? != 0,
                            "invalidation_condition": row.get::<_, String>(11)?,
                        }),
                        source: ContextSource {
                            kind: row.get(4)?,
                            reference: if source_ref.is_empty() { format!("memory:{id}") } else { source_ref },
                            observed_at: updated_at,
                        },
                        scope: row.get(6)?,
                        confidence: row.get(7)?,
                        version: row.get(8)?,
                        invalidated_at: None,
                        invalidation_reason: None,
                        updated_at,
                    })
                })
                .map_err(|e| e.to_string())?;
            facts.extend(rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?);
        }
    }
    Ok(facts)
}

fn load_valid_artifacts(
    conn: &Connection,
    conversation_id: &str,
    limit: usize,
) -> Result<Vec<ContextArtifactRef>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id,run_id,artifact_kind,uri,label,digest,metadata_json,source_ref,valid,updated_at
             FROM conversation_context_artifacts WHERE conversation_id=?1 AND valid=1
             ORDER BY updated_at DESC LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(
            params![conversation_id, limit.clamp(1, 500) as i64],
            |row| {
                let raw: String = row.get(6)?;
                Ok(ContextArtifactRef {
                    id: row.get(0)?,
                    conversation_id: conversation_id.to_string(),
                    run_id: row.get(1)?,
                    artifact_kind: row.get(2)?,
                    uri: row.get(3)?,
                    label: row.get(4)?,
                    digest: row.get(5)?,
                    metadata: serde_json::from_str(&raw).unwrap_or_default(),
                    source_ref: row.get(7)?,
                    valid: row.get::<_, i64>(8)? != 0,
                    updated_at: row.get(9)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

pub fn render_context_hint(context: &ConversationContextV2) -> Option<String> {
    let has_task = !context.task.goal.trim().is_empty();
    if !has_task && context.facts.is_empty() && context.artifacts.is_empty() && context.pins.is_empty() {
        return None;
    }
    let mut out = String::from("## 长会话结构化上下文 v2（事实均可追溯）\n");
    if has_task {
        out.push_str(&format!(
            "- 当前目标：{}\n- Run 状态：{} / {}\n",
            context.task.goal, context.task.state, context.task.phase
        ));
        if !context.task.required_conditions.is_empty() {
            out.push_str(&format!(
                "- 必需验收：{}\n",
                context.task.required_conditions.join("；")
            ));
        }
        if !context.task.constraints.is_empty() {
            out.push_str(&format!("- 用户/执行约束：{}\n", context.task.constraints.join("；")));
        }
        if !context.task.open_steps.is_empty() {
            out.push_str(&format!(
                "- 待完成：{}\n",
                context.task.open_steps.join("；")
            ));
        }
        if !context.task.blocked_steps.is_empty() {
            out.push_str(&format!(
                "- 阻塞：{}\n",
                context.task.blocked_steps.join("；")
            ));
        }
        if let Some(next) = &context.task.next_action {
            out.push_str(&format!("- 下一步：{next}\n"));
        }
    }
    if !context.hot.pending_interactions.is_empty() {
        out.push_str("- 待用户确认（不得被摘要覆盖或自动批准）：\n");
        for interaction in &context.hot.pending_interactions {
            out.push_str(&format!(
                "  - {} {}（来源 {}）\n",
                interaction.kind, interaction.payload, interaction.source_ref
            ));
        }
    }
    if !context.pins.is_empty() {
        out.push_str("- 用户固定上下文（不得被压缩、摘要或推断覆盖）：\n");
        for pin in context.pins.iter().take(30) {
            out.push_str(&format!(
                "  - [{}] {}={}（来源 {}）\n",
                pin.pin_kind, pin.label, pin.content, pin.source_ref
            ));
        }
    }
    if !context.hot.current_errors.is_empty() {
        out.push_str(&format!("- 当前错误：{}\n", context.hot.current_errors.join("；")));
    }
    if !context.hot.active_files.is_empty() {
        out.push_str(&format!("- 活跃文件：{}\n", context.hot.active_files.join("；")));
    }
    if !context.facts.is_empty() {
        out.push_str("- 活跃事实：\n");
        for fact in context.facts.iter().take(30) {
            out.push_str(&format!(
                "  - {}/{}={}（来源 {}:{}，v{}）\n",
                fact.fact_kind,
                fact.fact_key,
                fact.value,
                fact.source.kind,
                fact.source.reference,
                fact.version
            ));
        }
    }
    if !context.artifacts.is_empty() {
        out.push_str("- 关键产物：\n");
        for artifact in context.artifacts.iter().take(20) {
            out.push_str(&format!(
                "  - [{}] {}（来源 {}）\n",
                artifact.artifact_kind, artifact.uri, artifact.source_ref
            ));
        }
    }
    let max_chars = (context.budget.task_tokens + context.budget.project_tokens)
        .saturating_mul(2)
        .clamp(2_000, 24_000) as usize;
    Some(out.chars().take(max_chars).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_conn(conn: &Connection) {
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE projects(id TEXT PRIMARY KEY);
             CREATE TABLE conversations(
               id TEXT PRIMARY KEY, project_id TEXT NOT NULL, ledger TEXT, summary TEXT,
               updated_at INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE agent_runs(
               run_id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL,goal TEXT NOT NULL,
               state TEXT NOT NULL,phase TEXT NOT NULL,goal_contract_json TEXT,error TEXT,
               started_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,last_event_seq INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE run_events(
               event_id TEXT PRIMARY KEY,run_id TEXT NOT NULL,conversation_id TEXT NOT NULL,
               seq INTEGER NOT NULL,event_type TEXT NOT NULL,payload TEXT NOT NULL,created_at INTEGER NOT NULL
             );
             CREATE TABLE execution_steps(
               step_id TEXT PRIMARY KEY,run_id TEXT NOT NULL,conversation_id TEXT NOT NULL,
               ordinal INTEGER NOT NULL,title TEXT NOT NULL,state TEXT NOT NULL,
               result_summary TEXT,updated_at INTEGER NOT NULL
             );
             CREATE TABLE messages(
               id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL,role TEXT NOT NULL,
               content TEXT NOT NULL,queued INTEGER NOT NULL DEFAULT 0,
               hidden INTEGER NOT NULL DEFAULT 0,created_at INTEGER NOT NULL
             );
             CREATE TABLE session_events(
               id INTEGER PRIMARY KEY AUTOINCREMENT,conversation_id TEXT NOT NULL,
               seq INTEGER NOT NULL
             );",
        )
        .unwrap();
        conn.execute("INSERT INTO projects(id) VALUES ('p')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO conversations(id,project_id,updated_at) VALUES ('c','p',1)",
            [],
        )
        .unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/008_project_memories.sql"
        ))
        .unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/063_conversation_context_v2.sql"
        ))
        .unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/064_pending_interactions.sql"
        ))
        .unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/065_context_reconciliation.sql"
        ))
        .unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/066_structured_project_memories.sql"
        ))
        .unwrap();
        conn.execute_batch(include_str!("../../migrations/067_context_pins.sql"))
            .unwrap();
    }

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_conn(&conn);
        conn
    }

    fn fact(value: serde_json::Value) -> ContextFactInput {
        ContextFactInput {
            conversation_id: "c".into(),
            project_id: Some("p".into()),
            run_id: None,
            fact_kind: "workspace".into(),
            fact_key: "git_head".into(),
            value,
            source_kind: "git".into(),
            source_ref: "git:HEAD".into(),
            scope: "project".into(),
            confidence: 1.0,
            observed_at: Some(10),
        }
    }

    fn insert_project_memory(
        conn: &Connection,
        id: &str,
        confirmed: bool,
        pinned: bool,
        invalidated_at: Option<i64>,
        invalidation_condition: &str,
    ) {
        conn.execute(
            "INSERT INTO project_memories
             (id,project_id,category,title,content,enabled,source_kind,source_ref,scope,
              confidence,version,confirmed,pinned,invalidation_condition,invalidated_at,
              created_at,updated_at)
             VALUES (?1,'p','architecture','模块边界','UI 不直接访问 SQLite',1,'user',?2,
                     'project',1.0,2,?3,?4,?5,?6,1,2)",
            params![
                id,
                format!("memory:{id}"),
                confirmed as i64,
                pinned as i64,
                invalidation_condition,
                invalidated_at
            ],
        )
        .unwrap();
    }

    #[test]
    fn budget_allocation_never_exceeds_input_window() {
        for total in [1_000, 8_192, 200_000] {
            let budget = ContextBudgetV2::allocate(total);
            assert_eq!(
                budget.system_tokens
                    + budget.task_tokens
                    + budget.project_tokens
                    + budget.archive_tokens
                    + budget.hot_tokens,
                budget.input_tokens()
            );
            assert!(budget.hot_tokens > budget.task_tokens);
        }
    }

    #[test]
    fn fact_versions_replace_without_losing_history() {
        let conn = conn();
        let first = upsert_fact(&conn, &fact(serde_json::json!("a"))).unwrap();
        let same = upsert_fact(&conn, &fact(serde_json::json!("a"))).unwrap();
        assert_eq!(first.id, same.id);
        assert_eq!(same.version, 1);

        let second = upsert_fact(&conn, &fact(serde_json::json!("b"))).unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(second.version, 2);
        let (active, invalid): (i64, i64) = conn
            .query_row(
                "SELECT SUM(invalidated_at IS NULL),SUM(invalidated_at IS NOT NULL)
                 FROM conversation_context_facts",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((active, invalid), (1, 1));
    }

    #[test]
    fn confirmed_project_memory_is_shared_but_unconfirmed_or_stale_memory_is_hidden() {
        let conn = conn();
        insert_project_memory(&conn, "shared", true, true, None, "module graph changes");
        insert_project_memory(&conn, "draft", false, false, None, "");
        insert_project_memory(&conn, "stale", true, false, Some(3), "");
        conn.execute(
            "INSERT INTO conversations(id,project_id,updated_at) VALUES ('other','p',1)",
            [],
        )
        .unwrap();

        let first = load_context_v2(&conn, "c", 20_000).unwrap();
        let second = load_context_v2(&conn, "other", 20_000).unwrap();
        for context in [&first, &second] {
            let memories = context
                .facts
                .iter()
                .filter(|fact| fact.fact_kind == "project_memory")
                .collect::<Vec<_>>();
            assert_eq!(memories.len(), 1);
            assert_eq!(memories[0].id, "memory:shared");
            assert_eq!(memories[0].version, 2);
            assert_eq!(memories[0].value["pinned"], true);
            assert_eq!(memories[0].source.reference, "memory:shared");
        }
        assert_eq!(first.facts_digest, second.facts_digest);
        assert!(first.facts_digest.is_some());
    }

    #[test]
    fn declared_memory_conditions_invalidate_only_matching_project_knowledge() {
        let conn = conn();
        conn.execute(
            "INSERT INTO conversations(id,project_id,updated_at) VALUES ('other','p',1)",
            [],
        )
        .unwrap();
        insert_project_memory(&conn, "branch", true, false, None, "切换 Git 分支");
        insert_project_memory(
            &conn,
            "file",
            true,
            false,
            None,
            "build-profile.json5 修改时失效",
        );
        insert_project_memory(&conn, "device", true, false, None, "设备系统版本变化");
        insert_project_memory(&conn, "stable", true, true, None, "");

        assert_eq!(
            invalidate_project_memories(&conn, "p", "git_branch_changed", &[]).unwrap(),
            1
        );
        assert_eq!(
            invalidate_project_memories(
                &conn,
                "p",
                "file_changed",
                &["entry/build-profile.json5".into()],
            )
            .unwrap(),
            1
        );
        assert_eq!(
            invalidate_project_memories(&conn, "p", "device_changed", &[]).unwrap(),
            1
        );
        let active = load_active_facts(&conn, "c", 20).unwrap();
        let memory_ids = active
            .iter()
            .filter(|fact| fact.fact_kind == "project_memory")
            .map(|fact| fact.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(memory_ids, vec!["memory:stable"]);
        let epochs = conn
            .prepare(
                "SELECT invalidation_epoch FROM conversation_context_state
                 WHERE conversation_id IN ('c','other') ORDER BY conversation_id",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(epochs, vec![3, 3]);
    }

    #[test]
    fn user_pins_are_durable_authoritative_and_message_content_is_db_sourced() {
        let conn = conn();
        conn.execute(
            "INSERT INTO messages(id,conversation_id,role,content,created_at)
             VALUES ('m1','c','user','不要推送',1)",
            [],
        )
        .unwrap();
        let message = set_context_pin(
            &conn,
            "c",
            "message",
            "message:m1",
            "关键约束",
            "伪造内容",
            true,
        )
        .unwrap()
        .unwrap();
        assert_eq!(message.content, "不要推送");
        for (kind, reference, label, content) in [
            ("decision", "decision:local-only", "发布方式", "只创建本地提交"),
            ("file", "file:docs/ROADMAP.md", "路线图", "docs/ROADMAP.md"),
            ("acceptance", "acceptance:tests", "验收", "Rust 与前端测试通过"),
        ] {
            set_context_pin(&conn, "c", kind, reference, label, content, true)
                .unwrap()
                .unwrap();
        }
        let context = load_context_v2(&conn, "c", 20_000).unwrap();
        assert_eq!(context.pins.len(), 4);
        let hint = render_context_hint(&context).unwrap();
        assert!(hint.contains("不得被压缩、摘要或推断覆盖"));
        assert!(hint.contains("不要推送"));
        let reconciled = reconcile_summary(&conn, "c", "继续工作").unwrap();
        assert!(reconciled.authoritative_block.contains("用户固定上下文"));

        assert!(set_context_pin(
            &conn,
            "c",
            "message",
            "message:m1",
            "",
            "",
            false,
        )
        .unwrap()
        .is_none());
        assert_eq!(load_context_pins(&conn, "c", 20).unwrap().len(), 3);
    }

    #[test]
    fn checkpoint_round_trips_summary_task_and_sources() {
        let conn = conn();
        upsert_fact(&conn, &fact(serde_json::json!("9380478"))).unwrap();
        let task = TaskSnapshotV2 {
            goal: "推进长会话".into(),
            state: "running".into(),
            phase: "context".into(),
            next_action: Some("接入聊天循环".into()),
            updated_at: 20,
            ..TaskSnapshotV2::default()
        };
        let budget = ContextBudgetV2::allocate(32_000);
        persist_checkpoint(
            &conn,
            &ContextCheckpoint {
                conversation_id: "c",
                run_id: None,
                summary: Some("已完成数据映射"),
                summary_from_message_rowid: 1,
                summary_to_message_rowid: 40,
                summary_event_seq: 50,
                task: &task,
                budget: &budget,
            },
        )
        .unwrap();

        let loaded = load_context_v2(&conn, "c", 32_000).unwrap();
        assert_eq!(loaded.summary.as_deref(), Some("已完成数据映射"));
        assert_eq!(loaded.summary_to_message_rowid, 40);
        assert_eq!(loaded.task.goal, "推进长会话");
        assert_eq!(loaded.facts.len(), 1);
        assert!(loaded.facts_digest.is_some());
        assert!(render_context_hint(&loaded).unwrap().contains("git:HEAD"));
    }

    #[test]
    fn invalidation_increments_epoch_and_hides_fact() {
        let conn = conn();
        upsert_fact(&conn, &fact(serde_json::json!("main"))).unwrap();
        assert_eq!(
            invalidate_facts(&conn, "c", Some("project"), "branch_changed").unwrap(),
            1
        );
        let loaded = load_context_v2(&conn, "c", 8_192).unwrap();
        assert!(loaded.facts.is_empty());
        assert_eq!(loaded.invalidation_epoch, 1);
    }

    #[test]
    fn runtime_checkpoint_tracks_only_summarized_message_range() {
        let conn = conn();
        for index in 1..=6 {
            conn.execute(
                "INSERT INTO messages(id,conversation_id,role,content,created_at)
                 VALUES (?1,'c','user',?2,?3)",
                params![format!("m{index}"), format!("message-{index}"), index],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO session_events(conversation_id,seq) VALUES ('c',9)",
            [],
        )
        .unwrap();
        persist_runtime_checkpoint(&conn, "c", None, Some("前四条摘要"), 2, 16_000).unwrap();
        let loaded = load_context_v2(&conn, "c", 16_000).unwrap();
        assert_eq!(loaded.summary_from_message_rowid, 1);
        assert_eq!(loaded.summary_to_message_rowid, 4);
        assert_eq!(loaded.summary_event_seq, 9);
    }

    #[test]
    fn hot_context_keeps_recent_messages_files_errors_and_pending_waits() {
        let conn = conn();
        conn.execute(
            "INSERT INTO messages(id,conversation_id,role,content,created_at)
             VALUES ('m-hot','c','user','保持审批约束',10)",
            [],
        )
        .unwrap();
        record_tool_evidence(
            &conn,
            "c",
            "run-hot",
            "write_file",
            r#"{"path":"src/main.ets","content":"x"}"#,
            "written",
            true,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO pending_interactions
             (request_id,conversation_id,kind,state,payload_json,created_at,updated_at)
             VALUES ('ask-hot','c','ask_user','pending','{\"question\":\"继续吗\"}',11,11)",
            [],
        )
        .unwrap();
        let loaded = load_context_v2(&conn, "c", 16_000).unwrap();
        assert_eq!(loaded.hot.recent_messages.len(), 1);
        assert!(loaded.hot.active_files.iter().any(|path| path == "src/main.ets"));
        assert_eq!(loaded.hot.pending_interactions[0].request_id, "ask-hot");
        assert!(render_context_hint(&loaded).unwrap().contains("不得被摘要覆盖"));
    }

    #[test]
    fn summary_reconciliation_corrects_claims_that_conflict_with_failed_facts() {
        let conn = conn();
        conn.execute(
            "INSERT INTO agent_runs
             (run_id,conversation_id,goal,state,phase,started_at,updated_at)
             VALUES ('run-failed','c','修复构建','running','verifying',1,1)",
            [],
        )
        .unwrap();
        record_tool_evidence(
            &conn,
            "c",
            "run-failed",
            "build_project",
            "{}",
            "ArkTS compile failed",
            false,
        )
        .unwrap();
        let result = reconcile_summary(&conn, "c", "构建成功，所有工作已经完成。").unwrap();
        assert_eq!(result.status, "corrected");
        assert!(result.summary.contains("结构化事实对账"));
        assert!(result.summary.contains("\"passed\":false"));
        assert!(result.conflicts.iter().any(|item| item.contains("failed_fact")));
        let stored: (String, String) = conn
            .query_row(
                "SELECT status,conflicts_json FROM conversation_context_reconciliations
                 WHERE conversation_id='c' ORDER BY created_at DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(stored.0, "corrected");
        assert!(stored.1.contains("build_project"));
        let loaded = load_context_v2(&conn, "c", 16_000).unwrap();
        assert_eq!(loaded.reconciliation.count, 1);
        assert_eq!(loaded.reconciliation.latest_status.as_deref(), Some("corrected"));
        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM run_events WHERE run_id='run-failed'
                 AND event_type='context.summary_reconciled'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(events, 1);
    }

    #[test]
    fn tool_evidence_records_artifacts_and_supersedes_verification() {
        let conn = conn();
        record_tool_evidence(
            &conn,
            "c",
            "run-1",
            "build_project",
            r#"{"path":"entry/build/default/outputs/default/entry.hap"}"#,
            "构建成功",
            true,
        )
        .unwrap();
        let first = load_context_v2(&conn, "c", 16_000).unwrap();
        assert_eq!(first.artifacts.len(), 1);
        assert_eq!(first.facts.len(), 1);
        assert_eq!(first.facts[0].fact_kind, "verification");
        assert_eq!(first.facts[0].value["passed"], true);

        record_tool_evidence(
            &conn,
            "c",
            "run-2",
            "build_project",
            r#"{"path":"entry/build/default/outputs/default/entry.hap"}"#,
            "执行失败: ArkTS 编译错误",
            false,
        )
        .unwrap();
        let second = load_context_v2(&conn, "c", 16_000).unwrap();
        assert_eq!(second.facts.len(), 1);
        assert_eq!(second.facts[0].version, 2);
        assert_eq!(second.facts[0].value["passed"], false);

        record_tool_evidence(
            &conn,
            "c",
            "run-3",
            "edit_file",
            r#"{"path":"entry/src/main/ets/pages/Index.ets"}"#,
            "已修改",
            true,
        )
        .unwrap();
        let after_edit = load_context_v2(&conn, "c", 16_000).unwrap();
        assert!(after_edit.facts.is_empty());
        assert_eq!(after_edit.invalidation_epoch, 1);
    }

    #[test]
    fn project_invalidation_updates_every_conversation() {
        let conn = conn();
        conn.execute(
            "INSERT INTO conversations(id,project_id,updated_at) VALUES ('c2','p',1)",
            [],
        )
        .unwrap();
        upsert_fact(&conn, &fact(serde_json::json!("main"))).unwrap();
        let mut other = fact(serde_json::json!("main"));
        other.conversation_id = "c2".into();
        upsert_fact(&conn, &other).unwrap();
        assert_eq!(
            invalidate_project_facts(&conn, "p", "git_branch_changed").unwrap(),
            2
        );
        assert!(load_context_v2(&conn, "c", 8_192).unwrap().facts.is_empty());
        assert!(load_context_v2(&conn, "c2", 8_192)
            .unwrap()
            .facts
            .is_empty());
    }

    #[test]
    fn long_session_checkpoint_survives_reopen_and_fact_reconciliation() {
        let nonce = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "harmony-context-v2-{}-{nonce}.sqlite",
            std::process::id()
        ));
        {
            let conn = Connection::open(&path).unwrap();
            init_conn(&conn);
            for index in 1..=120 {
                conn.execute(
                    "INSERT INTO messages(id,conversation_id,role,content,created_at)
                     VALUES (?1,'c',?2,?3,?4)",
                    params![
                        format!("m{index}"),
                        if index % 2 == 0 { "assistant" } else { "user" },
                        format!("long-session-message-{index}"),
                        index,
                    ],
                )
                .unwrap();
            }
            let contract = crate::agent::acceptance::GoalContract::compile(
                "修复 src/main.ets，运行测试，但暂不提交",
            );
            conn.execute(
                "INSERT INTO agent_runs(run_id,conversation_id,goal,state,phase,goal_contract_json,error,started_at,updated_at)
                 VALUES ('run-long','c',?1,'interrupted','recovery_required',?2,'等待修复失败测试',1,200)",
                params![contract.original_goal, serde_json::to_string(&contract).unwrap()],
            ).unwrap();
            conn.execute_batch(
                "INSERT INTO execution_steps(step_id,run_id,conversation_id,ordinal,title,state,result_summary,updated_at)
                   VALUES ('done','run-long','c',1,'读取目标文件','completed','已读取',100),
                          ('open','run-long','c',2,'修复实现','pending',NULL,101),
                          ('blocked','run-long','c',3,'运行测试','blocked','测试失败',102);",
            ).unwrap();
            upsert_fact(&conn, &fact(serde_json::json!("head-a"))).unwrap();
            let mut verification = fact(serde_json::json!({"passed": false, "summary": "tests failed"}));
            verification.fact_kind = "verification".into();
            verification.fact_key = "run_tests".into();
            verification.source_ref = "tool:test-1".into();
            upsert_fact(&conn, &verification).unwrap();
            let mut dirty = fact(serde_json::json!({"clean": false, "files": ["src/main.ets"]}));
            dirty.fact_key = "git_status".into();
            dirty.source_ref = "tool:git-status-1".into();
            upsert_fact(&conn, &dirty).unwrap();
            set_context_pin(
                &conn,
                "c",
                "decision",
                "user:no-commit",
                "用户约束",
                "暂不提交",
                true,
            ).unwrap();
            persist_runtime_checkpoint(
                &conn,
                "c",
                None,
                Some("前 100 条消息的增量摘要"),
                20,
                64_000,
            )
            .unwrap();
        }
        {
            let conn = Connection::open(&path).unwrap();
            let restored = load_context_v2(&conn, "c", 64_000).unwrap();
            assert_eq!(restored.summary_to_message_rowid, 100);
            assert_eq!(restored.summary.as_deref(), Some("前 100 条消息的增量摘要"));
            assert_eq!(restored.task.goal, "修复 src/main.ets，运行测试，但暂不提交");
            assert_eq!(restored.task.completed_steps, vec!["读取目标文件: 已读取"]);
            assert_eq!(restored.task.open_steps, vec!["修复实现"]);
            assert_eq!(restored.task.blocked_steps, vec!["运行测试: 测试失败"]);
            assert!(restored.task.constraints.iter().any(|item| item.contains("完成声明")));
            assert!(restored.pins.iter().any(|item| item.content == "暂不提交"));
            assert!(restored.facts.iter().any(|item| item.fact_key == "run_tests" && item.value["passed"] == false));
            assert!(restored.facts.iter().any(|item| item.fact_key == "git_status" && item.value["clean"] == false));

            let changed = upsert_fact(&conn, &fact(serde_json::json!("head-b"))).unwrap();
            assert_eq!(changed.version, 2);
            let reconciled = load_context_v2(&conn, "c", 64_000).unwrap();
            assert!(reconciled.facts.iter().any(|item| item.fact_key == "git_head" && item.value == serde_json::json!("head-b")));
        }
        std::fs::remove_file(path).ok();
    }
}
