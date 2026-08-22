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
    /// 归一化连接状态：online / offline / unauthorized / unknown
    pub connection: String,
    /// hdc shell 是否已授权可用
    pub authorized: bool,
    /// 系统 API Level
    pub api_level: Option<i64>,
    /// 主 ABI/架构（如 arm64-v8a）
    pub architecture: String,
    /// 物理屏幕分辨率（如 1080x2400）
    pub resolution: String,
    /// 已用设备证据确认可用的能力
    pub capabilities: Vec<String>,
    /// 快照观测时间（Unix 秒）
    pub observed_at: i64,
    /// 是否为当前默认设备
    pub is_default: bool,
}

fn default_device_file() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    let home = std::env::var("APPDATA").ok();
    #[cfg(not(windows))]
    let home = std::env::var("HOME").ok();
    home.map(|h| {
        std::path::PathBuf::from(h)
            .join("deveco-code-switch")
            .join("default_device.txt")
    })
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

async fn run_hdc(args: &[&str], timeout_secs: u64) -> Result<String, String> {
    let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs.max(1)),
        process::command("hdc", &owned)?.output(),
    )
    .await
    .map_err(|_| {
        format!(
            "hdc 命令超时（{} 秒）：{}",
            timeout_secs.max(1),
            owned.join(" ")
        )
    })?
    .map_err(|e| format!("hdc 不可用: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn shell_param(device: &str, key: &str) -> String {
    let out = run_hdc(&["-t", device, "shell", "param", "get", key], 15)
        .await
        .unwrap_or_default();
    out.trim().to_string()
}

fn parse_target_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.eq_ignore_ascii_case("[Empty]") {
        return None;
    }
    let mut parts = line.split_whitespace();
    let id = parts.next()?;
    if id.starts_with('[') {
        return None;
    }
    Some((id.to_string(), parts.next().unwrap_or("Online").to_string()))
}

fn normalize_connection(state: &str) -> (&'static str, bool) {
    match state.to_ascii_lowercase().as_str() {
        "connected" | "ready" | "online" => ("online", true),
        value if value.contains("unauthor") || value.contains("reject") => ("unauthorized", false),
        "offline" | "disconnected" => ("offline", false),
        _ => ("unknown", false),
    }
}

fn parse_resolution(output: &str) -> String {
    output
        .lines()
        .find_map(|line| {
            let value = line
                .trim()
                .strip_prefix("Physical size:")
                .unwrap_or_else(|| line.trim())
                .trim();
            let (width, height) = value.split_once('x')?;
            (width.trim().parse::<u32>().is_ok() && height.trim().parse::<u32>().is_ok())
                .then(|| format!("{}x{}", width.trim(), height.trim()))
        })
        .unwrap_or_default()
}

fn parse_api_level(value: &str) -> Option<i64> {
    value
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| !part.is_empty())?
        .parse()
        .ok()
}

fn observed_at() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// 列出所有已连接设备及在线状态、型号、系统版本。
#[tauri::command]
pub async fn list_devices() -> Result<Vec<DeviceInfo>, String> {
    let out = run_hdc(&["list", "targets"], 30).await?;
    let default = load_default_device();
    let mut devices: Vec<DeviceInfo> = Vec::new();
    for line in out.lines() {
        let Some((id, state)) = parse_target_line(line) else {
            continue;
        };
        let (connection, authorized) = normalize_connection(&state);
        let is_default = default.as_deref() == Some(id.as_str());
        let (model, os_version, api_level, architecture, resolution, capabilities) = if authorized {
            let (
                model,
                fullname,
                version,
                product_name,
                api,
                abi_list,
                abi,
                screen,
                snapshot,
                uitest,
                hidumper,
            ) = tokio::join!(
                shell_param(&id, "const.product.model"),
                shell_param(&id, "const.ohos.fullname"),
                shell_param(&id, "const.ohos.version"),
                shell_param(&id, "const.product.name"),
                shell_param(&id, "const.ohos.apiversion"),
                shell_param(&id, "const.product.cpu.abilist"),
                shell_param(&id, "const.product.cpu.abi"),
                shell_exec(&id, "wm size"),
                shell_exec(&id, "command -v snapshot_display"),
                shell_exec(&id, "command -v uitest"),
                shell_exec(&id, "command -v hidumper"),
            );
            let api_level = parse_api_level(&api);
            let base_version = [fullname, version, product_name]
                .into_iter()
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            let os_version = match (base_version.is_empty(), api_level) {
                (false, Some(level)) => format!("{base_version} · API {level}"),
                (false, None) => base_version,
                (true, Some(level)) => format!("API {level}"),
                (true, None) => String::new(),
            };
            let architecture = if abi_list.is_empty() { abi } else { abi_list };
            let resolution = parse_resolution(&screen);
            let mut capabilities = vec![
                "shell".into(),
                "install".into(),
                "ability".into(),
                "hilog".into(),
            ];
            if !snapshot.is_empty() || !resolution.is_empty() {
                capabilities.push("screenshot".into());
            }
            if !uitest.is_empty() {
                capabilities.push("ui_automation".into());
            }
            if !hidumper.is_empty() {
                capabilities.push("diagnostics".into());
                capabilities.push("performance".into());
            }
            capabilities.sort();
            capabilities.dedup();
            (
                model,
                os_version,
                api_level,
                architecture,
                resolution,
                capabilities,
            )
        } else {
            (
                String::new(),
                String::new(),
                None,
                String::new(),
                String::new(),
                Vec::new(),
            )
        };
        devices.push(DeviceInfo {
            id,
            state,
            model,
            os_version,
            connection: connection.into(),
            authorized,
            api_level,
            architecture,
            resolution,
            capabilities,
            observed_at: observed_at(),
            is_default,
        });
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

/// 设备详情：展开卡片时按需查询（品牌/厂商/系统版本/分辨率/电池/内存/存储/频率）。
#[derive(Debug, Serialize, Clone, Default)]
pub struct DeviceDetail {
    pub brand: String,
    pub manufacturer: String,
    pub model: String,
    pub os_version: String,
    pub resolution: String,
    pub battery: String,
    pub ram: String,
    /// 存储用量文本（如 "28.2GB / 220.3GB（13%）"），读不到为空
    pub storage: String,
    /// CPU 当前频率（如 "1.62GHz"），读不到为空
    pub cpu_freq: String,
    /// 电池状态文本（充电状态 + 电压 + 电流 + 电芯技术），读不到为空
    pub battery_status: String,
    /// 电池温度 ℃（如 "30.0℃"），读不到为空
    pub battery_temp: String,
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
        .find_map(|l| {
            l.trim()
                .strip_prefix("Physical size:")
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_default();
    // 电池：hidumper BatteryService（鸿蒙 shell 无权限读 /sys/class/power_supply，
    // 实测 cat capacity 报 No such file / Permission denied；hidumper 是授权通道）
    let batt_raw = shell_exec(d, "hidumper -s BatteryService -a -i").await;
    let (capacity, temp, charging, voltage, current, tech) = parse_battery_info(&batt_raw);
    let battery = if capacity >= 0.0 {
        format!("{capacity:.0}%")
    } else {
        String::new()
    };
    let battery_temp = if temp >= 0.0 {
        format!("{temp:.1}℃")
    } else {
        String::new()
    };
    let battery_status = {
        let mut parts: Vec<String> = Vec::new();
        match charging {
            1 => parts.push("充电中".into()),
            2 => parts.push("未充电".into()),
            3 => parts.push("已充满".into()),
            _ => {}
        }
        if voltage > 0 {
            parts.push(format!("{:.2}V", voltage as f64 / 1_000_000.0));
        }
        if current > 0 {
            parts.push(format!("{current}mA"));
        }
        if !tech.is_empty() {
            parts.push(tech);
        }
        parts.join(" · ")
    };
    // 存储：hidumper --storage（df -k 输出，取 /data 分区）
    let storage = parse_storage(&shell_exec(d, "hidumper --storage").await);
    // CPU 当前频率：hidumper --cpufreq（取首个有效核）
    let cpu_freq = parse_cpu_freq(&shell_exec(d, "hidumper --cpufreq").await);
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
        storage,
        cpu_freq,
        battery_status,
        battery_temp,
    })
}

/// 解析 hidumper BatteryService -i 输出（"key: value" 行）。
/// 返回 (电量%, 温度℃, 充电状态, 电压µV, 电流mA, 电芯技术)；缺项取默认。
/// temperature 单位 0.1℃（如 300 → 30.0℃）；chargingStatus 1=充电中 2=未充电 3=已充满。
fn parse_battery_info(out: &str) -> (f64, f64, i64, u64, i64, String) {
    let mut capacity = -1.0f64;
    let mut temp = -1.0f64;
    let mut charging = -1i64;
    let mut voltage = 0u64;
    let mut current = 0i64;
    let mut tech = String::new();
    for line in out.lines() {
        let l = line.trim();
        let Some((k, v)) = l.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "capacity" => capacity = v.parse().unwrap_or(-1.0),
            "temperature" => temp = v.parse::<f64>().map(|t| t / 10.0).unwrap_or(-1.0),
            "chargingStatus" => charging = v.parse().unwrap_or(-1),
            "voltage" => voltage = v.parse().unwrap_or(0),
            "nowCurrent" => current = v.parse().unwrap_or(0),
            "technology" => tech = v.to_string(),
            _ => {}
        }
    }
    (capacity, temp, charging, voltage, current, tech)
}

/// 从 hidumper --storage 输出（df -k 表格）解析 /data 分区用量文本。
/// 行格式：<设备> <1K-blocks> <Used> <Available> <Use%> <挂载点>
fn parse_storage(out: &str) -> String {
    for line in out.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 6 && (parts[5] == "/data" || parts[5].starts_with("/data/")) {
            let total_kb: u64 = parts[1].parse().unwrap_or(0);
            let used_kb: u64 = parts[2].parse().unwrap_or(0);
            if total_kb == 0 {
                continue;
            }
            return format!(
                "{:.1}GB / {:.1}GB（{:.0}%）",
                used_kb as f64 / 1048576.0,
                total_kb as f64 / 1048576.0,
                used_kb as f64 / total_kb as f64 * 100.0
            );
        }
    }
    String::new()
}

/// 从 hidumper --cpufreq 输出解析首个有效核的当前频率（"cmd is: …" 行跳过）。
fn parse_cpu_freq(out: &str) -> String {
    for line in out.lines() {
        let l = line.trim();
        if l.starts_with("cmd is:") || l.is_empty() {
            continue;
        }
        if let Ok(hz) = l.parse::<u64>() {
            if hz > 0 {
                return format!("{:.2}GHz", hz as f64 / 1_000_000.0);
            }
        }
    }
    String::new()
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

    // ---- 电池/温度：hidumper BatteryService（鸿蒙 shell 读 sysfs 权限不足）----
    let (battery, temp) = sample_battery_via_hidumper(d).await;

    Ok(DevicePerf {
        cpu,
        mem,
        battery,
        temp,
        ts,
    })
}

/// 通过 hidumper BatteryService -i 读取电量与温度。
/// 鸿蒙 shell 读 /sys/class/power_supply 与 /sys/class/thermal 均 Permission denied，
/// hidumper 是系统授权通道；temperature 单位 0.1℃（300 → 30.0℃）。
async fn sample_battery_via_hidumper(device: &str) -> (f64, f64) {
    let out = shell_exec(device, "hidumper -s BatteryService -a -i").await;
    let (capacity, temp, _, _, _, _) = parse_battery_info(&out);
    (capacity, temp)
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

/// hdc shell 命令输出是否失败：hdc 的 shell 子命令失败时 exit code 仍为 0，
/// 错误只体现在输出文本里（如 snapshot_display 的 error: 行、screencap 的 not found），
/// 不能只信 status，须按文本特征判断。
fn hdc_shell_failed(out: &str) -> bool {
    out.contains("error:")
        || out.contains("[Fail]")
        || out.contains("not found")
        || out.contains("No such file")
}

/// 截取设备屏幕：截图保存到项目 `.deveco-agent/screenshots/` 目录，返回本地绝对路径。
/// 文件名带设备号与项目名（多个项目/多设备并存时一眼可辨）。
/// 前端用项目根 + 该路径通过 asset protocol 预览（复用 Agent 截图工具同款流程）。
#[tauri::command]
pub async fn capture_device_screenshot(
    project_id: String,
    device_id: Option<String>,
    project_name: Option<String>,
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

    // 设备端截图：snapshot_display（鸿蒙标准，-t png 显式输出真 PNG）→ 失败回退 screencap（AOSP）。
    // 路径用 /data/local/tmp（部分鸿蒙设备没有 /sdcard，且 snapshot_display 按后缀推断格式）；
    // 失败判断用文本特征（hdc shell 失败时 exit 仍为 0），最终以拉取到文件为唯一标准。
    let remote = "/data/local/tmp/deveco_agent_shot.png";
    let shot = run_hdc(
        &[
            "-t",
            device.as_str(),
            "shell",
            "snapshot_display",
            "-t",
            "png",
            "-f",
            remote,
        ],
        30,
    )
    .await
    .unwrap_or_default();
    if hdc_shell_failed(&shot) {
        let shot2 = run_hdc(
            &["-t", device.as_str(), "shell", "screencap", "-p", remote],
            30,
        )
        .await
        .unwrap_or_default();
        if hdc_shell_failed(&shot2) {
            return Err(format!(
                "设备截图失败：{}",
                shot.lines().next().unwrap_or("未知错误")
            ));
        }
    }

    // 拉取到项目 screenshots 目录（文件名带设备号与项目名；项目名清洗非法字符）
    let dir = Path::new(&project_path)
        .join(".deveco-agent")
        .join("screenshots");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let safe_name: String = project_name
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if "<>:\"/\\|?*".contains(c) || c.is_whitespace() {
                '-'
            } else {
                c
            }
        })
        .take(24)
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    let file_name = if safe_name.is_empty() {
        format!("shot-{device}-{ts}.png")
    } else {
        format!("shot-{device}-{safe_name}-{ts}.png")
    };
    let local = dir.join(&file_name);
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
    if !local.exists()
        || std::fs::metadata(&local)
            .map(|m| m.len() == 0)
            .unwrap_or(true)
    {
        return Err("截图文件未生成（设备端截图可能失败）".into());
    }
    // 清理设备端临时文件，避免多次截图累积
    let _ = run_hdc(&["-t", device.as_str(), "shell", "rm", remote], 10).await;

    // 项目目录注册为资源访问范围（前端 convertFileSrc 预览）
    let _ = app
        .asset_protocol_scope()
        .allow_directory(Path::new(&project_path), true);
    Ok(local.to_string_lossy().to_string())
}

/// 截图文件条目（列表用）
#[derive(Debug, Serialize, Clone)]
pub struct ShotFile {
    /// 文件名（如 shot-6UNB...-MyApp-20260814-193000.png）
    pub name: String,
    /// 本地绝对路径（前端 convertFileSrc 预览）
    pub path: String,
    /// 文件大小（字节）
    pub size: u64,
    /// 修改时间（unix 秒）
    pub mtime: i64,
}

/// 项目截图目录（.deveco-agent/screenshots）；项目不存在时报错。
fn screenshots_dir(
    project_id: &str,
    state: &State<'_, DbState>,
) -> Result<std::path::PathBuf, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let p: String = conn
        .query_row(
            "SELECT path FROM projects WHERE id = ?1",
            [project_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("项目不存在: {e}"))?;
    Ok(Path::new(&crate::utils::path::normalize_path(&p))
        .join(".deveco-agent")
        .join("screenshots"))
}

/// 列出项目截图目录的图片（png/jpg/jpeg，时间倒序，最多 50 张）。
#[tauri::command]
pub async fn list_device_screenshots(
    project_id: String,
    state: State<'_, DbState>,
) -> Result<Vec<ShotFile>, String> {
    let dir = screenshots_dir(&project_id, &state)?;
    let mut items: Vec<ShotFile> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let lower = name.to_lowercase();
            if !(lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg")) {
                continue;
            }
            let md = match e.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if !md.is_file() {
                continue;
            }
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            items.push(ShotFile {
                name,
                path: e.path().to_string_lossy().to_string(),
                size: md.len(),
                mtime,
            });
        }
    }
    items.sort_by_key(|a| std::cmp::Reverse(a.mtime));
    items.truncate(50);
    Ok(items)
}

/// 删除一张截图（仅限截图目录内、无路径分隔符的文件名，防目录穿越）。
#[tauri::command]
pub async fn delete_device_screenshot(
    project_id: String,
    name: String,
    state: State<'_, DbState>,
) -> Result<(), String> {
    let dir = screenshots_dir(&project_id, &state)?;
    let safe = name.replace('\\', "/");
    if safe.contains('/') || safe.contains("..") {
        return Err("非法的截图文件名".into());
    }
    let target = dir.join(&name);
    if !target.is_file() {
        return Err("截图不存在".into());
    }
    std::fs::remove_file(&target).map_err(|e| format!("删除截图失败: {e}"))
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
        &[
            "-t",
            d,
            "shell",
            "aa",
            "start",
            "-a",
            "EntryAbility",
            "-b",
            pkg,
        ],
        20,
    )
    .await;
    match out {
        Ok(s) if !s.contains("error") && !s.contains("failed") => Ok(s),
        _ => {
            let s = run_hdc(
                &[
                    "-t",
                    d,
                    "shell",
                    "aa",
                    "start",
                    "-a",
                    ".MainAbility",
                    "-b",
                    pkg,
                ],
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
static HILOG_TASKS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, tokio::task::JoinHandle<()>>>,
> = std::sync::OnceLock::new();

/// 取出全局 hilog 任务表
fn hilog_tasks_lock(
) -> std::sync::MutexGuard<'static, std::collections::HashMap<String, tokio::task::JoinHandle<()>>>
{
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
        let out = run_hdc(&["-t", &d, "shell", "pidof", &pkg], 15)
            .await
            .unwrap_or_default();
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
    fn target_state_is_normalized_without_losing_raw_state() {
        assert_eq!(
            parse_target_line("ABC123 Connected"),
            Some(("ABC123".into(), "Connected".into()))
        );
        assert_eq!(normalize_connection("Ready"), ("online", true));
        assert_eq!(
            normalize_connection("Unauthorized"),
            ("unauthorized", false)
        );
        assert_eq!(normalize_connection("Offline"), ("offline", false));
        assert!(parse_target_line("[Empty]").is_none());
    }

    #[test]
    fn screen_and_api_evidence_are_parsed_conservatively() {
        assert_eq!(
            parse_resolution("Physical size: 1080x2400\nOverride size: 720x1600"),
            "1080x2400"
        );
        assert_eq!(parse_resolution("permission denied"), "");
        assert_eq!(parse_api_level("14"), Some(14));
        assert_eq!(parse_api_level("OpenHarmony API 18"), Some(18));
        assert_eq!(parse_api_level("unknown"), None);
    }

    #[test]
    fn calc_cpu_usage_50_percent() {
        let s1 = stat_line(1000, 0, 500, 8000, 500);
        let s2 = stat_line(1300, 0, 700, 8100, 800);
        // 总增量 = 300+0+200+100+300 = 900；idle 增量 = 100+300 = 400
        // 占用率 = (900-400)/900 ≈ 55.56%
        let cpu = calc_cpu_usage(&s1, &s2);
        let expected: f64 = 500.0 / 900.0 * 100.0;
        assert!(
            (cpu - expected).abs() < 0.01,
            "got {cpu}, expected {expected}"
        );
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
