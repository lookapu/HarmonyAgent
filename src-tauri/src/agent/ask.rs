//! ask_user 工具：Agent 向用户提问并等待自由文本回答
//!
//! 协议：工具执行时注册一个 oneshot 通道并向前端推送 `chat-ask` 事件，
//! 前端渲染提问卡；用户提交后调 `resolve_ask_user` 命令把回答送回通道，
//! 工具把回答作为结果返回模型继续执行。
//!
//! 设计取舍：全局 OnceLock 静态（与 exec_ctx 的停止标志同模式）而非 Tauri State——
//! run_tool 无 State 访问，静态表避免改 run_tool 签名；挂起期间 stop_chat 通过
//! cancel_conversation 关闭通道实现立即退出。

use tokio::sync::oneshot;

#[derive(Clone, serde::Serialize)]
pub struct AskEvent {
    pub conversation_id: String,
    pub request_id: String,
    pub question: String,
    pub options: Vec<String>,
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
    request_id: String,
    question: String,
    options: Vec<String>,
) -> oneshot::Receiver<String> {
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
    rx
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

/// 用户回答：把回答文本送回通道。
pub fn resolve(request_id: &str, answer: String) -> bool {
    let tx = table().ask_waiters.remove(request_id).map(|(_, tx)| tx);
    match tx {
        Some(tx) => {
            let _ = tx.send(answer);
            true
        }
        None => false,
    }
}

/// 停止任务时关闭该会话所有未答复的提问（通道关闭 → 工具按"用户已停止"返回）。
pub fn cancel_conversation(conversation_id: &str) {
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
        let rid = "r1".to_string();
        let rx = wait("c1", rid.clone(), "q".into(), vec![]);
        assert_eq!(pending("c1").map(|e| e.request_id).as_deref(), Some("r1"));
        assert!(resolve(&rid, "是".into()));
        assert!(rx.blocking_recv().unwrap() == "是");
        assert!(!resolve(&rid, "again".into()));
        assert!(pending("c1").is_none());
    }

    #[test]
    fn cancel_closes_channel() {
        let rid = "r2".to_string();
        let rx = wait("c2", rid.clone(), "q".into(), vec!["a".into()]);
        assert_eq!(pending("c2").map(|e| e.options.len()), Some(1));
        cancel_conversation("c2");
        assert!(rx.blocking_recv().is_err());
        assert!(pending("c2").is_none());
    }
}
