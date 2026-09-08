//! AgentKernel 共享消息历史组装策略（Phase A/E：纯函数 + Assembler）。
//!
//! 纯策略、无 IO、无 Tauri 依赖：UI（chat.rs）与 headless（headless_driver.rs）共用。
//! Phase A 先搬入两个纯函数（`dynamic_history_limit`、`estimate_tokens`）；
//! Phase E 实现 `KernelHistoryAssembler` 接管 system/history/tool/注入/续写/纠正中段组装。

use crate::agent::tools::{has_pending_action_phrase, parse_data_url};
use serde_json;

/// 最小历史保留条数：压缩时至少保留这么多条最近消息，避免把关键上下文全压掉。
const MIN_HISTORY_KEEP: usize = 10;

/// 历史行数上限按模型上下文预算动态计算：预算越大保留越多历史，但有上下限防止
/// 小窗口模型撑爆上下文或大窗口模型历史过短丢失决策语境。
///
/// 公式从 chat.rs 逐字搬入：`(budget / 3000).clamp(20, 60)`
pub fn dynamic_history_limit(context_budget: i64) -> usize {
    ((context_budget / 3000) as usize).clamp(20, 60)
}

/// 消息列表 token 估算：委托给 tokenizer 工具函数，与 chat.rs 原口径一致。
pub fn estimate_tokens(messages: &[serde_json::Value]) -> usize {
    crate::utils::tokenizer::estimate_messages_tokens(messages)
}

/// 输出中断/截断后的统一续写指令。UI 与 headless 必须使用同一文案，避免推理模型在
/// reasoning-only 截断后继续消耗预算输出思考，也避免 adapter 把半截正文重复塞进 user 消息。
pub fn continuation_instruction(reasoning_only: bool) -> &'static str {
    if reasoning_only {
        "（系统提示：你的上一条回复未完成（思考过长或网络中断），本轮请不要再输出思考过程，直接给出最终结论；若任务未完成，直接输出下一步要执行的工具调用标记。）"
    } else {
        "（你的上一条回复未完整送达（被截断或网络中断），请直接从断点继续完成剩余内容，不要重复已输出的部分。）"
    }
}

// ── 历史行输入结构（adapter 从 DB 读出后传入，IO 留在 adapter） ───────────────────────

/// 历史行：adapter 从 messages 表读出后构造（role/content/references_json/reasoning）
#[derive(Clone, Debug)]
pub struct HistoryRow {
    pub role: String,
    pub content: String,
    pub references_json: Option<String>,
    pub reasoning: Option<String>,
}

/// 本轮已执行工具结果：adapter 从 tool_runs 派生后传入
#[derive(Clone, Debug)]
pub struct ToolResult {
    pub tool: String,
    pub output: String,
}

/// 用户注入指令：adapter 从 merged_instructions / session_ctx / replan 等收集后传入
#[derive(Clone, Debug)]
pub struct UserInjection {
    pub content: String,
}

// ── Assembler 输入（所有数据由 adapter 预读好，assembler 只做拼装） ───────────────────

/// KernelHistoryAssembler 输入：adapter 负责所有 IO（DB 查询、文件读取、context 加载），
/// assembler 只负责按固定顺序拼装消息序列。
#[derive(Clone, Debug)]
pub struct KernelHistoryInput<'a> {
    // System 层（adapter 预拼接好每段文本）
    pub system_prompt: &'a str,
    pub memo_replay: Option<&'a str>,
    pub context_hint: Option<&'a str>,
    pub workflow_directive: &'a str,
    pub ledger_hint: Option<&'a str>,
    pub compression_summary: Option<&'a str>,
    pub confirmed_plan: Option<&'a str>,

    // 历史行（adapter 已从 DB 读出并反转顺序）
    pub history_rows: Vec<HistoryRow>,

    // 本轮工具结果（adapter 已从 tool_runs 派生并截断）
    pub tool_results: Vec<ToolResult>,

    // 用户注入（adapter 已收集好所有 user 消息）
    pub user_injections: Vec<UserInjection>,

    // 续写/纠正状态（adapter 从 round_router 得到）
    pub continuation_text: &'a str,
    pub continuation_reasoning_only: bool,
    pub correction_text: &'a str,
    pub correction_hint: &'a str,

    // 进度对照标记（adapter 根据 tools_since_progress 决定）
    pub inject_progress_check: bool,

    // 多模态图片（E2：adapter 预读 supports_image，assembler 负责附加到消息）
    pub images: Option<&'a Vec<String>>,
    pub images_attached: usize,
    pub protocol: &'a str,
    pub supports_image: bool,

    // 压缩决策参数（E3：adapter 传入上下文预算和当前历史限制，assembler 判断是否需要压缩）
    pub context_budget: i64,
    pub history_limit: usize,
}

/// Assembler 输出：消息序列 + 更新后的图片计数 + 压缩决策（adapter 执行实际压缩）
#[derive(Clone, Debug)]
pub struct KernelAssembled {
    pub messages: Vec<serde_json::Value>,
    pub images_attached: usize,
    pub compress: bool, // true 时 adapter 应执行 summarize_rolling_history + 落库 + 事件
}

/// KernelHistoryAssembler：纯策略消息组装器（无 IO，所有数据由 adapter 预读）。
///
/// 组装顺序逐条对齐 chat.rs 4475-4700：
/// system full/core → memo replay → context hint → workflow directive → ledger hint
/// → compression summary → approved plan → 历史行（assistant/tool/user+references）
/// → 本轮工具结果 → 用户注入 → 进度对照 → replan → 续写 → 纠正
pub struct KernelHistoryAssembler;

impl KernelHistoryAssembler {
    /// 组装消息序列：返回 Vec<serde_json::Value> 供 adapter 直接发送给 Provider。
    ///
    /// 图片格式适配和压缩判断也在此完成；adapter 只负责 IO、落库和发送事件。
    pub fn assemble(input: &KernelHistoryInput) -> KernelAssembled {
        let mut messages: Vec<serde_json::Value> = Vec::new();

        // 1. System prompt（adapter 已根据 seam_count 选择 full 或 core）
        messages.push(serde_json::json!({ "role": "system", "content": input.system_prompt }));

        // 2. Memo replay（关键记忆回放，对齐 Qwen-Agent MemoAssistant）
        if let Some(memo) = input.memo_replay {
            messages.push(serde_json::json!({ "role": "system", "content": memo }));
        }

        // 3. Context V2 hint（每轮从 Durable Run 重建）
        if let Some(hint) = input.context_hint {
            messages.push(serde_json::json!({ "role": "system", "content": hint }));
        }

        // 4. Workflow directive
        messages.push(serde_json::json!({
            "role": "system",
            "content": input.workflow_directive,
        }));

        // 5. Task Ledger（任务账本，防长任务"忘记已做过什么/卡在哪一步"）
        if let Some(ledger) = input.ledger_hint {
            messages.push(serde_json::json!({ "role": "system", "content": ledger }));
        }

        // 6. Compression summary（早期对话滚动摘要）
        if let Some(summary) = input.compression_summary {
            messages.push(serde_json::json!({
                "role": "system",
                "content": format!("## 历史摘要（早期对话，已被压缩）\n{summary}"),
            }));
        }

        // 7. Confirmed plan（已批准计划锚定，防中途遗忘/偏离）
        if let Some(plan) = input.confirmed_plan {
            messages.push(serde_json::json!({
                "role": "system",
                "content": format!(
                    "## 已批准任务计划（必须严格遵守，不得擅自偏离或扩大范围）\n{plan}"
                ),
            }));
        }

        // 8. History rows（最近 history_limit 条，含 tool）
        for row in &input.history_rows {
            match row.role.as_str() {
                "assistant" => {
                    let cleaned = crate::agent::tools::sanitize_markers(&row.content);
                    // 未完话术污染过滤：历史上只描述计划未执行工具的短消息不重复喂给模型
                    if cleaned.chars().count() < 300 && has_pending_action_phrase(&cleaned) {
                        messages.push(serde_json::json!({ "role": "user", "content": "（此前有一轮未执行的过渡回复，已省略）" }));
                    } else {
                        // DeepSeek 推理模型多轮合规：携带 tools 参数时必须回传 reasoning_content
                        let mut m = serde_json::json!({ "role": "assistant", "content": cleaned });
                        if let Some(r) = row.reasoning.as_deref() {
                            if !r.trim().is_empty() {
                                m["reasoning_content"] = serde_json::json!(r);
                            }
                        }
                        messages.push(m);
                    }
                }
                "tool" => {
                    // tool 消息入库格式："工具名\n输出"，转 user 消息反馈给模型
                    let (name, out) = row.content.split_once('\n').unwrap_or(("tool", &row.content));
                    let out_guard = crate::agent::tools::sanitize_tool_output(out);
                    const HISTORY_TOOL_RESULT_LIMIT: usize = 1200;
                    let output_chars = out_guard.chars().count();
                    let output: String = out_guard
                        .chars()
                        .take(HISTORY_TOOL_RESULT_LIMIT)
                        .collect();
                    let suffix = if output_chars > HISTORY_TOOL_RESULT_LIMIT {
                        format!("\n...[历史工具输出已截断，共 {output_chars} 字符]")
                    } else {
                        String::new()
                    };
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": format!("[工具执行结果 - {name}]\n{output}{suffix}"),
                    }));
                }
                _ => {
                    // user 消息：references_json 已在 adapter 注入为完整文本
                    messages.push(serde_json::json!({ "role": "user", "content": &row.content }));
                }
            }
        }

        // 9. Tool results（本轮已执行的工具结果，adapter 已截断并防护）
        for item in &input.tool_results {
            messages.push(serde_json::json!({
                "role": "user",
                "content": format!(
                    "[工具执行结果 - {}]\n{}\n\n请根据以上结果继续，若失败请分析原因并给出修复建议。",
                    item.tool, item.output
                ),
            }));
        }

        // 10. User injections（本轮并入的用户挂起指令 / 异步事件 / session_ctx）
        for inj in &input.user_injections {
            messages.push(serde_json::json!({ "role": "user", "content": &inj.content }));
        }

        // 11. Progress check（计划执行进度对照，每执行 3 个工具注入一次）
        if input.inject_progress_check {
            messages.push(serde_json::json!({
                "role": "user",
                "content": "（执行对照：请对照上方\"已批准任务计划\"，用一两句话汇报当前进度——哪些步骤已完成、当前进行到哪一步、还剩哪些步骤，然后继续执行，不要偏离计划。）",
            }));
        }

        // 12. Continuation（输出截断续写：把上轮被截断的内容与"请继续"指令加入本轮）
        if !input.continuation_text.is_empty() || input.continuation_reasoning_only {
            if !input.continuation_text.is_empty() {
                messages.push(serde_json::json!({ "role": "assistant", "content": input.continuation_text }));
            }
            messages.push(serde_json::json!({
                "role": "user",
                "content": continuation_instruction(input.continuation_reasoning_only),
            }));
        }

        // 13. Correction（纠正注入：假调用/未完话术/空响应重试）
        if !input.correction_text.is_empty() || !input.correction_hint.is_empty() {
            if !input.correction_text.is_empty() {
                messages.push(serde_json::json!({ "role": "assistant", "content": input.correction_text }));
            }
            messages.push(serde_json::json!({ "role": "user", "content": input.correction_hint }));
        }

        // 14. Images attachment（E2：多模态图片附加到最后一轮 user 消息）
        let mut images_attached = input.images_attached;
        if let Some(imgs) = input.images {
            if imgs.len() > images_attached {
                let new_imgs: Vec<&String> = imgs[images_attached..].iter().collect();
                if !new_imgs.is_empty() && input.supports_image {
                    // 找到最后一条 user 消息
                    let last_user = messages.iter().rposition(|m| m["role"] == "user");
                    let idx = match last_user {
                        Some(i) => i,
                        None => {
                            messages.push(serde_json::json!({ "role": "user", "content": "" }));
                            messages.len() - 1
                        }
                    };
                    
                    let last = &mut messages[idx];
                    match input.protocol {
                        "gemini" => {
                            if !last["parts"].is_array() {
                                let text = last["content"].as_str().unwrap_or("").to_string();
                                last["parts"] =
                                    serde_json::Value::Array(vec![serde_json::json!({ "text": text })]);
                            }
                            if let Some(parts) = last["parts"].as_array_mut() {
                                for img in &new_imgs {
                                    if let Some((mime, data)) = parse_data_url(img) {
                                        parts.push(serde_json::json!({
                                            "inline_data": { "mime_type": mime, "data": data },
                                        }));
                                    }
                                }
                            }
                        }
                        "anthropic" => {
                            if !last["content"].is_array() {
                                let text = last["content"].as_str().unwrap_or("").to_string();
                                last["content"] = serde_json::Value::Array(vec![serde_json::json!({ "type": "text", "text": text })]);
                            }
                            if let Some(parts) = last["content"].as_array_mut() {
                                for img in &new_imgs {
                                    if let Some((mime, data)) = parse_data_url(img) {
                                        parts.push(serde_json::json!({
                                            "type": "image",
                                            "source": { "type": "base64", "media_type": mime, "data": data },
                                        }));
                                    }
                                }
                            }
                        }
                        _ => {
                            // OpenAI 及其他协议
                            if !last["content"].is_array() {
                                let text = last["content"].as_str().unwrap_or("").to_string();
                                last["content"] = serde_json::Value::Array(vec![serde_json::json!({ "type": "text", "text": text })]);
                            }
                            if let Some(parts) = last["content"].as_array_mut() {
                                for img in &new_imgs {
                                    if let Some((mime, data)) = parse_data_url(img) {
                                        parts.push(serde_json::json!({
                                            "type": "image_url",
                                            "image_url": { "url": format!("data:{mime};base64,{data}") },
                                        }));
                                    }
                                }
                            }
                        }
                    }
                    images_attached = imgs.len();
                } else if !new_imgs.is_empty() && !input.supports_image {
                    // 模型不支持图片：在最后一条 user 消息添加说明文本
                    let note = format!(
                        "（本轮 {} 张截图/图片因当前模型不支持图片输入未附带，模型无法查看图片内容）",
                        new_imgs.len()
                    );
                    let last_user = messages.iter().rposition(|m| m["role"] == "user");
                    if let Some(idx) = last_user {
                        let last = &mut messages[idx];
                        if last["content"].is_string() {
                            let text = last["content"].as_str().unwrap_or("").to_string();
                            last["content"] = serde_json::json!(format!("{text}\n{note}"));
                        } else if let Some(parts) = last["content"].as_array_mut() {
                            parts.push(serde_json::json!({ "type": "text", "text": note }));
                        }
                    }
                    images_attached = imgs.len();
                }
            }
        }

        // E3：压缩决策——估算请求 token，超过模型窗口 85% 时标记需要压缩
        // 注意：实际压缩执行（summarize_rolling_history + 落库 + 事件）留在 adapter
        let compress = input.history_limit > MIN_HISTORY_KEEP
            && estimate_tokens(&messages) > input.context_budget as usize * 85 / 100;

        KernelAssembled { messages, images_attached, compress }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_history_limit_clamps_to_20_for_small_budget() {
        assert_eq!(dynamic_history_limit(0), 20);
        assert_eq!(dynamic_history_limit(30_000), 20);
        assert_eq!(dynamic_history_limit(60_000), 20);
    }

    #[test]
    fn dynamic_history_limit_scales_with_budget() {
        // 120_000 / 3000 = 40
        assert_eq!(dynamic_history_limit(120_000), 40);
        // 150_000 / 3000 = 50
        assert_eq!(dynamic_history_limit(150_000), 50);
    }

    #[test]
    fn dynamic_history_limit_clamps_to_60_for_large_budget() {
        assert_eq!(dynamic_history_limit(180_000), 60);
        assert_eq!(dynamic_history_limit(1_000_000), 60);
    }

    #[test]
    fn estimate_tokens_empty_messages_returns_small_base() {
        let empty: Vec<serde_json::Value> = vec![];
        let est = estimate_tokens(&empty);
        assert!(est < 100, "empty messages should have small base overhead, got {est}");
    }

    #[test]
    fn estimate_tokens_nonzero_for_content() {
        let msgs = vec![serde_json::json!({
            "role": "user",
            "content": "请读取 src/main.rs 文件的内容并分析其中的错误"
        })];
        let est = estimate_tokens(&msgs);
        assert!(est > 0, "expected nonzero token estimate for Chinese content");
    }

    #[test]
    fn continuation_instruction_prevents_repeated_reasoning() {
        assert!(continuation_instruction(true).contains("不要再输出思考过程"));
        assert!(continuation_instruction(true).contains("直接给出最终结论"));
        assert!(continuation_instruction(false).contains("从断点继续"));
        assert!(continuation_instruction(false).contains("不要重复"));
    }

    #[test]
    fn assembler_system_prompt_only() {
        let input = KernelHistoryInput {
            system_prompt: "You are a helpful assistant.",
            memo_replay: None,
            context_hint: None,
            workflow_directive: "Follow the plan.",
            ledger_hint: None,
            compression_summary: None,
            confirmed_plan: None,
            history_rows: vec![],
            tool_results: vec![],
            user_injections: vec![],
            continuation_text: "",
            continuation_reasoning_only: false,
            correction_text: "",
            correction_hint: "",
            inject_progress_check: false,
            images: None,
            images_attached: 0,
            protocol: "",
            supports_image: false,
            context_budget: 128_000,
            history_limit: 40,
        };
        let assembled = KernelHistoryAssembler::assemble(&input);
        assert_eq!(assembled.messages.len(), 2); // system + workflow directive
        assert_eq!(assembled.images_attached, 0);
        assert_eq!(assembled.compress, false); // no history, should not compress
        assert_eq!(assembled.messages[0]["role"], "system");
        assert_eq!(assembled.messages[0]["content"], "You are a helpful assistant.");
        assert_eq!(assembled.messages[1]["role"], "system");
        assert_eq!(assembled.messages[1]["content"], "Follow the plan.");
    }

    #[test]
    fn assembler_with_memo_and_context() {
        let input = KernelHistoryInput {
            system_prompt: "Core rules.",
            memo_replay: Some("Memory: user prefers Rust."),
            context_hint: Some("Context: working on auth module."),
            workflow_directive: "Use tools.",
            ledger_hint: None,
            compression_summary: None,
            confirmed_plan: None,
            history_rows: vec![],
            tool_results: vec![],
            user_injections: vec![],
            continuation_text: "",
            continuation_reasoning_only: false,
            correction_text: "",
            correction_hint: "",
            inject_progress_check: false,
            images: None,
            images_attached: 0,
            protocol: "",
            supports_image: false,
            context_budget: 128_000,
            history_limit: 40,
        };
        let assembled = KernelHistoryAssembler::assemble(&input);
        assert_eq!(assembled.messages.len(), 4); // system + memo + context + workflow
        assert_eq!(assembled.images_attached, 0);
        assert_eq!(assembled.compress, false);
        assert_eq!(assembled.messages[0]["content"], "Core rules.");
        assert_eq!(assembled.messages[1]["content"], "Memory: user prefers Rust.");
        assert_eq!(assembled.messages[2]["content"], "Context: working on auth module.");
        assert_eq!(assembled.messages[3]["content"], "Use tools.");
    }

    #[test]
    fn assembler_with_ledger_and_plan() {
        let input = KernelHistoryInput {
            system_prompt: "System.",
            memo_replay: None,
            context_hint: None,
            workflow_directive: "Workflow.",
            ledger_hint: Some("Ledger: step 1 done."),
            compression_summary: None,
            confirmed_plan: Some("Plan: 1. Read 2. Write"),
            history_rows: vec![],
            tool_results: vec![],
            user_injections: vec![],
            continuation_text: "",
            continuation_reasoning_only: false,
            correction_text: "",
            correction_hint: "",
            inject_progress_check: false,
            images: None,
            images_attached: 0,
            protocol: "",
            supports_image: false,
            context_budget: 128_000,
            history_limit: 40,
        };
        let assembled = KernelHistoryAssembler::assemble(&input);
        assert_eq!(assembled.messages.len(), 4); // system + workflow + ledger + plan
        assert_eq!(assembled.images_attached, 0);
        assert_eq!(assembled.compress, false);
        assert_eq!(assembled.messages[0]["content"], "System.");
        assert_eq!(assembled.messages[1]["content"], "Workflow.");
        assert!(assembled.messages[2]["content"].as_str().unwrap().contains("Ledger:"));
        assert!(assembled.messages[3]["content"].as_str().unwrap().contains("已批准任务计划"));
    }

    #[test]
    fn assembler_history_assistant_with_reasoning() {
        let input = KernelHistoryInput {
            system_prompt: "System.",
            memo_replay: None,
            context_hint: None,
            workflow_directive: "W.",
            ledger_hint: None,
            compression_summary: None,
            confirmed_plan: None,
            history_rows: vec![
                HistoryRow {
                    role: "assistant".to_string(),
                    content: "Let me analyze this.".to_string(),
                    references_json: None,
                    reasoning: Some("Thinking about the problem...".to_string()),
                },
            ],
            tool_results: vec![],
            user_injections: vec![],
            continuation_text: "",
            continuation_reasoning_only: false,
            correction_text: "",
            correction_hint: "",
            inject_progress_check: false,
            images: None,
            images_attached: 0,
            protocol: "",
            supports_image: false,
            context_budget: 128_000,
            history_limit: 40,
        };
        let assembled = KernelHistoryAssembler::assemble(&input);
        // system + workflow + assistant (with reasoning)
        assert_eq!(assembled.messages.len(), 3);
        assert_eq!(assembled.images_attached, 0);
        assert_eq!(assembled.compress, false);
        assert_eq!(assembled.messages[0]["content"], "System.");
        assert_eq!(assembled.messages[1]["content"], "W.");
        assert_eq!(assembled.messages[2]["role"], "assistant");
        assert_eq!(assembled.messages[2]["reasoning_content"], "Thinking about the problem...");
    }

    #[test]
    fn assembler_history_tool_row() {
        let input = KernelHistoryInput {
            system_prompt: "S.",
            memo_replay: None,
            context_hint: None,
            workflow_directive: "W.",
            ledger_hint: None,
            compression_summary: None,
            confirmed_plan: None,
            history_rows: vec![
                HistoryRow {
                    role: "tool".to_string(),
                    content: "read_file\nFile contents here.".to_string(),
                    references_json: None,
                    reasoning: None,
                },
            ],
            tool_results: vec![],
            user_injections: vec![],
            continuation_text: "",
            continuation_reasoning_only: false,
            correction_text: "",
            correction_hint: "",
            inject_progress_check: false,
            images: None,
            images_attached: 0,
            protocol: "",
            supports_image: false,
            context_budget: 128_000,
            history_limit: 40,
        };
        let assembled = KernelHistoryAssembler::assemble(&input);
        assert_eq!(assembled.messages.len(), 3); // system + workflow + tool result as user
        assert_eq!(assembled.images_attached, 0);
        assert_eq!(assembled.compress, false);
        assert_eq!(assembled.messages[0]["content"], "S.");
        assert_eq!(assembled.messages[1]["content"], "W.");
        assert_eq!(assembled.messages[2]["role"], "user");
        assert!(assembled.messages[2]["content"].as_str().unwrap().contains("[工具执行结果 - read_file]"));
    }

    #[test]
    fn assembler_bounds_historical_tool_output() {
        let long_output = format!("{}TAIL_MUST_NOT_LEAK", "x".repeat(1400));
        let input = KernelHistoryInput {
            system_prompt: "S.",
            memo_replay: None,
            context_hint: None,
            workflow_directive: "W.",
            ledger_hint: None,
            compression_summary: None,
            confirmed_plan: None,
            history_rows: vec![HistoryRow {
                role: "tool".to_string(),
                content: format!("read_file\n{long_output}"),
                references_json: None,
                reasoning: None,
            }],
            tool_results: vec![],
            user_injections: vec![],
            continuation_text: "",
            continuation_reasoning_only: false,
            correction_text: "",
            correction_hint: "",
            inject_progress_check: false,
            images: None,
            images_attached: 0,
            protocol: "",
            supports_image: false,
            context_budget: 128_000,
            history_limit: 40,
        };

        let assembled = KernelHistoryAssembler::assemble(&input);
        let content = assembled.messages[2]["content"].as_str().unwrap();
        assert!(content.contains("历史工具输出已截断"));
        assert!(!content.contains("TAIL_MUST_NOT_LEAK"));
        assert!(content.chars().count() < long_output.chars().count());
    }

    #[test]
    fn assembler_pending_action_phrase_filtered() {
        let input = KernelHistoryInput {
            system_prompt: "S.",
            memo_replay: None,
            context_hint: None,
            workflow_directive: "W.",
            ledger_hint: None,
            compression_summary: None,
            confirmed_plan: None,
            history_rows: vec![
                HistoryRow {
                    role: "assistant".to_string(),
                    content: "接下来验证构建".to_string(),
                    references_json: None,
                    reasoning: None,
                },
            ],
            tool_results: vec![],
            user_injections: vec![],
            continuation_text: "",
            continuation_reasoning_only: false,
            correction_text: "",
            correction_hint: "",
            inject_progress_check: false,
            images: None,
            images_attached: 0,
            protocol: "",
            supports_image: false,
            context_budget: 128_000,
            history_limit: 40,
        };
        let assembled = KernelHistoryAssembler::assemble(&input);
        // Short pending action phrase (< 300 chars) should be replaced with placeholder
        assert_eq!(assembled.messages.len(), 3);
        assert_eq!(assembled.images_attached, 0);
        assert_eq!(assembled.compress, false);
        assert_eq!(assembled.messages[0]["content"], "S.");
        assert_eq!(assembled.messages[1]["content"], "W.");
        assert_eq!(assembled.messages[2]["role"], "user");
        assert!(assembled.messages[2]["content"].as_str().unwrap().contains("未执行的过渡回复"));
    }

    #[test]
    fn assembler_tool_results() {
        let input = KernelHistoryInput {
            system_prompt: "S.",
            memo_replay: None,
            context_hint: None,
            workflow_directive: "W.",
            ledger_hint: None,
            compression_summary: None,
            confirmed_plan: None,
            history_rows: vec![],
            tool_results: vec![
                ToolResult {
                    tool: "write_file".to_string(),
                    output: "File written successfully.".to_string(),
                },
            ],
            user_injections: vec![],
            continuation_text: "",
            continuation_reasoning_only: false,
            correction_text: "",
            correction_hint: "",
            inject_progress_check: false,
            images: None,
            images_attached: 0,
            protocol: "",
            supports_image: false,
            context_budget: 128_000,
            history_limit: 40,
        };
        let assembled = KernelHistoryAssembler::assemble(&input);
        assert_eq!(assembled.messages.len(), 3); // system + workflow + tool result
        assert_eq!(assembled.images_attached, 0);
        assert_eq!(assembled.compress, false);
        assert_eq!(assembled.messages[0]["content"], "S.");
        assert_eq!(assembled.messages[1]["content"], "W.");
        assert!(assembled.messages[2]["content"].as_str().unwrap().contains("[工具执行结果 - write_file]"));
        assert!(assembled.messages[2]["content"].as_str().unwrap().contains("请根据以上结果继续"));
    }

    #[test]
    fn assembler_user_injections() {
        let input = KernelHistoryInput {
            system_prompt: "S.",
            memo_replay: None,
            context_hint: None,
            workflow_directive: "W.",
            ledger_hint: None,
            compression_summary: None,
            confirmed_plan: None,
            history_rows: vec![],
            tool_results: vec![],
            user_injections: vec![
                UserInjection {
                    content: "User instruction from session.".to_string(),
                },
            ],
            continuation_text: "",
            continuation_reasoning_only: false,
            correction_text: "",
            correction_hint: "",
            inject_progress_check: false,
            images: None,
            images_attached: 0,
            protocol: "",
            supports_image: false,
            context_budget: 128_000,
            history_limit: 40,
        };
        let assembled = KernelHistoryAssembler::assemble(&input);
        assert_eq!(assembled.messages.len(), 3); // system + workflow + user injection
        assert_eq!(assembled.images_attached, 0);
        assert_eq!(assembled.compress, false);
        assert_eq!(assembled.messages[0]["content"], "S.");
        assert_eq!(assembled.messages[1]["content"], "W.");
        assert_eq!(assembled.messages[2]["role"], "user");
        assert_eq!(assembled.messages[2]["content"], "User instruction from session.");
    }

    #[test]
    fn assembler_progress_check_injected() {
        let input = KernelHistoryInput {
            system_prompt: "S.",
            memo_replay: None,
            context_hint: None,
            workflow_directive: "W.",
            ledger_hint: None,
            compression_summary: None,
            confirmed_plan: Some("Plan steps."),
            history_rows: vec![],
            tool_results: vec![],
            user_injections: vec![],
            continuation_text: "",
            continuation_reasoning_only: false,
            correction_text: "",
            correction_hint: "",
            inject_progress_check: true,
            images: None,
            images_attached: 0,
            protocol: "",
            supports_image: false,
            context_budget: 128_000,
            history_limit: 40,
        };
        let assembled = KernelHistoryAssembler::assemble(&input);
        // system + workflow + plan + progress check
        assert_eq!(assembled.messages.len(), 4);
        assert_eq!(assembled.images_attached, 0);
        assert_eq!(assembled.compress, false);
        assert_eq!(assembled.messages[0]["content"], "S.");
        assert_eq!(assembled.messages[1]["content"], "W.");
        assert!(assembled.messages[2]["content"].as_str().unwrap().contains("已批准任务计划"));
        assert!(assembled.messages[3]["content"].as_str().unwrap().contains("执行对照"));
    }

    #[test]
    fn assembler_continuation_normal() {
        let input = KernelHistoryInput {
            system_prompt: "S.",
            memo_replay: None,
            context_hint: None,
            workflow_directive: "W.",
            ledger_hint: None,
            compression_summary: None,
            confirmed_plan: None,
            history_rows: vec![],
            tool_results: vec![],
            user_injections: vec![],
            continuation_text: "Partial output from previous turn.",
            continuation_reasoning_only: false,
            correction_text: "",
            correction_hint: "",
            inject_progress_check: false,
            images: None,
            images_attached: 0,
            protocol: "",
            supports_image: false,
            context_budget: 128_000,
            history_limit: 40,
        };
        let assembled = KernelHistoryAssembler::assemble(&input);
        assert_eq!(assembled.messages.len(), 4); // system + workflow + assistant + user (continuation)
        assert_eq!(assembled.images_attached, 0);
        assert_eq!(assembled.compress, false);
        assert_eq!(assembled.messages[0]["content"], "S.");
        assert_eq!(assembled.messages[1]["content"], "W.");
        assert_eq!(assembled.messages[2]["role"], "assistant");
        assert_eq!(assembled.messages[2]["content"], "Partial output from previous turn.");
        assert!(assembled.messages[3]["content"].as_str().unwrap().contains("未完整送达"));
    }

    #[test]
    fn assembler_continuation_reasoning_only() {
        let input = KernelHistoryInput {
            system_prompt: "S.",
            memo_replay: None,
            context_hint: None,
            workflow_directive: "W.",
            ledger_hint: None,
            compression_summary: None,
            confirmed_plan: None,
            history_rows: vec![],
            tool_results: vec![],
            user_injections: vec![],
            continuation_text: "",
            continuation_reasoning_only: true,
            correction_text: "",
            correction_hint: "",
            inject_progress_check: false,
            images: None,
            images_attached: 0,
            protocol: "",
            supports_image: false,
            context_budget: 128_000,
            history_limit: 40,
        };
        let assembled = KernelHistoryAssembler::assemble(&input);
        // reasoning-only 截断没有正文断点，但仍必须注入“直接给结论”指令。
        assert_eq!(assembled.messages.len(), 3); // system + workflow + continuation instruction
        assert_eq!(assembled.images_attached, 0);
        assert_eq!(assembled.compress, false);
        assert_eq!(assembled.messages[0]["content"], "S.");
        assert_eq!(assembled.messages[1]["content"], "W.");
        assert_eq!(assembled.messages[2]["role"], "user");
        assert_eq!(
            assembled.messages[2]["content"],
            continuation_instruction(true)
        );
    }

    #[test]
    fn assembler_correction_injection() {
        let input = KernelHistoryInput {
            system_prompt: "S.",
            memo_replay: None,
            context_hint: None,
            workflow_directive: "W.",
            ledger_hint: None,
            compression_summary: None,
            confirmed_plan: None,
            history_rows: vec![],
            tool_results: vec![],
            user_injections: vec![],
            continuation_text: "",
            continuation_reasoning_only: false,
            correction_text: "Assistant said '已调用工具' without actual call.",
            correction_hint: "（检测到你的回复中出现了...",
            inject_progress_check: false,
            images: None,
            images_attached: 0,
            protocol: "",
            supports_image: false,
            context_budget: 128_000,
            history_limit: 40,
        };
        let assembled = KernelHistoryAssembler::assemble(&input);
        assert_eq!(assembled.messages.len(), 4); // system + workflow + assistant + user (correction)
        assert_eq!(assembled.images_attached, 0);
        assert_eq!(assembled.compress, false);
        assert_eq!(assembled.messages[0]["content"], "S.");
        assert_eq!(assembled.messages[1]["content"], "W.");
        assert_eq!(assembled.messages[2]["role"], "assistant");
        assert_eq!(assembled.messages[2]["content"], "Assistant said '已调用工具' without actual call.");
        assert_eq!(assembled.messages[3]["role"], "user");
        assert!(assembled.messages[3]["content"].as_str().unwrap().contains("检测到"));
    }

    #[test]
    fn assembler_compression_summary() {
        let input = KernelHistoryInput {
            system_prompt: "S.",
            memo_replay: None,
            context_hint: None,
            workflow_directive: "W.",
            ledger_hint: None,
            compression_summary: Some("Early conversation was about X and Y."),
            confirmed_plan: None,
            history_rows: vec![],
            tool_results: vec![],
            user_injections: vec![],
            continuation_text: "",
            continuation_reasoning_only: false,
            correction_text: "",
            correction_hint: "",
            inject_progress_check: false,
            images: None,
            images_attached: 0,
            protocol: "",
            supports_image: false,
            context_budget: 128_000,
            history_limit: 40,
        };
        let assembled = KernelHistoryAssembler::assemble(&input);
        assert_eq!(assembled.messages.len(), 3); // system + workflow + compression summary
        assert_eq!(assembled.images_attached, 0);
        assert_eq!(assembled.compress, false);
        assert_eq!(assembled.messages[0]["content"], "S.");
        assert_eq!(assembled.messages[1]["content"], "W.");
        assert!(assembled.messages[2]["content"].as_str().unwrap().contains("历史摘要"));
    }

    #[test]
    fn assembler_full_sequence_order() {
        let input = KernelHistoryInput {
            system_prompt: "System prompt.",
            memo_replay: Some("Memo."),
            context_hint: Some("Context."),
            workflow_directive: "Workflow.",
            ledger_hint: Some("Ledger."),
            compression_summary: Some("Summary."),
            confirmed_plan: Some("Plan."),
            history_rows: vec![
                HistoryRow {
                    role: "user".to_string(),
                    content: "User message.".to_string(),
                    references_json: None,
                    reasoning: None,
                },
            ],
            tool_results: vec![
                ToolResult {
                    tool: "tool1".to_string(),
                    output: "Output.".to_string(),
                },
            ],
            user_injections: vec![
                UserInjection {
                    content: "Injection.".to_string(),
                },
            ],
            continuation_text: "Continuation.",
            continuation_reasoning_only: false,
            correction_text: "Correction text.",
            correction_hint: "Correction hint.",
            inject_progress_check: true,
            images: None,
            images_attached: 0,
            protocol: "",
            supports_image: false,
            context_budget: 128_000,
            history_limit: 40,
        };
        let assembled = KernelHistoryAssembler::assemble(&input);
        // Expected order:
        // 0: system
        // 1: memo
        // 2: context
        // 3: workflow
        // 4: ledger
        // 5: compression summary
        // 6: confirmed plan
        // 7: history row (user)
        // 8: tool result
        // 9: user injection
        // 10: progress check
        // 11: continuation assistant
        // 12: continuation user
        // 13: correction assistant
        // 14: correction user
        assert_eq!(assembled.messages.len(), 15);
        assert_eq!(assembled.images_attached, 0);
        assert_eq!(assembled.compress, false);
        assert_eq!(assembled.messages[0]["content"], "System prompt.");
        assert_eq!(assembled.messages[1]["content"], "Memo.");
        assert_eq!(assembled.messages[2]["content"], "Context.");
        assert_eq!(assembled.messages[3]["content"], "Workflow.");
        assert!(assembled.messages[4]["content"].as_str().unwrap().contains("Ledger."));
        assert!(assembled.messages[5]["content"].as_str().unwrap().contains("历史摘要"));
        assert!(assembled.messages[6]["content"].as_str().unwrap().contains("已批准任务计划"));
        assert_eq!(assembled.messages[7]["content"], "User message.");
        assert!(assembled.messages[8]["content"].as_str().unwrap().contains("[工具执行结果 - tool1]"));
        assert_eq!(assembled.messages[9]["content"], "Injection.");
        assert!(assembled.messages[10]["content"].as_str().unwrap().contains("执行对照"));
        assert_eq!(assembled.messages[11]["content"], "Continuation.");
        assert!(assembled.messages[12]["content"].as_str().unwrap().contains("未完整送达"));
        assert_eq!(assembled.messages[13]["content"], "Correction text.");
        assert_eq!(assembled.messages[14]["content"], "Correction hint.");
    }
}
