use serde::Serialize;

use crate::utils::process;

#[derive(Debug, Serialize)]
pub struct VersionInfo {
    pub version: String,
    pub tag: Option<String>,
    pub is_current: bool,
}

/// 基座（@deveco-test/deveco-code CLI）升级检查结果。
#[derive(Debug, Serialize)]
pub struct BaseUpdateInfo {
    /// 当前已安装版本（取不到时为空）
    pub current: String,
    /// npm registry 上的最新版本
    pub latest: String,
    /// 是否存在比当前更新的版本
    pub can_update: bool,
    /// 升级所需的 npm 包名（前端确认后调用 install_version）
    pub package: String,
}

/// 比较两个 semver 字符串（仅比较数字段，忽略 -beta 等后缀），
/// 返回 a < b（即 a 更旧）。无法解析的段按 0 处理。
fn is_older(a: &str, b: &str) -> bool {
    fn parse(v: &str) -> Vec<u64> {
        v.split(['-', '+'])
            .next()
            .unwrap_or(v)
            .split('.')
            .map(|s| s.chars().take_while(|c| c.is_ascii_digit()).collect::<String>())
            .map(|s| s.parse::<u64>().unwrap_or(0))
            .collect()
    }
    let pa = parse(a);
    let pb = parse(b);
    let len = pa.len().max(pb.len());
    for i in 0..len {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        if x != y {
            return x < y;
        }
    }
    false
}

#[tauri::command]
pub async fn get_current_version() -> Result<String, String> {
    let output = process::command("deveco", &["--version".to_string()])?
        .output()
        .await
        .map_err(|e| format!("Failed to run deveco: {}", e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err("DevEco Code not installed or not in PATH".to_string())
    }
}

#[tauri::command]
pub async fn list_available_versions() -> Result<Vec<VersionInfo>, String> {
    let output = process::command(
        "npm",
        &[
            "view".to_string(),
            "@deveco-test/deveco-code".to_string(),
            "versions".to_string(),
            "--json".to_string(),
            "--registry=https://registry.npmjs.org".to_string(),
        ],
    )?
    .output()
    .await
    .map_err(|e| format!("Failed to query npm: {}", e))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    let versions: Vec<String> = serde_json::from_slice(&output.stdout)
        .map_err(|e| e.to_string())?;

    let current = get_current_version().await.unwrap_or_default();

    let infos: Vec<VersionInfo> = versions
        .into_iter()
        .rev()
        .map(|v| {
            let is_current = v == current;
            VersionInfo { version: v, tag: None, is_current }
        })
        .collect();

    Ok(infos)
}

#[tauri::command]
pub async fn install_version(version: String) -> Result<String, String> {
    let pkg = format!("@deveco-test/deveco-code@{}", version);
    let output = process::command(
        "npm",
        &[
            "install".to_string(),
            "-g".to_string(),
            pkg,
            "--registry=https://registry.npmjs.org".to_string(),
        ],
    )?
    .output()
    .await
    .map_err(|e| format!("Failed to install: {}", e))?;

    if output.status.success() {
        Ok(format!("Installed @deveco-test/deveco-code@{}", version))
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

/// 检查基座（@deveco-test/deveco-code CLI）是否有新版本。
/// 前端可在应用启动时调用：若 can_update 为 true 则提示用户升级（由用户确认后再调 install_version）。
/// 不做静默全局安装，避免在用户不知情时改动全局环境。
#[tauri::command]
pub async fn check_base_update() -> Result<BaseUpdateInfo, String> {
    let package = "@deveco-test/deveco-code".to_string();
    let current = get_current_version().await.unwrap_or_default();

    let output = process::command(
        "npm",
        &[
            "view".to_string(),
            package.clone(),
            "version".to_string(),
            "--registry=https://registry.npmjs.org".to_string(),
        ],
    )?
    .output()
    .await
    .map_err(|e| format!("Failed to query npm: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let latest = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let can_update = !current.is_empty() && !latest.is_empty() && is_older(&current, &latest);

    Ok(BaseUpdateInfo {
        current,
        latest,
        can_update,
        package,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compare() {
        assert!(is_older("1.0.0", "1.0.1"));
        assert!(is_older("1.0.9", "1.1.0"));
        assert!(is_older("1.9.0", "2.0.0"));
        assert!(!is_older("1.2.0", "1.2.0"));
        assert!(!is_older("1.3.0", "1.2.9"));
        // 缺段按 0
        assert!(is_older("1.0", "1.0.1"));
        // 预发布后缀在数字段相同时视为相等（不做严格 semver 预发布排序）
        assert!(!is_older("2.0.0-beta", "2.0.0"));
        assert!(is_older("2.0.0-beta", "2.0.1"));
    }
}
