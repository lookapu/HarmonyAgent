//! 本地使用统计（usage_events）的 Tauri command 层
//!
//! ## 承诺（三条，全部可验证）
//!
//! 1. **默认关闭**：开关存在 settings 表，键不存在即关闭。
//!    `record_usage_event` 在关闭时**不写库也不报错**——没开统计不是错误状态，
//!    埋点在正常路径上，不该让调用方为它 try/catch。
//! 2. **只落本地**：本文件没有任何网络调用，也**没有**「上报」command。
//!    想外发数据必须新写一条 command 并改文档，那是一次显式的决定而非默认行为。
//! 3. **事件名白名单**：白名单外的名字被拒绝（InvalidParameterName），
//!    而不是"先记下来再说"。埋点漂移会让聚合静默失真且无人察觉。
//!
//! ## 为什么 record 的错误要吞掉
//!
//! 埋点失败**绝不能影响主流程**（发消息、构建、打开文件）。所以这里
//! `record_usage_event` 返回的 Err 被静默忽略——白名单外的名字意味着
//! 开发期改了前端没改后端（或反之），而那种问题应该由契约测试抓，
//! 不该以"用户发送消息失败了"的形式暴露出来。
//!
//! 严格模式 `record_usage_event_strict` 保留原始错误，供契约测试与自检使用。

use tauri::State;

use crate::db::{queries, DbState};

/// 取应用版本（编译期来自 Cargo.toml，避免手写常量与 package.json 漂移）
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// 统计开关是否开启
#[tauri::command]
pub fn usage_stats_enabled(db: State<DbState>) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::usage_events_enabled(&conn).map_err(|e| e.to_string())
}

/// 开关设置。**关闭时连带清空已记录数据**——用户关掉它通常意味着不想留。
#[tauri::command]
pub fn set_usage_stats_enabled(db: State<DbState>, enabled: bool) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::set_usage_events_enabled(&conn, enabled).map_err(|e| e.to_string())
}

/// 记录一条使用事件（正常路径，失败静默）
#[tauri::command]
pub fn record_usage_event(
    db: State<DbState>,
    event_name: String,
    props: Option<String>,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    // 埋点失败不影响主流程：白名单外的名字说明前后端事件名没对齐，
    // 那由 frontend_backend_contract 之类的契约测试抓，不该让用户操作失败。
    let _ = queries::record_usage_event(
        &conn,
        &event_name,
        &app_version(),
        props.as_deref().unwrap_or("{}"),
    );
    Ok(())
}

/// 记录一条使用事件（严格模式：把底层错误抛回）
///
/// 仅供契约测试与「设置页 → 校验埋点」的自检按钮使用；
/// 正常埋点请用 record_usage_event。
#[tauri::command]
pub fn record_usage_event_strict(
    db: State<DbState>,
    event_name: String,
    props: Option<String>,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::record_usage_event(
        &conn,
        &event_name,
        &app_version(),
        props.as_deref().unwrap_or("{}"),
    )
    .map_err(|e| e.to_string())
}

/// 汇总：按事件名聚合计数。`since_ts` 传 0 表示不限。
#[tauri::command]
pub fn usage_stats_summary(
    db: State<DbState>,
    since_ts: Option<i64>,
) -> Result<Vec<queries::UsageEventSummary>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::usage_event_summary(&conn, since_ts.unwrap_or(0)).map_err(|e| e.to_string())
}

/// 统计里的总条数
#[tauri::command]
pub fn usage_stats_total(db: State<DbState>) -> Result<i64, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::usage_event_total(&conn).map_err(|e| e.to_string())
}

/// 清空全部统计记录（不改变开关状态）
#[tauri::command]
pub fn clear_usage_stats(db: State<DbState>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::clear_usage_events(&conn).map_err(|e| e.to_string())
}

/// 事件名白名单（前端埋点与设置页自检共用，避免两边各抄一份）
#[tauri::command]
pub fn usage_event_names() -> Vec<String> {
    queries::EVENT_NAMES.iter().map(|s| s.to_string()).collect()
}
