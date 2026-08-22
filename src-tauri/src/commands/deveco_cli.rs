use serde::Serialize;

/// DevEco CLI（@deveco/deveco-cli，`devecocli` 命令）探测结果
#[derive(Debug, Serialize)]
pub struct DevecoCliInfo {
    /// 是否可执行（已安装且 `--version` 跑通）
    pub installed: bool,
    /// devecocli --version 输出（如 1.3.0）；不可用时为空
    pub version: String,
    /// 命中的 shim/可执行路径（未安装时为空）
    pub path: Option<String>,
    /// 给用户的安装/排障指引（未安装或执行失败时展示）
    pub install_hint: String,
}

/// 探测 devecocli：供健康页展示（DC-05）与 MCP 模板创建引导（DC-09）共用。
/// 统一走 process::command 解析（系统 PATH / npm 全局 bin 内置 Node 直调），
/// 与 MCP 服务器启动路径完全一致，探测结果即真实可用性。
#[tauri::command]
pub async fn detect_devecocli() -> Result<DevecoCliInfo, String> {
    match crate::utils::process::command("devecocli", &["--version".to_string()]) {
        Ok(mut cmd) => match cmd.output().await {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                Ok(DevecoCliInfo {
                    installed: true,
                    version,
                    path: shim_path(),
                    install_hint: String::new(),
                })
            }
            Ok(output) => {
                // 命令能解析但执行失败（依赖缺失等）：报告未安装并带上原因
                let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let detail = if err.is_empty() {
                    "已找到 devecocli 但执行失败，请检查 DevEco Studio 是否已安装（要求 6.1+）"
                        .to_string()
                } else {
                    format!("已找到 devecocli 但执行失败：{err}")
                };
                Ok(DevecoCliInfo {
                    installed: false,
                    version: String::new(),
                    path: shim_path(),
                    install_hint: detail,
                })
            }
            Err(e) => Ok(DevecoCliInfo {
                installed: false,
                version: String::new(),
                path: None,
                install_hint: format!("{e}"),
            }),
        },
        Err(e) => Ok(DevecoCliInfo {
            installed: false,
            version: String::new(),
            path: None,
            // not_found_error 已带官方安装命令引导
            install_hint: e,
        }),
    }
}

/// 用户级 npm 全局 bin 中的 devecocli shim（存在时展示，便于用户确认安装位置）
fn shim_path() -> Option<String> {
    let dir = crate::utils::process::npm_global_bin_dir()?;
    let shim = dir.join(if cfg!(windows) { "devecocli.cmd" } else { "devecocli" });
    shim.is_file().then(|| shim.to_string_lossy().to_string())
}
