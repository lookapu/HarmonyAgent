//! 测试域工具：run_tests / read_logcat / web_fetch / search_sdk_api / write_unit_tests / run_ui_flow 等。
//! 共享辅助函数（tail / truncate_out / scan_root / run_in_project 等）仍定义在父模块 mod.rs，
//! 本模块通过 `use super::*` 继承访问。

use super::*;
// ---------- 测试 / 日志 ----------

/// run_tests：运行工程测试（hvigorw test，可选模块）
/// 按工程类型选择测试命令：鸿蒙 hvigorw test；Node npm test；Go go test；Rust cargo test；
/// Python pytest；Maven mvn test；Makefile make test。None=未识别（提示用 run_command）。
/// 返回 (program, args, envs)，envs 仅在鸿蒙分支可能非空（DEVECO_SDK_HOME 注入）。
pub(super) fn test_command_for(root: &Path) -> Option<(String, Vec<String>, Vec<(String, String)>)> {
    // Harmony：Windows 优先 node 直调 hvigor wrapper.js（与 build 一致）
    if root.join("build-profile.json5").is_file() || root.join("oh-package.json5").is_file() {
        let cmd = crate::services::harmony::hvigor_command(root).ok()?;
        let mut args = cmd.args;
        args.push("test".to_string());
        return Some((cmd.program, args, cmd.env));
    }
    if root.join("package.json").is_file() {
        return Some(("npm".to_string(), vec!["test".to_string()], Vec::new()));
    }
    if root.join("go.mod").is_file() {
        return Some(("go".to_string(), vec!["test".to_string(), "./...".to_string()], Vec::new()));
    }
    if root.join("Cargo.toml").is_file() {
        return Some(("cargo".to_string(), vec!["test".to_string()], Vec::new()));
    }
    if root.join("pyproject.toml").is_file()
        || root.join("pytest.ini").is_file()
        || root.join("setup.py").is_file()
    {
        return Some(("python".to_string(), vec!["-m".to_string(), "pytest".to_string()], Vec::new()));
    }
    if root.join("pom.xml").is_file() {
        return Some(("mvn".to_string(), vec!["test".to_string()], Vec::new()));
    }
    if root.join("Makefile").is_file() {
        return Some(("make".to_string(), vec!["test".to_string()], Vec::new()));
    }
    None
}

pub(super) async fn run_tests(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录，无法运行测试".into());
    }
    let root = Path::new(project_path);
    let (program, mut full_args, hvigor_env) = test_command_for(root).ok_or_else(|| {
        format!(
            "未识别到该目录的工程类型（{}），无法自动选择测试命令。\n请用 run_command 手动执行测试命令（如 npm test / go test ./... / cargo test）。",
            root.display()
        )
    })?;
    // 鸿蒙专有参数：module 仅对 hvigor 测试有效（--mode module -p module=xxx@default）
    if let Some(m) = args["module"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if program.contains("node") || program.contains("hvigor") {
            full_args.extend([
                "--mode".into(),
                "module".into(),
                "-p".into(),
                format!("module={m}@default"),
            ]);
        }
    }
    // 覆盖率参数：按测试命令类型追加对应参数（不支持的类型忽略并在结果中提示）
    let mut coverage_note = String::new();
    if args["coverage"].as_bool().unwrap_or(false) {
        match program.as_str() {
            // 鸿蒙：node 直调 hvigor wrapper（hvigor test --coverage）
            "node" => full_args.push("--coverage".into()),
            "npm" => {
                full_args.push("--".into());
                full_args.push("--coverage".into());
            }
            "go" => full_args.push("-coverprofile=coverage.out".into()),
            "python" => full_args.push("--cov".into()),
            _ => coverage_note =
                "（当前工程类型（Rust/Maven/Makefile）不支持 --coverage 参数，已忽略；如需覆盖率请配置对应插件，如 cargo llvm-cov / jacoco）"
                    .to_string(),
        }
    }
    let _gate = crate::services::tool_limits::acquire_gate("run_tests").await;
    let out = if hvigor_env.is_empty() {
        run_cmd(&program, &full_args, Some(root), 600).await
    } else {
        run_cmd_env(&program, &full_args, Some(root), 600, Some(&hvigor_env)).await
    };
    match out {
        Ok(out) => {
            let mut s = out;
            if !coverage_note.is_empty() {
                s.push_str(&format!("\n{coverage_note}"));
            }
            Ok(s)
        }
        Err(e) => Err(with_advice("run_tests", e)),
    }
}

/// read_logcat：读取设备最近 N 行日志（hdc logcat -T N，可选指定设备与关键词过滤）
pub(super) async fn read_logcat(args: &Value) -> Result<String, String> {
    let lines = args["lines"].as_u64().unwrap_or(200).clamp(10, 1000);
    let filter = args["filter"].as_str().unwrap_or("").trim();
    let package = args["package"].as_str().unwrap_or("").trim();
    let tag = args["tag"].as_str().unwrap_or("").trim();
    let level = args["level"].as_str().unwrap_or("").trim().to_uppercase();
    // 多设备连接时解析目标设备
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };

    // 包名 → pid（hilog 按进程过滤更精准；多进程取全部 pid）
    let mut pids: Vec<String> = Vec::new();
    if !package.is_empty() {
        if let Ok(out) = run_hdc_shell(&device, &["pidof", package], 15).await {
            for tok in out.split(|c: char| c.is_whitespace()) {
                let t = tok.trim();
                if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
                    pids.push(t.to_string());
                }
            }
        }
        if pids.is_empty() {
            return Ok(format!("未找到包名为「{package}」的运行进程（应用未启动或包名不正确）"));
        }
    }

    // 级别：hilog -L 支持过滤最低级别（D/I/W/E/F）
    let valid_levels = ["D", "I", "W", "E", "F"];
    let level_flag = if valid_levels.contains(&level.as_str()) {
        Some(level.as_str())
    } else {
        None
    };

    // 组装 hilog 命令：-x 转储历史后退出（不持续跟踪）；-T 不可靠，用 tail 控制行数
    let mut hilog_args: Vec<String> = vec!["hilog".to_string(), "-x".to_string()];
    if let Some(lv) = level_flag {
        hilog_args.push("-L".to_string());
        hilog_args.push(lv.to_string());
    }
    // tag 过滤：-T <tag> （hilog 按 tag 过滤；部分版本用 -D domain，这里用 -T）
    if !tag.is_empty() {
        hilog_args.push("-T".to_string());
        hilog_args.push(tag.to_string());
    }

    let raw = match run_hdc_shell(
        &device,
        &hilog_args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        25,
    )
    .await
    {
        Ok(v) => v,
        Err(_) => {
            // 兜底：部分设备 hilog 参数受限，回退到无过滤的 logcat -T
            run_cmd(
                "hdc",
                &[
                    "-t".to_string(),
                    device.clone(),
                    "logcat".to_string(),
                    "-T".to_string(),
                    lines.to_string(),
                ],
                None,
                20,
            )
            .await
            .map_err(|e| with_advice("read_logcat", e))?
        }
    };

    // 本地逐行过滤：pid、关键词
    let lower = filter.to_lowercase();
    let mut out = String::new();
    let mut kept: Vec<&str> = Vec::new();
    for line in raw.lines() {
        // pid 过滤：hilog 行格式含 "  pid" 或开头列，简单按数字 token 匹配
        if !pids.is_empty() {
            let hit_pid = pids.iter().any(|p| {
                // 匹配独立的 pid 数字（避免 1234 命中 12345）
                line.split(|c: char| !c.is_ascii_digit())
                    .any(|tok| tok == p)
            });
            if !hit_pid {
                continue;
            }
        }
        if !filter.is_empty() && !line.to_lowercase().contains(&lower) {
            continue;
        }
        kept.push(line);
    }
    // 只保留最后 lines 行
    let start = kept.len().saturating_sub(lines as usize);
    for line in &kept[start..] {
        out.push_str(line);
        out.push('\n');
    }
    if out.trim().is_empty() {
        return Ok(format!(
            "设备日志中没有匹配的行（包={package}, tag={tag}, level={}, 关键词={filter}）",
            level_flag.unwrap_or("-")
        ));
    }
    // 截断保护上下文
    Ok(if out.chars().count() > 6000 {
        tail(&out, 6000)
    } else {
        out
    })
}

// ---------- 网页抓取 ----------

/// 简易 HTML → 纯文本（剥离 script/style/标签/实体，压缩空白）
pub(super) fn html_to_text(html: &str) -> String {
    let chars: Vec<char> = html.chars().collect();
    let mut out = String::new();
    let mut in_block = false; // <script>/<style> 内容块
    let mut in_tag = false;
    let mut in_entity = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '<' {
            let rest: String = chars[i..].iter().take(20).collect();
            let l = rest.to_lowercase();
            if l.starts_with("<script") || l.starts_with("<style") {
                in_block = true;
            } else if l.starts_with("</script") || l.starts_with("</style") {
                in_block = false;
            }
            in_tag = true;
            i += 1;
            continue;
        }
        if in_block {
            i += 1;
            continue;
        }
        if in_tag {
            if c == '>' {
                in_tag = false;
                out.push(' ');
            }
            i += 1;
            continue;
        }
        if c == '&' {
            in_entity = true;
            i += 1;
            continue;
        }
        if in_entity {
            if c == ';' {
                in_entity = false;
                out.push(' ');
            } else if c.is_whitespace() {
                in_entity = false;
            }
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// read_runtime_logs：读取部署后自动回流的应用运行期错误日志（环形缓存）。
pub(super) async fn read_runtime_logs(
    args: &Value,
    roots: &[String],
    _ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录，无法读取运行日志".into());
    }
    let n = args.get("lines").and_then(|v| v.as_u64()).unwrap_or(100).clamp(20, 400) as usize;
    let logs = crate::agent::runtime_log::recent(project_path, n);
    if logs.trim().is_empty() {
        return Ok("（暂无可读的运行期错误日志：尚未部署/应用未运行，或运行期没有 error 级日志。部署应用后会自动监听。）".to_string());
    }
    Ok(format!("最近 {n} 行运行期错误日志：\n{logs}"))
}

/// web_fetch：抓取网页正文纯文本（自动代理，≤2MB，截断 max_chars）
pub(super) async fn web_fetch(args: &Value) -> Result<String, String> {
    let url = args["url"].as_str().ok_or("web_fetch 需要参数 {\"url\":\"<https://…>\"}")?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("仅支持 http/https 网址".into());
    }
    let max_chars = args["max_chars"].as_u64().unwrap_or(4000).clamp(500, 10000) as usize;
    let client = crate::utils::net::build_client_auto().map_err(|e| format!("网络初始化失败: {e}"))?;
    let resp = tokio::time::timeout(Duration::from_secs(20), client.get(url).send())
        .await
        .map_err(|_| "抓取超时（>20s），页面可能无响应".to_string())?
        .map_err(|e| format!("请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}（页面拒绝访问）", resp.status()));
    }
    // 预检：Content-Length 已超 2MB 时直接拒绝，避免把大文件整个读入内存
    if resp.content_length().is_some_and(|n| n > 2 * 1024 * 1024) {
        return Err("页面超过 2MB，放弃抓取（可换用 web_search 获取摘要）".into());
    }
    let bytes = tokio::time::timeout(Duration::from_secs(20), resp.bytes())
        .await
        .map_err(|_| "读取响应超时".to_string())?
        .map_err(|e| format!("读取响应失败: {e}"))?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("页面超过 2MB，放弃抓取（可换用 web_search 获取摘要）".into());
    }
    let text = html_to_text(&smart_decode(&bytes));
    if text.is_empty() {
        return Err("页面无可提取的文本内容（可能是动态渲染页面）".into());
    }
    Ok(format!(
        "{}（来源 {url}）\n\n{}\n",
        text.chars().count(),
        truncate_chars(&text, max_chars)
    ))
}

// ---------- SDK API 检索 ----------

use crate::services::sdk_api;

/// search_sdk_api 工具：检索本地 SDK 的 @ohos.*.d.ts 模块
pub(super) fn search_sdk_api(args: &Value, db: &crate::db::DbState) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if query.is_empty() {
        return Err("query 不能为空".to_string());
    }
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(20)
        .min(50);
    let env = crate::services::harmony_env::detect(db);
    let dir = crate::services::harmony_env::default_api_dir(&env)
        .ok_or_else(|| "未找到 SDK 的 ets/api 目录，请在健康检查页配置 SDK".to_string())?;
    let idx = sdk_api::index_api_dir(&dir);
    let hits = sdk_api::search(&idx, &query, limit);
    if hits.is_empty() {
        return Ok(format!("未在本地 SDK 中找到与「{query}」相关的 API 模块。"));
    }
    let mut out = format!(
        "本地 SDK（API {}, {} 个模块）中匹配「{query}」的结果：\n\n",
        env.default_api.as_deref().unwrap_or("?"),
        idx.modules.len()
    );
    for m in hits {
        out.push_str(&format!("## {} ", m.module));
        if let Some(k) = &m.kit {
            out.push_str(&format!("[{k}]"));
        }
        out.push('\n');
        if let Some(s) = &m.syscap {
            out.push_str(&format!("syscap: {s}\n"));
        }
        match (m.since_min, m.since_max) {
            (Some(a), Some(b)) if a != b => out.push_str(&format!("since: API {a}（更新至 API {b}）\n")),
            (Some(a), _) => out.push_str(&format!("since: API {a}\n")),
            _ => {}
        }
        if m.deprecated {
            out.push_str("⚠️ 已废弃（@deprecated）\n");
        }
        if !m.declarations.is_empty() {
            let preview: Vec<&str> = m.declarations.iter().take(20).map(|s| s.as_str()).collect();
            out.push_str(&format!("声明: {}\n", preview.join(", ")));
            if m.declarations.len() > 20 {
                out.push_str(&format!("  …及另外 {} 个\n", m.declarations.len() - 20));
            }
        }
        out.push('\n');
    }
    out.push_str("提示：需要某个模块的精确签名时，调用 read_sdk_api_module 并传入模块名。");
    Ok(out)
}

/// read_sdk_api_module 工具：读取完整 .d.ts 声明
pub(super) fn read_sdk_api_module(args: &Value, db: &crate::db::DbState) -> Result<String, String> {
    let module = args
        .get("module")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if module.is_empty() {
        return Err("module 不能为空".to_string());
    }
    let env = crate::services::harmony_env::detect(db);
    let dir = crate::services::harmony_env::default_api_dir(&env)
        .ok_or_else(|| "未找到 SDK 的 ets/api 目录".to_string())?;
    // 规范化文件名：补 .d.ts 后缀，防目录穿越
    let fname = std::path::Path::new(&module)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or(module.clone());
    let fname = if fname.ends_with(".d.ts") { fname } else { format!("{fname}.d.ts") };
    let path = std::path::PathBuf::from(&dir).join(&fname);
    if !path.is_file() {
        return Err(format!("未找到声明文件：{fname}（请先用 search_sdk_api 确认模块名）"));
    }
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    // 大文件截断到合理长度（d.ts 通常几十 KB，限制 60KB 避免撑爆上下文）
    const MAX: usize = 60_000;
    if content.len() <= MAX {
        Ok(format!("// {fname}\n{content}"))
    } else {
        let cut: String = content.chars().take(MAX).collect();
        Ok(format!(
            "// {fname}（文件较大，仅显示前 {MAX} 字符）\n{cut}\n// …已截断，可用 read_file 配合 start/lines 精读特定段落"
        ))
    }
}

/// search_harmony_docs 工具：检索本地 OpenHarmony 文档库
pub(super) async fn search_harmony_docs_tool(args: &Value, ctx: &crate::agent::exec_ctx::ToolCtx) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if query.is_empty() {
        return Err("query 不能为空".to_string());
    }
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(10)
        .min(30);
    let Some(app) = &ctx.app else {
        return Err("当前环境无文档库访问能力".into());
    };
    let root = crate::services::harmony_docs::docs_root(app)
        .ok_or_else(|| "本地 OpenHarmony 文档库未下载。请先让用户在健康检查页点击「下载/更新 OpenHarmony 文档」，或用 web_search 检索公开资料。".to_string())?;
    let idx = crate::services::harmony_docs::index_docs(&root);
    let hits = crate::services::harmony_docs::search(&idx, &query, limit);
    if hits.is_empty() {
        return Ok(format!(
            "本地文档库（共 {} 篇）中未找到与「{query}」相关的文档。\n可换关键词重试，或用 web_fetch 抓 docs.openharmony.cn 公开页面（无需登录）。",
            idx.entries.len()
        ));
    }
    let mut out = format!("本地 OpenHarmony 文档库（{} 篇）中匹配「{query}」的结果：\n\n", idx.entries.len());
    for e in hits {
        out.push_str(&format!(
            "## {} [{}]\n",
            e.title,
            if e.kit.is_empty() { "通用" } else { &e.kit }
        ));
        out.push_str(&format!("  路径: {}\n", e.rel_path));
        if e.has_example {
            out.push_str("  📎 含示例代码\n");
        }
        if !e.preview.is_empty() {
            out.push_str(&format!("  内容: {}\n", e.preview));
        }
        out.push('\n');
    }
    out.push_str("提示：需要精读某篇时调用 read_harmony_doc 并传入 path。");
    Ok(truncate_out_max(&out, 8000))
}

/// read_harmony_doc 工具：读取某篇文档完整原文
pub(super) async fn read_harmony_doc_tool(args: &Value, ctx: &crate::agent::exec_ctx::ToolCtx) -> Result<String, String> {
    let rel = args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if rel.is_empty() {
        return Err("缺少 path 参数（来自 search_harmony_docs 的 rel_path）".into());
    }
    let Some(app) = &ctx.app else {
        return Err("当前环境无文档库访问能力".into());
    };
    let root: std::path::PathBuf = crate::services::harmony_docs::docs_root(app)
        .ok_or_else(|| "本地 OpenHarmony 文档库未下载".to_string())?;
    // 防目录穿越：规范化后必须仍在 root 内
    let path = root.join(rel);
    let canon = path.canonicalize().map_err(|e| format!("文档路径无效: {e}"))?;
    if !canon.starts_with(&root) {
        return Err("禁止读取文档库之外的文件".into());
    }
    let text = std::fs::read_to_string(&canon).map_err(|e| format!("读取失败: {e}"))?;
    // 单篇文档通常 <100KB，截断保护上下文
    let cut: String = text.chars().take(100_000).collect();
    Ok(format!("// {rel}\n{cut}"))
}

// ---------- 单元测试生成 / UI 自动化 / 性能基准 ----------

/// write_unit_tests：根据 ArkTS 源码自动生成 hypium 单元测试骨架，写入模块 src/test/。
pub(super) async fn write_unit_tests(args: &Value, roots: &[String]) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法生成测试".into());
    }
    let raw = args["path"].as_str().ok_or("write_unit_tests 需要参数 {\"path\":\"<源码文件路径>\"}")?;
    let src = resolve_in_roots(roots, raw)?;
    if !src.is_file() {
        return Err(format!("路径不是文件: {}", src.display()));
    }
    let text = std::fs::read_to_string(&src).map_err(|e| format!("读取源码失败: {e}"))?;
    let root = roots.first().map(PathBuf::from);
    let cases = args["cases"].as_array().cloned().unwrap_or_default();

    // 按语言分支：非鸿蒙语言生成对应框架的测试骨架，ArkTS 保持原 hypium 逻辑
    let (test_file, content, exports) = match detect_test_lang(&src) {
        "node" => build_node_test(&src, &text, &cases)?,
        "python" => build_python_test(&src, root.as_deref(), &text, &cases)?,
        "go" => build_go_test(&src, &text, &cases)?,
        "rust" => build_rust_test(&src, &text, &cases)?,
        "java" => build_java_test(&src, root.as_deref(), &text, &cases)?,
        _ => {
            // ArkTS/hypium（原逻辑）
            let module_root = find_module_root(&src)
                .ok_or_else(|| "未定位到鸿蒙模块根（未找到 module.json5），无法确定 src/test 位置".to_string())?;
            let exports = extract_exports(&text);
            let (stem, content) = build_test_content(&src, &module_root, &exports, &cases);
            let test_dir = module_root.join("src").join("test");
            std::fs::create_dir_all(&test_dir).map_err(|e| format!("创建测试目录失败: {e}"))?;
            (test_dir.join(format!("{stem}.test.ets")), content, exports)
        }
    };
    if let Some(parent) = test_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建测试目录失败: {e}"))?;
    }
    std::fs::write(&test_file, &content).map_err(|e| format!("写入测试文件失败: {e}"))?;

    let mut out = format!("已生成单元测试：{}\n", test_file.display());
    out.push_str(&format!(
        "识别到 {} 个可测试符号：{}\n",
        exports.len(),
        if exports.is_empty() { "（无）".to_string() } else { exports.join(", ") }
    ));
    out.push_str("\n生成内容预览：\n");
    out.push_str(&tail(&content, 2000));
    out.push_str("\n\n下一步：若需补充真实断言，用 edit_file 修改该文件；完成后用 run_tests 验证。\n");
    Ok(out)
}

/// 语言识别：按源码扩展名判定测试框架（arkts|node|python|go|rust|java）。
pub(super) fn detect_test_lang(src: &Path) -> &'static str {
    let ext = src
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "ets" => "arkts",
        "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" => "node",
        "py" => "python",
        "go" => "go",
        "rs" => "rust",
        "java" => "java",
        _ => "arkts", // 未知按鸿蒙 hypium 处理（兼容原行为）
    }
}

/// 向上查找最近的 package.json（Node 工程根）。
pub(super) fn find_package_root(file: &Path) -> Option<PathBuf> {
    let mut dir = file.parent()?;
    loop {
        if dir.join("package.json").is_file() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// Node（vitest/jest）：与源码同目录生成 *.test.ts/js。
pub(super) fn build_node_test(source: &Path, text: &str, cases: &[Value]) -> Result<(PathBuf, String, Vec<String>), String> {
    let stem = source.file_stem().and_then(|s| s.to_str()).unwrap_or("module").to_string();
    let pkg_root = find_package_root(source).ok_or("未定位到 package.json（Node 工程根）")?;
    let pkg_text = std::fs::read_to_string(pkg_root.join("package.json")).unwrap_or_default();
    let has_jest = pkg_text.contains("jest");
    let exports = extract_exports(text);
    let ext = if matches!(source.extension().and_then(|s| s.to_str()), Some("ts") | Some("tsx")) {
        "test.ts"
    } else {
        "test.js"
    };
    let test_file = source.with_file_name(format!("{stem}.{ext}"));
    let import_from = if has_jest { "@jest/globals" } else { "vitest" };
    let mut body = String::new();
    body.push_str(&format!("import {{ describe, it, expect }} from '{import_from}';\n"));
    if !exports.is_empty() {
        let rel = source
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/");
        body.push_str(&format!("import {{ {} }} from './{}';\n", exports.join(", "), rel));
    }
    body.push('\n');
    body.push_str(&format!("describe('{stem}', () => {{\n"));
    for name in &exports {
        body.push_str(&format!("  it('{name}_should_exist', () => {{\n"));
        body.push_str("    // TODO: 根据实际行为补充断言\n");
        body.push_str(&format!("    expect({name}).toBeDefined();\n"));
        body.push_str("  });\n");
    }
    for c in cases {
        let cname = c["name"].as_str().unwrap_or("case").trim().to_string();
        if cname.is_empty() {
            continue;
        }
        let cname_safe = cname.replace('\\', "\\\\").replace('\'', "\\'");
        let cbody = c["body"].as_str().unwrap_or("");
        body.push_str(&format!("  it('{cname_safe}', () => {{\n"));
        if cbody.trim().is_empty() {
            body.push_str("    // TODO: 补充断言\n    expect(true).toBe(true);\n");
        } else {
            for line in cbody.lines() {
                body.push_str(&format!("    {line}\n"));
            }
        }
        body.push_str("  });\n");
    }
    if exports.is_empty() && cases.is_empty() {
        body.push_str("  it('placeholder', () => {\n    // 未识别到可测试的导出符号，请手动补充测试\n    expect(true).toBe(true);\n  });\n");
    }
    body.push_str("});\n");
    Ok((test_file, body, exports))
}

/// Python 顶层 def/class 符号。
pub(super) fn extract_python_symbols(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        let (prefix, rest) = if t.starts_with("def ") {
            (4, &t[4..])
        } else if t.starts_with("class ") {
            (6, &t[6..])
        } else {
            continue;
        };
        let _ = prefix;
        if let Some(name) = rest.split(|c: char| !(c.is_alphanumeric() || c == '_')).next() {
            if !name.is_empty() && !name.starts_with('_') && !names.contains(&name.to_string()) {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// Python（pytest）：tests/test_*.py（无 tests 目录时与源码同目录）。
pub(super) fn build_python_test(
    source: &Path,
    root: Option<&Path>,
    text: &str,
    cases: &[Value],
) -> Result<(PathBuf, String, Vec<String>), String> {
    let stem = source.file_stem().and_then(|s| s.to_str()).unwrap_or("module").to_string();
    let exports = extract_python_symbols(text);
    // 模块导入路径：相对工程根的正斜杠路径（去扩展名）
    let module_path = root
        .and_then(|r| source.strip_prefix(r).ok())
        .map(|p| p.with_extension("").to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| stem.clone());
    let src_dir = source.parent().ok_or("无法定位源码目录")?;
    let tests_dir = src_dir.join("..").join("tests");
    let target_dir = if tests_dir.is_dir() { tests_dir } else { src_dir.to_path_buf() };
    let test_file = target_dir.join(format!("test_{stem}.py"));

    let mut body = String::new();
    if !exports.is_empty() {
        body.push_str(&format!("from {} import {}\n\n", module_path, exports.join(", ")));
    }
    for name in &exports {
        body.push_str(&format!("def test_{name}_works():\n"));
        body.push_str("    # TODO: 补充真实断言\n");
        body.push_str(&format!("    assert {name} is not None\n\n"));
    }
    for c in cases {
        let cname = c["name"].as_str().unwrap_or("case").trim().to_string();
        if cname.is_empty() {
            continue;
        }
        let safe = cname.replace(|ch: char| !(ch.is_alphanumeric() || ch == '_'), "_");
        let cbody = c["body"].as_str().unwrap_or("");
        body.push_str(&format!("def test_{safe}():\n"));
        if cbody.trim().is_empty() {
            body.push_str("    # TODO: 补充断言\n    assert True\n\n");
        } else {
            for line in cbody.lines() {
                body.push_str(&format!("    {line}\n"));
            }
            body.push('\n');
        }
    }
    if exports.is_empty() && cases.is_empty() {
        body.push_str("def test_placeholder():\n    # 未识别到可测试符号，请手动补充测试\n    assert True\n");
    }
    Ok((test_file, body, exports))
}

/// Go 顶层函数符号（跳过 Test 前缀与私有）。
pub(super) fn extract_go_symbols(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("func ") {
            if let Some(name) = rest.split(|c: char| !(c.is_alphanumeric() || c == '_')).next() {
                if !name.is_empty()
                    && !name.starts_with('_')
                    && !name.starts_with("Test")
                    && !names.contains(&name.to_string())
                {
                    names.push(name.to_string());
                }
            }
        }
    }
    names
}

/// Go：同目录生成 {stem}_test.go（同包测试）。
pub(super) fn build_go_test(source: &Path, text: &str, cases: &[Value]) -> Result<(PathBuf, String, Vec<String>), String> {
    let stem = source.file_stem().and_then(|s| s.to_str()).unwrap_or("module").to_string();
    let pkg = text
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("package ")
                .map(|rest| rest.split_whitespace().next().unwrap_or("main").to_string())
        })
        .unwrap_or_else(|| "main".to_string());
    let exports = extract_go_symbols(text);
    let test_file = source.with_file_name(format!("{stem}_test.go"));

    let mut body = String::new();
    body.push_str(&format!("package {pkg}\n\nimport \"testing\"\n\n"));
    for name in &exports {
        let fname = format!("Test{}{}", name[..1].to_uppercase(), &name[1..]);
        body.push_str(&format!("func {fname}(t *testing.T) {{\n"));
        body.push_str("\t// TODO: 补充真实断言\n");
        body.push_str(&format!("\t_ = {name}\n"));
        body.push_str("}\n\n");
    }
    for c in cases {
        let cname = c["name"].as_str().unwrap_or("case").trim().to_string();
        if cname.is_empty() {
            continue;
        }
        let fname = format!("Test{}", cname);
        let cbody = c["body"].as_str().unwrap_or("");
        body.push_str(&format!("func {fname}(t *testing.T) {{\n"));
        if cbody.trim().is_empty() {
            body.push_str("\t// TODO: 补充断言\n\tt.Error(\"not implemented\")\n");
        } else {
            for line in cbody.lines() {
                body.push_str(&format!("\t{line}\n"));
            }
        }
        body.push_str("}\n\n");
    }
    if exports.is_empty() && cases.is_empty() {
        body.push_str("func TestPlaceholder(t *testing.T) {\n\t// 未识别到可测试符号，请手动补充测试\n}\n");
    }
    Ok((test_file, body, exports))
}

/// Rust 顶层 pub fn / fn 函数符号（跳过私有与测试）。
pub(super) fn extract_rust_symbols(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        let after = t.strip_prefix("pub ").unwrap_or(t);
        if let Some(rest) = after.strip_prefix("fn ") {
            if let Some(name) = rest.split(|c: char| !(c.is_alphanumeric() || c == '_')).next() {
                if !name.is_empty() && !name.starts_with('_') && !names.contains(&name.to_string()) {
                    names.push(name.to_string());
                }
            }
        }
    }
    names
}

/// 向上查找 Cargo.toml 所在目录（crate 根）。
pub(super) fn find_crate_root(source: &Path) -> Option<PathBuf> {
    let mut dir = source.parent()?;
    loop {
        if dir.join("Cargo.toml").is_file() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// 读取 Cargo.toml 的 [package] name（下划线形式）。
pub(super) fn find_crate_name(source: &Path) -> Option<String> {
    let root = find_crate_root(source)?;
    let text = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    for l in text.lines() {
        let t = l.trim();
        if let Some(rest) = t.strip_prefix("name") {
            if let Some(v) = rest.split('=').nth(1) {
                let v = v.trim().trim_matches('"');
                if !v.is_empty() {
                    return Some(v.replace('-', "_"));
                }
            }
        }
    }
    None
}

/// Rust：tests/{stem}_test.rs（集成测试，use crate 名引用被测模块）。
pub(super) fn build_rust_test(source: &Path, text: &str, cases: &[Value]) -> Result<(PathBuf, String, Vec<String>), String> {
    let stem = source.file_stem().and_then(|s| s.to_str()).unwrap_or("module").to_string();
    let exports = extract_rust_symbols(text);
    let crate_name = find_crate_name(source);
    // 测试文件放 crate 根 tests/ 目录（集成测试惯例）
    let crate_root = find_crate_root(source)
        .unwrap_or_else(|| source.parent().unwrap_or(Path::new(".")).to_path_buf());
    let test_file = crate_root.join("tests").join(format!("{stem}_test.rs"));

    let mut body = String::new();
    if let Some(cn) = &crate_name {
        body.push_str(&format!(
            "// 集成测试：被测 crate 为 {cn}（如需单元测试请在 src 内加 #[cfg(test)] mod tests）\n"
        ));
        if !exports.is_empty() {
            body.push_str(&format!("use {cn}::{{{}}};\n\n", exports.join(", ")));
        }
    } else {
        body.push_str("// 未找到 Cargo.toml，无法自动 use 被测 crate，请手动补 import\n\n");
    }
    for name in &exports {
        body.push_str(&format!("#[test]\nfn test_{name}_exists() {{\n"));
        body.push_str("    // TODO: 补充真实断言（当前仅为编译通过占位）\n");
        body.push_str(&format!("    let _ = {name};\n"));
        body.push_str("}\n\n");
    }
    for c in cases {
        let cname = c["name"].as_str().unwrap_or("case").trim().to_string();
        if cname.is_empty() {
            continue;
        }
        let safe = cname.replace(|ch: char| !(ch.is_alphanumeric() || ch == '_'), "_");
        let cbody = c["body"].as_str().unwrap_or("");
        body.push_str(&format!("#[test]\nfn test_{safe}() {{\n"));
        if cbody.trim().is_empty() {
            body.push_str("    // TODO: 补充断言\n    assert!(true);\n");
        } else {
            for line in cbody.lines() {
                body.push_str(&format!("    {line}\n"));
            }
        }
        body.push_str("}\n\n");
    }
    if exports.is_empty() && cases.is_empty() {
        body.push_str("#[test]\nfn placeholder() {\n    // 未识别到可测试符号，请手动补充测试\n    assert!(true);\n}\n");
    }
    Ok((test_file, body, exports))
}

/// Java 类的 public/protected 方法名（跳过 main、构造器与 static 块）。
pub(super) fn extract_java_methods(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        for kw in ["public ", "protected "] {
            let Some(rest) = t.strip_prefix(kw) else { continue };
            if rest.starts_with("static ") || rest.starts_with("class ") || rest.starts_with("interface ") {
                continue;
            }
            let Some(open) = rest.find('(') else { continue };
            let head = &rest[..open];
            let name = head.split_whitespace().last().unwrap_or("");
            if !name.is_empty() && !name.starts_with('_') && !names.contains(&name.to_string()) {
                names.push(name.to_string());
            }
            break;
        }
    }
    names
}

/// Java（JUnit 5）：src/test/java/<包路径>/{Stem}Test.java。
pub(super) fn build_java_test(
    source: &Path,
    root: Option<&Path>,
    text: &str,
    cases: &[Value],
) -> Result<(PathBuf, String, Vec<String>), String> {
    let stem = source.file_stem().and_then(|s| s.to_str()).unwrap_or("Module").to_string();
    let pkg = text
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("package ")
                .map(|rest| rest.trim_end_matches(';').trim().to_string())
        })
        .filter(|p| !p.is_empty());
    let methods = extract_java_methods(text);
    let test_dir = match (root, &pkg) {
        (Some(r), Some(p)) => r.join("src").join("test").join("java").join(p.replace('.', "/")),
        _ => source.parent().unwrap_or(Path::new(".")).to_path_buf(),
    };
    let test_file = test_dir.join(format!("{stem}Test.java"));

    let mut body = String::new();
    if let Some(p) = &pkg {
        body.push_str(&format!("package {p};\n\n"));
    }
    body.push_str("import org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.*;\n\n");
    body.push_str(&format!("public class {stem}Test {{\n\n"));
    for m in &methods {
        body.push_str(&format!("    @Test\n    void {m}_works() {{\n"));
        body.push_str("        // TODO: 补充真实断言\n");
        body.push_str("        assertTrue(true);\n");
        body.push_str("    }\n\n");
    }
    for c in cases {
        let cname = c["name"].as_str().unwrap_or("case").trim().to_string();
        if cname.is_empty() {
            continue;
        }
        let safe = cname.replace(|ch: char| !(ch.is_alphanumeric() || ch == '_'), "_");
        let cbody = c["body"].as_str().unwrap_or("");
        body.push_str(&format!("    @Test\n    void {safe}() {{\n"));
        if cbody.trim().is_empty() {
            body.push_str("        // TODO: 补充断言\n        assertTrue(true);\n");
        } else {
            for line in cbody.lines() {
                body.push_str(&format!("        {line}\n"));
            }
        }
        body.push_str("    }\n\n");
    }
    if methods.is_empty() && cases.is_empty() {
        body.push_str("    @Test\n    void placeholder() {\n        // 未识别到可测试方法，请手动补充测试\n        assertTrue(true);\n    }\n");
    }
    body.push_str("}\n");
    Ok((test_file, body, methods))
}

/// 从源码文件向上定位鸿蒙模块根（包含 module.json5 的目录）。
pub(super) fn find_module_root(file: &Path) -> Option<PathBuf> {
    let mut dir = file.parent()?;
    loop {
        if dir.join("module.json5").is_file() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// 提取源码中可运行时引用的导出符号名（跳过 interface/type/重导出）。
pub(super) fn extract_exports(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("//") || t.starts_with("/*") || t.starts_with('*') {
            continue;
        }
        if t.starts_with("export interface") || t.starts_with("export type") || t.starts_with("export {") {
            continue;
        }
        let Some(after_export) = t.strip_prefix("export ") else { continue };
        let after_default = after_export.strip_prefix("default ").unwrap_or(after_export);
        let after_async = after_default.strip_prefix("async ").unwrap_or(after_default);
        let candidate = after_async
            .strip_prefix("function ")
            .or_else(|| after_async.strip_prefix("class "))
            .or_else(|| after_async.strip_prefix("enum "))
            .or_else(|| after_async.strip_prefix("const "))
            .or_else(|| after_async.strip_prefix("let "))
            .or_else(|| after_async.strip_prefix("var "));
        if let Some(after_kw) = candidate {
            if let Some(name) = after_kw.split(|c: char| !(c.is_alphanumeric() || c == '_')).next() {
                if !name.is_empty() && !names.iter().any(|n| n == name) {
                    names.push(name.to_string());
                }
            }
        }
    }
    names
}

/// 计算 from_dir 到 to 的相对路径（正斜杠分隔，用于 import 语句）。
pub(super) fn relative_to(from_dir: &Path, to: &Path) -> String {
    let from: Vec<_> = from_dir.components().collect();
    let to: Vec<_> = to.components().collect();
    let mut i = 0;
    while i < from.len() && i < to.len() && from[i] == to[i] {
        i += 1;
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in i..from.len() {
        parts.push("..".to_string());
    }
    for c in &to[i..] {
        parts.push(c.as_os_str().to_string_lossy().to_string());
    }
    let joined = parts.join("/");
    if joined.is_empty() { ".".to_string() } else { joined }
}

/// 生成 hypium 测试文件内容，返回（文件 stem，内容）。
pub(super) fn build_test_content(source: &Path, module_root: &Path, exports: &[String], cases: &[Value]) -> (String, String) {
    let stem = source.file_stem().and_then(|s| s.to_str()).unwrap_or("module").to_string();
    let test_dir = module_root.join("src").join("test");
    let rel = relative_to(&test_dir, source);
    let rel_no_ext = rel
        .trim_end_matches(".ets")
        .trim_end_matches(".ts")
        .trim_end_matches(".js")
        .to_string();
    let import_path = rel_no_ext.strip_prefix("./").unwrap_or(&rel_no_ext).to_string();

    let mut body = String::new();
    body.push_str("import { describe, it, expect } from '@ohos/hypium';\n");
    if !exports.is_empty() {
        body.push_str(&format!("import {{ {} }} from '{}';\n", exports.join(", "), import_path));
    } else if !cases.is_empty() {
        body.push_str(&format!("// 未识别到导出符号，可手动 import 被测模块，如：import {{ x }} from '{}';\n", import_path));
    }
    body.push('\n');
    body.push_str(&format!("export default function {}Test() {{\n", stem));
    body.push_str(&format!("  describe('{}', () => {{\n", stem));
    for name in exports {
        body.push_str(&format!("    it('{name}_should_exist', 0, () => {{\n"));
        body.push_str(&format!("      // TODO: 根据 {name} 的实际行为补充断言\n"));
        body.push_str(&format!("      expect({name}).assertNotNull();\n"));
        body.push_str("    });\n");
    }
    for c in cases {
        let cname = c["name"].as_str().unwrap_or("case").trim().to_string();
        if cname.is_empty() {
            continue;
        }
        // 转义反斜杠与单引号：生成代码用单引号字符串，未转义会产出非法代码
        let cname_safe = cname.replace('\\', "\\\\").replace('\'', "\\'");
        let cbody = c["body"].as_str().unwrap_or("");
        body.push_str(&format!("    it('{cname_safe}', 0, () => {{\n"));
        if cbody.trim().is_empty() {
            body.push_str("      // TODO: 补充断言\n      expect(true).assertTrue();\n");
        } else {
            for line in cbody.lines() {
                body.push_str(&format!("      {line}\n"));
            }
        }
        body.push_str("    });\n");
    }
    if exports.is_empty() && cases.is_empty() {
        body.push_str("    it('placeholder', 0, () => {\n      // 未识别到可测试的导出符号，请手动补充测试\n      expect(true).assertTrue();\n    });\n");
    }
    body.push_str("  });\n");
    body.push_str("}\n");
    (stem, body)
}

/// run_ui_flow：在设备上执行一串 UI 操作（uitest uiInput 注入）。
pub(super) async fn run_ui_flow(args: &Value, roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let steps = args["steps"].as_array().ok_or("run_ui_flow 需要参数 {\"steps\":[...]}")?;
    if steps.is_empty() {
        return Err("steps 不能为空".into());
    }
    let results = execute_ui_steps(&device, steps).await;
    let mut out = format!("UI 操作流程（设备 {device}，共 {} 步）：\n", steps.len());
    for r in &results {
        out.push_str(r);
        out.push('\n');
    }

    // 可选：结束后截图供多模态核对
    let verify = args["verify"].as_bool().unwrap_or(false);
    if verify {
        if let Some(project_path) = roots.first() {
            if !project_path.is_empty() {
                match capture_screenshot(project_path, &device).await {
                    Ok((local, _)) => {
                        out.push_str(&format!("\n操作后截图：{}\n[VISION_IMAGE: {}]", local.display(), local.display()));
                    }
                    Err(e) => out.push_str(&format!("\n截图失败：{e}")),
                }
            }
        }
    }
    Ok(out)
}

/// 逐条执行 UI 步骤，返回每步结果描述；任一步失败即停止（避免在错误界面继续乱点）。
pub(super) async fn execute_ui_steps(device: &str, steps: &[Value]) -> Vec<String> {
    let mut results = Vec::new();
    for (i, s) in steps.iter().enumerate() {
        let desc = describe_step(s);
        match execute_ui_step(device, s).await {
            Ok(info) => {
                let suffix = if info.is_empty() { String::new() } else { format!("（{info}）") };
                results.push(format!("{}. {desc} → 成功{suffix}", i + 1));
            }
            Err(e) => {
                results.push(format!("{}. {desc} → 失败：{e}", i + 1));
                results.push("（后续步骤已跳过）".to_string());
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    results
}

/// 执行单个 UI 步骤，返回补充信息（wait 返回等待时长）。
pub(super) async fn execute_ui_step(device: &str, s: &Value) -> Result<String, String> {
    let action = s["action"].as_str().unwrap_or("");
    let mut cmd: Vec<String> = vec!["uitest".to_string(), "uiInput".to_string()];
    match action {
        "tap" | "click" => {
            let x = s["x"].as_i64().unwrap_or(0);
            let y = s["y"].as_i64().unwrap_or(0);
            cmd.extend(["click".to_string(), x.to_string(), y.to_string()]);
        }
        "swipe" => {
            let x1 = s["x1"].as_i64().unwrap_or(0);
            let y1 = s["y1"].as_i64().unwrap_or(0);
            let x2 = s["x2"].as_i64().unwrap_or(0);
            let y2 = s["y2"].as_i64().unwrap_or(0);
            let speed = s["speed"].as_i64().unwrap_or(600);
            cmd.extend(["swipe".to_string(), x1.to_string(), y1.to_string(), x2.to_string(), y2.to_string(), speed.to_string()]);
        }
        "long_press" | "longClick" => {
            let x = s["x"].as_i64().unwrap_or(0);
            let y = s["y"].as_i64().unwrap_or(0);
            cmd.extend(["longClick".to_string(), x.to_string(), y.to_string()]);
        }
        "text" => {
            let t = s["text"].as_str().unwrap_or("");
            if t.is_empty() {
                return Err("text 步骤缺少 text 参数".into());
            }
            cmd.extend(["text".to_string(), t.to_string()]);
        }
        "key" => {
            let name = s["name"].as_str().unwrap_or("back");
            cmd.extend(["keyEvent".to_string(), name.to_string()]);
        }
        "wait" => {
            let ms = s["ms"].as_u64().unwrap_or(500).clamp(1, 30000);
            tokio::time::sleep(Duration::from_millis(ms)).await;
            return Ok(format!("等待 {ms}ms"));
        }
        other => return Err(format!("未知 action: {other}")),
    }
    run_hdc_shell(device, &cmd.iter().map(|s| s.as_str()).collect::<Vec<_>>(), 20)
        .await
        .map_err(|e| format!("uitest 注入失败（确认设备已解锁亮屏且支持 uitest）：{e}"))
}

/// 描述单个 UI 步骤（用于报告展示）。
pub(super) fn describe_step(s: &Value) -> String {
    match s["action"].as_str().unwrap_or("") {
        "tap" | "click" => format!("点击 ({}, {})", s["x"].as_i64().unwrap_or(0), s["y"].as_i64().unwrap_or(0)),
        "swipe" => format!(
            "滑动 ({},{}) → ({},{})",
            s["x1"].as_i64().unwrap_or(0), s["y1"].as_i64().unwrap_or(0),
            s["x2"].as_i64().unwrap_or(0), s["y2"].as_i64().unwrap_or(0)
        ),
        "long_press" | "longClick" => format!("长按 ({}, {})", s["x"].as_i64().unwrap_or(0), s["y"].as_i64().unwrap_or(0)),
        "text" => format!("输入文本「{}」", s["text"].as_str().unwrap_or("")),
        "key" => format!("按键 {}", s["name"].as_str().unwrap_or("back")),
        "wait" => format!("等待 {}ms", s["ms"].as_u64().unwrap_or(500)),
        other => format!("未知操作 {other}"),
    }
}

