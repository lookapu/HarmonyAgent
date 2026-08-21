//! Versioned, auditable and reversible team sharing (EC11).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::models::ProjectMemory;

pub const TEAM_SHARE_SCHEMA: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TeamSharePackage {
    pub schema: u32,
    pub package_id: String,
    pub name: String,
    pub version: String,
    pub source: TeamShareSource,
    #[serde(default)]
    pub memories: Vec<SharedMemory>,
    #[serde(default)]
    pub conventions: Vec<SharedMemory>,
    #[serde(default)]
    pub eval_sets: Vec<SharedEvalSet>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TeamShareSource {
    pub uri: String,
    pub revision: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SharedMemory {
    pub key: String,
    pub category: String,
    pub title: String,
    pub content: String,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    #[serde(default)]
    pub invalidation_condition: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SharedEvalSet {
    pub key: String,
    pub name: String,
    pub cases: Vec<SharedEvalCase>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SharedEvalCase {
    pub scenario_id: String,
    pub expected: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SharePreviewItem {
    pub kind: String,
    pub key: String,
    pub action: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SharePreview {
    pub package_id: String,
    pub version: String,
    pub digest: String,
    pub inserts: usize,
    pub updates: usize,
    pub conflicts: usize,
    pub unchanged: usize,
    pub items: Vec<SharePreviewItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ShareImportRecord {
    pub batch_id: String,
    pub project_id: String,
    pub package_id: String,
    pub package_name: String,
    pub package_version: String,
    pub source_uri: String,
    pub source_revision: String,
    pub package_digest: String,
    pub state: String,
    pub imported_at: i64,
    pub reverted_at: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ShareChangeRecord {
    pub change_id: String,
    pub batch_id: String,
    pub item_kind: String,
    pub stable_key: String,
    pub local_id: Option<String>,
    pub action: String,
    pub before_json: Option<String>,
    pub after_digest: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TeamEvalRun {
    pub set_id: String,
    pub name: String,
    pub passed: bool,
    pub total_cases: usize,
    pub passed_cases: usize,
    pub results: Vec<crate::agent::evals::EvalCaseResult>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TeamEvalSetRecord {
    pub id: String,
    pub stable_key: String,
    pub name: String,
    pub version: String,
    pub case_count: usize,
    pub enabled: bool,
    pub source_ref: String,
    pub updated_at: i64,
}

fn default_confidence() -> f64 {
    0.8
}

pub fn parse_and_validate(value: &serde_json::Value) -> Result<TeamSharePackage, String> {
    let package: TeamSharePackage = serde_json::from_value(value.clone())
        .map_err(|error| format!("团队共享包格式错误：{error}"))?;
    validate(&package)?;
    Ok(package)
}

pub fn validate(package: &TeamSharePackage) -> Result<(), String> {
    if package.schema != TEAM_SHARE_SCHEMA {
        return Err(format!("不支持的团队共享 schema {}", package.schema));
    }
    validate_key(&package.package_id)?;
    if package.name.trim().is_empty() || package.name.chars().count() > 120 {
        return Err("共享包 name 必须为 1-120 个字符".into());
    }
    if !crate::services::skill_manifest::validate_version(&package.version) {
        return Err(format!(
            "共享包 version 不是合法 SemVer：{}",
            package.version
        ));
    }
    validate_source(&package.source)?;
    if package.memories.len() + package.conventions.len() > 500 || package.eval_sets.len() > 100 {
        return Err("共享包超过上限：记忆/约定合计 500，评测集 100".into());
    }
    let mut keys = std::collections::HashSet::new();
    for (kind, memory) in package
        .memories
        .iter()
        .map(|item| ("memory", item))
        .chain(package.conventions.iter().map(|item| ("convention", item)))
    {
        validate_memory(memory, kind == "convention")?;
        if !keys.insert(format!("{kind}:{}", memory.key)) {
            return Err(format!("共享项 key 重复：{kind}:{}", memory.key));
        }
    }
    let known = crate::agent::evals::scenarios()
        .into_iter()
        .map(|scenario| (scenario.id, scenario.expected))
        .collect::<std::collections::HashMap<_, _>>();
    for set in &package.eval_sets {
        validate_key(&set.key)?;
        if set.name.trim().is_empty()
            || set.name.chars().count() > 120
            || set.cases.is_empty()
            || set.cases.len() > 100
        {
            return Err(format!("评测集 {} 的 name/cases 非法", set.key));
        }
        if !keys.insert(format!("eval_set:{}", set.key)) {
            return Err(format!("共享项 key 重复：eval_set:{}", set.key));
        }
        let mut scenario_ids = std::collections::HashSet::new();
        for case in &set.cases {
            let expected = known
                .get(&case.scenario_id)
                .ok_or_else(|| format!("评测集 {} 引用未注册场景 {}", set.key, case.scenario_id))?;
            if expected != &case.expected {
                return Err(format!(
                    "场景 {} 的 expected 与本机注册契约不一致",
                    case.scenario_id
                ));
            }
            if !scenario_ids.insert(&case.scenario_id) {
                return Err(format!("评测集 {} 重复场景 {}", set.key, case.scenario_id));
            }
        }
    }
    Ok(())
}

pub fn preview(
    conn: &Connection,
    project_id: &str,
    package: &TeamSharePackage,
) -> Result<SharePreview, String> {
    validate(package)?;
    ensure_project(conn, project_id)?;
    let mut items = Vec::new();
    for (kind, memory) in package
        .memories
        .iter()
        .map(|item| ("memory", item))
        .chain(package.conventions.iter().map(|item| ("convention", item)))
    {
        items.push(preview_memory(conn, project_id, package, kind, memory)?);
    }
    for set in &package.eval_sets {
        items.push(preview_eval(conn, project_id, package, set)?);
    }
    Ok(summarize(package, items)?)
}

pub fn apply(
    conn: &mut Connection,
    project_id: &str,
    package: &TeamSharePackage,
) -> Result<ShareImportRecord, String> {
    let preview = preview(conn, project_id, package)?;
    let duplicate: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM team_share_imports WHERE project_id=?1 AND package_id=?2 AND package_version=?3 AND package_digest=?4 AND state='applied')",
        params![project_id,package.package_id,package.version,preview.digest], |row| row.get(0),
    ).map_err(|error| error.to_string())?;
    if duplicate {
        return Err("该共享包版本和摘要已经应用".into());
    }
    let latest: Option<(String, String)> = conn
        .query_row(
            "SELECT package_version,package_digest FROM team_share_imports WHERE project_id=?1 AND package_id=?2 AND source_uri=?3 AND state='applied' ORDER BY imported_at DESC,rowid DESC LIMIT 1",
            params![project_id,package.package_id,package.source.uri],
            |row| Ok((row.get(0)?,row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if let Some((version, digest)) = latest {
        if version == package.version && digest != preview.digest {
            return Err("同一共享包版本的内容摘要发生变化；请发布更高版本".into());
        }
        if crate::services::skill_manifest::compare_versions(&package.version, &version)?
            != std::cmp::Ordering::Greater
        {
            return Err(format!("共享包升级版本必须高于当前版本 {version}"));
        }
    }
    let batch_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let tx = conn.transaction().map_err(|error| error.to_string())?;
    tx.execute(
        "INSERT INTO team_share_imports(batch_id,project_id,package_id,package_name,package_version,source_uri,source_revision,package_digest,state,imported_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'applied',?9)",
        params![batch_id,project_id,package.package_id,package.name,package.version,package.source.uri,package.source.revision,preview.digest,now],
    ).map_err(|error| error.to_string())?;
    for (kind, memory) in package
        .memories
        .iter()
        .map(|item| ("memory", item))
        .chain(package.conventions.iter().map(|item| ("convention", item)))
    {
        apply_memory(&tx, project_id, package, &batch_id, kind, memory, now)?;
    }
    for set in &package.eval_sets {
        apply_eval(&tx, project_id, package, &batch_id, set, now)?;
    }
    crate::agent::enterprise::audit(
        &tx,
        None,
        None,
        "user",
        "team_share.apply",
        &batch_id,
        "success",
        &serde_json::json!({"project_id":project_id,"package_id":package.package_id,"version":package.version,"digest":preview.digest,"conflicts":preview.conflicts}),
    )?;
    tx.commit().map_err(|error| error.to_string())?;
    Ok(ShareImportRecord {
        batch_id,
        project_id: project_id.into(),
        package_id: package.package_id.clone(),
        package_name: package.name.clone(),
        package_version: package.version.clone(),
        source_uri: package.source.uri.clone(),
        source_revision: package.source.revision.clone(),
        package_digest: preview.digest,
        state: "applied".into(),
        imported_at: now,
        reverted_at: None,
    })
}

pub fn revert(conn: &mut Connection, project_id: &str, batch_id: &str) -> Result<usize, String> {
    let tx = conn.transaction().map_err(|error| error.to_string())?;
    let state: Option<String> = tx
        .query_row(
            "SELECT state FROM team_share_imports WHERE batch_id=?1 AND project_id=?2",
            params![batch_id, project_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    match state.as_deref() {
        Some("applied") => {}
        Some("reverted") => return Err("该共享导入已经撤销".into()),
        _ => return Err("共享导入批次不存在或不属于当前项目".into()),
    }
    let mut stmt = tx.prepare("SELECT item_kind,stable_key,local_id,action,before_json,after_digest FROM team_share_changes WHERE batch_id=?1 ORDER BY created_at DESC,change_id DESC")
        .map_err(|error| error.to_string())?;
    let changes = stmt
        .query_map([batch_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    drop(stmt);
    let mut reverted = 0;
    for (kind, stable_key, local_id, action, before, after_digest) in changes {
        let Some(local_id) = local_id else { continue };
        if kind == "eval_set" {
            reverted += revert_eval(
                &tx,
                &local_id,
                &stable_key,
                &action,
                before.as_deref(),
                &after_digest,
            )?;
        } else {
            reverted += revert_memory(
                &tx,
                &local_id,
                &stable_key,
                &kind,
                &action,
                before.as_deref(),
                &after_digest,
            )?;
        }
    }
    let now = chrono::Utc::now().timestamp();
    tx.execute(
        "UPDATE team_share_imports SET state='reverted',reverted_at=?2 WHERE batch_id=?1",
        params![batch_id, now],
    )
    .map_err(|error| error.to_string())?;
    crate::agent::enterprise::audit(
        &tx,
        None,
        None,
        "user",
        "team_share.revert",
        batch_id,
        "success",
        &serde_json::json!({"project_id":project_id,"reverted_items":reverted}),
    )?;
    tx.commit().map_err(|error| error.to_string())?;
    Ok(reverted)
}

pub fn list_imports(conn: &Connection, project_id: &str) -> Result<Vec<ShareImportRecord>, String> {
    let mut stmt = conn.prepare("SELECT batch_id,project_id,package_id,package_name,package_version,source_uri,source_revision,package_digest,state,imported_at,reverted_at FROM team_share_imports WHERE project_id=?1 ORDER BY imported_at DESC,rowid DESC")
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map([project_id], |row| {
            Ok(ShareImportRecord {
                batch_id: row.get(0)?,
                project_id: row.get(1)?,
                package_id: row.get(2)?,
                package_name: row.get(3)?,
                package_version: row.get(4)?,
                source_uri: row.get(5)?,
                source_revision: row.get(6)?,
                package_digest: row.get(7)?,
                state: row.get(8)?,
                imported_at: row.get(9)?,
                reverted_at: row.get(10)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

pub fn list_changes(
    conn: &Connection,
    project_id: &str,
    batch_id: &str,
) -> Result<Vec<ShareChangeRecord>, String> {
    let belongs: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM team_share_imports WHERE batch_id=?1 AND project_id=?2)",
            params![batch_id, project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !belongs {
        return Err("共享导入批次不存在或不属于当前项目".into());
    }
    let mut stmt = conn.prepare("SELECT change_id,batch_id,item_kind,stable_key,local_id,action,before_json,after_digest,created_at FROM team_share_changes WHERE batch_id=?1 ORDER BY created_at,change_id").map_err(|error|error.to_string())?;
    let rows = stmt
        .query_map([batch_id], |row| {
            Ok(ShareChangeRecord {
                change_id: row.get(0)?,
                batch_id: row.get(1)?,
                item_kind: row.get(2)?,
                stable_key: row.get(3)?,
                local_id: row.get(4)?,
                action: row.get(5)?,
                before_json: row.get(6)?,
                after_digest: row.get(7)?,
                created_at: row.get(8)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

pub fn export(
    conn: &Connection,
    project_id: &str,
    package_id: &str,
    name: &str,
    version: &str,
    source: TeamShareSource,
) -> Result<TeamSharePackage, String> {
    let memories =
        crate::db::queries::list_memories(conn, project_id).map_err(|error| error.to_string())?;
    let mut package = TeamSharePackage {
        schema: TEAM_SHARE_SCHEMA,
        package_id: package_id.into(),
        name: name.into(),
        version: version.into(),
        source,
        memories: Vec::new(),
        conventions: Vec::new(),
        eval_sets: Vec::new(),
    };
    for memory in memories
        .into_iter()
        .filter(|item| item.enabled && item.confirmed && item.invalidated_at.is_none())
    {
        let item = SharedMemory {
            key: format!("memory-{}", memory.id),
            category: memory.category.clone(),
            title: memory.title,
            content: memory.content,
            confidence: memory.confidence,
            invalidation_condition: memory.invalidation_condition,
        };
        if memory.category == "architecture" {
            package.conventions.push(item);
        } else {
            package.memories.push(item);
        }
    }
    let mut stmt = conn.prepare("SELECT stable_key,name,cases_json FROM team_eval_sets WHERE project_id=?1 AND enabled=1 ORDER BY stable_key").map_err(|error| error.to_string())?;
    package.eval_sets = stmt
        .query_map([project_id], |row| {
            Ok(SharedEvalSet {
                key: row.get(0)?,
                name: row.get(1)?,
                cases: serde_json::from_str(&row.get::<_, String>(2)?).unwrap_or_default(),
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    validate(&package)?;
    Ok(package)
}

pub fn run_eval_set(
    conn: &Connection,
    project_id: &str,
    set_id: &str,
) -> Result<TeamEvalRun, String> {
    let (name,cases): (String,String) = conn.query_row("SELECT name,cases_json FROM team_eval_sets WHERE id=?1 AND project_id=?2 AND enabled=1",params![set_id,project_id],|row| Ok((row.get(0)?,row.get(1)?))).map_err(|_| "团队评测集不存在、未启用或不属于当前项目".to_string())?;
    let cases: Vec<SharedEvalCase> =
        serde_json::from_str(&cases).map_err(|error| error.to_string())?;
    let results = cases
        .into_iter()
        .map(|case| {
            crate::agent::evals::evaluate_registered_scenario(&case.scenario_id, &case.expected)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let passed_cases = results.iter().filter(|result| result.passed).count();
    Ok(TeamEvalRun {
        set_id: set_id.into(),
        name,
        passed: passed_cases == results.len(),
        total_cases: results.len(),
        passed_cases,
        results,
    })
}

pub fn list_eval_sets(
    conn: &Connection,
    project_id: &str,
) -> Result<Vec<TeamEvalSetRecord>, String> {
    let mut stmt = conn.prepare("SELECT id,stable_key,name,version,cases_json,enabled,source_ref,updated_at FROM team_eval_sets WHERE project_id=?1 ORDER BY updated_at DESC").map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map([project_id], |row| {
            let cases = row.get::<_, String>(4)?;
            Ok(TeamEvalSetRecord {
                id: row.get(0)?,
                stable_key: row.get(1)?,
                name: row.get(2)?,
                version: row.get(3)?,
                case_count: serde_json::from_str::<Vec<SharedEvalCase>>(&cases)
                    .map(|items| items.len())
                    .unwrap_or(0),
                enabled: row.get(5)?,
                source_ref: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

pub fn handle_tool(
    args: &serde_json::Value,
    project_id: &str,
    db: &crate::db::DbState,
) -> Result<String, String> {
    if project_id.is_empty() {
        return Err("team_share 需要绑定项目".into());
    }
    let action = args
        .get("action")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("list");
    match action {
        "validate" | "preview" => {
            let package = parse_and_validate(args.get("package").ok_or("缺少 package")?)?;
            if action == "validate" {
                return Ok(format!(
                    "团队共享包 {} v{} 校验通过",
                    package.name, package.version
                ));
            }
            let conn = db.0.lock().map_err(|error| error.to_string())?;
            serde_json::to_string_pretty(&preview(&conn, project_id, &package)?)
                .map_err(|error| error.to_string())
        }
        "apply" => {
            let package = parse_and_validate(args.get("package").ok_or("缺少 package")?)?;
            let mut conn = db.0.lock().map_err(|error| error.to_string())?;
            serde_json::to_string_pretty(&apply(&mut conn, project_id, &package)?)
                .map_err(|error| error.to_string())
        }
        "revert" => {
            let batch = text_arg(args, "batch_id")?;
            let mut conn = db.0.lock().map_err(|error| error.to_string())?;
            Ok(format!(
                "已撤销团队共享批次 {batch}，恢复 {} 项；用户已编辑的项保持不变",
                revert(&mut conn, project_id, batch)?
            ))
        }
        "list" => {
            let conn = db.0.lock().map_err(|error| error.to_string())?;
            serde_json::to_string_pretty(&list_imports(&conn, project_id)?)
                .map_err(|error| error.to_string())
        }
        "export" => {
            let source = TeamShareSource {
                uri: text_arg(args, "source_uri")?.into(),
                revision: text_arg(args, "source_revision")?.into(),
            };
            let conn = db.0.lock().map_err(|error| error.to_string())?;
            let package = export(
                &conn,
                project_id,
                text_arg(args, "package_id")?,
                text_arg(args, "name")?,
                text_arg(args, "version")?,
                source,
            )?;
            serde_json::to_string_pretty(&package).map_err(|error| error.to_string())
        }
        "run_eval" => {
            let conn = db.0.lock().map_err(|error| error.to_string())?;
            serde_json::to_string_pretty(&run_eval_set(
                &conn,
                project_id,
                text_arg(args, "set_id")?,
            )?)
            .map_err(|error| error.to_string())
        }
        other => Err(format!(
            "未知 action={other}；支持 validate|preview|apply|revert|list|export|run_eval"
        )),
    }
}

fn text_arg<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("缺少 {key}"))
}

fn preview_memory(
    conn: &Connection,
    project_id: &str,
    package: &TeamSharePackage,
    kind: &str,
    item: &SharedMemory,
) -> Result<SharePreviewItem, String> {
    let source_ref = source_ref(package, kind, &item.key);
    let shared:Option<(String,String,String,f64,String)>=conn.query_row("SELECT category,title,content,confidence,invalidation_condition FROM project_memories WHERE project_id=?1 AND source_kind='team_share' AND source_ref=?2",params![project_id,source_ref],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(|e|e.to_string())?;
    if let Some((category, title, content, confidence, invalidation)) = shared {
        let unchanged = category == effective_category(kind, item)
            && title == item.title
            && content == item.content
            && (confidence - item.confidence).abs() < f64::EPSILON
            && invalidation == item.invalidation_condition;
        return Ok(preview_item(
            kind,
            &item.key,
            if unchanged { "unchanged" } else { "update" },
            "同一来源的共享项",
        ));
    }
    let local:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM project_memories WHERE project_id=?1 AND source_kind!='team_share' AND category=?2 AND (title=?3 OR content=?4))",params![project_id,effective_category(kind,item),item.title,item.content],|r|r.get(0)).map_err(|e|e.to_string())?;
    Ok(preview_item(
        kind,
        &item.key,
        if local { "conflict" } else { "insert" },
        if local {
            "与本地事实同名或同内容；将以禁用、未确认状态并存"
        } else {
            "新增共享项"
        },
    ))
}

fn preview_eval(
    conn: &Connection,
    project_id: &str,
    package: &TeamSharePackage,
    set: &SharedEvalSet,
) -> Result<SharePreviewItem, String> {
    let source_ref = source_ref(package, "eval_set", &set.key);
    let current:Option<(String,String)>=conn.query_row("SELECT name,cases_json FROM team_eval_sets WHERE project_id=?1 AND source_ref=?2 AND stable_key=?3",params![project_id,source_ref,set.key],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
    let encoded = serde_json::to_string(&set.cases).map_err(|e| e.to_string())?;
    Ok(preview_item(
        "eval_set",
        &set.key,
        match current.as_ref() {
            Some((name, value)) if name == &set.name && value == &encoded => "unchanged",
            Some(_) => "update",
            None => "insert",
        },
        "只组合已注册评测场景",
    ))
}

fn apply_memory(
    tx: &Connection,
    project_id: &str,
    package: &TeamSharePackage,
    batch: &str,
    kind: &str,
    item: &SharedMemory,
    now: i64,
) -> Result<(), String> {
    let preview = preview_memory(tx, project_id, package, kind, item)?;
    let source_ref = source_ref(package, kind, &item.key);
    let existing: Option<ProjectMemory> = crate::db::queries::list_memories(tx, project_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|m| m.source_kind == "team_share" && m.source_ref == source_ref);
    let (local_id, action, before) = if preview.action == "unchanged" {
        (existing.map(|m| m.id), "unchanged", None)
    } else if let Some(old) = existing {
        let before = serde_json::to_string(&old).map_err(|e| e.to_string())?;
        tx.execute("UPDATE project_memories SET category=?2,title=?3,content=?4,confidence=?5,version=version+1,confirmed=1,enabled=1,invalidation_condition=?6,invalidated_at=NULL,invalidation_reason=NULL,updated_at=?7 WHERE id=?1",params![old.id,effective_category(kind,item),item.title,item.content,item.confidence,item.invalidation_condition,now]).map_err(|e|e.to_string())?;
        (Some(old.id), "updated", Some(before))
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        let conflict = preview.action == "conflict";
        let memory = ProjectMemory {
            id: id.clone(),
            project_id: project_id.into(),
            category: effective_category(kind, item).into(),
            title: item.title.clone(),
            content: item.content.clone(),
            enabled: !conflict,
            source_kind: "team_share".into(),
            source_ref,
            scope: "project".into(),
            confidence: item.confidence,
            version: 1,
            confirmed: !conflict,
            pinned: false,
            invalidation_condition: item.invalidation_condition.clone(),
            invalidated_at: None,
            invalidation_reason: None,
            created_at: now,
            updated_at: now,
        };
        crate::db::queries::insert_memory(tx, &memory).map_err(|e| e.to_string())?;
        (
            Some(id),
            if conflict {
                "staged_conflict"
            } else {
                "inserted"
            },
            None,
        )
    };
    record_change(
        tx,
        batch,
        kind,
        &item.key,
        local_id.as_deref(),
        action,
        before.as_deref(),
        &memory_digest(kind, item)?,
        now,
    )
}

fn apply_eval(
    tx: &Connection,
    project_id: &str,
    package: &TeamSharePackage,
    batch: &str,
    set: &SharedEvalSet,
    now: i64,
) -> Result<(), String> {
    let source_ref = source_ref(package, "eval_set", &set.key);
    let after = digest(set)?;
    let old:Option<(String,String,String,String,i64)>=tx.query_row("SELECT id,name,version,cases_json,enabled FROM team_eval_sets WHERE project_id=?1 AND source_ref=?2 AND stable_key=?3",params![project_id,source_ref,set.key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(|e|e.to_string())?;
    let encoded = serde_json::to_string(&set.cases).map_err(|e| e.to_string())?;
    let (id, action, before) = if let Some((id, name, version, cases, enabled)) = old {
        if cases == encoded && name == set.name {
            (id, "unchanged", None)
        } else {
            let before=serde_json::json!({"name":name,"version":version,"cases_json":cases,"enabled":enabled}).to_string();
            tx.execute("UPDATE team_eval_sets SET name=?2,version=?3,cases_json=?4,enabled=1,updated_at=?5 WHERE id=?1",params![id,set.name,package.version,encoded,now]).map_err(|e|e.to_string())?;
            (id, "updated", Some(before))
        }
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute("INSERT INTO team_eval_sets(id,project_id,stable_key,name,version,cases_json,enabled,source_kind,source_ref,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,1,'team_share',?7,?8,?8)",params![id,project_id,set.key,set.name,package.version,encoded,source_ref,now]).map_err(|e|e.to_string())?;
        (id, "inserted", None)
    };
    record_change(
        tx,
        batch,
        "eval_set",
        &set.key,
        Some(&id),
        action,
        before.as_deref(),
        &after,
        now,
    )
}

fn revert_memory(
    tx: &Connection,
    id: &str,
    stable_key: &str,
    kind: &str,
    action: &str,
    before: Option<&str>,
    after: &str,
) -> Result<usize, String> {
    let current = crate::db::queries::list_memories_for_revert(tx, id)?;
    let Some(current) = current else { return Ok(0) };
    if current.source_kind != "team_share" {
        return Ok(0);
    };
    let item = SharedMemory {
        key: stable_key.into(),
        category: current.category.clone(),
        title: current.title.clone(),
        content: current.content.clone(),
        confidence: current.confidence,
        invalidation_condition: current.invalidation_condition.clone(),
    };
    if memory_digest(kind, &item)? != after {
        return Ok(0);
    };
    if matches!(action, "inserted" | "staged_conflict") {
        tx.execute("DELETE FROM project_memories WHERE id=?1", [id])
            .map_err(|e| e.to_string())?;
        return Ok(1);
    }
    if action == "updated" {
        let old: ProjectMemory = serde_json::from_str(before.ok_or("共享变更缺少恢复快照")?)
            .map_err(|e| e.to_string())?;
        tx.execute("UPDATE project_memories SET category=?2,title=?3,content=?4,enabled=?5,source_kind=?6,source_ref=?7,scope=?8,confidence=?9,version=?10,confirmed=?11,pinned=?12,invalidation_condition=?13,invalidated_at=?14,invalidation_reason=?15,updated_at=?16 WHERE id=?1",params![id,old.category,old.title,old.content,old.enabled as i64,old.source_kind,old.source_ref,old.scope,old.confidence,old.version,old.confirmed as i64,old.pinned as i64,old.invalidation_condition,old.invalidated_at,old.invalidation_reason,old.updated_at]).map_err(|e|e.to_string())?;
        return Ok(1);
    }
    Ok(0)
}

fn revert_eval(
    tx: &Connection,
    id: &str,
    stable_key: &str,
    action: &str,
    before: Option<&str>,
    after: &str,
) -> Result<usize, String> {
    let current:Option<(String,String,String)>=tx.query_row("SELECT name,cases_json,stable_key FROM team_eval_sets WHERE id=?1 AND source_kind='team_share'",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())?;
    let Some((name, cases, key)) = current else {
        return Ok(0);
    };
    if key != stable_key {
        return Ok(0);
    }
    let item = SharedEvalSet {
        key,
        name,
        cases: serde_json::from_str(&cases).map_err(|e| e.to_string())?,
    };
    if digest(&item)? != after {
        return Ok(0);
    };
    if action == "inserted" {
        tx.execute("DELETE FROM team_eval_sets WHERE id=?1", [id])
            .map_err(|e| e.to_string())?;
        return Ok(1);
    }
    if action == "updated" {
        let old: serde_json::Value =
            serde_json::from_str(before.ok_or("共享评测变更缺少恢复快照")?)
                .map_err(|e| e.to_string())?;
        tx.execute("UPDATE team_eval_sets SET name=?2,version=?3,cases_json=?4,enabled=?5,updated_at=?6 WHERE id=?1",params![id,old["name"].as_str(),old["version"].as_str(),old["cases_json"].as_str(),old["enabled"].as_i64(),chrono::Utc::now().timestamp()]).map_err(|e|e.to_string())?;
        return Ok(1);
    }
    Ok(0)
}

fn record_change(
    conn: &Connection,
    batch: &str,
    kind: &str,
    key: &str,
    local_id: Option<&str>,
    action: &str,
    before: Option<&str>,
    after: &str,
    now: i64,
) -> Result<(), String> {
    conn.execute("INSERT INTO team_share_changes(change_id,batch_id,item_kind,stable_key,local_id,action,before_json,after_digest,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![uuid::Uuid::new_v4().to_string(),batch,kind,key,local_id,action,before,after,now]).map_err(|e|e.to_string())?;
    Ok(())
}
fn preview_item(kind: &str, key: &str, action: &str, reason: &str) -> SharePreviewItem {
    SharePreviewItem {
        kind: kind.into(),
        key: key.into(),
        action: action.into(),
        reason: reason.into(),
    }
}
fn summarize(
    package: &TeamSharePackage,
    items: Vec<SharePreviewItem>,
) -> Result<SharePreview, String> {
    let digest = digest(package)?;
    Ok(SharePreview {
        package_id: package.package_id.clone(),
        version: package.version.clone(),
        digest,
        inserts: items.iter().filter(|i| i.action == "insert").count(),
        updates: items.iter().filter(|i| i.action == "update").count(),
        conflicts: items.iter().filter(|i| i.action == "conflict").count(),
        unchanged: items.iter().filter(|i| i.action == "unchanged").count(),
        items,
    })
}
fn source_ref(package: &TeamSharePackage, kind: &str, key: &str) -> String {
    format!(
        "team:{}#{}:{kind}:{key}",
        package.source.uri, package.package_id
    )
}
fn effective_category<'a>(kind: &str, item: &'a SharedMemory) -> &'a str {
    if kind == "convention" {
        "architecture"
    } else {
        &item.category
    }
}
fn digest<T: Serialize>(value: &T) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}
fn memory_digest(kind: &str, item: &SharedMemory) -> Result<String, String> {
    let mut normalized = item.clone();
    normalized.category = effective_category(kind, item).into();
    digest(&normalized)
}
fn ensure_project(conn: &Connection, id: &str) -> Result<(), String> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
            [id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    exists.then_some(()).ok_or("目标项目不存在".into())
}
fn validate_key(key: &str) -> Result<(), String> {
    (!key.is_empty()
        && key.len() <= 96
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.')))
    .then_some(())
    .ok_or_else(|| format!("共享 key 非法：{key}"))
}
fn validate_source(source: &TeamShareSource) -> Result<(), String> {
    if source.uri.trim().is_empty()
        || source.uri.len() > 2048
        || source.revision.trim().is_empty()
        || source.revision.len() > 256
        || source
            .uri
            .chars()
            .chain(source.revision.chars())
            .any(char::is_control)
    {
        return Err("共享来源 URI/revision 非法".into());
    }
    let url = url::Url::parse(&source.uri).map_err(|_| "共享来源必须是绝对 URI")?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("共享来源 URI 不能包含凭据".into());
    }
    Ok(())
}
fn validate_memory(item: &SharedMemory, convention: bool) -> Result<(), String> {
    validate_key(&item.key)?;
    if item.title.trim().is_empty()
        || item.title.chars().count() > 120
        || item.content.trim().is_empty()
        || item.content.chars().count() > 8000
        || !item.confidence.is_finite()
        || !(0.0..=1.0).contains(&item.confidence)
        || item.invalidation_condition.chars().count() > 500
    {
        return Err(format!("共享记忆 {} 字段非法", item.key));
    }
    if !convention
        && !matches!(
            item.category.as_str(),
            "general"
                | "architecture"
                | "build_command"
                | "module_role"
                | "user_preference"
                | "code"
                | "build"
                | "deploy"
                | "decision"
                | "pitfall"
                | "path"
        )
    {
        return Err(format!("共享记忆 {} category 非法", item.key));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE projects(id TEXT PRIMARY KEY); INSERT INTO projects VALUES('p');").unwrap();
        conn.execute_batch(include_str!("../../migrations/008_project_memories.sql"))
            .unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/066_structured_project_memories.sql"
        ))
        .unwrap();
        conn.execute_batch("CREATE TABLE agent_audit_events(audit_id TEXT PRIMARY KEY,tenant_id TEXT,run_id TEXT,conversation_id TEXT,actor TEXT,action TEXT,resource TEXT,outcome TEXT,details_json TEXT,created_at INTEGER);").unwrap();
        let migration = include_str!("../../migrations/073_team_sharing.sql");
        conn.execute_batch(migration).unwrap();
        conn.execute_batch(migration).unwrap();
        conn
    }

    fn package(version: &str, content: &str) -> TeamSharePackage {
        let scenario = crate::agent::evals::scenarios().remove(0);
        TeamSharePackage {
            schema: 1,
            package_id: "mobile-team".into(),
            name: "Mobile team".into(),
            version: version.into(),
            source: TeamShareSource {
                uri: "https://example.com/team/context".into(),
                revision: format!("rev-{version}"),
            },
            memories: vec![SharedMemory {
                key: "build-command".into(),
                category: "build_command".into(),
                title: "Build".into(),
                content: content.into(),
                confidence: 0.9,
                invalidation_condition: "build_profile_changed".into(),
            }],
            conventions: vec![SharedMemory {
                key: "architecture".into(),
                category: "general".into(),
                title: "Architecture".into(),
                content: "Use feature modules".into(),
                confidence: 0.8,
                invalidation_condition: String::new(),
            }],
            eval_sets: vec![SharedEvalSet {
                key: "recovery".into(),
                name: "Recovery".into(),
                cases: vec![SharedEvalCase {
                    scenario_id: scenario.id,
                    expected: scenario.expected,
                }],
            }],
        }
    }

    fn local_memory(conn: &Connection) {
        let now = chrono::Utc::now().timestamp();
        crate::db::queries::insert_memory(
            conn,
            &ProjectMemory {
                id: "local".into(),
                project_id: "p".into(),
                category: "architecture".into(),
                title: "Architecture".into(),
                content: "Keep local architecture".into(),
                enabled: true,
                source_kind: "user".into(),
                source_ref: "memory_panel".into(),
                scope: "project".into(),
                confidence: 1.0,
                version: 1,
                confirmed: true,
                pinned: false,
                invalidation_condition: String::new(),
                invalidated_at: None,
                invalidation_reason: None,
                created_at: now,
                updated_at: now,
            },
        )
        .unwrap();
    }

    #[test]
    fn conflict_is_staged_eval_runs_and_batch_reverts_without_touching_local_fact() {
        let mut conn = database();
        local_memory(&conn);
        let package = package("1.0.0", "hvigorw assembleHap");
        let preview = preview(&conn, "p", &package).unwrap();
        assert_eq!(preview.conflicts, 1);
        let import = apply(&mut conn, "p", &package).unwrap();
        let memories = crate::db::queries::list_memories(&conn, "p").unwrap();
        let conflict = memories
            .iter()
            .find(|item| item.source_ref.contains(":convention:architecture"))
            .unwrap();
        assert!(!conflict.enabled && !conflict.confirmed);
        assert_eq!(
            memories
                .iter()
                .find(|item| item.id == "local")
                .unwrap()
                .content,
            "Keep local architecture"
        );
        let sets = list_eval_sets(&conn, "p").unwrap();
        assert!(run_eval_set(&conn, "p", &sets[0].id).unwrap().passed);
        assert_eq!(revert(&mut conn, "p", &import.batch_id).unwrap(), 3);
        let remaining = crate::db::queries::list_memories(&conn, "p").unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "local");
        assert!(list_eval_sets(&conn, "p").unwrap().is_empty());
    }

    #[test]
    fn same_source_upgrade_has_history_and_revert_preserves_later_user_edit() {
        let mut conn = database();
        apply(&mut conn, "p", &package("1.0.0", "old")).unwrap();
        let second = apply(&mut conn, "p", &package("1.1.0", "new")).unwrap();
        let id = crate::db::queries::list_memories(&conn, "p")
            .unwrap()
            .into_iter()
            .find(|item| item.title == "Build")
            .unwrap()
            .id;
        conn.execute(
            "UPDATE project_memories SET source_kind='user',content='my edit' WHERE id=?1",
            [&id],
        )
        .unwrap();
        let reverted = revert(&mut conn, "p", &second.batch_id).unwrap();
        assert_eq!(reverted, 0);
        assert_eq!(
            crate::db::queries::list_memories_for_revert(&conn, &id)
                .unwrap()
                .unwrap()
                .content,
            "my edit"
        );
        assert!(list_imports(&conn, "p")
            .unwrap()
            .iter()
            .any(|item| item.batch_id == second.batch_id && item.state == "reverted"));
    }

    #[test]
    fn same_source_upgrade_reverts_to_previous_snapshot() {
        let mut conn = database();
        apply(&mut conn, "p", &package("1.0.0", "old")).unwrap();
        let second = apply(&mut conn, "p", &package("1.1.0", "new")).unwrap();
        let changes = list_changes(&conn, "p", &second.batch_id).unwrap();
        assert_eq!(changes.len(), 3);
        assert!(changes.iter().any(|change| {
            change.stable_key == "build-command"
                && change.action == "updated"
                && change.before_json.is_some()
        }));

        assert_eq!(revert(&mut conn, "p", &second.batch_id).unwrap(), 1);
        let restored = crate::db::queries::list_memories(&conn, "p")
            .unwrap()
            .into_iter()
            .find(|item| item.title == "Build")
            .unwrap();
        assert_eq!(restored.content, "old");
        assert_eq!(restored.source_kind, "team_share");
    }

    #[test]
    fn package_version_is_monotonic_and_same_version_is_immutable() {
        let mut conn = database();
        apply(&mut conn, "p", &package("1.1.0", "current")).unwrap();

        let drift = apply(&mut conn, "p", &package("1.1.0", "drift")).unwrap_err();
        assert!(drift.contains("同一共享包版本的内容摘要发生变化"));

        let downgrade = apply(&mut conn, "p", &package("1.0.0", "old")).unwrap_err();
        assert!(downgrade.contains("必须高于当前版本 1.1.0"));
    }

    #[test]
    fn unknown_or_contract_changed_eval_is_rejected() {
        let mut package = package("1.0.0", "build");
        package.eval_sets[0].cases[0].scenario_id = "arbitrary-script".into();
        assert!(validate(&package).unwrap_err().contains("未注册场景"));
    }

    #[test]
    fn provenance_requires_canonical_key_and_absolute_uri_without_credentials() {
        let mut invalid_key = package("1.0.0", "build");
        invalid_key.package_id = "Mobile-Team".into();
        assert!(validate(&invalid_key).unwrap_err().contains("key 非法"));

        let mut relative_source = package("1.0.0", "build");
        relative_source.source.uri = "../team/context.json".into();
        assert!(validate(&relative_source).unwrap_err().contains("绝对 URI"));

        let mut credential_source = package("1.0.0", "build");
        credential_source.source.uri = "https://token@example.com/context".into();
        assert!(validate(&credential_source)
            .unwrap_err()
            .contains("不能包含凭据"));
    }
}
