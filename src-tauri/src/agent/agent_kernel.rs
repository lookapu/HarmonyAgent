//! UI/headless 共用 Agent Kernel 的协议与预算基础。
//!
//! 本模块不依赖 Tauri、Provider 凭据或具体工具 runtime。协议 adapter 先把响应转换为
//! `KernelTurn`，预算账本再统一累计 usage/cost；消息循环与 UI 流式 adapter 后续逐步接入。

use std::collections::BTreeMap;

use serde_json::Value;

use crate::agent::acceptance::{
    evaluate_contract, remediation_prompt, AcceptanceReport, GoalContract, ToolEvidence,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KernelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub cache_creation_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernelToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug)]
pub struct KernelTurn {
    pub provider_message: Value,
    pub content: String,
    pub tool_calls: Vec<KernelToolCall>,
    pub usage: Option<KernelUsage>,
    pub finish_reason: Option<String>,
}

impl KernelTurn {
    pub fn was_truncated(&self) -> bool {
        self.finish_reason.as_deref() == Some("length")
    }

    pub fn is_stop_candidate(&self) -> bool {
        self.tool_calls.is_empty()
    }
}

/// OpenAI-compatible 非流式响应转为协议无关回合。协议损坏必须失败关闭，不能把缺字段
/// 当作模型正常停止，否则 grader 会收到一个看似有效但实际未运行的 trial。
pub fn parse_openai_turn(response: &Value) -> Result<KernelTurn, String> {
    let choice = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| "Provider 响应缺少 choices[0]".to_string())?;
    let message = choice
        .get("message")
        .filter(|message| message.is_object())
        .cloned()
        .ok_or_else(|| "Provider 响应缺少 message object".to_string())?;
    let content = match message.get("content") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(content)) => content.clone(),
        Some(_) => return Err("Provider message.content 必须是 string 或 null".into()),
    };
    let calls = match message.get("tool_calls") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(calls)) => calls.as_slice(),
        Some(_) => return Err("Provider message.tool_calls 必须是 array".into()),
    };
    let mut tool_calls = Vec::with_capacity(calls.len());
    for (index, call) in calls.iter().enumerate() {
        let id = call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| format!("Provider tool_calls[{index}] 缺少非空 id"))?;
        let function = call
            .get("function")
            .filter(|function| function.is_object())
            .ok_or_else(|| format!("Provider tool_calls[{index}] 缺少 function object"))?;
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| format!("Provider tool_calls[{index}] 缺少非空 function.name"))?;
        let arguments = function
            .get("arguments")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                format!("Provider tool_calls[{index}] 缺少 string function.arguments")
            })?;
        tool_calls.push(KernelToolCall {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        });
    }
    let usage = match response.get("usage") {
        None | Some(Value::Null) => None,
        Some(usage) if usage.is_object() => {
            let input_tokens = usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .ok_or_else(|| "Provider usage.prompt_tokens 必须是非负整数".to_string())?;
            let output_tokens = usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .ok_or_else(|| "Provider usage.completion_tokens 必须是非负整数".to_string())?;
            let cached_tokens = match usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
            {
                None | Some(Value::Null) => 0,
                Some(value) => value.as_u64().ok_or_else(|| {
                    "Provider usage.prompt_tokens_details.cached_tokens 必须是非负整数".to_string()
                })?,
            };
            Some(KernelUsage {
                input_tokens,
                output_tokens,
                cached_tokens,
                cache_creation_tokens: usage
                    .get("cache_creation_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            })
        }
        Some(_) => return Err("Provider usage 必须是 object 或 null".into()),
    };
    Ok(KernelTurn {
        provider_message: message,
        content,
        tool_calls,
        usage,
        finish_reason: choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelAuthScheme {
    Bearer,
    Anthropic,
    Gemini,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KernelReasoningReplay {
    pub substitutions: u32,
    pub replay_chars: u64,
}

/// 不携带 secret 的 Provider 请求计划。API key 只在 transport 最后一刻按 auth_scheme
/// 注入 reqwest，避免请求规划、日志或 eval manifest 意外持有凭据。
#[derive(Clone, Debug)]
pub struct KernelRequestPlan {
    pub url: String,
    pub body: Value,
    pub auth_scheme: KernelAuthScheme,
    pub reasoning_replay: KernelReasoningReplay,
}

#[allow(clippy::too_many_arguments)]
pub fn build_model_request_plan(
    protocol: &str,
    base_url: &str,
    model: &str,
    messages: &[Value],
    native_tools: Option<&[Value]>,
    stream: bool,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    reasoning_effort: Option<&str>,
) -> Result<KernelRequestPlan, String> {
    if base_url.trim().is_empty() || model.trim().is_empty() {
        return Err("Provider base_url/model 不能为空".into());
    }
    if temperature.is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value)) {
        return Err("temperature 必须是 0..=2 的有限数字".into());
    }
    if top_p.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
        return Err("top_p 必须是 0..=1 的有限数字".into());
    }
    if max_tokens == Some(0) {
        return Err("max_tokens 必须大于 0".into());
    }
    let base = base_url.trim_end_matches('/');
    match protocol {
        "anthropic" => {
            let (system, history) = split_system_history(messages);
            let history = strip_reasoning_fields(history);
            let mut body = serde_json::json!({
                "model": model,
                "system": system,
                "messages": history,
            });
            if stream {
                body["stream"] = Value::Bool(true);
            }
            apply_kernel_sampling(
                &mut body,
                "temperature",
                "top_p",
                "max_tokens",
                temperature,
                top_p,
                max_tokens,
            );
            Ok(KernelRequestPlan {
                url: format!("{base}/v1/messages"),
                body,
                auth_scheme: KernelAuthScheme::Anthropic,
                reasoning_replay: KernelReasoningReplay::default(),
            })
        }
        "gemini" => {
            let (system, history) = split_system_history(messages);
            let history = strip_reasoning_fields(history);
            let contents = history
                .iter()
                .map(|message| {
                    serde_json::json!({
                        "role": if message.get("role").and_then(Value::as_str) == Some("assistant") { "model" } else { "user" },
                        "parts": message.get("parts").cloned().unwrap_or_else(|| serde_json::json!([{"text":message.get("content").cloned().unwrap_or(Value::Null)}])),
                    })
                })
                .collect::<Vec<_>>();
            let mut body = serde_json::json!({
                "contents": contents,
                "systemInstruction": {"parts":[{"text":system}]},
            });
            apply_kernel_sampling(
                &mut body,
                "temperature",
                "topP",
                "maxOutputTokens",
                temperature,
                top_p,
                max_tokens,
            );
            let method = if stream {
                "streamGenerateContent?alt=sse"
            } else {
                "generateContent"
            };
            Ok(KernelRequestPlan {
                url: format!("{base}/v1beta/models/{model}:{method}"),
                body,
                auth_scheme: KernelAuthScheme::Gemini,
                reasoning_replay: KernelReasoningReplay::default(),
            })
        }
        _ => {
            let (messages, substitutions, replay_chars) =
                sanitize_thinking_messages(messages, model, reasoning_effort);
            let mut body = serde_json::json!({"model":model,"messages":messages});
            if stream {
                body["stream"] = Value::Bool(true);
            }
            apply_kernel_sampling(
                &mut body,
                "temperature",
                "top_p",
                "max_tokens",
                temperature,
                top_p,
                max_tokens,
            );
            if let Some(tools) = native_tools.filter(|tools| !tools.is_empty()) {
                body["tools"] = Value::Array(tools.to_vec());
                body["tool_choice"] = Value::String("auto".into());
            }
            if reasoning_effort.is_some_and(|value| matches!(value, "low" | "medium" | "high")) {
                body["reasoning_effort"] = Value::String(reasoning_effort.unwrap().into());
            }
            Ok(KernelRequestPlan {
                url: format!("{base}/chat/completions"),
                body,
                auth_scheme: KernelAuthScheme::Bearer,
                reasoning_replay: KernelReasoningReplay {
                    substitutions,
                    replay_chars,
                },
            })
        }
    }
}

fn apply_kernel_sampling(
    body: &mut Value,
    temperature_key: &str,
    top_p_key: &str,
    max_tokens_key: &str,
    temperature: Option<f32>,
    top_p: Option<f32>,
    max_tokens: Option<u32>,
) {
    if let Some(value) = temperature {
        body[temperature_key] = serde_json::json!(value);
    }
    if let Some(value) = top_p {
        body[top_p_key] = serde_json::json!(value);
    }
    if let Some(value) = max_tokens {
        body[max_tokens_key] = serde_json::json!(value);
    }
}

fn split_system_history(messages: &[Value]) -> (String, &[Value]) {
    match messages.split_first() {
        Some((first, rest)) if first.get("role").and_then(Value::as_str) == Some("system") => (
            first
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            rest,
        ),
        _ => (String::new(), messages),
    }
}

fn strip_reasoning_fields(messages: &[Value]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| {
            let mut message = message.clone();
            if let Value::Object(map) = &mut message {
                map.remove("reasoning_content");
            }
            message
        })
        .collect()
}

pub fn requires_reasoning_content(model: &str) -> bool {
    let lower = model.to_lowercase();
    lower.contains("deepseek-v3.2")
        || lower.contains("deepseek-v4")
        || lower.contains("reasoner")
        || lower.contains("-reasoning")
        || lower.contains("-thinking")
        || {
            const PREFIX: &str = "deepseek-r";
            lower.match_indices(PREFIX).any(|(index, _)| {
                lower[index + PREFIX.len()..]
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_ascii_digit())
            })
        }
}

pub fn should_replay_reasoning_content(model: &str, effort: Option<&str>) -> bool {
    let disabled = effort.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "off" | "disabled" | "none" | "false"
        )
    });
    !disabled && requires_reasoning_content(model)
}

pub fn sanitize_thinking_messages(
    messages: &[Value],
    model: &str,
    effort: Option<&str>,
) -> (Vec<Value>, u32, u64) {
    let replay = should_replay_reasoning_content(model, effort);
    let mut substitutions = 0u32;
    let mut replay_chars = 0u64;
    let messages = messages
        .iter()
        .map(|message| {
            let mut message = message.clone();
            if !replay {
                if let Value::Object(map) = &mut message {
                    map.remove("reasoning_content");
                }
            } else if let Value::Object(map) = &mut message {
                let missing = map
                    .get("reasoning_content")
                    .and_then(Value::as_str)
                    .is_none_or(|value| value.trim().is_empty());
                if map.get("role").and_then(Value::as_str) == Some("assistant") && missing {
                    map.insert(
                        "reasoning_content".into(),
                        Value::String("(reasoning omitted)".into()),
                    );
                    substitutions = substitutions.saturating_add(1);
                }
                if let Some(reasoning) = map.get("reasoning_content").and_then(Value::as_str) {
                    replay_chars = replay_chars.saturating_add(reasoning.len() as u64);
                }
            }
            message
        })
        .collect();
    (messages, substitutions, replay_chars)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KernelStreamFinish {
    #[default]
    None,
    Done,
    Truncated,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KernelStreamFrame {
    pub content: Option<String>,
    pub reasoning: Option<String>,
    pub tool_call_deltas: usize,
    pub finish: KernelStreamFinish,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct KernelToolCallFragment {
    id: String,
    name: String,
    arguments: String,
}

/// UI 流式 adapter 的协议归一化状态。网络分块、SSE 行缓冲和前端事件仍由调用方负责；
/// 本类型只处理一帧 JSON 的正文/思考/结束状态、跨帧工具参数和跨首尾帧 usage。
#[derive(Clone, Debug, Default)]
pub struct KernelStreamAccumulator {
    tool_calls: BTreeMap<usize, KernelToolCallFragment>,
    usage: KernelUsage,
    usage_observed: bool,
}

impl KernelStreamAccumulator {
    pub fn ingest(&mut self, protocol: &str, json: &Value) -> KernelStreamFrame {
        let mut frame = KernelStreamFrame {
            content: crate::utils::net::extract_stream_delta(protocol, json),
            reasoning: crate::utils::net::extract_reasoning_delta(protocol, json),
            finish: detect_stream_finish(protocol, json),
            ..KernelStreamFrame::default()
        };
        if protocol != "anthropic" && protocol != "gemini" {
            self.ingest_openai_tool_calls(json, &mut frame);
        }
        self.ingest_usage(protocol, json, &mut frame.warnings);
        frame
    }

    pub fn usage(&self) -> Option<KernelUsage> {
        self.usage_observed.then_some(self.usage)
    }

    pub fn finalized_tool_calls(&self) -> Vec<KernelToolCall> {
        self.tool_calls
            .iter()
            .filter(|(_, call)| !call.name.trim().is_empty())
            .map(|(index, call)| KernelToolCall {
                id: if call.id.is_empty() {
                    format!("stream-call-{index}")
                } else {
                    call.id.clone()
                },
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            })
            .collect()
    }

    fn ingest_openai_tool_calls(&mut self, json: &Value, frame: &mut KernelStreamFrame) {
        let Some(calls) = json
            .pointer("/choices/0/delta/tool_calls")
            .and_then(Value::as_array)
        else {
            return;
        };
        for call in calls {
            let Some(index) = call.get("index").and_then(Value::as_u64) else {
                frame
                    .warnings
                    .push("stream tool_call 缺少非负整数 index".into());
                continue;
            };
            let Ok(index) = usize::try_from(index) else {
                frame
                    .warnings
                    .push("stream tool_call index 超出范围".into());
                continue;
            };
            let fragment = self.tool_calls.entry(index).or_default();
            if let Some(id) = call.get("id").and_then(Value::as_str) {
                if fragment.id.is_empty() {
                    fragment.id.push_str(id);
                }
            }
            if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                fragment.name.push_str(name);
            }
            if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) {
                fragment.arguments.push_str(arguments);
            }
            frame.tool_call_deltas += 1;
        }
    }

    fn ingest_usage(&mut self, protocol: &str, json: &Value, warnings: &mut Vec<String>) {
        match protocol {
            "anthropic" => {
                for usage in [json.get("usage"), json.pointer("/message/usage")]
                    .into_iter()
                    .flatten()
                {
                    merge_stream_usage(
                        "input_tokens",
                        usage.get("input_tokens"),
                        &mut self.usage.input_tokens,
                        &mut self.usage_observed,
                        warnings,
                    );
                    merge_stream_usage(
                        "output_tokens",
                        usage.get("output_tokens"),
                        &mut self.usage.output_tokens,
                        &mut self.usage_observed,
                        warnings,
                    );
                    merge_stream_usage(
                        "cache_read_input_tokens",
                        usage.get("cache_read_input_tokens"),
                        &mut self.usage.cached_tokens,
                        &mut self.usage_observed,
                        warnings,
                    );
                    merge_stream_usage(
                        "cache_creation_input_tokens",
                        usage.get("cache_creation_input_tokens"),
                        &mut self.usage.cache_creation_tokens,
                        &mut self.usage_observed,
                        warnings,
                    );
                }
            }
            "gemini" => {
                let Some(usage) = json.get("usageMetadata") else {
                    return;
                };
                merge_stream_usage(
                    "promptTokenCount",
                    usage.get("promptTokenCount"),
                    &mut self.usage.input_tokens,
                    &mut self.usage_observed,
                    warnings,
                );
                merge_stream_usage(
                    "candidatesTokenCount",
                    usage.get("candidatesTokenCount"),
                    &mut self.usage.output_tokens,
                    &mut self.usage_observed,
                    warnings,
                );
                merge_stream_usage(
                    "cachedContentTokenCount",
                    usage.get("cachedContentTokenCount"),
                    &mut self.usage.cached_tokens,
                    &mut self.usage_observed,
                    warnings,
                );
            }
            _ => {
                let Some(usage) = json.get("usage") else {
                    return;
                };
                merge_stream_usage(
                    "prompt_tokens",
                    usage.get("prompt_tokens"),
                    &mut self.usage.input_tokens,
                    &mut self.usage_observed,
                    warnings,
                );
                merge_stream_usage(
                    "completion_tokens",
                    usage.get("completion_tokens"),
                    &mut self.usage.output_tokens,
                    &mut self.usage_observed,
                    warnings,
                );
                merge_stream_usage(
                    "cached_tokens",
                    usage.pointer("/prompt_tokens_details/cached_tokens"),
                    &mut self.usage.cached_tokens,
                    &mut self.usage_observed,
                    warnings,
                );
            }
        }
    }
}

fn merge_stream_usage(
    label: &str,
    value: Option<&Value>,
    target: &mut u64,
    observed: &mut bool,
    warnings: &mut Vec<String>,
) {
    let Some(value) = value else { return };
    match value.as_u64() {
        Some(value) => {
            *target = (*target).max(value);
            *observed = true;
        }
        None if !value.is_null() => warnings.push(format!("stream usage {label} 必须是非负整数")),
        None => {}
    }
}

pub fn detect_stream_finish(protocol: &str, json: &Value) -> KernelStreamFinish {
    match protocol {
        "anthropic" => match json.get("type").and_then(Value::as_str) {
            Some("message_stop") => KernelStreamFinish::Done,
            Some("message_delta")
                if json.pointer("/delta/stop_reason").and_then(Value::as_str)
                    == Some("max_tokens") =>
            {
                KernelStreamFinish::Truncated
            }
            _ => KernelStreamFinish::None,
        },
        "gemini" => match json
            .pointer("/candidates/0/finishReason")
            .and_then(Value::as_str)
        {
            Some("STOP") => KernelStreamFinish::Done,
            Some("MAX_TOKENS") => KernelStreamFinish::Truncated,
            _ => KernelStreamFinish::None,
        },
        _ => match json
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
        {
            Some("stop") | Some("tool_calls") => KernelStreamFinish::Done,
            Some("length") => KernelStreamFinish::Truncated,
            _ => KernelStreamFinish::None,
        },
    }
}

#[derive(Clone, Debug)]
pub struct KernelUsageLedger {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_cny: f64,
    max_cost_cny: f64,
    input_price_cny_per_1k: Option<f64>,
    output_price_cny_per_1k: Option<f64>,
}

impl KernelUsageLedger {
    pub fn new(
        max_cost_cny: f64,
        input_price_cny_per_1k: Option<f64>,
        output_price_cny_per_1k: Option<f64>,
    ) -> Result<Self, String> {
        if !max_cost_cny.is_finite() || max_cost_cny < 0.0 {
            return Err("max_cost_cny 必须是有限非负数字".into());
        }
        for (name, price) in [
            ("input_price_cny_per_1k", input_price_cny_per_1k),
            ("output_price_cny_per_1k", output_price_cny_per_1k),
        ] {
            if price.is_some_and(|price| !price.is_finite() || price < 0.0) {
                return Err(format!("{name} 必须是有限非负数字"));
            }
        }
        if max_cost_cny > 0.0
            && (input_price_cny_per_1k.is_none() || output_price_cny_per_1k.is_none())
        {
            return Err("任务设置了 max_cost_cny，但缺少输入/输出价格快照".into());
        }
        Ok(Self {
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            cache_creation_tokens: 0,
            cost_cny: 0.0,
            max_cost_cny,
            input_price_cny_per_1k,
            output_price_cny_per_1k,
        })
    }

    /// 返回是否已超过成本上限。设有成本上限时 usage 缺失必须失败关闭。
    pub fn record(&mut self, usage: Option<&KernelUsage>) -> Result<bool, String> {
        let Some(usage) = usage else {
            if self.max_cost_cny > 0.0 {
                return Err("Provider 未返回 usage，无法执行成本硬限制".into());
            }
            return Ok(false);
        };
        self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(usage.output_tokens);
        self.cached_tokens = self.cached_tokens.saturating_add(usage.cached_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(usage.cache_creation_tokens);
        if let (Some(input_price), Some(output_price)) =
            (self.input_price_cny_per_1k, self.output_price_cny_per_1k)
        {
            self.cost_cny += usage.input_tokens as f64 / 1000.0 * input_price
                + usage.output_tokens as f64 / 1000.0 * output_price;
        }
        Ok(self.max_cost_cny > 0.0 && self.cost_cny > self.max_cost_cny)
    }
}

/// 统一内核持有的工具证据。使用 owned 字段，既能跨异步回合保存，也不会把 UI/headless
/// 的具体 ToolRun 类型泄漏到验收模块。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernelToolEvidence {
    pub tool: String,
    pub arguments: String,
    pub output: String,
    pub succeeded: bool,
}

#[derive(Clone, Debug)]
pub enum KernelStopDecision {
    Accepted(AcceptanceReport),
    Remediate {
        report: AcceptanceReport,
        prompt: String,
        round: usize,
    },
    Exhausted(AcceptanceReport),
}

/// 对一次模型停止申请作统一裁决。调用方可以传入普通目标验收报告，也可以传入 UI
/// 聚合子任务 DAG 后的报告；内核只负责一致的有界补救语义。
pub fn decide_stop_candidate(
    report: AcceptanceReport,
    remediation_rounds: &mut usize,
    max_remediation_rounds: usize,
) -> KernelStopDecision {
    if report.passed {
        return KernelStopDecision::Accepted(report);
    }
    if *remediation_rounds >= max_remediation_rounds {
        return KernelStopDecision::Exhausted(report);
    }
    *remediation_rounds = remediation_rounds.saturating_add(1);
    KernelStopDecision::Remediate {
        prompt: remediation_prompt(&report),
        report,
        round: *remediation_rounds,
    }
}

/// 模型只能申请停止；是否真正停止由目标契约和真实工具证据裁决。
///
/// UI 与 headless 使用同一状态机后，benchmark 不会把“模型说完成了”误当成已完成，
/// 同时通过有界 remediation 次数避免弱模型无限自循环。
#[derive(Clone, Debug)]
pub struct KernelAcceptanceGate {
    contract: GoalContract,
    evidence: Vec<KernelToolEvidence>,
    remediation_rounds: usize,
    max_remediation_rounds: usize,
}

impl KernelAcceptanceGate {
    pub fn new(goal: &str, max_remediation_rounds: usize) -> Self {
        Self {
            contract: GoalContract::compile(goal),
            evidence: Vec::new(),
            remediation_rounds: 0,
            max_remediation_rounds,
        }
    }

    pub fn directive(&self) -> String {
        self.contract.directive()
    }

    pub fn record(&mut self, evidence: KernelToolEvidence) {
        self.evidence.push(evidence);
    }

    pub fn report(&self) -> AcceptanceReport {
        let evidence = self
            .evidence
            .iter()
            .map(|item| ToolEvidence {
                tool: &item.tool,
                args: &item.arguments,
                output: &item.output,
                succeeded: item.succeeded,
            })
            .collect::<Vec<_>>();
        evaluate_contract(&self.contract, &evidence)
    }

    pub fn request_stop(&mut self) -> KernelStopDecision {
        let report = self.report();
        decide_stop_candidate(
            report,
            &mut self.remediation_rounds,
            self.max_remediation_rounds,
        )
    }

    pub fn remediation_rounds(&self) -> usize {
        self.remediation_rounds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_turn_and_cached_usage() {
        let turn = parse_openai_turn(&serde_json::json!({
            "choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
              "tool_calls":[{"id":"c1","function":{"name":"read_file","arguments":"{\"path\":\"a\"}"}}]}}],
            "usage":{"prompt_tokens":12,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":5}}
        })).unwrap();
        assert_eq!(turn.tool_calls[0].name, "read_file");
        assert_eq!(turn.usage.as_ref().unwrap().cached_tokens, 5);
        assert!(!turn.is_stop_candidate());
    }

    #[test]
    fn rejects_malformed_provider_tool_calls() {
        let error = parse_openai_turn(&serde_json::json!({
            "choices":[{"message":{"tool_calls":[{"id":"","function":{"name":"read_file"}}]}}]
        }))
        .unwrap_err();
        assert!(error.contains("非空 id"));
    }

    #[test]
    fn rejects_usage_that_could_bypass_cost_accounting() {
        let error = parse_openai_turn(&serde_json::json!({
            "choices":[{"message":{"content":"done"}}],
            "usage":{"prompt_tokens":12}
        }))
        .unwrap_err();
        assert!(error.contains("completion_tokens"));

        let turn = parse_openai_turn(&serde_json::json!({
            "choices":[{"message":{"content":"done"}}],
            "usage":null
        }))
        .unwrap();
        assert!(turn.usage.is_none());
    }

    #[test]
    fn cost_limit_requires_prices_and_usage() {
        assert!(KernelUsageLedger::new(1.0, None, None).is_err());
        assert!(KernelUsageLedger::new(1.0, Some(-1.0), Some(20.0)).is_err());
        let mut ledger = KernelUsageLedger::new(1.0, Some(10.0), Some(20.0)).unwrap();
        assert!(ledger.record(None).unwrap_err().contains("未返回 usage"));
        assert!(ledger
            .record(Some(&KernelUsage {
                input_tokens: 100,
                output_tokens: 1,
                cached_tokens: 0,
                cache_creation_tokens: 0,
            }))
            .unwrap());
    }

    #[test]
    fn stream_accumulator_merges_multiple_parallel_tool_calls() {
        let mut stream = KernelStreamAccumulator::default();
        let first = stream.ingest(
            "openai",
            &serde_json::json!({"choices":[{"delta":{"tool_calls":[
                {"index":1,"id":"c2","function":{"name":"read_","arguments":r#"{"path":"#}},
                {"index":0,"id":"c1","function":{"name":"write_file","arguments":r#"{"path":"a","content":"#}}
            ]}}]}),
        );
        assert_eq!(first.tool_call_deltas, 2);
        let second = stream.ingest(
            "openai",
            &serde_json::json!({"choices":[{"delta":{"tool_calls":[
                {"index":0,"function":{"arguments":r#""x"}"#}},
                {"index":1,"function":{"name":"file","arguments":r#""a"}"#}}
            ]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":20,"completion_tokens":4}}),
        );
        assert_eq!(second.finish, KernelStreamFinish::Done);
        let calls = stream.finalized_tool_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "c1");
        assert_eq!(calls[0].name, "write_file");
        assert_eq!(calls[0].arguments, r#"{"path":"a","content":"x"}"#);
        assert_eq!(calls[1].id, "c2");
        assert_eq!(calls[1].name, "read_file");
        assert_eq!(calls[1].arguments, r#"{"path":"a"}"#);
        assert_eq!(stream.usage().unwrap().input_tokens, 20);
    }

    #[test]
    fn stream_accumulator_merges_anthropic_usage_across_start_and_end() {
        let mut stream = KernelStreamAccumulator::default();
        stream.ingest(
            "anthropic",
            &serde_json::json!({
                "type":"message_start",
                "message":{"usage":{"input_tokens":120,"cache_read_input_tokens":30,"cache_creation_input_tokens":7}}
            }),
        );
        let end = stream.ingest(
            "anthropic",
            &serde_json::json!({
                "type":"message_delta",
                "delta":{"stop_reason":"max_tokens"},
                "usage":{"output_tokens":18}
            }),
        );
        assert_eq!(end.finish, KernelStreamFinish::Truncated);
        assert_eq!(
            stream.usage(),
            Some(KernelUsage {
                input_tokens: 120,
                output_tokens: 18,
                cached_tokens: 30,
                cache_creation_tokens: 7,
            })
        );
    }

    #[test]
    fn stream_accumulator_reads_gemini_finish_and_usage() {
        let mut stream = KernelStreamAccumulator::default();
        let frame = stream.ingest(
            "gemini",
            &serde_json::json!({
                "candidates":[{"finishReason":"STOP"}],
                "usageMetadata":{"promptTokenCount":9,"candidatesTokenCount":3,"cachedContentTokenCount":2}
            }),
        );
        assert_eq!(frame.finish, KernelStreamFinish::Done);
        assert_eq!(stream.usage().unwrap().cached_tokens, 2);
    }

    #[test]
    fn request_plans_cover_protocol_endpoints_auth_and_tools() {
        let messages = vec![
            serde_json::json!({"role":"system","content":"system"}),
            serde_json::json!({"role":"user","content":"hello"}),
        ];
        let tools = vec![serde_json::json!({"type":"function","function":{"name":"read_file"}})];
        let openai = build_model_request_plan(
            "openai",
            "https://api.example/",
            "model-a",
            &messages,
            Some(&tools),
            true,
            Some(2048),
            Some(0.2),
            Some(0.9),
            Some("high"),
        )
        .unwrap();
        assert_eq!(openai.url, "https://api.example/chat/completions");
        assert_eq!(openai.auth_scheme, KernelAuthScheme::Bearer);
        assert_eq!(openai.body["stream"], true);
        assert_eq!(openai.body["tools"][0]["function"]["name"], "read_file");
        assert_eq!(openai.body["reasoning_effort"], "high");

        let anthropic = build_model_request_plan(
            "anthropic",
            "https://api.example",
            "model-b",
            &messages,
            None,
            true,
            Some(1024),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(anthropic.url, "https://api.example/v1/messages");
        assert_eq!(anthropic.auth_scheme, KernelAuthScheme::Anthropic);
        assert_eq!(anthropic.body["system"], "system");
        assert_eq!(anthropic.body["messages"].as_array().unwrap().len(), 1);

        let gemini = build_model_request_plan(
            "gemini",
            "https://api.example",
            "model-c",
            &messages,
            None,
            false,
            Some(512),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            gemini.url,
            "https://api.example/v1beta/models/model-c:generateContent"
        );
        assert_eq!(gemini.auth_scheme, KernelAuthScheme::Gemini);
        assert!(gemini.body.get("stream").is_none());
    }

    #[test]
    fn request_plan_sanitizes_reasoning_without_mutating_history() {
        let messages = vec![
            serde_json::json!({"role":"user","content":"hello"}),
            serde_json::json!({"role":"assistant","content":"answer"}),
            serde_json::json!({"role":"assistant","content":"answer 2","reasoning_content":"trace"}),
        ];
        let plan = build_model_request_plan(
            "openai",
            "https://api.example",
            "deepseek-v4",
            &messages,
            None,
            false,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(plan.reasoning_replay.substitutions, 1);
        assert_eq!(
            plan.body["messages"][1]["reasoning_content"],
            "(reasoning omitted)"
        );
        assert!(messages[1].get("reasoning_content").is_none());

        let ordinary = build_model_request_plan(
            "openai",
            "https://api.example",
            "gpt-test",
            &messages,
            None,
            false,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(ordinary.body["messages"][2]
            .get("reasoning_content")
            .is_none());
    }

    #[test]
    fn request_plan_rejects_invalid_sampling_values() {
        let build = |temperature, top_p, max_tokens| {
            build_model_request_plan(
                "openai",
                "https://api.example",
                "model",
                &[],
                None,
                false,
                max_tokens,
                temperature,
                top_p,
                None,
            )
        };
        assert!(build(Some(f32::NAN), None, None).is_err());
        assert!(build(Some(2.1), None, None).is_err());
        assert!(build(None, Some(-0.1), None).is_err());
        assert!(build(None, None, Some(0)).is_err());
    }

    #[test]
    fn acceptance_gate_requires_post_mutation_verification() {
        let mut gate = KernelAcceptanceGate::new("修改 src/a.rs 并验证", 2);
        gate.record(KernelToolEvidence {
            tool: "write_file".into(),
            arguments: r#"{"path":"src/a.rs","content":"fixed"}"#.into(),
            output: "written".into(),
            succeeded: true,
        });
        assert!(matches!(
            gate.request_stop(),
            KernelStopDecision::Remediate { round: 1, .. }
        ));
        gate.record(KernelToolEvidence {
            tool: "read_file".into(),
            arguments: r#"{"path":"src/a.rs"}"#.into(),
            output: "fixed".into(),
            succeeded: true,
        });
        assert!(matches!(
            gate.request_stop(),
            KernelStopDecision::Accepted(_)
        ));
    }

    #[test]
    fn acceptance_gate_exhaustion_is_bounded() {
        let mut gate = KernelAcceptanceGate::new("修改 a.rs", 1);
        assert!(matches!(
            gate.request_stop(),
            KernelStopDecision::Remediate { round: 1, .. }
        ));
        assert!(matches!(
            gate.request_stop(),
            KernelStopDecision::Exhausted(_)
        ));
        assert_eq!(gate.remediation_rounds(), 1);
    }
}
