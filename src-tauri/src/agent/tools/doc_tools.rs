//! 文档与图像域工具：view_image（图片进模型多模态视野）/ read_document（docx/pptx/xlsx/pdf/文本 → 纯文本）。
//! 共享辅助函数（resolve_in_roots / truncate_out_head_tail 等）在父模块 mod.rs，通过 `use super::*` 继承。

use super::*;

/// view_image：读取项目内图片并让模型直接看到（多模态）。
/// 复用截图视觉闭环：输出 [VISION_IMAGE: <路径>] 标记，由 chat.rs 剥离并编码为 data URL，
/// 下一轮请求时随工具结果注入模型视野。本工具执行时先做一次解码压缩验证，
/// 失败直接报错，避免标记被剥离后模型"以为看到却看不到"。
pub(super) async fn view_image(args: &Value, roots: &[String]) -> Result<String, String> {
    let raw = args["path"].as_str().unwrap_or("").trim();
    if raw.is_empty() {
        return Err("view_image 需要参数 {\"path\":\"<图片路径，相对项目根或绝对路径>\"}".into());
    }
    let p = resolve_in_roots(roots, raw)?;
    if !p.is_file() {
        return Err(format!("路径不是文件: {}", p.display()));
    }
    let meta = std::fs::metadata(&p).map_err(|e| e.to_string())?;
    if meta.len() > 20 * 1024 * 1024 {
        return Err(format!(
            "图片过大（{}，>20MB），无法编码进模型视野",
            crate::agent::tools::fs_tools::human_size(meta.len())
        ));
    }
    // 预编码验证（与 chat.rs 视觉闭环同源）：解码失败直接报错而非只给路径。
    // 图像解码+压缩为 CPU 密集操作（数百 ms），放 spawn_blocking 避免钉死 tokio worker。
    let p_buf = p.clone();
    let (data_url, w, h) = tokio::task::spawn_blocking(move || {
        let data_url = encode_vision_image(&p_buf)?;
        let bytes = std::fs::read(&p_buf).map_err(|e| format!("读取图片失败: {e}"))?;
        let img = image::load_from_memory(&bytes).map_err(|e| format!("图片解码失败: {e}"))?;
        Ok::<(String, u32, u32), String>((data_url, img.width(), img.height()))
    })
    .await
    .map_err(|e| format!("视觉编码任务异常: {e}"))??;
    Ok(format!(
        "已读取图片: {}\n（分辨率 {}x{}，原大小 {}，压缩后约 {:.0}KB，随下轮请求进入模型视野）\n[VISION_IMAGE: {}]",
        p.display(),
        w,
        h,
        crate::agent::tools::fs_tools::human_size(meta.len()),
        data_url.len() as f64 / 1024.0,
        p.display()
    ))
}

/// chart_extract：从图表截图/设计图中提取结构化数据（视觉模型读图，复用 view_image 的多模态闭环）。
/// 参数：{"path":"<单张图表路径>"} 或 {"charts":["<多张图表路径>"]}，
/// {"format":"table|json|csv"（缺省 table：Markdown 表格）,"focus":"<可选，提取重点，如 只看 2024 年数据>",
///  "title":"<可选，图表说明，辅助模型理解图意>"}。
/// 执行时先做解码压缩验证，失败直接报错；成功后每张图随下轮请求进入模型视野，
/// 并附提取要求，模型必须按指定格式输出数据（含列名/单位/系列），不可用自然语言代替。
pub(super) async fn chart_extract(args: &Value, roots: &[String]) -> Result<String, String> {
    // 1) 收集图片路径（单图 path 或 charts 数组）
    let mut paths: Vec<String> = Vec::new();
    if let Some(p) = args["path"].as_str() {
        paths.push(p.to_string());
    }
    if let Some(arr) = args["charts"].as_array() {
        for v in arr {
            if let Some(p) = v.as_str() {
                paths.push(p.to_string());
            }
        }
    }
    if paths.is_empty() {
        return Err("chart_extract 需要参数 {\"path\":\"<图表图片路径>\"} 或 {\"charts\":[\"<多张>\"]}".into());
    }
    if paths.len() > 8 {
        return Err(format!("一次最多提取 8 张图（收到 {} 张），可分批调用", paths.len()));
    }
    let format = args["format"].as_str().unwrap_or("table");
    if !["table", "json", "csv"].contains(&format) {
        return Err(format!("format 只接受 table|json|csv（收到 {format}）"));
    }
    let focus = args["focus"].as_str().map(str::trim).filter(|s| !s.is_empty());
    let hint = args["title"].as_str().map(str::trim).filter(|s| !s.is_empty());

    // 2) 逐张验证解码（与 view_image 同一视觉闭环，失败直接报错避免"以为看到却看不到"）
    let mut out = format!("图表数据提取任务（{} 张图，输出格式：{}）：\n", paths.len(), format);
    let mut markers = Vec::new();
    for (i, raw) in paths.iter().enumerate() {
        let p = resolve_in_roots(roots, raw)?;
        if !p.is_file() {
            return Err(format!("路径不是文件: {}", p.display()));
        }
        let meta = std::fs::metadata(&p).map_err(|e| e.to_string())?;
        if meta.len() > 20 * 1024 * 1024 {
            return Err(format!(
                "图片过大（{}，>20MB），无法编码进模型视野",
                crate::agent::tools::fs_tools::human_size(meta.len())
            ));
        }
        // 图像解码+压缩为 CPU 密集操作（每张数百 ms），放 spawn_blocking 避免钉死
        // tokio worker（timer driver 停转 → 流式超时全部失效）。闭包内一次完成
        // 编码与宽高解析，避免二次解码。
        let p_buf = p.clone();
        let (data_url, w, h) = tokio::task::spawn_blocking(move || {
            let data_url = encode_vision_image(&p_buf)?;
            let bytes = std::fs::read(&p_buf).map_err(|e| format!("读取图片失败: {e}"))?;
            let img = image::load_from_memory(&bytes).map_err(|e| format!("图片解码失败: {e}"))?;
            Ok::<(String, u32, u32), String>((data_url, img.width(), img.height()))
        })
        .await
        .map_err(|e| format!("视觉编码任务异常: {e}"))??;
        out.push_str(&format!(
            "  图[{}]: {}（{}x{}，原 {}，压缩后约 {:.0}KB）\n",
            i + 1,
            p.display(),
            w,
            h,
            crate::agent::tools::fs_tools::human_size(meta.len()),
            data_url.len() as f64 / 1024.0
        ));
        markers.push(p.display().to_string());
    }
    out.push_str("\n提取要求：请逐张查看下方图片，输出每张图的完整数据（不得省略行/列、不得臆造数据）：\n");
    if let Some(h) = hint {
        out.push_str(&format!("  - 图表说明：{h}\n"));
    }
    if let Some(f) = focus {
        out.push_str(&format!("  - 提取重点：{f}\n"));
    }
    out.push_str(match format {
        "json" => "  - 每张图输出一个 JSON 对象：{\"title\":\"图标题\",\"axes\":{...},\"series\":[{\"name\":...,\"points\":[...]}]}\n",
        "csv" => "  - 每张图输出一段 CSV（首行列名，含单位；多张图间用空行分隔）\n",
        _ => "  - 每张图输出一个 Markdown 表格（首行列名含单位，保留全部数据行）\n",
    });
    out.push_str("  输出以 \"【图N数据】\" 开头标注对应图片编号。\n");
    for m in markers {
        out.push_str(&format!("\n[VISION_IMAGE: {}]", m));
    }
    Ok(out)
}

/// read_document：读取文档文件为纯文本（docx/pptx/xlsx/pdf 及常见文本格式）。
pub(super) async fn read_document(args: &Value, roots: &[String]) -> Result<String, String> {
    let raw = args["path"].as_str().unwrap_or("").trim();
    if raw.is_empty() {
        return Err(
            "read_document 需要参数 {\"path\":\"<文档路径，相对项目根或绝对路径>\"}".into(),
        );
    }
    let p = resolve_in_roots(roots, raw)?;
    let text = extract_document_text(&p)?;
    // 保头保尾截断：前 60% 保留文档开头（标题/摘要），后 40% 保留结尾（结论/签名）
    Ok(truncate_out_head_tail(&text, 8000))
}

/// 按扩展名把文档文件解析为纯文本（docx/pptx/xlsx/pdf 及常见文本格式）。
/// 供 Agent 工具 read_document 与前端预览 command 共用；含 50MB 大小保护。
pub(crate) fn extract_document_text(p: &Path) -> Result<String, String> {
    if !p.is_file() {
        return Err(format!("路径不是文件: {}", p.display()));
    }
    let meta = std::fs::metadata(p).map_err(|e| e.to_string())?;
    if meta.len() > 50 * 1024 * 1024 {
        return Err(format!(
            "文档过大（{}，>50MB），无法读取",
            crate::agent::tools::fs_tools::human_size(meta.len())
        ));
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "docx" => extract_docx(p),
        "pptx" => extract_pptx(p),
        "xlsx" => extract_xlsx(p),
        "pdf" => extract_pdf(p),
        "txt" | "md" | "markdown" | "json" | "json5" | "csv" | "xml" | "html" | "htm"
        | "log" | "ini" | "conf" | "cfg" | "yaml" | "yml" | "toml" | "properties" | "env"
        | "bat" | "cmd" | "ps1" | "sh" => read_text_doc(p),
        _ => Err(format!(
            "暂不支持的文档格式: .{ext}（支持 docx/pptx/xlsx/pdf/txt/md/csv 及常见文本格式）"
        )),
    }
}

// ---------- 文本类 ----------

/// 文本类文档读取：UTF-8 严格校验 → GBK 回退（Windows 老文档）→ lossy 兜底。
/// 供 Agent read_document 与前端预览 command 共用。
pub(crate) fn read_text_doc(p: &Path) -> Result<String, String> {
    let bytes = std::fs::read(p).map_err(|e| format!("读取文件失败: {e}"))?;
    if bytes[..bytes.len().min(4096)].contains(&0) {
        return Err("文件是二进制，不是文本文档".into());
    }
    let text = match std::str::from_utf8(&bytes) {
        Ok(s) => s.strip_prefix('\u{feff}').unwrap_or(s).to_string(),
        Err(_) => {
            let (t, _, had_err) = encoding_rs::GBK.decode(&bytes);
            if had_err {
                String::from_utf8_lossy(&bytes).to_string()
            } else {
                t.into_owned()
            }
        }
    };
    Ok(text)
}

// ---------- OOXML（docx/pptx/xlsx 均为 zip 容器） ----------

/// 精确查找 `<name` 标签起始：标签名必须完整（后随 > 或空白），
/// 避免 `<w:t` 误匹配 `<w:tc>`/`<w:tbl>`/`<w:tr>` 等前缀相同的标签。
fn find_tag(rest: &str, name: &str) -> Option<usize> {
    let mut idx = 0;
    while let Some(p) = rest[idx..].find(name) {
        let pos = idx + p;
        let after_name = &rest[pos + name.len()..];
        match after_name.chars().next() {
            Some(c) if c == '>' || c == ' ' || c == '\t' || c == '\r' || c == '\n' => {
                return Some(pos)
            }
            _ => idx = pos + name.len(),
        }
    }
    None
}

/// 读取 zip 内指定条目为文本（缺失/损坏给出明确错误）
fn read_zip_text(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Result<String, String> {
    let mut f = zip
        .by_name(name)
        .map_err(|e| format!("压缩包内缺少 {name}（{e}）"))?;
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut f, &mut buf).map_err(|e| format!("读取 {name} 失败: {e}"))?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// docx 正文提取：按 <w:p> 段落切分，段内取所有 <w:t> 文本，tab/br 转义为制表符/换行。
fn extract_docx(p: &Path) -> Result<String, String> {
    let file = std::fs::File::open(p).map_err(|e| format!("打开文件失败: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("docx 不是有效压缩包: {e}"))?;
    let xml = read_zip_text(&mut zip, "word/document.xml")?;
    let mut paras: Vec<String> = Vec::new();
    let mut rest = xml.as_str();
    while let Some(start) = find_tag(rest, "<w:p") {
        let after = &rest[start..];
        let end = after
            .find("</w:p>")
            .ok_or("docx 段落结构异常（缺少 </w:p>）")?;
        paras.push(para_text(&after[..end]));
        rest = &after[end + "</w:p>".len()..];
    }
    if paras.is_empty() {
        return Ok("（文档正文为空）".into());
    }
    Ok(paras.join("\n"))
}

/// 段落 XML → 文本：<w:tab/> → 制表符、<w:br/> → 换行，取 <w:t> 内文并反转义 XML 实体
fn para_text(para: &str) -> String {
    let p1 = para.replace("<w:tab/>", "\t").replace("<w:br/>", "\n");
    let mut out = String::new();
    let mut rest = p1.as_str();
    while let Some(start) = find_tag(rest, "<w:t") {
        // 段内换行/制表符标记（位于 <w:t> 之外）补进输出，避免丢失
        let gap = &rest[..start];
        if gap.contains('\n') {
            out.push('\n');
        }
        if gap.contains('\t') {
            out.push('\t');
        }
        let after = &rest[start..];
        if let Some(gt) = after.find('>') {
            let inner = gt + 1;
            if let Some(close) = after[inner..].find("</w:t>") {
                out.push_str(&xml_unescape(&after[inner..inner + close]));
                rest = &after[inner + close + "</w:t>".len()..];
                continue;
            }
        }
        rest = &after["<w:t".len()..];
    }
    out
}

/// pptx 文本提取：按 slideN.xml 数字序逐页取 <a:t> 文本，页间/段间补换行。
fn extract_pptx(p: &Path) -> Result<String, String> {
    let file = std::fs::File::open(p).map_err(|e| format!("打开文件失败: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("pptx 不是有效压缩包: {e}"))?;
    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.name_for_index(i).map(|n| n.to_string()))
        .collect();
    let mut slides: Vec<(u32, String)> = Vec::new();
    for name in &names {
        if let Some(num) = parse_slide_number(name) {
            let xml = read_zip_text(&mut zip, name)?;
            slides.push((num, xml));
        }
    }
    slides.sort_by_key(|(num, _)| *num);
    if slides.is_empty() {
        return Ok("（文档无幻灯片内容）".into());
    }
    let mut out = String::new();
    for (num, xml) in &slides {
        out.push_str(&format!("\n\n--- 第 {num} 页 ---\n"));
        out.push_str(&extract_a_t(xml));
    }
    Ok(out.trim().to_string())
}

fn parse_slide_number(name: &str) -> Option<u32> {
    let n = name.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?;
    n.parse::<u32>().ok()
}

/// 提取 <a:t> 内文；<a:p> 段落边界与 <a:br/> 换行处补 \n
fn extract_a_t(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(start) = find_tag(rest, "<a:t") {
        let gap = &rest[..start];
        if gap.contains("</a:p>") || gap.contains("<a:br/>") {
            out.push('\n');
        }
        let after = &rest[start..];
        if let Some(gt) = after.find('>') {
            let inner = gt + 1;
            if let Some(close) = after[inner..].find("</a:t>") {
                out.push_str(&xml_unescape(&after[inner..inner + close]));
                rest = &after[inner + close + "</a:t>".len()..];
                continue;
            }
        }
        rest = &after["<a:t".len()..];
    }
    out.trim().to_string()
}

/// xlsx 文本提取：sharedStrings 共享字符串 + 各 sheet 单元格文本（行内制表符分隔）。
fn extract_xlsx(p: &Path) -> Result<String, String> {
    let file = std::fs::File::open(p).map_err(|e| format!("打开文件失败: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("xlsx 不是有效压缩包: {e}"))?;
    // 1. 共享字符串表（可能不存在：纯数字表）
    let shared: Vec<String> = match zip.by_name("xl/sharedStrings.xml") {
        Ok(mut f) => {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut f, &mut buf)
                .map_err(|e| format!("读取 sharedStrings 失败: {e}"))?;
            parse_shared_strings(&String::from_utf8_lossy(&buf))
        }
        Err(_) => Vec::new(),
    };
    // 2. 各工作表（按 sheetN.xml 数字序）
    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.name_for_index(i).map(|n| n.to_string()))
        .collect();
    let mut sheets: Vec<(u32, String)> = Vec::new();
    for name in &names {
        if let Some(num) = parse_sheet_number(name) {
            let xml = read_zip_text(&mut zip, name)?;
            sheets.push((num, xml));
        }
    }
    sheets.sort_by_key(|(num, _)| *num);
    if sheets.is_empty() {
        return Ok("（文档无工作表内容）".into());
    }
    let mut out = String::new();
    for (num, xml) in &sheets {
        out.push_str(&format!("\n--- 工作表 {num} ---\n"));
        out.push_str(&parse_sheet(&shared, xml));
    }
    Ok(out.trim().to_string())
}

fn parse_sheet_number(name: &str) -> Option<u32> {
    let n = name
        .strip_prefix("xl/worksheets/sheet")?
        .strip_suffix(".xml")?;
    n.parse::<u32>().ok()
}

/// <si> 列表解析：每个 <si> 内可能多个 <t>（富文本 run）拼接为一个字符串
fn parse_shared_strings(xml: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut rest = xml;
    while let Some(start) = find_tag(rest, "<si") {
        let after = &rest[start..];
        let end = match after.find("</si>") {
            Some(e) => e,
            None => break,
        };
        let si = &after[..end];
        let mut text = String::new();
        let mut r = si;
        while let Some(ts) = find_tag(r, "<t") {
            let ta = &r[ts..];
            if let Some(gt) = ta.find('>') {
                let inner = gt + 1;
                if let Some(tc) = ta[inner..].find("</t>") {
                    text.push_str(&xml_unescape(&ta[inner..inner + tc]));
                    r = &ta[inner + tc + "</t>".len()..];
                    continue;
                }
            }
            r = &ta["<t".len()..];
        }
        items.push(text);
        rest = &after[end + "</si>".len()..];
    }
    items
}

/// 工作表 XML → 文本：按 <row> 切行，行内 <c> 单元格取文本，单元格间制表符分隔
fn parse_sheet(shared: &[String], xml: &str) -> String {
    let mut rows: Vec<String> = Vec::new();
    let mut rest = xml;
    while let Some(start) = find_tag(rest, "<row") {
        let after = &rest[start..];
        let end = match after.find("</row>") {
            Some(e) => e,
            None => break,
        };
        rows.push(parse_row(shared, &after[..end]));
        rest = &after[end + "</row>".len()..];
    }
    rows.join("\n")
}

/// 行 XML → 文本：单元格文本（共享字符串索引/内联字符串/数字原文）
fn parse_row(shared: &[String], row_xml: &str) -> String {
    let mut cells: Vec<String> = Vec::new();
    let mut rest = row_xml;
    while let Some(start) = find_tag(rest, "<c") {
        let after = &rest[start..];
        let tag_end = match after.find('>') {
            Some(e) => e,
            None => break,
        };
        let tag = &after[..tag_end];
        let is_shared = tag.contains("t=\"s\"");
        let is_inline = tag.contains("t=\"inlineStr\"");
        let inner = &after[tag_end + 1..];
        let close = match inner.find("</c>") {
            Some(c) => c,
            None => break,
        };
        let body = &inner[..close];
        let cell_text = if is_shared {
            // <v>索引</v> → 共享字符串表取值
            if let Some(vs) = body.find("<v>") {
                let v = &body[vs + 3..];
                let ve = v.find("</v>").unwrap_or(0);
                v[..ve]
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| shared.get(i).cloned())
                    .unwrap_or_default()
            } else {
                String::new()
            }
        } else if is_inline {
            // 内联字符串：<is><t>…</t></is>
            extract_first_t(body)
        } else {
            // 数字/公式结果/布尔/日期：取 <v> 原文
            if let Some(vs) = body.find("<v>") {
                let v = &body[vs + 3..];
                let ve = v.find("</v>").unwrap_or(0);
                v[..ve].trim().to_string()
            } else {
                String::new()
            }
        };
        cells.push(cell_text);
        rest = &inner[close + "</c>".len()..];
    }
    cells.join("\t")
}

/// 取首个 <t>…</t> 内文（xlsx 内联字符串）
fn extract_first_t(s: &str) -> String {
    if let Some(ts) = find_tag(s, "<t") {
        let ta = &s[ts..];
        if let Some(gt) = ta.find('>') {
            let inner = gt + 1;
            if let Some(tc) = ta[inner..].find("</t>") {
                return xml_unescape(&ta[inner..inner + tc]);
            }
        }
    }
    String::new()
}

// ---------- PDF ----------

/// PDF 文本提取：pdf-extract（纯 Rust）内存提取。
/// 扫描件/加密 PDF 无文字层时提取失败，提示转图片后 view_image 查看。
fn extract_pdf(p: &Path) -> Result<String, String> {
    let bytes = std::fs::read(p).map_err(|e| format!("读取 PDF 失败: {e}"))?;
    pdf_extract::extract_text_from_mem(&bytes).map_err(|e| {
        format!("PDF 文本提取失败: {e}（加密 PDF 或扫描件无文字层时无法提取，可转图片后 view_image 查看）")
    })
}

// ---------- 公共 ----------

/// XML 实体反转义（&amp; &lt; &gt; &quot; &apos;）
fn xml_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_tag_skips_prefix_similar_tags() {
        // <w:t 不应匹配 <w:tc>/<w:tbl>/<w:tr>，但应匹配 <w:t> 与 <w:t xml:space="preserve">
        let s = r#"<w:tc><w:p><w:r><w:t>abc</w:t></w:r></w:p></w:tc>"#;
        let pos = find_tag(s, "<w:t").unwrap();
        assert_eq!(&s[pos..pos + 5], "<w:t>");
        let s2 = r#"<w:tbl><w:tr><w:tc><w:t xml:space="preserve">xy</w:t></w:tc></w:tr></w:tbl>"#;
        let pos = find_tag(s2, "<w:t").unwrap();
        assert_eq!(&s2[pos..pos + 5], "<w:t ");
    }

    #[test]
    fn para_text_extracts_tab_br_and_entities() {
        let para = r#"<w:pPr><w:spacing w:after="100"/></w:pPr><w:r><w:t>你好</w:t></w:r><w:r><w:tab/></w:r><w:r><w:t>&amp;&lt;x&gt;</w:t></w:r><w:r><w:br/></w:r><w:r><w:t>第二行</w:t></w:r>"#;
        assert_eq!(para_text(para), "你好\t&<x>\n第二行");
    }

    #[test]
    fn pptx_a_t_extracts_with_page_breaks() {
        let xml = r#"<p:sp><p:txBody><a:p><a:r><a:t>标题</a:t></a:r></a:p><a:p><a:r><a:t>正文</a:t></a:r></a:p></p:txBody></p:sp>"#;
        assert_eq!(extract_a_t(xml), "标题\n正文");
    }

    #[test]
    fn xlsx_shared_strings_and_rows() {
        let ss = r#"<sst><si><t>名称</t></si><si><t>数量</t></si></sst>"#;
        assert_eq!(parse_shared_strings(ss), vec!["名称".to_string(), "数量".to_string()]);
        let sheet = r#"<sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row><row r="2"><c r="A2"><v>42</v></c><c r="B2" t="inlineStr"><is><t>值</t></is></c></row></sheetData>"#;
        assert_eq!(parse_sheet(&parse_shared_strings(ss), sheet), "名称\t数量\n42\t值");
    }

    #[test]
    fn xml_unescape_all_entities() {
        assert_eq!(xml_unescape("a&amp;b&lt;c&gt;d&quot;e&apos;f"), "a&b<c>d\"e'f");
    }
}
