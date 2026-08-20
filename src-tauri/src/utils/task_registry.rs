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
use std::sync::atomic::{AtomicI64, Ordering};
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
    /// 登记运行中任务（任务 spawn 后立即调用）
    pub fn register(&self, conversation_id: &str, abort: tokio::task::AbortHandle) {
        if let Ok(mut m) = self.0.lock() {
            let now = now_ms();
            m.insert(
                conversation_id.to_string(),
                TaskHandle {
                    abort,
                    heartbeat_ms: AtomicI64::new(now),
                    phase: AtomicI64::new(PHASE_START),
                    stop_requested_at: AtomicI64::new(0),
                    stream_data_ms: AtomicI64::new(0),
                    stream_progress_ms: AtomicI64::new(0),
                    started_at: now,
                },
            );
        }
    }

    /// 任务正常收尾注销
    pub fn unregister(&self, conversation_id: &str) {
        if let Ok(mut m) = self.0.lock() {
            m.remove(conversation_id);
        }
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
    loop {
        std::thread::sleep(std::time::Duration::from_secs(SCAN_INTERVAL_SECS));
        let registry = app.state::<TaskRegistry>();
        let now = now_ms();
        // 收集待杀任务（先快照后杀，避免遍历中修改）
        let mut to_kill: Vec<(String, String, i64)> = Vec::new();
        {
            if let Ok(m) = registry.0.lock() {
                for (cid, h) in m.iter() {
                    let last = h.heartbeat_ms.load(Ordering::Relaxed);
                    let phase = h.phase.load(Ordering::Relaxed);
                    let stop_at = h.stop_requested_at.load(Ordering::Relaxed);
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
        for (cid, reason, elapsed_ms) in to_kill {
            let abort = if let Ok(mut m) = registry.0.lock() {
                m.remove(&cid).map(|h| h.abort)
            } else {
                None
            };
            if let Some(abort) = abort {
                abort.abort();
                crate::utils::logger::log_event(
                    "watchdog_kill",
                    serde_json::json!({
                        "conversation_id": cid,
                        "reason": reason,
                        "elapsed_ms": elapsed_ms,
                    }),
                );
                let _ = app.emit(
                    "chat-error",
                    serde_json::json!({
                        "conversation_id": cid,
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
