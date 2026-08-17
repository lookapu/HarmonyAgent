use serde::Deserialize;
use tauri::State;
use uuid::Uuid;

use crate::db::{
    models::{ProjectMemory, ToolStat, ToolTokenStat},
    queries, DbState,
};

/// 记忆保存入参（新增时无 id；编辑时带 id 走更新）
#[derive(Debug, Deserialize)]
pub struct MemoryInput {
    pub id: Option<String>,
    pub project_id: String,
    /// general|code|build|deploy|decision|pitfall
    pub category: String,
    pub title: String,
    pub content: String,
}

/// 列出项目的全部记忆（按更新时间倒序）
#[tauri::command]
pub fn list_memories(db: State<DbState>, project_id: String) -> Result<Vec<ProjectMemory>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::list_memories(&conn, &project_id).map_err(|e| e.to_string())
}

/// 新增或更新一条记忆（id 为空 = 新增）
#[tauri::command]
pub fn save_memory(db: State<DbState>, input: MemoryInput) -> Result<ProjectMemory, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();

    if let Some(id) = input.id.as_deref().filter(|s| !s.is_empty()) {
        let mut m = queries::list_memories(&conn, &input.project_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|m| m.id == id)
            .ok_or_else(|| "记忆不存在或已删除".to_string())?;
        m.category = input.category;
        m.title = input.title;
        m.content = input.content;
        m.updated_at = now;
        queries::update_memory(&conn, &m).map_err(|e| e.to_string())?;
        Ok(m)
    } else {
        let m = ProjectMemory {
            id: Uuid::new_v4().to_string(),
            project_id: input.project_id,
            category: input.category,
            title: input.title,
            content: input.content,
            enabled: true,
            created_at: now,
            updated_at: now,
        };
        queries::insert_memory(&conn, &m).map_err(|e| e.to_string())?;
        Ok(m)
    }
}

/// 删除一条记忆
#[tauri::command]
pub fn delete_memory(db: State<DbState>, id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::delete_memory(&conn, &id).map_err(|e| e.to_string())
}

/// 启用/禁用记忆（禁用后不再注入，但保留记录）
#[tauri::command]
pub fn set_memory_enabled(db: State<DbState>, id: String, enabled: bool) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::set_memory_enabled(&conn, &id, enabled).map_err(|e| e.to_string())
}

/// 工具调用统计（按工具聚合：次数 / 成功率 / 平均耗时 / 最近调用）
#[tauri::command]
pub fn list_tool_stats(db: State<DbState>, project_id: String) -> Result<Vec<ToolStat>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::list_tool_stats(&conn, &project_id).map_err(|e| e.to_string())
}

/// 工具 token 消耗排行（[69]：request_logs.tool_name 按工具聚合，代理链路口径）
#[tauri::command]
pub fn list_tool_token_stats(db: State<DbState>, days: i64) -> Result<Vec<ToolTokenStat>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::list_tool_token_stats(&conn, days).map_err(|e| e.to_string())
}
