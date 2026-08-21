//! HarmonyOS SDK API 元数据扫描器。
//!
//! 扫描 SDK ets/api 目录下的 `@ohos.*.d.ts` 声明文件，提取：
//! - 模块名（文件名，如 @ohos.abilityAccessCtrl）
//! - @kit 归属（如 AbilityKit）
//! - @syscap 系统能力
//! - @since 起始 API level（以及文件中出现的最大 since）
//! - 顶层声明（namespace/class/interface/enum/function 名称）
//!
//! 扫描结果在内存中缓存（按 ets/api 路径），供：
//! - 对话系统提示注入"常用模块速查"
//! - `search_sdk_api` 工具按关键字检索 API
//! - 前端 SDK API 浏览器展示

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 单个 SDK API 模块的元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiModule {
    /// 模块名，如 "@ohos.abilityAccessCtrl"
    pub module: String,
    /// 所属 Kit（从 @file @kit 提取），如 "AbilityKit"
    pub kit: Option<String>,
    /// 主要 syscap（取文件中第一个 @syscap）
    pub syscap: Option<String>,
    /// 文件中出现的全部系统能力
    #[serde(default)]
    pub system_capabilities: Vec<String>,
    /// 文件中出现的全部权限
    #[serde(default)]
    pub permissions: Vec<String>,
    /// 首次引入的 API level（文件中出现的最小 @since）
    pub since_min: Option<u32>,
    /// 文件中出现的最大 @since（反映最近更新）
    pub since_max: Option<u32>,
    /// 顶层声明名称列表（namespace/class/interface/enum/function/type）
    pub declarations: Vec<String>,
    /// 带版本/废弃/能力/权限元数据的类型与 API 符号
    #[serde(default)]
    pub symbols: Vec<ApiSymbol>,
    /// 是否为废弃模块（文件含 @deprecated）
    pub deprecated: bool,
    /// 声明文件绝对路径
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiSymbol {
    pub name: String,
    pub kind: String,
    pub since: Option<u32>,
    pub deprecated: bool,
    pub syscap: Option<String>,
    pub permissions: Vec<String>,
    #[serde(default)]
    pub replacement: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectApiContext {
    pub project_path: String,
    pub product: Option<String>,
    pub compile_api: Option<u32>,
    pub compatible_api: Option<u32>,
    pub target_api: Option<u32>,
    pub installed_api: Option<u32>,
}

impl ProjectApiContext {
    pub fn describe(&self) -> String {
        format!(
            "工程 {} | product {} | compile API {} | compatible API {} | target API {} | 本机 SDK API {}",
            if self.project_path.is_empty() { "（未绑定）" } else { &self.project_path },
            self.product.as_deref().unwrap_or("default/unknown"),
            self.compile_api.map(|value| value.to_string()).as_deref().unwrap_or("?"),
            self.compatible_api.map(|value| value.to_string()).as_deref().unwrap_or("?"),
            self.target_api.map(|value| value.to_string()).as_deref().unwrap_or("?"),
            self.installed_api.map(|value| value.to_string()).as_deref().unwrap_or("?"),
        )
    }

    pub fn availability(&self, since: Option<u32>, deprecated: bool) -> &'static str {
        if since
            .zip(self.compile_api.or(self.installed_api))
            .is_some_and(|(since, compile)| since > compile)
        {
            "不可用：高于当前编译 SDK"
        } else if deprecated {
            "可用但已废弃"
        } else if since
            .zip(self.compatible_api)
            .is_some_and(|(since, compatible)| since > compatible)
        {
            "条件可用：需 API Level 运行时守卫"
        } else {
            "可用"
        }
    }
}

pub fn project_api_context(
    root: Option<&Path>,
    requested_product: Option<&str>,
    installed_api: Option<&str>,
) -> ProjectApiContext {
    let project_path = root
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let model = root
        .filter(|path| path.is_dir())
        .map(crate::services::harmony_model::parse);
    let product = model.as_ref().and_then(|model| {
        requested_product
            .and_then(|name| model.products.iter().find(|product| product.name == name))
            .or_else(|| {
                model
                    .products
                    .iter()
                    .find(|product| product.name == "default")
            })
            .or_else(|| model.products.first())
    });
    ProjectApiContext {
        project_path,
        product: product
            .map(|product| product.name.clone())
            .or_else(|| requested_product.map(str::to_string)),
        compile_api: product
            .and_then(|product| product.compile_api_level)
            .and_then(|value| u32::try_from(value).ok()),
        compatible_api: product
            .and_then(|product| product.compatible_api_level)
            .and_then(|value| u32::try_from(value).ok()),
        target_api: product
            .and_then(|product| product.target_api_level)
            .and_then(|value| u32::try_from(value).ok()),
        installed_api: installed_api.and_then(parse_api_level),
    }
}

fn parse_api_level(value: &str) -> Option<u32> {
    value
        .rsplit_once('(')
        .and_then(|(_, suffix)| suffix.trim_end_matches(')').parse().ok())
        .or_else(|| value.trim().parse().ok())
}

/// 一次扫描的完整索引
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiIndex {
    /// ets/api 目录绝对路径
    pub api_dir: String,
    /// 模块列表（按模块名排序）
    pub modules: Vec<ApiModule>,
    /// 按 kit 分组的模块名列表
    pub by_kit: BTreeMap<String, Vec<String>>,
    /// SystemCapability → 模块
    #[serde(default)]
    pub by_syscap: BTreeMap<String, Vec<String>>,
    /// 权限 → 模块
    #[serde(default)]
    pub by_permission: BTreeMap<String, Vec<String>>,
    /// 首次引入 API level → `module::symbol`
    #[serde(default)]
    pub by_since: BTreeMap<u32, Vec<String>>,
    /// 本轮增量扫描统计
    #[serde(default)]
    pub rescanned_modules: usize,
    #[serde(default)]
    pub reused_modules: usize,
    #[serde(default)]
    pub removed_modules: usize,
    #[serde(default)]
    pub indexed_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified_nanos: u128,
}

#[derive(Debug, Clone)]
struct CachedFile {
    stamp: FileStamp,
    module: ApiModule,
}

#[derive(Debug, Clone)]
struct CachedIndex {
    files: BTreeMap<String, CachedFile>,
}

/// 缓存：api_dir → 文件级快照。每次查询只重扫发生变化的声明文件。
static CACHE: Mutex<BTreeMap<String, CachedIndex>> = Mutex::new(BTreeMap::new());

/// 从 d.ts 文件内容中提取 @kit 标注
fn extract_kit(content: &str) -> Option<String> {
    // @kit 通常出现在文件头部注释中，形如 "@kit AbilityKit"
    for line in content.lines().take(40) {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('*') {
            let r = rest.trim();
            if let Some(k) = r.strip_prefix("@kit") {
                let k = k.trim();
                if !k.is_empty() {
                    return Some(k.to_string());
                }
            }
        }
    }
    None
}

/// 提取文件中所有 @since 数值
fn extract_sinces(content: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        if let Some(idx) = t.find("@since") {
            let rest = t[idx + 6..].trim();
            let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = num.parse::<u32>() {
                out.push(n);
            }
        }
    }
    out
}

fn extract_tag_values(content: &str, tag: &str) -> Vec<String> {
    let mut values = BTreeSet::new();
    for line in content.lines() {
        let t = line.trim();
        if let Some(idx) = t.find(tag) {
            let rest = t[idx + tag.len()..].trim();
            let v: String = rest
                .chars()
                .take_while(|c| !c.is_whitespace() && !matches!(*c, '*' | ',' | ';'))
                .collect();
            if !v.is_empty() {
                values.insert(v);
            }
        }
    }
    values.into_iter().collect()
}

fn extract_syscaps(content: &str) -> Vec<String> {
    extract_tag_values(content, "@syscap")
}

fn extract_permissions(content: &str) -> Vec<String> {
    extract_tag_values(content, "@permission")
}

/// 提取顶层声明名称（declare namespace/class/interface/enum/function/type）
fn extract_declarations(content: &str) -> Vec<String> {
    let mut decls = Vec::new();
    for line in content.lines() {
        let t = line.trim_start();
        // 仅匹配顶层 declare（无前导空白以外的缩进问题：d.ts 顶层 declare 通常在行首）
        let rest = if let Some(r) = t.strip_prefix("declare ") {
            r
        } else if let Some(r) = t.strip_prefix("export declare ") {
            r
        } else {
            continue;
        };
        // 提取关键字后的标识符
        let mut iter = rest.split_whitespace();
        let kind = iter.next().unwrap_or("");
        // 跳过 "namespace Foo {" / "class Bar " / "function baz(" / "const X"
        let name = iter
            .next()
            .map(|s| s.trim_start_matches(['{', '(', '<']))
            .unwrap_or("");
        // 清理名字后的标点
        let clean: String = name
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        if !clean.is_empty()
            && matches!(
                kind,
                "namespace" | "class" | "interface" | "enum" | "function" | "type" | "const"
            )
        {
            decls.push(clean);
        }
    }
    decls.sort();
    decls.dedup();
    decls
}

fn declaration_at_line(line: &str) -> Option<(String, String)> {
    let mut rest = line.trim();
    for prefix in ["export default ", "export ", "declare ", "default "] {
        if let Some(value) = rest.strip_prefix(prefix) {
            rest = value.trim_start();
        }
    }
    let mut parts = rest.split_whitespace();
    let kind = parts.next()?;
    if !matches!(
        kind,
        "namespace" | "class" | "interface" | "enum" | "function" | "type" | "const"
    ) {
        return None;
    }
    let raw = parts.next()?;
    let name: String = raw
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(*c, '_' | '$'))
        .collect();
    (!name.is_empty()).then(|| (kind.to_string(), name))
}

fn extract_symbols(content: &str) -> Vec<ApiSymbol> {
    let mut symbols = Vec::new();
    let mut since = None;
    let mut deprecated = false;
    let mut syscap = None;
    let mut permissions = Vec::new();
    let mut replacement = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(index) = trimmed.find("@since") {
            since = trimmed[index + 6..]
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u32>()
                .ok();
        }
        if trimmed.contains("@deprecated") {
            deprecated = true;
        }
        if let Some(value) = extract_tag_values(trimmed, "@syscap").into_iter().next() {
            syscap = Some(value);
        }
        permissions.extend(extract_tag_values(trimmed, "@permission"));
        replacement = extract_tag_values(trimmed, "@useinstead")
            .into_iter()
            .next()
            .or(replacement);
        if let Some((kind, name)) = declaration_at_line(trimmed) {
            permissions.sort();
            permissions.dedup();
            symbols.push(ApiSymbol {
                name,
                kind,
                since,
                deprecated,
                syscap: syscap.clone(),
                permissions: permissions.clone(),
                replacement: replacement.clone(),
            });
            since = None;
            deprecated = false;
            syscap = None;
            permissions.clear();
            replacement = None;
        }
    }
    symbols.sort_by(|a, b| (&a.name, &a.kind).cmp(&(&b.name, &b.kind)));
    symbols.dedup_by(|a, b| a.name == b.name && a.kind == b.kind);
    symbols
}

/// 扫描单个 d.ts 文件
fn scan_module(path: &Path) -> Option<ApiModule> {
    let fname = path.file_name()?.to_string_lossy().to_string();
    // 仅索引 @ohos.* 与 @kit.* 声明
    if !fname.starts_with("@ohos.") && !fname.starts_with("@kit.") {
        return None;
    }
    // 跳过 .d.ets 等非声明（仅 .d.ts）
    if !fname.ends_with(".d.ts") {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    let module = fname.trim_end_matches(".d.ts").to_string();
    let sinces = extract_sinces(&content);
    let since_min = sinces.iter().min().copied();
    let since_max = sinces.iter().max().copied();
    let system_capabilities = extract_syscaps(&content);
    Some(ApiModule {
        module,
        kit: extract_kit(&content),
        syscap: system_capabilities.first().cloned(),
        system_capabilities,
        permissions: extract_permissions(&content),
        since_min,
        since_max,
        declarations: extract_declarations(&content),
        symbols: extract_symbols(&content),
        deprecated: content.contains("@deprecated"),
        path: path.to_string_lossy().to_string(),
    })
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let metadata = fs::metadata(path).ok()?;
    let modified_nanos = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(FileStamp {
        len: metadata.len(),
        modified_nanos,
    })
}

fn collect_declaration_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                dirs.push(path);
            } else if kind.is_file()
                && path.file_name().is_some_and(|name| {
                    let name = name.to_string_lossy();
                    name.ends_with(".d.ts")
                        && (name.starts_with("@ohos.") || name.starts_with("@kit."))
                })
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn push_reverse(map: &mut BTreeMap<String, Vec<String>>, key: &str, value: &str) {
    let values = map.entry(key.to_string()).or_default();
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn build_index(
    api_dir: &str,
    files: &BTreeMap<String, CachedFile>,
    rescanned_modules: usize,
    reused_modules: usize,
    removed_modules: usize,
) -> ApiIndex {
    let mut modules = files
        .values()
        .map(|file| file.module.clone())
        .collect::<Vec<_>>();
    modules.sort_by(|a, b| a.module.cmp(&b.module));
    let mut by_kit = BTreeMap::new();
    let mut by_syscap = BTreeMap::new();
    let mut by_permission = BTreeMap::new();
    let mut by_since = BTreeMap::new();
    for module in &modules {
        if let Some(kit) = &module.kit {
            push_reverse(&mut by_kit, kit, &module.module);
        }
        for capability in &module.system_capabilities {
            push_reverse(&mut by_syscap, capability, &module.module);
        }
        for permission in &module.permissions {
            push_reverse(&mut by_permission, permission, &module.module);
        }
        for symbol in &module.symbols {
            if let Some(level) = symbol.since {
                by_since
                    .entry(level)
                    .or_insert_with(Vec::new)
                    .push(format!("{}::{}", module.module, symbol.name));
            }
        }
    }
    for values in by_kit
        .values_mut()
        .chain(by_syscap.values_mut())
        .chain(by_permission.values_mut())
        .chain(by_since.values_mut())
    {
        values.sort();
        values.dedup();
    }
    ApiIndex {
        api_dir: api_dir.to_string(),
        modules,
        by_kit,
        by_syscap,
        by_permission,
        by_since,
        rescanned_modules,
        reused_modules,
        removed_modules,
        indexed_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_secs())
            .unwrap_or(0),
    }
}

/// 扫描 ets/api 目录，按文件增量更新索引。
pub fn index_api_dir(api_dir: &str) -> ApiIndex {
    let dir = PathBuf::from(api_dir);
    let previous = CACHE
        .lock()
        .ok()
        .and_then(|cache| cache.get(api_dir).cloned());
    let mut files = BTreeMap::new();
    let mut rescanned_modules = 0;
    let mut reused_modules = 0;
    for path in collect_declaration_files(&dir) {
        let Some(stamp) = file_stamp(&path) else {
            continue;
        };
        let key = path.to_string_lossy().to_string();
        if let Some(cached) = previous
            .as_ref()
            .and_then(|index| index.files.get(&key))
            .filter(|cached| cached.stamp == stamp)
        {
            files.insert(key, cached.clone());
            reused_modules += 1;
        } else if let Some(module) = scan_module(&path) {
            files.insert(key, CachedFile { stamp, module });
            rescanned_modules += 1;
        }
    }
    let removed_modules = previous
        .as_ref()
        .map(|index| {
            index
                .files
                .keys()
                .filter(|path| !files.contains_key(*path))
                .count()
        })
        .unwrap_or(0);
    let idx = build_index(
        api_dir,
        &files,
        rescanned_modules,
        reused_modules,
        removed_modules,
    );
    if let Ok(mut cache) = CACHE.lock() {
        cache.insert(api_dir.to_string(), CachedIndex { files });
    }
    idx
}

/// 失效缓存（SDK 路径变化后调用）
pub fn invalidate() {
    if let Ok(mut c) = CACHE.lock() {
        c.clear();
    }
}

/// 在索引中检索模块：匹配模块名、kit、syscap 或声明名包含关键字的模块
pub fn search<'a>(idx: &'a ApiIndex, query: &str, limit: usize) -> Vec<&'a ApiModule> {
    let q = query.to_lowercase();
    let mut hits: Vec<(i32, &ApiModule)> = Vec::new();
    for m in &idx.modules {
        let mut score = 0;
        if m.module.to_lowercase().contains(&q) {
            score += 10;
        }
        if m.kit
            .as_deref()
            .map(|k| k.to_lowercase().contains(&q))
            .unwrap_or(false)
        {
            score += 6;
        }
        if m.system_capabilities
            .iter()
            .any(|value| value.to_lowercase().contains(&q))
        {
            score += 5;
        }
        if m.permissions
            .iter()
            .any(|value| value.to_lowercase().contains(&q))
        {
            score += 5;
        }
        for d in &m.declarations {
            if d.to_lowercase().contains(&q) {
                score += 3;
            }
        }
        for symbol in &m.symbols {
            if symbol.name.to_lowercase().contains(&q) {
                score += 4;
            }
        }
        if score > 0 {
            hits.push((score, m));
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.module.cmp(&b.1.module)));
    hits.into_iter().take(limit).map(|(_, m)| m).collect()
}

/// 获取所有已索引的 api_dir（用于前端浏览默认变体）
pub fn all_indexed() -> Vec<String> {
    CACHE
        .lock()
        .map(|c| c.keys().cloned().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_api_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("harmony-sdk-api-{name}-{}", std::process::id()))
    }

    #[test]
    fn incremental_index_reuses_changes_and_removes_declarations() {
        let root = temp_api_dir("incremental");
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("nested")).unwrap();
        let module = root.join("@ohos.demo.d.ts");
        std::fs::write(
            &module,
            r#"
/** @kit DemoKit
 * @syscap SystemCapability.Demo.Core
 * @permission ohos.permission.DEMO
 * @since 12
 */
export declare interface DemoType {}
"#,
        )
        .unwrap();

        let first = index_api_dir(root.to_str().unwrap());
        assert_eq!(first.rescanned_modules, 1);
        assert_eq!(first.modules[0].permissions, vec!["ohos.permission.DEMO"]);
        assert_eq!(
            first.by_syscap["SystemCapability.Demo.Core"],
            vec!["@ohos.demo"]
        );
        assert_eq!(first.by_since[&12], vec!["@ohos.demo::DemoType"]);

        let unchanged = index_api_dir(root.to_str().unwrap());
        assert_eq!(unchanged.reused_modules, 1);
        assert_eq!(unchanged.rescanned_modules, 0);

        std::fs::write(
            &module,
            r#"
/** @kit DemoKit
 * @syscap SystemCapability.Demo.Extended
 * @permission ohos.permission.DEMO_EXTENDED
 * @since 14
 */
export declare class ExtendedDemoType { value: string }
"#,
        )
        .unwrap();
        let changed = index_api_dir(root.to_str().unwrap());
        assert_eq!(changed.rescanned_modules, 1);
        assert!(changed
            .by_permission
            .contains_key("ohos.permission.DEMO_EXTENDED"));
        assert!(!changed.by_permission.contains_key("ohos.permission.DEMO"));

        std::fs::remove_file(&module).unwrap();
        let removed = index_api_dir(root.to_str().unwrap());
        assert_eq!(removed.removed_modules, 1);
        assert!(removed.modules.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn search_covers_types_permissions_and_capabilities() {
        let module = ApiModule {
            module: "@ohos.demo".into(),
            kit: Some("DemoKit".into()),
            syscap: Some("SystemCapability.Demo.Core".into()),
            system_capabilities: vec!["SystemCapability.Demo.Core".into()],
            permissions: vec!["ohos.permission.DEMO".into()],
            since_min: Some(12),
            since_max: Some(12),
            declarations: vec!["DemoType".into()],
            symbols: vec![ApiSymbol {
                name: "DemoType".into(),
                kind: "interface".into(),
                since: Some(12),
                deprecated: false,
                syscap: Some("SystemCapability.Demo.Core".into()),
                permissions: vec!["ohos.permission.DEMO".into()],
                replacement: None,
            }],
            deprecated: false,
            path: "/sdk/@ohos.demo.d.ts".into(),
        };
        let index = ApiIndex {
            modules: vec![module],
            ..ApiIndex::default()
        };
        assert_eq!(search(&index, "DemoType", 10).len(), 1);
        assert_eq!(search(&index, "ohos.permission.DEMO", 10).len(), 1);
        assert_eq!(search(&index, "SystemCapability.Demo.Core", 10).len(), 1);
    }

    #[test]
    fn project_api_context_marks_unavailable_conditional_and_deprecated() {
        let context = ProjectApiContext {
            compile_api: Some(20),
            compatible_api: Some(12),
            installed_api: Some(20),
            ..ProjectApiContext::default()
        };
        assert_eq!(
            context.availability(Some(21), false),
            "不可用：高于当前编译 SDK"
        );
        assert_eq!(
            context.availability(Some(18), false),
            "条件可用：需 API Level 运行时守卫"
        );
        assert_eq!(context.availability(Some(10), true), "可用但已废弃");
        assert_eq!(context.availability(Some(10), false), "可用");
    }
}
