//! 任务清单（todo_write 工具的状态存储）
//!
//! todo_write 让 Agent 把复杂任务拆成清单并随进度更新状态，前端实时渲染进度；
//! 对标主流 Agent 的 TodoWrite/task list 能力，减少长任务遗漏与跑偏。
//!
//! 设计取舍：进程内 Mutex<HashMap>（会话级运行态，重启清空），不落库——
//! 与 undo 快照/诊断缓存同取舍；更新时同步 emit 事件，前端按会话过滤展示。

#[derive(Clone, Debug, serde::Serialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    /// pending | in_progress | done
    pub status: String,
}

#[derive(Clone, serde::Serialize)]
pub struct TodoEvent {
    pub conversation_id: String,
    pub todos: Vec<TodoItem>,
}

/// 访问会话级任务清单（统一收敛到 SessionContext，锁由进程级单例持有）
fn table() -> std::sync::MutexGuard<'static, crate::agent::session_ctx::SessionContext> {
    crate::agent::session_ctx::sessions()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// 按 id 合并清单（merge=true 语义）：已存在的项更新 content/status，新项追加；返回合并结果。
pub fn merge(conversation_id: &str, incoming: Vec<TodoItem>) -> Vec<TodoItem> {
    let mut ctx = table();
    let list = ctx.todo_lists.entry(conversation_id.to_string()).or_default();
    for item in incoming {
        if let Some(existing) = list.iter_mut().find(|t| t.id == item.id) {
            existing.content = item.content;
            existing.status = item.status;
        } else {
            list.push(item);
        }
    }
    list.clone()
}

/// 整体替换清单（merge=false 语义）。
pub fn replace(conversation_id: &str, items: Vec<TodoItem>) -> Vec<TodoItem> {
    let mut ctx = table();
    *ctx.todo_lists.entry(conversation_id.to_string()).or_default() = items.clone();
    items
}

/// 读取当前清单。
pub fn get(conversation_id: &str) -> Vec<TodoItem> {
    table()
        .todo_lists
        .get(conversation_id)
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, status: &str) -> TodoItem {
        TodoItem { id: id.into(), content: format!("任务 {id}"), status: status.into() }
    }

    #[test]
    fn merge_updates_and_appends() {
        crate::agent::session_ctx::drop_session("t1");
        let v = merge("t1", vec![item("a", "pending"), item("b", "pending")]);
        assert_eq!(v.len(), 2);
        let v = merge("t1", vec![item("a", "done"), item("c", "pending")]);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].status, "done");
        assert_eq!(v[2].id, "c");
    }

    #[test]
    fn replace_overwrites() {
        crate::agent::session_ctx::drop_session("t2");
        replace("t2", vec![item("a", "pending")]);
        let v = replace("t2", vec![item("z", "done")]);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, "z");
        assert_eq!(get("t2").len(), 1);
    }
}
