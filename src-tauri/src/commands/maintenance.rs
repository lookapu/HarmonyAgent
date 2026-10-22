//! 数据维护命令：滚动清理 + 一键清空内容数据 + 查看数据规模 + 导出备份。

use std::path::PathBuf;

use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::chat::{self, ChatCancel, ChatLock};
use crate::db::DbState;
use crate::services::maintenance;
use crate::utils::task_registry::TaskRegistry;

fn conversation_ids(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM conversations")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    rows.collect()
}

/// 导出数据库完整备份快照（VACUUM INTO 到目标目录，不影响运行中的库）。
/// dest 缺省时导出到应用数据目录 backups 子目录。
/// 返回 "路径|大小(字节)"（供前端展示）。
#[tauri::command]
pub fn export_backup(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    dest: Option<String>,
) -> Result<String, String> {
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dest = match dest {
        Some(p) => {
            let dir = PathBuf::from(p);
            if !dir.is_dir() {
                return Err(format!("目标目录不存在：{}", dir.display()));
            }
            dir
        }
        None => {
            app.path()
                .app_data_dir()
                .map_err(|e| e.to_string())?
                .join("backups")
        }
    };
    std::fs::create_dir_all(&dest).map_err(|e| format!("创建备份目录失败：{e}"))?;
    let file = dest.join(format!("deveco-backup-{ts}.db"));
    if file.exists() {
        return Err(format!(
            "备份文件已存在（同一秒内重复导出）：{}，请稍后重试",
            file.display()
        ));
    }
    // VACUUM INTO 路径不能参数化，转义单引号防注入
    let path_escaped = file.to_string_lossy().replace('\'', "''");
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    conn.execute_batch(&format!("VACUUM INTO '{path_escaped}';"))
        .map_err(|e| format!("导出备份失败：{e}（备份文件可能残留，可手动删除）"))?;
    drop(conn);
    let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    Ok(format!("{}|{}", file.display(), size))
}

/// 一键清空"内容类"数据：会话、消息、反馈、版本、任务/工具轨迹、请求日志、项目记忆、
/// 审批白名单。保留配置类（providers/models/mcp/skills/权限）与 API 知识库。
///
/// 清库前先停止所有会话的运行中任务：否则任务会继续消耗 token，并在会话被删后
/// 往 session_events/task_runs 写入孤儿行（下一次启动才被清扫）。
///
/// 返回被删除的会话数与消息数（供前端展示）。
#[tauri::command]
pub fn clear_content_data(
    app: AppHandle,
    state: State<'_, DbState>,
) -> Result<(u64, u64), String> {
    // 先取全部会话 id（随后会删掉，必须先取）；读取失败就不清，避免无 id 可停。
    let ids = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conversation_ids(&conn).map_err(|e| e.to_string())?
    };
    let cancel = app.state::<ChatCancel>();
    let lock = app.state::<ChatLock>();
    let registry = app.state::<TaskRegistry>();
    let mut stop_failures = 0usize;
    for id in &ids {
        if let Err(error) = chat::stop_conversation_runtime(
            id,
            &state,
            &cancel,
            &lock,
            &registry,
            "content_cleared",
        ) {
            // 审批持久撤销失败不阻塞清空：tool_runs/run_events 随后会被整体清空，
            // 活动调用已 abort，不存在可继续执行的凭据。
            stop_failures += 1;
            crate::utils::logger::log_event(
                "conversation_runtime_stop_failed",
                serde_json::json!({
                    "conversation_id": id,
                    "source": "clear_content_data",
                    "error": error,
                }),
            );
        }
    }

    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let (convs, msgs) = maintenance::clear_content_data(&mut conn).map_err(|e| e.to_string())?;
    // 大范围删除后回收文件空间，避免文件继续占盘
    let _ = maintenance::vacuum(&conn);
    drop(conn);
    // 通知前端：全部会话已删除（流式桶/当前会话/审批卡等必须一起复位）。
    let _ = app.emit(
        "conversations-cleared",
        serde_json::json!({
            "conversation_ids": ids,
            "stop_failures": stop_failures,
        }),
    );
    // 会话已全部删除，生成产物（图片/视频/音频）一并回收；失败只记日志，
    // 下次启动的孤儿清扫会兜底。
    if let Ok(data_dir) = app.path().app_data_dir() {
        match crate::utils::generated_media::remove_all(&data_dir) {
            Ok(true) => crate::utils::logger::log_event(
                "generated_media_removed",
                serde_json::json!({ "source": "clear_content_data" }),
            ),
            Ok(false) => {}
            Err(error) => crate::utils::logger::log_event(
                "generated_media_cleanup_failed",
                serde_json::json!({
                    "source": "clear_content_data",
                    "error": error.to_string(),
                }),
            ),
        }
    }
    Ok((convs, msgs))
}

/// 立即执行一次滚动清理（保留策略见 maintenance 常量）。
/// 返回 (清理请求日志条数, 清理成本明细条数)。
#[tauri::command]
pub fn run_maintenance(state: State<'_, DbState>) -> Result<(usize, usize), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let logs = maintenance::prune_request_logs(&conn, maintenance::REQUEST_LOG_KEEP);
    let runs = maintenance::prune_task_runs(&conn, maintenance::TASK_RUN_KEEP_DAYS);
    if logs > 0 || runs > 0 {
        let _ = maintenance::vacuum(&conn);
    }
    Ok((logs, runs))
}

/// 数据规模统计（供前端"数据管理"区展示当前体量，提示是否值得清理）。
#[derive(serde::Serialize)]
pub struct DataScale {
    pub conversations: i64,
    pub messages: i64,
    pub request_logs: i64,
    pub task_runs: i64,
    pub project_memories: i64,
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0)
}

#[tauri::command]
pub fn data_scale(state: State<'_, DbState>) -> Result<DataScale, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    Ok(DataScale {
        conversations: count(&conn, "SELECT COUNT(*) FROM conversations"),
        messages: count(&conn, "SELECT COUNT(*) FROM messages"),
        request_logs: count(&conn, "SELECT COUNT(*) FROM request_logs"),
        task_runs: count(&conn, "SELECT COUNT(*) FROM task_runs"),
        project_memories: count(&conn, "SELECT COUNT(*) FROM project_memories"),
    })
}
