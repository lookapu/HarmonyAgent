use rusqlite::{params, Connection};

pub struct PricingInfo {
    pub input_cost_per_mtok: f64,
    pub output_cost_per_mtok: f64,
    pub cache_read_cost_per_mtok: f64,
    pub cache_creation_cost_per_mtok: f64,
}

pub fn get_pricing(conn: &Connection, model_id: &str) -> Option<PricingInfo> {
    conn.query_row(
        "SELECT input_cost_per_mtok, output_cost_per_mtok, cache_read_cost_per_mtok, cache_creation_cost_per_mtok
         FROM model_pricing WHERE model_id = ?1",
        params![model_id],
        |row| {
            Ok(PricingInfo {
                input_cost_per_mtok: row.get(0)?,
                output_cost_per_mtok: row.get(1)?,
                cache_read_cost_per_mtok: row.get(2)?,
                cache_creation_cost_per_mtok: row.get(3)?,
            })
        },
    )
    .ok()
}

pub fn calculate_cost(
    pricing: &PricingInfo,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    cost_multiplier: f64,
) -> f64 {
    let input_cost = input_tokens as f64 * pricing.input_cost_per_mtok / 1_000_000.0;
    let output_cost = output_tokens as f64 * pricing.output_cost_per_mtok / 1_000_000.0;
    let cache_read_cost = cache_read_tokens as f64 * pricing.cache_read_cost_per_mtok / 1_000_000.0;
    let cache_creation_cost = cache_creation_tokens as f64 * pricing.cache_creation_cost_per_mtok / 1_000_000.0;

    (input_cost + output_cost + cache_read_cost + cache_creation_cost) * cost_multiplier
}

pub struct UsageInfo {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
}

pub fn extract_usage_from_response(body: &serde_json::Value) -> UsageInfo {
    let usage = body.get("usage").unwrap_or(&serde_json::Value::Null);

    UsageInfo {
        input_tokens: usage.get("prompt_tokens")
            .or_else(|| usage.get("input_tokens"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        output_tokens: usage.get("completion_tokens")
            .or_else(|| usage.get("output_tokens"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        cache_read_tokens: usage.get("cache_read_input_tokens")
            .or_else(|| usage.get("prompt_tokens_details").and_then(|d| d.get("cached_tokens")))
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        cache_creation_tokens: usage.get("cache_creation_input_tokens")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
    }
}

pub fn extract_usage_from_sse_chunks(chunks: &[String]) -> UsageInfo {
    for chunk in chunks.iter().rev() {
        let data = chunk.strip_prefix("data: ").unwrap_or(chunk);
        if data == "[DONE]" {
            continue;
        }
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
            if parsed.get("usage").is_some() {
                return extract_usage_from_response(&parsed);
            }
        }
    }
    UsageInfo {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
    }
}
