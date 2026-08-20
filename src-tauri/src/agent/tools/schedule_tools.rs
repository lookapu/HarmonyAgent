//! 会话内定时提醒工具（对齐 deepseek-harness schedule）：schedule_create / schedule_list / schedule_delete。
//! 到期提醒以普通对话消息注入本会话队列（不中断当前轮次），模型下一轮请求自动看到。

use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

/// schedule_create：创建会话内定时提醒（after 延时一次性 / at 绝对时点一次性 / every 固定间隔重复）。
/// 参数：{"kind":"after|at|every"（缺省 after）,"prompt":"<提醒内容>",
///  "after_seconds":<kind=after 的延时秒数，≥1>,"at":"<kind=at 的 RFC3339 时间，如 2026-08-21T10:00:00+08:00>",
///  "every_seconds":<kind=every 的间隔秒数，≥300>}。
/// 适合：让 Agent 稍后提醒自己或用户（如"构建完成后 10 分钟提醒我检查日志"、
/// "每 30 分钟提醒一次进度汇报"）；到期后以普通对话消息出现，不打断当前执行。
pub(super) async fn schedule_create(
    args: &Value,
    ctx: &crate::agent::exec_ctx::ToolCtx,
    db: &crate::db::DbState,
) -> Result<String, String> {
    let kind = args["kind"].as_str().unwrap_or("after").trim();
    let prompt = args["prompt"].as_str().unwrap_or("").trim().to_string();
    let after_seconds = args["after_seconds"].as_i64();
    let at = args["at"].as_str();
    let every_seconds = args["every_seconds"].as_i64();
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let id = crate::services::reminders::create(
        &conn,
        &ctx.conversation_id,
        kind,
        &prompt,
        after_seconds,
        at,
        every_seconds,
    )?;
    Ok(format!("已创建定时提醒（id={id}）：{prompt}\n到期后将以对话消息提醒，不会中断当前任务。"))
}

/// schedule_list：列出本会话全部定时提醒（含剩余时间与状态）。
pub(super) async fn schedule_list(
    _args: &Value,
    ctx: &crate::agent::exec_ctx::ToolCtx,
    db: &crate::db::DbState,
) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let items = crate::services::reminders::list(&conn, &ctx.conversation_id)?;
    if items.is_empty() {
        return Ok("当前会话没有定时提醒。需要定时提醒时可用 schedule_create 创建。".into());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut out = format!("当前会话定时提醒（{} 条）：\n", items.len());
    for (i, r) in items.iter().enumerate() {
        let remain = r.scheduled_at - now;
        let when = if !r.active {
            "已失效".to_string()
        } else if remain <= 0 {
            "已到期待投递".to_string()
        } else if remain < 60 {
            format!("{remain} 秒后")
        } else if remain < 3600 {
            format!("{} 分钟后", remain / 60)
        } else {
            format!("{} 小时后", remain / 3600)
        };
        let repeat = if r.kind == "every" {
            format!("（每 {} 分钟重复）", r.every_seconds.unwrap_or(300) / 60)
        } else {
            String::new()
        };
        out.push_str(&format!("{}. [{}{repeat}] {} —— {when}（id={}）\n", i + 1, r.kind, r.prompt, r.id));
    }
    Ok(out)
}

/// schedule_delete：删除指定定时提醒（传 id，先用 schedule_list 查看）。
pub(super) async fn schedule_delete(
    args: &Value,
    _ctx: &crate::agent::exec_ctx::ToolCtx,
    db: &crate::db::DbState,
) -> Result<String, String> {
    let id = args["id"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());
    let Some(id) = id else {
        return Err("schedule_delete 需要 id（先用 schedule_list 查看）".into());
    };
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::services::reminders::delete(&conn, id)?;
    Ok(format!("定时提醒 {id} 已删除。"))
}
