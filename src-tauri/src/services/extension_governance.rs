//! Shared third-party extension governance (EC10).
//!
//! Integrity and publisher identity are deliberately separate: a detached Ed25519
//! signature proves possession of the supplied key, but the key id remains an
//! asserted identity until a future trust-store policy pins it.

use base64::Engine;
use ring::signature::{UnparsedPublicKey, ED25519};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DEFAULT_CALLS_PER_MINUTE: i64 = 60;
pub const DEFAULT_FAILURE_THRESHOLD: i64 = 5;
pub const DEFAULT_COOLDOWN_SECONDS: i64 = 60;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionAttestation {
    pub source_uri: Option<String>,
    pub source_revision: Option<String>,
    pub algorithm: Option<String>,
    pub signer_key_id: Option<String>,
    pub public_key_base64: Option<String>,
    pub signature_base64: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verification {
    pub state: &'static str,
    pub digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExtensionGovernanceRecord {
    pub extension_kind: String,
    pub extension_id: String,
    pub project_id: Option<String>,
    pub source_uri: Option<String>,
    pub source_revision: Option<String>,
    pub content_sha256: String,
    pub signer_key_id: Option<String>,
    pub verification_state: String,
    pub calls_per_minute: i64,
    pub failure_threshold: i64,
    pub cooldown_seconds: i64,
    pub consecutive_failures: i64,
    pub circuit_open_until: Option<i64>,
    pub last_error: Option<String>,
    pub updated_at: i64,
}

pub fn list(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<Vec<ExtensionGovernanceRecord>, String> {
    let mut statement = conn
        .prepare(
            "SELECT extension_kind,extension_id,project_id,source_uri,source_revision,
         content_sha256,signer_key_id,verification_state,calls_per_minute,failure_threshold,
         cooldown_seconds,consecutive_failures,circuit_open_until,last_error,updated_at
         FROM extension_governance WHERE (?1 IS NULL OR project_id=?1)
         ORDER BY extension_kind,extension_id",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([project_id], |row| {
            Ok(ExtensionGovernanceRecord {
                extension_kind: row.get(0)?,
                extension_id: row.get(1)?,
                project_id: row.get(2)?,
                source_uri: row.get(3)?,
                source_revision: row.get(4)?,
                content_sha256: row.get(5)?,
                signer_key_id: row.get(6)?,
                verification_state: row.get(7)?,
                calls_per_minute: row.get(8)?,
                failure_threshold: row.get(9)?,
                cooldown_seconds: row.get(10)?,
                consecutive_failures: row.get(11)?,
                circuit_open_until: row.get(12)?,
                last_error: row.get(13)?,
                updated_at: row.get(14)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

pub fn configure(
    conn: &Connection,
    kind: &str,
    id: &str,
    calls: i64,
    threshold: i64,
    cooldown: i64,
) -> Result<(), String> {
    validate_kind(kind)?;
    if !(1..=10_000).contains(&calls)
        || !(1..=100).contains(&threshold)
        || !(1..=86_400).contains(&cooldown)
    {
        return Err("扩展治理参数越界：calls_per_minute=1..10000，failure_threshold=1..100，cooldown_seconds=1..86400".into());
    }
    let changed = conn.execute(
        "UPDATE extension_governance SET calls_per_minute=?3,failure_threshold=?4,cooldown_seconds=?5,updated_at=?6 WHERE extension_kind=?1 AND extension_id=?2",
        params![kind,id,calls,threshold,cooldown,chrono::Utc::now().timestamp()],
    ).map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err(format!("未找到 {kind} 扩展 {id}"));
    }
    audit(
        conn,
        kind,
        id,
        "extension.policy.update",
        "success",
        &serde_json::json!({"calls_per_minute":calls,"failure_threshold":threshold,"cooldown_seconds":cooldown}),
    );
    Ok(())
}

pub fn sha256(payload: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(payload))
}

pub fn verify(
    payload: &[u8],
    attestation: Option<&ExtensionAttestation>,
) -> Result<Verification, String> {
    let digest = sha256(payload);
    let Some(attestation) = attestation else {
        return Ok(Verification {
            state: "unsigned",
            digest,
        });
    };
    let fields = (
        attestation.algorithm.as_deref(),
        attestation.signer_key_id.as_deref(),
        attestation.public_key_base64.as_deref(),
        attestation.signature_base64.as_deref(),
    );
    if fields == (None, None, None, None) {
        return Ok(Verification {
            state: "unsigned",
            digest,
        });
    }
    let (Some(algorithm), Some(key_id), Some(public_key), Some(signature)) = fields else {
        return Err("扩展签名字段必须同时包含 algorithm、signer_key_id、public_key_base64 和 signature_base64".into());
    };
    if algorithm != "ed25519" || key_id.trim().is_empty() {
        return Err("仅支持 algorithm=ed25519，且 signer_key_id 不能为空".into());
    }
    if key_id.len() > 128 || public_key.len() > 256 || signature.len() > 512 {
        return Err("扩展签名元数据超过长度上限".into());
    }
    let decoder = base64::engine::general_purpose::STANDARD;
    let public_key = decoder
        .decode(public_key)
        .map_err(|_| "扩展公钥不是合法 Base64")?;
    let signature = decoder
        .decode(signature)
        .map_err(|_| "扩展签名不是合法 Base64")?;
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(payload, &signature)
        .map_err(|_| "扩展 Ed25519 签名验证失败".to_string())?;
    Ok(Verification {
        state: "verified",
        digest,
    })
}

pub fn register(
    conn: &Connection,
    kind: &str,
    id: &str,
    project_id: Option<&str>,
    payload: &[u8],
    attestation: Option<&ExtensionAttestation>,
) -> Result<Verification, String> {
    validate_kind(kind)?;
    let verification_result = verify(payload, attestation);
    let verification = verification_result
        .clone()
        .unwrap_or_else(|_| Verification {
            state: "invalid",
            digest: sha256(payload),
        });
    let now = chrono::Utc::now().timestamp();
    let source_uri = attestation.and_then(|value| value.source_uri.as_deref());
    let source_revision = attestation.and_then(|value| value.source_revision.as_deref());
    validate_source(source_uri, source_revision)?;
    conn.execute(
        "INSERT INTO extension_governance
         (extension_kind,extension_id,project_id,source_uri,source_revision,content_sha256,
          signature_algorithm,signer_key_id,signer_public_key,signature,verification_state,
          calls_per_minute,failure_threshold,cooldown_seconds,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,60,5,60,?12,?12)
         ON CONFLICT(extension_kind,extension_id) DO UPDATE SET
          project_id=excluded.project_id,source_uri=excluded.source_uri,
          source_revision=excluded.source_revision,content_sha256=excluded.content_sha256,
          signature_algorithm=excluded.signature_algorithm,signer_key_id=excluded.signer_key_id,
          signer_public_key=excluded.signer_public_key,signature=excluded.signature,
          verification_state=excluded.verification_state,consecutive_failures=0,
          circuit_open_until=NULL,last_error=NULL,updated_at=excluded.updated_at",
        params![
            kind,
            id,
            project_id,
            source_uri,
            source_revision,
            verification.digest,
            attestation.and_then(|v| v.algorithm.as_deref()),
            attestation.and_then(|v| v.signer_key_id.as_deref()),
            attestation.and_then(|v| v.public_key_base64.as_deref()),
            attestation.and_then(|v| v.signature_base64.as_deref()),
            verification.state,
            now
        ],
    )
    .map_err(|error| error.to_string())?;
    audit(
        conn,
        kind,
        id,
        "extension.register",
        verification.state,
        &serde_json::json!({"project_id":project_id,"source_uri":source_uri,"source_revision":source_revision,"digest":verification.digest,"signature_valid":verification.state == "verified","publisher_trust":"unresolved"}),
    );
    verification_result.map(|_| verification)
}

pub fn before_call(conn: &Connection, kind: &str, id: &str) -> Result<(), String> {
    validate_kind(kind)?;
    let now = chrono::Utc::now().timestamp();
    let row = conn.query_row(
        "SELECT calls_per_minute,window_started_at,window_calls,circuit_open_until,verification_state
         FROM extension_governance WHERE extension_kind=?1 AND extension_id=?2",
        params![kind,id],
        |row| Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,i64>(2)?,row.get::<_,Option<i64>>(3)?,row.get::<_,String>(4)?)),
    ).optional().map_err(|error| error.to_string())?
      .ok_or_else(|| format!("{kind} 扩展 {id} 尚未登记来源，已拒绝调用"))?;
    if row.4 == "invalid" || row.4 == "drifted" {
        return Err(format!("{kind} 扩展 {id} 完整性状态为 {}，已隔离", row.4));
    }
    if row.3.is_some_and(|until| until > now) {
        return Err(format!(
            "{kind} 扩展 {id} 熔断中，请在 {} 后重试",
            row.3.unwrap()
        ));
    }
    let (window_started, calls) = if now.saturating_sub(row.1) >= 60 {
        (now, 1)
    } else {
        (row.1, row.2 + 1)
    };
    if calls > row.0 {
        audit(
            conn,
            kind,
            id,
            "extension.call",
            "rate_limited",
            &serde_json::json!({"limit":row.0}),
        );
        return Err(format!("{kind} 扩展 {id} 超过每分钟 {} 次调用限制", row.0));
    }
    conn.execute(
        "UPDATE extension_governance SET window_started_at=?3,window_calls=?4,
         circuit_open_until=CASE WHEN circuit_open_until<=?5 THEN NULL ELSE circuit_open_until END,
         updated_at=?5 WHERE extension_kind=?1 AND extension_id=?2",
        params![kind, id, window_started, calls, now],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn record_result(conn: &Connection, kind: &str, id: &str, result: &Result<String, String>) {
    let now = chrono::Utc::now().timestamp();
    if result.is_ok() {
        let _ = conn.execute("UPDATE extension_governance SET consecutive_failures=0,last_error=NULL,updated_at=?3 WHERE extension_kind=?1 AND extension_id=?2", params![kind,id,now]);
        audit(
            conn,
            kind,
            id,
            "extension.call",
            "success",
            &serde_json::json!({}),
        );
        return;
    }
    let error = result
        .as_ref()
        .err()
        .map(|value| value.chars().take(300).collect::<String>())
        .unwrap_or_default();
    let threshold = conn.query_row("SELECT failure_threshold FROM extension_governance WHERE extension_kind=?1 AND extension_id=?2", params![kind,id], |row| row.get::<_,i64>(0)).unwrap_or(DEFAULT_FAILURE_THRESHOLD);
    let cooldown = conn.query_row("SELECT cooldown_seconds FROM extension_governance WHERE extension_kind=?1 AND extension_id=?2", params![kind,id], |row| row.get::<_,i64>(0)).unwrap_or(DEFAULT_COOLDOWN_SECONDS);
    let _ = conn.execute(
        "UPDATE extension_governance SET consecutive_failures=consecutive_failures+1,
         circuit_open_until=CASE WHEN consecutive_failures+1>=?3 THEN ?4 ELSE circuit_open_until END,
         last_error=?5,updated_at=?6 WHERE extension_kind=?1 AND extension_id=?2",
        params![kind,id,threshold,now+cooldown,error,now]);
    audit(
        conn,
        kind,
        id,
        "extension.call",
        "failure",
        &serde_json::json!({"error":error}),
    );
}

pub fn mark_drifted(conn: &Connection, kind: &str, id: &str, actual_digest: &str) {
    let _ = conn.execute("UPDATE extension_governance SET verification_state='drifted',last_error='content digest changed',updated_at=?3 WHERE extension_kind=?1 AND extension_id=?2", params![kind,id,chrono::Utc::now().timestamp()]);
    audit(
        conn,
        kind,
        id,
        "extension.verify",
        "drifted",
        &serde_json::json!({"actual_digest":actual_digest}),
    );
}

fn validate_kind(kind: &str) -> Result<(), String> {
    matches!(kind, "skill" | "mcp" | "workflow")
        .then_some(())
        .ok_or_else(|| format!("未知扩展类型：{kind}"))
}

fn validate_source(source_uri: Option<&str>, source_revision: Option<&str>) -> Result<(), String> {
    for (label, value, limit) in [
        ("source_uri", source_uri, 2_048usize),
        ("source_revision", source_revision, 256usize),
    ] {
        if value.is_some_and(|value| value.len() > limit || value.chars().any(char::is_control)) {
            return Err(format!("扩展 {label} 非法或超过长度上限"));
        }
    }
    if let Some(uri) = source_uri.and_then(|value| url::Url::parse(value).ok()) {
        if !uri.username().is_empty() || uri.password().is_some() {
            return Err("扩展 source_uri 不能包含用户名或密码".into());
        }
    }
    Ok(())
}

fn audit(
    conn: &Connection,
    kind: &str,
    id: &str,
    action: &str,
    outcome: &str,
    details: &serde_json::Value,
) {
    let _ = crate::agent::enterprise::audit(
        conn,
        None,
        None,
        "system",
        action,
        &format!("{kind}:{id}"),
        outcome,
        details,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE skills(id TEXT,project_id TEXT,repo_host TEXT,repo_owner TEXT,repo_name TEXT,repo_branch TEXT,content_hash TEXT,installed_at INTEGER,updated_at INTEGER); CREATE TABLE mcp_servers(id TEXT,project_id TEXT,homepage TEXT,created_at INTEGER);").unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/072_extension_governance.sql"
        ))
        .unwrap();
        conn
    }

    #[test]
    fn verifies_detached_ed25519_and_rejects_tampering() {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let payload = b"extension-v1";
        let encoder = base64::engine::general_purpose::STANDARD;
        let attestation = ExtensionAttestation {
            algorithm: Some("ed25519".into()),
            signer_key_id: Some("publisher-1".into()),
            public_key_base64: Some(encoder.encode(pair.public_key().as_ref())),
            signature_base64: Some(encoder.encode(pair.sign(payload).as_ref())),
            ..Default::default()
        };
        assert_eq!(
            verify(payload, Some(&attestation)).unwrap().state,
            "verified"
        );
        assert!(verify(b"tampered", Some(&attestation)).is_err());
        let conn = db();
        assert!(register(
            &conn,
            "skill",
            "signed",
            Some("p"),
            b"tampered",
            Some(&attestation),
        )
        .is_err());
        let state: String = conn
            .query_row(
                "SELECT verification_state FROM extension_governance WHERE extension_kind='skill' AND extension_id='signed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "invalid");
    }

    #[test]
    fn rate_limit_and_circuit_breaker_fail_closed() {
        let conn = db();
        register(&conn, "skill", "s", Some("p"), b"body", None).unwrap();
        conn.execute("UPDATE extension_governance SET calls_per_minute=1,failure_threshold=2,cooldown_seconds=60 WHERE extension_id='s'",[]).unwrap();
        before_call(&conn, "skill", "s").unwrap();
        assert!(before_call(&conn, "skill", "s")
            .unwrap_err()
            .contains("调用限制"));
        record_result(&conn, "skill", "s", &Err("one".into()));
        record_result(&conn, "skill", "s", &Err("two".into()));
        assert!(before_call(&conn, "skill", "s")
            .unwrap_err()
            .contains("熔断"));
    }
}
