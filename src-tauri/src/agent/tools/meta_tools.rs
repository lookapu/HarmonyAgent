//! 工具自我管理域：tool_list / tool_help / tool_history / db_query / share_session / import_session / trace_export
//! 以及工具元数据表（timeout_hint / retry_policy / cost_hint）——让模型在选工具前知道时限与成本，
//! system prompt 不必把所有工具的细节都塞进来，按需用 tool_help 拉取。

use super::*;

/// 工具元数据（稀疏标注：仅登记需要提示的工具，其余走默认文案）
pub struct ToolMeta {
    pub name: &'static str,
    pub timeout_hint: &'static str,
    pub retry_policy: &'static str,
    pub cost_hint: &'static str,
}

/// 重点工具的时限/重试/成本提示（模型选工具时的决策输入）
pub const TOOL_META: &[ToolMeta] = &[
    ToolMeta { name: "web_search", timeout_hint: "5~15 秒", retry_policy: "失败可重试 1 次", cost_hint: "约 0.001 元/次" },
    ToolMeta { name: "web_fetch", timeout_hint: "5~20 秒", retry_policy: "失败可重试 1 次", cost_hint: "约 0.001 元/次" },
    ToolMeta { name: "http_request", timeout_hint: "10~60 秒（可传 timeout 参数）", retry_policy: "网络类错误可重试 1 次", cost_hint: "免费（本机直连）" },
    ToolMeta { name: "build_project", timeout_hint: "1~10 分钟", retry_policy: "失败先 get_diagnostics 归因再重试", cost_hint: "免费（本机编译）" },
    ToolMeta { name: "deploy", timeout_hint: "1~5 分钟", retry_policy: "失败先 get_diagnostics 归因再重试", cost_hint: "免费（hdc 安装）" },
    ToolMeta { name: "run_tests", timeout_hint: "1~10 分钟", retry_policy: "失败可重试 1 次", cost_hint: "免费（本机执行）" },
    ToolMeta { name: "run_ui_flow", timeout_hint: "最长 30 分钟", retry_policy: "不可重试（自动遍历类）", cost_hint: "免费（设备端执行）" },
    ToolMeta { name: "run_perf_benchmark", timeout_hint: "1~5 分钟", retry_policy: "不可重试（采样窗口会漂移）", cost_hint: "免费（设备端执行）" },
    ToolMeta { name: "refresh_api_db", timeout_hint: "10~40 分钟（全量抓取）", retry_policy: "中断后可重跑（断点续抓）", cost_hint: "免费（官方文档抓取）" },
    ToolMeta { name: "device_shell", timeout_hint: "默认 30 秒", retry_policy: "失败可重试 1 次", cost_hint: "免费（hdc）" },
    ToolMeta { name: "spawn_agents", timeout_hint: "取决于子任务", retry_policy: "子 Agent 内部自管", cost_hint: "子 Agent 独立计费" },
    ToolMeta { name: "db_query", timeout_hint: "≤ 5 秒", retry_policy: "SQL 错误不重试（先检查语句）", cost_hint: "免费（本地库）" },
    ToolMeta { name: "lsp_definition", timeout_hint: "≤ 20 秒", retry_policy: "失败可重试 1 次", cost_hint: "免费（本地语言服务）" },
];

/// 查某工具元数据（未登记返回 None，调用方用默认文案）
pub fn meta_for(name: &str) -> Option<&'static ToolMeta> {
    TOOL_META.iter().find(|m| m.name == name)
}

/// 渲染单工具元数据行（未登记时返回空串）
pub fn fmt_meta(name: &str) -> String {
    match meta_for(name) {
        Some(m) => format!("（超时 {}/重试：{}/成本：{}）", m.timeout_hint, m.retry_policy, m.cost_hint),
        None => String::new(),
    }
}

// ---------------- tool_list / tool_help ----------------

/// tool_list：动态列出当前可用工具（名称 + 一句话描述 + 时限/成本元数据）。
/// 支持 {"group":"build|fix|explore|deploy|refactor|test|other"} 按任务域过滤（[62]）。
pub(super) async fn tool_list(args: &Value, _roots: &[String]) -> Result<String, String> {
    let group = args["group"].as_str().map(str::trim).filter(|s| !s.is_empty());
    if let Some(g) = group {
        if !super::TASK_GROUPS.contains(&g) {
            return Err(format!(
                "未知分组 \"{g}\"。可用分组：{}（省略 group 参数列出全部）",
                super::TASK_GROUPS.join(" / ")
            ));
        }
        let names: Vec<&str> = super::TOOL_SPECS
            .iter()
            .filter(|t| super::tool_group(t.name) == g)
            .map(|t| t.name)
            .collect();
        let mut s = format!("任务分组 \"{g}\" 共 {} 个工具：\n", names.len());
        for name in names {
            if let Some(t) = super::TOOL_SPECS.iter().find(|t| t.name == name) {
                s.push_str(&format!("- {}：{}{}\n", t.name, t.desc, fmt_meta(t.name)));
            }
        }
        s.push_str("\n想了解某个工具的详细用法，调用 tool_help name=<工具名>。");
        return Ok(s);
    }
    let mut s = format!("当前可用内置工具共 {} 个：\n", super::TOOL_SPECS.len());
    for t in super::TOOL_SPECS {
        s.push_str(&format!(
            "- {}：{}{}\n",
            t.name,
            t.desc,
            fmt_meta(t.name)
        ));
    }
    s.push_str("\n另有 MCP 服务器工具与技能，可用 list_mcp_servers / list_skills 查看。\n");
    s.push_str("想了解某个工具的详细用法，调用 tool_help name=<工具名>。");
    Ok(s)
}

/// tool_help：查某工具的详细说明（描述 + 权限级别 + 时限/重试/成本元数据）。
pub(super) async fn tool_help(args: &Value, _roots: &[String]) -> Result<String, String> {
    let name = args["name"].as_str().ok_or("需要参数 {\"name\":\"<工具名>\"}")?;
    let Some(t) = super::TOOL_SPECS.iter().find(|t| t.name == name) else {
        // MCP 工具也给出提示（mcp__server__tool 形式）
        if name.starts_with("mcp__") {
            return Ok(format!("{name} 是 MCP 服务器工具，可调用 list_mcp_servers 查看服务器清单及其工具。"));
        }
        return Err(format!("未找到工具 \"{name}\"。可用 tool_list 查看全部工具清单。"));
    };
    let level = crate::services::permissions::tool_level(name).as_str();
    let contract = crate::agent::tools::contracts::contract(name);
    let mut out = format!("【{name}】{}\n", t.desc);
    out.push_str(&format!("任务分组：{}\n", super::tool_group(name)));
    out.push_str(&format!("权限级别：{}（{}）\n", level,
        match crate::services::permissions::tool_level(name) {
            crate::services::permissions::Level::L0 => "只读，信任项目免审",
            crate::services::permissions::Level::L1 => "写入/开发动作，信任项目免审",
            crate::services::permissions::Level::L2 => "危险操作，始终需用户确认",
        }));
    let meta = fmt_meta(name);
    if !meta.is_empty() {
        out.push_str(&format!("执行预期：{}\n", meta));
    }
    out.push_str(&format!(
        "执行契约：副作用={:?} / 幂等={:?} / 超时={}ms / 取消={:?} / 重试安全={} / 审批={:?} / 恢复={:?} / 验证器={:?} / 恢复动作={:?}\n",
        contract.effect, contract.idempotency, contract.timeout_ms, contract.cancellation,
        contract.retry_safe, contract.approval, contract.recovery, contract.validator,
        contract.recovery_action,
    ));
    // 参数提示：从 desc 中的 JSON 示例提取（若描述里给了 { ... } 片段）
    if let Some(start) = t.desc.find('{') {
        let mut depth = 0;
        let mut end = t.desc.len();
        for (i, ch) in t.desc[start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        let sample = &t.desc[start..end];
        out.push_str(&format!("参数示例：{}\n", sample));
    }
    out.push_str("\n调用方式：输出一行【TOOL|工具名|JSON参数】标记。");
    Ok(out)
}

// ---------------- tool_history ----------------

/// tool_history：最近工具调用历史（tool_runs 表，默认当前会话，可跨会话/按工具/按状态过滤）。
/// 用于回答"刚才那个工具为什么失败"等复盘问题。
pub(super) async fn tool_history(
    args: &Value,
    _roots: &[String],
    conversation_id: &str,
    db: &crate::db::DbState,
) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let limit = (args["limit"].as_u64().unwrap_or(10) as usize).clamp(1, 100) as i64;
    let tool = args["tool"].as_str().map(str::trim).filter(|s| !s.is_empty());
    let status = args["status"].as_str().map(str::trim).filter(|s| !s.is_empty());
    let all = args["all"].as_bool().unwrap_or(false);

    let mut sql = String::from(
        "SELECT tool_name, status, duration_ms, input_json, result_json, created_at
         FROM tool_runs WHERE ",
    );
    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    if all {
        sql.push_str("1=1");
    } else {
        sql.push_str("conversation_id = ?");
        params.push(conversation_id.to_string().into());
    }
    if let Some(t) = tool {
        sql.push_str(" AND tool_name = ?");
        params.push(t.to_string().into());
    }
    if let Some(s) = status {
        sql.push_str(" AND status = ?");
        params.push(s.to_string().into());
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT ?");
    params.push(limit.into());
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("查询工具历史失败：{e}"))?;
    let q = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), row_map)
        .map_err(|e| format!("查询工具历史失败：{e}"))?;
    let rows = q.collect::<Result<Vec<(String, String, Option<i64>, Option<String>, Option<String>, i64)>, _>>()
        .map_err(|e| format!("读取工具历史失败：{e}"))?;
    if rows.is_empty() {
        return Ok("（当前会话暂无工具调用记录）".into());
    }
    let status_zh = |s: &str| -> String { match s {
        "ok" => "成功".to_string(),
        "error" => "失败".to_string(),
        "running" => "执行中".to_string(),
        "cancelled" => "已取消".to_string(),
        "ask" => "待确认".to_string(),
        _ => s.to_string(),
    }};
    let mut out = format!("工具调用历史（{} 条，新→旧）：\n", rows.len());
    for (name, status, dur, input, result, ts) in rows {
        let t = chrono::DateTime::from_timestamp(ts, 0)
            .map(|d| d.format("%H:%M:%S").to_string())
            .unwrap_or_else(|| "-".into());
        let dur_s = dur.map(|d| format!("{}ms", d)).unwrap_or_else(|| "-".into());
        let brief = input.as_deref().map(|s| {
            let mut v: serde_json::Value =
                serde_json::from_str(s).unwrap_or_else(|_| serde_json::Value::String(s.to_string()));
            redact(&mut v);
            v.to_string().chars().filter(|c| !c.is_whitespace()).take(80).collect::<String>()
        }).unwrap_or_default();
        let err = if status == "error" {
            result.as_deref().map(|s| {
                let mut v: serde_json::Value =
                    serde_json::from_str(s).unwrap_or_else(|_| serde_json::Value::String(s.to_string()));
                redact(&mut v);
                v.to_string().chars().filter(|c| !c.is_whitespace()).take(100).collect::<String>()
            }).unwrap_or_default()
        } else {
            String::new()
        };
        out.push_str(&format!("- {} {} {}（{}，{}）{}\n",
            t, name, status_zh(&status), dur_s,
            if brief.is_empty() { "无参数".to_string() } else { brief },
            if err.is_empty() { String::new() } else { format!(" → {}", err) },
        ));
    }
    Ok(out)
}

// 行映射闭包（tool_history 用）
fn row_map(row: &rusqlite::Row) -> rusqlite::Result<(String, String, Option<i64>, Option<String>, Option<String>, i64)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
}

// ---------------- db_query ----------------

/// db_query：只读 SQL 查询（白名单校验 + 独立只读连接，双保险）。
/// 模型可查 messages / tool_runs / session_events / conversation_todos 等业务表做诊断与复盘。
pub(super) async fn db_query(args: &Value, _roots: &[String], db: &crate::db::DbState) -> Result<String, String> {
    let sql = args["sql"].as_str().ok_or("需要参数 {\"sql\":\"<只读 SELECT 语句>\"}")?;
    let sql = sql.trim();
    if sql.is_empty() {
        return Err("sql 不能为空".into());
    }
    // 1) 语句形态校验：单条 SELECT/WITH（禁分号防多语句注入）
    let upper = sql.to_uppercase();
    let head = upper.split_whitespace().next().unwrap_or("");
    if head != "SELECT" && head != "WITH" {
        return Err(format!("db_query 只允许 SELECT 查询（收到 \"{head} ...\"）"));
    }
    if sql.contains(';') {
        return Err("db_query 只允许单条语句，禁止分号（防止多语句注入）".into());
    }
    // 2) 关键词黑名单（词边界匹配，防 SELECT 里嵌写操作）
    for kw in ["ATTACH", "DETACH", "PRAGMA", "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "REPLACE", "VACUUM", "REINDEX", "ANALYZE"] {
        let re = regex::Regex::new(&format!(r"(?i)\b{}\b", kw)).unwrap();
        if re.is_match(sql) {
            return Err(format!("db_query 为只读查询，语句中含被禁止的关键词 {kw}"));
        }
    }
    // 3) 强制 LIMIT（防大结果集拖垮会话）
    let has_limit = regex::Regex::new(r"(?i)\blimit\s+\d+").unwrap().is_match(sql);
    let final_sql = if has_limit { sql.to_string() } else { format!("{sql} LIMIT 200") };

    // 4) 独立只读连接：从共享连接拿 DB 路径 → open READ_ONLY（PRAGMA query_only 双保险，
    //    不污染共享连接状态）；查询放阻塞线程池并限时 10 秒（防慢查询拖住执行器）
    let path: String = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT file FROM pragma_database_list WHERE name = 'main'", [], |r| r.get(0))
            .map_err(|e| format!("获取数据库路径失败：{e}"))?
    };
    let query = final_sql;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking(move || -> Result<String, String> {
            let ro = rusqlite::Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| format!("打开只读连接失败：{e}"))?;
            ro.execute_batch("PRAGMA query_only=ON;").map_err(|e| e.to_string())?;

            let mut stmt = ro.prepare(&query).map_err(|e| format!("SQL 语法错误：{e}"))?;
            let col_count = stmt.column_count();
            let cols: Vec<String> = (0..col_count).map(|i| stmt.column_name(i).unwrap_or("?").to_string()).collect();
            let mut rows = stmt.query([]).map_err(|e| format!("SQL 执行失败：{e}"))?;
            let mut out = format!("查询结果（{} 列）：\n| {}\n", col_count, cols.join(" | "));
            let mut n = 0usize;
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                if n >= 50 {
                    out.push_str(&format!("…共 {n} 行（截断显示，可用 LIMIT/WHERE 缩小范围）\n"));
                    break;
                }
                let cells: Vec<String> = (0..col_count)
                    .map(|i| {
                        let v: rusqlite::types::Value = row.get(i).unwrap_or(rusqlite::types::Value::Null);
                        let s = match v {
                            rusqlite::types::Value::Null => "NULL".to_string(),
                            rusqlite::types::Value::Integer(x) => x.to_string(),
                            rusqlite::types::Value::Real(x) => format!("{x:.3}"),
                            rusqlite::types::Value::Text(t) => t,
                            rusqlite::types::Value::Blob(b) => format!("<blob {}B>", b.len()),
                        };
                        truncate_cell(&s, 60)
                    })
                    .collect();
                out.push_str(&format!("| {}\n", cells.join(" | ")));
                n += 1;
            }
            if n == 0 {
                out.push_str("（无匹配行）\n");
            }
            out.push_str("\n提示：db_query 为只读查询；跨表关联/视图复杂语句请拆简单查询执行。");
            Ok(out)
        }),
    )
    .await;
    match result {
        Err(_) => Err("查询超时（>10 秒），请缩小范围或加 WHERE 条件后重试".into()),
        Ok(Err(e)) => Err(format!("查询任务失败：{e}")),
        Ok(Ok(out)) => out,
    }
}

fn truncate_cell(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}

// ---------------- share_session / import_session ----------------

/// 会话分享、快照等 JSON 出口统一使用全局脱敏策略。
fn redact(v: &mut serde_json::Value) {
    *v = crate::utils::redact::redact_json_value(v);
}

/// share_session：把会话导出为 JSON 文件（消息 + 事件，脱敏后），默认写项目 .deveco-agent/shared/ 目录。
pub(super) async fn share_session(
    args: &Value,
    roots: &[String],
    conversation_id: &str,
    db: &crate::db::DbState,
) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    // 会话元信息
    let (title, created_at): (String, i64) = conn
        .query_row(
            "SELECT COALESCE(title, '会话'), COALESCE(created_at, 0) FROM conversations WHERE id = ?1",
            [conversation_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| format!("会话不存在：{e}"))?;
    // 消息
    let mut stmt = conn
        .prepare(
            "SELECT role, content, tool_calls_json, model, created_at FROM messages
             WHERE conversation_id = ?1 ORDER BY created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let msgs: Vec<serde_json::Value> = stmt
        .query_map([conversation_id], |r| {
            let tool_calls: Option<String> = r.get(2)?;
            Ok(serde_json::json!({
                "role": r.get::<_, String>(0)?,
                "content": r.get::<_, String>(1)?,
                "tool_calls": tool_calls.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
                "model": r.get::<_, Option<String>>(3)?,
                "created_at": r.get::<_, i64>(4)?,
            }))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(stmt);
    // 事件（脱敏后一并导出，保留可回放性）
    let events: Vec<serde_json::Value> = crate::agent::session_events::replay(&conn, conversation_id)?
        .iter()
        .map(|ev| {
            let mut p = ev.payload.clone();
            redact(&mut p);
            serde_json::json!({
                "seq": ev.seq,
                "event_type": format!("{:?}", ev.event_type),
                "payload": p,
                "trace_id": ev.trace_id,
                "created_at": ev.created_at,
            })
        })
        .collect();
    let mut doc = serde_json::json!({
        "format": "deveco-switch-session",
        "version": 1,
        "exported_at": chrono::Utc::now().timestamp(),
        "conversation": {"id": conversation_id, "title": title, "created_at": created_at},
        "message_count": msgs.len(),
        "messages": msgs,
    });
    let event_count = events.len();
    if !events.is_empty() {
        doc["events"] = serde_json::Value::Array(events);
    }
    // 数据已全部收集，尽早释放数据库锁（后续写文件为同步 IO，不占用共享连接）
    drop(conn);
    // 写文件：默认项目 .deveco-agent/shared/<conversation_id>.share.json
    let dir = match args["out"].as_str() {
        Some(p) => {
            let path = resolve_out_path(roots, p)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("创建导出目录失败：{e}"))?;
            }
            path
        }
        None => {
            let base = roots.first().map(String::as_str).unwrap_or("");
            if base.is_empty() {
                return Err("未绑定项目目录且未提供 out 参数，无法确定导出位置".into());
            }
            let dir = format!("{}/.deveco-agent/shared", base.trim_end_matches(['/', '\\']));
            std::fs::create_dir_all(&dir).map_err(|e| format!("创建导出目录失败：{e}"))?;
            std::path::PathBuf::from(format!("{dir}/{conversation_id}.share.json"))
        }
    };
    let body = serde_json::to_string_pretty(&doc).map_err(|e| format!("序列化失败：{e}"))?;
    std::fs::write(&dir, &body).map_err(|e| format!("写入分享文件失败：{e}"))?;
    Ok(format!(
        "会话已导出（脱敏：api_key/secret/token 等字段已替换为 ***）：{} 条消息，{} 个事件\n文件：{}",
        msgs.len(),
        event_count,
        dir.display()
    ))
}

/// import_session：导入分享的会话 JSON（新会话 + 消息落库，事务保护）。
pub(super) async fn import_session(
    args: &Value,
    roots: &[String],
    project_id: &str,
    db: &crate::db::DbState,
) -> Result<String, String> {
    let path = args["path"].as_str().ok_or("需要参数 {\"path\":\"<分享文件路径>\"}")?;
    let p = crate::agent::tools::resolve_in_roots(roots, path)?;
    // 防超大文件直接读入内存（分享文件 ≤10MB）
    let size = std::fs::metadata(&p).map_err(|e| format!("读取分享文件失败：{e}"))?.len();
    if size > 10 * 1024 * 1024 {
        return Err(format!("分享文件过大（{:.1} MB，上限 10 MB）", size as f64 / 1024.0 / 1024.0));
    }
    let raw = std::fs::read_to_string(&p).map_err(|e| format!("读取分享文件失败：{e}"))?;
    let doc: serde_json::Value = serde_json::from_str(&raw).map_err(|e| format!("分享文件格式错误：{e}"))?;
    if doc["format"].as_str() != Some("deveco-switch-session") {
        return Err("不是有效的会话分享文件（缺少 format=deveco-switch-session 标记）".into());
    }
    let title = doc["conversation"]["title"].as_str().unwrap_or("导入会话").to_string();
    let msgs = doc["messages"].as_array().cloned().unwrap_or_default();
    if msgs.is_empty() {
        return Err("分享文件没有消息内容".into());
    }
    let now = chrono::Utc::now().timestamp();
    let new_id = uuid::Uuid::new_v4().to_string();
    let mut conn = db.0.lock().map_err(|e| e.to_string())?;
    // 事务包裹：中途失败自动回滚，不留只有会话头没有消息的半成品
    let tx = conn.transaction().map_err(|e| format!("开启事务失败：{e}"))?;
    tx.execute(
        "INSERT INTO conversations (id, project_id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
        rusqlite::params![new_id, project_id, format!("{title}（导入）"), now],
    )
    .map_err(|e| format!("创建会话失败：{e}"))?;
    let mut n = 0usize;
    let mut skipped = 0usize;
    for m in msgs.iter().take(500) {
        // role 白名单：只接受标准角色，防分享文件注入 system 等特权角色消息
        let role = m["role"].as_str().unwrap_or("");
        if !["user", "assistant", "tool"].contains(&role) {
            skipped += 1;
            continue;
        }
        let content = m["content"].as_str().unwrap_or("").to_string();
        let ts = m["created_at"].as_i64().unwrap_or(now);
        let tool_calls = if m["tool_calls"].is_null() {
            None
        } else {
            serde_json::to_string(&m["tool_calls"]).ok()
        };
        let model = m["model"].as_str().map(String::from);
        tx.execute(
            "INSERT INTO messages (id, conversation_id, role, content, tool_calls_json, model, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                uuid::Uuid::new_v4().to_string(),
                new_id,
                role,
                content,
                tool_calls,
                model,
                ts,
            ],
        )
        .map_err(|e| format!("写入消息失败：{e}"))?;
        n += 1;
    }
    tx.commit().map_err(|e| format!("提交导入失败：{e}"))?;
    let skip_note = if skipped > 0 { format!("，跳过非标准角色消息 {skipped} 条") } else { String::new() };
    Ok(format!(
        "会话导入成功：新会话 id={new_id}（标题：{}（导入）），共 {n} 条消息{skip_note}\n当前项目已绑定该会话，可直接在会话列表查看。",
        title
    ))
}

// ---------------- trace_export ----------------

/// trace_export：把某 trace_id 的全部事件导出为 JSON（OpenTelemetry 风格 span 列表），
/// 与前端 TimelinePanel 的 trace 折叠配套，可离线分析一次任务的完整链路。
pub(super) async fn trace_export(
    args: &Value,
    roots: &[String],
    conversation_id: &str,
    db: &crate::db::DbState,
) -> Result<String, String> {
    let trace_id = args["trace_id"].as_str().ok_or("需要参数 {\"trace_id\":\"<任务级链路 ID>\"}")?;
    // 文件名净化：只保留安全字符，防路径穿越（默认导出路径用 trace_id 拼文件名）
    let fname: String = trace_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();
    if fname.is_empty() {
        return Err("trace_id 无效：仅允许字母/数字/中划线/下划线".into());
    }
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT seq, event_type, payload, created_at FROM session_events
             WHERE trace_id = ?1 ORDER BY seq ASC",
        )
        .map_err(|e| e.to_string())?;
    let spans: Vec<serde_json::Value> = stmt
        .query_map([trace_id], |r| {
            let payload: String = r.get(2)?;
            Ok(serde_json::json!({
                "name": r.get::<_, String>(1)?,
                "seq": r.get::<_, i64>(0)?,
                "start_time_unix_nano": r.get::<_, i64>(3)?.saturating_mul(1_000_000_000),
                "attributes": serde_json::from_str::<serde_json::Value>(&payload).unwrap_or(serde_json::Value::Null),
            }))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if spans.is_empty() {
        return Err(format!("trace_id={trace_id} 没有匹配的事件（检查 trace_export 前是否发生过任务）"));
    }
    // 数据已收集，尽早释放数据库锁（后续写文件不占用共享连接）
    drop(stmt);
    drop(conn);
    let doc = serde_json::json!({
        "resource_spans": [{
            "resource": {"attributes": {"service.name": "deveco-switch", "trace_id": trace_id, "conversation_id": conversation_id}},
            "scope_spans": [{"name": "agent-trace", "spans": spans}]
        }]
    });
    let dir = match args["out"].as_str() {
        Some(p) => {
            let path = resolve_out_path(roots, p)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("创建导出目录失败：{e}"))?;
            }
            path
        }
        None => {
            let base = roots.first().map(String::as_str).unwrap_or("");
            if base.is_empty() {
                return Err("未绑定项目目录且未提供 out 参数，无法确定导出位置".into());
            }
            let dir = format!("{}/.deveco-agent/traces", base.trim_end_matches(['/', '\\']));
            std::fs::create_dir_all(&dir).map_err(|e| format!("创建导出目录失败：{e}"))?;
            std::path::PathBuf::from(format!("{dir}/{fname}.json"))
        }
    };
    std::fs::write(&dir, serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?)
        .map_err(|e| format!("写入 trace 文件失败：{e}"))?;
    Ok(format!(
        "trace 已导出：{} 个事件（OpenTelemetry 格式）\n文件：{}",
        spans.len(),
        dir.display()
    ))
}

/// out 参数解析：相对路径挂在第一个有效根下，绝对路径直用；
/// 与文件工具同一安全口径（resolve_for_write：根内约束 + .. 防越界，允许目标尚不存在）
fn resolve_out_path(roots: &[String], raw: &str) -> Result<std::path::PathBuf, String> {
    crate::agent::tools::resolve_for_write(roots, raw)
}

// ---------------- permission_audit：工具使用安全审计 ----------------

/// 权限审计：聚合 tool_runs 调用统计 + 权限分级，输出审计报告。
/// 参数：{"days":<可选天数窗口，缺省全部>,"level":"L0|L1|L2"（可选只看某级）,"min_calls":<可选最少调用次数过滤>}。
/// 输出：总览（调用/成功率/危险占比）+ 按级别分组的工具使用排行 + 风险提示。
pub(super) async fn permission_audit(
    args: &Value,
    _roots: &[String],
    project_id: &str,
    db: &crate::db::DbState,
) -> Result<String, String> {
    let days = args["days"].as_i64();
    let level_filter = args["level"].as_str().map(|s| s.to_uppercase());
    if let Some(l) = &level_filter {
        if l != "L0" && l != "L1" && l != "L2" {
            return Err(format!("level 参数只接受 L0/L1/L2（收到 {l}）"));
        }
    }
    let min_calls = args["min_calls"].as_i64().unwrap_or(0);

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let stats = crate::db::queries::list_tool_stats(&conn, project_id).map_err(|e| e.to_string())?;
    // 按天数窗口过滤（last_called_at 是 unix 秒；days=0 表示全部）
    let cutoff = days
        .filter(|d| *d > 0)
        .map(|d| chrono::Utc::now().timestamp() - d * 86400);
    let stats: Vec<&crate::db::models::ToolStat> = stats
        .iter()
        .filter(|s| cutoff.map(|c| s.last_called_at.unwrap_or(0) >= c).unwrap_or(true))
        .filter(|s| s.call_count >= min_calls)
        .filter(|s| {
            level_filter
                .as_deref()
                .map(|l| crate::services::permissions::tool_level(&s.tool_name).as_str() == l)
                .unwrap_or(true)
        })
        .collect();
    if stats.is_empty() {
        return Ok("暂无工具调用记录（此项目还没有审计数据）".into());
    }

    let total_calls: i64 = stats.iter().map(|s| s.call_count).sum();
    let total_fail: i64 = stats.iter().map(|s| s.fail_count).sum();
    let l2_calls: i64 = stats
        .iter()
        .filter(|s| crate::services::permissions::tool_level(&s.tool_name) == crate::services::permissions::Level::L2)
        .map(|s| s.call_count)
        .sum();
    let l2_names: Vec<&str> = stats
        .iter()
        .filter(|s| crate::services::permissions::tool_level(&s.tool_name) == crate::services::permissions::Level::L2)
        .map(|s| s.tool_name.as_str())
        .collect();

    let mut out = String::new();
    out.push_str(&format!(
        "工具权限审计报告（{} 个工具 / {} 次调用）\n成功率 {:.1}% ｜ 失败 {} 次 ｜ L2 危险级调用 {} 次（{:.1}%）\n",
        stats.len(),
        total_calls,
        if total_calls > 0 { (total_calls - total_fail) as f64 * 100.0 / total_calls as f64 } else { 100.0 },
        total_fail,
        l2_calls,
        if total_calls > 0 { l2_calls as f64 * 100.0 / total_calls as f64 } else { 0.0 }
    ));
    if !l2_names.is_empty() {
        out.push_str(&format!("涉及的危险级工具：{}\n", l2_names.join(" / ")));
    }

    for level in ["L2", "L1", "L0"] {
        let mut bucket: Vec<_> = stats
            .iter()
            .filter(|s| crate::services::permissions::tool_level(&s.tool_name).as_str() == level)
            .collect();
        if bucket.is_empty() {
            continue;
        }
        bucket.sort_by(|a, b| b.call_count.cmp(&a.call_count));
        out.push_str(&format!("\n## {level}（{} 个）\n", bucket.len()));
        for s in bucket.iter().take(15) {
            let rate = if s.call_count > 0 {
                (s.call_count - s.fail_count) as f64 * 100.0 / s.call_count as f64
            } else {
                100.0
            };
            let avg = s
                .avg_duration_ms
                .map(|d| format!("{:.1}s", d as f64 / 1000.0))
                .unwrap_or_else(|| "-".into());
            out.push_str(&format!(
                "  {} ×{} ｜ 成功率 {:.0}% ｜ 平均 {avg}\n",
                s.tool_name, s.call_count, rate
            ));
        }
        if bucket.len() > 15 {
            out.push_str(&format!("  …另 {} 个工具\n", bucket.len() - 15));
        }
    }

    // 风险提示：L2 高频 / 失败率高企的工具
    let mut tips: Vec<String> = Vec::new();
    for s in &stats {
        let lv = crate::services::permissions::tool_level(&s.tool_name);
        if lv == crate::services::permissions::Level::L2 && s.call_count >= 5 {
            tips.push(format!(
                "{} 被调用 {} 次（L2 危险级，建议核对调用场景是否必要）",
                s.tool_name, s.call_count
            ));
        }
        if s.call_count >= 3 && s.fail_count as f64 / s.call_count as f64 > 0.5 {
            tips.push(format!(
                "{} 失败率 {:.0}%（{}/{}），可能存在配置/使用方式问题",
                s.tool_name,
                s.fail_count as f64 * 100.0 / s.call_count as f64,
                s.fail_count,
                s.call_count
            ));
        }
    }
    if !tips.is_empty() {
        out.push_str(&format!("\n## 风险提示\n{}", tips.join("\n")));
    }

    // [69] token 维度：LLM 调用按模型聚合（request_logs 未记工具名，无法按工具 join，
    // 口径为全部会话/项目的模型消耗；days 与上方工具统计共用同一时间窗口）
    match crate::db::queries::list_model_token_stats(&conn, days.unwrap_or(0)) {
        Ok(models) if !models.is_empty() => {
            out.push_str("\n## 模型 token 消耗排行\n");
            let tot_req: i64 = models.iter().map(|m| m.request_count).sum();
            let tot_in: i64 = models.iter().map(|m| m.input_tokens).sum();
            let tot_out: i64 = models.iter().map(|m| m.output_tokens).sum();
            let tot_cost: f64 = models.iter().map(|m| m.total_cost_cny).sum();
            out.push_str(&format!(
                "全部模型 {} 次请求 ｜ 输入 {} tok ｜ 输出 {} tok ｜ 费用 ¥{:.2}\n",
                tot_req, tot_in, tot_out, tot_cost
            ));
            for m in models.iter().take(8) {
                let avg = m
                    .avg_latency_ms
                    .map(|d| format!("{:.1}s", d as f64 / 1000.0))
                    .unwrap_or_else(|| "-".into());
                out.push_str(&format!(
                    "  {} ｜ {} 次 ｜ 入 {} / 出 {} / 缓存 {} tok ｜ ¥{:.4} ｜ 平均 {avg}\n",
                    m.model, m.request_count, m.input_tokens, m.output_tokens, m.cache_tokens, m.total_cost_cny
                ));
            }
            if models.len() > 8 {
                out.push_str(&format!("  …另 {} 个模型\n", models.len() - 8));
            }
        }
        _ => {}
    }
    Ok(out)
}

// ---------------- db_migrate：数据库迁移管理 ----------------

/// db_migrate：查看/应用数据库迁移（与启动时自动迁移共用同一清单）。
/// 参数：{"action":"status|apply"（缺省 status）}。
/// status：列出全部迁移的已应用状态；apply：补跑所有未应用的迁移（幂等，失败回滚）。
pub(super) async fn db_migrate(args: &Value, _roots: &[String], db: &crate::db::DbState) -> Result<String, String> {
    let action = args["action"].as_str().unwrap_or("status");
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    match action {
        "status" => {
            let list = crate::db::migration_status(&conn).map_err(|e| e.to_string())?;
            let applied_n = list.iter().filter(|(_, _, a, _)| *a).count();
            let total_n = list.len();
            let mut out = format!(
                "数据库迁移状态：{}/{} 已应用\n",
                applied_n,
                list.len()
            );
            for (id, name, applied, at) in list {
                let mark = if applied { "✅" } else { "⬜" };
                let when = at
                    .map(|t| {
                        chrono::DateTime::from_timestamp(t, 0)
                            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_else(|| "-".into())
                    })
                    .unwrap_or_else(|| "-".into());
                out.push_str(&format!("  {mark} #{id} {name}（{when}）\n"));
            }
            if applied_n < total_n {
                out.push_str("\n有未应用迁移，可调用 db_migrate action=apply 补跑（幂等）。");
            }
            Ok(out)
        }
        "apply" => {
            let n = crate::db::apply_pending_migrations(&conn).map_err(|e| e.to_string())?;
            if n == 0 {
                Ok("迁移已全部应用，无需补跑。".into())
            } else {
                Ok(format!("已补跑 {n} 个未应用迁移（每条独立事务，失败自动回滚）。"))
            }
        }
        other => Err(format!("未知 action \"{other}\"。可用：status|apply")),
    }
}

// ---------------- state_snapshot：关键表 JSON 快照（导出/恢复/列表） ----------------

/// 快照覆盖的关键表（白名单；表名不可参数化，绝不拼接外部输入）
const SNAPSHOT_TABLES: [&str; 6] = [
    "settings",
    "projects",
    "project_memories",
    "knowledge_entries",
    "mcp_servers",
    "providers",
];

/// 列名是否含敏感词（导出时掩码为 ***，防止快照文件泄露密钥）
fn snapshot_sensitive_col(k: &str) -> bool {
    let l = k.to_lowercase();
    ["api_key", "apikey", "secret", "password", "token"]
        .iter()
        .any(|kw| l.contains(kw))
}

/// 对导出值做脱敏：敏感列 → ***；JSON 字符串（如 mcp_servers.env）递归掩码敏感键的值
fn snapshot_redact(col: &str, v: rusqlite::types::Value) -> serde_json::Value {
    let s = match &v {
        rusqlite::types::Value::Text(t) => t.clone(),
        _ => {
            // rusqlite::types::Value 无 Serialize 实现，手动映射为 JSON 值
            let j = match &v {
                rusqlite::types::Value::Null => serde_json::Value::Null,
                rusqlite::types::Value::Integer(i) => serde_json::Value::Number((*i).into()),
                rusqlite::types::Value::Real(f) => serde_json::Number::from_f64(*f)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null),
                rusqlite::types::Value::Blob(b) => {
                    serde_json::Value::String(String::from_utf8_lossy(b).into_owned())
                }
                _ => serde_json::Value::Null,
            };
            return j;
        }
    };
    if snapshot_sensitive_col(col) {
        return serde_json::Value::String("***".into());
    }
    // JSON 文本列：递归脱敏
    if let Ok(mut j) = serde_json::from_str::<serde_json::Value>(&s) {
        redact(&mut j);
        // 有实际替换时输出脱敏后的 JSON，否则原样
        return j;
    }
    serde_json::Value::String(s)
}

/// 读取表结构列名（PRAGMA table_info；表名来自白名单）
fn table_columns(conn: &rusqlite::Connection, table: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .map_err(|e| e.to_string())?;
    let cols = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(cols)
}

/// state_snapshot：把关键表（settings/projects/project_memories/knowledge_entries/mcp_servers/providers）
/// 导出为可读 JSON 快照（敏感列掩码），或从快照文件恢复（按主键 INSERT OR REPLACE 合并）。
/// 参数：{"action":"export|import|list"（缺省 export）,"path":"<可选快照文件路径>","tables":["<可选子集>"],"dest":"<可选导出目录>"}。
pub(super) async fn state_snapshot(
    args: &Value,
    roots: &[String],
    db: &crate::db::DbState,
) -> Result<String, String> {
    let action = args["action"].as_str().unwrap_or("export");
    let tables: Vec<&str> = match args["tables"].as_array() {
        Some(arr) => {
            let list: Vec<&str> = arr
                .iter()
                .filter_map(|v| v.as_str())
                .filter(|t| SNAPSHOT_TABLES.contains(t))
                .collect();
            if list.is_empty() {
                return Err(format!(
                    "tables 只接受白名单子集：{}",
                    SNAPSHOT_TABLES.join(" / ")
                ));
            }
            list
        }
        None => SNAPSHOT_TABLES.to_vec(),
    };

    match action {
        "export" => {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let mut doc = serde_json::Map::new();
            doc.insert("exported_at".into(), serde_json::json!(chrono::Utc::now().timestamp()));
            doc.insert("app".into(), serde_json::json!("deveco-switch"));
            let mut table_doc = serde_json::Map::new();
            for t in &tables {
                let cols = table_columns(&conn, t)?;
                if cols.is_empty() {
                    continue;
                }
                let quoted: Vec<String> = cols.iter().map(|c| format!("\"{c}\"")).collect();
                let sql = format!("SELECT {} FROM \"{t}\"", quoted.join(","));
                let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
                let mut rows: Vec<serde_json::Value> = Vec::new();
                let mut iter = stmt.query([]).map_err(|e| e.to_string())?;
                while let Some(row) = iter.next().map_err(|e| e.to_string())? {
                    let mut obj = serde_json::Map::new();
                    for (i, c) in cols.iter().enumerate() {
                        let v: rusqlite::types::Value = row.get(i).unwrap_or(rusqlite::types::Value::Null);
                        obj.insert(c.clone(), snapshot_redact(c, v));
                    }
                    rows.push(serde_json::Value::Object(obj));
                }
                table_doc.insert(t.to_string(), serde_json::json!({"columns": cols, "rows": rows}));
            }
            drop(conn);
            doc.insert("tables".into(), serde_json::Value::Object(table_doc));

            // 默认目录：项目 .deveco-agent/snapshots/
            let base_dir = roots
                .first()
                .map(|r| format!("{r}/.deveco-agent/snapshots"))
                .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().to_string());
            let dest = args["dest"].as_str().map(String::from).unwrap_or(base_dir);
            std::fs::create_dir_all(&dest).map_err(|e| format!("创建快照目录失败：{e}"))?;
            let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
            let file = std::path::Path::new(&dest).join(format!("state-{ts}.json"));
            std::fs::write(&file, serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?)
                .map_err(|e| format!("写入快照失败：{e}"))?;
            let mut out = format!("状态快照已导出：{}\n", file.display());
            for t in &tables {
                let n = doc["tables"][t]["rows"].as_array().map(|a| a.len()).unwrap_or(0);
                out.push_str(&format!("  {t}: {n} 行\n"));
            }
            out.push_str("\n敏感字段（api_key/token/secret 等）已掩码为 ***，导入后需重新配置密钥。");
            Ok(out)
        }
        "import" => {
            let path = args["path"].as_str().ok_or("import 需要参数 path（快照 JSON 文件路径）")?;
            let resolved = resolve_out_path(roots, path)?;
            let text = std::fs::read_to_string(&resolved).map_err(|e| format!("读取快照失败：{e}"))?;
            let doc: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| format!("解析快照失败：{e}"))?;
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
            let mut imported = 0usize;
            let mut skipped: Vec<String> = Vec::new();
            for t in &tables {
                let Some(tdoc) = doc["tables"][t].as_object() else { continue };
                let cols = match tdoc.get("columns").and_then(|c| c.as_array()) {
                    Some(c) => c.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>(),
                    None => continue,
                };
                let Some(rows) = tdoc.get("rows").and_then(|r| r.as_array()) else { continue };
                if cols.is_empty() {
                    continue;
                }
                // 校验列存在性：快照列必须是当前表实际列的子集，防止导入错误 schema
                let actual = table_columns(&tx, t)?;
                if cols.iter().any(|c| !actual.contains(&c.to_string())) {
                    skipped.push(format!("{t}（列不匹配，已跳过）"));
                    continue;
                }
                let quoted: Vec<String> = cols.iter().map(|c| format!("\"{c}\"")).collect();
                let placeholders: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
                let sql = format!(
                    "INSERT OR REPLACE INTO \"{t}\" ({}) VALUES ({})",
                    quoted.join(","),
                    placeholders.join(",")
                );
                for row in rows {
                    let vals: Vec<rusqlite::types::Value> = cols
                        .iter()
                        .map(|c| match row.get(*c) {
                            Some(v) => {
                                if v.is_null() {
                                    rusqlite::types::Value::Null
                                } else if let Some(s) = v.as_str() {
                                    rusqlite::types::Value::Text(s.to_string())
                                } else if let Some(n) = v.as_i64() {
                                    rusqlite::types::Value::Integer(n)
                                } else if let Some(f) = v.as_f64() {
                                    rusqlite::types::Value::Real(f)
                                } else {
                                    rusqlite::types::Value::Null
                                }
                            }
                            None => rusqlite::types::Value::Null,
                        })
                        .collect();
                    let params = rusqlite::params_from_iter(vals.iter());
                    tx.execute(&sql, params).map_err(|e| e.to_string())?;
                    imported += 1;
                }
            }
            tx.commit().map_err(|e| e.to_string())?;
            drop(conn);
            let mut out = format!("状态快照恢复完成：共写入 {imported} 行（按主键合并，重复数据被覆盖）\n");
            for t in &tables {
                let n = doc["tables"][t]["rows"].as_array().map(|a| a.len()).unwrap_or(0);
                out.push_str(&format!("  {t}: 快照 {n} 行\n"));
            }
            if !skipped.is_empty() {
                out.push_str(&format!("跳过：{}\n", skipped.join("；")));
            }
            out.push_str("\n提示：api_key 等敏感字段导出时已掩码为 ***，如快照中包含 *** 需重新配置密钥。");
            Ok(out)
        }
        "list" => {
            let base_dir = roots
                .first()
                .map(|r| format!("{r}/.deveco-agent/snapshots"))
                .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().to_string());
            let Ok(entries) = std::fs::read_dir(&base_dir) else {
                return Ok(format!("快照目录不存在：{base_dir}"));
            };
            let mut files: Vec<(String, u64)> = entries
                .flatten()
                .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
                .filter_map(|e| {
                    let meta = e.metadata().ok()?;
                    Some((e.file_name().to_string_lossy().to_string(), meta.len()))
                })
                .collect();
            files.sort_by(|a, b| b.0.cmp(&a.0));
            if files.is_empty() {
                return Ok(format!("快照目录中没有 JSON 快照：{base_dir}"));
            }
            let mut out = format!("快照文件（{} 个）：\n", files.len());
            for (f, sz) in files.iter().take(20) {
                out.push_str(&format!("  {f}（{} KB）\n", sz / 1024));
            }
            out.push_str("\n恢复：state_snapshot action=import path=<文件路径>");
            Ok(out)
        }
        other => Err(format!("未知 action \"{other}\"。可用：export|import|list")),
    }
}

// ---------------- reflexion_query / reflexion_pin：反思卡片显式管理 ----------------

/// reflexion_query：查看当前反思卡片（失败模式/证据/建议/钉住状态），
/// 与自动注入 system prompt 的 format_hint 同源。
pub(super) async fn reflexion_query(args: &Value, _roots: &[String]) -> Result<String, String> {
    let limit = (args["limit"].as_u64().unwrap_or(20) as usize).clamp(1, 50);
    let cards = crate::agent::reflexion::query_cards();
    if cards.is_empty() {
        return Ok("（暂无反思卡片：任务结束时会自动分析最近一轮失败并沉淀教训；连续失败 ≥2 次才会成卡）".into());
    }
    let mut out = format!("反思卡片（{} 张，新→旧）：\n", cards.len().min(limit));
    for c in cards.iter().take(limit) {
        let t = chrono::DateTime::from_timestamp(c.at, 0)
            .map(|d| d.format("%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "-".into());
        out.push_str(&format!(
            "- [{}] {}{}\n  证据：{}\n  建议：{}\n  记录：{}\n",
            c.tool,
            c.pattern,
            if c.pinned { " 🔒已钉住" } else { "" },
            if c.evidence.is_empty() { "-".to_string() } else { c.evidence.clone() },
            c.advice,
            t
        ));
    }
    out.push_str("\n钉住某工具的卡片（不受 TTL 清理）：reflexion_pin tool=<工具名>；解除：reflexion_pin tool=<工具名> pin=false");
    Ok(out)
}

/// reflexion_pin：钉住/解除钉住某工具的反思卡片（钉住后常驻注入 system prompt）。
pub(super) async fn reflexion_pin(args: &Value, _roots: &[String]) -> Result<String, String> {
    let tool = args["tool"].as_str().ok_or("需要参数 {\"tool\":\"<工具名>\",\"pin\":<可选 true/false>}")?;
    let pinned = args["pin"].as_bool().unwrap_or(true);
    crate::agent::reflexion::pin_card(tool, pinned)
}

// ---------------- export_report：工作报告导出（Markdown → HTML → PDF） ----------------

/// 极简 Markdown → HTML 渲染（工作报告常用子集：标题/列表/表格/代码块/引用/强调）。
/// 不引入依赖；复杂表格/公式场景建议直接传 HTML。
fn md_to_html(md: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let mut in_code = false;
    let mut in_table = false;
    let mut table_rows: Vec<String> = Vec::new();
    let mut list_stack: Vec<bool> = Vec::new(); // 当前是否处于有序列表（ul/ol）

    let escape = |s: &str| -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    };
    let inline = |s: &str| -> String {
        let s = escape(s);
        // `code` → <code>（先处理，避免与粗体/斜体正则冲突）
        let mut buf = String::new();
        let mut rest = s.as_str();
        while let Some(i) = rest.find('`') {
            buf.push_str(&rest[..i]);
            rest = &rest[i + 1..];
            if let Some(j) = rest.find('`') {
                buf.push_str(&format!("<code>{}</code>", escape(&rest[..j])));
                rest = &rest[j + 1..];
            } else {
                buf.push('`');
            }
        }
        buf.push_str(rest);
        // **粗体** / *斜体*（非贪婪配对）
        let bold = regex::Regex::new(r"\*\*(.+?)\*\*").unwrap();
        let buf = bold.replace_all(&buf, "<b>$1</b>").to_string();
        // Rust regex 不支持 look-around；粗体已先替换完，此时剩余成对星号可直接处理。
        let italic = regex::Regex::new(r"\*([^*]+)\*").unwrap();
        italic.replace_all(&buf, "<i>$1</i>").to_string()
    };

    for line in md.lines() {
        // 代码块
        if line.trim_start().starts_with("```") {
            if in_code {
                out.push_str("</pre>\n");
                in_code = false;
            } else {
                out.push_str("<pre>");
                in_code = true;
            }
            continue;
        }
        if in_code {
            out.push_str(&format!("{}\n", escape(line)));
            continue;
        }
        // 表格行
        if line.trim_start().starts_with('|') {
            let cells: Vec<String> = line
                .trim()
                .trim_start_matches('|')
                .trim_end_matches('|')
                .split('|')
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
                .map(|c| inline(c))
                .collect();
            let is_sep = cells.iter().all(|c| c.trim().matches(['-', ':']).count() == c.trim().chars().count() && !c.trim().is_empty());
            if is_sep {
                continue; // 分隔行
            }
            if !in_table {
                table_rows.clear();
                out.push_str("<table><thead><tr>");
                for c in &cells {
                    let _ = write!(out, "<th>{c}</th>");
                }
                out.push_str("</tr></thead><tbody>");
                in_table = true;
            } else {
                out.push_str("<tr>");
                for c in &cells {
                    let _ = write!(out, "<td>{c}</td>");
                }
                out.push_str("</tr>");
            }
            continue;
        } else if in_table {
            out.push_str("</tbody></table>\n");
            in_table = false;
        }
        // 标题
        if let Some(rest) = line.strip_prefix("### ") {
            out.push_str(&format!("<h3>{}</h3>\n", inline(rest)));
            continue;
        }
        if let Some(rest) = line.strip_prefix("## ") {
            out.push_str(&format!("<h2>{}</h2>\n", inline(rest)));
            continue;
        }
        if let Some(rest) = line.strip_prefix("# ") {
            out.push_str(&format!("<h1>{}</h1>\n", inline(rest)));
            continue;
        }
        // 列表
        if line.trim_start().starts_with("- ") || line.trim_start().starts_with("* ") {
            let item = inline(line.trim_start().trim_start_matches(['-', '*', ' ']));
            if list_stack.last() != Some(&false) {
                out.push_str("<ul>\n");
                list_stack.push(false);
            }
            out.push_str(&format!("<li>{item}</li>\n"));
            continue;
        }
        if let Some(rest) = line.trim_start().strip_prefix("1. ") {
            let item = inline(rest);
            if list_stack.last() != Some(&true) {
                out.push_str("<ol>\n");
                list_stack.push(true);
            }
            out.push_str(&format!("<li>{item}</li>\n"));
            continue;
        }
        if !list_stack.is_empty() {
            for _ in list_stack.drain(..) {}
            out.push_str("</ul>\n");
        }
        // 引用
        if line.trim_start().starts_with('>') {
            out.push_str(&format!("<blockquote>{}</blockquote>\n", inline(line.trim_start().trim_start_matches('>').trim())));
            continue;
        }
        // 分隔线
        if line.trim().matches('-').count() >= 3 && line.trim().chars().all(|c| c == '-') {
            out.push_str("<hr/>\n");
            continue;
        }
        // 空行
        if line.trim().is_empty() {
            continue;
        }
        out.push_str(&format!("<p>{}</p>\n", inline(line)));
    }
    if in_code {
        out.push_str("</pre>\n");
    }
    if in_table {
        out.push_str("</tbody></table>\n");
    }
    if !list_stack.is_empty() {
        out.push_str("</ul>\n");
    }
    out
}

/// 生成自包含 HTML 报告（内嵌样式，可直接浏览器打开）
fn report_html(title: &str, body: &str) -> String {
    format!(
        "<!DOCTYPE html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><title>{}</title>\n\
         <style>\n\
         body{{font-family:'Segoe UI','Microsoft YaHei',sans-serif;max-width:860px;margin:32px auto;padding:0 24px;color:#222;line-height:1.7}}\
         h1{{border-bottom:2px solid #3b82f6;padding-bottom:8px}} h2{{margin-top:28px;border-left:4px solid #3b82f6;padding-left:10px}}\
         table{{border-collapse:collapse;width:100%;margin:12px 0}} th,td{{border:1px solid #ddd;padding:6px 10px;text-align:left}} th{{background:#f1f5f9}}\
         pre{{background:#f8fafc;border:1px solid #e2e8f0;border-radius:6px;padding:12px;overflow-x:auto}} code{{background:#f1f5f9;padding:1px 5px;border-radius:4px}}\
         blockquote{{border-left:4px solid #cbd5e1;margin:10px 0;padding:2px 14px;color:#475569}}\
         .meta{{color:#64748b;font-size:13px;margin-bottom:24px}}\
         </style></head><body>\n\
         <div class=\"meta\">生成时间：{} ｜ 工具：deveco-switch export_report</div>\n\
         <h1>{}</h1>\n{}</body></html>",
        escape_html(title),
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        escape_html(title),
        body
    )
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// 尝试用本机 Edge/Chrome headless 把 HTML 打印为 PDF；找不到浏览器返回 None。
async fn try_html_to_pdf(html_path: &std::path::Path, pdf_path: &std::path::Path) -> Option<String> {
    let candidates = [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ];
    let exe = candidates.iter().find(|p| std::path::Path::new(p).is_file())?;
    let url = format!("file:///{}", html_path.display().to_string().replace('\\', "/"));
    let out = crate::agent::tools::run_cmd(
        exe,
        &[
            "--headless".to_string(),
            "--disable-gpu".to_string(),
            "--no-sandbox".to_string(),
            format!("--print-to-pdf={}", pdf_path.display()),
            url,
        ],
        None,
        60,
    )
    .await;
    match out {
        Ok(_) if pdf_path.is_file() => Some(exe.to_string()),
        _ => None,
    }
}

/// export_report：把 Markdown 工作报告导出为 HTML / PDF（工作报告场景：
/// 会话总结、测试报告、审计结论留档）。
/// 参数：{"content":"<Markdown 正文>"} 或 {"path":"<Markdown 文件路径>"}，
/// {"title":"<可选标题，缺省取第一行 # 标题>","out":"<可选输出目录，缺省项目 .deveco-agent/reports>",
///  "format":"html|pdf|both（缺省 both：先 HTML 再尽力转 PDF，浏览器缺失时只出 HTML）"}。
pub(super) async fn export_report(args: &Value, roots: &[String]) -> Result<String, String> {
    // 1) 取 Markdown 正文
    let md = if let Some(c) = args["content"].as_str() {
        c.to_string()
    } else if let Some(p) = args["path"].as_str() {
        let resolved = crate::agent::tools::resolve_in_roots(roots, p)?;
        if !resolved.is_file() {
            return Err(format!("Markdown 文件不存在: {}", resolved.display()));
        }
        std::fs::read_to_string(&resolved).map_err(|e| format!("读取 Markdown 失败：{e}"))?
    } else {
        return Err("export_report 需要 content（Markdown 正文）或 path（Markdown 文件）参数".into());
    };
    let md_trim = md.trim();
    if md_trim.is_empty() {
        return Err("Markdown 内容为空".into());
    }
    // 2) 标题：显式参数 > 首个 # 标题 > 默认
    let title = args["title"]
        .as_str()
        .map(String::from)
        .or_else(|| {
            md_trim.lines().find(|l| l.starts_with("# ")).map(|l| l.trim_start_matches("# ").trim().to_string())
        })
        .unwrap_or_else(|| "工作报告".into());
    // 3) 输出目录
    let base = roots.first().map(String::as_str).unwrap_or("");
    let dir = match args["out"].as_str() {
        Some(p) => {
            let d = resolve_out_path(roots, p)?;
            std::fs::create_dir_all(&d).map_err(|e| format!("创建输出目录失败：{e}"))?;
            d
        }
        None => {
            if base.is_empty() {
                return Err("未绑定项目目录且未提供 out 参数，无法确定输出位置".into());
            }
            let d = std::path::PathBuf::from(format!("{}/.deveco-agent/reports", base.trim_end_matches(['/', '\\'])));
            std::fs::create_dir_all(&d).map_err(|e| format!("创建输出目录失败：{e}"))?;
            d
        }
    };
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let safe: String = title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .take(40)
        .collect();
    let safe = if safe.is_empty() { "report".to_string() } else { safe };
    let html_body = md_to_html(md_trim);
    let html_doc = report_html(&title, &html_body);
    let html_path = dir.join(format!("{safe}-{stamp}.html"));
    std::fs::write(&html_path, &html_doc).map_err(|e| format!("写入 HTML 失败：{e}"))?;

    let fmt = args["format"].as_str().unwrap_or("both");
    let mut out = format!("报告已导出：\nHTML：{}\n", html_path.display());
    if fmt == "html" {
        out.push_str("\n（format=html，仅生成 HTML；需要 PDF 时传 format=pdf|both）");
        return Ok(out);
    }
    // 4) PDF（尽力而为）：headless 浏览器打印
    let pdf_path = dir.join(format!("{safe}-{stamp}.pdf"));
    match try_html_to_pdf(&html_path, &pdf_path).await {
        Some(exe) => {
            out.push_str(&format!("PDF：{}（经 {}\n", pdf_path.display(), exe));
            Ok(out)
        }
        None => {
            if fmt == "pdf" {
                out.push_str("\n⚠ 未找到 Edge/Chrome，无法生成 PDF；已保留 HTML（可直接浏览器打开后打印为 PDF）。");
            } else {
                out.push_str("\n⚠ 未找到 Edge/Chrome，PDF 跳过；HTML 已生成（浏览器打开后 Ctrl+P 可存 PDF）。");
            }
            Ok(out)
        }
    }
}

/// 工具失败模式 → 优化建议（[40] prompt_optimize 的规则表）
fn fail_pattern_advice(tool: &str, err_sample: &str) -> Option<&'static str> {
    use super::errors::diagnose_tool_error;
    diagnose_tool_error(tool, err_sample)
}

/// prompt_optimize：收集最近失败的任务/工具执行记录，聚类失败模式并给出优化建议。
/// 不调用模型（离线分析），输出可直接指导模型调整行为，或人工写入规则/记忆。
pub(super) async fn prompt_optimize(
    args: &Value,
    _roots: &[String],
    _project_id: &str,
    db: &crate::db::DbState,
) -> Result<String, String> {
    let days = args["days"].as_i64().unwrap_or(7).max(1);
    let min_fail = args["min_fail"].as_i64().unwrap_or(1).max(1);
    let limit = args["limit"].as_i64().unwrap_or(10).clamp(1, 30) as usize;
    let cutoff = chrono::Utc::now().timestamp() - days * 86400;
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    // 1) 工具级失败聚合（tool_runs）
    let mut stmt = conn
        .prepare(
            "SELECT tool_name, status, result_json, COUNT(*) FROM tool_runs \
             WHERE created_at >= ?1 AND status = 'error' \
             GROUP BY tool_name, substr(result_json, 1, 160) ORDER BY COUNT(*) DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, String, String, i64)> = stmt
        .query_map([cutoff], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default(), r.get(3)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    drop(stmt);

    // 2) 任务级失败聚合（task_runs，按 error_kind）
    let mut stmt2 = conn
        .prepare(
            "SELECT error_kind, COUNT(*) FROM task_runs \
             WHERE started_at >= ?1 AND status = 'error' AND error_kind IS NOT NULL \
             GROUP BY error_kind ORDER BY COUNT(*) DESC",
        )
        .map_err(|e| e.to_string())?;
    let task_rows: Vec<(String, i64)> = stmt2
        .query_map([cutoff], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;

    // 3) 组装输出
    let mut out = format!(
        "近 {days} 天失败模式分析（tool_runs {}/task_runs {}，聚合门槛 ≥{} 次）：\n",
        rows.len(),
        task_rows.len(),
        min_fail
    );
    if rows.is_empty() && task_rows.is_empty() {
        out.push_str("\n无失败记录：执行链路健康，无需调整行为。");
        return Ok(out);
    }
    let mut shown = 0;
    for (tool, status, sample, cnt) in rows {
        if cnt < min_fail || shown >= limit {
            continue;
        }
        let sample_short: String = sample
            .chars()
            .filter(|c| !c.is_control())
            .take(140)
            .collect();
        out.push_str(&format!(
            "\n[{cnt} 次] {tool}（最近状态：{status}）\n  错误样本：{}\n",
            if sample_short.is_empty() { "（无输出）" } else { &sample_short }
        ));
        if let Some(adv) = fail_pattern_advice(&tool, &sample_short) {
            out.push_str(&format!("  建议：{adv}\n"));
        }
        shown += 1;
    }
    if !task_rows.is_empty() {
        out.push_str("\n任务级失败归类：\n");
        for (kind, cnt) in task_rows {
            if cnt < min_fail {
                continue;
            }
            out.push_str(&format!("- {kind}：{cnt} 次\n"));
        }
    }
    out.push_str("\n用法：根据以上高频失败模式调整行为（如构建前先查 SDK 对齐、部署前先 list_devices）；如需长期固化，可把改进规则写入记忆/技能。");
    Ok(out)
}

/// export_tools_meta：导出全部工具元数据为 JSON 快照（只导出，不改变运行时加载）。
pub(super) async fn export_tools_meta(args: &Value, roots: &[String]) -> Result<String, String> {
    let out_path = match args["out"].as_str() {
        Some(p) => resolve_out_path(roots, p)?,
        None => {
            let base = roots.first().map(String::as_str).unwrap_or("");
            if base.is_empty() {
                return Err("未绑定项目目录且未提供 out 参数，无法确定输出位置".into());
            }
            std::path::PathBuf::from(format!("{}/.deveco-agent/tools_meta.json", base.trim_end_matches(['/', '\\'])))
        }
    };
    if let Some(dir) = out_path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建输出目录失败：{e}"))?;
    }
    let tools: Vec<serde_json::Value> = super::TOOL_SPECS
        .iter()
        .map(|t| {
            let meta = meta_for(t.name);
            let contract = crate::agent::tools::contracts::contract(t.name);
            serde_json::json!({
                "name": t.name,
                "desc": t.desc,
                "group": super::tool_group(t.name),
                "level": crate::services::permissions::tool_level(t.name).as_str(),
                "timeout_hint": meta.map(|m| m.timeout_hint).unwrap_or(""),
                "retry_policy": meta.map(|m| m.retry_policy).unwrap_or(""),
                "cost_hint": meta.map(|m| m.cost_hint).unwrap_or(""),
                "contract": contract,
            })
        })
        .collect();
    let doc = serde_json::json!({
        "schema": "deveco-agent/tools_meta/v1",
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "count": tools.len(),
        "tools": tools,
    });
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(&out_path, text).map_err(|e| format!("写入失败：{e}"))?;
    Ok(format!("已导出 {} 个工具元数据 → {}", tools.len(), out_path.display()))
}
