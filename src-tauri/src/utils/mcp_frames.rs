//! MCP stdio 帧协议编解码：兼容两种官方帧格式。
//!
//! - **newline-delimited JSON**（MCP 规范 2025-06-18 起，SDK 1.x 中后期）：`{json}\n`
//! - **Content-Length 帧**（MCP 规范 2025-03-26 时期，SDK 0.8~1.2）：`Content-Length: N\r\n\r\n{json}`
//!
//! 现代服务器（如 mongodb-mcp-server 1.14 / SDK 1.30）按行解析；旧服务器按
//! Content-Length 帧解析。客户端无法预先知道服务器格式，因此发送默认走
//! NDJSON（覆盖面最广：早期 0.x 与 2025-06-18 之后均用此格式），超时后再
//! 回退 Content-Length；接收时自动检测两种格式，并跳过通知帧（无 id）。

use serde_json::Value;

/// 帧编码模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameMode {
    /// newline-delimited JSON（`{json}\n`）
    Ndjson,
    /// Content-Length 帧（`Content-Length: N\r\n\r\n{json}`）
    ContentLength,
}

impl FrameMode {
    /// 模式序号（用于进程内共享状态记录）
    pub fn code(self) -> u8 {
        match self {
            FrameMode::Ndjson => 1,
            FrameMode::ContentLength => 2,
        }
    }

    pub fn from_code(code: u8) -> Option<FrameMode> {
        match code {
            1 => Some(FrameMode::Ndjson),
            2 => Some(FrameMode::ContentLength),
            _ => None,
        }
    }
}

/// 按指定模式编码一帧 JSON-RPC 消息
pub fn encode_frame(body: &str, mode: FrameMode) -> Vec<u8> {
    match mode {
        FrameMode::Ndjson => {
            let mut v = Vec::with_capacity(body.len() + 1);
            v.extend_from_slice(body.as_bytes());
            v.push(b'\n');
            v
        }
        FrameMode::ContentLength => {
            format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
        }
    }
}

/// 判断缓冲区是否以（忽略大小写与前导空白）`Content-Length:` 开头
fn starts_with_content_length(buf: &[u8]) -> bool {
    const PREFIX: &[u8] = b"content-length:";
    let mut i = 0;
    while i < buf.len() && (buf[i] == b' ' || buf[i] == b'\t' || buf[i] == b'\r' || buf[i] == b'\n') {
        i += 1;
    }
    if buf.len() - i < PREFIX.len() {
        return false;
    }
    buf[i..i + PREFIX.len()]
        .iter()
        .zip(PREFIX.iter())
        .all(|(a, b)| a.to_ascii_lowercase() == *b)
}

/// 从缓冲区解析一帧消息。
///
/// 返回 `Ok((json, 消耗字节数))`：解析成功（注意：若解析到的是通知帧，调用方
/// 需自行跳过——由读取循环判断 `json.get("id")`）；返回 `Err`：帧数据非法；
/// 返回 `None`：数据不足，需等待更多输入。
///
/// 自动检测帧格式：缓冲区以 `Content-Length:` 开头走帧解析，否则按行解析。
pub fn try_parse_frame(buf: &[u8]) -> Option<Result<(Value, usize), String>> {
    if starts_with_content_length(buf) {
        // Content-Length 帧：找头尾空行
        let header_end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
        let header = String::from_utf8_lossy(&buf[..header_end]);
        let content_len: usize = header.lines().find_map(|l| {
            let mut it = l.split(':');
            let key = it.next()?.trim();
            if key.eq_ignore_ascii_case("content-length") {
                it.next()?.trim().parse().ok()
            } else {
                None
            }
        })?;
        let body_start = header_end + 4;
        if buf.len() < body_start + content_len {
            return None; // 响应体未收全
        }
        return Some(
            serde_json::from_slice::<Value>(&buf[body_start..body_start + content_len])
                .map(|v| (v, body_start + content_len))
                .map_err(|e| format!("响应 JSON 解析失败: {e}")),
        );
    }

    // newline-delimited JSON：按 \n 切行（JSON.stringify 会将字符串内换行转义为 \n 字面量，
    // 因此一行即一条完整消息）
    let line_end = buf.iter().position(|&b| b == b'\n')?;
    let mut line = &buf[..line_end];
    if line.ends_with(b"\r") {
        line = &line[..line.len() - 1];
    }
    // 空行与非 JSON 行（如进程横幅/警告误入 stdout）：以 Value::Null 标记跳过，
    // 不中断协议解析（由调用方统计跳过行数，持续大量垃圾输出时才判定服务器异常）
    if line.is_empty() {
        return Some(Ok((Value::Null, line_end + 1)));
    }
    Some(match serde_json::from_slice::<Value>(line) {
        Ok(v) => Ok((v, line_end + 1)),
        Err(_) => Ok((Value::Null, line_end + 1)),
    })
}
