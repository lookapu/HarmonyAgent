//! Host Capability Broker 原型（docs/AGENT_EVOLUTION_ROADMAP_2026.md §4 / §5.2）。
//!
//! 把宿主特权操作（hdc 设备管理、签名、部署）建模为**类型化、窄化的能力**，而不是暴露
//! 等价的任意 shell。每个能力经 [`HostCapability::validate`] 拒绝越界/越权参数，并由
//! [`execute_host_capability`] 生成固定 argv、执行及写入运行审计。

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Output;

/// Broker 内建的 faultlog 查询范围，避免调用方把任意设备目录拼入 `hdc shell ls`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultLogDirectory {
    FaultLogger,
    Temp,
    Root,
}

/// 设备截图后端。枚举值由 Broker 映射为固定 argv，调用方不能注入命令片段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceScreenshotBackend {
    SnapshotDisplay,
    Screencap,
}

/// Broker 支持的有限 UI 输入动作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceUiAction {
    Click { x: i64, y: i64 },
    Swipe { x1: i64, y1: i64, x2: i64, y2: i64, speed: i64 },
    LongClick { x: i64, y: i64 },
    Text { text: String },
    Key { name: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppStorageTarget {
    Cache,
    Data,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionCommandBackend {
    NamedFlags,
    Positional,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceRadio {
    Wifi,
    AirplaneMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceRadioBackend {
    WifiCommand,
    WpaCli,
    Svc,
    PowerCommand,
    GlobalSettings,
}

/// Broker 支持的有限 debuggerd 控制动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceDebuggerAction {
    Step,
    Next,
    Continue,
    Interrupt,
    Backtrace,
    Registers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmulatorQueryKind {
    Instances,
    DownloadedImages,
    ScreenProfiles,
}

impl DeviceDebuggerAction {
    fn as_command(self) -> &'static str {
        match self {
            Self::Step => "s",
            Self::Next => "n",
            Self::Continue => "c",
            Self::Interrupt => "i",
            Self::Backtrace => "bt",
            Self::Registers => "r",
        }
    }
}

impl DeviceScreenshotBackend {
    fn as_str(self) -> &'static str {
        match self {
            Self::SnapshotDisplay => "snapshot_display",
            Self::Screencap => "screencap",
        }
    }
}

impl FaultLogDirectory {
    pub fn as_path(self) -> &'static str {
        match self {
            Self::FaultLogger => "/data/log/faultlog/faultlogger",
            Self::Temp => "/data/log/faultlog/temp",
            Self::Root => "/data/log/faultlog",
        }
    }
}

/// 宿主特权能力的窄化集合。v0 覆盖 hdc 与 deploy；签名与真机操作待接线。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostCapability {
    /// 无线连接设备（target 为 IP:port 或设备序列号）。
    HdcConnect { target: String },
    /// 断开设备连接。
    HdcDisconnect { target: String },
    /// 列出在线设备。
    HdcListTargets,
    /// 启动 hdc daemon。
    HdcStartServer,
    /// 停止 hdc daemon。
    HdcKillServer,
    /// 查询指定 bundle 的进程 id。
    DevicePidof { device: String, bundle: String },
    /// 把 debuggerd 附加到明确的设备进程。
    AttachDeviceDebugger { device: String, pid: u32, wait_seconds: u64 },
    /// 为明确 bundle 启用 Ability 调试模式，作为 debuggerd attach 的兼容回退。
    EnableAbilityDebug { device: String, bundle: String },
    /// 向已附加的 debuggerd 会话发送一个枚举化控制动作。
    ControlDeviceDebugger {
        device: String,
        pid: u32,
        action: DeviceDebuggerAction,
    },
    /// 查询已创建实例、已下载镜像或支持的屏幕配置。
    QueryEmulator { kind: EmulatorQueryKind },
    /// 派发一个已存在模拟器实例的 GUI 进程。
    StartEmulator { name: String },
    /// 停止一个明确的模拟器实例。
    StopEmulator { name: String },
    /// 创建一个有界配置的模拟器实例。
    CreateEmulator {
        name: String,
        device_type: String,
        os_version: String,
        screen_profile: Option<String>,
        memory_gb: Option<u64>,
        storage_gb: Option<u64>,
    },
    /// 删除一个明确的模拟器实例。
    DeleteEmulator { name: String },
    /// 使用受信任的 DevEco packagingtool 把工作区 HAP 打包为工作区内 OTA 包。
    PackageOta {
        hap_path: String,
        output_path: String,
        profile_path: Option<String>,
    },
    /// 查询用于签名 profile 匹配的设备 UDID。
    ReadDeviceUdid { device: String },
    /// 读取设备历史 hilog，可选最低级别和 tag。
    ReadHilog { device: String, level: Option<String>, tag: Option<String> },
    /// 带有界尾部窗口、epoch 格式与可选表达式的 hilog 搜索。
    SearchHilog {
        device: String,
        level: String,
        tag: Option<String>,
        tail_lines: u64,
        expression: Option<String>,
    },
    /// 兼容旧设备的有限行 logcat 查询。
    ReadLogcat { device: String, lines: u64 },
    /// 枚举三个预定义 faultlog 目录之一。
    ListFaultLogs { device: String, directory: FaultLogDirectory },
    /// 读取预定义 faultlog 目录中的单个安全 basename 文件。
    ReadFaultLog { device: String, directory: FaultLogDirectory, filename: String },
    /// 执行经过 Broker 二次校验的只读设备查询 argv。
    DeviceReadQuery { device: String, argv: Vec<String> },
    /// 查询指定网络接口的 qdisc 状态。
    ReadNetworkCondition { device: String, interface: String },
    /// 原子替换或清除指定网络接口的 netem 条件；三个数值全零表示清除。
    ConfigureNetworkCondition {
        device: String,
        interface: String,
        delay_ms: u64,
        loss_pct: u64,
        bandwidth_kbps: u64,
    },
    /// 把工作区内普通文件发送到设备绝对路径。
    SendFile { device: String, local_path: String, remote_path: String },
    /// 把设备绝对路径拉取到工作区内。
    ReceiveFile { device: String, remote_path: String, local_path: String },
    /// 截图到 Broker 管理的设备临时文件。
    CaptureDeviceScreenshot {
        device: String,
        remote_path: String,
        backend: DeviceScreenshotBackend,
    },
    /// 把当前 UI 层级导出到 Broker 管理的设备临时文件。
    DumpUiLayout { device: String, remote_path: String },
    /// 删除 Broker 管理的单个设备临时文件。
    RemoveDeviceTempFile { device: String, remote_path: String },
    /// 注入一个经过类型化与有界校验的 UI 动作。
    DeviceUiInput { device: String, operation_id: String, action: DeviceUiAction },
    /// 强制停止明确 bundle 的应用进程。
    StopAbility { device: String, bundle: String },
    /// 卸载明确 bundle；用于新装部署失败后的补偿。
    UninstallBundle { device: String, bundle: String, keep_data: bool },
    /// 清除明确 bundle 的缓存或应用数据。
    ClearAppStorage { device: String, bundle: String, target: AppStorageTarget },
    /// 使用已知 bm 语法授予或撤销单项应用权限。
    ChangeAppPermission {
        device: String,
        bundle: String,
        permission: String,
        grant: bool,
        backend: PermissionCommandBackend,
    },
    /// 通过有限兼容后端切换 Wi-Fi 或飞行模式。
    SetDeviceRadio {
        device: String,
        radio: DeviceRadio,
        enable: bool,
        backend: DeviceRadioBackend,
    },
    /// 开始把 UI 操作录制到受管设备临时 CSV。
    StartUiRecording { device: String, remote_path: String },
    /// 停止当前设备上的 UI 操作录制。
    StopUiRecording { device: String },
    /// 探测设备是否支持 screenrecord。
    ProbeScreenRecording { device: String },
    /// 运行有界时长的 screenrecord；必须通过长任务入口派发。
    StartScreenRecording { device: String, remote_path: String, max_seconds: u64 },
    /// 向当前设备的 screenrecord 发送 SIGINT，使文件完成 flush。
    StopScreenRecording { device: String },
    /// 安装构建产物到设备（路径必须位于项目工作树内）。
    InstallHap { device: Option<String>, hap_path: String, replace: bool },
    /// 拉起一个已安装应用的明确 ability。
    StartAbility { device: String, bundle: String, ability: String },
    /// 按 bundle、可选 ability 和可选 URI 拉起应用；至少需要 bundle 或 URI。
    StartAbilityIntent {
        device: String,
        bundle: Option<String>,
        ability: Option<String>,
        uri: Option<String>,
    },
    /// 部署 = 安装 + 可选启动（组合窄能力）。
    Deploy { device: Option<String>, hap_path: String },
}

impl HostCapability {
    /// 稳定能力 id，用于审计、权限等级与后续按 id 路由执行。
    pub fn capability_id(&self) -> &'static str {
        match self {
            Self::HdcConnect { .. } => "hdc.connect",
            Self::HdcDisconnect { .. } => "hdc.disconnect",
            Self::HdcListTargets => "hdc.list",
            Self::HdcStartServer => "hdc.start_server",
            Self::HdcKillServer => "hdc.kill_server",
            Self::DevicePidof { .. } => "device.pidof",
            Self::AttachDeviceDebugger { .. } => "device.debugger.attach",
            Self::EnableAbilityDebug { .. } => "device.debugger.enable_ability",
            Self::ControlDeviceDebugger { .. } => "device.debugger.control",
            Self::QueryEmulator { .. } => "emulator.query",
            Self::StartEmulator { .. } => "emulator.start",
            Self::StopEmulator { .. } => "emulator.stop",
            Self::CreateEmulator { .. } => "emulator.create",
            Self::DeleteEmulator { .. } => "emulator.delete",
            Self::PackageOta { .. } => "release.package_ota",
            Self::ReadDeviceUdid { .. } => "device.read_udid",
            Self::ReadHilog { .. } => "device.read_hilog",
            Self::SearchHilog { .. } => "device.search_hilog",
            Self::ReadLogcat { .. } => "device.read_logcat",
            Self::ListFaultLogs { .. } => "device.list_faultlogs",
            Self::ReadFaultLog { .. } => "device.read_faultlog",
            Self::DeviceReadQuery { .. } => "device.read_query",
            Self::ReadNetworkCondition { .. } => "device.network_condition.read",
            Self::ConfigureNetworkCondition { .. } => "device.network_condition.configure",
            Self::SendFile { .. } => "device.file_send",
            Self::ReceiveFile { .. } => "device.file_receive",
            Self::CaptureDeviceScreenshot { .. } => "device.screenshot.capture",
            Self::DumpUiLayout { .. } => "device.ui_layout.dump",
            Self::RemoveDeviceTempFile { .. } => "device.temp_file.remove",
            Self::DeviceUiInput { .. } => "device.ui_input",
            Self::StopAbility { .. } => "device.stop_ability",
            Self::UninstallBundle { .. } => "deploy.uninstall_bundle",
            Self::ClearAppStorage { .. } => "device.app_storage.clear",
            Self::ChangeAppPermission { .. } => "device.permission.change",
            Self::SetDeviceRadio { .. } => "device.radio.set",
            Self::StartUiRecording { .. } => "device.ui_record.start",
            Self::StopUiRecording { .. } => "device.ui_record.stop",
            Self::ProbeScreenRecording { .. } => "device.screen_record.probe",
            Self::StartScreenRecording { .. } => "device.screen_record.start",
            Self::StopScreenRecording { .. } => "device.screen_record.stop",
            Self::InstallHap { .. } => "deploy.install",
            Self::StartAbility { .. } => "deploy.start_ability",
            Self::StartAbilityIntent { .. } => "device.start_ability_intent",
            Self::Deploy { .. } => "deploy",
        }
    }

    /// 校验能力参数；任何越界/越权都返回错误。这是 broker 的安全边界，不能放宽。
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::HdcConnect { target } | Self::HdcDisconnect { target } => {
                validate_device_target(target)
            }
            Self::HdcListTargets | Self::HdcStartServer | Self::HdcKillServer => Ok(()),
            Self::DevicePidof { device, bundle } => {
                validate_device_target(device)?;
                validate_app_identifier(bundle, "bundle")
            }
            Self::AttachDeviceDebugger { device, pid, wait_seconds } => {
                validate_device_target(device)?;
                validate_process_id(*pid)?;
                if !(1..=120).contains(wait_seconds) {
                    return Err("调试器等待时长必须在 1-120 秒之间".into());
                }
                Ok(())
            }
            Self::ControlDeviceDebugger { device, pid, .. } => {
                validate_device_target(device)?;
                validate_process_id(*pid)
            }
            Self::EnableAbilityDebug { device, bundle } => {
                validate_device_target(device)?;
                validate_app_identifier(bundle, "bundle")
            }
            Self::QueryEmulator { .. } => Ok(()),
            Self::StartEmulator { name }
            | Self::StopEmulator { name }
            | Self::DeleteEmulator { name } => validate_emulator_value(name, "模拟器实例名", 128),
            Self::CreateEmulator {
                name,
                device_type,
                os_version,
                screen_profile,
                memory_gb,
                storage_gb,
            } => {
                validate_emulator_value(name, "模拟器实例名", 128)?;
                validate_emulator_value(device_type, "模拟器设备类型", 64)?;
                validate_emulator_value(os_version, "模拟器系统版本", 128)?;
                if let Some(profile) = screen_profile {
                    validate_emulator_value(profile, "模拟器屏幕配置", 128)?;
                }
                if memory_gb.is_some_and(|value| !(2..=32).contains(&value)) {
                    return Err("模拟器内存必须在 2-32 GB 之间".into());
                }
                if storage_gb.is_some_and(|value| !(2..=1023).contains(&value)) {
                    return Err("模拟器存储必须在 2-1023 GB 之间".into());
                }
                Ok(())
            }
            Self::PackageOta { hap_path, output_path, profile_path } => {
                validate_hap_path(hap_path)?;
                validate_workspace_output_path(output_path, "pkg")?;
                if let Some(profile) = profile_path {
                    validate_workspace_profile_path(profile)?;
                }
                Ok(())
            }
            Self::ReadDeviceUdid { device } => validate_device_target(device),
            Self::ReadHilog { device, level, tag } => {
                validate_device_target(device)?;
                if let Some(level) = level {
                    if !matches!(level.as_str(), "D" | "I" | "W" | "E" | "F") {
                        return Err("hilog level 仅支持 D|I|W|E|F".into());
                    }
                }
                if let Some(tag) = tag {
                    validate_log_tag(tag)?;
                }
                Ok(())
            }
            Self::SearchHilog { device, level, tag, tail_lines, expression } => {
                validate_device_target(device)?;
                if !matches!(level.as_str(), "D" | "I" | "W" | "E" | "F") {
                    return Err("hilog level 仅支持 D|I|W|E|F".into());
                }
                if !(100..=5_000).contains(tail_lines) {
                    return Err("hilog tail_lines 必须在 100-5000 之间".into());
                }
                if let Some(tag) = tag {
                    validate_log_tag(tag)?;
                }
                if let Some(expression) = expression {
                    validate_log_expression(expression)?;
                }
                Ok(())
            }
            Self::ReadLogcat { device, lines } => {
                validate_device_target(device)?;
                if !(10..=1000).contains(lines) {
                    return Err("logcat lines 必须在 10-1000 之间".into());
                }
                Ok(())
            }
            Self::ListFaultLogs { device, .. } => validate_device_target(device),
            Self::ReadFaultLog { device, filename, .. } => {
                validate_device_target(device)?;
                validate_faultlog_filename(filename)
            }
            Self::DeviceReadQuery { device, argv } => {
                validate_device_target(device)?;
                validate_read_only_device_command(argv).map(|_| ())
            }
            Self::ReadNetworkCondition { device, interface } => {
                validate_device_target(device)?;
                validate_network_interface(interface)
            }
            Self::ConfigureNetworkCondition {
                device,
                interface,
                delay_ms,
                loss_pct,
                bandwidth_kbps,
            } => {
                validate_device_target(device)?;
                validate_network_interface(interface)?;
                if *delay_ms > 60_000 || *loss_pct > 100 || *bandwidth_kbps > 10_000_000 {
                    return Err("网络条件超出边界：delay<=60000ms、loss<=100%、bandwidth<=10000000kbps".into());
                }
                Ok(())
            }
            Self::SendFile { device, local_path, remote_path }
            | Self::ReceiveFile { device, remote_path, local_path } => {
                validate_device_target(device)?;
                validate_workspace_relative_path(local_path)?;
                validate_device_path(remote_path)
            }
            Self::CaptureDeviceScreenshot { device, remote_path, .. }
            | Self::DumpUiLayout { device, remote_path }
            | Self::RemoveDeviceTempFile { device, remote_path } => {
                validate_device_target(device)?;
                validate_managed_device_temp_path(remote_path)
            }
            Self::DeviceUiInput { device, operation_id, action } => {
                validate_device_target(device)?;
                validate_operation_id(operation_id)?;
                validate_device_ui_action(action)
            }
            Self::StopAbility { device, bundle }
            | Self::UninstallBundle { device, bundle, .. }
            | Self::ClearAppStorage { device, bundle, .. } => {
                validate_device_target(device)?;
                validate_app_identifier(bundle, "bundle")
            }
            Self::ChangeAppPermission { device, bundle, permission, .. } => {
                validate_device_target(device)?;
                validate_app_identifier(bundle, "bundle")?;
                validate_app_identifier(permission, "permission")
            }
            Self::SetDeviceRadio { device, radio, backend, .. } => {
                validate_device_target(device)?;
                if !matches!(
                    (radio, backend),
                    (DeviceRadio::Wifi, DeviceRadioBackend::WifiCommand)
                        | (DeviceRadio::Wifi, DeviceRadioBackend::WpaCli)
                        | (DeviceRadio::Wifi, DeviceRadioBackend::Svc)
                        | (DeviceRadio::AirplaneMode, DeviceRadioBackend::PowerCommand)
                        | (DeviceRadio::AirplaneMode, DeviceRadioBackend::GlobalSettings)
                ) {
                    return Err("设备无线状态与执行后端不兼容".into());
                }
                Ok(())
            }
            Self::StartUiRecording { device, remote_path } => {
                validate_device_target(device)?;
                validate_managed_device_temp_path(remote_path)
            }
            Self::StopUiRecording { device } => validate_device_target(device),
            Self::ProbeScreenRecording { device } | Self::StopScreenRecording { device } => {
                validate_device_target(device)
            }
            Self::StartScreenRecording { device, remote_path, max_seconds } => {
                validate_device_target(device)?;
                validate_managed_device_temp_path(remote_path)?;
                if !(1..=600).contains(max_seconds) {
                    return Err("录屏时长必须在 1-600 秒之间".into());
                }
                Ok(())
            }
            Self::InstallHap { device, hap_path, .. } | Self::Deploy { device, hap_path } => {
                if let Some(device) = device {
                    validate_device_target(device)?;
                }
                validate_hap_path(hap_path)
            }
            Self::StartAbility { device, bundle, ability } => {
                validate_device_target(device)?;
                validate_app_identifier(bundle, "bundle")?;
                validate_app_identifier(ability, "ability")
            }
            Self::StartAbilityIntent { device, bundle, ability, uri } => {
                validate_device_target(device)?;
                if bundle.is_none() && uri.is_none() {
                    return Err("启动意图至少需要 bundle 或 URI".into());
                }
                if let Some(bundle) = bundle {
                    validate_app_identifier(bundle, "bundle")?;
                }
                if let Some(ability) = ability {
                    if bundle.is_none() {
                        return Err("指定 ability 时必须同时指定 bundle".into());
                    }
                    validate_app_identifier(ability, "ability")?;
                }
                if let Some(uri) = uri {
                    validate_ability_uri(uri)?;
                }
                Ok(())
            }
        }
    }

    fn replay_safe(&self) -> bool {
        matches!(
            self,
            Self::HdcListTargets
                | Self::DevicePidof { .. }
                | Self::QueryEmulator { .. }
                | Self::ReadDeviceUdid { .. }
                | Self::ReadHilog { .. }
                | Self::SearchHilog { .. }
                | Self::ReadLogcat { .. }
                | Self::ListFaultLogs { .. }
                | Self::ReadFaultLog { .. }
                | Self::DeviceReadQuery { .. }
                | Self::ReadNetworkCondition { .. }
                | Self::ProbeScreenRecording { .. }
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HostInvocation {
    program: String,
    args: Vec<String>,
    timeout_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HostRequestIdentity {
    tool_call_id: String,
    request_digest: String,
    idempotency_key: String,
}

fn request_identity(
    ctx: &crate::agent::exec_ctx::ToolCtx,
    capability: &HostCapability,
) -> Result<HostRequestIdentity, String> {
    let run_id = ctx.run_id.trim();
    let tool_call_id = ctx.tool_call_id.as_deref().map(str::trim).filter(|id| !id.is_empty())
        .ok_or("Host Capability Broker 缺少 tool_call_id，拒绝执行")?;
    if run_id.is_empty() {
        return Err("Host Capability Broker 缺少 run_id，拒绝执行".into());
    }
    let capability_id = capability.capability_id();
    let material = request_material(capability);
    let request_digest = format!(
        "{:x}",
        Sha256::digest(format!("hcb-request-v1\0{capability_id}\0{material}").as_bytes())
    );
    let digest = Sha256::digest(
        format!("hcb-v1\0{run_id}\0{tool_call_id}\0{capability_id}\0{material}").as_bytes(),
    );
    Ok(HostRequestIdentity {
        tool_call_id: tool_call_id.to_string(),
        request_digest,
        idempotency_key: format!("hcb-v1:{digest:x}"),
    })
}

fn request_material(capability: &HostCapability) -> String {
    match capability {
        HostCapability::HdcConnect { target } | HostCapability::HdcDisconnect { target } =>
            target.trim().to_string(),
        HostCapability::HdcListTargets
        | HostCapability::HdcStartServer
        | HostCapability::HdcKillServer => String::new(),
        HostCapability::DevicePidof { device, bundle } => {
            format!("{}\0{}", device.trim(), bundle.trim())
        }
        HostCapability::AttachDeviceDebugger { device, pid, wait_seconds } => {
            format!("{}\0{pid}\0{wait_seconds}", device.trim())
        }
        HostCapability::EnableAbilityDebug { device, bundle } => {
            format!("{}\0{}", device.trim(), bundle.trim())
        }
        HostCapability::ControlDeviceDebugger { device, pid, action } => {
            format!("{}\0{pid}\0{}", device.trim(), action.as_command())
        }
        HostCapability::QueryEmulator { kind } => match kind {
            EmulatorQueryKind::Instances => "instances",
            EmulatorQueryKind::DownloadedImages => "downloaded_images",
            EmulatorQueryKind::ScreenProfiles => "screen_profiles",
        }
        .to_string(),
        HostCapability::StartEmulator { name }
        | HostCapability::StopEmulator { name }
        | HostCapability::DeleteEmulator { name } => name.trim().to_string(),
        HostCapability::CreateEmulator {
            name,
            device_type,
            os_version,
            screen_profile,
            memory_gb,
            storage_gb,
        } => format!(
            "{}\0{}\0{}\0{}\0{}\0{}",
            name.trim(),
            device_type.trim(),
            os_version.trim(),
            screen_profile.as_deref().unwrap_or("").trim(),
            memory_gb.map(|value| value.to_string()).unwrap_or_default(),
            storage_gb.map(|value| value.to_string()).unwrap_or_default(),
        ),
        HostCapability::PackageOta { hap_path, output_path, profile_path } => format!(
            "{}\0{}\0{}",
            hap_path.trim(),
            output_path.trim(),
            profile_path.as_deref().unwrap_or("").trim(),
        ),
        HostCapability::ReadDeviceUdid { device } => device.trim().to_string(),
        HostCapability::ReadHilog { device, level, tag } => format!(
            "{}\0{}\0{}",
            device.trim(),
            level.as_deref().unwrap_or(""),
            tag.as_deref().unwrap_or("").trim(),
        ),
        HostCapability::SearchHilog { device, level, tag, tail_lines, expression } => format!(
            "{}\0{}\0{}\0{}\0{}",
            device.trim(), level, tag.as_deref().unwrap_or(""), tail_lines,
            expression.as_deref().unwrap_or(""),
        ),
        HostCapability::ReadLogcat { device, lines } => format!("{}\0{lines}", device.trim()),
        HostCapability::ListFaultLogs { device, directory } => {
            format!("{}\0{}", device.trim(), directory.as_path())
        }
        HostCapability::ReadFaultLog { device, directory, filename } => {
            format!("{}\0{}\0{}", device.trim(), directory.as_path(), filename.trim())
        }
        HostCapability::DeviceReadQuery { device, argv } => {
            format!("{}\0{}", device.trim(), argv.join("\0"))
        }
        HostCapability::ReadNetworkCondition { device, interface } => {
            format!("{}\0{}", device.trim(), interface.trim())
        }
        HostCapability::ConfigureNetworkCondition {
            device, interface, delay_ms, loss_pct, bandwidth_kbps,
        } => format!(
            "{}\0{}\0{delay_ms}\0{loss_pct}\0{bandwidth_kbps}",
            device.trim(), interface.trim(),
        ),
        HostCapability::SendFile { device, local_path, remote_path }
        | HostCapability::ReceiveFile { device, remote_path, local_path } => format!(
            "{}\0{}\0{}",
            device.trim(), local_path.trim(), remote_path.trim(),
        ),
        HostCapability::CaptureDeviceScreenshot { device, remote_path, backend } => format!(
            "{}\0{}\0{}", device.trim(), remote_path.trim(), backend.as_str(),
        ),
        HostCapability::DumpUiLayout { device, remote_path }
        | HostCapability::RemoveDeviceTempFile { device, remote_path } => {
            format!("{}\0{}", device.trim(), remote_path.trim())
        }
        HostCapability::DeviceUiInput { device, operation_id, action } => {
            format!(
                "{}\0{}\0{}",
                device.trim(),
                operation_id.trim(),
                device_ui_action_material(action),
            )
        }
        HostCapability::StopAbility { device, bundle } => {
            format!("{}\0{}", device.trim(), bundle.trim())
        }
        HostCapability::UninstallBundle { device, bundle, keep_data } => {
            format!("{}\0{}\0{keep_data}", device.trim(), bundle.trim())
        }
        HostCapability::ClearAppStorage { device, bundle, target } => format!(
            "{}\0{}\0{}",
            device.trim(),
            bundle.trim(),
            match target { AppStorageTarget::Cache => "cache", AppStorageTarget::Data => "data" },
        ),
        HostCapability::ChangeAppPermission {
            device, bundle, permission, grant, backend,
        } => format!(
            "{}\0{}\0{}\0{grant}\0{}",
            device.trim(),
            bundle.trim(),
            permission.trim(),
            match backend {
                PermissionCommandBackend::NamedFlags => "named_flags",
                PermissionCommandBackend::Positional => "positional",
            },
        ),
        HostCapability::SetDeviceRadio { device, radio, enable, backend } => format!(
            "{}\0{}\0{enable}\0{}",
            device.trim(),
            match radio { DeviceRadio::Wifi => "wifi", DeviceRadio::AirplaneMode => "airplane" },
            match backend {
                DeviceRadioBackend::WifiCommand => "wifi_command",
                DeviceRadioBackend::WpaCli => "wpa_cli",
                DeviceRadioBackend::Svc => "svc",
                DeviceRadioBackend::PowerCommand => "power_command",
                DeviceRadioBackend::GlobalSettings => "global_settings",
            },
        ),
        HostCapability::StartUiRecording { device, remote_path } => {
            format!("{}\0{}", device.trim(), remote_path.trim())
        }
        HostCapability::StopUiRecording { device } => device.trim().to_string(),
        HostCapability::ProbeScreenRecording { device }
        | HostCapability::StopScreenRecording { device } => device.trim().to_string(),
        HostCapability::StartScreenRecording { device, remote_path, max_seconds } => {
            format!("{}\0{}\0{max_seconds}", device.trim(), remote_path.trim())
        }
        HostCapability::InstallHap { device, hap_path, replace } => format!(
            "{}\0{}\0{replace}", device.as_deref().unwrap_or("").trim(), hap_path.trim(),
        ),
        HostCapability::StartAbility { device, bundle, ability } =>
            format!("{}\0{}\0{}", device.trim(), bundle.trim(), ability.trim()),
        HostCapability::StartAbilityIntent { device, bundle, ability, uri } => format!(
            "{}\0{}\0{}\0{}",
            device.trim(),
            bundle.as_deref().unwrap_or("").trim(),
            ability.as_deref().unwrap_or("").trim(),
            uri.as_deref().map(short_digest).unwrap_or_default(),
        ),
        HostCapability::Deploy { device, hap_path } => format!(
            "{}\0{}", device.as_deref().unwrap_or("").trim(), hap_path.trim(),
        ),
    }
}

fn prepare_invocation(capability: &HostCapability, workspace: Option<&Path>) -> Result<HostInvocation, String> {
    capability.validate()?;
    let (args, timeout_seconds) = match capability {
        HostCapability::HdcConnect { target } => (vec!["tconn".into(), target.trim().into()], 30),
        HostCapability::HdcDisconnect { target } => (
            vec!["tconn".into(), "-d".into(), target.trim().into()], 30,
        ),
        HostCapability::HdcListTargets => (vec!["list".into(), "targets".into()], 15),
        HostCapability::HdcStartServer => (vec!["start".into()], 30),
        HostCapability::HdcKillServer => (vec!["kill".into()], 30),
        HostCapability::DevicePidof { device, bundle } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "pidof".into(),
                bundle.trim().into(),
            ],
            15,
        ),
        HostCapability::AttachDeviceDebugger { device, pid, wait_seconds } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "debuggerd".into(),
                "-p".into(), pid.to_string(),
            ],
            *wait_seconds,
        ),
        HostCapability::EnableAbilityDebug { device, bundle } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "aa".into(),
                "debug".into(), "-b".into(), bundle.trim().into(),
            ],
            30,
        ),
        HostCapability::ControlDeviceDebugger { device, pid, action } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "debuggerd".into(),
                "-p".into(), pid.to_string(), "-c".into(), action.as_command().into(),
            ],
            30,
        ),
        HostCapability::QueryEmulator { .. }
        | HostCapability::StartEmulator { .. }
        | HostCapability::StopEmulator { .. }
        | HostCapability::CreateEmulator { .. }
        | HostCapability::DeleteEmulator { .. } => {
            return prepare_emulator_invocation(capability);
        }
        HostCapability::PackageOta { hap_path, output_path, profile_path } => {
            let workspace = workspace.ok_or("release.package_ota 需要明确的项目工作区")?;
            let artifact = resolve_workspace_artifact(workspace, hap_path)?;
            let destination = resolve_workspace_output(workspace, output_path, "pkg")?;
            let packager = packaging_tool_executable()
                .ok_or("未找到 DevEco packagingtool.jar，请安装 DevEco Studio 或配置 HOS_PACKAGING_TOOL")?;
            let mut args = vec![
                "-jar".into(),
                packager.to_string_lossy().into_owned(),
                "--mode".into(),
                "ota".into(),
                "--hap".into(),
                artifact.to_string_lossy().into_owned(),
                "--out".into(),
                destination.to_string_lossy().into_owned(),
            ];
            if let Some(profile_path) = profile_path {
                let profile = resolve_workspace_profile(workspace, profile_path)?;
                args.extend(["--profile".into(), profile.to_string_lossy().into_owned()]);
            }
            args.push("--force".into());
            return Ok(HostInvocation {
                program: "java".into(),
                args,
                timeout_seconds: 180,
            });
        }
        HostCapability::ReadDeviceUdid { device } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "bm".into(), "get".into(),
                "-u".into(),
            ],
            30,
        ),
        HostCapability::ReadHilog { device, level, tag } => {
            let mut args = vec![
                "-t".into(), device.trim().into(), "shell".into(), "hilog".into(), "-x".into(),
            ];
            if let Some(level) = level {
                args.extend(["-L".into(), level.clone()]);
            }
            if let Some(tag) = tag {
                args.extend(["-T".into(), tag.trim().into()]);
            }
            (args, 25)
        }
        HostCapability::SearchHilog { device, level, tag, tail_lines, expression } => {
            let mut args = vec![
                "-t".into(), device.trim().into(), "shell".into(), "hilog".into(), "-x".into(),
                "-z".into(), tail_lines.to_string(), "-v".into(), "epoch".into(), "-L".into(),
                level.clone(),
            ];
            if let Some(tag) = tag {
                args.extend(["-T".into(), tag.trim().into()]);
            }
            if let Some(expression) = expression {
                args.extend(["-e".into(), expression.clone()]);
            }
            (args, 20)
        }
        HostCapability::ReadLogcat { device, lines } => (
            vec![
                "-t".into(), device.trim().into(), "logcat".into(), "-T".into(), lines.to_string(),
            ],
            20,
        ),
        HostCapability::ListFaultLogs { device, directory } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "ls".into(), "-1".into(),
                directory.as_path().into(),
            ],
            15,
        ),
        HostCapability::ReadFaultLog { device, directory, filename } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "cat".into(),
                format!("{}/{}", directory.as_path(), filename.trim()),
            ],
            20,
        ),
        HostCapability::DeviceReadQuery { device, argv } => {
            let query = validate_read_only_device_command(argv)?;
            let mut args = vec!["-t".into(), device.trim().into(), "shell".into()];
            args.extend(query);
            (args, 30)
        }
        HostCapability::ReadNetworkCondition { device, interface } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "tc".into(),
                "qdisc".into(), "show".into(), "dev".into(), interface.trim().into(),
            ],
            10,
        ),
        HostCapability::ConfigureNetworkCondition {
            device, interface, delay_ms, loss_pct, bandwidth_kbps,
        } => {
            let mut args = vec![
                "-t".into(), device.trim().into(), "shell".into(), "tc".into(),
                "qdisc".into(),
            ];
            if *delay_ms == 0 && *loss_pct == 0 && *bandwidth_kbps == 0 {
                args.extend([
                    "del".into(), "dev".into(), interface.trim().into(), "root".into(),
                ]);
            } else {
                args.extend([
                    "replace".into(), "dev".into(), interface.trim().into(), "root".into(),
                    "netem".into(),
                ]);
                if *delay_ms > 0 {
                    args.extend(["delay".into(), format!("{delay_ms}ms")]);
                }
                if *loss_pct > 0 {
                    args.extend(["loss".into(), format!("{loss_pct}%")]);
                }
                if *bandwidth_kbps > 0 {
                    args.extend(["rate".into(), format!("{bandwidth_kbps}kbit")]);
                }
            }
            (args, 10)
        }
        HostCapability::SendFile { device, local_path, remote_path } => {
            let local = resolve_workspace_source(
                workspace.ok_or("device.file_send 需要明确的项目工作区")?, local_path,
            )?;
            (
                vec![
                    "-t".into(), device.trim().into(), "file".into(), "send".into(),
                    local.to_string_lossy().into_owned(), remote_path.trim().into(),
                ],
                120,
            )
        }
        HostCapability::ReceiveFile { device, remote_path, local_path } => {
            let local = resolve_workspace_destination(
                workspace.ok_or("device.file_receive 需要明确的项目工作区")?, local_path,
            )?;
            (
                vec![
                    "-t".into(), device.trim().into(), "file".into(), "recv".into(),
                    remote_path.trim().into(), local.to_string_lossy().into_owned(),
                ],
                120,
            )
        }
        HostCapability::CaptureDeviceScreenshot { device, remote_path, backend } => {
            let command = match backend {
                DeviceScreenshotBackend::SnapshotDisplay => vec![
                    "snapshot_display".into(), "-t".into(), "png".into(), "-f".into(),
                    remote_path.trim().into(),
                ],
                DeviceScreenshotBackend::Screencap => vec![
                    "screencap".into(), "-p".into(), remote_path.trim().into(),
                ],
            };
            let mut args = vec!["-t".into(), device.trim().into(), "shell".into()];
            args.extend(command);
            (args, 30)
        }
        HostCapability::DumpUiLayout { device, remote_path } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "uitest".into(),
                "dumpLayout".into(), "-p".into(), remote_path.trim().into(),
            ],
            30,
        ),
        HostCapability::RemoveDeviceTempFile { device, remote_path } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "rm".into(), "-f".into(),
                remote_path.trim().into(),
            ],
            10,
        ),
        HostCapability::DeviceUiInput { device, action, .. } => {
            let mut args = vec![
                "-t".into(), device.trim().into(), "shell".into(), "uitest".into(),
                "uiInput".into(),
            ];
            match action {
                DeviceUiAction::Click { x, y } => {
                    args.extend(["click".into(), x.to_string(), y.to_string()]);
                }
                DeviceUiAction::Swipe { x1, y1, x2, y2, speed } => {
                    args.extend([
                        "swipe".into(), x1.to_string(), y1.to_string(), x2.to_string(),
                        y2.to_string(), speed.to_string(),
                    ]);
                }
                DeviceUiAction::LongClick { x, y } => {
                    args.extend(["longClick".into(), x.to_string(), y.to_string()]);
                }
                DeviceUiAction::Text { text } => {
                    args.extend(["text".into(), text.clone()]);
                }
                DeviceUiAction::Key { name } => {
                    args.extend(["keyEvent".into(), name.clone()]);
                }
            }
            (args, 20)
        }
        HostCapability::StopAbility { device, bundle } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "aa".into(),
                "force-stop".into(), bundle.trim().into(),
            ],
            20,
        ),
        HostCapability::UninstallBundle { device, bundle, keep_data } => {
            let mut args = vec![
                "-t".into(), device.trim().into(), "shell".into(), "bm".into(),
                "uninstall".into(),
            ];
            if *keep_data {
                args.push("-k".into());
            }
            args.extend(["-n".into(), bundle.trim().into()]);
            (args, 30)
        }
        HostCapability::ClearAppStorage { device, bundle, target } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "bm".into(), "clean".into(),
                match target { AppStorageTarget::Cache => "-c", AppStorageTarget::Data => "-d" }.into(),
                "-n".into(), bundle.trim().into(),
            ],
            20,
        ),
        HostCapability::ChangeAppPermission {
            device, bundle, permission, grant, backend,
        } => {
            let verb = match (grant, backend) {
                (true, PermissionCommandBackend::NamedFlags) => "grant-permission",
                (false, PermissionCommandBackend::NamedFlags) => "revoke-permission",
                (true, PermissionCommandBackend::Positional) => "grant",
                (false, PermissionCommandBackend::Positional) => "revoke",
            };
            let mut args = vec![
                "-t".into(), device.trim().into(), "shell".into(), "bm".into(), verb.into(),
            ];
            match backend {
                PermissionCommandBackend::NamedFlags => args.extend([
                    "-n".into(), bundle.trim().into(), "-p".into(), permission.trim().into(),
                ]),
                PermissionCommandBackend::Positional => args.extend([
                    bundle.trim().into(), permission.trim().into(),
                ]),
            }
            (args, 20)
        }
        HostCapability::SetDeviceRadio { device, radio, enable, backend } => {
            let value = if *enable { "1" } else { "0" };
            let command: Vec<String> = match (radio, backend) {
                (DeviceRadio::Wifi, DeviceRadioBackend::WifiCommand) => vec![
                    "cmd".into(), "wifi".into(), "set_wifi_enable".into(), value.into(),
                ],
                (DeviceRadio::Wifi, DeviceRadioBackend::WpaCli) => vec![
                    "wpa_cli".into(), "-i".into(), "wlan0".into(),
                    if *enable { "ifup" } else { "ifdown" }.into(),
                ],
                (DeviceRadio::Wifi, DeviceRadioBackend::Svc) => vec![
                    "svc".into(), "wifi".into(), if *enable { "enable" } else { "disable" }.into(),
                ],
                (DeviceRadio::AirplaneMode, DeviceRadioBackend::PowerCommand) => vec![
                    "cmd".into(), "power".into(), "set-airplane-mode".into(), value.into(),
                ],
                (DeviceRadio::AirplaneMode, DeviceRadioBackend::GlobalSettings) => vec![
                    "settings".into(), "put".into(), "global".into(), "airplane_mode_on".into(),
                    value.into(),
                ],
                _ => return Err("设备无线状态与执行后端不兼容".into()),
            };
            let mut args = vec!["-t".into(), device.trim().into(), "shell".into()];
            args.extend(command);
            (args, 10)
        }
        HostCapability::StartUiRecording { device, remote_path } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "uitest".into(),
                "uiRecord".into(), "record".into(), "-p".into(), remote_path.trim().into(),
            ],
            10,
        ),
        HostCapability::StopUiRecording { device } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "uitest".into(),
                "uiRecord".into(), "stop".into(),
            ],
            10,
        ),
        HostCapability::ProbeScreenRecording { device } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "screenrecord".into(),
                "--help".into(),
            ],
            5,
        ),
        HostCapability::StartScreenRecording { device, remote_path, max_seconds } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "screenrecord".into(),
                "--time-limit".into(), max_seconds.to_string(), "--size".into(),
                "1080x1920".into(), remote_path.trim().into(),
            ],
            max_seconds.saturating_add(10),
        ),
        HostCapability::StopScreenRecording { device } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "pkill".into(), "-2".into(),
                "screenrecord".into(),
            ],
            5,
        ),
        HostCapability::InstallHap { device, hap_path, replace } => {
            let artifact = resolve_workspace_artifact(
                workspace.ok_or("deploy.install 需要明确的项目工作区")?, hap_path,
            )?;
            let mut args = Vec::new();
            if let Some(device) = device {
                args.extend(["-t".into(), device.trim().into()]);
            }
            args.push("install".into());
            if *replace { args.push("-r".into()); }
            args.push(artifact.to_string_lossy().into_owned());
            (args, 300)
        }
        HostCapability::StartAbility { device, bundle, ability } => (
            vec![
                "-t".into(), device.trim().into(), "shell".into(), "aa".into(), "start".into(),
                "-b".into(), bundle.trim().into(), "-a".into(), ability.trim().into(),
            ], 30,
        ),
        HostCapability::StartAbilityIntent { device, bundle, ability, uri } => {
            let mut args = vec![
                "-t".into(), device.trim().into(), "shell".into(), "aa".into(), "start".into(),
            ];
            if let Some(bundle) = bundle {
                args.extend(["-b".into(), bundle.trim().into()]);
            }
            if let Some(ability) = ability {
                args.extend(["-a".into(), ability.trim().into()]);
            }
            if let Some(uri) = uri {
                args.extend(["-D".into(), uri.clone()]);
            }
            (args, 30)
        }
        HostCapability::Deploy { .. } => {
            return Err("deploy 是组合能力，必须拆分为 install 与 start_ability 执行".into());
        }
    };
    Ok(HostInvocation { program: "hdc".into(), args, timeout_seconds })
}

/// 只从 DevEco 安装目录和受支持的 Windows 默认位置发现官方模拟器程序。
/// Agent 请求本身不携带可执行路径，避免把 Broker 退化为任意程序启动器。
pub fn emulator_executable() -> Option<PathBuf> {
    for dir in crate::commands::health::discover_deveco_dirs() {
        for rel in [
            "tools/emulator/Emulator.exe",
            "sdk/emulator/Emulator.exe",
            "emulator/Emulator.exe",
        ] {
            let path = dir.join(rel);
            if path.is_file() {
                return Some(path.canonicalize().unwrap_or(path));
            }
        }
    }
    for raw in [
        r"C:\Program Files\Huawei\DevEco Studio\tools\emulator\Emulator.exe",
        r"D:\Huawei\DevEco Studio\tools\emulator\Emulator.exe",
        r"C:\Program Files\Huawei\DevEco Studio\sdk\emulator\Emulator.exe",
    ] {
        let path = PathBuf::from(raw);
        if path.is_file() {
            return Some(path.canonicalize().unwrap_or(path));
        }
    }
    None
}

fn packaging_tool_executable() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("HOS_PACKAGING_TOOL") {
        let path = PathBuf::from(raw);
        if is_packaging_tool(&path) {
            return path.canonicalize().ok();
        }
    }
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = PathBuf::from(home);
        for path in [
            home.join("AppData/Local/Huawei/Sdk/toolchains/packagingtool.jar"),
            home.join("Library/Huawei/Sdk/toolchains/packagingtool.jar"),
        ] {
            if is_packaging_tool(&path) {
                return path.canonicalize().ok();
            }
        }
    }
    for raw in [
        "C:/Program Files/Huawei/DevEco Studio/tools/packagingtool.jar",
        "D:/Huawei/DevEco Studio/tools/packagingtool.jar",
        "D:/DevEco Studio/tools/packagingtool.jar",
    ] {
        let path = PathBuf::from(raw);
        if is_packaging_tool(&path) {
            return path.canonicalize().ok();
        }
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            let path = directory.join("resources/packagingtool.jar");
            if is_packaging_tool(&path) {
                return path.canonicalize().ok();
            }
        }
    }
    None
}

fn is_packaging_tool(path: &Path) -> bool {
    path.is_file()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("packagingtool.jar"))
}

fn emulator_arguments(capability: &HostCapability) -> Result<(Vec<String>, u64), String> {
    capability.validate()?;
    match capability {
        HostCapability::QueryEmulator { kind } => Ok((
            match kind {
                EmulatorQueryKind::Instances => vec!["-list".into()],
                EmulatorQueryKind::DownloadedImages => {
                    vec!["-imageList".into(), "-downloaded".into()]
                }
                EmulatorQueryKind::ScreenProfiles => vec!["-screenProfileList".into()],
            },
            60,
        )),
        HostCapability::StartEmulator { name } => {
            Ok((vec!["-start".into(), name.trim().into()], 30))
        }
        HostCapability::StopEmulator { name } => {
            Ok((vec!["-stop".into(), name.trim().into()], 60))
        }
        HostCapability::DeleteEmulator { name } => Ok((
            vec!["-delete".into(), name.trim().into(), "-force".into()],
            60,
        )),
        HostCapability::CreateEmulator {
            name,
            device_type,
            os_version,
            screen_profile,
            memory_gb,
            storage_gb,
        } => {
            let mut args = vec![
                "-create".into(),
                name.trim().into(),
                "-deviceType".into(),
                device_type.trim().into(),
                "-osVersion".into(),
                os_version.trim().into(),
            ];
            if let Some(profile) = screen_profile {
                args.extend(["-screenProfile".into(), profile.trim().into()]);
            }
            if let Some(memory) = memory_gb {
                args.extend(["-memory".into(), memory.to_string()]);
            }
            if let Some(storage) = storage_gb {
                args.extend(["-storage".into(), storage.to_string()]);
            }
            Ok((args, 180))
        }
        _ => Err("不是模拟器能力".into()),
    }
}

fn prepare_emulator_invocation(capability: &HostCapability) -> Result<HostInvocation, String> {
    let executable = emulator_executable()
        .ok_or("未找到 DevEco Studio 模拟器（Emulator.exe），请先安装 DevEco Studio")?;
    let (args, timeout_seconds) = emulator_arguments(capability)?;
    Ok(HostInvocation {
        program: executable.to_string_lossy().into_owned(),
        args,
        timeout_seconds,
    })
}

/// 执行经过类型化校验的宿主能力。调用方负责解释领域输出和完成后验证；本入口
/// 只允许固定程序/argv 模板，并保证成功、非零退出与启动失败都进入同一审计链。
pub async fn execute_host_capability(
    capability: &HostCapability,
    workspace: Option<&Path>,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<Output, String> {
    let ota_inputs = if matches!(capability, HostCapability::PackageOta { .. }) {
        let root = workspace.ok_or("OTA 能力审批需要明确工作区")?.to_path_buf();
        let approval_ctx = ctx.clone();
        let approved_capability = capability.clone();
        let verification = tokio::task::spawn_blocking(move || {
            let scope = crate::agent::broker_approval::verify_ota_capability(
                &approval_ctx, &approved_capability, &root,
            )?;
            crate::agent::ota_inputs::OtaInputs::create(&approved_capability, &root, &scope)
        }).await.map_err(|error| format!("Broker OTA 作用域复验任务失败：{error}"))
            .and_then(|result| result);
        let inputs = match verification {
            Ok(inputs) => inputs,
            Err(error) => {
                ctx.record_run_event("host_capability.rejected", serde_json::json!({
                    "capability_id": "release.package_ota", "reason": "approval_scope_or_snapshot_failed",
                    "tool_call_id": ctx.tool_call_id,
                }));
                return Err(error);
            }
        };
        if crate::agent::exec_ctx::current_tool_stop_requested() {
            return Err("OTA 审批复验后检测到停止请求，未派发进程".into());
        }
        Some(inputs)
    } else { None };
    let capability_id = capability.capability_id();
    let identity = match request_identity(ctx, capability) {
        Ok(identity) => identity,
        Err(error) => {
            ctx.record_run_event("host_capability.rejected", serde_json::json!({
                "capability_id": capability_id,
                "reason": "missing_request_identity",
            }));
            return Err(error);
        }
    };
    let mut invocation = match prepare_invocation(capability, workspace) {
        Ok(invocation) => invocation,
        Err(error) => {
            ctx.record_run_event("host_capability.rejected", serde_json::json!({
                "capability_id": capability_id,
                "tool_call_id": &identity.tool_call_id,
                "idempotency_key": &identity.idempotency_key,
                "reason": "validation_failed",
            }));
            return Err(error);
        }
    };
    if let Some(inputs) = &ota_inputs {
        inputs.apply_to_args(&mut invocation.args)?;
    }
    let subject = audit_subject(capability);
    if capability.replay_safe() {
        record_request_event(
            ctx,
            "host_capability.started",
            serde_json::json!({
                "capability_id": capability_id,
                "tool_call_id": &identity.tool_call_id,
                "idempotency_key": &identity.idempotency_key,
                "replay_safe": true,
                "subject": subject,
            }),
        )?;
    } else {
        match claim_request(ctx, capability_id, &identity, &subject)? {
            crate::agent::runtime::HostCapabilityClaim::Claimed => {}
            crate::agent::runtime::HostCapabilityClaim::Duplicate { status } => {
                ctx.record_run_event("host_capability.rejected", serde_json::json!({
                    "capability_id": capability_id,
                    "tool_call_id": &identity.tool_call_id,
                    "idempotency_key": &identity.idempotency_key,
                    "reason": "duplicate_dispatch",
                    "previous_status": status,
                }));
                return Err(format!(
                    "宿主能力请求已登记（状态：{status}），为避免重复副作用已拒绝自动重放；请先核验外部状态并发起新的工具调用"
                ));
            }
        }
    }
    let result = crate::agent::exec_ctx::run_cmd_streaming(
        ctx, &invocation.program, &invocation.args, None, invocation.timeout_seconds, None,
    ).await;
    match result {
        Ok(output) => {
            let status = if output.status.success() { "succeeded" } else { "failed" };
            if capability.replay_safe() {
                record_replay_safe_finish(
                    ctx, capability_id, &identity, status, output.status.code(), None,
                )
                .map_err(|error| format!("宿主查询已经返回，但写入审计终态失败：{error}"))?;
            } else {
                finish_request(ctx, capability_id, &identity, status, output.status.code(), None)
                    .map_err(|error| format!(
                        "宿主命令已经返回，但持久化终态失败，实际副作用状态不确定：{error}"
                    ))?;
            }
            Ok(output)
        }
        Err(error) => {
            let error_kind = capability_error_kind(&error);
            if capability.replay_safe() {
                record_replay_safe_finish(
                    ctx, capability_id, &identity, "failed", None, Some(error_kind),
                )
                .map_err(|persist_error| format!(
                    "{error}；宿主查询失败事件写入审计链失败：{persist_error}"
                ))?;
            } else {
                finish_request(
                    ctx, capability_id, &identity, "indeterminate", None, Some(error_kind),
                )
                .map_err(|persist_error| format!(
                    "{error}；宿主能力的不确定终态持久化失败：{persist_error}"
                ))?;
            }
            Err(error)
        }
    }
}

/// 派发一个会脱离当前工具调用继续运行的宿主 GUI 进程。
///
/// 该入口只接受 `emulator.start`：在 spawn 前完成持久化 claim，spawn 成功即记录
/// “派发成功”终态，实际设备上线由调用方通过独立 HDC 只读能力验证。它不把任意
/// executable/argv 暴露给 Agent。
pub fn dispatch_host_capability(
    capability: &HostCapability,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<Option<u32>, String> {
    if !matches!(capability, HostCapability::StartEmulator { .. }) {
        return Err("宿主进程派发入口目前只允许 emulator.start".into());
    }
    let capability_id = capability.capability_id();
    let identity = request_identity(ctx, capability).map_err(|error| {
        ctx.record_run_event("host_capability.rejected", serde_json::json!({
            "capability_id": capability_id,
            "reason": "missing_request_identity",
        }));
        error
    })?;
    let invocation = prepare_invocation(capability, None).map_err(|error| {
        ctx.record_run_event("host_capability.rejected", serde_json::json!({
            "capability_id": capability_id,
            "tool_call_id": &identity.tool_call_id,
            "idempotency_key": &identity.idempotency_key,
            "reason": "validation_failed",
        }));
        error
    })?;
    let subject = audit_subject(capability);
    match claim_request(ctx, capability_id, &identity, &subject)? {
        crate::agent::runtime::HostCapabilityClaim::Claimed => {}
        crate::agent::runtime::HostCapabilityClaim::Duplicate { status } => {
            ctx.record_run_event("host_capability.rejected", serde_json::json!({
                "capability_id": capability_id,
                "tool_call_id": &identity.tool_call_id,
                "idempotency_key": &identity.idempotency_key,
                "reason": "duplicate_dispatch",
                "previous_status": status,
            }));
            return Err(format!(
                "宿主进程派发请求已登记（状态：{status}），为避免重复启动已拒绝自动重放"
            ));
        }
    }

    let spawn_result = match crate::utils::process::command(&invocation.program, &invocation.args) {
        Ok(mut command) => command.spawn().map_err(|error| error.to_string()),
        Err(error) => Err(error),
    };
    match spawn_result {
        Ok(child) => {
            let pid = child.id();
            finish_request(
                ctx,
                capability_id,
                &identity,
                "succeeded",
                None,
                None,
            )
            .map_err(|error| {
                format!("模拟器进程已经派发（PID {pid:?}），但持久化派发终态失败，实际状态不确定：{error}")
            })?;
            Ok(pid)
        }
        Err(error) => {
            let message = format!("模拟器进程派发失败：{error}");
            finish_request(
                ctx,
                capability_id,
                &identity,
                "failed",
                None,
                Some("spawn_failed"),
            )
            .map_err(|persist_error| {
                format!("{message}；持久化失败终态时发生错误：{persist_error}")
            })?;
            Err(message)
        }
    }
}

/// 派发一个会跨越当前工具调用存活的受管宿主任务。
///
/// 目前只开放有硬时长上限的 screenrecord。请求在 spawn 前完成参数校验与原子 claim；
/// 后台进程退出后写入同一 claim 的终态。若进程或应用在此期间崩溃，未完成 claim 会由
/// Durable Run 恢复逻辑标为 indeterminate，禁止静默重放。
pub fn spawn_host_capability(
    capability: &HostCapability,
    workspace: Option<&Path>,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<tokio::task::JoinHandle<Result<Output, String>>, String> {
    if !matches!(capability, HostCapability::StartScreenRecording { .. }) {
        return Err("长任务入口目前只允许 device.screen_record.start".into());
    }
    let capability_id = capability.capability_id();
    let identity = request_identity(ctx, capability).map_err(|error| {
        ctx.record_run_event("host_capability.rejected", serde_json::json!({
            "capability_id": capability_id,
            "reason": "missing_request_identity",
        }));
        error
    })?;
    let invocation = prepare_invocation(capability, workspace).map_err(|error| {
        ctx.record_run_event("host_capability.rejected", serde_json::json!({
            "capability_id": capability_id,
            "tool_call_id": &identity.tool_call_id,
            "idempotency_key": &identity.idempotency_key,
            "reason": "validation_failed",
        }));
        error
    })?;
    let subject = audit_subject(capability);
    match claim_request(ctx, capability_id, &identity, &subject)? {
        crate::agent::runtime::HostCapabilityClaim::Claimed => {}
        crate::agent::runtime::HostCapabilityClaim::Duplicate { status } => {
            ctx.record_run_event("host_capability.rejected", serde_json::json!({
                "capability_id": capability_id,
                "tool_call_id": &identity.tool_call_id,
                "idempotency_key": &identity.idempotency_key,
                "reason": "duplicate_dispatch",
                "previous_status": status,
            }));
            return Err(format!(
                "宿主长任务请求已登记（状态：{status}），为避免重复副作用已拒绝自动重放"
            ));
        }
    }

    let task_ctx = ctx.clone();
    Ok(tokio::spawn(async move {
        let result = crate::agent::exec_ctx::run_cmd_streaming(
            &task_ctx,
            &invocation.program,
            &invocation.args,
            None,
            invocation.timeout_seconds,
            None,
        )
        .await;
        match result {
            Ok(output) => {
                let status = if output.status.success() { "succeeded" } else { "failed" };
                finish_request(
                    &task_ctx,
                    capability_id,
                    &identity,
                    status,
                    output.status.code(),
                    None,
                )
                .map_err(|error| {
                    format!("宿主长任务已经退出，但持久化终态失败，实际状态不确定：{error}")
                })?;
                Ok(output)
            }
            Err(error) => {
                let error_kind = capability_error_kind(&error);
                finish_request(
                    &task_ctx,
                    capability_id,
                    &identity,
                    "indeterminate",
                    None,
                    Some(error_kind),
                )
                .map_err(|persist_error| {
                    format!("{error}；宿主长任务的不确定终态持久化失败：{persist_error}")
                })?;
                Err(error)
            }
        }
    }))
}

fn record_replay_safe_finish(
    ctx: &crate::agent::exec_ctx::ToolCtx,
    capability_id: &str,
    identity: &HostRequestIdentity,
    status: &str,
    exit_code: Option<i32>,
    error_kind: Option<&str>,
) -> Result<(), String> {
    record_request_event(ctx, "host_capability.finished", serde_json::json!({
        "capability_id": capability_id,
        "tool_call_id": &identity.tool_call_id,
        "idempotency_key": &identity.idempotency_key,
        "replay_safe": true,
        "success": status == "succeeded",
        "status": status,
        "exit_code": exit_code,
        "error_kind": error_kind,
    }))
}

fn record_request_event(
    ctx: &crate::agent::exec_ctx::ToolCtx,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<(), String> {
    let app = ctx.app.as_ref().ok_or("Host Capability Broker 缺少应用数据库，拒绝执行")?;
    let db: tauri::State<crate::db::DbState> = tauri::Manager::state(app);
    let conn = db.0.lock().map_err(|_| "Host Capability Broker 数据库锁已损坏")?;
    crate::agent::runtime::append_event(
        &conn,
        &ctx.run_id,
        &ctx.conversation_id,
        event_type,
        payload,
    )
    .map(|_| ())
}

fn claim_request(
    ctx: &crate::agent::exec_ctx::ToolCtx,
    capability_id: &str,
    identity: &HostRequestIdentity,
    subject: &serde_json::Value,
) -> Result<crate::agent::runtime::HostCapabilityClaim, String> {
    let app = ctx.app.as_ref().ok_or("Host Capability Broker 缺少应用数据库，拒绝执行")?;
    let db: tauri::State<crate::db::DbState> = tauri::Manager::state(app);
    let conn = db.0.lock().map_err(|_| "Host Capability Broker 数据库锁已损坏")?;
    let mut subject = subject.clone();
    if capability_id == "release.package_ota" {
        crate::agent::broker_approval::verify_ota_approval(
            &conn, &ctx.run_id, &ctx.conversation_id, &identity.tool_call_id,
        )?;
        subject["approval"] = serde_json::json!({
            "policy": "fresh_explicit", "binding": "durable_tool_request_and_ota_scope_v2",
            "receipt_version": 3, "lifecycle": "process_stop_generation_30min",
            "input_policy": "verified_readonly_copies",
            "evidence_event": "host_capability.explicit_approval",
        });
    }
    crate::agent::runtime::claim_host_capability(
        &conn,
        &ctx.run_id,
        &ctx.conversation_id,
        &identity.tool_call_id,
        capability_id,
        &identity.request_digest,
        &identity.idempotency_key,
        &subject,
    )
}

fn finish_request(
    ctx: &crate::agent::exec_ctx::ToolCtx,
    capability_id: &str,
    identity: &HostRequestIdentity,
    status: &str,
    exit_code: Option<i32>,
    error_kind: Option<&str>,
) -> Result<(), String> {
    let app = ctx.app.as_ref().ok_or("Host Capability Broker 缺少应用数据库")?;
    let db: tauri::State<crate::db::DbState> = tauri::Manager::state(app);
    let conn = db.0.lock().map_err(|_| "Host Capability Broker 数据库锁已损坏")?;
    crate::agent::runtime::finish_host_capability(
        &conn,
        &ctx.run_id,
        &ctx.conversation_id,
        &identity.idempotency_key,
        capability_id,
        &identity.tool_call_id,
        status,
        exit_code,
        error_kind,
    )
}

fn resolve_workspace_artifact(workspace: &Path, relative: &str) -> Result<std::path::PathBuf, String> {
    let root = workspace.canonicalize().map_err(|e| format!("无法解析项目工作区：{e}"))?;
    let artifact = root.join(relative.trim()).canonicalize()
        .map_err(|e| format!("无法解析 HAP 产物：{e}"))?;
    if !artifact.starts_with(&root) || !artifact.is_file() {
        return Err("HAP 产物必须是项目工作区内的普通文件".into());
    }
    Ok(artifact)
}

fn resolve_workspace_output(
    workspace: &Path,
    relative: &str,
    extension: &str,
) -> Result<PathBuf, String> {
    validate_workspace_output_path(relative, extension)?;
    let root = workspace.canonicalize().map_err(|e| format!("无法解析项目工作区：{e}"))?;
    let requested = root.join(relative.trim());
    if requested.exists() {
        let output = requested
            .canonicalize()
            .map_err(|e| format!("无法解析输出文件：{e}"))?;
        if !output.starts_with(&root) || !output.is_file() {
            return Err("OTA 输出必须是项目工作区内的普通文件".into());
        }
        return Ok(output);
    }
    let parent = requested.parent().ok_or("OTA 输出缺少父目录")?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|e| format!("无法解析 OTA 输出父目录：{e}"))?;
    if !canonical_parent.starts_with(&root) {
        return Err("OTA 输出父目录通过符号链接逃逸项目工作区".into());
    }
    Ok(canonical_parent.join(requested.file_name().ok_or("OTA 输出缺少文件名")?))
}

fn resolve_workspace_profile(workspace: &Path, relative: &str) -> Result<PathBuf, String> {
    validate_workspace_profile_path(relative)?;
    let root = workspace.canonicalize().map_err(|e| format!("无法解析项目工作区：{e}"))?;
    let profile = root
        .join(relative.trim())
        .canonicalize()
        .map_err(|e| format!("无法解析 OTA profile：{e}"))?;
    if !profile.starts_with(&root) || !profile.is_file() {
        return Err("OTA profile 必须是项目工作区内的普通 JSON 文件".into());
    }
    Ok(profile)
}

fn resolve_workspace_source(workspace: &Path, relative: &str) -> Result<std::path::PathBuf, String> {
    let root = workspace.canonicalize().map_err(|e| format!("无法解析项目工作区：{e}"))?;
    let source = root
        .join(relative.trim())
        .canonicalize()
        .map_err(|e| format!("无法解析工作区源文件：{e}"))?;
    if !source.starts_with(&root) || !source.is_file() {
        return Err("本地源必须是项目工作区内的普通文件".into());
    }
    Ok(source)
}

fn resolve_workspace_destination(
    workspace: &Path,
    relative: &str,
) -> Result<std::path::PathBuf, String> {
    let root = workspace.canonicalize().map_err(|e| format!("无法解析项目工作区：{e}"))?;
    let requested = root.join(relative.trim());
    if requested.exists() {
        let destination = requested
            .canonicalize()
            .map_err(|e| format!("无法解析工作区目标文件：{e}"))?;
        if !destination.starts_with(&root) || !destination.is_file() {
            return Err("本地目标必须是项目工作区内的普通文件".into());
        }
        return Ok(destination);
    }
    let parent = requested.parent().ok_or("本地目标缺少父目录")?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|e| format!("无法解析本地目标父目录：{e}"))?;
    if !canonical_parent.starts_with(&root) {
        return Err("本地目标父目录通过符号链接逃逸项目工作区".into());
    }
    let file_name = requested.file_name().ok_or("本地目标缺少文件名")?;
    Ok(canonical_parent.join(file_name))
}

fn validate_app_identifier(value: &str, label: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 256
        || !value.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '$'))
    {
        return Err(format!("{label} 标识非法"));
    }
    Ok(())
}

fn validate_process_id(pid: u32) -> Result<(), String> {
    if pid == 0 {
        return Err("进程 PID 必须大于 0".into());
    }
    Ok(())
}

fn validate_emulator_value(value: &str, label: &str, max_bytes: usize) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > max_bytes
        || trimmed.starts_with('-')
        || trimmed.chars().any(char::is_control)
    {
        return Err(format!(
            "{label}不能为空、不能以 - 开头、不得含控制字符且最多 {max_bytes} 字节"
        ));
    }
    Ok(())
}

fn audit_subject(capability: &HostCapability) -> serde_json::Value {
    match capability {
        HostCapability::HdcConnect { target } | HostCapability::HdcDisconnect { target } =>
            serde_json::json!({ "device_digest": short_digest(target.trim()) }),
        HostCapability::HdcListTargets
        | HostCapability::HdcStartServer
        | HostCapability::HdcKillServer => serde_json::json!({}),
        HostCapability::DevicePidof { device, bundle } => serde_json::json!({
            "device_digest": short_digest(device), "bundle": bundle,
        }),
        HostCapability::AttachDeviceDebugger { device, pid, wait_seconds } => serde_json::json!({
            "device_digest": short_digest(device), "pid": pid, "wait_seconds": wait_seconds,
        }),
        HostCapability::EnableAbilityDebug { device, bundle } => serde_json::json!({
            "device_digest": short_digest(device), "bundle": bundle,
        }),
        HostCapability::ControlDeviceDebugger { device, pid, action } => serde_json::json!({
            "device_digest": short_digest(device), "pid": pid,
            "action": action.as_command(),
        }),
        HostCapability::QueryEmulator { kind } => serde_json::json!({
            "query": match kind {
                EmulatorQueryKind::Instances => "instances",
                EmulatorQueryKind::DownloadedImages => "downloaded_images",
                EmulatorQueryKind::ScreenProfiles => "screen_profiles",
            },
        }),
        HostCapability::StartEmulator { name }
        | HostCapability::StopEmulator { name }
        | HostCapability::DeleteEmulator { name } => serde_json::json!({
            "instance_digest": short_digest(name),
        }),
        HostCapability::CreateEmulator {
            name,
            device_type,
            os_version,
            screen_profile,
            memory_gb,
            storage_gb,
        } => serde_json::json!({
            "instance_digest": short_digest(name),
            "device_type": device_type,
            "os_version": os_version,
            "screen_profile": screen_profile,
            "memory_gb": memory_gb,
            "storage_gb": storage_gb,
        }),
        HostCapability::PackageOta { hap_path, output_path, profile_path } => serde_json::json!({
            "artifact": hap_path,
            "output": output_path,
            "profile_digest": profile_path.as_deref().map(short_digest),
        }),
        HostCapability::ReadDeviceUdid { device } => serde_json::json!({
            "device_digest": short_digest(device),
        }),
        HostCapability::ReadHilog { device, level, tag } => serde_json::json!({
            "device_digest": short_digest(device), "level": level, "tag": tag,
        }),
        HostCapability::SearchHilog { device, level, tag, tail_lines, expression } => serde_json::json!({
            "device_digest": short_digest(device), "level": level, "tag": tag,
            "tail_lines": tail_lines,
            "expression_digest": expression.as_deref().map(short_digest),
        }),
        HostCapability::ReadLogcat { device, lines } => serde_json::json!({
            "device_digest": short_digest(device), "lines": lines,
        }),
        HostCapability::ListFaultLogs { device, directory } => serde_json::json!({
            "device_digest": short_digest(device), "directory": directory.as_path(),
        }),
        HostCapability::ReadFaultLog { device, directory, filename } => serde_json::json!({
            "device_digest": short_digest(device),
            "directory": directory.as_path(),
            "filename_digest": short_digest(filename),
        }),
        HostCapability::DeviceReadQuery { device, argv } => serde_json::json!({
            "device_digest": short_digest(device),
            "command": argv.first(),
            "query_digest": short_digest(&argv.join("\0")),
        }),
        HostCapability::ReadNetworkCondition { device, interface } => serde_json::json!({
            "device_digest": short_digest(device), "interface": interface,
        }),
        HostCapability::ConfigureNetworkCondition {
            device, interface, delay_ms, loss_pct, bandwidth_kbps,
        } => serde_json::json!({
            "device_digest": short_digest(device), "interface": interface,
            "delay_ms": delay_ms, "loss_pct": loss_pct, "bandwidth_kbps": bandwidth_kbps,
            "action": if *delay_ms == 0 && *loss_pct == 0 && *bandwidth_kbps == 0 {
                "clear"
            } else {
                "replace"
            },
        }),
        HostCapability::SendFile { device, local_path, remote_path } => serde_json::json!({
            "device_digest": short_digest(device), "local_path": local_path,
            "remote_digest": short_digest(remote_path),
        }),
        HostCapability::ReceiveFile { device, remote_path, local_path } => serde_json::json!({
            "device_digest": short_digest(device), "remote_digest": short_digest(remote_path),
            "local_path": local_path,
        }),
        HostCapability::CaptureDeviceScreenshot { device, remote_path, backend } => serde_json::json!({
            "device_digest": short_digest(device), "remote_digest": short_digest(remote_path),
            "backend": backend.as_str(),
        }),
        HostCapability::DumpUiLayout { device, remote_path }
        | HostCapability::RemoveDeviceTempFile { device, remote_path } => serde_json::json!({
            "device_digest": short_digest(device), "remote_digest": short_digest(remote_path),
        }),
        HostCapability::DeviceUiInput { device, operation_id, action } => serde_json::json!({
            "device_digest": short_digest(device),
            "operation_digest": short_digest(operation_id),
            "action": device_ui_action_audit(action),
        }),
        HostCapability::StopAbility { device, bundle } => serde_json::json!({
            "device_digest": short_digest(device), "bundle": bundle,
        }),
        HostCapability::UninstallBundle { device, bundle, keep_data } => serde_json::json!({
            "device_digest": short_digest(device), "bundle": bundle, "keep_data": keep_data,
        }),
        HostCapability::ClearAppStorage { device, bundle, target } => serde_json::json!({
            "device_digest": short_digest(device), "bundle": bundle,
            "target": match target { AppStorageTarget::Cache => "cache", AppStorageTarget::Data => "data" },
        }),
        HostCapability::ChangeAppPermission {
            device, bundle, permission, grant, backend,
        } => serde_json::json!({
            "device_digest": short_digest(device), "bundle": bundle,
            "permission": permission, "action": if *grant { "grant" } else { "revoke" },
            "backend": match backend {
                PermissionCommandBackend::NamedFlags => "named_flags",
                PermissionCommandBackend::Positional => "positional",
            },
        }),
        HostCapability::SetDeviceRadio { device, radio, enable, backend } => serde_json::json!({
            "device_digest": short_digest(device),
            "radio": match radio { DeviceRadio::Wifi => "wifi", DeviceRadio::AirplaneMode => "airplane" },
            "enable": enable,
            "backend": match backend {
                DeviceRadioBackend::WifiCommand => "wifi_command",
                DeviceRadioBackend::WpaCli => "wpa_cli",
                DeviceRadioBackend::Svc => "svc",
                DeviceRadioBackend::PowerCommand => "power_command",
                DeviceRadioBackend::GlobalSettings => "global_settings",
            },
        }),
        HostCapability::StartUiRecording { device, remote_path } => serde_json::json!({
            "device_digest": short_digest(device), "remote_digest": short_digest(remote_path),
        }),
        HostCapability::StopUiRecording { device } => serde_json::json!({
            "device_digest": short_digest(device),
        }),
        HostCapability::ProbeScreenRecording { device }
        | HostCapability::StopScreenRecording { device } => serde_json::json!({
            "device_digest": short_digest(device),
        }),
        HostCapability::StartScreenRecording { device, remote_path, max_seconds } => serde_json::json!({
            "device_digest": short_digest(device),
            "remote_digest": short_digest(remote_path),
            "max_seconds": max_seconds,
        }),
        HostCapability::InstallHap { device, hap_path, replace } => serde_json::json!({
            "device_digest": device.as_deref().map(short_digest), "artifact": hap_path, "replace": replace,
        }),
        HostCapability::StartAbility { device, bundle, ability } => serde_json::json!({
            "device_digest": short_digest(device), "bundle": bundle, "ability": ability,
        }),
        HostCapability::StartAbilityIntent { device, bundle, ability, uri } => serde_json::json!({
            "device_digest": short_digest(device),
            "bundle": bundle,
            "ability": ability,
            "uri_digest": uri.as_deref().map(short_digest),
        }),
        HostCapability::Deploy { device, hap_path } => serde_json::json!({
            "device_digest": device.as_deref().map(short_digest), "artifact": hap_path,
        }),
    }
}

fn short_digest(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("{:x}", digest)[..12].to_string()
}

fn capability_error_kind(error: &str) -> &'static str {
    if error.contains("已停止") { "cancelled" }
    else if error.contains("超时") { "timeout" }
    else { "execution_failed" }
}

fn validate_ability_uri(uri: &str) -> Result<(), String> {
    let uri = uri.trim();
    if uri.is_empty() || uri.len() > 2_048 || uri.chars().any(char::is_control) {
        return Err("Ability URI 不能为空、不得含控制字符且最多 2048 字节".into());
    }
    Ok(())
}

fn validate_device_target(target: &str) -> Result<(), String> {
    let t = target.trim();
    if t.is_empty() || t.len() > 256 {
        return Err("设备 target 不能为空且不得超过 256 字符".into());
    }
    let unsafe_char = |c: char| {
        c.is_control()
            || matches!(
                c,
                ';' | '|' | '&' | '>' | '<' | '$' | '`' | '(' | ')' | '\'' | '"' | '\\'
            )
    };
    if t.contains(unsafe_char) {
        return Err(format!("设备 target 含非法字符：{t}"));
    }
    Ok(())
}

fn validate_log_tag(tag: &str) -> Result<(), String> {
    let tag = tag.trim();
    if tag.is_empty() || tag.len() > 128 || tag.chars().any(char::is_control) {
        return Err("hilog tag 不能为空、不得含控制字符且最多 128 字符".into());
    }
    Ok(())
}

fn validate_log_expression(expression: &str) -> Result<(), String> {
    if expression.is_empty() || expression.len() > 256 || expression.chars().any(char::is_control) {
        return Err("hilog 表达式不能为空、不得含控制字符且最多 256 字符".into());
    }
    Ok(())
}

fn validate_workspace_relative_path(path: &str) -> Result<(), String> {
    let path = path.trim();
    let parsed = Path::new(path);
    if path.is_empty() || parsed.is_absolute() {
        return Err("本地文件路径必须是项目工作区内的相对路径".into());
    }
    if parsed.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::RootDir
        )
    }) {
        return Err("本地文件路径不得包含上级目录或根目录".into());
    }
    Ok(())
}

fn validate_device_path(path: &str) -> Result<(), String> {
    let path = path.trim();
    if !path.starts_with('/') || path.len() > 1024 || path.chars().any(char::is_control) {
        return Err("设备路径必须是无控制字符且不超过 1024 字符的绝对路径".into());
    }
    if path.split('/').any(|segment| segment == "..") {
        return Err("设备路径不得包含上级目录 ..".into());
    }
    Ok(())
}

fn validate_managed_device_temp_path(path: &str) -> Result<(), String> {
    const PREFIX: &str = "/data/local/tmp/deveco_agent_";
    let path = path.trim();
    let basename = path
        .strip_prefix(PREFIX)
        .ok_or("设备临时文件必须位于 Broker 管理前缀 /data/local/tmp/deveco_agent_")?;
    if basename.is_empty()
        || basename.len() > 220
        || basename.starts_with('.')
        || basename.contains('/')
        || !basename
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        || !matches!(Path::new(basename).extension().and_then(|value| value.to_str()), Some("png" | "json" | "mp4" | "csv"))
    {
        return Err("设备临时文件必须是受管前缀下的安全 .png/.json/.mp4/.csv basename".into());
    }
    Ok(())
}

fn validate_device_ui_action(action: &DeviceUiAction) -> Result<(), String> {
    let coordinate = |value: i64| (0..=100_000).contains(&value);
    match action {
        DeviceUiAction::Click { x, y } | DeviceUiAction::LongClick { x, y } => {
            if !coordinate(*x) || !coordinate(*y) {
                return Err("UI 坐标必须在 0-100000 之间".into());
            }
        }
        DeviceUiAction::Swipe { x1, y1, x2, y2, speed } => {
            if ![*x1, *y1, *x2, *y2].into_iter().all(coordinate) {
                return Err("UI 滑动坐标必须在 0-100000 之间".into());
            }
            if !(1..=10_000).contains(speed) {
                return Err("UI 滑动速度必须在 1-10000 之间".into());
            }
        }
        DeviceUiAction::Text { text } => {
            if text.is_empty() || text.len() > 2_000 || text.chars().any(|ch| ch == '\0' || ch.is_control()) {
                return Err("UI 输入文本不能为空、不得含控制字符且最多 2000 字节".into());
            }
        }
        DeviceUiAction::Key { name } => {
            if name.is_empty()
                || name.len() > 32
                || !name.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
            {
                return Err("UI 按键名非法".into());
            }
        }
    }
    Ok(())
}

fn validate_operation_id(operation_id: &str) -> Result<(), String> {
    let value = operation_id.trim();
    if value.is_empty()
        || value.len() > 128
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    {
        return Err("UI 动作 operation_id 非法".into());
    }
    Ok(())
}

fn device_ui_action_material(action: &DeviceUiAction) -> String {
    match action {
        DeviceUiAction::Click { x, y } => format!("click\0{x}\0{y}"),
        DeviceUiAction::Swipe { x1, y1, x2, y2, speed } => {
            format!("swipe\0{x1}\0{y1}\0{x2}\0{y2}\0{speed}")
        }
        DeviceUiAction::LongClick { x, y } => format!("long_click\0{x}\0{y}"),
        DeviceUiAction::Text { text } => format!("text\0{}", short_digest(text)),
        DeviceUiAction::Key { name } => format!("key\0{name}"),
    }
}

fn device_ui_action_audit(action: &DeviceUiAction) -> serde_json::Value {
    match action {
        DeviceUiAction::Click { x, y } => serde_json::json!({ "kind": "click", "x": x, "y": y }),
        DeviceUiAction::Swipe { x1, y1, x2, y2, speed } => serde_json::json!({
            "kind": "swipe", "x1": x1, "y1": y1, "x2": x2, "y2": y2, "speed": speed,
        }),
        DeviceUiAction::LongClick { x, y } => {
            serde_json::json!({ "kind": "long_click", "x": x, "y": y })
        }
        DeviceUiAction::Text { text } => serde_json::json!({
            "kind": "text", "text_digest": short_digest(text), "text_bytes": text.len(),
        }),
        DeviceUiAction::Key { name } => serde_json::json!({ "kind": "key", "name": name }),
    }
}

fn validate_network_interface(interface: &str) -> Result<(), String> {
    let interface = interface.trim();
    if interface.is_empty()
        || interface.len() > 64
        || !interface
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err("网络接口名非法".into());
    }
    Ok(())
}

pub fn validate_faultlog_filename(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 255
        || name.starts_with('.')
        || !name.chars().any(|c| c.is_ascii_digit())
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("faultlog 文件名必须是含数字的安全 basename".into());
    }
    Ok(())
}

const DEVICE_QUERY_COMMANDS: &[&str] = &[
    "ps", "ls", "cat", "df", "free", "uptime", "date", "top", "netstat", "ip",
    "ifconfig", "getprop", "param", "pwd", "dmesg", "echo", "hidumper", "aa", "bm",
    "wm", "command",
];
const DEVICE_QUERY_FORBIDDEN_PREFIXES: &[&str] = &[
    "rm", "mv", "cp", "kill", "pkill", "reboot", "shutdown", "mount", "umount", "chmod",
    "chown", "mkfs", "wipe", "flash", "format", "dd", "sed", "awk", "su", "install",
];
const DEVICE_QUERY_MUTATORS: &[&str] = &["set", "add", "del", "delete", "flush", "replace", "clear"];

/// 只接受已分词 argv；调用方与执行准备阶段都会调用，防止验证后参数漂移。
pub fn validate_read_only_device_command(argv: &[String]) -> Result<Vec<String>, String> {
    if argv.is_empty() || argv.len() > 64 {
        return Err("设备查询命令必须包含 1-64 个参数".into());
    }
    if argv.iter().any(|token| {
        token.is_empty()
            || token.len() > 512
            || !token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "/._-:+=,[%]".contains(c))
    }) {
        return Err("设备查询 argv 含空参数、超长参数或非法字符".into());
    }
    let command = argv[0].as_str();
    if !DEVICE_QUERY_COMMANDS.contains(&command) {
        return Err(format!("命令 {command} 不在设备只读查询白名单"));
    }
    if let Some(bad) = argv.iter().skip(1).find(|token| {
        let normalized = token.trim_start_matches('-');
        DEVICE_QUERY_FORBIDDEN_PREFIXES
            .iter()
            .any(|item| normalized.starts_with(item))
            || DEVICE_QUERY_MUTATORS.contains(&normalized)
    }) {
        return Err(format!("设备查询拒绝修改型参数 {bad}"));
    }
    match command {
        "aa" | "bm" if argv.get(1).map(String::as_str) != Some("dump") => {
            return Err(format!("{command} 仅允许 dump 查询子命令"));
        }
        "param" if argv.get(1).map(String::as_str) != Some("get") => {
            return Err("param 仅允许 get 查询子命令".into());
        }
        "wm" if argv.len() != 2 || argv.get(1).map(String::as_str) != Some("size") => {
            return Err("wm 仅允许 size 查询".into());
        }
        "command"
            if argv.len() != 3
                || argv.get(1).map(String::as_str) != Some("-v")
                || !matches!(
                    argv.get(2).map(String::as_str),
                    Some("snapshot_display" | "uitest" | "hidumper")
                ) =>
        {
            return Err("command 仅允许探测 snapshot_display、uitest 或 hidumper".into());
        }
        "ifconfig" if argv.len() > 2 => {
            return Err("ifconfig 仅允许无参数、-a 或单个网卡名查询".into());
        }
        "ifconfig" if argv.get(1).is_some_and(|arg| arg.starts_with('-') && arg != "-a") => {
            return Err("ifconfig 仅允许 -a 查询选项".into());
        }
        "dmesg" if argv.iter().any(|arg| {
            matches!(arg.as_str(), "-c" | "-C" | "--clear" | "-n" | "--console-level")
        }) => {
            return Err("dmesg 禁止清空日志或修改控制台日志级别".into());
        }
        "date"
            if argv
                .iter()
                .skip(1)
                .any(|arg| !arg.starts_with('+') && arg != "-u" && arg != "-R") =>
        {
            return Err("date 仅允许无参数、-u、-R 或 +FORMAT 查询".into());
        }
        _ => {}
    }
    Ok(argv.to_vec())
}

fn validate_hap_path(path: &str) -> Result<(), String> {
    let path = path.trim();
    let p = Path::new(path);
    if path.is_empty() {
        return Err("hap 路径不能为空".into());
    }
    if p.is_absolute() {
        return Err("hap 路径必须是项目工作树内的相对路径".into());
    }
    if p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err("hap 路径不得包含上级目录 ..".into());
    }
    let name = p
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if !name.ends_with(".hap") {
        return Err("安装/部署能力只接受 .hap 产物".into());
    }
    Ok(())
}

fn validate_workspace_output_path(path: &str, extension: &str) -> Result<(), String> {
    validate_workspace_relative_extension(path, extension, "输出路径")
}

fn validate_workspace_profile_path(path: &str) -> Result<(), String> {
    validate_workspace_relative_extension(path, "json", "profile 路径")
}

fn validate_workspace_relative_extension(
    path: &str,
    extension: &str,
    label: &str,
) -> Result<(), String> {
    let path = path.trim();
    let parsed = Path::new(path);
    if path.is_empty() || parsed.is_absolute() {
        return Err(format!("{label}必须是项目工作区内的相对路径"));
    }
    if parsed.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        return Err(format!("{label}不得包含上级目录或绝对路径前缀"));
    }
    let matches_extension = parsed
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(extension));
    if !matches_extension {
        return Err(format!("{label}必须使用 .{extension} 后缀"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_well_formed_capabilities() {
        assert!(HostCapability::HdcConnect { target: "192.168.1.10:5555".into() }.validate().is_ok());
        assert!(HostCapability::HdcListTargets.validate().is_ok());
        assert!(HostCapability::InstallHap {
            device: Some("ABC123".into()),
            hap_path: "entry/build/outputs/entry-default-signed.hap".into(),
            replace: false,
        }
        .validate()
        .is_ok());
        assert!(HostCapability::Deploy { device: None, hap_path: "app.hap".into() }.validate().is_ok());
    }

    #[test]
    fn rejects_shell_metacharacters_in_target() {
        assert!(HostCapability::HdcConnect { target: "x; rm -rf /".into() }.validate().is_err());
        assert!(HostCapability::HdcConnect { target: "x | cat /etc/passwd".into() }.validate().is_err());
    }

    #[test]
    fn rejects_absolute_and_parent_paths_in_hap() {
        assert!(HostCapability::InstallHap {
            device: None,
            hap_path: "/etc/passwd.hap".into(),
            replace: false,
        }
        .validate()
        .is_err());
        assert!(HostCapability::Deploy {
            device: None,
            hap_path: "../../secret.hap".into(),
        }
        .validate()
        .is_err());
        // 非 .hap 产物拒绝
        assert!(HostCapability::InstallHap { device: None, hap_path: "app.bin".into(), replace: false }.validate().is_err());
    }

    #[test]
    fn capability_ids_are_stable_for_audit() {
        assert_eq!(HostCapability::HdcConnect { target: "t".into() }.capability_id(), "hdc.connect");
        assert_eq!(HostCapability::HdcStartServer.capability_id(), "hdc.start_server");
        assert_eq!(HostCapability::HdcKillServer.capability_id(), "hdc.kill_server");
        assert_eq!(HostCapability::Deploy { device: None, hap_path: "a.hap".into() }.capability_id(), "deploy");
    }

    #[test]
    fn daemon_lifecycle_uses_fixed_argv_and_only_queries_are_replay_safe() {
        let start = prepare_invocation(&HostCapability::HdcStartServer, None).unwrap();
        let kill = prepare_invocation(&HostCapability::HdcKillServer, None).unwrap();
        assert_eq!(start.args, vec!["start"]);
        assert_eq!(kill.args, vec!["kill"]);
        assert!(HostCapability::HdcListTargets.replay_safe());
        assert!(!HostCapability::HdcStartServer.replay_safe());
        assert!(!HostCapability::HdcKillServer.replay_safe());
    }

    #[test]
    fn log_queries_use_validated_fixed_argv() {
        let hilog = HostCapability::ReadHilog {
            device: "ABC123".into(),
            level: Some("E".into()),
            tag: Some("MyApp".into()),
        };
        let invocation = prepare_invocation(&hilog, None).unwrap();
        assert_eq!(
            invocation.args,
            vec!["-t", "ABC123", "shell", "hilog", "-x", "-L", "E", "-T", "MyApp"]
        );
        assert!(hilog.replay_safe());
        assert!(HostCapability::ReadHilog {
            device: "ABC123".into(),
            level: Some("verbose".into()),
            tag: None,
        }
        .validate()
        .is_err());
        let search = HostCapability::SearchHilog {
            device: "ABC123".into(),
            level: "W".into(),
            tag: Some("MyApp".into()),
            tail_lines: 500,
            expression: Some("TypeError.*Entry".into()),
        };
        assert_eq!(
            prepare_invocation(&search, None).unwrap().args,
            vec![
                "-t", "ABC123", "shell", "hilog", "-x", "-z", "500", "-v", "epoch",
                "-L", "W", "-T", "MyApp", "-e", "TypeError.*Entry"
            ]
        );
        assert!(search.replay_safe());
        assert!(HostCapability::SearchHilog {
            device: "ABC123".into(),
            level: "W".into(),
            tag: None,
            tail_lines: 50_000,
            expression: None,
        }
        .validate()
        .is_err());
        assert!(HostCapability::ReadHilog {
            device: "ABC123".into(),
            level: None,
            tag: Some("bad\ntag".into()),
        }
        .validate()
        .is_err());
        assert!(HostCapability::ReadLogcat { device: "ABC123".into(), lines: 9 }
            .validate()
            .is_err());
        let faultlogs = HostCapability::ListFaultLogs {
            device: "ABC123".into(),
            directory: FaultLogDirectory::Temp,
        };
        let invocation = prepare_invocation(&faultlogs, None).unwrap();
        assert_eq!(
            invocation.args,
            vec!["-t", "ABC123", "shell", "ls", "-1", "/data/log/faultlog/temp"]
        );
        assert!(faultlogs.replay_safe());
        let udid = HostCapability::ReadDeviceUdid { device: "ABC123".into() };
        let invocation = prepare_invocation(&udid, None).unwrap();
        assert_eq!(invocation.args, vec!["-t", "ABC123", "shell", "bm", "get", "-u"]);
        assert!(udid.replay_safe());
        let read_fault = HostCapability::ReadFaultLog {
            device: "ABC123".into(),
            directory: FaultLogDirectory::Temp,
            filename: "JsError-com.example-20250102123456.log".into(),
        };
        let invocation = prepare_invocation(&read_fault, None).unwrap();
        assert_eq!(
            invocation.args,
            vec![
                "-t", "ABC123", "shell", "cat",
                "/data/log/faultlog/temp/JsError-com.example-20250102123456.log"
            ]
        );
        assert!(read_fault.replay_safe());
        assert!(HostCapability::ReadFaultLog {
            device: "ABC123".into(),
            directory: FaultLogDirectory::Temp,
            filename: "../20250102123456.log".into(),
        }
        .validate()
        .is_err());
        let query = HostCapability::DeviceReadQuery {
            device: "ABC123".into(),
            argv: vec!["param".into(), "get".into(), "const.product.model".into()],
        };
        let invocation = prepare_invocation(&query, None).unwrap();
        assert_eq!(
            invocation.args,
            vec!["-t", "ABC123", "shell", "param", "get", "const.product.model"]
        );
        assert!(query.replay_safe());
        for argv in [
            vec!["wm".into(), "size".into()],
            vec!["command".into(), "-v".into(), "snapshot_display".into()],
            vec!["command".into(), "-v".into(), "uitest".into()],
            vec!["command".into(), "-v".into(), "hidumper".into()],
        ] {
            assert!(HostCapability::DeviceReadQuery { device: "ABC123".into(), argv }
                .validate()
                .is_ok());
        }
        for argv in [
            vec!["param".into(), "set".into(), "x".into(), "y".into()],
            vec!["ip".into(), "link".into(), "set".into(), "wlan0".into()],
            vec!["aa".into(), "start".into(), "dump".into()],
            vec!["dmesg".into(), "-c".into()],
            vec!["date".into(), "--set".into(), "2030-01-01".into()],
            vec!["date".into(), "20300101".into()],
            vec!["wm".into(), "density".into()],
            vec!["command".into(), "-v".into(), "sh".into()],
        ] {
            assert!(HostCapability::DeviceReadQuery { device: "ABC123".into(), argv }
                .validate()
                .is_err());
        }
    }

    #[test]
    fn file_transfer_and_stop_use_scoped_fixed_argv() {
        let root = std::env::temp_dir().join(format!("harmony-transfer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("in")).unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("in/data.bin"), b"data").unwrap();
        let send = prepare_invocation(
            &HostCapability::SendFile {
                device: "ABC123".into(),
                local_path: "in/data.bin".into(),
                remote_path: "/data/local/tmp/data.bin".into(),
            },
            Some(&root),
        )
        .unwrap();
        assert_eq!(&send.args[..4], ["-t", "ABC123", "file", "send"]);
        assert!(send.args[4].ends_with("in/data.bin"));
        assert_eq!(send.args[5], "/data/local/tmp/data.bin");
        let receive = prepare_invocation(
            &HostCapability::ReceiveFile {
                device: "ABC123".into(),
                remote_path: "/data/local/tmp/result.txt".into(),
                local_path: "out/result.txt".into(),
            },
            Some(&root),
        )
        .unwrap();
        assert_eq!(&receive.args[..4], ["-t", "ABC123", "file", "recv"]);
        assert!(receive.args[5].ends_with("out/result.txt"));
        let stop = prepare_invocation(
            &HostCapability::StopAbility {
                device: "ABC123".into(),
                bundle: "com.example.app".into(),
            },
            None,
        )
        .unwrap();
        assert_eq!(
            stop.args,
            vec!["-t", "ABC123", "shell", "aa", "force-stop", "com.example.app"]
        );
        let uninstall = prepare_invocation(
            &HostCapability::UninstallBundle {
                device: "ABC123".into(),
                bundle: "com.example.app".into(),
                keep_data: false,
            },
            None,
        )
        .unwrap();
        assert_eq!(
            uninstall.args,
            vec!["-t", "ABC123", "shell", "bm", "uninstall", "-n", "com.example.app"]
        );
        assert_eq!(
            prepare_invocation(
                &HostCapability::UninstallBundle {
                    device: "ABC123".into(),
                    bundle: "com.example.app".into(),
                    keep_data: true,
                },
                None,
            )
            .unwrap()
            .args,
            vec![
                "-t", "ABC123", "shell", "bm", "uninstall", "-k", "-n", "com.example.app",
            ]
        );
        assert!(!HostCapability::UninstallBundle {
            device: "ABC123".into(),
            bundle: "com.example.app".into(),
            keep_data: false,
        }
        .replay_safe());
        let clear = HostCapability::ClearAppStorage {
            device: "ABC123".into(),
            bundle: "com.example.app".into(),
            target: AppStorageTarget::Cache,
        };
        assert_eq!(
            prepare_invocation(&clear, None).unwrap().args,
            vec!["-t", "ABC123", "shell", "bm", "clean", "-c", "-n", "com.example.app"]
        );
        assert!(!clear.replay_safe());
        let permission = HostCapability::ChangeAppPermission {
            device: "ABC123".into(),
            bundle: "com.example.app".into(),
            permission: "ohos.permission.CAMERA".into(),
            grant: true,
            backend: PermissionCommandBackend::NamedFlags,
        };
        assert_eq!(
            prepare_invocation(&permission, None).unwrap().args,
            vec![
                "-t", "ABC123", "shell", "bm", "grant-permission", "-n",
                "com.example.app", "-p", "ohos.permission.CAMERA",
            ]
        );
        assert!(!permission.replay_safe());
        assert!(HostCapability::ChangeAppPermission {
            device: "ABC123".into(),
            bundle: "com.example.app".into(),
            permission: "ohos.permission.CAMERA;bad".into(),
            grant: true,
            backend: PermissionCommandBackend::Positional,
        }
        .validate()
        .is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn device_capture_temp_files_use_scoped_fixed_argv() {
        let remote = "/data/local/tmp/deveco_agent_shot_20250101.png";
        let snapshot = HostCapability::CaptureDeviceScreenshot {
            device: "ABC123".into(),
            remote_path: remote.into(),
            backend: DeviceScreenshotBackend::SnapshotDisplay,
        };
        assert_eq!(
            prepare_invocation(&snapshot, None).unwrap().args,
            vec![
                "-t", "ABC123", "shell", "snapshot_display", "-t", "png", "-f", remote,
            ]
        );
        assert!(!snapshot.replay_safe());
        assert_eq!(
            prepare_invocation(
                &HostCapability::CaptureDeviceScreenshot {
                    device: "ABC123".into(),
                    remote_path: remote.into(),
                    backend: DeviceScreenshotBackend::Screencap,
                },
                None,
            )
            .unwrap()
            .args,
            vec!["-t", "ABC123", "shell", "screencap", "-p", remote]
        );
        let layout = "/data/local/tmp/deveco_agent_layout_20250101.json";
        assert_eq!(
            prepare_invocation(
                &HostCapability::DumpUiLayout {
                    device: "ABC123".into(),
                    remote_path: layout.into(),
                },
                None,
            )
            .unwrap()
            .args,
            vec!["-t", "ABC123", "shell", "uitest", "dumpLayout", "-p", layout]
        );
        assert_eq!(
            prepare_invocation(
                &HostCapability::RemoveDeviceTempFile {
                    device: "ABC123".into(),
                    remote_path: layout.into(),
                },
                None,
            )
            .unwrap()
            .args,
            vec!["-t", "ABC123", "shell", "rm", "-f", layout]
        );
        let record = "/data/local/tmp/deveco_agent_ui_record_20250101.csv";
        assert_eq!(
            prepare_invocation(
                &HostCapability::StartUiRecording {
                    device: "ABC123".into(),
                    remote_path: record.into(),
                },
                None,
            )
            .unwrap()
            .args,
            vec![
                "-t", "ABC123", "shell", "uitest", "uiRecord", "record", "-p", record,
            ]
        );
        assert_eq!(
            prepare_invocation(
                &HostCapability::StopUiRecording { device: "ABC123".into() },
                None,
            )
            .unwrap()
            .args,
            vec!["-t", "ABC123", "shell", "uitest", "uiRecord", "stop"]
        );
        for invalid in [
            "/data/local/tmp/layout.json",
            "/data/local/tmp/deveco_agent_../layout.json",
            "/data/local/tmp/deveco_agent_bad;name.png",
            "/data/local/tmp/deveco_agent_script.sh",
        ] {
            assert!(HostCapability::DumpUiLayout {
                device: "ABC123".into(),
                remote_path: invalid.into(),
            }
            .validate()
            .is_err());
        }
    }

    #[test]
    fn device_ui_input_uses_typed_bounded_fixed_argv() {
        let click = HostCapability::DeviceUiInput {
            device: "ABC123".into(),
            operation_id: "step-1".into(),
            action: DeviceUiAction::Click { x: 120, y: 240 },
        };
        assert_eq!(
            prepare_invocation(&click, None).unwrap().args,
            vec!["-t", "ABC123", "shell", "uitest", "uiInput", "click", "120", "240"]
        );
        assert!(!click.replay_safe());
        let swipe = HostCapability::DeviceUiInput {
            device: "ABC123".into(),
            operation_id: "step-2".into(),
            action: DeviceUiAction::Swipe {
                x1: 10,
                y1: 20,
                x2: 30,
                y2: 40,
                speed: 600,
            },
        };
        assert_eq!(
            prepare_invocation(&swipe, None).unwrap().args,
            vec![
                "-t", "ABC123", "shell", "uitest", "uiInput", "swipe", "10", "20",
                "30", "40", "600",
            ]
        );
        let text = HostCapability::DeviceUiInput {
            device: "ABC123".into(),
            operation_id: "step-3".into(),
            action: DeviceUiAction::Text { text: "安全 input 文本".into() },
        };
        assert_eq!(prepare_invocation(&text, None).unwrap().args[5], "text");
        assert_eq!(prepare_invocation(&text, None).unwrap().args[6], "安全 input 文本");
        for action in [
            DeviceUiAction::Click { x: -1, y: 0 },
            DeviceUiAction::Swipe { x1: 0, y1: 0, x2: 1, y2: 1, speed: 0 },
            DeviceUiAction::Text { text: "bad\ntext".into() },
            DeviceUiAction::Key { name: "back;rm".into() },
        ] {
            assert!(HostCapability::DeviceUiInput {
                device: "ABC123".into(),
                operation_id: "invalid-step".into(),
                action,
            }
            .validate()
            .is_err());
        }
        assert!(HostCapability::DeviceUiInput {
            device: "ABC123".into(),
            operation_id: "bad;step".into(),
            action: DeviceUiAction::Key { name: "back".into() },
        }
        .validate()
        .is_err());
    }

    #[test]
    fn network_condition_uses_bounded_fixed_argv() {
        let read = HostCapability::ReadNetworkCondition {
            device: "ABC123".into(),
            interface: "wlan0".into(),
        };
        assert_eq!(
            prepare_invocation(&read, None).unwrap().args,
            vec!["-t", "ABC123", "shell", "tc", "qdisc", "show", "dev", "wlan0"]
        );
        assert!(read.replay_safe());
        let apply = HostCapability::ConfigureNetworkCondition {
            device: "ABC123".into(),
            interface: "wlan0".into(),
            delay_ms: 100,
            loss_pct: 1,
            bandwidth_kbps: 500,
        };
        assert_eq!(
            prepare_invocation(&apply, None).unwrap().args,
            vec![
                "-t", "ABC123", "shell", "tc", "qdisc", "replace", "dev", "wlan0",
                "root", "netem", "delay", "100ms", "loss", "1%", "rate", "500kbit"
            ]
        );
        assert!(!apply.replay_safe());
        let clear = HostCapability::ConfigureNetworkCondition {
            device: "ABC123".into(),
            interface: "wlan0".into(),
            delay_ms: 0,
            loss_pct: 0,
            bandwidth_kbps: 0,
        };
        assert_eq!(
            prepare_invocation(&clear, None).unwrap().args,
            vec!["-t", "ABC123", "shell", "tc", "qdisc", "del", "dev", "wlan0", "root"]
        );
        assert!(HostCapability::ConfigureNetworkCondition {
            device: "ABC123".into(),
            interface: "wlan0;bad".into(),
            delay_ms: 1,
            loss_pct: 0,
            bandwidth_kbps: 0,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn device_radio_backends_are_fixed_and_compatible() {
        let wifi = HostCapability::SetDeviceRadio {
            device: "ABC123".into(),
            radio: DeviceRadio::Wifi,
            enable: false,
            backend: DeviceRadioBackend::Svc,
        };
        assert_eq!(
            prepare_invocation(&wifi, None).unwrap().args,
            vec!["-t", "ABC123", "shell", "svc", "wifi", "disable"]
        );
        assert!(!wifi.replay_safe());
        let airplane = HostCapability::SetDeviceRadio {
            device: "ABC123".into(),
            radio: DeviceRadio::AirplaneMode,
            enable: true,
            backend: DeviceRadioBackend::GlobalSettings,
        };
        assert_eq!(
            prepare_invocation(&airplane, None).unwrap().args,
            vec![
                "-t", "ABC123", "shell", "settings", "put", "global",
                "airplane_mode_on", "1",
            ]
        );
        assert!(HostCapability::SetDeviceRadio {
            device: "ABC123".into(),
            radio: DeviceRadio::AirplaneMode,
            enable: true,
            backend: DeviceRadioBackend::WpaCli,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn debugger_capabilities_use_typed_fixed_argv() {
        let attach = HostCapability::AttachDeviceDebugger {
            device: "ABC123".into(),
            pid: 4242,
            wait_seconds: 45,
        };
        let attach_invocation = prepare_invocation(&attach, None).unwrap();
        assert_eq!(
            attach_invocation.args,
            vec!["-t", "ABC123", "shell", "debuggerd", "-p", "4242"]
        );
        assert_eq!(attach_invocation.timeout_seconds, 45);
        assert!(!attach.replay_safe());

        let enable = HostCapability::EnableAbilityDebug {
            device: "ABC123".into(),
            bundle: "com.example.app".into(),
        };
        assert_eq!(
            prepare_invocation(&enable, None).unwrap().args,
            vec!["-t", "ABC123", "shell", "aa", "debug", "-b", "com.example.app"]
        );

        let control = HostCapability::ControlDeviceDebugger {
            device: "ABC123".into(),
            pid: 4242,
            action: DeviceDebuggerAction::Backtrace,
        };
        assert_eq!(
            prepare_invocation(&control, None).unwrap().args,
            vec!["-t", "ABC123", "shell", "debuggerd", "-p", "4242", "-c", "bt"]
        );
        assert_eq!(control.capability_id(), "device.debugger.control");
        assert!(!control.replay_safe());

        assert!(HostCapability::AttachDeviceDebugger {
            device: "ABC123".into(),
            pid: 0,
            wait_seconds: 30,
        }
        .validate()
        .is_err());
        assert!(HostCapability::AttachDeviceDebugger {
            device: "ABC123".into(),
            pid: 42,
            wait_seconds: 121,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn emulator_capabilities_use_internal_program_and_fixed_arguments() {
        let instances = HostCapability::QueryEmulator {
            kind: EmulatorQueryKind::Instances,
        };
        assert_eq!(emulator_arguments(&instances).unwrap(), (vec!["-list".into()], 60));
        assert!(instances.replay_safe());

        let create = HostCapability::CreateEmulator {
            name: "Pura 90".into(),
            device_type: "Phone".into(),
            os_version: "HarmonyOS 6.0.0(20)".into(),
            screen_profile: Some("Pura 90".into()),
            memory_gb: Some(8),
            storage_gb: Some(32),
        };
        assert_eq!(
            emulator_arguments(&create).unwrap().0,
            vec![
                "-create",
                "Pura 90",
                "-deviceType",
                "Phone",
                "-osVersion",
                "HarmonyOS 6.0.0(20)",
                "-screenProfile",
                "Pura 90",
                "-memory",
                "8",
                "-storage",
                "32",
            ]
        );
        assert!(!create.replay_safe());
        assert!(HostCapability::StartEmulator { name: "-delete".into() }
            .validate()
            .is_err());
        assert!(HostCapability::CreateEmulator {
            name: "test".into(),
            device_type: "Phone".into(),
            os_version: "HarmonyOS".into(),
            screen_profile: None,
            memory_gb: Some(64),
            storage_gb: None,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn ota_packaging_is_workspace_scoped_and_non_replay_safe() {
        let capability = HostCapability::PackageOta {
            hap_path: "artifacts/app.hap".into(),
            output_path: "release/update.pkg".into(),
            profile_path: Some("signing/profile.json".into()),
        };
        assert!(capability.validate().is_ok());
        assert_eq!(capability.capability_id(), "release.package_ota");
        assert!(!capability.replay_safe());
        assert!(HostCapability::PackageOta {
            hap_path: "artifacts/app.hap".into(),
            output_path: "/tmp/update.pkg".into(),
            profile_path: None,
        }
        .validate()
        .is_err());
        assert!(HostCapability::PackageOta {
            hap_path: "artifacts/app.hap".into(),
            output_path: "release/update.zip".into(),
            profile_path: Some("../profile.json".into()),
        }
        .validate()
        .is_err());

        let root = std::env::temp_dir().join(format!("harmony-ota-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("artifacts")).unwrap();
        std::fs::create_dir_all(root.join("release")).unwrap();
        std::fs::create_dir_all(root.join("signing")).unwrap();
        std::fs::write(root.join("artifacts/app.hap"), b"hap").unwrap();
        std::fs::write(root.join("signing/profile.json"), b"{}").unwrap();
        assert!(resolve_workspace_output(&root, "release/update.pkg", "pkg")
            .unwrap()
            .starts_with(root.canonicalize().unwrap()));
        assert!(resolve_workspace_profile(&root, "signing/profile.json")
            .unwrap()
            .is_file());
        assert!(resolve_workspace_output(&root, "../escape.pkg", "pkg").is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn file_transfer_rejects_external_local_and_ambiguous_device_paths() {
        assert!(HostCapability::SendFile {
            device: "ABC123".into(),
            local_path: "/tmp/secret".into(),
            remote_path: "/data/local/tmp/secret".into(),
        }
        .validate()
        .is_err());
        assert!(HostCapability::ReceiveFile {
            device: "ABC123".into(),
            remote_path: "/data/../secret".into(),
            local_path: "out/secret".into(),
        }
        .validate()
        .is_err());
        assert!(HostCapability::ReceiveFile {
            device: "ABC123".into(),
            remote_path: "relative/file".into(),
            local_path: "out/file".into(),
        }
        .validate()
        .is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let root = std::env::temp_dir().join(format!("harmony-recv-{}", uuid::Uuid::new_v4()));
            let external = std::env::temp_dir().join(format!("harmony-recv-out-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            std::fs::create_dir_all(&external).unwrap();
            symlink(&external, root.join("escape")).unwrap();
            let result = prepare_invocation(
                &HostCapability::ReceiveFile {
                    device: "ABC123".into(),
                    remote_path: "/data/local/tmp/file".into(),
                    local_path: "escape/file".into(),
                },
                Some(&root),
            );
            assert!(result.is_err());
            std::fs::remove_dir_all(root).ok();
            std::fs::remove_dir_all(external).ok();
        }
    }

    #[test]
    fn prepares_fixed_hdc_argv_and_scopes_artifact_to_workspace() {
        let root = std::env::temp_dir().join(format!("harmony-capability-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("out/app.hap"), b"hap").unwrap();
        let invocation = prepare_invocation(&HostCapability::InstallHap {
            device: Some("ABC123".into()), hap_path: "out/app.hap".into(), replace: true,
        }, Some(&root)).unwrap();
        assert_eq!(invocation.program, "hdc");
        assert_eq!(&invocation.args[..4], ["-t", "ABC123", "install", "-r"]);
        assert!(invocation.args[4].ends_with("out/app.hap"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn rejects_symlink_artifact_escape_and_unsafe_ability_identifiers() {
        assert!(HostCapability::StartAbility {
            device: "ABC123".into(), bundle: "com.example.app;bad".into(), ability: "EntryAbility".into(),
        }.validate().is_err());
        #[cfg(unix)] {
            use std::os::unix::fs::symlink;
            let root = std::env::temp_dir().join(format!("harmony-capability-{}", uuid::Uuid::new_v4()));
            let external = std::env::temp_dir().join(format!("harmony-capability-external-{}.hap", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(&external, b"hap").unwrap();
            symlink(&external, root.join("escape.hap")).unwrap();
            let result = prepare_invocation(&HostCapability::InstallHap {
                device: None, hap_path: "escape.hap".into(), replace: false,
            }, Some(&root));
            assert!(result.is_err());
            std::fs::remove_dir_all(root).ok();
            std::fs::remove_file(external).ok();
        }
    }

    #[test]
    fn ability_intent_uses_fixed_argv_and_redacts_uri_from_request_material() {
        let uri = "https://example.test/private/path?token=secret";
        let capability = HostCapability::StartAbilityIntent {
            device: "ABC123".into(),
            bundle: Some("com.example.app".into()),
            ability: None,
            uri: Some(uri.into()),
        };
        let invocation = prepare_invocation(&capability, None).unwrap();
        assert_eq!(
            invocation.args,
            vec![
                "-t",
                "ABC123",
                "shell",
                "aa",
                "start",
                "-b",
                "com.example.app",
                "-D",
                uri,
            ]
        );
        let material = request_material(&capability);
        assert!(!material.contains(uri));
        assert!(audit_subject(&capability)["uri_digest"].is_string());
        assert!(!capability.replay_safe());

        assert!(HostCapability::StartAbilityIntent {
            device: "ABC123".into(),
            bundle: None,
            ability: Some("EntryAbility".into()),
            uri: None,
        }
        .validate()
        .is_err());
        assert!(HostCapability::StartAbilityIntent {
            device: "ABC123".into(),
            bundle: None,
            ability: None,
            uri: Some("https://example.test/\nunsafe".into()),
        }
        .validate()
        .is_err());
    }

    #[test]
    fn screen_recording_capabilities_use_bounded_fixed_argv() {
        let remote = "/data/local/tmp/deveco_agent_screen_record_123.mp4";
        let probe = HostCapability::ProbeScreenRecording { device: "ABC123".into() };
        assert!(probe.replay_safe());
        assert_eq!(
            prepare_invocation(&probe, None).unwrap().args,
            vec!["-t", "ABC123", "shell", "screenrecord", "--help"]
        );

        let start = HostCapability::StartScreenRecording {
            device: "ABC123".into(),
            remote_path: remote.into(),
            max_seconds: 60,
        };
        let invocation = prepare_invocation(&start, None).unwrap();
        assert_eq!(
            invocation.args,
            vec![
                "-t",
                "ABC123",
                "shell",
                "screenrecord",
                "--time-limit",
                "60",
                "--size",
                "1080x1920",
                remote,
            ]
        );
        assert_eq!(invocation.timeout_seconds, 70);
        assert!(!start.replay_safe());
        assert!(HostCapability::StartScreenRecording {
            device: "ABC123".into(),
            remote_path: remote.into(),
            max_seconds: 0,
        }
        .validate()
        .is_err());
        assert!(HostCapability::StartScreenRecording {
            device: "ABC123".into(),
            remote_path: "/data/local/tmp/not_managed.mp4".into(),
            max_seconds: 10,
        }
        .validate()
        .is_err());

        let stop = HostCapability::StopScreenRecording { device: "ABC123".into() };
        assert_eq!(
            prepare_invocation(&stop, None).unwrap().args,
            vec!["-t", "ABC123", "shell", "pkill", "-2", "screenrecord"]
        );
        assert!(!stop.replay_safe());
    }

    #[test]
    fn request_identity_is_bound_to_run_call_and_capability() {
        let mut ctx = crate::agent::exec_ctx::ToolCtx::empty();
        ctx.run_id = "run-1".into();
        ctx.tool_call_id = Some("call-1".into());
        let first_capability = HostCapability::InstallHap {
            device: Some("device-a".into()), hap_path: "out/app.hap".into(), replace: false,
        };
        let first = request_identity(&ctx, &first_capability).unwrap();
        let repeated = request_identity(&ctx, &first_capability).unwrap();
        assert_eq!(first, repeated);
        let other_device = HostCapability::InstallHap {
            device: Some("device-b".into()), hap_path: "out/app.hap".into(), replace: false,
        };
        assert_ne!(first.idempotency_key, request_identity(&ctx, &other_device).unwrap().idempotency_key);
        ctx.tool_call_id = None;
        assert!(request_identity(&ctx, &first_capability).is_err());
    }
}
