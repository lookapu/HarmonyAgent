use serde::{Deserialize, Serialize};
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
    /// 输入模态（text/image/audio/video），如 ["text","image"]
    pub input_modalities: Option<Vec<String>>,
    /// 输出模态（text/image/audio/video）
    pub output_modalities: Option<Vec<String>>,
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
                        input_price_per_mtok, output_price_per_mtok, is_default, use_proxy, enabled, created_at,
                        sort_order)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
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
                    i as i64,
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
                    input_price_per_mtok, output_price_per_mtok, is_default, use_proxy, enabled, created_at,
                    sort_order
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
                    sort_order: r.get(15)?,
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
    if let Some(mods) = input.input_modalities {
        let json = serde_json::to_string(&mods).unwrap_or_else(|_| "[\"text\"]".into());
        conn.execute("UPDATE models SET input_modalities = ?1 WHERE id = ?2", rusqlite::params![json, id])
            .map_err(|e| e.to_string())?;
    }
    if let Some(mods) = input.output_modalities {
        let json = serde_json::to_string(&mods).unwrap_or_else(|_| "[\"text\"]".into());
        conn.execute("UPDATE models SET output_modalities = ?1 WHERE id = ?2", rusqlite::params![json, id])
            .map_err(|e| e.to_string())?;
    }

    let updated = conn
        .query_row(
            "SELECT id, provider_id, model_id, display_name, tool_call, context_limit,
                    output_limit, input_modalities, output_modalities,
                    input_price_per_mtok, output_price_per_mtok, is_default, use_proxy, enabled, created_at,
                    sort_order
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
                    sort_order: r.get(15)?,
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
                input_price_per_mtok, output_price_per_mtok, is_default, use_proxy, enabled, created_at,
                sort_order)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
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
            count,
        ],
    )
    .map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT id, provider_id, model_id, display_name, tool_call, context_limit,
                output_limit, input_modalities, output_modalities,
                input_price_per_mtok, output_price_per_mtok, is_default, use_proxy, enabled, created_at,
                sort_order
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
                sort_order: r.get(15)?,
            })
        },
    )
    .map_err(|e| e.to_string())
}

/// 删除模型；若删除的是默认模型，则自动将剩余模型中排序最靠前的顺延为默认（保持"默认置顶"）
#[tauri::command]
pub fn remove_model(db: State<DbState>, id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    // 删除前记录：是否为默认模型 + 所属 Provider
    let (was_default, provider_id): (bool, Option<String>) = conn
        .query_row(
            "SELECT is_default, provider_id FROM models WHERE id = ?1",
            [&id],
            |r| Ok((r.get::<_, i64>(0).map(|v| v != 0)?, r.get::<_, Option<String>>(1)?)),
        )
        .unwrap_or((false, None));
    conn.execute("DELETE FROM models WHERE id = ?1", [&id])
        .map_err(|e| e.to_string())?;
    if was_default {
        if let Some(pid) = provider_id {
            let next_id: Option<String> = conn
                .query_row(
                    "SELECT id FROM models WHERE provider_id = ?1
                     ORDER BY sort_order ASC, created_at ASC, id ASC LIMIT 1",
                    [&pid],
                    |r| r.get(0),
                )
                .ok();
            if let Some(nid) = next_id {
                conn.execute("UPDATE models SET is_default = 1 WHERE id = ?1", [&nid])
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

/// 手动排序模型：ordered_ids 为该 Provider 下模型的新顺序（需包含该 Provider 全部模型 ID）。
/// 仅写 sort_order，默认模型仍由查询层强制置顶。
#[tauri::command]
pub fn reorder_provider_models(
    db: State<DbState>,
    provider_id: String,
    ordered_ids: Vec<String>,
) -> Result<Vec<crate::db::models::Model>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    // 去重校验
    let unique: std::collections::HashSet<&String> = ordered_ids.iter().collect();
    if unique.len() != ordered_ids.len() {
        return Err("ordered_ids 存在重复项".into());
    }
    // 校验全部属于该 Provider
    for id in &ordered_ids {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM models WHERE id = ?1 AND provider_id = ?2",
                rusqlite::params![id, provider_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err(format!("模型 {id} 不属于该 Provider"));
        }
    }
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    for (i, id) in ordered_ids.iter().enumerate() {
        tx.execute(
            "UPDATE models SET sort_order = ?1 WHERE id = ?2 AND provider_id = ?3",
            rusqlite::params![i as i64, id, provider_id],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    queries::list_models_for_provider(&conn, &provider_id).map_err(|e| e.to_string())
}

/// 手动排序 Provider：ordered_ids 为全部 Provider 的新顺序（含当前激活的）。
/// 仅写 priority；当前激活的 Provider 仍由查询层 is_active 强制置顶。
#[tauri::command]
pub fn reorder_providers(
    db: State<DbState>,
    ordered_ids: Vec<String>,
) -> Result<Vec<Provider>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    // 去重校验
    let unique: std::collections::HashSet<&String> = ordered_ids.iter().collect();
    if unique.len() != ordered_ids.len() {
        return Err("ordered_ids 存在重复项".into());
    }
    // 校验全部存在
    for id in &ordered_ids {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM providers WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err(format!("Provider {id} 不存在"));
        }
    }
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    for (i, id) in ordered_ids.iter().enumerate() {
        tx.execute(
            "UPDATE providers SET priority = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![i as i64, chrono::Utc::now().timestamp(), id],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    queries::list_providers(&conn).map_err(|e| e.to_string())
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

/// 远端模型元数据（同步结果）：平台模型列表的展开信息，供前端排序/展示/自动填充
#[derive(Debug, Clone, Serialize)]
pub struct RemoteModelInfo {
    pub id: String,
    /// 上下文窗口（token）；平台未提供时为 0
    pub context_length: i64,
    /// 输入价格（美元/百万 token）
    pub input_price: f64,
    /// 输出价格（美元/百万 token）
    pub output_price: f64,
    /// 免费模型（OpenRouter :free 后缀或输入/输出价格均为 0）
    pub free: bool,
}

/// 模型同步结果：拉取平台当前模型列表后与本地配置对比
#[derive(Debug, Serialize)]
pub struct SyncModelsResult {
    pub provider_id: String,
    /// 平台当前返回的模型列表（含元数据，已按免费优先→价格升序→上下文降序排序）
    pub remote_models: Vec<RemoteModelInfo>,
    /// 本地已配置但平台当前不可用的模型 ID（默认模型等旧配置）
    pub missing: Vec<String>,
    /// 平台当前有、但本地未配置的模型（新增候选，含元数据）
    pub new_models: Vec<RemoteModelInfo>,
    /// 拉取远端模型列表失败时的原因（None=成功）
    pub error: Option<String>,
}

/// 同步 Provider 的模型配置：拉取平台当前模型列表，与本地 models 对比，
/// 返回「已失效（missing）」与「新增（new_models）」，前端据此提示手动移除/添加。
/// 按协议分派（与 test_provider 同口径），跟随默认模型的代理设置。
#[tauri::command]
pub async fn sync_provider_models(db: State<'_, DbState>, id: String) -> Result<SyncModelsResult, String> {
    let (protocol, base_url, use_proxy, api_key, local_models) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let provider = queries::get_provider(&conn, &id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Provider not found".to_string())?;
        let use_proxy: bool = conn
            .query_row(
                "SELECT use_proxy FROM models WHERE provider_id = ?1
                 ORDER BY is_default DESC, created_at ASC LIMIT 1",
                [&id],
                |r| r.get(0),
            )
            .unwrap_or(false);
        let local_models = queries::list_models_for_provider(&conn, &id).map_err(|e| e.to_string())?;
        (
            provider.protocol.clone(),
            provider.base_url.clone(),
            use_proxy,
            provider.api_key,
            local_models,
        )
    };
    // 密钥可能已迁移到系统凭据管理器，同步前安全读取补全
    let api_key = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        key_store::load_provider_key(&conn, &id).unwrap_or(api_key)
    };

    let client = crate::utils::net::build_client(use_proxy)?;
    let base = base_url.trim_end_matches('/');

    // 按协议分派模型列表端点
    let resp_json = {
        let req = match protocol.as_str() {
            "anthropic" => {
                let mut rb = client.get(format!("{base}/v1/models"));
                if let Some(ref key) = api_key {
                    rb = rb
                        .header("x-api-key", key)
                        .header("anthropic-version", "2023-06-01");
                }
                rb
            }
            "gemini" => {
                let mut rb = client.get(format!("{base}/v1beta/models"));
                if let Some(ref key) = api_key {
                    rb = rb.header("x-goog-api-key", key);
                }
                rb
            }
            _ => {
                let mut rb = client.get(format!("{base}/models"));
                if let Some(ref key) = api_key {
                    rb = rb.header("Authorization", format!("Bearer {key}"));
                }
                rb
            }
        };
        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Ok(SyncModelsResult {
                        provider_id: id,
                        remote_models: vec![],
                        missing: local_models.iter().map(|m| m.model_id.clone()).collect(),
                        new_models: vec![],
                        error: Some(format!("HTTP {status}: {text}", text = text.chars().take(200).collect::<String>())),
                    });
                }
                match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(v) => v,
                    Err(_) => {
                        return Ok(SyncModelsResult {
                            provider_id: id,
                            remote_models: vec![],
                            missing: local_models.iter().map(|m| m.model_id.clone()).collect(),
                            new_models: vec![],
                            error: Some("模型列表响应无法解析".to_string()),
                        })
                    }
                }
            }
            Err(e) => {
                return Ok(SyncModelsResult {
                    provider_id: id,
                    remote_models: vec![],
                    missing: local_models.iter().map(|m| m.model_id.clone()).collect(),
                    new_models: vec![],
                    error: Some(format!("连接失败: {e}")),
                })
            }
        }
    };

    // 平台模型元数据提取：
    // - openai 兼容（含 OpenRouter）：data[].id / context_length / pricing.prompt / pricing.completion
    // - anthropic：data[].id（无上下文/价格字段）
    // - gemini：models[].name（去 models/ 前缀）/ inputTokenLimit / outputTokenLimit
    // 免费判断：id 以 :free 结尾（OpenRouter 惯例）或输入/输出价格均为 0
    let parse_price = |v: &serde_json::Value| -> f64 {
        v.as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| v.as_f64())
            .unwrap_or(0.0)
    };
    let mut remote_models: Vec<RemoteModelInfo> = match protocol.as_str() {
        "gemini" => resp_json["models"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let name = m["name"].as_str()?;
                        Some(RemoteModelInfo {
                            id: name.strip_prefix("models/").unwrap_or(name).to_string(),
                            context_length: m["inputTokenLimit"].as_i64().unwrap_or(0),
                            input_price: 0.0,
                            output_price: 0.0,
                            free: false,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => resp_json["data"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let id = m["id"].as_str()?;
                        let pricing = &m["pricing"];
                        let in_p = parse_price(&pricing["prompt"]);
                        let out_p = parse_price(&pricing["completion"]);
                        Some(RemoteModelInfo {
                            id: id.to_string(),
                            context_length: m["context_length"].as_i64().unwrap_or(0),
                            input_price: in_p * 1_000_000.0,
                            output_price: out_p * 1_000_000.0,
                            free: id.ends_with(":free") || (in_p == 0.0 && out_p == 0.0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    };
    // 免费优先 → 合计价格升序 → 上下文窗口降序 → 名称升序（OpenRouter 等大列表更易找免费模型）
    remote_models.sort_by(|a, b| {
        b.free
            .cmp(&a.free)
            .then_with(|| {
                (a.input_price + a.output_price)
                    .partial_cmp(&(b.input_price + b.output_price))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| b.context_length.cmp(&a.context_length))
            .then_with(|| a.id.cmp(&b.id))
    });

    let remote_set: std::collections::HashSet<String> =
        remote_models.iter().map(|m| m.id.clone()).collect();
    let local_set: std::collections::HashSet<String> =
        local_models.iter().map(|m| m.model_id.clone()).collect();

    // missing：本地有、平台无（旧配置/已下线模型）；new_models：平台有、本地无（新增候选，携带元数据）
    let missing: Vec<String> = local_models
        .iter()
        .map(|m| m.model_id.clone())
        .filter(|m| !remote_set.contains(m))
        .collect();
    let new_models: Vec<RemoteModelInfo> = remote_models
        .iter()
        .filter(|m| !local_set.contains(&m.id))
        .cloned()
        .collect();

    Ok(SyncModelsResult {
        provider_id: id,
        remote_models,
        missing,
        new_models,
        error: None,
    })
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
