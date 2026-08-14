//! 权限分级与命令白名单
//!
//! - L0（只读）：读取/搜索/列表/查看类，对已信任项目免审。
//! - L1（写入，限定项目内）：写文件、编辑、构建、安装依赖、部署等，对已信任项目免审。
//! - L2（危险/越界）：删除、执行任意命令、git push、网络写等，始终需要用户确认。
//!
//! 命令白名单：run_command 仅允许一组明确的可执行程序（开发工具链），其余命令需经 L2 审批。

use rusqlite::Connection;

/// 工具权限级别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    L0,
    L1,
    L2,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::L0 => "L0",
            Level::L1 => "L1",
            Level::L2 => "L2",
        }
    }
}

/// 返回工具的权限级别。未登记的工具默认 L2（保守）。
pub fn tool_level(tool: &str) -> Level {
    let t = tool
        .strip_prefix("mcp__")
        .map(|s| s.split("__").last().unwrap_or(s))
        .unwrap_or(tool);
    match t {
        // L0 只读
        "list_devices" | "list_dir" | "read_file" | "find_files" | "grep_files"
        | "get_project_info" | "get_build_log" | "git_status" | "git_diff"
        | "read_logcat" | "web_search" | "web_fetch" | "search_symbols"
        | "get_diagnostics" | "todo_write" | "ask_user"
        | "check_code" | "deep_scan" | "codebase_search" | "get_symbol_details"
        | "git_log" | "git_blame" | "get_env_info" | "get_file_info"
        | "device_perf" | "collect_perf" | "read_runtime_logs"
        | "list_modules" | "read_module_config" | "search_sdk_api" | "read_sdk_api_module"
        | "check_sdk_alignment" | "search_harmony_docs" | "read_harmony_doc"
        | "show_diagnose_card"
        | "dump_ui_hierarchy" | "dump_memory" | "get_installed_apps" | "get_app_info"
        | "analyze_hap_size" | "search_hilog" | "run_lint" | "check_signature"
        | "dump_battery" | "scan_api_compat" | "search_api"
        | "get_api_detail" | "diff_api_versions"
        | "list_agents"
        | "list_emulators" | "device_shell" | "ohpm_search"
        | "environment_check" | "search_knowledge" | "list_mcp_servers"
        | "plan_task" | "update_progress" | "get_cost_summary"
        | "review_changes" | "analyze_generic_project" | "job_list" | "job_output" => Level::L0,
        // L1 写入（限定项目内 / 开发动作）
        "write_file" | "edit_file" | "move_file" | "copy_file" | "undo_edit" | "build_project" | "deploy" | "ohpm_install"
        | "run_tests" | "take_screenshot" | "save_memory" | "spawn_agents"
        | "http_request" | "multi_edit"
        | "verify_ui" | "deploy_all" | "write_unit_tests" | "run_ui_flow" | "run_perf_benchmark"
        | "start_ability" | "clear_app_data" | "uninstall_app" | "grant_permission"
        | "set_wifi_state" | "set_airplane_mode" | "screen_record"
        | "record_ui" | "replay_ui" | "set_network_condition" | "auto_explore"
        | "refresh_api_db" | "refresh_api_details"
        | "connect_device" | "manage_hdc" | "start_emulator" | "create_emulator"
        | "device_file" | "stop_app" | "analyze_crash"
        | "manage_memory" | "manage_knowledge" | "export_data"
        | "build_generic" | "run_app" | "git_fetch"
        | "git_branch" | "git_tag" | "job_kill" => Level::L1,
        // L2 危险/越界
        "delete_file" | "run_command" | "git_commit" | "git_stash" | "git_restore"
        | "git_pull" | "git_push" => Level::L2,
        _ => Level::L2,
    }
}

/// run_command 允许的可执行程序白名单（按小写程序名匹配，不含路径/扩展名）。
/// 命中白名单且无危险子命令时，视为 L1（已信任项目免审）；否则 L2。
pub const ALLOWED_COMMANDS: &[&str] = &[
    "git", "node", "npm", "npx", "pnpm", "yarn", "ohpm", "hvigorw",
    "hvigorw.bat", "hdc", "java", "gradlew", "gradlew.bat",
    "cargo", "rustc", "python", "python3", "py", "code", "cmd",
    "go", "mvn", "mvnw", "mvnw.bat", "dotnet", "pip", "pip3", "pytest",
    "ruby", "bundle", "composer", "php", "flutter", "dart", "swift", "xcodebuild",
    "make", "cmake", "clang", "gcc", "g++",
];

/// 命令中的危险子操作（即使程序在白名单内，命中也升级为 L2）。
const DANGEROUS_PATTERNS: &[&str] = &[
    "git push", "git reset --hard", "git clean -f", "git checkout .",
    "npm publish", "ohpm publish", "hdc shell", "hdc install",
    "format ", "shutdown", "diskpart", "mkfs", "rm -rf", "rm -fr",
    "rd /s", "del /f", "reg delete", "cipher /w",
];

/// 判断命令是否在白名单内（程序名允许且无危险子操作）。
pub fn is_command_allowed(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_lowercase();
    // 危险子操作直接拒绝（这类必须走 L2 人工确认，且部分会被 run_command 黑名单拦截）
    if DANGEROUS_PATTERNS.iter().any(|p| lower.contains(p)) {
        return false;
    }
    // 取第一个 token 作为程序名，去掉引号和可能的路径前缀
    let first = trimmed.split_whitespace().next().unwrap_or("");
    let first = first.trim_matches('"');
    let prog = std::path::Path::new(first)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(first)
        .trim_end_matches(".exe")
        .trim_end_matches(".cmd")
        .trim_end_matches(".bat");
    let prog_lower = prog.to_lowercase();
    ALLOWED_COMMANDS
        .iter()
        .any(|c| c.eq_ignore_ascii_case(&prog_lower))
}

/// run_command 的实际权限级别：白名单内 → L1，否则 L2。
pub fn command_level(cmd: &str) -> Level {
    if is_command_allowed(cmd) {
        Level::L1
    } else {
        Level::L2
    }
}

/// 某工具在给定项目信任状态下是否可免审自动执行。
pub fn auto_approve(tool: &str, project_trusted: bool, cmd_arg: Option<&str>) -> bool {
    if !project_trusted {
        return false;
    }
    let level = match tool {
        "run_command" => cmd_arg.map(command_level).unwrap_or(Level::L2),
        other => tool_level(other),
    };
    matches!(level, Level::L0 | Level::L1)
}

/// 查询某项目某工具是否已被用户"始终允许"（permissions 表记忆）。
pub fn is_remembered(conn: &Connection, project_id: &str, op_class: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM permissions WHERE project_id = ?1 AND op_class = ?2 AND allowed = 1 LIMIT 1",
        rusqlite::params![project_id, op_class],
        |_| Ok(()),
    )
    .is_ok()
}

/// 记忆用户对某项目某类操作的授权。
#[allow(dead_code)]
pub fn remember(conn: &Connection, project_id: &str, op_class: &str, allowed: bool) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO permissions (id, project_id, op_class, allowed, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            format!("{project_id}:{op_class}"),
            project_id,
            op_class,
            allowed as i32,
            chrono::Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_levels() {
        assert_eq!(tool_level("read_file"), Level::L0);
        assert_eq!(tool_level("search_symbols"), Level::L0);
        assert_eq!(tool_level("write_file"), Level::L1);
        assert_eq!(tool_level("delete_file"), Level::L2);
        assert_eq!(tool_level("unknown_xyz"), Level::L2);
        // 新工具分级回归：未登记的只读/验证工具默认 L2 会打断部署闭环，必须登记
        assert_eq!(tool_level("collect_perf"), Level::L0);
        assert_eq!(tool_level("read_runtime_logs"), Level::L0);
        assert_eq!(tool_level("list_modules"), Level::L0);
        assert_eq!(tool_level("read_module_config"), Level::L0);
        assert_eq!(tool_level("search_harmony_docs"), Level::L0);
        assert_eq!(tool_level("read_harmony_doc"), Level::L0);
        assert_eq!(tool_level("search_sdk_api"), Level::L0);
        assert_eq!(tool_level("read_sdk_api_module"), Level::L0);
        assert_eq!(tool_level("check_sdk_alignment"), Level::L0);
        assert_eq!(tool_level("show_diagnose_card"), Level::L0);
        assert_eq!(tool_level("verify_ui"), Level::L1);
        assert_eq!(tool_level("deploy_all"), Level::L1);
        assert_eq!(tool_level("write_unit_tests"), Level::L1);
        assert_eq!(tool_level("run_ui_flow"), Level::L1);
        assert_eq!(tool_level("run_perf_benchmark"), Level::L1);
        assert_eq!(tool_level("dump_ui_hierarchy"), Level::L0);
        assert_eq!(tool_level("dump_memory"), Level::L0);
        assert_eq!(tool_level("get_installed_apps"), Level::L0);
        assert_eq!(tool_level("get_app_info"), Level::L0);
        assert_eq!(tool_level("start_ability"), Level::L1);
        assert_eq!(tool_level("clear_app_data"), Level::L1);
        assert_eq!(tool_level("uninstall_app"), Level::L1);
        assert_eq!(tool_level("grant_permission"), Level::L1);
        assert_eq!(tool_level("set_wifi_state"), Level::L1);
        assert_eq!(tool_level("set_airplane_mode"), Level::L1);
        assert_eq!(tool_level("screen_record"), Level::L1);
        assert_eq!(tool_level("analyze_hap_size"), Level::L0);
        assert_eq!(tool_level("search_hilog"), Level::L0);
        assert_eq!(tool_level("run_lint"), Level::L0);
        assert_eq!(tool_level("check_signature"), Level::L0);
        assert_eq!(tool_level("dump_battery"), Level::L0);
        assert_eq!(tool_level("scan_api_compat"), Level::L0);
        assert_eq!(tool_level("record_ui"), Level::L1);
        assert_eq!(tool_level("replay_ui"), Level::L1);
        assert_eq!(tool_level("set_network_condition"), Level::L1);
        assert_eq!(tool_level("auto_explore"), Level::L1);
        assert_eq!(tool_level("search_api"), Level::L0);
        assert_eq!(tool_level("refresh_api_db"), Level::L1);
        assert_eq!(tool_level("refresh_api_details"), Level::L1);
        assert_eq!(tool_level("get_api_detail"), Level::L0);
        assert_eq!(tool_level("diff_api_versions"), Level::L0);
        // 设备/环境/知识/计划/git 类工具分级回归（未登记的只读/开发动作默认 L2 会打断自动闭环）
        assert_eq!(tool_level("connect_device"), Level::L1);
        assert_eq!(tool_level("manage_hdc"), Level::L1);
        assert_eq!(tool_level("list_emulators"), Level::L0);
        assert_eq!(tool_level("start_emulator"), Level::L1);
        assert_eq!(tool_level("create_emulator"), Level::L1);
        assert_eq!(tool_level("device_file"), Level::L1);
        assert_eq!(tool_level("stop_app"), Level::L1);
        assert_eq!(tool_level("device_shell"), Level::L0);
        assert_eq!(tool_level("analyze_crash"), Level::L1);
        assert_eq!(tool_level("ohpm_search"), Level::L0);
        assert_eq!(tool_level("environment_check"), Level::L0);
        assert_eq!(tool_level("search_knowledge"), Level::L0);
        assert_eq!(tool_level("list_mcp_servers"), Level::L0);
        assert_eq!(tool_level("plan_task"), Level::L0);
        assert_eq!(tool_level("update_progress"), Level::L0);
        assert_eq!(tool_level("manage_memory"), Level::L1);
        assert_eq!(tool_level("manage_knowledge"), Level::L1);
        assert_eq!(tool_level("export_data"), Level::L1);
        assert_eq!(tool_level("get_cost_summary"), Level::L0);
        assert_eq!(tool_level("review_changes"), Level::L0);
        assert_eq!(tool_level("analyze_generic_project"), Level::L0);
        assert_eq!(tool_level("build_generic"), Level::L1);
        assert_eq!(tool_level("run_app"), Level::L1);
        assert_eq!(tool_level("git_fetch"), Level::L1);
        assert_eq!(tool_level("git_pull"), Level::L2);
        assert_eq!(tool_level("git_push"), Level::L2);
        assert_eq!(tool_level("job_list"), Level::L0);
        assert_eq!(tool_level("job_output"), Level::L0);
        assert_eq!(tool_level("job_kill"), Level::L1);
    }

    #[test]
    fn test_whitelist() {
        assert!(is_command_allowed("git status"));
        assert!(is_command_allowed("npm install"));
        assert!(is_command_allowed("hvigorw.bat assembleHap"));
        assert!(is_command_allowed("hdc list targets"));
        assert!(is_command_allowed("go test ./..."));
        assert!(is_command_allowed("cargo build"));
        assert!(is_command_allowed("mvn compile"));
        assert!(is_command_allowed("dotnet build"));
        assert!(is_command_allowed("python -m pytest"));
        assert!(!is_command_allowed("git push origin main"));
        assert!(!is_command_allowed("format c:"));
        assert!(!is_command_allowed("malware.exe"));
    }

    #[test]
    fn test_auto_approve() {
        assert!(auto_approve("read_file", true, None));
        assert!(!auto_approve("read_file", false, None));
        assert!(auto_approve("delete_file", false, None) == false);
        assert!(auto_approve("run_command", true, Some("git status")));
        assert!(!auto_approve("run_command", true, Some("git push")));
    }
}
