//! 局域网访问（LAN Access）服务。
//!
//! 在进程内启动一个 hyper HTTP 服务器（绑定 0.0.0.0），提供 REST + SSE 接口，
//! 直接复用现有 Tauri 命令函数与全局事件系统（app.emit），本身零业务逻辑：
//! - 静态 Web UI（原生 HTML/CSS/JS，include_str! 内嵌）
//! - 鉴权中间件：Bearer <6位数字 token> + 失败锁定（恒定时间比较）
//! - REST /api/* → 直接调用现有命令函数（AppHandle 取 State）
//! - SSE /api/events → 桥接全局事件 + 按会话缓冲（中途加入回放）

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use bytes::Bytes;
use futures_util::stream::unfold;
use http_body::Frame;
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{HeaderMap, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, EventId, Listener, Manager};
use tokio::sync::{Mutex as TokioMutex, mpsc, oneshot};
use url::form_urlencoded;

use crate::commands::chat::{self, ChatCancel, ChatLock, PlanApprovalState, SessionToolAllowState, ToolApprovalState};
use crate::commands::project;
use crate::db::DbState;

/// 事件桥转发白名单（与前端 chatSlice 监听的桌面事件一致）
const EVENT_WHITELIST: &[&str] = &[
    "chat-stream",
    "chat-stream-batch",
    "chat-reasoning",
    "chat-run-started",
    "chat-heartbeat",
    "chat-done",
    "chat-error",
    "chat-stopped",
    "chat-tool-start",
    "chat-tool-done",
    "chat-tool-approval",
    "chat-plan",
    "chat-plan-resolved",
    "chat-ask",
    "agent:todo",
    "agent:log",
    "agent:log-batch",
    "chat-agent-start",
    "chat-agent-done",
    "chat-job-done",
    "conversation-renamed",
    "conversation-deleted",
    "projects-changed",
    "chat-compact",
];

/// 中途加入缓冲上限：最近 5 分钟 / ≤100KB（先到先截断）
const BUF_MAX_BYTES: usize = 100 * 1024;
const BUF_MAX_AGE_SECS: i64 = 300;

/// SSE 心跳间隔（防代理/移动网络断连）
const SSE_HEARTBEAT_SECS: u64 = 20;

/// 请求体上限（含图片 data URL 等），防止恶意超大 body 拖垮服务
const MAX_BODY_BYTES: usize = 50 * 1024 * 1024;

/// 同时活跃 SSE 连接上限（防连接泄漏）
const MAX_SSE_CONNS: usize = 200;

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct LanConfig {
    pub enabled: bool,
    pub port: u16,
    pub read_only: bool,
    /// 连续鉴权失败次数 + 锁定截止（全局；令牌级撤销/过期见 lan_tokens 表）
    pub fail_count: i32,
    pub lock_until: i64,
}

#[derive(Debug, Deserialize)]
pub struct LanConfigInput {
    pub port: Option<u16>,
    pub read_only: Option<bool>,
}

/// 读取 lan_config 表（id=1，仅全局状态；令牌存于 lan_tokens）
pub fn load_config(conn: &Connection) -> LanConfig {
    conn.query_row(
        "SELECT enabled, port, read_only, fail_count, lock_until
         FROM lan_config WHERE id = 1",
        [],
        |row| {
            Ok(LanConfig {
                enabled: row.get::<_, i32>(0)? != 0,
                port: row.get::<_, i32>(1)? as u16,
                read_only: row.get::<_, i32>(2)? != 0,
                fail_count: row.get(3)?,
                lock_until: row.get(4)?,
            })
        },
    )
    .unwrap_or_default()
}

/// 生成新的 6 位数字 token（uuid 低 4 字节取模 1_000_000，均匀性足够；
/// 安全性由失败锁定兜底，无需密码学随机源）
pub fn generate_token() -> String {
    let uuid = uuid::Uuid::new_v4();
    let b = uuid.as_bytes();
    let n = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) % 1_000_000;
    format!("{:06}", n)
}

/// 生成独立随机盐
pub fn random_salt() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// token 哈希：sha256(salt + token) 十六进制
pub fn hash_token(salt: &str, token: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update(token.as_bytes());
    format!("{:x}", h.finalize())
}

/// 恒定时间比较（逐字节 XOR 累加，防时序侧信道）
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 鉴权结果
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AuthOutcome {
    Ok,
    /// 拒绝；retry_after_secs > 0 表示因失败锁定，还需等待的秒数
    Denied { retry_after_secs: i64 },
}

/// 在有效（未过期）令牌中匹配输入，命中返回 (token_hash, expires_at)。
/// 供 verify_token（鉴权）与 SSE 建立（记录连接归属）共用。
fn match_token(conn: &Connection, input: &str) -> Option<(String, i64)> {
    let now = now_secs();
    let mut stmt = conn
        .prepare(
            "SELECT token_hash, token_salt, expires_at FROM lan_tokens \
             WHERE expires_at = 0 OR expires_at > ?1",
        )
        .ok()?;
    let rows = stmt
        .query_map(rusqlite::params![now], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })
        .ok()?;
    for row in rows.flatten() {
        let (hash, salt, expires_at) = row;
        if constant_time_eq(hash_token(&salt, input).as_bytes(), hash.as_bytes()) {
            return Some((hash, expires_at));
        }
    }
    None
}

/// 校验 token：在 lan_tokens 全表中匹配（恒定时间比较），并检查有效期。
/// 命中 → 记录 last_used_at + 清零全局失败计数；未命中 → 递增失败计数并触发锁定
/// （连续 5 次 → 30s，之后翻倍）。与读取/更新在同一 DB 锁内完成，保证原子性。
pub fn verify_token(conn: &Connection, input: &str) -> AuthOutcome {
    let cfg = load_config(conn);
    let now = now_secs();

    // 全局锁定中：一律拒绝（不消耗失败次数）
    if cfg.lock_until > now {
        return AuthOutcome::Denied {
            retry_after_secs: cfg.lock_until - now,
        };
    }
    // 未配置任何令牌：无法鉴权
    if count_tokens(conn) == 0 {
        return AuthOutcome::Denied { retry_after_secs: 0 };
    }

    if let Some((hash, _)) = match_token(conn, input) {
        let _ = conn.execute(
            "UPDATE lan_tokens SET last_used_at = ?1 WHERE token_hash = ?2",
            rusqlite::params![now, hash],
        );
        let _ = conn.execute(
            "UPDATE lan_config SET fail_count = 0, lock_until = 0 WHERE id = 1",
            [],
        );
        AuthOutcome::Ok
    } else {
        // 未命中：递增失败计数（逻辑与旧版一致）
        let fail = cfg.fail_count + 1;
        let retry = if fail >= 5 {
            let secs = 30i64.saturating_mul(1i64 << (fail - 5).min(8));
            let _ = conn.execute(
                "UPDATE lan_config SET fail_count = ?1, lock_until = ?2 WHERE id = 1",
                rusqlite::params![fail, now + secs],
            );
            secs
        } else {
            let _ = conn.execute(
                "UPDATE lan_config SET fail_count = ?1 WHERE id = 1",
                rusqlite::params![fail],
            );
            0
        };
        AuthOutcome::Denied { retry_after_secs: retry }
    }
}

/// 当前有效（含永久）令牌总数
pub fn count_tokens(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM lan_tokens", [], |r| r.get(0))
        .unwrap_or(0)
}

/// 令牌元数据（明文仅本机 sqlite 持久化，供有效期内重显二维码）
#[derive(Debug, Clone, Serialize)]
pub struct LanTokenInfo {
    pub id: i64,
    pub name: String,
    /// 到期时间戳（unix 秒，0=永不过期）
    pub expires_at: i64,
    pub created_at: i64,
    pub last_used_at: i64,
    /// 由 expires_at 派生的便捷字段：0=永久；>0=剩余秒数；<0=已过期
    pub remaining_secs: i64,
    /// 最近一次使用设备（来自 lan_sessions，空=从未使用）
    pub last_device: String,
    /// 最近一次使用时长（秒，0=未记录）
    pub last_duration_secs: i64,
    /// 6 位数字明文（046 之前创建的旧令牌为 NULL，无法恢复二维码）
    pub token_plain: Option<String>,
}

pub fn list_tokens(conn: &Connection) -> Vec<LanTokenInfo> {
    let now = now_secs();
    let mut out = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT t.id, t.name, t.expires_at, t.created_at, t.last_used_at,
                COALESCE((SELECT s.device FROM lan_sessions s WHERE s.token_id = t.id
                          ORDER BY s.id DESC LIMIT 1), ''),
                COALESCE((SELECT s.duration_secs FROM lan_sessions s WHERE s.token_id = t.id
                          ORDER BY s.id DESC LIMIT 1), 0),
                t.token_plain
         FROM lan_tokens t ORDER BY t.id",
    ) {
        if let Ok(rows) = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        }) {
            for row in rows.flatten() {
                let (
                    id,
                    name,
                    expires_at,
                    created_at,
                    last_used_at,
                    last_device,
                    last_duration_secs,
                    token_plain,
                ) = row;
                let remaining_secs = if expires_at == 0 {
                    0
                } else {
                    expires_at - now
                };
                out.push(LanTokenInfo {
                    id,
                    name,
                    expires_at,
                    created_at,
                    last_used_at,
                    remaining_secs,
                    last_device,
                    last_duration_secs,
                    token_plain,
                });
            }
        }
    }
    out
}

/// 创建新令牌。expires_at = 0 表示永不过期；否则为到期时间戳（unix 秒）。
/// 返回 (令牌 id, 明文)。明文同时落库（token_plain），供有效期内随时重显二维码。
pub fn create_token(
    conn: &Connection,
    name: &str,
    expires_at: i64,
) -> Result<(i64, String), String> {
    if name.trim().is_empty() {
        return Err("令牌名称不能为空".into());
    }
    let now = now_secs();
    let token = generate_token();
    let salt = random_salt();
    let hash = hash_token(&salt, &token);
    conn.execute(
        "INSERT INTO lan_tokens (name, token_hash, token_salt, token_plain, expires_at, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![name.trim(), hash, salt, token, expires_at, now],
    )
    .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    Ok((id, token))
}

/// 撤销令牌（删除记录 = 立即失效）：返回被撤销令牌的哈希，供服务端断开其 SSE 连接
pub fn revoke_token(conn: &Connection, id: i64) -> Result<String, String> {
    let hash: Option<String> = conn
        .query_row("SELECT token_hash FROM lan_tokens WHERE id = ?1", [id], |r| r.get(0))
        .ok();
    match hash {
        Some(h) => {
            conn.execute("DELETE FROM lan_tokens WHERE id = ?1", [id])
                .map_err(|e| e.to_string())?;
            Ok(h)
        }
        None => Err("令牌不存在".into()),
    }
}

/// 从 User-Agent 解析设备类型（手机/平板/桌面）
pub fn parse_device(ua: &str) -> String {
    let l = ua.to_lowercase();
    if l.contains("iphone") || l.contains("ipod") || (l.contains("android") && l.contains("mobile")) {
        if l.contains("iphone") || l.contains("ipod") {
            "手机 (iOS)".to_string()
        } else {
            "手机 (Android)".to_string()
        }
    } else if l.contains("ipad") || (l.contains("android") && !l.contains("mobile")) {
        if l.contains("ipad") {
            "平板 (iPad)".to_string()
        } else {
            "平板 (Android)".to_string()
        }
    } else if l.contains("windows") {
        "桌面 (Windows)".to_string()
    } else if l.contains("mac os") || l.contains("macintosh") {
        "桌面 (macOS)".to_string()
    } else if l.contains("linux") {
        "桌面 (Linux)".to_string()
    } else {
        "未知设备".to_string()
    }
}

/// 建立使用会话（SSE 连接建立时调用）：返回 session id；失败返回 None（不阻塞连接）
pub fn start_session(conn: &Connection, token_hash: &str, ua: &str) -> Option<i64> {
    let token_id: i64 = conn
        .query_row("SELECT id FROM lan_tokens WHERE token_hash = ?1", [token_hash], |r| r.get(0))
        .ok()?;
    let device = parse_device(ua);
    let now = now_secs();
    conn.execute(
        "INSERT INTO lan_sessions (token_id, device, user_agent, started_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![token_id, device, ua, now],
    )
    .ok()?;
    Some(conn.last_insert_rowid())
}

/// 结束使用会话（SSE 连接断开/到期时调用）：写入结束时间与时长
pub fn end_session(conn: &Connection, session_id: i64) {
    let now = now_secs();
    let _ = conn.execute(
        "UPDATE lan_sessions SET ended_at = ?1, duration_secs = ?2 \
         WHERE id = ?3 AND ended_at = 0",
        rusqlite::params![now, (now - start_time(conn, session_id)).max(0), session_id],
    );
}

fn start_time(conn: &Connection, session_id: i64) -> i64 {
    conn.query_row(
        "SELECT started_at FROM lan_sessions WHERE id = ?1",
        [session_id],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// 枚举本机局域网 IPv4 地址（过滤 loopback / link-local / 常见虚拟网卡）
pub fn list_lan_ips() -> Vec<String> {
    // 虚拟网卡名称黑名单（VMware/VirtualBox/Hyper-V/WSL/Tailscale/ZeroTier 等）
    const VIRTUAL_KEYWORDS: [&str; 12] = [
        "vmware",
        "virtualbox",
        "hyper-v",
        "default switch",
        "wsl",
        "tailscale",
        "zerotier",
        "hamachi",
        "wireguard",
        "tunnel",
        "tap-",
        "loopback",
    ];
    local_ip_address::list_afinet_netifas()
        .unwrap_or_default()
        .into_iter()
        .filter(|(name, ip)| {
            if ip.is_loopback() {
                return false;
            }
            let link_local = match ip {
                std::net::IpAddr::V4(v4) => v4.is_link_local(),
                std::net::IpAddr::V6(v6) => v6.is_unicast_link_local(),
            };
            if link_local {
                return false;
            }
            let lower = name.to_lowercase();
            !VIRTUAL_KEYWORDS.iter().any(|k| lower.contains(k))
        })
        .map(|(_, ip)| ip.to_string())
        .filter(|s| s.contains('.'))
        .collect()
}

// ---------------------------------------------------------------------------
// 事件缓冲（中途加入回放）
// ---------------------------------------------------------------------------

/// 单个会话的事件缓冲：按时间序保存流式增量，容量/年龄受限
#[derive(Default)]
struct ConvBuf {
    entries: VecDeque<(String, String, i64)>, // (event_name, payload_json, ts)
    bytes: usize,
}

impl ConvBuf {
    fn push(&mut self, name: String, payload: String, ts: i64) {
        let item_bytes = name.len() + payload.len() + 64;
        self.entries.push_back((name, payload.clone(), ts));
        self.bytes += item_bytes;

        // 按字节截断
        while self.bytes > BUF_MAX_BYTES {
            if let Some((_, p, _)) = self.entries.pop_front() {
                self.bytes = self.bytes.saturating_sub(p.len() + 64);
            }
        }
        // 按年龄截断（以当前时间为基准，而非本条插入时间）
        let cutoff = now_secs() - BUF_MAX_AGE_SECS;
        while self
            .entries
            .front()
            .map(|(_, _, t)| *t < cutoff)
            .unwrap_or(false)
        {
            if let Some((_, p, _)) = self.entries.pop_front() {
                self.bytes = self.bytes.saturating_sub(p.len() + 64);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 服务
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Debug)]
pub struct LanStatus {
    pub running: bool,
    pub listen_port: u16,
    pub read_only: bool,
}

/// 每个 HTTP 连接共享的运行时状态（Arc 化，跨 spawn 安全）
#[derive(Clone)]
struct LanHandlerState {
    buffer: Arc<StdMutex<HashMap<String, ConvBuf>>>,
    /// conn_id → (发送端, 令牌哈希, 到期时间戳) —— 令牌哈希用于撤销时定向断开
    conns: Arc<StdMutex<HashMap<u64, (mpsc::UnboundedSender<String>, String, i64)>>>,
    next_conn: Arc<AtomicU64>,
}

pub struct LanServer {
    shutdown_tx: Option<oneshot::Sender<()>>,
    status: Arc<TokioMutex<LanStatus>>,
    state: LanHandlerState,
    app: Option<AppHandle>,
    /// 事件桥注册的监听 id（白名单事件逐个注册，stop 时逐个注销）
    event_handlers: Vec<EventId>,
    /// 多开互斥锁文件（跨 start/stop 持有；drop 自动解锁）
    _lock_file: Option<std::fs::File>,
}

impl Default for LanServer {
    fn default() -> Self {
        Self::new()
    }
}

impl LanServer {
    pub fn new() -> Self {
        Self {
            shutdown_tx: None,
            status: Arc::new(TokioMutex::new(LanStatus {
                running: false,
                listen_port: 0,
                read_only: false,
            })),
            state: LanHandlerState {
                buffer: Arc::new(StdMutex::new(HashMap::new())),
                conns: Arc::new(StdMutex::new(HashMap::new())),
                next_conn: Arc::new(AtomicU64::new(1)),
            },
            app: None,
            event_handlers: Vec::new(),
            _lock_file: None,
        }
    }

    pub async fn is_running(&self) -> bool {
        self.status.lock().await.running
    }

    /// 注入多开互斥锁文件（命令层获取；start 失败时置 None 释放）
    pub fn set_lock_file(&mut self, file: Option<std::fs::File>) {
        self._lock_file = file;
    }

    pub async fn get_status(&self) -> LanStatus {
        self.status.lock().await.clone()
    }

    /// 启动服务（绑定 0.0.0.0，端口占用自动顺延最多 20 个）
    pub async fn start(
        &mut self,
        app: AppHandle,
        db: Arc<StdMutex<Connection>>,
        config: LanConfig,
    ) -> Result<(), String> {
        if self.shutdown_tx.is_some() {
            return Err("LAN 服务已在运行".to_string());
        }

        // 端口占用自动顺延（与代理服务一致）
        let mut listener = None;
        for offset in 0..20u16 {
            let port = config.port.saturating_add(offset);
            let addr: SocketAddr = format!("0.0.0.0:{}", port)
                .parse()
                .map_err(|e| format!("无效地址: {}", e))?;
            match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => {
                    listener = Some(l);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && offset < 19 => continue,
                Err(e) => {
                    return Err(format!(
                        "绑定 0.0.0.0:{} 失败: {}",
                        config.port.saturating_add(offset),
                        e
                    ))
                }
            }
        }
        let listener = listener.ok_or_else(|| {
            format!(
                "端口 {}~{} 均被占用，无法启动 LAN 服务",
                config.port,
                config.port + 19
            )
        })?;
        let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(config.port);

        {
            let mut s = self.status.lock().await;
            s.running = true;
            s.listen_port = actual_port;
            s.read_only = config.read_only;
        }

        // 注册全局事件桥（白名单事件逐个监听；常驻，保证中途加入有缓冲可回放）
        let buffer = self.state.buffer.clone();
        let conns = self.state.conns.clone();
        let mut handlers = Vec::with_capacity(EVENT_WHITELIST.len());
        for event_name in EVENT_WHITELIST {
            let name = (*event_name).to_string();
            let buffer = buffer.clone();
            let conns = conns.clone();
            let id = app.listen_any(name.clone(), move |event| {
                let payload = event.payload().to_string();
                let now = now_secs();
                // 从 payload 提取会话 id（事件体普遍带 conversation_id）
                let conv = serde_json::from_str::<serde_json::Value>(&payload)
                    .ok()
                    .and_then(|v| {
                        v.get("conversation_id")
                            .and_then(|c| c.as_str())
                            .map(String::from)
                    });

                match name.as_str() {
                    "chat-run-started" => {
                        if let Some(cid) = &conv {
                            if let Ok(mut buf) = buffer.lock() {
                                let cb = buf.entry(cid.clone()).or_default();
                                cb.entries.clear();
                                cb.bytes = 0;
                                cb.push(name.clone(), payload.clone(), now);
                            }
                        }
                    }
                    "chat-stream" | "chat-stream-batch" | "chat-reasoning" => {
                        if let Some(cid) = &conv {
                            if let Ok(mut buf) = buffer.lock() {
                                let cb = buf.entry(cid.clone()).or_default();
                                cb.push(name.clone(), payload.clone(), now);
                            }
                        }
                    }
                    "chat-done" | "chat-error" | "chat-stopped" => {
                        if let Some(cid) = &conv {
                            if let Ok(mut buf) = buffer.lock() {
                                buf.remove(cid);
                            }
                        }
                    }
                    _ => {}
                }

                // 广播给所有活跃 SSE 连接（try 发送，失败 = 连接已断，忽略）
                let text = format!("event: {}\ndata: {}\n\n", name, payload);
                if let Ok(conns) = conns.lock() {
                    for (tx, _, _) in conns.values() {
                        let _ = tx.send(text.clone());
                    }
                }
            });
            handlers.push(id);
        }
        self.event_handlers = handlers;
        self.app = Some(app.clone());

        let (tx, rx) = oneshot::channel::<()>();
        self.shutdown_tx = Some(tx);

        // accept 循环
        let app2 = app.clone();
        let hs = self.state.clone();
        tokio::spawn(async move {
            let mut shutdown_rx = rx;
            loop {
                tokio::select! {
                    accept = listener.accept() => {
                        match accept {
                            Ok((stream, _)) => {
                                let app = app2.clone();
                                let db = db.clone();
                                let hs = hs.clone();
                                tokio::spawn(async move {
                                    let io = TokioIo::new(stream);
                                    let svc = service_fn(move |req| {
                                        let app = app.clone();
                                        let db = db.clone();
                                        let hs = hs.clone();
                                        async move { handle_request(req, app, db, hs).await }
                                    });
                                    if let Err(e) = http1::Builder::new()
                                        .serve_connection(io, svc)
                                        .await
                                    {
                                        eprintln!("[lan] connection error: {}", e);
                                    }
                                });
                            }
                            Err(e) => eprintln!("[lan] accept error: {}", e),
                        }
                    }
                    _ = &mut shutdown_rx => {
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    /// 停止服务：发送关闭信号、注销事件监听、清理连接
    pub async fn stop(&mut self) -> Result<(), String> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(app) = &self.app {
            for id in self.event_handlers.drain(..) {
                app.unlisten(id);
            }
        }
        self.app = None;
        if let Ok(mut conns) = self.state.conns.lock() {
            conns.clear();
        }
        self._lock_file = None;
        {
            let mut s = self.status.lock().await;
            s.running = false;
        }
        Ok(())
    }

    /// 撤销令牌后定向断开其全部 SSE 连接（通知网页端登出，见 app.js session-expired）
    pub fn disconnect_token(&self, token_hash: &str) {
        if let Ok(mut conns) = self.state.conns.lock() {
            let doomed: Vec<u64> = conns
                .iter()
                .filter(|(_, (_, h, _))| h == token_hash)
                .map(|(id, _)| *id)
                .collect();
            for id in doomed {
                if let Some((tx, _, _)) = conns.get(&id) {
                    let _ = tx.send("event: session-expired\ndata: {}\n\n".to_string());
                }
                conns.remove(&id);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 静态资源
// ---------------------------------------------------------------------------

const INDEX_HTML: &str = include_str!("lan_ui/index.html");
const STYLE_CSS: &str = include_str!("lan_ui/style.css");
const APP_JS: &str = include_str!("lan_ui/app.js");
const MANIFEST_JSON: &str = include_str!("lan_ui/manifest.json");
const ICON_SVG: &str = include_str!("lan_ui/icon.svg");

fn static_response(content_type: &str, body: &'static str) -> Response<BoxBody<Bytes, hyper::Error>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", content_type)
        .body(full_body(body.as_bytes().to_vec()))
        .unwrap()
}

// ---------------------------------------------------------------------------
// HTTP 入口
// ---------------------------------------------------------------------------

async fn handle_request(
    req: Request<Incoming>,
    app: AppHandle,
    db: Arc<StdMutex<Connection>>,
    hs: LanHandlerState,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let (parts, body) = req.into_parts();
    let method = parts.method.clone();
    let path = parts.uri.path().to_string();
    let query = parts.uri.query().unwrap_or("").to_string();
    let headers = &parts.headers;

    // 静态资源（无需鉴权）
    match (method.as_str(), path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => {
            return Ok(static_response("text/html; charset=utf-8", INDEX_HTML))
        }
        ("GET", "/style.css") => return Ok(static_response("text/css; charset=utf-8", STYLE_CSS)),
        ("GET", "/app.js") => {
            return Ok(static_response("application/javascript; charset=utf-8", APP_JS))
        }
        ("GET", "/manifest.json") => {
            return Ok(static_response("application/manifest+json; charset=utf-8", MANIFEST_JSON))
        }
        ("GET", "/icon.svg") => {
            return Ok(static_response("image/svg+xml; charset=utf-8", ICON_SVG))
        }
        _ => {}
    }

    // 读取请求体（GET/HEAD 无体）
    let body_bytes = if method == Method::GET || method == Method::HEAD {
        Vec::new()
    } else {
        match body.collect().await {
            Ok(collected) => {
                let bytes = collected.to_bytes().to_vec();
                if bytes.len() > MAX_BODY_BYTES {
                    return Ok(err_response(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "请求体过大（超过 50MB 上限）",
                    ));
                }
                bytes
            }
            Err(_) => {
                return Ok(err_response(StatusCode::BAD_REQUEST, "读取请求体失败"));
            }
        }
    };
    let body_json: serde_json::Value = if body_bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body_bytes).unwrap_or(serde_json::Value::Null)
    };

    // API 前缀：一律鉴权
    let mut auth_token: Option<String> = None;
    if path.starts_with("/api") {
        let token = extract_token(headers, &query);
        auth_token = token.clone();
        let outcome = {
            let conn = match db.lock() {
                Ok(c) => c,
                Err(e) => return Ok(err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())),
            };
            match token {
                Some(t) => verify_token(&conn, &t),
                None => AuthOutcome::Denied { retry_after_secs: 0 },
            }
        };
        if let AuthOutcome::Denied { retry_after_secs } = outcome {
            let v = serde_json::json!({
                "error": { "message": "unauthorized", "retry_after": retry_after_secs }
            });
            return Ok(json_response(StatusCode::UNAUTHORIZED, &v));
        }
    }

    // SSE 事件流（已鉴权；记录令牌归属用于撤销定向断开 + 到期自动断开）
    if method == Method::GET && path == "/api/events" {
        let sess = auth_token.and_then(|t| {
            let conn = db.lock().ok()?;
            match_token(&conn, &t)
        });
        return Ok(match sess {
            Some((hash, expires_at)) => handle_sse(hs, &query, hash, expires_at, db),
            None => err_response(StatusCode::UNAUTHORIZED, "unauthorized"),
        });
    }

    // 只读文件查看（async 命令：内部 spawn_blocking，需 await）
    if method == Method::GET {
        if let ["api", "projects", pid, "file"] = path_segments(&path).as_slice() {
            let st = app.state::<DbState>();
            let file_path = query_get(&query, "path").unwrap_or_default();
            let root = query_get(&query, "root");
            let res = crate::commands::index::read_project_file(
                pid.to_string(),
                file_path,
                root,
                st,
                app.clone(),
            )
            .await;
            return Ok(cmd_response(res));
        }
    }

    // 只读模式：写接口一律 403（SSE 仍可用）
    let read_only = {
        let conn = match db.lock() {
            Ok(c) => c,
            Err(e) => return Ok(err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())),
        };
        load_config(&conn).read_only
    };
    if read_only && method != Method::GET && method != Method::HEAD {
        return Ok(err_response(StatusCode::FORBIDDEN, "只读模式：不允许写操作"));
    }

    Ok(dispatch_api(&app, &method, &path, &query, &body_json, db))
}

/// 从 Authorization: Bearer <token> 或 query ?token= 提取令牌
fn extract_token(headers: &HeaderMap, query: &str) -> Option<String> {
    if let Some(v) = headers.get(hyper::header::AUTHORIZATION) {
        if let Ok(s) = v.to_str() {
            for prefix in ["Bearer ", "bearer "] {
                if let Some(t) = s.strip_prefix(prefix) {
                    let t = t.trim();
                    if !t.is_empty() {
                        return Some(t.to_string());
                    }
                }
            }
        }
    }
    query_get(query, "token")
}

fn parse_query(query: &str) -> Vec<(String, String)> {
    if query.is_empty() {
        return Vec::new();
    }
    form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}

fn query_get(query: &str, key: &str) -> Option<String> {
    parse_query(query)
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

fn path_segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

// ---------------------------------------------------------------------------
// SSE 事件流
// ---------------------------------------------------------------------------

fn handle_sse(
    hs: LanHandlerState,
    query: &str,
    token_hash: String,
    expires_at: i64,
    db: Arc<StdMutex<Connection>>,
) -> Response<BoxBody<Bytes, hyper::Error>> {
    // 连接数上限：超限直接 503，避免异常连接泄漏拖垮服务
    if let Ok(conns) = hs.conns.lock() {
        if conns.len() >= MAX_SSE_CONNS {
            return err_response(StatusCode::SERVICE_UNAVAILABLE, "SSE 连接数过多，请稍后再试");
        }
    }

    // 建立使用会话（记录设备/UA，供设置页展示最近使用设备与时长）
    let ua = query_get(query, "ua").unwrap_or_default();
    let session_id = {
        let conn = db.lock().ok();
        conn.and_then(|c| start_session(&c, &token_hash, &ua))
    };

    let (tx, rx) = mpsc::unbounded_channel::<String>();
    let conn_id = hs.next_conn.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut conns) = hs.conns.lock() {
        conns.insert(conn_id, (tx.clone(), token_hash.clone(), expires_at));
    }

    // 回放：未指定会话则回放全部缓冲；指定则只回放该会话
    let target = query_get(query, "conversation");
    {
        if let Ok(buf) = hs.buffer.lock() {
            for (conv_id, cbuf) in buf.iter() {
                if let Some(t) = &target {
                    if t != conv_id {
                        continue;
                    }
                }
                for (name, payload, _) in &cbuf.entries {
                    let _ = tx.send(format!("event: {}\ndata: {}\n\n", name, payload));
                }
            }
        }
    }

    // 心跳 + 断连清理（channel 关闭后心跳发送失败，移除连接）+ 令牌到期主动断开
    let hb_conns = hs.conns.clone();
    let hb_db = db.clone();
    let hb_tx = tx.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(SSE_HEARTBEAT_SECS)).await;
            // 令牌已到期：通知网页端登出并断开连接
            if expires_at > 0 && now_secs() > expires_at {
                let _ = hb_tx.send("event: session-expired\ndata: {}\n\n".to_string());
                if let Ok(conn) = hb_db.lock() {
                    if let Some(sid) = session_id {
                        end_session(&conn, sid);
                    }
                }
                if let Ok(mut conns) = hb_conns.lock() {
                    conns.remove(&conn_id);
                }
                break;
            }
            if hb_tx.send(": ping\n\n".to_string()).is_err() {
                // 连接已断（网页端关闭/离开）：结束会话并清理连接
                if let Ok(conn) = hb_db.lock() {
                    if let Some(sid) = session_id {
                        end_session(&conn, sid);
                    }
                }
                if let Ok(mut conns) = hb_conns.lock() {
                    conns.remove(&conn_id);
                }
                break;
            }
        }
    });

    let stream = unfold(rx, |mut rx| async move {
        rx.recv().await.map(|msg| {
            (
                Ok::<_, hyper::Error>(Frame::data(Bytes::from(msg))),
                rx,
            )
        })
    });
    let body = StreamBody::new(stream).boxed();

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .header("Access-Control-Allow-Origin", "*")
        .body(body)
        .unwrap()
}

// ---------------------------------------------------------------------------
// REST 路由分派（会话域白名单）
// ---------------------------------------------------------------------------

fn dispatch_api(
    app: &AppHandle,
    method: &Method,
    path: &str,
    query: &str,
    body: &serde_json::Value,
    db: Arc<StdMutex<Connection>>,
) -> Response<BoxBody<Bytes, hyper::Error>> {
    let segs = path_segments(path);
    match (method.as_str(), segs.as_slice()) {
        // 服务状态（已鉴权）
        ("GET", ["api", "lan", "status"]) => {
            let st = app.state::<DbState>();
            let (cfg, token_set) = match st.0.lock() {
                Ok(c) => {
                    let cfg = load_config(&c);
                    (cfg, count_tokens(&c) > 0)
                }
                Err(e) => return err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            };
            let v = serde_json::json!({
                "running": false, // 真实状态由命令层 get_lan_server_status 提供；此处供网页健康检查
                "read_only": cfg.read_only,
                "listen_port": cfg.port,
                "token_set": token_set,
            });
            json_response(StatusCode::OK, &v)
        }

        // 读接口
        ("GET", ["api", "projects"]) => {
            let st = app.state::<DbState>();
            cmd_response(project::list_projects(st))
        }
        ("GET", ["api", "projects", id, "conversations"]) => {
            let st = app.state::<DbState>();
            let archived = query_get(query, "archived").map(|v| v == "1" || v == "true");
            let keyword = query_get(query, "keyword");
            cmd_response(project::list_conversations(
                id.to_string(),
                st,
                archived,
                keyword,
            ))
        }
        ("GET", ["api", "projects", id, "pending"]) => {
            let st = app.state::<DbState>();
            let approval = app.state::<ToolApprovalState>();
            let plan = app.state::<PlanApprovalState>();
            cmd_response(chat::list_pending_confirmations(
                id.to_string(),
                st,
                approval,
                plan,
            ))
        }
        ("GET", ["api", "projects", id, "search"]) => {
            let st = app.state::<DbState>();
            let q = query_get(query, "q").unwrap_or_default();
            let conv = query_get(query, "conversation");
            cmd_response(project::search_messages(id.to_string(), q, conv, st))
        }
        ("GET", ["api", "search"]) => {
            let st = app.state::<DbState>();
            let q = query_get(query, "q").unwrap_or_default();
            cmd_response(project::search_messages_all_projects(q, st))
        }
        ("GET", ["api", "conversations", id, "messages"]) => {
            let st = app.state::<DbState>();
            let before = query_get(query, "before");
            // 分页上限钳制，防止一次拉取海量消息
            let limit = query_get(query, "limit")
                .and_then(|v| v.parse::<usize>().ok())
                .map(|v| v.min(200));
            cmd_response(project::list_messages_page(id.to_string(), before, limit, st))
        }
        ("GET", ["api", "conversations", id, "todos"]) => {
            let v = serde_json::to_value(chat::get_todos(id.to_string()))
                .unwrap_or(serde_json::Value::Null);
            json_response(StatusCode::OK, &v)
        }
        ("GET", ["api", "conversations", id, "cost"]) => {
            let st = app.state::<DbState>();
            cmd_response(chat::conversation_cost_stats(id.to_string(), st))
        }
        ("GET", ["api", "conversations", id, "files"]) => {
            // 会话修改过的文件列表（由 messages.modified_files_json 聚合去重）
            let files = {
                let conn = match db.lock() {
                    Ok(c) => c,
                    Err(e) => {
                        return err_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
                    }
                };
                list_conversation_files(&conn, id)
            };
            json_response(StatusCode::OK, &files)
        }

        // 写接口
        ("POST", ["api", "projects", id, "conversations"]) => {
            let st = app.state::<DbState>();
            let title = body.get("title").and_then(|v| v.as_str()).map(String::from);
            let work_mode = body.get("work_mode").and_then(|v| v.as_str()).map(String::from);
            let worktree_path = body.get("worktree_path").and_then(|v| v.as_str()).map(String::from);
            let worktree_branch = body.get("worktree_branch").and_then(|v| v.as_str()).map(String::from);
            cmd_response(project::create_conversation(
                id.to_string(),
                title,
                work_mode,
                worktree_path,
                worktree_branch,
                st,
            ))
        }
        ("POST", ["api", "conversations", id, "messages"]) => {
            let st = app.state::<DbState>();
            let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if content.trim().is_empty() {
                return err_response(StatusCode::BAD_REQUEST, "content 不能为空");
            }
            cmd_response(project::send_message(id.to_string(), content, st))
        }
        ("POST", ["api", "conversations", id, "stream"]) => {
            let conversation_id = id.to_string();
            let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if content.trim().is_empty() {
                return err_response(StatusCode::BAD_REQUEST, "content 不能为空");
            }
            let options = body
                .get("options")
                .and_then(|o| serde_json::from_value::<chat::ChatOptions>(o.clone()).ok());
            let regenerate = body.get("regenerate").and_then(|v| v.as_bool());
            let regenerate_user_id = body
                .get("regenerate_user_id")
                .and_then(|v| v.as_str())
                .map(String::from);
            let references = body
                .get("references")
                .and_then(|r| serde_json::from_value::<Vec<String>>(r.clone()).ok());
            let images = body
                .get("images")
                .and_then(|r| serde_json::from_value::<Vec<String>>(r.clone()).ok());

            // stream_chat 是整段 async（跑完整任务），必须 spawn 后立即返回 202，事件走 SSE
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                let st = app2.state::<DbState>();
                let lock = app2.state::<ChatLock>();
                let cancel = app2.state::<ChatCancel>();
                let approval = app2.state::<ToolApprovalState>();
                let plan = app2.state::<PlanApprovalState>();
                let _ = chat::stream_chat(
                    app2.clone(),
                    st,
                    lock,
                    cancel,
                    approval,
                    plan,
                    conversation_id,
                    content,
                    options,
                    regenerate,
                    regenerate_user_id,
                    references,
                    images,
                )
                .await;
            });
            let v = serde_json::json!({ "status": "accepted" });
            json_response(StatusCode::ACCEPTED, &v)
        }
        ("POST", ["api", "conversations", id, "stop"]) => {
            let cancel = app.state::<ChatCancel>();
            let registry = app.state::<crate::utils::task_registry::TaskRegistry>();
            cmd_response(chat::stop_chat(id.to_string(), cancel, registry))
        }
        ("POST", ["api", "approvals", request_id]) => {
            dispatch_approval(app, request_id, body)
        }
        ("POST", ["api", "conversations", id, "rename"]) => {
            let st = app.state::<DbState>();
            let title = body.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if title.trim().is_empty() {
                return err_response(StatusCode::BAD_REQUEST, "title 不能为空");
            }
            cmd_response(chat::rename_conversation(id.to_string(), title, st))
        }
        ("POST", ["api", "conversations", id, "pin"]) => {
            let st = app.state::<DbState>();
            let pinned = body.get("pinned").and_then(|v| v.as_bool()).unwrap_or(true);
            cmd_response(project::update_conversation(
                id.to_string(),
                st,
                Some(pinned),
                None,
                None,
                None,
            ))
        }
        ("POST", ["api", "conversations", id, "archive"]) => {
            let st = app.state::<DbState>();
            let archived = body.get("archived").and_then(|v| v.as_bool()).unwrap_or(true);
            cmd_response(project::update_conversation(
                id.to_string(),
                st,
                None,
                Some(archived),
                None,
                None,
            ))
        }
        ("POST", ["api", "conversations", id, "delete"]) => {
            let st = app.state::<DbState>();
            cmd_response(chat::delete_conversation_sync(id, app, &st))
        }

        _ => err_response(StatusCode::NOT_FOUND, "not found"),
    }
}

/// 聚合会话内全部 `messages.modified_files_json` 的去重文件列表（按路径排序）。
/// 供网页"修改文件"栏展示；仅相对路径，不做任何读写。
fn list_conversation_files(conn: &Connection, conversation_id: &str) -> Vec<String> {
    use std::collections::HashSet;
    let mut out: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT modified_files_json FROM messages
         WHERE conversation_id = ?1 AND modified_files_json IS NOT NULL AND modified_files_json != ''",
    ) {
        if let Ok(rows) = stmt.query_map(rusqlite::params![conversation_id], |r| {
            r.get::<_, String>(0)
        }) {
            for row in rows.flatten() {
                if let Ok(v) = serde_json::from_str::<Vec<String>>(&row) {
                    for p in v {
                        let p = p.trim().to_string();
                        if !p.is_empty() && seen.insert(p.clone()) {
                            out.push(p);
                        }
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// 审批三合一：按 body.kind 分发到工具审批 / 计划审查 / Agent 提问
fn dispatch_approval(
    app: &AppHandle,
    request_id: &str,
    body: &serde_json::Value,
) -> Response<BoxBody<Bytes, hyper::Error>> {
    let kind = body.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let approved = body.get("approved").and_then(|v| v.as_bool()).unwrap_or(false);
    match kind {
        "approval" => {
            let remember = body.get("remember").and_then(|v| v.as_bool());
            let feedback = body.get("feedback").and_then(|v| v.as_str()).map(String::from);
            let scope = body.get("scope").and_then(|v| v.as_str()).map(String::from);
            let state = app.state::<ToolApprovalState>();
            let allow = app.state::<SessionToolAllowState>();
            let db = app.state::<DbState>();
            cmd_response(chat::resolve_tool_approval(
                request_id.to_string(),
                approved,
                remember,
                feedback,
                scope,
                state,
                allow,
                db,
            ))
        }
        "plan" => {
            let conversation_id = body
                .get("conversation_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let feedback = body.get("feedback").and_then(|v| v.as_str()).map(String::from);
            let state = app.state::<PlanApprovalState>();
            cmd_response(chat::resolve_plan_review(
                conversation_id,
                request_id.to_string(),
                approved,
                feedback,
                state,
            ))
        }
        "ask" => {
            let answer = body.get("answer").and_then(|v| v.as_str()).map(String::from);
            cmd_response(chat::resolve_ask_user(request_id.to_string(), answer))
        }
        _ => err_response(StatusCode::BAD_REQUEST, "kind 必须为 approval|plan|ask"),
    }
}

// ---------------------------------------------------------------------------
// 响应工具
// ---------------------------------------------------------------------------

fn json_response<T: Serialize>(
    status: StatusCode,
    value: &T,
) -> Response<BoxBody<Bytes, hyper::Error>> {
    let body = serde_json::to_string(value).unwrap_or_else(|_| "null".into());
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .header("Access-Control-Allow-Origin", "*")
        .body(full_body(body.into_bytes()))
        .unwrap()
}

fn err_response(
    status: StatusCode,
    message: &str,
) -> Response<BoxBody<Bytes, hyper::Error>> {
    let v = serde_json::json!({ "error": { "message": message } });
    json_response(status, &v)
}

fn cmd_response<T: Serialize>(r: Result<T, String>) -> Response<BoxBody<Bytes, hyper::Error>> {
    match r {
        Ok(v) => json_response(StatusCode::OK, &v),
        Err(e) => err_response(StatusCode::BAD_REQUEST, &e),
    }
}

fn full_body(data: Vec<u8>) -> BoxBody<Bytes, hyper::Error> {
    Full::new(Bytes::from(data))
        .map_err(|never| match never {})
        .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    /// 构造内存库：lan_config（仅全局状态）+ lan_tokens（多令牌）
    fn memory_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE lan_config (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                enabled INTEGER NOT NULL DEFAULT 0,
                port INTEGER NOT NULL DEFAULT 12345,
                read_only INTEGER NOT NULL DEFAULT 0,
                fail_count INTEGER NOT NULL DEFAULT 0,
                lock_until INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE lan_tokens (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL DEFAULT '',
                token_hash TEXT NOT NULL,
                token_salt TEXT NOT NULL,
                token_plain TEXT,
                expires_at INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                last_used_at INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO lan_config (id) VALUES (1);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn token_is_six_digits() {
        for _ in 0..200 {
            let t = generate_token();
            assert_eq!(t.len(), 6);
            assert!(t.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn verify_without_token_denied() {
        let conn = memory_conn();
        assert_eq!(
            verify_token(&conn, "123456"),
            AuthOutcome::Denied { retry_after_secs: 0 }
        );
    }

    #[test]
    fn verify_success_resets_failures() {
        let conn = memory_conn();
        let (_, token) = create_token(&conn, "手机", 0).unwrap();
        conn.execute(
            "UPDATE lan_config SET fail_count = 5, lock_until = 0 WHERE id = 1",
            [],
        )
        .unwrap();

        assert_eq!(verify_token(&conn, &token), AuthOutcome::Ok);
        let fail: i32 = conn
            .query_row("SELECT fail_count FROM lan_config WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fail, 0);
        // last_used_at 被记录
        let used: i64 = conn
            .query_row("SELECT last_used_at FROM lan_tokens WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert!(used > 0);
    }

    #[test]
    fn verify_multi_token_any_valid() {
        let conn = memory_conn();
        let (_, t1) = create_token(&conn, "手机", 0).unwrap();
        let (_, t2) = create_token(&conn, "平板", 0).unwrap();
        // 两个令牌都能通过
        assert_eq!(verify_token(&conn, &t1), AuthOutcome::Ok);
        assert_eq!(verify_token(&conn, &t2), AuthOutcome::Ok);
        // 未知令牌拒绝
        assert_eq!(
            verify_token(&conn, "999999"),
            AuthOutcome::Denied { retry_after_secs: 0 }
        );
    }

    #[test]
    fn verify_expired_token_denied() {
        let conn = memory_conn();
        // 创建已过期令牌（expires_at = now - 1）
        let now = now_secs();
        let token = generate_token();
        let salt = random_salt();
        let hash = hash_token(&salt, &token);
        conn.execute(
            "INSERT INTO lan_tokens (name, token_hash, token_salt, expires_at, created_at) \
             VALUES ('过期', ?1, ?2, ?3, ?4)",
            params![hash, salt, now - 1, now],
        )
        .unwrap();
        // 过期令牌：即使明文正确也拒绝
        assert_eq!(
            verify_token(&conn, &token),
            AuthOutcome::Denied { retry_after_secs: 0 }
        );
    }

    #[test]
    fn revoke_token_removes_authorization() {
        let conn = memory_conn();
        let (id, token) = create_token(&conn, "临时", 0).unwrap();
        assert_eq!(verify_token(&conn, &token), AuthOutcome::Ok);
        // 撤销后立即失效
        revoke_token(&conn, id).unwrap();
        assert_eq!(
            verify_token(&conn, &token),
            AuthOutcome::Denied { retry_after_secs: 0 }
        );
    }

    #[test]
    fn verify_failure_counts_and_locks() {
        let conn = memory_conn();
        let (_, token) = create_token(&conn, "测试", 0).unwrap();

        // 前 4 次失败：拒绝但不锁定
        for _ in 0..4 {
            assert_eq!(
                verify_token(&conn, "000000"),
                AuthOutcome::Denied { retry_after_secs: 0 }
            );
        }
        // 第 5 次失败：锁定（30s 起）
        let out = verify_token(&conn, "000000");
        let retry = match out {
            AuthOutcome::Denied { retry_after_secs } => retry_after_secs,
            _ => panic!("第 5 次失败应触发锁定"),
        };
        assert!(retry > 0 && retry <= 30, "锁定时长应在 (0, 30] 秒，实际 {retry}");

        // 锁定期间：即使正确 token 也被拒绝（返回剩余锁定秒数）
        let out2 = verify_token(&conn, &token);
        match out2 {
            AuthOutcome::Denied { retry_after_secs } => assert!(retry_after_secs > 0),
            _ => panic!("锁定期间应拒绝正确 token"),
        }

        // 锁定过期后：正确 token 通过并清零
        conn.execute("UPDATE lan_config SET lock_until = 0 WHERE id = 1", [])
            .unwrap();
        assert_eq!(verify_token(&conn, &token), AuthOutcome::Ok);
    }

    #[test]
    fn conv_buf_caps_by_bytes() {
        let mut cb = ConvBuf::default();
        let now = now_secs();
        // 单条超过字节上限 → 裁剪到上限内
        cb.push("chat-stream".to_string(), "x".repeat(BUF_MAX_BYTES + 4096), now);
        assert!(cb.bytes <= BUF_MAX_BYTES);
        // 多条累积也维持上限
        for i in 0..2000 {
            cb.push(
                "chat-stream".to_string(),
                format!("delta-{i}-{}", "y".repeat(200)),
                now,
            );
        }
        assert!(cb.bytes <= BUF_MAX_BYTES);
    }

    #[test]
    fn conv_buf_caps_by_age() {
        let mut cb = ConvBuf::default();
        let now = now_secs();
        // 过期的旧条目直接淘汰
        cb.push("chat-stream".to_string(), "old".to_string(), now - 1000);
        assert!(cb.entries.is_empty());
        // 新鲜条目保留，且 done 事件清空会话
        cb.push("chat-stream".to_string(), "new".to_string(), now);
        assert_eq!(cb.entries.len(), 1);
    }
}
