use tauri::State;

use crate::db::DbState;
use crate::services::licensing::{self, LicenseState, Tier, TierInfo};

/// 读取当前授权状态：档位 + 配额用量 + 超额项。
/// 前端 `/license` 页与设置页的配额角标都读它。
#[tauri::command]
pub fn get_license_state(db: State<DbState>) -> Result<LicenseState, String> {
    let conn = db.0.lock().map_err(|error| error.to_string())?;
    Ok(licensing::state(&conn))
}

/// 全部档位的权益与配额，供权益对比表使用。**与当前档位无关**——
/// 用户要先看清「Pro 能拿到什么」才谈得上切换。
#[tauri::command]
pub fn list_tiers() -> Result<Vec<TierInfo>, String> {
    Ok(Tier::ALL.iter().copied().map(licensing::tier_info).collect())
}

/// 本地切换档位。
///
/// **这是模型不是收费**：项目红线是「支付功能暂缓」，所以这里没有任何支付校验，
/// 档位就存在本地 `settings` 表里，用户自己就能切。
/// 生产环境把 `licensing::current_tier` 换成读服务端签发的授权即可，
/// 本命令、前端与权益表都不用改——这是刻意的分层：商业化外壳先搭好，
/// 收款那一步留在最里面。
#[tauri::command]
pub fn set_license_tier(db: State<DbState>, tier: String) -> Result<LicenseState, String> {
    let parsed = Tier::parse(&tier).ok_or_else(|| format!("未知档位：{tier}"))?;
    let conn = db.0.lock().map_err(|error| error.to_string())?;
    licensing::set_tier(&conn, parsed)?;
    Ok(licensing::state(&conn))
}
