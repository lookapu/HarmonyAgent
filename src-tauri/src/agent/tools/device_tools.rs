//! 设备管理域工具：无线连接 / hdc 服务 / 模拟器 / 文件传输 / 进程停止 / 受限 shell / 崩溃取证。
//! 共享辅助函数（run_cmd / run_hdc_shell / default_device_id / tail 等）在父模块 mod.rs，
//! 本模块通过 `use super::*` 继承访问。

use super::*;

pub(super) async fn connect_device(args: &Value, ctx: &crate::agent::exec_ctx::ToolCtx) -> Result<String, String> {
    let action = args["action"].as_str().unwrap_or("connect").trim();
    let host = args["host"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty()).unwrap_or("");
    let port = args["port"].as_u64().unwrap_or(5555);
    if !(1..=65535).contains(&port) {
        return Err("port 必须在 1-65535 之间".into());
    }
    let target = match args["sn"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(sn) => sn.to_string(),
        None => {
            if host.is_empty() {
                return Err("connect_device 需要 host（设备 IP）或 sn（完整 ip:port）".into());
            }
            format!("{host}:{port}")
        }
    };
    match action {
        "connect" => {
            let capability = crate::agent::capability_broker::HostCapability::HdcConnect { target: target.clone() };
            let output = crate::agent::capability_broker::execute_host_capability(&capability, None, ctx)
                .await.map_err(|e| format!("无线连接失败：{e}"))?;
            let out = smart_decode(&output.stdout) + &smart_decode(&output.stderr);
            if !output.status.success() {
                return Err(format!("无线连接失败：{}", out.trim()));
            }
            let out = out.trim();
            Ok(format!(
                "无线连接请求已发送：{target}\n设备输出：{}\n下一步：调用 list_devices 确认设备在线；部署/截图/日志时 device 参数填 {target}。",
                if out.is_empty() { "(无输出，通常表示连接成功)" } else { out }
            ))
        }
        "disconnect" => {
            let capability = crate::agent::capability_broker::HostCapability::HdcDisconnect { target: target.clone() };
            let output = crate::agent::capability_broker::execute_host_capability(&capability, None, ctx)
                .await.map_err(|e| format!("断开失败：{e}"))?;
            let out = smart_decode(&output.stdout) + &smart_decode(&output.stderr);
            if !output.status.success() {
                return Err(format!("断开失败：{}", out.trim()));
            }
            Ok(format!("已断开 {target}\n设备输出：{}", out.trim()))
        }
        "list" => list_devices(ctx).await,
        _ => Err(format!("action 仅支持 connect|disconnect|list，收到 {action}")),
    }
}

pub(super) async fn manage_hdc(
    args: &Value,
    db: &crate::db::DbState,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let action = args["action"].as_str().unwrap_or("status").trim();
    if !matches!(action, "start" | "stop" | "restart" | "status") {
        return Err("action 仅支持 start|stop|restart|status".into());
    }
    // hdc 路径：优先探测到的工具链，回退 PATH（detect 首次走 reg query 等同步 IO，放入 blocking 线程池）
    let db2 = crate::db::DbState(db.0.clone());
    let env = tokio::task::spawn_blocking(move || crate::services::harmony_env::detect(&db2))
        .await
        .map_err(|e| format!("环境探测失败: {e}"))?;
    let hdc = env.hdc_path.clone().unwrap_or_else(|| "hdc".to_string());
    // 服务状态探测：能执行 list targets 即视为在线
    let probe = async || {
        let capability = crate::agent::capability_broker::HostCapability::HdcListTargets;
        match crate::agent::capability_broker::execute_host_capability(&capability, None, ctx).await {
            Ok(output) if output.status.success() => {
                let t = smart_decode(&output.stdout) + &smart_decode(&output.stderr);
                let devs: Vec<&str> = t
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty() && !l.starts_with("Empty"))
                    .collect();
                let online = devs.iter().filter(|l| l.contains("Connected")).count();
                Some(format!(
                    "hdc 服务在线，检测到 {} 台设备（在线 {online} 台）",
                    devs.len()
                ))
            }
            _ => None,
        }
    };
    match action {
        "status" => match probe().await {
            Some(s) => Ok(format!("hdc 状态：✓ {s}\n（hdc 路径：{hdc}）")),
            None => Err(format!(
                "hdc 服务不可用（{hdc}）。\n建议：manage_hdc action=start 启动服务；若仍失败请用 environment_check 检查工具链路径，或确认 hdc 是否安装/在 PATH。"
            )),
        },
        "start" => {
            let capability = crate::agent::capability_broker::HostCapability::HdcStartServer;
            let output = crate::agent::capability_broker::execute_host_capability(&capability, None, ctx)
                .await
                .map_err(|e| format!("hdc start 失败：{e}"))?;
            let out = smart_decode(&output.stdout) + &smart_decode(&output.stderr);
            if !output.status.success() {
                return Err(format!("hdc start 失败：{}", out.trim()));
            }
            let mut s = format!("hdc start 执行完成。\n{}", out.trim_end());
            if let Some(ok) = probe().await {
                s.push_str(&format!("\n✓ {ok}"));
            } else {
                s.push_str("\n✗ 服务仍未响应，可稍后重试 manage_hdc action=status");
            }
            Ok(s)
        }
        "stop" => {
            let capability = crate::agent::capability_broker::HostCapability::HdcKillServer;
            let output = crate::agent::capability_broker::execute_host_capability(&capability, None, ctx)
                .await
                .map_err(|e| format!("hdc kill 失败：{e}"))?;
            let out = smart_decode(&output.stdout) + &smart_decode(&output.stderr);
            if !output.status.success() {
                return Err(format!("hdc kill 失败：{}", out.trim()));
            }
            let mut s = format!("hdc 服务已停止。\n{}", out.trim_end());
            if probe().await.is_some() {
                s.push_str("\n（探测到服务仍在响应，可能被自动拉起，可再次执行 stop）");
            }
            Ok(s)
        }
        "restart" => {
            let kill = crate::agent::capability_broker::HostCapability::HdcKillServer;
            let kill_output = crate::agent::capability_broker::execute_host_capability(&kill, None, ctx)
                .await
                .map_err(|e| format!("hdc restart 的停止阶段失败：{e}"))?;
            if !kill_output.status.success() {
                let detail = smart_decode(&kill_output.stdout) + &smart_decode(&kill_output.stderr);
                return Err(format!("hdc restart 的停止阶段失败：{}", detail.trim()));
            }
            let start = crate::agent::capability_broker::HostCapability::HdcStartServer;
            let output = crate::agent::capability_broker::execute_host_capability(&start, None, ctx)
                .await
                .map_err(|e| format!("hdc start 失败：{e}"))?;
            let out = smart_decode(&output.stdout) + &smart_decode(&output.stderr);
            if !output.status.success() {
                return Err(format!("hdc start 失败：{}", out.trim()));
            }
            let mut s = format!("hdc 服务已重启。\n{}", out.trim_end());
            match probe().await {
                Some(ok) => s.push_str(&format!("\n✓ {ok}")),
                None => s.push_str("\n✗ 服务仍未响应，可稍后重试 manage_hdc action=status"),
            }
            Ok(s)
        }
        _ => unreachable!(),
    }
}

async fn broker_hdc_targets(ctx: &crate::agent::exec_ctx::ToolCtx) -> Result<String, String> {
    let capability = crate::agent::capability_broker::HostCapability::HdcListTargets;
    let output = crate::agent::capability_broker::execute_host_capability(&capability, None, ctx)
        .await?;
    let text = smart_decode(&output.stdout) + &smart_decode(&output.stderr);
    if !output.status.success() {
        return Err(format!("设备清单查询失败：{}", text.trim()));
    }
    Ok(smart_decode(&output.stdout))
}

async fn broker_emulator_command(
    capability: &crate::agent::capability_broker::HostCapability,
    label: &str,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let output = crate::agent::capability_broker::execute_host_capability(capability, None, ctx)
        .await
        .map_err(|error| format!("{label}失败：{error}"))?;
    let text = smart_decode(&output.stdout) + &smart_decode(&output.stderr);
    if !output.status.success() {
        return Err(format!("{label}失败：{}", text.trim()));
    }
    Ok(smart_decode(&output.stdout))
}

pub(super) async fn list_emulators(
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let emu = tokio::task::spawn_blocking(
        crate::agent::capability_broker::emulator_executable,
    )
        .await
        .map_err(|e| format!("查找模拟器任务失败: {e}"))?;
    let Some(emu) = emu else {
        return Err(
            "未找到 DevEco Studio 模拟器（Emulator.exe）。请先安装 DevEco Studio 并创建至少一个模拟器实例（DevEco Studio → Device Manager → 新建模拟器）。"
                .into(),
        );
    };
    let query = crate::agent::capability_broker::HostCapability::QueryEmulator {
        kind: crate::agent::capability_broker::EmulatorQueryKind::Instances,
    };
    let out = broker_emulator_command(&query, "运行模拟器列表命令", ctx).await?;
    let names: Vec<&str> = out.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    if names.is_empty() {
        return Ok(format!(
            "模拟器工具可用（{}），但尚未创建任何实例。\n请在 DevEco Studio 的 Device Manager 中新建模拟器（选机型与系统版本），创建后再次调用本工具即可看到。",
            emu.display()
        ));
    }
    // 标注已在线的实例（hdc 里含 localhost/127.0.0.1 设备的粗略判断）
    let online = broker_hdc_targets(ctx).await.unwrap_or_default();
    let has_local = online.contains("127.0.0.1") || online.contains("localhost");
    let mut s = format!(
        "DevEco Studio 模拟器实例（{} 个，工具：{}）：\n",
        names.len(),
        emu.display()
    );
    for n in &names {
        s.push_str(&format!("- {n}{}\n", if has_local { "（可能有实例已在线，用 list_devices 确认）" } else { "" }));
    }
    s.push_str("\n启动：start_emulator name=<实例名>；停止：start_emulator action=stop name=<实例名>。");
    Ok(s)
}

pub(super) async fn start_emulator(
    args: &Value,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let name = args["name"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());
    let Some(name) = name else {
        return Err("start_emulator 需要 name（实例名，先用 list_emulators 查看）".into());
    };
    let action = args["action"].as_str().unwrap_or("start").trim();
    if !matches!(action, "start" | "stop") {
        return Err("action 仅支持 start|stop".into());
    }
    // 校验实例存在（-list 输出逐行是实例名）
    let query = crate::agent::capability_broker::HostCapability::QueryEmulator {
        kind: crate::agent::capability_broker::EmulatorQueryKind::Instances,
    };
    let list_out = broker_emulator_command(&query, "读取模拟器列表", ctx).await?;
    let exists = list_out.lines().any(|l| l.trim() == name);
    if !exists {
        let names: Vec<&str> = list_out.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
        return Err(format!(
            "实例 {name} 不存在。可用实例：{}\n（若需新建请在 DevEco Studio Device Manager 中操作）",
            if names.is_empty() { "（无）".to_string() } else { names.join(", ") }
        ));
    }
    if action == "stop" {
        let stop = crate::agent::capability_broker::HostCapability::StopEmulator {
            name: name.to_string(),
        };
        let out = broker_emulator_command(&stop, "停止模拟器", ctx).await?;
        return Ok(format!("已发送停止指令：{name}\n{}", out.trim_end()));
    }
    // HDC 只能提供设备上线证据，不能证明设备属于指定模拟器实例。
    let wait_secs = args["wait_secs"].as_u64().unwrap_or(60).clamp(5, 120);
    let baseline = broker_hdc_targets(ctx).await
        .map_err(|error| format!("未启动模拟器：无法取得启动前设备基线：{error}"))?;
    let before = online_target_set(&baseline);
    // start 是 GUI 长进程：Broker 在 spawn 前 claim，spawn 成功即记录派发终态；
    // 真实启动效果由下面独立的 HDC 查询验证。
    let start = crate::agent::capability_broker::HostCapability::StartEmulator {
        name: name.to_string(),
    };
    let dispatched_pid = crate::agent::capability_broker::dispatch_host_capability(&start, ctx)
        .map_err(|error| format!("启动模拟器失败：{error}"))?;
    let pid_note = dispatched_pid
        .map(|pid| pid.to_string())
        .unwrap_or_else(|| "进程已快速转交".into());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait_secs);
    let mut seen = String::new();
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        {
            let t = broker_hdc_targets(ctx).await.map_err(|error| format!(
                "模拟器 {name} 已派发（PID：{pid_note}），但上线验证中断，实际状态未知；请查询 list_devices，勿自动重复启动：{error}"
            ))?;
            let now_set = online_target_set(&t);
            let mut new: Vec<&String> = now_set.difference(&before).collect();
            new.sort();
            if !new.is_empty() {
                seen = new.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
                break;
            }
        }
    }
    if seen.is_empty() {
        Ok(format!(
            "模拟器 {name} 已后台派发（PID：{pid_note}；{wait_secs}s 内 hdc 未发现新设备）。\n模拟器首次冷启动可能需要 1-3 分钟，稍后调用 list_devices 确认在线；若始终未上线，检查 DevEco Studio 模拟器窗口是否有报错。"
        ))
    } else {
        Ok(format!(
            "模拟器 {name} 已后台派发（PID：{pid_note}）；观察到新增已授权在线设备：{seen}。\n尚不能证明这些设备属于实例 {name}，请用 list_devices 核对后显式选择部署目标，勿自动部署到新设备。"
        ))
    }
}

fn online_target_set(text: &str) -> std::collections::HashSet<String> {
    crate::commands::devices::online_device_ids_from_targets(text).into_iter().collect()
}


fn validate_instance_transition(list: &str, name: &str, action: &str) -> Result<(), String> {
    let exists = list.lines().any(|line| line.trim() == name);
    match (action, exists) {
        ("create", true) => Err(format!("实例 {name} 已存在，拒绝覆盖或将旧实例误报为新建成功")),
        ("delete", false) => Err(format!("实例 {name} 不存在，未执行删除")),
        _ => Ok(()),
    }
}

pub(super) async fn create_emulator(
    args: &Value,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let action = args["action"].as_str().unwrap_or("create").trim();
    if !matches!(action, "create" | "delete" | "images" | "models") {
        return Err("action 仅支持 create|delete|images|models".into());
    }
    // 镜像/机型查询是 replay-safe 的固定 Broker 能力。
    if action == "images" {
        let query = crate::agent::capability_broker::HostCapability::QueryEmulator {
            kind: crate::agent::capability_broker::EmulatorQueryKind::DownloadedImages,
        };
        let out = broker_emulator_command(&query, "查询镜像", ctx).await?;
        let body = out.trim();
        if body.is_empty() {
            return Ok("尚未下载任何模拟器系统镜像。\n可调用 create_emulator action=models 查看支持机型，或直接在 DevEco Studio Device Manager 中下载/创建。".into());
        }
        return Ok(format!("已下载的模拟器系统镜像：\n{body}\n\n创建实例时 os_version 传镜像对应的版本字符串（如 HarmonyOS 6.0.0(20)）。"));
    }
    if action == "models" {
        let query = crate::agent::capability_broker::HostCapability::QueryEmulator {
            kind: crate::agent::capability_broker::EmulatorQueryKind::ScreenProfiles,
        };
        let out = broker_emulator_command(&query, "查询机型", ctx).await?;
        let body = out.trim();
        return Ok(if body.is_empty() {
            "未获取到机型列表（可按设备类型创建：Phone/Foldable/Tablet/2in1/Wearable/TV 等）。".into()
        } else {
            format!("支持的模拟器机型/设备类型：\n{}", super::cmd_tools::cut_str(body, 2000))
        });
    }
    let name = args["name"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());
    let Some(name) = name else {
        return Err(format!("create_emulator {action} 需要 name（实例名）"));
    };
    let query = crate::agent::capability_broker::HostCapability::QueryEmulator {
        kind: crate::agent::capability_broker::EmulatorQueryKind::Instances,
    };
    let baseline = broker_emulator_command(&query, "读取实例变更前清单", ctx).await?;
    validate_instance_transition(&baseline, name, action)?;
    if action == "delete" {
        let delete = crate::agent::capability_broker::HostCapability::DeleteEmulator {
            name: name.to_string(),
        };
        let out = broker_emulator_command(&delete, "删除实例", ctx).await?;
        let verification = crate::agent::capability_broker::HostCapability::QueryEmulator {
            kind: crate::agent::capability_broker::EmulatorQueryKind::Instances,
        };
        let remaining = broker_emulator_command(&verification, "验证实例删除结果", ctx)
            .await
            .map_err(|error| format!("删除命令已返回成功，但无法验证实例清单：{error}"))?;
        if remaining.lines().any(|line| line.trim() == name) {
            return Err(format!("删除命令已返回成功，但实例 {name} 仍在清单中"));
        }
        return Ok(format!("已删除模拟器实例 {name}。\n{}", out.trim_end()));
    }
    // create：校验 device_type 与 os_version
    let device_type = args["device_type"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());
    let os_version = args["os_version"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());
    let Some(device_type) = device_type else {
        return Err("create 需要 device_type（如 Phone/Foldable/Tablet，可用 create_emulator action=models 查看）".into());
    };
    let Some(os_version) = os_version else {
        return Err("create 需要 os_version（如 \"HarmonyOS 6.0.0(20)\"，先 create_emulator action=images 查看已下载版本）".into());
    };
    let screen_profile = args["screen_profile"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from);
    let memory_gb = match args.get("memory") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_u64().ok_or("memory 必须是 2-32 之间的整数")?),
    };
    let storage_gb = match args.get("storage") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_u64().ok_or("storage 必须是 2-1023 之间的整数")?),
    };
    let create = crate::agent::capability_broker::HostCapability::CreateEmulator {
        name: name.to_string(),
        device_type: device_type.to_string(),
        os_version: os_version.to_string(),
        screen_profile,
        memory_gb,
        storage_gb,
    };
    let out = broker_emulator_command(&create, "创建实例", ctx)
        .await
        .map_err(|e| {
            let hint = if e.contains("license") || e.to_lowercase().contains("agreement") {
                "\n（可能未接受许可协议：请在 DevEco Studio Device Manager 中接受模拟器许可，或先创建一次实例）"
            } else if e.contains("network") || e.contains("download") {
                "\n（可能需要下载系统镜像，首次创建耗时较长且需联网，可先 create_emulator action=images 查看进度）"
            } else {
                ""
            };
            format!("{e}{hint}")
        })?;
    let verification = crate::agent::capability_broker::HostCapability::QueryEmulator {
        kind: crate::agent::capability_broker::EmulatorQueryKind::Instances,
    };
    let instances = broker_emulator_command(&verification, "验证实例创建结果", ctx)
        .await
        .map_err(|error| format!("创建命令已返回成功，但无法验证实例清单：{error}"))?;
    if !instances.lines().any(|line| line.trim() == name) {
        return Err(format!("创建命令已返回成功，但实例 {name} 未出现在实例清单中"));
    }
    Ok(format!(
        "模拟器实例 {name} 创建完成（{device_type} / {os_version}）。\n{}
下一步：list_emulators 确认实例在列，start_emulator name={name} 启动。",
        out.trim_end()
    ))
}

pub(super) async fn device_file(
    args: &Value,
    roots: &[String],
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let action = args["action"].as_str().unwrap_or("").trim();
    if action != "push" && action != "pull" {
        return Err("device_file 参数 action 仅支持 push 或 pull".into());
    }
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id(ctx).await?,
    };
    let remote = args["remote"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());
    let Some(remote) = remote else {
        return Err("device_file 需要 remote（设备端路径）".into());
    };
    let project_path = roots.first().map(String::as_str).filter(|path| !path.is_empty())
        .ok_or("device_file 需要绑定项目工作区")?;
    let project_root = Path::new(project_path)
        .canonicalize()
        .map_err(|e| format!("无法解析项目工作区：{e}"))?;
    let local_arg = args["local"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());
    if let Some(local) = local_arg {
        let path = Path::new(local);
        if path.is_absolute()
            || path.components().any(|component| {
                matches!(component, std::path::Component::ParentDir | std::path::Component::RootDir)
            })
        {
            return Err("device_file 的 local 必须是工作区内且不含 .. 的相对路径".into());
        }
    }
    match action {
        "push" => {
            let local = local_arg.ok_or_else(|| "push 需要 local（本地文件路径）".to_string())?;
            let requested = resolve_local_path(local, project_path);
            let local_path = requested
                .canonicalize()
                .map_err(|e| format!("无法解析本地文件：{e}"))?;
            if !local_path.starts_with(&project_root) || !local_path.is_file() {
                return Err("push 的本地源必须是项目工作区内的普通文件".into());
            }
            let relative = local_path
                .strip_prefix(&project_root)
                .map_err(|_| "无法把本地源转换为工作区相对路径")?
                .to_string_lossy()
                .into_owned();
            let capability = crate::agent::capability_broker::HostCapability::SendFile {
                device: device.clone(), local_path: relative, remote_path: remote.to_string(),
            };
            let output = crate::agent::capability_broker::execute_host_capability(
                &capability, Some(&project_root), ctx,
            )
            .await
            .map_err(|e| format!("推送失败：{e}"))?;
            if !output.status.success() {
                return Err(format!(
                    "推送失败：{}",
                    (smart_decode(&output.stdout) + &smart_decode(&output.stderr)).trim()
                ));
            }
            Ok(format!("已推送 {} → {remote}（设备 {device}）", local_path.display()))
        }
        "pull" => {
            let local_path = match local_arg {
                Some(l) => resolve_local_path(l, project_path),
                None => {
                    let base = project_root.join(".deveco-agent").join("files");
                    let fname = Path::new(remote)
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| "file".to_string());
                    base.join(fname)
                }
            };
            let requested_parent = local_path.parent().ok_or("pull 的本地目标缺少父目录")?;
            let mut existing_ancestor = requested_parent;
            while !existing_ancestor.exists() {
                existing_ancestor = existing_ancestor.parent().ok_or("pull 的本地目标无法定位工作区祖先")?;
            }
            let ancestor = existing_ancestor
                .canonicalize().map_err(|e| format!("无法解析本地目标祖先目录：{e}"))?;
            if !ancestor.starts_with(&project_root) {
                return Err("pull 的本地目标父目录通过符号链接逃逸项目工作区".into());
            }
            std::fs::create_dir_all(requested_parent).map_err(|e| e.to_string())?;
            let parent = requested_parent
                .canonicalize().map_err(|e| format!("无法解析本地目标父目录：{e}"))?;
            if !parent.starts_with(&project_root) {
                return Err("pull 的本地目标必须位于项目工作区内，且父目录不得通过符号链接逃逸".into());
            }
            let file_name = local_path.file_name().ok_or("pull 的本地目标缺少文件名")?;
            let local_path = parent.join(file_name);
            let relative = local_path
                .strip_prefix(&project_root)
                .map_err(|_| "无法把本地目标转换为工作区相对路径")?
                .to_string_lossy()
                .into_owned();
            let capability = crate::agent::capability_broker::HostCapability::ReceiveFile {
                device: device.clone(), remote_path: remote.to_string(), local_path: relative,
            };
            let output = crate::agent::capability_broker::execute_host_capability(
                &capability, Some(&project_root), ctx,
            )
            .await
            .map_err(|e| format!("拉取失败：{e}"))?;
            if !output.status.success() {
                return Err(format!(
                    "拉取失败：{}",
                    (smart_decode(&output.stdout) + &smart_decode(&output.stderr)).trim()
                ));
            }
            if !local_path.exists() {
                return Err("拉取失败：本地文件未生成（设备端路径可能不存在或权限受限）".into());
            }
            Ok(format!("已拉取 {remote} → {}（设备 {device}）", local_path.display()))
        }
        _ => unreachable!(),
    }
}

pub(super) fn resolve_local_path(p: &str, project_path: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else if !project_path.is_empty() {
        Path::new(project_path).join(path)
    } else {
        path.to_path_buf()
    }
}

pub(super) async fn stop_app(
    args: &Value,
    roots: &[String],
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id(ctx).await?,
    };
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    let bundle = match args["bundle"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(b) => b.to_string(),
        None => {
            if project_path.is_empty() {
                return Err("未指定 bundle 且当前会话未绑定工程".into());
            }
            crate::services::harmony::parse_project(Path::new(project_path))
                .bundle_name
                .ok_or_else(|| "未指定 bundle 且工程未解析出 bundleName".to_string())?
        }
    };
    let capability = crate::agent::capability_broker::HostCapability::StopAbility {
        device: device.clone(), bundle: bundle.clone(),
    };
    let output = crate::agent::capability_broker::execute_host_capability(&capability, None, ctx)
        .await?;
    if !output.status.success() {
        return Err(format!(
            "停止应用失败：{}",
            (smart_decode(&output.stdout) + &smart_decode(&output.stderr)).trim()
        ));
    }
    Ok(format!(
        "已强制停止 {bundle}（设备 {device}）。\n后续建议：start_ability 重新启动验证冷启动；collect_perf 采样冷启动性能。"
    ))
}

pub(super) fn validate_device_shell_command(command: &str) -> Result<Vec<&str>, String> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let owned = tokens.iter().map(|token| (*token).to_string()).collect::<Vec<_>>();
    crate::agent::capability_broker::validate_read_only_device_command(&owned)?;
    Ok(tokens)
}

pub(super) async fn device_shell(
    args: &Value,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id(ctx).await?,
    };
    let command = args["command"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());
    let Some(command) = command else {
        return Err("device_shell 需要 command（设备端命令串）".into());
    };
    // 四重安全校验（纯函数，便于单元测试）
    let tokens = validate_device_shell_command(command)?;
    let capability = crate::agent::capability_broker::HostCapability::DeviceReadQuery {
        device: device.clone(),
        argv: tokens.iter().map(|token| (*token).to_string()).collect(),
    };
    let output = crate::agent::capability_broker::execute_host_capability(&capability, None, ctx)
        .await?;
    let decoded = smart_decode(&output.stdout) + &smart_decode(&output.stderr);
    if !output.status.success() {
        return Err(format!("设备查询失败：{}", decoded.trim()));
    }
    let out = decoded.trim_end();
    if out.is_empty() {
        return Ok(format!("命令执行成功（设备 {device}），无输出"));
    }
    let truncated = if out.chars().count() > 3000 {
        format!("{}…\n（输出过长已截断）", out.chars().take(3000).collect::<String>())
    } else {
        out.to_string()
    };
    Ok(format!("设备 {device} 执行 `{command}`：\n{truncated}"))
}

pub(super) async fn analyze_crash(
    args: &Value,
    roots: &[String],
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id(ctx).await?,
    };
    let project_path = roots.first().map(String::as_str).filter(|path| !path.is_empty())
        .ok_or("analyze_crash 需要绑定项目工作区")?;
    let project_root = Path::new(project_path)
        .canonicalize()
        .map_err(|e| format!("无法解析项目工作区：{e}"))?;
    let bundle = match args["bundle"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(b) => b.to_string(),
        None => {
            crate::services::harmony::parse_project(&project_root)
                .bundle_name
                .unwrap_or_default()
        }
    };
    let limit = args["limit"].as_u64().unwrap_or(3).clamp(1, 10) as usize;
    // 1) 扫描 faultlog 目录（真机权限可能受限，多个候选目录逐个尝试）
    use crate::agent::capability_broker::{FaultLogDirectory, HostCapability};
    let dirs = [FaultLogDirectory::FaultLogger, FaultLogDirectory::Temp, FaultLogDirectory::Root];
    let mut remote_files: Vec<String> = Vec::new();
    for directory in dirs {
        let mut ok = false;
        let capability = HostCapability::ListFaultLogs {
            device: device.clone(), directory,
        };
        if let Ok(output) = crate::agent::capability_broker::execute_host_capability(
            &capability, None, ctx,
        ).await {
            if !output.status.success() {
                continue;
            }
            let listing = smart_decode(&output.stdout) + &smart_decode(&output.stderr);
            for line in listing.lines() {
                // 多列输出兼容：按空白拆分逐个取文件名
                for name in line.split_whitespace() {
                    let name = name.trim();
                    if !is_safe_faultlog_name(name)
                        || name.contains("denied")
                    {
                        continue;
                    }
                    remote_files.push(format!("{}/{name}", directory.as_path()));
                    ok = true;
                }
            }
        }
        if ok {
            break;
        }
    }
    if remote_files.is_empty() {
        return Err(format!(
            "无法读取设备 faultlog 目录（设备 {device}，真机 /data 目录通常需要 root 权限）。\n建议改用 read_runtime_logs 查看实时错误日志，或用 device_file 拉取已知路径的文件。"
        ));
    }
    // 2) 按 bundle 过滤
    if !bundle.is_empty() {
        let before = remote_files.len();
        remote_files.retain(|f| f.contains(&bundle));
        if remote_files.is_empty() {
            return Err(format!("faultlog 中未找到 {bundle} 的崩溃记录（共 {before} 条其他记录）"));
        }
    }
    // 3) 按文件名内嵌时间戳排序取最近 N 条
    remote_files.sort_by_key(|a| std::cmp::Reverse(crash_time_key(a)));
    remote_files.truncate(limit);
    // 4) 拉取到本地并解析
    let requested_base = project_root.join(".deveco-agent").join("crashes");
    let mut existing_ancestor = requested_base.as_path();
    while !existing_ancestor.exists() {
        existing_ancestor = existing_ancestor
            .parent()
            .ok_or("崩溃副本目录无法定位工作区祖先")?;
    }
    let ancestor = existing_ancestor
        .canonicalize()
        .map_err(|e| format!("无法解析崩溃副本目录祖先：{e}"))?;
    if !ancestor.starts_with(&project_root) {
        return Err("崩溃副本目录通过符号链接逃逸项目工作区".into());
    }
    std::fs::create_dir_all(&requested_base).map_err(|e| e.to_string())?;
    let base = requested_base
        .canonicalize()
        .map_err(|e| format!("无法解析崩溃副本目录：{e}"))?;
    if !base.starts_with(&project_root) {
        return Err("崩溃副本目录必须位于项目工作区内".into());
    }
    let mut out = format!("崩溃分析（设备 {device}，{} 条）：\n", remote_files.len());
    for (i, remote) in remote_files.iter().enumerate() {
        let fname = Path::new(remote)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("crash-{i}.log"));
        let local = base.join(&fname);
        let relative = local
            .strip_prefix(&project_root)
            .map_err(|_| "无法把崩溃副本转换为工作区相对路径")?
            .to_string_lossy()
            .into_owned();
        let capability = HostCapability::ReceiveFile {
            device: device.clone(), remote_path: remote.clone(), local_path: relative,
        };
        let received = crate::agent::capability_broker::execute_host_capability(
            &capability, Some(&project_root), ctx,
        ).await;
        if !matches!(received, Ok(ref output) if output.status.success()) || !local.exists() {
            out.push_str(&format!("\n[{}] {fname}：拉取失败（权限受限）\n", i + 1));
            continue;
        }
        let content = std::fs::read_to_string(&local).unwrap_or_default();
        out.push_str(&format!("\n[{}] {fname}（{} KB）\n", i + 1, content.len() / 1024));
        out.push_str(&summarize_crash_file(&content));
        out.push_str(&format!("\n本地副本：{}\n", local.display()));
    }
    out.push_str("\n建议：结合 read_runtime_logs 查看崩溃前后的运行日志；修复后重新部署验证。");
    Ok(out)
}

pub(super) fn is_safe_faultlog_name(name: &str) -> bool {
    crate::agent::capability_broker::validate_faultlog_filename(name).is_ok()
}

pub(super) fn crash_time_key(name: &str) -> u64 {
    let bytes: Vec<char> = name.chars().collect();
    let mut best: u64 = 0;
    let mut i = 0;
    while i + 14 <= bytes.len() {
        if bytes[i..i + 14].iter().all(|c| c.is_ascii_digit()) {
            let s: String = bytes[i..i + 14].iter().collect();
            if let Ok(v) = s.parse::<u64>() {
                best = best.max(v);
            }
        }
        i += 1;
    }
    best
}

pub(super) fn summarize_crash_file(content: &str) -> String {
    let keys = [
        "Reason", "reason", "Exception", "exception", "JS Crash", "Native Crash",
        "App Freeze", "Fault thread", "Fault", "Process name", "Process", "pid",
        "Signal", "Backtrace", "Stacktrace", "Caused by", "Summary", "Thread name",
    ];
    let mut hits: Vec<String> = Vec::new();
    for line in content.lines().take(300) {
        if hits.len() >= 12 {
            break;
        }
        if keys.iter().any(|k| line.contains(k)) {
            hits.push(line.trim().to_string());
        }
    }
    let mut s = String::new();
    if !hits.is_empty() {
        s.push_str(&format!("关键信息：\n{}\n", hits.join("\n")));
    }
    s.push_str("堆栈片段（前 20 行）：\n");
    let mut shown = 0;
    for line in content
        .lines()
        .skip_while(|l| !l.contains("stack") && !l.contains("Stack") && !l.contains("Backtrace"))
    {
        s.push_str(line);
        s.push('\n');
        shown += 1;
        if shown >= 20 {
            break;
        }
    }
    if s.chars().count() > 1500 {
        s = s.chars().take(1500).collect::<String>();
        s.push_str("…\n");
    }
    s
}

#[cfg(test)]
mod emulator_boundary_tests {
    use super::*;

    #[test]
    fn online_evidence_excludes_empty_offline_and_unauthorized_targets() {
        let targets = online_target_set("[Empty]\nold Offline\nlocked Unauthorized\npending Unknown\nnew Connected\nnew Connected\nlegacy\n");
        assert_eq!(targets, ["new".to_string(), "legacy".to_string()].into_iter().collect());
        let before = online_target_set("device Offline\n");
        let after = online_target_set("device Connected\n");
        assert_eq!(after.difference(&before).count(), 1);
    }

    #[test]
    fn instance_transition_requires_exact_precondition() {
        assert!(validate_instance_transition("Phone\nPhone2\n", "Phone", "create").is_err());
        assert!(validate_instance_transition("Phone2\n", "Phone", "create").is_ok());
        assert!(validate_instance_transition("Phone2\n", "Phone", "delete").is_err());
        assert!(validate_instance_transition(" Phone \r\n", "Phone", "delete").is_ok());
    }
}
