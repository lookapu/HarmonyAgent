//! 命令执行 + 代码搜索 + HTTP 域工具：run_command / check_code / codebase_search / http_request / device_perf 等。
//! 共享辅助函数（run_cmd / run_in_project / smart_decode / truncate_out / scan_root / stamps 系列 等）仍定义在父模块 mod.rs，
//! 本模块通过 `use super::*` 继承访问。

use super::*;

/// 命令执行请求（宽松）：字段均可选；默认值（超时/工作目录）与校验集中在 `resolve()` 显式落地。
#[derive(serde::Deserialize, Default)]
pub(super) struct CommandRequest {
    /// 要执行的命令（resolve 校验非空）
    pub command: Option<String>,
    /// 超时秒数（钳制到 1..=300，缺省 60）
    pub timeout: Option<u64>,
    /// 工作目录（相对工程根或绝对路径，缺省工程根）
    pub cwd: Option<String>,
    /// 后台执行：立即返回 job_id，任务完成时结果自动反馈（缺省 false）
    pub run_in_background: Option<bool>,
}

impl CommandRequest {
    /// 从工具入参解析宽松请求：容忍未知字段与缺省字段。
    pub(super) fn from_args(args: &Value) -> Result<Self, String> {
        serde_json::from_value(args.clone()).map_err(|e| format!("run_command 参数解析失败：{e}"))
    }

    /// 显式 resolve：命令非空/危险黑名单/白名单校验、超时钳制、cwd 路径归一化，产出严格规范。
    pub(super) fn resolve(self, roots: &[String]) -> Result<CommandSpec, String> {
        let command = self.command.as_deref().map(str::trim).unwrap_or("");
        if command.is_empty() {
            return Err("run_command 需要参数 {\"command\":\"<命令>\"}".into());
        }
        if is_dangerous_command(command) {
            return Err("命令被安全策略拒绝（危险命令黑名单）：删除/格式化/系统级操作禁止执行，请改用 write_file/edit_file 或 git 工具".into());
        }
        // 命令白名单不再在此硬拦截：未在白名单的命令交由审批层（pre_approval 钩子）按权限模式裁决——
        // allow_all（默认）直接放行；ask/auto 模式弹窗确认（command_level 判为 L2）；first_write 非写工具放行。
        // 此处仅保留危险命令黑名单（rm -rf / format 等系统级破坏操作，任何权限模式都拦截的安全底线）。
        let timeout = self.timeout.unwrap_or(60).clamp(1, 300);
        let cwd_raw = self.cwd.as_deref().unwrap_or(".");
        let cwd = resolve_in_roots(roots, cwd_raw)?;
        if !cwd.is_dir() {
            return Err(format!("工作目录不是目录: {}", cwd.display()));
        }
        Ok(CommandSpec {
            command: command.to_string(),
            timeout,
            cwd,
            run_in_background: self.run_in_background.unwrap_or(false),
        })
    }
}

/// 命令执行规范（严格）：由 `CommandRequest::resolve()` 产出，校验与默认值已完成。
pub(super) struct CommandSpec {
    /// 已校验的命令文本
    pub command: String,
    /// 已钳制的超时秒数
    pub timeout: u64,
    /// 已归一化的工作目录
    pub cwd: std::path::PathBuf,
    /// 后台执行模式（任务 id 管理）
    pub run_in_background: bool,
}

/// 危险命令黑名单（run_command 拦截，任何权限模式均生效的安全底线）
pub(super) fn is_dangerous_command(cmd: &str) -> bool {
    let c = cmd.trim().to_lowercase();
    const DANGEROUS: [&str; 17] = [
        "format ", "format.", "format.com", "shutdown", "diskpart", "mkfs",
        "dd if=", "rd /s", "rmdir /s", "rm -rf", "rm -fr", "del /s", "del /f",
        "reg delete", "cipher /w", "net user", "bcdedit",
    ];
    DANGEROUS.iter().any(|d| c.contains(d))
}

/// 简易命令解析：按空白拆分，保留双引号内的空格（"C:\Program Files\x.exe"）
pub(super) fn split_command(line: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for ch in line.chars() {
        match ch {
            '"' => in_q = !in_q,
            ' ' if !in_q => {
                if !cur.is_empty() {
                    parts.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

/// 解析命令为 (program, args, envs)：
/// - Windows 下 .bat/.cmd 脚本必须经 cmd /C 执行（CreateProcess 无法直接运行批处理，
///   直接 spawn 会“退出码 1 无输出”）；整条命令交给 cmd 以保留参数语义；
/// - 批处理若依赖 node（hvigorw.bat/ohpm 等）且 PATH 无 node，自动注入 DevEco 内置 node。
fn resolve_program(command: &str, cwd: &Path) -> (String, Vec<String>, Option<Vec<(String, String)>>) {
    let parts = split_command(command);
    let (prog, cargs) = (parts[0].clone(), parts[1..].to_vec());
    let local = cwd.join(&prog);
    let program = if local.is_file() {
        local.to_string_lossy().to_string()
    } else {
        prog
    };
    #[cfg(windows)]
    if program.to_lowercase().ends_with(".bat") || program.to_lowercase().ends_with(".cmd") {
        return (
            "cmd".to_string(),
            vec!["/C".to_string(), command.to_string()],
            deveco_node_env(),
        );
    }
    (program, cargs, None)
}

/// PATH 中无 node 且探测到 DevEco Studio 内置 node 时，返回注入其目录的 PATH（供 .bat 脚本链）。
#[cfg(windows)]
fn deveco_node_env() -> Option<Vec<(String, String)>> {
    let path_has_node = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("node.exe").is_file()))
        .unwrap_or(false);
    if path_has_node {
        return None;
    }
    const CANDIDATES: [&str; 2] = [
        r"C:\Program Files\Huawei\DevEco Studio\tools\node",
        r"C:\Program Files\Huawei\DevEco Studio\sdk\default\openharmony\toolchains\node",
    ];
    for cand in CANDIDATES {
        let p = std::path::Path::new(cand);
        if p.join("node.exe").is_file() {
            let mut dirs = vec![p.to_path_buf()];
            if let Some(cur) = std::env::var_os("PATH") {
                dirs.extend(std::env::split_paths(&cur));
            }
            if let Ok(joined) = std::env::join_paths(dirs) {
                return Some(vec![("PATH".to_string(), joined.to_string_lossy().to_string())]);
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn deveco_node_env() -> Option<Vec<(String, String)>> {
    None
}

/// run_command：在项目内静默执行命令（危险命令黑名单 + 超时 + 输出截断）
/// 把流式命令的完整输出转成 run_cmd 语义的文本（退出码判断 + 字符上限截断），
/// 供 run_command 在流式执行（agent:log 实时推送）后保持原有结果格式
pub(super) fn cmd_output_text(o: &std::process::Output, max_chars: usize) -> Result<String, String> {
    let mut text = smart_decode(&o.stdout).trim().to_string();
    let err = smart_decode(&o.stderr).trim().to_string();
    if !err.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&err);
    }
    if text.chars().count() > max_chars {
        text = text.chars().take(max_chars).collect::<String>() + "\n…(输出已截断)";
    }
    if o.status.success() {
        Ok(if text.is_empty() { "命令执行成功".to_string() } else { text })
    } else {
        Err(format!(
            "命令退出码 {}：\n{}",
            o.status.code().unwrap_or(-1),
            if text.is_empty() { "无输出".to_string() } else { text }
        ))
    }
}

/// 命令失败信息增强：给 Agent 可操作线索，避免“退出码 1 无输出”这类零信息失败
/// （testhy 会话实证：hvigorw.bat 连续 3 次“退出码 1 无输出”，Agent 无法判断原因只能盲试）。
/// - 鸿蒙构建命令（hvigorw/hvigorw.bat）→ 提示改用 build_project 专用工具（node 直调更可靠）；
/// - 无输出失败 → 自动附带工程内最近构建日志尾部（.hvigor 下），失败也有据可查。
fn enrich_run_error(e: String, command: &str, cwd: &Path) -> String {
    let lower = command.to_lowercase();
    let mut extra = String::new();
    if lower.contains("hvigorw") {
        extra.push_str(
            "\n提示：鸿蒙工程构建请改用 build_project 专用工具（node 直调 hvigor-wrapper，比手动跑 hvigorw.bat 更可靠，能自动注入 SDK/node 环境并保存日志）；部署请用 deploy。",
        );
    }
    if e.contains("无输出") {
        if let Some(log) = latest_hvigor_log(cwd) {
            let tail = read_tail(&log, 1500);
            extra.push_str(&format!("\n（最近构建日志 {} 尾部：\n{}\n）", log.display(), tail));
        }
    }
    if extra.is_empty() {
        e
    } else {
        format!("{e}{extra}")
    }
}

/// 工程内最近修改的 hvigor 构建日志（.hvigor 下递归扫描 *.log，取 mtime 最新）
fn latest_hvigor_log(cwd: &Path) -> Option<std::path::PathBuf> {
    let hv = cwd.join(".hvigor");
    if !hv.is_dir() {
        return None;
    }
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    let mut stack = vec![hv.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|s| s.to_str()) == Some("log") {
                let mtime = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
                    best = Some((mtime, p));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// 读取文件尾部字符（按 UTF-8 字符计数）
fn read_tail(p: &Path, max_chars: usize) -> String {
    let Ok(bytes) = std::fs::read(p) else {
        return "(读取失败)".to_string();
    };
    let text = smart_decode(&bytes);
    let total = text.chars().count();
    if total <= max_chars {
        text
    } else {
        let skip = total - max_chars;
        let mut out: String = text.chars().skip(skip).collect();
        out.insert_str(0, &format!("…(前 {} 字符省略)\n", skip));
        out
    }
}

pub(super) async fn run_command(args: &Value, roots: &[String], ctx: &crate::agent::exec_ctx::ToolCtx) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法执行命令".into());
    }
    // Request/Spec 分离：宽松参数 CommandRequest → 显式 resolve() 产出严格规范 CommandSpec
    let spec = CommandRequest::from_args(args)?.resolve(roots)?;
    let command = spec.command.as_str();
    let timeout = spec.timeout;
    let cwd: &Path = &spec.cwd;
    // 全局并发护栏：与构建/部署互斥，避免并发写 build 目录
    let _gate = crate::services::tool_limits::acquire_gate("run_command").await;
    // 后台模式：解析为 (program, args) 后交给 jobs 托管进程生命周期，立即返回 job_id；
    // 任务完成时结果注入会话队列（模型下一轮请求自动看到），并可 job_output/job_kill 管理
    if spec.run_in_background {
        // 注：后台任务暂不注入环境变量（jobs 模块无 env 支持），.bat 经 cmd /C 执行即可
        let (program, args, _envs) = if needs_shell(&command) {
            #[cfg(windows)]
            { ("cmd".to_string(), vec!["/C".to_string(), command.to_string()], None) }
            #[cfg(not(windows))]
            { ("sh".to_string(), vec!["-c".to_string(), command.to_string()], None) }
        } else {
            resolve_program(&command, &cwd)
        };
        let job_id = crate::agent::jobs::start_background(
            program,
            args,
            command.to_string(),
            cwd.to_path_buf(),
            timeout,
            ctx,
        )?;
        return Ok(format!(
            "命令已在后台启动（任务 {job_id}）：{command}\n工作目录：{}\n超时：{timeout}s\n可调用 job_output 查询输出、job_kill 终止；任务完成时结果会自动反馈。",
            cwd.display()
        ));
    }
    // 间接修改追踪：记录命令开始时间，执行后扫描工作区内变更文件（排除构建产物目录）
    let cmd_start = std::time::SystemTime::now();
    // shell 语法（&&、||、引号外的 | > < &）经系统 shell 执行（Windows: cmd /C；
    // macOS/Linux: sh -c），对齐 ChatGPT 式整条命令；
    // 引号内的 | 等不算（如 rg -n 'a|b' 的正则竖线），保持单程序直接执行
    // 流式执行：stdout/stderr 逐行推送 agent:log（工具卡片/终端面板实时可见），
    // 同时支持“停止当前工具”中断；结果解析保持 run_cmd 语义（退出码/截断/建议）
    let result = if needs_shell(&command) {
        #[cfg(windows)]
        let shell_prog = "cmd";
        #[cfg(windows)]
        let shell_args = vec!["/C".to_string(), command.to_string()];
        #[cfg(not(windows))]
        let shell_prog = "sh";
        #[cfg(not(windows))]
        let shell_args = vec!["-c".to_string(), command.to_string()];
        let envs = deveco_node_env();
        crate::agent::exec_ctx::run_cmd_streaming_env(
            ctx, shell_prog, &shell_args, Some(&cwd), timeout, None, envs.as_deref(),
        )
        .await
        .and_then(|o| cmd_output_text(&o, 30000))
        .map_err(|e| with_advice("run_command", e))
    } else {
        // 工程内脚本（如 hvigorw.bat）优先本地路径解析；.bat/.cmd 经 cmd /C 执行（见 resolve_program）
        let (program, full_args, envs) = resolve_program(&command, &cwd);
        crate::agent::exec_ctx::run_cmd_streaming_env(
            ctx, &program, &full_args, Some(&cwd), timeout, None, envs.as_deref(),
        )
        .await
        .and_then(|o| cmd_output_text(&o, 30000))
        .map_err(|e| with_advice("run_command", e))
    };
    match result {
        Ok(out) => {
            // 扫描命令间接修改/创建的文件（写文件类命令也受文件列表追踪，与 edit_file/write_file 一致）。
            // 全项目递归遍历在 spawn_blocking 中执行，避免钉死 tokio worker。
            let roots_owned = roots.to_vec();
            let changed = tokio::task::spawn_blocking(move || {
                scan_recent_changes(&roots_owned, cmd_start, 200)
            })
            .await
            .unwrap_or_default();
            if changed.is_empty() {
                Ok(out)
            } else {
                let shown: Vec<&str> = changed.iter().take(15).map(String::as_str).collect();
                let extra = if changed.len() > 15 { "…" } else { "" };
                record_cmd_changes(&changed);
                Ok(format!(
                    "{out}\n\n（命令间接修改/创建了 {} 个文件：{}{extra}）",
                    changed.len(),
                    shown.join(", ")
                ))
            }
        }
        Err(e) => Err(enrich_run_error(e, command, cwd)),
    }
}

/// 检测命令是否含 shell 语法（&&、|| 及引号外的 |、>、<、&），需要经 cmd /C 执行。
/// 引号（" 和 '）内的字符不算，避免把正则竖线（如 rg -n 'a|b'）误判为管道
pub(super) fn needs_shell(command: &str) -> bool {
    let mut in_double = false;
    let mut in_single = false;
    for c in command.chars() {
        match c {
            '"' => in_double = !in_double,
            '\'' => in_single = !in_single,
            '&' | '|' | '>' | '<' if !in_double && !in_single => return true,
            _ => {}
        }
    }
    false
}

/// check_code：静态检查（规则式 lint）
pub(super) async fn check_code_tool(args: &Value, roots: &[String]) -> Result<String, String> {
    let root = scan_root(roots)?.to_path_buf();
    let path = args["path"].as_str().map(str::to_string);
    let kind = args["kind"].as_str().map(str::to_string);
    tokio::task::spawn_blocking(move || {
        crate::agent::scanner::check_code(&root, path.as_deref(), kind.as_deref())
            .map_err(|e| with_advice("check_code", e))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// deep_scan：深度扫描报告
pub(super) async fn deep_scan_tool(args: &Value, roots: &[String]) -> Result<String, String> {
    let root = scan_root(roots)?.to_path_buf();
    let path = args["path"].as_str().map(str::to_string);
    tokio::task::spawn_blocking(move || {
        crate::agent::scanner::deep_scan(&root, path.as_deref()).map_err(|e| with_advice("deep_scan", e))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// secret_scan：密钥泄露专项扫描（源码 + 配置文件）
pub(super) async fn secret_scan_tool(args: &Value, roots: &[String]) -> Result<String, String> {
    let root = scan_root(roots)?.to_path_buf();
    let path = args["path"].as_str().map(str::to_string);
    let include_config = args["include_config"].as_bool();
    tokio::task::spawn_blocking(move || {
        crate::agent::scanner::secret_scan(&root, path.as_deref(), include_config)
            .map_err(|e| with_advice("secret_scan", e))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// codebase_search：全库混合检索
pub(super) async fn codebase_search_tool(args: &Value, roots: &[String]) -> Result<String, String> {
    let root = scan_root(roots)?.to_path_buf();
    let query = args["query"].as_str().ok_or("codebase_search 需要参数 {\"query\":\"<查询词>\"}")?.trim().to_string();
    if query.is_empty() {
        return Err("codebase_search 需要参数 {\"query\":\"<查询词>\"}".into());
    }
    let limit = args["limit"].as_u64().unwrap_or(10).clamp(1, 30) as usize;
    tokio::task::spawn_blocking(move || {
        crate::agent::scanner::codebase_search(&root, &query, limit)
            .map_err(|e| with_advice("codebase_search", e))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// get_symbol_details：符号详情 + 引用反查
pub(super) async fn get_symbol_details_tool(args: &Value, roots: &[String]) -> Result<String, String> {
    let root = scan_root(roots)?.to_path_buf();
    let name = args["name"].as_str().ok_or("get_symbol_details 需要参数 {\"name\":\"<符号名>\"}")?.trim().to_string();
    if name.is_empty() {
        return Err("get_symbol_details 需要参数 {\"name\":\"<符号名>\"}".into());
    }
    let file = args["file"].as_str().map(str::to_string);
    tokio::task::spawn_blocking(move || {
        crate::agent::scanner::symbol_details(&root, &name, file.as_deref())
            .map_err(|e| with_advice("get_symbol_details", e))
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------- 后台任务管理（jobs） ----------

/// job_list：列出本会话全部后台任务（含状态/退出码/输出大小）
pub(super) fn job_list_tool(_args: &Value, conversation_id: &str) -> Result<String, String> {
    let jobs = crate::agent::jobs::list_jobs(conversation_id);
    if jobs.is_empty() {
        return Ok("本会话暂无后台任务（可用 run_command 的 run_in_background:true 启动）。".into());
    }
    let mut out = format!("后台任务（{} 个）：\n", jobs.len());
    for j in &jobs {
        let status = match j.status.as_str() {
            "stopping" => "⏹ 停止中",
            "finished" => {
                if j.ok { "✓ 完成" } else { "✗ 失败" }
            }
            _ => "⏳ 运行中",
        };
        out.push_str(&format!(
            "- [{status}] {} | 输出 {} 字符 | 命令：{}\n",
            j.job_id,
            j.output_len,
            j.command.replace('\n', " ")
        ));
        if let Some(s) = &j.summary {
            out.push_str(&format!("    结果：{s}\n"));
        }
    }
    Ok(cut_str(&out, 4000))
}

/// job_output：查询后台任务输出（尾部，按缓冲上限裁剪）
pub(super) fn job_output_tool(args: &Value, conversation_id: &str) -> Result<String, String> {
    let job_id = args["job_id"].as_str().ok_or("job_output 需要参数 {\"job_id\":\"<任务 id>\"}")?.trim();
    if job_id.is_empty() {
        return Err("job_output 需要参数 {\"job_id\":\"<任务 id>\"}".into());
    }
    let out = crate::agent::jobs::get_job_output(conversation_id, job_id)?;
    Ok(format!("[任务 {job_id} 输出]\n{}", cut_str(&out, 6000)))
}

/// job_kill：终止后台任务（强杀进程树）
pub(super) fn job_kill_tool(args: &Value, conversation_id: &str) -> Result<String, String> {
    let job_id = args["job_id"].as_str().ok_or("job_kill 需要参数 {\"job_id\":\"<任务 id>\"}")?.trim();
    if job_id.is_empty() {
        return Err("job_kill 需要参数 {\"job_id\":\"<任务 id>\"}".into());
    }
    crate::agent::jobs::kill_job(conversation_id, job_id)?;
    Ok(format!("任务 {job_id} 已终止"))
}

// ---------- 截图视觉闭环 ----------

/// 截图 → 多模态内联 data URL：长边缩到 1568（保留界面文字可读性）、JPEG 质量 90。
/// 供 Agent 在 take_screenshot 后“看到”真机界面；失败仅返回错误（不影响截图本身成功）。
pub(crate) fn encode_vision_image(path: &std::path::Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取截图失败: {e}"))?;
    let img = image::load_from_memory(&bytes).map_err(|e| format!("截图解码失败: {e}"))?;
    let (w, h) = (img.width(), img.height());
    const MAX_EDGE: u32 = 1568;
    let thumb = if w.max(h) > MAX_EDGE {
        let nw = if w >= h { MAX_EDGE } else { (w * MAX_EDGE) / h.max(1) };
        let nh = if h >= w { MAX_EDGE } else { (h * MAX_EDGE) / w.max(1) };
        img.thumbnail(nw.max(1), nh.max(1))
    } else {
        img
    };
    let mut buf: Vec<u8> = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90);
    enc.encode_image(&thumb).map_err(|e| format!("截图压缩失败: {e}"))?;
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
    Ok(format!("data:image/jpeg;base64,{b64}"))
}

// ---------- HTTP 联调 ----------

/// http_request：通用 HTTP 客户端（接口联调），GET/POST/PUT/DELETE + 自定义头 + JSON 文本体。
/// 响应自动识别编码（BOM > header charset > UTF-8 严格验证 > GBK 回退），中文接口不乱码。
pub(super) async fn http_request(args: &Value) -> Result<String, String> {
    let url = args["url"].as_str().ok_or(
        "http_request 需要参数 {\"url\":\"<http(s)://…>\",\"method\":\"<GET|POST|PUT|DELETE，缺省 GET>\",\"body\":\"<可选请求体>\",\"headers\":{<可选请求头>},\"timeout_secs\":<可选超时秒，缺省 30>}",
    )?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("仅支持 http/https 地址".into());
    }
    let method = args["method"].as_str().unwrap_or("GET").to_uppercase();
    let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(30).clamp(1, 120);
    let client = crate::utils::net::build_client_auto().map_err(|e| format!("网络初始化失败: {e}"))?;
    let mut req = match method.as_str() {
        "GET" => client.get(url),
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "DELETE" => client.delete(url),
        other => return Err(format!("不支持的方法: {other}（仅 GET/POST/PUT/DELETE）")),
    };
    if let Some(hs) = args["headers"].as_object() {
        for (k, v) in hs {
            if let Some(sv) = v.as_str() {
                req = req.header(k, sv);
            }
        }
    }
    if let Some(body) = args["body"].as_str() {
        if !body.is_empty() {
            req = req.header(
                "Content-Type",
                args["content_type"].as_str().unwrap_or("application/json"),
            );
            req = req.body(body.to_string());
        }
    }
    let t0 = std::time::Instant::now();
    let resp = tokio::time::timeout(Duration::from_secs(timeout_secs), req.send())
        .await
        .map_err(|_| format!("请求超时（>{timeout_secs}s）"))?
        .map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = tokio::time::timeout(Duration::from_secs(timeout_secs), resp.bytes())
        .await
        .map_err(|_| "读取响应超时".to_string())?
        .map_err(|e| format!("读取响应失败: {e}"))?;
    if bytes.len() > 1024 * 1024 {
        return Err("响应超过 1MB，请缩小请求范围（加查询参数/分页）".into());
    }
    let text = decode_response(&bytes, &charset_from_content_type(&content_type));
    let elapsed = t0.elapsed().as_millis();
    Ok(format!(
        "HTTP {}（{url}，耗时 {elapsed}ms）\nContent-Type: {}\n\n{}",
        status.as_u16(),
        if content_type.is_empty() { "未知".to_string() } else { content_type },
        truncate_chars(&text, 6000)
    ))
}

/// 从 Content-Type 提取 charset 标签（如 text/html; charset=gbk）
pub(super) fn charset_from_content_type(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|part| {
        let kv: Vec<&str> = part.splitn(2, '=').collect();
        if kv.len() == 2 && kv[0].trim().eq_ignore_ascii_case("charset") {
            Some(kv[1].trim().trim_matches('"').to_string())
        } else {
            None
        }
    })
}

/// 响应字节 → 文本：声明 charset（encoding_rs 标签，含 gbk/gb2312/gb18030）优先，
/// 无声明走 smart_decode（BOM/UTF-8/GBK 检测链）
pub(super) fn decode_response(bytes: &[u8], charset: &Option<String>) -> String {
    if let Some(cs) = charset {
        if let Some(enc) = encoding_rs::Encoding::for_label(cs.as_bytes()) {
            let (text, _, _) = enc.decode(bytes);
            return text.into_owned();
        }
    }
    smart_decode(bytes)
}

pub(super) fn utf16_lossy(bytes: &[u8], little: bool) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| {
            if little {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

// ---------- 批量编辑 ----------

/// multi_edit：一次调用批量修改多个文件（逐项独立执行，失败不影响后续项，返回逐项汇总）
// ---------- 真机性能采样 ----------

/// device_perf：真机性能快照（CPU/内存/电量/温度），供卡顿/资源占用分析
pub(super) async fn device_perf(args: &Value) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let perf = crate::commands::devices::get_device_perf(device.clone()).await?;
    let fmt = |v: f64, unit: &str| {
        if v < 0.0 {
            "不可用".to_string()
        } else {
            format!("{v:.1}{unit}")
        }
    };
    let time = chrono::DateTime::from_timestamp_millis(perf.ts)
        .map(|t| t.format("%H:%M:%S").to_string())
        .unwrap_or_default();
    Ok(format!(
        "设备 {device} 性能快照（{time}）：\nCPU 占用：{}\n内存占用：{}\n电池电量：{}\n温度：{}",
        fmt(perf.cpu, "%"),
        fmt(perf.mem, "%"),
        fmt(perf.battery, "%"),
        fmt(perf.temp, "℃")
    ))
}

/// get_env_info：开发环境探测（SDK / command-line-tools / 工具链版本）
pub(super) async fn get_env_info() -> Result<String, String> {
    let env = crate::services::harmony_env::detect_auto();
    let mut out = String::new();
    out.push_str("开发环境探测：\n");
    // HarmonyOS SDK
    match &env.sdk_root {
        Some(r) => {
            out.push_str(&format!("HarmonyOS SDK: {r}\n"));
            if let Some(api) = &env.default_api {
                out.push_str(&format!("默认 API 版本: {api}\n"));
            }
            if !env.sdk_versions.is_empty() {
                out.push_str(&format!("已安装 API 版本: {}\n", env.sdk_versions.join(", ")));
            }
        }
        None => out.push_str("HarmonyOS SDK: 未发现\n"),
    }
    match &env.cli {
        Some(cli) => {
            out.push_str(&format!("command-line-tools: {}\n", cli.root));
            out.push_str(&format!(
                "  hdc:{} ohpm:{} hvigorw:{}\n",
                if cli.has_hdc { "✓" } else { "✗" },
                if cli.has_ohpm { "✓" } else { "✗" },
                if cli.has_hvigorw { "✓" } else { "✗" },
            ));
        }
        None => out.push_str("command-line-tools: 未发现（hdc/ohpm 不可用）\n"),
    }
    match &env.studio_dir {
        Some(d) => out.push_str(&format!("DevEco Studio: {d}\n")),
        None => out.push_str("DevEco Studio: 未发现\n"),
    }
    // 通用工具链版本探测（短超时，失败不阻塞）
    out.push_str("\n工具链版本：\n");
    let probes: [(&str, &[&str]); 6] = [
        ("node", &["--version"]),
        ("git", &["--version"]),
        ("cargo", &["--version"]),
        ("java", &["-version"]),
        ("python", &["--version"]),
        ("ohpm", &["--version"]),
    ];
    for (prog, ver_args) in probes {
        let vargs: Vec<String> = ver_args.iter().map(|s| s.to_string()).collect();
        match run_cmd(prog, &vargs, None, 10).await {
            Ok(v) => out.push_str(&format!("  {prog}: {}\n", v.lines().next().unwrap_or("").trim())),
            Err(_) => out.push_str(&format!("  {prog}: 不可用\n")),
        }
    }
    if !env.suggestions.is_empty() {
        out.push_str(&format!("\n建议检查: {}\n", env.suggestions.join("; ")));
    }
    Ok(cut_str(&out, 4000))
}

/// list_agents：子 Agent 运行记录
pub(super) fn list_agents_tool() -> Result<String, String> {
    let recs = crate::agent::subagents::snapshot();
    if recs.is_empty() {
        return Ok("尚无子 Agent 运行记录（本会话未使用 spawn_agents）".into());
    }
    let mut out = format!("子 Agent 运行记录（最近 {} 条，新→旧）：\n", recs.len());
    for r in recs {
        let status = match r.status.as_str() {
            "done" => "✓ 完成",
            "error" => "✗ 失败",
            "skipped" => "⏭ 跳过（用户停止）",
            _ => "? 未知",
        };
        out.push_str(&format!(
            "- [{}] {} | {} | {}ms\n",
            status, r.name, r.model, r.elapsed_ms
        ));
        if !r.output_tail.is_empty() {
            out.push_str(&format!("  摘要: {}\n", r.output_tail.replace('\n', " ")));
        }
    }
    Ok(cut_str(&out, 4000))
}

/// 文本截断（字符级，末尾加省略标记）
pub(super) fn cut_str(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let head: String = chars[..max].iter().collect();
    format!("{head}\n…（截断）")
}

