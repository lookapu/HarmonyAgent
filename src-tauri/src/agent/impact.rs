//! 宿主能力影响契约：审批前用统一口径说明「改什么、能不能撤销、影响到哪里」。
//!
//! 描述是**审批展示与审计口径**，不是权限判定本身：权限分级仍由 `permissions` 决定，
//! 凭据与撤销仍由 `broker_approval` 负责。这里只保证用户在看到弹窗时得到同一套说法，
//! 不再出现「有的操作只有工具名和参数、有的操作没人说清后果」。
//!
//! 未覆盖的工具返回 `None`（弹窗保持原样），不做猜测性描述。

/// 可逆性分级：决定审批卡片上的风险标签与文案。
pub const REVERSIBLE: &str = "reversible";
/// 可恢复但有代价（需要回滚包、重新授权、重建环境等）。
pub const HARD_TO_REVERSE: &str = "hard_to_reverse";
/// 不可撤销（数据删除、应用卸载、产物发布等）。
pub const IRREVERSIBLE: &str = "irreversible";

/// 影响对象类别：设备本身、设备上的应用数据、工程工作区、宿主环境。
pub const SCOPE_DEVICE: &str = "device";
pub const SCOPE_APP_DATA: &str = "app_data";
pub const SCOPE_WORKSPACE: &str = "workspace";
pub const SCOPE_HOST: &str = "host";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ImpactContract {
    /// [`REVERSIBLE`] / [`HARD_TO_REVERSE`] / [`IRREVERSIBLE`]
    pub reversibility: &'static str,
    /// [`SCOPE_DEVICE`] / [`SCOPE_APP_DATA`] / [`SCOPE_WORKSPACE`] / [`SCOPE_HOST`]
    pub scope: &'static str,
    /// 具体目标（设备、包名、路径…）；从参数中提取，最多 3 个，供用户核对。
    pub targets: Vec<String>,
    /// 一句话说明后果与恢复方式。
    pub note: &'static str,
}

/// 从参数里够用的目标键；顺序固定，便于卡片稳定展示。
const TARGET_KEYS: &[&str] = &[
    "device",
    "target",
    "bundle",
    "name",
    "path",
    "hap_path",
    "out_path",
    "profile_path",
    "local",
    "remote",
];
const MAX_TARGETS: usize = 3;

fn targets(args: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in TARGET_KEYS {
        if out.len() >= MAX_TARGETS {
            break;
        }
        let Some(value) = args.get(*key) else { continue };
        let text = match value {
            serde_json::Value::String(text) => text.trim().to_string(),
            serde_json::Value::Number(number) => number.to_string(),
            _ => continue,
        };
        if !text.is_empty() && !out.contains(&text) {
            out.push(text);
        }
    }
    out
}

/// 给出该工具的影响契约；未覆盖的工具返回 `None`，不猜测。
pub fn describe(tool: &str, args: &serde_json::Value) -> Option<ImpactContract> {
    let (reversibility, scope, note) = match tool {
        "ota_pack" => (
            IRREVERSIBLE,
            SCOPE_WORKSPACE,
            "在工作区生成并发布新的 OTA 升级包；产物不覆盖旧包，但已发布的包需要人工清理。",
        ),
        "deploy" | "deploy_all" => (
            HARD_TO_REVERSE,
            SCOPE_APP_DATA,
            "向设备安装或覆盖应用，会替换设备上已安装的版本与应用数据；需要回滚包才能恢复旧版本。",
        ),
        "uninstall_app" => (
            IRREVERSIBLE,
            SCOPE_APP_DATA,
            "从设备卸载应用并删除其数据，卸载后无法恢复原数据。",
        ),
        "clear_app_data" => (
            IRREVERSIBLE,
            SCOPE_APP_DATA,
            "清除设备上的应用数据，删除后无法恢复。",
        ),
        "grant_permission" => (
            HARD_TO_REVERSE,
            SCOPE_APP_DATA,
            "修改设备上应用的权限授予状态；需要再次执行相反的授权才能复原。",
        ),
        "create_emulator" => (
            HARD_TO_REVERSE,
            SCOPE_HOST,
            "在开发机上创建模拟器实例并占用磁盘/内存；需要显式删除才能回收。",
        ),
        "start_emulator" => (
            REVERSIBLE,
            SCOPE_HOST,
            "启动模拟器实例；停止后即可恢复开发机资源。",
        ),
        "stop_app" => (REVERSIBLE, SCOPE_DEVICE, "停止设备上的应用进程，重新启动即可恢复。"),
        "device_file" => (
            HARD_TO_REVERSE,
            SCOPE_DEVICE,
            "与设备互传文件，会覆盖设备上同名文件；被覆盖内容无法从宿主侧恢复。",
        ),
        "set_wifi_state" | "set_airplane_mode" => (
            REVERSIBLE,
            SCOPE_DEVICE,
            "改变设备网络状态，改回原状态即可恢复。",
        ),
        "screen_record" | "record_ui" => (
            REVERSIBLE,
            SCOPE_DEVICE,
            "在设备上开始录制，产物落到工作区；停止录制即结束。",
        ),
        _ => return None,
    };
    Some(ImpactContract {
        reversibility,
        scope,
        targets: targets(args),
        note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_tool_has_no_impact_contract() {
        assert!(describe("read_file", &serde_json::json!({"path": "a.rs"})).is_none());
        assert!(describe("run_command", &serde_json::json!({"command": "ls"})).is_none());
    }

    #[test]
    fn destructive_tools_are_marked_irreversible_with_app_data_scope() {
        for tool in ["clear_app_data", "uninstall_app"] {
            let contract = describe(
                tool,
                &serde_json::json!({"device": "ABC123", "bundle": "com.demo"}),
            )
            .unwrap();
            assert_eq!(contract.reversibility, IRREVERSIBLE, "{tool}");
            assert_eq!(contract.scope, SCOPE_APP_DATA, "{tool}");
            assert_eq!(contract.targets, vec!["ABC123", "com.demo"]);
            assert!(!contract.note.is_empty());
        }
    }

    #[test]
    fn deploy_is_recoverable_only_with_a_rollback_package() {
        let contract = describe("deploy", &serde_json::json!({"device": "ABC"})).unwrap();
        assert_eq!(contract.reversibility, HARD_TO_REVERSE);
        assert!(contract.note.contains("回滚"));
    }

    #[test]
    fn ota_publication_targets_workspace_paths() {
        let contract = describe(
            "ota_pack",
            &serde_json::json!({
                "hap_path": "entry.hap",
                "out_path": "dist/entry.pkg",
                "profile_path": "sign/profile.json",
                "ignore": "not-a-target",
            }),
        )
        .unwrap();
        assert_eq!(contract.reversibility, IRREVERSIBLE);
        assert_eq!(contract.scope, SCOPE_WORKSPACE);
        // 只取前 3 个已知目标键，未知键不进入展示
        assert_eq!(
            contract.targets,
            vec!["entry.hap", "dist/entry.pkg", "sign/profile.json"]
        );
    }

    #[test]
    fn targets_are_deduped_and_skip_non_strings() {
        let contract = describe(
            "device_file",
            &serde_json::json!({"device": "ABC", "target": "ABC", "local": 7, "remote": ""}),
        )
        .unwrap();
        assert_eq!(contract.targets, vec!["ABC", "7"]);
    }

    #[test]
    fn impact_contract_serializes_for_approval_payload() {
        let contract = describe("stop_app", &serde_json::json!({"bundle": "com.demo"})).unwrap();
        let value = serde_json::to_value(&contract).unwrap();
        assert_eq!(value["reversibility"], REVERSIBLE);
        assert_eq!(value["scope"], SCOPE_DEVICE);
        assert_eq!(value["targets"][0], "com.demo");
    }
}
