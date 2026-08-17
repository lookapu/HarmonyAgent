//! [68] 组合工具层 + [37] 冒烟测试链
//!
//! `compose`：把多个工具串成一条链顺序执行（预置链 build_and_deploy / smoke / test_and_report，
//! 或自定义 steps），每步输出结果，支持 stop_on_error 中止。
//! `smoke_test`：部署后自动冒烟链（build → deploy → run_ui_flow 断言 → 截图），输出冒烟报告。
//!
//! 设计取舍：步骤间不传递数据（保持简单可预测），需要串联数据的场景（如取部署输出再截图）
//! 仍由 Agent 分步调用；组合层解决的是「固定套路一次跑完」的常见需求。

use super::*;

/// 预置组合链：name → 步骤列表（tool, 默认参数, fallback）
fn preset_chain(name: &str) -> Result<Vec<(String, Value, Option<(String, Value)>)>, String> {
    match name {
        "build_and_deploy" => Ok(vec![
            ("build_project".into(), Value::Null, None),
            ("deploy".into(), Value::Null, None),
        ]),
        "smoke" => Ok(vec![
            ("deploy".into(), Value::Null, None),
            // run_ui_flow 的 steps 必须由调用方提供（通过链级参数合并）
            ("run_ui_flow".into(), Value::Null, None),
        ]),
        "test_and_report" => Ok(vec![
            ("run_tests".into(), Value::Null, None),
            // export_report 的 out/title 由调用方提供
            ("export_report".into(), Value::Null, None),
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
        let mut list: Vec<(String, Value, Option<(String, Value)>)> = Vec::with_capacity(arr.len());
        for s in arr {
            let tool = s["tool"].as_str().ok_or("steps 内每项需要 {\"tool\":\"<工具名>\",\"args\":{...}}")?;
            let step_args = s.get("args").cloned().unwrap_or(Value::Null);
            // [64] 步骤级 fallback：{"fallback":"<工具名>","fallback_args":{...}}，主工具失败时自动执行
            let fb = match (s["fallback"].as_str(), s.get("fallback_args")) {
                (Some(f), fa) => Some((f.to_string(), fa.cloned().unwrap_or(Value::Null))),
                (None, _) => None,
            };
            list.push((tool.to_string(), step_args, fb));
        }
        list
    } else {
        return Err("compose 需要 chain（预置链）或 steps（自定义步骤）参数".into());
    };
    let stop_on_error = args["stop_on_error"].as_bool().unwrap_or(true);
    let chain_label = args["chain"]
        .as_str()
        .map(|c| format!("（{c}）"))
        .unwrap_or_default();
    let mut out = format!("组合链执行{chain_label}，共 {} 步：\n", steps.len());
    let mut failed = 0usize;
    for (i, (tool, step_args, fb)) in steps.iter().enumerate() {
        let merged = merge_args(args, step_args);
        // fb 为链上声明的 fallback：先合并链级参数再借用（match 模式自动解引用，闭包参数不会）
        let fb_owned: Option<(String, Value)> = match fb {
            Some((f, fa)) => Some((f.clone(), merge_args(args, fa))),
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
    } else {
        out.push_str(&format!("\n组合链结束：{} 步失败。", failed));
    }
    Ok(out)
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
