//! ArkTS 编译错误的 API 证据映射。
//!
//! 将构建器给出的类型/API 错误关联到当前工程 API Level、本机 SDK 声明文件和
//! 官方 API 变更库。这里只生成只读证据与恢复建议，不根据模糊命中自动修改源码。

use std::collections::BTreeSet;
use std::fs;

use rusqlite::Connection;
use serde::Serialize;

use crate::services::harmony::BuildError;
use crate::services::sdk_api::{ApiIndex, ProjectApiContext};

#[derive(Debug, Clone, Serialize)]
pub struct ArktsApiMapping {
    pub error_index: usize,
    pub kind: String,
    pub confidence: f32,
    pub terms: Vec<String>,
    pub evidence: Vec<String>,
    pub recovery_steps: Vec<String>,
}

/// 将 ArkTS 的 API/类型错误映射到本机声明和官方变更记录。
pub fn map_errors(
    errors: &[BuildError],
    context: &ProjectApiContext,
    index: Option<&ApiIndex>,
    official_db: Option<&Connection>,
) -> Vec<ArktsApiMapping> {
    errors
        .iter()
        .enumerate()
        .filter(|(_, error)| error.kind == "arkts" && is_api_diagnostic(error))
        .filter_map(|(error_index, error)| {
            map_error(error_index, error, context, index, official_db)
        })
        .collect()
}

fn map_error(
    error_index: usize,
    error: &BuildError,
    context: &ProjectApiContext,
    index: Option<&ApiIndex>,
    official_db: Option<&Connection>,
) -> Option<ArktsApiMapping> {
    let terms = diagnostic_terms(&error.message);
    if terms.is_empty() {
        return None;
    }

    let mut evidence = Vec::new();
    let mut recovery_steps = BTreeSet::new();
    let mut has_local_definition = false;
    let mut has_official_change = false;

    if is_type_constraint(&error.message) {
        evidence.push(format!(
            "[类型约束] 编译器涉及类型/API：{}；必须以声明签名为准，不能用类型断言掩盖不兼容",
            terms.join(", ")
        ));
        recovery_steps.insert("按本机 .d.ts 的参数、返回值和泛型约束修正调用，再重新构建".into());
    }

    if let Some(index) = index {
        for module in &index.modules {
            let module_match = terms
                .iter()
                .any(|term| module_matches(&module.module, term));
            let symbols = module
                .symbols
                .iter()
                .filter(|symbol| {
                    terms
                        .iter()
                        .any(|term| symbol.name.eq_ignore_ascii_case(term))
                })
                .take(3)
                .collect::<Vec<_>>();
            if !module_match && symbols.is_empty() {
                continue;
            }
            has_local_definition = true;
            if symbols.is_empty() {
                evidence.push(format!(
                    "[本机官方定义] {} | {} | 声明文件 {}",
                    module.module,
                    context.availability(module.since_min, module.deprecated),
                    module.path
                ));
            } else {
                for symbol in symbols {
                    let definition = definition_line(&module.path, &symbol.name)
                        .unwrap_or_else(|| format!("{} {}", symbol.kind, symbol.name));
                    evidence.push(format!(
                        "[本机官方定义] {}::{} | since API {} | {} | {}{}",
                        module.module,
                        symbol.name,
                        symbol
                            .since
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "?".into()),
                        context.availability(symbol.since, symbol.deprecated),
                        definition,
                        symbol
                            .replacement
                            .as_deref()
                            .map(|value| format!(" | 替代：{value}"))
                            .unwrap_or_default()
                    ));
                    if symbol.deprecated {
                        recovery_steps.insert(match symbol.replacement.as_deref() {
                            Some(replacement) => {
                                format!("将已废弃的 {} 替换为 {replacement}", symbol.name)
                            }
                            None => format!(
                                "核对 {} 的官方废弃说明后选择当前 SDK 中的替代 API",
                                symbol.name
                            ),
                        });
                    }
                    if symbol
                        .since
                        .zip(context.compile_api.or(context.installed_api))
                        .is_some_and(|(since, compile)| since > compile)
                    {
                        recovery_steps.insert(format!(
                            "{} 从 API {} 引入；改用当前编译 SDK 可用 API，或在产品确有要求时升级 compileSdkVersion",
                            symbol.name,
                            symbol.since.unwrap_or_default()
                        ));
                    } else if let Some((since, compatible)) =
                        symbol.since.zip(context.compatible_api)
                    {
                        if since > compatible {
                            recovery_steps.insert(format!(
                                "{} 高于 compatible API {}；增加显式 API Level 运行时守卫和低版本回退",
                                symbol.name, compatible
                            ));
                        }
                    }
                }
            }
            if evidence.len() >= 7 {
                break;
            }
        }
    }

    if let Some(conn) = official_db {
        let mut seen = BTreeSet::new();
        for term in terms.iter().filter(searchable_term).take(5) {
            let query = crate::services::harmony_api_diff::SearchQuery {
                keyword: Some(term.clone()),
                limit: Some(4),
                ..Default::default()
            };
            let Ok(entries) = crate::services::harmony_api_diff::search(conn, &query) else {
                continue;
            };
            for entry in entries
                .into_iter()
                .filter(|entry| official_entry_matches(entry, term))
                .take(2)
            {
                let key = format!(
                    "{}:{}:{}",
                    entry.module.as_deref().unwrap_or(""),
                    entry.api_name.as_deref().unwrap_or(""),
                    entry.declaration
                );
                if !seen.insert(key) {
                    continue;
                }
                has_official_change = true;
                evidence.push(format!(
                    "[官方 API 变更] {} | {} API {} | {} | {}{}",
                    entry.change_type,
                    entry.version_label,
                    entry
                        .api_level
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "?".into()),
                    entry.declaration,
                    entry.source_url,
                    entry
                        .old_declaration
                        .as_deref()
                        .map(|value| format!(" | 旧定义：{value}"))
                        .unwrap_or_default()
                ));
                if evidence.len() >= 10 {
                    break;
                }
            }
            if evidence.len() >= 10 {
                break;
            }
        }
    }

    if !has_local_definition && !has_official_change && !is_type_constraint(&error.message) {
        return None;
    }
    if has_official_change {
        recovery_steps.insert("对照官方变更记录确认引入、修改或移除版本，再选择兼容声明".into());
    }
    if has_local_definition {
        recovery_steps
            .insert("以当前工程所用本机 SDK 声明为最终类型依据，修复后重新 build_project".into());
    }

    let kind = if has_official_change {
        "api_change"
    } else if is_type_constraint(&error.message) {
        "type_constraint"
    } else {
        "official_definition"
    };
    Some(ArktsApiMapping {
        error_index,
        kind: kind.into(),
        confidence: if has_local_definition && has_official_change {
            0.96
        } else if has_local_definition || has_official_change {
            0.9
        } else {
            0.78
        },
        terms,
        evidence,
        recovery_steps: recovery_steps.into_iter().collect(),
    })
}

fn is_type_constraint(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "not assignable",
        "does not exist on type",
        "argument of type",
        "type mismatch",
        "expected ",
        "no overload matches",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn is_api_diagnostic(error: &BuildError) -> bool {
    if matches!(error.category.as_str(), "type" | "api_level" | "dependency") {
        return true;
    }
    let lower = error.message.to_ascii_lowercase();
    [
        "cannot find name",
        "has no exported member",
        "cannot find module",
        "does not exist on type",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn searchable_term(term: &&String) -> bool {
    term.len() >= 3
        && !matches!(
            term.to_ascii_lowercase().as_str(),
            "any"
                | "boolean"
                | "never"
                | "null"
                | "number"
                | "object"
                | "string"
                | "undefined"
                | "unknown"
                | "void"
        )
}

fn official_entry_matches(entry: &crate::services::harmony_api_diff::ApiEntry, term: &str) -> bool {
    entry
        .api_name
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case(term))
        || entry
            .class_name
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(term))
        || entry
            .module
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(term))
        || contains_identifier(&entry.declaration, term)
}

fn contains_identifier(text: &str, identifier: &str) -> bool {
    text.match_indices(identifier).any(|(start, value)| {
        let before = text[..start].chars().next_back();
        let after = text[start + value.len()..].chars().next();
        !before.is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '$'))
            && !after.is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '$'))
    })
}

fn diagnostic_terms(message: &str) -> Vec<String> {
    let mut terms = BTreeSet::new();
    let mut quote = None;
    let mut start = 0;
    for (index, ch) in message.char_indices() {
        if matches!(ch, '\'' | '"' | '`') {
            if quote == Some(ch) {
                let value = message[start..index].trim();
                if valid_term(value) {
                    terms.insert(value.to_string());
                }
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
                start = index + ch.len_utf8();
            }
        }
    }
    for token in message.split(|ch: char| {
        ch.is_whitespace() || matches!(ch, '(' | ')' | '[' | ']' | ',' | ':' | ';')
    }) {
        let value = token.trim_matches(|ch: char| matches!(ch, '\'' | '"' | '`' | '.'));
        if (value.starts_with("@ohos.") || value.starts_with("@kit.")) && valid_term(value) {
            terms.insert(value.to_string());
        }
    }
    terms.into_iter().take(8).collect()
}

fn valid_term(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 120
        && value
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '$' | '.' | '@'))
}

fn module_matches(module: &str, term: &str) -> bool {
    module.eq_ignore_ascii_case(term)
        || term
            .strip_suffix(".d.ts")
            .is_some_and(|value| module.eq_ignore_ascii_case(value))
}

fn definition_line(path: &str, symbol: &str) -> Option<String> {
    fs::read_to_string(path).ok()?.lines().find_map(|line| {
        let trimmed = line.trim();
        (trimmed.contains(symbol)
            && [
                "class ",
                "interface ",
                "enum ",
                "function ",
                "type ",
                "const ",
            ]
            .iter()
            .any(|prefix| trimmed.contains(prefix)))
        .then(|| trimmed.chars().take(300).collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::sdk_api::{ApiModule, ApiSymbol};

    #[test]
    fn maps_type_error_to_local_definition_and_official_change() {
        let declaration =
            std::env::temp_dir().join(format!("harmony-api-diagnosis-{}.d.ts", std::process::id()));
        fs::write(&declaration, "export declare interface WantOptions {}\n").unwrap();
        let index = ApiIndex {
            modules: vec![ApiModule {
                module: "@ohos.app.ability.Want".into(),
                symbols: vec![ApiSymbol {
                    name: "WantOptions".into(),
                    kind: "interface".into(),
                    since: Some(14),
                    deprecated: false,
                    syscap: None,
                    permissions: Vec::new(),
                    replacement: None,
                }],
                path: declaration.to_string_lossy().into(),
                ..empty_module()
            }],
            ..Default::default()
        };
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE api_docs (id INTEGER PRIMARY KEY, kit TEXT NOT NULL, dts_file TEXT, module TEXT, class_name TEXT, declaration TEXT NOT NULL, api_name TEXT, change_type TEXT NOT NULL, version_label TEXT NOT NULL, api_level INTEGER, old_declaration TEXT, source_url TEXT NOT NULL, fetched_at INTEGER NOT NULL);\n
             INSERT INTO api_docs (kit,module,declaration,api_name,change_type,version_label,api_level,source_url,fetched_at) VALUES ('Ability Kit','@ohos.app.ability.Want','interface WantOptions','WantOptions','added','5.0.2(14)',14,'https://example.test/want',0);",
        )
        .unwrap();
        let errors = vec![BuildError {
            kind: "arkts".into(),
            category: "type".into(),
            message: "Type 'WantOptions' is not assignable to type 'string'".into(),
            ..empty_error()
        }];
        let context = ProjectApiContext {
            compile_api: Some(12),
            compatible_api: Some(12),
            ..Default::default()
        };

        let mapped = map_errors(&errors, &context, Some(&index), Some(&conn));
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].kind, "api_change");
        assert!(
            mapped[0]
                .evidence
                .iter()
                .any(|item| item.contains("不可用：高于当前编译 SDK"))
        );
        assert!(
            mapped[0]
                .evidence
                .iter()
                .any(|item| item.contains("官方 API 变更"))
        );
        fs::remove_file(declaration).ok();
    }

    #[test]
    fn retains_plain_type_constraint_without_api_hit() {
        let errors = vec![BuildError {
            kind: "arkts".into(),
            category: "type".into(),
            message: "Argument of type 'number' is not assignable to type 'string'".into(),
            ..empty_error()
        }];
        let mapped = map_errors(&errors, &ProjectApiContext::default(), None, None);
        assert_eq!(mapped[0].kind, "type_constraint");
        assert!(mapped[0].terms.contains(&"number".into()));
    }

    fn empty_module() -> ApiModule {
        ApiModule {
            module: String::new(),
            kit: None,
            syscap: None,
            system_capabilities: Vec::new(),
            permissions: Vec::new(),
            since_min: None,
            since_max: None,
            declarations: Vec::new(),
            symbols: Vec::new(),
            deprecated: false,
            path: String::new(),
        }
    }

    fn empty_error() -> BuildError {
        BuildError {
            kind: String::new(),
            category: String::new(),
            error_code: None,
            stage: "compile".into(),
            file: None,
            line: None,
            column: None,
            message: String::new(),
            suggestion: String::new(),
        }
    }
}
