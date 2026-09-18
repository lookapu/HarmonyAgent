//! 鸿蒙官方全量 API 知识库：从各版本 API diff 页面聚合。
//!
//! 数据来源：华为开发者文档站
//! - 版本首页：https://developer.huawei.com/consumer/cn/doc/harmonyos-releases/<version>
//! - 每版本的 API 变更清单页（列出所有 Kit 的 diff 链接）
//! - 每个 Kit 的 diff 页面是一张表格：操作 | 旧版本 | 新版本 | d.ts文件
//!
//! 正文经文档中心的 `documentPortal/getDocumentById` 接口获取（HTML，见
//! [`crate::services::harmony_doc_api`]）；页面 URL 追加 `.md` 的旧端点已下线。
//!
//! 聚合所有版本后，可以回答：
//! - 某个 API 在哪个版本引入、哪个版本废弃/删除
//! - 某两个版本之间有哪些 API 变更
//! - 代码里用到的 API 是否兼容目标版本
//!
//! 设计：
//! - 数据落地到 SQLite（api_docs 表，migration 028），FTS 搜索靠 LIKE 兜底
//! - 抓取走 reqwest + 系统代理，解析 HTML 表格；解析器同时保留 Markdown 表格分支，
//!   以便正文形态在 HTML/Markdown 之间切换时不至失效
//! - 提供 refresh（全量/增量刷新）与 search（按关键字/模块/版本过滤）

use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::Mutex;

use crate::services::harmony_doc_api::{
    extract_anchors, fetch_document, html_tables, object_id_from_url, CATALOG_RELEASES,
};

/// 一条 API 变更记录
#[derive(Debug, Clone, Serialize)]
pub struct ApiEntry {
    #[serde(default)]
    pub id: Option<i64>,
    pub kit: String,
    pub dts_file: Option<String>,
    pub module: Option<String>,
    pub class_name: Option<String>,
    pub declaration: String,
    pub api_name: Option<String>,
    pub change_type: String,
    pub version_label: String,
    pub api_level: Option<u32>,
    pub old_declaration: Option<String>,
    pub source_url: String,
}

/// 刷新进度（前端可订阅）
#[derive(Debug, Clone, Serialize)]
pub struct RefreshProgress {
    pub phase: String,
    pub current: usize,
    pub total: usize,
    pub message: String,
}

/// 已知版本映射：URL slug → (显示标签, API level)
///
/// 覆盖 HarmonyOS NEXT 至今的全部正式版本与 5.0.x/5.1.x 历史版本；
/// 元组: (版本页 slug, 版本标签, API level)。
/// slug 指向版本发布页（含 apidiff 链接或可推导 apidiff 路径）。
/// 5.0.1/5.0.2/5.0.5 的 apidiff 入口命名特殊，在 candidate_apidiff_slugs 中单独处理。
const KNOWN_VERSIONS: &[(&str, &str, u32)] = &[
    ("2600", "26.0.0", 26),
    ("611", "6.1.1(24)", 24),
    ("610", "6.1.0(23)", 23),
    ("602", "6.0.2(22)", 22),
    ("601", "6.0.1(21)", 21),
    ("600", "6.0.0(20)", 20),
    ("511", "5.1.1(19)", 19),
    ("510", "5.1.0(18)", 18),
    ("505", "5.0.5(17)", 17),
    ("504", "5.0.4(16)", 16),
    ("503", "5.0.3(15)", 15),
    ("5-0-2", "5.0.2(14)", 14),
    ("5-0-1", "5.0.1(13)", 13),
    ("5-0-0", "5.0.0(12)", 12),
];

const BASE_URL: &str = "https://developer.huawei.com/consumer/cn/doc/harmonyos-releases";

/// 抓取一篇版本说明正文（HTML）。
async fn fetch_release_doc(object_id: &str) -> Result<String, String> {
    fetch_document(CATALOG_RELEASES, object_id).await
}

/// 从 markdown 中解析 Markdown 链接 `[text](url)`。
fn extract_md_links(md: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = md.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(close) = md[i + 1..].find(']') {
                let text = &md[i + 1..i + 1 + close];
                let after = i + 1 + close + 1;
                if after < bytes.len() && bytes[after] == b'(' {
                    if let Some(end) = md[after + 1..].find(')') {
                        let url = &md[after + 1..after + 1 + end];
                        out.push((text.trim().to_string(), url.trim().to_string()));
                        i = after + 1 + end + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

/// 提取页面链接：优先 HTML 锚点，其次 Markdown 链接（兼容正文形态变化）。
fn extract_links(body: &str) -> Vec<(String, String)> {
    let anchors = extract_anchors(body);
    if anchors.is_empty() {
        extract_md_links(body)
    } else {
        anchors
    }
}

/// 版本 slug → 可能的「API 变更清单」入口 objectId 候选。
///
/// 版本页（如 `2600`）本身不含 apidiff 链接，真正的清单入口命名不统一：
/// `apidiff-2600`（索引页，再分 release/beta 子入口）、`apidiff-611`、
/// `apidiff-from-501-release`、`apidiff-6001`（无 600 索引页时）等，需枚举。
fn starter_slugs(version_slug: &str) -> Vec<String> {
    let digit = version_slug.replace('-', "");
    let mut out = vec![format!("apidiff-{version_slug}")];
    if digit != version_slug {
        out.push(format!("apidiff-{digit}"));
    }
    for c in candidate_apidiff_slugs(version_slug) {
        out.push(format!("apidiff-{c}"));
    }
    // 数字子版本直接入口兜底（如 6.0.0 只有 apidiff-6001/6002/6003/6004，无 apidiff-600）
    if digit.chars().all(|c| c.is_ascii_digit()) {
        for n in 1..=6 {
            out.push(format!("apidiff-{digit}{n}"));
        }
    }
    out
}

/// 从版本页出发发现该版本全部 Kit diff 页面。
///
/// 层级不固定：`apidiff-2600` 只列 release/beta 子入口（apidiff-7001/7002/7003），
/// 子入口页才列到每个 Kit 的 diff 链接；`apidiff-611` 同理要经 apidiff-6111/6112。
/// 因此按「页内含 Kit diff 链接即收、只含 apidiff 链接则继续下钻」做 BFS，最多 3 层。
async fn discover_kit_pages(
    version_slug: &str,
    level: u32,
) -> Result<Vec<(String, String, Option<u32>)>, String> {
    let mut queue: Vec<String> = starter_slugs(version_slug);
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<(String, String, Option<u32>)> = Vec::new();
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));

    for _depth in 0..3 {
        let batch: Vec<String> = std::mem::take(&mut queue);
        if batch.is_empty() {
            break;
        }
        let mut handles = Vec::new();
        for object_id in batch {
            if object_id.trim().is_empty() || !visited.insert(object_id.clone()) {
                continue;
            }
            let sem = sem.clone();
            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.ok();
                let body = fetch_release_doc(&object_id).await;
                (object_id, body)
            }));
        }
        // 并发结果按提交顺序回收，保持候选枚举的确定性
        let mut pages: Vec<(String, String)> = Vec::new();
        for h in handles {
            if let Ok((object_id, Ok(body))) = h.await {
                pages.push((object_id, body));
            }
        }
        for (_object_id, body) in pages {
            let links = extract_links(&body);
            let mut has_kit_diff = false;
            for (text, href) in &links {
                if is_kit_diff_url(href) {
                    let url = normalize_url(href);
                    if !out.iter().any(|(_, u, _)| u == &url) {
                        out.push((text.clone(), url, Some(level)));
                    }
                    has_kit_diff = true;
                }
            }
            if has_kit_diff {
                continue;
            }
            for (_text, href) in links {
                if !href.contains("apidiff") && !href.contains("apis-diff") {
                    continue;
                }
                if let Some(id) = object_id_from_url(&href) {
                    if !visited.contains(&id) {
                        queue.push(id);
                    }
                }
            }
        }
    }

    if out.is_empty() {
        return Err(format!(
            "未发现 {version_slug} 的 Kit diff 页面（API 变更清单索引页缺失或页面结构变化）"
        ));
    }
    Ok(out)
}

/// 给定版本页 slug，生成可能的 apidiff「直接入口」slug 候选（不含索引页）。
/// 华为不同版本命名不一致：release/beta1/beta2/beta3 后缀、from-{digit} 前缀等都有，
/// 需要枚举多种并全部收集（一个版本可能同时存在多个入口，如 5.0.1 有
/// apidiff-from-501-release 与 apidiff-from-501-beta3）。
fn candidate_apidiff_slugs(version_slug: &str) -> Vec<String> {
    let mut out = Vec::new();
    let digit = version_slug.replace('-', "");

    // from-{digit}-{stage}：apidiff-from-501-release / apidiff-from-501-beta3
    for stage in ["release", "beta1", "beta2", "beta3"] {
        out.push(format!("from-{digit}-{stage}"));
    }
    // {digit}-{stage}：apidiff-502-beta1 / apidiff-505-beta1
    for stage in ["release", "beta1", "beta2", "beta3"] {
        out.push(format!("{digit}-{stage}"));
    }
    // 5.0.0 首版特殊入口：apidiff-beta1（无 500 前缀）
    if version_slug == "5-0-0" || digit == "500" {
        out.push("beta1".to_string());
    }
    out
}

fn normalize_url(url: &str) -> String {
    if url.starts_with("http") {
        url.to_string()
    } else if url.starts_with('/') {
        format!("https://developer.huawei.com{url}")
    } else {
        format!("{BASE_URL}/{url}")
    }
}

fn is_kit_diff_url(url: &str) -> bool {
    url.contains("js-apidiff")
        || url.contains("c-apidiff")
        || url.contains("c-apis-diff")
        || url.contains("arkui-apidiff")
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

/// HTML 实体解码（常见几个）
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// 从 d.ts 文件路径推导模块名：
///   api/@ohos.app.ability.scriptManager.d.ts → @ohos.app.ability.scriptManager
fn module_from_dts(dts: &str) -> Option<String> {
    let name = dts
        .rsplit('/')
        .next()
        .unwrap_or(dts)
        .rsplit('\\')
        .next()
        .unwrap_or(dts);
    let name = name.trim();
    if name.starts_with('@') {
        // ArkTS 声明文件可能是 *.d.ets（如 @arkts.collections.d.ets），需一并剥离
        Some(
            name.trim_end_matches(".d.ts")
                .trim_end_matches(".d.ets")
                .trim_end_matches(".ts")
                .to_string(),
        )
    } else if name.contains('.') {
        // api/bundleManager/SkillInfo.d.ts 这种没有 @ 前缀，不归入模块
        None
    } else {
        None
    }
}

/// 从声明中提取 API 名称（函数名/属性名/枚举值）
fn extract_api_name(decl: &str, class: &str) -> Option<String> {
    let t = decl.trim();
    // function foo(...) → foo
    if let Some(rest) = t.strip_prefix("function ") {
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        if !name.is_empty() {
            return Some(name);
        }
    }
    // readonly/let/const name: type → name
    for prefix in ["readonly ", "const ", "let ", "var "] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                .collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    // class/interface/enum/namespace Name → Name（先去 export/declare 前缀）
    let mut t2 = t;
    if let Some(rest) = t2.strip_prefix("export ") {
        t2 = rest;
    }
    if let Some(rest) = t2.strip_prefix("declare ") {
        t2 = rest;
    }
    for prefix in [
        "abstract class ",
        "class ",
        "interface ",
        "enum ",
        "namespace ",
        "type ",
    ] {
        if let Some(rest) = t2.strip_prefix(prefix) {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                .collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    // 枚举值 "FOO = 0" 或 "FOO"，且 class 是某 enum → 取第一个 token
    if t2.contains('=') && !t2.contains('(') {
        let name: String = t2
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        if !name.is_empty() {
            return Some(name);
        }
    }
    // 方法名(...)：排除 class_name 本身
    if let Some(paren) = t2.find('(') {
        let candidate = t2[..paren].trim();
        if !candidate.is_empty()
            && candidate != class
            && !candidate.starts_with("export ")
            && !candidate.starts_with("declare ")
        {
            return Some(candidate.to_string());
        }
    }
    None
}

/// 解析一个 Kit diff 页面：优先 HTML 表格，回退 Markdown 表格。
///
/// 正文经 `getDocumentById` 返回 HTML（`<table><tr><td>`），但解析器保留
/// Markdown 分支：正文形态若回退，抓取不至失效。
fn parse_diff_page(
    body: &str,
    kit: &str,
    version_label: &str,
    api_level: Option<u32>,
    source_url: &str,
) -> Vec<ApiEntry> {
    let mut entries = Vec::new();
    for table in html_tables(body) {
        for row in table {
            if let Some(entry) = entry_from_cells(&row, kit, version_label, api_level, source_url) {
                entries.push(entry);
            }
        }
    }
    if entries.is_empty() {
        entries = parse_diff_table(body, kit, version_label, api_level, source_url);
    }
    entries
}

/// 解析一个 Kit diff 页面（Markdown 格式），返回 API 条目列表。
///
/// Markdown 表格形如：
///   |操作|旧版本|新版本|d.ts文件|
///   |---|---|---|---|
///   |新增API|NA|类名：xxx； API声明：function ...; 差异内容：...|api/@ohos.xxx.d.ts|
///
/// 单元格可能包含 `<br>`、多行 HTML，本函数按"管道符在表格行首"切行，
/// 再按未转义的 `|` 分列。
fn parse_diff_table(md: &str, kit: &str, version_label: &str, api_level: Option<u32>, source_url: &str) -> Vec<ApiEntry> {
    let mut entries = Vec::new();
    for raw_line in md.lines() {
        let line = raw_line.trim();
        if !line.starts_with('|') {
            continue;
        }
        // 跳过分隔线/表头
        let stripped = line.trim_start_matches('|').trim_end_matches('|');
        if stripped.chars().all(|c| c == '-' || c == ':' || c.is_whitespace() || c == '|') {
            continue;
        }
        let cells = split_md_row(line);
        if cells.len() < 4 {
            continue;
        }
        let normalized = vec![
            cells[0].trim().to_string(),
            normalize_cell(&cells[1]),
            normalize_cell(&cells[2]),
            cells[3].trim().to_string(),
        ];
        if let Some(entry) = entry_from_cells(&normalized, kit, version_label, api_level, source_url)
        {
            entries.push(entry);
        }
    }
    entries
}

/// 表格行 → API 条目。单元格需已是纯文本（HTML 路径已去标签解码；
/// Markdown 路径先经 [`normalize_cell`]）。
fn entry_from_cells(
    cells: &[String],
    kit: &str,
    version_label: &str,
    api_level: Option<u32>,
    source_url: &str,
) -> Option<ApiEntry> {
    if cells.len() < 4 {
        return None;
    }
    let op_raw = cells[0].trim();
    if op_raw == "操作" || op_raw.contains("操作") && cells[1].trim() == "旧版本" {
        return None;
    }
    let old_raw = cells[1].trim().to_string();
    let new_raw = cells[2].trim().to_string();
    let dts_raw = cells[3].trim();

    let op = op_raw.to_lowercase();
    let change_type = if op.contains("新增api") || op.contains("新增错误码") || op.contains("新增") {
        "added"
    } else if op.contains("删除") {
        "removed"
    } else if op.contains("废弃") {
        "deprecated"
    } else if op.contains("变更") || op.contains("修改") {
        "modified"
    } else if op.contains("kit") {
        "new_kit"
    } else {
        return None;
    };

    let dts_file = if dts_raw.is_empty() {
        None
    } else {
        Some(dts_raw.replace("api\\", "api/").to_string())
    };
    let module = dts_file.as_deref().and_then(module_from_dts);

    let primary = if new_raw.trim() != "NA" && !new_raw.trim().is_empty() {
        new_raw.clone()
    } else {
        old_raw.clone()
    };

    let class_name = extract_field_value(&primary, "类名");
    let declaration = extract_field_value(&primary, "API声明")
        .or_else(|| extract_field_value(&primary, "差异内容"))
        .unwrap_or_else(|| primary.trim().to_string());

    if declaration.trim().is_empty() || declaration.trim() == "NA" {
        return None;
    }

    let api_name = extract_api_name(&declaration, class_name.as_deref().unwrap_or(""));
    let old_declaration = if old_raw.trim() == "NA" || old_raw.trim().is_empty() {
        None
    } else {
        Some(old_raw.trim().to_string())
    };

    Some(ApiEntry {
        id: None,
        kit: kit.to_string(),
        dts_file,
        module,
        class_name,
        declaration: declaration.trim().to_string(),
        api_name,
        change_type: change_type.to_string(),
        version_label: version_label.to_string(),
        api_level,
        old_declaration,
        source_url: source_url.to_string(),
    })
}

/// 按未转义的 `|` 切分 Markdown 表格行。
pub(crate) fn split_md_row(line: &str) -> Vec<String> {
    let line = line.trim().trim_start_matches('|').trim_end_matches('|');
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut escape = false;
    for c in line.chars() {
        if escape {
            cur.push(c);
            escape = false;
            continue;
        }
        if c == '\\' {
            escape = true;
            continue;
        }
        if c == '|' {
            out.push(std::mem::take(&mut cur));
            continue;
        }
        cur.push(c);
    }
    out.push(cur);
    out
}

/// 把单元格内的 `<br>` 转成换行、去掉其它 HTML 标签、解码实体。
fn normalize_cell(s: &str) -> String {
    let s = s
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("<br>", "\n");
    let s = strip_html_tags(&s);
    decode_entities(&s)
}

/// 提取类似 "类名：xxx" 的字段值（中文冒号或英文冒号）
fn extract_field_value(text: &str, field: &str) -> Option<String> {
    for line in text.lines() {
        let t = line.trim();
        // 尝试 "字段：值" 或 "字段:值"
        for sep in [&format!("{field}："), &format!("{field}:")] {
            if let Some(idx) = t.find(sep.as_str()) {
                let v = t[idx + sep.len()..].trim();
                if !v.is_empty() {
                    // 类名/类型名后面可能带分号，剥掉；声明语句保留原始分号
                    let v = if field == "类名" {
                        v.trim_end_matches('；').trim_end_matches(';').trim()
                    } else {
                        v
                    };
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// 写入数据库（批量，UPSERT）
fn store_entries(conn: &Connection, entries: &[ApiEntry]) -> Result<usize, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut count = 0;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO api_docs
                    (kit, dts_file, module, class_name, declaration, api_name,
                     change_type, version_label, api_level, old_declaration, source_url, fetched_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(version_label, kit, dts_file, class_name, declaration)
                 DO UPDATE SET
                    change_type=excluded.change_type,
                    api_name=excluded.api_name,
                    api_level=excluded.api_level,
                    old_declaration=excluded.old_declaration,
                    source_url=excluded.source_url,
                    fetched_at=excluded.fetched_at",
            )
            .map_err(|e| e.to_string())?;
        for e in entries {
            stmt.execute(params![
                e.kit,
                e.dts_file,
                e.module,
                e.class_name,
                e.declaration,
                e.api_name,
                e.change_type,
                e.version_label,
                e.api_level,
                e.old_declaration,
                e.source_url,
                now,
            ])
            .map_err(|e| e.to_string())?;
            count += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(count)
}

/// 刷新结果汇总
#[derive(Debug, Clone, Default, Serialize)]
pub struct RefreshReport {
    pub versions_fetched: usize,
    pub pages_fetched: usize,
    pub entries_inserted: usize,
    pub errors: Vec<String>,
}

/// 进度回调
pub type ProgressCb = Box<dyn Fn(&RefreshProgress) + Send + Sync>;

/// 全量刷新：遍历所有已知版本与 Kit diff 页面。
pub async fn refresh_all(db: &crate::db::DbState, mut on_progress: Option<ProgressCb>) -> Result<RefreshReport, String> {
    let report = Mutex::new(RefreshReport::default());
    let errors: std::sync::Arc<Mutex<Vec<String>>> = std::sync::Arc::new(Mutex::new(Vec::new()));

    // 先发现所有 (version, kit_url)
    let mut targets: Vec<(String, String, String, Option<u32>)> = Vec::new();
    for (slug, label, level) in KNOWN_VERSIONS {
        match discover_kit_pages(slug, *level).await {
            Ok(links) => {
                for (kit, url, lvl) in links {
                    targets.push((label.to_string(), kit, url, lvl));
                }
            }
            Err(e) => {
                errors.lock().unwrap().push(format!("[{label}] 版本页发现失败: {e}"));
            }
        }
    }

    let total = targets.len();
    if let Some(cb) = &on_progress {
        cb(&RefreshProgress {
            phase: "fetch_pages".to_string(),
            current: 0,
            total,
            message: format!("发现 {total} 个 Kit 页面"),
        });
    }

    // 逐页抓取（并发度受限，避免被限流）
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    let all_entries = std::sync::Arc::new(Mutex::new(Vec::<ApiEntry>::new()));
    let pages_fetched = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let progress_cb: Option<std::sync::Arc<dyn Fn(&RefreshProgress) + Send + Sync>> =
        on_progress.take().map(std::sync::Arc::from);

    let mut handles = Vec::new();
    for (idx, (version_label, kit, url, level)) in targets.into_iter().enumerate() {
        let sem = sem.clone();
        let all_entries = all_entries.clone();
        let pages_fetched = pages_fetched.clone();
        let errors = errors.clone();
        let cb = progress_cb.clone();
        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.ok();
            let object_id = object_id_from_url(&url).unwrap_or_default();
            match fetch_release_doc(&object_id).await {
                Ok(html) => {
                    let entries = parse_diff_page(&html, &kit, &version_label, level, &url);
                    let n = entries.len();
                    all_entries.lock().unwrap().extend(entries);
                    pages_fetched.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if let Some(cb) = &cb {
                        cb(&RefreshProgress {
                            phase: "fetch_pages".to_string(),
                            current: idx + 1,
                            total,
                            message: format!("{kit} {version_label}（{n} 条）"),
                        });
                    }
                }
                Err(e) => {
                    errors.lock().unwrap().push(format!("[{version_label}/{kit}] {e}"));
                    if let Some(cb) = &cb {
                        cb(&RefreshProgress {
                            phase: "fetch_pages".to_string(),
                            current: idx + 1,
                            total,
                            message: format!("{kit} 抓取失败"),
                        });
                    }
                }
            }
        });
        handles.push(handle);
    }
    for h in handles {
        let _ = h.await;
    }

    let all_entries = std::sync::Arc::try_unwrap(all_entries)
        .map_err(|_| "entries lock poisoned".to_string())?
        .into_inner()
        .unwrap();
    let pages_fetched = pages_fetched.load(std::sync::atomic::Ordering::SeqCst);

    if let Some(cb) = &progress_cb {
        cb(&RefreshProgress {
            phase: "storing".to_string(),
            current: all_entries.len(),
            total: all_entries.len(),
            message: "写入数据库".to_string(),
        });
    }

    let inserted = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        store_entries(&conn, &all_entries)?
    };

    let mut r = report.lock().unwrap();
    r.versions_fetched = KNOWN_VERSIONS.len();
    r.pages_fetched = pages_fetched;
    r.entries_inserted = inserted;
    r.errors = std::sync::Arc::try_unwrap(errors)
        .map_err(|_| "errors lock poisoned".to_string())?
        .into_inner()
        .unwrap();

    // 记录最近一次刷新时间
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|x| x.as_secs() as i64)
            .unwrap_or(0);
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let _ = conn.execute(
            "INSERT OR REPLACE INTO api_docs_meta(key,value) VALUES ('last_refreshed_at', ?1)",
            params![now.to_string()],
        );
        let _ = conn.execute(
            "INSERT OR REPLACE INTO api_docs_meta(key,value) VALUES ('last_refreshed_entries', ?1)",
            params![inserted.to_string()],
        );
    }

    Ok(r.clone())
}

/// 搜索 API
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub keyword: Option<String>,
    pub module: Option<String>,
    pub kit: Option<String>,
    pub api_level: Option<u32>,
    pub change_type: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
pub struct SearchResult {
    pub total: usize,
    pub entries: Vec<ApiEntry>,
    /// 该 API 的首次引入版本（从 added 记录聚合）
    pub since: Option<(String, Option<u32>)>,
}

pub fn search(conn: &Connection, q: &SearchQuery) -> Result<Vec<ApiEntry>, String> {
    let mut sql = String::from("SELECT id, kit, dts_file, module, class_name, declaration, api_name, change_type, version_label, api_level, old_declaration, source_url FROM api_docs WHERE 1=1");
    let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(kw) = &q.keyword {
        if !kw.trim().is_empty() {
            sql.push_str(" AND (declaration LIKE ?1 OR api_name LIKE ?1 OR module LIKE ?1 OR class_name LIKE ?1)");
            args.push(Box::new(format!("%{kw}%")));
        }
    }
    if let Some(m) = &q.module {
        sql.push_str(&format!(" AND module LIKE ?{}", args.len() + 1));
        args.push(Box::new(format!("%{m}%")));
    }
    if let Some(k) = &q.kit {
        sql.push_str(&format!(" AND kit LIKE ?{}", args.len() + 1));
        args.push(Box::new(format!("%{k}%")));
    }
    if let Some(l) = q.api_level {
        sql.push_str(&format!(" AND api_level = ?{}", args.len() + 1));
        args.push(Box::new(l));
    }
    if let Some(c) = &q.change_type {
        sql.push_str(&format!(" AND change_type = ?{}", args.len() + 1));
        args.push(Box::new(c.clone()));
    }
    sql.push_str(" ORDER BY api_level DESC, kit, class_name LIMIT ?");
    let limit = q.limit.unwrap_or(100) as i64;
    args.push(Box::new(limit));

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let rows = stmt
        .query_map(params_ref.as_slice(), |row| {
            Ok(ApiEntry {
                id: row.get(0)?,
                kit: row.get(1)?,
                dts_file: row.get(2)?,
                module: row.get(3)?,
                class_name: row.get(4)?,
                declaration: row.get(5)?,
                api_name: row.get(6)?,
                change_type: row.get(7)?,
                version_label: row.get(8)?,
                api_level: row.get(9)?,
                old_declaration: row.get(10)?,
                source_url: row.get(11)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 查询某个 API/模块的首次引入版本（扫描所有 added 记录取最小 api_level）
#[allow(dead_code)]
pub fn find_introduced_version(conn: &Connection, api_name: &str) -> Option<(String, Option<u32>)> {
    let mut stmt = conn
        .prepare(
            "SELECT version_label, api_level FROM api_docs
             WHERE change_type='added' AND (api_name = ?1 OR declaration LIKE ?2)
             ORDER BY api_level ASC LIMIT 1",
        )
        .ok()?;
    let pat = format!("%{api_name}%");
    stmt.query_row(params![api_name, pat], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<u32>>(1)?))
    })
    .ok()
}

/// 数据库中已收录的条目总数
pub fn count(conn: &Connection) -> Result<usize, String> {
    conn.query_row("SELECT COUNT(*) FROM api_docs", [], |row| row.get(0))
        .map_err(|e| e.to_string())
}

/// 查询某个模块/API 在数据库中记录的最低引入版本（added 记录的最小 api_level）。
/// 传入一个 import 片段，如 "@ohos.file.fs" 或 "@kit.AbilityKit"。
/// 返回 (最低 api_level, 一个示例声明)。
pub fn min_introduced_version(conn: &Connection, module_like: &str) -> Option<(u32, String)> {
    // 优先按 module 精确/前缀匹配，再按 declaration 模糊匹配
    let pattern = format!("%{module_like}%");
    let mut stmt = conn
        .prepare(
            "SELECT api_level, declaration FROM api_docs
             WHERE change_type='added' AND api_level IS NOT NULL
               AND (module LIKE ?1 OR dts_file LIKE ?1 OR declaration LIKE ?1)
             ORDER BY api_level ASC LIMIT 1",
        )
        .ok()?;
    stmt.query_row(params![pattern], |row| {
        Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
    })
    .ok()
}

/// 批量查询：给定一组模块关键字，返回 (关键字, 最低引入版本) 列表。
#[allow(dead_code)]
pub fn min_introduced_versions(conn: &Connection, modules: &[&str]) -> Vec<(String, u32, String)> {
    let mut out = Vec::new();
    for m in modules {
        if let Some((lvl, decl)) = min_introduced_version(conn, m) {
            out.push((m.to_string(), lvl, decl));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_md_links_parses_bullets() {
        let md = "* [Ability Kit](https://x/js-apidiff-abilitykit-7001)\n* [ArkUI](https://x/js-apidiff-arkui-7001)";
        let links = extract_md_links(md);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].0, "Ability Kit");
        assert!(links[0].1.contains("abilitykit"));
    }

    #[test]
    fn parse_diff_table_parses_added_api_markdown() {
        let md = "|操作|旧版本|新版本|d.ts文件|\n|:---|:---|:---|:---|\n|新增API|NA|类名：skillManager；<br/>API声明：function getSkillInfoForSelf(moduleName: string): Promise\\<SkillInfo\\>;<br/>差异内容：function getSkillInfoForSelf(...)|api/@ohos.bundle.skillManager.d.ts|";
        let entries = parse_diff_table(md, "Ability Kit", "26.0.0 Beta1", Some(26), "http://x");
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.change_type, "added");
        assert_eq!(e.class_name.as_deref(), Some("skillManager"));
        assert_eq!(e.api_name.as_deref(), Some("getSkillInfoForSelf"));
        assert_eq!(e.module.as_deref(), Some("@ohos.bundle.skillManager"));
        assert_eq!(e.api_level, Some(26));
    }

    #[test]
    fn parse_diff_page_parses_html_table() {
        // 26.0.0 起正文为 HTML 表格（原 .md 端点下线），须与 Markdown 分支等价解析
        let html = r#"<html><body><div class="tablenoborder"><table><thead><tr>
            <th>操作</th><th>旧版本</th><th>新版本</th><th>d.ts文件</th></tr></thead><tbody><tr>
            <td>新增API</td><td>NA</td>
            <td>类名：skillManager；<br>API声明：function getSkillInfoForSelf(moduleName: string): Promise&lt;SkillInfo&gt;;<br>差异内容：function getSkillInfoForSelf(...)</td>
            <td>api/@ohos.bundle.skillManager.d.ts</td></tr></tbody></table></div></body></html>"#;
        let entries = parse_diff_page(html, "Ability Kit", "26.0.0", Some(26), "http://x");
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.change_type, "added");
        assert_eq!(e.class_name.as_deref(), Some("skillManager"));
        assert_eq!(e.api_name.as_deref(), Some("getSkillInfoForSelf"));
        assert_eq!(e.module.as_deref(), Some("@ohos.bundle.skillManager"));
        assert_eq!(e.api_level, Some(26));
    }

    #[test]
    fn discover_starters_cover_known_slug_shapes() {
        let s = starter_slugs("2600");
        assert!(s.contains(&"apidiff-2600".to_string()));
        assert!(s.contains(&"apidiff-26001".to_string()));
        let s = starter_slugs("5-0-1");
        assert!(s.contains(&"apidiff-5-0-1".to_string()));
        assert!(s.contains(&"apidiff-501".to_string()));
        assert!(s.contains(&"apidiff-from-501-release".to_string()));
    }

    #[test]
    fn module_from_dts_handles_paths() {
        assert_eq!(
            module_from_dts("api/@ohos.app.ability.scriptManager.d.ts"),
            Some("@ohos.app.ability.scriptManager".to_string())
        );
        assert_eq!(
            module_from_dts("api/@arkts.collections.d.ets"),
            Some("@arkts.collections".to_string())
        );
        assert_eq!(module_from_dts("api/bundleManager/SkillInfo.d.ts"), None);
    }

    #[test]
    fn extract_api_name_variants() {
        assert_eq!(
            extract_api_name("function getSkillInfoForSelf(a: string): Promise<void>;", "skillManager"),
            Some("getSkillInfoForSelf".to_string())
        );
        assert_eq!(
            extract_api_name("readonly requestCode: string;", "ArkTSScriptInfo"),
            Some("requestCode".to_string())
        );
        assert_eq!(
            extract_api_name("export enum AgentCardType", "agentConstant"),
            Some("AgentCardType".to_string())
        );
        assert_eq!(
            extract_api_name("APP = 0", "AgentCardType"),
            Some("APP".to_string())
        );
    }

    #[test]
    fn extract_field_value_works() {
        let text = "类名：scriptManager\nAPI声明：function foo(): void;\n差异内容：function foo(): void;";
        assert_eq!(extract_field_value(text, "类名").as_deref(), Some("scriptManager"));
        assert_eq!(extract_field_value(text, "API声明").as_deref(), Some("function foo(): void;"));
    }

    /// 端到端联网测试：抓取一个真实 Kit diff 页 → 解析 → 写入内存库 → 按关键字查回。
    /// 默认 ignore，避免离线环境/CI 失败；本地用 `cargo test -- --ignored` 手动跑。
    #[tokio::test]
    #[ignore = "需要联网，手动执行：cargo test --lib -- --ignored"]
    async fn e2e_fetch_parse_store_search() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/028_api_docs.sql"))
            .unwrap();

        // 26.0.0 Release 的 Basic Services Kit diff（objectId 后缀 7003 为 Release 入口）
        let url = "https://developer.huawei.com/consumer/cn/doc/harmonyos-releases/js-apidiff-basicserviceskit-7003";
        let html = fetch_release_doc("js-apidiff-basicserviceskit-7003")
            .await
            .expect("fetch");
        let entries = parse_diff_page(&html, "Basic Services Kit", "26.0.0", Some(26), url);
        assert!(!entries.is_empty(), "至少应解析到 1 条 diff；若华为站点结构变更，需检查 parser");
        let n = store_entries(&conn, &entries).expect("store");
        assert!(n > 0, "写入行数应 > 0");

        let total: usize = conn
            .query_row("SELECT COUNT(*) FROM api_docs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, n);

        // search 能按关键字命中
        let q = SearchQuery {
            keyword: entries[0].api_name.clone(),
            ..Default::default()
        };
        let hits = search(&conn, &q).expect("search");
        assert!(!hits.is_empty(), "search_api 应能命中刚写入的 API");
        assert_eq!(hits[0].api_level, Some(26));
    }
}
