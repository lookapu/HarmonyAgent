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
