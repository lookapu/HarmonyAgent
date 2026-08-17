//! 工具响应缓存（[67]）：仅缓存 L0 只读工具（list_dir/read_file/search 等），
//! 10-30s TTL，避免 Agent 在同一任务里重复调用只读工具时的重复 IO/计算。
//!
//! 安全边界：
//! - 只有 `permissions::tool_level == L0` 的工具才读写缓存（L1/L2 有副作用，绝不缓存）；
//! - 缓存键含 project_id 与参数序列化（防跨项目/跨参数脏读）；
//! - 写文件类工具成功后由调用方负责失效（本模块只按 TTL 过期，不感知文件系统）。

use serde_json::Value;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// 缓存 TTL：10s（快速重复调用去重）~ 30s（配置/环境类查询）
const TTL_SECS: i64 = 15;

struct Entry {
    expires_at: i64,
    value: String,
}

static CACHE: Mutex<Option<HashMap<String, Entry>>> = Mutex::new(None);

fn now_sec() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn cache() -> std::sync::MutexGuard<'static, Option<HashMap<String, Entry>>> {
    let mut guard = CACHE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    guard
}

fn key(tool: &str, project_id: &str, args: &Value) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    args.to_string().hash(&mut h);
    format!("{tool}\u{0}{project_id}\u{0}{}", h.finish())
}

/// 查询缓存（未命中或已过期返回 None；过期条目顺带清理）
pub fn get(tool: &str, project_id: &str, args: &Value) -> Option<String> {
    let mut guard = cache();
    let now = now_sec();
    let k = key(tool, project_id, args);
    let hit = guard
        .as_ref()
        .and_then(|m| m.get(&k))
        .filter(|e| e.expires_at > now)
        .map(|e| e.value.clone());
    if hit.is_none() {
        if let Some(m) = guard.as_mut() {
            m.remove(&k);
        }
    }
    hit
}

/// 写入缓存（仅 L0 工具由调用方把关；TTL 固定 15s）
pub fn put(tool: &str, project_id: &str, args: &Value, value: &str) {
    let mut guard = cache();
    let m = guard.as_mut().expect("cache() 已初始化");
    if m.len() > 512 {
        // 简单防膨胀：容量超限时清空最旧的一半
        let mut entries: Vec<(String, i64)> = m
            .iter()
            .map(|(k, e)| (k.clone(), e.expires_at))
            .collect();
        entries.sort_by_key(|(_, t)| *t);
        for (k, _) in entries.iter().take(entries.len() / 2) {
            m.remove(k);
        }
    }
    m.insert(
        key(tool, project_id, args),
        Entry {
            expires_at: now_sec() + TTL_SECS,
            value: value.to_string(),
        },
    );
}

/// 清空缓存（会话切换/手动失效时调用）
pub fn clear() {
    if let Ok(mut guard) = CACHE.lock() {
        if let Some(m) = guard.as_mut() {
            m.clear();
        }
    }
}
