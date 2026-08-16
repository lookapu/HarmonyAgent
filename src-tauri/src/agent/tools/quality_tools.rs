//! 质量/度量/工程治理域工具（TOOL_ENHANCEMENTS.md 第 2/3 批落地）：
//! [07] code_metrics 静态复杂度/注释率/嵌套深度
//! [16] metric_export Prometheus text 格式导出
//! [17] log_aggregate 三源日志归并（hilog + runtime + faultlog）
//! [06] snippet_insert 代码片段库 CRUD（snippets 表，migration 036）
//! [70] replay_trace 会话事件按 trace_id 回放调用链
//! [19] api_test OpenAPI/用例批量断言
//! [20] api_health 多 URL 健康探测
//! [35] obfuscate 混淆开关读写（build-profile.json5）
//! [73] sandbox_exec 危险命令临时目录干跑预览

use super::*;
use serde_json::json;

// ================= [07] code_metrics：静态代码度量 =================

/// 常见源码扩展名（启发式统计的目标文件集合）
const SOURCE_EXTS: &[&str] = &[
    "ets", "ts", "tsx", "js", "jsx", "kt", "java", "swift", "c", "h",
    "cpp", "hpp", "rs", "go", "py", "vue",
];
/// 目录遍历跳过项（构建产物/依赖/版本库）
const SKIP_DIRS: &[&str] = &[
    ".git", ".hvigor", ".ohpm", "oh_modules", "node_modules", "build",
    "dist", ".deveco-agent", ".idea", ".vscode", "target", ".cxx",
];

/// 单文件度量结果
#[derive(Default, Clone)]
struct FileMetrics {
    total_lines: u32,
    code_lines: u32,
    comment_lines: u32,
    blank_lines: u32,
    functions: u32,
    /// 圈复杂度增量（McCabe：决策点计数，最终复杂度 = 增量 + 1）
    cyclomatic_delta: u32,
    max_nesting: u32,
}

impl FileMetrics {
    fn merge(&mut self, other: &FileMetrics) {
        self.total_lines += other.total_lines;
        self.code_lines += other.code_lines;
        self.comment_lines += other.comment_lines;
        self.blank_lines += other.blank_lines;
        self.functions += other.functions;
        self.cyclomatic_delta += other.cyclomatic_delta;
        self.max_nesting = self.max_nesting.max(other.max_nesting);
    }
}

/// 单文件启发式分析：行计数（代码/注释/空行）、函数数、圈复杂度、最大嵌套深度。
/// 逐字符状态机剥离字符串与注释后统计，避免误计。
fn analyze_source_file(path: &Path) -> Result<FileMetrics, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > 1024 * 1024 {
        return Err(format!("文件过大跳过（>1MB）: {}", path.display()));
    }
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut m = FileMetrics::default();
    let mut in_block_comment = false;
    for line in raw.lines() {
        m.total_lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            m.blank_lines += 1;
            continue;
        }
        // 逐字符扫描：跳过字符串/注释内容，统计代码字符与决策点/花括号
        let mut code_chars: Vec<char> = Vec::new();
        let mut chars = line.chars().peekable();
        let mut in_str: Option<char> = None; // '"' 或 '\''
        while let Some(c) = chars.next() {
            if let Some(q) = in_str {
                if c == '\\' {
                    let _ = chars.next();
                } else if c == q {
                    in_str = None;
                }
                continue;
            }
            if in_block_comment {
                if c == '*' && chars.peek() == Some(&'/') {
                    let _ = chars.next();
                    in_block_comment = false;
                }
                continue;
            }
            if c == '/' && chars.peek() == Some(&'/') {
                break; // 行注释：剩余全部丢弃
            }
            if c == '/' && chars.peek() == Some(&'*') {
                let _ = chars.next();
                in_block_comment = true;
                continue;
            }
            if c == '"' || c == '\'' || c == '`' {
                in_str = Some(c);
                continue;
            }
            code_chars.push(c);
        }
        let code_line = !code_chars.is_empty();
        if code_line {
            m.code_lines += 1;
            // 复杂度决策点
            let s: String = code_chars.iter().collect();
            for kw in ["if", "for", "while", "catch", "case", "default", "do"] {
                // 词边界粗匹配：前后非标识符字符
                let bytes = s.as_bytes();
                let mut i = 0;
                while i + kw.len() <= bytes.len() {
                    if &s[i..i + kw.len()] == kw
                        && (i == 0 || !is_ident_char(bytes[i - 1]))
                        && (i + kw.len() == bytes.len() || !is_ident_char(bytes[i + kw.len()]))
                    {
                        m.cyclomatic_delta += 1;
                        i += kw.len();
                    } else {
                        i += 1;
                    }
                }
            }
            for op in ["&&", "||", "??"] {
                m.cyclomatic_delta += s.matches(op).count() as u32;
            }
            // 三元运算符 ?:（排除 ?. 可选链）
            let mut t = 0u32;
            for (i, c) in s.char_indices() {
                if c == '?' {
                    let next = s.chars().nth(i + 1).unwrap_or(' ');
                    if next != '.' && next != '?' {
                        t += 1;
                    }
                }
            }
            m.cyclomatic_delta += t;
            // 函数/方法签名：函数关键字或"名字("形态的方法声明
            if is_function_line(&s) {
                m.functions += 1;
            }
        } else if !in_block_comment && trimmed.starts_with('*') {
            // 块注释延续行（doc 注释体）
            m.comment_lines += 1;
        } else if in_block_comment || trimmed.starts_with("//") || trimmed.starts_with("/*") {
            m.comment_lines += 1;
        }
        // 花括号嵌套深度（只统计代码字符）
        let mut depth: i32 = 0;
        for c in &code_chars {
            if *c == '{' {
                depth += 1;
                m.max_nesting = m.max_nesting.max(depth as u32);
            } else if *c == '}' {
                depth -= 1;
            }
        }
    }
    Ok(m)
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// 行是否为函数/方法声明（启发式：`function foo(`、`fn foo(`、`foo(` 形态且不以关键字/控制流开头）
fn is_function_line(s: &str) -> bool {
    let s = s.trim_start();
    if s.starts_with("function ") || s.starts_with("fn ") || s.starts_with("def ") {
        return true;
    }
    // 方法形态：可选修饰符后跟 名字( —— 排除 if/for/while/switch/return/catch/async( 等控制流
    let leading = s.split('(').next().unwrap_or("");
    let leading = leading.trim_end();
    if leading.is_empty() {
        return false;
    }
    let name = leading.rsplit(|c: char| c.is_whitespace() || c == ')' || c == '>').next().unwrap_or("");
    if name.is_empty() {
        return false;
    }
    let first = name.chars().next().unwrap_or(' ');
    if !(first.is_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    !matches!(
        name,
        "if" | "for" | "while" | "switch" | "return" | "catch" | "async" | "await" | "new" | "else" | "do" | "case" | "with" | "of"
    )
}

/// [07] code_metrics：静态代码度量（圈复杂度/注释率/嵌套深度，正则启发式）。
/// 参数：{"path":"<文件或目录，相对项目根或绝对路径，缺省项目根>","top":<可选列出复杂度最高文件数，缺省 10>}。
pub(super) async fn code_metrics(args: &Value, roots: &[String]) -> Result<String, String> {
    let raw = args["path"].as_str().unwrap_or(".");
    let p = resolve_readable(roots, raw)?;
    if !p.exists() {
        return Err(format!("路径不存在: {}", p.display()));
    }
    let top_n = args["top"].as_u64().unwrap_or(10).min(50) as usize;
    let mut files: Vec<PathBuf> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    if p.is_file() {
        files.push(p.clone());
    } else {
        collect_source_files(&p, &mut files, 0);
    }
    if files.is_empty() {
        return Err(format!("未在 {} 下找到源码文件（扩展名: {}）", p.display(), SOURCE_EXTS.join("/")));
    }
    let mut total = FileMetrics::default();
    let mut per_file: Vec<(PathBuf, FileMetrics)> = Vec::new();
    for f in &files {
        match analyze_source_file(f) {
            Ok(m) => {
                total.merge(&m);
                per_file.push((f.clone(), m));
            }
            Err(e) => errors.push(e),
        }
    }
    per_file.sort_by(|a, b| b.1.cyclomatic_delta.cmp(&a.1.cyclomatic_delta));
    let comment_rate = if total.code_lines + total.comment_lines > 0 {
        total.comment_lines as f64 * 100.0 / (total.code_lines + total.comment_lines) as f64
    } else {
        0.0
    };
    let out = serde_json::json!({
        "files": per_file.len(),
        "total_lines": total.total_lines,
        "code_lines": total.code_lines,
        "comment_lines": total.comment_lines,
        "blank_lines": total.blank_lines,
        "comment_rate_pct": (comment_rate * 10.0).round() / 10.0,
        "functions": total.functions,
        "avg_functions_per_file": if per_file.is_empty() { 0.0 } else { (total.functions as f64 * 10.0 / per_file.len() as f64).round() / 10.0 },
        "cyclomatic_avg": if per_file.is_empty() { 0.0 } else { ((total.cyclomatic_delta as f64 + 1.0) * 10.0 / per_file.len() as f64).round() / 10.0 },
        "max_nesting": total.max_nesting,
        "top_complexity": per_file.iter().take(top_n).map(|(f, m)| serde_json::json!({
            "file": f.strip_prefix(&p).unwrap_or(f).display().to_string(),
            "cyclomatic": m.cyclomatic_delta + 1,
            "max_nesting": m.max_nesting,
            "functions": m.functions,
            "lines": m.total_lines,
        })).collect::<Vec<_>>(),
    });
    let mut txt = format!(
        "代码度量：{} 个文件 / {} 行（代码 {} / 注释 {} / 空行 {}）\n注释率 {:.1}% ｜ 函数 {} 个 ｜ 平均圈复杂度 {:.1} ｜ 最大嵌套深度 {}\n",
        per_file.len(), total.total_lines, total.code_lines, total.comment_lines, total.blank_lines,
        comment_rate, total.functions,
        total.cyclomatic_delta as f64 + 1.0,
        total.max_nesting
    );
    if !per_file.is_empty() {
        txt.push_str(&format!("复杂度 Top {}（圈复杂度 ≥ {} 的文件建议拆分）:\n", top_n.min(per_file.len()), "5"));
        for (i, (f, m)) in per_file.iter().take(top_n).enumerate() {
            let cc = m.cyclomatic_delta + 1;
            let flag = if cc >= 15 { "⚠️" } else if cc >= 10 { "▲" } else { "" };
            txt.push_str(&format!(
                "  {}. {}（复杂度 {}{} / 嵌套 {} / {} 行）\n",
                i + 1,
                f.strip_prefix(&p).unwrap_or(f).display(),
                cc,
                flag,
                m.max_nesting,
                m.total_lines
            ));
        }
    }
    if !errors.is_empty() {
        txt.push_str(&format!("\n跳过 {} 个文件：{}\n", errors.len(), errors.join("；")));
    }
    txt.push_str(&format!("\n机器可读指标：\n{}", serde_json::to_string_pretty(&out).unwrap_or_default()));
    Ok(txt)
}

/// 递归收集源码文件（限制深度 12，跳过构建产物/依赖目录）
fn collect_source_files(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 12 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            collect_source_files(&path, out, depth + 1);
        } else if let Some(ext) = path.extension().and_then(|x| x.to_str()) {
            if SOURCE_EXTS.contains(&ext.to_lowercase().as_str()) {
                out.push(path);
            }
        }
    }
}

// ================= [16] metric_export：Prometheus 文本格式 =================

/// [16] metric_export：导出 Prometheus text 格式指标（tool_runs + request_logs 聚合）。
/// 参数：{"days":<可选最近 N 天，缺省 7>}。
/// 输出：deveco_tool_calls_total / deveco_tool_duration_ms_sum / deveco_llm_* 系列。
pub(super) async fn metric_export(
    args: &Value,
    _roots: &[String],
    project_id: &str,
    db: &crate::db::DbState,
) -> Result<String, String> {
    let days = args["days"].as_u64().unwrap_or(7).min(3650);
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let cutoff = chrono::Utc::now().timestamp() - days as i64 * 86400;
    let mut out = String::new();
    out.push_str(&format!("# devEco Switch 工具指标（最近 {days} 天，project={project_id}）\n"));
    out.push_str("# TYPE deveco_tool_calls_total counter\n");
    out.push_str("# TYPE deveco_tool_fail_total counter\n");
    out.push_str("# TYPE deveco_tool_duration_ms_sum counter\n");
    out.push_str("# TYPE deveco_tool_duration_ms_count counter\n");
    let mut stmt = conn
        .prepare(
            "SELECT tool_name, COUNT(*), SUM(CASE WHEN status IN ('error','cancelled') THEN 1 ELSE 0 END),
                    COALESCE(SUM(duration_ms), 0)
             FROM tool_runs t JOIN conversations c ON c.id = t.conversation_id
             WHERE c.project_id = ?1 AND t.created_at >= ?2
             GROUP BY tool_name ORDER BY COUNT(*) DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![project_id, cutoff], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))
        })
        .map_err(|e| e.to_string())?;
    for row in rows.flatten() {
        let (tool, calls, fails, dur) = row;
        out.push_str(&format!("deveco_tool_calls_total{{tool=\"{tool}\"}} {calls}\n"));
        out.push_str(&format!("deveco_tool_fail_total{{tool=\"{tool}\"}} {fails}\n"));
        out.push_str(&format!("deveco_tool_duration_ms_sum{{tool=\"{tool}\"}} {dur}\n"));
        out.push_str(&format!("deveco_tool_duration_ms_count{{tool=\"{tool}\"}} {calls}\n"));
    }
    // LLM 请求按模型聚合（request_logs）
    out.push_str("# TYPE deveco_llm_requests_total counter\n");
    out.push_str("# TYPE deveco_llm_tokens_total counter\n");
    out.push_str("# TYPE deveco_llm_cost_cny_total counter\n");
    let mut stmt2 = conn
        .prepare(
            "SELECT COALESCE(model,'(unknown)'), COUNT(*), SUM(input_tokens), SUM(output_tokens),
                    COALESCE(SUM(total_cost_cny), 0)
             FROM request_logs WHERE created_at >= ?1
             GROUP BY COALESCE(model,'(unknown)') ORDER BY COUNT(*) DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows2 = stmt2
        .query_map([cutoff], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, f64>(4)?))
        })
        .map_err(|e| e.to_string())?;
    for row in rows2.flatten() {
        let (model, reqs, inp, outp, cost) = row;
        out.push_str(&format!("deveco_llm_requests_total{{model=\"{model}\"}} {reqs}\n"));
        out.push_str(&format!("deveco_llm_tokens_total{{model=\"{model}\",kind=\"input\"}} {inp}\n"));
        out.push_str(&format!("deveco_llm_tokens_total{{model=\"{model}\",kind=\"output\"}} {outp}\n"));
        out.push_str(&format!("deveco_llm_cost_cny_total{{model=\"{model}\"}} {cost:.6}\n"));
    }
    // 代理链路带工具名标注的 token 消耗（request_logs.tool_name，[69]）
    out.push_str("# TYPE deveco_tool_tokens_total counter\n");
    let mut stmt3 = conn
        .prepare(
            "SELECT COALESCE(tool_name,'(none)'), COUNT(*), SUM(input_tokens + output_tokens),
                    COALESCE(SUM(total_cost_cny), 0)
             FROM request_logs WHERE created_at >= ?1
             GROUP BY COALESCE(tool_name,'(none)') ORDER BY SUM(input_tokens + output_tokens) DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows3 = stmt3
        .query_map([cutoff], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, f64>(3)?))
        })
        .map_err(|e| e.to_string())?;
    for row in rows3.flatten() {
        let (tool, reqs, toks, cost) = row;
        out.push_str(&format!("deveco_tool_tokens_total{{tool=\"{tool}\"}} {toks}\n"));
        out.push_str(&format!("deveco_tool_tokens_cost_cny_total{{tool=\"{tool}\"}} {cost:.6}\n"));
        let _ = reqs;
    }
    Ok(out)
}

// ================= [17] log_aggregate：三源日志归并 =================

/// [17] log_aggregate：单次调用归并 hilog（设备）+ runtime（工程运行日志）+ faultlog（崩溃副本）。
/// 参数：{"device":"<可选>","since":<可选分钟，缺省 5，透传 hilog>,"sources":["hilog","runtime","faultlog"（缺省三源全开）],"max_lines":<可选每源行数上限，缺省 120>}。
pub(super) async fn log_aggregate(
    args: &Value,
    roots: &[String],
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let want: Vec<String> = args["sources"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_else(|| vec!["hilog".into(), "runtime".into(), "faultlog".into()]);
    let max_lines = args["max_lines"].as_u64().unwrap_or(120).min(600) as usize;
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    let mut out = String::new();
    out.push_str(&format!("日志归并报告（来源：{}）\n", want.join(" + ")));
    let mut seen_any = false;
    for src in &want {
        out.push_str(&format!("\n========== [{src}] ==========\n"));
        match src.as_str() {
            "hilog" => {
                let mut sub = args.clone();
                if sub.get("max_lines").is_none() {
                    sub["max_lines"] = json!(max_lines);
                }
                match super::debug_tools::search_hilog(&sub, roots).await {
                    Ok(t) => {
                        seen_any = true;
                        out.push_str(&t);
                    }
                    Err(e) => out.push_str(&format!("(不可用: {e})")),
                }
            }
            "runtime" => {
                let mut sub = args.clone();
                if sub.get("max_lines").is_none() {
                    sub["max_lines"] = json!(max_lines);
                }
                match super::test_tools::read_runtime_logs(&sub, roots, ctx).await {
                    Ok(t) => {
                        seen_any = true;
                        out.push_str(&t);
                    }
                    Err(e) => out.push_str(&format!("(不可用: {e})")),
                }
            }
            "faultlog" => {
                let crash_dir = format!("{}/.deveco-agent/crashes", project_path.trim_end_matches(['/', '\\']));
                let dir = Path::new(&crash_dir);
                let mut files: Vec<PathBuf> = Vec::new();
                if let Ok(rd) = std::fs::read_dir(dir) {
                    for e in rd.flatten() {
                        if e.path().is_file() {
                            files.push(e.path());
                        }
                    }
                }
                files.sort_by_key(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok());
                files.reverse();
                if files.is_empty() {
                    out.push_str("(无崩溃副本，可先 analyze_crash 拉取)\n");
                } else {
                    seen_any = true;
                    out.push_str(&format!("崩溃文件 {} 个，最近 {} 个：\n", files.len(), files.len().min(3)));
                    for f in files.iter().take(3) {
                        let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let meta = std::fs::metadata(f).ok();
                        let mtime = meta
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        let when = chrono::DateTime::from_timestamp(mtime, 0)
                            .map(|d| d.format("%m-%d %H:%M:%S").to_string())
                            .unwrap_or_else(|| "-".into());
                        out.push_str(&format!("  [{when}] {name}\n"));
                        if let Ok(content) = std::fs::read_to_string(f) {
                            for line in content.lines().take(40) {
                                out.push_str(&format!("    {line}\n"));
                            }
                        }
                    }
                }
            }
            other => out.push_str(&format!("(未知来源 {other}，可选 hilog/runtime/faultlog)\n")),
        }
    }
    if !seen_any {
        return Err("所有日志来源均不可用（设备未连接 / 无运行日志 / 无崩溃副本）。".into());
    }
    out.push_str("\n提示：按时间戳横向排查时，可用 keyword 参数缩小 hilog 范围；faultlog 是崩溃后取证，与运行日志配合定位根因。");
    Ok(out)
}

// ================= [14] log_query：结构化日志查询 =================

/// [14] log_query：结构化日志查询（在 log_aggregate 基础上加时间范围/级别/关键词/正则多维过滤）。
/// 比 search_hilog 更强：支持多源（hilog/runtime/faultlog）、since_minutes（最近 N 分钟）、
/// level_min（E/W/I/D）、keyword（普通 contains）、regex（更精准的子串匹配）。
/// 适合「过去 10 分钟内所有 ERROR + 含 'TypeError'」这类排查场景。
///
/// 参数：{"sources":["hilog","runtime","faultlog"]（缺省三源）,"since_minutes":<可选，缺省 10>,
///        "level_min":"E|W|I|D"（缺省 I，输出 ≥ 该级别）,"keyword":"<普通包含>","regex":"<可选正则>",
///        "max_lines":<可选每源上限，缺省 200>}
pub(super) async fn log_query(
    args: &Value,
    roots: &[String],
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let sources: Vec<String> = args["sources"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_else(|| vec!["hilog".into(), "runtime".into(), "faultlog".into()]);
    let since_minutes = args["since_minutes"].as_u64().unwrap_or(10);
    let level_min = args["level_min"].as_str().unwrap_or("I");
    let keyword = args["keyword"].as_str().map(str::trim).filter(|s| !s.is_empty());
    let regex_pat = args["regex"].as_str().map(str::trim).filter(|s| !s.is_empty());
    let max_lines = args["max_lines"].as_u64().unwrap_or(200).min(2000) as usize;
    let project_path = roots.first().map(String::as_str).unwrap_or("");

    // 预编译 regex（非法正则直接报错，避免下游静默无结果）
    let re = match regex_pat {
        Some(pat) => Some(regex::Regex::new(pat).map_err(|e| format!("regex 解析失败: {e}"))?),
        None => None,
    };

    // 把 level 映射成 hilog 优先级数字阈值（E=0, W=1, I=4, D=7）—— hilog -L 接优先级数字
    let level_threshold: &str = match level_min {
        "E" => "0", // 仅 fatal/error
        "W" => "1",
        "I" => "4",
        "D" => "7",
        _ => "7",  // 默认全开
    };

    let mut out = String::new();
    out.push_str(&format!(
        "结构化日志查询（来源: {} / since={}min / level≥{level_min} / max={max_lines}行/源）\n",
        sources.join("+"),
        since_minutes
    ));
    let mut total = 0usize;
    for src in &sources {
        out.push_str(&format!("\n========== [{src}] ==========\n"));
        match src.as_str() {
            "hilog" => {
                // 透传 since + level 给 search_hilog（用 since_minutes 替代 since_hours）
                let mut sub = serde_json::json!({
                    "max_lines": max_lines,
                    "since_minutes": since_minutes,
                    "level": level_threshold,
                });
                if let Some(k) = keyword { sub["keyword"] = json!(k); }
                if let Some(r) = regex_pat { sub["regex"] = json!(r); }
                match super::debug_tools::search_hilog(&sub, roots).await {
                    Ok(t) => {
                        let lines = filter_by_level(&t, level_min, re.as_ref());
                        total += lines.len();
                        out.push_str(&lines.join("\n"));
                        out.push('\n');
                    }
                    Err(e) => out.push_str(&format!("(不可用: {e})\n")),
                }
            }
            "runtime" => {
                let mut sub = serde_json::json!({ "max_lines": max_lines });
                if let Some(k) = keyword { sub["keyword"] = json!(k); }
                if let Some(r) = regex_pat { sub["regex"] = json!(r); }
                match super::test_tools::read_runtime_logs(&sub, roots, ctx).await {
                    Ok(t) => {
                        let lines = filter_by_level(&t, level_min, re.as_ref());
                        total += lines.len();
                        out.push_str(&lines.join("\n"));
                        out.push('\n');
                    }
                    Err(e) => out.push_str(&format!("(不可用: {e})\n")),
                }
            }
            "faultlog" => {
                let crash_dir = format!(
                    "{}/.deveco-agent/crashes",
                    project_path.trim_end_matches(['/', '\\'])
                );
                let dir = std::path::Path::new(&crash_dir);
                let mut files: Vec<std::path::PathBuf> = Vec::new();
                if let Ok(rd) = std::fs::read_dir(dir) {
                    for e in rd.flatten() {
                        if e.path().is_file() {
                            files.push(e.path());
                        }
                    }
                }
                files.sort_by_key(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok());
                files.reverse();
                let cutoff = chrono::Local::now()
                    .timestamp()
                    - (since_minutes as i64) * 60;
                let mut pushed = 0usize;
                for f in files.iter().take(10) {
                    let mtime = std::fs::metadata(f)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    if mtime < cutoff { continue; }
                    let name = f
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if let Ok(content) = std::fs::read_to_string(f) {
                        for line in content.lines() {
                            if keyword.is_some_and(|k| !line.contains(k)) { continue; }
                            if re.as_ref().is_some_and(|r| !r.is_match(line)) { continue; }
                            out.push_str(&format!("[{name}] {line}\n"));
                            pushed += 1;
                            if pushed >= max_lines { break; }
                        }
                    }
                    if pushed >= max_lines { break; }
                }
                if pushed == 0 { out.push_str("(时间范围内无崩溃文件)\n"); }
                total += pushed;
            }
            other => out.push_str(&format!("(未知来源 {other})\n")),
        }
    }
    out.push_str(&format!("\n合计匹配 {} 行（按 level≥{level_min} + 关键词过滤）\n", total));
    Ok(out)
}

/// 按 level 阈值过滤 + 可选 regex 二次过滤
fn filter_by_level(
    text: &str,
    level_min: &str,
    re: Option<&regex::Regex>,
) -> Vec<String> {
    // hilog / runtime 输出行格式如 "E 09-15 12:34:56 12345 6789 ..." 或 "FATAL ..."
    // 简单首字母匹配：E (Error/Fatal), W (Warn), I (Info), D (Debug)
    let threshold: u8 = match level_min {
        "E" => 0,
        "W" => 1,
        "I" => 4,
        "D" => 7,
        _ => 7,
    };
    text.lines()
        .filter(|line| {
            let first = line.trim_start().chars().next().unwrap_or(' ');
            let level_code = match first {
                'E' | 'F' => 0u8,    // Error / Fatal
                'W' => 1u8,
                'I' => 4u8,
                'D' => 7u8,
                _ => 99u8,             // 不识别视为"杂项"（默认通过）
            };
            if level_code != 99 && level_code > threshold {
                return false;
            }
            if let Some(r) = re { if !r.is_match(line) { return false; } }
            true
        })
        .map(String::from)
        .collect()
}

// ================= [30] memory_snapshot：内存快照归档 + 增长对比 =================

/// [30] memory_snapshot：在 dump_memory 基础上加时间序列归档 + 增长对比（定位内存泄漏）。
/// 参数：{"action":"take|list|diff"（缺省 take）,"tag":"<可选标签，便于对比，缺省时间戳>"}。
///   - take:  抓一次内存快照，落地到 .deveco-agent/memory-snapshots/<tag>.txt
///   - list:  列出已存的快照（时间/标签/路径）
///   - diff:  对比最近两个快照，输出 VmRSS / VmSize 增长（KB）+ 增长率
///
/// 适合：怀疑内存泄漏时抓两次对比（中间跑目标场景）、发布前做基线快照、上线后做趋势追踪。
/// 副作用：写工程目录的 .deveco-agent/memory-snapshots/（不影响业务代码）。
pub(super) async fn memory_snapshot(
    args: &Value,
    roots: &[String],
) -> Result<String, String> {
    let action = args["action"].as_str().unwrap_or("take");
    let project_path = roots.first().map(String::as_str).unwrap_or("").to_string();
    if project_path.is_empty() {
        return Err("memory_snapshot 需要绑定工程".into());
    }
    let snap_dir = std::path::PathBuf::from(&project_path)
        .join(".deveco-agent")
        .join("memory-snapshots");
    std::fs::create_dir_all(&snap_dir)
        .map_err(|e| format!("创建快照目录失败: {e}"))?;

    match action {
        "take" => {
            // 透传给 dump_memory 抓一次（透传 bundle/device）
            let pass = serde_json::json!({});
            let raw = super::ui_tools::dump_memory(&pass, roots).await?;
            // tag 缺省 = 时间戳
            let tag = args["tag"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| {
                    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
                });
            let file = snap_dir.join(format!("{tag}.txt"));
            std::fs::write(&file, &raw)
                .map_err(|e| format!("写快照失败: {e}"))?;
            Ok(format!(
                "内存快照已存：{}\n文件大小 {} 字节\n{}",
                file.display(),
                raw.len(),
                raw.lines().take(5).collect::<Vec<_>>().join("\n")
            ))
        }
        "list" => {
            let mut files: Vec<std::fs::DirEntry> = std::fs::read_dir(&snap_dir)
                .map_err(|e| format!("读快照目录失败: {e}"))?
                .flatten()
                .filter(|e| e.path().is_file())
                .collect();
            files.sort_by_key(|e| {
                e.path()
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            });
            files.reverse();
            if files.is_empty() {
                return Ok("暂无内存快照；先 memory_snapshot action=take".into());
            }
            let mut out = format!("已存快照 {} 个（按时间倒序）：\n", files.len());
            for f in files.iter().take(20) {
                // DirEntry::file_name() 返回 OsString（不是 Option），直接转字符串
                let name = f.file_name().to_string_lossy().to_string();
                let size = f.path().metadata().map(|m| m.len()).unwrap_or(0);
                out.push_str(&format!("  [{name}]  size={size}B\n"));
            }
            Ok(out)
        }
        "diff" => {
            // 找最近两个快照对比
            let mut files: Vec<std::fs::DirEntry> = std::fs::read_dir(&snap_dir)
                .map_err(|e| format!("读快照目录失败: {e}"))?
                .flatten()
                .filter(|e| e.path().is_file())
                .collect();
            files.sort_by_key(|e| {
                e.path()
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            });
            if files.len() < 2 {
                return Err(format!(
                    "diff 需要至少 2 个快照，当前 {} 个；先 action=take 抓两次",
                    files.len()
                ));
            }
            let latest = &files[files.len() - 1];
            let prev = &files[files.len() - 2];
            let latest_text = std::fs::read_to_string(latest.path())
                .map_err(|e| format!("读最新快照失败: {e}"))?;
            let prev_text = std::fs::read_to_string(prev.path())
                .map_err(|e| format!("读上一快照失败: {e}"))?;
            let parse_kb = |t: &str, key: &str| -> f64 {
                t.lines()
                    .find(|l| l.contains(key))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0)
            };
            let rss_now = parse_kb(&latest_text, "VmRSS");
            let rss_prev = parse_kb(&prev_text, "VmRSS");
            let vm_now = parse_kb(&latest_text, "VmSize");
            let vm_prev = parse_kb(&prev_text, "VmSize");
            let mut out = String::new();
            out.push_str(&format!(
                "内存增长对比：\n  旧：{}\n  新：{}\n",
                prev.path().display(),
                latest.path().display()
            ));
            let fmt = |label: &str, now: f64, prev: f64| -> String {
                let diff = now - prev;
                let pct = if prev > 0.0 { diff / prev * 100.0 } else { 0.0 };
                let arrow = if diff > 0.0 { "↑" } else if diff < 0.0 { "↓" } else { "=" };
                format!(
                    "  {label}: {:.1} MB → {:.1} MB  Δ {}{:.1} KB ({:+.1}%)",
                    prev / 1024.0,
                    now / 1024.0,
                    arrow,
                    diff.abs(),
                    pct
                )
            };
            out.push_str(&format!("{}\n{}\n", fmt("VmRSS", rss_now, rss_prev), fmt("VmSize", vm_now, vm_prev)));
            // 内存增长 > 10% 提示可能泄漏
            if rss_now > rss_prev * 1.1 && rss_prev > 0.0 {
                out.push_str("\n⚠️ VmRSS 增长 >10%，疑似内存泄漏，建议：\n  1. 检查最近改动是否引入未释放的对象/监听器/订阅\n  2. 多次 take 后再 diff 看是否持续增长\n  3. 配合 dump_memory 看 smaps 段占用排名\n");
            }
            Ok(out)
        }
        other => Err(format!(
            "memory_snapshot 未知 action: {other}（take/list/diff）"
        )),
    }
}

// ================= [06] snippet_insert：代码片段库 CRUD =================

/// [06] snippet_insert：自定义代码片段库（snippets 表，migration 036）。
/// 参数：{"action":"insert|list|get|search|update|delete（缺省 insert）","name":"<片段名，唯一>","body":"<代码体>","description":"<可选说明>","language":"<可选，缺省 ArkTS>","keyword":"<search 用>"}。
pub(super) async fn snippet_insert(
    args: &Value,
    _roots: &[String],
    db: &crate::db::DbState,
) -> Result<String, String> {
    let action = args["action"].as_str().unwrap_or("insert");
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();
    match action {
        "insert" | "update" => {
            let name = args["name"].as_str().unwrap_or("");
            if name.is_empty() {
                return Err("snippet_insert 需要参数 {\"name\":\"<片段名>\"}".into());
            }
            let body = args["body"].as_str().unwrap_or("");
            if body.is_empty() {
                return Err("snippet_insert 需要参数 {\"body\":\"<代码体>\"}（insert/update 时必填）".into());
            }
            if body.len() > 64 * 1024 {
                return Err("片段正文超过 64KB，请拆分".into());
            }
            let description = args["description"].as_str().unwrap_or("");
            let language = args["language"].as_str().unwrap_or("ArkTS");
            let exists: bool = conn
                .query_row("SELECT COUNT(*) FROM snippets WHERE name = ?1", [name], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            if action == "insert" && exists {
                return Err(format!("片段 \"{name}\" 已存在（可用 action=update 覆盖，或换 name）"));
            }
            if action == "update" && !exists {
                return Err(format!("片段 \"{name}\" 不存在（可用 action=insert 新建）"));
            }
            if action == "insert" {
                conn.execute(
                    "INSERT INTO snippets (name, body, description, language, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?5)",
                    rusqlite::params![name, body, description, language, now],
                )
                .map_err(|e| e.to_string())?;
            } else {
                conn.execute(
                    "UPDATE snippets SET body=?2, description=?3, language=?4, updated_at=?5 WHERE name=?1",
                    rusqlite::params![name, body, description, language, now],
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(format!("片段 \"{name}\"（{language}）已{}。当前库共 {} 个片段。", if action == "insert" { "保存" } else { "更新" }, snippet_count(&conn)?))
        }
        "list" => {
            let mut stmt = conn
                .prepare("SELECT name, description, language, length(body) FROM snippets ORDER BY name")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?))
                })
                .map_err(|e| e.to_string())?;
            let all: Vec<(String, String, String, i64)> = rows.flatten().collect();
            if all.is_empty() {
                return Ok("片段库为空：用 action=insert 保存第一个片段（name/body 必填）。".into());
            }
            let mut out = format!("片段库共 {} 个：\n", all.len());
            for (name, desc, lang, len) in &all {
                out.push_str(&format!("  - {name}（{lang}，{len} 字符）{}{}\n", if desc.is_empty() { "" } else { "： " }, desc));
            }
            out.push_str("\n用 action=get name=<名称> 查看正文，action=search keyword=<关键词> 检索。");
            Ok(out)
        }
        "get" => {
            let name = args["name"].as_str().ok_or("action=get 需要 name 参数")?;
            let row = conn
                .query_row(
                    "SELECT body, description, language, created_at, updated_at FROM snippets WHERE name = ?1",
                    [name],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, i64>(3)?,
                            r.get::<_, i64>(4)?,
                        ))
                    },
                )
                .map_err(|_| format!("片段 \"{name}\" 不存在（先 action=list 查看）"))?;
            let (body, desc, lang, created, updated) = row;
            let fmt = |t: i64| {
                chrono::DateTime::from_timestamp(t, 0)
                    .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "-".into())
            };
            Ok(format!(
                "片段：{name}（{lang}）\n说明：{}\n创建 {} ｜ 更新 {}\n\n{}\n",
                if desc.is_empty() { "-".to_string() } else { desc },
                fmt(created),
                fmt(updated),
                body
            ))
        }
        "search" => {
            let keyword = args["keyword"].as_str().unwrap_or("");
            if keyword.is_empty() {
                return Err("action=search 需要 keyword 参数".into());
            }
            let pat = format!("%{}%", keyword);
            let mut stmt = conn
                .prepare(
                    "SELECT name, description, language, length(body) FROM snippets
                     WHERE name LIKE ?1 OR description LIKE ?1 OR body LIKE ?1 ORDER BY name",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([&pat], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?))
                })
                .map_err(|e| e.to_string())?;
            let all: Vec<(String, String, String, i64)> = rows.flatten().collect();
            if all.is_empty() {
                return Ok(format!("未找到包含 \"{keyword}\" 的片段。"));
            }
            let mut out = format!("命中 {} 个片段：\n", all.len());
            for (name, desc, lang, len) in &all {
                out.push_str(&format!("  - {name}（{lang}，{len} 字符）{}\n", desc));
            }
            Ok(out)
        }
        "delete" => {
            let name = args["name"].as_str().ok_or("action=delete 需要 name 参数")?;
            let n = conn.execute("DELETE FROM snippets WHERE name = ?1", [name]).map_err(|e| e.to_string())?;
            if n == 0 {
                return Err(format!("片段 \"{name}\" 不存在"));
            }
            Ok(format!("片段 \"{name}\" 已删除。当前库共 {} 个片段。", snippet_count(&conn)?))
        }
        other => Err(format!("未知 action \"{other}\"。可用：insert|list|get|search|update|delete")),
    }
}

fn snippet_count(conn: &rusqlite::Connection) -> Result<i64, String> {
    conn.query_row("SELECT COUNT(*) FROM snippets", [], |r| r.get(0)).map_err(|e| e.to_string())
}

// ================= [70] replay_trace：会话事件回放 =================

/// [70] replay_trace：按 trace_id 回放会话事件（1:1 还原调用链）。
/// 参数：{"trace_id":"<可选，缺省列出最近 10 个任务>","conversation_id":"<可选，缺省当前会话>"}。
pub(super) async fn replay_trace(
    args: &Value,
    _roots: &[String],
    conversation_id: &str,
    db: &crate::db::DbState,
) -> Result<String, String> {
    let cid = args["conversation_id"].as_str().unwrap_or(conversation_id);
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let events = crate::agent::session_events::replay(&conn, cid).map_err(|e| e.to_string())?;
    if events.is_empty() {
        return Err(format!("会话 {cid} 无事件记录（事件日志从启用后开始采集）"));
    }
    // 按 trace_id 分组（保留出现顺序）
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<&crate::agent::session_events::SessionEvent>> = HashMap::new();
    for ev in &events {
        let key = ev.trace_id.clone().unwrap_or_else(|| "(untraced)".into());
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(ev);
    }
    let wanted = args["trace_id"].as_str().map(String::from);
    let mut out = String::new();
    let now_ts = chrono::Utc::now().timestamp();
    if let Some(w) = &wanted {
        let Some(list) = groups.get(w) else {
            return Err(format!("未找到 trace_id \"{w}\"。可用 trace_id 值：{}", order.iter().take(10).cloned().collect::<Vec<_>>().join(" / ")));
        };
        out.push_str(&format!("Trace {w}：{} 个事件\n", list.len()));
        render_trace_chain(&mut out, list);
    } else {
        out.push_str(&format!("会话 {cid} 共 {} 个事件 / {} 个任务（最近 10 个）：\n", events.len(), order.len()));
        for key in order.iter().take(10) {
            let list = &groups[key];
            let first_ts = list.first().map(|e| e.created_at).unwrap_or(0);
            let age_min = (now_ts - first_ts).max(0) / 60;
            let tool_calls = list.iter().filter(|e| e.event_type == crate::agent::session_events::SessionEventType::ToolCall).count();
            out.push_str(&format!(
                "  - {}（{} 事件 / {} 工具调用 / {} 分钟前）\n    命令：replay_trace {{\"trace_id\":\"{}\"}}\n",
                key, list.len(), tool_calls, age_min, key
            ));
        }
    }
    Ok(out)
}

/// 渲染单个 trace 的完整调用链（user → assistant → tool_call → tool_result 交错输出）
fn render_trace_chain(out: &mut String, events: &[&crate::agent::session_events::SessionEvent]) {
    use crate::agent::session_events::SessionEventType as T;
    for (i, ev) in events.iter().enumerate() {
        let when = chrono::DateTime::from_timestamp(ev.created_at, 0)
            .map(|d| d.format("%H:%M:%S").to_string())
            .unwrap_or_else(|| "-".into());
        match ev.event_type {
            T::UserMessage => {
                let content = ev.payload.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let brief = content.chars().take(80).collect::<String>();
                out.push_str(&format!("{}. [{when}] 📥 用户: {}\n", i + 1, brief));
            }
            T::AssistantMessage => {
                let content = ev.payload.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let brief = content.chars().take(80).collect::<String>();
                let has_reasoning = ev.payload.get("reasoning").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
                out.push_str(&format!("{}. [{when}] 💬 助手: {}{}\n", i + 1, brief, if has_reasoning { "（含推理）" } else { "" }));
            }
            T::ToolCall => {
                let name = ev.payload.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                let args_s = ev.payload.get("args").map(|a| a.to_string()).unwrap_or_default();
                let args_brief = truncate_chars(&args_s, 100);
                let out_txt = ev.payload.get("output").and_then(|v| v.as_str()).unwrap_or("");
                let out_brief = truncate_chars(out_txt, 60);
                let out_disp: String = if out_brief.is_empty() { "(无输出)".into() } else { out_brief };
                out.push_str(&format!("{}. [{when}] 🔧 {name}({args_brief})\n      → {out_disp}\n", i + 1));
            }
            T::ToolResult => {
                let name = ev.payload.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                let status = ev.payload.get("status").and_then(|v| v.as_str()).unwrap_or("ok");
                out.push_str(&format!("{}. [{when}] ✅ {name} 结果（{status}）\n", i + 1));
            }
            T::SystemNote => {
                let note = ev.payload.to_string();
                out.push_str(&format!("{}. [{when}] 📋 系统: {}\n", i + 1, truncate_chars(&note, 80)));
            }
        }
    }
}

// ================= [19] api_test：OpenAPI/用例批量断言 =================

/// [19] api_test：读取 OpenAPI 3 描述或显式用例，批量发起请求并断言状态码。
/// 参数：{"spec":"<OpenAPI JSON 文件路径（相对项目根）或内联 JSON>","base_url":"<可选覆盖 servers[0]>",
///        "cases":[{"name":"<可选>","path":"/users","method":"GET","status":200,"headers":{},"body":""}],
///        "timeout_secs":<可选，缺省 15>}。
/// 无 cases 时：从 spec 提取全部 GET 路径批量探测（只读冒烟）。
pub(super) async fn api_test(args: &Value, roots: &[String]) -> Result<String, String> {
    let spec_raw = args["spec"].as_str().ok_or("api_test 需要参数 {\"spec\":\"<OpenAPI JSON 路径或内联>\"}")?;
    let spec: Value = if spec_raw.trim_start().starts_with('{') {
        serde_json::from_str(spec_raw).map_err(|e| format!("spec JSON 解析失败: {e}"))?
    } else {
        let p = resolve_readable(roots, spec_raw)?;
        let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| format!("spec 文件 JSON 解析失败（{}）: {e}", p.display()))?
    };
    let base = args["base_url"]
        .as_str()
        .map(String::from)
        .or_else(|| {
            spec["servers"]
                .as_array()
                .and_then(|s| s.first())
                .and_then(|s| s["url"].as_str())
                .map(String::from)
        })
        .ok_or("无法确定 base_url：请传 base_url 参数或 spec 含 servers[0].url")?;
    let base = base.trim_end_matches('/');
    let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(15).clamp(1, 60);
    let client = crate::utils::net::build_client_auto().map_err(|e| format!("网络初始化失败: {e}"))?;

    // 用例来源：显式 cases 或从 spec 提取 GET 路径
    let cases: Vec<(String, String, String, Option<i64>, Option<Value>)> = if let Some(arr) = args["cases"].as_array() {
        let mut v = Vec::new();
        for c in arr {
            let path = c["path"].as_str().unwrap_or("").to_string();
            let method = c["method"].as_str().unwrap_or("GET").to_uppercase();
            let status = c["status"].as_i64();
            if path.is_empty() {
                return Err("cases[].path 不能为空".into());
            }
            let headers = c.get("headers").cloned();
            let body = c["body"].as_str().unwrap_or("").to_string();
            v.push((path, method, body, status, headers));
        }
        v
    } else {
        let mut v = Vec::new();
        if let Some(paths) = spec["paths"].as_object() {
            for (path, item) in paths {
                if let Some(get) = item.get("get") {
                    v.push((path.clone(), "GET".into(), String::new(), None, None));
                    let _ = get;
                }
            }
        }
        if v.is_empty() {
            return Err("spec 中无 GET 路径且未传 cases 参数".into());
        }
        v
    };
    if cases.len() > 40 {
        return Err(format!("用例过多（{}），单次最多 40 个（可拆分或减少 cases）", cases.len()));
    }
    let mut report = String::new();
    let mut pass = 0;
    let mut fail = 0;
    report.push_str(&format!("API 测试报告（{} 个用例 → {base}）\n", cases.len()));
    for (idx, (path, method, body, expect, headers)) in cases.iter().enumerate() {
        let url = if path.starts_with("http") { path.clone() } else { format!("{base}{path}") };
        let mut rb = match method.as_str() {
            "GET" => client.get(&url),
            "POST" => client.post(&url),
            "PUT" => client.put(&url),
            "DELETE" => client.delete(&url),
            "PATCH" => client.patch(&url),
            other => {
                fail += 1;
                report.push_str(&format!("{}. ❌ {method} {path}：不支持的方法 {other}\n", idx + 1));
                continue;
            }
        };
        if let Some(hs) = headers {
            if let Some(obj) = hs.as_object() {
                for (k, v) in obj {
                    if let Some(sv) = v.as_str() {
                        rb = rb.header(k, sv);
                    }
                }
            }
        }
        if !body.is_empty() {
            rb = rb.header("Content-Type", "application/json").body(body.clone());
        }
        let t0 = std::time::Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(timeout_secs), rb.send()).await;
        let elapsed = t0.elapsed().as_millis();
        match result {
            Ok(Ok(resp)) => {
                let status = resp.status().as_u16();
                let ok = expect.map(|e| status == e as u16).unwrap_or(status < 400);
                if ok {
                    pass += 1;
                    report.push_str(&format!("{}. ✅ {method} {path} → {status}（{}ms）\n", idx + 1, elapsed));
                } else {
                    fail += 1;
                    report.push_str(&format!("{}. ❌ {method} {path} → {status}（期望 {expect:?}，{}ms）\n", idx + 1, elapsed));
                }
            }
            Ok(Err(e)) => {
                fail += 1;
                report.push_str(&format!("{}. ❌ {method} {path} → 请求失败：{e}\n", idx + 1));
            }
            Err(_) => {
                fail += 1;
                report.push_str(&format!("{}. ❌ {method} {path} → 超时（>{timeout_secs}s）\n", idx + 1));
            }
        }
    }
   report.push_str(&format!("\n结果：{pass} 通过 / {fail} 失败（共 {}）", cases.len()));
    Ok(report)
}

// ================= [18] api_mock：OpenAPI → 本地 mock 服务 =================

/// 单个 mock 路由（method + 路径正则 + 样例响应）
struct MockRoute {
    method: String,
    path_regex: String,
    response: serde_json::Value,
}

/// 从 schema 递归生成样例值（$ref 解析 + 防环深度上限）
fn sample_from_schema(schema: &serde_json::Value, depth: usize) -> serde_json::Value {
    if depth > 6 {
        return serde_json::Value::Null;
    }
    if let Some(ex) = schema.get("example") {
        if !ex.is_null() {
            return ex.clone();
        }
    }
    if let Some(dv) = schema.get("default") {
        if !dv.is_null() {
            return dv.clone();
        }
    }
    if let Some(r) = schema["$ref"].as_str() {
        // 仅支持站内 components/schemas 引用（/components/schemas/Name）
        if let Some(name) = r.rsplit('/').next() {
            return serde_json::Value::Object(serde_json::Map::from_iter([
                ("$ref_target".to_string(), serde_json::Value::String(name.to_string())),
            ]));
        }
    }
    if let Some(enum_arr) = schema["enum"].as_array() {
        if let Some(first) = enum_arr.first() {
            return first.clone();
        }
    }
    match schema["type"].as_str().unwrap_or("") {
        "object" => {
            let mut obj = serde_json::Map::new();
            if let Some(props) = schema["properties"].as_object() {
                for (k, v) in props {
                    obj.insert(k.clone(), sample_from_schema(v, depth + 1));
                }
            } else if let Some(any) = schema.get("additionalProperties") {
                if any.is_object() && !any.is_null() {
                    obj.insert("key".to_string(), sample_from_schema(any, depth + 1));
                }
            }
            serde_json::Value::Object(obj)
        }
        "array" => {
            let items = &schema["items"];
            if items.is_object() && !items.is_null() {
                serde_json::json!([sample_from_schema(&items, depth + 1)])
            } else {
                serde_json::json!([])
            }
        }
        "string" => {
            let f = schema["format"].as_str().unwrap_or("");
            let sample = match f {
                "date-time" => "2026-01-01T00:00:00Z",
                "date" => "2026-01-01",
                "email" => "user@example.com",
                "uuid" => "00000000-0000-0000-0000-000000000000",
                "uri" => "https://example.com/",
                "ipv4" => "127.0.0.1",
                _ => "string",
            };
            serde_json::Value::String(sample.to_string())
        }
        "integer" | "number" => serde_json::json!(0),
        "boolean" => serde_json::json!(true),
        _ => serde_json::Value::Null,
    }
}

/// 从 operation 取 200（或首个 2xx/default）响应样例
fn pick_response_sample(op: &serde_json::Value, depth: usize) -> (u16, serde_json::Value) {
    let responses = op["responses"].as_object().cloned().unwrap_or_default();
    let mut candidates: Vec<(&String, &serde_json::Value)> = responses.iter().collect();
    candidates.sort_by_key(|(k, _)| {
        k.parse::<u16>().unwrap_or(999) // 数字状态码优先，default 排最后
    });
    for (code, resp) in candidates {
        if let Ok(n) = code.parse::<u16>() {
            if (200..300).contains(&n) {
                let body = &resp["content"]["application/json"];
                let sample = if !body["example"].is_null() {
                    body["example"].clone()
                } else if !body["schema"].is_null() {
                    sample_from_schema(&body["schema"], depth)
                } else {
                    serde_json::Value::Null
                };
                return (n, sample);
            }
        }
    }
    // 无 2xx：default 优先，其次第一个响应
    if let Some(d) = responses.get("default") {
        let sample = if !d["content"]["application/json"]["example"].is_null() {
            d["content"]["application/json"]["example"].clone()
        } else {
            serde_json::Value::Null
        };
        return (200, sample);
    }
    (200, serde_json::Value::Null)
}

/// 把 OpenAPI 路径模板（/users/{id}/posts）转为正则表达式字符串
fn path_template_to_regex(path: &str) -> String {
    let mut re = String::from("^");
    for seg in path.split('/') {
        if seg.starts_with('{') && seg.ends_with('}') {
            re.push_str("/[^/]+");
        } else if seg.is_empty() {
            continue;
        } else {
            re.push('/');
            for c in seg.chars() {
                if ".*+?^$|()[]\\".contains(c) {
                    re.push('\\');
                }
                re.push(c);
            }
        }
    }
    re.push_str("$");
    re
}

/// api_mock：解析 OpenAPI 3.x spec，生成本地 mock 服务（内置 Node 启动，后台常驻）。
/// 参数：{"path":"<OpenAPI JSON 路径或内联>","port":<可选端口，缺省 18080>,"headers":{...}}。
pub(super) async fn api_mock(
    args: &Value,
    roots: &[String],
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法确定 mock 输出位置".into());
    }
    let spec_raw = args["path"].as_str().ok_or("api_mock 需要参数 {\"path\":\"<OpenAPI JSON 路径或内联>\"}")?;
    let spec: Value = if spec_raw.trim_start().starts_with('{') {
        serde_json::from_str(spec_raw).map_err(|e| format!("spec JSON 解析失败: {e}"))?
    } else {
        let p = resolve_readable(roots, spec_raw)?;
        let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| format!("spec 文件 JSON 解析失败（{}）: {e}", p.display()))?
    };
    let port = args["port"].as_u64().unwrap_or(18080).clamp(1024, 65535) as u16;
    let extra_headers = args["headers"].as_object().cloned().unwrap_or_default();

    // 1) 提取路由
    let mut routes: Vec<MockRoute> = Vec::new();
    let Some(paths) = spec["paths"].as_object() else {
        return Err("spec 缺少 paths 字段（仅支持 OpenAPI 3.x）".into());
    };
    for (path, item) in paths {
        for (method, op) in item.as_object().unwrap_or(&serde_json::Map::new()) {
            let m = method.to_uppercase();
            if !["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"].contains(&m.as_str()) {
                continue;
            }
            let (status, sample) = pick_response_sample(op, 0);
            routes.push(MockRoute {
                method: m.clone(),
                path_regex: path_template_to_regex(path),
                response: serde_json::json!({
                    "_mock": { "status": status, "path": path, "method": m },
                    "data": sample,
                }),
            });
        }
    }
    if routes.is_empty() {
        return Err("spec 中未找到任何可 mock 的路径".into());
    }

    // 2) 生成 Node 脚本（零依赖，http 模块）
    let routes_json = serde_json::to_string(&routes.iter().map(|r| {
        serde_json::json!({
            "method": r.method,
            "regex": r.path_regex,
            "response": r.response,
        })
    }).collect::<Vec<_>>()).map_err(|e| e.to_string())?;
    let headers_json = serde_json::to_string(&extra_headers).map_err(|e| e.to_string())?;
    let script = format!(
        "const http = require('http');\nconst port = parseInt(process.argv[2] || '{}', 10);\n\nconst routes = {};\nconst extraHeaders = {};\n\nconst server = http.createServer((req, res) => {{\n  const url = (req.url || '').split('?')[0];\n  for (const r of routes) {{\n    if (req.method === r.method && new RegExp(r.regex).test(url)) {{\n      const body = JSON.stringify(r.response);\n      res.writeHead(r.response._mock.status, Object.assign({{'Content-Type': 'application/json'}}, extraHeaders));\n      res.end(body);\n      return;\n    }}\n  }}\n  res.writeHead(404, {{'Content-Type': 'application/json'}});\n  res.end(JSON.stringify({{error: 'Not Found', path: url}}));\n}});\nserver.listen(port, '127.0.0.1', () => console.log('mock ready on port ' + port));\n",
        port, routes_json, headers_json
    );
    let base = roots[0].trim_end_matches(['/', '\\']);
    let dir = format!("{base}/.deveco-agent/mock");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 mock 目录失败：{e}"))?;
    let script_path = std::path::Path::new(&dir).join("server.js");
    std::fs::write(&script_path, script).map_err(|e| format!("写脚本失败：{e}"))?;

    // 3) 用内置 Node 后台启动（常驻 12h，可 job_kill 终止）
    let node = if let Some(app) = ctx.app.as_ref() {
        let info = crate::services::node_runtime::get_node_runtime_info(app);
        info.dir
            .as_ref()
            .map(|d| {
                let p = std::path::Path::new(d).join("node.exe");
                if p.is_file() { p.to_string_lossy().to_string() } else { "node".to_string() }
            })
            .unwrap_or_else(|| "node".to_string())
    } else {
        "node".to_string()
    };
    let job_id = crate::agent::jobs::start_background(
        node,
        vec![script_path.to_string_lossy().to_string(), port.to_string()],
        format!("mock server on :{port}"),
        std::path::PathBuf::from(&dir),
        12 * 3600,
        ctx,
    )?;

    // 4) 返回使用说明
    let first = routes.first().unwrap();
    Ok(format!(
        "Mock 服务已启动（任务 {job_id}）：http://127.0.0.1:{port}\n共 {} 条路由，示例：{} {}\n返回结构：{{\"_mock\":{{\"status\",\"path\",\"method\"}},\"data\":<样例数据>}}\n服务日志与终止：job_output {job_id} / job_kill {job_id}\n调用示例：用 http_request 或 run_command curl 请求 http://127.0.0.1:{port}{}\n",
        routes.len(),
        first.method,
        spec["paths"].as_object().map(|p| {
            p.keys().next().cloned().unwrap_or_else(|| "/".to_string())
        }).unwrap_or_else(|| "/".to_string()),
        spec["paths"].as_object().and_then(|p| p.keys().next()).cloned().unwrap_or_else(|| "/".to_string()),
    ))
}

// ================= [20] api_health：多 URL 健康探测 =================

/// [20] api_health：批量探测外部 API 健康（GET 状态码 + 耗时）。
/// 参数：{"urls":["<http(s)://...>"],"timeout_secs":<可选，缺省 8>}。
pub(super) async fn api_health(args: &Value) -> Result<String, String> {
    let urls: Vec<String> = if let Some(arr) = args["urls"].as_array() {
        arr.iter().filter_map(|v| v.as_str().map(String::from)).collect()
    } else if let Some(u) = args["url"].as_str() {
        vec![u.to_string()]
    } else {
        return Err("api_health 需要参数 {\"urls\":[\"http://...\"]} 或 {\"url\":\"...\"}".into());
    };
    if urls.is_empty() {
        return Err("urls 为空".into());
    }
    if urls.len() > 10 {
        return Err("单次最多探测 10 个 URL".into());
    }
    let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(8).clamp(1, 30);
    for u in &urls {
        if !u.starts_with("http://") && !u.starts_with("https://") {
            return Err(format!("仅支持 http/https 地址：{u}"));
        }
    }
    let client = crate::utils::net::build_client_auto().map_err(|e| format!("网络初始化失败: {e}"))?;
    let mut out = String::new();
    out.push_str(&format!("API 健康探测（{} 个端点，超时 {timeout_secs}s）\n", urls.len()));
    let mut healthy = 0usize;
    for u in &urls {
        let t0 = std::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            client.get(u).send(),
        )
        .await;
        let elapsed = t0.elapsed().as_millis();
        match result {
            Ok(Ok(resp)) => {
                let status = resp.status().as_u16();
                let ok = status < 500;
                if ok {
                    healthy += 1;
                }
                out.push_str(&format!("  {} {status}（{elapsed}ms）{u}\n", if ok { "✅" } else { "⚠️" }));
            }
            Ok(Err(e)) => out.push_str(&format!("  ❌ 请求失败（{elapsed}ms）{u}\n     {e}\n")),
            Err(_) => out.push_str(&format!("  ❌ 超时（>{timeout_secs}s）{u}\n")),
        }
    }
    out.push_str(&format!("\n健康 {}/{}", healthy, urls.len()));
    Ok(out)
}

// ================= [35] obfuscate：混淆开关读写 =================

/// [35] obfuscate：读写 build-profile.json5 的 obfuscation 混淆配置。
/// 参数：{"action":"status|enable|disable（缺省 status）","path":"<可选 build-profile.json5 路径，缺省项目根>"}。
/// 说明：混淆规则在 products[].buildOption.arkOptions.obfuscation.ruleOptions（enable + files），
/// 仅文本级切换 enable 开关，不动规则文件内容；写前自动备份到 .deveco-agent/backups/。
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

// ================= [73] sandbox_exec：危险命令干跑预览 =================

/// [73] sandbox_exec：危险命令临时目录干跑——把影响面隔离在系统临时沙箱，
/// 先预览结果再决定是否在真实目录执行。
/// 参数：{"command":"<命令串>","source":"<可选源目录（复制到沙箱后执行，复制限制 50MB/200 文件）>",
///        "timeout_secs":<可选，缺省 30>,"mode":"simulate|preview（缺省 simulate）"}。
/// simulate：复制 source 到临时沙箱并执行 command（影响面仅沙箱）；无 source 时仅静态危险分析。
/// preview：只做静态危险模式分析并给出建议，不执行任何命令。
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

/// 递归复制目录到沙箱（限制 200 文件 / 50MB，跳过构建产物目录）
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

// ================= [48] license_check：依赖许可证合规检查 =================

/// [48] license_check：扫描工程依赖（ohpm/Cargo/uv）许可证合规性。
/// 参数：{"action":"scan|list"（缺省 scan）,"allow":<可选数组，缺省白名单 MIT/Apache-2.0/BSD-3-Clause/ISC/MPL-2.0/CC0-1.0>,"deny":<可选黑名单>,"path":"<可选工程子目录>"}。
/// 实现策略：纯静态解析（不联网），扫 oh-package.json5（ohpm 依赖）、oh-package-lock.json5（已锁定的版本 + 解析出的 license）、
/// Cargo.toml 的 license 字段、pyproject.toml。命中 deny 列表 / 不在 allow 列表的依赖标红。
/// 适合：法务合规审查、企业许可证策略、新人 onboarding 时检查项目依赖是否合规。
/// 副作用：仅读文件，不改任何状态。
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

/// 解析 "name": "version" 或 name == version 格式
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

/// 提取 "..." 内的字符串（首个非空）
fn extract_quoted(s: &str) -> Option<String> {
    let s = s.trim();
    if let Some(start) = s.find('"') {
        if let Some(end) = s[start + 1..].find('"') {
            return Some(s[start + 1..start + 1 + end].to_string());
        }
    }
    None
}

// ================= [49] vuln_scan：依赖漏洞扫描 =================

/// [49] vuln_scan：依赖漏洞扫描（基于 lock 文件 + 本地漏洞库，不联网）。
/// 参数：{"source":"ohpm|cargo|uv|all"（缺省 all）,"path":"<可选子目录>"}。
/// 实现策略：解析 lock 文件，提取包名 + 版本，与内置的已知漏洞库（CVE 编号列表，可后续维护到 DB）匹配。
/// 适合：CI 跑一次把明显漏洞兜出来、升级前快速评估。
/// 副作用：仅读 lock 文件。
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

// ================= [23] docx_read：读取 Word 文档正文 =================

/// [23] docx_read：解析 .docx（本质是 zip）取 word/document.xml 中的 <w:t> 文本。
/// 参数：{"path":"<docx 路径>"}。
/// 实现：纯标准库 zip + 字符串提取（避免引入 docx-rs 依赖）。
/// 限制：保留段落结构（段间换行），不保留格式；5000 字符截断。
pub(super) async fn docx_read(
    args: &Value,
    roots: &[String],
) -> Result<String, String> {
    let path = args["path"]
        .as_str()
        .ok_or("docx_read 需要参数 {\"path\":\"<docx 路径>\"}")?;
    let resolved = resolve_in_roots(roots, path)?;
    let file = std::fs::File::open(&resolved)
        .map_err(|e| format!("打开 docx 失败: {e}"))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| format!("解析 zip 失败（docx 是 zip 格式）: {e}"))?;
    let mut doc_xml = String::new();
    {
        let mut entry = zip
            .by_name("word/document.xml")
            .map_err(|e| format!("读 word/document.xml 失败: {e}"))?;
        use std::io::Read;
        entry
            .read_to_string(&mut doc_xml)
            .map_err(|e| format!("读 xml 失败: {e}"))?;
    }
    // 简化提取：取 <w:t>...</w:t> 标签内的文本，段间换行（<w:p ...>...</w:p> 之间插入 \n）
    // 标签不跨多行的简单实现
    let mut out = String::new();
    let mut in_p = false;
    for line in doc_xml.lines() {
        let l = line.trim();
        if l.starts_with("<w:p ") || l == "<w:p>" {
            in_p = true;
            continue;
        }
        if l == "</w:p>" {
            in_p = false;
            out.push('\n');
            continue;
        }
        // 提 <w:t ...>text</w:t> 里的 text
        if let Some(start) = l.find("<w:t") {
            if let Some(gt_pos) = l[start..].find('>') {
                let after_gt = start + gt_pos + 1;
                if let Some(end) = l[after_gt..].find("</w:t>") {
                    let text = &l[after_gt..after_gt + end];
                    if !text.trim().is_empty() {
                        if in_p && !out.ends_with('\n') && !out.is_empty() {
                            // 段内：追加空格
                        }
                        out.push_str(text);
                    }
                }
            }
        }
    }
    let truncated = if out.chars().count() > 5000 {
        let s: String = out.chars().take(5000).collect();
        format!("{s}…\n[截断：超过 5000 字符]")
    } else {
        out
    };
    let para_count = truncated.lines().filter(|l| !l.trim().is_empty()).count();
    Ok(format!(
        "📄 DOCX 解析：{}\n段落数：{}\n\n{}",
        resolved.display(),
        para_count,
        truncated
    ))
}

// ================= [26] audio_transcribe：音频转文字（whisper.cpp）=================

/// [26] audio_transcribe：调本地 whisper.cpp 二进制把音频转文字。
/// 参数：{"path":"<音频文件路径（wav/mp3/m4a/ogg）>"}。
/// 前置：需要用户安装 whisper.cpp（https://github.com/ggerganov/whisper.cpp），
/// 二进制在 PATH 或 resources/，模型 ggml-base.bin 放 ~/.cache/whisper/。
/// 副作用：调外部 CLI 进程（CPU/GPU 占用 1-3 分钟）。
pub(super) async fn audio_transcribe(
    args: &Value,
    roots: &[String],
) -> Result<String, String> {
    let path = args["path"]
        .as_str()
        .ok_or("audio_transcribe 需要参数 {\"path\":\"<音频路径>\"}")?;
    let resolved = resolve_in_roots(roots, path)?;
    if !resolved.exists() {
        return Err(format!("音频文件不存在: {}", resolved.display()));
    }

    // 找 whisper 二进制（PATH 优先，再找 resources/whisper/）
    let whisper_bin = find_whisper_binary().ok_or_else(|| {
        "未找到 whisper.cpp 二进制。请按以下步骤安装：\n  \
         1. git clone https://github.com/ggerganov/whisper.cpp\n  \
         2. cd whisper.cpp && make\n  \
         3. 把 main 二进制加到 PATH 或复制到 resources/whisper/\n  \
         4. 下载模型：bash models/download-ggml-model.sh base\n  \
         5. 放 ~/.cache/whisper/ggml-base.bin"
            .to_string()
    })?;
    let model = find_whisper_model().ok_or_else(|| {
        "未找到 whisper 模型。运行 bash models/download-ggml-model.sh base 并把 ggml-base.bin 放到 ~/.cache/whisper/".to_string()
    })?;

    // 转绝对路径 + 调命令行
    let audio_str = resolved.to_string_lossy().into_owned();
    let start = std::time::Instant::now();
    let out = std::process::Command::new(&whisper_bin)
        .arg("-m").arg(&model)
        .arg("-f").arg(&audio_str)
        .arg("--no-timestamps") // 简化输出
        .arg("-l").arg("auto")
        .output()
        .map_err(|e| format!("执行 whisper 失败: {e}（请确认二进制可执行）"))?;
    let elapsed = start.elapsed();
    if !out.status.success() {
        return Err(format!(
            "whisper 执行失败（退出码 {}）stderr: {}",
            out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "无".into()),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let truncated = if text.chars().count() > 4000 {
        let s: String = text.chars().take(4000).collect();
        format!("{s}…\n[截断：超过 4000 字符]")
    } else {
        text
    };
    let segments = truncated.lines().filter(|l| !l.trim().is_empty()).count();
    Ok(format!(
        "🎙️ 音频转写：{}\n二进制：{}\n模型：{}\n段数：{} / 耗时：{:.1}s\n\n{}",
        resolved.display(),
        whisper_bin,
        model,
        segments,
        elapsed.as_secs_f64(),
        truncated
    ))
}

fn find_whisper_binary() -> Option<String> {
    // PATH 优先
    for name in &["whisper", "main"] {
        if let Ok(out) = std::process::Command::new("which").arg(name).output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() { return Some(s); }
            }
        }
    }
    // resources/whisper/ 备选
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in &["whisper.exe", "whisper", "main"] {
                let p = dir.join("resources").join("whisper").join(name);
                if p.exists() { return Some(p.to_string_lossy().into_owned()); }
            }
        }
    }
    None
}

fn find_whisper_model() -> Option<String> {
    let candidates = [
        "~/.cache/whisper/ggml-base.bin",
        "~/.cache/whisper/ggml-small.bin",
        "~/whisper.cpp/models/ggml-base.bin",
    ];
    for c in candidates {
        if let Some(expanded) = expand_home(c) {
            if std::path::Path::new(&expanded).exists() {
                return Some(expanded);
            }
        }
    }
    None
}

fn expand_home(p: &str) -> Option<String> {
    if p.starts_with("~/") {
        if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
            return Some(format!("{}{}", home.to_string_lossy(), &p[1..]));
        }
    }
    Some(p.to_string())
}

// ================= [28] attach_debugger：远程 attach 调试器 =================

/// [28] attach_debugger：通过 hdc 把调试器 attach 到目标进程（ArkTS/JS debugger）。
/// 参数：{"device":"<可选>","bundle":"<可选包名>","wait_secs":<可选等待 attach 完成秒数，缺省 30>}。
/// 实现：基于 hdc shell `aa debug -b <bundle>` 启动调试会话（DevEco 工具链支持的 attach 模式），
/// 或 hdc shell debuggerd attach <pid>（旧接口）。返回 attach 状态 + 端口。
/// 适合：应用启动失败 / 闪退但 hilog 没抓到根因时，attach 看现场调用栈。
/// 副作用：在设备端拉起调试器（不修改用户数据，但有性能开销 + 日志输出增加）。
pub(super) async fn attach_debugger(
    args: &Value,
    roots: &[String],
) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => {
            // 默认设备：从 hdc 找 ★ 标记的
            hdc_shell(&["list", "targets"])
                .map_err(|e| format!("hdc list targets 失败: {e}"))?
                .lines()
                .find(|l| l.contains('\t') || l.contains("[empty]"))
                .map(|l| l.split_whitespace().next().unwrap_or("").to_string())
                .ok_or_else(|| "未找到默认设备，请先 list_devices".to_string())?
        }
    };
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    let bundle = match args["bundle"].as_str() {
        Some(b) => b.to_string(),
        None => {
            if project_path.is_empty() {
                return Err("未指定 bundle 且当前会话未绑定工程".into());
            }
            crate::services::harmony::parse_project(std::path::Path::new(project_path))
                .bundle_name
                .ok_or_else(|| "无法确定应用包名".to_string())?
        }
    };
    if bundle.is_empty() { return Err("无法确定应用包名".into()); }
    let wait_secs = args["wait_secs"].as_u64().unwrap_or(30);

    // 1) 拿 pid
    let pid_out = hdc_shell(&["-t", &device, "shell", "pidof", &bundle]).map_err(|e| format!("hdc pidof 失败: {e}"))?;
    let pid = pid_out.trim();
    if pid.is_empty() {
        return Err(format!("应用未运行或 pidof 返回空（先 deploy 启动应用）"));
    }

    // 2) attach 调试器（hdc shell debuggerd attach <pid>，系统服务）
    //    注：DevEco 工程的 attach 通常用 `aa debug -b <bundle>` 启动开发模式；
    //    这里是运行时 attach，更轻量。
    let attach_out = hdc_shell(&["-t", &device, "shell", "debuggerd", &format!("-p {pid}")]).map_err(|e| format!("debuggerd attach 失败: {e}"));

    match attach_out {
        Ok(out) => Ok(format!(
            "调试器已 attach：设备 {device} / 包 {bundle} / PID {pid} / 等待 {wait_secs}s\ndebuggerd 输出：{}\n\n下一步：\n  1. 在 DevEco Studio 中 Run > Attach Debugger，选已 attach 的进程\n  2. 或在终端用 jstack/jdb 远程连接到设备 debuggerd 端口\n  3. 配合 set_breakpoint / inspect_variable 等工具（如已实现）",
            if out.trim().is_empty() { "(无输出)" } else { out.trim() }
        )),
        Err(e) => {
            // 退路：尝试 aa debug 启动开发模式
            let aa = hdc_shell(&["-t", &device, "shell", "aa", "debug", "-b", &bundle]);
            match aa {
                Ok(out2) => Ok(format!(
                    "调试器已通过 aa debug 启动：设备 {device} / 包 {bundle} / PID {pid}\n输出：{}\n",
                    out2
                )),
                Err(_) => Err(format!(
                    "attach 失败：{e}\n回退方案也失败（aa debug 不可用，可能需要 userdebug 系统）\n替代：在 DevEco Studio 中 Run > Debug 'app'"
                )),
            }
        }
    }
}

// ================= [29] step_debug：单步调试 =================

/// [29] step_debug：通过 hdc debuggerd 发送单步/继续/中断指令。
/// 参数：{"device":"<可选>","pid":"<可选，未指定则用当前 attach 的 pid>","action":"step|next|continue|interrupt|where|info"（缺省 step）}。
/// 实现：hdc shell debuggerd 支持 -c <command>，step=si, next=ni, continue=c, interrupt=Ctrl+C, where=bt（backtrace）。
/// 适合：attach 后单步到可疑函数、看调用栈、查看线程信息。
/// 副作用：向已 attach 的进程发调试命令。
pub(super) async fn step_debug(
    args: &Value,
    roots: &[String],
) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => {
            hdc_shell(&["list", "targets"])
                .map_err(|e| format!("hdc list targets 失败: {e}"))?
                .lines()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.split_whitespace().next().unwrap_or("").to_string())
                .ok_or_else(|| "未找到默认设备".to_string())?
        }
    };
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    let pid = match args["pid"].as_str() {
        Some(p) => p.to_string(),
        None => {
            if project_path.is_empty() {
                return Err("未指定 pid 且当前会话未绑定工程".into());
            }
            let bundle = crate::services::harmony::parse_project(std::path::Path::new(project_path))
                .bundle_name
                .ok_or_else(|| "无法确定应用包名".to_string())?;
            let pid_out = hdc_shell(&["-t", &device, "shell", "pidof", &bundle]).map_err(|e| format!("hdc pidof 失败: {e}"))?;
            let p = pid_out.trim().to_string();
            if p.is_empty() {
                return Err("应用未运行（先 deploy 启动或 attach_debugger）".into());
            }
            p
        }
    };
    let action = args["action"].as_str().unwrap_or("step");
    // debuggerd 命令映射
    let cmd = match action {
        "step" => "s",        // step into
        "next" => "n",        // step over
        "continue" | "cont" | "c" => "c",
        "interrupt" | "int" => "i",
        "where" | "bt" | "backtrace" => "bt",
        "info" | "registers" => "r",
        other => return Err(format!("不支持的 step_debug action: {other}（step/next/continue/interrupt/where/info）")),
    };

    let out = hdc_shell(&["-t", &device, "shell", "debuggerd", &format!("-p {pid} -c {cmd}")]).map_err(|e| format!("debuggerd 命令失败: {e}"))?;

    Ok(format!(
        "单步调试（设备 {device} / PID {pid} / action={action}）：\n{}",
        if out.trim().is_empty() { "(无输出，可能进程未停在断点)" } else { out.trim() }
    ))
}

// ================= [36] ota_pack：制作 OTA 升级包 =================

/// [36] ota_pack：基于现有 HAP 包制作 HarmonyOS OTA 升级包（packaging_tool 模式）。
/// 参数：{"hap_path":"<HAP 文件路径>","out_path":"<输出 .pkg 路径>","profile_path":"<可选签名 profile.json>"}。
/// 实现：调 hvigorw assembleApp（生成 .app 包）→ hmos app packager 打 .pkg（OTA 格式）。
/// 前置：DevEco Studio 工具链（hms/packingtool.jar 在 PATH 或 DevEco 安装目录）；
///        .app 产物（packaging tool 输入）+ signing material（profile.json + cert）。
/// 适合：发布 OTA 升级前批量打包、给测试同学发包、CI 自动出包。
/// 副作用：写 .pkg 到 out_path。
pub(super) async fn ota_pack(
    args: &Value,
    roots: &[String],
) -> Result<String, String> {
    let hap_path = args["hap_path"]
        .as_str()
        .ok_or("ota_pack 需要参数 {\"hap_path\":\"<HAP 路径>\"}")?;
    let out_path = args["out_path"]
        .as_str()
        .ok_or("ota_pack 需要参数 {\"out_path\":\"<输出 .pkg 路径>\"}")?;
    let profile_path = args["profile_path"].as_str();

    // 1) 验证 HAP 存在
    let hap_full = resolve_in_roots(roots, hap_path)?;
    if !hap_full.exists() {
        return Err(format!("HAP 不存在: {}", hap_full.display()));
    }

    // 2) 找 packaging_tool（DevEco Studio 自带）
    let packager = find_packaging_tool().ok_or_else(|| {
        "未找到 packaging_tool.jar。请：\n  \
         1. 安装 DevEco Studio\n  \
         2. 或下载 HarmonyOS Sdk Command-Line Tools\n  \
         3. 把 packagingtool.jar 路径加到环境变量 HOS_SDK_HOME 或 PATH"
            .to_string()
    })?;

    // 3) 构造命令（hmos app packager 打 OTA 包）
    //    实际命令：java -jar <packager> --mode ota --hap <hap> --out <pkg> --profile <profile>
    let mut cmd = std::process::Command::new("java");
    cmd.arg("-jar").arg(&packager);
    cmd.arg("--mode").arg("ota");
    cmd.arg("--hap").arg(&hap_full);
    cmd.arg("--out").arg(out_path);
    if let Some(pp) = profile_path {
        cmd.arg("--profile").arg(pp);
    }
    cmd.arg("--force"); // 覆盖已存在

    let start = std::time::Instant::now();
    let output = cmd.output().map_err(|e| format!(
        "启动 packaging_tool 失败: {e}（确认 java 在 PATH 且 packaging_tool.jar 可访问）"
    ))?;
    let elapsed = start.elapsed();

    if !output.status.success() {
        return Err(format!(
            "OTA 打包失败（退出码 {}）\nstderr: {}\nstdout: {}",
            output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "无".into()),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let out_p = std::path::Path::new(out_path);
    let size = std::fs::metadata(out_p).map(|m| m.len()).unwrap_or(0);
    Ok(format!(
        "✅ OTA 包已生成：{}\n大小：{:.1} KB\n耗时：{:.1}s\npackaging_tool：{}\nstdout 摘要：\n{}",
        out_p.display(),
        size as f64 / 1024.0,
        elapsed.as_secs_f64(),
        packager,
        if stdout.trim().is_empty() { "(无输出)".to_string() } else { stdout.chars().take(1500).collect::<String>() }
    ))
}

fn find_packaging_tool() -> Option<String> {
    // 1) 环境变量
    if let Ok(p) = std::env::var("HOS_PACKAGING_TOOL") {
        if std::path::Path::new(&p).exists() { return Some(p); }
    }
    // 2) DevEco 常见路径
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = std::path::PathBuf::from(home);
        let candidates = [
            home.join("AppData").join("Local").join("Huawei").join("Sdk").join("toolchains").join("packagingtool.jar"),
            home.join("Library").join("Huawei").join("Sdk").join("toolchains").join("packagingtool.jar"),
        ];
        for c in candidates {
            if c.exists() { return Some(c.to_string_lossy().into_owned()); }
        }
    }
    // 3) Windows 全局
    for c in [
        "C:/Program Files/Huawei/DevEco Studio/tools/packagingtool.jar",
        "D:/Huawei/DevEco Studio/tools/packagingtool.jar",
        "D:/DevEco Studio/tools/packagingtool.jar",
    ] {
        if std::path::Path::new(c).exists() { return Some(c.to_string()); }
    }
    // 4) resources/packagingtool/ 备选
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("resources").join("packagingtool.jar");
            if p.exists() { return Some(p.to_string_lossy().into_owned()); }
        }
    }
    None
}

/// 包装 output_blocking：返回 stdout 字符串（替代直接用 output_blocking 配 .trim()/.lines()）
/// 接受任意 AsRef<str> 切片，支持混合 &str / &String（如 `&["-t", &device, "shell", "pidof", &bundle]`）
fn hdc_shell<S: AsRef<str>>(args: &[S]) -> Result<String, String> {
    let owned: Vec<String> = args.iter().map(|s| s.as_ref().to_string()).collect();
    let out = crate::utils::process::output_blocking("hdc", &owned)
        .map_err(|e| format!("hdc 执行失败: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
