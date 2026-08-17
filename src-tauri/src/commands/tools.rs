//! 工具治理命令（前端面板数据源）：
//! - list_tool_groups：全部工具 → task_group 映射（[75] 按任务分组折叠 UI）
//! - tools_health：轻量工具链体检（[66] 启动自动 ping 用，只查关键工具链不查工程结构）

use tauri::Manager;

/// 全部工具 → task_group 映射（与后端 TOOL_GROUP 同源，前端分组折叠用）
#[tauri::command]
pub fn list_tool_groups() -> Vec<(String, String)> {
    crate::agent::tools::TOOL_GROUP
        .iter()
        .map(|(n, g)| (n.to_string(), g.to_string()))
        .collect()
}

/// 轻量工具链体检：只查 hvigorw / hdc / ohpm 三个关键工具链是否可用（不查工程结构，
/// 供启动自动 ping 与顶部横幅使用，耗时毫秒级）。
#[tauri::command]
pub fn tools_health(app: tauri::AppHandle) -> Vec<crate::commands::health::ToolchainCheck> {
    // 复用完整体检命令：不传 project_id（跳过工程结构检查），custom_paths 走同样的自动发现
    let app_clone = app.clone();
    let db = app_clone.state::<crate::db::DbState>();
    crate::commands::health::check_harmony_toolchain(app, db, None, None)
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.name != "project_structure")
        .collect()
}
