//! Python 候选写入前的静态检查门禁（pyflakes 差分）。
//!
//! 与 Dart/Go 的门禁同构：基线与候选各检查一次，只拦**新出现**的诊断。
//! 语法层已由 tree-sitter 覆盖（见 `code_mutation`），这里补的是文件内语义问题
//! （未定义名字、未使用 import、重复定义等）。
//!
//! 候选不能写进用户源码树（门禁坚持「验证通过前不落盘」），因此两侧都写进系统临时目录
//! 里的同名文件再检查；pyflakes 是**文件内**分析，不需要包上下文，因此不存在 go vet 那种
//! 「临时模块解析不到工程依赖」的失真。
//!
//! 边界：只认 pyflakes（本机常见做法是 `pip install pyflakes`）；没有 pyflakes/没有 python3
//! 或超时都不阻塞写入，但如实标注「未做检查」，绝不冒充已校验。

use std::path::{Path, PathBuf};
use std::time::Duration;

const LINT_TIMEOUT: Duration = Duration::from_secs(20);

/// 检查器的调用方式：独立 `pyflakes` 可执行文件，或 `python3 -m pyflakes`。
#[derive(Clone, Debug, PartialEq, Eq)]
enum Checker {
    Binary(String),
    PythonModule(String),
}

impl Checker {
    fn program(&self) -> &str {
        match self {
            Self::Binary(program) => program,
            Self::PythonModule(python) => python,
        }
    }

    fn args(&self, target: &Path) -> Vec<String> {
        let path = target.to_string_lossy().to_string();
        match self {
            Self::Binary(_) => vec![path],
            Self::PythonModule(_) => vec!["-m".to_string(), "pyflakes".to_string(), path],
        }
    }
}

pub(super) enum PythonCheck {
    Checked {
        before: usize,
        after: usize,
        added: Vec<String>,
    },
    Skipped {
        reason: String,
    },
}

/// 解析可用的检查器：显式环境变量 → PATH 上的 `pyflakes` → `python3 -m pyflakes`。
fn resolve_checker() -> Option<Checker> {
    if let Some(explicit) = std::env::var("HARMONY_PYFLAKES_PATH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Path::new(&explicit).is_file().then_some(Checker::Binary(explicit));
    }
    if crate::utils::process::command("pyflakes", &[]).is_ok() {
        return Some(Checker::Binary("pyflakes".to_string()));
    }
    for python in python_candidates() {
        if python_has_pyflakes(&python) {
            return Some(Checker::PythonModule(python));
        }
    }
    None
}

/// 常见 python3 位置：PATH 优先，其次 Homebrew/系统位置（GUI 启动的 PATH 极简）。
fn python_candidates() -> Vec<String> {
    let mut out = vec!["python3".to_string()];
    if let Ok(home) = std::env::var("HOME") {
        out.push(format!("{home}/.pyenv/shims/python3"));
    }
    out.extend([
        "/opt/homebrew/bin/python3".to_string(),
        "/usr/local/bin/python3".to_string(),
        "/usr/bin/python3".to_string(),
    ]);
    out
}

fn python_has_pyflakes(python: &str) -> bool {
    let args = vec![
        "-c".to_string(),
        "import pyflakes, sys; sys.exit(0)".to_string(),
    ];
    matches!(
        crate::utils::process::output_stdout_blocking_with_timeout(
            python,
            &args,
            Duration::from_secs(10),
        ),
        Ok(Some((0, _)))
    )
}

fn work_file(dir: &Path, target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("candidate.py");
    dir.join(name)
}

/// 运行一次 pyflakes；返回诊断消息（已丢弃文件路径与行列号）。
fn lint(checker: &Checker, dir: &Path, target: &Path) -> Result<Option<Vec<String>>, String> {
    let captured = crate::utils::process::output_stdout_blocking_with_timeout(
        checker.program(),
        &checker.args(target),
        LINT_TIMEOUT,
    )?;
    let (code, stdout) = match captured {
        Some(value) => value,
        None => return Ok(None),
    };
    // pyflakes 把诊断写 stdout，退出码 1 表示"有诊断"，都算正常结果；
    // 退出码非 0/1 且没有可解析输出时视为调用失败（例如 -m pyflakes 不存在）
    if !matches!(code, 0 | 1) {
        let hint = stdout.trim();
        let _ = dir;
        return Err(format!(
            "pyflakes 调用失败（exit={code}）{}",
            if hint.is_empty() {
                String::new()
            } else {
                format!("：{hint}")
            }
        ));
    }
    Ok(Some(diagnostic_messages(&stdout)))
}

/// 解析 `path:line:col: message`，只保留消息本体（候选是临时路径、行号也会漂移）。
fn diagnostic_messages(stdout: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // 形如 /tmp/x/a.py:3:5: local variable 'x' is assigned to but never used
        let parts: Vec<&str> = trimmed.splitn(4, ':').collect();
        if parts.len() == 4 && parts[1].trim().parse::<u32>().is_ok() {
            let message = parts[3].trim();
            if !message.is_empty() {
                out.push(message.to_string());
            }
            continue;
        }
        // 没有位置前缀的输出（如语法错误摘要）原样保留
        out.push(trimmed.to_string());
    }
    out.sort();
    out
}

fn added_messages(before: &[String], after: &[String]) -> Vec<String> {
    let mut counts: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
    for message in before {
        *counts.entry(message.as_str()).or_default() -= 1;
    }
    for message in after {
        *counts.entry(message.as_str()).or_default() += 1;
    }
    let mut added: Vec<String> = counts
        .into_iter()
        .filter(|(_, delta)| *delta > 0)
        .map(|(message, _)| message.to_string())
        .collect();
    added.sort();
    added
}

/// 对候选文件做 pyflakes 差分。只应在 `path` 为 .py 时调用。
pub(super) fn check(path: &Path, before: &str, after: &str) -> PythonCheck {
    let Some(checker) = resolve_checker() else {
        return PythonCheck::Skipped {
            reason: "未找到 pyflakes（可 pip install pyflakes，或用 HARMONY_PYFLAKES_PATH 指定）"
                .into(),
        };
    };
    let Some(dir) = work_dir() else {
        return PythonCheck::Skipped {
            reason: "无法创建 Python 临时目录".into(),
        };
    };
    let result = run(&checker, &dir, path, before, after);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn work_dir() -> Option<PathBuf> {
    let dir = std::env::temp_dir().join(format!("harmony-pyflakes-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn run(checker: &Checker, dir: &Path, path: &Path, before: &str, after: &str) -> PythonCheck {
    let baseline_file = work_file(dir, path);
    if let Err(error) = std::fs::write(&baseline_file, before) {
        return PythonCheck::Skipped {
            reason: format!("写入 Python 临时源文件失败：{error}"),
        };
    }
    let candidate_file = dir.join("candidate").join(
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("candidate.py"),
    );
    if let Some(parent) = candidate_file.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            return PythonCheck::Skipped {
                reason: format!("创建 Python 临时目录失败：{error}"),
            };
        }
    }
    if let Err(error) = std::fs::write(&candidate_file, after) {
        return PythonCheck::Skipped {
            reason: format!("写入 Python 候选临时文件失败：{error}"),
        };
    }
    let baseline = match lint(checker, dir, &baseline_file) {
        Ok(Some(messages)) => messages,
        Ok(None) => {
            return PythonCheck::Skipped {
                reason: format!("pyflakes 基线超时（>{}s）", LINT_TIMEOUT.as_secs()),
            }
        }
        Err(reason) => return PythonCheck::Skipped { reason },
    };
    let candidate = match lint(checker, dir, &candidate_file) {
        Ok(Some(messages)) => messages,
        Ok(None) => {
            return PythonCheck::Skipped {
                reason: format!("pyflakes 候选超时（>{}s）", LINT_TIMEOUT.as_secs()),
            }
        }
        Err(reason) => return PythonCheck::Skipped { reason },
    };
    PythonCheck::Checked {
        before: baseline.len(),
        after: candidate.len(),
        added: added_messages(&baseline, &candidate),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_drop_path_and_position_but_keep_text() {
        let stdout = "\
/tmp/x/a.py:3:5: local variable 'x' is assigned to but never used
/tmp/x/candidate/a.py:7:1: 'os' imported but unused
a.py:1:1: invalid syntax
";
        assert_eq!(
            diagnostic_messages(stdout),
            vec![
                "'os' imported but unused".to_string(),
                "invalid syntax".to_string(),
                "local variable 'x' is assigned to but never used".to_string()
            ]
        );
    }

    #[test]
    fn added_messages_counts_repeats() {
        let before = vec!["'os' imported but unused".to_string()];
        let after = vec![
            "'os' imported but unused".to_string(),
            "undefined name 'missing'".to_string(),
        ];
        assert_eq!(
            added_messages(&before, &after),
            vec!["undefined name 'missing'".to_string()]
        );
    }

    /// 本机没有 pyflakes 时必须走降级（不阻塞写入），并给出可操作的原因。
    /// 装了 pyflakes 的环境下这条断言退化为「解析器可用」，两种环境都不误报。
    #[test]
    fn missing_checker_degrades_with_actionable_reason() {
        let dir = std::env::temp_dir().join(format!("pyflakes-probe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.py");
        let source = "import os\n";
        std::fs::write(&file, source).unwrap();
        match check(&file, source, "import os\nprint(1)\n") {
            PythonCheck::Skipped { reason } => {
                assert!(reason.contains("pyflakes"), "{reason}");
            }
            PythonCheck::Checked { .. } => {
                // 本机装了 pyflakes：跳过这半条断言（真实调用已覆盖）
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 有 pyflakes 时：候选引入未使用 import 必须被抓到；没有则跳过。
    #[test]
    fn real_lint_reports_new_unused_import_when_available() {
        if resolve_checker().is_none() {
            eprintln!("跳过：本机没有 pyflakes");
            return;
        }
        let dir = std::env::temp_dir().join(format!("pyflakes-real-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.py");
        let before = "x = 1\nprint(x)\n";
        let after = "import os\n\nx = 1\nprint(x)\n";
        std::fs::write(&file, before).unwrap();
        match check(&file, before, after) {
            PythonCheck::Checked { added, .. } => {
                assert!(added.iter().any(|item| item.contains("os")), "{added:?}");
            }
            PythonCheck::Skipped { reason } => panic!("解析到检查器时不应跳过：{reason}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
