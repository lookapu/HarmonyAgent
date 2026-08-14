use tauri::{AppHandle, State, Manager};
use crate::db::{models::McpServer, queries, DbState};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct CreateMcpInput {
    pub name: String,
    pub server_type: Option<String>,
    pub command: Vec<String>,
    pub env: Option<serde_json::Value>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    /// 作用域：None=用户级(全局)；Some=仅该项目生效
    #[serde(default)]
    pub project_id: Option<String>,
}

/// 从 URL 获取到的 MCP 服务器草稿（待用户确认添加）
#[derive(Debug, Serialize)]
pub struct McpDraft {
    pub name: String,
    pub command: Vec<String>,
    pub env: Option<serde_json::Map<String, serde_json::Value>>,
    pub description: Option<String>,
}

/// 编辑 MCP 服务器入参（全量替换 name/command/env 等，用于修改连接配置）
#[derive(Debug, Deserialize)]
pub struct UpdateMcpInput {
    pub name: String,
    pub server_type: Option<String>,
    pub command: Vec<String>,
    pub env: Option<serde_json::Value>,
    pub description: Option<String>,
    pub homepage: Option<String>,
}

/// 从 HTTP(S) URL 获取 MCP 配置 JSON 并解析为服务器列表。
/// 支持标准结构 {"mcpServers":{"name":{"command":...,"args":...,"env":...}}} 或直接的对象。
#[tauri::command]
pub async fn fetch_mcp_from_url(
    url: String,
    use_proxy: Option<bool>,
) -> Result<Vec<McpDraft>, String> {
    let client = crate::utils::net::build_client(use_proxy.unwrap_or(false))?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("获取 URL 失败: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("URL 返回 {status}: {}", &text.chars().take(200).collect::<String>()));
    }
    let text = resp.text().await.map_err(|e| format!("读取响应失败: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("解析 JSON 失败: {e}"))?;
    let servers = json.get("mcpServers").unwrap_or(&json);
    let obj = servers
        .as_object()
        .ok_or_else(|| "URL 返回的内容不是有效的 MCP 配置（缺少 mcpServers 对象）".to_string())?;

    let mut drafts = Vec::new();
    for (name, v) in obj {
        // 跳过非服务器条目（如 schemaVersion / $schema）
        if v.get("command").is_none() {
            continue;
        }
        let mut command = Vec::new();
        if let Some(c) = v["command"].as_str() {
            command.push(c.to_string());
        }
        if let Some(args) = v["args"].as_array() {
            for a in args {
                if let Some(s) = a.as_str() {
                    command.push(s.to_string());
                }
            }
        }
        if command.is_empty() {
            continue;
        }
        drafts.push(McpDraft {
            name: name.clone(),
            command,
            env: v.get("env").and_then(|e| e.as_object().cloned()),
            description: v.get("description").and_then(|d| d.as_str().map(String::from)),
        });
    }
    if drafts.is_empty() {
        return Err("未从 URL 解析到任何 MCP 服务器（需包含 command 字段）".into());
    }
    Ok(drafts)
}

/// 编辑 MCP 服务器连接配置（名称/启动命令/环境变量等）
#[tauri::command]
pub fn update_mcp_server(
    db: State<DbState>,
    id: String,
    input: UpdateMcpInput,
) -> Result<McpServer, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::update_mcp_server(
        &conn,
        &id,
        &input.name,
        &input.server_type.unwrap_or_else(|| "local".to_string()),
        &serde_json::to_string(&input.command).unwrap_or_default(),
        &input.env.map(|v| v.to_string()).unwrap_or_else(|| "{}".to_string()),
        input.description.as_deref(),
        input.homepage.as_deref(),
    )
    .map_err(|e| e.to_string())?;
    queries::get_mcp_server(&conn, &id).map_err(|e| e.to_string())
}

/// 测试 MCP 服务器连接：以 stdio 方式启动进程并完成 initialize 握手。
/// MCP 运行机制：服务器是独立子进程（npx/docker 启动），客户端通过
/// stdin/stdout 按 "Content-Length 头 + JSON-RPC 2.0" 帧格式通信。
#[tauri::command]
pub async fn test_mcp_server(
    app: AppHandle,
    db: State<'_, DbState>,
    id: String,
) -> Result<String, String> {
    let result = test_mcp_server_inner(&app, &db, &id).await;
    // 持久化最近一次测试结果（MCP 页据此展示"正常/异常/未测试"状态）
    if let Ok(conn) = db.0.lock() {
        match &result {
            Ok(_) => {
                let _ = queries::update_mcp_test_result(&conn, &id, true, None);
            }
            Err(e) => {
                let _ = queries::update_mcp_test_result(&conn, &id, false, Some(e));
            }
        }
    }
    result
}

async fn test_mcp_server_inner(
    app: &AppHandle,
    db: &State<'_, DbState>,
    id: &str,
) -> Result<String, String> {
    let server = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        queries::get_mcp_server(&conn, id).map_err(|e| e.to_string())?
    };

    let command: Vec<String> = serde_json::from_str(&server.command)
        .unwrap_or_else(|_| vec![server.command.clone()]);
    let env: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&server.env).unwrap_or_default();
    if command.is_empty() {
        return Err("启动命令为空".into());
    }

    let mut cmd = crate::utils::process::command(&command[0], &command[1..])?;
    // MCP 子进程公共环境：移除 NODE_TLS_REJECT_UNAUTHORIZED 污染 + npx 独立 npm 缓存
    // （测试与常驻连接并发 npx 时不再互相踩全局缓存导致 EPERM）；用户显式 env 后应用，可覆盖默认值
    crate::utils::process::apply_mcp_child_env(&mut cmd, &command[0], id)?;
    for (k, v) in &env {
        if let Some(s) = v.as_str() {
            cmd.env(k, s);
        }
    }
    // 注入系统代理：npx/npm 首次拉取 MCP 依赖包时走代理（npm 原生读这些环境变量）。
    // 用户显式配置的 env 优先——已设置代理变量时不再覆盖。
    if !env.contains_key("HTTP_PROXY") && !env.contains_key("HTTPS_PROXY")
        && !env.contains_key("http_proxy") && !env.contains_key("https_proxy")
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
    let mut stdin = child.stdin.take().ok_or("无法获取服务器 stdin")?;
    let mut stdout = child.stdout.take().ok_or("无法获取服务器 stdout")?;
    let mut stderr = child.stderr.take();
    let mut err_buf: Vec<u8> = Vec::new();

    // 发送一帧 JSON-RPC 请求（按指定帧模式编码）；自由函数避免闭包跨 await 的 lifetime 问题
    async fn send_mcp_request(
        stdin: &mut tokio::process::ChildStdin,
        v: &serde_json::Value,
        mode: crate::utils::mcp_frames::FrameMode,
    ) -> Result<(), String> {
        let body = serde_json::to_string(v).map_err(|e| e.to_string())?;
        let frame = crate::utils::mcp_frames::encode_frame(&body, mode);
        stdin
            .write_all(&frame)
            .await
            .map_err(|e| format!("发送请求失败: {e}"))?;
        stdin.flush().await.map_err(|e| e.to_string())?;
        Ok(())
    }

    // 失败时终止整个子进程树（npx 包装的孙进程同样清理）并附 stderr 尾部诊断
    // （自由函数而非闭包：闭包捕获 &mut child 跨 await 存在 lifetime 冲突）
    async fn fail_mcp_test(
        child: &mut tokio::process::Child,
        e: String,
        err_buf: &[u8],
    ) -> Result<String, String> {
        let _ = child.kill().await;
        let _ = child.wait().await;
        crate::utils::process::kill_tree(child.id());
        Err::<String, String>(format!("{e}{}", stderr_tail(err_buf)))
    }

    // 1. initialize 握手（60s 总预算：首次 npx 拉取依赖可能较慢；绝对 deadline，
    //    stderr 持续输出（npm 下载进度等）不会重置计时）。
    //    stdio 帧格式先按 newline-delimited JSON（2025-06-18 起官方规范）探测，
    //    无响应则回退 Content-Length 帧（2025-03-26 时期规范）重发。
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "deveco-switch", "version": "0.1.0" }
        }
    });
    let handshake_deadline =
        tokio::time::Instant::now() + tokio::time::Duration::from_secs(60);
    use crate::utils::mcp_frames::FrameMode;
    let probe_deadline =
        tokio::time::Instant::now() + tokio::time::Duration::from_secs(6);
    let mut frame_mode = FrameMode::Ndjson;
    let init = match send_mcp_request(&mut stdin, &req, frame_mode).await {
        Ok(()) => read_mcp_frame(&mut stdout, &mut stderr, &mut err_buf, probe_deadline).await,
        Err(e) => Err(e),
    };
    let init = match init {
        Ok(v) => Ok(v),
        // NDJSON 探测超时：回退 Content-Length 帧重发（服务器可能是 2025-03-26 时期旧包）
        Err(e) if e == "等待响应超时" => {
            frame_mode = FrameMode::ContentLength;
            send_mcp_request(&mut stdin, &req, frame_mode).await?;
            read_mcp_frame(&mut stdout, &mut stderr, &mut err_buf, handshake_deadline).await
        }
        Err(e) => Err(e),
    };
    let init = match init {
        Ok(v) => v,
        Err(e) => {
            return fail_mcp_test(
                &mut child,
                format!("连接超时或握手失败（60s）：首次 npx 拉取依赖可能较慢，请检查网络/代理后重试。{e}"),
                &err_buf,
            )
            .await;
        }
    };
    if let Some(err) = init.get("error") {
        return fail_mcp_test(&mut child, format!("服务器返回错误: {err}"), &err_buf).await;
    }
    let result = init.get("result").ok_or("响应缺少 result 字段")?;
    let proto_ver = result
        .get("protocolVersion")
        .and_then(|x| x.as_str())
        .unwrap_or("?");
    let server_info = result.get("serverInfo").unwrap_or(&serde_json::Value::Null);

    // 2. 通知服务器已初始化完成（MCP 规范要求），随后 tools/list 验证工具确实可用
    let _ = send_mcp_request(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
        frame_mode,
    )
    .await;
    let list_deadline =
        tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
    let list = match send_mcp_request(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        frame_mode,
    )
    .await
    {
        Ok(()) => read_mcp_frame(&mut stdout, &mut stderr, &mut err_buf, list_deadline).await,
        Err(e) => Err(e),
    };
    let list = match list {
        Ok(v) => v,
        Err(e) => {
            return fail_mcp_test(
                &mut child,
                format!("握手成功但列出工具失败（服务器未响应 tools/list）: {e}"),
                &err_buf,
            )
            .await;
        }
    };
    let tools: Vec<String> = list
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // 3. 测试完毕终止整个子进程树，返回成功信息
    crate::utils::process::kill_tree(child.id());
    let _ = child.wait().await;

    // 握手成功：清除失败标记（该服务器恢复参与对话工具注入）
    let manager = app.state::<crate::services::mcp_manager::McpManager>();
    manager.mark_connected(&server.id);
    let tools_txt = if tools.is_empty() {
        "未返回工具列表".to_string()
    } else {
        format!("工具 {} 个：{}", tools.len(), tools.join(", "))
    };
    Ok(format!(
        "连接成功 ✓  协议 v{proto_ver}  服务器 {server_info}\n{tools_txt}"
    ))
}

/// 读取一帧 MCP 响应（自动识别 newline-delimited JSON 与 Content-Length 帧，
/// 跳过通知帧）。
/// 关键：使用绝对 deadline（sleep_until 分支），服务器 stderr 持续输出
/// （如 npm 下载进度、日志）不会重置超时计时，避免"测试永远转圈"。
async fn read_mcp_frame(
    stdout: &mut tokio::process::ChildStdout,
    stderr: &mut Option<tokio::process::ChildStderr>,
    err_buf: &mut Vec<u8>,
    deadline: tokio::time::Instant,
) -> Result<serde_json::Value, String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    let mut err_tmp = [0u8; 4096];
    // 垃圾行计数：stdout 上的非 JSON 行（进程警告/横幅）容忍跳过，
    // 但持续大量输出说明启动的进程根本不是 MCP 服务器，直接报错避免干等超时
    let mut skip_lines = 0usize;
    loop {
        // 缓冲区内先尝试解析一帧；通知帧（无 id，如日志/进度）跳过继续读
        if let Some(parsed) = crate::utils::mcp_frames::try_parse_frame(&buf) {
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
                    if v.get("id").is_some() {
                        return Ok(v);
                    }
                    // 通知帧：丢弃并继续等待响应帧
                }
                Err(e) => return Err(e),
            }
        }
        tokio::select! {
            r = stdout.read(&mut tmp) => {
                let n = r.map_err(|e| format!("读取响应失败: {e}"))?;
                if n == 0 {
                    return Err("服务器进程提前退出（stdout EOF）".into());
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            r = async {
                match stderr.as_mut() {
                    Some(e) => e.read(&mut err_tmp).await,
                    None => Ok(0),
                }
            } => {
                let n = r.unwrap_or(0);
                if n > 0 {
                    err_buf.extend_from_slice(&err_tmp[..n]);
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                return Err("等待响应超时".into());
            }
        }
    }
}

/// stderr 尾部 400 字符（失败时附在错误后帮助定位，如 npm 报错、缺依赖等）
fn stderr_tail(err_buf: &[u8]) -> String {
    let s = String::from_utf8_lossy(err_buf);
    let s = s.trim();
    if s.is_empty() {
        String::new()
    } else {
        let tail: String = s
            .chars()
            .rev()
            .take(400)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        format!("\n服务器输出: {tail}")
    }
}

#[tauri::command]
pub fn list_mcp_servers(db: State<DbState>, project_id: Option<String>) -> Result<Vec<McpServer>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::list_mcp_servers(&conn, project_id.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_mcp_server(db: State<DbState>, input: CreateMcpInput) -> Result<McpServer, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();

    let server = McpServer {
        id: Uuid::new_v4().to_string(),
        name: input.name,
        server_type: input.server_type.unwrap_or_else(|| "local".to_string()),
        command: serde_json::to_string(&input.command).unwrap_or_default(),
        args: "[]".to_string(),
        env: input.env.map(|v| v.to_string()).unwrap_or_else(|| "{}".to_string()),
        enabled: true,
        description: input.description,
        homepage: input.homepage,
        created_at: now,
        last_test_ok: None,
        last_test_at: None,
        last_test_error: None,
        project_id: input.project_id,
    };

    queries::insert_mcp_server(&conn, &server).map_err(|e| e.to_string())?;
    Ok(server)
}

#[tauri::command]
pub fn toggle_mcp_server(db: State<DbState>, id: String, enabled: bool) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::toggle_mcp_server(&conn, &id, enabled).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_mcp_server(
    db: State<DbState>,
    manager: State<'_, crate::services::mcp_manager::McpManager>,
    id: String,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::delete_mcp_server(&conn, &id).map_err(|e| e.to_string())?;
    // 断开已缓存连接：删除后立即终止子进程，不残留到应用退出
    manager.disconnect(&[id]);
    Ok(())
}

/// 把一个 MCP 服务器复制到另一作用域（全局↔当前项目）。
/// 用于"全局配置在本项目单独定制"或"项目配置提升为全局共享"。
/// 同名多实例合法（如多个 mysql 连接）：同名复制后按 id 排序自动编号（mysql#1、mysql#2）。
#[tauri::command]
pub fn clone_mcp_server(
    db: State<DbState>,
    id: String,
    target_project_id: Option<String>,
) -> Result<McpServer, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut src = queries::get_mcp_server(&conn, &id).map_err(|e| e.to_string())?;

    if src.project_id == target_project_id {
        return Err("目标作用域与源配置相同".to_string());
    }

    src.id = Uuid::new_v4().to_string();
    src.project_id = target_project_id;
    src.enabled = true;
    src.last_test_ok = None;
    src.last_test_at = None;
    src.last_test_error = None;
    src.created_at = chrono::Utc::now().timestamp();
    queries::insert_mcp_server(&conn, &src).map_err(|e| e.to_string())?;
    Ok(src)
}

/// 可移植的 MCP 配置（导出/导入用，剔除 id/时间戳/健康状态）
#[derive(Debug, Serialize, Deserialize)]
pub struct PortableMcp {
    pub name: String,
    #[serde(default = "default_local")]
    pub server_type: String,
    /// command 为可执行程序，args 为参数（导入时合并为一条命令数组）
    pub command: Vec<String>,
    #[serde(default)]
    pub env: serde_json::Value,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
}
fn default_local() -> String { "local".into() }

/// 导出指定作用域的 MCP 配置为 JSON 字符串（可分享/备份）
#[tauri::command]
pub fn export_mcp_config(db: State<DbState>, project_id: Option<String>) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let list = queries::list_mcp_servers(&conn, project_id.as_deref()).map_err(|e| e.to_string())?;
    let portable: Vec<PortableMcp> = list
        .into_iter()
        .filter(|s| s.project_id == project_id)
        .map(|s| {
            let prog: Vec<String> = serde_json::from_str(&s.command).unwrap_or_default();
            let args: Vec<String> = serde_json::from_str(&s.args).unwrap_or_default();
            let mut command = prog;
            command.extend(args);
            let env: serde_json::Value = serde_json::from_str(&s.env).unwrap_or(serde_json::json!({}));
            PortableMcp {
                name: s.name,
                server_type: s.server_type,
                command,
                env,
                description: s.description,
                homepage: s.homepage,
            }
        })
        .collect();
    serde_json::to_string_pretty(&portable).map_err(|e| e.to_string())
}

/// 从 JSON 导入 MCP 配置到指定作用域。overwrite=true 时同名覆盖，否则跳过已存在的。
/// 返回成功导入的数量。
#[tauri::command]
pub fn import_mcp_config(
    db: State<DbState>,
    json: String,
    target_project_id: Option<String>,
    overwrite: Option<bool>,
) -> Result<usize, String> {
    let items: Vec<PortableMcp> = serde_json::from_str(&json).map_err(|e| format!("JSON 解析失败: {e}"))?;
    let overwrite = overwrite.unwrap_or(false);
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let existing = queries::list_mcp_servers(&conn, target_project_id.as_deref()).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();
    let mut imported = 0usize;
    for item in items {
        let same_name = existing.iter().find(|s| s.name.eq_ignore_ascii_case(&item.name) && s.project_id == target_project_id);
        if let Some(s) = same_name {
            if overwrite {
                queries::delete_mcp_server(&conn, &s.id).map_err(|e| e.to_string())?;
            } else {
                continue;
            }
        }
        let (prog, args) = split_command(item.command);
        let server = McpServer {
            id: Uuid::new_v4().to_string(),
            name: item.name,
            server_type: if item.server_type.is_empty() { "local".into() } else { item.server_type },
            command: serde_json::to_string(&prog).unwrap_or_else(|_| "[]".into()),
            args: serde_json::to_string(&args).unwrap_or_else(|_| "[]".into()),
            env: item.env.to_string(),
            enabled: true,
            description: item.description,
            homepage: item.homepage,
            created_at: now,
            last_test_ok: None,
            last_test_at: None,
            last_test_error: None,
            project_id: target_project_id.clone(),
        };
        queries::insert_mcp_server(&conn, &server).map_err(|e| e.to_string())?;
        imported += 1;
    }
    Ok(imported)
}

/// 把命令数组拆成 (可执行程序, 参数)
fn split_command(mut cmd: Vec<String>) -> (Vec<String>, Vec<String>) {
    if cmd.is_empty() {
        (vec![], vec![])
    } else {
        let args = cmd.split_off(1);
        (cmd, args)
    }
}
