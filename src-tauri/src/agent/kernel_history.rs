//! AgentKernel 共享消息历史组装策略（Phase A：仅搬入纯函数，assembler 在 Phase E 接入）。
//!
//! 纯策略、无 IO、无 Tauri 依赖：UI（chat.rs）与 headless（headless_driver.rs）共用。
//! Phase A 先搬入两个纯函数（`dynamic_history_limit`、`estimate_tokens`），chat.rs 改 import；
//! 完整 `KernelHistoryAssembler` 在 Phase E 落盘并接入 chat.rs 主循环。

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
}
