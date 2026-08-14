//! MCP stdio 客户端：启动 MCP 服务器子进程，按 JSON-RPC 2.0 over stdio 通信。
//! 支持两种官方 stdio 帧格式：newline-delimited JSON（2025-06-18 起规范）与
//! Content-Length 帧（2025-03-26 时期规范）；连接时自动探测并固定后续帧模式。
//!
//! 生命周期：每台服务器一个长驻子进程（跨会话复用），应用退出时由
//! McpManager::shutdown_all 统一终止；进程异常退出时 call 报错并移除连接，
//! 下次调用重新拉起。

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::db::models::McpServer;
use crate::utils::mcp_frames::{encode_frame, try_parse_frame, FrameMode};

/// MCP 工具定义（tools/list 返回，注入 Agent 工具清单）
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// MCP 服务器子进程客户端（请求串行化：帧读写非并发安全）
pub struct McpClient {
    pub server_id: String,
    pub server_name: String,
    /// 作用域：None=用户级（全局），Some=仅该项目（调用路由时优先匹配）
    pub project_id: Option<String>,
    child: Child,
    /// 内部可变性：request 通过共享引用读写（Arc 持有 + 串行锁）
    stdin: tokio::sync::Mutex<ChildStdin>,
    stdout: tokio::sync::Mutex<BufReader<ChildStdout>>,
    /// stderr 尾部收集（进程异常退出时附上诊断）
    stderr_tail: Arc<StdMutex<String>>,
    /// 串行化请求（同一时刻只处理一个请求/响应）
    lock: tokio::sync::Mutex<()>,
    next_id: AtomicU64,
    /// 单次请求超时（首次拉包慢，握手用长超时，后续用短超时）
    request_timeout: std::time::Duration,
    /// 已探测到的 stdio 帧模式（0=未知，1=NDJSON，2=Content-Length）
    frame_mode: AtomicU8,
}

impl McpClient {
    /// 启动 MCP 服务器子进程并完成 initialize 握手。
    /// 启动命令解析、内置 Node 兜底、系统代理注入与 MCP 页测试连接保持一致。
    pub async fn connect(server: &McpServer) -> Result<Self, String> {
        let command: Vec<String> = serde_json::from_str(&server.command)
            .unwrap_or_else(|_| vec![server.command.clone()]);
        let env: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&server.env).unwrap_or_default();
        if command.is_empty() {
            return Err("启动命令为空".into());
        }

        let mut cmd = crate::utils::process::command(&command[0], &command[1..])?;
        // MCP 子进程公共环境：移除 NODE_TLS_REJECT_UNAUTHORIZED 污染 + npx 独立 npm 缓存
        // （与 MCP 页测试连接共用同一缓存目录，避免多进程并发写全局缓存导致 EPERM）；
        // 用户显式 env 后应用，可覆盖默认值
        crate::utils::process::apply_mcp_child_env(&mut cmd, &command[0], &server.id)?;
        for (k, v) in &env {
            if let Some(s) = v.as_str() {
                cmd.env(k, s);
            }
        }
        // 注入系统代理：npx/npm 首次拉取 MCP 依赖包时走代理（npm 原生读这些环境变量）。
        // 用户显式配置的 env 优先——已设置代理变量时不再覆盖。
        if !env.contains_key("HTTP_PROXY")
            && !env.contains_key("HTTPS_PROXY")
            && !env.contains_key("http_proxy")
            && !env.contains_key("https_proxy")
        {
            if let Some(proxy) = crate::utils::net::read_system_proxy() {
                for var in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
                    cmd.env(var, &proxy);
                }
            }
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| format!("启动 MCP 服务器失败: {e}"))?;
        let stdin = child.stdin.take().ok_or("无法获取服务器 stdin")?;
        let stdout = child.stdout.take().ok_or("无法获取服务器 stdout")?;

        // stderr 后台收集（尾部 2KB，进程退出时提供诊断）
        let stderr_tail = Arc::new(StdMutex::new(String::new()));
        if let Some(mut stderr) = child.stderr.take() {
            let tail = stderr_tail.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let mut collected = String::new();
                loop {
                    match stderr.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            collected.push_str(&String::from_utf8_lossy(&buf[..n]));
                            if collected.chars().count() > 2000 {
                                let drop_n = collected.chars().count() - 2000;
                                collected = collected.chars().skip(drop_n).collect();
                            }
                        }
                    }
                }
                *tail.lock().unwrap() = collected;
            });
        }

        let client = Self {
            server_id: server.id.clone(),
            server_name: server.name.clone(),
            project_id: server.project_id.clone(),
            child,
            stdin: tokio::sync::Mutex::new(stdin),
            stdout: tokio::sync::Mutex::new(BufReader::new(stdout)),
            stderr_tail,
            lock: tokio::sync::Mutex::new(()),
            next_id: AtomicU64::new(1),
            frame_mode: AtomicU8::new(0),
            // 首次 npx 拉取依赖可能较慢，握手用稍长超时；
            // 对话主流程外层另有 12s 总超时护栏，单次请求再长也不会阻塞对话
            request_timeout: std::time::Duration::from_secs(20),
        };
        // initialize 握手（首次 npx 拉包可能较慢，用长超时）
        client
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "deveco-switch", "version": env!("CARGO_PKG_VERSION") }
                }),
            )
            .await
            .map_err(|e| format!("MCP 握手失败: {e}"))?;
        // 缩短后续请求超时（进程已就绪，无需长等待）
        let mut c = client;
        c.request_timeout = std::time::Duration::from_secs(30);
        Ok(c)
    }

    /// 拉取服务器可用工具列表
    pub async fn list_tools(&self) -> Result<Vec<McpToolDef>, String> {
        let v = self.request("tools/list", json!({})).await?;
        let tools = v["result"]["tools"]
            .as_array()
            .ok_or("tools/list 响应缺少 result.tools")?;
        Ok(tools
            .iter()
            .filter_map(|t| {
                let name = t["name"].as_str()?.to_string();
                Some(McpToolDef {
                    name,
                    description: t["description"].as_str().unwrap_or("").to_string(),
                    input_schema: t.get("inputSchema").cloned().unwrap_or(Value::Null),
                })
            })
            .collect())
    }

    /// 调用服务器工具，返回文本化结果（content 数组按类型拼接）
    pub async fn call_tool(&self, tool: &str, args: Value) -> Result<String, String> {
        let v = self
            .request("tools/call", json!({ "name": tool, "arguments": args }))
            .await?;
        if let Some(err) = v.get("error") {
            return Err(format!("服务器返回错误: {err}"));
        }
        let result = &v["result"];
        // isError=true 时内容为错误说明，同样提取文本返回（上层按失败处理）
        let text = extract_result_text(result);
        if result["isError"] == json!(true) {
            return Err(if text.is_empty() {
                "服务器执行工具失败（无详细错误信息）".to_string()
            } else {
                text
            });
        }
        if text.is_empty() {
            return Err("服务器返回空结果".into());
        }
        Ok(text)
    }

    /// 发送 JSON-RPC 请求并等待响应（串行化 + 帧模式自动探测 + 超时 + 进程退出检测）
    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let _g = self.lock.lock().await;
        // 帧模式未知时按 NDJSON 探测（2025-06-18 起官方规范，覆盖面最广）
        let mode = FrameMode::from_code(self.frame_mode.load(Ordering::SeqCst))
            .unwrap_or(FrameMode::Ndjson);

        let send_and_read = async {
            let mut stdin = self.stdin.lock().await;
            let mut stdout = self.stdout.lock().await;
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
            let body_str = body.to_string();
            stdin
                .write_all(&encode_frame(&body_str, mode))
                .await
                .map_err(|e| format!("写入请求失败: {e}"))?;
            stdin
                .flush()
                .await
                .map_err(|e| format!("刷新请求失败: {e}"))?;
            read_response_frame(&mut stdout, &id, &self.stderr_tail).await
        };

        match tokio::time::timeout(self.request_timeout, send_and_read).await {
            Ok(result) => {
                // 无论成败，本次尝试使用的模式即服务器所接受的模式
                self.frame_mode.store(mode.code(), Ordering::SeqCst);
                result
            }
            Err(_) if mode == FrameMode::Ndjson => {
                // NDJSON 探测超时：回退 Content-Length 帧重试
                // （服务器可能是 2025-03-26 时期旧包，仅支持帧协议）
                let mut stdin = self.stdin.lock().await;
                let mut stdout = self.stdout.lock().await;
                let id = self.next_id.fetch_add(1, Ordering::SeqCst);
                let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
                let body_str = body.to_string();
                stdin
                    .write_all(&encode_frame(&body_str, FrameMode::ContentLength))
                    .await
                    .map_err(|e| format!("写入请求失败: {e}"))?;
                stdin
                    .flush()
                    .await
                    .map_err(|e| format!("刷新请求失败: {e}"))?;
                let r = tokio::time::timeout(
                    self.request_timeout,
                    read_response_frame(&mut stdout, &id, &self.stderr_tail),
                )
                .await
                .map_err(|_| format!("MCP 请求超时（>{s}s）", s = self.request_timeout.as_secs()))?;
                self.frame_mode.store(FrameMode::ContentLength.code(), Ordering::SeqCst);
                r
            }
            Err(_) => Err(format!("MCP 请求超时（>{s}s）", s = self.request_timeout.as_secs())),
        }
    }
}

/// 读取一帧响应：自动识别 NDJSON 与 Content-Length 帧，跳过通知帧，校验响应 id。
async fn read_response_frame(
    stdout: &mut BufReader<ChildStdout>,
    id: &u64,
    stderr_tail: &Arc<StdMutex<String>>,
) -> Result<Value, String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    // 垃圾行计数：stdout 上的非 JSON 行（进程警告/横幅）容忍跳过，
    // 持续大量输出说明启动的进程不是 MCP 服务器，直接报错避免干等超时
    let mut skip_lines = 0usize;
    loop {
        if let Some(parsed) = try_parse_frame(&buf) {
            match parsed {
                Ok((v, used)) => {
                    buf.drain(..used);
                    if v.is_null() {
                        skip_lines += 1;
                        if skip_lines > 200 {
                            return Err("stdout 持续输出非 JSON-RPC 内容（启动的进程可能不是 MCP 服务器，或命令有误）".into());
                        }
                        continue;
                    }
                    // 通知帧（无 id，如日志/进度通知）跳过，继续等响应
                    if v.get("id").is_none() {
                        continue;
                    }
                    if v["id"] != json!(id) {
                        return Err(format!("响应 id 不匹配（期望 {id}，收到 {}）", v["id"]));
                    }
                    return Ok(v);
                }
                Err(e) => return Err(e),
            }
        }
        let n = stdout
            .read(&mut tmp)
            .await
            .map_err(|e| format!("读取响应失败: {e}"))?;
        if n == 0 {
            return Err(format!(
                "服务器进程已退出，未收到响应{}",
                stderr_suffix(stderr_tail)
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // 强杀整个子进程树（start_kill 只杀直接子进程；npx 包装的服务器孙进程会残留，
        // 继续占着管道导致握手无响应/进程堆积）
        crate::utils::process::kill_tree(self.child.id());
    }
}

/// 进程退出诊断后缀（stderr 尾部非空时附带）
fn stderr_suffix(tail: &Arc<StdMutex<String>>) -> String {
    let s = tail.lock().map(|g| g.clone()).unwrap_or_default();
    let s = s.trim();
    if s.is_empty() {
        String::new()
    } else {
        let tail: String = s.chars().rev().take(400).collect::<String>().chars().rev().collect();
        format!("\n服务器输出: {tail}")
    }
}

/// 从 tools/call 结果中提取文本：
/// content 数组的 text 项拼接 → structuredContent JSON → 原始结果兜底
fn extract_result_text(result: &Value) -> String {
    let mut s = String::new();
    if let Some(arr) = result["content"].as_array() {
        for item in arr {
            match item["type"].as_str() {
                Some("text") | None => {
                    // text 类型（或未声明类型但带 text 字段）
                    if let Some(t) = item["text"].as_str() {
                        s.push_str(t);
                        s.push('\n');
                    }
                }
                Some(other) => {
                    // 非文本内容（image/resource 等）仅提示存在
                    s.push_str(&format!("[{other} 内容]\n"));
                }
            }
        }
    }
    if s.trim().is_empty() {
        if let Some(sc) = result.get("structuredContent") {
            s = sc.to_string();
        }
    }
    if s.trim().is_empty() {
        s = result.to_string();
    }
    if s.chars().count() > 8000 {
        s = s.chars().take(8000).collect::<String>() + "\n…(结果已截断)";
    }
    s
}
