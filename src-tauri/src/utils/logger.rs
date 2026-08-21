//! 结构化任务日志（可观测性基础：问题排障、执行历史审计）。
//!
//! 追加 JSON 行到 `<app_data_dir>/logs/deveco-switch.log`；超过轮转阈值
//! （5MB）时重命名为 `.1` 并新建文件。日志写入失败静默忽略，不影响主流程。
//!
//! 用法：`lib.rs` setup 时 `logger::init(log_dir)`；关键点调用
//! `logger::log_event("task_finished", json!({...}))`。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// 日志文件轮转阈值（字节）
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

static LOG_DIR: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

/// 初始化日志目录（幂等；不传或失败时日志静默禁用）
pub fn init(dir: PathBuf) {
    std::fs::create_dir_all(&dir).ok();
    let _ = LOG_DIR.get_or_init(|| Mutex::new(Some(dir)));
}

/// 追加一条事件日志（JSON 行）。写入失败静默忽略（日志不能影响主流程）。
pub fn log_event(event: &str, payload: serde_json::Value) {
    let Some(guard) = LOG_DIR.get() else { return };
    let dir = match guard.lock() {
        Ok(g) => match g.as_ref() {
            Some(d) => d.clone(),
            None => return,
        },
        Err(_) => return,
    };
    let path = dir.join("deveco-switch.log");
    let payload = crate::utils::redact::redact_json_value(&payload);
    let line = serde_json::json!({
        "ts": chrono::Utc::now().timestamp_millis(),
        "event": event,
        "data": payload,
    });
    let line = format!("{line}\n");

    // 轮转：超过阈值时把旧文件顺延为 .1（再旧的覆盖，只保留一份历史）
    if std::fs::metadata(&path)
        .map(|m| m.len() > MAX_LOG_BYTES)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, dir.join("deveco-switch.log.1"));
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut f) = options.open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_log_event_appends_json_lines() {
        let _guard = TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("ds-log-test-{}", std::process::id()));
        // 先清理残留（pid 复用/上次中断会留下旧文件，append 模式会把旧行带进断言）
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok();
        init(dir.clone());
        log_event(
            "task_finished",
            serde_json::json!({"status": "success", "n": 1, "api_key": "sk-abc1234567890abcdef"}),
        );
        log_event(
            "task_finished",
            serde_json::json!({"status": "error", "n": 2}),
        );
        let content = std::fs::read_to_string(dir.join("deveco-switch.log")).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"event\":\"task_finished\""));
        assert!(lines[0].contains("\"success\""));
        assert!(!content.contains("sk-abc1234567890abcdef"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_uninit_is_noop() {
        let _guard = TEST_LOCK.lock().unwrap();
        // 未调用 init：不 panic、不写文件
        log_event("task_finished", serde_json::json!({"n": 1}));
    }
}
