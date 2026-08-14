//! 服务商余额/额度查询：
//! 不同服务商查询接口差异较大，这里按 base_url 的 host 自动探测已知端点：
//! - DeepSeek 官方：GET {origin}/user/balance（不带 /v1；balance_infos[] 字段为字符串）
//! - Kimi/Moonshot：GET {origin}/v1/users/me/balance（data.available_balance）
//! - 硅基流动 SiliconFlow：GET {origin}/v1/user/info（data.balance 字符串）
//! - OpenRouter：GET {origin}/api/v1/credits（data.total_credits / total_usage）
//! - one-api / new-api / 各类中转网关：/dashboard/billing/subscription（总额度）+ /dashboard/billing/usage（已用）
//! - 智谱/百炼/火山等未开放余额接口的服务商：返回明确提示
//!
//! 所有请求使用 provider 配置的 api_key（含系统凭据库补全）。
//! use_proxy: None=自动（有系统代理则用）；Some(true)=强制走系统代理；Some(false)=直连。
//! 返回归一化结构：币种、总额度、已用额度、剩余额度；任一字段缺失时为 null。

use serde::Serialize;
use tauri::State;

use crate::db::{queries, DbState};

#[derive(Debug, Serialize)]
pub struct ProviderBalance {
    pub provider_id: String,
    pub provider_name: String,
    /// 查询是否成功（端点可达且返回可解析）
    pub ok: bool,
    /// 原始币种（USD / CNY 等）；无法识别时为 null
    pub currency: Option<String>,
    /// 总额度
    pub total: Option<f64>,
    /// 已用额度
    pub used: Option<f64>,
    /// 剩余额度（total - used，或接口直接返回的 balance）
    pub remaining: Option<f64>,
    /// 是否已用尽 / 余额为 0（仅在能确定时为 true）
    pub exhausted: bool,
    /// 错误信息（ok=false 时）
    pub error: Option<String>,
}

/// 查询所有（或指定）服务商的余额，返回归一化结果列表。
/// use_proxy: None=自动（有系统代理则用）；Some(true)=强制走系统代理；Some(false)=直连。
#[tauri::command]
pub async fn query_balances(
    db: State<'_, DbState>,
    provider_id: Option<String>,
    use_proxy: Option<bool>,
) -> Result<Vec<ProviderBalance>, String> {
    let providers = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let list = queries::list_providers(&conn).map_err(|e| e.to_string())?;
        match provider_id {
            Some(id) => list.into_iter().filter(|p| p.id == id).collect::<Vec<_>>(),
            None => list,
        }
    };

    let client = match use_proxy {
        Some(true) => crate::utils::net::build_client(true)?,
        Some(false) => crate::utils::net::build_client(false)?,
        None => crate::utils::net::build_client_auto()?,
    };
    let mut results = Vec::new();
    for p in providers {
        let api_key = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            p.api_key
                .clone()
                .or_else(|| {
                    crate::services::key_store::load_provider_key(&conn, &p.id)
                        .ok()
                        .flatten()
                })
        };
        let base = p.base_url.trim_end_matches('/').to_string();
        let res = query_one(&client, &base, api_key.as_deref()).await;
        results.push(match res {
            Ok(b) => ProviderBalance {
                provider_id: p.id.clone(),
                provider_name: p.name.clone(),
                ok: true,
                currency: b.currency,
                total: b.total,
                used: b.used,
                remaining: b.remaining,
                exhausted: b.exhausted,
                error: None,
            },
            Err(e) => ProviderBalance {
                provider_id: p.id.clone(),
                provider_name: p.name.clone(),
                ok: false,
                currency: None,
                total: None,
                used: None,
                remaining: None,
                exhausted: false,
                error: Some(e),
            },
        });
    }
    Ok(results)
}

struct BalanceInfo {
    currency: Option<String>,
    total: Option<f64>,
    used: Option<f64>,
    remaining: Option<f64>,
    exhausted: bool,
}

/// 去掉 base_url 尾部的 /api/v1、/v1 与斜杠，得到服务商 origin（部分余额接口固定挂在 origin 下）
fn origin_of(base: &str) -> &str {
    let b = base.trim_end_matches('/');
    let lower = b.to_ascii_lowercase();
    if lower.ends_with("/api/v1") {
        &b[..b.len() - 7]
    } else if lower.ends_with("/v1") {
        &b[..b.len() - 3]
    } else {
        b
    }
}

/// 兼容 JSON 数字与字符串形式的余额数值（DeepSeek / SiliconFlow 返回字符串）
fn num(v: &serde_json::Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

async fn query_one(
    client: &reqwest::Client,
    base: &str,
    api_key: Option<&str>,
) -> Result<BalanceInfo, String> {
    let host = base
        .replace("https://", "")
        .replace("http://", "")
        .to_lowercase();
    let host = host.split('/').next().unwrap_or(&host);

    // DeepSeek 官方
    if host.contains("deepseek.com") {
        return deepseek(client, base, api_key).await;
    }
    // Kimi / Moonshot
    if host.contains("moonshot") {
        return moonshot(client, base, api_key).await;
    }
    // 硅基流动
    if host.contains("siliconflow.cn") {
        return siliconflow(client, base, api_key).await;
    }
    // OpenRouter
    if host.contains("openrouter.ai") {
        return openrouter(client, base, api_key).await;
    }
    // 未开放余额查询接口的服务商：直接给出明确提示，避免走通用端点返回怪异错误
    if host.contains("bigmodel.cn") {
        return Err("智谱 GLM 暂未开放余额查询接口，请到 open.bigmodel.cn 控制台查看".into());
    }
    if host.contains("dashscope.aliyuncs.com") {
        return Err("阿里云百炼暂未开放余额查询接口，请到百炼控制台查看".into());
    }
    if host.contains("volces.com") {
        return Err("火山方舟暂未开放余额查询接口，请到方舟控制台查看".into());
    }
    // one-api / new-api 网关与其他 OpenAI 兼容端点：尝试 billing 接口
    openai_compatible_billing(client, base, api_key).await
}

async fn auth_get(
    client: &reqwest::Client,
    url: &str,
    api_key: Option<&str>,
) -> Result<serde_json::Value, String> {
    let mut req = client.get(url).header("Content-Type", "application/json");
    if let Some(k) = api_key {
        if !k.is_empty() {
            req = req.header("Authorization", format!("Bearer {k}"));
        }
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败: {e}"))?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {}", body.chars().take(200).collect::<String>()));
    }
    serde_json::from_str(&body).map_err(|e| format!("解析响应失败: {e}"))
}

/// one-api/new-api/OpenAI 兼容网关：subscription 返回 hard_limit_usd（总额），usage 返回 total_usage（已用，单位美分）。
async fn openai_compatible_billing(
    client: &reqwest::Client,
    base: &str,
    api_key: Option<&str>,
) -> Result<BalanceInfo, String> {
    let sub_url = format!("{base}/dashboard/billing/subscription");
    let sub = auth_get(client, &sub_url, api_key).await?;

    // 总额度：优先 hard_limit_usd（OpenAI），其次 system_hard_limit_usd
    let total = sub
        .get("hard_limit_usd")
        .or_else(|| sub.get("system_hard_limit_usd"))
        .and_then(|v| v.as_f64());
    if total.is_none() {
        return Err("该服务商未提供余额查询接口".into());
    }

    // 已用额度：/dashboard/billing/usage 的 total_usage（美分）→ 美元
    let usage_url = format!("{base}/dashboard/billing/usage?start_date=2024-01-01&end_date=2099-01-01");
    let used = auth_get(client, &usage_url, api_key)
        .await
        .ok()
        .and_then(|u| u.get("total_usage").and_then(|v| v.as_f64()))
        .map(|cents| cents / 100.0);

    let remaining = match (total, used) {
        (Some(t), Some(u)) => Some((t - u).max(0.0)),
        (Some(t), None) => Some(t),
        _ => None,
    };

    Ok(BalanceInfo {
        currency: Some("USD".into()),
        total,
        used,
        remaining,
        exhausted: remaining.map(|r| r <= 0.0).unwrap_or(false),
    })
}

/// DeepSeek：GET {origin}/user/balance（余额接口不带 /v1）
/// → { is_available, balance_infos: [{ currency, total_balance, granted_balance, topped_up_balance }] }
/// 注意：balance_infos 里的数值是字符串形式（如 "110.00"）
async fn deepseek(
    client: &reqwest::Client,
    base: &str,
    api_key: Option<&str>,
) -> Result<BalanceInfo, String> {
    let url = format!("{}/user/balance", origin_of(base));
    let v = auth_get(client, &url, api_key).await?;
    if v.get("is_available").and_then(|x| x.as_bool()) == Some(false) {
        return Err("账户余额不可用（is_available=false）".into());
    }
    let infos = v
        .get("balance_infos")
        .and_then(|x| x.as_array())
        .ok_or_else(|| "返回格式异常：缺少 balance_infos".to_string())?;
    // 取第一项（DeepSeek 通常只返回 CNY）
    let first = infos
        .first()
        .ok_or_else(|| "balance_infos 为空".to_string())?;
    let total = first.get("total_balance").and_then(num);
    let granted = first.get("granted_balance").and_then(num);
    let topped = first.get("topped_up_balance").and_then(num);
    let currency = first
        .get("currency")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    Ok(BalanceInfo {
        currency,
        // 总额度 = 赠金 + 充值；剩余 = total_balance（当前可用）
        total: match (granted, topped) {
            (Some(g), Some(t)) => Some(g + t),
            _ => None,
        },
        used: None,
        remaining: total,
        exhausted: total.map(|r| r <= 0.0).unwrap_or(false),
    })
}

/// Kimi / Moonshot：GET {origin}/v1/users/me/balance
/// → data.{ available_balance, voucher_balance, cash_balance }（float，CNY）
async fn moonshot(
    client: &reqwest::Client,
    base: &str,
    api_key: Option<&str>,
) -> Result<BalanceInfo, String> {
    let url = format!("{}/v1/users/me/balance", origin_of(base));
    let v = auth_get(client, &url, api_key).await?;
    let data = v
        .get("data")
        .ok_or_else(|| "返回格式异常：缺少 data".to_string())?;
    let available = data.get("available_balance").and_then(num);
    let voucher = data.get("voucher_balance").and_then(num);
    let cash = data.get("cash_balance").and_then(num);
    Ok(BalanceInfo {
        currency: Some("CNY".into()),
        total: match (voucher, cash) {
            (Some(g), Some(c)) => Some(g + c),
            _ => None,
        },
        used: None,
        remaining: available,
        exhausted: available.map(|r| r <= 0.0).unwrap_or(false),
    })
}

/// 硅基流动：GET {origin}/v1/user/info → { data: { balance, chargeBalance, totalBalance, ... } }
/// 注意：数值是字符串形式（如 "15.996"），单位 CNY
async fn siliconflow(
    client: &reqwest::Client,
    base: &str,
    api_key: Option<&str>,
) -> Result<BalanceInfo, String> {
    let url = format!("{}/v1/user/info", origin_of(base));
    let v = auth_get(client, &url, api_key).await?;
    let data = v
        .get("data")
        .ok_or_else(|| "返回格式异常：缺少 data".to_string())?;
    let total = data
        .get("totalBalance")
        .or_else(|| data.get("balance"))
        .and_then(num);
    let remaining = data.get("balance").and_then(num).or(total);
    Ok(BalanceInfo {
        currency: Some("CNY".into()),
        total,
        used: None,
        remaining,
        exhausted: remaining.map(|r| r <= 0.0).unwrap_or(false),
    })
}

/// OpenRouter：GET {origin}/api/v1/credits → data.{ total_credits, total_usage }（USD）
async fn openrouter(
    client: &reqwest::Client,
    base: &str,
    api_key: Option<&str>,
) -> Result<BalanceInfo, String> {
    let url = format!("{}/api/v1/credits", origin_of(base));
    let v = auth_get(client, &url, api_key).await?;
    let data = v
        .get("data")
        .ok_or_else(|| "返回格式异常：缺少 data".to_string())?;
    let total = data.get("total_credits").and_then(num);
    let used = data.get("total_usage").and_then(num);
    let remaining = match (total, used) {
        (Some(t), Some(u)) => Some((t - u).max(0.0)),
        (Some(t), None) => Some(t),
        _ => None,
    };
    Ok(BalanceInfo {
        currency: Some("USD".into()),
        total,
        used,
        remaining,
        exhausted: remaining.map(|r| r <= 0.0).unwrap_or(false),
    })
}
