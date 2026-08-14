//! 后台任务（jobs）：run_command 的 run_in_background 模式 + job_list / job_output / job_kill
//! 组成的长任务管理协议。
//!
//! 场景：模型要跑长时间命令（构建/安装依赖/长测试）时不必阻塞等待——后台启动立即
//! 返回 job_id，模型继续规划其他步骤；任务完成时把结果摘要注入会话队列（模型下一轮
//! 请求自动看到），同时向前端推 chat-job-done 事件。
//!
//! 进程生命周期由本模块托管：超时/显式终止/会话清理时强杀进程树（kill_tree）；
//! 输出行级收集（smart_decode 兼容 GBK 中文），尾部缓冲上限 512KB 防内存膨胀。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::agent::exec_ctx::ToolCtx;

/// 会话活跃后台任务上限（防无限堆积：每个会话最多同时跑 4 个）
pub const MAX_JOBS_PER_CONVERSATION: usize = 4;
/// 任务输出尾部缓冲上限（字节）：超限丢弃前半（结论通常在末尾）
const JOB_OUTPUT_CAP: usize = 512 * 1024;

/// 后台任务完成事件（前端展示“后台任务完成”提示）
#[derive(Clone, serde::Serialize)]
pub struct JobDoneEvent {
    pub conversation_id: String,
    pub job_id: String,
    pub command: String,
    pub ok: bool,
    pub summary: String,
}

/// 单个后台任务记录（Arc<Mutex>：完成回调与查询并发安全）
pub struct Job {
    /// 归属会话（事件与注入都按会话定位）
    pub conversation_id: String,
    /// 命令文本（展示用）
    pub command: String,
    /// 工作目录
    pub cwd: PathBuf,
    /// 进程 id（未启动/已结束时为 None）
    pub pid: Option<u32>,
    /// 累计输出（stdout+stderr 合并，尾部缓冲）
    pub output: String,
    /// 生命周期状态：运行中 → 停止中 → 已结束（kill/超时先置 Stopping 再收尾）
    pub status: JobStatus,
    /// 是否成功（仅 status=Finished 后有效）
    pub ok: bool,
    /// 结束摘要（未结束时为 None）
    pub summary: Option<String>,
}

/// 任务生命周期状态（dsh 式 running→stopping→terminal 三态）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobStatus {
    /// 运行中（进程已启动或尚未退出）
    Running,
    /// 停止中（收到 kill/超时，进程树正在被终止，收尾尚未完成）
    Stopping,
    /// 已结束（正常完成/失败/被杀/超时，summary 有效）
    Finished,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Running => "running",
            JobStatus::Stopping => "stopping",
            JobStatus::Finished => "finished",
        }
    }
}

/// 任务注册表：job_id -> 任务记录
static JOBS: OnceLock<Mutex<HashMap<String, Arc<Mutex<Job>>>>> = OnceLock::new();

fn jobs() -> &'static Mutex<HashMap<String, Arc<Mutex<Job>>>> {
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 任务列表条目（序列化视图）
#[derive(Clone, serde::Serialize)]
pub struct JobInfo {
    pub job_id: String,
    pub command: String,
    pub cwd: String,
    pub finished: bool,
    /// 生命周期状态字符串（running/stopping/finished）
    pub status: String,
    pub ok: bool,
    pub summary: Option<String>,
    pub output_len: usize,
}

/// 后台启动命令：立即返回 job_id；完成后注入会话队列 + 发前端事件。
/// program/args 由调用方（run_command）完成 shell 语义解析（needs_shell/split_command）。
pub fn start_background(
    program: String,
    args: Vec<String>,
    command: String,
    cwd: PathBuf,
    timeout_secs: u64,
    ctx: &ToolCtx,
) -> Result<String, String> {
    // 会话活跃任务上限：超过拒绝启动（防无限堆积）
    {
        let map = jobs().lock().map_err(|e| e.to_string())?;
        let active = map
            .values()
            .filter(|j| {
                j.lock()
                    .map(|j| j.conversation_id == ctx.conversation_id && j.status != JobStatus::Finished)
                    .unwrap_or(false)
            })
            .count();
        if active >= MAX_JOBS_PER_CONVERSATION {
            return Err(format!(
                "本会话后台任务已达上限（{MAX_JOBS_PER_CONVERSATION} 个），请先用 job_kill 终止不再需要的任务"
            ));
        }
    }
    let job_id = format!("job-{:08x}", (uuid::Uuid::new_v4().as_u128() & 0xffff_ffff) as u32);
    let job = Arc::new(Mutex::new(Job {
        conversation_id: ctx.conversation_id.clone(),
        command: command.clone(),
        cwd: cwd.clone(),
        pid: None,
        output: String::new(),
        status: JobStatus::Running,
        ok: false,
        summary: None,
    }));
    {
        let mut map = jobs().lock().map_err(|e| e.to_string())?;
        map.insert(job_id.clone(), job.clone());
    }
    let app = ctx.app.clone();
    let conv = ctx.conversation_id.clone();
    let spawn_job_id = job_id.clone();
    let spawn_command = command.clone();
    tokio::spawn(async move {
        run_job(&job, &program, &args, &cwd, timeout_secs, app, conv, spawn_job_id, spawn_command).await;
    });
    Ok(job_id)
}

/// 后台任务主体：启动子进程、行级收集输出、超时强杀、结束收尾（注入 + 事件）
async fn run_job(
    job: &Arc<Mutex<Job>>,
    program: &str,
    args: &[String],
    cwd: &PathBuf,
    timeout_secs: u64,
    app: Option<AppHandle>,
    conv: String,
    job_id: String,
    command: String,
) {
    let mut cmd = match crate::utils::process::command(program, args) {
        Ok(c) => c,
        Err(e) => {
            finish_job(job, false, format!("无法启动命令 {program}: {e}"));
            notify(&app, &conv, &job_id, &command, false, "启动失败");
            return;
        }
    };
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .current_dir(cwd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            finish_job(job, false, format!("无法启动命令 {program}: {e}"));
            notify(&app, &conv, &job_id, &command, false, "启动失败");
            return;
        }
    };
    // tokio Child::id() 返回 Option<u32>（尚未启动时为 None）
    let pid = child.id();
    if let Ok(mut j) = job.lock() {
        j.pid = pid;
    }
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let job_stdout = job.clone();
    let stdout_task = tokio::spawn(async move {
        if let Some(pipe) = stdout {
            let mut reader = BufReader::new(pipe);
            let mut buf: Vec<u8> = Vec::new();
            while let Some(line) = read_line_smart(&mut reader, &mut buf).await {
                append_output(&job_stdout, &line);
            }
        }
    });
    let job_stderr = job.clone();
    let stderr_task = tokio::spawn(async move {
        if let Some(pipe) = stderr {
            let mut reader = BufReader::new(pipe);
            let mut buf: Vec<u8> = Vec::new();
            while let Some(line) = read_line_smart(&mut reader, &mut buf).await {
                append_output(&job_stderr, &line);
            }
        }
    });

    // 等待结束：超时强杀进程树（后台任务不响应“停止当前工具”，由 job_kill/会话清理终止）
    let wait_fut = child.wait();
    tokio::pin!(wait_fut);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let status = loop {
        tokio::select! {
            r = &mut wait_fut => break match r {
                Ok(s) => s,
                Err(e) => {
                    finish_job(job, false, format!("等待命令失败: {e}"));
                    return;
                }
            },
            _ = tokio::time::sleep_until(deadline) => {
                // 先置停止中（job_list 立即反映终止意图），再强杀进程树
                mark_stopping(job);
                crate::utils::process::kill_tree(pid);
                let _ = stdout_task.await;
                let _ = stderr_task.await;
                let summary = format!("命令超时（>{timeout_secs}s），已终止: {program}");
                finish_job(job, false, summary.clone());
                notify(&app, &conv, &job_id, &command, false, &summary);
                return;
            }
        }
    };
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    let ok = status.success();
    let summary = if ok {
        "命令执行成功（退出码 0）".to_string()
    } else {
        format!("命令退出码 {}", status.code().unwrap_or(-1))
    };
    finish_job(job, ok, summary.clone());
    // 结束摘要以任务记录为准（job_kill 已先置“已被终止”时不被退出码覆盖）
    let final_summary = job
        .lock()
        .map(|j| j.summary.clone().unwrap_or_else(|| summary.clone()))
        .unwrap_or_else(|_| summary.clone());
    notify(&app, &conv, &job_id, &command, ok, &final_summary);
}

/// 结束收尾：注入会话队列（模型下一轮请求自动看到）+ 前端事件
fn notify(app: &Option<AppHandle>, conv: &str, job_id: &str, command: &str, ok: bool, summary: &str) {
    crate::agent::session_ctx::inject_message(
        conv,
        format!(
            "[后台任务 {job_id} 完成] 命令：{command}\n结果：{summary}。可调用 job_output 查询完整输出后继续处理。"
        ),
    );
    if let Some(app) = app {
        let _ = app.emit(
            "chat-job-done",
            JobDoneEvent {
                conversation_id: conv.to_string(),
                job_id: job_id.to_string(),
                command: command.to_string(),
                ok,
                summary: summary.to_string(),
            },
        );
    }
}

/// 标记任务停止中（kill/超时发起后、收尾完成前）
fn mark_stopping(job: &Arc<Mutex<Job>>) {
    if let Ok(mut j) = job.lock() {
        if j.status == JobStatus::Running {
            j.status = JobStatus::Stopping;
        }
    }
}

/// 标记任务结束（记录结果摘要）；幂等：已结束的任务不覆盖（防 kill 与收尾竞争覆盖）
fn finish_job(job: &Arc<Mutex<Job>>, ok: bool, summary: String) {
    if let Ok(mut j) = job.lock() {
        if j.status == JobStatus::Finished {
            return;
        }
        j.status = JobStatus::Finished;
        j.ok = ok;
        j.summary = Some(summary);
    }
}

/// 追加一行输出到任务记录（带尾部缓冲上限，超限丢弃前半）
fn append_output(job: &Arc<Mutex<Job>>, line: &str) {
    if let Ok(mut j) = job.lock() {
        j.output.push_str(line);
        j.output.push('\n');
        if j.output.len() > JOB_OUTPUT_CAP * 2 {
            let mut split = j.output.len() - JOB_OUTPUT_CAP;
            while !j.output.is_char_boundary(split) {
                split += 1;
            }
            j.output.drain(..split);
        }
    }
}

/// 行级读取：按字节读一行后 smart_decode（UTF-8/GBK 检测链，与 exec_ctx 同口径）
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

/// 列出会话的全部后台任务（新→旧）
pub fn list_jobs(conversation_id: &str) -> Vec<JobInfo> {
    let map = match jobs().lock() {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<JobInfo> = map
        .iter()
        .filter(|(_, j)| {
            j.lock()
                .map(|j| j.conversation_id == conversation_id)
                .unwrap_or(false)
        })
        .map(|(id, j)| {
            let j = match j.lock() {
                Ok(j) => j,
                Err(_) => return None,
            };
        Some(JobInfo {
                job_id: id.clone(),
                command: j.command.clone(),
                cwd: j.cwd.display().to_string(),
                finished: j.status == JobStatus::Finished,
                status: j.status.as_str().to_string(),
                ok: j.ok,
                summary: j.summary.clone(),
                output_len: j.output.len(),
            })
        })
        .flatten()
        .collect();
    out.sort_by(|a, b| b.job_id.cmp(&a.job_id));
    out
}

/// 查询任务输出（尾部，已按缓冲上限裁剪）；任务不存在或不属于该会话返回错误
pub fn get_job_output(conversation_id: &str, job_id: &str) -> Result<String, String> {
    let job = find_job(conversation_id, job_id)?;
    let j = job.lock().map_err(|e| e.to_string())?;
    if j.output.is_empty() {
        Ok(match j.status {
            JobStatus::Finished => "（任务已结束，无输出）".to_string(),
            JobStatus::Stopping => "（任务正在停止…）".to_string(),
            JobStatus::Running => "（任务运行中，暂无输出）".to_string(),
        })
    } else {
        Ok(j.output.clone())
    }
}

/// 终止任务（强杀进程树）；幂等：已结束/停止中的任务直接返回对应状态
pub fn kill_job(conversation_id: &str, job_id: &str) -> Result<String, String> {
    let job = find_job(conversation_id, job_id)?;
    let (pid, status) = {
        let j = job.lock().map_err(|e| e.to_string())?;
        (j.pid, j.status)
    };
    match status {
        JobStatus::Finished => return Err("任务已结束，无需终止".into()),
        // 已在停止中：重复 kill 幂等返回，不重复强杀
        JobStatus::Stopping => return Ok("任务正在停止中".into()),
        JobStatus::Running => {}
    }
    mark_stopping(&job);
    crate::utils::process::kill_tree(pid);
    // 立即收尾（run_job 的 wait 收尾幂等，不会覆盖本摘要）；kill 后残留输出
    // 仍会被行级收集任务追加，直到进程树退出
    finish_job(&job, false, "已被 job_kill 终止".into());
    Ok("任务已终止".into())
}

/// 按 id 定位任务并校验会话归属
fn find_job(conversation_id: &str, job_id: &str) -> Result<Arc<Mutex<Job>>, String> {
    let map = jobs().lock().map_err(|e| e.to_string())?;
    let job = map
        .get(job_id)
        .cloned()
        .ok_or_else(|| format!("任务不存在（{job_id}）：job_id 无效或已过期"))?;
    if job
        .lock()
        .map(|j| j.conversation_id != conversation_id)
        .unwrap_or(true)
    {
        return Err("任务不属于当前会话".into());
    }
    Ok(job)
}

/// 清理会话的全部后台任务（会话删除/重置时调用）：强杀未完成任务并移除记录
pub fn drop_conversation_jobs(conversation_id: &str) {
    if let Ok(mut map) = jobs().lock() {
        let to_kill: Vec<(String, Option<u32>)> = map
            .iter()
            .filter(|(_, j)| {
                j.lock()
                    .map(|j| j.conversation_id == conversation_id && j.status != JobStatus::Finished)
                    .unwrap_or(false)
            })
            .map(|(id, j)| (id.clone(), j.lock().map(|j| j.pid).unwrap_or(None)))
            .collect();
        for (_, pid) in &to_kill {
            crate::utils::process::kill_tree(*pid);
        }
        for (id, _) in to_kill {
            map.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_line_smart_utf8_and_gbk() {
        // UTF-8 中文行
        let mut bytes: &[u8] = "你好世界\n".as_bytes();
        let mut buf = Vec::new();
        assert_eq!(
            read_line_smart(&mut bytes, &mut buf).await.as_deref(),
            Some("你好世界")
        );
        assert!(read_line_smart(&mut bytes, &mut buf).await.is_none());
        // GBK 编码的“中文”（D6D0 CEC4）经 smart_decode 检测链正确还原
        let gbk: &[u8] = &[0xD6, 0xD0, 0xCE, 0xC4, b'\n'];
        let mut bytes = gbk;
        let mut buf = Vec::new();
        assert_eq!(read_line_smart(&mut bytes, &mut buf).await.as_deref(), Some("中文"));
        // 无换行尾的末行也能读出
        let mut bytes: &[u8] = "done".as_bytes();
        let mut buf = Vec::new();
        assert_eq!(read_line_smart(&mut bytes, &mut buf).await.as_deref(), Some("done"));
    }

    #[test]
    fn output_cap_trims_head_keeps_tail() {
        let job = Arc::new(Mutex::new(Job {
            conversation_id: "c".into(),
            command: "x".into(),
            cwd: PathBuf::from("."),
            pid: None,
            output: String::new(),
            status: JobStatus::Running,
            ok: false,
            summary: None,
        }));
        // 先写头部块再写超大尾部块：裁剪后应只保留尾部（头部全部丢弃）
        let head = "b".repeat(JOB_OUTPUT_CAP + 100);
        let tail = "a".repeat(JOB_OUTPUT_CAP * 2);
        append_output(&job, &head);
        append_output(&job, &tail);
        let j = job.lock().unwrap();
        assert!(j.output.len() <= JOB_OUTPUT_CAP + 2, "len={}", j.output.len());
        assert!(j.output.ends_with('\n'), "保留的应是尾部内容（含最后追加的换行）");
        assert!(!j.output.contains('b'), "被裁剪的头部不应残留");
        assert!(j.output.starts_with('a'), "保留的应是后写入的尾部块");
    }

    #[test]
    fn finish_is_idempotent_and_status_transitions() {
        let job = Arc::new(Mutex::new(Job {
            conversation_id: "c".into(),
            command: "x".into(),
            cwd: PathBuf::from("."),
            pid: None,
            output: String::new(),
            status: JobStatus::Running,
            ok: false,
            summary: None,
        }));
        assert_eq!(job.lock().unwrap().status.as_str(), "running");
        // kill 先置 stopping：job_list 立即反映终止意图，且收尾前状态可查
        mark_stopping(&job);
        assert_eq!(job.lock().unwrap().status.as_str(), "stopping");
        // finish 幂等：kill 已写摘要后，收尾竞争写入不得覆盖
        finish_job(&job, false, "已被 job_kill 终止".into());
        finish_job(&job, true, "命令执行成功".into());
        let j = job.lock().unwrap();
        assert_eq!(j.status.as_str(), "finished");
        assert!(!j.ok, "幂等：已结束任务的 ok 不被后续覆盖");
        assert_eq!(j.summary.as_deref(), Some("已被 job_kill 终止"));
    }
}
