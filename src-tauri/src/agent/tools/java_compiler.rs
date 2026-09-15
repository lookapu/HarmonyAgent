//! Java 候选写入前的编译器类型门禁（javac 差分诊断）。
//!
//! 语法树能验证语法与注解形态，但判断不了父类是否存在、`@Override` 是否真的覆盖、
//! 被删除的 import 是否仍被引用。这里调用本机 javac 把基线与候选各编译一次，只比较
//! **新出现**的编译诊断：新增即拒绝，修复或持平放行。
//!
//! 差分而非绝对判定：工程 classpath 未解析时依赖缺失会在基线与候选中同时出现并被抵消。
//! 无 javac 或编译超时不阻塞写入，但状态如实标注为「未做类型校验」——不得表述为已完成
//! 类型检查，也不得据此宣称 Java 语义闭环完成。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 单次 javac 编译的墙钟上限；超时按「未校验」降级而不是判定失败。
const COMPILE_TIMEOUT: Duration = Duration::from_secs(10);
/// 诊断条数上限，避免大工程把错误截断在两份输出里不对称。
const MAX_DIAGNOSTICS: &str = "2000";

pub(super) enum JavaTypeCheck {
    /// 两份输出都拿到并完成差分；`added` 为候选新增的诊断签名。
    Checked {
        before: usize,
        after: usize,
        added: Vec<String>,
    },
    /// 没有 javac 或编译超时：没有做类型校验，调用方不得阻塞写入。
    Unavailable { reason: String },
}

/// 对完整的候选文件做 javac 差分。只应在 `path` 为 .java 时调用。
pub(super) fn check(path: &Path, before: &str, after: &str) -> JavaTypeCheck {
    let work = match work_dir() {
        Some(dir) => dir,
        None => {
            return JavaTypeCheck::Unavailable {
                reason: "无法创建 javac 临时工作目录".into(),
            }
        }
    };
    let result = run(&work, path, before, after);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn run(work: &Path, path: &Path, before: &str, after: &str) -> JavaTypeCheck {
    let package = package_declaration(after);
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("Candidate.java");
    let relative = package_path(&package, name);
    let out = work.join("out");
    if let Err(error) = std::fs::create_dir_all(&out) {
        return JavaTypeCheck::Unavailable {
            reason: format!("创建 javac 输出目录失败：{error}"),
        };
    }
    let candidate_file = work.join("candidate").join(&relative);
    if let Err(error) = write_source(&candidate_file, after) {
        return JavaTypeCheck::Unavailable { reason: error };
    }
    let source_path = source_root(path, &package);
    // 快速路径：候选自包含且无诊断时无需基线，省掉一次编译。
    let candidate_errors = match compile(&candidate_file, &out, &source_path) {
        Ok(Some(errors)) => errors,
        Ok(None) => {
            return JavaTypeCheck::Unavailable {
                reason: format!("javac 编译超时（>{}s）", COMPILE_TIMEOUT.as_secs()),
            }
        }
        Err(reason) => return JavaTypeCheck::Unavailable { reason },
    };
    if candidate_errors.is_empty() {
        return JavaTypeCheck::Checked {
            before: 0,
            after: 0,
            added: Vec::new(),
        };
    }
    let baseline_file = work.join("baseline").join(&relative);
    if let Err(error) = write_source(&baseline_file, before) {
        return JavaTypeCheck::Unavailable { reason: error };
    }
    let baseline_errors = match compile(&baseline_file, &out, &source_path) {
        Ok(Some(errors)) => errors,
        Ok(None) => {
            return JavaTypeCheck::Unavailable {
                reason: format!("javac 基线编译超时（>{}s）", COMPILE_TIMEOUT.as_secs()),
            }
        }
        Err(reason) => return JavaTypeCheck::Unavailable { reason },
    };
    JavaTypeCheck::Checked {
        before: baseline_errors.len(),
        after: candidate_errors.len(),
        added: added_signatures(&baseline_errors, &candidate_errors),
    }
}

fn work_dir() -> Option<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("harmony-javac-{}-{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn write_source(file: &Path, source: &str) -> Result<(), String> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建 javac 源码临时目录失败：{error}"))?;
    }
    std::fs::write(file, source).map_err(|error| format!("写入 javac 候选源码失败：{error}"))
}

/// 编译单个源文件；返回归一化后的诊断签名（空 = 无错），`None` 表示超时。
fn compile(file: &Path, out: &Path, source_path: &Path) -> Result<Option<Vec<String>>, String> {
    let mut args = vec![
        "-proc:none".to_string(),
        "-nowarn".to_string(),
        "-implicit:none".to_string(),
        "-Xmaxerrs".to_string(),
        MAX_DIAGNOSTICS.to_string(),
        // 锁定英文诊断：默认 locale 下消息会本地化，差分结果随环境漂移
        "-J-Duser.language=en".to_string(),
        "-d".to_string(),
        out.to_string_lossy().to_string(),
    ];
    if source_path.is_dir() {
        args.push("-sourcepath".to_string());
        args.push(source_path.to_string_lossy().to_string());
    }
    args.push(file.to_string_lossy().to_string());
    let captured = crate::utils::process::output_stderr_blocking_with_timeout(
        "javac",
        &args,
        COMPILE_TIMEOUT,
    )?;
    let (code, stderr) = match captured {
        Some(value) => value,
        None => return Ok(None),
    };
    if code == 0 {
        return Ok(Some(Vec::new()));
    }
    Ok(Some(diagnostic_signatures(&stderr)))
}

/// 把 javac stderr 归一化为可比较的诊断签名：去掉文件路径与行列号，保留错误消息和
/// `symbol/location/required/found/reason` 细节行——行号随编辑漂移不会产生假新增。
fn diagnostic_signatures(stderr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for line in stderr.lines() {
        if let Some(message) = header_message(line) {
            if !current.is_empty() {
                out.push(current.join(" | "));
            }
            current = vec![message.trim().to_string()];
            continue;
        }
        if current.is_empty() {
            continue;
        }
        let detail = line.trim();
        if detail.starts_with("symbol:")
            || detail.starts_with("location:")
            || detail.starts_with("required:")
            || detail.starts_with("found:")
            || detail.starts_with("reason:")
        {
            current.push(detail.to_string());
        }
    }
    if !current.is_empty() {
        out.push(current.join(" | "));
    }
    // 不同 javac 版本在细节行上的对齐空格数不一致；折叠空白后再比较，避免版本差异
    // 被当成新增诊断。
    out.into_iter()
        .map(|signature| {
            signature
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// 识别 `路径:行号: error: 消息` 诊断头（源码回显与脱字符行不匹配）。
fn header_message(line: &str) -> Option<&str> {
    const MARKER: &str = ": error: ";
    let index = line.find(MARKER)?;
    let (prefix, message) = (&line[..index], &line[index + MARKER.len()..]);
    let (_, line_no) = prefix.rsplit_once(':')?;
    line_no
        .chars()
        .all(|c| c.is_ascii_digit())
        .then_some(message)
}

/// 候选相对基线新增的诊断（按签名计数，同一诊断多出一次也算新增）。
fn added_signatures(before: &[String], after: &[String]) -> Vec<String> {
    let mut counts: HashMap<&str, i64> = HashMap::new();
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

/// 取文件首个非注释行声明的包名（package 语句必须在最前，取不到即默认包）。
fn package_declaration(source: &str) -> String {
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
        {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("package") {
            if rest.starts_with(char::is_whitespace) {
                let name = rest.split(';').next().unwrap_or_default().trim();
                if !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '$')
                {
                    return name.to_string();
                }
            }
        }
        break;
    }
    String::new()
}

fn package_path(package: &str, file_name: &str) -> PathBuf {
    let mut path = PathBuf::new();
    for segment in package.split('.').filter(|segment| !segment.is_empty()) {
        path.push(segment);
    }
    path.push(file_name);
    path
}

/// 按包名从文件目录上溯推导源码根（`.../src/main/java/com/foo/Bar.java` + `com.foo`
/// → `.../src/main/java`）。路径与包名对不上时退回文件所在目录。
fn source_root(path: &Path, package: &str) -> PathBuf {
    let Some(parent) = path.parent() else {
        return PathBuf::from(".");
    };
    let segments: Vec<&str> = package.split('.').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return parent.to_path_buf();
    }
    let mut dir = parent.to_path_buf();
    for segment in segments.iter().rev() {
        let matches = dir
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(segment));
        if !matches {
            return parent.to_path_buf();
        }
        match dir.parent() {
            Some(up) => dir = up.to_path_buf(),
            None => return parent.to_path_buf(),
        }
    }
    if dir.as_os_str().is_empty() {
        return PathBuf::from(".");
    }
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 无 javac 的环境（如未装 JDK 的构建机）不做编译断言，如实跳过而不是伪造通过；
    /// 「工具缺失 → 降级」这半条链路由 process.rs 的缺程序测试覆盖。
    fn checked_or_skip(result: JavaTypeCheck) -> Option<(usize, Vec<String>)> {
        match result {
            JavaTypeCheck::Checked { added, .. } => Some((added.len(), added)),
            JavaTypeCheck::Unavailable { reason } => {
                assert!(!reason.is_empty(), "降级必须给出可解释原因");
                eprintln!("跳过 javac 差分断言（本机未做类型校验）：{reason}");
                None
            }
        }
    }

    #[test]
    fn diagnostic_signatures_drop_path_and_line_but_keep_symbol_details() {
        let stderr = "\
/tmp/x/a/A.java:2: error: cannot find symbol
class A { void f() { Foo x = null; } }
                     ^
  symbol:   class Foo
  location: class A
2 errors
";
        let signatures = diagnostic_signatures(stderr);
        assert_eq!(signatures.len(), 1);
        assert_eq!(signatures[0], "cannot find symbol | symbol: class Foo | location: class A");
    }

    #[test]
    fn diagnostic_signatures_ignore_notes_and_source_echo() {
        let stderr = "\
/A.java:3: warning: [unchecked] unchecked call
    var x = raw();
/A.java:9: error: class B is public, should be declared in a file named B.java
public class B {}
       ^
1 error
";
        let signatures = diagnostic_signatures(stderr);
        assert_eq!(signatures.len(), 1);
        assert!(signatures[0].starts_with("class B is public"));
    }

    #[test]
    fn added_signatures_counts_repeats_and_orders_stably() {
        let before = vec!["cannot find symbol | symbol: class Foo".to_string()];
        let after = vec![
            "cannot find symbol | symbol: class Foo".to_string(),
            "cannot find symbol | symbol: class Foo".to_string(),
            "cannot find symbol | symbol: class Bar".to_string(),
        ];
        let added = added_signatures(&before, &after);
        assert_eq!(
            added,
            vec![
                "cannot find symbol | symbol: class Bar".to_string(),
                "cannot find symbol | symbol: class Foo".to_string()
            ]
        );
    }

    #[test]
    fn package_declaration_skips_leading_comments_only() {
        assert_eq!(package_declaration("package a.b;\nclass A {}"), "a.b");
        assert_eq!(
            package_declaration("/* c */\n// c\npackage a_b.c$d;\nclass A {}"),
            "a_b.c$d"
        );
        assert_eq!(package_declaration("class A {}\npackage a;"), "");
        assert_eq!(package_declaration(""), "");
    }

    #[test]
    fn source_root_strips_package_segments() {
        let path = Path::new("/proj/src/main/java/com/foo/Bar.java");
        assert_eq!(
            source_root(path, "com.foo"),
            PathBuf::from("/proj/src/main/java")
        );
        // 包名与目录不一致时退回文件所在目录，避免把 sourcepath 指到错误根
        assert_eq!(
            source_root(path, "org.other"),
            PathBuf::from("/proj/src/main/java/com/foo")
        );
        assert_eq!(source_root(path, ""), PathBuf::from("/proj/src/main/java/com/foo"));
    }

    #[test]
    fn javac_diff_rejects_import_removed_while_referenced() {
        let before = "package a;\nimport java.util.List;\nclass A { List<String> xs; }\n";
        let after = "package a;\nclass A { List<String> xs; }\n";
        let Some((count, added)) = checked_or_skip(check(Path::new("A.java"), before, after))
        else {
            return;
        };
        assert!(count > 0, "{added:?}");
        assert!(added.iter().any(|item| item.contains("cannot find symbol")));
    }

    #[test]
    fn javac_diff_rejects_missing_supertype_and_broken_override() {
        let base = "package a;\nclass A {}\n";
        for after in [
            "package a;\nclass A extends MissingBase {}\n",
            "package a;\nclass A { @Override public void unknown() {} }\n",
        ] {
            let Some((count, added)) = checked_or_skip(check(Path::new("A.java"), base, after))
            else {
                return;
            };
            assert!(count > 0, "{after} -> {added:?}");
        }
    }

    #[test]
    fn javac_diff_rejects_new_reference_to_unknown_type() {
        let before = "package a;\nclass A { void f() { Foo x = null; } }\n";
        let after = "package a;\nclass A { void f() { Foo x = null; Bar y = null; } }\n";
        let Some((count, added)) = checked_or_skip(check(Path::new("A.java"), before, after))
        else {
            return;
        };
        assert!(count > 0, "{added:?}");
        assert!(added.iter().any(|item| item.contains("class Bar")));
    }

    /// 既有诊断的行号随编辑漂移不能被当成新增（差分按签名计数，不按行列号）。
    #[test]
    fn javac_diff_tolerates_line_shift_of_preexisting_error() {
        let before = "package a;\nclass A {\n  Foo a;\n}\n";
        let after = "package a;\n// 新增注释使既有错误行号漂移\nclass A {\n  Foo a;\n}\n";
        let Some((count, added)) = checked_or_skip(check(Path::new("A.java"), before, after))
        else {
            return;
        };
        assert_eq!(count, 0, "{added:?}");
    }

    #[test]
    fn javac_diff_accepts_clean_edit_and_repair_of_existing_error() {
        let Some((count, added)) = checked_or_skip(check(
            Path::new("A.java"),
            "package a;\nclass A {}\n",
            "package a;\nclass A { int value() { return 1; } }\n",
        )) else {
            return;
        };
        assert_eq!(count, 0, "{added:?}");
        let Some((count, added)) = checked_or_skip(check(
            Path::new("A.java"),
            "package a;\nclass A { Foo x; }\n",
            "package a;\nclass A { Object x; }\n",
        )) else {
            return;
        };
        assert_eq!(count, 0, "{added:?}");
    }
}
