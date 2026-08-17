//! 轻量混合文本 token 预估（无第三方依赖，跨模型近似）。
//!
//! 用途：发送 LLM 请求前估算输入规模，用于上下文保护（窗口风险判断）、
//! 成本预估与预算门控。不需要精确到与实际 tokenizer 逐 token 一致，
//! 只要量级接近、且"宁可高估不可低估"即可——高估会提前压缩/门控，是安全方向。
//!
//! 近似依据（以 cl100k / 各主流 tokenizer 的实测量级为参考）：
//! - CJK（中日韩）字符：约 1 字符 ≈ 1 token（保守取 1，实际 0.6~1.2）
//! - 拉丁字母/数字/ASCII：约 4 字符 ≈ 1 token
//! - 每段连续文本（"词"）额外少量边界开销
//! - 每条消息固定角色/结构开销
//!
//! 全部向上取整，保证"宁可多算不可漏算"。

/// CJK 统一表意文字区段（含扩展 A/B、兼容表意、全角符号等常见中文字符）
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF    // CJK 统一表意文字
        | 0x3400..=0x4DBF  // CJK 扩展 A
        | 0x20000..=0x2A6DF// CJK 扩展 B
        | 0xF900..=0xFAFF  // CJK 兼容表意文字
        | 0x3000..=0x303F  // CJK 标点
        | 0xFF00..=0xFFEF  // 全角形式（含全角标点、全角字母）
    )
}

/// 估算一段文本的 token 数（单个字符串）。
pub fn estimate_text_tokens(text: &str) -> usize {
    let mut cjk = 0usize;
    let mut other = 0usize;
    // 记录连续非空白段的个数（每个"词"加 0.5 token 边界开销，折算为 ceil）
    let mut word_segs = 0usize;
    let mut in_word = false;
    for c in text.chars() {
        if is_cjk(c) {
            cjk += 1;
            in_word = true;
        } else if c.is_whitespace() {
            in_word = false;
        } else {
            other += 1;
            in_word = true;
        }
        // 段边界：从空白进入非空白时视为新词
    }
    // 重新按"空白分隔段"计数更直观
    let _ = in_word;
    let _ = word_segs;
    word_segs = text.split_whitespace().count();
    // CJK 1 字符 1 token；其余 4 字符 1 token（向上取整）
    let other_tokens = other.div_ceil(4);
    let seg_overhead = word_segs.div_ceil(2);
    cjk + other_tokens + seg_overhead + 4 // +4 固定开销（BOS/格式）
}

/// 估算一组 LLM 消息的 token 数。
/// `messages` 为 serde_json::Value 数组，每项含 "content"（文本）与可选 "role"。
/// 每条消息按 role/结构加少量固定开销，与 OpenAI 兼容协议的口径接近。
pub fn estimate_messages_tokens(messages: &[serde_json::Value]) -> usize {
    let mut total = 0usize;
    for m in messages {
        // content 可能是字符串或结构化数组（多模态）
        let content_tokens = match m.get("content") {
            Some(serde_json::Value::String(s)) => estimate_text_tokens(s),
            Some(serde_json::Value::Array(parts)) => parts
                .iter()
                .filter_map(|p| {
                    if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                        Some(estimate_text_tokens(t))
                    } else {
                        // 图片等非文本块：按 1200 token 估算（图片块大致开销）
                        None
                    }
                })
                .sum::<usize>()
                // 非文本块计入固定开销
                + parts
                    .iter()
                    .filter(|p| p.get("text").and_then(|t| t.as_str()).is_none())
                    .count()
                    * 1200,
            _ => 0,
        };
        total += content_tokens + 12; // 每条消息 role/name/结构开销
    }
    total + 32 // system 指令与协议固定开销
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_is_about_one_token_per_char() {
        let t = estimate_text_tokens("鸿蒙系统开发指南与示例代码");
        assert!(t >= 13, "中文 13 字至少 13 token，实际 {t}");
        assert!(t <= 20, "中文不应过估太多，实际 {t}");
    }

    #[test]
    fn english_about_four_chars_per_token() {
        let t = estimate_text_tokens("hello world this is a test message for token estimation");
        // 46 字母 + 10 词段开销 + 4 固定 ≈ 21 token；真实 tokenizer 约 12~14，
        // 我们"宁可高估"，量级接近且 ≥ 真实值即可。
        assert!(t >= 15 && t <= 25, "英文量级应在 15~25，实际 {t}");
    }

    #[test]
    fn mixed_text_sane_range() {
        let text = "调用 ohos.file.fs 的 openSync 打开文件，然后 readSync 读取内容，最后 closeSync 关闭。";
        let t = estimate_text_tokens(text);
        // 中文 30 字 + 英文标识符若干，量级 40 左右
        assert!(t >= 30 && t <= 60, "混合文本量级应在 30~60，实际 {t}");
    }

    #[test]
    fn messages_include_overhead() {
        let msgs = serde_json::json!([
            {"role": "user", "content": "帮我写一个读取文件的函数"},
            {"role": "assistant", "content": "好的，下面是示例代码"}
        ]);
        let t = estimate_messages_tokens(msgs.as_array().unwrap());
        assert!(t >= 20, "两条消息至少 20 token，实际 {t}");
    }

    #[test]
    fn estimate_never_zero_for_empty() {
        assert!(estimate_text_tokens("") >= 4);
        assert!(estimate_messages_tokens(&[]) >= 32);
    }
}
