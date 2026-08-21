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
pub struct TaskSnapshotV2 {
    pub run_id: Option<String>,
    pub goal: String,
    pub state: String,
    pub phase: String,
    pub required_conditions: Vec<String>,
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
    pub facts: Vec<ContextFactV2>,
    pub artifacts: Vec<ContextArtifactRef>,
    pub budget: ContextBudgetV2,
    pub facts_digest: Option<String>,
    pub invalidation_epoch: i64,
    pub updated_at: i64,
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

    let required_conditions = contract_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<crate::agent::acceptance::GoalContract>(raw).ok())
        .map(|contract| {
            contract
                .criteria
                .into_iter()
                .filter(|criterion| criterion.required)
                .map(|criterion| criterion.label)
                .collect()
        })
        .unwrap_or_default();

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
    Ok((count > 0).then(|| format!("{:x}", hasher.finalize())))
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
        facts: load_active_facts(conn, conversation_id, 100)?,
        artifacts: load_valid_artifacts(conn, conversation_id, 100)?,
        budget,
        facts_digest,
        invalidation_epoch: epoch,
        updated_at,
    })
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
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
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
    if !has_task && context.facts.is_empty() && context.artifacts.is_empty() {
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

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
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
               started_at INTEGER NOT NULL,updated_at INTEGER NOT NULL
             );
             CREATE TABLE execution_steps(
               step_id TEXT PRIMARY KEY,run_id TEXT NOT NULL,conversation_id TEXT NOT NULL,
               ordinal INTEGER NOT NULL,title TEXT NOT NULL,state TEXT NOT NULL,
               result_summary TEXT,updated_at INTEGER NOT NULL
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
            "../../migrations/063_conversation_context_v2.sql"
        ))
        .unwrap();
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
}
