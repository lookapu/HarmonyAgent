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
/// 点停止后未生效强杀宽限：40 秒
const STOP_GRACE_MS: i64 = 40_000;
/// 看门狗扫描周期
const SCAN_INTERVAL_SECS: u64 = 20;

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
