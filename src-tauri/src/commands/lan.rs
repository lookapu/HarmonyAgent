//! 局域网访问命令层：启动/停止/状态/配置/token 重置/IP 枚举。
//! 多开互斥（lan.lock 文件锁）保证同一时刻只有一个实例提供 LAN 服务。

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::db::DbState;
use crate::services::lan_server::{
    LanConfigInput, LanServer, LanTokenInfo, count_tokens, create_token, list_lan_ips, list_tokens,
    load_config, revoke_token,
};

/// LAN 服务器运行时状态（lib.rs setup 注册）
pub struct LanServerState(pub tokio::sync::Mutex<LanServer>);

#[derive(Serialize)]
pub struct LanStatusInfo {
    pub running: bool,
    pub enabled: bool,
    pub listen_port: u16,
    pub read_only: bool,
    pub token_set: bool,
    /// 令牌列表（不含明文，仅供管理展示）
    pub tokens: Vec<LanTokenInfo>,
    /// 本机局域网 IPv4 地址列表（设置页展示访问地址 + 二维码）
    pub ips: Vec<String>,
}

/// 尝试获取 LAN 服务互斥锁（应用数据目录 lan.lock，独占锁定到进程退出或显式释放）。
/// 成功 = 本实例负责启动/停止 LAN 服务；失败 = 其他实例已持有。
pub fn acquire_lan_lock(data_dir: &std::path::Path) -> Option<std::fs::File> {
    use fs2::FileExt;
    let path = data_dir.join("lan.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .ok()?;
    match file.try_lock_exclusive() {
        Ok(()) => Some(file),
        Err(_) => None,
    }
}

/// 启动 LAN 服务（端口被占用自动顺延；成功后持久化 enabled=1 与实际端口）
#[tauri::command]
pub async fn start_lan_server(
    app: AppHandle,
    db: State<'_, DbState>,
    state: State<'_, LanServerState>,
) -> Result<(), String> {
    let config = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        load_config(&conn)
    };
    let has_token = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        count_tokens(&conn) > 0
    };
    if !has_token {
        return Err("尚未创建访问令牌，请先在设置页生成令牌".into());
    }

    // 独立 DB 连接（不与池内连接互相阻塞；代理服务同款做法）
    let db_path = dirs_data_dir().join("deveco-switch.db");
    let lan_conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
    lan_conn
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .map_err(|e| e.to_string())?;
    let lan_db = std::sync::Arc::new(std::sync::Mutex::new(lan_conn));

    let mut server = state.0.lock().await;

    if server.is_running().await {
        return Err("LAN 服务已在运行".into());
    }
    // 多开保护：仅锁持有者启动
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let lock = acquire_lan_lock(&data_dir);
    if lock.is_none() {
        return Err("LAN 服务已在其他应用实例中运行，无需重复启动".into());
    }
    server.set_lock_file(lock);

    if let Err(e) = server.start(app.clone(), lan_db, config.clone()).await {
        // 启动失败释放锁
        server.set_lock_file(None);
        return Err(e);
    }

    // 持久化 enabled=1 + 实际顺延端口（DB 锁不跨 await）
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let _ = conn.execute("UPDATE lan_config SET enabled = 1 WHERE id = 1", []);
    }
    let status = server.get_status().await;
    if status.listen_port != config.port {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let _ = conn.execute(
            "UPDATE lan_config SET port = ?1 WHERE id = 1",
            rusqlite::params![status.listen_port as i32],
        );
    }
    Ok(())
}

/// 停止 LAN 服务（并持久化 enabled=0，释放多开锁）
#[tauri::command]
pub async fn stop_lan_server(
    db: State<'_, DbState>,
    state: State<'_, LanServerState>,
) -> Result<(), String> {
    let mut server = state.0.lock().await;
    server.stop().await?;
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let _ = conn.execute("UPDATE lan_config SET enabled = 0 WHERE id = 1", []);
    }
    Ok(())
}

/// 查询 LAN 服务状态 + 配置（token 明文不回传）
#[tauri::command]
pub async fn get_lan_server_status(
    db: State<'_, DbState>,
    state: State<'_, LanServerState>,
) -> Result<LanStatusInfo, String> {
    let server = state.0.lock().await;
    let status = server.get_status().await;
    let (enabled, read_only, token_set, tokens) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let cfg = load_config(&conn);
        (cfg.enabled, cfg.read_only, count_tokens(&conn) > 0, list_tokens(&conn))
    };
    Ok(LanStatusInfo {
        running: status.running,
        enabled,
        listen_port: if status.running { status.listen_port } else { 0 },
        read_only,
        token_set,
        tokens,
        ips: list_lan_ips(),
    })
}

/// 更新 LAN 服务配置（端口 / 只读模式；开关走 start/stop 命令）
#[tauri::command]
pub fn update_lan_server_config(db: State<DbState>, input: LanConfigInput) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    if let Some(port) = input.port {
        conn.execute(
            "UPDATE lan_config SET port = ?1 WHERE id = 1",
            rusqlite::params![port as i32],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(read_only) = input.read_only {
        conn.execute(
            "UPDATE lan_config SET read_only = ?1 WHERE id = 1",
            rusqlite::params![read_only as i32],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 创建访问令牌（名称 + 有效期）。expires_at = 0 表示永久；否则为到期时间戳。
/// 返回明文仅此一次可见（前端展示后立即清除，库里只存哈希）。
#[tauri::command]
pub fn create_lan_token(db: State<DbState>, name: String, expires_at: i64) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let (_, token) = create_token(&conn, &name, expires_at)?;
    Ok(token)
}

/// 令牌列表（不含明文，供设置页管理展示）
#[tauri::command]
pub fn list_lan_tokens(db: State<DbState>) -> Result<Vec<LanTokenInfo>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(list_tokens(&conn))
}

/// 撤销令牌：删除记录立即失效，并定向断开该令牌的全部 SSE 连接（网页端回登录页）
#[tauri::command]
pub async fn revoke_lan_token(
    db: State<'_, DbState>,
    state: State<'_, LanServerState>,
    id: i64,
) -> Result<(), String> {
    let hash = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        revoke_token(&conn, id)?
    };
    let server = state.0.lock().await;
    server.disconnect_token(&hash);
    Ok(())
}

/// 枚举本机局域网 IPv4 地址（设置页访问地址 + 二维码用）
#[tauri::command]
pub fn get_lan_ips() -> Vec<String> {
    list_lan_ips()
}

fn dirs_data_dir() -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        std::path::PathBuf::from(home).join("Library/Application Support/com.deveco-switch.app")
    }
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").unwrap_or_else(|_| "C:\\".to_string());
        std::path::PathBuf::from(appdata).join("com.deveco-switch.app")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        std::path::PathBuf::from(home).join(".local/share/com.deveco-switch.app")
    }
}
