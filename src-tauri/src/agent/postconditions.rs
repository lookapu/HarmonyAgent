//! 副作用工具的写后读确认矩阵。

use serde::{Deserialize, Serialize};

use super::acceptance::{CriterionKind, ToolEvidence};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingPostcondition {
    pub tool: String,
    pub verifiers: Vec<String>,
    pub reason: String,
}

fn requirement(tool: &str, args: &str) -> Option<(&'static [&'static str], &'static str)> {
    match tool {
        "deploy" | "deploy_all" | "install_app" | "install_launch" => Some((
            &["get_app_info", "verify_ui", "take_screenshot", "read_runtime_logs"],
            "部署后必须从设备读取安装、启动或界面状态",
        )),
        "start_ability" => Some((
            &["get_app_info", "verify_ui", "read_runtime_logs"],
            "启动 Ability 后必须读取应用或运行状态",
        )),
        "git_commit" => Some((&["git_status", "git_log"], "提交后必须读取 HEAD/工作树状态")),
        "git_push" => Some((&["git_status", "git_log"], "推送后必须读取分支跟踪状态")),
        "git_pull" | "git_merge" | "git_rebase" => Some((
            &["git_status", "git_log"], "Git 写入后必须读取分支与工作树状态",
        )),
        "db_migrate" => Some((&["db_query"], "迁移后必须查询数据库真实 schema/数据")),
        "secret_store" => Some((&["secret_get"], "保存密钥后必须通过受控读取确认存在")),
        "manage_memory" | "manage_knowledge" => Some((
            &["search_knowledge"], "外部知识写入后必须重新检索确认",
        )),
        "http_request" if is_http_write(args) => Some((
            &["http_request"], "HTTP 写入后必须用独立读取请求确认远端状态",
        )),
        _ => None,
    }
}

fn is_http_write(args: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(args).ok()
        .and_then(|value| value.get("method")?.as_str().map(str::to_ascii_uppercase))
        .is_some_and(|method| !matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS"))
}

pub fn pending(evidence: &[ToolEvidence<'_>]) -> Vec<PendingPostcondition> {
    let mut pending = Vec::new();
    for (index, item) in evidence.iter().enumerate() {
        if !item.succeeded { continue; }
        let Some((verifiers, reason)) = requirement(item.tool, item.args) else { continue };
        let confirmed = evidence[index + 1..].iter().any(|later| {
            later.succeeded && verifiers.contains(&later.tool)
                && !(item.tool == "http_request" && later.tool == "http_request" && is_http_write(later.args))
        });
        if !confirmed {
            pending.push(PendingPostcondition {
                tool: item.tool.into(),
                verifiers: verifiers.iter().map(|tool| (*tool).into()).collect(),
                reason: reason.into(),
            });
        }
    }
    pending
}

pub fn criterion_evidence_indices(
    kind: &CriterionKind,
    evidence: &[ToolEvidence<'_>],
) -> Option<(usize, usize)> {
    let mutator = match kind {
        CriterionKind::Deploy => &["deploy", "deploy_all", "install_app", "install_launch"][..],
        CriterionKind::GitCommit => &["git_commit"][..],
        CriterionKind::GitPush => &["git_push"][..],
        _ => return None,
    };
    let (index, item) = evidence.iter().enumerate().rev()
        .find(|(_, item)| item.succeeded && mutator.contains(&item.tool))?;
    let (verifiers, _) = requirement(item.tool, item.args)?;
    evidence[index + 1..].iter().position(|later| {
        later.succeeded && verifiers.contains(&later.tool)
    }).map(|offset| (index, index + 1 + offset))
}

pub fn directive(evidence: &[ToolEvidence<'_>]) -> Option<String> {
    let pending = pending(evidence);
    if pending.is_empty() { return None; }
    Some(format!(
        "## 副作用写后读确认\n{}\n写入工具自身的成功返回不能作为真实状态确认；必须执行其后的读取工具。",
        pending.iter().map(|item| format!(
            "- {}：{}；任选确认工具 {}",
            item.tool, item.reason, item.verifiers.join(" / "),
        )).collect::<Vec<_>>().join("\n"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ev<'a>(tool: &'a str, args: &'a str, ok: bool) -> ToolEvidence<'a> {
        ToolEvidence { tool, args, output: "ok", succeeded: ok }
    }

    #[test]
    fn deployment_requires_a_later_device_read() {
        let items = [ev("deploy", "{}", true)];
        assert_eq!(pending(&items)[0].tool, "deploy");
        let confirmed = [ev("deploy", "{}", true), ev("verify_ui", "{}", true)];
        assert!(pending(&confirmed).is_empty());
        assert_eq!(criterion_evidence_indices(&CriterionKind::Deploy, &confirmed), Some((0, 1)));
    }

    #[test]
    fn git_push_requires_a_later_status_read() {
        let items = [ev("git_push", "{}", true), ev("git_status", "{}", true)];
        assert!(pending(&items).is_empty());
        assert_eq!(criterion_evidence_indices(&CriterionKind::GitPush, &items), Some((0, 1)));
    }

    #[test]
    fn http_write_is_not_confirmed_by_another_write() {
        let post = r#"{"method":"POST"}"#;
        assert_eq!(pending(&[ev("http_request", post, true), ev("http_request", post, true)]).len(), 2);
        assert!(pending(&[ev("http_request", post, true), ev("http_request", r#"{"method":"GET"}"#, true)]).is_empty());
    }
}
