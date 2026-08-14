/// Windows 路径规范化：去掉 `std::fs::canonicalize` 产生的 `\\?\` verbatim 前缀，
/// 还原为普通 Win32 路径（`\\?\<REF_PROJECT>` → `<REF_PROJECT>`，`\\?\UNC\host\share` → `\\host\share`），
/// 便于界面展示与传给外部工具（git / hvigorw / hdc / cmd 等对 verbatim 路径支持不佳）。
/// 非 Windows 或无前缀时原样返回。
pub fn normalize_path(p: &str) -> String {
    #[cfg(windows)]
    {
        if let Some(rest) = p.strip_prefix(r"\\?\UNC\") {
            return format!("\\\\{rest}");
        }
        if let Some(rest) = p.strip_prefix(r"\\?\") {
            return rest.to_string();
        }
    }
    p.to_string()
}

/// 路径包含判断：inner 是否位于 outer 内（含相等）。
/// Windows 下大小写不敏感（盘符/目录名大小写不同不应误判越界）。
/// 入参应为 canonicalize 后的绝对路径。
pub fn path_within(inner: &std::path::Path, outer: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        let i = inner.to_string_lossy().to_lowercase();
        let o = outer.to_string_lossy().to_lowercase();
        i == o
            || i.starts_with(&format!("{o}\\"))
            || i.starts_with(&format!("{o}/"))
    }
    #[cfg(not(windows))]
    {
        inner.starts_with(outer)
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_path, path_within};

    #[test]
    fn strip_verbatim_prefix() {
        assert_eq!(normalize_path(r"\\?\<REF_PROJECT>"), r"<REF_PROJECT>");
        assert_eq!(normalize_path(r"\\?\C:\a\b"), r"C:\a\b");
        assert_eq!(normalize_path(r"\\?\UNC\host\share\dir"), r"\\host\share\dir");
        // 无前缀原样返回
        assert_eq!(normalize_path(r"<REF_PROJECT>"), r"<REF_PROJECT>");
        assert_eq!(normalize_path(r"relative/path"), r"relative/path");
        assert_eq!(normalize_path(""), "");
    }

    #[test]
    fn within_case_insensitive() {
        use std::path::Path;
        assert!(path_within(Path::new(r"D:\Work\Code\a\b"), Path::new(r"d:\work\code")));
        assert!(path_within(Path::new(r"D:\Work\Code"), Path::new(r"d:\work\code")));
        assert!(!path_within(Path::new(r"D:\Work\Code2\b"), Path::new(r"d:\work\code")));
        assert!(!path_within(Path::new(r"C:\Elsewhere"), Path::new(r"d:\work\code")));
    }
}
