//! ohpm 三方库推荐缓存管理命令（官方 landscape 推荐区镜像）
//!
//! 供健康检查页与 Agent 工具使用：状态 / 刷新 / 检索 / 热门 / 分类浏览。
//! 数据离线化后不再依赖 ohpm CLI 与本机网络环境（Agent 断网也能推荐）。

use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use crate::db::DbState;
use crate::services::ohpm_landscape::{
    self, CategoryStat, OhpmPkg, OhpmStatus, RefreshReport,
};

fn conn<'a>(db: &'a State<'_, DbState>) -> Result<std::sync::MutexGuard<'a, Connection>, String> {
    db.0.lock().map_err(|e| e.to_string())
}

/// 缓存状态（包数 / 更新时间 / 一级分类数）
#[tauri::command]
pub fn ohpm_landscape_status(db: State<'_, DbState>) -> Result<OhpmStatus, String> {
    let c = conn(&db)?;
    ohpm_landscape::status(&c)
}

/// 拉取官方接口并全量刷新本地缓存
#[tauri::command]
pub async fn ohpm_landscape_refresh(db: State<'_, DbState>) -> Result<RefreshReport, String> {
    let db_arc = db.0.clone();
    ohpm_landscape::refresh(&db_arc).await
}

/// 关键词检索（包名/描述/关键词/作者/分类）；order 可选排序：likes/popularity/latest（默认下载量）；offset 用于分页
#[tauri::command]
pub fn ohpm_landscape_search(
    db: State<'_, DbState>,
    query: String,
    order: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<OhpmPkg>, String> {
    let c = conn(&db)?;
    ohpm_landscape::search(
        &c,
        &query,
        &order.unwrap_or_default(),
        limit.unwrap_or(20).min(100),
        offset.unwrap_or(0),
    )
}

/// 热门推荐；order 可选排序：likes/popularity/latest（默认下载量）；offset 用于分页
#[tauri::command]
pub fn ohpm_landscape_hot(
    db: State<'_, DbState>,
    order: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<OhpmPkg>, String> {
    let c = conn(&db)?;
    ohpm_landscape::hot(
        &c,
        &order.unwrap_or_default(),
        limit.unwrap_or(20).min(100),
        offset.unwrap_or(0),
    )
}

/// 按分类取包（下载量排序）；level2 非空时进一步按二级分类过滤；order 可选排序：likes/popularity/latest；offset 用于分页
#[tauri::command]
pub fn ohpm_landscape_by_category(
    db: State<'_, DbState>,
    category: String,
    level2: Option<String>,
    order: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<OhpmPkg>, String> {
    let c = conn(&db)?;
    ohpm_landscape::by_category(
        &c,
        &category,
        &level2.unwrap_or_default(),
        &order.unwrap_or_default(),
        limit.unwrap_or(20).min(100),
        offset.unwrap_or(0),
    )
}

/// 统计匹配包数（过滤条件与检索/分类一致），用于页码分页
#[tauri::command]
pub fn ohpm_landscape_count(
    db: State<'_, DbState>,
    query: Option<String>,
    category: Option<String>,
    level2: Option<String>,
) -> Result<i64, String> {
    let c = conn(&db)?;
    ohpm_landscape::count(
        &c,
        &query.unwrap_or_default(),
        &category.unwrap_or_default(),
        &level2.unwrap_or_default(),
    )
}

/// 查询指定包的最新版元数据，返回仓库主页 URL（无仓库则返回 null，由前端回退官网详情页）
#[tauri::command]
pub async fn ohpm_landscape_repo_url(package_name: String) -> Result<Option<String>, String> {
    ohpm_landscape::repo_url(&package_name).await
}

/// 一二级分类树（按包数量降序）
#[derive(Debug, Serialize)]
pub struct CategoryTree {
    pub categories: Vec<CategoryStat>,
}

#[tauri::command]
pub fn ohpm_landscape_categories(db: State<'_, DbState>) -> Result<CategoryTree, String> {
    let c = conn(&db)?;
    Ok(CategoryTree {
        categories: ohpm_landscape::categories(&c)?,
    })
}
