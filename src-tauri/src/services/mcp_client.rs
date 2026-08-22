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
    pub async fn connect(server: &McpServer, project_root: &std::path::Path) -> Result<Self, String> {
        let command: Vec<String> = serde_json::from_str(&server.command)
            .unwrap_or_else(|_| vec![server.command.clone()]);
        if command.is_empty() {
            return Err("启动命令为空".into());
        }

        let mut cmd = crate::utils::process::command(&command[0], &command[1..])?;
        crate::services::mcp_policy::configure_child_environment(&mut cmd, server, &command[0])?;
        cmd.current_dir(project_root);
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
            Err(_) => {
                // 不再在同一管道回退 Content-Length 重发：超时通常是服务器未就绪
                // （npx 冷下载/初始化慢），并非帧格式不兼容；同一管道混发两种帧格式
                // 会打崩 SDK 1.x 的按行 JSON 解析器（Content-Length 残留 → JSON.parse
                // 崩溃 → 服务器进程存活但后续请求永久无响应）。request 为 &self 无法
                // 检测子进程状态，直接报超时，由上层决定重连（重连会重新探测帧模式）。
                Err(format!(
                    "MCP 请求超时（>{s}s）：服务器未就绪或无响应，首次 npx 拉取依赖可能较慢，请稍后重试",
                    s = self.request_timeout.as_secs()
                ))
            }
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

/// 进程退出诊断后缀（stderr 尾部非空时附带；2000 字符避免截断 Node 堆栈关键帧）
fn stderr_suffix(tail: &Arc<StdMutex<String>>) -> String {
    let s = tail.lock().map(|g| g.clone()).unwrap_or_default();
    let s = s.trim();
    if s.is_empty() {
        String::new()
    } else {
        let tail: String = s.chars().rev().take(2000).collect::<String>().chars().rev().collect();
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
