//! 鸿蒙工程智能分析：构建错误结构化解析 + 工程能力盘点（Kit 使用 / 权限 / 依赖 / 模块）。
//!
//! 供前端"构建结果错误列表"与"工程能力分析面板"使用；Agent 侧通过已有工具
//! （build_project / read_module_config / search_harmony_docs）复用同一批解析逻辑。

use serde::Serialize;
use std::path::Path;

/// 单个构建错误（复用 harmony::BuildError 的结构化结果）
pub type AnalyzedBuildError = crate::services::harmony::BuildError;

/// 模块级能力摘要
#[derive(Debug, Clone, Serialize)]
pub struct ModuleCapability {
    /// 相对路径，如 entry / features/xxx
    pub rel_path: String,
    /// 模块类型（module.json5 的 type：entry / feature / har / hsp 等）
    pub kind: String,
    /// 支持设备类型（deviceTypes）
    pub device_types: Vec<String>,
    /// 启动 Ability（mainElement）
    pub main_element: Option<String>,
    /// 使用的 Kit（import from '@kit.xxx'）
    pub kits: Vec<String>,
    /// 权限声明
    pub permissions: Vec<PermissionInfo>,
    /// oh-package 依赖（本模块声明）
    pub deps: Vec<OhpmDep>,
}

/// 权限信息
#[derive(Debug, Clone, Serialize)]
pub struct PermissionInfo {
    pub name: String,
    pub reason: Option<String>,
}

/// oh-package 依赖项
#[derive(Debug, Clone, Serialize)]
pub struct OhpmDep {
    /// 依赖名，如 @ohos/video_processing
    pub name: String,
    /// 声明版本约束，如 ^1.0.0
    pub version: String,
    /// 是否 devDependencies
    pub dev: bool,
    /// 所属模块相对路径（根 = ""）
    pub module: String,
}

/// Kit 使用统计
#[derive(Debug, Clone, Serialize)]
pub struct KitStat {
    pub kit: String,
    /// 使用到该 Kit 的模块数
    pub count: usize,
}

/// 工程能力分析结果
#[derive(Debug, Clone, Serialize)]
pub struct ProjectCapability {
    pub project: crate::services::harmony::HarmonyProject,
    pub modules: Vec<ModuleCapability>,
    /// 聚合 Kit 使用（按出现模块数降序）
    pub kit_usage: Vec<KitStat>,
    /// 聚合权限（去重）
    pub permissions: Vec<PermissionInfo>,
    /// 聚合依赖（去重，含根与各模块）
    pub deps: Vec<OhpmDep>,
    /// 最近一次构建日志中的结构化错误（无日志则空）
    pub build_errors: Vec<AnalyzedBuildError>,
}

/// 解析 oh-package.json5 的 dependencies / devDependencies
fn parse_oh_deps(root: &Path, module_rel: &str) -> Vec<OhpmDep> {
    let p = if module_rel.is_empty() {
        root.join("oh-package.json5")
    } else {
        root.join(module_rel).join("oh-package.json5")
    };
    let mut out = Vec::new();
    let Ok(text) = std::fs::read_to_string(&p) else {
        return out;
    };
    let Ok(v) = crate::services::harmony::parse_json5(&text) else {
        return out;
    };
    let push = |out: &mut Vec<OhpmDep>, obj: Option<&serde_json::Value>, dev: bool, module: String| {
        if let Some(o) = obj.and_then(|x| x.as_object()) {
            for (name, ver) in o {
                let version = ver.as_str().map(String::from).unwrap_or_default();
                out.push(OhpmDep { name: name.clone(), version, dev, module: module.clone() });
            }
        }
    };
    push(&mut out, v.get("dependencies"), false, module_rel.to_string());
    push(&mut out, v.get("devDependencies"), true, module_rel.to_string());
    out
}

/// 从 ArkTS 源码 import 语句中提取 @kit.xxx 使用
fn scan_kits_in_dir(dir: &Path, out: &mut Vec<String>, budget: &mut usize) {
    if *budget == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if *budget == 0 {
            return;
        }
        let p = e.path();
        if p.is_dir() {
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            // 跳过依赖/构建产物/隐藏目录
            if matches!(
                name,
                "oh_modules" | "node_modules" | ".ohpm" | "build" | ".hvigor" | ".git" | ".idea" | "Pods"
            ) {
                continue;
            }
            scan_kits_in_dir(&p, out, budget);
        } else if p.extension().is_some_and(|x| x == "ets" || x == "ts") {
            *budget -= 1;
            if let Ok(text) = std::fs::read_to_string(&p) {
                for line in text.lines().take(60) {
                    let l = line.trim();
                    // import ... from '@kit.XxxKit'
                    if l.starts_with("import ") && l.contains("@kit.") {
                        for seg in l.split(|c: char| c.is_whitespace() || c == '\'' || c == '"') {
                            if seg.starts_with("@kit.") {
                                let kit = seg.trim_end_matches([';', ',']).to_string();
                                if !out.contains(&kit) {
                                    out.push(kit);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 读取模块 module.json5 的能力字段
fn parse_module_json(root: &Path, module_rel: &str) -> (String, Vec<String>, Option<String>, Vec<PermissionInfo>) {
    let p = root.join(module_rel).join("src/main/module.json5");
    let mut kind = String::new();
    let mut device_types = Vec::new();
    let mut main_element = None;
    let mut permissions = Vec::new();
    let Ok(text) = std::fs::read_to_string(&p) else {
        return (kind, device_types, main_element, permissions);
    };
    let Ok(v) = crate::services::harmony::parse_json5(&text) else {
        return (kind, device_types, main_element, permissions);
    };
    if let Some(m) = v.get("module") {
        kind = m.get("type").and_then(|x| x.as_str()).unwrap_or("").to_string();
        device_types = m
            .get("deviceTypes")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        main_element = m.get("mainElement").and_then(|x| x.as_str()).map(String::from);
        if let Some(perms) = m.get("requestPermissions").and_then(|x| x.as_array()) {
            for per in perms {
                let name = per.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if name.is_empty() {
                    continue;
                }
                let reason = per.get("reason").and_then(|x| x.as_str()).map(String::from);
                permissions.push(PermissionInfo { name, reason });
            }
        }
    }
    (kind, device_types, main_element, permissions)
}

/// 取最新构建日志内容（无则空串）
fn latest_build_log(project_path: &str) -> String {
    let log_dir = crate::agent::exec_ctx::log_dir(project_path);
    let Ok(rd) = std::fs::read_dir(&log_dir) else { return String::new() };
    let mut logs: Vec<_> = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("build-"))
        .collect();
    logs.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH));
    logs.last()
        .and_then(|e| std::fs::read_to_string(e.path()).ok())
        .unwrap_or_default()
}

/// Tauri 命令：解析最近一次构建日志的结构化错误列表。
#[tauri::command]
pub fn analyze_build_errors(project_path: String) -> Result<Vec<AnalyzedBuildError>, String> {
    let log = latest_build_log(&project_path);
    if log.trim().is_empty() {
        return Err("暂无构建日志（先执行一次构建，或检查 .deveco-agent/logs 目录）".into());
    }
    Ok(crate::services::harmony::parse_build_errors(&log))
}

/// Tauri 命令：识别非鸿蒙工程（Node/Go/Rust/Python/Java/C/C++/Flutter/.NET 等）并返回概览。
/// 供前端"工程能力分析"面板对混合工作区中的非鸿蒙子工程做通用分析。
#[tauri::command]
pub fn analyze_generic_project(project_path: String) -> Result<String, String> {
    crate::services::generic_project::generic_project_overview(Path::new(&project_path))
}

/// Tauri 命令：盘点工程能力（模块 / Kit 使用 / 权限 / 依赖 / 最近构建错误）。
/// 供前端"工程能力分析"面板使用。
#[tauri::command]
pub fn analyze_harmony_project(project_path: String) -> Result<ProjectCapability, String> {
    let root = Path::new(&project_path);
    if !root.is_dir() {
        return Err(format!("项目目录不存在：{project_path}"));
    }
    let project = crate::services::harmony::parse_project(root);

    // 枚举鸿蒙模块：根下 src/main/module.json5（单模块）或子目录
    let mut module_rels: Vec<String> = Vec::new();
    if root.join("src/main/module.json5").is_file() {
        module_rels.push(String::new()); // 根即模块
    }
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && p.join("src/main/module.json5").is_file() {
                if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                    if !name.starts_with('.') && name != "oh_modules" && name != "node_modules" {
                        module_rels.push(name.to_string());
                    }
                }
            }
        }
    }

    let mut modules = Vec::new();
    let mut kit_usage: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut perm_map: std::collections::HashMap<String, PermissionInfo> = std::collections::HashMap::new();
    let mut dep_map: std::collections::HashMap<(String, String), OhpmDep> = std::collections::HashMap::new();

    // 根级 oh-package（若根不是模块也常声明依赖）
    for d in parse_oh_deps(root, "") {
        dep_map.entry((d.name.clone(), d.version.clone())).or_insert(d);
    }

    for rel in &module_rels {
        let module_root = if rel.is_empty() { root.to_path_buf() } else { root.join(rel) };
        let (kind, device_types, main_element, permissions) = parse_module_json(root, rel);
        let mut kits = Vec::new();
        let mut budget = 600usize;
        scan_kits_in_dir(&module_root.join("src"), &mut kits, &mut budget);
        kits.sort();
        for k in &kits {
            *kit_usage.entry(k.clone()).or_insert(0) += 1;
        }
        for p in &permissions {
            perm_map.entry(p.name.clone()).or_insert_with(|| p.clone());
        }
        for d in parse_oh_deps(root, rel) {
            dep_map.entry((d.name.clone(), d.version.clone())).or_insert(d);
        }
        modules.push(ModuleCapability {
            rel_path: if rel.is_empty() { ".".to_string() } else { rel.clone() },
            kind,
            device_types,
            main_element,
            kits,
            permissions,
            deps: parse_oh_deps(root, rel),
        });
    }

    let mut kit_usage: Vec<KitStat> = kit_usage
        .into_iter()
        .map(|(kit, count)| KitStat { kit, count })
        .collect();
    kit_usage.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.kit.cmp(&b.kit)));

    let mut permissions: Vec<PermissionInfo> = perm_map.into_values().collect();
    permissions.sort_by(|a, b| a.name.cmp(&b.name));

    let mut deps: Vec<OhpmDep> = dep_map.into_values().collect();
    deps.sort_by(|a, b| a.name.cmp(&b.name));

    let build_errors = crate::services::harmony::parse_build_errors(&latest_build_log(&project_path));

    Ok(ProjectCapability {
        project,
        modules,
        kit_usage,
        permissions,
        deps,
        build_errors,
    })
}

/// ohpm 依赖版本核对：声明的版本约束 vs oh_modules 中实际安装的版本
#[derive(Debug, Clone, Serialize)]
pub struct OhpmDepCheck {
    pub name: String,
    /// 声明的版本约束（如 ^1.0.0）
    pub declared: String,
    /// oh_modules 中实际安装版本（未安装为空串）
    pub installed: String,
    pub dev: bool,
    /// 所属模块相对路径（根 = ""）
    pub module: String,
}

/// Tauri 命令：核对 ohpm 依赖的声明版本与实际安装版本。
#[tauri::command]
pub fn check_ohpm_deps(project_path: String) -> Result<Vec<OhpmDepCheck>, String> {
    let root = Path::new(&project_path);
    if !root.is_dir() {
        return Err(format!("项目目录不存在：{project_path}"));
    }
    // 收集全部声明的依赖（根 + 各模块）
    let mut declared = parse_oh_deps(root, "");
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && p.join("src/main/module.json5").is_file() {
                if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                    if !name.starts_with('.') && name != "oh_modules" && name != "node_modules" {
                        declared.extend(parse_oh_deps(root, name));
                    }
                }
            }
        }
    }
    let installed = scan_installed_versions(root);
    Ok(declared
        .into_iter()
        .map(|d| OhpmDepCheck {
            name: d.name.clone(),
            declared: d.version.clone(),
            installed: installed.get(&d.name).cloned().unwrap_or_default(),
            dev: d.dev,
            module: d.module,
        })
        .collect())
}

/// 扫描根目录与各模块 oh_modules 下已安装包的 name → version。
fn scan_installed_versions(root: &Path) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let mut scan_one = |dir: &std::path::PathBuf| {
        let Ok(text) = std::fs::read_to_string(dir.join("package.json")) else { return };
        let Ok(v) = crate::services::harmony::parse_json5(&text) else { return };
        if let (Some(n), Some(ver)) = (
            v.get("name").and_then(|x| x.as_str()),
            v.get("version").and_then(|x| x.as_str()),
        ) {
            out.entry(n.to_string()).or_insert_with(|| ver.to_string());
        }
    };
    // 候选 oh_modules 目录：根 + 各子模块
    let mut om_dirs: Vec<std::path::PathBuf> = vec![root.join("oh_modules")];
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if !name.starts_with('.') && name != "oh_modules" && name != "node_modules" {
                    om_dirs.push(p.join("oh_modules"));
                }
            }
        }
    }
    for om in om_dirs {
        let Ok(rd) = std::fs::read_dir(&om) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.starts_with('@') {
                // scoped 包：@ohos/xxx → @ohos 目录下再一层
                if let Ok(rd2) = std::fs::read_dir(&p) {
                    for e2 in rd2.flatten() {
                        scan_one(&e2.path());
                    }
                }
            } else {
                scan_one(&p);
            }
        }
    }
    out
}

/// Tauri 命令：在工程目录执行 ohpm install（安装/更新依赖），返回过程日志与核对结果。
/// 使用 `--all` 递归安装所有模块（根 + entry 等子模块）的依赖，与 DevEco Studio Sync 行为一致；
/// 不带 --all 时 ohpm 只处理根模块依赖，子模块依赖会被静默忽略（表现为“假成功”）。
#[tauri::command]
pub async fn run_ohpm_install(project_path: String) -> Result<String, String> {
    let root = std::path::PathBuf::from(project_path.trim());
    if !root.is_dir() {
        return Err(format!("项目目录不存在：{}", root.display()));
    }
    let mut cmd = crate::utils::process::command("ohpm", &["install".to_string(), "--all".to_string()])?;
    cmd.current_dir(&root);
    cmd.kill_on_drop(true);
    let out = cmd
        .output()
        .await
        .map_err(|e| format!("启动 ohpm install 失败：{e}"))?;
    let mut log = String::from_utf8_lossy(&out.stdout).to_string();
    log.push_str(&String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        return Err(format!(
            "ohpm install 失败（退出码 {}）：\n{log}",
            out.status.code().unwrap_or(-1)
        ));
    }
    // 安装后核对：无依赖时明示（避免“0s 完成=假成功”的困惑），有依赖时检查 oh_modules 是否就位
    Ok(crate::services::harmony::verify_ohpm_install(&root, &log))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_project(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hanalyze-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("entry/src/main/ets/pages")).unwrap();
        std::fs::create_dir_all(dir.join("AppScope")).unwrap();
        std::fs::create_dir_all(dir.join("entry/src/main")).unwrap();
        // 工程根 app.json5 + build-profile + oh-package
        std::fs::write(
            dir.join("AppScope/app.json5"),
            r#"{"app":{"bundleName":"com.test.app","versionCode":1,"versionName":"1.0.0"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("build-profile.json5"),
            r#"{"app":{"products":[{"compatibleSdkVersion":"5.0.0(12)"}]},"modules":[{"name":"entry","srcPath":"./entry"}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("oh-package.json5"),
            r#"{"dependencies":{"@ohos/video_processing":"^1.0.0"},"devDependencies":{"@ohos/lottie":"^2.0.0"}}"#,
        )
        .unwrap();
        // entry 模块配置
        std::fs::write(
            dir.join("entry/src/main/module.json5"),
            r#"{"module":{"name":"entry","type":"entry","deviceTypes":["phone","tablet"],"mainElement":"EntryAbility","requestPermissions":[{"name":"ohos.permission.CAMERA","reason":"拍照"}]}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("entry/oh-package.json5"),
            r#"{"dependencies":{"@ohos/router":"^1.2.0"}}"#,
        )
        .unwrap();
        // ArkTS 源码：import @kit
        std::fs::write(
            dir.join("entry/src/main/ets/Index.ets"),
            "import { router } from '@kit.ArkUI'\nimport { cameraManager } from '@kit.CameraKit'\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn parses_oh_deps_and_kits() {
        let dir = tmp_project("kits");
        let cap = analyze_harmony_project(dir.display().to_string()).unwrap();
        assert_eq!(cap.project.bundle_name.as_deref(), Some("com.test.app"));
        assert_eq!(cap.project.api_version, Some(12));
        // 模块：仅 entry（根目录无 module.json5）
        assert_eq!(cap.modules.len(), 1);
        let entry = cap.modules.iter().find(|m| m.rel_path == "entry").unwrap();
        assert_eq!(entry.kind, "entry");
        assert_eq!(entry.device_types, vec!["phone".to_string(), "tablet".to_string()]);
        assert!(entry.kits.iter().any(|k| k == "@kit.ArkUI"));
        assert!(entry.kits.iter().any(|k| k == "@kit.CameraKit"));
        assert!(entry.permissions.iter().any(|p| p.name == "ohos.permission.CAMERA"));
        assert!(entry.deps.iter().any(|d| d.name == "@ohos/router"));
        // 聚合权限/依赖/Kit
        assert!(cap.permissions.iter().any(|p| p.name == "ohos.permission.CAMERA"));
        assert!(cap.deps.iter().any(|d| d.name == "@ohos/video_processing" && !d.dev));
        assert!(cap.deps.iter().any(|d| d.name == "@ohos/lottie" && d.dev));
        assert!(cap.kit_usage.iter().any(|k| k.kit == "@kit.ArkUI" && k.count >= 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_errors_empty_without_log() {
        let dir = tmp_project("nolog");
        let cap = analyze_harmony_project(dir.display().to_string()).unwrap();
        // 没有构建日志时 build_errors 应为空（parse 不 panic）
        assert!(cap.build_errors.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
