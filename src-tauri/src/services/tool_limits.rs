//! 工具资源限制（防 Agent 打转烧资源）。
//!
//! 三层护栏（原则：只拦真正的卡死/打转，不限制正常的深度任务）：
//! - 全局并发：`build_project` / `deploy` 等重操作同一时间只允许 1 个（信号量排队）；
//! - 打转检测：同一任务内连续 N 次“同一工具 + 同一参数”的重复调用判定打转，
//!   只拦截无进展的原地踏步，连续读不同文件/搜索等正常推进不受限（单任务可达数百次）；
//! - 重操作频率：同一任务内 build/deploy 最多调用 N 次（有副作用且耗时，反复执行无意义）。
//!
//! 预算按会话记录、任务开始时重置：同一次任务的工具调用（含子 Agent）共享一份预算，
//! 避免多子 Agent 并行时各自打转。
//!
//! 各项阈值（总次数/重操作次数/打转检测）的默认值定义在 agent_limits，
//! 运行时以 agent_limits::current() 动态配置为准（设置页可调，0/-1 表示不限制），
//! 此处常量仅作默认值与测试引用。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// 单任务工具调用总预算（默认值；运行时以 agent_limits 配置为准）
pub const TASK_TOOL_CALL_LIMIT: usize = crate::services::agent_limits::DEFAULT_TOOL_CALL_LIMIT as usize;
/// 重操作（build/deploy）单任务最多调用次数（默认值；运行时以 agent_limits 配置为准）。
/// 取值需覆盖“创建→构建→部署”一轮闭环的合理调用数（run_command 也计入），
/// 真正的死循环由 REPEAT_CALL_LIMIT 与失败黑名单兜底。
pub const HEAVY_TOOL_CALL_LIMIT: usize = crate::services::agent_limits::DEFAULT_HEAVY_TOOL_CALL_LIMIT as usize;
/// 打转检测阈值：同一 (工具, 参数) 连续重复 N 次判定打转（默认值；运行时以 agent_limits 配置为准）
pub const REPEAT_CALL_LIMIT: usize = crate::services::agent_limits::DEFAULT_REPEAT_CALL_LIMIT as usize;
/// 计为重操作（单任务调用次数受限）的工具名：有副作用且耗时，反复执行无意义
const GATED_TOOLS: &[&str] = &["build_project", "deploy", "run_command"];
/// 需要并发信号量互斥的工具：重操作 + 写工作区操作（并发会踩踏 build 目录/.ohpm/暂存区）
const CONCURRENT_TOOLS: &[&str] = &[
    "build_project", "deploy", "run_command", "run_tests", "git_commit", "ohpm_install", "git_stash",
];

/// 重操作并发信号量（每工具 1 个 permit → 全局同一时间只有一个构建/部署）
static TOOL_GATES: OnceLock<HashMap<&'static str, Arc<Semaphore>>> = OnceLock::new();
/// 动态命名信号量（如 per-device 部署门控）：key 为运行时字符串
static NAMED_GATES: OnceLock<Mutex<HashMap<String, Arc<Semaphore>>>> = OnceLock::new();

/// 任务级工具调用计数：conversation_id -> 已调用次数 / 重操作次数 / 重复调用检测
struct TaskBudget {
    calls: usize,
    heavy_calls: usize,
    /// 上一次调用的 (工具, 参数)，用于重复动作打转检测
    last_call: Option<(String, String)>,
    /// 与 last_call 相同的连续次数
    repeat_count: usize,
}
static BUDGETS: Mutex<Option<HashMap<String, TaskBudget>>> = Mutex::new(None);

fn gates() -> &'static HashMap<&'static str, Arc<Semaphore>> {
    TOOL_GATES.get_or_init(|| {
        CONCURRENT_TOOLS
            .iter()
            .map(|&name| (name, Arc::new(Semaphore::new(1))))
            .collect()
    })
}

/// 任务开始：重置该会话的预算（避免上次任务的计数影响本次）
pub fn reset_task_budget(conversation_id: &str) {
    if let Ok(mut map) = BUDGETS.lock() {
        let m = map.get_or_insert_with(HashMap::new);
        m.insert(
            conversation_id.to_string(),
            TaskBudget {
                calls: 0,
                heavy_calls: 0,
                last_call: None,
                repeat_count: 0,
            },
        );
    }
}

/// 工具调用前检查预算：总次数 / 重操作次数任一超限即拒绝继续；
/// 连续重复调用（同一工具同一参数）达到阈值判定打转，引导模型换策略或总结。
/// 各项阈值取自 agent_limits 动态配置（0/-1 表示不限制）。
/// 返回 Err 时调用方应停止工具循环（错误反馈给模型，防止打转）
pub fn check_task_budget(conversation_id: &str, tool: &str, args: &str) -> Result<(), String> {
    let limits = crate::services::agent_limits::current();
    if let Ok(map) = BUDGETS.lock() {
        if let Some(b) = map.as_ref().and_then(|m| m.get(conversation_id)) {
            if let Some(limit) = limits.tool_calls() {
                if b.calls >= limit {
                    return Err(format!(
                        "本任务工具调用已达上限（{limit} 次），请停止调用工具并直接总结结果"
                    ));
                }
            }
            if GATED_TOOLS.contains(&tool) {
                if let Some(limit) = limits.heavy_tool_calls() {
                    if b.heavy_calls >= limit {
                        return Err(format!(
                            "本任务 {tool} 已调用 {limit} 次，请勿反复构建/部署，直接总结结果"
                        ));
                    }
                }
            }
            // 打转检测：连续重复调用同一工具同一参数
            let same = b.last_call.as_ref().is_some_and(|(t, a)| {
                t == tool && a == args.trim()
            });
            let repeat = if same { b.repeat_count + 1 } else { 0 };
            if let Some(limit) = limits.repeat_calls() {
                if repeat >= limit {
                    return Err(format!(
                        "检测到连续 {limit} 次重复调用（{tool} 参数相同），疑似打转无进展，请停止重复调用，直接总结结果或改用其他策略"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// 工具执行完成后记账（总次数 + 重操作次数 + 重复调用计数）
pub fn record_tool_call(conversation_id: &str, tool: &str, args: &str) {
    if let Ok(mut map) = BUDGETS.lock() {
        if let Some(b) = map.as_mut().and_then(|m| m.get_mut(conversation_id)) {
            b.calls += 1;
            if GATED_TOOLS.contains(&tool) {
                b.heavy_calls += 1;
            }
            let same = b.last_call.as_ref().is_some_and(|(t, a)| {
                t == tool && a == args.trim()
            });
            b.repeat_count = if same { b.repeat_count + 1 } else { 0 };
            b.last_call = Some((tool.to_string(), args.trim().to_string()));
        }
    }
}

/// 获取重操作并发许可：无护栏的工具立即返回 None；有护栏的工具等待队列
pub async fn acquire_gate(tool: &str) -> Option<OwnedSemaphorePermit> {
    let gate = gates().get(tool)?.clone();
    Some(gate.acquire_owned().await.ok()?)
}

/// 获取动态命名信号量许可（如 per-device 部署门控）。同名共享容量为 1 的信号量，
/// 不同名互不阻塞，用于"同资源串行、不同资源并行"。
pub async fn acquire_named_gate(name: &str) -> OwnedSemaphorePermit {
    let gate = {
        let map = NAMED_GATES.get_or_init(|| Mutex::new(HashMap::new()));
        let mut guard = map.lock().unwrap();
        guard
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(1)))
            .clone()
    };
    gate.acquire_owned().await.expect("named gate semaphore closed")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(conv: &str) {
        reset_task_budget(conv);
    }

    #[test]
    fn test_budget_reset_and_count() {
        setup("c1");
        assert!(check_task_budget("c1", "list_devices", "").is_ok());
        // 参数不同的多次调用视为正常推进（不会触发重复检测）
        for i in 0..5 {
            let args = format!("--device {i}");
            record_tool_call("c1", "list_devices", &args);
            assert!(check_task_budget("c1", "list_devices", &args).is_ok());
        }
        // 重置后计数清空
        setup("c1");
        assert!(check_task_budget("c1", "list_devices", "").is_ok());
    }

    #[test]
    fn test_total_limit_reached() {
        setup("c2");
        for _ in 0..TASK_TOOL_CALL_LIMIT {
            record_tool_call("c2", "list_devices", "");
        }
        let err = check_task_budget("c2", "list_devices", "").unwrap_err();
        assert!(err.contains("上限"));
    }

    #[test]
    fn test_heavy_tool_limit() {
        setup("c3");
        // 重操作 6 次后第 7 次被拒（放宽后覆盖一轮“创建→构建→部署”的合理调用数）
        for _ in 0..HEAVY_TOOL_CALL_LIMIT {
            record_tool_call("c3", "build_project", "{\"mode\":\"debug\"}");
        }
        let err = check_task_budget("c3", "build_project", "{\"mode\":\"debug\"}").unwrap_err();
        assert!(err.contains("build_project"));
        assert!(err.contains("6 次"));
        // 但轻量工具仍可用（总预算未满）
        assert!(check_task_budget("c3", "list_devices", "").is_ok());
    }

    #[test]
    fn test_repeat_call_detection() {
        setup("c4");
        // 连续重复同一工具同一参数：达到阈值被拒
        for i in 0..REPEAT_CALL_LIMIT {
            record_tool_call("c4", "read_file", "{\"path\":\"a.json\"}");
            let r = check_task_budget("c4", "read_file", "{\"path\":\"a.json\"}");
            if i + 1 >= REPEAT_CALL_LIMIT {
                assert!(r.is_err());
                assert!(r.unwrap_err().contains("重复调用"));
            } else {
                assert!(r.is_ok());
            }
        }
        // 换参数即视为正常推进，不触发
        setup("c5");
        for i in 0..REPEAT_CALL_LIMIT + 2 {
            let p = format!(r#"{{"path":"file{i}.json"}}"#);
            record_tool_call("c5", "read_file", &p);
            assert!(check_task_budget("c5", "read_file", &p).is_ok());
        }
        // 中途换参数会打断重复计数
        setup("c6");
        for _ in 0..REPEAT_CALL_LIMIT - 1 {
            record_tool_call("c6", "read_file", "{\"path\":\"a.json\"}");
        }
        record_tool_call("c6", "read_file", "{\"path\":\"b.json\"}");
        assert!(check_task_budget("c6", "read_file", "{\"path\":\"b.json\"}").is_ok());
    }

    #[test]
    fn test_unknown_conversation_passes() {
        // 未初始化预算的会话：放行（向后兼容）
        assert!(check_task_budget("ghost", "build_project", "").is_ok());
    }

    #[test]
    fn test_gates_contain_heavy_tools() {
        // 断言所有需互斥的工具都注册了全局并发护栏（信号量容量为 1）。
        // 不直接断言 available_permits()==1，因为其它模块的并发测试可能临时持有许可。
        let g = gates();
        for tool in CONCURRENT_TOOLS {
            assert!(g.contains_key(tool), "missing gate for {tool}");
        }
        // 轻量工具不应有 gate
        assert!(!g.contains_key("list_devices"));
        assert!(!g.contains_key("read_file"));
    }
}
