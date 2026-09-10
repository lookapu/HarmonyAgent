//! Agent 命令执行沙箱的稳定策略模型、平台原生/OCI 能力探测与进程生命周期。
//!
//! 本模块不会自行切换现有 `run_command` 执行路径。OCI 仍需调用方显式选择；平台
//! 原生后端当前只提供可信 capability 探测。探测或能力校验失败时一律失败关闭，禁止
//! 静默回退宿主执行。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

pub const SANDBOX_SPEC_VERSION: u32 = 1;
pub const SANDBOX_WORKSPACE_PATH: &str = "/workspace";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilesystemPolicy {
    ReadOnly,
    WorkspaceWrite,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "hosts", rename_all = "snake_case")]
pub enum NetworkPolicy {
    None,
    Allowlist(Vec<String>),
    Full,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub cpu_count: u16,
    pub memory_mb: u64,
    pub pids: u32,
    pub writable_tmp_mb: u64,
    pub wall_time_seconds: u64,
    pub output_bytes: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            cpu_count: 2,
            memory_mb: 4_096,
            pids: 256,
            writable_tmp_mb: 1_024,
            wall_time_seconds: 300,
            output_bytes: 2 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxSpec {
    pub version: u32,
    pub workspace: PathBuf,
    pub filesystem: FilesystemPolicy,
    pub network: NetworkPolicy,
    pub limits: ResourceLimits,
    /// 只记录允许注入的变量名。secret 值不得进入 spec、日志或 trajectory。
    pub environment_keys: Vec<String>,
}

impl SandboxSpec {
    pub fn workspace_write(workspace: PathBuf) -> Self {
        Self {
            version: SANDBOX_SPEC_VERSION,
            workspace,
            filesystem: FilesystemPolicy::WorkspaceWrite,
            network: NetworkPolicy::None,
            limits: ResourceLimits::default(),
            environment_keys: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != SANDBOX_SPEC_VERSION {
            return Err(format!(
                "不支持 sandbox spec v{}，当前仅支持 v{}",
                self.version, SANDBOX_SPEC_VERSION
            ));
        }
        if !self.workspace.is_absolute() {
            return Err("sandbox workspace 必须是绝对路径".into());
        }
        if !self.workspace.is_dir() {
            return Err(format!(
                "sandbox workspace 不是可访问目录：{}",
                self.workspace.display()
            ));
        }
        if self.workspace.to_string_lossy().contains(',') {
            return Err("OCI --mount 暂不支持路径中含逗号的 workspace".into());
        }
        if self.limits.cpu_count == 0
            || self.limits.cpu_count > 64
            || self.limits.memory_mb < 128
            || self.limits.memory_mb > 65_536
            || self.limits.pids == 0
            || self.limits.pids > 4_096
            || self.limits.writable_tmp_mb < 16
            || self.limits.writable_tmp_mb > 16_384
            || self.limits.wall_time_seconds == 0
            || self.limits.wall_time_seconds > 3_600
            || self.limits.output_bytes == 0
            || self.limits.output_bytes > 64 * 1024 * 1024
        {
            return Err("sandbox 资源限制超出安全范围".into());
        }
        for key in &self.environment_keys {
            if !valid_environment_key(key) {
                return Err(format!("非法环境变量名：{key}"));
            }
            if sensitive_environment_key(key) {
                return Err(format!(
                    "sandbox 禁止直接注入敏感环境变量 {key}；请使用 Host Capability Broker"
                ));
            }
        }
        if let NetworkPolicy::Allowlist(hosts) = &self.network {
            if hosts.is_empty() {
                return Err("network allowlist 不能为空；无需网络请使用 none".into());
            }
            for host in hosts {
                if !valid_allowlist_host(host) {
                    return Err(format!("非法网络 allowlist host：{host}"));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxCapabilities {
    pub backend: String,
    pub available: bool,
    pub os_level_isolation: bool,
    pub filesystem_read_only: bool,
    pub workspace_write: bool,
    pub network_none: bool,
    pub network_allowlist: bool,
    pub resource_limits: bool,
    pub reason: Option<String>,
}

/// 平台原生轻量隔离候选。这里描述的是可被探测和审计的实现，不等同于已经可用；
/// [`probe_native_backend`] 只有在当前平台与运行时探测都匹配时才会置 `available=true`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeSandboxKind {
    MacosSandboxExec,
    LinuxBubblewrap,
    WindowsAppContainer,
}

impl NativeSandboxKind {
    pub fn backend_name(self) -> &'static str {
        match self {
            Self::MacosSandboxExec => "macos-sandbox-exec",
            Self::LinuxBubblewrap => "linux-bubblewrap",
            Self::WindowsAppContainer => "windows-app-container",
        }
    }

    fn probe_program(self) -> Option<(&'static str, &'static [&'static str])> {
        match self {
            // 不只检查可执行文件是否存在，而是运行一个无副作用的最小隔离域；否则
            // Linux user namespace 被系统策略禁用时会产生假阳性。
            Self::MacosSandboxExec => Some((
                "sandbox-exec",
                &["-p", "(version 1) (allow default)", "/usr/bin/true"],
            )),
            Self::LinuxBubblewrap => Some((
                "bwrap",
                &[
                    "--die-with-parent",
                    "--unshare-all",
                    "--ro-bind",
                    "/",
                    "/",
                    "--",
                    "/bin/true",
                ],
            )),
            // AppContainer 需要 token/profile/ACL 生命周期实现，不能用“系统是 Windows”
            // 冒充后端已经可用。
            Self::WindowsAppContainer => None,
        }
    }

    pub fn declared_capabilities(self) -> SandboxCapabilities {
        let (os_level_isolation, filesystem_read_only, workspace_write, network_none) = match self {
            Self::MacosSandboxExec | Self::LinuxBubblewrap => (true, true, true, true),
            Self::WindowsAppContainer => (true, true, true, true),
        };
        SandboxCapabilities {
            backend: self.backend_name().into(),
            available: false,
            os_level_isolation,
            filesystem_read_only,
            workspace_write,
            network_none,
            // 三个平台都不能仅凭基础原生机制可靠实现域名 allowlist。
            network_allowlist: false,
            // 当前统一 wall/output 上限由父进程治理；CPU/memory/pids 尚未在原生后端
            // 全量强制，因此不能把 resource_limits 标为 true。
            resource_limits: false,
            reason: Some("尚未执行平台原生沙箱能力探测".into()),
        }
    }
}

/// 返回当前编译目标对应的平台原生后端候选。未知平台显式返回 `None`，调用方必须
/// 保持未隔离状态或失败关闭，不能退化成一个名称看似安全的宿主进程。
pub fn native_sandbox_for_current_platform() -> Option<NativeSandboxKind> {
    #[cfg(target_os = "macos")]
    {
        return Some(NativeSandboxKind::MacosSandboxExec);
    }
    #[cfg(target_os = "linux")]
    {
        return Some(NativeSandboxKind::LinuxBubblewrap);
    }
    #[cfg(target_os = "windows")]
    {
        return Some(NativeSandboxKind::WindowsAppContainer);
    }
    #[allow(unreachable_code)]
    None
}

/// 探测当前平台的原生隔离能力。探测只回答“后续是否可以选择该后端”，不执行用户
/// 命令，也不下载二进制或镜像。探测程序存在但返回非零时仍视为不可用。
pub async fn probe_native_backend() -> SandboxCapabilities {
    let Some(kind) = native_sandbox_for_current_platform() else {
        return SandboxCapabilities {
            backend: "native-unsupported".into(),
            available: false,
            os_level_isolation: false,
            filesystem_read_only: false,
            workspace_write: false,
            network_none: false,
            network_allowlist: false,
            resource_limits: false,
            reason: Some("当前平台没有已登记的平台原生沙箱后端".into()),
        };
    };
    probe_native_kind(kind).await
}

async fn probe_native_kind(kind: NativeSandboxKind) -> SandboxCapabilities {
    let mut capabilities = kind.declared_capabilities();
    let Some((program, probe_args)) = kind.probe_program() else {
        capabilities.reason = Some(
            "Windows AppContainer backend 尚未实现 token/profile/ACL 生命周期，拒绝标记为可用"
                .into(),
        );
        return capabilities;
    };
    let args = probe_args
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    let mut command = match crate::utils::process::command(program, &args) {
        Ok(command) => command,
        Err(error) => {
            capabilities.reason = Some(format!("无法构造 {program} 探测命令：{error}"));
            return capabilities;
        }
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = match tokio::time::timeout(Duration::from_secs(3), command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            capabilities.reason = Some(format!("无法启动 {program}：{error}"));
            return capabilities;
        }
        Err(_) => {
            capabilities.reason = Some(format!("{program} 能力探测超时（3s）"));
            return capabilities;
        }
    };
    if output.status.success() {
        capabilities.available = true;
        capabilities.reason = first_summary_line(&output.stdout)
            .or_else(|| first_summary_line(&output.stderr))
            .map(|line| format!("平台原生沙箱探测通过：{line}"))
            .or_else(|| Some("平台原生沙箱探测通过".into()));
    } else {
        let detail = first_summary_line(&output.stderr)
            .or_else(|| first_summary_line(&output.stdout))
            .unwrap_or_else(|| format!("退出码 {}", output.status.code().unwrap_or(-1)));
        capabilities.reason = Some(format!("{program} 不可用：{detail}"));
    }
    capabilities
}

/// 所有沙箱后端必须提供的同步、可审计契约。运行时探测和执行由具体后端的
/// async 方法承担，避免为了 async trait 引入额外运行时依赖。
pub trait SandboxBackend {
    fn declared_capabilities(&self) -> SandboxCapabilities;
    fn build_run_command(
        &self,
        spec: &SandboxSpec,
        execution_id: &str,
        image: &str,
        command: &[String],
    ) -> Result<OciRunCommand, String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OciEngine {
    Docker,
    Podman,
}

impl OciEngine {
    pub fn program(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }

    pub fn declared_capabilities(self) -> SandboxCapabilities {
        SandboxCapabilities {
            backend: self.program().into(),
            // available 只能由运行时探测填写；静态声明不能冒充已安装可用。
            available: false,
            os_level_isolation: true,
            filesystem_read_only: true,
            workspace_write: true,
            network_none: true,
            // 单纯 `docker run`/`podman run` 不能可靠实现按域名 allowlist。
            network_allowlist: false,
            resource_limits: true,
            reason: Some("尚未执行运行时能力探测".into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OciBackend {
    pub engine: OciEngine,
}

impl OciBackend {
    pub fn new(engine: OciEngine) -> Self {
        Self { engine }
    }

    /// 探测 CLI 与后端服务是否都可用。`docker version` 会同时验证 daemon，
    /// Podman 的无 daemon 模式也能通过相同探测确认运行时完整可用。
    pub async fn probe(&self) -> SandboxCapabilities {
        probe_oci_engine(self.engine).await
    }

    /// 启动一个前台 OCI 执行域。所有参数以 argv 传递，不经过 shell。
    /// 超时或用户取消时会同时终止 CLI 进程树并强制删除命名容器。
    pub async fn run(
        &self,
        spec: &SandboxSpec,
        execution_id: &str,
        image: &str,
        command: &[String],
        ctx: &crate::agent::exec_ctx::ToolCtx,
    ) -> Result<SandboxRunResult, String> {
        let built = self.build_run_command(spec, execution_id, image, command)?;
        let capabilities = self.probe().await;
        if !capabilities.available {
            return Err(format!(
                "sandbox_unavailable: {}",
                capabilities
                    .reason
                    .unwrap_or_else(|| format!("{} 不可用", self.engine.program()))
            ));
        }

        ctx.record_run_event(
            "sandbox_started",
            serde_json::json!({
                "backend": self.engine.program(),
                "execution_id": execution_id,
                "spec_version": spec.version,
                "filesystem": spec.filesystem,
                "network": spec.network,
                "limits": spec.limits,
            }),
        );

        let started = Instant::now();
        let output = crate::agent::exec_ctx::run_cmd_streaming(
            ctx,
            &built.program,
            &built.args,
            None,
            spec.limits.wall_time_seconds,
            None,
        )
        .await;

        let (status, exit_code, stdout, stderr, forced_cleanup) = match output {
            Ok(output) => {
                let status = if output.status.success() {
                    SandboxRunStatus::Succeeded
                } else {
                    SandboxRunStatus::Failed
                };
                (
                    status,
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout).into_owned(),
                    String::from_utf8_lossy(&output.stderr).into_owned(),
                    false,
                )
            }
            Err(error) => {
                let status = if error.contains("用户已停止") {
                    SandboxRunStatus::Cancelled
                } else if error.contains("命令超时") {
                    SandboxRunStatus::TimedOut
                } else {
                    SandboxRunStatus::Failed
                };
                cleanup_container(self.engine, execution_id).await;
                (status, None, String::new(), error, true)
            }
        };
        let (stdout, stderr, output_truncated) =
            bound_output(stdout, stderr, spec.limits.output_bytes as usize);
        let result = SandboxRunResult {
            backend: self.engine.program().into(),
            execution_id: execution_id.into(),
            status,
            exit_code,
            stdout,
            stderr,
            duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            output_truncated,
            forced_cleanup,
        };
        ctx.record_run_event(
            "sandbox_finished",
            serde_json::json!({
                "backend": result.backend,
                "execution_id": result.execution_id,
                "status": result.status,
                "exit_code": result.exit_code,
                "duration_ms": result.duration_ms,
                "output_truncated": result.output_truncated,
                "forced_cleanup": result.forced_cleanup,
            }),
        );
        Ok(result)
    }
}

impl SandboxBackend for OciBackend {
    fn declared_capabilities(&self) -> SandboxCapabilities {
        self.engine.declared_capabilities()
    }

    fn build_run_command(
        &self,
        spec: &SandboxSpec,
        execution_id: &str,
        image: &str,
        command: &[String],
    ) -> Result<OciRunCommand, String> {
        build_oci_run_command(self.engine, spec, execution_id, image, command)
    }
}

/// 运行命令时的执行目标：要么走 OCI 隔离，要么显式退回宿主直跑。
/// 宿主直跑不是安全边界，调用方必须通过 [`SandboxExecutionTarget::host_direct_risk_note`] 显式标注。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxExecutionTarget {
    Oci(OciBackend),
    HostDirect,
}

impl SandboxExecutionTarget {
    pub fn backend_name(&self) -> &'static str {
        match self {
            Self::Oci(backend) => backend.engine.program(),
            Self::HostDirect => "host-direct",
        }
    }

    pub fn is_isolated(&self) -> bool {
        matches!(self, Self::Oci(_))
    }

    pub fn host_direct_risk_note(&self) -> Option<&'static str> {
        match self {
            Self::HostDirect => Some(host_direct_risk_note()),
            Self::Oci(_) => None,
        }
    }
}

/// 宿主直跑（未隔离）时的风险标注，供 `run_command` 等直接宿主执行路径持续显示（路线 5.2.3）。
pub fn host_direct_risk_note() -> &'static str {
    "未受沙箱隔离：命令在宿主用户权限下执行，可读取工作区外文件并联网"
}

/// 后端偏好：显式声明要哪种隔离。缺省为宿主直跑（显式兼容模式，非安全默认）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxBackendPreference {
    HostDirect,
    Oci(OciEngine),
}

/// 依据后端偏好与运行时探测结果选择执行目标，并 fail-closed：
/// 请求 OCI 但运行时不可用时返回错误，绝不静默回退宿主执行。
pub fn select_sandbox_target(
    preference: SandboxBackendPreference,
    probe: &SandboxCapabilities,
) -> Result<SandboxExecutionTarget, String> {
    match preference {
        SandboxBackendPreference::HostDirect => Ok(SandboxExecutionTarget::HostDirect),
        SandboxBackendPreference::Oci(engine) => {
            if probe.available && probe.backend == engine.program() {
                Ok(SandboxExecutionTarget::Oci(OciBackend::new(engine)))
            } else {
                Err(format!(
                    "sandbox_unavailable: 请求了 {} 沙箱但运行时不可用（{}）；已失败关闭，未回退宿主执行",
                    engine.program(),
                    probe.reason.as_deref().unwrap_or("未知原因"),
                ))
            }
        }
    }
}

/// 沙箱运行配置：从环境变量读取。缺省宿主直跑（显式兼容模式，非安全默认）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SandboxConfig {
    /// None = 宿主直跑（缺省）；Some = 显式请求的 OCI 引擎。
    pub backend: Option<OciEngine>,
    /// OCI 后端所需镜像（固定 digest 形式）；未配置时 OCI 请求失败关闭。
    pub image: Option<String>,
}

/// 从 `HARMONY_SANDBOX_BACKEND`（oci-docker|oci-podman）与 `HARMONY_SANDBOX_IMAGE` 读取配置。
pub fn sandbox_config_from_env() -> SandboxConfig {
    let backend = match std::env::var("HARMONY_SANDBOX_BACKEND").as_deref() {
        Ok("oci-docker") => Some(OciEngine::Docker),
        Ok("oci-podman") => Some(OciEngine::Podman),
        _ => None,
    };
    let image = std::env::var("HARMONY_SANDBOX_IMAGE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    SandboxConfig { backend, image }
}

/// 把 sandbox 配置解析为执行目标：缺省宿主直跑；请求 OCI 但缺镜像或运行时不可用时失败关闭。
pub fn resolve_sandbox_target(
    config: &SandboxConfig,
    probe: &SandboxCapabilities,
) -> Result<SandboxExecutionTarget, String> {
    match config.backend {
        None => Ok(SandboxExecutionTarget::HostDirect),
        Some(engine) => {
            if config.image.is_none() {
                return Err(format!(
                    "sandbox 镜像未配置：请求了 {} 沙箱但未设置 HARMONY_SANDBOX_IMAGE；已失败关闭，未回退宿主执行",
                    engine.program()
                ));
            }
            select_sandbox_target(SandboxBackendPreference::Oci(engine), probe)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxRunStatus {
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxRunResult {
    pub backend: String,
    pub execution_id: String,
    pub status: SandboxRunStatus,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub output_truncated: bool,
    /// 是否因超时、取消或启动后异常而额外执行了 `rm --force`。
    /// 正常结束仍由 OCI `--rm` 自动回收，不计入此字段。
    pub forced_cleanup: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OciRunCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// 并行探测所有内置 OCI 后端。返回顺序稳定，便于 UI 和诊断报告展示。
pub async fn probe_oci_backends() -> Vec<SandboxCapabilities> {
    let (docker, podman) = tokio::join!(
        probe_oci_engine(OciEngine::Docker),
        probe_oci_engine(OciEngine::Podman)
    );
    vec![docker, podman]
}

/// 并行生成完整沙箱能力清单。平台原生候选始终排在 OCI 之前，便于桌面设置页优先
/// 展示零镜像路径；`available=false` 的条目仍保留原因，不能从诊断结果中静默消失。
#[tauri::command]
pub async fn probe_sandbox_backends() -> Vec<SandboxCapabilities> {
    let (native, oci) = tokio::join!(probe_native_backend(), probe_oci_backends());
    let mut capabilities = Vec::with_capacity(1 + oci.len());
    capabilities.push(native);
    capabilities.extend(oci);
    capabilities
}

async fn probe_oci_engine(engine: OciEngine) -> SandboxCapabilities {
    let mut capabilities = engine.declared_capabilities();
    let args = vec!["version".to_string()];
    let mut command = match crate::utils::process::command(engine.program(), &args) {
        Ok(command) => command,
        Err(error) => {
            capabilities.reason = Some(error);
            return capabilities;
        }
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = match tokio::time::timeout(Duration::from_secs(5), command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            capabilities.reason = Some(format!("无法启动 {}：{error}", engine.program()));
            return capabilities;
        }
        Err(_) => {
            capabilities.reason = Some(format!("{} 运行时探测超时（5s）", engine.program()));
            return capabilities;
        }
    };
    if output.status.success() {
        capabilities.available = true;
        capabilities.reason = first_summary_line(&output.stdout)
            .map(|line| format!("运行时探测通过：{line}"))
            .or_else(|| Some("运行时探测通过".into()));
    } else {
        let detail = first_summary_line(&output.stderr)
            .or_else(|| first_summary_line(&output.stdout))
            .unwrap_or_else(|| format!("退出码 {}", output.status.code().unwrap_or(-1)));
        capabilities.reason = Some(format!("{} 后端不可用：{detail}", engine.program()));
    }
    capabilities
}

fn first_summary_line(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(160).collect())
}

async fn cleanup_container(engine: OciEngine, execution_id: &str) {
    let Ok(name) = normalize_container_name(execution_id) else {
        return;
    };
    let args = vec!["rm".into(), "--force".into(), name];
    let Ok(mut command) = crate::utils::process::command(engine.program(), &args) else {
        return;
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = tokio::time::timeout(Duration::from_secs(5), command.status()).await;
}

fn bound_output(stdout: String, stderr: String, limit: usize) -> (String, String, bool) {
    if stdout.len().saturating_add(stderr.len()) <= limit {
        return (stdout, stderr, false);
    }
    // stderr 通常包含失败结论，先为它保留一半预算，剩余预算在两个流间动态让渡。
    let stderr_budget = stderr.len().min(limit / 2);
    let stdout_budget = stdout.len().min(limit.saturating_sub(stderr_budget));
    let stderr_budget = stderr.len().min(limit.saturating_sub(stdout_budget));
    (
        tail_with_marker(&stdout, stdout_budget),
        tail_with_marker(&stderr, stderr_budget),
        true,
    )
}

fn tail_with_marker(value: &str, budget: usize) -> String {
    if value.len() <= budget {
        return value.into();
    }
    if budget == 0 {
        return String::new();
    }
    const MARKER: &str = "[前部输出已截断]\n";
    if budget <= MARKER.len() {
        let mut end = budget;
        while end > 0 && !MARKER.is_char_boundary(end) {
            end -= 1;
        }
        return MARKER[..end].into();
    }
    let keep = budget - MARKER.len();
    let mut start = value.len().saturating_sub(keep);
    while start < value.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    format!("{MARKER}{}", &value[start..])
}

/// 构造无 shell 插值的 OCI argv。调用方必须把 program/args 直接交给 Command，
/// 不得重新拼成 `sh -c`，否则会破坏这里建立的参数边界。
pub fn build_oci_run_command(
    engine: OciEngine,
    spec: &SandboxSpec,
    container_name: &str,
    image: &str,
    command: &[String],
) -> Result<OciRunCommand, String> {
    spec.validate()?;
    let name = normalize_container_name(container_name)?;
    if !image.contains("@sha256:") {
        return Err("OCI sandbox 镜像必须使用 sha256 digest 固定，不能使用浮动 tag".into());
    }
    if command.is_empty() || command[0].trim().is_empty() {
        return Err("OCI sandbox command 不能为空".into());
    }
    if matches!(spec.network, NetworkPolicy::Allowlist(_)) {
        return Err("当前 OCI backend 尚不能强制域名 allowlist；拒绝降级为 full network".into());
    }

    let mount_mode = match spec.filesystem {
        FilesystemPolicy::ReadOnly => "readonly",
        FilesystemPolicy::WorkspaceWrite => "rw",
    };
    let source = canonical_workspace(&spec.workspace)?;
    let mut args = vec![
        "run".into(),
        "--rm".into(),
        "--name".into(),
        name,
        "--read-only".into(),
        "--user=65532:65532".into(),
        "--cap-drop=ALL".into(),
        "--security-opt=no-new-privileges".into(),
        format!("--cpus={}", spec.limits.cpu_count),
        format!("--memory={}m", spec.limits.memory_mb),
        format!("--pids-limit={}", spec.limits.pids),
        "--tmpfs".into(),
        format!(
            "/tmp:rw,noexec,nosuid,nodev,size={}m",
            spec.limits.writable_tmp_mb
        ),
        "--mount".into(),
        format!(
            "type=bind,source={},target={SANDBOX_WORKSPACE_PATH},{mount_mode}",
            source.display()
        ),
        "--workdir".into(),
        SANDBOX_WORKSPACE_PATH.into(),
    ];
    if matches!(spec.network, NetworkPolicy::None) {
        args.push("--network=none".into());
    }
    for key in &spec.environment_keys {
        // 只转发经过 validate 的非敏感变量名，值由 OCI CLI 从调用环境读取。
        args.push("--env".into());
        args.push(key.clone());
    }
    args.push(image.into());
    args.extend(command.iter().cloned());
    Ok(OciRunCommand {
        program: engine.program().into(),
        args,
    })
}

fn canonical_workspace(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|error| format!("无法规范化 sandbox workspace：{error}"))
}

fn normalize_container_name(value: &str) -> Result<String, String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() || value.len() > 63 {
        return Err("sandbox container name 长度必须为 1..=63".into());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err("sandbox container name 只能包含字母、数字、点、下划线和连字符".into());
    }
    Ok(value)
}

fn valid_environment_key(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn sensitive_environment_key(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    [
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "API_KEY",
        "PRIVATE_KEY",
        "CREDENTIAL",
        "SSH_AUTH_SOCK",
        "AWS_",
        "AZURE_",
        "GOOGLE_APPLICATION_CREDENTIALS",
    ]
    .iter()
    .any(|marker| upper.contains(marker))
}

fn valid_allowlist_host(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && !value.contains("://")
        && !value.contains('/')
        && !value.contains('*')
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | ':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("deveco-sandbox-spec-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn digest_image() -> &'static str {
        "example.invalid/harmony-agent-eval@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }

    fn available_probe(engine: OciEngine) -> SandboxCapabilities {
        SandboxCapabilities {
            backend: engine.program().into(),
            available: true,
            os_level_isolation: true,
            filesystem_read_only: true,
            workspace_write: true,
            network_none: true,
            network_allowlist: false,
            resource_limits: true,
            reason: Some("运行时探测通过".into()),
        }
    }

    #[test]
    fn native_backend_declaration_never_claims_unprobed_availability() {
        for kind in [
            NativeSandboxKind::MacosSandboxExec,
            NativeSandboxKind::LinuxBubblewrap,
            NativeSandboxKind::WindowsAppContainer,
        ] {
            let capabilities = kind.declared_capabilities();
            assert_eq!(capabilities.backend, kind.backend_name());
            assert!(!capabilities.available);
            assert!(capabilities.os_level_isolation);
            assert!(capabilities.filesystem_read_only);
            assert!(capabilities.workspace_write);
            assert!(capabilities.network_none);
            assert!(!capabilities.network_allowlist);
            assert!(!capabilities.resource_limits);
            assert!(capabilities.reason.is_some());
        }
    }

    #[test]
    fn current_platform_selects_only_its_native_backend() {
        let selected = native_sandbox_for_current_platform();
        #[cfg(target_os = "macos")]
        assert_eq!(selected, Some(NativeSandboxKind::MacosSandboxExec));
        #[cfg(target_os = "linux")]
        assert_eq!(selected, Some(NativeSandboxKind::LinuxBubblewrap));
        #[cfg(target_os = "windows")]
        assert_eq!(selected, Some(NativeSandboxKind::WindowsAppContainer));
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        assert_eq!(selected, None);
    }

    #[tokio::test]
    async fn native_probe_reports_the_current_backend_without_overclaiming_limits() {
        let capabilities = probe_native_backend().await;
        let expected = native_sandbox_for_current_platform()
            .map(NativeSandboxKind::backend_name)
            .unwrap_or("native-unsupported");
        assert_eq!(capabilities.backend, expected);
        assert!(!capabilities.network_allowlist);
        assert!(!capabilities.resource_limits);
        assert!(capabilities.reason.is_some());
        if capabilities.available {
            assert!(capabilities.os_level_isolation);
            assert!(capabilities.network_none);
            assert!(capabilities.workspace_write);
        }
        #[cfg(target_os = "windows")]
        assert!(!capabilities.available);
    }

    #[test]
    fn select_target_prefers_host_direct_when_requested_and_flags_risk() {
        let target = select_sandbox_target(
            SandboxBackendPreference::HostDirect,
            &available_probe(OciEngine::Docker),
        )
        .unwrap();
        assert_eq!(target, SandboxExecutionTarget::HostDirect);
        assert!(!target.is_isolated());
        assert!(target.host_direct_risk_note().is_some());
    }

    #[test]
    fn select_target_uses_oci_when_available() {
        let target = select_sandbox_target(
            SandboxBackendPreference::Oci(OciEngine::Docker),
            &available_probe(OciEngine::Docker),
        )
        .unwrap();
        assert!(target.is_isolated());
        assert_eq!(target.backend_name(), "docker");
        assert!(target.host_direct_risk_note().is_none());
    }

    #[test]
    fn select_target_fails_closed_when_oci_unavailable_or_mismatched() {
        let unavailable = SandboxCapabilities {
            available: false,
            ..OciEngine::Docker.declared_capabilities()
        };
        let err = select_sandbox_target(
            SandboxBackendPreference::Oci(OciEngine::Docker),
            &unavailable,
        )
        .unwrap_err();
        assert!(err.contains("sandbox_unavailable"), "{err}");
        assert!(err.contains("未回退宿主执行"), "{err}");

        // 探测到 Podman 但请求 Docker：引擎不匹配也必须失败关闭。
        let mismatched = select_sandbox_target(
            SandboxBackendPreference::Oci(OciEngine::Docker),
            &available_probe(OciEngine::Podman),
        )
        .unwrap_err();
        assert!(mismatched.contains("sandbox_unavailable"), "{mismatched}");
    }

    #[test]
    fn resolve_sandbox_target_defaults_to_host_direct_and_fails_closed_on_oci() {
        // 缺省：无后端 → 宿主直跑
        let target = resolve_sandbox_target(
            &SandboxConfig::default(),
            &available_probe(OciEngine::Docker),
        )
        .unwrap();
        assert_eq!(target, SandboxExecutionTarget::HostDirect);

        // 请求 OCI 但缺镜像 → 失败关闭
        let err = resolve_sandbox_target(
            &SandboxConfig {
                backend: Some(OciEngine::Docker),
                image: None,
            },
            &available_probe(OciEngine::Docker),
        )
        .unwrap_err();
        assert!(err.contains("镜像未配置"), "{err}");

        // 请求 OCI + 镜像 + 运行时可用 → Oci
        let target = resolve_sandbox_target(
            &SandboxConfig {
                backend: Some(OciEngine::Docker),
                image: Some("img@sha256:".into()),
            },
            &available_probe(OciEngine::Docker),
        )
        .unwrap();
        assert!(target.is_isolated());
    }

    #[test]
    fn workspace_write_defaults_to_no_network_and_bounded_resources() {
        let workspace = temp_workspace();
        let spec = SandboxSpec::workspace_write(workspace.clone());
        assert_eq!(spec.network, NetworkPolicy::None);
        assert_eq!(spec.filesystem, FilesystemPolicy::WorkspaceWrite);
        assert!(spec.limits.memory_mb > 0);
        assert!(spec.validate().is_ok());
        std::fs::remove_dir_all(workspace).ok();
    }

    #[test]
    fn oci_argv_enforces_minimum_boundary_without_shell_joining() {
        let workspace = temp_workspace();
        let spec = SandboxSpec::workspace_write(workspace.clone());
        let built = build_oci_run_command(
            OciEngine::Docker,
            &spec,
            "run-123",
            digest_image(),
            &["cargo".into(), "test".into()],
        )
        .unwrap();
        assert_eq!(built.program, "docker");
        assert!(built.args.contains(&"--read-only".to_string()));
        assert!(built.args.contains(&"--user=65532:65532".to_string()));
        assert!(built.args.contains(&"--cap-drop=ALL".to_string()));
        assert!(built
            .args
            .contains(&"--security-opt=no-new-privileges".to_string()));
        assert!(built.args.contains(&"--network=none".to_string()));
        assert!(built
            .args
            .iter()
            .any(|arg| arg.contains("target=/workspace,rw")));
        assert_eq!(built.args.last().map(String::as_str), Some("test"));
        std::fs::remove_dir_all(workspace).ok();
    }

    #[test]
    fn oci_builder_fails_closed_for_unsupported_allowlist_and_floating_image() {
        let workspace = temp_workspace();
        let mut spec = SandboxSpec::workspace_write(workspace.clone());
        spec.network = NetworkPolicy::Allowlist(vec!["registry.npmjs.org".into()]);
        let allowlist = build_oci_run_command(
            OciEngine::Podman,
            &spec,
            "run-allowlist",
            digest_image(),
            &["npm".into(), "test".into()],
        )
        .unwrap_err();
        assert!(allowlist.contains("拒绝降级"));

        spec.network = NetworkPolicy::None;
        let floating = build_oci_run_command(
            OciEngine::Podman,
            &spec,
            "run-floating",
            "node:22",
            &["npm".into(), "test".into()],
        )
        .unwrap_err();
        assert!(floating.contains("sha256 digest"));
        std::fs::remove_dir_all(workspace).ok();
    }

    #[test]
    fn spec_rejects_secret_environment_forwarding() {
        let workspace = temp_workspace();
        let mut spec = SandboxSpec::workspace_write(workspace.clone());
        spec.environment_keys = vec!["OPENAI_API_KEY".into()];
        let error = spec.validate().unwrap_err();
        assert!(error.contains("Host Capability Broker"));
        std::fs::remove_dir_all(workspace).ok();
    }

    #[test]
    fn backend_contract_builds_the_same_fail_closed_argv() {
        let workspace = temp_workspace();
        let spec = SandboxSpec::workspace_write(workspace.clone());
        let backend = OciBackend::new(OciEngine::Podman);
        assert!(!backend.declared_capabilities().available);
        let built = backend
            .build_run_command(
                &spec,
                "contract-1",
                digest_image(),
                &["cargo".into(), "check".into()],
            )
            .unwrap();
        assert_eq!(built.program, "podman");
        assert!(built.args.contains(&"--network=none".into()));
        std::fs::remove_dir_all(workspace).ok();
    }

    #[test]
    fn bounded_output_keeps_valid_utf8_within_byte_budget() {
        let stdout = "前缀".repeat(100);
        let stderr = "错误".repeat(100);
        let (stdout, stderr, truncated) = bound_output(stdout, stderr, 101);
        assert!(truncated);
        assert!(stdout.len() + stderr.len() <= 101);
        assert!(stdout.is_char_boundary(stdout.len()));
        assert!(stderr.is_char_boundary(stderr.len()));
        assert!(stdout.contains("截断") || stderr.contains("截断"));
    }
}
