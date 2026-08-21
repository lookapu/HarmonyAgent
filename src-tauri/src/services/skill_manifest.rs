//! HarmonyAgent Skill manifest v1 parser and compatibility validator.

use sha2::{Digest, Sha256};

pub const MANIFEST_SCHEMA_VERSION: i64 = 1;
pub const KNOWN_PERMISSIONS: &[&str] = &[
    "project.read",
    "project.write",
    "process.exec",
    "network.read",
    "network.write",
    "device.read",
    "device.write",
    "secrets.read",
    "release.publish",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillManifest {
    pub schema: i64,
    pub version: String,
    pub agent_compat: Option<String>,
    pub permissions: Vec<String>,
    pub compatibility_status: String,
    pub content_hash: String,
}

pub fn parse_and_validate(content: &str) -> Result<SkillManifest, String> {
    let content_hash = format!("sha256:{:x}", Sha256::digest(content.as_bytes()));
    let Some(frontmatter) = frontmatter(content) else {
        return Ok(legacy(content_hash));
    };
    let schema = scalar(frontmatter, "harmony_agent_schema");
    let version = scalar(frontmatter, "version");
    let agent_compat = scalar(frontmatter, "harmony_agent_compat");
    let permissions = list(frontmatter, "permissions");
    // 只由 HarmonyAgent 命名空间字段激活 v1，避免把外部 Skill 自有的 version/permissions
    // 误判成半份 HarmonyAgent 清单。
    let declared = schema.is_some() || agent_compat.is_some();
    if !declared {
        return Ok(legacy(content_hash));
    }
    let schema = schema
        .ok_or("Skill v1 清单缺少 harmony_agent_schema")?
        .parse::<i64>()
        .map_err(|_| "harmony_agent_schema 必须是整数".to_string())?;
    if schema != MANIFEST_SCHEMA_VERSION {
        return Err(format!(
            "不支持的 Skill 清单版本 {schema}；当前支持 {MANIFEST_SCHEMA_VERSION}"
        ));
    }
    let version = version.ok_or("Skill v1 清单缺少 version")?;
    parse_version(&version).ok_or_else(|| format!("Skill version 不是合法 SemVer：{version}"))?;
    let agent_compat = agent_compat.ok_or("Skill v1 清单缺少 harmony_agent_compat")?;
    let permissions = permissions.ok_or("Skill v1 清单缺少 permissions（无额外权限时写 []）")?;
    let mut normalized = Vec::new();
    for permission in permissions {
        if !KNOWN_PERMISSIONS.contains(&permission.as_str()) {
            return Err(format!(
                "未知 Skill 权限 {permission}；允许值：{}",
                KNOWN_PERMISSIONS.join(", ")
            ));
        }
        if !normalized.contains(&permission) {
            normalized.push(permission);
        }
    }
    normalized.sort();
    let compatible = requirement_matches(&agent_compat, env!("CARGO_PKG_VERSION"))?;
    Ok(SkillManifest {
        schema,
        version,
        agent_compat: Some(agent_compat),
        permissions: normalized,
        compatibility_status: if compatible {
            "compatible"
        } else {
            "incompatible"
        }
        .into(),
        content_hash,
    })
}

fn legacy(content_hash: String) -> SkillManifest {
    SkillManifest {
        schema: 0,
        version: "0.0.0".into(),
        agent_compat: None,
        permissions: Vec::new(),
        compatibility_status: "legacy_unverified".into(),
        content_hash,
    }
}

fn frontmatter(content: &str) -> Option<&str> {
    let trimmed = content.trim_start();
    let body = trimmed.strip_prefix("---")?;
    let end = body.find("\n---")?;
    Some(&body[..end])
}

fn scalar(frontmatter: &str, key: &str) -> Option<String> {
    frontmatter
        .lines()
        .find_map(|line| {
            let (candidate, value) = line.trim().split_once(':')?;
            (candidate.trim() == key).then(|| unquote(value.trim()).to_string())
        })
        .filter(|value| !value.is_empty())
}

fn list(frontmatter: &str, key: &str) -> Option<Vec<String>> {
    let lines: Vec<&str> = frontmatter.lines().collect();
    let index = lines.iter().position(|line| {
        line.trim()
            .split_once(':')
            .is_some_and(|(candidate, _)| candidate.trim() == key)
    })?;
    let (_, inline) = lines[index].trim().split_once(':')?;
    let inline = inline.trim();
    if inline.starts_with('[') && inline.ends_with(']') {
        let inner = &inline[1..inline.len() - 1];
        return Some(
            inner
                .split(',')
                .map(|value| unquote(value.trim()).to_string())
                .filter(|value| !value.is_empty())
                .collect(),
        );
    }
    let mut values = Vec::new();
    for line in lines.iter().skip(index + 1) {
        if !line.starts_with(' ') && !line.starts_with('\t') {
            break;
        }
        if let Some(value) = line.trim().strip_prefix('-') {
            let value = unquote(value.trim());
            if !value.is_empty() {
                values.push(value.to_string());
            }
        }
    }
    Some(values)
}

fn unquote(value: &str) -> &str {
    value.trim_matches(|ch| matches!(ch, '\'' | '"'))
}

fn parse_version(value: &str) -> Option<(u64, u64, u64)> {
    let core = value
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

fn requirement_matches(requirement: &str, current: &str) -> Result<bool, String> {
    let current =
        parse_version(current).ok_or_else(|| format!("应用版本不是合法 SemVer：{current}"))?;
    let mut saw_constraint = false;
    for raw in requirement
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        saw_constraint = true;
        if let Some(version) = raw.strip_prefix('^') {
            let expected = parse_version(version)
                .ok_or_else(|| format!("无效 harmony_agent_compat 约束：{raw}"))?;
            if current < expected || current.0 != expected.0 {
                return Ok(false);
            }
            continue;
        }
        let (operator, version) = [">=", "<=", ">", "<", "="]
            .iter()
            .find_map(|operator| {
                raw.strip_prefix(operator)
                    .map(|version| (*operator, version))
            })
            .unwrap_or(("=", raw));
        let expected = parse_version(version.trim())
            .ok_or_else(|| format!("无效 harmony_agent_compat 约束：{raw}"))?;
        let matches = match operator {
            ">=" => current >= expected,
            "<=" => current <= expected,
            ">" => current > expected,
            "<" => current < expected,
            _ => current == expected,
        };
        if !matches {
            return Ok(false);
        }
    }
    if !saw_constraint {
        return Err("harmony_agent_compat 不能为空".into());
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_manifest_parses_permissions_and_compatibility() {
        let manifest = parse_and_validate("---\nname: harmony-build\ndescription: Build safely\nharmony_agent_schema: 1\nversion: 1.2.3\nharmony_agent_compat: \">=2.0.0,<3.0.0\"\npermissions: [project.read, project.write, process.exec]\n---\n# Instructions\n").unwrap();
        assert_eq!(manifest.schema, 1);
        assert_eq!(manifest.version, "1.2.3");
        assert_eq!(manifest.compatibility_status, "compatible");
        assert_eq!(
            manifest.permissions,
            ["process.exec", "project.read", "project.write"]
        );
        assert!(manifest.content_hash.starts_with("sha256:"));
    }

    #[test]
    fn legacy_is_explicit_and_declared_manifest_fails_closed() {
        let legacy = parse_and_validate("# Old skill").unwrap();
        assert_eq!(legacy.compatibility_status, "legacy_unverified");
        let external = parse_and_validate(
            "---\nname: external\nversion: 9.0.0\npermissions: [vendor-specific]\n---",
        )
        .unwrap();
        assert_eq!(external.compatibility_status, "legacy_unverified");
        let unknown = parse_and_validate("---\nharmony_agent_schema: 1\nversion: 1.0.0\nharmony_agent_compat: ^2.0.0\npermissions:\n  - root.everything\n---").unwrap_err();
        assert!(unknown.contains("未知 Skill 权限"));
        let incompatible = parse_and_validate("---\nharmony_agent_schema: 1\nversion: 1.0.0\nharmony_agent_compat: \">=3.0.0\"\npermissions: []\n---").unwrap();
        assert_eq!(incompatible.compatibility_status, "incompatible");
    }
}
