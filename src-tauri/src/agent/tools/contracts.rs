//! 工具执行契约：把副作用、恢复与自动重试策略收敛为单一机器可读真源。

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    Read,
    Write,
    Destructive,
}

impl EffectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Destructive => "destructive",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryPolicy {
    Replay,
    Verify,
    Manual,
}

impl RecoveryPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Replay => "replay",
            Self::Verify => "verify",
            Self::Manual => "manual",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolContract {
    pub effect: EffectKind,
    pub recovery: RecoveryPolicy,
    pub retry_safe: bool,
}

const MANUAL_RECOVERY: &[&str] = &[
    "delete_file",
    "run_command",
    "git_push",
    "git_commit",
    "git_merge",
    "git_rebase",
    "git_restore",
    "git_stash",
    "deploy",
    "deploy_all",
    "install_app",
    "uninstall_app",
    "secret_store",
    "secret_delete",
    "create_emulator",
];

/// 只从注册表的结构化约定派生契约。未知/MCP 工具默认按写入处理，禁止自动重放。
pub fn contract(tool: &str) -> ToolContract {
    if MANUAL_RECOVERY.contains(&tool) {
        return ToolContract {
            effect: EffectKind::Destructive,
            recovery: RecoveryPolicy::Manual,
            retry_safe: false,
        };
    }
    let declared_read_only = !tool.starts_with("mcp__")
        && super::TOOL_SPECS
            .iter()
            .find(|spec| spec.name == tool)
            .and_then(|spec| spec.desc.split("副作用：").nth(1))
            .map(str::trim_start)
            .is_some_and(|side_effect| side_effect.starts_with('无'));
    if declared_read_only {
        ToolContract {
            effect: EffectKind::Read,
            recovery: RecoveryPolicy::Replay,
            retry_safe: retry_allowlist(tool),
        }
    } else {
        ToolContract {
            effect: EffectKind::Write,
            recovery: RecoveryPolicy::Verify,
            retry_safe: false,
        }
    }
}

/// 网络/设备查询并非都适合自动重试；保留经过验证的幂等白名单，但由契约统一暴露。
fn retry_allowlist(tool: &str) -> bool {
    matches!(
        tool,
        "list_devices"
            | "list_dir"
            | "read_file"
            | "find_files"
            | "grep_files"
            | "web_search"
            | "web_fetch"
            | "search_symbols"
            | "codebase_search"
            | "get_symbol_details"
            | "git_status"
            | "git_diff"
            | "git_log"
            | "read_runtime_logs"
            | "search_hilog"
            | "environment_check"
            | "search_sdk_api"
            | "read_sdk_api_module"
            | "search_harmony_docs"
            | "read_harmony_doc"
            | "get_api_detail"
            | "diff_api_versions"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contracts_are_conservative() {
        assert_eq!(contract("read_file").recovery, RecoveryPolicy::Replay);
        assert!(contract("read_file").retry_safe);
        assert_eq!(contract("edit_file").recovery, RecoveryPolicy::Verify);
        assert_eq!(contract("git_push").recovery, RecoveryPolicy::Manual);
        assert_eq!(contract("run_command").recovery, RecoveryPolicy::Manual);
        assert_eq!(contract("secret_delete").recovery, RecoveryPolicy::Manual);
        assert_eq!(contract("mcp__unknown").effect, EffectKind::Write);
    }

    #[test]
    fn every_registered_read_contract_comes_from_declared_side_effect() {
        for spec in super::super::TOOL_SPECS {
            if contract(spec.name).effect == EffectKind::Read {
                let declared = spec
                    .desc
                    .split("副作用：")
                    .nth(1)
                    .unwrap_or("")
                    .trim_start();
                assert!(
                    declared.starts_with('无'),
                    "{} 的只读契约没有声明依据",
                    spec.name
                );
            }
        }
    }
}
