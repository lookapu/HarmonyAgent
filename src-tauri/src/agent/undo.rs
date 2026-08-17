//! 文件修改快照（edit_file/write_file 撤销回滚）
//!
//! 痛点：Agent 连续编辑多步后方向走偏，用户或模型想撤销上一步时只能靠 git
//! 或手动改回；delete_file 有回收站而编辑没有对应能力。
//! 这里在每次写/编辑落盘前把旧内容快照到会话级栈，undo_edit 工具按栈序恢复。
//!
//! 设计取舍：进程内 Mutex<HashMap> 而非数据库——会话级运行态，重启清空合理，
//! 避免给高频编辑路径增加 DB 锁竞争；单文件内容 ≤1MB（与工具写入限制一致），
//! 每会话最多 40 条（FIFO 淘汰），防止长会话内存膨胀。

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct Snapshot {
    /// 文件绝对路径
    pub path: PathBuf,
    /// 修改前的内容（≤1MB）
    pub content: Vec<u8>,
    /// 记录时的 unix 秒
    pub at: i64,
}

const MAX_PER_SESSION: usize = 40;
const MAX_CONTENT: usize = 1024 * 1024;

fn now_sec() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// 访问会话级撤销栈（统一收敛到 SessionContext，锁由进程级单例持有）
fn table() -> std::sync::MutexGuard<'static, crate::agent::session_ctx::SessionContext> {
    crate::agent::session_ctx::sessions()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// 记录一次修改前的文件快照（文件不存在时传 None 不记录；超大文件跳过）。
pub fn snapshot(conversation_id: &str, path: &std::path::Path, old_content: &[u8]) {
    if conversation_id.is_empty() || old_content.len() > MAX_CONTENT {
        return;
    }
    let mut ctx = table();
    let list = ctx.undo_stacks.entry(conversation_id.to_string()).or_default();
    list.push(Snapshot {
        path: path.to_path_buf(),
        content: old_content.to_vec(),
        at: now_sec(),
    });
    // FIFO 淘汰：只保留最近 MAX_PER_SESSION 条
    if list.len() > MAX_PER_SESSION {
        let drop_n = list.len() - MAX_PER_SESSION;
        list.drain(0..drop_n);
    }
}

/// 弹出最近一次快照（LIFO）。无快照时返回 None。
pub fn pop_undo(conversation_id: &str) -> Option<Snapshot> {
    table().undo_stacks.get_mut(conversation_id).and_then(|l| l.pop())
}


/// 查看从栈顶数第 n 条快照（n=0 为最近一次，不弹出，撤销预览用）。
pub fn peek_at(conversation_id: &str, n: usize) -> Option<Snapshot> {
    let ctx = table();
    let l = ctx.undo_stacks.get(conversation_id)?;
    let idx = l.len().checked_sub(n + 1)?;
    l.get(idx).cloned()
}

/// 查询当前剩余可撤销次数（前端/工具结果展示用）。
pub fn undo_count(conversation_id: &str) -> usize {
    table()
        .undo_stacks
        .get(conversation_id)
        .map(|l| l.len())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_pop_lifo() {
        crate::agent::session_ctx::drop_session("t1");
        snapshot("t1", std::path::Path::new("/x/a.txt"), b"v1");
        snapshot("t1", std::path::Path::new("/x/b.txt"), b"v2");
        assert_eq!(undo_count("t1"), 2);
        let s = pop_undo("t1").unwrap();
        assert_eq!(s.content, b"v2");
        let s = pop_undo("t1").unwrap();
        assert_eq!(s.content, b"v1");
        assert!(pop_undo("t1").is_none());
    }

    #[test]
    fn fifo_cap() {
        crate::agent::session_ctx::drop_session("t2");
        for i in 0..(MAX_PER_SESSION + 5) {
            snapshot("t2", std::path::Path::new("/x/f.txt"), &[i as u8]);
        }
        assert_eq!(undo_count("t2"), MAX_PER_SESSION);
        // 最老的被淘汰，最早可弹出的应是第 5 条之后的内容
        let s = pop_undo("t2").unwrap();
        assert_eq!(s.content, &[(MAX_PER_SESSION + 4) as u8]);
    }

    #[test]
    fn oversized_skipped() {
        crate::agent::session_ctx::drop_session("t3");
        snapshot("t3", std::path::Path::new("/x/big.txt"), &vec![0u8; MAX_CONTENT + 1]);
        assert_eq!(undo_count("t3"), 0);
    }
}
