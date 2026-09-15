//! 代码写入前的统一候选验证。
//!
//! 所有文件修改工具先在内存中形成完整候选文本，再经过这里的语言门禁。当前 P0 在
//! 通用配平守卫之上为 ArkTS/TypeScript/JavaScript/Java 提供真实语法树错误增量检查；其它
//! 语言明确回退配平层，后续由 language adapter 逐步补齐，不能把 fallback 冒充 AST。

use std::path::{Path, PathBuf};


fn java_boundary_error() -> String {
    "结构编辑句柄边界不安全：Java 范围无法唯一对应完整声明，可能包含相邻同类节点或多变量字段。候选内容未落盘；请重新查询结构，或读取后使用精确 old/new 修改。".into()
}

/// 文件 SHA 与结构身份由调用方先验证；在绑定行范围中必须恰好找到一个同种 AST 声明。
/// 返回原文 UTF-8 字节边界，包含注解，绝不包含同行邻居。
pub(super) fn resolve_java_handle_byte_range(
    source: &str,
    start: usize,
    end: usize,
    expected_kind: Option<&str>,
) -> Result<(usize, usize), String> {
    let expected_kind = expected_kind.ok_or_else(java_boundary_error)?;
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .map_err(|e| e.to_string())?;
    let tree = parser.parse(source, None).ok_or_else(java_boundary_error)?;
    if tree.root_node().has_error() || start == 0 || end < start {
        return Err(java_boundary_error());
    }
    let mut found = None;
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        let kind = match node.kind() {
            "class_declaration" | "record_declaration" => "class",
            "interface_declaration" | "annotation_type_declaration" => "interface",
            "enum_declaration" => "enum",
            "method_declaration"
            | "constructor_declaration"
            | "compact_constructor_declaration" => "method",
            "field_declaration" => "field",
            _ => "",
        };
        if kind == expected_kind
            && node.start_position().row + 1 == start
            && node.end_position().row + 1 == end
        {
            let mut children = node.walk();
            let grouped = kind == "field"
                && node
                    .named_children(&mut children)
                    .filter(|child| child.kind() == "variable_declarator")
                    .count()
                    != 1;
            if grouped || found.is_some() {
                return Err(java_boundary_error());
            }
            found = Some((node.start_byte(), node.end_byte()));
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return found.ok_or_else(java_boundary_error);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MutationGuardReport {
    pub parser: &'static str,
    pub before_errors: usize,
    pub after_errors: usize,
}

fn tree_sitter_language(ext: &str) -> Option<tree_sitter::Language> {
    match ext {
        "ets" => Some(tree_sitter_arkts::LANGUAGE.into()),
        "ts" | "js" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" | "jsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        _ => None,
    }
}

fn syntax_error_count(node: tree_sitter::Node<'_>) -> usize {
    let mut cursor = node.walk();
    let mut errors = 0;
    loop {
        let node = cursor.node();
        errors += usize::from(node.is_error() || node.is_missing());
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return errors;
            }
        }
    }
}

fn parse_error_count(language: &tree_sitter::Language, text: &str) -> Result<usize, String> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(language)
        .map_err(|error| format!("初始化语法解析器失败：{error}"))?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| "语法解析器未返回语法树".to_string())?;
    Ok(syntax_error_count(tree.root_node()))
}

/// 依据真实 AST 判断注解所附着的声明，不扫描注释、字符串或猜测后续行。
/// 仅检查常见标准注解的声明种类，不解析继承、classpath 或依赖注入类型。
fn java_analysis(text: &str) -> Result<(usize, usize), String> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .map_err(|error| format!("初始化 Java 语法解析器失败：{error}"))?;
    let tree = parser.parse(text, None).ok_or("Java 解析器未返回语法树")?;
    let mut syntax = 0;
    let mut declarations = 0;
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        syntax += usize::from(node.is_error() || node.is_missing());
        if matches!(node.kind(), "marker_annotation" | "annotation") {
            let name = node
                .child_by_field_name("name")
                .and_then(|name| name.utf8_text(text.as_bytes()).ok())
                .unwrap_or_default();
            let target = node
                .parent()
                .filter(|parent| parent.kind() == "modifiers")
                .and_then(|modifiers| modifiers.parent())
                .map(|target| target.kind());
            let valid = match name {
                "Override" | "java.lang.Override" => target == Some("method_declaration"),
                "Resource" | "javax.annotation.Resource" | "jakarta.annotation.Resource" => {
                    matches!(
                        target,
                        Some(
                            "field_declaration"
                                | "method_declaration"
                                | "class_declaration"
                                | "interface_declaration"
                                | "enum_declaration"
                                | "record_declaration"
                                | "annotation_type_declaration"
                        )
                    )
                }
                _ => true,
            };
            declarations += usize::from(!valid);
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return Ok((syntax, declarations));
            }
        }
    }
}

/// 验证内存中的完整候选文件。原文件已有语法错误时允许错误数下降或保持，不允许增加；
/// 原文件干净时任何新 error/missing node 都会在落盘前拒绝。
pub(super) fn validate_candidate(
    path: &Path,
    before: &str,
    after: &str,
) -> Result<MutationGuardReport, String> {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    super::fs_tools::balance_guard(before, after, &ext)?;
    if ext == "java" {
        let (before_syntax, before_declarations) = java_analysis(before)?;
        let (after_syntax, after_declarations) = java_analysis(after)?;
        if after_syntax > before_syntax || after_declarations > before_declarations {
            return Err(format!(
                "代码修改事务被 Java 声明门禁拒绝：{} 的语法错误 {}→{}，@Override/@Resource 声明目标错误 {}→{}。候选内容未落盘；请修改完整声明及其注解。此检查不替代 Java 编译器类型检查。",
                path.display(), before_syntax, after_syntax, before_declarations, after_declarations
            ));
        }
        return Ok(MutationGuardReport {
            parser: "java_tree_sitter",
            before_errors: before_syntax + before_declarations,
            after_errors: after_syntax + after_declarations,
        });
    }
    let Some(language) = tree_sitter_language(&ext) else {
        return Ok(MutationGuardReport {
            parser: "delimiter_fallback",
            before_errors: 0,
            after_errors: 0,
        });
    };
    let before_errors = parse_error_count(&language, before)?;
    let after_errors = parse_error_count(&language, after)?;
    if after_errors > before_errors {
        return Err(format!(
            "代码修改事务被语法门禁拒绝：{} 使用 tree-sitter 解析后错误节点由 {} 增至 {}。候选内容未落盘，请重新生成完整语法节点。",
            path.display(), before_errors, after_errors
        ));
    }
    Ok(MutationGuardReport {
        parser: "tree_sitter",
        before_errors,
        after_errors,
    })
}

fn is_java_source(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("java"))
}

/// 单次事务里最多并入多少个“未参与本批”的候选调用方文件（控制 javac 时长）。
const MAX_AFFECTED_FILES: usize = 20;
/// 收集候选调用方时的目录扫描上限；超出即停止扫描，按已收集结果继续。
const MAX_AFFECTED_SCAN: usize = 400;
/// 反查调用方时单个源文件的读取上限（与编辑能力上限一致）。
const MAX_AFFECTED_FILE_BYTES: u64 = 1024 * 1024;

/// 取 Java 源码中声明的类型、方法、构造器和字段名，用于反查引用方。
fn java_declaration_names(source: &str) -> Vec<String> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .is_err()
    {
        return Vec::new();
    }
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        let kind = node.kind();
        if matches!(
            kind,
            "class_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "record_declaration"
                | "annotation_type_declaration"
                | "method_declaration"
                | "constructor_declaration"
        ) {
            if let Some(name) = node
                .child_by_field_name("name")
                .and_then(|name| name.utf8_text(source.as_bytes()).ok())
            {
                names.push(name.to_string());
            }
        } else if kind == "field_declaration" {
            let mut inner = node.walk();
            for child in node.named_children(&mut inner) {
                if child.kind() != "variable_declarator" {
                    continue;
                }
                if let Some(name) = child
                    .child_by_field_name("name")
                    .and_then(|name| name.utf8_text(source.as_bytes()).ok())
                {
                    names.push(name.to_string());
                }
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                names.sort();
                names.dedup();
                return names;
            }
        }
    }
}

/// 找出同源码根下可能引用本次改动、但没被一起编辑的 Java 文件。
///
/// 只做声明名文本预筛：差分校验对“多纳入无关文件”是安全的（两侧同集合编译，无关错误
/// 会互相抵消，代价只是编译更慢），因此这里的目标是别漏掉真正的调用方而不是精确。
/// 找不到源码根、根不是绝对路径或没有声明名时返回空，由调用方按“未覆盖调用方”处理。
pub(super) fn affected_java_sources(path: &Path, before: &str) -> Vec<PathBuf> {
    let names = java_declaration_names(before);
    if names.is_empty() {
        return Vec::new();
    }
    let package = super::java_compiler::package_declaration(before);
    let root = super::java_compiler::source_root(path, &package);
    // 相对路径或文件系统根都会让扫描范围失去边界，宁可不做
    if !root.is_absolute() || root.parent().is_none() {
        return Vec::new();
    }
    let mut found: Vec<PathBuf> = Vec::new();
    let mut scanned = 0usize;
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        if found.len() >= MAX_AFFECTED_FILES || scanned >= MAX_AFFECTED_SCAN {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if found.len() >= MAX_AFFECTED_FILES || scanned >= MAX_AFFECTED_SCAN {
                break;
            }
            let candidate = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if candidate.is_dir() {
                if !name.starts_with('.') && !super::fs_tools::should_skip_dir(&name) {
                    stack.push(candidate);
                }
                continue;
            }
            if !name.ends_with(".java") || candidate == path {
                continue;
            }
            // 超过 1MB 的源文件按编辑能力上限直接跳过，避免为一次写入读入大量正文
            if entry.metadata().map(|meta| meta.len()).unwrap_or(0) > MAX_AFFECTED_FILE_BYTES {
                continue;
            }
            scanned += 1;
            let Ok(text) = std::fs::read_to_string(&candidate) else {
                continue;
            };
            if names.iter().any(|name| text.contains(name.as_str())) {
                found.push(candidate);
            }
        }
    }
    found.sort();
    found
}

/// 合并多次反查结果：去重、排除本批已编辑的文件、按全局上限截断。
fn merge_affected(batches: Vec<Vec<PathBuf>>, edited: &[PathBuf]) -> Vec<PathBuf> {
    let mut merged: Vec<PathBuf> = Vec::new();
    for batch in batches {
        for path in batch {
            if edited.contains(&path) || merged.contains(&path) {
                continue;
            }
            merged.push(path);
        }
    }
    merged.truncate(MAX_AFFECTED_FILES);
    merged
}

/// 写入路径上的完整门禁：先跑语法/注解检查，再为 Java 追加 javac 类型诊断差分。
///
/// 差分只拦「候选新增的编译诊断」——既有工程依赖缺失会在两侧同时出现并抵消，因此
/// 不解析 Maven/Gradle classpath 也能拦住误删仍在使用的 import、不存在的父类和错误的
/// override。无 javac 或编译超时时降级为未校验（记录事件，不阻塞写入），绝不冒充已校验。
pub(super) fn validate_candidate_with_types(
    path: &Path,
    before: &str,
    after: &str,
) -> Result<MutationGuardReport, String> {
    let report = validate_candidate(path, before, after)?;
    if !is_java_source(path) {
        return Ok(report);
    }
    let affected = affected_java_sources(path, before);
    match super::java_compiler::check_batch_with_affected(&[(path, before, after)], &affected) {
        super::java_compiler::JavaTypeCheck::Checked {
            before: before_errors,
            after: after_errors,
            added,
            affected_files,
            affected_skipped,
        } => {
            log_affected_skipped(affected_skipped, path.display().to_string(), affected_files);
            if !added.is_empty() {
                let shown: Vec<String> = added.iter().take(3).cloned().collect();
                return Err(format!(
                    "代码修改事务被 Java 编译器门禁拒绝：{} 的 javac 诊断由 {} 条增至 {} 条，新增：{}。候选内容未落盘；请修正引用的类型或补回仍在使用的 import 后重试。判定为单文件 javac 差分{}，未解析工程 classpath。",
                    path.display(),
                    before_errors,
                    after_errors,
                    shown.join("；"),
                    coverage_note(affected_files, affected_skipped),
                ));
            }
        }
        super::java_compiler::JavaTypeCheck::Unavailable { reason } => {
            crate::utils::logger::log_event(
                "java_type_gate_unavailable",
                serde_json::json!({ "path": path.display().to_string(), "reason": reason }),
            );
        }
    }
    Ok(report)
}

/// 多文件事务的 Java 类型门禁：本批候选联编一次，并并入同源码根下可能引用改动的
/// 未编辑文件（真正的调用方）。非 Java 批次无需调用；无 javac 或超时降级为未校验。
pub(super) fn validate_batch_types(files: &[(&Path, &str, &str)]) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }
    let edited: Vec<PathBuf> = files.iter().map(|(path, _, _)| path.to_path_buf()).collect();
    let affected = merge_affected(
        files
            .iter()
            .map(|(path, before, _)| affected_java_sources(path, before))
            .collect(),
        &edited,
    );
    match super::java_compiler::check_batch_with_affected(files, &affected) {
        super::java_compiler::JavaTypeCheck::Checked {
            before,
            after,
            added,
            affected_files,
            affected_skipped,
        } => {
            log_affected_skipped(affected_skipped, format!("{} 个文件", files.len()), affected_files);
            if added.is_empty() {
                return Ok(());
            }
            let shown: Vec<String> = added.iter().take(3).cloned().collect();
            Err(format!(
                "代码修改事务被 Java 编译器门禁拒绝：本批 {} 个候选联编后 javac 诊断由 {} 条增至 {} 条，新增：{}。该批候选未写入任何文件；请修正引用的类型或补回仍在使用的 import 后重试。判定为同批候选联编{}，未解析工程 classpath。",
                files.len(),
                before,
                after,
                shown.join("；"),
                coverage_note(affected_files, affected_skipped),
            ))
        }
        super::java_compiler::JavaTypeCheck::Unavailable { reason } => {
            crate::utils::logger::log_event(
                "java_type_gate_unavailable",
                serde_json::json!({ "files": files.len(), "reason": reason }),
            );
            Ok(())
        }
    }
}

/// 编译超时导致退回只编本批时如实记录：调用方覆盖已放弃，不能对外宣称已覆盖。
fn log_affected_skipped(affected_skipped: bool, target: String, affected_files: usize) {
    if !affected_skipped {
        return;
    }
    crate::utils::logger::log_event(
        "java_type_gate_affected_skipped",
        serde_json::json!({ "target": target, "affected_files": affected_files }),
    );
}

/// 说明本次是否并入了未参与编辑的调用方文件（如实标注，不夸大战果）。
fn coverage_note(affected_files: usize, affected_skipped: bool) -> String {
    if affected_skipped {
        "（并入了可能引用改动的文件，但编译超时后已退回只编本批，调用方未覆盖）".into()
    } else if affected_files > 0 {
        format!("（并编译了 {affected_files} 个可能引用改动的未编辑文件）")
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_byte_range_preserves_unicode_neighbors_and_includes_annotation() {
        let source = "class A {\n String s = \"中文\"; @Override public String toString() { return s; } int tail;\n}";
        let (start, end) = resolve_java_handle_byte_range(source, 2, 2, Some("method")).unwrap();
        assert_eq!(
            &source[start..end],
            "@Override public String toString() { return s; }"
        );
        let result = format!("{}{}", &source[..start], &source[end..]);
        assert!(result.contains("String s = \"中文\";"));
        assert!(result.contains("int tail;"));
        assert!(!result.contains("@Override"));
        assert!(validate_candidate(Path::new("A.java"), source, &result).is_ok());
        assert!(resolve_java_handle_byte_range(
            "class A { void a() {} void b() {} }",
            1,
            1,
            Some("method")
        )
        .is_err());
    }

    #[test]
    fn java_handle_requires_unique_complete_declaration() {
        let valid = "class A {\r\n  @Override\r\n  public void run() {}\r\n}\r\n";
        assert!(resolve_java_handle_byte_range(valid, 2, 3, Some("method")).is_ok());
        for (source, start, end, kind) in [
            ("class A {\n void a() {} void b() {}\n}", 2, 2, "method"),
            ("class A {\n int a, b;\n}", 2, 2, "field"),
            (
                "class A {\n @Override\n public void run() {}\n}",
                3,
                3,
                "method",
            ),
        ] {
            assert!(
                resolve_java_handle_byte_range(source, start, end, Some(kind)).is_err(),
                "{source}"
            );
        }
        assert!(resolve_java_handle_byte_range(valid, 2, 3, None).is_err());
    }

    #[test]
    fn rejects_balanced_but_invalid_typescript_before_write() {
        let error = validate_candidate(
            Path::new("src/a.ts"),
            "function f() { return 1; }\n",
            "function f() { const value = ; return value; }\n",
        )
        .unwrap_err();
        assert!(error.contains("语法门禁拒绝"));
        assert!(error.contains("未落盘"));
    }

    #[test]
    fn accepts_error_reducing_repair_and_reports_parser_source() {
        let report = validate_candidate(
            Path::new("src/a.ts"),
            "function f() { const value = ; return value; }\n",
            "function f() { const value = 1; return value; }\n",
        )
        .unwrap();
        assert_eq!(report.parser, "tree_sitter");
        assert!(report.after_errors < report.before_errors);
    }

    #[test]
    fn java_guard_rejects_new_orphan_annotations() {
        let error = validate_candidate(
            Path::new("src/A.java"),
            "class A {\n  @Override\n  public String toString() { return \"A\"; }\n}\n",
            "class A {\n  @Override\n}\n",
        )
        .unwrap_err();
        assert!(error.contains("Java 声明门禁拒绝"), "{error}");
        assert!(error.contains("未落盘"), "{error}");
    }

    #[test]
    fn java_guard_accepts_deleting_annotation_with_declaration() {
        let report = validate_candidate(
            Path::new("src/A.java"),
            "class A {\n  @Resource\n  private Service service;\n}\n",
            "class A {\n}\n",
        )
        .unwrap();
        assert_eq!(report.parser, "java_tree_sitter");
        assert_eq!(report.after_errors, 0);
    }

    #[test]
    fn java_ast_rejects_balanced_invalid_syntax_and_wrong_annotation_targets() {
        let before = "class A {}";
        for after in [
            "class A { int value = ; }",
            "class A { @Override int value; }",
            "class A { @Override A() {} }",
            "class A { @javax.annotation.Resource A() {} }",
        ] {
            assert!(
                validate_candidate(Path::new("A.java"), before, after).is_err(),
                "{after}"
            );
        }
    }

    #[test]
    fn java_ast_ignores_comments_and_supports_multiline_annotation_arguments() {
        let after = r#"class A {
            /*
             @Override
             @Resource
            */
            String marker = "@Override";
            @jakarta.annotation.Resource(
                name = "service",
                description = "a (quoted) description"
            )
            private Service service = new Service();
            @java.lang.Override
            public String toString() { return "A"; }
        }"#;
        let report = validate_candidate(Path::new("A.java"), "class A {}", after).unwrap();
        assert_eq!(report.parser, "java_tree_sitter");
        assert_eq!(report.after_errors, 0);
    }

    #[test]
    fn java_ast_allows_repairs_but_does_not_trade_syntax_for_target_errors() {
        let before = "class A { int value = ; }";
        assert!(
            validate_candidate(Path::new("A.java"), before, "class A { int value = 1; }").is_ok()
        );
        assert!(validate_candidate(
            Path::new("A.java"),
            before,
            "class A { @Override int value = 1; }"
        )
        .is_err());
    }

    #[test]
    fn java_ast_does_not_claim_type_resolution() {
        // 语法树无法判断父类是否存在或方法是否真的 override；必须交给编译器/LSP。
        let report = validate_candidate(
            Path::new("A.java"),
            "class A {}",
            "class A extends MissingBase { @Override public void unknown() {} }",
        )
        .unwrap();
        assert_eq!(report.after_errors, 0);
        assert!(validate_candidate(
            Path::new("A.java"),
            "class A {}",
            "class A { @custom.Override int value; }"
        )
        .is_ok());
    }

    /// 反查调用方只按声明名做文本预筛：命中才并入，且不做无边界扫描。
    #[test]
    fn affected_java_sources_picks_declaration_name_mentions_only() {
        let dir = std::env::temp_dir().join(format!(
            "java_affected_scan_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("A.java");
        let b = dir.join("B.java");
        let c = dir.join("C.java");
        let a_src = "package a;\nclass A {\n  static int compute() { return 1; }\n}\n";
        std::fs::write(&a, a_src).unwrap();
        std::fs::write(&b, "package a;\nclass B {\n  int use() { return A.compute(); }\n}\n").unwrap();
        std::fs::write(&c, "package a;\nclass C {\n  int other() { return 2; }\n}\n").unwrap();
        assert_eq!(affected_java_sources(&a, a_src), vec![b]);
        // 相对路径可能上溯到无边界目录，宁可不扫描
        assert!(affected_java_sources(Path::new("A.java"), a_src).is_empty());
        // 没有声明名（空文件/非 Java 内容）时不做反查
        assert!(affected_java_sources(&a, "").is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unsupported_language_is_explicit_delimiter_fallback() {
        let report = validate_candidate(
            Path::new("src/a.dart"),
            "class A {}\n",
            "class A { void f() {} }\n",
        )
        .unwrap();
        assert_eq!(report.parser, "delimiter_fallback");
    }
}
