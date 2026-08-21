use tauri::State;

use crate::db::DbState;
use crate::services::extension_governance::ExtensionGovernanceRecord;

#[tauri::command]
pub fn list_extension_governance(
    db: State<DbState>,
    project_id: Option<String>,
) -> Result<Vec<ExtensionGovernanceRecord>, String> {
    let conn = db.0.lock().map_err(|error| error.to_string())?;
    crate::services::extension_governance::list(
        &conn,
        project_id.as_deref().filter(|value| !value.is_empty()),
    )
}

#[tauri::command]
pub fn configure_extension_governance(
    db: State<DbState>,
    extension_kind: String,
    extension_id: String,
    calls_per_minute: i64,
    failure_threshold: i64,
    cooldown_seconds: i64,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|error| error.to_string())?;
    crate::services::extension_governance::configure(
        &conn,
        &extension_kind,
        &extension_id,
        calls_per_minute,
        failure_threshold,
        cooldown_seconds,
    )
}
