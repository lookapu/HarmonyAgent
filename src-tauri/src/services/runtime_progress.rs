//! 运行时（Node / Git / JDK）在线下载的通用进度事件。
//!
//! 下载在 tokio 异步上下文中逐块读取，通过 tauri 事件推送到前端，
//! 全程不阻塞主进程；SHA256 校验与解压位于 spawn_blocking，同样不占主线程。

use serde::Serialize;

/// 运行时下载/安装进度（Node / Git / JDK 共用）
#[derive(Debug, Serialize, Clone)]
pub struct RuntimeProgress {
    /// 阶段：check（网络/版本检查）/ download / verify / extract / done
    pub phase: String,
    /// 阶段描述（直接展示）
    pub message: String,
    /// 下载进度百分比（0-100，download 阶段有效）
    pub percent: Option<f64>,
    /// 已下载字节数
    pub downloaded: Option<u64>,
    /// 总字节数（响应未给 Content-Length 时为 None）
    pub total: Option<u64>,
    /// 实时速度（字节/秒）
    pub speed: Option<f64>,
}

impl RuntimeProgress {
    /// 构造一个无进度数字的阶段事件（check/verify/extract/done）
    pub fn phase(phase: &str, message: impl Into<String>) -> Self {
        Self {
            phase: phase.into(),
            message: message.into(),
            percent: None,
            downloaded: None,
            total: None,
            speed: None,
        }
    }
}

/// 推送进度事件（发送失败不影响主流程）
pub fn emit(app: &tauri::AppHandle, event: &str, p: &RuntimeProgress) {
    use tauri::Emitter;
    let _ = app.emit(event, p);
}

/// 流式下载到临时文件：逐块写盘 + 同步计算 SHA256，按节流间隔推送进度。
/// 返回 (文件路径, sha256 十六进制字符串)。全程不把大文件整体缓冲进内存。
pub async fn download_to_file(
    app: &tauri::AppHandle,
    event: &str,
    client: &reqwest::Client,
    url: &str,
    file_path: &std::path::Path,
    phase_message: impl Fn(&str) -> String + Send + Sync,
    throttle_ms: u64,
) -> Result<(std::path::PathBuf, String), String> {
    use futures_util::StreamExt;
    use sha2::{Digest, Sha256};

    let resp = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => return Err(format!("下载失败（请检查网络或系统代理）: {e}")),
    };
    if !resp.status().is_success() {
        return Err(format!("下载失败: HTTP {}", resp.status()));
    }
    let total = resp.content_length();
    let mut file = std::fs::File::create(file_path).map_err(|e| format!("创建临时文件失败: {e}"))?;
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let start = std::time::Instant::now();
    let mut last_emit = std::time::Instant::now();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("读取下载内容失败: {e}"))?;
        downloaded += chunk.len() as u64;
        hasher.update(&chunk);
        std::io::Write::write_all(&mut file, &chunk).map_err(|e| format!("写入临时文件失败: {e}"))?;
        if last_emit.elapsed().as_millis() >= throttle_ms as u128 {
            let elapsed = start.elapsed().as_secs_f64();
            let speed = if elapsed > 0.0 { downloaded as f64 / elapsed } else { 0.0 };
            let percent = total
                .filter(|t| *t > 0)
                .map(|t| downloaded as f64 * 100.0 / t as f64);
            emit(
                app,
                event,
                &RuntimeProgress {
                    phase: "download".into(),
                    message: phase_message(&format_percent(percent)),
                    percent,
                    downloaded: Some(downloaded),
                    total,
                    speed: Some(speed),
                },
            );
            last_emit = std::time::Instant::now();
        }
    }
    let sha = format!("{:x}", hasher.finalize());
    Ok((file_path.to_path_buf(), sha))
}

fn format_percent(p: Option<f64>) -> String {
    p.map(|v| format!("{v:.1}%")).unwrap_or_default()
}
