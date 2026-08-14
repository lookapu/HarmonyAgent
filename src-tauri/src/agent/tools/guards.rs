//! 工具执行流水线护栏钩子：预算/黑名单/审批（pre）+ 进度记录/大输出落盘（post）。
//!
//! 注册到 pipeline 注册表，由 chat.rs 主循环与子任务循环在工具调用点统一触发；
//! 拦截后的收尾语义由调用方按 `InterceptKind` 处理：
//! - Budget/Blacklist：给模型一次总结机会后终止任务（request_final_summary + break）
//! - Approval/Generic：直接终止工具循环（break）
//!
//! 审批弹窗依赖 AppHandle 上挂载的全局状态（ToolApprovalState 等），钩子只在
//! ToolInvocation.ctx.app 存在时生效；无事件环境（测试/离线调用）直接放行。

use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::commands::chat::{
    ChatCancel, ChatToolApprovalEvent, FirstWriteApprovedState, SessionToolAllowState,
    ToolApprovalState,
};
use crate::db::DbState;
use crate::services::{permissions, task_guard, tool_limits};

use super::pipeline::{
    Intercept, InterceptKind, ToolInvocation, register_post_hook, register_pre_hook,
};

/// 幂等注册全部护栏钩子（进程内常驻，可多次调用只注册一次）
pub fn ensure_registered() {
    static REGISTERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if REGISTERED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    // pre：任务预算（总次数/重操作/打转）
    register_pre_hook(Box::new(|inv| Box::pin(pre_budget(inv))));
    // pre：失败黑名单（同动作连续失败多次后拦截，防反复撞同一堵墙）
    register_pre_hook(Box::new(|inv| Box::pin(pre_blacklist(inv))));
    // pre：权限分级审核（allow_all 直接放行；ask/auto/first_write 决策 + 弹窗）
    register_pre_hook(Box::new(|inv| Box::pin(pre_approval(inv))));
    // post：任务护栏进度记录（强制验证/失速/目标锚定提示追加到结果）
    register_post_hook(Box::new(|inv, result| Box::pin(post_guard(inv, result))));
    // post：超大输出落盘（阈值以上截断为预览 + 提示模型用 read_file 读完整文件）
    register_post_hook(Box::new(|inv, result| Box::pin(post_spill(inv, result))));
}

/// 超大输出落盘阈值（字符）：超过后写 .deveco-agent/spill 并只返回预览
const SPILL_THRESHOLD: usize = 20_000;
/// 免落盘工具：输出含结构化标记，必须原样保留给上层处理（截图视觉闭环）
const NO_SPILL_TOOLS: &[&str] = &["take_screenshot", "verify_ui", "run_ui_flow"];

// ---------- pre 钩子 ----------

/// 任务级工具预算检查（防打转）：超限不再执行，由调用方请求总结并终止
async fn pre_budget(inv: &ToolInvocation<'_>) -> Result<(), Intercept> {
    if let Err(err) = tool_limits::check_task_budget(inv.conversation_id, inv.name, inv.args_raw) {
        return Err(Intercept::new(InterceptKind::Budget, err));
    }
    Ok(())
}

/// 失败黑名单预检：同一动作签名连续失败多次后，本轮不再重复执行，
/// 直接提示模型换方案（避免反复撞同一堵墙）
async fn pre_blacklist(inv: &ToolInvocation<'_>) -> Result<(), Intercept> {
    if task_guard::is_blacklisted(inv.conversation_id, inv.name, inv.args) {
        return Err(Intercept::new(
            InterceptKind::Blacklist,
            "（系统拦截：该操作在本任务中已连续失败多次，已被暂时拉黑。请更换工具、调整参数或改用其它方案，不要重复同样的尝试。）",
        ));
    }
    Ok(())
}

/// 权限分级审核：
/// - allow_all 模式：直接执行（但 run_command 仍受命令白名单/黑名单约束）
/// - ask 模式：已信任项目的 L0/L1 自动放行；L2 或未信任项目弹窗确认
/// - auto 模式：不依赖项目信任，L0/L1 一律免审，仅 L2 弹窗确认
/// - first_write 模式：写文件类工具首次弹窗确认，本任务后续写操作免审；其他工具直接放行
async fn pre_approval(inv: &ToolInvocation<'_>) -> Result<(), Intercept> {
    let Some(app) = inv.ctx.app.as_ref() else {
        return Ok(()); // 无事件环境（测试/离线）：直接放行
    };
    let approval_mode_str = inv.approval_mode;
    let tool = inv.name;
    let conversation_id = inv.conversation_id;
    let is_write_tool = matches!(tool, "edit_file" | "write_file" | "delete_file");
    let needs_approval = if approval_mode_str == "first_write" && is_write_tool {
        let approved = app
            .state::<FirstWriteApprovedState>()
            .0
            .lock()
            .map(|s| s.contains(conversation_id))
            .unwrap_or(false);
        !approved
    } else if approval_mode_str == "first_write" {
        // 非写文件工具：直接放行
        false
    } else {
        approval_mode_str != "allow_all" && {
            // 会话级“始终允许此工具”记忆：本会话已勾选允许则免审
            let session_allowed = {
                let allow = app.state::<SessionToolAllowState>();
                allow
                    .0
                    .lock()
                    .map(|s| s.contains(&(conversation_id.to_string(), tool.to_string())))
                    .unwrap_or(false)
            };
            if session_allowed {
                false
            } else if tool_whitelisted(&app.state::<DbState>(), inv.project_id, tool) {
                // 项目级持久化白名单：跨会话免审
                false
            } else {
                let trusted = is_project_trusted(&app.state::<DbState>(), inv.project_id)
                    || approval_mode_str == "auto";
                let cmd_arg = if tool == "run_command" {
                    serde_json::from_str::<serde_json::Value>(inv.args_raw)
                        .ok()
                        .and_then(|v| v.get("command").and_then(|x| x.as_str()).map(String::from))
                } else {
                    None
                };
                if permissions::auto_approve(tool, trusted, cmd_arg.as_deref()) {
                    false
                } else {
                    // 命中“始终允许”记忆也免审
                    let op_class = permissions::tool_level(tool).as_str();
                    let remembered = app
                        .state::<DbState>()
                        .0
                        .lock()
                        .map(|c| permissions::is_remembered(&c, inv.project_id, op_class))
                        .unwrap_or(false);
                    !remembered
                }
            }
        }
    };
    if !needs_approval {
        return Ok(());
    }
    let approval = app.state::<ToolApprovalState>();
    let cancel = app.state::<ChatCancel>();
    let (approved, feedback) = request_tool_approval(
        app,
        &approval,
        &cancel,
        conversation_id,
        tool,
        inv.args_raw,
    )
    .await
    .map_err(|e| Intercept::new(InterceptKind::Generic, e))?;
    if !approved {
        // 拒绝理由（用户可附）反馈给模型，帮助其调整方案而非盲目重试
        let reason = feedback
            .filter(|f| !f.trim().is_empty())
            .map(|f| format!("拒绝理由：{f}\n"))
            .unwrap_or_default();
        return Err(Intercept::new(
            InterceptKind::Approval,
            format!(
                "用户拒绝了该工具调用（权限审核未通过）。{reason}请停止该工具调用，直接给出结论或替代建议"
            ),
        ));
    }
    // first_write 模式：用户确认首次写文件后，本任务后续写操作免审
    if approval_mode_str == "first_write" && is_write_tool {
        if let Ok(mut s) = app.state::<FirstWriteApprovedState>().0.lock() {
            s.insert(conversation_id.to_string());
        }
    }
    Ok(())
}

// ---------- post 钩子 ----------

/// 任务护栏：记录进展并在需要时注入强制验证/失速/目标锚定提示
async fn post_guard(inv: &ToolInvocation<'_>, result: &mut Result<String, String>) {
    let args_val = serde_json::from_str(inv.args_raw).unwrap_or(serde_json::Value::Null);
    let hint = task_guard::record_tool(inv.conversation_id, inv.name, &args_val, result.is_ok());
    if let Some(note) = hint.to_injection() {
        match result {
            Ok(out) => out.push_str(&format!("\n\n{note}")),
            Err(e) => e.push_str(&format!("\n\n{note}")),
        }
    }
}

/// 超大输出落盘：成功结果超过阈值时写入 {root}/.deveco-agent/spill/，
/// 返回预览 + 定位符，模型需要完整内容时用 read_file 读取（防长输出撑爆上下文）
async fn post_spill(inv: &ToolInvocation<'_>, result: &mut Result<String, String>) {
    let Ok(out) = result else { return };
    let cnt = out.chars().count();
    if cnt <= SPILL_THRESHOLD || NO_SPILL_TOOLS.contains(&inv.name) {
        return;
    }
    let Some(root) = inv.roots.first() else { return };
    let spill_dir = std::path::Path::new(root).join(".deveco-agent").join("spill");
    if std::fs::create_dir_all(&spill_dir).is_err() {
        return;
    }
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let safe_name: String = inv
        .name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let fname = format!("{ts}-{safe_name}.txt");
    if std::fs::write(spill_dir.join(&fname), out.as_str()).is_err() {
        return;
    }
    let rel = format!(".deveco-agent/spill/{fname}");
    let head: String = out.chars().take(3000).collect();
    *out = format!(
        "{head}\n\n…(输出过长共 {cnt} 字符，已完整保存到 {rel}；如需继续处理请用 read_file 读取该文件)…"
    );
}

// ---------- 审批辅助（从 chat.rs 迁移，主循环/子任务循环与钩子共用） ----------

/// 查询项目是否已被用户“信任”（信任后 L0/L1 工具免审自动执行）
pub(crate) fn is_project_trusted(state: &DbState, project_id: &str) -> bool {
    state
        .0
        .lock()
        .ok()
        .and_then(|c| {
            c.query_row(
                "SELECT trusted FROM projects WHERE id = ?1",
                [project_id],
                |r| r.get::<_, i64>(0),
            )
            .ok()
        })
        .map(|v| v != 0)
        .unwrap_or(false)
}

/// 项目级工具免审白名单：项目 × 工具 已持久化允许（审批弹窗“本项目始终允许”）则返回 true
pub(crate) fn tool_whitelisted(state: &DbState, project_id: &str, tool: &str) -> bool {
    state
        .0
        .lock()
        .ok()
        .map(|c| {
            c.query_row(
                "SELECT 1 FROM tool_approval_whitelist WHERE project_id = ?1 AND tool = ?2",
                rusqlite::params![project_id, tool],
                |_| Ok(()),
            )
            .ok()
            .is_some()
        })
        .unwrap_or(false)
}

/// 检查并消费停止请求（一次性：读取后清除，避免影响后续请求）
pub(crate) fn is_cancelled(cancel: &ChatCancel, conversation_id: &str) -> bool {
    if let Ok(set) = cancel.0.lock() {
        if set.contains(conversation_id) {
            drop(set);
            if let Ok(mut set) = cancel.0.lock() {
                set.remove(conversation_id);
            }
            return true;
        }
    }
    false
}

/// 等待用户审核（自动审核模式）：发事件给前端并等待确认；超时按拒绝处理。
/// 命中会话级“始终允许”记忆直接放行。返回 (是否允许, 用户拒绝理由)。
pub(crate) async fn request_tool_approval(
    app: &AppHandle,
    state: &State<'_, ToolApprovalState>,
    cancel: &ChatCancel,
    conversation_id: &str,
    tool: &str,
    args: &str,
) -> Result<(bool, Option<String>), String> {
    // 会话级“始终允许此工具”记忆：本会话已勾选允许则直接放行，不再弹窗
    {
        let session_allowed = app
            .state::<SessionToolAllowState>()
            .0
            .lock()
            .map(|m| m.contains(&(conversation_id.to_string(), tool.to_string())))
            .unwrap_or(false);
        if session_allowed {
            return Ok((true, None));
        }
    }
    let request_id = Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let mut map = state.0.lock().map_err(|e| e.to_string())?;
        map.insert(
            request_id.clone(),
            (tx, tool.to_string(), conversation_id.to_string()),
        );
    }
    let _ = app.emit(
        "chat-tool-approval",
        ChatToolApprovalEvent {
            conversation_id: conversation_id.to_string(),
            request_id: request_id.clone(),
            tool: tool.to_string(),
            args: args.to_string(),
        },
    );
    // 60 秒未回复按拒绝处理（避免卡死任务）；等待期间用户点停止则立即返回
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut rx = rx;
    loop {
        tokio::select! {
            r = &mut rx => {
                let _ = state.0.lock().map(|mut m| m.remove(&request_id));
                return match r {
                    Ok((approved, feedback)) => Ok((approved, feedback)),
                    Err(_) => Ok((false, None)),
                };
            }
            _ = tokio::time::sleep_until(deadline) => {
                let _ = state.0.lock().map(|mut m| m.remove(&request_id));
                return Ok((false, None));
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                if is_cancelled(cancel, conversation_id) {
                    let _ = state.0.lock().map(|mut m| m.remove(&request_id));
                    return Ok((false, Some("用户已停止生成".to_string())));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::exec_ctx::ToolCtx;

    fn inv<'a>(
        name: &'a str,
        roots: &'a [String],
        ctx: &'a ToolCtx,
        args: &'a serde_json::Value,
    ) -> ToolInvocation<'a> {
        ToolInvocation {
            name,
            args,
            args_raw: "{}",
            project_id: "",
            roots,
            conversation_id: "test",
            approval_mode: "allow_all",
            ctx,
        }
    }

    #[tokio::test]
    async fn post_spill_writes_file_and_returns_preview() {
        let ctx = ToolCtx::empty();
        let dir = std::env::temp_dir().join(format!("spill-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).ok();
        let roots = vec![dir.to_string_lossy().to_string()];
        let args = serde_json::json!({});
        let mut result: Result<String, String> = Ok("x".repeat(SPILL_THRESHOLD + 100));
        post_spill(&inv("run_command", &roots, &ctx, &args), &mut result).await;
        let out = result.unwrap();
        assert!(out.contains(".deveco-agent/spill/"), "预览应含落盘定位符");
        assert!(out.chars().count() < SPILL_THRESHOLD, "预览应被截断");
        // 落盘文件存在且内容完整（长输出不丢，模型需要时 read_file 取回）
        let spill = dir.join(".deveco-agent").join("spill");
        let files: Vec<_> = std::fs::read_dir(&spill).unwrap().flatten().collect();
        assert_eq!(files.len(), 1, "应生成一个落盘文件");
        let content = std::fs::read_to_string(files[0].path()).unwrap();
        assert_eq!(content.len(), SPILL_THRESHOLD + 100, "落盘内容应完整");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn post_spill_skips_small_and_no_spill_tools() {
        let ctx = ToolCtx::empty();
        let args = serde_json::json!({});
        // 小输出不改写
        let mut result: Result<String, String> = Ok("短输出".repeat(10));
        post_spill(&inv("run_command", &[], &ctx, &args), &mut result).await;
        assert_eq!(result.unwrap(), "短输出".repeat(10));
        // 免落盘工具（截图/UI 验证）即使超长也原样保留（结构化标记供上层处理）
        let mut big: Result<String, String> = Ok("y".repeat(SPILL_THRESHOLD + 10));
        post_spill(&inv("take_screenshot", &[], &ctx, &args), &mut big).await;
        assert!(big.unwrap().starts_with("yyyy"));
    }

    #[tokio::test]
    async fn pre_approval_without_app_passes() {
        // 无事件环境（离线/测试）：直接放行，不弹窗不拦截
        let ctx = ToolCtx::empty();
        let args = serde_json::json!({});
        let inv = ToolInvocation {
            name: "run_command",
            args: &args,
            args_raw: r#"{"command":"rm -rf /"}"#,
            project_id: "p",
            roots: &[],
            conversation_id: "c",
            approval_mode: "ask",
            ctx: &ctx,
        };
        assert!(pre_approval(&inv).await.is_ok());
    }
}
