//! 真实 Agent 评测任务 schema v1 与安全校验。
//!
//! 任务文件是不可信输入：它不能携带宿主命令或凭据。本模块只负责“解析 + 拒绝不安全输入”，
//! 是 headless eval harness 执行状态机的 `validate input` 阶段；实际 runner 在此之后才接工作树与沙箱。
//! 契约见 docs/AGENT_EVAL_HARNESS.md。

use serde::Deserialize;

pub const EVAL_TASK_SCHEMA_VERSION: u32 = 1;
const MAX_PROBLEM_STATEMENT_BYTES: usize = 64 * 1024;
const MAX_WALL_TIME_SECONDS: u64 = 24 * 3600;
const MAX_STEPS: u64 = 10_000;
const MAX_GRADER_TIMEOUT_SECONDS: u64 = 24 * 3600;
const MAX_ARTIFACTS: usize = 64;

#[derive(Debug, Clone, Deserialize)]
pub struct EvalTask {
    pub schema_version: u32,
    pub task_id: String,
    pub suite: String,
    pub problem_statement: String,
    pub repo: EvalRepo,
    pub limits: EvalLimits,
    pub grader: EvalGrader,
    #[serde(default)]
    pub artifacts: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EvalRepo {
    pub url: String,
    pub base_commit: String,
    #[serde(default)]
    pub subdir: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EvalLimits {
    pub wall_time_seconds: u64,
    pub max_steps: u64,
    pub max_cost_cny: f64,
    pub network: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EvalGrader {
    pub kind: String,
    pub command: Vec<String>,
    pub timeout_seconds: u64,
}

/// 解析并校验任务 JSON；任何违反 schema 或安全边界都返回错误，绝不部分接受。
pub fn parse_eval_task(json: &str) -> Result<EvalTask, String> {
    let task: EvalTask =
        serde_json::from_str(json).map_err(|error| format!("任务 JSON 无法解析：{error}"))?;
    validate_eval_task(&task)?;
    Ok(task)
}

pub fn validate_eval_task(task: &EvalTask) -> Result<(), String> {
    if task.schema_version != EVAL_TASK_SCHEMA_VERSION {
        return Err(format!(
            "不支持的 task schema 版本 {}（仅支持 {EVAL_TASK_SCHEMA_VERSION}）",
            task.schema_version
        ));
    }
    if !is_safe_id(&task.task_id) {
        return Err("task_id 只能包含字母、数字、`.`、`-`、`_`".into());
    }
    if task.suite.trim().is_empty() || task.suite.len() > 128 {
        return Err("suite 不能为空且不得超过 128 字符".into());
    }
    if task.problem_statement.trim().is_empty()
        || task.problem_statement.len() > MAX_PROBLEM_STATEMENT_BYTES
    {
        return Err("problem_statement 不能为空且不得超过 64 KiB".into());
    }
    validate_repo(&task.repo)?;
    validate_limits(&task.limits)?;
    validate_grader(&task.grader)?;
    if task.artifacts.len() > MAX_ARTIFACTS {
        return Err(format!("artifacts 不得超过 {MAX_ARTIFACTS} 项"));
    }
    for artifact in &task.artifacts {
        if !is_safe_relative_glob(artifact) {
            return Err(format!("artifact 路径必须是工作树内的相对 glob，收到：{artifact}"));
        }
    }
    Ok(())
}

fn validate_repo(repo: &EvalRepo) -> Result<(), String> {
    if repo.url.trim().is_empty() || repo.url.len() > 2048 {
        return Err("repo.url 不能为空且不得超过 2048 字符".into());
    }
    let url = repo.url.trim();
    let allowed_scheme = url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("git@")
        || url.starts_with("file://");
    if !allowed_scheme || url.contains(char::is_whitespace) {
        return Err("repo.url 必须是 http(s)/git@/file 形式的无空白 URL".into());
    }
    let commit = repo.base_commit.trim();
    if !commit.chars().all(|c| c.is_ascii_hexdigit()) || !(7..=64).contains(&commit.len()) {
        return Err("repo.base_commit 必须是 7-64 位十六进制提交哈希".into());
    }
    if let Some(subdir) = repo.subdir.as_deref() {
        if subdir.starts_with('/') || subdir.split('/').any(|seg| seg == ".." || seg.is_empty()) {
            return Err("repo.subdir 必须是工作树内的相对路径".into());
        }
    }
    Ok(())
}

fn validate_limits(limits: &EvalLimits) -> Result<(), String> {
    if limits.wall_time_seconds == 0 || limits.wall_time_seconds > MAX_WALL_TIME_SECONDS {
        return Err(format!("limits.wall_time_seconds 必须在 1..={MAX_WALL_TIME_SECONDS} 之间"));
    }
    if limits.max_steps == 0 || limits.max_steps > MAX_STEPS {
        return Err(format!("limits.max_steps 必须在 1..={MAX_STEPS} 之间"));
    }
    if !limits.max_cost_cny.is_finite() || limits.max_cost_cny < 0.0 {
        return Err("limits.max_cost_cny 必须是非负有限数".into());
    }
    // v1 只支持断网运行；allowlist/full 需要 Host Capability Broker 之后才放开。
    if limits.network != "none" {
        return Err(format!(
            "limits.network 当前仅支持 \"none\"（收到 {}），避免不可信任务联网",
            limits.network
        ));
    }
    Ok(())
}

fn validate_grader(grader: &EvalGrader) -> Result<(), String> {
    if grader.kind != "command" {
        return Err(format!(
            "grader.kind 当前仅支持 \"command\"（收到 {}）",
            grader.kind
        ));
    }
    if grader.command.is_empty() || grader.command.len() > 64 {
        return Err("grader.command 必须是非空且不超过 64 个令牌的列表".into());
    }
    // 拒绝把 shell 解释器当作 grader 程序：`sh -c <脚本>` 会让任务文件里的任意字符串
    // 以宿主命令身份执行。grader 只接受直接程序 + 固定 argv（如 npm test）。
    if is_shell_interpreter(&grader.command[0]) {
        return Err(format!(
            "grader.command 不得以 shell 解释器开头（收到 {}）；请使用直接程序 + 固定参数",
            grader.command[0]
        ));
    }
    for token in &grader.command {
        // 拒绝 shell 元字符、绝对路径、上级路径与命令替换，防止把不可信任务里的字符串直接交给宿主 shell。
        let unsafe_char = |c: char| {
            matches!(
                c,
                ';' | '|' | '&' | '>' | '<' | '$' | '`' | '(' | ')' | '\'' | '"' | '\n' | '\r' | '\0'
            )
        };
        if token.starts_with('/')
            || token.contains("..")
            || token.contains(unsafe_char)
            || token.is_empty()
        {
            return Err(format!("grader.command 含不安全令牌：{token}"));
        }
    }
    if grader.timeout_seconds == 0 || grader.timeout_seconds > MAX_GRADER_TIMEOUT_SECONDS {
        return Err(format!(
            "grader.timeout_seconds 必须在 1..={MAX_GRADER_TIMEOUT_SECONDS} 之间"
        ));
    }
    Ok(())
}

fn is_shell_interpreter(program: &str) -> bool {
    let base = program.rsplit('/').next().unwrap_or(program).to_ascii_lowercase();
    matches!(
        base.as_str(),
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" | "cmd" | "cmd.exe" | "powershell" | "pwsh"
    )
}

fn is_safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn is_safe_relative_glob(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.starts_with('\\')
        && !value.split(['/', '\\']).any(|seg| seg == "..")
        && !value.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_task() -> String {
        serde_json::json!({
            "schema_version": 1,
            "task_id": "smoke__example-1",
            "suite": "harmonybench-smoke-v0",
            "problem_statement": "修复给定问题并验证结果。",
            "repo": { "url": "https://example.invalid/repo.git", "base_commit": "0123456789abcdef0123456789abcdef01234567" },
            "limits": { "wall_time_seconds": 1800, "max_steps": 200, "max_cost_cny": 20.0, "network": "none" },
            "grader": { "kind": "command", "command": ["npm", "test"], "timeout_seconds": 600 },
            "artifacts": ["test-results/**"]
        })
        .to_string()
    }

    #[test]
    fn accepts_a_well_formed_task() {
        let task = parse_eval_task(&valid_task()).unwrap();
        assert_eq!(task.task_id, "smoke__example-1");
        assert_eq!(task.grader.command, vec!["npm", "test"]);
        assert_eq!(task.limits.network, "none");
    }

    #[test]
    fn rejects_wrong_schema_version() {
        let mut value: serde_json::Value = serde_json::from_str(&valid_task()).unwrap();
        value["schema_version"] = serde_json::json!(2);
        assert!(parse_eval_task(&value.to_string()).is_err());
    }

    #[test]
    fn rejects_host_path_and_command_substitution_in_grader() {
        for command in [
            vec!["/usr/bin/rm", "-rf"],
            vec!["sh", "-c", "npm test; rm -rf /"],
            vec!["npm", "test", "$(curl evil)"],
            vec!["../../bin/run"],
        ] {
            let mut value: serde_json::Value = serde_json::from_str(&valid_task()).unwrap();
            value["grader"]["command"] = serde_json::json!(command);
            assert!(parse_eval_task(&value.to_string()).is_err(), "{command:?}");
        }
    }

    #[test]
    fn rejects_network_and_unsafe_artifacts() {
        let mut value: serde_json::Value = serde_json::from_str(&valid_task()).unwrap();
        value["limits"]["network"] = serde_json::json!("full");
        assert!(parse_eval_task(&value.to_string()).is_err());

        let mut value: serde_json::Value = serde_json::from_str(&valid_task()).unwrap();
        value["artifacts"] = serde_json::json!(["../../etc/passwd"]);
        assert!(parse_eval_task(&value.to_string()).is_err());
    }

    #[test]
    fn rejects_bad_commit_and_task_id() {
        let mut value: serde_json::Value = serde_json::from_str(&valid_task()).unwrap();
        value["repo"]["base_commit"] = serde_json::json!("not a sha");
        assert!(parse_eval_task(&value.to_string()).is_err());

        let mut value: serde_json::Value = serde_json::from_str(&valid_task()).unwrap();
        value["task_id"] = serde_json::json!("bad id with spaces");
        assert!(parse_eval_task(&value.to_string()).is_err());
    }
}
