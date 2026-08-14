//! JDK 运行时命令：状态查询 / 可用版本 / 在线安装 / 更新检查 / 默认切换 / 卸载（健康页卡片）

use tauri::AppHandle;
use crate::services::jdk_runtime::{self, JdkRuntimeInfo, JdkUpdateInfo};

/// 查询 JDK 运行时状态（版本列表、默认版本、系统 JAVA_HOME）
#[tauri::command]
pub fn get_jdk_runtime(app: AppHandle) -> JdkRuntimeInfo {
    jdk_runtime::get_jdk_runtime_info(&app)
}

/// 查询可安装的 feature 版本（Adoptium LTS 列表，如 8/11/17/21/25）。
/// use_proxy: None=自动（有系统代理则用）；Some(true)=强制走系统代理；Some(false)=直连。
#[tauri::command]
pub async fn fetch_jdk_releases(use_proxy: Option<bool>) -> Result<Vec<String>, String> {
    jdk_runtime::fetch_available_releases(use_proxy).await
}

/// 在线安装/更新指定 feature 版本的 JDK（下载源在 GitHub，推荐走系统代理），
/// 下载全程通过 `jdk-install-progress` 事件推送进度，完成后立即生效
/// （未设置默认版本时自动设为默认；同 feature 已安装时为覆盖更新）。
/// use_proxy: None=自动（优先系统代理，无则直连）；Some(true)=强制走系统代理；Some(false)=直连。
#[tauri::command]
pub async fn install_jdk(
    app: AppHandle,
    feature: String,
    use_proxy: Option<bool>,
) -> Result<JdkRuntimeInfo, String> {
    jdk_runtime::install_jdk(&app, feature, use_proxy).await
}

/// 检查已装 JDK 是否有可用的补丁更新（Adoptium 最新版本与本地比较）。
/// 网络不可达时返回 Err，前端静默降级。
#[tauri::command]
pub async fn check_jdk_updates(app: AppHandle) -> Result<Vec<JdkUpdateInfo>, String> {
    jdk_runtime::check_jdk_updates(&app).await
}

/// 设置默认 JDK 版本（多版本并存时切换构建/命令使用的 JDK）
#[tauri::command]
pub fn set_default_jdk(app: AppHandle, feature: String) -> Result<JdkRuntimeInfo, String> {
    jdk_runtime::set_default_jdk(&app, feature)
}

/// 卸载升级版 JDK（捆绑版不可卸载）；卸载默认版本时自动回落其他版本
#[tauri::command]
pub fn uninstall_jdk(app: AppHandle, feature: String) -> Result<JdkRuntimeInfo, String> {
    jdk_runtime::uninstall_jdk(&app, feature)
}
