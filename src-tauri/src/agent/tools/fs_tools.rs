//! 文件系统域工具：目录浏览 / 文件读取 / 搜索 / 写入 / 编辑 / 删除 / 移动 / 复制 / 撤销 / 批量编辑。
//! 共享辅助函数（truncate_out / stamps / run_cmd 等）在父模块 mod.rs，通过 `use super::*` 继承。

use super::*;

/// 读文件请求（宽松）：字段均可选；路径归一化/基础文件校验集中在 `resolve()` 显式落地。
#[derive(serde::Deserialize, Default)]
pub(super) struct ReadFileRequest {
    /// 文件路径（相对工程根或绝对路径，resolve 校验非空）
    pub path: Option<String>,
    /// 骨架模式：只输出结构定义行（import/类/函数等），快速了解大文件
    pub outline: Option<bool>,
    /// 起始行号（1 起，缺省 1）
    pub start: Option<u64>,
    /// 读取行数（缺省读到文件尾，单次最多 2000 行）
    pub lines: Option<u64>,
}

impl ReadFileRequest {
    /// 从工具入参解析宽松请求。
    pub(super) fn from_args(args: &Value) -> Result<Self, String> {
        serde_json::from_value(args.clone()).map_err(|e| format!("read_file 参数解析失败：{e}"))
    }

    /// 显式 resolve：路径非空/归一化/文件类型/大小上限校验，产出严格规范。
    pub(super) fn resolve(self, roots: &[String]) -> Result<ReadFileSpec, String> {
        let raw = self.path.as_deref().unwrap_or("");
        if raw.is_empty() {
            return Err("read_file 需要参数 {\"path\":\"<文件路径>\"}".into());
        }
        let p = resolve_in_roots(roots, raw)?;
        if !p.is_file() {
            return Err(format!("路径不是文件: {}", p.display()));
        }
        let meta = std::fs::metadata(&p).map_err(|e| e.to_string())?;
        if meta.len() > 1024 * 1024 {
            return Err(format!(
                "文件过大（{}，>1MB），无法直接读取：先用 grep_files 定位关键词所在行号，再 read_file 传 start/lines 分段读取，或用命令行工具处理",
                human_size(meta.len())
            ));
        }
        Ok(ReadFileSpec {
            path: p,
            outline: self.outline.unwrap_or(false),
            start: self.start.unwrap_or(1).max(1),
            lines: self.lines.unwrap_or(0),
        })
    }
}

/// 读文件规范（严格）：由 `ReadFileRequest::resolve()` 产出。
pub(super) struct ReadFileSpec {
    /// 已归一化且已验证的文件路径（≤1MB 的文本文件）
    pub path: PathBuf,
    /// 是否骨架模式
    pub outline: bool,
    /// 起始行号（≥1）
    pub start: u64,
    /// 读取行数（0 表示读到文件尾）
    pub lines: u64,
}

/// 写文件请求（宽松）：默认值/校验在 `resolve()` 显式落地。
#[derive(serde::Deserialize, Default)]
pub(super) struct WriteFileRequest {
    /// 目标路径（相对工程根或绝对路径）
    pub path: Option<String>,
    /// 要写入的内容（resolve 校验非空且 ≤1MB）
    pub content: Option<String>,
}

impl WriteFileRequest {
    pub(super) fn from_args(args: &Value) -> Result<Self, String> {
        serde_json::from_value(args.clone()).map_err(|e| format!("write_file 参数解析失败：{e}"))
    }

    /// 显式 resolve：路径/内容非空校验、大小上限、敏感文件保护、写入目标归一化。
    pub(super) fn resolve(self, roots: &[String]) -> Result<WriteFileSpec, String> {
        let raw = self.path.as_deref().unwrap_or("");
        if raw.is_empty() {
            return Err("write_file 需要参数 {\"path\":\"<文件路径>\",\"content\":\"<内容>\"}".into());
        }
        let content = self.content.ok_or("write_file 缺少 content 参数（要写入的内容）")?;
        if content.len() > 1024 * 1024 {
            return Err("内容超过 1MB，write_file 单次仅支持小文件，请拆分写入".into());
        }
        let p = resolve_for_write(roots, raw)?;
        if let Some(reason) = is_protected_file(&p) {
            return Err(format!("写入被安全策略拒绝：{reason}"));
        }
        if p.is_dir() {
            return Err(format!("目标是目录，无法写入: {}", p.display()));
        }
        Ok(WriteFileSpec { path: p, content })
    }
}

/// 写文件规范（严格）：由 `WriteFileRequest::resolve()` 产出。
pub(super) struct WriteFileSpec {
    /// 已归一化且通过安全校验的目标路径
    pub path: PathBuf,
    /// 已校验的内容（≤1MB）
    pub content: String,
}

/// 编辑文件请求（宽松）：默认值/校验在 `resolve()` 显式落地。
#[derive(serde::Deserialize, Default)]
pub(super) struct EditFileRequest {
    /// 目标路径
    pub path: Option<String>,
    /// 被替换的原文（resolve 校验非空；与 start 互斥）
    pub old: Option<String>,
    /// 替换成的新文（缺省空串）
    pub new: Option<String>,
    /// 是否全部替换（缺省 false，仅替换第一处）
    pub replace_all: Option<bool>,
    /// 代码块模式：按该行号定位所在完整方法/函数/代码块并整体替换（new 为空 = 整块删除）。
    /// 语言感知成对匹配：不固定行数，块有多长就替换多长，杜绝漏掉结束符。
    pub start: Option<u64>,
}

impl EditFileRequest {
    pub(super) fn from_args(args: &Value) -> Result<Self, String> {
        serde_json::from_value(args.clone()).map_err(|e| format!("edit_file 参数解析失败：{e}"))
    }

    /// 显式 resolve：路径归一化/文件校验/敏感保护 + old 非空校验（start 模式按代码块替换）。
    pub(super) fn resolve(self, roots: &[String]) -> Result<EditFileSpec, String> {
        let raw = self.path.as_deref().unwrap_or("");
        if raw.is_empty() {
            return Err("edit_file 需要参数 {\"path\":\"<文件路径>\",\"old\":\"<原文>\",\"new\":\"<新文>\"}".into());
        }
        if self.old.is_some() && self.start.is_some() {
            return Err("edit_file 的 old 与 start 参数互斥：old=精确文本替换；start=按代码块整体替换（start 定位所在完整方法/块，不固定行数）".into());
        }
        let old = self.old.unwrap_or_default();
        if old.is_empty() && self.start.is_none() {
            return Err("edit_file 需要 old 参数（原文片段），或用 start 参数按代码块整体替换".into());
        }
        let p = resolve_in_roots(roots, raw)?;
        if let Some(reason) = is_protected_file(&p) {
            return Err(format!("编辑被安全策略拒绝：{reason}"));
        }
        if !p.is_file() {
            return Err(format!("路径不是文件: {}", p.display()));
        }
        let meta = std::fs::metadata(&p).map_err(|e| e.to_string())?;
        if meta.len() > 1024 * 1024 {
            return Err("文件超过 1MB，请用 run_command 执行脚本处理或拆分修改".into());
        }
        Ok(EditFileSpec {
            path: p,
            old,
            new: self.new.unwrap_or_default(),
            replace_all: self.replace_all.unwrap_or(false),
            start: self.start,
        })
    }
}

/// 编辑文件规范（严格）：由 `EditFileRequest::resolve()` 产出。
pub(super) struct EditFileSpec {
    /// 已归一化且通过校验的目标文件
    pub path: PathBuf,
    /// 被替换的原文（非空）
    pub old: String,
    /// 替换成的新文
    pub new: String,
    /// 是否全部替换
    pub replace_all: bool,
    /// 代码块模式定位行（Some = 按完整代码块替换，忽略 old/replace_all）
    pub start: Option<u64>,
}

pub(super) async fn delete_file(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录，无法删除".into());
    }
    let rel = args["path"].as_str().unwrap_or("").trim();
    if rel.is_empty() {
        return Err("缺少 path 参数".into());
    }
    let root = Path::new(project_path);
    let target = root.join(rel);
    // 防越权：必须在项目根内
    let canon_target = target.canonicalize().map_err(|e| format!("路径无效: {e}"))?;
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if !canon_target.starts_with(&canon_root) {
        return Err("禁止删除项目目录之外的文件".into());
    }
    // 禁止删除受保护目录（与工具描述一致：版本库/依赖/产物/IDE 配置目录一律禁止）
    let name = canon_target.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let protected = [
        ".git", "oh_modules", "node_modules", ".ohpm", ".deveco-agent",
        "build", ".hvigor", ".idea", ".arkui-x",
    ];
    if protected.contains(&name) && canon_target.is_dir() {
        return Err(format!("受保护目录 {name} 不允许通过工具删除"));
    }
    if canon_target == canon_root {
        return Err("不能删除项目根目录".into());
    }
    if is_protected_file(&canon_target).is_some() {
        return Err("敏感文件（密钥/证书/迁移）不允许删除".into());
    }
    if !target.exists() {
        return Err(format!("文件不存在: {rel}"));
    }
    // 移到回收站（保留相对路径结构以便恢复）
    let trash_root = root.join(".deveco-agent").join("trash");
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dest = trash_root
        .join(ts.to_string())
        .join(rel.trim_start_matches(['/', '\\']));
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // 跨设备移动兜底：先 rename，失败则 copy + remove
    match std::fs::rename(&target, &dest) {
        Ok(_) => Ok(format!("已移至回收站: {}（可恢复）", dest.display())),
        Err(_) => {
            if target.is_dir() {
                copy_dir_recursive(&target, &dest)?;
                std::fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
            } else {
                std::fs::copy(&target, &dest).map_err(|e| e.to_string())?;
                std::fs::remove_file(&target).map_err(|e| e.to_string())?;
            }
            Ok(format!("已移至回收站: {}（可恢复）", dest.display()))
        }
    }
}

pub(super) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for e in std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
        let p = e.path();
        let t = dst.join(e.file_name());
        if p.is_dir() {
            copy_dir_recursive(&p, &t)?;
        } else {
            std::fs::copy(&p, &t).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// move_file：移动/重命名项目内文件或目录（不覆盖目标，禁止受保护路径）
pub(super) async fn move_file(args: &Value, roots: &[String]) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法移动文件".into());
    }
    let from_raw = args["from"].as_str().ok_or("move_file 需要参数 {\"from\":\"<源路径>\",\"to\":\"<目标路径>\"}")?.trim();
    let to_raw = args["to"].as_str().ok_or("move_file 缺少 to 参数（目标路径）")?.trim();
    if from_raw.is_empty() || to_raw.is_empty() {
        return Err("from/to 参数不能为空".into());
    }
    let src = resolve_in_roots(roots, from_raw)?;
    // 目标允许不存在（移动=重命名到新路径），用 resolve_for_write 解析：
    // 逐级校验最近存在祖先在根内 + 拼接剩余段，防 .. 越界与指向项目外；
    // 目标已存在时 resolve_for_write 内部先走 resolve_in_roots 返回原路径，由下方拒绝覆盖。
    let dst = resolve_for_write(roots, to_raw)?;
    if let Some(reason) = is_protected_file(&src) {
        return Err(format!("移动被安全策略拒绝：{reason}"));
    }
    if !src.exists() {
        return Err(format!("源路径不存在: {from_raw}"));
    }
    // 项目根本身不可移动（与 delete_file 同规则）
    let canon_src = src.canonicalize().map_err(|e| format!("路径无效: {e}"))?;
    for r in roots {
        let rc = std::fs::canonicalize(r).unwrap_or_else(|_| PathBuf::from(r));
        if canon_src == rc {
            return Err("不能移动项目根目录".into());
        }
    }
    // 受保护目录（版本库/依赖/产物/IDE 配置）不可移动，目标也不可落入其中
    const PROTECTED: [&str; 9] = [
        ".git", "oh_modules", "node_modules", ".ohpm", ".deveco-agent",
        "build", ".hvigor", ".idea", ".arkui-x",
    ];
    let sname = src.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if PROTECTED.contains(&sname) && src.is_dir() {
        return Err(format!("受保护目录 {sname} 不允许移动"));
    }
    for comp in dst.ancestors().skip(1) {
        if let Some(n) = comp.file_name().and_then(|s| s.to_str()) {
            if PROTECTED.contains(&n) && comp.is_dir() {
                return Err(format!("目标落入受保护目录 {n}，拒绝移动"));
            }
        }
    }
    if dst.exists() {
        return Err(format!("目标已存在，拒绝覆盖: {}", dst.display()));
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目标父目录失败 {}: {e}", parent.display()))?;
    }
    // 跨设备移动兜底：先 rename，失败则 copy + remove（与 delete_file 同模式）
    match std::fs::rename(&src, &dst) {
        Ok(_) => Ok(format!("已移动 {} → {}", src.display(), dst.display())),
        Err(_) => {
            if src.is_dir() {
                copy_dir_recursive(&src, &dst)?;
                std::fs::remove_dir_all(&src).map_err(|e| e.to_string())?;
            } else {
                std::fs::copy(&src, &dst).map_err(|e| e.to_string())?;
                std::fs::remove_file(&src).map_err(|e| e.to_string())?;
            }
            Ok(format!("已移动 {} → {}（跨设备复制方案）", src.display(), dst.display()))
        }
    }
}

/// undo_edit：按栈序（LIFO）恢复最近一次文件修改前的快照
pub(super) async fn undo_edit(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    let count = args["count"].as_u64().unwrap_or(1).clamp(1, 10) as usize;
    let mut restored: Vec<String> = Vec::new();
    for _ in 0..count {
        let Some(s) = crate::agent::undo::pop_undo(conversation_id) else {
            break;
        };
        // 恢复前校验路径仍在会话可见根内（跨项目快照不可恢复）
        let allowed = roots.iter().any(|r| {
            let rc = std::fs::canonicalize(r).unwrap_or_else(|_| PathBuf::from(r));
            s.path.starts_with(&rc)
        });
        if !allowed {
            continue;
        }
        if let Some(parent) = s.path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&s.path, &s.content)
            .map_err(|e| format!("恢复 {} 失败: {e}", s.path.display()))?;
        if let Ok(meta) = std::fs::metadata(&s.path) {
            stamp_put(&s.path, &meta, &s.content);
        }
        restored.push(s.path.display().to_string());
    }
    if restored.is_empty() {
        return Ok("没有可撤销的修改（本会话尚无 Agent 文件写入记录）".into());
    }
    let remain = crate::agent::undo::undo_count(conversation_id);
    let mut out = format!(
        "已撤销 {} 处修改（剩余可撤销 {remain} 步）：\n",
        restored.len()
    );
    for p in &restored {
        out.push_str(&format!("- {p}\n"));
    }
    Ok(out)
}

/// get_diagnostics：返回近期构建/部署失败的结构化归因清单（进程内缓存，1 小时 TTL）

pub(super) fn should_skip_dir(name: &str) -> bool {
    SKIP_DIRS.iter().any(|s| *s == name)
}

/// 人类可读文件大小
pub(super) fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n}B")
    } else {
        format!("{v:.1}{}", UNITS[u])
    }
}

pub(super) fn fmt_time(ts: std::time::SystemTime) -> String {
    let d: chrono::DateTime<chrono::Local> = ts.into();
    d.format("%Y-%m-%d %H:%M").to_string()
}

/// 简单 glob 匹配（支持 * 单层、** 任意层级、? 单字符；不区分大小写；* 不跨 /）
pub(super) fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    fn m(p: &[char], t: &[char]) -> bool {
        if p.is_empty() {
            return t.is_empty();
        }
        match p[0] {
            '*' => {
                if p.len() > 1 && p[1] == '*' {
                    if p.len() > 2 && p[2] == '/' {
                        // **/：匹配零个或多个路径段
                        if m(&p[3..], t) {
                            return true;
                        }
                        for i in 0..t.len() {
                            if t[i] == '/' && m(&p[3..], &t[i + 1..]) {
                                return true;
                            }
                        }
                        return false;
                    }
                    // **：匹配任意内容（含 /）
                    for i in 0..=t.len() {
                        if m(&p[2..], &t[i..]) {
                            return true;
                        }
                    }
                    return false;
                }
                // *：不跨 /
                for i in 0..=t.len() {
                    if t.get(i) == Some(&'/') {
                        break;
                    }
                    if m(&p[1..], &t[i..]) {
                        return true;
                    }
                }
                false
            }
            '?' => !t.is_empty() && t[0] != '/' && m(&p[1..], &t[1..]),
            c => !t.is_empty() && t[0].eq_ignore_ascii_case(&c) && m(&p[1..], &t[1..]),
        }
    }
    m(&p, &t)
}

/// 单目录最多展示条目数：超出折叠为一行汇总，避免静态资源/生成物把输出刷爆后整体截断（截断对模型无用）
const DIR_SHOW_MAX: usize = 30;
/// . 开头目录白名单（默认隐藏目录全部跳过，仅保留对 AI 有用的）
const HIDDEN_ALLOW: [&str; 1] = [".github"];
/// 单壳目录穿透预算：只有单个子目录且无文件的目录链（如 entry/src/main/ets）自动展开，不消耗 depth
const SHELL_PENETRATE_MAX: usize = 3;

/// .gitignore 规则（简化子集：! 取反、结尾 / 仅目录、/ 开头锚定根、含 / 按相对路径匹配，否则按条目名匹配任意层级）
#[derive(Clone)]
struct IgnoreRule {
    pattern: String,
    negate: bool,
    dir_only: bool,
    anchored: bool,
    /// 规则基准：相对项目根的目录（如 "entry"，规则仅作用于该目录及以下）；空 = 项目根
    base: String,
}

/// 解析 .gitignore：仅支持本项目需要的子集（完整 git 语义过于复杂，够用即可）
fn parse_gitignore(content: &str) -> Vec<IgnoreRule> {
    parse_gitignore_at(content, "")
}

/// 解析某子目录下的 .gitignore：规则以 base（相对项目根）为基准
fn parse_gitignore_at(content: &str, base: &str) -> Vec<IgnoreRule> {
    let mut rules = Vec::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut pat = line;
        let mut negate = false;
        if let Some(rest) = pat.strip_prefix('!') {
            negate = true;
            pat = rest;
        }
        if pat.is_empty() {
            continue;
        }
        let dir_only = pat.ends_with('/');
        if dir_only {
            pat = pat.trim_end_matches('/');
        }
        // / 开头：锚定到规则所在目录（按相对路径匹配，防止 "/build" 误伤子层同名目录）
        let anchored = pat.starts_with('/');
        if anchored {
            pat = pat.trim_start_matches('/');
        }
        if pat.is_empty() {
            continue;
        }
        rules.push(IgnoreRule {
            pattern: pat.to_string(),
            negate,
            dir_only,
            anchored,
            base: base.to_string(),
        });
    }
    rules
}

/// 相对路径与规则基准的关系：base 为空 → 全匹配；否则 rel 必须是 base 自身或 base 的子路径
/// （子目录 .gitignore 只作用于其目录及以下；返回剥离基准后的相对路径用于匹配）
fn rel_vs_base<'a>(rel: &'a str, base: &str) -> Option<&'a str> {
    if base.is_empty() {
        return Some(rel);
    }
    if rel == base {
        return Some("");
    }
    rel.strip_prefix(base).and_then(|rest| rest.strip_prefix('/'))
}

/// 判断条目是否被 gitignore 规则忽略（后置规则覆盖前置；目录与文件共用条目名匹配）
fn gitignore_ignored(rules: &[IgnoreRule], name: &str, rel: &str, is_dir: bool) -> bool {
    let mut ignored = false;
    for r in rules {
        if r.dir_only && !is_dir {
            continue;
        }
        // 子目录规则先验基准：不在 base 子树内的条目直接跳过
        let Some(eff_rel) = rel_vs_base(rel, &r.base) else { continue };
        // 锚定或含 / 的规则按相对路径匹配，否则按条目名匹配（任意层级）
        let hit = if r.anchored || r.pattern.contains('/') {
            glob_match(&r.pattern, eff_rel)
        } else {
            glob_match(&r.pattern, name)
        };
        if hit {
            ignored = !r.negate;
        }
    }
    ignored
}

/// 查找包含 root 的项目根（roots 中首个包含者），加载其 .gitignore 规则；
/// 返回 (规则, root 相对项目根的路径，作为相对路径匹配基准)
fn load_project_ignore(root: &Path, roots: &[String]) -> (Vec<IgnoreRule>, String) {
    let mut proj_root: Option<PathBuf> = None;
    for r in roots {
        let Ok(rc) = std::fs::canonicalize(r) else { continue };
        // canonicalize 在 Windows 带 \?\ 前缀，与 resolve_in_roots 返回的规范化路径不一致，
        // 不归一化会导致 path_within 字符串比较失败 → gitignore 规则静默失效
        let rc = PathBuf::from(crate::utils::path::normalize_path(&rc.to_string_lossy()));
        if crate::utils::path::path_within(root, &rc) {
            proj_root = Some(rc);
            break;
        }
    }
    let mut rules = Vec::new();
    if let Some(pr) = &proj_root {
        if let Ok(content) = std::fs::read_to_string(pr.join(".gitignore")) {
            rules = parse_gitignore(&content);
        }
    }
    let start_rel = proj_root
        .as_ref()
        .and_then(|pr| root.strip_prefix(pr).ok())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    (rules, start_rel)
}

/// 子目录 .gitignore 合并：本目录的规则以 rel 为基准，只对其子树生效，与父规则合并后向下传递
fn load_child_rules(rules: &[IgnoreRule], dir: &Path, rel: &str) -> Vec<IgnoreRule> {
    if rel.is_empty() {
        return rules.to_vec();
    }
    match std::fs::read_to_string(dir.join(".gitignore")) {
        Ok(content) => {
            let mut v = rules.to_vec();
            v.extend(parse_gitignore_at(&content, rel));
            v
        }
        Err(_) => rules.to_vec(),
    }
}

/// 注释折叠阈值：连续注释行达到该值才折叠为一行摘要（短注释块信息密度高，保留原文）
const COMMENT_FOLD_MIN: usize = 8;

/// 代码语言扩展名（注释识别/折叠只对代码生效；文本/数据文件如 .md 的 # 是标题、.json 无注释，误伤代价高）
fn is_code_ext(ext: &str) -> bool {
    matches!(
        ext,
        "ets" | "ts" | "tsx" | "js" | "jsx" | "rs" | "py" | "java" | "kt" | "swift"
            | "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "go" | "sql" | "lua"
            | "sh" | "bash" | "bat" | "cmd" | "ps1" | "css" | "scss" | "less"
            | "vue" | "svelte" | "php" | "rb" | "cs" | "dart" | "m" | "mm"
    )
}

/// 判断某行是否为注释行（C 系 // /* */ *，Python/Shell #，SQL/Lua --）
fn is_comment_line(line: &str, ext: &str) -> bool {
    if !is_code_ext(ext) {
        return false;
    }
    let t = line.trim_start();
    if t.is_empty() {
        return false;
    }
    if t.starts_with("//") || t.starts_with("/*") || t.starts_with('*') || t.starts_with("*/") {
        return true;
    }
    if (ext == "py" || ext == "sh" || ext == "bash" || ext == "ps1" || ext == "rb")
        && t.starts_with('#')
    {
        return true;
    }
    if (ext == "sql" || ext == "lua") && t.starts_with("--") {
        return true;
    }
    false
}

/// 低价值文件聚合：扩展名 → 类别（这类文件逐条列出对理解结构无益，按类别汇总一行即可）
fn agg_category(name: &str) -> Option<&'static str> {
    const EXT: &[(&str, &str)] = &[
        ("png", "图片"), ("jpg", "图片"), ("jpeg", "图片"), ("gif", "图片"),
        ("webp", "图片"), ("svg", "图片"), ("ico", "图片"), ("bmp", "图片"),
        ("avif", "图片"), ("heic", "图片"),
        ("ttf", "字体"), ("otf", "字体"), ("woff", "字体"), ("woff2", "字体"), ("eot", "字体"),
        ("mp3", "媒体"), ("mp4", "媒体"), ("avi", "媒体"), ("mkv", "媒体"),
        ("mov", "媒体"), ("wav", "媒体"), ("flac", "媒体"), ("webm", "媒体"),
        ("zip", "压缩包"), ("rar", "压缩包"), ("7z", "压缩包"), ("tar", "压缩包"),
        ("gz", "压缩包"), ("jar", "压缩包"), ("har", "压缩包"),
        ("bin", "二进制"), ("exe", "二进制"), ("dll", "二进制"), ("so", "二进制"),
        ("apk", "安装包"), ("hap", "安装包"), ("hsp", "安装包"),
        ("db", "数据库"), ("sqlite", "数据库"), ("sqlite3", "数据库"),
        ("log", "日志"), ("map", "构建产物"),
    ];
    let lower = name.to_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    EXT.iter().find(|(e, _)| *e == ext).map(|(_, c)| *c)
}

/// 标志文件（★ 标注：配置/清单/说明类，帮模型快速定位关键文件）
fn is_mark_file(name: &str) -> bool {
    matches!(
        name,
        "pom.xml" | "package.json" | "build-profile.json5" | "oh-package.json5"
            | "Cargo.toml" | "go.mod" | "build.gradle" | "build.gradle.kts"
            | "settings.gradle" | "settings.gradle.kts" | "pyproject.toml"
            | "requirements.txt" | "README.md" | "AGENTS.md" | ".gitignore"
            | "hvigorfile.ts" | "hvigorfile.js" | "tsconfig.json" | "vite.config.ts"
            | "vite.config.js" | "webpack.config.js" | "Dockerfile" | "docker-compose.yml"
    )
}

/// 项目类型探测：根目录标志文件（HarmonyOS 优先，避免与 Node 误判）
fn detect_project_type(root: &Path) -> Option<(String, String)> {
    const MARKS: &[(&str, &str)] = &[
        ("HarmonyOS 工程（DevEco）", "build-profile.json5"),
        ("Java 工程（Maven）", "pom.xml"),
        ("Gradle 工程", "build.gradle"),
        ("Node.js 工程", "package.json"),
        ("Rust 工程（Cargo）", "Cargo.toml"),
        ("Go 工程", "go.mod"),
        ("Python 工程", "pyproject.toml"),
    ];
    for (kind, mark) in MARKS {
        if root.join(mark).is_file() {
            return Some((kind.to_string(), mark.to_string()));
        }
    }
    // .NET 工程以解决方案/项目文件命名（无法用固定文件名，扫一级）
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.ends_with(".sln") || n.ends_with(".csproj") {
                return Some((".NET 工程".to_string(), n));
            }
        }
    }
    None
}

/// 目录浏览统计（跨层累计）
#[derive(Default)]
struct ListStats {
    dirs: usize,
    files: usize,
    bytes: u64,
    /// 聚合类别 → (数量, 字节)
    agg: std::collections::BTreeMap<&'static str, (usize, u64)>,
}

pub(super) async fn list_dir(args: &Value, roots: &[String]) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法浏览文件".into());
    }
    let raw = args["path"].as_str().unwrap_or(".");
    let depth = args["depth"].as_u64().unwrap_or(1).clamp(1, 3) as u32;
    let root = resolve_in_roots(roots, raw)?;
    if !root.is_dir() {
        return Err(format!("路径不是目录: {}", root.display()));
    }
    // .gitignore 规则：项目根规则 + 浏览目标自身规则
    // （root 非项目根时，其自身 .gitignore 由 walk 首层按子规则加载，基准为 root 自身，语义等价）
    let (ignore_rules, start_rel) = load_project_ignore(&root, roots);

    let mut out = String::new();
    // 浏览项目根时附带项目类型识别，帮助模型快速定位技术栈；
    // Git 仓库提示：list_dir 只反映文件系统现状，查变更/历史应转用 git 工具（自 root 向上查找 .git）
    let in_git = root.ancestors().any(|a| a.join(".git").exists());
    if let Some((kind, mark)) = detect_project_type(&root) {
        let hint = if in_git {
            "；目录在 Git 仓库内，可用 git status 查变更、git log 查历史"
        } else {
            ""
        };
        out.push_str(&format!("（项目类型：{kind}，依据：{mark}{hint}）\n"));
    } else if in_git {
        out.push_str("（目录在 Git 仓库内，可用 git status 查变更、git log 查历史）\n");
    }
    let mut skipped = 0u32;
    let mut stats = ListStats::default();
    fn walk(
        dir: &Path,
        rel: &str,
        depth: u32,
        max_depth: u32,
        shell_left: usize,
        chain: String,
        rules: &[IgnoreRule],
        stats: &mut ListStats,
        skipped: &mut u32,
        out: &mut String,
    ) -> Result<(), String> {
        // 本目录的 .gitignore（多模块工程/子仓库常见）：规则只对其所在子树生效，
        // 与父规则合并后向下传递（项目根已在外层加载，rel 为空时跳过）
        let child_rules = load_child_rules(rules, dir, rel);
        let entries =
            std::fs::read_dir(dir).map_err(|e| format!("读取目录失败 {}: {e}", dir.display()))?;
        let mut items: Vec<(String, bool, u64, std::time::SystemTime)> = Vec::new();
        let mut agg: std::collections::BTreeMap<&'static str, (usize, u64)> =
            std::collections::BTreeMap::new();
        let mut folded = 0usize; // 超出 DIR_SHOW_MAX 后折叠的条目数
        let mut folded_dirs: Vec<String> = Vec::new(); // 被折叠的目录名（前几个，提示模型可继续深入）
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(ft) = e.file_type() else { continue };
            let is_dir = ft.is_dir();
            // 隐藏目录（. 开头，白名单除外）+ 已知忽略目录 + .gitignore 命中 → 跳过
            // 注意：含 / 的规则（如 src/main/）按条目自身相对路径匹配，而不是父目录路径
            let hidden =
                is_dir && name.starts_with('.') && !HIDDEN_ALLOW.contains(&name.as_str());
            let entry_rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if hidden || should_skip_dir(&name)
                || gitignore_ignored(&child_rules, &name, &entry_rel, is_dir)
            {
                *skipped += 1;
                continue;
            }
            if is_dir {
                stats.dirs += 1;
                if items.len() < DIR_SHOW_MAX {
                    items.push((name, true, 0, std::time::UNIX_EPOCH));
                } else {
                    folded += 1;
                    if folded_dirs.len() < 5 {
                        folded_dirs.push(name);
                    }
                }
            } else if let Some(cat) = agg_category(&name) {
                // 低价值文件：只聚合统计，不逐条列出（大图/字体/压缩包对结构理解无益）
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                let ent = agg.entry(cat).or_insert((0, 0));
                ent.0 += 1;
                ent.1 += size;
                stats.files += 1;
                stats.bytes += size;
            } else if is_mark_file(&name) || items.len() < DIR_SHOW_MAX {
                // 关键配置/清单文件（★）不受折叠限制：结构信息永远可见
                let Ok(meta) = e.metadata() else { continue };
                let size = meta.len();
                let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                stats.files += 1;
                stats.bytes += size;
                items.push((name, false, size, mtime));
            } else {
                folded += 1;
                // 折叠文件也计入统计，保证汇总数字真实
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                stats.files += 1;
                stats.bytes += size;
            }
        }
        for (k, (n, sz)) in agg {
            let ent = stats.agg.entry(k).or_insert((0, 0));
            ent.0 += n;
            ent.1 += sz;
        }
        // 单壳目录穿透：仅 1 个子目录且无文件/聚合/折叠时，合并路径继续深入（不消耗 depth）
        if shell_left > 0 && items.len() == 1 && items[0].1 && folded == 0 {
            let sub = items[0].0.clone();
            let child_rel = if rel.is_empty() {
                sub.clone()
            } else {
                format!("{rel}/{sub}")
            };
            // 先借用 sub 构造路径，再 move 进 new_chain（顺序敏感：sub 随后被转移）
            let joined = dir.join(&sub);
            let new_chain = if chain.is_empty() {
                sub
            } else {
                format!("{chain}/{sub}")
            };
            return walk(
                &joined,
                &child_rel,
                depth,
                max_depth,
                shell_left - 1,
                new_chain,
                &child_rules,
                stats,
                skipped,
                out,
            );
        }
        // 目录优先，同级按名称排序
        items.sort_by(|a, b| {
            b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
        });
        let indent = "  ".repeat(depth as usize);
        for (name, is_dir, size, mtime) in &items {
            if *is_dir {
                let disp = if chain.is_empty() {
                    name.clone()
                } else {
                    format!("{chain}/{name}")
                };
                out.push_str(&format!("{indent}[目录] {disp}/\n"));
                // 目录超限（folded>0）时结构不完整，不再深入，避免输出缺块误导模型
                if depth < max_depth && folded == 0 {
                    let child_rel = if rel.is_empty() {
                        name.clone()
                    } else {
                        format!("{rel}/{name}")
                    };
                    walk(
                        &dir.join(name),
                        &child_rel,
                        depth + 1,
                        max_depth,
                        SHELL_PENETRATE_MAX,
                        String::new(),
                        &child_rules,
                        stats,
                        skipped,
                        out,
                    )?;
                }
            } else {
                let star = if is_mark_file(name) { "★" } else { "" };
                out.push_str(&format!(
                    "{indent}{star}[文件] {name}  {}  {}\n",
                    human_size(*size),
                    fmt_time(*mtime)
                ));
            }
        }
        if folded > 0 {
            let dir_hint = if folded_dirs.is_empty() {
                String::new()
            } else {
                format!(
                    "（含目录：{} 等）",
                    folded_dirs.iter().map(|d| format!("{d}/")).collect::<Vec<_>>().join("、")
                )
            };
            out.push_str(&format!(
                "{indent}…（该目录共 {} 项，其余 {folded} 项省略{dir_hint}；如需查看请直接对该目录再调 list_dir）\n",
                items.len() + folded
            ));
        }
        Ok(())
    }
    walk(
        &root,
        &start_rel,
        0,
        depth,
        SHELL_PENETRATE_MAX,
        String::new(),
        &ignore_rules,
        &mut stats,
        &mut skipped,
        &mut out,
    )?;
    if out.is_empty() {
        out.push_str("（空目录）\n");
    }
    // 汇总：计数 + 聚合类别（低价值文件不逐条列出但明确告知，避免模型误以为目录里没有）
    out.push_str(&format!(
        "（目录: {}，共 {} 个文件、{} 个目录，总大小 {}；已跳过 {skipped} 个忽略项）\n",
        root.display(),
        stats.files,
        stats.dirs,
        human_size(stats.bytes)
    ));
    if !stats.agg.is_empty() {
        let parts: Vec<String> = stats
            .agg
            .iter()
            .map(|(c, (n, sz))| format!("{c} {n} 个/{}", human_size(*sz)))
            .collect();
        out.push_str(&format!(
            "（聚合未逐一列出：{}；如需查看请对该目录单独 list_dir）\n",
            parts.join("、")
        ));
    }
    // 保尾截断：统计汇总在尾部，超长时纯截头会让模型拿到残缺结构认知
    Ok(truncate_out_head_tail(&out, 3000))
}

pub(super) async fn read_file(args: &Value, roots: &[String]) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法读取文件".into());
    }
    // Request/Spec 分离：宽松参数 ReadFileRequest → 显式 resolve() 产出严格规范 ReadFileSpec
    let spec = ReadFileRequest::from_args(args)?.resolve(roots)?;
    let p = &spec.path;
    let meta = std::fs::metadata(p).map_err(|e| e.to_string())?;
    let bytes = std::fs::read(p).map_err(|e| format!("读取文件失败: {e}"))?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err(format!(
            "文件是二进制（{}），无法以文本方式读取",
            human_size(meta.len())
        ));
    }
    // 记录文件指纹（外部修改检测基线）
    stamp_put(p, &meta, &bytes);
    // 编码：UTF-8 严格校验；非 UTF-8 用 GBK 解码展示（Windows 老项目常见），并标注编码避免误导
    let (text, enc_note) = match std::str::from_utf8(&bytes) {
        Ok(s) => {
            // BOM 剥离展示：BOM 是不可见字符，模型拼接 old 时不会带 BOM，
            // 展示中保留会让首行看起来与原文不同；剥离并标注。
            if let Some(b) = s.strip_prefix('\u{feff}') {
                (b.to_string(), "（UTF-8 含 BOM，编辑时自动兼容）".to_string())
            } else {
                (s.to_string(), String::new())
            }
        }
        Err(_) => {
            let (t, _, had_err) = encoding_rs::GBK.decode(&bytes);
            if had_err {
                (String::from_utf8_lossy(&bytes).to_string(), "（编码：非 UTF-8/GBK，展示可能失真）".to_string())
            } else {
                (t.into_owned(), "（编码：GBK，非 UTF-8，只读展示；如需编辑请先转换为 UTF-8）".to_string())
            }
        }
    };
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();

    // 骨架模式：只提取结构（导入/类/函数/接口/组件/装饰器等），帮助 Agent 先了解大文件
    if spec.outline {
        return Ok(render_outline(p, &lines, meta.len()));
    }

    let start = spec.start as usize;
    let limit = spec.lines as usize;
    // start 超出总行数：明确提示，避免误输出“文件为空”误导模型
    if total > 0 && start > total {
        return Ok(format!(
            "start={start} 超出文件总行数（共 {total} 行），请调小 start，或传 lines 参数从文件头部读取"
        ));
    }
    let begin = if start > total { total } else { start - 1 };
    let end = if limit > 0 {
        (begin + limit).min(total)
    } else {
        total
    };

    // ---- 语言感知的代码块对齐 ----
    // 原则：读取窗口绝不把代码块截断在中间。窗口起点若落在方法/函数内部 → 上扩到完整块首行；
    // 窗口末尾若仍在块内 → 补齐到块尾（结束符一定可见）。
    // 依据扩展名识别语言：花括号语言按 {}()[] 成对匹配，Python 按 def/class 缩进块。
    // 文本/数据类文件（.md/.json/.txt 等）无结构化块，行为与旧版一致。
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mut win_begin = begin;
    let mut win_end = end;
    // 1) 起始行上扩：begin 在方法级根块内部 → 从完整块首行开始展示
    if win_begin < total {
        if let Some((o, _)) = find_root_block(&lines, win_begin, &ext) {
            if o < win_begin {
                win_begin = o;
            }
        }
    }
    // 2) 结束行补齐：窗口最后一行仍处块内 → 延伸到块尾（锚定 win_begin，
    //    与窗口起点同一层级的块，避免跳进内层块而漏掉外层方法尾部）
    if win_begin < total && win_end > win_begin && win_end < total {
        let last = win_end - 1;
        if let Some((_, c)) = find_block_anchored(&lines, last, win_begin, &ext) {
            if c + 1 > win_end {
                win_end = (c + 1).min(total);
            }
        }
    }
    let block_note = if win_begin != begin || win_end != end {
        format!(
            "（已按语言代码块自动对齐 L{}-L{}：保证方法/块完整、不丢结束符）\n",
            win_begin + 1, win_end
        )
    } else {
        String::new()
    };

    // 单次输出上限：普通模式 2000 行 / 15000 字符；代码块补齐场景预算放大到 40000 字符，
    // 尽量完整输出整个方法。病理级超大块由下方给出完整行区间与结束符位置提示。
    const MAX_LINES: usize = 2000;
    const MAX_CHARS: usize = 15000;
    const BLOCK_CHARS: usize = 40000;
    let block_expanded = !block_note.is_empty();
    let char_budget = if block_expanded { BLOCK_CHARS } else { MAX_CHARS };
    let line_budget = if block_expanded { MAX_LINES * 2 } else { MAX_LINES };
    // 注释清洗：文件注释占比高时，连续长注释块折叠为一行摘要。
    // 痛点：license/文件头大段注释会吃掉字符预算，代码本身还没读到就被截断，
    // 喂给模型的是一堆注释而看不到代码。短注释块信息密度高，保留原文。
    let comment_ratio = if is_code_ext(&ext) {
        lines.iter().filter(|l| is_comment_line(l, &ext)).count() as f64 / total.max(1) as f64
    } else {
        0.0
    };
    let mut folded_runs = 0usize; // 折叠的注释块数
    let mut folded_lines = 0usize; // 折叠的注释总行数
    fn flush_comment_buf(
        out: &mut String,
        buf: &[usize],
        lines: &[&str],
        shown_lines: &mut usize,
        folded_runs: &mut usize,
        folded_lines: &mut usize,
    ) {
        if buf.is_empty() {
            return;
        }
        let run = buf.len();
        if run >= COMMENT_FOLD_MIN {
            let first = buf[0] + 1;
            let last = buf[run - 1] + 1;
            out.push_str(&format!(
                "{first:>5} │ …(L{first}-L{last}：{run} 行注释已折叠；start={first}/lines={run} 可看原文)…\n"
            ));
            *folded_runs += 1;
            *folded_lines += run;
        } else {
            // 短注释块：逐行展示
            for abs in buf {
                let s: String = lines[*abs].chars().take(240).collect();
                out.push_str(&format!("{:>5} │ {s}\n", abs + 1));
            }
        }
        *shown_lines += run;
    }
    let mut out = String::new();
    let mut shown_lines = 0usize; // 已展示的真实行数（截断时用于提示续读位置）
    let mut cut_block: Option<(usize, usize)> = None; // 字符/行数截断命中的块（用于完整区间提示）
    let mut comment_buf: Vec<usize> = Vec::new(); // 累积的连续注释行（绝对行号）
    for (i, line) in lines[win_begin..win_end].iter().enumerate() {
        if i >= line_budget || out.chars().count() >= char_budget {
            // 命中预算：若窗口结束符仍不可见，记录截断处所在块，给出完整区间与结束符位置
            let cut_abs = (win_begin + i).saturating_sub(1).min(win_end - 1);
            if block_expanded && cut_abs >= win_begin {
                cut_block = find_enclosing_block(&lines, cut_abs, &ext);
            }
            break;
        }
        let abs = win_begin + i;
        // 注释行先累积（判断是否成块），代码行先 flush 注释块再输出
        if comment_ratio > 0.35 && is_comment_line(line, &ext) {
            comment_buf.push(abs);
            continue;
        }
        flush_comment_buf(
            &mut out,
            &comment_buf,
            &lines,
            &mut shown_lines,
            &mut folded_runs,
            &mut folded_lines,
        );
        comment_buf.clear();
        let snippet: String = line.chars().take(240).collect();
        out.push_str(&format!("{:>5} │ {snippet}\n", abs + 1));
        shown_lines += 1;
    }
    // 窗口结尾的注释块同样处理
    flush_comment_buf(
        &mut out,
        &comment_buf,
        &lines,
        &mut shown_lines,
        &mut folded_runs,
        &mut folded_lines,
    );
    // 截断提示：优先给出“块没读完”的完整区间与结束符行号（防模型基于残缺片段编辑）；
    // 普通截断则告知已读到第几行、从哪继续。
    if shown_lines < win_end - win_begin {
        if let Some((o, c)) = cut_block.filter(|(_, c)| *c >= (win_begin + shown_lines).saturating_sub(1)) {
            let block_lines = c - o + 1;
            let block_chars: usize = lines[o..=c].iter().map(|l| l.chars().count()).sum();
            out.push_str(&format!(
                "…(代码块过大：所在块跨 L{}-L{}（{} 行、约 {} 字符），超出单次输出上限 {char_budget} 字符；结束符在第 {} 行。如需完整读取请传 start={}/lines={})\n",
                o + 1,
                c + 1,
                block_lines,
                block_chars,
                c + 1,
                o + 1,
                block_lines
            ));
        } else {
            out.push_str(&format!(
                "…(已截断，共 {total} 行，仅显示到第 {} 行；可传 start={} 继续读取)\n",
                win_begin + shown_lines,
                win_begin + shown_lines + 1
            ));
        }
    }
    if out.is_empty() {
        out.push_str("（文件为空或没有可显示的内容）\n");
    }
    // 注释清洗提示：折叠发生时说明原文结构（模型可据此决定是否 outline 或精读注释区）
    let comment_note = if folded_lines > 0 {
        format!(
            "（注释占 {:.0}%，长注释块已折叠 {folded_runs} 处/共 {folded_lines} 行；可用 outline=true 查看结构骨架）\n",
            comment_ratio * 100.0
        )
    } else if comment_ratio >= 0.4 {
        format!(
            "（注释占 {:.0}%，文件注释较多；可用 outline=true 查看结构骨架）\n",
            comment_ratio * 100.0
        )
    } else {
        String::new()
    };
    Ok(truncate_out_max(
        &format!(
            "文件 {}{enc_note}（{}，共 {total} 行）：\n{block_note}{comment_note}{out}",
            p.display(),
            human_size(meta.len())
        ),
        if block_expanded { BLOCK_CHARS + 2000 } else { 15000 },
    ))
}

/// 生成代码文件骨架：仅保留结构定义行（导入、类/接口/struct/enum、函数/方法、组件、装饰器），
/// 帮助 Agent 在不读取全文的情况下快速了解大文件结构，再决定精读哪段。
pub(super) fn render_outline(p: &Path, lines: &[&str], byte_len: u64) -> String {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mut out = String::new();
    let mut last_was_import = false;
    let mut import_count = 0usize;
    let mut shown = 0usize;
    const MAX_ENTRIES: usize = 200;

    for (idx, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('*') || line.starts_with("/*") {
            continue;
        }
        let lineno = idx + 1;

        // 导入语句：折叠显示（只给首尾与数量），避免 import 区占用大量篇幅
        let is_import = line.starts_with("import ")
            || line.starts_with("import\t")
            || line.starts_with("use ")
            || line.starts_with("const ") && line.contains("require(")
            || line.starts_with("#import");
        if is_import {
            import_count += 1;
            last_was_import = true;
            continue;
        } else if last_was_import {
            out.push_str(&format!("  … ({import_count} 条导入已折叠)…\n"));
            last_was_import = false;
        }

        if shown >= MAX_ENTRIES {
            continue;
        }

        let kind = outline_kind(line, &ext);
        if let Some(label) = kind {
            let snippet: String = raw.chars().take(160).collect();
            out.push_str(&format!("{lineno:>5} │ {label} {}\n", snippet.trim()));
            shown += 1;
        }
    }
    if last_was_import {
        out.push_str(&format!("  … ({import_count} 条导入已折叠)…\n"));
    }

    let mut header = format!(
        "文件 {}（{}，共 {} 行）骨架：\n",
        p.display(),
        human_size(byte_len),
        lines.len()
    );
    if out.is_empty() {
        header.push_str("（未能识别出结构化定义，可能是非代码文件或使用了非常规语法；请用 start/lines 直接读取）\n");
    } else {
        header.push_str(&out);
        if shown >= MAX_ENTRIES {
            header.push_str(&format!("…(结构项超过 {MAX_ENTRIES}，已截断；可用 start/lines 精读目标段落)\n"));
        }
    }
    truncate_out_max(&header, 12000)
}

/// 判断某行是否为结构定义，返回其分类标签。
pub(super) fn outline_kind(line: &str, ext: &str) -> Option<&'static str> {
    // ArkTS/TS/JS：装饰器、类/接口/枚举、函数/方法、组件
    let is_ts = matches!(ext, "ets" | "ts" | "tsx" | "js" | "jsx");
    if is_ts {
        if line.starts_with('@')
            && (line.starts_with("@Entry")
                || line.starts_with("@Component")
                || line.starts_with("@Builder")
                || line.starts_with("@CustomDialog")
                || line.starts_with("@State")
                || line.starts_with("@Prop")
                || line.starts_with("@Link")
                || line.starts_with("@Provide")
                || line.starts_with("@Consume")
                || line.starts_with("@Watch")
                || line.starts_with("@StorageLink")
                || line.starts_with("@Router"))
        {
            return Some("装饰器");
        }
        if starts_with_any(line, &["export ", "declare "]) {
            let body = line
                .trim_start_matches("export ")
                .trim_start_matches("default ")
                .trim_start_matches("declare ");
            return outline_kind(body.trim(), ext);
        }
        if starts_with_word(line, "class") || starts_with_word(line, "interface")
            || starts_with_word(line, "enum") || starts_with_word(line, "type")
            || starts_with_word(line, "abstract class")
        {
            return Some("类型");
        }
        if starts_with_word(line, "function") || starts_with_word(line, "async function") {
            return Some("函数");
        }
        // 方法/箭头函数：name(...) {  或  name = (...) =>
        if (line.contains('(') && line.ends_with('{') && looks_like_method(line))
            || (line.contains("=>") && looks_like_arrow(line))
        {
            return Some("方法");
        }
        if starts_with_word(line, "struct") && ext == "ets" {
            return Some("组件");
        }
        if line.starts_with("build(") || line == "build() {" {
            return Some("构建");
        }
        return None;
    }

    // Rust
    if ext == "rs" {
        if starts_with_word(line, "fn") || starts_with_word(line, "async fn")
            || starts_with_word(line, "pub fn") || starts_with_word(line, "pub async fn")
        {
            return Some("函数");
        }
        if starts_with_any(line, &["struct ", "enum ", "trait ", "type ", "mod ", "impl ", "pub struct ", "pub enum ", "pub trait ", "pub mod "]) {
            return Some("类型");
        }
        if line.starts_with("#[") {
            return Some("属性");
        }
        return None;
    }

    // Python
    if ext == "py" {
        if starts_with_word(line, "def ") || starts_with_word(line, "async def ") {
            return Some("函数");
        }
        if starts_with_word(line, "class ") {
            return Some("类型");
        }
        return None;
    }

    // C/C++/Java/Kotlin/Swift
    if matches!(ext, "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "java" | "kt" | "swift") {
        if starts_with_word(line, "class ") || starts_with_word(line, "interface ")
            || starts_with_word(line, "struct ") || starts_with_word(line, "enum ")
        {
            return Some("类型");
        }
        if line.contains('(') && (line.ends_with('{') || line.ends_with(';')) && looks_like_method(line) {
            return Some("函数");
        }
        if line.starts_with("@IBAction") || line.starts_with("@IBOutlet") || line.starts_with("@Override") {
            return Some("注解");
        }
        return None;
    }

    // Go
    if ext == "go" {
        if starts_with_word(line, "func ") {
            return Some("函数");
        }
        if starts_with_word(line, "type ") {
            return Some("类型");
        }
        return None;
    }

    None
}

pub(super) fn starts_with_word(s: &str, word: &str) -> bool {
    if !s.starts_with(word) {
        return false;
    }
    s.as_bytes().get(word.len()).map_or(true, |b| !b.is_ascii_alphanumeric() && *b != b'_')
}

pub(super) fn starts_with_any(s: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| s.starts_with(p))
}

/// 粗略判断是否为方法定义（含括号、以 { 结尾，且像 `name(...)` 开头），排除控制流语句。
pub(super) fn looks_like_method(line: &str) -> bool {
    let first = line.split('(').next().unwrap_or("").trim();
    if first.is_empty() {
        return false;
    }
    let name = first.split_whitespace().last().unwrap_or("");
    if name.is_empty() {
        return false;
    }
    // 排除 if/for/while/switch/catch 等控制流
    if matches!(name, "if" | "for" | "while" | "switch" | "catch" | "when" | "return" | "else") {
        return false;
    }
    name.chars().next().map_or(false, |c| c.is_ascii_alphabetic() || c == '_' || c == '$')
}

pub(super) fn looks_like_arrow(line: &str) -> bool {
    let first = line.split('=').next().unwrap_or("").trim();
    if first.is_empty() {
        return false;
    }
    let name = first.split_whitespace().last().unwrap_or("");
    !name.is_empty()
        && name
            .chars()
            .next()
            .map_or(false, |c| c.is_ascii_alphabetic() || c == '_' || c == '$')
}

// ---------- 语言感知的成对代码块 ----------
// 原则：文件操作（读取截取/编辑/查找/删除）绝不把代码块截断在中间。
// 依据扩展名识别语言结构——花括号语言按 `{}` `()` `[]` 成对匹配（字符串/注释感知），
// Python 按 def/class 缩进块——从而整块操作：一个方法行数再多，也给出完整方法，
// 避免固定行数漏掉块结束符导致后续编辑基于残缺片段。

/// 代码块结构风格（语言感知）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum BlockStyle {
    /// 花括号语言：`{}` 成对（`()`/`[]` 作为同组括号参与配对）
    Brace,
    /// 缩进语言（Python）：块由 `def/class ...:` + 缩进界定
    Indent,
    /// 无结构化块（文本/数据/样式等）：不做块级操作
    None,
}

pub(super) fn block_style(ext: &str) -> BlockStyle {
    match ext.to_lowercase().as_str() {
        "py" | "pyw" => BlockStyle::Indent,
        // 花括号语言（含 ArkTS/TS/JS/Rust/C 系/Go/Kotlin/Swift/C#/Dart/PHP/Shell/Vue 等）
        "rs" | "c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "java" | "kt" | "kts"
        | "swift" | "cs" | "dart" | "scala" | "groovy" | "go" | "php" | "ts" | "tsx"
        | "js" | "jsx" | "mjs" | "cjs" | "ets" | "vue" | "svelte" | "sh" | "bash"
        | "zsh" | "fish" => BlockStyle::Brace,
        _ => BlockStyle::None,
    }
}

/// 跨行扫描状态：块注释（/* */）与多行字符串（` 模板串等）跨行持续
#[derive(Default)]
pub(super) struct LineScanner {
    in_block_comment: bool,
    in_str: Option<char>,
}

impl LineScanner {
    /// 逐字符扫描一行，把「有意义的括号字符」按出现顺序交给 cb；
    /// 字符串（" ' `）、转义、行注释（//，Python/Shell 的 #）、块注释（/* */）内的字符全部跳过。
    pub(super) fn scan(&mut self, line: &str, ext: &str, mut cb: impl FnMut(char)) {
        let hash_comment = matches!(ext, "py" | "pyw" | "sh" | "bash" | "zsh" | "fish");
        let mut chars = line.char_indices().peekable();
        while let Some((_, c)) = chars.next() {
            if let Some(q) = self.in_str {
                if c == '\\' {
                    chars.next(); // 转义：跳过下一字符
                    continue;
                }
                if c == q {
                    self.in_str = None;
                }
                continue;
            }
            if self.in_block_comment {
                if c == '*' && chars.peek().map(|(_, n)| *n) == Some('/') {
                    chars.next();
                    self.in_block_comment = false;
                }
                continue;
            }
            match c {
                '"' | '\'' | '`' => self.in_str = Some(c),
                '/' if chars.peek().map(|(_, n)| *n) == Some('/') => break, // 行注释
                '/' if chars.peek().map(|(_, n)| *n) == Some('*') => {
                    chars.next();
                    self.in_block_comment = true;
                }
                '#' if hash_comment => break,
                '{' | '}' | '(' | ')' | '[' | ']' => cb(c),
                _ => {}
            }
        }
    }
}

/// 行缩进宽度（空格+制表）
fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

/// 判断一行是否为「块根」：方法/函数/类型等定义行（非控制流）。
/// 用于在嵌套块中选「要完整操作的那个方法」，避免只补到 if/for 内部。
pub(super) fn is_block_root(line: &str, ext: &str) -> bool {
    let t = line.trim();
    if outline_kind(t, ext).is_some() {
        return true;
    }
    // 兜底：语言无关的常见定义关键字（outline_kind 未覆盖的语言/写法）
    starts_with_word(t, "fn")
        || starts_with_word(t, "function")
        || starts_with_word(t, "def")
        || starts_with_word(t, "class")
        || starts_with_word(t, "struct")
        || starts_with_word(t, "interface")
        || starts_with_word(t, "enum")
        || starts_with_word(t, "trait")
        || starts_with_word(t, "impl")
        || starts_with_word(t, "type")
        || starts_with_word(t, "func")
        || starts_with_word(t, "pub fn")
        || starts_with_word(t, "export default")
        || looks_like_method(t)
        || looks_like_arrow(t)
}

/// 自上而下收集「包含 idx 行」的所有未配对开块行（外层在前、内层在后）。
/// 空列表 = idx 不在任何块内（文本/数据区域或块间）。
pub(super) fn enclosing_opens(lines: &[&str], idx: usize, ext: &str) -> Vec<usize> {
    let mut stack: Vec<usize> = Vec::new();
    let mut sc = LineScanner::default();
    for (i, line) in lines.iter().enumerate().take(idx + 1) {
        sc.scan(line, ext, |c| match c {
            '{' | '(' | '[' => stack.push(i),
            _ => {
                stack.pop();
            }
        });
    }
    stack
}

/// 从 open_idx 行向下扫描，找到与开块括号配对闭合的那一行（0 起，含）。
/// 显式栈配对：每次 `})]` 弹出最近未闭合的开括号；目标 = 开行最后一个未闭合开括号，
/// 被弹出即找到配对行——比扁平深度计数更稳：`} else {`、单行内嵌块不会误判，
/// 行首闭合符（如 `} else {` 的 `}`）也不会干扰。找不到返回 None。
pub(super) fn find_matching_close(lines: &[&str], open_idx: usize, ext: &str) -> Option<usize> {
    let mut stack: Vec<usize> = Vec::new();
    let mut target: Option<usize> = None;
    let mut sc = LineScanner::default();
    for (i, line) in lines.iter().enumerate().skip(open_idx) {
        let mut closed_target = false;
        sc.scan(line, ext, |c| match c {
            '{' | '(' | '[' => stack.push(i),
            '}' | ')' | ']' => {
                if let Some(popped) = stack.pop() {
                    if target == Some(popped) {
                        closed_target = true;
                    }
                }
            }
            _ => {}
        });
        if target.is_none() {
            // 开行处理完毕：目标 = 该行最后未闭合的开括号（若无 → 该行未真正开块）
            target = stack.last().copied();
            if target.is_none() {
                return None;
            }
        } else if closed_target {
            return Some(i);
        }
    }
    None
}

/// Python 缩进块：向上找最近的 def/class 定义行，向下到缩进归位前的最后一行。
fn find_indent_block(lines: &[&str], idx: usize) -> Option<(usize, usize)> {
    if lines.is_empty() {
        return None;
    }
    let mut open = idx.min(lines.len() - 1);
    loop {
        let t = lines[open].trim();
        if t.is_empty() || t.starts_with('#') {
            if open == 0 {
                return None;
            }
            open -= 1;
            continue;
        }
        if t.starts_with("def ") || t.starts_with("async def ") || t.starts_with("class ") {
            break;
        }
        if open == 0 {
            return None;
        }
        open -= 1;
    }
    let base_indent = indent_of(lines[open]);
    // 向下找缩进回到 base（或更浅）的第一行 → 其上一行是块尾
    let mut close = open + 1;
    let mut last_body = open;
    while close < lines.len() {
        let t = lines[close].trim();
        if t.is_empty() || t.starts_with('#') {
            close += 1;
            continue;
        }
        if indent_of(lines[close]) <= base_indent {
            break;
        }
        last_body = close;
        close += 1;
    }
    Some((open, last_body))
}

/// 找到 idx 所在的方法/类型级根块（最内层根块；Python 为 def/class 块）。
pub(super) fn find_root_block(lines: &[&str], idx: usize, ext: &str) -> Option<(usize, usize)> {
    if idx >= lines.len() {
        return None;
    }
    match block_style(ext) {
        BlockStyle::Indent => return find_indent_block(lines, idx),
        BlockStyle::None => return None,
        BlockStyle::Brace => {}
    }
    let opens = enclosing_opens(lines, idx, ext);
    let root = opens.iter().rev().find(|&&o| is_block_root(lines[o], ext)).copied()?;
    let close = find_matching_close(lines, root, ext)?;
    Some((root, close))
}

/// 找到 idx 所在的完整块：最内层根块优先；无根块则最外层块（括号组）。
/// 保证返回的块是「完整的」——成对结束符一定包含在内。
pub(super) fn find_enclosing_block(lines: &[&str], idx: usize, ext: &str) -> Option<(usize, usize)> {
    if idx >= lines.len() {
        return None;
    }
    match block_style(ext) {
        BlockStyle::Indent => return find_indent_block(lines, idx),
        BlockStyle::None => return None,
        BlockStyle::Brace => {}
    }
    let opens = enclosing_opens(lines, idx, ext);
    if opens.is_empty() {
        return None;
    }
    let open = opens
        .iter()
        .rev()
        .find(|&&o| is_block_root(lines[o], ext))
        .copied()
        .unwrap_or(opens[0]);
    let close = find_matching_close(lines, open, ext)?;
    Some((open, close))
}

/// 以 anchor 为锚点的块查找：在 idx 的 enclosing 候选里，选「开行 ≤ anchor」的最内层块
/// （即与窗口起点同一层级的那块），保证窗口终点不跳进内层块而漏掉外层方法尾部。
pub(super) fn find_block_anchored(
    lines: &[&str],
    idx: usize,
    anchor: usize,
    ext: &str,
) -> Option<(usize, usize)> {
    if idx >= lines.len() {
        return None;
    }
    match block_style(ext) {
        BlockStyle::Indent => return find_indent_block(lines, idx),
        BlockStyle::None => return None,
        BlockStyle::Brace => {}
    }
    let opens = enclosing_opens(lines, idx, ext);
    if opens.is_empty() {
        return None;
    }
    let chosen = opens
        .iter()
        .rev()
        .find(|&&o| o <= anchor)
        .copied()
        .unwrap_or(opens[0]);
    let close = find_matching_close(lines, chosen, ext)?;
    Some((chosen, close))
}


/// find_files：按文件名 glob 搜索（最多 100 条）
pub(super) async fn find_files(args: &Value, roots: &[String]) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法搜索文件".into());
    }
    let pattern = args["pattern"].as_str().unwrap_or("").trim();
    if pattern.is_empty() {
        return Err("find_files 需要参数 {\"pattern\":\"<glob 模式，如 *.ets 或 **/*.json>\"}".into());
    }
    let raw = args["path"].as_str().unwrap_or(".");
    let root = resolve_in_roots(roots, raw)?;
    if !root.is_dir() {
        return Err(format!("路径不是目录: {}", root.display()));
    }
    let (ignore_rules, start_rel) = load_project_ignore(&root, roots);
    let mut hits: Vec<PathBuf> = Vec::new();
    let mut skipped = 0u32;
    fn walk(
        dir: &Path,
        root: &Path,
        rel: &str,
        rules: &[IgnoreRule],
        pattern: &str,
        hits: &mut Vec<PathBuf>,
        skipped: &mut u32,
    ) {
        // 子目录 .gitignore（与 list_dir 同口径）
        let child_rules = load_child_rules(rules, dir, rel);
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                let entry_rel = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                if should_skip_dir(&name)
                    || gitignore_ignored(&child_rules, &name, &entry_rel, true)
                {
                    *skipped += 1;
                    continue;
                }
                walk(&p, root, &entry_rel, &child_rules, pattern, hits, skipped);
                if hits.len() >= 100 {
                    return;
                }
            } else {
                // pattern 可匹配文件名（*.ets）或相对路径（src/**/*.ets）
                let rel = p
                    .strip_prefix(root)
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|_| name.clone());
                if glob_match(pattern, &name) || glob_match(pattern, &rel) {
                    hits.push(p);
                    if hits.len() >= 100 {
                        return;
                    }
                }
            }
        }
    }
    walk(&root, &root, &start_rel, &ignore_rules, pattern, &mut hits, &mut skipped);
    // 排序：按路径字典序，同级目录聚拢、结果可预测（read_dir 顺序是随机的）
    hits.sort();
    if hits.is_empty() {
        return Ok(format!(
            "未找到匹配 {pattern} 的文件（已跳过 {skipped} 个忽略目录）"
        ));
    }
    let mut out = format!("找到 {} 个文件（已跳过 {skipped} 个忽略目录）：\n", hits.len());
    for p in &hits {
        out.push_str(&format!("{}\n", p.display()));
    }
    Ok(truncate_out(&out))
}

/// grep_files：按文本内容搜索（缺省不区分大小写，可指定 case_sensitive；跳过忽略目录、
/// 二进制与超大文件，最多 50 条）
pub(super) async fn grep_files(args: &Value, roots: &[String]) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法搜索内容".into());
    }
    let pattern = args["pattern"].as_str().unwrap_or("").trim();
    if pattern.is_empty() {
        return Err("grep_files 需要参数 {\"pattern\":\"<搜索关键词>\"}".into());
    }
    let raw = args["path"].as_str().unwrap_or(".");
    let root = resolve_in_roots(roots, raw)?;
    let glob = args["glob"].as_str().unwrap_or("").trim();
    let case_sensitive = args["case_sensitive"].as_bool().unwrap_or(false);
    // block=true：命中时给出所在「完整代码块」（方法/函数整体，语言感知成对匹配），
    // 便于直接了解/编辑整个方法而不只看单行；最多展开前 5 条防输出爆炸
    let block_mode = args["block"].as_bool().unwrap_or(false);
    const MAX_BLOCK_HITS: usize = 5;
    let (ignore_rules, start_rel) = load_project_ignore(&root, roots);
    let lower = pattern.to_lowercase();
    let mut hits: Vec<String> = Vec::new();
    let mut files_checked = 0u32;
    let mut skipped = 0u32;
    let mut block_shown = 0usize;
    // 单文件搜索大小上限：超大文本文件跳过（防读入内存爆炸）
    const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
    fn walk(
        dir: &Path,
        rel: &str,
        rules: &[IgnoreRule],
        pattern: &str,
        lower: &str,
        case_sensitive: bool,
        glob: &str,
        block_mode: bool,
        block_shown: &mut usize,
        hits: &mut Vec<String>,
        files_checked: &mut u32,
        skipped: &mut u32,
    ) {
        // 子目录 .gitignore（与 list_dir 同口径）：规则只对其子树生效
        let child_rules = load_child_rules(rules, dir, rel);
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                let entry_rel = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                if should_skip_dir(&name)
                    || gitignore_ignored(&child_rules, &name, &entry_rel, true)
                {
                    *skipped += 1;
                    continue;
                }
                walk(
                    &p,
                    &entry_rel,
                    &child_rules,
                    pattern,
                    lower,
                    case_sensitive,
                    glob,
                    block_mode,
                    block_shown,
                    hits,
                    files_checked,
                    skipped,
                );
                if hits.len() >= 50 {
                    return;
                }
            } else {
                if !glob.is_empty() && !glob_match(glob, &name) {
                    continue;
                }
                let Ok(meta) = std::fs::metadata(&p) else { continue };
                if meta.len() > MAX_FILE_BYTES {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&p) else { continue };
                if bytes[..bytes.len().min(4096)].contains(&0) {
                    continue;
                }
                *files_checked += 1;
                let text = smart_decode(&bytes);
                let flines: Vec<&str> = text.lines().collect();
                let ext = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                // 同一文件内已展开过的块区间（同一方法多条命中不重复整块展开）
                let mut seen_blocks: Vec<(usize, usize)> = Vec::new();
                for (i, line) in flines.iter().enumerate() {
                    let matched = if case_sensitive {
                        line.contains(pattern)
                    } else {
                        line.to_lowercase().contains(lower)
                    };
                    if matched {
                        // block 模式：展开所在完整代码块（成对 {}() 语言感知，整方法不截断）
                        if block_mode && *block_shown < MAX_BLOCK_HITS {
                            if let Some((o, c)) = find_enclosing_block(&flines, i, &ext) {
                                if seen_blocks.contains(&(o, c)) {
                                    continue;
                                }
                                seen_blocks.push((o, c));
                                *block_shown += 1;
                                let mut b = String::new();
                                b.push_str(&format!("{}（完整代码块 L{}-L{}）\n", p.display(), o + 1, c + 1));
                                for (li, l) in flines[o..=c].iter().enumerate() {
                                    let s: String = l.trim_end().chars().take(200).collect();
                                    b.push_str(&format!("{:>5} │ {s}\n", o + li + 1));
                                }
                                b.push_str(&format!("…块结束 L{}\n", c + 1));
                                hits.push(b);
                                if hits.len() >= 50 {
                                    return;
                                }
                                continue;
                            }
                        }
                        let snippet: String = line.trim().chars().take(160).collect();
                        // 注释命中标注：模型据此区分代码逻辑位置与说明性位置，避免误读
                        let tag = if is_comment_line(line, &ext) { "[注释] " } else { "" };
                        hits.push(format!("{}:{}: {tag}{snippet}", p.display(), i + 1));
                        if hits.len() >= 50 {
                            return;
                        }
                    }
                }
            }
        }
    }
    walk(
        &root,
        &start_rel,
        &ignore_rules,
        pattern,
        &lower,
        case_sensitive,
        glob,
        block_mode,
        &mut block_shown,
        &mut hits,
        &mut files_checked,
        &mut skipped,
    );
    if hits.is_empty() {
        return Ok(format!(
            "未找到包含「{pattern}」的内容（检查了 {files_checked} 个文件，跳过 {skipped} 个忽略目录）"
        ));
    }
    let mut out = format!(
        "找到 {} 条命中（检查 {files_checked} 个文件，跳过 {skipped} 个忽略目录）：\n",
        hits.len()
    );
    if block_mode {
        out.push_str(&format!(
            "（block=true：前 {} 条命中已展开所在完整代码块，其余仅显示单行）\n",
            block_shown.min(MAX_BLOCK_HITS)
        ));
    }
    for h in &hits {
        out.push_str(&format!("{h}\n"));
    }
    // block 模式需要容纳整块代码，输出上限放大；普通模式沿用 3000 字符
    Ok(truncate_out_max(&out, if block_mode { 20000 } else { 3000 }))
}

// ---------- 写文件 / 编辑文件 ----------

/// 写入/覆盖文本文件（UTF-8，单次 ≤1MB，自动创建父目录）
pub(super) async fn write_file(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法写入文件".into());
    }
    // Request/Spec 分离：宽松参数 WriteFileRequest → 显式 resolve() 产出严格规范 WriteFileSpec
    let spec = WriteFileRequest::from_args(args)?.resolve(roots)?;
    let p = &spec.path;
    let content = spec.content.as_str();
    let existed = p.exists();
    let mut content_out = content.to_string();
    if existed {
        // 冲突保护：文件自上次读取后被外部修改（IDE/用户/其他会话）→ 拒绝覆盖，要求重读确认
        if let Ok(bytes) = std::fs::read(p) {
            if has_external_change(p, &bytes) {
                return Err(format!(
                    "写入冲突：文件 {} 自上次读取后被修改（可能被外部编辑器/IDE、其他会话或命令间接改动）。\n请先 read_file 查看最新内容、确认意图后再写入（重新读取会解除冲突保护）。",
                    p.display()
                ));
            }
            // 换行风格保持：原文件 CRLF（Windows 项目常见）且新内容纯 LF → 统一转 CRLF，
            // 避免覆盖后整个文件 diff 全部变化
            if bytes.windows(2).any(|w| w == b"\r\n") && !content.contains('\r') {
                content_out = content.replace('\n', "\r\n");
            }
            // BOM 保留：原文件带 UTF-8 BOM（EF BB BF）时新内容也前置 BOM（与 edit_file 同口径），
            // 避免覆盖后 .bat/带 BOM 文本文件的首行字节变化导致乱码
            if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) && !content_out.starts_with('\u{feff}') {
                content_out.insert(0, '\u{feff}');
            }
            // 撤销快照：覆盖前记录旧内容（必须记旧字节，记新内容会导致 undo 失效）
            crate::agent::undo::snapshot(conversation_id, p, &bytes);
        }
    }
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录失败 {}: {e}", parent.display()))?;
    }
    std::fs::write(p, content_out.as_bytes()).map_err(|e| format!("写入文件失败: {e}"))?;
    if let Ok(meta) = std::fs::metadata(p) {
        stamp_put(p, &meta, content_out.as_bytes());
    }
    Ok(format!(
        "已{}文件 {}（{} 字节）",
        if existed { "覆盖" } else { "创建" },
        p.display(),
        content.len()
    ))
}

/// 编辑文件：精确文本替换（old → new，可选全部替换），返回替换处数
pub(super) fn apply_edit(text: &str, old: &str, new: &str, replace_all: bool) -> Result<(String, usize), String> {
    // CRLF/LF 兼容匹配：Windows 项目文件常见 CRLF，而 read_file 展示时行尾 \r 被剥离，
    // 模型拼的 old 用 \n（或单行无换行），直接匹配 CRLF 文件必然失败。
    // 策略：原样匹配失败时，按文件实际换行风格归一 old/new 重试；写回保持文件原风格。
    let mut eff_old = old.to_string();
    let mut eff_new = new.to_string();
    let mut occurrences = text.matches(&eff_old).count();
    if occurrences == 0 {
        // 文件 CRLF、old 用 LF：把 old/new 的 \n 换成 \r\n 重试
        if text.contains("\r\n") && !old.contains('\r') && old.contains('\n') {
            let c_old = old.replace('\n', "\r\n");
            let n = text.matches(&c_old).count();
            if n > 0 {
                eff_old = c_old;
                eff_new = new.replace('\n', "\r\n");
                occurrences = n;
            }
        }
        // 反向：文件 LF、old 带 CRLF（罕见，模型直接拼了原文含 \r 的情况）
        if occurrences == 0 && !text.contains('\r') && old.contains("\r\n") {
            let l_old = old.replace("\r\n", "\n");
            let n = text.matches(&l_old).count();
            if n > 0 {
                eff_old = l_old;
                eff_new = new.replace("\r\n", "\n");
                occurrences = n;
            }
        }
    }
    if occurrences == 0 {
        return Err(format!("old 内容在文件中未找到（{old:?}），请先 read_file 确认原文（注意缩进/引号/空白）"));
    }
    let count = if replace_all { occurrences } else { 1 };
    let replaced = if replace_all {
        text.replace(&eff_old, &eff_new)
    } else {
        text.replacen(&eff_old, &eff_new, 1)
    };
    Ok((replaced, count))
}

/// edit_file：精确文本替换修改文件（≤1MB）
pub(super) async fn edit_file(args: &Value, roots: &[String], conversation_id: &str) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法编辑文件".into());
    }
    // Request/Spec 分离：宽松参数 EditFileRequest → 显式 resolve() 产出严格规范 EditFileSpec
    let spec = EditFileRequest::from_args(args)?.resolve(roots)?;
    let p = &spec.path;
    let old = spec.old.as_str();
    let new = spec.new.as_str();
    let replace_all = spec.replace_all;
    let bytes = std::fs::read(p).map_err(|e| format!("读取文件失败: {e}"))?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err("文件是二进制，无法以文本方式编辑".into());
    }
    // 冲突保护：文件自上次读取后被外部修改 → 提前拦截（比 old 匹配失败的报错更明确）
    if has_external_change(p, &bytes) {
        return Err(format!(
            "编辑冲突：文件 {} 自上次读取后被修改（可能被外部编辑器/IDE、其他会话或命令间接改动）。\n请先 read_file 查看最新内容后重新编辑（重新读取会解除冲突保护）。",
            p.display()
        ));
    }
    // 严格 UTF-8 校验：GBK 文件若用 from_utf8_lossy 读入再写回，中文会被替换字符永久破坏
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| {
            format!(
                "文件 {} 非 UTF-8 编码（可能是 GBK/GB2312），为避免中文被写坏已拒绝编辑。请先用 iconv 等工具转换为 UTF-8 后再编辑",
                p.display()
            )
        })?
        .to_string();
    // BOM 处理：剥离后匹配（模型看不到 BOM 字符，不会在 old 里携带），写回时保留，
    // 避免「首行内容永远匹配不上」
    let (has_bom, body) = match text.strip_prefix('\u{feff}') {
        Some(b) => (true, b),
        None => (false, text.as_str()),
    };

    // ---- start 模式：按语言感知的「完整代码块」整体替换/删除 ----
    // 不固定行数：块有多长就操作多长（{}() 成对匹配，Python 按缩进），
    // 从块首行到结束符一整套替换/删除，杜绝漏掉块结束符。
    if let Some(start_line) = spec.start {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let body_lines: Vec<&str> = body.split('\n').collect();
        let total = body_lines.len();
        if total == 0 {
            return Err("文件为空，没有可替换的代码块".into());
        }
        let idx = ((start_line as usize).saturating_sub(1)).min(total - 1);
        let (o, c) = find_enclosing_block(&body_lines, idx, &ext).ok_or_else(|| {
            format!(
                "无法识别第 {start_line} 行所在的代码块：未找到成对的 {{}}/()（该行可能在字符串/注释中，或文件不是结构化代码；请改用 old 参数精确替换）"
            )
        })?;
        // 按字节边界切出完整块（split('\n') 保留 CRLF 的 \r，替换后保持原换行风格）
        let mut line_starts: Vec<usize> = Vec::with_capacity(body_lines.len());
        let mut off = 0usize;
        for l in body.split_inclusive('\n') {
            line_starts.push(off);
            off += l.len();
        }
        let start_off = line_starts[o];
        let end_off = if c + 1 < line_starts.len() { line_starts[c + 1] } else { body.len() };
        let block_lines = c - o + 1;
        let final_body = format!("{}{}{}", &body[..start_off], spec.new, &body[end_off..]);
        let final_text = if has_bom { format!("\u{feff}{final_body}") } else { final_body };
        // 撤销快照：落盘前记录旧内容（会话级，undo_edit 工具按栈序恢复）
        crate::agent::undo::snapshot(conversation_id, p, &bytes);
        std::fs::write(p, final_text.as_bytes()).map_err(|e| format!("写入文件失败: {e}"))?;
        if let Ok(meta) = std::fs::metadata(p) {
            stamp_put(p, &meta, final_text.as_bytes());
        }
        let show = |s: &str| -> String {
            let n = s.chars().count();
            if n > 200 {
                format!("{}…（共 {n} 字符）", s.chars().take(200).collect::<String>())
            } else {
                s.to_string()
            }
        };
        let mut report = format!(
            "已{}完整代码块（第 {}–{} 行，共 {} 行）\n",
            if spec.new.is_empty() { "删除" } else { "替换" },
            o + 1,
            c + 1,
            block_lines
        );
        report.push_str(&format!("块首行：{}\n", show(body_lines[o].trim())));
        if !spec.new.is_empty() {
            report.push_str(&format!("新内容：{}\n", show(&spec.new)));
        }
        report.push_str(&format!(
            "（语言感知成对匹配：块结束符已包含在操作范围内）\n文件：{}",
            p.display()
        ));
        return Ok(report);
    }

    let (replaced, count) = apply_edit(body, old, new, replace_all)
        .map_err(|e| with_advice("edit_file", e))?;
    let final_text = if has_bom { format!("\u{feff}{replaced}") } else { replaced };
    // 撤销快照：落盘前记录旧内容（会话级，undo_edit 工具按栈序恢复）
    crate::agent::undo::snapshot(conversation_id, p, &bytes);
    std::fs::write(p, final_text.as_bytes()).map_err(|e| format!("写入文件失败: {e}"))?;
    if let Ok(meta) = std::fs::metadata(p) {
        stamp_put(p, &meta, final_text.as_bytes());
    }
    // 原文/新文各截 200 字符展示，避免大段替换把结果撑爆上下文
    let show = |s: &str| -> String {
        let n = s.chars().count();
        if n > 200 {
            format!("{}…（共 {n} 字符）", s.chars().take(200).collect::<String>())
        } else {
            s.to_string()
        }
    };
    Ok(format!(
        "已替换 {} 处（{}）\n原文：{}\n新文：{}\n文件：{}",
        count,
        if replace_all { "全部替换" } else { "仅第一处" },
        show(old),
        show(new),
        p.display()
    ))
}

// ---------- 命令执行工具 ----------

/// 敏感文件保护（write_file/edit_file 代码级拦截）：命中返回拒绝原因。
/// 环境约束 > Prompt 约束：密钥类文件一律拒绝；已执行的迁移 SQL 不可修改（新建文件允许）。
pub(super) fn is_protected_file(p: &std::path::Path) -> Option<&'static str> {
    let name = p.file_name()?.to_string_lossy().to_lowercase();
    if name.starts_with(".env") {
        return Some("环境变量文件（.env*）禁止写入：密钥类配置不应由 Agent 修改，请手动编辑");
    }
    if name.ends_with(".key")
        || name.ends_with(".pem")
        || name.ends_with(".pfx")
        || name.ends_with(".p12")
        || name.ends_with(".keystore")
        || name.ends_with(".cer")
        || name.ends_with(".p7b")
        || name.ends_with(".jks")
    {
        return Some("密钥/证书文件禁止写入（*.key/*.pem/*.pfx/*.p12/*.keystore/*.cer/*.p7b/*.jks，含鸿蒙签名材料）");
    }
    // 已执行的数据库迁移 SQL 不可修改（须新建递增编号文件）；新建文件允许。
    // 按父目录组件名判断（避免项目路径本身含 migrations 字样时误伤全部 .sql 文件）
    if p.exists() && name.ends_with(".sql") {
        let parent_name = p
            .parent()
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if parent_name == "migrations" || parent_name == "migration" {
            return Some("已执行的数据库迁移 SQL 不可修改：请新建递增编号的迁移文件（如 014_xxx.sql）");
        }
    }
    None
}

pub(super) async fn multi_edit(
    args: &Value,
    roots: &[String],
    conversation_id: &str,
) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法编辑文件".into());
    }
    let edits = args["edits"].as_array().ok_or(
        "multi_edit 需要参数 {\"edits\":[{\"path\":\"<文件>\",\"old\":\"<原文>\",\"new\":\"<新文>\",\"replace_all\":<可选布尔>}]}",
    )?;
    if edits.is_empty() {
        return Err("edits 数组不能为空".into());
    }
    if edits.len() > 10 {
        return Err("单次最多批量编辑 10 个文件，请拆分为多次调用".into());
    }
    let mut report: Vec<String> = Vec::new();
    let mut ok_count = 0usize;
    for (i, e) in edits.iter().enumerate() {
        let raw = match e.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                report.push(format!("{}. ❌ 缺少 path 参数，跳过", i + 1));
                continue;
            }
        };
        let old = e.get("old").and_then(|v| v.as_str()).unwrap_or("");
        let new = e.get("new").and_then(|v| v.as_str()).unwrap_or("");
        let replace_all = e.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
        match apply_single_edit(raw, old, new, replace_all, roots, conversation_id) {
            Ok(msg) => {
                ok_count += 1;
                report.push(format!("{}. ✅ {msg}", i + 1));
            }
            Err(err) => report.push(format!("{}. ❌ {err}", i + 1)),
        }
    }
    Ok(format!(
        "批量编辑完成：{ok_count}/{} 项成功\n\n{}",
        edits.len(),
        report.join("\n")
    ))
}

/// 单文件替换核心（multi_edit 复用；校验/冲突保护/撤销快照/落盘与 edit_file 同口径）
pub(super) fn apply_single_edit(
    raw: &str,
    old: &str,
    new: &str,
    replace_all: bool,
    roots: &[String],
    conversation_id: &str,
) -> Result<String, String> {
    if old.is_empty() {
        return Err(format!("{raw}: old 参数不能为空"));
    }
    let p = resolve_in_roots(roots, raw)?;
    if let Some(reason) = is_protected_file(&p) {
        return Err(format!("{raw}: 被安全策略拒绝：{reason}"));
    }
    if !p.is_file() {
        return Err(format!("{raw}: 路径不是文件"));
    }
    let meta = std::fs::metadata(&p).map_err(|e| e.to_string())?;
    if meta.len() > 1024 * 1024 {
        return Err(format!("{raw}: 超过 1MB，请用 run_command 处理"));
    }
    let bytes = std::fs::read(&p).map_err(|e| format!("读取失败: {e}"))?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err(format!("{raw}: 二进制文件无法文本编辑"));
    }
    if has_external_change(&p, &bytes) {
        return Err(format!(
            "{raw}: 自上次读取后被修改，请先 read_file 后重试（重新读取解除冲突保护）"
        ));
    }
    // 严格 UTF-8 校验：GBK 文件用 lossy 读入再写回会把中文永久写坏（与 edit_file 同口径拒绝）
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| format!("{raw}: 文件非 UTF-8 编码（可能是 GBK/GB2312），为避免中文被写坏已拒绝编辑，请先转换为 UTF-8"))?
        .to_string();
    // BOM 剥离匹配、写回保留（与 edit_file 同口径）
    let (has_bom, body) = match text.strip_prefix('\u{feff}') {
        Some(b) => (true, b),
        None => (false, text.as_str()),
    };
    let (replaced, count) = apply_edit(body, old, new, replace_all)?;
    let final_text = if has_bom { format!("\u{feff}{replaced}") } else { replaced };
    crate::agent::undo::snapshot(conversation_id, &p, &bytes);
    std::fs::write(&p, final_text.as_bytes()).map_err(|e| format!("写入失败: {e}"))?;
    if let Ok(meta) = std::fs::metadata(&p) {
        stamp_put(&p, &meta, final_text.as_bytes());
    }
    Ok(format!("{}（替换 {count} 处）", p.display()))
}

/// copy_file：复制项目内文件/目录（不覆盖目标，禁止受保护路径）
pub(super) async fn copy_file(args: &Value, roots: &[String]) -> Result<String, String> {
    if roots.is_empty() {
        return Err("当前会话未绑定项目目录，无法复制文件".into());
    }
    let from_raw = args["from"].as_str().ok_or("copy_file 需要参数 {\"from\":\"<源路径>\",\"to\":\"<目标路径>\"}")?.trim();
    let to_raw = args["to"].as_str().ok_or("copy_file 缺少 to 参数（目标路径）")?.trim();
    if from_raw.is_empty() || to_raw.is_empty() {
        return Err("from/to 参数不能为空".into());
    }
    let src = resolve_in_roots(roots, from_raw)?;
    // 目标允许不存在（复制到新路径），用 resolve_for_write 解析（防越界语义同 move_file）；
    // 目标已存在时由下方拒绝覆盖。
    let dst = resolve_for_write(roots, to_raw)?;
    if let Some(reason) = is_protected_file(&src) {
        return Err(format!("复制被安全策略拒绝：{reason}"));
    }
    if !src.exists() {
        return Err(format!("源路径不存在: {from_raw}"));
    }
    if dst.exists() {
        return Err(format!("目标已存在，拒绝覆盖: {}", dst.display()));
    }
    const PROTECTED: [&str; 9] = [
        ".git", "oh_modules", "node_modules", ".ohpm", ".deveco-agent",
        "build", ".hvigor", ".idea", ".arkui-x",
    ];
    let sname = src.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if PROTECTED.contains(&sname) && src.is_dir() {
        return Err(format!("受保护目录 {sname} 不允许复制"));
    }
    for comp in dst.ancestors().skip(1) {
        if let Some(n) = comp.file_name().and_then(|s| s.to_str()) {
            if PROTECTED.contains(&n) && comp.is_dir() {
                return Err(format!("目标落入受保护目录 {n}，拒绝复制"));
            }
        }
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目标父目录失败 {}: {e}", parent.display()))?;
    }
    if src.is_dir() {
        copy_dir_recursive(&src, &dst)?;
    } else {
        std::fs::copy(&src, &dst).map_err(|e| e.to_string())?;
    }
    Ok(format!("已复制 {} → {}", src.display(), dst.display()))
}

/// get_file_info：文件元信息（大小/修改时间/行数/编码/类型）
pub(super) async fn get_file_info(args: &Value, roots: &[String]) -> Result<String, String> {
    let raw = args["path"].as_str().ok_or("get_file_info 需要参数 {\"path\":\"<文件路径>\"}")?.trim();
    if raw.is_empty() {
        return Err("get_file_info 需要参数 {\"path\":\"<文件路径>\"}".into());
    }
    let p = if roots.is_empty() {
        PathBuf::from(raw)
    } else {
        resolve_in_roots(roots, raw)?
    };
    let meta = std::fs::metadata(&p).map_err(|e| format!("无法读取 {}: {e}", p.display()))?;
    let mut out = String::new();
    out.push_str(&format!("路径: {}\n", p.display()));
    out.push_str(&format!("类型: {}\n", if meta.is_dir() { "目录" } else if meta.is_file() { "文件" } else { "其他" }));
    out.push_str(&format!("大小: {} 字节", meta.len()));
    if meta.len() >= 1024 {
        out.push_str(&format!("（约 {:.1} KB）", meta.len() as f64 / 1024.0));
    }
    out.push('\n');
    if let Ok(mtime) = meta.modified() {
        if let Ok(dt) = mtime.duration_since(std::time::UNIX_EPOCH) {
            out.push_str(&format!("修改时间: {}\n", dt.as_secs()));
        }
    }
    // 行数 + 编码探测（单次读取）：严格 UTF-8 → GBK 验证 → 未知
    // （from_utf8_lossy 会把 GBK 误报为 UTF-8，必须严格校验）
    if let Ok(bytes) = std::fs::read(&p) {
        let bin = bytes.iter().take(2048).any(|b| *b == 0);
        let lines = if bin { 0 } else { smart_decode(&bytes).lines().count() };
        out.push_str(&format!("行数: {lines}\n"));
        let enc = if bin {
            "二进制（含 NUL 字节）"
        } else if std::str::from_utf8(&bytes).is_ok() {
            "UTF-8 文本"
        } else {
            let (_, _, had_err) = encoding_rs::GBK.decode(&bytes);
            if had_err {
                "非 UTF-8（未知编码，可能是 Shift-JIS 等）"
            } else {
                "GBK/GB2312（非 UTF-8，编辑前需转换）"
            }
        };
        out.push_str(&format!("编码: {enc}\n"));
        out.push_str(&format!("只读: {}\n", meta.permissions().readonly()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同步构造 tokio runtime 跑 async 工具（与 mod.rs 测试同款）
    fn block_on_rt<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Runtime::new().unwrap().block_on(f)
    }

    /// 在独立临时目录创建测试文件，返回 (文件路径, 会话根目录列表)
    fn tmp_file(tag: &str, content: &str, ext: &str) -> (std::path::PathBuf, Vec<String>) {
        let dir = std::env::temp_dir().join(format!(
            "agent_block_test_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(format!("t.{ext}"));
        std::fs::write(&f, content).unwrap();
        (f, vec![dir.to_string_lossy().to_string()])
    }

    // ---------- 成对代码块核心 ----------

    #[test]
    fn block_whole_method_detected() {
        let src = "fn a() {\n  let x = 1;\n  if x {\n    foo();\n  }\n  y();\n}\n\nfn b() {\n  z();\n}\n";
        let l: Vec<&str> = src.lines().collect();
        // 方法 a 内部（含 if 内部）→ 整方法 0-6
        assert_eq!(find_enclosing_block(&l, 3, "rs"), Some((0, 6)));
        assert_eq!(find_root_block(&l, 3, "rs"), Some((0, 6)));
        // 方法 b（头行与体内行一致）
        assert_eq!(find_enclosing_block(&l, 8, "rs"), Some((8, 10)));
        assert_eq!(find_enclosing_block(&l, 9, "rs"), Some((8, 10)));
        // 方法之间（空行）→ 无块
        assert_eq!(find_enclosing_block(&l, 7, "rs"), None);
    }

    #[test]
    fn block_ignores_strings_and_comments() {
        let src = "fn a() {\n  let s = \"{\";\n  let t = \"}\";\n  // }\n  let u = `x`;\n  foo();\n}\n";
        let l: Vec<&str> = src.lines().collect();
        // 字符串/注释里的括号不参与配对 → 结束符是第 6 行 }
        assert_eq!(find_enclosing_block(&l, 5, "ts"), Some((0, 6)));
    }

    #[test]
    fn block_else_falls_back_to_outermost() {
        let src = "if (a) {\n  foo();\n} else {\n  bar();\n}\n";
        let l: Vec<&str> = src.lines().collect();
        // 无方法级根 → 退回最外层块（else 块）
        assert_eq!(find_enclosing_block(&l, 3, "ts"), Some((2, 4)));
        // 行首 } 属前一块：if 块 0..=2
        assert_eq!(find_matching_close(&l, 0, "ts"), Some(2));
    }

    #[test]
    fn block_nested_arrow_prefers_innermost_root() {
        let src = "fn a() {\n  const inner = () => {\n    x();\n  };\n  y();\n}\n";
        let l: Vec<&str> = src.lines().collect();
        // 内层箭头内部 → 最内层根块是箭头（1..=3）
        assert_eq!(find_enclosing_block(&l, 2, "ts"), Some((1, 3)));
        // 外层方法体（箭头外）→ 整个方法 a（0..=5）
        assert_eq!(find_enclosing_block(&l, 4, "ts"), Some((0, 5)));
        // 锚定：窗口起点在方法 a 首行 → 末尾不跳进箭头，仍补齐整个方法
        assert_eq!(find_block_anchored(&l, 2, 0, "ts"), Some((0, 5)));
        assert_eq!(find_block_anchored(&l, 2, 1, "ts"), Some((1, 3)));
    }

    #[test]
    fn block_matching_close_skips_leading_close() {
        let src = "fn a() {\n  if x {\n    y();\n  } else {\n    z();\n  }\n}\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(find_matching_close(&l, 1, "ts"), Some(3)); // if 块 1..=3
        assert_eq!(find_matching_close(&l, 3, "ts"), Some(5)); // else 块 3..=5
    }

    #[test]
    fn block_python_indent() {
        let src = "def a():\n    x = 1\n    if x:\n        y()\n    return 2\n\ndef b():\n    z()\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(find_enclosing_block(&l, 3, "py"), Some((0, 4)));
        assert_eq!(find_enclosing_block(&l, 7, "py"), Some((6, 7)));
        assert_eq!(find_root_block(&l, 3, "py"), Some((0, 4)));
    }

    #[test]
    fn block_single_line_inner_blocks() {
        // 单行内嵌块闭合时不得误判外层方法已结束（栈配对核心回归）
        let src = "fn a() {\n  if (x) { foo(); }\n  bar();\n}\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(find_matching_close(&l, 0, "rs"), Some(3)); // fn a 结束在第 3 行 }
        assert_eq!(find_enclosing_block(&l, 2, "rs"), Some((0, 3))); // bar(); 仍属 fn a
        assert_eq!(find_enclosing_block(&l, 1, "rs"), Some((0, 3)));
    }

    #[test]
    fn block_else_if_chain_is_one_construct() {
        let src = "if (a) {\n  x();\n} else if (b) {\n  y();\n} else {\n  z();\n}\n";
        let l: Vec<&str> = src.lines().collect();
        // 顶层 if-else-if-else：每个分支是独立完整块（各自成对括号）
        assert_eq!(find_enclosing_block(&l, 1, "ts"), Some((0, 2))); // if 分支
        assert_eq!(find_enclosing_block(&l, 3, "ts"), Some((2, 4))); // else-if 分支
        assert_eq!(find_enclosing_block(&l, 5, "ts"), Some((4, 6))); // else 分支
        assert_eq!(find_matching_close(&l, 0, "ts"), Some(2));
        // 若链在方法内 → 任意分支都归到整个方法（不会只补到分支块）
        let src2 = "fn a() {\n  if (a) {\n    x();\n  } else if (b) {\n    y();\n  }\n  z();\n}\n";
        let l2: Vec<&str> = src2.lines().collect();
        assert_eq!(find_enclosing_block(&l2, 4, "ts"), Some((0, 7)));
    }

    #[test]
    fn block_non_code_file_returns_none() {
        let src = "title\n\nplain text { not code } more\n";
        let l: Vec<&str> = src.lines().collect();
        assert_eq!(block_style("md"), BlockStyle::None);
        assert_eq!(find_enclosing_block(&l, 0, "md"), None);
        assert_eq!(block_style("json"), BlockStyle::None, "数据文件不做块操作");
    }

    // ---------- read_file 块补齐 ----------

    #[test]
    fn read_file_completes_method_block() {
        // 读取窗口结束位置在方法内部 → 自动补齐到方法结束符（整方法）
        let content = "fn a() {\n  let x = 1;\n}\nfn b() {\n  let y = 2;\n  let z = 3;\n}\nfn c() {\n  let w = 4;\n}\n";
        let (f, roots) = tmp_file("read_block", content, "rs");
        let rel = f.to_string_lossy().to_string();
        // start=5（fn b 内部）lines=2 → 上扩到 fn b 首行、补齐到结束符
        let args = serde_json::json!({"path": rel, "start": 5, "lines": 2});
        let out = block_on_rt(read_file(&args, &roots)).expect("读取应成功");
        assert!(out.contains("fn b() {"), "应上扩到方法首行: {out}");
        assert!(out.contains("let y = 2") && out.contains("let z = 3"), "应包含完整方法体: {out}");
        assert!(!out.contains("let w = 4"), "不应越界到方法 c: {out}");
        assert!(out.contains("自动对齐"), "应有块对齐提示: {out}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn read_file_plain_text_keeps_fixed_window() {
        let content = "line1\nline2\nline3\nline4\n";
        let (f, roots) = tmp_file("read_plain", content, "txt");
        let rel = f.to_string_lossy().to_string();
        let args = serde_json::json!({"path": rel, "start": 2, "lines": 2});
        let out = block_on_rt(read_file(&args, &roots)).unwrap();
        assert!(!out.contains("代码块"), "非代码文件不应做块对齐: {out}");
        assert!(out.contains("line2") && out.contains("line3"), "{out}");
        assert!(!out.contains("line1"), "应严格按 lines=2: {out}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    // ---------- edit_file start 模式（整块替换/删除） ----------

    #[test]
    fn edit_block_mode_replaces_whole_method() {
        let content = "fn a() {\n  let x = 1;\n}\nfn b() {\n  let y = 2;\n  let z = 3;\n}\nfn c() {\n  let w = 4;\n}\n";
        let (f, roots) = tmp_file("edit_block", content, "rs");
        let rel = f.to_string_lossy().to_string();
        // 先读建立指纹基线（避免冲突保护拦截）
        block_on_rt(read_file(&serde_json::json!({"path": rel.clone()}), &roots)).unwrap();
        // start 定位方法 b 内部（第 5 行）→ 整块替换
        let args = serde_json::json!({"path": rel.clone(), "start": 5, "new": "fn b2() {\n  let q = 9;\n}\n"});
        let out = block_on_rt(edit_file(&args, &roots, "t_edit_block")).expect("块替换应成功");
        assert!(out.contains("完整代码块"), "{out}");
        assert!(out.contains("共 4 行"), "方法 b 应整体 4 行: {out}");
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains("fn b2()"), "应整块替换: {text}");
        assert!(!text.contains("let z = 3"), "旧方法体应消失: {text}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn edit_block_mode_deletes_whole_method() {
        let content = "fn a() {\n  let x = 1;\n}\nfn b() {\n  let y = 2;\n  let z = 3;\n}\nfn c() {\n  let w = 4;\n}\n";
        let (f, roots) = tmp_file("edit_del", content, "rs");
        let rel = f.to_string_lossy().to_string();
        block_on_rt(read_file(&serde_json::json!({"path": rel.clone()}), &roots)).unwrap();
        // start 定位 fn c 首行，new 为空 → 整块删除
        let args = serde_json::json!({"path": rel, "start": 8, "new": ""});
        let out = block_on_rt(edit_file(&args, &roots, "t_edit_del")).expect("块删除应成功");
        assert!(out.contains("删除"), "{out}");
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(!text.contains("fn c"), "方法 c 应被整块删除: {text}");
        assert!(text.contains("fn b"), "其余方法应保留: {text}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn edit_old_start_mutually_exclusive() {
        let content = "fn a() {\n  let x = 1;\n}\n";
        let (f, roots) = tmp_file("edit_mutex", content, "rs");
        let args = serde_json::json!({"path": f.to_string_lossy().to_string(), "old": "x", "start": 1});
        let err = block_on_rt(edit_file(&args, &roots, "t_edit_mutex")).unwrap_err();
        assert!(err.contains("互斥"), "{err}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    // ---------- grep_files block 模式 ----------

    #[test]
    fn grep_block_mode_shows_whole_method() {
        let content = "fn a() {\n  let x = 1;\n}\nfn b() {\n  let y = 2;\n}\n";
        let (f, roots) = tmp_file("grep_block", content, "rs");
        let args = serde_json::json!({"path": ".", "pattern": "let y", "block": true});
        let out = block_on_rt(grep_files(&args, &roots)).expect("搜索应成功");
        assert!(out.contains("完整代码块 L4-L6"), "应显示完整方法块: {out}");
        assert!(out.contains("let y = 2"), "{out}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn grep_plain_mode_keeps_single_line() {
        let content = "fn a() {\n  let x = 1;\n}\nfn b() {\n  let y = 2;\n}\n";
        let (f, roots) = tmp_file("grep_plain", content, "rs");
        let args = serde_json::json!({"path": ".", "pattern": "let y"});
        let out = block_on_rt(grep_files(&args, &roots)).expect("搜索应成功");
        assert!(out.contains("let y = 2"), "{out}");
        assert!(!out.contains("完整代码块"), "默认单行模式不应展开块: {out}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    // ---------- 注释识别 / 折叠清洗 ----------

    #[test]
    fn comment_line_detection() {
        assert!(is_comment_line("// x", "rs"));
        assert!(is_comment_line("  // x", "ts"));
        assert!(is_comment_line("/* x", "c"));
        assert!(is_comment_line(" * doc", "java"));
        assert!(is_comment_line("*/", "go"));
        assert!(is_comment_line("# x", "py"));
        assert!(is_comment_line("-- x", "sql"));
        assert!(!is_comment_line("let a = 1; // 行尾注释", "ts"), "行内注释不算整行注释");
        assert!(!is_comment_line("# 标题", "md"), "非代码语言不识别");
        assert!(!is_comment_line("// 字符串", "json"), "数据文件不识别");
    }

    #[test]
    fn read_file_folds_long_comment_blocks() {
        // 10 行头注释 + 3 行代码：注释占比 77% → 长注释块折叠，代码可见
        let content = format!(
            "{}",
            [
                "// 文件头说明",
                "// 文件头说明",
                "// 文件头说明",
                "// 文件头说明",
                "// 文件头说明",
                "// 文件头说明",
                "// 文件头说明",
                "// 文件头说明",
                "// 文件头说明",
                "// 文件头说明",
                "fn a() {",
                "  let x = 1;",
                "}",
            ]
            .join("\n")
        );
        let (f, roots) = tmp_file("read_fold", &content, "rs");
        let rel = f.to_string_lossy().to_string();
        let args = serde_json::json!({"path": rel});
        let out = block_on_rt(read_file(&args, &roots)).expect("读取应成功");
        assert!(out.contains("行注释已折叠"), "长注释块应折叠: {out}");
        assert!(out.contains("注释占 77%"), "应标注注释占比: {out}");
        assert!(out.contains("fn a() {") && out.contains("let x = 1"), "代码必须完整可见: {out}");
        assert!(!out.contains("文件头说明"), "折叠后不应再逐行展示注释: {out}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn read_file_keeps_short_comment_blocks() {
        // 3 行短注释块（< 8 行）即使注释占比高也保留原文
        let content = "// 简介\n// 简介2\n// 简介3\nfn a() {\n  let x = 1;\n}\n";
        let (f, roots) = tmp_file("read_keep", content, "rs");
        let rel = f.to_string_lossy().to_string();
        let args = serde_json::json!({"path": rel});
        let out = block_on_rt(read_file(&args, &roots)).expect("读取应成功");
        assert!(out.contains("简介2"), "短注释块应保留原文: {out}");
        assert!(!out.contains("行注释已折叠"), "短注释块不应折叠: {out}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn grep_marks_comment_hits() {
        let content = "// let y = 2\nfn b() {\n  let y = 2;\n}\n";
        let (f, roots) = tmp_file("grep_cmt", content, "rs");
        let args = serde_json::json!({"path": ".", "pattern": "let y"});
        let out = block_on_rt(grep_files(&args, &roots)).expect("搜索应成功");
        assert!(out.contains("[注释] // let y = 2"), "注释命中应标注: {out}");
        assert!(out.contains("let y = 2;"), "代码命中不应标注: {out}");
        std::fs::remove_dir_all(f.parent().unwrap()).ok();
    }

    #[test]
    fn grep_respects_gitignore() {
        // .gitignore 忽略的目录不进入搜索结果
        let dir = std::env::temp_dir().join(format!("agent_gi_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(dir.join("ignored")).unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::write(dir.join("kept.txt"), "needle here\n").unwrap();
        std::fs::write(dir.join("ignored").join("secret.txt"), "needle here\n").unwrap();
        let roots = vec![dir.to_string_lossy().to_string()];
        let args = serde_json::json!({"path": ".", "pattern": "needle"});
        let out = block_on_rt(grep_files(&args, &roots)).expect("搜索应成功");
        assert!(out.contains("kept.txt"), "应命中保留文件: {out}");
        assert!(!out.contains("secret.txt"), "gitignore 目录不应命中: {out}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_dir_respects_gitignore_and_hidden() {
        // gitignore 目录 + 隐藏目录跳过；普通文件可见；★ 关键文件标注
        let dir = std::env::temp_dir().join(format!("agent_ld_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(dir.join("ignored")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::write(dir.join("keep.txt"), "x").unwrap();
        std::fs::write(dir.join("README.md"), "# t").unwrap();
        std::fs::write(dir.join("ignored").join("secret.txt"), "x").unwrap();
        let roots = vec![dir.to_string_lossy().to_string()];
        let args = serde_json::json!({"path": "."});
        let out = block_on_rt(list_dir(&args, &roots)).expect("浏览应成功");
        assert!(out.contains("keep.txt"), "普通文件应可见: {out}");
        assert!(out.contains("README.md"), "关键文件应可见: {out}");
        assert!(!out.contains("secret.txt") && !out.contains("ignored/"), "gitignore 目录应跳过: {out}");
        assert!(!out.contains(".hidden"), "隐藏目录应跳过: {out}");
        assert!(out.contains("跳过 2 个忽略项"), "应统计跳过数: {out}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

