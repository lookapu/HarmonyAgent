use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{Mutex as TokioMutex, oneshot};
use hyper::{body::Incoming, server::conn::http1, service::service_fn, Request, Response, StatusCode, Method};
use hyper_util::rt::TokioIo;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use bytes::Bytes;
use rusqlite::Connection;

use super::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use super::cost_calculator;

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub listen_address: String,
    pub listen_port: u16,
    pub auto_failover: bool,
    pub max_retries: u32,
    pub streaming_first_byte_timeout_s: u64,
    pub non_streaming_timeout_s: u64,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen_address: "127.0.0.1".to_string(),
            listen_port: 15800,
            auto_failover: false,
            max_retries: 3,
            streaming_first_byte_timeout_s: 60,
            non_streaming_timeout_s: 600,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProxyStatus {
    pub running: bool,
    pub listen_address: String,
    pub listen_port: u16,
    pub total_requests: u64,
    pub active_provider: Option<String>,
}

struct ProviderInfo {
    id: String,
    base_url: String,
    api_key: Option<String>,
    cost_multiplier: f64,
}

struct ProxyState {
    db: Arc<std::sync::Mutex<Connection>>,
    circuit_breakers: TokioMutex<HashMap<String, CircuitBreaker>>,
    request_count: std::sync::atomic::AtomicU64,
    config: ProxyConfig,
}

pub struct ProxyServer {
    shutdown_tx: Option<oneshot::Sender<()>>,
    status: Arc<TokioMutex<ProxyStatus>>,
}

impl ProxyServer {
    pub fn new() -> Self {
        Self {
            shutdown_tx: None,
            status: Arc::new(TokioMutex::new(ProxyStatus {
                running: false,
                listen_address: "127.0.0.1".to_string(),
                listen_port: 15800,
                total_requests: 0,
                active_provider: None,
            })),
        }
    }

    pub async fn start(
        &mut self,
        db: Arc<std::sync::Mutex<Connection>>,
        mut config: ProxyConfig,
    ) -> Result<(), String> {
        if self.shutdown_tx.is_some() {
            return Err("Proxy already running".to_string());
        }

        // 端口占用自动顺延：从配置端口起逐个尝试（最多 20 个），成功后记录实际端口
        let mut listener = None;
        for offset in 0..20u16 {
            let port = config.listen_port.saturating_add(offset);
            let addr: SocketAddr = format!("{}:{}", config.listen_address, port)
                .parse()
                .map_err(|e| format!("Invalid address: {}", e))?;
            match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => {
                    listener = Some(l);
                    config.listen_port = port;
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && offset < 19 => continue,
                Err(e) => {
                    return Err(format!(
                        "绑定 {}:{} 失败: {}",
                        config.listen_address, config.listen_port.saturating_add(offset), e
                    ))
                }
            }
        }
        let listener = listener.ok_or_else(|| {
            format!(
                "端口 {}~{} 均被占用，无法启动代理（请释放端口或更换监听端口）",
                config.listen_port,
                config.listen_port + 19
            )
        })?;

        let (tx, rx) = oneshot::channel::<()>();
        self.shutdown_tx = Some(tx);

        {
            let mut s = self.status.lock().await;
            s.running = true;
            s.listen_address = config.listen_address.clone();
            s.listen_port = config.listen_port;
        }

        let state = Arc::new(ProxyState {
            db,
            circuit_breakers: TokioMutex::new(HashMap::new()),
            request_count: std::sync::atomic::AtomicU64::new(0),
            config,
        });

        let status_clone = self.status.clone();
        tokio::spawn(async move {
            let mut shutdown_rx = rx;

            loop {
                tokio::select! {
                    accept_result = listener.accept() => {
                        match accept_result {
                            Ok((stream, _)) => {
                                let state = state.clone();
                                let status = status_clone.clone();
                                tokio::spawn(async move {
                                    let io = TokioIo::new(stream);
                                    let svc = service_fn(move |req| {
                                        let st = state.clone();
                                        let sta = status.clone();
                                        async move { handle_request(req, st, sta).await }
                                    });
                                    if let Err(e) = http1::Builder::new()
                                        .serve_connection(io, svc)
                                        .await
                                    {
                                        eprintln!("Proxy connection error: {}", e);
                                    }
                                });
                            }
                            Err(e) => eprintln!("Accept error: {}", e),
                        }
                    }
                    _ = &mut shutdown_rx => {
                        let mut s = status_clone.lock().await;
                        s.running = false;
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), String> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
            let mut s = self.status.lock().await;
            s.running = false;
            Ok(())
        } else {
            Err("Proxy not running".to_string())
        }
    }

    pub async fn get_status(&self) -> ProxyStatus {
        self.status.lock().await.clone()
    }
}

async fn handle_request(
    req: Request<Incoming>,
    state: Arc<ProxyState>,
    status: Arc<TokioMutex<ProxyStatus>>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let count = state.request_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    {
        let mut s = status.lock().await;
        s.total_requests = count;
    }

    if req.method() == Method::OPTIONS {
        return Ok(cors_response());
    }

    let start = std::time::Instant::now();

    let (_parts, body) = req.into_parts();
    let body_bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            return Ok(error_response(StatusCode::BAD_REQUEST, "Failed to read request body"));
        }
    };

    let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_default();
    let model = body_json.get("model").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
    let is_streaming = body_json.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);

    let provider = match select_provider(&state).await {
        Some(p) => p,
        None => {
            return Ok(error_response(StatusCode::SERVICE_UNAVAILABLE, "No available provider"));
        }
    };

    let target_url = format!(
        "{}/chat/completions",
        provider.base_url.trim_end_matches('/')
    );

    let client = reqwest::Client::new();
    let mut req_builder = client.post(&target_url)
        .header("Content-Type", "application/json")
        .body(body_bytes.to_vec());

    if let Some(ref key) = provider.api_key {
        req_builder = req_builder.header("Authorization", format!("Bearer {}", key));
    }

    let timeout = if is_streaming {
        std::time::Duration::from_secs(state.config.streaming_first_byte_timeout_s)
    } else {
        std::time::Duration::from_secs(state.config.non_streaming_timeout_s)
    };

    let response = match tokio::time::timeout(timeout, req_builder.send()).await {
        Ok(Ok(resp)) => resp,
        Ok(Err(e)) => {
            record_failure(&state, &provider.id).await;
            return Ok(error_response(StatusCode::BAD_GATEWAY, &format!("Provider error: {}", e)));
        }
        Err(_) => {
            record_failure(&state, &provider.id).await;
            return Ok(error_response(StatusCode::GATEWAY_TIMEOUT, "Request timeout"));
        }
    };

    let status_code = response.status();
    let latency_ms = start.elapsed().as_millis() as i64;

    if status_code.is_success() {
        record_success(&state, &provider.id).await;
    } else {
        record_failure(&state, &provider.id).await;
    }

    let resp_bytes = response.bytes().await.unwrap_or_default();

    if let Ok(resp_json) = serde_json::from_slice::<serde_json::Value>(&resp_bytes) {
        let usage = cost_calculator::extract_usage_from_response(&resp_json);
        log_request(
            &state,
            &provider.id,
            &model,
            &usage,
            latency_ms,
            status_code.as_u16() as i32,
            provider.cost_multiplier,
            is_streaming,
        );
    }

    let resp = Response::builder()
        .status(status_code.as_u16())
        .header("Content-Type", "application/json")
        .header("Access-Control-Allow-Origin", "*");

    Ok(resp.body(full_body(resp_bytes.to_vec())).unwrap())
}

async fn select_provider(state: &ProxyState) -> Option<ProviderInfo> {
    let providers = {
        let conn = state.db.lock().ok()?;
        let mut stmt = conn.prepare(
            "SELECT id, base_url, api_key, cost_multiplier FROM providers
             WHERE is_active = 1 OR in_failover_queue = 1
             ORDER BY is_active DESC, priority ASC"
        ).ok()?;

        let result: Vec<ProviderInfo> = stmt.query_map([], |row| {
            Ok(ProviderInfo {
                id: row.get(0)?,
                base_url: row.get(1)?,
                api_key: row.get(2)?,
                cost_multiplier: row.get(3)?,
            })
        }).ok()?.filter_map(|r| r.ok()).collect();
        // 密钥可能已迁移到系统凭据管理器（keyring），转发前安全读取补全
        let mut result = result;
        for p in result.iter_mut() {
            if p.api_key.is_none() {
                p.api_key = crate::services::key_store::load_provider_key(&conn, &p.id)
                    .ok()
                    .flatten();
            }
        }
        result
    };

    let mut breakers = state.circuit_breakers.lock().await;

    for provider in providers {
        let breaker = breakers.entry(provider.id.clone()).or_insert_with(|| {
            CircuitBreaker::new(CircuitBreakerConfig::default())
        });

        if breaker.can_execute() {
            return Some(provider);
        }
    }

    None
}

async fn record_success(state: &ProxyState, provider_id: &str) {
    let mut breakers = state.circuit_breakers.lock().await;
    if let Some(cb) = breakers.get_mut(provider_id) {
        cb.record_success();
    }
}

async fn record_failure(state: &ProxyState, provider_id: &str) {
    let mut breakers = state.circuit_breakers.lock().await;
    if let Some(cb) = breakers.get_mut(provider_id) {
        cb.record_failure();
    }
}

fn log_request(
    state: &ProxyState,
    provider_id: &str,
    model: &str,
    usage: &cost_calculator::UsageInfo,
    latency_ms: i64,
    status_code: i32,
    cost_multiplier: f64,
    is_streaming: bool,
) {
    let conn = match state.db.lock() {
        Ok(c) => c,
        Err(_) => return,
    };

    let pricing = cost_calculator::get_pricing(&conn, model);
    let total_cost = pricing.map(|p| {
        cost_calculator::calculate_cost(
            &p, usage.input_tokens, usage.output_tokens,
            usage.cache_read_tokens, usage.cache_creation_tokens, cost_multiplier,
        )
    }).unwrap_or(0.0);

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();

    let _ = conn.execute(
        "INSERT INTO request_logs (id, provider_id, model, input_tokens, output_tokens,
            cache_read_tokens, cache_creation_tokens, total_cost_cny, latency_ms,
            status_code, is_streaming, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        rusqlite::params![
            id, provider_id, model, usage.input_tokens, usage.output_tokens,
            usage.cache_read_tokens, usage.cache_creation_tokens, total_cost,
            latency_ms, status_code, is_streaming as i32, now
        ],
    );

    // 滚动清理：超出保留上限的最旧记录直接删除，防止日志无限堆积
    let _ = crate::services::maintenance::prune_request_logs(&conn, crate::services::maintenance::REQUEST_LOG_KEEP);
}

fn cors_response() -> Response<BoxBody<Bytes, hyper::Error>> {
    Response::builder()
        .status(200)
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Allow-Methods", "POST, GET, OPTIONS")
        .header("Access-Control-Allow-Headers", "Content-Type, Authorization")
        .body(full_body(vec![]))
        .unwrap()
}

fn error_response(status: StatusCode, message: &str) -> Response<BoxBody<Bytes, hyper::Error>> {
    let body = serde_json::json!({ "error": { "message": message } }).to_string();
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .header("Access-Control-Allow-Origin", "*")
        .body(full_body(body.into_bytes()))
        .unwrap()
}

fn full_body(data: Vec<u8>) -> BoxBody<Bytes, hyper::Error> {
    Full::new(Bytes::from(data))
        .map_err(|never| match never {})
        .boxed()
}
