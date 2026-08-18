//! 工作区模块检测：在一个根目录（项目）下识别各类子工程（Vue/React/Java/Go/Python/
//! Rust/Node/静态 HTML/HarmonyOS 等）。根目录始终作为一个项目入库，这里记录的是其下
//! 的各类型模块，供对话系统提示词与工具联动使用。

use std::fs;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// 支持识别的模块类型。unknown 用于用户手动绑定一个暂无法自动识别的目录。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ModuleKind {
    Harmony,
    Vue,
    React,
    Angular,
    Node,
    Java,
    Kotlin,
    Go,
    Python,
    Rust,
    Dotnet,
    Flutter,
    Android,
    Ios,
    Html,
    Php,
    Ruby,
    Cpp,
    Unknown,
}

impl ModuleKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModuleKind::Harmony => "harmony",
            ModuleKind::Vue => "vue",
            ModuleKind::React => "react",
            ModuleKind::Angular => "angular",
            ModuleKind::Node => "node",
            ModuleKind::Java => "java",
            ModuleKind::Kotlin => "kotlin",
            ModuleKind::Go => "go",
            ModuleKind::Python => "python",
            ModuleKind::Rust => "rust",
            ModuleKind::Dotnet => "dotnet",
            ModuleKind::Flutter => "flutter",
            ModuleKind::Android => "android",
            ModuleKind::Ios => "ios",
            ModuleKind::Html => "html",
            ModuleKind::Php => "php",
            ModuleKind::Ruby => "ruby",
            ModuleKind::Cpp => "cpp",
            ModuleKind::Unknown => "unknown",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ModuleKind::Harmony => "HarmonyOS",
            ModuleKind::Vue => "Vue",
            ModuleKind::React => "React",
            ModuleKind::Angular => "Angular",
            ModuleKind::Node => "Node.js",
            ModuleKind::Java => "Java",
            ModuleKind::Kotlin => "Kotlin",
            ModuleKind::Go => "Go",
            ModuleKind::Python => "Python",
            ModuleKind::Rust => "Rust",
            ModuleKind::Dotnet => ".NET",
            ModuleKind::Flutter => "Flutter",
            ModuleKind::Android => "Android",
            ModuleKind::Ios => "iOS",
            ModuleKind::Html => "静态站点",
            ModuleKind::Php => "PHP",
            ModuleKind::Ruby => "Ruby",
            ModuleKind::Cpp => "C/C++",
            ModuleKind::Unknown => "未分类",
        }
    }

    pub fn all() -> &'static [ModuleKind] {
        &[
            ModuleKind::Harmony,
            ModuleKind::Vue,
            ModuleKind::React,
            ModuleKind::Angular,
            ModuleKind::Node,
            ModuleKind::Java,
            ModuleKind::Kotlin,
            ModuleKind::Go,
            ModuleKind::Python,
            ModuleKind::Rust,
            ModuleKind::Dotnet,
            ModuleKind::Flutter,
            ModuleKind::Android,
            ModuleKind::Ios,
            ModuleKind::Html,
            ModuleKind::Php,
            ModuleKind::Ruby,
            ModuleKind::Cpp,
            ModuleKind::Unknown,
        ]
    }
}

/// 一个工作区模块（子工程）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceModule {
    /// 相对项目根的路径（正斜杠）
    pub rel_path: String,
    /// 模块类型
    pub kind: ModuleKind,
    /// 展示名（目录名或从配置中解析出的名称）
    pub name: String,
    /// 是否用户手动绑定（手动绑定的模块不会被自动扫描覆盖）
    #[serde(default)]
    pub manual: bool,
}

/// 扫描时跳过的目录（依赖/构建/缓存/IDE/版本控制）
const SKIP_DIRS: &[&str] = &[
    "node_modules", "oh_modules", ".git", ".hvigor", ".idea", "build", ".cxx", ".preview",
    ".test", ".ohpm", ".arkui-x", "dist", "coverage", ".venv", "target", "out", ".gradle",
    ".dart_tool", "Pods", ".swiftpm", "DerivedData", "vendor", "__pycache__", ".next", ".nuxt",
    ".turbo", ".cache", ".parcel-cache",
];

/// 扫描最大递归深度：混合工作区中鸿蒙/其它子工程可能位于 3~5 级子目录
/// （如 root/team/apps/mobile/proj），取 8 留足余量；MAX_MODULES 与 SKIP_DIRS 保证性能。
const MAX_DEPTH: u32 = 8;
const MAX_MODULES: usize = 200;

fn has_file(dir: &Path, name: &str) -> bool {
    dir.join(name).is_file()
}

fn has_any_file(dir: &Path, names: &[&str]) -> bool {
    names.iter().any(|n| has_file(dir, n))
}

fn has_dir(dir: &Path, name: &str) -> bool {
    dir.join(name).is_dir()
}

/// 判断目录是否为某种模块的工程根。命中即返回类型，否则 None。
/// 注意顺序：特征更具体的类型放前面，避免被通用类型（如 node）抢先匹配。
pub fn classify(dir: &Path) -> Option<ModuleKind> {
    // HarmonyOS：build-profile.json5 / oh-package.json5 / AppScope/app.json5
    if has_file(dir, "build-profile.json5")
        || has_file(dir, "oh-package.json5")
        || has_file(&dir.join("AppScope"), "app.json5")
    {
        return Some(ModuleKind::Harmony);
    }

    // Flutter：pubspec.yaml 且含 flutter 依赖（简化判断：含 pubspec.yaml + .dart_tool 或 flutter 标记）
    if has_file(dir, "pubspec.yaml") {
        if let Ok(text) = fs::read_to_string(dir.join("pubspec.yaml")) {
            let lower = text.to_lowercase();
            // sdk:flutter 表示 Flutter 工程；纯 Dart 包不含它
            if lower.contains("sdk:flutter") || lower.contains("flutter:") {
                return Some(ModuleKind::Flutter);
            }
        }
    }

    // Android（原生）：settings.gradle + app/build.gradle，或 build.gradle 含 com.android
    if has_any_file(dir, &["settings.gradle", "settings.gradle.kts"])
        && (has_dir(dir, "app") || has_any_file(dir, &["build.gradle", "build.gradle.kts"]))
    {
        return Some(ModuleKind::Android);
    }

    // iOS：*.xcodeproj / *.xcworkspace 目录
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if (n.ends_with(".xcodeproj") || n.ends_with(".xcworkspace")) && e.path().is_dir() {
                return Some(ModuleKind::Ios);
            }
        }
    }

    // Java / Kotlin（含 Maven/Gradle，且未被判定为 Android）
    if has_any_file(dir, &["pom.xml", "build.gradle", "build.gradle.kts"]) {
        if has_dir(dir, "src") {
            // Kotlin 优先（含 .kt 源或 build.gradle 中 kotlin 插件）
            if has_file(dir, "build.gradle.kts") {
                return Some(ModuleKind::Kotlin);
            }
            return Some(ModuleKind::Java);
        }
        return Some(ModuleKind::Java);
    }

    // Go：go.mod
    if has_file(dir, "go.mod") {
        return Some(ModuleKind::Go);
    }

    // Rust：Cargo.toml
    if has_file(dir, "Cargo.toml") {
        return Some(ModuleKind::Rust);
    }

    // Python：pyproject.toml / requirements.txt / setup.py
    if has_any_file(dir, &["pyproject.toml", "requirements.txt", "setup.py", "Pipfile"]) {
        return Some(ModuleKind::Python);
    }

    // .NET：*.sln / *.csproj / *.fsproj
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if n.ends_with(".sln") || n.ends_with(".csproj") || n.ends_with(".fsproj") {
                return Some(ModuleKind::Dotnet);
            }
        }
    }

    // PHP：composer.json
    if has_file(dir, "composer.json") {
        return Some(ModuleKind::Php);
    }

    // Ruby：Gemfile
    if has_file(dir, "Gemfile") {
        return Some(ModuleKind::Ruby);
    }

    // C/C++：CMakeLists.txt / Makefile
    if has_file(dir, "CMakeLists.txt") || has_file(dir, "Makefile") {
        return Some(ModuleKind::Cpp);
    }

    // 前端框架：需要 package.json
    if has_file(dir, "package.json") {
        if let Ok(text) = fs::read_to_string(dir.join("package.json")) {
            let lower = text.to_lowercase();
            // 通过依赖特征识别框架（dependencies/devDependencies 合并判断）
            if lower.contains("\"vue\"") || lower.contains("\"nuxt\"") || lower.contains("\"@vitejs/plugin-vue\"") {
                return Some(ModuleKind::Vue);
            }
            if lower.contains("\"react\"") || lower.contains("\"next\"") || lower.contains("\"@vitejs/plugin-react\"") {
                return Some(ModuleKind::React);
            }
            if lower.contains("\"@angular/core\"") {
                return Some(ModuleKind::Angular);
            }
            return Some(ModuleKind::Node);
        }
        return Some(ModuleKind::Node);
    }

    // 静态 HTML 站点：根目录直接含 index.html
    if has_file(dir, "index.html") {
        return Some(ModuleKind::Html);
    }

    None
}

/// 目录名作为模块展示名
fn dir_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| dir.to_string_lossy().to_string())
}

/// 递归收集模块（命中工程根即剪枝）。manual 为之前已手动绑定的模块（按 rel_path 索引），
/// 这些模块即使自动扫描未命中也会保留，且类型不被覆盖。
fn collect(dir: &Path, depth: u32, root: &Path, manual: &[(String, WorkspaceModule)], out: &mut Vec<WorkspaceModule>) {
    if depth > MAX_DEPTH || out.len() >= MAX_MODULES {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if SKIP_DIRS.contains(&name.as_str()) || name.starts_with('.') {
            continue;
        }
        subdirs.push(p);
    }

    for sub in subdirs {
        if out.len() >= MAX_MODULES {
            return;
        }
        let rel = sub
            .strip_prefix(root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if rel.is_empty() {
            continue;
        }

        // 手动绑定优先：保留用户设置，不再自动分类
        let manual_match = manual.iter().find(|(r, _)| r == &rel).map(|(_, m)| m);

        if let Some(m) = manual_match {
            out.push(m.clone());
        } else if let Some(kind) = classify(&sub) {
            out.push(WorkspaceModule {
                name: dir_name(&sub),
                rel_path: rel.clone(),
                kind,
                manual: false,
            });
        }

        // 不再剪枝：即使当前子目录命中工程根，也继续向下扫描以发现其中嵌套的
        // 子工程（例如根目录是鸿蒙工程、其下 features/* 又各自是独立鸿蒙模块）。
        // SKIP_DIRS 已过滤掉 node_modules/oh_modules/build 等，深度上限保证性能。
        collect(&sub, depth + 1, root, manual, out);
    }
}

/// 扫描项目根下的所有模块。existing 为之前记录的模块（用于保留手动绑定项）。
pub fn scan(root: &Path, existing: Option<&[WorkspaceModule]>) -> Vec<WorkspaceModule> {
    let manual: Vec<(String, WorkspaceModule)> = existing
        .unwrap_or(&[])
        .iter()
        .filter(|m| m.manual)
        .map(|m| (m.rel_path.clone(), m.clone()))
        .collect();
    let mut out = Vec::new();
    collect(root, 0, root, &manual, &mut out);
    // 排序：按相对路径，稳定展示
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    out
}

/// 解析数据库中存储的 workspace_modules JSON
pub fn parse(json: Option<&str>) -> Vec<WorkspaceModule> {
    match json {
        Some(s) if !s.is_empty() => serde_json::from_str(s).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// 序列化模块列表为 JSON 字符串
pub fn stringify(modules: &[WorkspaceModule]) -> String {
    serde_json::to_string(modules).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mkdir(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn write(dir: &Path, name: &str, content: &str) {
        fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn detects_mixed_workspace() {
        let tmp = std::env::temp_dir().join(format!("ws_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let web = mkdir(&tmp, "web-app");
        write(&web, "package.json", r#"{"dependencies":{"vue":"^3.0.0"}}"#);

        let api = mkdir(&tmp, "api-server");
        write(&api, "go.mod", "module example.com/api\n");

        let svc = mkdir(&tmp, "svc");
        write(&svc, "Cargo.toml", "[package]\nname=\"svc\"\n");

        let app = mkdir(&tmp, "mobile-app");
        write(&app, "build-profile.json5", "{}");

        let site = mkdir(&tmp, "docs-site");
        write(&site, "index.html", "<html></html>");

        let mods = scan(&tmp, None);
        let kinds: Vec<&str> = mods.iter().map(|m| m.kind.as_str()).collect();
        assert!(kinds.contains(&"vue"), "expected vue, got {kinds:?}");
        assert!(kinds.contains(&"go"), "expected go, got {kinds:?}");
        assert!(kinds.contains(&"rust"), "expected rust, got {kinds:?}");
        assert!(kinds.contains(&"harmony"), "expected harmony, got {kinds:?}");
        assert!(kinds.contains(&"html"), "expected html, got {kinds:?}");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn manual_modules_preserved() {
        let tmp = std::env::temp_dir().join(format!("ws_manual_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let sub = mkdir(&tmp, "some-dir");
        write(&sub, "README.md", "# hi");

        let manual = vec![WorkspaceModule {
            rel_path: "some-dir".to_string(),
            kind: ModuleKind::Unknown,
            name: "some-dir".to_string(),
            manual: true,
        }];
        let mods = scan(&tmp, Some(&manual));
        assert!(mods.iter().any(|m| m.rel_path == "some-dir" && m.manual));

        let _ = fs::remove_dir_all(&tmp);
    }

    /// 根目录自身是鸿蒙工程，其下又嵌套多个鸿蒙/前端子工程（如 D:\projects\harmony-app）。
    /// 命中工程根不应剪枝，子工程应被全部识别。
    #[test]
    fn detects_nested_modules_under_harmony_root() {
        let tmp = std::env::temp_dir().join(format!("ws_nested_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // 根目录是鸿蒙工程
        write(&tmp, "build-profile.json5", "{}");
        let app_scope = mkdir(&tmp, "AppScope");
        write(&app_scope, "app.json5", "{}");

        // 嵌套的鸿蒙子模块
        let product_a = mkdir(&tmp, "products/productA");
        write(&product_a, "build-profile.json5", "{}");
        let feature_b = mkdir(&tmp, "features/featureB");
        write(&feature_b, "oh-package.json5", "{}");

        // 嵌套的前端子工程
        let web = mkdir(&tmp, "web/admin");
        write(&web, "package.json", r#"{"dependencies":{"vue":"^3.0.0"}}"#);

        let mods = scan(&tmp, None);
        let rels: Vec<&str> = mods.iter().map(|m| m.rel_path.as_str()).collect();
        assert!(rels.contains(&"products/productA"), "expected products/productA, got {rels:?}");
        assert!(rels.contains(&"features/featureB"), "expected features/featureB, got {rels:?}");
        assert!(rels.contains(&"web/admin"), "expected web/admin, got {rels:?}");
        assert!(mods.len() >= 3, "expected >=3 modules, got {rels:?}");

        let _ = fs::remove_dir_all(&tmp);
    }

    /// 深层嵌套识别：鸿蒙工程位于项目根下 4 级子目录（用户场景：项目根是混合工作区）
    #[test]
    fn detects_deep_nested_modules() {
        let tmp = std::env::temp_dir().join(format!("ws_deep_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        // root/team/apps/mobile/harmony-app（深度 4）
        let deep_harmony = mkdir(&tmp, "team/apps/mobile/harmony-app");
        write(&deep_harmony, "build-profile.json5", "{}");
        let deep_go = mkdir(&tmp, "team/services/backend/api");
        write(&deep_go, "go.mod", "module example.com/api\n");
        let mods = scan(&tmp, None);
        let rels: Vec<&str> = mods.iter().map(|m| m.rel_path.as_str()).collect();
        assert!(
            rels.contains(&"team/apps/mobile/harmony-app"),
            "expected deep harmony, got {rels:?}"
        );
        assert!(
            rels.contains(&"team/services/backend/api"),
            "expected deep go, got {rels:?}"
        );
        let _ = fs::remove_dir_all(&tmp);
    }
}
