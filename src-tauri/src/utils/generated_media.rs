//! 生成媒体产物的目录治理。
//!
//! 产物布局：`{app_data}/generated/{conversation_id}/{uuid}/<files>`。
//! 会话删除与一键清空必须回收对应目录，否则图片/视频/音频会永久占盘；
//! 启动时再按现存会话做一次孤儿清扫，兜住崩溃或删库失败留下的残留。
//!
//! 会话 id 来自前端入参，拼路径前必须校验为单段文件名，避免 `..` 或分隔符
//! 把删除范围带出 generated 根目录。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// 生成的媒体根目录。
pub fn generated_root(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("generated")
}

/// 会话 id 必须是单段安全文件名：非空、非 `.`/`..`、不含分隔符/盘符/NUL。
fn is_safe_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value
            .chars()
            .any(|c| matches!(c, '/' | '\\' | ':' | '\0'))
}

fn conversation_dir(app_data_dir: &Path, conversation_id: &str) -> Option<PathBuf> {
    is_safe_segment(conversation_id).then(|| generated_root(app_data_dir).join(conversation_id))
}

/// 删除某个会话的全部生成产物。返回是否真的删除了目录（不存在视为成功）。
/// 会话 id 非法时拒绝操作并返回 `Ok(false)`，不会触碰 generated 根目录之外的路径。
pub fn remove_conversation(app_data_dir: &Path, conversation_id: &str) -> std::io::Result<bool> {
    let Some(dir) = conversation_dir(app_data_dir, conversation_id) else {
        return Ok(false);
    };
    remove_dir_if_exists(&dir)
}

/// 删除 generated 根目录及其下全部产物（一键清空内容时使用）。
pub fn remove_all(app_data_dir: &Path) -> std::io::Result<bool> {
    remove_dir_if_exists(&generated_root(app_data_dir))
}

fn remove_dir_if_exists(dir: &Path) -> std::io::Result<bool> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// 启动孤儿清扫：删除 generated 下不属于任何现存会话的目录。
///
/// 只处理目录，根目录下散落的文件保持不动（来源不明，不擅自删除）；
/// 无法转成 UTF-8 的目录名不可能是会话 id，同样跳过。
/// 返回 (删除目录数, 删除失败数)。
pub fn sweep_orphans(app_data_dir: &Path, live: &HashSet<String>) -> (usize, usize) {
    let root = generated_root(app_data_dir);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return (0, 0);
    };
    let mut removed = 0usize;
    let mut failed = 0usize;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if live.contains(name) {
            continue;
        }
        match std::fs::remove_dir_all(entry.path()) {
            Ok(()) => removed += 1,
            Err(_) => failed += 1,
        }
    }
    (removed, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个用例独占一个临时根目录，避免并行测试互删。
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let dir = std::env::temp_dir()
                .join(format!("deveco-generated-media-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_file(dir: &Path, relative: &str) {
        let file = dir.join(relative);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, b"media").unwrap();
    }

    #[test]
    fn unsafe_conversation_ids_are_rejected() {
        let root = TempRoot::new();
        for id in ["", ".", "..", "a/b", "a\\b", "C:conv", "a\0b"] {
            assert!(
                conversation_dir(&root.0, id).is_none(),
                "会话 id {id:?} 不应被拼进路径"
            );
            assert!(!remove_conversation(&root.0, id).unwrap());
        }
        for id in ["conv-1", "0f8fad5b-d9cb-469f-a165-70867728950e"] {
            assert!(conversation_dir(&root.0, id).is_some(), "{id} 应合法");
        }
        // 根目录不能被非法 id 误删
        let generated = generated_root(&root.0);
        std::fs::create_dir_all(generated.join("keep")).unwrap();
        assert!(!remove_conversation(&root.0, "..").unwrap());
        assert!(generated.join("keep").is_dir());
    }

    #[test]
    fn remove_conversation_touches_only_target() {
        let root = TempRoot::new();
        let generated = generated_root(&root.0);
        write_file(&generated.join("conv-a").join("u1"), "a.png");
        write_file(&generated.join("conv-b").join("u2"), "b.png");

        assert!(remove_conversation(&root.0, "conv-a").unwrap());
        assert!(!generated.join("conv-a").exists());
        assert!(generated.join("conv-b").join("u2").join("b.png").is_file());
        // 已经不存在：重复删除不报错
        assert!(!remove_conversation(&root.0, "conv-a").unwrap());
    }

    #[test]
    fn remove_all_drops_generated_root() {
        let root = TempRoot::new();
        let generated = generated_root(&root.0);
        write_file(&generated.join("conv-a").join("u1"), "a.png");
        write_file(&generated.join("conv-b").join("u2"), "b.png");

        assert!(remove_all(&root.0).unwrap());
        assert!(!generated.exists());
        assert!(!remove_all(&root.0).unwrap());
    }

    #[test]
    fn sweep_keeps_live_and_removes_orphans() {
        let root = TempRoot::new();
        let generated = generated_root(&root.0);
        write_file(&generated.join("live").join("u1"), "a.png");
        write_file(&generated.join("orphan").join("u2"), "b.png");
        // 根目录散落文件不动
        write_file(&generated, "stray.txt");

        let live: HashSet<String> = ["live".to_string()].into_iter().collect();
        assert_eq!(sweep_orphans(&root.0, &live), (1, 0));
        assert!(generated.join("live").join("u1").join("a.png").is_file());
        assert!(!generated.join("orphan").exists());
        assert!(generated.join("stray.txt").is_file());
    }

    #[test]
    fn sweep_without_root_is_noop() {
        let root = TempRoot::new();
        assert_eq!(sweep_orphans(&root.0, &HashSet::new()), (0, 0));
    }
}
