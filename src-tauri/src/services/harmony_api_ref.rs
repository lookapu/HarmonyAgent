//! 鸿蒙官方 API 参考正文抓取与解析。
//!
//! 与 harmony_api_diff.rs（版本 diff）互补：
//! - diff 数据回答“哪个版本新增/删除/废弃了哪个 API”
//! - ref 数据回答“这个 API 怎么用、参数/返回值/权限/示例是什么”
//!
//! 数据来源：华为开发者文档站
//!   基础 URL：https://developer.huawei.com/consumer/cn/doc/harmonyos-references/<slug>
//!   例如 js-apis-battery-info 对应 @ohos.batteryInfo。
//!
//! 设计：
//! - 页面 HTML 经简单清洗后得到纯文本正文（去脚本/样式/导航），
//! - 抽取 H1 标题、导入模块、系统能力、权限、首批 API version、设备类型、示例代码，
//! - 从正文按二级标题（## Name）切出子项（class/interface/enum/method），
//! - 写入 api_details + api_members（migration 029）。
//! - refresh 时按 api_docs 里出现过的 module/dts_file 生成候选 slug 列表，
//!   再补充一批常用模块，避免无边界爬取。

use rusqlite::{params, Connection};
use serde::Serialize;
use std::time::Duration;

use crate::services::harmony_api_diff::split_md_row;
use crate::utils::net::build_client_auto;

const REF_BASE: &str = "https://developer.huawei.com/consumer/cn/doc/harmonyos-references";

/// 一个 API 参考页面解析结果
#[derive(Debug, Clone, Serialize)]
pub struct ApiDetail {
    pub module: String,
    pub slug: String,
    pub title: Option<String>,
    pub kit: Option<String>,
    pub since_api_level: Option<u32>,
    pub deprecated: bool,
    pub import_snippet: Option<String>,
    pub syscap: Option<String>,
    pub permissions: Option<String>,
    pub device_types: Option<String>,
    pub body: String,
    pub examples: Option<String>,
    pub members: Vec<ApiMember>,
    pub source_url: String,
}

/// 子项（方法/属性/枚举值等）
#[derive(Debug, Clone, Serialize)]
pub struct ApiMember {
    pub parent_name: Option<String>,
    pub member_name: String,
    pub kind: String,
    pub declaration: Option<String>,
    pub description: Option<String>,
    pub since_api_level: Option<u32>,
    pub deprecated: bool,
    pub syscap: Option<String>,
    pub permission: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefProgress {
    pub phase: String,
    pub current: usize,
    pub total: usize,
    pub message: String,
}

pub type ProgressCb = Box<dyn Fn(&RefProgress) + Send + Sync>;

#[derive(Debug, Clone, Default, Serialize)]
pub struct RefReport {
    pub pages_fetched: usize,
    pub pages_stored: usize,
    pub members_stored: usize,
    pub errors: Vec<String>,
}

/// 常用模块 → slug 兜底（当 api_docs 无数据时，仍可抓取这些核心页面）
const FALLBACK_SLUGS: &[(&str, &str)] = &[
    ("@ohos.batteryInfo", "js-apis-battery-info"),
    ("@ohos.deviceInfo", "js-apis-device-info"),
    ("@ohos.power", "js-apis-power"),
    ("@ohos.file.fs", "js-apis-file-fs"),
    ("@ohos.fileio", "js-apis-fileio"),
    ("@ohos.router", "js-apis-router"),
    ("@ohos.app.ability.UIAbility", "js-apis-app-ability-uiAbility"),
    ("@ohos.app.ability.common", "js-apis-app-ability-common"),
    ("@ohos.bundle.bundleManager", "js-apis-bundleManager"),
    ("@ohos.abilityAccessCtrl", "js-apis-abilityAccessCtrl"),
    ("@ohos.ability.particleAbility", "js-apis-ability-particleAbility"),
    ("@ohos.window", "js-apis-window"),
    ("@ohos.display", "js-apis-display"),
    ("@ohos.measure", "js-apis-measure"),
    ("@ohos.promptAction", "js-apis-promptAction"),
    ("@ohos.prompt", "js-apis-prompt"),
    ("@ohos.notificationManager", "js-apis-notificationManager"),
    ("@ohos.reminderAgentManager", "js-apis-reminderAgentManager"),
    ("@ohos.preferences", "js-apis-data-preferences"),
    ("@ohos.relationalStore", "js-apis-data-relationalStore"),
    ("@ohos.data.distributedKVStore", "js-apis-distributedKVStore"),
    ("@ohos.net.http", "js-apis-http"),
    ("@ohos.net.connection", "js-apis-net-connection"),
    ("@ohos.net.socket", "js-apis-socket"),
    ("@ohos.request", "js-apis-request"),
    ("@ohos.rpc", "js-apis-rpc"),
    ("@ohos.worker", "js-apis-worker"),
    ("@ohos.taskpool", "js-apis-taskpool"),
    ("@ohos.util", "js-apis-util"),
    ("@ohos.stationary", "js-apis-stationary"),
    ("@ohos.sensor", "js-apis-sensor"),
    ("@ohos.vibrator", "js-apis-vibrator"),
    ("@ohos.geolocation", "js-apis-geolocation"),
    ("@ohos.multimedia.camera", "js-apis-camera"),
    ("@ohos.multimedia.media", "js-apis-media"),
    ("@ohos.multimedia.audio", "js-apis-audio"),
    ("@ohos.multimedia.image", "js-apis-image"),
    ("@ohos.bluetooth", "js-apis-bluetooth"),
    ("@ohos.bluetooth.ble", "js-apis-bluetooth-ble"),
    ("@ohos.wifiManager", "js-apis-wifiManager"),
    ("@ohos.telephony.sms", "js-apis-sms"),
    ("@ohos.telephony.call", "js-apis-call"),
    ("@ohos.telephony.radio", "js-apis-radio"),
    ("@ohos.connectedTag", "js-apis-connectedTag"),
    ("@ohos.nfc.tag", "js-apis-nfc-tag"),
    ("@ohos.nfc.cardEmulation", "js-apis-nfc-cardEmulation"),
];

async fn fetch_html(url: &str) -> Result<String, String> {
    let client = build_client_auto()?;
    let mut last_err = String::new();
    for attempt in 0..3 {
        match client
            .get(url)
            .header("Accept", "text/markdown,text/plain,*/*")
            .timeout(Duration::from_secs(30))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                return resp.text().await.map_err(|e| format!("读取响应失败: {e}"));
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

/// 华为文档站对任意页面 URL 追加 `.md` 即返回 Markdown 原文，避免 SPA 空壳 HTML。
async fn fetch_markdown(url: &str) -> Result<String, String> {
    let md_url = if url.ends_with(".md") {
        url.to_string()
    } else {
        format!("{url}.md")
    };
    fetch_html(&md_url).await
}

/// 把 @ohos.xxx 转成华为文档常用的 slug（主候选：全小写连写）。
///
/// 注意：华为文档对驼峰的拆分规则不统一（batteryInfo→battery-info，但
/// notificationManager→notificationmanager），因此实际抓取时应使用
/// [`candidate_slugs`] 生成多个候选并并发探测。本函数仅返回最常见的全连写形式，
/// 供外部（如工具层）需要一个确定 slug 时使用。
pub fn module_to_slug(module: &str) -> String {
    let m = module.trim().trim_start_matches('@');
    let parts: Vec<&str> = m.split('.').collect();
    if parts.len() <= 1 {
        return m.to_lowercase();
    }
    let tail: Vec<String> = parts[1..].iter().map(|s| s.to_lowercase()).collect();
    format!("js-apis-{}", tail.join("-"))
}

/// 小写→大写边界插 `-`（batteryInfo → battery-info），用于生成候选 slug。
fn kebab_case(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(chars.len() + 4);
    for i in 0..chars.len() {
        let c = chars[i];
        if c.is_ascii_uppercase() && i > 0 && chars[i - 1].is_ascii_lowercase() {
            out.push('-');
        }
        out.push(c.to_ascii_lowercase());
    }
    out.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

/// 从正文清洗 HTML：去脚本/样式/标签，<br> 转换行，解码实体。
pub fn html_to_text(html: &str) -> String {
    let lower = html.to_lowercase();
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < html.len() {
        // 跳过 <script>...</script> 和 <style>...</style>
        if lower[i..].starts_with("<script") {
            if let Some(end) = lower[i..].find("</script>") {
                i += end + "</script>".len();
                continue;
            }
            break;
        }
        if lower[i..].starts_with("<style") {
            if let Some(end) = lower[i..].find("</style>") {
                i += end + "</style>".len();
                continue;
            }
            break;
        }
        // <br> → 换行
        if lower[i..].starts_with("<br") {
            out.push('\n');
            // 找到 >
            if let Some(rel) = bytes[i..].iter().position(|b| *b == b'>') {
                i += rel + 1;
                continue;
            }
        }
        if lower[i..].starts_with("</p>")
            || lower[i..].starts_with("</div>")
            || lower[i..].starts_with("</li>")
            || lower[i..].starts_with("</h1>")
            || lower[i..].starts_with("</h2>")
            || lower[i..].starts_with("</h3>")
            || lower[i..].starts_with("</tr>")
        {
            out.push('\n');
            i += lower[i..]
                .find('>')
                .map(|r| r + 1)
                .unwrap_or(lower[i..].len());
            continue;
        }
        let c = html[i..].chars().next().unwrap();
        if c == '<' {
            // 跳过整个标签
            if let Some(rel) = bytes[i..].iter().position(|b| *b == b'>') {
                i += rel + 1;
                continue;
            }
            break;
        }
        out.push(c);
        i += c.len_utf8();
    }
    decode_entities(&out)
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn extract_title(body: &str) -> Option<String> {
    for line in body.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("# ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn extract_kit(body: &str) -> Option<String> {
    // markdown 里面包屑形如：[Basic Services Kit（基础服务）](...)
    // 已由 strip_md_links 去链接，保留 "Basic Services Kit（基础服务）"
    for line in body.lines() {
        let t = line.trim();
        if t.contains("Kit（") && t.contains("）") && t.len() < 120 {
            return Some(t.to_string());
        }
        if t.contains("Kit(") && t.contains(")") && t.len() < 120 && t.ends_with("Kit") {
            return Some(t.to_string());
        }
    }
    None
}

fn extract_since(body: &str) -> Option<u32> {
    for line in body.lines() {
        let t = line.trim();
        if t.contains("首批接口从API version") {
            let mut num = String::new();
            for c in t.chars() {
                if c.is_ascii_digit() {
                    num.push(c);
                } else if !num.is_empty() {
                    break;
                }
            }
            if let Ok(v) = num.parse::<u32>() {
                return Some(v);
            }
        }
    }
    None
}

fn extract_import(body: &str) -> Option<String> {
    // 找"导入模块"标题，取它后面第一个 fenced code block
    let idx = body.find("导入模块")?;
    let after = &body[idx..];
    let mut in_code = false;
    let mut snippet = String::new();
    for line in after.lines().skip(1) {
        let t = line.trim_start();
        if t.starts_with("```") {
            if in_code {
                break;
            }
            in_code = true;
            continue;
        }
        if in_code {
            if t.is_empty() && !snippet.is_empty() {
                // 代码块内部空行也允许，但简单处理：到空行结束
                break;
            }
            snippet.push_str(line.trim());
            snippet.push('\n');
        }
    }
    let s = snippet.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn extract_syscap(body: &str) -> Option<String> {
    for line in body.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("系统能力：") {
            return Some(rest.trim().to_string());
        }
        if let Some(rest) = t.strip_prefix("**系统能力**：") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn extract_permissions(body: &str) -> Option<String> {
    let mut buf = String::new();
    let mut capturing = false;
    for line in body.lines() {
        let t = line.trim();
        if t.contains("所需权限") || t.contains("权限：") || t.starts_with("**权限**") {
            capturing = true;
            buf.push_str(t);
            buf.push('\n');
            continue;
        }
        if capturing {
            if t.starts_with("## ")
                || t.starts_with('#')
                || t.contains("系统能力")
                || t.starts_with("错误码")
            {
                break;
            }
            if !t.is_empty() {
                buf.push_str(t);
                buf.push('\n');
            }
        }
    }
    let s = buf.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn extract_device_types(body: &str) -> Option<String> {
    // markdown 里设备类型通常紧跟标题：一行包含多个设备名，例如
    // "Phone， PC/2in1， Tablet， Wearable" 或 "PhonePC/2in1TabletWearable"
    const DEVICES: &[&str] = &[
        "Phone",
        "PC/2in1",
        "Tablet",
        "Wearable",
        "Car",
        "TV",
        "2in1",
        "Router",
    ];
    let mut found = Vec::new();
    for line in body.lines().take(60) {
        let t = line.trim();
        if t.len() > 200 {
            continue;
        }
        for d in DEVICES {
            if t.contains(d) && !found.contains(&d.to_string()) {
                found.push(d.to_string());
            }
        }
        if found.len() >= 2 {
            return Some(found.join(","));
        }
        found.clear();
    }
    None
}

fn extract_examples(body: &str) -> Option<String> {
    let mut out = String::new();
    let mut in_code = false;
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.push_str(line);
            out.push('\n');
        }
    }
    let s = out.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// 从正文里按二级标题切分子项。
fn extract_members(body: &str, module: &str) -> Vec<ApiMember> {
    let mut members = Vec::new();
    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0;
    let mut current_parent: Option<String> = None;
    while i < lines.len() {
        let t = lines[i].trim();
        if let Some(rest) = t.strip_prefix("## ") {
            let heading = rest.trim();
            // 跳过纯导航/固定章节
            if is_constant_heading(heading) {
                current_parent = None;
                i += 1;
                continue;
            }
            let (name, since) = strip_version_sup(heading);
            let kind = classify_member(heading, body, i);
            // 收集接下来的描述，直到下一个 ##
            let mut desc = String::new();
            let mut decl = String::new();
            let mut j = i + 1;
            let mut in_code = false;
            while j < lines.len() {
                let lt = lines[j].trim();
                if lt.starts_with("## ") && !in_code {
                    break;
                }
                if lt.starts_with("```") {
                    in_code = !in_code;
                    j += 1;
                    continue;
                }
                if in_code {
                    decl.push_str(lt);
                    decl.push('\n');
                } else if !lt.starts_with('#') {
                    if desc.len() < 2000 {
                        desc.push_str(lt);
                        desc.push('\n');
                    }
                }
                j += 1;
            }
            let deprecated = desc.contains("废弃") || desc.contains("deprecated");
            let syscap = desc
                .lines()
                .find_map(|l| {
                    let l = l.trim();
                    l.strip_prefix("系统能力：")
                        .or_else(|| l.strip_prefix("**系统能力**："))
                        .map(|s| s.trim().to_string())
                });
            let permission = desc
                .lines()
                .find(|l| l.contains("所需权限") || l.contains("权限："))
                .map(|s| s.trim().to_string());

            // 如果是 class/interface/enum，把它记为后续成员的 parent
            if kind == "class" || kind == "interface" || kind == "enum" {
                current_parent = Some(name.clone());
            }

            members.push(ApiMember {
                parent_name: current_parent.clone(),
                member_name: name,
                kind: kind.to_string(),
                declaration: if decl.trim().is_empty() {
                    None
                } else {
                    Some(decl.trim().to_string())
                },
                description: if desc.trim().is_empty() {
                    None
                } else {
                    Some(desc.trim().to_string())
                },
                since_api_level: since,
                deprecated,
                syscap,
                permission,
            });
            i = j;
            continue;
        }
        // 表格行：| 名称 | 类型 | 只读 | 说明 |
        if t.starts_with('|') {
            if let Some(m) = parse_table_member(t, current_parent.as_deref()) {
                members.push(m);
            }
        }
        i += 1;
    }
    // 若完全没抽到，但有模块名，至少给一个 module 占位
    if members.is_empty() {
        members.push(ApiMember {
            parent_name: None,
            member_name: module.to_string(),
            kind: "module".to_string(),
            declaration: None,
            description: None,
            since_api_level: None,
            deprecated: false,
            syscap: None,
            permission: None,
        });
    }
    members
}

fn is_constant_heading(h: &str) -> bool {
    matches!(
        h,
        "导入模块"
            | "常量"
            | "枚举"
            | "示例"
            | "说明"
            | "本文导读"
            | "相关推荐"
            | "权限"
            | "错误码"
            | "属性"
            | "方法"
            | "回调"
            | "变量"
            | "类型"
            | "别名"
    )
}

fn strip_version_sup(h: &str) -> (String, Option<u32>) {
    // 支持三种版本上标写法：
    //   BatteryCapacityLevel<sup>9+</sup>
    //   BatteryCapacityLevel (9+)
    //   BatteryCapacityLevel^9+^        （华为 Markdown 原生上标语法）
    let mut name = h.to_string();
    let mut since = None;
    if let Some(start) = name.find("<sup>") {
        if let Some(end) = name.find("</sup>") {
            let raw = &name[start + 5..end];
            let digits: String = raw.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(v) = digits.parse::<u32>() {
                since = Some(v);
            }
            name.replace_range(start..end + 6, "");
        }
    }
    // ^...^ 上标（Markdown 原生语法）：支持 ^7+^、^12^、^(deprecated21)^ 等。
    // 能提取版本号就填 since，其余一律清理掉，避免污染成员名。
    {
        let bytes = name.as_bytes();
        let mut i = 0usize;
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        while i < bytes.len() {
            if bytes[i] == b'^' {
                if let Some(rel) = name[i + 1..].find('^') {
                    let inner_start = i + 1;
                    let inner_end = i + 1 + rel;
                    let inner = &name[inner_start..inner_end];
                    // 尝试从 inner 里提取第一个数字（7+ / 12 / deprecated21）
                    let digits: String = inner
                        .chars()
                        .skip_while(|c| !c.is_ascii_digit())
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    if since.is_none() {
                        if let Ok(v) = digits.parse::<u32>() {
                            since = Some(v);
                        }
                    }
                    ranges.push((i, inner_end + 1));
                    i = inner_end + 1;
                    continue;
                }
            }
            i += 1;
        }
        // 从后往前删除，避免索引错位
        for (s, e) in ranges.into_iter().rev() {
            name.replace_range(s..e, "");
        }
    }
    // 匹配 (9+) 后缀
    if since.is_none() {
        if let Some(open) = name.rfind('(') {
            if let Some(close) = name.find(')').map(|c| c) {
                if close > open {
                    let inner = &name[open + 1..close];
                    let digits: String =
                        inner.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(v) = digits.parse::<u32>() {
                        since = Some(v);
                        name.replace_range(open..=close, "");
                    }
                }
            }
        }
    }
    (name.trim().to_string(), since)
}

fn classify_member(heading: &str, _body: &str, _idx: usize) -> &'static str {
    let lower = heading.to_lowercase();
    if lower.contains("interface") {
        "interface"
    } else if lower.contains("enum") {
        "enum"
    } else if lower.starts_with("class")
        || lower.ends_with("类")
        || lower.contains(" class")
    {
        "class"
    } else if lower.contains('(') || lower.contains("function") {
        "method"
    } else if lower.contains("常量") {
        "const"
    } else {
        "type"
    }
}

fn parse_table_member(line: &str, parent: Option<&str>) -> Option<ApiMember> {
    // | name | type | readonly | desc |
    let cols = split_md_row(line);
    if cols.len() < 4 {
        return None;
    }
    let name = cols[0].trim().to_string();
    // 去掉成员名里的版本上标：isBatteryPresent^7+^ / XXX^(deprecated21)^
    let (name, since) = strip_version_sup(&name);
    // 过滤表头和 Markdown 表格分隔行：
    //   | 名称 | 类型 | ... |
    //   |:---|:---|...|
    //   | --- | --- | ... |
    let normalized = name.trim().trim_start_matches(':').trim_end_matches(':');
    if name.is_empty()
        || name == "名称"
        || name == "Name"
        || (!normalized.is_empty() && normalized.chars().all(|c| c == '-'))
    {
        return None;
    }
    // 方法名形如 getName()，去掉括号再判断空白
    let bare = name.trim_end_matches("()").trim();
    if bare.contains(' ') {
        return None;
    }
    let type_cell = cols[1].trim();
    let kind = if type_cell.contains("()") || bare.ends_with("()") {
        "method"
    } else {
        "property"
    };
    let desc = if cols.len() >= 4 && !cols[3].trim().is_empty() {
        Some(cols[3].trim().to_string())
    } else {
        None
    };
    Some(ApiMember {
        parent_name: parent.map(|s| s.to_string()),
        member_name: name,
        kind: kind.to_string(),
        declaration: None,
        description: desc,
        since_api_level: since,
        deprecated: false,
        syscap: None,
        permission: None,
    })
}

fn strip_html_tags(s: &str) -> String {
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
    out
}

/// 解析华为文档站返回的 Markdown 正文。
///
/// 页面结构（以 `js-apis-battery-info.md` 为例）：
///   # @ohos.batteryInfo (电量信息)
///   ...
///   本模块首批接口从API version 6开始支持。
///   #### 导入模块
///   ```arkts
///   import {batteryInfo} from '@kit.BasicServicesKit';
///   ```
///   系统能力：SystemCapability.PowerManager.BatteryManager.Core
///   |名称|类型|只读|说明|
///   |:---|:---|:---|:---|
///   |batterySOC|number|是|表示当前设备...|
///   ## BatteryPluggedType
///   ...
pub fn parse_reference(md: &str, module: &str, slug: &str, source_url: &str) -> ApiDetail {
    // 1) 去掉 Markdown 链接，只留文字（保留 (url) 里的 url 作为来源信息可选）
    let text = strip_md_links(md);
    // 2) 去 HTML 标签（markdown 中残留的 <br>、<sup> 等）
    let text = strip_html_tags(&text);
    let text = decode_entities(&text);
    // 3) 反斜杠转义：\< → <、\> → >、\( → ( 等
    let text = unescape_md(&text);

    let title = extract_title(&text);
    let kit = extract_kit(&text);
    let since = extract_since(&text);
    let import = extract_import(md);
    let syscap = extract_syscap(&text);
    let perms = extract_permissions(&text);
    let devices = extract_device_types(&text);
    let examples = extract_examples(md);
    // 5) 从正文中切出成员
    let members = extract_members(&text, module);
    let deprecated = text.contains("废弃") || text.to_lowercase().contains("deprecated");

    let mut body = text.clone();
    if body.len() > 200 * 1024 {
        // 按字符边界截断，避免切到 UTF-8 多字节字符中间导致 panic
        let mut trunc = 200 * 1024;
        while trunc > 0 && !body.is_char_boundary(trunc) {
            trunc -= 1;
        }
        body.truncate(trunc);
        body.push_str("\n...(truncated)");
    }

    ApiDetail {
        module: module.to_string(),
        slug: slug.to_string(),
        title,
        kit,
        since_api_level: since,
        deprecated,
        import_snippet: import,
        syscap,
        permissions: perms,
        device_types: devices,
        body,
        examples,
        members,
        source_url: source_url.to_string(),
    }
}

fn unescape_md(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut escape = false;
    for c in s.chars() {
        if escape {
            out.push(c);
            escape = false;
            continue;
        }
        if c == '\\' {
            escape = true;
            continue;
        }
        out.push(c);
    }
    out
}

/// 去掉 Markdown 链接 `[text](url)`，保留 text。
fn strip_md_links(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(close) = s[i + 1..].find(']') {
                let text = &s[i + 1..i + 1 + close];
                let after = i + 1 + close + 1;
                if after < bytes.len() && bytes[after] == b'(' {
                    if let Some(end) = s[after + 1..].find(')') {
                        out.push_str(text);
                        i = after + 1 + end + 1;
                        continue;
                    }
                }
                out.push_str(text);
                i = i + 1 + close + 1;
                continue;
            }
        }
        let c = s[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

fn store_detail(conn: &Connection, d: &ApiDetail) -> Result<usize, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|x| x.as_secs() as i64)
        .unwrap_or(0);
    let members_json = serde_json::to_string(&d.members).unwrap_or_else(|_| "[]".to_string());

    conn.execute(
        "INSERT INTO api_details
            (module, slug, title, kit, since_api_level, deprecated, import_snippet,
             syscap, permissions, device_types, body, examples, members, source_url, fetched_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
         ON CONFLICT(slug) DO UPDATE SET
            module=excluded.module,
            title=excluded.title,
            kit=excluded.kit,
            since_api_level=excluded.since_api_level,
            deprecated=excluded.deprecated,
            import_snippet=excluded.import_snippet,
            syscap=excluded.syscap,
            permissions=excluded.permissions,
            device_types=excluded.device_types,
            body=excluded.body,
            examples=excluded.examples,
            members=excluded.members,
            source_url=excluded.source_url,
            fetched_at=excluded.fetched_at",
        params![
            d.module,
            d.slug,
            d.title,
            d.kit,
            d.since_api_level,
            d.deprecated as i64,
            d.import_snippet,
            d.syscap,
            d.permissions,
            d.device_types,
            d.body,
            d.examples,
            members_json,
            d.source_url,
            now,
        ],
    )
    .map_err(|e| e.to_string())?;

    // 重建 members
    conn.execute("DELETE FROM api_members WHERE detail_slug = ?1", params![d.slug])
        .map_err(|e| e.to_string())?;

    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut n = 0usize;
    {
        let mut stmt = tx
            .prepare(
                "INSERT OR IGNORE INTO api_members
                    (detail_slug, module, parent_name, member_name, kind, declaration,
                     description, since_api_level, deprecated, syscap, permission, source_url)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            )
            .map_err(|e| e.to_string())?;
        for m in &d.members {
            stmt.execute(params![
                d.slug,
                d.module,
                m.parent_name,
                m.member_name,
                m.kind,
                m.declaration,
                m.description,
                m.since_api_level,
                m.deprecated as i64,
                m.syscap,
                m.permission,
                d.source_url,
            ])
            .map_err(|e| e.to_string())?;
            n += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

/// 从 api_docs 聚合已知 module/dts_file，生成候选 (module, Vec<slug>) 列表。
///
/// 每个模块可能对应多个候选 slug（华为文档对驼峰命名的拆分不统一，
/// 例如 batteryInfo→battery-info，但 notificationManager→notificationmanager），
/// 抓取时会并发尝试所有候选，取第一个返回有效 Markdown 的。
fn collect_candidates_from_db(conn: &Connection) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let mut push = |module: String, slug: String| {
        if let Some(entry) = out.iter_mut().find(|(m, _)| m == &module) {
            if !entry.1.contains(&slug) {
                entry.1.push(slug);
            }
        } else {
            out.push((module, vec![slug]));
        }
    };

    // 直接 module 字段
    if let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT module FROM api_docs
         WHERE module IS NOT NULL AND module LIKE '@%'",
    ) {
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .ok();
        if let Some(rows) = rows {
            for r in rows.flatten() {
                for slug in candidate_slugs(&r) {
                    push(r.clone(), slug);
                }
            }
        }
    }

    // d.ts 文件路径：api/@ohos.xxx.d.ts
    if let Ok(mut stmt) =
        conn.prepare("SELECT DISTINCT dts_file FROM api_docs WHERE dts_file LIKE '%@%'")
    {
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .ok();
        if let Some(rows) = rows {
            for r in rows.flatten() {
                let name = r
                    .rsplit('/')
                    .next()
                    .unwrap_or(&r)
                    .trim_end_matches(".d.ts")
                    .trim_end_matches(".ts");
                if name.starts_with('@') {
                    for slug in candidate_slugs(name) {
                        push(name.to_string(), slug);
                    }
                }
            }
        }
    }

    for (m, s) in FALLBACK_SLUGS {
        push((*m).to_string(), (*s).to_string());
    }
    out
}

/// 为一个模块生成候选 slug 列表（不含 `js-apis-` 前缀）。
///
/// 策略：
/// 1. FALLBACK_SLUGS 里已经人工验证过的映射（若有）放最前；
/// 2. 点号路径全小写连写（@ohos.app.ability.UIAbility → app-ability-uiability）；
/// 3. 按 Info/Action/Tag/Store 等常见词做驼峰拆分的变体；
/// 4. 全连写（不拆任何驼峰）作为兜底。
///
/// 所有候选去重后保持顺序返回。调用方并发尝试，取第一个 200 的。
fn candidate_slugs(module: &str) -> Vec<String> {
    let m = module.trim().trim_start_matches('@');
    let parts: Vec<&str> = m.split('.').collect();
    if parts.len() <= 1 {
        // 无点号的模块（极少），直接返回原文小写
        return vec![m.to_lowercase()];
    }
    // 去掉开头的 "ohos"
    let tail_parts: Vec<&str> = if parts[0].eq_ignore_ascii_case("ohos") {
        parts[1..].to_vec()
    } else {
        parts.clone()
    };

    let mut variants: Vec<String> = Vec::new();

    // 变体 1：每个 path segment 全小写连写（UIAbility → uiability）
    let v1 = tail_parts
        .iter()
        .map(|s| s.to_lowercase())
        .collect::<Vec<_>>()
        .join("-");
    variants.push(v1);

    // 变体 2：在小写→大写边界拆（batteryInfo → battery-info）
    let v2 = tail_parts
        .iter()
        .map(|s| kebab_case(s))
        .collect::<Vec<_>>()
        .join("-");
    if !variants.contains(&v2) {
        variants.push(v2);
    }

    // 变体 3：按已知词缀拆分（只对最后一个 segment 加拆词变体）
    // 覆盖华为文档里实际出现的 Info/Tag 等拆分情况。
    const SPLIT_WORDS: &[&str] = &["Info", "Tag"];
    if let Some((last, head)) = tail_parts.split_last() {
        for w in SPLIT_WORDS {
            if let Some(idx) = last.rfind(w) {
                if idx > 0 {
                    let prefix = &last[..idx];
                    let suffix = &last[idx..];
                    let merged = format!(
                        "{}-{}",
                        prefix.to_lowercase(),
                        suffix.to_lowercase()
                    );
                    let v3 = if head.is_empty() {
                        merged
                    } else {
                        format!(
                            "{}-{}",
                            head.iter()
                                .map(|s| s.to_lowercase())
                                .collect::<Vec<_>>()
                                .join("-"),
                            merged
                        )
                    };
                    if !variants.contains(&v3) {
                        variants.push(v3);
                    }
                }
            }
        }
    }

    variants
        .into_iter()
        .map(|v| format!("js-apis-{v}"))
        .collect()
}

pub async fn refresh_all(
    db: &crate::db::DbState,
    on_progress: Option<ProgressCb>,
) -> Result<RefReport, String> {
    let candidates = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        collect_candidates_from_db(&conn)
    };

    let total = candidates.len();
    if let Some(cb) = &on_progress {
        cb(&RefProgress {
            phase: "discover".to_string(),
            current: 0,
            total,
            message: format!("发现 {total} 个候选模块"),
        });
    }

    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    let progress_cb: Option<std::sync::Arc<dyn Fn(&RefProgress) + Send + Sync>> =
        on_progress.map(std::sync::Arc::from);
    let mut report = RefReport::default();
    let mut done: usize = 0;

    // 分块：每 4 个模块一组并发；模块内部多个候选 slug 也并发尝试，
    // 取第一个返回 200 且正文以 `#` 开头（真正的 Markdown 文档）的 slug。
    // 抓完一组后串行落库（rusqlite Connection 不是 Sync）。
    for chunk in candidates.chunks(4) {
        let mut futs = Vec::new();
        for (module, slugs) in chunk {
            let sem = sem.clone();
            let module = module.clone();
            let slugs = slugs.clone();
            futs.push(async move {
                let _permit = sem.acquire().await.ok();
                let mut last_err: Option<String> = None;
                // 并发尝试所有候选 slug，取第一个成功的
                let slug_futs: Vec<_> = slugs
                    .iter()
                    .map(|slug| {
                        let url = format!("{REF_BASE}/{slug}");
                        let slug = slug.clone();
                        async move {
                            match fetch_markdown(&url).await {
                                Ok(html) => {
                                    let t = html.trim_start();
                                    // 华为 404 页返回的是 HTML 壳，以 `<` 开头；
                                    // 真正的文档以 `#` 开头。
                                    if t.starts_with('#') && !t.is_empty() {
                                        Some((slug, url, html))
                                    } else {
                                        None
                                    }
                                }
                                Err(_) => None,
                            }
                        }
                    })
                    .collect();
                let results = futures_util::future::join_all(slug_futs).await;
                for r in results {
                    if let Some(triple) = r {
                        return (module, Some(triple), last_err);
                    }
                }
                last_err = Some(format!("所有候选 slug 均 404: {}", slugs.join(", ")));
                (module, None, last_err)
            });
        }
        let results = futures_util::future::join_all(futs).await;
        for (module, triple, err) in results {
            done += 1;
            if let Some(cb) = &progress_cb {
                cb(&RefProgress {
                    phase: "fetch".to_string(),
                    current: done,
                    total,
                    message: module.clone(),
                });
            }
            match triple {
                Some((slug, url, html)) => {
                    report.pages_fetched += 1;
                    let detail = parse_reference(&html, &module, &slug, &url);
                    let conn = db.0.lock().map_err(|e| e.to_string())?;
                    match store_detail(&conn, &detail) {
                        Ok(n) => {
                            report.pages_stored += 1;
                            report.members_stored += n;
                        }
                        Err(e) => report.errors.push(format!("[{module}] 写入失败: {e}")),
                    }
                }
                None => {
                    if let Some(e) = err {
                        report.errors.push(format!("[{module}] {e}"));
                    }
                }
            }
        }
    }

    Ok(report)
}

/// 按模块/关键字查询详情
#[derive(Debug, Clone, Default)]
pub struct DetailQuery {
    pub module: Option<String>,
    pub keyword: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DetailHit {
    pub module: String,
    pub slug: String,
    pub title: Option<String>,
    pub kit: Option<String>,
    pub since_api_level: Option<u32>,
    pub deprecated: bool,
    pub import_snippet: Option<String>,
    pub syscap: Option<String>,
    pub permissions: Option<String>,
    pub device_types: Option<String>,
    pub examples: Option<String>,
    pub source_url: String,
    /// 命中子项（最多 20 条）
    pub members: Vec<ApiMember>,
    /// body 片段（含关键字前后 400 字符）
    pub snippet: Option<String>,
}

pub fn query_details(conn: &Connection, q: &DetailQuery) -> Result<Vec<DetailHit>, String> {
    let mut sql = String::from(
        "SELECT module, slug, title, kit, since_api_level, deprecated, import_snippet,
                syscap, permissions, device_types, examples, source_url, body
         FROM api_details WHERE 1=1",
    );
    let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(m) = &q.module {
        sql.push_str(&format!(" AND (module LIKE ?{} OR slug LIKE ?{})", args.len() + 1, args.len() + 1));
        args.push(Box::new(format!("%{m}%")));
    }
    if let Some(kw) = &q.keyword {
        if !kw.trim().is_empty() {
            sql.push_str(&format!(
                " AND (body LIKE ?{} OR title LIKE ?{} OR module LIKE ?{})",
                args.len() + 1,
                args.len() + 1,
                args.len() + 1
            ));
            args.push(Box::new(format!("%{kw}%")));
        }
    }
    sql.push_str(" ORDER BY since_api_level DESC LIMIT ?");
    let limit = q.limit.unwrap_or(10) as i64;
    args.push(Box::new(limit));

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let rows = stmt
        .query_map(params_ref.as_slice(), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<u32>>(4)?,
                row.get::<_, bool>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, String>(12)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for r in rows {
        let (module, slug, title, kit, since, deprecated, import_snip, syscap, perms, devs, examples, url, body) =
            r.map_err(|e| e.to_string())?;
        let members = fetch_members(conn, &slug)?;
        let snippet = q.keyword.as_ref().and_then(|kw| build_snippet(&body, kw));
        out.push(DetailHit {
            module,
            slug,
            title,
            kit,
            since_api_level: since,
            deprecated,
            import_snippet: import_snip,
            syscap,
            permissions: perms,
            device_types: devs,
            examples,
            source_url: url,
            members,
            snippet,
        });
    }
    Ok(out)
}

fn fetch_members(conn: &Connection, slug: &str) -> Result<Vec<ApiMember>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT parent_name, member_name, kind, declaration, description,
                    since_api_level, deprecated, syscap, permission
             FROM api_members WHERE detail_slug = ?1
             ORDER BY parent_name, kind, member_name LIMIT 200",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![slug], |row| {
            Ok(ApiMember {
                parent_name: row.get(0)?,
                member_name: row.get(1)?,
                kind: row.get(2)?,
                declaration: row.get(3)?,
                description: row.get(4)?,
                since_api_level: row.get(5)?,
                deprecated: row.get::<_, bool>(6)?,
                syscap: row.get(7)?,
                permission: row.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

fn build_snippet(body: &str, kw: &str) -> Option<String> {
    let lower = body.to_lowercase();
    let k = kw.to_lowercase();
    let pos = lower.find(&k)?;
    let start = pos.saturating_sub(200);
    let end = (pos + kw.len() + 400).min(body.len());
    let snippet = body[start..end].replace(|c: char| c.is_control() && c != '\n', " ");
    Some(snippet)
}

pub fn count_details(conn: &Connection) -> Result<(usize, usize), String> {
    let d: usize = conn
        .query_row("SELECT COUNT(*) FROM api_details", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let m: usize = conn
        .query_row("SELECT COUNT(*) FROM api_members", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    Ok((d, m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_to_slug_basic() {
        assert_eq!(module_to_slug("@ohos.file.fs"), "js-apis-file-fs");
        assert_eq!(
            module_to_slug("@ohos.app.ability.UIAbility"),
            "js-apis-app-ability-uiability"
        );
        assert_eq!(module_to_slug("@ohos.batteryInfo"), "js-apis-batteryinfo");
    }

    #[test]
    fn candidate_slugs_covers_variants() {
        // batteryInfo：全连写 + 小写大写边界拆 + Info 词缀拆，其中 `battery-info` 是华为实际 slug
        let s = candidate_slugs("@ohos.batteryInfo");
        assert!(s.contains(&"js-apis-batteryinfo".to_string()));
        assert!(s.contains(&"js-apis-battery-info".to_string()));

        // UIAbility：不应拆成 ui-ability
        let s = candidate_slugs("@ohos.app.ability.UIAbility");
        assert!(s.contains(&"js-apis-app-ability-uiability".to_string()));

        // notificationManager：全连写必须在候选里（华为实际 slug）
        let s = candidate_slugs("@ohos.notificationManager");
        assert!(s.contains(&"js-apis-notificationmanager".to_string()));

        // deviceInfo：device-info 必须在候选里
        let s = candidate_slugs("@ohos.deviceInfo");
        assert!(s.contains(&"js-apis-device-info".to_string()));
    }

    #[test]
    fn kebab_case_handles_camel_boundaries() {
        assert_eq!(kebab_case("batteryInfo"), "battery-info");
        assert_eq!(kebab_case("deviceInfo"), "device-info");
        // 连续大写不拆（HTMLElement → htmlelement），华为实际命名如此
        assert_eq!(kebab_case("HTMLElement"), "htmlelement");
        assert_eq!(kebab_case("fs"), "fs");
        assert_eq!(kebab_case("already-kebab"), "already-kebab");
    }

    #[test]
    fn html_to_text_strips_scripts() {
        let html = "<p>hello</p><script>alert(1)</script><p>world</p>";
        let txt = html_to_text(html);
        assert!(txt.contains("hello"));
        assert!(txt.contains("world"));
        assert!(!txt.contains("alert"));
    }

    #[test]
    fn strip_version_sup_extracts_level() {
        let (n, lvl) = strip_version_sup("BatteryCapacityLevel<sup>9+</sup>");
        assert_eq!(n, "BatteryCapacityLevel");
        assert_eq!(lvl, Some(9));
    }

    #[test]
    fn parse_reference_extracts_core_fields() {
        let md = r#"# @ohos.batteryInfo (电量信息)

[Basic Services Kit（基础服务）](https://x)

本模块首批接口从API version 6开始支持。

#### 导入模块

```
import { batteryInfo } from '@kit.BasicServicesKit';
```

**系统能力**：SystemCapability.PowerManager.BatteryManager.Core

| 名称 | 类型 | 只读 | 说明 |
|---|---|---|---|
| batterySOC | number | 是 | 表示当前设备剩余电池电量百分比 |

## BatteryPluggedType

表示连接的充电器类型的枚举。

**系统能力**：SystemCapability.PowerManager.BatteryManager.Core

| 名称 | 值 | 说明 |
|---|---|---|
| NONE | 0 | 未获取到 |

## 示例

```
import {batteryInfo} from '@kit.BasicServicesKit';
let batterySOCInfo: number = batteryInfo.batterySOC;
console.info("The batterySOCInfo is: " + batterySOCInfo);
```
"#;
        let d = parse_reference(md, "@ohos.batteryInfo", "js-apis-battery-info", "http://x");
        assert_eq!(d.since_api_level, Some(6));
        assert!(d.syscap.unwrap().contains("PowerManager"));
        assert!(d.members.iter().any(|m| m.member_name == "batterySOC"));
        assert!(d.members.iter().any(|m| m.member_name == "BatteryPluggedType"));
        assert!(d.examples.unwrap().contains("console.info"));
        assert!(d.import_snippet.unwrap().contains("BasicServicesKit"));
    }

    /// 端到端联网测试：抓一个真实 API 参考页 → 解析 → 写入内存库 → 查回详情。
    /// 默认 ignore，本地用 `cargo test --lib -- --ignored` 手动跑。
    #[tokio::test]
    #[ignore = "需要联网，手动执行：cargo test --lib -- --ignored"]
    async fn e2e_fetch_parse_store_query() {
        use rusqlite::Connection;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/029_api_details.sql"))
            .unwrap();

        let url = format!(
            "{}/js-apis-battery-info",
            "https://developer.huawei.com/consumer/cn/doc/harmonyos-references"
        );
        let html = fetch_markdown(&url).await.expect("fetch");
        let detail = parse_reference(
            &html,
            "@ohos.batteryInfo",
            "js-apis-battery-info",
            &url,
        );
        assert!(
            detail.title.as_deref().unwrap_or("").contains("batteryInfo"),
            "标题应包含 batteryInfo，实际：{:?}",
            detail.title
        );
        assert_eq!(detail.since_api_level, Some(6));
        assert!(detail.import_snippet.is_some(), "应抽到 import 语句");
        assert!(
            detail.members.iter().any(|m| m.member_name == "batterySOC"),
            "应抽到 batterySOC 属性"
        );
        assert!(detail.examples.is_some(), "应抽到示例代码");

        let n = store_detail(&conn, &detail).expect("store");
        assert!(n > 0, "members 行数应 > 0");

        let (dc, mc) = count_details(&conn).unwrap();
        assert_eq!(dc, 1);
        assert!(mc >= n);

        let hits = query_details(
            &conn,
            &DetailQuery {
                module: Some("batteryInfo".into()),
                ..Default::default()
            },
        )
        .expect("query");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].members.iter().any(|m| m.member_name == "batterySOC"));
    }
}
