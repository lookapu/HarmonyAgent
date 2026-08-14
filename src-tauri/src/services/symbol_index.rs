//! 项目符号索引：扫描源码提取类/组件/函数/路由等符号，供 Agent 快速定位，减少盲读。
//!
//! 轻量实现：基于行级正则/关键字匹配，不做完整 AST 解析；覆盖 ArkTS/TS/JS/Rust/Python/Kotlin 等。
//! 结果可持久化到 project_index_cache 表（kind='symbols'），也可即时返回。

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::services::harmony;

const SYMBOL_EXTS: &[&str] = &["ets", "ts", "tsx", "js", "jsx", "rs", "py", "kt", "java", "swift", "go", "cpp", "c", "h", "hpp"];

const SKIP_DIRS: &[&str] = &[
    "node_modules", ".git", "build", ".hvigor", "oh_modules", ".idea", "dist",
    ".cxx", ".preview", ".test", ".ohpm", ".arkui-x", "coverage", ".venv", "target",
];

const MAX_FILES: usize = 4000;
const MAX_BYTES: u64 = 512 * 1024;

/// ArkTS 状态管理装饰器（属性声明/状态流转标记，鸿蒙工程定位数据流的关键符号）
const ETS_STATE_DECORATORS: &[&str] = &[
    "@State", "@Prop", "@Link", "@Provide", "@Consume", "@ObjectLink", "@Observed",
    "@Builder", "@Styles", "@Extend", "@StorageLink", "@StorageProp", "@Watch",
    "@LocalStorageLink", "@LocalStorageProp", "@Require",
];

/// 单个符号定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    /// 符号类型：component / class / interface / function / method / route / struct / enum / decorator
    pub kind: String,
    /// 符号名
    pub name: String,
    /// 相对项目根的文件路径
    pub file: String,
    /// 1-based 行号
    pub line: usize,
    /// 所在类/组件（方法的归属，顶层为空）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

fn safe_rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
        .replace('\\', "/")
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '$'
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// 从一行中取紧跟关键字之后的标识符
fn ident_after(line: &str, kw: &str) -> Option<String> {
    let mut search = line;
    // 处理 export/default/declare 等前缀
    for pre in ["export ", "default ", "declare ", "pub ", "async "] {
        if let Some(rest) = search.strip_prefix(pre) {
            search = rest;
        }
    }
    let rest = search.strip_prefix(kw)?;
    let mut chars = rest.chars().skip_while(|c| c.is_whitespace());
    let first = chars.next()?;
    if !is_ident_start(first) {
        return None;
    }
    let mut name = String::new();
    name.push(first);
    for c in chars {
        if is_ident(c) {
            name.push(c);
        } else {
            break;
        }
    }
    if name.is_empty() { None } else { Some(name) }
}

/// 解析单个源文件中的符号
fn scan_file(path: &Path, rel: &str, out: &mut Vec<Symbol>) {
    let meta = match fs::metadata(path) {
        Ok(m) if m.len() <= MAX_BYTES => m,
        _ => return,
    };
    let _ = meta;
    let content = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return,
    };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let mut current_parent: Option<String> = None;
    let mut brace_depth = 0i32;
    let class_like = ["class ", "interface ", "struct ", "enum ", "object ", "trait ", "impl "];

    for (idx, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('*') || line.starts_with("/*") {
            // 仍需统计大括号（注释里的括号近似忽略，足够符号提取使用）
            continue;
        }
        let lineno = idx + 1;

        // ArkTS/HarmonyOS 装饰器：入口/组件/路由 + 状态管理装饰器
        if ext == "ets" && line.starts_with('@') {
            if line.starts_with("@Entry") {
                out.push(Symbol { kind: "decorator".into(), name: "@Entry".into(), file: rel.into(), line: lineno, parent: None });
            }
            if line.starts_with("@Component") {
                out.push(Symbol { kind: "decorator".into(), name: "@Component".into(), file: rel.into(), line: lineno, parent: None });
            }
            if line.starts_with("@Router") {
                out.push(Symbol { kind: "route".into(), name: "@Router".into(), file: rel.into(), line: lineno, parent: None });
            }
            // 状态管理装饰器：仅当装饰器名后是空白/(/) 等边界符时计入，
            // 避免把 "@StateXxx" 这类普通标识符误报为装饰器
            for dec in ETS_STATE_DECORATORS {
                if let Some(rest) = line.strip_prefix(*dec) {
                    if rest.chars().next().map_or(true, |c| !is_ident(c)) {
                        out.push(Symbol { kind: "decorator".into(), name: (*dec).into(), file: rel.into(), line: lineno, parent: None });
                    }
                    break;
                }
            }
        }

        // 类型定义
        for kw in &class_like {
            if let Some(name) = ident_after(line, kw) {
                let kind = kw.trim();
                out.push(Symbol { kind: kind.into(), name: name.clone(), file: rel.into(), line: lineno, parent: None });
                current_parent = Some(name);
                break;
            }
        }

        // 函数/方法
        let fn_kw = if ext == "py" { "def " } else { "fn " };
        if let Some(name) = ident_after(line, fn_kw) {
            out.push(Symbol { kind: "function".into(), name, file: rel.into(), line: lineno, parent: current_parent.clone() });
        }
        if let Some(name) = ident_after(line, "function ") {
            out.push(Symbol { kind: "function".into(), name, file: rel.into(), line: lineno, parent: current_parent.clone() });
        }
        // ArkTS 组件 struct
        if ext == "ets" {
            if let Some(name) = ident_after(line, "struct ") {
                out.push(Symbol { kind: "component".into(), name, file: rel.into(), line: lineno, parent: None });
            }
            // 方法形似 name(...) {
            if line.contains('(') && line.ends_with('{') {
                let first = line.split('(').next().unwrap_or("").trim();
                let name = first.split_whitespace().last().unwrap_or("");
                if !name.is_empty()
                    && is_ident_start(name.chars().next().unwrap_or(' '))
                    && !["if", "for", "while", "switch", "catch", "when", "return", "else"].contains(&name)
                {
                    out.push(Symbol { kind: "method".into(), name: name.to_string(), file: rel.into(), line: lineno, parent: current_parent.clone() });
                }
            }
        }

        // 简易括号深度，用于离开 class 作用域后清空 parent
        for c in line.chars() {
            if c == '{' {
                brace_depth += 1;
            } else if c == '}' {
                brace_depth -= 1;
                if brace_depth <= 0 {
                    brace_depth = 0;
                    current_parent = None;
                }
            }
        }
    }
}

/// 文件指纹：mtime（Unix 纳秒，NTFS 精度 100ns，可察觉同秒内改写）+ 字节数。
/// 纳秒纪元在 u64 内可表示到 2554 年，截断安全；ext4 等秒级文件系统自动退化为秒+长度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct FileStamp {
    mtime: u64,
    len: u64,
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    Some(FileStamp { mtime, len: meta.len() })
}

/// 收集项目内全部候选源文件及其指纹（mtime 秒 + 字节数）。
/// 只做 read_dir + stat，不做内容解析——增量同步用它定位变化文件。
fn collect_files(dir: &Path, root: &Path, count: &mut usize, files: &mut HashMap<String, FileStamp>) {
    if *count >= MAX_FILES {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        if *count >= MAX_FILES {
            return;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            collect_files(&path, root, count, files);
        } else {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !SYMBOL_EXTS.contains(&ext) {
                continue;
            }
            *count += 1;
            let rel = safe_rel(root, &path);
            if let Some(stamp) = file_stamp(&path) {
                files.insert(rel, stamp);
            }
        }
    }
}

/// 扫描整个项目，返回全部符号（全量构建：无缓存的底层实现）。
/// 强制刷新入口 refresh_project_symbols 已改为 invalidate_cache + cached 组合，此函数暂无调用者。
#[allow(dead_code)]
pub fn index_project(root: &Path) -> Vec<Symbol> {
    let mut files = HashMap::new();
    let mut count = 0usize;
    collect_files(root, root, &mut count, &mut files);
    let mut out = Vec::new();
    for rel in files.keys() {
        let p = root.join(rel);
        scan_file(&p, rel, &mut out);
    }
    out
}

/// 符号索引缓存：key = 规范化后的项目根路径，value = (文件指纹映射, 符号列表, 最近同步秒)。
/// 每次检索 walk + stat 收集当前文件指纹，与缓存对比后只重扫变化文件；
/// 另持久化到磁盘（<app_data>/symbol_cache/），重启后首次打开面板即可命中。

struct CacheEntry {
    files: HashMap<String, FileStamp>,
    syms: Vec<Symbol>,
    /// 最近一次增量同步的秒：冷却期内直接复用内存结果（Agent 修改文件会主动精确失效）
    last_sync: u64,
    /// 数据来源：disk（磁盘恢复）/ scan（本次会话扫描建立），供面板展示缓存状态
    source: &'static str,
}

static SYMBOL_CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();

/// 持久化缓存目录（lib.rs setup 中初始化）；未初始化时退化为纯内存模式（测试场景）
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 由应用启动流程注入符号缓存的持久化目录
pub fn init_cache_dir(dir: PathBuf) {
    let _ = DATA_DIR.set(dir);
}

/// 增量同步冷却（秒）：冷却期内直接返回内存结果，避免高频检索反复 walk；
/// 修改类工具会主动 invalidate_files 立即更新，冷却不会掩盖 Agent 的改动。
const SYNC_COOLDOWN_SECS: u64 = 2;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    SYMBOL_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn canonical_key(root: &Path) -> String {
    root.canonicalize()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| root.to_string_lossy().to_string())
}

// ---------- 磁盘持久化 ----------

/// 磁盘缓存格式：<data_dir>/symbol_cache/<fnv1a(项目路径)>.json
#[derive(Debug, Serialize, Deserialize)]
struct PersistedIndex {
    version: u32,
    files: HashMap<String, FileStamp>,
    syms: Vec<Symbol>,
}

const PERSIST_VERSION: u32 = 1;

/// FNV-1a 64 位：把项目根路径稳定散列为缓存文件名
fn stable_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn cache_file_at(dir: &Path, root: &Path) -> PathBuf {
    let key = canonical_key(root);
    dir.join(format!("{:016x}.json", stable_hash(&key)))
}

fn cache_file_for(root: &Path) -> Option<PathBuf> {
    let dir = DATA_DIR.get()?;
    Some(cache_file_at(dir, root))
}

fn load_from(dir: &Path, root: &Path) -> Option<PersistedIndex> {
    let text = fs::read_to_string(cache_file_at(dir, root)).ok()?;
    let idx: PersistedIndex = serde_json::from_str(&text).ok()?;
    if idx.version != PERSIST_VERSION {
        return None;
    }
    Some(idx)
}

/// 原子写盘（tmp + rename），失败静默——缓存只是加速手段，不影响正确性
fn save_to(dir: &Path, root: &Path, files: &HashMap<String, FileStamp>, syms: &[Symbol]) {
    let path = cache_file_at(dir, root);
    let Some(parent) = path.parent() else { return };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let idx = PersistedIndex { version: PERSIST_VERSION, files: files.clone(), syms: syms.to_vec() };
    let Ok(json) = serde_json::to_string(&idx) else { return };
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, json).is_err() {
        return;
    }
    let _ = fs::rename(&tmp, &path);
}

fn load_persisted(root: &Path) -> Option<PersistedIndex> {
    let dir = DATA_DIR.get()?;
    load_from(dir, root)
}

fn save_persisted(root: &Path, files: &HashMap<String, FileStamp>, syms: &[Symbol]) {
    if let Some(dir) = DATA_DIR.get() {
        save_to(dir, root, files, syms);
    }
}

// ---------- 增量同步 ----------

/// 对缓存状态做增量同步：walk 收集当前文件指纹，与缓存对比，
/// 仅重新解析新增/变化的文件；被删除文件的符号直接剔除。
/// 只操作传入的 files/syms（无锁），供 index_project_cached 锁外阶段调用。
/// 返回 (重新解析的文件数, 移除符号的文件数)；无变化时为 (0, 0)。
fn sync_incremental(
    files: &mut HashMap<String, FileStamp>,
    syms: &mut Vec<Symbol>,
    root: &Path,
) -> (usize, usize) {
    let mut current: HashMap<String, FileStamp> = HashMap::new();
    let mut count = 0usize;
    collect_files(root, root, &mut count, &mut current);

    let mut rescanned = 0usize;
    let mut removed = 0usize;

    // 删除的文件：缓存里有、当前没有
    let gone: Vec<String> = files
        .keys()
        .filter(|rel| !current.contains_key(*rel))
        .cloned()
        .collect();
    for rel in gone {
        files.remove(&rel);
        let before = syms.len();
        syms.retain(|s| s.file != rel);
        removed += before - syms.len();
    }

    // 新增/变化的文件：指纹不同才重扫
    let changed: Vec<String> = current
        .iter()
        .filter(|(rel, stamp)| files.get(*rel) != Some(*stamp))
        .map(|(rel, _)| rel.clone())
        .collect();
    for rel in changed {
        if let Some(stamp) = current.get(&rel) {
            files.insert(rel.clone(), *stamp);
        }
        syms.retain(|s| s.file != rel);
        let mut fresh = Vec::new();
        scan_file(&root.join(&rel), &rel, &mut fresh);
        syms.extend(fresh);
        rescanned += 1;
    }
    (rescanned, removed)
}

/// 带缓存的符号索引：内存 → 磁盘 → 增量同步。
/// 每次调用 walk + stat 收集文件指纹（廉价），仅解析变化文件；
/// 磁盘缓存使重启后首次打开面板即可命中（只校正变化部分）。
///
/// 三段式：锁内取快照 → 锁外扫描（最耗时部分）→ 锁内 CAS 写回。
/// 扫描不在锁内进行，多项目并行检索（search_symbols_all）互不阻塞。
pub fn index_project_cached(root: &Path) -> Vec<Symbol> {
    let key = canonical_key(root);
    let now = now_secs();
    // 阶段 1：锁内取快照（克隆 files/syms + 记录 last_sync），冷却期内直接复用。
    let (mut files, mut syms, snap_sync) = {
        let mut guard = cache().lock().unwrap();
        let entry = guard.entry(key.clone()).or_insert_with(|| CacheEntry {
            files: HashMap::new(),
            syms: Vec::new(),
            last_sync: 0,
            source: "scan",
        });
        // 条目为空时尝试从磁盘恢复，避免重启后全量重扫
        if entry.files.is_empty() && entry.syms.is_empty() {
            if let Some(persisted) = load_persisted(root) {
                entry.files = persisted.files;
                entry.syms = persisted.syms;
                entry.source = "disk";
            }
        }
        // 冷却期内直接复用（空项目除外：可能刚建了新文件）
        if now.saturating_sub(entry.last_sync) < SYNC_COOLDOWN_SECS && !entry.syms.is_empty() {
            return entry.syms.clone();
        }
        (entry.files.clone(), entry.syms.clone(), entry.last_sync)
    };
    // 阶段 2（无锁）：walk + 指纹对比 + 只重扫变化文件
    let (rescanned, removed) = sync_incremental(&mut files, &mut syms, root);
    // 阶段 3（锁内）：CAS 写回——期间有其他线程同步过（last_sync 变化）则丢弃本地结果。
    // invalidate_files 精确更新同样会推进 last_sync，不会被本阶段覆盖丢失。
    let mut guard = cache().lock().unwrap();
    let out = if let Some(entry) = guard.get_mut(&key) {
        if entry.last_sync == snap_sync {
            entry.files = files;
            entry.syms = syms;
            entry.last_sync = now;
            if rescanned > 0 || removed > 0 {
                save_persisted(root, &entry.files, &entry.syms);
            }
        }
        entry.syms.clone()
    } else {
        // 条目被并发清空（容量上限）：返回本地计算结果即可
        syms
    };
    // 简单容量上限：超过 16 个项目时清空内存（磁盘缓存不受影响）
    if guard.len() > 16 {
        guard.clear();
    }
    out
}

/// 全盘失效：清内存条目并删除磁盘缓存（手动刷新/强制重建时调用）。
pub fn invalidate_cache(root: &Path) {
    let key = canonical_key(root);
    if let Ok(mut guard) = cache().lock() {
        guard.remove(&key);
    }
    if let Some(path) = cache_file_for(root) {
        let _ = fs::remove_file(path);
    }
}

/// 增量失效：仅更新指定文件（写/改/删）的符号，其余文件复用缓存。
/// rel 为工具参数中的路径（相对项目根或绝对路径）；目录路径会剔除其下全部文件符号。
/// 内存中无该条目时不做任何事：下次检索会基于最新指纹构建。
pub fn invalidate_files(root: &Path, rels: &[String]) {
    let key = canonical_key(root);
    let mut guard = cache().lock().unwrap();
    let Some(entry) = guard.get_mut(&key) else { return };
    let mut changed = false;
    for rel in rels {
        let abs = if Path::new(rel).is_absolute() {
            PathBuf::from(rel)
        } else {
            root.join(rel)
        };
        // 跨根防护：绝对路径（write_file/edit_file 允许）不属于当前根时跳过，
        // 避免 safe_rel 退化把绝对路径 key 写进缓存条目（多路径提示目录场景）。
        // canonicalize 失败（文件已删）视为不属于本根，删除场景由下次同步兜底。
        if Path::new(rel).is_absolute() {
            let in_root = abs
                .canonicalize()
                .ok()
                .zip(root.canonicalize().ok())
                .map_or(false, |(a, r)| a.starts_with(&r));
            if !in_root {
                continue;
            }
        }
        let rel_norm = safe_rel(root, &abs);
        if rel_norm.is_empty() {
            continue;
        }
        // 目录（含已删除目录，按缓存指纹前缀判断）：剔除其下全部文件
        let prefix = format!("{rel_norm}/");
        let dir_like = abs.is_dir() || entry.files.keys().any(|f| f.starts_with(&prefix));
        if dir_like {
            entry.syms.retain(|s| s.file != rel_norm && !s.file.starts_with(&prefix));
            entry.files.retain(|f, _| f != &rel_norm && !f.starts_with(&prefix));
            changed = true;
            continue;
        }
        // 单文件：存在则重扫，不存在则剔除
        match file_stamp(&abs) {
            Some(stamp) => {
                entry.files.insert(rel_norm.clone(), stamp);
                entry.syms.retain(|s| s.file != rel_norm);
                let mut fresh = Vec::new();
                scan_file(&abs, &rel_norm, &mut fresh);
                entry.syms.extend(fresh);
            }
            None => {
                entry.files.remove(&rel_norm);
                entry.syms.retain(|s| s.file != rel_norm);
            }
        }
        changed = true;
    }
    if changed {
        // 推进 last_sync：与 index_project_cached 阶段 3 的 CAS 协调，
        // 防止并发中的锁外扫描写回时覆盖本次精确更新
        entry.last_sync = now_secs();
        save_persisted(root, &entry.files, &entry.syms);
    }
}

/// 路径安全校验：仅项目内相对路径，拒绝越界
#[allow(dead_code)]
fn ensure_inside(root: &Path, rel: &str) -> Result<std::path::PathBuf, String> {
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err("仅支持项目内相对路径".into());
    }
    if p.components().any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_))) {
        return Err("路径越界".into());
    }
    let canonical_root = root.canonicalize().map_err(|e| format!("项目目录不可访问: {e}"))?;
    let target = root
        .join(p)
        .canonicalize()
        .map_err(|e| format!("文件不可读: {e}"))?;
    if !target.starts_with(&canonical_root) {
        return Err("路径越界".into());
    }
    Ok(target)
}

/// 在符号列表中按关键字/类型过滤
pub fn filter_symbols<'a>(syms: &'a [Symbol], query: &str, kind: Option<&str>) -> Vec<&'a Symbol> {
    let q = query.trim().to_lowercase();
    syms.iter()
        .filter(|s| kind.map_or(true, |k| s.kind == k))
        .filter(|s| q.is_empty() || s.name.to_lowercase().contains(&q) || s.file.to_lowercase().contains(&q))
        .take(200)
        .collect()
}

/// 项目级摘要：组件数、页面数、函数数、路由清单（用于 Agent 快速了解工程结构）
#[derive(Debug, Serialize, Default)]
pub struct ProjectOutline {
    pub components: Vec<Symbol>,
    pub pages: Vec<String>,
    pub symbols_count: usize,
}

pub fn build_outline(root: &Path) -> ProjectOutline {
    // 走缓存索引：对话每轮构建概要时命中增量缓存，避免全量重扫
    let syms = index_project_cached(root);
    let components: Vec<Symbol> = syms
        .iter()
        .filter(|s| s.kind == "component" || (s.kind == "decorator" && s.name == "@Entry"))
        .cloned()
        .collect();
    let pages = harmony::collect_routes(root, None);
    ProjectOutline {
        components,
        pages,
        symbols_count: syms.len(),
    }
}

/// 索引元信息：符号/文件数量与数据来源（供面板展示缓存状态）
#[derive(Debug, Serialize)]
pub struct SymbolIndexMeta {
    pub symbols: usize,
    pub files: usize,
    /// 数据来源：disk（磁盘恢复）/ scan（本次会话扫描建立）
    pub source: &'static str,
    /// 最近同步距今秒数（磁盘恢复后未同步时为较大值）
    pub synced_ago_secs: u64,
}

/// 查询索引元信息：内部先确保索引已构建且新鲜（有冷却/增量，不会重复全量扫描）
pub fn index_meta(root: &Path) -> SymbolIndexMeta {
    let syms = index_project_cached(root);
    let key = canonical_key(root);
    let now = now_secs();
    let guard = cache().lock().unwrap();
    match guard.get(&key) {
        Some(e) => SymbolIndexMeta {
            symbols: e.syms.len(),
            files: e.files.len(),
            source: e.source,
            synced_ago_secs: now.saturating_sub(e.last_sync),
        },
        // 条目被容量上限清空：仅能给出符号数（来源视为本次扫描）
        None => SymbolIndexMeta {
            symbols: syms.len(),
            files: 0,
            source: "scan",
            synced_ago_secs: 0,
        },
    }
}

/// 文件级符号数量（供文件树面板徽标展示）
#[derive(Debug, Serialize)]
pub struct SymbolCount {
    pub file: String,
    pub count: usize,
}

pub fn symbol_counts(root: &Path) -> Vec<SymbolCount> {
    let syms = index_project_cached(root);
    let mut map: HashMap<String, usize> = HashMap::new();
    for s in &syms {
        *map.entry(s.file.clone()).or_default() += 1;
    }
    let mut out: Vec<SymbolCount> = map
        .into_iter()
        .map(|(file, count)| SymbolCount { file, count })
        .collect();
    out.sort_by(|a, b| a.file.cmp(&b.file));
    out
}

/// 即时扫描单文件符号（供 Agent 工具/前端单文件大纲使用）
#[allow(dead_code)]
pub fn symbols_of_file(root: &Path, rel: &str) -> Result<Vec<Symbol>, String> {
    let target = ensure_inside(root, rel)?;
    if !target.is_file() {
        return Err("目标不是文件".into());
    }
    let mut out = Vec::new();
    let r = safe_rel(root, &target);
    scan_file(&target, &r, &mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_arkts_component_and_methods() {
        let src = r#"
import { router } from '@kit.ArkUI';

@Entry
@Component
struct Index {
  @State count: number = 0;

  aboutToAppear() {
  }

  build() {
  }
}
"#;
        let dir = std::env::temp_dir().join("deveco-symbol-test");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("Index.ets");
        std::fs::write(&f, src).unwrap();
        let mut out = Vec::new();
        scan_file(&f, "Index.ets", &mut out);
        assert!(out.iter().any(|s| s.kind == "decorator" && s.name == "@Entry"));
        assert!(out.iter().any(|s| s.kind == "component" && s.name == "Index"), "应识别 struct Index: {out:?}");
        assert!(out.iter().any(|s| s.kind == "method" && s.name == "aboutToAppear"));
        assert!(out.iter().any(|s| s.kind == "method" && s.name == "build"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extracts_arkts_state_decorators() {
        let src = r#"
@Entry
@Component
struct Detail {
  @State count: number = 0;
  @Prop title: string = '';
  @Link linked: boolean;
  @Watch('onChange')
  @State watched: number = 1;
  @StateXxx helper: string = '';
  build() {}
}
"#;
        let dir = std::env::temp_dir().join("deveco-symbol-decor");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("Detail.ets");
        std::fs::write(&f, src).unwrap();
        let mut out = Vec::new();
        scan_file(&f, "Detail.ets", &mut out);
        assert!(out.iter().any(|s| s.kind == "decorator" && s.name == "@State"));
        assert!(out.iter().any(|s| s.kind == "decorator" && s.name == "@Prop"));
        assert!(out.iter().any(|s| s.kind == "decorator" && s.name == "@Link"));
        assert!(out.iter().any(|s| s.kind == "decorator" && s.name == "@Watch"));
        assert!(!out.iter().any(|s| s.name == "@StateXxx"), "普通标识符不应误报为装饰器: {out:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extracts_rust_items() {
        let src = "pub struct Foo;\nfn bar() {}\npub async fn baz() {}";
        let dir = std::env::temp_dir().join("deveco-symbol-rs");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.rs");
        std::fs::write(&f, src).unwrap();
        let mut out = Vec::new();
        scan_file(&f, "a.rs", &mut out);
        assert!(out.iter().any(|s| s.kind == "struct" && s.name == "Foo"));
        assert!(out.iter().any(|s| s.kind == "function" && s.name == "bar"));
        assert!(out.iter().any(|s| s.kind == "function" && s.name == "baz"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn filter_works() {
        let syms = vec![
            Symbol { kind: "function".into(), name: "loadData".into(), file: "a.ts".into(), line: 1, parent: None },
            Symbol { kind: "component".into(), name: "BookCard".into(), file: "b.ets".into(), line: 2, parent: None },
        ];
        assert_eq!(filter_symbols(&syms, "book", None).len(), 1);
        assert_eq!(filter_symbols(&syms, "", Some("component")).len(), 1);
        assert_eq!(filter_symbols(&syms, "", None).len(), 2);
    }

    /// 辅助：建一个含两个 ets 文件的临时项目目录
    fn make_project(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("deveco-symbol-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.ets"), "struct Aaa {}\nfn oldA() {}").unwrap();
        std::fs::write(dir.join("b.ets"), "struct Bbb {}").unwrap();
        dir
    }

    #[test]
    fn sync_incremental_rescans_only_changed() {
        let dir = make_project("incr");
        let mut entry = CacheEntry { files: HashMap::new(), syms: Vec::new(), last_sync: 0, source: "scan" };
        // 首次同步：两个文件都是新增
        let (r1, _) = sync_incremental(&mut entry.files, &mut entry.syms, &dir);
        assert_eq!(r1, 2);
        assert!(entry.syms.iter().any(|s| s.name == "Aaa"));
        assert!(entry.syms.iter().any(|s| s.name == "oldA"));
        assert!(entry.syms.iter().any(|s| s.name == "Bbb"));
        // 只改 a.ets（长度变化 → 指纹变化）
        std::fs::write(dir.join("a.ets"), "struct Aaa {}\nfn oldA() {}\nfn newA() {}").unwrap();
        let (r2, _) = sync_incremental(&mut entry.files, &mut entry.syms, &dir);
        assert_eq!(r2, 1, "只有 a.ets 应被重扫");
        assert!(entry.syms.iter().any(|s| s.name == "newA"), "变化文件的新符号应出现");
        assert!(entry.syms.iter().any(|s| s.name == "Bbb"), "未变文件符号应保留");
        assert_eq!(entry.syms.iter().filter(|s| s.name == "oldA").count(), 1, "旧符号不应重复");
        // 删除 b.ets
        std::fs::remove_file(dir.join("b.ets")).unwrap();
        let (_, removed) = sync_incremental(&mut entry.files, &mut entry.syms, &dir);
        assert!(removed > 0);
        assert!(!entry.syms.iter().any(|s| s.name == "Bbb"), "被删文件符号应移除");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalidate_files_updates_only_target() {
        let dir = make_project("invf");
        // 构建内存条目（DATA_DIR 未初始化 → 纯内存）
        let syms = index_project_cached(&dir);
        assert!(syms.iter().any(|s| s.name == "Aaa"));
        assert!(syms.iter().any(|s| s.name == "Bbb"));
        // 改 a.ets 后精确失效：冷却期内应直接看到更新后的符号
        std::fs::write(dir.join("a.ets"), "struct Aaa {}\nfn newA() {}").unwrap();
        invalidate_files(&dir, &["a.ets".to_string()]);
        let syms2 = index_project_cached(&dir);
        assert!(syms2.iter().any(|s| s.name == "newA"), "精确失效后应看到新符号");
        assert!(syms2.iter().any(|s| s.name == "Bbb"), "其他文件符号不受影响");
        assert!(!syms2.iter().any(|s| s.name == "oldA"), "被替换的旧符号应移除");
        // 删除 b.ets 后精确失效
        std::fs::remove_file(dir.join("b.ets")).unwrap();
        invalidate_files(&dir, &["b.ets".to_string()]);
        let syms3 = index_project_cached(&dir);
        assert!(!syms3.iter().any(|s| s.name == "Bbb"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalidate_files_handles_deleted_dir() {
        let dir = make_project("invd");
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("c.ets"), "struct Ccc {}").unwrap();
        let syms = index_project_cached(&dir);
        assert!(syms.iter().any(|s| s.name == "Ccc"));
        // 删除整个子目录后按目录路径失效
        std::fs::remove_dir_all(&sub).unwrap();
        invalidate_files(&dir, &["sub".to_string()]);
        let syms2 = index_project_cached(&dir);
        assert!(!syms2.iter().any(|s| s.name == "Ccc"), "目录下文件符号应全部移除");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn persisted_roundtrip() {
        let data_dir = std::env::temp_dir().join("deveco-symbol-cache-dir");
        let _ = std::fs::remove_dir_all(&data_dir);
        std::fs::create_dir_all(&data_dir).unwrap();
        let proj = make_project("persist");
        let mut files = HashMap::new();
        files.insert("a.ets".to_string(), FileStamp { mtime: 123, len: 45 });
        let syms = vec![Symbol { kind: "struct".into(), name: "Aaa".into(), file: "a.ets".into(), line: 1, parent: None }];
        save_to(&data_dir, &proj, &files, &syms);
        let loaded = load_from(&data_dir, &proj).expect("应能从磁盘恢复");
        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.files["a.ets"], FileStamp { mtime: 123, len: 45 });
        assert_eq!(loaded.syms.len(), 1);
        assert_eq!(loaded.syms[0].name, "Aaa");
        // 损坏内容应返回 None（触发全量重建，不 panic）
        std::fs::write(cache_file_at(&data_dir, &proj), "not-json").unwrap();
        assert!(load_from(&data_dir, &proj).is_none());
        std::fs::remove_dir_all(&data_dir).ok();
        std::fs::remove_dir_all(&proj).ok();
    }
}
