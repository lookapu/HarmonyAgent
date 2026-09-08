//! 代码写入前的统一候选验证。
//!
//! 所有文件修改工具先在内存中形成完整候选文本，再经过这里的语言门禁。当前 P0 在
//! 通用配平守卫之上为 ArkTS/TypeScript/JavaScript 提供真实语法树错误增量检查；其它
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
    let own = usize::from(node.is_error() || node.is_missing());
    let mut cursor = node.walk();
    own + node
        .children(&mut cursor)
        .map(syntax_error_count)
        .sum::<usize>()
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

fn strip_leading_java_annotation(line: &str) -> &str {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('@') {
        return trimmed;
    }
    let mut depth = 0_i32;
    let mut saw_paren = false;
    for (index, ch) in trimmed.char_indices() {
        match ch {
            '(' => {
                saw_paren = true;
                depth += 1;
            }
            ')' if depth > 0 => depth -= 1,
            c if c.is_whitespace() && (!saw_paren || depth == 0) => {
                return trimmed[index..].trim_start();
            }
            _ => {}
        }
    }
    ""
}

fn java_annotation_name(line: &str) -> Option<&'static str> {
    let token = line.trim_start().split(['(', ' ', '\t']).next()?;
    match token.rsplit('.').next()? {
        "@Override" | "Override" => Some("@Override"),
        "@Resource" | "Resource" => Some("@Resource"),
        _ => None,
    }
}

fn contains_java_type_declaration(candidate: &str) -> bool {
    ["class", "interface", "record", "enum"]
        .iter()
        .any(|keyword| {
            candidate
                .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
                .any(|word| word == *keyword)
        })
}

fn looks_like_java_declaration(annotation: &str, candidate: &str) -> bool {
    let candidate = candidate.trim();
    if candidate.is_empty() || candidate.starts_with('}') {
        return false;
    }
    if annotation == "@Resource" && contains_java_type_declaration(candidate) {
        return true;
    }
    if let Some(open) = candidate.find('(') {
        let prefix = candidate[..open].trim();
        let has_declaration_prefix = prefix.split_whitespace().count() >= 2;
        return has_declaration_prefix && (candidate.contains('{') || candidate.contains(';'));
    }
    let field_prefix = candidate.split('=').next().unwrap_or_default();
    annotation == "@Resource"
        && candidate.contains(';')
        && !candidate.contains('(')
        && field_prefix.split_whitespace().count() >= 2
}

/// Java parser/JDT 不可用时的 fail-before-write 补强：至少阻止删除声明时遗留最常见的
/// `@Override` / `@Resource`。只比较新增违规数，允许修复原本已损坏的文件。
fn java_orphan_annotation_count(text: &str) -> usize {
    let lines = text.lines().collect::<Vec<_>>();
    let mut violations = 0;
    for (index, line) in lines.iter().enumerate() {
        let Some(annotation) = java_annotation_name(line) else {
            continue;
        };
        let mut candidate = strip_leading_java_annotation(line).to_string();
        for next in lines.iter().skip(index + 1).take(12) {
            let trimmed = next.trim();
            if trimmed.is_empty()
                || trimmed.starts_with("//")
                || trimmed.starts_with("/*")
                || trimmed.starts_with('*')
            {
                continue;
            }
            if trimmed.starts_with('@') {
                let remainder = strip_leading_java_annotation(trimmed);
                if !remainder.is_empty() {
                    candidate.push(' ');
                    candidate.push_str(remainder);
                }
                continue;
            }
            candidate.push(' ');
            candidate.push_str(trimmed);
            if trimmed.starts_with('}') || trimmed.contains('{') || trimmed.contains(';') {
                break;
            }
        }
        if !looks_like_java_declaration(annotation, &candidate) {
            violations += 1;
        }
    }
    violations
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
        let before_errors = java_orphan_annotation_count(before);
        let after_errors = java_orphan_annotation_count(after);
        if after_errors > before_errors {
            return Err(format!(
                "代码修改事务被 Java 声明门禁拒绝：{} 中游离的 @Override/@Resource 注解由 {} 增至 {}。候选内容未落盘；删除字段或方法时必须连同其注解一起修改。",
                path.display(), before_errors, after_errors
            ));
        }
        return Ok(MutationGuardReport {
            parser: "java_declaration_guard",
            before_errors,
            after_errors,
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
        assert_eq!(report.parser, "java_declaration_guard");
        assert_eq!(report.after_errors, 0);
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
