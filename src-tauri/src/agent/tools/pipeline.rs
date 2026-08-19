//! 工具执行流水线：pre → execute → post 三阶段。
//!
//! execute 阶段由 run_tool 的既有工具分发承担（本模块不接管分发）；
//! pre/post 阶段支持注册异步钩子（hooks），供预算控制、黑名单、权限审批、
//! 副作用追踪、大输出落盘等横切关注点挂接，无需改动任何工具实现。
//! 参考 deepseek-harness 的 pre-execute / execute / post-execute 三层模型。
//!
//! - pre 钩子：工具执行前运行，返回 `Err(Intercept)` 可拦截（终止）本次工具调用；
//!   调用方按 `InterceptKind` 决定收尾方式（预算/黑名单拦截 → 请求总结并终止，
//!   审批拒绝/通用错误 → 直接终止，均不再向模型反馈工具结果）；
//! - post 钩子：工具执行后运行，可读取并改写结果（截断大输出、追加护栏提示），
//!   不改变执行本身。

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;

use crate::agent::exec_ctx::ToolCtx;

/// 拦截原因分类：调用方据此决定收尾方式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterceptKind {
    /// 任务预算/次数超限
    Budget,
    /// 黑名单拦截（危险操作/目标锚定）
    Blacklist,
    /// 用户审批拒绝或审批等待超时
    Approval,
    /// 用户主动停止生成（区别于"拒绝"：任务应按停止收尾，而非正常完成）
    Cancelled,
    /// 其他拦截（钩子内部错误等）
    Generic,
}

/// pre 钩子拦截结果：分类 + 面向日志/前端的事件消息
#[derive(Debug, Clone)]
pub struct Intercept {
    pub kind: InterceptKind,
    pub message: String,
}

impl Intercept {
    pub fn new(kind: InterceptKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}

impl std::fmt::Display for Intercept {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// 工具调用上下文快照：钩子可见的只读视图（生命周期绑定调用期间）。
pub struct ToolInvocation<'a> {
    /// 工具名（含 mcp__ 前缀时为 MCP 工具）
    pub name: &'a str,
    /// 已解析的工具参数（未解析成功时为空对象）
    pub args: &'a Value,
    /// 模型输出的原始参数文本（预算打转比较/审批弹窗展示用原样，避免序列化差异）
    pub args_raw: &'a str,
    /// 会话项目 id
    pub project_id: &'a str,
    /// 有效根目录（用户指定目录优先，会话项目根兜底）
    pub roots: &'a [String],
    /// 会话 id（审批事件、护栏记录、注入队列均按会话定位）
    pub conversation_id: &'a str,
    /// 审批模式（allow_all / ask，来自 ChatOptions.tool_approval）
    pub approval_mode: &'a str,
    /// 工具执行上下文（事件发射、日志等）
    pub ctx: &'a ToolCtx,
}

/// pre 钩子：async，返回 `Err(Intercept)` 则终止本次工具执行（拦截），`Ok` 继续。
/// HRTB：Future 生命周期绑定在入参引用上，钩子可安全捕获 `&ToolInvocation`。
pub type PreHook = Box<
    dyn for<'a> Fn(
            &'a ToolInvocation<'_>,
        ) -> Pin<Box<dyn Future<Output = Result<(), Intercept>> + Send + 'a>>
        + Send
        + Sync,
>;

/// post 钩子：async，可读取并改写执行结果（成功/失败）。
pub type PostHook = Box<
    dyn for<'a> Fn(
            &'a ToolInvocation<'_>,
            &'a mut Result<String, String>,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>
        + Send
        + Sync,
>;

struct PipelineRegistry {
    pre: Mutex<Vec<Arc<PreHook>>>,
    post: Mutex<Vec<Arc<PostHook>>>,
}

static REGISTRY: OnceLock<PipelineRegistry> = OnceLock::new();

fn registry() -> &'static PipelineRegistry {
    REGISTRY.get_or_init(|| PipelineRegistry {
        pre: Mutex::new(Vec::new()),
        post: Mutex::new(Vec::new()),
    })
}

/// 注册 pre 钩子：在每次工具执行前调用；任一钩子返回 `Err` 将终止该次工具调用。
pub fn register_pre_hook(hook: PreHook) {
    if let Ok(mut hooks) = registry().pre.lock() {
        hooks.push(Arc::new(hook));
    }
}

/// 注册 post 钩子：在每次工具执行后调用，可改写结果（截断/追加提示）。
pub fn register_post_hook(hook: PostHook) {
    if let Ok(mut hooks) = registry().post.lock() {
        hooks.push(Arc::new(hook));
    }
}

/// 运行全部 pre 钩子；任一返回 `Err(Intercept)` 立即终止并回传该拦截。
/// 注册表快照在锁内克隆、锁外逐项 await（MutexGuard 不能跨 await 持有）。
pub async fn run_pre_hooks(inv: &ToolInvocation<'_>) -> Result<(), Intercept> {
    let hooks = {
        let guard = registry()
            .pre
            .lock()
            .map_err(|e| Intercept::new(InterceptKind::Generic, e.to_string()))?;
        (*guard).clone()
    };
    for hook in hooks.iter() {
        if let Err(intercept) = hook(inv).await {
            return Err(intercept);
        }
    }
    Ok(())
}

/// 运行全部 post 钩子（钩子自身失败不影响工具结果）。
pub async fn run_post_hooks(inv: &ToolInvocation<'_>, result: &mut Result<String, String>) {
    let hooks = match registry().post.lock() {
        Ok(guard) => (*guard).clone(),
        Err(_) => return,
    };
    for hook in hooks.iter() {
        hook(inv, result).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation<'a>(
        name: &'a str,
        args: &'a Value,
        roots: &'a [String],
        ctx: &'a ToolCtx,
    ) -> ToolInvocation<'a> {
        ToolInvocation {
            name,
            args,
            args_raw: "",
            project_id: "",
            roots,
            conversation_id: "test",
            approval_mode: "allow_all",
            ctx,
        }
    }

    #[tokio::test]
    async fn pipeline_pre_hook_can_intercept() {
        // pre 钩子拦截：返回 Err 时工具不执行（结果 = 拦截错误）
        register_pre_hook(Box::new(|inv| {
            Box::pin(async move {
                if inv.name == "blocked_tool" {
                    Err(Intercept::new(InterceptKind::Generic, "被 pre 钩子拦截"))
                } else {
                    Ok(())
                }
            })
        }));
        let args = Value::Null;
        let roots: Vec<String> = Vec::new();
        let ctx = ToolCtx::empty();
        let inv = invocation("blocked_tool", &args, &roots, &ctx);
        let err = run_pre_hooks(&inv).await.unwrap_err();
        assert!(err.message.contains("拦截"));
        // 非拦截工具正常通过
        let inv = invocation("read_file", &args, &roots, &ctx);
        assert!(run_pre_hooks(&inv).await.is_ok());
    }

    #[tokio::test]
    async fn pipeline_post_hook_observes_result() {
        // post 钩子观察到结果（成功/失败各一次）
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEEN_OK: AtomicUsize = AtomicUsize::new(0);
        static SEEN_ERR: AtomicUsize = AtomicUsize::new(0);
        register_post_hook(Box::new(|_inv, result| {
            Box::pin(async move {
                if result.is_ok() {
                    SEEN_OK.fetch_add(1, Ordering::SeqCst);
                } else {
                    SEEN_ERR.fetch_add(1, Ordering::SeqCst);
                }
            })
        }));
        let args = Value::Null;
        let roots: Vec<String> = Vec::new();
        let ctx = ToolCtx::empty();
        let inv = invocation("read_file", &args, &roots, &ctx);
        let mut ok: Result<String, String> = Ok("ok".to_string());
        let mut fail: Result<String, String> = Err("fail".to_string());
        run_post_hooks(&inv, &mut ok).await;
        run_post_hooks(&inv, &mut fail).await;
        assert_eq!(SEEN_OK.load(Ordering::SeqCst), 1);
        assert_eq!(SEEN_ERR.load(Ordering::SeqCst), 1);
    }
}
