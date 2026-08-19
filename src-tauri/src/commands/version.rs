use serde::Serialize;

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

/// 用户级 npm 全局目录（基座 CLI 的安装前缀，需 PATH 可见）：
/// - Windows: `%APPDATA%\npm`（npm 用户级全局目录，装 Node 时默认在 PATH）
/// - macOS/Linux: `~/.npm-global`
/// 内置 npm 的默认前缀在应用资源目录下（不在 PATH），直接 `-g` 安装会导致
/// 安装成功后 `deveco` 命令仍无法解析，因此安装/读取时显式使用该目录。
fn user_npm_global_dir() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .map(|d| d.join("npm"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(std::path::PathBuf::from)
            .map(|h| h.join(".npm-global"))
    }
}

/// 构造 npm 子进程并按 use_proxy 注入系统代理：
/// - `None` / `Some(true)`: 检测到系统代理则走（未检测到则直连，不报错）
/// - `Some(false)`: 直连（移除从宿主环境继承的代理变量，避免意外走代理）
fn npm_cmd(args: &[String], use_proxy: Option<bool>) -> Result<tokio::process::Command, String> {
    let mut cmd = crate::utils::process::command("npm", args)?;
    if use_proxy == Some(false) {
        for var in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            cmd.env_remove(var);
        }
        return Ok(cmd);
    }
    // 显式传 --proxy/--https-proxy 给 npm（比环境变量更可靠，覆盖 .npmrc 之外的场景）
    if let Some(proxy) = crate::utils::net::read_system_proxy() {
        cmd.args([format!("--proxy={proxy}"), format!("--https-proxy={proxy}")]);
    }
    Ok(cmd)
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
    // 1) PATH 中的 deveco 优先
    if let Ok(mut cmd) = crate::utils::process::command("deveco", &["--version".to_string()]) {
        if let Ok(output) = cmd.output().await {
            if output.status.success() {
                let v = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !v.is_empty() {
                    return Ok(v);
                }
            }
        }
    }
    // 2) 回退：用户级 npm 全局目录中的 deveco（安装目标，未进 PATH 时仍可识别）
    if let Some(dir) = user_npm_global_dir() {
        let shim = if cfg!(windows) {
            dir.join("deveco.cmd")
        } else {
            dir.join("deveco")
        };
        if shim.is_file() {
            let shim_str = shim.to_string_lossy().to_string();
            let out = tokio::task::spawn_blocking(move || {
                crate::utils::process::output_blocking(&shim_str, &["--version".to_string()])
            })
            .await
            .map_err(|e| format!("执行 deveco --version 失败: {e}"))??;
            if out.status.success() {
                let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !v.is_empty() {
                    return Ok(v);
                }
            }
        }
    }
    Err("DevEco Code not installed or not in PATH".to_string())
}

#[tauri::command]
pub async fn list_available_versions(use_proxy: Option<bool>) -> Result<Vec<VersionInfo>, String> {
    let args = vec![
        "view".to_string(),
        "@deveco-test/deveco-code".to_string(),
        "versions".to_string(),
        "--json".to_string(),
        "--registry=https://registry.npmjs.org".to_string(),
    ];
    let output = npm_cmd(&args, use_proxy)?
        .output()
        .await
        .map_err(|e| format!("Failed to query npm: {e}"))?;

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
pub async fn install_version(version: String, use_proxy: Option<bool>) -> Result<String, String> {
    let pkg = format!("@deveco-test/deveco-code@{version}");
    let mut args = vec![
        "install".to_string(),
        "-g".to_string(),
        pkg,
        "--registry=https://registry.npmjs.org".to_string(),
    ];
    // 安装到用户级 npm 全局目录（PATH 可见），使安装后 deveco 命令全局可用；
    // 内置 npm 默认前缀在应用资源目录下（不在 PATH），直接 -g 会导致装了找不到。
    if let Some(dir) = user_npm_global_dir() {
        args.push(format!("--prefix={}", dir.to_string_lossy()));
    }
    let output = npm_cmd(&args, use_proxy)?
        .output()
        .await
        .map_err(|e| format!("Failed to install: {e}"))?;

    if output.status.success() {
        Ok(format!("Installed @deveco-test/deveco-code@{version}"))
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

/// 检查基座（@deveco-test/deveco-code CLI）是否有新版本。
/// 前端可在应用启动时调用：若 can_update 为 true 则提示用户升级（由用户确认后再调 install_version）。
/// 不做静默全局安装，避免在用户不知情时改动全局环境。
#[tauri::command]
pub async fn check_base_update(use_proxy: Option<bool>) -> Result<BaseUpdateInfo, String> {
    let package = "@deveco-test/deveco-code".to_string();
    let current = get_current_version().await.unwrap_or_default();

    let args = vec![
        "view".to_string(),
        package.clone(),
        "version".to_string(),
        "--registry=https://registry.npmjs.org".to_string(),
    ];
    let output = npm_cmd(&args, use_proxy)?
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

    /// 用户级 npm 全局目录必须指向 PATH 可见的目录（Windows 下为 %APPDATA%\npm）
    #[test]
    fn user_global_dir_is_named() {
        let dir = user_npm_global_dir().expect("应能解析用户级 npm 全局目录");
        let name = dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        assert_eq!(name, if cfg!(windows) { "npm" } else { ".npm-global" });
    }
}
