//! SDK / API 版本号解析与比较。
//!
//! HarmonyOS 开发套件（API 版本）存在两种格式：
//! - **26.0.0 之前**：`X.Y.Z(N)`，`N` 为 OpenHarmony 底座 API level（如 `6.1.1(24)`）；
//!   API 9 及以下允许裸数字（如 `9`）。
//! - **26.0.0 起**：语义化版本 `X.Y.Z`，取代旧格式；OpenHarmony 底座版本号与之统一，
//!   因此 `26.0.0` 对应的 API level 就是 `26`。
//!
//! 官方给出的次序：`26.0.0 > 6.1.1(24) > 6.1.0(23) > … > 5.0.5(17)`。
//! 因此比较时统一映射为 (API level 等价值, 主, 次, 修订) 元组：
//! 旧格式取括号里的 level，新格式取主版本号。

use std::cmp::Ordering;

/// 解析后的版本号
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SdkVersion {
    pub major: i64,
    pub minor: i64,
    pub patch: i64,
    /// 可比较的等价 API level（旧格式 = 括号内数字；新格式 = 主版本号）
    pub api_level: i64,
}

impl SdkVersion {
    /// 比较键：先比 API level，再比主/次/修订（同一 level 下区分 26.0.0 与 26.1.0）
    fn key(&self) -> (i64, i64, i64, i64) {
        (self.api_level, self.major, self.minor, self.patch)
    }
}

/// 解析版本号字符串，失败返回 None（不做臆造推断）。
pub fn parse(raw: &str) -> Option<SdkVersion> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    // 旧格式 X.Y.Z(N)：括号内为 OpenHarmony 底座 API level
    if s.contains('(') {
        if !s.ends_with(')') {
            return None;
        }
        let open = s.find('(')?;
        let close = s.rfind(')')?;
        if close > open {
            let digits = s[open + 1..close].trim();
            if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let api_level = digits.parse::<i64>().ok()?;
            let (major, minor, patch) = parse_triplet(&s[..open])?;
            return Some(SdkVersion { major, minor, patch, api_level });
        }
        return None;
    }
    // 裸数字：API 9 及以下的历史写法
    if s.chars().all(|c| c.is_ascii_digit()) {
        let api_level = s.parse::<i64>().ok()?;
        return Some(SdkVersion { major: api_level, minor: 0, patch: 0, api_level });
    }
    // 新格式 X.Y.Z（26.0.0 起）：主版本号即等价 API level
    let (major, minor, patch) = parse_triplet(s)?;
    Some(SdkVersion { major, minor, patch, api_level: major })
}

/// 解析 `X`、`X.Y`、`X.Y.Z`；缺省位补 0。
fn parse_triplet(s: &str) -> Option<(i64, i64, i64)> {
    let parts: Vec<&str> = s.trim().split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut nums = [0i64; 3];
    for (i, part) in parts.iter().enumerate() {
        let part = part.trim();
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        nums[i] = part.parse::<i64>().ok()?;
    }
    Some((nums[0], nums[1], nums[2]))
}

/// 取等价 API level（旧格式取括号内数字；`26.0.0` 取 26）。
pub fn api_level(raw: &str) -> Option<i64> {
    parse(raw).map(|v| v.api_level)
}

/// 是否为合法的 SDK 版本配置值（写进 build-profile.json5 前的格式校验）。
///
/// - 26.0.0 起：语义化版本 `X.Y.Z`（如 `26.0.0`），不允许再带括号后缀；
/// - 26.0.0 之前：`X.Y.Z(N)`（如 `6.1.1(24)`），不允许裸数字（API 9 及以下除外，
///   但本工具只生成 API 10+ 工程）。
pub fn is_config_version_like(raw: &str) -> bool {
    let s = raw.trim();
    let Some(v) = parse(s) else { return false };
    let has_parens = s.contains('(');
    if v.major >= SEMVER_SINCE_MAJOR {
        !has_parens && s.split('.').count() == 3
    } else {
        has_parens && s.ends_with(')') && v.api_level != v.major
    }
}

/// 启用语义化版本号体系的主版本（官方：从 API 26.0.0 起）。
const SEMVER_SINCE_MAJOR: i64 = 26;

/// 是否为合法的 SDK 版本字符串（`X.Y.Z(N)` 或 `X.Y.Z`；含历史裸数字写法）。
pub fn is_version_like(raw: &str) -> bool {
    parse(raw).is_some()
}

/// 比较两个版本的先后（官方次序：26.0.0 > 6.1.1(24) > … > 5.0.5(17)）。
pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    Some(parse(a)?.key().cmp(&parse(b)?.key()))
}

/// 由 sdk-pkg.json 的 platformVersion / apiVersion 组合出配置字符串。
///
/// - 旧格式（`6.1.1` + `24`）→ `"6.1.1(24)"`
/// - 新格式（`26.0.0` + `26`，或 `26.0.0` + `26.0.0`）→ `"26.0.0"`
///
/// 判断依据：apiVersion 与 platformVersion 的主版本号一致时即为新格式（同一版本号体系），
/// 否则沿用 `平台版本(API版本)` 组合。
pub fn join(platform_version: &str, api_version: &str) -> Option<String> {
    let platform = platform_version.trim();
    let api = api_version.trim();
    if platform.is_empty() || api.is_empty() {
        return None;
    }
    let parsed = parse(platform)?;
    // api 为纯数字且与平台主版本号相同 → 新格式，直接用平台版本
    if api.chars().all(|c| c.is_ascii_digit()) {
        let api_num = api.parse::<i64>().ok()?;
        if api_num == parsed.major && platform.contains('.') {
            return Some(platform.to_string());
        }
        return Some(format!("{platform}({api_num})"));
    }
    // api 自身即 Semantic 版本（如 26.0.0）
    let api_parsed = parse(api)?;
    if api_parsed.major == parsed.major {
        return Some(platform.to_string());
    }
    Some(format!("{platform}({api})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_legacy_and_semver() {
        let v = parse("6.1.1(24)").unwrap();
        assert_eq!((v.major, v.minor, v.patch, v.api_level), (6, 1, 1, 24));
        let v = parse("26.0.0").unwrap();
        assert_eq!((v.major, v.minor, v.patch, v.api_level), (26, 0, 0, 26));
        assert_eq!(api_level("24"), Some(24));
        assert_eq!(api_level("5.0.0(12)"), Some(12));
        assert_eq!(api_level("26.0.0"), Some(26));
    }

    #[test]
    fn rejects_invalid() {
        assert!(parse("").is_none());
        assert!(parse("6.1.1(24").is_none());
        assert!(parse("(24)").is_none());
        assert!(parse("6.1.1()").is_none());
        assert!(parse("6.1.1(a)").is_none());
        assert!(parse("6.1.1(24) 之后").is_none());
        assert!(!is_version_like("abc"));
    }

    #[test]
    fn config_version_format_follows_era() {
        // 26.0.0 之前：必须 平台版本(API版本)
        assert!(is_config_version_like("6.1.1(24)"));
        assert!(is_config_version_like("5.0.0(12)"));
        assert!(!is_config_version_like("5.0.0"));
        assert!(!is_config_version_like("24"));
        // 26.0.0 起：语义化版本，不带括号
        assert!(is_config_version_like("26.0.0"));
        assert!(is_config_version_like("26.1.0"));
        assert!(!is_config_version_like("26.0.0(26)"));
        assert!(!is_config_version_like("26.0"));
        assert!(!is_config_version_like("6.1.1(24"));
        assert!(!is_config_version_like("(24)"));
        assert!(!is_config_version_like("6.1.1()"));
    }

    #[test]
    fn official_ordering_holds() {
        // 官方示例次序：26.0.0 > 6.1.1(24) > 6.1.0(23) > 6.0.2(22) > 5.0.5(17)
        let order = ["26.0.0", "6.1.1(24)", "6.1.0(23)", "6.0.2(22)", "5.0.5(17)"];
        for pair in order.windows(2) {
            assert_eq!(compare(pair[0], pair[1]), Some(Ordering::Greater), "{pair:?}");
        }
        // 同一主版本下按次版本区分
        assert_eq!(compare("26.1.0", "26.0.0"), Some(Ordering::Greater));
    }

    #[test]
    fn joins_sdk_pkg_versions() {
        assert_eq!(join("6.1.1", "24").as_deref(), Some("6.1.1(24)"));
        assert_eq!(join("26.0.0", "26").as_deref(), Some("26.0.0"));
        assert_eq!(join("26.0.0", "26.0.0").as_deref(), Some("26.0.0"));
        assert_eq!(join("", "24"), None);
    }
}
