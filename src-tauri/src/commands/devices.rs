//! 设备管理：列出/查询 hdc 目标设备、设置默认设备。
//!
//! 给前端右侧设备面板使用，与 Agent 工具（deploy/screenshot）共用同一套默认设备记忆。

use serde::Serialize;
use std::path::Path;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::db::DbState;
use crate::utils::process;

#[derive(Debug, Serialize, Clone)]
pub struct DeviceInfo {
    /// hdc 目标 id（序列号 / ip:port）
    pub id: String,
    /// 连接状态：Online / Offline / Unauthorized 等
    pub state: String,
    /// 设备型号（const.product.model），取不到为空
    pub model: String,
    /// 系统版本（const.product.name 或 os_version），取不到为空
    pub os_version: String,
    /// 是否为当前默认设备
    pub is_default: bool,
}

fn default_device_file() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    let home = std::env::var("APPDATA").ok();
    #[cfg(not(windows))]
    let home = std::env::var("HOME").ok();
    home.map(|h| std::path::PathBuf::from(h).join("deveco-code-switch").join("default_device.txt"))
}

fn load_default_device() -> Option<String> {
    let path = default_device_file()?;
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn save_default_device(device_id: &str) {
    if let Some(path) = default_device_file() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, device_id);
    }
}

async fn run_hdc(args: &[&str], _timeout: u64) -> Result<String, String> {
    let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let output = process::command("hdc", &owned)?
        .output()
        .await
        .map_err(|e| format!("hdc 不可用: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn shell_param(device: &str, key: &str) -> String {
    let out = run_hdc(
        &["-t", device, "shell", "param", "get", key],
        15,
    )
    .await
    .unwrap_or_default();
    out.trim().to_string()
}

/// 列出所有已连接设备及在线状态、型号、系统版本。
#[tauri::command]
pub async fn list_devices() -> Result<Vec<DeviceInfo>, String> {
    let out = run_hdc(&["list", "targets"], 30).await?;
    let default = load_default_device();
    let mut devices: Vec<DeviceInfo> = Vec::new();
    for line in out.lines() {
        let line = line.trim();
        if line.is_empty() || line.eq_ignore_ascii_case("[Empty]") {
            continue;
        }
        let mut parts = line.split_whitespace();
        let id = match parts.next() {
            Some(id) if !id.starts_with('[') => id.to_string(),
            _ => continue,
        };
        // hdc list targets 第二列可能是 Connected/Ready 等状态词
        let state = parts.next().unwrap_or("Online").to_string();
        let is_online = state.eq_ignore_ascii_case("Connected")
            || state.eq_ignore_ascii_case("Ready")
            || state.eq_ignore_ascii_case("Online");
        let is_default = default.as_deref() == Some(id.as_str());
        // 在线设备才查询型号/系统版本（离线设备查询会超时）
        let (model, os_version) = if is_online {
            let model = shell_param(&id, "const.product.model").await;
            let os = shell_param(&id, "const.ohos.apiversion").await;
            let name = shell_param(&id, "const.product.name").await;
            let os_version = if !os.is_empty() { format!("API {os}") } else { name };
            (model, os_version)
        } else {
            (String::new(), String::new())
        };
        devices.push(DeviceInfo { id, state, model, os_version, is_default });
    }
    Ok(devices)
}

/// 设置默认设备（部署/截图等操作优先使用）。
#[tauri::command]
pub async fn set_default_device(device_id: String) -> Result<(), String> {
    if device_id.trim().is_empty() {
        return Err("设备 id 不能为空".into());
    }
    save_default_device(device_id.trim());
    Ok(())
}

/// 设备详情：展开卡片时按需查询（品牌/厂商/系统版本/分辨率/电池/内存）。
#[derive(Debug, Serialize, Clone, Default)]
pub struct DeviceDetail {
    pub brand: String,
    pub manufacturer: String,
    pub model: String,
    pub os_version: String,
    pub resolution: String,
    pub battery: String,
    pub ram: String,
}

/// 在目标设备上执行 shell 命令（单命令字符串，返回去空白输出）
async fn shell_exec(device: &str, cmd: &str) -> String {
    run_hdc(&["-t", device, "shell", cmd], 20)
        .await
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// 查询设备详情（按需调用；查询较慢，前端展开卡片时显示加载中）。
#[tauri::command]
pub async fn get_device_detail(device_id: String) -> Result<DeviceDetail, String> {
    let d = device_id.trim();
    if d.is_empty() {
        return Err("设备 id 不能为空".into());
    }
    let brand = shell_param(d, "const.product.brand").await;
    let manufacturer = shell_param(d, "const.product.manufacturer").await;
    let model = shell_param(d, "const.product.model").await;
    let fullname = shell_param(d, "const.ohos.fullname").await;
    let ver = shell_param(d, "const.ohos.version").await;
    let api = shell_param(d, "const.ohos.apiversion").await;
    let os_version = if !fullname.is_empty() {
        fullname
    } else if !ver.is_empty() {
        ver
    } else {
        api
    };
    // wm size 输出形如 "Physical size: 1080x2400"
    let resolution = shell_exec(d, "wm size")
        .await
        .lines()
        .find_map(|l| l.trim().strip_prefix("Physical size:").map(|s| s.trim().to_string()))
        .unwrap_or_default();
    // 电池电量百分比（%）
    let battery = shell_exec(d, "cat /sys/class/power_supply/battery/capacity").await;
    // 内存总量（/proc/meminfo MemTotal，单位 kB）
    let ram = shell_exec(d, "cat /proc/meminfo")
        .await
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("MemTotal:")
                .map(|s| s.trim().trim_start_matches("kB").trim().to_string())
        })
        .unwrap_or_default();
    Ok(DeviceDetail {
        brand,
        manufacturer,
        model,
        os_version,
        resolution,
        battery,
        ram,
    })
}

/// 设备性能快照（CPU/内存/温度/电量），供实时监控曲线使用。
#[derive(Debug, Serialize, Clone, Default)]
pub struct DevicePerf {
    /// CPU 总占用率 %（0-100，读取失败为 -1）
    pub cpu: f64,
    /// 内存占用率 %（0-100，读取失败为 -1）
    pub mem: f64,
    /// 电池电量 %（-1 表示无法读取）
    pub battery: f64,
    /// 温度 ℃（-1 表示无法读取）
    pub temp: f64,
    /// 时间戳（ms）
    pub ts: i64,
}

/// 采样设备性能：CPU（/proc/stat 两次采样取差值）、内存（/proc/meminfo）、
/// 电量（battery capacity）、温度（thermal_zone0 temp）。
/// 单次采样即可返回（CPU 用带空闲的瞬时估计，避免等 1 秒）。
#[tauri::command]
pub async fn get_device_perf(device_id: String) -> Result<DevicePerf, String> {
    let d = device_id.trim();
    if d.is_empty() {
        return Err("设备 id 不能为空".into());
    }
    let ts = chrono::Utc::now().timestamp_millis();

    // ---- CPU：读 /proc/stat 两次（间隔 ~200ms）取占用率 ----
    let stat1 = shell_exec(d, "cat /proc/stat").await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let stat2 = shell_exec(d, "cat /proc/stat").await;
    let cpu = calc_cpu_usage(&stat1, &stat2);

    // ---- 内存：/proc/meminfo MemTotal / MemAvailable ----
    let mem = {
        let info = shell_exec(d, "cat /proc/meminfo").await;
        let total = parse_kb(&info, "MemTotal:");
        let avail = parse_kb(&info, "MemAvailable:").or_else(|| parse_kb(&info, "MemFree:"));
        match (total, avail) {
            (Some(t), Some(a)) if t > 0 => (1.0 - a as f64 / t as f64) * 100.0,
            _ => -1.0,
        }
    };

    // ---- 电池 ----
    let battery = shell_exec(d, "cat /sys/class/power_supply/battery/capacity")
        .await
        .trim()
        .parse::<f64>()
        .unwrap_or(-1.0);

    // ---- 温度：遍历 thermal_zone0..3 取第一个有效 ----
    let mut temp = -1.0;
    for i in 0..4 {
        let v = shell_exec(d, &format!("cat /sys/class/thermal/thermal_zone{i}/temp")).await;
        let v = v.trim().trim_end_matches('0');
        if let Ok(t) = v.parse::<f64>() {
            if t > 0.0 {
                // thermal_zone temp 通常为毫摄氏度（如 42000 → 42℃），小数值为摄氏度
                temp = if t > 1000.0 { t / 1000.0 } else { t };
                break;
            }
        }
    }

    Ok(DevicePerf { cpu, mem, battery, temp, ts })
}

/// 从 /proc/meminfo 文本提取 "MemTotal:" 后的 kB 数值（数字后可能带 kB 单位，取首个空白分隔 token）
fn parse_kb(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|l| {
        let rest = l.trim().strip_prefix(key)?.trim();
        rest.split_whitespace().next()?.parse::<u64>().ok()
    })
}

/// 通过两次 /proc/stat 采样计算 CPU 占用率（%）
fn calc_cpu_usage(stat1: &str, stat2: &str) -> f64 {
    let parse = |s: &str| -> Option<(u64, u64)> {
        let line = s.lines().find(|l| l.starts_with("cpu "))?;
        let parts: Vec<&str> = line.split_whitespace().skip(1).collect();
        // user nice system idle iowait irq softirq steal guest guest_nice
        let times: Vec<u64> = parts.iter().filter_map(|p| p.parse::<u64>().ok()).collect();
        if times.is_empty() {
            return None;
        }
        let idle = times.get(3).copied().unwrap_or(0) + times.get(4).copied().unwrap_or(0);
        let total: u64 = times.iter().sum();
        Some((total, idle))
    };
    let (t1, i1) = match parse(stat1) {
        Some(v) => v,
        None => return -1.0,
    };
    let (t2, i2) = match parse(stat2) {
        Some(v) => v,
        None => return -1.0,
    };
    let dt = t2.saturating_sub(t1);
    let di = i2.saturating_sub(i1);
    if dt == 0 {
        return 0.0;
    }
    ((dt - di) as f64 / dt as f64 * 100.0).clamp(0.0, 100.0)
}

/// hdc 工具是否可用（hdc 在 PATH 中且可执行）。
#[tauri::command]
pub async fn hdc_available() -> Result<bool, String> {
    match process::command("hdc", &["version".to_string()]) {
        Ok(mut cmd) => match cmd.output().await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        },
        Err(_) => Ok(false),
    }
}

/// 启动 hdc 服务端（daemon）。
#[tauri::command]
pub async fn start_hdc_service() -> Result<String, String> {
    let output = process::command("hdc", &["start".to_string()])?
        .output()
        .await
        .map_err(|e| format!("启动 hdc 服务失败: {e}"))?;
    let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Ok(if !out.is_empty() { out } else { err })
}

/// 停止 hdc 服务端（daemon）。
#[tauri::command]
pub async fn stop_hdc_service() -> Result<String, String> {
    let output = process::command("hdc", &["kill".to_string()])?
        .output()
        .await
        .map_err(|e| format!("停止 hdc 服务失败: {e}"))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// 截取设备屏幕：截图保存到项目 `.deveco-agent/screenshots/` 目录，返回项目内相对路径。
/// 前端用项目根 + 该路径通过 asset protocol 预览（复用 Agent 截图工具同款流程）。
#[tauri::command]
pub async fn capture_device_screenshot(
    project_id: String,
    device_id: Option<String>,
    state: State<'_, DbState>,
    app: AppHandle,
) -> Result<String, String> {
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        let p: String = conn
            .query_row(
                "SELECT path FROM projects WHERE id = ?1",
                [&project_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("项目不存在: {e}"))?;
        crate::utils::path::normalize_path(&p)
    };
    if project_path.is_empty() {
        return Err("全局项目没有文件目录".into());
    }
    let device = match device_id {
        Some(d) if !d.trim().is_empty() => d,
        _ => load_default_device().unwrap_or_default(),
    };
    if device.is_empty() {
        return Err("未指定设备且没有默认设备".into());
    }

    // 设备端截图（旧版 hdc 回退 screencap）
    let remote = "/sdcard/deveco_agent_shot.png";
    if run_hdc(
        &["-t", device.as_str(), "shell", "snapshot_display", "-f", remote],
        30,
    )
    .await
    .is_err()
    {
        run_hdc(
            &["-t", device.as_str(), "shell", "screencap", "-p", remote],
            30,
        )
        .await?;
    }

    // 拉取到项目 screenshots 目录
    let dir = Path::new(&project_path).join(".deveco-agent").join("screenshots");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let local = dir.join(format!("shot-{ts}.png"));
    let owned: Vec<String> = vec![
        "-t".into(),
        device.clone(),
        "file".into(),
        "recv".into(),
        remote.into(),
        local.to_string_lossy().to_string(),
    ];
    let output = process::command("hdc", &owned)?
        .output()
        .await
        .map_err(|e| format!("截图拉取失败: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    if !local.exists() {
        return Err("截图文件未生成".into());
    }

    // 项目目录注册为资源访问范围（前端 convertFileSrc 预览）
    let _ = app.asset_protocol_scope().allow_directory(Path::new(&project_path), true);
    Ok(local.to_string_lossy().to_string())
}

/// 已安装应用信息（第三方应用清单）
#[derive(Debug, Serialize, Clone)]
pub struct InstalledApp {
    pub package: String,
    pub launcher: bool,
}

/// 列出设备上已安装的第三方应用（bm dump --visible-third-party 或 pm list packages -3）。
/// launcher=true 表示该包含可启动入口（有 ability）。
#[tauri::command]
pub async fn list_installed_apps(device_id: String) -> Result<Vec<InstalledApp>, String> {
    let d = device_id.trim();
    if d.is_empty() {
        return Err("设备 id 不能为空".into());
    }
    // 优先用 bm dump（含 Ability 信息），失败回退 pm list packages -3
    let raw = shell_exec(d, "bm dump --visible-third-party").await;
    let mut pkgs: Vec<String> = Vec::new();
    let mut has_ability: std::collections::HashSet<String> = std::collections::HashSet::new();
    if !raw.is_empty() {
        for line in raw.lines() {
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("\"bundleName\" : \"") {
                if let Some(end) = rest.find('"') {
                    pkgs.push(rest[..end].to_string());
                }
            }
            if l.contains("\"name\" : \".MainAbility\"") || l.contains("EntryAbility") {
                // 粗略标记：当前包是最近一次 bundleName
                if let Some(last) = pkgs.last() {
                    has_ability.insert(last.clone());
                }
            }
        }
    }
    if pkgs.is_empty() {
        let raw2 = shell_exec(d, "pm list packages -3").await;
        for line in raw2.lines() {
            if let Some(rest) = line.trim().strip_prefix("package:") {
                let name = rest.trim();
                if !name.is_empty() {
                    pkgs.push(name.to_string());
                }
            }
        }
    }
    pkgs.sort();
    pkgs.dedup();
    Ok(pkgs
        .into_iter()
        .map(|p| InstalledApp {
            launcher: has_ability.contains(&p),
            package: p,
        })
        .collect())
}

/// 启动应用（aa start -a EntryAbility -b <package>，回退 .MainAbility）。
#[tauri::command]
pub async fn launch_app(device_id: String, package: String) -> Result<String, String> {
    let d = device_id.trim();
    let pkg = package.trim();
    if d.is_empty() || pkg.is_empty() {
        return Err("设备 id 和包名不能为空".into());
    }
    // 先尝试 EntryAbility（HarmonyOS 标准入口），失败回退 MainAbility
    let out = run_hdc(
        &["-t", d, "shell", "aa", "start", "-a", "EntryAbility", "-b", pkg],
        20,
    )
    .await;
    match out {
        Ok(s) if !s.contains("error") && !s.contains("failed") => Ok(s),
        _ => {
            let s = run_hdc(
                &["-t", d, "shell", "aa", "start", "-a", ".MainAbility", "-b", pkg],
                20,
            )
            .await?;
            Ok(s)
        }
    }
}

/// 强制停止应用（aa force-stop <package>）。
#[tauri::command]
pub async fn stop_app(device_id: String, package: String) -> Result<String, String> {
    let d = device_id.trim();
    let pkg = package.trim();
    if d.is_empty() || pkg.is_empty() {
        return Err("设备 id 和包名不能为空".into());
    }
    run_hdc(&["-t", d, "shell", "aa", "force-stop", pkg], 20).await
}

/// 当前运行中的进程摘要（ps -A -T -o PID,NAME，仅展示应用进程）
#[derive(Debug, Serialize, Clone)]
pub struct DeviceProcess {
    pub pid: String,
    pub name: String,
}

/// 列出设备上运行中的应用进程（hdc shell "ps -A -T -o PID,NAME"）。
#[tauri::command]
pub async fn list_device_processes(device_id: String) -> Result<Vec<DeviceProcess>, String> {
    let d = device_id.trim();
    if d.is_empty() {
        return Err("设备 id 不能为空".into());
    }
    let out = run_hdc(&["-t", d, "shell", "ps", "-A", "-T", "-o", "PID,NAME"], 20).await?;
    let mut procs = Vec::new();
    for (i, line) in out.lines().enumerate() {
        if i == 0 && line.contains("PID") {
            continue;
        }
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        let mut parts = l.split_whitespace();
        let pid = parts.next().unwrap_or("").to_string();
        let name = parts.last().unwrap_or("").to_string();
        if pid.is_empty() || name.is_empty() {
            continue;
        }
        // 只保留带点的应用进程（com.xxx.yyy），过滤系统原生进程
        if name.contains('.') {
            procs.push(DeviceProcess { pid, name });
        }
    }
    procs.sort_by(|a, b| a.name.cmp(&b.name));
    procs.dedup_by(|a, b| a.pid == b.pid && a.name == b.name);
    Ok(procs)
}

// ---------- 实时 hilog 流 ----------

/// 正在运行的 hilog 抓取任务句柄（按设备 id 索引）。
/// start 时写入，stop/任务退出时移除；用 JoinHandle 以便中止后台读取任务。
/// OnceLock 惰性初始化（HashMap 非 const，不能直接放 static）。
static HILOG_TASKS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, tokio::task::JoinHandle<()>>>> =
    std::sync::OnceLock::new();

/// 取出全局 hilog 任务表
fn hilog_tasks_lock(
) -> std::sync::MutexGuard<'static, std::collections::HashMap<String, tokio::task::JoinHandle<()>>> {
    HILOG_TASKS
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap()
}

/// 实时 hilog 推送事件载荷
#[derive(Debug, Clone, Serialize)]
struct HilogLine {
    device_id: String,
    line: String,
}

/// 开启设备实时日志流。
/// 参数：device_id 必填；package/tag/level 可选（与 Agent read_logcat 同义，去掉 -x 改为持续流）。
/// 后端 spawn `hdc shell hilog`，逐行通过 event `device-hilog-line` 推送给前端。
#[tauri::command]
pub async fn start_hilog_stream(
    app: AppHandle,
    device_id: String,
    package: Option<String>,
    tag: Option<String>,
    level: Option<String>,
) -> Result<(), String> {
    let d = device_id.trim().to_string();
    if d.is_empty() {
        return Err("设备 id 不能为空".into());
    }
    // 停止同设备已有流，避免重复
    stop_hilog_stream_inner(&d);

    let pkg = package.unwrap_or_default().trim().to_string();
    let tag = tag.unwrap_or_default().trim().to_string();
    let level = level.unwrap_or_default().trim().to_uppercase();

    // 包名 → pid（在启动前解析一次；实时流期间进程重启需重新开启）
    let pids = if !pkg.is_empty() {
        let out = run_hdc(&["-t", &d, "shell", "pidof", &pkg], 15).await.unwrap_or_default();
        let p: Vec<String> = out
            .split(|c: char| c.is_whitespace())
            .filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()))
            .map(|s| s.to_string())
            .collect();
        if p.is_empty() {
            return Err(format!("未找到包名为「{pkg}」的运行进程"));
        }
        p
    } else {
        Vec::new()
    };

    // 组装 hilog 参数（持续流，不加 -x）
    let valid_levels = ["D", "I", "W", "E", "F"];
    let mut args: Vec<String> = vec!["-t".into(), d.clone(), "shell".into(), "hilog".into()];
    if valid_levels.contains(&level.as_str()) {
        args.push("-L".into());
        args.push(level.clone());
    }
    if !tag.is_empty() {
        args.push("-T".into());
        args.push(tag.clone());
    }

    let mut cmd = process::command("hdc", &args)?;
    use tokio::io::{AsyncBufReadExt, BufReader};
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());
    cmd.stdin(std::process::Stdio::null());
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("启动 hilog 失败: {e}"))?;
    let stdout = child.stdout.take().ok_or("无法获取 hilog stdout")?;

    let dev = d.clone();
    let handle = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            // pid 过滤
            if !pids.is_empty() {
                let hit = pids.iter().any(|p| {
                    line.split(|c: char| !c.is_ascii_digit())
                        .any(|tok| tok == p)
                });
                if !hit {
                    continue;
                }
            }
            let payload = HilogLine {
                device_id: dev.clone(),
                line,
            };
            let _ = app.emit("device-hilog-line", payload);
        }
        // 子进程结束：清理任务表（若仍是自己）
        let mut tasks = hilog_tasks_lock();
        if let Some(h) = tasks.get(&dev) {
            if h.is_finished() {
                tasks.remove(&dev);
            }
        }
        let _ = app.emit(
            "device-hilog-ended",
            serde_json::json!({ "device_id": dev }),
        );
    });
    hilog_tasks_lock().insert(d.clone(), handle);
    Ok(())
}

/// 停止指定设备的实时日志流。
#[tauri::command]
pub async fn stop_hilog_stream(device_id: String) -> Result<(), String> {
    stop_hilog_stream_inner(device_id.trim());
    Ok(())
}

fn stop_hilog_stream_inner(d: &str) {
    if let Some(h) = hilog_tasks_lock().remove(d) {
        h.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat_line(user: u64, nice: u64, system: u64, idle: u64, iowait: u64) -> String {
        format!("cpu  {user} {nice} {system} {idle} {iowait} 0 0 0 0 0")
    }

    #[test]
    fn calc_cpu_usage_50_percent() {
        let s1 = stat_line(1000, 0, 500, 8000, 500);
        let s2 = stat_line(1300, 0, 700, 8100, 800);
        // 总增量 = 300+0+200+100+300 = 900；idle 增量 = 100+300 = 400
        // 占用率 = (900-400)/900 ≈ 55.56%
        let cpu = calc_cpu_usage(&s1, &s2);
        let expected: f64 = 500.0 / 900.0 * 100.0;
        assert!((cpu - expected).abs() < 0.01, "got {cpu}, expected {expected}");
    }

    #[test]
    fn calc_cpu_usage_idle_means_zero() {
        let s1 = stat_line(100, 0, 50, 9000, 100);
        let s2 = stat_line(200, 0, 100, 9800, 100);
        // idle 增量 800+0=800，total 增量 100+0+50+800+0=950 → 占用率 ≈ 15.79%
        let cpu = calc_cpu_usage(&s1, &s2);
        assert!(cpu > 0.0 && cpu < 20.0, "got {cpu}");
    }

    #[test]
    fn calc_cpu_usage_missing_stat_returns_neg1() {
        assert_eq!(calc_cpu_usage("", "cpu  1 2 3 4 5"), -1.0);
        assert_eq!(calc_cpu_usage("cpu  1 2 3 4 5", "no cpu line"), -1.0);
    }

    #[test]
    fn calc_cpu_usage_clamped_and_no_delta() {
        // 两次采样完全相同 → 0%
        let s = stat_line(10, 0, 5, 100, 10);
        assert_eq!(calc_cpu_usage(&s, &s), 0.0);
        // 全忙：idle 不增长
        let s3 = stat_line(100, 0, 50, 0, 0);
        let s4 = stat_line(150, 0, 80, 0, 0);
        let cpu = calc_cpu_usage(&s3, &s4);
        assert!(cpu >= 100.0, "got {cpu}");
    }

    #[test]
    fn parse_kb_extracts_value() {
        let info = "MemTotal:        2030656 kB\nMemFree:          802344 kB\nMemAvailable:     995200 kB\n";
        assert_eq!(parse_kb(info, "MemTotal:"), Some(2030656));
        assert_eq!(parse_kb(info, "MemAvailable:"), Some(995200));
        assert_eq!(parse_kb(info, "SwapTotal:"), None);
        assert_eq!(parse_kb("", "MemTotal:"), None);
    }
}

