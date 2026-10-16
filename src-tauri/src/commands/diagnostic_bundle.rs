use tauri::State;

use crate::db::DbState;
use crate::services::diagnostic_bundle::{
    DiagnosticBundleRecord, DiagnosticPreview,
};

/// 预览诊断包内容：返回将要写出的条目、字节数、各来源条数与摘要。
/// 不落盘。导出必须先拿到这里的 `preview_digest`。
#[tauri::command]
pub fn preview_diagnostic_bundle(db: State<DbState>) -> Result<DiagnosticPreview, String> {
    let conn = db.0.lock().map_err(|error| error.to_string())?;
    crate::services::diagnostic_bundle::preview(&conn)
}

/// 导出诊断包到 `dest` 目录。必须 `confirmed = true` 且 `preview_digest` 与当场重算一致。
#[tauri::command]
pub fn generate_diagnostic_bundle(
    db: State<DbState>,
    dest: String,
    confirmed: bool,
    preview_digest: String,
) -> Result<DiagnosticBundleRecord, String> {
    let conn = db.0.lock().map_err(|error| error.to_string())?;
    crate::services::diagnostic_bundle::generate(&conn, &dest, confirmed, &preview_digest)
}
