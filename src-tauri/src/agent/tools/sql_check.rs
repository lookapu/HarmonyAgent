//! SQL 候选写入前的执行检查门禁（sqlite3 内存库差分）。
//!
//! SQL 的“语义”只能靠一个真实引擎来给：这里把基线与候选分别喂给 `sqlite3 :memory:` 执行，
//! 只拦**新出现**的错误。文件内自洽的问题（重复列名、INSERT 值个数与表列数不符、语法错误）
//! 会被抓到；引用工程别处 schema 的语句在两侧同样报错，会被差抵消。
//!
//! 安全边界（为什么可以执行候选）：库固定在内存里（`:memory:`，不落盘），并且候选命中
//! 可能触碰外部文件的语句（`ATTACH`/`DETACH`/`VACUUM INTO`/`CREATE VIRTUAL TABLE`/点命令）
//! 时**直接不执行**、按「未做检查」降级——不能为了校验就把不可信内容跑起来产生副作用。
//!
//! 边界：不做 schema 感知的语义分析（工程其它文件的建表语句不参与），不判断迁移顺序；
//! 无 sqlite3、候选含副作用语句、文件过大或超时都不阻塞写入，但如实标注「未做检查」。

use std::path::Path;
use std::time::Duration;

const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
/// 单次执行的内容上限（避免把超大文件塞进进程参数）。
const MAX_SQL_BYTES: usize = 256 * 1024;

/// 可能影响宿主文件的语句/点命令：命中即不执行候选。
const SIDE_EFFECT_MARKERS: &[&str] = &[
    "attach",
    "detach",
    "vacuum into",
    "create virtual table",
    ".read",
    ".open",
    ".output",
    ".once",
    ".shell",
    ".system",
    ".load",
    ".import",
    ".backup",
    ".restore",
    ".save",
];

/// 依赖工程别处 schema 才会出现的错误：两侧都会出现（或候选新增引用时只出现在候选侧），
/// 属于「单文件执行看不全工程」的产物，不能算真实回归。
const CONTEXT_MISSING_MARKERS: &[&str] = &[
    "no such table",
    "no such column",
    "no such function",
    "no such index",
    "no such view",
    "no such trigger",
    "no such module",
    "no such vtab",
    "unknown database",
];

pub(super) enum SqlCheck {
    Checked {
        before: usize,
        after: usize,
        added: Vec<String>,
    },
    Skipped {
        reason: String,
    },
}

/// 解析可用的 sqlite3：显式环境变量 → PATH → 系统自带位置（macOS/Linux 通常都有）。
fn resolve_sqlite3() -> Option<String> {
    if let Some(explicit) = std::env::var("HARMONY_SQLITE3_PATH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Path::new(&explicit).is_file().then_some(explicit);
    }
    if crate::utils::process::command("sqlite3", &[]).is_ok() {
        return Some("sqlite3".to_string());
    }
    ["/usr/bin/sqlite3", "/opt/homebrew/bin/sqlite3", "/usr/local/bin/sqlite3"]
        .into_iter()
        .find(|candidate| Path::new(candidate).is_file())
        .map(str::to_string)
}

/// 命中可能触碰宿主文件的语句时返回原因（用于降级说明）。
fn side_effect_reason(source: &str) -> Option<String> {
    let lowered = source.to_lowercase();
    SIDE_EFFECT_MARKERS
        .iter()
        .find(|marker| lowered.contains(*marker))
        .map(|marker| format!("候选包含可能产生副作用的语句（{marker}），未执行检查"))
}

/// 运行一次 sqlite3；返回（上下文缺失类错误数, 真实错误消息）。
///
/// 用 stdin 而不是把 SQL 当命令行参数：参数形式只报第一个错误（`Error: in prepare, ...`），
/// stdin 形式会逐行报出全部错误并带行号，差分才有意义。
fn execute(sqlite3: &str, dir: &Path, tag: &str, sql: &str) -> Result<Option<(usize, Vec<String>)>, String> {
    let script = dir.join(format!("{tag}.sql"));
    std::fs::write(&script, sql).map_err(|error| format!("写入 SQL 临时脚本失败：{error}"))?;
    let args = vec!["-batch".to_string(), ":memory:".to_string()];
    let captured = crate::utils::process::output_stderr_blocking_with_timeout_stdin(
        sqlite3,
        &args,
        CHECK_TIMEOUT,
        &script,
    )?;
    let _ = std::fs::remove_file(&script);
    let (_code, stderr) = match captured {
        Some(value) => value,
        None => return Ok(None),
    };
    Ok(Some(split_errors(&stderr)))
}

/// 解析 `Parse error near line N: <message>`，丢掉行号；并分离依赖工程 schema 的错误。
fn split_errors(stderr: &str) -> (usize, Vec<String>) {
    let mut context_missing = 0;
    let mut real = Vec::new();
    for line in stderr.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("Parse error near line ") else {
            continue;
        };
        let Some((_line_no, message)) = rest.split_once(": ") else {
            continue;
        };
        let message = message.trim();
        if message.is_empty() {
            continue;
        }
        if CONTEXT_MISSING_MARKERS
            .iter()
            .any(|marker| message.starts_with(marker))
        {
            context_missing += 1;
            continue;
        }
        real.push(message.to_string());
    }
    real.sort();
    (context_missing, real)
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

/// 对候选 SQL 做执行差分。只应在 `path` 为 .sql 时调用。
pub(super) fn check(path: &Path, before: &str, after: &str) -> SqlCheck {
    let _ = path;
    if after.len() > MAX_SQL_BYTES || before.len() > MAX_SQL_BYTES {
        return SqlCheck::Skipped {
            reason: format!("SQL 文件超过 {}KB，跳过执行检查", MAX_SQL_BYTES / 1024),
        };
    }
    if let Some(reason) = side_effect_reason(after).or_else(|| side_effect_reason(before)) {
        return SqlCheck::Skipped { reason };
    }
    let Some(sqlite3) = resolve_sqlite3() else {
        return SqlCheck::Skipped {
            reason: "未找到 sqlite3 可执行文件（可设置 HARMONY_SQLITE3_PATH）".into(),
        };
    };
    let dir = std::env::temp_dir().join(format!("harmony-sqlcheck-{}", uuid::Uuid::new_v4()));
    if let Err(error) = std::fs::create_dir_all(&dir) {
        return SqlCheck::Skipped {
            reason: format!("无法创建 SQL 临时目录：{error}"),
        };
    }
    let result = run(&sqlite3, &dir, before, after);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn run(sqlite3: &str, dir: &Path, before: &str, after: &str) -> SqlCheck {
    let (baseline_missing, baseline) = match execute(sqlite3, dir, "baseline", before) {
        Ok(Some(value)) => value,
        Ok(None) => {
            return SqlCheck::Skipped {
                reason: format!("sqlite3 基线执行超时（>{}s）", CHECK_TIMEOUT.as_secs()),
            }
        }
        Err(reason) => return SqlCheck::Skipped { reason },
    };
    let (candidate_missing, candidate) = match execute(sqlite3, dir, "candidate", after) {
        Ok(Some(value)) => value,
        Ok(None) => {
            return SqlCheck::Skipped {
                reason: format!("sqlite3 候选执行超时（>{}s）", CHECK_TIMEOUT.as_secs()),
            }
        }
        Err(reason) => return SqlCheck::Skipped { reason },
    };
    // 某一侧只剩「依赖工程 schema」的错误时无法判定，降级而不是冒充干净
    if (baseline.is_empty() && baseline_missing > 0) || (candidate.is_empty() && candidate_missing > 0) {
        return SqlCheck::Skipped {
            reason: "单文件执行缺少工程 schema，sqlite3 结果不完整，未做判定".into(),
        };
    }
    SqlCheck::Checked {
        before: baseline.len(),
        after: candidate.len(),
        added: added_messages(&baseline, &candidate),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_drop_line_numbers_and_separate_context_problems() {
        let stderr = "\
Parse error near line 1: no such table: missing
Parse error near line 2: table t has 1 columns but 2 values were supplied
Parse error near line 3: syntax error
SELECT * FROM
";
        let (missing, real) = split_errors(stderr);
        // 只有「no such table」属上下文缺失；列数不匹配与语法错误都是要拦的真实问题
        assert_eq!(missing, 1);
        assert_eq!(
            real,
            vec![
                "syntax error".to_string(),
                "table t has 1 columns but 2 values were supplied".to_string()
            ]
        );
    }

    #[test]
    fn side_effect_statements_are_refused() {
        for sql in [
            "ATTACH DATABASE '/tmp/x.db' AS x;\n",
            "VACUUM INTO '/tmp/out.db';\n",
            "CREATE VIRTUAL TABLE t USING fts5(a);\n",
            ".read other.sql\n",
        ] {
            assert!(side_effect_reason(sql).is_some(), "{sql}");
        }
        assert!(side_effect_reason("CREATE TABLE t (a INT);\n").is_none());
    }

    #[test]
    fn added_messages_counts_repeats() {
        let before = vec!["syntax error".to_string()];
        let after = vec!["syntax error".to_string(), "near \"(\": syntax error".to_string()];
        assert_eq!(
            added_messages(&before, &after),
            vec!["near \"(\": syntax error".to_string()]
        );
    }

    /// 真实 sqlite3：候选引入的值个数不匹配必须被抓到（文件内自洽的语义错误）。
    #[test]
    fn real_execution_catches_value_count_mismatch() {
        if resolve_sqlite3().is_none() {
            eprintln!("跳过：本机没有 sqlite3");
            return;
        }
        let before = "CREATE TABLE t (a INT);\nINSERT INTO t VALUES (1);\n";
        let after = "CREATE TABLE t (a INT);\nINSERT INTO t VALUES (1, 2);\n";
        match check(Path::new("a.sql"), before, after) {
            SqlCheck::Checked { added, .. } => {
                assert!(
                    added.iter().any(|item| item.contains("2 values were supplied")),
                    "{added:?}"
                );
            }
            SqlCheck::Skipped { reason } => panic!("sqlite3 可用时不应跳过：{reason}"),
        }
    }

    /// 引用工程别处 schema 的语句在两侧同报，必须降级而不是当成干净或误拒。
    #[test]
    fn schema_dependent_sql_degrades_instead_of_guessing() {
        if resolve_sqlite3().is_none() {
            eprintln!("跳过：本机没有 sqlite3");
            return;
        }
        let before = "SELECT * FROM other_file_table;\n";
        let after = "SELECT * FROM other_file_table;\nSELECT 1;\n";
        match check(Path::new("a.sql"), before, after) {
            SqlCheck::Skipped { reason } => assert!(reason.contains("schema"), "{reason}"),
            SqlCheck::Checked { added, .. } => panic!("缺少 schema 时不应判定，得到 {added:?}"),
        }
    }

    #[test]
    fn clean_candidate_has_no_added_errors() {
        if resolve_sqlite3().is_none() {
            eprintln!("跳过：本机没有 sqlite3");
            return;
        }
        let before = "CREATE TABLE t (a INT);\nINSERT INTO t VALUES (1);\n";
        let after = "CREATE TABLE t (a INT);\nINSERT INTO t VALUES (2);\n";
        match check(Path::new("a.sql"), before, after) {
            SqlCheck::Checked { added, .. } => assert!(added.is_empty(), "{added:?}"),
            SqlCheck::Skipped { reason } => panic!("sqlite3 可用时不应跳过：{reason}"),
        }
    }
}
