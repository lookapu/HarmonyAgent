//! Skill 调用工具：use_skill —— 模型决定按某技能规范执行时显式调用，
//! 后端记录调用（skill_usage 表，供技能管理页/统计页展示）并返回技能完整指令。

use serde_json::Value;

use crate::db::DbState;

use super::protocol::read_skill_md;

/// use_skill：声明正在使用某技能并记录一次调用，返回该技能的描述与完整指令。
/// 参数：{"name":"<技能名>"}（与技能管理页展示的名称一致；同名时项目级优先）。
pub async fn use_skill(
    args: &Value,
    conversation_id: &str,
    project_id: &str,
    db: &DbState,
) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "参数缺失：请提供 {\"name\":\"<技能名>\"}".to_string())?;

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let pid = if project_id.is_empty() {
        None
    } else {
        Some(project_id)
    };
    let skill = crate::db::queries::find_enabled_skill_by_name(&conn, name, pid)
        .map_err(|e| e.to_string())?;
    let Some(skill) = skill else {
        return Err(format!(
            "技能「{name}」未找到或未启用。可在「Skill 管理」页查看已安装技能并确认其已启用。"
        ));
    };

    // 落库调用记录（单条失败不阻断技能执行）
    let _ = crate::db::queries::record_skill_usage(
        &conn,
        &skill.id,
        &skill.name,
        conversation_id,
        project_id,
    );

    // 返回技能指令：描述 + SKILL.md 全文（截断护栏，模型据此执行）
    let mut out = format!(
        "已记录 Skill「{}」调用。请严格按以下指令完成任务：\n{}",
        skill.name,
        skill.description.as_deref().unwrap_or("")
    );
    if let Some(dir) = &skill.directory {
        if let Some(content) = read_skill_md(dir) {
            let content: String = content.chars().take(6000).collect();
            out.push_str(&format!("\n\n=== 技能指令（SKILL.md） ===\n{content}"));
        }
    }
    Ok(out)
}
