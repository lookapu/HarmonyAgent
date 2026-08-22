//! 鸿蒙官方 API 知识库管理命令
//!
//! 提供给前端"知识库管理"页面使用的 CRUD + 分页 + 筛选 + 刷新能力。
//! 数据分两层：
//! - `api_docs`：版本 diff 记录（哪个 API 在哪一版 added/removed/modified/deprecated）
//! - `api_details` / `api_members`：参考正文详情（描述、参数、示例、成员列表）

use std::collections::HashMap;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::db::DbState;
use crate::services::harmony_api_diff::{
    self, ApiEntry, RefreshProgress as DiffProgress, RefreshReport as DiffReport,
};
use crate::services::harmony_api_ref::{
    self, RefProgress, RefReport,
};

// ───────────────────────── 统计概览 ─────────────────────────

#[derive(Debug, Serialize)]
pub struct ApiKbStats {
    pub docs_total: i64,
    pub details_total: i64,
    pub members_total: i64,
    pub versions: Vec<VersionStat>,
    pub kits: Vec<KitStat>,
    pub change_types: Vec<ChangeTypeStat>,
    pub last_refreshed_at: Option<i64>,
    pub last_refreshed_entries: i64,
}

#[derive(Debug, Serialize)]
pub struct VersionStat {
    pub version_label: String,
    pub api_level: Option<i64>,
    pub total: i64,
    pub added: i64,
    pub removed: i64,
    pub modified: i64,
    pub deprecated: i64,
}

#[derive(Debug, Serialize)]
pub struct KitStat {
    pub kit: String,
    pub total: i64,
}

#[derive(Debug, Serialize)]
pub struct ChangeTypeStat {
    pub change_type: String,
    pub total: i64,
}

#[tauri::command]
pub fn api_kb_stats(db: State<DbState>) -> Result<ApiKbStats, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    let docs_total: i64 = conn
        .query_row("SELECT COUNT(*) FROM api_docs", [], |r| r.get(0))
        .unwrap_or(0);
    let details_total: i64 = conn
        .query_row("SELECT COUNT(*) FROM api_details", [], |r| r.get(0))
        .unwrap_or(0);
    let members_total: i64 = conn
        .query_row("SELECT COUNT(*) FROM api_members", [], |r| r.get(0))
        .unwrap_or(0);

    let mut stmt = conn
        .prepare(
            "SELECT version_label, api_level,
                    COUNT(*) AS total,
                    SUM(CASE WHEN change_type='added' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN change_type='removed' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN change_type='modified' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN change_type='deprecated' THEN 1 ELSE 0 END)
             FROM api_docs
             GROUP BY version_label
             ORDER BY MAX(api_level) DESC, version_label DESC",
        )
        .map_err(|e| e.to_string())?;
    let versions = stmt
        .query_map([], |r| {
            Ok(VersionStat {
                version_label: r.get(0)?,
                api_level: r.get(1)?,
                total: r.get(2)?,
                added: r.get(3)?,
                removed: r.get(4)?,
                modified: r.get(5)?,
                deprecated: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();

    let mut stmt = conn
        .prepare(
            "SELECT kit, COUNT(*) FROM api_docs
             GROUP BY kit ORDER BY COUNT(*) DESC LIMIT 50",
        )
        .map_err(|e| e.to_string())?;
    let kits = stmt
        .query_map([], |r| {
            Ok(KitStat {
                kit: r.get(0)?,
                total: r.get(1)?,
            })
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();

    let mut stmt = conn
        .prepare(
            "SELECT change_type, COUNT(*) FROM api_docs
             GROUP BY change_type ORDER BY COUNT(*) DESC",
        )
        .map_err(|e| e.to_string())?;
    let change_types = stmt
        .query_map([], |r| {
            Ok(ChangeTypeStat {
                change_type: r.get(0)?,
                total: r.get(1)?,
            })
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();

    let last_refreshed_at: Option<i64> = conn
        .query_row(
            "SELECT value FROM api_docs_meta WHERE key='last_refreshed_at'",
            [],
            |r| r.get(0),
        )
        .ok()
        .and_then(|v: String| v.parse().ok());
    let last_refreshed_entries: i64 = conn
        .query_row(
            "SELECT value FROM api_docs_meta WHERE key='last_refreshed_entries'",
            [],
            |r| r.get(0),
        )
        .ok()
        .and_then(|v: String| v.parse().ok())
        .unwrap_or(0);

    Ok(ApiKbStats {
        docs_total,
        details_total,
        members_total,
        versions,
        kits,
        change_types,
        last_refreshed_at,
        last_refreshed_entries,
    })
}

// ───────────────────────── api_docs 分页查询 ─────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocsQuery {
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub kit: Option<String>,
    #[serde(default)]
    pub version_label: Option<String>,
    #[serde(default)]
    pub api_level: Option<i64>,
    #[serde(default)]
    pub change_type: Option<String>,
    #[serde(default = "default_page")]
    pub page: i64,
    #[serde(default = "default_page_size")]
    pub page_size: i64,
}

fn default_page() -> i64 {
    1
}
fn default_page_size() -> i64 {
    50
}

#[derive(Debug, Serialize)]
pub struct DocsPage {
    pub items: Vec<ApiEntry>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

#[tauri::command]
pub fn api_docs_list(
    db: State<DbState>,
    query: DocsQuery,
) -> Result<DocsPage, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let page = query.page.max(1);
    let page_size = query.page_size.clamp(1, 500);
    let offset = (page - 1) * page_size;

    let mut where_clauses: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(kw) = &query.keyword {
        let kw = kw.trim();
        if !kw.is_empty() {
            where_clauses.push(
                "(declaration LIKE ?1 OR api_name LIKE ?1 OR module LIKE ?1 OR class_name LIKE ?1)"
                    .to_string(),
            );
            params.push(Box::new(format!("%{kw}%")));
        }
    }
    if let Some(m) = &query.module {
        if !m.trim().is_empty() {
            let idx = params.len() + 1;
            where_clauses.push(format!("module LIKE ?{idx}"));
            params.push(Box::new(format!("%{}%", m.trim())));
        }
    }
    if let Some(k) = &query.kit {
        if !k.trim().is_empty() {
            let idx = params.len() + 1;
            where_clauses.push(format!("kit = ?{idx}"));
            params.push(Box::new(k.trim().to_string()));
        }
    }
    if let Some(v) = &query.version_label {
        if !v.trim().is_empty() {
            let idx = params.len() + 1;
            where_clauses.push(format!("version_label = ?{idx}"));
            params.push(Box::new(v.trim().to_string()));
        }
    }
    if let Some(lvl) = query.api_level {
        let idx = params.len() + 1;
        where_clauses.push(format!("api_level = ?{idx}"));
        params.push(Box::new(lvl));
    }
    if let Some(ct) = &query.change_type {
        if !ct.trim().is_empty() {
            let idx = params.len() + 1;
            where_clauses.push(format!("change_type = ?{idx}"));
            params.push(Box::new(ct.trim().to_string()));
        }
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    let count_sql = format!("SELECT COUNT(*) FROM api_docs {where_sql}");
    let total: i64 = conn
        .query_row(&count_sql, rusqlite::params_from_iter(params.iter()), |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;

    let data_sql = format!(
        "SELECT id, kit, dts_file, module, class_name, declaration, api_name,
                change_type, version_label, api_level, old_declaration, source_url
         FROM api_docs {where_sql}
         ORDER BY COALESCE(api_level, 0) DESC, kit, declaration
         LIMIT ? OFFSET ?"
    );
    params.push(Box::new(page_size));
    params.push(Box::new(offset));

    let mut stmt = conn.prepare(&data_sql).map_err(|e| e.to_string())?;
    let items = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |r| {
            Ok(ApiEntry {
                id: r.get(0)?,
                kit: r.get(1)?,
                dts_file: r.get(2)?,
                module: r.get(3)?,
                class_name: r.get(4)?,
                declaration: r.get(5)?,
                api_name: r.get(6)?,
                change_type: r.get(7)?,
                version_label: r.get(8)?,
                api_level: r.get(9)?,
                old_declaration: r.get(10)?,
                source_url: r.get(11)?,
            })
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();

    Ok(DocsPage {
        items,
        total,
        page,
        page_size,
    })
}

// ───────────────────────── api_details 分页查询 ─────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailsQuery {
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub kit: Option<String>,
    #[serde(default)]
    pub since_api_level: Option<i64>,
    #[serde(default)]
    pub include_deprecated: Option<bool>,
    #[serde(default = "default_page")]
    pub page: i64,
    #[serde(default = "default_page_size")]
    pub page_size: i64,
}

#[derive(Debug, Serialize)]
pub struct DetailListItem {
    pub module: String,
    pub slug: String,
    pub title: Option<String>,
    pub kit: Option<String>,
    pub since_api_level: Option<i64>,
    pub deprecated: bool,
    pub has_import: bool,
    pub has_examples: bool,
    pub member_count: i64,
    pub source_url: String,
}

#[derive(Debug, Serialize)]
pub struct DetailsPage {
    pub items: Vec<DetailListItem>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

#[tauri::command]
pub fn api_details_list(
    db: State<DbState>,
    query: DetailsQuery,
) -> Result<DetailsPage, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let page = query.page.max(1);
    let page_size = query.page_size.clamp(1, 500);
    let offset = (page - 1) * page_size;

    let mut where_clauses: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(kw) = &query.keyword {
        let kw = kw.trim();
        if !kw.is_empty() {
            let idx = params.len() + 1;
            where_clauses.push(format!(
                "(d.module LIKE ?{idx} OR d.title LIKE ?{idx} OR d.body LIKE ?{idx})"
            ));
            params.push(Box::new(format!("%{kw}%")));
        }
    }
    if let Some(m) = &query.module {
        if !m.trim().is_empty() {
            let idx = params.len() + 1;
            where_clauses.push(format!("d.module LIKE ?{idx}"));
            params.push(Box::new(format!("%{}%", m.trim())));
        }
    }
    if let Some(k) = &query.kit {
        if !k.trim().is_empty() {
            let idx = params.len() + 1;
            where_clauses.push(format!("d.kit = ?{idx}"));
            params.push(Box::new(k.trim().to_string()));
        }
    }
    if let Some(lvl) = query.since_api_level {
        let idx = params.len() + 1;
        where_clauses.push(format!("d.since_api_level <= ?{idx}"));
        params.push(Box::new(lvl));
    }
    if !query.include_deprecated.unwrap_or(true) {
        where_clauses.push("d.deprecated = 0".to_string());
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    let count_sql = format!("SELECT COUNT(*) FROM api_details d {where_sql}");
    let total: i64 = conn
        .query_row(&count_sql, rusqlite::params_from_iter(params.iter()), |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;

    let data_sql = format!(
        "SELECT d.module, d.slug, d.title, d.kit, d.since_api_level,
                d.deprecated, d.import_snippet IS NOT NULL,
                d.examples IS NOT NULL, d.source_url,
                (SELECT COUNT(*) FROM api_members m WHERE m.detail_slug = d.slug)
         FROM api_details d
         {where_sql}
         ORDER BY d.module
         LIMIT ? OFFSET ?"
    );
    params.push(Box::new(page_size));
    params.push(Box::new(offset));

    let mut stmt = conn.prepare(&data_sql).map_err(|e| e.to_string())?;
    let items = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |r| {
            Ok(DetailListItem {
                module: r.get(0)?,
                slug: r.get(1)?,
                title: r.get(2)?,
                kit: r.get(3)?,
                since_api_level: r.get(4)?,
                deprecated: r.get::<_, i64>(5)? != 0,
                has_import: r.get::<_, i64>(6)? != 0,
                has_examples: r.get::<_, i64>(7)? != 0,
                source_url: r.get(8)?,
                member_count: r.get(9)?,
            })
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();

    Ok(DetailsPage {
        items,
        total,
        page,
        page_size,
    })
}

// ───────────────────────── 详情（含成员） ─────────────────────────

#[derive(Debug, Serialize)]
pub struct ApiDetailFull {
    pub module: String,
    pub slug: String,
    pub title: Option<String>,
    pub kit: Option<String>,
    pub since_api_level: Option<i64>,
    pub deprecated: bool,
    pub import_snippet: Option<String>,
    pub syscap: Option<String>,
    pub permissions: Option<String>,
    pub device_types: Option<String>,
    pub body: String,
    pub examples: Option<String>,
    pub source_url: String,
    pub fetched_at: i64,
    pub members: Vec<ApiMemberItem>,
}

#[derive(Debug, Serialize)]
pub struct ApiMemberItem {
    pub parent_name: Option<String>,
    pub member_name: String,
    pub kind: String,
    pub declaration: Option<String>,
    pub description: Option<String>,
    pub since_api_level: Option<i64>,
    pub deprecated: bool,
    pub syscap: Option<String>,
    pub permission: Option<String>,
}

#[tauri::command]
pub fn api_detail_get(
    db: State<DbState>,
    slug: String,
) -> Result<ApiDetailFull, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    let detail = conn
        .query_row(
            "SELECT module, slug, title, kit, since_api_level, deprecated,
                    import_snippet, syscap, permissions, device_types,
                    body, examples, source_url, fetched_at
             FROM api_details WHERE slug = ?1",
            [&slug],
            |r| {
                Ok(ApiDetailFull {
                    module: r.get(0)?,
                    slug: r.get(1)?,
                    title: r.get(2)?,
                    kit: r.get(3)?,
                    since_api_level: r.get(4)?,
                    deprecated: r.get::<_, i64>(5)? != 0,
                    import_snippet: r.get(6)?,
                    syscap: r.get(7)?,
                    permissions: r.get(8)?,
                    device_types: r.get(9)?,
                    body: r.get(10)?,
                    examples: r.get(11)?,
                    source_url: r.get(12)?,
                    fetched_at: r.get(13)?,
                    members: Vec::new(),
                })
            },
        )
        .map_err(|e| format!("未找到详情: {e}"))?;

    let mut stmt = conn
        .prepare(
            "SELECT parent_name, member_name, kind, declaration, description,
                    since_api_level, deprecated, syscap, permission
             FROM api_members WHERE detail_slug = ?1
             ORDER BY parent_name, kind, member_name",
        )
        .map_err(|e| e.to_string())?;
    let members = stmt
        .query_map([&slug], |r| {
            Ok(ApiMemberItem {
                parent_name: r.get(0)?,
                member_name: r.get(1)?,
                kind: r.get(2)?,
                declaration: r.get(3)?,
                description: r.get(4)?,
                since_api_level: r.get(5)?,
                deprecated: r.get::<_, i64>(6)? != 0,
                syscap: r.get(7)?,
                permission: r.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();

    Ok(ApiDetailFull {
        members,
        ..detail
    })
}

// ───────────────────────── 删除 ─────────────────────────

#[tauri::command]
pub fn api_doc_delete(db: State<DbState>, id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM api_docs WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn api_detail_delete(db: State<DbState>, slug: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM api_members WHERE detail_slug = ?1", [&slug])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM api_details WHERE slug = ?1", [&slug])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 清空全部 API 知识库（不可恢复，前端需二次确认）
#[tauri::command]
pub fn api_kb_clear(db: State<DbState>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute_batch(
        "DELETE FROM api_members;
         DELETE FROM api_details;
         DELETE FROM api_docs;
         DELETE FROM api_docs_meta;",
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ───────────────────────── 手动新增 / 编辑 ─────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocInput {
    pub kit: String,
    #[serde(default)]
    pub dts_file: Option<String>,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub class_name: Option<String>,
    pub declaration: String,
    #[serde(default)]
    pub api_name: Option<String>,
    pub change_type: String,
    pub version_label: String,
    #[serde(default)]
    pub api_level: Option<i64>,
    #[serde(default)]
    pub old_declaration: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
}

#[tauri::command]
pub fn api_doc_add(
    db: State<DbState>,
    input: DocInput,
) -> Result<i64, String> {
    if input.kit.trim().is_empty() || input.declaration.trim().is_empty() {
        return Err("Kit 和声明不能为空".into());
    }
    let now = chrono::Utc::now().timestamp();
    let source_url = input
        .source_url
        .unwrap_or_else(|| "local://custom".to_string());
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO api_docs
            (kit, dts_file, module, class_name, declaration, api_name,
             change_type, version_label, api_level, old_declaration,
             source_url, fetched_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        rusqlite::params![
            input.kit.trim(),
            input.dts_file,
            input.module,
            input.class_name,
            input.declaration.trim(),
            input.api_name,
            input.change_type,
            input.version_label,
            input.api_level,
            input.old_declaration,
            source_url,
            now,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailInput {
    pub module: String,
    pub title: Option<String>,
    pub kit: Option<String>,
    pub since_api_level: Option<i64>,
    #[serde(default)]
    pub deprecated: bool,
    pub import_snippet: Option<String>,
    pub syscap: Option<String>,
    pub permissions: Option<String>,
    pub device_types: Option<String>,
    pub body: String,
    pub examples: Option<String>,
    pub source_url: String,
}

#[tauri::command]
pub fn api_detail_upsert(
    db: State<DbState>,
    input: DetailInput,
) -> Result<(), String> {
    if input.module.trim().is_empty() || input.body.trim().is_empty() {
        return Err("模块名和正文不能为空".into());
    }
    let now = chrono::Utc::now().timestamp();
    let slug = harmony_api_ref::module_to_slug(&input.module);
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO api_details
            (module, slug, title, kit, since_api_level, deprecated,
             import_snippet, syscap, permissions, device_types,
             body, examples, members, source_url, fetched_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'[]',?13,?14)
         ON CONFLICT(slug) DO UPDATE SET
            module=excluded.module, title=excluded.title, kit=excluded.kit,
            since_api_level=excluded.since_api_level,
            deprecated=excluded.deprecated,
            import_snippet=excluded.import_snippet,
            syscap=excluded.syscap, permissions=excluded.permissions,
            device_types=excluded.device_types, body=excluded.body,
            examples=excluded.examples, source_url=excluded.source_url,
            fetched_at=excluded.fetched_at",
        rusqlite::params![
            input.module.trim(),
            slug,
            input.title,
            input.kit,
            input.since_api_level,
            input.deprecated as i64,
            input.import_snippet,
            input.syscap,
            input.permissions,
            input.device_types,
            input.body,
            input.examples,
            input.source_url,
            now,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ───────────────────────── 筛选元数据 ─────────────────────────

fn query_string_vec(conn: &Connection, sql: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for v in rows.flatten() {
        out.push(v);
    }
    Ok(out)
}

#[tauri::command]
pub fn api_kb_filters(db: State<DbState>) -> Result<HashMap<String, Vec<String>>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut out = HashMap::new();

    out.insert(
        "kits".to_string(),
        query_string_vec(&conn, "SELECT DISTINCT kit FROM api_docs ORDER BY kit")?,
    );
    out.insert(
        "versions".to_string(),
        query_string_vec(
            &conn,
            "SELECT DISTINCT version_label FROM api_docs
             ORDER BY COALESCE(api_level,0) DESC, version_label DESC",
        )?,
    );
    out.insert(
        "modules".to_string(),
        query_string_vec(
            &conn,
            "SELECT DISTINCT module FROM api_docs
             WHERE module IS NOT NULL ORDER BY module",
        )?,
    );
    out.insert(
        "detail_kits".to_string(),
        query_string_vec(
            &conn,
            "SELECT DISTINCT kit FROM api_details
             WHERE kit IS NOT NULL ORDER BY kit",
        )?,
    );

    Ok(out)
}

// ───────────────────────── 在线刷新 ─────────────────────────

/// 从华为官网抓取最新版本 diff。进度通过 `api-refresh-progress` 事件推送。
#[tauri::command]
pub async fn api_kb_refresh_docs(
    app: AppHandle,
    db: State<'_, DbState>,
) -> Result<DiffReport, String> {
    let app_clone = app.clone();
    let cb: harmony_api_diff::ProgressCb = Box::new(move |p: &DiffProgress| {
        let _ = app_clone.emit("api-refresh-progress", p);
    });
    harmony_api_diff::refresh_all(&db, Some(cb)).await
}

/// 从华为官网抓取最新参考正文。进度通过 `api-details-progress` 事件推送。
#[tauri::command]
pub async fn api_kb_refresh_details(
    app: AppHandle,
    db: State<'_, DbState>,
) -> Result<RefReport, String> {
    let app_clone = app.clone();
    let cb: harmony_api_ref::ProgressCb = Box::new(move |p: &RefProgress| {
        let _ = app_clone.emit("api-details-progress", p);
    });
    harmony_api_ref::refresh_all(&db, Some(cb)).await
}

// ───────────────────────── 语义向量索引 ─────────────────────────

/// 建索引任务运行标志（防并发重建）
static EMBED_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Debug, Serialize)]
pub struct ApiKbEmbedStatus {
    /// 语义模型是否可用（未启用 embedding feature 或模型文件缺失均为 false）
    pub available: bool,
    pub model: Option<String>,
    /// 已索引条数
    pub indexed: i64,
    /// 知识库总条数
    pub total: i64,
    /// 是否有建索引任务在后台运行
    pub running: bool,
}

/// 查询向量索引状态（模型可用性 + 覆盖进度 + 运行标志）
#[tauri::command]
pub fn api_kb_embed_status(db: State<DbState>) -> Result<ApiKbEmbedStatus, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let indexed: i64 = conn
        .query_row("SELECT COUNT(*) FROM api_docs_embeddings", [], |r| r.get(0))
        .unwrap_or(0);
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM api_docs", [], |r| r.get(0))
        .unwrap_or(0);
    drop(conn);

    #[cfg(feature = "embedding")]
    let available = crate::services::embedding::global_embedder().is_some();
    #[cfg(not(feature = "embedding"))]
    let available = false;

    Ok(ApiKbEmbedStatus {
        available,
        model: if available {
            Some("bge-small-zh-v1.5".to_string())
        } else {
            None
        },
        indexed,
        total,
        running: EMBED_RUNNING.load(std::sync::atomic::Ordering::SeqCst),
    })
}

/// 后台重建语义向量索引：进度经 `api-embed-progress` 事件推送，完成发 `api-embed-done`。
/// 分批游标处理 + 每批短暂持锁，不阻塞其他数据库操作；防并发重入。
/// 幂等：已按当前模型全量建过则直接跳过；知识库数据变化（刷新/增删）后全量重建。
#[tauri::command]
pub fn api_kb_embed_index(app: AppHandle, db: State<'_, DbState>) -> Result<(), String> {
    #[cfg(not(feature = "embedding"))]
    {
        let _ = (&app, &db);
        Err("当前构建未启用语义检索（embedding feature），无法建索引".into())
    }

    #[cfg(feature = "embedding")]
    {
        if EMBED_RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Err("已有语义索引任务在运行中，请稍候".into());
        }
        let state = db.0.clone();
        let app_clone = app.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let app_emit = app_clone.clone();
            let cb: crate::services::embedding::EmbedProgressCb = Box::new(move |p| {
                let _ = app_emit.emit("api-embed-progress", p);
            });
            let result = crate::services::embedding::build_index_streaming(&state, 128, Some(cb));
            let elapsed = started.elapsed().as_secs();
            let payload = match &result {
                Ok((inserted, skipped)) => serde_json::json!({
                    "ok": true, "indexed": inserted, "skipped": skipped, "elapsed": elapsed
                }),
                Err(e) => serde_json::json!({
                    "ok": false, "error": e, "elapsed": elapsed
                }),
            };
            let _ = app_clone.emit("api-embed-done", payload);
            EMBED_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
        });
        Ok(())
    }
}

