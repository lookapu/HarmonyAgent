use serde::Deserialize;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use crate::db::DbState;
use crate::services::proxy_service::{ProxyConfig, ProxyStatus};

pub struct ProxyState(pub tokio::sync::Mutex<crate::services::proxy_service::ProxyServer>);

/// 代理互斥锁：多开时仅持锁实例负责启动/停止本地代理，其余实例共享不重复启动
pub struct ProxyLock(pub tokio::sync::Mutex<Option<std::fs::File>>);

/// 尝试获取代理互斥锁（应用数据目录 proxy.lock，独占锁定到进程退出或显式释放）。
/// 成功 = 本实例负责启动/停止代理；失败 = 其他实例已持有（代理已在运行）。
pub fn acquire_proxy_lock(data_dir: &std::path::Path) -> Option<std::fs::File> {
    use fs2::FileExt;
    let path = data_dir.join("proxy.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        // 锁文件不截断：内容无关紧要，只依赖文件存在与独占锁
        .truncate(false)
        .open(&path)
        .ok()?;
    match file.try_lock_exclusive() {
        Ok(()) => Some(file),
        Err(_) => None,
    }
}

#[derive(Debug, Deserialize)]
pub struct ProxyConfigInput {
    pub listen_address: Option<String>,
    pub listen_port: Option<u16>,
    pub auto_failover: Option<bool>,
    pub max_retries: Option<u32>,
    pub streaming_first_byte_timeout_s: Option<u64>,
    pub non_streaming_timeout_s: Option<u64>,
    /// 是否随应用启动自动开启代理（enabled=1）
    pub enabled: Option<bool>,
}

/// 完整代理配置（含自动启动开关），供前端展示
#[derive(Debug, Serialize)]
pub struct ProxyConfigInfo {
    pub enabled: bool,
    pub listen_address: String,
    pub listen_port: u16,
    pub auto_failover: bool,
    pub max_retries: u32,
    pub streaming_first_byte_timeout_s: u64,
    pub non_streaming_timeout_s: u64,
}

fn read_config(db: &State<DbState>) -> Result<ProxyConfig, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(conn
        .query_row(
            "SELECT listen_address, listen_port, auto_failover, max_retries,
                    streaming_first_byte_timeout_s, non_streaming_timeout_s
             FROM proxy_config WHERE id = 1",
            [],
            |row| {
                Ok(ProxyConfig {
                    listen_address: row.get(0)?,
                    listen_port: row.get::<_, i32>(1)? as u16,
                    auto_failover: row.get::<_, i32>(2)? != 0,
                    max_retries: row.get::<_, i32>(3)? as u32,
                    streaming_first_byte_timeout_s: row.get::<_, i32>(4)? as u64,
                    non_streaming_timeout_s: row.get::<_, i32>(5)? as u64,
                })
            },
        )
        .unwrap_or_default())
}

#[tauri::command]
pub async fn start_proxy(
    app: AppHandle,
    db: State<'_, DbState>,
    proxy: State<'_, ProxyState>,
    lock: State<'_, ProxyLock>,
) -> Result<(), String> {
    // 多开保护：其他实例已持有代理锁时，不重复启动（共享其代理）
    {
        let mut held = lock.0.lock().await;
        if held.is_none() {
            let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
            *held = acquire_proxy_lock(&data_dir);
            if held.is_none() {
                return Err("本地代理已在其他应用实例中运行，无需重复启动".into());
            }
        }
    }

    let config = read_config(&db)?;

    let db_path = dirs_data_dir().join("deveco-switch.db");
    let proxy_conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
    proxy_conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;").map_err(|e| e.to_string())?;
    let proxy_db = std::sync::Arc::new(std::sync::Mutex::new(proxy_conn));

    let mut server = proxy.0.lock().await;
    server.start(proxy_db, config).await?;

    // 手动启动成功后自动记住"随应用启动自动开启"，下次打开应用即自动启动
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let _ = conn.execute("UPDATE proxy_config SET enabled = 1 WHERE id = 1", []);
    }

    // 端口被占用自动顺延后，把实际端口写回配置（下次启动沿用）
    let status = server.get_status().await;
    if status.listen_port != read_config(&db)?.listen_port {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE proxy_config SET listen_port = ?1 WHERE id = 1",
            rusqlite::params![status.listen_port as i32],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn stop_proxy(proxy: State<'_, ProxyState>, lock: State<'_, ProxyLock>) -> Result<(), String> {
    let mut server = proxy.0.lock().await;
    server.stop().await?;
    // 释放代理锁，允许后续实例接管
    let mut held = lock.0.lock().await;
    *held = None;
    Ok(())
}

#[tauri::command]
pub async fn get_proxy_status(proxy: State<'_, ProxyState>) -> Result<ProxyStatus, String> {
    let server = proxy.0.lock().await;
    Ok(server.get_status().await)
}

#[tauri::command]
pub fn update_proxy_config(db: State<DbState>, input: ProxyConfigInput) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    if let Some(addr) = input.listen_address {
        conn.execute("UPDATE proxy_config SET listen_address = ?1 WHERE id = 1", rusqlite::params![addr])
            .map_err(|e| e.to_string())?;
    }
    if let Some(port) = input.listen_port {
        conn.execute("UPDATE proxy_config SET listen_port = ?1 WHERE id = 1", rusqlite::params![port as i32])
            .map_err(|e| e.to_string())?;
    }
    if let Some(failover) = input.auto_failover {
        conn.execute("UPDATE proxy_config SET auto_failover = ?1 WHERE id = 1", rusqlite::params![failover as i32])
            .map_err(|e| e.to_string())?;
    }
    if let Some(retries) = input.max_retries {
        conn.execute("UPDATE proxy_config SET max_retries = ?1 WHERE id = 1", rusqlite::params![retries as i32])
            .map_err(|e| e.to_string())?;
    }
    if let Some(t) = input.streaming_first_byte_timeout_s {
        conn.execute("UPDATE proxy_config SET streaming_first_byte_timeout_s = ?1 WHERE id = 1", rusqlite::params![t as i32])
            .map_err(|e| e.to_string())?;
    }
    if let Some(t) = input.non_streaming_timeout_s {
        conn.execute("UPDATE proxy_config SET non_streaming_timeout_s = ?1 WHERE id = 1", rusqlite::params![t as i32])
            .map_err(|e| e.to_string())?;
    }
    if let Some(enabled) = input.enabled {
        conn.execute("UPDATE proxy_config SET enabled = ?1 WHERE id = 1", rusqlite::params![enabled as i32])
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// 读取完整代理配置（含自动启动开关），供前端初始化
#[tauri::command]
pub fn get_proxy_config(db: State<DbState>) -> Result<ProxyConfigInfo, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT enabled, listen_address, listen_port, auto_failover, max_retries,
                streaming_first_byte_timeout_s, non_streaming_timeout_s
         FROM proxy_config WHERE id = 1",
        [],
        |row| {
            Ok(ProxyConfigInfo {
                enabled: row.get::<_, i32>(0)? != 0,
                listen_address: row.get(1)?,
                listen_port: row.get::<_, i32>(2)? as u16,
                auto_failover: row.get::<_, i32>(3)? != 0,
                max_retries: row.get::<_, i32>(4)? as u32,
                streaming_first_byte_timeout_s: row.get::<_, i32>(5)? as u64,
                non_streaming_timeout_s: row.get::<_, i32>(6)? as u64,
            })
        },
    )
    .map_err(|e| e.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 多开互斥：首个实例拿到锁，第二个拿不到；释放后可重新获取
    #[test]
    fn test_proxy_lock_exclusive() {
        let dir = std::env::temp_dir().join(format!("dss-proxy-lock-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let f1 = acquire_proxy_lock(&dir);
        assert!(f1.is_some(), "首个实例应持有代理锁");
        let f2 = acquire_proxy_lock(&dir);
        assert!(f2.is_none(), "第二实例不应拿到锁（共享代理）");
        drop(f1);
        let f3 = acquire_proxy_lock(&dir);
        assert!(f3.is_some(), "锁释放后应可重新获取");

        std::fs::remove_dir_all(&dir).ok();
    }
}
