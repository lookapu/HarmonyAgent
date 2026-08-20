//! 错误分类与友好错误信息体系。
//!
//! 目标：把散落的 `Result<_, String>` 错误升级为结构化错误 ——
//! 前端可展示「标题 / 原因 / 建议」，重试层按分类决策是否自动重试，
//! 任务级 Trace 按分类聚合错误分布。
//!
//! 分类语义（与重试白名单、前端展示、指标聚合共用一份定义）：
//! - `Auth` 认证失败（401/403）：不重试，提示更新密钥
//! - `RateLimited` 限流（429）：可重试，尊重 Retry-After
//! - `ContextOverflow` 上下文/输出超长：不重试，建议开新会话
//! - `Server` 服务端 5xx：可重试
//! - `Network` 连接类：可重试
//! - `Timeout` 超时：可重试
//! - `Client` 其他 4xx：不重试，检查参数
//! - `Local` 本地资源/配置问题：不重试
//! - `Budget` 预算不足（日/月限额）：不重试，提示设置或等待限额重置

use serde::Serialize;

/// 错误分类（前端展示 + 重试决策 + 指标聚合共用）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ErrorKind {
    /// 认证失败（401/403，密钥无效/过期）
    Auth,
    /// 限流（429）
    RateLimited,
    /// 上下文/输出长度超限
    ContextOverflow,
    /// Provider 服务端错误（5xx）
    Server,
    /// 网络连接类错误（DNS/连接拒绝/TLS）
    Network,
    /// 请求超时
    Timeout,
    /// 请求被拒（其他 4xx，参数/模型配置问题）
    Client,
    /// 本地配置/资源问题
    Local,
    /// 预算不足（日/月限额），发送前门控拦截
    Budget,
    /// 未分类
    Unknown,
}

impl ErrorKind {
    /// 稳定字符串标识（前端 key / 指标聚合分组）
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorKind::Auth => "auth",
            ErrorKind::RateLimited => "rate_limited",
            ErrorKind::ContextOverflow => "context_overflow",
            ErrorKind::Server => "server",
            ErrorKind::Network => "network",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Client => "client",
            ErrorKind::Local => "local",
            ErrorKind::Budget => "budget",
            ErrorKind::Unknown => "unknown",
        }
    }

    /// 是否值得自动重试（可恢复错误白名单）
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            ErrorKind::RateLimited | ErrorKind::Server | ErrorKind::Network | ErrorKind::Timeout
        )
    }

    /// 默认标题（按分类给出一句话说明）
    pub fn title(&self) -> &'static str {
        match self {
            ErrorKind::Auth => "API Key 无效或已过期",
            ErrorKind::RateLimited => "请求过于频繁（限流）",
            ErrorKind::ContextOverflow => "上下文长度超限",
            ErrorKind::Server => "模型服务端暂时不可用",
            ErrorKind::Network => "网络连接失败",
            ErrorKind::Timeout => "请求超时",
            ErrorKind::Client => "请求被 Provider 拒绝",
            ErrorKind::Local => "本地资源不可用",
            ErrorKind::Budget => "已达预算上限",
            ErrorKind::Unknown => "请求失败",
        }
    }

    /// 默认建议动作
    pub fn suggestion(&self) -> &'static str {
        match self {
            ErrorKind::Auth => "请到 Provider 设置中更新 API Key",
            ErrorKind::RateLimited => "稍后自动重试；若持续发生请降低并发或检查额度",
            ErrorKind::ContextOverflow => "开启新会话或精简对话历史后重试",
            ErrorKind::Server => "稍后自动重试，或切换到其他 Provider",
            ErrorKind::Network => "请检查网络连接或代理设置",
            ErrorKind::Timeout => "稍后自动重试；若频繁超时请更换模型或检查网络",
            ErrorKind::Client => "请检查模型与参数配置",
            ErrorKind::Local => "请检查本地配置后重试",
            ErrorKind::Budget => "请在 Provider 设置中调高日/月预算，或等待限额重置后再试",
            ErrorKind::Unknown => "请重试；若持续失败请查看日志",
        }
    }
}

/// 结构化友好错误：标题 / 原因 / 建议 / 可恢复性 / HTTP 状态码
#[derive(Debug, Clone, Serialize)]
pub struct FriendlyError {
    pub kind: ErrorKind,
    pub title: String,
    pub reason: String,
    pub suggestion: String,
    pub status_code: Option<u16>,
    /// Provider 的 Retry-After（秒，限流时返回），重试层据此等待
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

impl FriendlyError {
    pub fn new(kind: ErrorKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            title: kind.title().to_string(),
            reason: reason.into(),
            suggestion: kind.suggestion().to_string(),
            status_code: None,
            retry_after_secs: None,
        }
    }

    /// 是否值得自动重试
    pub fn retryable(&self) -> bool {
        self.kind.retryable()
    }

    /// 尊重 Provider 的 Retry-After（毫秒）
    pub fn retry_after_ms(&self) -> Option<u64> {
        self.retry_after_secs.map(|s| s.saturating_mul(1000))
    }

    /// 完整可读文本（给模型反馈 / 日志 / 兼容旧字符串接口）
    pub fn to_user_string(&self) -> String {
        format!("{}：{}。{}", self.title, self.reason, self.suggestion)
    }
}

/// HTTP 状态码 → 错误分类
pub fn classify_status(status: u16) -> ErrorKind {
    match status {
        401 | 403 => ErrorKind::Auth,
        429 => ErrorKind::RateLimited,
        400..=499 => ErrorKind::Client,
        500..=599 => ErrorKind::Server,
        _ => ErrorKind::Unknown,
    }
}

/// 从 Provider 非 2xx 响应构造友好错误（识别上下文超长等特殊 4xx）
pub fn provider_error(status: u16, body: &str) -> FriendlyError {
    let lower = body.to_lowercase();
    let kind = if lower.contains("context_length")
        || lower.contains("context length")
        || lower.contains("too many tokens")
        || lower.contains("maximum context")
        || lower.contains("input too long")
    {
        ErrorKind::ContextOverflow
    } else {
        classify_status(status)
    };
    // 图片类 4xx（模型不支持多模态输入，如纯文本模型收到 image_url）：给针对性建议
    let image_related = lower.contains("image_url")
        || lower.contains("unknown variant")
        || lower.contains("multimodal")
        || lower.contains("image input");
    let suggestion = if image_related && kind == ErrorKind::Client {
        "当前模型不支持图片输入（多模态），请切换到支持图片的模型，或移除图片后重试".to_string()
    } else {
        kind.suggestion().to_string()
    };
    let reason = extract_error_message(body).unwrap_or_else(|| {
        let t = body.trim();
        if t.is_empty() {
            "Provider 未返回错误详情".to_string()
        } else {
            t.chars().take(200).collect()
        }
    });
    FriendlyError {
        kind,
        title: kind.title().to_string(),
        reason,
        suggestion,
        status_code: Some(status),
        retry_after_secs: None,
    }
}

/// 带 Retry-After 的 Provider 错误（限流时优先用服务端给出的等待时间）
pub fn provider_error_with_retry_after(
    status: u16,
    body: &str,
    retry_after_secs: Option<u64>,
) -> FriendlyError {
    let mut fe = provider_error(status, body);
    fe.retry_after_secs = retry_after_secs;
    fe
}

/// 解析 Retry-After 头（RFC 7231）为“还需等待的秒数”（对齐 qwen-code retryPolicy 的
/// getRetryAfterDelayMs 口径）：
/// - 延迟秒数：纯数字，如 "120"；
/// - HTTP-date：IMF-fixdate 格式，如 "Wed, 21 Oct 2015 07:28:00 GMT"，
///   返回距该时刻的秒数（已过期返回 0，由退避层兜底）；
/// 两种形态都无法解析时返回 None（调用方退回指数退避）。
pub fn parse_retry_after_secs(v: &str) -> Option<u64> {
    let s = v.trim();
    if s.is_empty() {
        return None;
    }
    // 延迟秒数：RFC 7231 为 1*DIGIT，部分实现带小数，按整数解析即可
    if let Ok(secs) = s.parse::<u64>() {
        return Some(secs);
    }
    // HTTP-date：chrono 的 RFC 2822 解析器覆盖 IMF-fixdate 固定格式
    let dt = chrono::DateTime::parse_from_rfc2822(s).ok()?;
    let now = chrono::Utc::now();
    Some((dt.with_timezone(&chrono::Utc) - now).num_seconds().max(0) as u64)
}

/// 从响应体提取 error.message / message 等常见错误字段
fn extract_error_message(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let pick = |val: &serde_json::Value| -> Option<String> {
        let msg = val
            .get("message")
            .or_else(|| val.get("error_message"))?
            .as_str()?;
        Some(msg.to_string())
    };
    if let Some(m) = v.get("error") {
        if let Some(s) = m.as_str() {
            return Some(s.to_string());
        }
        if let Some(msg) = pick(m) {
            return Some(msg);
        }
        if let Some(code) = m.get("code").and_then(|c| c.as_str()) {
            if let Some(msg) = pick(m) {
                return Some(format!("{code}: {msg}"));
            }
        }
    }
    pick(&v)
}

/// 传输层错误（reqwest）→ 友好错误（区分超时与连接失败）
pub fn transport_error(e: &reqwest::Error) -> FriendlyError {
    let kind = if e.is_timeout() {
        ErrorKind::Timeout
    } else {
        ErrorKind::Network
    };
    FriendlyError::new(kind, e.to_string())
}

/// 旧字符串错误 → 分类（工具执行结果 / 历史错误信息归类用）
pub fn classify_text(s: &str) -> ErrorKind {
    let l = s.to_lowercase();
    if l.contains("超时") || l.contains("timeout") || l.contains("timed out") {
        ErrorKind::Timeout
    } else if l.contains("限流") || l.contains("rate limit") || l.contains("429") {
        ErrorKind::RateLimited
    } else if l.contains("401")
        || l.contains("403")
        || l.contains("unauthorized")
        || l.contains("invalid key")
        || l.contains("api key")
    {
        ErrorKind::Auth
    } else if l.contains("context_length")
        || l.contains("context length")
        || l.contains("截断")
        || l.contains("max_tokens")
    {
        ErrorKind::ContextOverflow
    } else if l.contains("502") || l.contains("503") || l.contains("server error") || l.contains("服务端") {
        ErrorKind::Server
    } else if l.contains("连接")
        || l.contains("connect")
        || l.contains("network")
        || l.contains("dns")
        || l.contains("请求失败")
    {
        ErrorKind::Network
    } else if l.contains("404") || l.contains("bad request") {
        ErrorKind::Client
    } else {
        ErrorKind::Unknown
    }
}

impl From<FriendlyError> for String {
    fn from(e: FriendlyError) -> Self {
        e.to_user_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_status() {
        assert_eq!(classify_status(401), ErrorKind::Auth);
        assert_eq!(classify_status(403), ErrorKind::Auth);
        assert_eq!(classify_status(429), ErrorKind::RateLimited);
        assert_eq!(classify_status(503), ErrorKind::Server);
        assert_eq!(classify_status(404), ErrorKind::Client);
        assert_eq!(classify_status(200), ErrorKind::Unknown);
    }

    #[test]
    fn test_retryable_whitelist() {
        assert!(ErrorKind::RateLimited.retryable());
        assert!(ErrorKind::Server.retryable());
        assert!(ErrorKind::Timeout.retryable());
        assert!(ErrorKind::Network.retryable());
        assert!(!ErrorKind::Auth.retryable());
        assert!(!ErrorKind::Client.retryable());
        assert!(!ErrorKind::ContextOverflow.retryable());
        assert!(!ErrorKind::Local.retryable());
    }

    #[test]
    fn test_provider_error_context_overflow() {
        let body = r#"{"error":{"message":"This model's maximum context length is 128000 tokens"}}"#;
        let fe = provider_error(400, body);
        assert_eq!(fe.kind, ErrorKind::ContextOverflow);
        assert!(fe.reason.contains("maximum context"));
        assert!(!fe.retryable());
    }

    #[test]
    fn test_provider_error_extracts_message() {
        let body = r#"{"error":{"message":"Invalid API key"}}"#;
        let fe = provider_error(401, body);
        assert_eq!(fe.kind, ErrorKind::Auth);
        assert_eq!(fe.reason, "Invalid API key");
        assert_eq!(fe.status_code, Some(401));
    }

    #[test]
    fn test_provider_error_image_not_supported_suggestion() {
        let body = r#"{"error":{"message":"Failed to deserialize the JSON body into the target type: messages[62]: unknown variant `image_url`, expected `text`"}}"#;
        let fe = provider_error(400, body);
        assert_eq!(fe.kind, ErrorKind::Client);
        assert!(fe.suggestion.contains("图片"), "suggestion: {}", fe.suggestion);
        assert!(fe.reason.contains("image_url"));
    }

    #[test]
    fn test_provider_error_normal_client_suggestion_unchanged() {
        let fe = provider_error(400, r#"{"error":{"message":"bad request"}}"#);
        assert_eq!(fe.kind, ErrorKind::Client);
        assert_eq!(fe.suggestion, ErrorKind::Client.suggestion());
    }

    #[test]
    fn test_retry_after_field() {
        let fe = provider_error_with_retry_after(429, "slow down", Some(5));
        assert_eq!(fe.retry_after_ms(), Some(5000));
        assert!(fe.retryable());
    }

    #[test]
    fn test_parse_retry_after() {
        // 延迟秒数
        assert_eq!(parse_retry_after_secs("120"), Some(120));
        assert_eq!(parse_retry_after_secs(" 5 "), Some(5));
        // 无法解析
        assert_eq!(parse_retry_after_secs("abc"), None);
        assert_eq!(parse_retry_after_secs(""), None);
        // HTTP-date 未来时刻 → 正秒数
        let future = chrono::Utc::now() + chrono::Duration::seconds(90);
        let header = future.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        let parsed = parse_retry_after_secs(&header).unwrap();
        assert!(parsed >= 80 && parsed <= 100, "parsed={parsed}");
        // HTTP-date 过去时刻 → 0（退避层兜底）
        let past = chrono::Utc::now() - chrono::Duration::seconds(60);
        let past_header = past.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        assert_eq!(parse_retry_after_secs(&past_header), Some(0));
    }

    #[test]
    fn test_classify_text() {
        assert_eq!(classify_text("连接 Provider 失败: timeout"), ErrorKind::Timeout);
        assert_eq!(classify_text("命令超时（>300s）: hdc"), ErrorKind::Timeout);
        assert_eq!(classify_text("Provider 返回 429: rate limit"), ErrorKind::RateLimited);
        assert_eq!(classify_text("unknown tool"), ErrorKind::Unknown);
    }

    #[test]
    fn test_to_user_string() {
        let fe = FriendlyError::new(ErrorKind::Auth, "key rejected");
        let s = fe.to_user_string();
        assert!(s.contains("API Key"));
        assert!(s.contains("key rejected"));
        assert!(s.contains("更新"));
    }
}
