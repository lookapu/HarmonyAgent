//! 项目文件事件监听：原生 watcher 负责低延迟，目录重扫负责最终一致性。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

struct WatchHandle {
    _watcher: RecommendedWatcher,
    last_used: std::time::Instant,
}
static WATCHERS: OnceLock<Mutex<HashMap<String, WatchHandle>>> = OnceLock::new();
const SKIP_DIRS: &[&str] = &[
    ".git", ".idea", ".hvigor", ".ohpm", ".venv", "node_modules", "oh_modules",
    "build", "dist", "target", "coverage",
];

fn watchers() -> &'static Mutex<HashMap<String, WatchHandle>> {
    WATCHERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn key(root: &Path) -> String {
    root.canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn relevant_event(event: &Event) -> bool {
    !matches!(event.kind, EventKind::Access(_))
}

fn ignored_path(rel: &str) -> bool {
    let parts = rel.split('/').filter(|part| !part.is_empty()).collect::<Vec<_>>();
    parts.iter().enumerate().any(|(index, part)| {
        SKIP_DIRS.contains(part) || (index + 1 < parts.len() && part.starts_with('.'))
    })
}

fn process_batch(root: &Path, events: Vec<Result<Event, notify::Error>>) {
    let mut paths = HashSet::new();
    let mut uncertain = false;
    for result in events {
        match result {
            Ok(event) => {
                uncertain |= event.need_rescan() || event.paths.is_empty();
                if !relevant_event(&event) {
                    continue;
                }
                for path in event.paths {
                    if let Ok(rel) = path.strip_prefix(root) {
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        if rel.is_empty() {
                            uncertain = true;
                        } else if !ignored_path(&rel) {
                            paths.insert(rel);
                        }
                    }
                }
            }
            Err(_) => uncertain = true,
        }
    }
    let had_paths = !paths.is_empty();
    let precise = if had_paths {
        let paths = paths.into_iter().collect::<Vec<_>>();
        crate::services::symbol_index::invalidate_files(root, &paths)
    } else {
        true
    };
    // 精确文件事件已直接提交 SQLite；仅目录/丢事件/数据库失败需要延迟全库校验。
    if uncertain || !precise {
        crate::services::symbol_index::request_reconciliation(root);
    }
}

/// 为项目懒启动一个原生递归 watcher。失败时返回 false，索引继续使用周期扫描兜底。
pub fn ensure_watching(root: &Path) -> bool {
    if !root.is_dir() {
        return false;
    }
    let watch_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let watch_key = key(&watch_root);
    if let Ok(mut guard) = watchers().lock() {
        if let Some(handle) = guard.get_mut(&watch_key) {
            handle.last_used = std::time::Instant::now();
            return true;
        }
    }

    let (tx, rx) = mpsc::channel::<Result<Event, notify::Error>>();
    let mut watcher = match notify::recommended_watcher(move |event| {
        let _ = tx.send(event);
    }) {
        Ok(value) => value,
        Err(_) => return false,
    };
    if watcher.watch(&watch_root, RecursiveMode::Recursive).is_err() {
        return false;
    }

    let worker_root = watch_root;
    if std::thread::Builder::new()
        .name("repo-index-watcher".into())
        .spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut batch = vec![first];
                loop {
                    match rx.recv_timeout(Duration::from_millis(200)) {
                        Ok(event) => batch.push(event),
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            process_batch(&worker_root, batch);
                            return;
                        }
                    }
                }
                process_batch(&worker_root, batch);
            }
        })
        .is_err()
    {
        return false;
    }

    if let Ok(mut guard) = watchers().lock() {
        // 每个原生 watcher 都占用系统句柄；按最近使用淘汰，防止多项目会话无限累积。
        if guard.len() >= 16 {
            if let Some(oldest) = guard
                .iter()
                .min_by_key(|(_, handle)| handle.last_used)
                .map(|(key, _)| key.clone())
            {
                guard.remove(&oldest);
            }
        }
        guard.insert(
            watch_key,
            WatchHandle {
                _watcher: watcher,
                last_used: std::time::Instant::now(),
            },
        );
        true
    } else {
        false
    }
}

pub fn is_watching(root: &Path) -> bool {
    let watch_key = key(root);
    watchers()
        .lock()
        .ok()
        .is_some_and(|guard| guard.contains_key(&watch_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_events_do_not_dirty_the_index() {
        assert!(!relevant_event(&Event::new(EventKind::Access(
            notify::event::AccessKind::Any,
        ))));
        assert!(relevant_event(&Event::new(EventKind::Any)));
        assert!(ignored_path("target/debug/app"));
        assert!(ignored_path(".git/index"));
        assert!(!ignored_path(".env"));
        assert!(!ignored_path("entry/src/main/Index.ets"));
    }

    #[test]
    fn key_is_stable_for_existing_root() {
        let root = std::env::temp_dir();
        assert_eq!(key(&root), key(&root));
    }

    #[test]
    fn mutating_batch_requests_lazy_reconciliation() {
        let root = std::env::temp_dir().join(format!(
            "deveco-watcher-batch-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("before.rs"), "fn before() {}\n").unwrap();
        let _ = crate::services::symbol_index::index_project_cached(&root);
        let changed = root.join("after.rs");
        std::fs::write(&changed, "fn after() {}\n").unwrap();
        process_batch(
            &root,
            vec![Ok(Event::new(EventKind::Create(
                notify::event::CreateKind::File,
            ))
            .add_path(changed))],
        );
        assert!(crate::services::symbol_index::reconciliation_pending(&root));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    #[ignore = "依赖宿主原生文件事件能力，作为本机/CI 环境探针"]
    fn native_watcher_observes_created_file() {
        let root = std::env::temp_dir().join(format!(
            "deveco-native-watcher-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("before.rs"), "fn before() {}\n").unwrap();
        let _ = crate::services::symbol_index::index_project_cached(&root);
        assert!(ensure_watching(&root));
        std::thread::sleep(Duration::from_millis(500));
        std::fs::write(root.join("created.rs"), "fn created() {}\n").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline
            && !crate::services::symbol_index::reconciliation_pending(&root)
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(crate::services::symbol_index::reconciliation_pending(&root));
        std::fs::remove_dir_all(root).ok();
    }
}
