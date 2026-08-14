//! 全量抓取鸿蒙官方 API 知识库（diff + 参考正文）并写入真实 SQLite 库。
//!
//! 用法（在 src-tauri 目录下）：
//!   cargo run --bin full_fetch                       # 用默认生产库路径
//!   cargo run --bin full_fetch -- --seed             # 更新出厂种子库（自动备份 .bak）
//!   cargo run --bin full_fetch -- --seed --diff-only # 只刷新种子库的版本 diff
//!   cargo run --bin full_fetch -- --embed-only       # 只构建/重建语义向量索引
//!   $env:DEVECO_DB="H:\path\to\deveco-switch.db"; cargo run --bin full_fetch
//!
//! 也可只跑某一阶段：
//!   cargo run --bin full_fetch -- --diff-only
//!   cargo run --bin full_fetch -- --ref-only
//!
//! 默认会先抓 14 个版本的 diff（约 200+ 个 Kit 页），再根据 diff 聚合出的模块
//! 列表抓参考正文（约 100~300 个页面），整体需联网，耗时几分钟到十几分钟。
//!
//! 提示：--seed 直接更新 src-tauri/resources/seed/knowledge.db（随安装包发布），
//! 更新前自动备份为 knowledge.db.bak；普通用户请用应用内“刷新”按钮即可。

use std::path::PathBuf;
use std::time::Instant;

use deveco_switch::db::{self, DbState};

/// 解析应用真实数据目录，与 Tauri 的 app_data_dir() 对齐。
/// 标识符来自 tauri.conf.json: com.deveco-switch.app。
fn default_app_db_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").ok()?;
        Some(PathBuf::from(appdata).join("com.deveco-switch.app").join("deveco-switch.db"))
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").ok()?;
        Some(
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("com.deveco-switch.app")
                .join("deveco-switch.db"),
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            if !xdg.is_empty() {
                return Some(
                    PathBuf::from(xdg)
                        .join("com.deveco-switch.app")
                        .join("deveco-switch.db"),
                );
            }
        }
        let home = std::env::var("HOME").ok()?;
        Some(
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("com.deveco-switch.app")
                .join("deveco-switch.db"),
        )
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
    {
        None
    }
}

fn resolve_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("DEVECO_DB") {
        let p = PathBuf::from(p);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        return p;
    }
    if let Some(p) = default_app_db_path() {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        return p;
    }
    PathBuf::from("deveco-switch.db")
}

fn print_diff(p: &deveco_switch::services::harmony_api_diff::RefreshProgress) {
    let pct = if p.total == 0 {
        0
    } else {
        p.current * 100 / p.total
    };
    println!(
        "[diff {:>3}% {}/{}] {} - {}",
        pct, p.current, p.total, p.phase, p.message
    );
}

fn print_ref(p: &deveco_switch::services::harmony_api_ref::RefProgress) {
    let pct = if p.total == 0 {
        0
    } else {
        p.current * 100 / p.total
    };
    println!(
        "[ref  {:>3}% {}/{}] {} - {}",
        pct, p.current, p.total, p.phase, p.message
    );
}

async fn run_diff(db: &DbState) {
    println!("=== 阶段 1/2：刷新 API 版本 diff ===");
    let started = Instant::now();
    let cb: deveco_switch::services::harmony_api_diff::ProgressCb = Box::new(print_diff);
    let report = deveco_switch::services::harmony_api_diff::refresh_all(db, Some(cb))
        .await
        .expect("refresh diff failed");
    println!(
        "✅ diff 完成：版本 {} 个，页面 {} 个，写入 {} 条，错误 {} 条，用时 {:.1?}",
        report.versions_fetched,
        report.pages_fetched,
        report.entries_inserted,
        report.errors.len(),
        started.elapsed()
    );
    if !report.errors.is_empty() {
        let preview: Vec<&String> = report.errors.iter().take(10).collect();
        println!("   前 10 条错误：{:?}", preview);
    }
}

async fn run_ref(db: &DbState) {
    println!("=== 阶段 2/2：刷新 API 参考正文 ===");
    let started = Instant::now();
    let cb: deveco_switch::services::harmony_api_ref::ProgressCb = Box::new(print_ref);
    let report = deveco_switch::services::harmony_api_ref::refresh_all(db, Some(cb))
        .await
        .expect("refresh ref failed");
    println!(
        "✅ ref 完成：页面抓取 {} 个，写入 {} 个，成员 {} 条，错误 {} 条，用时 {:.1?}",
        report.pages_fetched,
        report.pages_stored,
        report.members_stored,
        report.errors.len(),
        started.elapsed()
    );
    if !report.errors.is_empty() {
        let preview: Vec<&String> = report.errors.iter().take(10).collect();
        println!("   前 10 条错误：{:?}", preview);
    }
}

fn print_counts(db_path: &std::path::Path) {
    let conn = rusqlite::Connection::open(db_path).expect("open db for counts");
    let diff_total: i64 = conn
        .query_row("SELECT COUNT(*) FROM api_docs", [], |r| r.get(0))
        .unwrap_or(-1);
    let diff_versions: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT version_label) FROM api_docs",
            [],
            |r| r.get(0),
        )
        .unwrap_or(-1);
    let diff_kits: i64 = conn
        .query_row("SELECT COUNT(DISTINCT kit) FROM api_docs", [], |r| r.get(0))
        .unwrap_or(-1);
    let diff_modules: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT module) FROM api_docs WHERE module IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(-1);
    let (d_count, m_count) =
        deveco_switch::services::harmony_api_ref::count_details(&conn).unwrap_or((0, 0));
    println!("=== 数据库统计 ===");
    println!("  api_docs      : {diff_total} 行（{diff_versions} 版本 / {diff_kits} Kit / {diff_modules} 模块）");
    println!("  api_details   : {d_count} 行");
    println!("  api_members   : {m_count} 行");

    // 前 5 个最新版本的 added 条目示例
    let mut stmt = conn
        .prepare(
            "SELECT version_label, change_type, COUNT(*) FROM api_docs
             GROUP BY version_label, change_type
             ORDER BY MIN(api_level) DESC, version_label DESC LIMIT 30",
        )
        .expect("prepare stats");
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })
        .expect("query stats");
    println!("--- 各版本变更分布（最多 30 行）---");
    for r in rows.flatten() {
        println!("  {:<24} {:<12} {}", r.0, r.1, r.2);
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let diff_only = args.iter().any(|a| a == "--diff-only");
    let ref_only = args.iter().any(|a| a == "--ref-only");
    let seed_mode = args.iter().any(|a| a == "--seed");
    let embed_only = args.iter().any(|a| a == "--embed-only");

    let db_path = resolve_db_path();
    println!("📂 数据库路径：{}", db_path.display());

    // --seed 模式：直接更新出厂种子库（src-tauri/resources/seed/knowledge.db），
    // 与 --diff-only / --ref-only 可组合。更新前自动备份一份 .bak。
    if seed_mode {
        let seed_path = seed_db_path();
        println!("🌱 更新出厂种子库：{}", seed_path.display());
        if seed_path.is_file() {
            let bak = seed_path.with_extension("db.bak");
            std::fs::copy(&seed_path, &bak).map_err(|e| {
                eprintln!("备份种子库失败: {e}");
                std::process::exit(3);
            }).unwrap();
            println!("  已备份旧种子库 → {}", bak.display());
        }
        // 强制使用种子库路径
        let p = seed_path.clone();
        let conn = db::init(&p).expect("init seed db");
        let db = DbState(std::sync::Arc::new(conn));
        run_fetch(&db, &p, diff_only, ref_only).await;
        // 种子库同步建向量索引（随安装包分发，用户开箱即有语义检索）
        run_embed(&db);
        std::process::exit(0);
    }

    // 初始化（会自动跑全部 migrations，包括 028/029）
    let conn = db::init(&db_path).expect("init db");
    let db = DbState(std::sync::Arc::new(conn));

    if diff_only && ref_only {
        eprintln!("不能同时指定 --diff-only 与 --ref-only");
        std::process::exit(2);
    }

    if embed_only {
        run_embed(&db);
        return;
    }

    run_fetch(&db, &db_path, diff_only, ref_only).await;
}

/// 出厂种子库路径：src-tauri/resources/seed/knowledge.db
fn seed_db_path() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.join("resources").join("seed").join("knowledge.db")
}

/// 建向量索引：对 db 内全部 api_docs 编码写入 api_docs_embeddings。
/// 模型不可用时打印提示并返回 false（不阻塞其他阶段）。
/// 仅在 embedding feature 下实现；未启用时降级为提示（full_fetch 默认不依赖 candle）。
#[cfg(feature = "embedding")]
fn run_embed(db: &DbState) -> bool {
    println!("=== 阶段 3：构建语义向量索引（embedding）===");
    let started = Instant::now();
    let conn = match db.0.lock() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("获取数据库连接失败: {e}");
            return false;
        }
    };
    match deveco_switch::services::embedding::build_index(&conn, 128) {
        Ok((inserted, skipped)) => {
            println!(
                "✅ 向量索引完成：新增 {inserted} 条，跳过 {skipped} 条，用时 {:.1?}",
                started.elapsed()
            );
            inserted > 0
        }
        Err(e) => {
            eprintln!("⚠️ 向量索引失败（不影响 diff/ref 数据）：{e}");
            eprintln!("   提示：确保 resources/embedding/bge-small-zh-v1.5/ 下有模型文件（随安装包分发）");
            false
        }
    }
}

#[cfg(not(feature = "embedding"))]
fn run_embed(_db: &DbState) -> bool {
    eprintln!("⚠️ 当前构建未启用 embedding feature，跳过向量索引。");
    eprintln!("   如需建索引：cargo run --bin full_fetch --features embedding -- --embed-only");
    false
}

async fn run_fetch(db: &DbState, db_path: &std::path::Path, diff_only: bool, ref_only: bool) {
    let before = {
        let c = db.0.lock().unwrap();
        let d: i64 = c
            .query_row("SELECT COUNT(*) FROM api_docs", [], |r| r.get(0))
            .unwrap_or(0);
        d
    };
    println!("抓取前 api_docs 行数：{before}");

    let total_started = Instant::now();
    if !ref_only {
        run_diff(db).await;
    }
    if !diff_only {
        run_ref(db).await;
    }

    println!("全部完成，总用时 {:.1?}", total_started.elapsed());
    print_counts(db_path);
}
