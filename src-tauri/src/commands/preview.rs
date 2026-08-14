//! Web 预览：在独立窗口打开 http/https 地址（内置浏览器窗口，供调试/产物预览）。
//!
//! 复用固定标签 `preview`：窗口已存在时直接导航新地址并聚焦，避免多开堆积。

use tauri::Manager;

/// 打开（或导航）Web 预览窗口。仅接受 http/https，防协议注入。
#[tauri::command]
pub async fn open_preview_window(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let url = url.trim().to_string();
    let lower = url.to_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("仅支持 http/https 地址".into());
    }
    let parsed: url::Url = url.parse().map_err(|_| "URL 格式不正确".to_string())?;
    if let Some(w) = app.get_webview_window("preview") {
        let _ = w.navigate(parsed);
        let _ = w.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(&app, "preview", tauri::WebviewUrl::External(parsed))
        .title("Web 预览")
        .inner_size(1100.0, 760.0)
        .min_inner_size(480.0, 360.0)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}
