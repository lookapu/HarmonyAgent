//! 失败对话反思（Reflexion）：任务结束后分析会话事件日志中最近一轮的失败工具调用，
//! 提炼"哪一步决策错了 / 哪条工具参数错了"的教训，沉淀为反思卡片；
//! 下一轮 system prompt 注入，让 Agent 跨轮记住自己的失败模式，不再重复踩坑。
//!
//! 与 diagnostics.rs 的分工：diagnostics 缓存的是"构建/崩溃的客观归因结论"（来自工具返回），
//! 本模块缓存的是"模型决策层面的教训"（失败工具序列 + 参数 + 启发式建议）——
//! 前者回答"为什么失败"，后者回答"下次怎么不失败"。
//!
//! 设计取舍：进程内 Mutex<Vec> 缓存（同 diagnostics，短 TTL 运行态信息，重启清空合理），
//! 分析在任务结束后同步执行（开销小：只回放最近一轮事件），失败静默不影响主流程。

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::agent::session_events::{replay, SessionEventType};

/// 一张反思卡片：一个可复用的失败教训。
#[derive(Clone, Debug)]
pub struct ReflexionCard {
    /// 失败模式一句话（如 "build_project 连续 3 次失败"）
    pub pattern: String,
    /// 涉及工具
    pub tool: String,
    /// 证据（失败输出摘要，截断）
    pub evidence: String,
    /// 决策改进建议（注入 prompt 的"下次怎么做"）
    pub advice: String,
    /// 记录时的 unix 秒
    pub at: i64,
    /// 钉住标记：钉住的卡片不受 TTL 清理，常驻注入直到手动解除
    pub pinned: bool,
}

/// 反思卡片缓存（按时间倒序，最多 6 张）
static CARDS: Mutex<Option<Vec<ReflexionCard>>> = Mutex::new(None);

fn now_sec() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn cards() -> std::sync::MutexGuard<'static, Option<Vec<ReflexionCard>>> {
    let mut guard = CARDS.lock().unwrap();
    if guard.is_none() {
        *guard = Some(Vec::new());
    }
    guard
}

/// 从失败输出提炼"下次怎么做"的启发式建议（按错误关键词匹配，先命中先返回）。
fn suggest(tool: &str, out_lower: &str) -> String {
    let pairs: &[(&str, &str)] = &[
        (
            "sdkpath",
            "先检查 SDK 路径/版本对齐（check_sdk_alignment / harmony_env），不要重复相同构建",
        ),
        (
            "签名",
            "先 show_diagnose_card(category=signing) 或 check_signature 排查签名配置，再重新构建",
        ),
        (
            "device_offline",
            "先 list_devices 确认设备在线/默认设备选择，再重试部署",
        ),
        (
            "cannot find module",
            "先用 codebase_search/read_file 确认导入路径与实际文件结构一致，再修复 import",
        ),
        (
            "old 内容在文件中未找到",
            "edit_file 的 old 文本必须直接复制 read_file 输出原文（注意缩进/引号/转义），改前先重读文件",
        ),
        (
            "重复调用",
            "同一工具同参数已连续失败，立即更换策略（读文件定位 → 换工具/换参数），不要原地重试",
        ),
        (
            "超时",
            "任务/命令超时后应拆分为更小的步骤重试，不要原样重复长命令",
        ),
        (
            "非 utf-8",
            "目标文件可能是 GBK 编码，先转 UTF-8 再编辑，不要直接改写",
        ),
        (
            "未找到 @arkts",
            "LSP 包未安装：npm i -g @arkts/language-server 后重试；或改用文本扫描工具（get_symbol_details）",
        ),
        (
            "token",
            "请求被 token 限额拦截：先压缩上下文或切换模型，再继续任务",
        ),
    ];
    for (kw, advice) in pairs {
        if out_lower.contains(kw) {
            return (*advice).to_string();
        }
    }
    // 兜底：按工具类型给通用建议
    match tool {
        "edit_file" | "write_file" => {
            "先 read_file 读取最新内容再编辑；连续失败时改用 start 模式（按代码块替换）".to_string()
        }
        "build_project" | "deploy" | "deploy_hap" => {
            "连续构建/部署失败后先读取错误分类与定位（file:line）并修复源码，再重试验证".to_string()
        }
        _ => "连续失败后应更换策略：读取相关文件定位根因 → 换工具/换参数 → 验证，不要重复相同调用".to_string(),
    }
}

/// 任务结束后调用：回放会话事件，分析最近一轮（最后一条 user 消息之后）的失败工具调用，
/// 提炼反思卡片。仅在存在可提炼教训（同一工具或整体失败 ≥2 次）时写入；
/// 同 (tool, pattern) 去重保留最新。任何异常静默（不阻塞主流程）。
pub fn analyze_conversation(conn: &Connection, conversation_id: &str) {
    let events = match replay(conn, conversation_id) {
        Ok(e) => e,
        Err(_) => return,
    };
    // 定位最近一轮起点：最后一条 UserMessage 之后（该轮含工具调用/失败/总结）
    let start = events
        .iter()
        .rposition(|e| e.event_type == SessionEventType::UserMessage)
        .map(|i| i + 1)
        .unwrap_or(0);
    let round = &events[start..];
    if round.is_empty() {
        return;
    }
    // 配对：ToolCall 记录当前工具名，失败 ToolResult 归因到最近的工具
    let mut last_tool: Option<String> = None;
    let mut fails: Vec<(String, String)> = Vec::new(); // (tool, 失败输出)
    for ev in round {
        match ev.event_type {
            SessionEventType::ToolCall => {
                last_tool = ev
                    .payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
            SessionEventType::ToolResult => {
                let ok = ev.payload.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                if !ok {
                    let out = ev
                        .payload
                        .get("output")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(t) = last_tool.clone() {
                        fails.push((t, out));
                    }
                }
            }
            _ => {}
        }
    }
    if fails.len() < 2 {
        return; // 单次失败不构成模式，不沉淀教训（避免噪音卡片）
    }
    // 同一工具失败次数（取最多的那个作为卡片主体）
    let mut by_tool: Vec<(String, usize, String)> = Vec::new();
    for (t, out) in &fails {
        if let Some(e) = by_tool.iter_mut().find(|(tt, _, _)| tt == t) {
            e.1 += 1;
            if out.len() > e.2.len() {
                e.2 = out.clone();
            }
        } else {
            by_tool.push((t.clone(), 1, out.clone()));
        }
    }
    by_tool.sort_by_key(|a| std::cmp::Reverse(a.1));
    let (tool, count, evidence) = by_tool[0].clone();
    let evidence_clean: String = evidence.split_whitespace().collect::<Vec<_>>().join(" ");
    let evidence_short: String = evidence_clean.chars().take(160).collect();
    let out_lower = evidence.to_lowercase();
    let advice = suggest(&tool, &out_lower);
    let pattern = if count >= 3 {
        format!("{tool} 连续失败 {count} 次（同一思路反复踩坑）")
    } else {
        format!("本轮任务中 {tool} 失败 {count} 次")
    };
    let card = ReflexionCard {
        pattern,
        tool,
        evidence: evidence_short,
        advice,
        at: now_sec(),
        pinned: false,
    };
    let mut guard = cards();
    let list = guard.as_mut().unwrap();
    // 同 (tool, pattern 前缀) 去重：替换旧卡片（保留最新证据）
    if let Some(old) = list.iter_mut().find(|c| c.tool == card.tool) {
        *old = card;
    } else {
        list.push(card);
        // 只留最近 6 张
        if list.len() > 6 {
            list.sort_by_key(|a| std::cmp::Reverse(a.at));
            list.truncate(6);
        }
    }
}

/// 读取反思卡片并格式化为注入 system prompt 的文本（无卡片时返回空串）。
/// 钉住的卡片（pinned）不受 TTL 限制常驻注入，直到 reflexion_pin 解除。
pub fn format_hint(ttl_sec: i64) -> String {
    let cutoff = now_sec() - ttl_sec;
    let mut guard = cards();
    let list = match guard.as_mut() {
        Some(l) => l,
        None => return String::new(),
    };
    list.retain(|c| c.pinned || c.at >= cutoff);
    if list.is_empty() {
        return String::new();
    }
    let mut s = String::from("【失败对话反思（最近任务留下的教训，新任务中避免重复同类错误）】\n");
    for c in list.iter() {
        s.push_str(&format!("- [{}] {}{}\n", c.tool, c.pattern, if c.pinned { "（🔒 已钉住）" } else { "" }));
        if !c.evidence.is_empty() {
            s.push_str(&format!("  证据：{}\n", c.evidence));
        }
        s.push_str(&format!("  下次做法：{}\n", c.advice));
    }
    s
}

/// 查询全部反思卡片（新→旧，含钉住状态），供 reflexion_query 工具展示。
pub fn query_cards() -> Vec<ReflexionCard> {
    let mut guard = cards();
    let list = guard.as_mut().unwrap();
    let mut sorted = list.clone();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.at));
    sorted
}

/// 钉住/解除钉住某工具的反思卡片（同工具多张时钉住最新的那张）。
/// 返回操作后的卡片摘要。
pub fn pin_card(tool: &str, pinned: bool) -> Result<String, String> {
    let mut guard = cards();
    let list = guard.as_mut().unwrap();
    let Some(card) = list.iter_mut().find(|c| c.tool == tool) else {
        return Err(format!("没有工具 {tool} 的反思卡片（先用 reflexion_query 查看现有卡片）"));
    };
    card.pinned = pinned;
    Ok(format!(
        "已{} {tool} 的反思卡片：{}\n（钉住的卡片不受 TTL 清理，常驻注入 system prompt）",
        if pinned { "钉住" } else { "解除钉住" },
        card.pattern
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 卡片缓存是进程级静态量，且本模块多个测试共享它：
    /// 并行运行时会出现“A 测试分析出的卡片被 B 测试清空/断言读到对方卡片”的竞态，
    /// 因此用静态锁把这些测试串行化。
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 内存库 + session_events 表（与 032 迁移同构 + 034 trace_id 列）
    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                payload TEXT NOT NULL DEFAULT '{}',
                trace_id TEXT,
                created_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE INDEX idx_session_events_conv_seq ON session_events(conversation_id, seq);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn single_failure_produces_no_card() {
        let _guard = TEST_LOCK.lock().unwrap();
        // 清空缓存，保证断言时只反映本测试的分析结果
        cards().as_mut().unwrap().clear();
        let conn = mem_conn();
        crate::agent::session_events::append_event(
            &conn,
            "c1",
            SessionEventType::UserMessage,
            serde_json::json!({"content": "构建"}),
            None,
        )
        .unwrap();
        crate::agent::session_events::append_event(
            &conn,
            "c1",
            SessionEventType::ToolCall,
            serde_json::json!({"name": "build_project", "args": {}}),
            Some("t1"),
        )
        .unwrap();
        crate::agent::session_events::append_event(
            &conn,
            "c1",
            SessionEventType::ToolResult,
            serde_json::json!({"ok": false, "output": "签名错误"}),
            Some("t1"),
        )
        .unwrap();
        analyze_conversation(&conn, "c1");
        assert!(format_hint(3600).is_empty(), "单次失败不应产出卡片");
    }

    #[test]
    fn repeated_failure_produces_card_with_advice() {
        let _guard = TEST_LOCK.lock().unwrap();
        let conn = mem_conn();
        crate::agent::session_events::append_event(
            &conn,
            "c1",
            SessionEventType::UserMessage,
            serde_json::json!({"content": "构建"}),
            None,
        )
        .unwrap();
        for _ in 0..3 {
            crate::agent::session_events::append_event(
                &conn,
                "c1",
                SessionEventType::ToolCall,
                serde_json::json!({"name": "build_project", "args": {"mode": "debug"}}),
                Some("t1"),
            )
            .unwrap();
            crate::agent::session_events::append_event(
                &conn,
                "c1",
                SessionEventType::ToolResult,
                serde_json::json!({"ok": false, "output": "签名配置缺失，无法构建"}),
                Some("t1"),
            )
            .unwrap();
        }
        analyze_conversation(&conn, "c1");
        let hint = format_hint(3600);
        assert!(hint.contains("build_project"), "应产出 build_project 卡片：{hint}");
        assert!(hint.contains("签名"), "建议应命中签名规则：{hint}");
        // 同工具再次分析：去重不增长
        analyze_conversation(&conn, "c1");
        let hint2 = format_hint(3600);
        assert_eq!(hint.matches("build_project").count(), hint2.matches("build_project").count());
    }

    #[test]
    fn other_conversation_failure_not_mixed() {
        let _guard = TEST_LOCK.lock().unwrap();
        let conn = mem_conn();
        crate::agent::session_events::append_event(
            &conn,
            "c2",
            SessionEventType::UserMessage,
            serde_json::json!({"content": "改代码"}),
            None,
        )
        .unwrap();
        for _ in 0..3 {
            crate::agent::session_events::append_event(
                &conn,
                "c2",
                SessionEventType::ToolCall,
                serde_json::json!({"name": "edit_file", "args": {}}),
                Some("t2"),
            )
            .unwrap();
            crate::agent::session_events::append_event(
                &conn,
                "c2",
                SessionEventType::ToolResult,
                serde_json::json!({"ok": false, "output": "old 内容在文件中未找到"}),
                Some("t2"),
            )
            .unwrap();
        }
        analyze_conversation(&conn, "c2");
        let hint = format_hint(3600);
        assert!(hint.contains("edit_file"), "应产出 edit_file 卡片：{hint}");
    }
}
