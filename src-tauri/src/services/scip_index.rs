//! Bounded, dependency-free SCIP importer.
//!
//! SCIP indexes can be many gigabytes. The top-level protobuf is therefore read one
//! `Document` at a time; a single document is capped so corrupt lengths cannot force
//! an unbounded allocation. Unknown protobuf fields are skipped for forward compatibility.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read};
use std::path::{Component, Path};
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use rusqlite::{params, Connection, TransactionBehavior};
use serde::Serialize;

const MAX_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ITEM_BYTES: usize = 8 * 1024 * 1024;
const MAX_STRING_BYTES: usize = 1024 * 1024;
const DEFINITION_ROLE: u64 = 1;
// SCIP SymbolRole.ForwardDefinition：前向声明也是符号的合法定义位置，不应被当作引用。
const FORWARD_DEFINITION_ROLE: u64 = 64;
const DEFINITION_OR_FORWARD_ROLE: u64 = DEFINITION_ROLE | FORWARD_DEFINITION_ROLE;
const DOCUMENTS_PER_TRANSACTION: usize = 256;
const EDGE_BUILD_BATCH_ROWS: i64 = 50_000;
const CLEANUP_BATCH_ROWS: i64 = 50_000;

#[derive(Debug, Clone, Serialize)]
pub struct ScipImportStats {
    pub index_path: String,
    pub skipped_unchanged: bool,
    pub documents: usize,
    pub definitions: usize,
    pub references: usize,
    pub resolved_references: usize,
    pub ignored_documents: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ScipIndexStatus {
    pub state: String,
    pub index_path: Option<String>,
    pub documents: usize,
    pub definitions: usize,
    pub references: usize,
    pub resolved_references: usize,
    pub imported_ago_secs: Option<u64>,
}

#[derive(Default)]
struct Occurrence {
    symbol: String,
    line: u64,
    column: u64,
    roles: u64,
}

#[derive(Default)]
struct SymbolInfo {
    symbol: String,
    display_name: String,
    kind: u64,
}

fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn stamp(path: &Path) -> Result<(u64, u64), String> {
    let meta = fs::metadata(path).map_err(|e| format!("读取 SCIP 索引元数据失败：{e}"))?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0);
    Ok((meta.len(), mtime))
}

fn discovered_index(root: &Path) -> Option<(String, std::path::PathBuf)> {
    ["index.scip", ".scip/index.scip"]
        .into_iter()
        .map(|rel| (rel.to_string(), root.join(rel)))
        .find(|(_, path)| {
            fs::symlink_metadata(path)
                .is_ok_and(|meta| meta.is_file() && !meta.file_type().is_symlink())
        })
}

pub fn status(root: &Path, database: &Path) -> ScipIndexStatus {
    let discovered = discovered_index(root);
    let Ok(conn) = Connection::open(database) else {
        return ScipIndexStatus {
            state: if discovered.is_some() {
                "available_not_imported"
            } else {
                "not_found"
            }
            .into(),
            index_path: discovered.map(|(rel, _)| rel),
            ..Default::default()
        };
    };
    let has_state = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='scip_import_state')",
        [], |row| row.get::<_, bool>(0),
    ).unwrap_or(false);
    if !has_state {
        return ScipIndexStatus {
            state: if discovered.is_some() {
                "available_not_imported"
            } else {
                "not_found"
            }
            .into(),
            index_path: discovered.map(|(rel, _)| rel),
            ..Default::default()
        };
    }
    let stored = conn.query_row(
        "SELECT index_path, index_size, index_mtime_ns, imported_at, document_count,
                definition_count, reference_count, edge_count FROM scip_import_state WHERE id=1",
        [],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
            ))
        },
    );
    let Ok((path, size, mtime, imported_at, documents, definitions, references, edges)) = stored
    else {
        return ScipIndexStatus {
            state: if discovered.is_some() {
                "available_not_imported"
            } else {
                "not_found"
            }
            .into(),
            index_path: discovered.map(|(rel, _)| rel),
            ..Default::default()
        };
    };
    let current = stamp(&root.join(&path)).ok();
    let state = if current == Some((size.max(0) as u64, mtime.max(0) as u64)) {
        "active"
    } else if current.is_some() || discovered.is_some() {
        "stale_index"
    } else {
        "imported_source_missing"
    };
    ScipIndexStatus {
        state: state.into(),
        index_path: Some(path),
        documents: documents.max(0) as usize,
        definitions: definitions.max(0) as usize,
        references: references.max(0) as usize,
        resolved_references: edges.max(0) as usize,
        imported_ago_secs: Some(
            now_nanos()
                .saturating_div(1_000_000_000)
                .saturating_sub(imported_at.max(0) as u64),
        ),
    }
}

fn read_varint<R: Read>(reader: &mut R) -> io::Result<Option<u64>> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let mut byte = [0u8; 1];
        match reader.read_exact(&mut byte) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof && shift == 0 => {
                return Ok(None)
            }
            Err(error) => return Err(error),
        }
        if shift == 63 && byte[0] > 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "protobuf varint overflow",
            ));
        }
        value |= ((byte[0] & 0x7f) as u64) << shift;
        if byte[0] & 0x80 == 0 {
            return Ok(Some(value));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "protobuf varint too long",
    ))
}

fn read_len<R: Read>(reader: &mut R, cap: usize) -> io::Result<Vec<u8>> {
    let len = read_varint(reader)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "missing protobuf length"))?;
    if len > cap as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("protobuf field exceeds {cap} byte bound"),
        ));
    }
    let mut bytes = vec![0; len as usize];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn skip_field<R: Read>(reader: &mut R, wire: u64) -> io::Result<()> {
    match wire {
        0 => {
            read_varint(reader)?
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "truncated varint"))?;
        }
        1 => {
            io::copy(&mut reader.take(8), &mut io::sink())?;
        }
        2 => {
            let len = read_varint(reader)?.ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "missing field length")
            })?;
            let copied = io::copy(&mut reader.take(len), &mut io::sink())?;
            if copied != len {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated field",
                ));
            }
        }
        5 => {
            io::copy(&mut reader.take(4), &mut io::sink())?;
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported protobuf wire type {wire}"),
            ))
        }
    }
    Ok(())
}

fn parse_range(bytes: &[u8]) -> io::Result<(u64, u64)> {
    let mut input = bytes;
    let mut values = Vec::with_capacity(4);
    while let Some(value) = read_varint(&mut input)? {
        values.push(value);
        if values.len() > 4 {
            break;
        }
    }
    match values.as_slice() {
        [line, column, ..] if matches!(values.len(), 3 | 4) => Ok((*line, *column)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid SCIP range",
        )),
    }
}

fn parse_single_line_range(bytes: &[u8]) -> io::Result<(u64, u64)> {
    let mut input = bytes;
    let (mut line, mut column) = (0, 0);
    while let Some(key) = read_varint(&mut input)? {
        let (field, wire) = (key >> 3, key & 7);
        match (field, wire) {
            (1, 0) => line = read_varint(&mut input)?.unwrap_or(0),
            (2, 0) => column = read_varint(&mut input)?.unwrap_or(0),
            _ => skip_field(&mut input, wire)?,
        }
    }
    Ok((line, column))
}

fn parse_multi_line_range(bytes: &[u8]) -> io::Result<(u64, u64)> {
    parse_single_line_range(bytes)
}

fn parse_occurrence(bytes: &[u8]) -> io::Result<Occurrence> {
    let mut input = bytes;
    let mut occurrence = Occurrence::default();
    let mut deprecated_range = None;
    let mut typed_range = None;
    while let Some(key) = read_varint(&mut input)? {
        let (field, wire) = (key >> 3, key & 7);
        match (field, wire) {
            (1, 2) => deprecated_range = Some(parse_range(&read_len(&mut input, 64)?)?),
            (2, 2) => {
                occurrence.symbol =
                    String::from_utf8_lossy(&read_len(&mut input, MAX_STRING_BYTES)?).into_owned()
            }
            (3, 0) => occurrence.roles = read_varint(&mut input)?.unwrap_or(0),
            (8, 2) => typed_range = Some(parse_single_line_range(&read_len(&mut input, 128)?)?),
            (9, 2) => typed_range = Some(parse_multi_line_range(&read_len(&mut input, 128)?)?),
            _ => skip_field(&mut input, wire)?,
        }
    }
    let (line, column) = typed_range.or(deprecated_range).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "SCIP occurrence has no range")
    })?;
    occurrence.line = line;
    occurrence.column = column;
    Ok(occurrence)
}

fn parse_symbol_info(bytes: &[u8]) -> io::Result<SymbolInfo> {
    let mut input = bytes;
    let mut info = SymbolInfo::default();
    while let Some(key) = read_varint(&mut input)? {
        let (field, wire) = (key >> 3, key & 7);
        match (field, wire) {
            (1, 2) => {
                info.symbol =
                    String::from_utf8_lossy(&read_len(&mut input, MAX_STRING_BYTES)?).into_owned()
            }
            (5, 0) => info.kind = read_varint(&mut input)?.unwrap_or(0),
            (6, 2) => {
                info.display_name =
                    String::from_utf8_lossy(&read_len(&mut input, MAX_STRING_BYTES)?).into_owned()
            }
            _ => skip_field(&mut input, wire)?,
        }
    }
    Ok(info)
}

fn safe_relative_path(value: &str) -> Option<String> {
    let path = Path::new(value);
    if path.is_absolute() {
        return None;
    }
    let mut clean = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => clean.push(value.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!clean.is_empty()).then(|| clean.join("/"))
}

fn symbol_key(path: &str, symbol: &str) -> String {
    if symbol.starts_with("local ") {
        format!("{path}\0{symbol}")
    } else {
        symbol.to_owned()
    }
}

fn fallback_name(symbol: &str) -> String {
    symbol
        .split_whitespace()
        .last()
        .unwrap_or(symbol)
        .trim_end_matches(|ch: char| matches!(ch, '.' | '#' | '/' | ')' | '('))
        .to_string()
}

fn parse_document(
    bytes: &[u8],
    conn: &Connection,
    import_id: i64,
    root: &Path,
    stats: &mut ScipImportStats,
) -> Result<(), String> {
    let mut input = bytes;
    let mut path = String::new();
    let mut occurrences = Vec::new();
    let mut infos = Vec::new();
    while let Some(key) = read_varint(&mut input).map_err(|e| e.to_string())? {
        let (field, wire) = (key >> 3, key & 7);
        match (field, wire) {
            (1, 2) => {
                path = String::from_utf8_lossy(
                    &read_len(&mut input, MAX_STRING_BYTES).map_err(|e| e.to_string())?,
                )
                .into_owned()
            }
            (2, 2) => occurrences.push(
                parse_occurrence(&read_len(&mut input, MAX_ITEM_BYTES).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?,
            ),
            (3, 2) => infos.push(
                parse_symbol_info(
                    &read_len(&mut input, MAX_ITEM_BYTES).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?,
            ),
            _ => skip_field(&mut input, wire).map_err(|e| e.to_string())?,
        }
    }
    let Some(path) = safe_relative_path(&path) else {
        stats.ignored_documents += 1;
        return Ok(());
    };
    let absolute = root.join(&path);
    let meta = match fs::symlink_metadata(&absolute) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => meta,
        _ => {
            stats.ignored_documents += 1;
            return Ok(());
        }
    };
    let file_mtime = meta
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0);
    let catalog_matches = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM files WHERE path=?1 AND state='indexed' AND size=?2 AND mtime_ns=?3)",
        params![path, meta.len() as i64, file_mtime as i64], |row| row.get::<_, bool>(0),
    ).unwrap_or(false);
    if !catalog_matches {
        stats.ignored_documents += 1;
        return Ok(());
    }
    for info in infos {
        if info.symbol.is_empty() {
            continue;
        }
        conn.execute(
            "INSERT OR REPLACE INTO scip_import_symbols(import_id, symbol_key, display_name, kind) VALUES(?1, ?2, ?3, ?4)",
            params![import_id, symbol_key(&path, &info.symbol), info.display_name, info.kind as i64],
        ).map_err(|e| e.to_string())?;
    }
    for occurrence in occurrences {
        if occurrence.symbol.is_empty() {
            continue;
        }
        let key = symbol_key(&path, &occurrence.symbol);
        if occurrence.roles & DEFINITION_OR_FORWARD_ROLE != 0 {
            conn.execute(
                "INSERT OR IGNORE INTO scip_import_definitions(import_id, symbol_key, file, line, column, file_size, file_mtime_ns, fallback_name) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![import_id, key, path, occurrence.line.saturating_add(1) as i64, occurrence.column.saturating_add(1) as i64, meta.len() as i64, file_mtime as i64, fallback_name(&occurrence.symbol)],
            ).map_err(|e| e.to_string())?;
            stats.definitions += 1;
        } else {
            conn.execute(
                "INSERT OR IGNORE INTO scip_import_occurrences(import_id, symbol_key, file, line, column, file_size, file_mtime_ns) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![import_id, key, path, occurrence.line.saturating_add(1) as i64, occurrence.column.saturating_add(1) as i64, meta.len() as i64, file_mtime as i64],
            ).map_err(|e| e.to_string())?;
            stats.references += 1;
        }
    }
    stats.documents += 1;
    Ok(())
}

fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS scip_import_state (
           id INTEGER PRIMARY KEY CHECK(id=1), active_import_id INTEGER NOT NULL,
           index_path TEXT NOT NULL, index_size INTEGER NOT NULL, index_mtime_ns INTEGER NOT NULL,
           imported_at INTEGER NOT NULL, document_count INTEGER NOT NULL,
           definition_count INTEGER NOT NULL, reference_count INTEGER NOT NULL, edge_count INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS scip_import_symbols (
           import_id INTEGER NOT NULL, symbol_key TEXT NOT NULL, display_name TEXT NOT NULL, kind INTEGER NOT NULL,
           PRIMARY KEY(import_id, symbol_key)
         );
         CREATE TABLE IF NOT EXISTS scip_import_definitions (
           import_id INTEGER NOT NULL, symbol_key TEXT NOT NULL, file TEXT NOT NULL, line INTEGER NOT NULL,
           column INTEGER NOT NULL, file_size INTEGER NOT NULL, file_mtime_ns INTEGER NOT NULL, fallback_name TEXT NOT NULL,
           PRIMARY KEY(import_id, symbol_key)
         );
         CREATE TABLE IF NOT EXISTS scip_import_occurrences (
           import_id INTEGER NOT NULL, symbol_key TEXT NOT NULL, file TEXT NOT NULL, line INTEGER NOT NULL,
           column INTEGER NOT NULL, file_size INTEGER NOT NULL, file_mtime_ns INTEGER NOT NULL,
           PRIMARY KEY(import_id, file, line, column, symbol_key)
         );
         CREATE INDEX IF NOT EXISTS idx_scip_occurrence_symbol ON scip_import_occurrences(import_id, symbol_key);
         CREATE TABLE IF NOT EXISTS scip_reference_edges (
           import_id INTEGER NOT NULL, source_file TEXT NOT NULL, source_name TEXT NOT NULL, source_line INTEGER NOT NULL,
           occurrence_line INTEGER NOT NULL, occurrence_column INTEGER NOT NULL, source_size INTEGER NOT NULL,
           source_mtime_ns INTEGER NOT NULL, target_file TEXT NOT NULL, target_name TEXT NOT NULL, target_line INTEGER NOT NULL,
           target_size INTEGER NOT NULL, target_mtime_ns INTEGER NOT NULL, symbol_key TEXT NOT NULL,
           PRIMARY KEY(import_id, source_file, occurrence_line, occurrence_column, symbol_key)
         );
         CREATE INDEX IF NOT EXISTS idx_scip_edges_source ON scip_reference_edges(import_id, source_file, source_name, source_line);
         CREATE INDEX IF NOT EXISTS idx_scip_edges_target ON scip_reference_edges(import_id, target_file, target_name, target_line);"
    )
}

pub fn import(root: &Path, database: &Path, index: &Path) -> Result<ScipImportStats, String> {
    let canonical_root = fs::canonicalize(root).map_err(|e| format!("项目目录不可访问：{e}"))?;
    let canonical_index = fs::canonicalize(index).map_err(|e| format!("SCIP 索引不可访问：{e}"))?;
    if !canonical_index.starts_with(&canonical_root) {
        return Err("SCIP 索引必须位于项目目录内".into());
    }
    if fs::symlink_metadata(index)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("拒绝读取符号链接形式的 SCIP 索引".into());
    }
    let lock_path = database.with_extension("scip-import.lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| format!("创建 SCIP 导入锁失败：{e}"))?;
    lock_file
        .try_lock_exclusive()
        .map_err(|_| "已有 SCIP 导入正在运行，请稍后重试".to_string())?;
    let (index_size, index_mtime) = stamp(&canonical_index)?;
    let mut conn = Connection::open(database).map_err(|e| format!("打开结构索引失败：{e}"))?;
    ensure_schema(&conn).map_err(|e| format!("初始化 SCIP 表失败：{e}"))?;
    let display_path = canonical_index
        .strip_prefix(&canonical_root)
        .unwrap_or(&canonical_index)
        .to_string_lossy()
        .replace('\\', "/");
    if conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM scip_import_state WHERE id=1 AND index_path=?1 AND index_size=?2 AND index_mtime_ns=?3)",
        params![display_path, index_size as i64, index_mtime as i64], |row| row.get::<_, bool>(0),
    ).unwrap_or(false) {
        let (documents, definitions, references, resolved) = conn.query_row(
            "SELECT document_count, definition_count, reference_count, edge_count FROM scip_import_state WHERE id=1", [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)),
        ).map_err(|e| e.to_string())?;
        return Ok(ScipImportStats { index_path: display_path, skipped_unchanged: true, documents: documents as usize, definitions: definitions as usize, references: references as usize, resolved_references: resolved as usize, ignored_documents: 0 });
    }
    let import_id = now_nanos().min(i64::MAX as u64) as i64;
    let mut stats = ScipImportStats {
        index_path: display_path.clone(),
        skipped_unchanged: false,
        documents: 0,
        definitions: 0,
        references: 0,
        resolved_references: 0,
        ignored_documents: 0,
    };
    let parse_result = (|| -> Result<(), String> {
        let file = File::open(&canonical_index).map_err(|e| format!("打开 SCIP 索引失败：{e}"))?;
        let mut reader = BufReader::with_capacity(256 * 1024, file);
        let mut documents_in_transaction = 0usize;
        while let Some(key) =
            read_varint(&mut reader).map_err(|e| format!("解析 SCIP 顶层字段失败：{e}"))?
        {
            let (field, wire) = (key >> 3, key & 7);
            if field == 2 && wire == 2 {
                let len = read_varint(&mut reader)
                    .map_err(|e| e.to_string())?
                    .ok_or("SCIP document 缺少长度")?;
                if len > MAX_DOCUMENT_BYTES {
                    return Err(format!(
                        "单个 SCIP document 超过 {} MiB 安全上限",
                        MAX_DOCUMENT_BYTES / 1024 / 1024
                    ));
                }
                let mut bytes = vec![0; len as usize];
                reader
                    .read_exact(&mut bytes)
                    .map_err(|e| format!("SCIP document 被截断：{e}"))?;
                if documents_in_transaction == 0 {
                    conn.execute_batch("BEGIN DEFERRED")
                        .map_err(|e| format!("开启 SCIP 批事务失败：{e}"))?;
                }
                parse_document(&bytes, &conn, import_id, &canonical_root, &mut stats)?;
                documents_in_transaction += 1;
                if documents_in_transaction == DOCUMENTS_PER_TRANSACTION {
                    conn.execute_batch("COMMIT")
                        .map_err(|e| format!("提交 SCIP 批事务失败：{e}"))?;
                    documents_in_transaction = 0;
                }
            } else {
                skip_field(&mut reader, wire).map_err(|e| format!("跳过 SCIP 字段失败：{e}"))?;
            }
        }
        if documents_in_transaction > 0 {
            conn.execute_batch("COMMIT")
                .map_err(|e| format!("提交 SCIP 批事务失败：{e}"))?;
        }
        Ok(())
    })();
    if let Err(error) = parse_result {
        let _ = conn.execute_batch("ROLLBACK");
        let _ = conn.execute(
            "DELETE FROM scip_import_symbols WHERE import_id=?1",
            [import_id],
        );
        let _ = conn.execute(
            "DELETE FROM scip_import_definitions WHERE import_id=?1",
            [import_id],
        );
        let _ = conn.execute(
            "DELETE FROM scip_import_occurrences WHERE import_id=?1",
            [import_id],
        );
        return Err(error);
    }
    if stamp(&canonical_index)? != (index_size, index_mtime) {
        let _ = conn.execute(
            "DELETE FROM scip_import_symbols WHERE import_id=?1",
            [import_id],
        );
        let _ = conn.execute(
            "DELETE FROM scip_import_definitions WHERE import_id=?1",
            [import_id],
        );
        let _ = conn.execute(
            "DELETE FROM scip_import_occurrences WHERE import_id=?1",
            [import_id],
        );
        return Err("SCIP 索引在导入期间被外部工具改写，已放弃未完成代次；请重试".into());
    }
    let (min_occurrence_rowid, max_occurrence_rowid) = conn
        .query_row(
            "SELECT COALESCE(MIN(rowid), 1), COALESCE(MAX(rowid), 0)
         FROM scip_import_occurrences WHERE import_id=?1",
            [import_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(|e| format!("读取 SCIP 引用游标失败：{e}"))?;
    let mut occurrence_rowid = min_occurrence_rowid.saturating_sub(1);
    while occurrence_rowid < max_occurrence_rowid {
        let batch_end = occurrence_rowid.saturating_add(EDGE_BUILD_BATCH_ROWS);
        conn.execute(
            "INSERT INTO scip_reference_edges(import_id, source_file, source_name, source_line, occurrence_line, occurrence_column, source_size, source_mtime_ns, target_file, target_name, target_line, target_size, target_mtime_ns, symbol_key)
         SELECT o.import_id, o.file,
           COALESCE((SELECT s.name FROM symbols s WHERE s.file=o.file AND s.line<=o.line AND s.end_line>=o.line ORDER BY CASE WHEN s.role='logic' THEN 0 ELSE 1 END, (s.end_line-s.line), s.line DESC LIMIT 1), '<file>'),
           COALESCE((SELECT s.line FROM symbols s WHERE s.file=o.file AND s.line<=o.line AND s.end_line>=o.line ORDER BY CASE WHEN s.role='logic' THEN 0 ELSE 1 END, (s.end_line-s.line), s.line DESC LIMIT 1), o.line),
           o.line, o.column, o.file_size, o.file_mtime_ns, d.file,
           COALESCE(NULLIF((SELECT s.name FROM symbols s WHERE s.file=d.file AND s.line<=d.line AND s.end_line>=d.line ORDER BY (s.end_line-s.line), s.line DESC LIMIT 1), ''), NULLIF(i.display_name, ''), d.fallback_name),
           COALESCE((SELECT s.line FROM symbols s WHERE s.file=d.file AND s.line<=d.line AND s.end_line>=d.line ORDER BY (s.end_line-s.line), s.line DESC LIMIT 1), d.line),
           d.file_size, d.file_mtime_ns, o.symbol_key
         FROM scip_import_occurrences o JOIN scip_import_definitions d ON d.import_id=o.import_id AND d.symbol_key=o.symbol_key
         LEFT JOIN scip_import_symbols i ON i.import_id=o.import_id AND i.symbol_key=o.symbol_key
         WHERE o.import_id=?1 AND o.rowid>?2 AND o.rowid<=?3
           AND (o.file<>d.file OR o.line<>d.line OR o.column<>d.column)",
            params![import_id, occurrence_rowid, batch_end],
        ).map_err(|e| format!("分块解析 SCIP 引用关系失败：{e}"))?;
        occurrence_rowid = batch_end;
    }
    stats.resolved_references = conn
        .query_row(
            "SELECT COUNT(*) FROM scip_reference_edges WHERE import_id=?1",
            [import_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as usize;
    if stamp(&canonical_index)? != (index_size, index_mtime) {
        let _ = conn.execute(
            "DELETE FROM scip_reference_edges WHERE import_id=?1",
            [import_id],
        );
        let _ = conn.execute(
            "DELETE FROM scip_import_symbols WHERE import_id=?1",
            [import_id],
        );
        let _ = conn.execute(
            "DELETE FROM scip_import_definitions WHERE import_id=?1",
            [import_id],
        );
        let _ = conn.execute(
            "DELETE FROM scip_import_occurrences WHERE import_id=?1",
            [import_id],
        );
        return Err("SCIP 索引在关系构建期间被外部工具改写，已保留上一有效代次；请重试".into());
    }
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO scip_import_state(id, active_import_id, index_path, index_size, index_mtime_ns, imported_at, document_count, definition_count, reference_count, edge_count)
         VALUES(1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET active_import_id=excluded.active_import_id, index_path=excluded.index_path, index_size=excluded.index_size, index_mtime_ns=excluded.index_mtime_ns, imported_at=excluded.imported_at, document_count=excluded.document_count, definition_count=excluded.definition_count, reference_count=excluded.reference_count, edge_count=excluded.edge_count",
        params![import_id, display_path, index_size as i64, index_mtime as i64, (now_nanos()/1_000_000_000) as i64, stats.documents as i64, stats.definitions as i64, stats.references as i64, stats.resolved_references as i64],
    ).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    // The state switch makes old generations invisible. Reclaim them in short batches
    // so a multi-million-edge cleanup does not monopolize the catalog write lock.
    for table in [
        "scip_reference_edges",
        "scip_import_occurrences",
        "scip_import_definitions",
        "scip_import_symbols",
    ] {
        loop {
            let sql = format!(
                "DELETE FROM {table} WHERE rowid IN (
                   SELECT rowid FROM {table} WHERE import_id<>?1 LIMIT ?2
                 )"
            );
            let deleted = conn
                .execute(&sql, params![import_id, CLEANUP_BATCH_ROWS])
                .unwrap_or(0);
            if deleted < CLEANUP_BATCH_ROWS as usize {
                break;
            }
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufWriter, Write};

    fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            out.push(if value == 0 { byte } else { byte | 0x80 });
            if value == 0 {
                break;
            }
        }
        out
    }

    fn len_field(number: u64, value: &[u8]) -> Vec<u8> {
        [
            varint((number << 3) | 2),
            varint(value.len() as u64),
            value.to_vec(),
        ]
        .concat()
    }

    fn occurrence(symbol: &str, line: u64, column: u64, definition: bool) -> Vec<u8> {
        let range = [varint(line), varint(column), varint(column + 1)].concat();
        [
            len_field(1, &range),
            len_field(2, symbol.as_bytes()),
            [
                varint(3 << 3),
                varint(if definition { DEFINITION_ROLE } else { 0 }),
            ]
            .concat(),
        ]
        .concat()
    }

    fn document(path: &str, occurrences: &[Vec<u8>]) -> Vec<u8> {
        let mut out = len_field(1, path.as_bytes());
        for occurrence in occurrences {
            out.extend(len_field(2, occurrence));
        }
        out
    }

    fn occurrence_with_roles(symbol: &str, line: u64, column: u64, roles: u64) -> Vec<u8> {
        let range = [varint(line), varint(column), varint(column + 1)].concat();
        [
            len_field(1, &range),
            len_field(2, symbol.as_bytes()),
            [varint(3 << 3), varint(roles)].concat(),
        ]
        .concat()
    }

    #[test]
    fn forward_definition_counts_as_definition_not_reference() {
        let root = std::env::temp_dir().join(format!("harmony-scip-fwd-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("src")).unwrap();
        let source = root.join("src/source.ts");
        let target = root.join("src/target.ts");
        fs::write(&source, "function caller() { fetch(); }\n").unwrap();
        fs::write(&target, "declare function fetch(): void;\n").unwrap();
        let source_meta = fs::metadata(&source).unwrap();
        let target_meta = fs::metadata(&target).unwrap();
        let source_mtime = source_meta.modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_nanos() as i64;
        let target_mtime = target_meta.modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_nanos() as i64;
        let database = root.join("catalog.sqlite3");
        let conn = Connection::open(&database).unwrap();
        conn.execute_batch(
            "CREATE TABLE files(path TEXT PRIMARY KEY, state TEXT, size INTEGER, mtime_ns INTEGER);
             CREATE TABLE symbols(file TEXT, name TEXT, line INTEGER, end_line INTEGER, role TEXT);",
        ).unwrap();
        conn.execute(
            "INSERT INTO files VALUES('src/source.ts','indexed',?1,?2)",
            params![source_meta.len() as i64, source_mtime],
        ).unwrap();
        conn.execute(
            "INSERT INTO files VALUES('src/target.ts','indexed',?1,?2)",
            params![target_meta.len() as i64, target_mtime],
        ).unwrap();
        conn.execute(
            "INSERT INTO symbols VALUES('src/source.ts','caller',1,1,'logic')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO symbols VALUES('src/target.ts','fetch',1,1,'logic')",
            [],
        ).unwrap();
        drop(conn);

        let symbol = "scip-typescript npm demo 1.0 fetch().";
        let mut bytes = len_field(
            2,
            &document(
                "src/target.ts",
                &[occurrence_with_roles(symbol, 0, 9, FORWARD_DEFINITION_ROLE)],
            ),
        );
        for column in 0..3u64 {
            bytes.extend(len_field(
                2,
                &document("src/source.ts", &[occurrence_with_roles(symbol, 0, 20 + column, 0)]),
            ));
        }
        let index = root.join("index.scip");
        fs::write(&index, bytes).unwrap();
        let stats = import(&root, &database, &index).unwrap();
        // 前向声明被记为定义位置；三个引用都解析到它，而不是把前向声明误算成第 4 个引用。
        assert_eq!(stats.definitions, 1);
        assert_eq!(stats.references, 3);
        assert_eq!(stats.resolved_references, 3);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn rejects_parent_paths_and_namespaces_local_symbols() {
        assert_eq!(safe_relative_path("src/a.ts").as_deref(), Some("src/a.ts"));
        assert_eq!(safe_relative_path("../a.ts"), None);
        assert_ne!(symbol_key("a.ts", "local 1"), symbol_key("b.ts", "local 1"));
    }

    #[test]
    fn reads_deprecated_and_typed_ranges() {
        let deprecated = [varint(4), varint(7), varint(9)].concat();
        assert_eq!(parse_range(&deprecated).unwrap(), (4, 7));
        let typed = [varint(8), varint(3), varint(16), varint(5)].concat();
        assert_eq!(parse_single_line_range(&typed).unwrap(), (3, 5));
    }

    #[test]
    fn rejects_oversized_length_before_allocating() {
        let bytes = varint((MAX_ITEM_BYTES + 1) as u64);
        assert_eq!(
            read_len(&mut bytes.as_slice(), MAX_ITEM_BYTES)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn imports_cross_file_references_and_keeps_active_generation_on_failure() {
        let root = std::env::temp_dir().join(format!("harmony-scip-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("src")).unwrap();
        let source = root.join("src/source.ts");
        let target = root.join("src/target.ts");
        fs::write(&source, "function caller() { fetch(); }\n").unwrap();
        fs::write(&target, "function fetch() {}\n").unwrap();
        let source_meta = fs::metadata(&source).unwrap();
        let target_meta = fs::metadata(&target).unwrap();
        let source_mtime = source_meta
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;
        let target_mtime = target_meta
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;
        let database = root.join("catalog.sqlite3");
        let conn = Connection::open(&database).unwrap();
        conn.execute_batch(
            "CREATE TABLE files(path TEXT PRIMARY KEY, state TEXT, size INTEGER, mtime_ns INTEGER);
             CREATE TABLE symbols(file TEXT, name TEXT, line INTEGER, end_line INTEGER, role TEXT);",
        ).unwrap();
        conn.execute(
            "INSERT INTO files VALUES('src/source.ts','indexed',?1,?2)",
            params![source_meta.len() as i64, source_mtime],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO files VALUES('src/target.ts','indexed',?1,?2)",
            params![target_meta.len() as i64, target_mtime],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO symbols VALUES('src/source.ts','caller',1,1,'logic')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO symbols VALUES('src/target.ts','fetch',1,1,'logic')",
            [],
        )
        .unwrap();
        drop(conn);

        let symbol = "scip-typescript npm demo 1.0 fetch().";
        let mut index_bytes = len_field(
            2,
            &document("src/target.ts", &[occurrence(symbol, 0, 9, true)]),
        );
        // Cross the 256-document transaction boundary while duplicate occurrences
        // still collapse to one persisted reference edge.
        for _ in 0..DOCUMENTS_PER_TRANSACTION {
            index_bytes.extend(len_field(
                2,
                &document("src/source.ts", &[occurrence(symbol, 0, 20, false)]),
            ));
        }
        let index = root.join("index.scip");
        fs::write(&index, index_bytes).unwrap();
        let available = status(&root, &database);
        assert_eq!(available.state, "available_not_imported");
        assert_eq!(available.index_path.as_deref(), Some("index.scip"));
        let stats = import(&root, &database, &index).unwrap();
        assert_eq!(stats.documents, DOCUMENTS_PER_TRANSACTION + 1);
        assert_eq!(stats.resolved_references, 1);
        let active_status = status(&root, &database);
        assert_eq!(active_status.state, "active");
        assert_eq!(active_status.resolved_references, 1);
        let conn = Connection::open(&database).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT source_name || '->' || target_name FROM scip_reference_edges",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "caller->fetch"
        );
        let active = conn
            .query_row(
                "SELECT active_import_id FROM scip_import_state",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        drop(conn);

        fs::write(
            &index,
            [varint(2 << 3 | 2), varint(12), vec![1, 2]].concat(),
        )
        .unwrap();
        assert_eq!(status(&root, &database).state, "stale_index");
        assert!(import(&root, &database, &index).is_err());
        let conn = Connection::open(&database).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT active_import_id FROM scip_import_state",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            active
        );
        drop(conn);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[ignore = "手动 SCIP 流式导入基准；通过 HARMONY_SCIP_BENCH_DOCS 选择规模"]
    fn large_scip_import_baseline() {
        let documents = std::env::var("HARMONY_SCIP_BENCH_DOCS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(10_000)
            .clamp(1, 1_000_000);
        let root =
            std::env::temp_dir().join(format!("harmony-scip-bench-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("src")).unwrap();
        let source = root.join("src/source.ts");
        let target = root.join("src/target.ts");
        fs::write(&source, "function caller() { fetch(); }\n").unwrap();
        fs::write(&target, "function fetch() {}\n").unwrap();
        let database = root.join("catalog.sqlite3");
        let conn = Connection::open(&database).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
             CREATE TABLE files(path TEXT PRIMARY KEY, state TEXT, size INTEGER, mtime_ns INTEGER);
             CREATE TABLE symbols(file TEXT, name TEXT, line INTEGER, end_line INTEGER, role TEXT);",
        ).unwrap();
        for (path, rel) in [(&source, "src/source.ts"), (&target, "src/target.ts")] {
            let meta = fs::metadata(path).unwrap();
            let mtime = meta
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos() as i64;
            conn.execute(
                "INSERT INTO files VALUES(?1,'indexed',?2,?3)",
                params![rel, meta.len() as i64, mtime],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO symbols VALUES('src/source.ts','caller',1,1,'logic')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO symbols VALUES('src/target.ts','fetch',1,1,'logic')",
            [],
        )
        .unwrap();
        drop(conn);

        let symbol = "scip-typescript npm bench 1.0 fetch().";
        let index = root.join("index.scip");
        let mut writer = BufWriter::new(File::create(&index).unwrap());
        writer
            .write_all(&len_field(
                2,
                &document("src/target.ts", &[occurrence(symbol, 0, 9, true)]),
            ))
            .unwrap();
        for line in 0..documents {
            writer
                .write_all(&len_field(
                    2,
                    &document(
                        "src/source.ts",
                        &[occurrence(symbol, line as u64, 20, false)],
                    ),
                ))
                .unwrap();
        }
        writer.flush().unwrap();
        drop(writer);

        let started = std::time::Instant::now();
        let stats = import(&root, &database, &index).unwrap();
        let import_ms = started.elapsed().as_millis() as u64;
        assert_eq!(stats.resolved_references, documents);
        let database_bytes = [
            database.clone(),
            database.with_extension("sqlite3-wal"),
            database.with_extension("sqlite3-shm"),
        ]
        .into_iter()
        .filter_map(|path| fs::metadata(path).ok().map(|meta| meta.len()))
        .sum::<u64>();
        println!(
            "HARMONY_SCIP_BASELINE={}",
            serde_json::json!({
                "schema_version": 1,
                "documents": documents + 1,
                "references": documents,
                "resolved_references": stats.resolved_references,
                "index_bytes": fs::metadata(&index).unwrap().len(),
                "database_bytes": database_bytes,
                "import_ms": import_ms,
                "documents_per_second": (documents as u64).saturating_mul(1000) / import_ms.max(1),
                "transaction_documents": DOCUMENTS_PER_TRANSACTION,
                "edge_batch_rows": EDGE_BUILD_BATCH_ROWS,
                "platform": std::env::consts::OS,
                "architecture": std::env::consts::ARCH,
            })
        );
        let _ = fs::remove_dir_all(root);
    }
}
