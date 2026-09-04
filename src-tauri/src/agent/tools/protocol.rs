//! 工具调用标记协议层：`【TOOL|工具名|JSON参数】` 标记的解析、清理与注入防护。
//! 纯函数层，不依赖工具实现，可独立测试。

use std::path::Path;

use super::TOOL_SPECS;


pub const MARK_START: &str = "【TOOL|";
pub const MARK_END: &str = "】";

/// 容错查找标记结束位置（返回 (结束符起始偏移, 结束符长度)，相对 after 开头）。
/// 优先按 JSON 结构定位参数末尾：字符串内的 】/]}/]/换行 一律按内容跳过，
/// 避免 edit_file/write_file 等含代码的参数被误截断（曾导致参数后半泄漏进正文、
/// JSON 残缺而工具执行失败）；JSON 结束后按优先级匹配结束符：】 > ]} > ]，
/// 结束符缺失时以 JSON 末尾为界（不吞后续正文——参数完整即边界权威，
/// 模型漏写结束符且正文紧跟同行时按换行截断会误删整行正文）。
/// JSON 畸形（模型输出残缺）时回退旧的全文搜索逻辑，保证畸形标记仍能被识别并剥离。
fn mark_end_offset(after: &str) -> Option<(usize, usize)> {
    if let Some(end) = try_json_param_end(after) {
        let tail = &after[end..];
        let lead = tail.len() - tail.trim_start().len();
        let pos = end + lead;
        let t = &after[pos..];
        if t.starts_with(MARK_END) {
            return Some((pos, MARK_END.len()));
        }
        if t.starts_with("]}") {
            return Some((pos, 2));
        }
        if t.starts_with(']') {
            return Some((pos, 1));
        }
        // 结束符缺失：参数已完整，以 JSON 末尾为界（不消费任何字符）
        return Some((end, 0));
    }
    // 回退：参数非完整 JSON 时的旧容错逻辑
    if let Some(e) = after.find(MARK_END) {
        return Some((e, MARK_END.len()));
    }
    if let Some(e) = after.find("]}") {
        return Some((e, 2));
    }
    if let Some(e) = after.rfind(']') {
        return Some((e, 1));
    }
    if let Some(e) = after.find('\n') {
        return Some((e, 1));
    }
    None
}

/// after 形如 "工具名|JSON参数…"：跳过工具名与前导空白，按 JSON 结构扫描参数，
/// 返回参数结束后相对 after 开头的字节偏移；参数不是完整 JSON 返回 None。
fn try_json_param_end(after: &str) -> Option<usize> {
    let pipe = after.find('|')?;
    let params = &after[pipe + 1..];
    let head = skip_json_ws(params.as_bytes(), 0);
    let len = scan_json_value(params, head, 0)?;
    Some(pipe + 1 + len)
}

/// 从 i 起跳过 JSON 空白，返回偏移（越界时停留在末尾）
fn skip_json_ws(b: &[u8], mut i: usize) -> usize {
    while b.get(i).is_some_and(|c| (*c as char).is_whitespace()) {
        i += 1;
    }
    i
}

/// JSON 嵌套深度上限：模型参数正常不超过十几层，超深嵌套（恶意/异常输出）
/// 直接放弃结构扫描走回退逻辑，防止递归栈溢出
const MAX_JSON_DEPTH: usize = 64;

/// 从 s[i] 起扫描一个完整 JSON 值，返回该值结束后的字节偏移。
/// 宽松处理：字符串内的真实换行（JSON 标准不允许）也按内容跳过，
/// 保证模型未转义换行的参数不会把后半截泄漏进正文。
fn scan_json_value(s: &str, i: usize, depth: usize) -> Option<usize> {
    if depth > MAX_JSON_DEPTH {
        return None;
    }
    let b = s.as_bytes();
    let c = *b.get(i)?;
    match c {
        b'{' => {
            let mut j = skip_json_ws(b, i + 1);
            if b.get(j) == Some(&b'}') {
                return Some(j + 1);
            }
            loop {
                if b.get(j) != Some(&b'"') {
                    return None;
                }
                let end = scan_json_str(b, j)?;
                j = skip_json_ws(b, end);
                if b.get(j) != Some(&b':') {
                    return None;
                }
                j = scan_json_value(s, skip_json_ws(b, j + 1), depth + 1)?;
                j = skip_json_ws(b, j);
                match b.get(j) {
                    Some(&b',') => j = skip_json_ws(b, j + 1),
                    Some(&b'}') => return Some(j + 1),
                    _ => return None,
                }
            }
        }
        b'[' => {
            let mut j = skip_json_ws(b, i + 1);
            if b.get(j) == Some(&b']') {
                return Some(j + 1);
            }
            loop {
                j = scan_json_value(s, skip_json_ws(b, j), depth + 1)?;
                j = skip_json_ws(b, j);
                match b.get(j) {
                    Some(&b',') => j = skip_json_ws(b, j + 1),
                    Some(&b']') => return Some(j + 1),
                    _ => return None,
                }
            }
        }
        b'"' => scan_json_str(b, i),
        b't' | b'f' | b'n' => {
            let lit = if c == b't' {
                "true"
            } else if c == b'f' {
                "false"
            } else {
                "null"
            };
            if s[i..].starts_with(lit) {
                Some(i + lit.len())
            } else {
                None
            }
        }
        _ if c == b'-' || c.is_ascii_digit() => scan_json_num(b, i),
        _ => None,
    }
}

/// 扫描字符串字面量（b[i] 必须为 '"'），返回闭合引号后的偏移。
/// 转义统一跳过 2 字节（\uXXXX 的剩余字节作为普通内容跳过，不影响定位）
fn scan_json_str(b: &[u8], i: usize) -> Option<usize> {
    if b.get(i) != Some(&b'"') {
        return None;
    }
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// 扫描数字字面量：可选负号、整数/小数、可选指数
fn scan_json_num(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    if b.get(j) == Some(&b'-') {
        j += 1;
    }
    let digits = j;
    while b.get(j).is_some_and(|c| c.is_ascii_digit()) {
        j += 1;
    }
    if j == digits {
        return None;
    }
    if b.get(j) == Some(&b'.') {
        j += 1;
        let frac = j;
        while b.get(j).is_some_and(|c| c.is_ascii_digit()) {
            j += 1;
        }
        if j == frac {
            return None;
        }
    }
    if matches!(b.get(j), Some(&b'e') | Some(&b'E')) {
        j += 1;
        if matches!(b.get(j), Some(&b'+') | Some(&b'-')) {
            j += 1;
        }
        let exp = j;
        while b.get(j).is_some_and(|c| c.is_ascii_digit()) {
            j += 1;
        }
        if j == exp {
            return None;
        }
    }
    Some(j)
}

/// 清理参数尾部杂散字符：模型误写结束符后参数尾部残留 ] }（如 ..."}]} 多出 ]}），
/// 逐个去掉直到 JSON 可解析，提升容错解析后的工具执行成功率
fn clean_args_tail(args: &str) -> String {
    let mut a = args.trim().to_string();
    loop {
        if a.is_empty() || serde_json::from_str::<serde_json::Value>(&a).is_ok() {
            break;
        }
        if a.ends_with(']') || a.ends_with('}') {
            a.pop();
        } else {
            break;
        }
    }
    a
}

/// 生成系统提示中的工具说明
fn selected_specs(query: &str) -> Vec<&'static super::ToolSpec> {
    let names = super::capabilities::selected_tool_names(query, 40);
    names.into_iter().filter_map(|name| {
        TOOL_SPECS.iter().find(|spec| spec.name == name)
    }).collect()
}

fn selected_specs_for_phase(
    query: &str,
    phase: super::capabilities::TaskPhase,
) -> Vec<&'static super::ToolSpec> {
    super::capabilities::selected_tool_names_for_phase(query, phase, 32)
        .into_iter()
        .filter_map(|name| TOOL_SPECS.iter().find(|spec| spec.name == name))
        .collect()
}

pub fn system_hint_for(query: &str) -> String {
    let mut hint = String::from("本轮能力包（按最小工具集执行）：\n");
    for pack in super::capabilities::select(query) {
        hint.push_str(&format!(
            "- {}：顺序 {}；停止条件 {}；验收 {}\n",
            pack.id,
            pack.recommended_order.join(" → "),
            pack.stop_conditions.join(" / "),
            pack.acceptance.join(" / "),
        ));
    }
    hint.push('\n');
    hint.push_str(&system_hint_from_specs(selected_specs(query).into_iter()));
    hint
}

pub fn phase_hint_for(query: &str, phase: super::capabilities::TaskPhase) -> String {
    format!(
        "当前工具阶段：{}。本轮仅使用以下阶段工具；需要阶段外能力时先说明证据和切换理由。\n{}",
        phase.as_str(),
        system_hint_from_specs(selected_specs_for_phase(query, phase).into_iter()),
    )
}

pub fn phase_hint_for_names(
    phase: super::capabilities::TaskPhase,
    names: &[String],
) -> String {
    let specs = names.iter().filter_map(|name| {
        TOOL_SPECS.iter().find(|spec| spec.name == name)
    });
    format!(
        "当前工具阶段：{}。工具已结合历史成功率、预计成本/耗时、副作用与当前环境排序；本轮仅使用以下阶段工具。\n{}",
        phase.as_str(),
        system_hint_from_specs(specs),
    )
}

fn system_hint_from_specs<'a>(specs: impl Iterator<Item = &'a super::ToolSpec>) -> String {
    let mut s = String::from(
        "你可以调用开发工具完成构建/部署等任务。需要调用工具时，在回复中单独输出一行标记（不要用 Markdown 代码块包裹，不要加解释）：\n\
         【TOOL|工具名|JSON参数】\n可用工具：\n",
    );
    for t in specs {
        s.push_str(&format!("- {}：{}\n", t.name, t.desc));
    }
    s.push_str(
        "工具使用规则：\n\
         - 工具调用唯一方式是输出工具标记行【TOOL|工具名|JSON参数】（一行一个，一条回复可输出多个，系统会依次执行）；\n\
         - “（已调用工具 xxx）”或“已调用工具 xxx”这类文字只是系统展示历史记录时的说明文案，不代表工具已执行；请勿在正文中叙述“已调用工具”，写了系统也不会执行；\n\
         - 需要读取多个文件/执行多步操作时，可在一条回复中连续输出多个工具标记，系统会依次执行；\n\
         - 复杂多步骤任务开始前先 todo_write 拆分清单，完成一项标记一项，界面会实时展示进度；\n\
         - 改完 UI 并部署后，用 take_screenshot 截图验证：截图会自动以图片形式进入你的视野，直接观察真机界面，判断布局/样式/文字是否达标，再决定是否继续修；\n\
         - 修改代码前先定位再动手：默认先用 search_symbols 按 entity/logic 结构定位并取得签名与完整行区间，再用 read_file 读取目标区块；需要文本召回或索引覆盖不足时用 codebase_search/LSP 补查，跨结构上下文确有必要时才读全文；\n\
         - 接手陌生工程或大范围重构前先 deep_scan 了解全库结构与依赖；改完代码后用 check_code 自查调试残留/硬编码密钥等常见问题；\n\
         - 大任务可 spawn_agents 并行委派互不依赖的子任务，执行后用 list_agents 回看各子任务结果；\n\
         - 工具执行失败时，根据错误信息分析原因，给出修复建议或改用其他工具；不要编造工具结果；\n\
         - 不确定某个工具的用法/参数时，先 tool_help name=<工具名> 查详细说明（含权限级别、执行预期、参数示例），或 tool_list 查看完整工具清单与超时/成本提示；\n\
         - 回看工具调用历史（如“刚才那个工具为什么失败”）用 tool_history；项目数据库只读查询用 db_query（仅 SELECT，自动只读保护）；\n\
         - 同一工具失败后最多重试一次，重试仍失败应放弃该路径并调整策略；\n\
         - 每完成一个可展示的产出（写完文件/生成图片/报告等），写总结前用 ui_focus 把界面聚焦到成果（打开文件预览或切换面板）——用户不会主动注意到你的产出，同一成果不要重复聚焦；\n\
         - 长任务中跨轮必须记住的关键事实（用户原始约束、确定的方案、踩过的坑）用 memorize 记录（put/update/delete），系统会自动注入后续轮次系统提示；不要重复记忆同一事实；\n\
         - 连续多次重复调用同一工具且参数相同会被判定为打转并终止任务，请确保每次调用有新的进展；\n\
         - 任务目标已达成或无法继续时，直接总结结果，不要反复调用工具。\n\
         输出格式：\n\
         - 清单、步骤、行动列表等使用 Markdown 有序/无序列表（1. 或 -，每项一行），不要用代码块包裹普通文本；代码块仅用于代码、配置、目录树等需要等宽展示的内容。",
    );
    s
}

/// 生成技能库注入提示：把启用 Skill 的 SKILL.md 指令喂给 Agent。
/// - 仅注入启用的技能（与 MCP 同框架：单条失败不影响整体）；
/// - 数量与长度受限（最多 8 个、每个 3000 字符），防止上下文膨胀；
/// - 无 SKILL.md 或读取失败时仅注入名称/描述。
pub fn skill_hint(skills: &[crate::db::models::Skill]) -> String {
    let enabled: Vec<&crate::db::models::Skill> = skills.iter().filter(|s| s.enabled).collect();
    if enabled.is_empty() {
        return String::new();
    }
    // 同名技能去重（全局 + 项目级）：list_skills 按“全局在前、项目级在后”排序，
    // 同名时项目级覆盖全局（位置保持首次出现处，内容取后出现者）
    let mut unique: Vec<&crate::db::models::Skill> = Vec::new();
    for s in enabled {
        match unique.iter().position(|u| u.name == s.name) {
            Some(pos) => unique[pos] = s,
            None => unique.push(s),
        }
    }
    let mut parts: Vec<String> = Vec::new();
    for s in unique.iter().take(8) {
        let mut block = format!(
            "【{} v{} | {}】{}",
            s.name,
            s.skill_version,
            s.compatibility_status,
            s.description.as_deref().unwrap_or("")
        );
        if let Some(dir) = &s.directory {
            if let Some(content) = read_skill_md(dir) {
                let validated = crate::services::skill_manifest::parse_and_validate(&content).ok();
                let hash_matches = validated.as_ref().is_some_and(|manifest| {
                    manifest.compatibility_status != "incompatible"
                        && s.content_hash.as_deref().is_none_or(|hash| hash == manifest.content_hash)
                });
                if hash_matches {
                    block.push_str(&format!("\n指令内容：\n{}", truncate_chars(&content, 3000)));
                } else {
                    block.push_str("\n指令内容未注入：清单不兼容、无效或导入后内容哈希漂移，请重新审核导入。");
                }
            }
        }
        parts.push(block);
    }
    if unique.len() > 8 {
        parts.push(format!(
            "（另有 {} 个技能未注入完整指令，如需使用可参考 Skill 页中的内容）",
            unique.len() - 8
        ));
    }
    format!(
        "技能库（Skill）——如果任务与以下技能相关，必须先调用 use_skill 工具（参数 {{\"name\":\"技能名\"}}）声明正在使用该技能，然后严格遵循技能指令完成任务：\n\n{}",
        parts.join("\n\n")
    )
}

/// 读取技能目录下的 SKILL.md（兼容大小写变体）
pub(super) fn read_skill_md(dir: &str) -> Option<String> {
    let p = Path::new(dir);
    for candidate in ["SKILL.md", "skill.md"] {
        let f = p.join(candidate);
        if f.is_file() {
            if let Ok(c) = std::fs::read_to_string(&f) {
                return Some(c);
            }
        }
    }
    None
}

/// 按字符数截断（末尾附加说明）
pub(super) fn truncate_chars(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        s.chars().take(n).collect::<String>() + "\n…(内容已截断，完整指令见 Skill 页)"
    } else {
        s.to_string()
    }
}

/// 从文本中提取全部工具调用标记（按出现顺序），返回 (工具名, 参数字符串) 列表
/// 模型可能在一轮输出里连续写入多个标记（如连续读多个文件），全部解析依次执行，
/// 避免只执行第一个导致其余标记成为"假卡片"并让模型误以为结果丢失而打转。
pub fn parse_tool_calls(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(MARK_START) {
        let after = &rest[start + MARK_START.len()..];
        let Some((end, len)) = mark_end_offset(after) else { break };
        let body = after[..end].trim();
        let mut parts = body.splitn(2, '|');
        let name = parts.next().unwrap_or("").trim().to_string();
        if !name.is_empty() {
            // 容错：清理误写结束符后残留的尾部杂散字符（如 ]} ），提升参数解析成功率
            let args = clean_args_tail(parts.next().unwrap_or(""));
            out.push((name, args));
        }
        rest = &after[end + len..];
    }
    out
}

/// 剔除文本中的全部工具调用标记，仅保留正文（用于入库/展示，避免残留标记被渲染成假卡片）
pub fn strip_tool_calls(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(MARK_START) {
        out.push_str(&rest[..start]);
        let after = &rest[start + MARK_START.len()..];
        let Some((end, len)) = mark_end_offset(after) else {
            // 标记 JSON 残缺且无任何结束符（模型输出被流式截断）：参数与后续正文
            // 无边界可分，整段丢弃——宁可丢少量正文也不把代码碎片泄漏进入库文本
            return out;
        };
        rest = &after[end + len..];
    }
    out.push_str(rest);
    out
}

/// 工具短描述：取 TOOL_SPECS 中 desc 的第一行（一句话说明），供前端卡片悬浮提示；未登记（如 MCP 工具）返回空
pub fn tool_short_desc(name: &str) -> &'static str {
    TOOL_SPECS
        .iter()
        .find(|s| s.name == name)
        .and_then(|s| s.desc.split('\n').next())
        .unwrap_or("")
}

fn parameter_segment(desc: &str) -> String {
    desc.find("参数：")
        .map(|i| {
            let rest = &desc[i + "参数：".len()..];
            let mut seg = String::new();
            for line in rest.lines() {
                let t = line.trim();
                if seg.is_empty() {
                    seg = t.to_string();
                } else if t.starts_with(',') || t.starts_with('{') {
                    seg.push_str(t);
                } else {
                    break;
                }
            }
            seg.trim()
                .trim_end_matches(['。', '.', '；', ';', ' '])
                .to_string()
        })
        .unwrap_or_default()
}

fn parameter_map(seg: &str) -> serde_json::Map<String, serde_json::Value> {
    if !seg.starts_with('{') {
        return serde_json::Map::new();
    }
    serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(seg)
        .unwrap_or_else(|_| extract_param_keys(seg))
}

pub(crate) fn declared_parameter_names(tool: &str) -> Vec<String> {
    TOOL_SPECS.iter().find(|spec| spec.name == tool)
        .map(|spec| parameter_map(&parameter_segment(spec.desc)).into_iter()
            .map(|(key, _)| key).collect())
        .unwrap_or_default()
}

fn required_parameter_keys(seg: &str, keys: impl Iterator<Item = String>) -> Vec<String> {
    keys.filter(|key| {
        let marker = format!("\"{key}\"");
        let Some(start) = seg.find(&marker) else { return false };
        let tail = &seg[start + marker.len()..];
        let end = tail.find(",\"").or_else(|| tail.find(", \"")).unwrap_or(tail.len());
        let declaration = &tail[..end];
        declaration.contains("必填") || declaration.contains("required")
    }).collect()
}

/// 生成 OpenAI function calling 工具 schema（工具协议标准化 Phase 1：原生工具调用）。
/// 从 ToolSpec.desc 的「参数：{...}」段提取参数对象：优先严格 JSON 解析；
/// desc 中参数值常带中文注解（如 `"host":"<设备 IP>"（connect/disconnect 需要）`，非严格 JSON），
/// 解析失败时退化为键名扫描（properties 保留全部键，值给宽松 string schema）。
/// 说明取第一行；MCP / Skill 动态工具不在注册表内，由调用方另行注入。
pub fn tool_schemas() -> Vec<serde_json::Value> {
    TOOL_SPECS
        .iter()
        .map(|spec| {
            let desc = spec.desc;
            // 参数段：从「参数：」起按 JSON 行接续规则提取——首行即参数 JSON 开头，
            // 后续行仅当以 `,`/`{` 起始（JSON 跨行延续）时拼接，遇到其他段落标题即停止。
            // 相比按段落标题截断更稳：desc 段落标题不止 副作用/返回/适合/前提（还有"比 read_file…"等叙述行）
            let params_seg = parameter_segment(desc);
            let params = parameter_map(&params_seg);
            // 参数 schema：值统一 string 类型并保留原始说明文本（含缺省值提示）；
            // 数字/布尔由工具执行侧宽松解析兜底（字符串可 as_str/parse 回原始类型）
            let properties = params
                .iter()
                .map(|(k, v)| {
                    let desc_text = v.as_str().unwrap_or("").to_string();
                    (
                        k.clone(),
                        serde_json::json!({ "type": "string", "description": desc_text }),
                    )
                })
                .collect::<serde_json::Map<_, _>>();
            let required = required_parameter_keys(&params_seg, params.keys().cloned());
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": spec.name,
                    "description": tool_short_desc(spec.name),
                    "parameters": {
                        "type": "object",
                        "properties": properties,
                        "required": required,
                        "additionalProperties": false,
                    }
                }
            })
        })
        .collect()
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ToolArgumentIssue {
    pub code: &'static str,
    pub field: Option<String>,
    pub suggestion: String,
    pub sensitive: bool,
}

/// 按注册表 schema 检查原生工具参数并返回纠错建议。
///
/// 此函数只诊断、从不改写参数。MCP 工具的 schema 由运行期服务提供，不在静态注册表
/// 内，因此仍交给 MCP 服务校验。建议文本不包含参数值，避免敏感信息二次扩散。
pub fn validate_tool_arguments(name: &str, args_raw: &str) -> Vec<ToolArgumentIssue> {
    let Some(spec) = TOOL_SPECS.iter().find(|spec| spec.name == name) else {
        return Vec::new();
    };
    let seg = parameter_segment(spec.desc);
    let params = parameter_map(&seg);
    let required = required_parameter_keys(&seg, params.keys().cloned());
    let value = if args_raw.trim().is_empty() {
        serde_json::json!({})
    } else {
        match serde_json::from_str::<serde_json::Value>(args_raw) {
            Ok(value) => value,
            Err(err) => return vec![ToolArgumentIssue {
                code: "invalid_json",
                field: None,
                suggestion: format!("请传入合法 JSON 对象（解析错误位于第 {} 行第 {} 列）", err.line(), err.column()),
                sensitive: false,
            }],
        }
    };
    let Some(object) = value.as_object() else {
        return vec![ToolArgumentIssue {
            code: "expected_object",
            field: None,
            suggestion: "工具参数必须是 JSON 对象，例如 {}".to_string(),
            sensitive: false,
        }];
    };
    let mut issues = Vec::new();
    for key in &required {
        if !object.contains_key(key) {
            issues.push(ToolArgumentIssue {
                code: "missing_required",
                field: Some(key.clone()),
                suggestion: format!("补充必填字段 `{key}` 后重试"),
                sensitive: is_sensitive_parameter(key),
            });
        }
    }
    for key in object.keys() {
        if params.contains_key(key) {
            continue;
        }
        let candidate = params.keys()
            .map(|known| (edit_distance(key, known), known))
            .min_by_key(|(distance, _)| *distance)
            .filter(|(distance, known)| {
                *distance <= 3
                    || known.starts_with(key)
                    || key.starts_with(known.as_str())
            })
            .map(|(_, known)| known.clone());
        let sensitive = is_sensitive_parameter(key)
            || candidate.as_deref().is_some_and(is_sensitive_parameter);
        let suggestion = match candidate {
            Some(candidate) if sensitive => format!(
                "未知字段 `{key}` 可能是敏感字段 `{candidate}`；请显式核对字段名和值，系统不会自动修正"
            ),
            Some(candidate) => format!("未知字段 `{key}`；是否应为 `{candidate}`？请修正后重试"),
            None => format!("移除未知字段 `{key}`，或先查看 `{name}` 的参数 schema"),
        };
        issues.push(ToolArgumentIssue {
            code: "unknown_field",
            field: Some(key.clone()),
            suggestion,
            sensitive,
        });
    }
    issues
}

pub fn tool_argument_error(name: &str, args_raw: &str) -> Option<String> {
    let issues = validate_tool_arguments(name, args_raw);
    if issues.is_empty() {
        return None;
    }
    let details = issues.iter().map(|issue| {
        let marker = if issue.sensitive { " [敏感参数：禁止自动修正]" } else { "" };
        format!("- {}{}", issue.suggestion, marker)
    }).collect::<Vec<_>>().join("\n");
    Some(format!(
        "工具 `{name}` 参数未通过 schema 校验，本次未执行且未自动改写：\n{details}"
    ))
}

fn is_sensitive_parameter(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    ["token", "secret", "password", "passwd", "private", "certificate", "cert", "profile", "keystore", "sign", "device", "serial", "sn"]
        .iter()
        .any(|marker| key.contains(marker))
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right_chars = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right_chars.len()).collect::<Vec<_>>();
    for (i, left_char) in left.chars().enumerate() {
        let mut current = vec![i + 1];
        for (j, right_char) in right_chars.iter().enumerate() {
            current.push((previous[j + 1] + 1).min(current[j] + 1).min(
                previous[j] + usize::from(left_char != *right_char),
            ));
        }
        previous = current;
    }
    previous[right_chars.len()]
}

pub fn tool_schemas_for(query: &str) -> Vec<serde_json::Value> {
    let selected: std::collections::HashSet<&str> = selected_specs(query)
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    tool_schemas()
        .into_iter()
        .filter(|schema| {
            schema.pointer("/function/name")
                .and_then(|v| v.as_str())
                .is_some_and(|name| selected.contains(name))
        })
        .collect()
}

pub fn tool_schemas_for_phase(
    query: &str,
    phase: super::capabilities::TaskPhase,
) -> Vec<serde_json::Value> {
    let selected: std::collections::HashSet<&str> = selected_specs_for_phase(query, phase)
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    tool_schemas().into_iter().filter(|schema| {
        schema.pointer("/function/name")
            .and_then(|value| value.as_str())
            .is_some_and(|name| selected.contains(name))
    }).collect()
}

pub fn tool_schemas_for_names(names: &[String]) -> Vec<serde_json::Value> {
    let schemas = tool_schemas();
    names.iter().filter_map(|name| {
        schemas.iter().find(|schema| {
            schema.pointer("/function/name").and_then(|value| value.as_str())
                == Some(name.as_str())
        }).cloned()
    }).collect()
}

/// 参数段非严格 JSON（desc 里参数值常带中文注解）时的键名提取回退：
/// 扫描 `"键名":` 模式（键为 ASCII 标识符），值为宽松 string schema。
fn extract_param_keys(seg: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut out = serde_json::Map::new();
    let bytes = seg.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'"' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'"' {
                end += 1;
            }
            if end < bytes.len() {
                let key = &seg[start..end];
                let tail = &bytes[end + 1..];
                let skip = tail.iter().take_while(|&&b| b == b' ' || b == b'\t').count();
                if tail.get(skip) == Some(&b':')
                    && !key.is_empty()
                    && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    out.insert(
                        key.to_string(),
                        serde_json::json!({ "type": "string", "description": "" }),
                    );
                }
            }
            i = end.saturating_add(1);
        } else {
            i += 1;
        }
    }
    out
}

/// 解析 MCP 工具名 `mcp__服务器名__工具名` → (服务器名, 工具名)；非 MCP 工具返回 None
pub fn parse_mcp_tool_name(name: &str) -> Option<(String, String)> {
    let mut it = name.splitn(3, "__");
    if it.next()? != "mcp" {
        return None;
    }
    let server = it.next()?.to_string();
    let tool = it.next()?.to_string();
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server, tool))
}

/// 拆同名多实例后缀 `mysql#2` → (mysql, 2)；无后缀返回 None
pub fn split_instance_name(name: &str) -> Option<(&str, usize)> {
    let (base, n) = name.rsplit_once('#')?;
    if base.is_empty() {
        return None;
    }
    let n: usize = n.parse().ok()?;
    if n == 0 {
        return None;
    }
    Some((base, n))
}

/// 生成 MCP 工具说明（注入系统提示；动态来源于 tools/list）
/// entries: (服务器名, 工具定义) 列表，总计最多列出 40 个工具（防上下文爆炸）。
/// 同名多实例由 load_mcp_hint 编号为 name#n 后传入（不在此处区分）；
/// 此处去重仅防御性：真重名（如历史残留）时后出现者覆盖先出现者。
pub fn mcp_tools_hint(entries: &[(String, crate::services::mcp_client::McpToolDef)]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    // 去重（防御性）：真重名时后出现者覆盖先出现者，位置保持首次出现处
    let mut unique: Vec<&(String, crate::services::mcp_client::McpToolDef)> = Vec::new();
    for e in entries {
        match unique
            .iter()
            .position(|u| u.0 == e.0 && u.1.name == e.1.name)
        {
            Some(pos) => unique[pos] = e,
            None => unique.push(e),
        }
    }
    let mut s = String::from(
        "MCP 服务器工具（调用格式：【TOOL|mcp__服务器名__工具名|JSON参数】）：\n",
    );
    let mut listed = 0usize;
    let mut skipped = 0usize;
    for (server, t) in &unique {
        if listed >= 40 {
            skipped += 1;
            continue;
        }
        let desc: String = t.description.trim().chars().take(300).collect();
        let schema: String = t.input_schema.to_string();
        let schema = if schema.chars().count() > 500 {
            schema.chars().take(500).collect::<String>() + "…"
        } else {
            schema
        };
        s.push_str(&format!(
            "- mcp__{server}__{}：{desc}\n  参数 JSON Schema: {schema}\n",
            t.name
        ));
        listed += 1;
    }
    if skipped > 0 {
        s.push_str(&format!("（另有 {skipped} 个 MCP 工具超出上限未列出）\n"));
    }
    s
}

/// 历史消息回放时清理工具调用痕迹（避免模型重复触发/模仿）。
/// 标记与模型模仿出的“（已调用工具 xxx）”“【历史工具调用记录：xxx】”叙述一律删除：
/// 曾多次出现模型看到历史里的调用叙述后误以为这就是调用方式，只写叙述不输出标记，
/// 导致工具从未执行而任务“中断”的恶性循环，故历史中不保留任何“工具调用”字样。
pub fn sanitize_markers(text: &str) -> String {
    // 第一遍：删除【TOOL|...】标记（结束符容错：模型偶发把】写成 ]} 或 ]，或漏写）
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(MARK_START) {
        out.push_str(&rest[..start]);
        let after = &rest[start + MARK_START.len()..];
        if let Some((end, len)) = mark_end_offset(after) {
            rest = &after[end + len..];
        } else {
            // 残缺标记无结束符：丢弃标记段及其后内容（不把代码碎片回灌历史）
            rest = "";
        }
    }
    out.push_str(rest);
    // 第二遍：删除“（已调用工具 xxx）”叙述式文本（旧格式污染）
    const NARR_START: &str = "（已调用工具 ";
    let mut out2 = String::new();
    let mut rest2 = out.as_str();
    while let Some(start) = rest2.find(NARR_START) {
        out2.push_str(&rest2[..start]);
        let after = &rest2[start + NARR_START.len()..];
        if let Some(end) = after.find('）') {
            rest2 = &after[end + '）'.len_utf8()..];
        } else {
            // 残缺叙述（流式截断无闭合括号）：丢弃其后内容，与第一遍残缺标记处理一致
            rest2 = "";
        }
    }
    out2.push_str(rest2);
    // 第三遍：删除“【历史工具调用记录：xxx】”叙述式文本（新格式污染）
    const REC_START: &str = "【历史工具调用记录：";
    let mut out3 = String::new();
    let mut rest3 = out2.as_str();
    while let Some(start) = rest3.find(REC_START) {
        out3.push_str(&rest3[..start]);
        let after = &rest3[start + REC_START.len()..];
        if let Some(end) = after.find('】') {
            rest3 = &after[end + '】'.len_utf8()..];
        } else {
            // 残缺叙述（流式截断无闭合符号）：丢弃其后内容，防止半截话术回灌历史
            rest3 = "";
        }
    }
    out3.push_str(rest3);
    out3
}

/// 工具结果注入防护：外部内容（文件/命令输出/MCP 返回）里的指令性文字
/// 仅作信息参考，不构成对 Agent 的新指令。检测到疑似指令注入特征
/// （如「忽略之前的指令」）时，从命中处截断并标注，防止恶意内容指挥模型。
/// 只影响喂给模型的内容，不影响入库原文与前端展示。
pub fn sanitize_tool_output(out: &str) -> String {
    // 中文注入特征（无大小写问题，直接精确查找）
    const CN_PATTERNS: [&str; 13] = [
        "忽略之前", "忽略以上", "无视之前", "无视以上", "不要理会之前", "不用理会之前",
        "忽略系统提示", "忽略系统指令", "忽略系统消息", "忽略系统规则", "忽略你的指令",
        "不再遵守", "推翻之前的",
    ];
    for pat in CN_PATTERNS {
        if let Some(pos) = out.find(pat) {
            return inject_cutoff(out, pos, pat);
        }
    }
    // 英文注入特征（ASCII 大小写不敏感查找，不依赖 to_lowercase 的偏移对齐）
    const EN_PATTERNS: [&str; 7] = [
        "ignore all previous", "ignore previous", "ignore the system",
        "disregard previous", "override your instructions", "ignore your instructions",
        "forget everything",
    ];
    for pat in EN_PATTERNS {
        if let Some(pos) = find_ci_ascii(out, pat) {
            return inject_cutoff(out, pos, pat);
        }
    }
    out.to_string()
}

/// 从命中位置截断工具结果：保留命中前内容，丢弃疑似注入区并标注
fn inject_cutoff(out: &str, pos: usize, pat: &str) -> String {
    let head = out[..pos].trim_end();
    format!("{head}\n\n…[内容已截断：检测到疑似指令注入片段（{pat}），后续内容不予处理]")
}

/// ASCII 大小写不敏感查找，返回原始字符串中的字节偏移（字符边界安全）
fn find_ci_ascii(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || n.len() > h.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| {
        haystack.is_char_boundary(i) && (0..n.len()).all(|j| h[i + j].to_ascii_lowercase() == n[j])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_schemas_cover_all_registered_tools() {
        let schemas = tool_schemas();
        assert_eq!(schemas.len(), TOOL_SPECS.len());
        for (spec, schema) in TOOL_SPECS.iter().zip(&schemas) {
            assert_eq!(schema["type"], "function");
            assert_eq!(schema["function"]["name"], spec.name);
            assert!(!schema["function"]["description"].as_str().unwrap_or("").is_empty());
        }
    }

    #[test]
    fn task_tool_selection_keeps_core_and_reduces_catalog() {
        let schemas = tool_schemas_for("修复对话界面卡死并运行测试");
        let names: Vec<&str> = schemas.iter().filter_map(|s| s.pointer("/function/name")?.as_str()).collect();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"edit_file"));
        assert!(names.contains(&"run_tests"));
        assert!(names.len() <= 40);
        assert!(names.len() < TOOL_SPECS.len());
    }

    #[test]
    fn system_hint_contains_selected_pack_controls() {
        let hint = system_hint_for("构建并部署到真机");
        assert!(hint.contains("build_deploy"));
        assert!(hint.contains("停止条件"));
        assert!(hint.contains("验收"));
        assert!(hint.contains("list_devices"));
    }

    #[test]
    fn native_schemas_follow_execution_phase() {
        use super::super::capabilities::TaskPhase;
        let goal = "修复失败测试后提交并推送";
        let names = |phase| tool_schemas_for_phase(goal, phase).into_iter()
            .filter_map(|schema| schema.pointer("/function/name")?.as_str().map(str::to_string))
            .collect::<Vec<_>>();
        let explore = names(TaskPhase::Explore);
        let verify = names(TaskPhase::Verify);
        let deliver = names(TaskPhase::Deliver);
        assert!(!explore.contains(&"edit_file".into()));
        assert!(verify.contains(&"run_tests".into()));
        assert!(!verify.contains(&"git_push".into()));
        assert!(deliver.contains(&"git_push".into()));
        assert!(explore.len() <= 32 && verify.len() <= 32 && deliver.len() <= 32);
    }

    #[test]
    fn ranked_names_control_schema_and_hint_order() {
        let names = vec!["git_status".to_string(), "read_file".to_string()];
        let schemas = tool_schemas_for_names(&names);
        assert_eq!(schemas[0]["function"]["name"], "git_status");
        assert_eq!(schemas[1]["function"]["name"], "read_file");
        let hint = phase_hint_for_names(super::super::capabilities::TaskPhase::Explore, &names);
        assert!(hint.contains("历史成功率"));
        assert!(hint.find("- git_status").unwrap() < hint.find("- read_file").unwrap());
    }

    #[test]
    fn tool_schemas_extract_named_params() {
        let schemas = tool_schemas();
        // connect_device 有显式参数（含中文注解，非严格 JSON → 键名扫描回退）→ properties 应包含 host/port/sn
        let conn = schemas
            .iter()
            .find(|s| s["function"]["name"] == "connect_device")
            .expect("connect_device schema missing");
        let props = &conn["function"]["parameters"]["properties"];
        for key in ["action", "host", "port", "sn"] {
            assert!(props.get(key).is_some(), "缺少参数 {key}");
            assert_eq!(props[key]["type"], "string");
        }
        // 严格 JSON 参数的工具（read_module_config 无注解、值全在引号内）应保留原始说明文本
        let cfg = schemas
            .iter()
            .find(|s| s["function"]["name"] == "read_module_config");
        if let Some(cfg) = cfg {
            let props = &cfg["function"]["parameters"]["properties"];
            if props.get("module").is_some() {
                assert!(!props["module"]["description"].as_str().unwrap_or("").is_empty());
            }
        }
    }

    #[test]
    fn tool_schemas_allow_no_param_tools() {
        let schemas = tool_schemas();
        // list_devices 参数：无 → properties 为空 object（仍可原生调用）
        let dev = schemas
            .iter()
            .find(|s| s["function"]["name"] == "list_devices")
            .expect("list_devices schema missing");
        assert_eq!(dev["function"]["parameters"]["properties"].as_object().unwrap().len(), 0);
    }

    #[test]
    fn tool_schemas_parse_parameters_directly() {
        // 直接验证参数段提取：跨行 JSON 参数应被解析（read_module_config 的参数段跨两行）
        let schemas = tool_schemas();
        let cfg = schemas
            .iter()
            .find(|s| s["function"]["name"] == "read_module_config");
        if let Some(cfg) = cfg {
            let props = &cfg["function"]["parameters"]["properties"];
            assert!(props.get("module").is_some() || props.get("file").is_some());
        }
    }

    #[test]
    fn argument_validation_suggests_schema_field_without_rewriting() {
        let issues = validate_tool_arguments("read_file", r#"{"pth":"src/main.rs"}"#);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "unknown_field");
        assert_eq!(issues[0].field.as_deref(), Some("pth"));
        assert!(issues[0].suggestion.contains("`path`"));
        assert!(!issues[0].sensitive);
    }

    #[test]
    fn argument_validation_reports_required_and_sensitive_fields() {
        let issues = validate_tool_arguments("ota_pack", r#"{"profile_pth":"demo.p7b"}"#);
        assert!(issues.iter().any(|issue| {
            issue.code == "missing_required" && issue.field.as_deref() == Some("hap_path")
        }));
        assert!(issues.iter().any(|issue| {
            issue.code == "missing_required" && issue.field.as_deref() == Some("out_path")
        }));
        let sensitive = issues.iter().find(|issue| issue.field.as_deref() == Some("profile_pth"))
            .expect("sensitive typo issue missing");
        assert!(sensitive.sensitive);
        assert!(sensitive.suggestion.contains("不会自动修正"));
    }

    #[test]
    fn argument_validation_rejects_malformed_or_non_object_json() {
        let malformed = validate_tool_arguments("read_file", r#"{"path":}"#);
        assert_eq!(malformed[0].code, "invalid_json");
        let array = validate_tool_arguments("read_file", "[]");
        assert_eq!(array[0].code, "expected_object");
    }

    #[test]
    fn argument_validation_leaves_dynamic_mcp_tools_to_runtime() {
        assert!(validate_tool_arguments("mcp__demo__search", r#"{"anything":1}"#).is_empty());
    }
}
