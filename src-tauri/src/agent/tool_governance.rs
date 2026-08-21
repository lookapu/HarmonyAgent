//! 工具目录治理：生成重复、长期未使用和高失败率的可操作清单。

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;
use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ToolGovernanceItem {
    pub tool: String,
    pub related_tool: Option<String>,
    pub issue: String,
    pub action: String,
    pub calls: i64,
    pub failures: i64,
    pub failure_rate: f64,
    pub evidence: String,
}

const OVERLAP_CANDIDATES: &[(&str, &str, &str)] = &[
    ("build_hap", "build_project", "均提供 HarmonyOS 构建入口，需确认模块/产品差异后合并或隐藏窄入口"),
    ("search_api", "search_sdk_api", "均搜索 API，需明确知识库与本地 SDK 语义边界"),
    ("read_document", "docx_read", "文档读取能力重叠，需统一格式路由和结果协议"),
    ("get_diagnostics", "lsp_diagnostics", "均返回诊断，需明确聚合诊断与语言服务诊断边界"),
];

pub fn report(conn: &Connection, since_seconds: i64) -> Result<Vec<ToolGovernanceItem>, String> {
    let mut stats = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT tool_name,COUNT(*),COALESCE(SUM(CASE WHEN status!='ok' THEN 1 ELSE 0 END),0)
         FROM tool_runs WHERE created_at>=?1 GROUP BY tool_name",
    ).map_err(|error| error.to_string())?;
    let rows = stmt.query_map([since_seconds], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
    }).map_err(|error| error.to_string())?;
    for row in rows {
        let (tool, calls, failures) = row.map_err(|error| error.to_string())?;
        stats.insert(tool, (calls, failures));
    }
    drop(stmt);

    let mut older_tools = HashSet::new();
    let mut older = conn.prepare(
        "SELECT DISTINCT tool_name FROM tool_runs WHERE created_at<?1",
    ).map_err(|error| error.to_string())?;
    let rows = older.query_map([since_seconds], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    for row in rows { older_tools.insert(row.map_err(|error| error.to_string())?); }
    drop(older);
    let mut items = Vec::new();
    for spec in crate::agent::tools::TOOL_SPECS {
        let (calls, failures) = stats.get(spec.name).copied().unwrap_or((0, 0));
        let rate = if calls == 0 { 0.0 } else { failures as f64 / calls as f64 };
        if calls >= 5 && rate >= 0.4 {
            items.push(ToolGovernanceItem {
                tool: spec.name.into(), related_tool: None, issue: "high_failure_rate".into(),
                action: "fix".into(), calls, failures, failure_rate: rate,
                evidence: format!("窗口内 {calls} 次调用，{failures} 次未成功"),
            });
        } else if older_tools.contains(spec.name) && calls == 0 {
            items.push(ToolGovernanceItem {
                tool: spec.name.into(), related_tool: None, issue: "long_unused".into(),
                action: "hide_candidate".into(), calls, failures, failure_rate: 0.0,
                evidence: "数据库已有窗口前历史，但该工具在当前窗口无调用".into(),
            });
        }
    }
    for (tool, related, evidence) in OVERLAP_CANDIDATES {
        if crate::agent::tools::TOOL_SPECS.iter().any(|spec| spec.name == *tool)
            && crate::agent::tools::TOOL_SPECS.iter().any(|spec| spec.name == *related)
        {
            let (calls, failures) = stats.get(*tool).copied().unwrap_or((0, 0));
            items.push(ToolGovernanceItem {
                tool: (*tool).into(), related_tool: Some((*related).into()),
                issue: "overlap_candidate".into(), action: "merge_review".into(),
                calls, failures,
                failure_rate: if calls == 0 { 0.0 } else { failures as f64 / calls as f64 },
                evidence: (*evidence).into(),
            });
        }
    }
    items.sort_by(|a, b| {
        let rank = |issue: &str| match issue { "high_failure_rate" => 0, "overlap_candidate" => 1, _ => 2 };
        rank(&a.issue).cmp(&rank(&b.issue))
            .then_with(|| b.failure_rate.total_cmp(&a.failure_rate))
            .then_with(|| a.tool.cmp(&b.tool))
    });
    items.truncate(100);
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_distinguishes_failures_inactivity_and_overlap() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tool_runs(tool_name TEXT,status TEXT,created_at INTEGER);
             INSERT INTO tool_runs VALUES
             ('read_file','ok',1),
             ('build_project','error',200),('build_project','error',201),
             ('build_project','error',202),('build_project','ok',203),('build_project','ok',204);",
        ).unwrap();
        let items = report(&conn, 100).unwrap();
        assert!(items.iter().any(|item| item.tool == "build_project" && item.action == "fix"));
        assert!(items.iter().any(|item| item.tool == "read_file" && item.action == "hide_candidate"));
        assert!(items.iter().any(|item| item.issue == "overlap_candidate" && item.related_tool.is_some()));
    }

    #[test]
    fn fresh_database_does_not_label_every_tool_unused() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE tool_runs(tool_name TEXT,status TEXT,created_at INTEGER);").unwrap();
        let items = report(&conn, 100).unwrap();
        assert!(!items.iter().any(|item| item.issue == "long_unused"));
    }
}
