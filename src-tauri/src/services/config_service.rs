use std::path::PathBuf;
use crate::db::models::{Model, Provider};

pub fn get_config_path() -> PathBuf {
    let home = dirs_next().join(".config").join("deveco");
    home.join("deveco.jsonc")
}

fn dirs_next() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("C:\\"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
    }
}

/// 配置文件里一个 provider 条目（导入用；不含应用侧 id）
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedProvider {
    /// `provider` 段里的键，如 `minimax_coding_plan`
    pub key: String,
    pub name: String,
    pub base_url: String,
    /// 由 baseURL 推断：含 `/anthropic` → anthropic，含 `/gemini` → gemini，否则 openai
    pub protocol: String,
    pub api_key: Option<String>,
    pub models: Vec<ImportedModel>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedModel {
    pub model_id: String,
    pub tool_call: bool,
    pub context_limit: i64,
    pub output_limit: i64,
}

/// 从配置文本解析 provider 条目。
///
/// 存在的意义：这份 JSONC 一直是**单向导出**（DB → 文件），于是用户手写/粘贴进去的配置
/// 应用根本看不见——只会觉得"配了没用"。这里把它读回来，让手写配置真的生效。
/// 解析整体宽松：条目缺 name 或 baseURL 就跳过，不影响其它条目与启动。
pub fn parse_providers(config: &serde_json::Value) -> Vec<ImportedProvider> {
    let Some(map) = config.get("provider").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, entry) in map {
        let Some(obj) = entry.as_object() else { continue };
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(key.as_str())
            .to_string();
        let options = obj.get("options").and_then(|v| v.as_object());
        let base_url = options
            .and_then(|o| o.get("baseURL").or_else(|| o.get("baseUrl")))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let Some(base_url) = base_url else { continue };
        let api_key = options
            .and_then(|o| o.get("apiKey"))
            .and_then(|v| v.as_str())
            .map(String::from)
            .filter(|s| !s.trim().is_empty());
        let mut models = Vec::new();
        if let Some(models_obj) = obj.get("models").and_then(|v| v.as_object()) {
            for (model_id, m) in models_obj {
                let limit = m.get("limit");
                models.push(ImportedModel {
                    model_id: model_id.clone(),
                    tool_call: m.get("tool_call").and_then(|v| v.as_bool()).unwrap_or(true),
                    context_limit: limit
                        .and_then(|l| l.get("context"))
                        .and_then(|v| v.as_i64())
                        .unwrap_or(200_000),
                    output_limit: limit
                        .and_then(|l| l.get("output"))
                        .and_then(|v| v.as_i64())
                        .unwrap_or(8_192),
                });
            }
        }
        out.push(ImportedProvider {
            key: key.clone(),
            name,
            protocol: infer_protocol(&base_url).to_string(),
            base_url,
            api_key,
            models,
        });
    }
    out
}

/// 从 baseURL 推断协议：`.../anthropic` 这类端点必须按 anthropic 协议请求。
fn infer_protocol(base_url: &str) -> &'static str {
    let lower = base_url.to_ascii_lowercase();
    if lower.contains("anthropic") {
        "anthropic"
    } else if lower.contains("gemini") || lower.contains("generativelanguage") {
        "gemini"
    } else {
        "openai"
    }
}

pub fn read_deveco_config() -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let path = get_config_path();
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let content = std::fs::read_to_string(&path)?;
    let stripped = strip_jsonc_comments(&content);
    let value: serde_json::Value = serde_json::from_str(&stripped)?;
    Ok(value)
}

pub fn write_deveco_config(config: &serde_json::Value) -> Result<(), Box<dyn std::error::Error>> {
    let path = get_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(config)?;
    std::fs::write(&path, content)?;
    Ok(())
}

pub fn write_provider_to_config(provider: &Provider, models: &[Model]) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = read_deveco_config()?;

    let provider_key = provider.name.to_lowercase().replace(' ', "_");

    let mut models_obj = serde_json::Map::new();
    for m in models {
        let mut model_config = serde_json::Map::new();
        model_config.insert("tool_call".to_string(), serde_json::Value::Bool(m.tool_call));
        model_config.insert("limit".to_string(), serde_json::json!({
            "context": m.context_limit,
            "output": m.output_limit
        }));

        let input_mod: Vec<String> = serde_json::from_str(&m.input_modalities).unwrap_or_else(|_| vec!["text".to_string()]);
        let output_mod: Vec<String> = serde_json::from_str(&m.output_modalities).unwrap_or_else(|_| vec!["text".to_string()]);

        if input_mod != vec!["text"] || output_mod != vec!["text"] {
            model_config.insert("modalities".to_string(), serde_json::json!({
                "input": input_mod,
                "output": output_mod
            }));
        }

        models_obj.insert(m.model_id.clone(), serde_json::Value::Object(model_config));
    }

    let mut provider_config = serde_json::Map::new();
    if let Some(ref npm) = provider.npm_package {
        provider_config.insert("npm".to_string(), serde_json::Value::String(npm.clone()));
    }
    provider_config.insert("name".to_string(), serde_json::Value::String(provider.name.clone()));

    let mut options = serde_json::Map::new();
    options.insert("baseURL".to_string(), serde_json::Value::String(provider.base_url.clone()));
    if let Some(ref key) = provider.api_key {
        options.insert("apiKey".to_string(), serde_json::Value::String(key.clone()));
    }
    provider_config.insert("options".to_string(), serde_json::Value::Object(options));
    provider_config.insert("models".to_string(), serde_json::Value::Object(models_obj));

    let config_obj = config.as_object_mut().ok_or("Config is not an object")?;
    let provider_section = config_obj
        .entry("provider")
        .or_insert_with(|| serde_json::json!({}));

    let provider_map = provider_section.as_object_mut().ok_or("provider section is not an object")?;
    provider_map.clear();
    provider_map.insert(provider_key, serde_json::Value::Object(provider_config));

    config_obj.insert("$schema".to_string(), serde_json::Value::String("https://opencode.ai/config.json".to_string()));

    write_deveco_config(&config)?;
    Ok(())
}

fn strip_jsonc_comments(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escape_next = false;

    while let Some(c) = chars.next() {
        if escape_next {
            result.push(c);
            escape_next = false;
            continue;
        }

        if in_string {
            result.push(c);
            if c == '\\' {
                escape_next = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }

        match c {
            '"' => {
                in_string = true;
                result.push(c);
            }
            '/' => {
                if chars.peek() == Some(&'/') {
                    chars.next();
                    while let Some(&nc) = chars.peek() {
                        if nc == '\n' { break; }
                        chars.next();
                    }
                } else if chars.peek() == Some(&'*') {
                    chars.next();
                    loop {
                        match chars.next() {
                            Some('*') if chars.peek() == Some(&'/') => {
                                chars.next();
                                break;
                            }
                            None => break,
                            _ => {}
                        }
                    }
                    result.push(' ');
                } else {
                    result.push(c);
                }
            }
            _ => result.push(c),
        }
    }

    result
}

/// 导入结果：`imported` 为新建的 provider 名，`skipped` 为已存在（按名字去重）而跳过的。
#[derive(Debug, Default, serde::Serialize)]
pub struct ImportReport {
    pub imported: Vec<String>,
    pub skipped: Vec<String>,
}

/// 把配置文件里的 provider 导入数据库。
///
/// - 按 **name** 去重：同名已存在就跳过，不覆盖用户已有的配置（导入是"补上"，不是"同步覆盖"）。
/// - 密钥走 `key_store`（系统凭据库优先，失败才落库明文），与 UI 创建 provider 完全同一条路径。
pub fn import_providers(conn: &rusqlite::Connection) -> Result<ImportReport, String> {
    let config = read_deveco_config().map_err(|e| e.to_string())?;
    let entries = parse_providers(&config);
    let mut report = ImportReport::default();
    for entry in entries {
        let exists: i64 = conn
            .query_row("SELECT COUNT(*) FROM providers WHERE name = ?1", [&entry.name], |r| r.get(0))
            .unwrap_or(0);
        if exists > 0 {
            report.skipped.push(entry.name);
            continue;
        }
        let now = chrono::Utc::now().timestamp();
        let provider = Provider {
            id: uuid::Uuid::new_v4().to_string(),
            name: entry.name.clone(),
            provider_type: "openai-compatible".to_string(),
            protocol: entry.protocol.clone(),
            base_url: entry.base_url.clone(),
            endpoints: Vec::new(),
            api_key: entry.api_key.clone(),
            npm_package: None,
            is_active: false,
            in_failover_queue: false,
            priority: 0,
            cost_multiplier: 1.0,
            limit_daily_cny: None,
            limit_monthly_cny: None,
            settings_json: "{}".to_string(),
            notes: Some("从配置文件导入".to_string()),
            icon: None,
            auto_pool_mode: 0,
            created_at: now,
            updated_at: now,
        };
        crate::db::queries::insert_provider(conn, &provider).map_err(|e| e.to_string())?;
        if let Some(key) = &entry.api_key {
            crate::services::key_store::save_provider_key(conn, &provider.id, key)?;
        }
        for (i, m) in entry.models.iter().enumerate() {
            conn.execute(
                "INSERT INTO models (id, provider_id, model_id, display_name, tool_call,
                        context_limit, output_limit, input_modalities, output_modalities,
                        input_price_per_mtok, output_price_per_mtok, is_default, use_proxy, enabled, created_at,
                        sort_order)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    provider.id,
                    m.model_id,
                    m.model_id,
                    m.tool_call as i64,
                    m.context_limit,
                    m.output_limit,
                    "[\"text\",\"image\"]",
                    "[\"text\"]",
                    0.0f64,
                    0.0f64,
                    (i == 0) as i64,
                    0i64,
                    1i64,
                    now,
                    i as i64,
                ],
            )
            .map_err(|e| format!("插入模型 {} 失败: {e}", m.model_id))?;
        }
        report.imported.push(entry.name);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> serde_json::Value {
        serde_json::json!({
            "$schema": "https://opencode.ai/config.json",
            "provider": {
                "minimax_coding_plan": {
                    "name": "MiniMax Coding Plan",
                    "options": { "apiKey": "sk-x", "baseURL": "https://api.minimaxi.com/anthropic" },
                    "models": {
                        "MiniMax-M3": {
                            "limit": { "context": 200000, "output": 8192 },
                            "tool_call": true
                        }
                    }
                }
            }
        })
    }

    /// 用户手写/粘贴进配置文件的 provider 必须能被解析出来——否则"配了没用"。
    #[test]
    fn parse_providers_reads_handwritten_entries() {
        let parsed = parse_providers(&sample());
        assert_eq!(parsed.len(), 1);
        let p = &parsed[0];
        assert_eq!(p.key, "minimax_coding_plan");
        assert_eq!(p.name, "MiniMax Coding Plan");
        assert_eq!(p.base_url, "https://api.minimaxi.com/anthropic");
        assert_eq!(p.protocol, "anthropic", "anthropic 端点必须按 anthropic 协议请求");
        assert_eq!(p.api_key.as_deref(), Some("sk-x"));
        assert_eq!(p.models.len(), 1);
        assert_eq!(p.models[0].model_id, "MiniMax-M3");
        assert_eq!(p.models[0].context_limit, 200_000);
        assert_eq!(p.models[0].output_limit, 8_192);
    }

    #[test]
    fn parse_providers_infers_protocol_and_tolerates_junk() {
        let config = serde_json::json!({
            "provider": {
                "a": { "options": { "baseURL": "https://api.openai.com/v1" } },        // 无 name → 用键名
                "b": { "options": { "baseURL": "https://g.example/gemini/v1" } },
                "c": { "name": "缺 baseURL", "options": { "apiKey": "x" } },           // 跳过
                "d": "不是对象"                                                          // 跳过
            }
        });
        let parsed = parse_providers(&config);
        let names: Vec<&str> = parsed.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names.len(), 2, "缺 baseURL 与非对象条目都应跳过：{names:?}");
        assert!(names.contains(&"a"));
        assert_eq!(parsed.iter().find(|p| p.name == "a").unwrap().protocol, "openai");
        assert_eq!(parsed.iter().find(|p| p.name == "b").unwrap().protocol, "gemini");
    }

    #[test]
    fn parse_providers_without_section_is_empty() {
        assert!(parse_providers(&serde_json::json!({})).is_empty());
        assert!(parse_providers(&serde_json::json!({ "provider": 1 })).is_empty());
    }
}
