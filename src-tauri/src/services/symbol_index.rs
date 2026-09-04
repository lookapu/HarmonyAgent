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
use rusqlite::{params, Connection};

use crate::services::harmony;

const SYMBOL_EXTS: &[&str] = &["ets", "ts", "tsx", "js", "jsx", "rs", "py", "kt", "java", "swift", "go", "cpp", "c", "h", "hpp"];

const SKIP_DIRS: &[&str] = &[
    "node_modules", ".git", "build", ".hvigor", "oh_modules", ".idea", "dist",
    ".cxx", ".preview", ".test", ".ohpm", ".arkui-x", "coverage", ".venv", "target",
];

const MAX_FILES: usize = 4000;
const MAX_BYTES: u64 = 512 * 1024;

/// 全库文件目录统计。目录覆盖所有未被忽略的普通文件；结构解析可以渐进完成。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct CatalogStats {
    pub discovered_files: usize,
    pub source_files: usize,
    pub indexed_source_files: usize,
    pub deferred_source_files: usize,
    pub oversized_source_files: usize,
    pub unsupported_files: usize,
    pub symlink_files: usize,
    pub unreadable_files: usize,
    pub unreadable_directories: usize,
    pub persisted: bool,
}

impl CatalogStats {
    fn coverage(&self) -> String {
        if self.deferred_source_files > 0 {
            format!(
                "partial_{}_source_files_deferred_by_parse_budget",
                self.deferred_source_files
            )
        } else if self.oversized_source_files > 0
            || self.unreadable_files > 0
            || self.unreadable_directories > 0
        {
            format!(
                "partial_{}_oversized_{}_unreadable_files_{}_unreadable_directories",
                self.oversized_source_files,
                self.unreadable_files,
                self.unreadable_directories,
            )
        } else {
            "best_effort_lightweight_syntax_index".into()
        }
    }
}

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
    /// 结构块结束行（1-based，含）；无法识别块时等于定义行。
    #[serde(default)]
    pub end_line: usize,
    /// 结构角色：entity（类/组件/类型/状态）或 logic（函数/方法）。
    #[serde(default)]
    pub role: String,
    /// 定义签名的单行摘要，不包含方法正文。
    #[serde(default)]
    pub signature: String,
    /// 所在类/组件（方法的归属，顶层为空）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

fn structure_role(kind: &str) -> &'static str {
    if matches!(kind, "function" | "method") {
        "logic"
    } else {
        "entity"
    }
}

fn leading_indent(line: &str) -> usize {
    line.chars().take_while(|ch| ch.is_whitespace()).count()
}

/// 轻量结构块范围。这里保持容错和零额外依赖；Tree-sitter/LSP 接入后将作为 fallback。
fn structure_end_line(lines: &[&str], start: usize, ext: &str, kind: &str) -> usize {
    if matches!(kind, "decorator" | "route") {
        return start + 1;
    }
    if ext == "py" {
        let base = leading_indent(lines.get(start).copied().unwrap_or(""));
        let mut end = start;
        for (idx, line) in lines.iter().enumerate().skip(start + 1) {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if leading_indent(line) <= base {
                break;
            }
            end = idx;
        }
        return end + 1;
    }
    let mut found_open = false;
    let mut depth = 0i64;
    for (idx, line) in lines.iter().enumerate().skip(start) {
        for ch in line.chars() {
            match ch {
                '{' => {
                    found_open = true;
                    depth += 1;
                }
                '}' if found_open => {
                    depth -= 1;
                    if depth == 0 {
                        return idx + 1;
                    }
                }
                _ => {}
            }
        }
        // 声明没有块体时不要吞掉后续定义。
        if !found_open && line.trim_end().ends_with(';') {
            return start + 1;
        }
    }
    start + 1
}

fn make_symbol(
    kind: &str,
    name: String,
    rel: &str,
    line: usize,
    parent: Option<String>,
    raw: &str,
    lines: &[&str],
    ext: &str,
) -> Symbol {
    Symbol {
        kind: kind.into(),
        name,
        file: rel.into(),
        line,
        end_line: structure_end_line(lines, line.saturating_sub(1), ext, kind),
        role: structure_role(kind).into(),
        signature: raw.trim().chars().take(300).collect(),
        parent,
    }
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
    let lines: Vec<&str> = content.lines().collect();
    let mut current_parent: Option<String> = None;
    let mut brace_depth = 0i32;
    let class_like = ["class ", "interface ", "struct ", "enum ", "object ", "trait ", "impl "];

    for (idx, raw) in lines.iter().copied().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('*') || line.starts_with("/*") {
            // 仍需统计大括号（注释里的括号近似忽略，足够符号提取使用）
            continue;
        }
        let lineno = idx + 1;

        // ArkTS/HarmonyOS 装饰器：入口/组件/路由 + 状态管理装饰器
        if ext == "ets" && line.starts_with('@') {
            if line.starts_with("@Entry") {
                out.push(make_symbol("decorator", "@Entry".into(), rel, lineno, None, raw, &lines, ext));
            }
            if line.starts_with("@Component") {
                out.push(make_symbol("decorator", "@Component".into(), rel, lineno, None, raw, &lines, ext));
            }
            if line.starts_with("@Router") {
                out.push(make_symbol("route", "@Router".into(), rel, lineno, None, raw, &lines, ext));
            }
            // 状态管理装饰器：仅当装饰器名后是空白/(/) 等边界符时计入，
            // 避免把 "@StateXxx" 这类普通标识符误报为装饰器
            for dec in ETS_STATE_DECORATORS {
                if let Some(rest) = line.strip_prefix(*dec) {
                    if rest.chars().next().is_none_or(|c| !is_ident(c)) {
                        out.push(make_symbol("decorator", (*dec).into(), rel, lineno, None, raw, &lines, ext));
                    }
                    break;
                }
            }
        }

        // 类型定义
        for kw in &class_like {
            if let Some(name) = ident_after(line, kw) {
                let kind = kw.trim();
                out.push(make_symbol(kind, name.clone(), rel, lineno, None, raw, &lines, ext));
                current_parent = Some(name);
                break;
            }
        }

        // 函数/方法
        let fn_kw = if ext == "py" { "def " } else { "fn " };
        if let Some(name) = ident_after(line, fn_kw) {
            out.push(make_symbol("function", name, rel, lineno, current_parent.clone(), raw, &lines, ext));
        }
        if let Some(name) = ident_after(line, "function ") {
            out.push(make_symbol("function", name, rel, lineno, current_parent.clone(), raw, &lines, ext));
        }
        // ArkTS 组件 struct
        if ext == "ets" {
            if let Some(name) = ident_after(line, "struct ") {
                out.push(make_symbol("component", name, rel, lineno, None, raw, &lines, ext));
            }
            // 方法形似 name(...) {
            if line.contains('(') && line.ends_with('{') {
                let first = line.split('(').next().unwrap_or("").trim();
                let name = first.split_whitespace().last().unwrap_or("");
                if !name.is_empty()
                    && is_ident_start(name.chars().next().unwrap_or(' '))
                    && !["if", "for", "while", "switch", "catch", "when", "return", "else"].contains(&name)
                {
                    out.push(make_symbol("method", name.to_string(), rel, lineno, current_parent.clone(), raw, &lines, ext));
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
    Some(stamp_from_meta(&meta))
}

fn stamp_from_meta(meta: &fs::Metadata) -> FileStamp {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    FileStamp { mtime, len: meta.len() }
}

fn catalog_file_at(dir: &Path, root: &Path) -> PathBuf {
    let key = canonical_key(root);
    dir.join("repo_catalog")
        .join(format!("{:016x}.sqlite3", stable_hash(&key)))
}

#[derive(Debug, Serialize)]
pub struct CatalogFile {
    pub path: String,
    pub extension: String,
    pub size: u64,
    pub state: String,
    pub shard: String,
}

#[derive(Debug, Serialize)]
pub struct CatalogQueryResult {
    pub items: Vec<CatalogFile>,
    pub total_matches: usize,
    pub page: usize,
    pub page_size: usize,
    pub next_page: Option<usize>,
}

fn glob_to_sql_like(pattern: &str) -> String {
    let mut out = String::new();
    for ch in pattern.replace('\\', "/").chars() {
        match ch {
            '*' => out.push('%'),
            '?' => out.push('_'),
            '%' | '_' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

/// 查询持久化全库目录。None 表示应用数据目录尚未初始化，调用方可回退即时扫描。
pub fn query_catalog_files(
    root: &Path,
    pattern: &str,
    prefix: Option<&str>,
    state: Option<&str>,
    page: usize,
    page_size: usize,
) -> Option<Result<CatalogQueryResult, String>> {
    let data_dir = DATA_DIR.get()?;
    let _ = index_project_cached(root);
    let conn = match Connection::open(catalog_file_at(data_dir, root)) {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("打开全库目录失败：{error}"))),
    };
    let page = page.max(1);
    let page_size = page_size.clamp(1, 200);
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    let like = glob_to_sql_like(pattern);
    let basename_like = format!("%/{like}");
    let prefix_like = prefix
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != ".")
        .map(|value| format!("{}/%", value.trim_matches('/')))
        .unwrap_or_else(|| "%".into());
    let state = state.map(str::trim).filter(|value| !value.is_empty()).unwrap_or("%");
    let where_sql = "(path LIKE ?1 ESCAPE '\\' COLLATE NOCASE
                      OR path LIKE ?2 ESCAPE '\\' COLLATE NOCASE)
                     AND path LIKE ?3 ESCAPE '\\' COLLATE NOCASE
                     AND state LIKE ?4 COLLATE NOCASE";
    let total_matches = match conn.query_row(
        &format!("SELECT COUNT(*) FROM files WHERE {where_sql}"),
        params![like, basename_like, prefix_like, state],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(value) => value.max(0) as usize,
        Err(error) => return Some(Err(format!("查询全库目录失败：{error}"))),
    };
    let mut stmt = match conn.prepare(&format!(
        "SELECT path, extension, size, state, shard FROM files
         WHERE {where_sql} ORDER BY path LIMIT ?5 OFFSET ?6"
    )) {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("准备全库目录查询失败：{error}"))),
    };
    let rows = match stmt.query_map(
        params![like, basename_like, prefix_like, state, page_size as i64, offset as i64],
        |row| {
            Ok(CatalogFile {
                path: row.get(0)?,
                extension: row.get(1)?,
                size: row.get::<_, i64>(2)?.max(0) as u64,
                state: row.get(3)?,
                shard: row.get(4)?,
            })
        },
    ) {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("读取全库目录失败：{error}"))),
    };
    let items = match rows.collect::<Result<Vec<_>, _>>() {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("解析全库目录结果失败：{error}"))),
    };
    Some(Ok(CatalogQueryResult {
        items,
        total_matches,
        page,
        page_size,
        next_page: (offset.saturating_add(page_size) < total_matches).then_some(page + 1),
    }))
}

fn shard_for(rel: &str) -> &str {
    rel.split('/').next().filter(|value| !value.is_empty()).unwrap_or(".")
}

/// 遍历所有未忽略文件。回调是流式的，百万文件时不需要把全目录保存在内存。
fn walk_catalog<F>(dir: &Path, root: &Path, stats: &mut CatalogStats, visit: &mut F)
where
    F: FnMut(&str, &str, u64, u64, &str, &str),
{
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => {
            stats.unreadable_directories += 1;
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let file_type = match entry.file_type() {
            Ok(value) => value,
            Err(_) => {
                stats.unreadable_files += 1;
                continue;
            }
        };
        if file_type.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            walk_catalog(&path, root, stats, visit);
            continue;
        }

        let rel = safe_rel(root, &path);
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        stats.discovered_files += 1;
        if file_type.is_symlink() {
            stats.symlink_files += 1;
            visit(&rel, ext, 0, 0, "symlink", shard_for(&rel));
            continue;
        }
        if !file_type.is_file() {
            stats.unsupported_files += 1;
            visit(&rel, ext, 0, 0, "unsupported", shard_for(&rel));
            continue;
        }
        let meta = match entry.metadata() {
            Ok(value) => value,
            Err(_) => {
                stats.unreadable_files += 1;
                visit(&rel, ext, 0, 0, "unreadable", shard_for(&rel));
                continue;
            }
        };
        let stamp = stamp_from_meta(&meta);
        if !SYMBOL_EXTS.contains(&ext) {
            stats.unsupported_files += 1;
            visit(&rel, ext, stamp.len, stamp.mtime, "unsupported", shard_for(&rel));
        } else if stamp.len > MAX_BYTES {
            stats.source_files += 1;
            stats.oversized_source_files += 1;
            visit(&rel, ext, stamp.len, stamp.mtime, "oversized", shard_for(&rel));
        } else if stats.indexed_source_files < MAX_FILES {
            stats.source_files += 1;
            stats.indexed_source_files += 1;
            visit(&rel, ext, stamp.len, stamp.mtime, "indexed", shard_for(&rel));
        } else {
            stats.source_files += 1;
            stats.deferred_source_files += 1;
            visit(&rel, ext, stamp.len, stamp.mtime, "deferred", shard_for(&rel));
        }
    }
}

/// 刷新全库目录，并返回本轮允许进入轻量结构解析预算的源码文件。
fn collect_files_at(
    root: &Path,
    data_dir: Option<&Path>,
) -> (HashMap<String, FileStamp>, CatalogStats) {
    let mut files = HashMap::new();
    let mut stats = CatalogStats::default();
    let generation = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0);

    let mut catalog = data_dir
        .and_then(|dir| {
            let path = catalog_file_at(dir, root);
            fs::create_dir_all(path.parent()?).ok()?;
            let conn = Connection::open(path).ok()?;
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=NORMAL;
                 CREATE TABLE IF NOT EXISTS files (
                   path TEXT PRIMARY KEY,
                   extension TEXT NOT NULL,
                   size INTEGER NOT NULL,
                   mtime_ns INTEGER NOT NULL,
                   state TEXT NOT NULL,
                   shard TEXT NOT NULL,
                   generation INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_files_state ON files(state);
                 CREATE INDEX IF NOT EXISTS idx_files_shard ON files(shard);
                 BEGIN IMMEDIATE;",
            )
            .ok()?;
            Some(conn)
        });
    let mut write_failed = false;
    walk_catalog(root, root, &mut stats, &mut |rel, ext, size, mtime, state, shard| {
        if state == "indexed" {
            files.insert(rel.to_string(), FileStamp { mtime, len: size });
        }
        if let Some(conn) = catalog.as_mut() {
            if conn.execute(
                "INSERT INTO files(path, extension, size, mtime_ns, state, shard, generation)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(path) DO UPDATE SET
                   extension=excluded.extension, size=excluded.size, mtime_ns=excluded.mtime_ns,
                   state=excluded.state, shard=excluded.shard, generation=excluded.generation",
                params![rel, ext, size as i64, mtime as i64, state, shard, generation],
            ).is_err() {
                write_failed = true;
            }
        }
    });
    if let Some(conn) = catalog.as_mut() {
        // 某个目录暂时不可读时保留其上一代记录，避免一次权限抖动被误判成整目录删除。
        let cleanup_ok = stats.unreadable_directories > 0
            || conn
                .execute("DELETE FROM files WHERE generation <> ?1", [generation])
                .is_ok();
        if !write_failed
            && cleanup_ok
            && conn.execute_batch("COMMIT;").is_ok()
        {
            stats.persisted = true;
        } else {
            let _ = conn.execute_batch("ROLLBACK;");
        }
    }
    (files, stats)
}

fn collect_files(root: &Path) -> (HashMap<String, FileStamp>, CatalogStats) {
    collect_files_at(root, DATA_DIR.get().map(PathBuf::as_path))
}

/// 扫描整个项目，返回全部符号（全量构建：无缓存的底层实现）。
/// 强制刷新入口 refresh_project_symbols 已改为 invalidate_cache + cached 组合，此函数暂无调用者。
#[allow(dead_code)]
pub fn index_project(root: &Path) -> Vec<Symbol> {
    let (files, _) = collect_files(root);
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
    catalog: CatalogStats,
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
// 全库目录会覆盖所有未忽略文件，避免高频查询反复遍历百万文件；工具内修改仍会精确失效。
// watcher/Git diff 补偿接入后可进一步延长或移除周期性 walk。
const SYNC_COOLDOWN_SECS: u64 = 30;

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
    catalog: CatalogStats,
}

// v3 持久化全库目录覆盖统计；旧缓存重建以避免把解析子集误报为全库。
const PERSIST_VERSION: u32 = 3;

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
fn save_to(
    dir: &Path,
    root: &Path,
    files: &HashMap<String, FileStamp>,
    syms: &[Symbol],
    catalog: CatalogStats,
) {
    let path = cache_file_at(dir, root);
    let Some(parent) = path.parent() else { return };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let idx = PersistedIndex {
        version: PERSIST_VERSION,
        files: files.clone(),
        syms: syms.to_vec(),
        catalog,
    };
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

fn save_persisted(
    root: &Path,
    files: &HashMap<String, FileStamp>,
    syms: &[Symbol],
    catalog: CatalogStats,
) {
    if let Some(dir) = DATA_DIR.get() {
        save_to(dir, root, files, syms, catalog);
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
) -> (usize, usize, CatalogStats) {
    let (current, catalog) = collect_files(root);

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
    (rescanned, removed, catalog)
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
            catalog: CatalogStats::default(),
            last_sync: 0,
            source: "scan",
        });
        // 条目为空时尝试从磁盘恢复，避免重启后全量重扫
        if entry.files.is_empty() && entry.syms.is_empty() {
            if let Some(persisted) = load_persisted(root) {
                entry.files = persisted.files;
                entry.syms = persisted.syms;
                entry.catalog = persisted.catalog;
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
    let (_rescanned, _removed, catalog) = sync_incremental(&mut files, &mut syms, root);
    // 阶段 3（锁内）：CAS 写回——期间有其他线程同步过（last_sync 变化）则丢弃本地结果。
    // invalidate_files 精确更新同样会推进 last_sync，不会被本阶段覆盖丢失。
    let mut guard = cache().lock().unwrap();
    let out = if let Some(entry) = guard.get_mut(&key) {
        if entry.last_sync == snap_sync {
            entry.files = files;
            entry.syms = syms;
            entry.catalog = catalog;
            entry.last_sync = now;
            save_persisted(root, &entry.files, &entry.syms, entry.catalog);
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
    if let Some(dir) = DATA_DIR.get() {
        let path = catalog_file_at(dir, root);
        for candidate in [
            path.clone(),
            path.with_extension("sqlite3-wal"),
            path.with_extension("sqlite3-shm"),
        ] {
            let _ = fs::remove_file(candidate);
        }
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
                .is_some_and(|(a, r)| a.starts_with(&r));
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
        save_persisted(root, &entry.files, &entry.syms, entry.catalog);
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
        .filter(|s| kind.is_none_or(|k| s.kind == k))
        .filter(|s| q.is_empty() || s.name.to_lowercase().contains(&q) || s.file.to_lowercase().contains(&q))
        .take(200)
        .collect()
}

/// 面向 Agent 的结构优先查询结果。保留 Symbol 作为前端兼容模型，同时补齐分页、
/// 覆盖状态和新鲜度，调用方据此决定是否读取具体代码块。
#[derive(Debug, Serialize)]
pub struct StructureQueryResult {
    pub items: Vec<Symbol>,
    pub total_matches: usize,
    pub page: usize,
    pub page_size: usize,
    pub next_page: Option<usize>,
    pub indexed_files: usize,
    pub indexed_symbols: usize,
    pub catalog: CatalogStats,
    pub coverage: String,
    pub synced_ago_secs: u64,
}

pub fn query_structure(
    root: &Path,
    query: &str,
    role: Option<&str>,
    kind: Option<&str>,
    file: Option<&str>,
    page: usize,
    page_size: usize,
) -> StructureQueryResult {
    let syms = index_project_cached(root);
    let q = query.trim().to_lowercase();
    let role = role.map(str::trim).filter(|value| !value.is_empty());
    let kind = kind.map(str::trim).filter(|value| !value.is_empty());
    let file_filter = file.map(str::trim).filter(|value| !value.is_empty()).map(str::to_lowercase);
    // 查询阶段只排序引用，最后仅克隆当前页，避免空查询时复制百万级结构元数据。
    let mut matched: Vec<&Symbol> = syms
        .iter()
        .filter(|symbol| role.is_none_or(|value| symbol.role == value))
        .filter(|symbol| kind.is_none_or(|value| symbol.kind == value))
        .filter(|symbol| {
            file_filter
                .as_ref()
                .is_none_or(|value| symbol.file.to_lowercase().contains(value))
        })
        .filter(|symbol| {
            q.is_empty()
                || symbol.name.to_lowercase().contains(&q)
                || symbol.file.to_lowercase().contains(&q)
                || symbol.signature.to_lowercase().contains(&q)
        })
        .collect();
    matched.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.name.cmp(&b.name))
    });
    let total_matches = matched.len();
    let page = page.max(1);
    let page_size = page_size.clamp(1, 200);
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    let items = matched
        .into_iter()
        .skip(offset)
        .take(page_size)
        .cloned()
        .collect();
    let next_page = (offset.saturating_add(page_size) < total_matches).then_some(page + 1);

    let key = canonical_key(root);
    let now = now_secs();
    let (indexed_files, catalog, synced_ago_secs) = cache()
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .get(&key)
                .map(|entry| {
                    (
                        entry.files.len(),
                        entry.catalog,
                        now.saturating_sub(entry.last_sync),
                    )
                })
        })
        .unwrap_or((0, CatalogStats::default(), 0));
    let coverage = catalog.coverage();
    StructureQueryResult {
        items,
        total_matches,
        page,
        page_size,
        next_page,
        indexed_files,
        indexed_symbols: syms.len(),
        catalog,
        coverage,
        synced_ago_secs,
    }
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
    pub catalog: CatalogStats,
    pub coverage: String,
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
            catalog: e.catalog,
            coverage: e.catalog.coverage(),
        },
        // 条目被容量上限清空：仅能给出符号数（来源视为本次扫描）
        None => SymbolIndexMeta {
            symbols: syms.len(),
            files: 0,
            source: "scan",
            synced_ago_secs: 0,
            catalog: CatalogStats::default(),
            coverage: "unavailable".into(),
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
        let component = out.iter().find(|s| s.kind == "component" && s.name == "Index").unwrap();
        assert_eq!(component.role, "entity");
        assert!(component.end_line > component.line);
        let method = out.iter().find(|s| s.kind == "method" && s.name == "aboutToAppear").unwrap();
        assert_eq!(method.role, "logic");
        assert!(method.signature.contains("aboutToAppear"));
        assert!(method.end_line >= method.line);
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
            Symbol { kind: "function".into(), name: "loadData".into(), file: "a.ts".into(), line: 1, end_line: 1, role: "logic".into(), signature: "function loadData()".into(), parent: None },
            Symbol { kind: "component".into(), name: "BookCard".into(), file: "b.ets".into(), line: 2, end_line: 5, role: "entity".into(), signature: "struct BookCard".into(), parent: None },
        ];
        assert_eq!(filter_symbols(&syms, "book", None).len(), 1);
        assert_eq!(filter_symbols(&syms, "", Some("component")).len(), 1);
        assert_eq!(filter_symbols(&syms, "", None).len(), 2);
    }

    #[test]
    fn catalog_coverage_distinguishes_deferred_and_best_effort() {
        let complete = CatalogStats {
            discovered_files: 3,
            source_files: 2,
            indexed_source_files: 2,
            unsupported_files: 1,
            ..CatalogStats::default()
        };
        assert_eq!(complete.coverage(), "best_effort_lightweight_syntax_index");
        let deferred = CatalogStats {
            deferred_source_files: 17,
            ..complete
        };
        assert_eq!(
            deferred.coverage(),
            "partial_17_source_files_deferred_by_parse_budget"
        );
        assert_eq!(glob_to_sql_like("src/**/*.ets"), "src/%%/%.ets");
        assert_eq!(glob_to_sql_like("100%_ok?.ts"), "100\\%\\_ok_.ts");
    }

    #[test]
    fn catalog_persists_all_files_and_removes_stale_rows() {
        let root = std::env::temp_dir().join(format!(
            "deveco-catalog-project-{}",
            uuid::Uuid::new_v4()
        ));
        let data_dir = std::env::temp_dir().join(format!(
            "deveco-catalog-data-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(root.join("small.rs"), "fn small() {}\n").unwrap();
        std::fs::write(root.join("README.md"), "# hello\n").unwrap();
        std::fs::write(root.join("large.ts"), vec![b'x'; MAX_BYTES as usize + 1]).unwrap();

        let (files, stats) = collect_files_at(&root, Some(&data_dir));
        assert_eq!(files.len(), 1);
        assert_eq!(stats.discovered_files, 3);
        assert_eq!(stats.source_files, 2);
        assert_eq!(stats.oversized_source_files, 1);
        assert_eq!(stats.unsupported_files, 1);
        assert!(stats.persisted);

        let conn = Connection::open(catalog_file_at(&data_dir, &root)).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 3);
        let state: String = conn
            .query_row(
                "SELECT state FROM files WHERE path='large.ts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "oversized");
        drop(conn);

        std::fs::remove_file(root.join("README.md")).unwrap();
        let (_, refreshed) = collect_files_at(&root, Some(&data_dir));
        assert!(refreshed.persisted);
        let conn = Connection::open(catalog_file_at(&data_dir, &root)).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 2);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn structure_query_filters_roles_and_paginates_with_coverage() {
        let dir = make_project("structure-query");
        let first = query_structure(&dir, "", Some("entity"), None, None, 1, 1);
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].role, "entity");
        assert_eq!(first.page_size, 1);
        assert!(first.next_page.is_some());
        assert_eq!(first.coverage, "best_effort_lightweight_syntax_index");
        assert_eq!(first.catalog.discovered_files, 2);
        assert_eq!(first.catalog.indexed_source_files, 2);

        let logic = query_structure(&dir, "oldA", Some("logic"), None, Some("a.ets"), 1, 20);
        assert_eq!(logic.total_matches, 1);
        assert_eq!(logic.items[0].name, "oldA");
        assert_eq!(logic.items[0].role, "logic");
        std::fs::remove_dir_all(&dir).ok();
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
        let mut entry = CacheEntry {
            files: HashMap::new(),
            syms: Vec::new(),
            catalog: CatalogStats::default(),
            last_sync: 0,
            source: "scan",
        };
        // 首次同步：两个文件都是新增
        let (r1, _, catalog) = sync_incremental(&mut entry.files, &mut entry.syms, &dir);
        entry.catalog = catalog;
        assert_eq!(r1, 2);
        assert!(entry.syms.iter().any(|s| s.name == "Aaa"));
        assert!(entry.syms.iter().any(|s| s.name == "oldA"));
        assert!(entry.syms.iter().any(|s| s.name == "Bbb"));
        // 只改 a.ets（长度变化 → 指纹变化）
        std::fs::write(dir.join("a.ets"), "struct Aaa {}\nfn oldA() {}\nfn newA() {}").unwrap();
        let (r2, _, catalog) = sync_incremental(&mut entry.files, &mut entry.syms, &dir);
        entry.catalog = catalog;
        assert_eq!(r2, 1, "只有 a.ets 应被重扫");
        assert!(entry.syms.iter().any(|s| s.name == "newA"), "变化文件的新符号应出现");
        assert!(entry.syms.iter().any(|s| s.name == "Bbb"), "未变文件符号应保留");
        assert_eq!(entry.syms.iter().filter(|s| s.name == "oldA").count(), 1, "旧符号不应重复");
        // 删除 b.ets
        std::fs::remove_file(dir.join("b.ets")).unwrap();
        let (_, removed, catalog) = sync_incremental(&mut entry.files, &mut entry.syms, &dir);
        entry.catalog = catalog;
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
        let syms = vec![Symbol { kind: "struct".into(), name: "Aaa".into(), file: "a.ets".into(), line: 1, end_line: 1, role: "entity".into(), signature: "struct Aaa {}".into(), parent: None }];
        let catalog = CatalogStats {
            discovered_files: 1,
            source_files: 1,
            indexed_source_files: 1,
            persisted: true,
            ..CatalogStats::default()
        };
        save_to(&data_dir, &proj, &files, &syms, catalog);
        let loaded = load_from(&data_dir, &proj).expect("应能从磁盘恢复");
        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.files["a.ets"], FileStamp { mtime: 123, len: 45 });
        assert_eq!(loaded.syms.len(), 1);
        assert_eq!(loaded.syms[0].name, "Aaa");
        assert_eq!(loaded.catalog.discovered_files, 1);
        // 损坏内容应返回 None（触发全量重建，不 panic）
        std::fs::write(cache_file_at(&data_dir, &proj), "not-json").unwrap();
        assert!(load_from(&data_dir, &proj).is_none());
        std::fs::remove_dir_all(&data_dir).ok();
        std::fs::remove_dir_all(&proj).ok();
    }

    /// Phase 0 大仓基线。默认忽略，避免在普通 CI 中创建大量文件。
    ///
    /// 运行示例：
    /// HARMONY_INDEX_BENCH_FILES=10000 cargo test --lib \
    ///   services::symbol_index::tests::large_repo_baseline -- --ignored --exact --nocapture
    #[test]
    #[ignore = "手动大仓索引基准；通过 HARMONY_INDEX_BENCH_FILES 选择规模"]
    fn large_repo_baseline() {
        let requested = std::env::var("HARMONY_INDEX_BENCH_FILES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(10_000)
            .clamp(1, 1_000_000);
        let files_per_shard = 1_000usize;
        let root = std::env::temp_dir().join(format!(
            "deveco-symbol-scale-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();

        let generate_started = std::time::Instant::now();
        for index in 0..requested {
            let shard = root.join(format!("shard_{:04}", index / files_per_shard));
            if index % files_per_shard == 0 {
                std::fs::create_dir_all(&shard).unwrap();
            }
            std::fs::write(
                shard.join(format!("file_{index:07}.rs")),
                format!("pub fn symbol_{index:07}() -> usize {{ {index} }}\n"),
            )
            .unwrap();
        }
        let generation_ms = generate_started.elapsed().as_millis() as u64;

        invalidate_cache(&root);
        let cold_started = std::time::Instant::now();
        let cold_symbols = index_project_cached(&root);
        let cold_ms = cold_started.elapsed().as_millis() as u64;

        let warm_started = std::time::Instant::now();
        let warm_symbols = index_project_cached(&root);
        let warm_ms = warm_started.elapsed().as_millis() as u64;

        let changed_file = cold_symbols
            .first()
            .map(|symbol| symbol.file.clone())
            .expect("基准至少应索引一个符号");
        std::fs::write(
            root.join(&changed_file),
            "pub fn symbol_after_incremental_update() -> usize { 42 }\n",
        )
        .unwrap();
        let incremental_started = std::time::Instant::now();
        invalidate_files(&root, std::slice::from_ref(&changed_file));
        let incremental_symbols = index_project_cached(&root);
        let incremental_ms = incremental_started.elapsed().as_millis() as u64;
        let meta = index_meta(&root);
        assert_eq!(meta.catalog.discovered_files, requested);
        assert_eq!(meta.catalog.source_files, requested);
        assert_eq!(
            meta.catalog.deferred_source_files,
            requested.saturating_sub(MAX_FILES)
        );

        let report = serde_json::json!({
            "schema_version": 2,
            "requested_files": requested,
            "configured_max_files": MAX_FILES,
            "indexed_files": cold_symbols.iter().map(|symbol| &symbol.file).collect::<std::collections::HashSet<_>>().len(),
            "catalog_discovered_files": meta.catalog.discovered_files,
            "catalog_source_files": meta.catalog.source_files,
            "deferred_source_files": meta.catalog.deferred_source_files,
            "coverage": meta.coverage,
            "cold_symbols": cold_symbols.len(),
            "warm_symbols": warm_symbols.len(),
            "incremental_symbols": incremental_symbols.len(),
            "generation_ms": generation_ms,
            "cold_index_ms": cold_ms,
            "warm_query_ms": warm_ms,
            "single_file_incremental_ms": incremental_ms,
            "structure_parse_is_partial": requested > MAX_FILES,
            "platform": std::env::consts::OS,
            "architecture": std::env::consts::ARCH,
        });
        println!("HARMONY_INDEX_BASELINE={report}");

        assert!(!cold_symbols.is_empty());
        assert_eq!(cold_symbols.len(), warm_symbols.len());
        assert!(incremental_symbols
            .iter()
            .any(|symbol| symbol.name == "symbol_after_incremental_update"));
        invalidate_cache(&root);
        std::fs::remove_dir_all(&root).ok();
    }
}
