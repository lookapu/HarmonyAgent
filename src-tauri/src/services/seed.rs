//! API 知识库种子数据导入。
//!
//! 出厂安装包内置一份预抓取的鸿蒙 API 知识库（api_docs / api_details / api_members），
//! 放在资源目录 `seed/knowledge.db`。启动时后台线程按版本集合比对：
//! - 主库为空（新装用户）→ 全量导入，开箱即可离线查询 API 文档
//! - 主库有部分版本（老用户/升级）→ 只补全种子库中主库缺失的版本，保证齐全
//! - 主库版本已覆盖种子库全部版本 → 跳过
//!
//! - 只补不删：仅 INSERT OR IGNORE 新增种子库独有条目，不动主库已有数据
//! - 独立连接导入：不阻塞主连接与 UI（WAL 模式支持并发读）
//! - 失败静默：种子库缺失（开发模式）或导入失败只记日志，不影响启动

use std::path::{Path, PathBuf};

/// 种子库在资源目录下的相对路径（对应 tauri.conf.json 的 bundle.resources 映射）
const SEED_REL: &str = "seed/knowledge.db";

/// setup 时调用：后台线程导入/补全种子数据。
/// 传入主库路径与资源目录（无需 AppHandle，便于线程内独立打开连接）。
pub fn seed_api_knowledge(db_path: &Path, resource_dir: Option<PathBuf>) {
    let seed = match resource_dir {
        Some(dir) => dir.join(SEED_REL),
        None => return,
    };
    if !seed.is_file() {
        return;
    }

    let db_path = db_path.to_path_buf();
    std::thread::spawn(move || {
        let imported = match import_into(&db_path, &seed) {
            Ok(n) => n,
            Err(e) => {
                crate::utils::logger::log_event(
                    "seed_import_error",
                    serde_json::json!({ "error": e }),
                );
                return;
            }
        };
        crate::utils::logger::log_event(
            "seed_imported",
            serde_json::json!({ "rows": imported }),
        );
    });
}

/// 打开主库新连接执行导入/补全：ATTACH 种子库 → 比对版本集合 → 需要补全时
/// 逐表 INSERT OR IGNORE → 记录 meta。返回本次补全的总行数。
///
/// 补全条件：种子库中存在主库没有的 version_label（主库为空时天然成立）。
/// 主库版本已全覆盖种子库 → 返回 0 跳过，避免每次启动重复写库。
fn import_into(db_path: &Path, seed: &Path) -> Result<usize, String> {
    let conn = rusqlite::Connection::open(db_path).map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA busy_timeout=5000;")
        .map_err(|e| e.to_string())?;
    let attach = format!("ATTACH DATABASE '{}' AS seed", seed.to_string_lossy().replace('\'', "''"));
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute_batch(&attach).map_err(|e| e.to_string())?;

    // 种子库中主库缺失的版本数（去重）
    let missing: i64 = tx
        .query_row(
            "SELECT COUNT(DISTINCT version_label) FROM seed.api_docs
             WHERE version_label NOT IN (
                SELECT DISTINCT version_label FROM main.api_docs
             )",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let mut total = 0usize;
    if missing > 0 {
        let tables = ["api_docs", "api_details", "api_members", "api_docs_embeddings", "api_docs_meta"];
        for t in tables {
            // 种子库可能缺表（旧种子/精简种子），逐表容错：表不存在则跳过
            let seed_has: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM seed.sqlite_master WHERE type='table' AND name=?1",
                    [t],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            if seed_has == 0 {
                continue;
            }
            let n = tx
                .execute(
                    &format!("INSERT OR IGNORE INTO main.{t} SELECT * FROM seed.{t}"),
                    [],
                )
                .map_err(|e| format!("导入 {t} 失败: {e}"))?;
            total += n;
        }
        tx.execute(
            "INSERT OR REPLACE INTO main.api_docs_meta (key, value) VALUES ('seeded_at', unixepoch())",
            [],
        )
        .map_err(|e| format!("写入导入标记失败: {e}"))?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    let _ = conn.execute_batch("DETACH DATABASE seed");
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空库 + 有效种子库 → 导入成功且幂等（二次调用跳过）
    #[test]
    fn test_seed_import_idempotent() {
        let dir = std::env::temp_dir().join(format!("deveco-seed-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let main = dir.join("main.db");
        let seed = dir.join("seed.db");

        // 构造种子库：含最小迁移结构（四张 API 表 + 1 行 api_docs + 1 行 meta）
        let schema = "
            CREATE TABLE api_docs (id INTEGER PRIMARY KEY AUTOINCREMENT, kit TEXT NOT NULL, dts_file TEXT, module TEXT, class_name TEXT, declaration TEXT NOT NULL, api_name TEXT, change_type TEXT NOT NULL, version_label TEXT NOT NULL, api_level INTEGER, old_declaration TEXT, source_url TEXT, fetched_at INTEGER NOT NULL);
            CREATE TABLE api_details (id INTEGER PRIMARY KEY AUTOINCREMENT, module TEXT NOT NULL, slug TEXT NOT NULL UNIQUE, title TEXT, kit TEXT, since_api_level INTEGER, deprecated INTEGER NOT NULL DEFAULT 0, import_snippet TEXT, syscap TEXT, permissions TEXT, device_types TEXT, body TEXT, examples TEXT, members TEXT, source_url TEXT NOT NULL, fetched_at INTEGER NOT NULL);
            CREATE TABLE api_members (id INTEGER PRIMARY KEY AUTOINCREMENT, detail_slug TEXT NOT NULL, module TEXT, parent_name TEXT, member_name TEXT NOT NULL, kind TEXT NOT NULL, declaration TEXT, description TEXT, since_api_level INTEGER, deprecated INTEGER NOT NULL DEFAULT 0, syscap TEXT, permission TEXT, source_url TEXT);
            CREATE TABLE api_docs_meta (key TEXT PRIMARY KEY, value TEXT);
        ";
        {
            let c = rusqlite::Connection::open(&seed).unwrap();
            c.execute_batch(schema).unwrap();
            c.execute_batch(
                "INSERT INTO api_docs (kit, declaration, change_type, version_label, fetched_at) VALUES ('Ability Kit','function f(): void;','added','26.0.0 Beta1', 0);
                 INSERT INTO api_docs_meta (key, value) VALUES ('last_refreshed_at', '0');",
            )
            .unwrap();
        }
        // 主库：模拟 db::init 后的结构（四张 API 表均为空）
        {
            let c = rusqlite::Connection::open(&main).unwrap();
            c.execute_batch(schema).unwrap();
        }

        let n = import_into(&main, &seed).expect("首次导入应成功");
        assert_eq!(n, 2, "导入 1 条 api_docs + 1 条 meta");
        let again = import_into(&main, &seed).expect("二次调用应跳过");
        assert_eq!(again, 0, "已有数据时不再导入");

        let c = rusqlite::Connection::open(&main).unwrap();
        let cnt: i64 = c.query_row("SELECT COUNT(*) FROM api_docs", [], |r| r.get(0)).unwrap();
        assert_eq!(cnt, 1, "重复导入不应产生重复行");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 主库已有部分版本 + 种子库含更多版本 → 只补全缺失版本，不动已有数据。
    #[test]
    fn test_seed_import_backfills_missing_versions() {
        let dir = std::env::temp_dir().join(format!("deveco-seed-backfill-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let main = dir.join("main.db");
        let seed = dir.join("seed.db");

        let schema = "
            CREATE TABLE api_docs (id INTEGER PRIMARY KEY AUTOINCREMENT, kit TEXT NOT NULL, dts_file TEXT, module TEXT, class_name TEXT, declaration TEXT NOT NULL, api_name TEXT, change_type TEXT NOT NULL, version_label TEXT NOT NULL, api_level INTEGER, old_declaration TEXT, source_url TEXT, fetched_at INTEGER NOT NULL);
            CREATE TABLE api_details (id INTEGER PRIMARY KEY AUTOINCREMENT, module TEXT NOT NULL, slug TEXT NOT NULL UNIQUE, title TEXT, kit TEXT, since_api_level INTEGER, deprecated INTEGER NOT NULL DEFAULT 0, import_snippet TEXT, syscap TEXT, permissions TEXT, device_types TEXT, body TEXT, examples TEXT, members TEXT, source_url TEXT NOT NULL, fetched_at INTEGER NOT NULL);
            CREATE TABLE api_members (id INTEGER PRIMARY KEY AUTOINCREMENT, detail_slug TEXT NOT NULL, module TEXT, parent_name TEXT, member_name TEXT NOT NULL, kind TEXT NOT NULL, declaration TEXT, description TEXT, since_api_level INTEGER, deprecated INTEGER NOT NULL DEFAULT 0, syscap TEXT, permission TEXT, source_url TEXT);
            CREATE TABLE api_docs_meta (key TEXT PRIMARY KEY, value TEXT);
        ";
        // 种子库：两个版本各 1 条
        {
            let c = rusqlite::Connection::open(&seed).unwrap();
            c.execute_batch(schema).unwrap();
            c.execute_batch(
                "INSERT INTO api_docs (kit, declaration, change_type, version_label, fetched_at) VALUES
                   ('Ability Kit','function a(): void;','added','26.0.0 Beta1', 0),
                   ('Ability Kit','function b(): void;','added','6.1.1(24)', 0);
                 INSERT INTO api_docs_meta (key, value) VALUES ('last_refreshed_at', '0');",
            )
            .unwrap();
        }
        // 主库：只有 26.0.0 Beta1 一条（模拟老用户只抓过部分版本）
        {
            let c = rusqlite::Connection::open(&main).unwrap();
            c.execute_batch(schema).unwrap();
            c.execute_batch(
                "INSERT INTO api_docs (kit, declaration, change_type, version_label, fetched_at) VALUES
                   ('Ability Kit','function a(): void;','added','26.0.0 Beta1', 0);",
            )
            .unwrap();
        }

        let n = import_into(&main, &seed).expect("补全导入应成功");
        assert!(n >= 1, "应补入缺失版本，实际插入 {n} 行");

        let c = rusqlite::Connection::open(&main).unwrap();
        let versions: Vec<String> = c
            .prepare("SELECT DISTINCT version_label FROM api_docs ORDER BY version_label")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(versions, vec!["26.0.0 Beta1".to_string(), "6.1.1(24)".to_string()], "应补齐两个版本");

        // 幂等：版本已全覆盖后再次调用应跳过
        let again = import_into(&main, &seed).expect("二次调用应跳过");
        assert_eq!(again, 0, "版本已齐全时不再导入");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
