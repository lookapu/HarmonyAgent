//! Host Capability Broker 原型（docs/AGENT_EVOLUTION_ROADMAP_2026.md §4 / §5.2）。
//!
//! 把宿主特权操作（hdc 设备管理、签名、部署）建模为**类型化、窄化的能力**，而不是暴露
//! 等价的任意 shell。每个能力经 [`HostCapability::validate`] 拒绝越界/越权参数，并由
//! [`execute_host_capability`] 生成固定 argv、执行及写入运行审计。

use sha2::{Digest, Sha256};
use std::path::Path;
use std::process::Output;

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
    /// 安装构建产物到设备（路径必须位于项目工作树内）。
    InstallHap { device: Option<String>, hap_path: String, replace: bool },
    /// 拉起一个已安装应用的明确 ability。
    StartAbility { device: String, bundle: String, ability: String },
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
            Self::InstallHap { .. } => "deploy.install",
            Self::StartAbility { .. } => "deploy.start_ability",
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
        }
    }

    fn replay_safe(&self) -> bool {
        matches!(self, Self::HdcListTargets)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HostInvocation {
    program: &'static str,
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
        HostCapability::InstallHap { device, hap_path, replace } => format!(
            "{}\0{}\0{replace}", device.as_deref().unwrap_or("").trim(), hap_path.trim(),
        ),
        HostCapability::StartAbility { device, bundle, ability } =>
            format!("{}\0{}\0{}", device.trim(), bundle.trim(), ability.trim()),
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
        HostCapability::Deploy { .. } => {
            return Err("deploy 是组合能力，必须拆分为 install 与 start_ability 执行".into());
        }
    };
    Ok(HostInvocation { program: "hdc", args, timeout_seconds })
}

/// 执行经过类型化校验的宿主能力。调用方负责解释领域输出和完成后验证；本入口
/// 只允许固定程序/argv 模板，并保证成功、非零退出与启动失败都进入同一审计链。
pub async fn execute_host_capability(
    capability: &HostCapability,
    workspace: Option<&Path>,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<Output, String> {
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
    let invocation = match prepare_invocation(capability, workspace) {
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
        ctx, invocation.program, &invocation.args, None, invocation.timeout_seconds, None,
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
    crate::agent::runtime::claim_host_capability(
        &conn,
        &ctx.run_id,
        &ctx.conversation_id,
        &identity.tool_call_id,
        capability_id,
        &identity.request_digest,
        &identity.idempotency_key,
        subject,
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

fn validate_app_identifier(value: &str, label: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 256
        || !value.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '$'))
    {
        return Err(format!("{label} 标识非法"));
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
        HostCapability::InstallHap { device, hap_path, replace } => serde_json::json!({
            "device_digest": device.as_deref().map(short_digest), "artifact": hap_path, "replace": replace,
        }),
        HostCapability::StartAbility { device, bundle, ability } => serde_json::json!({
            "device_digest": short_digest(device), "bundle": bundle, "ability": ability,
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
