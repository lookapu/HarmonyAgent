//! 内置 Git 运行时管理。
//!
//! 目标：系统未安装 Git 时，分支工作流 / 文档下载 / git 面板仍可工作（出厂捆绑便携版）。
//! - 捆绑版：打进安装包（`bundle.resources` 映射到资源目录 `git/`），完全离线可用
//! - 升级版：用户可在线升级到最新版，下载 PortableGit 自解压包解压到应用数据目录
//!   `git_runtime/`，优先于捆绑版
//! - 生效顺序（进程解析）：升级版 > 捆绑版 > 系统 PATH（保证脱离系统环境的一致性，
//!   与内置 Node 行为一致）
//! - 命令执行统一走 `crate::utils::process`（内置 `cmd\git.exe` 直调，不注入 PATH）

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::Manager;

/// 出厂捆绑版在应用资源目录下的相对路径（对应 tauri.conf.json 的 bundle.resources 映射）
const BUNDLED_REL: &str = "git";
/// 升级版存放目录名（应用数据目录下）
const UPGRADED_DIR: &str = "git_runtime";
/// PortableGit 发布仓库（查询最新版与下载均走 GitHub，代理策略与 Node 一致）
const RELEASE_API: &str = "https://api.github.com/repos/git-for-windows/git/releases/latest";
const DOWNLOAD_BASE: &str = "https://github.com/git-for-windows/git/releases/download";

/// Git 运行时状态（环境页卡片展示）
#[derive(Debug, Serialize, Clone)]
pub struct GitRuntimeInfo {
    /// 生效的 git --version（如 git version 2.50.1.windows.1）；空表示不可用
    pub git_version: String,
    /// 来源：upgraded / bundled / system / none
    pub source: String,
    /// 生效目录（upgraded/bundled 时）
    pub dir: Option<String>,
    /// 升级版目录（应用数据目录/git_runtime）
    pub upgraded_dir: Option<String>,
    /// 捆绑版目录（应用资源目录/git）
    pub bundled_dir: Option<String>,
    /// 版本读取失败原因（git_version 为空时展示）
    pub git_error: Option<String>,
}

/// setup 时调用：确定生效的 Git 目录并注册到进程解析兜底
pub fn init_git_runtime(app: &tauri::AppHandle) {
    crate::utils::process::set_bundled_git_dir(effective_dir(app));
}

/// 生效目录：升级版存在优先，其次捆绑版
fn effective_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    let upgraded = app.path().app_data_dir().ok().map(|d| d.join(UPGRADED_DIR));
    let bundled = app.path().resource_dir().ok().map(|d| d.join(BUNDLED_REL));
    match (&upgraded, &bundled) {
        (Some(u), _) if u.join("cmd").join("git.exe").is_file() => Some(u.clone()),
        (_, Some(b)) if b.join("cmd").join("git.exe").is_file() => Some(b.clone()),
        _ => None,
    }
}

/// 查询 Git 运行时状态（版本取实际生效的那份；来源与进程解析优先级一致）
pub fn get_git_runtime_info(app: &tauri::AppHandle) -> GitRuntimeInfo {
    let upgraded = app.path().app_data_dir().ok().map(|d| d.join(UPGRADED_DIR));
    let bundled = app.path().resource_dir().ok().map(|d| d.join(BUNDLED_REL));
    let has_upgraded = upgraded.as_ref().is_some_and(|d| d.join("cmd").join("git.exe").is_file());
    let has_bundled = bundled.as_ref().is_some_and(|d| d.join("cmd").join("git.exe").is_file());

    // 生效来源与 utils::process 解析一致：内置（升级版优先）→ 系统 PATH → none
    let (source, dir) = if has_upgraded {
        (
            "upgraded",
            upgraded
                .clone()
                .map(|d| crate::utils::path::normalize_path(&d.to_string_lossy())),
        )
    } else if has_bundled {
        (
            "bundled",
            bundled
                .clone()
                .map(|d| crate::utils::path::normalize_path(&d.to_string_lossy())),
        )
    } else if system_git_found() {
        ("system", None)
    } else {
        ("none", None)
    };

    let (git_version, git_error) = run_git_version();

    GitRuntimeInfo {
        git_version,
        source: source.to_string(),
        dir,
        upgraded_dir: upgraded
            .map(|d| crate::utils::path::normalize_path(&d.to_string_lossy())),
        bundled_dir: bundled
            .map(|d| crate::utils::path::normalize_path(&d.to_string_lossy())),
        git_error,
    }
}

/// 升级到最新版：GitHub API 查最新 tag → 下载 PortableGit 自解压包 →
/// 静默解压（7z SFX -o -y）到 git_runtime → 立即生效。
/// 下载全程通过 `git-runtime-progress` 事件推送进度（流式写盘，不占内存、不阻塞主进程）。
/// use_proxy: None=自动（优先系统代理，无则直连）；Some(true)=强制系统代理；Some(false)=直连。
pub async fn upgrade_git_runtime(
    app: &tauri::AppHandle,
    use_proxy: Option<bool>,
) -> Result<GitRuntimeInfo, String> {
    use crate::services::runtime_progress::{self, RuntimeProgress};
    const EVENT: &str = "git-runtime-progress";

    // 1. 网络检查 + 查询最新 tag
    runtime_progress::emit(
        app,
        EVENT,
        &RuntimeProgress::phase("check", "检查网络与最新 Git for Windows 版本…"),
    );
    let tag = fetch_latest_tag(use_proxy).await?;
    let ver = tag.trim_start_matches('v').to_string();
    let url = format!("{DOWNLOAD_BASE}/{tag}/PortableGit-{ver}-64-bit.7z.exe");

    // 2. 流式下载到临时文件（逐块写盘 + 进度事件推送）
    let client = build_client(use_proxy)?;
    let data_dir = app.path().app_data_dir().map_err(|e| format!("无法解析应用数据目录: {e}"))?;
    let sfx_path = data_dir.join(format!("PortableGit-{ver}-64-bit.7z.exe"));
    let v = ver.clone();
    runtime_progress::download_to_file(
        app,
        EVENT,
        &client,
        &url,
        &sfx_path,
        move |pct| {
            if pct.is_empty() {
                format!("下载 PortableGit {v}…")
            } else {
                format!("下载 PortableGit {v} {pct}…")
            }
        },
        300,
    )
    .await?;

    // 3. 7z SFX 静默解压：-o 指定输出目录（与 -o 之间不能有空格），-y 覆盖确认
    let tmp = data_dir.join(format!("{}.tmp", UPGRADED_DIR));
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp).map_err(|e| format!("清理临时目录失败: {e}"))?;
    }
    std::fs::create_dir_all(&tmp).map_err(|e| format!("创建临时目录失败: {e}"))?;
    let sfx_for_task = sfx_path.clone();
    let tmp_for_task = tmp.clone();
    let app_for_task = app.clone();
    tokio::task::spawn_blocking(move || {
        runtime_progress::emit(
            &app_for_task,
            EVENT,
            &RuntimeProgress::phase("extract", "解压安装中…"),
        );
        let out_flag = format!("-o{}", tmp_for_task.display());
        let output = std::process::Command::new(&sfx_for_task)
            .args([out_flag.as_str(), "-y"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|e| format!("自解压执行失败: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "自解压失败（退出码 {}）: {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        runtime_progress::emit(
            &app_for_task,
            EVENT,
            &RuntimeProgress::phase("done", "安装完成"),
        );
        Ok(())
    })
    .await
    .map_err(|e| format!("解压任务失败: {e}"))??;

    // 解压后校验产物（PortableGit 目录结构含 cmd/git.exe），通过则原子替换
    if !tmp.join("cmd").join("git.exe").is_file() {
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_file(&sfx_path);
        return Err("解压产物校验失败：未找到 cmd\\git.exe".into());
    }
    let target = data_dir.join(UPGRADED_DIR);
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|e| format!("替换旧版本失败（可能有进程占用 git.exe）: {e}"))?;
    }
    std::fs::rename(&tmp, &target).map_err(|e| format!("启用新版本失败: {e}"))?;
    let _ = std::fs::remove_file(&sfx_path);

    init_git_runtime(app);
    // get_git_runtime_info 内部会同步执行 git --version，放入 blocking 线程池
    let app_for_task = app.clone();
    let info = tokio::task::spawn_blocking(move || {
        crate::services::git_runtime::get_git_runtime_info(&app_for_task)
    })
    .await
    .map_err(|e| format!("查询 Git 运行时状态失败: {e}"))?;
    Ok(info)
}

/// 恢复出厂：删除升级版，回到捆绑版（无捆绑版时回到系统/无）
pub fn reset_git_runtime(app: &tauri::AppHandle) -> Result<GitRuntimeInfo, String> {
    if let Some(upgraded) = app.path().app_data_dir().ok().map(|d| d.join(UPGRADED_DIR)) {
        if upgraded.is_dir() {
            std::fs::remove_dir_all(&upgraded)
                .map_err(|e| format!("删除升级版目录失败（可能仍有进程占用 git.exe）: {e}"))?;
        }
    }
    init_git_runtime(app);
    Ok(get_git_runtime_info(app))
}

/// 查询 Git for Windows 最新发布 tag（如 v2.50.1.windows.1）。
/// 走 GitHub API；失败返回友好错误（环境页提示"查询失败"）。
pub async fn fetch_latest_tag(use_proxy: Option<bool>) -> Result<String, String> {
    let client = build_client(use_proxy)?;
    let resp = client
        .get(RELEASE_API)
        .header("User-Agent", "deveco-switch")
        .send()
        .await
        .map_err(|e| format!("网络不可达：无法查询 Git 最新版本（请检查网络或系统代理）\n详情：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("查询最新版本失败: HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析版本信息失败: {e}"))?;
    body.get("tag_name")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "未找到最新版本 tag".to_string())
}

/// 按代理策略构建下载客户端（对齐 node_runtime 的代理处理）
fn build_client(use_proxy: Option<bool>) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(600));
    match use_proxy {
        Some(true) => {
            let proxy = crate::utils::net::read_system_proxy()
                .ok_or("未检测到系统代理，无法走代理下载（可关闭“走系统代理”后直连）")?;
            builder = builder
                .proxy(reqwest::Proxy::all(proxy).map_err(|e| format!("代理配置无效: {e}"))?);
        }
        Some(false) => {
            builder = builder.no_proxy();
        }
        None => {
            if let Some(proxy) = crate::utils::net::read_system_proxy() {
                builder = builder
                    .proxy(reqwest::Proxy::all(proxy).map_err(|e| format!("代理配置无效: {e}"))?);
            }
        }
    }
    builder.build().map_err(|e| format!("创建下载客户端失败: {e}"))
}

/// 系统 PATH 中是否存在 Git（git.exe）
fn system_git_found() -> bool {
    let path_var = match std::env::var_os("PATH") {
        Some(p) => p,
        None => return false,
    };
    std::env::split_paths(&path_var).any(|dir| dir.join("git.exe").is_file())
}

/// 执行 git --version 并捕获输出（走 process 解析，与命令执行逻辑一致）
fn run_git_version() -> (String, Option<String>) {
    match crate::utils::process::output_blocking("git", &["--version".to_string()]) {
        Ok(o) if o.status.success() => match String::from_utf8(o.stdout) {
            Ok(s) if !s.trim().is_empty() => (s.trim().to_string(), None),
            Ok(_) => (String::new(), Some("git --version 输出为空".to_string())),
            Err(e) => (String::new(), Some(format!("读取 git 输出失败: {e}"))),
        },
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            let err = if err.is_empty() {
                format!("退出码 {}", o.status.code().unwrap_or(-1))
            } else {
                err
            };
            (String::new(), Some(format!("git 执行失败: {err}")))
        }
        Err(e) => (String::new(), Some(format!("git 不可用: {e}"))),
    }
}

/// 便携版 git 可执行文件路径（cmd\git.exe）
pub fn git_exe_in(dir: &Path) -> PathBuf {
    dir.join("cmd").join("git.exe")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 开发机 runtime/git 存在时，effective 解析应命中捆绑版
    #[test]
    fn test_git_exe_in_layout() {
        assert_eq!(
            git_exe_in(Path::new("D:/x/git")),
            Path::new("D:/x/git").join("cmd").join("git.exe")
        );
    }

    /// tag 转 PortableGit 版本号：v2.50.1.windows.1 → 2.50.1.windows.1
    #[test]
    fn test_tag_to_ver() {
        let tag = "v2.50.1.windows.1";
        let ver = tag.trim_start_matches('v');
        assert_eq!(ver, "2.50.1.windows.1");
        assert_eq!(
            format!("{DOWNLOAD_BASE}/{tag}/PortableGit-{ver}-64-bit.7z.exe"),
            "https://github.com/git-for-windows/git/releases/download/v2.50.1.windows.1/PortableGit-2.50.1.windows.1-64-bit.7z.exe"
        );
    }
}
