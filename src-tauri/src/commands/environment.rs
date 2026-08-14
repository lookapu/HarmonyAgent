//! 环境统一管理：应用基座信息、内置工具（Node 运行时 / 鸿蒙工具链）的
//! 版本查询与手动升级。内置工具的版本信息、路径与升级入口统一走这里，
//! 前端"环境"页集中展示与操作。

use serde::Serialize;
use tauri::Manager;

/// 应用基座信息（安装位置 / 数据目录 / 当前版本）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub install_dir: Option<String>,
    pub data_dir: Option<String>,
    pub bundled_node_dir: Option<String>,
    pub upgraded_node_dir: Option<String>,
}

/// 环境信息汇总（环境页一次性加载）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentInfo {
    pub app: AppInfo,
    /// 内置 Node 运行时状态（版本/来源/目录）
    pub node: crate::services::node_runtime::NodeRuntimeInfo,
    /// 最新 Node LTS 版本（查询失败为 null）
    pub node_latest_lts: Option<String>,
    /// 内置 Git 运行时状态（版本/来源/目录）
    pub git: crate::services::git_runtime::GitRuntimeInfo,
    /// 最新 Git for Windows 版本 tag（查询失败为 null）
    pub git_latest: Option<String>,
    /// 鸿蒙工具链检查结果（hvigorw / hdc / ohpm / 工程结构）
    pub toolchain: Vec<crate::commands::health::ToolchainCheck>,
}

/// 应用基座信息（安装位置 / 数据目录 / 当前版本），路径统一去掉 `\\?\` 前缀便于展示
#[tauri::command]
pub fn get_app_info(app: tauri::AppHandle) -> AppInfo {
    let norm = |d: std::path::PathBuf| crate::utils::path::normalize_path(&d.to_string_lossy());
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        install_dir: app.path().resource_dir().ok().map(norm),
        data_dir: app.path().app_data_dir().ok().map(norm),
        bundled_node_dir: app
            .path()
            .resource_dir()
            .ok()
            .map(|d| d.join("node"))
            .filter(|d| d.join("node.exe").is_file())
            .map(norm),
        upgraded_node_dir: app
            .path()
            .app_data_dir()
            .ok()
            .map(|d| d.join("node_runtime"))
            .filter(|d| d.join("node.exe").is_file())
            .map(norm),
    }
}

/// 环境总览（环境页加载用）：基座信息 + Node 运行时 + 最新 LTS + 工具链检查。
/// 失败项各自兜底，不影响其他项展示。
#[tauri::command]
pub async fn get_environment_info(
    app: tauri::AppHandle,
    db: tauri::State<'_, crate::db::DbState>,
    custom_paths: Option<Vec<String>>,
) -> Result<EnvironmentInfo, String> {
    let app_info = get_app_info(app.clone());
    let node = crate::services::node_runtime::get_node_runtime_info(&app);
    let node_latest_lts = crate::services::node_runtime::fetch_latest_lts(None).await.ok();
    let git = crate::services::git_runtime::get_git_runtime_info(&app);
    let git_latest = crate::services::git_runtime::fetch_latest_tag(None).await.ok();
    // 工具链检查复用健康页命令（含自定义目录与 toolkit 目录查找）
    let toolchain =
        crate::commands::health::check_harmony_toolchain(app, db, None, custom_paths).unwrap_or_default();
    Ok(EnvironmentInfo {
        app: app_info,
        node,
        node_latest_lts,
        git,
        git_latest,
        toolchain,
    })
}

/// 查询内置工具最新版（Node 官方有稳定地址：npmmirror LTS 清单）。
/// 其他工具（hvigorw / hdc / ohpm）无官方独立下载地址，走手动填 URL 升级。
#[tauri::command]
pub async fn fetch_node_latest_lts() -> Result<String, String> {
    crate::services::node_runtime::fetch_latest_lts(None).await
}

/// 查询内置 Git 运行时状态（版本、来源、目录），与 Node/JDK 运行时卡片对齐
#[tauri::command]
pub fn get_git_runtime(app: tauri::AppHandle) -> crate::services::git_runtime::GitRuntimeInfo {
    crate::services::git_runtime::get_git_runtime_info(&app)
}

/// 查询 Git for Windows 最新发布 tag（GitHub API，失败返回友好错误）
#[tauri::command]
pub async fn fetch_git_latest_version() -> Result<String, String> {
    crate::services::git_runtime::fetch_latest_tag(None).await
}

/// 升级内置 Git 运行时到最新版（下载 PortableGit 自解压包，静默解压生效）。
/// use_proxy: None=自动；Some(true)=强制走系统代理；Some(false)=直连。
#[tauri::command]
pub async fn upgrade_git_runtime(
    app: tauri::AppHandle,
    use_proxy: Option<bool>,
) -> Result<crate::services::git_runtime::GitRuntimeInfo, String> {
    crate::services::git_runtime::upgrade_git_runtime(&app, use_proxy).await
}

/// 恢复出厂 Git 运行时（删除升级版，回到捆绑版）
#[tauri::command]
pub fn reset_git_runtime(
    app: tauri::AppHandle,
) -> Result<crate::services::git_runtime::GitRuntimeInfo, String> {
    crate::services::git_runtime::reset_git_runtime(&app)
}

/// 手动升级工具包：下载 zip 解压到应用数据目录
/// toolkits/<name>/ 下，工具链检查时优先该目录（用户自选的自定义目录次之）。
/// use_proxy: None=自动（有系统代理则用）；Some(true)=强制走系统代理；Some(false)=直连。
/// 返回解压后的目录路径。
#[tauri::command]
pub async fn install_toolkit(
    app: tauri::AppHandle,
    name: String,
    url: String,
    use_proxy: Option<bool>,
) -> Result<String, String> {
    let name = name.trim().to_string();
    let url = url.trim().to_string();
    if name.is_empty() {
        return Err("工具名不能为空".into());
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("升级地址必须以 http:// 或 https:// 开头".into());
    }

    let client = match use_proxy {
        Some(true) => crate::utils::net::build_client(true)?,
        Some(false) => crate::utils::net::build_client(false)?,
        None => crate::utils::net::build_client_auto()?,
    };
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("下载失败（{url}）: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("下载失败: HTTP {}（地址可能无效）", resp.status()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取下载内容失败: {e}"))?;

    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析应用数据目录: {e}"))?;
    let zip_path = data_dir.join(format!("toolkit-{name}.zip"));
    std::fs::write(&zip_path, &bytes).map_err(|e| format!("写入临时文件失败: {e}"))?;

    let target = data_dir.join("toolkits").join(&name);
    let zip_for_task = zip_path.clone();
    let target_for_task = target.clone();
    tokio::task::spawn_blocking(move || {
        crate::services::node_runtime::extract_zip(&zip_for_task, &target_for_task)
    })
    .await
    .map_err(|e| format!("解压任务失败: {e}"))??;

    let _ = std::fs::remove_file(&zip_path);
    Ok(crate::utils::path::normalize_path(&target.to_string_lossy()))
}

/// 官方 command-line-tools 压缩包布局为 `<顶层目录>/command-line-tools/{bin,sdk,...}`，
/// extract_zip 拉平顶层目录后 target 下仍是 command-line-tools 子目录，这里再拉平一层。
fn flatten_command_line_tools(target: &std::path::Path) -> Result<(), String> {
    let nested = target.join("command-line-tools");
    if !nested.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&nested).map_err(|e| format!("读取解压结果失败: {e}"))? {
        let e = entry.map_err(|e| format!("读取解压结果失败: {e}"))?;
        let dst = target.join(e.file_name());
        if dst.exists() {
            if dst.is_dir() {
                std::fs::remove_dir_all(&dst).map_err(|e| format!("替换旧目录失败: {e}"))?;
            } else {
                std::fs::remove_file(&dst).map_err(|e| format!("替换旧文件失败: {e}"))?;
            }
        }
        std::fs::rename(e.path(), &dst).map_err(|e| format!("移动文件失败: {e}"))?;
    }
    std::fs::remove_dir(&nested).map_err(|e| format!("清理目录失败: {e}"))?;
    Ok(())
}

/// 从本地 zip 安装工具包（官方 Command Line Tools 压缩包或用户自备 zip）：
/// 解压到 toolkits/<name>/（官方包的嵌套 command-line-tools 目录自动拉平），
/// 安装后刷新鸿蒙环境探测，使工程分析的 hdc/ohpm 子进程 PATH 立即生效。
/// 返回解压后的目录路径。
#[tauri::command]
pub async fn install_toolkit_from_zip(
    app: tauri::AppHandle,
    name: String,
    zip_path: String,
) -> Result<String, String> {
    let name = name.trim().to_string();
    let zip_path = zip_path.trim().to_string();
    if name.is_empty() {
        return Err("工具名不能为空".into());
    }
    let zip_src = std::path::Path::new(&zip_path);
    if !zip_src.is_file() {
        return Err(format!("压缩包不存在：{zip_path}"));
    }
    let is_zip = zip_src
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("zip"));
    if !is_zip {
        return Err("仅支持 .zip 格式的压缩包".into());
    }

    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析应用数据目录: {e}"))?;
    let target = data_dir.join("toolkits").join(&name);

    // 解压（extract_zip 内部会拉平 zip 的顶层目录并整体替换 target），官方包再拉平一层
    let zip_for_task = zip_src.to_path_buf();
    let target_for_task = target.clone();
    tokio::task::spawn_blocking(move || {
        crate::services::node_runtime::extract_zip(&zip_for_task, &target_for_task)?;
        flatten_command_line_tools(&target_for_task)
    })
    .await
    .map_err(|e| format!("解压任务失败: {e}"))??;

    // 刷新鸿蒙环境探测：工具链安装后，工程分析的 hdc/ohpm 子进程 PATH 立即生效
    crate::services::harmony_env::invalidate_cache();
    if let Some(db) = app.try_state::<crate::db::DbState>() {
        let env = crate::services::harmony_env::detect(db.inner());
        crate::utils::process::set_harmony_path_dirs(crate::services::harmony_env::path_dirs(&env));
    }

    Ok(crate::utils::path::normalize_path(&target.to_string_lossy()))
}

/// hdc 只接受 `version`（无 -- 前缀），传 --version 会报 Invalid arguments，故特判；
/// 其余工具统一用 --version。
fn version_arg(tool: &str) -> &'static str {
    if tool.to_ascii_lowercase().starts_with("hdc") {
        "version"
    } else {
        "--version"
    }
}

/// 读取工具版本（默认执行 --version，15 秒超时防卡死）。
/// 返回首行非空输出；执行失败返回友好错误（部分工具如 hvigorw 首次运行较慢）。
#[tauri::command]
pub async fn get_tool_version(path: String) -> Result<String, String> {
    let tool = std::path::Path::new(&path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let mut cmd = crate::utils::process::command(&path, &[version_arg(tool).to_string()])?;
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = tokio::time::timeout(std::time::Duration::from_secs(15), cmd.output())
        .await
        .map_err(|_| "执行超时（15 秒）".to_string())?
        .map_err(|e| format!("执行失败: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "退出码 {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stdout
        .lines()
        .chain(stderr.lines())
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .to_string();
    if line.is_empty() {
        Err("无版本输出".into())
    } else {
        Ok(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// hdc 用 `version`，其余工具用 `--version`（hdc 传 --version 会 Invalid arguments）
    #[test]
    fn version_arg_special_cases_hdc() {
        assert_eq!(version_arg("hdc.exe"), "version");
        assert_eq!(version_arg("HDC.EXE"), "version");
        assert_eq!(version_arg("hdc"), "version");
        assert_eq!(version_arg("hvigorw.bat"), "--version");
        assert_eq!(version_arg("ohpm.bat"), "--version");
        assert_eq!(version_arg("node.exe"), "--version");
        assert_eq!(version_arg(""), "--version");
    }

    /// 官方包嵌套布局：target/command-line-tools/{bin,version.txt} → 拉平为 target/{bin,version.txt}
    #[test]
    fn flatten_official_nested_layout() {
        let tmp = std::env::temp_dir().join(format!(
            "deveco-flatten-test-{}",
            std::process::id()
        ));
        let nested = tmp.join("command-line-tools");
        std::fs::create_dir_all(nested.join("bin")).unwrap();
        std::fs::write(nested.join("version.txt"), "6.0.0").unwrap();
        std::fs::write(nested.join("bin").join("hdc.exe"), "fake").unwrap();

        flatten_command_line_tools(&tmp).unwrap();

        assert!(tmp.join("version.txt").is_file(), "顶层文件应拉平到 target");
        assert!(tmp.join("bin").join("hdc.exe").is_file(), "bin 应拉平到 target");
        assert!(!tmp.join("command-line-tools").exists(), "嵌套目录应被移除");
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 无嵌套目录时不动（用户自备的已拉平 zip）
    #[test]
    fn flatten_skips_flat_layout() {
        let tmp = std::env::temp_dir().join(format!(
            "deveco-flatten-flat-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(tmp.join("bin")).unwrap();
        std::fs::write(tmp.join("bin").join("ohpm.bat"), "fake").unwrap();

        flatten_command_line_tools(&tmp).unwrap();

        assert!(tmp.join("bin").join("ohpm.bat").is_file());
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 组合路径：官方嵌套 zip（<顶层>/command-line-tools/...）经 extract_zip + 拉平后
    /// 得到 target/bin/hdc.exe，即 install_toolkit_from_zip 的核心流程。
    #[test]
    fn extract_and_flatten_official_zip_layout() {
        use std::io::Write;
        let tmp = std::env::temp_dir().join(format!(
            "deveco-zip-combo-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let zip_path = tmp.join("official.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(file);
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zw.start_file("cmdline-tools-win-x64-6.0.0.100/command-line-tools/version.txt", opts)
                .unwrap();
            zw.write_all(b"6.0.0.100").unwrap();
            zw.start_file("cmdline-tools-win-x64-6.0.0.100/command-line-tools/bin/hdc.exe", opts)
                .unwrap();
            zw.write_all(b"fake-hdc").unwrap();
            zw.finish().unwrap();
        }

        let target = tmp.join("toolkits").join("command-line-tools");
        crate::services::node_runtime::extract_zip(&zip_path, &target).unwrap();
        flatten_command_line_tools(&target).unwrap();

        assert!(target.join("bin").join("hdc.exe").is_file(), "hdc 应位于 toolkits/command-line-tools/bin 下");
        assert!(target.join("version.txt").is_file());
        assert!(!target.join("command-line-tools").exists());
        std::fs::remove_dir_all(&tmp).ok();
    }
}
