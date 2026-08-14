use tauri::State;
use crate::db::{
    models::{CostSummary, DailyUsage, RequestLog, TaskStats},
    queries, DbState,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct DateRange {
    pub start: String,
    pub end: String,
}

#[derive(Debug, Deserialize)]
pub struct LogFilter {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub provider_id: Option<String>,
    pub model: Option<String>,
}

#[tauri::command]
pub fn get_cost_summary(db: State<DbState>, range: DateRange) -> Result<CostSummary, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    let daily = queries::get_daily_usage(&conn, &range.start, &range.end)
        .map_err(|e| e.to_string())?;

    let mut total_requests: i64 = 0;
    let mut total_input: i64 = 0;
    let mut total_output: i64 = 0;
    let mut total_cost: f64 = 0.0;

    for d in &daily {
        total_requests += d.request_count;
        total_input += d.input_tokens;
        total_output += d.output_tokens;
        total_cost += d.total_cost_cny;
    }

    // 按模型分组统计（请求日志明细聚合）
    let by_model = queries::get_cost_by_model(
        &conn,
        date_start_ts(&range.start),
        date_end_ts(&range.end),
    )
    .map_err(|e| e.to_string())?;

    Ok(CostSummary {
        total_requests,
        total_input_tokens: total_input,
        total_output_tokens: total_output,
        total_cost_cny: total_cost,
        by_provider: vec![],
        by_model,
    })
}

/// 日期字符串（YYYY-MM-DD）→ 当天 00:00 的秒级时间戳（UTC，与日志 created_at 对齐）
fn date_start_ts(date: &str) -> i64 {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|dt| dt.and_utc().timestamp())
        .unwrap_or(0)
}

/// 日期字符串（YYYY-MM-DD）→ 当天 23:59:59 的秒级时间戳
fn date_end_ts(date: &str) -> i64 {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(23, 59, 59))
        .map(|dt| dt.and_utc().timestamp())
        .unwrap_or(i64::MAX)
}

#[tauri::command]
pub fn get_request_logs(db: State<DbState>, filter: LogFilter) -> Result<Vec<RequestLog>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let limit = filter.limit.unwrap_or(50);
    let offset = filter.offset.unwrap_or(0);
    queries::get_request_logs(&conn, limit, offset).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_daily_usage(db: State<DbState>, range: DateRange) -> Result<Vec<DailyUsage>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::get_daily_usage(&conn, &range.start, &range.end).map_err(|e| e.to_string())
}

/// 任务级指标聚合（成功率 / P50 / P95 / 成本 / 错误分布）；project_id 为空 = 全局
#[tauri::command]
pub fn get_task_stats(db: State<DbState>, project_id: Option<String>, days: Option<i64>) -> Result<TaskStats, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let project_id = project_id.unwrap_or_default();
    let days = days.unwrap_or(30).clamp(1, 365);
    queries::get_task_stats(&conn, &project_id, days).map_err(|e| e.to_string())
}

/// 最近任务列表（trace 明细）：project_id 为空 = 全局；status 可选过滤；limit 缺省 20
#[tauri::command]
pub fn get_task_runs(
    db: State<DbState>,
    project_id: Option<String>,
    status: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<crate::db::models::TaskRun>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let limit = limit.unwrap_or(20).clamp(1, 200);
    queries::list_task_runs(&conn, &project_id.unwrap_or_default(), status.as_deref(), limit)
        .map_err(|e| e.to_string())
}
