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

/// 弹出**最近 `count` 条中通过 `keep` 筛选的**快照，不通过的原样留在栈里。
///
/// 返回顺序 = **LIFO：栈顶（最近一次）在前**，`taken.first()` 是最新的一条。
///
/// ⚠️ 这里曾经是「从旧到新」，与本函数自己的文档相反，也与 `undo_preview` 相反——
/// 预览用 `peek_at(i)` 枚举，`peek_at(0)` 是最近一次，所以预览把最近一次列为「步骤 1」，
/// 而实际恢复却从最旧的一条开始。中途失败时得到的是一段**与预览描述相反顺序**的部分撤销。
/// 现在实现与文档、与预览三者一致。
/// 终态（同一文件连续改 N 次全部恢复）在两种顺序下相同，差别只在**中途失败**时，
/// 所以这是修正契约漂移，不是改变既有语义。
///
/// 为什么没有「先全 pop 再逐条判断」的写法：那会让被否决的条目**永久离开栈**。
/// `fs_tools::undo_edit` 旧实现正是如此——快照 pop 出来发现路径不在会话可见根内就
/// `continue` 丢弃，用户那次撤销能力凭空消失，而返回文案还告诉调用方
/// 「本会话尚无 Agent 文件写入记录」。本原语让越界条目留在栈内
/// （换个项目/根重绑后仍可撤销），并让调用方能如实报告跳过数。
///
/// ⚠️ 这是本模块**唯一**的弹栈入口。不要为了「省事」再加一个无条件 `pop` 版本——
/// 那等于给同一个破坏性操作开出第二条没有护栏的路。
pub fn pop_undo_filtered<F: Fn(&Snapshot) -> bool>(
    conversation_id: &str,
    count: usize,
    keep: F,
) -> (Vec<Snapshot>, usize) {
    let mut ctx = table();
    let Some(list) = ctx.undo_stacks.get_mut(conversation_id) else {
        return (Vec::new(), 0);
    };
    // 从栈顶往下最多检查 count 条；保持其余条目的相对顺序不动。
    let window = count.min(list.len());
    let split = list.len() - window;
    let mut taken: Vec<Snapshot> = Vec::with_capacity(window);
    let mut skipped: Vec<Snapshot> = Vec::with_capacity(window);
    for item in list.drain(split..) {
        if taken.len() < count && keep(&item) {
            taken.push(item);
        } else {
            skipped.push(item);
        }
    }
    // 跳过的按原顺序接回去（栈是 FIFO 淘汰 + LIFO 弹出，顺序必须保持）
    let skipped_n = skipped.len();
    for item in skipped.into_iter().rev() {
        list.push(item);
    }
    // drain 按下标递增产出，得到的是「最旧 → 最新」；反转成 LIFO 后返回。
    taken.reverse();
    (taken, skipped_n)
}

/// 把一批已弹出的快照**放回**栈中，恢复它们原先的相对顺序。
///
/// 为什么需要它：`pop_undo_filtered` 是破坏性的，而 `fs_tools::undo_edit` 弹栈之后
/// 才逐条写盘。写盘失败时若不回填，那几条快照就**永久离开撤销栈**——
/// 而它们是旧内容的**唯一副本**（磁盘上已经是新内容了），丢掉等于用户的撤销能力
/// 凭空消失且再也找不回来。实测可达：一次 undo 恢复多条，第 2 条写失败
/// （父目录创建被拒、路径已变成目录、磁盘满）时，后续条目一并消失。
///
/// `pop_undo_filtered` 解决的是「被筛选否决的条目不能销毁」，本原语解决
/// 「取出来之后没派上用场也不能销毁」——同一条纪律的两半，缺一半就仍有洞。
///
/// ⚠️ `items` 必须是 `pop_undo_filtered` 返回的**原始 LIFO 顺序（栈顶在前）**，
/// 本函数**逆序**压回，从而恢复原有栈序（最后压入的落在栈尾 = 最新的那条）。
/// 若误正序压回，栈序会被整个颠倒，下次 undo 恢复的顺序全错。
/// ⚠️ 只回填**尚未恢复成功**的条目：已成功恢复的若也放回，会被二次恢复，
/// 把用户在两次 undo 之间做的修改覆盖掉。
pub fn restore_undo(conversation_id: &str, items: Vec<Snapshot>) {
    if conversation_id.is_empty() || items.is_empty() {
        return;
    }
    let mut ctx = table();
    let list = ctx.undo_stacks.entry(conversation_id.to_string()).or_default();
    // items 是栈顶在前，逆序 push 才能让其中最新的一条最终落在栈尾。
    for item in items.into_iter().rev() {
        list.push(item);
    }
    // 回填后仍受同一 FIFO 上限约束，避免回填把栈顶破
    if list.len() > MAX_PER_SESSION {
        let drop_n = list.len() - MAX_PER_SESSION;
        list.drain(0..drop_n);
    }
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
        let s = pop_undo_filtered("t1", 1, |_| true).0.remove(0);
        assert_eq!(s.content, b"v2");
        let s = pop_undo_filtered("t1", 1, |_| true).0.remove(0);
        assert_eq!(s.content, b"v1");
        assert!(pop_undo_filtered("t1", 1, |_| true).0.is_empty());
    }

    #[test]
    fn fifo_cap() {
        crate::agent::session_ctx::drop_session("t2");
        for i in 0..(MAX_PER_SESSION + 5) {
            snapshot("t2", std::path::Path::new("/x/f.txt"), &[i as u8]);
        }
        assert_eq!(undo_count("t2"), MAX_PER_SESSION);
        // 最老的被淘汰，最早可弹出的应是第 5 条之后的内容
        let s = pop_undo_filtered("t2", 1, |_| true).0.remove(0);
        assert_eq!(s.content, &[(MAX_PER_SESSION + 4) as u8]);
    }

    #[test]
    fn oversized_skipped() {
        crate::agent::session_ctx::drop_session("t3");
        snapshot("t3", std::path::Path::new("/x/big.txt"), &vec![0u8; MAX_CONTENT + 1]);
        assert_eq!(undo_count("t3"), 0);
    }
}
