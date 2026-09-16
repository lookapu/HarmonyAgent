//! Go 候选写入前的静态检查门禁（`go vet` 差分）。
//!
//! 与 Dart 的分析器门禁同构：基线与候选各检查一次，只拦**新出现**的诊断。
//! 语法层已由 tree-sitter 覆盖（见 `code_mutation`），这里补的是类型与静态检查
//! （类型不匹配、未定义标识符、printf 格式不匹配等）。
//!
//! 候选不能写进用户源码树（门禁坚持「验证通过前不落盘」），因此把候选放进**临时模块**：
//! 临时目录里写 `go.mod`（并尽量沿用真实模块的 module 路径与 `go.sum`），让标准库与
//! 模块缓存里的依赖能解析；同模块内的兄弟包解析不到，两侧同样解析不到，会被差分抵消。
//!
//! 工程上下文缺失类诊断（找不到包/模块）**不算新增错误**，否则「新增一个合法 import」
//! 就会把正常编辑拒掉；若某一侧只剩这类诊断，则整体按「未做检查」降级而不是当作干净。
//!
//! 边界：不做跨包检查，改动破坏同模块其它包不在覆盖内；不联网（`GOPROXY=off`）；
//! 无 `go`、找不到 go.mod 或超时都不阻塞写入，但如实标注「未做检查」。

use std::path::{Path, PathBuf};
use std::time::Duration;

const VET_TIMEOUT: Duration = Duration::from_secs(20);

/// 工程上下文缺失导致的诊断：两侧都会出现（或候选新增 import 时只出现在候选侧），
/// 但都属于「临时模块看不全工程」的产物，不能当作真实回归。
const CONTEXT_MISSING_MARKERS: &[&str] = &[
    "no required module provides package",
    "cannot find package",
    "is not in std",
    "no Go files in",
    "go.mod file not found",
    "missing go.sum entry",
    "updates to go.mod needed",
    "unrecognized import path",
];

pub(super) enum GoCheck {
    Checked {
        before: usize,
        after: usize,
        added: Vec<String>,
    },
    Skipped {
        reason: String,
    },
}

/// 解析可用的 go 可执行文件：PATH 优先，其次 Go 的标准安装位置与 Homebrew。
fn resolve_go() -> Option<String> {
    if let Some(explicit) = std::env::var("HARMONY_GO_PATH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Path::new(&explicit).is_file().then_some(explicit);
    }
    if crate::utils::process::command("go", &[]).is_ok() {
        return Some("go".to_string());
    }
    let home = std::env::var("HOME").ok()?;
    [
        "/usr/local/go/bin/go".to_string(),
        "/opt/homebrew/bin/go".to_string(),
        format!("{home}/go/bin/go"),
        format!("{home}/.local/share/mise/shims/go"),
    ]
    .into_iter()
    .find(|candidate| Path::new(candidate).is_file())
}

/// 从文件所在目录上溯找模块根（含 `go.mod`）。
fn module_root(path: &Path) -> Option<PathBuf> {
    let mut dir = path.parent()?.to_path_buf();
    for _ in 0..32 {
        if dir.join("go.mod").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
    None
}

fn work_dir() -> Option<PathBuf> {
    let dir = std::env::temp_dir().join(format!("harmony-govet-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// 在临时模块里放好待检查文件，并尽量沿用真实模块的 module 路径与依赖锁定信息。
fn prepare(dir: &Path, source: &str, target: &Path, module_root: &Path) -> Result<(), String> {
    let name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("candidate.go");
    std::fs::write(dir.join(name), source)
        .map_err(|error| format!("写入 Go 临时源文件失败：{error}"))?;
    let module_path = std::fs::read_to_string(module_root.join("go.mod"))
        .ok()
        .and_then(|content| {
            content
                .lines()
                .find_map(|line| line.trim().strip_prefix("module ").map(|rest| rest.trim().to_string()))
        })
        .unwrap_or_else(|| "harmonycheck".to_string());
    let go_directive = std::fs::read_to_string(module_root.join("go.mod"))
        .ok()
        .and_then(|content| {
            content
                .lines()
                .find_map(|line| line.trim().strip_prefix("go ").map(|rest| rest.trim().to_string()))
        })
        .unwrap_or_else(|| "1.20".to_string());
    std::fs::write(
        dir.join("go.mod"),
        format!("module {module_path}\n\ngo {go_directive}\n"),
    )
    .map_err(|error| format!("写入 Go 临时 go.mod 失败：{error}"))?;
    // 依赖版本锁定信息一并带上：外部依赖才能从本地模块缓存解析，不需要联网
    if let Ok(sum) = std::fs::read_to_string(module_root.join("go.sum")) {
        let _ = std::fs::write(dir.join("go.sum"), sum);
    }
    Ok(())
}

/// 运行 `go vet`；返回（内容缺失类诊断数, 真实诊断签名）。
fn vet(go: &str, dir: &Path) -> Result<Option<(usize, Vec<String>)>, String> {
    let args = vec!["vet".to_string(), "./...".to_string()];
    let envs = [
        ("GOPROXY".to_string(), "off".to_string()),
        // 临时模块可能缺 go.sum 条目；不要因此改写用户的工程
        ("GOFLAGS".to_string(), "-mod=mod".to_string()),
    ];
    let captured = crate::utils::process::output_stderr_blocking_with_timeout_opts(
        go,
        &args,
        VET_TIMEOUT,
        Some(dir),
        &envs,
    )?;
    let (_code, stderr) = match captured {
        Some(value) => value,
        None => return Ok(None),
    };
    Ok(Some(split_diagnostics(&stderr)))
}

/// 解析 `vet: ./a.go:4:14: <message>` 形式的诊断：丢掉路径与行列号（候选是临时文件、
/// 行号也会漂移），只留消息；同时分离出「工程上下文缺失」类诊断。
fn split_diagnostics(stderr: &str) -> (usize, Vec<String>) {
    let mut context_missing = 0;
    let mut real = Vec::new();
    for line in stderr.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("vet: ") else {
            continue;
        };
        // 去掉 `./<file>:<line>:<col>: ` 前缀，保留可读消息
        let message = rest
            .splitn(4, ':')
            .nth(3)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(rest);
        if CONTEXT_MISSING_MARKERS
            .iter()
            .any(|marker| message.contains(marker))
        {
            context_missing += 1;
            continue;
        }
        real.push(message.to_string());
    }
    real.sort();
    (context_missing, real)
}

fn added_signatures(before: &[String], after: &[String]) -> Vec<String> {
    let mut counts: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
    for signature in before {
        *counts.entry(signature.as_str()).or_default() -= 1;
    }
    for signature in after {
        *counts.entry(signature.as_str()).or_default() += 1;
    }
    let mut added: Vec<String> = counts
        .into_iter()
        .filter(|(_, delta)| *delta > 0)
        .map(|(signature, _)| signature.to_string())
        .collect();
    added.sort();
    added
}

/// 对候选文件做 `go vet` 差分。只应在 `path` 为 .go 时调用。
pub(super) fn check(path: &Path, before: &str, after: &str) -> GoCheck {
    let Some(go) = resolve_go() else {
        return GoCheck::Skipped {
            reason: "未找到 go 可执行文件（可设置 HARMONY_GO_PATH）".into(),
        };
    };
    let Some(root) = module_root(path) else {
        return GoCheck::Skipped {
            reason: "目标不在 Go 模块内（未找到 go.mod），跳过静态检查".into(),
        };
    };
    let Some(baseline_dir) = work_dir() else {
        return GoCheck::Skipped {
            reason: "无法创建 Go 临时模块目录".into(),
        };
    };
    let Some(candidate_dir) = work_dir() else {
        let _ = std::fs::remove_dir_all(&baseline_dir);
        return GoCheck::Skipped {
            reason: "无法创建 Go 临时模块目录".into(),
        };
    };
    let result = run(&go, path, before, after, &root, &baseline_dir, &candidate_dir);
    let _ = std::fs::remove_dir_all(&baseline_dir);
    let _ = std::fs::remove_dir_all(&candidate_dir);
    result
}

fn run(
    go: &str,
    path: &Path,
    before: &str,
    after: &str,
    root: &Path,
    baseline_dir: &Path,
    candidate_dir: &Path,
) -> GoCheck {
    if let Err(reason) = prepare(baseline_dir, before, path, root) {
        return GoCheck::Skipped { reason };
    }
    if let Err(reason) = prepare(candidate_dir, after, path, root) {
        return GoCheck::Skipped { reason };
    }
    let (baseline_missing, baseline) = match vet(go, baseline_dir) {
        Ok(Some(value)) => value,
        Ok(None) => {
            return GoCheck::Skipped {
                reason: format!("go vet 基线超时（>{}s）", VET_TIMEOUT.as_secs()),
            }
        }
        Err(reason) => return GoCheck::Skipped { reason },
    };
    let (candidate_missing, candidate) = match vet(go, candidate_dir) {
        Ok(Some(value)) => value,
        Ok(None) => {
            return GoCheck::Skipped {
                reason: format!("go vet 候选超时（>{}s）", VET_TIMEOUT.as_secs()),
            }
        }
        Err(reason) => return GoCheck::Skipped { reason },
    };
    // 某一侧只剩「工程上下文缺失」时无法判定，按未检查降级，不冒充干净
    if (baseline.is_empty() && baseline_missing > 0) || (candidate.is_empty() && candidate_missing > 0) {
        return GoCheck::Skipped {
            reason: "临时模块解析不到工程依赖，go vet 结果不完整，未做判定".into(),
        };
    }
    GoCheck::Checked {
        before: baseline.len(),
        after: candidate.len(),
        added: added_signatures(&baseline, &candidate),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_drop_position_and_separate_context_problems() {
        let stderr = "\
# harmonycheck
vet: ./a.go:4:14: cannot use \"str\" (untyped string constant) as int value in variable declaration
vet: ./a.go:9:2: no required module provides package github.com/x/y; to add it: go get github.com/x/y
vet: ./a.go:3:8: undefined: missingThing
";
        let (missing, real) = split_diagnostics(stderr);
        assert_eq!(missing, 1);
        assert_eq!(
            real,
            vec![
                "cannot use \"str\" (untyped string constant) as int value in variable declaration".to_string(),
                "undefined: missingThing".to_string()
            ]
        );
    }

    #[test]
    fn added_signatures_ignores_repeats_of_existing_problems() {
        let before = vec!["undefined: a".to_string()];
        let after = vec!["undefined: a".to_string(), "undefined: b".to_string()];
        assert_eq!(added_signatures(&before, &after), vec!["undefined: b".to_string()]);
    }

    #[test]
    fn files_outside_a_go_module_are_skipped() {
        let dir = std::env::temp_dir().join(format!("govet-not-module-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.go");
        std::fs::write(&file, "package main\n").unwrap();
        match check(&file, "package main\n", "package main\n") {
            GoCheck::Skipped { reason } => assert!(reason.contains("go.mod"), "{reason}"),
            GoCheck::Checked { .. } => panic!("非 Go 模块内不应产生检查结果"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 真实 go vet：候选引入类型错误必须被抓到，且不留下临时目录。
    #[test]
    fn real_vet_catches_type_error_in_candidate() {
        let Some(_go) = resolve_go() else {
            eprintln!("跳过：本机没有 go");
            return;
        };
        let dir = std::env::temp_dir().join(format!("govet-module-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("go.mod"), "module probe\n\ngo 1.20\n").unwrap();
        let file = dir.join("a.go");
        let before = "package main\n\nfunc main() {\n\tvar x int = 1\n\t_ = x\n}\n";
        let after = "package main\n\nfunc main() {\n\tvar x int = \"str\"\n\t_ = x\n}\n";
        std::fs::write(&file, before).unwrap();
        match check(&file, before, after) {
            GoCheck::Checked { added, .. } => {
                assert!(
                    added.iter().any(|item| item.contains("as int value")),
                    "{added:?}"
                );
            }
            GoCheck::Skipped { reason } => panic!("go 可用时不应跳过：{reason}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clean_candidate_has_no_added_diagnostics() {
        let Some(_go) = resolve_go() else {
            eprintln!("跳过：本机没有 go");
            return;
        };
        let dir = std::env::temp_dir().join(format!("govet-clean-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("go.mod"), "module probe\n\ngo 1.20\n").unwrap();
        let file = dir.join("a.go");
        let before = "package main\n\nfunc main() {\n\tvar x int = 1\n\t_ = x\n}\n";
        let after = "package main\n\nfunc main() {\n\tvar x int = 2\n\t_ = x\n}\n";
        std::fs::write(&file, before).unwrap();
        match check(&file, before, after) {
            GoCheck::Checked { added, .. } => assert!(added.is_empty(), "{added:?}"),
            GoCheck::Skipped { reason } => panic!("go 可用时不应跳过：{reason}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
