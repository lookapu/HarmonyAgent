//! 从已检出的 HarmonyOS/OpenHarmony 工程提取可复用工程模式。
//!
//! 结论只来自语义模型、源码命中和 Git checkout 元数据；不把 README 宣传语当作实现事实。

use serde::Serialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::services::harmony_model::HarmonySemanticModel;

const MAX_FILES: usize = 2_500;
const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_EVIDENCE: usize = 8;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RepositoryEvidence {
    pub origin: Option<String>,
    pub host: String,
    pub revision: Option<String>,
    pub branch: Option<String>,
    pub observed_at: u64,
    pub traceable: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReusablePattern {
    pub id: String,
    pub name: String,
    pub confidence: String,
    pub summary: String,
    pub evidence: Vec<String>,
    pub reuse_guidance: String,
    pub applicability: String,
    pub cautions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PatternReport {
    pub repository: RepositoryEvidence,
    pub scanned_files: usize,
    pub truncated: bool,
    pub patterns: Vec<ReusablePattern>,
    pub limitations: Vec<String>,
}

#[derive(Default)]
struct SourceSignals {
    scanned: usize,
    truncated: bool,
    state: Vec<String>,
    navigation: Vec<String>,
    network: Vec<String>,
    storage: Vec<String>,
    testing: Vec<String>,
    native: Vec<String>,
    adaptive: Vec<String>,
}

pub fn analyze(root: &Path, model: &HarmonySemanticModel) -> PatternReport {
    let repository = repository_evidence(root);
    let signals = scan_sources(root);
    let mut patterns = Vec::new();

    architecture_pattern(model)
        .into_iter()
        .for_each(|value| patterns.push(value));
    product_pattern(model)
        .into_iter()
        .for_each(|value| patterns.push(value));
    routing_pattern(model, &signals)
        .into_iter()
        .for_each(|value| patterns.push(value));
    dependency_pattern(model)
        .into_iter()
        .for_each(|value| patterns.push(value));
    ability_pattern(model)
        .into_iter()
        .for_each(|value| patterns.push(value));
    device_pattern(model, &signals)
        .into_iter()
        .for_each(|value| patterns.push(value));
    source_pattern(
        "state-management",
        "状态管理",
        &signals.state,
        "源码使用 ArkUI 状态装饰器或应用级存储形成状态流。",
        "复用前先确认组件生命周期和状态所有权，再迁移装饰器组合与持久化边界。",
        "适用于相同 ArkUI 状态管理代际与组件生命周期的页面。",
        &["不同 API 代际的状态管理装饰器不可机械替换。"],
    )
    .into_iter()
    .for_each(|value| patterns.push(value));
    source_pattern(
        "network-layer",
        "网络访问层",
        &signals.network,
        "源码存在系统 HTTP/网络 Kit 或 ohpm 网络库调用。",
        "复用接口封装、超时/错误映射与取消策略；域名、证书和鉴权必须由目标工程重新配置。",
        "适用于网络协议与数据契约相同的业务模块。",
        &[
            "不能复制令牌、证书、域名白名单或用户数据。",
            "需复验 INTERNET 权限与网络安全策略。",
        ],
    )
    .into_iter()
    .for_each(|value| patterns.push(value));
    source_pattern(
        "persistence-layer",
        "数据持久化层",
        &signals.storage,
        "源码存在 Preferences、关系型数据库或分布式 KV 存储调用。",
        "优先复用仓储接口与迁移策略，不直接复制数据库文件、表数据或密钥。",
        "适用于数据模型和一致性要求相近的模块。",
        &["必须重新评估数据迁移、加密、备份和隐私合规。"],
    )
    .into_iter()
    .for_each(|value| patterns.push(value));
    source_pattern(
        "test-structure",
        "测试组织",
        &signals.testing,
        "工程包含 Hypium/测试目录或测试用例结构。",
        "复用测试分层、fixture 与断言组织；把包名、设备能力和外部服务替换为目标工程事实。",
        "适用于同类模块的单元测试和 ohosTest 验证。",
        &["测试存在不等于覆盖充分，仍需检查实际断言与运行证据。"],
    )
    .into_iter()
    .for_each(|value| patterns.push(value));
    source_pattern(
        "native-interop",
        "Native 互操作",
        &signals.native,
        "工程包含 C/C++、CMake 或 N-API 相关实现。",
        "只复用清晰的 ABI 边界、资源释放与线程模型；按目标设备 ABI 重新构建。",
        "适用于确实需要 Native 性能或既有 C/C++ 资产的模块。",
        &[
            "预编译二进制不可跨 ABI/系统版本直接复用。",
            "需重新进行内存、线程和崩溃测试。",
        ],
    )
    .into_iter()
    .for_each(|value| patterns.push(value));

    patterns.sort_by(|a, b| a.id.cmp(&b.id));
    PatternReport {
        repository,
        scanned_files: signals.scanned,
        truncated: signals.truncated,
        patterns,
        limitations: vec![
            "报告证明仓库中观察到的实现形态，不证明该模式适合当前目标工程。".to_string(),
            "复用前必须核对许可证、当前工程 API Level、本机 SDK 声明、依赖版本并完成 lint/test/build。".to_string(),
            "源码扫描有文件数和单文件大小上限；truncated=true 时结论是不完整样本。".to_string(),
        ],
    }
}

fn architecture_pattern(model: &HarmonySemanticModel) -> Option<ReusablePattern> {
    if model.modules.len() < 2 {
        return None;
    }
    let mut kinds = model
        .modules
        .iter()
        .map(|module| {
            format!(
                "{}:{}({})",
                module.rel_path, module.name, module.artifact_kind
            )
        })
        .collect::<Vec<_>>();
    kinds.truncate(MAX_EVIDENCE);
    Some(pattern(
        "modular-architecture",
        "HAP/HSP/HAR 模块化",
        "high",
        format!(
            "工程以 {} 个模块和明确产物类型拆分应用与共享能力。",
            model.modules.len()
        ),
        kinds,
        "复用模块职责与依赖方向，先在目标工程画出 HAP/HSP/HAR 边界，再迁移公共接口。",
        "适用于需要独立交付、动态共享或静态复用的多模块工程。",
        &["模块名和目录结构不是架构本身，必须核对真实依赖边。"],
    ))
}

fn product_pattern(model: &HarmonySemanticModel) -> Option<ReusablePattern> {
    if model.products.len() < 2 {
        return None;
    }
    let evidence = model
        .products
        .iter()
        .take(MAX_EVIDENCE)
        .map(|product| {
            format!(
                "product {}: compile={:?}, compatible={:?}, target={:?}, modules={}",
                product.name,
                product.compile_api_level,
                product.compatible_api_level,
                product.target_api_level,
                product.modules.join(",")
            )
        })
        .collect();
    Some(pattern(
        "product-matrix",
        "多产品矩阵",
        "high",
        format!(
            "工程定义 {} 个 product，并保留 API、模块或构建差异。",
            model.products.len()
        ),
        evidence,
        "复用 product 差异的配置方法，把品牌、渠道、设备和 API 差异留在构建配置而非业务条件分支。",
        "适用于需要多渠道、多设备或分层 API 交付的工程。",
        &["签名材料和发布配置不能从示例仓库复制。"],
    ))
}

fn routing_pattern(
    model: &HarmonySemanticModel,
    signals: &SourceSignals,
) -> Option<ReusablePattern> {
    if model.graph.pages.is_empty() && signals.navigation.is_empty() {
        return None;
    }
    let mut evidence = model
        .graph
        .pages
        .iter()
        .map(|page| format!("{} [{}] {}", page.source_file, page.source_kind, page.path))
        .collect::<Vec<_>>();
    evidence.extend(signals.navigation.iter().cloned());
    evidence.sort();
    evidence.dedup();
    evidence.truncate(MAX_EVIDENCE);
    Some(pattern(
        "navigation",
        "页面路由与导航",
        "high",
        "工程包含可追溯的页面清单、router map 或 Navigation/NavPathStack 实现。".to_string(),
        evidence,
        "复用路由注册、参数类型与页面解耦方式，不复制业务路由名和鉴权假设。",
        "适用于采用相同 Navigation 代际和页面生命周期的应用。",
        &["路由 API 与状态恢复行为受 SDK/API Level 影响。"],
    ))
}

fn dependency_pattern(model: &HarmonySemanticModel) -> Option<ReusablePattern> {
    if model.dependencies.is_empty() {
        return None;
    }
    let evidence = model
        .dependencies
        .iter()
        .take(MAX_EVIDENCE)
        .map(|dependency| {
            format!(
                "{} -> {} {}{} [{}]",
                dependency.from_module,
                dependency.name,
                dependency.requirement,
                dependency
                    .locked_version
                    .as_ref()
                    .map(|value| format!(" => {value}"))
                    .unwrap_or_default(),
                dependency.scope
            )
        })
        .collect();
    Some(pattern(
        "dependency-governance",
        "依赖与锁文件治理",
        "high",
        format!(
            "语义模型解析到 {} 条模块或三方依赖。",
            model.dependencies.len()
        ),
        evidence,
        "复用依赖分层和锁定策略；三方包逐个运行 ohpm_search 审计，再在目标工程重新解析锁文件。",
        "适用于依赖边界和发布方式相近的工程。",
        &["版本约束、锁定版本与许可证必须以目标工程重新验证。"],
    ))
}

fn ability_pattern(model: &HarmonySemanticModel) -> Option<ReusablePattern> {
    let mut evidence = Vec::new();
    for module in &model.modules {
        evidence.extend(module.abilities.iter().map(|ability| {
            format!(
                "{} Ability {} entry={:?} exported={:?}",
                module.rel_path, ability.name, ability.src_entry, ability.exported
            )
        }));
        evidence.extend(module.extension_abilities.iter().map(|ability| {
            format!(
                "{} Extension {} type={:?} entry={:?}",
                module.rel_path, ability.name, ability.extension_type, ability.src_entry
            )
        }));
    }
    if evidence.is_empty() {
        return None;
    }
    evidence.truncate(MAX_EVIDENCE);
    Some(pattern(
        "ability-composition",
        "Ability / ExtensionAbility 组合",
        "high",
        "manifest 中声明了可追溯的 Ability 或 ExtensionAbility 入口。".to_string(),
        evidence,
        "复用生命周期职责拆分与入口组织，重新核对 exported、权限、skills 和系统回调。",
        "适用于需要相同系统入口类型的应用。",
        &["exported 与权限配置属于安全边界，不可机械复制。"],
    ))
}

fn device_pattern(
    model: &HarmonySemanticModel,
    signals: &SourceSignals,
) -> Option<ReusablePattern> {
    let devices = model
        .modules
        .iter()
        .flat_map(|module| module.device_types.iter().cloned())
        .collect::<BTreeSet<_>>();
    if devices.len() < 2
        && model.graph.system_capabilities.is_empty()
        && signals.adaptive.is_empty()
    {
        return None;
    }
    let mut evidence = Vec::new();
    if !devices.is_empty() {
        evidence.push(format!(
            "deviceTypes={}",
            devices.into_iter().collect::<Vec<_>>().join(",")
        ));
    }
    evidence.extend(model.graph.system_capabilities.iter().take(4).map(|item| {
        format!(
            "{}:{} SystemCapability {}",
            item.source_file, item.line, item.capability
        )
    }));
    evidence.extend(signals.adaptive.iter().cloned());
    evidence.truncate(MAX_EVIDENCE);
    Some(pattern(
        "device-adaptation",
        "多设备与能力适配",
        "medium",
        "工程通过 deviceTypes、SystemCapability 或窗口/资源限定实现设备差异。".to_string(),
        evidence,
        "复用能力探测和响应式分层，不复用设备型号白名单；对目标设备矩阵逐台验证。",
        "适用于 Phone/Tablet/2in1/Wearable 等多设备交付。",
        &["声明设备类型不证明所有布局和能力分支已在真机验证。"],
    ))
}

fn source_pattern(
    id: &str,
    name: &str,
    evidence: &[String],
    summary: &str,
    guidance: &str,
    applicability: &str,
    cautions: &[&str],
) -> Option<ReusablePattern> {
    if evidence.is_empty() {
        return None;
    }
    Some(pattern(
        id,
        name,
        "medium",
        summary.to_string(),
        evidence.iter().take(MAX_EVIDENCE).cloned().collect(),
        guidance,
        applicability,
        cautions,
    ))
}

fn pattern(
    id: &str,
    name: &str,
    confidence: &str,
    summary: String,
    evidence: Vec<String>,
    reuse_guidance: &str,
    applicability: &str,
    cautions: &[&str],
) -> ReusablePattern {
    ReusablePattern {
        id: id.to_string(),
        name: name.to_string(),
        confidence: confidence.to_string(),
        summary,
        evidence,
        reuse_guidance: reuse_guidance.to_string(),
        applicability: applicability.to_string(),
        cautions: cautions.iter().map(|value| (*value).to_string()).collect(),
    }
}

fn scan_sources(root: &Path) -> SourceSignals {
    let mut signals = SourceSignals::default();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut paths = entries
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        paths.sort();
        let mut directories = Vec::new();
        for path in paths {
            // 不跟随仓库中的符号链接，避免不可信 checkout 把只读扫描引出工作区。
            if fs::symlink_metadata(&path)
                .ok()
                .is_some_and(|metadata| metadata.file_type().is_symlink())
            {
                continue;
            }
            if path.is_dir() {
                if !is_ignored_dir(&path) {
                    if adaptive_directory(&path) {
                        push_evidence(&mut signals.adaptive, root, &path, 0, "resource qualifier");
                    }
                    directories.push(path);
                }
                continue;
            }
            if signals.scanned >= MAX_FILES {
                signals.truncated = true;
                break;
            }
            if !is_source_file(&path) {
                continue;
            }
            signals.scanned += 1;
            if is_native_file(&path) {
                push_evidence(
                    &mut signals.native,
                    root,
                    &path,
                    0,
                    "native source/build file",
                );
            }
            if path
                .components()
                .any(|part| matches!(part.as_os_str().to_str(), Some("test" | "ohosTest")))
            {
                push_evidence(&mut signals.testing, root, &path, 0, "test tree");
            }
            if fs::metadata(&path)
                .ok()
                .is_some_and(|meta| meta.len() > MAX_FILE_BYTES)
            {
                continue;
            }
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            for (line_index, line) in content.lines().enumerate() {
                let line_no = line_index + 1;
                match_signal(
                    &mut signals.state,
                    root,
                    &path,
                    line_no,
                    line,
                    &[
                        "@State",
                        "@Observed",
                        "@Track",
                        "@Local",
                        "@Provider",
                        "@Consumer",
                        "AppStorage",
                        "PersistentStorage",
                    ],
                );
                match_signal(
                    &mut signals.navigation,
                    root,
                    &path,
                    line_no,
                    line,
                    &["Navigation(", "NavPathStack", "RouterMap", "@Route"],
                );
                match_signal(
                    &mut signals.network,
                    root,
                    &path,
                    line_no,
                    line,
                    &[
                        "@ohos.net.http",
                        "@ohos.net.connection",
                        "@ohos.request",
                        "@ohos/axios",
                        "axios",
                    ],
                );
                match_signal(
                    &mut signals.storage,
                    root,
                    &path,
                    line_no,
                    line,
                    &[
                        "data.preferences",
                        "relationalStore",
                        "distributedKVStore",
                        "Preferences",
                    ],
                );
                match_signal(
                    &mut signals.testing,
                    root,
                    &path,
                    line_no,
                    line,
                    &["@ohos/hypium", "describe(", "it(", "expect("],
                );
                match_signal(
                    &mut signals.adaptive,
                    root,
                    &path,
                    line_no,
                    line,
                    &["mediaquery", "windowSize", "deviceType", "SystemCapability"],
                );
            }
        }
        // stack 为 LIFO，倒序压栈后仍按路径升序扫描，保证证据选择可重复。
        for path in directories.into_iter().rev() {
            stack.push(path);
        }
        if signals.truncated {
            break;
        }
    }
    signals
}

fn match_signal(
    target: &mut Vec<String>,
    root: &Path,
    path: &Path,
    line: usize,
    content: &str,
    needles: &[&str],
) {
    if target.len() >= MAX_EVIDENCE {
        return;
    }
    if let Some(needle) = needles.iter().find(|needle| content.contains(**needle)) {
        push_evidence(target, root, path, line, needle);
    }
}

fn push_evidence(target: &mut Vec<String>, root: &Path, path: &Path, line: usize, label: &str) {
    if target.len() >= MAX_EVIDENCE {
        return;
    }
    let relative = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
    let value = if line == 0 {
        format!("{relative} [{label}]")
    } else {
        format!("{relative}:{line} [{label}]")
    };
    if !target.contains(&value) {
        target.push(value);
    }
}

fn is_ignored_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            matches!(
                value,
                ".git"
                    | ".hvigor"
                    | ".idea"
                    | ".ohpm"
                    | "oh_modules"
                    | "node_modules"
                    | "build"
                    | "dist"
                    | "target"
            )
        })
}

fn is_source_file(path: &Path) -> bool {
    if path.file_name().and_then(|value| value.to_str()) == Some("CMakeLists.txt") {
        return true;
    }
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            matches!(
                value,
                "ets" | "ts" | "json" | "json5" | "yaml" | "yml" | "cpp" | "cc" | "c" | "h" | "hpp"
            )
        })
}

fn is_native_file(path: &Path) -> bool {
    path.file_name().and_then(|value| value.to_str()) == Some("CMakeLists.txt")
        || path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| matches!(value, "cpp" | "cc" | "c" | "h" | "hpp"))
}

fn adaptive_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            ["land", "tablet", "car", "tv", "wearable", "2in1", "dark"]
                .iter()
                .any(|marker| value.to_ascii_lowercase().contains(marker))
        })
}

fn repository_evidence(root: &Path) -> RepositoryEvidence {
    let git_dir = resolve_git_dir(root);
    let origin = git_dir.as_deref().and_then(read_origin);
    let (branch, revision) = git_dir.as_deref().map(read_head).unwrap_or_default();
    let host = origin
        .as_deref()
        .map(classify_host)
        .unwrap_or("local")
        .to_string();
    RepositoryEvidence {
        traceable: origin.is_some() && revision.is_some(),
        origin,
        host,
        revision,
        branch,
        observed_at: epoch_seconds(SystemTime::now()),
    }
}

fn resolve_git_dir(root: &Path) -> Option<PathBuf> {
    let marker = root.join(".git");
    if marker.is_dir() {
        return Some(marker);
    }
    let content = fs::read_to_string(&marker).ok()?;
    let value = PathBuf::from(content.trim().strip_prefix("gitdir:")?.trim());
    Some(if value.is_absolute() {
        value
    } else {
        root.join(value)
    })
}

fn read_origin(git_dir: &Path) -> Option<String> {
    let config = fs::read_to_string(git_dir.join("config")).ok()?;
    let mut in_origin = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_origin = line == "[remote \"origin\"]";
        } else if in_origin {
            if let Some(value) = line
                .strip_prefix("url")
                .and_then(|value| value.trim_start().strip_prefix('='))
            {
                let value = value.trim();
                if !value.is_empty() {
                    return Some(sanitize_origin(value));
                }
            }
        }
    }
    None
}

fn sanitize_origin(origin: &str) -> String {
    let Some(scheme_end) = origin.find("://") else {
        // SCP 风格地址仅保留常规 git 用户名；拒绝把包含口令形态的 userinfo 写入报告。
        if let Some((userinfo, host_path)) = origin.split_once('@') {
            return if userinfo == "git" {
                format!("git@{host_path}")
            } else {
                format!("[redacted]@{host_path}")
            };
        }
        return origin.to_string();
    };
    let authority_start = scheme_end + 3;
    let authority_end = origin[authority_start..]
        .find('/')
        .map(|index| authority_start + index)
        .unwrap_or(origin.len());
    let authority = &origin[authority_start..authority_end];
    let Some(at) = authority.rfind('@') else {
        return origin.to_string();
    };
    format!(
        "{}://{}{}",
        &origin[..scheme_end],
        &authority[at + 1..],
        &origin[authority_end..]
    )
}

fn read_head(git_dir: &Path) -> (Option<String>, Option<String>) {
    let Ok(head) = fs::read_to_string(git_dir.join("HEAD")) else {
        return (None, None);
    };
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref:").map(str::trim) {
        let revision = fs::read_to_string(git_dir.join(reference))
            .ok()
            .map(|value| value.trim().to_string())
            .or_else(|| read_packed_ref(git_dir, reference));
        return (
            reference.strip_prefix("refs/heads/").map(str::to_string),
            revision,
        );
    }
    (None, (!head.is_empty()).then(|| head.to_string()))
}

fn read_packed_ref(git_dir: &Path, reference: &str) -> Option<String> {
    fs::read_to_string(git_dir.join("packed-refs"))
        .ok()?
        .lines()
        .find_map(|line| {
            let (hash, name) = line.split_once(' ')?;
            (name == reference).then(|| hash.to_string())
        })
}

fn classify_host(origin: &str) -> &'static str {
    if origin.to_ascii_lowercase().contains("github.com") {
        "github"
    } else if origin.to_ascii_lowercase().contains("gitee.com") {
        "gitee"
    } else {
        "other-git"
    }
}

fn epoch_seconds(value: SystemTime) -> u64 {
    value
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::harmony_model::{
        HarmonyDependency, HarmonyModule, HarmonyPage, HarmonyProduct, HarmonySemanticModel,
    };

    #[test]
    fn extracts_traceable_patterns_from_a_gitee_checkout() {
        let root = std::env::temp_dir().join(format!("harmony-patterns-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
        fs::create_dir_all(root.join("entry/src/main/ets/pages")).unwrap();
        fs::create_dir_all(root.join("entry/src/ohosTest/ets/test")).unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(root.join(".git/refs/heads/main"), "abcdef0123456789\n").unwrap();
        fs::write(
            root.join(".git/config"),
            "[remote \"origin\"]\n url = https://gitee.com/example/demo.git\n",
        )
        .unwrap();
        fs::write(
            root.join("entry/src/main/ets/pages/Index.ets"),
            "import http from '@ohos.net.http'\n@State count: number = 0\nNavigation(this.path) {}",
        )
        .unwrap();
        fs::write(
            root.join("entry/src/ohosTest/ets/test/App.test.ets"),
            "describe('app', () => { it('works', () => {}) })",
        )
        .unwrap();
        let mut model = HarmonySemanticModel::default();
        model.modules = vec![
            HarmonyModule {
                name: "entry".into(),
                rel_path: "entry".into(),
                artifact_kind: "hap".into(),
                ..Default::default()
            },
            HarmonyModule {
                name: "shared".into(),
                rel_path: "shared".into(),
                artifact_kind: "har".into(),
                ..Default::default()
            },
        ];
        model.products = vec![HarmonyProduct {
            name: "default".into(),
            ..Default::default()
        }];
        model.dependencies.push(HarmonyDependency {
            from_module: "entry".into(),
            name: "shared".into(),
            requirement: "file:../shared".into(),
            target_module: Some("shared".into()),
            ..Default::default()
        });
        model.graph.pages.push(HarmonyPage {
            module: "entry".into(),
            path: "pages/Index".into(),
            source_kind: "main_pages".into(),
            source_file: "entry/src/main/resources/base/profile/main_pages.json".into(),
            ..Default::default()
        });

        let report = analyze(&root, &model);
        assert_eq!(report.repository.host, "gitee");
        assert_eq!(report.repository.branch.as_deref(), Some("main"));
        assert!(report.repository.traceable);
        let ids = report
            .patterns
            .iter()
            .map(|value| value.id.as_str())
            .collect::<BTreeSet<_>>();
        assert!(ids.contains("modular-architecture"));
        assert!(ids.contains("navigation"));
        assert!(ids.contains("network-layer"));
        assert!(ids.contains("state-management"));
        assert!(ids.contains("test-structure"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repository_origin_redacts_credentials() {
        assert_eq!(
            sanitize_origin("https://token:secret@gitee.com/example/demo.git"),
            "https://gitee.com/example/demo.git"
        );
        assert_eq!(
            sanitize_origin("git@gitee.com:example/demo.git"),
            "git@gitee.com:example/demo.git"
        );
        assert_eq!(
            sanitize_origin("token@gitee.com:example/demo.git"),
            "[redacted]@gitee.com:example/demo.git"
        );
    }
}
