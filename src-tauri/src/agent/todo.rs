//! 任务清单（todo_write 工具的状态存储）
//!
//! todo_write 让 Agent 把复杂任务拆成清单并随进度更新状态，前端实时渲染进度；
//! 对标主流 Agent 的 TodoWrite/task list 能力，减少长任务遗漏与跑偏。
//!
//! 存储策略：内存（SessionContext）为读写主径，DB（conversation_todos 表）为持久化镜像——
//! 每次变更同步 upsert，读取时内存为空则从 DB 恢复。这样重启应用后历史会话的
//! 任务清单仍可展示（此前纯内存、重启即丢）。

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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
    let out = list.clone();
    drop(ctx);
    persist(conversation_id, &out);
    out
}

/// 整体替换清单（merge=false 语义）。
pub fn replace(conversation_id: &str, items: Vec<TodoItem>) -> Vec<TodoItem> {
    let mut ctx = table();
    *ctx.todo_lists.entry(conversation_id.to_string()).or_default() = items.clone();
    drop(ctx);
    persist(conversation_id, &items);
    items
}

/// 读取当前清单：内存优先，内存为空时尝试从 DB 恢复（重启后历史会话仍可见）。
pub fn get(conversation_id: &str) -> Vec<TodoItem> {
    {
        let ctx = table();
        if let Some(list) = ctx.todo_lists.get(conversation_id) {
            if !list.is_empty() {
                return list.clone();
            }
        }
    }
    load(conversation_id)
}

/// 持久化清单到 DB（best effort：写失败不影响内存主径，仅静默降级）。
fn persist(conversation_id: &str, items: &[TodoItem]) {
    let Some(db) = crate::db::global() else { return };
    let Ok(conn) = db.lock() else { return };
    let json = serde_json::to_string(items).unwrap_or_else(|_| "[]".to_string());
    let _ = conn.execute(
        "INSERT INTO conversation_todos (conversation_id, items_json, updated_at)
         VALUES (?1, ?2, unixepoch())
         ON CONFLICT(conversation_id) DO UPDATE SET items_json = excluded.items_json, updated_at = unixepoch()",
        rusqlite::params![conversation_id, json],
    );
}

/// 从 DB 恢复清单（内存无数据时调用）。
fn load(conversation_id: &str) -> Vec<TodoItem> {
    let Some(db) = crate::db::global() else { return Vec::new() };
    let Ok(conn) = db.lock() else { return Vec::new() };
    let json: String = match conn.query_row(
        "SELECT items_json FROM conversation_todos WHERE conversation_id = ?1",
        [conversation_id],
        |r| r.get(0),
    ) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    serde_json::from_str(&json).unwrap_or_default()
}

/// 项目级共享清单的存储键：`@project:<路径>` 前缀与会话 key 区分，跨会话共享（同项目各会话读写同一份）。
pub fn project_key(project_path: &str) -> String {
    format!("@project:{}", project_path.trim_end_matches(['/', '\\']))
}

/// 项目级清单统计（供 todo_write 带 project 时返回：跨会话可见历史任务）。
pub fn project_digest(project_path: &str) -> String {
    let items = get(&project_key(project_path));
    if items.is_empty() {
        return "该项目暂无跨会话共享任务清单".to_string();
    }
    let done = items.iter().filter(|t| t.status == "done").count();
    let doing = items.iter().filter(|t| t.status == "in_progress").count();
    let pending = items.iter().filter(|t| t.status == "pending").count();
    let mut s = format!("项目级共享清单：共 {} 项（done {done} / in_progress {doing} / pending {pending}）：\n", items.len());
    for t in items.iter().filter(|t| t.status != "done").take(8) {
        s.push_str(&format!("- [{}{}] {}\n", t.status, t.id, t.content));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, status: &str) -> TodoItem {
        TodoItem { id: id.into(), content: format!("任务 {id}"), status: status.into() }
    }

    #[test]
    fn project_scope_shared_across_conversations() {
        let key = project_key("P:/fake/proj");
        crate::agent::session_ctx::drop_session(&key);
        let v = merge(&key, vec![item("a", "pending")]);
        assert_eq!(v.len(), 1);
        assert!(project_digest("P:/fake/proj").contains("共 1 项"));
        // 同一项目不同会话（key 归一化）读写同一份
        assert_eq!(get(&project_key("P:/fake/proj")).len(), 1);
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
