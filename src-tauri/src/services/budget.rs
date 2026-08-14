//! 成本预估 + 预算门控。
//!
//! 在发送 LLM 请求前，用本地 token 预估估算本次请求成本，结合服务商日/月预算
//! （providers.limit_daily_cny / limit_monthly_cny）与当日/当月已用成本，决定：
//! - 放行（未超预算）
//! - 拒绝（本次请求会突破预算，返回明确原因，避免用户悄悄花超）
//!
//! 口径说明：
//! - "已用成本"来自 task_runs.cost_cny（正式记账口径，含 cache 与 cost_multiplier），
//!   与成本页统计一致；request_logs 是代理路径的旁路记录，不作为门控依据，避免双计。
//! - 预估成本只算本次输入 token × 输入单价 + 预估输出 token × 输出单价，
//!   cache 部分无法事前预知，按 0 处理（低估方向）；门控设计为"接近上限即拦截"，
//!   留出 cache 波动余量，见 estimate 的保守系数。

use chrono::Datelike;
use rusqlite::{params, Connection};

/// 预算门控结果
#[derive(Debug, Clone, PartialEq)]
pub enum GateDecision {
    /// 放行
    Allow,
    /// 拒绝：超日预算
    DailyLimit { used_cny: f64, limit_cny: f64, est_cny: f64 },
    /// 拒绝：超月预算
    MonthlyLimit { used_cny: f64, limit_cny: f64, est_cny: f64 },
}

/// 预算门控详情（供前端展示）
#[derive(Debug, Clone, serde::Serialize)]
pub struct BudgetStatus {
    pub provider_id: Option<String>,
    /// 当日已用成本（元）
    pub used_today_cny: f64,
    /// 当月已用成本（元）
    pub used_month_cny: f64,
    /// 日预算（未设置时为 None）
    pub daily_limit_cny: Option<f64>,
    /// 月预算（未设置时为 None）
    pub monthly_limit_cny: Option<f64>,
}

/// 查询某 Provider 当前预算使用情况（前端成本页/门控前展示）。
pub fn budget_status(conn: &Connection, provider_id: Option<&str>) -> BudgetStatus {
    let (used_today, used_month) = used_cost(conn, provider_id);
    let (daily, monthly) = match provider_id {
        Some(pid) => conn
            .query_row(
                "SELECT limit_daily_cny, limit_monthly_cny FROM providers WHERE id = ?1",
                params![pid],
                |r| Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, Option<f64>>(1)?)),
            )
            .unwrap_or((None, None)),
        None => (None, None),
    };
    BudgetStatus {
        provider_id: provider_id.map(|s| s.to_string()),
        used_today_cny: used_today,
        used_month_cny: used_month,
        daily_limit_cny: daily,
        monthly_limit_cny: monthly,
    }
}

/// 门控主入口：发送前调用。
/// - est_input_tokens / est_output_tokens：本次请求预估 token 数（本地预估）。
/// - provider 未设置任何预算 → 一律放行。
/// - 已用 + 本次预估超过任一限额 → 拒绝并说明超的是日还是月。
pub fn check_budget(
    conn: &Connection,
    provider_id: &str,
    input_price_per_mtok: f64,
    output_price_per_mtok: f64,
    est_input_tokens: usize,
    est_output_tokens: usize,
) -> GateDecision {
    let (daily, monthly) = conn
        .query_row(
            "SELECT limit_daily_cny, limit_monthly_cny FROM providers WHERE id = ?1",
            params![provider_id],
            |r| Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, Option<f64>>(1)?)),
        )
        .unwrap_or((None, None));
    if daily.is_none() && monthly.is_none() {
        return GateDecision::Allow;
    }

    let (used_today, used_month) = used_cost(conn, Some(provider_id));
    // 保守系数：预估本就"宁可高估"，输出侧再乘 1.2 留出波动余地
    let est_cny = (est_input_tokens as f64 * input_price_per_mtok
        + est_output_tokens as f64 * output_price_per_mtok * 1.2)
        / 1_000_000.0;

    if let Some(d) = daily {
        if d > 0.0 && used_today + est_cny > d {
            return GateDecision::DailyLimit { used_cny: used_today, limit_cny: d, est_cny };
        }
    }
    if let Some(m) = monthly {
        if m > 0.0 && used_month + est_cny > m {
            return GateDecision::MonthlyLimit { used_cny: used_month, limit_cny: m, est_cny };
        }
    }
    GateDecision::Allow
}

/// 已用成本（元）：当日 00:00 起与当月 1 号 00:00 起，聚合 task_runs.cost_cny。
/// provider_id 为 None 时统计全部（跨 Provider 总览）。
fn used_cost(conn: &Connection, provider_id: Option<&str>) -> (f64, f64) {
    let (day0, month0) = period_starts();
    let today = if let Some(pid) = provider_id {
        conn.query_row(
            "SELECT COALESCE(SUM(cost_cny), 0) FROM task_runs
             WHERE provider_id = ?1 AND created_at >= ?2",
            params![pid, day0],
            |r| r.get(0),
        )
        .unwrap_or(0.0)
    } else {
        conn.query_row(
            "SELECT COALESCE(SUM(cost_cny), 0) FROM task_runs WHERE created_at >= ?1",
            params![day0],
            |r| r.get(0),
        )
        .unwrap_or(0.0)
    };
    let month = if let Some(pid) = provider_id {
        conn.query_row(
            "SELECT COALESCE(SUM(cost_cny), 0) FROM task_runs
             WHERE provider_id = ?1 AND created_at >= ?2",
            params![pid, month0],
            |r| r.get(0),
        )
        .unwrap_or(0.0)
    } else {
        conn.query_row(
            "SELECT COALESCE(SUM(cost_cny), 0) FROM task_runs WHERE created_at >= ?1",
            params![month0],
            |r| r.get(0),
        )
        .unwrap_or(0.0)
    };
    (today, month)
}

/// 当日 0 点与当月 1 号 0 点的 unix 秒
fn period_starts() -> (i64, i64) {
    let now = chrono::Local::now();
    let day0 = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
    let month0 = now
        .date_naive()
        .with_day(1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp();
    (day0, month0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE providers (
                id TEXT PRIMARY KEY, name TEXT NOT NULL,
                cost_multiplier REAL DEFAULT 1.0,
                limit_daily_cny REAL, limit_monthly_cny REAL
            );
            CREATE TABLE task_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider_id TEXT, created_at INTEGER, cost_cny REAL
            );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn no_budget_means_allow() {
        let conn = mem_conn();
        conn.execute(
            "INSERT INTO providers (id, name) VALUES ('p1', 'P1')",
            [],
        )
        .unwrap();
        let d = check_budget(&conn, "p1", 10.0, 30.0, 1000, 500);
        assert_eq!(d, GateDecision::Allow);
    }

    #[test]
    fn over_daily_limit_is_blocked() {
        let conn = mem_conn();
        conn.execute(
            "INSERT INTO providers (id, name, limit_daily_cny) VALUES ('p1', 'P1', 0.5)",
            [],
        )
        .unwrap();
        // 已用 0.4，本次预估输入 1M token×10元 + 输出 0.1M×30×1.2 ≈ 10+3.6 = 13.6 元
        conn.execute(
            "INSERT INTO task_runs (provider_id, created_at, cost_cny) VALUES ('p1', 0, 0.4)",
            [],
        )
        .unwrap();
        let d = check_budget(&conn, "p1", 10.0, 30.0, 1_000_000, 100_000);
        assert!(matches!(d, GateDecision::DailyLimit { .. }), "应拦截日预算，实际 {d:?}");
    }

    #[test]
    fn under_limits_allows() {
        let conn = mem_conn();
        conn.execute(
            "INSERT INTO providers (id, name, limit_daily_cny, limit_monthly_cny)
             VALUES ('p1', 'P1', 100.0, 500.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_runs (provider_id, created_at, cost_cny) VALUES ('p1', 0, 1.0)",
            [],
        )
        .unwrap();
        let d = check_budget(&conn, "p1", 10.0, 30.0, 1000, 500);
        assert_eq!(d, GateDecision::Allow);
    }
}
