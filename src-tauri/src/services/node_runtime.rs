//! 内置 Node 运行时管理。
//!
//! 目标：系统未安装 Node.js 时，MCP 的 npx 命令仍可工作（出厂捆绑 Node 便携版）。
//! - 捆绑版：打进安装包（`bundle.resources` 映射到资源目录 `node/`），完全离线可用
//! - 升级版：用户可在线升级到最新 LTS，下载解压到应用数据目录 `node_runtime/`，优先于捆绑版
//! - 生效顺序：系统 PATH > 升级版 > 捆绑版（系统已装 Node 时以系统为准）
//! - 版本读取与命令执行统一走 `crate::utils::process`（npx/npm 直调 `node.exe <cli.js>`，绕开 .cmd）

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::Manager;

/// 出厂捆绑版在应用资源目录下的相对路径（对应 tauri.conf.json 的 bundle.resources 映射）
const BUNDLED_REL: &str = "node";
/// 升级版存放目录名（应用数据目录下）
const UPGRADED_DIR: &str = "node_runtime";
/// Node 便携版下载源（国内镜像，直连即可）
const BINARY_BASE: &str = "https://registry.npmmirror.com/-/binary/node";

/// Node 运行时状态（健康页卡片展示）
#[derive(Debug, Serialize, Clone)]
pub struct NodeRuntimeInfo {
    /// 生效的 node --version（如 v22.14.0）；空表示不可用
    pub node_version: String,
    /// 生效的 npx --version（如 10.9.2）
    pub npx_version: String,
    /// 来源：system / upgraded / bundled / none
    pub source: String,
    /// 生效目录（system/upgraded/bundled 时）
    pub dir: Option<String>,
    /// 升级版目录（应用数据目录/node_runtime）
    pub upgraded_dir: Option<String>,
    /// 捆绑版目录（应用资源目录/node）
    pub bundled_dir: Option<String>,
    /// node 版本读取失败原因（node_version 为空时展示，帮助定位）
    pub node_error: Option<String>,
    /// npx 版本读取失败原因（npx_version 为空时展示，帮助定位）
    pub npx_error: Option<String>,
}

/// setup 时调用：确定生效的 Node 目录并注册到进程解析兜底
pub fn init_node_runtime(app: &tauri::AppHandle) {
    crate::utils::process::set_bundled_node_dir(effective_dir(app));
}

/// 生效目录：升级版存在优先，其次捆绑版
fn effective_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    let upgraded = app.path().app_data_dir().ok().map(|d| d.join(UPGRADED_DIR));
    let bundled = app.path().resource_dir().ok().map(|d| d.join(BUNDLED_REL));
    match (&upgraded, &bundled) {
        (Some(u), _) if crate::utils::process::node_exe_in(u).is_file() => Some(u.clone()),
        (_, Some(b)) if crate::utils::process::node_exe_in(b).is_file() => Some(b.clone()),
        _ => None,
    }
}

/// 查询 Node 运行时状态（版本取实际生效的那份；来源与进程解析优先级一致）
pub fn get_node_runtime_info(app: &tauri::AppHandle) -> NodeRuntimeInfo {
    let upgraded = app.path().app_data_dir().ok().map(|d| d.join(UPGRADED_DIR));
    let bundled = app.path().resource_dir().ok().map(|d| d.join(BUNDLED_REL));
    let has_upgraded = upgraded.as_ref().is_some_and(|d| crate::utils::process::node_exe_in(d).is_file());
    let has_bundled = bundled.as_ref().is_some_and(|d| crate::utils::process::node_exe_in(d).is_file());

    // 生效来源与 utils::process 解析一致：内置（升级版优先）→ 系统 PATH → none
    // 展示字段统一 normalize_path：去掉 canonicalize 产生的 `\\?\` verbatim 前缀
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
    } else if system_node_found() {
        (
            "system",
            system_node_dir()
                .map(|d| crate::utils::path::normalize_path(&d.to_string_lossy())),
        )
    } else {
        ("none", None)
    };

    let (node_version, node_error) = run_capture("node", "--version");
    let (npx_version, npx_error) = run_capture("npx", "--version");

    NodeRuntimeInfo {
        node_version,
        npx_version,
        source: source.to_string(),
        dir,
        upgraded_dir: upgraded
            .filter(|d| d.is_dir())
            .map(|d| crate::utils::path::normalize_path(&d.to_string_lossy())),
        bundled_dir: bundled
            .filter(|d| d.is_dir())
            .map(|d| crate::utils::path::normalize_path(&d.to_string_lossy())),
        node_error,
        npx_error,
    }
}

/// 升级到指定版本（缺省取最新 LTS）：下载 zip → 解压到 node_runtime → 立即生效。
/// 下载全程通过 `node-runtime-progress` 事件推送进度（流式写盘，不占内存、不阻塞主进程）。
/// use_proxy: None=自动（优先系统代理，无则直连）；Some(true)=强制系统代理；Some(false)=直连。
pub async fn upgrade_node_runtime(
    app: &tauri::AppHandle,
    version: Option<String>,
    use_proxy: Option<bool>,
) -> Result<NodeRuntimeInfo, String> {
    use crate::services::runtime_progress::{self, RuntimeProgress};
    const EVENT: &str = "node-runtime-progress";

    // 1. 网络检查 + 确定目标版本
    runtime_progress::emit(
        app,
        EVENT,
        &RuntimeProgress::phase("check", "检查网络与最新 Node LTS 版本…"),
    );
    let version = match version.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
        Some(v) => v,
        None => fetch_latest_lts(use_proxy).await?,
    };
    let version = version.trim_start_matches('v').to_string();

    // 平台对应的压缩包后缀与扩展名（npmmirror 官方命名：darwin-arm64/darwin-x64/linux-x64/win-x64）
    let (suffix, is_targz): (&str, bool) = if cfg!(windows) {
        ("win-x64", false)
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            ("darwin-arm64", true)
        } else {
            ("darwin-x64", true)
        }
    } else if cfg!(target_arch = "aarch64") {
        ("linux-arm64", true)
    } else {
        ("linux-x64", true)
    };
    let ext = if is_targz { "tar.gz" } else { "zip" };
    let url = format!("{BINARY_BASE}/v{version}/node-v{version}-{suffix}.{ext}");

    // 代理策略：None=自动；Some(true)=强制走系统代理；Some(false)=直连
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
    let client = builder.build().map_err(|e| format!("创建下载客户端失败: {e}"))?;

    // 2. 流式下载到临时压缩包（逐块写盘 + 进度事件推送）
    let data_dir = app.path().app_data_dir().map_err(|e| format!("无法解析应用数据目录: {e}"))?;
    let zip_path = data_dir.join(format!("node-v{version}-{suffix}.{ext}"));
    let v = version.clone();
    runtime_progress::download_to_file(
        app,
        EVENT,
        &client,
        &url,
        &zip_path,
        move |pct| {
            if pct.is_empty() {
                format!("下载 Node v{v}…")
            } else {
                format!("下载 Node v{v} {pct}…")
            }
        },
        300,
    )
    .await?;

    // 3. 解压到临时目录，成功后原子替换 node_runtime
    let target = data_dir.join(UPGRADED_DIR);
    let zip_for_task = zip_path.clone();
    let app_for_task = app.clone();
    tokio::task::spawn_blocking(move || {
        runtime_progress::emit(
            &app_for_task,
            EVENT,
            &RuntimeProgress::phase("extract", "解压安装中…"),
        );
        let r = if is_targz {
            crate::services::jdk_runtime::extract_targz(&zip_for_task, &target)
        } else {
            extract_zip(&zip_for_task, &target)
        };
        runtime_progress::emit(
            &app_for_task,
            EVENT,
            &RuntimeProgress::phase("done", "安装完成"),
        );
        r
    })
    .await
    .map_err(|e| format!("解压任务失败: {e}"))??;

    let _ = std::fs::remove_file(&zip_path);
    init_node_runtime(app);
    // get_node_runtime_info 内部会同步执行 node/npx --version，放入 blocking 线程池
    let app_for_task = app.clone();
    let info = tokio::task::spawn_blocking(move || {
        crate::services::node_runtime::get_node_runtime_info(&app_for_task)
    })
    .await
    .map_err(|e| format!("查询 Node 运行时状态失败: {e}"))?;
    Ok(info)
}

/// 恢复出厂：删除升级版，回到捆绑版（无捆绑版时回到 none）
pub fn reset_node_runtime(app: &tauri::AppHandle) -> Result<NodeRuntimeInfo, String> {
    if let Some(upgraded) = app.path().app_data_dir().ok().map(|d| d.join(UPGRADED_DIR)) {
        if upgraded.is_dir() {
            std::fs::remove_dir_all(&upgraded)
                .map_err(|e| format!("删除升级版目录失败（可能仍有进程占用 node.exe）: {e}"))?;
        }
    }
    init_node_runtime(app);
    Ok(get_node_runtime_info(app))
}

/// 系统 PATH 中是否存在 Node（node.exe / node.cmd / node.bat）
fn system_node_found() -> bool {
    system_node_dir().is_some()
}

/// 系统 PATH 中 Node 可执行文件所在目录（找到的第一个）
fn system_node_dir() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["node.exe", "node.cmd", "node.bat"]
    } else {
        &["node"]
    };
    let path_var = std::env::var_os("PATH")?;
    if let Some(d) = std::env::split_paths(&path_var).find(|dir| names.iter().any(|n| dir.join(n).is_file())) {
        return Some(d);
    }
    // macOS/Linux GUI 启动 PATH 极简（不继承 shell 配置）：兑底探测常见安装目录（nvm/brew/usr/local）
    crate::utils::process::probe_common_program("node").and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

/// 执行程序并捕获 stdout，返回（版本串, 失败原因）。
/// 失败原因用于 UI 展示（版本为空时说明具体原因），走 process 解析与 MCP 启动逻辑一致。
fn run_capture(program: &str, flag: &str) -> (String, Option<String>) {
    match crate::utils::process::output_blocking(program, &[flag.to_string()]) {
        Ok(o) if o.status.success() => match String::from_utf8(o.stdout) {
            Ok(s) if !s.trim().is_empty() => (s.trim().to_string(), None),
            Ok(_) => (String::new(), Some(format!("{program} --version 输出为空"))),
            Err(e) => (String::new(), Some(format!("读取 {program} 输出失败: {e}"))),
        },
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            let err = if err.is_empty() {
                format!("退出码 {}", o.status.code().unwrap_or(-1))
            } else {
                err
            };
            (String::new(), Some(format!("{program} 执行失败: {err}")))
        }
        Err(e) => (String::new(), Some(format!("{program} 不可用: {e}"))),
    }
}

/// 查询 npmmirror index.json 原始列表（按版本降序：v26 在前、v0.1 在后）
async fn fetch_index(use_proxy: Option<bool>) -> Result<Vec<serde_json::Value>, String> {
    // 代理策略：None=自动；Some(true)=强制系统代理；Some(false)=直连
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30));
    match use_proxy {
        Some(true) => {
            let proxy = crate::utils::net::read_system_proxy()
                .ok_or("未检测到系统代理")?;
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
    let client = builder.build().map_err(|e| format!("创建查询客户端失败: {e}"))?;
    let resp = client
        .get(format!("{BINARY_BASE}/index.json"))
        .send()
        .await
        .map_err(|e| format!("网络不可达：无法查询 Node 最新版本（请检查网络或系统代理）\n详情：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("查询最新版本失败: HTTP {}", resp.status()));
    }
    resp.json().await.map_err(|e| format!("解析版本列表失败: {e}"))
}

/// 查询 npmmirror index.json，返回最新 LTS 版本号（如 22.14.0）。
/// 注意：列表按版本降序排列（v26 在前、v0.1 在后），不能依赖顺序——
/// 统一遍历取 lts 标识非空且版本号最大的条目。
pub(crate) async fn fetch_latest_lts(use_proxy: Option<bool>) -> Result<String, String> {
    let list = fetch_index(use_proxy).await?;
    pick_latest_lts(&list).ok_or_else(|| "未找到可用的 LTS 版本".to_string())
}

/// 查询最近的 N 个 LTS 版本（按版本降序），供环境页“选择版本”下拉候选
pub(crate) async fn fetch_lts_list(
    use_proxy: Option<bool>,
    limit: usize,
) -> Result<Vec<String>, String> {
    let list = fetch_index(use_proxy).await?;
    let mut lts: Vec<(String, (u32, u32, u32))> = list
        .iter()
        .filter_map(|item| {
            let lts = item.get("lts").and_then(|l| l.as_str()).unwrap_or("");
            if lts.is_empty() {
                return None;
            }
            let v = item.get("version").and_then(|v| v.as_str())?.trim_start_matches('v');
            let ver = parse_semver(v)?;
            Some((v.to_string(), ver))
        })
        .collect();
    lts.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(lts.into_iter().take(limit).map(|(v, _)| v).collect())
}

/// 从 index.json 条目中挑选最新 LTS：lts 标识非空（字符串）且版本号最大
fn pick_latest_lts(list: &[serde_json::Value]) -> Option<String> {
    let mut best: Option<(String, (u32, u32, u32))> = None;
    for item in list {
        let lts = item.get("lts").and_then(|l| l.as_str()).unwrap_or("");
        if lts.is_empty() {
            continue;
        }
        let Some(v) = item.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        let v = v.trim_start_matches('v');
        let Some(ver) = parse_semver(v) else { continue };
        if best.as_ref().is_none_or(|(_, b)| ver > *b) {
            best = Some((v.to_string(), ver));
        }
    }
    best.map(|(v, _)| v)
}

/// 解析 x.y.z 版本号为数值元组（位数不足补 0，如 "4" → (4,0,0)）
fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut out = [0u32; 3];
    for (i, p) in parts.iter().enumerate() {
        out[i] = p.parse().ok()?;
    }
    Some((out[0], out[1], out[2]))
}

/// 解压 zip 到 target：zip 根目录内容直接落到 target 下（通用：Node 便携版 / 手动升级的工具包）
pub(crate) fn extract_zip(zip_path: &Path, target: &Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| format!("打开压缩包失败: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("读取压缩包失败: {e}"))?;

    let parent = target.parent().unwrap_or(Path::new("."));
    let tmp = parent.join(format!("{}.tmp", UPGRADED_DIR));
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
        std::fs::remove_dir_all(target).map_err(|e| format!("替换旧版本失败（请先停止使用 Node 的 MCP 服务器）: {e}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// npmmirror index.json 按版本降序排列，取 LTS 最大版本不应受顺序影响
    #[test]
    fn test_pick_latest_lts_desc_order() {
        // 模拟真实返回：v26 在最前、v0.1 在最后（降序）
        let list = vec![
            json!({ "version": "v26.7.0", "lts": false }),
            json!({ "version": "v22.14.0", "lts": "Jod" }),
            json!({ "version": "v20.19.0", "lts": "Iron" }),
            json!({ "version": "v4.2.0", "lts": "Argon" }),
            json!({ "version": "v0.1.14", "lts": false }),
        ];
        assert_eq!(pick_latest_lts(&list).as_deref(), Some("22.14.0"));
    }

    #[test]
    fn test_pick_latest_lts_asc_order() {
        // 升序排列也不受影响
        let list = vec![
            json!({ "version": "v0.1.14", "lts": false }),
            json!({ "version": "v4.2.0", "lts": "Argon" }),
            json!({ "version": "v20.19.0", "lts": "Iron" }),
            json!({ "version": "v22.14.0", "lts": "Jod" }),
            json!({ "version": "v26.7.0", "lts": false }),
        ];
        assert_eq!(pick_latest_lts(&list).as_deref(), Some("22.14.0"));
    }

    #[test]
    fn test_pick_latest_lts_none() {
        assert_eq!(pick_latest_lts(&[]), None);
        assert_eq!(
            pick_latest_lts(&[json!({ "version": "v26.7.0", "lts": false })]),
            None
        );
    }

    #[test]
    fn test_parse_semver() {
        assert_eq!(parse_semver("22.14.0"), Some((22, 14, 0)));
        assert_eq!(parse_semver("4"), Some((4, 0, 0)));
        assert_eq!(parse_semver("4.2"), Some((4, 2, 0)));
        assert_eq!(parse_semver("abc"), None);
        assert_eq!(parse_semver("1.2.3.4"), None);
    }
}
