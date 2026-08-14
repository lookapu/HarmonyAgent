//! 会话级运行态上下文：集中持有按会话（conversation_id）键控的内存状态。
//!
//! 背景：文件撤销栈（undo）、任务清单（todo）、任务计划（task_plans）、
//! 提问等待（ask）原先各自维护一个进程级 `static Mutex<HashMap<String, ...>>`，
//! 散落各处、无法统一清理。这里收敛为一个 `SessionContext` 单例：
//!
//! - 各业务模块仍以 conversation_id 为参数的公开 API 不变，调用点零改动；
//! - 新增 `drop_session`，会话删除/重置时一次性释放该会话的全部运行态；
//! - 进程级缓存类状态（embedding / sdk_api / harmony_env 等）不在此列，
//!   它们本应是进程共享缓存，仍留在原模块。

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use tokio::sync::oneshot;

use crate::agent::ask::AskEvent;
use crate::agent::todo::TodoItem;
use crate::agent::undo::Snapshot;

/// 计划步骤（plan_task 工具：todo/doing/done/failed 状态机）
#[derive(Clone, Debug)]
pub struct PlanStep {
    pub title: String,
    pub status: String, // todo / doing / done / failed
    pub note: String,
}

/// 全部会话级运行态存储（键为 conversation_id 或 request_id）。
#[derive(Default)]
pub struct SessionContext {
    /// 文件编辑撤销栈（undo_edit）：conversation_id -> 快照栈
    pub undo_stacks: HashMap<String, Vec<Snapshot>>,
    /// 任务清单（todo_write）：conversation_id -> 清单
    pub todo_lists: HashMap<String, Vec<TodoItem>>,
    /// 任务计划（plan_task）：conversation_id -> (标题, 步骤)
    pub task_plans: HashMap<String, (String, Vec<PlanStep>)>,
    /// 提问等待者（ask_user）：request_id -> (事件, 回答通道)
    pub ask_waiters: HashMap<String, (AskEvent, oneshot::Sender<String>)>,
    /// 轻量注入队列：异步事件（后台任务完成等）写入的模型可见消息，
    /// 下一轮请求组装时 drain 为 user 消息注入（防堆积：每会话上限 10 条）
    pub injected_messages: HashMap<String, VecDeque<String>>,
}

static SESSIONS: OnceLock<Mutex<SessionContext>> = OnceLock::new();

/// 进程级会话上下文单例（get_or_init 惰性初始化）。
pub fn sessions() -> &'static Mutex<SessionContext> {
    SESSIONS.get_or_init(|| Mutex::new(SessionContext::default()))
}

/// 清理某会话的全部运行态（撤销栈/清单/计划/提问/注入队列/后台任务），会话删除或重置时调用。
pub fn drop_session(conversation_id: &str) {
    // 后台任务强杀进程树并移除记录（jobs 模块独立注册表，不在 SessionContext 内）
    crate::agent::jobs::drop_conversation_jobs(conversation_id);
    if let Ok(mut ctx) = sessions().lock() {
        ctx.undo_stacks.remove(conversation_id);
        ctx.todo_lists.remove(conversation_id);
        ctx.task_plans.remove(conversation_id);
        ctx.ask_waiters.retain(|_, (ev, _)| ev.conversation_id != conversation_id);
        ctx.injected_messages.remove(conversation_id);
    }
}

/// 向会话注入一条模型可见消息（后台任务完成等异步事件）。
/// 队列上限 10 条，超限丢弃最旧的（防异步事件堆积撑大上下文）。
pub fn inject_message(conversation_id: &str, content: String) {
    if let Ok(mut ctx) = sessions().lock() {
        let q = ctx.injected_messages.entry(conversation_id.to_string()).or_default();
        if q.len() >= 10 {
            q.pop_front();
        }
        q.push_back(content);
    }
}

/// 取出会话全部注入消息（请求组装时调用；取出即清空）。
pub fn drain_injected(conversation_id: &str) -> Vec<String> {
    if let Ok(mut ctx) = sessions().lock() {
        if let Some(q) = ctx.injected_messages.remove(conversation_id) {
            return q.into_iter().collect();
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::todo::TodoItem;

    #[test]
    fn drop_session_clears_all_state() {
        // 不调用 clear_all()：全局单例与其它测试并发共享，清空会误删他人数据；
        // 只操作本测试专属的 c1/c2 key，按 key 精确断言
        {
            let mut ctx = sessions().lock().unwrap();
            ctx.undo_stacks.insert("c1".into(), vec![]);
            ctx.todo_lists.insert(
                "c1".into(),
                vec![TodoItem { id: "t".into(), content: "x".into(), status: "pending".into() }],
            );
            ctx.task_plans.insert("c1".into(), ("plan".into(), vec![]));
            ctx.ask_waiters.insert(
                "r1".into(),
                (
                    AskEvent {
                        conversation_id: "c1".into(),
                        request_id: "r1".into(),
                        question: "q".into(),
                        options: vec![],
                    },
                    oneshot::channel().0,
                ),
            );
            ctx.injected_messages.insert("c1".into(), VecDeque::from(["后台任务完成".into()]));
            // 另一会话不受影响
            ctx.todo_lists.insert("c2".into(), vec![]);
        }
        drop_session("c1");
        let ctx = sessions().lock().unwrap();
        assert!(!ctx.undo_stacks.contains_key("c1"));
        assert!(!ctx.todo_lists.contains_key("c1"));
        assert!(!ctx.task_plans.contains_key("c1"));
        assert!(!ctx.ask_waiters.contains_key("r1"));
        assert!(!ctx.injected_messages.contains_key("c1"));
        assert!(ctx.todo_lists.contains_key("c2")); // 另一会话不受影响
    }
}
