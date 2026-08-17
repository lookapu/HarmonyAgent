//! 鸿蒙开发高频坑知识库。
//!
//! 用途一（常驻）：鸿蒙工程对话首轮即通过 format_all_for_prompt 全量注入 system prompt，
//! 让 Agent 写代码/配置前就掌握正确写法；用途二（按需）：构建/运行错误出现时按关键词
//! 匹配附加根因与修复。条目必须是经实际构建/运行验证过的正确写法，禁止臆造版本号/API。

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
    KnowledgeEntry {
        keywords: &["compatiblesdkversion", "00306042", "00303038", "schema validate", "configuration error", "specification limit", "targetsdkversion", "compilesdkversion", "api level is 10", "must be string"],
        title: "SDK 版本字段格式（hvigor 00306042/00303038）",
        cause: "HarmonyOS 配置模式下 API 10 及以上，build-profile.json5 的 compileSdkVersion/compatibleSdkVersion/targetSdkVersion 必须写成 \"平台版本(API版本)\" 字符串（如 \"6.1.1(24)\"），裸数字 \"24\" 不合法；API 9 及以下可写数字。",
        fix: "写法：\"compatibleSdkVersion\": \"6.1.1(24)\"、\"targetSdkVersion\": \"6.1.1(24)\"。平台版本与 API 必须取自本机实际安装的 SDK：读 DEVECO_SDK_HOME（或 DevEco Studio 内置 SDK）下 default/sdk-pkg.json 的 platformVersion 与 apiVersion 字段，二者必须匹配（如 5.0.0 平台=API 12、6.1.1 平台=API 24），禁止臆造组合。",
    },
    KnowledgeEntry {
        keywords: &["00303083", "do not match", "platform version", "configured sdk version does not exist", "sdk version"],
        title: "平台版本与 API 版本不匹配（hvigor 00303083）",
        cause: "compatibleSdkVersion 的 \"平台版本(API版本)\" 组合在本机 SDK 中不存在（如 \"5.0.0(24)\"：5.0.0 平台只对应 API 12），hvigor 报 00303083 Configuration Error。",
        fix: "读 DEVECO_SDK_HOME 或 DevEco Studio 内置 SDK 的 default/sdk-pkg.json，用其中的 platformVersion 与 apiVersion 组合（如 \"6.1.1(24)\"）；也可直接参考本机可正常构建的鸿蒙工程（如 D:\\DevEcoStudioProjects 下用户工程的 build-profile.json5 配置）。",
    },
    KnowledgeEntry {
        keywords: &["unsigned", "signingconfigs", "signhap", "hap install", "install bundle failed", "signature", "provision profile", "signed.hap"],
        title: "真机安装必须签名（signingConfigs）",
        cause: "hvigor 默认产物 entry-default-unsigned.hap 未签名，hdc install 会因签名校验失败无法安装；签名材料（cer/p12/p7b）与 provisioning profile 绑定 bundleName 与设备，profile 不匹配会签名失败。",
        fix: "在 build-profile.json5 的 signingConfigs 配置 material（certpath/storeFile/profile/keyAlias/signAlg，密码可为 DevEco 加密格式 0000001B 开头，hvigor 直接支持）；签名材料的 bundleName 必须与 AppScope/app.json5 的 bundleName 一致；可复用 ~/.ohos/config 下已有的签名材料，或让用户在 DevEco Studio 自动签名。",
    },
    KnowledgeEntry {
        keywords: &["media", "app_icon", "icon", "png", "resources", "图标", "资源缺失", "resource not found", "cannot find resource", "image resource"],
        title: "鸿蒙工程图标资源必须真实存在（$media:icon / $media:app_icon）",
        cause: "AppScope/app.json5 的 icon 引用 $media:app_icon、entry 的 module.json5 的 abilities[].icon 引用 $media:icon 时，必须存在对应 PNG 文件；创建工程若只写了引用而没放图片（如 media 目录仅 .gitkeep），构建报图标资源缺失。",
        fix: "创建工程时同步创建 AppScope/resources/base/media/app_icon.png 与 entry/src/main/resources/base/media/icon.png（尺寸建议 1024x1024 / 512x512，PNG 格式，可由代码生成纯色底图），并确认 module.json5 与 app.json5 中引用名称与文件名一致；修改资源后 clean 再构建。",
    },
    KnowledgeEntry {
        keywords: &["dev eco sdk home", "sdk root", "00303217", "00303312", "sdk not found", "环境检测", "sdk 路径"],
        title: "构建实际使用的 SDK 由 DEVECO_SDK_HOME 决定",
        cause: "hvigor（HarmonyOS 模式）只认 DEVECO_SDK_HOME 环境变量定位 SDK；若该变量指向 DevEco Studio 内置 SDK（C:\\Program Files\\Huawei\\DevEco Studio\\sdk），则构建使用其 default 变体（如 6.1.1/API 24），即使环境检测另报了其他 SDK 目录（如 AppData\\Local\\Huawei\\Sdk 只有 8/9），写版本配置也应以 DEVECO_SDK_HOME 为准。",
        fix: "写 compatibleSdkVersion 前先确认实际构建 SDK：读 DEVECO_SDK_HOME 下的 default/sdk-pkg.json（platformVersion/apiVersion）；环境检测与构建 SDK 不一致时以构建实际使用的为准。",
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

/// 全量内置知识条目，格式化供鸿蒙项目常驻注入（对话首轮即注入，
/// 让 Agent 写代码/配置前就掌握高频坑，而不是等构建失败后才匹配到）。
pub fn format_all_for_prompt() -> String {
    if ENTRIES.is_empty() {
        return String::new();
    }
    let mut s = String::from("鸿蒙知识库（高频坑与正确写法，涉及对应场景时优先遵守）：\n");
    for e in ENTRIES {
        s.push_str(&format!("- {}：{} 修复：{}\n", e.title, e.cause, e.fix));
    }
    s
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
