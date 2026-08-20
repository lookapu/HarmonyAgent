//! Windows 子进程启动辅助（MCP 服务器 / git 等外部命令共用）。
//!
//! 解决三类问题：
//! 1. `Command::new("npx")` 在 Windows 上找不到 `npx.cmd`（CreateProcess 只认 .exe，
//!    PATH 里只有 .cmd 脚本时直接报 program not found）——这里按 PATHEXT 显式查找，
//!    命中 .cmd/.bat 时用 `cmd.exe /C` 包装执行。
//! 2. 子进程弹出黑色控制台窗口——统一附加 `CREATE_NO_WINDOW` 标志。
//! 3. node/npm/npx 优先使用内置捆绑的 Node 便携版（node.exe 直调 / npx、npm 经
//!    `node.exe <cli.js> 参数` 启动），完全绕开系统 Node——避免无扩展名 npx sh 脚本
//!    （os error 193）、.cmd 批处理兼容性及系统版本差异问题；内置缺失时才回退系统 PATH。
//!
//! 用法：`let cmd = crate::utils::process::command("npx", &["-y", "redis-mcp"])?;`
//! 返回配置好的 `tokio::process::Command`，找不到程序时返回带建议的错误。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// CREATE_NO_WINDOW：子进程不创建控制台窗口（Windows）
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// App 是否持有（隐藏的）控制台。GUI 双击运行时无控制台，需 AllocConsole 创建一个隐藏控制台，
/// 使 hvigor/ohpm 等命令行工具的子进程、孙进程（hvigor worker / node 编译进程等）
/// 继承隐藏控制台而非新建窗口——这是消除“构建时弹出 cmd 窗口”的根本手段。
static APP_HAS_CONSOLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 初始化隐藏控制台（lib.rs setup 时调用一次）：
/// - 无控制台（GUI 双击启动）→ AllocConsole 创建并立即隐藏
/// - 已有控制台（终端/调试启动）→ 直接标记，子进程继承终端控制台也不会弹窗
/// 之后 Windows 子进程不再加 CREATE_NO_WINDOW（它会断开控制台继承链，导致孙进程新建窗口）。
#[cfg(windows)]
pub fn init_hidden_console() {
    use windows_sys::Win32::System::Console::{AllocConsole, GetConsoleWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
    unsafe {
        if GetConsoleWindow().is_null() {
            // AllocConsole 失败（如无交互桌面）时保持无控制台，子进程仍走 CREATE_NO_WINDOW
            if AllocConsole() == 0 {
                return;
            }
        }
        let hwnd = GetConsoleWindow();
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_HIDE);
            APP_HAS_CONSOLE.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// 子进程是否应继承 App 控制台（有隐藏/终端控制台时为 true，则不加 CREATE_NO_WINDOW）
#[cfg(windows)]
fn inherit_console() -> bool {
    APP_HAS_CONSOLE.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(not(windows))]
pub fn init_hidden_console() {}

/// 内置 Node 运行时目录（升级版优先于出厂捆绑版）；None 表示未初始化/无内置
static BUNDLED_NODE_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// 注册内置 Node 运行时目录（lib.rs setup 时调用）
pub fn set_bundled_node_dir(dir: Option<PathBuf>) {
    *BUNDLED_NODE_DIR.lock().unwrap() = dir;
}

/// 内置 Git 运行时目录（升级版优先于出厂捆绑版）；None 表示未初始化/无内置
static BUNDLED_GIT_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// 注册内置 Git 运行时目录（lib.rs setup 时调用）
pub fn set_bundled_git_dir(dir: Option<PathBuf>) {
    *BUNDLED_GIT_DIR.lock().unwrap() = dir;
}

/// 鸿蒙工具链额外 PATH 目录（command-line-tools/bin、hdc 所在目录等）。
/// 环境探测后注册，使 `process::command("hdc", ...)` 在 hdc 未进系统 PATH 时也能命中。
static HARMONY_EXTRA_PATH: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// 注册鸿蒙工具链 PATH 目录（覆盖设置）
pub fn set_harmony_path_dirs(dirs: Vec<PathBuf>) {
    *HARMONY_EXTRA_PATH.lock().unwrap() = dirs;
}

/// 当前注册的鸿蒙工具链额外 PATH 目录（内置终端等构造子进程环境时注入 PATH，
/// 使 hdc/ohpm 未进系统 PATH 时终端命令也能命中）。
pub fn extra_path_dirs() -> Vec<PathBuf> {
    HARMONY_EXTRA_PATH.lock().unwrap().clone()
}

/// 内置默认 JDK 目录（jdk_runtime 默认版本或出厂捆绑版）；None 表示未初始化/无内置。
/// 系统已设置 JAVA_HOME 时注入跳过（以系统为准）；否则自动注入 JAVA_HOME 并前置 bin 到 PATH，
/// 使 hvigor 构建 / java 命令在无系统 JDK 时也能工作。
static DEFAULT_JDK_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// 注册内置默认 JDK 目录（lib.rs setup 时调用）
pub fn set_default_jdk_dir(dir: Option<PathBuf>) {
    *DEFAULT_JDK_DIR.lock().unwrap() = dir;
}

/// 计算 JDK 环境覆盖（系统已有 JDK 时返回空，尊重用户环境）：
/// - 系统 JAVA_HOME 已存在 → 跳过
/// - 系统 PATH 已有 java.exe → 跳过
/// 否则返回 (key, value) 对：JAVA_HOME + 前置 `<jdk>/bin` 的 PATH。
pub fn jdk_env_overrides() -> Vec<(String, String)> {
    if std::env::var_os("JAVA_HOME").is_some() || system_path_has_java() {
        return Vec::new();
    }
    let Some(dir) = DEFAULT_JDK_DIR.lock().unwrap().clone() else {
        return Vec::new();
    };
    let mut overrides = vec![("JAVA_HOME".to_string(), dir.to_string_lossy().to_string())];
    // PATH 前插 <jdk>/bin，使子进程可直接解析 java.exe
    let bin = dir.join("bin");
    let cur = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<std::ffi::OsString> = vec![bin.into_os_string()];
    paths.extend(std::env::split_paths(&cur).map(|p| p.into_os_string()));
    if let Ok(joined) = std::env::join_paths(paths) {
        overrides.push(("PATH".to_string(), joined.to_string_lossy().to_string()));
    }
    overrides
}

/// 系统 PATH 中是否存在 java.exe（存在时视为系统已有 JDK，不注入内置）
fn system_path_has_java() -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|d| d.join("java.exe").is_file())
}

/// App 专属 npm 缓存根目录（None 表示未注册）。
/// 每个 MCP 服务器一个独立子目录：App 常驻连接与手动测试的多个 npx 并发执行时，
/// 若共用系统全局缓存（%LOCALAPPDATA%\npm-cache），Windows 文件锁/Defender 扫描
/// 会相互踩踏导致 EPERM（npm error code EPERM, _cacache\tmp 打开失败）。
static MCP_NPM_CACHE_ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// 注册 App 专属 npm 缓存根目录（lib.rs setup 时调用）
pub fn set_mcp_npm_cache_root(dir: PathBuf) {
    *MCP_NPM_CACHE_ROOT.lock().unwrap() = Some(dir);
}

/// 是否为 npx/npm 类命令：这类命令首次运行会通过 npm 下载包并写缓存，
/// 需要独立的 npm_config_cache 避免并发写冲突；其他命令（python/docker/node 直跑）不受影响。
fn is_npx_like(program: &str) -> bool {
    let base = Path::new(program)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    let base = base
        .strip_suffix(".cmd")
        .or_else(|| base.strip_suffix(".bat"))
        .or_else(|| base.strip_suffix(".exe"))
        .or_else(|| base.strip_suffix(".cjs"))
        .unwrap_or(&base);
    base == "npx" || base == "npm" || base == "npm-exec"
}

/// MCP 子进程公共环境（测试连接与常驻连接统一调用）：
/// 1. 移除 NODE_TLS_REJECT_UNAUTHORIZED：用户系统级设置会被子进程继承，导致 node
///    每次启动输出警告（干扰诊断）且关闭 TLS 证书校验（安全风险），不应传染给 MCP 服务器。
/// 2. npx/npm 命令注入按服务器隔离的 npm 缓存目录（App 数据目录下），
///    避免多进程并发写全局 npm 缓存时的 Windows 文件锁冲突（EPERM）。
pub fn apply_mcp_child_env(
    cmd: &mut tokio::process::Command,
    program: &str,
    server_id: &str,
) -> Result<(), String> {
    cmd.env_remove("NODE_TLS_REJECT_UNAUTHORIZED");
    cmd.env_remove("node_tls_reject_unauthorized");
    if is_npx_like(program) {
        if let Some(root) = MCP_NPM_CACHE_ROOT.lock().unwrap().clone() {
            // 服务器 id 归一化后作子目录名（uuid 本身已安全，防御性清洗）
            let safe: String = server_id
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .take(64)
                .collect();
            let dir = root.join(if safe.is_empty() { "default".into() } else { safe });
            std::fs::create_dir_all(&dir).map_err(|e| format!("创建 npm 缓存目录失败: {e}"))?;
            cmd.env("npm_config_cache", dir);
        }
    }
    Ok(())
}

/// 程序解析结果：最终可执行程序 + 是否需要 cmd 包装 + node CLI 脚本
struct Resolved {
    program: PathBuf,
    /// true 时需以 `cmd.exe /C` 启动（.cmd/.bat 脚本）
    needs_cmd_wrap: bool,
    /// 非空时表示应执行 `node.exe <node_cli> args`（内置 npx/npm 直调，绕开 .cmd）
    node_cli: Option<PathBuf>,
}

/// 判断文件是否为有效的 Windows PE 可执行程序（文件头 MZ 魔术字）。
/// 无扩展名文件可能是 shell 脚本（如 Node 自带的 npx/npm），直接 CreateProcess
/// 会报"不是有效的 Win32 应用程序 (os error 193)"，必须先做 PE 校验。
fn is_pe_executable(p: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(p) else {
        return false;
    };
    let mut buf = [0u8; 2];
    f.read_exact(&mut buf).is_ok() && buf == [b'M', b'Z']
}

/// 显式查找可执行程序：
/// - 含路径分隔符：直接检查该路径（不存在时尝试补 PATHEXT 扩展名）
/// - node/npm/npx（纯名字）：内置 Node 运行时优先（node.exe 直调 / npx、npm 经
///   `node.exe <cli.js>` 启动），完全绕开系统 Node，内置缺失才回退系统 PATH
/// - 其他纯名字：遍历 PATH 目录，按 PATHEXT 顺序尝试（exe -> cmd -> bat）
/// - 无扩展名文件需通过 PE 校验（sh 脚本直接执行会报 os error 193），
///   未通过则跳过继续找 .cmd/.bat
fn resolve_program(program: &str) -> Option<Resolved> {
    if program.is_empty() {
        return None;
    }
    if program.contains('/') || program.contains('\\') {
        let p = Path::new(program);
        if p.is_file() {
            let wrap = is_script_ext(p);
            // 脚本（.cmd/.bat）可 cmd 包装；PE 可执行直接运行；
            // 其他（无扩展名 sh 脚本等）非有效 Win32 程序，交给补扩展名分支再试
            if wrap || is_pe_executable(p) {
                return Some(Resolved { program: p.to_path_buf(), needs_cmd_wrap: wrap, node_cli: None });
            }
        }
        // 带路径但无扩展名：补 .exe/.cmd/.bat 再试
        for ext in ["exe", "cmd", "bat"] {
            let cand = PathBuf::from(format!("{program}.{ext}"));
            if cand.is_file() {
                return Some(Resolved { program: cand, needs_cmd_wrap: ext != "exe", node_cli: None });
            }
        }
        return None;
    }

    // node/npm/npx 优先解析内置 Node 运行时：node.exe 直接运行；npx/npm 经
    // `node.exe <cli.js> 参数` 启动，不依赖系统 Node 的 sh 脚本/.cmd 包装，
    // 也避免系统 Node 版本差异导致的不兼容。内置未初始化时返回 None 回退系统 PATH。
    if let Some(r) = resolve_bundled_node(program) {
        return Some(r);
    }

    // git 优先解析内置 Git 运行时（cmd\git.exe 直调），保证未装系统 Git 时
    // 分支工作流 / 文档下载 / git 面板仍可用；内置未初始化时回退系统 PATH。
    if let Some(r) = resolve_bundled_git(program) {
        return Some(r);
    }

    // java/javac 优先解析内置默认 JDK（无系统 JDK 时兑底）
    if let Some(r) = resolve_bundled_jdk(program) {
        return Some(r);
    }

    // ohpm 直调：ohpm.bat 链最终是 `node pm-cli.js`，直接 node 直调绕开 .bat 链
    // （避免 cmd /C 参数错乱与子进程黑框，也不依赖 DevEco 自带 node）；
    // 找不到 pm-cli.js 或内置 node 时回退到普通 .cmd/.bat 包装。
    if let Some(r) = resolve_ohpm_direct(program) {
        return Some(r);
    }

    // 鸿蒙工具链额外 PATH（command-line-tools/bin 等）：hdc/ohpm 未进系统 PATH 时也能命中
    for dir in HARMONY_EXTRA_PATH.lock().unwrap().iter() {
        let exact = dir.join(program);
        if exact.is_file() {
            if is_script_ext(&exact) {
                return Some(Resolved { program: exact, needs_cmd_wrap: true, node_cli: None });
            }
            if is_pe_executable(&exact) {
                return Some(Resolved { program: exact, needs_cmd_wrap: false, node_cli: None });
            }
        }
        for ext in ["exe", "cmd", "bat"] {
            let cand = dir.join(format!("{program}.{ext}"));
            if cand.is_file() {
                return Some(Resolved { program: cand, needs_cmd_wrap: ext != "exe", node_cli: None });
            }
        }
    }

    let path_var = std::env::var_os("PATH")?;
    // PATH 中命中但无法直接执行的脚本（无扩展名非 PE），全部未命中时兑底尝试
    let mut script_fallback: Option<PathBuf> = None;
    for dir in std::env::split_paths(&path_var) {
        // 先尝试原样（可能是 .exe 名），再按 PATHEXT 补扩展名
        let exact = dir.join(program);
        if exact.is_file() {
            if is_script_ext(&exact) {
                return Some(Resolved { program: exact, needs_cmd_wrap: true, node_cli: None });
            }
            if is_pe_executable(&exact) {
                return Some(Resolved { program: exact, needs_cmd_wrap: false, node_cli: None });
            }
            // 无扩展名 sh 脚本（如 Node 的 npx）：记下候选，继续找同目录 .exe/.cmd/.bat
            script_fallback.get_or_insert(exact);
        }
        for ext in ["exe", "cmd", "bat"] {
            let cand = dir.join(format!("{program}.{ext}"));
            if cand.is_file() {
                return Some(Resolved { program: cand, needs_cmd_wrap: ext != "exe", node_cli: None });
            }
        }
    }

    // 系统 PATH 全部未命中：最后尝试 sh 脚本候选（cmd 包装，错误提示比 not found 更准确）
    if let Some(p) = script_fallback {
        return Some(Resolved { program: p, needs_cmd_wrap: true, node_cli: None });
    }
    None
}

/// 在捆绑 Node 目录中解析 node/npm/npx：npx/npm 直接经 `node.exe <cli.js>` 启动
fn resolve_bundled_node(program: &str) -> Option<Resolved> {
    let dir = BUNDLED_NODE_DIR.lock().unwrap().clone()?;
    if !dir.is_dir() {
        return None;
    }
    let base = program
        .strip_suffix(".exe")
        .unwrap_or(program)
        .strip_suffix(".cmd")
        .unwrap_or(program)
        .strip_suffix(".bat")
        .unwrap_or(program)
        .to_ascii_lowercase();

    match base.as_str() {
        "node" => {
            let exe = dir.join("node.exe");
            if exe.is_file() {
                Some(Resolved { program: exe, needs_cmd_wrap: false, node_cli: None })
            } else {
                None
            }
        }
        "npx" | "npm" => {
            let exe = dir.join("node.exe");
            let cli = dir.join("node_modules/npm/bin")
                .join(if base == "npx" { "npx-cli.js" } else { "npm-cli.js" });
            if exe.is_file() && cli.is_file() {
                Some(Resolved { program: exe, needs_cmd_wrap: false, node_cli: Some(cli) })
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_script_ext(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("cmd") | Some("bat")
    )
}

/// 在内置 Git 目录中解析 git：统一指向 `cmd\git.exe`（便携版布局）
fn resolve_bundled_git(program: &str) -> Option<Resolved> {
    let dir = BUNDLED_GIT_DIR.lock().unwrap().clone()?;
    if !dir.is_dir() {
        return None;
    }
    let base = program
        .strip_suffix(".exe")
        .unwrap_or(program)
        .strip_suffix(".cmd")
        .unwrap_or(program)
        .strip_suffix(".bat")
        .unwrap_or(program)
        .to_ascii_lowercase();
    if base != "git" {
        return None;
    }
    let exe = dir.join("cmd").join("git.exe");
    if exe.is_file() {
        Some(Resolved { program: exe, needs_cmd_wrap: false, node_cli: None })
    } else {
        None
    }
}

/// 在内置默认 JDK 目录中解析 java/javac（系统无 JDK 时兑底）。
/// 与 jdk_env_overrides 的跳过条件一致：系统 PATH 已有 java 或 JAVA_HOME 已设时
/// 尊重系统环境，不再兑底内置。
fn resolve_bundled_jdk(program: &str) -> Option<Resolved> {
    if std::env::var_os("JAVA_HOME").is_some() || system_path_has_java() {
        return None;
    }
    let dir = DEFAULT_JDK_DIR.lock().unwrap().clone()?;
    if !dir.is_dir() {
        return None;
    }
    let base = program.strip_suffix(".exe").unwrap_or(program).to_ascii_lowercase();
    if base != "java" && base != "javac" {
        return None;
    }
    let exe = dir.join("bin").join(format!("{base}.exe"));
    if exe.is_file() {
        Some(Resolved { program: exe, needs_cmd_wrap: false, node_cli: None })
    } else {
        None
    }
}

/// ohpm 直调：ohpm.bat 链最终是 `node pm-cli.js`（见 command-line-tools/ohpm/bin 布局），
/// 直接 `node.exe <pm-cli.js> <args>` 启动，绕开 .bat 链的黑框与参数传递问题。
/// 布局探测：`<bin>/../ohpm/bin/pm-cli.js`（bin/ohpm.bat 转发）或 `<dir>/pm-cli.js`。
fn resolve_ohpm_direct(program: &str) -> Option<Resolved> {
    let base = program
        .strip_suffix(".exe")
        .unwrap_or(program)
        .strip_suffix(".cmd")
        .unwrap_or(program)
        .strip_suffix(".bat")
        .unwrap_or(program)
        .to_ascii_lowercase();
    if base != "ohpm" {
        return None;
    }
    // 查找 ohpm.bat/.cmd：鸿蒙额外 PATH 优先，其次系统 PATH
    let mut dirs: Vec<PathBuf> = HARMONY_EXTRA_PATH.lock().unwrap().clone();
    if let Some(p) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    let script = dirs.iter().find_map(|d| {
        ["ohpm.bat", "ohpm.cmd"]
            .iter()
            .find_map(|n| d.join(n).is_file().then(|| d.join(n)))
    })?;
    // pm-cli.js 布局：<bin>/../ohpm/bin/pm-cli.js 或 <dir>/pm-cli.js
    let dir = script.parent()?;
    let cli = [
        dir.join("..").join("ohpm").join("bin").join("pm-cli.js"),
        dir.join("pm-cli.js"),
    ]
    .into_iter()
    .find(|p| p.is_file())?;
    // 用内置 Node 直调（绕开 DevEco 自带 node 与系统 node 的版本差异）
    let node = BUNDLED_NODE_DIR.lock().unwrap().clone()?.join("node.exe");
    if !node.is_file() {
        return None;
    }
    Some(Resolved { program: node, needs_cmd_wrap: false, node_cli: Some(cli) })
}

/// 找不到程序时的友好错误（含安装/路径建议）
fn not_found_error(program: &str) -> String {
    let hint = if program.eq_ignore_ascii_case("npx") {
        "请确认已安装 Node.js（npm 自带 npx），或将 Node 目录加入 PATH；也可以在命令中填写完整路径，如 C:\\Program Files\\nodejs\\npx.cmd"
    } else if program.eq_ignore_ascii_case("git") {
        "请确认已安装 Git（https://git-scm.com/download/win），或将 git 加入 PATH"
    } else if program.eq_ignore_ascii_case("docker") {
        "请确认已安装并启动 Docker Desktop"
    } else {
        "请确认该程序已安装并加入 PATH，或在命令中填写完整路径"
    };
    format!("找不到程序 {program}：{hint}")
}

/// .cmd/.bat 经 cmd.exe /C 执行时，把程序+参数拼成单条命令（引号包裹防空格破坏）。
/// 注意：cmd /C 会剥离整条命令最外层的一对引号，因此这里再包一层，
/// 否则 `"C:\path\x.cmd" args` 会被剥成 `C:\path\x.cmd" "args` 导致引号错位。
fn build_cmd_line(program: &Path, args: &[String]) -> String {
    let mut inner = format!("\"{}\"", program.display());
    for a in args {
        inner.push(' ');
        inner.push('"');
        inner.push_str(&a.replace('"', "\\\""));
        inner.push('"');
    }
    format!("\"{inner}\"")
}

/// 内置 npx/npm 直调：`node.exe <cli.js> <args...>`，把 cli.js 插到参数最前面
fn with_node_cli(cli: &Path, args: &[String]) -> Vec<std::ffi::OsString> {
    let mut v: Vec<std::ffi::OsString> = Vec::with_capacity(args.len() + 1);
    v.push(cli.as_os_str().to_owned());
    for a in args {
        v.push(a.into());
    }
    v
}

/// 构建配置好的子进程命令；找不到程序时返回友好错误（含安装/路径建议）。
pub fn command(program: &str, args: &[String]) -> Result<tokio::process::Command, String> {
    let resolved = resolve_program(program).ok_or_else(|| not_found_error(program))?;

    #[cfg(windows)]
    {
        // .cmd/.bat 需经 cmd.exe /C 包装：程序换成 cmd.exe。
        // 若直接 spawn 脚本并把 /C 当参数传，/C 会被脚本本身收到
        // （如 ohpm ERROR: unknown command '/C'）。
        let mut cmd = if resolved.needs_cmd_wrap {
            tokio::process::Command::new("cmd.exe")
        } else {
            tokio::process::Command::new(&resolved.program)
        };
        // 有隐藏/终端控制台时继承它（防孙进程新建窗口弹 cmd）；无控制台才用 CREATE_NO_WINDOW
        if !inherit_console() {
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        if let Some(cli) = &resolved.node_cli {
            cmd.args(with_node_cli(cli, args));
        } else if resolved.needs_cmd_wrap {
            // 用 raw_arg 原样传参：Rust 默认 arg 会把内部引号转义成 \"，
            // cmd.exe 不识别该转义，剥引号规则（/C 首字符为引号时去首尾）
            // 会失效导致程序名带引号报错
            cmd.raw_arg(format!("/C {}", build_cmd_line(&resolved.program, args)));
        } else {
            cmd.args(args);
        }
        apply_jdk_env(&mut cmd);
        // Agent task被看门狗 abort 时 Child 会被直接 drop；默认行为会把进程遗留在后台。
        // 至少终止直接子进程，正常的超时/停止路径仍用 kill_tree 处理完整进程树。
        cmd.kill_on_drop(true);
        Ok(cmd)
    }

    #[cfg(not(windows))]
    {
        let mut cmd = tokio::process::Command::new(&resolved.program);
        if let Some(cli) = &resolved.node_cli {
            cmd.args(with_node_cli(cli, args));
        } else {
            cmd.args(args);
        }
        apply_jdk_env(&mut cmd);
        cmd.kill_on_drop(true);
        Ok(cmd)
    }
}

/// 把内置 JDK 环境覆盖应用到命令（系统已有 JDK 时不注入，见 jdk_env_overrides）
fn apply_jdk_env(cmd: &mut tokio::process::Command) {
    for (k, v) in jdk_env_overrides() {
        cmd.env(k, v);
    }
}

/// 强杀整个子进程树（Windows 用 taskkill /T /F，其他平台 kill -9）。
/// 解决 cmd.exe / npx 包装启动的子进程只杀直接子进程时孙进程残留的问题
/// （残留进程会继续占用管道/端口，且 npx 下载卡住时反复 spawn 会堆积）。
///
/// 这里只负责发起终止，不同步等待 taskkill/kill 退出。该函数会被 Tauri 同步命令和
/// async worker 共同调用；Windows 上 `.output()` 偶尔会被系统进程查询/安全软件拖住，
/// 进而直接冻结命令处理线程乃至界面。调用方通过 child.wait/管道收尾观察最终退出。
pub fn kill_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    if pid == 0 {
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    #[cfg(not(windows))]
    {
        // macOS/Linux 同样可能有 shell/node/hvigor 孙进程继续持有管道。先终止直接
        // 子进程，再终止包装器本身；通过 sh 顺序执行，避免父进程先死后子进程被
        // reparent 导致 `pkill -P` 再也找不到。pkill 不存在/无匹配时仍继续 kill 父进程。
        let script = format!(
            "pkill -KILL -P {pid} >/dev/null 2>&1 || true; kill -KILL {pid} >/dev/null 2>&1 || true"
        );
        let _ = std::process::Command::new("sh")
            .args(["-c", &script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

/// 同步执行并捕获输出（Windows 下同样隐藏窗口）；用于非 async 上下文（如 git rev-parse）。
pub fn output_blocking(program: &str, args: &[String]) -> Result<std::process::Output, String> {
    let resolved = resolve_program(program).ok_or_else(|| not_found_error(program))?;

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // 同 command()：.cmd/.bat 经 cmd.exe /C 包装，程序换成 cmd.exe
        let mut cmd = if resolved.needs_cmd_wrap {
            std::process::Command::new("cmd.exe")
        } else {
            std::process::Command::new(&resolved.program)
        };
        if !inherit_console() {
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        if let Some(cli) = &resolved.node_cli {
            cmd.args(with_node_cli(cli, args));
        } else if resolved.needs_cmd_wrap {
            cmd.raw_arg(format!("/C {}", build_cmd_line(&resolved.program, args)));
        } else {
            cmd.args(args);
        }
        for (k, v) in jdk_env_overrides() {
            cmd.env(k, v);
        }
        cmd.output().map_err(|e| format!("执行 {program} 失败: {e}"))
    }

    #[cfg(not(windows))]
    {
        let mut cmd = std::process::Command::new(&resolved.program);
        if let Some(cli) = &resolved.node_cli {
            cmd.args(with_node_cli(cli, args));
        } else {
            cmd.args(args);
        }
        for (k, v) in jdk_env_overrides() {
            cmd.env(k, v);
        }
        cmd.output().map_err(|e| format!("执行 {program} 失败: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全局工具链状态（内置 node/git 目录、鸿蒙 PATH 目录）由多个测试共享写，
    /// 测试并行时互踩会偶发失败；用互斥锁把共享状态的测试串行化。
    static STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_resolve_empty() {
        assert!(resolve_program("").is_none());
    }

    #[test]
    fn test_resolve_nonexistent() {
        assert!(resolve_program("this-program-definitely-not-exists-xyz").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn test_resolve_cmd_exe() {
        // cmd.exe 一定存在；应解析为 .exe 且不需要 cmd 包装
        let r = resolve_program("cmd.exe").unwrap();
        assert!(!r.needs_cmd_wrap);
    }

    #[test]
    fn test_command_not_found_has_hint() {
        let err = command("this-program-definitely-not-exists-xyz", &[]).unwrap_err();
        assert!(err.contains("找不到程序"));
    }

    /// 内置 Node 目录路径（开发机上存在 runtime/node；CI 无则跳过）
    fn bundled_dir() -> Option<std::path::PathBuf> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/node");
        dir.join("node.exe").is_file().then_some(dir)
    }

    #[test]
    fn test_bundled_node_resolve() {
        let _g = STATE_LOCK.lock().unwrap();
        let Some(dir) = bundled_dir() else { return };
        set_bundled_node_dir(Some(dir));
        let npx = resolve_bundled_node("npx").expect("内置 npx 应可解析");
        assert!(npx.node_cli.is_some(), "npx 应走 node.exe <cli.js> 直调");
        assert!(!npx.needs_cmd_wrap);
        let node = resolve_bundled_node("node").expect("内置 node 应可解析");
        assert!(node.node_cli.is_none());
        assert!(resolve_bundled_node("git").is_none(), "非 node/npm/npx 不兑底");
    }

    /// 内置 Node 存在时，npx/npm/node 必须内置优先（绕开系统 Node 的 sh 脚本/.cmd）
    #[test]
    fn test_bundled_node_priority() {
        let _g = STATE_LOCK.lock().unwrap();
        let Some(dir) = bundled_dir() else { return };
        set_bundled_node_dir(Some(dir));
        let npx = resolve_program("npx").expect("npx 应可解析");
        assert!(npx.node_cli.is_some(), "npx 应命中内置 node.exe <cli.js> 直调");
        assert!(npx.program.ends_with("node.exe"));
        let node = resolve_program("node").expect("node 应可解析");
        assert!(node.program.ends_with("node.exe"), "node 应命中内置 node.exe");
        assert!(!node.needs_cmd_wrap);
    }

    /// 内置 Git 目录（开发机上存在 runtime/git；CI 无则跳过）
    fn bundled_git_dir() -> Option<std::path::PathBuf> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/git");
        dir.join("cmd").join("git.exe").is_file().then_some(dir)
    }

    /// 内置 Git 存在时，git 应命中内置 cmd\git.exe（不依赖系统 PATH）
    #[test]
    fn test_bundled_git_resolve() {
        let _g = STATE_LOCK.lock().unwrap();
        let Some(dir) = bundled_git_dir() else { return };
        set_bundled_git_dir(Some(dir));
        let git = resolve_bundled_git("git").expect("内置 git 应可解析");
        assert!(git.program.ends_with("git.exe"));
        assert!(!git.needs_cmd_wrap);
        assert!(resolve_bundled_git("node").is_none(), "非 git 不兑底");
        assert!(resolve_bundled_git("gitk").is_none(), "非 git 命令不兑底");
    }

    /// 内置 Git 存在时，git 解析内置优先（系统 PATH 有 git 也不影响）
    #[test]
    fn test_bundled_git_priority() {
        let _g = STATE_LOCK.lock().unwrap();
        let Some(dir) = bundled_git_dir() else { return };
        set_bundled_git_dir(Some(dir));
        let git = resolve_program("git").expect("git 应可解析");
        assert!(git.program.ends_with("cmd\\git.exe") || git.program.ends_with("cmd/git.exe"),
            "git 应命中内置目录，实际: {}", git.program.display());
    }

    /// 端到端：内置 npx 直调真实执行（绕开系统 PATH）
    #[test]
    fn test_bundled_npx_direct_run() {
        let _g = STATE_LOCK.lock().unwrap();
        let Some(dir) = bundled_dir() else { return };
        set_bundled_node_dir(Some(dir));
        let r = resolve_bundled_node("npx").expect("内置 npx 应可解析");
        let cli = r.node_cli.clone().expect("npx 应有 cli 脚本");
        let out = std::process::Command::new(&r.program)
            .args(with_node_cli(&cli, &["--version".to_string()]))
            .output()
            .expect("运行内置 npx 失败");
        assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(!s.trim().is_empty(), "npx --version 输出为空");
    }

    /// build_cmd_line 应在外层包一对引号，避免 cmd /C 剥引号导致错位
    #[test]
    fn test_build_cmd_line_wrapping() {
        let s = build_cmd_line(Path::new(r"C:\a b\x.cmd"), &["install".to_string()]);
        assert_eq!(s, "\"\"C:\\a b\\x.cmd\" \"install\"\"");
    }

    /// .cmd/.bat 经 cmd.exe /C 包装后可正确执行且参数传递无误（真实端到端）
    #[cfg(windows)]
    #[test]
    fn test_cmd_wrap_executes_script() {
        let dir = std::env::temp_dir().join(format!("deveco-cmdwrap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let bat = dir.join("echoer.bat");
        // %~1 去掉外层引号（真实批处理脚本惯例）；能去成功说明引号包裹成对正确
        std::fs::write(&bat, "@echo off\r\necho hello-from-bat %~1\r\n").unwrap();
        let prog = bat.to_string_lossy().to_string();
        let r = resolve_program(&prog).expect("bat 应可解析");
        assert!(r.needs_cmd_wrap, "bat 应走 cmd 包装");
        let out = output_blocking(&prog, &["world".to_string()]).expect("bat 执行失败");
        assert!(
            out.status.success(),
            "exit={:?} stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("hello-from-bat world"),
            "参数应正确传给脚本，实际: {stdout}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ohpm 直调：内置 node + 鸿蒙 command-line-tools 布局（开发机存在时验证）
    #[test]
    fn test_ohpm_direct_layout_probe() {
        let _g = STATE_LOCK.lock().unwrap();
        let Some(dir) = bundled_dir() else { return };
        set_bundled_node_dir(Some(dir));
        // 构造模拟布局：bin/ohpm.bat + ../ohpm/bin/pm-cli.js
        let tmp = std::env::temp_dir().join(format!("deveco-ohpm-{}", std::process::id()));
        let bin = tmp.join("bin");
        std::fs::create_dir_all(tmp.join("ohpm").join("bin")).ok();
        std::fs::create_dir_all(&bin).ok();
        std::fs::write(bin.join("ohpm.bat"), "@echo off").unwrap();
        std::fs::write(tmp.join("ohpm").join("bin").join("pm-cli.js"), "// pm-cli").unwrap();
        set_harmony_path_dirs(vec![bin.clone()]);
        let r = resolve_ohpm_direct("ohpm").expect("ohpm 应直调解析");
        assert!(r.node_cli.is_some(), "ohpm 应走 node <pm-cli.js> 直调");
        assert!(!r.needs_cmd_wrap);
        assert!(r.program.ends_with("node.exe"));
        assert!(resolve_ohpm_direct("hdc").is_none(), "非 ohpm 不兑底");
        set_harmony_path_dirs(Vec::new());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 开发机存在鸿蒙 command-line-tools 时，ohpm 应直调真实执行（--version）
    #[test]
    fn test_ohpm_real_direct_run() {
        let _g = STATE_LOCK.lock().unwrap();
        let Some(dir) = bundled_dir() else { return };
        set_bundled_node_dir(Some(dir));
        let bin = Path::new(r"H:\command-line-tools\bin");
        if !bin.join("ohpm.bat").is_file() {
            return;
        }
        set_harmony_path_dirs(vec![bin.to_path_buf()]);
        let r = resolve_program("ohpm").expect("ohpm 应可解析");
        assert!(r.node_cli.is_some(), "ohpm 应走 node <pm-cli.js> 直调");
        let out = output_blocking("ohpm", &["--version".to_string()]).expect("ohpm 执行失败");
        assert!(
            out.status.success(),
            "exit={:?} stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        set_harmony_path_dirs(Vec::new());
    }
}
