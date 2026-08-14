//! 分级代码扫描族：静态检查 / 深度扫描 / 全库检索 / 符号详情
//!
//! 对标主流 Agent 的扫描分层能力：
//! - `check_code`      静态检查（规则式 lint）：调试残留 / TODO / 硬编码密钥 / 空 catch /
//!                     类型逃逸 / 明文 http 等，返回 file:line + 规则 + 建议
//! - `deep_scan`       深度扫描：全库结构 + 质量报告（扩展名分布 / 大文件 / 符号密度 /
//!                     import 依赖拓扑 / 疑似死代码候选）
//! - `codebase_search` 全库混合检索：符号名 + 路径 + 内容行三路匹配打分排序（无需向量库）
//! - `get_symbol_details` 符号详情：定义信息 + 前置注释 + 全库引用位置反查
//!
//! 设计取舍：全部基于进程内轻量扫描（复用 symbol_index 缓存），不引入外部 LSP/索引服务；
//! 输出统一截断护栏，避免挤占模型上下文预算。

use std::path::Path;

/// 需要跳过的目录（与 tools.rs 保持同一清单，避免扫描依赖/产物/工具自身数据）
const SKIP_DIRS: [&str; 15] = [
    ".git", ".hvigor", ".idea", ".ohpm", "node_modules", "oh_modules", "build", ".arkui-x",
    ".deveco-agent", "dist", "target", ".venv", "coverage", ".cxx", ".preview",
];

fn should_skip_dir(name: &str) -> bool {
    SKIP_DIRS.iter().any(|s| *s == name)
}

/// 源码文件扩展名（扫描对象；json5 参与结构统计但不参与规则检查）
const SRC_EXTS: [&str; 6] = ["ets", "ts", "tsx", "js", "jsx", "json5"];

fn is_src_file(name: &str) -> bool {
    SRC_EXTS
        .iter()
        .any(|e| name.len() > e.len() + 1 && name.ends_with(&format!(".{e}")))
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
        .replace('\\', "/")
}

/// 递归收集源码文件（跳过忽略目录与超大文件）
fn collect_src_files(root: &Path, max_size: u64) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    walk(root, root, max_size, &mut out);
    out
}

fn walk(dir: &Path, root: &Path, max_size: u64, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if !should_skip_dir(&name) {
                walk(&p, root, max_size, out);
            }
        } else if is_src_file(&name) {
            if e.metadata().map(|m| m.len() <= max_size).unwrap_or(false) {
                out.push(p.clone());
            }
        }
    }
    let _ = root;
}

/// 快速统计文件行数（BufRead 按行 count，避免整文件读入内存）
fn count_lines(path: &Path) -> Option<usize> {
    use std::io::BufRead;
    let f = std::fs::File::open(path).ok()?;
    let mut n = 0usize;
    for line in std::io::BufReader::new(f).lines() {
        if line.is_ok() {
            n += 1;
        }
    }
    Some(n)
}

/// 截断输出（超出按字符截断，保留头部；尾部附提示）
fn cut(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!("{}\n…(输出已截断，可缩小范围或分目录再次扫描)", s.chars().take(max).collect::<String>())
    } else {
        s.to_string()
    }
}

// ==================== check_code：规则式静态检查 ====================

#[derive(Clone, Copy)]
enum Severity {
    High,
    Medium,
    Low,
    Info,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::High => "高危",
            Severity::Medium => "中危",
            Severity::Low => "低危",
            Severity::Info => "提示",
        }
    }
}

struct Rule {
    id: &'static str,
    severity: Severity,
    message: &'static str,
    hit: fn(&str) -> bool,
}

/// 规则表：只收录误报率可控、工程内确实有价值的模式。
/// 命中行会被去杂后回传（截断 200 字符），供模型判断是否真实问题。
const RULES: &[Rule] = &[
    Rule {
        id: "debug-log",
        severity: Severity::Info,
        message: "调试输出残留（console.log/debug/info、print(、debugger、hilog 调试级），正式提交前应清理或降级",
        hit: |line| {
            line.contains("console.log")
                || line.contains("console.debug")
                || line.contains("console.info")
                || line.contains("console.warn")
                || line.contains("print(")
                || line.contains("debugger")
                || line.contains("hilog.debug(")
                || line.contains("hilog.info(")
        },
    },
    Rule {
        id: "todo-mark",
        severity: Severity::Info,
        message: "TODO/FIXME/HACK 待办标记，确认是否仍需处理或已过期",
        hit: |line| {
            let t = line.trim_start();
            (t.starts_with("//") || t.starts_with("/*") || t.starts_with('*'))
                && (line.contains("TODO")
                    || line.contains("FIXME")
                    || line.contains("HACK")
                    || line.contains("XXX")
                    || line.contains("待办"))
        },
    },
    Rule {
        id: "hardcoded-secret",
        severity: Severity::High,
        message: "疑似硬编码密钥/口令（password/secret/api_key/token 赋值为非空字面量），存在泄露风险，应改用环境变量或配置中心",
        hit: |line| {
            let l = line.to_lowercase();
            (l.contains("password") || l.contains("passwd") || l.contains("secret") || l.contains("api_key") || l.contains("apikey") || l.contains("token"))
                && (line.contains('=') || line.contains(':'))
                && (line.contains('"') || line.contains('\''))
                && !line.trim_start().starts_with("//")
        },
    },
    Rule {
        id: "empty-catch",
        severity: Severity::Medium,
        message: "空 catch 块吞掉异常（静默失败难以排查），至少记录日志或处理错误",
        hit: |line| {
            let t: String = line.chars().filter(|c| !c.is_whitespace()).collect();
            t.contains("catch{") || t.contains("catch(e){}") || t.contains("catch(err){}") || t.contains("catch(_){}")
        },
    },
    Rule {
        id: "any-escape",
        severity: Severity::Medium,
        message: "any 类型逃逸（: any / as any / <any>）或 ts-ignore，削弱类型检查，应改用具体类型或 unknown",
        hit: |line| {
            line.contains(": any")
                || line.contains("as any")
                || line.contains("<any>")
                || line.contains("@ts-ignore")
                || line.contains("@ts-nocheck")
        },
    },
    Rule {
        id: "plaintext-http",
        severity: Severity::Low,
        message: "明文 http:// 地址（非加密传输），生产环境建议升级 https",
        hit: |line| line.contains("http://") && !line.contains("localhost") && !line.contains("127.0.0.1"),
    },
];

/// 单条命中记录
struct Hit {
    file: String,
    line: usize,
    text: String,
}

/// 执行静态检查：path 指定子目录（缺省项目根），kind 过滤扩展名（arkts=ets/ts）。
/// 每规则每文件最多报 3 条，最多扫 300 个文件，输出按严重级别分组。
pub fn check_code(root: &Path, path: Option<&str>, kind: Option<&str>) -> Result<String, String> {
    let scan_root = match path {
        Some(p) if !p.trim().is_empty() => {
            let c = root.join(p.trim());
            if !c.is_dir() {
                return Err(format!("扫描目录不存在: {}", c.display()));
            }
            c
        }
        _ => root.to_path_buf(),
    };
    let kind_arkts = kind.map(|k| k == "arkts").unwrap_or(false);
    let files = collect_src_files(&scan_root, 512 * 1024);
    if files.is_empty() {
        return Ok("未发现可扫描的源码文件（.ets/.ts/.js 等）".into());
    }
    // 每个规则聚合命中（文件+行号+行内容），限制总量防输出爆炸
    let mut by_rule: Vec<(&'static Rule, Vec<Hit>)> = RULES.iter().map(|r| (r, Vec::new())).collect();
    let mut scanned = 0usize;
    for f in files.iter().take(300) {
        scanned += 1;
        let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("");
        if kind_arkts && ext != "ets" && ext != "ts" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let mut counts: Vec<usize> = vec![0; RULES.len()];
        for (i, line) in text.lines().enumerate() {
            for (ri, rule) in RULES.iter().enumerate() {
                if counts[ri] >= 3 {
                    continue;
                }
                if (rule.hit)(line) {
                    counts[ri] += 1;
                    let t = line.trim();
                    if t.len() > 200 {
                        continue;
                    }
                    by_rule[ri].1.push(Hit {
                        file: rel(root, f),
                        line: i + 1,
                        text: t.to_string(),
                    });
                }
            }
        }
    }
    let mut out = String::new();
    out.push_str(&format!(
        "静态检查完成：扫描 {scanned} 个文件，{} 条命中。\n",
        by_rule.iter().map(|(_, h)| h.len()).sum::<usize>()
    ));
    for (rule, hits) in &by_rule {
        if hits.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "\n## [{}] {}\n说明：{}\n",
            rule.severity.label(),
            rule.id,
            rule.message
        ));
        for h in hits.iter().take(12) {
            out.push_str(&format!("  {}:{}  {}\n", h.file, h.line, h.text));
        }
        if hits.len() > 12 {
            out.push_str(&format!("  …另 {} 条同类命中\n", hits.len() - 12));
        }
    }
    if by_rule.iter().all(|(_, h)| h.is_empty()) {
        out.push_str("\n未发现规则命中，代码整体较整洁。");
    }
    Ok(cut(&out, 15000))
}

// ==================== deep_scan：深度扫描报告 ====================

/// 提取单行 import 的目标（import … from 'xxx' / import 'xxx'）
fn import_target(line: &str) -> Option<String> {
    let t = line.trim();
    if !t.starts_with("import") {
        return None;
    }
    let from_pos = t.find("from")?;
    let rest = &t[from_pos + 4..];
    let s = rest.find(['\'', '"'])?;
    let q = rest.as_bytes()[s] as char;
    let e = rest[s + 1..].find(q)?;
    Some(rest[s + 1..s + 1 + e].to_string())
}

/// 深度扫描：全库结构 + 质量报告。
/// 输出：扩展名分布 / 总行数 / 最大文件 / 符号统计 / import 依赖拓扑 / 疑似死代码候选。
pub fn deep_scan(root: &Path, path: Option<&str>) -> Result<String, String> {
    let scan_root = match path {
        Some(p) if !p.trim().is_empty() => {
            let c = root.join(p.trim());
            if !c.is_dir() {
                return Err(format!("扫描目录不存在: {}", c.display()));
            }
            c
        }
        _ => root.to_path_buf(),
    };
    let files = collect_src_files(&scan_root, 1024 * 1024);
    if files.is_empty() {
        return Ok("未发现可扫描的源码文件".into());
    }

    // 1) 大小与扩展名分布
    let mut total_lines = 0usize;
    let mut ext_lines: std::collections::HashMap<String, (usize, usize)> = std::collections::HashMap::new();
    let mut sized: Vec<(String, usize)> = Vec::new();
    for f in &files {
        let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("?").to_string();
        let lines = count_lines(f).unwrap_or(0);
        total_lines += lines;
        let en = ext_lines.entry(ext).or_default();
        en.0 += 1;
        en.1 += lines;
        sized.push((rel(root, f), lines));
    }
    sized.sort_by(|a, b| b.1.cmp(&a.1));

    let mut out = String::new();
    out.push_str(&format!(
        "深度扫描报告（{}）\n源码文件 {} 个，共 {} 行。\n",
        scan_root.display(),
        files.len(),
        total_lines
    ));
    let mut exts: Vec<_> = ext_lines.into_iter().collect();
    exts.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
    out.push_str("\n按扩展名分布：\n");
    for (ext, (n, l)) in &exts {
        out.push_str(&format!("  .{ext}: {n} 个文件 / {l} 行\n"));
    }
    out.push_str("\n最大的文件（Top 15，超过 1000 行建议拆分）：\n");
    for (p, l) in sized.iter().take(15) {
        out.push_str(&format!("  {} 行  {p}\n", l));
    }

    // 2) 符号统计（复用缓存索引；超大工程索引耗时由 60s TTL 缓存摊薄）
    let syms = crate::services::symbol_index::index_project_cached(root);
    let mut by_kind: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut by_file: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for s in &syms {
        *by_kind.entry(s.kind.clone()).or_default() += 1;
        *by_file.entry(s.file.clone()).or_default() += 1;
    }
    let mut kinds: Vec<_> = by_kind.into_iter().collect();
    kinds.sort_by(|a, b| b.1.cmp(&a.1));
    out.push_str(&format!("\n符号索引：共 {} 个符号。\n", syms.len()));
    for (k, n) in kinds.iter().take(10) {
        out.push_str(&format!("  {k}: {n}\n"));
    }
    let mut dense: Vec<_> = by_file.into_iter().collect();
    dense.sort_by(|a, b| b.1.cmp(&a.1));
    out.push_str("\n符号最密集的文件（Top 10）：\n");
    for (f, n) in dense.iter().take(10) {
        out.push_str(&format!("  {n} 个符号  {f}\n"));
    }

    // 3) import 依赖拓扑（本地相对 import 计入图；第三方/系统包跳过）
    let mut imports_of: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let mut imported_by: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for f in &files {
        let relf = rel(root, f);
        let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "ets" && ext != "ts" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let mut deps: Vec<String> = Vec::new();
        for line in text.lines() {
            let Some(t) = import_target(line) else { continue };
            if t.starts_with("./") || t.starts_with("../") {
                deps.push(t);
            }
        }
        imports_of.insert(relf.clone(), deps);
    }
    // 本地文件 → 其他文件通过相对路径引用它的次数（目标归一化后按后缀包含匹配）
    for (_from, deps) in &imports_of {
        for d in deps {
            for to in imports_of.keys() {
                let base = to.trim_end_matches(".ets").trim_end_matches(".ts");
                if d.ends_with(base) || d.ends_with(to.as_str()) {
                    *imported_by.entry(to.clone()).or_default() += 1;
                }
            }
        }
    }
    let mut hot: Vec<_> = imported_by.iter().collect();
    hot.sort_by(|a, b| b.1.cmp(&a.1));
    out.push_str("\n被引用最多的模块（Top 10）：\n");
    for (f, n) in hot.iter().take(10) {
        out.push_str(&format!("  {n} 次引用  {f}\n"));
    }
    let mut outdegree: Vec<_> = imports_of.iter().map(|(f, d)| (f, d.len())).collect();
    outdegree.sort_by(|a, b| b.1.cmp(&a.1));
    out.push_str("\n依赖最多的模块（Top 10）：\n");
    for (f, n) in outdegree.iter().take(10) {
        out.push_str(&format!("  {n} 个依赖  {f}\n"));
    }
    // 疑似死代码候选：未被任何模块引用、且不含 @Entry 入口、不在 pages/ 目录
    let mut orphans: Vec<&String> = imported_by
        .iter()
        .filter(|(f, n)| {
            **n == 0
                && !f.contains("/pages/")
                && !f.contains("/view/")
                && !f.contains("/entry/")
        })
        .map(|(f, _)| f)
        .collect();
    orphans.sort();
    if !orphans.is_empty() {
        out.push_str(&format!(
            "\n疑似未被引用的文件（{} 个，需人工确认，可能为入口/反射/动态引用）：\n",
            orphans.len()
        ));
        for f in orphans.iter().take(20) {
            out.push_str(&format!("  {f}\n"));
        }
    } else {
        out.push_str("\n未发现明显孤立的本地模块。\n");
    }
    Ok(cut(&out, 15000))
}

// ==================== codebase_search：全库混合检索 ====================

/// 分词：非字母数字切分 + 驼峰边界拆分，保留长度 ≥2 的 token，去重，最多 5 个
fn tokenize(query: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for part in query.split(|c: char| !c.is_alphanumeric()) {
        if part.is_empty() {
            continue;
        }
        tokens.push(part.to_lowercase());
        // 驼峰拆分：CamelCase → camel, case
        let mut cur = String::new();
        for (i, c) in part.chars().enumerate() {
            if i > 0 && c.is_uppercase() {
                if !cur.is_empty() {
                    tokens.push(cur.to_lowercase());
                }
                cur = c.to_string();
            } else {
                cur.push(c);
            }
        }
        if !cur.is_empty() {
            tokens.push(cur.to_lowercase());
        }
    }
    let mut seen = std::collections::HashSet::new();
    tokens.retain(|t| t.len() >= 2 && seen.insert(t.clone()));
    tokens.truncate(5);
    tokens
}

/// 全库混合检索：query 分 token 后对符号名（5/3 分）、文件路径（2 分）、
/// 内容行（1 分/token）三路匹配并累计打分，返回 Top N 结果。
pub fn codebase_search(root: &Path, query: &str, limit: usize) -> Result<String, String> {
    let tokens = tokenize(query);
    if tokens.is_empty() {
        return Err("query 无有效关键词（需至少 2 个字母/数字）".into());
    }
    let limit = limit.clamp(1, 50);
    let mut scores: std::collections::HashMap<String, (usize, Vec<(usize, String)>)> =
        std::collections::HashMap::new();

    // 路 1：符号名/路径匹配（高权重）
    let syms = crate::services::symbol_index::index_project_cached(root);
    for s in &syms {
        let mut sc = 0usize;
        for t in &tokens {
            let name = s.name.to_lowercase();
            let file = s.file.to_lowercase();
            if name == *t {
                sc += 5;
            } else if name.contains(t) {
                sc += 3;
            }
            if file.contains(t) {
                sc += 2;
            }
        }
        if sc > 0 {
            let e = scores.entry(s.file.clone()).or_default();
            e.0 += sc;
            let marker = format!("符号 [{}] {}{}（定义于第 {} 行）", s.kind, s.name, s.parent.as_deref().map(|p| format!(" in {p}")).unwrap_or_default(), s.line);
            if e.1.len() < 3 {
                e.1.push((s.line, marker));
            }
        }
    }

    // 路 2：内容行匹配（低权重，量大截断）
    let files = collect_src_files(root, 256 * 1024);
    'outer: for f in files.iter().take(400) {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let relf = rel(root, f);
        for (i, line) in text.lines().enumerate() {
            let ll = line.to_lowercase();
            let mut hit = 0usize;
            for t in &tokens {
                if ll.contains(t) {
                    hit += 1;
                }
            }
            if hit == 0 {
                continue;
            }
            let e = scores.entry(relf.clone()).or_default();
            e.0 += hit;
            if e.1.len() < 3 {
                let text = line.trim();
                if text.len() <= 200 {
                    e.1.push((i + 1, text.to_string()));
                }
            }
            if scores.len() > 2000 {
                break 'outer; // 命中面过大：截断检索，避免输出爆炸
            }
        }
    }

    let mut ranked: Vec<_> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    if ranked.is_empty() {
        return Ok(format!("未找到与 \"{query}\" 匹配的内容（已检索符号索引与源码内容）"));
    }
    let mut out = String::new();
    out.push_str(&format!(
        "检索 \"{query}\"（关键词：{}），Top {} 结果：\n",
        tokens.join(", "),
        ranked.len().min(limit)
    ));
    for (file, (sc, hits)) in ranked.iter().take(limit) {
        out.push_str(&format!("\n[{sc} 分] {file}\n"));
        for (ln, text) in hits.iter().take(3) {
            out.push_str(&format!("  {ln}: {text}\n"));
        }
    }
    Ok(cut(&out, 12000))
}

// ==================== get_symbol_details：符号详情 + 引用反查 ====================

/// 读取定义行上方的连续注释（// 与 /** */ 混合场景取最近连续块，最多 6 行）
fn doc_comment_above(file: &Path, def_line: usize) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(file) else { return Vec::new() };
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = def_line.saturating_sub(2); // 0-based：定义行上一行
    while i > 0 && out.len() < 6 {
        let t = lines.get(i).map(|s| s.trim()).unwrap_or("");
        if t.starts_with("//") || t.starts_with('*') || t.starts_with("/*") {
            out.push(t.trim_start_matches('/').trim_start_matches('*').trim().to_string());
            i -= 1;
        } else if t.is_empty() {
            i -= 1; // 跳过空行，继续向上
        } else {
            break;
        }
    }
    out.reverse();
    out
}

/// 符号详情：定义信息（含前置注释）+ 全库引用位置（词边界粗匹配，排除定义处）。
/// 最多返回 5 个同名符号详情；引用最多 20 条。
pub fn symbol_details(root: &Path, name: &str, file_filter: Option<&str>) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("get_symbol_details 需要参数 {\"name\":\"<符号名>\",\"file\":\"<可选文件过滤>\"}".into());
    }
    let syms = crate::services::symbol_index::index_project_cached(root);
    let exact: Vec<_> = syms
        .iter()
        .filter(|s| s.name == name)
        .filter(|s| file_filter.map(|f| s.file.contains(f)).unwrap_or(true))
        .collect();
    let fuzzy: Vec<_> = syms
        .iter()
        .filter(|s| s.name.to_lowercase().contains(&name.to_lowercase()))
        .filter(|s| file_filter.map(|f| s.file.contains(f)).unwrap_or(true))
        .collect();
    let targets: Vec<_> = if !exact.is_empty() { exact } else { fuzzy };
    if targets.is_empty() {
        return Ok(format!("未找到符号 \"{name}\"（可先用 search_symbols 确认名称）"));
    }
    let mut out = String::new();
    out.push_str(&format!("符号 \"{name}\" 共 {} 个匹配：\n", targets.len()));
    for s in targets.iter().take(5) {
        let abs = root.join(&s.file);
        let docs = doc_comment_above(&abs, s.line);
        out.push_str(&format!(
            "\n- [{}] {}{}\n  定义：{}:{}\n",
            s.kind,
            s.name,
            s.parent.as_deref().map(|p| format!("（属于 {p}）")).unwrap_or_default(),
            s.file,
            s.line
        ));
        if !docs.is_empty() {
            out.push_str(&format!("  注释：{}\n", docs.join(" ")));
        }
    }
    // 引用反查：全库 grep（词边界粗匹配，排除定义文件:行）
    out.push_str(&format!("\n全库引用（\"{name}\" 出现位置，排除定义处，最多 20 条）：\n"));
    let mut refs = 0usize;
    for f in collect_src_files(root, 256 * 1024).iter().take(400) {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let relf = rel(root, f);
        for (i, line) in text.lines().enumerate() {
            if !line.contains(name) {
                continue;
            }
            if targets.iter().any(|s| s.file == relf && s.line == i + 1) {
                continue;
            }
            let t = line.trim();
            if t.len() > 160 {
                continue;
            }
            out.push_str(&format!("  {relf}:{}  {t}\n", i + 1));
            refs += 1;
            if refs >= 20 {
                break;
            }
        }
        if refs >= 20 {
            break;
        }
    }
    if refs == 0 {
        out.push_str("  （未发现其他引用）\n");
    }
    Ok(cut(&out, 12000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_splits_words_and_camel() {
        let t = tokenize("UserProfile router");
        assert!(t.contains(&"userprofile".to_string()));
        assert!(t.contains(&"router".to_string()));
        assert!(t.iter().any(|x| x == "user" || x == "profile"));
    }

    #[test]
    fn tokenize_drops_short() {
        let t = tokenize("a b cd x");
        assert!(t.contains(&"cd".to_string()));
        assert!(!t.contains(&"a".to_string()));
    }

    #[test]
    fn import_target_parses() {
        assert_eq!(import_target("import Foo from '../components/Foo'"), Some("../components/Foo".into()));
        assert_eq!(import_target("import { a } from \"@ohos.router\""), Some("@ohos.router".into()));
        assert_eq!(import_target("const x = 1"), None);
    }

    #[test]
    fn rules_catch_patterns() {
        let r = RULES.iter().find(|r| r.id == "hardcoded-secret").unwrap();
        assert!((r.hit)("const password = \"123456\";"));
        assert!(!(r.hit)("// const password = x;"));
        let r = RULES.iter().find(|r| r.id == "empty-catch").unwrap();
        assert!((r.hit)("  catch (e) {}"));
        let r = RULES.iter().find(|r| r.id == "any-escape").unwrap();
        assert!((r.hit)("const a: any = 1"));
    }
}
