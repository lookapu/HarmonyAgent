//! media 子模块 — 按职责拆分（详见 quality_tools.rs facade）。
//!
//! 调用方式不变：quality_tools::xxx(...)，通过 quality_tools 内的 pub use re-export。

use super::*;
pub(super) async fn docx_read(
    args: &Value,
    roots: &[String],
) -> Result<String, String> {
    let path = args["path"]
        .as_str()
        .ok_or("docx_read 需要参数 {\"path\":\"<docx 路径>\"}")?;
    let resolved = resolve_in_roots(roots, path)?;
    let file = std::fs::File::open(&resolved)
        .map_err(|e| format!("打开 docx 失败: {e}"))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| format!("解析 zip 失败（docx 是 zip 格式）: {e}"))?;
    let mut doc_xml = String::new();
    {
        let mut entry = zip
            .by_name("word/document.xml")
            .map_err(|e| format!("读 word/document.xml 失败: {e}"))?;
        use std::io::Read;
        entry
            .read_to_string(&mut doc_xml)
            .map_err(|e| format!("读 xml 失败: {e}"))?;
    }
    // 简化提取：取 <w:t>...</w:t> 标签内的文本，段间换行（<w:p ...>...</w:p> 之间插入 \n）
    // 标签不跨多行的简单实现
    let mut out = String::new();
    let mut in_p = false;
    for line in doc_xml.lines() {
        let l = line.trim();
        if l.starts_with("<w:p ") || l == "<w:p>" {
            in_p = true;
            continue;
        }
        if l == "</w:p>" {
            in_p = false;
            out.push('\n');
            continue;
        }
        // 提 <w:t ...>text</w:t> 里的 text
        if let Some(start) = l.find("<w:t") {
            if let Some(gt_pos) = l[start..].find('>') {
                let after_gt = start + gt_pos + 1;
                if let Some(end) = l[after_gt..].find("</w:t>") {
                    let text = &l[after_gt..after_gt + end];
                    if !text.trim().is_empty() {
                        if in_p && !out.ends_with('\n') && !out.is_empty() {
                            // 段内：追加空格
                        }
                        out.push_str(text);
                    }
                }
            }
        }
    }
    let truncated = if out.chars().count() > 5000 {
        let s: String = out.chars().take(5000).collect();
        format!("{s}…\n[截断：超过 5000 字符]")
    } else {
        out
    };
    let para_count = truncated.lines().filter(|l| !l.trim().is_empty()).count();
    Ok(format!(
        "📄 DOCX 解析：{}\n段落数：{}\n\n{}",
        resolved.display(),
        para_count,
        truncated
    ))
}

pub(super) async fn audio_transcribe(
    args: &Value,
    roots: &[String],
) -> Result<String, String> {
    let path = args["path"]
        .as_str()
        .ok_or("audio_transcribe 需要参数 {\"path\":\"<音频路径>\"}")?;
    let resolved = resolve_in_roots(roots, path)?;
    if !resolved.exists() {
        return Err(format!("音频文件不存在: {}", resolved.display()));
    }

    // 找 whisper 二进制（PATH 优先，再找 resources/whisper/）
    let whisper_bin = find_whisper_binary().ok_or_else(|| {
        "未找到 whisper.cpp 二进制。请按以下步骤安装：\n  \
         1. git clone https://github.com/ggerganov/whisper.cpp\n  \
         2. cd whisper.cpp && make\n  \
         3. 把 main 二进制加到 PATH 或复制到 resources/whisper/\n  \
         4. 下载模型：bash models/download-ggml-model.sh base\n  \
         5. 放 ~/.cache/whisper/ggml-base.bin"
            .to_string()
    })?;
    let model = find_whisper_model().ok_or_else(|| {
        "未找到 whisper 模型。运行 bash models/download-ggml-model.sh base 并把 ggml-base.bin 放到 ~/.cache/whisper/".to_string()
    })?;

    // 转绝对路径 + 调命令行
    let audio_str = resolved.to_string_lossy().into_owned();
    let start = std::time::Instant::now();
    let out = std::process::Command::new(&whisper_bin)
        .arg("-m").arg(&model)
        .arg("-f").arg(&audio_str)
        .arg("--no-timestamps") // 简化输出
        .arg("-l").arg("auto")
        .output()
        .map_err(|e| format!("执行 whisper 失败: {e}（请确认二进制可执行）"))?;
    let elapsed = start.elapsed();
    if !out.status.success() {
        return Err(format!(
            "whisper 执行失败（退出码 {}）stderr: {}",
            out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "无".into()),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let truncated = if text.chars().count() > 4000 {
        let s: String = text.chars().take(4000).collect();
        format!("{s}…\n[截断：超过 4000 字符]")
    } else {
        text
    };
    let segments = truncated.lines().filter(|l| !l.trim().is_empty()).count();
    Ok(format!(
        "🎙️ 音频转写：{}\n二进制：{}\n模型：{}\n段数：{} / 耗时：{:.1}s\n\n{}",
        resolved.display(),
        whisper_bin,
        model,
        segments,
        elapsed.as_secs_f64(),
        truncated
    ))
}


fn find_whisper_binary() -> Option<String> {
    // PATH 优先
    for name in &["whisper", "main"] {
        if let Ok(out) = std::process::Command::new("which").arg(name).output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() { return Some(s); }
            }
        }
    }
    // resources/whisper/ 备选
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in &["whisper.exe", "whisper", "main"] {
                let p = dir.join("resources").join("whisper").join(name);
                if p.exists() { return Some(p.to_string_lossy().into_owned()); }
            }
        }
    }
    None
}


fn find_whisper_model() -> Option<String> {
    let candidates = [
        "~/.cache/whisper/ggml-base.bin",
        "~/.cache/whisper/ggml-small.bin",
        "~/whisper.cpp/models/ggml-base.bin",
    ];
    for c in candidates {
        if let Some(expanded) = expand_home(c) {
            if std::path::Path::new(&expanded).exists() {
                return Some(expanded);
            }
        }
    }
    None
}


fn expand_home(p: &str) -> Option<String> {
    if p.starts_with("~/") {
        if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
            return Some(format!("{}{}", home.to_string_lossy(), &p[1..]));
        }
    }
    Some(p.to_string())
}

