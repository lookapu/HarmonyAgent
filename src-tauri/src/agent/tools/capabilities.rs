//! 任务能力包：把常见目标映射为最小工具集、推荐顺序、停止条件与验收条件。

#[derive(Clone, Copy, Debug)]
pub struct CapabilityPack {
    pub id: &'static str,
    pub schema_version: u32,
    pub version: &'static str,
    pub min_agent_version: &'static str,
    pub permission_ceiling: PermissionCeiling,
    pub triggers: &'static [&'static str],
    pub tools: &'static [&'static str],
    pub recommended_order: &'static [&'static str],
    pub stop_conditions: &'static [&'static str],
    pub acceptance: &'static [&'static str],
}

pub const CAPABILITY_PACK_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionCeiling {
    ReadOnly,
    ProjectWrite,
    DeviceWrite,
    Delivery,
}

const PROJECT_UNDERSTANDING: CapabilityPack = CapabilityPack {
    id: "project_understanding",
    schema_version: CAPABILITY_PACK_SCHEMA_VERSION,
    version: "1.0.0",
    min_agent_version: "2.0.0",
    permission_ceiling: PermissionCeiling::ReadOnly,
    triggers: &["project", "understand", "inspect", "项目", "理解", "阅读", "分析", "架构"],
    tools: &["list_dir", "get_project_info", "list_modules", "deep_scan", "codebase_search", "search_symbols", "read_file", "environment_check"],
    recommended_order: &["list_dir", "get_project_info", "list_modules", "deep_scan", "codebase_search", "read_file"],
    stop_conditions: &["核心入口、模块边界和构建方式已有来源证据", "继续读取不会改变当前任务计划"],
    acceptance: &["给出工程类型、入口、模块、依赖和风险摘要", "所有关键判断可追溯到文件或工具结果"],
};

const COMPILE_FIX: CapabilityPack = CapabilityPack {
    id: "compile_fix",
    schema_version: CAPABILITY_PACK_SCHEMA_VERSION,
    version: "1.0.0",
    min_agent_version: "2.0.0",
    permission_ceiling: PermissionCeiling::ProjectWrite,
    triggers: &["compile", "build error", "fix", "bug", "error", "编译", "构建失败", "修复", "报错"],
    tools: &["get_diagnostics", "get_build_log", "codebase_search", "read_file", "edit_file", "check_code", "build_project", "run_tests", "git_diff"],
    recommended_order: &["get_diagnostics", "get_build_log", "read_file", "edit_file", "check_code", "build_project", "run_tests"],
    stop_conditions: &["同一失败签名重复且没有新证据", "修复需要用户选择或外部环境变更"],
    acceptance: &["原始失败不再出现", "相关静态检查、构建或测试通过", "diff 仅包含目标修复"],
};

const FEATURE_DEVELOPMENT: CapabilityPack = CapabilityPack {
    id: "feature_development",
    schema_version: CAPABILITY_PACK_SCHEMA_VERSION,
    version: "1.0.0",
    min_agent_version: "2.0.0",
    permission_ceiling: PermissionCeiling::ProjectWrite,
    triggers: &["feature", "implement", "add", "develop", "功能", "实现", "新增", "开发"],
    tools: &["codebase_search", "get_symbol_details", "read_file", "write_file", "edit_file", "write_unit_tests", "run_tests", "build_project", "review_changes"],
    recommended_order: &["codebase_search", "get_symbol_details", "read_file", "edit_file", "write_unit_tests", "run_tests", "build_project", "review_changes"],
    stop_conditions: &["需求或交互存在会改变实现的歧义", "验收失败且缺少新的安全修复路径"],
    acceptance: &["功能行为满足目标契约", "新增或相关测试通过", "构建通过且变更经过审查"],
};

const REFACTOR: CapabilityPack = CapabilityPack {
    id: "refactor",
    schema_version: CAPABILITY_PACK_SCHEMA_VERSION,
    version: "1.0.0",
    min_agent_version: "2.0.0",
    permission_ceiling: PermissionCeiling::ProjectWrite,
    triggers: &["refactor", "rename", "cleanup", "重构", "重命名", "整理"],
    tools: &["deep_scan", "search_symbols", "lsp_references", "preview_edit", "multi_edit", "lsp_rename", "lsp_format", "run_tests", "build_project", "git_diff"],
    recommended_order: &["deep_scan", "search_symbols", "lsp_references", "preview_edit", "multi_edit", "lsp_format", "run_tests", "build_project"],
    stop_conditions: &["公共接口影响超出目标范围", "无法用测试或构建证明行为保持不变"],
    acceptance: &["目标结构已改善且公共行为不变", "引用完整、测试与构建通过"],
};

const BUILD_DEPLOY: CapabilityPack = CapabilityPack {
    id: "build_deploy",
    schema_version: CAPABILITY_PACK_SCHEMA_VERSION,
    version: "1.0.0",
    min_agent_version: "2.0.0",
    permission_ceiling: PermissionCeiling::DeviceWrite,
    triggers: &["deploy", "package", "hap", "install", "部署", "打包", "安装", "真机"],
    tools: &["environment_check", "check_sdk_alignment", "diagnose_signing", "ohpm_install", "build_project", "list_devices", "deploy", "start_ability", "take_screenshot", "read_logcat"],
    recommended_order: &["environment_check", "check_sdk_alignment", "diagnose_signing", "build_project", "list_devices", "deploy", "start_ability", "take_screenshot"],
    stop_conditions: &["签名或设备选择需要用户授权", "真实设备状态与预期不一致且无法安全补偿"],
    acceptance: &["目标产品构建成功并记录产物", "指定设备安装/启动成功", "读取设备状态确认结果"],
};

const DEVICE_DIAGNOSTICS: CapabilityPack = CapabilityPack {
    id: "device_diagnostics",
    schema_version: CAPABILITY_PACK_SCHEMA_VERSION,
    version: "1.0.0",
    min_agent_version: "2.0.0",
    permission_ceiling: PermissionCeiling::DeviceWrite,
    triggers: &["device", "crash", "freeze", "log", "设备", "崩溃", "卡死", "日志", "性能"],
    tools: &["list_devices", "get_app_info", "read_logcat", "search_hilog", "dump_ui_hierarchy", "take_screenshot", "dump_memory", "dump_battery", "device_perf", "analyze_crash"],
    recommended_order: &["list_devices", "get_app_info", "read_logcat", "take_screenshot", "dump_ui_hierarchy", "analyze_crash"],
    stop_conditions: &["设备离线或目标应用身份不明确", "采样已足以定位根因或需要代码修复阶段"],
    acceptance: &["设备、应用和时间窗口明确", "诊断结论引用日志、截图、层级或性能证据"],
};

const GIT_DELIVERY: CapabilityPack = CapabilityPack {
    id: "git_delivery",
    schema_version: CAPABILITY_PACK_SCHEMA_VERSION,
    version: "1.0.0",
    min_agent_version: "2.0.0",
    permission_ceiling: PermissionCeiling::Delivery,
    triggers: &["git", "commit", "push", "pull", "delivery", "提交", "推送", "拉取", "交付"],
    tools: &["git_status", "git_diff", "review_changes", "run_tests", "build_project", "secret_scan", "git_commit", "git_push", "git_log"],
    recommended_order: &["git_status", "git_diff", "review_changes", "secret_scan", "run_tests", "build_project", "git_commit", "git_push", "git_log"],
    stop_conditions: &["用户明确禁止提交或推送", "工作树含不属于当前任务的高风险改动", "交付门禁失败"],
    acceptance: &["提交内容和目标契约一致", "要求的测试/构建通过", "仅在明确要求时推送并读取远端状态确认"],
};

pub const CAPABILITY_PACKS: &[CapabilityPack] = &[
    PROJECT_UNDERSTANDING, COMPILE_FIX, FEATURE_DEVELOPMENT, REFACTOR,
    BUILD_DEPLOY, DEVICE_DIAGNOSTICS, GIT_DELIVERY,
];

pub const COMMON_TOOLS: &[&str] = &[
    "plan_task", "todo_write", "todo_get", "ask_user", "tool_help", "tool_list", "tool_history",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskPhase {
    Explore,
    Modify,
    Verify,
    Deliver,
    Recover,
}

impl TaskPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explore => "explore",
            Self::Modify => "modify",
            Self::Verify => "verify",
            Self::Deliver => "deliver",
            Self::Recover => "recover",
        }
    }
}

pub fn select(query: &str) -> Vec<&'static CapabilityPack> {
    let query = query.to_lowercase();
    let mut selected: Vec<&CapabilityPack> = CAPABILITY_PACKS
        .iter()
        .filter(|pack| pack.triggers.iter().any(|trigger| query.contains(trigger)))
        .collect();
    if selected.is_empty() {
        selected.push(&PROJECT_UNDERSTANDING);
    }
    selected
}

pub fn selected_tool_names(query: &str, limit: usize) -> Vec<&'static str> {
    let mut names = COMMON_TOOLS.to_vec();
    for pack in select(query) {
        for tool in pack.tools {
            if names.len() >= limit { return names; }
            if !names.contains(tool) && !tool_explicitly_forbidden(query, tool) {
                names.push(tool);
            }
        }
    }
    names
}

fn tool_explicitly_forbidden(query: &str, tool: &str) -> bool {
    let words: &[&str] = match tool {
        "git_push" => &["推送", "push"],
        "git_commit" => &["提交", "commit"],
        "deploy" | "deploy_all" | "install_app" => &["部署", "安装", "deploy"],
        _ => return false,
    };
    let query = query.to_lowercase();
    words.iter().any(|word| {
        ["不", "不要", "不用", "无需", "暂不", "别", "禁止"]
            .iter().any(|prefix| query.contains(&format!("{prefix}{word}")))
            || ["不用", "不需要", "暂时不用", "先不用", "取消"]
                .iter().any(|suffix| query.contains(&format!("{word}{suffix}")))
            || query.contains(&format!("do not {word}"))
            || query.contains(&format!("without {word}"))
    })
}

pub fn selected_tool_names_for_phase(
    query: &str,
    phase: TaskPhase,
    limit: usize,
) -> Vec<&'static str> {
    use super::contracts::EffectKind;
    let mut candidates = selected_tool_names(query, 64);
    if phase == TaskPhase::Verify {
        for tool in [
            "lsp_format", "format_file", "check_sdk_alignment", "lsp_diagnostics",
            "run_lint", "check_code", "run_tests",
            "build_project", "build_generic", "git_diff",
        ].into_iter().rev() {
            if let Some(index) = candidates.iter().position(|candidate| *candidate == tool) {
                candidates.remove(index);
            }
            candidates.insert(COMMON_TOOLS.len().min(candidates.len()), tool);
        }
    }
    let mut names = Vec::new();
    for tool in candidates {
        let contract = super::contracts::contract(tool);
        let allowed = COMMON_TOOLS.contains(&tool) || match phase {
            TaskPhase::Explore | TaskPhase::Recover => contract.effect == EffectKind::Read,
            TaskPhase::Modify => contract.effect != EffectKind::Destructive,
            TaskPhase::Verify => contract.effect == EffectKind::Read
                || contract.validator.is_some()
                || matches!(tool, "edit_file" | "write_file" | "multi_edit" | "preview_edit"
                    | "lsp_format" | "format_file" | "run_lint" | "check_code"),
            TaskPhase::Deliver => {
                select("git delivery").iter().any(|pack| pack.id == "git_delivery" && pack.tools.contains(&tool))
                    && (contract.validator.is_some()
                        || matches!(tool, "git_status" | "git_diff" | "review_changes" | "secret_scan" | "git_commit" | "git_push" | "git_log"))
            }
        };
        if allowed && !names.contains(&tool) {
            names.push(tool);
            if names.len() >= limit { break; }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_are_complete_and_reference_registered_tools() {
        for pack in CAPABILITY_PACKS {
            assert_eq!(pack.schema_version, CAPABILITY_PACK_SCHEMA_VERSION);
            assert!(crate::services::skill_manifest::parse_and_validate(&format!(
                "---\nharmony_agent_schema: 1\nversion: {}\nharmony_agent_compat: >={}\npermissions: []\n---",
                pack.version, pack.min_agent_version
            )).is_ok(), "{} 的版本/兼容范围必须可解析", pack.id);
            assert!(!pack.tools.is_empty());
            assert!(!pack.recommended_order.is_empty());
            assert!(!pack.stop_conditions.is_empty());
            assert!(!pack.acceptance.is_empty());
            for tool in pack.tools.iter().chain(pack.recommended_order.iter()) {
                assert!(crate::agent::tools::TOOL_SPECS.iter().any(|spec| spec.name == *tool),
                    "{} 引用了未注册工具 {tool}", pack.id);
            }
        }
    }

    #[test]
    fn selection_is_bounded_and_task_specific() {
        let fix = selected_tool_names("修复编译错误并运行测试", 40);
        assert!(fix.contains(&"get_diagnostics"));
        assert!(fix.contains(&"build_project"));
        assert!(!fix.contains(&"git_push"));
        let delivery = selected_tool_names("提交并推送 git 交付", 40);
        assert!(delivery.contains(&"git_push"));
        assert!(delivery.len() <= 40);
    }

    #[test]
    fn phase_selection_unlocks_side_effects_only_when_needed() {
        let goal = "修复编译错误，测试通过后提交并推送";
        let explore = selected_tool_names_for_phase(goal, TaskPhase::Explore, 32);
        assert!(explore.contains(&"read_file"));
        assert!(!explore.contains(&"edit_file"));
        assert!(!explore.contains(&"git_push"));
        let modify = selected_tool_names_for_phase(goal, TaskPhase::Modify, 32);
        assert!(modify.contains(&"edit_file"));
        assert!(!modify.contains(&"git_push"));
        let verify = selected_tool_names_for_phase(goal, TaskPhase::Verify, 32);
        assert!(verify.contains(&"run_tests"));
        assert!(verify.contains(&"lsp_format"));
        assert!(verify.contains(&"check_code"));
        assert!(verify.contains(&"git_diff"));
        assert!(!verify.contains(&"git_push"));
        let deliver = selected_tool_names_for_phase(goal, TaskPhase::Deliver, 32);
        assert!(deliver.contains(&"git_commit"));
        assert!(deliver.contains(&"git_push"));
    }

    #[test]
    fn explicit_negative_requirement_never_exposes_forbidden_delivery_tool() {
        let tools = selected_tool_names_for_phase(
            "检查、测试并提交，推送暂时不用",
            TaskPhase::Deliver,
            32,
        );
        assert!(tools.contains(&"git_commit"));
        assert!(!tools.contains(&"git_push"));
    }

    #[test]
    fn bounded_phase_selection_keeps_representative_tasks_acceptable() {
        let cases: &[(&str, &[(&str, &str, &str)])] = &[
            (
                "修复代码，运行测试并构建",
                &[
                    ("edit_file", r#"{"path":"src/a.rs"}"#, "ok"),
                    ("run_tests", "{}", "tests passed"),
                    ("build_project", "{}", "build passed"),
                ],
            ),
            (
                "部署应用到设备",
                &[
                    ("deploy", "{}", "installed"),
                    ("take_screenshot", "{}", "device UI captured"),
                ],
            ),
            (
                "提交并推送 git 交付",
                &[
                    ("git_commit", "{}", "committed"),
                    ("git_status", "{}", "clean"),
                    ("git_push", "{}", "pushed"),
                    ("git_status", "{}", "up to date"),
                ],
            ),
        ];
        for (goal, evidence_rows) in cases {
            let exposed = [TaskPhase::Explore, TaskPhase::Modify, TaskPhase::Verify, TaskPhase::Deliver]
                .into_iter()
                .flat_map(|phase| selected_tool_names_for_phase(goal, phase, 32))
                .collect::<std::collections::HashSet<_>>();
            assert!(exposed.len() < crate::agent::tools::TOOL_SPECS.len());
            assert!(evidence_rows.iter().all(|(tool, _, _)| exposed.contains(tool)), "{goal}");
            let evidence = evidence_rows.iter().map(|(tool, args, output)| {
                crate::agent::acceptance::ToolEvidence {
                    tool, args, output, succeeded: true,
                }
            }).collect::<Vec<_>>();
            let report = crate::agent::acceptance::evaluate(goal, &evidence);
            assert!(report.passed, "{goal}: {:?}", report.blockers);
        }
    }
}
