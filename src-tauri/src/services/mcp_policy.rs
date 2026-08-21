//! EC09 project-scoped MCP authorization and call-time policy enforcement.

use crate::db::models::McpServer;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

pub const NETWORK_DENY: &str = "deny";
pub const NETWORK_ALLOW: &str = "allow";

pub fn parse_list(raw: &str, field: &str) -> Result<Vec<String>, String> {
    let values: Vec<String> = serde_json::from_str(raw)
        .map_err(|error| format!("MCP {field} 不是字符串数组：{error}"))?;
    let mut out = Vec::new();
    for value in values {
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("MCP {field} 不能包含空值"));
        }
        if !out.iter().any(|item| item == value) {
            out.push(value.to_string());
        }
    }
    Ok(out)
}

pub fn validate_authorization(
    allowed_tools: &[String],
    allowed_roots: &[String],
    network_policy: &str,
    credential_keys: &[String],
) -> Result<(), String> {
    if allowed_tools.is_empty() {
        return Err("MCP 授权至少需要一个 allowed tool".into());
    }
    for tool in allowed_tools {
        if tool == "*" || tool.contains("__") || tool.chars().any(char::is_whitespace) {
            return Err(format!("MCP 工具名不允许通配、空白或协议分隔符：{tool}"));
        }
    }
    if allowed_roots.is_empty() {
        return Err("MCP 授权至少需要一个项目内 allowed root".into());
    }
    for root in allowed_roots {
        let path = Path::new(root);
        if path.is_absolute()
            || path.components().any(|part| {
                matches!(
                    part,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(format!(
                "MCP allowed root 必须是无 .. 的项目相对路径：{root}"
            ));
        }
    }
    if !matches!(network_policy, NETWORK_DENY | NETWORK_ALLOW) {
        return Err(format!(
            "MCP network policy 仅支持 deny|allow：{network_policy}"
        ));
    }
    for key in credential_keys {
        if key.is_empty()
            || !key
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            return Err(format!("MCP credential key 非法：{key}"));
        }
    }
    Ok(())
}

pub fn validate_server_command(server: &McpServer) -> Result<(), String> {
    let command: Vec<String> = serde_json::from_str(&server.command)
        .map_err(|error| format!("MCP command 格式错误：{error}"))?;
    let sensitive_flags = [
        "--token",
        "--password",
        "--passwd",
        "--secret",
        "--api-key",
        "--apikey",
        "--authorization",
    ];
    for (index, argument) in command.iter().enumerate() {
        let lower = argument.to_ascii_lowercase();
        if sensitive_flags.iter().any(|flag| {
            lower == *flag
                && command
                    .get(index + 1)
                    .is_some_and(|value| !value.trim().is_empty())
                || lower.starts_with(&format!("{flag}="))
        }) || (lower.contains("://")
            && lower.split_once("://").is_some_and(|(_, rest)| {
                rest.split('/')
                    .next()
                    .is_some_and(|authority| authority.contains('@'))
            }))
        {
            return Err("MCP command 不得内嵌凭据；请改用 env 并在项目授权中列出变量名".into());
        }
    }
    Ok(())
}

pub fn ensure_server_authorized(server: &McpServer, project_id: &str) -> Result<(), String> {
    if server.project_id.as_deref() != Some(project_id) {
        return Err("MCP 服务器未绑定当前项目；全局配置需先克隆到项目并授权".into());
    }
    if server.authorization_state != "configured" {
        return Err("MCP 服务器尚未配置项目授权".into());
    }
    validate_authorization(
        &parse_list(&server.allowed_tools, "allowed_tools")?,
        &parse_list(&server.allowed_roots, "allowed_roots")?,
        &server.network_policy,
        &parse_list(&server.credential_keys, "credential_keys")?,
    )
}

pub fn tool_allowed(server: &McpServer, tool: &str) -> Result<bool, String> {
    Ok(parse_list(&server.allowed_tools, "allowed_tools")?
        .iter()
        .any(|allowed| allowed == tool))
}

pub fn validate_call(
    server: &McpServer,
    project_id: &str,
    project_root: &Path,
    tool: &str,
    args: &Value,
) -> Result<(), String> {
    ensure_server_authorized(server, project_id)?;
    if !tool_allowed(server, tool)? {
        return Err(format!("MCP 工具 {tool} 不在当前项目授权清单"));
    }
    if server.network_policy == NETWORK_DENY && is_network_tool(tool) {
        return Err(format!(
            "MCP 当前项目网络策略为 deny，拒绝网络语义工具 {tool}"
        ));
    }
    let roots = parse_list(&server.allowed_roots, "allowed_roots")?
        .into_iter()
        .map(|root| resolve_path(&project_root.join(root)))
        .collect::<Vec<_>>();
    validate_values(
        args,
        None,
        project_root,
        &roots,
        server.network_policy == NETWORK_ALLOW,
    )
}

fn validate_values(
    value: &Value,
    key: Option<&str>,
    project_root: &Path,
    roots: &[PathBuf],
    network_allowed: bool,
) -> Result<(), String> {
    match value {
        Value::Object(fields) => {
            for (field, value) in fields {
                validate_values(value, Some(field), project_root, roots, network_allowed)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                validate_values(item, key, project_root, roots, network_allowed)?;
            }
        }
        Value::String(text) => {
            if !network_allowed && is_network_value(key, text) {
                return Err("MCP 当前项目网络策略为 deny，本次调用包含网络地址".into());
            }
            if key.is_some_and(is_path_key) {
                let candidate = Path::new(text);
                let resolved = if candidate.is_absolute() {
                    resolve_path(candidate)
                } else {
                    resolve_path(&project_root.join(candidate))
                };
                if !roots.iter().any(|root| resolved.starts_with(root)) {
                    return Err(format!("MCP 路径参数越过项目授权目录：{text}"));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn is_path_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "path",
        "paths",
        "file",
        "files",
        "folder",
        "folders",
        "directory",
        "directories",
        "root",
        "roots",
        "cwd",
        "workspace",
        "workspaces",
    ]
    .iter()
    .any(|part| key == *part || key.ends_with(&format!("_{part}")))
}

fn is_network_tool(tool: &str) -> bool {
    let tool = tool.to_ascii_lowercase();
    [
        "http", "web", "fetch", "download", "upload", "request", "remote", "browser", "url",
        "github", "gitlab", "slack",
    ]
    .iter()
    .any(|part| tool.contains(part))
}

fn is_network_value(key: Option<&str>, value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.starts_with("http://")
        || value.starts_with("https://")
        || key.is_some_and(|key| {
            let key = key.to_ascii_lowercase();
            matches!(
                key.as_str(),
                "url" | "uri" | "endpoint" | "host" | "hostname"
            )
        })
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn resolve_path(path: &Path) -> PathBuf {
    let normalized = normalize(path);
    if let Ok(canonical) = std::fs::canonicalize(&normalized) {
        return canonical;
    }
    let mut ancestor = normalized.clone();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name().map(|name| name.to_os_string()) else {
            return normalized;
        };
        missing.push(name);
        if !ancestor.pop() {
            return normalized;
        }
    }
    let mut resolved = std::fs::canonicalize(&ancestor).unwrap_or(ancestor);
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    resolved
}

pub fn configure_child_environment(
    cmd: &mut tokio::process::Command,
    server: &McpServer,
    program: &str,
) -> Result<(), String> {
    cmd.env_clear();
    for key in [
        "PATH",
        "HOME",
        "TMPDIR",
        "LANG",
        "LC_ALL",
        "SystemRoot",
        "WINDIR",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
        "ComSpec",
        "PATHEXT",
    ] {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    crate::utils::process::apply_mcp_child_env(cmd, program, &server.id)?;
    let allowed_credentials = parse_list(&server.credential_keys, "credential_keys")?;
    let env: serde_json::Map<String, Value> =
        serde_json::from_str(&server.env).map_err(|error| format!("MCP env 格式错误：{error}"))?;
    let mut passed_keys = std::collections::HashSet::new();
    for (key, value) in &env {
        let Some(value) = value.as_str() else {
            continue;
        };
        if !allowed_credentials.iter().any(|allowed| allowed == key) {
            continue;
        }
        if server.network_policy == NETWORK_DENY && is_proxy_key(key) {
            continue;
        }
        cmd.env(key, value);
        passed_keys.insert(key.as_str());
    }
    cmd.env("HARMONY_AGENT_NETWORK_POLICY", &server.network_policy);
    if server.network_policy == NETWORK_DENY {
        cmd.env("npm_config_offline", "true");
    }
    if server.network_policy == NETWORK_ALLOW && !passed_keys.iter().any(|key| is_proxy_key(key)) {
        if let Some(proxy) = crate::utils::net::read_system_proxy() {
            for key in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
                cmd.env(key, &proxy);
            }
        }
    }
    Ok(())
}

/// MCP 页面“测试连接”是用户显式诊断，不等同于 Agent 授权。它仍清空宿主进程环境，
/// 但允许测试该配置自己声明的 env 与网络，以便在授权前完成握手和工具发现。
pub fn configure_test_environment(
    cmd: &mut tokio::process::Command,
    server: &McpServer,
    program: &str,
) -> Result<(), String> {
    let env: serde_json::Map<String, Value> =
        serde_json::from_str(&server.env).map_err(|error| format!("MCP env 格式错误：{error}"))?;
    let mut test_server = server.clone();
    test_server.credential_keys = serde_json::to_string(&env.keys().collect::<Vec<_>>())
        .map_err(|error| error.to_string())?;
    test_server.network_policy = NETWORK_ALLOW.into();
    configure_child_environment(cmd, &test_server, program)
}

fn is_proxy_key(key: &str) -> bool {
    matches!(
        key,
        "HTTP_PROXY" | "HTTPS_PROXY" | "http_proxy" | "https_proxy"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> McpServer {
        McpServer {
            id: "m".into(),
            name: "local".into(),
            server_type: "local".into(),
            command: "[]".into(),
            args: "[]".into(),
            env: "{}".into(),
            enabled: true,
            description: None,
            homepage: None,
            created_at: 0,
            last_test_ok: None,
            last_test_at: None,
            last_test_error: None,
            project_id: Some("p".into()),
            authorization_state: "configured".into(),
            allowed_tools: "[\"read\"]".into(),
            allowed_roots: "[\"src\"]".into(),
            network_policy: "deny".into(),
            credential_keys: "[]".into(),
        }
    }

    #[test]
    fn rejects_cross_project_tool_path_and_network() {
        let server = server();
        let root = Path::new("/workspace/project");
        validate_call(
            &server,
            "p",
            root,
            "read",
            &serde_json::json!({"path":"src/a.ets"}),
        )
        .unwrap();
        assert!(validate_call(&server, "other", root, "read", &Value::Null).is_err());
        assert!(validate_call(&server, "p", root, "write", &Value::Null).is_err());
        assert!(validate_call(
            &server,
            "p",
            root,
            "read",
            &serde_json::json!({"path":"../secret"})
        )
        .is_err());
        assert!(validate_call(&server, "p", root, "web_fetch", &Value::Null).is_err());
        assert!(validate_call(
            &server,
            "p",
            root,
            "read",
            &serde_json::json!({"url":"https://example.com"})
        )
        .is_err());
    }

    #[test]
    fn authorization_rejects_wildcards_and_external_roots() {
        assert!(validate_authorization(&["*".into()], &[".".into()], "deny", &[]).is_err());
        assert!(
            validate_authorization(&["read".into()], &["../outside".into()], "deny", &[]).is_err()
        );
        validate_authorization(
            &["read".into()],
            &[".".into()],
            "allow",
            &["API_TOKEN".into()],
        )
        .unwrap();
        let mut inline_secret = server();
        inline_secret.command = r#"["node","server.js","--token=secret"]"#.into();
        assert!(validate_server_command(&inline_secret).is_err());
    }

    #[test]
    fn child_environment_only_receives_explicit_server_keys() {
        let mut server = server();
        server.env = r#"{"API_TOKEN":"secret","ORDINARY_SETTING":"hidden"}"#.into();
        server.credential_keys = "[\"API_TOKEN\"]".into();
        let mut command = tokio::process::Command::new("mcp-test-program");
        configure_child_environment(&mut command, &server, "mcp-test-program").unwrap();
        let env = command
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| {
                Some((
                    key.to_string_lossy().into_owned(),
                    value?.to_string_lossy().into_owned(),
                ))
            })
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(env.get("API_TOKEN").map(String::as_str), Some("secret"));
        assert!(!env.contains_key("ORDINARY_SETTING"));
        assert_eq!(
            env.get("npm_config_offline").map(String::as_str),
            Some("true")
        );

        let mut test_command = tokio::process::Command::new("mcp-test-program");
        configure_test_environment(&mut test_command, &server, "mcp-test-program").unwrap();
        let test_env = test_command
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| {
                Some((
                    key.to_string_lossy().into_owned(),
                    value?.to_string_lossy().into_owned(),
                ))
            })
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            test_env.get("ORDINARY_SETTING").map(String::as_str),
            Some("hidden")
        );
        assert!(!test_env.contains_key("npm_config_offline"));
    }

    #[cfg(unix)]
    #[test]
    fn path_policy_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let base = std::env::temp_dir().join(format!("mcp-policy-{}", uuid::Uuid::new_v4()));
        let project = base.join("project");
        let outside = base.join("outside");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        symlink(&outside, project.join("src/link")).unwrap();
        let result = validate_call(
            &server(),
            "p",
            &project,
            "read",
            &serde_json::json!({"path":"src/link/secret.txt"}),
        );
        let _ = std::fs::remove_dir_all(base);
        assert!(result.is_err());
    }

    #[test]
    fn lookup_fails_closed_until_exact_project_authorization() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&conn).unwrap();
        let mut global = server();
        global.id = "global".into();
        global.project_id = None;
        global.authorization_state = "unconfigured".into();
        crate::db::queries::insert_mcp_server(&conn, &global).unwrap();
        let mut project = server();
        project.id = "project".into();
        project.authorization_state = "unconfigured".into();
        crate::db::queries::insert_mcp_server(&conn, &project).unwrap();
        assert!(
            crate::db::queries::find_mcp_instance_id(&conn, "local", Some("p"), 0)
                .unwrap()
                .is_none()
        );
        crate::db::queries::authorize_mcp_server(
            &conn,
            "project",
            "p",
            "[\"read\"]",
            "[\".\"]",
            "deny",
            "[]",
        )
        .unwrap();
        assert_eq!(
            crate::db::queries::find_mcp_instance_id(&conn, "local", Some("p"), 0)
                .unwrap()
                .as_deref(),
            Some("project")
        );
    }
}
