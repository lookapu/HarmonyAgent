//! Android / Web / TypeScript 常见实现到 HarmonyOS 的证据化迁移建议。

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::services::sdk_api::{ApiIndex, ProjectApiContext};

struct MigrationRule {
    platform: &'static str,
    concepts: &'static [&'static str],
    source_name: &'static str,
    strategy: &'static str,
    candidates: &'static [MigrationCandidate],
    caveats: &'static [&'static str],
}

struct MigrationCandidate {
    module: &'static str,
    symbols: &'static [&'static str],
    purpose: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifiedMigrationTarget {
    pub module: String,
    pub symbols: Vec<String>,
    pub purpose: String,
    /// verified / conditional / unavailable / unverified
    pub status: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MigrationAdvice {
    pub source_platform: String,
    pub source_concept: String,
    pub matched_pattern: String,
    pub strategy: String,
    pub targets: Vec<VerifiedMigrationTarget>,
    pub caveats: Vec<String>,
    pub verification_steps: Vec<String>,
}

pub fn advise(
    platform: &str,
    concept: &str,
    context: &ProjectApiContext,
    index: Option<&ApiIndex>,
    official_db: Option<&Connection>,
) -> Result<Vec<MigrationAdvice>, String> {
    let platform = normalize_platform(platform)?;
    let query = concept.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Err("迁移建议需要非空 concept".into());
    }
    let matched = RULES
        .iter()
        .filter(|rule| {
            rule.platform == platform
                && rule
                    .concepts
                    .iter()
                    .any(|keyword| query.contains(&keyword.to_ascii_lowercase()))
        })
        .take(5)
        .collect::<Vec<_>>();
    if matched.is_empty() {
        return Err(format!(
            "未找到 {platform} 概念“{concept}”的内置迁移模式。可尝试：{}",
            supported_concepts(platform).join("、")
        ));
    }
    Ok(matched
        .into_iter()
        .map(|rule| MigrationAdvice {
            source_platform: platform.into(),
            source_concept: concept.trim().into(),
            matched_pattern: rule.source_name.into(),
            strategy: rule.strategy.into(),
            targets: rule
                .candidates
                .iter()
                .map(|candidate| verify_candidate(candidate, context, index, official_db))
                .collect(),
            caveats: rule.caveats.iter().map(|value| (*value).into()).collect(),
            verification_steps: vec![
                "先读取状态为 verified/conditional 的本机 .d.ts 精确签名，不复制平台原 API 形状"
                    .into(),
                "生成或修改 ArkTS 后运行 lsp_diagnostics，清零语法、类型和模块解析错误".into(),
                "运行 check_sdk_alignment 复核权限、SystemCapability、设备类型与 module.json5"
                    .into(),
                "最后用 build_project 构建；涉及设备能力时部署真机并验证允许、拒绝和恢复路径"
                    .into(),
            ],
        })
        .collect())
}

pub fn render(context: &ProjectApiContext, advice: &[MigrationAdvice]) -> String {
    let mut out = format!("迁移上下文：{}", context.describe());
    for item in advice {
        out.push_str(&format!(
            "\n\n══ {} → HarmonyOS：{} ══\n策略：{}",
            item.source_platform, item.matched_pattern, item.strategy
        ));
        for target in &item.targets {
            out.push_str(&format!(
                "\n- [{}] {}{}：{}",
                target.status,
                target.module,
                if target.symbols.is_empty() {
                    String::new()
                } else {
                    format!(" :: {}", target.symbols.join(", "))
                },
                target.purpose
            ));
            for evidence in target.evidence.iter().take(4) {
                out.push_str(&format!("\n  证据：{evidence}"));
            }
        }
        if !item.caveats.is_empty() {
            out.push_str("\n风险边界：");
            for caveat in &item.caveats {
                out.push_str(&format!("\n- {caveat}"));
            }
        }
        out.push_str("\n验证闭环：");
        for (index, step) in item.verification_steps.iter().enumerate() {
            out.push_str(&format!("\n{}. {step}", index + 1));
        }
    }
    out
}

fn verify_candidate(
    candidate: &MigrationCandidate,
    context: &ProjectApiContext,
    index: Option<&ApiIndex>,
    official_db: Option<&Connection>,
) -> VerifiedMigrationTarget {
    let mut evidence = Vec::new();
    let module = index.and_then(|index| {
        index
            .modules
            .iter()
            .find(|module| module.module.eq_ignore_ascii_case(candidate.module))
    });
    let mut status = "unverified";
    if let Some(module) = module {
        let availability = context.availability(module.since_min, module.deprecated);
        evidence.push(format!(
            "本机 SDK={} | since API {} | {} | {}",
            module.path,
            module
                .since_min
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".into()),
            availability,
            module.kit.as_deref().unwrap_or("kit unknown")
        ));
        let found = candidate
            .symbols
            .iter()
            .filter(|expected| {
                module
                    .symbols
                    .iter()
                    .any(|symbol| symbol.name.eq_ignore_ascii_case(expected))
                    || module
                        .declarations
                        .iter()
                        .any(|symbol| symbol.eq_ignore_ascii_case(expected))
            })
            .copied()
            .collect::<Vec<_>>();
        if !candidate.symbols.is_empty() {
            evidence.push(format!(
                "本机符号命中 {}/{}：{}",
                found.len(),
                candidate.symbols.len(),
                if found.is_empty() {
                    "无".into()
                } else {
                    found.join(",")
                }
            ));
        }
        status = if availability.starts_with("不可用") {
            "unavailable"
        } else if availability.starts_with("条件可用") {
            "conditional"
        } else if candidate.symbols.is_empty() || found.len() == candidate.symbols.len() {
            "verified"
        } else {
            "unverified"
        };
    } else if index.is_some() {
        status = "unavailable";
        evidence.push(format!("当前本机 SDK 索引不存在 {}", candidate.module));
    } else {
        evidence.push("未配置本机 SDK 索引，不能验证目标模块".into());
    }

    if let Some((source_url, since, deprecated)) = official_reference(official_db, candidate.module)
    {
        evidence.push(format!(
            "官方参考={} | since API {} | deprecated={deprecated}",
            source_url,
            since
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".into())
        ));
    } else if let Some((source_url, change, level)) = official_change(official_db, candidate.module)
    {
        evidence.push(format!(
            "官方变更={} | {} API {}",
            source_url,
            change,
            level
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".into())
        ));
    } else {
        evidence.push(
            "本地官方知识库暂无该模块来源；需要 refresh_api_details/refresh_api_db 后复核".into(),
        );
    }

    VerifiedMigrationTarget {
        module: candidate.module.into(),
        symbols: candidate
            .symbols
            .iter()
            .map(|value| (*value).into())
            .collect(),
        purpose: candidate.purpose.into(),
        status: status.into(),
        evidence,
    }
}

fn official_reference(
    db: Option<&Connection>,
    module: &str,
) -> Option<(String, Option<u32>, bool)> {
    let conn = db?;
    conn.query_row(
        "SELECT source_url, since_api_level, deprecated FROM api_details WHERE lower(module)=lower(?1) LIMIT 1",
        params![module],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get::<_, i64>(2)? != 0,
            ))
        },
    )
    .ok()
}

fn official_change(db: Option<&Connection>, module: &str) -> Option<(String, String, Option<u32>)> {
    let conn = db?;
    conn.query_row(
        "SELECT source_url, change_type, api_level FROM api_docs WHERE lower(module)=lower(?1) ORDER BY api_level DESC LIMIT 1",
        params![module],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .ok()
}

fn normalize_platform(platform: &str) -> Result<&'static str, String> {
    match platform.trim().to_ascii_lowercase().as_str() {
        "android" | "kotlin" | "java" => Ok("android"),
        "web" | "browser" | "javascript" | "js" => Ok("web"),
        "typescript" | "ts" => Ok("typescript"),
        _ => Err("source_platform 仅支持 android、web、typescript".into()),
    }
}

fn supported_concepts(platform: &str) -> Vec<&'static str> {
    RULES
        .iter()
        .filter(|rule| rule.platform == platform)
        .map(|rule| rule.source_name)
        .collect()
}

const RULES: &[MigrationRule] = &[
    MigrationRule {
        platform: "android",
        concepts: &["activity", "fragment", "intent", "navigation"],
        source_name: "Activity / Fragment / Intent / Navigation",
        strategy: "用 UIAbility 承载生命周期，用 Want 传递启动参数；复杂页面栈用 ArkUI Navigation/NavPathStack，不照搬 FragmentManager。",
        candidates: &[
            MigrationCandidate {
                module: "@kit.AbilityKit",
                symbols: &["UIAbility", "Want"],
                purpose: "应用组件生命周期与启动参数",
            },
            MigrationCandidate {
                module: "@kit.ArkUI",
                symbols: &["NavPathStack"],
                purpose: "声明式页面导航与栈管理",
            },
        ],
        caveats: &[
            "Android Context、Activity 和 Fragment 没有一一对应关系",
            "跨 Ability 跳转与页面内导航必须分层设计",
        ],
    },
    MigrationRule {
        platform: "android",
        concepts: &["sharedpreferences", "datastore", "key-value", "key value"],
        source_name: "SharedPreferences / DataStore",
        strategy: "把轻量持久键值迁移到 Preferences；建立显式数据类型、命名空间和升级策略。",
        candidates: &[MigrationCandidate {
            module: "@ohos.data.preferences",
            symbols: &["Preferences", "getPreferences"],
            purpose: "应用级轻量键值持久化",
        }],
        caveats: &[
            "不要把大对象或关系数据继续塞入键值存储",
            "首次迁移需要设计旧数据导入与幂等标记",
        ],
    },
    MigrationRule {
        platform: "android",
        concepts: &["room", "sqlite", "database"],
        source_name: "Room / SQLite",
        strategy: "将实体和 DAO 语义重建为关系型存储的表、谓词和事务；保留 schema 版本与可回滚迁移。",
        candidates: &[MigrationCandidate {
            module: "@ohos.data.relationalStore",
            symbols: &["RdbStore", "RdbPredicates"],
            purpose: "关系数据、查询与事务",
        }],
        caveats: &[
            "Room 注解处理器生成代码不能直接迁移",
            "必须为既有数据库设计版本迁移与失败恢复",
        ],
    },
    MigrationRule {
        platform: "android",
        concepts: &["retrofit", "okhttp", "http", "network"],
        source_name: "Retrofit / OkHttp",
        strategy: "用 Network Kit HTTP 客户端重建请求层，在业务层保留 DTO、错误分类、取消、重试与超时策略。",
        candidates: &[MigrationCandidate {
            module: "@ohos.net.http",
            symbols: &["createHttp", "HttpRequest"],
            purpose: "HTTP 请求、响应与生命周期",
        }],
        caveats: &[
            "不要无条件重试非幂等请求",
            "证书、代理和网络权限必须单独核对",
        ],
    },
    MigrationRule {
        platform: "android",
        concepts: &["broadcastreceiver", "broadcast", "eventbus"],
        source_name: "BroadcastReceiver / EventBus",
        strategy: "系统或跨应用事件使用 CommonEvent，进程内状态优先用 ArkUI 状态管理，避免把所有消息都迁成全局广播。",
        candidates: &[MigrationCandidate {
            module: "@ohos.commonEventManager",
            symbols: &["subscribe", "publish"],
            purpose: "公共事件订阅与发布",
        }],
        caveats: &["跨应用事件涉及权限和导出边界", "订阅生命周期必须显式释放"],
    },
    MigrationRule {
        platform: "web",
        concepts: &["localstorage", "sessionstorage", "storage"],
        source_name: "localStorage / sessionStorage",
        strategy: "持久键值使用 Preferences；只属于页面会话的瞬态状态放入 ArkUI 状态管理，不机械持久化。",
        candidates: &[MigrationCandidate {
            module: "@ohos.data.preferences",
            symbols: &["Preferences", "getPreferences"],
            purpose: "持久键值数据",
        }],
        caveats: &[
            "Preferences 为异步 API，需要重新设计初始化时序",
            "敏感数据应使用安全存储而非普通 Preferences",
        ],
    },
    MigrationRule {
        platform: "web",
        concepts: &["fetch", "xmlhttprequest", "axios", "http"],
        source_name: "fetch / XMLHttpRequest / Axios",
        strategy: "迁移到 Network Kit HTTP 请求对象，显式管理销毁、超时、错误码、网络权限与取消。",
        candidates: &[MigrationCandidate {
            module: "@ohos.net.http",
            symbols: &["createHttp", "HttpRequest"],
            purpose: "HTTP 网络访问",
        }],
        caveats: &[
            "浏览器 CORS 模型与原生应用网络权限模型不同",
            "组件销毁时必须取消或释放请求对象",
        ],
    },
    MigrationRule {
        platform: "web",
        concepts: &["websocket", "socket"],
        source_name: "WebSocket",
        strategy: "使用 Network Kit WebSocket，并把连接、重连、心跳和前后台切换建模为可恢复状态机。",
        candidates: &[MigrationCandidate {
            module: "@ohos.net.webSocket",
            symbols: &["createWebSocket", "WebSocket"],
            purpose: "双向长连接",
        }],
        caveats: &[
            "后台存活策略不能照搬浏览器标签页",
            "重连必须有退避和网络状态门禁",
        ],
    },
    MigrationRule {
        platform: "web",
        concepts: &[
            "history",
            "react router",
            "vue router",
            "router",
            "navigation",
        ],
        source_name: "History API / Web Router",
        strategy: "用 ArkUI Navigation/NavPathStack 表达页面栈、参数和返回结果；外部拉起另由 Want/Ability 处理。",
        candidates: &[
            MigrationCandidate {
                module: "@kit.ArkUI",
                symbols: &["NavPathStack"],
                purpose: "应用内声明式导航",
            },
            MigrationCandidate {
                module: "@kit.AbilityKit",
                symbols: &["Want"],
                purpose: "Ability 间与外部启动参数",
            },
        ],
        caveats: &[
            "URL 与原生页面栈不是同一状态模型",
            "需要显式设计深链、恢复和返回结果",
        ],
    },
    MigrationRule {
        platform: "typescript",
        concepts: &["node fs", "filesystem", "file system", " fs"],
        source_name: "Node.js fs",
        strategy: "使用 File Kit 文件 API，并基于应用沙箱 URI/路径重新设计访问范围与权限。",
        candidates: &[MigrationCandidate {
            module: "@ohos.file.fs",
            symbols: &["open", "read", "write"],
            purpose: "应用沙箱文件访问",
        }],
        caveats: &[
            "Node.js 的任意主机路径假设不成立",
            "用户文件应通过选择器/URI 授权链访问",
        ],
    },
    MigrationRule {
        platform: "typescript",
        concepts: &["eventemitter", "event emitter", "events"],
        source_name: "Node.js EventEmitter",
        strategy: "跨组件事件可用 emitter，但 UI 状态优先使用 ArkUI 状态管理，确保订阅可追踪并在生命周期结束时释放。",
        candidates: &[MigrationCandidate {
            module: "@ohos.events.emitter",
            symbols: &["on", "emit", "off"],
            purpose: "进程内事件发布订阅",
        }],
        caveats: &[
            "不要用全局事件总线替代单向状态流",
            "回调持有对象时需防止生命周期泄漏",
        ],
    },
    MigrationRule {
        platform: "typescript",
        concepts: &[
            "worker",
            "worker_threads",
            "web worker",
            "concurrency",
            "taskpool",
        ],
        source_name: "Worker / worker_threads",
        strategy: "CPU 密集型独立任务优先迁移到 TaskPool，按可传输数据和取消边界拆分任务。",
        candidates: &[MigrationCandidate {
            module: "@ohos.taskpool",
            symbols: &["Task", "execute"],
            purpose: "并发任务调度",
        }],
        caveats: &[
            "闭包、UI 对象和不可序列化状态不能直接跨线程",
            "I/O 异步不应为了形式一致滥用线程池",
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::sdk_api::{ApiModule, ApiSymbol};

    #[test]
    fn verifies_android_storage_against_project_sdk_and_official_source() {
        let index = ApiIndex {
            modules: vec![ApiModule {
                module: "@ohos.data.preferences".into(),
                since_min: Some(9),
                declarations: vec!["Preferences".into(), "getPreferences".into()],
                symbols: vec![ApiSymbol {
                    name: "Preferences".into(),
                    kind: "interface".into(),
                    since: Some(9),
                    deprecated: false,
                    syscap: None,
                    permissions: Vec::new(),
                    replacement: None,
                }],
                path: "/sdk/@ohos.data.preferences.d.ts".into(),
                ..empty_module()
            }],
            ..Default::default()
        };
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE api_details (module TEXT, source_url TEXT, since_api_level INTEGER, deprecated INTEGER); INSERT INTO api_details VALUES ('@ohos.data.preferences','https://example.test/preferences',9,0);").unwrap();
        let advice = advise(
            "android",
            "SharedPreferences",
            &ProjectApiContext {
                compile_api: Some(12),
                compatible_api: Some(9),
                ..Default::default()
            },
            Some(&index),
            Some(&conn),
        )
        .unwrap();
        assert_eq!(advice[0].targets[0].status, "verified");
        assert!(
            advice[0].targets[0]
                .evidence
                .iter()
                .any(|value| value.contains("https://example.test/preferences"))
        );
    }

    #[test]
    fn rejects_unknown_platform_and_lists_known_concepts() {
        assert!(
            advise(
                "ios",
                "UIViewController",
                &ProjectApiContext::default(),
                None,
                None
            )
            .is_err()
        );
        let error = advise(
            "web",
            "unknown-widget",
            &ProjectApiContext::default(),
            None,
            None,
        )
        .unwrap_err();
        assert!(error.contains("localStorage"));
    }

    fn empty_module() -> ApiModule {
        ApiModule {
            module: String::new(),
            kit: None,
            syscap: None,
            system_capabilities: Vec::new(),
            permissions: Vec::new(),
            since_min: None,
            since_max: None,
            declarations: Vec::new(),
            symbols: Vec::new(),
            deprecated: false,
            path: String::new(),
        }
    }
}
