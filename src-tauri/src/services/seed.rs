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

/// 打开主库新连接执行导入/补全：ATTACH 种子库 → 判断是否需要补 → 逐表
/// INSERT OR IGNORE → 记录 meta。返回本次补全的总行数。
///
/// 补全条件（任一成立即补，均为只增不删）：
/// 1. 种子库存在主库没有的 version_label（主库为空时天然成立）；
/// 2. 种子库已重新生成（`last_refreshed_at` 与主库记录的 `seeded_revision` 不同）。
///
/// 条件 2 的必要性：同一版本的数据会被重新抓取刷新（如 26.0.0 先有 Beta 后有
/// Release，两者 version_label 相同但条目更多）。只按版本集合判断会让老用户
/// 永远拿不到同版本的新增条目。
fn import_into(db_path: &Path, seed: &Path) -> Result<usize, String> {
    let conn = rusqlite::Connection::open(db_path).map_err(|e| e.to_string())?;
    // 显式打开外键：宁可让「向量挂到不存在的文档」这类错误在这里失败，也不要静默写进孤儿行
    conn.execute_batch("PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;")
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

    // 种子库构建批次：以种子库自身的 last_refreshed_at 为修订号
    let seed_revision: String = tx
        .query_row(
            "SELECT COALESCE((SELECT value FROM seed.api_docs_meta WHERE key='last_refreshed_at'), '')",
            [],
            |r| r.get(0),
        )
        .unwrap_or_default();
    let imported_revision: String = tx
        .query_row(
            "SELECT COALESCE((SELECT value FROM main.api_docs_meta WHERE key='seeded_revision'), '')",
            [],
            |r| r.get(0),
        )
        .unwrap_or_default();
    // 修订号为空（旧种子库没有 last_refreshed_at）时不据此触发，避免每次启动全表比对
    let revision_changed = !seed_revision.is_empty() && seed_revision != imported_revision;

    let mut total = 0usize;
    if missing > 0 || revision_changed {
        // api_docs_embeddings 不在这一批：它按「种子自己的 doc_id」存，而主库 api_docs 的
        // id 可能与种子不一致（先做过在线抓取时 id 由主库自增分配），直接 `SELECT *` 会撞
        // `doc_id REFERENCES api_docs(id)` 的外键，**并把整批导入一起回滚**（连已成功的
        // api_details/api_members 都留不下来）。它按自然键重映射，见下方单独处理。
        let tables = ["api_docs", "api_details", "api_members", "api_docs_meta"];
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
        // 向量按自然键重映射到主库 id：种子的 doc_id → 种子 api_docs 的自然键 → 主库同键那一行
        let seed_has_emb: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM seed.sqlite_master WHERE type='table' AND name='api_docs_embeddings'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if seed_has_emb > 0 {
            let n = tx
                .execute(
                    "INSERT OR IGNORE INTO main.api_docs_embeddings (doc_id, model, vector, created_at)
                     SELECT m.id, e.model, e.vector, e.created_at
                       FROM seed.api_docs_embeddings e
                       JOIN seed.api_docs s ON s.id = e.doc_id
                       JOIN main.api_docs m
                         ON m.id = (SELECT MIN(m2.id) FROM main.api_docs m2
                                     WHERE m2.version_label = s.version_label
                                       AND m2.kit = s.kit
                                       AND m2.dts_file IS s.dts_file
                                       AND m2.class_name IS s.class_name
                                       AND m2.declaration = s.declaration)",
                    [],
                )
                .map_err(|e| format!("导入 api_docs_embeddings 失败: {e}"))?;
            total += n;
        }
        tx.execute(
            "INSERT OR REPLACE INTO main.api_docs_meta (key, value) VALUES ('seeded_at', unixepoch())",
            [],
        )
        .map_err(|e| format!("写入导入标记失败: {e}"))?;
        if !seed_revision.is_empty() {
            tx.execute(
                "INSERT OR REPLACE INTO main.api_docs_meta (key, value) VALUES ('seeded_revision', ?1)",
                [seed_revision.as_str()],
            )
            .map_err(|e| format!("写入种子修订号失败: {e}"))?;
        }
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

    /// 主库的 doc_id 与种子不一致时（典型：先做过在线抓取，id 由主库自增分配），
    /// 向量必须**按自然键重映射**到主库那一行；照搬种子的 doc_id 会撞
    /// `doc_id REFERENCES api_docs(id)` 并把整批导入回滚（本机实测到的
    /// `seed_import_error: 导入 api_docs_embeddings 失败: FOREIGN KEY constraint failed`）。
    #[test]
    fn test_seed_remaps_embeddings_when_doc_ids_differ() {
        let dir = std::env::temp_dir().join(format!("deveco-seed-remap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok();
        let main = dir.join("main.db");
        let seed = dir.join("seed.db");

        // 与真实迁移一致：api_docs 有自然键 UNIQUE、embeddings 有指向 api_docs(id) 的外键
        let schema = "
            CREATE TABLE api_docs (id INTEGER PRIMARY KEY AUTOINCREMENT, kit TEXT NOT NULL, dts_file TEXT, module TEXT, class_name TEXT, declaration TEXT NOT NULL, api_name TEXT, change_type TEXT NOT NULL, version_label TEXT NOT NULL, api_level INTEGER, old_declaration TEXT, source_url TEXT, fetched_at INTEGER NOT NULL, UNIQUE(version_label, kit, dts_file, class_name, declaration));
            CREATE TABLE api_details (id INTEGER PRIMARY KEY AUTOINCREMENT, module TEXT NOT NULL, slug TEXT NOT NULL UNIQUE, title TEXT, kit TEXT, since_api_level INTEGER, deprecated INTEGER NOT NULL DEFAULT 0, import_snippet TEXT, syscap TEXT, permissions TEXT, device_types TEXT, body TEXT, examples TEXT, members TEXT, source_url TEXT NOT NULL, fetched_at INTEGER NOT NULL);
            CREATE TABLE api_members (id INTEGER PRIMARY KEY AUTOINCREMENT, detail_slug TEXT NOT NULL, module TEXT, parent_name TEXT, member_name TEXT NOT NULL, kind TEXT NOT NULL, declaration TEXT, description TEXT, since_api_level INTEGER, deprecated INTEGER NOT NULL DEFAULT 0, syscap TEXT, permission TEXT, source_url TEXT);
            CREATE TABLE api_docs_embeddings (doc_id INTEGER PRIMARY KEY REFERENCES api_docs(id) ON DELETE CASCADE, model TEXT NOT NULL, vector BLOB NOT NULL, created_at INTEGER NOT NULL);
            CREATE TABLE api_docs_meta (key TEXT PRIMARY KEY, value TEXT);
        ";
        // 种子：文档 id=1，向量挂在 1 上
        {
            let c = rusqlite::Connection::open(&seed).unwrap();
            c.execute_batch(schema).unwrap();
            c.execute_batch(
                "INSERT INTO api_docs (id, kit, dts_file, class_name, declaration, change_type, version_label, fetched_at)
                   VALUES (1, 'Ability Kit', '@ohos.a.d.ts', 'A', 'function f(): void;', 'added', '26.0.0 Beta1', 0);
                 INSERT INTO api_docs_embeddings (doc_id, model, vector, created_at) VALUES (1, 'bge', x'00', 0);
                 INSERT INTO api_docs_meta (key, value) VALUES ('last_refreshed_at', '0');",
            )
            .unwrap();
        }
        // 主库：同一条 API（同自然键）已在，但 id 是 7（模拟在线抓取先分配过 id）。
        // 于是补入时那条 api_docs 会被自然键 UNIQUE 忽略，而向量必须改挂到 7。
        {
            let c = rusqlite::Connection::open(&main).unwrap();
            c.execute_batch(schema).unwrap();
            c.execute_batch(
                "INSERT INTO api_docs (id, kit, dts_file, class_name, declaration, change_type, version_label, fetched_at)
                   VALUES (7, 'Ability Kit', '@ohos.a.d.ts', 'A', 'function f(): void;', 'added', '26.0.0 Beta1', 0);",
            )
            .unwrap();
        }

        import_into(&main, &seed).expect("id 不一致时也必须导入成功（按自然键重映射）");
        let c = rusqlite::Connection::open(&main).unwrap();
        let doc_id: i64 = c
            .query_row(
                "SELECT e.doc_id FROM api_docs_embeddings e
                   JOIN api_docs d ON d.id = e.doc_id
                  WHERE d.version_label = '26.0.0 Beta1'",
                [],
                |r| r.get(0),
            )
            .expect("向量应挂到重映射后的主库行上");
        assert_eq!(doc_id, 7, "向量必须改挂到主库原有的那一行（id=7）");
        let total: i64 = c
            .query_row("SELECT COUNT(*) FROM api_docs_embeddings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 1, "只应留下一条可映射的向量");
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

    /// 同版本数据被重新抓取（如 26.0.0 从 Beta 口径刷新到 Release 口径）→
    /// 版本集合没变，但条目更多：应按种子修订号补入新增条目，且只增不删、可重入。
    #[test]
    fn test_seed_import_backfills_same_version_refresh() {
        let dir = std::env::temp_dir().join(format!("deveco-seed-rev-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let main = dir.join("main.db");
        let seed = dir.join("seed.db");

        let schema = "
            CREATE TABLE api_docs (id INTEGER PRIMARY KEY AUTOINCREMENT, kit TEXT NOT NULL, dts_file TEXT, module TEXT, class_name TEXT, declaration TEXT NOT NULL, api_name TEXT, change_type TEXT NOT NULL, version_label TEXT NOT NULL, api_level INTEGER, old_declaration TEXT, source_url TEXT, fetched_at INTEGER NOT NULL);
            CREATE TABLE api_details (id INTEGER PRIMARY KEY AUTOINCREMENT, module TEXT NOT NULL, slug TEXT NOT NULL UNIQUE, title TEXT, kit TEXT, since_api_level INTEGER, deprecated INTEGER NOT NULL DEFAULT 0, import_snippet TEXT, syscap TEXT, permissions TEXT, device_types TEXT, body TEXT, examples TEXT, members TEXT, source_url TEXT NOT NULL, fetched_at INTEGER NOT NULL);
            CREATE TABLE api_members (id INTEGER PRIMARY KEY AUTOINCREMENT, detail_slug TEXT NOT NULL, module TEXT, parent_name TEXT, member_name TEXT NOT NULL, kind TEXT NOT NULL, declaration TEXT, description TEXT, since_api_level INTEGER, deprecated INTEGER NOT NULL DEFAULT 0, syscap TEXT, permission TEXT, source_url TEXT);
            CREATE TABLE api_docs_meta (key TEXT PRIMARY KEY, value TEXT);
        ";
        // 种子库：26.0.0 两条，修订号 100
        {
            let c = rusqlite::Connection::open(&seed).unwrap();
            c.execute_batch(schema).unwrap();
            c.execute_batch(
                "INSERT INTO api_docs (kit, declaration, change_type, version_label, fetched_at) VALUES
                   ('Ability Kit','function betaOnly(): void;','added','26.0.0', 0),
                   ('Ability Kit','function releaseOnly(): void;','added','26.0.0', 0);
                 INSERT INTO api_docs_meta (key, value) VALUES ('last_refreshed_at', '100');",
            )
            .unwrap();
        }
        // 主库：同一版本只有 Beta 期的一条，记录的是上一批次修订号 50
        {
            let c = rusqlite::Connection::open(&main).unwrap();
            c.execute_batch(schema).unwrap();
            c.execute_batch(
                "INSERT INTO api_docs (kit, declaration, change_type, version_label, fetched_at) VALUES
                   ('Ability Kit','function betaOnly(): void;','added','26.0.0', 0);
                 INSERT INTO api_docs_meta (key, value) VALUES ('seeded_revision', '50');",
            )
            .unwrap();
        }

        let n = import_into(&main, &seed).expect("同版本刷新应补入新增条目");
        assert!(n >= 1, "应补入 Release 期新增条目，实际插入 {n} 行");
        let c = rusqlite::Connection::open(&main).unwrap();
        let rows: i64 = c
            .query_row("SELECT COUNT(*) FROM api_docs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2, "原有条目保留 + 新增 1 条");
        let rev: String = c
            .query_row(
                "SELECT value FROM api_docs_meta WHERE key='seeded_revision'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rev, "100", "应记录本批种子修订号");

        // 幂等：修订号已一致 → 跳过
        let again = import_into(&main, &seed).expect("二次调用应跳过");
        assert_eq!(again, 0, "修订号已对齐时不再导入");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
