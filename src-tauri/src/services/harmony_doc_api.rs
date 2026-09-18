//! 华为开发者文档站正文接口。
//!
//! 背景：原先依赖「任意文档页 URL 追加 `.md` 返回 Markdown 原文」的技巧，
//! 华为已在 2026-09 下线该端点（全站 404）。文档站现由前端 SPA 调用
//! `documentPortal/getDocumentById` 取正文，正文以 HTML 返回。
//!
//! 本模块封装该接口与配套 HTML 处理：
//! - [`fetch_document`]：按 (catalogName, objectId) 取 HTML 正文；
//! - [`extract_anchors`]：提取页面锚点链接（发现 Kit diff 页面用）；
//! - [`html_tables`]：提取表格行列文本（<br> → 换行，其余标签剥离）；
//! - [`html_to_markdown`]：HTML → 近似 Markdown，供仍按 Markdown 解析的
//!   API 参考正文管线复用。

use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

use crate::utils::net::build_client_auto;

/// 版本说明（版本页 / API 变更清单）目录
pub const CATALOG_RELEASES: &str = "harmonyos-releases";
/// API 参考目录
pub const CATALOG_REFERENCES: &str = "harmonyos-references";

const DOC_API: &str = "https://svc-drcn.developer.huawei.com/community/servlet/consumer/cn/documentPortal/getDocumentById";
const DOC_TREE_API: &str = "https://svc-drcn.developer.huawei.com/community/servlet/consumer/cn/documentPortal/getCatalogTree";

/// 文档目录树中的一个文档节点（叶子）
#[derive(Debug, Clone, Serialize)]
pub struct CatalogDoc {
    /// 文档 objectId（`getDocumentById` 直接可用）
    pub object_id: String,
    /// 目录节点标题（参考文档形如 `@ohos.batteryInfo (电量信息)`）
    pub title: String,
    /// 从根到该节点的标题路径，便于区分同名节点
    pub path: Vec<String>,
}

/// 拉取某个目录的完整文档树（文档中心的 `getCatalogTree`）。
///
/// 用于把「模块名 → 文档 objectId」变成精确查表，取代按命名规则猜 slug：
/// 参考文档的 slug 命名不统一（`@ohos.nfc.tag → js-apis-nfctag`、
/// `@hms.security.confidentialSpace → confidentialspace-confidentialspace`），
/// 靠猜永远覆盖不全。
pub async fn fetch_catalog_docs(catalog: &str) -> Result<Vec<CatalogDoc>, String> {
    let body = serde_json::json!({
        "language": "cn",
        "catalogName": catalog,
        "showHide": 0,
    });
    let client = build_client_auto()?;
    let mut last_err = String::new();
    for attempt in 0..3 {
        match client
            .post(DOC_TREE_API)
            .header("Origin", "https://developer.huawei.com")
            .header("Referer", "https://developer.huawei.com/consumer/cn/doc/")
            .timeout(Duration::from_secs(40))
            .json(&body)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                let text = resp.text().await.map_err(|e| format!("读取响应失败: {e}"))?;
                return parse_catalog_tree(&text);
            }
            Ok(resp) => last_err = format!("HTTP {}", resp.status()),
            Err(e) => last_err = e.to_string(),
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
        }
    }
    Err(last_err)
}

/// 解析 `getCatalogTree` 响应，收集全部带 `relateDocument` 的叶子节点。
pub fn parse_catalog_tree(text: &str) -> Result<Vec<CatalogDoc>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("目录树 JSON 解析失败: {e}"))?;
    if let Some(code) = v.get("code").and_then(|c| c.as_i64()) {
        if code != 0 {
            return Err(format!("目录树接口返回 code={code}"));
        }
    }
    let list = v
        .get("value")
        .and_then(|value| value.get("catalogTreeList"))
        .and_then(|list| list.as_array())
        .ok_or_else(|| "目录树响应缺少 catalogTreeList".to_string())?;
    let mut out = Vec::new();
    let mut path = Vec::new();
    for node in list {
        walk_catalog_node(node, &mut path, &mut out);
    }
    Ok(out)
}

fn walk_catalog_node(node: &Value, path: &mut Vec<String>, out: &mut Vec<CatalogDoc>) {
    let title = node
        .get("nodeName")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    // 注意：带 relateDocument 的节点也可能是父节点（其下还有子文档），
    // 因此记录自身后必须继续下钻，不能提前 return。
    if let Some(object_id) = node.get("relateDocument").and_then(|d| d.as_str()) {
        let object_id = object_id.trim();
        if !object_id.is_empty() {
            out.push(CatalogDoc {
                object_id: object_id.to_string(),
                title: title.clone(),
                path: path.clone(),
            });
        }
    }
    let pushed = !title.is_empty();
    if pushed {
        path.push(title);
    }
    if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
        for child in children {
            walk_catalog_node(child, path, out);
        }
    }
    if pushed {
        path.pop();
    }
}

/// 文档目录索引：把「模块名 → objectId」变成查表。
///
/// 参考文档的标题不一定带完整模块名：`@hms.ai.face.faceDetector` 的页面标题是
/// `faceDetector（人脸检测）`，`@hms.security.confidentialSpace` 的标题却是完整的
/// `@hms.security.confidentialSpace (机密空间服务)`。因此索引同时保存
/// 「完整标题键」与「末段键」，解析时优先完整匹配，其次在末段**唯一**命中时才采用
/// （避免 `image`、`common` 这类通用末段误配）。
#[derive(Debug, Default)]
pub struct CatalogIndex {
    full: std::collections::HashMap<String, String>,
    tails: std::collections::HashMap<String, Vec<String>>,
}

impl CatalogIndex {
    /// 解析模块名对应的文档 objectId。
    ///
    /// 顺序：完整模块名 → 末段唯一 → 末段多义时按「父段一致」消歧（平票优先 `js-apis-*`
    /// 的 ArkTS 文档）。多义时若除错误码页外只剩一篇，取那一篇：`errorcode-*` 是独立的
    /// 错误码参考页，不能当作模块正文。
    pub fn resolve(&self, module: &str) -> Option<&str> {
        let key = normalize_module(module);
        if key.is_empty() {
            return None;
        }
        if let Some(hit) = self.full.get(&key) {
            return Some(hit.as_str());
        }
        let tail = key.rsplit('.').next().unwrap_or("");
        if tail.len() < MIN_TAIL_LEN {
            return None;
        }
        let hits: Vec<&String> = self
            .tails
            .get(tail)
            .map(|hits| hits.iter().filter(|id| !id.starts_with("errorcode-")).collect())
            .unwrap_or_default();
        match hits.len() {
            0 => None,
            1 => Some(hits[0].as_str()),
            _ => self.disambiguate(&key, tail, &hits),
        }
    }

    /// 末段撞名时按父段消歧：objectId 里末段前一段必须等于模块的父段
    /// （`@hms.core.map.map` → `map-map` 而不是 `js-apis-bluetooth-map`）。
    fn disambiguate<'a>(&self, key: &str, tail: &str, hits: &[&'a String]) -> Option<&'a str> {
        let segments: Vec<&str> = key.split('.').collect();
        let parent = if segments.len() >= 3 {
            segments[segments.len() - 2]
        } else {
            ""
        };
        let mut candidates: Vec<&String> = hits
            .iter()
            .copied()
            .filter(|id| {
                let parts: Vec<String> = id.to_lowercase().split('-').map(str::to_string).collect();
                parts.iter().enumerate().any(|(i, part)| {
                    part == tail
                        && i > 0
                        && (parent.is_empty()
                            || parts[i - 1] == parent
                            || (i > 1 && parts[i - 1] == "apis" && parts[i - 2] == parent))
                })
            })
            .collect();
        candidates.sort();
        candidates.dedup();
        match candidates.len() {
            0 => None,
            1 => Some(candidates[0].as_str()),
            _ => {
                // 平票：ArkTS 参考文档（js-apis-*）优先于 C API / Kit 落地页
                let mut arkts: Vec<&String> = candidates
                    .iter()
                    .copied()
                    .filter(|id| id.starts_with("js-apis-"))
                    .collect();
                arkts.sort();
                arkts.dedup();
                if arkts.len() == 1 {
                    Some(arkts[0].as_str())
                } else {
                    None
                }
            }
        }
    }
}

/// 末段匹配的最短长度：太短的末段（`tag`、`map`）容易撞名，但 3 字符配合「父段消歧 +
/// 唯一性」仍可安全命中（`@hms.ai.A2A → hmaf-a2a-protocol`、`@hms.core.iap → iap-iap`）
const MIN_TAIL_LEN: usize = 3;

/// 去空白并小写（模块名与标题都走这一套规范化，大小写差异不影响匹配）
fn normalize_module(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

/// 文档标题 → 模块名键：取括号（中英文）之前的部分，去空白并小写。
///
/// `@ohos.batteryInfo (电量信息)` → `@ohos.batteryinfo`
/// `@hms.enterpriseSpaceService.fileTransfer(空间数据传输)` → `@hms.enterprisespaceservice.filetransfer`
pub fn module_key(title: &str) -> String {
    normalize_module(title.split(['(', '（']).next().unwrap_or(title).trim())
}

/// 从文档目录构建索引；标题不以 `@` 开头的目录节点不进索引。
pub fn catalog_index(docs: &[CatalogDoc]) -> CatalogIndex {
    let mut index = CatalogIndex::default();
    for doc in docs {
        let key = module_key(&doc.title);
        if key.is_empty() {
            continue;
        }
        if key.starts_with('@') {
            index.full.entry(key.clone()).or_insert_with(|| doc.object_id.clone());
        }
        // 末段键负责「标题省略模块前缀」的页面（faceDetector（人脸检测））；
        // 只收含 ASCII 字母的末段，纯中文目录节点不参与匹配。
        let tail = key.rsplit('.').next().unwrap_or("").to_string();
        if tail.chars().any(|c| c.is_ascii_alphabetic()) {
            let entry = index.tails.entry(tail).or_default();
            if !entry.contains(&doc.object_id) {
                entry.push(doc.object_id.clone());
            }
        }
    }
    index
}

/// 抓取一篇文档正文（HTML），带 3 次重试。
///
/// 接口返回 `{"code":0,"value":{"content":{"type":"html","content":"<html>…"}}}`；
/// `code != 0` 表示文档不存在（如 slug 已改名）或接口异常，一律返回 Err，由调用方
/// 决定是否继续探测下一个候选 slug。
pub async fn fetch_document(catalog: &str, object_id: &str) -> Result<String, String> {
    let object_id = object_id.trim();
    if object_id.is_empty() {
        return Err("objectId 为空".into());
    }
    let body = serde_json::json!({
        "objectId": object_id,
        "version": "",
        "catalogName": catalog,
        "language": "cn",
    });
    let client = build_client_auto()?;
    let mut last_err = String::new();
    for attempt in 0..3 {
        match client
            .post(DOC_API)
            .header("Origin", "https://developer.huawei.com")
            .header("Referer", "https://developer.huawei.com/consumer/cn/doc/")
            .timeout(Duration::from_secs(30))
            .json(&body)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                let text = resp.text().await.map_err(|e| format!("读取响应失败: {e}"))?;
                return match extract_content(&text) {
                    Some(html) => Ok(html),
                    None => Err(format!("正文接口无内容: {}", preview(&text))),
                };
            }
            Ok(resp) => last_err = format!("HTTP {}", resp.status()),
            Err(e) => last_err = e.to_string(),
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
        }
    }
    Err(last_err)
}

/// 从接口响应 JSON 中取出 HTML 正文。
fn extract_content(text: &str) -> Option<String> {
    let v: Value = serde_json::from_str(text).ok()?;
    if let Some(code) = v.get("code").and_then(|c| c.as_i64()) {
        if code != 0 {
            return None;
        }
    }
    let content = v.get("value")?.get("content")?.get("content")?.as_str()?;
    if content.trim().is_empty() {
        return None;
    }
    Some(content.to_string())
}

fn preview(text: &str) -> String {
    text.chars().take(200).collect()
}

/// 文档 URL → objectId（末段路径，去掉查询串与 `.md` 后缀）。
pub fn object_id_from_url(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path.trim_end_matches('/').rsplit('/').next()?.trim();
    let last = last.strip_suffix(".md").unwrap_or(last).trim();
    if last.is_empty() {
        None
    } else {
        Some(last.to_string())
    }
}

/// 常见 HTML 实体解码（含十进制/十六进制数字实体）。
pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'&' {
            let c = s[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
            continue;
        }
        let rest = &s[i..];
        let Some(semi) = rest.find(';').filter(|p| *p <= 10) else {
            out.push('&');
            i += 1;
            continue;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .and_then(char::from_u32)
                .or_else(|| {
                    entity
                        .strip_prefix('#')
                        .and_then(|dec| dec.parse::<u32>().ok())
                        .and_then(char::from_u32)
                }),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                i += semi + 1;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

/// 去掉标签、`<br>` 转空格、解码实体，得到单行纯文本。
pub fn strip_tags(s: &str) -> String {
    let spaced = s
        .replace("<br/>", " ")
        .replace("<br />", " ")
        .replace("<br>", " ")
        .replace("</p>", " ")
        .replace("</div>", " ")
        .replace("</li>", " ");
    let mut out = String::with_capacity(spaced.len());
    let mut in_tag = false;
    for c in spaced.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    decode_entities(&out).trim().to_string()
}

/// 提取全部 `<a href="…">文本</a>`（文本已去标签、解码实体）。
pub fn extract_anchors(html: &str) -> Vec<(String, String)> {
    let lower = html.to_lowercase();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel) = lower[i..].find("<a ") {
        let start = i + rel;
        let Some(tag_end_rel) = html[start..].find('>') else {
            break;
        };
        let tag_end = start + tag_end_rel;
        let tag = &html[start..tag_end];
        i = tag_end + 1;
        let Some(href) = attr_value(tag, "href") else {
            continue;
        };
        let Some(close_rel) = lower[i..].find("</a>") else {
            break;
        };
        let text = strip_tags(&html[i..i + close_rel]);
        i += close_rel + 4;
        out.push((text, href.trim().to_string()));
    }
    out
}

/// 取标签内某属性值（支持双引号、单引号、无引号）。
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let mut from = 0;
    while let Some(rel) = lower[from..].find(name) {
        let idx = from + rel;
        let before_ok = idx == 0
            || !lower.as_bytes()[idx - 1].is_ascii_alphanumeric()
                && lower.as_bytes()[idx - 1] != b'-';
        let after = &tag[idx + name.len()..];
        let after_trim = after.trim_start();
        if before_ok && after_trim.starts_with('=') {
            let value = after_trim[1..].trim_start();
            let quote = value.chars().next();
            return match quote {
                Some('"') => value[1..].split('"').next().map(str::to_string),
                Some('\'') => value[1..].split('\'').next().map(str::to_string),
                _ => value
                    .split_whitespace()
                    .next()
                    .map(|v| v.trim_end_matches('>').to_string()),
            };
        }
        from = idx + name.len();
    }
    None
}

/// 提取文档中所有 `<table>`：表格 → 行 → 单元格文本。
///
/// 单元格内 `<br>` 转换行、`<p>` 边界转换行，其余标签剥离并解码实体，
/// 便于既有「按行取字段」的解析逻辑（类名：/API声明：）直接复用。
pub fn html_tables(html: &str) -> Vec<Vec<Vec<String>>> {
    let lower = html.to_lowercase();
    let mut tables = Vec::new();
    let mut i = 0;
    while let Some(rel) = lower[i..].find("<table") {
        let start = i + rel;
        let Some(end_rel) = lower[start..].find("</table>") else {
            break;
        };
        let end = start + end_rel;
        tables.push(rows_of_table(&html[start..end]));
        i = end + "</table>".len();
    }
    tables
}

fn rows_of_table(table: &str) -> Vec<Vec<String>> {
    let lower = table.to_lowercase();
    let mut rows = Vec::new();
    let mut i = 0;
    while let Some(rel) = lower[i..].find("<tr") {
        let start = i + rel;
        let Some(end_rel) = lower[start..].find("</tr>") else {
            break;
        };
        let end = start + end_rel;
        rows.push(cells_of_row(&table[start..end]));
        i = end + "</tr>".len();
    }
    rows
}

fn cells_of_row(row: &str) -> Vec<String> {
    let lower = row.to_lowercase();
    let mut cells = Vec::new();
    let mut i = 0;
    while let Some(rel) = lower[i..].find("<t") {
        let start = i + rel;
        let after = lower[start..].chars().take(3).collect::<String>();
        if !after.starts_with("<td") && !after.starts_with("<th") {
            i = start + 2;
            continue;
        }
        let Some(tag_end_rel) = row[start..].find('>') else {
            break;
        };
        let content_start = start + tag_end_rel + 1;
        let closing = if after.starts_with("<td") { "</td>" } else { "</th>" };
        let Some(close_rel) = lower[content_start..].find(closing) else {
            break;
        };
        let content = &row[content_start..content_start + close_rel];
        cells.push(cell_text(content));
        i = content_start + close_rel + closing.len();
    }
    cells
}

/// 单元格文本：`<br>` / `</p>` 转换行，其余标签剥离。
fn cell_text(cell: &str) -> String {
    let normalized = cell
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("<br>", "\n")
        .replace("</p>", "\n")
        .replace("</li>", "\n");
    let mut out = String::with_capacity(normalized.len());
    let mut in_tag = false;
    for c in normalized.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let decoded = decode_entities(&out);
    // 逐行 trim，去掉缩进噪音但保留行结构
    decoded
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// HTML → 近似 Markdown（标题 `#`、代码块 ```、表格 `|`、保留 `<sup>` 上标）。
///
/// 供仍按 Markdown 解析正文的 API 参考管线复用：Huawei 文档正文的层级是
/// `h1`（模块标题）与 `h4`（章节/成员），因此 `h1 → #`、`h4 → ##`，
/// 与既有 `extract_members`（识别 `## `）的约定一致。
pub fn html_to_markdown(html: &str) -> String {
    let lower = html.to_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        if !html[i..].starts_with('<') {
            let c = html[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
            continue;
        }
        // 跳过 script / style / head
        let mut skipped = false;
        for (open, close) in [("<script", "</script>"), ("<style", "</style>"), ("<head", "</head>")] {
            if lower[i..].starts_with(open) {
                match lower[i..].find(close) {
                    Some(rel) => i += rel + close.len(),
                    None => i = html.len(),
                }
                skipped = true;
                break;
            }
        }
        if skipped {
            continue;
        }
        if lower[i..].starts_with("<!--") {
            i += lower[i..].find("-->").map(|r| r + 3).unwrap_or(html.len() - i);
            continue;
        }
        if lower[i..].starts_with("<pre") {
            let Some(tag_end) = html[i..].find('>') else { break };
            let content_start = i + tag_end + 1;
            let Some(close_rel) = lower[content_start..].find("</pre>") else { break };
            let raw = strip_tags_preserving_lines(&html[content_start..content_start + close_rel]);
            out.push_str("\n```\n");
            out.push_str(raw.trim_end());
            out.push_str("\n```\n");
            i = content_start + close_rel + "</pre>".len();
            continue;
        }
        if lower[i..].starts_with("<table") {
            let Some(end_rel) = lower[i..].find("</table>") else { break };
            let table_html = &html[i..i + end_rel];
            let rows: Vec<Vec<String>> = rows_of_table(table_html);
            let mut table_out = String::new();
            for (idx, row) in rows.iter().enumerate() {
                if row.is_empty() {
                    continue;
                }
                let cells: Vec<String> = row
                    .iter()
                    .map(|c| c.replace('\n', " ").trim().to_string())
                    .collect();
                table_out.push_str(&format!("| {} |\n", cells.join(" | ")));
                // 首行后补 Markdown 分隔行，让表格在严格解析器下依然成立
                if idx == 0 {
                    table_out.push_str(&format!("|{}\n", " --- |".repeat(cells.len())));
                }
            }
            if !table_out.is_empty() {
                out.push('\n');
                out.push_str(&table_out);
            }
            i += end_rel + "</table>".len();
            continue;
        }
        let Some(tag_rel) = html[i..].find('>') else { break };
        let tag_end = i + tag_rel;
        let tag = &html[i..tag_end + 1];
        let tag_lower = lower[i..tag_end + 1].to_string();
        match () {
            _ if tag_lower.starts_with("<h1") => out.push_str("\n# "),
            _ if tag_lower.starts_with("<h2") || tag_lower.starts_with("<h4") => out.push_str("\n## "),
            _ if tag_lower.starts_with("<h3") || tag_lower.starts_with("<h5") || tag_lower.starts_with("<h6") => {
                out.push_str("\n### ")
            }
            _ if tag_lower.starts_with("</h1")
                || tag_lower.starts_with("</h2")
                || tag_lower.starts_with("</h3")
                || tag_lower.starts_with("</h4")
                || tag_lower.starts_with("</h5")
                || tag_lower.starts_with("</h6") =>
            {
                out.push('\n')
            }
            _ if tag_lower.starts_with("<strong") || tag_lower.starts_with("<b>") || tag_lower.starts_with("<b ") => {
                out.push_str("**")
            }
            _ if tag_lower.starts_with("</strong") || tag_lower.starts_with("</b>") => out.push_str("**"),
            _ if tag_lower.starts_with("<code") => out.push('`'),
            _ if tag_lower.starts_with("</code") => out.push('`'),
            _ if tag_lower.starts_with("<sup") || tag_lower.starts_with("</sup") => out.push_str(tag),
            _ if tag_lower.starts_with("<br") => out.push('\n'),
            _ if tag_lower.starts_with("<li") => out.push_str("\n- "),
            _ if tag_lower.starts_with("<p")
                || tag_lower.starts_with("</p")
                || tag_lower.starts_with("<div")
                || tag_lower.starts_with("</div")
                || tag_lower.starts_with("<section")
                || tag_lower.starts_with("</section")
                || tag_lower.starts_with("<ul")
                || tag_lower.starts_with("</ul")
                || tag_lower.starts_with("<ol")
                || tag_lower.starts_with("</ol") =>
            {
                out.push('\n')
            }
            _ => {}
        }
        i = tag_end + 1;
    }
    let decoded = decode_entities(&out);
    // 折叠连续空行 + 去掉标题/表格前的多余空行
    let mut lines: Vec<String> = Vec::new();
    for line in decoded.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() && lines.last().map(|l: &String| l.trim().is_empty()).unwrap_or(true) {
            continue;
        }
        lines.push(trimmed.to_string());
    }
    lines.join("\n").trim().to_string()
}

/// 保留行结构的去标签（用于 `<pre>` 代码块：缩进保留，标签剥掉，实体解码）。
fn strip_tags_preserving_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    decode_entities(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_id_from_url_strips_query_and_md() {
        assert_eq!(
            object_id_from_url("https://developer.huawei.com/consumer/cn/doc/harmonyos-releases/js-apidiff-abilitykit-7003")
                .as_deref(),
            Some("js-apidiff-abilitykit-7003")
        );
        assert_eq!(
            object_id_from_url("https://x/y/apidiff-2600.md?ha_source=a#b").as_deref(),
            Some("apidiff-2600")
        );
    }

    #[test]
    fn extract_content_rejects_non_zero_code() {
        assert!(extract_content(r#"{"code":92531031,"message":"not found"}"#).is_none());
        let ok = r#"{"code":0,"value":{"content":{"type":"html","content":"<p>hi</p>"}}}"#;
        assert_eq!(extract_content(ok).as_deref(), Some("<p>hi</p>"));
    }

    #[test]
    fn decode_entities_handles_named_and_numeric() {
        assert_eq!(decode_entities("a&amp;b &lt;c&gt; &#39;d&#39; &#x4E2D;"), "a&b <c> 'd' 中");
    }

    #[test]
    fn extract_anchors_reads_href_and_text() {
        let html = r#"<a href="/consumer/cn/doc/harmonyos-releases/js-apidiff-arkui-7003">ArkUI</a><a href='x'>Y</a>"#;
        let anchors = extract_anchors(html);
        assert_eq!(anchors.len(), 2);
        assert_eq!(anchors[0].0, "ArkUI");
        assert!(anchors[0].1.ends_with("js-apidiff-arkui-7003"));
        assert_eq!(anchors[1].0, "Y");
    }

    #[test]
    fn html_tables_reads_rows_and_cells() {
        let html = r#"<table><thead><tr><th>操作</th><th>旧版本</th><th>新版本</th><th>d.ts文件</th></tr></thead>
            <tbody><tr><td>新增API</td><td>NA</td><td>类名：A；<br>API声明：function f(): void;</td><td>api/@ohos.a.d.ts</td></tr></tbody></table>"#;
        let tables = html_tables(html);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].len(), 2);
        assert_eq!(tables[0][1][0], "新增API");
        assert_eq!(tables[0][1][2], "类名：A；\nAPI声明：function f(): void;");
    }

    #[test]
    fn html_to_markdown_keeps_headings_code_and_tables() {
        let html = r#"<h1>T</h1><div class="section"><h4>导入模块</h4><pre class="ts">import {a} from '@kit.X';</pre>
            <h4>Preferences</h4><p><strong>系统能力：</strong> S.C</p>
            <table><tr><th>名称</th><th>类型</th><th>只读</th><th>说明</th></tr><tr><td>get</td><td>function</td><td>-</td><td>取值<br>补充</td></tr></table>
            <h4>[h2]put<sup>9+</sup></h4><p>put(key: string): void</p></div>"#;
        let md = html_to_markdown(html);
        assert!(md.contains("# T"), "{md}");
        assert!(md.contains("## 导入模块"), "{md}");
        assert!(md.contains("```\nimport {a} from '@kit.X';\n```"), "{md}");
        assert!(md.contains("**系统能力：**"), "{md}");
        assert!(md.contains("| 名称 | 类型 | 只读 | 说明 |"), "{md}");
        assert!(md.contains("| --- | --- | --- | --- |"), "{md}");
        assert!(md.contains("| get | function | - | 取值 补充 |"), "{md}");
        // 上标保留，供 strip_version_sup 提取 since
        assert!(md.contains("## [h2]put<sup>9+</sup>"), "{md}");
    }

    /// 回归：标签分支曾把「相对偏移」当绝对下标用，导致 i 不前进、转换陷入死循环
    /// （构造：多个相邻闭合标签 + 表格 + 上标）。
    #[test]
    fn html_to_markdown_advances_over_every_tag() {
        let html = "<h1>标题</h1><div><h4>章节</h4><p>正文</p></div><table><tr><th>a</th></tr><tr><td>b</td></tr></table><p>尾</p>";
        let md = html_to_markdown(html);
        assert!(md.contains("# 标题"), "{md}");
        assert!(md.contains("## 章节"), "{md}");
        assert!(md.ends_with("尾"), "{md}");
    }

    #[test]
    fn catalog_tree_collects_leaf_object_ids() {
        let json = r#"{"code":0,"value":{"catalogTreeList":[
            {"nodeName":"API参考","isLeaf":false,"children":[
                {"nodeName":"@ohos.batteryInfo (电量信息)","isLeaf":true,"relateDocument":"js-apis-battery-info"},
                {"nodeName":"@hms.security.confidentialSpace (机密空间服务)","isLeaf":true,"relateDocument":"confidentialspace-confidentialspace"},
                {"nodeName":"@arkts.collections (ArkTS容器集)","isLeaf":true,"relateDocument":"js-apis-arkts-collections"},
                {"nodeName":"空节点"}
            ]},
            {"nodeName":"应用框架","relateDocument":"app-framework-overview","isLeaf":false,"children":[
                {"nodeName":"@ohos.app.ability.UIAbility (UIAbility)","isLeaf":true,"relateDocument":"js-apis-app-ability-uiability"}
            ]}
        ]}}"#;
        let docs = parse_catalog_tree(json).expect("解析目录树");
        // 带 relateDocument 的父节点自身要收，其子文档也不能被漏掉（曾经的 return 提前退出 bug）
        assert_eq!(docs.len(), 5);
        assert_eq!(docs[0].object_id, "js-apis-battery-info");
        assert_eq!(docs[0].path, vec!["API参考".to_string()]);
        assert!(
            docs.iter().any(|d| d.object_id == "js-apis-app-ability-uiability"),
            "父节点带 relateDocument 时仍须下钻：{:?}",
            docs.iter().map(|d| d.object_id.as_str()).collect::<Vec<_>>()
        );
        let index = catalog_index(&docs);
        assert_eq!(
            index.resolve("@hms.security.confidentialSpace"),
            Some("confidentialspace-confidentialspace"),
            "标题带完整模块名时走完整匹配"
        );
        assert_eq!(index.resolve("@ohos.batteryInfo"), Some("js-apis-battery-info"));
        assert_eq!(
            index.resolve("@ohos.app.ability.UIAbility"),
            Some("js-apis-app-ability-uiability")
        );
        assert_eq!(index.resolve("空节点"), None, "非 @ 开头节点不进索引");
        assert_eq!(index.resolve("应用框架"), None, "目录节点不进索引");
    }

    #[test]
    fn catalog_index_falls_back_to_unique_tail() {
        // @hms.ai.* 的页面标题不带模块前缀（faceDetector（人脸检测）），只能靠末段匹配；
        // 末段不唯一或过短时宁可不匹配，也不能猜错文档。
        let docs = vec![
            CatalogDoc {
                object_id: "core-vision-face-detector-api".into(),
                title: "faceDetector（人脸检测）".into(),
                path: vec![],
            },
            CatalogDoc {
                object_id: "hms-ai-speechrecognizer".into(),
                title: "speechRecognizer（语音识别）".into(),
                path: vec![],
            },
            CatalogDoc {
                object_id: "js-apis-image".into(),
                title: "@ohos.multimedia.image (图片)".into(),
                path: vec![],
            },
            CatalogDoc {
                object_id: "js-apis-image-other".into(),
                title: "@ohos.other.image (另一处 image)".into(),
                path: vec![],
            },
        ];
        let index = catalog_index(&docs);
        assert_eq!(
            index.resolve("@hms.ai.face.faceDetector"),
            Some("core-vision-face-detector-api")
        );
        assert_eq!(index.resolve("@hms.ai.speechRecognizer"), Some("hms-ai-speechrecognizer"));
        // 末段 `image` 命中两篇 → 不解析，退回猜 slug
        assert_eq!(index.resolve("@hms.ai.vision.image"), None);
        // 末段过短 → 不解析
        assert_eq!(index.resolve("@ohos.foo.tag"), None);
    }

    #[test]
    fn catalog_index_resolves_short_tails_and_disambiguates() {
        // 3 字符末段（唯一）可解；撞名时按父段消歧；错误码页不当作正文；
        // 平票时 ArkTS 文档（js-apis-*）优先。
        let docs = vec![
            CatalogDoc { object_id: "hmaf-a2a-protocol".into(), title: "A2A（智能体通信协议）".into(), path: vec![] },
            CatalogDoc { object_id: "iap-iap".into(), title: "iap（应用内支付）".into(), path: vec![] },
            CatalogDoc { object_id: "map-map".into(), title: "map（地图服务）".into(), path: vec![] },
            CatalogDoc { object_id: "js-apis-bluetooth-map".into(), title: "map（蓝牙MAP）".into(), path: vec![] },
            CatalogDoc { object_id: "js-apis-nearlink-advertising".into(), title: "@ohos.nearlink.advertising (星闪广播)".into(), path: vec![] },
            CatalogDoc { object_id: "nearlink-advertising".into(), title: "advertising（星闪广播 C API）".into(), path: vec![] },
            CatalogDoc { object_id: "devicesecurity-trusted-auth-api".into(), title: "TrustedAuthentication（数字盾服务）".into(), path: vec![] },
            CatalogDoc { object_id: "errorcode-devicesecurity-trusted-auth".into(), title: "TrustedAuthentication （数字盾服务）".into(), path: vec![] },
        ];
        let index = catalog_index(&docs);
        // 3 字符末段唯一 → 命中
        assert_eq!(index.resolve("@hms.ai.A2A"), Some("hmaf-a2a-protocol"));
        assert_eq!(index.resolve("@hms.core.iap"), Some("iap-iap"));
        // 末段 `map` 撞名：父段消歧选中 map-map，而不是蓝牙 MAP
        assert_eq!(index.resolve("@hms.core.map.map"), Some("map-map"));
        // 末段撞名且都能对上父段：ArkTS 文档优先
        assert_eq!(
            index.resolve("@hms.nearlink.advertising"),
            Some("js-apis-nearlink-advertising")
        );
        // 错误码页被排除后只剩正文页 → 命中
        assert_eq!(
            index.resolve("@hms.security.trustedAuthentication"),
            Some("devicesecurity-trusted-auth-api")
        );
    }

    #[test]
    fn catalog_index_refuses_ambiguous_or_short_tails() {
        let docs = vec![
            CatalogDoc { object_id: "a-alpha".into(), title: "alpha（甲）".into(), path: vec![] },
            CatalogDoc { object_id: "b-beta-alpha".into(), title: "beta.alpha（乙）".into(), path: vec![] },
            CatalogDoc { object_id: "errorcode-only".into(), title: "only（错误码）".into(), path: vec![] },
        ];
        let index = catalog_index(&docs);
        // 末段 `alpha` 两篇、父段都对不上唯一 → 不猜
        assert_eq!(index.resolve("@hms.x.alpha"), None);
        // 只剩错误码页 → 不当作正文
        assert_eq!(index.resolve("@hms.x.only"), None);
    }

    #[test]
    fn catalog_tree_rejects_error_response() {
        assert!(parse_catalog_tree(r#"{"code":92511002,"message":"bad"}"#).is_err());
        assert!(parse_catalog_tree("not json").is_err());
    }

    #[test]
    fn module_key_normalizes_titles() {
        assert_eq!(module_key("@ohos.batteryInfo (电量信息)"), "@ohos.batteryinfo");
        assert_eq!(
            module_key("@hms.enterpriseSpaceService.fileTransfer(空间数据传输)"),
            "@hms.enterprisespaceservice.filetransfer"
        );
        assert_eq!(module_key("@ohos.nfc.tag（标准NFC-Tag）"), "@ohos.nfc.tag");
        assert_eq!(module_key("导入模块"), "导入模块");
    }
}

