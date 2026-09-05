//! Host Capability Broker 原型（docs/AGENT_EVOLUTION_ROADMAP_2026.md §4 / §5.2）。
//!
//! 把宿主特权操作（hdc 设备管理、签名、部署）建模为**类型化、窄化的能力**，而不是暴露
//! 等价的任意 shell。每个能力经 [`HostCapability::validate`] 拒绝越界/越权参数；真实执行
//! 待接入现有 `device_tools`/`build_tools` 时按能力 id 路由到对应窄接口。

use std::path::Path;

/// 宿主特权能力的窄化集合。v0 覆盖 hdc 与 deploy；签名与真机操作待接线。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostCapability {
    /// 无线连接设备（target 为 IP:port 或设备序列号）。
    HdcConnect { target: String },
    /// 断开设备连接。
    HdcDisconnect { target: String },
    /// 列出在线设备。
    HdcListTargets,
    /// 安装构建产物到设备（路径必须位于项目工作树内）。
    InstallHap { device: Option<String>, hap_path: String },
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
            Self::InstallHap { .. } => "deploy.install",
            Self::Deploy { .. } => "deploy",
        }
    }

    /// 校验能力参数；任何越界/越权都返回错误。这是 broker 的安全边界，不能放宽。
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::HdcConnect { target } | Self::HdcDisconnect { target } => {
                validate_device_target(target)
            }
            Self::HdcListTargets => Ok(()),
            Self::InstallHap { device, hap_path } | Self::Deploy { device, hap_path } => {
                if let Some(device) = device {
                    validate_device_target(device)?;
                }
                validate_hap_path(hap_path)
            }
        }
    }
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
    let p = Path::new(path.trim());
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
        assert!(HostCapability::InstallHap { device: None, hap_path: "app.bin".into() }.validate().is_err());
    }

    #[test]
    fn capability_ids_are_stable_for_audit() {
        assert_eq!(HostCapability::HdcConnect { target: "t".into() }.capability_id(), "hdc.connect");
        assert_eq!(HostCapability::Deploy { device: None, hap_path: "a.hap".into() }.capability_id(), "deploy");
    }
}
