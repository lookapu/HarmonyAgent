use serde::Deserialize;
use tauri::State;
use uuid::Uuid;

use crate::db::{models::{EndpointDef, Provider}, queries, DbState};
use crate::services::{config_service, key_store};

#[derive(Debug, Deserialize)]
pub struct CreateProviderInput {
    pub name: String,
    pub provider_type: String,
    pub protocol: Option<String>, // openai | anthropic | gemini
    pub base_url: String,
    /// 多协议端点（可选：如 DeepSeek 同时提供 OpenAI 与 Anthropic 端点）
    pub endpoints: Option<Vec<EndpointDef>>,
    pub api_key: Option<String>,
    pub npm_package: Option<String>,
    pub models: Option<Vec<CreateModelInput>>,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateModelInput {
    pub model_id: String,
    pub display_name: Option<String>,
    pub tool_call: Option<bool>,
    pub context_limit: Option<i64>,
    pub output_limit: Option<i64>,
    pub input_modalities: Option<Vec<String>>,
    pub output_modalities: Option<Vec<String>>,
    pub use_proxy: Option<bool>,
}

/// 模型更新（代理开关 / 设为默认 / 启用开关等）
#[derive(Debug, Deserialize)]
pub struct UpdateModelInput {
    pub use_proxy: Option<bool>,
    pub is_default: Option<bool>,
    pub display_name: Option<String>,
    pub context_limit: Option<i64>,
    pub output_limit: Option<i64>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProviderInput {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub npm_package: Option<String>,
    pub notes: Option<String>,
    pub priority: Option<i32>,
    pub protocol: Option<String>, // openai | anthropic | gemini（切换协议时 Base URL 需同步匹配）
    /// 多协议端点（缺省不修改）
    pub endpoints: Option<Vec<EndpointDef>>,
    /// 日预算上限（元）：发送前门控，突破则拦截
    pub limit_daily_cny: Option<f64>,
    /// 月预算上限（元）：发送前门控，突破则拦截
    pub limit_monthly_cny: Option<f64>,
}

#[tauri::command]
pub fn list_providers(db: State<DbState>) -> Result<Vec<Provider>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::list_providers(&conn).map_err(|e| e.to_string())
}

/// 查询 Provider 下已配置的模型
#[tauri::command]
pub fn list_provider_models(db: State<DbState>, provider_id: String) -> Result<Vec<crate::db::models::Model>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::list_models_for_provider(&conn, &provider_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_provider(db: State<DbState>, input: CreateProviderInput) -> Result<Provider, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();

    let provider = Provider {
        id: Uuid::new_v4().to_string(),
        name: input.name,
        provider_type: input.provider_type,
        protocol: input.protocol.unwrap_or_else(|| "openai".to_string()),
        base_url: input.base_url,
        endpoints: input.endpoints.unwrap_or_default(),
        api_key: input.api_key,
        npm_package: input.npm_package,
        is_active: false,
        in_failover_queue: false,
        priority: 0,
        cost_multiplier: 1.0,
        limit_daily_cny: None,
        limit_monthly_cny: None,
        settings_json: "{}".to_string(),
        notes: input.notes,
        icon: None,
        created_at: now,
        updated_at: now,
    };

    queries::insert_provider(&conn, &provider).map_err(|e| e.to_string())?;

    // API Key 安全存储：系统凭据管理器写入成功后数据库置空（Key 只存在凭据库）
    if let Some(ref key) = provider.api_key {
        key_store::save_provider_key(&conn, &provider.id, key).map_err(|e| e.to_string())?;
    }

    // 批量创建默认模型（首个为默认）
    if let Some(models) = input.models {
        for (i, m) in models.into_iter().enumerate() {
            let id = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO models (id, provider_id, model_id, display_name, tool_call,
                        context_limit, output_limit, input_modalities, output_modalities,
                        input_price_per_mtok, output_price_per_mtok, is_default, use_proxy, enabled, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                rusqlite::params![
                    id,
                    provider.id,
                    m.model_id,
                    m.display_name,
                    m.tool_call.unwrap_or(true) as i64,
                    m.context_limit.unwrap_or(200000),
                    m.output_limit.unwrap_or(8192),
                    m.input_modalities
                        .map(|v| serde_json::to_string(&v).unwrap_or_else(|_| "[\"text\"]".into()))
                        .unwrap_or_else(|| "[\"text\"]".into()),
                    m.output_modalities
                        .map(|v| serde_json::to_string(&v).unwrap_or_else(|_| "[\"text\"]".into()))
                        .unwrap_or_else(|| "[\"text\"]".into()),
                    0.0,
                    0.0,
                    (i == 0) as i64,
                    m.use_proxy.unwrap_or(false) as i64,
                    1,
                    now,
                ],
            )
            .map_err(|e| e.to_string())?;
        }
    }

    Ok(provider)
}

/// 更新模型：切换代理开关 / 设为默认 / 展示名等
#[tauri::command]
pub fn update_model(db: State<DbState>, id: String, input: UpdateModelInput) -> Result<crate::db::models::Model, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let model = conn
        .query_row(
            "SELECT id, provider_id, model_id, display_name, tool_call, context_limit,
                    output_limit, input_modalities, output_modalities,
                    input_price_per_mtok, output_price_per_mtok, is_default, use_proxy, enabled, created_at
             FROM models WHERE id = ?1",
            [&id],
            |r| {
                Ok(crate::db::models::Model {
                    id: r.get(0)?,
                    provider_id: r.get(1)?,
                    model_id: r.get(2)?,
                    display_name: r.get(3)?,
                    tool_call: r.get(4)?,
                    context_limit: r.get(5)?,
                    output_limit: r.get(6)?,
                    input_modalities: r.get(7)?,
                    output_modalities: r.get(8)?,
                    input_price_per_mtok: r.get(9)?,
                    output_price_per_mtok: r.get(10)?,
                    is_default: r.get(11)?,
                    use_proxy: r.get(12)?,
                    enabled: r.get(13)?,
                    created_at: r.get(14)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;

    if let Some(use_proxy) = input.use_proxy {
        conn.execute("UPDATE models SET use_proxy = ?1 WHERE id = ?2", rusqlite::params![use_proxy as i64, id])
            .map_err(|e| e.to_string())?;
    }
    if let Some(is_default) = input.is_default {
        if is_default {
            // 同一 Provider 下先清掉旧默认
            conn.execute(
                "UPDATE models SET is_default = 0 WHERE provider_id = ?1",
                [&model.provider_id],
            )
            .map_err(|e| e.to_string())?;
        }
        conn.execute("UPDATE models SET is_default = ?1 WHERE id = ?2", rusqlite::params![is_default as i64, id])
            .map_err(|e| e.to_string())?;
    }
    if let Some(name) = input.display_name {
        conn.execute("UPDATE models SET display_name = ?1 WHERE id = ?2", rusqlite::params![name, id])
            .map_err(|e| e.to_string())?;
    }
    if let Some(ctx) = input.context_limit {
        conn.execute("UPDATE models SET context_limit = ?1 WHERE id = ?2", rusqlite::params![ctx, id])
            .map_err(|e| e.to_string())?;
    }
    if let Some(out) = input.output_limit {
        conn.execute("UPDATE models SET output_limit = ?1 WHERE id = ?2", rusqlite::params![out, id])
            .map_err(|e| e.to_string())?;
    }
    if let Some(en) = input.enabled {
        conn.execute("UPDATE models SET enabled = ?1 WHERE id = ?2", rusqlite::params![en as i64, id])
            .map_err(|e| e.to_string())?;
    }

    let updated = conn
        .query_row(
            "SELECT id, provider_id, model_id, display_name, tool_call, context_limit,
                    output_limit, input_modalities, output_modalities,
                    input_price_per_mtok, output_price_per_mtok, is_default, use_proxy, enabled, created_at
             FROM models WHERE id = ?1",
            [&id],
            |r| {
                Ok(crate::db::models::Model {
                    id: r.get(0)?,
                    provider_id: r.get(1)?,
                    model_id: r.get(2)?,
                    display_name: r.get(3)?,
                    tool_call: r.get(4)?,
                    context_limit: r.get(5)?,
                    output_limit: r.get(6)?,
                    input_modalities: r.get(7)?,
                    output_modalities: r.get(8)?,
                    input_price_per_mtok: r.get(9)?,
                    output_price_per_mtok: r.get(10)?,
                    is_default: r.get(11)?,
                    use_proxy: r.get(12)?,
                    enabled: r.get(13)?,
                    created_at: r.get(14)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(updated)
}

#[tauri::command]
pub fn update_provider(db: State<DbState>, id: String, input: UpdateProviderInput) -> Result<Provider, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    let mut provider = queries::get_provider(&conn, &id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Provider not found".to_string())?;

    if let Some(name) = input.name { provider.name = name; }
    if let Some(url) = input.base_url { provider.base_url = url; }
    // API Key 更新走系统凭据管理器（数据库置空，避免明文落库）
    if let Some(ref key) = input.api_key {
        key_store::save_provider_key(&conn, &id, key).map_err(|e| e.to_string())?;
        provider.api_key = Some(key.clone());
    }
    if let Some(pkg) = input.npm_package { provider.npm_package = Some(pkg); }
    if let Some(notes) = input.notes { provider.notes = Some(notes); }
    if let Some(priority) = input.priority { provider.priority = priority; }
    if let Some(protocol) = input.protocol {
        if protocol != "openai" && protocol != "anthropic" && protocol != "gemini" {
            return Err("protocol 仅支持 openai / anthropic / gemini".into());
        }
        provider.protocol = protocol;
    }
    if let Some(endpoints) = input.endpoints {
        provider.endpoints = endpoints;
    }
    if let Some(d) = input.limit_daily_cny {
        if d < 0.0 {
            return Err("日预算不能为负数".into());
        }
        provider.limit_daily_cny = if d > 0.0 { Some(d) } else { None };
    }
    if let Some(m) = input.limit_monthly_cny {
        if m < 0.0 {
            return Err("月预算不能为负数".into());
        }
        provider.limit_monthly_cny = if m > 0.0 { Some(m) } else { None };
    }
    provider.updated_at = chrono::Utc::now().timestamp();

    queries::update_provider(&conn, &provider).map_err(|e| e.to_string())?;
    Ok(provider)
}

/// 为已存在的 Provider 添加模型（首个自动设为默认）
#[tauri::command]
pub fn add_model(
    db: State<DbState>,
    provider_id: String,
    input: CreateModelInput,
) -> Result<crate::db::models::Model, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();
    let id = Uuid::new_v4().to_string();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM models WHERE provider_id = ?1",
            [&provider_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO models (id, provider_id, model_id, display_name, tool_call,
                context_limit, output_limit, input_modalities, output_modalities,
                input_price_per_mtok, output_price_per_mtok, is_default, use_proxy, enabled, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
        rusqlite::params![
            id,
            provider_id,
            input.model_id,
            input.display_name,
            input.tool_call.unwrap_or(true) as i64,
            input.context_limit.unwrap_or(200000),
            input.output_limit.unwrap_or(8192),
            input.input_modalities
                .map(|v| serde_json::to_string(&v).unwrap_or_else(|_| "[\"text\"]".into()))
                .unwrap_or_else(|| "[\"text\"]".into()),
            input.output_modalities
                .map(|v| serde_json::to_string(&v).unwrap_or_else(|_| "[\"text\"]".into()))
                .unwrap_or_else(|| "[\"text\"]".into()),
            0.0,
            0.0,
            (count == 0) as i64,
            input.use_proxy.unwrap_or(false) as i64,
            1,
            now,
        ],
    )
    .map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT id, provider_id, model_id, display_name, tool_call, context_limit,
                output_limit, input_modalities, output_modalities,
                input_price_per_mtok, output_price_per_mtok, is_default, use_proxy, enabled, created_at
         FROM models WHERE id = ?1",
        [&id],
        |r| {
            Ok(crate::db::models::Model {
                id: r.get(0)?,
                provider_id: r.get(1)?,
                model_id: r.get(2)?,
                display_name: r.get(3)?,
                tool_call: r.get(4)?,
                context_limit: r.get(5)?,
                output_limit: r.get(6)?,
                input_modalities: r.get(7)?,
                output_modalities: r.get(8)?,
                input_price_per_mtok: r.get(9)?,
                output_price_per_mtok: r.get(10)?,
                is_default: r.get(11)?,
                use_proxy: r.get(12)?,
                enabled: r.get(13)?,
                created_at: r.get(14)?,
            })
        },
    )
    .map_err(|e| e.to_string())
}

/// 删除模型
#[tauri::command]
pub fn remove_model(db: State<DbState>, id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM models WHERE id = ?1", [&id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn delete_provider(db: State<DbState>, id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    // 清理系统凭据（Key 可能已迁移到 Windows 凭据管理器）
    key_store::delete_provider_key(&conn, &id).map_err(|e| e.to_string())?;
    queries::delete_provider(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn switch_provider(db: State<DbState>, id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    let provider = queries::get_provider(&conn, &id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Provider not found".to_string())?;

    let models = queries::list_models_for_provider(&conn, &id)
        .map_err(|e| e.to_string())?;

    queries::set_active_provider(&conn, &id).map_err(|e| e.to_string())?;

    // 写配置文件前补全密钥（可能已迁移到系统凭据管理器，DevEco 侧仍需要明文）
    let mut provider = provider;
    if provider.api_key.is_none() {
        provider.api_key = key_store::load_provider_key(&conn, &id).unwrap_or(None);
    }
    config_service::write_provider_to_config(&provider, &models)
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// 测试 Provider 连通性（按协议分派最小请求，用该 Provider 配置的默认模型，避免写死 model ID 造成假失败）
#[tauri::command]
pub async fn test_provider(db: State<'_, DbState>, id: String) -> Result<String, String> {
    let (base_url, api_key, protocol, use_proxy, model) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let row = conn
            .query_row(
                "SELECT base_url, api_key, protocol FROM providers WHERE id = ?1",
                [&id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(|e| e.to_string())?;
        // 跟随默认模型的代理设置
        let use_proxy: bool = conn
            .query_row(
                "SELECT use_proxy FROM models WHERE provider_id = ?1
                 ORDER BY is_default DESC, created_at ASC LIMIT 1",
                [&id],
                |r| r.get(0),
            )
            .unwrap_or(false);
        // 用默认模型 ID 测试（必须真实存在，否则测试无意义）
        let model: Option<String> = conn
            .query_row(
                "SELECT model_id FROM models WHERE provider_id = ?1
                 ORDER BY is_default DESC, created_at ASC LIMIT 1",
                [&id],
                |r| r.get(0),
            )
            .ok();
        (row.0, row.1, row.2, use_proxy, model)
    };
    // 密钥可能已迁移到系统凭据管理器，测试前安全读取补全
    let api_key = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        key_store::load_provider_key(&conn, &id).unwrap_or(api_key)
    };
    let model = model.ok_or_else(|| "请先为该 Provider 添加模型，再执行测试".to_string())?;

    let client = crate::utils::net::build_client(use_proxy)?;
    let start = std::time::Instant::now();
    let base = base_url.trim_end_matches('/');

    let req = match protocol.as_str() {
        "anthropic" => {
            let mut rb = client.post(format!("{base}/v1/messages")).json(&serde_json::json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "hi"}],
            }));
            if let Some(ref key) = api_key {
                rb = rb
                    .header("x-api-key", key)
                    .header("anthropic-version", "2023-06-01");
            }
            rb
        }
        "gemini" => {
            let mut rb = client
                .post(format!("{base}/v1beta/models/{model}:generateContent"))
                .json(&serde_json::json!({
                    "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
                }));
            if let Some(ref key) = api_key {
                rb = rb.header("x-goog-api-key", key);
            }
            rb
        }
        _ => {
            let mut rb = client.post(format!("{base}/chat/completions")).json(&serde_json::json!({
                "model": model,
                "messages": [{"role": "user", "content": "hi"}],
                "max_tokens": 1,
            }));
            if let Some(ref key) = api_key {
                rb = rb.header("Authorization", format!("Bearer {key}"));
            }
            rb
        }
    };

    match req.send().await {
        Ok(resp) => {
            let elapsed = start.elapsed().as_millis();
            Ok(format!("Status: {} | Latency: {}ms", resp.status(), elapsed))
        }
        Err(e) => Err(format!("Connection failed: {e}")),
    }
}
