//! 生成媒体服务：调用各厂商图片/视频/音频生成 API，产物保存到本地并返回文件路径。
//!
//! 端点形态（按服务商域名特征自动判定）：
//! - 图像：OpenAI 兼容 `POST {base}/images/generations`（智谱/OpenAI/硅基流动/混元等）；
//!   MiniMax 专用 `POST {base}/v1/image_generation`（image-01）
//! - 视频：异步任务 + 轮询。MiniMax H3 `POST {base}/v2/video_generation` →
//!   `GET {base}/v2/query/video_generation/{task_id}`（content[] 多模态数组）；
//!   火山方舟 `POST {base}/api/v3/contents/generations/tasks` → 同路径 GET 查询（Seedance）；
//!   智谱 `POST {base}/videos/generations` → `GET {base}/async-result/{task_id}`（CogVideoX）
//! - 音频：OpenAI 兼容 `POST {base}/audio/speech`（返回 MP3 字节）；MiniMax
//!   `POST {base}/v1/t2a_v2`（speech-2.8-hd，返回 hex 编码音频）

use std::path::Path;

use base64::Engine;
use serde_json::Value;

/// 生成媒体类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenKind {
    Image,
    Video,
    Audio,
}

impl GenKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "image" => Some(Self::Image),
            "video" => Some(Self::Video),
            "audio" => Some(Self::Audio),
            _ => None,
        }
    }

    /// 中文名（错误提示/事件载荷使用）
    pub fn label(self) -> &'static str {
        match self {
            Self::Image => "图片",
            Self::Video => "视频",
            Self::Audio => "音频",
        }
    }

    fn default_ext(self) -> &'static str {
        match self {
            Self::Image => "png",
            Self::Video => "mp4",
            Self::Audio => "mp3",
        }
    }
}

/// 生成端点形态：决定请求路径与报文结构
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenEndpointStyle {
    /// OpenAI 兼容图像生成：POST {base}/images/generations
    ImagesGenerations,
    /// MiniMax 图像生成：POST {base}/v1/image_generation
    MinimaxImage,
    /// MiniMax 视频生成（H3 等）：POST {base}/v2/video_generation + 任务查询
    MinimaxVideo,
    /// 火山方舟视频生成（Seedance）：POST {base}/api/v3/contents/generations/tasks + 任务查询
    ArkVideo,
    /// 智谱视频生成（CogVideoX）：POST {base}/videos/generations + async-result
    ZhipuVideo,
    /// OpenAI 兼容 TTS：POST {base}/audio/speech（返回音频字节）
    OpenaiSpeech,
    /// MiniMax TTS：POST {base}/v1/t2a_v2（返回 hex 音频）
    MinimaxT2a,
}

/// 按服务商域名特征判定端点形态；未知域名回退 OpenAI 兼容形态（视频默认 MiniMax 任务型）。
pub fn endpoint_style_for(base_url: &str, kind: GenKind) -> GenEndpointStyle {
    let lower = base_url.to_lowercase();
    match kind {
        GenKind::Image => {
            if lower.contains("minimaxi.com") || lower.contains("minimax.io") {
                GenEndpointStyle::MinimaxImage
            } else {
                GenEndpointStyle::ImagesGenerations
            }
        }
        GenKind::Video => {
            if lower.contains("minimaxi.com") || lower.contains("minimax.io") {
                GenEndpointStyle::MinimaxVideo
            } else if lower.contains("volces.com") {
                GenEndpointStyle::ArkVideo
            } else if lower.contains("bigmodel.cn") {
                GenEndpointStyle::ZhipuVideo
            } else {
                GenEndpointStyle::MinimaxVideo
            }
        }
        GenKind::Audio => {
            if lower.contains("minimaxi.com") || lower.contains("minimax.io") {
                GenEndpointStyle::MinimaxT2a
            } else {
                GenEndpointStyle::OpenaiSpeech
            }
        }
    }
}

/// 生成请求参数（闭包：is_stopped 检查停止请求；on_progress 视频轮询进度秒数）
pub struct GenRequest<'a> {
    pub client: &'a reqwest::Client,
    pub base_url: &'a str,
    pub api_key: Option<&'a str>,
    pub kind: GenKind,
    pub model: &'a str,
    pub prompt: &'a str,
    /// 参考图（data URL 或公网 URL；视频生图的参考帧）
    pub images: &'a [String],
    /// 产物保存目录（不存在则创建）
    pub out_dir: &'a Path,
    pub is_stopped: &'a (dyn Fn() -> bool + Send + Sync),
    /// 视频异步轮询进度回调（已等待秒数）
    pub on_progress: &'a (dyn Fn(u64) + Send + Sync),
}

/// 视频轮询参数：10 秒间隔，15 分钟上限
const POLL_INTERVAL_SECS: u64 = 10;
const POLL_MAX_SECS: u64 = 900;

/// 调用生成 API 并把产物保存到 out_dir，返回本地文件路径列表。
pub async fn generate(req: &GenRequest<'_>) -> Result<Vec<String>, String> {
    let style = endpoint_style_for(req.base_url, req.kind);
    let files = match req.kind {
        GenKind::Image => generate_image(req, style).await?,
        GenKind::Video => generate_video(req, style).await?,
        GenKind::Audio => generate_audio(req, style).await?,
    };
    if files.is_empty() {
        return Err(format!(
            "{}生成成功但未解析到产物（服务商响应格式异常）",
            req.kind.label()
        ));
    }
    Ok(files)
}

/// 带鉴权头的 POST JSON 请求
fn post_json(
    client: &reqwest::Client,
    url: &str,
    api_key: Option<&str>,
    body: &Value,
) -> reqwest::RequestBuilder {
    let mut rb = client.post(url).json(body);
    if let Some(key) = api_key {
        rb = rb.header("Authorization", format!("Bearer {key}"));
    }
    rb
}

/// 带鉴权头的 GET 请求
fn get_json(client: &reqwest::Client, url: &str, api_key: Option<&str>) -> reqwest::RequestBuilder {
    let mut rb = client.get(url);
    if let Some(key) = api_key {
        rb = rb.header("Authorization", format!("Bearer {key}"));
    }
    rb
}

/// 从响应体提取可读错误信息（截断 200 字符）
fn err_text(status: reqwest::StatusCode, body: &str) -> String {
    let body = body.trim();
    let detail = if body.is_empty() {
        String::new()
    } else {
        format!(": {}", truncate(body, 200))
    };
    format!("HTTP {}{}", status.as_u16(), detail)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

/// 保存产物：data URL 解码 / http(s) 下载，写入 out_dir/{name}.{ext}
async fn save_asset(
    client: &reqwest::Client,
    api_key: Option<&str>,
    src: &str,
    out_dir: &Path,
    name: &str,
    ext: &str,
) -> Result<String, String> {
    let bytes: Vec<u8> = if let Some(b64) = src.strip_prefix("data:") {
        // data:[mime];base64,xxxx
        let b64 = b64.split_once(',').map(|(_, b)| b).unwrap_or(b64);
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("产物 base64 解码失败: {e}"))?
    } else if src.starts_with("http://") || src.starts_with("https://") {
        let mut rb = client.get(src);
        if let Some(key) = api_key {
            rb = rb.header("Authorization", format!("Bearer {key}"));
        }
        let resp = rb
            .send()
            .await
            .map_err(|e| format!("产物下载失败: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("产物下载失败: HTTP {}", status.as_u16()));
        }
        resp.bytes()
            .await
            .map_err(|e| format!("产物读取失败: {e}"))?
            .to_vec()
    } else {
        return Err(format!("不支持的产物地址: {src}"));
    };
    if bytes.is_empty() {
        return Err("产物内容为空".to_string());
    }
    std::fs::create_dir_all(out_dir).map_err(|e| format!("创建保存目录失败: {e}"))?;
    let path = out_dir.join(format!("{name}.{ext}"));
    std::fs::write(&path, &bytes).map_err(|e| format!("产物保存失败: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// 从产物 URL 推断扩展名（无后缀时用 kind 默认扩展名）
fn ext_from_url(url: &str, fallback: &str) -> String {
    let lower = url.to_lowercase();
    for pat in [
        ".jpg", ".jpeg", ".webp", ".gif", ".png", ".mp4", ".mov", ".webm", ".mp3", ".wav",
    ] {
        if lower.contains(pat) {
            return pat.trim_start_matches('.').to_string();
        }
    }
    fallback.to_string()
}

/// 从生成响应提取图片地址（多格式容错）：
/// - OpenAI 兼容：data[].b64_json / data[].url / data[].urls[] / data[].images[].url
/// - MiniMax image-01：data.image_urls[] / data[].url
///
/// b64 统一包装为 data URL 交给 save_asset 解码
fn extract_image_urls(resp: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push_url = |u: &str, out: &mut Vec<String>| {
        if !u.is_empty() && !out.contains(&u.to_string()) {
            out.push(u.to_string());
        }
    };
    if let Some(arr) = resp.get("data").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(b) = item.get("b64_json").and_then(|v| v.as_str()) {
                push_url(&format!("data:image/png;base64,{b}"), &mut out);
            }
            if let Some(u) = item.get("url").and_then(|v| v.as_str()) {
                push_url(u, &mut out);
            }
            if let Some(urls) = item.get("urls").and_then(|v| v.as_array()) {
                for u in urls.iter().filter_map(|v| v.as_str()) {
                    push_url(u, &mut out);
                }
            }
            if let Some(imgs) = item.get("images").and_then(|v| v.as_array()) {
                for img in imgs {
                    if let Some(u) = img.get("url").and_then(|v| v.as_str()) {
                        push_url(u, &mut out);
                    }
                }
            }
        }
    }
    if let Some(obj) = resp.get("data").and_then(|v| v.as_object()) {
        if let Some(urls) = obj.get("image_urls").and_then(|v| v.as_array()) {
            for u in urls.iter().filter_map(|v| v.as_str()) {
                push_url(u, &mut out);
            }
        }
    }
    if let Some(arr) = resp.get("images").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(u) = item.get("url").and_then(|v| v.as_str()) {
                push_url(u, &mut out);
            }
        }
    }
    out
}

/// 视频任务状态
enum VideoStatus {
    Pending,
    Succeeded,
    Failed,
}

/// 判定视频任务状态（兼容 status / task_status 两种字段，大小写不敏感）
fn video_status(resp: &Value) -> VideoStatus {
    let status = resp
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    match status.as_str() {
        "succeeded" | "success" | "done" | "completed" => return VideoStatus::Succeeded,
        "failed" | "failure" | "cancelled" | "canceled" | "expired" => return VideoStatus::Failed,
        _ => {}
    }
    let task = resp
        .get("task_status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    match task.as_str() {
        "success" | "succeeded" | "done" => VideoStatus::Succeeded,
        "fail" | "failed" | "cancelled" | "canceled" => VideoStatus::Failed,
        _ => VideoStatus::Pending,
    }
}

/// 从视频查询响应提取产物地址（多格式容错）：
/// - MiniMax：content[].video_url.url / content[].url
/// - 火山方舟：content.video_url（字符串或对象）/ content[].video_url
/// - 智谱：video_result[].url
fn extract_video_url(resp: &Value) -> Option<String> {
    if let Some(arr) = resp.get("content").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(vu) = item.get("video_url") {
                if let Some(u) = vu.get("url").and_then(|v| v.as_str()) {
                    return Some(u.to_string());
                }
                if let Some(u) = vu.as_str() {
                    return Some(u.to_string());
                }
            }
            if let Some(u) = item.get("url").and_then(|v| v.as_str()) {
                return Some(u.to_string());
            }
        }
    }
    if let Some(obj) = resp.get("content").and_then(|v| v.as_object()) {
        if let Some(vu) = obj.get("video_url") {
            if let Some(u) = vu.as_str() {
                return Some(u.to_string());
            }
            if let Some(u) = vu.get("url").and_then(|v| v.as_str()) {
                return Some(u.to_string());
            }
        }
    }
    if let Some(arr) = resp.get("video_result").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(u) = item.get("url").and_then(|v| v.as_str()) {
                return Some(u.to_string());
            }
        }
    }
    None
}

/// 图片生成（同步请求）
async fn generate_image(req: &GenRequest<'_>, style: GenEndpointStyle) -> Result<Vec<String>, String> {
    let base = req.base_url.trim_end_matches('/');
    let (url, body) = match style {
        GenEndpointStyle::MinimaxImage => (
            format!("{base}/v1/image_generation"),
            serde_json::json!({
                "model": req.model,
                "prompt": req.prompt,
                "n": 1,
            }),
        ),
        _ => (
            format!("{base}/images/generations"),
            serde_json::json!({
                "model": req.model,
                "prompt": req.prompt,
                "n": 1,
            }),
        ),
    };
    let resp = post_json(req.client, &url, req.api_key, &body)
        .send()
        .await
        .map_err(|e| format!("{}生成请求失败: {e}", req.kind.label()))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "{}生成失败 {}",
            req.kind.label(),
            err_text(status, &text)
        ));
    }
    let json: Value =
        serde_json::from_str(&text).map_err(|e| format!("响应解析失败: {e}"))?;
    let urls = extract_image_urls(&json);
    if urls.is_empty() {
        return Err(format!("{}生成成功但响应中无图片数据", req.kind.label()));
    }
    let mut files = Vec::new();
    for (i, u) in urls.iter().enumerate() {
        let ext = ext_from_url(u, req.kind.default_ext());
        files.push(
            save_asset(req.client, req.api_key, u, req.out_dir, &format!("gen-{i}"), &ext)
                .await?,
        );
    }
    Ok(files)
}

/// 视频生成（异步任务 + 轮询）
async fn generate_video(req: &GenRequest<'_>, style: GenEndpointStyle) -> Result<Vec<String>, String> {
    let base = req.base_url.trim_end_matches('/');
    let (submit_url, query_base, zhipu_style) = match style {
        GenEndpointStyle::ArkVideo => (
            format!("{base}/api/v3/contents/generations/tasks"),
            format!("{base}/api/v3/contents/generations/tasks/"),
            false,
        ),
        GenEndpointStyle::ZhipuVideo => (
            format!("{base}/videos/generations"),
            format!("{base}/async-result/"),
            true,
        ),
        _ => (
            format!("{base}/v2/video_generation"),
            format!("{base}/v2/query/video_generation/"),
            false,
        ),
    };
    // MiniMax H3 / 方舟 Seedance：content[] 多模态数组（文本 + 参考图）；
    // 智谱 CogVideoX：prompt + 可选 image_url
    let body = if zhipu_style {
        let mut b = serde_json::json!({ "model": req.model, "prompt": req.prompt });
        if let Some(img) = req.images.first() {
            b["image_url"] = serde_json::json!(img);
        }
        b
    } else {
        let mut content: Vec<Value> =
            vec![serde_json::json!({ "type": "text", "text": req.prompt })];
        for img in req.images {
            content.push(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": img }
            }));
        }
        serde_json::json!({ "model": req.model, "content": content })
    };
    let resp = post_json(req.client, &submit_url, req.api_key, &body)
        .send()
        .await
        .map_err(|e| format!("视频生成任务提交失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "视频生成任务提交失败 {}",
            err_text(status, &text)
        ));
    }
    let json: Value =
        serde_json::from_str(&text).map_err(|e| format!("响应解析失败: {e}"))?;
    let task_id = json
        .get("task_id")
        .or_else(|| json.get("id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "视频生成响应缺少 task_id".to_string())?;
    let query_url = format!("{query_base}{task_id}");

    // 轮询：10s 间隔、15min 上限；每轮检查停止请求并回调进度
    let mut waited: u64 = 0;
    loop {
        if (req.is_stopped)() {
            return Err("任务已停止".to_string());
        }
        tokio::time::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS)).await;
        waited += POLL_INTERVAL_SECS;
        (req.on_progress)(waited);
        if waited >= POLL_MAX_SECS {
            return Err(format!(
                "视频生成超时（已等待 {waited} 秒，上限 {} 秒），请稍后在服务商控制台查看",
                POLL_MAX_SECS
            ));
        }
        let text = match get_json(req.client, &query_url, req.api_key).send().await {
            Ok(r) if r.status().is_success() => r.text().await.unwrap_or_default(),
            Ok(r) => {
                return Err(format!(
                    "视频任务查询失败 HTTP {}",
                    r.status().as_u16()
                ))
            }
            Err(e) => return Err(format!("视频任务查询失败: {e}")),
        };
        let json: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            // 偶发非 JSON 响应（网关抖动）：继续等待下一轮
            Err(_) => continue,
        };
        match video_status(&json) {
            VideoStatus::Pending => continue,
            VideoStatus::Failed => {
                let reason = json
                    .get("error")
                    .map(|v| truncate(&v.to_string(), 160))
                    .unwrap_or_default();
                return Err(format!(
                    "视频生成失败{}",
                    if reason.is_empty() {
                        String::new()
                    } else {
                        format!("：{reason}")
                    }
                ));
            }
            VideoStatus::Succeeded => {
                let url = extract_video_url(&json)
                    .ok_or_else(|| "视频生成成功但响应中无视频地址".to_string())?;
                let ext = ext_from_url(&url, req.kind.default_ext());
                let path =
                    save_asset(req.client, req.api_key, &url, req.out_dir, "gen-0", &ext)
                        .await?;
                return Ok(vec![path]);
            }
        }
    }
}

/// 音频生成（同步请求）
async fn generate_audio(req: &GenRequest<'_>, style: GenEndpointStyle) -> Result<Vec<String>, String> {
    let base = req.base_url.trim_end_matches('/');
    let (bytes, ext): (Vec<u8>, &str) = match style {
        GenEndpointStyle::MinimaxT2a => {
            let url = format!("{base}/v1/t2a_v2");
            let body = serde_json::json!({
                "model": req.model,
                "text": req.prompt,
                "voice_setting": {
                    "voice_id": "male-qn-qingse",
                    "speed": 1.0,
                    "vol": 1.0,
                    "pitch": 0
                },
                "audio_setting": {
                    "sample_rate": 32000,
                    "bitrate": 128000,
                    "format": "mp3",
                    "channel": 1
                }
            });
            let resp = post_json(req.client, &url, req.api_key, &body)
                .send()
                .await
                .map_err(|e| format!("音频生成请求失败: {e}"))?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(format!(
                    "音频生成失败 {}",
                    err_text(status, &text)
                ));
            }
            let json: Value =
                serde_json::from_str(&text).map_err(|e| format!("响应解析失败: {e}"))?;
            let hex = json
                .pointer("/data/audio")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "音频生成响应缺少 data.audio".to_string())?;
            (hex_decode(hex).map_err(|e| format!("音频 hex 解码失败: {e}"))?, "mp3")
        }
        _ => {
            let url = format!("{base}/audio/speech");
            let body = serde_json::json!({
                "model": req.model,
                "input": req.prompt,
                "voice": "alloy",
                "response_format": "mp3"
            });
            let resp = post_json(req.client, &url, req.api_key, &body)
                .send()
                .await
                .map_err(|e| format!("音频生成请求失败: {e}"))?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(format!(
                    "音频生成失败 {}",
                    err_text(status, &text)
                ));
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| format!("音频读取失败: {e}"))?
                .to_vec();
            (bytes, "mp3")
        }
    };
    if bytes.is_empty() {
        return Err("音频生成结果为空".to_string());
    }
    std::fs::create_dir_all(req.out_dir).map_err(|e| format!("创建保存目录失败: {e}"))?;
    let path = req.out_dir.join(format!("gen-0.{ext}"));
    std::fs::write(&path, &bytes).map_err(|e| format!("音频保存失败: {e}"))?;
    Ok(vec![path.to_string_lossy().to_string()])
}

/// 十六进制字符串解码（MiniMax TTS 返回 hex 编码音频）
fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if !s.len().is_multiple_of(2) {
        return Err("hex 长度非法".to_string());
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for i in (0..bytes.len()).step_by(2) {
        let hi = hex_val(bytes[i]).ok_or_else(|| "非法 hex 字符".to_string())?;
        let lo = hex_val(bytes[i + 1]).ok_or_else(|| "非法 hex 字符".to_string())?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn endpoint_style_detection() {
        // MiniMax 域名 → 各自专用形态
        assert_eq!(
            endpoint_style_for("https://api.minimax.io", GenKind::Image),
            GenEndpointStyle::MinimaxImage
        );
        assert_eq!(
            endpoint_style_for("https://api.minimaxi.com", GenKind::Video),
            GenEndpointStyle::MinimaxVideo
        );
        assert_eq!(
            endpoint_style_for("https://api.minimax.io", GenKind::Audio),
            GenEndpointStyle::MinimaxT2a
        );
        // 方舟 / 智谱 → 视频专用形态
        assert_eq!(
            endpoint_style_for("https://ark.cn-beijing.volces.com/api/v3", GenKind::Video),
            GenEndpointStyle::ArkVideo
        );
        assert_eq!(
            endpoint_style_for("https://open.bigmodel.cn/api/paas/v4", GenKind::Video),
            GenEndpointStyle::ZhipuVideo
        );
        // 其余回退 OpenAI 兼容
        assert_eq!(
            endpoint_style_for("https://api.openai.com/v1", GenKind::Image),
            GenEndpointStyle::ImagesGenerations
        );
        assert_eq!(
            endpoint_style_for("https://api.siliconflow.cn/v1", GenKind::Audio),
            GenEndpointStyle::OpenaiSpeech
        );
    }

    #[test]
    fn image_urls_multi_format() {
        // OpenAI 兼容 data[].url
        let urls = extract_image_urls(&json!({"data": [{"url": "https://a.png"}]}));
        assert_eq!(urls, vec!["https://a.png"]);
        // b64_json → data URL 包装
        let urls = extract_image_urls(&json!({"data": [{"b64_json": "QUJD"}]}));
        assert_eq!(urls, vec!["data:image/png;base64,QUJD"]);
        // urls[] / images[].url
        let urls = extract_image_urls(&json!({"data": [{"urls": ["https://a.png", "https://b.png"]}]}));
        assert_eq!(urls.len(), 2);
        let urls = extract_image_urls(&json!({"data": [{"images": [{"url": "https://c.png"}]}]}));
        assert_eq!(urls, vec!["https://c.png"]);
        // MiniMax image-01：data.image_urls[]
        let urls = extract_image_urls(&json!({"data": {"image_urls": ["https://m.png"]}}));
        assert_eq!(urls, vec!["https://m.png"]);
        // 空响应
        assert!(extract_image_urls(&json!({"error": "x"})).is_empty());
    }

    #[test]
    fn video_status_and_url_extraction() {
        // MiniMax：status + content[].video_url.url
        assert!(matches!(
            video_status(&json!({"status": "Succeeded"})),
            VideoStatus::Succeeded
        ));
        assert!(matches!(
            video_status(&json!({"status": "Pending"})),
            VideoStatus::Pending
        ));
        assert!(matches!(
            video_status(&json!({"status": "Failed"})),
            VideoStatus::Failed
        ));
        // 智谱：task_status
        assert!(matches!(
            video_status(&json!({"task_status": "SUCCESS"})),
            VideoStatus::Succeeded
        ));
        assert!(matches!(
            video_status(&json!({"task_status": "PROCESSING"})),
            VideoStatus::Pending
        ));
        let url = extract_video_url(&json!({
            "content": [{"type": "video_url", "video_url": {"url": "https://v.mp4"}}]
        }));
        assert_eq!(url.as_deref(), Some("https://v.mp4"));
        // 方舟：content.video_url 字符串
        let url = extract_video_url(&json!({"content": {"video_url": "https://v2.mp4"}}));
        assert_eq!(url.as_deref(), Some("https://v2.mp4"));
        // 智谱：video_result[].url
        let url = extract_video_url(&json!({"video_result": [{"url": "https://v3.mp4"}]}));
        assert_eq!(url.as_deref(), Some("https://v3.mp4"));
        assert!(extract_video_url(&json!({"status": "Pending"})).is_none());
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(hex_decode("00ff10").unwrap(), vec![0x00, 0xff, 0x10]);
        assert_eq!(hex_decode("0F").unwrap(), vec![0x0f]);
        assert!(hex_decode("0").is_err());
        assert!(hex_decode("zz").is_err());
    }

    #[test]
    fn ext_from_url_guesses() {
        assert_eq!(ext_from_url("https://a/b.png", "png"), "png");
        assert_eq!(ext_from_url("https://a/b.mp4", "mp4"), "mp4");
        assert_eq!(ext_from_url("https://a/b", "png"), "png");
    }
}
