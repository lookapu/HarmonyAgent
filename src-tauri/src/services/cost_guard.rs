//! Token/成本软预警 + 模型自动降级。
//!
//! 与 budget.rs 的分工：budget 是**硬限额**（已用 + 本次预估超限 → 拒绝发送）；
//! 本模块是**软预警**，在放行路径上继续盯比率：
//! - 已用预算 ≥ 80%（WARN_RATIO）→ 返回 `Warn`，调用方发提醒事件（不阻断）；
//! - 已用预算 ≥ 90%（DOWNGRADE_RATIO）→ 返回 `Downgrade`，调用方联动
//!   model_router::pick_economy_model 自动切到同 Provider 更便宜的模型，
//!   避免"贵的模型继续烧预算直到硬拒"（如 deepseek-flash 扛不住时换 qwen-turbo）。
//!
//! 口径与 budget.rs 一致：已用成本 = task_runs.cost_cny（正式记账口径）；
//! 只看日/月预算中最接近的那一个（任一个达到阈值即预警）。

use chrono::{Datelike, TimeZone};
use rusqlite::Connection;

/// 软预警阈值：已用预算占比达到该值提醒用户（不阻断）
pub const WARN_RATIO: f64 = 0.8;
/// 自动降级阈值：已用预算占比达到该值自动切换经济模型
pub const DOWNGRADE_RATIO: f64 = 0.9;

/// 软预警状态
#[derive(Debug, Clone, PartialEq)]
pub enum SoftStatus {
    /// 未达预警线
    Normal,
    /// 已达预警线（≥80%）：提醒用户，不阻断
    Warn { used_cny: f64, limit_cny: f64, ratio: f64 },
    /// 已达降级线（≥90%）：应切换经济模型
    Downgrade { used_cny: f64, limit_cny: f64, ratio: f64 },
}

impl SoftStatus {
    /// 是否达到降级线
    pub fn should_downgrade(&self) -> bool {
        matches!(self, SoftStatus::Downgrade { .. })
    }

    /// 是否达到预警线（含降级线）
    pub fn should_warn(&self) -> bool {
        !matches!(self, SoftStatus::Normal)
    }
}

/// 软预警主入口：发送前在 budget 放行后调用，按日/月预算中占用率更高者返回状态。
/// Provider 未设置任何预算 → Normal（无上限无从预警）。
pub fn soft_check(conn: &Connection, provider_id: &str) -> SoftStatus {
    let (daily, monthly) = conn
        .query_row(
            "SELECT limit_daily_cny, limit_monthly_cny FROM providers WHERE id = ?1",
            rusqlite::params![provider_id],
            |r| Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, Option<f64>>(1)?)),
        )
        .unwrap_or((None, None));
    if daily.is_none() && monthly.is_none() {
        return SoftStatus::Normal;
    }

    let (used_today, used_month) = used_cost(conn, Some(provider_id));
    // 取占用率更高的那个限额作为预警依据（日/月任一接近上限都应提醒）
    let mut best: Option<(f64, f64, f64)> = None; // (used, limit, ratio)
    if let Some(d) = daily {
        if d > 0.0 {
            let ratio = used_today / d;
            if best.as_ref().map(|(_, _, r)| ratio > *r).unwrap_or(true) {
                best = Some((used_today, d, ratio));
            }
        }
    }
    if let Some(m) = monthly {
        if m > 0.0 {
            let ratio = used_month / m;
            if best.as_ref().map(|(_, _, r)| ratio > *r).unwrap_or(true) {
                best = Some((used_month, m, ratio));
            }
        }
    }
    match best {
        Some((used, limit, ratio)) if ratio >= DOWNGRADE_RATIO => SoftStatus::Downgrade {
            used_cny: used,
            limit_cny: limit,
            ratio,
        },
        Some((used, limit, ratio)) if ratio >= WARN_RATIO => SoftStatus::Warn {
            used_cny: used,
            limit_cny: limit,
            ratio,
        },
        _ => SoftStatus::Normal,
    }
}

/// 自动降级候选：同 Provider 中比当前模型更便宜、已启用、支持工具调用的模型。
/// 无候选/已是经济模型时返回 None（调用方保持当前模型，不做破坏性切换）。
pub fn pick_downgrade_model(
    conn: &Connection,
    provider_id: &str,
    current_model: &str,
) -> Option<String> {
    crate::services::model_router::pick_economy_model(conn, provider_id, current_model)
}

/// 已用成本（与 budget.rs 同口径：task_runs.cost_cny 按日/月汇总）
fn used_cost(conn: &Connection, provider_id: Option<&str>) -> (f64, f64) {
    let now = chrono::Local::now();
    let (y, m, d) = (now.year(), now.month(), now.day());
    let day_start = chrono::Local
        .with_ymd_and_hms(y, m, d, 0, 0, 0)
        .single()
        .map(|t| t.timestamp())
        .unwrap_or(0);
    let month_start = chrono::Local
        .with_ymd_and_hms(y, m, 1, 0, 0, 0)
        .single()
        .map(|t| t.timestamp())
        .unwrap_or(0);
    let (daily, monthly) = match provider_id {
        Some(pid) => (
            conn.query_row(
                "SELECT COALESCE(SUM(cost_cny), 0) FROM task_runs
                 WHERE provider_id = ?1 AND created_at >= ?2",
                rusqlite::params![pid, day_start],
                |r| r.get::<_, f64>(0),
            )
            .unwrap_or(0.0),
            conn.query_row(
                "SELECT COALESCE(SUM(cost_cny), 0) FROM task_runs
                 WHERE provider_id = ?1 AND created_at >= ?2",
                rusqlite::params![pid, month_start],
                |r| r.get::<_, f64>(0),
            )
            .unwrap_or(0.0),
        ),
        None => (0.0, 0.0),
    };
    (daily, monthly)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE providers (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                limit_daily_cny REAL,
                limit_monthly_cny REAL
            );
            CREATE TABLE task_runs (
                id TEXT PRIMARY KEY,
                provider_id TEXT,
                created_at INTEGER,
                cost_cny REAL
            );
            CREATE TABLE models (
                model_id TEXT,
                provider_id TEXT,
                enabled INTEGER,
                tool_call INTEGER,
                input_price_per_mtok REAL,
                output_price_per_mtok REAL,
                is_default INTEGER,
                context_limit INTEGER,
                output_limit INTEGER,
                input_modalities TEXT
            );",
        )
        .unwrap();
        conn
    }

    /// 当前时间戳（task_runs.created_at 按本地日/月窗口过滤，必须落在“今天”）
    fn now() -> i64 {
        chrono::Local::now().timestamp()
    }

    #[test]
    fn under_warn_threshold_is_normal() {
        let conn = mem_conn();
        conn.execute(
            "INSERT INTO providers (id, name, limit_daily_cny) VALUES ('p1', 'P1', 100.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_runs (id, provider_id, created_at, cost_cny) VALUES ('r1', 'p1', ?1, 70.0)",
            rusqlite::params![now()],
        )
        .unwrap();
        assert_eq!(soft_check(&conn, "p1"), SoftStatus::Normal);
    }

    #[test]
    fn over_eighty_percent_warns() {
        let conn = mem_conn();
        conn.execute(
            "INSERT INTO providers (id, name, limit_daily_cny) VALUES ('p1', 'P1', 100.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_runs (id, provider_id, created_at, cost_cny) VALUES ('r1', 'p1', ?1, 85.0)",
            rusqlite::params![now()],
        )
        .unwrap();
        match soft_check(&conn, "p1") {
            SoftStatus::Warn { used_cny, limit_cny, ratio } => {
                assert_eq!(used_cny, 85.0);
                assert_eq!(limit_cny, 100.0);
                assert!((ratio - 0.85).abs() < 1e-9);
            }
            other => panic!("应预警，实际 {other:?}"),
        }
    }

    #[test]
    fn over_ninety_percent_downgrades() {
        let conn = mem_conn();
        conn.execute(
            "INSERT INTO providers (id, name, limit_daily_cny) VALUES ('p1', 'P1', 100.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_runs (id, provider_id, created_at, cost_cny) VALUES ('r1', 'p1', ?1, 95.0)",
            rusqlite::params![now()],
        )
        .unwrap();
        let s = soft_check(&conn, "p1");
        assert!(s.should_downgrade(), "应触发降级，实际 {s:?}");
    }

    #[test]
    fn no_limit_is_normal() {
        let conn = mem_conn();
        conn.execute(
            "INSERT INTO providers (id, name) VALUES ('p1', 'P1')",
            [],
        )
        .unwrap();
        assert_eq!(soft_check(&conn, "p1"), SoftStatus::Normal);
    }

    #[test]
    fn downgrade_picks_cheaper_model() {
        let conn = mem_conn();
        conn.execute(
            "INSERT INTO providers (id, name) VALUES ('p1', 'P1')",
            [],
        )
        .unwrap();
        // 主模型贵，候选模型便宜且支持工具
        conn.execute(
            "INSERT INTO models (model_id, provider_id, enabled, tool_call, input_price_per_mtok, output_price_per_mtok, is_default)
             VALUES ('flash', 'p1', 1, 1, 10.0, 30.0, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO models (model_id, provider_id, enabled, tool_call, input_price_per_mtok, output_price_per_mtok, is_default)
             VALUES ('turbo', 'p1', 1, 1, 2.0, 6.0, 0)",
            [],
        )
        .unwrap();
        assert_eq!(
            pick_downgrade_model(&conn, "p1", "flash").as_deref(),
            Some("turbo")
        );
        // 已是经济模型：不再降级
        assert_eq!(pick_downgrade_model(&conn, "p1", "turbo"), None);
    }
}
