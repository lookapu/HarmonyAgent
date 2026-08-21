//! 目标到验收的统一执行循环状态机。

use serde::{Deserialize, Serialize};

use super::acceptance::{AcceptanceReport, CriterionKind, GoalContract, ToolEvidence};
use super::tools::capabilities::TaskPhase;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LoopStage {
    Understand,
    Plan,
    Select,
    Execute,
    Verify,
    Accept,
}

impl LoopStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Understand => "understand",
            Self::Plan => "plan",
            Self::Select => "select",
            Self::Execute => "execute",
            Self::Verify => "verify",
            Self::Accept => "accept",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionLoopSnapshot {
    pub stage: LoopStage,
    pub recommended_phase: String,
    pub plan: Vec<String>,
    pub minimal_tools: Vec<String>,
    pub completed_evidence: usize,
    pub blockers: Vec<String>,
    pub acceptance: AcceptanceReport,
    pub verification_plan: super::verification_planner::VerificationPlan,
    pub pending_postconditions: Vec<super::postconditions::PendingPostcondition>,
}

pub fn snapshot(contract: &GoalContract, evidence: &[ToolEvidence<'_>]) -> ExecutionLoopSnapshot {
    let mut acceptance = super::acceptance::evaluate_contract(contract, evidence);
    let verification_plan = super::verification_planner::plan(evidence);
    let pending_verification = verification_plan.pending_required();
    if !pending_verification.is_empty() {
        acceptance.passed = false;
        acceptance.blockers.push(format!(
            "文件变更验证未闭环：{}",
            pending_verification.iter().map(|step| step.tool.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    let pending_postconditions = super::postconditions::pending(evidence);
    let successful = evidence.iter().filter(|item| item.succeeded).count();
    let has_plan = evidence.iter().any(|item| {
        item.succeeded && matches!(item.tool, "plan_task" | "todo_write")
    });
    let attempted_execution = evidence.iter().any(|item| {
        !matches!(item.tool,
            "plan_task" | "todo_write" | "todo_get" | "tool_help" | "tool_list"
                | "list_dir" | "get_project_info" | "list_modules" | "deep_scan"
                | "codebase_search" | "search_symbols" | "get_symbol_details" | "read_file")
    });
    let last_effect = evidence.iter().enumerate().filter(|(_, item)| {
        let contract = super::tools::contracts::contract(item.tool);
        item.succeeded
            && contract.effect != super::tools::contracts::EffectKind::Read
            && contract.validator.is_none()
            && !matches!(item.tool, "plan_task" | "todo_write" | "todo_get")
    }).map(|(index, _)| index).next_back();
    let last_verifier = evidence.iter().enumerate().filter(|(_, item)| {
        item.succeeded && super::tools::contracts::contract(item.tool).validator.is_some()
    }).map(|(index, _)| index).next_back();
    let needs_post_effect_verification = last_effect.is_some_and(|effect| {
        last_verifier.is_none_or(|verifier| verifier <= effect)
    });

    let stage = if acceptance.passed {
        LoopStage::Accept
    } else if evidence.is_empty() {
        LoopStage::Understand
    } else if !has_plan && !attempted_execution {
        LoopStage::Plan
    } else if has_plan && !attempted_execution {
        LoopStage::Select
    } else if needs_post_effect_verification || !pending_verification.is_empty() {
        LoopStage::Verify
    } else {
        LoopStage::Execute
    };
    let phase = recommended_phase(stage, contract, &acceptance);
    let minimal_tools = super::tools::capabilities::selected_tool_names_for_phase(
        &contract.original_goal,
        phase,
        16,
    ).into_iter().map(str::to_string).collect();
    let plan = if contract.criteria.is_empty() {
        vec!["形成有来源支撑的准确结论".into()]
    } else {
        contract.criteria.iter().filter(|item| item.required)
            .map(|item| format!("[{}] {}", item.id, item.label)).collect()
    };
    ExecutionLoopSnapshot {
        stage,
        recommended_phase: phase.as_str().into(),
        plan,
        minimal_tools,
        completed_evidence: successful,
        blockers: acceptance.blockers.clone(),
        acceptance,
        verification_plan,
        pending_postconditions,
    }
}

fn recommended_phase(
    stage: LoopStage,
    contract: &GoalContract,
    acceptance: &AcceptanceReport,
) -> TaskPhase {
    if stage == LoopStage::Verify {
        return TaskPhase::Verify;
    }
    let pending_delivery = contract.criteria.iter().any(|criterion| {
        matches!(criterion.kind, CriterionKind::GitCommit | CriterionKind::GitPush)
            && acceptance.criteria.iter().any(|result| result.id == criterion.id && !result.passed)
    });
    if pending_delivery && matches!(stage, LoopStage::Execute | LoopStage::Select) {
        return TaskPhase::Deliver;
    }
    match stage {
        LoopStage::Understand => TaskPhase::Explore,
        LoopStage::Plan | LoopStage::Select | LoopStage::Execute => TaskPhase::Modify,
        LoopStage::Verify => TaskPhase::Verify,
        LoopStage::Accept => TaskPhase::Deliver,
    }
}

impl ExecutionLoopSnapshot {
    pub fn directive(&self) -> String {
        let plan = self.plan.iter().map(|item| format!("- {item}"))
            .collect::<Vec<_>>().join("\n");
        let blockers = if self.blockers.is_empty() {
            "无；进入验收并绑定最终证据".to_string()
        } else {
            self.blockers.join("；")
        };
        let verification = self.verification_plan.directive()
            .map(|value| format!("\n{value}"))
            .unwrap_or_default();
        let postconditions = if self.pending_postconditions.is_empty() {
            String::new()
        } else {
            format!("\n## 副作用写后读确认\n{}\n写入返回值不能自证真实状态。",
                self.pending_postconditions.iter().map(|item| format!(
                    "- {}：{}；确认工具 {}",
                    item.tool, item.reason, item.verifiers.join(" / "),
                )).collect::<Vec<_>>().join("\n"))
        };
        format!(
            "## 统一执行循环\n当前阶段：{}（工具阶段 {}）\n可验证计划：\n{}\n本阶段最小工具集：{}\n当前证据数：{}\n未通过项：{}\n规则：按 理解目标 → 可验证计划 → 最小工具集 → 执行 → 独立验证 → 验收 推进；不得用写入成功代替验证，也不得在验收未通过时宣称完成。",
            self.stage.as_str(), self.recommended_phase, plan,
            self.minimal_tools.join(", "), self.completed_evidence, blockers,
        ) + &verification + &postconditions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence<'a>(tool: &'a str, ok: bool) -> ToolEvidence<'a> {
        ToolEvidence { tool, args: "{}", output: "ok", succeeded: ok }
    }

    #[test]
    fn mutation_task_moves_through_the_full_loop() {
        let contract = GoalContract::compile("修复问题并运行测试");
        assert_eq!(snapshot(&contract, &[]).stage, LoopStage::Understand);
        assert_eq!(snapshot(&contract, &[evidence("read_file", true)]).stage, LoopStage::Plan);
        assert_eq!(snapshot(&contract, &[
            evidence("read_file", true), evidence("plan_task", true),
        ]).stage, LoopStage::Select);
        assert_eq!(snapshot(&contract, &[
            evidence("read_file", true), evidence("plan_task", true), evidence("edit_file", false),
        ]).stage, LoopStage::Execute);
        assert_eq!(snapshot(&contract, &[
            evidence("read_file", true), evidence("plan_task", true), evidence("edit_file", true),
        ]).stage, LoopStage::Verify);
        assert_eq!(snapshot(&contract, &[
            evidence("read_file", true), evidence("plan_task", true), evidence("edit_file", true),
            evidence("run_tests", true),
        ]).stage, LoopStage::Accept);
    }

    #[test]
    fn delivery_requirement_selects_delivery_phase_only_after_verification() {
        let contract = GoalContract::compile("修改文件，测试后提交并推送");
        let items = [
            evidence("edit_file", true), evidence("run_tests", true), evidence("git_commit", false),
        ];
        let state = snapshot(&contract, &items);
        assert_eq!(state.stage, LoopStage::Execute);
        assert_eq!(state.recommended_phase, "deliver");
        assert!(state.minimal_tools.iter().any(|tool| tool == "git_commit"));
    }

    #[test]
    fn arkts_write_cannot_accept_until_sdk_lsp_and_build_are_clean() {
        let contract = GoalContract::compile("修改 ArkTS 文件并验证");
        let write = ToolEvidence {
            tool: "edit_file",
            args: r#"{"path":"entry/src/main/ets/Index.ets"}"#,
            output: "ok",
            succeeded: true,
        };
        let state = snapshot(&contract, &[write, evidence("build_project", true)]);
        assert_eq!(state.stage, LoopStage::Verify);
        assert!(state.blockers.iter().any(|blocker| blocker.contains("lsp_diagnostics")));
    }
}
