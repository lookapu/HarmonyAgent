//! LLM 请求录制/重放（无 key 回归 agent 行为；dsh llm-replay 方案的最小落地）
//!
//! 环境变量 `DEVS_LLM_REPLAY` 控制：
//! - `record:<目录>`：每个 LLM 请求（主循环流式 + 子 Agent 非流式）把请求指纹与完整
//!   响应原文追加写入 `<目录>/replay.jsonl`（JSONL：key/model/text/reasoning）。
//!   流式响应录制的是**原始 SSE 文本流**（含 reasoning delta），重放时可完整还原。
//! - `replay:<目录>`：按请求指纹命中录制响应直接返回，不发起真实请求
//!   （fail-closed：未命中报错，保证回归确定性，避免测试静默打到真实 API）。
//!
//! 指纹 = model + 消息序列 SHA-256 前 16 位（消息含工具结果，逐轮不同 → 轮级精确匹配）。

use std::io::Write as _;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

/// 重放模式（从 DEVS_LLM_REPLAY 解析）
#[derive(Clone, Debug, PartialEq)]
pub enum ReplayMode {
    /// 未启用
    Off,
    /// 录制：正常发送 + 落盘响应原文
    Record(String),
    /// 重放：命中录制响应，不发送真实请求
    Replay(String),
}

pub fn mode() -> ReplayMode {
    parse_mode(std::env::var("DEVS_LLM_REPLAY").unwrap_or_default().as_str())
}

/// 解析模式（独立函数便于单测）：record:dir / replay:dir / 其他=Off
fn parse_mode(v: &str) -> ReplayMode {
    if let Some(dir) = v.strip_prefix("record:") {
        ReplayMode::Record(dir.to_string())
    } else if let Some(dir) = v.strip_prefix("replay:") {
        ReplayMode::Replay(dir.to_string())
    } else {
        ReplayMode::Off
    }
}

/// 请求指纹：model + 消息序列哈希（消息含工具结果逐轮不同 → 轮级匹配）
pub fn request_key(model: &str, messages: &[serde_json::Value]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(model.as_bytes());
    for m in messages {
        hasher.update(b"\x1e");
        hasher.update(serde_json::to_string(m).unwrap_or_default().as_bytes());
    }
    let digest = hasher.finalize();
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// 录制条目（JSONL 一行一条）
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ReplayEntry {
    pub key: String,
    pub model: String,
    /// 完整响应原文（流式=SSE 文本流；非流式=最终文本）
    pub text: String,
    /// 思考过程（非流式录制时可为空；流式原文已含 reasoning delta）
    pub reasoning: String,
}

fn replay_path(dir: &str) -> PathBuf {
    PathBuf::from(dir).join("replay.jsonl")
}

/// 重放命中查找（每次读文件：录制规模小、保证录制后立即可见；同 key 重复录制取最新一条）
pub fn lookup(dir: &str, key: &str) -> Option<ReplayEntry> {
    let content = std::fs::read_to_string(replay_path(dir)).ok()?;
    let mut hit: Option<ReplayEntry> = None;
    for line in content.lines() {
        if let Ok(e) = serde_json::from_str::<ReplayEntry>(line) {
            if e.key == key {
                // 覆盖式：同 key 多次录制时取最新一次运行的真实响应
                hit = Some(e);
            }
        }
    }
    hit
}

/// 追加录制一条请求-响应（目录不存在时创建；失败静默，录制不应阻断正常请求）
pub fn record(dir: &str, key: &str, model: &str, text: &str, reasoning: &str) {
    let path = replay_path(dir);
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let entry = ReplayEntry {
            key: key.to_string(),
            model: model.to_string(),
            text: text.to_string(),
            reasoning: reasoning.to_string(),
        };
        let _ = writeln!(f, "{}", serde_json::to_string(&entry).unwrap_or_default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mode_variants() {
        assert_eq!(parse_mode(""), ReplayMode::Off);
        assert_eq!(parse_mode("record:/tmp/r"), ReplayMode::Record("/tmp/r".into()));
        assert_eq!(parse_mode("replay:./rec"), ReplayMode::Replay("./rec".into()));
        assert_eq!(parse_mode("weird"), ReplayMode::Off);
    }

    #[test]
    fn key_is_deterministic_and_sensitive_to_messages() {
        let msgs = vec![
            serde_json::json!({"role": "user", "content": "你好"}),
            serde_json::json!({"role": "assistant", "content": "hi"}),
        ];
        let k1 = request_key("deepseek-chat", &msgs);
        let k2 = request_key("deepseek-chat", &msgs);
        assert_eq!(k1, k2, "同输入必须同指纹");
        assert_eq!(k1.len(), 16);
        // 消息变化（工具结果注入）→ 指纹变化（轮级匹配）
        let mut msgs2 = msgs.clone();
        msgs2.push(serde_json::json!({"role": "user", "content": "[工具执行结果 - read_file] xxx"}));
        assert_ne!(k1, request_key("deepseek-chat", &msgs2), "消息序列变化必须换指纹");
        // 模型变化 → 指纹变化
        assert_ne!(k1, request_key("other-model", &msgs));
    }

    #[test]
    fn record_then_lookup_roundtrip() {
        let dir = std::env::temp_dir().join(format!("llm-replay-test-{}", uuid::Uuid::new_v4()));
        let key = "abc123";
        record(&dir.to_string_lossy(), key, "m1", "data: {\"x\":1}\n\ndata: [DONE]\n", "");
        // 同 key 追加不冲突（重复录制取最新一条）
        record(&dir.to_string_lossy(), key, "m1", "data: updated\n\ndata: [DONE]\n", "");
        let e = lookup(&dir.to_string_lossy(), key).expect("应命中");
        assert_eq!(e.model, "m1");
        assert!(e.text.contains("updated"), "重复录制应取最后一条");
        assert!(lookup(&dir.to_string_lossy(), "nope").is_none(), "未录制 key 不命中");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
