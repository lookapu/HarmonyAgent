use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};

use crate::commands::desktop::focus_main_window;

pub fn setup(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let new_chat_item = MenuItemBuilder::with_id("new_chat", "新建对话").build(app)?;
    let switch_item = MenuItemBuilder::with_id("switch", "切换 Provider").build(app)?;
    let dashboard_item = MenuItemBuilder::with_id("dashboard", "打开面板").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "退出").build(app)?;

    let menu = MenuBuilder::new(app)
        .items(&[
            &new_chat_item,
            &dashboard_item,
            &switch_item,
            &quit_item,
        ])
        .build()?;

    TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("DevEco Switch")
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "new_chat" => {
                focus_main_window(app);
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.emit("tray-new-chat", ());
                }
            }
            "dashboard" => focus_main_window(app),
            "switch" => {
                focus_main_window(app);
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.emit("tray-open-settings", ());
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                focus_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}
