//! 工具响应缓存：仅缓存明确列入白名单的稳定查询工具，短 TTL 避免重复 IO/计算。
//!
//! 安全边界：
//! - L0 只是必要条件，调用方还必须通过 `permissions::is_cacheable` 白名单；
//! - 缓存键含 project_id、有效根目录范围与参数（防跨项目/跨权限范围脏读）；
//! - 任意成功的非缓存工具调用都会由调用方清空缓存，正确性优先于命中率。

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

fn key(tool: &str, project_id: &str, scope: &[String], args: &Value) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    scope.hash(&mut h);
    args.to_string().hash(&mut h);
    format!("{tool}\u{0}{project_id}\u{0}{}", h.finish())
}

/// 查询缓存（未命中或已过期返回 None；过期条目顺带清理）
pub fn get(tool: &str, project_id: &str, scope: &[String], args: &Value) -> Option<String> {
    let mut guard = cache();
    let now = now_sec();
    let k = key(tool, project_id, scope, args);
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

/// 写入缓存（仅稳定查询工具由调用方把关；TTL 固定 15s）
pub fn put(tool: &str, project_id: &str, scope: &[String], args: &Value, value: &str) {
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
        key(tool, project_id, scope, args),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_isolated_by_effective_roots_and_clearable() {
        clear();
        let args = serde_json::json!({"path":"a.txt"});
        let root_a = vec!["/workspace/a".to_string()];
        let root_b = vec!["/workspace/b".to_string()];
        put("read_harmony_doc", "p", &root_a, &args, "A");
        assert_eq!(get("read_harmony_doc", "p", &root_a, &args).as_deref(), Some("A"));
        assert!(get("read_harmony_doc", "p", &root_b, &args).is_none());
        clear();
        assert!(get("read_harmony_doc", "p", &root_a, &args).is_none());
    }
}
