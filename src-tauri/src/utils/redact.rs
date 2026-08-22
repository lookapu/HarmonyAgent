//! 敏感信息文本脱敏（[57] 工具输出统一遮罩）
//!
//! 与 `meta_tools::redact`（JSON 键名级脱敏，share_session 导出用）互补：
//! 本模块处理**自由文本**——read_file 读 .env、命令输出、日志等非结构化内容，
//! 覆盖 meta_tools 按字段名匹配够不到的盲区。
//!
//! 应用点：`tools::run_tool` 出口统一包裹（含 MCP 工具），所有工具返回都过一遍。
//! 原则：只遮蔽「上下文明确」的敏感模式，普通代码/示例值尽量不误伤；
//! 邮箱/手机号/身份证保留部分明文结构（前几位 + 尾部），便于人工辨认。

use regex::Regex;
use std::sync::LazyLock;

/// 一条脱敏规则：命中即整体替换为 `to`
struct Rule {
    re: Regex,
    to: &'static str,
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    vec![
        // 私钥块（RSA/EC/OPENSSH/PGP 等）
        Rule {
            re: Regex::new(r"(?s)-----BEGIN [A-Z ]+PRIVATE KEY-----.*?-----END [A-Z ]+PRIVATE KEY-----").unwrap(),
            to: "[PRIVATE KEY REDACTED]",
        },
        // 签名证书块（证书链可能包含组织和设备身份；审计/分享默认隐藏）
        Rule {
            re: Regex::new(r"(?s)-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----").unwrap(),
            to: "[CERTIFICATE REDACTED]",
        },
        // JWT（三段式）
        Rule {
            re: Regex::new(r"eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{4,}").unwrap(),
            to: "[JWT REDACTED]",
        },
        // AWS AKIA
        Rule {
            re: Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(),
            to: "AKIA****************",
        },
        // sk- 前缀密钥（DeepSeek/OpenAI 风格）
        Rule {
            re: Regex::new(r"\bsk-[A-Za-z0-9_-]{16,}").unwrap(),
            to: "sk-***",
        },
        // GitHub 令牌
        Rule {
            re: Regex::new(r"\bgh[pousr]_[A-Za-z0-9]{20,}").unwrap(),
            to: "gh***",
        },
        // Bearer 令牌
        Rule {
            re: Regex::new(r"(?i)\bbearer\s+[A-Za-z0-9_\-\.]{8,}").unwrap(),
            to: "bearer ***",
        },
        // 密钥字段赋值（api_key=xxx / secret: xxx / token=xxx 等，值 ≥6 字符）
        Rule {
            re: Regex::new(r#"(?i)(api[_-]?key|apikey|secret[_-]?key|access[_-]?key|client[_-]?secret|app[_-]?secret|refresh[_-]?token|access[_-]?token|auth[_-]?token|authorization)(\s*[=:]\s*)([^,\s"'{}\[]{6,120})"#).unwrap(),
            to: "$1$2***",
        },
        // 密码字段赋值
        Rule {
            re: Regex::new(r#"(?i)(password|passwd|pwd)(\s*[=:]\s*)([^,\s"'{}\[]{4,120})"#).unwrap(),
            to: "$1$2***",
        },
        // 敏感环境变量、签名材料路径/口令与设备唯一标识
        Rule {
            re: Regex::new(r#"(?i)([A-Z0-9_]*(?:TOKEN|SECRET|PASSWORD|CREDENTIAL|PRIVATE_KEY)|keystore(?:_?file|_?path|_?password)?|store_?password|key_?password|certificate(?:_?file|_?path)?|provisioning_?profile)(\s*[=:]\s*)([^,\s"'{}\[]{4,500})"#).unwrap(),
            to: "$1$2***",
        },
        Rule {
            re: Regex::new(r#"(?i)(device_?id|device_?serial|serial_?number|udid)(\s*[=:]\s*)([^,\s"'{}]{6,160})"#).unwrap(),
            to: "$1$2[DEVICE ID REDACTED]",
        },
        // 带明文用户名/口令的连接 URL
        Rule {
            re: Regex::new(r"(?i)\b([a-z][a-z0-9+.-]*://[^\s:/@]+:)[^\s@/]+(@)").unwrap(),
            to: "$1***$2",
        },
    ]
});

/// 邮箱：保留首字符 + 域名（a***@example.com），部分遮蔽防爬取
fn mask_email(original: &str, caps: &regex::Captures) -> String {
    let full = &caps[0];
    if let Some(at) = full.rfind('@') {
        let (local, domain) = full.split_at(at);
        let head: String = local.chars().take(1).collect();
        // local part ≥3 字符时保留尾部 2 字符（a***bc@…）；≤2 字符只留头，
        // 否则 head+tail 重叠会拼回完整地址（a***a@… 仍含 a@…）
        let tail: String = if local.chars().count() >= 3 {
            local.chars().rev().take(2).collect::<Vec<_>>().into_iter().rev().collect()
        } else {
            String::new()
        };
        format!("{head}***{tail}{domain}")
    } else {
        original.to_string()
    }
}

/// 手机号：保留前 3 后 4（1[3-9]xxxxxxxxx → 1[3-9]x****xxxx）
fn mask_phone(original: &str) -> String {
    let chars: Vec<char> = original.chars().collect();
    if chars.len() == 11 {
        let head: String = chars[..3].iter().collect();
        let tail: String = chars[7..].iter().collect();
        format!("{head}****{tail}")
    } else {
        original.to_string()
    }
}

/// 身份证：保留前 6 后 4
fn mask_id_card(original: &str) -> String {
    let chars: Vec<char> = original.chars().collect();
    if chars.len() == 18 {
        let head: String = chars[..6].iter().collect();
        let tail: String = chars[14..].iter().collect();
        format!("{head}********{tail}")
    } else {
        original.to_string()
    }
}

/// 文本级脱敏入口：对自由文本应用全部规则
pub fn redact_text(input: &str) -> String {
    let mut out = input.to_string();
    for rule in RULES.iter() {
        out = rule.re.replace_all(&out, rule.to).into_owned();
    }
    // 邮箱（闭包替换，保留结构）
    let email_re = Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}").unwrap();
    out = email_re
        .replace_all(&out, |caps: &regex::Captures| mask_email(&caps[0], caps))
        .into_owned();
    // 手机号：独立 token（前后非数字），避免遮蔽年份/编号
    let phone_re = Regex::new(r"(^|[^\d])(1[3-9]\d{9})([^\d]|$)").unwrap();
    out = phone_re
        .replace_all(&out, |caps: &regex::Captures| {
            format!("{} {}{}", &caps[1], mask_phone(&caps[2]), &caps[3])
        })
        .into_owned();
    // 身份证：18 位数字串（前后非数字）
    let id_re = Regex::new(r"(^|[^\d])(\d{17}[\dXx])([^\d]|$)").unwrap();
    out = id_re
        .replace_all(&out, |caps: &regex::Captures| {
            format!("{} {}{}", &caps[1], mask_id_card(&caps[2]), &caps[3])
        })
        .into_owned();
    out
}

fn is_sensitive_field(key: &str) -> bool {
    let lower = key.to_lowercase().replace('-', "_");
    [
        "api_key", "apikey", "secret", "token", "password", "passwd", "pwd",
        "authorization", "credential", "private_key", "keystore", "store_password",
        "key_password", "certificate", "cert_path", "profile_path", "provisioning_profile",
        "device_id", "deviceid", "device_serial", "deviceserial", "serial_number",
        "serialnumber", "udid",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

/// JSON 级统一脱敏：按字段语义隐藏值，同时对自由文本叶子应用同一规则集。
pub fn redact_json_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(fields) => {
            let name_marks_value = fields
                .get("name")
                .and_then(|item| item.as_str())
                .is_some_and(is_sensitive_field);
            serde_json::Value::Object(fields.iter().map(|(key, value)| {
                let redacted = if is_sensitive_field(key) || (key == "value" && name_marks_value) {
                    serde_json::Value::String("***".into())
                } else {
                    redact_json_value(value)
                };
                (key.clone(), redacted)
            }).collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(redact_json_value).collect())
        }
        serde_json::Value::String(text) => serde_json::Value::String(redact_text(text)),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_key_field() {
        let out = redact_text("api_key=sk-abc1234567890abcdef, timeout=30");
        assert!(!out.contains("sk-abc1234567890"));
        assert!(out.contains("api_key=***"));
        assert!(out.contains("timeout=30"), "普通字段不应误伤: {out}");
    }

    #[test]
    fn test_redact_private_key_block() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA1234567890abcdef\n-----END RSA PRIVATE KEY-----";
        let out = redact_text(pem);
        assert!(out.contains("[PRIVATE KEY REDACTED]"));
        assert!(!out.contains("MIIEow"));
    }

    #[test]
    fn test_redact_jwt_and_bearer() {
        let out = redact_text("token: eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U");
        assert!(out.contains("[JWT REDACTED]"));
        let out2 = redact_text("Authorization: Bearer AbCdEf1234567890XyZ");
        assert!(out2.contains("bearer ***") || out2.contains("Bearer ***") || out2.contains("***"));
        assert!(!out2.contains("AbCdEf1234567890XyZ"));
    }

    #[test]
    fn test_redact_email_phone_id() {
        let out = redact_text("联系人 a@example.com 13812345678 110101199001011234");
        assert!(!out.contains("a@example.com"));
        assert!(out.contains("a***@example.com"), "邮箱应保留结构: {out}");
        assert!(out.contains("138****5678"), "手机应部分遮蔽: {out}");
        assert!(out.contains("110101********1234"), "身份证应部分遮蔽: {out}");
    }

    #[test]
    fn test_no_over_redact_normal_code() {
        // 普通代码/URL 不应被误伤
        let out = redact_text("const url = \"https://example.com/api/v1?page=2&size=20\";\nlet count = 13800138000 / 100;");
        assert!(out.contains("https://example.com/api/v1"), "URL 不应被遮: {out}");
    }

    #[test]
    fn test_redact_signing_env_device_and_connection_material() {
        let input = "HAP_SIGNING_TOKEN=very-secret-token\nkeystoreFile=/Users/me/release.p12\n\
device_id: ABCDEF0123456789\nDATABASE_URL=postgres://alice:plainpass@db.local/app\n\
-----BEGIN CERTIFICATE-----\nMIICSECRET\n-----END CERTIFICATE-----";
        let out = redact_text(input);
        for secret in [
            "very-secret-token", "/Users/me/release.p12", "ABCDEF0123456789",
            "plainpass", "MIICSECRET",
        ] {
            assert!(!out.contains(secret), "仍包含敏感值 {secret}: {out}");
        }
        assert!(out.contains("[DEVICE ID REDACTED]"));
        assert!(out.contains("[CERTIFICATE REDACTED]"));
    }

    #[test]
    fn test_json_redaction_uses_keys_and_name_value_pairs() {
        let input = serde_json::json!({
            "env": {"PATH": "/usr/bin", "SIGNING_PASSWORD": "open-sesame"},
            "deviceSerial": "DEVICE-123456",
            "headers": [{"name": "Authorization", "value": "Bearer abcdefghijklmnop"}],
            "message": "api_key=abcdefghijklmnop"
        });
        let out = redact_json_value(&input);
        assert_eq!(out["env"]["PATH"], "/usr/bin");
        assert_eq!(out["env"]["SIGNING_PASSWORD"], "***");
        assert_eq!(out["deviceSerial"], "***");
        assert_eq!(out["headers"][0]["value"], "***");
        assert!(!out["message"].as_str().unwrap().contains("abcdefghijklmnop"));
    }
}
