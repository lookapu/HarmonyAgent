//! metrics 子模块 — 按职责拆分（详见 quality_tools.rs facade）。
//!
//! 调用方式不变：quality_tools::xxx(...)，通过 pub use re-export 暴露。

use crate::agent::tools::{resolve_readable, truncate_chars};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SOURCE_EXTS: &[&str] = &[
    "ets", "ts", "tsx", "js", "jsx", "kt", "java", "swift", "c", "h",
    "cpp", "hpp", "rs", "go", "py", "vue",
];
const SKIP_DIRS: &[&str] = &[
    ".git", ".hvigor", ".ohpm", "oh_modules", "node_modules", "build",
    "dist", ".deveco-agent", ".idea", ".vscode", "target", ".cxx",
];

/// 单文件指标聚合
#[derive(Default, Clone)]
struct FileMetrics {
    total_lines: u32,
    code_lines: u32,
    comment_lines: u32,
    blank_lines: u32,
    functions: u32,
    /// 圈复杂度增量（McCabe），每多一个控制流分支 +1
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

pub async fn code_metrics(args: &Value, roots: &[String]) -> Result<String, String> {
    let raw = args["path"].as_str().unwrap_or(".");
    let p = resolve_readable(roots, raw)?;
    if !p.exists() {
        return Err(format!("路径不存在: {}", p.display()));
    }
    let top_n = args["top"].as_u64().unwrap_or(10).min(50) as usize;
    // 全量收集源码文件 + 逐文件扫描（CPU/IO 密集）在 blocking 线程池执行，避免钉死 tokio worker
    let scan_p = p.clone();
    let (errors, total, per_file) = tokio::task::spawn_blocking(move || {
        let mut files: Vec<PathBuf> = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        if scan_p.is_file() {
            files.push(scan_p.clone());
        } else {
            collect_source_files(&scan_p, &mut files, 0);
        }
        if files.is_empty() {
            return Err(format!("未在 {} 下找到源码文件（扩展名: {}）", scan_p.display(), SOURCE_EXTS.join("/")));
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
        per_file.sort_by_key(|a| std::cmp::Reverse(a.1.cyclomatic_delta));
        Ok((errors, total, per_file))
    })
    .await
    .map_err(|e| e.to_string())??;
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

pub async fn metric_export(
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

pub async fn log_aggregate(
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
                match crate::agent::tools::debug_tools::search_hilog(&sub, roots).await {
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
                match crate::agent::tools::test_tools::read_runtime_logs(&sub, roots, ctx).await {
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

pub async fn log_query(
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
                match crate::agent::tools::debug_tools::search_hilog(&sub, roots).await {
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
                match crate::agent::tools::test_tools::read_runtime_logs(&sub, roots, ctx).await {
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

pub async fn memory_snapshot(
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
            let raw = crate::agent::tools::ui_tools::dump_memory(&pass, roots).await?;
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

pub async fn snippet_insert(
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

pub async fn replay_trace(
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


fn snippet_count(conn: &rusqlite::Connection) -> Result<i64, String> {
    conn.query_row("SELECT COUNT(*) FROM snippets", [], |r| r.get(0)).map_err(|e| e.to_string())
}


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
            T::ContextCompress => {
                let trigger = ev.payload.get("trigger").and_then(|v| v.as_str()).unwrap_or("?");
                out.push_str(&format!("{}. [{when}] 🗜️ 上下文压缩（{}）\n", i + 1, trigger));
            }
        }
    }
}


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

