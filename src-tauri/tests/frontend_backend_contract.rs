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
