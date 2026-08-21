//! Versioned, project-scoped workflow template lifecycle (EC08).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const WORKFLOW_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowTemplate {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub harmony_agent_compat: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default = "enabled_default")]
    pub enabled: bool,
    pub steps: Vec<WorkflowStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStep {
    pub id: String,
    pub tool: String,
    #[serde(default)]
    pub args: Value,
    pub acceptance: String,
}

fn enabled_default() -> bool {
    true
}

pub fn handle(
    args: &Value,
    roots: &[String],
    db: &crate::db::DbState,
    project_id: &str,
    run_id: &str,
    conversation_id: &str,
) -> Result<String, String> {
    let action = args.get("action").and_then(Value::as_str).unwrap_or("list");
    let root = roots
        .first()
        .map(PathBuf::from)
        .ok_or("workflow_template 需要绑定项目目录")?;
    let store = root.join(".deveco-agent").join("workflow-templates");
    match action {
        "list" => list(&store),
        "validate" => {
            let template = template_arg(args)?;
            validate(&template)?;
            Ok(render(&template, "校验通过"))
        }
        "import" => {
            let template = template_arg(args)?;
            validate(&template)?;
            let path = template_path(&store, &template.id)?;
            if path.exists() {
                return Err(format!(
                    "工作流模板 {} 已存在；请使用 action=upgrade",
                    template.id
                ));
            }
            let payload = serde_json::to_vec(&template).map_err(|error| error.to_string())?;
            let attestation = attestation_arg(args)?;
            {
                let conn = db.0.lock().map_err(|error| error.to_string())?;
                crate::services::extension_governance::register(
                    &conn, "workflow", &template.id, nonempty(project_id), &payload,
                    attestation.as_ref(),
                )?;
                audit(&conn, run_id, conversation_id, "workflow.import", &template.id, "accepted");
            }
            write_template(&path, &template)?;
            Ok(render(&template, "已导入"))
        }
        "enable" | "disable" => {
            let id = id_arg(args)?;
            {
                let conn = db.0.lock().map_err(|error| error.to_string())?;
                crate::services::extension_governance::before_call(&conn, "workflow", id)?;
            }
            let path = template_path(&store, id)?;
            let mut template = read_template(&path)?;
            template.enabled = action == "enable";
            let result = write_template(&path, &template).map(|_| render(
                &template,
                if template.enabled {
                    "已启用"
                } else {
                    "已禁用"
                },
            ));
            if let Ok(conn) = db.0.lock() {
                crate::services::extension_governance::record_result(&conn, "workflow", id, &result);
                audit(&conn, run_id, conversation_id, &format!("workflow.{action}"), id, if result.is_ok() { "success" } else { "failure" });
            }
            result
        }
        "upgrade" => {
            let template = template_arg(args)?;
            validate(&template)?;
            {
                let conn = db.0.lock().map_err(|error| error.to_string())?;
                crate::services::extension_governance::before_call(&conn, "workflow", &template.id)?;
            }
            let path = template_path(&store, &template.id)?;
            let previous = read_template(&path)?;
            if crate::services::skill_manifest::compare_versions(
                &template.version,
                &previous.version,
            )? != std::cmp::Ordering::Greater
            {
                return Err(format!(
                    "升级版本必须高于当前版本 {}，收到 {}",
                    previous.version, template.version
                ));
            }
            let added: Vec<_> = template
                .permissions
                .iter()
                .filter(|permission| !previous.permissions.contains(permission))
                .cloned()
                .collect();
            if !added.is_empty()
                && !args
                    .get("allow_permission_escalation")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            {
                return Err(format!(
                    "升级新增权限 [{}]；审核后显式传 allow_permission_escalation=true",
                    added.join(", ")
                ));
            }
            let payload = serde_json::to_vec(&template).map_err(|error| error.to_string())?;
            let attestation = attestation_arg(args)?;
            crate::services::extension_governance::verify(&payload, attestation.as_ref())?;
            archive(&store, &previous)?;
            write_template(&path, &template)?;
            {
                let conn = db.0.lock().map_err(|error| error.to_string())?;
                crate::services::extension_governance::register(
                    &conn, "workflow", &template.id, nonempty(project_id), &payload,
                    attestation.as_ref(),
                )?;
                audit(&conn, run_id, conversation_id, "workflow.upgrade", &template.id, "success");
            }
            Ok(format!(
                "{}\n上一版本 {} 已归档，可人工回滚。",
                render(&template, "已升级"),
                previous.version
            ))
        }
        other => Err(format!(
            "未知 action={other}；支持 list|validate|import|enable|disable|upgrade"
        )),
    }
}

fn attestation_arg(args: &Value) -> Result<Option<crate::services::extension_governance::ExtensionAttestation>, String> {
    args.get("attestation").map(|value| serde_json::from_value(value.clone())
        .map_err(|error| format!("扩展签名格式错误：{error}"))).transpose()
}

fn nonempty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn audit(conn: &rusqlite::Connection, run_id: &str, conversation_id: &str, action: &str, id: &str, outcome: &str) {
    let _ = crate::agent::enterprise::audit(
        conn, nonempty(run_id), nonempty(conversation_id), "agent", action,
        &format!("workflow:{id}"), outcome, &serde_json::json!({}),
    );
}

fn template_arg(args: &Value) -> Result<WorkflowTemplate, String> {
    let value = args.get("template").ok_or("缺少 template 对象")?;
    serde_json::from_value(value.clone()).map_err(|error| format!("工作流模板格式错误：{error}"))
}

fn id_arg(args: &Value) -> Result<&str, String> {
    args.get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or("缺少 id".into())
}

pub fn validate(template: &WorkflowTemplate) -> Result<(), String> {
    if template.schema != WORKFLOW_SCHEMA_VERSION {
        return Err(format!(
            "不支持的工作流 schema {}；当前支持 {}",
            template.schema, WORKFLOW_SCHEMA_VERSION
        ));
    }
    validate_id(&template.id)?;
    if template.name.trim().is_empty() {
        return Err("工作流 name 不能为空".into());
    }
    if !crate::services::skill_manifest::validate_version(&template.version) {
        return Err(format!(
            "工作流 version 不是合法 SemVer：{}",
            template.version
        ));
    }
    if !crate::services::skill_manifest::agent_requirement_matches(
        &template.harmony_agent_compat,
        env!("CARGO_PKG_VERSION"),
    )? {
        return Err(format!(
            "工作流要求 HarmonyAgent {}，当前版本 {} 不兼容",
            template.harmony_agent_compat,
            env!("CARGO_PKG_VERSION")
        ));
    }
    for permission in &template.permissions {
        if !crate::services::skill_manifest::KNOWN_PERMISSIONS.contains(&permission.as_str()) {
            return Err(format!("未知工作流权限：{permission}"));
        }
    }
    if template.steps.is_empty() || template.steps.len() > 64 {
        return Err("工作流 steps 必须包含 1-64 项".into());
    }
    let mut ids = std::collections::HashSet::new();
    for step in &template.steps {
        validate_id(&step.id)?;
        if !ids.insert(&step.id) {
            return Err(format!("工作流 step id 重复：{}", step.id));
        }
        if step.tool == "workflow_template"
            || !crate::agent::tools::TOOL_SPECS
                .iter()
                .any(|spec| spec.name == step.tool)
        {
            return Err(format!("step {} 引用未知或递归工具 {}", step.id, step.tool));
        }
        if !step.args.is_object() {
            return Err(format!("step {} 的 args 必须是对象", step.id));
        }
        if step.acceptance.trim().is_empty() {
            return Err(format!("step {} 缺少 acceptance", step.id));
        }
        let required = required_permission(&step.tool);
        if !template
            .permissions
            .iter()
            .any(|permission| permission == required)
        {
            return Err(format!(
                "step {} 使用 {}，清单缺少权限 {}",
                step.id, step.tool, required
            ));
        }
    }
    Ok(())
}

fn required_permission(tool: &str) -> &'static str {
    if matches!(
        tool,
        "ota_pack" | "sign_hap" | "certificate_import" | "app_market_publish"
    ) {
        "release.publish"
    } else if tool == "secret_get" {
        "secrets.read"
    } else if tool == "run_command" {
        "process.exec"
    } else if matches!(tool, "web_search" | "web_fetch" | "ohpm_search") {
        "network.read"
    } else if matches!(tool, "http_request" | "api_test" | "api_health") {
        "network.write"
    } else if tool.contains("device")
        || matches!(
            tool,
            "deploy" | "deploy_all" | "start_ability" | "read_logcat" | "take_screenshot"
        )
    {
        if crate::agent::tools::contracts::contract(tool).effect
            == crate::agent::tools::contracts::EffectKind::Read
        {
            "device.read"
        } else {
            "device.write"
        }
    } else if crate::agent::tools::contracts::contract(tool).effect
        == crate::agent::tools::contracts::EffectKind::Read
    {
        "project.read"
    } else {
        "project.write"
    }
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_'))
    {
        return Err(format!(
            "id 非法：{id}（仅小写字母、数字、-、_，长度 1-64）"
        ));
    }
    Ok(())
}

fn template_path(store: &Path, id: &str) -> Result<PathBuf, String> {
    validate_id(id)?;
    Ok(store.join(format!("{id}.json")))
}

fn write_template(path: &Path, template: &WorkflowTemplate) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建模板目录失败：{error}"))?;
    }
    let bytes = serde_json::to_vec_pretty(template).map_err(|error| error.to_string())?;
    std::fs::write(path, bytes)
        .map_err(|error| format!("写入工作流模板失败 {}：{error}", path.display()))
}

fn read_template(path: &Path) -> Result<WorkflowTemplate, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("读取工作流模板失败 {}：{error}", path.display()))?;
    let template: WorkflowTemplate = serde_json::from_str(&text)
        .map_err(|error| format!("已存模板格式错误 {}：{error}", path.display()))?;
    validate(&template)?;
    Ok(template)
}

fn archive(store: &Path, template: &WorkflowTemplate) -> Result<(), String> {
    let history = store
        .join("history")
        .join(&template.id)
        .join(format!("{}.json", template.version));
    if !history.exists() {
        write_template(&history, template)?;
    }
    Ok(())
}

fn list(store: &Path) -> Result<String, String> {
    if !store.is_dir() {
        return Ok("未导入工作流模板".into());
    }
    let mut templates = Vec::new();
    for entry in std::fs::read_dir(store)
        .map_err(|error| error.to_string())?
        .flatten()
    {
        if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        match read_template(&entry.path()) {
            Ok(template) => templates.push(template),
            Err(error) => templates.push(WorkflowTemplate {
                schema: 0,
                id: entry.file_name().to_string_lossy().into(),
                name: error,
                version: "invalid".into(),
                harmony_agent_compat: String::new(),
                permissions: Vec::new(),
                enabled: false,
                steps: Vec::new(),
            }),
        }
    }
    templates.sort_by(|left, right| left.id.cmp(&right.id));
    if templates.is_empty() {
        Ok("未导入工作流模板".into())
    } else {
        Ok(templates
            .iter()
            .map(|template| render(template, "已安装"))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn render(template: &WorkflowTemplate, state: &str) -> String {
    format!(
        "{state}：{} ({}) v{}，enabled={}，steps={}，permissions=[{}]",
        template.name,
        template.id,
        template.version,
        template.enabled,
        template.steps.len(),
        template.permissions.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn database() -> crate::db::DbState {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE skills(id TEXT,project_id TEXT,repo_host TEXT,repo_owner TEXT,repo_name TEXT,repo_branch TEXT,content_hash TEXT,installed_at INTEGER,updated_at INTEGER); CREATE TABLE mcp_servers(id TEXT,project_id TEXT,homepage TEXT,created_at INTEGER);").unwrap();
        conn.execute_batch(include_str!("../../migrations/072_extension_governance.sql")).unwrap();
        crate::db::DbState(Arc::new(Mutex::new(conn)))
    }

    fn template(version: &str, permissions: &[&str]) -> WorkflowTemplate {
        WorkflowTemplate {
            schema: 1,
            id: "build-check".into(),
            name: "Build check".into(),
            version: version.into(),
            harmony_agent_compat: ">=2.0.0,<3.0.0".into(),
            permissions: permissions.iter().map(|value| (*value).into()).collect(),
            enabled: true,
            steps: vec![WorkflowStep {
                id: "inspect".into(),
                tool: "read_file".into(),
                args: serde_json::json!({"path":"README.md"}),
                acceptance: "文件可读".into(),
            }],
        }
    }

    #[test]
    fn validates_permissions_tools_and_compatibility() {
        validate(&template("1.0.0", &["project.read"])).unwrap();
        let mut missing = template("1.0.0", &[]);
        assert!(validate(&missing).unwrap_err().contains("project.read"));
        missing.permissions.push("project.read".into());
        missing.steps[0].tool = "workflow_template".into();
        assert!(validate(&missing).unwrap_err().contains("递归工具"));
    }

    #[test]
    fn lifecycle_import_toggle_upgrade_and_permission_diff() {
        let root = std::env::temp_dir().join(format!("workflow-template-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let roots = [root.to_string_lossy().to_string()];
        let db = database();
        let initial = template("1.0.0", &["project.read"]);
        handle(
            &serde_json::json!({"action":"import","template":initial}),
            &roots,
            &db, "p", "", "",
        )
        .unwrap();
        handle(
            &serde_json::json!({"action":"disable","id":"build-check"}),
            &roots,
            &db, "p", "", "",
        )
        .unwrap();
        let upgraded = template("1.1.0", &["project.read", "project.write"]);
        let err = handle(
            &serde_json::json!({"action":"upgrade","template":upgraded}),
            &roots,
            &db, "p", "", "",
        )
        .unwrap_err();
        assert!(err.contains("allow_permission_escalation"));
        handle(&serde_json::json!({"action":"upgrade","template":upgraded,"allow_permission_escalation":true}), &roots, &db, "p", "", "").unwrap();
        assert!(root
            .join(".deveco-agent/workflow-templates/history/build-check/1.0.0.json")
            .is_file());
        let _ = std::fs::remove_dir_all(root);
    }
}
