//! 前端 ↔ 后端接口名字的一致性契约。
//!
//! 前端用**字符串**调用 IPC、用**字符串**订阅事件，编译器两侧都管不到：命令名或事件名
//! 写错时，类型检查、单测（api 层被 mock）与 Rust 测试全都会通过，直到有人在运行的应用里
//! 点那一下才暴露。设备预览面板的接线只有静态检查时就是这个处境，于是把两侧的字符串钉住：
//!
//! - 前端 `invoke*('<name>'` 调到的每个命令，必须在 `lib.rs` 的 `generate_handler!` 里注册；
//! - 前端 `listen*('<event>'` 订阅的每个事件名，必须在 Rust 源码里出现过。
//!
//! 两个方向都带**非空下限断言**：抽取逻辑本身写坏时（例如改了调用风格）测试必须失败，
//! 而不是因为「一个名字都没抽到」而空过。
//!
//! ## 本测试与 `scripts/check-tauri-events.mjs` / `scripts/check-ipc-commands.mjs` 的分工
//!
//! 本测试是**粗网**：`listen('x')` 只要**在 Rust 源码里出现过**这个字面量就算通过
//! （哪怕它只出现在注释里、只出现在 LAN 转发白名单里）。
//! 脚本门禁是**细网**：用 TypeScript 解析器读前端，并要求后端**真的 emit** 它，
//! 还额外看住 LAN 桥白名单（死转发项）。
//!
//! ⇒ 粗网漏掉的（字面量被提到但没人发）正是细网抓的；细网抓不到的
//! （前端用非字面量订阅）粗网也抓不到。**两层都要留着。**
//!
//! ⚠️ 粗网的代价：它按**文本**抽取，所以注释里的字面量会被当真引用
//! （2026-10-19 实测踩过：一条提到旧事件名的注释把本测试判红了）。
//! ⇒ 前端那一侧先过 `strip_comments()`；**后端那一侧不剥注释**——
//! Rust 的 `'` 是生命周期而非字符串，套 JS 那套屏蔽器会把代码整段吃掉。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use regex::Regex;

/// `invoke(` / `invokeWithError<泛型>(` 后紧跟的引号字面量。
/// 泛型用 `[^()]*` 匹配（可含嵌套的 `>`，如 `Record<string, unknown>`），
/// 且**必须在函数名之后紧跟泛型或括号**——早期写成「名字后允许任意非括号字符」
/// 时，`config.listen_address …{t('proxy.usage2')}` 这类跨段文本会被误当成调用。
const INVOKE_PATTERN: &str = r#"\binvoke(?:WithError)?\s*(?:<[^()]*>\s*)?\(\s*['"]([A-Za-z0-9_]+)['"]"#;
/// 事件名同理由 `listen<...>('name'` 抽取（事件名允许连字符）。
const LISTEN_PATTERN: &str = r#"\blisten\s*(?:<[^()]*>\s*)?\(\s*['"]([^'"]+)['"]"#;
/// 抽取下限：低于这些数量说明抽取规则已失效，需要修测试而不是放行。
const MIN_COMMANDS: usize = 100;
const MIN_EVENTS: usize = 10;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 应有父目录")
        .to_path_buf()
}

fn collect_by_ext(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            // 依赖与产物目录不参与（它们不在我们维护的调用面上）
            if name == "node_modules" || name == "dist" || name.starts_with('.') {
                continue;
            }
            collect_by_ext(&path, exts, out);
            continue;
        }
        if exts.iter().any(|ext| name.ends_with(ext)) {
            out.push(path);
        }
    }
}

/// 把注释内容替换成等长空格：位置不变，但注释里的 `listen('x')` 不会被当成调用点。
///
/// ## 为什么必须有这一步（2026-10-19 实测）
///
/// 本测试按**文本**抽取前端调用，注释里的字面量会被当真引用。实测踩过：
/// 一条注释里写了「此前这里还挂过一个 `listen('chat-reasoning', …)`」，
/// 于是本测试报「前端订阅了 Rust 侧不存在的字面量事件名」——而那条订阅
/// 三个批次前就已经删掉了，**注释才是唯一残留**。
///
/// ⚠️ 必须**跟踪字符串**：源码里有 `https://…` 这类字面量，
/// 朴素的「按 `//` 切到行尾」会把 URL 之后的内容整段吃掉，
/// 那会把真引用一起弄丢（假阴性）。
fn strip_comments(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out: Vec<char> = chars.clone();
    let mut i = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    while i < chars.len() {
        let ch = chars[i];
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if ch == '\'' || ch == '"' || ch == '`' {
            quote = Some(ch);
            i += 1;
            continue;
        }
        if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                out[i] = ' ';
                i += 1;
            }
            continue;
        }
        if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            let mut k = i + 2;
            while k < chars.len() {
                if chars[k] == '*' && k + 1 < chars.len() && chars[k + 1] == '/' {
                    out[k] = ' ';
                    out[k + 1] = ' ';
                    k += 2;
                    break;
                }
                if chars[k] != '\n' {
                    out[k] = ' ';
                }
                k += 1;
            }
            i = k;
            continue;
        }
        i += 1;
    }
    out.into_iter().collect()
}

fn frontend_names() -> (BTreeSet<String>, BTreeSet<String>) {
    let mut files = Vec::new();
    collect_by_ext(&repo_root().join("src"), &[".ts", ".tsx"], &mut files);
    let invoke = Regex::new(INVOKE_PATTERN).expect("调用正则应可用");
    let listen = Regex::new(LISTEN_PATTERN).expect("事件正则应可用");
    let mut commands = BTreeSet::new();
    let mut events = BTreeSet::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        // 注释里的字面量**不是**调用点（见 strip_comments 的说明）
        let text = strip_comments(&text);
        for caps in invoke.captures_iter(&text) {
            commands.insert(caps[1].to_string());
        }
        for caps in listen.captures_iter(&text) {
            events.insert(caps[1].to_string());
        }
    }
    (commands, events)
}

fn backend_sources() -> String {
    let mut files = Vec::new();
    collect_by_ext(&repo_root().join("src-tauri").join("src"), &[".rs"], &mut files);
    let mut text = String::new();
    for file in files {
        if let Ok(body) = std::fs::read_to_string(&file) {
            text.push_str(&body);
            text.push('\n');
        }
    }
    text
}

#[test]
fn every_frontend_command_is_registered() {
    let (commands, _) = frontend_names();
    assert!(
        commands.len() >= MIN_COMMANDS,
        "只抽到 {} 个调用命令，抽取规则可能已失效（下限 {MIN_COMMANDS}）",
        commands.len()
    );
    let lib = std::fs::read_to_string(repo_root().join("src-tauri/src/lib.rs")).expect("应读到 lib.rs");
    let missing: Vec<&String> = commands
        .iter()
        .filter(|name| {
            // 注册项形如 `commands::x::name,` / `services::x::name,`，按「::name 后跟逗号」判定，
            // 不解析 handler 块（块内含 `]` 的写法会截断）。
            !Regex::new(&format!(r"::\s*{}\s*,", regex::escape(name)))
                .expect("名字正则应可用")
                .is_match(&lib)
        })
        .collect();
    assert!(
        missing.is_empty(),
        "前端调用了未在 generate_handler! 注册的命令：{missing:?}"
    );
}

#[test]
fn every_frontend_event_exists_in_backend() {
    let (_, events) = frontend_names();
    assert!(
        events.len() >= MIN_EVENTS,
        "只抽到 {} 个监听事件，抽取规则可能已失效（下限 {MIN_EVENTS}）",
        events.len()
    );
    let backend = backend_sources();
    let missing: Vec<&String> = events
        .iter()
        .filter(|event| !backend.contains(&format!("\"{event}\"")))
        .collect();
    assert!(
        missing.is_empty(),
        "前端订阅了 Rust 侧不存在的字面量事件名（多半是拼写不一致）：{missing:?}"
    );
}

/// 屏蔽器自己的回归：**手写词法分析器必须有自测**，否则它坏掉时
/// 表现是「静默少抽/多抽」，和「真的没有」在报告里长得一模一样。
#[test]
fn comment_stripper_keeps_strings_and_drops_comments() {
    let src = "let a = 1; // listen('x')\n/* listen('y') */ let u = 'https://a//b';\nlisten('z')";
    let out = strip_comments(src);
    assert!(!out.contains("listen('x')"), "行注释里的调用必须被去掉");
    assert!(!out.contains("listen('y')"), "块注释里的调用必须被去掉");
    assert!(out.contains("listen('z')"), "真调用必须留下");
    assert!(
        out.contains("https://a//b"),
        "字符串里的 // 必须留下——朴素切行尾会把它之后的内容整段吃掉"
    );
    assert_eq!(out.chars().count(), src.chars().count(), "屏蔽必须保持长度不变");
}
