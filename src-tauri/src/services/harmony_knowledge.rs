//! 鸿蒙开发常见错误知识库（按需注入，不常驻 system prompt）。
//!
//! 目的：Agent 改代码/修 bug 时，遇到高频鸿蒙报错能直接给出"根因 + 正确写法"，
//! 而不是让模型凭通用 TS 经验猜测（Stage 模型、ArkTS 严格模式、资源引用等有很多
//! 与普通 Web/TS 不同的约束）。为节省 token，只在出现对应错误时按类别附加。

/// 一条知识条目：关键词匹配 + 简短根因 + 修复要点。
pub struct KnowledgeEntry {
    pub keywords: &'static [&'static str],
    pub title: &'static str,
    pub cause: &'static str,
    pub fix: &'static str,
}

/// ArkTS / Stage 模型 / 资源 高频错误配方。
/// 关键词使用小写匹配（调用前先 lower-case）。
const ENTRIES: &[KnowledgeEntry] = &[
    KnowledgeEntry {
        keywords: &["use sharedstorage", "sharepreference", "getpreferencessync", "preferences.getpreferences"],
        title: "轻量级存储 API 误用（@ohos.data.preferences）",
        cause: "HarmonyOS 没有 localStorage/SharedStorage；旧文档里的 dataPreferences 已改为 preferences。",
        fix: "用 import dataPreferences from '@ohos.data.preferences'; dataPreferences.getPreferences(context, 'mystore')，读写为异步 put/flush。context 用 UIAbility 的 this.context。",
    },
    KnowledgeEntry {
        keywords: &["cannot find name", "is not defined", "uiabilitycontext", "abilitycontext"],
        title: "Stage 模型 Context/Ability 获取方式",
        cause: "Stage 模型下没有全局 ability/context，FA 模型的 getContext(this) 写法在 Stage 不适用。",
        fix: "在 EntryAbility 的 onCreate(windowStage) 中把 this.context 存入 AppStorage，或在组件内用 getContext(this) as common.UIAbilityContext。this 指 UIAbility 实例而非组件。",
    },
    KnowledgeEntry {
        keywords: &["@state", "object reference", "observed", "objectlink", "does not refresh", "不刷新"],
        title: "@State 对对象/数组不触发刷新",
        cause: "ArkTS 的 @State 仅观察第一层引用变化；嵌套对象属性修改需要 @Observed + @ObjectLink，或整体替换引用。",
        fix: "对类加 @Observed，子组件用 @ObjectLink 接收；数组变更用 this.arr = [...this.arr, item] 整体替换，避免 push/splice 后不刷新。",
    },
    KnowledgeEntry {
        keywords: &["resource", "string:app", "$r(", "resourcemanage", "getstringvalue", "\\$r is not"],
        title: "$r 资源引用与 Resource 类型",
        cause: "$r('app.string.xxx') 返回 Resource 对象而非字符串，不能直接拼接或当 string 传；资源名必须在 resources/base/element/string.json 存在。",
        fix: "显示时 Text($r('app.string.xxx')) 直接接收 Resource；需要字符串值时用 getContext(this).resourceManager.getStringValue($r('app.string.xxx'))。多语言在 resources/en_US、zh_CN 下提供同名资源。",
    },
    KnowledgeEntry {
        keywords: &["navigation", "navpathstack", "router.pushurl", "router.back", "page not found"],
        title: "页面路由（Navigation/NavPathStack vs router）",
        cause: "Stage 模型推荐 Navigation + NavPathStack 声明式路由；旧 router.pushUrl 在 Navigation 容器内可能不生效。",
        fix: "用 NavPathStack.pushPath({ name: 'PageName' })，在 NavDestination 中注册 name；返回用 pop()。跨模块用 import 导入页面组件。",
    },
    KnowledgeEntry {
        keywords: &["import", "cannot find module", "has no exported member", "ohpm", "module not found"],
        title: "模块导入与 ohpm 依赖",
        cause: "@kit.* 是 Kit 聚合导入需 SDK 支持；三方库需先 ohpm install；相对路径后缀在 ArkTS 中通常省略。",
        fix: "三方库：在模块目录执行 ohpm install <pkg>；系统能力用 import { xxx } from '@kit.KitName'；确认 compatibleSdkVersion 不低于该 API 引入版本。",
    },
    KnowledgeEntry {
        keywords: &["arkts", "any is disallowed", "arkts-no-any", "use explicit type", "unknown type"],
        title: "ArkTS 严格类型限制（no-any/结构化类型）",
        cause: "ArkTS 是 TypeScript 的受限子集，禁止 any、对象字面量直接当接口、structural typing 等。",
        fix: "显式声明 interface/class 类型；不确定时用具体类型而非 any；JSON.parse 后用 class 实例并校验字段；不要写 as any。",
    },
    KnowledgeEntry {
        keywords: &["requestpermissions", "permission", "201", "2013", "not granted", "verifyaccesstoken"],
        title: "权限声明与运行时申请",
        cause: "仅在 module.json5 的 requestPermissions 声明不够，危险权限（如 INTERNET 是 normal 级，相机/位置是 dangerous）还需运行时申请。",
        fix: "module.json5 的 requestPermissions 加 name/usedScene；运行时用 abilityAccessCtrl.createAtManager().requestPermissionsFromUser(context, perms)；用 globalThis.abilityContext 或 this.context。",
    },
    KnowledgeEntry {
        keywords: &["windowstage", "windowstageevent", "onwindowstagecreate", "setwindowlayoutfullscreen"],
        title: "窗口与沉浸式配置位置",
        cause: "沉浸式/状态栏应在 EntryAbility.onCreate 的 windowStage 上设置，而非页面 onPageShow。",
        fix: "onCreate(w: WindowStage) { w.getMainWindowSync().setWindowLayoutFullScreen(true); setWindowSystemBarProperties(...) }。",
    },
    KnowledgeEntry {
        keywords: &["hap", "module.json5", "abilities", "mainElement", "entrycard", "no ability"],
        title: "module.json5 / Ability 配置",
        cause: "Ability 必须在 module.json5 的 abilities 数组注册，且 srcEntry 指向的类存在并导出；mainElement 要与 Ability 名一致。",
        fix: "检查 srcEntry 路径（如 './ets/entryability/EntryAbility.ets'）、name、export default class、@Entry 装饰器；修改后 clean 再构建。",
    },
];

/// 用于展示的知识条目（内置或用户自定义）
#[derive(Debug, Clone)]
pub struct MatchedEntry {
    pub title: String,
    pub cause: String,
    pub fix: String,
}

impl MatchedEntry {
    fn from_builtin(e: &'static KnowledgeEntry) -> Self {
        Self { title: e.title.into(), cause: e.cause.into(), fix: e.fix.into() }
    }
}

/// 针对一段错误文本，返回命中的知识条目（最多 max 条），按相关性打分排序。
/// 打分：关键词在错误文本中出现次数（TF）× 词长权重（长词特异性高），
/// 标题关键词命中额外加权（标题更凝练地表达条目主题）。
/// 用于在构建/类型错误时附加给 Agent，避免它重复踩坑。
pub fn match_knowledge(error_text: &str, max: usize) -> Vec<&'static KnowledgeEntry> {
    let lower = error_text.to_lowercase();
    let mut scored: Vec<(f64, &'static KnowledgeEntry)> = ENTRIES
        .iter()
        .filter_map(|e| {
            let score = score_kw_match(&lower, &e.keywords, &e.title);
            if score > 0.0 { Some((score, e)) } else { None }
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(max).map(|(_, e)| e).collect()
}

/// 关键词相关性打分：TF（出现次数）× 词长权重；标题命中额外 +1.5 加权。
fn score_kw_match(lower_text: &str, keywords: &[&str], title: &str) -> f64 {
    let lower_title = title.to_lowercase();
    let mut score = 0.0f64;
    for k in keywords {
        let k = k.to_lowercase();
        if k.is_empty() {
            continue;
        }
        // TF：出现次数
        let mut hits = 0usize;
        let mut start = 0;
        while let Some(pos) = lower_text[start..].find(&k) {
            hits += 1;
            start += pos + k.len();
        }
        if hits == 0 {
            continue;
        }
        // 词长权重：4 字词满权重，2 字词约半权重
        let len_w = (k.chars().count() as f64 / 4.0).min(1.0);
        score += hits as f64 * (0.5 + 0.5 * len_w);
        // 标题命中额外加权
        if lower_title.contains(&k) {
            score += 1.5;
        }
    }
    score
}

/// 同时匹配内置条目与用户自定义条目。用户条目前缀（在结果中排前面），
/// 因为它代表团队/项目特定经验，优先于通用内置知识。
/// user_entries 元素为 (id, keywords, title, cause, fix)；命中的用户条目 id
/// 通过 out_hit_ids 返回，供调用方累加 hit_count。
pub fn match_knowledge_with_user<'a>(
    error_text: &str,
    max: usize,
    user_entries: &'a [(String, String, String, String, String)],
) -> (Vec<MatchedEntry>, Vec<&'a str>) {
    let lower = error_text.to_lowercase();
    let mut scored: Vec<(f64, MatchedEntry, Option<&'a str>)> = Vec::new();

    for (id, keywords, title, cause, fix) in user_entries {
        let kws: Vec<&str> = keywords.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        // 用户条目前缀：+100 保证团队经验排在通用内置之前
        let score = score_kw_match(&lower, &kws, title) + 100.0;
        if score > 100.0 {
            scored.push((score, MatchedEntry { title: title.clone(), cause: cause.clone(), fix: fix.clone() }, Some(id.as_str())));
        }
    }
    for e in match_knowledge(error_text, max) {
        scored.push((1.0, MatchedEntry::from_builtin(e), None));
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut hit_ids = Vec::new();
    let entries = scored
        .into_iter()
        .take(max)
        .map(|(_, e, id)| {
            if let Some(i) = id {
                hit_ids.push(i);
            }
            e
        })
        .collect();
    (entries, hit_ids)
}

/// 把命中的知识条目格式化为可追加到工具失败结果的文本。
pub fn format_matched(entries: &[MatchedEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n【鸿蒙知识库·相关条目】\n");
    for e in entries {
        s.push_str(&format!("- ▶ {}\n  根因: {}\n  修复: {}\n", e.title, e.cause, e.fix));
    }
    s
}

/// 把命中的知识条目格式化为可追加到工具失败结果的文本。
pub fn format_knowledge(entries: &[&'static KnowledgeEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mapped: Vec<MatchedEntry> = entries.iter().map(|e| MatchedEntry::from_builtin(e)).collect();
    format_matched(&mapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_preferences_api() {
        let hits = match_knowledge("Error: getPreferencesSync is not a function in dataPreferences", 3);
        assert!(hits.iter().any(|e| e.title.contains("preferences") || e.title.contains("存储")));
    }

    #[test]
    fn match_state_observed() {
        let hits = match_knowledge("@State array push 后 UI 不刷新 observed", 3);
        assert!(hints_about_state(hits));
    }

    fn hints_about_state(hits: Vec<&KnowledgeEntry>) -> bool {
        hits.iter().any(|e| e.title.contains("@State") || e.keywords.contains(&"@state"))
    }

    #[test]
    fn no_match_for_unrelated() {
        assert!(match_knowledge("everything is fine", 3).is_empty());
    }

    #[test]
    fn arkts_strict_match() {
        let hits = match_knowledge("ArkTS ERROR: any is disallowed (arkts-no-any)", 3);
        assert!(hits.iter().any(|e| e.title.contains("ArkTS")));
    }
}
