//! [68] 组合工具层 + [37] 冒烟测试链
//!
//! `compose`：把多个工具串成一条链顺序执行（预置链 build_and_deploy / smoke / test_and_report，
//! 或自定义 steps），每步输出结果，支持 stop_on_error 中止。
//! `smoke_test`：部署后自动冒烟链（build → deploy → run_ui_flow 断言 → 截图），输出冒烟报告。
//!
//! 设计取舍：步骤间不传递数据（保持简单可预测），需要串联数据的场景（如取部署输出再截图）
//! 仍由 Agent 分步调用；组合层解决的是「固定套路一次跑完」的常见需求。

use super::*;

#[derive(Clone)]
struct ChainStep {
    tool: String,
    args: Value,
    fallback: Option<(String, Value)>,
    compensation: Option<(String, Value)>,
}

impl ChainStep {
    fn new(tool: &str) -> Self {
        Self { tool: tool.into(), args: Value::Null, fallback: None, compensation: None }
    }
}

/// 预置组合链：name → 步骤列表（tool, 默认参数, fallback）
fn preset_chain(name: &str) -> Result<Vec<ChainStep>, String> {
    match name {
        "build_and_deploy" => Ok(vec![ChainStep::new("build_project"), ChainStep::new("deploy")]),
        "smoke" => Ok(vec![
            ChainStep::new("deploy"),
            // run_ui_flow 的 steps 必须由调用方提供（通过链级参数合并）
            ChainStep::new("run_ui_flow"),
        ]),
        "test_and_report" => Ok(vec![
            ChainStep::new("run_tests"),
            // export_report 的 out/title 由调用方提供
            ChainStep::new("export_report"),
        ]),
        _ => Err(format!(
            "未知预置链 \"{name}\"。可用：build_and_deploy / smoke / test_and_report；或改用 steps 参数自定义"
        )),
    }
}

/// 链级参数与步骤级参数合并（步骤级优先；null 视为未提供）
fn merge_args(chain: &Value, step: &Value) -> Value {
    if step.is_null() {
        return chain.clone();
    }
    if chain.is_null() {
        return step.clone();
    }
    if let (Some(a), Some(b)) = (chain.as_object(), step.as_object()) {
        let mut m = a.clone();
        for (k, v) in b {
            m.insert(k.clone(), v.clone());
        }
        return Value::Object(m);
    }
    step.clone()
}

fn chain_parameters(args: &Value) -> Value {
    let Some(object) = args.as_object() else { return Value::Null };
    let preset = args.get("chain").and_then(Value::as_str).is_some();
    Value::Object(object.iter().filter(|(key, _)| {
        !matches!(key.as_str(),
            "chain" | "stop_on_error" | "transaction" | "rollback_on_error")
            && (preset || key.as_str() != "steps")
    }).map(|(key, value)| (key.clone(), value.clone())).collect())
}

fn route_chain_parameters(tool: &str, chain: &Value) -> Value {
    let declared = super::protocol::declared_parameter_names(tool);
    let Some(object) = chain.as_object() else { return Value::Null };
    Value::Object(object.iter()
        .filter(|(key, _)| declared.contains(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect())
}

/// 输出截断（按字符，尾部附提示）
fn truncate_out(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}\n…（输出截断，完整内容见各步骤）")
    }
}

/// [64] fallback 链：主工具失败且配置了 fallback 时自动执行备选工具（组合宏的"减震"层）。
/// fallback 定义：{"tool":"<备选工具名>","args":<可选备选参数>}。
/// 主调用成功 → 直接返回；主调用失败 → 执行 fallback，把失败原因与备选结果合并返回。
/// 适用场景：主工具受环境/前置条件影响可能失败（如 manage_hdc 失败 → list_devices 检查设备状态；
/// build_project 失败 → get_diagnostics 归因），给模型可执行的下一步而不是硬错误。
pub(super) async fn run_tool_with_fallback(
    tool: &str,
    args: &Value,
    fallback: Option<(&str, &Value)>,
    project_path: &str,
    path_hints: &[String],
    project_id: &str,
    db: &crate::db::DbState,
    mcp: &crate::services::mcp_manager::McpManager,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let primary = super::run_tool_boxed(
        tool,
        &args.to_string(),
        project_path,
        path_hints,
        project_id,
        db,
        mcp,
        ctx,
    )
    .await;
    match primary {
        Ok(o) => Ok(o),
        Err(e) => {
            let Some((fb, fb_args)) = fallback else {
                return Err(e);
            };
            if fb == tool {
                return Err(format!("{e}\n（fallback 不能指向主工具自身）"));
            }
            let r = super::run_tool_boxed(
                fb,
                &fb_args.to_string(),
                project_path,
                path_hints,
                project_id,
                db,
                mcp,
                ctx,
            )
            .await;
            match r {
                Ok(fo) => Ok(format!(
                    "⚠ {tool} 执行失败，已自动切换 fallback={fb}：\n{tool} 错误：{}\n\n{fb} 结果：{}\n\n（如 fallback 已解决问题可直接继续；否则换方案）",
                    truncate_out(&e, 600),
                    truncate_out(&fo, 1200)
                )),
                Err(fe) => Err(format!(
                    "{tool} 失败且 fallback={fb} 也失败：\n{tool} 错误：{}\n{fb} 错误：{}",
                    truncate_out(&e, 400),
                    truncate_out(&fe, 400)
                )),
            }
        }
    }
}

/// compose：组合链执行。参数：
/// {"chain":"build_and_deploy|smoke|test_and_report"} 或 {"steps":[{"tool":"<工具名>","args":{...}},...],
///  "stop_on_error":<可选，缺省 true>}；链级参数（如 device）会合并进每一步。
pub(super) async fn compose(
    args: &Value,
    project_path: &str,
    path_hints: &[String],
    project_id: &str,
    db: &crate::db::DbState,
    mcp: &crate::services::mcp_manager::McpManager,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let steps = if let Some(cn) = args["chain"].as_str() {
        preset_chain(cn)?
    } else if let Some(arr) = args["steps"].as_array() {
        if arr.is_empty() {
            return Err("steps 不能为空".into());
        }
        let mut list: Vec<ChainStep> = Vec::with_capacity(arr.len());
        for s in arr {
            let tool = s["tool"].as_str().ok_or("steps 内每项需要 {\"tool\":\"<工具名>\",\"args\":{...}}")?;
            if matches!(tool, "compose" | "smoke_test") {
                return Err("compose 不允许嵌套 compose/smoke_test，避免递归事务与补偿边界失控".into());
            }
            let step_args = s.get("args").cloned().unwrap_or(Value::Null);
            // [64] 步骤级 fallback：{"fallback":"<工具名>","fallback_args":{...}}，主工具失败时自动执行
            let fb = match (s["fallback"].as_str(), s.get("fallback_args")) {
                (Some(f), fa) => Some((f.to_string(), fa.cloned().unwrap_or(Value::Null))),
                (None, _) => None,
            };
            let compensation = s.get("compensate").and_then(|value| value.as_object()).map(|value| {
                let tool = value.get("tool").and_then(|item| item.as_str())
                    .ok_or("compensate 需要 {\"tool\":\"<补偿工具>\",\"args\":{...}}")?;
                if matches!(tool, "compose" | "smoke_test") {
                    return Err("补偿动作不允许嵌套组合工具".to_string());
                }
                Ok((tool.to_string(), value.get("args").cloned().unwrap_or(Value::Null)))
            }).transpose()?;
            list.push(ChainStep { tool: tool.to_string(), args: step_args, fallback: fb, compensation });
        }
        list
    } else {
        return Err("compose 需要 chain（预置链）或 steps（自定义步骤）参数".into());
    };
    let stop_on_error = args["stop_on_error"].as_bool().unwrap_or(true);
    let transaction = args["transaction"].as_bool().unwrap_or(steps.len() > 1);
    let rollback_on_error = args["rollback_on_error"].as_bool().unwrap_or(transaction);
    let transaction_id = uuid::Uuid::new_v4().to_string();
    let chain_args = chain_parameters(args);
    let chain_label = args["chain"]
        .as_str()
        .map(|c| format!("（{c}）"))
        .unwrap_or_default();
    let mut out = format!(
        "组合链执行{chain_label}，共 {} 步；事务={}，失败补偿={}，transaction_id={}：\n",
        steps.len(), transaction, rollback_on_error, transaction_id,
    );
    let mut failed = 0usize;
    let mut completed: Vec<String> = Vec::new();
    let mut compensation_stack: Vec<(String, Value, String)> = Vec::new();
    record_transaction_event(db, ctx, "compose.transaction_started", serde_json::json!({
        "transaction_id": transaction_id,
        "steps": steps.iter().map(|step| step.tool.as_str()).collect::<Vec<_>>(),
        "transaction": transaction,
        "rollback_on_error": rollback_on_error,
    }));
    for (i, step) in steps.iter().enumerate() {
        let tool = &step.tool;
        let merged = merge_args(&route_chain_parameters(tool, &chain_args), &step.args);
        // fb 为链上声明的 fallback：先合并链级参数再借用（match 模式自动解引用，闭包参数不会）
        let fb_owned: Option<(String, Value)> = match &step.fallback {
            Some((f, fa)) => Some((
                f.clone(),
                merge_args(&route_chain_parameters(f, &chain_args), fa),
            )),
            None => None,
        };
        let fb_merged = fb_owned.as_ref().map(|(f, fa)| (f.as_str(), fa));
        let r = run_tool_with_fallback(
            tool,
            &merged,
            fb_merged,
            project_path,
            path_hints,
            project_id,
            db,
            mcp,
            ctx,
        )
        .await;
        match r {
            Ok(o) => {
                out.push_str(&format!("\n[{}/{}] {tool} ✅\n", i + 1, steps.len()));
                out.push_str(&truncate_out(&o, 800));
                completed.push(tool.clone());
                if let Some((compensation_tool, compensation_args)) = &step.compensation {
                    compensation_stack.push((
                        compensation_tool.clone(),
                        merge_args(
                            &route_chain_parameters(compensation_tool, &chain_args),
                            compensation_args,
                        ),
                        tool.clone(),
                    ));
                }
                persist_transaction_checkpoint(
                    db, ctx, &transaction_id, i + 1, &completed, &compensation_stack,
                );
            }
            Err(e) => {
                failed += 1;
                out.push_str(&format!(
                    "\n[{}/{}] {tool} ❌ {}\n",
                    i + 1,
                    steps.len(),
                    truncate_out(&e, 400)
                ));
                if stop_on_error {
                    out.push_str("\n（stop_on_error=true，后续步骤已跳过）");
                    break;
                }
            }
        }
    }
    if failed == 0 {
        out.push_str(&format!("\n组合链全部完成（{} 步均成功）。", steps.len()));
        record_transaction_event(db, ctx, "compose.transaction_committed", serde_json::json!({
            "transaction_id": transaction_id,
            "completed": completed,
        }));
        Ok(out)
    } else {
        out.push_str(&format!("\n组合链结束：{} 步失败。", failed));
        let mut compensation_failed = 0usize;
        if rollback_on_error && !compensation_stack.is_empty() {
            out.push_str("\n开始按逆序执行显式补偿动作：");
            for (compensation_tool, compensation_args, original_tool) in compensation_stack.iter().rev() {
                let result = super::run_tool_boxed(
                    compensation_tool,
                    &compensation_args.to_string(),
                    project_path,
                    path_hints,
                    project_id,
                    db,
                    mcp,
                    ctx,
                ).await;
                match result {
                    Ok(value) => out.push_str(&format!(
                        "\n- {original_tool} → {compensation_tool}：已补偿\n{}",
                        truncate_out(&value, 500),
                    )),
                    Err(error) => {
                        compensation_failed += 1;
                        out.push_str(&format!(
                            "\n- {original_tool} → {compensation_tool}：补偿失败：{}",
                            truncate_out(&error, 400),
                        ));
                    }
                }
            }
        }
        let uncompensated = completed.iter().filter(|tool| {
            !compensation_stack.iter().any(|(_, _, original)| original == *tool)
                && super::contracts::contract(tool).effect != super::contracts::EffectKind::Read
        }).cloned().collect::<Vec<_>>();
        if !uncompensated.is_empty() {
            out.push_str(&format!(
                "\n以下已完成副作用步骤没有显式补偿，必须人工核验后处理：{}",
                uncompensated.join(", "),
            ));
        }
        record_transaction_event(db, ctx, "compose.transaction_rolled_back", serde_json::json!({
            "transaction_id": transaction_id,
            "completed": completed,
            "compensation_failed": compensation_failed,
            "uncompensated": uncompensated,
        }));
        Err(out)
    }
}

fn record_transaction_event(
    db: &crate::db::DbState,
    ctx: &crate::agent::exec_ctx::ToolCtx,
    event: &str,
    payload: Value,
) {
    if ctx.run_id.is_empty() { return; }
    if let Ok(conn) = db.0.lock() {
        let _ = crate::agent::runtime::append_event(
            &conn, &ctx.run_id, &ctx.conversation_id, event, payload,
        );
    }
}

fn persist_transaction_checkpoint(
    db: &crate::db::DbState,
    ctx: &crate::agent::exec_ctx::ToolCtx,
    transaction_id: &str,
    next_step: usize,
    completed: &[String],
    compensation_stack: &[(String, Value, String)],
) {
    if ctx.run_id.is_empty() { return; }
    if let Ok(conn) = db.0.lock() {
        let checkpoint = serde_json::json!({
            "kind": "compose_transaction",
            "transaction_id": transaction_id,
            "next_step": next_step,
            "completed": completed,
            "compensation_pending": compensation_stack.iter().map(|(tool, _, original)| {
                serde_json::json!({"tool": tool, "for": original})
            }).collect::<Vec<_>>(),
        });
        let _ = crate::agent::scheduler::checkpoint(&conn, &ctx.run_id, &checkpoint, 60_000);
        let _ = crate::agent::runtime::append_event(
            &conn, &ctx.run_id, &ctx.conversation_id, "compose.checkpoint", checkpoint,
        );
    }
}

/// smoke_test：部署后自动冒烟链。build（可选跳过）→ deploy → run_ui_flow 断言 → 截图。
/// 参数：{"device":"<可选>","hap":"<可选 HAP 路径>","bundle":"<可选，如需要>",
///        "steps":[<run_ui_flow 步骤>]（必填，冒烟断言），"verify":<可选，缺省 true 结束截图>,
///        "skip_build":<可选，缺省 false>}。
pub(super) async fn smoke_test(
    args: &Value,
    project_path: &str,
    path_hints: &[String],
    project_id: &str,
    db: &crate::db::DbState,
    mcp: &crate::services::mcp_manager::McpManager,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let steps = args["steps"].as_array().ok_or("smoke_test 需要参数 {\"steps\":[<UI 断言步骤>],...}")?;
    if steps.is_empty() {
        return Err("steps 不能为空（至少一条冒烟断言）".into());
    }
    let verify = args["verify"].as_bool().unwrap_or(true);
    let skip_build = args["skip_build"].as_bool().unwrap_or(false);
    // 组合链参数：device/hap 等透传给 deploy；steps/verify 给 run_ui_flow
    let deploy_args = {
        let mut m = serde_json::Map::new();
        if let Some(d) = args["device"].as_str() {
            m.insert("device".into(), Value::String(d.into()));
        }
        if let Some(h) = args["hap"].as_str() {
            m.insert("hap".into(), Value::String(h.into()));
        }
        Value::Object(m)
    };
    let flow_args = {
        let mut m = serde_json::Map::new();
        if let Some(d) = args["device"].as_str() {
            m.insert("device".into(), Value::String(d.into()));
        }
        m.insert("steps".into(), Value::Array(steps.clone()));
        m.insert("verify".into(), Value::Bool(verify));
        Value::Object(m)
    };

    let mut out = String::new();
    let mut failed = 0usize;

    // 1) 构建（可选）
    if !skip_build {
        out.push_str("[1/3] build_project …\n");
        match super::run_tool_boxed(
            "build_project",
            "{}",
            project_path,
            path_hints,
            project_id,
            db,
            mcp,
            ctx,
        )
        .await
        {
            Ok(o) => out.push_str(&truncate_out(&o, 500)),
            Err(e) => {
                out.push_str(&format!("构建失败：{e}\n（可用 skip_build=true 跳过构建直接部署已有产物）"));
                out.push_str("\n冒烟未通过：构建阶段失败。");
                return Ok(out);
            }
        }
    }

    // 2) 部署
    out.push_str(if skip_build { "\n[1/2] deploy …\n" } else { "\n[2/3] deploy …\n" });
    match super::run_tool_boxed(
        "deploy",
        &deploy_args.to_string(),
        project_path,
        path_hints,
        project_id,
        db,
        mcp,
        ctx,
    )
    .await
    {
        Ok(o) => out.push_str(&truncate_out(&o, 500)),
        Err(e) => {
            out.push_str(&format!("部署失败：{e}\n"));
            out.push_str("\n冒烟未通过：部署阶段失败。");
            return Ok(out);
        }
    }

    // 3) UI 断言流程
    out.push_str(if skip_build { "\n[2/2] run_ui_flow …\n" } else { "\n[3/3] run_ui_flow …\n" });
    match super::run_tool_boxed(
        "run_ui_flow",
        &flow_args.to_string(),
        project_path,
        path_hints,
        project_id,
        db,
        mcp,
        ctx,
    )
    .await
    {
        Ok(o) => out.push_str(&truncate_out(&o, 1500)),
        Err(e) => {
            failed += 1;
            out.push_str(&format!("UI 断言失败：{e}\n"));
        }
    }

    if failed == 0 {
        out.push_str("\n✅ 冒烟测试通过：部署成功且全部 UI 断言执行完成。");
    } else {
        out.push_str("\n❌ 冒烟测试未通过（见上方失败步骤）。");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn state() -> (crate::db::DbState, crate::services::mcp_manager::McpManager) {
        (
            crate::db::DbState(Arc::new(Mutex::new(rusqlite::Connection::open_in_memory().unwrap()))),
            crate::services::mcp_manager::McpManager::default(),
        )
    }

    #[test]
    fn chain_control_fields_are_not_forwarded_to_child_tools() {
        let value = serde_json::json!({
            "chain": "smoke",
            "stop_on_error": true,
            "transaction": true,
            "rollback_on_error": true,
            "device": "demo",
        });
        assert_eq!(chain_parameters(&value), serde_json::json!({"device":"demo"}));
    }

    #[test]
    fn preset_business_arguments_are_routed_only_to_declaring_steps() {
        let value = serde_json::json!({
            "chain": "smoke",
            "device": "demo",
            "steps": [{"action":"tap","x":1,"y":1}],
            "verify": true
        });
        let chain = chain_parameters(&value);
        assert_eq!(route_chain_parameters("deploy", &chain), serde_json::json!({"device":"demo"}));
        let flow = route_chain_parameters("run_ui_flow", &chain);
        assert!(flow.get("steps").is_some());
        assert_eq!(flow.get("device"), Some(&serde_json::json!("demo")));
    }

    #[tokio::test]
    async fn failed_transaction_runs_explicit_compensation_and_returns_error() {
        let (db, mcp) = state();
        let args = serde_json::json!({
            "steps": [
                {"tool":"tool_list","args":{},"compensate":{"tool":"tool_list","args":{}}},
                {"tool":"not_a_registered_tool","args":{}}
            ],
            "transaction": true,
            "rollback_on_error": true
        });
        let error = compose(
            &args, "", &[], "", &db, &mcp, &crate::agent::exec_ctx::ToolCtx::empty(),
        ).await.unwrap_err();
        assert!(error.contains("开始按逆序执行显式补偿动作"));
        assert!(error.contains("tool_list：已补偿"));
    }

    #[tokio::test]
    async fn fallback_is_a_degraded_success_and_commits_the_chain() {
        let (db, mcp) = state();
        let args = serde_json::json!({
            "steps": [{
                "tool":"not_a_registered_tool","args":{},
                "fallback":"tool_list","fallback_args":{}
            }],
            "transaction": true
        });
        let output = compose(
            &args, "", &[], "", &db, &mcp, &crate::agent::exec_ctx::ToolCtx::empty(),
        ).await.unwrap();
        assert!(output.contains("已自动切换 fallback=tool_list"));
        assert!(output.contains("组合链全部完成"));
    }
}
