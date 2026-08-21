//! 日志/静态检查/网络/签名/电池/API 兼容域工具：search_hilog / run_lint / set_network_condition / check_signature / scan_api_compat 等。
//! 共享辅助函数（run_hdc_shell / default_device_id / truncate_out 等）仍定义在父模块 mod.rs，
//! 本模块通过 `use super::*` 继承访问。

use super::*;
/// search_hilog：在设备 hilog 中按条件搜索。
pub(super) async fn search_hilog(args: &Value, _roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let level = args["level"].as_str().unwrap_or("WARN").to_uppercase();
    let tag = args["tag"].as_str().unwrap_or("").to_string();
    let package = args["package"].as_str().unwrap_or("").to_string();
    let keyword = args["keyword"].as_str().unwrap_or("").to_string();
    let use_regex = args["regex"].as_bool().unwrap_or(false);
    let since_min = args["since"].as_u64().unwrap_or(5).min(60 * 24);
    // until：时间上限（分钟），只保留 N 分钟以前的日志；与 since 组合成 [since, until] 时间窗口（默认 0=无上限）
    let until_min = args["until"].as_u64().unwrap_or(0).min(60 * 24);
    let max_lines = args["max_lines"].as_u64().unwrap_or(200).min(2000) as usize;
    let context = args["context"].as_u64().unwrap_or(2).min(10) as usize;

    let level_flag = match level.as_str() {
        "DEBUG" | "D" => "D",
        "INFO" | "I" => "I",
        "WARN" | "W" => "W",
        "ERROR" | "E" => "E",
        "FATAL" | "F" => "F",
        _ => "W",
    };

    // hilog 参数语义（官方文档）：-x 读完退出；-z <n> 只输出缓冲区最后 n 行（最近的日志）；
    // -L <level> 级别过滤；-T <tag> tag 过滤；-e <expr> 正则过滤；-v epoch 行首输出 epoch 时间戳（便于按 since 过滤）。
    // 注意 -T 是 tag 不是时间，不能用它做时间过滤；时间过滤在本地用 epoch 时间戳完成。
    let tail_lines = (max_lines * 3 + 300).clamp(500, 5000);
    let mut shell_cmd: Vec<String> = vec![
        "hilog".into(), "-x".into(), "-z".into(), tail_lines.to_string(),
        "-v".into(), "epoch".into(), "-L".into(), level_flag.to_string(),
    ];
    if !tag.is_empty() {
        shell_cmd.push("-T".into());
        shell_cmd.push(tag.clone());
    }
    if use_regex && !keyword.is_empty() {
        shell_cmd.push("-e".into());
        shell_cmd.push(keyword.clone());
    }
    let mut full: Vec<String> = vec!["-t".into(), device.clone(), "shell".into()];
    full.extend(shell_cmd);
    // 日志输出远超 3000 字符，用大上限读取（设备端已限行数，内存可控）
    let out_raw = run_cmd_capped("hdc", &full, None, 20, 20_000).await.unwrap_or_default();

    let lines: Vec<&str> = out_raw.lines().collect();
    let mut matches: Vec<(usize, &str)> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let l = *line;
        // 时间过滤：-v epoch 时行首为 epoch 秒；解析失败（设备回退默认格式）保守保留
        if !log_line_recent_enough(l, since_min) {
            continue;
        }
        // 时间上限：until=N 只保留 N 分钟以前的日志（与 since 组合成时间窗口）
        if !log_line_older_than(l, until_min) {
            continue;
        }
        // 级别过滤（epoch 格式下级别仍在第 4 列，双保险，设备端 -L 已过滤）
        if !line_matches_level(l, level_flag) {
            continue;
        }
        // tag 兜底过滤：设备端 -T 已过滤，这里兼容不支持 -T 的老版本；
        // hilog 输出格式为 domain/tag:，tag 前是 / 不是空格
        if !tag.is_empty() && !l.contains(&format!("/{tag}")) {
            continue;
        }
        if !package.is_empty() && !l.contains(&package) {
            continue;
        }
        if !keyword.is_empty() {
            if use_regex {
                // 设备端 -e 已做正则过滤，这里再兜底一次包含匹配（老版本不支持 -e 时降级）
                if !l.contains(&keyword) {
                    continue;
                }
            } else if !l.to_lowercase().contains(&keyword.to_lowercase()) {
                continue;
            }
        }
        matches.push((i, l));
        if matches.len() >= max_lines {
            break;
        }
    }

    let window = if until_min > 0 {
        format!("，时间窗口 [now-{until_min}min, now-{since_min}min]")
    } else {
        format!("，最近 {since_min} 分钟")
    };
    let mut out = format!("hilog 搜索结果（设备 {device}，级别 ≥ {level_flag}{window}）\n");
    out.push_str(&format!("匹配到 {} 条（显示前 {max_lines} 条，上下文 ±{context} 行）\n\n", matches.len()));

    if matches.is_empty() {
        out.push_str("（没有匹配的日志）\n");
        out.push_str("提示：可放宽级别（如 level=DEBUG）、调整 since 时间范围、或换个关键词试试。");
        return Ok(out);
    }

    let mut last_end: isize = -1;
    for (idx, _line) in &matches {
        let start = (*idx as isize - context as isize).max(0) as usize;
        let end = (idx + context).min(lines.len().saturating_sub(1));
        if start as isize <= last_end && last_end >= 0 {
            // 与上一段重叠，不重复显示分隔符
        } else if last_end >= 0 {
            out.push_str("...\n");
        }
        for j in start..=end {
            let marker = if j == *idx { "▶" } else { " " };
            out.push_str(&format!("{marker} {}\n", lines[j]));
        }
        last_end = end as isize;
    }
    Ok(out)
}

pub(super) fn line_matches_level(line: &str, min_level: &str) -> bool {
    let levels = ["D", "I", "W", "E", "F"];
    let min_idx = levels.iter().position(|l| *l == min_level).unwrap_or(2);
    for l in line.split_whitespace().take(6) {
        if let Some(idx) = levels.iter().position(|x| *x == l) {
            return idx >= min_idx;
        }
    }
    // 无法解析级别时默认显示（保守策略）
    true
}

/// 解析行首 epoch 时间戳（秒；毫秒级自动折算）。解析失败返回 None（保守保留策略由调用方决定）。
fn parse_line_epoch_secs(line: &str) -> Option<u64> {
    let first = line.split_whitespace().next()?;
    let secs_str = first.split('.').next()?;
    if secs_str.len() < 10 || !secs_str.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut ts: u64 = secs_str.parse().ok()?;
    if secs_str.len() >= 13 {
        ts /= 1000; // 毫秒级时间戳
    }
    Some(ts)
}

fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(u64::MAX)
}

/// 按行首 epoch 时间戳过滤最近 since_min 分钟的日志。
/// -v epoch 输出行首为「秒[.毫秒]」；解析失败（设备回退默认 MM-DD 格式）或时间异常时保守保留。
pub(super) fn log_line_recent_enough(line: &str, since_min: u64) -> bool {
    let Some(ts) = parse_line_epoch_secs(line) else { return true };
    let now = now_epoch_secs();
    if ts == 0 || ts > now {
        return true; // 设备时钟异常或超前 → 保守保留
    }
    now.saturating_sub(ts) <= since_min.saturating_mul(60)
}

/// 时间上限过滤：until=N 分钟时只保留比 now-N 分钟更早的日志（与 since 组合成 [since, until] 窗口）；
/// until=0 表示无上限；解析失败保守保留。
pub(super) fn log_line_older_than(line: &str, until_min: u64) -> bool {
    if until_min == 0 {
        return true;
    }
    let Some(ts) = parse_line_epoch_secs(line) else { return true };
    let now = now_epoch_secs();
    ts <= now.saturating_sub(until_min.saturating_mul(60))
}

/// run_lint：运行 ArkTS 代码静态检查。
pub(super) async fn run_lint(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("").to_string();
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录，无法运行 Lint".into());
    }
    // 非鸿蒙工程提示：run_lint 为鸿蒙专用（hvigor lint / codelinter），避免模型盲目调用
    let lint_root = Path::new(&project_path);
    if !crate::services::workspace::classify(lint_root)
        .is_some_and(|k| k == crate::services::workspace::ModuleKind::Harmony)
    {
        return Err(format!(
            "目标目录不是 HarmonyOS 工程（{}），run_lint 仅支持鸿蒙（hvigor lint / codelinter）。\n其它语言工程请用 run_command 执行对应 lint 工具（如 npm run lint / go vet ./... / cargo clippy / ruff check .）。",
            lint_root.display()
        ));
    }
    let path = args["path"].as_str().unwrap_or("").to_string();
    let _rule_set = args["rule_set"].as_str().unwrap_or("");
    let severity = args["severity"].as_str().unwrap_or("").to_lowercase();

    let target_dir = if path.is_empty() {
        project_path.clone()
    } else {
        match resolve_in_roots(roots, &path) {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(e) => return Err(e),
        }
    };

    // 方案：先试 hvigor lint，再试 codelinter；都不可用时给出提示
    let hvigor_path = if cfg!(windows) {
        format!("{project_path}\\hvigorw.bat")
    } else {
        format!("{project_path}/hvigorw")
    };

    let mut success = false;
    let mut output = String::new();
    let mut tool_used = String::new();

    if std::path::Path::new(&hvigor_path).exists() {
        let cmd_args: Vec<String> = vec!["lint".to_string()];
        // cwd 指定工程目录：hvigorw 依赖当前目录定位工程配置，否则会用 App 安装目录导致失败
        let r = run_cmd_capped(&hvigor_path, &cmd_args, Some(Path::new(&project_path)), 120, 30_000).await;
        tool_used = "hvigor lint".to_string();
        match r {
            Ok(o) => { output = o; success = true; }
            Err(e) => output = format!("执行失败：{e}"),
        }
    }

    // 如果 hvigor 不行，试试 codelinter（通过 node 调用，工程有 code-linter.json5 时）
    if !success {
        let config_path = format!("{project_path}/code-linter.json5");
        if std::path::Path::new(&config_path).exists() {
            // 确保报告输出目录存在，否则 codelinter 写文件失败
            std::fs::create_dir_all(format!("{project_path}/.deveco-agent")).ok();
            let args: Vec<String> = vec![
                "-c".to_string(), config_path.clone(),
                "-s".to_string(), target_dir.clone(),
                "-o".to_string(), format!("{project_path}/.deveco-agent/lint-report.json"),
                "--no-color".to_string(),
            ];
            // lint 输出可能很长（几百条问题），用大上限避免问题列表被截断
            let r = run_cmd_capped("codelinter", &args, Some(Path::new(&project_path)), 120, 30_000).await;
            tool_used = "codelinter".to_string();
            match r {
                Ok(o) => { output = o; success = true; }
                Err(e) => output = format!("执行失败：{e}"),
            }
        }
    }

    if !success {
        return Err(format!(
            "运行 Lint 失败：未找到可用的 lint 工具。\n尝试：{tool_used}\n{output}\n\n提示：可安装 Code Linter 或在 DevEco Studio 中配置后重试。当前可先用 search_file / glob 手动扫描常见问题。"
        ));
    }

    // 解析输出，提取结构化问题
    let issues = parse_lint_output(&output, &severity);

    let mut out = format!("Lint 检查完成（工具：{tool_used}）\n");
    out.push_str(&format!("目标：{target_dir}\n"));
    out.push_str(&format!("共发现 {} 个问题\n", issues.len()));

    let errors: Vec<_> = issues.iter().filter(|i| i.severity == "error").collect();
    let warns: Vec<_> = issues.iter().filter(|i| i.severity == "warning").collect();
    let others: Vec<_> = issues.iter().filter(|i| i.severity != "error" && i.severity != "warning").collect();
    out.push_str(&format!("  错误 (error)：{}\n", errors.len()));
    out.push_str(&format!("  警告 (warn)：{}\n", warns.len()));
    out.push_str(&format!("  其他：{}\n\n", others.len()));

    let show_limit = 50;
    out.push_str(&format!("问题列表（前 {show_limit} 条）：\n"));
    for (i, issue) in issues.iter().take(show_limit).enumerate() {
        out.push_str(&format!(
            "  {:>3}. [{}] {}:{}  {}  ({})\n",
            i + 1, issue.severity, issue.file, issue.line, issue.message, issue.rule
        ));
    }
    if issues.len() > show_limit {
        out.push_str(&format!("  ... 还有 {} 条\n", issues.len() - show_limit));
    }
    out.push_str("\n下一步：Agent 可根据这些问题用 edit_file / multi_edit 批量修复代码，再重新 run_lint 验证。");
    Ok(out)
}

#[derive(Clone)]
pub(super) struct LintIssue {
    file: String,
    line: usize,
    severity: String,
    rule: String,
    message: String,
}

pub(super) fn parse_lint_output(output: &str, filter_severity: &str) -> Vec<LintIssue> {
    let mut issues: Vec<LintIssue> = Vec::new();
    for line in output.lines() {
        // 匹配常见 lint 格式：file:line:col severity rule - message
        // 或 file:line:col: severity: message
        let lower = line.to_lowercase();
        let severity = if lower.contains("error") { "error" }
            else if lower.contains("warning") || lower.contains("warn") { "warning" }
            else { continue };
        if !filter_severity.is_empty() && !severity.contains(filter_severity) {
            continue;
        }

        // 尝试提取 file:line
        let re_file = r"([A-Za-z0-9_./\\\-]+\.[et]s):(\d+)";
        if let Some((file, line_num)) = first_file_line(line, re_file) {
            // 提取消息
            let message = line.splitn(3, '-').nth(2).unwrap_or(line).trim().to_string();
            // 提取规则名（在括号里或前缀）
            let rule = extract_rule_name(line);
            issues.push(LintIssue {
                file,
                line: line_num,
                severity: severity.to_string(),
                rule,
                message,
            });
        }
    }
    issues
}

pub(super) fn first_file_line(text: &str, _pattern: &str) -> Option<(String, usize)> {
    // 简易：找 file.ext:line 的模式
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // 找 .ets: 或 .ts:
        if i + 4 < bytes.len() && bytes[i] == b'.' {
            if i + 5 < bytes.len() && &text[i..i+4] == ".ets" && bytes[i+4] == b':' {
                // 向前找文件名起点
                let mut start = i;
                while start > 0 && bytes[start-1] != b' ' && bytes[start-1] != b'\t' && bytes[start-1] != b'\n' {
                    start -= 1;
                }
                let file = text[start..i+4].to_string();
                // 读行号
                let mut j = i + 5;
                let line_start = j;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > line_start {
                    let line_num: usize = text[line_start..j].parse().unwrap_or(0);
                    return Some((file, line_num));
                }
            }
            if i + 4 <= bytes.len() && &text[i..i+3] == ".ts" && i+3 < bytes.len() && bytes[i+3] == b':' {
                let mut start = i;
                while start > 0 && bytes[start-1] != b' ' && bytes[start-1] != b'\t' && bytes[start-1] != b'\n' {
                    start -= 1;
                }
                let file = text[start..i+3].to_string();
                let mut j = i + 4;
                let line_start = j;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > line_start {
                    let line_num: usize = text[line_start..j].parse().unwrap_or(0);
                    return Some((file, line_num));
                }
            }
        }
        i += 1;
    }
    None
}

pub(super) fn extract_rule_name(line: &str) -> String {
    // 找括号里的规则名
    if let Some(start) = line.find('(') {
        if let Some(end) = line[start..].find(')') {
            return line[start+1..start+end].trim().to_string();
        }
    }
    // 找 @xxx/xxx 规则
    for part in line.split_whitespace() {
        if part.starts_with('@') && (part.contains('/') || part.contains('-')) {
            return part.trim_end_matches(':').to_string();
        }
    }
    "unknown".to_string()
}

/// set_network_condition：设置网络条件（弱网/延迟/丢包）。
pub(super) async fn set_network_condition(
    args: &Value,
    _roots: &[String],
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let device = super::ui_tools::resolve_authorized_device(args["device"].as_str(), "shell").await?;
    let mode = args["mode"].as_str().unwrap_or("normal");

    let (bandwidth_kbps, delay_ms, loss_pct) = match mode {
        "normal" => (0u64, 0u64, 0u64),
        "weak" => (500, 100, 1),
        "slow" => (100, 500, 0),
        "lossy" => (1000, 50, 10),
        "custom" => (
            args["custom_bandwidth_kbps"].as_u64().unwrap_or(0),
            args["custom_delay_ms"].as_u64().unwrap_or(0),
            args["custom_loss_pct"].as_u64().unwrap_or(0).min(100),
        ),
        _ => return Err("mode 必须是 normal/weak/slow/lossy/custom".into()),
    };

    if mode == "normal" {
        let iface = detect_network_iface(&device).await.ok_or("未发现可恢复的在线网络接口")?;
        let output = match run_hdc_shell(&device, &["tc", "qdisc", "del", "dev", &iface, "root"], 10).await {
            Ok(output) => output,
            Err(error) if error.to_ascii_lowercase().contains("no such file") => error,
            Err(error) => return Err(format!("重置网络失败（设备 {device}，接口 {iface}）：{error}")),
        };
        let lower = output.to_lowercase();
        if lower.contains("not found") || lower.contains("inaccessible") {
            return Err(format!("重置网络失败（设备 {device}，接口 {iface}）：{}", output.trim()));
        }
        let state = run_hdc_shell(&device, &["tc", "qdisc", "show", "dev", &iface], 10).await?;
        if qdisc_has_impairment(&state) {
            return Err(format!("网络恢复命令已返回，但读回仍存在限速规则（设备 {device}，接口 {iface}）：{}", state.trim()));
        }
        ctx.record_run_event("harmony.network.condition", serde_json::json!({
            "device_id": device, "mode": "normal", "interface": iface, "verified": true,
            "evidence": tail(&state, 500),
        }));
        return Ok(format!("网络已恢复正常并完成读回确认（设备 {device}，接口 {iface}）\n{}", state.trim()));
    }

    // 设置弱网：用 tc netem（需要 root）
    let iface = detect_network_iface(&device).await;
    let iface_str = iface.as_deref().unwrap_or("wlan0");

    // 先删除现有 qdisc
    let _ = run_hdc_shell(&device, &["tc", "qdisc", "del", "dev", iface_str, "root"], 5).await;

    let mut cmd = vec!["tc", "qdisc", "add", "dev", iface_str, "root", "netem"];
    let mut owned: Vec<String> = Vec::new();
    if delay_ms > 0 {
        owned.push("delay".to_string());
        owned.push(format!("{delay_ms}ms"));
    }
    if loss_pct > 0 {
        owned.push("loss".to_string());
        owned.push(format!("{loss_pct}%"));
    }
    if bandwidth_kbps > 0 {
        owned.push("rate".to_string());
        owned.push(format!("{bandwidth_kbps}kbit"));
    }
    for o in &owned {
        cmd.push(o.as_str());
    }

    match run_hdc_shell(&device, &cmd, 10).await {
        Ok(o) if !o.to_lowercase().contains("not found") && !o.contains("No such file") => {
            let state = run_hdc_shell(&device, &["tc", "qdisc", "show", "dev", iface_str], 10).await?;
            if !qdisc_has_impairment(&state) {
                let _ = run_hdc_shell(&device, &["tc", "qdisc", "del", "dev", iface_str, "root"], 5).await;
                return Err(format!("弱网命令已返回，但读回未发现 netem/tbf 规则，已尝试恢复：{}", state.trim()));
            }
            let mut out = format!("网络条件已设置（设备 {device}，模式：{mode}）\n");
            if bandwidth_kbps > 0 { out.push_str(&format!("带宽：{bandwidth_kbps} Kbps\n")); }
            if delay_ms > 0 { out.push_str(&format!("延迟：{delay_ms} ms\n")); }
            if loss_pct > 0 { out.push_str(&format!("丢包率：{loss_pct}%\n")); }
            out.push_str(&format!("接口：{iface_str}\n"));
            out.push_str(&format!("输出：{}\n", o.trim()));
            out.push_str(&format!("读回：{}\n", state.trim()));
            out.push_str("\n⚠️  测试完成后记得调用 set_network_condition mode=normal 恢复网络！");
            ctx.record_run_event("harmony.network.condition", serde_json::json!({
                "device_id": device, "mode": mode, "interface": iface_str, "verified": true,
                "bandwidth_kbps": bandwidth_kbps, "delay_ms": delay_ms, "loss_pct": loss_pct,
                "evidence": tail(&state, 500),
            }));
            Ok(out)
        }
        Ok(o) => Err(format!("设置网络条件失败：{o}\n\n提示：需要 root 或 userdebug 权限的设备才能使用 tc 命令。")),
        Err(e) => Err(format!("设置网络条件失败：{e}\n\n提示：需要 root 或 userdebug 权限的设备才能使用 tc 命令。")),
    }
}

fn qdisc_has_impairment(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("netem") || lower.contains(" tbf ") || lower.starts_with("qdisc tbf")
}

#[cfg(test)]
mod network_condition_tests {
    use super::{network_iface_is_active, qdisc_has_impairment};

    #[test]
    fn qdisc_readback_distinguishes_normal_and_impaired_network() {
        assert!(qdisc_has_impairment("qdisc netem 8001: root refcnt 2 limit 1000 delay 100ms loss 1%"));
        assert!(qdisc_has_impairment("qdisc tbf 1: root rate 500Kbit"));
        assert!(!qdisc_has_impairment("qdisc mq 0: root\nqdisc fq_codel 0: parent :1"));
    }

    #[test]
    fn network_interface_requires_up_or_non_loopback_address() {
        assert!(network_iface_is_active("wlan0: flags=4163<UP,BROADCAST,RUNNING>"));
        assert!(network_iface_is_active("    inet 192.168.1.8 netmask 255.255.255.0"));
        assert!(!network_iface_is_active("wlan0: flags=4098<BROADCAST,MULTICAST>"));
        assert!(!network_iface_is_active("    inet 127.0.0.1 netmask 255.0.0.0"));
    }
}

pub(super) async fn detect_network_iface(device: &str) -> Option<String> {
    // 优先尝试 wlan0，然后 eth0
    for iface in ["wlan0", "eth0", "wlan1"] {
        if let Ok(out) = run_hdc_shell(device, &["ifconfig", iface], 3).await {
            if network_iface_is_active(&out) {
                return Some(iface.to_string());
            }
        }
    }
    None
}

fn network_iface_is_active(output: &str) -> bool {
    output.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("flags=") && lower.contains("up")
            || lower.trim_start().starts_with("inet ") && !lower.contains("127.0.0.1")
    })
}

/// check_signature：检查签名信息。
pub(super) async fn check_signature(args: &Value, roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let bundle = match args["bundle"].as_str() {
        Some(b) => b.to_string(),
        None => String::new(),
    };
    let hap_path = match args["hap_path"].as_str() {
        Some(p) => {
            let resolved = resolve_in_roots(roots, p)?;
            Some(resolved.to_string_lossy().to_string())
        }
        None => None,
    };

    let mut out = format!("签名诊断报告\n");

    // 如果指定了本地 hap
    if let Some(hap) = &hap_path {
        out.push_str(&format!("本地 HAP：{hap}\n"));
        // HAP 本质是 zip，签名通常在 META-INF 或 profile 里
        let file = std::fs::File::open(hap).map_err(|e| format!("打开 HAP 失败: {e}"))?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("解析 HAP 失败: {e}"))?;
        let mut sig_files: Vec<String> = Vec::new();
        let mut profile_content = String::new();
        for i in 0..zip.len() {
            if let Ok(entry) = zip.by_index(i) {
                let name = entry.name().to_string();
                let lower = name.to_lowercase();
                if lower.contains("meta-inf") || lower.ends_with(".p7b") || lower.ends_with(".pem") || lower.ends_with(".cert") || lower.ends_with(".pf") {
                    sig_files.push(name.clone());
                }
                if lower.ends_with("app_profile.json") || lower.contains("provision") {
                    // 读取 profile 内容
                    // （这里只读大小，不做解析）
                    let sz = entry.size();
                    sig_files.push(format!("{name} ({})", super::ui_tools::format_bytes(sz)));
                }
                if lower.contains("pack.info") || lower.ends_with("pack.info") {
                    profile_content = format!("大小: {}", super::ui_tools::format_bytes(entry.size()));
                }
            }
        }
        out.push_str(&format!("签名/配置相关文件（{} 个）：\n", sig_files.len()));
        for f in &sig_files {
            out.push_str(&format!("  • {f}\n"));
        }
        if !profile_content.is_empty() {
            out.push_str(&format!("pack.info：{profile_content}\n"));
        }
        out.push_str("\n签名类型判断：HAP 中有 release.p7b/release.pem 通常是 release 签名；debug 通常对应 debug 证书；系统签名含系统级 profile。\n");
    }

    // 如果指定了已安装包，用 bm dump 看 profile
    if !bundle.is_empty() {
        out.push_str(&format!("\n已安装应用：{bundle}\n"));
        let dump = run_hdc_shell(&device, &["bm", "dump", "-n", &bundle], 20).await
            .unwrap_or_default();
        // 提取签名相关字段
        let app_prov = super::ui_tools::extract_json_str(&dump, "appProvisionType").unwrap_or_else(|| "（未知）".to_string());
        let priv_level = super::ui_tools::extract_json_str(&dump, "appPrivilegeLevel").unwrap_or_else(|| "（未知）".to_string());
        out.push_str(&format!("- 签名类型：{app_prov}\n"));
        out.push_str(&format!("- 特权等级：{priv_level}\n"));

        // 常见错误码解释
        out.push_str("\n常见签名相关错误码：\n");
        out.push_str("  • 9568319：签名不匹配（安装包签名与设备上已装版本不同，卸载旧版本再装）\n");
        out.push_str("  • 其他错误码请结合 bm dump 输出与故障日志确认\n");
    }

    if hap_path.is_none() && bundle.is_empty() {
        return Err("请至少指定 bundle 或 hap_path 之一".into());
    }

    Ok(out)
}

/// dump_battery：电池与耗电分析。
pub(super) async fn dump_battery(args: &Value, _roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let bundle = args["bundle"].as_str().unwrap_or("").to_string();

    let mut out = format!("电池状态报告（设备 {device}）\n\n");

    // 1. hidumper BatteryService
    if let Ok(o) = run_hdc_shell(&device, &["hidumper", "-s", "BatteryService", "-a", "-i"], 10).await {
        let capacity = grep_number(&o, "capacity:");
        let level = grep_number(&o, "batteryLevel:");
        let charging = grep_text(&o, "chargingStatus:");
        let voltage = grep_number(&o, "voltage:");
        let temp = grep_number(&o, "temperature:");

        out.push_str("BatteryService 信息：\n");
        if let Some(c) = capacity { out.push_str(&format!("  电量：{c}%\n")); }
        if let Some(l) = level { out.push_str(&format!("  电量等级：{l}\n")); }
        if let Some(c) = charging { out.push_str(&format!("  充电状态：{c}\n")); }
        if let Some(v) = voltage { out.push_str(&format!("  电压：{v} mV\n")); }
        if let Some(t) = temp {
            // 温度可能是 0.1℃ 单位
            let t_c = t / 10.0;
            out.push_str(&format!("  温度：{t_c:.1} ℃\n"));
        }
    }

    // 2. /sys/class/power_supply/battery/ 兜底读取
    if let Ok(o) = run_hdc_shell(&device, &["cat", "/sys/class/power_supply/battery/capacity"], 5).await {
        let v = o.trim();
        if !v.is_empty() {
            out.push_str(&format!("  电量（sysfs）：{v}%\n"));
        }
    }
    if let Ok(o) = run_hdc_shell(&device, &["cat", "/sys/class/power_supply/battery/status"], 5).await {
        let v = o.trim();
        if !v.is_empty() {
            out.push_str(&format!("  状态（sysfs）：{v}\n"));
        }
    }

    // 3. 应用耗电排行（尽力而为，不同系统接口不同）
    if !bundle.is_empty() {
        out.push_str("\n应用耗电：\n");
        out.push_str("  （耗电排行读取需要系统权限或特定版本，结果仅供参考）\n");
        // 尝试 hidumper -s BatteryStatsService
        if let Ok(o) = run_hdc_shell(&device, &["hidumper", "-s", "BatteryStatsService"], 10).await {
            if o.contains(&bundle) {
                out.push_str("  应用在 BatteryStatsService 输出中被检测到\n");
            } else {
                out.push_str("  未检测到应用耗电数据（可能需要更高级权限或应用未产生显著耗电）\n");
            }
        } else {
            out.push_str("  无法获取 BatteryStatsService 数据\n");
        }
    }

    out.push_str("\n耗电分析提示：\n");
    out.push_str("  • CPU 高频使用是耗电主因，可用 collect_perf 观察 CPU 使用率\n");
    out.push_str("  • 后台长时间运行会持续耗电，检查是否有未释放的 wakelock\n");
    out.push_str("  • 网络频繁唤醒也会耗电，弱网下尤其明显\n");
    out.push_str("  • 可用 run_perf_benchmark 对比操作前后电量变化\n");

    Ok(out)
}

pub(super) fn grep_number(text: &str, key: &str) -> Option<f64> {
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with(key) {
            let rest = &t[key.len()..];
            return super::ui_tools::first_number(rest.trim_start_matches(':').trim());
        }
    }
    None
}

pub(super) fn grep_text<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with(key) {
            return Some(t[key.len()..].trim_start_matches(':').trim());
        }
    }
    None
}

/// scan_api_compat：扫描 ArkTS 源码 API 版本兼容性。
pub(super) async fn scan_api_compat(args: &Value, roots: &[String], db: &crate::db::DbState) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("").to_string();
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录，无法扫描 API 兼容性".into());
    }

    // 读取目标 API 版本
    let target_api = if let Some(v) = args["target_api"].as_u64() {
        v as u32
    } else {
        // 从工程配置读取
        let info = crate::services::harmony::parse_project(Path::new(&project_path));
        info.api_version.unwrap_or(12) as u32
    };

    let scan_path = if let Some(p) = args["path"].as_str() {
        let resolved = resolve_in_roots(roots, p)?;
        resolved.to_string_lossy().to_string()
    } else {
        project_path.clone()
    };

    // 收集所有 .ets/.ts 文件
    let mut files: Vec<String> = Vec::new();
    super::explore_tools::collect_source_files(std::path::Path::new(&scan_path), &mut files);

    // 1. 尝试连接官方 API 知识库
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let db_total = crate::services::harmony_api_diff::count(&conn).unwrap_or(0);
    let using_db = db_total > 0;

    // 2. 扫描所有文件中的 import 语句，提取 @ohos.* / @kit.* 模块
    let mut usages: Vec<(String, usize, String)> = Vec::new(); // (file, line, module)
    for file in &files {
        let content = match std::fs::read_to_string(file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let rel_path = file.strip_prefix(&format!("{project_path}/"))
            .unwrap_or(file)
            .strip_prefix(&format!("{project_path}\\"))
            .unwrap_or(file)
            .to_string();
        for (i, line) in content.lines().enumerate() {
            if !line.contains("import") {
                continue;
            }
            // 提取 from '...' 或 require('...') 中的模块名
            for module in super::explore_tools::extract_import_modules(line) {
                if module.starts_with("@ohos.") || module.starts_with("@kit.") {
                    usages.push((rel_path.clone(), i + 1, module));
                }
            }
        }
    }
    drop(conn);

    let mut issues: Vec<(String, usize, String, u32, String)> = Vec::new();

    if using_db {
        // 3a. 用官方知识库：对每个用到的模块查最低引入版本
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        for (file, line, module) in &usages {
            if let Some((min_ver, decl)) =
                crate::services::harmony_api_diff::min_introduced_version(&conn, module)
            {
                if min_ver > target_api {
                    issues.push((file.clone(), *line, module.clone(), min_ver, decl));
                }
            }
        }
        drop(conn);
    } else {
        // 3b. 回退：内置极简兜底表（仅 @kit. 导入需要 API 12+）
        for (file, line, module) in &usages {
            if module.starts_with("@kit.") && target_api < 12 {
                issues.push((file.clone(), *line, module.clone(), 12, "Kit 化导入".to_string()));
            }
        }
    }

    let mut out = format!("API 版本兼容性扫描\n");
    out.push_str(&format!("扫描路径：{scan_path}\n"));
    out.push_str(&format!("目标 API 版本：{target_api}\n"));
    out.push_str(&format!("扫描文件数：{}\n", files.len()));
    out.push_str(&format!("识别到的鸿蒙模块 import 数：{}\n", usages.len()));
    out.push_str(&format!(
        "知识库：{}\n\n",
        if using_db { format!("官方 API diff 知识库（{db_total} 条）") } else { "未抓取，使用内置兜底（建议先 refresh_api_db）".to_string() }
    ));

    if issues.is_empty() {
        out.push_str("✅ 未发现高于目标版本的 API 使用，兼容性良好。\n");
    } else {
        out.push_str(&format!("⚠️  发现 {} 个潜在不兼容的 API（高于目标 API {target_api}）：\n\n", issues.len()));
        for (i, (file, line, module, min_ver, _decl)) in issues.iter().enumerate() {
            out.push_str(&format!("  {:>3}. {}:{}  →  {}（最低 API {min_ver}）\n", i + 1, file, line, module));
        }
        out.push_str("\n修复建议：\n");
        out.push_str("  1. 升级目标 API 版本（build-profile.json5 中 compatibleSdkVersion）\n");
        out.push_str("  2. 对高版本 API 做运行时版本判断（canIUse / try-catch），低版本降级处理\n");
        out.push_str("  3. 寻找低版本可用的等价 API 替代\n");
        out.push_str("  4. 用 search_api 查该模块各版本的具体声明，确认是否有 API 变更\n");
    }
    Ok(out)
}

// ---------- debug_probe：hilog 插桩（软件断点，无需 DevEco 闭源调试协议） ----------

/// 插桩点记录（会话级，cleanup 按此精确还原）
#[derive(Clone)]
struct ProbePoint {
    /// 工程内相对路径（POSIX 分隔符）
    file: String,
    /// 插桩行号（1 起）
    line: usize,
    /// 插入的整行文本（cleanup 按内容匹配删除）
    inserted: String,
    /// 所在函数/方法名
    target: String,
}

static PROBES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Vec<ProbePoint>>>> =
    std::sync::OnceLock::new();

fn probe_table() -> &'static std::sync::Mutex<std::collections::HashMap<String, Vec<ProbePoint>>> {
    PROBES.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// debug_probe：在 .ets 源文件中按函数/方法名插桩 hilog 日志（入口 + 变量值），
/// 测试完成后 cleanup 一键还原。action: insert（缺省）/ cleanup / list。
pub(super) async fn debug_probe(
    args: &Value,
    roots: &[String],
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    match args["action"].as_str().unwrap_or("insert") {
        "cleanup" => cleanup_probes(&ctx.conversation_id, roots),
        "list" => list_probes(&ctx.conversation_id),
        _ => insert_probe(args, roots, &ctx.conversation_id),
    }
}

/// 插入插桩：定位函数体 → 确保 hilog import → 函数体首行插入日志 → 记录插桩点
fn insert_probe(args: &Value, roots: &[String], conv: &str) -> Result<String, String> {
    let raw = args["path"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("debug_probe 需要参数 {\"path\":\"<文件>\",\"target\":\"<函数/方法名>\",\"vars\":[\"<可选变量名>\"]}")?;
    let target = args["target"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("debug_probe 缺少 target（要插桩的函数/方法名）")?;
    let vars: Vec<String> = args["vars"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let p = resolve_in_roots(roots, raw)?;
    if !p.is_file() {
        return Err(format!("路径不是文件: {}", p.display()));
    }
    let text = std::fs::read_to_string(&p).map_err(|e| format!("读取失败: {e}"))?;
    let mut lines: Vec<String> = text.lines().map(String::from).collect();

    // 1) 定位函数体起始行（含 { 的行）
    let body_idx = find_function_body(&lines, target).ok_or_else(|| {
        format!(
            "未找到函数/方法 {target}（支持 `function {target}(` 与类方法 `{target}(` 签名；请确认 target 与源码一致）"
        )
    })?;

    // 2) 生成插桩行（ArkTS 字符串拼接 + JSON.stringify 防对象打印 [object Object]）
    let probe = if vars.is_empty() {
        format!("hilog.info(0x0000, 'devecoProbe', 'PROBE enter {target}');")
    } else {
        let parts: Vec<String> = vars
            .iter()
            .map(|v| format!("{v}=' + JSON.stringify({v}) + '"))
            .collect();
        format!(
            "hilog.info(0x0000, 'devecoProbe', 'PROBE enter {target} {}');",
            parts.join(" ")
        )
    };

    // 3) 确保 hilog 已导入（缺 import 时在 import 区插入，记录行号偏移）
    let import_added = ensure_hilog_import(&mut lines);
    let offset = if import_added { 1 } else { 0 };

    // 4) 函数体首行插入（body_idx 是含 { 的行；probe 放其下一行 = 函数体第一行）
    let insert_at = body_idx + 1 + offset;
    lines.insert(insert_at, probe.clone());

    // 5) 写回 + 记录插桩点
    std::fs::write(&p, lines.join("\n") + "\n").map_err(|e| format!("写入失败: {e}"))?;
    let rel = normalize_rel(&p, roots);
    let mut t = probe_table().lock().unwrap_or_else(|x| x.into_inner());
    t.entry(conv.to_string()).or_default().push(ProbePoint {
        file: rel.clone(),
        line: insert_at + 1,
        inserted: probe.clone(),
        target: target.to_string(),
    });
    let count = t.get(conv).map(|v| v.len()).unwrap_or(0);

    Ok(format!(
        "已插桩 {rel}:{}（{target}）\n插入内容：{probe}\n当前会话累计 {count} 处插桩。\n后续流程：build_project → deploy → 操作复现 → query_hilog(tag=\"devecoProbe\", level=\"INFO\") 查看日志 → debug_probe(action=\"cleanup\") 清理还原。",
        insert_at + 1
    ))
}

/// 行级启发式定位函数体起始行：行内出现 `<target>(` 且不是调用语句/箭头函数/控制流，
/// `{` 可在本行或紧随其后（签名跨行最多看 3 行）。
fn find_function_body(lines: &[String], target: &str) -> Option<usize> {
    let sig = format!("{target}(");
    for (i, line) in lines.iter().enumerate() {
        let Some(idx) = line.find(&sig) else { continue };
        // target 前不能是标识符字符（避免 fooBar 命中 foo）
        let before_ok = idx == 0
            || !line[..idx]
                .chars()
                .last()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if !before_ok {
            continue;
        }
        let ts = line.trim_start();
        if ts.starts_with("//")
            || line.contains("=>")
            || [
                "if ", "for ", "while ", "switch ", "catch ", "return ", "const ", "let ",
                "var ", "this.",
            ]
            .iter()
            .any(|k| ts.starts_with(k))
        {
            continue;
        }
        if line.contains('{') {
            return Some(i);
        }
        // 签名跨行：后续 3 行内找 {（排除注释行）
        for j in (i + 1)..(i + 4).min(lines.len()) {
            let tj = lines[j].trim_start();
            if tj.starts_with("//") {
                continue;
            }
            if lines[j].contains('{') {
                return Some(j);
            }
        }
    }
    None
}

/// 确保文件已导入 hilog；缺省时在 import 区插入（返回是否新增）。
/// 新 SDK（API 12+）用 @kit.PerformanceAnalysisKit；文件整体是旧式 @ohos. import 时用 @ohos.hilog。
fn ensure_hilog_import(lines: &mut Vec<String>) -> bool {
    if lines.iter().any(|l| l.contains("import") && l.contains("hilog")) {
        return false;
    }
    let joined = lines.join("\n");
    let use_kit = !joined.contains("from '@ohos.") || joined.contains("@kit.");
    let import_line = if use_kit {
        "import { hilog } from '@kit.PerformanceAnalysisKit';".to_string()
    } else {
        "import hilog from '@ohos.hilog';".to_string()
    };
    if let Some(pos) = lines.iter().position(|l| l.trim_start().starts_with("import ")) {
        lines.insert(pos + 1, import_line);
    } else {
        // 无 import：跳过文件头注释/空行后插入
        let mut pos = 0;
        while pos < lines.len() {
            let t = lines[pos].trim_start();
            if t.is_empty() || t.starts_with("//") || t.starts_with("/*") || t.starts_with('*') {
                pos += 1;
            } else {
                break;
            }
        }
        lines.insert(pos, import_line);
    }
    true
}

/// 工程内相对路径（POSIX 分隔符）；roots 匹配失败时返回原始显示路径
fn normalize_rel(p: &std::path::Path, roots: &[String]) -> String {
    for r in roots {
        if let Ok(rc) = std::fs::canonicalize(r) {
            if let Ok(pc) = p.canonicalize() {
                if let Ok(rel) = pc.strip_prefix(&rc) {
                    return rel.to_string_lossy().replace('\\', "/");
                }
            }
        }
    }
    p.to_string_lossy().to_string()
}

/// 按记录删除插桩行（多文件分组、行号从大到小删除避免偏移；行号失配时按内容匹配兜底）
fn cleanup_probes(conv: &str, roots: &[String]) -> Result<String, String> {
    let mut t = probe_table().lock().unwrap_or_else(|x| x.into_inner());
    let Some(points) = t.get_mut(conv) else {
        return Ok("当前会话没有插桩记录（可能已清理）".into());
    };
    if points.is_empty() {
        return Ok("当前会话没有插桩记录（可能已清理）".into());
    }
    // 按文件分组
    let mut by_file: std::collections::BTreeMap<String, Vec<ProbePoint>> = std::collections::BTreeMap::new();
    for p in points.drain(..) {
        by_file.entry(p.file.clone()).or_default().push(p);
    }
    let mut removed = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for (file, mut pts) in by_file {
        let Ok(abs) = resolve_in_roots(roots, &file) else {
            errors.push(format!("{file}: 无法定位（项目目录可能已变更）"));
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&abs) else {
            errors.push(format!("{file}: 读取失败"));
            continue;
        };
        let mut lines: Vec<String> = text.lines().map(String::from).collect();
        pts.sort_by_key(|p| std::cmp::Reverse(p.line));
        for p in &pts {
            let by_line = p
                .line
                .checked_sub(1)
                .filter(|&i| i < lines.len())
                .is_some_and(|i| lines[i].trim() == p.inserted.trim());
            if by_line {
                lines.remove(p.line - 1);
                removed += 1;
            } else if let Some(real) = lines.iter().position(|l| l.trim() == p.inserted.trim()) {
                lines.remove(real);
                removed += 1;
            }
        }
        if let Err(e) = std::fs::write(&abs, lines.join("\n") + "\n") {
            errors.push(format!("{file}: 写回失败 {e}"));
        }
    }
    // 清理空会话记录
    if let Some(v) = t.get(conv) {
        if v.is_empty() {
            t.remove(conv);
        }
    }
    let mut out = format!("已清理 {removed} 处插桩。");
    if !errors.is_empty() {
        out.push_str(&format!("\n部分文件处理失败：\n{}", errors.join("\n")));
    }
    Ok(out)
}

fn list_probes(conv: &str) -> Result<String, String> {
    let t = probe_table().lock().unwrap_or_else(|x| x.into_inner());
    let Some(points) = t.get(conv) else {
        return Ok("当前会话没有插桩记录".into());
    };
    if points.is_empty() {
        return Ok("当前会话没有插桩记录".into());
    }
    let mut out = format!("当前会话共 {} 处插桩：\n", points.len());
    for p in points {
        out.push_str(&format!("  {}:{}  {}\n", p.file, p.line, p.target));
    }
    out.push_str("测试完成后调用 debug_probe(action=\"cleanup\") 一键清理还原。");
    Ok(out)
}

// ---------- stack_dump：进程/线程级运行快照 ----------

/// stack_dump：定位应用进程并采集线程快照（ps 找 pid → /proc 线程枚举 → hidumper 进程详情）。
/// 完整 JS 函数级调用栈依赖 DevEco Profiler 闭源协议，本工具提供可达的最强进程/线程快照，
/// 需要函数级执行顺序时配合 debug_probe 插桩观察。
pub(super) async fn stack_dump(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let bundle = match args["package"].as_str().map(String::from) {
        Some(b) if !b.trim().is_empty() => b,
        _ => {
            if project_path.is_empty() {
                return Err(
                    "未指定 package 且当前会话未绑定工程（无法推导 bundleName）。参数：{\"device\":\"<可选>\",\"package\":\"<包名>\"}"
                        .into(),
                );
            }
            crate::services::harmony::parse_project(std::path::Path::new(project_path))
                .bundle_name
                .ok_or("当前工程无法解析 bundleName（未找到 AppScope/app.json5 的 app.bundleName）")?
        }
    };

    // 1) 定位主进程 pid：ps -A 中 CMD 含包名
    let ps = run_hdc_shell(&device, &["ps", "-A"], 30).await?;
    let mut pids: Vec<String> = Vec::new();
    for line in ps.lines() {
        if line.contains(&bundle) {
            if let Some(pid) = line
                .split_whitespace()
                .next()
                .filter(|p| p.chars().all(|c| c.is_ascii_digit()))
            {
                if !pids.contains(&pid.to_string()) {
                    pids.push(pid.to_string());
                }
            }
        }
    }
    if pids.is_empty() {
        let bm = run_hdc_shell(&device, &["bm", "dump", "-n", &bundle], 30).await?;
        if hdc_shell_failed(&bm) || !bm.contains("bundleName") {
            return Err(format!(
                "设备 {device} 上未找到应用 {bundle}（可能未安装；请先 deploy）"
            ));
        }
        return Err(format!(
            "应用 {bundle} 已安装但进程未运行（先 start_app 或手动打开应用，再调用本工具）"
        ));
    }

    let mut out = format!("应用 {bundle} 进程快照（设备 {device}，{} 个进程）：\n", pids.len());
    for pid in &pids {
        // 2) 线程列表：/proc/<pid>/task 枚举 + comm 名称（比 ps -T 更可靠）
        let ls_cmd = format!("ls /proc/{pid}/task");
        let ls_args = vec!["sh", "-c", ls_cmd.as_str()];
        let tasks = run_hdc_shell(&device, &ls_args, 30).await.unwrap_or_default();
        let mut tids: Vec<String> = tasks.split_whitespace().map(String::from).collect();
        tids.sort_by_key(|t| t.parse::<u32>().unwrap_or(0));
        let mut thread_lines: Vec<String> = Vec::new();
        for tid in tids.iter().take(60) {
            let cat_cmd = format!("cat /proc/{pid}/task/{tid}/comm");
            let cat_args = vec!["sh", "-c", cat_cmd.as_str()];
            if let Ok(comm) = run_hdc_shell(&device, &cat_args, 20).await {
                let name = comm.trim();
                if !name.is_empty() {
                    thread_lines.push(format!("    tid {tid}: {name}"));
                }
            }
        }
        // 3) 进程详情（CPU/内存/线程状态）
        let detail = run_hdc_shell(&device, &["hidumper", "-p", pid], 40)
            .await
            .unwrap_or_else(|e| format!("(hidumper 不可用: {e})"));
        out.push_str(&format!(
            "\n=== PID {pid} ===\n线程数：{}（展示前 60）\n{}\n进程详情：\n{}\n",
            tids.len(),
            thread_lines.join("\n"),
            truncate_out(&detail)
        ));
    }
    out.push_str(
        "\n说明：这是进程/线程级快照；需要函数级执行顺序时，在关键路径用 debug_probe 插桩 hilog 后在 hilog 中观察。",
    );
    Ok(out)
}
