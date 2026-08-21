use std::path::PathBuf;

use serde::Deserialize;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::db::{
    models::{Skill, SkillUsageEvent, SkillUsageStat},
    queries, DbState,
};

#[derive(Debug, Deserialize)]
pub struct ImportSkillInput {
    /// Git 仓库地址：支持 GitHub/Gitee——https://github.com/owner/name、git@github.com:owner/name.git、
    /// owner/name（缺省 github）、https://gitee.com/owner/name、git@gitee.com:owner/name.git
    pub repo: String,
    /// 分支（可选，缺省使用仓库默认分支）
    pub branch: Option<String>,
    /// 技能在仓库内的子目录（可选；如 anthropics/skills 的 skills/docx，缺省取仓库根）
    pub subdir: Option<String>,
    /// 是否使用系统代理克隆（缺省 false）
    pub use_proxy: Option<bool>,
    /// 作用域：None=用户级(全局)；Some=仅该项目生效
    #[serde(default)]
    pub project_id: Option<String>,
}

/// 解析 Git 仓库地址 -> (host, owner, name)，支持 GitHub/Gitee：
/// https://github.com/owner/name、git@github.com:owner/name.git、owner/name（缺省 github）；
/// gitee 同理（https://gitee.com/owner/name、git@gitee.com:owner/name.git）。
fn parse_git_repo(repo: &str) -> Result<(String, String, String), String> {
    let trimmed = repo.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("仓库地址不能为空".into());
    }
    let (host, rest) = if let Some(rest) = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("http://github.com/"))
        .or_else(|| trimmed.strip_prefix("git@github.com:"))
    {
        ("github".to_string(), rest)
    } else if let Some(rest) = trimmed
        .strip_prefix("https://gitee.com/")
        .or_else(|| trimmed.strip_prefix("http://gitee.com/"))
        .or_else(|| trimmed.strip_prefix("git@gitee.com:"))
    {
        ("gitee".to_string(), rest)
    } else {
        // 无平台前缀（owner/name 简写）：默认 GitHub
        ("github".to_string(), trimmed)
    };
    let mut parts = rest.split('/').filter(|s| !s.is_empty());
    let owner = parts.next().ok_or_else(|| format!("无法解析仓库地址: {repo}"))?.to_string();
    let name = parts
        .next()
        .ok_or_else(|| format!("无法解析仓库地址: {repo}"))?
        .trim_end_matches(".git")
        .to_string();
    if owner.is_empty() || name.is_empty() {
        return Err(format!("无法解析仓库地址: {repo}"));
    }
    Ok((host, owner, name))
}

/// 解析 SKILL.md frontmatter 中的 name / description（支持引号、单行与多行折叠描述）
fn parse_skill_meta(content: &str) -> (Option<String>, Option<String>) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, None);
    }
    let body = &trimmed[3..];
    let end = body.find("\n---").unwrap_or(0);
    if end == 0 {
        return (None, None);
    }
    let front = &body[..end];
    let mut name = None;
    let mut desc: Option<Vec<String>> = None;
    let mut i = 0;
    let lines: Vec<&str> = front.lines().collect();
    while i < lines.len() {
        let line = lines[i].trim();
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
        } else if let Some(v) = line.strip_prefix("description:") {
            let first = v.trim().trim_matches('"').trim_matches('\'').to_string();
            // 多行描述：后续缩进行继续追加（YAML 常见写法）
            let mut collected = vec![first];
            i += 1;
            while i < lines.len() {
                let next = lines[i];
                if next.starts_with(' ') || next.starts_with('\t') {
                    let part = next.trim().trim_matches('"').trim_matches('\'');
                    if !part.is_empty() {
                        collected.push(part.to_string());
                    }
                    i += 1;
                } else {
                    break;
                }
            }
            desc = Some(collected);
            continue;
        }
        i += 1;
    }
    let desc = desc.map(|parts| {
        parts
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    });
    (name, desc.filter(|d| !d.is_empty()))
}

/// 从 GitHub/Gitee 导入 Skill：
/// git clone 到数据目录 skills/{host}__{owner}__{name}，读取 SKILL.md 元信息后入库。
#[tauri::command]
pub async fn import_skill_from_github(
    app: AppHandle,
    db: State<'_, DbState>,
    input: ImportSkillInput,
) -> Result<Skill, String> {
    let (host, owner, name) = parse_git_repo(&input.repo)?;
    let branch = input.branch.unwrap_or_default();
    let branch = branch.trim();

    let skills_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("skills");
    std::fs::create_dir_all(&skills_dir).map_err(|e| format!("创建 skills 目录失败: {e}"))?;

    let dest = skills_dir.join(format!("{host}__{owner}__{name}"));
    // 已存在则清空后重新拉取（保证与远程一致）
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| format!("清理旧目录失败: {e}"))?;
    }

    let url = format!("https://{host}.com/{owner}/{name}.git");
    let mut args = vec!["clone".to_string(), "--depth".to_string(), "1".to_string()];
    if !branch.is_empty() {
        args.push("--branch".to_string());
        args.push(branch.to_string());
    }
    args.push(url.clone());
    args.push(dest.to_string_lossy().to_string());
    let mut cmd = crate::utils::process::command("git", &args)?;
    // 走系统代理时注入 HTTPS_PROXY/HTTP_PROXY 环境变量（git 会优先读取）
    if input.use_proxy.unwrap_or(false) {
        if let Some(proxy) = crate::utils::net::read_system_proxy() {
            cmd.env("HTTPS_PROXY", &proxy).env("HTTP_PROXY", &proxy);
        }
    }
    let output = cmd
        .output()
        .await
        .map_err(|e| format!("git clone 执行失败: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git clone 失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    // 解析实际分支（未指定时使用仓库默认分支）
    let actual_branch = if branch.is_empty() {
        let args = vec![
            "-C".to_string(),
            dest.to_string_lossy().to_string(),
            "rev-parse".to_string(),
            "--abbrev-ref".to_string(),
            "HEAD".to_string(),
        ];
        let out = tokio::task::spawn_blocking(move || {
            crate::utils::process::output_blocking("git", &args)
        })
        .await
        .map_err(|e| format!("git rev-parse 任务失败: {e}"))?
        .ok();
        out.and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "main".to_string())
    } else {
        branch.to_string()
    };

    // 技能实际所在目录：仓库根或仓库内子目录（如 skills/docx）
    let subdir = input.subdir.unwrap_or_default().trim().trim_matches('/').to_string();
    let mut skill_root = if subdir.is_empty() {
        dest.clone()
    } else {
        dest.join(&subdir)
    };
    if !skill_root.is_dir() {
        // 容错：子目录名大小写不敏感匹配（如 Coding vs coding）
        let fuzzy = std::fs::read_dir(&dest).ok().and_then(|entries| {
            entries
                .filter_map(|e| e.ok())
                .find(|e| {
                    e.file_name().to_string_lossy().eq_ignore_ascii_case(&subdir)
                        && e.path().is_dir()
                })
                .map(|e| e.path())
        });
        if let Some(p) = fuzzy {
            skill_root = p;
        } else {
            return Err(format!("仓库内未找到子目录 {subdir}（请确认路径）"));
        }
    }

    // 读取 SKILL.md（兼容 SKILL.md 大小写变体）
    let mut skill_md_path: Option<PathBuf> = None;
    for candidate in ["SKILL.md", "skill.md"] {
        let p = skill_root.join(candidate);
        if p.exists() {
            skill_md_path = Some(p);
            break;
        }
    }
    let skill_md_path = skill_md_path.ok_or_else(|| {
        format!("技能目录缺少 SKILL.md：{}", skill_root.display())
    })?;
    let skill_content = std::fs::read_to_string(&skill_md_path)
        .map_err(|e| format!("读取 SKILL.md 失败 {}：{e}", skill_md_path.display()))?;
    let (meta_name, meta_desc) = parse_skill_meta(&skill_content);
    let manifest = crate::services::skill_manifest::parse_and_validate(&skill_content)?;

    let skill_name = meta_name.unwrap_or_else(|| name.clone());
    let description = meta_desc.or_else(|| Some(format!("{host}/{owner}/{name} 仓库中安装的 Skill")));
    let directory = skill_root.to_string_lossy().to_string();

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();

    // 已存在同一仓库同一子目录的 Skill：更新目录/描述/时间（按平台+仓库查重）
    let existing = queries::find_skill_by_repo(&conn, &host, &owner, &name, &subdir, input.project_id.as_deref()).map_err(|e| e.to_string())?;
    let skill = if let Some(mut s) = existing {
        s.name = skill_name;
        s.description = description;
        s.directory = Some(directory);
        s.repo_host = Some(host.clone());
        s.repo_branch = actual_branch.clone();
        s.content_hash = Some(manifest.content_hash.clone());
        s.manifest_schema = manifest.schema;
        s.skill_version = manifest.version.clone();
        s.agent_compat = manifest.agent_compat.clone();
        s.permissions_json = serde_json::to_string(&manifest.permissions).map_err(|e| e.to_string())?;
        s.compatibility_status = manifest.compatibility_status.clone();
        s.enabled = manifest.compatibility_status != "incompatible";
        s.updated_at = Some(now);
        queries::update_skill(&conn, &s).map_err(|e| e.to_string())?;
        s
    } else {
        let s = Skill {
            id: Uuid::new_v4().to_string(),
            name: skill_name,
            description,
            directory: Some(directory),
            repo_owner: Some(owner.clone()),
            repo_name: Some(name.clone()),
            repo_host: Some(host.clone()),
            repo_branch: actual_branch.clone(),
            subdir: if subdir.is_empty() { None } else { Some(subdir.clone()) },
            enabled: manifest.compatibility_status != "incompatible",
            content_hash: Some(manifest.content_hash.clone()),
            manifest_schema: manifest.schema,
            skill_version: manifest.version.clone(),
            agent_compat: manifest.agent_compat.clone(),
            permissions_json: serde_json::to_string(&manifest.permissions).map_err(|e| e.to_string())?,
            compatibility_status: manifest.compatibility_status.clone(),
            installed_at: now,
            updated_at: None,
            project_id: input.project_id.clone(),
        };
        queries::insert_skill(&conn, &s).map_err(|e| e.to_string())?;
        s
    };

    // 记录到 skill_repos（幂等，按平台+仓库）
    conn.execute(
        "INSERT OR REPLACE INTO skill_repos (host, owner, name, branch, enabled) VALUES (?1, ?2, ?3, ?4, 1)",
        rusqlite::params![host, owner, name, actual_branch],
    )
    .map_err(|e| e.to_string())?;

    Ok(skill)
}

/// 删除 Skill：移除数据库记录；若磁盘目录（仓库根/子目录）不再被其他 Skill 引用则一并清理。
#[tauri::command]
pub fn remove_skill(app: AppHandle, db: State<DbState>, id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let skill = queries::get_skill(&conn, &id).map_err(|e| e.to_string())?;
    let directory = skill.directory.clone();
    queries::delete_skill(&conn, &id).map_err(|e| e.to_string())?;

    // 目录清理：仅当没有其他 Skill 引用同一目录时才删除，避免误删共享仓库
    if let Some(dir) = directory {
        let still_used = queries::skill_directory_in_use(&conn, &dir).map_err(|e| e.to_string())?;
        if !still_used {
            // 子目录技能删子目录；仓库根技能删整个 {owner}__{name} 目录
            let dir_path = std::path::Path::new(&dir);
            if dir_path.is_dir() {
                let _ = std::fs::remove_dir_all(dir_path);
            }
            // 仓库根目录若已空，也顺手清理（{host}__{owner}__{name}，兼容旧格式 {owner}__{name}）
            if let (Some(owner), Some(name)) = (&skill.repo_owner, &skill.repo_name) {
                let skills_dir = app
                    .path()
                    .app_data_dir()
                    .map_err(|e| e.to_string())?
                    .join("skills");
                let host = skill.repo_host.as_deref().unwrap_or("github");
                let repo_dir = skills_dir.join(format!("{host}__{owner}__{name}"));
                let repo_dir = if repo_dir.is_dir() {
                    repo_dir
                } else {
                    skills_dir.join(format!("{owner}__{name}"))
                };
                let empty = std::fs::read_dir(&repo_dir)
                    .map(|mut it| it.next().is_none())
                    .unwrap_or(false);
                if empty {
                    let _ = std::fs::remove_dir_all(&repo_dir);
                }
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn list_skills(db: State<DbState>, project_id: Option<String>) -> Result<Vec<Skill>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::list_skills(&conn, project_id.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn toggle_skill(db: State<DbState>, id: String, enabled: bool) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    if enabled {
        let skill = queries::get_skill(&conn, &id).map_err(|e| e.to_string())?;
        if skill.compatibility_status == "incompatible" {
            return Err(format!(
                "Skill「{}」与当前 HarmonyAgent {} 不兼容，不能启用",
                skill.name,
                env!("CARGO_PKG_VERSION")
            ));
        }
    }
    conn.execute(
        "UPDATE skills SET enabled = ?2 WHERE id = ?1",
        rusqlite::params![id, enabled],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 把一个技能复制到另一作用域（全局↔当前项目）。磁盘上的 SKILL.md 等文件已在共享目录，
/// 这里只复制数据库记录（directory 指向同一份文件），不重复 clone 仓库。
/// 目标作用域已存在同 repo+subdir 的技能时返回错误。
#[tauri::command]
pub fn clone_skill(
    db: State<DbState>,
    id: String,
    target_project_id: Option<String>,
) -> Result<Skill, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut src = queries::get_skill(&conn, &id).map_err(|e| e.to_string())?;

    if src.project_id == target_project_id {
        return Err("目标作用域与源技能相同".to_string());
    }

    let subdir = src.subdir.clone().unwrap_or_default();
    if let (Some(owner), Some(name)) = (src.repo_owner.as_deref(), src.repo_name.as_deref()) {
        let host = src.repo_host.as_deref().unwrap_or("github");
        if queries::find_skill_by_repo(&conn, host, owner, name, &subdir, target_project_id.as_deref())
            .map_err(|e| e.to_string())?
            .is_some()
        {
            return Err(format!("目标作用域已存在同名技能「{}」", src.name));
        }
    }

    src.id = Uuid::new_v4().to_string();
    src.project_id = target_project_id;
    src.enabled = true;
    src.installed_at = chrono::Utc::now().timestamp();
    src.updated_at = None;
    queries::insert_skill(&conn, &src).map_err(|e| e.to_string())?;
    Ok(src)
}

/// Skill 调用统计（按技能聚合：次数 / 最近调用时间）。project_id 空 = 全部项目。
#[tauri::command]
pub fn list_skill_usage(
    db: State<DbState>,
    project_id: Option<String>,
) -> Result<Vec<SkillUsageStat>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    queries::list_skill_usage_stats(&conn, project_id.as_deref().filter(|s| !s.is_empty()))
        .map_err(|e| e.to_string())
}

/// 最近 Skill 调用明细（时间线，limit 缺省 100）。project_id 空 = 全部项目。
#[tauri::command]
pub fn list_skill_usage_events(
    db: State<DbState>,
    project_id: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<SkillUsageEvent>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let limit = limit.unwrap_or(100).clamp(1, 500);
    queries::list_skill_usage_events(&conn, project_id.as_deref().filter(|s| !s.is_empty()), limit)
        .map_err(|e| e.to_string())
}
