//! 记忆/知识/计划/导出/成本域工具：save_memory / search_knowledge / plan_task / export_data / get_cost_summary 等。
//! 共享辅助函数（run_cmd / run_hdc_shell / tail / truncate_out / emit_knowledge_candidate 等）仍定义在父模块 mod.rs，
//! 本模块通过 `use super::*` 继承访问。

use super::*;
use crate::agent::session_ctx::PlanStep;
/// save_memory：把模型提取的工程经验写入项目记忆（enabled=1，注入后续对话）
pub(super) async fn save_memory(args: &Value, project_id: &str, db: &crate::db::DbState) -> Result<String, String> {
    if project_id.is_empty() {
        return Err("当前会话未绑定项目目录，无法保存记忆".into());
    }
    let title = args["title"].as_str().unwrap_or("").trim().to_string();
    let content = args["content"].as_str().unwrap_or("").trim().to_string();
    if title.is_empty() || content.is_empty() {
        return Err("save_memory 需要参数 {\"title\":\"<标题>\",\"content\":\"<经验>\",\"category\":\"<可选分类>\"}".into());
    }
    if title.chars().count() > 60 {
        return Err(format!("title 过长（{} 字符），请精简到 60 字符内", title.chars().count()));
    }
    if content.chars().count() > 2000 {
        return Err(format!("content 过长（{} 字符），请精简到 2000 字符内", content.chars().count()));
    }
    let raw_cat = args["category"].as_str().unwrap_or("general").trim().to_string();
    let category = if matches!(raw_cat.as_str(), "general" | "architecture" | "build_command" | "module_role" | "user_preference" | "code" | "build" | "deploy" | "decision" | "pitfall" | "path") {
        raw_cat
    } else {
        "general".to_string()
    };
    let now = chrono::Utc::now().timestamp();
    let m = crate::db::models::ProjectMemory {
        id: uuid::Uuid::new_v4().to_string(),
        project_id: project_id.to_string(),
        category,
        title,
        content,
        enabled: true,
        source_kind: "agent_tool".into(),
        source_ref: "tool:save_memory".into(),
        scope: "project".into(),
        confidence: args["confidence"].as_f64().unwrap_or(0.9).clamp(0.0, 1.0),
        version: 1,
        confirmed: args["confirmed"].as_bool().unwrap_or(true),
        pinned: args["pinned"].as_bool().unwrap_or(false),
        invalidation_condition: args["invalidation_condition"].as_str().unwrap_or("").trim().to_string(),
        invalidated_at: None,
        invalidation_reason: None,
        created_at: now,
        updated_at: now,
    };
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::db::queries::insert_memory(&conn, &m).map_err(|e| e.to_string())?;
    Ok(format!("已保存项目记忆「{}」（分类 {}），后续对话会自动参考该经验", m.title, m.category))
}

/// 字符级相似度（0~1）：公共字符集合占比，用于事实去重粗筛。
fn char_overlap(a: &str, b: &str) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let ca: Vec<char> = a.chars().collect();
    let cb_orig = b.chars().count();
    let mut cb: Vec<char> = b.chars().collect();
    let mut common = 0usize;
    for ch in &ca {
        if let Some(i) = cb.iter().position(|c| c == ch) {
            cb.remove(i);
            common += 1;
        }
    }
    common as f64 / ca.len().max(cb_orig) as f64
}

/// fact_extract：自动事实抽取与沉淀。模型在任务收尾时把值得长期记住的事实
/// （架构约定/技术决策/踩坑结论/构建命令等）交给本工具：自动生成标题、与已有记忆
/// 去重（相似度高的不重复入库，防止知识库膨胀），确认唯一后才写入 project_memories。
/// 参数：{"fact":"<事实/经验文本>"（必填），"category":"<可选 general|code|build|deploy|decision|pitfall>",
///  "title":"<可选，缺省从 fact 自动截取>","dedupe":<可选，缺省 true>}。
pub(super) async fn fact_extract(args: &Value, project_id: &str, db: &crate::db::DbState) -> Result<String, String> {
    if project_id.is_empty() {
        return Err("当前会话未绑定项目目录，无法沉淀事实".into());
    }
    let fact = args["fact"].as_str().unwrap_or("").trim().to_string();
    if fact.is_empty() {
        return Err("fact_extract 需要参数 {\"fact\":\"<要沉淀的事实/经验文本>\"}".into());
    }
    if fact.chars().count() > 2000 {
        return Err(format!("fact 过长（{} 字符），请精简到 2000 字符内", fact.chars().count()));
    }
    // 1) 标题：显式参数 > 自动截取（去首尾空白与常见口头前缀）
    let mut title = args["title"].as_str().unwrap_or("").trim().to_string();
    if title.is_empty() {
        let t = fact.trim();
        let t = t.strip_prefix("我们").unwrap_or(t);
        let t = t.strip_prefix('我').unwrap_or(t);
        title = t.trim().chars().take(30).collect::<String>();
        if title.chars().count() == 30 {
            title.push('…');
        }
    }
    if title.chars().count() > 60 {
        return Err(format!("title 过长（{} 字符），请精简到 60 字符内", title.chars().count()));
    }
    let raw_cat = args["category"].as_str().unwrap_or("general").trim().to_string();
    let category = if matches!(raw_cat.as_str(), "general" | "architecture" | "build_command" | "module_role" | "user_preference" | "code" | "build" | "deploy" | "decision" | "pitfall") {
        raw_cat
    } else {
        "general".to_string()
    };
    // 2) 去重：与该项目已有记忆比对（标题/正文重合度），高相似返回"已存在"不重复入库
    let dedupe = args["dedupe"].as_bool().unwrap_or(true);
    if dedupe {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let existing = crate::db::queries::list_memories(&conn, project_id).map_err(|e| e.to_string())?;
        drop(conn);
        let needle = format!("{title} {fact}");
        let mut best: Option<(String, f64)> = None;
        for m in &existing {
            let hay = format!("{} {}", m.title, m.content);
            let sim = char_overlap(&needle, &hay);
            if best.as_ref().map(|(_, s)| sim > *s).unwrap_or(true) {
                best = Some((m.title.clone(), sim));
            }
        }
        if let Some((t, sim)) = best {
            if sim > 0.7 {
                return Ok(format!(
                    "事实与已有记忆「{}」高度重复（相似度 {:.0}%），未重复入库。\n已有内容开头：{}…\n如需强制保存请传 dedupe=false",
                    t,
                    sim * 100.0,
                    existing
                        .iter()
                        .find(|m| m.title == t)
                        .map(|m| m.content.chars().take(60).collect::<String>())
                        .unwrap_or_default()
                ));
            }
        }
    }
    // 3) 入库（与 save_memory 同表同结构）
    let now = chrono::Utc::now().timestamp();
    let m = crate::db::models::ProjectMemory {
        id: uuid::Uuid::new_v4().to_string(),
        project_id: project_id.to_string(),
        category,
        title,
        content: fact,
        enabled: true,
        source_kind: "agent_extraction".into(),
        source_ref: "tool:fact_extract".into(),
        scope: "project".into(),
        confidence: args["confidence"].as_f64().unwrap_or(0.8).clamp(0.0, 1.0),
        version: 1,
        confirmed: args["confirmed"].as_bool().unwrap_or(true),
        pinned: args["pinned"].as_bool().unwrap_or(false),
        invalidation_condition: args["invalidation_condition"].as_str().unwrap_or("").trim().to_string(),
        invalidated_at: None,
        invalidation_reason: None,
        created_at: now,
        updated_at: now,
    };
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::db::queries::insert_memory(&conn, &m).map_err(|e| e.to_string())?;
    Ok(format!(
        "事实已沉淀为项目记忆「{}」（分类 {}，已去重确认唯一），后续对话会自动参考。\n内容：{}…",
        m.title,
        m.category,
        m.content.chars().take(80).collect::<String>()
    ))
}

/// search_knowledge：主动查询知识库（团队经验/已知问题的解决方案），按命中次数排序。
pub(super) async fn search_knowledge(args: &Value, project_id: &str, db: &crate::db::DbState) -> Result<String, String> {
    let keyword = args["keyword"].as_str().unwrap_or("").trim();
    if keyword.is_empty() {
        return Err("search_knowledge 需要参数 {\"keyword\":\"<搜索关键词>\"}".into());
    }
    let limit = args["limit"].as_u64().unwrap_or(5).clamp(1, 20) as usize;
    let rows = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        crate::db::queries::search_knowledge(
            &conn,
            if project_id.is_empty() { None } else { Some(project_id) },
            keyword,
            limit,
        )
        .map_err(|e| format!("查询知识库失败：{e}"))?
    };
    let ecosystem = crate::services::harmony_ecosystem_knowledge::search(
        &crate::services::harmony_ecosystem_knowledge::KnowledgeQuery {
            keyword,
            api_level: args["api_level"].as_u64().and_then(|value| u32::try_from(value).ok()),
            device_type: args["device_type"].as_str().map(str::trim).filter(|value| !value.is_empty()),
            error_code: args["error_code"].as_str().map(str::trim).filter(|value| !value.is_empty()),
        },
        limit,
    );
    if rows.is_empty() && ecosystem.is_empty() {
        return Ok(format!(
            "知识库中没有匹配「{keyword}」的条目。\n若刚解决了相关问题，可调用 save_memory 把经验记入知识库（分类 build/deploy/code/pitfall 等），下次同类问题即可命中。"
        ));
    }
    let mut out = format!(
        "知识库命中团队经验 {} 条、生态证据 {} 条（关键词「{keyword}」）：\n",
        rows.len(), ecosystem.len()
    );
    for (i, e) in rows.iter().enumerate() {
        out.push_str(&format!(
            "\n[{}] {}\n  关键词: {}\n  问题: {}\n  解决: {}\n  命中次数: {} | 作用域: {}\n",
            i + 1,
            e.title,
            if e.keywords.is_empty() { "（无）".to_string() } else { e.keywords.clone() },
            super::cmd_tools::cut_str(&e.cause, 160),
            super::cmd_tools::cut_str(&e.fix, 260),
            e.hit_count,
            if e.project_id.is_some() { "本项目" } else { "全局" }
        ));
    }
    out.push_str(&crate::services::harmony_ecosystem_knowledge::render(&ecosystem));
    Ok(out)
}

/// manage_memory：管理项目记忆——查看/启用/禁用/删除（save_memory 写入的经验）。
pub(super) async fn manage_memory(args: &Value, project_id: &str, db: &crate::db::DbState) -> Result<String, String> {
    let action = args["action"].as_str().unwrap_or("list").trim();
    if !matches!(action, "list" | "enable" | "disable" | "delete") {
        return Err("action 仅支持 list|enable|disable|delete".into());
    }
    if project_id.is_empty() {
        return Err("当前会话未绑定项目目录，无法管理项目记忆".into());
    }
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    if action == "list" {
        let limit = args["limit"].as_u64().unwrap_or(20).clamp(1, 50) as usize;
        let rows = crate::db::queries::list_memories(&conn, project_id)
            .map_err(|e| format!("读取记忆失败：{e}"))?;
        if rows.is_empty() {
            return Ok("项目记忆库为空。\n可调用 save_memory 沉淀本次任务的经验（分类 code/build/deploy/decision/pitfall），后续对话会自动参考。".into());
        }
        let mut out = format!("项目记忆共 {} 条（按更新时间倒序，显示前 {limit} 条）：\n", rows.len());
        for m in rows.iter().take(limit) {
            out.push_str(&format!(
                "\n[{}] {} ｜ 分类：{} ｜ {}\n  {}\n",
                m.id,
                m.title,
                m.category,
                if m.enabled { "启用" } else { "已禁用" },
                super::cmd_tools::cut_str(&m.content, 160)
            ));
        }
        out.push_str("\n管理：manage_memory action=enable|disable|delete id=<记忆 id>（删除前请先确认该条已无价值）。");
        return Ok(out);
    }
    let id = args["id"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());
    let Some(id) = id else {
        return Err(format!("manage_memory {action} 需要 id（先用 action=list 查看记忆 id）"));
    };
    match action {
        "enable" => {
            crate::db::queries::set_memory_enabled(&conn, id, true).map_err(|e| e.to_string())?;
            Ok(format!("记忆 {id} 已启用，后续对话会重新参考该经验。"))
        }
        "disable" => {
            crate::db::queries::set_memory_enabled(&conn, id, false).map_err(|e| e.to_string())?;
            Ok(format!("记忆 {id} 已禁用（不再注入对话，记录保留）。"))
        }
        "delete" => {
            crate::db::queries::delete_memory(&conn, id).map_err(|e| e.to_string())?;
            Ok(format!("记忆 {id} 已删除。"))
        }
        _ => unreachable!(),
    }
}

/// manage_knowledge：管理知识库条目——查看（按命中排序）/删除。
pub(super) async fn manage_knowledge(args: &Value, project_id: &str, db: &crate::db::DbState) -> Result<String, String> {
    let action = args["action"].as_str().unwrap_or("list").trim();
    if !matches!(action, "list" | "delete") {
        return Err("action 仅支持 list|delete".into());
    }
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    if action == "list" {
        let limit = args["limit"].as_u64().unwrap_or(20).clamp(1, 50) as usize;
        let rows = crate::db::queries::list_knowledge(
            &conn,
            if project_id.is_empty() { None } else { Some(project_id) },
        )
        .map_err(|e| format!("读取知识库失败：{e}"))?;
        if rows.is_empty() {
            return Ok("知识库为空。\n经验会自动在构建/部署失败时沉淀（也可调用 save_memory 主动记录）。".into());
        }
        let mut out = format!("知识库条目共 {} 条（按命中次数排序，显示前 {limit} 条）：\n", rows.len());
        for e in rows.iter().take(limit) {
            out.push_str(&format!(
                "\n[{}] {} ｜ 命中 {} 次 ｜ {}\n  关键词: {}\n  问题: {}\n  解决: {}\n",
                e.id,
                e.title,
                e.hit_count,
                if e.project_id.is_some() { "本项目" } else { "全局" },
                if e.keywords.is_empty() { "（无）".to_string() } else { e.keywords.clone() },
                super::cmd_tools::cut_str(&e.cause, 120),
                super::cmd_tools::cut_str(&e.fix, 180)
            ));
        }
        out.push_str("\n管理：manage_knowledge action=delete id=<条目 id>（如旧解法已失效）。");
        return Ok(out);
    }
    let id = args["id"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());
    let Some(id) = id else {
        return Err("manage_knowledge delete 需要 id（先用 action=list 查看条目 id）".into());
    };
    crate::db::queries::delete_knowledge(&conn, id).map_err(|e| format!("删除知识条目失败：{e}"))?;
    Ok(format!("知识条目 {id} 已删除。"))
}

/// list_mcp_servers：列出项目可用的 MCP 服务器与工具清单、连接健康状态。
pub(super) async fn list_mcp_servers(
    args: &Value,
    project_id: &str,
    db: &crate::db::DbState,
    mcp: &crate::services::mcp_manager::McpManager,
) -> Result<String, String> {
    let detail = args["detail"].as_bool().unwrap_or(false);
    // 锁仅用于读取服务器列表，作用域化保证在下方 await（collect_tools）前释放
    let servers = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        crate::db::queries::list_mcp_servers(
            &conn,
            if project_id.is_empty() { None } else { Some(project_id) },
        )
        .map_err(|e| format!("读取 MCP 服务器列表失败：{e}"))?
    };
    if servers.is_empty() {
        return Ok("当前没有配置任何 MCP 服务器（可在 MCP 页面添加；添加后可通过 mcp__服务器名__工具名 调用其工具）。".into());
    }
    let mut out = format!("MCP 服务器（{} 个）：\n", servers.len());
    if detail {
        // 逐个连接并拉工具清单（失败单独标注，不影响其他服务器）
        let collected = mcp.collect_tools(&servers).await;
        for (i, server) in servers.iter().enumerate() {
            let status = if server.enabled { "启用" } else { "停用" };
            let (name, tools, conn_res) = &collected[i];
            out.push_str(&format!("\n[{status}] {name}\n"));
            if let Some(d) = server.description.as_ref().filter(|d| !d.trim().is_empty()) {
                out.push_str(&format!("  描述: {}\n", super::cmd_tools::cut_str(d, 200)));
            }
            match conn_res {
                Ok(()) => out.push_str(&format!(
                    "  状态: ✓ 连接成功，{} 个工具：\n",
                    tools.len()
                )),
                Err(e) => out.push_str(&format!(
                    "  状态: ✗ 连接失败：{}\n",
                    super::cmd_tools::cut_str(e, 300)
                )),
            }
            for t in tools {
                out.push_str(&format!(
                    "    - mcp__{name}__{}\n",
                    t.name
                ));
            }
        }
        out.push_str("\n提示：调用格式为【TOOL|mcp__服务器名__工具名|JSON参数】；连接失败的服务器可检查配置/重启后重试。");
    } else {
        // 只列元数据与最近测试状态，不产生连接副作用
        for server in &servers {
            let last = match (server.last_test_ok, server.last_test_error.as_deref()) {
                (Some(true), _) => "最近测试：✓ 通过".to_string(),
                (Some(false), Some(e)) => format!("最近测试：✗ {}", super::cmd_tools::cut_str(e, 120)),
                (Some(false), None) => "最近测试：✗ 失败".to_string(),
                _ => "尚未测试连接".to_string(),
            };
            out.push_str(&format!(
                "\n[{}] {} ｜ {} ｜ {}（{}\n",
                if server.enabled { "启用" } else { "停用" },
                server.name,
                server.server_type,
                last,
                if server.project_id.is_some() { "仅本项目" } else { "全局" },
            ));
            if let Some(d) = server.description.as_ref().filter(|d| !d.trim().is_empty()) {
                out.push_str(&format!("  描述: {}\n", super::cmd_tools::cut_str(d, 200)));
            }
        }
        out.push_str("\n查看每台服务器的工具清单与实时连接状态：list_mcp_servers detail=true（会发起连接探测）。");
    }
    Ok(out)
}

// ---------- 任务规划（plan_task / update_progress，会话级内存状态） ----------

/// 会话级任务计划表（统一收敛到 SessionContext，锁由进程级单例持有）
pub(super) fn task_plans() -> std::sync::MutexGuard<'static, crate::agent::session_ctx::SessionContext> {
    crate::agent::session_ctx::sessions()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// 渲染计划清单（编号/状态/标题/备注）。
pub(super) fn render_plan(title: &str, steps: &[PlanStep]) -> String {
    let done = steps.iter().filter(|s| s.status == "done").count();
    let failed = steps.iter().filter(|s| s.status == "failed").count();
    let mut s = format!(
        "📋 任务计划：{title}\n进度：{done}/{} 完成（失败 {failed}）\n",
        steps.len()
    );
    for (i, step) in steps.iter().enumerate() {
        let mark = match step.status.as_str() {
            "done" => "✓",
            "failed" => "✗",
            "doing" => "▶",
            _ => "·",
        };
        s.push_str(&format!("  {}. {} {}\n", i + 1, mark, step.title));
        if !step.note.is_empty() {
            s.push_str(&format!("     备注: {}\n", super::cmd_tools::cut_str(&step.note, 200)));
        }
    }
    s
}

/// 把计划镜像到 todo_store（前端任务清单实时渲染），并推送 agent:todo 事件。
/// plan map 是状态事实源；todo_store 是渲染镜像（todo 只有 pending/in_progress/done 三态，
/// failed 以 pending 展示并在内容中标注）。
pub(super) fn mirror_plan_to_todo(app: Option<&tauri::AppHandle>, conversation_id: &str, title: &str, steps: &[PlanStep]) {
    let mut items = Vec::with_capacity(steps.len() + 1);
    items.push(crate::agent::todo::TodoItem {
        id: "plan_title".into(),
        content: format!("📋 {title}"),
        status: "pending".into(),
    });
    for (i, s) in steps.iter().enumerate() {
        let status = match s.status.as_str() {
            "done" => "done",
            "doing" => "in_progress",
            _ => "pending",
        };
        let mut content = s.title.clone();
        if s.status == "failed" {
            content.push_str("（✗ 失败）");
        } else if !s.note.is_empty() {
            content.push_str(&format!("（{}）", s.note));
        }
        items.push(crate::agent::todo::TodoItem {
            id: format!("step-{}", i + 1),
            content,
            status: status.into(),
        });
    }
    let _ = crate::agent::todo::replace(conversation_id, items.clone());
    if let Some(app) = app {
        use tauri::Emitter;
        let _ = app.emit(
            "agent:todo",
            crate::agent::todo::TodoEvent {
                conversation_id: conversation_id.to_string(),
                todos: items,
            },
        );
    }
}

/// plan_task：创建/查看/清空当前会话的任务计划（镜像到前端任务清单实时展示进度）。
pub(super) async fn plan_task(args: &Value, ctx: &crate::agent::exec_ctx::ToolCtx) -> Result<String, String> {
    let conversation_id = &ctx.conversation_id;
    let action = args["action"].as_str().unwrap_or("create").trim();
    let mut plans = task_plans();
    match action {
        "show" => match plans.task_plans.get(conversation_id) {
            Some((title, steps)) => Ok(render_plan(title, steps)),
            None => Err("当前会话还没有任务计划，可用 plan_task action=create 创建（steps 传步骤数组）".into()),
        },
        "clear" => {
            plans.task_plans.remove(conversation_id);
            // 同步清空前端任务清单
            let _ = crate::agent::todo::replace(conversation_id, Vec::new());
            if let Some(app) = &ctx.app {
                use tauri::Emitter;
                let _ = app.emit(
                    "agent:todo",
                    crate::agent::todo::TodoEvent {
                        conversation_id: conversation_id.clone(),
                        todos: Vec::new(),
                    },
                );
            }
            Ok("已清空当前会话的任务计划。".into())
        }
        "create" => {
            let title = args["title"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty()).unwrap_or("未命名任务");
            let steps: Vec<String> = args["steps"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            if steps.is_empty() {
                return Err("plan_task create 需要 steps（步骤数组，每项为字符串描述）".into());
            }
            let list: Vec<PlanStep> = steps
                .into_iter()
                .map(|t| PlanStep { title: t, status: "todo".into(), note: String::new() })
                .collect();
            plans.task_plans.insert(conversation_id.to_string(), (title.to_string(), list));
            let (t, st) = plans.task_plans.get(conversation_id).unwrap();
            mirror_plan_to_todo(ctx.app.as_ref(), conversation_id, t, st);
            Ok(format!(
                "任务计划已创建（会话级，跨轮对话保留；重启后清空，前端任务清单同步展示进度）。\n{}后续每完成一步调用 update_progress step=<编号> 更新状态。",
                render_plan(t, st)
            ))
        }
        _ => Err(format!("action 仅支持 create|show|clear，收到 {action}")),
    }
}

/// update_progress：更新计划中某一步的状态（同步镜像到前端任务清单）。
pub(super) async fn update_progress(args: &Value, ctx: &crate::agent::exec_ctx::ToolCtx) -> Result<String, String> {
    let conversation_id = &ctx.conversation_id;
    let step = args["step"].as_u64().unwrap_or(0);
    if step < 1 {
        return Err("update_progress 需要 step（步骤编号，从 1 开始）".into());
    }
    let status = args["status"].as_str().unwrap_or("done").trim();
    if !matches!(status, "done" | "failed" | "doing") {
        return Err("status 仅支持 done|failed|doing".into());
    }
    let note = args["note"].as_str().unwrap_or("").trim().to_string();
    let mut plans = task_plans();
    let Some((_, steps)) = plans.task_plans.get_mut(conversation_id) else {
        return Err("当前会话还没有任务计划，先调用 plan_task action=create 创建".into());
    };
    let idx = (step - 1) as usize;
    if idx >= steps.len() {
        return Err(format!("步骤编号 {step} 超出计划范围（共 {} 步）", steps.len()));
    }
    let s = &mut steps[idx];
    s.status = status.to_string();
    if !note.is_empty() {
        s.note = note;
    }
    let (t, st) = plans.task_plans.get(conversation_id).unwrap();
    mirror_plan_to_todo(ctx.app.as_ref(), conversation_id, t, st);
    Ok(render_plan(t, st))
}

/// export_data：导出数据库完整备份快照（VACUUM INTO，不影响运行中的库）。
pub(super) async fn export_data(args: &Value, ctx: &crate::agent::exec_ctx::ToolCtx, db: &crate::db::DbState) -> Result<String, String> {
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dest = match args["dest"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(p) => {
            let dir = PathBuf::from(p);
            if !dir.is_dir() {
                return Err(format!("dest 目录不存在：{}", dir.display()));
            }
            dir
        }
        None => {
            let app = ctx
                .app
                .as_ref()
                .ok_or("无法确定默认备份目录（app 句柄不可用），请显式传 dest 目录")?;
            use tauri::Manager;
            let base = app.path().app_data_dir().map_err(|e| e.to_string())?;
            base.join("backups")
        }
    };
    std::fs::create_dir_all(&dest).map_err(|e| format!("创建备份目录失败：{e}"))?;
    let file = dest.join(format!("deveco-backup-{ts}.db"));
    if file.exists() {
        return Err(format!("备份文件已存在（同一秒内重复导出）：{}，请稍后重试", file.display()));
    }
    // VACUUM INTO 路径不能参数化，转义单引号防注入
    let path_escaped = file.to_string_lossy().replace('\'', "''");
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute_batch(&format!("VACUUM INTO '{path_escaped}';")).map_err(|e| {
        format!("导出备份失败：{e}（备份文件可能残留，可手动删除）")
    })?;
    drop(conn);
    let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    Ok(format!(
        "备份完成：{}\n大小：{} KB\n包含全部数据（会话/消息/记忆/日志/成本/配置）。\n恢复方式：用 SQLite 工具打开或替换应用数据库文件（先退出应用）；也可用于换机迁移。",
        file.display(),
        size / 1024
    ))
}

/// get_cost_summary：查看 AI 调用成本统计（今日/本月，按模型聚合）。
pub(super) async fn get_cost_summary(args: &Value, db: &crate::db::DbState) -> Result<String, String> {
    let range = args["range"].as_str().unwrap_or("today").trim();
    if !matches!(range, "today" | "month") {
        return Err("range 仅支持 today|month".into());
    }
    let now = chrono::Local::now();
    use chrono::Datelike;
    let today = now.format("%Y-%m-%d").to_string();
    let start_date = if range == "today" {
        today.clone()
    } else {
        now.format("%Y-%m-01").to_string()
    };
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let daily = crate::db::queries::get_daily_usage(&conn, &start_date, &today)
        .map_err(|e| format!("读取成本数据失败：{e}"))?;
    let mut requests = 0i64;
    let mut input = 0i64;
    let mut output = 0i64;
    let mut cost = 0.0f64;
    for d in &daily {
        requests += d.request_count;
        input += d.input_tokens;
        output += d.output_tokens;
        cost += d.total_cost_cny;
    }
    // 按模型聚合（request_logs 明细）：范围起始时间戳 → 现在
    let range_start_ts = if range == "today" {
        now.date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp()
    } else {
        chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp()
    };
    let by_model = crate::db::queries::get_cost_by_model(&conn, range_start_ts, now.timestamp())
        .map_err(|e| format!("读取模型成本明细失败：{e}"))?;
    let label = if range == "today" { "今日" } else { "本月" };
    let mut out = format!(
        "{label} AI 成本统计（{start_date} ~ {today}）：\n请求 {} 次 ｜ 输入 {input} tokens ｜ 输出 {output} tokens ｜ 费用 ¥{:.4}\n",
        requests, cost
    );
    if by_model.is_empty() {
        out.push_str("（暂无明细记录）");
    } else {
        out.push_str("\n按模型：\n");
        for m in by_model.iter().take(8) {
            out.push_str(&format!(
                "- {}：{} 次 ｜ 输入 {} ｜ 输出 {} ｜ ¥{:.4}\n",
                m.model, m.request_count, m.input_tokens, m.output_tokens, m.total_cost_cny
            ));
        }
        if by_model.len() > 8 {
            out.push_str(&format!("（另有 {} 个模型未列出）\n", by_model.len() - 8));
        }
    }
    Ok(out)
}

/// [38] conversation_search：全局历史对话语义搜索（LIKE 关键词方案，无需向量库）。
/// 跨会话检索 user/assistant 消息，输出命中片段 + 会话标题 + 时间，供回忆历史决策/排查记录。
/// 参数：{"query":"<关键词>","project":"<可选项目 id，缺省全部项目>","role":"user|assistant|all（可选缺省 all）","limit":<可选 1-20 缺省 8>}。
pub(super) async fn conversation_search(
    args: &Value,
    project_id: &str,
    db: &crate::db::DbState,
) -> Result<String, String> {
    let query = args["query"].as_str().map(str::trim).filter(|s| !s.is_empty());
    let Some(query) = query else {
        return Err("conversation_search 需要参数 {\"query\":\"<关键词>\"}".into());
    };
    if query.len() > 100 {
        return Err("关键词过长（≤100 字符）".into());
    }
    let limit = args["limit"].as_u64().unwrap_or(8).clamp(1, 20) as usize;
    let role = args["role"].as_str().unwrap_or("all");
    if !matches!(role, "all" | "user" | "assistant") {
        return Err("role 只接受 all|user|assistant".into());
    }
    let proj_filter = args["project"].as_str().map(|p| p.to_string()).unwrap_or_else(|| project_id.to_string());
    // LIKE 特殊字符转义（% _ \ → \% \_ \\）
    let escaped = query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    let pat = format!("%{escaped}%");
    let role_filter = if role == "all" {
        "AND (?4 IS NULL OR m.role = ?4)"
    } else {
        "AND m.role = ?4"
    };
    let sql = format!(
        "SELECT m.role, m.content, m.created_at, c.title, c.id
         FROM messages m
         JOIN conversations c ON c.id = m.conversation_id
         WHERE m.content LIKE ?1 ESCAPE '\\' {role_filter}
         AND c.project_id = ?2
         ORDER BY m.created_at DESC
         LIMIT ?3"
    );
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let role_param: Option<&str> = if role == "all" { None } else { Some(role) };
    // 参数数量恒定 4 个（role=all 时 ?4 传 NULL），避免 if/else 两分支闭包类型不一致
    let rows = stmt
        .query_map(
            rusqlite::params![pat, proj_filter, limit as i64, role_param],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .map_err(|e| e.to_string())?;
    let hits: Vec<(String, String, i64, String, String)> =
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    if hits.is_empty() {
        return Ok(format!(
            "未找到包含 \"{query}\" 的历史消息（搜索范围：当前项目{}）。\n可尝试更短关键词，或加 role 参数限定 user/assistant。",
            if proj_filter == project_id && project_id.is_empty() { "（全部项目）".to_string() } else { String::new() }
        ));
    }
    let mut out = format!("历史对话搜索 \"{query}\"：{} 条命中\n", hits.len());
    for (i, (role, content, ts, title, conv_id)) in hits.iter().enumerate() {
        let time = chrono::DateTime::from_timestamp(*ts, 0)
            .map(|d| d.format("%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "-".into());
        let role_cn = match role.as_str() {
            "user" => "用户",
            "assistant" => "助手",
            _ => "系统",
        };
        // 命中片段：截取关键词前后各 60 字符（按字符安全截取，避免中文 UTF-8 边界切坏）
        let content_flat = content.replace(['\n', '\r'], " ");
        let snippet = if let Some(pos) = content_flat.to_lowercase().find(&query.to_lowercase()) {
            let start = pos.saturating_sub(60);
            let end = (pos + query.len() + 120).min(content_flat.len());
            let s: String = content_flat
                .chars()
                .skip(start)
                .take(end - start)
                .collect();
            format!("…{}…", s.trim())
        } else {
            content_flat.chars().take(160).collect::<String>()
        };
        out.push_str(&format!(
            "\n{}. [{}] {}（{}）会话：{}（{conv_id}）\n  {}\n",
            i + 1,
            role_cn,
            time,
            title,
            "查看详情可打开该会话",
            snippet
        ));
    }
    Ok(out)
}
