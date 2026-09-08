//! UI/headless 共用 Agent Kernel 的协议与预算基础。
//!
//! 本模块不依赖 Tauri、Provider 凭据或具体工具 runtime。协议 adapter 先把响应转换为
//! `KernelTurn`，预算账本再统一累计 usage/cost；消息循环与 UI 流式 adapter 后续逐步接入。

use serde_json::Value;

use crate::agent::acceptance::{
    evaluate_contract, remediation_prompt, AcceptanceReport, GoalContract, ToolEvidence,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KernelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernelToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug)]
pub struct KernelTurn {
    pub provider_message: Value,
    pub content: String,
    pub tool_calls: Vec<KernelToolCall>,
    pub usage: Option<KernelUsage>,
    pub finish_reason: Option<String>,
}

impl KernelTurn {
    pub fn was_truncated(&self) -> bool {
        self.finish_reason.as_deref() == Some("length")
    }

    pub fn is_stop_candidate(&self) -> bool {
        self.tool_calls.is_empty()
    }
}

/// OpenAI-compatible 非流式响应转为协议无关回合。协议损坏必须失败关闭，不能把缺字段
/// 当作模型正常停止，否则 grader 会收到一个看似有效但实际未运行的 trial。
pub fn parse_openai_turn(response: &Value) -> Result<KernelTurn, String> {
    let choice = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| "Provider 响应缺少 choices[0]".to_string())?;
    let message = choice
        .get("message")
        .filter(|message| message.is_object())
        .cloned()
        .ok_or_else(|| "Provider 响应缺少 message object".to_string())?;
    let content = match message.get("content") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(content)) => content.clone(),
        Some(_) => return Err("Provider message.content 必须是 string 或 null".into()),
    };
    let calls = match message.get("tool_calls") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(calls)) => calls.as_slice(),
        Some(_) => return Err("Provider message.tool_calls 必须是 array".into()),
    };
    let mut tool_calls = Vec::with_capacity(calls.len());
    for (index, call) in calls.iter().enumerate() {
        let id = call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| format!("Provider tool_calls[{index}] 缺少非空 id"))?;
        let function = call
            .get("function")
            .filter(|function| function.is_object())
            .ok_or_else(|| format!("Provider tool_calls[{index}] 缺少 function object"))?;
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| format!("Provider tool_calls[{index}] 缺少非空 function.name"))?;
        let arguments = function
            .get("arguments")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                format!("Provider tool_calls[{index}] 缺少 string function.arguments")
            })?;
        tool_calls.push(KernelToolCall {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        });
    }
    let usage = match response.get("usage") {
        None | Some(Value::Null) => None,
        Some(usage) if usage.is_object() => {
            let input_tokens = usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .ok_or_else(|| "Provider usage.prompt_tokens 必须是非负整数".to_string())?;
            let output_tokens = usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .ok_or_else(|| "Provider usage.completion_tokens 必须是非负整数".to_string())?;
            let cached_tokens = match usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
            {
                None | Some(Value::Null) => 0,
                Some(value) => value.as_u64().ok_or_else(|| {
                    "Provider usage.prompt_tokens_details.cached_tokens 必须是非负整数".to_string()
                })?,
            };
            Some(KernelUsage {
                input_tokens,
                output_tokens,
                cached_tokens,
            })
        }
        Some(_) => return Err("Provider usage 必须是 object 或 null".into()),
    };
    Ok(KernelTurn {
        provider_message: message,
        content,
        tool_calls,
        usage,
        finish_reason: choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

#[derive(Clone, Debug)]
pub struct KernelUsageLedger {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub cost_cny: f64,
    max_cost_cny: f64,
    input_price_cny_per_1k: Option<f64>,
    output_price_cny_per_1k: Option<f64>,
}

impl KernelUsageLedger {
    pub fn new(
        max_cost_cny: f64,
        input_price_cny_per_1k: Option<f64>,
        output_price_cny_per_1k: Option<f64>,
    ) -> Result<Self, String> {
        if !max_cost_cny.is_finite() || max_cost_cny < 0.0 {
            return Err("max_cost_cny 必须是有限非负数字".into());
        }
        for (name, price) in [
            ("input_price_cny_per_1k", input_price_cny_per_1k),
            ("output_price_cny_per_1k", output_price_cny_per_1k),
        ] {
            if price.is_some_and(|price| !price.is_finite() || price < 0.0) {
                return Err(format!("{name} 必须是有限非负数字"));
            }
        }
        if max_cost_cny > 0.0
            && (input_price_cny_per_1k.is_none() || output_price_cny_per_1k.is_none())
        {
            return Err("任务设置了 max_cost_cny，但缺少输入/输出价格快照".into());
        }
        Ok(Self {
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            cost_cny: 0.0,
            max_cost_cny,
            input_price_cny_per_1k,
            output_price_cny_per_1k,
        })
    }

    /// 返回是否已超过成本上限。设有成本上限时 usage 缺失必须失败关闭。
    pub fn record(&mut self, usage: Option<&KernelUsage>) -> Result<bool, String> {
        let Some(usage) = usage else {
            if self.max_cost_cny > 0.0 {
                return Err("Provider 未返回 usage，无法执行成本硬限制".into());
            }
            return Ok(false);
        };
        self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(usage.output_tokens);
        self.cached_tokens = self.cached_tokens.saturating_add(usage.cached_tokens);
        if let (Some(input_price), Some(output_price)) =
            (self.input_price_cny_per_1k, self.output_price_cny_per_1k)
        {
            self.cost_cny += usage.input_tokens as f64 / 1000.0 * input_price
                + usage.output_tokens as f64 / 1000.0 * output_price;
        }
        Ok(self.max_cost_cny > 0.0 && self.cost_cny > self.max_cost_cny)
    }
}

/// 统一内核持有的工具证据。使用 owned 字段，既能跨异步回合保存，也不会把 UI/headless
/// 的具体 ToolRun 类型泄漏到验收模块。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernelToolEvidence {
    pub tool: String,
    pub arguments: String,
    pub output: String,
    pub succeeded: bool,
}

#[derive(Clone, Debug)]
pub enum KernelStopDecision {
    Accepted(AcceptanceReport),
    Remediate {
        report: AcceptanceReport,
        prompt: String,
        round: usize,
    },
    Exhausted(AcceptanceReport),
}

/// 对一次模型停止申请作统一裁决。调用方可以传入普通目标验收报告，也可以传入 UI
/// 聚合子任务 DAG 后的报告；内核只负责一致的有界补救语义。
pub fn decide_stop_candidate(
    report: AcceptanceReport,
    remediation_rounds: &mut usize,
    max_remediation_rounds: usize,
) -> KernelStopDecision {
    if report.passed {
        return KernelStopDecision::Accepted(report);
    }
    if *remediation_rounds >= max_remediation_rounds {
        return KernelStopDecision::Exhausted(report);
    }
    *remediation_rounds = remediation_rounds.saturating_add(1);
    KernelStopDecision::Remediate {
        prompt: remediation_prompt(&report),
        report,
        round: *remediation_rounds,
    }
}

/// 模型只能申请停止；是否真正停止由目标契约和真实工具证据裁决。
///
/// UI 与 headless 使用同一状态机后，benchmark 不会把“模型说完成了”误当成已完成，
/// 同时通过有界 remediation 次数避免弱模型无限自循环。
#[derive(Clone, Debug)]
pub struct KernelAcceptanceGate {
    contract: GoalContract,
    evidence: Vec<KernelToolEvidence>,
    remediation_rounds: usize,
    max_remediation_rounds: usize,
}

impl KernelAcceptanceGate {
    pub fn new(goal: &str, max_remediation_rounds: usize) -> Self {
        Self {
            contract: GoalContract::compile(goal),
            evidence: Vec::new(),
            remediation_rounds: 0,
            max_remediation_rounds,
        }
    }

    pub fn directive(&self) -> String {
        self.contract.directive()
    }

    pub fn record(&mut self, evidence: KernelToolEvidence) {
        self.evidence.push(evidence);
    }

    pub fn report(&self) -> AcceptanceReport {
        let evidence = self
            .evidence
            .iter()
            .map(|item| ToolEvidence {
                tool: &item.tool,
                args: &item.arguments,
                output: &item.output,
                succeeded: item.succeeded,
            })
            .collect::<Vec<_>>();
        evaluate_contract(&self.contract, &evidence)
    }

    pub fn request_stop(&mut self) -> KernelStopDecision {
        let report = self.report();
        decide_stop_candidate(
            report,
            &mut self.remediation_rounds,
            self.max_remediation_rounds,
        )
    }

    pub fn remediation_rounds(&self) -> usize {
        self.remediation_rounds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_turn_and_cached_usage() {
        let turn = parse_openai_turn(&serde_json::json!({
            "choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
              "tool_calls":[{"id":"c1","function":{"name":"read_file","arguments":"{\"path\":\"a\"}"}}]}}],
            "usage":{"prompt_tokens":12,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":5}}
        })).unwrap();
        assert_eq!(turn.tool_calls[0].name, "read_file");
        assert_eq!(turn.usage.as_ref().unwrap().cached_tokens, 5);
        assert!(!turn.is_stop_candidate());
    }

    #[test]
    fn rejects_malformed_provider_tool_calls() {
        let error = parse_openai_turn(&serde_json::json!({
            "choices":[{"message":{"tool_calls":[{"id":"","function":{"name":"read_file"}}]}}]
        }))
        .unwrap_err();
        assert!(error.contains("非空 id"));
    }

    #[test]
    fn rejects_usage_that_could_bypass_cost_accounting() {
        let error = parse_openai_turn(&serde_json::json!({
            "choices":[{"message":{"content":"done"}}],
            "usage":{"prompt_tokens":12}
        }))
        .unwrap_err();
        assert!(error.contains("completion_tokens"));

        let turn = parse_openai_turn(&serde_json::json!({
            "choices":[{"message":{"content":"done"}}],
            "usage":null
        }))
        .unwrap();
        assert!(turn.usage.is_none());
    }

    #[test]
    fn cost_limit_requires_prices_and_usage() {
        assert!(KernelUsageLedger::new(1.0, None, None).is_err());
        assert!(KernelUsageLedger::new(1.0, Some(-1.0), Some(20.0)).is_err());
        let mut ledger = KernelUsageLedger::new(1.0, Some(10.0), Some(20.0)).unwrap();
        assert!(ledger.record(None).unwrap_err().contains("未返回 usage"));
        assert!(ledger
            .record(Some(&KernelUsage {
                input_tokens: 100,
                output_tokens: 1,
                cached_tokens: 0,
            }))
            .unwrap());
    }

    #[test]
    fn acceptance_gate_requires_post_mutation_verification() {
        let mut gate = KernelAcceptanceGate::new("修改 src/a.rs 并验证", 2);
        gate.record(KernelToolEvidence {
            tool: "write_file".into(),
            arguments: r#"{"path":"src/a.rs","content":"fixed"}"#.into(),
            output: "written".into(),
            succeeded: true,
        });
        assert!(matches!(
            gate.request_stop(),
            KernelStopDecision::Remediate { round: 1, .. }
        ));
        gate.record(KernelToolEvidence {
            tool: "read_file".into(),
            arguments: r#"{"path":"src/a.rs"}"#.into(),
            output: "fixed".into(),
            succeeded: true,
        });
        assert!(matches!(
            gate.request_stop(),
            KernelStopDecision::Accepted(_)
        ));
    }

    #[test]
    fn acceptance_gate_exhaustion_is_bounded() {
        let mut gate = KernelAcceptanceGate::new("修改 a.rs", 1);
        assert!(matches!(
            gate.request_stop(),
            KernelStopDecision::Remediate { round: 1, .. }
        ));
        assert!(matches!(
            gate.request_stop(),
            KernelStopDecision::Exhausted(_)
        ));
        assert_eq!(gate.remediation_rounds(), 1);
    }
}
