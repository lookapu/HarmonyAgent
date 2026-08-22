//! 设备端运行日志实时回流：部署成功后自动挂 hilog 监听，把应用运行期 error/异常
//! 捕获到环形缓存，Agent 可通过 read_runtime_logs 工具读取；检测到异常时写入
//! 跨轮诊断并向前端推送事件，形成"用户操作 → 运行报错 → Agent 主动修复"的闭环。

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::Emitter;

use crate::agent::exec_ctx::ToolCtx;

/// 每个项目保留的最近运行日志行数（环形缓冲）。
const MAX_LINES: usize = 400;
/// 连续多少行非异常后，把"异常片段"判定结束并落诊断。
const ANOMALY_TAIL_LINES: usize = 6;

struct Ring {
    lines: VecDeque<String>,
    /// 后台监听任务句柄（停止旧监听时 abort）
    handle: Option<tokio::task::JoinHandle<()>>,
}

static STORE: std::sync::OnceLock<Mutex<HashMap<String, Ring>>> = std::sync::OnceLock::new();

fn store() -> std::sync::MutexGuard<'static, HashMap<String, Ring>> {
    STORE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
}


/// 启动指定项目在指定设备上的运行日志监听（幂等：重复调用会先停掉旧监听）。
/// 监听 `hdc shell hilog -L E` 并按 bundle 关键字过滤，写入环形缓存；
/// 检测到 ArkTS 异常/崩溃/致命错误时落一条跨轮诊断并 emit 事件。
pub fn start(project_path: &str, ctx: &ToolCtx, device_id: &str, bundle: &str) {
    stop(project_path);

    let mut s = store();
    s.entry(project_path.to_string())
        .or_insert_with(|| Ring { lines: VecDeque::with_capacity(MAX_LINES), handle: None });
    drop(s);

    let key = project_path.to_string();
    let dev = device_id.to_string();
    let pkg = bundle.to_string();
    // 先在闭包外用原引用 emit"监听中"，再 clone 进后台任务
    if let Some(app) = &ctx.app {
        let _ = app.emit(
            "runtime-watching",
            serde_json::json!({ "conversation_id": ctx.conversation_id, "project_path": project_path, "watching": true, "bundle": bundle }),
        );
    }
    let ctx = ctx.clone();

    let handle = tokio::spawn(async move {
        // 仅抓 E（Error）及以上，降低噪声；按 bundle 关键字过滤
        let args = vec![
            "-t".to_string(), dev.clone(),
            "shell".into(), "hilog".into(),
            "-L".into(), "E".into(),
        ];
        let Ok(mut cmd) = crate::utils::process::command("hdc", &args) else { return };
        use tokio::io::{AsyncBufReadExt, BufReader};
        use std::process::Stdio;
        cmd.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());
        cmd.kill_on_drop(true);
        #[cfg(windows)]
        {
            cmd.creation_flags(0x0800_0000);
        }
        let Ok(mut child) = cmd.spawn() else { return };
        let Some(stdout) = child.stdout.take() else { return };

        let mut reader = BufReader::new(stdout).lines();
        // 异常片段聚合：命中异常关键词后开始收集，直到连续 ANOMALY_TAIL_LINES 行无关键词
        let mut anomaly: Vec<String> = Vec::new();
        let mut quiet_since = 0usize;
        let mut anomaly_kind = String::new();

        while let Ok(Some(line)) = reader.next_line().await {
            let relevant = line.contains(&pkg);
            let lower = line.to_lowercase();
            let is_error_marker = lower.contains("fatal")
                || lower.contains("exception")
                || lower.contains("crash")
                || lower.contains("err_error")
                || lower.contains("typeerror")
                || lower.contains("referenceerror")
                || lower.contains("syntaxerror")
                || lower.contains("appfreeze");
            // 只缓存与本应用相关的 error 行（hilog -L E 已经很稀，含包名才算）
            if relevant || is_error_marker {
                push_line(&key, &line);
            }
            // 异常聚合：命中异常关键词开始收集，相关栈帧延续
            if relevant && is_error_marker {
                if anomaly.is_empty() {
                    anomaly_kind = classify(&lower);
                }
                anomaly.push(line.clone());
                quiet_since = 0;
            } else if !anomaly.is_empty() && (relevant || line.trim_start().starts_with("at ")) {
                anomaly.push(line.clone());
                quiet_since = 0;
            } else if !anomaly.is_empty() {
                quiet_since += 1;
                if quiet_since >= ANOMALY_TAIL_LINES {
                    flush_anomaly(&key, &ctx, &anomaly_kind, &anomaly);
                    anomaly.clear();
                    anomaly_kind.clear();
                    quiet_since = 0;
                }
            }
        }
        // 流结束：刷新残留异常
        if !anomaly.is_empty() {
            flush_anomaly(&key, &ctx, &anomaly_kind, &anomaly);
        }
    });

    if let Some(ring) = store().get_mut(project_path) {
        ring.handle = Some(handle);
    }
}

/// 停止指定项目的运行日志监听
pub fn stop(project_path: &str) {
    let handle = store().get_mut(project_path).and_then(|r| r.handle.take());
    if let Some(h) = handle {
        h.abort();
    }
}

/// 读取最近的运行日志（供 read_runtime_logs 工具）
pub fn recent(project_path: &str, max: usize) -> String {
    let s = store();
    let Some(ring) = s.get(project_path) else { return String::new() };
    let n = ring.lines.len().min(max);
    ring.lines
        .iter()
        .rev()
        .take(n)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
}

/// 日志查询 DSL：在环形缓存上按关键字/正则过滤（grep on the fly）。
/// `filter` 为大小写不敏感子串；`regex` 为正则（filter 优先）；`context` 为命中行前后附加行数。
/// 两者都空时退化为 recent()。
pub fn search(project_path: &str, max: usize, filter: Option<&str>, regex: Option<&str>, context: usize) -> String {
    let s = store();
    let Some(ring) = s.get(project_path) else { return String::new() };
    let lines: Vec<&String> = ring.lines.iter().collect();
    let start = lines.len().saturating_sub(max.max(1));
    let window = &lines[start..];
    let Some(f) = filter.map(str::trim).filter(|f| !f.is_empty()) else {
        // 无过滤条件：原样返回最近 max 行
        return window.iter().map(|l| l.as_str()).collect::<Vec<_>>().join("\n");
    };
    let re = regex.map(str::trim).filter(|r| !r.is_empty()).and_then(|r| {
        regex::Regex::new(r).ok()
    });
    let lower_f = f.to_lowercase();
    let ctx_n = context.clamp(0, 10);
    let mut out: Vec<String> = Vec::new();
    for (i, l) in window.iter().enumerate() {
        let hit = re.as_ref().map(|re| re.is_match(l)).unwrap_or_else(|| l.to_lowercase().contains(&lower_f));
        if !hit {
            continue;
        }
        // 附带上下文行（不重复输出已输出的行）
        let lo = i.saturating_sub(ctx_n);
        let hi = (i + ctx_n + 1).min(window.len());
        for (j, line) in window.iter().enumerate().take(hi).skip(lo) {
            if out.last().map(String::as_str) == Some(line.as_str()) {
                continue;
            }
            let marker = if j == i { "> " } else { "  " };
            out.push(format!("{marker}{line}"));
        }
    }
    out.join("\n")
}

fn push_line(key: &str, line: &str) {
    let mut s = store();
    let ring = s
        .entry(key.to_string())
        .or_insert_with(|| Ring { lines: VecDeque::with_capacity(MAX_LINES), handle: None });
    if ring.lines.len() >= MAX_LINES {
        ring.lines.pop_front();
    }
    ring.lines.push_back(line.to_string());
}

fn flush_anomaly(key: &str, ctx: &ToolCtx, kind: &str, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    let joined = lines.join("\n");
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let summary = lines
        .iter()
        .find(|l| {
            let ll = l.to_lowercase();
            ll.contains("error") || ll.contains("exception") || ll.contains("fatal")
        })
        .cloned()
        .unwrap_or_else(|| lines[0].clone());
    let summary = truncate(&summary, 160).to_string();

    crate::agent::diagnostics::record(
        key,
        crate::agent::diagnostics::Diagnosis {
            source: "runtime_error".into(),
            category: kind.to_string(),
            summary: summary.clone(),
            detail: truncate(&joined, 800).to_string(),
            at: now,
        },
    );

    ctx.record_run_event(
        "harmony.runtime.anomaly",
        serde_json::json!({
            "project_path": key,
            "category": kind,
            "source": "hilog",
            "summary": summary,
            "detail": truncate(&joined, 1200),
        }),
    );

    if let Some(app) = &ctx.app {
        let _ = app.emit(
            "runtime-anomaly",
            serde_json::json!({
                "conversation_id": ctx.conversation_id,
                "run_id": ctx.run_id,
                "project_path": key,
                "category": kind,
                "summary": summary,
                "detail": truncate(&joined, 1200),
            }),
        );
    }
}

fn classify(lower: &str) -> String {
    if lower.contains("typeerror") {
        "arkts_type_error"
    } else if lower.contains("referenceerror") {
        "arkts_reference_error"
    } else if lower.contains("syntaxerror") {
        "arkts_syntax_error"
    } else if lower.contains("rangeerror") {
        "arkts_range_error"
    } else if lower.contains("native crash") || lower.contains("sigsegv") || lower.contains("cppcrash") {
        "native_crash"
    } else if lower.contains("appfreeze") || lower.contains("anr") || lower.contains("not responding") {
        "app_freeze"
    } else if lower.contains("permission") {
        "permission_missing"
    } else {
        "runtime_error"
    }
    .to_string()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_error_types() {
        assert_eq!(classify("e typeerror: xxx"), "arkts_type_error");
        assert_eq!(classify("f referenceerror bar"), "arkts_reference_error");
        assert_eq!(classify("native crash sigsegv"), "native_crash");
        assert_eq!(classify("appfreeze detected"), "app_freeze");
        assert_eq!(classify("application not responding anr"), "app_freeze");
        assert_eq!(classify("permission denied"), "permission_missing");
        assert_eq!(classify("some weird error"), "runtime_error");
    }

    #[test]
    fn ring_buffer_keeps_tail() {
        let k = "__rt_test__";
        stop(k);
        for i in 0..(MAX_LINES + 50) {
            push_line(k, &format!("line{i}"));
        }
        let out = recent(k, 1000);
        assert!(out.contains(&format!("line{}", MAX_LINES + 49)));
        assert!(!out.contains("line0"));
        stop(k);
        store().remove(k);
    }
}
