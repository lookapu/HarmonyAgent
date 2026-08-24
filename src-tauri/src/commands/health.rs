use serde::Serialize;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use tauri::{Manager, State};
use crate::db::{queries, DbState};

#[derive(Debug, Serialize)]
pub struct HealthResult {
    pub provider_id: String,
    pub provider_name: String,
    pub status: String,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

#[tauri::command]
pub async fn check_all_health(db: State<'_, DbState>) -> Result<Vec<HealthResult>, String> {
    let providers = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        queries::list_providers(&conn).map_err(|e| e.to_string())?
    };

    let mut results = Vec::new();

    for provider in providers {
        let start = std::time::Instant::now();

        // 跟随默认模型的代理设置与默认模型 ID（与 test_provider 同口径，避免 health 与测试结论不一致）
        let (use_proxy, model) = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let row = conn
                .query_row(
                    "SELECT model_id, use_proxy FROM models WHERE provider_id = ?1
                     ORDER BY is_default DESC, created_at ASC LIMIT 1",
                    [&provider.id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
                )
                .ok();
            match row {
                Some((m, u)) => (u, Some(m)),
                None => (false, None),
            }
        };

        let client = crate::utils::net::build_client(use_proxy).unwrap_or_else(|_| {
            crate::utils::net::build_client_auto().expect("构建健康检查客户端失败")
        });

        // 密钥可能已迁移到系统凭据管理器（keyring），健康检查前安全读取补全
        let api_key = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            provider
                .api_key
                .clone()
                .or_else(|| crate::services::key_store::load_provider_key(&conn, &provider.id).ok().flatten())
        };

        let base = provider.base_url.trim_end_matches('/');

        // 按协议分派探测请求（anthropic/gemini 端点无 /models 路由，GET /models 会假 404；
        // 与 test_provider 完全一致的最小请求，确保健康页结论与"测试连接"一致）
        let send = match provider.protocol.as_str() {
            "anthropic" => match &model {
                Some(m) => {
                    let mut rb = client
                        .post(format!("{base}/v1/messages"))
                        .json(&serde_json::json!({
                            "model": m,
                            "max_tokens": 1,
                            "messages": [{"role": "user", "content": "hi"}],
                        }));
                    if let Some(ref key) = api_key {
                        rb = rb
                            .header("x-api-key", key)
                            .header("anthropic-version", "2023-06-01");
                    }
                    rb.send().await
                }
                None => {
                    results.push(HealthResult {
                        provider_id: provider.id.clone(),
                        provider_name: provider.name.clone(),
                        status: "down".to_string(),
                        latency_ms: None,
                        error: Some("请先为该 Provider 添加模型，再执行健康检查".to_string()),
                    });
                    continue;
                }
            },
            "gemini" => match &model {
                Some(m) => {
                    let mut rb = client
                        .post(format!("{base}/v1beta/models/{m}:generateContent"))
                        .json(&serde_json::json!({
                            "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
                        }));
                    if let Some(ref key) = api_key {
                        rb = rb.header("x-goog-api-key", key);
                    }
                    rb.send().await
                }
                None => {
                    results.push(HealthResult {
                        provider_id: provider.id.clone(),
                        provider_name: provider.name.clone(),
                        status: "down".to_string(),
                        latency_ms: None,
                        error: Some("请先为该 Provider 添加模型，再执行健康检查".to_string()),
                    });
                    continue;
                }
            },
            _ => {
                let mut req = client.get(format!("{base}/models"));
                if let Some(ref key) = api_key {
                    req = req.header("Authorization", format!("Bearer {key}"));
                }
                req.send().await
            }
        };

        let result = match send {
            Ok(resp) => {
                let elapsed = start.elapsed().as_millis() as u64;
                if resp.status().is_success() || resp.status().as_u16() == 401 {
                    HealthResult {
                        provider_id: provider.id.clone(),
                        provider_name: provider.name.clone(),
                        status: "healthy".to_string(),
                        latency_ms: Some(elapsed),
                        error: None,
                    }
                } else {
                    HealthResult {
                        provider_id: provider.id.clone(),
                        provider_name: provider.name.clone(),
                        status: "degraded".to_string(),
                        latency_ms: Some(elapsed),
                        error: Some(format!("HTTP {}", resp.status())),
                    }
                }
            }
            Err(e) => HealthResult {
                provider_id: provider.id.clone(),
                provider_name: provider.name.clone(),
                status: "down".to_string(),
                latency_ms: None,
                error: Some(e.to_string()),
            },
        };

        results.push(result);
    }

    Ok(results)
}

// ---------- 鸿蒙工具链检查（doctor：hvigorw / hdc / ohpm / 工程结构） ----------

/// 工程结构检查详情（仅 name="project_structure" 的检查项有值；前端据此做 i18n 渲染）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStructure {
    /// 检查结果：single=标准单工程 / workspace=多项目工作区 / invalid=非完整工程
    pub kind: String,
    /// workspace 时的工程名列表（最多 8 个）
    pub projects: Vec<String>,
    /// workspace 工程总数（可能大于 projects.len()，超出部分未列出）
    pub total: usize,
    /// invalid 时缺失的关键文件（如 build-profile.json5）
    pub missing: Vec<String>,
    /// invalid 时目标目录是否存在
    pub dir_exists: bool,
}

#[derive(Debug, Serialize)]
pub struct ToolchainCheck {
    /// 检查项名称（hvigorw / hdc / ohpm / project_structure）
    pub name: String,
    pub found: bool,
    /// 找到的路径或缺失说明
    pub detail: String,
    /// 缺失时的修复建议
    pub suggestion: Option<String>,
    /// 工程结构检查详情（仅 name="project_structure" 时有值）
    pub structure: Option<ProjectStructure>,
}

/// 在 PATH 中查找可执行文件（Windows 自动补 .exe/.bat/.cmd 扩展名）
fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for cand in [
            name.to_string(),
            format!("{name}.exe"),
            format!("{name}.bat"),
            format!("{name}.cmd"),
        ] {
            let p = dir.join(&cand);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// 在用户指定的目录中查找可执行文件（自定义 toolchain 目录，优先于 PATH）。
/// 直接命中或 dir/bin 子目录命中均可（DevEco 组件的可执行文件多在 bin/ 下）。
fn find_in_dirs(name: &str, dirs: &[String]) -> Option<PathBuf> {
    for dir in dirs {
        let d = std::path::Path::new(dir);
        if let Some(p) = find_tool_in_dir(name, d) {
            return Some(p);
        }
    }
    None
}

/// 在单个目录（含 bin/ 子目录）中查找指定工具的可执行文件
fn find_tool_in_dir(name: &str, dir: &std::path::Path) -> Option<PathBuf> {
    if !dir.is_dir() {
        return None;
    }
    for base in [dir.to_path_buf(), dir.join("bin")] {
        if !base.is_dir() {
            continue;
        }
        for cand in [
            name.to_string(),
            format!("{name}.exe"),
            format!("{name}.bat"),
            format!("{name}.cmd"),
        ] {
            let p = base.join(&cand);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// 自动发现 DevEco Studio 安装根目录（Windows 注册表 + 常见路径；macOS /Applications）。
pub fn discover_deveco_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(windows)]
    {
        // 注册表探测：HKCU/HKLM\Software\Huawei\DevEcoStudio -> Path
        for hive in ["HKCU", "HKLM"] {
            let output = std::process::Command::new("reg")
                .args(["query", &format!(r"{hive}\Software\Huawei\DevEcoStudio"), "/v", "Path"])
                .creation_flags(0x0800_0000)
                .output();
            if let Ok(out) = output {
                let text = String::from_utf8_lossy(&out.stdout);
                for line in text.lines() {
                    if let Some(pos) = line.find("REG_SZ") {
                        let path = line[pos + 6..].trim();
                        if !path.is_empty() {
                            dirs.push(PathBuf::from(path));
                        }
                    }
                }
            }
        }
        // 常见安装路径（含 <base>/Huawei 子目录：DevEco Studio 6.x 默认装在
        // %ProgramFiles%\Huawei\DevEco Studio；5.x 装在 %LOCALAPPDATA%\Huawei\DevEcoStudioX.Y）
        for base in [
            std::env::var("ProgramFiles").unwrap_or_default(),
            std::env::var("LOCALAPPDATA").unwrap_or_default(),
            std::env::var("ProgramFiles(x86)").unwrap_or_default(),
        ] {
            if base.is_empty() {
                continue;
            }
            let mut roots = vec![std::path::Path::new(&base).to_path_buf()];
            let huawei = std::path::Path::new(&base).join("Huawei");
            if huawei.is_dir() {
                roots.push(huawei);
            }
            for root in roots {
                if let Ok(entries) = std::fs::read_dir(&root) {
                    for e in entries.flatten() {
                        let n = e.file_name().to_string_lossy().to_lowercase();
                        if n.starts_with("deveco studio")
                            || n.starts_with("deveco-studio")
                            || n.starts_with("devecostudio")
                        {
                            dirs.push(e.path());
                        }
                    }
                }
            }
        }
        // 用户目录下的 SDK 工具链
        if let Ok(home) = std::env::var("USERPROFILE") {
            let sdk = std::path::Path::new(&home).join("AppData/Local/Huawei/Sdk");
            if sdk.is_dir() {
                dirs.push(sdk);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        // 安装包名可能为 DevEco-Studio.app（连字符）或 DevEco Studio.app（空格），两种都探测
        for app_name in ["DevEco-Studio.app", "DevEco Studio.app"] {
            let app = PathBuf::from("/Applications").join(app_name).join("Contents");
            if app.is_dir() {
                dirs.push(app);
            }
        }
    }
    dirs.sort();
    dirs.dedup();
    dirs
}

/// Windows 上 ohpm 独立安装位置（ohpm 官方安装器默认装到 %USERPROFILE%\ohpm\<version>，
/// 该位置不在 DevEco 安装目录与 PATH 内，需单独探测）
fn user_ohpm_dirs() -> Vec<String> {
    #[cfg(windows)]
    let mut out = Vec::new();
    #[cfg(not(windows))]
    let out = Vec::new();
    #[cfg(windows)]
    if let Ok(home) = std::env::var("USERPROFILE") {
        let ohpm_root = std::path::Path::new(&home).join("ohpm");
        if let Ok(entries) = std::fs::read_dir(&ohpm_root) {
            for e in entries.flatten() {
                if e.path().is_dir() {
                    out.push(e.path().to_string_lossy().to_string());
                }
            }
        }
    }
    out
}

/// 在 DevEco 安装目录下递归查找指定工具（hdc 在 sdk/<ver>/toolchains，ohpm/hvigorw 在 tools 下）。
/// 递归深度受限，避免遍历整个安装目录。
fn find_in_deveco(name: &str, deveco_dirs: &[PathBuf]) -> Option<PathBuf> {
    let exts: &[&str] = if cfg!(windows) {
        &[".exe", ".bat", ".cmd", ""]
    } else {
        &["", ".sh"]
    };
    fn walk(dir: &Path, name: &str, exts: &[&str], depth: u32, out: &mut Option<PathBuf>) {
        if depth == 0 || out.is_some() {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut subdirs = Vec::new();
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                // 跳过明显无关目录
                let n = e.file_name().to_string_lossy().to_lowercase();
                if n == "jbr" || n == "plugins" || n == "lib" || n == "license" {
                    continue;
                }
                subdirs.push(p);
            } else if p.file_name().and_then(|s| s.to_str()).is_some_and(|fname| {
                let lower = fname.to_ascii_lowercase();
                exts.iter().any(|ext| {
                    if ext.is_empty() {
                        lower == name
                    } else {
                        lower == format!("{name}{ext}")
                    }
                })
            }) {
                *out = Some(p);
                return;
            }
        }
        for s in subdirs {
            walk(&s, name, exts, depth - 1, out);
            if out.is_some() {
                return;
            }
        }
    }
    for base in deveco_dirs {
        let mut found = None;
        walk(base, name, exts, 6, &mut found);
        if let Some(p) = found {
            return Some(p);
        }
    }
    None
}

/// 鸿蒙工具链健康检查：hvigorw / hdc / ohpm 可用性 + 工程结构完整性。
/// project_id 提供时检查对应工程目录；custom_paths 为 UI 自定义的工具链目录
/// （如 DevEco Studio 安装目录），查找顺序：自定义目录（用户显式选择）>
/// 软件内置 toolkits 目录 > DevEco Studio > PATH > 工程目录。
#[tauri::command]
pub fn check_harmony_toolchain(
    app: tauri::AppHandle,
    db: State<DbState>,
    project_id: Option<String>,
    custom_paths: Option<Vec<String>>,
) -> Result<Vec<ToolchainCheck>, String> {
    check_harmony_toolchain_impl(&app, db.inner(), project_id, custom_paths)
}

/// 内部实现（不带 State 包装）：供 get_environment_info 在 blocking 线程池中调用。
/// 内部有注册表/目录扫描等同步 IO，需避免在 async 上下文中直接调用。
pub(crate) fn check_harmony_toolchain_impl(
    app: &tauri::AppHandle,
    db: &DbState,
    project_id: Option<String>,
    custom_paths: Option<Vec<String>>,
) -> Result<Vec<ToolchainCheck>, String> {
    // 自定义目录：去空白、去引号，仅保留存在的目录（用户显式选择，最高优先级）
    let mut search_dirs: Vec<String> = custom_paths
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty() && std::path::Path::new(s).is_dir())
        .collect();
    // 软件内置工具包目录（app_data_dir/toolkits/<name>，次高优先级）
    if let Ok(data_dir) = app.path().app_data_dir() {
        let tk = data_dir.join("toolkits");
        if tk.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&tk) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        search_dirs.push(entry.path().to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    // 自动发现 DevEco Studio 安装目录（注册表/常见路径），作为自定义目录之外的补充来源
    let deveco_dirs = discover_deveco_dirs();
    // 项目路径（可选）：用于工程结构检查。优先按项目 id 查库；
    // 兼容直接传目录路径的调用方（旧版前端传的是 path），目录存在时直接使用
    let project_path: Option<String> = if let Some(pid) = project_id.filter(|p| !p.is_empty()) {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let by_id = conn
            .query_row(
                "SELECT path FROM projects WHERE id = ?1",
                [&pid],
                |r| Ok(crate::utils::path::normalize_path(&r.get::<_, String>(0)?)),
            )
            .ok();
        drop(conn);
        by_id.or_else(|| {
            let p = crate::utils::path::normalize_path(&pid);
            std::path::Path::new(&p).is_dir().then_some(p)
        })
    } else {
        None
    };

    let mut checks = Vec::new();

    // 1. hvigorw：优先 toolkit/自定义目录，其次 DevEco 安装目录、PATH，最后项目目录内脚本
    let hvigorw = find_in_dirs("hvigorw", &search_dirs)
        .or_else(|| find_in_deveco("hvigorw", &deveco_dirs))
        .or_else(|| find_in_path("hvigorw"))
        .or_else(|| {
            project_path.as_ref().and_then(|p| {
                for name in ["hvigorw.bat", "hvigorw"] {
                    let cand = std::path::Path::new(p).join(name);
                    if cand.is_file() {
                        return Some(cand);
                    }
                }
                None
            })
        });
    let hvigorw_found = hvigorw.is_some();
    checks.push(ToolchainCheck {
        name: "hvigorw".into(),
        found: hvigorw_found,
        detail: hvigorw
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "未找到（自定义目录、PATH 与工程目录均无）".into()),
        suggestion: (!hvigorw_found).then(|| {
            "构建功能不可用：请在下方填写 DevEco Studio 的 hvigor 目录，或安装 DevEco Studio 后将 hvigorw 加入 PATH，或在工程根目录放置 hvigorw.bat".into()
        }),
        structure: None,
    });

    // 2. hdc（调试/部署依赖）：优先自定义目录，其次复用环境探测结果
    //    （与顶部环境/设备面板同源；避免递归扫描误命中历史遗留 SDK 的旧 hdc），最后 PATH
    let env = crate::services::harmony_env::detect(db);
    let hdc = find_in_dirs("hdc", &search_dirs)
        .or_else(|| env.hdc_path.as_ref().map(PathBuf::from))
        .or_else(|| find_in_path("hdc"));
    let hdc_found = hdc.is_some();
    checks.push(ToolchainCheck {
        name: "hdc".into(),
        found: hdc_found,
        detail: hdc
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "未找到（自定义目录与 PATH 均无）".into()),
        suggestion: (!hdc_found).then(|| {
            "设备部署不可用：请在下方填写 DevEco Studio 的 SDK toolchains 目录，或安装 DevEco Studio（含 HDC）后将 hdc 所在目录加入系统 PATH".into()
        }),
        structure: None,
    });

    // 3. ohpm（依赖安装依赖）：优先自定义目录，其次用户独立安装目录，再 DevEco 目录递归查找，最后 PATH
    let ohpm = find_in_dirs("ohpm", &search_dirs)
        .or_else(|| find_in_dirs("ohpm", &user_ohpm_dirs()))
        .or_else(|| find_in_deveco("ohpm", &deveco_dirs))
        .or_else(|| find_in_path("ohpm"));
    let ohpm_found = ohpm.is_some();
    checks.push(ToolchainCheck {
        name: "ohpm".into(),
        found: ohpm_found,
        detail: ohpm
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "未找到（自定义目录与 PATH 均无）".into()),
        suggestion: (!ohpm_found).then(|| {
            "依赖安装不可用：请在下方填写 DevEco Studio 的 ohpm 目录，或安装 DevEco Studio 后将 ohpm 加入系统 PATH".into()
        }),
        structure: None,
    });

    // 4. 工程结构（指定项目时检查是否为完整 Harmony 工程；支持根目录下多项目工作区）。
    //    全局模式无项目可查，不返回该检查项，避免零信息占位条目。
    if let Some(p) = project_path.filter(|p| !p.is_empty()) {
        checks.push(check_project_structure(std::path::Path::new(&p), &p));
    }

    Ok(checks)
}

/// 某工具的一个可用环境目录（供前端“选择环境”下拉展示）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCandidate {
    pub path: String,
    /// 来源：custom=自定义目录 / bundled=软件内置 / deveco=DevEco Studio / path=系统 PATH
    pub source: String,
}

/// 枚举某工具的所有候选环境目录（不按优先级短路），供前端“选择环境”下拉展示。
/// 返回顺序即推荐优先级：自定义 > 软件内置 > DevEco Studio > 系统 PATH。
#[tauri::command]
pub fn get_toolchain_candidates(
    app: tauri::AppHandle,
    name: String,
    custom_paths: Option<Vec<String>>,
) -> Result<Vec<ToolCandidate>, String> {
    let name = name.trim().to_string();
    let mut out: Vec<ToolCandidate> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    fn push_candidate(
        out: &mut Vec<ToolCandidate>,
        seen: &mut std::collections::HashSet<String>,
        path: PathBuf,
        source: &str,
    ) {
        let key = path.to_string_lossy().to_lowercase();
        if seen.insert(key) {
            out.push(ToolCandidate {
                path: path.to_string_lossy().to_string(),
                source: source.to_string(),
            });
        }
    }

    // 自定义目录（用户显式选择）
    for dir in custom_paths.unwrap_or_default() {
        let d = dir.trim().trim_matches('"').to_string();
        let p = std::path::PathBuf::from(&d);
        if p.is_dir() && find_tool_in_dir(&name, &p).is_some() {
            push_candidate(&mut out, &mut seen, p, "custom");
        }
    }
    // 软件内置 toolkits 目录
    if let Ok(data_dir) = app.path().app_data_dir() {
        let tk = data_dir.join("toolkits");
        if tk.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&tk) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() && find_tool_in_dir(&name, &p).is_some() {
                        push_candidate(&mut out, &mut seen, p, "bundled");
                    }
                }
            }
        }
    }
    // DevEco Studio 自动发现（返回工具文件所在目录）
    for base in discover_deveco_dirs() {
        if let Some(p) = find_in_deveco(&name, &[base]) {
            let dir = p.parent().map(|x| x.to_path_buf()).unwrap_or(p);
            push_candidate(&mut out, &mut seen, dir, "deveco");
        }
    }
    // 系统 PATH
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            if find_tool_in_dir(&name, &dir).is_some() {
                push_candidate(&mut out, &mut seen, dir, "path");
            }
        }
    }
    Ok(out)
}

/// 工程结构检查：
/// - 根目录含 build-profile.json5 + oh-package.json5 → 单工程
/// - 根目录下一级子目录中存在多个上述结构 → 多项目工作区（列出各工程名）
/// - 均无 → 报缺失信息并提示可能是工作区布局
fn check_project_structure(root: &std::path::Path, display_path: &str) -> ToolchainCheck {
    let is_harmony_root = |d: &std::path::Path| {
        d.join("build-profile.json5").is_file() && d.join("oh-package.json5").is_file()
    };

    if is_harmony_root(root) {
        return ToolchainCheck {
            name: "project_structure".into(),
            found: true,
            detail: format!(
                "{display_path}\n（标准 HarmonyOS 工程：build-profile.json5 + oh-package.json5 齐全）"
            ),
            suggestion: None,
            structure: Some(ProjectStructure {
                kind: "single".into(),
                projects: vec![],
                total: 0,
                missing: vec![],
                dir_exists: true,
            }),
        };
    }

    // 工作区布局：扫描一级子目录，收集其中的 HarmonyOS 工程
    let mut projects: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let sub = e.path();
            if sub.is_dir() && is_harmony_root(&sub) {
                projects.push(e.file_name().to_string_lossy().to_string());
            }
        }
    }
    // read_dir 顺序由文件系统决定，Windows/macOS 上都不稳定。固定排序可避免
    // 健康检查重复刷新时项目列表跳动，也让前端缓存与测试结果保持确定性。
    projects.sort_unstable();
    if !projects.is_empty() {
        let total = projects.len();
        let shown: Vec<String> = projects.into_iter().take(8).collect();
        return ToolchainCheck {
            name: "project_structure".into(),
            found: true,
            detail: format!(
                "{display_path}\n（多项目工作区：{} 共 {total} 个 HarmonyOS 工程）",
                shown.join("、")
            ),
            suggestion: None,
            structure: Some(ProjectStructure {
                kind: "workspace".into(),
                projects: shown,
                total,
                missing: vec![],
                dir_exists: true,
            }),
        };
    }

    let has_build_profile = root.join("build-profile.json5").is_file();
    let has_oh_package = root.join("oh-package.json5").is_file();
    let missing: Vec<String> = [
        (!has_build_profile).then(|| "build-profile.json5".to_string()),
        (!has_oh_package).then(|| "oh-package.json5".to_string()),
    ]
    .into_iter()
    .flatten()
    .collect();
    let dir_exists = root.is_dir();
    ToolchainCheck {
        name: "project_structure".into(),
        found: false,
        detail: format!(
            "{display_path}\n缺失: {}（{}）",
            missing.join(", "),
            if dir_exists {
                "目录存在，但根目录与一级子目录中均未发现完整工程"
            } else {
                "目录不存在"
            }
        ),
        suggestion: Some(
            "该目录不是标准 HarmonyOS 工程（缺少构建/依赖清单），构建与部署工具将不可用；如为工作区布局请确认子目录内工程文件齐全".into(),
        ),
        structure: Some(ProjectStructure {
            kind: "invalid".into(),
            projects: vec![],
            total: 0,
            missing,
            dir_exists,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ds_health_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn project_structure_single_project() {
        let root = tmp_dir("single");
        std::fs::write(root.join("build-profile.json5"), "{}").unwrap();
        std::fs::write(root.join("oh-package.json5"), "{}").unwrap();
        let c = check_project_structure(&root, &root.to_string_lossy());
        assert_eq!(c.name, "project_structure");
        assert!(c.found);
        let s = c.structure.expect("single 应有结构详情");
        assert_eq!(s.kind, "single");
        assert!(s.projects.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn project_structure_workspace() {
        let root = tmp_dir("ws");
        for p in ["app1", "app2", "app3"] {
            let d = root.join(p);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("build-profile.json5"), "{}").unwrap();
            std::fs::write(d.join("oh-package.json5"), "{}").unwrap();
        }
        let c = check_project_structure(&root, &root.to_string_lossy());
        assert!(c.found);
        let s = c.structure.expect("workspace 应有结构详情");
        assert_eq!(s.kind, "workspace");
        assert_eq!(s.projects, vec!["app1", "app2", "app3"]);
        assert_eq!(s.total, 3);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn project_structure_invalid() {
        let root = tmp_dir("invalid");
        std::fs::write(root.join("README.md"), "hi").unwrap();
        let c = check_project_structure(&root, &root.to_string_lossy());
        assert!(!c.found);
        let s = c.structure.expect("invalid 应有结构详情");
        assert_eq!(s.kind, "invalid");
        assert!(s.missing.contains(&"build-profile.json5".to_string()));
        assert!(s.missing.contains(&"oh-package.json5".to_string()));
        assert!(s.dir_exists);
        assert!(c.suggestion.is_some());
        std::fs::remove_dir_all(&root).ok();
    }
}
