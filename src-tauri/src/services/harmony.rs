//! 鸿蒙工程理解层：解析工程关键信息（bundleName / 启动 Ability / 签名 / API 版本 /
//! entry 模块 / hap 产物目录），并提供构建日志错误正则解析。
//!
//! 设计为轻量、容错：任何文件解析失败都返回 None 而不阻塞整体流程。
//! 不引入 JSON5 依赖，采用"去注释 + serde_json"的方式解析 json5（注释与尾逗号容错）。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// 部署/构建所需的最小工程信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyProject {
    pub bundle_name: Option<String>,
    pub version_code: Option<i64>,
    pub version_name: Option<String>,
    pub app_label: Option<String>,
    /// 启动 Ability 名（entry 模块 module.json5 的 mainElement）
    pub main_element: Option<String>,
    pub entry_module: Option<String>,
    pub api_version: Option<i64>,
    /// compatibleSdkVersion 原文（如 "6.1.1(24)"；未识别时为 None）
    pub sdk_version: Option<String>,
    pub signing_configured: bool,
    /// entry 模块构建产物目录（推导，不一定存在）
    pub hap_output_dir: Option<PathBuf>,
}

/// 构建错误结构化结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildError {
    pub kind: String,
    /// 根因分类：type / dependency / signing / sdk / api_level / resource / ohpm / syntax / other
    pub category: String,
    pub file: Option<String>,
    pub line: Option<i64>,
    pub column: Option<i64>,
    pub message: String,
    pub suggestion: String,
}

/// 根据错误消息推断根因分类，用于在 Agent 修复提示中前置"先查什么"
fn classify_message(kind: &str, message: &str) -> String {
    let m = message.to_ascii_lowercase();
    // 依赖类：模块解析失败 / 找不到包
    if m.contains("cannot find module")
        || m.contains("failed to resolve dependency")
        || m.contains("module not found")
        || m.contains("unresolved import")
        || m.contains("cannot find name 'ohpm'")
    {
        return "dependency".to_string();
    }
    // API 级别：使用了高于工程 compatibleSdkVersion 的 API
    if m.contains("not supported") && (m.contains("api") || m.contains("version"))
        || m.contains("requires api")
        || m.contains("api version")
        || m.contains("sdk version")
    {
        return "api_level".to_string();
    }
    // 类型错误
    if m.contains("type")
        && (m.contains("not assignable")
            || m.contains("is not")
            || m.contains("does not exist")
            || m.contains("cannot")
            || m.contains("mismatch")
            || m.contains("incompatible"))
        || m.contains("ts2")
    {
        return "type".to_string();
    }
    // 语法错误
    if m.contains("syntax error") || m.contains("unexpected token") || m.contains("parse error") {
        return "syntax".to_string();
    }
    // 资源类：找不到资源 / R 引用
    if m.contains("$r(")
        || m.contains("resource")
            && (m.contains("not found") || m.contains("cannot find") || m.contains("missing"))
    {
        return "resource".to_string();
    }
    // 签名类
    if kind == "signing" || m.contains("signing") || m.contains("certificate") || m.contains("profile not match") {
        return "signing".to_string();
    }
    // SDK/ohpm 类
    if kind == "sdk" || m.contains("sdk not found") || m.contains("compatible sdk") {
        return "sdk".to_string();
    }
    if kind == "ohpm" || m.contains("ohpm") {
        return "ohpm".to_string();
    }
    "other".to_string()
}

/// 目录是否为鸿蒙"工程根"（AppScope/app.json5 存在，或根级 build-profile.json5 顶层含 "app" 键），
/// 区别于仅有 oh-package.json5 / 模块级 build-profile.json5 的纯模块目录（如 entry / features/* 子模块）。
/// 用于主工程兜底判定：纯模块目录没有 bundleName/SDK 信息，不应作为分析根。
///
/// 注意：模块级 build-profile.json5 只有 apiType/buildOption/targets 等字段、没有顶层 "app" 键，
/// 不能仅凭 build-profile.json5 存在判定工程根——否则 entry 模块目录会被误判为工程根，
/// 导致构建/部署/Agent 工作根全部错位（典型事故：测试文件写进 entry/entry/src/ 嵌套目录）。
pub fn is_project_root(dir: &Path) -> bool {
    // AppScope/app.json5 是工程根最可靠标志（模块目录不会有）
    if dir.join("AppScope").join("app.json5").is_file() {
        return true;
    }
    // 兼容无 AppScope 的旧布局：build-profile.json5 顶层必须含 "app" 键（products/signingConfigs/modules）
    let bp = dir.join("build-profile.json5");
    bp.is_file()
        && read_to_string_opt(&bp).is_some_and(|t| parse_json5(&t).is_ok_and(|v| v.get("app").is_some()))
}

/// 解析鸿蒙工程的关键信息（部署与构建闭环所需的最小集合）。
///
/// 该兼容视图从统一语义模型派生，避免部署侧与工程分析侧分别猜测模块布局。
pub fn parse_project(root: &Path) -> HarmonyProject {
    let model = crate::services::harmony_model::parse(root);
    project_summary(root, &model)
}

pub fn project_summary(
    root: &Path,
    model: &crate::services::harmony_model::HarmonySemanticModel,
) -> HarmonyProject {
    let preferred_product = model
        .products
        .iter()
        .find(|product| product.name == "default")
        .or_else(|| model.products.first());
    let sdk_product = preferred_product
        .filter(|product| {
            product.compatible_sdk_version.is_some() || product.compile_sdk_version.is_some()
        })
        .or_else(|| {
            model.products.iter().find(|product| {
                product.compatible_sdk_version.is_some() || product.compile_sdk_version.is_some()
            })
        });
    let entry = model
        .modules
        .iter()
        .find(|module| module.kind == "entry")
        .or_else(|| model.modules.iter().find(|module| module.name == "entry"))
        .or_else(|| model.modules.iter().find(|module| module.artifact_kind == "hap"));
    let sdk_version = sdk_product.and_then(|product| {
        product
            .compatible_sdk_version
            .clone()
            .or_else(|| product.compile_sdk_version.clone())
    });
    HarmonyProject {
        bundle_name: model.app.bundle_name.clone(),
        version_code: model.app.version_code,
        version_name: model.app.version_name.clone(),
        app_label: model.app.label.clone(),
        main_element: entry.and_then(|module| {
            module
                .main_element
                .clone()
                .or_else(|| module.abilities.first().map(|ability| ability.name.clone()))
        }),
        entry_module: entry.map(|module| module.rel_path.clone()),
        api_version: sdk_version.as_deref().and_then(parse_api_version),
        sdk_version,
        signing_configured: model.products.iter().any(|product| {
            product
                .signing_config
                .as_ref()
                .is_some_and(|name| model.signing_configs.contains(name))
        }),
        hap_output_dir: entry.map(|module| {
            root.join(&module.rel_path)
                .join("build/default/outputs/default")
        }),
    }
}

fn parse_api_version(s: &str) -> Option<i64> {
    if let Some(start) = s.find('(') {
        if let Some(end) = s.find(')') {
            return s[start + 1..end].trim().parse().ok();
        }
    }
    s.trim().parse().ok()
}

/// 优先在推导的产物目录查找 hap，找不到则递归全工程（跳过依赖目录）。
pub fn find_latest_hap(root: &Path, preferred_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = preferred_dir {
        if let Some(p) = latest_hap_in_dir(dir) {
            return Some(p);
        }
    }
    find_latest_hap_fallback(root)
}

fn latest_hap_in_dir(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(PathBuf, SystemTime)> = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "hap") {
            let mtime = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            // 与递归 fallback 保持同一排序：签名产物优先，同类产物再按修改时间。
            // 给 signed 加固定权重，避免刚生成的 unsigned 包覆盖仍可部署的 signed 包。
            let is_signed = p
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.contains("-signed"));
            let score = if is_signed {
                mtime + std::time::Duration::from_secs(365 * 24 * 3600)
            } else {
                mtime
            };
            if best.as_ref().map_or(true, |(_, t)| score > *t) {
                best = Some((p, score));
            }
        }
    }
    best.map(|(p, _)| p)
}

fn find_latest_hap_fallback(root: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, best: &mut Option<(PathBuf, SystemTime)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if name.starts_with('.')
                    || name == "node_modules"
                    || name == "oh_modules"
                    || name == ".ohpm"
                {
                    continue;
                }
                walk(&p, best);
            } else if p.extension().is_some_and(|x| x == "hap") {
                let mtime = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                // signed 优先：给 signed 加一个时间权重
                let is_signed = name.contains("-signed");
                let score = if is_signed {
                    mtime + std::time::Duration::from_secs(365 * 24 * 3600)
                } else {
                    mtime
                };
                if best.as_ref().map_or(true, |(_, t)| score > *t) {
                    *best = Some((p, score));
                }
            }
        }
    }
    let mut best = None;
    walk(root, &mut best);
    best.map(|(p, _)| p)
}

/// 构建 hvigorw 命令参数（单 entry 模块工程用 assembleHap，多模块按文档加 --mode module）。
pub fn assemble_args(module: Option<&str>, mode: &str) -> Vec<String> {
    let mut args = vec!["assembleHap".to_string(), "--no-daemon".to_string()];
    if let Some(m) = module {
        args.push("--mode".to_string());
        args.push("module".to_string());
        args.push("-p".to_string());
        args.push(format!("module={m}@default"));
    }
    args.push("-p".to_string());
    args.push("product=default".to_string());
    args.push("-p".to_string());
    args.push(format!("buildMode={mode}"));
    args
}

/// clean 任务参数：清理构建缓存（build/ 目录），用于缓存导致的诡异构建失败
pub fn clean_args() -> Vec<String> {
    vec!["clean".to_string(), "--no-daemon".to_string()]
}

/// 解析出的 hvigor 启动命令：可执行程序、参数前缀、需注入的环境变量。
pub struct HvigorCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// 解析 hvigor 启动命令。
/// Windows 下解析顺序：
/// 1. 工程内 `hvigor/hvigor-wrapper.js` → 内置/系统 node 直调（绕过 cmd/.bat 弹窗与解析开销）
/// 2. 工程内 `hvigorw.bat`（完整路径，由 process::command 经 cmd /C 执行）
/// 3. 软件内置工具链（app_data/toolkits/<name>/，官方 Command Line Tools 自带
///    hvigor 引擎，node 直调）——未安装 DevEco Studio 时也能构建
/// 4. DevEco Studio 内置 hvigor 工具链（tools/hvigor/bin/hvigorw.js，node 直调）——
///    工程因 .gitignore/拷贝丢失 hvigorw 脚本时仍可构建
/// 5. 均未找到 → Err（明确提示，避免让调用方拿到一个必然启动失败的程序名）
///
/// env 注入 DEVECO_SDK_HOME：hvigor 解析 HarmonyOS SDK 路径时只认该环境变量
/// （HarmonyOS 模式下 Property 不读 local.properties 的 hwsdk.dir），未设置且探测到
/// DevEco Studio 内置 SDK 时自动注入，否则构建会以 00303217/00303312 失败。
pub fn hvigor_command(project_path: &Path) -> Result<HvigorCommand, String> {
    #[cfg(windows)]
    let env = hvigor_env();
    #[cfg(not(windows))]
    let env: Vec<(String, String)> = Vec::new();
    let wrapper = project_path.join("hvigor").join("hvigor-wrapper.js");
    if cfg!(windows) && wrapper.is_file() {
        return Ok(HvigorCommand {
            program: "node".to_string(),
            args: vec![wrapper.to_string_lossy().to_string()],
            env,
        });
    }
    let bat = project_path.join("hvigorw.bat");
    if cfg!(windows) && bat.is_file() && hvigorw_bat_usable(&bat, &wrapper) {
        return Ok(HvigorCommand {
            program: bat.to_string_lossy().to_string(),
            args: Vec::new(),
            env,
        });
    }
    #[cfg(windows)]
    {
        // 软件内置工具链：官方 Command Line Tools 自带 hvigor 引擎
        if let Some(toolkit_hvigorw) = find_toolkit_hvigorw() {
            return Ok(HvigorCommand {
                program: "node".to_string(),
                args: vec![toolkit_hvigorw.to_string_lossy().to_string()],
                env,
            });
        }
        // 环境探测已发现 command-line-tools（盘符扫描/手动配置/软件内置）：复用其 hvigor 引擎，
        // 避免“探测报告可用、构建却不用”的纸面工具链
        if let Some(cli_hvigorw) = find_cli_hvigorw() {
            return Ok(HvigorCommand {
                program: "node".to_string(),
                args: vec![cli_hvigorw.to_string_lossy().to_string()],
                env,
            });
        }
        if let Some(dev_hvigorw) = find_deveco_toolchain().map(|(h, _)| h) {
            return Ok(HvigorCommand {
                program: "node".to_string(),
                args: vec![dev_hvigorw.to_string_lossy().to_string()],
                env,
            });
        }
    }
    let script = project_path.join("hvigorw");
    if !cfg!(windows) && script.is_file() {
        return Ok(HvigorCommand {
            program: script.to_string_lossy().to_string(),
            args: Vec::new(),
            env,
        });
    }
    Err("工程缺少 hvigor 启动脚本（hvigor/hvigor-wrapper.js 或 hvigorw.bat 均不存在），且未找到可用的 hvigor 引擎。\n请任选其一：\n1. 在软件 设置 → 环境 页安装官方 Command Line Tools（自带 hvigor/ohpm/hdc 工具链）；\n2. 确认 DevEco Studio 已安装（默认路径 C:\\Program Files\\Huawei\\DevEco Studio）；\n3. 在 DevEco Studio 中打开工程让其补全构建脚本。".into())
}

/// 复用环境探测缓存的 command-line-tools hvigor 引擎
/// （<cli_root>/hvigor/bin/hvigorw.js 或 <cli_root>/hvigor/hvigorw.js）。
#[cfg(windows)]
fn find_cli_hvigorw() -> Option<PathBuf> {
    let cli = crate::services::harmony_env::cached_cli_root()?;
    [
        cli.join("hvigor").join("bin").join("hvigorw.js"),
        cli.join("hvigor").join("hvigorw.js"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// 在软件内置工具链目录（app_data/toolkits/）下查找 hvigor 引擎。
/// 官方 Command Line Tools 包布局：<toolkits>/<name>/hvigor/bin/hvigorw.js；
/// 兼容 <name>/hvigor/hvigorw.js 与 <name>/bin/hvigorw.js 两种直装布局。
#[cfg(windows)]
fn find_toolkit_hvigorw() -> Option<PathBuf> {
    let bundled = crate::services::harmony_env::get_bundled_cli_dir()?;
    let mut bases = vec![bundled.clone()];
    if let Some(parent) = bundled.parent() {
        bases.push(parent.to_path_buf()); // toolkits 根：扫描全部已安装工具包
    }
    let mut seen = std::collections::HashSet::new();
    for base in bases {
        // 单个 base 不存在（如仅装了 hvigor-engine 未装 command-line-tools）时
        // 继续扫下一个，不能直接返回
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_dir() || !seen.insert(p.clone()) {
                continue;
            }
            for cand in [
                p.join("hvigor").join("bin").join("hvigorw.js"),
                p.join("hvigor").join("hvigorw.js"),
                p.join("bin").join("hvigorw.js"),
            ] {
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

/// hvigorw.bat 是 DevEco 模板脚本：wrapper 存在时它只负责转发给
/// hvigor/hvigor-wrapper.js；wrapper 缺失时回退调用全局 hvigorw 命令（通常不在
/// PATH 中，且 Windows 下 cmd 报错可能被吞掉、看不到任何输出）。因此 bat 存在
/// 但引用缺失的 wrapper 时应判定不可用，跳过它改走 DevEco 内置工具链。
fn hvigorw_bat_usable(bat: &Path, wrapper: &Path) -> bool {
    wrapper.is_file()
        || read_to_string_opt(bat)
            .map_or(true, |t| !t.contains("hvigor-wrapper.js"))
}

/// 构建 hvigor 所需环境变量：用户显式设置了 DEVECO_SDK_HOME 时不覆盖，
/// 否则探测 DevEco Studio 内置 SDK 根目录（sdk/default/sdk-pkg.json 布局），
/// 未发现时回退复用环境探测的 command-line-tools 内置 SDK。
#[cfg(windows)]
fn hvigor_env() -> Vec<(String, String)> {
    if std::env::var("DEVECO_SDK_HOME").is_ok() {
        return Vec::new();
    }
    if let Some(sdk) = find_deveco_toolchain().map(|(_, sdk)| sdk) {
        return vec![("DEVECO_SDK_HOME".to_string(), sdk.to_string_lossy().to_string())];
    }
    // command-line-tools 内置 SDK（官方包自带 sdk/ 目录）
    if let Some(cli) = crate::services::harmony_env::cached_cli_root() {
        let sdk = cli.join("sdk");
        if sdk.join("default").join("sdk-pkg.json").is_file() {
            return vec![("DEVECO_SDK_HOME".to_string(), sdk.to_string_lossy().to_string())];
        }
    }
    Vec::new()
}

/// 非 Windows 平台无 DevEco Studio 桌面端（hvigor 依赖 HarmonyOS CLI 工具链），返回 None。
#[cfg(not(windows))]
pub(crate) fn find_deveco_toolchain() -> Option<(PathBuf, PathBuf)> {
    None
}

/// 探测 DevEco Studio 内置工具链：hvigor 启动器 + HarmonyOS SDK 根目录。
/// 候选顺序：环境变量 DEVECO_HOME/DEVECO_STUDIO_HOME → 默认安装目录 → Huawei 目录下扫描 → 注册表 Uninstall。
/// SDK 根要求存在 `sdk/default/sdk-pkg.json`（DevEco 6.x 内置 SDK 布局：
/// hvigor 的 SDK 扫描器只找 {SDK_ROOT}/<子目录>/sdk-pkg.json，因此 DEVECO_SDK_HOME
/// 必须指向 sdk-pkg.json 所在目录（default）的父目录，指向 sdk/default 或
/// sdk/default/openharmony 都会报 00303312）。
#[cfg(windows)]
pub(crate) fn find_deveco_toolchain() -> Option<(PathBuf, PathBuf)> {
    fn probe(root: &Path) -> Option<(PathBuf, PathBuf)> {
        let hvigorw = root.join("tools").join("hvigor").join("bin").join("hvigorw.js");
        let sdk = root.join("sdk");
        (hvigorw.is_file() && sdk.join("default").join("sdk-pkg.json").is_file())
            .then(|| (hvigorw, sdk))
    }
    for var in ["DEVECO_HOME", "DEVECO_STUDIO_HOME"] {
        if let Ok(v) = std::env::var(var) {
            if let Some(p) = probe(Path::new(&v)) {
                return Some(p);
            }
        }
    }
    let base = Path::new(r"C:\Program Files\Huawei");
    if let Some(p) = probe(&base.join("DevEco Studio")) {
        return Some(p);
    }
    // 多版本共存（DevEco Studio 5.0.2 等）时扫描子目录
    if let Ok(entries) = std::fs::read_dir(base) {
        let mut hits: Vec<((PathBuf, PathBuf), std::time::SystemTime)> = entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("DevEco"))
            .filter_map(|e| {
                let p = probe(&e.path())?;
                let t = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                Some((p, t))
            })
            .collect();
        hits.sort_by_key(|(_, t)| *t);
        if let Some((p, _)) = hits.pop() {
            return Some(p);
        }
    }
    // 注册表 Uninstall 键（64/32 位视图）中的 DevEco Studio 安装位置
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;
    for sub in [
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    ] {
        if let Ok(key) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(sub, KEY_READ) {
            for name in key.enum_keys().flatten() {
                if !name.to_lowercase().contains("deveco") {
                    continue;
                }
                if let Ok(app) = key.open_subkey_with_flags(&name, KEY_READ) {
                    if let Ok(loc) = app.get_value::<String, _>("InstallLocation") {
                        if let Some(p) = probe(Path::new(&loc)) {
                            return Some(p);
                        }
                    }
                }
            }
        }
    }
    None
}

/// 收集工程全部 oh-package.json5 声明的依赖（含 devDependencies），
/// 返回 (模块名，空串=根模块, 依赖名) 列表；JSON5 解析失败/无文件时跳过该模块。
pub fn collect_ohpm_deps(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut scan = |path: &Path, module: &str| {
        let Some(text) = read_to_string_opt(&path.join("oh-package.json5")) else { return };
        let Ok(v) = parse_json5(&text) else { return };
        for key in ["dependencies", "devDependencies"] {
            if let Some(map) = v.get(key).and_then(|x| x.as_object()) {
                for (name, _) in map {
                    out.push((module.to_string(), name.clone()));
                }
            }
        }
    };
    scan(root, "");
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "oh_modules" || name == "node_modules" {
                continue;
            }
            if p.join("oh-package.json5").is_file() {
                scan(&p, &name);
            }
        }
    }
    out
}

/// 核对 ohpm install 结果：无依赖时明示（消除“21ms 假成功”的困惑），
/// 有依赖时检查各模块 oh_modules 是否已包含对应包（scoped 包按 @scope/name 两级目录）。
/// 返回可直接展示的核对文本。
pub fn verify_ohpm_install(root: &Path, log: &str) -> String {
    let deps = collect_ohpm_deps(root);
    if deps.is_empty() {
        return format!(
            "{}\n\n工程未声明任何依赖（所有 oh-package.json5 的 dependencies 均为空），因此无需下载任何包。\n若子模块（如 entry）的依赖被注释，解除注释后重新安装即可。",
            log.trim()
        );
    }
    let mut missing = Vec::new();
    let mut installed = Vec::new();
    for (module, name) in &deps {
        let dirs: Vec<PathBuf> = if module.is_empty() {
            vec![root.join("oh_modules")]
        } else {
            vec![root.join(module).join("oh_modules"), root.join("oh_modules")]
        };
        let ok = dirs.iter().any(|om| {
            let target = if let Some((scope, pkg)) = name.split_once('/') {
                om.join(scope).join(pkg)
            } else {
                om.join(name)
            };
            target.is_dir()
        });
        let label = if module.is_empty() {
            name.clone()
        } else {
            format!("{module}/{name}")
        };
        if ok {
            installed.push(label);
        } else {
            missing.push(label);
        }
    }
    let tail = log.trim();
    if missing.is_empty() {
        format!(
            "{}\n\n依赖核对通过：{} 个依赖已安装（{}）",
            tail,
            installed.len(),
            installed.join(", ")
        )
    } else {
        format!(
            "{}\n\n依赖核对未通过，以下依赖未在 oh_modules 中找到（{}）：{}\n请检查网络/registry 配置后重试，或在 DevEco Studio 中 Sync。",
            tail,
            missing.len(),
            missing.join(", ")
        )
    }
}

/// 解析构建日志，返回结构化错误（按 HARMONY_INTEGRATION.md §4.1 正则库）。
pub fn parse_build_errors(log: &str) -> Vec<BuildError> {
    let mut errors = Vec::new();
    for line in log.lines() {
        if let Some(e) = match_error_line(line) {
            errors.push(e);
        }
    }
    // 去重
    errors.dedup_by(|a, b| a.kind == b.kind && a.file == b.file && a.line == b.line && a.message == b.message);
    errors
}

fn match_error_line(line: &str) -> Option<BuildError> {
    // 1. ArkTS:ERROR File: xxx.ets:line:col
    if let Some(e) = parse_arkts_error(line, "ArkTS:ERROR File:") {
        return Some(e);
    }
    // 2. 旧版 ERROR File: xxx:line:col
    if let Some(e) = parse_arkts_error(line, "ERROR File:") {
        return Some(e);
    }
    let lower = line.to_ascii_lowercase();
    if lower.contains("failed to resolve dependency") || lower.contains("cannot find module") {
        return Some(BuildError {
            kind: "dependency".into(),
            category: "dependency".into(),
            file: None, line: None, column: None,
            message: line.trim().to_string(),
            suggestion: "执行 ohpm install 后重试；检查 oh-package.json5 依赖声明".into(),
        });
    }
    if lower.contains("sign") && (lower.contains("fail") || lower.contains("error"))
        || lower.contains("signing configuration")
        || lower.contains("certificate") && lower.contains("expired")
    {
        return Some(BuildError {
            kind: "signing".into(),
            category: "signing".into(),
            file: None, line: None, column: None,
            message: line.trim().to_string(),
            suggestion: "检查 build-profile.json5 的 signingConfigs，在 DevEco Studio 重新配置签名".into(),
        });
    }
    if lower.contains("sdk not found")
        || lower.contains("compatiblesdkversion")
        || lower.contains("cannot find sdk")
    {
        return Some(BuildError {
            kind: "sdk".into(),
            category: "sdk".into(),
            file: None, line: None, column: None,
            message: line.trim().to_string(),
            suggestion: "检查 compatibleSdkVersion 与已安装 HarmonyOS SDK 是否匹配".into(),
        });
    }
    // hvigor 的 SDK 路径错误码：00303217 = DEVECO_SDK_HOME 未设置/路径不存在；
    // 00303312 = 扫描不到 SDK 组件（DEVECO_SDK_HOME 须指向 sdk 根目录，即 default 的父目录）
    if lower.contains("00303217") || lower.contains("00303312") {
        return Some(BuildError {
            kind: "sdk".into(),
            category: "sdk".into(),
            file: None, line: None, column: None,
            message: line.trim().to_string(),
            suggestion: "DEVECO_SDK_HOME 须指向 DevEco Studio 的 sdk 根目录（含 default\\sdk-pkg.json 的父目录），如 C:\\Program Files\\Huawei\\DevEco Studio\\sdk；指向 sdk\\default 或其子目录会扫描不到 SDK 组件（00303312）。应用会自动探测注入；若手动设置了该变量，请取消设置或修正指向".into(),
        });
    }
    if lower.contains("ohpm") && (lower.contains("error") || lower.contains("enoent")) {
        return Some(BuildError {
            kind: "ohpm".into(),
            category: "ohpm".into(),
            file: None, line: None, column: None,
            message: line.trim().to_string(),
            suggestion: "检查 ohpm 工具链路径，或在 DevEco Studio 中重新安装 ohpm".into(),
        });
    }
    None
}

/// 手写解析 ArkTS 编译错误行：`...File: <path>:<line>:<col>[: ]<message>`。
/// Windows 路径含盘符冒号，因此不能简单 split(':')；正向扫描定位首个 `:<digits>:<digits>` 模式。
fn parse_arkts_error(line: &str, marker: &str) -> Option<BuildError> {
    let idx = line.find(marker)?;
    let rest = line[idx + marker.len()..].trim_start();
    let b = rest.as_bytes();
    // 在 rest 中找第一个 ':' 后紧跟数字、再 ':'、再数字的位置
    let mut i = 0;
    let (line_start, line_end, col_start, col_end) = loop {
        if i >= b.len() {
            return None;
        }
        if b[i] == b':' {
            // 尝试读取数字
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 && j < b.len() && b[j] == b':' {
                let k = j + 1;
                let mut m = k;
                while m < b.len() && b[m].is_ascii_digit() {
                    m += 1;
                }
                if m > k {
                    // 确保这个冒号不是盘符（盘符形式 C:，冒号后是 '\' 或 '/'）
                    // 通过要求"前面已有一个冒号且文件部分非空"来排除盘符
                    break (i + 1, j, k, m);
                }
            }
        }
        i += 1;
    };
    let line_num: i64 = rest[line_start..line_end].parse().ok()?;
    let col_num: i64 = rest[col_start..col_end].parse().ok()?;
    let file = rest[..i].trim();
    if file.is_empty() {
        return None;
    }
    let message = rest[col_end..].trim_start_matches(':').trim().to_string();
    let category = classify_message("arkts", &message);
    Some(BuildError {
        kind: "arkts".into(),
        category,
        file: Some(file.to_string()),
        line: Some(line_num),
        column: Some(col_num),
        message,
        suggestion: "读取对应文件行号，修复 ArkTS 语法/类型错误后重新构建".into(),
    })
}

/// 容错 JSON5 解析：剥离 // 与 /* */ 注释、去除尾逗号后用 serde_json 解析。
pub fn parse_json5(text: &str) -> Result<serde_json::Value, String> {
    let stripped = strip_jsonc_comments(text);
    let cleaned = strip_trailing_commas(&stripped);
    serde_json::from_str(&cleaned).map_err(|e| e.to_string())
}

fn strip_jsonc_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut in_string = false;
    let mut string_quote = b'"';
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            out.push(c as char);
            if c == b'\\' && i + 1 < bytes.len() {
                out.push(bytes[i + 1] as char);
                i += 2;
                continue;
            }
            if c == string_quote {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == b'"' || c == b'\'' {
            // JSON5 单引号字符串：归一化为双引号
            in_string = true;
            string_quote = c;
            out.push('"');
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'/' {
                // 行注释，跳到行尾
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if bytes[i + 1] == b'*' {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
        }
        out.push(c as char);
        i += 1;
    }
    out
}

fn strip_trailing_commas(text: &str) -> String {
    // 去除 } 或 ] 前的逗号（手写扫描，避免引入 regex 依赖）
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' {
            in_string = !in_string;
            out.push(c as char);
            i += 1;
            continue;
        }
        if !in_string && c == b',' {
            // 向后看，跳过空白后是否为 } 或 ]
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\n' || bytes[j] == b'\r') {
                j += 1;
            }
            if j < bytes.len() && (bytes[j] == b'}' || bytes[j] == b']') {
                i += 1;
                continue;
            }
        }
        out.push(c as char);
        i += 1;
    }
    out
}

fn read_to_string_opt(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// 从统一语义图收集指定模块页面，兼容旧 `get_project_info.pages` 字段。
pub fn collect_routes(root: &Path, entry_module: Option<&str>) -> Vec<String> {
    let model = crate::services::harmony_model::parse(root);
    routes_from_model(&model, entry_module)
}

pub fn routes_from_model(
    model: &crate::services::harmony_model::HarmonySemanticModel,
    entry_module: Option<&str>,
) -> Vec<String> {
    let module = entry_module
        .map(String::from)
        .or_else(|| {
            model
                .modules
                .iter()
                .find(|module| module.kind == "entry")
                .map(|module| module.rel_path.clone())
        })
        .unwrap_or_else(|| "entry".into());
    let mut routes = model
        .graph
        .pages
        .iter()
        .filter(|page| page.module == module)
        .map(|page| page.path.clone())
        .collect::<Vec<_>>();
    routes.sort();
    routes.dedup();
    routes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_api_version() {
        assert_eq!(parse_api_version("5.0.0(12)"), Some(12));
        assert_eq!(parse_api_version("4.1.0(11)"), Some(11));
        assert_eq!(parse_api_version("12"), Some(12));
    }

    #[test]
    fn test_preferred_hap_dir_keeps_signed_artifact_priority() {
        let nonce = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("harmony-signed-hap-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let signed = dir.join("entry-default-signed.hap");
        let unsigned = dir.join("entry-default-unsigned.hap");
        std::fs::write(&signed, b"signed").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&unsigned, b"unsigned").unwrap();

        assert_eq!(latest_hap_in_dir(&dir).as_deref(), Some(signed.as_path()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_is_project_root_distinguishes_module_dir() {
        // 回归：模块级 build-profile.json5（无顶层 "app" 键）不得被误判为工程根，
        // 否则 entry 模块目录会抢占主工程根，导致 Agent 写文件/构建全部错位（entry/entry 嵌套事故）
        let dir = std::env::temp_dir().join(format!("isprojroot-{}", std::process::id()));
        let proj = dir.join("proj");
        let modu = proj.join("entry");
        std::fs::create_dir_all(modu.join("src/main")).unwrap();
        std::fs::create_dir_all(proj.join("AppScope")).unwrap();
        std::fs::write(proj.join("AppScope/app.json5"), r#"{"app":{"bundleName":"com.x"}}"#).unwrap();
        // 根级 build-profile：含 app 键（products/signingConfigs/modules）
        std::fs::write(proj.join("build-profile.json5"), r#"{"app":{"signingConfigs":[]},"modules":[]}"#).unwrap();
        // 模块级 build-profile：只有 apiType/buildOption/targets，无 app 键
        std::fs::write(modu.join("build-profile.json5"), r#"{"apiType":"stageMode","buildOption":{},"targets":[{"name":"default"}]}"#).unwrap();
        std::fs::write(modu.join("oh-package.json5"), r#"{"name":"entry"}"#).unwrap();
        std::fs::write(modu.join("hvigorfile.ts"), "").unwrap();

        assert!(is_project_root(&proj), "工程根（AppScope + 含 app 键的根 build-profile）应判定为 true");
        assert!(!is_project_root(&modu), "entry 模块目录不得被误判为工程根");
        assert!(!is_project_root(&proj.join("src")), "无配置的普通目录应为 false");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_strip_comments_and_trailing() {
        let s = r#"{
            "a": 1, // comment
            "b": { "c": 2, },
            /* block */ "d": "str//ing",
        }"#;
        let v: serde_json::Value = parse_json5(s).unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"]["c"], 2);
        assert_eq!(v["d"], "str//ing");
    }

    #[test]
    fn test_parse_arkts_error() {
        let line = "ERROR: ArkTS:ERROR File: D:/app/entry/src/main/ets/pages/Home.ets:23:5 Object literal must correspond";
        let errs = parse_build_errors(line);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].kind, "arkts");
        assert_eq!(errs[0].line, Some(23));
        assert!(errs[0].file.as_ref().unwrap().ends_with("Home.ets"));
    }

    #[test]
    fn test_parse_sdk_env_error_codes() {
        // 00303312：DEVECO_SDK_HOME 指向错误导致扫描不到 SDK 组件
        let line = "ERROR: [00303312] Cannot find the corresponding SDK version.";
        let errs = parse_build_errors(line);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].kind, "sdk");
        assert!(errs[0].suggestion.contains("DEVECO_SDK_HOME"), "建议应指向 SDK 根目录");
        assert!(errs[0].suggestion.contains("sdk"));
        // 00303217：环境变量未设置/路径无效
        let line2 = "[00303217] Invalid value of DEVECO_SDK_HOME in the system environment path";
        let errs2 = parse_build_errors(line2);
        assert_eq!(errs2.len(), 1);
        assert_eq!(errs2[0].kind, "sdk");
        assert!(errs2[0].suggestion.contains("DEVECO_SDK_HOME"));
    }

    #[test]
    fn test_classify_message() {
        assert_eq!(classify_message("arkts", "Cannot find module 'ohos.router'"), "dependency");
        assert_eq!(classify_message("arkts", "Type 'string' is not assignable to type 'number'"), "type");
        assert_eq!(classify_message("arkts", "Syntax error: unexpected token"), "syntax");
        assert_eq!(classify_message("arkts", "Resource not found: app.string.title"), "resource");
        assert_eq!(classify_message("arkts", "This API requires API version 12 or higher"), "api_level");
        assert_eq!(classify_message("signing", "Signing configuration error"), "signing");
        assert_eq!(classify_message("sdk", "SDK not found"), "sdk");
        assert_eq!(classify_message("ohpm", "ohpm install failed"), "ohpm");
        assert_eq!(classify_message("arkts", "some unknown weird failure"), "other");
    }

    #[test]
    fn test_clean_args() {
        let args = clean_args();
        assert!(args.contains(&"clean".to_string()));
        assert!(args.contains(&"--no-daemon".to_string()));
    }

    #[test]
    fn test_find_deveco_toolchain_shape() {
        // 探测结果必须自洽：hvigorw.js 与 sdk/default/sdk-pkg.json 同根存在；
        // 用户显式设置 DEVECO_SDK_HOME 时不注入
        if let Some((hvigorw, sdk)) = find_deveco_toolchain() {
            assert!(hvigorw.is_file(), "hvigorw 不存在: {}", hvigorw.display());
            assert!(
                sdk.join("default").join("sdk-pkg.json").is_file(),
                "sdk-pkg.json 不存在: {}",
                sdk.join("default").join("sdk-pkg.json").display()
            );
        }
        #[cfg(windows)]
        if std::env::var("DEVECO_SDK_HOME").is_ok() {
            assert!(hvigor_env().is_empty(), "用户显式配置时应保持不注入");
        }
    }

    #[test]
    fn test_hvigor_command_skips_broken_bat() {
        // bat 引用缺失的 hvigor-wrapper.js（如拷贝/被忽略丢失）时不应选 bat 分支，
        // 否则 bat 回退调用全局 hvigorw 命令静默失败；应落到 DevEco 内置工具链或报错。
        let root = std::env::temp_dir().join(format!("hvigor-broken-bat-{}", std::process::id()));
        std::fs::create_dir_all(root.join("hvigor")).unwrap();
        std::fs::write(
            root.join("hvigorw.bat"),
            "@echo off\r\nif exist \"%~dp0hvigor\\hvigor-wrapper.js\" (\r\n  node \"%~dp0hvigor\\hvigor-wrapper.js\" %*\r\n) else (\r\n  hvigorw %*\r\n)\r\n",
        )
        .unwrap();
        let bat_path = root.join("hvigorw.bat").to_string_lossy().to_string();
        match hvigor_command(&root) {
            Ok(cmd) => assert_ne!(
                cmd.program, bat_path,
                "不应选择引用缺失 wrapper 的 bat 分支"
            ),
            Err(e) => assert!(e.contains("hvigor"), "错误信息应提示构建脚本缺失: {}", e),
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn test_hvigor_command_uses_toolkit_engine() {
        // 软件内置 toolkits 自带 hvigor 引擎时（未装 DevEco Studio 也能构建），
        // 工程无启动脚本也应落到 <toolkits>/<name>/hvigor/bin/hvigorw.js
        let root = std::env::temp_dir().join(format!("hvigor-toolkit-{}", std::process::id()));
        let tk = root.join("toolkits").join("command-line-tools");
        std::fs::create_dir_all(tk.join("hvigor").join("bin")).unwrap();
        std::fs::write(tk.join("hvigor").join("bin").join("hvigorw.js"), "// engine").unwrap();
        let prev = crate::services::harmony_env::get_bundled_cli_dir();
        crate::services::harmony_env::set_bundled_cli_dir(Some(tk.clone()));
        let proj = root.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let cmd = hvigor_command(&proj).expect("应落到 toolkits hvigor 引擎");
        assert_eq!(cmd.program, "node");
        assert!(cmd.args[0].ends_with("hvigorw.js"), "args={:?}", cmd.args);
        crate::services::harmony_env::set_bundled_cli_dir(prev);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn test_hvigor_command_uses_cli_root_engine() {
        // 环境探测发现 command-line-tools（如盘符根目录 H:\command-line-tools）时，
        // 工程无启动脚本也应复用其 hvigor 引擎，而非纸面可用
        let root = std::env::temp_dir().join(format!("hvigor-cli-{}", std::process::id()));
        let cli = root.join("command-line-tools");
        std::fs::create_dir_all(cli.join("hvigor").join("bin")).unwrap();
        std::fs::write(cli.join("hvigor").join("bin").join("hvigorw.js"), "// engine").unwrap();
        crate::services::harmony_env::set_cached_cli_root_for_test(Some(cli.clone()));
        let proj = root.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let cmd = hvigor_command(&proj).expect("应复用 command-line-tools hvigor 引擎");
        assert_eq!(cmd.program, "node");
        assert!(cmd.args[0].ends_with("hvigorw.js"), "args={:?}", cmd.args);
        crate::services::harmony_env::set_cached_cli_root_for_test(None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_assemble_args_module_mode() {
        let args = assemble_args(Some("entry"), "release");
        assert!(args.contains(&"assembleHap".to_string()));
        assert!(args.iter().any(|a| a.contains("buildMode=release")));
        assert!(args.iter().any(|a| a.contains("entry@default")));
    }

    #[cfg(windows)]
    #[test]
    fn test_hvigor_command_wrapper_priority() {
        // 工程内 hvigor/hvigor-wrapper.js 存在 → node 直调 wrapper
        let root = std::env::temp_dir().join(format!("hvigor-cmd-{}", std::process::id()));
        std::fs::create_dir_all(root.join("hvigor")).unwrap();
        std::fs::write(root.join("hvigor/hvigor-wrapper.js"), "// wrapper").unwrap();
        let cmd = hvigor_command(&root).expect("应解析到 wrapper 直调");
        assert_eq!(cmd.program, "node");
        assert_eq!(cmd.args.len(), 1);
        assert!(cmd.args[0].ends_with("hvigor-wrapper.js"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn test_hvigor_command_bat_fallback() {
        // 无 wrapper、有 hvigorw.bat → 回退完整路径 bat
        let root = std::env::temp_dir().join(format!("hvigor-cmd-bat-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("hvigorw.bat"), "@echo off").unwrap();
        let cmd = hvigor_command(&root).expect("应回退到工程内 hvigorw.bat");
        assert!(cmd.program.ends_with("hvigorw.bat"), "prog={}", cmd.program);
        assert!(cmd.args.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_collect_ohpm_deps_all_modules() {
        let root = std::env::temp_dir().join(format!("hvigor-deps-{}", std::process::id()));
        std::fs::create_dir_all(root.join("entry")).unwrap();
        std::fs::write(
            root.join("oh-package.json5"),
            r#"{"dependencies": {"@ohos/video_processing": "^1.0.0"}}"#,
        )
        .unwrap();
        // entry 的依赖含注释与 devDependencies，应都能收集
        std::fs::write(
            root.join("entry/oh-package.json5"),
            "// 注释\n{\n  \"dependencies\": { // 行内注释\n    \"@ohos/hypium\": \"1.0.19\",\n  },\n  \"devDependencies\": { \"@ohos/lottie\": \"^2.0.0\" }\n}",
        )
        .unwrap();
        let deps = collect_ohpm_deps(&root);
        assert_eq!(deps.len(), 3);
        assert!(deps.contains(&("".to_string(), "@ohos/video_processing".to_string())));
        assert!(deps.contains(&("entry".to_string(), "@ohos/hypium".to_string())));
        assert!(deps.contains(&("entry".to_string(), "@ohos/lottie".to_string())));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_verify_ohpm_install_no_deps() {
        // 工程无依赖：提示“未声明依赖”，而不是含糊的完成
        let root = std::env::temp_dir().join(format!("hvigor-nodeps-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("oh-package.json5"), r#"{"dependencies": {}}"#).unwrap();
        let text = verify_ohpm_install(&root, "install completed in 0s 21ms");
        assert!(text.contains("未声明任何依赖"), "text={text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_verify_ohpm_install_missing_dep() {
        // 有依赖但 oh_modules 缺包：应列出缺失清单
        let root = std::env::temp_dir().join(format!("hvigor-miss-{}", std::process::id()));
        std::fs::create_dir_all(root.join("oh_modules/@ohos/hypium")).unwrap();
        std::fs::create_dir_all(root.join("entry")).unwrap();
        std::fs::write(
            root.join("oh-package.json5"),
            r#"{"dependencies": {"@ohos/hypium": "1.0.19", "@ohos/missing": "^1.0.0"}}"#,
        )
        .unwrap();
        let text = verify_ohpm_install(&root, "ok");
        assert!(text.contains("@ohos/missing"), "应列出缺失依赖: {text}");
        assert!(text.contains("依赖核对未通过"), "text={text}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
