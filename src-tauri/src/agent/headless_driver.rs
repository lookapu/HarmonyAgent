//! Builtin headless Agent driver (Phase 1).
//!
//! This deliberately starts with a small, auditable OpenAI-compatible loop.  It is
//! not a replacement for the UI loop yet; the driver is marked `minimal` in the
//! event stream and is intended for eval smoke tests while AgentKernel is extracted.

use crate::agent::agent_kernel::{
    build_model_request_plan, run_provider_transport, sse_payload, KernelAcceptanceGate,
    KernelRunTermination, KernelSseBuffer, KernelStopDecision,
    KernelStreamAccumulator, KernelStreamFinish, KernelStreamGovernor, KernelStreamSignal,
    KernelToolEvidence, KernelTransportStop, KernelTurn, KernelUsageLedger, KERNEL_STREAM_MAX_BYTES,
    KERNEL_STREAM_REASONING_GRACE, KERNEL_STREAM_SILENT_TIMEOUT,
};
use crate::agent::kernel_executor::{KernelExecutorState, KernelRunPermit};
use crate::agent::kernel_loop::{KernelRoundControl, KernelRoundInput};
use crate::agent::kernel_history::continuation_instruction;
use crate::agent::eval_report::ModelInfo;
use crate::agent::eval_runner::{AgentDriverError, AgentDriverOutcome, AsyncAgentDriver};
use crate::agent::eval_task::EvalTask;
use crate::agent::event_sink::{AgentEventSink, SessionTrajectorySink};
use crate::agent::headless_runtime::HeadlessToolRuntime;
use crate::agent::session_events::SessionEventType;
use crate::utils::errors::{
    parse_retry_after_secs, provider_error_with_retry_after, transport_error, ErrorKind,
    FriendlyError,
};
use crate::utils::retry::STREAM_REQUEST_POLICY;
use bytes::Bytes;
use futures_util::Stream;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_TOOL_RESULT_CHARS: usize = 32_000;
const MAX_AUDIT_TEXT_CHARS: usize = 4_000;
const MAX_REMEDIATION_ROUNDS: usize = 2;

fn bounded_audit_text(value: &str) -> (String, bool, String) {
    let redacted = serde_json::from_str::<Value>(value)
        .map(|json| crate::utils::redact::redact_json_value(&json).to_string())
        .unwrap_or_else(|_| crate::utils::redact::redact_text(value));
    let mut chars = redacted.chars();
    let preview: String = chars.by_ref().take(MAX_AUDIT_TEXT_CHARS).collect();
    let truncated = chars.next().is_some();
    let digest = format!("sha256:{:x}", Sha256::digest(value.as_bytes()));
    (preview, truncated, digest)
}

fn bounded_tool_output(value: String) -> (String, bool) {
    let mut chars = value.chars();
    let bounded: String = chars.by_ref().take(MAX_TOOL_RESULT_CHARS).collect();
    let truncated = chars.next().is_some();
    if truncated {
        (
            format!("{bounded}\n…[tool output truncated at {MAX_TOOL_RESULT_CHARS} chars]"),
            true,
        )
    } else {
        (bounded, false)
    }
}

#[derive(Clone, Debug)]
pub struct HeadlessProviderConfig {
    pub provider_id: String,
    pub base_url: String,
    pub api_key: String,
    pub model_id: String,
    pub max_rounds: u32,
    pub input_price_cny_per_1k: Option<f64>,
    pub output_price_cny_per_1k: Option<f64>,
}

impl HeadlessProviderConfig {
    pub fn from_env() -> Result<Self, AgentDriverError> {
        let get = |name: &str| {
            std::env::var(name).map_err(|_| {
                AgentDriverError::Failed(format!("builtin driver 缺少环境变量 {name}"))
            })
        };
        let base_url = get("HARMONY_EVAL_BASE_URL")?;
        let api_key = get("HARMONY_EVAL_API_KEY")?;
        let model_id = get("HARMONY_EVAL_MODEL_ID")?;
        let provider_id =
            std::env::var("HARMONY_EVAL_PROVIDER_ID").unwrap_or_else(|_| "openai".into());
        let protocol = std::env::var("HARMONY_EVAL_PROTOCOL").unwrap_or_else(|_| "openai".into());
        if protocol != "openai" {
            return Err(AgentDriverError::Failed(
                "builtin driver 当前仅支持 HARMONY_EVAL_PROTOCOL=openai".into(),
            ));
        }
        if !(base_url.starts_with("https://")
            || base_url.starts_with("http://localhost")
            || base_url.starts_with("http://127.0.0.1"))
        {
            return Err(AgentDriverError::Failed(
                "builtin driver 仅允许 HTTPS 或本地测试 endpoint".into(),
            ));
        }
        if model_id.trim().is_empty() || api_key.trim().is_empty() {
            return Err(AgentDriverError::Failed(
                "builtin driver 的 model_id/api_key 不能为空".into(),
            ));
        }
        let parse_price = |name: &str| -> Result<Option<f64>, AgentDriverError> {
            match std::env::var(name) {
                Ok(value) => {
                    let price = value.parse::<f64>().map_err(|_| {
                        AgentDriverError::Failed(format!("{name} 必须是有限非负数字"))
                    })?;
                    if !price.is_finite() || price < 0.0 {
                        return Err(AgentDriverError::Failed(format!(
                            "{name} 必须是有限非负数字"
                        )));
                    }
                    Ok(Some(price))
                }
                Err(_) => Ok(None),
            }
        };
        Ok(Self {
            provider_id,
            base_url: base_url.trim_end_matches('/').into(),
            api_key,
            model_id,
            max_rounds: 32,
            input_price_cny_per_1k: parse_price("HARMONY_EVAL_INPUT_PRICE_CNY_PER_1K")?,
            output_price_cny_per_1k: parse_price("HARMONY_EVAL_OUTPUT_PRICE_CNY_PER_1K")?,
        })
    }

    pub fn validate_against(&self, model: &ModelInfo) -> Result<(), AgentDriverError> {
        if self.provider_id != model.provider {
            return Err(AgentDriverError::Failed(format!(
                "builtin provider 不匹配 run-config：{} != {}",
                self.provider_id, model.provider
            )));
        }
        if self.model_id != model.model_id {
            return Err(AgentDriverError::Failed(format!(
                "builtin model_id 不匹配 run-config：{} != {}",
                self.model_id, model.model_id
            )));
        }
        let protocol = std::env::var("HARMONY_EVAL_PROTOCOL").unwrap_or_else(|_| "openai".into());
        if protocol != model.protocol {
            return Err(AgentDriverError::Failed(format!(
                "builtin protocol 不匹配 run-config：{} != {}",
                protocol, model.protocol
            )));
        }
        Ok(())
    }
}

trait HeadlessModelClient: Send + Sync {
    fn request<'a>(
        &'a self,
        provider: &'a HeadlessProviderConfig,
        messages: Vec<Value>,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<(KernelTurn, u64), AgentDriverError>> + Send + 'a>>;
}

#[derive(Default)]
struct OpenAiCompatibleClient;

/// run-config 未显式设置 `request_timeout_seconds` 时的内置单请求上限。
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

pub struct HeadlessAgentDriver {
    pub provider: HeadlessProviderConfig,
    client: Arc<dyn HeadlessModelClient>,
    request_timeout: Option<Duration>,
}

impl HeadlessAgentDriver {
    pub fn new(provider: HeadlessProviderConfig) -> Self {
        Self {
            provider,
            client: Arc::new(OpenAiCompatibleClient),
            request_timeout: None,
        }
    }

    /// 单次 Provider 请求的硬上限；缺省时使用 [`DEFAULT_REQUEST_TIMEOUT`]。
    /// 实际生效值始终取它与剩余 wall time 的较小值。
    pub fn with_request_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.request_timeout = timeout;
        self
    }

    #[cfg(test)]
    fn with_client(provider: HeadlessProviderConfig, client: Arc<dyn HeadlessModelClient>) -> Self {
        Self {
            provider,
            client,
            request_timeout: None,
        }
    }

    fn tool_specs() -> Value {
        json!([
            {"type":"function","function":{"name":"list_dir","description":"列出工作区目录结构","parameters":{"type":"object","properties":{"path":{"type":"string"},"depth":{"type":"integer","minimum":1,"maximum":3}}}}},
            {"type":"function","function":{"name":"read_file","description":"读取工作区内的文本文件","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}},
            {"type":"function","function":{"name":"write_file","description":"写入工作区内的文本文件","parameters":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}}},
            {"type":"function","function":{"name":"preview_edit","description":"预览精确文本替换，不写入文件","parameters":{"type":"object","properties":{"path":{"type":"string"},"old":{"type":"string"},"new":{"type":"string"}},"required":["path","old","new"]}}},
            {"type":"function","function":{"name":"edit_file","description":"对工作区文件执行精确文本替换","parameters":{"type":"object","properties":{"path":{"type":"string"},"old":{"type":"string"},"new":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["path","old","new"]}}},
            {"type":"function","function":{"name":"find_files","description":"按 glob 查找工作区内文件","parameters":{"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":200}},"required":["pattern"]}}},
            {"type":"function","function":{"name":"grep_files","description":"在工作区内容中搜索文本或正则","parameters":{"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"},"glob":{"type":"string"},"case_sensitive":{"type":"boolean"},"regex":{"type":"boolean"},"block":{"type":"boolean"}},"required":["pattern"]}}},
            {"type":"function","function":{"name":"search_symbols","description":"结构优先搜索类、函数和方法","parameters":{"type":"object","properties":{"query":{"type":"string"},"role":{"type":"string","enum":["entity","logic"]},"kind":{"type":"string"},"file":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":200}}}}},
            {"type":"function","function":{"name":"repo_query","description":"统一代码检索入口，支持符号、路径、概念和影响面","parameters":{"type":"object","properties":{"query":{"type":"string"},"mode":{"type":"string","enum":["auto","symbol","path","concept","impact"]},"limit":{"type":"integer","minimum":1,"maximum":50}},"required":["query"]}}},
            {"type":"function","function":{"name":"git_status","description":"读取当前工作树 Git 状态","parameters":{"type":"object","properties":{}}}},
            {"type":"function","function":{"name":"git_diff","description":"读取当前工作树 diff","parameters":{"type":"object","properties":{"path":{"type":"string"},"staged":{"type":"boolean"}}}}}
        ])
    }

    async fn request_openai_stream(
        provider: &HeadlessProviderConfig,
        messages: &[Value],
        timeout: Duration,
    ) -> Result<(KernelTurn, u64), AgentDriverError> {
        let tool_specs = Self::tool_specs();
        let mut request_plan = build_model_request_plan(
            "openai",
            &provider.base_url,
            &provider.model_id,
            messages,
            tool_specs.as_array().map(Vec::as_slice),
            true,
            None,
            Some(0.0),
            None,
            None,
        )
        .map_err(AgentDriverError::Failed)?;
        // 流式回合需要 usage 尾帧才能执行成本账本；支持的兼容 Provider 会附带该帧。
        request_plan.body["stream_options"] = json!({"include_usage": true});
        let client = reqwest::Client::new();
        let deadline = tokio::time::Instant::now() + timeout;
        let mut attempt = || {
            let request = client
                .post(&request_plan.url)
                .bearer_auth(&provider.api_key)
                .json(&request_plan.body)
                .send();
            async {
                let response = request.await.map_err(|error| transport_error(&error))?;
                let status = response.status();
                let retry_after = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(parse_retry_after_secs);
                if !status.is_success() {
                    let bytes = response.bytes().await.map_err(|error| transport_error(&error))?;
                    let detail = crate::utils::redact::redact_text(
                        &String::from_utf8_lossy(&bytes)
                            .chars()
                            .take(500)
                            .collect::<String>(),
                    );
                    return Err(provider_error_with_retry_after(
                        status.as_u16(),
                        &detail,
                        retry_after,
                    ));
                }
                read_openai_sse_stream(response.bytes_stream()).await
            }
        };
        let result = run_provider_transport(
            &STREAM_REQUEST_POLICY,
            Some(deadline),
            Duration::from_millis(100),
            &mut attempt,
            FriendlyError::retryable,
            FriendlyError::retry_after_ms,
            || false,
            || {},
        )
        .await
        .map_err(|stop| match stop {
            KernelTransportStop::Cancelled => {
                AgentDriverError::Cancelled("Provider 请求已取消".into())
            }
            KernelTransportStop::DeadlineExceeded => AgentDriverError::Cancelled(
                "Provider 请求或重试超过剩余 wall time，已取消".into(),
            ),
        })?;
        let retries = result.attempts.saturating_sub(1) as u64;
        result
            .value
            .map(|value| (value, retries))
            .map_err(|error| AgentDriverError::Failed(error.to_user_string()))
    }
}

/// 消费一个 OpenAI-compatible SSE 字节流：字节级行缓冲、`KernelStreamGovernor` 停滞治理、
/// `KernelStreamAccumulator` 帧归一化，组装为严格 `KernelTurn`。
///
/// 结束条件：`data: [DONE]` 或 finish_reason 帧。停滞（静默超时/思考宽限封顶）、
/// 无结束标记提前关闭、无产出空流、JSON 帧损坏与响应体积超限全部失败关闭，
/// 不把不完整流伪装成正常回合。流式与桌面 UI 共用同一停滞策略常量。
async fn read_openai_sse_stream<S, E>(stream: S) -> Result<KernelTurn, FriendlyError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display + 'static,
{
    read_openai_sse_stream_with(
        stream,
        KernelStreamGovernor::new(
            KERNEL_STREAM_SILENT_TIMEOUT,
            KERNEL_STREAM_REASONING_GRACE,
            KERNEL_STREAM_MAX_BYTES,
            tokio::time::Instant::now(),
        ),
    )
    .await
}

async fn read_openai_sse_stream_with<S, E>(
    mut stream: S,
    mut governor: KernelStreamGovernor,
) -> Result<KernelTurn, FriendlyError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display + 'static,
{
    let mut sse = KernelSseBuffer::default();
    let mut accumulator = KernelStreamAccumulator::default();
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut finish = KernelStreamFinish::None;
    let mut warnings = Vec::new();
    let mut done_marker = false;

    loop {
        tokio::select! {
            chunk = futures_util::StreamExt::next(&mut stream) => match chunk {
                Some(Ok(bytes)) => {
                    governor
                        .observe(KernelStreamSignal::Data(bytes.len()), tokio::time::Instant::now())
                        .map_err(|error| FriendlyError::new(ErrorKind::Network, error))?;
                    sse.push(&bytes);
                    while let Some(line) = sse.next_line() {
                        if ingest_openai_sse_line(
                            &mut accumulator,
                            &mut governor,
                            &mut content,
                            &mut reasoning,
                            &mut finish,
                            &mut warnings,
                            &line,
                        )? {
                            done_marker = true;
                            break;
                        }
                    }
                }
                Some(Err(error)) => {
                    return Err(FriendlyError::new(
                        ErrorKind::Network,
                        format!("读取 Provider 流失败: {error}"),
                    ))
                }
                None => break,
            },
            _ = tokio::time::sleep_until(governor.deadline()) => {
                return Err(FriendlyError::new(
                    ErrorKind::Network,
                    format!("Provider 流停滞（{}s 无有效产出）", KERNEL_STREAM_SILENT_TIMEOUT.as_secs()),
                ));
            }
        }
        if done_marker || finish != KernelStreamFinish::None {
            break;
        }
    }
    if !done_marker {
        while let Some(line) = sse.flush() {
            if ingest_openai_sse_line(
                &mut accumulator,
                &mut governor,
                &mut content,
                &mut reasoning,
                &mut finish,
                &mut warnings,
                &line,
            )? {
                break;
            }
        }
    }
    if finish == KernelStreamFinish::None && !done_marker {
        return Err(FriendlyError::new(
            ErrorKind::Network,
            "Provider 流在结束标记前关闭，未产生完整回合",
        ));
    }
    if !warnings.is_empty() {
        return Err(FriendlyError::new(
            ErrorKind::Network,
            format!("Provider 流含协议警告：{}", warnings.join("；")),
        ));
    }
    let tool_calls = accumulator.finalized_tool_calls();
    let mut provider_message = json!({
        "role": "assistant",
        "content": if content.is_empty() { Value::Null } else { Value::String(content.clone()) },
    });
    if !reasoning.is_empty() {
        provider_message["reasoning_content"] = Value::String(reasoning);
    }
    if !tool_calls.is_empty() {
        provider_message["tool_calls"] = Value::Array(
            tool_calls
                .iter()
                .map(|call| {
                    json!({
                        "id": call.id,
                        "type": "function",
                        "function": {"name": call.name, "arguments": call.arguments},
                    })
                })
                .collect(),
        );
    }
    Ok(KernelTurn {
        provider_message,
        content,
        tool_calls,
        usage: accumulator.usage(),
        finish_reason: match finish {
            KernelStreamFinish::Done => Some("stop".into()),
            KernelStreamFinish::Truncated => Some("length".into()),
            KernelStreamFinish::None => None,
        },
    })
}

fn ingest_openai_sse_line(
    accumulator: &mut KernelStreamAccumulator,
    governor: &mut KernelStreamGovernor,
    content: &mut String,
    reasoning: &mut String,
    finish: &mut KernelStreamFinish,
    warnings: &mut Vec<String>,
    line: &str,
) -> Result<bool, FriendlyError> {
    let Some(payload) = sse_payload(line) else { return Ok(false) };
    if payload == "[DONE]" {
        return Ok(true);
    }
    let frame_json: Value = serde_json::from_str(payload).map_err(|error| {
        FriendlyError::new(ErrorKind::Network, format!("Provider SSE 帧无法解析：{error}"))
    })?;
    let frame = accumulator.ingest("openai", &frame_json);
    let now = tokio::time::Instant::now();
    if frame.content.is_some() {
        let _ = governor.observe(KernelStreamSignal::Content, now);
    }
    if frame.reasoning.is_some() {
        let _ = governor.observe(KernelStreamSignal::Reasoning, now);
    }
    if frame.tool_call_deltas > 0 {
        let _ = governor.observe(KernelStreamSignal::ToolCall, now);
    }
    if frame.finish != KernelStreamFinish::None {
        *finish = frame.finish;
        let _ = governor.observe(KernelStreamSignal::Finish, now);
    }
    if let Some(delta) = frame.content {
        content.push_str(&delta);
        if content.len() > MAX_RESPONSE_BYTES {
            return Err(FriendlyError::new(
                ErrorKind::Network,
                "Provider 流式正文超过 8 Mi 字符上限",
            ));
        }
    }
    if let Some(delta) = frame.reasoning {
        reasoning.push_str(&delta);
        if reasoning.len() > MAX_RESPONSE_BYTES {
            return Err(FriendlyError::new(
                ErrorKind::Network,
                "Provider 流式思考内容超过 8 Mi 字符上限",
            ));
        }
    }
    warnings.extend(frame.warnings);
    Ok(false)
}

impl HeadlessModelClient for OpenAiCompatibleClient {
    fn request<'a>(
        &'a self,
        provider: &'a HeadlessProviderConfig,
        messages: Vec<Value>,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<(KernelTurn, u64), AgentDriverError>> + Send + 'a>> {
        Box::pin(async move {
            HeadlessAgentDriver::request_openai_stream(provider, &messages, timeout).await
        })
    }
}

impl HeadlessAgentDriver {
    async fn run_async_impl(
        &self,
        task: &EvalTask,
        workspace: &Path,
    ) -> Result<AgentDriverOutcome, AgentDriverError> {
        let started = Instant::now();
        let conversation_id = format!("headless-{}", uuid::Uuid::new_v4());
        let trace_id = task.task_id.as_str();
        let runtime = HeadlessToolRuntime::new(workspace).map_err(AgentDriverError::Failed)?;
        let mut sink =
            SessionTrajectorySink::from_db(&runtime.db, conversation_id, trace_id.to_string())
                .map_err(AgentDriverError::Failed)?;
        let mut acceptance =
            KernelAcceptanceGate::new(&task.problem_statement, MAX_REMEDIATION_ROUNDS);
        let system = format!(
            "你是 HarmonyAgent 的 headless eval agent。只使用提供的工具修改当前工作区；完成修改后必须验证。不要执行工作区外操作。\n\n{}",
            acceptance.directive()
        );
        let mut messages = vec![
            json!({"role":"system","content":system}),
            json!({"role":"user","content":task.problem_statement}),
        ];
        sink.append(
            SessionEventType::UserMessage,
            json!({"content":task.problem_statement}),
            "user_message",
            json!({"chars":task.problem_statement.len()}),
        )
        .map_err(AgentDriverError::Failed)?;
        let mut outcome = AgentDriverOutcome::default();
        sink.append(SessionEventType::SystemNote, json!({"text":"builtin driver started","mode":"minimal","provider":self.provider.provider_id,"request_timeout_ms":self.request_timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT).as_millis() as u64}), "driver_started", json!({"mode":"minimal","provider":self.provider.provider_id,"request_timeout_ms":self.request_timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT).as_millis() as u64})).map_err(AgentDriverError::Failed)?;
        let mut usage = KernelUsageLedger::new(
            task.limits.max_cost_cny,
            self.provider.input_price_cny_per_1k,
            self.provider.output_price_cny_per_1k,
        )
        .map_err(AgentDriverError::Failed)?;
        let round_limit = self.provider.max_rounds.min(task.limits.max_steps as u32);
        let mut attempted_tool_calls = 0u64;
        
        // UI/headless 共用 executor 状态：轮级路由、循环治理、唯一终止原因。
        let mut kernel_executor = KernelExecutorState::new();
        
        'rounds: for round in 0..round_limit {
            let wall_time = Duration::from_secs(task.limits.wall_time_seconds);
            let remaining = match kernel_executor.permit_run(false, started.elapsed(), wall_time) {
                KernelRunPermit::Proceed { remaining } => remaining,
                KernelRunPermit::Halt(KernelRunTermination::DeadlineExceeded) => {
                    return Err(AgentDriverError::Cancelled(
                        "builtin driver 超过 wall time".into(),
                    ));
                }
                KernelRunPermit::Halt(reason) => {
                    return Err(AgentDriverError::Cancelled(format!(
                        "builtin driver 已停止：{}",
                        reason.as_str()
                    )));
                }
            };
            outcome.steps = round as u64 + 1;
            let request_timeout = self
                .request_timeout
                .unwrap_or(DEFAULT_REQUEST_TIMEOUT)
                .min(remaining);
            let (turn, retries) = self
                .client
                .request(&self.provider, messages.clone(), request_timeout)
                .await?;
            outcome.retries = outcome.retries.saturating_add(retries);
            let cost_exceeded = usage
                .record(turn.usage.as_ref())
                .map_err(AgentDriverError::Failed)?;
            outcome.input_tokens = usage.input_tokens;
            outcome.output_tokens = usage.output_tokens;
            outcome.cached_tokens = usage.cached_tokens;
            outcome.cost_cny = usage.cost_cny;
            if turn.was_truncated() {
                outcome.failure_taxonomy.push("provider_truncated".into());
            }
            sink.append(
                SessionEventType::AssistantMessage,
                json!({"content":turn.content,"tool_calls":turn.tool_calls.len()}),
                "assistant_message",
                json!({"chars":turn.content.len(),"tool_calls":turn.tool_calls.len()}),
            )
            .map_err(AgentDriverError::Failed)?;
            messages.push(turn.provider_message.clone());
            if cost_exceeded {
                kernel_executor.terminate(KernelRunTermination::CostBudgetExceeded);
                sink.append(
                    SessionEventType::SystemNote,
                    json!({"reason":"max_cost_exceeded","cost_cny":outcome.cost_cny}),
                    "agent_budget_stop",
                    json!({"reason":"max_cost_exceeded","cost_cny":outcome.cost_cny}),
                )
                .map_err(AgentDriverError::Failed)?;
                break;
            }
            
            // Phase F：轮级路由——在 stop-candidate 前判定空轮/冻结重放/中断续写/截断续写/假调用纠正
            let has_reasoning = turn
                .provider_message
                .get("reasoning_content")
                .or_else(|| turn.provider_message.get("reasoning"))
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty());
            let round_input = KernelRoundInput {
                text: &turn.content,
                has_reasoning,
                truncated: turn.was_truncated(),
                interrupted: false, // headless 流错误 fail-closed，不进入中断续写（文档画线）
                has_native_tool_calls: !turn.tool_calls.is_empty(),
            };
            let decision = kernel_executor.decide_round(&round_input);
            for notice in decision.notices {
                sink.append(
                    SessionEventType::SystemNote,
                    json!({"notice": notice}),
                    "round_notice",
                    json!({"notice": notice}),
                )
                .map_err(AgentDriverError::Failed)?;
            }
            match decision.control {
                KernelRoundControl::Proceed => {
                    // 继续后续门控（stop-candidate 等）
                }
                KernelRoundControl::RetryEmpty { hint } => {
                    // 空轮重试：注入纠正提示，下一轮继续
                    sink.append(
                        SessionEventType::SystemNote,
                        json!({"hint": hint}),
                        "round_retry_empty",
                        json!({"hint": hint}),
                    )
                    .map_err(AgentDriverError::Failed)?;
                    messages.push(json!({"role":"user","content":hint}));
                    continue 'rounds;
                }
                KernelRoundControl::StopEmpty { note } => {
                    // 空轮耗尽：追加注记后收尾
                    kernel_executor.terminate(KernelRunTermination::EmptyRoundsExhausted);
                    sink.append(
                        SessionEventType::SystemNote,
                        json!({"note": note}),
                        "round_stop_empty",
                        json!({"note": note}),
                    )
                    .map_err(AgentDriverError::Failed)?;
                    break 'rounds;
                }
                KernelRoundControl::ReplayFrozen => {
                    // 冻结重放：headless 不支持（流错误 fail-closed），落穿
                }
                KernelRoundControl::ContinueInterrupted { .. } => {
                    // 中断续写：headless 不支持（流错误 fail-closed），落穿
                }
                KernelRoundControl::ContinueTruncated {
                    continuation_text,
                    reasoning_only,
                } => {
                    // assistant 半截正文已经写入 messages；这里只追加共用续写指令，禁止重复正文。
                    let prompt = continuation_instruction(reasoning_only);
                    sink.append(
                        SessionEventType::SystemNote,
                        json!({"continuation_text": continuation_text, "reasoning_only": reasoning_only}),
                        "round_continuation_truncated",
                        json!({"continuation_text": continuation_text, "reasoning_only": reasoning_only}),
                    )
                    .map_err(AgentDriverError::Failed)?;
                    messages.push(json!({"role":"user","content":prompt}));
                    continue 'rounds;
                }
                KernelRoundControl::CorrectFakeCall {
                    correction_text,
                    hint,
                } => {
                    // 假调用纠正：注入纠正提示继续
                    sink.append(
                        SessionEventType::SystemNote,
                        json!({"correction_text": correction_text, "hint": hint}),
                        "round_correct_fake_call",
                        json!({"correction_text": correction_text, "hint": hint}),
                    )
                    .map_err(AgentDriverError::Failed)?;
                    messages.push(json!({"role":"user","content":hint}));
                    continue 'rounds;
                }
            }

            if turn.is_stop_candidate() {
                match acceptance.request_stop() {
                    KernelStopDecision::Accepted(report) => {
                        kernel_executor.terminate(KernelRunTermination::ModelAccepted);
                        sink.append(
                            SessionEventType::SystemNote,
                            serde_json::to_value(&report)
                                .map_err(|error| AgentDriverError::Failed(error.to_string()))?,
                            "agent_stop_candidate",
                            json!({"reason":"acceptance_passed","evidence_count":report.evidence_count}),
                        )
                        .map_err(AgentDriverError::Failed)?;
                        break;
                    }
                    KernelStopDecision::Remediate {
                        report,
                        prompt,
                        round,
                    } => {
                        sink.append(
                            SessionEventType::SystemNote,
                            serde_json::to_value(&report)
                                .map_err(|error| AgentDriverError::Failed(error.to_string()))?,
                            "agent_acceptance_remediation",
                            json!({"round":round,"blockers":report.blockers}),
                        )
                        .map_err(AgentDriverError::Failed)?;
                        messages.push(json!({"role":"user","content":prompt}));
                        continue;
                    }
                    KernelStopDecision::Exhausted(report) => {
                        kernel_executor.terminate(KernelRunTermination::AcceptanceExhausted);
                        sink.append(
                            SessionEventType::SystemNote,
                            serde_json::to_value(&report)
                                .map_err(|error| AgentDriverError::Failed(error.to_string()))?,
                            "agent_acceptance_exhausted",
                            json!({"blockers":report.blockers,"remediation_rounds":acceptance.remediation_rounds()}),
                        )
                        .map_err(AgentDriverError::Failed)?;
                        break;
                    }
                }
            }
            for call in turn.tool_calls {
                attempted_tool_calls = attempted_tool_calls.saturating_add(1);
                if attempted_tool_calls > task.limits.max_tool_calls {
                    kernel_executor.terminate(KernelRunTermination::ToolCallBudgetExceeded);
                    sink.append(
                        SessionEventType::SystemNote,
                        json!({
                            "reason":"max_tool_calls_exceeded",
                            "attempted":attempted_tool_calls,
                            "limit":task.limits.max_tool_calls,
                        }),
                        "agent_tool_budget_stop",
                        json!({
                            "reason":"max_tool_calls_exceeded",
                            "attempted":attempted_tool_calls,
                            "limit":task.limits.max_tool_calls,
                        }),
                    )
                    .map_err(AgentDriverError::Failed)?;
                    break 'rounds;
                }
                let id = call.id.as_str();
                let name = call.name.as_str();
                let args = call.arguments.as_str();
                let contract = match runtime.policy.check(name, args) {
                    Ok(contract) => contract,
                    Err(reason) => {
                        outcome.policy_violations += 1;
                        sink.append(
                            SessionEventType::ToolApproval,
                            json!({"tool":name,"approved":false,"reason":reason}),
                            "tool_rejected",
                            json!({"name":name,"reason":reason}),
                        )
                        .map_err(AgentDriverError::Failed)?;
                        messages.push(json!({"role":"tool","tool_call_id":id,"content":reason}));
                        continue;
                    }
                };
                
                // Phase F：工具循环检测——在每次工具调用前观察，命中循环时注入纠正提示或直接收尾
                let verdict = kernel_executor.observe_tool(name, args);
                match verdict {
                    crate::agent::kernel_loop::KernelLoopVerdict::Proceed => {
                        // 继续执行工具
                    }
                    crate::agent::kernel_loop::KernelLoopVerdict::Halt { corrective_hint, final_halt, .. } => {
                        if final_halt {
                            // loop_breaks 已超上限，直接收尾
                            kernel_executor.terminate(KernelRunTermination::ToolLoopExhausted);
                            sink.append(
                                SessionEventType::SystemNote,
                                json!({"reason":"tool_loop_exhausted","tool":name}),
                                "tool_loop_halt",
                                json!({"reason":"tool_loop_exhausted","tool":name}),
                            ).map_err(AgentDriverError::Failed)?;
                            break 'rounds;
                        }
                        // 注入纠正提示，让模型换方案
                        if let Some(hint) = corrective_hint {
                            sink.append(
                                SessionEventType::SystemNote,
                                json!({"hint": hint, "tool": name}),
                                "tool_loop_correction",
                                json!({"hint": hint, "tool": name}),
                            ).map_err(AgentDriverError::Failed)?;
                            messages.push(json!({"role":"tool","tool_call_id":id,"content":hint}));
                            continue;
                        }
                    }
                }
                
                sink.append(
                    SessionEventType::ToolApproval,
                    json!({
                        "tool":name,
                        "approved":true,
                        "reason":"headless_policy_and_contract",
                        "effect":contract.effect.as_str(),
                        "recovery":contract.recovery.as_str(),
                        "timeout_ms":contract.timeout_ms,
                    }),
                    "tool_approval",
                    json!({
                        "name":name,
                        "approved":true,
                        "effect":contract.effect.as_str(),
                        "recovery":contract.recovery.as_str(),
                        "timeout_ms":contract.timeout_ms,
                    }),
                )
                .map_err(AgentDriverError::Failed)?;
                sink.append(
                    SessionEventType::ToolCall,
                    {
                        let (args_preview, args_truncated, args_digest) =
                            bounded_audit_text(args);
                        json!({
                            "name":name,
                            "args_preview":args_preview,
                            "args_truncated":args_truncated,
                            "args_digest":args_digest,
                        })
                    },
                    "tool_call",
                    {
                        let (_, args_truncated, args_digest) = bounded_audit_text(args);
                        json!({"name":name,"args_truncated":args_truncated,"args_digest":args_digest})
                    },
                )
                .map_err(AgentDriverError::Failed)?;
                let remaining_wall_time = Duration::from_secs(task.limits.wall_time_seconds)
                    .saturating_sub(started.elapsed());
                let result = runtime
                    .execute_observed(name, args, id, remaining_wall_time)
                    .await;
                outcome.tool_calls += 1;
                let (ok, raw_text) = match result {
                    Ok(v) => (true, v),
                    Err(e) => (false, e),
                };
                let (text, truncated) = bounded_tool_output(raw_text);
                if !ok {
                    outcome.failure_taxonomy.push("tool_error".into());
                    if text.contains("超时") || text.contains("剩余 wall time") {
                        outcome.failure_taxonomy.push("tool_timeout".into());
                    }
                }
                if truncated {
                    outcome
                        .failure_taxonomy
                        .push("tool_output_truncated".into());
                }
                let (output_preview, audit_truncated, output_digest) = bounded_audit_text(&text);
                sink.append(
                    SessionEventType::ToolResult,
                    json!({
                        "name":name,
                        "ok":ok,
                        "output_preview":output_preview,
                        "output_digest":output_digest,
                        "model_context_truncated":truncated,
                        "audit_truncated":audit_truncated,
                    }),
                    "tool_result",
                    json!({
                        "name":name,
                        "ok":ok,
                        "output_digest":output_digest,
                        "model_context_truncated":truncated,
                        "audit_truncated":audit_truncated,
                    }),
                )
                .map_err(AgentDriverError::Failed)?;
                acceptance.record(KernelToolEvidence {
                    tool: name.to_string(),
                    arguments: args.to_string(),
                    output: text.clone(),
                    succeeded: ok,
                });
                messages.push(json!({"role":"tool","tool_call_id":id,"content":text}));
            }
        }
        if let Some(taxonomy) = kernel_executor
            .finish(outcome.steps, round_limit as u64)
            .and_then(KernelRunTermination::failure_taxonomy)
        {
            outcome.failure_taxonomy.push(taxonomy.into());
        }
        let acceptance_report = acceptance.report();
        if !acceptance_report.passed
            && !outcome
                .failure_taxonomy
                .iter()
                .any(|item| item == "acceptance_failed")
        {
            outcome.failure_taxonomy.push("acceptance_failed".into());
        }
        sink.append(
            SessionEventType::SystemNote,
            serde_json::to_value(&acceptance_report)
                .map_err(|error| AgentDriverError::Failed(error.to_string()))?,
            "agent_acceptance_final",
            json!({
                "passed":acceptance_report.passed,
                "blockers":acceptance_report.blockers,
                "evidence_count":acceptance_report.evidence_count,
                "remediation_rounds":acceptance.remediation_rounds(),
            }),
        )
        .map_err(AgentDriverError::Failed)?;
        let tool_quality = runtime
            .quality_summary()
            .map_err(AgentDriverError::Failed)?;
        sink.append(
            SessionEventType::SystemNote,
            serde_json::to_value(&tool_quality)
                .map_err(|error| AgentDriverError::Failed(error.to_string()))?,
            "tool_metrics",
            serde_json::to_value(&tool_quality)
                .map_err(|error| AgentDriverError::Failed(error.to_string()))?,
        )
        .map_err(AgentDriverError::Failed)?;
        let termination_reason = kernel_executor
            .termination()
            .map(KernelRunTermination::as_str)
            .unwrap_or("unknown");
        sink.append(
            SessionEventType::SystemNote,
            json!({"text":"builtin driver finished","steps":outcome.steps,"tool_calls":outcome.tool_calls,"termination_reason":termination_reason}),
            "driver_finished",
            json!({"steps":outcome.steps,"tool_calls":outcome.tool_calls,"termination_reason":termination_reason}),
        )
        .map_err(AgentDriverError::Failed)?;
        outcome.trajectory = sink.into_trajectory();
        if self.provider.input_price_cny_per_1k.is_none()
            || self.provider.output_price_cny_per_1k.is_none()
        {
            outcome.failure_taxonomy.push("cost_unavailable".into());
        }
        Ok(outcome)
    }
}

impl AsyncAgentDriver for HeadlessAgentDriver {
    fn run_async<'a>(
        &'a self,
        task: &'a EvalTask,
        workspace: &'a Path,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<AgentDriverOutcome, AgentDriverError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(self.run_async_impl(task, workspace))
    }
}

#[cfg(test)]
mod tests {
    use super::{HeadlessAgentDriver, HeadlessProviderConfig};
    use crate::agent::eval_report::ModelInfo;
    use crate::agent::eval_runner::AsyncAgentDriver;
    use crate::agent::eval_task::{EvalGrader, EvalLimits, EvalRepo, EvalTask};
    use crate::agent::headless_runtime::HeadlessToolPolicy;
    use bytes::Bytes;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct ScriptedClient {
        responses: Mutex<VecDeque<serde_json::Value>>,
        /// Phase F：记录每请求 messages，用于测试循环检测场景
        recorded_messages: Arc<Mutex<Vec<Vec<serde_json::Value>>>>,
    }

    impl ScriptedClient {
        fn new(responses: VecDeque<serde_json::Value>) -> Self {
            Self::with_recorder(responses, Arc::new(Mutex::new(Vec::new())))
        }

        fn with_recorder(
            responses: VecDeque<serde_json::Value>,
            recorded_messages: Arc<Mutex<Vec<Vec<serde_json::Value>>>>,
        ) -> Self {
            Self {
                responses: Mutex::new(responses),
                recorded_messages,
            }
        }
        
    }

    impl super::HeadlessModelClient for ScriptedClient {
        fn request<'a>(
            &'a self,
            _provider: &'a HeadlessProviderConfig,
            messages: Vec<serde_json::Value>,
            _timeout: Duration,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            (crate::agent::agent_kernel::KernelTurn, u64),
                            crate::agent::eval_runner::AgentDriverError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                // Phase F：记录每请求 messages
                self.recorded_messages.lock()
                    .map_err(|error| {
                        crate::agent::eval_runner::AgentDriverError::Failed(error.to_string())
                    })?
                    .push(messages);
                
                let response = self
                    .responses
                    .lock()
                    .map_err(|error| {
                        crate::agent::eval_runner::AgentDriverError::Failed(error.to_string())
                    })?
                    .pop_front()
                    .ok_or_else(|| {
                        crate::agent::eval_runner::AgentDriverError::Failed(
                            "scripted provider exhausted".into(),
                        )
                    })?;
                let turn = crate::agent::agent_kernel::parse_openai_turn(&response).map_err(
                    crate::agent::eval_runner::AgentDriverError::Failed,
                )?;
                Ok((turn, 0))
            })
        }
    }

    fn offline_task() -> EvalTask {
        EvalTask {
            schema_version: crate::agent::eval_task::EVAL_TASK_SCHEMA_VERSION,
            task_id: "offline__headless-loop".into(),
            suite: "offline".into(),
            problem_statement: "set a.txt content to the target value".into(),
            repo: EvalRepo {
                url: "file:///offline".into(),
                base_commit: "0000000".into(),
                subdir: None,
            },
            limits: EvalLimits {
                wall_time_seconds: 10,
                max_steps: 4,
                max_tool_calls: 100,
                max_cost_cny: 0.0,
                network: "none".into(),
            },
            grader: EvalGrader {
                kind: "command".into(),
                command: vec!["true".into()],
                timeout_seconds: 1,
            },
            artifacts: vec![],
        }
    }

    fn scripted_driver(
        responses: impl IntoIterator<Item = serde_json::Value>,
    ) -> HeadlessAgentDriver {
        scripted_driver_with_prices(responses, 0.0, 0.0)
    }

    fn scripted_driver_with_prices(
        responses: impl IntoIterator<Item = serde_json::Value>,
        input_price: f64,
        output_price: f64,
    ) -> HeadlessAgentDriver {
        let client = ScriptedClient::new(responses.into_iter().collect());
        HeadlessAgentDriver::with_client(
            HeadlessProviderConfig {
                provider_id: "stub".into(),
                base_url: "http://127.0.0.1/offline".into(),
                api_key: "test-secret".into(),
                model_id: "offline-model".into(),
                max_rounds: 4,
                input_price_cny_per_1k: Some(input_price),
                output_price_cny_per_1k: Some(output_price),
            },
            Arc::new(client),
        )
    }

    fn scripted_driver_with_recording(
        responses: impl IntoIterator<Item = serde_json::Value>,
    ) -> (
        HeadlessAgentDriver,
        Arc<Mutex<Vec<Vec<serde_json::Value>>>>,
    ) {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let client = ScriptedClient::with_recorder(
            responses.into_iter().collect(),
            Arc::clone(&recorded),
        );
        let driver = HeadlessAgentDriver::with_client(
            HeadlessProviderConfig {
                provider_id: "stub".into(),
                base_url: "http://127.0.0.1/offline".into(),
                api_key: "test-secret".into(),
                model_id: "offline-model".into(),
                max_rounds: 4,
                input_price_cny_per_1k: Some(0.0),
                output_price_cny_per_1k: Some(0.0),
            },
            Arc::new(client),
        );
        (driver, recorded)
    }

    #[test]
    fn policy_is_fail_closed_for_unknown_tools() {
        let policy = HeadlessToolPolicy;
        assert!(policy.allows("read_file"));
        assert!(policy.allows("write_file"));
        assert!(policy.allows("preview_edit"));
        assert!(policy.allows("edit_file"));
        assert!(policy.allows("find_files"));
        assert!(policy.allows("search_symbols"));
        assert!(policy.allows("repo_query"));
        assert!(policy.allows("git_diff"));
        assert!(!policy.allows("run_command"));
        assert!(!policy.allows("mcp__server__read_file"));
    }

    #[test]
    fn provider_config_must_match_run_manifest() {
        let config = HeadlessProviderConfig {
            provider_id: "openai".into(),
            base_url: "https://example.invalid".into(),
            api_key: "secret".into(),
            model_id: "gpt-test".into(),
            max_rounds: 1,
            input_price_cny_per_1k: Some(1.0),
            output_price_cny_per_1k: Some(2.0),
        };
        let model = ModelInfo {
            provider: "other".into(),
            model_id: "gpt-test".into(),
            protocol: "openai".into(),
            reasoning_effort: "none".into(),
        };
        assert!(config.validate_against(&model).is_err());
    }

    #[test]
    fn tool_output_is_utf8_safe_and_bounded() {
        let source = "界".repeat(super::MAX_TOOL_RESULT_CHARS + 1);
        let (bounded, truncated) = super::bounded_tool_output(source);
        assert!(truncated);
        assert!(bounded.contains("tool output truncated"));
        assert!(bounded.chars().count() < super::MAX_TOOL_RESULT_CHARS + 100);
    }

    #[test]
    fn audit_preview_redacts_json_secrets_and_keeps_raw_digest() {
        let secret = "sk-abcdefghijklmnop123456";
        let raw = format!(r#"{{"api_key":"{secret}","path":"a.txt"}}"#);
        let (preview, truncated, digest) = super::bounded_audit_text(&raw);
        assert!(!truncated);
        assert!(!preview.contains(secret));
        assert!(preview.contains("***"));
        assert!(digest.starts_with("sha256:"));
        assert!(!digest.contains(secret));
    }

    #[tokio::test]
    async fn builtin_driver_completes_offline_tool_loop() {
        let responses = [
            serde_json::json!({
                "choices": [{
                    "finish_reason": "tool_calls",
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call-1",
                            "type": "function",
                            "function": {
                                "name": "write_file",
                                "arguments": "{\"path\":\"a.txt\",\"content\":\"fixed\\n\"}"
                            }
                        }]
                    }
                }],
                "usage": {"prompt_tokens": 10, "completion_tokens": 5}
            }),
            serde_json::json!({
                "choices": [{
                    "finish_reason": "stop",
                    "message": {"role": "assistant", "content": "done"}
                }],
                "usage": {"prompt_tokens": 12, "completion_tokens": 2}
            }),
        ];
        let workspace =
            std::env::temp_dir().join(format!("harmony-headless-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("a.txt"), "base\n").unwrap();
        let task = offline_task();
        let driver = scripted_driver(responses);

        let outcome = driver.run_async(&task, &workspace).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
            "fixed\n"
        );
        assert_eq!(outcome.steps, 2);
        assert_eq!(outcome.tool_calls, 1);
        assert_eq!(outcome.input_tokens, 22);
        assert_eq!(outcome.output_tokens, 7);
        assert!(outcome.failure_taxonomy.is_empty());
        assert!(outcome
            .trajectory
            .iter()
            .any(|event| event.kind == "tool_result"));
        let quality = outcome
            .trajectory
            .iter()
            .find(|event| event.kind == "tool_metrics")
            .expect("工具质量摘要必须进入 trajectory");
        assert_eq!(quality.fields["total_calls"], 1);
        assert_eq!(quality.fields["successful_calls"], 1);
        assert!(!serde_json::to_string(&outcome.trajectory)
            .unwrap()
            .contains("test-secret"));
        std::fs::remove_dir_all(workspace).ok();
    }

    #[tokio::test]
    async fn builtin_driver_remediates_early_stop_until_evidence_passes() {
        let responses = [
            serde_json::json!({
                "choices": [{
                    "finish_reason": "tool_calls",
                    "message": {"role": "assistant", "content": null, "tool_calls": [{
                        "id": "call-write", "type": "function",
                        "function": {"name": "write_file", "arguments": "{\"path\":\"a.txt\",\"content\":\"fixed\\n\"}"}
                    }]}
                }],
                "usage": {"prompt_tokens": 10, "completion_tokens": 5}
            }),
            serde_json::json!({
                "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "done"}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 2}
            }),
            serde_json::json!({
                "choices": [{
                    "finish_reason": "tool_calls",
                    "message": {"role": "assistant", "content": null, "tool_calls": [{
                        "id": "call-read", "type": "function",
                        "function": {"name": "read_file", "arguments": "{\"path\":\"a.txt\"}"}
                    }]}
                }],
                "usage": {"prompt_tokens": 10, "completion_tokens": 3}
            }),
            serde_json::json!({
                "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "verified"}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 2}
            }),
        ];
        let workspace = std::env::temp_dir().join(format!(
            "harmony-headless-acceptance-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("a.txt"), "base\n").unwrap();
        let mut task = offline_task();
        task.problem_statement = "修改 a.txt 的内容并验证".into();

        let outcome = scripted_driver(responses)
            .run_async(&task, &workspace)
            .await
            .unwrap();

        assert_eq!(outcome.steps, 4);
        assert_eq!(outcome.tool_calls, 2);
        assert!(!outcome
            .failure_taxonomy
            .contains(&"acceptance_failed".to_string()));
        assert!(outcome
            .trajectory
            .iter()
            .any(|event| event.kind == "agent_acceptance_remediation"));
        let final_acceptance = outcome
            .trajectory
            .iter()
            .find(|event| event.kind == "agent_acceptance_final")
            .expect("最终验收必须进入 trajectory");
        assert_eq!(final_acceptance.fields["passed"], true);
        assert_eq!(final_acceptance.fields["remediation_rounds"], 1);
        std::fs::remove_dir_all(workspace).ok();
    }

    #[tokio::test]
    async fn builtin_driver_rejects_unapproved_tools_and_records_policy_event() {
        let responses = [
            serde_json::json!({
                "choices": [{
                    "finish_reason": "tool_calls",
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call-dangerous",
                            "type": "function",
                            "function": {"name": "run_command", "arguments": "{\"command\":\"true\"}"}
                        }]
                    }
                }]
            }),
            serde_json::json!({
                "choices": [{
                    "finish_reason": "stop",
                    "message": {"role": "assistant", "content": "cannot run it"}
                }]
            }),
        ];
        let workspace =
            std::env::temp_dir().join(format!("harmony-headless-policy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        let outcome = scripted_driver(responses)
            .run_async(&offline_task(), &workspace)
            .await
            .unwrap();

        assert_eq!(outcome.policy_violations, 1);
        assert_eq!(outcome.tool_calls, 0);
        let rejected = outcome
            .trajectory
            .iter()
            .find(|event| event.kind == "tool_rejected")
            .expect("拒绝必须进入 trajectory");
        assert_eq!(rejected.fields["name"], "run_command");
        assert!(rejected.fields["reason"]
            .as_str()
            .unwrap()
            .contains("未授权"));
        std::fs::remove_dir_all(workspace).ok();
    }

    #[tokio::test]
    async fn cost_limit_stop_preserves_trajectory_and_missing_usage_fails_closed() {
        let response_with_usage = serde_json::json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {"role": "assistant", "content": "expensive response"}
            }],
            "usage": {"prompt_tokens": 100, "completion_tokens": 10}
        });
        let workspace =
            std::env::temp_dir().join(format!("harmony-headless-cost-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        let mut task = offline_task();
        task.limits.max_cost_cny = 0.1;
        let outcome = scripted_driver_with_prices([response_with_usage], 10.0, 10.0)
            .run_async(&task, &workspace)
            .await
            .unwrap();
        assert!(outcome.cost_cny > task.limits.max_cost_cny);
        assert!(outcome
            .failure_taxonomy
            .contains(&"max_cost_exceeded".to_string()));
        assert!(outcome
            .trajectory
            .iter()
            .any(|event| event.kind == "agent_budget_stop"));
        assert!(outcome
            .trajectory
            .iter()
            .any(|event| event.kind == "driver_finished"));

        let response_without_usage = serde_json::json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {"role": "assistant", "content": "no usage"}
            }]
        });
        let error = scripted_driver_with_prices([response_without_usage], 10.0, 10.0)
            .run_async(&task, &workspace)
            .await
            .unwrap_err();
        assert!(
            matches!(error, crate::agent::eval_runner::AgentDriverError::Failed(message) if message.contains("未返回 usage"))
        );
        std::fs::remove_dir_all(workspace).ok();
    }

    #[tokio::test]
    async fn run_permit_stops_before_provider_when_wall_time_is_exhausted() {
        let workspace = std::env::temp_dir().join(format!(
            "harmony-headless-wall-time-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        let mut task = offline_task();
        task.limits.wall_time_seconds = 0;

        let error = scripted_driver([])
            .run_async(&task, &workspace)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            crate::agent::eval_runner::AgentDriverError::Cancelled(message)
                if message.contains("wall time")
        ));

        std::fs::remove_dir_all(workspace).ok();
    }

    // ── Phase F：循环治理与轮级路由集成测试 ───────────────────────────────────────

    #[tokio::test]
    async fn loop_governor_detects_identical_calls_and_injects_correction() {
        // 构造单轮含 6 个相同工具调用：第 5 个应被 governor 拦截并注入纠正提示
        let responses = [
            serde_json::json!({
                "choices": [{
                    "finish_reason": "tool_calls",
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": (0..6).map(|i| {
                            serde_json::json!({
                                "id": format!("call-{}", i),
                                "type": "function",
                                "function": {
                                    "name": "read_file",
                                    "arguments": "{\"path\":\"a.txt\"}"
                                }
                            })
                        }).collect::<Vec<_>>()
                    }
                }],
                "usage": {"prompt_tokens": 10, "completion_tokens": 30}
            }),
            // 模型收到纠正后应给出结论
            serde_json::json!({
                "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "done"}}],
                "usage": {"prompt_tokens": 20, "completion_tokens": 2}
            }),
        ];

        let workspace = std::env::temp_dir().join(format!(
            "harmony-headless-loop-correction-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("a.txt"), "base\n").unwrap();

        let outcome = scripted_driver(responses)
            .run_async(&offline_task(), &workspace)
            .await
            .unwrap();

        // 验证 trajectory 中包含循环纠正事件（第 5 个相同调用被拦截）
        let has_correction = outcome
            .trajectory
            .iter()
            .any(|event| event.kind == "tool_loop_correction");
        assert!(has_correction, "应在第 5 次相同调用时注入纠正提示");
        
        // 验证 tool_result 数量：前 4 个执行，第 5、6 个被拦截
        let tool_results: Vec<_> = outcome
            .trajectory
            .iter()
            .filter(|e| e.kind == "tool_result")
            .collect();
        assert_eq!(tool_results.len(), 4, "前 4 个应执行，第 5、6 个被循环检测拦截");

        std::fs::remove_dir_all(workspace).ok();
    }

    #[tokio::test]
    async fn loop_governor_final_halt_stops_the_driver_round_loop() {
        let response = serde_json::json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": (0..7).map(|i| {
                        serde_json::json!({
                            "id": format!("call-{i}"),
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"a.txt\"}"
                            }
                        })
                    }).collect::<Vec<_>>()
                }
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 30}
        });
        let workspace = std::env::temp_dir().join(format!(
            "harmony-headless-loop-final-halt-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("a.txt"), "base\n").unwrap();

        let outcome = scripted_driver([response])
            .run_async(&offline_task(), &workspace)
            .await
            .unwrap();

        assert_eq!(outcome.steps, 1, "最终循环熔断必须终止外层 round loop");
        assert!(outcome
            .trajectory
            .iter()
            .any(|event| event.kind == "tool_loop_halt"));
        assert!(outcome
            .failure_taxonomy
            .contains(&"tool_loop_exhausted".to_string()));
        assert!(!outcome
            .failure_taxonomy
            .contains(&"max_steps_exceeded".to_string()));
        let finished = outcome
            .trajectory
            .iter()
            .find(|event| event.kind == "driver_finished")
            .expect("终止原因必须进入最终事件");
        assert_eq!(
            finished.fields["termination_reason"],
            "tool_loop_exhausted"
        );

        std::fs::remove_dir_all(workspace).ok();
    }

    #[tokio::test]
    async fn tool_call_budget_stops_varying_calls_inside_one_round() {
        let response = serde_json::json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": (0..4).map(|i| {
                        serde_json::json!({
                            "id": format!("call-{i}"),
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": format!("{{\"path\":\"{i}.txt\"}}")
                            }
                        })
                    }).collect::<Vec<_>>()
                }
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 30}
        });
        let workspace = std::env::temp_dir().join(format!(
            "harmony-headless-tool-budget-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        for i in 0..4 {
            std::fs::write(workspace.join(format!("{i}.txt")), "base\n").unwrap();
        }
        let mut task = offline_task();
        task.limits.max_tool_calls = 3;

        let outcome = scripted_driver([response])
            .run_async(&task, &workspace)
            .await
            .unwrap();

        assert_eq!(outcome.steps, 1);
        assert_eq!(outcome.tool_calls, 3);
        assert!(outcome
            .failure_taxonomy
            .contains(&"max_tool_calls_exceeded".to_string()));
        let budget_stop = outcome
            .trajectory
            .iter()
            .find(|event| event.kind == "agent_tool_budget_stop")
            .expect("工具预算熔断事件必须写入 trajectory");
        assert_eq!(budget_stop.fields["attempted"], 4);
        assert_eq!(budget_stop.fields["limit"], 3);
        assert!(!outcome
            .failure_taxonomy
            .contains(&"max_steps_exceeded".to_string()));
        let finished = outcome
            .trajectory
            .iter()
            .find(|event| event.kind == "driver_finished")
            .expect("终止原因必须进入最终事件");
        assert_eq!(
            finished.fields["termination_reason"],
            "max_tool_calls_exceeded"
        );

        std::fs::remove_dir_all(workspace).ok();
    }

    #[tokio::test]
    async fn round_router_stops_on_consecutive_empty_rounds() {
        // 连续 2 轮空响应 → empty_rounds_exhausted
        let responses = [
            serde_json::json!({
                "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": ""}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 0}
            }),
            serde_json::json!({
                "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": ""}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 0}
            }),
        ];

        let workspace = std::env::temp_dir().join(format!(
            "harmony-headless-empty-rounds-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();

        let mut task = offline_task();
        task.limits.max_steps = 2;
        let outcome = scripted_driver(responses)
            .run_async(&task, &workspace)
            .await
            .unwrap();

        // 验证 failure_taxonomy 包含 empty_rounds_exhausted
        assert!(outcome
            .failure_taxonomy
            .contains(&"empty_rounds_exhausted".to_string()));
        // 验证 trajectory 包含 StopEmpty 注记
        assert!(outcome
            .trajectory
            .iter()
            .any(|event| event.kind == "round_stop_empty"));
        assert!(!outcome
            .failure_taxonomy
            .contains(&"max_steps_exceeded".to_string()));
        let finished = outcome
            .trajectory
            .iter()
            .find(|event| event.kind == "driver_finished")
            .expect("终止原因必须进入最终事件");
        assert_eq!(
            finished.fields["termination_reason"],
            "empty_rounds_exhausted"
        );

        std::fs::remove_dir_all(workspace).ok();
    }

    #[tokio::test]
    async fn round_router_corrects_fake_tool_call_narrative() {
        // 模型在正文中叙述"已调用工具"但未输出标记 → 纠正提示进下一请求
        let responses = [
            serde_json::json!({
                "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "已调用工具 read_file 读取了文件"}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 8}
            }),
            serde_json::json!({
                "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "done after correction"}}],
                "usage": {"prompt_tokens": 15, "completion_tokens": 3}
            }),
        ];

        let workspace = std::env::temp_dir().join(format!(
            "harmony-headless-fake-call-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();

        let outcome = scripted_driver(responses)
            .run_async(&offline_task(), &workspace)
            .await
            .unwrap();

        // 验证 trajectory 包含假调用纠正事件
        assert!(outcome
            .trajectory
            .iter()
            .any(|event| event.kind == "round_correct_fake_call"));
        // 验证纠正后继续执行（steps=2）
        assert_eq!(outcome.steps, 2);

        std::fs::remove_dir_all(workspace).ok();
    }

    #[tokio::test]
    async fn round_router_continues_on_length_truncation() {
        // length 截断 → 续写注入
        let responses = [
            serde_json::json!({
                "choices": [{"finish_reason": "length", "message": {"role": "assistant", "content": "partial output"}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 5}
            }),
            serde_json::json!({
                "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "continued and done"}}],
                "usage": {"prompt_tokens": 15, "completion_tokens": 3}
            }),
        ];

        let workspace = std::env::temp_dir().join(format!(
            "harmony-headless-truncation-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();

        let outcome = scripted_driver(responses)
            .run_async(&offline_task(), &workspace)
            .await
            .unwrap();

        // 验证 trajectory 包含截断续写事件
        assert!(outcome
            .trajectory
            .iter()
            .any(|event| event.kind == "round_continuation_truncated"));
        // 验证续写后完成（steps=2）
        assert_eq!(outcome.steps, 2);

        std::fs::remove_dir_all(workspace).ok();
    }

    #[tokio::test]
    async fn truncation_reuses_shared_instruction_without_duplicating_partial_text() {
        let responses = [
            serde_json::json!({
                "choices": [{"finish_reason": "length", "message": {"role": "assistant", "content": "unique partial output"}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 5}
            }),
            serde_json::json!({
                "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "continued and done"}}],
                "usage": {"prompt_tokens": 15, "completion_tokens": 3}
            }),
        ];
        let workspace = std::env::temp_dir().join(format!(
            "harmony-headless-shared-continuation-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        let (driver, recorded) = scripted_driver_with_recording(responses);

        driver
            .run_async(&offline_task(), &workspace)
            .await
            .unwrap();

        let requests = recorded.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let second = &requests[1];
        let partial_occurrences = second
            .iter()
            .filter(|message| message["content"].as_str() == Some("unique partial output"))
            .count();
        assert_eq!(partial_occurrences, 1, "半截正文只能以 assistant 消息出现一次");
        let last = second.last().unwrap();
        assert_eq!(last["role"], "user");
        assert_eq!(
            last["content"].as_str(),
            Some(crate::agent::kernel_history::continuation_instruction(false))
        );

        std::fs::remove_dir_all(workspace).ok();
    }

    // ── Phase G：差分测试——验证 router 黄金轨迹与 driver 事件序列一致 ────────────────

    #[tokio::test]
    async fn differential_test_loop_governor_trajectory() {
        // 同一语料：先跑 router 得黄金动作，再跑 driver 断言事件序列匹配
        use crate::agent::kernel_loop::KernelLoopGovernor;
        
        // 黄金轨迹：6 次相同调用 → 前 4 次 Proceed，第 5、6 次 Halt（纠正）
        let mut governor = KernelLoopGovernor::new();
        let expected_actions: Vec<&str> = (0..6)
            .map(|_| {
                let verdict = governor.observe("read_file", "{\"path\":\"a.txt\"}");
                match verdict {
                    crate::agent::kernel_loop::KernelLoopVerdict::Proceed => "proceed",
                    crate::agent::kernel_loop::KernelLoopVerdict::Halt { .. } => "halt",
                }
            })
            .collect();
        
        assert_eq!(expected_actions.len(), 6);
        assert_eq!(&expected_actions[..4], &["proceed"; 4]);
        assert_eq!(&expected_actions[4..], &["halt"; 2]);
        
        // Driver 实际事件：前 4 个 tool_result，第 5、6 个被拦截无 tool_result
        let responses = [
            serde_json::json!({
                "choices": [{
                    "finish_reason": "tool_calls",
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": (0..6).map(|i| {
                            serde_json::json!({
                                "id": format!("call-{}", i),
                                "type": "function",
                                "function": {
                                    "name": "read_file",
                                    "arguments": "{\"path\":\"a.txt\"}"
                                }
                            })
                        }).collect::<Vec<_>>()
                    }
                }],
                "usage": {"prompt_tokens": 10, "completion_tokens": 30}
            }),
            serde_json::json!({
                "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "done"}}],
                "usage": {"prompt_tokens": 20, "completion_tokens": 2}
            }),
        ];
        
        let workspace = std::env::temp_dir().join(format!(
            "harmony-diff-loop-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("a.txt"), "base\n").unwrap();
        
        let outcome = scripted_driver(responses)
            .run_async(&offline_task(), &workspace)
            .await
            .unwrap();
        
        // 验证：4 个 proceed → 4 个 tool_result；2 个 halt → 1 个 correction + 1 个静默跳过
        let tool_results: Vec<_> = outcome
            .trajectory
            .iter()
            .filter(|e| e.kind == "tool_result")
            .collect();
        assert_eq!(tool_results.len(), 4, "前 4 次 proceed 应产生 4 个 tool_result");
        
        let corrections: Vec<_> = outcome
            .trajectory
            .iter()
            .filter(|e| e.kind == "tool_loop_correction")
            .collect();
        assert_eq!(corrections.len(), 2, "第 5、6 次 halt 应各注入 1 个纠正提示（共 2 个）");
        
        std::fs::remove_dir_all(workspace).ok();
    }

    #[tokio::test]
    async fn differential_test_round_router_empty_trajectory() {
        // 黄金轨迹：连续 2 轮空 → RetryEmpty(第1轮) → StopEmpty(第2轮)
        use crate::agent::kernel_loop::KernelRoundRouter;
        let mut router = KernelRoundRouter::new();
        let input = crate::agent::kernel_loop::KernelRoundInput {
            text: "",
            has_reasoning: false,
            truncated: false,
            interrupted: false,
            has_native_tool_calls: false,
        };
        
        let decision_r1 = router.decide(&input);
        assert!(decision_r1.notices.is_empty());
        assert!(matches!(
            decision_r1.control,
            crate::agent::kernel_loop::KernelRoundControl::RetryEmpty { .. }
        ));
        
        let decision_r2 = router.decide(&input);
        assert!(decision_r2.notices.is_empty());
        assert!(matches!(
            decision_r2.control,
            crate::agent::kernel_loop::KernelRoundControl::StopEmpty { .. }
        ));
        
        // Driver 实际事件：1 个 round_retry_empty + 1 个 round_stop_empty
        let responses = [
            serde_json::json!({
                "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": ""}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 0}
            }),
            serde_json::json!({
                "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": ""}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 0}
            }),
        ];
        
        let workspace = std::env::temp_dir().join(format!(
            "harmony-diff-empty-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        
        let outcome = scripted_driver(responses)
            .run_async(&offline_task(), &workspace)
            .await
            .unwrap();
        
        let retry_events: Vec<_> = outcome
            .trajectory
            .iter()
            .filter(|e| e.kind == "round_retry_empty")
            .collect();
        assert_eq!(retry_events.len(), 1, "第 1 轮空应产生 round_retry_empty");
        
        let stop_events: Vec<_> = outcome
            .trajectory
            .iter()
            .filter(|e| e.kind == "round_stop_empty")
            .collect();
        assert_eq!(stop_events.len(), 1, "第 2 轮空应产生 round_stop_empty");
        
        std::fs::remove_dir_all(workspace).ok();
    }

    fn sse_chunks(parts: &[&str]) -> Vec<Result<Bytes, std::io::Error>> {
        parts
            .iter()
            .map(|part| Ok(Bytes::from(part.to_string())))
            .collect()
    }

    #[tokio::test]
    async fn stream_reader_assembles_split_utf8_tool_calls_and_usage() {
        // 前两个 chunk 故意把多字节 UTF-8 字符 "固" 截断在字节中间，
        // 字节级行缓冲必须跨 chunk 重组而不是用 lossy 解码损坏 JSON。
        let ch = "固".as_bytes();
        let chunks: Vec<Result<Bytes, std::io::Error>> = vec![
            {
                let mut head = b"data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"".to_vec();
                head.extend_from_slice(&ch[..2]);
                Ok(Bytes::from(head))
            },
            {
                let mut tail = ch[2..].to_vec();
                tail.extend_from_slice(b"\"}}]}\n\n");
                Ok(Bytes::from(tail))
            },
            Ok(Bytes::from(
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-x\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}}]}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3}}\n\ndata: [DONE]\n\n",
            )),
        ];
        let turn = super::read_openai_sse_stream(futures_util::stream::iter(chunks))
            .await
            .unwrap();
        assert_eq!(turn.content, "固");
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].name, "read_file");
        assert_eq!(turn.tool_calls[0].arguments, "{\"path\":\"a.txt\"}");
        assert_eq!(turn.finish_reason.as_deref(), Some("stop"));
        let usage = turn.usage.expect("usage 尾帧必须进入回合");
        assert_eq!(usage.input_tokens, 7);
        assert_eq!(usage.output_tokens, 3);
        assert_eq!(
            turn.provider_message["tool_calls"][0]["function"]["name"],
            "read_file",
            "provider_message 必须能回放给 Provider"
        );
    }

    #[tokio::test]
    async fn stream_reader_marks_length_finish_as_truncated() {
        let chunks = sse_chunks(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
        ]);
        let turn = super::read_openai_sse_stream(futures_util::stream::iter(chunks))
            .await
            .unwrap();
        assert!(turn.was_truncated());
        assert_eq!(turn.finish_reason.as_deref(), Some("length"));
    }

    #[tokio::test]
    async fn stream_reader_preserves_reasoning_for_reasoning_only_truncation() {
        let chunks = sse_chunks(&[
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"internal trace\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
        ]);
        let turn = super::read_openai_sse_stream(futures_util::stream::iter(chunks))
            .await
            .unwrap();

        assert!(turn.content.is_empty());
        assert!(turn.was_truncated());
        assert_eq!(
            turn.provider_message["reasoning_content"],
            "internal trace"
        );
    }

    #[tokio::test]
    async fn stream_reader_fails_closed_on_early_close_without_end_marker() {
        let chunks = sse_chunks(&["data: {\"choices\":[{\"delta\":{\"content\":\"half\"}}]}\n\n"]);
        let error = super::read_openai_sse_stream(futures_util::stream::iter(chunks))
            .await
            .unwrap_err();
        assert!(error.to_user_string().contains("结束标记"), "{error:?}");
    }

    #[tokio::test]
    async fn stream_reader_fails_closed_on_stall() {
        let governor = crate::agent::agent_kernel::KernelStreamGovernor::new(
            Duration::from_millis(50),
            Duration::from_secs(180),
            1024,
            tokio::time::Instant::now(),
        );
        let stream = futures_util::stream::pending::<Result<Bytes, std::io::Error>>();
        let error = super::read_openai_sse_stream_with(stream, governor)
            .await
            .unwrap_err();
        assert!(error.to_user_string().contains("停滞"), "{error:?}");
    }

    #[tokio::test]
    async fn stream_reader_fails_closed_on_broken_frame_and_oversized_content() {
        let chunks = sse_chunks(&["data: {not json\n\n"]);
        let error = super::read_openai_sse_stream(futures_util::stream::iter(chunks))
            .await
            .unwrap_err();
        assert!(error.to_user_string().contains("SSE 帧无法解析"), "{error:?}");

        let big = "x".repeat(super::MAX_RESPONSE_BYTES + 1);
        let chunks = sse_chunks(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"",
            &big,
            "\"}}]}\n\n",
        ]);
        let error = super::read_openai_sse_stream(futures_util::stream::iter(chunks))
            .await
            .unwrap_err();
        assert!(error.to_user_string().contains("上限"), "{error:?}");
    }
}
