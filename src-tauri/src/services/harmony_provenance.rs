//! HarmonyOS/OpenHarmony SDK 与官方文档索引的统一来源证明。
//!
//! 本模块不创建新的索引，而是对已有的本机 SDK 声明、官方 API 变更库、
//! 官方 API 参考库和 OpenHarmony 文档镜像做来源、版本、更新时间与覆盖率对账。

use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::services::harmony_env::HarmonyEnv;
use crate::services::sdk_api::ApiIndex;

const FRESH_SECONDS: u64 = 30 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEvidence {
    pub name: &'static str,
    pub status: &'static str,
    pub source: String,
    pub version: String,
    pub updated_at: Option<u64>,
    pub entries: usize,
    pub source_coverage: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProvenanceReport {
    pub sources: Vec<SourceEvidence>,
}

pub fn collect(
    env: &HarmonyEnv,
    sdk_index: Option<&ApiIndex>,
    conn: Option<&Connection>,
    docs_root: Option<&Path>,
) -> ProvenanceReport {
    let mut sources = vec![sdk_evidence(env, sdk_index)];
    sources.push(database_evidence(
        conn,
        "官方 API 变更索引",
        "api_docs",
        "api_level",
    ));
    sources.push(database_evidence(
        conn,
        "官方 API 参考索引",
        "api_details",
        "since_api_level",
    ));
    sources.push(docs_evidence(docs_root));
    ProvenanceReport { sources }
}

pub fn render(report: &ProvenanceReport) -> String {
    let mut out = String::from("[SDK / 官方文档来源证明]\n");
    for item in &report.sources {
        out.push_str(&format!(
            "- {}: {}；版本 {}；条目 {}；来源覆盖 {}\n  来源: {}\n  更新时间: {}\n",
            item.name,
            item.status,
            item.version,
            item.entries,
            item.source_coverage,
            item.source,
            item.updated_at
                .map(format_timestamp)
                .unwrap_or_else(|| "未知".to_string()),
        ));
    }
    let ready = report
        .sources
        .iter()
        .filter(|item| item.status == "可信")
        .count();
    out.push_str(&format!(
        "- 汇总: {ready}/{} 个来源当前可信；“过期”需要刷新，“缺失/不可追溯”不得作为生成代码的唯一依据。\n",
        report.sources.len()
    ));
    out
}

fn sdk_evidence(env: &HarmonyEnv, index: Option<&ApiIndex>) -> SourceEvidence {
    let source = index
        .map(|value| value.api_dir.clone())
        .or_else(|| env.sdk_root.clone())
        .unwrap_or_else(|| "未发现本机 SDK".to_string());
    let version = env
        .default_api
        .clone()
        .or_else(|| (!env.sdk_versions.is_empty()).then(|| env.sdk_versions.join("/")))
        .unwrap_or_else(|| "未知".to_string());
    let entries = index.map(|value| value.modules.len()).unwrap_or(0);
    let updated_at = index.and_then(|value| (value.indexed_at > 0).then_some(value.indexed_at));
    SourceEvidence {
        name: "本机 SDK 声明索引",
        status: status(entries, updated_at, version != "未知"),
        source,
        version,
        updated_at,
        entries,
        source_coverage: if entries > 0 {
            "本机文件 100%"
        } else {
            "0%"
        }
        .to_string(),
    }
}

fn database_evidence(
    conn: Option<&Connection>,
    name: &'static str,
    table: &str,
    version_column: &str,
) -> SourceEvidence {
    let Some(conn) = conn else {
        return missing_database_evidence(name);
    };
    // table/column 只来自本模块常量，不能接收外部输入。
    let sql = format!(
        "SELECT COUNT(*), COUNT(NULLIF(TRIM(source_url), '')), MAX(fetched_at), \
         MIN({version_column}), MAX({version_column}) FROM {table}"
    );
    let row = conn.query_row(&sql, [], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
        ))
    });
    let Ok((total, sourced, updated, min_version, max_version)) = row else {
        return missing_database_evidence(name);
    };
    let hosts = source_hosts(conn, table);
    let total = total.max(0) as usize;
    let sourced = sourced.max(0) as usize;
    let fully_sourced = total > 0 && sourced == total;
    let has_version = min_version.is_some() || max_version.is_some();
    let updated_at = updated
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value > 0);
    SourceEvidence {
        name,
        status: status(total, updated_at, fully_sourced && has_version),
        source: if hosts.is_empty() {
            "未记录 source_url".to_string()
        } else {
            hosts.join(", ")
        },
        version: match (min_version, max_version) {
            (Some(min), Some(max)) if min != max => format!("API {min}…{max}"),
            (Some(value), _) | (_, Some(value)) => format!("API {value}"),
            _ => "未知".to_string(),
        },
        updated_at,
        entries: total,
        source_coverage: if total == 0 {
            "0%".to_string()
        } else {
            format!(
                "{sourced}/{total} ({:.1}%)",
                sourced as f64 * 100.0 / total as f64
            )
        },
    }
}

fn missing_database_evidence(name: &'static str) -> SourceEvidence {
    SourceEvidence {
        name,
        status: "缺失",
        source: "本地知识库未就绪".to_string(),
        version: "未知".to_string(),
        updated_at: None,
        entries: 0,
        source_coverage: "0%".to_string(),
    }
}

fn source_hosts(conn: &Connection, table: &str) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT DISTINCT source_url FROM {table} WHERE source_url IS NOT NULL AND TRIM(source_url) != '' LIMIT 20"
    )) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) else {
        return Vec::new();
    };
    let mut hosts = rows
        .flatten()
        .filter_map(|url| {
            url.split_once("://")
                .map(|(_, rest)| rest.split('/').next().unwrap_or(rest).to_string())
        })
        .collect::<Vec<_>>();
    hosts.sort();
    hosts.dedup();
    hosts
}

fn docs_evidence(root: Option<&Path>) -> SourceEvidence {
    let Some(root) = root else {
        return SourceEvidence {
            name: "OpenHarmony 官方文档镜像",
            status: "缺失",
            source: "尚未下载".to_string(),
            version: "未知".to_string(),
            updated_at: None,
            entries: 0,
            source_coverage: "0%".to_string(),
        };
    };
    let entries = crate::services::harmony_docs::count_docs(root);
    let git_dir = resolve_git_dir(root);
    let source = git_dir
        .as_deref()
        .and_then(read_origin)
        .unwrap_or_else(|| root.to_string_lossy().to_string());
    let version = git_dir
        .as_deref()
        .and_then(read_revision)
        .map(|value| value.chars().take(12).collect())
        .unwrap_or_else(|| "未知".to_string());
    let updated_at = git_dir.as_deref().and_then(observed_at);
    SourceEvidence {
        name: "OpenHarmony 官方文档镜像",
        status: status(
            entries,
            updated_at,
            source.starts_with("http") && version != "未知",
        ),
        source,
        version,
        updated_at,
        entries,
        source_coverage: if entries > 0 {
            "本地镜像可定位到文件"
        } else {
            "0%"
        }
        .to_string(),
    }
}

fn resolve_git_dir(root: &Path) -> Option<PathBuf> {
    let marker = root.join(".git");
    if marker.is_dir() {
        return Some(marker);
    }
    let content = fs::read_to_string(&marker).ok()?;
    let path = content.trim().strip_prefix("gitdir:")?.trim();
    let path = PathBuf::from(path);
    Some(if path.is_absolute() {
        path
    } else {
        root.join(path)
    })
}

fn read_origin(git_dir: &Path) -> Option<String> {
    let config = fs::read_to_string(git_dir.join("config")).ok()?;
    let mut in_origin = false;
    for line in config.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_origin = trimmed == "[remote \"origin\"]";
        } else if in_origin {
            if let Some(value) = trimmed
                .strip_prefix("url")
                .and_then(|value| value.trim_start().strip_prefix('='))
            {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

fn read_revision(git_dir: &Path) -> Option<String> {
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if !head.starts_with("ref:") {
        return Some(head.to_string());
    }
    let reference = head.strip_prefix("ref:")?.trim();
    fs::read_to_string(git_dir.join(reference))
        .ok()
        .map(|value| value.trim().to_string())
        .or_else(|| read_packed_ref(git_dir, reference))
}

fn read_packed_ref(git_dir: &Path, reference: &str) -> Option<String> {
    fs::read_to_string(git_dir.join("packed-refs"))
        .ok()?
        .lines()
        .filter(|line| !line.starts_with('#') && !line.starts_with('^'))
        .find_map(|line| {
            let (hash, name) = line.split_once(' ')?;
            (name == reference).then(|| hash.to_string())
        })
}

fn observed_at(git_dir: &Path) -> Option<u64> {
    [git_dir.join("FETCH_HEAD"), git_dir.join("HEAD")]
        .into_iter()
        .filter_map(|path| fs::metadata(path).ok()?.modified().ok())
        .filter_map(epoch_seconds)
        .max()
}

fn epoch_seconds(value: SystemTime) -> Option<u64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_secs())
}

fn status(entries: usize, updated_at: Option<u64>, traceable: bool) -> &'static str {
    if entries == 0 {
        "缺失"
    } else if !traceable {
        "不可追溯"
    } else if updated_at.is_some_and(|value| now().saturating_sub(value) > FRESH_SECONDS) {
        "过期"
    } else if updated_at.is_none() {
        "不可追溯"
    } else {
        "可信"
    }
}

fn now() -> u64 {
    epoch_seconds(SystemTime::now()).unwrap_or_default()
}

fn format_timestamp(value: u64) -> String {
    chrono::DateTime::from_timestamp(value as i64, 0)
        .map(|value| value.to_rfc3339())
        .unwrap_or_else(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_env() -> HarmonyEnv {
        HarmonyEnv {
            sdk_root: None,
            default_api: None,
            sdk_variants: Vec::new(),
            sdk_versions: Vec::new(),
            cli: None,
            hdc_path: None,
            hdc_source: None,
            ohpm_path: None,
            hvigorw_path: None,
            studio_dir: None,
            source: "auto".to_string(),
            suggestions: Vec::new(),
        }
    }

    #[test]
    fn database_sources_keep_version_time_and_coverage() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE api_docs (source_url TEXT, fetched_at INTEGER, version_label TEXT, api_level INTEGER);\n\
             CREATE TABLE api_details (source_url TEXT, fetched_at INTEGER, module TEXT, since_api_level INTEGER);",
        )
        .unwrap();
        let timestamp = now();
        conn.execute(
            "INSERT INTO api_docs VALUES (?1, ?2, 'API 12', 12)",
            ("https://developer.huawei.com/example", timestamp),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO api_details VALUES (?1, ?2, '@ohos.foo', 12)",
            ("https://docs.openharmony.cn/example", timestamp),
        )
        .unwrap();

        let report = collect(&empty_env(), None, Some(&conn), None);
        assert_eq!(report.sources[1].status, "可信");
        assert_eq!(report.sources[1].version, "API 12");
        assert_eq!(report.sources[1].source_coverage, "1/1 (100.0%)");
        assert!(report.sources[1].source.contains("developer.huawei.com"));
        assert_eq!(report.sources[2].status, "可信");
    }

    #[test]
    fn local_docs_are_bound_to_remote_revision_and_observation_time() {
        let root =
            std::env::temp_dir().join(format!("harmony-provenance-{}", uuid::Uuid::new_v4()));
        let git = root.join(".git");
        fs::create_dir_all(git.join("refs/heads")).unwrap();
        fs::write(root.join("reference.md"), "# API").unwrap();
        fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(git.join("refs/heads/main"), "0123456789abcdef\n").unwrap();
        fs::write(
            git.join("config"),
            "[remote \"origin\"]\n  url = https://gitee.com/openharmony/docs.git\n",
        )
        .unwrap();

        let item = docs_evidence(Some(&root));
        assert_eq!(item.status, "可信");
        assert_eq!(item.version, "0123456789ab");
        assert_eq!(item.entries, 1);
        assert_eq!(item.source, "https://gitee.com/openharmony/docs.git");
        fs::remove_dir_all(root).unwrap();
    }
}
