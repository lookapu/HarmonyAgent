//! 终端：系统终端（可见 cmd 窗口）+ 内置终端（应用内命令执行面板）。
//!
//! - `open_terminal`：打开系统终端窗口（cmd /K），与 Agent 的 run_command（静默执行）不同，
//!   这里是交互式窗口，刻意不设置 CREATE_NO_WINDOW。
//! - `terminal_exec/kill/status`：WebView 内置终端，每项目独立会话（维护当前目录 + 运行中子进程），
//!   支持 cd / && / | 等 cmd 语法与 GBK 输出解码；交互式程序（top/vim 等）不支持。

use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// 在项目根目录打开系统终端窗口（Windows 为 cmd.exe /K，以项目目录为启动目录）。
#[tauri::command]
pub fn open_terminal(project_path: String) -> Result<(), String> {
    let path = project_path.trim().to_string();
    if path.is_empty() {
        return Err("未指定项目目录".into());
    }
    let dir = Path::new(&path);
    if !dir.is_dir() {
        return Err(format!("项目目录不存在：{path}"));
    }
    #[cfg(windows)]
    {
        // 窗口标题取项目目录名，便于多项目多窗口区分
        let title = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "DevEco Switch".to_string());
        // 直接以项目根目录作为 cmd 的启动目录（current_dir），不再拼接 `cd /d`：
        // 之前用 arg 传 `cd /d \"path\"`，Rust 会把参数内引号转义成 \"，cmd 不识别，
        // cd 失败后提示符停留在应用自身目录（target\release）。
        std::process::Command::new("cmd.exe")
            .arg("/K")
            .arg(format!("title {}", title))
            .current_dir(dir)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("打开终端失败：{e}"))
    }
    #[cfg(not(windows))]
    {
        // 非 Windows：尝试常见终端模拟器（在项目目录下启动）
        for t in [
            "x-terminal-emulator",
            "gnome-terminal",
            "konsole",
            "xfce4-terminal",
            "xterm",
        ] {
            if std::process::Command::new(t).current_dir(dir).spawn().is_ok() {
                return Ok(());
            }
        }
        Err("未找到可用的终端模拟器".into())
    }
}

// ---------- 内置终端（应用内命令执行面板） ----------

/// 每项目终端会话：当前目录 + 运行中命令的 pid（供停止按钮强杀进程树）。
/// 子进程本体留在 terminal_exec 内 wait（两处持有所有权会冲突）。
struct TermSession {
    cwd: PathBuf,
    child_pid: Option<u32>,
}

fn sessions() -> &'static Mutex<HashMap<String, TermSession>> {
    static S: std::sync::OnceLock<Mutex<HashMap<String, TermSession>>> = std::sync::OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 终端执行结果
#[derive(Debug, Serialize, Clone)]
pub struct TermResult {
    /// 命令输出（stdout+stderr 合并，GBK/UTF-8 已解码，尾部截断保护）
    pub output: String,
    /// 执行后当前目录
    pub cwd: String,
    /// 命令是否仍在运行（正常返回时恒为 false）
    pub running: bool,
    /// 退出码（超时被终止时为 None）
    pub exit_code: Option<i32>,
    /// 是否超时被终止
    pub timed_out: bool,
}

/// 在内置终端执行一条命令（初始 cwd 为项目根，cd 命令更新会话目录）。
/// 会话串行：上一条命令未结束时新命令直接报错（先点停止）。
#[tauri::command]
pub async fn terminal_exec(
    project_id: String,
    project_path: String,
    command: String,
) -> Result<TermResult, String> {
    let cmd = command.trim().to_string();
    if cmd.is_empty() {
        return Err("命令不能为空".into());
    }
    // 取/建会话（锁在 block 内释放：std MutexGuard 非 Send，不能跨 await）
    let (mut child, stdout, stderr) = {
        let mut map = sessions().lock().unwrap();
        let entry = map.entry(project_id.clone()).or_insert_with(|| TermSession {
            cwd: PathBuf::from(&project_path),
            child_pid: None,
        });
        // cd 命令：本地更新会话目录，不启动子进程（cmd /C 里 cd 只影响该子进程，无法跨命令保持）
        if let Some(target) = cd_target(&cmd, &entry.cwd) {
            if target.is_dir() {
                entry.cwd = target;
                return Ok(TermResult {
                    output: String::new(),
                    cwd: entry.cwd.to_string_lossy().to_string(),
                    running: false,
                    exit_code: Some(0),
                    timed_out: false,
                });
            }
            return Err(format!("目录不存在：{}", target.display()));
        }
        if entry.child_pid.is_some() {
            return Err("上一条命令仍在运行，请先停止".into());
        }
        // 启动命令：Child 本体留在本函数 wait，会话只记 pid（供 terminal_kill 强杀）
        let mut child = build_term_child(&cmd, &entry.cwd)?;
        let pid = child.id().ok_or("无法获取进程 id")?;
        // 先取走管道（读输出用）
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        entry.child_pid = Some(pid);
        (child, stdout, stderr)
    };

    // 读输出（stdout/stderr 串行行级读取，smart_decode 处理 GBK）；超 3000 行丢头部防内存膨胀
    let mut lines: Vec<String> = Vec::new();
    if let Some(pipe) = stdout {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut reader = BufReader::new(pipe);
        let mut buf: Vec<u8> = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    while matches!(buf.last(), Some(b'\n') | Some(b'\r')) {
                        buf.pop();
                    }
                    lines.push(crate::agent::tools::smart_decode(&buf));
                    if lines.len() > 3000 {
                        lines.drain(..500);
                    }
                }
            }
        }
    }
    if let Some(pipe) = stderr {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut reader = BufReader::new(pipe);
        let mut buf: Vec<u8> = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    while matches!(buf.last(), Some(b'\n') | Some(b'\r')) {
                        buf.pop();
                    }
                    lines.push(crate::agent::tools::smart_decode(&buf));
                    if lines.len() > 3000 {
                        lines.drain(..500);
                    }
                }
            }
        }
    }
    // 等待结束（900s 超时后强杀进程树）
    let wait = tokio::time::timeout(Duration::from_secs(900), child.wait()).await;
    let (exit_code, timed_out) = match wait {
        Ok(Ok(st)) => (st.code(), false),
        Ok(Err(e)) => return Err(format!("等待命令结束失败: {e}")),
        Err(_) => {
            crate::utils::process::kill_tree(child.id());
            let _ = child.wait().await;
            (None, true)
        }
    };
    // 清会话运行状态
    let cwd_now = {
        let mut map = sessions().lock().unwrap();
        map.get_mut(&project_id).map(|s| {
            s.child_pid = None;
            s.cwd.to_string_lossy().to_string()
        })
    }
    .unwrap_or_else(|| Path::new(&project_path).to_string_lossy().to_string());

    let output = lines.join("\n");
    Ok(TermResult {
        output,
        cwd: cwd_now,
        running: false,
        exit_code,
        timed_out,
    })
}

/// 停止当前正在运行的终端命令（强杀进程树）。
#[tauri::command]
pub async fn terminal_kill(project_id: String) -> Result<(), String> {
    let pid = sessions()
        .lock()
        .unwrap()
        .get_mut(&project_id)
        .and_then(|s| s.child_pid.take());
    if let Some(pid) = pid {
        crate::utils::process::kill_tree(Some(pid));
    }
    Ok(())
}

/// 终端会话状态（当前目录 + 是否有命令在运行）。
#[derive(Debug, Serialize, Clone)]
pub struct TermStatus {
    pub cwd: String,
    pub running: bool,
}

#[tauri::command]
pub fn terminal_status(project_id: String, project_path: String) -> Result<TermStatus, String> {
    let mut map = sessions().lock().unwrap();
    let e = map.entry(project_id).or_insert_with(|| TermSession {
        cwd: PathBuf::from(&project_path),
        child_pid: None,
    });
    Ok(TermStatus {
        cwd: e.cwd.to_string_lossy().to_string(),
        running: e.child_pid.is_some(),
    })
}

/// 命令是否为仅 cd（返回目标目录；非 cd 命令返回 None）。
/// 支持：`cd`（回用户主目录）、`cd ..`/`cd..`、`cd /d X`、`cd X`（相对当前目录）。
fn cd_target(cmd: &str, cwd: &Path) -> Option<PathBuf> {
    let t = cmd.trim();
    let lower = t.to_lowercase();
    if lower == "cd" {
        return Some(home_or(cwd));
    }
    if lower == "cd.." || lower == "cd .." {
        return cwd.parent().map(|p| p.to_path_buf());
    }
    let rest = if lower.starts_with("cd /d ") {
        Some(&t[6..])
    } else if lower.starts_with("cd ") {
        Some(&t[3..])
    } else {
        None
    }?;
    let rest = rest.trim().trim_matches('"');
    if rest.is_empty() {
        return Some(home_or(cwd));
    }
    let target = if Path::new(rest).is_absolute() {
        PathBuf::from(rest)
    } else {
        cwd.join(rest)
    };
    Some(target)
}

fn home_or(cwd: &Path) -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.to_path_buf())
}

/// 构造系统 shell 子进程：隐藏窗口（Windows）+ 当前目录 + 注入鸿蒙工具链 PATH 与 JDK 环境
/// （与 process::command 对齐：hdc/ohpm 未进系统 PATH 时终端里也能直接用）。
fn build_term_child(command: &str, cwd: &Path) -> Result<tokio::process::Child, String> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = tokio::process::Command::new("cmd.exe");
        c.creation_flags(crate::utils::process::CREATE_NO_WINDOW);
        // raw_arg 原样传参：Rust 默认 arg 会把内部引号转义成 \"，cmd 不识别
        c.raw_arg(format!("/C {command}"));
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = tokio::process::Command::new("/bin/sh");
        c.arg("-c").arg(command);
        c
    };
    cmd.current_dir(cwd);
    // 鸿蒙工具链目录前置到 PATH，再叠加系统 PATH
    let mut paths = crate::utils::process::extra_path_dirs();
    if let Some(p) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&p));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env("PATH", joined);
    }
    // JDK 环境（绿色版捆绑 JDK 时注入，与 process::command 一致）
    for (k, v) in crate::utils::process::jdk_env_overrides() {
        cmd.env(k, v);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.spawn().map_err(|e| format!("无法启动命令: {e}"))
}
