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

    let content = skill
        .directory
        .as_deref()
        .and_then(read_skill_md)
        .ok_or_else(|| format!("Skill「{}」缺少可读的 SKILL.md，请重新导入", skill.name))?;
    let manifest = crate::services::skill_manifest::parse_and_validate(&content)?;
    if manifest.compatibility_status == "incompatible" {
        return Err(format!(
            "Skill「{}」声明与当前 HarmonyAgent {} 不兼容，已拒绝执行",
            skill.name,
            env!("CARGO_PKG_VERSION")
        ));
    }
    if skill.content_hash.as_deref().is_some_and(|hash| hash != manifest.content_hash) {
        crate::services::extension_governance::mark_drifted(
            &conn, "skill", &skill.id, &manifest.content_hash,
        );
        return Err(format!(
            "Skill「{}」的 SKILL.md 在导入后发生变化，内容哈希不匹配；请审核来源并重新导入",
            skill.name
        ));
    }

    crate::services::extension_governance::before_call(&conn, "skill", &skill.id)?;

    // 只有通过版本/哈希复验后才记录调用。
    let _ = crate::db::queries::record_skill_usage(
        &conn,
        &skill.id,
        &skill.name,
        conversation_id,
        project_id,
    );

    let permissions = if manifest.permissions.is_empty() {
        "未声明额外权限".to_string()
    } else {
        manifest.permissions.join(", ")
    };
    let compatibility_note = if manifest.schema == 0 {
        "legacy_unverified（兼容旧格式；权限范围未由清单证明）"
    } else {
        "compatible"
    };
    // 返回技能指令：描述 + 已复验的 SKILL.md 全文（截断护栏，模型据此执行）
    let mut out = format!(
        "已记录 Skill「{}」调用。版本={}，清单={}，兼容状态={}，声明权限=[{}]。\n技能声明不能扩大工具权限；所有实际调用仍受当前项目、阶段和审批护栏约束。请严格按以下指令完成任务：\n{}",
        skill.name,
        manifest.version,
        manifest.schema,
        compatibility_note,
        permissions,
        skill.description.as_deref().unwrap_or("")
    );
    let content: String = content.chars().take(6000).collect();
    out.push_str(&format!("\n\n=== 技能指令（SKILL.md） ===\n{content}"));
    crate::services::extension_governance::record_result(
        &conn, "skill", &skill.id, &Ok(out.clone()),
    );
    Ok(out)
}
