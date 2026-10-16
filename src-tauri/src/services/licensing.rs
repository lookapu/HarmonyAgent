//! 授权分级模型（P2-2）。
//!
//! ## 这一层到底管什么
//!
//! §2 原文：「免费版（本地全功能，限 Agent 预算）/ Pro（多 Provider 池、团队共享、
//! 评测报告）/ Team（LAN 协作 + 审计）。**先做用量计量与配额展示，再谈付费**」。
//!
//! 「先做计量」那条其实**已经做完了**（`CostPage` 已有成本汇总 / 日用量 / 任务统计 /
//! 预算状态，`agent_quota_usage` 有真实生产写入）。所以本模块补的是**缺的那一半**：
//! 分级本身。
//!
//! ## 三条设计红线
//!
//! 1. **不阉割功能，只分档**。免费版是「本地全功能」——本地会话、本地构建、
//!    本地部署一律可用。分级限制的只有**协作类**能力（团队共享 / 评测报告 /
//!    LAN / 审计）与**配额**。理由：本地优先产品一旦在核心路径上阉割，
//!    用户会直接去用别的工具，而商业化还没赚到钱就先丢了用户。
//! 2. **不接支付**（项目红线：支付功能暂缓）。档位存在本地 `settings` 表里，
//!    可由用户自行切换——这是**模型**不是**收费**。生产环境要把
//!    `Tier::from_settings` 换成服务端签发，其余代码不用动。
//! 3. **权益矩阵是单一真源**。所有「某能力属于哪档」的判断只在这里发生。
//!    command 层一律调 `check()`，**不允许**自己写 `if tier == ...`。
//!    门禁 `check-entitlement-gate.py` 守的就是这条。
//!
//! ## 为什么免费版要限 Agent 预算
//!
//! Agent 运行是**唯一真正花钱**的动作（模型调用）。本地其它能力（构建、部署、
//! 索引）不产生外部成本。所以配额只挂在两处：
//! - `monthly_budget_cny`：月度成本上限（免费版有，Pro/Team 为 `None` = 不限）
//! - `monthly_runs` / `monthly_tool_calls`：运行与工具调用次数上限
//!
//! 配额数据来自 `agent_quota_usage`（`enterprise::quota`），**不另建一套计数**——
//! 另建一份必然与真实用量分叉，而分叉的配额系统会放过超量用户。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::agent::enterprise;

/// 档位。**序列化值即 `settings` 表里存的值**，改它等于换了一种历史数据格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Free,
    Pro,
    Team,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::Free, Tier::Pro, Tier::Team];

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Free => "free",
            Tier::Pro => "pro",
            Tier::Team => "team",
        }
    }

    pub fn parse(raw: &str) -> Option<Tier> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "free" => Some(Tier::Free),
            "pro" => Some(Tier::Pro),
            "team" => Some(Tier::Team),
            _ => None,
        }
    }
}

/// 能力键。新增能力时**必须**同时更新 `TIER_ENTITLEMENTS`——
/// 门禁会核对「枚举项是否都在矩阵里出现过」，否则新能力会静默对所有档位关闭。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// 多 Provider 自动池（多 Provider 轮转 / 故障转移）
    MultiProviderPool,
    /// 团队共享包（导出/导入记忆、配置、评测集）
    TeamSharing,
    /// 评测报告
    EvalReports,
    /// 局域网协作（手机/平板/其他电脑接入）
    LanCollaboration,
    /// 审计事件（谁在什么时候动了什么）
    AuditTrail,
}

/// 权益矩阵。**这是「哪档有什么」的唯一真源。**
///
/// 形状是 `[档位][能力] = 最低可用档位`——比逐档列举更抗改：
/// 加一列新能力时只有一行要动，漏填会表现为「所有档位都不可用」
/// （fail-closed，安全的那个方向），而逐档列举的漏填是「所有档位都可用」。
pub const TIER_ENTITLEMENTS: [(Tier, &[Capability]); 3] = [
    (Tier::Free, &[]),
    (
        Tier::Pro,
        &[
            Capability::MultiProviderPool,
            Capability::TeamSharing,
            Capability::EvalReports,
        ],
    ),
    (
        Tier::Team,
        &[
            Capability::MultiProviderPool,
            Capability::TeamSharing,
            Capability::EvalReports,
            Capability::LanCollaboration,
        Capability::AuditTrail,
            ],
    ),
];

/// 单档位的完整描述，喂给前端的权益对比表。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TierInfo {
    pub tier: Tier,
    pub capabilities: Vec<Capability>,
    /// `None` = 不限。免费版的月度上限是**产品决策的默认值**，
    /// 不是从用量推出来的——它回答的是「免费版一个月值多少钱」。
    pub monthly_budget_cny: Option<f64>,
    pub monthly_runs: Option<i64>,
    pub monthly_tool_calls: Option<i64>,
}

/// 配额当前用量。**字段来自 `agent_quota_usage`，本模块不重新计数。**
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    pub period: String,
    pub runs: i64,
    pub tool_calls: i64,
    pub cost_cny: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LicenseState {
    pub tier: Tier,
    /// 数据来源。`local` = 用户在本地切换；生产应为 `server`。
    /// 前端据此决定要不要显示「这是你自己切的，不是买来的」。
    pub issued_by: String,
    pub quota: QuotaSnapshot,
    /// 已超额的项。**非空时前端必须在权益区顶部显示**，不能只藏在角落。
    pub exceeded: Vec<QuotaBreach>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuotaBreach {
    /// `monthly_budget_cny` / `monthly_runs` / `monthly_tool_calls`
    pub limit: String,
    pub used: f64,
    pub cap: f64,
}

const SETTINGS_KEY: &str = "license_tier";
const DEFAULT_TIER: Tier = Tier::Free;

fn entitlements_of(tier: Tier) -> &'static [Capability] {
    TIER_ENTITLEMENTS
        .iter()
        .find(|(candidate, _)| *candidate == tier)
        .map(|(_, caps)| *caps)
        .unwrap_or(&[])
}

/// 某档位是否含某能力。**不读库**——纯函数，便于在无连接的场景下用（比如测试、
/// 静态检查、以及前端拿到的 `capabilities` 列表之外的场合）。
pub fn allows(tier: Tier, capability: Capability) -> bool {
    entitlements_of(tier).contains(&capability)
}

pub fn tier_info(tier: Tier) -> TierInfo {
    let (budget, runs, tool_calls) = match tier {
        // 免费版：本地全功能，唯一的约束是 Agent 预算与次数
        Tier::Free => (Some(50.0), Some(200), Some(2_000)),
        // Pro / Team：不设本地上限。本地分档模型管不了真实成本，
        // 上限应由服务端按订阅计费——写死一个数只会给人「已经限过了」的错觉。
        Tier::Pro | Tier::Team => (None, None, None),
    };
    TierInfo {
        tier,
        capabilities: entitlements_of(tier).to_vec(),
        monthly_budget_cny: budget,
        monthly_runs: runs,
        monthly_tool_calls: tool_calls,
    }
}

/// 读当前档位。**读不到 / 值不认识一律回落到免费版**——fail-closed。
/// 反过来（读不到就给 Team）是不可接受的：那是「配置损坏 = 白送 Pro」。
pub fn current_tier(conn: &Connection) -> Tier {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        [SETTINGS_KEY],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|raw| Tier::parse(&raw))
    .unwrap_or(DEFAULT_TIER)
}

pub fn set_tier(conn: &Connection, tier: Tier) -> Result<(), String> {
    conn.execute(
        "INSERT INTO settings(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![SETTINGS_KEY, tier.as_str()],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn state(conn: &Connection) -> LicenseState {
    let tier = current_tier(conn);
    let info = tier_info(tier);
    let usage = enterprise::quota(conn).unwrap_or_default();
    let quota = QuotaSnapshot {
        period: usage.period,
        runs: usage.runs,
        tool_calls: usage.tool_calls,
        cost_cny: usage.cost_cny,
    };
    LicenseState {
        tier,
        issued_by: "local".into(),
        exceeded: breaches(&info, &quota),
        quota,
        updated_at: chrono::Utc::now().timestamp_millis(),
    }
}

fn breaches(info: &TierInfo, quota: &QuotaSnapshot) -> Vec<QuotaBreach> {
    let mut out = Vec::new();
    let mut check = |limit: &str, used: f64, cap: Option<f64>| {
        if let Some(cap) = cap {
            if used > cap {
                out.push(QuotaBreach {
                    limit: limit.into(),
                    used,
                    cap,
                });
            }
        }
    };
    check("monthly_budget_cny", quota.cost_cny, info.monthly_budget_cny);
    check("monthly_runs", quota.runs as f64, info.monthly_runs.map(|v| v as f64));
    check(
        "monthly_tool_calls",
        quota.tool_calls as f64,
        info.monthly_tool_calls.map(|v| v as f64),
    );
    out
}

/// **能力网关**。command 层唯一的入口。
///
/// 返回值刻意是 `String` 而不是自定义错误类型：前端已经有一套
/// `invokeWithError` 的错误展示路径，再造一种错误形状只会多一处需要同步的地方。
/// 错误文案带能力键，是为了让客服在用户截图里能直接看到卡在哪一项。
pub fn check(conn: &Connection, capability: Capability) -> Result<(), String> {
    let tier = current_tier(conn);
    if allows(tier, capability) {
        return Ok(());
    }
    Err(format!(
        "该功能需要更高档位（当前：{}，需要：{}）",
        tier.as_str(),
        required_tier_label(capability)
    ))
}

fn required_tier_label(capability: Capability) -> &'static str {
    TIER_ENTITLEMENTS
        .iter()
        .find(|(_, caps)| caps.contains(&capability))
        .map(|(tier, _)| tier.as_str())
        .unwrap_or("unknown")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_tier_has_no_collaboration_capability() {
        // 红线 1：免费版是「本地全功能」，所以这里断言的是**协作类**能力全关，
        // 而不是「全部能力都关」。后者会让「免费版=残废版」。
        for capability in [
            Capability::MultiProviderPool,
            Capability::TeamSharing,
            Capability::EvalReports,
            Capability::LanCollaboration,
            Capability::AuditTrail,
        ] {
            assert!(!allows(Tier::Free, capability), "{capability:?} 不该在免费版");
        }
    }

    #[test]
    fn tiers_are_monotonic() {
        // Team 的能力集合必须是 Pro 的超集、Pro 的是 Free 的超集。
        // 分级模型一旦非单调，「Pro 比免费多什么」这种问题就没有确定答案了。
        for capability in [
            Capability::MultiProviderPool,
            Capability::TeamSharing,
            Capability::EvalReports,
        ] {
            assert!(allows(Tier::Pro, capability));
            assert!(allows(Tier::Team, capability));
        }
        for capability in [Capability::LanCollaboration, Capability::AuditTrail] {
            assert!(!allows(Tier::Pro, capability));
            assert!(allows(Tier::Team, capability));
        }
    }

    #[test]
    fn every_capability_is_listed_somewhere() {
        // 枚举加了成员但矩阵忘了填 → 该能力对所有档位关闭，且**不报任何错**。
        for capability in [
            Capability::MultiProviderPool,
            Capability::TeamSharing,
            Capability::EvalReports,
            Capability::LanCollaboration,
            Capability::AuditTrail,
        ] {
            let listed = TIER_ENTITLEMENTS.iter().any(|(_, caps)| caps.contains(&capability));
            assert!(listed, "{capability:?} 没进任何档位的权益矩阵");
        }
    }

    #[test]
    fn unknown_setting_falls_back_to_free() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        // 表里有值但不认识
        conn.execute("INSERT INTO settings VALUES('license_tier','enterprise')", [])
            .unwrap();
        assert_eq!(current_tier(&conn), Tier::Free);
        // 表是空的
        conn.execute("DELETE FROM settings", []).unwrap();
        assert_eq!(current_tier(&conn), Tier::Free);
    }

    #[test]
    fn missing_settings_table_falls_back_to_free() {
        // 残缺库（老版本 / 测试）不能让读档位直接抛错——那会让整个应用打不开。
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(current_tier(&conn), Tier::Free);
    }

    #[test]
    fn set_then_read_roundtrips() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        for tier in Tier::ALL {
            set_tier(&conn, tier).unwrap();
            assert_eq!(current_tier(&conn), tier);
        }
    }

    #[test]
    fn check_blocks_collaboration_on_free_and_allows_on_team() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        let error = check(&conn, Capability::LanCollaboration).unwrap_err();
        assert!(error.contains("free"), "{error}");
        assert!(error.contains("team"), "错误要说明需要哪一档：{error}");

        set_tier(&conn, Tier::Team).unwrap();
        assert!(check(&conn, Capability::LanCollaboration).is_ok());
    }

    #[test]
    fn quota_breach_is_reported_not_silently_dropped() {
        // 配额超了必须能被看见。返回 `Ok` 但额度已爆，等于让用户以为还能用。
        let free = tier_info(Tier::Free);
        let over = QuotaSnapshot {
            period: "2026-10".into(),
            runs: 5,
            tool_calls: 5,
            cost_cny: 999.0,
        };
        let found = breaches(&free, &over);
        assert!(found.iter().any(|b| b.limit == "monthly_budget_cny"), "{found:?}");

        let under = QuotaSnapshot {
            period: "2026-10".into(),
            runs: 1,
            tool_calls: 1,
            cost_cny: 1.0,
        };
        assert!(breaches(&free, &under).is_empty());
    }

    #[test]
    fn paid_tiers_have_no_local_cap() {
        // Pro/Team 在本地分档模型里不设上限——真实成本由服务端按订阅管。
        // 写死一个数只会给人「已经限过了」的错觉，而那比不限更糟。
        for tier in [Tier::Pro, Tier::Team] {
            let info = tier_info(tier);
            assert!(info.monthly_budget_cny.is_none(), "{tier:?} 不该有本地预算上限");
            assert!(info.monthly_runs.is_none(), "{tier:?} 不该有本地次数上限");
        }
    }

    #[test]
    fn tier_parse_rejects_unknown_values() {
        assert_eq!(Tier::parse("PRO"), Some(Tier::Pro));
        assert_eq!(Tier::parse("  team "), Some(Tier::Team));
        assert_eq!(Tier::parse("enterprise"), None);
        assert_eq!(Tier::parse(""), None);
    }

    /// 降级后必须还能读到自己已被授予的权益。
    ///
    /// 这一条盯的是「关掉」方向：Pro 用户降级到免费后，**关闭**多池必须还能执行。
    /// 若关也要 Pro，他就被永久卡在开着的多池上，只能改数据库自救。
    /// 网关天然只管「进」不管「出」，所以这个方向要单独钉一条测试，
    /// 否则它只靠代码注释里的自觉。
    #[test]
    fn downgrade_still_allows_reading_state_and_clearing_settings() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE agent_quota_usage(tenant_id TEXT,period TEXT,runs INTEGER,tool_calls INTEGER,
                 failed_tools INTEGER,duration_ms INTEGER,cost_cny REAL,updated_at INTEGER,
                 PRIMARY KEY(tenant_id,period));",
        )
        .unwrap();
        set_tier(&conn, Tier::Pro).unwrap();
        assert!(check(&conn, Capability::MultiProviderPool).is_ok());

        set_tier(&conn, Tier::Free).unwrap();
        // 进：被拦
        assert!(check(&conn, Capability::MultiProviderPool).is_err());
        // 出：仍能读状态、仍能改回别的档位
        let snapshot = state(&conn);
        assert_eq!(snapshot.tier, Tier::Free);
        set_tier(&conn, Tier::Team).unwrap();
        assert_eq!(current_tier(&conn), Tier::Team);
    }
}
