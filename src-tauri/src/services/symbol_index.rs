//! 项目符号索引：扫描源码提取类/组件/函数/路由等符号，供 Agent 快速定位，减少盲读。
//!
//! 轻量实现：基于行级正则/关键字匹配，不做完整 AST 解析；覆盖 ArkTS/TS/JS/Rust/Python/Kotlin 等。
//! 结果可持久化到 project_index_cache 表（kind='symbols'），也可即时返回。

use std::collections::HashMap;
#[cfg(not(test))]
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
#[cfg(not(test))]
use std::sync::Arc;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use rusqlite::{params, params_from_iter, types::Value, Connection, TransactionBehavior};

use crate::services::harmony;

const SYMBOL_EXTS: &[&str] = &["ets", "ts", "tsx", "js", "jsx", "rs", "py", "kt", "java", "swift", "go", "cpp", "c", "h", "hpp"];

const SKIP_DIRS: &[&str] = &[
    "node_modules", ".git", "build", ".hvigor", "oh_modules", ".idea", "dist",
    ".cxx", ".preview", ".test", ".ohpm", ".arkui-x", "coverage", ".venv", "target",
];

const MAX_FILES: usize = 4000;
const MAX_BYTES: u64 = 512 * 1024;
const STRUCTURE_PARSER_VERSION: i64 = 11;
const MAX_REEXPORT_DEPTH: usize = 8;
const MAX_REEXPORT_BRANCHES: usize = 16;
const MAX_REEXPORT_VISITS: usize = 128;

/// 全库文件目录统计。目录覆盖所有未被忽略的普通文件；结构解析可以渐进完成。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct CatalogStats {
    /// SQLite 目录代次，用于在并发全量/增量写入之间做 fencing。
    #[serde(default)]
    pub revision: u64,
    pub discovered_files: usize,
    pub source_files: usize,
    pub indexed_source_files: usize,
    pub deferred_source_files: usize,
    pub oversized_source_files: usize,
    pub unsupported_files: usize,
    pub symlink_files: usize,
    pub unreadable_files: usize,
    pub unreadable_directories: usize,
    pub persisted: bool,
}

impl CatalogStats {
    fn coverage(&self) -> String {
        if self.deferred_source_files > 0 {
            format!(
                "partial_{}_source_files_deferred_by_parse_budget",
                self.deferred_source_files
            )
        } else if self.oversized_source_files > 0
            || self.unreadable_files > 0
            || self.unreadable_directories > 0
        {
            format!(
                "partial_{}_oversized_{}_unreadable_files_{}_unreadable_directories",
                self.oversized_source_files,
                self.unreadable_files,
                self.unreadable_directories,
            )
        } else {
            "best_effort_lightweight_syntax_index".into()
        }
    }
}

/// ArkTS 状态管理装饰器（属性声明/状态流转标记，鸿蒙工程定位数据流的关键符号）
const ETS_STATE_DECORATORS: &[&str] = &[
    "@State", "@Prop", "@Link", "@Provide", "@Consume", "@ObjectLink", "@Observed",
    "@Builder", "@Styles", "@Extend", "@StorageLink", "@StorageProp", "@Watch",
    "@LocalStorageLink", "@LocalStorageProp", "@Require",
];

/// 单个符号定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    /// 符号类型：component / class / interface / function / method / route / struct / enum / decorator
    pub kind: String,
    /// 符号名
    pub name: String,
    /// 相对项目根的文件路径
    pub file: String,
    /// 1-based 行号
    pub line: usize,
    /// 结构块结束行（1-based，含）；无法识别块时等于定义行。
    #[serde(default)]
    pub end_line: usize,
    /// 结构角色：entity（类/组件/类型/状态）或 logic（函数/方法）。
    #[serde(default)]
    pub role: String,
    /// 定义签名的单行摘要，不包含方法正文。
    #[serde(default)]
    pub signature: String,
    /// 所在类/组件（方法的归属，顶层为空）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Source language inferred from the file extension.
    #[serde(default)]
    pub language: String,
    /// Parser layer that produced this node: tree_sitter or lightweight.
    #[serde(default)]
    pub source_layer: String,
    /// Syntactically declared outgoing relationships; targets are resolved in a later layer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declared_relations: Vec<DeclaredRelation>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DeclaredRelation {
    pub kind: String,
    pub target_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_specifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_name: Option<String>,
}

/// 全局结构图中的关系边。空 target_file/0 target_line 表示语法目标尚未完成名称解析。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StructureEdge {
    pub kind: String,
    pub source_file: String,
    pub source_name: String,
    pub source_line: usize,
    pub target_file: String,
    pub target_name: String,
    pub target_line: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_module: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_imported_name: Option<String>,
}

fn structure_role(kind: &str) -> &'static str {
    if matches!(kind, "function" | "method") {
        "logic"
    } else {
        "entity"
    }
}

fn leading_indent(line: &str) -> usize {
    line.chars().take_while(|ch| ch.is_whitespace()).count()
}

/// 轻量结构块范围。这里保持容错和零额外依赖；Tree-sitter/LSP 接入后将作为 fallback。
fn structure_end_line(lines: &[&str], start: usize, ext: &str, kind: &str) -> usize {
    if matches!(kind, "decorator" | "route") {
        return start + 1;
    }
    if ext == "py" {
        let base = leading_indent(lines.get(start).copied().unwrap_or(""));
        let mut end = start;
        for (idx, line) in lines.iter().enumerate().skip(start + 1) {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if leading_indent(line) <= base {
                break;
            }
            end = idx;
        }
        return end + 1;
    }
    let mut found_open = false;
    let mut depth = 0i64;
    for (idx, line) in lines.iter().enumerate().skip(start) {
        for ch in line.chars() {
            match ch {
                '{' => {
                    found_open = true;
                    depth += 1;
                }
                '}' if found_open => {
                    depth -= 1;
                    if depth == 0 {
                        return idx + 1;
                    }
                }
                _ => {}
            }
        }
        // 声明没有块体时不要吞掉后续定义。
        if !found_open && line.trim_end().ends_with(';') {
            return start + 1;
        }
    }
    start + 1
}

fn make_symbol(
    kind: &str,
    name: String,
    rel: &str,
    line: usize,
    parent: Option<String>,
    raw: &str,
    lines: &[&str],
    ext: &str,
) -> Symbol {
    Symbol {
        kind: kind.into(),
        name,
        file: rel.into(),
        line,
        end_line: structure_end_line(lines, line.saturating_sub(1), ext, kind),
        role: structure_role(kind).into(),
        signature: raw.trim().chars().take(300).collect(),
        parent,
        language: ext.to_string(),
        source_layer: "lightweight".into(),
        declared_relations: Vec::new(),
    }
}

fn safe_rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
        .replace('\\', "/")
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '$'
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// 从一行中取紧跟关键字之后的标识符
fn ident_after(line: &str, kw: &str) -> Option<String> {
    let mut search = line;
    // 处理 export/default/declare 等前缀
    for pre in ["export ", "default ", "declare ", "pub ", "async "] {
        if let Some(rest) = search.strip_prefix(pre) {
            search = rest;
        }
    }
    let rest = search.strip_prefix(kw)?;
    let mut chars = rest.chars().skip_while(|c| c.is_whitespace());
    let first = chars.next()?;
    if !is_ident_start(first) {
        return None;
    }
    let mut name = String::new();
    name.push(first);
    for c in chars {
        if is_ident(c) {
            name.push(c);
        } else {
            break;
        }
    }
    if name.is_empty() { None } else { Some(name) }
}

fn tree_sitter_language(ext: &str) -> Option<tree_sitter::Language> {
    match ext {
        "ets" => Some(tree_sitter_arkts::LANGUAGE.into()),
        "ts" | "js" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" | "jsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        _ => None,
    }
}

fn node_text(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    node.utf8_text(source).ok().map(str::to_string)
}

fn tree_sitter_signature(node: tree_sitter::Node<'_>, source: &str) -> String {
    source
        .lines()
        .nth(node.start_position().row)
        .unwrap_or("")
        .trim()
        .chars()
        .take(300)
        .collect()
}

fn collect_declared_relations(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    out: &mut Vec<DeclaredRelation>,
) {
    let relation_kind = match node.kind() {
        "extends_clause" | "extends_type_clause" => Some("extends"),
        "implements_clause" => Some("implements"),
        _ => None,
    };
    if let Some(kind) = relation_kind {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "type_arguments" {
                continue;
            }
            if let Some(target_name) = node_text(child, source)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
            {
                out.push(DeclaredRelation {
                    kind: kind.into(),
                    target_name,
                    module_specifier: None,
                    imported_name: None,
                });
            }
        }
        return;
    }
    if matches!(node.kind(), "class_heritage") {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            collect_declared_relations(child, source, out);
        }
    }
}

fn collect_direct_calls(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    root: bool,
    out: &mut Vec<DeclaredRelation>,
) {
    if !root
        && matches!(
            node.kind(),
            "function_declaration"
                | "generator_function_declaration"
                | "method_definition"
                | "arrow_function"
                | "function_expression"
        )
    {
        return;
    }
    if node.kind() == "call_expression" {
        if let Some(target_name) = node
            .child_by_field_name("function")
            .filter(|function| function.kind() == "identifier")
            .and_then(|function| node_text(function, source))
        {
            out.push(DeclaredRelation {
                kind: "calls".into(),
                target_name,
                module_specifier: None,
                imported_name: None,
            });
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_direct_calls(child, source, false, out);
    }
}

#[derive(Debug, Clone)]
struct NamedImport {
    module_specifier: String,
    imported_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModuleReexport {
    exported_name: String,
    target_module: String,
    imported_name: String,
}

fn string_literal_value(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let value = node_text(node, source)?;
    let quote = value.chars().next()?;
    matches!(quote, '\'' | '"')
        .then(|| value.strip_prefix(quote)?.strip_suffix(quote).map(str::to_string))
        .flatten()
}

fn collect_named_import_specifiers(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    module_specifier: &str,
    imports: &mut HashMap<String, Vec<NamedImport>>,
) {
    if node.kind() == "import_specifier" {
        let Some(name_node) = node.child_by_field_name("name") else { return };
        let Some(imported_name) = node_text(name_node, source) else { return };
        let local_name = node
            .child_by_field_name("alias")
            .and_then(|alias| node_text(alias, source))
            .unwrap_or_else(|| imported_name.clone());
        imports.entry(local_name).or_default().push(NamedImport {
            module_specifier: module_specifier.to_string(),
            imported_name,
        });
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_named_import_specifiers(child, source, module_specifier, imports);
    }
}

fn named_imports(root: tree_sitter::Node<'_>, source: &[u8]) -> HashMap<String, Vec<NamedImport>> {
    let mut imports = HashMap::new();
    let mut cursor = root.walk();
    for statement in root.named_children(&mut cursor) {
        if statement.kind() != "import_statement" {
            continue;
        }
        let import_node = statement
            .named_child(0)
            .filter(|child| child.kind() == "lazy_import_statement")
            .unwrap_or(statement);
        let Some(module_specifier) = import_node
            .child_by_field_name("source")
            .and_then(|source_node| string_literal_value(source_node, source))
        else {
            continue;
        };
        collect_named_import_specifiers(import_node, source, &module_specifier, &mut imports);
    }
    imports
}

fn collect_export_specifiers(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    target_module: &str,
    out: &mut Vec<ModuleReexport>,
) {
    if node.kind() == "export_specifier" {
        let Some(name) = node
            .child_by_field_name("name")
            .and_then(|value| node_text(value, source))
        else {
            return;
        };
        let exported_name = node
            .child_by_field_name("alias")
            .and_then(|value| node_text(value, source))
            .unwrap_or_else(|| name.clone());
        out.push(ModuleReexport {
            exported_name,
            target_module: target_module.to_string(),
            imported_name: name,
        });
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_export_specifiers(child, source, target_module, out);
    }
}

fn parse_module_reexports(content: &str, ext: &str) -> Vec<ModuleReexport> {
    let Some(language) = tree_sitter_language(ext) else {
        return Vec::new();
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(content, None) else {
        return Vec::new();
    };
    let source = content.as_bytes();
    let mut out = Vec::new();
    let mut cursor = tree.root_node().walk();
    for statement in tree.root_node().named_children(&mut cursor) {
        if statement.kind() != "export_statement" {
            continue;
        }
        let Some(target_module) = statement
            .child_by_field_name("source")
            .and_then(|value| string_literal_value(value, source))
        else {
            continue;
        };
        let before = out.len();
        collect_export_specifiers(statement, source, &target_module, &mut out);
        if out.len() == before {
            let mut children = statement.walk();
            let namespace_export = statement
                .named_children(&mut children)
                .any(|child| child.kind() == "namespace_export");
            let mut raw_children = statement.walk();
            let star_export = statement
                .children(&mut raw_children)
                .any(|child| child.kind() == "*");
            if star_export && !namespace_export {
                out.push(ModuleReexport {
                    exported_name: "*".into(),
                    target_module,
                    imported_name: "*".into(),
                });
            }
        }
    }
    out.sort_by(|a, b| {
        (&a.exported_name, &a.target_module, &a.imported_name).cmp(&(
            &b.exported_name,
            &b.target_module,
            &b.imported_name,
        ))
    });
    out.dedup();
    out
}

fn relation_local_identifier(value: &str) -> Option<&str> {
    let identifier = value.split('<').next()?.trim();
    let mut chars = identifier.chars();
    is_ident_start(chars.next()?)
        .then(|| chars.all(is_ident))
        .filter(|valid| *valid)
        .map(|_| identifier)
}

fn declared_relations(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    imports: &HashMap<String, Vec<NamedImport>>,
    include_calls: bool,
) -> Vec<DeclaredRelation> {
    let mut relations = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "class_heritage" | "extends_type_clause") {
            collect_declared_relations(child, source, &mut relations);
        }
    }
    if include_calls {
        collect_direct_calls(node, source, true, &mut relations);
    }
    for relation in &mut relations {
        let Some(local_name) = relation_local_identifier(&relation.target_name) else { continue };
        let Some(bindings) = imports.get(local_name).filter(|bindings| bindings.len() == 1) else {
            continue;
        };
        relation.module_specifier = Some(bindings[0].module_specifier.clone());
        relation.imported_name = Some(bindings[0].imported_name.clone());
    }
    relations.sort();
    relations.dedup();
    relations
}

fn push_tree_sitter_symbol(
    out: &mut Vec<Symbol>,
    node: tree_sitter::Node<'_>,
    name_node: tree_sitter::Node<'_>,
    kind: &str,
    parent: Option<String>,
    rel: &str,
    ext: &str,
    source: &str,
    declared_relations: Vec<DeclaredRelation>,
) -> Option<String> {
    let name = node_text(name_node, source.as_bytes())?;
    // Decorators are part of a declaration node in TypeScript/ArkTS, so the
    // declaration itself starts at the name/keyword line rather than @Entry.
    let line = name_node.start_position().row + 1;
    out.push(Symbol {
        kind: kind.into(),
        name: name.clone(),
        file: rel.into(),
        line,
        end_line: (node.end_position().row + 1).max(line),
        role: structure_role(kind).into(),
        signature: source
            .lines()
            .nth(name_node.start_position().row)
            .unwrap_or("")
            .trim()
            .chars()
            .take(300)
            .collect(),
        parent,
        language: ext.into(),
        source_layer: "tree_sitter".into(),
        declared_relations,
    });
    Some(name)
}

fn walk_syntax_tree(
    node: tree_sitter::Node<'_>,
    parent: Option<&str>,
    rel: &str,
    ext: &str,
    source: &str,
    imports: &HashMap<String, Vec<NamedImport>>,
    out: &mut Vec<Symbol>,
) {
    let node_kind = node.kind();
    let declaration_kind = match node_kind {
        "class_declaration" | "abstract_class_declaration" => Some("class"),
        "struct_declaration" => Some("component"),
        "interface_declaration" => Some("interface"),
        "type_alias_declaration" => Some("type"),
        "enum_declaration" => Some("enum"),
        "function_declaration" | "generator_function_declaration" => Some("function"),
        "method_definition" | "method_signature" | "abstract_method_signature" => Some("method"),
        _ => None,
    };
    let mut child_parent = parent.map(str::to_string);
    if node_kind == "decorator" && ext == "ets" {
        if let Some(raw) = node_text(node, source.as_bytes()) {
            let name = raw
                .trim_start()
                .strip_prefix('@')
                .and_then(|value| value.split(|ch: char| !is_ident(ch)).next())
                .filter(|value| !value.is_empty())
                .map(|value| format!("@{value}"));
            if let Some(name) = name.filter(|value| {
                matches!(value.as_str(), "@Entry" | "@Component" | "@Router")
                    || ETS_STATE_DECORATORS.contains(&value.as_str())
            }) {
                let line = node.start_position().row + 1;
                out.push(Symbol {
                    kind: if name == "@Router" { "route" } else { "decorator" }.into(),
                    name,
                    file: rel.into(),
                    line,
                    end_line: (node.end_position().row + 1).max(line),
                    role: "entity".into(),
                    signature: tree_sitter_signature(node, source),
                    parent: parent.map(str::to_string),
                    language: ext.into(),
                    source_layer: "tree_sitter".into(),
                    declared_relations: Vec::new(),
                });
            }
        }
    } else if let (Some(kind), Some(name_node)) = (declaration_kind, node.child_by_field_name("name")) {
        let symbol_parent = matches!(kind, "function" | "method")
            .then(|| parent.map(str::to_string))
            .flatten();
        if let Some(name) = push_tree_sitter_symbol(
            out,
            node,
            name_node,
            kind,
            symbol_parent,
            rel,
            ext,
            source,
            declared_relations(
                node,
                source.as_bytes(),
                imports,
                matches!(kind, "function" | "method"),
            ),
        ) {
            if matches!(kind, "class" | "component" | "interface" | "type" | "enum") {
                child_parent = Some(name);
            }
        }
    } else if node_kind == "variable_declarator"
        && node
            .child_by_field_name("value")
            .is_some_and(|value| matches!(value.kind(), "arrow_function" | "function_expression"))
    {
        if let Some(name_node) = node.child_by_field_name("name") {
            let relations = node
                .child_by_field_name("value")
                .map(|value| declared_relations(value, source.as_bytes(), imports, true))
                .unwrap_or_default();
            let _ = push_tree_sitter_symbol(
                out,
                node,
                name_node,
                "function",
                parent.map(str::to_string),
                rel,
                ext,
                source,
                relations,
            );
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_syntax_tree(child, child_parent.as_deref(), rel, ext, source, imports, out);
    }
}

/// Returns true only when a supported grammar produced an error-free syntax tree.
fn scan_file_tree_sitter(content: &str, rel: &str, ext: &str, out: &mut Vec<Symbol>) -> bool {
    let Some(language) = tree_sitter_language(ext) else { return false };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return false;
    }
    let Some(tree) = parser.parse(content, None) else { return false };
    if tree.root_node().has_error() {
        return false;
    }
    let imports = named_imports(tree.root_node(), content.as_bytes());
    walk_syntax_tree(tree.root_node(), None, rel, ext, content, &imports, out);
    true
}

/// 解析单个源文件中的符号
fn scan_file(path: &Path, rel: &str, out: &mut Vec<Symbol>) {
    let meta = match fs::metadata(path) {
        Ok(m) if m.len() <= MAX_BYTES => m,
        _ => return,
    };
    let _ = meta;
    let content = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return,
    };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if scan_file_tree_sitter(&content, rel, ext, out) {
        return;
    }
    let lines: Vec<&str> = content.lines().collect();
    let mut current_parent: Option<String> = None;
    let mut brace_depth = 0i32;
    let class_like = ["class ", "interface ", "struct ", "enum ", "object ", "trait ", "impl "];

    for (idx, raw) in lines.iter().copied().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('*') || line.starts_with("/*") {
            // 仍需统计大括号（注释里的括号近似忽略，足够符号提取使用）
            continue;
        }
        let lineno = idx + 1;

        // ArkTS/HarmonyOS 装饰器：入口/组件/路由 + 状态管理装饰器
        if ext == "ets" && line.starts_with('@') {
            if line.starts_with("@Entry") {
                out.push(make_symbol("decorator", "@Entry".into(), rel, lineno, None, raw, &lines, ext));
            }
            if line.starts_with("@Component") {
                out.push(make_symbol("decorator", "@Component".into(), rel, lineno, None, raw, &lines, ext));
            }
            if line.starts_with("@Router") {
                out.push(make_symbol("route", "@Router".into(), rel, lineno, None, raw, &lines, ext));
            }
            // 状态管理装饰器：仅当装饰器名后是空白/(/) 等边界符时计入，
            // 避免把 "@StateXxx" 这类普通标识符误报为装饰器
            for dec in ETS_STATE_DECORATORS {
                if let Some(rest) = line.strip_prefix(*dec) {
                    if rest.chars().next().is_none_or(|c| !is_ident(c)) {
                        out.push(make_symbol("decorator", (*dec).into(), rel, lineno, None, raw, &lines, ext));
                    }
                    break;
                }
            }
        }

        // 类型定义
        for kw in &class_like {
            if let Some(name) = ident_after(line, kw) {
                let kind = kw.trim();
                out.push(make_symbol(kind, name.clone(), rel, lineno, None, raw, &lines, ext));
                current_parent = Some(name);
                break;
            }
        }

        // 函数/方法
        let fn_kw = if ext == "py" { "def " } else { "fn " };
        if let Some(name) = ident_after(line, fn_kw) {
            out.push(make_symbol("function", name, rel, lineno, current_parent.clone(), raw, &lines, ext));
        }
        if let Some(name) = ident_after(line, "function ") {
            out.push(make_symbol("function", name, rel, lineno, current_parent.clone(), raw, &lines, ext));
        }
        // ArkTS 组件 struct
        if ext == "ets" {
            if let Some(name) = ident_after(line, "struct ") {
                out.push(make_symbol("component", name, rel, lineno, None, raw, &lines, ext));
            }
            // 方法形似 name(...) {
            if line.contains('(') && line.ends_with('{') {
                let first = line.split('(').next().unwrap_or("").trim();
                let name = first.split_whitespace().last().unwrap_or("");
                if !name.is_empty()
                    && is_ident_start(name.chars().next().unwrap_or(' '))
                    && !["if", "for", "while", "switch", "catch", "when", "return", "else"].contains(&name)
                {
                    out.push(make_symbol("method", name.to_string(), rel, lineno, current_parent.clone(), raw, &lines, ext));
                }
            }
        }

        // 简易括号深度，用于离开 class 作用域后清空 parent
        for c in line.chars() {
            if c == '{' {
                brace_depth += 1;
            } else if c == '}' {
                brace_depth -= 1;
                if brace_depth <= 0 {
                    brace_depth = 0;
                    current_parent = None;
                }
            }
        }
    }
}

/// 文件指纹：mtime（Unix 纳秒，NTFS 精度 100ns，可察觉同秒内改写）+ 字节数。
/// 纳秒纪元在 u64 内可表示到 2554 年，截断安全；ext4 等秒级文件系统自动退化为秒+长度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct FileStamp {
    mtime: u64,
    len: u64,
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    Some(stamp_from_meta(&meta))
}

fn stamp_from_meta(meta: &fs::Metadata) -> FileStamp {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    FileStamp { mtime, len: meta.len() }
}

pub(crate) fn catalog_file_at(dir: &Path, root: &Path) -> PathBuf {
    let key = canonical_key(root);
    dir.join("repo_catalog")
        .join(format!("{:016x}.sqlite3", stable_hash(&key)))
}

/// Import a compiler-produced SCIP index into an independent precise-reference layer.
/// The existing active generation remains queryable until the new generation is complete.
pub(crate) fn import_scip_index(root: &Path, index_path: Option<&Path>) -> Result<crate::services::scip_index::ScipImportStats, String> {
    // Ensure the file catalog and syntax symbols exist before validating SCIP document stamps.
    let _ = index_project_cached(root);
    let data_dir = DATA_DIR.get().ok_or("结构索引数据目录尚未初始化")?;
    let index = index_path.map(Path::to_path_buf).unwrap_or_else(|| {
        let conventional = root.join("index.scip");
        if conventional.is_file() {
            conventional
        } else {
            root.join(".scip").join("index.scip")
        }
    });
    crate::services::scip_index::import(root, &catalog_file_at(data_dir, root), &index)
}

#[derive(Debug, Serialize)]
pub struct CatalogFile {
    pub path: String,
    pub extension: String,
    pub size: u64,
    pub state: String,
    pub shard: String,
}

#[derive(Debug, Serialize)]
pub struct CatalogQueryResult {
    pub items: Vec<CatalogFile>,
    pub total_matches: usize,
    pub page: usize,
    pub page_size: usize,
    pub next_page: Option<usize>,
}

fn glob_to_sql_like(pattern: &str) -> String {
    let mut out = String::new();
    for ch in pattern.replace('\\', "/").chars() {
        match ch {
            '*' => out.push('%'),
            '?' => out.push('_'),
            '%' | '_' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

/// 查询持久化全库目录。None 表示应用数据目录尚未初始化，调用方可回退即时扫描。
pub fn query_catalog_files(
    root: &Path,
    pattern: &str,
    prefix: Option<&str>,
    state: Option<&str>,
    page: usize,
    page_size: usize,
) -> Option<Result<CatalogQueryResult, String>> {
    let data_dir = DATA_DIR.get()?;
    let _ = index_project_cached(root);
    let conn = match Connection::open(catalog_file_at(data_dir, root)) {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("打开全库目录失败：{error}"))),
    };
    let page = page.max(1);
    let page_size = page_size.clamp(1, 200);
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    let like = glob_to_sql_like(pattern);
    let basename_like = format!("%/{like}");
    let prefix_like = prefix
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != ".")
        .map(|value| format!("{}/%", value.trim_matches('/')))
        .unwrap_or_else(|| "%".into());
    let state = state.map(str::trim).filter(|value| !value.is_empty()).unwrap_or("%");
    let where_sql = "(path LIKE ?1 ESCAPE '\\' COLLATE NOCASE
                      OR path LIKE ?2 ESCAPE '\\' COLLATE NOCASE)
                     AND path LIKE ?3 ESCAPE '\\' COLLATE NOCASE
                     AND state LIKE ?4 COLLATE NOCASE";
    let total_matches = match conn.query_row(
        &format!("SELECT COUNT(*) FROM files WHERE {where_sql}"),
        params![like, basename_like, prefix_like, state],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(value) => value.max(0) as usize,
        Err(error) => return Some(Err(format!("查询全库目录失败：{error}"))),
    };
    let mut stmt = match conn.prepare(&format!(
        "SELECT path, extension, size, state, shard FROM files
         WHERE {where_sql} ORDER BY path LIMIT ?5 OFFSET ?6"
    )) {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("准备全库目录查询失败：{error}"))),
    };
    let rows = match stmt.query_map(
        params![like, basename_like, prefix_like, state, page_size as i64, offset as i64],
        |row| {
            Ok(CatalogFile {
                path: row.get(0)?,
                extension: row.get(1)?,
                size: row.get::<_, i64>(2)?.max(0) as u64,
                state: row.get(3)?,
                shard: row.get(4)?,
            })
        },
    ) {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("读取全库目录失败：{error}"))),
    };
    let items = match rows.collect::<Result<Vec<_>, _>>() {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("解析全库目录结果失败：{error}"))),
    };
    Some(Ok(CatalogQueryResult {
        items,
        total_matches,
        page,
        page_size,
        next_page: (offset.saturating_add(page_size) < total_matches).then_some(page + 1),
    }))
}

fn shard_for(rel: &str) -> &str {
    rel.split('/').next().filter(|value| !value.is_empty()).unwrap_or(".")
}

fn ignored_catalog_path(rel: &str) -> bool {
    let parts = rel
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    parts.iter().enumerate().any(|(index, part)| {
        SKIP_DIRS.contains(part) || (index + 1 < parts.len() && part.starts_with('.'))
    })
}

/// 把 watcher/文件工具传入的路径规范为项目内相对路径。允许已删除路径，拒绝 `..` 越界。
fn normalize_changed_path(root: &Path, value: &str) -> Option<(String, PathBuf)> {
    let input = Path::new(value);
    let rel_path = if input.is_absolute() {
        input
            .strip_prefix(root)
            .ok()
            .map(Path::to_path_buf)
            .or_else(|| {
                let canonical_root = root.canonicalize().ok()?;
                input
                    .strip_prefix(canonical_root)
                    .ok()
                    .map(Path::to_path_buf)
            })?
    } else {
        input.to_path_buf()
    };
    if rel_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return None;
    }
    let rel = rel_path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if rel.is_empty() || ignored_catalog_path(&rel) {
        return None;
    }
    Some((rel.clone(), root.join(rel)))
}

#[derive(Debug)]
enum CatalogDelta {
    Updated(CatalogStats),
    NeedsReconciliation,
}

fn catalog_stats(conn: &Connection) -> rusqlite::Result<CatalogStats> {
    let mut stats = CatalogStats {
        persisted: true,
        ..CatalogStats::default()
    };
    let mut statement = conn.prepare("SELECT state, COUNT(*) FROM files GROUP BY state")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?.max(0) as usize,
        ))
    })?;
    for row in rows {
        let (state, count) = row?;
        stats.discovered_files += count;
        match state.as_str() {
            "indexed" => {
                stats.source_files += count;
                stats.indexed_source_files += count;
            }
            "deferred" => {
                stats.source_files += count;
                stats.deferred_source_files += count;
            }
            "oversized" => {
                stats.source_files += count;
                stats.oversized_source_files += count;
            }
            "symlink" => stats.symlink_files += count,
            "unreadable" => stats.unreadable_files += count,
            _ => stats.unsupported_files += count,
        }
    }
    drop(statement);
    stats.revision = conn
        .query_row("SELECT COALESCE(MAX(generation), 0) FROM files", [], |row| {
            row.get::<_, i64>(0)
        })?
        .max(0) as u64;
    Ok(stats)
}

/// 直接把文件级变化合并进持久化目录。目录创建/修改无法仅靠单条事件获知其子树，要求回退扫描。
fn apply_catalog_changes_at(root: &Path, data_dir: &Path, rels: &[String]) -> CatalogDelta {
    let path = catalog_file_at(data_dir, root);
    if !path.is_file() {
        return CatalogDelta::NeedsReconciliation;
    }
    let mut conn = match Connection::open(path) {
        Ok(value) => value,
        Err(_) => return CatalogDelta::NeedsReconciliation,
    };
    let transaction = match conn.transaction() {
        Ok(value) => value,
        Err(_) => return CatalogDelta::NeedsReconciliation,
    };
    let generation = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0);

    for value in rels {
        let Some((rel, abs)) = normalize_changed_path(root, value) else {
            continue;
        };
        let metadata = match fs::symlink_metadata(&abs) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if transaction
                    .execute(
                        "DELETE FROM files
                         WHERE path = ?1 OR substr(path, 1, length(?1) + 1) = ?1 || '/'",
                        params![rel],
                    )
                    .is_err()
                {
                    return CatalogDelta::NeedsReconciliation;
                }
                continue;
            }
            Err(_) => return CatalogDelta::NeedsReconciliation,
        };
        if metadata.is_dir() {
            return CatalogDelta::NeedsReconciliation;
        }
        let ext = abs
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let stamp = stamp_from_meta(&metadata);
        let state = if metadata.file_type().is_symlink() {
            "symlink"
        } else if !metadata.is_file() || !SYMBOL_EXTS.contains(&ext) {
            "unsupported"
        } else if metadata.len() > MAX_BYTES {
            "oversized"
        } else {
            "indexed"
        };
        if transaction
            .execute(
                "INSERT INTO files(path, extension, size, mtime_ns, state, shard, generation)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(path) DO UPDATE SET
                   extension=excluded.extension, size=excluded.size, mtime_ns=excluded.mtime_ns,
                   state=CASE
                     WHEN files.state='indexed' AND excluded.state='deferred'
                          AND files.size=excluded.size AND files.mtime_ns=excluded.mtime_ns
                     THEN files.state ELSE excluded.state END,
                   shard=excluded.shard, generation=excluded.generation",
                params![
                    rel,
                    ext,
                    stamp.len as i64,
                    stamp.mtime as i64,
                    state,
                    shard_for(&rel),
                    generation
                ],
            )
            .is_err()
        {
            return CatalogDelta::NeedsReconciliation;
        }
    }
    let stats = match catalog_stats(&transaction) {
        Ok(value) => value,
        Err(_) => return CatalogDelta::NeedsReconciliation,
    };
    if transaction.commit().is_err() {
        return CatalogDelta::NeedsReconciliation;
    }
    CatalogDelta::Updated(stats)
}
fn apply_catalog_changes(root: &Path, rels: &[String]) -> CatalogDelta {
    let Some(data_dir) = DATA_DIR.get() else {
        return CatalogDelta::NeedsReconciliation;
    };
    apply_catalog_changes_at(root, data_dir, rels)
}

fn insert_symbol_row(transaction: &rusqlite::Transaction<'_>, symbol: &Symbol) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO symbols(file, kind, name, line, end_line, role, signature, parent, shard, language, source_layer, declared_relations)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            symbol.file,
            symbol.kind,
            symbol.name,
            symbol.line as i64,
            symbol.end_line as i64,
            symbol.role,
            symbol.signature,
            symbol.parent,
            shard_for(&symbol.file),
            symbol.language,
            symbol.source_layer,
            serde_json::to_string(&symbol.declared_relations).unwrap_or_else(|_| "[]".into()),
        ],
    )?;
    Ok(())
}

fn containment_edges(symbols: &[Symbol]) -> Vec<StructureEdge> {
    let mut parents: HashMap<(&str, &str), Vec<&Symbol>> = HashMap::new();
    for symbol in symbols.iter().filter(|symbol| symbol.role == "entity") {
        parents
            .entry((&symbol.file, &symbol.name))
            .or_default()
            .push(symbol);
    }
    for candidates in parents.values_mut() {
        candidates.sort_by_key(|symbol| symbol.line);
    }
    let mut edges = Vec::new();
    for child in symbols {
        let Some(parent_name) = child.parent.as_deref() else { continue };
        let Some(candidates) = parents.get(&(child.file.as_str(), parent_name)) else { continue };
        let Some(parent) = candidates.iter().rev().find(|parent| parent.line <= child.line) else {
            continue;
        };
        if parent.line == child.line && parent.name == child.name {
            continue;
        }
        edges.push(StructureEdge {
            kind: "contains".into(),
            source_file: parent.file.clone(),
            source_name: parent.name.clone(),
            source_line: parent.line,
            target_file: child.file.clone(),
            target_name: child.name.clone(),
            target_line: child.line,
            target_module: None,
            target_imported_name: None,
        });
    }
    edges.sort();
    edges.dedup();
    edges
}

fn structure_edges(symbols: &[Symbol]) -> Vec<StructureEdge> {
    let mut edges = containment_edges(symbols);
    let mut local_targets: HashMap<(&str, &str), Vec<&Symbol>> = HashMap::new();
    for candidate in symbols {
        local_targets
            .entry((&candidate.file, &candidate.name))
            .or_default()
            .push(candidate);
    }
    for symbol in symbols {
        for relation in &symbol.declared_relations {
            let resolved = relation
                .module_specifier
                .is_none()
                .then(|| local_targets.get(&(symbol.file.as_str(), relation.target_name.as_str())))
                .flatten()
                .and_then(|candidates| {
                    let matching = candidates
                        .iter()
                        .filter(|candidate| {
                            if relation.kind == "calls" {
                                candidate.kind == "function"
                            } else {
                                candidate.role == "entity"
                            }
                        })
                        .collect::<Vec<_>>();
                    (matching.len() == 1).then(|| *matching[0])
                })
                .filter(|candidate| {
                    relation.kind == "calls"
                        || candidate.line != symbol.line
                        || candidate.name != symbol.name
                });
            edges.push(StructureEdge {
                kind: relation.kind.clone(),
                source_file: symbol.file.clone(),
                source_name: symbol.name.clone(),
                source_line: symbol.line,
                target_file: resolved
                    .map(|candidate| candidate.file.clone())
                    .unwrap_or_default(),
                target_name: relation.target_name.clone(),
                target_line: resolved.map(|candidate| candidate.line).unwrap_or(0),
                target_module: relation.module_specifier.clone(),
                target_imported_name: relation.imported_name.clone(),
            });
        }
    }
    edges.sort();
    edges.dedup();
    edges
}

fn insert_edge_row(
    transaction: &rusqlite::Transaction<'_>,
    root: &Path,
    edge: &StructureEdge,
    aliases: Option<&ModuleAliases>,
) -> rusqlite::Result<()> {
    let mut resolved = edge.clone();
    resolve_import_target_from_catalog(root, transaction, aliases, &mut resolved);
    let target_line = if edge.target_module.is_some() && !resolved.target_file.is_empty() {
        // Imported targets are rebound on every query so external edits cannot stale the line.
        0
    } else {
        resolved.target_line
    };
    transaction.execute(
        "INSERT OR IGNORE INTO symbol_edges(
           kind, source_file, source_name, source_line,
           target_file, target_name, target_line, shard,
           target_module, target_imported_name
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            edge.kind,
            edge.source_file,
            edge.source_name,
            edge.source_line as i64,
            resolved.target_file,
            resolved.target_name,
            target_line as i64,
            shard_for(&edge.source_file),
            edge.target_module,
            edge.target_imported_name,
        ],
    )?;
    Ok(())
}

fn replace_module_reexports_for_files<'a>(
    transaction: &rusqlite::Transaction<'_>,
    root: &Path,
    files: impl Iterator<Item = &'a str>,
) -> rusqlite::Result<()> {
    for file in files {
        transaction.execute(
            "DELETE FROM module_reexports
             WHERE source_file = ?1
                OR substr(source_file, 1, length(?1) + 1) = ?1 || '/'",
            params![file],
        )?;
        let Some(ext) = Path::new(file).extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if !matches!(ext, "ets" | "ts" | "tsx" | "js" | "jsx") {
            continue;
        }
        let path = root.join(file);
        let Some(content) = fs::metadata(&path)
            .ok()
            .filter(|metadata| metadata.len() <= MAX_BYTES)
            .and_then(|_| fs::read_to_string(path).ok())
        else {
            continue;
        };
        for reexport in parse_module_reexports(&content, ext) {
            transaction.execute(
                "INSERT OR IGNORE INTO module_reexports(
                   source_file, exported_name, target_module, imported_name
                 ) VALUES(?1, ?2, ?3, ?4)",
                params![
                    file,
                    reexport.exported_name,
                    reexport.target_module,
                    reexport.imported_name,
                ],
            )?;
        }
    }
    Ok(())
}

/// 一致性扫描后重建本轮基础批次，同时保留指纹未变、已由后台补齐的节点。
fn replace_all_symbol_rows_with_files_at(
    root: &Path,
    data_dir: &Path,
    symbols: &[Symbol],
    indexed_files: &[String],
    expected_revision: u64,
) -> bool {
    let mut conn = match Connection::open(catalog_file_at(data_dir, root)) {
        Ok(value) => value,
        Err(_) => return false,
    };
    let transaction = match conn.transaction() {
        Ok(value) => value,
        Err(_) => return false,
    };
    let revision = transaction
        .query_row("SELECT COALESCE(MAX(generation), 0) FROM files", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or(-1);
    if revision < 0 || revision as u64 != expected_revision {
        return false;
    }
    if transaction
        .execute(
            "DELETE FROM symbol_edges
             WHERE NOT EXISTS (
               SELECT 1 FROM files f
               WHERE f.path = symbol_edges.source_file AND f.state = 'indexed'
             )",
            [],
        )
        .is_err()
    {
        return false;
    }
    if transaction
        .execute(
            "DELETE FROM symbols
             WHERE NOT EXISTS (
               SELECT 1 FROM files f WHERE f.path = symbols.file AND f.state = 'indexed'
             )",
            [],
        )
        .is_err()
    {
        return false;
    }
    if transaction
        .execute(
            "DELETE FROM module_reexports
             WHERE NOT EXISTS (
               SELECT 1 FROM files f
               WHERE f.path = module_reexports.source_file AND f.state = 'indexed'
             )",
            [],
        )
        .is_err()
    {
        return false;
    }
    if transaction
        .execute(
            "DELETE FROM semantic_call_edges
             WHERE NOT EXISTS (
               SELECT 1 FROM files f
               WHERE f.path = semantic_call_edges.source_file AND f.state = 'indexed'
             )",
            [],
        )
        .is_err()
    {
        return false;
    }
    if transaction
        .execute(
            "DELETE FROM semantic_target_scans
             WHERE NOT EXISTS (
               SELECT 1 FROM files f
               WHERE f.path=semantic_target_scans.target_file AND f.state='indexed'
                 AND f.size=semantic_target_scans.target_size
                 AND f.mtime_ns=semantic_target_scans.target_mtime_ns
             )
             OR NOT EXISTS (
               SELECT 1 FROM symbols target
               WHERE target.file=semantic_target_scans.target_file
                 AND target.name=semantic_target_scans.target_name
                 AND target.line=semantic_target_scans.target_line AND target.role='logic'
             )",
            [],
        )
        .is_err()
    {
        return false;
    }
    if transaction
        .execute(
            "DELETE FROM semantic_scan_failures
             WHERE NOT EXISTS (
               SELECT 1 FROM files f
               WHERE f.path=semantic_scan_failures.target_file AND f.state='indexed'
                 AND f.size=semantic_scan_failures.target_size
                 AND f.mtime_ns=semantic_scan_failures.target_mtime_ns
             )
             OR NOT EXISTS (
               SELECT 1 FROM symbols target
               WHERE target.file=semantic_scan_failures.target_file
                 AND target.name=semantic_scan_failures.target_name
                 AND target.line=semantic_scan_failures.target_line AND target.role='logic'
             )",
            [],
        )
        .is_err()
    {
        return false;
    }
    let mut baseline_files = symbols
        .iter()
        .map(|symbol| symbol.file.as_str())
        .collect::<Vec<_>>();
    baseline_files.sort_unstable();
    baseline_files.dedup();
    for file in baseline_files {
        if transaction
            .execute(
                "DELETE FROM symbol_edges WHERE source_file = ?1",
                params![file],
            )
            .is_err()
            || transaction
                .execute("DELETE FROM symbols WHERE file = ?1", params![file])
                .is_err()
        {
            return false;
        }
    }
    for symbol in symbols {
        if insert_symbol_row(&transaction, symbol).is_err() {
            return false;
        }
    }
    if transaction
        .execute(
            "DELETE FROM semantic_call_edges
             WHERE NOT EXISTS (
               SELECT 1 FROM files f
               WHERE f.path=semantic_call_edges.source_file AND f.state='indexed'
                 AND f.size=semantic_call_edges.source_size
                 AND f.mtime_ns=semantic_call_edges.source_mtime_ns
             )
             OR NOT EXISTS (
               SELECT 1 FROM symbols source
               WHERE source.file=semantic_call_edges.source_file
                 AND source.name=semantic_call_edges.source_name
                 AND source.line=semantic_call_edges.source_line AND source.role='logic'
             )
             OR NOT EXISTS (
               SELECT 1 FROM symbols target
               WHERE target.file=semantic_call_edges.target_file
                 AND target.name=semantic_call_edges.target_name
                 AND target.line=semantic_call_edges.target_line AND target.role='logic'
             )",
            [],
        )
        .is_err()
    {
        return false;
    }
    if replace_module_reexports_for_files(
        &transaction,
        root,
        indexed_files.iter().map(String::as_str),
    )
    .is_err()
    {
        return false;
    }
    let aliases = load_module_aliases(root, symbols.iter().map(|symbol| symbol.file.as_str()));
    for edge in structure_edges(symbols) {
        if insert_edge_row(&transaction, root, &edge, Some(&aliases)).is_err() {
            return false;
        }
    }
    if transaction
        .execute(
            "UPDATE structure_meta SET revision = revision + 1 WHERE id=1",
            [],
        )
        .is_err()
    {
        return false;
    }
    transaction.commit().is_ok()
}

#[cfg(test)]
fn replace_all_symbol_rows_at(
    root: &Path,
    data_dir: &Path,
    symbols: &[Symbol],
    expected_revision: u64,
) -> bool {
    let mut indexed_files = symbols
        .iter()
        .map(|symbol| symbol.file.clone())
        .collect::<Vec<_>>();
    indexed_files.sort();
    indexed_files.dedup();
    replace_all_symbol_rows_with_files_at(
        root,
        data_dir,
        symbols,
        &indexed_files,
        expected_revision,
    )
}

fn replace_all_symbol_rows(
    root: &Path,
    symbols: &[Symbol],
    indexed_files: &[String],
    expected_revision: u64,
) -> bool {
    let Some(data_dir) = DATA_DIR.get() else { return false };
    replace_all_symbol_rows_with_files_at(
        root,
        data_dir,
        symbols,
        indexed_files,
        expected_revision,
    )
}

/// 文件级事件只替换对应结构节点；删除目录时同时清理路径前缀。
fn replace_changed_symbol_rows_at(
    root: &Path,
    data_dir: &Path,
    rels: &[String],
    symbols: &[Symbol],
) -> bool {
    let mut conn = match Connection::open(catalog_file_at(data_dir, root)) {
        Ok(value) => value,
        Err(_) => return false,
    };
    let transaction = match conn.transaction() {
        Ok(value) => value,
        Err(_) => return false,
    };
    let mut normalized = Vec::new();
    for value in rels {
        let Some((rel, _)) = normalize_changed_path(root, value) else { continue };
        if transaction
            .execute(
                "DELETE FROM symbols
                 WHERE file = ?1 OR substr(file, 1, length(?1) + 1) = ?1 || '/'",
                params![rel],
            )
            .is_err()
        {
            return false;
        }
        if transaction
            .execute(
                "DELETE FROM symbol_edges
                 WHERE source_file = ?1
                    OR substr(source_file, 1, length(?1) + 1) = ?1 || '/'",
                params![rel],
            )
            .is_err()
        {
            return false;
        }
        if transaction
            .execute(
                "DELETE FROM semantic_call_edges
                 WHERE source_file = ?1
                    OR substr(source_file, 1, length(?1) + 1) = ?1 || '/'
                    OR target_file = ?1
                    OR substr(target_file, 1, length(?1) + 1) = ?1 || '/'",
                params![rel],
            )
            .is_err()
        {
            return false;
        }
        if transaction
            .execute(
                "DELETE FROM semantic_target_scans
                 WHERE target_file = ?1
                    OR substr(target_file, 1, length(?1) + 1) = ?1 || '/'",
                params![rel],
            )
            .is_err()
        {
            return false;
        }
        if transaction
            .execute(
                "DELETE FROM semantic_scan_failures
                 WHERE target_file = ?1
                    OR substr(target_file, 1, length(?1) + 1) = ?1 || '/'",
                params![rel],
            )
            .is_err()
        {
            return false;
        }
        normalized.push(rel);
    }
    for symbol in symbols {
        if normalized.iter().any(|rel| {
            symbol.file == *rel || symbol.file.strip_prefix(rel).is_some_and(|tail| tail.starts_with('/'))
        }) && insert_symbol_row(&transaction, symbol).is_err()
        {
            return false;
        }
    }
    if replace_module_reexports_for_files(
        &transaction,
        root,
        normalized.iter().map(String::as_str),
    )
    .is_err()
    {
        return false;
    }
    let aliases = load_module_aliases(root, symbols.iter().map(|symbol| symbol.file.as_str()));
    for edge in structure_edges(symbols) {
        if insert_edge_row(&transaction, root, &edge, Some(&aliases)).is_err() {
            return false;
        }
    }
    if transaction
        .execute(
            "UPDATE structure_meta SET revision = revision + 1 WHERE id=1",
            [],
        )
        .is_err()
    {
        return false;
    }
    transaction.commit().is_ok()
}

fn replace_changed_symbol_rows(root: &Path, rels: &[String], symbols: &[Symbol]) -> bool {
    let Some(data_dir) = DATA_DIR.get() else { return false };
    replace_changed_symbol_rows_at(root, data_dir, rels, symbols)
}

#[derive(Debug)]
struct DeferredBatchResult {
    promoted: usize,
    catalog: Option<CatalogStats>,
    needs_reconciliation: bool,
    lock_wait_ms: u64,
}

/// 从 SQLite 领取一小批 deferred 文件，锁外解析，再以指纹条件更新提交。
fn promote_deferred_batch_at(
    root: &Path,
    data_dir: &Path,
    batch_size: usize,
) -> Result<DeferredBatchResult, String> {
    promote_deferred_batch_at_if(root, data_dir, batch_size, || false)
}

fn promote_deferred_batch_at_if<F>(
    root: &Path,
    data_dir: &Path,
    batch_size: usize,
    is_cancelled: F,
) -> Result<DeferredBatchResult, String>
where
    F: Fn() -> bool,
{
    let db_path = catalog_file_at(data_dir, root);
    let mut conn = Connection::open(db_path).map_err(|error| error.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|error| error.to_string())?;
    let deferred = {
        let mut statement = conn
            .prepare(
                "SELECT path, size, mtime_ns FROM files
                 WHERE state='deferred' ORDER BY shard, path LIMIT ?1",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([batch_size.clamp(1, 1000) as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    FileStamp {
                        len: row.get::<_, i64>(1)?.max(0) as u64,
                        mtime: row.get::<_, i64>(2)?.max(0) as u64,
                    },
                ))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        rows
    };
    if deferred.is_empty() {
        return Ok(DeferredBatchResult {
            promoted: 0,
            catalog: Some(catalog_stats(&conn).map_err(|error| error.to_string())?),
            needs_reconciliation: false,
            lock_wait_ms: 0,
        });
    }

    let mut parsed = Vec::new();
    let mut needs_reconciliation = false;
    for (rel, expected) in deferred {
        if is_cancelled() {
            return Ok(DeferredBatchResult {
                promoted: 0,
                catalog: None,
                needs_reconciliation: false,
                lock_wait_ms: 0,
            });
        }
        let path = root.join(&rel);
        if file_stamp(&path) != Some(expected) {
            needs_reconciliation = true;
            continue;
        }
        let mut symbols = Vec::new();
        scan_file(&path, &rel, &mut symbols);
        parsed.push((rel, expected, symbols));
    }

    if is_cancelled() {
        return Ok(DeferredBatchResult {
            promoted: 0,
            catalog: None,
            needs_reconciliation: false,
            lock_wait_ms: 0,
        });
    }

    let lock_started = std::time::Instant::now();
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let lock_wait_ms = lock_started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let mut promoted = 0usize;
    let aliases = load_module_aliases(
        root,
        parsed
            .iter()
            .flat_map(|(_, _, symbols)| symbols.iter().map(|symbol| symbol.file.as_str())),
    );
    for (rel, expected, symbols) in parsed {
        let updated = transaction
            .execute(
                "UPDATE files SET state='indexed'
                 WHERE path=?1 AND state='deferred' AND size=?2 AND mtime_ns=?3",
                params![rel, expected.len as i64, expected.mtime as i64],
            )
            .map_err(|error| error.to_string())?;
        if updated == 0 {
            continue;
        }
        transaction
            .execute("DELETE FROM symbols WHERE file=?1", params![rel])
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "DELETE FROM symbol_edges WHERE source_file=?1",
                params![rel],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "DELETE FROM semantic_call_edges WHERE source_file=?1",
                params![rel],
            )
            .map_err(|error| error.to_string())?;
        replace_module_reexports_for_files(&transaction, root, std::iter::once(rel.as_str()))
            .map_err(|error| error.to_string())?;
        for symbol in &symbols {
            insert_symbol_row(&transaction, symbol).map_err(|error| error.to_string())?;
        }
        for edge in structure_edges(&symbols) {
            insert_edge_row(&transaction, root, &edge, Some(&aliases))
                .map_err(|error| error.to_string())?;
        }
        promoted += 1;
    }
    if promoted > 0 {
        transaction
            .execute(
                "UPDATE structure_meta SET revision = revision + 1 WHERE id=1",
                [],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())?;
    let catalog = catalog_stats(&conn).map_err(|error| error.to_string())?;
    Ok(DeferredBatchResult {
        promoted,
        catalog: Some(catalog),
        needs_reconciliation,
        lock_wait_ms,
    })
}

#[cfg(not(test))]
struct ProgressiveWorker {
    cancel: AtomicBool,
    promoted: AtomicUsize,
    batches: AtomicUsize,
    last_batch_ms: AtomicU64,
    last_lock_wait_ms: AtomicU64,
    throttle_ms: AtomicU64,
}

#[cfg(not(test))]
static PROGRESSIVE_WORKERS: OnceLock<Mutex<HashMap<String, Arc<ProgressiveWorker>>>> = OnceLock::new();
#[cfg(not(test))]
const PROGRESSIVE_BATCH_FILES: usize = 128;

fn progressive_throttle_ms(elapsed_ms: u64) -> u64 {
    if elapsed_ms >= 500 {
        200
    } else if elapsed_ms >= 200 {
        100
    } else if elapsed_ms >= 75 {
        50
    } else {
        20
    }
}

#[cfg(not(test))]
fn ensure_progressive_indexing(root: &Path) {
    let Some(data_dir) = DATA_DIR.get().cloned() else { return };
    let root = root.to_path_buf();
    let key = canonical_key(&root);
    let workers = PROGRESSIVE_WORKERS.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut guard) = workers.lock() else { return };
    if guard.contains_key(&key) {
        return;
    }
    let state = Arc::new(ProgressiveWorker {
        cancel: AtomicBool::new(false),
        promoted: AtomicUsize::new(0),
        batches: AtomicUsize::new(0),
        last_batch_ms: AtomicU64::new(0),
        last_lock_wait_ms: AtomicU64::new(0),
        throttle_ms: AtomicU64::new(20),
    });
    guard.insert(key.clone(), state.clone());
    drop(guard);
    let spawn_key = key.clone();
    let spawn_state = state.clone();
    if std::thread::Builder::new()
        .name("repo-progressive-index".into())
        .spawn(move || {
            loop {
                if state.cancel.load(Ordering::Relaxed) {
                    break;
                }
                let batch_started = std::time::Instant::now();
                match promote_deferred_batch_at_if(
                    &root,
                    &data_dir,
                    PROGRESSIVE_BATCH_FILES,
                    || state.cancel.load(Ordering::Relaxed),
                ) {
                    Ok(result) => {
                        let elapsed_ms = batch_started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                        state.last_batch_ms.store(elapsed_ms, Ordering::Relaxed);
                        state
                            .last_lock_wait_ms
                            .store(result.lock_wait_ms, Ordering::Relaxed);
                        state.promoted.fetch_add(result.promoted, Ordering::Relaxed);
                        state.batches.fetch_add(1, Ordering::Relaxed);
                        if let Some(catalog) = result.catalog {
                            if let Ok(mut cache) = cache().lock() {
                                if let Some(entry) = cache.get_mut(&key) {
                                    entry.catalog = CatalogStats {
                                        unreadable_directories: entry.catalog.unreadable_directories,
                                        ..catalog
                                    };
                                }
                            }
                        }
                        if result.needs_reconciliation {
                            request_reconciliation(&root);
                            break;
                        }
                        if result.promoted == 0 {
                            break;
                        }
                    }
                    Err(_) => {
                        request_reconciliation(&root);
                        break;
                    }
                }
                // 慢盘/复杂源码批次主动加大间隔，避免后台索引持续争抢前台 IO/CPU。
                let elapsed_ms = state.last_batch_ms.load(Ordering::Relaxed);
                let throttle_ms = progressive_throttle_ms(elapsed_ms);
                state.throttle_ms.store(throttle_ms, Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(throttle_ms));
            }
            if let Some(workers) = PROGRESSIVE_WORKERS.get() {
                if let Ok(mut guard) = workers.lock() {
                    if guard.get(&key).is_some_and(|current| Arc::ptr_eq(current, &state)) {
                        guard.remove(&key);
                    }
                }
            }
        })
        .is_err()
    {
        if let Ok(mut guard) = workers.lock() {
            if guard
                .get(&spawn_key)
                .is_some_and(|current| Arc::ptr_eq(current, &spawn_state))
            {
                guard.remove(&spawn_key);
            }
        }
    }
}

#[cfg(not(test))]
fn cancel_progressive_indexing(root: &Path) {
    let key = canonical_key(root);
    if let Some(workers) = PROGRESSIVE_WORKERS.get() {
        if let Ok(mut guard) = workers.lock() {
            if let Some(state) = guard.remove(&key) {
                state.cancel.store(true, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
fn cancel_progressive_indexing(_root: &Path) {}

/// 遍历所有未忽略文件。回调是流式的，百万文件时不需要把全目录保存在内存。
fn walk_catalog<F>(
    dir: &Path,
    root: &Path,
    parse_budget: usize,
    stats: &mut CatalogStats,
    visit: &mut F,
)
where
    F: FnMut(&str, &str, u64, u64, &str, &str),
{
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => {
            stats.unreadable_directories += 1;
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let file_type = match entry.file_type() {
            Ok(value) => value,
            Err(_) => {
                stats.unreadable_files += 1;
                continue;
            }
        };
        if file_type.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            walk_catalog(&path, root, parse_budget, stats, visit);
            continue;
        }

        let rel = safe_rel(root, &path);
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        stats.discovered_files += 1;
        if file_type.is_symlink() {
            stats.symlink_files += 1;
            visit(&rel, ext, 0, 0, "symlink", shard_for(&rel));
            continue;
        }
        if !file_type.is_file() {
            stats.unsupported_files += 1;
            visit(&rel, ext, 0, 0, "unsupported", shard_for(&rel));
            continue;
        }
        let meta = match entry.metadata() {
            Ok(value) => value,
            Err(_) => {
                stats.unreadable_files += 1;
                visit(&rel, ext, 0, 0, "unreadable", shard_for(&rel));
                continue;
            }
        };
        let stamp = stamp_from_meta(&meta);
        if !SYMBOL_EXTS.contains(&ext) {
            stats.unsupported_files += 1;
            visit(&rel, ext, stamp.len, stamp.mtime, "unsupported", shard_for(&rel));
        } else if stamp.len > MAX_BYTES {
            stats.source_files += 1;
            stats.oversized_source_files += 1;
            visit(&rel, ext, stamp.len, stamp.mtime, "oversized", shard_for(&rel));
        } else if stats.indexed_source_files < parse_budget {
            stats.source_files += 1;
            stats.indexed_source_files += 1;
            visit(&rel, ext, stamp.len, stamp.mtime, "indexed", shard_for(&rel));
        } else {
            stats.source_files += 1;
            stats.deferred_source_files += 1;
            visit(&rel, ext, stamp.len, stamp.mtime, "deferred", shard_for(&rel));
        }
    }
}

/// 刷新全库目录，并返回本轮允许进入轻量结构解析预算的源码文件。
fn ensure_structure_schema(conn: &mut Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS module_reexports (
           source_file TEXT NOT NULL,
           exported_name TEXT NOT NULL,
           target_module TEXT NOT NULL,
           imported_name TEXT NOT NULL,
           PRIMARY KEY(source_file, exported_name, target_module, imported_name)
         );
         CREATE INDEX IF NOT EXISTS idx_reexports_source_name
           ON module_reexports(source_file, exported_name);
         CREATE TABLE IF NOT EXISTS semantic_call_edges (
           source_file TEXT NOT NULL,
           source_name TEXT NOT NULL,
           source_line INTEGER NOT NULL,
           call_line INTEGER NOT NULL,
           call_column INTEGER NOT NULL,
           source_size INTEGER NOT NULL,
           source_mtime_ns INTEGER NOT NULL,
           target_file TEXT NOT NULL,
           target_name TEXT NOT NULL,
           target_line INTEGER NOT NULL,
           provider TEXT NOT NULL,
           PRIMARY KEY(source_file, call_line, call_column, provider)
         );
         CREATE INDEX IF NOT EXISTS idx_semantic_calls_source
           ON semantic_call_edges(source_file, source_name, source_line);
         CREATE INDEX IF NOT EXISTS idx_semantic_calls_target
           ON semantic_call_edges(target_file, target_name, target_line);
         CREATE TABLE IF NOT EXISTS semantic_target_scans (
           target_file TEXT NOT NULL,
           target_name TEXT NOT NULL,
           target_line INTEGER NOT NULL,
           target_size INTEGER NOT NULL,
           target_mtime_ns INTEGER NOT NULL,
           provider TEXT NOT NULL,
           scanned_at INTEGER NOT NULL,
           reference_count INTEGER NOT NULL,
           recorded_call_count INTEGER NOT NULL,
           truncated INTEGER NOT NULL DEFAULT 0,
           PRIMARY KEY(target_file, target_name, target_line, provider)
         );
         CREATE TABLE IF NOT EXISTS semantic_scan_failures (
           target_file TEXT NOT NULL,
           target_name TEXT NOT NULL,
           target_line INTEGER NOT NULL,
           target_size INTEGER NOT NULL,
           target_mtime_ns INTEGER NOT NULL,
           provider TEXT NOT NULL,
           failure_count INTEGER NOT NULL,
           last_attempt_at INTEGER NOT NULL,
           retry_after INTEGER NOT NULL,
           PRIMARY KEY(target_file, target_name, target_line, provider)
         );
         CREATE INDEX IF NOT EXISTS idx_semantic_failures_retry
           ON semantic_scan_failures(provider, retry_after, target_file, target_line);",
    )?;
    let columns = conn
        .prepare("PRAGMA table_info(symbols)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|column| column == "language") {
        conn.execute(
            "ALTER TABLE symbols ADD COLUMN language TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    if !columns.iter().any(|column| column == "source_layer") {
        conn.execute(
            "ALTER TABLE symbols ADD COLUMN source_layer TEXT NOT NULL DEFAULT 'lightweight'",
            [],
        )?;
    }
    if !columns.iter().any(|column| column == "declared_relations") {
        conn.execute(
            "ALTER TABLE symbols ADD COLUMN declared_relations TEXT NOT NULL DEFAULT '[]'",
            [],
        )?;
    }
    let edge_columns = conn
        .prepare("PRAGMA table_info(symbol_edges)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !edge_columns.iter().any(|column| column == "target_module") {
        conn.execute("ALTER TABLE symbol_edges ADD COLUMN target_module TEXT", [])?;
    }
    if !edge_columns
        .iter()
        .any(|column| column == "target_imported_name")
    {
        conn.execute(
            "ALTER TABLE symbol_edges ADD COLUMN target_imported_name TEXT",
            [],
        )?;
    }
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_edges_import_target
         ON symbol_edges(target_name, target_module)
         WHERE target_module IS NOT NULL",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_symbols_semantic_schedule
         ON symbols(role, language, kind, file, line, name)",
        [],
    )?;
    let meta_columns = conn
        .prepare("PRAGMA table_info(structure_meta)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !meta_columns.iter().any(|column| column == "parser_version") {
        conn.execute(
            "ALTER TABLE structure_meta ADD COLUMN parser_version INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    let stats_columns = conn
        .prepare("PRAGMA table_info(structure_stats)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !stats_columns
        .iter()
        .any(|column| column == "semantic_relation_count")
    {
        conn.execute(
            "ALTER TABLE structure_stats
             ADD COLUMN semantic_relation_count INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
        conn.execute(
            "UPDATE structure_stats SET semantic_relation_count=(
               SELECT COUNT(*) FROM semantic_call_edges
             ) WHERE id=1",
            [],
        )?;
    }
    if !stats_columns
        .iter()
        .any(|column| column == "logic_symbol_count")
    {
        conn.execute(
            "ALTER TABLE structure_stats
             ADD COLUMN logic_symbol_count INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
        conn.execute(
            "UPDATE structure_stats SET logic_symbol_count=(
               SELECT COUNT(*) FROM symbols WHERE role='logic'
             ) WHERE id=1",
            [],
        )?;
    }
    if !stats_columns
        .iter()
        .any(|column| column == "semantic_target_count")
    {
        conn.execute(
            "ALTER TABLE structure_stats
             ADD COLUMN semantic_target_count INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
        conn.execute(
            "UPDATE structure_stats SET semantic_target_count=(
               SELECT COUNT(*) FROM semantic_target_scans
             ) WHERE id=1",
            [],
        )?;
    }
    if !stats_columns
        .iter()
        .any(|column| column == "semantic_truncated_target_count")
    {
        conn.execute(
            "ALTER TABLE structure_stats
             ADD COLUMN semantic_truncated_target_count INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
        conn.execute(
            "UPDATE structure_stats SET semantic_truncated_target_count=(
               SELECT COUNT(*) FROM semantic_target_scans WHERE truncated=1
             ) WHERE id=1",
            [],
        )?;
    }
    if !stats_columns
        .iter()
        .any(|column| column == "semantic_failure_target_count")
    {
        conn.execute(
            "ALTER TABLE structure_stats
             ADD COLUMN semantic_failure_target_count INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
        conn.execute(
            "UPDATE structure_stats SET semantic_failure_target_count=(
               SELECT COUNT(*) FROM semantic_scan_failures
             ) WHERE id=1",
            [],
        )?;
    }
    conn.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS trg_semantic_call_edges_insert
           AFTER INSERT ON semantic_call_edges BEGIN
             UPDATE structure_stats
             SET semantic_relation_count=semantic_relation_count + 1 WHERE id=1;
           END;
         CREATE TRIGGER IF NOT EXISTS trg_semantic_call_edges_delete
           AFTER DELETE ON semantic_call_edges BEGIN
             UPDATE structure_stats
             SET semantic_relation_count=MAX(0, semantic_relation_count - 1) WHERE id=1;
           END;
         CREATE TRIGGER IF NOT EXISTS trg_logic_symbols_insert
           AFTER INSERT ON symbols WHEN NEW.role='logic' BEGIN
             UPDATE structure_stats SET logic_symbol_count=logic_symbol_count + 1 WHERE id=1;
           END;
         CREATE TRIGGER IF NOT EXISTS trg_logic_symbols_delete
           AFTER DELETE ON symbols WHEN OLD.role='logic' BEGIN
             UPDATE structure_stats SET logic_symbol_count=MAX(0, logic_symbol_count - 1) WHERE id=1;
           END;
         CREATE TRIGGER IF NOT EXISTS trg_semantic_target_scans_insert
           AFTER INSERT ON semantic_target_scans BEGIN
             UPDATE structure_stats
             SET semantic_target_count=semantic_target_count + 1,
                 semantic_truncated_target_count=semantic_truncated_target_count + NEW.truncated
             WHERE id=1;
           END;
         CREATE TRIGGER IF NOT EXISTS trg_semantic_target_scans_delete
           AFTER DELETE ON semantic_target_scans BEGIN
             UPDATE structure_stats
             SET semantic_target_count=MAX(0, semantic_target_count - 1),
                 semantic_truncated_target_count=MAX(
                   0, semantic_truncated_target_count - OLD.truncated
                 ) WHERE id=1;
           END;
         CREATE TRIGGER IF NOT EXISTS trg_semantic_target_scans_update
           AFTER UPDATE OF truncated ON semantic_target_scans BEGIN
             UPDATE structure_stats
             SET semantic_truncated_target_count=MAX(
               0, semantic_truncated_target_count + NEW.truncated - OLD.truncated
             ) WHERE id=1;
           END;
         CREATE TRIGGER IF NOT EXISTS trg_semantic_scan_failures_insert
           AFTER INSERT ON semantic_scan_failures BEGIN
             UPDATE structure_stats
             SET semantic_failure_target_count=semantic_failure_target_count + 1
             WHERE id=1;
           END;
         CREATE TRIGGER IF NOT EXISTS trg_semantic_scan_failures_delete
           AFTER DELETE ON semantic_scan_failures BEGIN
             UPDATE structure_stats
             SET semantic_failure_target_count=MAX(0, semantic_failure_target_count - 1)
             WHERE id=1;
           END;",
    )?;
    let parser_version = conn.query_row(
        "SELECT parser_version FROM structure_meta WHERE id=1",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if parser_version < STRUCTURE_PARSER_VERSION {
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("DELETE FROM symbol_edges", [])?;
        transaction.execute("DELETE FROM symbols", [])?;
        transaction.execute("DELETE FROM module_reexports", [])?;
        transaction.execute("DELETE FROM semantic_call_edges", [])?;
        transaction.execute("DELETE FROM semantic_target_scans", [])?;
        transaction.execute("DELETE FROM semantic_scan_failures", [])?;
        transaction.execute("UPDATE files SET state='deferred' WHERE state='indexed'", [])?;
        transaction.execute(
            "UPDATE structure_meta
             SET parser_version=?1, revision=revision + 1 WHERE id=1",
            [STRUCTURE_PARSER_VERSION],
        )?;
        transaction.commit()?;
    }
    Ok(())
}

fn collect_files_at_with_budget(
    root: &Path,
    data_dir: Option<&Path>,
    parse_budget: usize,
) -> (HashMap<String, FileStamp>, CatalogStats) {
    let mut files = HashMap::new();
    let mut stats = CatalogStats::default();
    let generation = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0);

    let mut catalog = data_dir
        .and_then(|dir| {
            let path = catalog_file_at(dir, root);
            fs::create_dir_all(path.parent()?).ok()?;
            let mut conn = Connection::open(path).ok()?;
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=NORMAL;
                 CREATE TABLE IF NOT EXISTS files (
                   path TEXT PRIMARY KEY,
                   extension TEXT NOT NULL,
                   size INTEGER NOT NULL,
                   mtime_ns INTEGER NOT NULL,
                   state TEXT NOT NULL,
                   shard TEXT NOT NULL,
                   generation INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_files_state ON files(state);
                 CREATE INDEX IF NOT EXISTS idx_files_shard ON files(shard);
                 CREATE INDEX IF NOT EXISTS idx_files_state_order
                   ON files(state, shard, path);
                 CREATE TABLE IF NOT EXISTS symbols (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   file TEXT NOT NULL,
                   kind TEXT NOT NULL,
                   name TEXT NOT NULL,
                   line INTEGER NOT NULL,
                   end_line INTEGER NOT NULL,
                   role TEXT NOT NULL,
                   signature TEXT NOT NULL,
                   parent TEXT,
                   shard TEXT NOT NULL,
                   language TEXT NOT NULL DEFAULT '',
                   source_layer TEXT NOT NULL DEFAULT 'lightweight',
                   declared_relations TEXT NOT NULL DEFAULT '[]'
                 );
                 CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file);
                 CREATE INDEX IF NOT EXISTS idx_symbols_role_kind ON symbols(role, kind);
                 CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name COLLATE NOCASE);
                 CREATE INDEX IF NOT EXISTS idx_symbols_shard ON symbols(shard);
                 CREATE INDEX IF NOT EXISTS idx_symbols_kind_order
                   ON symbols(kind, file, line, name);
                 CREATE TABLE IF NOT EXISTS symbol_edges (
                   kind TEXT NOT NULL,
                   source_file TEXT NOT NULL,
                   source_name TEXT NOT NULL,
                   source_line INTEGER NOT NULL,
                   target_file TEXT NOT NULL,
                   target_name TEXT NOT NULL,
                   target_line INTEGER NOT NULL,
                   shard TEXT NOT NULL,
                   target_module TEXT,
                   target_imported_name TEXT,
                   PRIMARY KEY(kind, source_file, source_name, source_line,
                               target_file, target_name, target_line)
                 );
                 CREATE INDEX IF NOT EXISTS idx_edges_source
                   ON symbol_edges(source_file, source_name, source_line);
                 CREATE INDEX IF NOT EXISTS idx_edges_target
                   ON symbol_edges(target_file, target_name, target_line);
                 CREATE INDEX IF NOT EXISTS idx_edges_shard ON symbol_edges(shard);
                 CREATE TABLE IF NOT EXISTS module_reexports (
                   source_file TEXT NOT NULL,
                   exported_name TEXT NOT NULL,
                   target_module TEXT NOT NULL,
                   imported_name TEXT NOT NULL,
                   PRIMARY KEY(source_file, exported_name, target_module, imported_name)
                 );
                 CREATE INDEX IF NOT EXISTS idx_reexports_source_name
                   ON module_reexports(source_file, exported_name);
                 CREATE TABLE IF NOT EXISTS semantic_call_edges (
                   source_file TEXT NOT NULL,
                   source_name TEXT NOT NULL,
                   source_line INTEGER NOT NULL,
                   call_line INTEGER NOT NULL,
                   call_column INTEGER NOT NULL,
                   source_size INTEGER NOT NULL,
                   source_mtime_ns INTEGER NOT NULL,
                   target_file TEXT NOT NULL,
                   target_name TEXT NOT NULL,
                   target_line INTEGER NOT NULL,
                   provider TEXT NOT NULL,
                   PRIMARY KEY(source_file, call_line, call_column, provider)
                 );
                 CREATE INDEX IF NOT EXISTS idx_semantic_calls_source
                   ON semantic_call_edges(source_file, source_name, source_line);
                 CREATE INDEX IF NOT EXISTS idx_semantic_calls_target
                   ON semantic_call_edges(target_file, target_name, target_line);
                 CREATE TABLE IF NOT EXISTS semantic_target_scans (
                   target_file TEXT NOT NULL,
                   target_name TEXT NOT NULL,
                   target_line INTEGER NOT NULL,
                   target_size INTEGER NOT NULL,
                   target_mtime_ns INTEGER NOT NULL,
                   provider TEXT NOT NULL,
                   scanned_at INTEGER NOT NULL,
                   reference_count INTEGER NOT NULL,
                   recorded_call_count INTEGER NOT NULL,
                   truncated INTEGER NOT NULL DEFAULT 0,
                   PRIMARY KEY(target_file, target_name, target_line, provider)
                 );
                 CREATE TABLE IF NOT EXISTS semantic_scan_failures (
                   target_file TEXT NOT NULL,
                   target_name TEXT NOT NULL,
                   target_line INTEGER NOT NULL,
                   target_size INTEGER NOT NULL,
                   target_mtime_ns INTEGER NOT NULL,
                   provider TEXT NOT NULL,
                   failure_count INTEGER NOT NULL,
                   last_attempt_at INTEGER NOT NULL,
                   retry_after INTEGER NOT NULL,
                   PRIMARY KEY(target_file, target_name, target_line, provider)
                 );
                 CREATE INDEX IF NOT EXISTS idx_semantic_failures_retry
                   ON semantic_scan_failures(provider, retry_after, target_file, target_line);
                 CREATE TABLE IF NOT EXISTS structure_stats (
                   id INTEGER PRIMARY KEY CHECK(id = 1),
                   relation_count INTEGER NOT NULL DEFAULT 0,
                   semantic_relation_count INTEGER NOT NULL DEFAULT 0,
                   logic_symbol_count INTEGER NOT NULL DEFAULT 0,
                   semantic_target_count INTEGER NOT NULL DEFAULT 0,
                   semantic_truncated_target_count INTEGER NOT NULL DEFAULT 0,
                   semantic_failure_target_count INTEGER NOT NULL DEFAULT 0
                 );
                 INSERT OR IGNORE INTO structure_stats(id, relation_count)
                   SELECT 1, COUNT(*) FROM symbol_edges;
                 CREATE TRIGGER IF NOT EXISTS trg_symbol_edges_insert
                   AFTER INSERT ON symbol_edges BEGIN
                     UPDATE structure_stats SET relation_count = relation_count + 1 WHERE id = 1;
                   END;
                 CREATE TRIGGER IF NOT EXISTS trg_symbol_edges_delete
                   AFTER DELETE ON symbol_edges BEGIN
                     UPDATE structure_stats SET relation_count = MAX(0, relation_count - 1) WHERE id = 1;
                   END;
                 CREATE TRIGGER IF NOT EXISTS trg_semantic_call_edges_insert
                   AFTER INSERT ON semantic_call_edges BEGIN
                     UPDATE structure_stats
                     SET semantic_relation_count=semantic_relation_count + 1 WHERE id=1;
                   END;
                 CREATE TRIGGER IF NOT EXISTS trg_semantic_call_edges_delete
                   AFTER DELETE ON semantic_call_edges BEGIN
                     UPDATE structure_stats
                     SET semantic_relation_count=MAX(0, semantic_relation_count - 1) WHERE id=1;
                   END;
                 CREATE TABLE IF NOT EXISTS structure_meta (
                   id INTEGER PRIMARY KEY CHECK(id = 1),
                   revision INTEGER NOT NULL DEFAULT 0,
                   parser_version INTEGER NOT NULL DEFAULT 0
                 );
                 INSERT OR IGNORE INTO structure_meta(id, revision) VALUES(1, 0);",
            )
            .ok()?;
            ensure_structure_schema(&mut conn).ok()?;
            conn.execute_batch("BEGIN IMMEDIATE;").ok()?;
            Some(conn)
        });
    let mut write_failed = false;
    walk_catalog(root, root, parse_budget, &mut stats, &mut |rel, ext, size, mtime, state, shard| {
        if state == "indexed" {
            files.insert(rel.to_string(), FileStamp { mtime, len: size });
        }
        if let Some(conn) = catalog.as_mut() {
            if conn.execute(
                "INSERT INTO files(path, extension, size, mtime_ns, state, shard, generation)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(path) DO UPDATE SET
                   extension=excluded.extension, size=excluded.size, mtime_ns=excluded.mtime_ns,
                   state=CASE
                     WHEN files.state='indexed' AND excluded.state='deferred'
                          AND files.size=excluded.size AND files.mtime_ns=excluded.mtime_ns
                     THEN files.state ELSE excluded.state END,
                   shard=excluded.shard, generation=excluded.generation",
                params![rel, ext, size as i64, mtime as i64, state, shard, generation],
            ).is_err() {
                write_failed = true;
            }
        }
    });
    if let Some(conn) = catalog.as_mut() {
        // 某个目录暂时不可读时保留其上一代记录，避免一次权限抖动被误判成整目录删除。
        let cleanup_ok = stats.unreadable_directories > 0
            || conn
                .execute("DELETE FROM files WHERE generation <> ?1", [generation])
                .is_ok();
        if !write_failed
            && cleanup_ok
            && conn.execute_batch("COMMIT;").is_ok()
        {
            stats = catalog_stats(conn).unwrap_or(stats);
            stats.persisted = true;
            stats.revision = generation.max(0) as u64;
        } else {
            let _ = conn.execute_batch("ROLLBACK;");
        }
    }
    (files, stats)
}

fn collect_files_at(
    root: &Path,
    data_dir: Option<&Path>,
) -> (HashMap<String, FileStamp>, CatalogStats) {
    collect_files_at_with_budget(root, data_dir, MAX_FILES)
}

fn collect_files(root: &Path) -> (HashMap<String, FileStamp>, CatalogStats) {
    collect_files_at(root, DATA_DIR.get().map(PathBuf::as_path))
}

/// 扫描整个项目，返回全部符号（全量构建：无缓存的底层实现）。
/// 强制刷新入口 refresh_project_symbols 已改为 invalidate_cache + cached 组合，此函数暂无调用者。
#[allow(dead_code)]
pub fn index_project(root: &Path) -> Vec<Symbol> {
    let (files, _) = collect_files(root);
    let mut out = Vec::new();
    for rel in files.keys() {
        let p = root.join(rel);
        scan_file(&p, rel, &mut out);
    }
    out
}

/// 符号索引缓存：key = 规范化后的项目根路径，value = (文件指纹映射, 符号列表, 最近同步秒)。
/// 每次检索 walk + stat 收集当前文件指纹，与缓存对比后只重扫变化文件；
/// 另持久化到磁盘（<app_data>/symbol_cache/），重启后首次打开面板即可命中。
struct CacheEntry {
    files: HashMap<String, FileStamp>,
    syms: Vec<Symbol>,
    catalog: CatalogStats,
    /// Git HEAD/index 的廉价指纹，用于补偿 checkout/reset 等 watcher 可能漏报的批量变化。
    git_checkpoint: Option<GitCheckpoint>,
    /// watcher 检测到外部变化或丢事件后，下一次查询执行全库一致性校验。
    needs_reconciliation: bool,
    /// 最近一次增量同步的秒：冷却期内直接复用内存结果（Agent 修改文件会主动精确失效）
    last_sync: u64,
    /// 数据来源：disk（磁盘恢复）/ scan（本次会话扫描建立），供面板展示缓存状态
    source: &'static str,
}

static SYMBOL_CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();

/// 持久化缓存目录（lib.rs setup 中初始化）；未初始化时退化为纯内存模式（测试场景）
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 由应用启动流程注入符号缓存的持久化目录
pub fn init_cache_dir(dir: PathBuf) {
    let _ = DATA_DIR.set(dir);
}

/// 增量同步冷却（秒）：冷却期内直接返回内存结果，避免高频检索反复 walk；
/// 修改类工具会主动 invalidate_files 立即更新，冷却不会掩盖 Agent 的改动。
// 全库目录会覆盖所有未忽略文件，避免高频查询反复遍历百万文件；工具内修改仍会精确失效。
// watcher/Git diff 补偿接入后可进一步延长或移除周期性 walk。
const SYNC_COOLDOWN_SECS: u64 = 30;
/// 即使 watcher 自称 active，也要低频校验，防止网络盘、队列溢出或静默失效永久污染索引。
const WATCHER_RECONCILE_SECS: u64 = 5 * 60;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    SYMBOL_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn canonical_key(root: &Path) -> String {
    root.canonicalize()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| root.to_string_lossy().to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitCheckpoint {
    head: String,
    index: Option<FileStamp>,
}

fn git_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let value = fs::read_to_string(dot_git).ok()?;
    let path = value.trim().strip_prefix("gitdir:")?.trim();
    let path = Path::new(path);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    })
}

fn git_common_dir(git_dir: &Path) -> PathBuf {
    let Some(value) = fs::read_to_string(git_dir.join("commondir")).ok() else {
        return git_dir.to_path_buf();
    };
    let path = Path::new(value.trim());
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        git_dir.join(path)
    }
}

fn packed_ref_oid(common_dir: &Path, reference: &str) -> Option<String> {
    fs::read_to_string(common_dir.join("packed-refs"))
        .ok()?
        .lines()
        .filter(|line| !line.starts_with('#') && !line.starts_with('^'))
        .find_map(|line| {
            let (oid, name) = line.split_once(' ')?;
            (name == reference).then(|| oid.to_string())
        })
}

fn git_checkpoint(root: &Path) -> Option<GitCheckpoint> {
    let git_dir = git_dir(root)?;
    let common_dir = git_common_dir(&git_dir);
    let head_value = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head_value = head_value.trim();
    let head = if let Some(reference) = head_value.strip_prefix("ref: ") {
        fs::read_to_string(git_dir.join(reference))
            .or_else(|_| fs::read_to_string(common_dir.join(reference)))
            .ok()
            .map(|value| value.trim().to_string())
            .or_else(|| packed_ref_oid(&common_dir, reference))?
    } else {
        head_value.to_string()
    };
    if head.is_empty() {
        return None;
    }
    let index = fs::metadata(git_dir.join("index"))
        .ok()
        .map(|metadata| stamp_from_meta(&metadata));
    Some(GitCheckpoint { head, index })
}

const MAX_GIT_DELTA_PATHS: usize = 20_000;
const MAX_GIT_DELTA_BYTES: usize = 8 * 1024 * 1024;

/// HEAD 变化时让 Git 枚举变化文件；禁用 rename 检测后 rename 会自然展开为 delete + add。
/// index-only 变化（如 reset --hard）没有旧 tree 可比较，返回 None 触发一致性扫描。
fn git_changed_paths(
    root: &Path,
    previous: &GitCheckpoint,
    current: &GitCheckpoint,
) -> Option<Vec<String>> {
    if previous.head == current.head {
        return (previous.index == current.index).then(Vec::new);
    }
    let args = vec![
        "-C".to_string(),
        root.to_string_lossy().to_string(),
        "diff".to_string(),
        "--name-only".to_string(),
        "-z".to_string(),
        "--no-renames".to_string(),
        previous.head.clone(),
        current.head.clone(),
        "--".to_string(),
    ];
    let output = crate::utils::process::output_blocking("git", &args).ok()?;
    if !output.status.success() || output.stdout.len() > MAX_GIT_DELTA_BYTES {
        return None;
    }
    let mut paths = Vec::new();
    for value in output.stdout.split(|byte| *byte == 0).filter(|value| !value.is_empty()) {
        let rel = std::str::from_utf8(value).ok()?.replace('\\', "/");
        if normalize_changed_path(root, &rel).is_none() {
            return None;
        }
        paths.push(rel);
        if paths.len() > MAX_GIT_DELTA_PATHS {
            return None;
        }
    }
    paths.sort();
    paths.dedup();
    Some(paths)
}

// ---------- 磁盘持久化 ----------

/// 磁盘缓存格式：<data_dir>/symbol_cache/<fnv1a(项目路径)>.json
#[derive(Debug, Serialize, Deserialize)]
struct PersistedIndex {
    version: u32,
    files: HashMap<String, FileStamp>,
    syms: Vec<Symbol>,
    catalog: CatalogStats,
}

// v12 persists AST-declared direct call evidence in addition to type relationships.
const PERSIST_VERSION: u32 = 12;

/// FNV-1a 64 位：把项目根路径稳定散列为缓存文件名
fn stable_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn cache_file_at(dir: &Path, root: &Path) -> PathBuf {
    let key = canonical_key(root);
    dir.join(format!("{:016x}.json", stable_hash(&key)))
}

fn cache_file_for(root: &Path) -> Option<PathBuf> {
    let dir = DATA_DIR.get()?;
    Some(cache_file_at(dir, root))
}

fn load_from(dir: &Path, root: &Path) -> Option<PersistedIndex> {
    let text = fs::read_to_string(cache_file_at(dir, root)).ok()?;
    let idx: PersistedIndex = serde_json::from_str(&text).ok()?;
    if idx.version != PERSIST_VERSION {
        return None;
    }
    Some(idx)
}

/// 原子写盘（tmp + rename），失败静默——缓存只是加速手段，不影响正确性
fn save_to(
    dir: &Path,
    root: &Path,
    files: &HashMap<String, FileStamp>,
    syms: &[Symbol],
    catalog: CatalogStats,
) {
    let path = cache_file_at(dir, root);
    let Some(parent) = path.parent() else { return };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let idx = PersistedIndex {
        version: PERSIST_VERSION,
        files: files.clone(),
        syms: syms.to_vec(),
        catalog,
    };
    let Ok(json) = serde_json::to_string(&idx) else { return };
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, json).is_err() {
        return;
    }
    let _ = fs::rename(&tmp, &path);
}

fn load_persisted(root: &Path) -> Option<PersistedIndex> {
    let dir = DATA_DIR.get()?;
    load_from(dir, root)
}

fn save_persisted(
    root: &Path,
    files: &HashMap<String, FileStamp>,
    syms: &[Symbol],
    catalog: CatalogStats,
) {
    if let Some(dir) = DATA_DIR.get() {
        save_to(dir, root, files, syms, catalog);
    }
}

// ---------- 增量同步 ----------

/// 对缓存状态做增量同步：walk 收集当前文件指纹，与缓存对比，
/// 仅重新解析新增/变化的文件；被删除文件的符号直接剔除。
/// 只操作传入的 files/syms（无锁），供 index_project_cached 锁外阶段调用。
/// 返回 (重新解析的文件数, 移除符号的文件数)；无变化时为 (0, 0)。
fn sync_incremental(
    files: &mut HashMap<String, FileStamp>,
    syms: &mut Vec<Symbol>,
    root: &Path,
) -> (usize, usize, CatalogStats) {
    let (current, catalog) = collect_files(root);

    let mut rescanned = 0usize;
    let mut removed = 0usize;

    // 删除的文件：缓存里有、当前没有
    let gone: Vec<String> = files
        .keys()
        .filter(|rel| !current.contains_key(*rel))
        .cloned()
        .collect();
    for rel in gone {
        files.remove(&rel);
        let before = syms.len();
        syms.retain(|s| s.file != rel);
        removed += before - syms.len();
    }

    // 新增/变化的文件：指纹不同才重扫
    let changed: Vec<String> = current
        .iter()
        .filter(|(rel, stamp)| files.get(*rel) != Some(*stamp))
        .map(|(rel, _)| rel.clone())
        .collect();
    for rel in changed {
        if let Some(stamp) = current.get(&rel) {
            files.insert(rel.clone(), *stamp);
        }
        syms.retain(|s| s.file != rel);
        let mut fresh = Vec::new();
        scan_file(&root.join(&rel), &rel, &mut fresh);
        syms.extend(fresh);
        rescanned += 1;
    }
    (rescanned, removed, catalog)
}

/// 带缓存的符号索引：内存 → 磁盘 → watcher 快路径 → 增量同步。
/// watcher 可用时事件触发精准失效并低频一致性扫描；不可用时周期 walk + stat；
/// 磁盘缓存使重启后首次打开面板即可恢复，再校正变化部分。
///
/// 三段式：锁内取快照 → 锁外扫描（最耗时部分）→ 锁内 CAS 写回。
/// 扫描不在锁内进行，多项目并行检索（search_symbols_all）互不阻塞。
pub fn index_project_cached(root: &Path) -> Vec<Symbol> {
    let key = canonical_key(root);
    let now = now_secs();
    // watcher 初始化可能触发底层线程；必须在符号缓存锁之外执行，避免回调反向失效死锁。
    #[cfg(not(test))]
    let watcher_active = crate::services::repo_watcher::ensure_watching(root);
    #[cfg(test)]
    let watcher_active = false;
    let current_git = git_checkpoint(root);
    let previous_git = cache()
        .lock()
        .ok()
        .and_then(|guard| guard.get(&key).and_then(|entry| entry.git_checkpoint.clone()));
    if let (Some(previous), Some(current)) = (&previous_git, &current_git) {
        match git_changed_paths(root, previous, current) {
            Some(paths) if !paths.is_empty() => {
                if !invalidate_files(root, &paths) {
                    request_reconciliation(root);
                }
            }
            Some(_) => {}
            None => request_reconciliation(root),
        }
    }
    // 阶段 1：锁内取快照（克隆 files/syms + 记录 last_sync），冷却期内直接复用。
    let (mut files, mut syms, snap_sync) = {
        let mut guard = cache().lock().unwrap();
        let entry = guard.entry(key.clone()).or_insert_with(|| CacheEntry {
            files: HashMap::new(),
            syms: Vec::new(),
            catalog: CatalogStats::default(),
            git_checkpoint: None,
            needs_reconciliation: false,
            last_sync: 0,
            source: "scan",
        });
        // 条目为空时尝试从磁盘恢复，避免重启后全量重扫
        if entry.files.is_empty() && entry.syms.is_empty() {
            if let Some(persisted) = load_persisted(root) {
                entry.files = persisted.files;
                entry.syms = persisted.syms;
                entry.catalog = persisted.catalog;
                entry.source = "disk";
            }
        }
        entry.git_checkpoint = current_git.clone();
        // 已完成过同步即可复用；空项目/无可识别符号的项目由 watcher 捕获新文件，
        // watcher 不可用时仍按冷却周期扫描。
        if !entry.needs_reconciliation
            && entry.last_sync > 0
            && now.saturating_sub(entry.last_sync)
                < if watcher_active {
                    WATCHER_RECONCILE_SECS
                } else {
                    SYNC_COOLDOWN_SECS
                }
        {
            return entry.syms.clone();
        }
        (entry.files.clone(), entry.syms.clone(), entry.last_sync)
    };
    // 阶段 2（无锁）：walk + 指纹对比 + 只重扫变化文件
    let (_rescanned, _removed, catalog) = sync_incremental(&mut files, &mut syms, root);
    if catalog.persisted {
        let mut indexed_files = files.keys().cloned().collect::<Vec<_>>();
        indexed_files.sort();
        let _ = replace_all_symbol_rows(root, &syms, &indexed_files, catalog.revision);
    }
    // 阶段 3（锁内）：CAS 写回——期间有其他线程同步过（last_sync 变化）则丢弃本地结果。
    // invalidate_files 精确更新同样会推进 last_sync，不会被本阶段覆盖丢失。
    let mut guard = cache().lock().unwrap();
    let out = if let Some(entry) = guard.get_mut(&key) {
        if entry.last_sync == snap_sync {
            entry.files = files;
            entry.syms = syms;
            entry.catalog = catalog;
            entry.git_checkpoint = current_git;
            entry.needs_reconciliation = false;
            entry.last_sync = now;
            save_persisted(root, &entry.files, &entry.syms, entry.catalog);
        }
        entry.syms.clone()
    } else {
        // 条目被并发清空（容量上限）：返回本地计算结果即可
        syms
    };
    // 简单容量上限：超过 16 个项目时清空内存（磁盘缓存不受影响）
    if guard.len() > 16 {
        guard.clear();
    }
    out
}

/// 全盘失效：清内存条目并删除磁盘缓存（手动刷新/强制重建时调用）。
pub fn invalidate_cache(root: &Path) {
    cancel_progressive_indexing(root);
    let key = canonical_key(root);
    if let Ok(mut guard) = cache().lock() {
        guard.remove(&key);
    }
    if let Some(path) = cache_file_for(root) {
        let _ = fs::remove_file(path);
    }
    if let Some(dir) = DATA_DIR.get() {
        let path = catalog_file_at(dir, root);
        for candidate in [
            path.clone(),
            path.with_extension("sqlite3-wal"),
            path.with_extension("sqlite3-shm"),
        ] {
            let _ = fs::remove_file(candidate);
        }
    }
}

/// watcher 的最终一致性闩锁：不在事件线程里做全库扫描，推迟到下一次真实查询。
pub fn request_reconciliation(root: &Path) {
    let key = canonical_key(root);
    if let Ok(mut guard) = cache().lock() {
        if let Some(entry) = guard.get_mut(&key) {
            entry.needs_reconciliation = true;
        }
    }
}

#[cfg(test)]
pub fn reconciliation_pending(root: &Path) -> bool {
    let key = canonical_key(root);
    cache()
        .lock()
        .ok()
        .and_then(|guard| guard.get(&key).map(|entry| entry.needs_reconciliation))
        .unwrap_or(false)
}

/// 增量失效：仅更新指定文件（写/改/删）的符号，其余文件复用缓存。
/// rel 为工具参数中的路径（相对项目根或绝对路径）；目录路径会剔除其下全部文件符号。
/// 内存中无该条目时不做任何事：下次检索会基于最新指纹构建。
pub fn invalidate_files(root: &Path, rels: &[String]) -> bool {
    // SQLite I/O 必须发生在全局内存缓存锁之外，避免慢盘阻塞其他项目查询。
    let catalog_delta = apply_catalog_changes(root, rels);
    let catalog_precise = matches!(catalog_delta, CatalogDelta::Updated(_));
    let key = canonical_key(root);
    let mut guard = cache().lock().unwrap();
    let Some(entry) = guard.get_mut(&key) else {
        return catalog_precise;
    };
    if let CatalogDelta::Updated(stats) = catalog_delta {
        entry.catalog = CatalogStats {
            unreadable_directories: entry.catalog.unreadable_directories,
            ..stats
        };
    } else {
        entry.needs_reconciliation = true;
    }
    let mut changed = false;
    for value in rels {
        let Some((rel_norm, abs)) = normalize_changed_path(root, value) else {
            continue;
        };
        // 目录（含已删除目录，按缓存指纹前缀判断）：剔除其下全部文件
        let prefix = format!("{rel_norm}/");
        let dir_like = abs.is_dir() || entry.files.keys().any(|f| f.starts_with(&prefix));
        if dir_like {
            entry
                .syms
                .retain(|s| s.file != rel_norm && !s.file.starts_with(&prefix));
            entry
                .files
                .retain(|f, _| f != &rel_norm && !f.starts_with(&prefix));
            changed = true;
            continue;
        }
        // 单文件：存在则重扫，不存在则剔除
        let supported = abs
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| SYMBOL_EXTS.contains(&ext));
        match supported.then(|| file_stamp(&abs)).flatten() {
            Some(stamp) => {
                entry.files.insert(rel_norm.clone(), stamp);
                entry.syms.retain(|s| s.file != rel_norm);
                let mut fresh = Vec::new();
                scan_file(&abs, &rel_norm, &mut fresh);
                entry.syms.extend(fresh);
            }
            None => {
                entry.files.remove(&rel_norm);
                entry.syms.retain(|s| s.file != rel_norm);
            }
        }
        changed = true;
    }
    if changed {
        // 推进 last_sync：与 index_project_cached 阶段 3 的 CAS 协调，
        // 防止并发中的锁外扫描写回时覆盖本次精确更新
        entry.last_sync = now_secs();
        save_persisted(root, &entry.files, &entry.syms, entry.catalog);
    }
    let normalized = rels
        .iter()
        .filter_map(|value| normalize_changed_path(root, value).map(|(rel, _)| rel))
        .collect::<Vec<_>>();
    let affected_symbols = entry
        .syms
        .iter()
        .filter(|symbol| {
            normalized.iter().any(|rel| {
                symbol.file == *rel
                    || symbol
                        .file
                        .strip_prefix(rel)
                        .is_some_and(|tail| tail.starts_with('/'))
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    drop(guard);
    let symbols_precise = !changed || replace_changed_symbol_rows(root, rels, &affected_symbols);
    let precise = catalog_precise && symbols_precise;
    if !precise {
        request_reconciliation(root);
    }
    precise
}

/// 路径安全校验：仅项目内相对路径，拒绝越界
#[allow(dead_code)]
fn ensure_inside(root: &Path, rel: &str) -> Result<std::path::PathBuf, String> {
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err("仅支持项目内相对路径".into());
    }
    if p.components().any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_))) {
        return Err("路径越界".into());
    }
    let canonical_root = root.canonicalize().map_err(|e| format!("项目目录不可访问: {e}"))?;
    let target = root
        .join(p)
        .canonicalize()
        .map_err(|e| format!("文件不可读: {e}"))?;
    if !target.starts_with(&canonical_root) {
        return Err("路径越界".into());
    }
    Ok(target)
}

/// 在符号列表中按关键字/类型过滤
pub fn filter_symbols<'a>(syms: &'a [Symbol], query: &str, kind: Option<&str>) -> Vec<&'a Symbol> {
    let q = query.trim().to_lowercase();
    syms.iter()
        .filter(|s| kind.is_none_or(|k| s.kind == k))
        .filter(|s| q.is_empty() || s.name.to_lowercase().contains(&q) || s.file.to_lowercase().contains(&q))
        .take(200)
        .collect()
}

/// 面向 Agent 的结构优先查询结果。保留 Symbol 作为前端兼容模型，同时补齐分页、
/// 覆盖状态和新鲜度，调用方据此决定是否读取具体代码块。
#[derive(Debug, Default, Serialize)]
pub struct ProgressiveIndexStatus {
    pub active: bool,
    pub promoted_this_run: usize,
    pub batches: usize,
    pub last_batch_ms: u64,
    pub last_lock_wait_ms: u64,
    pub throttle_ms: u64,
    pub remaining_files: usize,
}

#[cfg(not(test))]
fn progressive_status(root: &Path, remaining_files: usize) -> ProgressiveIndexStatus {
    let key = canonical_key(root);
    let state = PROGRESSIVE_WORKERS
        .get()
        .and_then(|workers| workers.lock().ok())
        .and_then(|workers| workers.get(&key).cloned());
    match state {
        Some(state) => ProgressiveIndexStatus {
            active: true,
            promoted_this_run: state.promoted.load(Ordering::Relaxed),
            batches: state.batches.load(Ordering::Relaxed),
            last_batch_ms: state.last_batch_ms.load(Ordering::Relaxed),
            last_lock_wait_ms: state.last_lock_wait_ms.load(Ordering::Relaxed),
            throttle_ms: state.throttle_ms.load(Ordering::Relaxed),
            remaining_files,
        },
        None => ProgressiveIndexStatus {
            remaining_files,
            ..ProgressiveIndexStatus::default()
        },
    }
}

#[cfg(test)]
fn progressive_status(_root: &Path, remaining_files: usize) -> ProgressiveIndexStatus {
    ProgressiveIndexStatus {
        remaining_files,
        ..ProgressiveIndexStatus::default()
    }
}

pub(crate) fn semantic_background_ready(root: &Path) -> bool {
    let key = canonical_key(root);
    let catalog_ready = cache()
        .lock()
        .ok()
        .and_then(|entries| entries.get(&key).map(|entry| !entry.needs_reconciliation))
        .unwrap_or(false);
    if !catalog_ready {
        return false;
    }
    #[cfg(not(test))]
    {
        !PROGRESSIVE_WORKERS
            .get()
            .and_then(|workers| workers.lock().ok())
            .is_some_and(|workers| workers.contains_key(&key))
    }
    #[cfg(test)]
    true
}

#[derive(Debug, Serialize)]
pub struct SemanticCoverageStats {
    pub indexed_logic_symbols: usize,
    pub scanned_logic_symbols: usize,
    pub semantic_call_relations: usize,
    pub truncated_targets: usize,
    pub backoff_targets: usize,
    pub coverage_percent: f64,
    pub coverage: String,
}

#[derive(Debug, Clone)]
pub(crate) struct LspSemanticTarget {
    pub path: PathBuf,
    pub name: String,
    pub line: usize,
    pub column: usize,
}

impl SemanticCoverageStats {
    fn from_counts(
        indexed_logic_symbols: usize,
        scanned_logic_symbols: usize,
        semantic_call_relations: usize,
        truncated_targets: usize,
        backoff_targets: usize,
    ) -> Self {
        let scanned_logic_symbols = scanned_logic_symbols.min(indexed_logic_symbols);
        let coverage_percent = if indexed_logic_symbols == 0 {
            0.0
        } else {
            ((scanned_logic_symbols as f64 * 10_000.0 / indexed_logic_symbols as f64).round())
                / 100.0
        };
        let coverage = if indexed_logic_symbols == 0 {
            "not_applicable".into()
        } else if scanned_logic_symbols == 0 {
            "not_started_query_driven".into()
        } else if scanned_logic_symbols == indexed_logic_symbols && truncated_targets == 0 {
            "complete_for_current_index".into()
        } else if truncated_targets > 0 {
            "partial_with_truncated_targets".into()
        } else {
            "partial_query_driven".into()
        };
        Self {
            indexed_logic_symbols,
            scanned_logic_symbols,
            semantic_call_relations,
            truncated_targets,
            backoff_targets,
            coverage_percent,
            coverage,
        }
    }
}

fn persisted_semantic_coverage_at(
    root: &Path,
    data_dir: &Path,
) -> Option<SemanticCoverageStats> {
    let conn = Connection::open(catalog_file_at(data_dir, root)).ok()?;
    conn.query_row(
        "SELECT logic_symbol_count, semantic_target_count,
                semantic_relation_count, semantic_truncated_target_count,
                semantic_failure_target_count
         FROM structure_stats WHERE id=1",
        [],
        |row| {
            Ok(SemanticCoverageStats::from_counts(
                row.get::<_, i64>(0)?.max(0) as usize,
                row.get::<_, i64>(1)?.max(0) as usize,
                row.get::<_, i64>(2)?.max(0) as usize,
                row.get::<_, i64>(3)?.max(0) as usize,
                row.get::<_, i64>(4)?.max(0) as usize,
            ))
        },
    )
    .ok()
}

fn persisted_semantic_coverage(root: &Path) -> Option<SemanticCoverageStats> {
    persisted_semantic_coverage_at(root, DATA_DIR.get()?)
}

fn symbol_name_utf16_column(path: &Path, line0: usize, name: &str) -> Option<usize> {
    let content = fs::read_to_string(path).ok()?;
    let line = content.lines().nth(line0)?;
    line.match_indices(name)
        .find(|(byte, _)| {
            let before = line[..*byte].chars().next_back();
            let after = line[byte + name.len()..].chars().next();
            let is_identifier = |ch: char| ch.is_alphanumeric() || matches!(ch, '_' | '$');
            before.is_none_or(|ch| !is_identifier(ch))
                && after.is_none_or(|ch| !is_identifier(ch))
        })
        .map(|(byte, _)| line[..byte].encode_utf16().count())
}

fn next_lsp_semantic_targets_at(
    root: &Path,
    data_dir: &Path,
    limit: usize,
) -> Vec<LspSemanticTarget> {
    let conn = match Connection::open(catalog_file_at(data_dir, root)) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    let mut statement = match conn.prepare(
        "SELECT s.file, s.name, s.line
         FROM symbols s
         JOIN files f ON f.path=s.file AND f.state='indexed'
         LEFT JOIN semantic_target_scans scan
           ON scan.target_file=s.file AND scan.target_name=s.name
          AND scan.target_line=s.line AND scan.provider='arkts_lsp'
         LEFT JOIN semantic_scan_failures failure
           ON failure.target_file=s.file AND failure.target_name=s.name
          AND failure.target_line=s.line AND failure.provider='arkts_lsp'
         WHERE s.role='logic' AND s.language=?2 AND s.kind=?1
           AND scan.target_file IS NULL
           AND (failure.target_file IS NULL OR failure.retry_after <= ?4)
         ORDER BY s.file, s.line, s.name
         LIMIT ?3",
    ) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    let limit = limit.clamp(1, 64);
    let now = now_secs() as i64;
    let mut targets = Vec::new();
    for kind in ["method", "function"] {
        for language in ["ets", "ts"] {
            let remaining = limit.saturating_sub(targets.len());
            if remaining == 0 {
                break;
            }
            let candidate_limit = remaining.saturating_mul(4).min(256) as i64;
            let rows = match statement.query_map(
                params![kind, language, candidate_limit, now],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?.max(1) as usize,
                    ))
                },
            ) {
                Ok(value) => value,
                Err(_) => return targets,
            };
            for row in rows.filter_map(Result::ok) {
                let (file, name, line1) = row;
                let path = root.join(file);
                let line = line1.saturating_sub(1);
                let Some(column) = symbol_name_utf16_column(&path, line, &name) else {
                    continue;
                };
                targets.push(LspSemanticTarget {
                    path,
                    name,
                    line,
                    column,
                });
                if targets.len() == limit {
                    break;
                }
            }
        }
    }
    targets
}

pub(crate) fn next_lsp_semantic_targets(
    root: &Path,
    limit: usize,
) -> Vec<LspSemanticTarget> {
    let Some(data_dir) = DATA_DIR.get() else {
        return Vec::new();
    };
    next_lsp_semantic_targets_at(root, data_dir, limit)
}

#[derive(Debug, Serialize)]
pub struct StructureQueryResult {
    pub items: Vec<Symbol>,
    /// 与当前页节点相连的结构关系；端点可能位于当前页之外。
    pub relations: Vec<StructureEdge>,
    pub total_matches: usize,
    pub page: usize,
    pub page_size: usize,
    pub next_page: Option<usize>,
    /// Opaque keyset cursor. Prefer this over deep numeric pages on large repositories.
    pub next_cursor: Option<String>,
    pub indexed_files: usize,
    pub indexed_symbols: usize,
    pub indexed_relations: usize,
    pub semantic: SemanticCoverageStats,
    pub scip: crate::services::scip_index::ScipIndexStatus,
    pub catalog: CatalogStats,
    pub watcher_active: bool,
    pub progressive: ProgressiveIndexStatus,
    pub coverage: String,
    pub synced_ago_secs: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct StructureCursor {
    version: u8,
    filter_hash: u64,
    index_revision: u64,
    total_matches: usize,
    exact_match: bool,
    file: String,
    line: i64,
    name: String,
    row_id: i64,
}

fn structure_filter_hash(
    root: &Path,
    query: &str,
    role: Option<&str>,
    kind: Option<&str>,
    file: Option<&str>,
) -> u64 {
    let normalized = format!(
        "{}\0{}\0{}\0{}\0{}",
        canonical_key(root),
        query.trim().to_lowercase(),
        role.unwrap_or("").trim(),
        kind.unwrap_or("").trim(),
        file.unwrap_or("").trim().to_lowercase(),
    );
    stable_hash(&normalized)
}

fn encode_structure_cursor(cursor: &StructureCursor) -> Result<String, String> {
    let payload = serde_json::to_vec(cursor).map_err(|error| format!("编码结构游标失败：{error}"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload))
}

fn decode_structure_cursor(value: &str, expected_filter_hash: u64) -> Result<StructureCursor, String> {
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value.trim())
        .map_err(|_| "结构游标无效或已损坏".to_string())?;
    let cursor: StructureCursor =
        serde_json::from_slice(&payload).map_err(|_| "结构游标格式不受支持".to_string())?;
    if cursor.version != 1 {
        return Err("结构游标版本不受支持，请从第一页重新查询".into());
    }
    if cursor.filter_hash != expected_filter_hash {
        return Err("结构游标与当前项目或过滤条件不匹配，请从第一页重新查询".into());
    }
    Ok(cursor)
}

fn declared_relations_from_json(value: String) -> Vec<DeclaredRelation> {
    serde_json::from_str(&value).unwrap_or_default()
}

fn query_persisted_symbols_at(
    root: &Path,
    data_dir: &Path,
    query: &str,
    role: Option<&str>,
    kind: Option<&str>,
    file: Option<&str>,
    page: usize,
    page_size: usize,
) -> Option<Result<(Vec<Symbol>, usize), String>> {
    let conn = match Connection::open(catalog_file_at(data_dir, root)) {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("打开结构节点库失败：{error}"))),
    };
    let query = query.trim().to_lowercase();
    let role = role.map(str::trim).filter(|value| !value.is_empty()).unwrap_or("");
    let kind = kind.map(str::trim).filter(|value| !value.is_empty()).unwrap_or("");
    let file = file.map(str::trim).filter(|value| !value.is_empty()).unwrap_or("").to_lowercase();
    // 按类型浏览结构图时使用固定谓词，让 kind+file+line 复合索引同时承担过滤和排序。
    if query.is_empty() && role.is_empty() && !kind.is_empty() && file.is_empty() {
        let total = conn
            .query_row(
                "SELECT COUNT(*) FROM symbols WHERE kind=?1",
                params![kind],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            .max(0) as usize;
        let offset = page.saturating_sub(1).saturating_mul(page_size);
        let mut statement = match conn.prepare(
            "SELECT kind, name, file, line, end_line, role, signature, parent, language, source_layer, declared_relations
             FROM symbols WHERE kind=?1
             ORDER BY file, line, name LIMIT ?2 OFFSET ?3",
        ) {
            Ok(value) => value,
            Err(error) => return Some(Err(format!("准备类型结构查询失败：{error}"))),
        };
        let rows = match statement.query_map(
            params![kind, page_size as i64, offset as i64],
            |row| {
                Ok(Symbol {
                    kind: row.get(0)?,
                    name: row.get(1)?,
                    file: row.get(2)?,
                    line: row.get::<_, i64>(3)?.max(0) as usize,
                    end_line: row.get::<_, i64>(4)?.max(0) as usize,
                    role: row.get(5)?,
                    signature: row.get(6)?,
                    parent: row.get(7)?,
                    language: row.get(8)?,
                    source_layer: row.get(9)?,
                    declared_relations: declared_relations_from_json(row.get(10)?),
                })
            },
        ) {
            Ok(value) => value,
            Err(error) => return Some(Err(format!("读取类型结构查询失败：{error}"))),
        };
        let items = match rows.collect::<Result<Vec<_>, _>>() {
            Ok(value) => value,
            Err(error) => return Some(Err(format!("解析类型结构查询失败：{error}"))),
        };
        return Some(Ok((items, total)));
    }
    // Agent 多数情况下会带着结构名继续定位。先走可命中 name 索引的精确路径；
    // 没有精确命中时再保留原有 substring 召回语义。
    if !query.is_empty() {
        let exact_where = "(?1 = '' OR role = ?1)
                           AND (?2 = '' OR kind = ?2)
                           AND (?3 = '' OR instr(lower(file), ?3) > 0)
                           AND name = ?4 COLLATE NOCASE";
        let exact_total = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM symbols WHERE {exact_where}"),
                params![role, kind, file, query],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            .max(0) as usize;
        if exact_total > 0 {
            let offset = page.saturating_sub(1).saturating_mul(page_size);
            let mut statement = match conn.prepare(&format!(
                "SELECT kind, name, file, line, end_line, role, signature, parent, language, source_layer, declared_relations
                 FROM symbols WHERE {exact_where}
                 ORDER BY file, line, name LIMIT ?5 OFFSET ?6"
            )) {
                Ok(value) => value,
                Err(error) => return Some(Err(format!("准备精确结构查询失败：{error}"))),
            };
            let rows = match statement.query_map(
                params![role, kind, file, query, page_size as i64, offset as i64],
                |row| {
                    Ok(Symbol {
                        kind: row.get(0)?,
                        name: row.get(1)?,
                        file: row.get(2)?,
                        line: row.get::<_, i64>(3)?.max(0) as usize,
                        end_line: row.get::<_, i64>(4)?.max(0) as usize,
                        role: row.get(5)?,
                        signature: row.get(6)?,
                        parent: row.get(7)?,
                        language: row.get(8)?,
                        source_layer: row.get(9)?,
                        declared_relations: declared_relations_from_json(row.get(10)?),
                    })
                },
            ) {
                Ok(value) => value,
                Err(error) => return Some(Err(format!("读取精确结构查询失败：{error}"))),
            };
            let items = match rows.collect::<Result<Vec<_>, _>>() {
                Ok(value) => value,
                Err(error) => return Some(Err(format!("解析精确结构查询失败：{error}"))),
            };
            return Some(Ok((items, exact_total)));
        }
    }
    let where_sql = "(?1 = '' OR role = ?1)
                     AND (?2 = '' OR kind = ?2)
                     AND (?3 = '' OR instr(lower(file), ?3) > 0)
                     AND (?4 = '' OR instr(lower(name), ?4) > 0
                                      OR instr(lower(file), ?4) > 0
                                      OR instr(lower(signature), ?4) > 0)";
    let total = match conn.query_row(
        &format!("SELECT COUNT(*) FROM symbols WHERE {where_sql}"),
        params![role, kind, file, query],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(value) => value.max(0) as usize,
        Err(error) => return Some(Err(format!("查询结构节点数量失败：{error}"))),
    };
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    let mut statement = match conn.prepare(&format!(
        "SELECT kind, name, file, line, end_line, role, signature, parent, language, source_layer, declared_relations
         FROM symbols WHERE {where_sql}
         ORDER BY file, line, name LIMIT ?5 OFFSET ?6"
    )) {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("准备结构节点查询失败：{error}"))),
    };
    let rows = match statement.query_map(
        params![role, kind, file, query, page_size as i64, offset as i64],
        |row| {
            Ok(Symbol {
                kind: row.get(0)?,
                name: row.get(1)?,
                file: row.get(2)?,
                line: row.get::<_, i64>(3)?.max(0) as usize,
                end_line: row.get::<_, i64>(4)?.max(0) as usize,
                role: row.get(5)?,
                signature: row.get(6)?,
                parent: row.get(7)?,
                language: row.get(8)?,
                source_layer: row.get(9)?,
                declared_relations: declared_relations_from_json(row.get(10)?),
            })
        },
    ) {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("读取结构节点失败：{error}"))),
    };
    let items = match rows.collect::<Result<Vec<_>, _>>() {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("解析结构节点失败：{error}"))),
    };
    Some(Ok((items, total)))
}

/// Keyset query used by the Agent-facing cursor protocol. Predicates are emitted only when
/// active so SQLite can select the targeted indexes instead of planning around optional ORs.
fn query_persisted_symbols_keyset_at(
    root: &Path,
    data_dir: &Path,
    query: &str,
    role: Option<&str>,
    kind: Option<&str>,
    file: Option<&str>,
    cursor: Option<&StructureCursor>,
    page_size: usize,
    filter_hash: u64,
) -> Option<Result<(Vec<Symbol>, usize, Option<String>), String>> {
    let conn = match Connection::open(catalog_file_at(data_dir, root)) {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("打开结构节点库失败：{error}"))),
    };
    let index_revision = match conn.query_row(
        "SELECT revision FROM structure_meta WHERE id=1",
        [],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(value) => value.max(0) as u64,
        Err(error) => return Some(Err(format!("读取结构索引版本失败：{error}"))),
    };
    if cursor.is_some_and(|value| value.index_revision != index_revision) {
        return Some(Err(
            "结构索引已在翻页期间更新，请从第一页重新查询以避免遗漏或重复".into(),
        ));
    }
    let query = query.trim().to_lowercase();
    let role = role.map(str::trim).filter(|value| !value.is_empty());
    let kind = kind.map(str::trim).filter(|value| !value.is_empty());
    let file = file
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase);
    let mut filters = Vec::<String>::new();
    let mut values = Vec::<Value>::new();
    let push_text = |values: &mut Vec<Value>, value: String| {
        values.push(Value::Text(value));
        values.len()
    };
    if let Some(value) = role {
        let parameter = push_text(&mut values, value.to_string());
        filters.push(format!("role=?{parameter}"));
    }
    if let Some(value) = kind {
        let parameter = push_text(&mut values, value.to_string());
        filters.push(format!("kind=?{parameter}"));
    }
    if let Some(value) = file {
        let parameter = push_text(&mut values, value);
        filters.push(format!("instr(lower(file), ?{parameter}) > 0"));
    }

    let count = |clauses: &[String], parameters: &[Value]| -> Result<usize, String> {
        let where_sql = if clauses.is_empty() {
            "1".to_string()
        } else {
            clauses.join(" AND ")
        };
        conn.query_row(
            &format!("SELECT COUNT(*) FROM symbols WHERE {where_sql}"),
            params_from_iter(parameters.iter()),
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value.max(0) as usize)
        .map_err(|error| format!("查询结构节点数量失败：{error}"))
    };

    let (total, exact_match) = if let Some(cursor) = cursor {
        if !query.is_empty() {
            let query_parameter = push_text(&mut values, query.clone());
            if cursor.exact_match {
                filters.push(format!("name=?{query_parameter} COLLATE NOCASE"));
            } else {
                filters.push(format!(
                    "(instr(lower(name), ?{query_parameter}) > 0
                       OR instr(lower(file), ?{query_parameter}) > 0
                       OR instr(lower(signature), ?{query_parameter}) > 0)"
                ));
            }
        }
        (cursor.total_matches, cursor.exact_match)
    } else if query.is_empty() {
        let total = match count(&filters, &values) {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        (total, false)
    } else {
        let query_parameter = push_text(&mut values, query.clone());
        let mut exact_filters = filters.clone();
        exact_filters.push(format!("name=?{query_parameter} COLLATE NOCASE"));
        let exact_total = match count(&exact_filters, &values) {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        if exact_total > 0 {
            filters = exact_filters;
            (exact_total, true)
        } else {
            filters.push(format!(
                "(instr(lower(name), ?{query_parameter}) > 0
                   OR instr(lower(file), ?{query_parameter}) > 0
                   OR instr(lower(signature), ?{query_parameter}) > 0)"
            ));
            let total = match count(&filters, &values) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            (total, false)
        }
    };

    if let Some(cursor) = cursor {
        let file_parameter = push_text(&mut values, cursor.file.clone());
        values.push(Value::Integer(cursor.line));
        let line_parameter = values.len();
        let name_parameter = push_text(&mut values, cursor.name.clone());
        values.push(Value::Integer(cursor.row_id));
        let id_parameter = values.len();
        filters.push(format!(
            "(file, line, name, id) >
             (?{file_parameter}, ?{line_parameter}, ?{name_parameter}, ?{id_parameter})"
        ));
    }
    values.push(Value::Integer(page_size.saturating_add(1) as i64));
    let limit_parameter = values.len();
    let where_sql = if filters.is_empty() {
        "1".to_string()
    } else {
        filters.join(" AND ")
    };
    let mut statement = match conn.prepare(&format!(
        "SELECT kind, name, file, line, end_line, role, signature, parent, language, source_layer, declared_relations, id
         FROM symbols WHERE {where_sql}
         ORDER BY file, line, name, id LIMIT ?{limit_parameter}"
    )) {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("准备游标结构查询失败：{error}"))),
    };
    let rows = match statement.query_map(params_from_iter(values.iter()), |row| {
        Ok((
            Symbol {
                kind: row.get(0)?,
                name: row.get(1)?,
                file: row.get(2)?,
                line: row.get::<_, i64>(3)?.max(0) as usize,
                end_line: row.get::<_, i64>(4)?.max(0) as usize,
                role: row.get(5)?,
                signature: row.get(6)?,
                parent: row.get(7)?,
                language: row.get(8)?,
                source_layer: row.get(9)?,
                declared_relations: declared_relations_from_json(row.get(10)?),
            },
            row.get::<_, i64>(11)?,
        ))
    }) {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("读取游标结构查询失败：{error}"))),
    };
    let mut rows = match rows.collect::<Result<Vec<_>, _>>() {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("解析游标结构查询失败：{error}"))),
    };
    let has_more = rows.len() > page_size;
    rows.truncate(page_size);
    let next_cursor = if has_more {
        rows.last().map(|(symbol, row_id)| {
            encode_structure_cursor(&StructureCursor {
                version: 1,
                filter_hash,
                index_revision,
                total_matches: total,
                exact_match,
                file: symbol.file.clone(),
                line: symbol.line as i64,
                name: symbol.name.clone(),
                row_id: *row_id,
            })
        }).transpose()
    } else {
        Ok(None)
    };
    let next_cursor = match next_cursor {
        Ok(value) => value,
        Err(error) => return Some(Err(error)),
    };
    Some(Ok((
        rows.into_iter().map(|(symbol, _)| symbol).collect(),
        total,
        next_cursor,
    )))
}

fn query_persisted_symbols(
    root: &Path,
    query: &str,
    role: Option<&str>,
    kind: Option<&str>,
    file: Option<&str>,
    page: usize,
    page_size: usize,
) -> Option<Result<(Vec<Symbol>, usize), String>> {
    let data_dir = DATA_DIR.get()?;
    query_persisted_symbols_at(root, data_dir, query, role, kind, file, page, page_size)
}

fn normalize_project_path(base: &str, value: &str) -> Option<String> {
    if value.contains('\\') || value.starts_with('/') {
        return None;
    }
    let mut parts = base
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>();
    for part in value.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return None;
                }
            }
            value if value != "." && value != ".." => parts.push(value),
            _ => return None,
        }
    }
    let normalized = parts.join("/");
    (!normalized.is_empty()).then_some(normalized)
}

fn source_parent(source_file: &str) -> &str {
    source_file.rsplit_once('/').map(|(parent, _)| parent).unwrap_or("")
}

fn module_file_candidates(base: &str) -> Vec<String> {
    if base.is_empty() {
        return Vec::new();
    }
    if Path::new(&base).extension().is_some() {
        return vec![base.to_string()];
    }
    let mut candidates = Vec::new();
    for extension in ["ets", "ts", "tsx", "js", "jsx"] {
        candidates.push(format!("{base}.{extension}"));
        candidates.push(format!("{base}/index.{extension}"));
    }
    candidates
}

#[derive(Debug, Clone)]
struct TsconfigPathRule {
    pattern: String,
    replacements: Vec<String>,
}

#[derive(Debug, Clone)]
struct TsconfigAliases {
    base_dir: String,
    rules: Vec<TsconfigPathRule>,
}

#[derive(Debug, Clone)]
struct OhpmLocalAlias {
    owner_dir: String,
    package_name: String,
    entry_base: String,
}

#[derive(Debug, Clone, Default)]
struct ModuleAliases {
    tsconfig: Option<TsconfigAliases>,
    ohpm: Vec<OhpmLocalAlias>,
}

fn load_tsconfig_aliases(root: &Path) -> Option<TsconfigAliases> {
    let path = root.join("tsconfig.json");
    let content = fs::read_to_string(path).ok()?;
    let value = crate::services::harmony::parse_json5(&content).ok()?;
    let compiler = value.get("compilerOptions")?.as_object()?;
    let base_url = compiler
        .get("baseUrl")
        .and_then(|value| value.as_str())
        .unwrap_or(".");
    let base_dir = normalize_project_path("", base_url).unwrap_or_default();
    let paths = compiler.get("paths")?.as_object()?;
    let mut rules = Vec::new();
    for (pattern, replacements) in paths {
        let replacements = replacements
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect::<Vec<_>>();
        if !replacements.is_empty() && pattern.matches('*').count() <= 1 {
            rules.push(TsconfigPathRule { pattern: pattern.clone(), replacements });
        }
    }
    (!rules.is_empty()).then_some(TsconfigAliases { base_dir, rules })
}

fn load_module_aliases<'a>(
    root: &Path,
    source_files: impl Iterator<Item = &'a str>,
) -> ModuleAliases {
    let mut manifest_dirs = source_files
        .flat_map(|source_file| {
            let mut dirs = Vec::new();
            let mut current = source_parent(source_file);
            loop {
                dirs.push(current.to_string());
                let Some((parent, _)) = current.rsplit_once('/') else {
                    break;
                };
                current = parent;
            }
            dirs
        })
        .collect::<Vec<_>>();
    manifest_dirs.push(String::new());
    manifest_dirs.sort();
    manifest_dirs.dedup();

    let mut ohpm = Vec::new();
    for owner_dir in manifest_dirs {
        let manifest = if owner_dir.is_empty() {
            root.join("oh-package.json5")
        } else {
            root.join(&owner_dir).join("oh-package.json5")
        };
        let Some(value) = fs::read_to_string(manifest)
            .ok()
            .and_then(|content| crate::services::harmony::parse_json5(&content).ok())
        else {
            continue;
        };
        for scope in ["dependencies", "devDependencies", "dynamicDependencies"] {
            let Some(dependencies) = value.get(scope).and_then(|value| value.as_object()) else {
                continue;
            };
            for (package_name, requirement) in dependencies {
                let Some(requirement) = requirement.as_str() else {
                    continue;
                };
                let Some(raw_target) = requirement
                    .strip_prefix("file:")
                    .or_else(|| requirement.strip_prefix("link:"))
                else {
                    continue;
                };
                let Some(target_dir) = normalize_project_path(&owner_dir, raw_target) else {
                    continue;
                };
                let target_manifest = root.join(&target_dir).join("oh-package.json5");
                let Some(target) = fs::read_to_string(target_manifest)
                    .ok()
                    .and_then(|content| crate::services::harmony::parse_json5(&content).ok())
                else {
                    continue;
                };
                let Some(main) = target.get("main").and_then(|value| value.as_str()) else {
                    continue;
                };
                let Some(entry_base) = normalize_project_path(&target_dir, main) else {
                    continue;
                };
                ohpm.push(OhpmLocalAlias {
                    owner_dir: owner_dir.clone(),
                    package_name: package_name.clone(),
                    entry_base,
                });
            }
        }
    }
    ohpm.sort_by(|a, b| {
        (&a.owner_dir, &a.package_name, &a.entry_base).cmp(&(
            &b.owner_dir,
            &b.package_name,
            &b.entry_base,
        ))
    });
    ohpm.dedup_by(|a, b| {
        a.owner_dir == b.owner_dir
            && a.package_name == b.package_name
            && a.entry_base == b.entry_base
    });
    ModuleAliases {
        tsconfig: load_tsconfig_aliases(root),
        ohpm,
    }
}

fn alias_replacements(aliases: &TsconfigAliases, module_specifier: &str) -> Vec<String> {
    let exact = aliases
        .rules
        .iter()
        .filter(|rule| !rule.pattern.contains('*') && rule.pattern == module_specifier)
        .collect::<Vec<_>>();
    let matched = if exact.len() == 1 {
        exact
    } else if exact.is_empty() {
        let wildcard = aliases
            .rules
            .iter()
            .filter_map(|rule| {
                let (prefix, suffix) = rule.pattern.split_once('*')?;
                module_specifier
                    .strip_prefix(prefix)?
                    .strip_suffix(suffix)
                    .map(|capture| (rule, capture, prefix.len() + suffix.len()))
            })
            .collect::<Vec<_>>();
        let Some(best) = wildcard.iter().map(|(_, _, score)| *score).max() else {
            return Vec::new();
        };
        let best = wildcard
            .into_iter()
            .filter(|(_, _, score)| *score == best)
            .collect::<Vec<_>>();
        if best.len() != 1 {
            return Vec::new();
        }
        let (rule, capture, _) = best[0];
        return rule
            .replacements
            .iter()
            .filter_map(|replacement| {
                normalize_project_path(
                    &aliases.base_dir,
                    &replacement.replacen('*', capture, 1),
                )
            })
            .collect();
    } else {
        return Vec::new();
    };
    matched[0]
        .replacements
        .iter()
        .filter_map(|replacement| normalize_project_path(&aliases.base_dir, replacement))
        .collect()
}

fn module_candidates(
    source_file: &str,
    module_specifier: &str,
    aliases: Option<&ModuleAliases>,
) -> Vec<String> {
    let bases = if module_specifier.starts_with('.') {
        normalize_project_path(source_parent(source_file), module_specifier)
            .into_iter()
            .collect::<Vec<_>>()
    } else {
        aliases
            .map(|config| {
                let tsconfig = config
                    .tsconfig
                    .as_ref()
                    .map(|tsconfig| alias_replacements(tsconfig, module_specifier))
                    .unwrap_or_default();
                if !tsconfig.is_empty() {
                    return tsconfig;
                }
                let source_dir = source_parent(source_file);
                let best_scope = config
                    .ohpm
                    .iter()
                    .filter(|alias| {
                        alias.package_name == module_specifier
                            && (alias.owner_dir.is_empty()
                                || source_dir == alias.owner_dir
                                || source_dir
                                    .strip_prefix(&alias.owner_dir)
                                    .is_some_and(|tail| tail.starts_with('/')))
                    })
                    .map(|alias| alias.owner_dir.len())
                    .max();
                best_scope
                    .into_iter()
                    .flat_map(|scope_len| {
                        config.ohpm.iter().filter(move |alias| {
                            alias.package_name == module_specifier
                                && alias.owner_dir.len() == scope_len
                                && (alias.owner_dir.is_empty()
                                    || source_dir == alias.owner_dir
                                    || source_dir
                                        .strip_prefix(&alias.owner_dir)
                                        .is_some_and(|tail| tail.starts_with('/')))
                        })
                    })
                    .map(|alias| alias.entry_base.clone())
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut candidates = bases
        .iter()
        .flat_map(|base| module_file_candidates(base))
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();
    candidates
}

fn resolve_module_file(
    conn: &Connection,
    source_file: &str,
    module_specifier: &str,
    aliases: Option<&ModuleAliases>,
) -> Option<String> {
    let candidates = module_candidates(source_file, module_specifier, aliases);
    if candidates.is_empty() {
        return None;
    }
    let placeholders = (1..=candidates.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut file_statement = match conn.prepare(&format!(
        "SELECT path FROM files WHERE path IN ({placeholders}) ORDER BY path LIMIT 2"
    )) {
        Ok(value) => value,
        Err(_) => return None,
    };
    let existing = match file_statement
        .query_map(params_from_iter(candidates.iter()), |row| row.get::<_, String>(0))
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
    {
        Ok(value) => value,
        Err(_) => return None,
    };
    if existing.len() != 1 {
        return None;
    }
    existing.into_iter().next()
}

fn resolve_exported_target(
    root: &Path,
    conn: &Connection,
    aliases: Option<&ModuleAliases>,
    source_file: &str,
    module_specifier: &str,
    imported_name: &str,
    relation_kind: &str,
    depth: usize,
    visited: &mut Vec<(String, String, String)>,
    remaining_visits: &mut usize,
) -> Option<(String, String, usize)> {
    if depth >= MAX_REEXPORT_DEPTH || *remaining_visits == 0 {
        return None;
    }
    *remaining_visits -= 1;
    let key = (
        source_file.to_string(),
        module_specifier.to_string(),
        imported_name.to_string(),
    );
    if visited.contains(&key) {
        return None;
    }
    visited.push(key);
    let owned_aliases = (depth > 0)
        .then(|| load_module_aliases(root, std::iter::once(source_file)));
    let effective_aliases = if depth == 0 {
        aliases
    } else {
        owned_aliases.as_ref()
    };
    let target_file =
        resolve_module_file(conn, source_file, module_specifier, effective_aliases)?;
    let lines = conn
        .prepare(
            "SELECT line FROM symbols
             WHERE file=?1 AND name=?2
               AND ((?3='calls' AND kind='function')
                    OR (?3<>'calls' AND role='entity'))
             ORDER BY line LIMIT 2",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![target_file, imported_name, relation_kind], |row| {
                    row.get::<_, i64>(0)
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();
    if lines.len() == 1 {
        return Some((
            target_file,
            imported_name.to_string(),
            lines[0].max(0) as usize,
        ));
    }
    if !lines.is_empty() {
        return None;
    }
    let named = conn
        .prepare(
            "SELECT target_module, imported_name FROM module_reexports
             WHERE source_file=?1 AND exported_name=?2
             ORDER BY target_module, imported_name LIMIT 2",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![target_file, imported_name], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();
    if named.len() == 1 {
        let (next_module, next_name) = &named[0];
        return resolve_exported_target(
            root,
            conn,
            aliases,
            &target_file,
            next_module,
            next_name,
            relation_kind,
            depth + 1,
            visited,
            remaining_visits,
        );
    }
    if !named.is_empty() {
        return None;
    }
    let stars = conn
        .prepare(
            "SELECT target_module FROM module_reexports
             WHERE source_file=?1 AND exported_name='*' AND imported_name='*'
             ORDER BY target_module LIMIT ?2",
        )
        .and_then(|mut statement| {
            statement
                .query_map(
                    params![target_file, (MAX_REEXPORT_BRANCHES + 1) as i64],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();
    if stars.is_empty() || stars.len() > MAX_REEXPORT_BRANCHES {
        return None;
    }
    let mut resolved = Vec::new();
    for next_module in stars {
        let mut branch_visited = visited.clone();
        if let Some(target) = resolve_exported_target(
            root,
            conn,
            aliases,
            &target_file,
            &next_module,
            imported_name,
            relation_kind,
            depth + 1,
            &mut branch_visited,
            remaining_visits,
        ) {
            resolved.push(target);
        }
    }
    resolved.sort();
    resolved.dedup();
    (resolved.len() == 1).then(|| resolved.remove(0))
}

fn resolve_import_target_from_catalog(
    root: &Path,
    conn: &Connection,
    aliases: Option<&ModuleAliases>,
    edge: &mut StructureEdge,
) {
    let (Some(module_specifier), Some(imported_name)) = (
        edge.target_module.as_deref(),
        edge.target_imported_name.as_deref(),
    ) else {
        return;
    };
    let mut remaining_visits = MAX_REEXPORT_VISITS;
    if let Some((target_file, target_name, target_line)) = resolve_exported_target(
        root,
        conn,
        aliases,
        &edge.source_file,
        module_specifier,
        imported_name,
        &edge.kind,
        0,
        &mut Vec::new(),
        &mut remaining_visits,
    ) {
        edge.target_file = target_file;
        edge.target_name = target_name;
        edge.target_line = target_line;
    } else {
        edge.target_file.clear();
        edge.target_line = 0;
    }
}

fn query_persisted_edges_at(
    root: &Path,
    data_dir: &Path,
    symbols: &[Symbol],
) -> Option<Result<(Vec<StructureEdge>, usize), String>> {
    let conn = match Connection::open(catalog_file_at(data_dir, root)) {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("打开结构关系库失败：{error}"))),
    };
    let mut total = match conn.query_row("SELECT relation_count + semantic_relation_count
                                      FROM structure_stats WHERE id=1", [], |row| {
        row.get::<_, i64>(0)
    }) {
        Ok(value) => value.max(0) as usize,
        Err(error) => return Some(Err(format!("查询结构关系数量失败：{error}"))),
    };
    let mut statement = match conn.prepare(
        "SELECT kind, source_file, source_name, source_line,
                target_file, target_name, target_line,
                target_module, target_imported_name
         FROM symbol_edges
         WHERE (source_file = ?1 AND source_name = ?2 AND source_line = ?3)
            OR (target_file = ?1 AND target_name = ?2)
            OR (target_module IS NOT NULL AND target_name = ?2)",
    ) {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("准备结构关系查询失败：{error}"))),
    };
    let mut edges = Vec::new();
    for symbol in symbols {
        let rows = match statement.query_map(
            params![symbol.file, symbol.name, symbol.line as i64],
            |row| {
                Ok(StructureEdge {
                    kind: row.get(0)?,
                    source_file: row.get(1)?,
                    source_name: row.get(2)?,
                    source_line: row.get::<_, i64>(3)?.max(0) as usize,
                    target_file: row.get(4)?,
                    target_name: row.get(5)?,
                    target_line: row.get::<_, i64>(6)?.max(0) as usize,
                    target_module: row.get(7)?,
                    target_imported_name: row.get(8)?,
                })
            },
        ) {
            Ok(value) => value,
            Err(error) => return Some(Err(format!("读取结构关系失败：{error}"))),
        };
        for row in rows {
            match row {
                Ok(edge) => edges.push(edge),
                Err(error) => return Some(Err(format!("解析结构关系失败：{error}"))),
            }
        }
    }
    let mut semantic_statement = match conn.prepare(
        "SELECT e.source_file, e.source_name, e.source_line,
                e.target_file, e.target_name, e.target_line
         FROM semantic_call_edges e
         WHERE EXISTS (
           SELECT 1 FROM files f
           WHERE f.path=e.source_file AND f.state='indexed'
             AND f.size=e.source_size AND f.mtime_ns=e.source_mtime_ns
         )
         AND EXISTS (
           SELECT 1 FROM symbols source
           WHERE source.file=e.source_file AND source.name=e.source_name
             AND source.line=e.source_line AND source.role='logic'
         )
         AND EXISTS (
           SELECT 1 FROM symbols target
           WHERE target.file=e.target_file AND target.name=e.target_name
             AND target.line=e.target_line AND target.role='logic'
         )
         AND ((e.source_file=?1 AND e.source_name=?2 AND e.source_line=?3)
           OR (e.target_file=?1 AND e.target_name=?2 AND e.target_line=?3))",
    ) {
        Ok(value) => value,
        Err(error) => return Some(Err(format!("准备语义调用关系查询失败：{error}"))),
    };
    for symbol in symbols {
        let rows = match semantic_statement.query_map(
            params![symbol.file, symbol.name, symbol.line as i64],
            |row| {
                Ok(StructureEdge {
                    kind: "calls".into(),
                    source_file: row.get(0)?,
                    source_name: row.get(1)?,
                    source_line: row.get::<_, i64>(2)?.max(0) as usize,
                    target_file: row.get(3)?,
                    target_name: row.get(4)?,
                    target_line: row.get::<_, i64>(5)?.max(0) as usize,
                    target_module: None,
                    target_imported_name: None,
                })
            },
        ) {
            Ok(value) => value,
            Err(error) => return Some(Err(format!("读取语义调用关系失败：{error}"))),
        };
        for row in rows {
            match row {
                Ok(edge) => edges.push(edge),
                Err(error) => return Some(Err(format!("解析语义调用关系失败：{error}"))),
            }
        }
    }
    let has_scip = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='scip_reference_edges')",
        [], |row| row.get::<_, bool>(0),
    ).unwrap_or(false);
    if has_scip {
        total = total.saturating_add(conn.query_row(
            "SELECT COALESCE(edge_count, 0) FROM scip_import_state WHERE id=1",
            [], |row| row.get::<_, i64>(0),
        ).unwrap_or(0).max(0) as usize);
        let mut scip_statement = match conn.prepare(
            "SELECT e.source_file, e.source_name, e.source_line,
                    e.target_file, e.target_name, e.target_line
             FROM scip_reference_edges e JOIN scip_import_state state
               ON state.id=1 AND state.active_import_id=e.import_id
             WHERE EXISTS (SELECT 1 FROM files f WHERE f.path=e.source_file AND f.state='indexed'
               AND f.size=e.source_size AND f.mtime_ns=e.source_mtime_ns)
               AND EXISTS (SELECT 1 FROM files f WHERE f.path=e.target_file AND f.state='indexed'
               AND f.size=e.target_size AND f.mtime_ns=e.target_mtime_ns)
               AND ((e.source_file=?1 AND e.source_name=?2 AND e.source_line=?3)
                 OR (e.target_file=?1 AND e.target_name=?2 AND e.target_line=?3))"
        ) {
            Ok(value) => value,
            Err(error) => return Some(Err(format!("准备 SCIP 引用查询失败：{error}"))),
        };
        for symbol in symbols {
            let rows = match scip_statement.query_map(
                params![symbol.file, symbol.name, symbol.line as i64],
                |row| Ok(StructureEdge {
                    kind: "references".into(),
                    source_file: row.get(0)?, source_name: row.get(1)?,
                    source_line: row.get::<_, i64>(2)?.max(0) as usize,
                    target_file: row.get(3)?, target_name: row.get(4)?,
                    target_line: row.get::<_, i64>(5)?.max(0) as usize,
                    target_module: None, target_imported_name: None,
                }),
            ) {
                Ok(value) => value,
                Err(error) => return Some(Err(format!("读取 SCIP 引用失败：{error}"))),
            };
            for row in rows {
                match row {
                    Ok(edge) => edges.push(edge),
                    Err(error) => return Some(Err(format!("解析 SCIP 引用失败：{error}"))),
                }
            }
        }
    }
    let aliases = load_module_aliases(root, edges.iter().map(|edge| edge.source_file.as_str()));
    for edge in &mut edges {
        resolve_import_target_from_catalog(root, &conn, Some(&aliases), edge);
    }
    edges.retain(|edge| {
        symbols.iter().any(|symbol| {
            (symbol.file == edge.source_file
                && symbol.name == edge.source_name
                && symbol.line == edge.source_line)
                || (symbol.file == edge.target_file
                    && symbol.name == edge.target_name
                    && symbol.line == edge.target_line)
        })
    });
    edges.sort();
    edges.dedup();
    Some(Ok((edges, total)))
}

fn query_persisted_edges(
    root: &Path,
    symbols: &[Symbol],
) -> Option<Result<(Vec<StructureEdge>, usize), String>> {
    let data_dir = DATA_DIR.get()?;
    query_persisted_edges_at(root, data_dir, symbols)
}

fn utf16_column_to_byte(line: &str, utf16_column: usize) -> usize {
    let mut units = 0usize;
    for (byte, ch) in line.char_indices() {
        if units >= utf16_column {
            return byte;
        }
        units += ch.len_utf16();
        if units > utf16_column {
            return byte;
        }
    }
    line.len()
}

fn is_call_callee_position(path: &Path, line: usize, utf16_column: usize) -> bool {
    !member_call_callee_positions(path, &[(line, utf16_column)]).is_empty()
}

fn member_call_callee_positions(
    path: &Path,
    positions: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
        return Vec::new();
    };
    let Some(language) = tree_sitter_language(ext) else {
        return Vec::new();
    };
    let Some(content) = fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.len() <= MAX_BYTES)
        .and_then(|_| fs::read_to_string(path).ok())
    else {
        return Vec::new();
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(&content, None) else {
        return Vec::new();
    };
    let lines = content.lines().collect::<Vec<_>>();
    positions
        .iter()
        .copied()
        .filter(|(line, utf16_column)| {
            let Some(line_text) = lines.get(*line) else {
                return false;
            };
            let point = tree_sitter::Point::new(
                *line,
                utf16_column_to_byte(line_text, *utf16_column),
            );
            let Some(mut node) = tree.root_node().descendant_for_point_range(point, point) else {
                return false;
            };
            loop {
                if node.kind() == "call_expression" {
                    return node
                        .child_by_field_name("function")
                        .filter(|function| function.kind() == "member_expression")
                        .and_then(|function| function.child_by_field_name("property"))
                        .is_some_and(|property| {
                            matches!(
                                property.kind(),
                                "property_identifier" | "private_property_identifier"
                            ) && property.start_position() <= point
                                && point <= property.end_position()
                        });
                }
                let Some(parent) = node.parent() else {
                    return false;
                };
                node = parent;
            }
        })
        .collect()
}

fn indexed_stamp_matches(conn: &Connection, rel: &str, stamp: FileStamp) -> bool {
    conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM files
           WHERE path=?1 AND state='indexed' AND size=?2 AND mtime_ns=?3
         )",
        params![rel, stamp.len as i64, stamp.mtime as i64],
        |row| row.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

fn logic_symbol_at(conn: &Connection, rel: &str, line0: usize) -> Option<(String, i64)> {
    let position = line0.saturating_add(1) as i64;
    conn.query_row(
        "SELECT name, line FROM symbols
         WHERE file=?1 AND role='logic' AND line<=?2 AND end_line>=?2
         ORDER BY CASE WHEN line=?2 THEN 0 ELSE 1 END,
                  (end_line-line), line DESC LIMIT 1",
        params![rel, position],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )
    .ok()
}

fn insert_semantic_call(
    conn: &Connection,
    source_rel: &str,
    source_name: &str,
    source_symbol_line: i64,
    source_line: usize,
    source_column: usize,
    source_stamp: FileStamp,
    target_rel: &str,
    target_name: &str,
    target_symbol_line: i64,
) -> bool {
    conn.execute(
        "INSERT INTO semantic_call_edges(
           source_file, source_name, source_line, call_line, call_column,
           source_size, source_mtime_ns, target_file, target_name, target_line, provider
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'arkts_lsp')
         ON CONFLICT(source_file, call_line, call_column, provider) DO UPDATE SET
           source_name=excluded.source_name, source_line=excluded.source_line,
           source_size=excluded.source_size, source_mtime_ns=excluded.source_mtime_ns,
           target_file=excluded.target_file, target_name=excluded.target_name,
           target_line=excluded.target_line",
        params![
            source_rel,
            source_name,
            source_symbol_line,
            source_line.saturating_add(1) as i64,
            source_column.saturating_add(1) as i64,
            source_stamp.len as i64,
            source_stamp.mtime as i64,
            target_rel,
            target_name,
            target_symbol_line,
        ],
    )
    .is_ok()
}

fn project_relative(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .filter(|rel| !rel.is_empty())
}

const LSP_SCAN_BACKOFF_SECS: [u64; 8] = [30, 60, 120, 300, 600, 1_800, 3_600, 21_600];

fn record_lsp_scan_failure_at(
    root: &Path,
    data_dir: &Path,
    target_path: &Path,
    target_line: usize,
) -> u64 {
    let Some(target_rel) = project_relative(root, target_path) else {
        return 0;
    };
    let Some(target_stamp) = file_stamp(target_path) else {
        return 0;
    };
    let mut conn = match Connection::open(catalog_file_at(data_dir, root)) {
        Ok(value) => value,
        Err(_) => return 0,
    };
    if !indexed_stamp_matches(&conn, &target_rel, target_stamp) {
        return 0;
    }
    let transaction = match conn.transaction_with_behavior(TransactionBehavior::Immediate) {
        Ok(value) => value,
        Err(_) => return 0,
    };
    if file_stamp(target_path) != Some(target_stamp)
        || !indexed_stamp_matches(&transaction, &target_rel, target_stamp)
    {
        return 0;
    }
    let Some((target_name, target_symbol_line)) =
        logic_symbol_at(&transaction, &target_rel, target_line)
    else {
        return 0;
    };
    let previous = transaction
        .query_row(
            "SELECT failure_count, target_size, target_mtime_ns
             FROM semantic_scan_failures
             WHERE target_file=?1 AND target_name=?2 AND target_line=?3
               AND provider='arkts_lsp'",
            params![target_rel, target_name, target_symbol_line],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
        )
        .ok();
    let failure_count = previous
        .filter(|(_, size, mtime)| {
            *size == target_stamp.len as i64 && *mtime == target_stamp.mtime as i64
        })
        .map(|(count, _, _)| count.max(0) as usize + 1)
        .unwrap_or(1)
        .min(LSP_SCAN_BACKOFF_SECS.len());
    let delay = LSP_SCAN_BACKOFF_SECS[failure_count - 1];
    let attempted_at = now_secs();
    if transaction
        .execute(
            "INSERT INTO semantic_scan_failures(
               target_file, target_name, target_line, target_size, target_mtime_ns,
               provider, failure_count, last_attempt_at, retry_after
             ) VALUES(?1, ?2, ?3, ?4, ?5, 'arkts_lsp', ?6, ?7, ?8)
             ON CONFLICT(target_file, target_name, target_line, provider) DO UPDATE SET
               target_size=excluded.target_size,
               target_mtime_ns=excluded.target_mtime_ns,
               failure_count=excluded.failure_count,
               last_attempt_at=excluded.last_attempt_at,
               retry_after=excluded.retry_after",
            params![
                target_rel,
                target_name,
                target_symbol_line,
                target_stamp.len as i64,
                target_stamp.mtime as i64,
                failure_count as i64,
                attempted_at as i64,
                attempted_at.saturating_add(delay) as i64,
            ],
        )
        .is_err()
    {
        return 0;
    }
    if transaction.commit().is_ok() { delay } else { 0 }
}

fn record_lsp_call_references_at(
    root: &Path,
    data_dir: &Path,
    target_path: &Path,
    target_line: usize,
    references: &[(PathBuf, usize, usize)],
    truncated: bool,
) -> usize {
    let Some(target_rel) = project_relative(root, target_path) else {
        return 0;
    };
    let Some(target_stamp) = file_stamp(target_path) else {
        return 0;
    };
    let mut conn = match Connection::open(catalog_file_at(data_dir, root)) {
        Ok(value) => value,
        Err(_) => return 0,
    };
    if !indexed_stamp_matches(&conn, &target_rel, target_stamp) {
        return 0;
    }
    let mut by_file = HashMap::<PathBuf, Vec<(usize, usize)>>::new();
    for (path, line, column) in references {
        if project_relative(root, path).is_some() {
            by_file.entry(path.clone()).or_default().push((*line, *column));
        }
    }
    for positions in by_file.values_mut() {
        positions.sort_unstable();
        positions.dedup();
    }

    let mut valid_sources = Vec::new();
    for (source_path, positions) in by_file {
        let Some(source_rel) = project_relative(root, &source_path) else {
            continue;
        };
        let Some(source_stamp) = file_stamp(&source_path) else {
            continue;
        };
        if !indexed_stamp_matches(&conn, &source_rel, source_stamp) {
            continue;
        }
        let valid_positions = member_call_callee_positions(&source_path, &positions);
        if !valid_positions.is_empty() {
            valid_sources.push((source_path, source_rel, source_stamp, valid_positions));
        }
    }

    let transaction = match conn.transaction_with_behavior(TransactionBehavior::Immediate) {
        Ok(value) => value,
        Err(_) => return 0,
    };
    if file_stamp(target_path) != Some(target_stamp)
        || !indexed_stamp_matches(&transaction, &target_rel, target_stamp)
    {
        return 0;
    }
    let Some((target_name, target_symbol_line)) =
        logic_symbol_at(&transaction, &target_rel, target_line)
    else {
        return 0;
    };
    if transaction
        .execute(
            "DELETE FROM semantic_scan_failures
             WHERE target_file=?1 AND target_name=?2 AND target_line=?3
               AND provider='arkts_lsp'",
            params![target_rel, target_name, target_symbol_line],
        )
        .is_err()
    {
        return 0;
    }
    let mut recorded = 0usize;
    for (source_path, source_rel, source_stamp, positions) in valid_sources {
        if file_stamp(&source_path) != Some(source_stamp)
            || !indexed_stamp_matches(&transaction, &source_rel, source_stamp)
        {
            continue;
        }
        for (source_line, source_column) in positions {
            let Some((source_name, source_symbol_line)) =
                logic_symbol_at(&transaction, &source_rel, source_line)
            else {
                continue;
            };
            if insert_semantic_call(
                &transaction,
                &source_rel,
                &source_name,
                source_symbol_line,
                source_line,
                source_column,
                source_stamp,
                &target_rel,
                &target_name,
                target_symbol_line,
            ) {
                recorded += 1;
            }
        }
    }
    if transaction
        .execute(
            "INSERT INTO semantic_target_scans(
               target_file, target_name, target_line, target_size, target_mtime_ns,
               provider, scanned_at, reference_count, recorded_call_count, truncated
             ) VALUES(?1, ?2, ?3, ?4, ?5, 'arkts_lsp', ?6, ?7, ?8, ?9)
             ON CONFLICT(target_file, target_name, target_line, provider) DO UPDATE SET
               target_size=excluded.target_size,
               target_mtime_ns=excluded.target_mtime_ns,
               scanned_at=excluded.scanned_at,
               reference_count=excluded.reference_count,
               recorded_call_count=excluded.recorded_call_count,
               truncated=excluded.truncated",
            params![
                target_rel,
                target_name,
                target_symbol_line,
                target_stamp.len as i64,
                target_stamp.mtime as i64,
                now_secs() as i64,
                references.len() as i64,
                recorded as i64,
                i64::from(truncated),
            ],
        )
        .is_err()
    {
        return 0;
    }
    if transaction.commit().is_ok() { recorded } else { 0 }
}

fn record_lsp_call_definition_at(
    root: &Path,
    data_dir: &Path,
    source_path: &Path,
    source_line: usize,
    source_column: usize,
    target_path: &Path,
    target_line: usize,
) -> bool {
    if !is_call_callee_position(source_path, source_line, source_column) {
        return false;
    }
    let (Some(source_rel), Some(target_rel)) = (
        project_relative(root, source_path),
        project_relative(root, target_path),
    ) else {
        return false;
    };
    let (Some(source_stamp), Some(target_stamp)) =
        (file_stamp(source_path), file_stamp(target_path))
    else {
        return false;
    };
    let conn = match Connection::open(catalog_file_at(data_dir, root)) {
        Ok(value) => value,
        Err(_) => return false,
    };
    if !indexed_stamp_matches(&conn, &source_rel, source_stamp)
        || !indexed_stamp_matches(&conn, &target_rel, target_stamp)
    {
        return false;
    }
    let caller = logic_symbol_at(&conn, &source_rel, source_line);
    let target = logic_symbol_at(&conn, &target_rel, target_line);
    let (Some((source_name, source_symbol_line)), Some((target_name, target_symbol_line))) =
        (caller, target)
    else {
        return false;
    };
    insert_semantic_call(
        &conn,
        &source_rel,
        &source_name,
        source_symbol_line,
        source_line,
        source_column,
        source_stamp,
        &target_rel,
        &target_name,
        target_symbol_line,
    )
}

pub(crate) fn record_lsp_call_definition(
    root: &Path,
    source_path: &Path,
    source_line: usize,
    source_column: usize,
    target_path: &Path,
    target_line: usize,
) -> bool {
    let Some(data_dir) = DATA_DIR.get() else {
        return false;
    };
    record_lsp_call_definition_at(
        root,
        data_dir,
        source_path,
        source_line,
        source_column,
        target_path,
        target_line,
    )
}

pub(crate) fn record_lsp_call_references(
    root: &Path,
    target_path: &Path,
    target_line: usize,
    references: &[(PathBuf, usize, usize)],
    truncated: bool,
) -> usize {
    let Some(data_dir) = DATA_DIR.get() else {
        return 0;
    };
    record_lsp_call_references_at(
        root,
        data_dir,
        target_path,
        target_line,
        references,
        truncated,
    )
}

pub(crate) fn record_lsp_scan_failure(
    root: &Path,
    target_path: &Path,
    target_line: usize,
) -> u64 {
    let Some(data_dir) = DATA_DIR.get() else {
        return 0;
    };
    record_lsp_scan_failure_at(root, data_dir, target_path, target_line)
}

fn persisted_symbol_count(root: &Path) -> Option<usize> {
    let data_dir = DATA_DIR.get()?;
    let conn = Connection::open(catalog_file_at(data_dir, root)).ok()?;
    conn.query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get::<_, i64>(0))
        .ok()
        .map(|value| value.max(0) as usize)
}

pub fn query_structure(
    root: &Path,
    query: &str,
    role: Option<&str>,
    kind: Option<&str>,
    file: Option<&str>,
    page: usize,
    page_size: usize,
) -> StructureQueryResult {
    query_structure_with_cursor(root, query, role, kind, file, page, page_size, None)
        .expect("without a cursor the structure query retains its in-memory fallback")
}

pub fn query_structure_with_cursor(
    root: &Path,
    query: &str,
    role: Option<&str>,
    kind: Option<&str>,
    file: Option<&str>,
    page: usize,
    page_size: usize,
    cursor: Option<&str>,
) -> Result<StructureQueryResult, String> {
    let syms = index_project_cached(root);
    #[cfg(not(test))]
    ensure_progressive_indexing(root);
    let page_size = page_size.clamp(1, 200);
    let cursor = cursor.map(str::trim).filter(|value| !value.is_empty());
    let page = if cursor.is_some() { 1 } else { page.max(1) };
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    let filter_hash = structure_filter_hash(root, query, role, kind, file);
    let decoded_cursor = cursor
        .map(|value| decode_structure_cursor(value, filter_hash))
        .transpose()?;
    let persisted = if page == 1 || decoded_cursor.is_some() {
        DATA_DIR.get().and_then(|data_dir| {
            query_persisted_symbols_keyset_at(
                root,
                data_dir,
                query,
                role,
                kind,
                file,
                decoded_cursor.as_ref(),
                page_size,
                filter_hash,
            )
        })
    } else {
        query_persisted_symbols(root, query, role, kind, file, page, page_size)
            .map(|result| result.map(|(items, total)| (items, total, None)))
    };
    let persisted = match persisted {
        Some(Ok(value)) => Some(value),
        Some(Err(error)) => return Err(error),
        None if decoded_cursor.is_some() => {
            return Err("持久结构索引不可用，无法安全续读游标；请从第一页重新查询".into())
        }
        None => None,
    };
    let (items, total_matches, next_cursor) = persisted.unwrap_or_else(|| {
        let q = query.trim().to_lowercase();
        let role = role.map(str::trim).filter(|value| !value.is_empty());
        let kind = kind.map(str::trim).filter(|value| !value.is_empty());
        let file_filter = file.map(str::trim).filter(|value| !value.is_empty()).map(str::to_lowercase);
        let mut matched: Vec<&Symbol> = syms
            .iter()
            .filter(|symbol| role.is_none_or(|value| symbol.role == value))
            .filter(|symbol| kind.is_none_or(|value| symbol.kind == value))
            .filter(|symbol| {
                file_filter
                    .as_ref()
                    .is_none_or(|value| symbol.file.to_lowercase().contains(value))
            })
            .filter(|symbol| {
                q.is_empty()
                    || symbol.name.to_lowercase().contains(&q)
                    || symbol.file.to_lowercase().contains(&q)
                    || symbol.signature.to_lowercase().contains(&q)
            })
            .collect();
        matched.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then(a.line.cmp(&b.line))
                .then(a.name.cmp(&b.name))
        });
        let total = matched.len();
        let items = matched.into_iter().skip(offset).take(page_size).cloned().collect();
        (items, total, None)
    });
    let fallback_edges = || {
        let mut edges = structure_edges(&syms)
            .into_iter()
            .filter(|edge| {
                items.iter().any(|symbol| {
                    (symbol.file == edge.source_file
                        && symbol.name == edge.source_name
                        && symbol.line == edge.source_line)
                        || (symbol.file == edge.target_file
                            && symbol.name == edge.target_name
                            && symbol.line == edge.target_line)
                })
            })
            .collect::<Vec<_>>();
        let total = structure_edges(&syms).len();
        edges.sort();
        edges.dedup();
        (edges, total)
    };
    let (relations, indexed_relations) = query_persisted_edges(root, &items)
        .and_then(Result::ok)
        .unwrap_or_else(fallback_edges);
    let next_page = decoded_cursor
        .is_none()
        .then(|| (offset.saturating_add(page_size) < total_matches).then_some(page + 1))
        .flatten();

    let key = canonical_key(root);
    let now = now_secs();
    let (indexed_files, catalog, synced_ago_secs) = cache()
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .get(&key)
                .map(|entry| {
                    (
                        entry.catalog.indexed_source_files,
                        entry.catalog,
                        now.saturating_sub(entry.last_sync),
                    )
                })
        })
        .unwrap_or((0, CatalogStats::default(), 0));
    let coverage = catalog.coverage();
    let progressive = progressive_status(root, catalog.deferred_source_files);
    let semantic = persisted_semantic_coverage(root).unwrap_or_else(|| {
        SemanticCoverageStats::from_counts(
            syms.iter().filter(|symbol| symbol.role == "logic").count(),
            0,
            0,
            0,
            0,
        )
    });
    let scip = DATA_DIR.get()
        .map(|data_dir| crate::services::scip_index::status(root, &catalog_file_at(data_dir, root)))
        .unwrap_or_default();
    Ok(StructureQueryResult {
        items,
        relations,
        total_matches,
        page,
        page_size,
        next_page,
        next_cursor,
        indexed_files,
        indexed_symbols: persisted_symbol_count(root).unwrap_or(syms.len()),
        indexed_relations,
        semantic,
        scip,
        catalog,
        watcher_active: crate::services::repo_watcher::is_watching(root),
        progressive,
        coverage,
        synced_ago_secs,
    })
}

/// 项目级摘要：组件数、页面数、函数数、路由清单（用于 Agent 快速了解工程结构）
#[derive(Debug, Serialize, Default)]
pub struct ProjectOutline {
    pub components: Vec<Symbol>,
    pub pages: Vec<String>,
    pub symbols_count: usize,
}

pub fn build_outline(root: &Path) -> ProjectOutline {
    // 走缓存索引：对话每轮构建概要时命中增量缓存，避免全量重扫
    let syms = index_project_cached(root);
    let components: Vec<Symbol> = syms
        .iter()
        .filter(|s| s.kind == "component" || (s.kind == "decorator" && s.name == "@Entry"))
        .cloned()
        .collect();
    let pages = harmony::collect_routes(root, None);
    ProjectOutline {
        components,
        pages,
        symbols_count: syms.len(),
    }
}

/// 索引元信息：符号/文件数量与数据来源（供面板展示缓存状态）
#[derive(Debug, Serialize)]
pub struct SymbolIndexMeta {
    pub symbols: usize,
    pub files: usize,
    /// 数据来源：disk（磁盘恢复）/ scan（本次会话扫描建立）
    pub source: &'static str,
    /// 最近同步距今秒数（磁盘恢复后未同步时为较大值）
    pub synced_ago_secs: u64,
    pub catalog: CatalogStats,
    pub coverage: String,
    pub watcher_active: bool,
}

/// 查询索引元信息：内部先确保索引已构建且新鲜（有冷却/增量，不会重复全量扫描）
pub fn index_meta(root: &Path) -> SymbolIndexMeta {
    let syms = index_project_cached(root);
    let key = canonical_key(root);
    let now = now_secs();
    let guard = cache().lock().unwrap();
    match guard.get(&key) {
        Some(e) => SymbolIndexMeta {
            symbols: e.syms.len(),
            files: e.files.len(),
            source: e.source,
            synced_ago_secs: now.saturating_sub(e.last_sync),
            catalog: e.catalog,
            coverage: e.catalog.coverage(),
            watcher_active: crate::services::repo_watcher::is_watching(root),
        },
        // 条目被容量上限清空：仅能给出符号数（来源视为本次扫描）
        None => SymbolIndexMeta {
            symbols: syms.len(),
            files: 0,
            source: "scan",
            synced_ago_secs: 0,
            catalog: CatalogStats::default(),
            coverage: "unavailable".into(),
            watcher_active: false,
        },
    }
}

/// 文件级符号数量（供文件树面板徽标展示）
#[derive(Debug, Serialize)]
pub struct SymbolCount {
    pub file: String,
    pub count: usize,
}

pub fn symbol_counts(root: &Path) -> Vec<SymbolCount> {
    let syms = index_project_cached(root);
    let mut map: HashMap<String, usize> = HashMap::new();
    for s in &syms {
        *map.entry(s.file.clone()).or_default() += 1;
    }
    let mut out: Vec<SymbolCount> = map
        .into_iter()
        .map(|(file, count)| SymbolCount { file, count })
        .collect();
    out.sort_by(|a, b| a.file.cmp(&b.file));
    out
}

/// 即时扫描单文件符号（供 Agent 工具/前端单文件大纲使用）
#[allow(dead_code)]
pub fn symbols_of_file(root: &Path, rel: &str) -> Result<Vec<Symbol>, String> {
    let target = ensure_inside(root, rel)?;
    if !target.is_file() {
        return Err("目标不是文件".into());
    }
    let mut out = Vec::new();
    let r = safe_rel(root, &target);
    scan_file(&target, &r, &mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_arkts_component_and_methods() {
        let src = r#"
import { router } from '@kit.ArkUI';

@Entry
@Component
struct Index {
  @State count: number = 0;

  aboutToAppear() {
  }

  build() {
  }
}
"#;
        let dir = std::env::temp_dir().join("deveco-symbol-test");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("Index.ets");
        std::fs::write(&f, src).unwrap();
        let mut out = Vec::new();
        scan_file(&f, "Index.ets", &mut out);
        assert!(out.iter().any(|s| s.kind == "decorator" && s.name == "@Entry"));
        assert!(out.iter().any(|s| s.kind == "component" && s.name == "Index"), "应识别 struct Index: {out:?}");
        assert!(out.iter().any(|s| s.kind == "method" && s.name == "aboutToAppear"));
        assert!(out.iter().any(|s| s.kind == "method" && s.name == "build"));
        let component = out.iter().find(|s| s.kind == "component" && s.name == "Index").unwrap();
        assert_eq!(component.role, "entity");
        assert!(component.end_line > component.line);
        let method = out.iter().find(|s| s.kind == "method" && s.name == "aboutToAppear").unwrap();
        assert_eq!(method.role, "logic");
        assert!(method.signature.contains("aboutToAppear"));
        assert!(method.end_line >= method.line);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extracts_arkts_state_decorators() {
        let src = r#"
@Entry
@Component
struct Detail {
  @State count: number = 0;
  @Prop title: string = '';
  @Link linked: boolean;
  @Watch('onChange')
  @State watched: number = 1;
  @StateXxx helper: string = '';
  build() {}
}
"#;
        let dir = std::env::temp_dir().join("deveco-symbol-decor");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("Detail.ets");
        std::fs::write(&f, src).unwrap();
        let mut out = Vec::new();
        scan_file(&f, "Detail.ets", &mut out);
        assert!(out.iter().any(|s| s.kind == "decorator" && s.name == "@State"));
        assert!(out.iter().any(|s| s.kind == "decorator" && s.name == "@Prop"));
        assert!(out.iter().any(|s| s.kind == "decorator" && s.name == "@Link"));
        assert!(out.iter().any(|s| s.kind == "decorator" && s.name == "@Watch"));
        assert!(!out.iter().any(|s| s.name == "@StateXxx"), "普通标识符不应误报为装饰器: {out:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extracts_rust_items() {
        let src = "pub struct Foo;\nfn bar() {}\npub async fn baz() {}";
        let dir = std::env::temp_dir().join("deveco-symbol-rs");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.rs");
        std::fs::write(&f, src).unwrap();
        let mut out = Vec::new();
        scan_file(&f, "a.rs", &mut out);
        assert!(out.iter().any(|s| s.kind == "struct" && s.name == "Foo"));
        assert!(out.iter().any(|s| s.kind == "function" && s.name == "bar"));
        assert!(out.iter().any(|s| s.kind == "function" && s.name == "baz"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tree_sitter_extracts_typescript_ranges_parents_and_arrow_functions() {
        let src = r#"export interface Loader {
  load(value: string): Promise<string>;
}

export class Service {
  async fetch(value: string): Promise<string> {
    const braces = "{not a block}";
    return value + braces;
  }
}

export const normalize = (value: string) => {
  return value.trim();
};
"#;
        let dir = std::env::temp_dir().join(format!(
            "deveco-symbol-ts-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("service.ts");
        std::fs::write(&file, src).unwrap();
        let mut out = Vec::new();
        scan_file(&file, "service.ts", &mut out);

        let interface = out.iter().find(|symbol| symbol.name == "Loader").unwrap();
        assert_eq!(interface.kind, "interface");
        assert_eq!(interface.end_line, 3);
        assert_eq!(interface.source_layer, "tree_sitter");
        assert_eq!(interface.language, "ts");
        let signature = out.iter().find(|symbol| symbol.name == "load").unwrap();
        assert_eq!(signature.kind, "method");
        assert_eq!(signature.parent.as_deref(), Some("Loader"));
        let method = out.iter().find(|symbol| symbol.name == "fetch").unwrap();
        assert_eq!(method.parent.as_deref(), Some("Service"));
        assert_eq!(method.end_line, 9, "字符串中的大括号不应破坏精确范围");
        let arrow = out.iter().find(|symbol| symbol.name == "normalize").unwrap();
        assert_eq!(arrow.kind, "function");
        assert_eq!(arrow.line, 12);
        assert_eq!(arrow.end_line, 14);
        assert!(out.iter().all(|symbol| symbol.source_layer == "tree_sitter"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn malformed_typescript_falls_back_to_lightweight_scanner() {
        let dir = std::env::temp_dir().join(format!(
            "deveco-symbol-ts-fallback-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("broken.ts");
        std::fs::write(&file, "function recover() {\n  return 1;\n").unwrap();
        let mut out = Vec::new();
        scan_file(&file, "broken.ts", &mut out);
        let recovered = out.iter().find(|symbol| symbol.name == "recover").unwrap();
        assert_eq!(recovered.source_layer, "lightweight");
        assert_eq!(recovered.language, "ts");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tree_sitter_supports_javascript_tsx_and_jsx_entrypoints() {
        let cases = [
            ("js", "class JsService { run() { return 1; } }"),
            ("jsx", "const JsCard = () => <section>ok</section>;"),
            ("tsx", "const TsCard = (): JSX.Element => <section>ok</section>;"),
        ];
        for (ext, source) in cases {
            let mut out = Vec::new();
            assert!(scan_file_tree_sitter(
                source,
                &format!("entry.{ext}"),
                ext,
                &mut out,
            ));
            assert!(!out.is_empty(), "{ext} 应产生至少一个结构节点");
            assert!(out.iter().all(|symbol| {
                symbol.language == ext && symbol.source_layer == "tree_sitter"
            }));
        }
    }

    #[test]
    fn tree_sitter_extracts_arkts_components_methods_and_state_decorators() {
        let source = r#"@Entry
@Component
struct CounterCard {
  @State count: number = 0;

  build() {
    Column() {
      Text(`${this.count}`)
    }
  }

  increment(): void {
    this.count++;
  }
}
"#;
        let mut out = Vec::new();
        assert!(scan_file_tree_sitter(
            source,
            "entry/src/main/ets/pages/CounterCard.ets",
            "ets",
            &mut out,
        ));

        let component = out.iter().find(|symbol| symbol.name == "CounterCard").unwrap();
        assert_eq!(component.kind, "component");
        assert_eq!(component.line, 3);
        assert_eq!(component.end_line, 15);
        assert_eq!(component.source_layer, "tree_sitter");
        let build = out.iter().find(|symbol| symbol.name == "build").unwrap();
        assert_eq!(build.kind, "method");
        assert_eq!(build.parent.as_deref(), Some("CounterCard"), "{out:?}");
        assert_eq!(build.end_line, 10);
        let state = out.iter().find(|symbol| symbol.name == "@State").unwrap();
        assert_eq!(state.parent.as_deref(), Some("CounterCard"));
        assert!(out.iter().any(|symbol| symbol.name == "@Entry"));
        assert!(out.iter().any(|symbol| symbol.name == "@Component"));
        assert!(out.iter().all(|symbol| {
            symbol.language == "ets" && symbol.source_layer == "tree_sitter"
        }));
    }

    #[test]
    fn tree_sitter_records_declared_type_relations_without_guessing_targets() {
        let source = r#"interface Identified {}
interface Loadable extends Identified {}
class BaseService {}
class Service extends BaseService implements Loadable, Disposable {}
"#;
        let mut symbols = Vec::new();
        assert!(scan_file_tree_sitter(source, "service.ts", "ts", &mut symbols));

        let loadable = symbols.iter().find(|symbol| symbol.name == "Loadable").unwrap();
        assert_eq!(
            loadable.declared_relations,
            vec![DeclaredRelation { kind: "extends".into(), target_name: "Identified".into(), module_specifier: None, imported_name: None }]
        );
        let service = symbols.iter().find(|symbol| symbol.name == "Service").unwrap();
        assert_eq!(
            service.declared_relations,
            vec![
                DeclaredRelation { kind: "extends".into(), target_name: "BaseService".into(), module_specifier: None, imported_name: None },
                DeclaredRelation { kind: "implements".into(), target_name: "Disposable".into(), module_specifier: None, imported_name: None },
                DeclaredRelation { kind: "implements".into(), target_name: "Loadable".into(), module_specifier: None, imported_name: None },
            ]
        );
        let edges = structure_edges(&symbols);
        let declared = edges
            .iter()
            .filter(|edge| edge.source_name == "Service" && edge.kind != "contains")
            .collect::<Vec<_>>();
        assert_eq!(declared.len(), 3);
        assert_eq!(
            declared
                .iter()
                .filter(|edge| !edge.target_file.is_empty() && edge.target_line > 0)
                .count(),
            2,
            "同文件唯一声明应解析，缺失的 Disposable 必须保持未解析"
        );
        let disposable = declared
            .iter()
            .find(|edge| edge.target_name == "Disposable")
            .unwrap();
        assert!(disposable.target_file.is_empty());
        assert_eq!(disposable.target_line, 0);

        let mut cross_file = Vec::new();
        assert!(scan_file_tree_sitter("class RemoteBase {}", "base.ts", "ts", &mut cross_file));
        assert!(scan_file_tree_sitter(
            "class RemoteChild extends RemoteBase {}",
            "child.ts",
            "ts",
            &mut cross_file,
        ));
        let remote = structure_edges(&cross_file)
            .into_iter()
            .find(|edge| edge.source_name == "RemoteChild")
            .unwrap();
        assert!(remote.target_file.is_empty(), "没有 import 证据时不得跨文件猜测目标");
        assert_eq!(remote.target_line, 0);
    }

    #[test]
    fn tree_sitter_attaches_relative_named_import_evidence_to_type_relations() {
        let cases = [
            (
                "ts",
                "import { BaseService as Parent, Loadable } from './base';\nclass Service extends Parent implements Loadable {}\n",
            ),
            (
                "ets",
                "import lazy { BaseService as Parent } from './base';\nclass Service extends Parent {}\n",
            ),
        ];
        for (ext, source) in cases {
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(&tree_sitter_language(ext).unwrap()).unwrap();
            let tree = parser.parse(source, None).unwrap();
            let imports = named_imports(tree.root_node(), source.as_bytes());
            assert!(imports.contains_key("Parent"), "{ext}: {} / {imports:?}", tree.root_node().to_sexp());
            let mut symbols = Vec::new();
            assert!(scan_file_tree_sitter(source, &format!("service.{ext}"), ext, &mut symbols));
            let service = symbols.iter().find(|symbol| symbol.name == "Service").unwrap();
            let parent = service
                .declared_relations
                .iter()
                .find(|relation| relation.target_name == "Parent")
                .unwrap();
            assert_eq!(parent.module_specifier.as_deref(), Some("./base"));
            assert_eq!(parent.imported_name.as_deref(), Some("BaseService"));
            if ext == "ts" {
                let loadable = service
                    .declared_relations
                    .iter()
                    .find(|relation| relation.target_name == "Loadable")
                    .unwrap();
                assert_eq!(loadable.module_specifier.as_deref(), Some("./base"));
                assert_eq!(loadable.imported_name.as_deref(), Some("Loadable"));
            }
        }
    }

    #[test]
    fn malformed_arkts_falls_back_to_lightweight_scanner() {
        let dir = std::env::temp_dir().join(format!(
            "deveco-symbol-ets-fallback-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Broken.ets");
        std::fs::write(&file, "@Component\nstruct Broken {\n  build() {\n").unwrap();
        let mut out = Vec::new();
        scan_file(&file, "Broken.ets", &mut out);
        let recovered = out.iter().find(|symbol| symbol.name == "Broken").unwrap();
        assert_eq!(recovered.source_layer, "lightweight");
        assert_eq!(recovered.language, "ets");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn filter_works() {
        let syms = vec![
            Symbol { kind: "function".into(), name: "loadData".into(), file: "a.ts".into(), line: 1, end_line: 1, role: "logic".into(), signature: "function loadData()".into(), parent: None, language: "ts".into(), source_layer: "tree_sitter".into(), declared_relations: Vec::new() },
            Symbol { kind: "component".into(), name: "BookCard".into(), file: "b.ets".into(), line: 2, end_line: 5, role: "entity".into(), signature: "struct BookCard".into(), parent: None, language: "ets".into(), source_layer: "lightweight".into(), declared_relations: Vec::new() },
        ];
        assert_eq!(filter_symbols(&syms, "book", None).len(), 1);
        assert_eq!(filter_symbols(&syms, "", Some("component")).len(), 1);
        assert_eq!(filter_symbols(&syms, "", None).len(), 2);
    }

    #[test]
    fn catalog_coverage_distinguishes_deferred_and_best_effort() {
        let complete = CatalogStats {
            discovered_files: 3,
            source_files: 2,
            indexed_source_files: 2,
            unsupported_files: 1,
            ..CatalogStats::default()
        };
        assert_eq!(complete.coverage(), "best_effort_lightweight_syntax_index");
        let deferred = CatalogStats {
            deferred_source_files: 17,
            ..complete
        };
        assert_eq!(
            deferred.coverage(),
            "partial_17_source_files_deferred_by_parse_budget"
        );
        assert_eq!(glob_to_sql_like("src/**/*.ets"), "src/%%/%.ets");
        assert_eq!(glob_to_sql_like("100%_ok?.ts"), "100\\%\\_ok_.ts");
    }

    #[test]
    fn catalog_persists_all_files_and_removes_stale_rows() {
        let root = std::env::temp_dir().join(format!(
            "deveco-catalog-project-{}",
            uuid::Uuid::new_v4()
        ));
        let data_dir = std::env::temp_dir().join(format!(
            "deveco-catalog-data-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(root.join("small.rs"), "fn small() {}\n").unwrap();
        std::fs::write(root.join("README.md"), "# hello\n").unwrap();
        std::fs::write(root.join("large.ts"), vec![b'x'; MAX_BYTES as usize + 1]).unwrap();

        let (files, stats) = collect_files_at(&root, Some(&data_dir));
        assert_eq!(files.len(), 1);
        assert_eq!(stats.discovered_files, 3);
        assert_eq!(stats.source_files, 2);
        assert_eq!(stats.oversized_source_files, 1);
        assert_eq!(stats.unsupported_files, 1);
        assert!(stats.persisted);

        let conn = Connection::open(catalog_file_at(&data_dir, &root)).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 3);
        let state: String = conn
            .query_row(
                "SELECT state FROM files WHERE path='large.ts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "oversized");
        drop(conn);

        std::fs::remove_file(root.join("README.md")).unwrap();
        let (_, refreshed) = collect_files_at(&root, Some(&data_dir));
        assert!(refreshed.persisted);
        let conn = Connection::open(catalog_file_at(&data_dir, &root)).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 2);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn existing_symbol_database_adds_parser_and_relation_columns() {
        let root = std::env::temp_dir().join(format!(
            "deveco-symbol-migrate-{}",
            uuid::Uuid::new_v4()
        ));
        let data_dir = std::env::temp_dir().join(format!(
            "deveco-symbol-migrate-data-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        let database = catalog_file_at(&data_dir, &root);
        std::fs::create_dir_all(database.parent().unwrap()).unwrap();
        let conn = Connection::open(&database).unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
               id INTEGER PRIMARY KEY AUTOINCREMENT, file TEXT NOT NULL, kind TEXT NOT NULL,
               name TEXT NOT NULL, line INTEGER NOT NULL, end_line INTEGER NOT NULL,
               role TEXT NOT NULL, signature TEXT NOT NULL, parent TEXT, shard TEXT NOT NULL
             );
             CREATE TABLE structure_stats (
               id INTEGER PRIMARY KEY CHECK(id = 1),
               relation_count INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO structure_stats(id, relation_count) VALUES(1, 0);",
        )
        .unwrap();
        drop(conn);

        let _ = collect_files_at(&root, Some(&data_dir));
        let conn = Connection::open(database).unwrap();
        let columns = conn
            .prepare("PRAGMA table_info(symbols)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "language"));
        assert!(columns.iter().any(|column| column == "source_layer"));
        assert!(columns.iter().any(|column| column == "declared_relations"));
        let stats_columns = conn
            .prepare("PRAGMA table_info(structure_stats)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(stats_columns
            .iter()
            .any(|column| column == "semantic_relation_count"));
        for column in [
            "logic_symbol_count",
            "semantic_target_count",
            "semantic_truncated_target_count",
            "semantic_failure_target_count",
        ] {
            assert!(stats_columns.iter().any(|value| value == column), "{column}");
        }
        let edge_columns = conn
            .prepare("PRAGMA table_info(symbol_edges)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(edge_columns.iter().any(|column| column == "target_module"));
        assert!(edge_columns
            .iter()
            .any(|column| column == "target_imported_name"));
        let reexport_table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='module_reexports'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reexport_table, 1);
        let semantic_table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='semantic_call_edges'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(semantic_table, 1);
        let semantic_scan_table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='semantic_target_scans'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(semantic_scan_table, 1);
        let semantic_failure_table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='semantic_scan_failures'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(semantic_failure_table, 1);
        let parser_version: i64 = conn
            .query_row(
                "SELECT parser_version FROM structure_meta WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(parser_version, STRUCTURE_PARSER_VERSION);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn catalog_applies_file_deltas_without_full_walk() {
        let root =
            std::env::temp_dir().join(format!("deveco-catalog-delta-{}", uuid::Uuid::new_v4()));
        let data_dir =
            std::env::temp_dir().join(format!("deveco-catalog-data-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(root.join("small.rs"), "fn before() {}\n").unwrap();
        std::fs::write(root.join("README.md"), "# hello\n").unwrap();
        let (_, initial) = collect_files_at(&root, Some(&data_dir));
        assert!(initial.persisted);

        std::fs::write(root.join("small.rs"), "fn after_change() {}\n").unwrap();
        std::fs::write(root.join("added.py"), "def added():\n    pass\n").unwrap();
        std::fs::remove_file(root.join("README.md")).unwrap();
        let delta = apply_catalog_changes_at(
            &root,
            &data_dir,
            &["small.rs".into(), "added.py".into(), "README.md".into()],
        );
        let CatalogDelta::Updated(stats) = delta else {
            panic!("普通文件变化应能精确更新目录")
        };
        assert_eq!(stats.discovered_files, 2);
        assert_eq!(stats.source_files, 2);
        assert_eq!(stats.indexed_source_files, 2);

        let conn = Connection::open(catalog_file_at(&data_dir, &root)).unwrap();
        let paths = conn
            .prepare("SELECT path FROM files ORDER BY path")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(paths, vec!["added.py", "small.rs"]);
        drop(conn);

        std::fs::create_dir_all(root.join("new_dir")).unwrap();
        assert!(matches!(
            apply_catalog_changes_at(&root, &data_dir, &["new_dir".into()]),
            CatalogDelta::NeedsReconciliation
        ));
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn persisted_structure_nodes_paginate_and_replace_one_file() {
        let root =
            std::env::temp_dir().join(format!("deveco-symbol-db-{}", uuid::Uuid::new_v4()));
        let data_dir =
            std::env::temp_dir().join(format!("deveco-symbol-data-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(root.join("a.rs"), "struct Alpha {}\nfn old_logic() {}\n").unwrap();
        std::fs::write(root.join("b.rs"), "struct Beta {}\n").unwrap();
        let (_, catalog) = collect_files_at(&root, Some(&data_dir));
        let mut symbols = Vec::new();
        scan_file(&root.join("a.rs"), "a.rs", &mut symbols);
        scan_file(&root.join("b.rs"), "b.rs", &mut symbols);
        assert!(replace_all_symbol_rows_at(
            &root,
            &data_dir,
            &symbols,
            catalog.revision,
        ));

        let (first, total) = query_persisted_symbols_at(
            &root, &data_dir, "", None, None, None, 1, 1,
        )
        .unwrap()
        .unwrap();
        assert_eq!(total, 3);
        assert_eq!(first.len(), 1);
        let (logic, logic_total) = query_persisted_symbols_at(
            &root, &data_dir, "logic", Some("logic"), None, Some("a.rs"), 1, 20,
        )
        .unwrap()
        .unwrap();
        assert_eq!(logic_total, 1);
        assert_eq!(logic[0].name, "old_logic");

        std::fs::write(root.join("a.rs"), "struct Alpha {}\nfn new_logic() {}\n").unwrap();
        let mut fresh = Vec::new();
        scan_file(&root.join("a.rs"), "a.rs", &mut fresh);
        assert!(replace_changed_symbol_rows_at(
            &root,
            &data_dir,
            &["a.rs".into()],
            &fresh,
        ));
        let (updated, _) = query_persisted_symbols_at(
            &root, &data_dir, "logic", Some("logic"), None, None, 1, 20,
        )
        .unwrap()
        .unwrap();
        assert_eq!(updated[0].name, "new_logic");
        assert!(!updated.iter().any(|symbol| symbol.name == "old_logic"));

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn sqlite_structure_query_plans_use_targeted_indexes() {
        let root = std::env::temp_dir().join(format!(
            "deveco-symbol-plan-{}",
            uuid::Uuid::new_v4()
        ));
        let data_dir = std::env::temp_dir().join(format!(
            "deveco-symbol-plan-data-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        let _ = collect_files_at(&root, Some(&data_dir));
        let conn = Connection::open(catalog_file_at(&data_dir, &root)).unwrap();

        let exact_plan = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT kind, name, file, line, end_line, role, signature, parent
                 FROM symbols
                 WHERE (?1 = '' OR role = ?1)
                   AND (?2 = '' OR kind = ?2)
                   AND (?3 = '' OR instr(lower(file), ?3) > 0)
                   AND name = ?4 COLLATE NOCASE
                 ORDER BY file, line, name LIMIT ?5 OFFSET ?6",
            )
            .unwrap()
            .query_map(params!["", "", "", "Alpha", 20, 0], |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(
            exact_plan.contains("idx_symbols_name"),
            "精确名称查询应命中名称索引：{exact_plan}"
        );

        let kind_plan = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT kind, name, file, line, end_line, role, signature, parent
                 FROM symbols WHERE kind=?1
                 ORDER BY file, line, name LIMIT ?2 OFFSET ?3",
            )
            .unwrap()
            .query_map(params!["component", 20, 0], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(
            kind_plan.contains("idx_symbols_kind_order"),
            "按类型浏览应命中覆盖排序的复合索引：{kind_plan}"
        );

        let cursor_plan = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT kind, name, file, line, end_line, role, signature, parent, id
                 FROM symbols
                 WHERE kind=?1 AND (file, line, name, id) > (?2, ?3, ?4, ?5)
                 ORDER BY file, line, name, id LIMIT ?6",
            )
            .unwrap()
            .query_map(
                params!["component", "src/a.ets", 1, "Page", 1, 20],
                |row| row.get::<_, String>(3),
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(
            cursor_plan.contains("idx_symbols_kind_order"),
            "游标续页应从复合索引定位而不是扫描前序页：{cursor_plan}"
        );

        let deferred_plan = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT path, size, mtime_ns FROM files
                 WHERE state='deferred' ORDER BY shard, path LIMIT 128",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(
            deferred_plan.contains("idx_files_state_order"),
            "渐进批次领取应使用覆盖顺序索引：{deferred_plan}"
        );

        let semantic_plan = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT s.file, s.name, s.line
                 FROM symbols s
                 JOIN files f ON f.path=s.file AND f.state='indexed'
                 LEFT JOIN semantic_target_scans scan
                   ON scan.target_file=s.file AND scan.target_name=s.name
                  AND scan.target_line=s.line AND scan.provider='arkts_lsp'
                 LEFT JOIN semantic_scan_failures failure
                   ON failure.target_file=s.file AND failure.target_name=s.name
                  AND failure.target_line=s.line AND failure.provider='arkts_lsp'
                 WHERE s.role='logic' AND s.language=?2 AND s.kind=?1
                   AND scan.target_file IS NULL
                   AND (failure.target_file IS NULL OR failure.retry_after <= ?4)
                 ORDER BY s.file, s.line, s.name LIMIT ?3",
            )
            .unwrap()
            .query_map(params!["method", "ets", 16, now_secs() as i64], |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(
            semantic_plan.contains("idx_symbols_semantic_schedule"),
            "语义调度应从复合索引领取未覆盖目标：{semantic_plan}"
        );

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn keyset_cursor_paginates_without_duplicates_and_binds_filters() {
        let root = std::env::temp_dir().join(format!(
            "deveco-symbol-cursor-{}",
            uuid::Uuid::new_v4()
        ));
        let data_dir = std::env::temp_dir().join(format!(
            "deveco-symbol-cursor-data-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(root.join("a.rs"), "struct Alpha {}\nfn alpha() {}\n").unwrap();
        std::fs::write(root.join("b.rs"), "struct Beta {}\n").unwrap();
        let (_, catalog) = collect_files_at(&root, Some(&data_dir));
        let mut symbols = Vec::new();
        scan_file(&root.join("a.rs"), "a.rs", &mut symbols);
        scan_file(&root.join("b.rs"), "b.rs", &mut symbols);
        assert!(replace_all_symbol_rows_at(
            &root,
            &data_dir,
            &symbols,
            catalog.revision,
        ));

        let filter_hash = structure_filter_hash(&root, "", None, None, None);
        let (first, total, first_cursor) = query_persisted_symbols_keyset_at(
            &root, &data_dir, "", None, None, None, None, 1, filter_hash,
        )
        .unwrap()
        .unwrap();
        assert_eq!(total, 3);
        let first_cursor = first_cursor.expect("第一页之后应返回游标");
        let decoded = decode_structure_cursor(&first_cursor, filter_hash).unwrap();
        let (second, _, second_cursor) = query_persisted_symbols_keyset_at(
            &root,
            &data_dir,
            "",
            None,
            None,
            None,
            Some(&decoded),
            1,
            filter_hash,
        )
        .unwrap()
        .unwrap();
        assert_ne!(first[0].name, second[0].name);
        assert!(second_cursor.is_some());
        assert!(decode_structure_cursor(
            &first_cursor,
            structure_filter_hash(&root, "different", None, None, None),
        )
        .is_err());
        assert!(decode_structure_cursor("not-a-cursor", filter_hash).is_err());
        let conn = Connection::open(catalog_file_at(&data_dir, &root)).unwrap();
        conn.execute(
            "UPDATE structure_meta SET revision = revision + 1 WHERE id=1",
            [],
        )
        .unwrap();
        drop(conn);
        assert!(query_persisted_symbols_keyset_at(
            &root,
            &data_dir,
            "",
            None,
            None,
            None,
            Some(&decoded),
            1,
            filter_hash,
        )
        .unwrap()
        .is_err());

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn containment_edges_persist_query_and_incrementally_replace() {
        let root =
            std::env::temp_dir().join(format!("deveco-edge-db-{}", uuid::Uuid::new_v4()));
        let data_dir =
            std::env::temp_dir().join(format!("deveco-edge-data-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        let file = root.join("Page.ets");
        std::fs::write(
            &file,
            "@Component\nstruct Page {\n  load() {\n  }\n}\n",
        )
        .unwrap();
        let (_, catalog) = collect_files_at(&root, Some(&data_dir));
        let mut symbols = Vec::new();
        scan_file(&file, "Page.ets", &mut symbols);
        assert!(replace_all_symbol_rows_at(
            &root,
            &data_dir,
            &symbols,
            catalog.revision,
        ));
        let (edges, total) = query_persisted_edges_at(&root, &data_dir, &symbols)
            .unwrap()
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].kind, "contains");
        assert_eq!(edges[0].source_name, "Page");
        assert_eq!(edges[0].target_name, "load");

        std::fs::write(
            &file,
            "@Component\nstruct Page {\n  refresh() {\n  }\n}\n",
        )
        .unwrap();
        let mut fresh = Vec::new();
        scan_file(&file, "Page.ets", &mut fresh);
        assert!(replace_changed_symbol_rows_at(
            &root,
            &data_dir,
            &["Page.ets".into()],
            &fresh,
        ));
        let (updated, updated_total) = query_persisted_edges_at(&root, &data_dir, &fresh)
            .unwrap()
            .unwrap();
        assert_eq!(updated_total, 1);
        assert_eq!(updated[0].target_name, "refresh");
        assert!(!updated.iter().any(|edge| edge.target_name == "load"));

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn declared_type_relations_roundtrip_through_sqlite() {
        let root =
            std::env::temp_dir().join(format!("deveco-type-edge-db-{}", uuid::Uuid::new_v4()));
        let data_dir =
            std::env::temp_dir().join(format!("deveco-type-edge-data-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        let file = root.join("service.ts");
        std::fs::write(
            &file,
            "interface Loadable {}\nclass BaseService {}\nclass Service extends BaseService implements Loadable {}\n",
        )
        .unwrap();
        let (_, catalog) = collect_files_at(&root, Some(&data_dir));
        let mut symbols = Vec::new();
        scan_file(&file, "service.ts", &mut symbols);
        assert!(replace_all_symbol_rows_at(
            &root,
            &data_dir,
            &symbols,
            catalog.revision,
        ));

        let (persisted, total) = query_persisted_symbols_at(
            &root,
            &data_dir,
            "Service",
            None,
            Some("class"),
            None,
            1,
            20,
        )
        .unwrap()
        .unwrap();
        assert_eq!(total, 1);
        assert_eq!(persisted[0].declared_relations.len(), 2);
        let (edges, edge_total) = query_persisted_edges_at(&root, &data_dir, &persisted)
            .unwrap()
            .unwrap();
        assert_eq!(edge_total, 2);
        assert_eq!(edges.len(), 2);
        assert!(edges.iter().all(|edge| {
            matches!(edge.kind.as_str(), "extends" | "implements")
                && edge.target_file == "service.ts"
                && edge.target_line > 0
        }));

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn relative_import_relations_resolve_fresh_cross_file_targets() {
        let root =
            std::env::temp_dir().join(format!("deveco-import-edge-db-{}", uuid::Uuid::new_v4()));
        let data_dir = std::env::temp_dir().join(format!(
            "deveco-import-edge-data-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join("model")).unwrap();
        std::fs::create_dir_all(root.join("service")).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        let base_file = root.join("model/base.ts");
        let service_file = root.join("service/service.ts");
        std::fs::write(&base_file, "export class BaseService {}\n").unwrap();
        std::fs::write(
            &service_file,
            "import { BaseService as Parent } from '../model/base';\nexport class Service extends Parent {}\n",
        )
        .unwrap();
        let (_, catalog) = collect_files_at(&root, Some(&data_dir));
        let mut symbols = Vec::new();
        scan_file(&base_file, "model/base.ts", &mut symbols);
        scan_file(&service_file, "service/service.ts", &mut symbols);
        assert!(replace_all_symbol_rows_at(
            &root,
            &data_dir,
            &symbols,
            catalog.revision,
        ));
        let service = symbols
            .iter()
            .find(|symbol| symbol.name == "Service")
            .cloned()
            .unwrap();
        let (resolved, _) = query_persisted_edges_at(&root, &data_dir, &[service.clone()])
            .unwrap()
            .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].target_file, "model/base.ts");
        assert_eq!(resolved[0].target_name, "BaseService");
        assert_eq!(resolved[0].target_line, 1);
        let base = symbols
            .iter()
            .find(|symbol| symbol.name == "BaseService")
            .cloned()
            .unwrap();
        let (incoming, _) = query_persisted_edges_at(&root, &data_dir, &[base])
            .unwrap()
            .unwrap();
        assert_eq!(incoming.len(), 1, "目标节点应能反查跨文件入边");
        assert_eq!(incoming[0].source_name, "Service");

        std::fs::write(&base_file, "export class RenamedBaseService {}\n").unwrap();
        let mut changed = Vec::new();
        scan_file(&base_file, "model/base.ts", &mut changed);
        assert!(replace_changed_symbol_rows_at(
            &root,
            &data_dir,
            &["model/base.ts".into()],
            &changed,
        ));
        let (stale_safe, _) = query_persisted_edges_at(&root, &data_dir, &[service])
            .unwrap()
            .unwrap();
        assert_eq!(stale_safe.len(), 1);
        assert!(stale_safe[0].target_file.is_empty());
        assert_eq!(stale_safe[0].target_line, 0);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn tsconfig_path_alias_resolves_one_existing_target() {
        let root =
            std::env::temp_dir().join(format!("deveco-alias-edge-db-{}", uuid::Uuid::new_v4()));
        let data_dir = std::env::temp_dir().join(format!(
            "deveco-alias-edge-data-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join("src/model")).unwrap();
        std::fs::create_dir_all(root.join("src/service")).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(
            root.join("tsconfig.json"),
            r#"{
              // JSON5 comments and trailing commas are accepted.
              "compilerOptions": {
                "baseUrl": ".",
                "paths": { "@model/*": ["src/model/*"], },
              },
            }"#,
        )
        .unwrap();
        let base_file = root.join("src/model/base.ts");
        let service_file = root.join("src/service/service.ts");
        std::fs::write(&base_file, "export interface BaseModel {}\n").unwrap();
        std::fs::write(
            &service_file,
            "import { BaseModel as Parent } from '@model/base';\nexport interface ServiceModel extends Parent {}\n",
        )
        .unwrap();
        let (_, catalog) = collect_files_at(&root, Some(&data_dir));
        let mut symbols = Vec::new();
        scan_file(&base_file, "src/model/base.ts", &mut symbols);
        scan_file(&service_file, "src/service/service.ts", &mut symbols);
        assert!(replace_all_symbol_rows_at(
            &root,
            &data_dir,
            &symbols,
            catalog.revision,
        ));
        let service = symbols
            .iter()
            .find(|symbol| symbol.name == "ServiceModel")
            .cloned()
            .unwrap();
        let (resolved, _) = query_persisted_edges_at(&root, &data_dir, &[service])
            .unwrap()
            .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].target_file, "src/model/base.ts");
        assert_eq!(resolved[0].target_name, "BaseModel");
        assert_eq!(resolved[0].target_line, 1);
        let base = symbols
            .iter()
            .find(|symbol| symbol.name == "BaseModel")
            .cloned()
            .unwrap();
        let (incoming, _) = query_persisted_edges_at(&root, &data_dir, &[base])
            .unwrap()
            .unwrap();
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].source_name, "ServiceModel");

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn tsconfig_alias_replacements_remain_conservative_when_ambiguous() {
        let aliases = ModuleAliases {
            tsconfig: Some(TsconfigAliases {
                base_dir: String::new(),
                rules: vec![TsconfigPathRule {
                    pattern: "@model/*".into(),
                    replacements: vec!["src/model/*".into(), "generated/model/*".into()],
                }],
            }),
            ohpm: Vec::new(),
        };
        let candidates = module_candidates("src/service.ts", "@model/base", Some(&aliases));
        assert!(candidates.contains(&"src/model/base.ts".to_string()));
        assert!(candidates.contains(&"generated/model/base.ts".to_string()));
    }

    #[test]
    fn reexports_parse_named_aliases_and_plain_stars() {
        let exports = parse_module_reexports(
            "export { Core as PublicCore, Other } from './core';\nexport * from './wild';\nexport * as Namespace from './namespace';\n",
            "ts",
        );
        assert_eq!(
            exports,
            vec![
                ModuleReexport {
                    exported_name: "*".into(),
                    target_module: "./wild".into(),
                    imported_name: "*".into(),
                },
                ModuleReexport {
                    exported_name: "Other".into(),
                    target_module: "./core".into(),
                    imported_name: "Other".into(),
                },
                ModuleReexport {
                    exported_name: "PublicCore".into(),
                    target_module: "./core".into(),
                    imported_name: "Core".into(),
                },
            ]
        );
    }

    #[test]
    fn star_reexport_closure_requires_one_final_definition() {
        let root =
            std::env::temp_dir().join(format!("deveco-star-edge-db-{}", uuid::Uuid::new_v4()));
        let data_dir =
            std::env::temp_dir().join(format!("deveco-star-edge-data-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(root.join("one.ts"), "export interface Shared {}\n").unwrap();
        std::fs::write(root.join("two.ts"), "export interface Other {}\n").unwrap();
        std::fs::write(
            root.join("index.ts"),
            "export * from './one';\nexport * from './two';\n",
        )
        .unwrap();
        std::fs::write(
            root.join("service.ts"),
            "import { Shared } from './index';\nexport class Service implements Shared {}\n",
        )
        .unwrap();
        let (files, catalog) = collect_files_at(&root, Some(&data_dir));
        let mut symbols = Vec::new();
        for rel in files.keys() {
            scan_file(&root.join(rel), rel, &mut symbols);
        }
        let indexed_files = files.keys().cloned().collect::<Vec<_>>();
        assert!(replace_all_symbol_rows_with_files_at(
            &root,
            &data_dir,
            &symbols,
            &indexed_files,
            catalog.revision,
        ));
        let service = symbols
            .iter()
            .find(|symbol| symbol.name == "Service")
            .cloned()
            .unwrap();
        let (unique, _) = query_persisted_edges_at(&root, &data_dir, &[service.clone()])
            .unwrap()
            .unwrap();
        assert_eq!(unique.len(), 1);
        assert_eq!(unique[0].target_file, "one.ts");
        assert_eq!(unique[0].target_name, "Shared");

        std::fs::write(root.join("two.ts"), "export interface Shared {}\n").unwrap();
        let mut changed = Vec::new();
        scan_file(&root.join("two.ts"), "two.ts", &mut changed);
        assert!(replace_changed_symbol_rows_at(
            &root,
            &data_dir,
            &["two.ts".into()],
            &changed,
        ));
        let (ambiguous, _) = query_persisted_edges_at(&root, &data_dir, &[service])
            .unwrap()
            .unwrap();
        assert_eq!(ambiguous.len(), 1);
        assert!(ambiguous[0].target_file.is_empty());
        assert_eq!(ambiguous[0].target_line, 0);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn direct_calls_resolve_local_imported_barrel_and_recursive_targets() {
        let root =
            std::env::temp_dir().join(format!("deveco-call-edge-db-{}", uuid::Uuid::new_v4()));
        let data_dir =
            std::env::temp_dir().join(format!("deveco-call-edge-data-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(root.join("util.ts"), "export function loadData() {}\n").unwrap();
        std::fs::write(
            root.join("barrel.ts"),
            "export { loadData as fetchData } from './util';\n",
        )
        .unwrap();
        std::fs::write(
            root.join("service.ts"),
            "import { fetchData as fetch } from './barrel';\n\
             function helper() {}\n\
             export function run() { helper(); fetch(); client.fetch(); }\n\
             export function recursive() { recursive(); }\n\
             export function outer() { const inner = () => { helper(); }; }\n",
        )
        .unwrap();
        let (files, catalog) = collect_files_at(&root, Some(&data_dir));
        let mut symbols = Vec::new();
        for rel in files.keys() {
            scan_file(&root.join(rel), rel, &mut symbols);
        }
        let indexed_files = files.keys().cloned().collect::<Vec<_>>();
        assert!(replace_all_symbol_rows_with_files_at(
            &root,
            &data_dir,
            &symbols,
            &indexed_files,
            catalog.revision,
        ));
        let find = |name: &str| {
            symbols
                .iter()
                .find(|symbol| symbol.name == name)
                .cloned()
                .unwrap()
        };
        let run = find("run");
        assert_eq!(
            run.declared_relations
                .iter()
                .filter(|relation| relation.kind == "calls")
                .count(),
            2,
            "成员调用不能作为无类型信息的直接调用绑定",
        );
        let (calls, _) = query_persisted_edges_at(&root, &data_dir, &[run])
            .unwrap()
            .unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().any(|edge| {
            edge.kind == "calls"
                && edge.target_file == "service.ts"
                && edge.target_name == "helper"
        }));
        assert!(calls.iter().any(|edge| {
            edge.kind == "calls"
                && edge.target_file == "util.ts"
                && edge.target_name == "loadData"
        }));

        let recursive = find("recursive");
        let (recursive_calls, _) = query_persisted_edges_at(&root, &data_dir, &[recursive])
            .unwrap()
            .unwrap();
        assert_eq!(recursive_calls.len(), 1);
        assert_eq!(recursive_calls[0].source_name, "recursive");
        assert_eq!(recursive_calls[0].target_name, "recursive");

        let outer = find("outer");
        assert!(outer
            .declared_relations
            .iter()
            .all(|relation| relation.kind != "calls"));
        let inner = find("inner");
        assert!(inner
            .declared_relations
            .iter()
            .any(|relation| relation.kind == "calls" && relation.target_name == "helper"));

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn lsp_definition_records_fresh_member_call_evidence() {
        let root =
            std::env::temp_dir().join(format!("deveco-lsp-call-db-{}", uuid::Uuid::new_v4()));
        let data_dir =
            std::env::temp_dir().join(format!("deveco-lsp-call-data-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        let source_file = root.join("service.ts");
        let target_file = root.join("client.ts");
        let source = "export class Service {\n  run() { client.fetch(); }\n}\n";
        std::fs::write(&source_file, source).unwrap();
        std::fs::write(
            &target_file,
            "export class Client {\n  fetch() {}\n}\n",
        )
        .unwrap();
        let (files, catalog) = collect_files_at(&root, Some(&data_dir));
        let mut symbols = Vec::new();
        for rel in files.keys() {
            scan_file(&root.join(rel), rel, &mut symbols);
        }
        assert!(replace_all_symbol_rows_at(
            &root,
            &data_dir,
            &symbols,
            catalog.revision,
        ));
        let call_column = source.lines().nth(1).unwrap().find("fetch").unwrap();
        assert!(record_lsp_call_definition_at(
            &root,
            &data_dir,
            &source_file,
            1,
            call_column,
            &target_file,
            1,
        ));
        assert!(!record_lsp_call_definition_at(
            &root,
            &data_dir,
            &source_file,
            0,
            13,
            &target_file,
            1,
        ));
        let object_column = source.lines().nth(1).unwrap().find("client").unwrap();
        assert!(!record_lsp_call_definition_at(
            &root,
            &data_dir,
            &source_file,
            1,
            object_column,
            &target_file,
            1,
        ));
        let run = symbols
            .iter()
            .find(|symbol| symbol.name == "run")
            .cloned()
            .unwrap();
        let (edges, total) = query_persisted_edges_at(&root, &data_dir, &[run.clone()])
            .unwrap()
            .unwrap();
        assert!(edges.iter().any(|edge| {
            edge.kind == "calls"
                && edge.source_name == "run"
                && edge.target_file == "client.ts"
                && edge.target_name == "fetch"
        }));
        assert!(total >= 2, "contains 与语义 calls 都应计入关系总数");

        std::fs::write(
            &target_file,
            "export class Client {\n  renamed() {}\n}\n",
        )
        .unwrap();
        assert!(matches!(
            apply_catalog_changes_at(&root, &data_dir, &["client.ts".into()]),
            CatalogDelta::Updated(_)
        ));
        let mut changed_target_symbols = Vec::new();
        scan_file(&target_file, "client.ts", &mut changed_target_symbols);
        assert!(replace_changed_symbol_rows_at(
            &root,
            &data_dir,
            &["client.ts".into()],
            &changed_target_symbols,
        ));
        let (invalid_target, _) = query_persisted_edges_at(&root, &data_dir, &[run.clone()])
            .unwrap()
            .unwrap();
        assert!(invalid_target
            .iter()
            .all(|edge| !(edge.kind == "calls" && edge.target_name == "fetch")));

        std::fs::write(
            &target_file,
            "export class Client {\n  fetch() {}\n}\n",
        )
        .unwrap();
        assert!(matches!(
            apply_catalog_changes_at(&root, &data_dir, &["client.ts".into()]),
            CatalogDelta::Updated(_)
        ));
        let mut restored_target_symbols = Vec::new();
        scan_file(&target_file, "client.ts", &mut restored_target_symbols);
        assert!(replace_changed_symbol_rows_at(
            &root,
            &data_dir,
            &["client.ts".into()],
            &restored_target_symbols,
        ));
        assert!(record_lsp_call_definition_at(
            &root,
            &data_dir,
            &source_file,
            1,
            call_column,
            &target_file,
            1,
        ));

        std::fs::write(
            &source_file,
            "export class Service {\n  run() { client.otherLonger(); }\n}\n",
        )
        .unwrap();
        assert!(matches!(
            apply_catalog_changes_at(&root, &data_dir, &["service.ts".into()]),
            CatalogDelta::Updated(_)
        ));
        let (stale_safe, _) = query_persisted_edges_at(&root, &data_dir, &[run])
            .unwrap()
            .unwrap();
        assert!(stale_safe
            .iter()
            .all(|edge| !(edge.kind == "calls" && edge.target_name == "fetch")));

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn lsp_reference_batch_deduplicates_and_records_only_member_calls() {
        let root =
            std::env::temp_dir().join(format!("deveco-lsp-batch-db-{}", uuid::Uuid::new_v4()));
        let data_dir = std::env::temp_dir().join(format!(
            "deveco-lsp-batch-data-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        let first_file = root.join("first.ts");
        let second_file = root.join("second.ts");
        let target_file = root.join("client.ts");
        let first_source = "export function first() { client.fetch(); }\nexport function second() { client.fetch(); }\n";
        let second_source = "export function third() { fetch(); client.fetch(); }\n";
        std::fs::write(&first_file, first_source).unwrap();
        std::fs::write(&second_file, second_source).unwrap();
        std::fs::write(&target_file, "export class Client {\n  fetch() {}\n}\n").unwrap();
        let (files, catalog) = collect_files_at(&root, Some(&data_dir));
        let mut symbols = Vec::new();
        for rel in files.keys() {
            scan_file(&root.join(rel), rel, &mut symbols);
        }
        assert!(replace_all_symbol_rows_at(
            &root,
            &data_dir,
            &symbols,
            catalog.revision,
        ));

        let first_fetch = first_source.lines().next().unwrap().find("fetch").unwrap();
        let second_fetch = first_source.lines().nth(1).unwrap().find("fetch").unwrap();
        let third_line = second_source.lines().next().unwrap();
        let direct_fetch = third_line.find("fetch").unwrap();
        let member_fetch = third_line.rfind("fetch").unwrap();
        let object = third_line.find("client").unwrap();
        let references = vec![
            (first_file.clone(), 0, first_fetch),
            (first_file.clone(), 0, first_fetch),
            (first_file.clone(), 1, second_fetch),
            (second_file.clone(), 0, direct_fetch),
            (second_file.clone(), 0, object),
            (second_file.clone(), 0, member_fetch),
        ];
        let pending = next_lsp_semantic_targets_at(&root, &data_dir, 4);
        assert_eq!(pending.len(), 4);
        assert_eq!(pending[0].name, "fetch", "成员方法应优先于顶层函数");
        assert_eq!(pending[0].line, 1);
        assert_eq!(pending[0].column, 2);
        assert_eq!(
            record_lsp_scan_failure_at(&root, &data_dir, &target_file, 1),
            30
        );
        let coverage = persisted_semantic_coverage_at(&root, &data_dir).unwrap();
        assert_eq!(coverage.backoff_targets, 1);
        assert!(next_lsp_semantic_targets_at(&root, &data_dir, 4)
            .iter()
            .all(|target| !(target.path == target_file && target.name == "fetch")));
        let conn = Connection::open(catalog_file_at(&data_dir, &root)).unwrap();
        conn.execute("UPDATE semantic_scan_failures SET retry_after=0", [])
            .unwrap();
        assert!(next_lsp_semantic_targets_at(&root, &data_dir, 4)
            .iter()
            .any(|target| target.path == target_file && target.name == "fetch"));
        assert_eq!(
            record_lsp_scan_failure_at(&root, &data_dir, &target_file, 1),
            60
        );
        assert_eq!(
            record_lsp_call_references_at(
                &root,
                &data_dir,
                &target_file,
                1,
                &references,
                false,
            ),
            3
        );
        let conn = Connection::open(catalog_file_at(&data_dir, &root)).unwrap();
        let (rows, stats): (i64, i64) = (
            conn.query_row("SELECT COUNT(*) FROM semantic_call_edges", [], |row| row.get(0))
                .unwrap(),
            conn.query_row(
                "SELECT semantic_relation_count FROM structure_stats WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap(),
        );
        assert_eq!(rows, 3);
        assert_eq!(stats, 3);
        let coverage = persisted_semantic_coverage_at(&root, &data_dir).unwrap();
        assert_eq!(coverage.indexed_logic_symbols, 4);
        assert_eq!(coverage.scanned_logic_symbols, 1);
        assert_eq!(coverage.semantic_call_relations, 3);
        assert_eq!(coverage.truncated_targets, 0);
        assert_eq!(coverage.backoff_targets, 0, "成功扫描应清除失败退避");
        assert_eq!(coverage.coverage_percent, 25.0);
        assert_eq!(coverage.coverage, "partial_query_driven");
        assert!(next_lsp_semantic_targets_at(&root, &data_dir, 4)
            .iter()
            .all(|target| !(target.path == target_file && target.name == "fetch")));

        assert_eq!(
            record_lsp_call_references_at(
                &root,
                &data_dir,
                &target_file,
                1,
                &references,
                true,
            ),
            3
        );
        let coverage = persisted_semantic_coverage_at(&root, &data_dir).unwrap();
        assert_eq!(coverage.scanned_logic_symbols, 1, "重复扫描不能重复计数");
        assert_eq!(coverage.truncated_targets, 1);
        assert_eq!(coverage.coverage, "partial_with_truncated_targets");
        assert_eq!(
            record_lsp_scan_failure_at(&root, &data_dir, &target_file, 1),
            30
        );

        std::fs::write(&target_file, "export class Client {\n  renamed() {}\n}\n").unwrap();
        assert!(matches!(
            apply_catalog_changes_at(&root, &data_dir, &["client.ts".into()]),
            CatalogDelta::Updated(_)
        ));
        let mut changed_target_symbols = Vec::new();
        scan_file(&target_file, "client.ts", &mut changed_target_symbols);
        assert!(replace_changed_symbol_rows_at(
            &root,
            &data_dir,
            &["client.ts".into()],
            &changed_target_symbols,
        ));
        let coverage = persisted_semantic_coverage_at(&root, &data_dir).unwrap();
        assert_eq!(coverage.indexed_logic_symbols, 4);
        assert_eq!(coverage.scanned_logic_symbols, 0);
        assert_eq!(coverage.semantic_call_relations, 0);
        assert_eq!(coverage.truncated_targets, 0);
        assert_eq!(coverage.backoff_targets, 0, "目标变化应清除失败退避");
        assert_eq!(coverage.coverage, "not_started_query_driven");

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn named_reexport_chain_resolves_forward_and_reverse_edges() {
        let root =
            std::env::temp_dir().join(format!("deveco-reexport-edge-db-{}", uuid::Uuid::new_v4()));
        let data_dir = std::env::temp_dir().join(format!(
            "deveco-reexport-edge-data-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join("barrel/inner")).unwrap();
        std::fs::create_dir_all(root.join("model-next")).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        let contract_file = root.join("contract.ts");
        let next_contract_file = root.join("model-next/contract.ts");
        let inner_file = root.join("barrel/inner/index.ts");
        let barrel_file = root.join("barrel/index.ts");
        let service_file = root.join("service.ts");
        std::fs::write(&contract_file, "export interface CoreContract {}\n").unwrap();
        std::fs::write(&next_contract_file, "export interface CoreContract {}\n").unwrap();
        std::fs::write(
            &inner_file,
            "export { CoreContract as InternalContract } from '../../contract';\n",
        )
        .unwrap();
        std::fs::write(
            &barrel_file,
            "export { InternalContract as PublicContract } from './inner';\n",
        )
        .unwrap();
        std::fs::write(
            &service_file,
            "import { PublicContract as Contract } from './barrel';\nexport class Service implements Contract {}\n",
        )
        .unwrap();
        let (files, catalog) = collect_files_at(&root, Some(&data_dir));
        let mut symbols = Vec::new();
        for rel in files.keys() {
            scan_file(&root.join(rel), rel, &mut symbols);
        }
        let mut indexed_files = files.keys().cloned().collect::<Vec<_>>();
        indexed_files.sort();
        assert!(replace_all_symbol_rows_with_files_at(
            &root,
            &data_dir,
            &symbols,
            &indexed_files,
            catalog.revision,
        ));
        let service = symbols
            .iter()
            .find(|symbol| symbol.name == "Service")
            .cloned()
            .unwrap();
        let contract = symbols
            .iter()
            .find(|symbol| symbol.name == "CoreContract" && symbol.file == "contract.ts")
            .cloned()
            .unwrap();
        let (forward, _) = query_persisted_edges_at(&root, &data_dir, &[service.clone()])
            .unwrap()
            .unwrap();
        assert_eq!(forward.len(), 1);
        assert_eq!(forward[0].target_file, "contract.ts");
        assert_eq!(forward[0].target_name, "CoreContract");
        assert_eq!(forward[0].target_line, 1);
        let (reverse, _) = query_persisted_edges_at(&root, &data_dir, &[contract])
            .unwrap()
            .unwrap();
        assert_eq!(reverse.len(), 1);
        assert_eq!(reverse[0].source_name, "Service");

        std::fs::write(
            &inner_file,
            "export { CoreContract as InternalContract } from '../../model-next/contract';\n",
        )
        .unwrap();
        assert!(replace_changed_symbol_rows_at(
            &root,
            &data_dir,
            &["barrel/inner/index.ts".into()],
            &[],
        ));
        let next_contract = symbols
            .iter()
            .find(|symbol| {
                symbol.name == "CoreContract" && symbol.file == "model-next/contract.ts"
            })
            .cloned()
            .unwrap();
        let (repointed, _) = query_persisted_edges_at(&root, &data_dir, &[service])
            .unwrap()
            .unwrap();
        assert_eq!(repointed.len(), 1);
        assert_eq!(repointed[0].target_file, "model-next/contract.ts");
        let (new_reverse, _) = query_persisted_edges_at(&root, &data_dir, &[next_contract])
            .unwrap()
            .unwrap();
        assert_eq!(new_reverse.len(), 1);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn ambiguous_or_cyclic_reexports_stay_unresolved() {
        let root =
            std::env::temp_dir().join(format!("deveco-reexport-safe-db-{}", uuid::Uuid::new_v4()));
        let data_dir =
            std::env::temp_dir().join(format!("deveco-reexport-safe-data-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(root.join("a.ts"), "export { Loop } from './b';\n").unwrap();
        std::fs::write(root.join("b.ts"), "export { Loop } from './a';\n").unwrap();
        std::fs::write(root.join("one.ts"), "export interface Value {}\n").unwrap();
        std::fs::write(root.join("two.ts"), "export interface Value {}\n").unwrap();
        std::fs::write(
            root.join("ambiguous.ts"),
            "export { Value } from './one';\nexport { Value } from './two';\n",
        )
        .unwrap();
        std::fs::write(
            root.join("service.ts"),
            "import { Loop } from './a';\nimport { Value } from './ambiguous';\nexport class LoopService implements Loop {}\nexport class ValueService implements Value {}\n",
        )
        .unwrap();
        let (files, catalog) = collect_files_at(&root, Some(&data_dir));
        let mut symbols = Vec::new();
        for rel in files.keys() {
            scan_file(&root.join(rel), rel, &mut symbols);
        }
        let indexed_files = files.keys().cloned().collect::<Vec<_>>();
        assert!(replace_all_symbol_rows_with_files_at(
            &root,
            &data_dir,
            &symbols,
            &indexed_files,
            catalog.revision,
        ));
        for name in ["LoopService", "ValueService"] {
            let source = symbols
                .iter()
                .find(|symbol| symbol.name == name)
                .cloned()
                .unwrap();
            let (edges, _) = query_persisted_edges_at(&root, &data_dir, &[source])
                .unwrap()
                .unwrap();
            assert_eq!(edges.len(), 1);
            assert!(edges[0].target_file.is_empty());
            assert_eq!(edges[0].target_line, 0);
        }
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn ohpm_file_dependency_resolves_explicit_package_entry() {
        let root =
            std::env::temp_dir().join(format!("deveco-ohpm-edge-db-{}", uuid::Uuid::new_v4()));
        let data_dir =
            std::env::temp_dir().join(format!("deveco-ohpm-edge-data-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("entry/src/main/ets")).unwrap();
        std::fs::create_dir_all(root.join("shared/core/src/main/ets")).unwrap();
        std::fs::create_dir_all(root.join("shared/core-next/src/main/ets")).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(
            root.join("oh-package.json5"),
            r#"{"dependencies":{"@app/core":"file:./shared/core-next"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("entry/oh-package.json5"),
            r#"{"dependencies":{"@app/core":"file:../shared/core"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("shared/core/oh-package.json5"),
            r#"{"name":"@app/core","main":"src/main/ets/Index.ets"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("shared/core-next/oh-package.json5"),
            r#"{"name":"@app/core","main":"src/main/ets/Index.ets"}"#,
        )
        .unwrap();
        let base_file = root.join("shared/core/src/main/ets/Index.ets");
        let next_base_file = root.join("shared/core-next/src/main/ets/Index.ets");
        let service_file = root.join("entry/src/main/ets/Service.ets");
        std::fs::write(&base_file, "export interface CoreContract {}\n").unwrap();
        std::fs::write(&next_base_file, "export interface CoreContract {}\n").unwrap();
        std::fs::write(
            &service_file,
            "import { CoreContract as Contract } from '@app/core';\nexport class Service implements Contract {}\n",
        )
        .unwrap();
        let (_, catalog) = collect_files_at(&root, Some(&data_dir));
        let mut symbols = Vec::new();
        scan_file(
            &base_file,
            "shared/core/src/main/ets/Index.ets",
            &mut symbols,
        );
        scan_file(
            &next_base_file,
            "shared/core-next/src/main/ets/Index.ets",
            &mut symbols,
        );
        scan_file(
            &service_file,
            "entry/src/main/ets/Service.ets",
            &mut symbols,
        );
        assert!(replace_all_symbol_rows_at(
            &root,
            &data_dir,
            &symbols,
            catalog.revision,
        ));
        let service = symbols
            .iter()
            .find(|symbol| symbol.name == "Service")
            .cloned()
            .unwrap();
        let (resolved, _) = query_persisted_edges_at(&root, &data_dir, &[service])
            .unwrap()
            .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].target_file,
            "shared/core/src/main/ets/Index.ets"
        );
        assert_eq!(resolved[0].target_name, "CoreContract");
        assert_eq!(resolved[0].target_line, 1);

        let contract = symbols
            .iter()
            .find(|symbol| {
                symbol.name == "CoreContract"
                    && symbol.file == "shared/core/src/main/ets/Index.ets"
            })
            .cloned()
            .unwrap();
        let (incoming, _) = query_persisted_edges_at(&root, &data_dir, &[contract])
            .unwrap()
            .unwrap();
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].source_name, "Service");

        std::fs::write(
            root.join("entry/oh-package.json5"),
            r#"{"dependencies":{"@app/core":"file:../shared/core-next"}}"#,
        )
        .unwrap();
        let next_contract = symbols
            .iter()
            .find(|symbol| {
                symbol.name == "CoreContract"
                    && symbol.file == "shared/core-next/src/main/ets/Index.ets"
            })
            .cloned()
            .unwrap();
        let (repointed, _) = query_persisted_edges_at(&root, &data_dir, &[next_contract])
            .unwrap()
            .unwrap();
        assert_eq!(repointed.len(), 1, "清单改指向后不应要求重建全库入边");
        assert_eq!(
            repointed[0].target_file,
            "shared/core-next/src/main/ets/Index.ets"
        );

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn ohpm_remote_or_entryless_dependency_stays_unresolved() {
        let root =
            std::env::temp_dir().join(format!("deveco-ohpm-safe-db-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("entry/src/main/ets")).unwrap();
        std::fs::create_dir_all(root.join("shared/core")).unwrap();
        std::fs::write(
            root.join("entry/oh-package.json5"),
            r#"{"dependencies":{"remote":"^1.0.0","entryless":"file:../shared/core"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("shared/core/oh-package.json5"),
            r#"{"name":"entryless"}"#,
        )
        .unwrap();
        let aliases = load_module_aliases(&root, ["entry/src/main/ets/Service.ets"].into_iter());
        assert!(module_candidates(
            "entry/src/main/ets/Service.ets",
            "remote",
            Some(&aliases),
        )
        .is_empty());
        assert!(module_candidates(
            "entry/src/main/ets/Service.ets",
            "entryless",
            Some(&aliases),
        )
        .is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn deferred_files_promote_in_bounded_batches_and_detect_stale_input() {
        let root =
            std::env::temp_dir().join(format!("deveco-deferred-db-{}", uuid::Uuid::new_v4()));
        let data_dir =
            std::env::temp_dir().join(format!("deveco-deferred-data-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        for name in ["a", "b", "c"] {
            std::fs::write(
                root.join(format!("{name}.ets")),
                format!("@Component\nstruct {name} {{\n  run() {{\n  }}\n}}\n"),
            )
            .unwrap();
        }
        let (_, catalog) = collect_files_at_with_budget(&root, Some(&data_dir), 1);
        assert_eq!(catalog.indexed_source_files, 1);
        assert_eq!(catalog.deferred_source_files, 2);

        let cancelled = promote_deferred_batch_at_if(&root, &data_dir, 1, || true).unwrap();
        assert_eq!(cancelled.promoted, 0);
        assert!(cancelled.catalog.is_none());

        let first = promote_deferred_batch_at(&root, &data_dir, 1).unwrap();
        let first_catalog = first.catalog.expect("成功批次应刷新目录统计");
        assert_eq!(first.promoted, 1);
        assert_eq!(first_catalog.indexed_source_files, 2);
        assert_eq!(first_catalog.deferred_source_files, 1);
        assert!(!first.needs_reconciliation);
        let (_, reconciled) = collect_files_at_with_budget(&root, Some(&data_dir), 1);
        assert_eq!(reconciled.indexed_source_files, 2, "已提升文件不应被基础预算降级");
        assert_eq!(reconciled.deferred_source_files, 1);
        let conn = Connection::open(catalog_file_at(&data_dir, &root)).unwrap();
        let nodes: usize = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get::<_, i64>(0))
            .unwrap()
            .max(0) as usize;
        let edges: usize = conn
            .query_row("SELECT COUNT(*) FROM symbol_edges", [], |row| row.get::<_, i64>(0))
            .unwrap()
            .max(0) as usize;
        assert!(nodes > 0);
        assert_eq!(edges, 1);
        drop(conn);

        std::fs::write(
            root.join("c.ets"),
            "@Component\nstruct ChangedExternally {\n  refresh() {\n  }\n}\n",
        )
        .unwrap();
        let stale = promote_deferred_batch_at(&root, &data_dir, 1).unwrap();
        let stale_catalog = stale.catalog.expect("非取消批次应刷新目录统计");
        assert_eq!(stale.promoted, 0);
        assert!(stale.needs_reconciliation);
        assert_eq!(stale_catalog.deferred_source_files, 1);
        assert!(catalog.revision > 0);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn progressive_backpressure_increases_for_slow_batches() {
        assert_eq!(progressive_throttle_ms(0), 20);
        assert_eq!(progressive_throttle_ms(74), 20);
        assert_eq!(progressive_throttle_ms(75), 50);
        assert_eq!(progressive_throttle_ms(200), 100);
        assert_eq!(progressive_throttle_ms(500), 200);
        assert_eq!(progressive_throttle_ms(10_000), 200);
    }

    #[test]
    fn git_checkpoint_lists_paths_across_head_changes() {
        let root =
            std::env::temp_dir().join(format!("deveco-git-checkpoint-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .ok()
                .filter(|output| output.status.success())
        };
        if run(&["init", "-q"]).is_none() {
            std::fs::remove_dir_all(&root).ok();
            return;
        }
        run(&["config", "user.name", "HarmonyAgent Test"]).unwrap();
        run(&["config", "user.email", "harmony-agent@example.invalid"]).unwrap();
        std::fs::write(root.join("old.rs"), "fn old() {}\n").unwrap();
        run(&["add", "old.rs"]).unwrap();
        run(&["commit", "-q", "-m", "first"]).unwrap();
        let previous = git_checkpoint(&root).expect("首次提交应有 Git 指纹");

        std::fs::remove_file(root.join("old.rs")).unwrap();
        std::fs::write(root.join("new.rs"), "fn new() {}\n").unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-q", "-m", "second"]).unwrap();
        let current = git_checkpoint(&root).expect("第二次提交应有 Git 指纹");
        let paths = git_changed_paths(&root, &previous, &current).expect("HEAD diff 应可精确枚举");
        assert_eq!(paths, vec!["new.rs", "old.rs"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn structure_query_filters_roles_and_paginates_with_coverage() {
        let dir = make_project("structure-query");
        let first = query_structure(&dir, "", Some("entity"), None, None, 1, 1);
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].role, "entity");
        assert_eq!(first.page_size, 1);
        assert!(first.next_page.is_some());
        assert_eq!(first.coverage, "best_effort_lightweight_syntax_index");
        assert_eq!(first.catalog.discovered_files, 2);
        assert_eq!(first.catalog.indexed_source_files, 2);

        let logic = query_structure(&dir, "oldA", Some("logic"), None, Some("a.ets"), 1, 20);
        assert_eq!(logic.total_matches, 1);
        assert_eq!(logic.items[0].name, "oldA");
        assert_eq!(logic.items[0].role, "logic");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 辅助：建一个含两个 ets 文件的临时项目目录
    fn make_project(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("deveco-symbol-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.ets"), "struct Aaa {}\nfn oldA() {}").unwrap();
        std::fs::write(dir.join("b.ets"), "struct Bbb {}").unwrap();
        dir
    }

    fn peak_rss_kib() -> Option<u64> {
        #[cfg(unix)]
        {
            let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
            // SAFETY: getrusage initializes the provided rusage buffer for the current process.
            if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
                return None;
            }
            // SAFETY: a successful getrusage call initialized the whole rusage value.
            let bytes_or_kib = unsafe { usage.assume_init() }.ru_maxrss.max(0) as u64;
            #[cfg(target_os = "macos")]
            return Some(bytes_or_kib / 1024);
            #[cfg(not(target_os = "macos"))]
            return Some(bytes_or_kib);
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    #[test]
    fn sync_incremental_rescans_only_changed() {
        let dir = make_project("incr");
        let mut entry = CacheEntry {
            files: HashMap::new(),
            syms: Vec::new(),
            catalog: CatalogStats::default(),
            git_checkpoint: None,
            needs_reconciliation: false,
            last_sync: 0,
            source: "scan",
        };
        // 首次同步：两个文件都是新增
        let (r1, _, catalog) = sync_incremental(&mut entry.files, &mut entry.syms, &dir);
        entry.catalog = catalog;
        assert_eq!(r1, 2);
        assert!(entry.syms.iter().any(|s| s.name == "Aaa"));
        assert!(entry.syms.iter().any(|s| s.name == "oldA"));
        assert!(entry.syms.iter().any(|s| s.name == "Bbb"));
        // 只改 a.ets（长度变化 → 指纹变化）
        std::fs::write(dir.join("a.ets"), "struct Aaa {}\nfn oldA() {}\nfn newA() {}").unwrap();
        let (r2, _, catalog) = sync_incremental(&mut entry.files, &mut entry.syms, &dir);
        entry.catalog = catalog;
        assert_eq!(r2, 1, "只有 a.ets 应被重扫");
        assert!(entry.syms.iter().any(|s| s.name == "newA"), "变化文件的新符号应出现");
        assert!(entry.syms.iter().any(|s| s.name == "Bbb"), "未变文件符号应保留");
        assert_eq!(entry.syms.iter().filter(|s| s.name == "oldA").count(), 1, "旧符号不应重复");
        // 删除 b.ets
        std::fs::remove_file(dir.join("b.ets")).unwrap();
        let (_, removed, catalog) = sync_incremental(&mut entry.files, &mut entry.syms, &dir);
        entry.catalog = catalog;
        assert!(removed > 0);
        assert!(!entry.syms.iter().any(|s| s.name == "Bbb"), "被删文件符号应移除");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalidate_files_updates_only_target() {
        let dir = make_project("invf");
        // 构建内存条目（DATA_DIR 未初始化 → 纯内存）
        let syms = index_project_cached(&dir);
        assert!(syms.iter().any(|s| s.name == "Aaa"));
        assert!(syms.iter().any(|s| s.name == "Bbb"));
        // 改 a.ets 后精确失效：冷却期内应直接看到更新后的符号
        std::fs::write(dir.join("a.ets"), "struct Aaa {}\nfn newA() {}").unwrap();
        invalidate_files(&dir, &["a.ets".to_string()]);
        let syms2 = index_project_cached(&dir);
        assert!(syms2.iter().any(|s| s.name == "newA"), "精确失效后应看到新符号");
        assert!(syms2.iter().any(|s| s.name == "Bbb"), "其他文件符号不受影响");
        assert!(!syms2.iter().any(|s| s.name == "oldA"), "被替换的旧符号应移除");
        // 删除 b.ets 后精确失效
        std::fs::remove_file(dir.join("b.ets")).unwrap();
        invalidate_files(&dir, &["b.ets".to_string()]);
        let syms3 = index_project_cached(&dir);
        assert!(!syms3.iter().any(|s| s.name == "Bbb"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalidate_files_handles_deleted_dir() {
        let dir = make_project("invd");
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("c.ets"), "struct Ccc {}").unwrap();
        let syms = index_project_cached(&dir);
        assert!(syms.iter().any(|s| s.name == "Ccc"));
        // 删除整个子目录后按目录路径失效
        std::fs::remove_dir_all(&sub).unwrap();
        invalidate_files(&dir, &["sub".to_string()]);
        let syms2 = index_project_cached(&dir);
        assert!(!syms2.iter().any(|s| s.name == "Ccc"), "目录下文件符号应全部移除");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn persisted_roundtrip() {
        let data_dir = std::env::temp_dir().join("deveco-symbol-cache-dir");
        let _ = std::fs::remove_dir_all(&data_dir);
        std::fs::create_dir_all(&data_dir).unwrap();
        let proj = make_project("persist");
        let mut files = HashMap::new();
        files.insert("a.ets".to_string(), FileStamp { mtime: 123, len: 45 });
        let syms = vec![Symbol { kind: "struct".into(), name: "Aaa".into(), file: "a.ets".into(), line: 1, end_line: 1, role: "entity".into(), signature: "struct Aaa {}".into(), parent: None, language: "ets".into(), source_layer: "lightweight".into(), declared_relations: Vec::new() }];
        let catalog = CatalogStats {
            discovered_files: 1,
            source_files: 1,
            indexed_source_files: 1,
            persisted: true,
            ..CatalogStats::default()
        };
        save_to(&data_dir, &proj, &files, &syms, catalog);
        let loaded = load_from(&data_dir, &proj).expect("应能从磁盘恢复");
        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.files["a.ets"], FileStamp { mtime: 123, len: 45 });
        assert_eq!(loaded.syms.len(), 1);
        assert_eq!(loaded.syms[0].name, "Aaa");
        assert_eq!(loaded.catalog.discovered_files, 1);
        // 损坏内容应返回 None（触发全量重建，不 panic）
        std::fs::write(cache_file_at(&data_dir, &proj), "not-json").unwrap();
        assert!(load_from(&data_dir, &proj).is_none());
        std::fs::remove_dir_all(&data_dir).ok();
        std::fs::remove_dir_all(&proj).ok();
    }

    /// Phase 0 大仓基线。默认忽略，避免在普通 CI 中创建大量文件。
    ///
    /// 运行示例：
    /// HARMONY_INDEX_BENCH_FILES=10000 cargo test --lib \
    ///   services::symbol_index::tests::large_repo_baseline -- --ignored --exact --nocapture
    #[test]
    #[ignore = "手动大仓索引基准；通过 HARMONY_INDEX_BENCH_FILES 选择规模"]
    fn large_repo_baseline() {
        let requested = std::env::var("HARMONY_INDEX_BENCH_FILES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(10_000)
            .clamp(1, 1_000_000);
        let benchmark_ext = std::env::var("HARMONY_INDEX_BENCH_EXT")
            .ok()
            .filter(|value| matches!(value.as_str(), "ets" | "ts" | "tsx" | "js" | "jsx"))
            .unwrap_or_else(|| "ets".into());
        let files_per_shard = 1_000usize;
        let root = std::env::temp_dir().join(format!(
            "deveco-symbol-scale-{}",
            uuid::Uuid::new_v4()
        ));
        let data_dir = std::env::temp_dir().join(format!(
            "deveco-symbol-scale-data-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        let rss_before_kib = peak_rss_kib();

        let generate_started = std::time::Instant::now();
        for index in 0..requested {
            let shard = root.join(format!("shard_{:04}", index / files_per_shard));
            if index % files_per_shard == 0 {
                std::fs::create_dir_all(&shard).unwrap();
            }
            std::fs::write(
                shard.join(format!("file_{index:07}.{benchmark_ext}")),
                if benchmark_ext == "ets" {
                    format!(
                        "@Component\nstruct Page_{index:07} {{\n  symbol_{index:07}() {{\n    return {index}\n  }}\n}}\n"
                    )
                } else {
                    format!(
                        "export class Page_{index:07} {{\n  symbol_{index:07}(): number {{\n    return {index};\n  }}\n}}\n"
                    )
                },
            )
            .unwrap();
        }
        let generation_ms = generate_started.elapsed().as_millis() as u64;
        let rss_after_generation_kib = peak_rss_kib();

        let cold_started = std::time::Instant::now();
        let (files, catalog) = collect_files_at(&root, Some(&data_dir));
        let mut cold_symbols = Vec::new();
        for rel in files.keys() {
            scan_file(&root.join(rel), rel, &mut cold_symbols);
        }
        assert!(replace_all_symbol_rows_at(
            &root,
            &data_dir,
            &cold_symbols,
            catalog.revision,
        ));
        let cold_ms = cold_started.elapsed().as_millis() as u64;
        let rss_after_cold_index_kib = peak_rss_kib();

        let query_name = cold_symbols
            .iter()
            .find(|symbol| symbol.role == "logic")
            .map(|symbol| symbol.name.clone())
            .expect("基准至少应索引一个逻辑节点");
        let warm_started = std::time::Instant::now();
        let (warm_symbols, total_matches) = query_persisted_symbols_at(
            &root,
            &data_dir,
            &query_name,
            Some("logic"),
            None,
            None,
            1,
            50,
        )
        .unwrap()
        .unwrap();
        let warm_ms = warm_started.elapsed().as_millis() as u64;

        // Hold a real SQLite write lock briefly so the batch reports observable contention.
        let db_path = catalog_file_at(&data_dir, &root);
        let blocker_path = db_path.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let blocker = std::thread::spawn(move || {
            let mut conn = Connection::open(blocker_path).unwrap();
            let transaction = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            ready_tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(80));
            transaction.commit().unwrap();
        });
        ready_rx.recv().unwrap();
        let progressive_started = std::time::Instant::now();
        let progressive = promote_deferred_batch_at(&root, &data_dir, 128).unwrap();
        let progressive_batch_ms = progressive_started.elapsed().as_millis() as u64;
        blocker.join().unwrap();

        let cancel_checks = std::cell::Cell::new(0usize);
        let cancel_started = std::time::Instant::now();
        let cancelled = promote_deferred_batch_at_if(&root, &data_dir, 128, || {
            let next = cancel_checks.get().saturating_add(1);
            cancel_checks.set(next);
            next > 32
        })
        .unwrap();
        let cancellation_latency_ms = cancel_started.elapsed().as_millis() as u64;
        let rss_after_progressive_kib = peak_rss_kib();

        let changed_file = files.keys().next().cloned().expect("基准至少应索引一个文件");
        std::fs::write(
            root.join(&changed_file),
            if benchmark_ext == "ets" {
                "@Component\nstruct ChangedPage {\n  symbol_after_incremental_update() {\n    return 42\n  }\n}\n"
            } else {
                "export class ChangedPage {\n  symbol_after_incremental_update(): number {\n    return 42;\n  }\n}\n"
            },
        )
        .unwrap();
        let incremental_started = std::time::Instant::now();
        assert!(matches!(
            apply_catalog_changes_at(&root, &data_dir, std::slice::from_ref(&changed_file)),
            CatalogDelta::Updated(_)
        ));
        let mut incremental_symbols = Vec::new();
        scan_file(&root.join(&changed_file), &changed_file, &mut incremental_symbols);
        assert!(replace_changed_symbol_rows_at(
            &root,
            &data_dir,
            std::slice::from_ref(&changed_file),
            &incremental_symbols,
        ));
        let incremental_ms = incremental_started.elapsed().as_millis() as u64;
        let conn = Connection::open(catalog_file_at(&data_dir, &root)).unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);").unwrap();
        let relation_count: usize = conn
            .query_row("SELECT COUNT(*) FROM symbol_edges", [], |row| row.get::<_, i64>(0))
            .unwrap()
            .max(0) as usize;
        drop(conn);
        let database_bytes = [
            db_path.clone(),
            db_path.with_extension("sqlite3-wal"),
            db_path.with_extension("sqlite3-shm"),
        ]
        .into_iter()
        .filter_map(|path| std::fs::metadata(path).ok().map(|metadata| metadata.len()))
        .sum::<u64>();
        let peak_rss_kib = [
            rss_before_kib,
            rss_after_generation_kib,
            rss_after_cold_index_kib,
            rss_after_progressive_kib,
        ]
        .into_iter()
        .flatten()
        .max();
        assert_eq!(catalog.discovered_files, requested);
        assert_eq!(catalog.source_files, requested);
        assert_eq!(
            catalog.deferred_source_files,
            requested.saturating_sub(MAX_FILES)
        );

        let report = serde_json::json!({
            "schema_version": 4,
            "requested_files": requested,
            "benchmark_extension": benchmark_ext,
            "configured_max_files": MAX_FILES,
            "indexed_files": cold_symbols.iter().map(|symbol| &symbol.file).collect::<std::collections::HashSet<_>>().len(),
            "catalog_discovered_files": catalog.discovered_files,
            "catalog_source_files": catalog.source_files,
            "deferred_source_files": catalog.deferred_source_files,
            "coverage": catalog.coverage(),
            "cold_symbols": cold_symbols.len(),
            "tree_sitter_symbols": cold_symbols.iter().filter(|symbol| symbol.source_layer == "tree_sitter").count(),
            "lightweight_symbols": cold_symbols.iter().filter(|symbol| symbol.source_layer == "lightweight").count(),
            "indexed_relations": relation_count,
            "warm_query_matches": total_matches,
            "warm_query_page_items": warm_symbols.len(),
            "incremental_symbols": incremental_symbols.len(),
            "generation_ms": generation_ms,
            "cold_index_ms": cold_ms,
            "warm_query_ms": warm_ms,
            "single_file_incremental_ms": incremental_ms,
            "progressive_batch_files": progressive.promoted,
            "progressive_batch_ms": progressive_batch_ms,
            "progressive_lock_wait_ms": progressive.lock_wait_ms,
            "deferred_after_progressive_batch": progressive.catalog.expect("成功批次应刷新目录统计").deferred_source_files,
            "cancellation_latency_ms": cancellation_latency_ms,
            "cancellation_checks": cancel_checks.get(),
            "cancelled_batch_promoted": cancelled.promoted,
            "database_bytes": database_bytes,
            "peak_rss_before_kib": rss_before_kib,
            "peak_rss_after_generation_kib": rss_after_generation_kib,
            "peak_rss_after_cold_index_kib": rss_after_cold_index_kib,
            "peak_rss_after_progressive_kib": rss_after_progressive_kib,
            "peak_rss_kib": peak_rss_kib,
            "structure_parse_is_partial": requested > MAX_FILES,
            "platform": std::env::consts::OS,
            "architecture": std::env::consts::ARCH,
        });
        println!("HARMONY_INDEX_BASELINE={report}");

        assert!(!cold_symbols.is_empty());
        assert_eq!(total_matches, 1);
        assert_eq!(warm_symbols[0].name, query_name);
        assert!(incremental_symbols
            .iter()
            .any(|symbol| symbol.name == "symbol_after_incremental_update"));
        assert_eq!(progressive.promoted, requested.saturating_sub(MAX_FILES).min(128));
        assert_eq!(cancelled.promoted, 0);
        assert!(cancel_checks.get() <= 33);
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    /// SQLite 节点/边规模基线：不创建海量实体文件，专门测持久图存储与查询。
    #[test]
    #[ignore = "手动 1M SQLite 结构图基准；通过 HARMONY_SQLITE_BENCH_FILES 选择规模"]
    fn million_scale_sqlite_graph_baseline() {
        let requested = std::env::var("HARMONY_SQLITE_BENCH_FILES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(100_000)
            .clamp(1, 1_000_000);
        let root = std::env::temp_dir().join(format!(
            "deveco-sqlite-scale-{}",
            uuid::Uuid::new_v4()
        ));
        let data_dir = std::env::temp_dir().join(format!(
            "deveco-sqlite-scale-data-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        let _ = collect_files_at(&root, Some(&data_dir));
        let db_path = catalog_file_at(&data_dir, &root);
        let mut conn = Connection::open(&db_path).unwrap();

        let insert_started = std::time::Instant::now();
        let transaction = conn.transaction().unwrap();
        {
            let mut file_statement = transaction
                .prepare(
                    "INSERT INTO files(path, extension, size, mtime_ns, state, shard, generation)
                     VALUES(?1, 'ets', 128, 1, 'indexed', ?2, 1)",
                )
                .unwrap();
            let mut symbol_statement = transaction
                .prepare(
                    "INSERT INTO symbols(file, kind, name, line, end_line, role, signature, parent, shard)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )
                .unwrap();
            let mut edge_statement = transaction
                .prepare(
                    "INSERT INTO symbol_edges(
                       kind, source_file, source_name, source_line,
                       target_file, target_name, target_line, shard
                     ) VALUES('contains', ?1, ?2, 1, ?1, ?3, 2, ?4)",
                )
                .unwrap();
            for index in 0..requested {
                let shard = format!("shard_{:04}", index / 1_000);
                let path = format!("{shard}/file_{index:07}.ets");
                let entity = format!("Page_{index:07}");
                file_statement.execute(params![path, shard]).unwrap();
                symbol_statement
                    .execute(params![
                        path,
                        "component",
                        entity,
                        1,
                        4,
                        "entity",
                        format!("struct {entity}"),
                        Option::<String>::None,
                        shard,
                    ])
                    .unwrap();
                if index % 4 == 0 {
                    let method = format!("method_{index:07}");
                    symbol_statement
                        .execute(params![
                            path,
                            "method",
                            method,
                            2,
                            3,
                            "logic",
                            format!("{method}()"),
                            entity,
                            shard,
                        ])
                        .unwrap();
                    edge_statement
                        .execute(params![path, entity, method, shard])
                        .unwrap();
                }
            }
        }
        transaction.commit().unwrap();
        let insert_ms = insert_started.elapsed().as_millis() as u64;

        let target_index = (requested.saturating_sub(1) / 4) * 4;
        let target_name = format!("method_{target_index:07}");
        let exact_started = std::time::Instant::now();
        let (exact, exact_total) = query_persisted_symbols_at(
            &root,
            &data_dir,
            &target_name,
            Some("logic"),
            None,
            None,
            1,
            20,
        )
        .unwrap()
        .unwrap();
        let exact_ms = exact_started.elapsed().as_millis() as u64;

        let deep_page = ((requested.saturating_mul(9) / 10) / 50).max(1);
        let deep_started = std::time::Instant::now();
        let (deep_items, _) = query_persisted_symbols_at(
            &root,
            &data_dir,
            "",
            None,
            Some("component"),
            None,
            deep_page,
            50,
        )
        .unwrap()
        .unwrap();
        let deep_page_ms = deep_started.elapsed().as_millis() as u64;

        let cursor_index = requested.saturating_mul(9) / 10;
        let cursor_index = cursor_index.saturating_sub(1);
        let cursor_shard = format!("shard_{:04}", cursor_index / 1_000);
        let cursor_filter_hash = structure_filter_hash(
            &root,
            "",
            None,
            Some("component"),
            None,
        );
        let cursor = StructureCursor {
            version: 1,
            filter_hash: cursor_filter_hash,
            index_revision: 0,
            total_matches: requested,
            exact_match: false,
            file: format!("{cursor_shard}/file_{cursor_index:07}.ets"),
            line: 1,
            name: format!("Page_{cursor_index:07}"),
            row_id: (cursor_index + cursor_index.div_ceil(4) + 1) as i64,
        };
        let cursor_started = std::time::Instant::now();
        let (cursor_items, _, _) = query_persisted_symbols_keyset_at(
            &root,
            &data_dir,
            "",
            None,
            Some("component"),
            None,
            Some(&cursor),
            50,
            cursor_filter_hash,
        )
        .unwrap()
        .unwrap();
        let cursor_page_ms = cursor_started.elapsed().as_millis() as u64;

        let edge_started = std::time::Instant::now();
        let (edges, edge_total) = query_persisted_edges_at(&root, &data_dir, &exact)
            .unwrap()
            .unwrap();
        let edge_query_ms = edge_started.elapsed().as_millis() as u64;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
        drop(conn);
        let database_bytes = std::fs::metadata(&db_path).unwrap().len();

        let report = serde_json::json!({
            "schema_version": 1,
            "files": requested,
            "symbols": requested + requested.div_ceil(4),
            "relations": requested.div_ceil(4),
            "insert_ms": insert_ms,
            "exact_query_ms": exact_ms,
            "deep_page": deep_page,
            "deep_page_ms": deep_page_ms,
            "cursor_page_ms": cursor_page_ms,
            "edge_query_ms": edge_query_ms,
            "database_bytes": database_bytes,
            "platform": std::env::consts::OS,
            "architecture": std::env::consts::ARCH,
        });
        println!("HARMONY_SQLITE_GRAPH_BASELINE={report}");

        assert_eq!(exact_total, 1);
        assert_eq!(exact[0].name, target_name);
        assert!(!deep_items.is_empty());
        assert!(!cursor_items.is_empty());
        assert_eq!(edge_total, requested.div_ceil(4));
        assert_eq!(edges.len(), 1);
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }
}
