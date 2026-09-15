//! Java 候选写入前的编译器类型门禁（javac 差分诊断）。
//!
//! 语法树能验证语法与注解形态，但判断不了父类是否存在、`@Override` 是否真的覆盖、
//! 被删除的 import 是否仍被引用。这里调用本机 javac 把基线与候选各编译一次，只比较
//! **新出现**的编译诊断：新增即拒绝，修复或持平放行。
//!
//! 差分而非绝对判定：工程 classpath 未解析时依赖缺失会在基线与候选中同时出现并被抵消。
//! 多文件事务把同批候选放在一起编译，因此“同一批里 A 改坏了 B 的调用”会被拦住；
//! 但改动的文件破坏**未参与本批**的调用方仍不在覆盖内（那需要按引用反查受影响文件）。
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

/// 单文件事务（write_file / edit_file 等）的差分校验。
pub(super) fn check(path: &Path, before: &str, after: &str) -> JavaTypeCheck {
    check_batch(&[(path, before, after)])
}

/// 批量联编差分：同批候选作为一次 javac 调用的显式输入，彼此引用解析到候选版本，
/// 因此能拦住单文件校验看不到的“同批跨文件类型破坏”。空批次视为无诊断。
pub(super) fn check_batch(files: &[(&Path, &str, &str)]) -> JavaTypeCheck {
    if files.is_empty() {
        return JavaTypeCheck::Checked {
            before: 0,
            after: 0,
            added: Vec::new(),
        };
    }
    let work = match work_dir() {
        Some(dir) => dir,
        None => {
            return JavaTypeCheck::Unavailable {
                reason: "无法创建 javac 临时工作目录".into(),
            }
        }
    };
    let result = run(&work, files);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn run(work: &Path, files: &[(&Path, &str, &str)]) -> JavaTypeCheck {
    let candidate_root = work.join("candidate");
    let baseline_root = work.join("baseline");
    let out_candidate = work.join("out-candidate");
    let out_baseline = work.join("out-baseline");
    for dir in [&candidate_root, &baseline_root, &out_candidate, &out_baseline] {
        if let Err(error) = std::fs::create_dir_all(dir) {
            return JavaTypeCheck::Unavailable {
                reason: format!("创建 javac 临时目录失败：{error}"),
            };
        }
    }
    // 每个文件按包名落到两侧的源码根，得到 <侧>/<包路径>/<文件名>；两侧相对路径一致，
    // 诊断签名才可能逐条可比（绝对临时路径被剥掉）。
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut candidate_files = Vec::with_capacity(files.len());
    let mut baseline_files = Vec::with_capacity(files.len());
    for (path, before, after) in files {
        let package = package_declaration(after);
        let rel = package_path(&package, file_name(path));
        push_root(&mut roots, source_root(path, &package));
        if let Some(parent) = path.parent() {
            push_root(&mut roots, parent.to_path_buf());
        }
        let candidate_file = candidate_root.join(&rel);
        if let Err(reason) = write_source(&candidate_file, after) {
            return JavaTypeCheck::Unavailable { reason };
        }
        let baseline_file = baseline_root.join(&rel);
        if let Err(reason) = write_source(&baseline_file, before) {
            return JavaTypeCheck::Unavailable { reason };
        }
        candidate_files.push(candidate_file);
        baseline_files.push(baseline_file);
    }
    let candidate = match compile(
        &candidate_files,
        &candidate_root,
        &roots,
        &out_candidate,
    ) {
        Ok(Some(diagnostics)) => diagnostics,
        Ok(None) => {
            return JavaTypeCheck::Unavailable {
                reason: format!("javac 候选编译超时（>{}s）", COMPILE_TIMEOUT.as_secs()),
            }
        }
        Err(reason) => return JavaTypeCheck::Unavailable { reason },
    };
    // 快速路径：候选自包含且无诊断时无需基线，省掉一次编译。
    if candidate.is_empty() {
        return JavaTypeCheck::Checked {
            before: 0,
            after: 0,
            added: Vec::new(),
        };
    }
    let baseline = match compile(&baseline_files, &baseline_root, &roots, &out_baseline) {
        Ok(Some(diagnostics)) => diagnostics,
        Ok(None) => {
            return JavaTypeCheck::Unavailable {
                reason: format!("javac 基线编译超时（>{}s）", COMPILE_TIMEOUT.as_secs()),
            }
        }
        Err(reason) => return JavaTypeCheck::Unavailable { reason },
    };
    JavaTypeCheck::Checked {
        before: baseline.len(),
        after: candidate.len(),
        added: added_signatures(&baseline, &candidate),
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

fn push_root(roots: &mut Vec<PathBuf>, root: PathBuf) {
    if root.is_dir() && !roots.contains(&root) {
        roots.push(root);
    }
}

fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("Candidate.java")
}

fn write_source(file: &Path, source: &str) -> Result<(), String> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建 javac 源码临时目录失败：{error}"))?;
    }
    std::fs::write(file, source).map_err(|error| format!("写入 javac 候选源码失败：{error}"))
}

/// 编译显式给出的源文件集合；返回归一化后的诊断签名（空 = 无错），`None` 表示超时。
/// `shadow_root` 是本次候选/基线各自的源码根，用于把诊断里的绝对路径还原成相对路径。
fn compile(
    files: &[PathBuf],
    shadow_root: &Path,
    roots: &[PathBuf],
    out: &Path,
) -> Result<Option<Vec<String>>, String> {
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
    // 影子根放最前：本批未显式给出的同类文件优先解析到候选版本，其次才回落到真实源码根
    let mut search = vec![shadow_root.to_path_buf()];
    search.extend(roots.iter().cloned());
    if let Ok(joined) = std::env::join_paths(&search) {
        args.push("-sourcepath".to_string());
        args.push(joined.to_string_lossy().to_string());
    }
    for file in files {
        args.push(file.to_string_lossy().to_string());
    }
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
    Ok(Some(diagnostic_signatures(&stderr, shadow_root)))
}

/// 把 javac stderr 归一化为可比较的诊断签名：去掉行号与临时根路径（保留相对文件位置），
/// 保留错误消息和 `symbol/location/required/found/reason` 细节行——行号随编辑漂移不会
/// 产生假新增，同时仍能指出是哪个文件出的问题。
fn diagnostic_signatures(stderr: &str, shadow_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for line in stderr.lines() {
        if let Some((location, message)) = header_parts(line) {
            if !current.is_empty() {
                out.push(current.join(" | "));
            }
            current = vec![normalize_location(location, shadow_root), message.trim().to_string()];
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

/// 识别 `路径:行号: error: 消息` 诊断头（源码回显与脱字符行不匹配），
/// 返回（路径，消息）；行号被丢弃，路径交给调用方归一化。
fn header_parts(line: &str) -> Option<(&str, &str)> {
    const MARKER: &str = ": error: ";
    let index = line.find(MARKER)?;
    let (prefix, message) = (&line[..index], &line[index + MARKER.len()..]);
    let (location, line_no) = prefix.rsplit_once(':')?;
    line_no
        .chars()
        .all(|c| c.is_ascii_digit())
        .then_some((location, message))
}

/// 把影子根下的绝对路径还原成相对路径；影子根之外（真实工程文件）保持原样——两侧
/// 指向同一真实文件，字符串相同即可抵消。
fn normalize_location(location: &str, shadow_root: &Path) -> String {
    match Path::new(location).strip_prefix(shadow_root) {
        Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
        Err(_) => location.to_string(),
    }
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
    fn diagnostic_signatures_drop_line_but_keep_relative_file_and_symbol_details() {
        let stderr = "\
/tmp/x/a/A.java:2: error: cannot find symbol
class A { void f() { Foo x = null; } }
                     ^
  symbol:   class Foo
  location: class A
2 errors
";
        let signatures = diagnostic_signatures(stderr, Path::new("/tmp/x"));
        assert_eq!(signatures.len(), 1);
        assert_eq!(
            signatures[0],
            "a/A.java | cannot find symbol | symbol: class Foo | location: class A"
        );
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
        let signatures = diagnostic_signatures(stderr, Path::new("/"));
        assert_eq!(signatures.len(), 1);
        assert_eq!(
            signatures[0],
            "A.java | class B is public, should be declared in a file named B.java"
        );
    }

    #[test]
    fn added_signatures_counts_repeats_and_orders_stably() {
        let before = vec!["a/A.java | cannot find symbol | symbol: class Foo".to_string()];
        let after = vec![
            "a/A.java | cannot find symbol | symbol: class Foo".to_string(),
            "a/A.java | cannot find symbol | symbol: class Foo".to_string(),
            "a/A.java | cannot find symbol | symbol: class Bar".to_string(),
        ];
        let added = added_signatures(&before, &after);
        assert_eq!(
            added,
            vec![
                "a/A.java | cannot find symbol | symbol: class Bar".to_string(),
                "a/A.java | cannot find symbol | symbol: class Foo".to_string()
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
        assert_eq!(
            source_root(path, ""),
            PathBuf::from("/proj/src/main/java/com/foo")
        );
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
        assert!(added.iter().all(|item| item.starts_with("a/A.java | ")), "{added:?}");
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

    /// 同批候选一起编译：B 仍调用 A 已删除的方法，属于本批内部不一致，必须拦住。
    #[test]
    fn javac_batch_diff_catches_cross_file_breakage_inside_one_edit_batch() {
        let a_before = "package a;\nclass A {\n  static int value() { return 1; }\n}\n";
        let a_after = "package a;\nclass A {\n}\n";
        let b_before = "package a;\nclass B {\n  int use() { return A.value(); }\n}\n";
        let b_after = "package a;\n// 本批同时改动 B\nclass B {\n  int use() { return A.value(); }\n}\n";
        let files = [
            (Path::new("A.java"), a_before, a_after),
            (Path::new("B.java"), b_before, b_after),
        ];
        let Some((count, added)) = checked_or_skip(check_batch(&files)) else {
            return;
        };
        assert!(count > 0, "{added:?}");
        assert!(
            added.iter().any(|item| item.starts_with("a/B.java | ")),
            "新增诊断应指向批内的调用方 B：{added:?}"
        );
    }

    /// 反例：同批把调用方一起改对，联编不得误报。
    #[test]
    fn javac_batch_diff_accepts_consistent_multi_file_refactor() {
        let a_before = "package a;\nclass A {\n  static int value() { return 1; }\n}\n";
        let a_after = "package a;\nclass A {\n  static int amount() { return 1; }\n}\n";
        let b_before = "package a;\nclass B {\n  int use() { return A.value(); }\n}\n";
        let b_after = "package a;\nclass B {\n  int use() { return A.amount(); }\n}\n";
        let files = [
            (Path::new("A.java"), a_before, a_after),
            (Path::new("B.java"), b_before, b_after),
        ];
        let Some((count, added)) = checked_or_skip(check_batch(&files)) else {
            return;
        };
        assert_eq!(count, 0, "{added:?}");
    }

    #[test]
    fn javac_batch_diff_treats_empty_batch_as_clean() {
        match check_batch(&[]) {
            JavaTypeCheck::Checked { before, after, added } => {
                assert_eq!((before, after, added.len()), (0, 0, 0));
            }
            JavaTypeCheck::Unavailable { reason } => panic!("空批次不应触发编译：{reason}"),
        }
    }
}
