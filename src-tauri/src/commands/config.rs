use crate::services::config_service;

#[tauri::command]
pub fn read_config() -> Result<serde_json::Value, String> {
    config_service::read_deveco_config().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn write_config(config: serde_json::Value) -> Result<(), String> {
    config_service::write_deveco_config(&config).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_config_path() -> Result<String, String> {
    let path = config_service::get_config_path();
    Ok(path.to_string_lossy().to_string())
}
