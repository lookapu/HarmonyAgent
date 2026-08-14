/// 网络共享工具：系统代理读取、请求客户端构建、SSE 增量提取

/// 读取系统代理地址：环境变量优先，Windows 注册表兜底
pub fn read_system_proxy() -> Option<String> {
    for var in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        if let Ok(v) = std::env::var(var) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        if let Ok(key) = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        {
            let enable: u32 = key.get_value("ProxyEnable").unwrap_or(0);
            if enable == 1 {
                if let Ok(server) = key.get_value::<String, _>("ProxyServer") {
                    let server = server.trim().to_string();
                    if !server.is_empty() {
                        return Some(if server.contains("://") {
                            server
                        } else {
                            format!("http://{server}")
                        });
                    }
                }
            }
        }
    }

    None
}

/// 把系统代理临时写入进程环境变量（供 tauri-plugin-updater 等读环境变量的库使用），
/// 返回被覆盖变量的旧值（未设置的返回 None），供 restore_env_proxy 恢复。
pub fn apply_env_proxy() -> Vec<(String, Option<String>)> {
    let mut saved = Vec::new();
    let Some(proxy) = read_system_proxy() else {
        return saved;
    };
    for var in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        let old = std::env::var(var).ok();
        if old.as_deref() != Some(proxy.as_str()) {
            std::env::set_var(var, &proxy);
            saved.push((var.to_string(), old));
        }
    }
    saved
}

/// 恢复 apply_env_proxy 覆盖前的环境变量
pub fn restore_env_proxy(saved: &[(String, Option<String>)]) {
    for (var, old) in saved {
        match old {
            Some(v) => std::env::set_var(var, v),
            None => std::env::remove_var(var),
        }
    }
}

/// 构建 reqwest 客户端；use_proxy=true 时挂载系统代理。
/// 按 use_proxy 全局复用（连接池/TLS 会话跨对话保留，避免每次对话重复握手）。
pub fn build_client(use_proxy: bool) -> Result<reqwest::Client, String> {
    use std::sync::Mutex;
    static CACHE: Mutex<Option<(bool, reqwest::Client)>> = Mutex::new(None);
    {
        let guard = CACHE.lock().unwrap();
        if let Some((k, c)) = guard.as_ref() {
            if *k == use_proxy {
                return Ok(c.clone());
            }
        }
    }
    // 总超时 120s：流式响应读取也受此约束；超时后由上层指数退避重试（最多 3 次），
    // 避免 Provider 挂起时前端长时间停留在"正在输入…"
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(120));
    if use_proxy {
        if let Some(proxy) = read_system_proxy() {
            builder = builder
                .proxy(reqwest::Proxy::all(proxy).map_err(|e| e.to_string())?)
        }
    }
    let client = builder.build().map_err(|e| e.to_string())?;
    let mut guard = CACHE.lock().unwrap();
    *guard = Some((use_proxy, client.clone()));
    Ok(client)
}

/// 构建 reqwest 客户端（自动代理策略）：检测到系统代理则使用，没有则直连
pub fn build_client_auto() -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36");
    if let Some(proxy) = read_system_proxy() {
        builder = builder.proxy(reqwest::Proxy::all(proxy).map_err(|e| e.to_string())?);
    } else {
        // 无系统代理时显式禁用环境变量代理，保证直连
        builder = builder.no_proxy();
    }
    builder.build().map_err(|e| e.to_string())
}

/// 从 SSE 单条 data JSON 中提取文本增量（按协议分派）
pub fn extract_stream_delta(protocol: &str, json: &serde_json::Value) -> Option<String> {
    match protocol {
        // Anthropic: {"type":"content_block_delta","delta":{"type":"text_delta","text":"..."}}
        "anthropic" => json["delta"]["text"].as_str().map(String::from),
        // Gemini: {"candidates":[{"content":{"parts":[{"text":"..."}]}}]}
        "gemini" => json["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .map(String::from),
        // OpenAI 兼容: {"choices":[{"delta":{"content":"..."}}]}
        _ => json["choices"][0]["delta"]["content"].as_str().map(String::from),
    }
}

/// 从流式 JSON 中提取思考过程增量（DeepSeek reasoning_content / Anthropic thinking / Gemini thought）
pub fn extract_reasoning_delta(protocol: &str, json: &serde_json::Value) -> Option<String> {
    match protocol {
        // Anthropic: {"delta":{"type":"thinking_delta","thinking":"..."}}
        "anthropic" => json["delta"]["thinking"].as_str().map(String::from),
        // Gemini: parts 中 thought=true 的文本块
        "gemini" => json["candidates"][0]["content"]["parts"]
            .as_array()
            .and_then(|parts| {
                parts
                    .iter()
                    .find(|p| p["thought"].as_bool().unwrap_or(false))
                    .and_then(|p| p["text"].as_str().map(String::from))
            }),
        // OpenAI 兼容: {"choices":[{"delta":{"reasoning_content":"..."}}]}（DeepSeek 推理模型）
        _ => json["choices"][0]["delta"]["reasoning_content"]
            .as_str()
            .map(String::from)
            .or_else(|| json["choices"][0]["delta"]["reasoning"].as_str().map(String::from)),
    }
}

/// 从非流式 JSON 响应中提取文本（子 Agent 使用，按协议分派）
pub fn extract_non_stream_text(protocol: &str, json: &serde_json::Value) -> Option<String> {
    match protocol {
        // Anthropic: {"content":[{"type":"text","text":"..."}]}
        "anthropic" => json["content"][0]["text"].as_str().map(String::from),
        // Gemini: {"candidates":[{"content":{"parts":[{"text":"..."}]}}]}
        "gemini" => json["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .map(String::from),
        // OpenAI 兼容: {"choices":[{"message":{"content":"..."}}]}
        _ => json["choices"][0]["message"]["content"].as_str().map(String::from),
    }
}

/// 从 OpenAI 兼容 SSE JSON 中提取原生工具调用增量（function calling）。
/// 返回 (工具序号 index, 名称增量, 参数字符串增量)；无增量时返回 None。
/// 模型流式返回 tool_calls：每个 chunk 携带 index + id/name/arguments 片段，
/// 调用方按 index 累积：name 拼接（罕见跨 chunk）、arguments 按 JSON 片段拼接。
pub fn extract_tool_call_delta(
    json: &serde_json::Value,
) -> Option<(usize, Option<String>, Option<String>)> {
    let call = json["choices"][0]["delta"]["tool_calls"]
        .as_array()?
        .first()?;
    let index = call["index"].as_u64().unwrap_or(0) as usize;
    let name = call["function"]["name"].as_str().map(String::from);
    let args = call["function"]["arguments"].as_str().map(String::from);
    if name.is_none() && args.is_none() {
        return None;
    }
    Some((index, name, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_tool_call_delta_first_chunk_with_name_and_args() {
        let json = serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": { "name": "read_file", "arguments": "{\"path\":\"a" }
                    }]
                }
            }]
        });
        let (idx, name, args) = extract_tool_call_delta(&json).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(name.as_deref(), Some("read_file"));
        assert_eq!(args.as_deref(), Some("{\"path\":\"a"));
    }

    #[test]
    fn extract_tool_call_delta_args_continuation() {
        let json = serde_json::json!({
            "choices": [{ "delta": { "tool_calls": [{ "index": 0, "function": { "arguments": ".txt\"}" } }] } }]
        });
        let (idx, name, args) = extract_tool_call_delta(&json).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(name, None); // 纯参数增量无名称
        assert_eq!(args.as_deref(), Some(".txt\"}"));
    }

    #[test]
    fn extract_tool_call_delta_ignores_plain_text_chunk() {
        let json = serde_json::json!({
            "choices": [{ "delta": { "content": "普通文本" } }]
        });
        assert!(extract_tool_call_delta(&json).is_none());
    }

    #[test]
    fn extract_tool_call_delta_ignores_role_chunk() {
        let json = serde_json::json!({
            "choices": [{ "delta": { "role": "assistant" } }]
        });
        assert!(extract_tool_call_delta(&json).is_none());
    }
}
