//! 工具执行上下文：携带 Tauri AppHandle，用于向对话流推送实时日志事件
//! （如构建日志 agent:log），并提供构建日志落盘能力。

use std::path::PathBuf;
use std::process::Output;
use tauri::{AppHandle, Emitter};

/// 会话级“停止当前工具”代次：每次请求递增。工具启动时记录基线，运行中发现代次
/// 改变就中断。相比一次性 HashSet，同批并行工具都能观察到停止且后续新工具不会误停。
static STOP_TOOL_GENERATIONS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> =
    std::sync::OnceLock::new();

tokio::task_local! {
    /// 当前异步工具调用所属会话与启动时停止代次。
    static CURRENT_TOOL_SESSION: (String, u64);
}

pub async fn scope_tool_session<F: std::future::Future>(
    conversation_id: String,
    stop_generation: u64,
    future: F,
) -> F::Output {
    CURRENT_TOOL_SESSION.scope((conversation_id, stop_generation), future).await
}

pub fn stop_generation(conversation_id: &str) -> u64 {
    STOP_TOOL_GENERATIONS
        .get()
        .and_then(|m| m.lock().ok())
        .and_then(|m| m.get(conversation_id).copied())
        .unwrap_or(0)
}

/// 请求停止会话当前正在执行的全部工具。
pub fn request_stop_tool(conversation_id: &str) {
    if let Ok(mut generations) = STOP_TOOL_GENERATIONS
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
        .lock()
    {
        let generation = generations.entry(conversation_id.to_string()).or_insert(0);
        *generation = generation.wrapping_add(1);
    }
}

/// 当前工具是否在启动后收到停止请求。无工具作用域时返回 false。
pub fn current_tool_stop_requested() -> bool {
    CURRENT_TOOL_SESSION
        .try_with(|(conversation_id, baseline)| stop_generation(conversation_id) != *baseline)
        .unwrap_or(false)
}

#[cfg(test)]
mod stop_generation_tests {
    use super::*;

    #[tokio::test]
    async fn one_stop_is_seen_by_all_current_tools_but_not_future_tools() {
        let conversation_id = format!("stop-generation-{}", uuid::Uuid::new_v4());
        let baseline = stop_generation(&conversation_id);
        request_stop_tool(&conversation_id);
        let first = scope_tool_session(conversation_id.clone(), baseline, async {
            current_tool_stop_requested()
        });
        let second = scope_tool_session(conversation_id.clone(), baseline, async {
            current_tool_stop_requested()
        });
        assert!(first.await);
        assert!(second.await);

        let latest = stop_generation(&conversation_id);
        assert!(!scope_tool_session(conversation_id, latest, async {
            current_tool_stop_requested()
        })
        .await);
    }
}

/// 工具执行上下文（无事件能力时也可使用 empty）
#[derive(Clone)]
pub struct ToolCtx {
    pub app: Option<AppHandle>,
    /// 当前会话 id（事件 payload 中带回，前端按会话过滤）
    pub conversation_id: String,
    /// 当前任务代次 id。工具/日志事件必须携带它，避免已停止旧任务的延迟输出污染新任务。
    pub run_id: String,
    /// 还可再委派子 Agent 的层数（防无限嵌套；主 Agent=1，子 Agent 由委派约束决定）
    pub spawn_remaining: usize,
}

impl ToolCtx {
    pub fn new(app: AppHandle, conversation_id: String, run_id: String) -> Self {
        Self { app: Some(app), conversation_id, run_id, spawn_remaining: 1 }
    }

    #[allow(dead_code)]
    pub fn empty() -> Self {
        Self { app: None, conversation_id: String::new(), run_id: String::new(), spawn_remaining: 0 }
    }

    /// 推送一行流式日志到前端。失败静默（日志推送不应中断工具执行）。
    pub fn emit_log(&self, stream: &str, line: &str) {
        if let Some(app) = &self.app {
            let _ = app.emit(
                "agent:log",
                LogEvent {
                    conversation_id: self.conversation_id.clone(),
                    run_id: self.run_id.clone(),
                    stream: stream.to_string(),
                    line: line.to_string(),
                },
            );
        }
    }
}

#[derive(serde::Serialize, Clone)]
pub struct LogEvent {
    pub conversation_id: String,
    pub run_id: String,
    /// "stdout" | "stderr" | "system"
    pub stream: String,
    pub line: String,
}

/// 高频子进程输出批量事件。把几十行合为一次 IPC，避免 Windows WebView2 事件泵被
/// hvigor/hdc 的逐行输出淹没；系统提示仍使用单行 LogEvent 以保证即时性。
#[derive(serde::Serialize, Clone)]
pub struct LogBatchEvent {
    pub conversation_id: String,
    pub run_id: String,
    pub stream: String,
    pub lines: Vec<String>,
}

impl ToolCtx {
    fn emit_log_batch(&self, stream: &str, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        if let Some(app) = &self.app {
            let _ = app.emit(
                "agent:log-batch",
                LogBatchEvent {
                    conversation_id: self.conversation_id.clone(),
                    run_id: self.run_id.clone(),
                    stream: stream.to_string(),
                    lines: lines.to_vec(),
                },
            );
        }
    }
}

/// 构建日志落盘目录：{project}/.deveco-agent/logs
pub fn log_dir(project_path: &str) -> PathBuf {
    PathBuf::from(project_path).join(".deveco-agent").join("logs")
}

/// 生成新的构建日志文件路径
pub fn new_build_log_path(project_path: &str) -> PathBuf {
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    log_dir(project_path).join(format!("build-{ts}.log"))
}

/// 追加文本到指定日志文件（失败静默）
pub fn append_log(path: &std::path::Path, text: &str) {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// 带流式回调的命令执行器：逐行读取 stdout/stderr，边读边通过 ctx 推送 `agent:log`，
/// 同时写入落盘日志。返回完整 Output（含退出码），供工具结果/错误解析使用。
///
/// 设计要点：回调只通过 `ctx.emit_log`（内部是 Clone 的 AppHandle，无 FnMut 共享问题），
/// stdout/stderr 各自一个读取任务，互不阻塞。
pub async fn run_cmd_streaming(
    ctx: &ToolCtx,
    program: &str,
    args: &[String],
    cwd: Option<&std::path::Path>,
    timeout_secs: u64,
    log_file: Option<&std::path::Path>,
) -> Result<Output, String> {
    run_cmd_streaming_env(ctx, program, args, cwd, timeout_secs, log_file, None).await
}

/// 与 run_cmd_streaming 相同，额外注入环境变量（如 hvigor 的 DEVECO_SDK_HOME）。
pub async fn run_cmd_streaming_env(
    ctx: &ToolCtx,
    program: &str,
    args: &[String],
    cwd: Option<&std::path::Path>,
    timeout_secs: u64,
    log_file: Option<&std::path::Path>,
    envs: Option<&[(String, String)]>,
) -> Result<Output, String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let mut cmd = crate::utils::process::command(program, args)?;
    if let Some(envs) = envs {
        cmd.envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().map_err(|e| format!("无法启动命令 {program}: {e}"))?;
    let pid = child.id();

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // 行级读取器：按字节读一行后 smart_decode（UTF-8/GBK 检测链）。
    // 不能直接用 BufReader::lines()：它按严格 UTF-8 解析，GBK 输出行会整行报错被吞，
    // 导致 Windows 下 hvigor/hdc 的中文错误信息完全消失。
    // （GBK 双字节序列不含 0x0A，按换行切行不会拆坏多字节字符）
    async fn read_line_smart<R: tokio::io::AsyncBufRead + Unpin>(
        reader: &mut R,
        buf: &mut Vec<u8>,
    ) -> Option<String> {
        buf.clear();
        let n = reader.read_until(b'\n', buf).await.ok()?;
        if n == 0 {
            return None;
        }
        while matches!(buf.last(), Some(b'\n') | Some(b'\r')) {
            buf.pop();
        }
        const MAX_EVENT_LINE_BYTES: usize = 64 * 1024;
        if buf.len() > MAX_EVENT_LINE_BYTES {
            let drop_n = buf.len() - MAX_EVENT_LINE_BYTES;
            buf.drain(..drop_n);
            return Some(format!(
                "[单行输出过长，已省略前 {drop_n} 字节] {}",
                crate::agent::tools::smart_decode(buf)
            ));
        }
        Some(crate::agent::tools::smart_decode(buf))
    }

    // 日志目录只同步创建一次；文件写入由 Tokio 文件句柄批量完成，避免每一行都在
    // async worker 上同步 open/write/close（Windows Defender 下该模式尤其容易卡顿）。
    if let Some(path) = log_file {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    // 每个流读取任务持有 ctx 的 clone 与独立 append 句柄。stdout/stderr 原本就是并发流，
    // 因此跨流的严格行顺序不作保证；单个批次内顺序保持不变。
    let stdout_snapshot = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let stdout_for_task = stdout_snapshot.clone();
    let ctx_out = ctx.clone();
    let log_out = log_file.map(|p| p.to_path_buf());
    let stdout_task = tokio::spawn(async move {
        let mut log = match log_out {
            Some(path) => tokio::fs::OpenOptions::new().create(true).append(true).open(path).await.ok(),
            None => None,
        };
        if let Some(pipe) = stdout {
            let mut reader = BufReader::new(pipe);
            let mut buf: Vec<u8> = Vec::new();
            let mut event_lines: Vec<String> = Vec::with_capacity(32);
            let mut last_event_flush = tokio::time::Instant::now() - std::time::Duration::from_millis(50);
            while let Some(line) = read_line_smart(&mut reader, &mut buf).await {
                if let Ok(mut collected) = stdout_for_task.lock() {
                    collected.push_str(&line);
                    collected.push('\n');
                    // 收集缓冲上限：构建输出可达数十 MB，只保留尾部供工具结果解析
                    // （完整日志已逐行落盘 + 推送事件，不依赖此缓冲）
                    keep_tail_of_collected(&mut collected);
                }
                event_lines.push(line);
                if event_lines.len() >= 32 || last_event_flush.elapsed() >= std::time::Duration::from_millis(50) {
                    ctx_out.emit_log_batch("stdout", &event_lines);
                    if let Some(file) = &mut log {
                        let chunk = event_lines.join("\n") + "\n";
                        let _ = file.write_all(chunk.as_bytes()).await;
                    }
                    event_lines.clear();
                    last_event_flush = tokio::time::Instant::now();
                }
            }
            ctx_out.emit_log_batch("stdout", &event_lines);
            if let Some(file) = &mut log {
                if !event_lines.is_empty() {
                    let chunk = event_lines.join("\n") + "\n";
                    let _ = file.write_all(chunk.as_bytes()).await;
                }
                let _ = file.flush().await;
            }
        }
        stdout_for_task.lock().map(|s| s.clone()).unwrap_or_default()
    });

    let stderr_snapshot = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let stderr_for_task = stderr_snapshot.clone();
    let ctx_err = ctx.clone();
    let log_err = log_file.map(|p| p.to_path_buf());
    let stderr_task = tokio::spawn(async move {
        let mut log = match log_err {
            Some(path) => tokio::fs::OpenOptions::new().create(true).append(true).open(path).await.ok(),
            None => None,
        };
        if let Some(pipe) = stderr {
            let mut reader = BufReader::new(pipe);
            let mut buf: Vec<u8> = Vec::new();
            let mut event_lines: Vec<String> = Vec::with_capacity(32);
            let mut last_event_flush = tokio::time::Instant::now() - std::time::Duration::from_millis(50);
            while let Some(line) = read_line_smart(&mut reader, &mut buf).await {
                if let Ok(mut collected) = stderr_for_task.lock() {
                    collected.push_str(&line);
                    collected.push('\n');
                    keep_tail_of_collected(&mut collected);
                }
                event_lines.push(line);
                if event_lines.len() >= 32 || last_event_flush.elapsed() >= std::time::Duration::from_millis(50) {
                    ctx_err.emit_log_batch("stderr", &event_lines);
                    if let Some(file) = &mut log {
                        let chunk = event_lines.join("\n") + "\n";
                        let _ = file.write_all(chunk.as_bytes()).await;
                    }
                    event_lines.clear();
                    last_event_flush = tokio::time::Instant::now();
                }
            }
            ctx_err.emit_log_batch("stderr", &event_lines);
            if let Some(file) = &mut log {
                if !event_lines.is_empty() {
                    let chunk = event_lines.join("\n") + "\n";
                    let _ = file.write_all(chunk.as_bytes()).await;
                }
                let _ = file.flush().await;
            }
        }
        stderr_for_task.lock().map(|s| s.clone()).unwrap_or_default()
    });

    // 等待结束：同时监听超时与“停止当前工具”请求（轮询中断标志，命中强杀进程树）
    let wait_fut = child.wait();
    tokio::pin!(wait_fut);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let status = loop {
        tokio::select! {
            r = &mut wait_fut => break r.map_err(|e| format!("等待命令失败: {e}"))?,
            _ = tokio::time::sleep_until(deadline) => {
                crate::utils::process::kill_tree(pid);
                let _ = finish_output_readers(stdout_task, stderr_task, &stdout_snapshot, &stderr_snapshot).await;
                return Err(format!("命令超时（>{timeout_secs}s），已终止: {program}"));
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(300)) => {
                if current_tool_stop_requested() {
                    crate::utils::process::kill_tree(pid);
                    let _ = finish_output_readers(stdout_task, stderr_task, &stdout_snapshot, &stderr_snapshot).await;
                    ctx.emit_log("system", "命令已由用户中断");
                    return Err("用户已停止当前工具".into());
                }
            }
        }
    };

    let (out, err) = finish_output_readers(stdout_task, stderr_task, &stdout_snapshot, &stderr_snapshot).await;

    Ok(Output {
        status,
        stdout: out.into_bytes(),
        stderr: err.into_bytes(),
    })
}

/// 子进程/包装器已退出后，孙进程在 Windows 上仍可能持有继承的 stdout/stderr 句柄，
/// 导致读取任务永远等不到 EOF。收尾最多等待 5 秒，之后中止读取任务；完整实时日志已
/// 在读取过程中落盘/推送，宁可丢失极少量尾部也不能让整个 Agent 对话永久挂起。
async fn finish_output_readers(
    mut stdout_task: tokio::task::JoinHandle<String>,
    mut stderr_task: tokio::task::JoinHandle<String>,
    stdout_snapshot: &std::sync::Mutex<String>,
    stderr_snapshot: &std::sync::Mutex<String>,
) -> (String, String) {
    let out = match tokio::time::timeout(std::time::Duration::from_secs(5), &mut stdout_task).await {
        Ok(v) => v.unwrap_or_default(),
        Err(_) => {
            stdout_task.abort();
            stdout_snapshot.lock().map(|s| s.clone()).unwrap_or_default()
        }
    };
    let err = match tokio::time::timeout(std::time::Duration::from_secs(5), &mut stderr_task).await {
        Ok(v) => v.unwrap_or_default(),
        Err(_) => {
            stderr_task.abort();
            stderr_snapshot.lock().map(|s| s.clone()).unwrap_or_default()
        }
    };
    (out, err)
}

/// 收集缓冲超过阈值时丢弃前半（保留尾部，错误结论通常在日志末尾）：
/// 缓冲增长到 2 倍上限才裁剪一次，避免超大输出时每行都做 drain 的 O(n²) 复制。
fn keep_tail_of_collected(collected: &mut String) {
    const MAX_KEEP: usize = 8 * 1024 * 1024;
    if collected.len() <= MAX_KEEP * 2 {
        return;
    }
    let mut split = collected.len() - MAX_KEEP;
    while !collected.is_char_boundary(split) {
        split += 1;
    }
    collected.drain(..split);
}
