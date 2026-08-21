//! 设备管理域工具：无线连接 / hdc 服务 / 模拟器 / 文件传输 / 进程停止 / 受限 shell / 崩溃取证。
//! 共享辅助函数（run_cmd / run_hdc_shell / default_device_id / tail 等）在父模块 mod.rs，
//! 本模块通过 `use super::*` 继承访问。

use super::*;

pub(super) async fn connect_device(args: &Value) -> Result<String, String> {
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
            let out = run_cmd("hdc", &["tconn".into(), target.clone()], None, 30).await
                .map_err(|e| format!("无线连接失败：{e}"))?;
            let out = out.trim();
            Ok(format!(
                "无线连接请求已发送：{target}\n设备输出：{}\n下一步：调用 list_devices 确认设备在线；部署/截图/日志时 device 参数填 {target}。",
                if out.is_empty() { "(无输出，通常表示连接成功)" } else { out }
            ))
        }
        "disconnect" => {
            let out = run_cmd("hdc", &["tconn".into(), "-d".into(), target.clone()], None, 30).await
                .map_err(|e| format!("断开失败：{e}"))?;
            Ok(format!("已断开 {target}\n设备输出：{}", out.trim()))
        }
        "list" => list_devices().await,
        _ => Err(format!("action 仅支持 connect|disconnect|list，收到 {action}")),
    }
}

pub(super) async fn manage_hdc(args: &Value, db: &crate::db::DbState) -> Result<String, String> {
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
        match run_cmd(&hdc, &["list".into(), "targets".into()], None, 15).await {
            Ok(t) => {
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
            Err(_) => None,
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
            let out = run_cmd(&hdc, &["start".into()], None, 30)
                .await
                .map_err(|e| format!("hdc start 失败：{e}"))?;
            let mut s = format!("hdc start 执行完成。\n{}", out.trim_end());
            if let Some(ok) = probe().await {
                s.push_str(&format!("\n✓ {ok}"));
            } else {
                s.push_str("\n✗ 服务仍未响应，可稍后重试 manage_hdc action=status");
            }
            Ok(s)
        }
        "stop" => {
            let out = run_cmd(&hdc, &["kill".into()], None, 30)
                .await
                .map_err(|e| format!("hdc kill 失败：{e}"))?;
            let mut s = format!("hdc 服务已停止。\n{}", out.trim_end());
            if probe().await.is_some() {
                s.push_str("\n（探测到服务仍在响应，可能被自动拉起，可再次执行 stop）");
            }
            Ok(s)
        }
        "restart" => {
            let _ = run_cmd(&hdc, &["kill".into()], None, 20).await;
            let out = run_cmd(&hdc, &["start".into()], None, 30)
                .await
                .map_err(|e| format!("hdc start 失败：{e}"))?;
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

pub(super) fn emulator_exe() -> Option<PathBuf> {
    for dir in crate::commands::health::discover_deveco_dirs() {
        for rel in [
            "tools/emulator/Emulator.exe",
            "sdk/emulator/Emulator.exe",
            "emulator/Emulator.exe",
        ] {
            let p = dir.join(rel);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    for p in [
        r"C:\Program Files\Huawei\DevEco Studio\tools\emulator\Emulator.exe",
        r"D:\Huawei\DevEco Studio\tools\emulator\Emulator.exe",
        r"C:\Program Files\Huawei\DevEco Studio\sdk\emulator\Emulator.exe",
    ] {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    None
}

pub(super) async fn list_emulators() -> Result<String, String> {
    // emulator_exe 内部走 discover_deveco_dirs（reg query 等同步 IO），放入 blocking 线程池
    let emu = tokio::task::spawn_blocking(emulator_exe)
        .await
        .map_err(|e| format!("查找模拟器任务失败: {e}"))?;
    let Some(emu) = emu else {
        return Err(
            "未找到 DevEco Studio 模拟器（Emulator.exe）。请先安装 DevEco Studio 并创建至少一个模拟器实例（DevEco Studio → Device Manager → 新建模拟器）。"
                .into(),
        );
    };
    let out = run_cmd(
        &emu.to_string_lossy(),
        &["-list".into()],
        None,
        30,
    )
    .await
    .map_err(|e| format!("运行模拟器列表命令失败：{e}"))?;
    let names: Vec<&str> = out.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    if names.is_empty() {
        return Ok(format!(
            "模拟器工具可用（{}），但尚未创建任何实例。\n请在 DevEco Studio 的 Device Manager 中新建模拟器（选机型与系统版本），创建后再次调用本工具即可看到。",
            emu.display()
        ));
    }
    // 标注已在线的实例（hdc 里含 localhost/127.0.0.1 设备的粗略判断）
    let online = run_cmd("hdc", &["list".into(), "targets".into()], None, 15)
        .await
        .unwrap_or_default();
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

pub(super) async fn start_emulator(args: &Value) -> Result<String, String> {
    let name = args["name"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());
    let Some(name) = name else {
        return Err("start_emulator 需要 name（实例名，先用 list_emulators 查看）".into());
    };
    let action = args["action"].as_str().unwrap_or("start").trim();
    if !matches!(action, "start" | "stop") {
        return Err("action 仅支持 start|stop".into());
    }
    let Some(emu) = emulator_exe() else {
        return Err("未找到 DevEco Studio 模拟器（Emulator.exe），请先安装 DevEco Studio".into());
    };
    // 校验实例存在（-list 输出逐行是实例名）
    let list_out = run_cmd(&emu.to_string_lossy(), &["-list".into()], None, 30)
        .await
        .map_err(|e| format!("读取模拟器列表失败：{e}"))?;
    let exists = list_out.lines().any(|l| l.trim() == name);
    if !exists {
        let names: Vec<&str> = list_out.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
        return Err(format!(
            "实例 {name} 不存在。可用实例：{}\n（若需新建请在 DevEco Studio Device Manager 中操作）",
            if names.is_empty() { "（无）".to_string() } else { names.join(", ") }
        ));
    }
    if action == "stop" {
        let out = run_cmd(&emu.to_string_lossy(), &["-stop".into(), name.to_string()], None, 60)
            .await
            .map_err(|e| format!("停止模拟器失败：{e}"))?;
        return Ok(format!("已发送停止指令：{name}\n{}", out.trim_end()));
    }
    // start：后台拉起（模拟器有 GUI 窗口，不隐藏、不等待退出）
    let mut cmd = crate::utils::process::command(&emu.to_string_lossy(), &["-start".into(), name.to_string()])
        .map_err(|e| e.to_string())?;
    let _child = cmd.spawn().map_err(|e| format!("启动模拟器失败：{e}"))?;
    // 轮询 hdc：启动前设备快照 → 新设备出现即上线
    let wait_secs = args["wait_secs"].as_u64().unwrap_or(60).clamp(5, 120);
    let before: std::collections::HashSet<String> = run_cmd("hdc", &["list".into(), "targets".into()], None, 15)
        .await
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_whitespace().next().map(String::from))
        .collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait_secs);
    let mut seen = String::new();
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        match run_cmd("hdc", &["list".into(), "targets".into()], None, 15).await {
            Ok(t) => {
                let now_set: std::collections::HashSet<String> = t
                    .lines()
                    .filter_map(|l| l.split_whitespace().next().map(String::from))
                    .collect();
                let new: Vec<&String> = now_set.difference(&before).collect();
                if !new.is_empty() {
                    seen = new.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
                    break;
                }
            }
            Err(_) => {}
        }
    }
    if seen.is_empty() {
        Ok(format!(
            "模拟器 {name} 已后台启动（{wait_secs}s 内 hdc 未发现新设备）。\n模拟器首次冷启动可能需要 1-3 分钟，稍后调用 list_devices 确认在线；若始终未上线，检查 DevEco Studio 模拟器窗口是否有报错。"
        ))
    } else {
        Ok(format!(
            "模拟器 {name} 已启动，新设备上线：{seen}\n下一步：list_devices 查看详情后即可部署/测试（deploy 会部署到全部在线设备，注意区分真机与模拟器）。"
        ))
    }
}

pub(super) async fn create_emulator(args: &Value) -> Result<String, String> {
    let action = args["action"].as_str().unwrap_or("create").trim();
    if !matches!(action, "create" | "delete" | "images" | "models") {
        return Err("action 仅支持 create|delete|images|models".into());
    }
    let Some(emu) = emulator_exe() else {
        return Err("未找到 DevEco Studio 模拟器（Emulator.exe），请先安装 DevEco Studio".into());
    };
    let exe = emu.to_string_lossy();
    // 镜像/机型查询：无参数副作用，直接执行
    if action == "images" {
        let out = run_cmd(&exe, &["-imageList".into(), "-downloaded".into()], None, 60)
            .await
            .map_err(|e| format!("查询镜像失败：{e}"))?;
        let body = out.trim();
        if body.is_empty() {
            return Ok("尚未下载任何模拟器系统镜像。\n可调用 create_emulator action=models 查看支持机型，或直接在 DevEco Studio Device Manager 中下载/创建。".into());
        }
        return Ok(format!("已下载的模拟器系统镜像：\n{body}\n\n创建实例时 os_version 传镜像对应的版本字符串（如 HarmonyOS 6.0.0(20)）。"));
    }
    if action == "models" {
        let out = run_cmd(&exe, &["-screenProfileList".into()], None, 60)
            .await
            .map_err(|e| format!("查询机型失败：{e}"))?;
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
    if action == "delete" {
        let out = run_cmd(&exe, &["-delete".into(), name.to_string(), "-force".into()], None, 60)
            .await
            .map_err(|e| format!("删除实例失败：{e}"))?;
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
    let mut cmd_args: Vec<String> = vec![
        "-create".into(),
        name.to_string(),
        "-deviceType".into(),
        device_type.to_string(),
        "-osVersion".into(),
        os_version.to_string(),
    ];
    if let Some(sp) = args["screen_profile"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        cmd_args.extend(["-screenProfile".into(), sp.to_string()]);
    }
    let memory = args["memory"].as_u64().unwrap_or(4);
    if (2..=32).contains(&memory) && memory != 4 {
        cmd_args.extend(["-memory".into(), memory.to_string()]);
    }
    let storage = args["storage"].as_u64().unwrap_or(6);
    if (2..=1023).contains(&storage) && storage != 6 {
        cmd_args.extend(["-storage".into(), storage.to_string()]);
    }
    let out = run_cmd(&exe, &cmd_args, None, 180)
        .await
        .map_err(|e| {
            let hint = if e.contains("license") || e.to_lowercase().contains("agreement") {
                "\n（可能未接受许可协议：请在 DevEco Studio Device Manager 中接受模拟器许可，或先创建一次实例）"
            } else if e.contains("network") || e.contains("download") {
                "\n（可能需要下载系统镜像，首次创建耗时较长且需联网，可先 create_emulator action=images 查看进度）"
            } else {
                ""
            };
            format!("创建实例失败：{e}{hint}")
        })?;
    Ok(format!(
        "模拟器实例 {name} 创建完成（{device_type} / {os_version}）。\n{}
下一步：list_emulators 确认实例在列，start_emulator name={name} 启动。",
        out.trim_end()
    ))
}

pub(super) async fn device_file(args: &Value, roots: &[String]) -> Result<String, String> {
    let action = args["action"].as_str().unwrap_or("").trim();
    if action != "push" && action != "pull" {
        return Err("device_file 参数 action 仅支持 push 或 pull".into());
    }
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let remote = args["remote"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());
    let Some(remote) = remote else {
        return Err("device_file 需要 remote（设备端路径）".into());
    };
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    let local_arg = args["local"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());
    match action {
        "push" => {
            let local = local_arg.ok_or_else(|| "push 需要 local（本地文件路径）".to_string())?;
            let local_path = resolve_local_path(local, project_path);
            if !local_path.is_file() {
                return Err(format!("本地文件不存在：{}", local_path.display()));
            }
            let hdc_args: Vec<String> = vec![
                "-t".into(), device.clone(), "file".into(), "send".into(),
                local_path.to_string_lossy().to_string(), remote.to_string(),
            ];
            run_cmd("hdc", &hdc_args, None, 120).await.map_err(|e| format!("推送失败：{e}"))?;
            Ok(format!("已推送 {} → {remote}（设备 {device}）", local_path.display()))
        }
        "pull" => {
            let local_path = match local_arg {
                Some(l) => resolve_local_path(l, project_path),
                None => {
                    // 缺省保存到工程 .deveco-agent/files/（无工程时用系统临时目录）
                    let base = if project_path.is_empty() {
                        std::env::temp_dir().join("deveco-agent-files")
                    } else {
                        Path::new(project_path).join(".deveco-agent").join("files")
                    };
                    let fname = Path::new(remote)
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| "file".to_string());
                    base.join(fname)
                }
            };
            if let Some(parent) = local_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let hdc_args: Vec<String> = vec![
                "-t".into(), device.clone(), "file".into(), "recv".into(),
                remote.to_string(), local_path.to_string_lossy().to_string(),
            ];
            run_cmd("hdc", &hdc_args, None, 120).await.map_err(|e| format!("拉取失败：{e}"))?;
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

pub(super) async fn stop_app(args: &Value, roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
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
    run_hdc_shell(&device, &["aa", "force-stop", &bundle], 20).await?;
    Ok(format!(
        "已强制停止 {bundle}（设备 {device}）。\n后续建议：start_ability 重新启动验证冷启动；collect_perf 采样冷启动性能。"
    ))
}

pub(super) fn validate_device_shell_command(command: &str) -> Result<Vec<&str>, String> {
    if !command
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || " /._-:+=,[%]".contains(c))
    {
        return Err(format!(
            "device_shell 拒绝执行包含 shell 元字符的命令（仅允许字母/数字/空格及 / . _ - : + = , [ ] %）：{command}"
        ));
    }
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let Some(cmd) = tokens.first().copied() else {
        return Err("device_shell 命令不能为空".into());
    };
    if !DEVICE_SHELL_ALLOWED.contains(&cmd) {
        return Err(format!(
            "命令 {cmd} 不在 device_shell 白名单（{}）；如需修改设备状态请用对应专用工具",
            DEVICE_SHELL_ALLOWED.join("/")
        ));
    }
    if let Some(bad) = DEVICE_SHELL_FORBIDDEN_TOKENS
        .iter()
        .find(|t| command.split_whitespace().any(|w| w.starts_with(**t)))
    {
        return Err(format!("device_shell 拒绝破坏性命令 {bad}，请使用对应专用工具"));
    }
    if cmd == "aa" && !tokens.iter().skip(1).any(|t| *t == "dump") {
        return Err("device_shell 中 aa 仅允许 dump 查询子命令；启动/停止应用请用 start_ability/stop_app".into());
    }
    if cmd == "bm" && !tokens.iter().skip(1).any(|t| *t == "dump") {
        return Err("device_shell 中 bm 仅允许 dump 查询子命令；安装/卸载请用 deploy/uninstall_app".into());
    }
    Ok(tokens)
}

pub(super) async fn device_shell(args: &Value) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let command = args["command"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());
    let Some(command) = command else {
        return Err("device_shell 需要 command（设备端命令串）".into());
    };
    // 四重安全校验（纯函数，便于单元测试）
    let tokens = validate_device_shell_command(command)?;
    let out = run_hdc_shell(&device, &tokens, 30).await?;
    let out = out.trim_end();
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

pub(super) async fn analyze_crash(args: &Value, roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    let bundle = match args["bundle"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(b) => b.to_string(),
        None => {
            if project_path.is_empty() {
                String::new()
            } else {
                crate::services::harmony::parse_project(Path::new(project_path))
                    .bundle_name
                    .unwrap_or_default()
            }
        }
    };
    let limit = args["limit"].as_u64().unwrap_or(3).clamp(1, 10) as usize;
    // 1) 扫描 faultlog 目录（真机权限可能受限，多个候选目录逐个尝试）
    let dirs = ["/data/log/faultlog/faultlogger", "/data/log/faultlog/temp", "/data/log/faultlog"];
    let mut remote_files: Vec<String> = Vec::new();
    for dir in dirs {
        let mut ok = false;
        if let Ok(out) = run_hdc_shell(&device, &["ls", "-1", dir], 15).await {
            for line in out.lines() {
                // 多列输出兼容：按空白拆分逐个取文件名
                for name in line.split_whitespace() {
                    let name = name.trim();
                    if name.is_empty()
                        || name.starts_with('.')
                        || name.contains(':')
                        || name.contains("denied")
                        || !name.chars().any(|c| c.is_ascii_digit())
                    {
                        continue;
                    }
                    remote_files.push(format!("{dir}/{name}"));
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
    remote_files.sort_by(|a, b| crash_time_key(b).cmp(&crash_time_key(a)));
    remote_files.truncate(limit);
    // 4) 拉取到本地并解析
    let base = if project_path.is_empty() {
        std::env::temp_dir().join("deveco-agent-crashes")
    } else {
        Path::new(project_path).join(".deveco-agent").join("crashes")
    };
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let mut out = format!("崩溃分析（设备 {device}，{} 条）：\n", remote_files.len());
    for (i, remote) in remote_files.iter().enumerate() {
        let fname = Path::new(remote)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("crash-{i}.log"));
        let local = base.join(&fname);
        let hdc_args: Vec<String> = vec![
            "-t".into(), device.clone(), "file".into(), "recv".into(),
            remote.clone(), local.to_string_lossy().to_string(),
        ];
        if run_cmd("hdc", &hdc_args, None, 60).await.is_err() || !local.exists() {
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
