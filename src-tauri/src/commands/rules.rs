//! 指令（Rules）命令：全局指令存 settings 表（key=global_rules），项目级存 projects.rules。
//! 两者在 stream_chat 组装 system_prompt 时注入（全局优先于项目）。
//! 前端 Rules 编辑弹窗读写这些命令。

use crate::db::DbState;
use rusqlite::params;
use tauri::State;

/// 读取全局指令（未配置时返回空串）
#[tauri::command]
pub fn get_global_rules(state: State<'_, DbState>) -> Result<String, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let v: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = 'global_rules'", [], |r| r.get(0))
        .ok();
    Ok(v.unwrap_or_default())
}

/// 保存全局指令（覆盖写入；传空串即清空）
#[tauri::command]
pub fn set_global_rules(rules: String, state: State<'_, DbState>) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('global_rules', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [&rules],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 保存项目级指令（覆盖写入 projects.rules；传空串即清空）
#[tauri::command]
pub fn update_project_rules(project_id: String, rules: String, state: State<'_, DbState>) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE projects SET rules = ?1 WHERE id = ?2",
        params![rules, project_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
