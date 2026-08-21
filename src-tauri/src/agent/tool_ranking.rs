//! 基于历史质量、预计成本、耗时、副作用与当前工程环境的工具排序。

use std::collections::HashMap;
use std::path::Path;

use rusqlite::Connection;
use serde::Serialize;

use super::tools::capabilities::TaskPhase;
use super::tools::contracts::EffectKind;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ToolRank {
    pub tool: String,
    pub score: f64,
    pub calls: i64,
    pub success_rate: Option<f64>,
    pub expected_duration_ms: Option<f64>,
    pub expected_output_tokens: Option<f64>,
    pub effect: String,
    pub environment_fit: bool,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct Environment {
    harmony_project: bool,
    git_repository: bool,
    device_available: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct History {
    calls: i64,
    successes: i64,
    avg_duration_ms: f64,
    avg_output_bytes: f64,
}

const HARMONY_TOOLS: &[&str] = &[
    "check_sdk_alignment", "diagnose_signing", "ohpm_install", "build_project",
    "build_hap", "deploy", "deploy_all", "start_ability", "search_sdk_api",
    "read_sdk_api_module", "search_harmony_docs", "read_harmony_doc",
];
const GIT_TOOLS: &[&str] = &[
    "git_status", "git_diff", "git_log", "git_commit", "git_push", "git_pull",
    "git_merge", "git_rebase", "git_restore", "git_stash", "review_changes",
];
const DEVICE_TOOLS: &[&str] = &[
    "list_devices", "connect_device", "deploy", "deploy_all", "start_ability",
    "take_screenshot", "read_logcat", "search_hilog", "dump_ui_hierarchy",
    "get_app_info", "dump_memory", "dump_battery", "device_perf", "analyze_crash",
];

pub fn rank_tools(
    conn: &Connection,
    conversation_id: &str,
    candidates: &[&str],
    phase: TaskPhase,
) -> Vec<ToolRank> {
    let environment = detect_environment(conn, conversation_id);
    let history = load_history(conn, candidates);
    let mut ranked = candidates.iter().enumerate().map(|(index, tool)| {
        let contract = super::tools::contracts::contract(tool);
        let past = history.get(*tool).copied().unwrap_or_default();
        let success_rate = (past.calls > 0).then_some(past.successes as f64 / past.calls as f64);
        let expected_duration = (past.calls > 0).then_some(past.avg_duration_ms);
        let expected_tokens = (past.calls > 0).then_some(past.avg_output_bytes / 4.0);
        let environment_fit = environment_fit(tool, &environment);

        // 能力包原始顺序仍是先验；历史数据只在有证据时调整，避免冷启动抖动。
        let mut score = 100.0 - index as f64 * 1.5;
        let mut reasons = Vec::new();
        if let Some(rate) = success_rate {
            score += (rate - 0.5) * 40.0;
            reasons.push(format!("历史成功率 {:.0}%（{} 次）", rate * 100.0, past.calls));
        } else {
            reasons.push("暂无历史样本，保留能力包先验".into());
        }
        if let Some(duration) = expected_duration {
            score -= (1.0 + duration / 1_000.0).ln().mul_add(3.0, 0.0).min(18.0);
            reasons.push(format!("预计耗时 {:.1}s", duration / 1_000.0));
        }
        if let Some(tokens) = expected_tokens {
            score -= (tokens / 1_000.0).min(10.0);
            reasons.push(format!("预计结果成本约 {:.0} tokens", tokens));
        }
        score -= effect_penalty(contract.effect, phase);
        reasons.push(format!("副作用等级 {}", contract.effect.as_str()));
        if environment_fit {
            score += 4.0;
            reasons.push("当前环境满足工具前提".into());
        } else {
            score -= 30.0;
            reasons.push("当前环境尚未发现所需工程、Git 或设备前提".into());
        }
        ToolRank {
            tool: (*tool).to_string(), score, calls: past.calls, success_rate,
            expected_duration_ms: expected_duration,
            expected_output_tokens: expected_tokens,
            effect: contract.effect.as_str().into(), environment_fit, reasons,
        }
    }).collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.tool.cmp(&b.tool)));
    ranked
}

fn load_history(conn: &Connection, candidates: &[&str]) -> HashMap<String, History> {
    let candidate_set = candidates.iter().copied().collect::<std::collections::HashSet<_>>();
    let since = chrono::Utc::now().timestamp() - 90 * 24 * 60 * 60;
    let Ok(mut stmt) = conn.prepare(
        "SELECT tool_name,COUNT(*),SUM(CASE WHEN status='ok' THEN 1 ELSE 0 END),
                AVG(COALESCE(duration_ms,0)),AVG(LENGTH(COALESCE(result_json,'')))
         FROM tool_runs
         WHERE created_at>=?1 AND status IN ('ok','error','blocked','cancelled')
         GROUP BY tool_name",
    ) else { return HashMap::new() };
    let Ok(rows) = stmt.query_map([since], |row| {
        Ok((row.get::<_, String>(0)?, History {
            calls: row.get(1)?, successes: row.get(2)?, avg_duration_ms: row.get(3)?,
            avg_output_bytes: row.get(4)?,
        }))
    }) else { return HashMap::new() };
    rows.filter_map(Result::ok)
        .filter(|(tool, _)| candidate_set.contains(tool.as_str()))
        .collect()
}

fn detect_environment(conn: &Connection, conversation_id: &str) -> Environment {
    let project = conn.query_row(
        "SELECT COALESCE(NULLIF(c.worktree_path,''),p.path),p.kind
         FROM conversations c JOIN projects p ON p.id=c.project_id WHERE c.id=?1",
        [conversation_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    ).ok();
    let (path, kind) = project.unwrap_or_default();
    let root = Path::new(&path);
    let kind = kind.to_ascii_lowercase();
    let harmony_project = kind.contains("harmony") || kind.contains("openharmony")
        || root.join("build-profile.json5").is_file()
        || root.join("oh-package.json5").is_file();
    let git_repository = root.join(".git").exists();
    let recent = chrono::Utc::now().timestamp() - 15 * 60;
    let device_available = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM tool_runs
         WHERE conversation_id=?1 AND tool_name IN ('list_devices','connect_device','deploy','start_ability')
           AND status='ok' AND created_at>=?2)",
        rusqlite::params![conversation_id, recent],
        |row| row.get::<_, bool>(0),
    ).unwrap_or(false);
    Environment { harmony_project, git_repository, device_available }
}

fn environment_fit(tool: &str, environment: &Environment) -> bool {
    (!HARMONY_TOOLS.contains(&tool) || environment.harmony_project)
        && (!GIT_TOOLS.contains(&tool) || environment.git_repository)
        && (!DEVICE_TOOLS.contains(&tool) || tool == "list_devices" || environment.device_available)
}

fn effect_penalty(effect: EffectKind, phase: TaskPhase) -> f64 {
    match (effect, phase) {
        (EffectKind::Read, _) => 0.0,
        (EffectKind::Write, TaskPhase::Modify) => 1.0,
        (EffectKind::Write, _) => 7.0,
        (EffectKind::Destructive, TaskPhase::Deliver) => 3.0,
        (EffectKind::Destructive, _) => 15.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE projects(id TEXT,path TEXT,kind TEXT);
             CREATE TABLE conversations(id TEXT,project_id TEXT,worktree_path TEXT);
             CREATE TABLE tool_runs(tool_name TEXT,status TEXT,duration_ms INTEGER,result_json TEXT,created_at INTEGER,conversation_id TEXT);
             INSERT INTO projects VALUES('p','','harmony');
             INSERT INTO conversations VALUES('c','p','');",
        ).unwrap();
        conn
    }

    #[test]
    fn ranking_uses_quality_latency_cost_effect_and_environment() {
        let conn = database();
        let now = chrono::Utc::now().timestamp();
        for _ in 0..8 {
            conn.execute("INSERT INTO tool_runs VALUES('read_file','ok',20,'ok',?1,'c')", [now]).unwrap();
            conn.execute("INSERT INTO tool_runs VALUES('run_command','error',90000,?1,?2,'c')", rusqlite::params!["x".repeat(20_000), now]).unwrap();
        }
        let ranked = rank_tools(&conn, "c", &["run_command", "read_file"], TaskPhase::Explore);
        assert_eq!(ranked[0].tool, "read_file");
        assert_eq!(ranked[0].success_rate, Some(1.0));
        assert!(ranked[1].expected_output_tokens.unwrap() >= 5_000.0);
        assert_eq!(ranked[1].effect, "destructive");
    }

    #[test]
    fn ranking_penalizes_missing_device_but_keeps_discovery_available() {
        let conn = database();
        let ranked = rank_tools(&conn, "c", &["take_screenshot", "list_devices"], TaskPhase::Explore);
        assert_eq!(ranked[0].tool, "list_devices");
        assert!(!ranked.iter().find(|rank| rank.tool == "take_screenshot").unwrap().environment_fit);
    }
}
