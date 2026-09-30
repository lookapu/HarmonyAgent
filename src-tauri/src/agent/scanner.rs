//! 分级代码扫描族：静态检查 / 深度扫描 / 全库检索 / 符号详情
//!
//! 对标主流 Agent 的扫描分层能力：
//! - `check_code`      静态检查（规则式 lint）：调试残留 / TODO / 硬编码密钥 / 空 catch /
//!   类型逃逸 / 明文 http 等，返回 file:line + 规则 + 建议
//! - `deep_scan`       深度扫描：全库结构 + 质量报告（扩展名分布 / 大文件 / 符号密度 /
//!   import 依赖拓扑 / 疑似死代码候选）
//! - `codebase_search` 全库混合检索：符号名 + 路径 + 内容行三路匹配打分排序（无需向量库）
//! - `get_symbol_details` 符号详情：定义信息 + 前置注释 + 全库引用位置反查
//!
//! 设计取舍：全部基于进程内轻量扫描（复用 symbol_index 缓存），不引入外部 LSP/索引服务；
//! 输出统一截断护栏，避免挤占模型上下文预算。

use std::path::Path;

/// 需要跳过的目录（与 tools.rs 保持同一清单，避免扫描依赖/产物/工具自身数据）
pub const SKIP_DIRS: [&str; 15] = [
    ".git", ".hvigor", ".idea", ".ohpm", "node_modules", "oh_modules", "build", ".arkui-x",
    ".deveco-agent", "dist", "target", ".venv", "coverage", ".cxx", ".preview",
];

fn should_skip_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
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

/// 收集源码文件的结果：文件列表 + **没能收集到**的部分。
///
/// 原实现只回 `Vec<PathBuf>`，读不到的目录、读失败的目录条目、取不到元数据的文件
/// 三种丢失全部静默。`check_code` 随后会拿这份残缺清单得出「未发现规则命中，
/// 代码整体较整洁」——**结论的覆盖面小于它字面声称的范围**，而调用方无从分辨。
struct CollectedSrcFiles {
    files: Vec<std::path::PathBuf>,
    /// 整棵子树读不到的目录（`read_dir` 失败 → 递归直接 return）
    unreadable_dirs: Vec<String>,
    /// 取不到元数据、因而被跳过的文件
    metadata_failed: usize,
}

/// check_code 单次扫描的文件数上限。超出部分计入覆盖缺口并在报告里披露。
const SCAN_FILE_CAP: usize = 300;
/// secret_scan 源码文件单次扫描上限。
const SECRET_FILE_CAP: usize = 500;
/// secret_scan 配置文件单次扫描上限。
const SECRET_CONF_CAP: usize = 200;

/// 覆盖率不完整时写入输出的稳定标记。
/// `verification_planner::completion` 读它来决定 check_code 是否算通过验证——
/// **「没看到高危」不等于「没有高危」**，读不到的文件上同样看不到。
pub const SCAN_INCOMPLETE: &str = "⚠️ 扫描覆盖不完整";

/// 一次扫描的覆盖缺口。check_code 与 secret_scan 共用同一套披露口径——
/// 格式若各写一份，改一处必然漏另一处。
/// 公开是因为**第三处**也需要它：`services::harmony_consistency` 自己的遍历策略
/// （深度上限 12 + 跳符号链接）与 `walk_filtered` 不同，不能强行换掉；
/// 但**缺口的枚举与措辞必须同一份**，否则「扫描覆盖不完整」这句话又会分叉。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CoverageGaps {
    pub unreadable_dirs: Vec<String>,
    pub metadata_failed: usize,
    pub read_failed: usize,
    pub dropped_by_cap: usize,
}

impl CoverageGaps {
    pub fn is_empty(&self) -> bool {
        self.unreadable_dirs.is_empty()
            && self.metadata_failed == 0
            && self.read_failed == 0
            && self.dropped_by_cap == 0
    }

    /// 缺口的**统一描述**（单一真源）。只说「漏了什么」，不替调用方下结论——
    /// 「无命中」与「未发现不一致」是不同的结论，措辞不能共用，由各消费方自配。
    pub fn describe(&self, cap: usize) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut gaps: Vec<String> = Vec::new();
        if !self.unreadable_dirs.is_empty() {
            gaps.push(format!(
                "{} 个目录读不到（{}）",
                self.unreadable_dirs.len(),
                self.unreadable_dirs.join("、")
            ));
        }
        if self.read_failed > 0 {
            gaps.push(format!("{} 个文件读不到内容", self.read_failed));
        }
        if self.metadata_failed > 0 {
            gaps.push(format!("{} 个文件取不到元数据", self.metadata_failed));
        }
        if self.dropped_by_cap > 0 {
            gaps.push(format!("{} 个文件超过单次 {cap} 上限未扫描", self.dropped_by_cap));
        }
        Some(gaps.join("；"))
    }

    /// 渲染披露段落；无缺口返回 None。
    fn render(&self, cap: usize) -> Option<String> {
        self.describe(cap).map(|d| {
            format!(
                "{SCAN_INCOMPLETE}：{d}\n本次「无命中」结论**不覆盖**以上文件，修复访问权限或分目录重扫后才能作为干净结论。\n"
            )
        })
    }
}

/// 递归收集源码文件（跳过忽略目录与超大文件），并记录未能收集到的部分
fn collect_src_files(root: &Path, max_size: u64) -> CollectedSrcFiles {
    let mut out = CollectedSrcFiles {
        files: Vec::new(),
        unreadable_dirs: Vec::new(),
        metadata_failed: 0,
    };
    walk_filtered(root, max_size, is_src_file, &mut out);
    out
}

/// 遍历 + 收集的唯一实现。`keep` 决定哪些文件名算数。
///
/// 原先有 `walk` 与 `walk_conf` 两份几乎相同的拷贝，`keep` 分别是
/// `is_src_file` 与 `is_secret_config_file`；修好 `walk` 之后 `walk_conf`
/// 仍是旧形态（静默丢目录 / 元数据失败混成「过大」）——**修完一处 ≠ 修完这条链**。
fn walk_filtered(dir: &Path, max_size: u64, keep: fn(&str) -> bool, out: &mut CollectedSrcFiles) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        // 原先 `else { return }` 静默放弃整棵子树
        out.unreadable_dirs.push(dir.display().to_string());
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if !should_skip_dir(&name) {
                walk_filtered(&p, max_size, keep, out);
            }
        } else if keep(&name) {
            // 元数据取不到 ≠ 文件过大：原先 unwrap_or(false) 把两类混成「跳过」
            match e.metadata() {
                Ok(meta) if meta.len() <= max_size => out.files.push(p),
                Ok(_) => {}
                Err(_) => out.metadata_failed += 1,
            }
        }
    }
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
    let collected = collect_src_files(&scan_root, 512 * 1024);
    if collected.files.is_empty() {
        // 目录读不到与「确实没有源码文件」不是一回事，照旧要说清楚
        let extra = if collected.unreadable_dirs.is_empty() {
            String::new()
        } else {
            format!(
                "（另有 {} 个目录读不到：{}）",
                collected.unreadable_dirs.len(),
                collected.unreadable_dirs.join("、")
            )
        };
        return Ok(format!("未发现可扫描的源码文件（.ets/.ts/.js 等）{extra}"));
    }
    // 每个规则聚合命中（文件+行号+行内容），限制总量防输出爆炸
    let mut by_rule: Vec<(&'static Rule, Vec<Hit>)> = RULES.iter().map(|r| (r, Vec::new())).collect();
    let mut scanned = 0usize;
    // 读不到内容的文件：原先 `let Ok(text) = … else { continue }` 静默跳过，
    // 而 scanned 已在 continue 之前自增——**读不到的文件被算成「已扫描」**，
    // 于是「扫描 N 个文件」高估了真实覆盖面，「未发现规则命中」也高估了结论强度。
    let mut read_failed = 0usize;
    let selected: Vec<&std::path::PathBuf> =
        collected.files.iter().take(SCAN_FILE_CAP).collect();
    let dropped_by_cap = collected.files.len().saturating_sub(selected.len());
    for f in &selected {
        scanned += 1;
        let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("");
        if kind_arkts && ext != "ets" && ext != "ts" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(f) else {
            read_failed += 1;
            continue;
        };
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
    // 覆盖率不完整 → 结论打折。**「没看到高危」不等于「没有高危」**：
    // 读不到的文件上同样看不到高危规则命中。
    let gaps = CoverageGaps {
        unreadable_dirs: collected.unreadable_dirs.clone(),
        metadata_failed: collected.metadata_failed,
        read_failed,
        dropped_by_cap,
    };
    if let Some(disclosure) = gaps.render(SCAN_FILE_CAP) {
        out.push_str(&disclosure);
    }
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
        if gaps.is_empty() {
            out.push_str("\n未发现规则命中，代码整体较整洁。");
        } else {
            // 覆盖不全时不能说「整洁」——没扫到的地方同样可能是脏的
            out.push_str("\n已扫描到的文件中未发现规则命中（覆盖不完整，见上）。");
        }
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
    let files = collect_src_files(&scan_root, 1024 * 1024).files;
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
    sized.sort_by_key(|a| std::cmp::Reverse(a.1));

    let mut out = String::new();
    out.push_str(&format!(
        "深度扫描报告（{}）\n源码文件 {} 个，共 {} 行。\n",
        scan_root.display(),
        files.len(),
        total_lines
    ));
    let mut exts: Vec<_> = ext_lines.into_iter().collect();
    exts.sort_by_key(|a| std::cmp::Reverse(a.1 .1));
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
    kinds.sort_by_key(|a| std::cmp::Reverse(a.1));
    out.push_str(&format!("\n符号索引：共 {} 个符号。\n", syms.len()));
    for (k, n) in kinds.iter().take(10) {
        out.push_str(&format!("  {k}: {n}\n"));
    }
    let mut dense: Vec<_> = by_file.into_iter().collect();
    dense.sort_by_key(|a| std::cmp::Reverse(a.1));
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
    for deps in imports_of.values() {
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
    hot.sort_by_key(|a| std::cmp::Reverse(a.1));
    out.push_str("\n被引用最多的模块（Top 10）：\n");
    for (f, n) in hot.iter().take(10) {
        out.push_str(&format!("  {n} 次引用  {f}\n"));
    }
    let mut outdegree: Vec<_> = imports_of.iter().map(|(f, d)| (f, d.len())).collect();
    outdegree.sort_by_key(|a| std::cmp::Reverse(a.1));
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
    'outer: for f in collect_src_files(root, 256 * 1024).files.iter().take(400) {
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
    ranked.sort_by_key(|a| std::cmp::Reverse(a.1 .0));
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
    for f in collect_src_files(root, 256 * 1024).files.iter().take(400) {
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

// ==================== secret_scan：密钥泄露专项扫描 ====================

/// 配置类文件名（白名单），扫描密钥时额外纳入：.env 族 / properties / 密钥证书文件
fn is_secret_config_file(name: &str) -> bool {
    let n = name.to_lowercase();
    n == ".env"
        || n.starts_with(".env.")
        || n == "local.properties"
        || n.ends_with(".properties")
        || n == ".npmrc"
        || n == ".pypirc"
        || n == ".netrc"
        || n.ends_with(".pem")
        || n.ends_with(".key")
        || n.ends_with(".p12")
        || n.ends_with(".keystore")
        || n.ends_with(".jks")
        || n.ends_with(".bks")
}

/// 对疑似密钥值做掩码展示（只保留前 2 字符 + 长度），防止扫描结果自身泄露明文
fn mask_value(v: &str) -> String {
    let v = v.trim().trim_matches('"').trim_matches('\'');
    let chars: Vec<char> = v.chars().collect();
    if chars.len() <= 2 {
        return "***".into();
    }
    format!("{}***（长度 {}）", chars[..2].iter().collect::<String>(), chars.len())
}

/// 配置文件中提取键值对（兼容 KEY=value / KEY: value / KEY value）
fn parse_kv(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    if t.is_empty() || t.starts_with('#') || t.starts_with(';') || t.starts_with("//") {
        return None;
    }
    let pos = t.find(['=', ':'])?;
    let key = t[..pos].trim().trim_matches('"').trim_matches('\'');
    if key.is_empty() {
        return None;
    }
    let mut val = t[pos + 1..].trim();
    // 行尾注释剥离（# 前有空格视为注释）
    if let Some(c) = val.find(" #") {
        val = val[..c].trim();
    }
    if val.is_empty() {
        return None;
    }
    Some((key.to_string(), val.to_string()))
}

/// 配置值是否像真实密钥（排除空壳占位符：xxx/your-/<...>/${...} 引用/纯数字短串）
fn looks_real_secret(k: &str, v: &str) -> bool {
    let kl = k.to_lowercase();
    let sensitive = kl.contains("password")
        || kl.contains("passwd")
        || kl.contains("secret")
        || kl.contains("api_key")
        || kl.contains("apikey")
        || kl.contains("token")
        || kl.contains("credential")
        || kl.contains("access_key")
        || kl.contains("private_key")
        || kl.contains("client_secret")
        || kl.contains("auth")
        || kl == "key"
        || kl == "pwd";
    if !sensitive {
        return false;
    }
    let vl = v.to_lowercase();
    if vl.is_empty()
        || vl.contains("xxx")
        || vl.contains("your_")
        || vl.contains("your-")
        || vl.contains("<")
        || vl.contains(">")
        || vl.contains("placeholder")
        || vl.contains("example")
        || vl.starts_with("${")
        || vl.starts_with("\\$")
    {
        return false;
    }
    // 纯数字且长度 ≤ 4：多半是端口/版本号等误报
    !(v.chars().all(|c| c.is_ascii_digit()) && v.len() <= 4)
}

/// 密钥泄露专项扫描：源码（复用 hardcoded-secret 规则）+ 配置类文件（.env/local.properties 等）。
/// 输出命中文件:行 + 键名 + 掩码值；源码每文件每类最多 3 条，配置文件最多 5 条。
/// include_config=false 时只扫源码（默认 true 一并扫配置文件）。
pub fn secret_scan(
    root: &Path,
    path: Option<&str>,
    include_config: Option<bool>,
) -> Result<String, String> {
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
    let rule = RULES.iter().find(|r| r.id == "hardcoded-secret").unwrap();
    let mut hits: Vec<(String, usize, String)> = Vec::new(); // (file, line, 掩码文本)
    let mut scanned_files = 0usize;
    // 覆盖缺口：与 check_code 同一套口径。secret_scan 是 git_commit 前的推荐
    // 扫描（capabilities.rs 的 commit 流程里），「安全状况良好」这句话若建立在
    // 一份漏了文件的清单上，等于发了一张覆盖面不足的合格证。
    let mut read_failed = 0usize;
    let mut dropped_by_cap = 0usize;
    let mut unreadable_dirs: Vec<String> = Vec::new();
    let mut metadata_failed = 0usize;

    // 1) 源码文件：复用 hardcoded-secret 规则
    let src_collected = collect_src_files(&scan_root, 512 * 1024);
    unreadable_dirs.extend(src_collected.unreadable_dirs.iter().cloned());
    metadata_failed += src_collected.metadata_failed;
    let src_selected: Vec<&std::path::PathBuf> =
        src_collected.files.iter().take(SECRET_FILE_CAP).collect();
    dropped_by_cap += src_collected.files.len().saturating_sub(src_selected.len());
    for f in &src_selected {
        scanned_files += 1;
        let Ok(text) = std::fs::read_to_string(f) else {
            // 计数器已在上面自增：读不到的文件原先被算成「已检查」
            read_failed += 1;
            continue;
        };
        let relf = rel(root, f);
        let mut n = 0usize;
        for (i, line) in text.lines().enumerate() {
            if n >= 3 {
                break;
            }
            if (rule.hit)(line) {
                // 掩码赋值右侧的值：截取 = / : 后片段
                let shown = if let Some(pos) = line.find(['=', ':']) {
                    let v = line[pos + 1..].trim();
                    format!("{} {}", line[..=pos].trim(), mask_value(v))
                } else {
                    line.trim().to_string()
                };
                hits.push((relf.clone(), i + 1, shown));
                n += 1;
            }
        }
    }

    // 2) 配置文件：白名单名 + 键值模式检测
    let mut conf_hits: Vec<(String, usize, String)> = Vec::new();
    if include_config.unwrap_or(true) {
        // 与源码扫描共用 walk_filtered，不再各写一份遍历
        let mut conf_collected = CollectedSrcFiles {
            files: Vec::new(),
            unreadable_dirs: Vec::new(),
            metadata_failed: 0,
        };
        walk_filtered(&scan_root, 512 * 1024, is_secret_config_file, &mut conf_collected);
        unreadable_dirs.extend(conf_collected.unreadable_dirs.iter().cloned());
        metadata_failed += conf_collected.metadata_failed;
        let conf_selected: Vec<&std::path::PathBuf> =
            conf_collected.files.iter().take(SECRET_CONF_CAP).collect();
        dropped_by_cap += conf_collected.files.len().saturating_sub(conf_selected.len());
        for f in &conf_selected {
            scanned_files += 1;
            let Ok(text) = std::fs::read_to_string(f) else {
                read_failed += 1;
                continue;
            };
            let relf = rel(root, f);
            let mut n = 0usize;
            for (i, line) in text.lines().enumerate() {
                if n >= 5 {
                    break;
                }
                if let Some((k, v)) = parse_kv(line) {
                    if looks_real_secret(&k, &v) {
                        conf_hits.push((relf.clone(), i + 1, format!("{} = {}", k, mask_value(&v))));
                        n += 1;
                    }
                }
            }
        }
    }

    let total = hits.len() + conf_hits.len();
    let mut out = String::new();
    out.push_str(&format!(
        "密钥泄露扫描完成：检查 {} 个文件，发现 {} 处疑似敏感信息。\n（值已掩码，仅保留前 2 字符 + 长度；确认后应立即改用环境变量/配置中心）\n",
        scanned_files, total
    ));
    if !hits.is_empty() {
        out.push_str(&format!("\n## 源码硬编码（高危）\n{} 处\n", hits.len()));
        for (f, l, t) in hits.iter().take(30) {
            out.push_str(&format!("  {f}:{l}  {t}\n"));
        }
        if hits.len() > 30 {
            out.push_str(&format!("  …另 {} 处\n", hits.len() - 30));
        }
    }
    if !conf_hits.is_empty() {
        out.push_str(&format!("\n## 配置文件疑似密钥（高危）\n{} 处\n", conf_hits.len()));
        for (f, l, t) in conf_hits.iter().take(30) {
            out.push_str(&format!("  {f}:{l}  {t}\n"));
        }
        if conf_hits.len() > 30 {
            out.push_str(&format!("  …另 {} 处\n", conf_hits.len() - 30));
        }
    }
    if total == 0 {
        let gaps = CoverageGaps {
            unreadable_dirs,
            metadata_failed,
            read_failed,
            dropped_by_cap,
        };
        if gaps.is_empty() {
            out.push_str("\n未发现疑似密钥，安全状况良好。");
        } else {
            if let Some(disclosure) = gaps.render(SECRET_FILE_CAP) {
                out.push('\n');
                out.push_str(&disclosure);
            }
            out.push_str("\n已检查到的文件中未发现疑似密钥（覆盖不完整，见上）。");
        }
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
