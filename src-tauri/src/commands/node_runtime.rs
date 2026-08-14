//! Node 运行时命令：状态查询 / 在线升级 / 恢复出厂（健康页卡片）

use tauri::AppHandle;
use crate::services::node_runtime::{self, NodeRuntimeInfo};

/// 查询 Node 运行时状态（版本、来源、目录）
#[tauri::command]
pub fn get_node_runtime(app: AppHandle) -> NodeRuntimeInfo {
    node_runtime::get_node_runtime_info(&app)
}

/// 升级 Node 运行时到指定版本（version 缺省时自动取最新 LTS），完成后立即生效。
/// use_proxy: None=自动（有系统代理则用）；Some(true)=强制走系统代理；Some(false)=直连。
#[tauri::command]
pub async fn upgrade_node_runtime(
    app: AppHandle,
    version: Option<String>,
    use_proxy: Option<bool>,
) -> Result<NodeRuntimeInfo, String> {
    node_runtime::upgrade_node_runtime(&app, version, use_proxy).await
}

/// 删除升级版，回到出厂捆绑版本
#[tauri::command]
pub fn reset_node_runtime(app: AppHandle) -> Result<NodeRuntimeInfo, String> {
    node_runtime::reset_node_runtime(&app)
}
