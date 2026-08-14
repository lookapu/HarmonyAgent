//! 工具执行上下文：携带 Tauri AppHandle，用于向对话流推送实时日志事件
//! （如构建日志 agent:log），并提供构建日志落盘能力。

use std::path::PathBuf;
use std::process::Output;
use tauri::{AppHandle, Emitter};

/// 会话级“停止当前工具”请求集合：chat.rs 的 stop_tool 命令写入，工具执行处消费（一次性）。
/// 与 ChatCancel（停止整个任务）独立：只打断当前工具，模型拿到中断反馈后继续生成结论。
static STOP_TOOL_FLAGS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

/// 当前正在执行工具的会话 id 集合（跨项目可并行：多会话可同时执行工具；
/// 引用计数处理嵌套 run_tool 场景，避免一个会话退出时清掉其他会话的标记）
static ACTIVE_TOOL_SESSIONS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, usize>>,
> = std::sync::OnceLock::new();

/// 请求停止会话当前正在执行的工具（一次性标志）
pub fn request_stop_tool(conversation_id: &str) {
    if let Ok(mut set) = STOP_TOOL_FLAGS
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
        .lock()
    {
        set.insert(conversation_id.to_string());
    }
}

/// 检查并消费“停止当前工具”请求（一次性：读取后清除）
pub fn take_stop_tool(conversation_id: &str) -> bool {
    let Some(flag) = STOP_TOOL_FLAGS.get() else {
        return false;
    };
    if let Ok(mut set) = flag.lock() {
        if set.remove(conversation_id) {
            return true;
        }
    }
    false
}

/// run_tool 入口登记：会话进入工具执行（嵌套调用计数 +1）
pub fn enter_tool_session(conversation_id: &str) {
    if let Ok(mut map) = ACTIVE_TOOL_SESSIONS
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
        .lock()
    {
        *map.entry(conversation_id.to_string()).or_insert(0) += 1;
    }
}

/// run_tool 结束退出：计数 -1，归零移除
pub fn exit_tool_session(conversation_id: &str) {
    let Some(m) = ACTIVE_TOOL_SESSIONS.get() else {
        return;
    };
    if let Ok(mut map) = m.lock() {
        if let Some(n) = map.get_mut(conversation_id) {
            *n -= 1;
            if *n == 0 {
                map.remove(conversation_id);
            }
        }
    }
}

/// 当前正在执行工具的会话 id 列表（供无会话信息的命令执行器轮询中断）
pub fn active_tool_sessions() -> Vec<String> {
    ACTIVE_TOOL_SESSIONS
        .get()
        .and_then(|s| s.lock().ok())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// 工具执行上下文（无事件能力时也可使用 empty）
#[derive(Clone)]
pub struct ToolCtx {
    pub app: Option<AppHandle>,
    /// 当前会话 id（事件 payload 中带回，前端按会话过滤）
    pub conversation_id: String,
    /// 还可再委派子 Agent 的层数（防无限嵌套；主 Agent=1，子 Agent 由委派约束决定）
    pub spawn_remaining: usize,
}

impl ToolCtx {
    pub fn new(app: AppHandle, conversation_id: String) -> Self {
        Self { app: Some(app), conversation_id, spawn_remaining: 1 }
    }

    #[allow(dead_code)]
    pub fn empty() -> Self {
        Self { app: None, conversation_id: String::new(), spawn_remaining: 0 }
    }

    /// 推送一行流式日志到前端。失败静默（日志推送不应中断工具执行）。
    pub fn emit_log(&self, stream: &str, line: &str) {
        if let Some(app) = &self.app {
            let _ = app.emit(
                "agent:log",
                LogEvent {
                    conversation_id: self.conversation_id.clone(),
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
    /// "stdout" | "stderr" | "system"
    pub stream: String,
    pub line: String,
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
    use tokio::io::{AsyncBufReadExt, BufReader};

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
        Some(crate::agent::tools::smart_decode(buf))
    }

    // 每个流读取任务持有 ctx 的 clone 与 log_file 的 clone
    let ctx_out = ctx.clone();
    let log_out = log_file.map(|p| p.to_path_buf());
    let stdout_task = tokio::spawn(async move {
        let mut collected = String::new();
        if let Some(pipe) = stdout {
            let mut reader = BufReader::new(pipe);
            let mut buf: Vec<u8> = Vec::new();
            while let Some(line) = read_line_smart(&mut reader, &mut buf).await {
                collected.push_str(&line);
                collected.push('\n');
                // 收集缓冲上限：构建输出可达数十 MB，只保留尾部供工具结果解析
                // （完整日志已逐行落盘 + 推送事件，不依赖此缓冲）
                keep_tail_of_collected(&mut collected);
                ctx_out.emit_log("stdout", &line);
                if let Some(p) = &log_out {
                    append_log(p, &(line + "\n"));
                }
            }
        }
        collected
    });

    let ctx_err = ctx.clone();
    let log_err = log_file.map(|p| p.to_path_buf());
    let stderr_task = tokio::spawn(async move {
        let mut collected = String::new();
        if let Some(pipe) = stderr {
            let mut reader = BufReader::new(pipe);
            let mut buf: Vec<u8> = Vec::new();
            while let Some(line) = read_line_smart(&mut reader, &mut buf).await {
                collected.push_str(&line);
                collected.push('\n');
                keep_tail_of_collected(&mut collected);
                ctx_err.emit_log("stderr", &line);
                if let Some(p) = &log_err {
                    append_log(p, &(line + "\n"));
                }
            }
        }
        collected
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
                let _ = stdout_task.await;
                let _ = stderr_task.await;
                return Err(format!("命令超时（>{timeout_secs}s），已终止: {program}"));
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(300)) => {
                // 消费式检查：中断后清除标志，避免下一次工具调用被误中断
                if take_stop_tool(&ctx.conversation_id) {
                    crate::utils::process::kill_tree(pid);
                    let _ = stdout_task.await;
                    let _ = stderr_task.await;
                    ctx.emit_log("system", "命令已由用户中断");
                    return Err("用户已停止当前工具".into());
                }
            }
        }
    };

    let out = stdout_task.await.unwrap_or_default();
    let err = stderr_task.await.unwrap_or_default();

    Ok(Output {
        status,
        stdout: out.into_bytes(),
        stderr: err.into_bytes(),
    })
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
