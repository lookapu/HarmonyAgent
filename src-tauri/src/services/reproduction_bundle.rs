//! Confirmed, redacted and integrity-verifiable issue reproduction bundles (EC12).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const REPRODUCTION_BUNDLE_SCHEMA: u32 = 1;
const MAX_ATTACHMENT_COUNT: usize = 20;
const MAX_ATTACHMENT_BYTES: u64 = 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 2 * 1024 * 1024;
const MAX_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 128;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReproductionRequest {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub expected: String,
    #[serde(default)]
    pub actual: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default = "default_true")]
    pub include_messages: bool,
    #[serde(default = "default_true")]
    pub include_tool_runs: bool,
    #[serde(default = "default_true")]
    pub include_run_events: bool,
    #[serde(default)]
    pub attachments: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReproductionEntryPreview {
    pub path: String,
    pub kind: String,
    pub bytes: usize,
    pub sha256: String,
    pub redacted: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReproductionPreview {
    pub schema: u32,
    pub title: String,
    pub preview_digest: String,
    pub conversation_id: Option<String>,
    pub run_id: Option<String>,
    pub entries: Vec<ReproductionEntryPreview>,
    pub total_bytes: usize,
    pub redacted_entry_count: usize,
    pub omitted_attachments: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReproductionManifestEntry {
    pub path: String,
    pub kind: String,
    pub bytes: usize,
    pub sha256: String,
    pub redacted: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReproductionManifest {
    pub schema: u32,
    pub format: String,
    pub bundle_id: String,
    pub title: String,
    pub preview_digest: String,
    pub generator_version: String,
    pub generated_at: i64,
    pub entries: Vec<ReproductionManifestEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReproductionBundleRecord {
    pub bundle_id: String,
    pub project_id: String,
    pub conversation_id: Option<String>,
    pub run_id: Option<String>,
    pub title: String,
    pub preview_digest: String,
    pub archive_rel_path: String,
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub entry_count: usize,
    pub redacted_entry_count: usize,
    pub generated_at: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArchiveValidation {
    pub valid: bool,
    pub bundle_id: String,
    pub entry_count: usize,
    pub preview_digest: String,
}

#[derive(Clone, Debug)]
struct BundleEntry {
    path: String,
    kind: String,
    bytes: Vec<u8>,
    redacted: bool,
}

#[derive(Clone, Debug)]
struct CollectedBundle {
    title: String,
    conversation_id: Option<String>,
    run_id: Option<String>,
    entries: Vec<BundleEntry>,
    omitted_attachments: Vec<String>,
    warnings: Vec<String>,
}

fn default_true() -> bool {
    true
}

pub fn parse_request(value: &serde_json::Value) -> Result<ReproductionRequest, String> {
    let request: ReproductionRequest = serde_json::from_value(value.clone())
        .map_err(|error| format!("复现包请求格式错误：{error}"))?;
    validate_request(&request)?;
    Ok(request)
}

pub fn validate_request(request: &ReproductionRequest) -> Result<(), String> {
    let text_fields = [
        ("title", request.title.as_str(), 120usize, true),
        ("description", request.description.as_str(), 10_000, false),
        ("expected", request.expected.as_str(), 5_000, false),
        ("actual", request.actual.as_str(), 5_000, false),
    ];
    for (name, value, limit, required) in text_fields {
        if (required && value.trim().is_empty()) || value.chars().count() > limit {
            return Err(format!("{name} 为空或超过 {limit} 字符"));
        }
    }
    if request.steps.len() > 50
        || request
            .steps
            .iter()
            .any(|step| step.trim().is_empty() || step.chars().count() > 1_000)
    {
        return Err("复现步骤最多 50 条，每条必须为 1-1000 字符".into());
    }
    if request.attachments.len() > MAX_ATTACHMENT_COUNT {
        return Err(format!("附件最多 {MAX_ATTACHMENT_COUNT} 个"));
    }
    let mut unique = BTreeSet::new();
    for path in &request.attachments {
        validate_relative_path(path)?;
        if !unique.insert(path) {
            return Err(format!("附件路径重复：{path}"));
        }
    }
    Ok(())
}

pub fn preview(
    conn: &Connection,
    project_id: &str,
    request: &ReproductionRequest,
) -> Result<ReproductionPreview, String> {
    validate_request(request)?;
    let collected = collect(conn, project_id, request)?;
    preview_from_collected(&collected)
}

pub fn generate(
    conn: &mut Connection,
    project_id: &str,
    request: &ReproductionRequest,
    confirmed: bool,
    expected_preview_digest: &str,
) -> Result<ReproductionBundleRecord, String> {
    if !confirmed {
        return Err("生成复现包必须由用户显式确认".into());
    }
    if expected_preview_digest.trim().is_empty() {
        return Err("缺少预览摘要；请先预览再确认生成".into());
    }
    validate_request(request)?;
    let collected = collect(conn, project_id, request)?;
    let preview = preview_from_collected(&collected)?;
    if preview.preview_digest != expected_preview_digest {
        return Err("预览后会话、运行证据或附件已变化；请重新预览并确认".into());
    }
    let (_, project_root, _) = project(conn, project_id)?;
    let bundle_id = uuid::Uuid::new_v4().to_string();
    let generated_at = chrono::Utc::now().timestamp_millis();
    let manifest = ReproductionManifest {
        schema: REPRODUCTION_BUNDLE_SCHEMA,
        format: "harmony-agent-reproduction-bundle".into(),
        bundle_id: bundle_id.clone(),
        title: collected.title.clone(),
        preview_digest: preview.preview_digest.clone(),
        generator_version: env!("CARGO_PKG_VERSION").into(),
        generated_at,
        entries: preview
            .entries
            .iter()
            .map(|entry| ReproductionManifestEntry {
                path: entry.path.clone(),
                kind: entry.kind.clone(),
                bytes: entry.bytes,
                sha256: entry.sha256.clone(),
                redacted: entry.redacted,
            })
            .collect(),
    };
    let out_dir = project_root.join(".deveco-agent/repro-bundles");
    std::fs::create_dir_all(&out_dir).map_err(|error| format!("创建复现包目录失败：{error}"))?;
    let canonical_root = std::fs::canonicalize(&project_root)
        .map_err(|error| format!("项目目录不可访问：{error}"))?;
    let canonical_out =
        std::fs::canonicalize(&out_dir).map_err(|error| format!("复现包目录不可访问：{error}"))?;
    if !canonical_out.starts_with(&canonical_root) {
        return Err("复现包目录通过符号链接逃逸项目边界".into());
    }
    let filename = format!(
        "{}-{}-{}.zip",
        slug(&collected.title),
        generated_at,
        &bundle_id[..8]
    );
    let final_path = out_dir.join(&filename);
    let temp_path = out_dir.join(format!(".{filename}.tmp"));
    write_archive(&temp_path, &manifest, &collected.entries)?;
    if let Err(error) = validate_archive(&temp_path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!("复现包生成后校验失败：{error}"));
    }
    std::fs::rename(&temp_path, &final_path).map_err(|error| {
        let _ = std::fs::remove_file(&temp_path);
        format!("提交复现包文件失败：{error}")
    })?;
    let archive_bytes = std::fs::metadata(&final_path)
        .map_err(|error| error.to_string())?
        .len();
    let archive_sha256 = file_sha256(&final_path)?;
    let archive_rel_path = format!(".deveco-agent/repro-bundles/{filename}");
    let record = ReproductionBundleRecord {
        bundle_id: bundle_id.clone(),
        project_id: project_id.into(),
        conversation_id: collected.conversation_id.clone(),
        run_id: collected.run_id.clone(),
        title: collected.title,
        preview_digest: preview.preview_digest,
        archive_rel_path,
        archive_sha256,
        archive_bytes,
        entry_count: preview.entries.len(),
        redacted_entry_count: preview.redacted_entry_count,
        generated_at,
    };
    let tx = conn.transaction().map_err(|error| error.to_string())?;
    let persisted = (|| -> Result<(), String> {
        tx.execute(
            "INSERT INTO reproduction_bundles(bundle_id,project_id,conversation_id,run_id,title,preview_digest,archive_rel_path,archive_sha256,archive_bytes,entry_count,redacted_entry_count,generated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![record.bundle_id,record.project_id,record.conversation_id,record.run_id,record.title,record.preview_digest,record.archive_rel_path,record.archive_sha256,record.archive_bytes as i64,record.entry_count as i64,record.redacted_entry_count as i64,record.generated_at],
        ).map_err(|error| error.to_string())?;
        crate::agent::enterprise::audit(
            &tx,
            record.run_id.as_deref(),
            record.conversation_id.as_deref(),
            "user",
            "reproduction_bundle.generate",
            &record.bundle_id,
            "success",
            &serde_json::json!({
                "project_id": project_id,
                "preview_digest": record.preview_digest,
                "archive_sha256": record.archive_sha256,
                "archive_bytes": record.archive_bytes,
                "entry_count": record.entry_count,
                "redacted_entry_count": record.redacted_entry_count,
            }),
        )?;
        tx.commit().map_err(|error| error.to_string())
    })();
    if let Err(error) = persisted {
        let _ = std::fs::remove_file(&final_path);
        return Err(error);
    }
    Ok(record)
}

pub fn list_records(
    conn: &Connection,
    project_id: &str,
) -> Result<Vec<ReproductionBundleRecord>, String> {
    project(conn, project_id)?;
    let mut stmt = conn.prepare("SELECT bundle_id,project_id,conversation_id,run_id,title,preview_digest,archive_rel_path,archive_sha256,archive_bytes,entry_count,redacted_entry_count,generated_at FROM reproduction_bundles WHERE project_id=?1 ORDER BY generated_at DESC,bundle_id DESC").map_err(|error|error.to_string())?;
    let rows = stmt
        .query_map([project_id], |row| {
            Ok(ReproductionBundleRecord {
                bundle_id: row.get(0)?,
                project_id: row.get(1)?,
                conversation_id: row.get(2)?,
                run_id: row.get(3)?,
                title: row.get(4)?,
                preview_digest: row.get(5)?,
                archive_rel_path: row.get(6)?,
                archive_sha256: row.get(7)?,
                archive_bytes: row.get::<_, i64>(8)?.max(0) as u64,
                entry_count: row.get::<_, i64>(9)?.max(0) as usize,
                redacted_entry_count: row.get::<_, i64>(10)?.max(0) as usize,
                generated_at: row.get(11)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

pub fn validate_record_archive(
    conn: &Connection,
    project_id: &str,
    bundle_id: &str,
) -> Result<ArchiveValidation, String> {
    let (_, root, _) = project(conn, project_id)?;
    let (rel_path, expected_sha): (String, String) = conn
        .query_row(
            "SELECT archive_rel_path,archive_sha256 FROM reproduction_bundles WHERE project_id=?1 AND bundle_id=?2",
            params![project_id, bundle_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| "复现包记录不存在或不属于当前项目".to_string())?;
    validate_relative_path(&rel_path)?;
    let path = root.join(&rel_path);
    let actual_sha = file_sha256(&path)?;
    if actual_sha != expected_sha {
        return Err("复现包文件摘要与导出记录不一致".into());
    }
    validate_archive(&path)
}

pub fn validate_archive(path: &Path) -> Result<ArchiveValidation, String> {
    let file = std::fs::File::open(path).map_err(|error| format!("读取复现包失败：{error}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| format!("ZIP 格式错误：{error}"))?;
    if archive.is_empty() || archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err("复现包条目数非法".into());
    }
    let mut payloads = BTreeMap::new();
    let mut manifest_bytes = None;
    let mut payload_bytes = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let name = entry.name().to_string();
        validate_archive_entry_name(&name)?;
        if entry.is_dir() || entry.size() as usize > MAX_ENTRY_BYTES {
            return Err(format!("复现包条目非法或过大：{name}"));
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if name == "manifest.json" {
            if manifest_bytes.replace(bytes).is_some() {
                return Err("复现包含重复 manifest.json".into());
            }
        } else {
            payload_bytes = payload_bytes.saturating_add(bytes.len());
            if payload_bytes > MAX_PAYLOAD_BYTES {
                return Err("复现包解压后载荷超过安全上限".into());
            }
            if payloads.insert(name.clone(), bytes).is_some() {
                return Err(format!("复现包含重复条目：{name}"));
            }
        }
    }
    let manifest: ReproductionManifest = serde_json::from_slice(
        manifest_bytes
            .as_deref()
            .ok_or("复现包缺少 manifest.json")?,
    )
    .map_err(|error| format!("manifest.json 非法：{error}"))?;
    if manifest.schema != REPRODUCTION_BUNDLE_SCHEMA
        || manifest.format != "harmony-agent-reproduction-bundle"
        || manifest.entries.len() != payloads.len()
    {
        return Err("复现包 manifest 版本、格式或条目数不匹配".into());
    }
    let mut listed = BTreeSet::new();
    for expected in &manifest.entries {
        validate_archive_entry_name(&expected.path)?;
        if !listed.insert(expected.path.clone()) {
            return Err(format!("manifest 重复条目：{}", expected.path));
        }
        let bytes = payloads
            .get(&expected.path)
            .ok_or_else(|| format!("manifest 条目缺失：{}", expected.path))?;
        if bytes.len() != expected.bytes || sha256(bytes) != expected.sha256 {
            return Err(format!("复现包条目完整性校验失败：{}", expected.path));
        }
    }
    Ok(ArchiveValidation {
        valid: true,
        bundle_id: manifest.bundle_id,
        entry_count: payloads.len(),
        preview_digest: manifest.preview_digest,
    })
}

fn collect(
    conn: &Connection,
    project_id: &str,
    request: &ReproductionRequest,
) -> Result<CollectedBundle, String> {
    let (project_name, root, project_kind) = project(conn, project_id)?;
    let (conversation_id, run_id) = resolve_context(conn, project_id, request)?;
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let mut omitted_attachments = Vec::new();
    entries.push(text_entry(
        "issue.md",
        "issue",
        &issue_markdown(request),
        &root,
    )?);
    entries.push(json_entry(
        "context/environment.json",
        "environment",
        &environment_json(&project_name, &project_kind, &root),
        &root,
    )?);
    if request.include_messages {
        if let Some(conversation_id) = conversation_id.as_deref() {
            entries.push(json_entry(
                "context/messages.json",
                "conversation",
                &messages_json(conn, conversation_id)?,
                &root,
            )?);
        } else {
            warnings.push("未绑定会话，未包含消息".into());
        }
    }
    if request.include_tool_runs {
        if let Some(conversation_id) = conversation_id.as_deref() {
            entries.push(json_entry(
                "diagnostics/tool-runs.json",
                "tool_runs",
                &tool_runs_json(conn, conversation_id)?,
                &root,
            )?);
        } else {
            warnings.push("未绑定会话，未包含工具调用".into());
        }
    }
    if request.include_run_events {
        if let Some(run_id) = run_id.as_deref() {
            entries.push(json_entry(
                "diagnostics/run.json",
                "run_events",
                &run_json(conn, run_id)?,
                &root,
            )?);
        } else {
            warnings.push("未绑定 Agent Run，未包含运行事件".into());
        }
    }
    for (index, raw) in request.attachments.iter().enumerate() {
        match attachment_entry(&root, raw, index) {
            Ok(entry) => entries.push(entry),
            Err(reason) => omitted_attachments.push(format!("{raw}: {reason}")),
        }
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    if entries.len() + 1 > MAX_ARCHIVE_ENTRIES {
        return Err("复现包条目数超过安全上限".into());
    }
    let total = entries.iter().map(|entry| entry.bytes.len()).sum::<usize>();
    if total > MAX_PAYLOAD_BYTES {
        return Err(format!(
            "复现包脱敏后内容 {:.1} MiB，超过 {:.1} MiB 上限",
            total as f64 / 1024.0 / 1024.0,
            MAX_PAYLOAD_BYTES as f64 / 1024.0 / 1024.0
        ));
    }
    Ok(CollectedBundle {
        title: sanitize_text(request.title.trim(), &root),
        conversation_id,
        run_id,
        entries,
        omitted_attachments,
        warnings,
    })
}

fn preview_from_collected(collected: &CollectedBundle) -> Result<ReproductionPreview, String> {
    let entries = collected
        .entries
        .iter()
        .map(|entry| ReproductionEntryPreview {
            path: entry.path.clone(),
            kind: entry.kind.clone(),
            bytes: entry.bytes.len(),
            sha256: sha256(&entry.bytes),
            redacted: entry.redacted,
        })
        .collect::<Vec<_>>();
    let preview_digest = digest_entries(&collected.entries);
    Ok(ReproductionPreview {
        schema: REPRODUCTION_BUNDLE_SCHEMA,
        title: collected.title.clone(),
        preview_digest,
        conversation_id: collected.conversation_id.clone(),
        run_id: collected.run_id.clone(),
        total_bytes: entries.iter().map(|entry| entry.bytes).sum(),
        redacted_entry_count: entries.iter().filter(|entry| entry.redacted).count(),
        entries,
        omitted_attachments: collected.omitted_attachments.clone(),
        warnings: collected.warnings.clone(),
    })
}

fn project(conn: &Connection, project_id: &str) -> Result<(String, PathBuf, String), String> {
    conn.query_row(
        "SELECT name,path,kind FROM projects WHERE id=?1",
        [project_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                PathBuf::from(row.get::<_, String>(1)?),
                row.get::<_, String>(2)?,
            ))
        },
    )
    .map_err(|_| "目标项目不存在".into())
}

fn resolve_context(
    conn: &Connection,
    project_id: &str,
    request: &ReproductionRequest,
) -> Result<(Option<String>, Option<String>), String> {
    let requested_conversation = request
        .conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(run_id) = request
        .run_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let conversation: String = conn
            .query_row(
                "SELECT ar.conversation_id FROM agent_runs ar JOIN conversations c ON c.id=ar.conversation_id WHERE ar.run_id=?1 AND c.project_id=?2",
                params![run_id, project_id],
                |row| row.get(0),
            )
            .map_err(|_| "指定 Agent Run 不存在或不属于当前项目".to_string())?;
        if requested_conversation.is_some_and(|value| value != conversation) {
            return Err("指定会话与 Agent Run 不属于同一上下文".into());
        }
        return Ok((Some(conversation), Some(run_id.into())));
    }
    let Some(conversation_id) = requested_conversation else {
        return Ok((None, None));
    };
    let belongs: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=?1 AND project_id=?2)",
            params![conversation_id, project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !belongs {
        return Err("指定会话不存在或不属于当前项目".into());
    }
    let run_id = conn
        .query_row(
            "SELECT run_id FROM agent_runs WHERE conversation_id=?1 ORDER BY started_at DESC,run_id DESC LIMIT 1",
            [conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    Ok((Some(conversation_id.into()), run_id))
}

fn environment_json(project_name: &str, project_kind: &str, root: &Path) -> serde_json::Value {
    let model = crate::services::harmony_model::cached(root);
    serde_json::json!({
        "generator": {"name":"HarmonyAgent","version":env!("CARGO_PKG_VERSION")},
        "host": {"os":std::env::consts::OS,"arch":std::env::consts::ARCH},
        "project": {"name":project_name,"kind":project_kind},
        "harmony": {
            "semantic_schema": model.schema_version,
            "app": model.app,
            "build_modes": model.build_modes,
            "products": model.products,
            "modules": model.modules.iter().map(|module| serde_json::json!({
                "name":module.name,"rel_path":module.rel_path,"kind":module.kind,
                "api_type":module.api_type,"artifact_kind":module.artifact_kind,
                "package_name":module.package_name,"device_types":module.device_types,
                "main_element":module.main_element,"targets":module.targets,
                "abilities":module.abilities,"extension_abilities":module.extension_abilities,
                "permissions":module.permissions,
            })).collect::<Vec<_>>(),
            "dependencies": model.dependencies,
            "manifests": model.manifests,
        }
    })
}

fn messages_json(conn: &Connection, conversation_id: &str) -> Result<serde_json::Value, String> {
    let mut stmt = conn.prepare("SELECT role,content,references_json,plan_json,tool_calls_json,model,created_at FROM (SELECT id,role,content,references_json,plan_json,tool_calls_json,model,created_at FROM messages WHERE conversation_id=?1 ORDER BY created_at DESC,id DESC LIMIT 50) ORDER BY created_at,id").map_err(|error|error.to_string())?;
    let rows = stmt
        .query_map([conversation_id], |row| {
            Ok(serde_json::json!({
                "role":row.get::<_,String>(0)?,
                "content":row.get::<_,String>(1)?,
                "references":parse_optional_json(row.get::<_,Option<String>>(2)?),
                "plan":parse_optional_json(row.get::<_,Option<String>>(3)?),
                "tool_calls":parse_optional_json(row.get::<_,Option<String>>(4)?),
                "model":row.get::<_,Option<String>>(5)?,
                "created_at":row.get::<_,i64>(6)?,
            }))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(serde_json::json!({"limit":50,"messages":rows}))
}

fn tool_runs_json(conn: &Connection, conversation_id: &str) -> Result<serde_json::Value, String> {
    let mut stmt = conn.prepare("SELECT tool_name,input_json,result_json,status,duration_ms,effect_kind,recovery_policy,error_code,compensation_json,metrics_json,created_at FROM (SELECT id,tool_name,input_json,result_json,status,duration_ms,effect_kind,recovery_policy,error_code,compensation_json,metrics_json,created_at FROM tool_runs WHERE conversation_id=?1 ORDER BY created_at DESC,id DESC LIMIT 100) ORDER BY created_at,id").map_err(|error|error.to_string())?;
    let rows = stmt
        .query_map([conversation_id], |row| {
            Ok(serde_json::json!({
                "tool":row.get::<_,String>(0)?,
                "input":parse_optional_json(row.get::<_,Option<String>>(1)?),
                "result":parse_optional_json(row.get::<_,Option<String>>(2)?),
                "status":row.get::<_,String>(3)?,
                "duration_ms":row.get::<_,Option<i64>>(4)?,
                "effect_kind":row.get::<_,String>(5)?,
                "recovery_policy":row.get::<_,String>(6)?,
                "error_code":row.get::<_,Option<String>>(7)?,
                "compensation":parse_optional_json(row.get::<_,Option<String>>(8)?),
                "metrics":parse_optional_json(row.get::<_,Option<String>>(9)?),
                "created_at":row.get::<_,i64>(10)?,
            }))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(serde_json::json!({"limit":100,"tool_runs":rows}))
}

fn run_json(conn: &Connection, run_id: &str) -> Result<serde_json::Value, String> {
    let run = conn.query_row("SELECT goal,state,phase,attempt,recovery_count,resume_policy,acceptance_json,metadata_json,error,started_at,updated_at,finished_at FROM agent_runs WHERE run_id=?1",[run_id],|row| {
        Ok(serde_json::json!({
            "goal":row.get::<_,String>(0)?,"state":row.get::<_,String>(1)?,"phase":row.get::<_,String>(2)?,
            "attempt":row.get::<_,i64>(3)?,"recovery_count":row.get::<_,i64>(4)?,"resume_policy":row.get::<_,String>(5)?,
            "acceptance":parse_optional_json(row.get::<_,Option<String>>(6)?),"metadata":parse_optional_json(row.get::<_,Option<String>>(7)?),
            "error":row.get::<_,Option<String>>(8)?,"started_at":row.get::<_,i64>(9)?,"updated_at":row.get::<_,i64>(10)?,"finished_at":row.get::<_,Option<i64>>(11)?,
        }))
    }).map_err(|error|error.to_string())?;
    let mut stmt = conn.prepare("SELECT seq,event_type,payload,created_at FROM (SELECT event_id,seq,event_type,payload,created_at FROM run_events WHERE run_id=?1 ORDER BY seq DESC LIMIT 200) ORDER BY seq,event_id").map_err(|error|error.to_string())?;
    let events = stmt.query_map([run_id], |row| {
        Ok(serde_json::json!({"seq":row.get::<_,i64>(0)?,"event_type":row.get::<_,String>(1)?,"payload":parse_optional_json(row.get::<_,Option<String>>(2)?),"created_at":row.get::<_,i64>(3)?}))
    }).map_err(|error|error.to_string())?
      .collect::<Result<Vec<_>,_>>().map_err(|error|error.to_string())?;
    Ok(serde_json::json!({"run":run,"event_limit":200,"events":events}))
}

fn issue_markdown(request: &ReproductionRequest) -> String {
    let steps = if request.steps.is_empty() {
        "1. （未提供）".into()
    } else {
        request
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| format!("{}. {}", index + 1, step.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!("# {}\n\n## 问题描述\n\n{}\n\n## 复现步骤\n\n{}\n\n## 预期结果\n\n{}\n\n## 实际结果\n\n{}\n\n> 本文件和包内 JSON 已由 HarmonyAgent 默认脱敏；附件仅接受项目内 UTF-8 文本。\n",request.title.trim(),empty_placeholder(&request.description),steps,empty_placeholder(&request.expected),empty_placeholder(&request.actual))
}

fn empty_placeholder(value: &str) -> &str {
    if value.trim().is_empty() {
        "（未提供）"
    } else {
        value.trim()
    }
}

fn text_entry(path: &str, kind: &str, text: &str, root: &Path) -> Result<BundleEntry, String> {
    let redacted = sanitize_text(text, root);
    let changed = redacted != text;
    build_entry(path, kind, redacted.into_bytes(), changed)
}

fn json_entry(
    path: &str,
    kind: &str,
    value: &serde_json::Value,
    root: &Path,
) -> Result<BundleEntry, String> {
    let redacted = sanitize_json(value, root);
    let changed = &redacted != value;
    let bytes = serde_json::to_vec_pretty(&redacted).map_err(|error| error.to_string())?;
    build_entry(path, kind, bytes, changed)
}

fn build_entry(
    path: &str,
    kind: &str,
    bytes: Vec<u8>,
    redacted: bool,
) -> Result<BundleEntry, String> {
    validate_archive_entry_name(path)?;
    if bytes.len() > MAX_ENTRY_BYTES {
        return Err(format!("复现包条目超过 2 MiB：{path}"));
    }
    Ok(BundleEntry {
        path: path.into(),
        kind: kind.into(),
        bytes,
        redacted,
    })
}

fn attachment_entry(root: &Path, raw: &str, index: usize) -> Result<BundleEntry, String> {
    validate_relative_path(raw)?;
    if sensitive_attachment(raw) {
        return Err("疑似凭据、签名或机器本地配置，默认拒绝".into());
    }
    let canonical_root =
        std::fs::canonicalize(root).map_err(|error| format!("项目目录不可访问：{error}"))?;
    let candidate =
        std::fs::canonicalize(root.join(raw)).map_err(|error| format!("附件不可访问：{error}"))?;
    if !candidate.starts_with(&canonical_root) || !candidate.is_file() {
        return Err("附件不在项目内或不是普通文件".into());
    }
    let metadata = std::fs::metadata(&candidate).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_ATTACHMENT_BYTES {
        return Err("附件超过 1 MiB".into());
    }
    let bytes = std::fs::read(&candidate).map_err(|error| error.to_string())?;
    let text = String::from_utf8(bytes).map_err(|_| "附件不是 UTF-8 文本，默认拒绝二进制内容")?;
    let normalized = raw.replace('\\', "/");
    let sanitized_path = crate::utils::redact::redact_text(&normalized);
    let path_redacted = sanitized_path != normalized;
    let entry_path = if path_redacted {
        format!("attachments/redacted-{}.txt", index + 1)
    } else {
        format!("attachments/{sanitized_path}")
    };
    let mut entry = if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
        json_entry(&entry_path, "attachment", &value, root)?
    } else {
        text_entry(&entry_path, "attachment", &text, root)?
    };
    entry.redacted |= path_redacted;
    Ok(entry)
}

fn sanitize_json(value: &serde_json::Value, root: &Path) -> serde_json::Value {
    sanitize_json_paths(crate::utils::redact::redact_json_value(value), root)
}

fn sanitize_json_paths(value: serde_json::Value, root: &Path) -> serde_json::Value {
    match value {
        serde_json::Value::Object(fields) => serde_json::Value::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key, sanitize_json_paths(value, root)))
                .collect(),
        ),
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .into_iter()
                .map(|value| sanitize_json_paths(value, root))
                .collect(),
        ),
        serde_json::Value::String(text) => serde_json::Value::String(scrub_paths(&text, root)),
        other => other,
    }
}

fn sanitize_text(text: &str, root: &Path) -> String {
    scrub_paths(&crate::utils::redact::redact_text(text), root)
}

fn scrub_paths(text: &str, root: &Path) -> String {
    static UNIX_HOME: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?i)(?:/Users|/home)/[^/\\\s"']+"#).expect("valid home regex")
    });
    static WINDOWS_HOME: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?i)[A-Z]:\\Users\\[^\\/\s"']+"#).expect("valid home regex")
    });
    let mut out = text.to_string();
    for candidate in [
        root.to_string_lossy().to_string(),
        root.to_string_lossy().replace('\\', "/"),
    ] {
        if !candidate.is_empty() {
            out = out.replace(&candidate, "<PROJECT_ROOT>");
        }
    }
    out = UNIX_HOME.replace_all(&out, "<HOME>").into_owned();
    WINDOWS_HOME.replace_all(&out, "<HOME>").into_owned()
}

fn sensitive_attachment(raw: &str) -> bool {
    let lower = raw.replace('\\', "/").to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let extension = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    name == "local.properties"
        || name == ".npmrc"
        || name == ".pypirc"
        || name.starts_with(".env")
        || [
            "pem",
            "key",
            "p12",
            "pfx",
            "jks",
            "keystore",
            "cer",
            "crt",
            "mobileprovision",
            "p7b",
        ]
        .contains(&extension)
        || lower.contains("private_key")
        || lower.contains("signing")
        || name.contains("profile")
        || lower.contains("provisioning")
        || lower.contains("signing/")
        || lower.contains("certificate")
}

fn parse_optional_json(value: Option<String>) -> serde_json::Value {
    match value {
        None => serde_json::Value::Null,
        Some(text) => serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)),
    }
}

fn write_archive(
    path: &Path,
    manifest: &ReproductionManifest,
    entries: &[BundleEntry],
) -> Result<(), String> {
    let mut open_options = std::fs::OpenOptions::new();
    open_options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_options.mode(0o600);
    }
    let file = open_options
        .open(path)
        .map_err(|error| format!("创建临时复现包失败：{error}"))?;
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o600);
    let result = (|| -> Result<(), String> {
        archive
            .start_file("manifest.json", options)
            .map_err(|error| error.to_string())?;
        archive
            .write_all(&serde_json::to_vec_pretty(manifest).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
        for entry in entries {
            archive
                .start_file(&entry.path, options)
                .map_err(|error| error.to_string())?;
            archive
                .write_all(&entry.bytes)
                .map_err(|error| error.to_string())?;
        }
        archive.finish().map_err(|error| error.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

fn digest_entries(entries: &[BundleEntry]) -> String {
    let mut digest = Sha256::new();
    for entry in entries {
        digest.update((entry.path.len() as u64).to_be_bytes());
        digest.update(entry.path.as_bytes());
        digest.update((entry.kind.len() as u64).to_be_bytes());
        digest.update(entry.kind.as_bytes());
        digest.update((entry.bytes.len() as u64).to_be_bytes());
        digest.update(&entry.bytes);
    }
    format!("sha256:{:x}", digest.finalize())
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn validate_relative_path(raw: &str) -> Result<(), String> {
    let path = Path::new(raw);
    if raw.trim().is_empty()
        || raw.chars().any(char::is_control)
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!("必须使用项目内相对路径：{raw}"));
    }
    Ok(())
}

fn validate_archive_entry_name(name: &str) -> Result<(), String> {
    validate_relative_path(name)?;
    if name.starts_with('/') || name.contains("//") || name.contains('\\') {
        return Err(format!("ZIP 条目路径非法：{name}"));
    }
    Ok(())
}

fn slug(title: &str) -> String {
    let value = title
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let collapsed = value
        .split('-')
        .filter(|part| !part.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join("-");
    if collapsed.is_empty() {
        "issue".into()
    } else {
        collapsed.chars().take(48).collect()
    }
}

pub fn handle_tool(
    args: &serde_json::Value,
    project_id: &str,
    conversation_id: &str,
    db: &crate::db::DbState,
) -> Result<String, String> {
    if project_id.is_empty() {
        return Err("reproduction_bundle 需要绑定项目".into());
    }
    let action = args
        .get("action")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("list");
    match action {
        "preview" | "generate" => {
            let mut request = parse_request(args.get("request").ok_or("缺少 request")?)?;
            if request.conversation_id.as_deref().is_none_or(str::is_empty)
                && !conversation_id.is_empty()
            {
                request.conversation_id = Some(conversation_id.into());
            }
            if action == "preview" {
                let conn = db.0.lock().map_err(|error| error.to_string())?;
                serde_json::to_string_pretty(&preview(&conn, project_id, &request)?)
                    .map_err(|error| error.to_string())
            } else {
                let confirmed = args
                    .get("confirmed")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let digest = args
                    .get("preview_digest")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let mut conn = db.0.lock().map_err(|error| error.to_string())?;
                serde_json::to_string_pretty(&generate(
                    &mut conn, project_id, &request, confirmed, digest,
                )?)
                .map_err(|error| error.to_string())
            }
        }
        "list" => {
            let conn = db.0.lock().map_err(|error| error.to_string())?;
            serde_json::to_string_pretty(&list_records(&conn, project_id)?)
                .map_err(|error| error.to_string())
        }
        "validate" => {
            let bundle_id = args
                .get("bundle_id")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or("缺少 bundle_id")?;
            let conn = db.0.lock().map_err(|error| error.to_string())?;
            serde_json::to_string_pretty(&validate_record_archive(&conn, project_id, bundle_id)?)
                .map_err(|error| error.to_string())
        }
        other => Err(format!(
            "未知 action={other}；支持 preview|generate|list|validate"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Connection, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "harmony-agent-repro-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join("logs")).unwrap();
        std::fs::write(
            root.join("logs/build.log"),
            "Authorization: Bearer AbCdEf1234567890XyZ\nbuild failed",
        )
        .unwrap();
        std::fs::write(root.join("local.properties"), "sdk.dir=/private/sdk").unwrap();
        let conn = Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&conn).unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/074_reproduction_bundles.sql"
        ))
        .unwrap();
        let now = 1_700_000_000_000i64;
        conn.execute(
            "INSERT INTO projects(id,name,path,kind,trusted,index_state,created_at) VALUES('p','Demo',?1,'harmony',1,'ready',?2)",
            params![root.to_string_lossy(), now],
        ).unwrap();
        conn.execute("INSERT INTO conversations(id,project_id,title,created_at,updated_at) VALUES('c','p','Failure',?1,?1)",[now]).unwrap();
        conn.execute(
            "INSERT INTO messages(id,conversation_id,role,content,model,created_at) VALUES('m','c','user',?1,'model-x',?2)",
            params![format!("token=sk-abc1234567890abcdef build fails at {}", root.display()),now],
        ).unwrap();
        conn.execute("INSERT INTO agent_runs(run_id,conversation_id,goal,state,phase,attempt,last_event_seq,recovery_count,resume_policy,metadata_json,started_at,updated_at) VALUES('r','c','fix token=sk-abc1234567890abcdef','failed','verify',1,1,0,'continue','{}',?1,?1)",[now]).unwrap();
        conn.execute("INSERT INTO run_events(event_id,run_id,conversation_id,seq,event_type,payload,created_at) VALUES('e','r','c',1,'failure','{\"password\":\"secret-value\"}',?1)",[now]).unwrap();
        conn.execute("INSERT INTO tool_runs(id,conversation_id,tool_name,input_json,result_json,status,effect_kind,recovery_policy,protocol_version,created_at) VALUES('t','c','build_project','{\"api_key\":\"secret-key\"}','{\"error\":\"failed\"}','error','read','safe_retry',2,?1)",[now]).unwrap();
        (conn, root)
    }

    fn request() -> ReproductionRequest {
        ReproductionRequest {
            title: "Build api_key=sk-abc1234567890abcdef".into(),
            description: "Contact a@example.com; token=sk-abc1234567890abcdef".into(),
            steps: vec!["Run build".into()],
            expected: "HAP generated".into(),
            actual: "Authorization: Bearer AbCdEf1234567890XyZ".into(),
            conversation_id: Some("c".into()),
            run_id: Some("r".into()),
            include_messages: true,
            include_tool_runs: true,
            include_run_events: true,
            attachments: vec!["logs/build.log".into(), "local.properties".into()],
        }
    }

    fn archive_text(path: &Path) -> String {
        let file = std::fs::File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut out = String::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            entry.read_to_string(&mut out).unwrap();
        }
        out
    }

    #[test]
    fn preview_redacts_all_sources_and_omits_sensitive_attachment() {
        let (conn, root) = fixture();
        let collected = collect(&conn, "p", &request()).unwrap();
        let preview = preview_from_collected(&collected).unwrap();
        assert!(!preview.title.contains("sk-abc1234567890abcdef"));
        assert_eq!(preview.entries.len(), 6);
        assert!(preview.redacted_entry_count >= 4);
        assert_eq!(preview.omitted_attachments.len(), 1);
        assert!(preview.omitted_attachments[0].contains("local.properties"));
        let combined = collected
            .entries
            .iter()
            .flat_map(|entry| entry.bytes.iter().copied())
            .collect::<Vec<_>>();
        let text = String::from_utf8(combined).unwrap();
        assert!(!text.contains("sk-abc1234567890abcdef"));
        assert!(!text.contains("AbCdEf1234567890XyZ"));
        assert!(!text.contains("secret-value"));
        assert!(!text.contains(&root.to_string_lossy().to_string()));
        std::fs::write(
            root.join("logs/a@example.com.json"),
            r#"{"password":"plain-secret"}"#,
        )
        .unwrap();
        let attachment = attachment_entry(&root, "logs/a@example.com.json", 2).unwrap();
        assert_eq!(attachment.path, "attachments/redacted-3.txt");
        assert!(attachment.redacted);
        assert!(!String::from_utf8(attachment.bytes)
            .unwrap()
            .contains("plain-secret"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn generation_requires_confirmation_and_exact_fresh_preview_then_self_validates() {
        let (mut conn, root) = fixture();
        let request = request();
        let preview = preview(&conn, "p", &request).unwrap();
        assert!(
            generate(&mut conn, "p", &request, false, &preview.preview_digest)
                .unwrap_err()
                .contains("显式确认")
        );
        assert!(generate(&mut conn, "p", &request, true, "sha256:stale")
            .unwrap_err()
            .contains("已变化"));

        let record = generate(&mut conn, "p", &request, true, &preview.preview_digest).unwrap();
        assert!(!record.title.contains("sk-abc1234567890abcdef"));
        assert!(!record.archive_rel_path.contains("sk-abc1234567890abcdef"));
        assert_eq!(list_records(&conn, "p").unwrap().len(), 1);
        let validated = validate_record_archive(&conn, "p", &record.bundle_id).unwrap();
        assert!(validated.valid);
        assert_eq!(validated.entry_count, preview.entries.len());
        let path = root.join(&record.archive_rel_path);
        let text = archive_text(&path);
        assert!(text.contains("harmony-agent-reproduction-bundle"));
        assert!(!text.contains("sk-abc1234567890abcdef"));
        assert!(!text.contains("secret-key"));
        assert!(!text.contains("AbCdEf1234567890XyZ"));
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"tampered")
            .unwrap();
        assert!(validate_record_archive(&conn, "p", &record.bundle_id)
            .unwrap_err()
            .contains("摘要"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn changed_attachment_invalidates_preview_and_traversal_is_rejected() {
        let (mut conn, root) = fixture();
        let request = request();
        let preview = preview(&conn, "p", &request).unwrap();
        std::fs::write(root.join("logs/build.log"), "different").unwrap();
        assert!(
            generate(&mut conn, "p", &request, true, &preview.preview_digest)
                .unwrap_err()
                .contains("已变化")
        );
        let mut invalid = request;
        invalid.attachments = vec!["../secret.txt".into()];
        assert!(validate_request(&invalid)
            .unwrap_err()
            .contains("项目内相对路径"));
        std::fs::remove_dir_all(root).ok();
    }
}
