//! 鸿蒙知识库 CRUD 命令（用户可在设置页增删自定义条目，沉淀团队踩坑经验）

use chrono::Utc;
use serde::Deserialize;
use tauri::State;
use uuid::Uuid;

use crate::db::{models::KnowledgeEntry, queries, DbState};

/// 列出指定作用域的知识条目（不含内置条目时由前端控制展示）。
/// project_id 为 null 取全局，为某 id 取该项目专属。
#[tauri::command]
pub fn list_knowledge(
    db: State<DbState>,
    project_id: Option<String>,
) -> Result<Vec<KnowledgeEntry>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::list_knowledge(&conn, project_id.as_deref()).map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
pub struct KnowledgeInput {
    /// 逗号分隔的关键词
    pub keywords: String,
    pub title: String,
    #[serde(default)]
    pub cause: String,
    #[serde(default)]
    pub fix: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}
fn default_true() -> bool {
    true
}

#[tauri::command]
pub fn add_knowledge(
    db: State<DbState>,
    input: KnowledgeInput,
    project_id: Option<String>,
) -> Result<KnowledgeEntry, String> {
    let now = Utc::now().timestamp();
    let entry = KnowledgeEntry {
        id: Uuid::new_v4().to_string(),
        keywords: input.keywords.trim().to_string(),
        title: input.title.trim().to_string(),
        cause: input.cause,
        fix: input.fix,
        enabled: input.enabled,
        builtin: false,
        project_id,
        hit_count: 0,
        created_at: now,
        updated_at: None,
    };
    if entry.keywords.is_empty() || entry.title.is_empty() {
        return Err("关键词与标题不能为空".into());
    }
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::insert_knowledge(&conn, &entry).map_err(|e| e.to_string())?;
    Ok(entry)
}

#[tauri::command]
pub fn update_knowledge(
    db: State<DbState>,
    id: String,
    input: KnowledgeInput,
    project_id: Option<String>,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    // 先在当前作用域查，再在全局查，避免跨作用域误改
    let mut entry = queries::list_knowledge(&conn, project_id.as_deref())
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|e| e.id == id);
    if entry.is_none() && project_id.is_some() {
        entry = queries::list_knowledge(&conn, None)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|e| e.id == id);
    }
    let mut entry = entry.ok_or_else(|| "知识条目不存在".to_string())?;
    entry.keywords = input.keywords.trim().to_string();
    entry.title = input.title.trim().to_string();
    entry.cause = input.cause;
    entry.fix = input.fix;
    entry.enabled = input.enabled;
    entry.updated_at = Some(Utc::now().timestamp());
    queries::update_knowledge(&conn, &entry).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn toggle_knowledge(
    db: State<DbState>,
    id: String,
    enabled: bool,
    project_id: Option<String>,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut entry = queries::list_knowledge(&conn, project_id.as_deref())
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|e| e.id == id);
    if entry.is_none() && project_id.is_some() {
        entry = queries::list_knowledge(&conn, None)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|e| e.id == id);
    }
    let mut entry = entry.ok_or_else(|| "知识条目不存在".to_string())?;
    entry.enabled = enabled;
    entry.updated_at = Some(Utc::now().timestamp());
    queries::update_knowledge(&conn, &entry).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn delete_knowledge(db: State<DbState>, id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::delete_knowledge(&conn, &id).map_err(|e| e.to_string())?;
    Ok(())
}

/// 把一条知识条目在全局↔项目作用域间复制（与 MCP/Skill 的 clone 一致）。
#[tauri::command]
pub fn clone_knowledge(
    db: State<DbState>,
    id: String,
    target_project_id: Option<String>,
) -> Result<KnowledgeEntry, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let src = queries::list_knowledge(&conn, None)
        .map_err(|e| e.to_string())?
        .into_iter()
        .chain(
            queries::list_knowledge(&conn, target_project_id.as_deref())
                .map_err(|e| e.to_string())?
                .into_iter(),
        )
        .find(|e| e.id == id)
        .ok_or_else(|| "知识条目不存在".to_string())?;
    if src.project_id == target_project_id {
        return Err("目标作用域与来源相同".into());
    }
    let mut dst = src.clone();
    dst.id = Uuid::new_v4().to_string();
    dst.project_id = target_project_id;
    dst.builtin = false;
    dst.hit_count = 0;
    dst.created_at = Utc::now().timestamp();
    dst.updated_at = None;
    queries::insert_knowledge(&conn, &dst).map_err(|e| e.to_string())?;
    Ok(dst)
}

#[derive(Debug, Deserialize)]
pub struct SaveFromTextInput {
    /// 条目标题（可空，空时用错误首行自动生成）
    #[serde(default)]
    pub title: String,
    /// 错误/现象原文（用于自动提取触发关键词）
    #[serde(default)]
    pub error_text: String,
    /// 修复方法说明
    #[serde(default)]
    pub fix: String,
    /// 根因说明（可空）
    #[serde(default)]
    pub cause: String,
}

/// 从一次对话的"错误 + 修复"文本生成并保存一条知识条目。
/// 自动从 error_text 中提取有辨识度的关键词（错误码、ArkTS 标识、异常名等），
/// 让用户在聊天里点一下"记住这次修复"即可沉淀经验，无需手动想关键词。
#[tauri::command]
pub fn save_knowledge_from_text(
    db: State<DbState>,
    input: SaveFromTextInput,
    project_id: Option<String>,
) -> Result<KnowledgeEntry, String> {
    let title = if input.title.trim().is_empty() {
        derive_title(&input.error_text)
    } else {
        input.title.trim().to_string()
    };
    let keywords = extract_keywords(&input.error_text);
    if keywords.is_empty() {
        return Err("无法从文本中提取关键词，请补充错误信息或手动填写".into());
    }
    if title.is_empty() {
        return Err("标题不能为空".into());
    }
    let entry = KnowledgeEntry {
        id: Uuid::new_v4().to_string(),
        keywords,
        title,
        cause: input.cause,
        fix: input.fix,
        enabled: true,
        builtin: false,
        project_id,
        hit_count: 0,
        created_at: Utc::now().timestamp(),
        updated_at: None,
    };
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::insert_knowledge(&conn, &entry).map_err(|e| e.to_string())?;
    Ok(entry)
}

/// 从错误文本提取有辨识度的关键词（逗号分隔）。
/// 策略：取错误码（数字串）、全大写标识、CamelCase 异常/类型名、hvigor/ArkTS 关键 token，
/// 去重后最多 6 个。
fn extract_keywords(text: &str) -> String {
    let mut kws: Vec<String> = Vec::new();
    for token in text.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | ',' | ';' | ':' | '\'' | '"' | '`' | '，' | '。')) {
        let t = token.trim();
        if t.len() < 3 || t.len() > 40 {
            continue;
        }
        // 错误码：纯数字 5-6 位 或 含数字的 code
        if t.chars().all(|c| c.is_ascii_digit()) && t.len() >= 4 {
            push_kw(&mut kws, t);
            continue;
        }
        // 全大写常量（如 ERROR_CODE）
        if t.chars().all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit()) && t.contains('_') {
            push_kw(&mut kws, t);
            continue;
        }
        // CamelCase 异常/类型名（含大写字母且以大写开头）
        let starts_upper = t.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false);
        let has_lower = t.chars().any(|c| c.is_ascii_lowercase());
        if starts_upper && has_lower && t.chars().all(|c| c.is_alphanumeric()) {
            push_kw(&mut kws, t);
            continue;
        }
        // 已知高频工具链/模型词
        let lower = t.to_lowercase();
        if matches!(
            lower.as_str(),
            "hvigor" | "ohpm" | "arkts" | "stage" | "hap" | "hsp" | "hdc" | "bundle" | "ability" | "signing" | "preferences"
        ) {
            push_kw(&mut kws, &lower);
        }
    }
    kws.truncate(6);
    kws.join(",")
}

fn push_kw(list: &mut Vec<String>, kw: &str) {
    let k = kw.to_string();
    if !list.iter().any(|x| x.eq_ignore_ascii_case(&k)) {
        list.push(k);
    }
}

fn derive_title(error_text: &str) -> String {
    error_text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.chars().take(60).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_error_code_and_type() {
        let kws = extract_keywords("BUILD ERROR TypeError: Cannot read property in ArkTS (9568339)");
        assert!(kws.contains("9568339"), "kws={kws}");
        assert!(kws.contains("TypeError") || kws.contains("arkts"));
    }

    #[test]
    fn extracts_known_tooling() {
        let kws = extract_keywords("hvigor task failed because ohpm install missing");
        assert!(kws.contains("hvigor") && kws.contains("ohpm"));
    }

    #[test]
    fn no_keywords_for_plain_text() {
        assert!(extract_keywords("the quick brown fox jumps over").is_empty());
    }
}
