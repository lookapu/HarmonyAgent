use rusqlite::{params, Connection, OptionalExtension};
use crate::db::models::*;

pub fn list_providers(conn: &Connection) -> Result<Vec<Provider>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, name, provider_type, protocol, base_url, api_key, npm_package,
                is_active, in_failover_queue, priority, cost_multiplier,
                limit_daily_cny, limit_monthly_cny, settings_json, notes, icon,
                created_at, updated_at, endpoints_json
         FROM providers ORDER BY is_active DESC, priority ASC, name ASC"
    )?;

    let rows = stmt.query_map([], |row| {
        let endpoints: Vec<EndpointDef> =
            serde_json::from_str(&row.get::<_, String>(18).unwrap_or_else(|_| "[]".into())).unwrap_or_default();
        Ok(Provider {
            id: row.get(0)?,
            name: row.get(1)?,
            provider_type: row.get(2)?,
            protocol: row.get(3)?,
            base_url: row.get(4)?,
            endpoints,
            api_key: row.get(5)?,
            npm_package: row.get(6)?,
            is_active: row.get(7)?,
            in_failover_queue: row.get(8)?,
            priority: row.get(9)?,
            cost_multiplier: row.get(10)?,
            limit_daily_cny: row.get(11)?,
            limit_monthly_cny: row.get(12)?,
            settings_json: row.get(13)?,
            notes: row.get(14)?,
            icon: row.get(15)?,
            created_at: row.get(16)?,
            updated_at: row.get(17)?,
        })
    })?;

    rows.collect()
}

pub fn get_provider(conn: &Connection, id: &str) -> Result<Option<Provider>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, name, provider_type, protocol, base_url, api_key, npm_package,
                is_active, in_failover_queue, priority, cost_multiplier,
                limit_daily_cny, limit_monthly_cny, settings_json, notes, icon,
                created_at, updated_at, endpoints_json
         FROM providers WHERE id = ?1"
    )?;

    let mut rows = stmt.query_map(params![id], |row| {
        let endpoints: Vec<EndpointDef> =
            serde_json::from_str(&row.get::<_, String>(18).unwrap_or_else(|_| "[]".into())).unwrap_or_default();
        Ok(Provider {
            id: row.get(0)?,
            name: row.get(1)?,
            provider_type: row.get(2)?,
            protocol: row.get(3)?,
            base_url: row.get(4)?,
            endpoints,
            api_key: row.get(5)?,
            npm_package: row.get(6)?,
            is_active: row.get(7)?,
            in_failover_queue: row.get(8)?,
            priority: row.get(9)?,
            cost_multiplier: row.get(10)?,
            limit_daily_cny: row.get(11)?,
            limit_monthly_cny: row.get(12)?,
            settings_json: row.get(13)?,
            notes: row.get(14)?,
            icon: row.get(15)?,
            created_at: row.get(16)?,
            updated_at: row.get(17)?,
        })
    })?;

    Ok(rows.next().transpose()?)
}

pub fn insert_provider(conn: &Connection, p: &Provider) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO providers (id, name, provider_type, protocol, base_url, api_key, npm_package,
            is_active, in_failover_queue, priority, cost_multiplier,
            limit_daily_cny, limit_monthly_cny, settings_json, notes, icon,
            created_at, updated_at, endpoints_json)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
        params![
            p.id, p.name, p.provider_type, p.protocol, p.base_url, p.api_key, p.npm_package,
            p.is_active, p.in_failover_queue, p.priority, p.cost_multiplier,
            p.limit_daily_cny, p.limit_monthly_cny, p.settings_json, p.notes, p.icon,
            p.created_at, p.updated_at,
            serde_json::to_string(&p.endpoints).unwrap_or_else(|_| "[]".into()),
        ],
    )?;
    Ok(())
}

pub fn update_provider(conn: &Connection, p: &Provider) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE providers SET name=?2, provider_type=?3, base_url=?4, api_key=?5,
            npm_package=?6, is_active=?7, in_failover_queue=?8, priority=?9,
            cost_multiplier=?10, limit_daily_cny=?11, limit_monthly_cny=?12,
            settings_json=?13, notes=?14, icon=?15, updated_at=?16, endpoints_json=?17
         WHERE id=?1",
        params![
            p.id, p.name, p.provider_type, p.base_url, p.api_key, p.npm_package,
            p.is_active, p.in_failover_queue, p.priority, p.cost_multiplier,
            p.limit_daily_cny, p.limit_monthly_cny, p.settings_json, p.notes, p.icon,
            p.updated_at,
            serde_json::to_string(&p.endpoints).unwrap_or_else(|_| "[]".into()),
        ],
    )?;
    Ok(())
}

pub fn delete_provider(conn: &Connection, id: &str) -> Result<(), rusqlite::Error> {
    conn.execute("DELETE FROM providers WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn set_active_provider(conn: &Connection, id: &str) -> Result<(), rusqlite::Error> {
    conn.execute("UPDATE providers SET is_active = 0", [])?;
    conn.execute("UPDATE providers SET is_active = 1 WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn list_models_for_provider(conn: &Connection, provider_id: &str) -> Result<Vec<Model>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, provider_id, model_id, display_name, tool_call, context_limit,
                output_limit, input_modalities, output_modalities,
                input_price_per_mtok, output_price_per_mtok, is_default, use_proxy, enabled, created_at,
                sort_order
         FROM models WHERE provider_id = ?1
         ORDER BY is_default DESC, sort_order ASC, created_at ASC, id ASC"
    )?;

    let rows = stmt.query_map(params![provider_id], |row| {
        Ok(Model {
            id: row.get(0)?,
            provider_id: row.get(1)?,
            model_id: row.get(2)?,
            display_name: row.get(3)?,
            tool_call: row.get(4)?,
            context_limit: row.get(5)?,
            output_limit: row.get(6)?,
            input_modalities: row.get(7)?,
            output_modalities: row.get(8)?,
            input_price_per_mtok: row.get(9)?,
            output_price_per_mtok: row.get(10)?,
            is_default: row.get(11)?,
            use_proxy: row.get(12)?,
            enabled: row.get(13)?,
            created_at: row.get(14)?,
            sort_order: row.get(15)?,
        })
    })?;

    rows.collect()
}

/// 同名多实例定位：会话可见范围内同 name 实例按 (project_id IS NOT NULL, id) 排序，
/// offset 从 0 起（对应 hint 中的 name#offset+1 编号，两处排序规则必须一致）。
pub fn find_mcp_instance_id(
    conn: &Connection,
    name: &str,
    project_id: Option<&str>,
    offset: usize,
) -> Result<Option<String>, rusqlite::Error> {
    conn.query_row(
        "SELECT id FROM mcp_servers WHERE enabled = 1 AND name = ?1
           AND (project_id IS NULL OR (?2 IS NOT NULL AND project_id = ?2))
         ORDER BY project_id IS NOT NULL, id
         LIMIT 1 OFFSET ?3",
        params![name, project_id, offset as i64],
        |r| r.get(0),
    )
    .optional()
}

/// 列出 MCP 服务器。project_id 为 Some 时返回"用户级(全局) + 该项目级"的并集；
/// 为 None 时仅返回用户级（用于未打开具体项目的全局工作区）。
pub fn list_mcp_servers(conn: &Connection, project_id: Option<&str>) -> Result<Vec<McpServer>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, name, server_type, command, args, env, enabled, description, homepage, created_at,
                last_test_ok, last_test_at, last_test_error, project_id
         FROM mcp_servers
         WHERE project_id IS NULL OR (?1 IS NOT NULL AND project_id = ?1)
         ORDER BY project_id IS NOT NULL, name, id"
    )?;

    let rows = stmt.query_map([project_id], |row| {
        Ok(McpServer {
            id: row.get(0)?,
            name: row.get(1)?,
            server_type: row.get(2)?,
            command: row.get(3)?,
            args: row.get(4)?,
            env: row.get(5)?,
            enabled: row.get(6)?,
            description: row.get(7)?,
            homepage: row.get(8)?,
            created_at: row.get(9)?,
            last_test_ok: row.get(10)?,
            last_test_at: row.get(11)?,
            last_test_error: row.get(12)?,
            project_id: row.get(13)?,
        })
    })?;

    rows.collect()
}

pub fn get_mcp_server(conn: &Connection, id: &str) -> Result<McpServer, rusqlite::Error> {
    conn.query_row(
        "SELECT id, name, server_type, command, args, env, enabled, description, homepage, created_at,
                last_test_ok, last_test_at, last_test_error, project_id
         FROM mcp_servers WHERE id = ?1",
        [id],
        |row| {
            Ok(McpServer {
                id: row.get(0)?,
                name: row.get(1)?,
                server_type: row.get(2)?,
                command: row.get(3)?,
                args: row.get(4)?,
                env: row.get(5)?,
                enabled: row.get(6)?,
                description: row.get(7)?,
                homepage: row.get(8)?,
                created_at: row.get(9)?,
                last_test_ok: row.get(10)?,
                last_test_at: row.get(11)?,
                last_test_error: row.get(12)?,
                project_id: row.get(13)?,
            })
        },
    )
}

pub fn update_mcp_server(
    conn: &Connection,
    id: &str,
    name: &str,
    server_type: &str,
    command: &str,
    env: &str,
    description: Option<&str>,
    homepage: Option<&str>,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE mcp_servers SET name = ?2, server_type = ?3, command = ?4, env = ?5,
                description = ?6, homepage = ?7 WHERE id = ?1",
        params![id, name, server_type, command, env, description, homepage],
    )?;
    Ok(())
}

pub fn insert_mcp_server(conn: &Connection, s: &McpServer) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO mcp_servers (id, name, server_type, command, args, env, enabled, description, homepage, created_at, project_id)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![s.id, s.name, s.server_type, s.command, s.args, s.env, s.enabled, s.description, s.homepage, s.created_at, s.project_id],
    )?;
    Ok(())
}

pub fn toggle_mcp_server(conn: &Connection, id: &str, enabled: bool) -> Result<(), rusqlite::Error> {
    conn.execute("UPDATE mcp_servers SET enabled = ?2 WHERE id = ?1", params![id, enabled])?;
    Ok(())
}

/// 记录最近一次连接测试结果（成功时清空错误信息）
pub fn update_mcp_test_result(
    conn: &Connection,
    id: &str,
    ok: bool,
    error: Option<&str>,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE mcp_servers SET last_test_ok = ?2, last_test_at = ?3, last_test_error = ?4
         WHERE id = ?1",
        params![id, ok, chrono::Utc::now().timestamp(), error],
    )?;
    Ok(())
}

pub fn delete_mcp_server(conn: &Connection, id: &str) -> Result<(), rusqlite::Error> {
    conn.execute("DELETE FROM mcp_servers WHERE id = ?1", params![id])?;
    Ok(())
}

/// 列出技能。project_id 为 Some 时返回"用户级(全局) + 该项目级"并集；None 时仅用户级。
pub fn list_skills(conn: &Connection, project_id: Option<&str>) -> Result<Vec<Skill>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, directory, repo_owner, repo_name, repo_host, repo_branch,
                subdir, enabled, content_hash, installed_at, updated_at, project_id
         FROM skills
         WHERE project_id IS NULL OR (?1 IS NOT NULL AND project_id = ?1)
         ORDER BY project_id IS NOT NULL, name"
    )?;

    let rows = stmt.query_map([project_id], |row| {
        Ok(Skill {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            directory: row.get(3)?,
            repo_owner: row.get(4)?,
            repo_name: row.get(5)?,
            repo_host: row.get(6)?,
            repo_branch: row.get(7)?,
            subdir: row.get(8)?,
            enabled: row.get(9)?,
            content_hash: row.get(10)?,
            installed_at: row.get(11)?,
            updated_at: row.get(12)?,
            project_id: row.get(13)?,
        })
    })?;

    rows.collect()
}

/// 按 (仓库平台, 仓库, 子目录, 作用域) 查技能
pub fn find_skill_by_repo(conn: &Connection, host: &str, owner: &str, name: &str, subdir: &str, project_id: Option<&str>) -> Result<Option<Skill>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, directory, repo_owner, repo_name, repo_host, repo_branch,
                subdir, enabled, content_hash, installed_at, updated_at, project_id
         FROM skills WHERE IFNULL(repo_host, 'github') = ?1 AND repo_owner = ?2 AND repo_name = ?3 AND IFNULL(subdir, '') = ?4
           AND (?5 IS NULL AND project_id IS NULL OR project_id = ?5)",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![host, owner, name, subdir, project_id], |row| {
        Ok(Skill {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            directory: row.get(3)?,
            repo_owner: row.get(4)?,
            repo_name: row.get(5)?,
            repo_host: row.get(6)?,
            repo_branch: row.get(7)?,
            subdir: row.get(8)?,
            enabled: row.get(9)?,
            content_hash: row.get(10)?,
            installed_at: row.get(11)?,
            updated_at: row.get(12)?,
            project_id: row.get(13)?,
        })
    })?;
    rows.next().transpose()
}

pub fn insert_skill(conn: &Connection, s: &Skill) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO skills (id, name, description, directory, repo_owner, repo_name, repo_host, repo_branch,
                subdir, enabled, content_hash, installed_at, updated_at, project_id)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
        params![
            s.id,
            s.name,
            s.description,
            s.directory,
            s.repo_owner,
            s.repo_name,
            s.repo_host,
            s.repo_branch,
            s.subdir,
            s.enabled as i64,
            s.content_hash,
            s.installed_at,
            s.updated_at,
            s.project_id,
        ],
    )?;
    Ok(())
}

pub fn update_skill(conn: &Connection, s: &Skill) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE skills SET name = ?2, description = ?3, directory = ?4, repo_branch = ?5,
                subdir = ?6, updated_at = ?7 WHERE id = ?1",
        params![s.id, s.name, s.description, s.directory, s.repo_branch, s.subdir, s.updated_at],
    )?;
    Ok(())
}

pub fn get_skill(conn: &Connection, id: &str) -> Result<Skill, rusqlite::Error> {
    conn.query_row(
        "SELECT id, name, description, directory, repo_owner, repo_name, repo_host, repo_branch,
                subdir, enabled, content_hash, installed_at, updated_at, project_id
         FROM skills WHERE id = ?1",
        [id],
        |row| {
            Ok(Skill {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                directory: row.get(3)?,
                repo_owner: row.get(4)?,
                repo_name: row.get(5)?,
                repo_host: row.get(6)?,
                repo_branch: row.get(7)?,
                subdir: row.get(8)?,
                enabled: row.get(9)?,
                content_hash: row.get(10)?,
                installed_at: row.get(11)?,
                updated_at: row.get(12)?,
                project_id: row.get(13)?,
            })
        },
    )
}

pub fn delete_skill(conn: &Connection, id: &str) -> Result<(), rusqlite::Error> {
    conn.execute("DELETE FROM skills WHERE id = ?1", [id])?;
    Ok(())
}

/// 目录是否仍被其他 Skill 引用（删除磁盘目录前的安全检查）
pub fn skill_directory_in_use(conn: &Connection, directory: &str) -> Result<bool, rusqlite::Error> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM skills WHERE directory = ?1",
        [directory],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

/// 按名称查找启用的技能（全局 + 项目级；同名时项目级优先，与 skill_hint 注入规则一致）
pub fn find_enabled_skill_by_name(
    conn: &Connection,
    name: &str,
    project_id: Option<&str>,
) -> Result<Option<Skill>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, directory, repo_owner, repo_name, repo_host, repo_branch,
                subdir, enabled, content_hash, installed_at, updated_at, project_id
         FROM skills
         WHERE name = ?1 AND enabled = 1
           AND (project_id IS NULL OR (?2 IS NOT NULL AND project_id = ?2))
         ORDER BY (project_id IS NOT NULL) DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query_map(
        rusqlite::params![name, project_id],
        |r| Ok(Skill {
            id: r.get(0)?,
            name: r.get(1)?,
            description: r.get(2)?,
            directory: r.get(3)?,
            repo_owner: r.get(4)?,
            repo_name: r.get(5)?,
            repo_host: r.get(6)?,
            repo_branch: r.get(7)?,
            subdir: r.get(8)?,
            enabled: r.get(9)?,
            content_hash: r.get(10)?,
            installed_at: r.get(11)?,
            updated_at: r.get(12)?,
            project_id: r.get(13)?,
        }),
    )?;
    rows.next().transpose()
}

/* ============ Skill 调用记录（041） ============ */

/// 记录一次 Skill 调用（use_skill 工具；单条失败不影响工具主流程）
pub fn record_skill_usage(
    conn: &Connection,
    skill_id: &str,
    skill_name: &str,
    conversation_id: &str,
    project_id: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO skill_usage (id, skill_id, skill_name, conversation_id, project_id, created_at)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![
            uuid::Uuid::new_v4().to_string(),
            skill_id,
            skill_name,
            conversation_id,
            project_id,
            chrono::Utc::now().timestamp(),
        ],
    )?;
    Ok(())
}

/// 按技能聚合调用统计（次数 / 最近调用时间）。project_id 为 None 时统计全部项目。
pub fn list_skill_usage_stats(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<Vec<SkillUsageStat>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT skill_id, MAX(skill_name), COUNT(*), MAX(created_at)
         FROM skill_usage
         WHERE (?1 IS NULL OR project_id = ?1)
         GROUP BY skill_id ORDER BY COUNT(*) DESC",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![project_id], |row| {
            Ok(SkillUsageStat {
                skill_id: row.get(0)?,
                skill_name: row.get(1)?,
                call_count: row.get(2)?,
                last_called_at: row.get(3)?,
            })
        })?
        .collect();
    rows
}

/// 最近 Skill 调用明细（时间线，limit 条）。project_id 为 None 时返回全部项目。
pub fn list_skill_usage_events(
    conn: &Connection,
    project_id: Option<&str>,
    limit: i64,
) -> Result<Vec<SkillUsageEvent>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT u.id, u.skill_id, u.skill_name, IFNULL(c.title, ''), u.project_id, u.created_at
         FROM skill_usage u
         LEFT JOIN conversations c ON c.id = u.conversation_id
         WHERE (?1 IS NULL OR u.project_id = ?1)
         ORDER BY u.created_at DESC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![project_id, limit], |row| {
            Ok(SkillUsageEvent {
                id: row.get(0)?,
                skill_id: row.get(1)?,
                skill_name: row.get(2)?,
                conversation_title: row.get(3)?,
                project_id: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?
        .collect();
    rows
}

/* ============ 项目记忆（008） ============ */

/// 列出项目的全部记忆（按更新时间倒序）
pub fn list_memories(conn: &Connection, project_id: &str) -> Result<Vec<ProjectMemory>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, category, title, content, enabled, created_at, updated_at
         FROM project_memories WHERE project_id = ?1 ORDER BY updated_at DESC"
    )?;
    let rows = stmt.query_map([project_id], |row| {
        Ok(ProjectMemory {
            id: row.get(0)?,
            project_id: row.get(1)?,
            category: row.get(2)?,
            title: row.get(3)?,
            content: row.get(4)?,
            enabled: row.get(5)?,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
        })
    })?;
    rows.collect()
}

/// 插入一条记忆
pub fn insert_memory(conn: &Connection, m: &ProjectMemory) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO project_memories (id, project_id, category, title, content, enabled, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![m.id, m.project_id, m.category, m.title, m.content, m.enabled as i64, m.created_at, m.updated_at],
    )?;
    Ok(())
}

/// 更新记忆内容（标题/分类/内容）
pub fn update_memory(conn: &Connection, m: &ProjectMemory) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE project_memories SET category = ?2, title = ?3, content = ?4, updated_at = ?5 WHERE id = ?1",
        params![m.id, m.category, m.title, m.content, m.updated_at],
    )?;
    Ok(())
}

/// 删除一条记忆
pub fn delete_memory(conn: &Connection, id: &str) -> Result<(), rusqlite::Error> {
    conn.execute("DELETE FROM project_memories WHERE id = ?1", [id])?;
    Ok(())
}

/// 启用/禁用记忆（禁用后不再注入，但保留记录）
pub fn set_memory_enabled(conn: &Connection, id: &str, enabled: bool) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE project_memories SET enabled = ?2 WHERE id = ?1",
        params![id, enabled as i64],
    )?;
    Ok(())
}

/* ============ 工具调用统计（Evaluation） ============ */

/// 按工具聚合统计：次数 / 成功失败 / 平均耗时 / 最近调用
pub fn list_tool_stats(conn: &Connection, project_id: &str) -> Result<Vec<ToolStat>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT t.tool_name,
                COUNT(*),
                SUM(CASE WHEN t.status = 'ok' THEN 1 ELSE 0 END),
                SUM(CASE WHEN t.status IN ('error','cancelled') THEN 1 ELSE 0 END),
                CAST(AVG(t.duration_ms) AS INTEGER),
                MAX(t.created_at)
         FROM tool_runs t
         JOIN conversations c ON c.id = t.conversation_id
         WHERE c.project_id = ?1
         GROUP BY t.tool_name
         ORDER BY COUNT(*) DESC"
    )?;
    let rows = stmt.query_map([project_id], |row| {
        Ok(ToolStat {
            tool_name: row.get(0)?,
            call_count: row.get(1)?,
            success_count: row.get(2)?,
            fail_count: row.get(3)?,
            avg_duration_ms: row.get(4)?,
            last_called_at: row.get(5)?,
        })
    })?;
    rows.collect()
}

/// 按 MCP 工具聚合调用统计（tool_runs 中 tool_name 以 mcp__ 前缀的工具）。
/// project_id 为空时统计全部项目（全局 MCP 服务器跨项目调用也计入）。
/// 返回按工具名的原始统计，服务器级聚合由调用方完成。
pub fn list_mcp_tool_stats(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<Vec<ToolStat>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT t.tool_name,
                COUNT(*),
                SUM(CASE WHEN t.status = 'ok' THEN 1 ELSE 0 END),
                SUM(CASE WHEN t.status IN ('error','cancelled') THEN 1 ELSE 0 END),
                CAST(AVG(t.duration_ms) AS INTEGER),
                MAX(t.created_at)
         FROM tool_runs t
         JOIN conversations c ON c.id = t.conversation_id
         WHERE substr(t.tool_name, 1, 5) = 'mcp__'
           AND (?1 IS NULL OR c.project_id = ?1)
         GROUP BY t.tool_name
         ORDER BY COUNT(*) DESC"
    )?;
    let rows = stmt.query_map([project_id], |row| {
        Ok(ToolStat {
            tool_name: row.get(0)?,
            call_count: row.get(1)?,
            success_count: row.get(2)?,
            fail_count: row.get(3)?,
            avg_duration_ms: row.get(4)?,
            last_called_at: row.get(5)?,
        })
    })?;
    rows.collect()
}

/// 按模型聚合 LLM 调用统计：请求数 / 输入输出缓存 token / 费用 / 平均耗时。
/// days>0 时只统计最近 N 天（created_at 为 unix 秒），days=0 表示全部。
/// 说明：request_logs 未记录工具名/项目归属，无法按工具 join token 维度，
/// 因此 token 排行按模型聚合（口径为全部会话的模型消耗）。
pub fn list_model_token_stats(conn: &Connection, days: i64) -> Result<Vec<ModelTokenStat>, rusqlite::Error> {
    let cutoff = if days > 0 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Some(now - days * 86400)
    } else {
        None
    };
    let mut stmt = conn.prepare(
        "SELECT COALESCE(model, '(unknown)'),
                COUNT(*),
                SUM(input_tokens),
                SUM(output_tokens),
                SUM(cache_read_tokens + cache_creation_tokens),
                SUM(total_cost_cny),
                CAST(AVG(latency_ms) AS INTEGER)
         FROM request_logs
         WHERE (?1 IS NULL OR created_at >= ?1)
         GROUP BY COALESCE(model, '(unknown)')
         ORDER BY SUM(input_tokens + output_tokens) DESC"
    )?;
    let rows = stmt.query_map(params![cutoff], |row| {
        Ok(ModelTokenStat {
            model: row.get(0)?,
            request_count: row.get(1)?,
            input_tokens: row.get(2)?,
            output_tokens: row.get(3)?,
            cache_tokens: row.get(4)?,
            total_cost_cny: row.get(5)?,
            avg_latency_ms: row.get(6)?,
        })
    })?;
    rows.collect()
}

/// 按工具聚合 LLM token 消耗（[69]：request_logs.tool_name 非空的记录，代理链路口径）。
/// days>0 时只统计最近 N 天；days=0 表示全部。
pub fn list_tool_token_stats(conn: &Connection, days: i64) -> Result<Vec<ToolTokenStat>, rusqlite::Error> {
    let cutoff = if days > 0 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Some(now - days * 86400)
    } else {
        None
    };
    let mut stmt = conn.prepare(
        "SELECT tool_name,
                COUNT(*),
                SUM(input_tokens),
                SUM(output_tokens),
                SUM(total_cost_cny)
         FROM request_logs
         WHERE tool_name IS NOT NULL AND tool_name != ''
           AND (?1 IS NULL OR created_at >= ?1)
         GROUP BY tool_name
         ORDER BY SUM(input_tokens + output_tokens) DESC"
    )?;
    let rows = stmt.query_map(params![cutoff], |row| {
        Ok(ToolTokenStat {
            tool_name: row.get(0)?,
            request_count: row.get(1)?,
            input_tokens: row.get(2)?,
            output_tokens: row.get(3)?,
            total_cost_cny: row.get(4)?,
        })
    })?;
    rows.collect()
}

pub fn get_request_logs(conn: &Connection, limit: i64, offset: i64) -> Result<Vec<RequestLog>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, provider_id, model, input_tokens, output_tokens,
                cache_read_tokens, cache_creation_tokens, total_cost_cny,
                latency_ms, first_token_ms, status_code, error_message,
                session_id, is_streaming, created_at
         FROM request_logs ORDER BY created_at DESC LIMIT ?1 OFFSET ?2"
    )?;

    let rows = stmt.query_map(params![limit, offset], |row| {
        Ok(RequestLog {
            id: row.get(0)?,
            provider_id: row.get(1)?,
            model: row.get(2)?,
            input_tokens: row.get(3)?,
            output_tokens: row.get(4)?,
            cache_read_tokens: row.get(5)?,
            cache_creation_tokens: row.get(6)?,
            total_cost_cny: row.get(7)?,
            latency_ms: row.get(8)?,
            first_token_ms: row.get(9)?,
            status_code: row.get(10)?,
            error_message: row.get(11)?,
            session_id: row.get(12)?,
            is_streaming: row.get(13)?,
            created_at: row.get(14)?,
        })
    })?;

    rows.collect()
}

pub fn get_daily_usage(conn: &Connection, start_date: &str, end_date: &str) -> Result<Vec<DailyUsage>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT date, provider_id, model, request_count, input_tokens, output_tokens, total_cost_cny
         FROM usage_daily WHERE date >= ?1 AND date <= ?2 ORDER BY date"
    )?;

    let rows = stmt.query_map(params![start_date, end_date], |row| {
        Ok(DailyUsage {
            date: row.get(0)?,
            provider_id: row.get(1)?,
            model: row.get(2)?,
            request_count: row.get(3)?,
            input_tokens: row.get(4)?,
            output_tokens: row.get(5)?,
            total_cost_cny: row.get(6)?,
        })
    })?;

    rows.collect()
}

/// 按模型聚合请求日志（费用按模型分组统计）；start/end 为秒级时间戳
pub fn get_cost_by_model(conn: &Connection, start: i64, end: i64) -> Result<Vec<ModelCost>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(model, 'unknown'), COUNT(*), SUM(input_tokens), SUM(output_tokens), SUM(total_cost_cny)
         FROM request_logs
         WHERE created_at >= ?1 AND created_at <= ?2
         GROUP BY COALESCE(model, 'unknown')
         ORDER BY SUM(total_cost_cny) DESC, COUNT(*) DESC"
    )?;

    let rows = stmt.query_map(params![start, end], |row| {
        Ok(ModelCost {
            model: row.get(0)?,
            request_count: row.get(1)?,
            input_tokens: row.get(2)?,
            output_tokens: row.get(3)?,
            total_cost_cny: row.get(4)?,
        })
    })?;

    rows.collect()
}

/* ============ 任务级 Trace（010） ============ */

pub fn insert_task_run(conn: &Connection, t: &TaskRun) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO task_runs (id, conversation_id, project_id, provider_id, model, status,
                error_kind, error_message, tool_rounds, retry_count,
                input_tokens, output_tokens, cost_cny, duration_ms, started_at, finished_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
        params![
            t.id, t.conversation_id, t.project_id, t.provider_id, t.model, t.status,
            t.error_kind, t.error_message, t.tool_rounds, t.retry_count,
            t.input_tokens, t.output_tokens, t.cost_cny, t.duration_ms, t.started_at, t.finished_at,
        ],
    )?;

    // 滚动清理：超出保留期的最旧任务记录直接删除，防止成本明细无限堆积
    let _ = crate::services::maintenance::prune_task_runs(
        conn,
        crate::services::maintenance::TASK_RUN_KEEP_DAYS,
    );
    Ok(())
}

/// 最近任务列表（倒序分页）：project_id 为空表示全局；status 可选过滤（success/error/cancelled）
pub fn list_task_runs(
    conn: &Connection,
    project_id: &str,
    status: Option<&str>,
    limit: i64,
) -> Result<Vec<TaskRun>, rusqlite::Error> {
    let mut sql = String::from(
        "SELECT id, conversation_id, project_id, provider_id, model, status,
                error_kind, error_message, tool_rounds, retry_count,
                input_tokens, output_tokens, cost_cny, duration_ms, started_at, finished_at
         FROM task_runs",
    );
    let mut conds: Vec<String> = Vec::new();
    let mut vals: Vec<&dyn rusqlite::ToSql> = Vec::new();
    let mut status_val: Option<String> = None;
    if !project_id.is_empty() {
        conds.push("project_id = ?".to_string());
        vals.push(&project_id);
    }
    if let Some(s) = status.filter(|s| !s.is_empty()) {
        conds.push("status = ?".to_string());
        status_val = Some(s.to_string());
    }
    if let Some(s) = &status_val {
        vals.push(s);
    }
    if !conds.is_empty() {
        sql.push_str(&format!(" WHERE {}", conds.join(" AND ")));
    }
    sql.push_str(" ORDER BY started_at DESC LIMIT ?");
    vals.push(&limit);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(vals.iter().copied()), |r| {
        Ok(TaskRun {
            id: r.get(0)?,
            conversation_id: r.get(1)?,
            project_id: r.get(2)?,
            provider_id: r.get(3)?,
            model: r.get(4)?,
            status: r.get(5)?,
            error_kind: r.get(6)?,
            error_message: r.get(7)?,
            tool_rounds: r.get(8)?,
            retry_count: r.get(9)?,
            input_tokens: r.get(10)?,
            output_tokens: r.get(11)?,
            cost_cny: r.get(12)?,
            duration_ms: r.get(13)?,
            started_at: r.get(14)?,
            finished_at: r.get(15)?,
        })
    })?;
    rows.collect()
}

/// 任务指标聚合：project_id 为空表示全局；days 限定时间窗口（近 N 天）
pub fn get_task_stats(conn: &Connection, project_id: &str, days: i64) -> Result<TaskStats, rusqlite::Error> {
    let since = chrono::Utc::now().timestamp() - days * 86400;
    let (where_sql, where_params): (&str, Vec<&dyn rusqlite::ToSql>) = if project_id.is_empty() {
        ("WHERE started_at >= ?1", vec![&since])
    } else {
        ("WHERE project_id = ?1 AND started_at >= ?2", vec![&project_id, &since])
    };

    let mut stmt = conn.prepare(&format!(
        "SELECT COUNT(*),
                SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END),
                SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END),
                SUM(CASE WHEN status = 'cancelled' THEN 1 ELSE 0 END),
                SUM(input_tokens), SUM(output_tokens), SUM(cost_cny),
                AVG(duration_ms)
         FROM task_runs {where_sql}"
    ))?;
    let (total, success, error, cancelled, input, output, cost, avg) = stmt
        .query_row(rusqlite::params_from_iter(where_params.iter().copied()), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                r.get::<_, Option<i64>>(5)?.unwrap_or(0),
                r.get::<_, Option<f64>>(6)?.unwrap_or(0.0),
                r.get::<_, Option<f64>>(7)?,
            ))
        })?;

    // 耗时百分位：取全部 duration_ms 排序后定位（数据量小，直接读全量）
    let mut durations: Vec<i64> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT duration_ms FROM task_runs {where_sql} ORDER BY duration_ms"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(where_params.iter().copied()), |r| r.get::<_, i64>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    durations.sort_unstable();
    let percentile = |p: f64| -> Option<i64> {
        if durations.is_empty() {
            return None;
        }
        let idx = (((durations.len() as f64 - 1.0) * p) as usize).min(durations.len() - 1);
        Some(durations[idx])
    };
    let p50 = percentile(0.5);
    let p95 = percentile(0.95);

    // 错误分类分布
    let mut by_error_kind: Vec<ErrorKindCount> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT IFNULL(error_kind, 'unknown'), COUNT(*) FROM task_runs
             WHERE status = 'error' AND {}",
            if project_id.is_empty() {
                "started_at >= ?1".to_string()
            } else {
                "project_id = ?1 AND started_at >= ?2".to_string()
            }
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(where_params.iter().copied()), |r| {
            Ok(ErrorKindCount { kind: r.get(0)?, count: r.get(1)? })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    by_error_kind.sort_by(|a, b| b.count.cmp(&a.count));

    let effective = (total - cancelled).max(1);
    Ok(TaskStats {
        total_tasks: total,
        success_count: success,
        error_count: error,
        cancelled_count: cancelled,
        success_rate: success as f64 / effective as f64,
        p50_ms: p50,
        p95_ms: p95,
        avg_duration_ms: avg.map(|v| v as i64),
        total_cost_cny: cost,
        total_input_tokens: input,
        total_output_tokens: output,
        by_error_kind,
    })
}

// ---------- 通用 settings 表读写（键值，供环境配置等使用） ----------

/// 读取一个设置项，不存在返回 None
pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>, rusqlite::Error> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        [key],
        |r| r.get::<_, String>(0),
    )
    .optional()
}

/// 写入一个设置项（UPSERT）
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

// ---------- 知识库条目 ----------

/// 按项目路径查 project_id（用于工具执行时把路径映射到作用域）。
/// 精确匹配优先；子目录（如工作区内的鸿蒙子工程）按前缀命中所属项目，取前缀最长者。
pub fn project_id_by_path(conn: &Connection, path: &str) -> Result<Option<String>, rusqlite::Error> {
    let exact = conn
        .query_row(
            "SELECT id FROM projects WHERE path = ?1 COLLATE NOCASE LIMIT 1",
            [path],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    if exact.is_some() {
        return Ok(exact);
    }
    // 子目录前缀匹配：路径须在项目根下（分隔符边界），取前缀最长（最接近根）的项目
    let normalized = path.replace('\\', "/").trim_end_matches('/').to_string();
    let mut stmt = conn.prepare("SELECT id, path FROM projects WHERE path IS NOT NULL")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut best: Option<(String, usize)> = None;
    for row in rows.flatten() {
        let root = row.1.replace('\\', "/").trim_end_matches('/').to_string();
        if normalized.len() > root.len()
            && normalized.starts_with(&root)
            && normalized[root.len()..].starts_with('/')
        {
            let len = root.len();
            if best.as_ref().map_or(true, |(_, bl)| len > *bl) {
                best = Some((row.0, len));
            }
        }
    }
    Ok(best.map(|(id, _)| id))
}

/// 列出指定作用域可见的知识条目：project_id 为 None 时取全局，
/// 为 Some(pid) 时取该项目专属（不含全局；全局+项目合并在匹配层处理）。
pub fn list_knowledge(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<Vec<KnowledgeEntry>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, keywords, title, cause, fix, enabled, builtin, project_id, hit_count, created_at, updated_at
         FROM knowledge_entries
         WHERE (?1 IS NULL AND project_id IS NULL) OR (project_id = ?1)
         ORDER BY hit_count DESC, builtin DESC, title ASC",
    )?;
    let rows = stmt.query_map([project_id], row_to_knowledge)?;
    rows.collect()
}

/// 列出所有启用的知识条目（全局 + 指定项目），用于工具失败时匹配注入。
/// 项目条目优先；同层内命中次数多的排前面。
pub fn list_enabled_knowledge(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<Vec<KnowledgeEntry>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, keywords, title, cause, fix, enabled, builtin, project_id, hit_count, created_at, updated_at
         FROM knowledge_entries
         WHERE enabled = 1 AND (project_id IS NULL OR project_id = ?1)
         ORDER BY CASE WHEN project_id IS NULL THEN 1 ELSE 0 END, hit_count DESC, title ASC",
    )?;
    let rows = stmt.query_map([project_id], row_to_knowledge)?;
    rows.collect()
}

pub fn insert_knowledge(conn: &Connection, e: &KnowledgeEntry) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO knowledge_entries (id, keywords, title, cause, fix, enabled, builtin, project_id, hit_count, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![e.id, e.keywords, e.title, e.cause, e.fix, e.enabled as i64, e.builtin as i64, e.project_id, e.hit_count, e.created_at, e.updated_at],
    )?;
    Ok(())
}

pub fn increment_knowledge_hits(conn: &Connection, id: &str) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE knowledge_entries SET hit_count = hit_count + 1 WHERE id = ?1",
        [id],
    )?;
    Ok(())
}

/// 按关键字搜索知识库（title/keywords/cause/fix 模糊匹配），供 Agent 主动查询团队经验。
pub fn search_knowledge(
    conn: &Connection,
    project_id: Option<&str>,
    keyword: &str,
    limit: usize,
) -> Result<Vec<KnowledgeEntry>, rusqlite::Error> {
    let like = format!("%{}%", keyword.trim());
    let mut stmt = conn.prepare(
        "SELECT id, keywords, title, cause, fix, enabled, builtin, project_id, hit_count, created_at, updated_at
         FROM knowledge_entries
         WHERE enabled = 1 AND (project_id IS NULL OR project_id = ?1)
           AND (title LIKE ?2 OR keywords LIKE ?2 OR cause LIKE ?2 OR fix LIKE ?2)
         ORDER BY CASE WHEN project_id IS NULL THEN 1 ELSE 0 END, hit_count DESC, title ASC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![project_id, like, limit as i64], row_to_knowledge)?;
    rows.collect()
}

pub fn update_knowledge(conn: &Connection, e: &KnowledgeEntry) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE knowledge_entries SET keywords=?2, title=?3, cause=?4, fix=?5, enabled=?6, updated_at=?7 WHERE id=?1",
        params![e.id, e.keywords, e.title, e.cause, e.fix, e.enabled as i64, e.updated_at],
    )?;
    Ok(())
}

pub fn delete_knowledge(conn: &Connection, id: &str) -> Result<(), rusqlite::Error> {
    conn.execute("DELETE FROM knowledge_entries WHERE id=?1 AND builtin=0", [id])?;
    Ok(())
}

fn row_to_knowledge(row: &rusqlite::Row) -> rusqlite::Result<KnowledgeEntry> {
    Ok(KnowledgeEntry {
        id: row.get(0)?,
        keywords: row.get(1)?,
        title: row.get(2)?,
        cause: row.get(3)?,
        fix: row.get(4)?,
        enabled: row.get::<_, i64>(5)? != 0,
        builtin: row.get::<_, i64>(6)? != 0,
        project_id: row.get(7)?,
        hit_count: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

