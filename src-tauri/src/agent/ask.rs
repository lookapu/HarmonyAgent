//! ask_user 工具：Agent 向用户提问并等待自由文本回答
//!
//! 协议：工具执行时注册一个 oneshot 通道并向前端推送 `chat-ask` 事件，
//! 前端渲染提问卡；用户提交后调 `resolve_ask_user` 命令把回答送回通道，
//! 工具把回答作为结果返回模型继续执行。
//!
//! 设计取舍：全局 OnceLock 静态（与 exec_ctx 的停止标志同模式）而非 Tauri State——
//! run_tool 无 State 访问，静态表避免改 run_tool 签名；挂起期间 stop_chat 通过
//! cancel_conversation 关闭通道实现立即退出。

use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::oneshot;

#[derive(Clone, serde::Serialize)]
pub struct AskEvent {
    pub conversation_id: String,
    pub request_id: String,
    pub question: String,
    pub options: Vec<String>,
}

/// 一条已答复的提问记录（问题历史）
#[derive(Clone, serde::Serialize)]
pub struct AskRecord {
    pub question: String,
    pub options: Vec<String>,
    /// 用户的实际回答（跳过/超时为空串）
    pub answer: String,
    /// 答复时的 unix 秒
    pub at: i64,
}

fn now_sec() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// 访问会话级提问等待表（统一收敛到 SessionContext，锁由进程级单例持有）
fn table() -> std::sync::MutexGuard<'static, crate::agent::session_ctx::SessionContext> {
    crate::agent::session_ctx::sessions()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// 注册一次提问等待。返回回答通道。
pub fn wait(
    conversation_id: &str,
    run_id: &str,
    request_id: String,
    question: String,
    options: Vec<String>,
) -> Result<oneshot::Receiver<String>, String> {
    crate::agent::interactions::begin(
        &request_id,
        conversation_id,
        Some(run_id),
        "ask_user",
        &serde_json::json!({ "question": question, "options": options }),
    )?;
    let (tx, rx) = oneshot::channel();
    table()
        .ask_waiters
        .insert(
            request_id.clone(),
            (
                AskEvent {
                    conversation_id: conversation_id.to_string(),
                    request_id: request_id.clone(),
                    question,
                    options,
                },
                tx,
            ),
        );
    Ok(rx)
}

/// 查询会话内挂起的提问（前端切回会话时恢复提问卡；无挂起返回 None）。
pub fn pending(conversation_id: &str) -> Option<AskEvent> {
    table()
        .ask_waiters
        .values()
        .find(|(ev, _)| ev.conversation_id == conversation_id)
        .map(|(ev, _)| ev.clone())
}

/// 移除等待项（超时/完成时调用，防止表膨胀）。
pub fn remove(request_id: &str) {
    table().ask_waiters.remove(request_id);
}

/// 用户回答：把回答文本送回通道，并写入该会话的问题历史。
pub fn resolve(request_id: &str, answer: String) -> bool {
    let removed = table().ask_waiters.remove(request_id);
    match removed {
        Some((ev, tx)) => {
            let _ = crate::agent::interactions::finish(
                request_id,
                if answer.trim().is_empty() { "skipped" } else { "answered" },
                serde_json::json!({ "answer": answer }),
            );
            let _ = tx.send(answer.clone());
            record_history(&ev.conversation_id, &ev, &answer);
            true
        }
        None => false,
    }
}

/// 记录一条已答复的提问（每会话最多保留 20 条，超出丢弃最旧）。
pub fn record_history(conversation_id: &str, ev: &AskEvent, answer: &str) {
    let mut ctx = table();
    let list = ctx.ask_history.entry(conversation_id.to_string()).or_default();
    list.push(AskRecord {
        question: ev.question.clone(),
        options: ev.options.clone(),
        answer: answer.to_string(),
        at: now_sec(),
    });
    if list.len() > 20 {
        let drop_n = list.len() - 20;
        list.drain(0..drop_n);
    }
}

/// 查询会话内已答复的提问历史（新 → 旧，最多 limit 条）。
pub fn history(conversation_id: &str, limit: usize) -> Vec<AskRecord> {
    table()
        .ask_history
        .get(conversation_id)
        .map(|l| l.iter().rev().take(limit.clamp(1, 20)).cloned().collect())
        .unwrap_or_default()
}

/// 停止任务时关闭该会话所有未答复的提问（通道关闭 → 工具按"用户已停止"返回）。
pub fn cancel_conversation(conversation_id: &str) {
    crate::agent::interactions::cancel_conversation(conversation_id, "conversation_cancelled");
    let doomed: Vec<String> = table()
        .ask_waiters
        .iter()
        .filter(|(_, (ev, _))| ev.conversation_id == conversation_id)
        .map(|(k, _)| k.clone())
        .collect();
    for k in doomed {
        table().ask_waiters.remove(&k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_returns_answer() {
        // 会话/请求 ID 用专属前缀：全局会话单例与其它模块测试并发共享，
        // 避免与 session_ctx 等测试的同名 key 互相误删
        let rid = "ask-r1".to_string();
        let rx = wait("ask-c1", "", rid.clone(), "q".into(), vec![]).unwrap();
        assert_eq!(pending("ask-c1").map(|e| e.request_id).as_deref(), Some("ask-r1"));
        assert!(resolve(&rid, "是".into()));
        assert!(rx.blocking_recv().unwrap() == "是");
        assert!(!resolve(&rid, "again".into()));
        assert!(pending("ask-c1").is_none());
    }

    #[test]
    fn history_records_answered() {
        let rid = "ask-r3".to_string();
        let _rx = wait("ask-c3", "", rid.clone(), "选哪个？".into(), vec!["A".into(), "B".into()]).unwrap();
        assert!(resolve(&rid, "A".into()));
        let h = history("ask-c3", 10);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].question, "选哪个？");
        assert_eq!(h[0].answer, "A");
        assert_eq!(h[0].options.len(), 2);
    }

    #[test]
    fn cancel_closes_channel() {
        let rid = "ask-r2".to_string();
        let rx = wait("ask-c2", "", rid.clone(), "q".into(), vec!["a".into()]).unwrap();
        assert_eq!(pending("ask-c2").map(|e| e.options.len()), Some(1));
        cancel_conversation("ask-c2");
        assert!(rx.blocking_recv().is_err());
        assert!(pending("ask-c2").is_none());
    }
}
