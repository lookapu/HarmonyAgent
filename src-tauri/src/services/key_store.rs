//! API Key 安全存储：优先 Windows 凭据管理器（keyring），失败时回退数据库明文（绝不丢 Key）。
//!
//! 读写策略：
//! - 保存：系统凭据写入成功 → 数据库 `api_key` 列置空（Key 只存在于系统凭据库）；
//!   写入失败（如无桌面会话/凭据服务不可用）→ 明文落库兜底。
//! - 读取：数据库有明文 → 直接使用（兼容旧数据与兜底场景）；为空 → 从系统凭据读取。
//! - 删除：同时清理两处。

use keyring::Entry;
use rusqlite::Connection;

/// 凭据服务名（Windows 凭据管理器中的应用名）
const SERVICE: &str = "deveco-switch";

fn entry_for(provider_id: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, provider_id).map_err(|e| format!("无法打开系统凭据存储: {e}"))
}

/// 保存 Provider 密钥：优先系统凭据管理器，失败回退数据库明文
pub fn save_provider_key(conn: &Connection, provider_id: &str, key: &str) -> Result<(), String> {
    let stored_securely = entry_for(provider_id)
        .and_then(|e| e.set_password(key).map_err(|err| format!("写入系统凭据失败: {err}")))
        .is_ok();
    let db_value: Option<String> = if stored_securely { None } else { Some(key.to_string()) };
    conn.execute(
        "UPDATE providers SET api_key = ?2 WHERE id = ?1",
        rusqlite::params![provider_id, db_value],
    )
    .map_err(|e| format!("保存密钥失败: {e}"))?;
    Ok(())
}

/// 读取 Provider 密钥：数据库明文优先（兼容旧数据），其次系统凭据管理器
pub fn load_provider_key(conn: &Connection, provider_id: &str) -> Result<Option<String>, String> {
    let plain: Option<String> = conn
        .query_row("SELECT api_key FROM providers WHERE id = ?1", [provider_id], |r| r.get(0))
        .map_err(|e| format!("读取密钥失败: {e}"))?;
    if let Some(k) = plain.filter(|k| !k.is_empty()) {
        return Ok(Some(k));
    }
    match entry_for(provider_id).and_then(|e| {
        e.get_password()
            .map_err(|err| format!("读取系统凭据失败: {err}"))
    }) {
        Ok(k) => Ok((!k.is_empty()).then_some(k)),
        // 系统凭据无记录/不可用：按未配置处理，不阻塞请求流程
        Err(_) => Ok(None),
    }
}

/// 删除 Provider 密钥（同时清理系统凭据与数据库明文）
pub fn delete_provider_key(conn: &Connection, provider_id: &str) -> Result<(), String> {
    conn.execute("UPDATE providers SET api_key = NULL WHERE id = ?1", [provider_id])
        .map_err(|e| format!("清理密钥失败: {e}"))?;
    let _ = entry_for(provider_id).map(|e| e.delete_credential());
    Ok(())
}
