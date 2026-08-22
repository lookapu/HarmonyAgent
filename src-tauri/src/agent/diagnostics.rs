//! 最近一次构建/部署/崩溃归因的进程内缓存（跨轮会话记忆）。
//!
//! 痛点：工具失败结果只作为当轮 tool 消息进入对话，几轮之后模型可能"忘掉"
//! 之前定位到的根因（比如签名未配、某个 ArkTS 异常类型），重复踩坑。
//! 这里把关键归因结论按项目路径缓存，system prompt 构建时读取最近的条目注入，
//! 让模型在整个修复会话中都记得"上次失败是什么、建议怎么修"。
//!
//! 设计取舍：用进程内 Mutex<HashMap> 而非数据库——这是临时的、短 TTL 的运行态
//! 信息，重启后清空是合理的；同时避免给工具执行路径增加 DB 锁竞争。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct Diagnosis {
    /// 来源工具：build_project / deploy_hap / crash_analysis 等
    pub source: String,
    /// 归因类别（与 structured_tool_error 的 category 对齐）
    pub category: String,
    /// 一句话摘要
    pub summary: String,
    /// 详细结论（可含定位与下一步建议）
    pub detail: String,
    /// 记录时的 unix 秒
    pub at: i64,
}

static STORE: Mutex<Option<HashMap<String, Vec<Diagnosis>>>> = Mutex::new(None);

fn now_sec() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn store() -> std::sync::MutexGuard<'static, Option<HashMap<String, Vec<Diagnosis>>>> {
    let mut guard = STORE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    guard
}

/// 记录某项目的一条归因。同来源只保留最新一条，整体最多保留 8 条，避免无限增长。
pub fn record(project_key: &str, d: Diagnosis) {
    let mut guard = store();
    let map = guard.as_mut().unwrap();
    let list = map.entry(project_key.to_string()).or_default();
    list.retain(|x| x.source != d.source);
    list.push(d);
    // 只留最近 8 条
    if list.len() > 8 {
        let drop_n = list.len() - 8;
        list.drain(0..drop_n);
    }
}

/// 读取某项目在 ttl_sec 内的归因（按时间倒序）。
pub fn recent(project_key: &str, ttl_sec: i64) -> Vec<Diagnosis> {
    let cutoff = now_sec() - ttl_sec;
    let mut guard = store();
    let map = match guard.as_mut() {
        Some(m) => m,
        None => return Vec::new(),
    };
    // 顺手清理过期项
    if let Some(list) = map.get_mut(project_key) {
        list.retain(|d| d.at >= cutoff);
    }
    map.get(project_key)
        .map(|list| {
            let mut v = list.clone();
            v.sort_by_key(|a| std::cmp::Reverse(a.at));
            v
        })
        .unwrap_or_default()
}

/// 构建成功/部署成功时清除该项目对应来源的失败归因，并返回被清除的记录
/// （调用方可据此生成"修复经验"候选：刚从失败走到成功，说明问题被解决了）。
pub fn clear_source(project_key: &str, source: &str) -> Vec<Diagnosis> {
    let mut guard = store();
    let map = match guard.as_mut() {
        Some(m) => m,
        None => return Vec::new(),
    };
    let removed = if let Some(list) = map.get_mut(project_key) {
        let (kept, removed): (Vec<_>, Vec<_>) = list.drain(..).partition(|d| d.source != source);
        *list = kept;
        removed
    } else {
        Vec::new()
    };
    removed
}

/// 格式化为注入 system prompt 的文本（无记录时返回空串）。
pub fn format_hint(project_key: &str, ttl_sec: i64) -> String {
    let items = recent(project_key, ttl_sec);
    if items.is_empty() {
        return String::new();
    }
    let mut s = String::from("【近期构建/部署/崩溃归因（本次会话修复时参考，不要重复已尝试失败的方式）】\n");
    for d in items {
        let mins = (now_sec() - d.at).max(0) / 60;
        s.push_str(&format!("- [{}·{}] {}（{} 分钟前）\n", d.source, d.category, d.summary, mins));
        for line in d.detail.lines().take(4) {
            if !line.trim().is_empty() {
                s.push_str(&format!("  {}\n", line.trim()));
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(source: &str, category: &str, summary: &str) -> Diagnosis {
        Diagnosis {
            source: source.into(),
            category: category.into(),
            summary: summary.into(),
            detail: "do this".into(),
            at: now_sec(),
        }
    }

    #[test]
    fn record_and_recent() {
        let key = "test-proj-record";
        clear_source(key, "build_project");
        record(key, mk("build_project", "type", "类型错误"));
        let r = recent(key, 600);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].category, "type");
    }

    #[test]
    fn same_source_replaces() {
        let key = "test-proj-replace";
        clear_source(key, "build_project");
        record(key, mk("build_project", "type", "旧"));
        record(key, mk("build_project", "syntax", "新"));
        let r = recent(key, 600);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].summary, "新");
    }

    #[test]
    fn clear_removes_source() {
        let key = "test-proj-clear";
        record(key, mk("deploy_hap", "signing", "签名"));
        clear_source(key, "deploy_hap");
        assert!(recent(key, 600).is_empty());
    }

    #[test]
    fn expired_excluded() {
        let key = "test-proj-expired";
        clear_source(key, "build_project");
        let mut d = mk("build_project", "type", "过期");
        d.at = now_sec() - 10_000;
        record(key, d);
        assert!(recent(key, 100).is_empty());
    }
}
