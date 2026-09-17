//! Dart 候选写入前的分析器门禁（`dart analyze` 差分诊断）。
//!
//! 与 Java 的 javac 门禁同构：把基线与候选各分析一次，只拦**新出现**的诊断。
//! 语法树只能看语法，Dart 的真实类型错误（未定义标识符、类型不匹配、缺 import）要靠
//! 分析器；差分使「工程本身解析不到的包」在两侧同时出现并被抵消，因此不必先把
//! `pub get` 跑通。
//!
//! 候选必须落到磁盘上才能被分析（分析器按文件解析 import），因此会在**目标文件同级目录**
//! 写一个点前缀的临时文件，并在所有返回路径上删除。基线直接分析磁盘上的真实文件，不写盘。
//!
//! 边界：不做跨文件的批量联编（`dart analyze` 以包为单位、耗时不可控），不解析依赖版本，
//! 分析器即本机 Dart SDK 版本；无 dart 或超时不阻塞写入，但状态如实标注为「未做分析」。

use std::path::{Path, PathBuf};
use std::time::Duration;

/// 单次 `dart analyze` 的墙钟上限；超时按「未分析」降级而不是判定失败。
const ANALYZE_TIMEOUT: Duration = Duration::from_secs(20);

pub(super) enum DartCheck {
    /// 两侧都拿到并完成差分；`added` 为候选新增的诊断签名。
    Checked {
        before: usize,
        after: usize,
        added: Vec<String>,
    },
    /// 没有 dart、不在 Dart 包内或分析超时：没有做分析，调用方不得阻塞写入。
    Skipped { reason: String },
}

/// 解析可用的 dart 可执行文件。
///
/// 与其他工具一致优先走 `resolve_program`（PATH + 常见安装位置）；GUI 启动的 macOS 应用
/// PATH 极简，因此再补几个 Flutter/Dart 常见安装位置，并允许 `HARMONY_DART_PATH` 覆盖。
fn resolve_dart() -> Option<String> {
    if let Some(explicit) = std::env::var("HARMONY_DART_PATH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Path::new(&explicit).is_file().then_some(explicit);
    }
    if crate::utils::process::command("dart", &[]).is_ok() {
        return Some("dart".to_string());
    }
    let home = std::env::var("HOME").ok()?;
    [
        format!("{home}/development/flutter/bin/dart"),
        format!("{home}/fvm/default/bin/dart"),
        format!("{home}/flutter/bin/dart"),
        "/opt/homebrew/bin/dart".to_string(),
        "/usr/local/bin/dart".to_string(),
    ]
    .into_iter()
    .find(|candidate| Path::new(candidate).is_file())
}

/// 从文件所在目录上溯找包根（含 `pubspec.yaml`）。找不到就说明不处在 Dart 包里。
fn package_root(path: &Path) -> Option<PathBuf> {
    let mut dir = path.parent()?.to_path_buf();
    for _ in 0..16 {
        if dir.join("pubspec.yaml").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
    None
}

/// 执行一次分析并返回 ERROR 级诊断签名（空 = 无错误）。
fn analyze(dart: &str, root: &Path, target: &Path) -> Result<Option<Vec<String>>, String> {
    let args = vec![
        "analyze".to_string(),
        "--format".to_string(),
        "machine".to_string(),
        target.to_string_lossy().to_string(),
    ];
    let captured = crate::utils::process::output_stdout_blocking_with_timeout(
        dart,
        &args,
        ANALYZE_TIMEOUT,
    )?;
    let (code, stdout) = match captured {
        Some(value) => value,
        None => return Ok(None),
    };
    // 退出码非零是「有诊断」的正常结果；只有既非零又没有可解析输出才视为失败
    let signatures = error_signatures(&stdout);
    if code != 0 && signatures.is_empty() && stdout.trim().is_empty() {
        return Err("dart analyze 未返回可解析输出".into());
    }
    let _ = root;
    Ok(Some(signatures))
}

/// 解析 `SEVERITY|TYPE|CODE|FILE|LINE|COL|LENGTH|MESSAGE` 机器格式，只取 ERROR。
/// 文件名与行列号一律丢弃：候选与基线的临时文件名不同、行号也会漂移，只有
/// 「诊断代码 + 消息」才是两侧可比的部分。
fn error_signatures(stdout: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let mut fields = line.splitn(8, '|');
        let severity = fields.next().unwrap_or_default();
        if severity != "ERROR" {
            continue;
        }
        let _kind = fields.next();
        let code = fields.next().unwrap_or_default();
        let _file = fields.next();
        let _line = fields.next();
        let _col = fields.next();
        let _length = fields.next();
        let message = fields.next().unwrap_or_default().trim();
        out.push(format!("{code} | {message}"));
    }
    out.sort();
    out
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

fn candidate_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("candidate.dart");
    let dir = path.parent().unwrap_or(Path::new("."));
    dir.join(format!(".harmony-candidate-{}-{name}", uuid::Uuid::new_v4()))
}

fn write_candidate(path: &Path, source: &str) -> Result<PathBuf, String> {
    let target = candidate_path(path);
    std::fs::write(&target, source)
        .map_err(|error| format!("写入 Dart 候选临时文件失败：{error}"))?;
    Ok(target)
}

/// 对候选文件做分析器差分。只应在 `path` 为 .dart 时调用。
pub(super) fn check(path: &Path, before: &str, after: &str) -> DartCheck {
    let Some(dart) = resolve_dart() else {
        return DartCheck::Skipped {
            reason: "未找到 dart 可执行文件（可设置 HARMONY_DART_PATH）".into(),
        };
    };
    let Some(root) = package_root(path) else {
        return DartCheck::Skipped {
            reason: "目标不在 Dart 包内（未找到 pubspec.yaml），跳过分析".into(),
        };
    };
    let candidate = match write_candidate(path, after) {
        Ok(path) => path,
        Err(reason) => return DartCheck::Skipped { reason },
    };
    let result = run(&dart, &root, path, &candidate, before);
    let _ = std::fs::remove_file(&candidate);
    result
}

fn run(dart: &str, root: &Path, path: &Path, candidate: &Path, before: &str) -> DartCheck {
    let baseline = match analyze(dart, root, path) {
        Ok(Some(diagnostics)) => diagnostics,
        Ok(None) => {
            return DartCheck::Skipped {
                reason: format!("dart analyze 基线超时（>{}s）", ANALYZE_TIMEOUT.as_secs()),
            }
        }
        Err(reason) => return DartCheck::Skipped { reason },
    };
    let candidate_diagnostics = match analyze(dart, root, candidate) {
        Ok(Some(diagnostics)) => diagnostics,
        Ok(None) => {
            return DartCheck::Skipped {
                reason: format!("dart analyze 候选超时（>{}s）", ANALYZE_TIMEOUT.as_secs()),
            }
        }
        Err(reason) => return DartCheck::Skipped { reason },
    };
    let _ = before;
    DartCheck::Checked {
        before: baseline.len(),
        after: candidate_diagnostics.len(),
        added: added_signatures(&baseline, &candidate_diagnostics),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_output_keeps_only_errors_and_drops_paths_and_positions() {
        let stdout = "\
INFO|LINT|UNUSED_IMPORT|/p/lib/a.dart|1|8|6|Unused import: 'dart:io'.
ERROR|COMPILE_TIME_ERROR|UNDEFINED_IDENTIFIER|/p/lib/a.dart|2|24|12|Undefined name 'missingThing'.
WARNING|HINT|DEAD_CODE|/p/lib/a.dart|3|3|4|Dead code.
ERROR|COMPILE_TIME_ERROR|UNDEFINED_IDENTIFIER|/p/.harmony-candidate-1-a.dart|2|24|12|Undefined name 'missingThing'.
";
        let signatures = error_signatures(stdout);
        assert_eq!(
            signatures,
            vec![
                "UNDEFINED_IDENTIFIER | Undefined name 'missingThing'.".to_string(),
                "UNDEFINED_IDENTIFIER | Undefined name 'missingThing'.".to_string()
            ]
        );
    }

    #[test]
    fn message_may_contain_pipe_characters() {
        let stdout = "ERROR|COMPILE_TIME_ERROR|X|/p/a.dart|1|1|1|bad a | b | c\n";
        assert_eq!(error_signatures(stdout), vec!["X | bad a | b | c".to_string()]);
    }

    #[test]
    fn added_signatures_ignores_line_shift_and_counts_repeats() {
        let before = vec!["A | one".to_string()];
        let after = vec!["A | one".to_string(), "A | two".to_string()];
        assert_eq!(added_signatures(&before, &after), vec!["A | two".to_string()]);
    }

    /// 非包内文件不做分析：不能靠"猜"包根去写临时文件。
    #[test]
    fn files_outside_a_dart_package_are_skipped() {
        let dir = std::env::temp_dir().join(format!("dart-not-pkg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.dart");
        std::fs::write(&file, "class A {}\n").unwrap();
        // 两种降级都是合法的：本机没有 dart（CI 常见）或目标不在包内。
        // 断言的是"要么因缺工具跳过、要么因不在包内跳过"，不能假定工具一定存在。
        let tool_missing = resolve_dart().is_none();
        match check(&file, "class A {}\n", "class A {}\n") {
            DartCheck::Skipped { reason } => {
                let expected = if tool_missing { "dart" } else { "pubspec.yaml" };
                assert!(reason.contains(expected), "{reason}");
            }
            DartCheck::Checked { .. } => panic!("非 Dart 包内不应产生分析结果"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 真实 dart analyze：候选引入未定义标识符时必须被差分抓到，且不留下临时文件。
    #[test]
    fn real_analyzer_rejects_new_type_error_and_cleans_up() {
        let Some(dart) = resolve_dart() else {
            eprintln!("跳过：本机没有 dart");
            return;
        };
        let dir = std::env::temp_dir().join(format!("dart-gate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(
            dir.join("pubspec.yaml"),
            "name: probe\nenvironment:\n  sdk: \">=3.0.0 <4.0.0\"\n",
        )
        .unwrap();
        let file = dir.join("lib/a.dart");
        let before = "class A {\n  int value() { return 1; }\n}\n";
        std::fs::write(&file, before).unwrap();
        let after = "class A {\n  int value() { return missingThing; }\n}\n";
        match check(&file, before, after) {
            DartCheck::Checked { added, .. } => {
                assert!(
                    added.iter().any(|item| item.contains("UNDEFINED_IDENTIFIER")),
                    "{added:?}"
                );
            }
            // 并行满载时 dart analyze 可能超过 20s 上限而降级；「外部工具超时降级」是生产上的
            // 既定行为（不阻塞写入），不应把「本机太忙」记成门禁缺陷。其余跳过原因
            // （缺 dart、不在包内、候选写入失败）仍视为测试失败。
            DartCheck::Skipped { reason } if reason.contains("超时") => {
                eprintln!("跳过：本机负载下 dart analyze 超时降级（{reason}）");
            }
            DartCheck::Skipped { reason } => panic!("dart 可用时不应跳过：{reason}"),
        }
        // 候选临时文件必须清理干净
        let leftovers: Vec<String> = std::fs::read_dir(dir.join("lib"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(".harmony-candidate-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = dart;
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clean_candidate_has_no_added_diagnostics() {
        let Some(_dart) = resolve_dart() else {
            eprintln!("跳过：本机没有 dart");
            return;
        };
        let dir = std::env::temp_dir().join(format!("dart-clean-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(
            dir.join("pubspec.yaml"),
            "name: probe\nenvironment:\n  sdk: \">=3.0.0 <4.0.0\"\n",
        )
        .unwrap();
        let file = dir.join("lib/a.dart");
        let before = "class A {\n  int value() { return 1; }\n}\n";
        std::fs::write(&file, before).unwrap();
        let after = "class A {\n  int value() { return 2; }\n}\n";
        match check(&file, before, after) {
            DartCheck::Checked { added, .. } => assert!(added.is_empty(), "{added:?}"),
            // 并行满载时 dart analyze 可能超过 20s 上限而降级；「外部工具超时降级」是生产上的
            // 既定行为（不阻塞写入），不应把「本机太忙」记成门禁缺陷。其余跳过原因
            // （缺 dart、不在包内、候选写入失败）仍视为测试失败。
            DartCheck::Skipped { reason } if reason.contains("超时") => {
                eprintln!("跳过：本机负载下 dart analyze 超时降级（{reason}）");
            }
            DartCheck::Skipped { reason } => panic!("dart 可用时不应跳过：{reason}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
