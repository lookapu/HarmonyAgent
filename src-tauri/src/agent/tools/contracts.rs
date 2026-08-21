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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdempotencyKind {
    Natural,
    Keyed,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationMode {
    Cooperative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    None,
    ProjectTrust,
    Always,
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
    pub idempotency: IdempotencyKind,
    pub timeout_ms: u64,
    pub cancellation: CancellationMode,
    pub approval: ApprovalPolicy,
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
    let (effect, recovery, retry_safe, idempotency) = if MANUAL_RECOVERY.contains(&tool) {
        (
            EffectKind::Destructive,
            RecoveryPolicy::Manual,
            false,
            IdempotencyKind::None,
        )
    } else {
        let declared_read_only = !tool.starts_with("mcp__")
            && super::TOOL_SPECS
                .iter()
                .find(|spec| spec.name == tool)
                .and_then(|spec| spec.desc.split("副作用：").nth(1))
                .map(str::trim_start)
                .is_some_and(|side_effect| side_effect.starts_with('无'));
        if declared_read_only {
            (
                EffectKind::Read,
                RecoveryPolicy::Replay,
                retry_allowlist(tool),
                IdempotencyKind::Natural,
            )
        } else {
            (
                EffectKind::Write,
                RecoveryPolicy::Verify,
                false,
                IdempotencyKind::Keyed,
            )
        }
    };
    let approval = match crate::services::permissions::tool_level(tool) {
        crate::services::permissions::Level::L0 => ApprovalPolicy::None,
        crate::services::permissions::Level::L1 => ApprovalPolicy::ProjectTrust,
        crate::services::permissions::Level::L2 => ApprovalPolicy::Always,
    };
    ToolContract {
        effect,
        recovery,
        retry_safe,
        idempotency,
        timeout_ms: timeout_ms(tool),
        cancellation: CancellationMode::Cooperative,
        approval,
    }
}

fn timeout_ms(tool: &str) -> u64 {
    let seconds = match tool {
        "build_project" | "deploy" | "deploy_all" | "run_tests" | "flaky_test_detect"
        | "build_generic" | "run_perf_benchmark" | "auto_explore" => 15 * 60,
        "spawn_agents" => 20 * 60,
        _ => 3 * 60,
    };
    seconds * 1000
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
        assert_eq!(contract("mcp__unknown").approval, ApprovalPolicy::Always);
        assert_eq!(contract("mcp__unknown").idempotency, IdempotencyKind::Keyed);
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

    #[test]
    fn every_registered_tool_has_complete_execution_metadata() {
        for spec in super::super::TOOL_SPECS {
            let value = serde_json::to_value(contract(spec.name)).unwrap();
            for field in [
                "effect", "recovery", "retry_safe", "idempotency", "timeout_ms",
                "cancellation", "approval",
            ] {
                assert!(value.get(field).is_some(), "{} 缺少 {field}", spec.name);
            }
            assert!(value["timeout_ms"].as_u64().unwrap_or(0) > 0);
        }
    }
}
