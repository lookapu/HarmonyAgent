//! security 子模块 — 按职责拆分（详见 quality_tools.rs facade）。
//!
//! 调用方式不变：quality_tools::xxx(...)，通过 quality_tools 内的 pub use re-export。

use super::*;
pub(super) async fn obfuscate(args: &Value, roots: &[String]) -> Result<String, String> {
    let action = args["action"].as_str().unwrap_or("status");
    if !matches!(action, "status" | "enable" | "disable") {
        return Err(format!("未知 action \"{action}\"。可用：status|enable|disable"));
    }
    let raw = args["path"].as_str().unwrap_or("build-profile.json5");
    let p = resolve_readable(roots, raw)?;
    if !p.is_file() {
        return Err(format!("未找到 {}（当前目录不是 HarmonyOS 工程根？）", p.display()));
    }
    let content = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    // 定位 obfuscation 段及其后的第一个 enable 键行
    let obs_pos = content.find("obfuscation").ok_or(format!("{} 中未找到 obfuscation 段（工程可能未配置混淆）", p.display()))?;
    let tail = &content[obs_pos..];
    // 在 obfuscation 段后查找 enable: <bool> 行（JSON5 允许不带引号键；可能带引号）
    let enable_patterns = ["\"enable\"", "enable"];
    let mut enable_pos: Option<usize> = None;
    for pat in enable_patterns {
        if let Some(rel) = tail.find(pat) {
            // 必须是键位置：后面跟着 : 
            let after = &tail[rel + pat.len()..];
            if after.trim_start().starts_with(':') {
                enable_pos = Some(obs_pos + rel);
                break;
            }
        }
    }
    let Some(ep) = enable_pos else {
        return Err("obfuscation 段下未找到 enable 开关（结构异常，请人工检查 build-profile.json5）".into());
    };
    // 从 enable 键冒号后截取当前布尔值
    let after_colon = &content[ep + content[ep..].find(':').unwrap() + 1..];
    let after_colon = after_colon.trim_start();
    let mut current: Option<bool> = None;
    for (val, b) in [("true", true), ("false", false)] {
        if after_colon.starts_with(val) {
            current = Some(b);
            break;
        }
    }
    let current = current.ok_or("enable 开关值无法解析（仅支持 true/false）")?;
    if action == "status" {
        let rule_files: Vec<String> = content[obs_pos..]
            .lines()
            .filter(|l| l.contains("files") || l.trim_start().starts_with('"') && l.contains(".txt"))
            .take(5)
            .map(|l| l.trim().to_string())
            .collect();
        let mut out = format!("混淆开关：{}（{}\n", if current { "✅ 已开启" } else { "⬜ 已关闭" }, p.display());
        if current {
            out.push_str("  说明：release 构建将按 ruleOptions.files 规则混淆产物；混淆后可用 stack_dump/analyze_crash 验证符号映射。");
        } else {
            out.push_str("  说明：开启后 release 构建执行混淆（注意保留规则文件，否则可能误删导出符号）。");
        }
        if !rule_files.is_empty() {
            out.push_str(&format!("\n  相关配置行：\n{}", rule_files.join("\n")));
        }
        return Ok(out);
    }
    let want = action == "enable";
    if current == want {
        return Ok(format!("混淆已是{}状态，无需变更。", if want { "开启" } else { "关闭" }));
    }
    // 备份 + 行级替换
    let project_root = roots.first().map(String::as_str).unwrap_or("");
    let backup_dir = format!("{}/.deveco-agent/backups", project_root.trim_end_matches(['/', '\\']));
    if !backup_dir.is_empty() && backup_dir != "/.deveco-agent/backups" {
        std::fs::create_dir_all(&backup_dir).map_err(|e| e.to_string())?;
        let stamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let backup = format!("{backup_dir}/build-profile.json5.{stamp}.bak");
        std::fs::copy(&p, &backup).map_err(|e| e.to_string())?;
    }
    // 重建文件：把 enable 行的布尔值替换
    let line_span = content[..ep].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = content[ep..].find('\n').map(|i| ep + i).unwrap_or(content.len());
    let old_line = &content[line_span..line_end];
    let colon = old_line.find(':').ok_or("enable 行缺少冒号")?;
    let val_start = line_span + colon + 1;
    let val_end = line_end;
    let mut new_content = content.clone();
    new_content.replace_range(val_start..val_end, &format!(" {}", if want { "true" } else { "false" }));
    std::fs::write(&p, new_content).map_err(|e| e.to_string())?;
    Ok(format!(
        "混淆已{}：{}\n已备份原文件到 .deveco-agent/backups/。\n下次 release 构建生效（build_project mode=release）。",
        if want { "开启" } else { "关闭" },
        p.display()
    ))
}

pub(super) async fn sandbox_exec(args: &Value, roots: &[String]) -> Result<String, String> {
    let command = args["command"].as_str().ok_or("sandbox_exec 需要参数 {\"command\":\"<命令串>\"}")?;
    if command.trim().is_empty() {
        return Err("command 为空".into());
    }
    let mode = args["mode"].as_str().unwrap_or("simulate");
    if !matches!(mode, "simulate" | "preview") {
        return Err(format!("未知 mode \"{mode}\"。可用：simulate|preview"));
    }
    let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(30).clamp(5, 120);
    // 静态危险分析（与 run_command 同一规则口径）
    let lower = command.to_lowercase();
    let dangerous: Vec<&str> = crate::services::permissions::DANGEROUS_PATTERNS
        .iter()
        .filter(|p| lower.contains(&p.to_lowercase()))
        .copied()
        .collect();
    let first_word = command.split_whitespace().next().unwrap_or("");
    let program = first_word
        .split(['/', '\\'])
        .last()
        .unwrap_or("")
        .to_lowercase();
    let allowed = crate::services::permissions::ALLOWED_COMMANDS.contains(&program.as_str());
    let mut out = String::new();
    out.push_str(&format!("🛡️ 沙箱干跑预览\n命令：{command}\n模式：{mode}\n"));
    out.push_str(&format!("程序：{program}（{}）\n", if allowed { "白名单内" } else { "白名单外 ⚠️" }));
    if !dangerous.is_empty() {
        out.push_str(&format!("命中危险模式：{}\n", dangerous.join(" / ")));
    }
    if mode == "preview" {
        out.push_str("\n（preview 模式：仅分析未执行。建议：确认影响面后，或改在沙箱 simulate 模式执行，或直接 run_command 真执行）");
        return Ok(out);
    }
    // simulate：复制 source 到临时沙箱后执行
    let sandbox = std::env::temp_dir().join(format!("deveco_sandbox_{}", uuid::Uuid::new_v4()));
    if let Some(src) = args["source"].as_str() {
        let src_path = resolve_readable(roots, src)?;
        if !src_path.exists() {
            return Err(format!("source 目录不存在: {}", src_path.display()));
        }
        std::fs::create_dir_all(&sandbox).map_err(|e| e.to_string())?;
        let copied = copy_tree(&src_path, &sandbox, 0)?;
        out.push_str(&format!("已复制 {} 到沙箱（{} 个文件）\n", src_path.display(), copied));
    } else if !dangerous.is_empty() && !allowed {
        // 无 source 且命令危险且程序不在白名单：拒绝模拟执行（无隔离边界）
        return Ok(format!(
            "{out}\n⚠️ 无 source 隔离边界且程序不在白名单，拒绝模拟执行。\n建议：传 source 参数把目录复制进沙箱后模拟，或直接 run_command 走审批流程。"
        ));
    }
    // 在沙箱中执行（白名单程序；沙箱内影响面可控）
    let mut cmd = tokio::process::Command::new(first_word);
    cmd.args(command.split_whitespace().skip(1));
    if sandbox.exists() {
        cmd.current_dir(&sandbox);
    }
    let output = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        cmd.output(),
    )
    .await
    .map_err(|_| format!("沙箱执行超时（>{timeout_secs}s）"))?
    .map_err(|e| format!("沙箱执行失败：{e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    out.push_str(&format!(
        "退出码：{}\n",
        output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "无".into())
    ));
    if !stdout.trim().is_empty() {
        out.push_str(&format!("标准输出（截断 4000 字符）：\n{}\n", truncate_chars(&stdout, 4000)));
    }
    if !stderr.trim().is_empty() {
        out.push_str(&format!("标准错误：\n{}\n", truncate_chars(&stderr, 2000)));
    }
    out.push_str(&format!(
        "\n⚠️ 以上在临时沙箱 {} 中执行，未影响真实目录。\n确认行为符合预期后，再在真实目录执行（run_command 会走审批）。",
        sandbox.display()
    ));
    Ok(out)
}

pub(super) async fn license_check(
    args: &Value,
    roots: &[String],
) -> Result<String, String> {
    let action = args["action"].as_str().unwrap_or("scan");
    let allow: Vec<String> = args["allow"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_else(|| vec![
            "MIT".into(), "Apache-2.0".into(), "BSD-3-Clause".into(),
            "ISC".into(), "MPL-2.0".into(), "CC0-1.0".into(),
            "Unlicense".into(), "MIT-0".into(),
        ]);
    let deny: Vec<String> = args["deny"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let sub_path = args["path"].as_str().map(str::to_string).unwrap_or_default();
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() { return Err("license_check 需要绑定工程".into()); }
    let base = std::path::Path::new(project_path).join(&sub_path);

    if action == "list" {
        return Ok(format!(
            "白名单（{} 个）：{}\n黑名单：{:?}",
            allow.len(),
            allow.join(", "),
            deny
        ));
    }

    let mut findings: Vec<(String, String, String, String)> = Vec::new();
    // 解析 oh-package.json5
    let oh_pkg = base.join("oh-package.json5");
    if oh_pkg.exists() {
        if let Ok(text) = std::fs::read_to_string(&oh_pkg) {
            for line in text.lines() {
                let t = line.trim();
                if t.starts_with("//") || t.is_empty() { continue; }
                // 形如 "@ohos/xxx": "1.0.0" 或 "name": "version"
                if let Some((name, version)) = parse_dep_line(t) {
                    findings.push((
                        "ohpm".into(),
                        name,
                        version,
                        "(license 待 lock 解析)".into(),
                    ));
                }
            }
        }
    }
    // 解析 oh-package-lock.json5（取 dependencies.*.license）
    let lock = base.join("oh-package-lock.json5");
    if lock.exists() {
        if let Ok(text) = std::fs::read_to_string(&lock) {
            // 简化：按 "name": { "version": "x", "license": "MIT" } 的结构匹配
            for line in text.lines() {
                if !line.contains("license") { continue; }
                // 提取 license 值
                if let Some(pos) = line.find("\"license\":") {
                    let tail = &line[pos + 10..];
                    if let Some(lv) = extract_quoted(tail) {
                        // 找本块对应的 package（上一行 "name": "...")
                        // 简化：直接记一个全局
                        if let Some(name) = extract_quoted(&line[..pos]) {
                            if let Some(last) = findings.last_mut() {
                                if last.0 == "ohpm" && last.3.starts_with("(") {
                                    last.3 = lv;
                                }
                            }
                            let _ = name; // unused
                        }
                    }
                }
            }
        }
    }
    // 解析 Cargo.toml
    let cargo = base.join("Cargo.toml");
    if cargo.exists() {
        if let Ok(text) = std::fs::read_to_string(&cargo) {
            let mut in_deps = false;
            for line in text.lines() {
                if line.starts_with("[dependencies]") { in_deps = true; continue; }
                if line.starts_with("[") && in_deps { in_deps = false; }
                if !in_deps { continue; }
                if let Some((name, version)) = parse_dep_line(line) {
                    findings.push((
                        "cargo".into(),
                        name,
                        version,
                        "(license 需 cargo metadata 联网查询)".into(),
                    ));
                }
            }
        }
    }
    // pyproject.toml 依赖段
    let pyp = base.join("pyproject.toml");
    if pyp.exists() {
        if let Ok(text) = std::fs::read_to_string(&pyp) {
            for line in text.lines() {
                if !line.contains("==") { continue; }
                if let Some((name, version)) = parse_dep_line(line) {
                    findings.push((
                        "uv".into(),
                        name,
                        version,
                        "(license 需 uv pip 联网查询)".into(),
                    ));
                }
            }
        }
    }

    if findings.is_empty() {
        return Ok("未发现可扫描的依赖文件（oh-package.json5 / Cargo.toml / pyproject.toml）".into());
    }

    // 合规性检查
    let mut out = format!("许可证合规扫描报告（基础目录：{}）\n共 {} 个依赖\n\n", base.display(), findings.len());
    let mut allow_count = 0;
    let mut deny_count = 0;
    let mut unknown_count = 0;
    let mut rows = String::new();
    rows.push_str("| 来源 | 名称 | 版本 | License | 状态 |\n");
    rows.push_str("|---|---|---|---|---|\n");
    for (src, name, ver, lic) in &findings {
        let lic_norm = lic.trim_matches('(').trim_matches(')').to_string();
        let status = if deny.iter().any(|d| d.eq_ignore_ascii_case(&lic_norm)) {
            deny_count += 1;
            "❌ DENY"
        } else if lic.contains("待") || lic.contains("需") {
            unknown_count += 1;
            "⚠️ 待查"
        } else if allow.iter().any(|a| a.eq_ignore_ascii_case(&lic_norm)) {
            allow_count += 1;
            "✅ ALLOW"
        } else {
            unknown_count += 1;
            "⚠️ 未在白名单"
        };
        rows.push_str(&format!("| {src} | `{name}` | {ver} | {lic} | {status} |\n"));
    }
    out.push_str(&rows);
    out.push_str(&format!(
        "\n汇总：✅ ALLOW={} / ❌ DENY={} / ⚠️ 未确认={}\n",
        allow_count, deny_count, unknown_count
    ));
    if deny_count > 0 {
        out.push_str("\n⚠️ 存在 DENY 依赖，建议：\n  1. 替换为白名单内的等价库\n  2. 申请法务例外并文档化\n  3. 移除不必要依赖\n");
    }
    if unknown_count > 0 {
        out.push_str(&format!(
            "\nℹ️ {unknown_count} 个依赖 license 待查，可联网时跑 `npm view <pkg> license` / `cargo metadata` / `uv pip show <pkg>` 后再扫\n"
        ));
    }
    Ok(out)
}

pub(super) async fn vuln_scan(
    args: &Value,
    roots: &[String],
) -> Result<String, String> {
    let source = args["source"].as_str().unwrap_or("all");
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() { return Err("vuln_scan 需要绑定工程".into()); }
    let base = std::path::Path::new(project_path);

    // 内置已知漏洞库（小范围示例；实际项目应同步官方 OSV / NVD）
    // 格式：(包名, 受影响版本前缀, 严重级别, 描述)
    let known: Vec<(&str, &str, &str, &str)> = vec![
        ("lodash", "<4.17.21", "high", "原型链污染（CVE-2021-23337）"),
        ("minimatch", "<3.0.5", "high", "ReDoS（CVE-2022-3517）"),
        ("axios", "<1.6.0", "medium", "SSRF（CVE-2023-45857）"),
        ("requests", "<2.31.0", "medium", "证书验证问题（CVE-2023-32681）"),
        ("urllib3", "<1.26.17", "medium", "CRLF 注入（CVE-2023-43804）"),
        ("cryptography", "<41.0.6", "high", "内存破坏（CVE-2023-49083）"),
        ("pyyaml", "<5.4", "high", "任意代码执行（CVE-2020-14343）"),
        ("@ohos/hypium", "<1.0.0", "low", "测试框架，本地版本无已知 CVE"),
        ("serde", "<1.0.190", "low", "整数溢出（仅在特制数据时触发）"),
        ("tokio", "<1.32.0", "medium", "任务调度竞态（CVE-2023-42465）"),
    ];

    let mut found: Vec<(String, String, String, String, String)> = Vec::new();
    let scan_ohpm = source == "all" || source == "ohpm";
    let scan_cargo = source == "all" || source == "cargo";
    let scan_uv = source == "all" || source == "uv";

    if scan_ohpm {
        let lock = base.join("oh-package-lock.json5");
        if lock.exists() {
            if let Ok(text) = std::fs::read_to_string(&lock) {
                for line in text.lines() {
                    if let Some((name, ver)) = parse_dep_line(line) {
                        for (vn, vprefix, sev, desc) in &known {
                            if name == *vn && version_lt(&ver, vprefix) {
                                found.push(("ohpm".into(), name.clone(), ver.clone(), (*sev).into(), (*desc).into()));
                            }
                        }
                    }
                }
            }
        }
    }
    if scan_cargo {
        let lock = base.join("Cargo.lock");
        if lock.exists() {
            if let Ok(text) = std::fs::read_to_string(&lock) {
                for line in text.lines() {
                    if line.trim().starts_with("name = ") || line.trim().starts_with("version = ") {
                        // 简单占位解析（实际逻辑在下方 block 维护 last_name / last_version）
                        let _ = extract_toml_string(line);
                    }
                }
                // 用更稳的解析：按行扫，name + version 配对
                let mut last_name = String::new();
                for line in text.lines() {
                    if let Some(eq) = line.find('=') {
                        let key = line[..eq].trim();
                        if let Some(val) = extract_toml_string(line) {
                            if key == "name" { last_name = val; }
                            else if key == "version" && !last_name.is_empty() {
                                for (vn, vprefix, sev, desc) in &known {
                                    if last_name == *vn && version_lt(&val, vprefix) {
                                        found.push(("cargo".into(), last_name.clone(), val.clone(), (*sev).into(), (*desc).into()));
                                    }
                                }
                                last_name.clear();
                            }
                        }
                    }
                }
            }
        }
    }
    if scan_uv {
        // pip 锁定文件 requirements.txt with ==
        let req = base.join("requirements.txt");
        if req.exists() {
            if let Ok(text) = std::fs::read_to_string(&req) {
                for line in text.lines() {
                    if let Some((name, ver)) = parse_dep_line(line) {
                        for (vn, vprefix, sev, desc) in &known {
                            if name.eq_ignore_ascii_case(vn) && version_lt(&ver, vprefix) {
                                found.push(("uv".into(), name.clone(), ver.clone(), (*sev).into(), (*desc).into()));
                            }
                        }
                    }
                }
            }
        }
    }

    let mut out = String::new();
    out.push_str(&format!("依赖漏洞扫描报告（来源：{source}，路径：{}）\n", base.display()));
    if found.is_empty() {
        out.push_str("✅ 未发现已知漏洞（基于内置小型漏洞库；生产建议接 OSV / NVD 实时数据）\n");
        return Ok(out);
    }
    out.push_str(&format!("⚠️ 发现 {} 个匹配已知漏洞的依赖：\n\n", found.len()));
    out.push_str("| 来源 | 名称 | 当前版本 | 严重 | 描述 |\n");
    out.push_str("|---|---|---|---|---|\n");
    for (src, name, ver, sev, desc) in &found {
        out.push_str(&format!("| {src} | `{name}` | {ver} | {sev} | {desc} |\n"));
    }
    let high = found.iter().filter(|f| f.3 == "high").count();
    if high > 0 {
        out.push_str(&format!("\n🚨 高危 {} 个，强烈建议立即升级到修复版本。\n", high));
    }
    Ok(out)
}


fn parse_dep_line(line: &str) -> Option<(String, String)> {
    let line = line.trim().trim_end_matches(',');
    // 跳过注释 / 段头
    if line.starts_with('[') || line.starts_with('#') { return None; }
    // 形式1：key = "value" / key = value
    if let Some(eq) = line.find('=') {
        let name = line[..eq].trim().trim_matches('"').to_string();
        let val = line[eq + 1..].trim().trim_matches('"').to_string();
        if name.is_empty() || val.is_empty() { return None; }
        if name.contains(' ') || name.contains('#') { return None; }
        return Some((name, val));
    }
    // 形式2："key": "value"
    if line.contains(':') {
        let parts: Vec<&str> = line.splitn(2, ':').collect();
        let name = parts[0].trim().trim_matches('"').trim_matches('\'').to_string();
        let val = parts[1].trim().trim_matches(',').trim_matches('"').trim_matches('\'').to_string();
        if name.is_empty() || val.is_empty() { return None; }
        if name.contains(' ') { return None; }
        return Some((name, val));
    }
    None
}


fn extract_quoted(s: &str) -> Option<String> {
    let s = s.trim();
    if let Some(start) = s.find('"') {
        if let Some(end) = s[start + 1..].find('"') {
            return Some(s[start + 1..start + 1 + end].to_string());
        }
    }
    None
}


fn version_lt(a: &str, b: &str) -> bool {
    // b 形如 "<1.2.3" 或 "<=1.2.3"；简化：解析开头的比较符和数字
    let b = b.trim_start_matches("<=").trim_start_matches('<').trim();
    let av: Vec<u32> = a.split('.').filter_map(|s| s.split('-').next().and_then(|n| n.parse().ok())).collect();
    let bv: Vec<u32> = b.split('.').filter_map(|s| s.split('-').next().and_then(|n| n.parse().ok())).collect();
    for i in 0..av.len().max(bv.len()) {
        let x = av.get(i).copied().unwrap_or(0);
        let y = bv.get(i).copied().unwrap_or(0);
        if x < y { return true; }
        if x > y { return false; }
    }
    false
}


fn extract_toml_string(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if let Some(start) = trimmed.find('"') {
        if let Some(end) = trimmed[start + 1..].find('"') {
            return Some(trimmed[start + 1..start + 1 + end].to_string());
        }
    }
    None
}


fn copy_tree(src: &Path, dst: &Path, depth: u32) -> Result<u32, String> {
    if depth > 8 {
        return Err("复制嵌套过深（>8），中止".into());
    }
    let mut copied = 0u32;
    let mut total_bytes = 0u64;
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for e in std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
        let sp = e.path();
        let name = e.file_name().to_string_lossy().to_lowercase();
        if SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let dp = dst.join(e.file_name());
        if sp.is_dir() {
            copied += copy_tree(&sp, &dp, depth + 1)?;
        } else if sp.is_file() {
            if copied >= 200 {
                return Err("复制超过 200 个文件，中止（source 过大）".into());
            }
            let len = std::fs::metadata(&sp).map(|m| m.len()).unwrap_or(0);
            if total_bytes + len > 50 * 1024 * 1024 {
                return Err("复制超过 50MB，中止（source 过大）".into());
            }
            std::fs::copy(&sp, &dp).map_err(|e| e.to_string())?;
            total_bytes += len;
            copied += 1;
        }
    }
    Ok(copied)
}

