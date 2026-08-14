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

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
    /// 首次引入的 API level（文件中出现的最小 @since）
    pub since_min: Option<u32>,
    /// 文件中出现的最大 @since（反映最近更新）
    pub since_max: Option<u32>,
    /// 顶层声明名称列表（namespace/class/interface/enum/function/type）
    pub declarations: Vec<String>,
    /// 是否为废弃模块（文件含 @deprecated）
    pub deprecated: bool,
    /// 声明文件绝对路径
    pub path: String,
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
}

/// 缓存：api_dir → ApiIndex
static CACHE: Mutex<BTreeMap<String, ApiIndex>> = Mutex::new(BTreeMap::new());

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

/// 提取第一个 @syscap 值
fn extract_syscap(content: &str) -> Option<String> {
    for line in content.lines() {
        let t = line.trim();
        if let Some(idx) = t.find("@syscap") {
            let rest = t[idx + 7..].trim();
            // syscap 形如 SystemCapability.Security.AccessToken，取到空白/注释结束
            let v: String = rest
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '*')
                .collect();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
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
        if !clean.is_empty() && matches!(kind, "namespace" | "class" | "interface" | "enum" | "function" | "type" | "const") {
            decls.push(clean);
        }
    }
    decls.sort();
    decls.dedup();
    decls
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
    Some(ApiModule {
        module,
        kit: extract_kit(&content),
        syscap: extract_syscap(&content),
        since_min,
        since_max,
        declarations: extract_declarations(&content),
        deprecated: content.contains("@deprecated"),
        path: path.to_string_lossy().to_string(),
    })
}

/// 扫描 ets/api 目录，构建索引（带缓存）
pub fn index_api_dir(api_dir: &str) -> ApiIndex {
    if let Ok(cache) = CACHE.lock() {
        if let Some(idx) = cache.get(api_dir) {
            return idx.clone();
        }
    }
    let mut modules = Vec::new();
    let dir = PathBuf::from(api_dir);
    if let Ok(entries) = fs::read_dir(&dir) {
        for e in entries.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            if let Some(m) = scan_module(&p) {
                modules.push(m);
            }
        }
    }
    modules.sort_by(|a, b| a.module.cmp(&b.module));

    let mut by_kit: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for m in &modules {
        if let Some(k) = &m.kit {
            by_kit.entry(k.clone()).or_default().push(m.module.clone());
        }
    }
    for v in by_kit.values_mut() {
        v.sort();
    }

    let idx = ApiIndex {
        api_dir: api_dir.to_string(),
        modules,
        by_kit,
    };
    if let Ok(mut cache) = CACHE.lock() {
        cache.insert(api_dir.to_string(), idx.clone());
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
        if m.kit.as_deref().map(|k| k.to_lowercase().contains(&q)).unwrap_or(false) {
            score += 6;
        }
        if m.syscap.as_deref().map(|s| s.to_lowercase().contains(&q)).unwrap_or(false) {
            score += 5;
        }
        for d in &m.declarations {
            if d.to_lowercase().contains(&q) {
                score += 3;
            }
        }
        if score > 0 {
            hits.push((score, m));
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    hits.into_iter().take(limit).map(|(_, m)| m).collect()
}

/// 获取所有已索引的 api_dir（用于前端浏览默认变体）
pub fn all_indexed() -> Vec<String> {
    CACHE.lock().map(|c| c.keys().cloned().collect()).unwrap_or_default()
}
