//! OpenHarmony 官方文档本地镜像与检索（替代需登录的华为开发者文档站）。
//!
//! 思路：
//! 1. 从公开的 OpenHarmony 文档仓库（gitee/github 镜像，均无需登录）用
//!    `git sparse-checkout` 只拉取 `zh-cn/application-dev/reference/apis-*/` 的
//!    API 参考 Markdown（纯文本、体积可控、可离线检索）。
//! 2. 启动/更新后扫描建立内存索引（静态缓存，模式同 sdk_api.rs），
//!    提供按文件名/标题/正文关键字的检索。
//! 3. Agent 工具 search_harmony_docs / 健康检查页共用这套检索；
//!    华为官方文档站需要登录的内容（HarmonyOS NEXT 专属说明）则交给
//!    web_fetch 抓 docs.openharmony.cn 公开页面兜底。

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 文档仓库（公开、无需登录）。优先 gitee（国内快），失败回退 github 镜像。
pub const DOCS_REPO_GITEE: &str = "https://gitee.com/openharmony/docs.git";
pub const DOCS_REPO_GITHUB: &str = "https://github.com/eclipse-oniro-openharmony/docs.git";
/// sparse-checkout 只拉 API 参考目录（zh-cn/application-dev/reference 下所有 apis-*）
pub const DOCS_SPARSE_DIRS: &[&str] = &["zh-cn/application-dev/reference"];

/// 单篇文档条目（索引结果）
#[derive(Debug, Clone, Serialize)]
pub struct DocEntry {
    /// 相对路径，如 zh-cn/application-dev/reference/apis-basic-services-kit/js-apis-battery-info.md
    pub rel_path: String,
    /// 一级标题（去掉 # 与行内链接标记）
    pub title: String,
    /// 所属 Kit 目录名（apis-xxx-kit → xxx）
    pub kit: String,
    /// 正文纯文本预览（前 N 字符，去掉 markdown 标记）
    pub preview: String,
    /// 是否包含示例代码
    pub has_example: bool,
}

/// 文档索引
#[derive(Debug, Clone, Default)]
pub struct DocIndex {
    pub root: Option<String>,
    pub entries: Vec<DocEntry>,
}

/// 缓存：索引根目录 → DocIndex
static CACHE: Mutex<Option<DocIndex>> = Mutex::new(None);

pub fn invalidate() {
    if let Ok(mut c) = CACHE.lock() {
        *c = None;
    }
}

/// 返回文档库根目录（无则 None）：<app_data>/harmony-docs
pub fn docs_root(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().ok()?.join("harmony-docs");
    if dir.is_dir() {
        Some(dir)
    } else {
        None
    }
}

/// 索引是否存在（目录里至少有一个 .md）
pub fn is_downloaded(root: &Path) -> bool {
    let mut found = false;
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() && p.extension().is_some_and(|x| x == "md") {
                found = true;
                break;
            }
            if p.is_dir() && p.join("apis-arkui").is_dir() {
                found = true;
                break;
            }
        }
    }
    found
}

/// 统计已下载的 .md 数量（遍历可能慢，仅健康检查用）
pub fn count_docs(root: &Path) -> usize {
    fn walk(dir: &Path, n: &mut usize) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|s| s == ".git") {
                    continue;
                }
                walk(&p, n);
            } else if p.extension().is_some_and(|x| x == "md") {
                *n += 1;
            }
        }
    }
    let mut n = 0;
    walk(root, &mut n);
    n
}

/// 用 git sparse-checkout 下载/更新 OpenHarmony 文档（只拉 API 参考目录）。
/// 异步执行（内部 await 子进程输出），由 Tauri 命令在异步运行时调用。
/// use_proxy=true 时把系统代理注入 git 子进程环境变量（HTTPS_PROXY/HTTP_PROXY）。
/// 返回 (是否为首次下载) 或错误。
pub async fn sync_docs(root: &Path, prefer_gitee: bool, use_proxy: bool) -> Result<bool, String> {
    let _ = std::fs::create_dir_all(root);

    // 系统代理地址（显式注入 git 子进程，不污染进程级环境）
    let proxy_env: Option<String> = if use_proxy { crate::utils::net::read_system_proxy() } else { None };

    let is_repo = root.join(".git").is_dir() || root.join(".git").is_file();
    let urls: Vec<&str> = if prefer_gitee {
        vec![DOCS_REPO_GITEE, DOCS_REPO_GITHUB]
    } else {
        vec![DOCS_REPO_GITHUB, DOCS_REPO_GITEE]
    };

    // 统一执行辅助：运行 git 并检查退出码
    async fn run_git(args: &[&str], capture_stderr: bool, proxy_env: &Option<String>) -> Result<String, String> {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut cmd = crate::utils::process::command("git", &owned).map_err(|e| e.to_string())?;
        if let Some(p) = proxy_env {
            for var in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
                cmd.env(var, p);
            }
        }
        cmd.stdout(std::process::Stdio::null());
        if capture_stderr {
            cmd.stderr(std::process::Stdio::piped());
        } else {
            cmd.stderr(std::process::Stdio::null());
        }
        let out = cmd.output().await.map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        } else {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if err.is_empty() {
                format!("git {} 退出码 {}", args[0], out.status.code().unwrap_or(-1))
            } else {
                err
            })
        }
    }

    let r = root.to_str().unwrap_or("");

    if is_repo {
        // 已有仓库：逐源尝试更新（fetch + reset 到远端 master）
        let mut last_err = String::new();
        for url in &urls {
            let _ = run_git(&["-C", r, "remote", "set-url", "origin", url], false, &proxy_env).await;
            match run_git(
                &["-C", r, "fetch", "--depth=1", "origin", "master"],
                true,
                &proxy_env,
            )
            .await
            {
                Ok(_) => {
                    let _ = run_git(&["-C", r, "reset", "--hard", "origin/master"], false, &proxy_env).await;
                    invalidate();
                    return Ok(false);
                }
                Err(e) => last_err = e,
            }
        }
        return Err(format!("更新文档失败：{last_err}"));
    }

    // 首次：init + remote + fetch(--filter=blob:none) + checkout + sparse-checkout
    let mut last_err = String::new();
    for url in &urls {
        let _ = std::fs::remove_dir_all(root.join(".git"));
        if let Err(e) = run_git(&["init", r], false, &proxy_env).await {
            last_err = format!("git init 失败：{e}");
            continue;
        }
        if let Err(e) = run_git(&["-C", r, "remote", "add", "origin", url], false, &proxy_env).await {
            last_err = format!("git remote add 失败：{e}");
            continue;
        }
        if let Err(e) = run_git(
            &["-C", r, "fetch", "--depth=1", "--filter=blob:none", "origin", "master"],
            true,
            &proxy_env,
        )
        .await
        {
            last_err = format!("拉取文档仓库失败：{e}");
            continue;
        }
        if let Err(e) = run_git(&["-C", r, "checkout", "master"], true, &proxy_env).await {
            last_err = format!("切换 master 失败：{e}");
            continue;
        }
        let mut sparse_args = vec!["-C", r, "sparse-checkout", "set", "--no-cone"];
        for d in DOCS_SPARSE_DIRS {
            sparse_args.push(d);
        }
        if let Err(e) = run_git(&sparse_args, true, &proxy_env).await {
            last_err = format!("设置 sparse-checkout 失败：{e}");
            continue;
        }
        invalidate();
        return Ok(true);
    }
    Err(format!("下载文档失败：{last_err}"))
}

/// 解析 md 文本 → DocEntry（标题、kit、预览、是否含示例）
fn parse_md(root: &Path, rel: &str) -> Option<DocEntry> {
    let full = root.join(rel);
    let text = std::fs::read_to_string(&full).ok()?;
    // 一级标题：# 后第一个非空行；若 # 是链接形式(# [xx](url))则取链接文本
    let title = text
        .lines()
        .find(|l| l.trim_start().starts_with("# "))
        .map(|l| {
            let t = l.trim_start().trim_start_matches("# ").trim();
            strip_md_link(t)
        })
        .unwrap_or_else(|| {
            Path::new(rel)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(rel)
                .to_string()
        });
    // kit 目录名：apis-xxx-kit → xxx
    let kit = rel
        .split('/')
        .find(|seg| seg.starts_with("apis-"))
        .map(|seg| seg.trim_start_matches("apis-").trim_end_matches("-kit").to_string())
        .unwrap_or_default();
    // 预览：取正文（去掉标题），纯文本化，截断 300 字
    let body: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let preview = plain_preview(&body, 300);
    let has_example = text.contains("```");
    Some(DocEntry {
        rel_path: rel.replace('\\', "/"),
        title,
        kit,
        preview,
        has_example,
    })
}

/// 去掉 markdown 行内链接/标记： [文本](url) → 文本；开头标题标记（1~6 级 # + 空格）一并剥除
fn strip_md_link(s: &str) -> String {
    let s = {
        let trimmed = s.trim_start();
        let hash_count = trimmed.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hash_count) {
            let rest = &trimmed[hash_count..];
            rest.strip_prefix(' ').unwrap_or(rest)
        } else {
            trimmed
        }
    };
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' {
            // 找匹配的 ]( 与后括号
            if let Some(close) = chars[i + 1..].iter().position(|&c| c == ']') {
                let after = i + 1 + close;
                if after + 1 < chars.len() && chars[after + 1] == '(' {
                    let text: String = chars[i + 1..i + 1 + close].iter().collect();
                    out.push_str(&text);
                    i = after + 1;
                    // 跳到右括号
                    while i < chars.len() && chars[i] != ')' {
                        i += 1;
                    }
                    i += 1;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out.trim().to_string()
}

/// 正文纯文本预览：去 markdown 符号、压缩空白、截断 N 字
fn plain_preview(text: &str, max: usize) -> String {
    let stripped: String = text
        .lines()
        .map(|l| {
            let t = l.trim();
            let t = t.trim_start_matches(|c: char| "#*->`|".contains(c) || c == ' ' || c == '>');
            strip_md_link(t)
        })
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let compact = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(max).collect()
}

/// 构建索引（扫描全部 .md，做缓存）。返回索引的克隆。
pub fn index_docs(root: &Path) -> DocIndex {
    if let Ok(g) = CACHE.lock() {
        if let Some(idx) = g.as_ref() {
            if idx.root.as_deref() == Some(root.to_string_lossy().as_ref()) {
                return idx.clone();
            }
        }
    }
    let mut entries = Vec::new();
    fn walk(dir: &Path, root: &Path, out: &mut Vec<DocEntry>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if name == ".git" || name == ".github" {
                    continue;
                }
                walk(&p, root, out);
            } else if p.extension().is_some_and(|x| x == "md") {
                if let Ok(rel) = p.strip_prefix(root) {
                    if let Some(entry) = parse_md(root, &rel.to_string_lossy()) {
                        out.push(entry);
                    }
                }
            }
        }
    }
    walk(root, root, &mut entries);
    // 按 rel_path 排序保证稳定
    entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    let idx = DocIndex {
        root: Some(root.to_string_lossy().to_string()),
        entries,
    };
    if let Ok(mut g) = CACHE.lock() {
        *g = Some(idx.clone());
    }
    idx
}

/// 检索：文件名/标题/正文关键字打分，取前 limit 篇。
pub fn search(idx: &DocIndex, query: &str, limit: usize) -> Vec<DocEntry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return idx.entries.iter().take(limit).cloned().collect();
    }
    let mut scored: Vec<(i32, DocEntry)> = Vec::new();
    for e in &idx.entries {
        let file_lower = e.rel_path.to_lowercase();
        let title_lower = e.title.to_lowercase();
        let kit_lower = e.kit.to_lowercase();
        let preview_lower = e.preview.to_lowercase();
        let mut score = 0;
        // 文件名直接命中 @ohos.xxx 模块
        if file_lower.contains(&q) {
            score += 50;
        }
        // 模块名语义：如 "battery-info" 命中文件 js-apis-battery-info.md
        let stem = file_lower.rsplit('/').next().unwrap_or("");
        if !q.contains('.') && stem.contains(&q.replace('_', "-")) {
            score += 40;
        }
        if title_lower.contains(&q) {
            score += 30;
        }
        if kit_lower.contains(&q) {
            score += 20;
        }
        if preview_lower.contains(&q) {
            score += 10;
        }
        if score > 0 {
            scored.push((score, e.clone()));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.rel_path.cmp(&b.1.rel_path)));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, e)| e)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_md_link_basic() {
        assert_eq!(strip_md_link("# [bundle](https://x.md)"), "bundle");
        assert_eq!(strip_md_link("普通标题"), "普通标题");
        assert_eq!(strip_md_link("[AbilityInfo](a.md) 查询"), "AbilityInfo 查询");
    }

    #[test]
    fn parse_md_extracts_title_kit_example() {
        let dir = std::env::temp_dir().join(format!("hdocs-test-{}", std::process::id()));
        let api_dir = dir.join("zh-cn/application-dev/reference/apis-basic-services-kit");
        std::fs::create_dir_all(&api_dir).unwrap();
        std::fs::write(
            api_dir.join("js-apis-battery-info.md"),
            "# @ohos.batteryInfo (电量信息)\n\n提供电量信息查询。\n\n```ts\nlet level = batteryInfo.batterySOC\n```\n",
        )
        .unwrap();
        let rel = "zh-cn/application-dev/reference/apis-basic-services-kit/js-apis-battery-info.md";
        let entry = parse_md(&dir, rel).unwrap();
        assert!(entry.title.contains("batteryInfo"));
        assert_eq!(entry.kit, "basic-services");
        assert!(entry.has_example);
        assert!(entry.preview.contains("电量"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_scores_by_module_name() {
        let idx = DocIndex {
            root: None,
            entries: vec![
                DocEntry {
                    rel_path: "zh-cn/application-dev/reference/apis-basic-services-kit/js-apis-battery-info.md".into(),
                    title: "@ohos.batteryInfo (电量信息)".into(),
                    kit: "basic-services".into(),
                    preview: "电量信息".into(),
                    has_example: false,
                },
                DocEntry {
                    rel_path: "zh-cn/application-dev/reference/apis-ability-kit/js-apis-Bundle.md".into(),
                    title: "@ohos.bundle (Bundle模块)".into(),
                    kit: "ability".into(),
                    preview: "Bundle 模块".into(),
                    has_example: true,
                },
            ],
        };
        let hits = search(&idx, "battery", 5);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].rel_path.contains("battery-info"));
        let hits2 = search(&idx, "bundle", 5);
        assert_eq!(hits2.len(), 1);
        assert!(hits2[0].rel_path.contains("Bundle"));
    }
}
