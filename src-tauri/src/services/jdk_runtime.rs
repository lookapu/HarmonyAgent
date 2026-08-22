//! 内置 JDK 运行时管理（多版本）。
//!
//! 目标：鸿蒙 hvigor 构建需要 JDK 17，用户机器无 JDK/DevEco JBR 时构建仍可用。
//! - 捆绑版：打进安装包（`bundle.resources` 映射到资源目录 `jdk/`），完全离线可用
//! - 升级版：按 feature 版本（17/21/25…）在线安装到应用数据目录 `jdk_runtime/jdk-<feature>/`，
//!   多版本并存，可在环境页切换默认版本
//! - 默认版本：`jdk_runtime/default.txt` 记录 feature 号；未设置时取最高 feature 升级版，
//!   再兑底捆绑版。默认 JDK 目录注册进 `utils::process`，在系统未设置 JAVA_HOME 时
//!   自动注入子进程环境（hvigor 构建 / java 命令兑底）
//! - 下载源：Adoptium API v3（下载文件在 GitHub Release，需走系统代理时按 use_proxy 控制）
//! - 生效顺序：系统 JAVA_HOME 已存在时以系统为准；否则注入内置默认 JDK

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::Manager;

use crate::services::runtime_progress::{self, RuntimeProgress};

/// 出厂捆绑版在应用资源目录下的相对路径（对应 tauri.conf.json 的 bundle.resources 映射）
const BUNDLED_REL: &str = "jdk";
/// 升级版根目录名（应用数据目录下），内部按 `jdk-<feature>` 子目录存多版本
const UPGRADED_ROOT: &str = "jdk_runtime";
/// 默认版本标记文件名（内容为 feature 号，如 "17"）
const DEFAULT_FILE: &str = "default.txt";
/// JDK 安装/更新进度事件名（前端 listen 同名）
const PROGRESS_EVENT: &str = "jdk-install-progress";
/// Adoptium API：列 LTS feature 版本
const AVAILABLE_API: &str = "https://api.adoptium.net/v3/info/available_releases";
/// Adoptium 资产 API：按当前平台动态拼 os/架构（mac 上 GUI 与 shell 环境一致，需 arm64/x64 区分）
fn assets_api(feature: &str) -> String {
    let (os, arch) = if cfg!(windows) {
        ("windows", "x64")
    } else if cfg!(target_os = "macos") {
        if std::env::consts::ARCH == "aarch64" {
            ("macos", "aarch64")
        } else {
            ("macos", "x64")
        }
    } else {
        ("linux", if std::env::consts::ARCH == "aarch64" { "aarch64" } else { "x64" })
    };
    format!("https://api.adoptium.net/v3/assets/latest/{feature}/hotspot?architecture={arch}&image_type=jdk&os={os}&vendor=eclipse")
}

/// 当前平台 JDK 压缩包扩展名（Adoptium：Windows zip，macOS/Linux tar.gz）
fn archive_suffix() -> &'static str {
    if cfg!(windows) { ".zip" } else { ".tar.gz" }
}
/// 更新检查缓存 TTL（10 分钟；安装/卸载后立即失效）
const UPDATE_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(600);
/// 更新检查缓存：feature → 最新版本号（静态内存缓存，避免每次进健康页都请求 Adoptium API）
static UPDATE_CACHE: std::sync::Mutex<Option<(std::time::SystemTime, std::collections::HashMap<String, String>)>> =
    std::sync::Mutex::new(None);

/// 单个 JDK 版本信息（卡片列表项）
#[derive(Debug, Serialize, Clone)]
pub struct JdkVersionInfo {
    /// feature 号（如 "17"、"21"）
    pub feature: String,
    /// 完整版本（如 "17.0.20+8"，取自 release 文件）
    pub full_version: String,
    /// 目录路径（规范化，去 verbatim 前缀）
    pub path: String,
    /// 来源：bundled / upgraded
    pub source: String,
    /// 是否为当前默认版本
    pub is_default: bool,
}

/// JDK 运行时状态（健康页卡片展示）
#[derive(Debug, Serialize, Clone)]
pub struct JdkRuntimeInfo {
    /// 已安装版本列表（feature 号降序）
    pub versions: Vec<JdkVersionInfo>,
    /// 生效的默认目录（无任何 JDK 时为 None）
    pub active_dir: Option<String>,
    /// 生效 JDK 版本（release 文件 JAVA_VERSION，如 17.0.20）
    pub active_version: Option<String>,
    /// 系统环境变量 JAVA_HOME（存在时优先于内置，子进程注入跳过）
    pub system_java_home: Option<String>,
}

/// 已装 JDK 的更新检查结果
#[derive(Debug, Serialize, Clone)]
pub struct JdkUpdateInfo {
    pub feature: String,
    /// 已装完整版本（如 17.0.20+8；捆绑版无 release 时为空）
    pub installed: String,
    /// Adoptium 最新版本（如 17.0.20+8）
    pub latest: String,
    /// 是否有可用更新
    pub updatable: bool,
}

/// setup 时调用：确定默认 JDK 目录并注册到进程解析兜底（JAVA_HOME 注入 + java 命令兑底）
pub fn init_jdk_runtime(app: &tauri::AppHandle) {
    crate::utils::process::set_default_jdk_dir(default_jdk_dir(app));
}

/// JDK 可执行文件路径（跨平台：Windows 为 java.exe，macOS/Linux 为 java）
fn java_bin(dir: &Path) -> PathBuf {
    if cfg!(windows) {
        dir.join("bin").join("java.exe")
    } else {
        dir.join("bin").join("java")
    }
}

/// 目录下是否存在 JDK 可执行文件（作为「已安装 JDK」的判定）
fn has_java_bin(dir: &Path) -> bool {
    java_bin(dir).is_file()
}

/// 默认 JDK 目录：default.txt 指定的 feature（升级版优先，其次捆绑版同 feature ）
/// → 最高 feature 升级版 → 捆绑版 → None
pub fn default_jdk_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    // default.txt 指定 feature：升级版优先，捆绑版 feature 匹配时兑底
    if let Some(feat) = default_feature(app) {
        let upgraded = upgraded_root(app).map(|r| r.join(format!("jdk-{feat}")));
        if upgraded.as_ref().is_some_and(|d| has_java_bin(d)) {
            return upgraded;
        }
        if let Some(b) = bundled_dir(app) {
            if feature_of(&b).as_deref() == Some(feat.as_str()) {
                return Some(b);
            }
        }
    }
    // 未指定：最高 feature 升级版
    if let Some(dir) = upgraded_dirs(app).into_iter().max_by_key(|d| feature_num(d)) {
        return Some(dir);
    }
    // 兑底捆绑版
    let b = bundled_dir(app)?;
    has_java_bin(&b).then_some(b)
}

/// 探测系统 JAVA_HOME（跨平台）：优先环境变量；macOS GUI 启动不继承 shell 环境，
/// 追加 /usr/libexec/java_home 与 sdkman 常见路径兜底
fn system_java_home() -> Option<String> {
    if let Some(v) = std::env::var_os("JAVA_HOME") {
        let s = v.to_string_lossy().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    #[cfg(target_os = "macos")]
    {
        // /usr/libexec/java_home：macOS 官方 JDK 定位工具（安装 Java.framework 时生效）
        if let Ok(out) = std::process::Command::new("/usr/libexec/java_home").output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() && Path::new(&s).join("bin").join("java").is_file() {
                    return Some(s);
                }
            }
        }
        // sdkman：GUI 启动时 JAVA_HOME 不可见，但目录真实存在（current 为符号链接，is_file 跟随）
        if let Some(home) = std::env::var_os("HOME") {
            let cur = PathBuf::from(home)
                .join(".sdkman")
                .join("candidates")
                .join("java")
                .join("current");
            if cur.join("bin").join("java").is_file() {
                return Some(cur.to_string_lossy().to_string());
            }
        }
    }
    None
}

/// 查询 JDK 运行时状态（版本列表、默认版本、系统 JAVA_HOME）
pub fn get_jdk_runtime_info(app: &tauri::AppHandle) -> JdkRuntimeInfo {
    let active = default_jdk_dir(app);
    let sys = system_java_home();
    let mut versions: Vec<JdkVersionInfo> = Vec::new();

    // 捆绑版
    if let Some(b) = bundled_dir(app) {
        if has_java_bin(&b) {
            let feat = feature_of(&b).unwrap_or_default();
            let full = read_release_full(&b).unwrap_or_default();
            versions.push(JdkVersionInfo {
                is_default: active.as_ref() == Some(&b),
                feature: feat,
                full_version: full,
                path: crate::utils::path::normalize_path(&b.to_string_lossy()),
                source: "bundled".into(),
            });
        }
    }
    // 升级版（多版本）
    for d in upgraded_dirs(app) {
        let feat = feature_of(&d).unwrap_or_default();
        if feat.is_empty() {
            continue;
        }
        let full = read_release_full(&d).unwrap_or_default();
        versions.push(JdkVersionInfo {
            is_default: active.as_ref() == Some(&d),
            feature: feat,
            full_version: full,
            path: crate::utils::path::normalize_path(&d.to_string_lossy()),
            source: "upgraded".into(),
        });
    }
    // feature 号降序展示（21 在前，17 在后）
    versions.sort_by_key(|a| std::cmp::Reverse(a.feature.clone()));
    // 同一 feature 升级版与捆绑版并存时，升级版在前
    versions.sort_by_key(|v| if v.source == "upgraded" { 0 } else { 1 });

    // 无内置 JDK 时，系统 JDK（JAVA_HOME / macOS sdkman 探测）作为可用项兑底，
    // 避免 mac 上明明装了 JDK 却显示「未检测到任何 JDK」
    if versions.is_empty() {
        if let Some(sys_path) = &sys {
            let p = PathBuf::from(sys_path);
            let full = read_release_full(&p).unwrap_or_default();
            let feature = read_release_java_version(&p)
                .and_then(|v| v.split('.').next().map(|s| s.to_string()))
                .unwrap_or_default();
            versions.push(JdkVersionInfo {
                is_default: true,
                feature,
                full_version: full,
                path: crate::utils::path::normalize_path(sys_path),
                source: "system".into(),
            });
        }
    }
    // 生效目录：内置默认优先，其次系统 JDK
    let active_dir = active.clone().or_else(|| sys.clone().map(PathBuf::from));

    JdkRuntimeInfo {
        active_dir: active_dir
            .as_ref()
            .map(|d| crate::utils::path::normalize_path(&d.to_string_lossy())),
        active_version: active_dir.as_ref().and_then(|d| read_release_java_version(d)),
        versions,
        system_java_home: sys,
    }
}

/// 查询可安装的 feature 版本（Adoptium LTS 列表：8/11/17/21/25）。
/// use_proxy: None=自动（有系统代理则用）；Some(true)=强制系统代理；Some(false)=直连。
pub async fn fetch_available_releases(
    use_proxy: Option<bool>,
) -> Result<Vec<String>, String> {
    let client = build_download_client(use_proxy, 30)?;
    let resp = client
        .get(AVAILABLE_API)
        .send()
        .await
        .map_err(|e| format!("查询可用版本失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("查询可用版本失败: HTTP {}", resp.status()));
    }
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析版本列表失败: {e}"))?;
    let list = json
        .get("available_lts_releases")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "版本列表缺少 available_lts_releases 字段".to_string())?;
    let feats: Vec<String> = list
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n.to_string()))
        .collect();
    if feats.is_empty() {
        return Err("未找到可用的 LTS 版本".into());
    }
    Ok(feats)
}

/// 安装指定 feature 版本的 JDK（安装/更新共用）：网络预检 → 查最新资产 → 流式下载
/// （SHA256 校验）→ 解压到 jdk_runtime/jdk-<feature>（替换同 feature 旧版）→
/// 未设默认时自动设为默认。全程通过 `jdk-install-progress` 事件推送进度，不阻塞主进程。
/// use_proxy: None=自动（优先系统代理，无则直连）；Some(true)=强制系统代理；Some(false)=直连。
pub async fn install_jdk(
    app: &tauri::AppHandle,
    feature: String,
    use_proxy: Option<bool>,
) -> Result<JdkRuntimeInfo, String> {
    let feature = feature.trim().to_string();
    if feature.is_empty() || !feature.chars().all(|c| c.is_ascii_digit()) {
        return Err("版本号格式不正确（应为 feature 号，如 17）".into());
    }

    // 1. 网络检查 + 查询最新资产，定位当前平台 JDK 压缩包下载链接与 SHA256
    runtime_progress::emit(
        app,
        PROGRESS_EVENT,
        &RuntimeProgress::phase("check", format!("检查网络与 JDK {feature} 最新版本…")),
    );
    let client = build_download_client(use_proxy, 15)?;
    let url = assets_api(&feature);
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            return Err(format!(
                "网络不可达：无法连接 Adoptium 服务（请检查网络或系统代理设置后重试）\n详情：{e}"
            ))
        }
    };
    if resp.status().as_u16() == 404 {
        return Err(format!("JDK {feature} 版本不存在（Adoptium 未发布该版本）"));
    }
    if !resp.status().is_success() {
        return Err(format!("查询 JDK {feature} 资产失败: HTTP {}（网络异常，请稍后重试）", resp.status()));
    }
    let assets: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析资产列表失败: {e}"))?;
    let (link, checksum, latest_ver) = pick_jdk_pkg(&assets, &feature)?;

    // 2. 流式下载到临时压缩包（逐块写盘 + 边下边算 SHA256，不占内存，进度事件推送）
    let dl_client = build_download_client(use_proxy, 3600)?;
    let data_dir = app.path().app_data_dir().map_err(|e| format!("无法解析应用数据目录: {e}"))?;
    let root = data_dir.join(UPGRADED_ROOT);
    let archive_path = root.join(format!("jdk-{feature}{}", archive_suffix()));
    std::fs::create_dir_all(&root).map_err(|e| format!("创建 JDK 目录失败: {e}"))?;
    let ver = latest_ver.clone();
    let feat = feature.clone();
    let (_file, actual_sha) = runtime_progress::download_to_file(
        app,
        PROGRESS_EVENT,
        &dl_client,
        &link,
        &archive_path,
        move |pct| {
            if pct.is_empty() {
                format!("下载 JDK {feat}（{ver}）…")
            } else {
                format!("下载 JDK {feat}（{ver}）{pct}…")
            }
        },
        300,
    )
    .await?;

    // 3. SHA256 校验（防损坏/劫持，与官方 checksum 比较）
    runtime_progress::emit(
        app,
        PROGRESS_EVENT,
        &RuntimeProgress::phase("verify", "校验 SHA256 完整性…"),
    );
    if let Some(expect) = checksum {
        if !expect.eq_ignore_ascii_case(&actual_sha) {
            let _ = std::fs::remove_file(&archive_path);
            return Err(format!(
                "SHA256 校验失败：下载内容与官方校验值不一致（可能被损坏或劫持）\n本地：{actual_sha}\n期望：{expect}"
            ));
        }
    }

    // 4. 解压到 jdk_runtime/jdk-<feature>（替换同 feature 旧版），随后瘦身删无用大文件
    let target = root.join(format!("jdk-{feature}"));
    let archive_for_task = archive_path.clone();
    let app_for_task = app.clone();
    let slim_target = target.clone();
    tokio::task::spawn_blocking(move || {
        runtime_progress::emit(
            &app_for_task,
            PROGRESS_EVENT,
            &RuntimeProgress::phase("extract", "解压安装中…"),
        );
        let r = if cfg!(windows) {
            extract_zip(&archive_for_task, &target)
        } else {
            extract_targz(&archive_for_task, &target)
        }
        .map(|_| {
            slim_jdk(&slim_target);
        });
        runtime_progress::emit(
            &app_for_task,
            PROGRESS_EVENT,
            &RuntimeProgress::phase("done", "安装完成"),
        );
        r
    })
    .await
    .map_err(|e| format!("解压任务失败: {e}"))??;
    let _ = std::fs::remove_file(&archive_path);

    // 5. 更新检查缓存失效 + 未设置默认版本时，新装版本自动成为默认（首个可用版本即可用）
    invalidate_update_cache();
    if default_feature(app).is_none() {
        set_default_feature(app, &feature)?;
    }
    init_jdk_runtime(app);
    Ok(get_jdk_runtime_info(app))
}

/// 设置默认 JDK 版本（feature 号）；写入 default.txt 并立即生效
pub fn set_default_jdk(
    app: &tauri::AppHandle,
    feature: String,
) -> Result<JdkRuntimeInfo, String> {
    let feature = feature.trim().to_string();
    let exists = bundled_dir(app).is_some_and(|b| feature_of(&b).as_deref() == Some(feature.as_str()))
        || upgraded_dirs(app).iter().any(|d| feature_of(d).as_deref() == Some(feature.as_str()));
    if !exists {
        return Err(format!("未找到 JDK {feature}（请先安装）"));
    }
    set_default_feature(app, &feature)?;
    init_jdk_runtime(app);
    Ok(get_jdk_runtime_info(app))
}

/// 卸载升级版 JDK（feature 号）；捆绑版不可卸载。
/// 卸载当前默认版本时清除默认标记，自动回落（最高升级版 → 捆绑版）。
pub fn uninstall_jdk(
    app: &tauri::AppHandle,
    feature: String,
) -> Result<JdkRuntimeInfo, String> {
    let feature = feature.trim().to_string();
    let Some(root) = upgraded_root(app) else {
        return Err("无法解析应用数据目录".into());
    };
    let target = root.join(format!("jdk-{feature}"));
    if !target.is_dir() {
        return Err(format!("未找到已安装的 JDK {feature}"));
    }
    std::fs::remove_dir_all(&target)
        .map_err(|e| format!("删除 JDK {feature} 失败（可能有进程占用 java.exe）: {e}"))?;
    if default_feature(app).as_deref() == Some(feature.as_str()) {
        let _ = std::fs::remove_file(root.join(DEFAULT_FILE));
    }
    invalidate_update_cache();
    init_jdk_runtime(app);
    Ok(get_jdk_runtime_info(app))
}

/// 捆绑版目录（应用资源目录/jdk）
fn bundled_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path().resource_dir().ok().map(|d| d.join(BUNDLED_REL))
}

/// 升级版根目录（应用数据目录/jdk_runtime）
fn upgraded_root(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join(UPGRADED_ROOT))
}

/// 已安装的升级版目录列表（jdk-<feature> 存在 java.exe 的）
fn upgraded_dirs(app: &tauri::AppHandle) -> Vec<PathBuf> {
    let Some(root) = upgraded_root(app) else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(&root) else { return Vec::new() };
    rd.filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.join("bin").join("java.exe").is_file())
        .collect()
}

/// 读取默认版本标记（feature 号）；文件不存在或内容异常返回 None
fn default_feature(app: &tauri::AppHandle) -> Option<String> {
    let root = upgraded_root(app)?;
    let content = std::fs::read_to_string(root.join(DEFAULT_FILE)).ok()?;
    let feat = content.trim().to_string();
    (!feat.is_empty()).then_some(feat)
}

/// 写入默认版本标记
fn set_default_feature(app: &tauri::AppHandle, feature: &str) -> Result<(), String> {
    let root = upgraded_root(app).ok_or_else(|| "无法解析应用数据目录".to_string())?;
    std::fs::create_dir_all(&root).map_err(|e| format!("创建 JDK 目录失败: {e}"))?;
    std::fs::write(root.join(DEFAULT_FILE), format!("{feature}\n"))
        .map_err(|e| format!("保存默认版本失败: {e}"))
}

/// 从 JDK 目录的 release 文件读取 feature 号（JAVA_VERSION 的 major 段，如 17.0.20 → 17）
fn feature_of(dir: &Path) -> Option<String> {
    read_release_java_version(dir)
        .and_then(|v| v.split('.').next().map(|s| s.to_string()))
}

/// 从 release 文件读取 JAVA_VERSION（如 17.0.20）
fn read_release_java_version(dir: &Path) -> Option<String> {
    parse_release(dir).0
}

/// 从 release 文件读取完整版本（JAVA_VERSION + 构建号，如 17.0.20+8）
fn read_release_full(dir: &Path) -> Option<String> {
    let (java_version, impl_version) = parse_release(dir);
    match (&java_version, &impl_version) {
        (Some(jv), Some(iv)) => {
            // IMPLEMENTOR_VERSION 形如 Temurin-17.0.20+8，取其 `+` 之后构建号拼接
            let build = iv.rsplit('+').next().unwrap_or("");
            Some(format!("{jv}+{build}"))
        }
        (Some(jv), None) => Some(jv.clone()),
        _ => None,
    }
}

/// 解析 JDK release 属性文件，返回 (JAVA_VERSION, IMPLEMENTOR_VERSION)
fn parse_release(dir: &Path) -> (Option<String>, Option<String>) {
    let content = match std::fs::read_to_string(dir.join("release")) {
        Ok(c) => c,
        Err(_) => return (None, None),
    };
    let mut java_version = None;
    let mut impl_version = None;
    for line in content.lines() {
        if let Some(v) = line.strip_prefix("JAVA_VERSION=") {
            java_version = Some(v.trim_matches('"').to_string());
        } else if let Some(v) = line.strip_prefix("IMPLEMENTOR_VERSION=") {
            impl_version = Some(v.trim_matches('"').to_string());
        }
    }
    (java_version, impl_version)
}

/// feature 目录排序用的数值键（jdk-17 → 17，解析失败为 0）
fn feature_num(dir: &Path) -> u32 {
    feature_of(dir)
        .and_then(|f| f.parse::<u32>().ok())
        .unwrap_or(0)
}

/// 从 Adoptium assets JSON 中挑选 windows x64 jdk zip 的下载链接、SHA256 与版本号
/// 选择当前平台 JDK 压缩包资产（Windows zip / macOS·Linux tar.gz）
fn pick_jdk_pkg(
    assets: &serde_json::Value,
    feature: &str,
) -> Result<(String, Option<String>, String), String> {
    let suffix = archive_suffix();
    let arr = assets
        .as_array()
        .ok_or_else(|| "资产响应格式异常".to_string())?;
    for item in arr {
        let pkg = item.get("binary").and_then(|b| b.get("package"));
        let Some(name) = pkg.and_then(|p| p.get("name")).and_then(|n| n.as_str()) else {
            continue;
        };
        if !name.ends_with(suffix) {
            continue;
        }
        let link = pkg
            .and_then(|p| p.get("link"))
            .and_then(|l| l.as_str())
            .ok_or_else(|| "资产缺少下载链接".to_string())?;
        let checksum = pkg
            .and_then(|p| p.get("checksum"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());
        // 版本号取资产的 version.semver（如 17.0.20+8），缺失时回退 feature 号
        let ver = item
            .get("version")
            .and_then(|v| v.get("semver"))
            .and_then(|s| s.as_str())
            .unwrap_or(feature)
            .to_string();
        return Ok((link.to_string(), checksum, ver));
    }
    Err(format!("未找到 JDK {feature} 的 {suffix} 下载项"))
}

/// 检查已装 JDK 是否有可用的补丁更新（Adoptium latest assets 与本地 release 版本比较）。
/// 网络不可达时返回 Err（前端静默降级，不影响其他功能）。
pub async fn check_jdk_updates(app: &tauri::AppHandle) -> Result<Vec<JdkUpdateInfo>, String> {
    // 已装 feature 集合（bundled + upgraded 去重）
    let mut features: Vec<String> = Vec::new();
    if let Some(b) = bundled_dir(app) {
        if let Some(f) = feature_of(&b) {
            features.push(f);
        }
    }
    for d in upgraded_dirs(app) {
        if let Some(f) = feature_of(&d) {
            features.push(f);
        }
    }
    features.sort();
    features.dedup();

    let mut out = Vec::new();
    for feat in &features {
        // 单项失败静默跳过（个别版本查询异常不影响其他版本）
        let latest = match fetch_latest_full_version_cached(feat).await {
            Ok(v) => v,
            Err(_) => continue,
        };
        let installed = installed_full_version(app, feat).unwrap_or_default();
        let updatable = version_newer(&latest, &installed);
        out.push(JdkUpdateInfo {
            feature: feat.clone(),
            installed,
            latest,
            updatable,
        });
    }
    if out.is_empty() && !features.is_empty() {
        return Err("查询 JDK 更新失败（网络不可达）".into());
    }
    Ok(out)
}

/// 查询指定 feature 的最新完整版本号（Adoptium assets 的 version.semver），
/// 带 10 分钟内存缓存（避免每次进健康页都请求 API）；安装/更新/卸载后缓存失效。
async fn fetch_latest_full_version_cached(feature: &str) -> Result<String, String> {
    // 命中缓存且未过期时直接返回（锁在块内释放，不跨 await）
    {
        let guard = UPDATE_CACHE.lock().map_err(|_| "缓存锁异常")?;
        if let Some((t, map)) = guard.as_ref() {
            if t.elapsed().unwrap_or(UPDATE_CACHE_TTL) < UPDATE_CACHE_TTL {
                if let Some(v) = map.get(feature) {
                    return Ok(v.clone());
                }
            }
        }
    }
    let v = fetch_latest_full_version(feature).await?;
    {
        let mut guard = UPDATE_CACHE.lock().map_err(|_| "缓存锁异常")?;
        let (t, map) = guard
            .get_or_insert_with(|| (std::time::SystemTime::now(), std::collections::HashMap::new()));
        *t = std::time::SystemTime::now();
        map.insert(feature.to_string(), v.clone());
    }
    Ok(v)
}

/// 使更新检查缓存失效（安装/更新/卸载后调用，确保立即反映最新状态）
fn invalidate_update_cache() {
    if let Ok(mut g) = UPDATE_CACHE.lock() {
        *g = None;
    }
}

/// 查询指定 feature 的最新完整版本号（Adoptium assets 的 version.semver）
async fn fetch_latest_full_version(feature: &str) -> Result<String, String> {
    let client = build_download_client(None, 15)?;
    let url = assets_api(feature);
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let assets: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析失败: {e}"))?;
    let arr = assets.as_array().ok_or("格式异常")?;
    for item in arr {
        if let Some(v) = item
            .get("version")
            .and_then(|v| v.get("semver"))
            .and_then(|s| s.as_str())
        {
            return Ok(v.to_string());
        }
    }
    Err("未找到版本信息".into())
}

/// 已装 feature 的完整版本（捆绑版 feature 匹配时取捆绑版，否则取升级版）
fn installed_full_version(app: &tauri::AppHandle, feature: &str) -> Option<String> {
    if let Some(b) = bundled_dir(app) {
        if feature_of(&b).as_deref() == Some(feature) {
            return read_release_full(&b);
        }
    }
    upgraded_root(app)
        .map(|r| r.join(format!("jdk-{feature}")))
        .and_then(|d| read_release_full(&d))
}

/// 解析 "17.0.20+8" 为 (major, minor, security, build)
fn parse_full_version(s: &str) -> Option<(u32, u32, u32, u32)> {
    let (v, build) = s.split_once('+')?;
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let security = parts.next().unwrap_or("0").parse().ok()?;
    let build = build.parse().ok()?;
    Some((major, minor, security, build))
}

/// 判断 latest 是否比 installed 新；解析失败时退化为字符串不等比较
fn version_newer(latest: &str, installed: &str) -> bool {
    match (parse_full_version(latest), parse_full_version(installed)) {
        (Some(l), Some(i)) => l > i,
        _ => !latest.is_empty() && latest != installed,
    }
}

/// 删除构建不需要的大文件（源码包/jmods/示例/手册/JNI 头），减小磁盘占用与安装包体积。
/// 删除失败静默忽略（不影响 JDK 使用）。
fn slim_jdk(dir: &Path) {
    for name in ["src.zip", "jmods", "demo", "man", "include"] {
        let p = dir.join(name);
        if p.is_dir() {
            let _ = std::fs::remove_dir_all(&p);
        } else if p.is_file() {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// 解压 zip 到 target：zip 顶层目录内容直接落到 target 下（与 node_runtime::extract_zip
/// 同构，但临时目录按调用方隔离，避免多版本并发安装时互相踩踏）。
fn extract_zip(zip_path: &Path, target: &Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| format!("打开压缩包失败: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("读取压缩包失败: {e}"))?;

    let parent = target.parent().unwrap_or(Path::new("."));
    let tmp = parent.join(format!(
        "{}.{}.tmp",
        target.file_name().and_then(|n| n.to_str()).unwrap_or("jdk"),
        std::process::id()
    ));
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp).map_err(|e| format!("清理临时目录失败: {e}"))?;
    }
    std::fs::create_dir_all(&tmp).map_err(|e| format!("创建临时目录失败: {e}"))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("读取压缩条目失败: {e}"))?;
        let name = entry.name().to_string();
        // 防路径穿越：拒绝绝对路径与 .. 段
        if name.starts_with('/') || name.split(['/', '\\']).any(|seg| seg == "..") {
            return Err(format!("压缩包包含非法路径: {name}"));
        }
        let out_path = tmp.join(&name);
        if entry.is_dir() || name.ends_with('/') {
            std::fs::create_dir_all(&out_path).map_err(|e| format!("创建目录失败: {e}"))?;
            continue;
        }
        if let Some(parent_dir) = out_path.parent() {
            std::fs::create_dir_all(parent_dir).map_err(|e| format!("创建目录失败: {e}"))?;
        }
        let mut out = std::fs::File::create(&out_path).map_err(|e| format!("创建文件失败: {e}"))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| format!("解压文件失败: {e}"))?;
    }

    // 找到 zip 根目录（唯一顶层目录），把内容移入 target
    let root = std::fs::read_dir(&tmp)
        .map_err(|e| format!("读取临时目录失败: {e}"))?
        .next()
        .and_then(|r| r.ok())
        .map(|r| r.path())
        .filter(|p| p.is_dir())
        .ok_or_else(|| "压缩包结构异常：未找到顶层目录".to_string())?;

    if target.exists() {
        std::fs::remove_dir_all(target)
            .map_err(|e| format!("替换旧版本失败（请先停止正在运行的 java 进程）: {e}"))?;
    }
    std::fs::create_dir_all(target).map_err(|e| format!("创建目标目录失败: {e}"))?;
    for entry in std::fs::read_dir(&root).map_err(|e| format!("读取解压结果失败: {e}"))? {
        let e = entry.map_err(|e| format!("读取解压结果失败: {e}"))?;
        let dst = target.join(e.file_name());
        std::fs::rename(e.path(), &dst).map_err(|e| format!("移动文件失败: {e}"))?;
    }
    std::fs::remove_dir_all(&tmp).map_err(|e| format!("清理临时目录失败: {e}"))?;
    Ok(())
}

/// macOS/Linux：系统 tar 解压 tar.gz（解到临时目录后与 extract_zip
/// 相同的落盘模式：顶层 jdk-xxx 内容提升到 target）。
/// 供 node_runtime 等共享使用（node 官方包同为 tar.gz + 顶层目录布局）。
pub(crate) fn extract_targz(archive: &Path, target: &Path) -> Result<(), String> {
    let parent = target.parent().unwrap_or(Path::new("."));
    let tmp = parent.join(format!(
        "{}.{}.tmp",
        target.file_name().and_then(|n| n.to_str()).unwrap_or("jdk"),
        std::process::id()
    ));
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp).map_err(|e| format!("清理临时目录失败: {e}"))?;
    }
    std::fs::create_dir_all(&tmp).map_err(|e| format!("创建临时目录失败: {e}"))?;

    let st = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .current_dir(&tmp)
        .status()
        .map_err(|e| format!("调用系统 tar 失败: {e}"))?;
    if !st.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("tar 解压失败（压缩包可能损坏或已被占用）".to_string());
    }

    let root = std::fs::read_dir(&tmp)
        .map_err(|e| format!("读取临时目录失败: {e}"))?
        .next()
        .and_then(|r| r.ok())
        .map(|r| r.path())
        .filter(|p| p.is_dir())
        .ok_or_else(|| "压缩包结构异常：未找到顶层目录".to_string())?;

    if target.exists() {
        std::fs::remove_dir_all(target)
            .map_err(|e| format!("替换旧版本失败（请先停止正在运行的 java 进程）: {e}"))?;
    }
    std::fs::create_dir_all(target).map_err(|e| format!("创建目标目录失败: {e}"))?;
    for entry in std::fs::read_dir(&root).map_err(|e| format!("读取解压结果失败: {e}"))? {
        let e = entry.map_err(|e| format!("读取解压结果失败: {e}"))?;
        let dst = target.join(e.file_name());
        std::fs::rename(e.path(), &dst).map_err(|e| format!("移动文件失败: {e}"))?;
    }
    std::fs::remove_dir_all(&tmp).map_err(|e| format!("清理临时目录失败: {e}"))?;
    Ok(())
}

/// 构建下载客户端（与 node_runtime 升级一致的代理三态策略；下载大文件用长超时）。
fn build_download_client(
    use_proxy: Option<bool>,
    timeout_secs: u64,
) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(timeout_secs));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// release 文件解析（Temurin 标准格式）与 feature/完整版本提取
    #[test]
    fn test_parse_release_temurin() {
        let dir = std::env::temp_dir().join(format!("jdk_release_test_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(
            dir.join("release"),
            "IMPLEMENTOR=\"Eclipse Adoptium\"\nIMPLEMENTOR_VERSION=\"Temurin-17.0.20+8\"\nJAVA_VERSION=\"17.0.20\"\n",
        )
        .unwrap();
        let (jv, iv) = parse_release(&dir);
        assert_eq!(jv.as_deref(), Some("17.0.20"));
        assert_eq!(iv.as_deref(), Some("Temurin-17.0.20+8"));
        assert_eq!(feature_of(&dir).as_deref(), Some("17"));
        assert_eq!(read_release_full(&dir).as_deref(), Some("17.0.20+8"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// release 文件缺失时各项均为 None，不 panic
    #[test]
    fn test_parse_release_missing() {
        let dir = std::env::temp_dir().join(format!("jdk_release_missing_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(parse_release(&dir), (None, None));
        assert_eq!(feature_of(&dir), None);
        assert_eq!(read_release_full(&dir), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Adoptium assets 中跳过非本平台后缀（msi 等），只取当前平台（Windows zip / macOS·Linux tar.gz）的包
    #[test]
    fn test_pick_jdk_pkg_skips_other_ext() {
        let suffix = archive_suffix();
        let bad_name = if cfg!(windows) {
            "OpenJDK17U-jdk_x64_windows_hotspot_17.0.20_8.msi"
        } else {
            "OpenJDK17U-jdk_x64_mac_hotspot_17.0.20_8.dmg"
        };
        let good_name = if cfg!(windows) {
            "OpenJDK17U-jdk_x64_windows_hotspot_17.0.20_8.zip"
        } else {
            "OpenJDK17U-jdk_x64_mac_hotspot_17.0.20_8.tar.gz"
        };
        let assets = serde_json::json!([
            { "binary": { "package": { "name": bad_name, "link": "https://x/bad", "checksum": "aaa" } }, "version": { "semver": "17.0.20+8" } },
            { "binary": { "package": { "name": good_name, "link": "https://x/good", "checksum": "bbb" } }, "version": { "semver": "17.0.20+8" } }
        ]);
        let (link, checksum, ver) = pick_jdk_pkg(&assets, "17").unwrap();
        assert_eq!(link, "https://x/good");
        assert_eq!(checksum.as_deref(), Some("bbb"));
        assert_eq!(ver, "17.0.20+8");
        assert!(good_name.ends_with(suffix));
    }

    /// 无任何本平台包时报错
    #[test]
    fn test_pick_jdk_pkg_none() {
        let assets = serde_json::json!([]);
        assert!(pick_jdk_pkg(&assets, "17").is_err());
    }

    /// 完整版本号解析：17.0.20+8 → (17, 0, 20, 8)；非法格式返回 None
    #[test]
    fn test_parse_full_version() {
        assert_eq!(parse_full_version("17.0.20+8"), Some((17, 0, 20, 8)));
        assert_eq!(parse_full_version("21.0.5+11"), Some((21, 0, 5, 11)));
        assert_eq!(parse_full_version("8.0.442+6"), Some((8, 0, 442, 6)));
        assert_eq!(parse_full_version("17.0.20"), None);
        assert_eq!(parse_full_version("abc"), None);
        assert_eq!(parse_full_version(""), None);
    }

    /// 版本比较：最新 > 已装 → 可更新；相同/更旧/解析失败 → 不可更新
    #[test]
    fn test_version_newer() {
        assert!(version_newer("17.0.21+9", "17.0.20+8"));
        assert!(!version_newer("17.0.20+8", "17.0.20+8"));
        assert!(!version_newer("17.0.19+7", "17.0.20+8"));
        assert!(!version_newer("", "17.0.20+8"));
        assert!(version_newer("17.0.20+8", "")); // 捆绑版无 release 信息时视为可更新
        assert!(!version_newer("", ""));
    }
}
