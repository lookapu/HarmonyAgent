//! 回复语言跟随：检测用户消息的主要语言，生成系统提示中的语言指令。
//!
//! 背景：此前系统提示硬编码“回答使用中文”，导致用户用英文/阿拉伯语等输入时
//! 模型仍输出中文。这里提供两级控制：
//! 1. 对话级 reply_language（ChatOptions）：用户显式指定回复语言（auto 或语言代码）；
//! 2. auto 时按 Unicode 字符区间统计用户消息的主要语言，检测不到（纯 ASCII/拉丁混排）
//!    则注入通用跟随指令，让模型自行对齐用户消息语言。

/// 支持显式指定的语言代码 → 提示中使用的语言名（中文名 + 自名，便于模型识别）
pub const LANGUAGES: &[(&str, &str)] = &[
    ("zh", "中文"),
    ("en", "英文（English）"),
    ("ar", "阿拉伯语（العربية）"),
    ("ja", "日语（日本語）"),
    ("ko", "韩语（한국어）"),
    ("ru", "俄语（Русский）"),
    ("fr", "法语（Français）"),
    ("de", "德语（Deutsch）"),
    ("es", "西班牙语（Español）"),
    ("pt", "葡萄牙语（Português）"),
    ("it", "意大利语（Italiano）"),
    ("th", "泰语（ไทย）"),
    ("vi", "越南语（Tiếng Việt）"),
    ("he", "希伯来语（עברית）"),
    ("hi", "印地语（हिन्दी）"),
];

/// 语言代码 → 展示名（未知代码回退空串）
pub fn language_display(code: &str) -> &'static str {
    LANGUAGES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, name)| *name)
        .unwrap_or("")
}

/// 检测文本的主要语言（按 Unicode 区间统计非 ASCII 字符占比）。
/// 仅返回字符特征足够明确的语言；纯 ASCII 或拉丁混排（无法可靠区分）返回 None，
/// 由调用方注入通用跟随指令。
pub fn detect_language(text: &str) -> Option<&'static str> {
    // (语言代码, 区间判断闭包)；各区间互斥，逐字符只命中一个
    let ranges: &[(&str, fn(char) -> bool)] = &[
        // 阿拉伯语（含补充平面常用区）
        ("ar", |c| matches!(c as u32, 0x0600..=0x06FF | 0x0750..=0x077F | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF)),
        // 希伯来语
        ("he", |c| matches!(c as u32, 0x0590..=0x05FF)),
        // 天城文（印地语等）
        ("hi", |c| matches!(c as u32, 0x0900..=0x097F)),
        // 西里尔（俄语等）
        ("ru", |c| matches!(c as u32, 0x0400..=0x04FF)),
        // 泰语
        ("th", |c| matches!(c as u32, 0x0E00..=0x0E7F)),
        // 日文假名（平假名 + 片假名；纯汉字归中文）
        ("ja", |c| matches!(c as u32, 0x3040..=0x30FF)),
        // 韩文（谚文音节 + 字母 + 兼容谚文）
        ("ko", |c| matches!(c as u32, 0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F)),
        // 中文（CJK 统一表意 + 扩展 A）
        ("zh", |c| matches!(c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF)),
        // 越南语（拉丁扩展附加区，字母特征明确）
        ("vi", |c| matches!(c as u32, 0x1EA0..=0x1EFF)),
    ];
    let mut counts: [u32; 9] = [0; 9];
    for ch in text.chars() {
        for (i, (_, f)) in ranges.iter().enumerate() {
            if f(ch) {
                counts[i] += 1;
                break;
            }
        }
    }
    let (best, max) = counts
        .iter()
        .enumerate()
        .max_by_key(|(_, n)| **n)
        .map(|(i, n)| (i, *n))
        .unwrap_or((0, 0));
    if max == 0 {
        None
    } else {
        Some(ranges[best].0)
    }
}

/// 生成系统提示中的语言指令（形如“回答与思考过程使用阿拉伯语（العربية），”）。
/// 同时约束正文与思考过程（reasoning_content）：推理模型的思考链同样跟随语言，
/// 避免正文英文、思考链却用中文的割裂体验。
/// - reply_language: 显式指定（auto 或缺省 = 跟随输入）
/// - text: 用于自动检测的用户消息原文
pub fn language_directive(reply_language: Option<&str>, text: &str) -> String {
    let explicit = reply_language
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| *s != "auto");
    if let Some(code) = explicit {
        let name = language_display(code);
        return if name.is_empty() {
            format!("回答与思考过程使用与用户消息相同的语言（代码 {code}），")
        } else {
            format!("回答与思考过程使用{name}，")
        };
    }
    match detect_language(text) {
        Some(code) => format!("回答与思考过程使用与用户消息相同的语言（{}），", language_display(code)),
        None => "回答与思考过程使用与用户最近一条消息相同的语言（用户消息是什么语言就用什么语言回复，禁止默认使用中文），".to_string(),
    }
}

/// 提示词**末尾**的语言强约束。
///
/// 中段那条 [`language_directive`] 会被上方上万字中文提示与每轮注入的中文账本淹没——
/// 本机实测：选了英文仍回简体中文，模型几乎总是跟随最近、最长的语言信号。因此语言要求
/// 必须同时出现在**最末**（离本轮输入最近），并显式禁止"因为系统提示是中文就回中文"。
/// 目标语言非中文时，明确要求正文/标题/清单/思考过程都不得出现中文（代码、命令、路径、
/// API 名与专有名词保持原样）。
pub fn language_footer(reply_language: Option<&str>, text: &str) -> String {
    let explicit = reply_language
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| *s != "auto");
    let code = match explicit {
        Some(c) => Some(c),
        None => detect_language(text),
    };
    let phrase = match code {
        Some(c) if !language_display(c).is_empty() => language_display(c).to_string(),
        Some(c) => format!("与用户消息相同的语言（代码 {c}）"),
        None => "与用户最近一条消息相同的语言".to_string(),
    };
    let avoid_chinese = !matches!(code, Some("zh"));
    let mut footer = format!("【回复语言 · 最高优先级 · 覆盖前文任何相反的表述】全程使用{phrase}。");
    if avoid_chinese {
        footer.push_str(
            "不要因为系统提示是中文就改用中文回复；正文、标题、清单与思考过程都不得出现中文。",
        );
    }
    footer.push_str("代码、命令、路径、API 名与专有名词保持原样，不要翻译或音译。");
    footer
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 末尾强约束：固定语言必须点名该语言，且非中文时显式禁止中文输出
    #[test]
    fn footer_names_explicit_language_and_forbids_chinese() {
        let f = language_footer(Some("en"), "帮我修一下构建错误");
        assert!(f.contains("English"), "应点名目标语言：{f}");
        assert!(f.contains("不得出现中文"), "非中文目标语言必须禁止中文输出：{f}");
    }

    #[test]
    fn footer_allows_chinese_target() {
        let f = language_footer(Some("zh"), "fix the build");
        assert!(f.contains("中文"), "目标语言为中文时应点名中文：{f}");
        assert!(!f.contains("不得出现中文"), "目标语言就是中文时不该禁止中文：{f}");
    }

    /// auto（或未设置）跟随用户消息语言：中文输入 → 中文，不禁止中文
    #[test]
    fn footer_follows_user_language_on_auto() {
        let zh = language_footer(None, "帮我看看这个报错");
        assert!(zh.contains("中文"), "中文输入应跟随中文：{zh}");
        // 纯 ASCII/拉丁文本按设计不判语言（无法与西语德语等区分），走"跟随用户最近一条消息"，
        // 但**禁止中文**的约束必须保留——否则中文系统提示会把回复带偏
        let latin = language_footer(Some("auto"), "please fix this build error");
        assert!(latin.contains("与用户最近一条消息相同"), "拉丁文本应回退为跟随：{latin}");
        assert!(latin.contains("不得出现中文"), "拉丁文本输入也必须禁止中文：{latin}");
    }

    #[test]
    fn detects_arabic() {
        assert_eq!(detect_language("أهلاً بك في دليل المطور"), Some("ar"));
        assert_eq!(detect_language("مرحبا كيف حالك"), Some("ar"));
    }

    #[test]
    fn detects_chinese_japanese_korean() {
        assert_eq!(detect_language("请帮我修复这个构建错误"), Some("zh"));
        assert_eq!(detect_language("こんにちは、助けてください"), Some("ja"));
        assert_eq!(detect_language("안녕하세요 도와주세요"), Some("ko"));
    }

    #[test]
    fn detects_cyrillic_thai() {
        assert_eq!(detect_language("Помогите исправить ошибку"), Some("ru"));
        assert_eq!(detect_language("ช่วยแก้ไขข้อผิดพลาด"), Some("th"));
    }

    #[test]
    fn ascii_or_latin_returns_none() {
        assert_eq!(detect_language("fix this build error please"), None);
        assert_eq!(detect_language("Comment allez-vous aujourd'hui"), None);
        // 代码为主时不应误判语言
        assert_eq!(detect_language("const x = 1; return build()"), None);
    }

    #[test]
    fn explicit_language_overrides_detection() {
        assert_eq!(
            language_directive(Some("ar"), "fix this"),
            "回答与思考过程使用阿拉伯语（العربية），"
        );
        assert_eq!(
            language_directive(Some("auto"), "أهلاً بك"),
            "回答与思考过程使用与用户消息相同的语言（阿拉伯语（العربية）），"
        );
        assert_eq!(
            language_directive(None, "fix this build error"),
            "回答与思考过程使用与用户最近一条消息相同的语言（用户消息是什么语言就用什么语言回复，禁止默认使用中文），"
        );
    }
}
