//! 基座更新（tauri-plugin-updater）系统代理支持。
//!
//! updater 插件内部用 reqwest 且无代理配置项，但 reqwest 默认读取
//! HTTPS_PROXY / HTTP_PROXY 环境变量。因此更新检查/下载前临时注入系统代理，
//! 结束后恢复快照，避免影响其他请求。

/// 开始基座更新：若存在系统代理则临时注入环境变量，返回被覆盖变量的快照
/// （每个元素 [变量名, 旧值]；旧值为 null 表示注入前未设置）。
#[tauri::command]
pub fn begin_update_proxy() -> Vec<Vec<Option<String>>> {
    crate::utils::net::apply_env_proxy()
        .into_iter()
        .map(|(k, v)| vec![Some(k), v])
        .collect()
}

/// 结束基座更新：按 begin_update_proxy 返回的快照恢复环境变量
#[tauri::command]
pub fn end_update_proxy(saved: Vec<Vec<Option<String>>>) {
    let pairs: Vec<(String, Option<String>)> = saved
        .into_iter()
        .filter_map(|mut pair| {
            if pair.len() >= 2 {
                Some((pair.remove(0).unwrap_or_default(), pair.remove(0)))
            } else {
                None
            }
        })
        .collect();
    crate::utils::net::restore_env_proxy(&pairs);
}

/// 读取当前系统代理地址（环境变量优先，Windows 注册表兑底）。
/// 供前端显式传给 updater 的 check({ proxy })，检查+下载+安装全程生效。
#[tauri::command]
pub fn get_system_proxy() -> Option<String> {
    crate::utils::net::read_system_proxy()
}
