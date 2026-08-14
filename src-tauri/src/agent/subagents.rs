//! 子 Agent（spawn_agents）运行登记表：进程内最近运行记录，供 list_agents 工具查询。
//! 与 ask/todo 同模式：OnceLock 全局静态表，不落库，仅保留最近 50 条。

use std::sync::{Mutex, OnceLock};

/// 一条子 Agent 运行记录
#[derive(Clone, serde::Serialize)]
pub struct SubAgentRecord {
    /// 任务名（spawn_agents 的 name 参数）
    pub name: String,
    /// 实际使用的模型
    pub model: String,
    /// 开始时间（HH:MM:SS，本地时区）
    pub started_at: String,
    /// done | error | skipped（skipped=用户停止后未执行）
    pub status: String,
    /// 耗时毫秒（skipped 为 0）
    pub elapsed_ms: i64,
    /// 输出尾部摘要（最多 200 字符）
    pub output_tail: String,
}

static REGISTRY: OnceLock<Mutex<Vec<SubAgentRecord>>> = OnceLock::new();

fn table() -> &'static Mutex<Vec<SubAgentRecord>> {
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

/// 追加一条运行记录（只保留最近 50 条，超出丢弃最旧）
pub fn record(rec: SubAgentRecord) {
    if let Ok(mut v) = table().lock() {
        v.push(rec);
        if v.len() > 50 {
            let excess = v.len() - 50;
            v.drain(..excess);
        }
    }
}

/// 运行记录快照（新 → 旧）
pub fn snapshot() -> Vec<SubAgentRecord> {
    table()
        .lock()
        .map(|v| v.iter().rev().cloned().collect())
        .unwrap_or_default()
}
