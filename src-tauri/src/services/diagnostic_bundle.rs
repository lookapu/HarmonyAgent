//! 一键导出诊断包（P2-3，第十三批）。
//!
//! ## 它和 `reproduction_bundle` 是什么关系
//!
//! 两者**不是**同一个东西，脱敏要求正好相反：
//!
//! | | 复现包 | 诊断包 |
//! |---|---|---|
//! | 目的 | 帮开发者复现**某一个**缺陷 | 帮客服判断**环境**是否异常 |
//! | 主体 | 某个会话的完整消息 / 工具输入输出 | 版本、规模计数、近期错误 |
//! | 归属 | 项目级（要 project_id） | 应用级（无项目概念） |
//! | 用户内容 | 就是要给 | **绝不给** |
//!
//! 所以本模块**不**复用复现包的收集逻辑，只复用三样东西：
//! `utils::redact` 的脱敏、ZIP 写盘、条目摘要。收集是另一套——
//! 这正是「同一判据两个方向不能共用口径」在数据面的对应物。
//!
//! ## 隐私契约（本文件最重要的部分）
//!
//! 诊断包的目标是**用户敢直接贴到公开 issue 里**。因此：
//!
//! 1. **不含任何用户内容列**。会话标题、消息正文、任务目标、工具输入输出、
//!    项目名、会话 ID 一律不进包。判据由 `scripts/check-diagnostic-boundary.py`
//!    守着——它逐条扫本文件的 SELECT 列表，命中用户内容列即报红。
//! 2. **错误文本是唯一的自由文本出口**，必须过 `utils::redact::sanitize_text`
//!    （密钥/身份模式 + 路径两道），并按 `MAX_ERROR_TEXT_CHARS` 截断。
//! 3. **两段式**。必须先 `preview` 拿到摘要，再带着摘要 `generate`；
//!    摘要是对「将要导出的确切内容」的承诺，生成时重算比对，不一致就拒绝——
//!    否则「我预览的和导出的不一样」本身就是隐私漏洞。
//!
//! 第 3 条的 `confirmed` 开关是硬闸门：任何绕过预览的调用都直接失败。

use std::io::Write;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::utils::redact;

pub const DIAGNOSTIC_BUNDLE_SCHEMA: u32 = 1;

/// 单条错误文本的字符上限。错误信息常是整段 HTTP 响应体，
/// 不截断的话一个包能到几 MB，而且长文本里更容易带出用户 prompt 片段。
const MAX_ERROR_TEXT_CHARS: usize = 500;
const MAX_TASK_ERRORS: usize = 30;
const MAX_REQUEST_ERRORS: usize = 30;
const MAX_TOOL_ERRORS: usize = 50;
const MAX_ALERTS: usize = 20;
const MAX_RUN_ERRORS: usize = 30;
const MAX_RUNTIME_LOG_LINES: usize = 200;
const MAX_ENTRY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiagnosticPreview {
    pub schema: u32,
    pub preview_digest: String,
    pub app_version: String,
    pub entries: Vec<DiagnosticEntryPreview>,
    pub total_bytes: usize,
    /// 各来源本次实际命中的条数，让用户在导出前就看到「有货」还是「空包」。
    pub source_counts: Vec<DiagnosticSourceCount>,
    /// 因为达到条数上限被截断的来源。非空表示日志被截了。
    pub truncated: Vec<DiagnosticSourceCount>,
    pub generated_at: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiagnosticEntryPreview {
    pub path: String,
    pub kind: String,
    pub bytes: usize,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiagnosticSourceCount {
    pub source: String,
    pub included: usize,
    /// 该来源表内的候选总数（受上限截断前）。`included < available` 即被截断。
    pub available: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiagnosticBundleRecord {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub entry_count: usize,
    pub preview_digest: String,
    pub generated_at: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DiagnosticManifest {
    schema: u32,
    format: String,
    app_version: String,
    preview_digest: String,
    generated_at: i64,
    entries: Vec<DiagnosticEntryPreview>,
    /// 把「不含什么」写进包里。用户拿到文件不用问客服就知道边界。
    excluded: Vec<String>,
}

#[derive(Clone, Debug)]
struct Entry {
    path: String,
    kind: String,
    bytes: Vec<u8>,
}

/// 一行错误记录。**字段名即隐私契约**：门禁只允许这 13 个名字出现在包内。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct ErrorRow {
    // 系统状态类（枚举/整数，无自由文本）
    status: Option<String>,
    state: Option<String>,
    phase: Option<String>,
    severity: Option<String>,
    code: Option<String>,
    error_code: Option<String>,
    error_kind: Option<String>,
    verification_state: Option<String>,
    model: Option<String>,
    tool_name: Option<String>,
    status_code: Option<i64>,
    attempt: Option<i64>,
    retry_count: Option<i64>,
    recovery_count: Option<i64>,
    duration_ms: Option<i64>,
    latency_ms: Option<i64>,
    first_token_ms: Option<i64>,
    is_streaming: Option<i64>,
    created_at: Option<i64>,
    started_at: Option<i64>,
    finished_at: Option<i64>,
    // 三个「只有布尔、不给名字」的开关：把「有没有 Provider」这类事实
    // 留下来，却不留 Provider 本身
    has_provider: Option<bool>,
    has_recovery_plan: Option<bool>,
    /// 自由文本（已过脱敏 + 截断 + 地标比对）
    error_message: Option<String>,
}

impl ErrorRow {
    /// 命名走 clippy 的 `into_` 约定：按值消耗 self 的转换不该叫 `to_`。
    fn into_json(self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

/// 预览：算出将要写出的确切字节并给出摘要。不落盘。
pub fn preview(conn: &Connection) -> Result<DiagnosticPreview, String> {
    let collected = collect(conn)?;
    let entries: Vec<DiagnosticEntryPreview> = collected.preview_entries();
    Ok(DiagnosticPreview {
        schema: DIAGNOSTIC_BUNDLE_SCHEMA,
        preview_digest: entries_digest(&entries),
        app_version: env!("CARGO_PKG_VERSION").into(),
        total_bytes: entries.iter().map(|entry| entry.bytes).sum(),
        entries,
        source_counts: collected.counts,
        truncated: collected.truncated,
        generated_at: collected.generated_at,
    })
}

/// 生成：必须由用户显式确认，且摘要必须与预览一致。
pub fn generate(
    conn: &Connection,
    dest: &str,
    confirmed: bool,
    expected_preview_digest: &str,
) -> Result<DiagnosticBundleRecord, String> {
    if !confirmed {
        return Err("导出诊断包必须由用户显式确认".into());
    }
    if expected_preview_digest.trim().is_empty() {
        return Err("缺少预览摘要；请先预览再确认导出".into());
    }
    let collected = collect(conn)?;
    let entries = collected.preview_entries();
    let preview_digest = entries_digest(&entries);
    if preview_digest != expected_preview_digest {
        return Err("预览后错误日志已变化；请重新预览并确认".into());
    }

    let dir = PathBuf::from(dest);
    if !dir.is_dir() {
        return Err(format!("目标目录不存在：{}", dir.display()));
    }
    let generated_at = collected.generated_at;
    let manifest = DiagnosticManifest {
        schema: DIAGNOSTIC_BUNDLE_SCHEMA,
        format: "harmony-agent-diagnostic-bundle".into(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        preview_digest: preview_digest.clone(),
        generated_at,
        entries: entries.clone(),
        excluded: EXCLUDED_FROM_BUNDLE.iter().map(|item| item.to_string()).collect(),
    };
    // 文件名用**内容摘要**做后缀而不是时间戳：同内容重复导出得到同名文件，
    // 用户会得到「已存在」而不是拿到两份一模一样的东西。
    let filename = format!(
        "harmony-agent-diagnostic-{}-{}.zip",
        generated_at,
        short_digest(&preview_digest)
    );
    let final_path = dir.join(&filename);
    if final_path.exists() {
        return Err(format!("诊断包文件已存在：{}", final_path.display()));
    }
    let temp_path = dir.join(format!(".{filename}.tmp"));
    if let Err(error) = write_archive(&temp_path, &manifest, &collected.entries) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(error);
    }
    std::fs::rename(&temp_path, &final_path).map_err(|error| {
        let _ = std::fs::remove_file(&temp_path);
        format!("提交诊断包文件失败：{error}")
    })?;

    let bytes = std::fs::metadata(&final_path)
        .map_err(|error| error.to_string())?
        .len();
    Ok(DiagnosticBundleRecord {
        sha256: file_sha256(&final_path)?,
        path: final_path.display().to_string(),
        bytes,
        entry_count: collected.entries.len(),
        preview_digest,
        generated_at,
    })
}

/// 写进 manifest 的排除清单。这不是注释，是**随包分发的对外承诺**，
/// 门禁会校验它与 `check-diagnostic-boundary.py` 的用户内容列清单对得上。
const EXCLUDED_FROM_BUNDLE: &[&str] = &[
    "会话标题与消息正文",
    "任务目标（agent_runs.goal）",
    "工具输入与输出（tool_runs.input_json / result_json）",
    "项目名称与项目路径",
    "会话 ID / 运行 ID（可反向定位到具体内容）",
    "Provider 名称与端点（只报有无，不报名字）",
    "设备序列号 / UDID / 签名材料",
];

struct Collected {
    entries: Vec<Entry>,
    counts: Vec<DiagnosticSourceCount>,
    truncated: Vec<DiagnosticSourceCount>,
    generated_at: i64,
}

impl Collected {
    fn preview_entries(&self) -> Vec<DiagnosticEntryPreview> {
        self.entries
            .iter()
            .map(|entry| DiagnosticEntryPreview {
                path: entry.path.clone(),
                kind: entry.kind.clone(),
                bytes: entry.bytes.len(),
                sha256: digest(&entry.bytes),
            })
            .collect()
    }
}

/// 收集全部条目。**本文件所有 SQL 的 SELECT 列表都是隐私契约的一部分**，
/// 门禁 `check-diagnostic-boundary.py` 逐列扫描，命中用户内容列即报红。
/// 改这里之前先读那个脚本的 docstring。
fn collect(conn: &Connection) -> Result<Collected, String> {
    // 地标要在任何收集动作之前取好：它既是「不导出」的理由，也是「丢掉整条」的依据。
    let landmarks = collect_landmarks(conn);
    let mut entries: Vec<Entry> = Vec::new();
    let mut counts: Vec<DiagnosticSourceCount> = Vec::new();
    let mut truncated: Vec<DiagnosticSourceCount> = Vec::new();

    // ---- 环境体检 ----
    // 只报**数量**，不报名称：Provider / MCP 名字由用户起，可能是人名。
    entries.push(json_entry(
        "environment.json",
        "environment",
        &environment_json(conn)?,
    )?);

    // ---- 错误来源 1：任务级失败（chat / agent 主循环）----
    // 不取 conversation_id：它是回指用户内容的钥匙。
    // **注意**：下面这段 SQL 的列清单受门禁保护。
    let (rows, available) = query_rows(
        conn,
        "task_runs",
        "SELECT status, error_kind, error_message, model, provider_id, duration_ms, retry_count, finished_at
         FROM task_runs
         WHERE status <> 'success'
         ORDER BY started_at DESC",
        MAX_TASK_ERRORS,
    )?;
    push_error_file(
        &mut entries,
        &mut counts,
        &mut truncated,
        "errors/task-runs.json",
        "task_runs",
        "task_runs WHERE status <> 'success'",
        "不含 conversation_id / goal / 标题",
        available,
        &rows,
        &landmarks,
        |row, lm| ErrorRow {
            status: row.text("status"),
            error_kind: row.text("error_kind"),
            error_message: row.clean("error_message", lm),
            model: row.text("model"),
            has_provider: row.present("provider_id"),
            duration_ms: row.int("duration_ms"),
            retry_count: row.int("retry_count"),
            finished_at: row.int("finished_at"),
            ..ErrorRow::default()
        },
    )?;

    // ---- 错误来源 2：模型请求失败（网络/鉴权/限流/上下文超长）----
    // 不取 session_id（= 会话 ID）。provider_id 只留「有无」。
    let (rows, available) = query_rows(
        conn,
        "request_logs",
        "SELECT status_code, error_message, model, provider_id, latency_ms, first_token_ms, is_streaming, created_at
         FROM request_logs
         WHERE error_message IS NOT NULL OR (status_code IS NOT NULL AND status_code >= 400)
         ORDER BY created_at DESC",
        MAX_REQUEST_ERRORS,
    )?;
    push_error_file(
        &mut entries,
        &mut counts,
        &mut truncated,
        "errors/request-logs.json",
        "request_logs",
        "request_logs WHERE error_message IS NOT NULL OR status_code >= 400",
        "不含 session_id",
        available,
        &rows,
        &landmarks,
        |row, lm| ErrorRow {
            status_code: row.int("status_code"),
            error_message: row.clean("error_message", lm),
            model: row.text("model"),
            has_provider: row.present("provider_id"),
            latency_ms: row.int("latency_ms"),
            first_token_ms: row.int("first_token_ms"),
            is_streaming: row.int("is_streaming"),
            created_at: row.int("created_at"),
            ..ErrorRow::default()
        },
    )?;

    // ---- 错误来源 3：工具调用失败 ----
    // 刻意**不取** input_json / result_json：那是用户代码与命令输出，
    // 是复现包的内容，不是诊断包的内容。
    let (rows, available) = query_rows(
        conn,
        "tool_runs",
        "SELECT tool_name, status, error_code, duration_ms, retry_count, verification_state, created_at
         FROM tool_runs
         WHERE status = 'error' OR error_code IS NOT NULL
         ORDER BY created_at DESC",
        MAX_TOOL_ERRORS,
    )?;
    push_error_file(
        &mut entries,
        &mut counts,
        &mut truncated,
        "errors/tool-runs.json",
        "tool_runs",
        "tool_runs WHERE status = 'error' OR error_code IS NOT NULL",
        "不含 input_json / result_json",
        available,
        &rows,
        &landmarks,
        |row, _lm| ErrorRow {
            tool_name: row.text("tool_name"),
            status: row.text("status"),
            error_code: row.text("error_code"),
            duration_ms: row.int("duration_ms"),
            retry_count: row.int("retry_count"),
            verification_state: row.text("verification_state"),
            created_at: row.int("created_at"),
            ..ErrorRow::default()
        },
    )?;

    // ---- 错误来源 4：Agent 运行失败 ----
    // 刻意**不取** goal：那是用户写的任务目标。
    let (rows, available) = query_rows(
        conn,
        "agent_runs",
        "SELECT state, phase, attempt, recovery_count, error, started_at, finished_at
         FROM agent_runs
         WHERE error IS NOT NULL
         ORDER BY started_at DESC",
        MAX_RUN_ERRORS,
    )?;
    push_error_file(
        &mut entries,
        &mut counts,
        &mut truncated,
        "errors/agent-runs.json",
        "agent_runs",
        "agent_runs WHERE error IS NOT NULL",
        "不含 goal / conversation_id",
        available,
        &rows,
        &landmarks,
        |row, lm| ErrorRow {
            state: row.text("state"),
            phase: row.text("phase"),
            attempt: row.int("attempt"),
            recovery_count: row.int("recovery_count"),
            error_message: row.clean("error", lm),
            started_at: row.int("started_at"),
            finished_at: row.int("finished_at"),
            ..ErrorRow::default()
        },
    )?;

    // ---- 错误来源 5：SLO 告警 ----
    // severity/code 是枚举，message 是系统生成的模板文案，不含用户内容。
    let (rows, available) = query_rows(
        conn,
        "agent_alerts",
        "SELECT severity, code, message, state, created_at
         FROM agent_alerts
         ORDER BY created_at DESC",
        MAX_ALERTS,
    )?;
    push_error_file(
        &mut entries,
        &mut counts,
        &mut truncated,
        "errors/alerts.json",
        "agent_alerts",
        "agent_alerts",
        "SLO 告警，系统生成文案",
        available,
        &rows,
        &landmarks,
        |row, lm| ErrorRow {
            severity: row.text("severity"),
            code: row.text("code"),
            state: row.text("state"),
            error_message: row.clean("message", lm),
            created_at: row.int("created_at"),
            ..ErrorRow::default()
        },
    )?;

    // ---- 错误来源 6：设备端运行日志（内存环形缓冲）----
    // 唯一不来自 DB 的来源：宿主应用自身的错误在 DB 里没有统一表，
    // 而设备端 hilog 是「最近一次真实故障现场」最有价值的一段。
    // **它是内存态**：重启即清空，空包不代表「没出过错」。
    let log = crate::agent::runtime_log::recent_all(MAX_RUNTIME_LOG_LINES);
    let log_lines = log.lines().count();
    entries.push(text_entry("runtime-log.txt", "device-log", &log)?);
    push_counts(
        &mut counts,
        &mut truncated,
        "runtime_log",
        log_lines,
        log_lines,
        false,
    );

    // ---- 说明文件 ----
    // 必须最后 push：它的正文依赖上面统计出来的 counts。
    entries.push(text_entry(
        "README.txt",
        "readme",
        &readme_text(&counts, &truncated),
    )?);

    Ok(Collected {
        entries,
        counts,
        truncated,
        generated_at: chrono::Utc::now().timestamp(),
    })
}

#[allow(clippy::too_many_arguments)]
fn push_error_file<F>(
    entries: &mut Vec<Entry>,
    counts: &mut Vec<DiagnosticSourceCount>,
    truncated: &mut Vec<DiagnosticSourceCount>,
    path: &str,
    source: &str,
    query: &str,
    note: &str,
    available: usize,
    rows: &[RawRow],
    landmarks: &[String],
    build: F,
) -> Result<(), String>
where
    F: Fn(&RawRow, &[String]) -> ErrorRow,
{
    let payload: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| build(row, landmarks).into_json())
        .collect();
    entries.push(json_entry(
        path,
        "errors",
        &serde_json::json!({
            "source": query,
            "included": rows.len(),
            "available": available,
            "note": note,
            "rows": payload,
        }),
    )?);
    push_counts(counts, truncated, source, rows.len(), available, true);
    Ok(())
}

/// 环境体检。只报数量与版本，不报任何用户起的名字。
fn environment_json(conn: &Connection) -> Result<serde_json::Value, String> {
    // 表可能不存在（老库 / 精简测试库），查不到就报 0 而不是让整个包失败。
    // 「诊断包因为少一张表就导不出来」是更糟的失败模式。
    let count_of = |table: &str| -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap_or(0)
    };
    let schema_version: i64 = conn
        .query_row("SELECT COALESCE(MAX(id),0) FROM _migrations", [], |row| {
            row.get(0)
        })
        .unwrap_or(0);
    Ok(serde_json::json!({
        "generator": {
            "name": "HarmonyAgent",
            "version": env!("CARGO_PKG_VERSION"),
            "bundle_schema": DIAGNOSTIC_BUNDLE_SCHEMA,
        },
        "host": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "family": std::env::consts::FAMILY,
            "dll_extension": std::env::consts::DLL_EXTENSION,
        },
        "database": {
            "schema_version": schema_version,
            "counts": {
                "projects": count_of("projects"),
                "conversations": count_of("conversations"),
                "messages": count_of("messages"),
                "providers": count_of("providers"),
                "models": count_of("models"),
                "mcp_servers": count_of("mcp_servers"),
                "skills": count_of("skills"),
            },
        },
        "limits": {
            "max_error_text_chars": MAX_ERROR_TEXT_CHARS,
            "max_task_errors": MAX_TASK_ERRORS,
            "max_request_errors": MAX_REQUEST_ERRORS,
            "max_tool_errors": MAX_TOOL_ERRORS,
            "max_run_errors": MAX_RUN_ERRORS,
            "max_alerts": MAX_ALERTS,
            "max_runtime_log_lines": MAX_RUNTIME_LOG_LINES,
        },
    }))
}

fn readme_text(counts: &[DiagnosticSourceCount], truncated: &[DiagnosticSourceCount]) -> String {
    let mut text = String::from(
        "# HarmonyAgent 诊断包\n\n\
         用途：把「环境与错误」交给客服或开发者定位问题。\n\n\
         ## 包含什么\n\n\
         - 应用版本、宿主系统、数据库 schema 版本与数据规模计数（只有条数）\n\
         - 近期的任务失败 / 模型请求失败 / 工具失败 / Agent 运行失败 / SLO 告警\n\
         - 设备端 hilog 运行日志尾部（脱敏后；**重启即清空**，空不代表没出错）\n\n\
         ## 不包含什么\n\n",
    );
    for item in EXCLUDED_FROM_BUNDLE {
        text.push_str(&format!("- {item}\n"));
    }
    text.push_str(
        "\n错误文本已过两道脱敏（密钥与身份模式、文件路径）并截断到 500 字符；\
         仍可能残留与故障无关的上下文片段，发送前请自行过目。\n\n\
         ## 本次采集\n\n| 来源 | 收入 | 库内候选 |\n|---|---|---|\n",
    );
    for count in counts {
        text.push_str(&format!(
            "| {} | {} | {} |\n",
            count.source, count.included, count.available
        ));
    }
    if truncated.is_empty() {
        text.push_str("\n所有来源均未触及条数上限。\n");
    } else {
        text.push_str("\n以下来源触及条数上限，仅保留最近的若干条：\n\n");
        for count in truncated {
            text.push_str(&format!(
                "- {}：库内 {} 条，包内 {} 条\n",
                count.source, count.available, count.included
            ));
        }
    }
    text
}

/// 一行查询结果，按**列名**取值。
///
/// 按下标取值会让 SQL 改列顺序就静默取错列——而这个文件的列顺序
/// 正是隐私契约的一部分，按名字取能让「契约被改坏」变成编译期或显式的错。
struct RawRow {
    columns: std::collections::BTreeMap<String, rusqlite::types::Value>,
}

impl RawRow {
    fn text(&self, column: &str) -> Option<String> {
        match self.columns.get(column)? {
            rusqlite::types::Value::Text(value) => Some(value.clone()),
            rusqlite::types::Value::Integer(value) => Some(value.to_string()),
            rusqlite::types::Value::Real(value) => Some(value.to_string()),
            _ => None,
        }
    }

    fn int(&self, column: &str) -> Option<i64> {
        match self.columns.get(column)? {
            rusqlite::types::Value::Integer(value) => Some(*value),
            rusqlite::types::Value::Real(value) => Some(*value as i64),
            rusqlite::types::Value::Text(value) => value.parse().ok(),
            _ => None,
        }
    }

    /// 只回答「有没有」，不给值。用于 provider_id 这类：事实有用、身份不该给。
    fn present(&self, column: &str) -> Option<bool> {
        self.columns
            .get(column)
            .map(|value| !matches!(value, rusqlite::types::Value::Null))
    }

    /// 自由文本出口：两道脱敏 + 地标比对 + 截断。**包内只有这一个函数能写用户可读文本。**
    fn clean(&self, column: &str, landmarks: &[String]) -> Option<String> {
        clean(self.text(column).as_deref(), landmarks)
    }
}

/// 用户内容地标：一旦在错误文本里出现，就说明那条错误**混进了用户内容**。
///
/// ## 为什么需要这一道
///
/// `redact_text` 按**模式**遮蔽（密钥、JWT、邮箱、家目录）。但错误文本是
/// 自由文本，运行时完全可能把任务目标、命令、路径原样抄进去，例如：
///
/// ```text
/// command failed: hdc shell hdc list targets (goal: 修复登录页无法跳转)
/// ```
///
/// 「修复登录页无法跳转」不匹配任何敏感模式，任何正则都抓不到它。
/// **任意用户文本都不可能被模式脱敏识别**——这是脱敏的能力边界，不是实现疏忽。
///
/// 所以换一种判据：不问「这段文字像不像敏感信息」，而问
/// 「这段文字里有没有**我库里已知属于用户**的串」。命中的整条字段丢掉，
/// 只留一个占位标记。地标本身**只用于比对，绝不进包**。
///
/// ## 地标取哪几样
///
/// - `agent_runs.goal`：任务目标，最常被错误消息引用
/// - `conversations.title`：会话标题
/// - `projects.name`：项目名
/// - `messages.content`：近期用户消息。运行时常把「处理这条消息时出错」
///   连同原文一起写进错误字段，不纳入的话这一路是敞开的。
///
/// ## 长度下限
///
/// 短于 `MIN_LANDMARK_CHARS` 的地标不参与比对。这里有个**方向不对称**：
///
/// - 误杀（把正常错误当成含用户内容而丢掉）→ 少一条诊断信息，用户看得见、能反馈
/// - 漏放（放过一条引用了用户内容的错误）→ 隐私泄漏，用户看不见、不可挽回
///
/// 所以阈值要偏小。`6` 是权衡点：中文常用词「错误」「失败」「超时」都在 2-3 字、
/// 会被排除，而「登录页跳转失败」这类真实句子 7 字以上能被抓住。
/// 英文侧同理，`failed` 6 字会被误杀一次，代价可接受。
/// 残余风险：极短的用户内容（如「重跑」）仍可能漏网——但它在 500 字符
/// 截断里也只剩两个字符，且 README 已明写发送前请自行过目。
const MIN_LANDMARK_CHARS: usize = 6;
/// 地标总数上限。比对是 O(地标数 × 错误数)，不设上限的话一个千库会拖慢导出。
const MAX_LANDMARKS: usize = 600;
/// 单条消息地标的比对长度上限。整段消息当子串去匹配既慢又会大面积误杀
/// （错误消息里出现消息里的某个词是常事），只取前缀已能覆盖「原文被抄进错误」的形态。
const MAX_LANDMARK_MATCH_CHARS: usize = 120;

fn collect_landmarks(conn: &Connection) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    let mut push_column = |sql: &str, cap_each: Option<usize>| {
        if let Ok(mut stmt) = conn.prepare(sql) {
            if let Ok(rows) = stmt.query_map([], |row| row.get::<_, Option<String>>(0)) {
                // 两层 flatten：外层吃掉 Result，内层丢掉 NULL
                for value in rows.flatten().flatten() {
                    let trimmed = value.trim();
                    let candidate = match cap_each {
                        Some(cap) if trimmed.chars().count() > cap => {
                            trimmed.chars().take(cap).collect::<String>()
                        }
                        _ => trimmed.to_string(),
                    };
                    if candidate.chars().count() >= MIN_LANDMARK_CHARS {
                        values.push(candidate);
                    }
                }
            }
        }
    };
    push_column(
        "SELECT goal FROM agent_runs WHERE goal IS NOT NULL ORDER BY started_at DESC LIMIT 200",
        None,
    );
    push_column(
        "SELECT title FROM conversations WHERE title IS NOT NULL ORDER BY updated_at DESC LIMIT 200",
        None,
    );
    push_column("SELECT name FROM projects WHERE name IS NOT NULL LIMIT 100", None);
    push_column(
        "SELECT content FROM messages WHERE role='user' AND content IS NOT NULL ORDER BY created_at DESC LIMIT 200",
        Some(MAX_LANDMARK_MATCH_CHARS),
    );
    values.truncate(MAX_LANDMARKS);
    values
}

/// 地标命中时的占位。写清楚发生了什么，比留一个空字符串有用。
const LANDMARK_HIT: &str = "[已省略：该条错误混入了你的任务/会话内容]";

fn clean(value: Option<&str>, landmarks: &[String]) -> Option<String> {
    let value = value?;
    if value.trim().is_empty() {
        return None;
    }
    // 长度过滤放在**函数内**而不是只靠 collect_landmarks：
    // 让 `clean` 的正确性依赖调用方已经过滤过，是把契约写在调用点上，
    // 下一个调用方忘了过滤就会静默误杀。函数自己守，才守得住。
    let mut usable = landmarks
        .iter()
        .filter(|mark| mark.chars().count() >= MIN_LANDMARK_CHARS);
    // 比对原文与脱敏后两版：脱敏可能把一段用户文字改得不再相同，
    // 只查脱敏后的版本会漏掉「脱敏恰好打断了匹配」的情形。
    if usable.clone().any(|mark| value.contains(mark.as_str())) {
        return Some(LANDMARK_HIT.to_string());
    }
    let sanitized = redact::sanitize_text(value, None);
    if usable.any(|mark| sanitized.contains(mark.as_str())) {
        return Some(LANDMARK_HIT.to_string());
    }
    let total = sanitized.chars().count();
    if total <= MAX_ERROR_TEXT_CHARS {
        return Some(sanitized);
    }
    let head: String = sanitized.chars().take(MAX_ERROR_TEXT_CHARS).collect();
    Some(format!("{head}…[已截断 {total} 字符]"))
}

/// 表是否还在。缺表时该来源按 0 条处理。
///
/// 「诊断包因为少一张表就导不出来」是最糟的失败模式——用户点导出拿到一个
/// 报错，而他真正想要的是**其余**那份证据。所以这里只吞「表不存在」，
/// 列不存在之类的错误照常上抛（那是迁移没跟上，是真 bug，必须被看见）。
fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |row| row.get(0),
    )
    .unwrap_or(false)
}

/// 跑一次受限查询，同时拿「取到的条数」与「库内候选总数」。
/// 两者不等即说明被上限截断——这个信息必须让用户知道，否则他会以为看到的是全部。
fn query_rows(
    conn: &Connection,
    table: &str,
    sql: &str,
    limit: usize,
) -> Result<(Vec<RawRow>, usize), String> {
    if !table_exists(conn, table) {
        return Ok((Vec::new(), 0));
    }
    let available: usize = conn
        .query_row(&format!("SELECT COUNT(*) FROM ({sql})"), [], |row| row.get(0))
        .map_err(|error| format!("统计 {table} 条数失败：{error}"))?;
    let bounded = format!("SELECT * FROM ({sql}) LIMIT {limit}");
    let mut stmt = conn.prepare(&bounded).map_err(|error| error.to_string())?;
    let names: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|name| name.to_string())
        .collect();
    let rows = stmt
        .query_map([], |row| {
            let mut columns = std::collections::BTreeMap::new();
            for (index, name) in names.iter().enumerate() {
                columns.insert(name.clone(), row.get::<_, rusqlite::types::Value>(index)?);
            }
            Ok(RawRow { columns })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok((rows, available))
}

fn push_counts(
    counts: &mut Vec<DiagnosticSourceCount>,
    truncated: &mut Vec<DiagnosticSourceCount>,
    source: &str,
    included: usize,
    available: usize,
    bounded: bool,
) {
    let entry = DiagnosticSourceCount {
        source: source.into(),
        included,
        available,
    };
    if bounded && included < available {
        truncated.push(entry.clone());
    }
    counts.push(entry);
}

fn text_entry(path: &str, kind: &str, text: &str) -> Result<Entry, String> {
    // 文本出口也要过脱敏：设备端 hilog 里可能带出用户数据路径与业务标识
    let bytes = redact::sanitize_text(text, None).into_bytes();
    if bytes.len() > MAX_ENTRY_BYTES {
        return Err(format!("{path} 超过单条目上限"));
    }
    Ok(Entry {
        path: path.into(),
        kind: kind.into(),
        bytes,
    })
}

fn json_entry(path: &str, kind: &str, value: &serde_json::Value) -> Result<Entry, String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_ENTRY_BYTES {
        return Err(format!("{path} 超过单条目上限"));
    }
    Ok(Entry {
        path: path.into(),
        kind: kind.into(),
        bytes,
    })
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn short_digest(value: &str) -> String {
    value
        .rsplit(':')
        .next()
        .unwrap_or(value)
        .chars()
        .take(8)
        .collect()
}

fn file_sha256(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn entries_digest(entries: &[DiagnosticEntryPreview]) -> String {
    let mut hasher = Sha256::new();
    for entry in entries {
        hasher.update((entry.path.len() as u64).to_be_bytes());
        hasher.update(entry.path.as_bytes());
        hasher.update((entry.kind.len() as u64).to_be_bytes());
        hasher.update(entry.kind.as_bytes());
        hasher.update((entry.bytes as u64).to_be_bytes());
        hasher.update(entry.sha256.as_bytes());
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn write_archive(
    path: &Path,
    manifest: &DiagnosticManifest,
    entries: &[Entry],
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
        .map_err(|error| format!("创建临时诊断包失败：{error}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// 秘密与用户内容各出现一次，断言它们**都不在**包里。
    ///
    /// 这套标记串是本文件的核心判据：每一条都必须能从包内任一 entry 的
    /// 原始字节里找不到。少写一条 = 少验一条 = 那个字段的泄漏无人看守。
    const SECRET_KEY: &str = "sk-leakedkeyabcdefghijklmnop";
    const SECRET_JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJqb2huIn0.dozjgNryP4J3jVmNHl0w5N";
    const SECRET_HOME: &str = "/Users/diagnostic-victim/secret-place";
    const USER_MESSAGE: &str = "USERMESSAGE-SHOULD-NEVER-APPEAR";
    const USER_CONV_TITLE: &str = "USERCONVTITLE-SHOULD-NEVER-APPEAR";
    const USER_GOAL: &str = "USERGOAL-SHOULD-NEVER-APPEAR";
    const USER_PROJ_NAME: &str = "USERPROJNAME-SHOULD-NEVER-APPEAR";
    const USER_TOOL_INPUT: &str = "USERTOOLINPUT-SHOULD-NEVER-APPEAR";
    const USER_TOOL_OUTPUT: &str = "USERTOOLOUTPUT-SHOULD-NEVER-APPEAR";
    const USER_CONV_ID: &str = "conv-must-not-leak";
    const USER_RUN_ID: &str = "run-must-not-leak";
    const USER_PROVIDER_NAME: &str = "USERPROVIDERNAME-SHOULD-NEVER-APPEAR";

    /// 真实 schema 的内存库，灌满「每张表都有敏感内容」的数据。
    fn seeded_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&conn).unwrap();
        let now = 1_700_000_000_000i64;

        // providers 必须先插：request_logs.provider_id 有外键。
        // 这里存的是「用户起的名字 + 密钥」，两者都不得进包。
        conn.execute(
            "INSERT INTO providers(id,name,provider_type,base_url,api_key,created_at,updated_at)
             VALUES('prov-1',?1,'openai-compatible','https://api.example.com',?2,?3,?3)",
            rusqlite::params![USER_PROVIDER_NAME, SECRET_KEY, now],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO projects(id,name,path,kind,trusted,index_state,created_at)
             VALUES('p',?1,'/Users/diagnostic-victim/projects/app','harmony',1,'ready',?2)",
            rusqlite::params![USER_PROJ_NAME, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO conversations(id,project_id,title,created_at,updated_at)
             VALUES(?1,'p',?2,?3,?3)",
            rusqlite::params![USER_CONV_ID, USER_CONV_TITLE, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages(id,conversation_id,role,content,model,created_at)
             VALUES('m',?1,'user',?2,'model-x',?3)",
            rusqlite::params![USER_CONV_ID, USER_MESSAGE, now],
        )
        .unwrap();

        // 任务失败：error_message 里同时埋了密钥、JWT、家目录路径与用户原文
        conn.execute(
            "INSERT INTO task_runs(id,conversation_id,status,error_kind,error_message,model,provider_id,
                                    duration_ms,retry_count,started_at,finished_at)
             VALUES('t',?1,'error','provider_timeout',?2,'model-x','prov-1',1234,2,?3,?4)",
            rusqlite::params![
                USER_CONV_ID,
                format!("api_key={SECRET_KEY} token={SECRET_JWT} at {SECRET_HOME} while handling {USER_MESSAGE}"),
                now,
                now + 1000
            ],
        )
        .unwrap();

        // 另一条**不含任何用户内容**、但含密钥与家目录路径的失败。
        // 它的存在是为了证明两层机制是独立的：地标层放第一条，模式层放这一条。
        // 只留一条错误的话，「不泄漏」可能只是因为整条被丢掉了，看不出遮蔽有没有生效。
        conn.execute(
            "INSERT INTO task_runs(id,conversation_id,status,error_kind,error_message,model,provider_id,
                                    duration_ms,retry_count,started_at,finished_at)
             VALUES('t-clean',?1,'error','http_401',?2,'model-x','prov-1',77,0,?3,?4)",
            rusqlite::params![
                USER_CONV_ID,
                format!("401 from {SECRET_HOME} with api_key={SECRET_KEY}"),
                now,
                now + 500
            ],
        )
        .unwrap();

        // 请求失败：provider 只应留「有无」，session_id 根本不该被 SELECT
        conn.execute(
            "INSERT INTO request_logs(id,provider_id,model,error_message,session_id,latency_ms,status_code,created_at)
             VALUES('r','prov-1','model-x',?1,?2,900,401,?3)",
            rusqlite::params![
                format!("401 Unauthorized: bearer {SECRET_KEY} for {USER_MESSAGE}"),
                USER_CONV_ID,
                now
            ],
        )
        .unwrap();

        // 工具失败：input_json / result_json 装了用户代码与命令输出
        conn.execute(
            "INSERT INTO tool_runs(id,conversation_id,tool_name,input_json,result_json,status,error_code,
                                    duration_ms,created_at)
             VALUES('tr',?1,'read_file',?2,?3,'error','TOOL_IO',55,?4)",
            rusqlite::params![
                USER_CONV_ID,
                format!("{{\"path\":\"{USER_TOOL_INPUT}\"}}"),
                format!("{USER_TOOL_OUTPUT} api_key={SECRET_KEY}"),
                now
            ],
        )
        .unwrap();

        // Agent 运行失败：goal 是用户写的任务目标
        conn.execute(
            "INSERT INTO agent_runs(run_id,conversation_id,goal,state,phase,attempt,recovery_count,
                                    error,started_at,updated_at)
             VALUES(?1,?2,?3,'failed','execute',2,1,?4,?5,?5)",
            rusqlite::params![
                USER_RUN_ID,
                USER_CONV_ID,
                USER_GOAL,
                format!("recovered from {SECRET_HOME} for {USER_GOAL}"),
                now
            ],
        )
        .unwrap();

        // SLO 告警 + 成功任务（成功任务**不该**出现在包里）
        conn.execute(
            "INSERT INTO agent_alerts(alert_id,tenant_id,run_id,severity,code,message,state,created_at)
             VALUES('al','local',?1,'error','SLO_BREACH',?2,'open',?3)",
            rusqlite::params![
                USER_RUN_ID,
                format!("latency breach {SECRET_JWT}"),
                now
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_runs(id,conversation_id,status,started_at,finished_at)
             VALUES('t-ok',?1,'success',?2,?2)",
            rusqlite::params![USER_CONV_ID, now],
        )
        .unwrap();
        conn
    }

    /// 把生成的包读回内存，返回 (文件名 -> 内容文本)。
    fn read_archive(path: &Path) -> std::collections::BTreeMap<String, String> {
        let file = std::fs::File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut out = std::collections::BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let name = entry.name().to_string();
            let mut buffer = String::new();
            entry.read_to_string(&mut buffer).unwrap();
            out.insert(name, buffer);
        }
        out
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "harmony-agent-diag-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// **核心判据**：包里任何一处字节都不得出现这些串。
    ///
    /// 注意这里检查的是**全部 entry 的全部原始文本**而不是解析后的字段——
    /// 泄漏可能发生在 manifest、README 或任何一处，漏查一个文件就等于没查。
    fn assert_no_leak(files: &std::collections::BTreeMap<String, String>) {
        let mut haystack = String::new();
        for (name, content) in files {
            haystack.push_str(name);
            haystack.push('\n');
            haystack.push_str(content);
            haystack.push('\n');
        }
        for secret in [
            SECRET_KEY,
            SECRET_JWT,
            SECRET_HOME,
            USER_MESSAGE,
            USER_CONV_TITLE,
            USER_GOAL,
            USER_PROJ_NAME,
            USER_TOOL_INPUT,
            USER_TOOL_OUTPUT,
            USER_CONV_ID,
            USER_RUN_ID,
            USER_PROVIDER_NAME,
        ] {
            assert!(
                !haystack.contains(secret),
                "诊断包泄漏了 {secret}\n---\n{haystack}"
            );
        }
    }

    #[test]
    fn generated_bundle_contains_no_user_content_and_no_secrets() {
        let conn = seeded_conn();
        let dir = temp_dir("leak");

        let preview = preview(&conn).unwrap();
        let record = generate(&conn, dir.to_str().unwrap(), true, &preview.preview_digest).unwrap();

        let files = read_archive(Path::new(&record.path));
        assert_no_leak(&files);

        // 结构：manifest + 环境 + 5 类错误 + 设备日志 + 说明
        let names: Vec<&String> = files.keys().collect();
        for expected in [
            "manifest.json",
            "environment.json",
            "errors/task-runs.json",
            "errors/request-logs.json",
            "errors/tool-runs.json",
            "errors/agent-runs.json",
            "errors/alerts.json",
            "runtime-log.txt",
            "README.txt",
        ] {
            assert!(names.iter().any(|n| n.as_str() == expected), "缺少 {expected}：{names:?}");
        }

        // 错误确实被收进来了（否则「不泄漏」是因为什么都没导出）
        let task = &files["errors/task-runs.json"];
        assert!(task.contains("provider_timeout"), "{task}");
        assert!(task.contains("http_401"), "{task}");

        // **两层机制各自生效，且互相独立**：
        // 混入用户内容的那条 → 整条字段被换成占位符
        assert!(task.contains(LANDMARK_HIT), "{task}");
        // 只含密钥不含用户内容的那条 → 走模式遮蔽，保留可诊断内容
        assert!(task.contains("api_key=***"), "密钥应被遮蔽而非删除：{task}");
        assert!(task.contains("<HOME>/secret-place"), "家目录应被替换：{task}");
        // 遮蔽不是删除：原文的其余部分还得留着，否则错误信息本身没了意义
        assert!(task.contains("401 from"), "{task}");

        // 成功的任务不该出现
        assert!(!task.contains("t-ok"), "成功任务不应进包：{task}");

        // Provider 只留「有无」。这里**解析后按结构断言**而不是子串匹配——
        // 上一版用子串匹配 `"has_provider":true`，而美化输出是 `"has_provider": true`，
        // 少一个空格就报红。断言写错与实现写坏在退出码上同形，解析能分开它们。
        let parsed: serde_json::Value = serde_json::from_str(task).unwrap();
        let rows = parsed["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2, "{task}");
        for row in rows {
            assert_eq!(row["has_provider"], serde_json::Value::Bool(true), "{row}");
            assert!(row.get("provider_id").is_none(), "Provider ID 不应进包：{row}");
            assert!(row.get("conversation_id").is_none(), "会话 ID 不应进包：{row}");
        }
        let clean_row = rows
            .iter()
            .find(|row| row["error_kind"] == "http_401")
            .expect("应有未被地标拦下的那条");
        assert_eq!(clean_row["error_message"], "401 from <HOME>/secret-place with api_key=***");
        let dropped_row = rows
            .iter()
            .find(|row| row["error_kind"] == "provider_timeout")
            .expect("应有被地标拦下的那条");
        assert_eq!(dropped_row["error_message"], LANDMARK_HIT);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn environment_reports_counts_but_never_names() {
        let conn = seeded_conn();
        let dir = temp_dir("env");
        let preview = preview(&conn).unwrap();
        let record = generate(&conn, dir.to_str().unwrap(), true, &preview.preview_digest).unwrap();
        let files = read_archive(Path::new(&record.path));
        let env = &files["environment.json"];
        assert!(env.contains("\"conversations\": 1"), "{env}");
        assert!(env.contains("\"messages\": 1"), "{env}");
        assert!(env.contains("\"projects\": 1"), "{env}");
        assert!(env.contains("\"providers\": 1"), "{env}");
        assert!(!env.contains(USER_PROJ_NAME), "{env}");
        assert!(!env.contains(USER_PROVIDER_NAME), "{env}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn generate_refuses_without_confirmation() {
        let conn = seeded_conn();
        let dir = temp_dir("noconfirm");
        let preview = preview(&conn).unwrap();
        let error = generate(&conn, dir.to_str().unwrap(), false, &preview.preview_digest).unwrap_err();
        assert!(error.contains("显式确认"), "{error}");
        // 拒绝时不能留下任何文件
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn generate_refuses_without_or_with_wrong_digest() {
        let conn = seeded_conn();
        let dir = temp_dir("digest");
        let error = generate(&conn, dir.to_str().unwrap(), true, "  ").unwrap_err();
        assert!(error.contains("预览摘要"), "{error}");
        let error = generate(&conn, dir.to_str().unwrap(), true, "sha256:deadbeef").unwrap_err();
        assert!(error.contains("已变化"), "{error}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn generate_refuses_when_logs_changed_after_preview() {
        let conn = seeded_conn();
        let dir = temp_dir("changed");
        let preview = preview(&conn).unwrap();
        // 预览之后又失败了一次——这正是「预览 ≠ 导出」要拦的场景
        conn.execute(
            "INSERT INTO task_runs(id,conversation_id,status,error_kind,error_message,started_at,finished_at)
             VALUES('t-late','c','error','late_failure','late',1,2)",
            [],
        )
        .unwrap();
        let error = generate(&conn, dir.to_str().unwrap(), true, &preview.preview_digest).unwrap_err();
        assert!(error.contains("已变化"), "{error}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn error_text_is_capped_and_says_so() {
        let long = "E".repeat(MAX_ERROR_TEXT_CHARS * 3);
        let cleaned = clean(Some(long.as_str()), &[]).unwrap();
        assert!(cleaned.contains("已截断"), "{cleaned}");
        // 截断标记本身也要截在上限内，不能因为加了标记就超
        assert!(cleaned.chars().count() <= MAX_ERROR_TEXT_CHARS + 16, "{}", cleaned.chars().count());
    }

    #[test]
    fn empty_and_null_error_text_become_none_not_empty_strings() {
        assert!(clean(None, &[]).is_none());
        assert!(clean(Some("   "), &[]).is_none());
    }

    /// 地标这一道的**直接**判据：不经过 SQL，只验比对逻辑本身。
    #[test]
    fn landmark_hit_drops_the_whole_field_instead_of_leaking_it() {
        let goal = "修复登录页无法跳转".to_string();
        let text = format!("command failed: hdc shell list targets (goal: {goal})");
        let out = clean(Some(text.as_str()), std::slice::from_ref(&goal)).unwrap();
        assert_eq!(out, LANDMARK_HIT);
        assert!(!out.contains("登录页"));
        // 不带地标时同一条文本正常保留（证明是比对在起作用，不是无条件清空）
        assert!(clean(Some(text.as_str()), &[]).unwrap().contains("hdc"));
    }

    #[test]
    fn short_landmarks_are_ignored_to_avoid_false_positives() {
        // 「失败」这种词出现在任何报错里都正常，误杀会让诊断包失去信息量
        let out = clean(Some("失败：断言不成立"), &["失败".to_string()]).unwrap();
        assert!(out.contains("断言不成立"), "{out}");
        // 但只要够长（≥ MIN_LANDMARK_CHARS）就必须拦
        let long_mark = "登录页跳转失败".to_string();
        let out = clean(
            Some("运行时报错：登录页跳转失败，请重试"),
            std::slice::from_ref(&long_mark),
        )
        .unwrap();
        assert_eq!(out, LANDMARK_HIT);
    }

    #[test]
    fn collect_landmarks_applies_minimum_length() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&conn).unwrap();
        let now = 1_700_000_000_000i64;
        conn.execute(
            "INSERT INTO projects(id,name,path,kind,trusted,index_state,created_at)
             VALUES('p','短名','/tmp/p','harmony',1,'ready',?1)",
            [now],
        )
        .unwrap();
        assert!(
            !collect_landmarks(&conn).contains(&"短名".to_string()),
            "短于下限的地标不该参与比对"
        );
    }

    #[test]
    fn landmarks_are_collected_from_db_but_never_returned_in_the_bundle() {
        let conn = seeded_conn();
        let landmarks = collect_landmarks(&conn);
        assert!(landmarks.contains(&USER_GOAL.to_string()), "{landmarks:?}");
        assert!(landmarks.contains(&USER_CONV_TITLE.to_string()), "{landmarks:?}");
        assert!(landmarks.contains(&USER_PROJ_NAME.to_string()), "{landmarks:?}");
        // 消息正文也是地标：运行时常把用户原文抄进错误字段
        assert!(landmarks.contains(&USER_MESSAGE.to_string()), "{landmarks:?}");
    }

    #[test]
    fn preview_reports_truncation_when_source_exceeds_limit() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&conn).unwrap();
        let now = 1_700_000_000_000i64;
        // tool_runs.conversation_id 有外键，得先把父行建出来
        conn.execute(
            "INSERT INTO projects(id,name,path,kind,trusted,index_state,created_at)
             VALUES('p','P','/tmp/p','harmony',1,'ready',?1)",
            [now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO conversations(id,project_id,title,created_at,updated_at)
             VALUES('c','p','C',?1,?1)",
            [now],
        )
        .unwrap();
        for index in 0..(MAX_TOOL_ERRORS + 7) {
            conn.execute(
                "INSERT INTO tool_runs(id,conversation_id,tool_name,status,error_code,created_at)
                 VALUES(?1,'c','t','error','E',?2)",
                rusqlite::params![format!("tr{index}"), now + index as i64],
            )
            .unwrap();
        }
        let preview = preview(&conn).unwrap();
        let tool = preview
            .source_counts
            .iter()
            .find(|c| c.source == "tool_runs")
            .unwrap();
        assert_eq!(tool.included, MAX_TOOL_ERRORS);
        assert_eq!(tool.available, MAX_TOOL_ERRORS + 7);
        assert!(
            preview.truncated.iter().any(|c| c.source == "tool_runs"),
            "截断必须让用户看见：{:?}",
            preview.truncated
        );
        // README 里也要写明截断
        assert!(preview.entries.iter().any(|e| e.path == "README.txt"));
    }

    #[test]
    fn preview_and_generate_agree_on_digest_for_unchanged_data() {
        let conn = seeded_conn();
        let dir = temp_dir("stable");
        let first = preview(&conn).unwrap();
        let second = preview(&conn).unwrap();
        assert_eq!(first.preview_digest, second.preview_digest, "同一份数据两次预览摘要应一致");
        let record = generate(&conn, dir.to_str().unwrap(), true, &first.preview_digest).unwrap();
        assert_eq!(record.preview_digest, first.preview_digest);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_tables_do_not_break_the_bundle() {
        // 只跑最小 schema：诊断包必须能在残缺库里导出，
        // 否则「表少一张 → 导不出包 → 客服拿不到证据」是最糟的失败模式。
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE _migrations(id INTEGER PRIMARY KEY, name TEXT, applied_at INTEGER);")
            .unwrap();
        let dir = temp_dir("minimal");
        let preview = preview(&conn).unwrap();
        let record = generate(&conn, dir.to_str().unwrap(), true, &preview.preview_digest).unwrap();
        let files = read_archive(Path::new(&record.path));
        assert!(files.contains_key("environment.json"));
        assert_no_leak(&files);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn manifest_declares_what_is_excluded() {
        let conn = seeded_conn();
        let dir = temp_dir("manifest");
        let preview = preview(&conn).unwrap();
        let record = generate(&conn, dir.to_str().unwrap(), true, &preview.preview_digest).unwrap();
        let files = read_archive(Path::new(&record.path));
        let manifest: serde_json::Value =
            serde_json::from_str(&files["manifest.json"]).unwrap();
        let excluded = manifest["excluded"].as_array().unwrap();
        assert!(excluded.len() >= 5, "排除清单太短：{excluded:?}");
        let joined = excluded
            .iter()
            .map(|v| v.as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ");
        for topic in ["会话标题", "消息正文", "任务目标", "工具输入与输出", "会话 ID"] {
            assert!(joined.contains(topic), "排除清单未提到「{topic}」：{joined}");
        }
        assert_eq!(manifest["format"], "harmony-agent-diagnostic-bundle");
        std::fs::remove_dir_all(&dir).ok();
    }
}
