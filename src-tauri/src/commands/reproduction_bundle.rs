use tauri::State;

use crate::db::DbState;
use crate::services::reproduction_bundle::{
    ArchiveValidation, ReproductionBundleRecord, ReproductionPreview,
};

#[tauri::command]
pub fn preview_reproduction_bundle(
    db: State<DbState>,
    project_id: String,
    request: serde_json::Value,
) -> Result<ReproductionPreview, String> {
    let request = crate::services::reproduction_bundle::parse_request(&request)?;
    let conn = db.0.lock().map_err(|error| error.to_string())?;
    crate::services::reproduction_bundle::preview(&conn, &project_id, &request)
}

#[tauri::command]
pub fn generate_reproduction_bundle(
    db: State<DbState>,
    project_id: String,
    request: serde_json::Value,
    confirmed: bool,
    preview_digest: String,
) -> Result<ReproductionBundleRecord, String> {
    let request = crate::services::reproduction_bundle::parse_request(&request)?;
    let mut conn = db.0.lock().map_err(|error| error.to_string())?;
    crate::services::reproduction_bundle::generate(
        &mut conn,
        &project_id,
        &request,
        confirmed,
        &preview_digest,
    )
}

#[tauri::command]
pub fn list_reproduction_bundles(
    db: State<DbState>,
    project_id: String,
) -> Result<Vec<ReproductionBundleRecord>, String> {
    let conn = db.0.lock().map_err(|error| error.to_string())?;
    crate::services::reproduction_bundle::list_records(&conn, &project_id)
}

#[tauri::command]
pub fn validate_reproduction_bundle(
    db: State<DbState>,
    project_id: String,
    bundle_id: String,
) -> Result<ArchiveValidation, String> {
    let conn = db.0.lock().map_err(|error| error.to_string())?;
    crate::services::reproduction_bundle::validate_record_archive(&conn, &project_id, &bundle_id)
}
