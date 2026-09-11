//! runtime 子模块 — 按职责拆分（详见 quality_tools.rs facade）。
//!
//! 调用方式不变：quality_tools::xxx(...)，通过 pub use re-export 暴露。

use crate::agent::tools::{resolve_for_write, resolve_in_roots, resolve_readable};
use serde_json::Value;
use std::time::Duration;

/// 单个 mock 路由（method + 路径正则 + 样例响应）
struct MockRoute {
    method: String,
    path_regex: String,
    response: serde_json::Value,
}

async fn execute_debug_capability(
    capability: &crate::agent::capability_broker::HostCapability,
    label: &str,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let output = crate::agent::capability_broker::execute_host_capability(capability, None, ctx)
        .await
        .map_err(|error| format!("{label}失败: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let combined = format!("{stdout}\n{stderr}");
    if !output.status.success() || debugger_command_failed(&combined) {
        let detail = combined.trim();
        return Err(format!(
            "{label}失败{}",
            if detail.is_empty() { String::new() } else { format!("：{detail}") }
        ));
    }
    Ok(stdout)
}

fn debugger_command_failed(output: &str) -> bool {
    let normalized = output.to_ascii_lowercase();
    ["[fail]", "error:", "not found", "no such", "unknown command", "permission denied"]
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn parse_pid(raw: &str, label: &str) -> Result<u32, String> {
    let value = raw.split_whitespace().next().unwrap_or("");
    let pid = value
        .parse::<u32>()
        .map_err(|_| format!("{label}必须是大于 0 的十进制 PID"))?;
    if pid == 0 {
        return Err(format!("{label}必须大于 0"));
    }
    Ok(pid)
}
pub async fn api_test(args: &Value, roots: &[String]) -> Result<String, String> {
    let spec_raw = args["spec"].as_str().ok_or("api_test 需要参数 {\"spec\":\"<OpenAPI JSON 路径或内联>\"}")?;
    let spec: Value = if spec_raw.trim_start().starts_with('{') {
        serde_json::from_str(spec_raw).map_err(|e| format!("spec JSON 解析失败: {e}"))?
    } else {
        let p = resolve_readable(roots, spec_raw)?;
        let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| format!("spec 文件 JSON 解析失败（{}）: {e}", p.display()))?
    };
    let base = args["base_url"]
        .as_str()
        .map(String::from)
        .or_else(|| {
            spec["servers"]
                .as_array()
                .and_then(|s| s.first())
                .and_then(|s| s["url"].as_str())
                .map(String::from)
        })
        .ok_or("无法确定 base_url：请传 base_url 参数或 spec 含 servers[0].url")?;
    let base = base.trim_end_matches('/');
    let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(15).clamp(1, 60);
    let client = crate::utils::net::build_client_auto().map_err(|e| format!("网络初始化失败: {e}"))?;

    // 用例来源：显式 cases 或从 spec 提取 GET 路径
    let cases: Vec<(String, String, String, Option<i64>, Option<Value>)> = if let Some(arr) = args["cases"].as_array() {
        let mut v = Vec::new();
        for c in arr {
            let path = c["path"].as_str().unwrap_or("").to_string();
            let method = c["method"].as_str().unwrap_or("GET").to_uppercase();
            let status = c["status"].as_i64();
            if path.is_empty() {
                return Err("cases[].path 不能为空".into());
            }
            let headers = c.get("headers").cloned();
            let body = c["body"].as_str().unwrap_or("").to_string();
            v.push((path, method, body, status, headers));
        }
        v
    } else {
        let mut v = Vec::new();
        if let Some(paths) = spec["paths"].as_object() {
            for (path, item) in paths {
                if let Some(get) = item.get("get") {
                    v.push((path.clone(), "GET".into(), String::new(), None, None));
                    let _ = get;
                }
            }
        }
        if v.is_empty() {
            return Err("spec 中无 GET 路径且未传 cases 参数".into());
        }
        v
    };
    if cases.len() > 40 {
        return Err(format!("用例过多（{}），单次最多 40 个（可拆分或减少 cases）", cases.len()));
    }
    let mut report = String::new();
    let mut pass = 0;
    let mut fail = 0;
    report.push_str(&format!("API 测试报告（{} 个用例 → {base}）\n", cases.len()));
    for (idx, (path, method, body, expect, headers)) in cases.iter().enumerate() {
        let url = if path.starts_with("http") { path.clone() } else { format!("{base}{path}") };
        let mut rb = match method.as_str() {
            "GET" => client.get(&url),
            "POST" => client.post(&url),
            "PUT" => client.put(&url),
            "DELETE" => client.delete(&url),
            "PATCH" => client.patch(&url),
            other => {
                fail += 1;
                report.push_str(&format!("{}. ❌ {method} {path}：不支持的方法 {other}\n", idx + 1));
                continue;
            }
        };
        if let Some(hs) = headers {
            if let Some(obj) = hs.as_object() {
                for (k, v) in obj {
                    if let Some(sv) = v.as_str() {
                        rb = rb.header(k, sv);
                    }
                }
            }
        }
        if !body.is_empty() {
            rb = rb.header("Content-Type", "application/json").body(body.clone());
        }
        let t0 = std::time::Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(timeout_secs), rb.send()).await;
        let elapsed = t0.elapsed().as_millis();
        match result {
            Ok(Ok(resp)) => {
                let status = resp.status().as_u16();
                let ok = expect.map(|e| status == e as u16).unwrap_or(status < 400);
                if ok {
                    pass += 1;
                    report.push_str(&format!("{}. ✅ {method} {path} → {status}（{}ms）\n", idx + 1, elapsed));
                } else {
                    fail += 1;
                    report.push_str(&format!("{}. ❌ {method} {path} → {status}（期望 {expect:?}，{}ms）\n", idx + 1, elapsed));
                }
            }
            Ok(Err(e)) => {
                fail += 1;
                report.push_str(&format!("{}. ❌ {method} {path} → 请求失败：{e}\n", idx + 1));
            }
            Err(_) => {
                fail += 1;
                report.push_str(&format!("{}. ❌ {method} {path} → 超时（>{timeout_secs}s）\n", idx + 1));
            }
        }
    }
   report.push_str(&format!("\n结果：{pass} 通过 / {fail} 失败（共 {}）", cases.len()));
    Ok(report)
}

pub async fn api_mock(
    args: &Value,
    roots: &[String],
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法确定 mock 输出位置".into());
    }
    let spec_raw = args["path"].as_str().ok_or("api_mock 需要参数 {\"path\":\"<OpenAPI JSON 路径或内联>\"}")?;
    let spec: Value = if spec_raw.trim_start().starts_with('{') {
        serde_json::from_str(spec_raw).map_err(|e| format!("spec JSON 解析失败: {e}"))?
    } else {
        let p = resolve_readable(roots, spec_raw)?;
        let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| format!("spec 文件 JSON 解析失败（{}）: {e}", p.display()))?
    };
    let port = args["port"].as_u64().unwrap_or(18080).clamp(1024, 65535) as u16;
    let extra_headers = args["headers"].as_object().cloned().unwrap_or_default();

    // 1) 提取路由
    let mut routes: Vec<MockRoute> = Vec::new();
    let Some(paths) = spec["paths"].as_object() else {
        return Err("spec 缺少 paths 字段（仅支持 OpenAPI 3.x）".into());
    };
    for (path, item) in paths {
        for (method, op) in item.as_object().unwrap_or(&serde_json::Map::new()) {
            let m = method.to_uppercase();
            if !["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"].contains(&m.as_str()) {
                continue;
            }
            let (status, sample) = pick_response_sample(op, 0);
            routes.push(MockRoute {
                method: m.clone(),
                path_regex: path_template_to_regex(path),
                response: serde_json::json!({
                    "_mock": { "status": status, "path": path, "method": m },
                    "data": sample,
                }),
            });
        }
    }
    if routes.is_empty() {
        return Err("spec 中未找到任何可 mock 的路径".into());
    }

    // 2) 生成 Node 脚本（零依赖，http 模块）
    let routes_json = serde_json::to_string(&routes.iter().map(|r| {
        serde_json::json!({
            "method": r.method,
            "regex": r.path_regex,
            "response": r.response,
        })
    }).collect::<Vec<_>>()).map_err(|e| e.to_string())?;
    let headers_json = serde_json::to_string(&extra_headers).map_err(|e| e.to_string())?;
    let script = format!(
        "const http = require('http');\nconst port = parseInt(process.argv[2] || '{}', 10);\n\nconst routes = {};\nconst extraHeaders = {};\n\nconst server = http.createServer((req, res) => {{\n  const url = (req.url || '').split('?')[0];\n  for (const r of routes) {{\n    if (req.method === r.method && new RegExp(r.regex).test(url)) {{\n      const body = JSON.stringify(r.response);\n      res.writeHead(r.response._mock.status, Object.assign({{'Content-Type': 'application/json'}}, extraHeaders));\n      res.end(body);\n      return;\n    }}\n  }}\n  res.writeHead(404, {{'Content-Type': 'application/json'}});\n  res.end(JSON.stringify({{error: 'Not Found', path: url}}));\n}});\nserver.listen(port, '127.0.0.1', () => console.log('mock ready on port ' + port));\n",
        port, routes_json, headers_json
    );
    let base = roots[0].trim_end_matches(['/', '\\']);
    let dir = format!("{base}/.deveco-agent/mock");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 mock 目录失败：{e}"))?;
    let script_path = std::path::Path::new(&dir).join("server.js");
    std::fs::write(&script_path, script).map_err(|e| format!("写脚本失败：{e}"))?;

    // 3) 用内置 Node 后台启动（常驻 12h，可 job_kill 终止）
    let node = if let Some(app) = ctx.app.as_ref() {
        let app = app.clone();
        let info = tokio::task::spawn_blocking(move || {
            crate::services::node_runtime::get_node_runtime_info(&app)
        })
        .await
        .map_err(|e| format!("查询 Node 运行时失败: {e}"))?;
        info.dir
            .as_ref()
            .map(|d| {
                let p = std::path::Path::new(d).join("node.exe");
                if p.is_file() { p.to_string_lossy().to_string() } else { "node".to_string() }
            })
            .unwrap_or_else(|| "node".to_string())
    } else {
        "node".to_string()
    };
    let job_id = crate::agent::jobs::start_background(
        node,
        vec![script_path.to_string_lossy().to_string(), port.to_string()],
        format!("mock server on :{port}"),
        std::path::PathBuf::from(&dir),
        12 * 3600,
        ctx,
        None,
    )?;

    // 4) 返回使用说明
    let first = routes.first().unwrap();
    Ok(format!(
        "Mock 服务已启动（任务 {job_id}）：http://127.0.0.1:{port}\n共 {} 条路由，示例：{} {}\n返回结构：{{\"_mock\":{{\"status\",\"path\",\"method\"}},\"data\":<样例数据>}}\n服务日志与终止：job_output {job_id} / job_kill {job_id}\n调用示例：用 http_request 或 run_command curl 请求 http://127.0.0.1:{port}{}\n",
        routes.len(),
        first.method,
        spec["paths"].as_object().map(|p| {
            p.keys().next().cloned().unwrap_or_else(|| "/".to_string())
        }).unwrap_or_else(|| "/".to_string()),
        spec["paths"].as_object().and_then(|p| p.keys().next()).cloned().unwrap_or_else(|| "/".to_string()),
    ))
}

pub async fn api_health(args: &Value) -> Result<String, String> {
    let urls: Vec<String> = if let Some(arr) = args["urls"].as_array() {
        arr.iter().filter_map(|v| v.as_str().map(String::from)).collect()
    } else if let Some(u) = args["url"].as_str() {
        vec![u.to_string()]
    } else {
        return Err("api_health 需要参数 {\"urls\":[\"http://...\"]} 或 {\"url\":\"...\"}".into());
    };
    if urls.is_empty() {
        return Err("urls 为空".into());
    }
    if urls.len() > 10 {
        return Err("单次最多探测 10 个 URL".into());
    }
    let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(8).clamp(1, 30);
    for u in &urls {
        if !u.starts_with("http://") && !u.starts_with("https://") {
            return Err(format!("仅支持 http/https 地址：{u}"));
        }
    }
    let client = crate::utils::net::build_client_auto().map_err(|e| format!("网络初始化失败: {e}"))?;
    let mut out = String::new();
    out.push_str(&format!("API 健康探测（{} 个端点，超时 {timeout_secs}s）\n", urls.len()));
    let mut healthy = 0usize;
    for u in &urls {
        let t0 = std::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            client.get(u).send(),
        )
        .await;
        let elapsed = t0.elapsed().as_millis();
        match result {
            Ok(Ok(resp)) => {
                let status = resp.status().as_u16();
                let ok = status < 500;
                if ok {
                    healthy += 1;
                }
                out.push_str(&format!("  {} {status}（{elapsed}ms）{u}\n", if ok { "✅" } else { "⚠️" }));
            }
            Ok(Err(e)) => out.push_str(&format!("  ❌ 请求失败（{elapsed}ms）{u}\n     {e}\n")),
            Err(_) => out.push_str(&format!("  ❌ 超时（>{timeout_secs}s）{u}\n")),
        }
    }
    out.push_str(&format!("\n健康 {}/{}", healthy, urls.len()));
    Ok(out)
}

pub async fn attach_debugger(
    args: &Value,
    roots: &[String],
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => crate::agent::tools::default_device_id(ctx).await?,
    };
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    let bundle = match args["bundle"].as_str() {
        Some(b) => b.to_string(),
        None => {
            if project_path.is_empty() {
                return Err("未指定 bundle 且当前会话未绑定工程".into());
            }
            crate::services::harmony::parse_project(std::path::Path::new(project_path))
                .bundle_name
                .ok_or_else(|| "无法确定应用包名".to_string())?
        }
    };
    if bundle.is_empty() { return Err("无法确定应用包名".into()); }
    let wait_secs = args["wait_secs"].as_u64().unwrap_or(30).clamp(1, 120);

    // 1) 拿 pid
    let pid_query = crate::agent::capability_broker::HostCapability::DevicePidof {
        device: device.clone(),
        bundle: bundle.clone(),
    };
    let pid_out = execute_debug_capability(&pid_query, "查询应用 PID", ctx).await?;
    if pid_out.trim().is_empty() {
        return Err("应用未运行或 pidof 返回空（先 deploy 启动应用）".to_string());
    }
    let pid = parse_pid(&pid_out, "pidof 输出")?;

    // 2) attach 调试器（hdc shell debuggerd attach <pid>，系统服务）
    //    注：DevEco 工程的 attach 通常用 `aa debug -b <bundle>` 启动开发模式；
    //    这里是运行时 attach，更轻量。
    let attach = crate::agent::capability_broker::HostCapability::AttachDeviceDebugger {
        device: device.clone(),
        pid,
        wait_seconds: wait_secs,
    };
    let attach_output = crate::agent::capability_broker::execute_host_capability(&attach, None, ctx)
        .await
        .map_err(|error| {
            format!(
                "debuggerd attach 未取得确定终态，为避免叠加设备副作用，未自动执行 aa debug 回退：{error}"
            )
        })?;
    let attach_stdout = String::from_utf8_lossy(&attach_output.stdout).into_owned();
    let attach_stderr = String::from_utf8_lossy(&attach_output.stderr).into_owned();
    let attach_detail = format!("{attach_stdout}\n{attach_stderr}");
    let attach_out = if attach_output.status.success() && !debugger_command_failed(&attach_detail) {
        Ok(attach_stdout)
    } else {
        Err(if attach_detail.trim().is_empty() {
            "debuggerd attach 返回失败状态".to_string()
        } else {
            format!("debuggerd attach 失败：{}", attach_detail.trim())
        })
    };

    match attach_out {
        Ok(out) => Ok(format!(
            "调试器已 attach：设备 {device} / 包 {bundle} / PID {pid} / 等待 {wait_secs}s\ndebuggerd 输出：{}\n\n下一步：\n  1. 在 DevEco Studio 中 Run > Attach Debugger，选已 attach 的进程\n  2. 或在终端用 jstack/jdb 远程连接到设备 debuggerd 端口\n  3. 配合 set_breakpoint / inspect_variable 等工具（如已实现）",
            if out.trim().is_empty() { "(无输出)" } else { out.trim() }
        )),
        Err(e) => {
            // 退路：尝试 aa debug 启动开发模式
            let fallback = crate::agent::capability_broker::HostCapability::EnableAbilityDebug {
                device: device.clone(),
                bundle: bundle.clone(),
            };
            let aa = execute_debug_capability(&fallback, "启用 Ability 调试模式", ctx).await;
            match aa {
                Ok(out2) => Ok(format!(
                    "调试器已通过 aa debug 启动：设备 {device} / 包 {bundle} / PID {pid}\n输出：{}\n",
                    out2
                )),
                Err(_) => Err(format!(
                    "attach 失败：{e}\n回退方案也失败（aa debug 不可用，可能需要 userdebug 系统）\n替代：在 DevEco Studio 中 Run > Debug 'app'"
                )),
            }
        }
    }
}

pub async fn step_debug(
    args: &Value,
    roots: &[String],
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => crate::agent::tools::default_device_id(ctx).await?,
    };
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    let pid = match args["pid"].as_str() {
        Some(p) => parse_pid(p, "pid")?,
        None => {
            if project_path.is_empty() {
                return Err("未指定 pid 且当前会话未绑定工程".into());
            }
            let bundle = crate::services::harmony::parse_project(std::path::Path::new(project_path))
                .bundle_name
                .ok_or_else(|| "无法确定应用包名".to_string())?;
            let query = crate::agent::capability_broker::HostCapability::DevicePidof {
                device: device.clone(),
                bundle,
            };
            let pid_out = execute_debug_capability(&query, "查询应用 PID", ctx).await?;
            if pid_out.trim().is_empty() {
                return Err("应用未运行（先 deploy 启动或 attach_debugger）".into());
            }
            parse_pid(&pid_out, "pidof 输出")?
        }
    };
    let action = args["action"].as_str().unwrap_or("step");
    let debugger_action = match action {
        "step" => crate::agent::capability_broker::DeviceDebuggerAction::Step,
        "next" => crate::agent::capability_broker::DeviceDebuggerAction::Next,
        "continue" | "cont" | "c" => crate::agent::capability_broker::DeviceDebuggerAction::Continue,
        "interrupt" | "int" => crate::agent::capability_broker::DeviceDebuggerAction::Interrupt,
        "where" | "bt" | "backtrace" => crate::agent::capability_broker::DeviceDebuggerAction::Backtrace,
        "info" | "registers" => crate::agent::capability_broker::DeviceDebuggerAction::Registers,
        other => return Err(format!("不支持的 step_debug action: {other}（step/next/continue/interrupt/where/info）")),
    };

    let control = crate::agent::capability_broker::HostCapability::ControlDeviceDebugger {
        device: device.clone(),
        pid,
        action: debugger_action,
    };
    let out = execute_debug_capability(&control, "debuggerd 命令", ctx).await?;

    Ok(format!(
        "单步调试（设备 {device} / PID {pid} / action={action}）：\n{}",
        if out.trim().is_empty() { "(无输出，可能进程未停在断点)" } else { out.trim() }
    ))
}

pub async fn ota_pack(
    args: &Value,
    roots: &[String],
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let hap_path = args["hap_path"]
        .as_str()
        .ok_or("ota_pack 需要参数 {\"hap_path\":\"<HAP 路径>\"}")?;
    let out_path = args["out_path"]
        .as_str()
        .ok_or("ota_pack 需要参数 {\"out_path\":\"<输出 .pkg 路径>\"}")?;
    let profile_path = args["profile_path"].as_str().map(str::trim).filter(|path| !path.is_empty());

    // 1) 把所有输入、输出绑定到同一个已授权工作区。
    let hap_full = resolve_in_roots(roots, hap_path)?;
    let out_full = resolve_for_write(roots, out_path)?;
    let profile_full = profile_path.map(|path| resolve_in_roots(roots, path)).transpose()?;
    let workspace = roots
        .iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .find(|root| {
            hap_full.starts_with(root)
                && out_full.starts_with(root)
                && profile_full.as_ref().map_or(true, |profile| profile.starts_with(root))
        })
        .ok_or("HAP、输出和 profile 必须位于同一个已授权项目根内")?;
    let relative = |path: &std::path::Path| -> Result<String, String> {
        path.strip_prefix(&workspace)
            .map(|value| value.to_string_lossy().into_owned())
            .map_err(|_| "OTA 路径越出项目工作区".into())
    };
    let capability = crate::agent::capability_broker::HostCapability::PackageOta {
        hap_path: relative(&hap_full)?,
        output_path: relative(&out_full)?,
        profile_path: profile_full.as_deref().map(relative).transpose()?,
    };
    capability.validate()?;
    let output_parent = out_full.parent().ok_or("OTA 输出路径缺少父目录")?;
    std::fs::create_dir_all(output_parent).map_err(|error| format!("创建 OTA 输出目录失败：{error}"))?;
    let canonical_parent = output_parent
        .canonicalize()
        .map_err(|error| format!("无法解析 OTA 输出目录：{error}"))?;
    if !canonical_parent.starts_with(&workspace) {
        return Err("OTA 输出目录通过符号链接逃逸项目工作区".into());
    }

    // 2) Broker 内部发现受信任 packagingtool，固定 java argv 并持久化不可重放 claim。
    let start = std::time::Instant::now();
    let output = crate::agent::capability_broker::execute_host_capability(
        &capability,
        Some(&workspace),
        ctx,
    )
    .await
    .map_err(|error| format!("OTA 打包未取得确定终态：{error}"))?;
    let elapsed = start.elapsed();

    if !output.status.success() {
        return Err(format!(
            "OTA 打包失败（退出码 {}）\nstderr: {}\nstdout: {}",
            output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "无".into()),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let packaged = out_full
        .canonicalize()
        .map_err(|error| format!("packagingtool 返回成功，但输出文件不存在：{error}"))?;
    if !packaged.starts_with(&workspace) || !packaged.is_file() {
        return Err("packagingtool 返回成功，但输出不是工作区内普通文件".into());
    }
    let size = std::fs::metadata(&packaged).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return Err("packagingtool 返回成功，但 OTA 输出文件为空".into());
    }
    Ok(format!(
        "✅ OTA 包已生成：{}\n大小：{:.1} KB\n耗时：{:.1}s\nstdout 摘要：\n{}",
        packaged.display(),
        size as f64 / 1024.0,
        elapsed.as_secs_f64(),
        if stdout.trim().is_empty() { "(无输出)".to_string() } else { stdout.chars().take(1500).collect::<String>() }
    ))
}


fn sample_from_schema(schema: &serde_json::Value, depth: usize) -> serde_json::Value {
    if depth > 6 {
        return serde_json::Value::Null;
    }
    if let Some(ex) = schema.get("example") {
        if !ex.is_null() {
            return ex.clone();
        }
    }
    if let Some(dv) = schema.get("default") {
        if !dv.is_null() {
            return dv.clone();
        }
    }
    if let Some(r) = schema["$ref"].as_str() {
        // 仅支持站内 components/schemas 引用（/components/schemas/Name）
        if let Some(name) = r.rsplit('/').next() {
            return serde_json::Value::Object(serde_json::Map::from_iter([
                ("$ref_target".to_string(), serde_json::Value::String(name.to_string())),
            ]));
        }
    }
    if let Some(enum_arr) = schema["enum"].as_array() {
        if let Some(first) = enum_arr.first() {
            return first.clone();
        }
    }
    match schema["type"].as_str().unwrap_or("") {
        "object" => {
            let mut obj = serde_json::Map::new();
            if let Some(props) = schema["properties"].as_object() {
                for (k, v) in props {
                    obj.insert(k.clone(), sample_from_schema(v, depth + 1));
                }
            } else if let Some(any) = schema.get("additionalProperties") {
                if any.is_object() && !any.is_null() {
                    obj.insert("key".to_string(), sample_from_schema(any, depth + 1));
                }
            }
            serde_json::Value::Object(obj)
        }
        "array" => {
            let items = &schema["items"];
            if items.is_object() && !items.is_null() {
                serde_json::json!([sample_from_schema(items, depth + 1)])
            } else {
                serde_json::json!([])
            }
        }
        "string" => {
            let f = schema["format"].as_str().unwrap_or("");
            let sample = match f {
                "date-time" => "2026-01-01T00:00:00Z",
                "date" => "2026-01-01",
                "email" => "user@example.com",
                "uuid" => "00000000-0000-0000-0000-000000000000",
                "uri" => "https://example.com/",
                "ipv4" => "127.0.0.1",
                _ => "string",
            };
            serde_json::Value::String(sample.to_string())
        }
        "integer" | "number" => serde_json::json!(0),
        "boolean" => serde_json::json!(true),
        _ => serde_json::Value::Null,
    }
}


fn pick_response_sample(op: &serde_json::Value, depth: usize) -> (u16, serde_json::Value) {
    let responses = op["responses"].as_object().cloned().unwrap_or_default();
    let mut candidates: Vec<(&String, &serde_json::Value)> = responses.iter().collect();
    candidates.sort_by_key(|(k, _)| {
        k.parse::<u16>().unwrap_or(999) // 数字状态码优先，default 排最后
    });
    for (code, resp) in candidates {
        if let Ok(n) = code.parse::<u16>() {
            if (200..300).contains(&n) {
                let body = &resp["content"]["application/json"];
                let sample = if !body["example"].is_null() {
                    body["example"].clone()
                } else if !body["schema"].is_null() {
                    sample_from_schema(&body["schema"], depth)
                } else {
                    serde_json::Value::Null
                };
                return (n, sample);
            }
        }
    }
    // 无 2xx：default 优先，其次第一个响应
    if let Some(d) = responses.get("default") {
        let sample = if !d["content"]["application/json"]["example"].is_null() {
            d["content"]["application/json"]["example"].clone()
        } else {
            serde_json::Value::Null
        };
        return (200, sample);
    }
    (200, serde_json::Value::Null)
}


fn path_template_to_regex(path: &str) -> String {
    let mut re = String::from("^");
    for seg in path.split('/') {
        if seg.starts_with('{') && seg.ends_with('}') {
            re.push_str("/[^/]+");
        } else if seg.is_empty() {
            continue;
        } else {
            re.push('/');
            for c in seg.chars() {
                if ".*+?^$|()[]\\".contains(c) {
                    re.push('\\');
                }
                re.push(c);
            }
        }
    }
    re.push('$');
    re
}
