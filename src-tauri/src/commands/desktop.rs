use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

#[derive(Serialize)]
pub struct LocaleInfo {
    /// 系统主语言（zh / en 等，取 BCP47 首段，小写）
    pub locale: String,
    /// 是否中文环境
    pub is_zh: bool,
}

/// 探测系统 UI 语言，用于前端“跟随系统”。
/// 优先使用 tauri-plugin-os 的 locale()；失败时回退环境变量。
#[tauri::command]
pub fn detect_system_locale() -> LocaleInfo {
    let raw = tauri_plugin_os::locale()
        .map(|s| s.to_lowercase())
        .unwrap_or_else(|| detect_locale_env().to_lowercase());
    let locale = raw
        .split(['-', '_'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("en")
        .to_string();
    let is_zh = locale.starts_with("zh");
    LocaleInfo {
        locale: if is_zh { "zh".into() } else { locale },
        is_zh,
    }
}

fn detect_locale_env() -> String {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(key) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    "en".to_string()
}

#[derive(Serialize, Clone)]
pub struct NotifyPayload {
    pub title: String,
    pub body: String,
    /// success / error / info，影响前端内联提示样式
    pub kind: String,
}

/// 发送桌面通知（构建完成 / 失败 / 部署成功等）。
/// 同时向主窗口派发 "desktop-notify" 事件，应用在前台时可显示为内联横幅。
#[tauri::command]
pub fn send_notification(app: AppHandle, title: String, body: String, kind: Option<String>) {
    let kind = kind.unwrap_or_else(|| "info".to_string());
    let _ = app.notification().builder().title(&title).body(&body).show();

    if let Some(win) = app.get_webview_window("main") {
        let _ = win.emit(
            "desktop-notify",
            NotifyPayload {
                title,
                body,
                kind,
            },
        );
    }
}

/// 显示/聚焦主窗口（托盘及全局快捷键调用）。
pub fn focus_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
