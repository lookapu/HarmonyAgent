//! LSP 深度集成骨架：@arkts/language-server 的会话级 stdio 客户端
//!
//! 背景：传统 get_symbol_details/codebase_search 是"文本扫描"（正则+索引），拿不到真实 AST。
//! @arkts/language-server（ohosvscode/arkTS 社区项目，npm 安装）基于 Volar 2.4，能提供
//! 与 DevEco Studio 一致的 ArkTS 语言能力（跳转定义/引用/悬停/补全/诊断/符号树）。
//! 本模块以 stdio JSON-RPC 直连该语言服务器（无需额外 crate），按会话懒启动一个常驻子进程：
//! - `lsp_definition`：跳转定义（含 SDK .d.ts 内置组件声明）
//! - `lsp_references`：查找引用
//! - `lsp_symbols`：文档符号树（struct/方法/状态变量）
//! - `lsp_hover`：符号文档（API 说明）
//! - `lsp_diagnostics`：真实类型检查/语法诊断（跨文件模块解析）
//!
//! SDK 路径约定：服务器要求 initializationOptions.ets.sdkPath 指向鸿蒙 SDK 的
//! openharmony 组件目录（缺省为 <SDK 根>/default/openharmony），hmsPath 指向 hms 目录。
//! 发现顺序：DEVECO_SDK_HOME 环境变量 → DevEco Studio 常见安装路径 → 用户目录 Huawei/Sdk。
//! 本机缺 Node/LSP 包时工具返回可读错误并提示用 deveco-mcp 模板（MCP 快集成）兜底。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, oneshot};

/// 会话级连接池：conversation_id → LSP 连接（懒启动，进程常驻至会话结束）
static POOL: OnceLock<StdMutex<HashMap<String, Arc<LspConnection>>>> = OnceLock::new();

fn pool() -> &'static StdMutex<HashMap<String, Arc<LspConnection>>> {
    POOL.get_or_init(|| StdMutex::new(HashMap::new()))
}

// ---------------- SDK / Node 路径发现 ----------------

/// 定位鸿蒙 SDK 的 openharmony 组件目录（sdkPath 应指向含 ets/ 子目录的一层）
fn discover_sdk() -> Option<(String, String)> {
    let candidates: Vec<PathBuf> = {
        let mut v = Vec::new();
        if let Ok(home) = std::env::var("DEVECO_SDK_HOME") {
            v.push(PathBuf::from(home).join("default").join("openharmony"));
        }
        if let Some(up) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
            let up = PathBuf::from(up);
            v.push(up.join("AppData").join("Local").join("Huawei").join("Sdk").join("default").join("openharmony"));
        }
        v.push(PathBuf::from(r"C:\Program Files\Huawei\DevEco Studio\sdk\default\openharmony"));
        v.push(PathBuf::from(r"D:\Huawei\DevEco Studio\sdk\default\openharmony"));
        v.push(PathBuf::from(r"D:\DevEco Studio\sdk\default\openharmony"));
        v
    };
    for sdk in candidates {
        if sdk.join("ets").join("component").exists() {
            let hms = sdk.parent().and_then(|p| p.join("hms").is_dir().then(|| p.join("hms").to_string_lossy().into_owned()));
            let sdk_str = sdk.to_string_lossy().replace('\\', "/");
            return Some((sdk_str, hms.unwrap_or_default()));
        }
    }
    None
}

/// Node 可执行：优先 PATH，其次便携版 resources/node
fn node_cmd() -> String {
    if which_node().is_some() {
        return "node".to_string();
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("resources").join("node").join("node.exe");
            if p.exists() {
                return p.to_string_lossy().into_owned();
            }
        }
    }
    "node".to_string()
}

fn which_node() -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        for name in ["node.exe", "node"] {
            let cand = dir.join(name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// 定位 @arkts/language-server 入口 js：优先便携包 resources 内嵌，其次全局 npm 根
fn lsp_entry() -> Option<PathBuf> {
    // 1) 便携版内置（打包脚本把 node_modules 放进 resources）
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("resources").join("lsp").join("@arkts").join("language-server").join("bin").join("ets-language-server.js");
            if p.exists() {
                return Some(p);
            }
        }
    }
    // 2) 全局 npm 安装（npm i -g @arkts/language-server 后）
    let npm_root = global_npm_root()?;
    let p = npm_root.join("@arkts").join("language-server").join("bin").join("ets-language-server.js");
    p.is_file().then_some(p)
}

fn global_npm_root() -> Option<PathBuf> {
    // 尝试 `npm root -g`；失败时按常见布局猜测
    for cand in [
        dirs_user_profile()?.join("AppData").join("Roaming").join("npm").join("node_modules"),
        PathBuf::from(r"C:\Program Files\nodejs\node_modules"),
    ] {
        if cand.join("@arkts").exists() {
            return Some(cand);
        }
    }
    None
}

fn dirs_user_profile() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

// ---------------- LSP 连接 ----------------

/// 单个 LSP 连接：子进程 + 写锁 + 请求应答表 + 诊断缓存
struct LspConnection {
    child: StdMutex<Child>,
    writer: Mutex<ChildStdin>,
    next_id: AtomicI64,
    pending: StdMutex<HashMap<i64, oneshot::Sender<Value>>>,
    diag_cache: StdMutex<HashMap<String, (i64, Vec<Value>)>>, // uri → (version, diagnostics)
    initialized: tokio::sync::OnceCell<()>,
}

impl LspConnection {
    fn new(child: Child, stdout: ChildStdout, stdin: ChildStdin) -> Arc<Self> {
        let conn = Arc::new(LspConnection {
            child: StdMutex::new(child),
            writer: Mutex::new(stdin),
            next_id: AtomicI64::new(1),
            pending: StdMutex::new(HashMap::new()),
            diag_cache: StdMutex::new(HashMap::new()),
            initialized: tokio::sync::OnceCell::new(),
        });
        // 读循环：解析帧 → 应答分发 / 诊断缓存 / showMessageRequest 自动回复
        {
            let conn = conn.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout);
                loop {
                    let Some(len) = read_frame_head(&mut reader).await else { break };
                    let mut body = vec![0u8; len];
                    if reader.read_exact(&mut body).await.is_err() {
                        break;
                    }
                    let Ok(msg) = serde_json::from_slice::<Value>(&body) else { continue };
                    conn.dispatch(msg).await;
                }
                conn.pending.lock().unwrap().clear();
            });
        }
        conn
    }

    async fn dispatch(self: &Arc<Self>, msg: Value) {
        if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
            match method {
                // 服务器提问：一律回复空（不阻断协议）
                "window/showMessageRequest" => {
                    let _ = self.send_msg(&json!({"jsonrpc": "2.0", "id": msg["id"].clone(), "result": null})).await;
                }
                // 诊断推送：写入缓存（didOpen/didChange 后由 lsp_diagnostics 读取）
                "textDocument/publishDiagnostics" => {
                    if let (Some(uri), Some(diags)) = (
                        msg["params"]["uri"].as_str().map(String::from),
                        msg["params"]["diagnostics"].as_array().cloned(),
                    ) {
                        let version = msg["params"].get("version").and_then(|v| v.as_i64()).unwrap_or(0);
                        self.diag_cache.lock().unwrap().insert(uri, (version, diags));
                    }
                }
                _ => {}
            }
            return;
        }
        // response：按 id 投递给等待方
        if let Some(id) = msg.get("id").and_then(|i| i.as_i64()) {
            if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
                let _ = tx.send(msg);
            }
        }
    }

    async fn send_msg(self: &Arc<Self>, msg: &Value) -> Result<(), String> {
        let body = serde_json::to_vec(msg).map_err(|e| format!("序列化 LSP 请求失败: {e}"))?;
        let mut w = self.writer.lock().await;
        w.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
            .await
            .map_err(|e| format!("写入 LSP 管道失败（Node 是否可用？）: {e}"))?;
        w.write_all(&body).await.map_err(|e| format!("写入 LSP 管道失败: {e}"))
    }

    /// 发送请求并等待应答（timeout 秒）
    async fn request(self: &Arc<Self>, method: &str, params: Value, timeout: std::time::Duration) -> Result<Value, String> {
        self.ensure_initialized().await?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if let Err(e) = self.send_msg(&msg).await {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => {
                if let Some(err) = resp.get("error") {
                    Err(format!(
                        "LSP {method} 失败: {}（{}）",
                        err["message"].as_str().unwrap_or("未知错误"),
                        err["code"].as_i64().unwrap_or(-1)
                    ))
                } else {
                    Ok(resp["result"].clone())
                }
            }
            Ok(Err(_)) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("LSP {method} 连接已断开"))
            }
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("LSP {method} 请求超时（>{timeout:?}）"))
            }
        }
    }

    async fn ensure_initialized(self: &Arc<Self>) -> Result<(), String> {
        let conn = self.clone();
        let init_cell = &conn.initialized;
        init_cell
            .get_or_try_init(|| {
                let conn = conn.clone();
                async move {
                let (sdk, hms) = discover_sdk().ok_or_else(|| {
                    "未找到鸿蒙 SDK（DEVECO_SDK_HOME 或 DevEco Studio 常见安装路径），LSP 无法初始化。\n\
                     提示：可在设置中配置 DEVECO_SDK_HOME 指向 SDK 根目录（含 default/openharmony），\n\
                     或改用 MCP 快集成：设置 → MCP → 添加 DevEco Toolbox 模板。".to_string()
                })?;
                let mut opts = json!({"ets": {"sdkPath": sdk}});
                if !hms.is_empty() {
                    opts["ets"]["hmsPath"] = json!(hms);
                }
                let init = conn
                    .send_raw_initialize(json!({
                        "processId": null,
                        "rootUri": Value::Null,
                        "capabilities": {},
                        "initializationOptions": opts,
                    }))
                    .await?;
                if init.get("error").is_some() {
                    return Err(format!("LSP initialize 被拒绝: {}", init["error"]["message"].as_str().unwrap_or("未知错误")));
                }
                let _ = conn
                    .send_msg(&json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}))
                    .await;
                Ok(())
                }
            })
            .await
            .map(|_| ())
    }

    /// 裸 initialize（不经 request，避免递归 ensure_initialized）
    async fn send_raw_initialize(self: &Arc<Self>, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": params});
        self.send_msg(&msg).await?;
        tokio::time::timeout(std::time::Duration::from_secs(45), rx)
            .await
            .map_err(|_| "LSP initialize 超时（45s，SDK 索引较慢或 SDK 路径无效）".to_string())?
            .map_err(|_| "LSP 连接已断开".to_string())
    }

    /// didOpen 指定文件（读磁盘内容），返回版本号
    async fn open_document(self: &Arc<Self>, path: &Path) -> Result<i64, String> {
        let text = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        let uri = to_file_uri(path)?;
        let version = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send_msg(&json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": {"textDocument": {"uri": uri, "languageId": "typescript", "version": version, "text": text}}
        }))
        .await?;
        Ok(version)
    }
}

impl Drop for LspConnection {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

/// 读一帧头，返回 Content-Length（EOF/异常返回 None）
async fn read_frame_head(r: &mut BufReader<ChildStdout>) -> Option<usize> {
    let mut line = Vec::new();
    let mut total = 0usize;
    loop {
        line.clear();
        let n = r.read_until(b'\n', &mut line).await.ok()?;
        if n == 0 {
            return None;
        }
        if line == b"\r\n" || line == b"\n" {
            return Some(total);
        }
        let s = String::from_utf8_lossy(&line);
        if s.to_ascii_lowercase().starts_with("content-length:") {
            total = s.split(':').nth(1)?.trim().parse().ok()?;
        }
    }
}

// ---------------- 通用辅助 ----------------

fn to_file_uri(path: &Path) -> Result<String, String> {
    url::Url::from_file_path(path)
        .map(|u| u.to_string())
        .map_err(|_| format!("路径转 file:// URI 失败: {}", path.display()))
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    url::Url::parse(uri).ok().and_then(|u| u.to_file_path().ok())
}

/// 取某文件某行（1-based）的文本，供结果附上下文
fn read_line_at(path: &Path, line1: usize) -> String {
    let Ok(text) = std::fs::read_to_string(path) else { return String::new() };
    text.lines().nth(line1.saturating_sub(1)).unwrap_or("").trim().chars().take(120).collect()
}

/// 工具参数：文件路径（项目内解析）+ 1-based 行列 → 0-based LSP 位置
fn resolve_lsp_pos(args: &Value, roots: &[String]) -> Result<(PathBuf, usize, usize), String> {
    let raw = args["path"].as_str().ok_or("需要参数 {\"path\":\"<文件路径>\",\"line\":<行号 1 起>,\"column\":<列号 1 起>}")?;
    let path = crate::agent::tools::resolve_in_roots(roots, raw)?;
    let line = args["line"].as_u64().ok_or("需要参数 line（行号，从 1 开始）")?.saturating_sub(1) as usize;
    let column = args["column"].as_u64().ok_or("需要参数 column（列号，从 1 开始）")?.saturating_sub(1) as usize;
    Ok((path, line, column))
}

/// 渲染 Location 列表（引用/定义结果）
fn render_locations(items: &[Value]) -> String {
    if items.is_empty() {
        return "（无结果）".into();
    }
    let mut out = String::new();
    for it in items.iter().take(30) {
        // LocationLink 用 targetUri/targetRange；Location 用 uri/range
        let uri = it["targetUri"].as_str().or_else(|| it["uri"].as_str()).unwrap_or("");
        let range = it["targetRange"].as_object().or_else(|| it["range"].as_object());
        let Some(path) = uri_to_path(uri) else { continue };
        if let Some(r) = range {
            let (sl, sc) = (r["start"]["line"].as_u64().unwrap_or(0) as usize + 1, r["start"]["character"].as_u64().unwrap_or(0) as usize + 1);
            let ctx = read_line_at(&path, sl);
            out.push_str(&format!("  {}:{}:{}{}\n", path.display(), sl, sc, if ctx.is_empty() { String::new() } else { format!("  {}", ctx) }));
        }
    }
    out
}

// ---------------- 工具实现（tools/mod.rs 分发） ----------------

/// 取（或懒启动）会话连接；缺 Node/LSP 包时给出安装引导
async fn conn_for(conversation_id: &str) -> Result<Arc<LspConnection>, String> {
    if let Some(c) = pool().lock().unwrap().get(conversation_id) {
        return Ok(c.clone());
    }
    let entry = lsp_entry().ok_or_else(|| {
        "未找到 @arkts/language-server（LSP 深度集成需要它）。\n安装：npm i -g @arkts/language-server@1.3.10\n\
         或改用 MCP 快集成：设置 → MCP → 添加 DevEco Toolbox 模板（npx deveco-mcp-server，自带官方 LSP 的 check_ets_files）。".to_string()
    })?;
    let mut cmd = Command::new(node_cmd());
    cmd.arg(&entry).arg("--stdio").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("启动 ets-language-server 失败: {e}"))?;
    let stdout = child.stdout.take().ok_or("无法获取 LSP stdout")?;
    let stdin = child.stdin.take().ok_or("无法获取 LSP stdin")?;
    let conn = LspConnection::new(child, stdout, stdin);
    pool().lock().unwrap().insert(conversation_id.to_string(), conn.clone());
    Ok(conn)
}

/// lsp_definition：跳转定义（含 SDK 内置组件 .d.ts 声明）
pub(super) async fn lsp_definition(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    let (path, line, column) = resolve_lsp_pos(args, roots)?;
    let conn = conn_for(conversation_id).await?;
    let res = conn
        .request("textDocument/definition", json!({
            "textDocument": {"uri": to_file_uri(&path)?},
            "position": {"line": line, "character": column}
        }), std::time::Duration::from_secs(20))
        .await?;
    let items = res.as_array().cloned().unwrap_or_else(|| vec![res]);
    Ok(format!("符号定义（{} 处）：\n{}", items.len(), render_locations(&items)))
}

/// lsp_references：查找引用（含声明本身与否由 include_declaration 控制）
pub(super) async fn lsp_references(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    let (path, line, column) = resolve_lsp_pos(args, roots)?;
    let include_decl = args["include_declaration"].as_bool().unwrap_or(true);
    let conn = conn_for(conversation_id).await?;
    let res = conn
        .request("textDocument/references", json!({
            "textDocument": {"uri": to_file_uri(&path)?},
            "position": {"line": line, "character": column},
            "context": {"includeDeclaration": include_decl}
        }), std::time::Duration::from_secs(20))
        .await?;
    let items = res.as_array().cloned().unwrap_or_default();
    Ok(format!("引用位置（{} 处）：\n{}", items.len(), render_locations(&items)))
}

/// lsp_symbols：文档符号树（struct/方法/成员，带行号）
pub(super) async fn lsp_symbols(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    let raw = args["path"].as_str().ok_or("需要参数 {\"path\":\"<文件路径>\"}")?;
    let path = crate::agent::tools::resolve_in_roots(roots, raw)?;
    let conn = conn_for(conversation_id).await?;
    let res = conn
        .request("textDocument/documentSymbol", json!({
            "textDocument": {"uri": to_file_uri(&path)?}
        }), std::time::Duration::from_secs(20))
        .await?;
    let mut out = String::new();
    fn walk(sym: &Value, depth: usize, out: &mut String) {
        let name = sym["name"].as_str().unwrap_or("?");
        let kind = match sym["kind"].as_u64().unwrap_or(0) {
            2 => "文件", 3 => "模块", 4 => "命名空间", 5 => "包", 6 => "类",
            7 => "方法", 8 => "属性", 9 => "字段", 11 => "接口", 12 => "函数",
            13 => "变量", 15 => "结构体", 23 => "构造器", 24 => "枚举", 26 => "常量",
            14 => "参数", _ => "?",
        };
        let line = sym["range"]["start"]["line"].as_u64().unwrap_or(0) + 1;
        out.push_str(&format!("{}{} {} : 第 {} 行\n", "  ".repeat(depth), kind, name, line));
        if let Some(children) = sym["children"].as_array() {
            for c in children {
                walk(c, depth + 1, out);
            }
        }
    }
    if let Some(arr) = res.as_array() {
        for s in arr {
            walk(s, 0, &mut out);
        }
    }
    if out.is_empty() {
        return Ok("（无符号）".into());
    }
    Ok(out)
}

/// lsp_hover：符号文档（API 说明）
pub(super) async fn lsp_hover(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    let (path, line, column) = resolve_lsp_pos(args, roots)?;
    let conn = conn_for(conversation_id).await?;
    let res = conn
        .request("textDocument/hover", json!({
            "textDocument": {"uri": to_file_uri(&path)?},
            "position": {"line": line, "character": column}
        }), std::time::Duration::from_secs(20))
        .await?;
    let contents = &res["contents"];
    let text = match contents {
        Value::String(s) => s.clone(),
        Value::Object(o) => o["value"].as_str().map(String::from).unwrap_or_default(),
        Value::Array(a) => a.iter().filter_map(|v| v["value"].as_str().or_else(|| v.as_str())).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    };
    if text.trim().is_empty() {
        return Ok("（该位置无悬停信息）".into());
    }
    Ok(text.trim().to_string())
}

/// lsp_diagnostics：真实类型检查（didOpen 后等待诊断推送）
pub(super) async fn lsp_diagnostics(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    let raw = args["path"].as_str().ok_or("需要参数 {\"path\":\"<文件路径>\"}")?;
    let path = crate::agent::tools::resolve_in_roots(roots, raw)?;
    let conn = conn_for(conversation_id).await?;
    let uri = to_file_uri(&path)?;
    let version = conn.open_document(&path).await?;
    // 轮询诊断缓存直到拿到该版本（或超时 30s）
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let diags = loop {
        if let Some((v, d)) = conn.diag_cache.lock().unwrap().get(&uri).cloned() {
            if v >= version {
                break d;
            }
        }
        if std::time::Instant::now() > deadline {
            break vec![];
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    };
    if diags.is_empty() {
        return Ok("无诊断错误（文件通过类型检查）".into());
    }
    let sev = |s: u64| match s { 1 => "错误", 2 => "警告", 3 => "信息", 4 => "提示", _ => "?" };
    let mut out = format!("诊断结果（{} 条）：\n", diags.len());
    for d in diags.iter().take(40) {
        let (sl, sc) = (
            d["range"]["start"]["line"].as_u64().unwrap_or(0) + 1,
            d["range"]["start"]["character"].as_u64().unwrap_or(0) + 1,
        );
        let ctx = read_line_at(&path, sl as usize);
        out.push_str(&format!("  [{}] {}:{}:{}  {}{}\n", sev(d["severity"].as_u64().unwrap_or(4)), sl, sc, d["message"].as_str().unwrap_or(""), ctx, if ctx.is_empty() { String::new() } else { format!("  ← {}", ctx) }));
    }
    if diags.len() > 40 {
        out.push_str(&format!("  …共 {} 条（截断显示）\n", diags.len()));
    }
    Ok(out)
}

// ---------------- 写类 / 交互类 LSP 工具（rename/format/code_action/completion/signature） ----------------

/// LSP 坐标的 character 是 UTF-16 code unit（中文/emoji 占 2），转字符下标
fn utf16_to_chars(line: &str, utf16_col: usize) -> usize {
    let mut n = 0usize;
    for (i, c) in line.chars().enumerate() {
        if n >= utf16_col {
            return i;
        }
        n += c.len_utf16();
    }
    line.chars().count()
}

/// 在内存中按位置倒序应用 TextEdit 列表（与 apply_text_edits 同一套定位规则），
/// 返回 (应用后的完整文本, 新增字符数, 删除字符数)。不落盘。
fn apply_edits_to_text(text: &str, edits: &[Value]) -> (String, usize, usize) {
    let mut lines: Vec<String> = text.split('\n').map(String::from).collect();
    let mut add = 0usize;
    let mut del = 0usize;
    // 按位置倒序应用（同一行内列号降序）：前面的编辑不会影响后续编辑的行/列定位
    let mut edits: Vec<&Value> = edits.iter().collect();
    edits.sort_by(|a, b| {
        let key = |e: &&Value| {
            (
                e["range"]["start"]["line"].as_u64().unwrap_or(0),
                e["range"]["start"]["character"].as_u64().unwrap_or(0),
            )
        };
        key(b).cmp(&key(a))
    });
    for e in edits {
        let range = &e["range"];
        let (sl, sc) = (
            range["start"]["line"].as_u64().unwrap_or(0) as usize,
            range["start"]["character"].as_u64().unwrap_or(0) as usize,
        );
        let (el, ec) = (
            range["end"]["line"].as_u64().unwrap_or(0) as usize,
            range["end"]["character"].as_u64().unwrap_or(0) as usize,
        );
        let new_text = e["newText"].as_str().unwrap_or("");
        if sl >= lines.len() || el >= lines.len() {
            continue;
        }
        let start_line = &lines[sl];
        let end_line = &lines[el];
        let s_char = utf16_to_chars(start_line, sc);
        let e_char = utf16_to_chars(end_line, ec);
        // 被替换的旧文本长度（按字符估算，仅用于结果统计）
        if el == sl {
            let old_len = end_line.chars().skip(s_char).take(e_char.saturating_sub(s_char)).count();
            del += old_len;
            add += new_text.chars().count();
            let prefix: String = start_line.chars().take(s_char).collect();
            let suffix: String = end_line.chars().skip(e_char).collect();
            lines[sl] = format!("{prefix}{new_text}{suffix}");
        } else {
            let head = start_line.chars().count().saturating_sub(s_char);
            let mid: usize = lines[sl + 1..el].iter().map(|l| l.chars().count() + 1).sum();
            del += head + mid + e_char;
            add += new_text.chars().count();
            let prefix: String = start_line.chars().take(s_char).collect();
            let suffix: String = end_line.chars().skip(e_char).collect();
            lines.drain(sl + 1..=el);
            lines[sl] = format!("{prefix}{new_text}{suffix}");
        }
    }
    (lines.join("\n"), add, del)
}

/// 应用 TextEdit 列表到文件（按位置倒序应用，行号不会因前面的编辑漂移）。
/// 写盘前记录 undo 快照（可 undo_edit 回退）。返回 (新增字符数, 删除字符数)。
fn apply_text_edits(path: &Path, edits: &[Value], conversation_id: &str) -> Result<(usize, usize), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    // 落盘前记录快照：rename/format/code_action 的写盘可被 undo_edit 回退
    crate::agent::undo::snapshot(conversation_id, path, &bytes);
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let (out, add, del) = apply_edits_to_text(&text, edits);
    std::fs::write(path, out).map_err(|e| format!("写回 {} 失败: {e}", path.display()))?;
    Ok((add, del))
}

/// 应用 LSP WorkspaceEdit（changes / documentChanges 两种形态）到磁盘，返回 (文件数, 新增字符, 删除字符)
fn apply_workspace_edit(res: &Value, conversation_id: &str) -> Result<(usize, usize, usize), String> {
    let mut files = 0usize;
    let mut total_add = 0usize;
    let mut total_del = 0usize;
    if let Some(changes) = res["changes"].as_object() {
        for (uri, edits) in changes {
            let Some(p) = uri_to_path(uri) else { continue };
            if let Some(arr) = edits.as_array() {
                let (a, d) = apply_text_edits(&p, arr, conversation_id)?;
                total_add += a;
                total_del += d;
                files += 1;
            }
        }
    }
    if let Some(docs) = res["documentChanges"].as_array() {
        for dc in docs {
            // TextDocumentEdit: {textDocument:{uri}, edits:[...]}
            if let (Some(uri), Some(edits)) = (
                dc["textDocument"]["uri"].as_str(),
                dc["edits"].as_array(),
            ) {
                if let Some(p) = uri_to_path(uri) {
                    let (a, d) = apply_text_edits(&p, edits, conversation_id)?;
                    total_add += a;
                    total_del += d;
                    files += 1;
                }
            }
        }
    }
    Ok((files, total_add, total_del))
}

/// lsp_rename：重命名符号 + 同步所有引用（跨文件 WorkspaceEdit 自动应用）。
pub(super) async fn lsp_rename(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    let (path, line, column) = resolve_lsp_pos(args, roots)?;
    let new_name = args["new_name"].as_str().ok_or("需要参数 new_name（新符号名）")?;
    let conn = conn_for(conversation_id).await?;
    let _ = conn.open_document(&path).await?;
    let res = conn
        .request("textDocument/rename", json!({
            "textDocument": {"uri": to_file_uri(&path)?},
            "position": {"line": line, "character": column},
            "newName": new_name
        }), std::time::Duration::from_secs(30))
        .await?;
    if res.is_null() {
        return Ok("该位置没有可重命名的符号（检查光标是否在符号上）".into());
    }
    let (files, add, del) = apply_workspace_edit(&res, conversation_id)?;
    Ok(format!(
        "重命名完成：\"{new_name}\"（涉及 {files} 个文件，+{add} −{del} 字符）\n\
         建议随后用 check_code 或构建验证无残留引用。"
    ))
}

/// lsp_format：按 ArkTS 风格格式化整个文件（直接落盘，format 前建议先 git 或 undo 保护）。
pub(super) async fn lsp_format(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    let raw = args["path"].as_str().ok_or("需要参数 {\"path\":\"<文件路径>\"}")?;
    let path = crate::agent::tools::resolve_in_roots(roots, raw)?;
    let tab_size = args["tab_size"].as_u64().unwrap_or(4) as u8;
    let insert_spaces = args["insert_spaces"].as_bool().unwrap_or(false);
    let conn = conn_for(conversation_id).await?;
    let _ = conn.open_document(&path).await?;
    let res = conn
        .request("textDocument/formatting", json!({
            "textDocument": {"uri": to_file_uri(&path)?},
            "options": {"tabSize": tab_size, "insertSpaces": insert_spaces}
        }), std::time::Duration::from_secs(30))
        .await?;
    let edits = res.as_array().cloned().unwrap_or_default();
    if edits.is_empty() {
        return Ok("文件已符合格式（无格式化变更）".into());
    }
    let (add, del) = apply_text_edits(&path, &edits, conversation_id)?;
    Ok(format!(
        "格式化完成（{} 处编辑，+{add} −{del} 字符）：{}\n\
         如需撤销可用 undo_edit。",
        edits.len(),
        path.display()
    ))
}

/// [05] format_file：独立单文件格式化工具（ArkTS/TS/JS/JSON/CSS 等 LSP 支持的格式）。
/// 与 lsp_format 的区别：支持 dry_run=true 只返回 unified diff 不落盘，
/// 先预览再落盘，避免格式化误伤；落盘路径与 lsp_format 同内核（undo 可回退）。
pub(super) async fn format_file(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    let raw = args["path"].as_str().ok_or("需要参数 {\"path\":\"<文件路径>\",\"dry_run\":<可选 true>}")?;
    let path = crate::agent::tools::resolve_in_roots(roots, raw)?;
    let dry_run = args["dry_run"].as_bool().unwrap_or(false);
    let tab_size = args["tab_size"].as_u64().unwrap_or(4) as u8;
    let insert_spaces = args["insert_spaces"].as_bool().unwrap_or(false);
    if !path.exists() {
        return Err(format!("文件不存在：{}", path.display()));
    }
    let conn = conn_for(conversation_id).await?;
    let _ = conn.open_document(&path).await?;
    let res = conn
        .request("textDocument/formatting", json!({
            "textDocument": {"uri": to_file_uri(&path)?},
            "options": {"tabSize": tab_size, "insertSpaces": insert_spaces}
        }), std::time::Duration::from_secs(30))
        .await?;
    let edits = res.as_array().cloned().unwrap_or_default();
    if edits.is_empty() {
        return Ok("文件已符合格式（无格式化变更）".into());
    }
    if dry_run {
        // 内存应用 + 行级 unified diff 预览（不落盘、不写 undo）
        let bytes = std::fs::read(&path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let (out, _, _) = apply_edits_to_text(&text, &edits);
        let old_lines: Vec<&str> = text.split('\n').collect();
        let new_lines: Vec<&str> = out.split('\n').collect();
        let (diff, add_lines, del_lines) = crate::agent::tools::fs_tools::build_unified_diff(
            &old_lines,
            &new_lines,
            &path.display().to_string(),
            0,
            0,
        );
        return Ok(format!(
            "【format 预览】（dry_run 未落盘，文件：{}，预计 +{add_lines} −{del_lines} 行）\n{diff}\n确认无误后去掉 dry_run 重新调用即落盘（可 undo_edit 回退）。",
            path.display()
        ));
    }
    let (add, del) = apply_text_edits(&path, &edits, conversation_id)?;
    Ok(format!(
        "格式化完成（{} 处编辑，+{add} −{del} 字符）：{}\n\
         如需撤销可用 undo_edit。",
        edits.len(),
        path.display()
    ))
}

/// lsp_code_action：列出该位置的 quick fix（无 index 时），或执行第 index 个 action（有 index 时）。
pub(super) async fn lsp_code_action(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    let (path, line, column) = resolve_lsp_pos(args, roots)?;
    let index = args["index"].as_u64();
    let conn = conn_for(conversation_id).await?;
    let _ = conn.open_document(&path).await?;
    // 先拿该位置的诊断（多数 quick fix 依赖诊断上下文）
    let uri = to_file_uri(&path)?;
    let diags = conn.diag_cache.lock().unwrap().get(&uri).cloned().map(|(_, d)| d).unwrap_or_default();
    let res = conn
        .request("textDocument/codeAction", json!({
            "textDocument": {"uri": uri},
            "range": {"start": {"line": line, "character": column}, "end": {"line": line + 1, "character": 0}},
            "context": {"diagnostics": diags.iter().take(10).collect::<Vec<_>>()}
        }), std::time::Duration::from_secs(30))
        .await?;
    let actions = res.as_array().cloned().unwrap_or_default();
    // 兼容 Command 形态
    let titles: Vec<(String, Value)> = actions
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let title = a["title"].as_str().unwrap_or("?");
            let kind = a["kind"].as_str().unwrap_or("");
            (format!("{i}. {title}{}", if kind.is_empty() { String::new() } else { format!("（{kind}）") }), a.clone())
        })
        .collect();
    if titles.is_empty() {
        return Ok("该位置没有可用的代码操作（quick fix）".into());
    }
    if let Some(idx) = index {
        let (title, action) = titles.get(idx as usize).ok_or(format!("index {idx} 超出范围（共 {} 个）", titles.len()))?;
        if let Some(edit) = action.get("edit") {
            let (files, add, del) = apply_workspace_edit(edit, conversation_id)?;
            Ok(format!(
                "已执行代码操作：{title}\n（涉及 {files} 个文件，+{add} −{del} 字符）\n\
                 建议随后运行 lsp_diagnostics 或构建验证修复效果。"
            ))
        } else if let Some(_cmd) = action.get("command") {
            // Command 形态（服务器侧执行）：@arkts 的 fix 多为 edit 形态，这里给出提示
            Ok(format!("代码操作 {title} 为服务器命令形态，暂不支持自动执行；请手动处理该问题。"))
        } else {
            Ok(format!("代码操作 {title} 没有可应用的编辑，无需执行。"))
        }
    } else {
        let mut out = format!("该位置可用代码操作（{} 个，带 index 参数可执行）：\n", titles.len());
        for t in titles {
            out.push_str(&format!("- {}\n", t.0));
        }
        out.push_str("\n执行：lsp_code_action 传 index=<编号>");
        Ok(out)
    }
}

/// lsp_completion：获取光标位置的自动补全候选（只读）。
pub(super) async fn lsp_completion(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    let (path, line, column) = resolve_lsp_pos(args, roots)?;
    let conn = conn_for(conversation_id).await?;
    let _ = conn.open_document(&path).await?;
    let res = conn
        .request("textDocument/completion", json!({
            "textDocument": {"uri": to_file_uri(&path)?},
            "position": {"line": line, "character": column}
        }), std::time::Duration::from_secs(20))
        .await?;
    let items = res.get("items").and_then(|v| v.as_array()).cloned()
        .unwrap_or_else(|| res.as_array().cloned().unwrap_or_default());
    if items.is_empty() {
        return Ok("（该位置无补全候选）".into());
    }
    let kind_zh = |k: u64| match k {
        1 => "方法", 2 => "函数", 3 => "构造器", 4 => "字段", 5 => "变量", 6 => "类",
        7 => "接口", 8 => "模块", 9 => "属性", 10 => "常量", 12 => "关键字", 15 => "枚举", _ => "?",
    };
    let mut out = format!("补全候选（{} 条，前 30）：\n", items.len());
    for (i, it) in items.iter().take(30).enumerate() {
        let label = it["label"].as_str().unwrap_or("?");
        let kind = it["kind"].as_u64().map(kind_zh).unwrap_or("?");
        let detail = it["detail"].as_str().unwrap_or("");
        let insert = it["insertText"].as_str().map(|s| s.chars().take(40).collect::<String>()).unwrap_or_default();
        out.push_str(&format!("{}. {} [{}]{}{}\n", i + 1, label, kind,
            if detail.is_empty() { String::new() } else { format!(" {}", detail) },
            if insert.is_empty() { String::new() } else { format!(" → {}", insert) }));
    }
    Ok(out)
}

/// lsp_signature：函数签名提示（参数名/类型/当前参数高亮）。
pub(super) async fn lsp_signature(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    let (path, line, column) = resolve_lsp_pos(args, roots)?;
    let conn = conn_for(conversation_id).await?;
    let _ = conn.open_document(&path).await?;
    let res = conn
        .request("textDocument/signatureHelp", json!({
            "textDocument": {"uri": to_file_uri(&path)?},
            "position": {"line": line, "character": column}
        }), std::time::Duration::from_secs(20))
        .await?;
    let sigs = res["signatures"].as_array().cloned().unwrap_or_default();
    if sigs.is_empty() {
        return Ok("（该位置无签名信息）".into());
    }
    let active = res["activeSignature"].as_u64().unwrap_or(0) as usize;
    let param = res["activeParameter"].as_u64().unwrap_or(0) as usize;
    let mut out = String::new();
    for (i, s) in sigs.iter().enumerate().take(5) {
        let label = s["label"].as_str().unwrap_or("?");
        let docs = s["documentation"]["value"].as_str().or_else(|| s["documentation"].as_str()).unwrap_or("");
        out.push_str(&format!("{}{}\n{}",
            if i == active { "▶ " } else { "  " },
            label,
            if docs.is_empty() { String::new() } else { format!("   {}\n", docs) }));
    }
    out.push_str(&format!("（当前参数下标：{param}）"));
    Ok(out)
}
