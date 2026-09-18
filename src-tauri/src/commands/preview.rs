//! 预览：Web 预览窗口（既有）+ 鸿蒙设备预览（驱动 SDK 内置 Previewer 取真实帧）。
//!
//! 设备预览的配方与实测结论见 `services/previewer.rs` 的模块注释与
//! docs/MAINLINE_AUDIT_2026-09-14.md §52。这里只负责编排：构建 → 起引擎 → 收帧转发。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::services::previewer;

/// 打开（或导航）Web 预览窗口。仅接受 http/https，防协议注入。
#[tauri::command]
pub async fn open_preview_window(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let url = url.trim().to_string();
    let lower = url.to_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("仅支持 http/https 地址".into());
    }
    let parsed: url::Url = url.parse().map_err(|_| "URL 格式不正确".to_string())?;
    if let Some(w) = app.get_webview_window("preview") {
        let _ = w.navigate(parsed);
        let _ = w.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(&app, "preview", tauri::WebviewUrl::External(parsed))
        .title("Web 预览")
        .inner_size(1100.0, 760.0)
        .min_inner_size(480.0, 360.0)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 正在运行的预览会话（同一时刻只保留一个，重复启动会先停掉旧的）。
struct Session {
    child: tokio::process::Child,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// 回给前端的会话信息
#[derive(Debug, Clone, Serialize)]
pub struct PreviewSessionInfo {
    pub port: u16,
    pub sid: String,
    pub module: String,
    pub device: String,
    pub width: u32,
    pub height: u32,
}

async fn run_preview_build(root: &Path, module: &str, product: &str) -> Result<(), String> {
    let hvigor = crate::services::harmony::hvigor_command(root)?;
    let mut args = hvigor.args.clone();
    // 必须带 buildRoot=.preview：否则 hvigor 走非预览任务链、不产出 .preview/（§52 根因）
    args.extend([
        "--mode".to_string(),
        "module".to_string(),
        "-p".to_string(),
        format!("module={module}@default"),
        "-p".to_string(),
        format!("product={product}"),
        "-p".to_string(),
        "buildRoot=.preview".to_string(),
        "PreviewBuild".to_string(),
    ]);
    let mut cmd = crate::utils::process::command(&hvigor.program, &args)?;
    cmd.current_dir(root);
    if !hvigor.env.is_empty() {
        cmd.envs(hvigor.env.iter().cloned());
    }
    let output = tokio::time::timeout(Duration::from_secs(300), cmd.output())
        .await
        .map_err(|_| "预览构建超时（300 秒）".to_string())?
        .map_err(|e| format!("预览构建无法启动：{e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    let tail: String = text.chars().rev().take(1200).collect::<Vec<_>>().into_iter().rev().collect();
    Err(format!("预览构建失败：\n{}", tail.trim()))
}

/// 设备档 json：优先取 preview server 自带的 `deviceConfigJson/<device>SettingConfig.json`。
/// 找不到就不传 `-f`（引擎仍可渲染，只是少了设备外观参数）。
fn device_config(studio_dir: Option<&str>, device: &str) -> Option<PathBuf> {
    let studio = Path::new(studio_dir?);
    let dir = studio
        .join("plugins")
        .join("openharmony")
        .join("openharmony-preview-server")
        .join("deviceConfigJson");
    let by_device = dir.join(format!("{}SettingConfig.json", device));
    if by_device.is_file() {
        return Some(by_device);
    }
    let fallback = dir.join("phoneSettingConfig.json");
    fallback.is_file().then_some(fallback)
}

/// 收帧循环：连引擎的 WebSocket，按 §52 的帧格式切出 JPEG 推给前端。
async fn stream_frames(app: tauri::AppHandle, port: u16, sid: String) {
    // 引擎监听需要一点时间，重试几次再放弃
    let mut stream = None;
    let mut last_err = String::from("预览引擎未就绪");
    for _ in 0..20 {
        match previewer::ws_connect(port, &sid).await {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => {
                last_err = e;
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }
    let Some(mut stream) = stream else {
        let _ = app.emit("preview-error", last_err);
        return;
    };
    let mut buf: Vec<u8> = Vec::new();
    loop {
        match previewer::next_payload(&mut stream, &mut buf).await {
            Ok(Some(frame)) => {
                let Some(jpeg) = previewer::jpeg_in_frame(&frame) else {
                    continue;
                };
                use base64::Engine;
                let payload = serde_json::json!({
                    "jpeg": base64::engine::general_purpose::STANDARD.encode(jpeg),
                    "bytes": jpeg.len(),
                });
                let _ = app.emit("preview-frame", payload);
            }
            Ok(None) => break,
            Err(e) => {
                let _ = app.emit("preview-error", e);
                break;
            }
        }
    }
}

/// 启动设备预览：构建 → 起引擎 → 后台收帧。
/// 引擎不可用时返回 Err（前端据此退回 Web 预览，不静默失败）。
#[tauri::command]
pub async fn preview_start(
    app: tauri::AppHandle,
    project: String,
    module: Option<String>,
    page: Option<String>,
    device: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<PreviewSessionInfo, String> {
    let root = PathBuf::from(project.trim());
    if !root.is_dir() {
        return Err(format!("工程目录不存在：{}", root.display()));
    }
    if !crate::services::workspace::classify(&root)
        .is_some_and(|k| k == crate::services::workspace::ModuleKind::Harmony)
    {
        return Err("目标目录不是 HarmonyOS 工程，无法设备预览".into());
    }
    let env = crate::services::harmony_env::detect(
        app.try_state::<crate::db::DbState>()
            .map(|s| s.inner())
            .ok_or_else(|| "数据库状态不可用".to_string())?,
    );
    let bin = previewer::locate(&env).ok_or_else(|| {
        "未找到 Previewer 引擎（需要 DevEco Studio 的 SDK 组件 previewer/common/bin）。\
         可用 DevEco Studio 打开工程，或改用 Web 预览。"
            .to_string()
    })?;

    let model = crate::services::harmony_model::cached(&root);
    let module = module
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .or_else(|| crate::services::harmony::project_summary(&root, &model).entry_module)
        .unwrap_or_else(|| "entry".to_string());
    let product = "default".to_string();
    let device = device
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "phone".to_string());
    let width = width.unwrap_or(1080);
    let height = height.unwrap_or(2340);
    let page = page
        .map(|p| p.trim().trim_start_matches('/').to_string())
        .filter(|p| !p.is_empty())
        .or_else(|| previewer::default_page(&root, &module))
        .ok_or_else(|| {
            format!(
                "无法确定预览页面：{module} 里既没有 module.json5 声明的页面清单（$profile:），\
                 也找不到约定入口 pages/Index。请在 DevEco 里确认该模块能正常预览。"
            )
        })?;

    // 先停掉旧会话，避免重复点击泄漏引擎进程
    let previous = { SESSION.lock().map_err(|e| e.to_string())?.take() };
    if let Some(mut old) = previous {
        let _ = old.child.kill().await;
        let _ = old.child.wait().await;
    }

    run_preview_build(&root, &module, &product).await?;
    let artifacts = previewer::locate_artifacts(&root, &module, &product).ok_or_else(|| {
        format!(
            "预览构建产物不完整（{module}/.preview/{product}/intermediates）。\
             若是首次预览，请确认工程能在 DevEco 里正常构建。"
        )
    })?;
    let port = previewer::pick_port()
        .ok_or_else(|| "找不到可用的预览端口（引擎要求 29000~50000）".to_string())?;
    let sid = previewer::gen_sid();
    let cfg = device_config(env.studio_dir.as_deref(), &device);

    let spec = previewer::EngineSpec {
        module: module.as_str(),
        page: page.as_str(),
        device: device.as_str(),
        width,
        height,
        project_id: 1,
        sid: sid.as_str(),
        port,
        bundle_dir: artifacts.bundle_dir.as_path(),
        loader_json: artifacts.loader_json.as_path(),
        res_dir: artifacts.res_dir.as_path(),
        device_config: cfg.as_deref(),
        trace_pipe: "deveco_switch_trace_commandPipe",
    };
    let args = previewer::engine_args(&spec);
    let mut cmd = crate::utils::process::command(&bin.exe.to_string_lossy(), &args)?;
    // cwd 必须是引擎自己的目录，否则找不到 fontconfig.json，文字渲染不出来（§52 实测）
    cmd.current_dir(&bin.dir);
    let child = cmd
        .spawn()
        .map_err(|e| format!("启动预览引擎失败：{e}"))?;

    {
        let mut guard = SESSION.lock().map_err(|e| e.to_string())?;
        *guard = Some(Session { child });
    }
    let handle = app.clone();
    let sid_for_task = sid.clone();
    tokio::spawn(async move { stream_frames(handle, port, sid_for_task).await });

    Ok(PreviewSessionInfo {
        port,
        sid,
        module,
        device,
        width,
        height,
    })
}

/// 停止设备预览：杀掉引擎进程，收帧循环随连接断开自然退出。
#[tauri::command]
pub async fn preview_stop() -> Result<(), String> {
    let session = { SESSION.lock().map_err(|e| e.to_string())?.take() };
    if let Some(mut session) = session {
        let _ = session.child.kill().await;
        let _ = session.child.wait().await;
    }
    Ok(())
}
