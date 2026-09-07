use deveco_switch::agent::eval_runner::{
    run_trial, EvalRunConfig, ProcessAgentDriver, OUTCOME_CANCELLED, OUTCOME_RESOLVED,
};
use deveco_switch::agent::eval_task::parse_eval_task;
use deveco_switch::agent::headless_driver::{HeadlessAgentDriver, HeadlessProviderConfig};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_INPUT_BYTES: u64 = 1024 * 1024;

#[derive(Debug, PartialEq)]
struct EvalRunArgs {
    task: PathBuf,
    workspace: PathBuf,
    run_config: PathBuf,
    driver: String,
    driver_args: Vec<String>,
    output: PathBuf,
}

fn usage() -> &'static str {
    "用法：harmony-agent eval run --task <task.json> --workspace <repo> \\
  --run-config <run.json> --driver <absolute-adapter|builtin> [--driver-arg <arg>]... \\
  --output <new-output-dir>"
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<EvalRunArgs, String> {
    let mut values = args.into_iter();
    if values.next().as_deref() != Some("eval") || values.next().as_deref() != Some("run") {
        return Err(usage().into());
    }
    let mut task = None;
    let mut workspace = None;
    let mut run_config = None;
    let mut driver = None;
    let mut driver_args = Vec::new();
    let mut output = None;
    while let Some(flag) = values.next() {
        let value = values
            .next()
            .ok_or_else(|| format!("{flag} 缺少参数\n{}", usage()))?;
        match flag.as_str() {
            "--task" => task = Some(PathBuf::from(value)),
            "--workspace" => workspace = Some(PathBuf::from(value)),
            "--run-config" => run_config = Some(PathBuf::from(value)),
            "--driver" => driver = Some(value),
            "--driver-arg" => driver_args.push(value),
            "--output" => output = Some(PathBuf::from(value)),
            _ => return Err(format!("未知参数：{flag}\n{}", usage())),
        }
    }
    Ok(EvalRunArgs {
        task: task.ok_or("缺少 --task")?,
        workspace: workspace.ok_or("缺少 --workspace")?,
        run_config: run_config.ok_or("缺少 --run-config")?,
        driver: driver.ok_or("缺少 --driver")?,
        driver_args,
        output: output.ok_or("缺少 --output")?,
    })
}

fn read_bounded(path: &Path, label: &str) -> Result<String, String> {
    let metadata = std::fs::metadata(path).map_err(|e| format!("读取 {label} 元数据失败：{e}"))?;
    if !metadata.is_file() || metadata.len() > MAX_INPUT_BYTES {
        return Err(format!("{label} 必须是至多 1 MiB 的普通文件"));
    }
    std::fs::read_to_string(path).map_err(|e| format!("读取 {label} 失败：{e}"))
}

async fn execute(args: EvalRunArgs) -> Result<i32, String> {
    let task = parse_eval_task(&read_bounded(&args.task, "task")?)?;
    let config: EvalRunConfig =
        serde_json::from_str(&read_bounded(&args.run_config, "run config")?)
            .map_err(|e| format!("run config JSON 无法解析：{e}"))?;
    let workspace = args
        .workspace
        .canonicalize()
        .map_err(|e| format!("workspace 不存在或不可访问：{e}"))?;
    if !workspace.is_dir() {
        return Err("workspace 必须是目录".into());
    }
    if args.output.exists() {
        return Err("output 已存在；为防覆盖，请指定新的输出目录".into());
    }
    let result = if args.driver == "builtin" {
        if !args.driver_args.is_empty() {
            return Err("builtin driver 不接受 --driver-arg".into());
        }
        let provider = HeadlessProviderConfig::from_env()
            .map_err(|e| format!("builtin driver 配置失败：{e:?}"))?;
        provider
            .validate_against(&config.model)
            .map_err(|e| format!("builtin driver 配置与 run-config 不一致：{e:?}"))?;
        let adapter = HeadlessAgentDriver::new(provider);
        run_trial(&task, &workspace, &args.output, &adapter, &config).await
    } else {
        let driver = PathBuf::from(&args.driver)
            .canonicalize()
            .map_err(|e| format!("driver 不存在或不可访问：{e}"))?;
        let adapter = ProcessAgentDriver {
            program: driver,
            args: args.driver_args,
            timeout: Duration::from_secs(task.limits.wall_time_seconds),
        };
        run_trial(&task, &workspace, &args.output, &adapter, &config).await
    };
    match result {
        Ok(outcome) => Ok(if outcome.status == OUTCOME_RESOLVED {
            0
        } else {
            1
        }),
        Err(error) if error.starts_with(OUTCOME_CANCELLED) => {
            eprintln!("{error}");
            Ok(130)
        }
        Err(error) => Err(error),
    }
}

#[tokio::main]
async fn main() {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    match execute(args).await {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("eval run 失败：{error}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn parses_required_values_and_repeated_driver_args() {
        let parsed = parse_args(
            [
                "eval",
                "run",
                "--task",
                "t.json",
                "--workspace",
                "repo",
                "--run-config",
                "r.json",
                "--driver",
                "/bin/agent",
                "--driver-arg",
                "--model",
                "--driver-arg",
                "x",
                "--output",
                "out",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();
        assert_eq!(parsed.driver_args, vec!["--model", "x"]);
        assert_eq!(parsed.output, PathBuf::from("out"));
    }

    #[test]
    fn rejects_unknown_or_incomplete_commands() {
        assert!(parse_args(["chat"].into_iter().map(str::to_string)).is_err());
        assert!(parse_args(
            ["eval", "run", "--wat", "x"]
                .into_iter()
                .map(str::to_string)
        )
        .is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_runs_a_complete_local_trial() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("harmony-eval-cli-{}", uuid::Uuid::new_v4()));
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("a.txt"), "base\n").unwrap();
        for args in [
            ["init", "-q"].as_slice(),
            ["add", "a.txt"].as_slice(),
            [
                "-c",
                "user.email=e@x",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "-m",
                "base",
            ]
            .as_slice(),
        ] {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success());
        }
        let base = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&repo)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        let task_path = root.join("task.json");
        std::fs::write(&task_path, serde_json::json!({
            "schema_version": 1, "task_id": "cli__smoke", "suite": "cli-smoke",
            "problem_statement": "fix a.txt", "repo": { "url": "file:///repo", "base_commit": base },
            "limits": { "wall_time_seconds": 10, "max_steps": 5, "max_cost_cny": 0.0, "network": "none" },
            "grader": { "kind": "command", "command": ["grep", "-q", "fixed", "a.txt"], "timeout_seconds": 5 },
            "artifacts": []
        }).to_string()).unwrap();
        let config_path = root.join("run.json");
        let digest = format!("sha256:{}", "a".repeat(64));
        std::fs::write(&config_path, serde_json::json!({
            "run_id": "cli-smoke", "suite_version": "1", "grader_version": "command-v1",
            "harness": { "commit": "abcdef1", "app_version": "test", "platform": std::env::consts::OS },
            "model": { "provider": "stub", "model_id": "stub", "protocol": "process", "reasoning_effort": "none" },
            "prompt": { "profile_version": "1", "digest": digest },
            "tool_registry": { "version": "1", "digest": digest },
            "sandbox": { "backend": "test", "capabilities": "workspace-write", "image_digest": null, "network_policy": "none" }
        }).to_string()).unwrap();
        let driver = root.join("driver.sh");
        std::fs::write(&driver, "#!/bin/sh\ncat >/dev/null\nprintf 'fixed\\n' > a.txt\nprintf '{\"steps\":1,\"tool_calls\":1}'\n").unwrap();
        let mut permissions = std::fs::metadata(&driver).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&driver, permissions).unwrap();
        let output = root.join("output");
        let code = execute(EvalRunArgs {
            task: task_path,
            workspace: repo,
            run_config: config_path,
            driver: driver.to_string_lossy().into_owned(),
            driver_args: vec![],
            output: output.clone(),
        })
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert!(output.join("report.json").is_file());
        std::fs::remove_dir_all(root).ok();
    }
}
