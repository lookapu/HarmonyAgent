use tauri::State;

use crate::db::DbState;
use crate::services::team_sharing::{
    ShareChangeRecord, ShareImportRecord, SharePreview, TeamEvalRun, TeamEvalSetRecord,
    TeamSharePackage, TeamShareSource,
};

#[tauri::command]
pub fn preview_team_share(
    db: State<DbState>,
    project_id: String,
    package: serde_json::Value,
) -> Result<SharePreview, String> {
    let package = crate::services::team_sharing::parse_and_validate(&package)?;
    let conn = db.0.lock().map_err(|error| error.to_string())?;
    crate::services::team_sharing::preview(&conn, &project_id, &package)
}

#[tauri::command]
pub fn apply_team_share(
    db: State<DbState>,
    project_id: String,
    package: serde_json::Value,
) -> Result<ShareImportRecord, String> {
    let package = crate::services::team_sharing::parse_and_validate(&package)?;
    let mut conn = db.0.lock().map_err(|error| error.to_string())?;
    crate::services::team_sharing::apply(&mut conn, &project_id, &package)
}

#[tauri::command]
pub fn revert_team_share(
    db: State<DbState>,
    project_id: String,
    batch_id: String,
) -> Result<usize, String> {
    let mut conn = db.0.lock().map_err(|error| error.to_string())?;
    crate::services::team_sharing::revert(&mut conn, &project_id, &batch_id)
}

#[tauri::command]
pub fn list_team_share_imports(
    db: State<DbState>,
    project_id: String,
) -> Result<Vec<ShareImportRecord>, String> {
    let conn = db.0.lock().map_err(|error| error.to_string())?;
    crate::services::team_sharing::list_imports(&conn, &project_id)
}

#[tauri::command]
pub fn list_team_share_changes(
    db: State<DbState>,
    project_id: String,
    batch_id: String,
) -> Result<Vec<ShareChangeRecord>, String> {
    let conn = db.0.lock().map_err(|error| error.to_string())?;
    crate::services::team_sharing::list_changes(&conn, &project_id, &batch_id)
}

#[tauri::command]
pub fn export_team_share(
    db: State<DbState>,
    project_id: String,
    package_id: String,
    name: String,
    version: String,
    source_uri: String,
    source_revision: String,
) -> Result<TeamSharePackage, String> {
    let conn = db.0.lock().map_err(|error| error.to_string())?;
    crate::services::team_sharing::export(
        &conn,
        &project_id,
        &package_id,
        &name,
        &version,
        TeamShareSource {
            uri: source_uri,
            revision: source_revision,
        },
    )
}

#[tauri::command]
pub fn run_team_eval_set(
    db: State<DbState>,
    project_id: String,
    set_id: String,
) -> Result<TeamEvalRun, String> {
    let conn = db.0.lock().map_err(|error| error.to_string())?;
    crate::services::team_sharing::run_eval_set(&conn, &project_id, &set_id)
}

#[tauri::command]
pub fn list_team_eval_sets(
    db: State<DbState>,
    project_id: String,
) -> Result<Vec<TeamEvalSetRecord>, String> {
    let conn = db.0.lock().map_err(|error| error.to_string())?;
    crate::services::team_sharing::list_eval_sets(&conn, &project_id)
}
