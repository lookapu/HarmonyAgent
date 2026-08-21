//! EC-18：资产版本清单与兼容策略。
//!
//! 统一回答"数据库、工作流、Skill、工具协议和知识索引当前处于什么版本、
//! 兼容承诺是什么、破坏性变化如何迁移"。数据库侧需要连接以读取
//! `tool_protocol_versions` 表；其余资产以代码与文档中的常量版本为准。

use rusqlite::Connection;

#[derive(Clone, Debug, serde::Serialize)]
pub struct AssetVersion {
    pub asset: String,
    pub schema_version: String,
    pub status: String,
    pub compatibility: String,
    pub migration_notes: String,
}

/// 工具结果协议的当前 schema 版本，与 docs/TOOL_RESULT_V2.md 保持一致。
pub const TOOL_PROTOCOL_SCHEMA_VERSION: i64 = 2;
/// Skill 与能力包规范版本，见 docs/SKILL_CAPABILITY_SPEC.md。
pub const SKILL_SPEC_VERSION: &str = "1";
/// 工作流模板规范版本，见 docs/WORKFLOW_TEMPLATE_SPEC.md。
pub const WORKFLOW_SPEC_VERSION: &str = "1";
/// 评测运行快照 schema，见 docs/EVALUATION_RUN_SNAPSHOTS.md。
pub const EVAL_SNAPSHOT_SCHEMA: u32 = crate::agent::evals::EVAL_SNAPSHOT_SCHEMA_VERSION;
/// 评测 CI 基线 schema，见 docs/EVALUATION_CI_GATES.md。
pub const EVAL_BASELINE_SCHEMA: u32 = crate::agent::evals::BASELINE_SCHEMA_VERSION;

/// 汇总全部版本化资产的当前版本与兼容承诺。
/// `conn` 为 None 时工具协议只报告代码常量，不读历史兼容记录。
pub fn asset_versions(conn: Option<&Connection>) -> Vec<AssetVersion> {
    let mut versions = vec![
        AssetVersion {
            asset: "database".into(),
            schema_version: crate::db::MIGRATIONS.len().to_string(),
            status: "stable".into(),
            compatibility: "只允许递增迁移；旧数据必须可前滚且重放受保护".into(),
            migration_notes: "新增迁移必须满足 Q-01：前滚、重复执行保护和兼容旧数据测试".into(),
        },
        AssetVersion {
            asset: "tool_protocol".into(),
            schema_version: TOOL_PROTOCOL_SCHEMA_VERSION.to_string(),
            status: "stable".into(),
            compatibility: "读取器必须忽略并保留未知字段；删除字段或改变含义才提升主版本并提供迁移".into(),
            migration_notes: "tool_protocol_versions 表保存历史版本的生产者、兼容性与迁移说明".into(),
        },
        AssetVersion {
            asset: "skill_spec".into(),
            schema_version: SKILL_SPEC_VERSION.into(),
            status: "stable".into(),
            compatibility: "manifest 版本化、权限声明与兼容范围按 SKILL_CAPABILITY_SPEC.md".into(),
            migration_notes: "旧格式 Skill 有显式升级路径与内容漂移门禁".into(),
        },
        AssetVersion {
            asset: "workflow_spec".into(),
            schema_version: WORKFLOW_SPEC_VERSION.into(),
            status: "stable".into(),
            compatibility: "模板格式、权限差异、审批与版本归档按 WORKFLOW_TEMPLATE_SPEC.md".into(),
            migration_notes: "升级与兼容规则见 WORKFLOW_TEMPLATE_SPEC.md".into(),
        },
        AssetVersion {
            asset: "knowledge_index".into(),
            schema_version: "in-process".into(),
            status: "evolving".into(),
            compatibility: "以本机 SDK 声明文件为重建真源，无持久 schema，不跨 SDK 版本污染".into(),
            migration_notes: "一旦持久化或跨进程共享，必须携带 schema_version 并遵循向后兼容规则".into(),
        },
        AssetVersion {
            asset: "eval_snapshot".into(),
            schema_version: EVAL_SNAPSHOT_SCHEMA.to_string(),
            status: "stable".into(),
            compatibility: "新字段保持旧 reader 可忽略；破坏性变化提升 schema_version".into(),
            migration_notes: "历史运行按 schema 0 兼容读取，见 EVALUATION_RUN_SNAPSHOTS.md".into(),
        },
        AssetVersion {
            asset: "eval_baseline".into(),
            schema_version: EVAL_BASELINE_SCHEMA.to_string(),
            status: "stable".into(),
            compatibility: "基线跨机器比较；套件或结构变化时旧基线自动失效重建".into(),
            migration_notes: "容差与生命周期见 EVALUATION_CI_GATES.md".into(),
        },
    ];
    if let Some(conn) = conn {
        if let Ok(mut rows) = conn.prepare(
            "SELECT schema_version,status,min_reader_version,producer_version,compatibility,migration_notes
             FROM tool_protocol_versions ORDER BY schema_version DESC LIMIT 1",
        ) {
            if let Ok(row) = rows.query_row([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            }) {
                versions.push(AssetVersion {
                    asset: "tool_protocol_history".into(),
                    schema_version: row.0.to_string(),
                    status: row.1,
                    compatibility: format!("最低读取器版本 {}", row.2),
                    migration_notes: format!("生产者 {}：{}", row.3, row.5),
                });
            }
        }
    }
    versions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_versions_are_complete_and_consistent() {
        let versions = asset_versions(None);
        let assets = versions
            .iter()
            .map(|item| item.asset.as_str())
            .collect::<std::collections::HashSet<_>>();
        for expected in [
            "database",
            "tool_protocol",
            "skill_spec",
            "workflow_spec",
            "knowledge_index",
            "eval_snapshot",
            "eval_baseline",
        ] {
            assert!(assets.contains(expected), "缺少资产版本条目 {expected}");
        }
        // 数据库版本必须与迁移注册表一致
        let database = versions
            .iter()
            .find(|item| item.asset == "database")
            .unwrap();
        assert_eq!(
            database.schema_version,
            crate::db::MIGRATIONS.len().to_string()
        );
        // 协议版本与文档常量一致
        let protocol = versions
            .iter()
            .find(|item| item.asset == "tool_protocol")
            .unwrap();
        assert_eq!(
            protocol.schema_version,
            TOOL_PROTOCOL_SCHEMA_VERSION.to_string()
        );
        // 每个条目都必须有兼容承诺与迁移说明
        for version in &versions {
            assert!(!version.compatibility.is_empty());
            assert!(!version.migration_notes.is_empty());
        }
    }

    #[test]
    fn asset_versions_read_protocol_history_from_db() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tool_protocol_versions(schema_version INTEGER,status TEXT,
             min_reader_version INTEGER,producer_version TEXT,compatibility TEXT,migration_notes TEXT);
             INSERT INTO tool_protocol_versions VALUES(2,'stable',2,'1.0.0','read','write');",
        )
        .unwrap();
        let versions = asset_versions(Some(&conn));
        assert!(versions
            .iter()
            .any(|item| item.asset == "tool_protocol_history"
                && item.schema_version == "2"
                && item.compatibility.contains("2")));
    }
}
