//! Agent 命令执行沙箱的稳定策略模型与 OCI 启动参数构造。
//!
//! 本模块目前只建立能力契约和可测试的 OCI argv，不会自行切换现有
//! `run_command` 执行路径。接入前必须完成进程生命周期、日志、取消、
//! artifact 导出和对抗测试，禁止在能力不足时静默回退宿主执行。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
            || self.limits.memory_mb < 128
            || self.limits.pids == 0
            || self.limits.writable_tmp_mb < 16
            || self.limits.wall_time_seconds == 0
            || self.limits.output_bytes == 0
        {
            return Err("sandbox 资源限制必须为有效的非零安全值".into());
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OciRunCommand {
    pub program: String,
    pub args: Vec<String>,
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
}
