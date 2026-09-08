//! UI/headless 共用 Agent Kernel 的协议与预算基础。
//!
//! 本模块不依赖 Tauri、Provider 凭据或具体工具 runtime。协议 adapter 先把响应转换为
//! `KernelTurn`，预算账本再统一累计 usage/cost；消息循环与 UI 流式 adapter 后续逐步接入。

use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use serde_json::Value;
use tokio::time::Instant;

use crate::agent::acceptance::{
    evaluate_contract, remediation_prompt, AcceptanceReport, GoalContract, ToolEvidence,
};
use crate::utils::retry::{retry_with_backoff, RetryPolicy, RetryResult};

/// Provider 请求在真正返回响应前可能停留在 DNS/TCP/TLS、首字节等待或重试退避。
/// UI 与 headless 通过同一控制层轮询取消和绝对截止时间，避免各自维护一套略有差异的
/// `select!`。具体 HTTP client、鉴权和错误类型仍由 adapter 注入。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelTransportStop {
    Cancelled,
    DeadlineExceeded,
}

/// 在统一的重试策略外包裹取消/截止时间控制。
///
/// `attempt` 只负责一次 Provider 交互；可恢复性和 Retry-After 由调用方的结构化错误提供。
/// 丢弃该 future 会同时丢弃当前 HTTP send/read 或退避 sleep，因此取消不需要等待一次
/// 请求自然返回。`on_poll` 用于 UI watchdog touch；headless 可传空闭包。
#[allow(clippy::too_many_arguments)]
pub async fn run_provider_transport<T, E, F, Fut, Retryable, RetryAfter, Cancelled, Poll>(
    policy: &RetryPolicy,
    deadline: Option<tokio::time::Instant>,
    poll_interval: Duration,
    attempt: &mut F,
    should_retry: Retryable,
    retry_after_of: RetryAfter,
    mut is_cancelled: Cancelled,
    mut on_poll: Poll,
) -> Result<RetryResult<T, E>, KernelTransportStop>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    Retryable: Fn(&E) -> bool,
    RetryAfter: Fn(&E) -> Option<u64>,
    Cancelled: FnMut() -> bool,
    Poll: FnMut(),
{
    if is_cancelled() {
        return Err(KernelTransportStop::Cancelled);
    }
    if deadline.is_some_and(|value| tokio::time::Instant::now() >= value) {
        return Err(KernelTransportStop::DeadlineExceeded);
    }

    let future = retry_with_backoff(policy, attempt, should_retry, retry_after_of);
    tokio::pin!(future);
    let poll_interval = poll_interval.max(Duration::from_millis(1));
    loop {
        on_poll();
        let wait = deadline
            .map(|value| value.saturating_duration_since(tokio::time::Instant::now()))
            .map(|remaining| remaining.min(poll_interval))
            .unwrap_or(poll_interval);
        tokio::select! {
            result = &mut future => return Ok(result),
            _ = tokio::time::sleep(wait) => {
                if is_cancelled() {
                    return Err(KernelTransportStop::Cancelled);
                }
                if deadline.is_some_and(|value| tokio::time::Instant::now() >= value) {
                    return Err(KernelTransportStop::DeadlineExceeded);
                }
            }
        }
    }
}

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

/// 流读取侧观察到的信号，决定停滞线如何刷新。
///
/// 语义对齐桌面 UI 的流循环：数据到达只刷新外部看门狗基线，不推进循环内的
/// wall-clock 停滞线（首字节除外）；有效解析产出（正文/工具调用/结束标记）才
/// 重置静默超时；纯 Reasoning 流最多把停滞线顺延到「首次思考 + 宽限期」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelStreamSignal {
    /// 网络 chunk 到达（携带字节数）
    Data(usize),
    /// 正文 Delta 解析产出
    Content,
    /// 思考增量（Reasoning-only 流受宽限期封顶）
    Reasoning,
    /// 工具调用增量
    ToolCall,
    /// 正常结束/截断标记
    Finish,
}

/// Provider 流响应停滞治理的共用状态机：字节预算、静默超时与
/// reasoning-only 宽限封顶。时间全部由调用方注入（`tokio::time::Instant`），
/// 纯策略、无 IO，可离线单测与故障注入。
#[derive(Debug)]
pub struct KernelStreamGovernor {
    silent_timeout: Duration,
    reasoning_grace: Duration,
    max_bytes: usize,
    /// wall-clock 停滞 deadline：命中即无有效产出，独立于流读取 future 计时。
    stall_deadline: Instant,
    /// 首个 Reasoning 事件时间：纯思考流停滞线的顺延基线。
    first_reasoning_at: Option<Instant>,
    total_bytes: usize,
}

impl KernelStreamGovernor {
    pub fn new(silent_timeout: Duration, reasoning_grace: Duration, max_bytes: usize, now: Instant) -> Self {
        Self {
            silent_timeout,
            reasoning_grace,
            max_bytes,
            stall_deadline: now + silent_timeout,
            first_reasoning_at: None,
            total_bytes: 0,
        }
    }

    /// 记录一次信号并推进停滞线。响应体积超限立即报错，防止异常巨大流持续烧资源。
    pub fn observe(&mut self, signal: KernelStreamSignal, now: Instant) -> Result<(), String> {
        match signal {
            KernelStreamSignal::Data(bytes) => {
                self.total_bytes = self.total_bytes.saturating_add(bytes);
                if self.total_bytes > self.max_bytes {
                    return Err(format!(
                        "流式响应体积超限(>{:.1}MiB)，已中断防止持续卡死",
                        self.max_bytes as f64 / 1024.0 / 1024.0
                    ));
                }
                // 首字节视为一次数据到达 + 有效产出，初始化停滞线；后续数据到达
                // 不推进 wall-clock 停滞线（大输出传输由数据看门狗另行兜底）。
                if self.total_bytes == bytes {
                    self.stall_deadline = now + self.silent_timeout;
                }
            }
            KernelStreamSignal::Content | KernelStreamSignal::ToolCall | KernelStreamSignal::Finish => {
                self.stall_deadline = now + self.silent_timeout;
                // 正文/工具调用到达后退出 pure-reasoning 模式，恢复常规静默语义。
                self.first_reasoning_at = None;
            }
            KernelStreamSignal::Reasoning => {
                // reasoning-only 护栏：停滞线最多顺延到首次思考 + 宽限期，
                // 之后即使思考持续到达也强制判死。
                if self.first_reasoning_at.is_none() {
                    self.first_reasoning_at = Some(now);
                }
                let grace_end = self.first_reasoning_at.expect("刚写入") + self.reasoning_grace;
                self.stall_deadline = std::cmp::min(now + self.silent_timeout, grace_end);
            }
        }
        Ok(())
    }

    /// 当前停滞 deadline；调用方用它做 `select!` 的 `sleep_until` 分支或轮询判据。
    pub fn deadline(&self) -> Instant {
        self.stall_deadline
    }

    /// 现在是否已停滞（无有效产出超过静默超时或 reasoning 宽限封顶）。
    pub fn stalled(&self, now: Instant) -> bool {
        now >= self.stall_deadline
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }
}

/// 停滞治理的共用策略默认值（与桌面 UI 流循环的 STREAM_SILENT_TIMEOUT /
/// REASONING_ONLY_GRACE_SECS / STREAM_MAX_BYTES 语义一致；UI 切换到本组件后收敛于此）。
pub const KERNEL_STREAM_SILENT_TIMEOUT: Duration = Duration::from_secs(60);
pub const KERNEL_STREAM_REASONING_GRACE: Duration = Duration::from_secs(180);
pub const KERNEL_STREAM_MAX_BYTES: usize = 256 * 1024 * 1024;

/// OpenAI-compatible SSE 字节流行缓冲：跨 chunk 累积，按完整行提取 `data:` payload。
/// 字节级缓冲避免多字节 UTF-8 字符被网络分块截断后损坏 JSON。
#[derive(Default)]
pub struct KernelSseBuffer {
    pending: Vec<u8>,
}

impl KernelSseBuffer {
    pub fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
    }

    /// 取出下一个完整行（不含换行符）。缓冲为空时返回 None；
    /// 行不是合法 UTF-8 时返回空字符串（协议损坏，由调用方跳过）。
    pub fn next_line(&mut self) -> Option<String> {
        let pos = self.pending.iter().position(|byte| *byte == b'\n')?;
        let line: Vec<u8> = self.pending.drain(..=pos).collect();
        let line = line.strip_suffix(b"\n").unwrap_or(&line);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        Some(String::from_utf8_lossy(line).into_owned())
    }

    /// 流结束后冲刷剩余未以换行结尾的尾部。
    pub fn flush(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let tail: Vec<u8> = std::mem::take(&mut self.pending);
        Some(String::from_utf8_lossy(&tail).into_owned())
    }
}

/// 提取一行 SSE 的 `data:` payload（去前导空格后 trim）。注释行、keepalive 空行
/// 与无 `data:` 前缀的行返回 None。
pub fn sse_payload(line: &str) -> Option<&str> {
    let payload = line.strip_prefix("data:")?;
    let payload = payload.strip_prefix(' ').unwrap_or(payload);
    let payload = payload.trim();
    (!payload.is_empty()).then_some(payload)
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

/// 一次 Agent run 的唯一主终止原因。
///
/// adapter 可以继续记录工具错误、验收阻塞等附加 taxonomy，但外层 round loop 只能由一个
/// 主原因终止。集中记录可避免多个 `stopped_by_*` 布尔值遗漏或同时成立。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelRunTermination {
    ModelAccepted,
    CostBudgetExceeded,
    AcceptanceExhausted,
    EmptyRoundsExhausted,
    ToolCallBudgetExceeded,
    ToolLoopExhausted,
    MaxStepsExceeded,
}

impl KernelRunTermination {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ModelAccepted => "model_accepted",
            Self::CostBudgetExceeded => "max_cost_exceeded",
            Self::AcceptanceExhausted => "acceptance_exhausted",
            Self::EmptyRoundsExhausted => "empty_rounds_exhausted",
            Self::ToolCallBudgetExceeded => "max_tool_calls_exceeded",
            Self::ToolLoopExhausted => "tool_loop_exhausted",
            Self::MaxStepsExceeded => "max_steps_exceeded",
        }
    }

    /// 需要进入失败分类的终止原因；模型通过验收后正常停止不产生失败 taxonomy。
    pub fn failure_taxonomy(self) -> Option<&'static str> {
        match self {
            Self::ModelAccepted => None,
            Self::CostBudgetExceeded => Some("max_cost_exceeded"),
            Self::AcceptanceExhausted => Some("acceptance_failed"),
            Self::EmptyRoundsExhausted => Some("empty_rounds_exhausted"),
            Self::ToolCallBudgetExceeded => Some("max_tool_calls_exceeded"),
            Self::ToolLoopExhausted => Some("tool_loop_exhausted"),
            Self::MaxStepsExceeded => Some("max_steps_exceeded"),
        }
    }
}

/// 外层 run-loop 的最小共享状态：锁定首个终止原因，并在自然跑满时归因为步数耗尽。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KernelRunState {
    termination: Option<KernelRunTermination>,
}

impl KernelRunState {
    pub fn terminate(&mut self, reason: KernelRunTermination) {
        if self.termination.is_none() {
            self.termination = Some(reason);
        }
    }

    pub fn termination(&self) -> Option<KernelRunTermination> {
        self.termination
    }

    /// 仅在尚无更具体原因且确实跑满时标记步数耗尽；返回最终终止原因。
    pub fn finish(&mut self, completed_steps: u64, round_limit: u64) -> Option<KernelRunTermination> {
        if self.termination.is_none() && completed_steps >= round_limit {
            self.termination = Some(KernelRunTermination::MaxStepsExceeded);
        }
        self.termination
    }
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

/// 与桌面 UI tool loop 相同的自动重试语义：契约 retry_safe + 可恢复错误白名单 +
/// 指数退避。生产路径传 `TOOL_POLICY`；`attempt` 负责一次执行（含取消与剩余
/// wall time 检查）。
pub async fn run_tool_with_retry<F, Fut>(
    contract: &crate::agent::tools::contracts::ToolContract,
    policy: &RetryPolicy,
    mut attempt: F,
) -> RetryResult<String, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    retry_with_backoff(
        policy,
        &mut attempt,
        |error: &String| contract.retry_safe && crate::agent::tools::is_retryable_err(error),
        |_| None,
    )
    .await
}

/// 重试成功后的模型可见提示，与 UI 的措辞保持一致。
pub fn retry_notice(output: String, attempts: usize) -> String {
    if attempts > 1 {
        format!("（首次执行超时/网络错误，已自动重试 {} 次）\n{output}", attempts - 1)
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn provider_transport_shares_retry_attempt_accounting() {
        let calls = std::cell::Cell::new(0usize);
        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 1,
            max_delay_ms: 2,
        };
        let mut attempt = || async {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                Err("temporary")
            } else {
                Ok(42)
            }
        };
        let result = run_provider_transport(
            &policy,
            None,
            Duration::from_millis(1),
            &mut attempt,
            |_| true,
            |_| None,
            || false,
            || {},
        )
        .await
        .unwrap();
        assert_eq!(result.attempts, 2);
        assert_eq!(result.value.unwrap(), 42);
    }

    #[tokio::test]
    async fn provider_transport_cancellation_drops_pending_attempt() {
        let polls = std::cell::Cell::new(0usize);
        let mut attempt = || std::future::pending::<Result<(), &'static str>>();
        let result = run_provider_transport(
            &RetryPolicy {
                max_attempts: 1,
                base_delay_ms: 1,
                max_delay_ms: 1,
            },
            None,
            Duration::from_millis(1),
            &mut attempt,
            |_| false,
            |_| None,
            || polls.get() >= 2,
            || polls.set(polls.get() + 1),
        )
        .await;
        assert!(matches!(result, Err(KernelTransportStop::Cancelled)));
    }

    #[tokio::test]
    async fn provider_transport_enforces_absolute_deadline_during_attempt() {
        let mut attempt = || std::future::pending::<Result<(), &'static str>>();
        let result = run_provider_transport(
            &RetryPolicy {
                max_attempts: 1,
                base_delay_ms: 1,
                max_delay_ms: 1,
            },
            Some(tokio::time::Instant::now() + Duration::from_millis(5)),
            Duration::from_secs(1),
            &mut attempt,
            |_| false,
            |_| None,
            || false,
            || {},
        )
        .await;
        assert!(matches!(
            result,
            Err(KernelTransportStop::DeadlineExceeded)
        ));
    }

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

    #[test]
    fn run_state_preserves_specific_termination_at_step_limit() {
        let mut state = KernelRunState::default();
        state.terminate(KernelRunTermination::EmptyRoundsExhausted);
        assert_eq!(
            state.finish(2, 2),
            Some(KernelRunTermination::EmptyRoundsExhausted)
        );
        state.terminate(KernelRunTermination::ToolLoopExhausted);
        assert_eq!(
            state.termination(),
            Some(KernelRunTermination::EmptyRoundsExhausted),
            "首个终止原因必须保持稳定"
        );
        assert_eq!(
            state.termination().map(KernelRunTermination::as_str),
            Some("empty_rounds_exhausted")
        );
    }

    #[test]
    fn run_state_classifies_natural_step_exhaustion() {
        let mut state = KernelRunState::default();
        assert_eq!(
            state.finish(3, 3),
            Some(KernelRunTermination::MaxStepsExceeded)
        );
        assert_eq!(
            state
                .termination()
                .and_then(KernelRunTermination::failure_taxonomy),
            Some("max_steps_exceeded")
        );
    }

    fn stream_governor() -> KernelStreamGovernor {
        KernelStreamGovernor::new(
            Duration::from_secs(60),
            Duration::from_secs(180),
            1024,
            Instant::now(),
        )
    }

    #[test]
    fn stream_governor_stalls_without_progress_after_silent_timeout() {
        let start = Instant::now();
        let governor = stream_governor();
        assert!(!governor.stalled(start + Duration::from_secs(59)));
        assert!(governor.stalled(start + Duration::from_secs(61)));
    }

    #[test]
    fn stream_governor_content_refreshes_deadline() {
        let start = Instant::now();
        let mut governor = stream_governor();
        let mut now = start;
        for _ in 0..5 {
            now += Duration::from_secs(59);
            assert!(!governor.stalled(now), "进度刷新前不应停滞");
            governor.observe(KernelStreamSignal::Content, now).unwrap();
        }
        assert!(!governor.stalled(now + Duration::from_secs(59)));
        assert!(governor.stalled(now + Duration::from_secs(61)));
    }

    #[test]
    fn stream_governor_caps_reasoning_only_streams_at_grace_end() {
        let start = Instant::now();
        let mut governor = stream_governor();
        let first_reasoning = start + Duration::from_secs(30);
        governor
            .observe(KernelStreamSignal::Reasoning, first_reasoning)
            .unwrap();
        // 持续思考可推进静默线，但绝不能越过首次思考后的硬上限。
        let mut now = first_reasoning;
        for _ in 0..200 {
            now += Duration::from_secs(10);
            governor.observe(KernelStreamSignal::Reasoning, now).unwrap();
        }
        assert!(!governor.stalled(first_reasoning + Duration::from_secs(179)));
        assert!(governor.stalled(first_reasoning + Duration::from_secs(181)));
        assert_eq!(
            governor.deadline(),
            first_reasoning + Duration::from_secs(180),
            "停滞线必须封顶在首次思考 + 宽限期"
        );
    }

    #[test]
    fn stream_governor_content_exits_pure_reasoning_mode() {
        let start = Instant::now();
        let mut governor = stream_governor();
        governor
            .observe(KernelStreamSignal::Reasoning, start + Duration::from_secs(10))
            .unwrap();
        // 宽限期内持续思考：停滞线被宽限封顶在 t+190，而非 t+210。
        governor
            .observe(KernelStreamSignal::Reasoning, start + Duration::from_secs(150))
            .unwrap();
        assert_eq!(governor.deadline(), start + Duration::from_secs(190));
        // 正文在封顶前到达 → 退出 pure-reasoning 模式，恢复常规静默刷新。
        governor
            .observe(KernelStreamSignal::Content, start + Duration::from_secs(185))
            .unwrap();
        assert_eq!(governor.deadline(), start + Duration::from_secs(245));
        assert!(
            !governor.stalled(start + Duration::from_secs(191)),
            "正文后宽限封顶不应再生效"
        );
        assert!(governor.stalled(start + Duration::from_secs(246)));
    }

    #[test]
    fn stream_governor_data_arrival_does_not_extend_progress_deadline() {
        let start = Instant::now();
        let mut governor = stream_governor();
        governor.observe(KernelStreamSignal::Data(4), start).unwrap();
        // 后续纯数据到达不推进 wall-clock 停滞线（由数据看门狗另行兜底）。
        let mut now = start;
        for _ in 0..10 {
            now += Duration::from_secs(10);
            governor.observe(KernelStreamSignal::Data(4), now).unwrap();
        }
        assert!(governor.stalled(start + Duration::from_secs(61)));
    }

    #[test]
    fn stream_governor_rejects_oversized_responses() {
        let mut governor = stream_governor();
        assert!(governor.observe(KernelStreamSignal::Data(1024), Instant::now()).is_ok());
        let error = governor
            .observe(KernelStreamSignal::Data(1), Instant::now())
            .unwrap_err();
        assert!(error.contains("体积超限"), "{error}");
        assert_eq!(governor.total_bytes(), 1025);
    }
}
