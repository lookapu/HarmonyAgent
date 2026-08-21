//! 任务监管注册表 + 看门狗强杀。
//!
//! 背景：对话任务线程一旦卡在无防护的同步/异步点（如 Provider 挂起、emit 阻塞），
//! 协作式停止标志（ChatCancel）无人消费，表现为"点停止无效、空跑不结束"。
//! 本模块为每个运行中任务登记 AbortHandle + 心跳时间戳，由独立看门狗线程周期扫描：
//! - 心跳长期停滞（任务线程整体卡死）→ 强制 abort 并通知前端
//! - 用户点停止后宽限期内停止标志未被消费（is_cancelled 未生效）→ 强制 abort
//!
//! 心跳为纯原子写（高频调用路径，不落日志），看门狗杀任务时统一落 watchdog_kill 日志。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter, Manager};

/// 阶段编码：心跳打点位置（看门狗日志里报告最后卡点）
pub const PHASE_START: i64 = 1;
pub const PHASE_MAIN_LOOP: i64 = 2;
pub const PHASE_ROUND_REQUEST: i64 = 3;
pub const PHASE_SEND: i64 = 4;
pub const PHASE_STREAMING: i64 = 5;
pub const PHASE_TOOL: i64 = 6;

/// 无心跳强杀阈值：8 分钟（覆盖长工具执行，如 hvigor 构建可达数分钟；
/// 正常请求阶段每 200~300ms 有 PHASE_SEND/STREAMING 心跳，工具阶段有 PHASE_TOOL 心跳，
/// 仅同步卡死/线程死锁才会持续无心跳，故比 10 分钟更早兜底）
const TASK_STALL_MS: i64 = 480_000;
/// 流式阶段无数据到达强杀阈值：300 秒（对齐 DeepSeek-Reasonix 的 5 分钟流空闲超时）。
/// 必须大于 tokio 层软机制 STREAM_SILENT_TIMEOUT（60s 无有效产出→保留已收内容自动续写）
/// 与产出前中断重放（0 产出时冻结请求原样重发，最多 5 次）：正常断流由软机制与重放
/// 先行恢复，看门狗仅兜底软机制失效的场景——worker 被同步代码钉死（tokio timer 停转、
/// 续写/重放都无法触发）时在 300s 内强杀。判据是“数据到达”而非“有效产出”：
/// 模型输出大/长响应（体积、行数远超常态，解析需要更长时间）时，只要网络仍在到达
/// 数据就不强杀——输出量级不是关闭理由。独立 OS 线程不依赖 tokio timer。
const STREAM_STALL_MS: i64 = 300_000;
/// 点停止后未生效强杀宽限：40 秒
const STOP_GRACE_MS: i64 = 40_000;
/// 看门狗扫描周期
const SCAN_INTERVAL_SECS: u64 = 5;

fn is_resume_gap(scan_gap_ms: i64) -> bool {
    scan_gap_ms > (SCAN_INTERVAL_SECS as i64 * 4 * 1000)
}

pub fn phase_name(p: i64) -> &'static str {
    match p {
        PHASE_START => "start",
        PHASE_MAIN_LOOP => "main_loop",
        PHASE_ROUND_REQUEST => "round_request",
        PHASE_SEND => "send",
        PHASE_STREAMING => "streaming",
        PHASE_TOOL => "tool",
        _ => "unknown",
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub struct TaskHandle {
    /// 同一会话多次运行的代次；旧任务收尾不得注销随后启动的新任务。
    pub generation: u64,
    /// 前后端事件代次 ID；任务主体启动后写入，供看门狗终态事件隔离旧任务。
    pub run_id: StdMutex<Option<String>>,
    pub abort: tokio::task::AbortHandle,
    pub heartbeat_ms: AtomicI64,
    pub phase: AtomicI64,
    pub stop_requested_at: AtomicI64,
    /// 流式阶段最近一次“数据到达”时间戳（收到任何网络 chunk 即刷新，无论是否解析出内容）。
    /// 大输出/长响应时数据持续到达但解析可能长时间无产出，故以数据到达为看门狗判据：
    /// 只要数据还在到达就说明流仍存活，不强制关闭；数据停滞才判定挂起。
    pub stream_data_ms: AtomicI64,
    /// 流式阶段最近一次“有效产出”时间戳（正文/思考/工具增量/首字节/结束标记）。
    /// 仅信息用途（排障日志），不作为强杀判据——产出慢不等于流挂起。
    pub stream_progress_ms: AtomicI64,
    pub started_at: i64,
}

#[derive(Default)]
pub struct TaskRegistry(pub StdMutex<HashMap<String, TaskHandle>>);

impl TaskRegistry {
    /// 尝试登记运行中任务。已存在时拒绝，避免并发 stream_chat 覆盖原任务的 AbortHandle。
    /// 返回本次代次，正常收尾时必须带回该值做条件注销。
    pub fn register(&self, conversation_id: &str, abort: tokio::task::AbortHandle) -> Option<u64> {
        static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
        if let Ok(mut m) = self.0.lock() {
            if m.contains_key(conversation_id) {
                return None;
            }
            let now = now_ms();
            let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
            m.insert(
                conversation_id.to_string(),
                TaskHandle {
                    generation,
                    run_id: StdMutex::new(None),
                    abort,
                    heartbeat_ms: AtomicI64::new(now),
                    phase: AtomicI64::new(PHASE_START),
                    stop_requested_at: AtomicI64::new(0),
                    stream_data_ms: AtomicI64::new(0),
                    stream_progress_ms: AtomicI64::new(0),
                    started_at: now,
                },
            );
            Some(generation)
        } else {
            None
        }
    }

    /// 任务正常收尾注销；只移除自己的代次，防看门狗强杀后的旧 join 误删新任务。
    pub fn unregister(&self, conversation_id: &str, generation: u64) {
        if let Ok(mut m) = self.0.lock() {
            if m.get(conversation_id).map(|h| h.generation) == Some(generation) {
                m.remove(conversation_id);
            }
        }
    }

    pub fn set_run_id(&self, conversation_id: &str, run_id: &str) {
        if let Ok(m) = self.0.lock() {
            if let Some(h) = m.get(conversation_id) {
                if let Ok(mut current) = h.run_id.lock() {
                    *current = Some(run_id.to_string());
                }
            }
        }
    }

    /// 读取当前任务代次，供工具/子 Agent 的过程事件绑定到正确运行。
    pub fn run_id(&self, conversation_id: &str) -> String {
        self.0
            .lock()
            .ok()
            .and_then(|m| {
                m.get(conversation_id)
                    .and_then(|h| h.run_id.lock().ok().and_then(|id| id.clone()))
            })
            .unwrap_or_default()
    }

    /// 心跳打点：更新最后活跃时间与阶段（原子写，高频路径）
    pub fn touch(&self, conversation_id: &str, phase: i64) {
        if let Ok(m) = self.0.lock() {
            if let Some(h) = m.get(conversation_id) {
                h.heartbeat_ms.store(now_ms(), Ordering::Relaxed);
                h.phase.store(phase, Ordering::Relaxed);
            }
        }
    }

    /// 流式数据到达打点：每收到一个网络 chunk 即调用（无论是否解析出内容）。
    /// 独立 OS 线程看门狗据此判断流是否仍在传输（大输出/长响应处理中不误杀）；
    /// 同时刷新心跳，避免大输出解析期间触发无心跳兜底。
    pub fn touch_stream_data(&self, conversation_id: &str) {
        if let Ok(m) = self.0.lock() {
            if let Some(h) = m.get(conversation_id) {
                let now = now_ms();
                h.heartbeat_ms.store(now, Ordering::Relaxed);
                h.stream_data_ms.store(now, Ordering::Relaxed);
                h.phase.store(PHASE_STREAMING, Ordering::Relaxed);
            }
        }
    }

    /// 流式有效产出打点：仅在解析到正文/思考/工具增量/首字节/结束标记时调用。
    /// 保留用于排障信息（stream_progress_ms），不再作为看门狗强杀判据。
    pub fn touch_stream_progress(&self, conversation_id: &str) {
        if let Ok(m) = self.0.lock() {
            if let Some(h) = m.get(conversation_id) {
                let now = now_ms();
                h.heartbeat_ms.store(now, Ordering::Relaxed);
                h.stream_progress_ms.store(now, Ordering::Relaxed);
                h.phase.store(PHASE_STREAMING, Ordering::Relaxed);
            }
        }
    }

    /// 记录用户停止请求时间（看门狗据此判断协作停止是否失效）
    pub fn mark_stop_requested(&self, conversation_id: &str) {
        if let Ok(m) = self.0.lock() {
            if let Some(h) = m.get(conversation_id) {
                h.stop_requested_at.store(now_ms(), Ordering::Relaxed);
            }
        }
    }

    /// 删除会话等场景：立即请求停止并 abort 正在运行的任务。
    /// 返回 true 表示曾有运行中任务被中止。
    pub fn abort_conversation(&self, conversation_id: &str) -> bool {
        self.mark_stop_requested(conversation_id);
        let abort = if let Ok(mut m) = self.0.lock() {
            m.remove(conversation_id).map(|h| h.abort)
        } else {
            None
        };
        if let Some(abort) = abort {
            abort.abort();
            crate::utils::logger::log_event(
                "task_aborted",
                serde_json::json!({
                    "conversation_id": conversation_id,
                    "reason": "conversation_deleted",
                }),
            );
            true
        } else {
            false
        }
    }

    /// 懒启动看门狗线程（全局仅一次）：周期扫描注册表，强杀卡死/停止失效任务
    pub fn ensure_watchdog(app: AppHandle) {
        static WATCHDOG: OnceLock<()> = OnceLock::new();
        WATCHDOG.get_or_init(|| {
            std::thread::Builder::new()
                .name("task-watchdog".into())
                .spawn(move || watchdog_loop(app))
                .ok();
        });
    }
}

fn watchdog_loop(app: AppHandle) {
    let mut last_scan_ms = now_ms();
    loop {
        std::thread::sleep(std::time::Duration::from_secs(SCAN_INTERVAL_SECS));
        let registry = app.state::<TaskRegistry>();
        let now = now_ms();
        let scan_gap_ms = now.saturating_sub(last_scan_ms);
        last_scan_ms = now;
        // Windows 现代待机/macOS 睡眠期间 wall clock 会继续前进，但任务线程、网络和
        // 看门狗都被冻结。唤醒后的第一次扫描若直接比较 wall clock，会把所有健康任务
        // 误判成 8 分钟卡死。检测到扫描间隔异常时给活跃任务一次完整宽限再继续巡检。
        if is_resume_gap(scan_gap_ms) {
            if let Ok(m) = registry.0.lock() {
                for h in m.values() {
                    h.heartbeat_ms.store(now, Ordering::Relaxed);
                    if h.stream_data_ms.load(Ordering::Relaxed) > 0 {
                        h.stream_data_ms.store(now, Ordering::Relaxed);
                    }
                    if h.stream_progress_ms.load(Ordering::Relaxed) > 0 {
                        h.stream_progress_ms.store(now, Ordering::Relaxed);
                    }
                }
            }
            crate::utils::logger::log_event(
                "watchdog_resume_grace",
                serde_json::json!({ "scan_gap_ms": scan_gap_ms }),
            );
            continue;
        }
        // 收集待杀任务（先快照后杀，避免遍历中修改）
        let mut to_kill: Vec<(String, String, i64)> = Vec::new();
        let mut heartbeats: Vec<(String, String, i64, i64, u64)> = Vec::new();
        {
            if let Ok(m) = registry.0.lock() {
                for (cid, h) in m.iter() {
                    let last = h.heartbeat_ms.load(Ordering::Relaxed);
                    let phase = h.phase.load(Ordering::Relaxed);
                    let stop_at = h.stop_requested_at.load(Ordering::Relaxed);
                    let run_id = h
                        .run_id
                        .lock()
                        .ok()
                        .and_then(|id| id.clone())
                        .unwrap_or_default();
                    heartbeats.push((cid.clone(), run_id, phase, h.started_at, h.generation));
                    // 流式阶段：以“数据到达时间戳”为基准，60s 无任何数据即强杀。
                    // 判据是数据而非产出：模型输出大/长响应时解析可能长时间无产出，
                    // 但只要数据仍在到达就说明流存活，不应强杀；数据完全停滞
                    // （网络挂起/worker 被同步代码钉死不再读流）才触发兜底。
                    // 此检查跑在独立 OS 线程上，不依赖 tokio timer。
                    if phase == PHASE_STREAMING {
                        let data = h.stream_data_ms.load(Ordering::Relaxed);
                        let base = if data > 0 { data } else { last };
                        if now - base > STREAM_STALL_MS {
                            to_kill.push((
                                cid.clone(),
                                "stream_no_data".to_string(),
                                now - h.started_at,
                            ));
                            continue;
                        }
                    }
                    if now - last > TASK_STALL_MS {
                        to_kill.push((
                            cid.clone(),
                            format!("no_heartbeat:{}", phase_name(phase)),
                            now - h.started_at,
                        ));
                    } else if stop_at > 0 && now - stop_at > STOP_GRACE_MS {
                        to_kill.push((
                            cid.clone(),
                            "stop_not_effective".to_string(),
                            now - h.started_at,
                        ));
                    }
                }
            }
        }
        // 心跳既刷新桌面端前端活性，也让 WebView2/WKWebView 重载后能自动重新挂接
        // 仍在运行的 Rust 任务。事件很小且 5 秒一次，不携带正文。
        for (cid, run_id, phase, started_at, generation) in heartbeats {
            // 快照后任务可能已完成或因 WebView 重载被 RAII 注销；再次核对代次，
            // 禁止把最后一帧过期心跳投递给新前端并复活已结束任务。
            let still_active = registry
                .0
                .lock()
                .ok()
                .and_then(|m| m.get(&cid).map(|h| h.generation == generation))
                .unwrap_or(false);
            if !still_active {
                continue;
            }
            let _ = app.emit(
                "chat-heartbeat",
                serde_json::json!({
                    "conversation_id": cid,
                    "run_id": run_id,
                    "phase": phase_name(phase),
                    "started_at": started_at,
                }),
            );
        }
        for (cid, reason, elapsed_ms) in to_kill {
            let task = if let Ok(mut m) = registry.0.lock() {
                m.remove(&cid)
            } else {
                None
            };
            if let Some(task) = task {
                let run_id = task
                    .run_id
                    .lock()
                    .ok()
                    .and_then(|id| id.clone())
                    .unwrap_or_default();
                // abort 会跳过审批/计划 Future 内的正常 remove 收尾；先删除对应 Sender，
                // 防止待确认表永久残留并让前端一直显示“待确认”。
                if let Ok(mut approvals) = app
                    .state::<crate::commands::chat::ToolApprovalState>()
                    .0
                    .lock()
                {
                    approvals.retain(|_, (_, _, conversation_id, _)| conversation_id != &cid);
                }
                if let Ok(mut plans) = app
                    .state::<crate::commands::chat::PlanApprovalState>()
                    .0
                    .lock()
                {
                    plans.retain(|_, pending| pending.conversation_id != cid);
                }
                crate::agent::ask::cancel_conversation(&cid);
                crate::agent::exec_ctx::request_stop_tool(&cid);
                task.abort.abort();
                if !run_id.is_empty() {
                    crate::agent::runtime::transition_global(
                        &run_id,
                        &cid,
                        "interrupted",
                        "watchdog_terminated",
                        Some(&format!("看门狗强制终止：{reason}")),
                    );
                }
                crate::utils::logger::log_event(
                    "watchdog_kill",
                    serde_json::json!({
                        "conversation_id": cid,
                        "run_id": run_id,
                        "reason": reason,
                        "elapsed_ms": elapsed_ms,
                    }),
                );
                let _ = app.emit(
                    "chat-error",
                    serde_json::json!({
                        "conversation_id": cid,
                        "run_id": run_id,
                        "error": "任务异常卡死或停止未生效，已被强制终止。请重试；若复现，可查看应用日志定位卡点。",
                        "kind": "timeout",
                        "title": "任务被强制终止",
                        "reason": format!("看门狗检测到{reason}，已强制中止任务"),
                        "suggestion": "请重试该请求；若频繁复现请检查模型 Provider 网络状况",
                        "retryable": true,
                        "status_code": null,
                    }),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn duplicate_registration_is_rejected() {
        let registry = TaskRegistry::default();
        let first = tokio::spawn(std::future::pending::<()>());
        let second = tokio::spawn(std::future::pending::<()>());
        let generation = registry.register("conv", first.abort_handle()).expect("first register");
        assert!(registry.register("conv", second.abort_handle()).is_none());
        registry.unregister("conv", generation);
        first.abort();
        second.abort();
    }

    #[tokio::test]
    async fn stale_unregister_does_not_remove_newer_task() {
        let registry = TaskRegistry::default();
        let first = tokio::spawn(std::future::pending::<()>());
        let generation = registry.register("conv", first.abort_handle()).expect("register");

        registry.unregister("conv", generation.wrapping_add(1));
        assert!(registry.0.lock().unwrap().contains_key("conv"));

        registry.unregister("conv", generation);
        assert!(!registry.0.lock().unwrap().contains_key("conv"));
        first.abort();
    }

    #[tokio::test]
    async fn exposes_current_run_id_for_process_events() {
        let registry = TaskRegistry::default();
        let task = tokio::spawn(std::future::pending::<()>());
        let generation = registry.register("conv-run", task.abort_handle()).expect("register");
        assert_eq!(registry.run_id("conv-run"), "");
        registry.set_run_id("conv-run", "run-123");
        assert_eq!(registry.run_id("conv-run"), "run-123");
        registry.unregister("conv-run", generation);
        assert_eq!(registry.run_id("conv-run"), "");
        task.abort();
    }

    #[test]
    fn grants_grace_after_system_sleep_but_not_normal_jitter() {
        assert!(!is_resume_gap(5_000));
        assert!(!is_resume_gap(20_000));
        assert!(is_resume_gap(90_000));
        assert!(is_resume_gap(8 * 60 * 1000));
    }
}
