//! 会话内定时提醒（对齐 deepseek-harness schedule 子系统）：
//! - 三类规则：after（延时一次性）/ at（绝对时点一次性）/ every（固定间隔 ≥300s 重复）
//! - 到期以普通对话消息注入原会话队列（session-local：会话不活跃不投递，无外部通知渠道），
//!   不中断当前轮次——注入后模型下一轮请求自动看到（与后台任务完成同通道）
//! - delete 终结性；一次性提醒 dispatch 后终结；every 提醒推进到下一个锚点对齐目标
//! - 错误码对齐 dsh：invalid_prompt / invalid_selector / not_future / frequency_too_high

use rusqlite::Connection;
use std::time::{SystemTime, UNIX_EPOCH};

/// 当前 unix 秒
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 提醒记录（列表视图）
#[derive(Clone, serde::Serialize)]
pub struct ReminderInfo {
    pub id: String,
    pub kind: String,
    pub prompt: String,
    /// 目标时点（unix 秒）；every 为下一个未投递的锚点对齐目标
    pub scheduled_at: i64,
    pub every_seconds: Option<i64>,
    pub active: bool,
}

/// 创建提醒。kind 取值：
/// - "after"：after_seconds 延时一次性（≥1 秒）
/// - "at"：at 为 RFC3339 字符串（未来时点）
/// - "every"：every_seconds 固定间隔重复（≥300 秒，dsh 五分钟下限）
pub fn create(
    conn: &Connection,
    conversation_id: &str,
    kind: &str,
    prompt: &str,
    after_seconds: Option<i64>,
    at: Option<&str>,
    every_seconds: Option<i64>,
) -> Result<String, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("invalid_prompt：提醒内容不能为空".into());
    }
    if prompt.chars().count() > 500 {
        return Err("invalid_prompt：提醒内容过长（≤500 字符）".into());
    }
    let now = now_secs();
    let (scheduled_at, every_secs): (i64, Option<i64>) = match kind {
        "after" => {
            let secs = after_seconds.unwrap_or(0);
            if secs < 1 {
                return Err("invalid_selector：after_seconds 必须 ≥1".into());
            }
            (now + secs, None)
        }
        "at" => {
            let at = at.ok_or_else(|| "invalid_selector：at 需要 RFC3339 时间字符串".to_string())?;
            let dt = chrono::DateTime::parse_from_rfc3339(at)
                .map_err(|_| "invalid_selector：at 不是合法 RFC3339（如 2026-08-21T10:00:00+08:00）".to_string())?;
            let ts = dt.timestamp();
            if ts <= now {
                return Err("not_future：at 必须是未来时点".into());
            }
            (ts, None)
        }
        "every" => {
            let secs = every_seconds.unwrap_or(0);
            if secs < 300 {
                return Err("frequency_too_high：every_seconds 必须 ≥300（5 分钟）".into());
            }
            (now + secs, Some(secs))
        }
        _ => return Err("invalid_selector：kind 仅支持 after / at / every".into()),
    };
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO message_reminders
            (id, conversation_id, kind, prompt, scheduled_at, every_seconds, active, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,1,?7)",
        rusqlite::params![id, conversation_id, kind, prompt, scheduled_at, every_secs, now],
    )
    .map_err(|e| format!("保存提醒失败：{e}"))?;
    Ok(id)
}

/// 列出会话的全部活跃提醒（新→旧）
pub fn list(conn: &Connection, conversation_id: &str) -> Result<Vec<ReminderInfo>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, prompt, scheduled_at, every_seconds, active
             FROM message_reminders WHERE conversation_id = ?1
             ORDER BY scheduled_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([conversation_id], |r| {
            Ok(ReminderInfo {
                id: r.get(0)?,
                kind: r.get(1)?,
                prompt: r.get(2)?,
                scheduled_at: r.get(3)?,
                every_seconds: r.get(4)?,
                active: r.get::<_, i64>(5)? != 0,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

/// 删除提醒（终结性：删除已不存在的 id 也成功）
pub fn delete(conn: &Connection, id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM message_reminders WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 到期投递：把全部已到期提醒作为对话消息交付，返回 (conversation_id, prompt) 列表。
/// - after/at：投递后终结（active=0）
/// - every：推进 scheduled_at 到判断时刻之后第一个锚点对齐目标（错过多次只投递一次，不枚举）
///   调用方负责把返回项注入会话队列 + 桌面通知（对齐 dsh：只经普通对话 transcript 出现）
pub fn dispatch_due(conn: &Connection, now: i64) -> Result<Vec<(String, String)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, conversation_id, kind, prompt, scheduled_at, every_seconds
             FROM message_reminders WHERE active = 1 AND scheduled_at <= ?1
             ORDER BY scheduled_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([now], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<i64>>(5)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let due: Vec<(String, String, String, String, i64, Option<i64>)> =
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    drop(stmt);
    let mut delivered = Vec::with_capacity(due.len());
    for (id, conv, kind, prompt, scheduled_at, every_secs) in due {
        if kind == "every" {
            let every = every_secs.unwrap_or(300).max(1);
            let mut next = scheduled_at;
            while next <= now {
                next += every;
            }
            conn.execute(
                "UPDATE message_reminders SET scheduled_at = ?1, last_dispatch_at = ?2 WHERE id = ?3",
                rusqlite::params![next, now, id],
            )
            .map_err(|e| e.to_string())?;
        } else {
            conn.execute(
                "UPDATE message_reminders SET active = 0, last_dispatch_at = ?1 WHERE id = ?2",
                rusqlite::params![now, id],
            )
            .map_err(|e| e.to_string())?;
        }
        delivered.push((conv, format!("【定时提醒】{prompt}")));
    }
    Ok(delivered)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn create_validates_prompt_and_selector() {
        let conn = mem();
        conn.execute_batch(
            "CREATE TABLE message_reminders (
                id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL,
                kind TEXT NOT NULL, prompt TEXT NOT NULL,
                scheduled_at INTEGER NOT NULL, every_seconds INTEGER,
                active INTEGER NOT NULL DEFAULT 1, created_at INTEGER NOT NULL,
                last_dispatch_at INTEGER)",
        )
        .unwrap();
        assert!(create(&conn, "c1", "after", "  ", None, None, None).unwrap_err().contains("invalid_prompt"));
        assert!(create(&conn, "c1", "bad", "x", None, None, None).unwrap_err().contains("invalid_selector"));
        assert!(create(&conn, "c1", "after", "x", Some(0), None, None).unwrap_err().contains("invalid_selector"));
        assert!(create(&conn, "c1", "at", "x", None, Some("2000-01-01T00:00:00Z"), None).unwrap_err().contains("not_future"));
        assert!(create(&conn, "c1", "every", "x", None, None, Some(60)).unwrap_err().contains("frequency_too_high"));
    }

    #[test]
    fn dispatch_finishes_one_shot_and_advances_every() {
        let conn = mem();
        conn.execute_batch(
            "CREATE TABLE message_reminders (
                id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL,
                kind TEXT NOT NULL, prompt TEXT NOT NULL,
                scheduled_at INTEGER NOT NULL, every_seconds INTEGER,
                active INTEGER NOT NULL DEFAULT 1, created_at INTEGER NOT NULL,
                last_dispatch_at INTEGER)",
        )
        .unwrap();
        let now = now_secs();
        let id1 = create(&conn, "c1", "after", "提醒一", Some(1), None, None).unwrap();
        let id2 = create(&conn, "c1", "every", "提醒二", None, None, Some(300)).unwrap();
        // 强制把两条的 scheduled_at 都拨到过去：验证一次性终结 + every 锚点推进
        conn.execute("UPDATE message_reminders SET scheduled_at = ?1 WHERE id = ?2", rusqlite::params![now - 100, &id1]).unwrap();
        conn.execute("UPDATE message_reminders SET scheduled_at = ?1 WHERE id = ?2", rusqlite::params![now - 610, &id2]).unwrap();
        let due = dispatch_due(&conn, now).unwrap();
        assert_eq!(due.len(), 2);
        assert!(due.iter().any(|(c, p)| c == "c1" && p.contains("提醒一")));
        // 一次性已终结，every 推进到 > now 的下一个锚点（610 秒前 + 300 步长 → 至少推进 2 步）
        let active1: i64 = conn
            .query_row("SELECT active FROM message_reminders WHERE id = ?1", rusqlite::params![&id1], |r| r.get(0))
            .unwrap();
        assert_eq!(active1, 0);
        let sched2: i64 = conn
            .query_row("SELECT scheduled_at FROM message_reminders WHERE id = ?1", rusqlite::params![&id2], |r| r.get(0))
            .unwrap();
        assert!(sched2 > now, "every 应推进到未来锚点：{sched2} > {now}");
        // 再次投递：every 未到期不重复
        let due2 = dispatch_due(&conn, now).unwrap();
        assert!(due2.is_empty());
    }
}
