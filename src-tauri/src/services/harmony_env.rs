//! 鸿蒙 SDK / command-line-tools 环境统一探测与配置持久化。
//!
//! 职责：
//! - 自动发现 HarmonyOS SDK 根目录（DevEco Studio 自带 sdk、注册表、常见安装路径）
//! - 自动发现 command-line-tools（H:\command-line-tools、SDK 同级目录等）
//! - 解析 SDK 下已安装的 API 版本（default/open源、ets/native 层级）
//! - 持久化用户手动指定的路径（settings 表），优先于自动发现
//! - 产出统一 `HarmonyEnv`，供对话 system prompt 注入与子进程 PATH 注入使用

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::db::queries;
use crate::db::DbState;

/// settings 表中存储用户环境配置的 key
const CFG_KEY: &str = "harmony_env";

/// 单个已安装的 SDK 组件（ets/native/js/toolchains/previewer），属於某个变体下的某个版本
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkComponent {
    /// 组件名（ets/native/js/toolchains/previewer）
    pub name: String,
    /// API 版本号（来自 oh-uni-package.json 的 apiVersion，如 "24"）
    pub api_version: String,
    /// 完整版本（如 "6.1.1.125"）
    pub version: Option<String>,
    /// 组件根目录
    pub path: String,
    /// API 声明文件目录（ets/api，仅 ets 组件有）
    pub api_dir: Option<String>,
}

/// 一个 SDK 变体：openharmony（开源）或 hms（华为商用）。
/// 新版 SDK（DevEco Studio 4.1+）采用扁平布局：sdk/<variant>/ets/...
/// 旧版 SDK 采用版本目录布局：sdk/<variant>/<apiVersion>/ets/...
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkVariant {
    /// 变体名：openharmony 或 hms
    pub variant: String,
    /// 变体根目录
    pub path: String,
    /// 该变体下的组件（以"当前激活版本"为准）
    pub components: Vec<SdkComponent>,
    /// API 版本号
    pub api_version: Option<String>,
    /// 是否为默认变体（default 软链/目录指向）
    pub is_default: bool,
}

/// 命令行工具信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandLineTools {
    /// command-line-tools 根目录
    pub root: String,
    /// bin 目录（加入 PATH）
    pub bin: String,
    /// 关键工具是否存在
    pub has_hdc: bool,
    pub has_ohpm: bool,
    pub has_hvigorw: bool,
}

/// 统一鸿蒙环境快照
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarmonyEnv {
    /// SDK 根目录
    pub sdk_root: Option<String>,
    /// 默认 API 版本（从默认变体的 ets 组件 oh-uni-package.json 读取）
    pub default_api: Option<String>,
    /// SDK 变体列表（openharmony / hms），每个变体含其组件与版本
    pub sdk_variants: Vec<SdkVariant>,
    /// 向后兼容：已安装 API 版本号列表（取各变体 ets 组件的 api_version 去重）
    pub sdk_versions: Vec<String>,
    /// command-line-tools 信息
    pub cli: Option<CommandLineTools>,
    /// hdc 可执行文件绝对路径（优先取 command-line-tools/hdc，回退 SDK/PATH）
    pub hdc_path: Option<String>,
    /// hdc 实际来源：cli=command-line-tools / sdk=SDK toolchains / deveco=DevEco 安装目录 / path=系统 PATH
    pub hdc_source: Option<String>,
    /// ohpm 可执行文件绝对路径
    pub ohpm_path: Option<String>,
    /// hvigorw 包装脚本绝对路径（若在工程外能找到）
    pub hvigorw_path: Option<String>,
    /// DevEco Studio 安装目录（若发现）
    pub studio_dir: Option<String>,
    /// 配置来源：auto（自动发现）或 manual（用户手动指定）
    pub source: String,
    /// 未找到但推荐检查的常见路径（提示用户手动指定）
    pub suggestions: Vec<String>,
}

/// 用户可手动覆盖的配置
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyEnvConfig {
    pub sdk_root: Option<String>,
    pub cli_root: Option<String>,
}

/// 进程内缓存（避免每次对话都扫盘）
static CACHE: Mutex<Option<HarmonyEnv>> = Mutex::new(None);

/// 软件内置工具链目录（app_data/toolkits/command-line-tools）；lib.rs setup 时注册，
/// 探测优先级：用户手动配置 > 软件内置 > 常见候选（DevEco Studio / 盘符目录）。
static BUNDLED_CLI_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// 注册软件内置工具链目录（lib.rs setup 时调用）
pub fn set_bundled_cli_dir(dir: Option<PathBuf>) {
    *BUNDLED_CLI_DIR.lock().unwrap() = dir;
}

/// 读取软件内置工具链目录（探测时作为 cli 候选）
pub fn get_bundled_cli_dir() -> Option<PathBuf> {
    BUNDLED_CLI_DIR.lock().unwrap().clone()
}

/// 读取最近一次探测缓存中的 command-line-tools 根目录（盘符扫描/手动配置/软件内置均可能）。
/// 供构建端复用：探测报告 hvigorw 可用时，构建必须能真正找到它（避免“纸面工具链”）。
pub fn cached_cli_root() -> Option<PathBuf> {
    CACHE
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|e| e.cli.as_ref())
        .map(|c| PathBuf::from(&c.root))
}

/// 测试用：注入探测缓存（模拟已探测到指定 command-line-tools）
#[cfg(test)]
pub fn set_cached_cli_root_for_test(root: Option<PathBuf>) {
    let mut cache = CACHE.lock().unwrap();
    *cache = root.map(|r| HarmonyEnv {
        cli: Some(CommandLineTools {
            root: r.to_string_lossy().to_string(),
            bin: String::new(),
            has_hdc: false,
            has_ohpm: false,
            has_hvigorw: false,
        }),
        sdk_root: None,
        default_api: None,
        sdk_variants: Vec::new(),
        sdk_versions: Vec::new(),
        hdc_path: None,
        hdc_source: None,
        ohpm_path: None,
        hvigorw_path: None,
        studio_dir: None,
        source: "test".to_string(),
        suggestions: Vec::new(),
    });
}

// ---------- 配置持久化 ----------

/// 读取用户持久化的环境配置
pub fn load_config(state: &DbState) -> HarmonyEnvConfig {
    match state.0.lock() {
        Ok(conn) => queries::get_setting(&conn, CFG_KEY)
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default(),
        Err(_) => HarmonyEnvConfig::default(),
    }
}

/// 保存用户环境配置（手动指定的路径），并失效缓存
pub fn save_config(state: &DbState, cfg: &HarmonyEnvConfig) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let v = serde_json::to_string(cfg).map_err(|e| e.to_string())?;
    queries::set_setting(&conn, CFG_KEY, &v).map_err(|e| e.to_string())?;
    let _ = CACHE.lock().map(|mut c| *c = None);
    Ok(())
}

/// 失效缓存（环境变化后调用）
pub fn invalidate_cache() {
    if let Ok(mut c) = CACHE.lock() {
        *c = None;
    }
}

// ---------- 路径探测 ----------

/// 发现 DevEco Studio 安装目录（复用 health 模块的注册表 + 常见路径策略）
pub fn discover_studio_dir() -> Option<PathBuf> {
    crate::commands::health::discover_deveco_dirs().into_iter().next()
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// 读取组件目录下的 oh-uni-package.json，返回 (apiVersion, version)
fn read_uni_package(comp_dir: &Path) -> (Option<String>, Option<String>) {
    let pkg_path = comp_dir.join("oh-uni-package.json");
    let Ok(text) = std::fs::read_to_string(&pkg_path) else {
        return (None, None);
    };
    let api = extract_json_string(&text, "apiVersion");
    let ver = extract_json_string(&text, "version");
    (api, ver)
}

/// 从 JSON 文本中粗略提取字符串字段值（oh-uni-package.json 字段均为简单字符串）
fn extract_json_string(text: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let idx = text.find(&pat)?;
    let rest = &text[idx + pat.len()..];
    let colon = rest.find(':')?;
    let after = &rest[colon + 1..];
    let first_quote = after.find('"')?;
    let val = &after[first_quote + 1..];
    let end = val.find('"')?;
    Some(val[..end].trim().to_string())
}

/// 枚举单个版本目录下的组件（ets/native/js/toolchains/previewer）
fn scan_components(version_dir: &Path) -> Vec<SdkComponent> {
    let mut comps = Vec::new();
    let Ok(entries) = std::fs::read_dir(version_dir) else { return comps };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !e.path().is_dir() || name.starts_with('.') {
            continue;
        }
        let p = e.path();
        // read_uni_package 内部已对读取失败返回 (None, None)
        let (api_version, version) = read_uni_package(&p);
        let api_dir = if name == "ets" {
            let d = p.join("api");
            d.is_dir().then(|| d.to_string_lossy().to_string())
        } else {
            None
        };
        comps.push(SdkComponent {
            name,
            api_version: api_version.unwrap_or_default(),
            version,
            path: p.to_string_lossy().to_string(),
            api_dir,
        });
    }
    comps
}

/// 校验某个目录是否像 SDK 根：含 openharmony/hms 变体目录或 default 目录
fn looks_like_sdk_root(dir: &Path) -> bool {
    for sub in ["openharmony", "hms", "default"] {
        let p = dir.join(sub);
        if p.is_dir()
            && (p.join("ets").is_dir()
                || p.join("oh-uni-package.json").is_file()
                || p.join("openharmony").join("ets").is_dir()
                || p.join("hms").join("ets").is_dir())
        {
            return true;
        }
    }
    // 旧版布局：sdk/openharmony/<digit>/ets
    for sub in ["openharmony", "hms"] {
        let variant = dir.join(sub);
        if variant.is_dir() {
            if let Ok(entries) = std::fs::read_dir(variant) {
                for e in entries.flatten() {
                    let n = e.file_name().to_string_lossy().to_string();
                    if n.chars().next().is_some_and(|c| c.is_ascii_digit())
                        && e.path().join("ets").is_dir()
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// 探测 DevEco Studio 内置 SDK（与构建 hvigor 注入的 DEVECO_SDK_HOME 一致）：
/// 要求存在 hvigorw.js 与 sdk/default/sdk-pkg.json（hvigor 的 SDK 扫描布局）
fn discover_deveco_bundled_sdk() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = discover_studio_dir().into_iter().collect();
    if cfg!(target_os = "windows") {
        for p in [
            r"C:\Program Files\Huawei\DevEco Studio",
            r"D:\Program Files\Huawei\DevEco Studio",
            r"D:\DevEco Studio",
        ] {
            roots.push(PathBuf::from(p));
        }
    }
    for r in roots {
        let sdk = r.join("sdk");
        if r.join("tools").join("hvigor").join("bin").join("hvigorw.js").is_file()
            && sdk.join("default").join("sdk-pkg.json").is_file()
        {
            return Some(sdk);
        }
    }
    None
}

/// 解析一个变体目录（openharmony 或 hms）为 SdkVariant。
/// 新版扁平布局：<variant>/ets/...；旧版版本目录：<variant>/<apiVersion>/ets/...
fn parse_variant(variant_dir: &Path, name: &str, is_default: bool) -> Option<SdkVariant> {
    if !variant_dir.is_dir() {
        return None;
    }
    if variant_dir.join("ets").is_dir() || variant_dir.join("oh-uni-package.json").is_file() {
        let components = scan_components(variant_dir);
        let api_version = components
            .iter()
            .find(|c| c.name == "ets")
            .map(|c| c.api_version.clone())
            .filter(|s| !s.is_empty());
        return Some(SdkVariant {
            variant: name.to_string(),
            path: variant_dir.to_string_lossy().to_string(),
            components,
            api_version,
            is_default,
        });
    }
    // 旧版：扫描数字版本子目录，取最新
    let mut latest: Option<(i64, PathBuf)> = None;
    if let Ok(entries) = std::fs::read_dir(variant_dir) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if !e.path().is_dir() {
                continue;
            }
            if let Ok(num) = n.parse::<i64>() {
                if latest.as_ref().is_none_or(|(m, _)| num > *m) {
                    latest = Some((num, e.path()));
                }
            }
        }
    }
    if let Some((num, ver_dir)) = latest {
        if ver_dir.join("ets").is_dir() {
            let components = scan_components(&ver_dir);
            return Some(SdkVariant {
                variant: name.to_string(),
                path: ver_dir.to_string_lossy().to_string(),
                components,
                api_version: Some(num.to_string()),
                is_default,
            });
        }
    }
    None
}

/// 扫描 SDK 根目录，返回所有变体与默认 API 版本
fn parse_sdk(sdk_root: &Path) -> (Vec<SdkVariant>, Option<String>) {
    let mut variants = Vec::new();
    let default_root = sdk_root.join("default");
    for name in ["openharmony", "hms"] {
        let in_default = default_root.join(name);
        if in_default.is_dir()
            && (in_default.join("ets").is_dir() || in_default.join("oh-uni-package.json").is_file())
        {
            if let Some(v) = parse_variant(&in_default, name, true) {
                variants.push(v);
                continue;
            }
        }
        let direct = sdk_root.join(name);
        if let Some(v) = parse_variant(&direct, name, false) {
            variants.push(v);
        }
    }
    let default_api = variants
        .iter()
        .find(|v| v.is_default)
        .and_then(|v| v.api_version.clone())
        .or_else(|| variants.first().and_then(|v| v.api_version.clone()));
    (variants, default_api)
}

/// 在 SDK 根目录下查找 hdc：布局 `sdk/<variant>/toolchains/hdc.exe` 或
/// `sdk/<variant>/<os>/toolchains/hdc.exe`（如 default/openharmony/toolchains/hdc.exe）。
/// DevEco Studio 安装后 hdc 往往不进 PATH、也不在 command-line-tools 里，
/// 只藏在 SDK 的 toolchains 目录，需主动扫描（否则设备面板报找不到程序）。
fn find_hdc_in_sdk(sdk_root: &Path) -> Option<PathBuf> {
    let exe = exe_name("hdc");
    let variants: Vec<PathBuf> = std::fs::read_dir(sdk_root)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();
    // 一层：<variant>/toolchains/hdc.exe
    for v in &variants {
        let one = v.join("toolchains").join(&exe);
        if one.is_file() {
            return Some(one);
        }
    }
    // 两层：<variant>/<os>/toolchains/hdc.exe
    for v in variants {
        if let Ok(entries) = std::fs::read_dir(&v) {
            for e in entries.flatten() {
                if !e.path().is_dir() {
                    continue;
                }
                let two = e.path().join("toolchains").join(&exe);
                if two.is_file() {
                    return Some(two);
                }
            }
        }
    }
    None
}

/// 校验是否为 command-line-tools 目录：含 bin/hdc 或 bin/ohpm
fn looks_like_cli_root(dir: &Path) -> bool {
    let bin = dir.join("bin");
    bin.join(exe_name("hdc")).is_file()
        || bin.join("hdc").is_file()
        || bin.join(exe_name("ohpm")).is_file()
        || bin.join("ohpm").is_file()
        || bin.join("ohpm.bat").is_file()
}

/// 在 PATH 中查找可执行文件
fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_env = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_env) {
        for candidate in [
            dir.join(if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }),
            dir.join(if cfg!(windows) { format!("{name}.cmd") } else { name.to_string() }),
            dir.join(if cfg!(windows) { format!("{name}.bat") } else { name.to_string() }),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// 获取用户主目录（跨平台，避免引入 dirs 依赖）
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// 收集常见候选路径（SDK / command-line-tools）
fn common_sdk_candidates(studio: &Option<PathBuf>) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(s) = studio {
        v.push(s.join("sdk"));
    }
    // 环境变量（跨平台）
    for var in ["HOS_SDK_HOME", "OHOS_SDK_HOME", "DEVECO_SDK_HOME", "HARMONYOS_SDK_HOME"] {
        if let Ok(p) = std::env::var(var) {
            v.push(PathBuf::from(p));
        }
    }
    if let Some(home) = home_dir() {
        if cfg!(target_os = "windows") {
            // Windows：用户明确提到的默认安装位置 + 本地应用数据
            v.push(PathBuf::from(r"C:\Program Files\Huawei\DevEco Studio\sdk"));
            v.push(home.join("AppData").join("Local").join("Huawei").join("Sdk"));
            v.push(home.join("AppData").join("Local").join("OpenHarmony").join("Sdk"));
        } else if cfg!(target_os = "macos") {
            // macOS：DevEco Studio 自带 SDK 与用户级 SDK 目录
            v.push(PathBuf::from("/Applications/DevEco Studio.app/Contents/sdk"));
            v.push(home.join("Library").join("Huawei").join("Sdk"));
            v.push(home.join("Library").join("OpenHarmony").join("Sdk"));
        } else {
            // Linux
            v.push(home.join(".local").join("share").join("Huawei").join("Sdk"));
            v.push(home.join("huawei").join("Sdk"));
            v.push(PathBuf::from("/opt/huawei/sdk"));
            v.push(PathBuf::from("/opt/DevEco Studio/sdk"));
        }
    }
    v
}

fn common_cli_candidates(studio: &Option<PathBuf>) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(s) = studio {
        v.push(s.join("command-line-tools"));
        v.push(s.join("sdk").join("command-line-tools"));
        v.push(s.join("tools").join("command-line-tools"));
    }
    if let Ok(p) = std::env::var("HOS_COMMAND_LINE_HOME") {
        v.push(PathBuf::from(p));
    }
    if let Some(home) = home_dir() {
        if cfg!(target_os = "windows") {
            // 用户明确提到的盘符
            v.push(PathBuf::from(r"H:\command-line-tools"));
            v.push(PathBuf::from(r"C:\command-line-tools"));
            v.push(PathBuf::from(r"D:\command-line-tools"));
            v.push(home.join("command-line-tools"));
            v.push(home.join("AppData").join("Local").join("Huawei").join("command-line-tools"));
        } else if cfg!(target_os = "macos") {
            v.push(home.join("command-line-tools"));
            v.push(home.join("Library").join("Huawei").join("command-line-tools"));
            v.push(PathBuf::from("/Applications/DevEco Studio.app/Contents/command-line-tools"));
            v.push(PathBuf::from("/opt/command-line-tools"));
        } else {
            v.push(home.join("command-line-tools"));
            v.push(PathBuf::from("/opt/command-line-tools"));
        }
    }
    v
}

/// 核心探测：组装完整环境快照。`manual` 为用户覆盖配置。
pub fn detect_with(manual: &HarmonyEnvConfig) -> HarmonyEnv {
    let studio = discover_studio_dir();

    // 候选路径去重并保序
    let dedup = |mut v: Vec<PathBuf>| {
        let mut seen = std::collections::HashSet::new();
        v.retain(|p| seen.insert(p.to_string_lossy().to_lowercase()));
        v
    };

    // SDK 根目录：DEVECO_SDK_HOME（构建实际使用）> 用户手动 > DevEco 内置 SDK > 常见候选
    let mut sdk_root: Option<PathBuf> = std::env::var("DEVECO_SDK_HOME")
        .ok()
        .map(PathBuf::from)
        .filter(|p| looks_like_sdk_root(p));

    if sdk_root.is_none() {
        sdk_root = manual
            .sdk_root
            .as_ref()
            .filter(|p| looks_like_sdk_root(Path::new(p)))
            .map(PathBuf::from);
    }
    // 与构建 hvigor 对齐：未显式指定时优先 DevEco Studio 内置 SDK
    // （hvigor_env 构建时会注入它，环境检测必须与构建实际使用的 SDK 一致）
    if sdk_root.is_none() {
        if let Some(bundled) = discover_deveco_bundled_sdk().filter(|p| looks_like_sdk_root(p)) {
            sdk_root = Some(bundled);
        }
    }

    let mut sdk_suggestions = Vec::new();
    if sdk_root.is_none() {
        for cand in dedup(common_sdk_candidates(&studio)) {
            if looks_like_sdk_root(&cand) {
                sdk_root = Some(cand);
                break;
            }
        }
    }
    // 收集建议路径（存在但不像 SDK，或完全没找到时）
    for cand in dedup(common_sdk_candidates(&studio)) {
        if sdk_root.as_ref() == Some(&cand) {
            continue;
        }
        let display = cand.to_string_lossy().to_string();
        if !sdk_suggestions.contains(&display) {
            sdk_suggestions.push(display);
        }
    }

    // command-line-tools：用户手动 > 软件内置（toolkits/command-line-tools）> 常见候选
    let mut cli_root: Option<PathBuf> = manual
        .cli_root
        .as_ref()
        .filter(|p| looks_like_cli_root(Path::new(p)))
        .map(PathBuf::from);
    if cli_root.is_none() {
        if let Some(bundled) = get_bundled_cli_dir().filter(|p| looks_like_cli_root(p)) {
            cli_root = Some(bundled);
        }
    }
    if cli_root.is_none() {
        for cand in dedup(common_cli_candidates(&studio)) {
            if looks_like_cli_root(&cand) {
                cli_root = Some(cand);
                break;
            }
        }
    }
    let cli_suggestions: Vec<String> = dedup(common_cli_candidates(&studio))
        .into_iter()
        .filter(|c| Some(c) != cli_root.as_ref())
        .map(|c| c.to_string_lossy().to_string())
        .collect();

    // 解析 SDK 变体与版本
    let (sdk_variants, default_api) = match &sdk_root {
        Some(r) => parse_sdk(r),
        None => (Vec::new(), None),
    };
    // 已安装 API 版本号去重列表（取各变体 ets 组件）
    let mut sdk_versions: Vec<String> = sdk_variants
        .iter()
        .filter_map(|v| v.api_version.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    sdk_versions.sort_by(|a, b| b.cmp(a));

    // hdc / ohpm：优先 command-line-tools/bin，再 SDK toolchains，最后 PATH
    let mut hdc_path = None;
    let mut ohpm_path = None;
    let mut hdc_source: Option<&'static str> = None;
    let mut cli_info = None;
    if let Some(cr) = &cli_root {
        let bin = cr.join("bin");
        let hdc = [
            bin.join(exe_name("hdc")),
            bin.join("hdc"),
            bin.join("hdc.exe"),
        ]
        .into_iter()
        .find(|p| p.is_file());
        let ohpm = [
            bin.join(exe_name("ohpm")),
            bin.join("ohpm"),
            bin.join("ohpm.bat"),
            bin.join("ohpm.cmd"),
        ]
        .into_iter()
        .find(|p| p.is_file());
        let hvigorw = [
            bin.join(exe_name("hvigorw")),
            bin.join("hvigorw"),
            bin.join("hvigorw.bat"),
        ]
        .into_iter()
        .find(|p| p.is_file());
        if hdc.is_some() {
            hdc_source = Some("cli");
        }
        hdc_path = hdc.clone();
        ohpm_path = ohpm.clone();
        cli_info = Some(CommandLineTools {
            root: cr.to_string_lossy().to_string(),
            bin: bin.to_string_lossy().to_string(),
            has_hdc: hdc.is_some(),
            has_ohpm: ohpm.is_some(),
            has_hvigorw: hvigorw.is_some(),
        });
    }
    // hdc 不在 command-line-tools 时，扫 SDK toolchains（sdk/<variant>/<os>/toolchains/hdc.exe）
    if hdc_path.is_none() {
        if let Some(sdk) = &sdk_root {
            hdc_path = find_hdc_in_sdk(sdk);
            if hdc_path.is_some() {
                hdc_source = Some("sdk");
            }
        }
    }
    // 兜底：sdk_root 未探测到时，直接扫 DevEco Studio 安装目录的 sdk 子目录
    if hdc_path.is_none() {
        if let Some(s) = &studio {
            hdc_path = find_hdc_in_sdk(&s.join("sdk"));
            if hdc_path.is_some() {
                hdc_source = Some("deveco");
            }
        }
    }
    if hdc_path.is_none() {
        hdc_path = find_in_path("hdc");
        if hdc_path.is_some() {
            hdc_source = Some("path");
        }
    }
    if ohpm_path.is_none() {
        ohpm_path = find_in_path("ohpm");
    }

    let mut suggestions = Vec::new();
    if sdk_root.is_none() {
        suggestions.push("未检测到 HarmonyOS SDK，请手动指定 SDK 根目录（含版本子目录的那一层）".to_string());
        suggestions.extend(sdk_suggestions.iter().take(4).cloned());
    }
    if cli_root.is_none() {
        suggestions.push("未检测到 command-line-tools，请手动指定（含 bin/hdc 的那一层）".to_string());
        suggestions.extend(cli_suggestions.iter().take(4).cloned());
    }

    let source = if manual.sdk_root.is_some() || manual.cli_root.is_some() {
        "manual".to_string()
    } else {
        "auto".to_string()
    };

    HarmonyEnv {
        sdk_root: sdk_root.map(|p| p.to_string_lossy().to_string()),
        default_api,
        sdk_variants,
        sdk_versions,
        cli: cli_info,
        hdc_path: hdc_path.map(|p| p.to_string_lossy().to_string()),
        hdc_source: hdc_source.map(String::from),
        ohpm_path: ohpm_path.map(|p| p.to_string_lossy().to_string()),
        hvigorw_path: None,
        studio_dir: studio.map(|p| p.to_string_lossy().to_string()),
        source,
        suggestions,
    }
}

/// 读取持久化配置后探测
pub fn detect(state: &DbState) -> HarmonyEnv {
    if let Ok(cache) = CACHE.lock() {
        if let Some(env) = cache.as_ref() {
            return env.clone();
        }
    }
    let cfg = load_config(state);
    let env = detect_with(&cfg);
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some(env.clone());
    }
    env
}

/// 不依赖数据库的探测（用于设置页打开时即时展示自动发现结果）
pub fn detect_auto() -> HarmonyEnv {
    detect_with(&HarmonyEnvConfig::default())
}

/// 获取应注入子进程 PATH 的目录列表（command-line-tools/bin 等），
/// 供 utils/process 在启动 hdc/ohpm/hvigor 时优先使用。
pub fn path_dirs(env: &HarmonyEnv) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(cli) = &env.cli {
        dirs.push(PathBuf::from(&cli.bin));
    }
    if let Some(hdc) = &env.hdc_path {
        if let Some(parent) = Path::new(hdc).parent() {
            dirs.push(parent.to_path_buf());
        }
    }
    dirs
}

/// Tauri 命令：获取当前环境（读持久化配置 + 自动探测）
#[tauri::command]
pub fn get_harmony_env(state: tauri::State<'_, DbState>) -> Result<HarmonyEnv, String> {
    Ok(detect(&state))
}

/// Tauri 命令：仅自动探测（忽略手动配置），用于设置页展示"自动发现结果"
#[tauri::command]
pub fn detect_harmony_env() -> Result<HarmonyEnv, String> {
    Ok(detect_auto())
}

/// Tauri 命令：校验用户给定路径并保存配置；返回最新环境快照
#[tauri::command]
pub fn save_harmony_env(
    state: tauri::State<'_, DbState>,
    sdk_root: Option<String>,
    cli_root: Option<String>,
) -> Result<HarmonyEnv, String> {
    let cfg = HarmonyEnvConfig {
        sdk_root: sdk_root.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        cli_root: cli_root.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
    };
    // 校验：若用户填了路径，必须存在
    if let Some(s) = &cfg.sdk_root {
        if !Path::new(s).is_dir() {
            return Err(format!("SDK 路径不存在或不是目录：{s}"));
        }
    }
    if let Some(c) = &cfg.cli_root {
        if !Path::new(c).is_dir() {
            return Err(format!("command-line-tools 路径不存在或不是目录：{c}"));
        }
    }
    save_config(&state, &cfg)?;
    let env = detect(&state);
    // 同步更新子进程 PATH 注入目录
    crate::utils::process::set_harmony_path_dirs(path_dirs(&env));
    // SDK 路径变化，失效 API 索引缓存
    crate::services::sdk_api::invalidate();
    Ok(env)
}

// ---------- SDK API 检索与工程版本对齐 ----------

use crate::services::sdk_api::{self, ApiIndex, ApiModule};

/// 取默认 ets/api 目录（优先 default 变体的 ets 组件）
pub fn default_api_dir(env: &HarmonyEnv) -> Option<String> {
    env.sdk_variants
        .iter()
        .find(|v| v.is_default)
        .or_else(|| env.sdk_variants.first())
        .and_then(|v| v.components.iter().find(|c| c.name == "ets"))
        .and_then(|c| c.api_dir.clone())
}

/// Tauri 命令：列出 SDK API 模块（可按 kit 过滤），供前端浏览
#[tauri::command]
pub fn list_sdk_api_modules(
    state: tauri::State<'_, DbState>,
    kit: Option<String>,
) -> Result<ApiIndex, String> {
    let env = detect(&state);
    let dir = default_api_dir(&env).ok_or_else(|| "未找到 SDK 的 ets/api 目录，请检查 SDK 配置".to_string())?;
    let mut idx = sdk_api::index_api_dir(&dir);
    if let Some(k) = kit {
        let k_lower = k.to_lowercase();
        idx.modules.retain(|m| m.kit.as_deref().map(|x| x.to_lowercase() == k_lower).unwrap_or(false));
    }
    Ok(idx)
}

/// Tauri 命令：按关键字检索 SDK API 模块
#[tauri::command]
pub fn search_sdk_api(
    state: tauri::State<'_, DbState>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<ApiModule>, String> {
    let env = detect(&state);
    let dir = default_api_dir(&env).ok_or_else(|| "未找到 SDK 的 ets/api 目录".to_string())?;
    let idx = sdk_api::index_api_dir(&dir);
    let hits = sdk_api::search(&idx, &query, limit.unwrap_or(30));
    Ok(hits.into_iter().cloned().collect())
}

/// Tauri 命令：读取指定 API 模块的完整声明内容（模型需要精确签名时调用）
#[tauri::command]
pub fn read_sdk_api_module(
    state: tauri::State<'_, DbState>,
    module: String,
) -> Result<String, String> {
    let env = detect(&state);
    let dir = default_api_dir(&env).ok_or_else(|| "未找到 SDK 的 ets/api 目录".to_string())?;
    // 防目录穿越：只取文件名部分，并从本机索引解析真实路径（兼容嵌套声明目录）
    let mut fname = Path::new(&module)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or(module);
    // 模块名不带后缀：补 .d.ts（与 agent 侧读取口径一致，如 @ohos.abilityAccessCtrl → @ohos.abilityAccessCtrl.d.ts）
    if !fname.ends_with(".d.ts") {
        fname.push_str(".d.ts");
    }
    let module_name = fname.trim_end_matches(".d.ts");
    let idx = sdk_api::index_api_dir(&dir);
    let path = idx.modules.iter().find(|item| item.module == module_name).map(|item| PathBuf::from(&item.path));
    let Some(path) = path.filter(|path| path.is_file()) else {
        return Err(format!("未找到声明文件：{fname}"));
    };
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

/// 工程 SDK 版本对齐结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSdkAlignment {
    /// 工程声明的 compatibleSdkVersion（API level，字符串可能是 "5.0.0(12)" 或 "12"）
    pub project_compatible: Option<String>,
    /// 解析出的数字 API level
    pub project_api: Option<i64>,
    /// SDK 已安装的默认 API level
    pub installed_api: Option<String>,
    /// 对齐状态：ok（匹配）/ behind（工程要求高于已装）/ ahead（工程要求低于已装，通常仍兼容）/ unknown
    pub status: String,
    /// 人类可读的提示
    pub message: String,
}

/// Tauri 命令：检查工程的 compatibleSdkVersion 与已装 SDK 是否对齐
#[tauri::command]
pub fn check_project_sdk_alignment(
    state: tauri::State<'_, DbState>,
    project_path: String,
) -> Result<ProjectSdkAlignment, String> {
    project_sdk_alignment(&project_path, &state)
}

/// 检查工程的 compatibleSdkVersion 与已装 SDK 是否对齐（前端命令与 Agent 工具共用）。
pub fn project_sdk_alignment(project_path: &str, db: &DbState) -> Result<ProjectSdkAlignment, String> {
    let root = PathBuf::from(project_path);
    let build_profile = root.join("build-profile.json5");
    let mut project_compatible: Option<String> = None;
    let mut project_api: Option<i64> = None;
    if let Ok(text) = std::fs::read_to_string(&build_profile) {
        // 不引入 json5 依赖：在去掉注释的文本中查找 compatibleSdkVersion 字段
        let stripped = strip_jsonc(&text);
        if let Some((raw, num)) = extract_compatible(&stripped) {
            project_compatible = Some(raw);
            project_api = num;
        }
    }

    let env = detect(db);
    let installed_api = env.default_api.clone();
    let installed_num: Option<i64> = installed_api.as_deref().and_then(|s| s.parse().ok());

    let (status, message) = match (project_api, installed_num) {
        (Some(req), Some(have)) => {
            if req == have {
                ("ok".to_string(), format!("工程要求 API {req}，与已安装 SDK 完全匹配"))
            } else if req > have {
                (
                    "behind".to_string(),
                    format!("工程要求 API {req}，但已安装 SDK 为 API {have}，可能导致编译失败，请安装对应版本 SDK"),
                )
            } else {
                (
                    "ahead".to_string(),
                    format!("工程要求 API {req}，已安装 SDK 为 API {have}（更高版本，通常向下兼容）"),
                )
            }
        }
        (Some(req), None) => (
            "unknown".to_string(),
            format!("工程要求 API {req}，但未检测到已安装 SDK，请配置 SDK 路径"),
        ),
        (None, Some(have)) => (
            "unknown".to_string(),
            format!("未在 build-profile.json5 中解析到 compatibleSdkVersion，已安装 SDK 为 API {have}"),
        ),
        (None, None) => (
            "unknown".to_string(),
            "未能确定工程 SDK 版本与已安装 SDK，请检查工程结构与 SDK 配置".to_string(),
        ),
    };

    Ok(ProjectSdkAlignment {
        project_compatible,
        project_api,
        installed_api,
        status,
        message,
    })
}

/// 从 JSONC 文本中剥离 // 和 /* */ 注释（简易实现，字符串内的注释不处理，配置文件足够）
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '/' && i + 1 < chars.len() {
            if chars[i + 1] == '/' {
                // 行注释
                i += 2;
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            } else if chars[i + 1] == '*' {
                // 块注释
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// 从去注释后的文本中提取 compatibleSdkVersion 的原始字符串值与数字 API level。
/// 支持 "5.0.0(12)"、"12"、纯数字等形式。
fn extract_compatible(text: &str) -> Option<(String, Option<i64>)> {
    let key = "\"compatibleSdkVersion\"";
    let idx = text.find(key)?;
    let rest = &text[idx + key.len()..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    // 字符串形式
    if let Some(inner) = after.strip_prefix('"') {
        let end = inner.find('"')?;
        let raw = inner[..end].trim().to_string();
        let num = parse_compatible_version(&raw);
        Some((raw, num))
    } else {
        // 数字形式
        let num_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        let n: i64 = num_str.parse().ok()?;
        Some((n.to_string(), Some(n)))
    }
}

/// 从 "5.0.0(12)" 或 "12" 提取 API level 数字
fn parse_compatible_version(s: &str) -> Option<i64> {
    if let Some(start) = s.find('(') {
        if let Some(end) = s.find(')') {
            return s[start + 1..end].trim().parse().ok();
        }
    }
    s.trim().parse().ok()
}

// ---------- OpenHarmony 文档本地镜像（替代需登录的华为文档站） ----------

/// 文档库状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarmonyDocsStatus {
    /// 是否已下载（存在文档目录）
    pub downloaded: bool,
    /// 已下载文档 .md 数量（0 = 未下载）
    pub doc_count: usize,
    /// 文档库根目录（未下载时为空）
    pub root: String,
}

/// Tauri 命令：查询文档库状态
#[tauri::command]
pub fn get_harmony_docs_status(app: tauri::AppHandle) -> HarmonyDocsStatus {
    let downloaded = crate::services::harmony_docs::docs_root(&app);
    let (root, doc_count, is_dl) = match downloaded {
        Some(r) => {
            let n = crate::services::harmony_docs::count_docs(&r);
            let dl = crate::services::harmony_docs::is_downloaded(&r);
            (
                crate::utils::path::normalize_path(&r.display().to_string()),
                n,
                dl,
            )
        }
        None => (String::new(), 0, false),
    };
    HarmonyDocsStatus {
        downloaded: is_dl,
        doc_count,
        root,
    }
}

/// Tauri 命令：下载/更新 OpenHarmony 文档（sparse-checkout 只拉 API 参考）。
/// 耗时较长，前端应异步调用并展示进度状态。use_proxy=true 时 git 走系统代理。
#[tauri::command]
pub async fn update_harmony_docs(
    app: tauri::AppHandle,
    prefer_gitee: Option<bool>,
    use_proxy: Option<bool>,
) -> Result<HarmonyDocsStatus, String> {
    use tauri::Manager;
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?.join("harmony-docs");
    crate::services::harmony_docs::sync_docs(
        &root,
        prefer_gitee.unwrap_or(true),
        use_proxy.unwrap_or(false),
    )
    .await?;
    Ok(get_harmony_docs_status(app))
}

/// Tauri 命令：检索本地 OpenHarmony 文档（按文件名/标题/正文打分）。
#[tauri::command]
pub fn search_harmony_docs(
    app: tauri::AppHandle,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<crate::services::harmony_docs::DocEntry>, String> {
    let root = crate::services::harmony_docs::docs_root(&app)
        .ok_or_else(|| "文档库未下载，请先在健康检查页点击「下载 OpenHarmony 文档」".to_string())?;
    let idx = crate::services::harmony_docs::index_docs(&root);
    Ok(crate::services::harmony_docs::search(&idx, &query, limit.unwrap_or(20)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("deveco-env-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn find_hdc_in_sdk_two_level_layout() {
        // DevEco 6.x 默认布局：sdk/default/openharmony/toolchains/hdc.exe
        let root = tmp_dir("hdc-2");
        let p = root.join("default").join("openharmony").join("toolchains");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join(exe_name("hdc")), "fake").unwrap();
        assert!(find_hdc_in_sdk(&root).is_some());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn extract_compatible_parses_string_form() {
        // 回归：字符串形式 "6.1.1(24)" 必须解析出 API 24（曾因越位取串解析失败 → check_sdk_alignment 报 unknown）
        let text = r#"{ "app": { "products": [ { "name": "default", "compatibleSdkVersion": "6.1.1(24)" } ] } }"#;
        let (raw, num) = extract_compatible(text).expect("字符串形式应解析成功");
        assert_eq!(raw, "6.1.1(24)");
        assert_eq!(num, Some(24));
    }

    #[test]
    fn extract_compatible_parses_numeric_form() {
        let text = r#"{ "app": { "products": [ { "name": "default", "compatibleSdkVersion": 12 } ] } }"#;
        let (raw, num) = extract_compatible(text).expect("数字形式应解析成功");
        assert_eq!(raw, "12");
        assert_eq!(num, Some(12));
    }

    #[test]
    fn extract_compatible_parses_with_trailing_comma() {
        // 字符串后跟逗号/其他字段（真实 build-profile.json5 常见）不能影响取值
        let text = "\"compatibleSdkVersion\": \"5.0.0(13)\",\n\"runtimeOS\": \"HarmonyOS\"";
        let (raw, num) = extract_compatible(text).expect("带尾逗号应解析成功");
        assert_eq!(raw, "5.0.0(13)");
        assert_eq!(num, Some(13));
    }

    #[test]
    fn extract_compatible_missing_returns_none() {
        assert!(extract_compatible("{ \"products\": [] }").is_none());
    }

    #[test]
    fn find_hdc_in_sdk_one_level_layout() {
        // 一层布局：sdk/api10/toolchains/hdc.exe
        let root = tmp_dir("hdc-1");
        let p = root.join("api10").join("toolchains");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join(exe_name("hdc")), "fake").unwrap();
        assert!(find_hdc_in_sdk(&root).is_some());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn find_hdc_in_sdk_missing_returns_none() {
        let root = tmp_dir("hdc-0");
        std::fs::create_dir_all(root.join("default").join("openharmony")).unwrap();
        assert!(find_hdc_in_sdk(&root).is_none());
        assert!(find_hdc_in_sdk(&root.join("nonexistent")).is_none());
        std::fs::remove_dir_all(&root).ok();
    }
}

/// Tauri 命令：读取本地文档某篇的完整 Markdown 原文（Agent 需要精读时调用）。
#[tauri::command]
pub fn read_harmony_doc(
    app: tauri::AppHandle,
    rel_path: String,
) -> Result<String, String> {
    let root = crate::services::harmony_docs::docs_root(&app)
        .ok_or_else(|| "文档库未下载".to_string())?;
    // 防目录穿越：规范化后必须仍在 root 内
    let path = root.join(&rel_path);
    let canon = path.canonicalize().map_err(|e| format!("文档路径无效: {e}"))?;
    if !canon.starts_with(&root) {
        return Err("禁止读取文档库之外的文件".into());
    }
    let text = std::fs::read_to_string(&canon).map_err(|e| format!("读取失败: {e}"))?;
    // 截断保护上下文（单篇文档通常 <100KB）
    Ok(if text.len() > 150_000 {
        text.chars().take(150_000).collect()
    } else {
        text
    })
}
