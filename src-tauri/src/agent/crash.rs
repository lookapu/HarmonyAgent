//! 运行时崩溃分析：部署拉起后检测存活、抓取 faultlog/hilog、提取 ArkTS/原生崩溃定位。
//!
//! 设计目标：让 Agent 能像处理构建失败一样，拿到结构化的崩溃归因（类别 + 源码定位 +
//! 推荐下一步），从而自主读文件→修复→重新部署，形成运行时闭环。

use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// 一次崩溃的结构化分析结果。
#[derive(Debug, Clone, Default, Serialize)]
pub struct CrashReport {
    /// 归因类别：arkts_exception / native_crash / permission_missing /
    /// ability_not_found / api_level / startup_timeout / unknown
    pub category: String,
    /// 面向人/模型的一句话摘要（含异常类型与消息）
    pub summary: String,
    /// 推荐的修复下一步（注入工具错误信封）
    pub advice: String,
    /// 从堆栈/faultlog 中提取的源码定位（file:line 形式，越靠前越相关）
    pub locations: Vec<String>,
    /// 原始异常类型名（如 TypeError、ReferenceError、CppCrash），无则空
    pub exception: String,
    /// 原始异常消息
    pub message: String,
    /// 截断后的相关日志（用于展示/记录）
    pub snippet: String,
}


/// 分析 faultlog（优先，结构化程度高）与 hilog -x（兜底）文本，返回结构化崩溃报告。
/// `bundle` 用于过滤与本应用相关的行。
pub fn analyze(bundle: &str, faultlog: &str, hilog: &str) -> CrashReport {
    // 1) 优先解析 faultlog 中的 JsError（ArkTS 异常）——信息最完整
    if let Some(r) = parse_js_fault(faultlog, bundle) {
        return r;
    }
    // 2) faultlog 中的 CppCrash（原生崩溃）
    if let Some(r) = parse_cpp_fault(faultlog, bundle) {
        return r;
    }
    // 3) 回退到 hilog 关键词分析
    analyze_hilog(hilog, bundle)
}

fn parse_js_fault(text: &str, bundle: &str) -> Option<CrashReport> {
    let lower_all = text.to_lowercase();
    if !lower_all.contains("jserror") && !lower_all.contains("jsfault") {
        return None;
    }
    let lower = &lower_all;
    let relevant = text
        .lines()
        .filter(|l| {
            let ll = l.to_lowercase();
            l.contains(bundle)
                || ll.contains("error")
                || ll.contains("exception")
                || ll.contains("at ")
                || ll.contains(".ets")
                || ll.contains(".ts")
                || ll.contains(".tsx")
        })
        .collect::<Vec<_>>();

    // 异常名 + 消息：形如 "TypeError: Cannot read property ..." 或 "Error message: ..."
    let (exception, message) = extract_exception_message(&relevant);

    let locations = extract_source_locations(&relevant);

    let category = classify_js_exception(&lower, &exception);
    let advice = advice_for(category).to_string();

    let summary = if message.is_empty() {
        format!("应用 {bundle} 运行时崩溃（{category}）")
    } else {
        format!("应用 {bundle} 运行时崩溃（{category}）：{}", truncate(&message, 160))
    };

    Some(CrashReport {
        category: category.to_string(),
        summary,
        advice,
        locations,
        exception,
        message,
        snippet: build_snippet(&relevant),
    })
}

fn parse_cpp_fault(text: &str, bundle: &str) -> Option<CrashReport> {
    let lower = text.to_lowercase();
    if !(lower.contains("cppcrash") || lower.contains("sigsegv") || lower.contains("sigabrt") || lower.contains("native crash")) {
        return None;
    }
    let relevant = text
        .lines()
        .filter(|l| {
            let ll = l.to_lowercase();
            l.contains(bundle)
                || ll.contains("signal")
                || ll.contains("backtrace")
                || ll.contains("#0")
                || ll.contains("abort")
                || ll.contains("lib")
        })
        .collect::<Vec<_>>();

    let signal = relevant
        .iter()
        .find_map(|l| {
            let ll = l.to_lowercase();
            for s in ["SIGSEGV", "SIGABRT", "SIGBUS", "SIGILL"] {
                if ll.contains(&s.to_lowercase()) {
                    return Some(s.to_string());
                }
            }
            None
        })
        .unwrap_or_default();

    Some(CrashReport {
        category: "native_crash".into(),
        summary: format!("应用 {bundle} 原生层崩溃{}", if signal.is_empty() { String::new() } else { format!("（{signal}）") }),
        advice: advice_for("native_crash").to_string(),
        locations: extract_native_frames(&relevant),
        exception: signal,
        message: String::new(),
        snippet: build_snippet(&relevant),
    })
}

fn analyze_hilog(hilog: &str, bundle: &str) -> CrashReport {
    let lower = hilog.to_lowercase();
    let relevant: Vec<&str> = hilog
        .lines()
        .filter(|l| {
            let ll = l.to_lowercase();
            l.contains(bundle)
                || l.contains("FATAL")
                || ll.contains("error")
                || ll.contains("exception")
                || ll.contains("crash")
                || ll.contains("sigsegv")
                || ll.contains("abort")
                || ll.contains(".ets:")
                || ll.contains(".ts:")
                || ll.contains(".tsx:")
        })
        .collect();
    let snippet_lines: Vec<&str> = relevant.iter().rev().take(30).copied().collect();
    let snippet = build_snippet_from_iter(snippet_lines.iter().rev().copied());

    let (exception, message) = extract_exception_message(&relevant);
    let category = if lower.contains("permission")
        && (lower.contains("denied") || lower.contains("not granted"))
    {
        "permission_missing"
    } else if lower.contains("sigsegv") || lower.contains("sigabrt") || lower.contains("native crash")
        || lower.contains("cppcrash") || lower.contains("tombstone")
    {
        "native_crash"
    } else if lower.contains("err_error") || lower.contains("arkts")
        || lower.contains("referenceerror") || lower.contains("typeerror")
        || lower.contains("syntaxerror") || lower.contains("rangeerror")
        || lower.contains("exception")
    {
        classify_js_exception(&lower, &exception)
    } else if lower.contains("ability")
        && (lower.contains("not found") || lower.contains("not exist") || lower.contains("class name"))
        || lower.contains("entryability")
    {
        "ability_not_found"
    } else if lower.contains("api")
        && (lower.contains("not support") || lower.contains("requires") || lower.contains("higher"))
        || lower.contains("nosuchmethod") || lower.contains("nosuchclass")
    {
        "api_level"
    } else {
        "unknown"
    };

    let locations = extract_source_locations(&relevant);
    let summary = if message.is_empty() {
        format!("应用 {bundle} 运行时崩溃（{category}）")
    } else {
        format!("应用 {bundle} 运行时崩溃（{category}）：{}", truncate(&message, 160))
    };

    CrashReport {
        category: category.to_string(),
        summary,
        advice: advice_for(category).to_string(),
        locations,
        exception,
        message,
        snippet,
    }
}

fn classify_js_exception(lower: &str, exception: &str) -> &'static str {
    let e = exception.to_lowercase();
    if e.contains("referenceerror") || lower.contains("referenceerror") {
        "arkts_reference_error"
    } else if e.contains("typeerror") || lower.contains("typeerror") {
        "arkts_type_error"
    } else if e.contains("syntaxerror") || lower.contains("syntaxerror") {
        "arkts_syntax_error"
    } else if e.contains("rangeerror") || lower.contains("rangeerror") {
        "arkts_range_error"
    } else {
        "arkts_exception"
    }
}

fn advice_for(category: &str) -> &'static str {
    match category {
        "permission_missing" =>
            "应用因权限缺失崩溃。检查 module.json5 的 requestPermissions，确认运行时权限是否在 Ability 启动时申请；定位缺失的具体权限后在代码中补充申请或在 module.json5 声明。",
        "native_crash" =>
            "原生 C/C++ 层崩溃（SIGSEGV/SIGABRT）。检查 NAPI/三方 .so 调用、空指针、数组越界；根据回溯帧定位原生库源码，必要时让用户提供完整 faultlog。不要盲目改 ArkTS 代码。",
        "arkts_type_error" | "arkts_reference_error" =>
            "ArkTS 空值/未定义访问崩溃。根据定位的 file:line 读取源文件，对可能为 undefined/null 的对象（如 getContext()、this.context、@State 初始值、异步回调结果）加判空；确认生命周期内未访问已释放资源。修复后重新构建部署验证。",
        "arkts_syntax_error" =>
            "ArkTS 语法错误。按 file:line 读取源文件修正语法（常见为缺少分号、类型标注、装饰器用法）后重新构建。",
        "arkts_range_error" =>
            "ArkTS 数组/字符串越界。按 file:line 检查下标访问与长度边界，增加边界判断后重新构建部署。",
        "arkts_exception" =>
            "ArkTS/JS 异常导致崩溃。按 file:line 读取源文件定位抛异常处，结合异常消息修复（常见为 API 调用时机错误、参数非法），修复后重新构建部署验证。",
        "ability_not_found" =>
            "Ability 未找到或配置错误。检查 module.json5 的 mainElement/abilities 配置、EntryAbility 类名与包名是否一致、是否声明了启动 Ability。",
        "api_level" =>
            "API 级别不兼容。对高于设备 API 的接口做版本判断（canIUse / try-catch）或改用兼容 API；核对 compatibleSdkVersion。",
        _ =>
            "无法从日志明确归类。用 read_logcat(package=<包名>,level=E) 抓取更完整错误栈，结合堆栈首帧定位源码。",
    }
}

/// 从相关行中提取 "异常名: 消息"。ArkTS faultlog 常见格式：
/// - `Error message: TypeError: Cannot read property ...`
/// - `Reason: TypeError: ...`
/// - 单独一行 `TypeError: Cannot read property ...`
fn extract_exception_message(lines: &[&str]) -> (String, String) {
    let mut exception = String::new();
    let mut message = String::new();
    for l in lines {
        let t = l.trim();
        // 跳过纯栈帧行
        if t.starts_with("at ") {
            continue;
        }
        // 找 "XxxError: msg" 模式
        if let Some(pos) = t.find(": ") {
            let head = &t[..pos];
            if is_exception_name(head) {
                exception = head.to_string();
                message = t[pos + 2..].trim().to_string();
                break;
            }
        }
        // "Error message: XxxError: msg" / "Reason: XxxError: msg"（可能带日志级别前缀，
        // 如 "E JsFault Error message: ..."，用 find 而非行首匹配）
        for prefix in ["Error message:", "Reason:", "Error name:"] {
            if let Some(pos) = t.find(prefix) {
                let rest = t[pos + prefix.len()..].trim();
                if let Some(pos2) = rest.find(": ") {
                    let head = &rest[..pos2];
                    if is_exception_name(head) {
                        exception = head.to_string();
                        message = rest[pos2 + 2..].trim().to_string();
                        return (exception, message);
                    }
                }
                if exception.is_empty() && is_exception_name(rest) {
                    exception = rest.to_string();
                } else if message.is_empty() {
                    message = rest.to_string();
                }
            }
        }
    }
    (exception, message)
}

fn is_exception_name(s: &str) -> bool {
    let s = s.trim();
    if s.len() < 3 || s.len() > 40 {
        return false;
    }
    let first_upper = s.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false);
    first_upper
        && s.chars().all(|c| c.is_alphanumeric())
        && (s.ends_with("Error")
            || s.ends_with("Exception")
            || s == "Error"
            || s.contains("Crash")
            || s == "TypeError"
            || s == "ReferenceError"
            || s == "SyntaxError"
            || s == "RangeError"
            || s == "EvalError"
            || s == "URIError")
}

/// 从堆栈行中提取应用源码定位（file:line）。优先 .ets/.ts 文件，排除系统/node_modules 路径。
fn extract_source_locations(lines: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for l in lines {
        // 形如：at func (file path:line:col) 或  at file://.../pages/Index.ets:24:10
        let lower = l.to_lowercase();
        if !(lower.contains(".ets:") || lower.contains(".ts:") || lower.contains(".tsx:")) {
            continue;
        }
        if lower.contains("/system/")
            || lower.contains("\\system\\")
            || lower.contains("node_modules")
            || lower.contains("oh_modules")
            || lower.contains("/ets/api/")
            || lower.contains("@ohos:")
            || lower.contains("/ets/framework/")
        {
            continue;
        }
        // 抽取 "xxx.ets(:数字)+" 片段
        if let Some(loc) = extract_file_line(l) {
            if !out.contains(&loc) {
                out.push(loc);
            }
        }
    }
    out.truncate(5);
    out
}

fn extract_file_line(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    // 找到 .ets/.ts/.tsx 的位置，然后向后取行号
    for ext in [".ets", ".tsx", ".ts"] {
        if let Some(eidx) = line.find(ext) {
            let after = eidx + ext.len();
            if after >= bytes.len() || bytes[after] != b':' {
                continue;
            }
            // 向前取文件名（到路径分隔符或空白）
            let path_start = line[..eidx]
                .rfind(|c: char| c == '/' || c == '\\' || c == '(' || c.is_whitespace())
                .map(|i| i + 1)
                .unwrap_or(0);
            // 向后取数字:数字
            let rest = &line[after + 1..];
            let line_num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if line_num.is_empty() {
                continue;
            }
            return Some(format!("{}:{line_num}", &line[path_start..after]));
        }
    }
    None
}

fn extract_native_frames(lines: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for l in lines {
        let t = l.trim();
        // 回溯帧 #0x pc ... libxxx.so (offset)
        if t.starts_with('#') && (t.contains(".so") || t.contains("lib")) {
            out.push(truncate(t, 120).to_string());
            if out.len() >= 5 {
                break;
            }
        }
    }
    out
}

fn build_snippet(lines: &[&str]) -> String {
    let iter = lines.iter().rev().take(30).copied().collect::<Vec<_>>();
    build_snippet_from_iter(iter.iter().rev().copied())
}

fn build_snippet_from_iter<'a, I: Iterator<Item = &'a str>>(iter: I) -> String {
    let mut s = String::new();
    for l in iter {
        s.push_str(l);
        s.push('\n');
    }
    truncate(&s, 2000).to_string()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

// ---------------- 历史崩溃模式（同类崩溃聚集） ----------------

/// 一个历史崩溃模式：同项目内 (category, exception, 定位文件) 相同则聚为一类。
#[derive(Clone, serde::Serialize)]
pub struct CrashPattern {
    pub category: String,
    pub exception: String,
    /// 首个定位（file:line，无则空）
    pub location: String,
    /// 累计出现次数
    pub count: usize,
    /// 最近一次出现的 unix 秒
    pub last_at: i64,
}

static HISTORY: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Vec<CrashPattern>>>> =
    std::sync::OnceLock::new();

fn history() -> std::sync::MutexGuard<'static, std::collections::HashMap<String, Vec<CrashPattern>>> {
    HISTORY
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// 每项目最多保留的模式数（超出丢弃最旧，防内存膨胀）
const MAX_PATTERNS_PER_PROJECT: usize = 20;

/// 记录一次崩溃并返回该模式的历史计数（含本次）；
/// 返回 >1 表示同类崩溃已反复出现，调用方可提示模型参考既往修复经验。
pub fn record_pattern(project_key: &str, report: &CrashReport) -> usize {
    let mut h = history();
    let list = h.entry(project_key.to_string()).or_default();
    let loc = report.locations.first().cloned().unwrap_or_default();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    match list.iter_mut().find(|p| {
        p.category == report.category && p.exception == report.exception && p.location == loc
    }) {
        Some(p) => {
            p.count += 1;
            p.last_at = now;
            p.count
        }
        None => {
            list.push(CrashPattern {
                category: report.category.clone(),
                exception: report.exception.clone(),
                location: loc,
                count: 1,
                last_at: now,
            });
            if list.len() > MAX_PATTERNS_PER_PROJECT {
                // 丢弃最久未出现的模式
                list.sort_by_key(|p| p.last_at);
                list.drain(0..list.len() - MAX_PATTERNS_PER_PROJECT);
            }
            1
        }
    }
}

/// 查询某项目的历史崩溃模式（按出现次数倒序，同次按最近时间倒序）。
pub fn patterns(project_key: &str) -> Vec<CrashPattern> {
    let mut list = history().get(project_key).cloned().unwrap_or_default();
    list.sort_by(|a, b| b.count.cmp(&a.count).then(b.last_at.cmp(&a.last_at)));
    list
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn pattern_groups_same_crash() {
        let mk = |cat: &str, ex: &str, loc: &str| CrashReport {
            category: cat.into(),
            exception: ex.into(),
            locations: if loc.is_empty() { Vec::new() } else { vec![loc.into()] },
            ..Default::default()
        };
        assert_eq!(record_pattern("proj-a", &mk("arkts_type_error", "TypeError", "a.ets:1")), 1);
        assert_eq!(record_pattern("proj-a", &mk("arkts_type_error", "TypeError", "a.ets:1")), 2);
        // 同类别不同位置：另一类
        assert_eq!(record_pattern("proj-a", &mk("arkts_type_error", "TypeError", "b.ets:9")), 1);
        let ps = patterns("proj-a");
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[0].count, 2);
        assert_eq!(ps[0].location, "a.ets:1");
        // 项目隔离
        assert!(patterns("proj-b").is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_arkts_typeerror_with_location() {
        let log = "E JsFault Error message: TypeError: Cannot read property 'name' of undefined\n\
                   at onPageShow (entry/src/main/ets/pages/Index.ets:42:18)\n\
                   at aboutToAppear (entry/src/main/ets/pages/Index.ets:30:5)";
        let r = analyze("com.demo.app", log, "");
        assert_eq!(r.category, "arkts_type_error", "cat={}", r.category);
        assert!(r.message.contains("Cannot read property"));
        assert!(r.locations.iter().any(|l| l.contains("Index.ets:42")), "locs={:?}", r.locations);
    }

    #[test]
    fn classifies_reference_error() {
        let log = "E FATAL com.demo.app ReferenceError: foo is not defined\n\
                   at build (entry/src/main/ets/pages/Home.ets:88:3)";
        let r = analyze("com.demo.app", "", log);
        assert_eq!(r.category, "arkts_reference_error");
        assert!(r.locations.iter().any(|l| l.contains("Home.ets:88")));
    }

    #[test]
    fn classifies_native_crash() {
        let log = "E CppCrash signal: SIGSEGV\n#0 pc 000123 libentry.so (Foo::bar()+20)";
        let r = analyze("com.demo.app", log, "");
        assert_eq!(r.category, "native_crash");
        assert!(r.exception.contains("SIGSEGV"));
    }

    #[test]
    fn ignores_system_stack_frames() {
        let line = "    at gen (\\system\\...\\ets\\framework\\native_node.js:12:3)";
        let locs = extract_source_locations(&[line]);
        assert!(locs.is_empty(), "should skip system path, got {:?}", locs);
    }
}
