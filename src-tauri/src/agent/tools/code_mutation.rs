//! 代码写入前的统一候选验证。
//!
//! 所有文件修改工具先在内存中形成完整候选文本，再经过这里的语言门禁。当前 P0 在
//! 通用配平守卫之上为 ArkTS/TypeScript/JavaScript/Java 提供真实语法树错误增量检查；其它
//! 语言明确回退配平层，后续由 language adapter 逐步补齐，不能把 fallback 冒充 AST。

use std::path::Path;

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

#[cfg(test)]
mod tests {
    use super::*;

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
