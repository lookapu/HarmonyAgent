//! 系统终端：在当前项目根目录打开可见的 cmd 窗口（Windows），供用户手动执行命令。
//!
//! 与 Agent 的 run_command（静默执行、CREATE_NO_WINDOW）不同，这里是交互式终端窗口，
//! 刻意不设置 CREATE_NO_WINDOW，让窗口保持可见供用户输入命令。

use std::path::Path;

/// 在项目根目录打开系统终端窗口（Windows 为 cmd.exe /K，以项目目录为启动目录）。
#[tauri::command]
pub fn open_terminal(project_path: String) -> Result<(), String> {
    let path = project_path.trim().to_string();
    if path.is_empty() {
        return Err("未指定项目目录".into());
    }
    let dir = Path::new(&path);
    if !dir.is_dir() {
        return Err(format!("项目目录不存在：{path}"));
    }
    #[cfg(windows)]
    {
        // 窗口标题取项目目录名，便于多项目多窗口区分
        let title = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "DevEco Switch".to_string());
        // 直接以项目根目录作为 cmd 的启动目录（current_dir），不再拼接 `cd /d`：
        // 之前用 arg 传 `cd /d \"path\"`，Rust 会把参数内引号转义成 \"，cmd 不识别，
        // cd 失败后提示符停留在应用自身目录（target\release）。
        std::process::Command::new("cmd.exe")
            .arg("/K")
            .arg(format!("title {}", title))
            .current_dir(dir)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("打开终端失败：{e}"))
    }
    #[cfg(not(windows))]
    {
        // 非 Windows：尝试常见终端模拟器（在项目目录下启动）
        for t in [
            "x-terminal-emulator",
            "gnome-terminal",
            "konsole",
            "xfce4-terminal",
            "xterm",
        ] {
            if std::process::Command::new(t).current_dir(dir).spawn().is_ok() {
                return Ok(());
            }
        }
        Err("未找到可用的终端模拟器".into())
    }
}
