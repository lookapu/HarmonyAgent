//! 模型路由：按价格与任务类型自动选择最合适的模型。
//!
//! 两类能力：
//! 1. `pick_economy_model`：同 Provider 中比主模型更便宜、已启用、支持工具调用的模型（用于摘要/标题/子 Agent）。
//! 2. `TaskKind` + `pick_model_for_task`：根据任务类型（对话/代码/快速/视觉）按能力与价格选模，
//!    在不突破用户显式选择的前提下降低成本、提升匹配度。

use rusqlite::Connection;

/// 任务类型：用于把不同性质的请求路由到能力/价格最匹配的模型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// 普通对话/解释/问答（需要工具调用能力，但不追求最强代码推理）
    Chat,
    /// 代码编写/重构/调试（优先支持工具调用、上下文足够大的模型）
    Code,
    /// 轻量任务：标题生成、摘要、记忆提取、简单改写（优先便宜模型）
    Fast,
    /// 含图片输入（必须支持 image 输入模态）
    Vision,
}

impl TaskKind {
    /// 该任务类型是否要求模型支持工具调用。
    fn needs_tool_call(self) -> bool {
        matches!(self, TaskKind::Chat | TaskKind::Code)
    }

    /// 该任务类型要求的输入模态（"text" / "image"）。
    fn required_input(self) -> &'static str {
        match self {
            TaskKind::Vision => "image",
            _ => "text",
        }
    }
}

/// 候选模型行（价格：元 / 百万 token；能力位用于筛选）。
struct CandRow {
    model_id: String,
    input_price: f64,
    output_price: f64,
    context_limit: i64,
    tool_call: bool,
    input_modalities: String,
}

/// 候选模型行（价格：元 / 百万 token）
struct PriceRow {
    model_id: String,
    input_price: f64,
    output_price: f64,
}

/// 根据用户消息内容与附件情况推断任务类型。
/// - 含图片 → Vision
/// - 含明确代码/工程/构建/调试意图 → Code
/// - 纯短问答/问候/闲聊/简单翻译 → Chat
///   `is_background_aux` 用于摘要/标题/记忆等后台辅助任务，直接判为 Fast。
pub fn classify_task(message: &str, has_images: bool, is_background_aux: bool) -> TaskKind {
    if is_background_aux {
        return TaskKind::Fast;
    }
    if has_images {
        return TaskKind::Vision;
    }
    let lower = message.to_lowercase();
    let code_kw = [
        "代码", "函数", "重构", "调试", "报错", "bug", "编译", "构建", "hvigor", "arkts",
        "ets", "typescript", "rust", "java", "kotlin", "swift", "python", "组件", "接口",
        "实现", "修复", "单元测试", "部署", "hap", "hdc", "ohpm", "class ", "struct ",
        "fn ", "function ", "async ", "await", "import ", "export ",
    ];
    let is_code = code_kw.iter().any(|k| lower.contains(k));
    if is_code {
        TaskKind::Code
    } else {
        TaskKind::Chat
    }
}

/// 按任务类型在同 Provider 内挑选最合适的模型。
///
/// 规则：
/// - Vision：必须支持 image 输入，在满足条件的模型中选最便宜的（不限工具调用）。
/// - Fast：最便宜、已启用、不要求工具调用（用于摘要/标题等后台任务）。
/// - Code：已启用、支持工具调用、上下文窗口最大者（同等时取更便宜）。
/// - Chat：已启用、支持工具调用，优先不超过主模型价格的最便宜模型（无则回退主模型）。
///
/// 任何不确定情况（缺价格/缺候选）都返回 None，调用方回退主模型，不做破坏性切换。
pub fn pick_model_for_task(
    conn: &Connection,
    provider_id: &str,
    main_model: &str,
    kind: TaskKind,
) -> Option<String> {
    // 主模型上下文窗口（Code 任务用于比较），查不到则按 0 处理
    let main_ctx: i64 = conn
        .query_row(
            "SELECT context_limit FROM models WHERE provider_id = ?1 AND model_id = ?2",
            rusqlite::params![provider_id, main_model],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let mut stmt = conn
        .prepare(
            "SELECT model_id, input_price_per_mtok, output_price_per_mtok,
                    context_limit, tool_call, input_modalities
             FROM models
             WHERE provider_id = ?1 AND enabled = 1 AND model_id != ?2",
        )
        .ok()?;
    let rows = stmt
        .query_map(rusqlite::params![provider_id, main_model], |r| {
            Ok(CandRow {
                model_id: r.get(0)?,
                input_price: r.get(1)?,
                output_price: r.get(2)?,
                context_limit: r.get(3)?,
                tool_call: r.get::<_, i64>(4)? != 0,
                input_modalities: r.get(5)?,
            })
        })
        .ok()?;

    let mut cands: Vec<CandRow> = Vec::new();
    for row in rows.flatten() {
        // 工具调用门槛
        if kind.needs_tool_call() && !row.tool_call {
            continue;
        }
        // 输入模态门槛
        if kind.required_input() == "image" && !modalities_include(&row.input_modalities, "image") {
            continue;
        }
        // 未定价模型不参与自动路由（避免选到未知的昂贵模型）
        let total = row.input_price + row.output_price;
        if total <= 0.0 {
            continue;
        }
        cands.push(row);
    }
    if cands.is_empty() {
        return None;
    }

    match kind {
        TaskKind::Fast | TaskKind::Vision => {
            // 最便宜优先
            cands.sort_by(|a, b| {
                (a.input_price + a.output_price)
                    .partial_cmp(&(b.input_price + b.output_price))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            cands.first().map(|r| r.model_id.clone())
        }
        TaskKind::Code => {
            // 上下文窗口最大优先，同等上下文取更便宜
            cands.sort_by(|a, b| {
                b.context_limit
                    .cmp(&a.context_limit)
                    .then_with(|| {
                        (a.input_price + a.output_price)
                            .partial_cmp(&(b.input_price + b.output_price))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
            });
            // 只在候选上下文不小于主模型时才切换（否则回退主模型）
            cands
                .into_iter()
                .find(|r| r.context_limit >= main_ctx)
                .map(|r| r.model_id)
        }
        TaskKind::Chat => {
            // 不超过主模型价格的候选中取最便宜；无则回退主模型
            let main_total = conn
                .query_row(
                    "SELECT input_price_per_mtok + output_price_per_mtok FROM models
                     WHERE provider_id = ?1 AND model_id = ?2",
                    rusqlite::params![provider_id, main_model],
                    |r| r.get::<_, f64>(0),
                )
                .unwrap_or(0.0);
            if main_total <= 0.0 {
                return None;
            }
            cands.retain(|r| r.input_price + r.output_price <= main_total);
            cands.sort_by(|a, b| {
                (a.input_price + a.output_price)
                    .partial_cmp(&(b.input_price + b.output_price))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            cands.first().map(|r| r.model_id.clone())
        }
    }
}

/// 视觉兜底：同 Provider 内挑选支持 image 输入且已启用的模型（不要求已定价）。
///
/// 用于「附带图片但当前模型不支持 image」时的自动切换（如用户显式选了纯文本模型，
/// 或自动路由因候选未定价而落空）。排序：默认模型优先 → 已定价者按单价合计升序
/// （未定价模型排后）→ 上下文窗口降序。排除主模型本身；无候选返回 None。
pub fn pick_vision_fallback(
    conn: &Connection,
    provider_id: &str,
    exclude_model: &str,
) -> Option<String> {
    let mut stmt = conn
        .prepare(
            "SELECT model_id, input_price_per_mtok, output_price_per_mtok,
                    context_limit, is_default, input_modalities
             FROM models
             WHERE provider_id = ?1 AND enabled = 1 AND model_id != ?2",
        )
        .ok()?;
    let rows = stmt
        .query_map(rusqlite::params![provider_id, exclude_model], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, f64>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, i64>(4)? != 0,
                r.get::<_, String>(5)?,
            ))
        })
        .ok()?;
    let mut cands: Vec<(String, f64, f64, Option<i64>, bool)> = Vec::new();
    for row in rows.flatten() {
        if !modalities_include(&row.5, "image") {
            continue;
        }
        cands.push((row.0, row.1, row.2, row.3, row.4));
    }
    if cands.is_empty() {
        return None;
    }
    // 默认模型优先；已定价（单价合计 > 0）按价格升序，未定价模型排后按上下文窗口降序
    cands.sort_by(|a, b| {
        let ap = a.1 + a.2;
        let bp = b.1 + b.2;
        b.4.cmp(&a.4).then_with(|| match (ap > 0.0, bp > 0.0) {
            (true, true) => ap.partial_cmp(&bp).unwrap_or(std::cmp::Ordering::Equal),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (false, false) => b.3.unwrap_or(0).cmp(&a.3.unwrap_or(0)),
        })
    });
    cands.first().map(|c| c.0.clone())
}

/// 判断 input_modalities JSON 数组字符串是否包含某模态。
fn modalities_include(json: &str, want: &str) -> bool {
    // 容错：直接子串匹配 "image" / "text"，足以应对 ["text","image"] 这类序列化结果；
    // 对历史纯文本值 "text" 也成立。
    json.contains(&format!("\"{want}\"")) || json == want
}

/// 按输出模态挑选生成模型（图片/视频/音频）：同 Provider 内 enabled=1 且
/// output_modalities 含目标模态的候选。排序：默认模型优先 → 已定价（单价合计 > 0）
/// 按单价升序 → 未定价模型排后按上下文窗口降序。无候选返回 None（不要求已定价，
/// 同 pick_vision_fallback 容错风格）。
pub fn pick_model_for_output(
    conn: &Connection,
    provider_id: &str,
    modality: &str,
) -> Option<String> {
    let mut stmt = conn
        .prepare(
            "SELECT model_id, input_price_per_mtok, output_price_per_mtok,
                    context_limit, is_default, output_modalities
             FROM models
             WHERE provider_id = ?1 AND enabled = 1",
        )
        .ok()?;
    let rows = stmt
        .query_map(rusqlite::params![provider_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, f64>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, i64>(4)? != 0,
                r.get::<_, String>(5)?,
            ))
        })
        .ok()?;
    let mut cands: Vec<(String, f64, f64, Option<i64>, bool)> = Vec::new();
    for row in rows.flatten() {
        if !modalities_include(&row.5, modality) {
            continue;
        }
        cands.push((row.0, row.1, row.2, row.3, row.4));
    }
    if cands.is_empty() {
        return None;
    }
    // 默认模型优先；已定价（单价合计 > 0）按价格升序，未定价模型排后按上下文窗口降序
    cands.sort_by(|a, b| {
        let ap = a.1 + a.2;
        let bp = b.1 + b.2;
        b.4.cmp(&a.4).then_with(|| match (ap > 0.0, bp > 0.0) {
            (true, true) => ap.partial_cmp(&bp).unwrap_or(std::cmp::Ordering::Equal),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (false, false) => b.3.unwrap_or(0).cmp(&a.3.unwrap_or(0)),
        })
    });
    cands.first().map(|c| c.0.clone())
}

/// 同 Provider 中比主模型更便宜的可用模型：返回最便宜的一个（按输入+输出单价合计）。
/// 约束：enabled = 1、支持工具调用（tool_call = 1）、排除主模型本身。
/// 无更便宜候选时返回 None（调用方回退主模型）。
pub fn pick_economy_model(
    conn: &Connection,
    provider_id: &str,
    main_model: &str,
) -> Option<String> {
    // 1. 主模型价格（未知价格时不做路由，避免误判）
    let main: PriceRow = conn
        .query_row(
            "SELECT model_id, input_price_per_mtok, output_price_per_mtok
             FROM models WHERE provider_id = ?1 AND model_id = ?2",
            rusqlite::params![provider_id, main_model],
            |r| {
                Ok(PriceRow {
                    model_id: r.get(0)?,
                    input_price: r.get(1)?,
                    output_price: r.get(2)?,
                })
            },
        )
        .ok()?;
    let main_total = main.input_price + main.output_price;

    // 2. 全部候选按价格升序，取最便宜的；只有严格更便宜才路由
    let mut stmt = conn
        .prepare(
            "SELECT model_id, input_price_per_mtok, output_price_per_mtok
             FROM models
             WHERE provider_id = ?1 AND enabled = 1 AND tool_call = 1 AND model_id != ?2
             ORDER BY (input_price_per_mtok + output_price_per_mtok) ASC, is_default DESC
             LIMIT 1",
        )
        .ok()?;
    let cand: PriceRow = stmt
        .query_row(rusqlite::params![provider_id, main_model], |r| {
            Ok(PriceRow {
                model_id: r.get(0)?,
                input_price: r.get(1)?,
                output_price: r.get(2)?,
            })
        })
        .ok()?;

    let cand_total = cand.input_price + cand.output_price;
    // 价格相同或未定价（0+0）不切换，避免无意义跳转
    if cand_total > 0.0 && cand_total < main_total {
        Some(cand.model_id)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// 内存库 + models 表（与 001_initial.sql 同构的必需列）
    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE models (
                id TEXT PRIMARY KEY,
                provider_id TEXT NOT NULL,
                model_id TEXT NOT NULL,
                tool_call INTEGER NOT NULL DEFAULT 1,
                context_limit INTEGER NOT NULL DEFAULT 8192,
                input_price_per_mtok REAL DEFAULT 0,
                output_price_per_mtok REAL DEFAULT 0,
                input_modalities TEXT DEFAULT '[\"text\"]',
                output_modalities TEXT DEFAULT '[\"text\"]',
                is_default INTEGER NOT NULL DEFAULT 0,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL DEFAULT 0
            );",
        )
        .unwrap();
        conn
    }

    fn insert(conn: &Connection, id: &str, provider: &str, model: &str, tool: i64, in_p: f64, out_p: f64, enabled: i64) {
        conn.execute(
            "INSERT INTO models (id, provider_id, model_id, tool_call, input_price_per_mtok, output_price_per_mtok, enabled)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![id, provider, model, tool, in_p, out_p, enabled],
        )
        .unwrap();
    }

    fn insert_full(
        conn: &Connection,
        id: &str,
        provider: &str,
        model: &str,
        tool: i64,
        ctx: i64,
        in_p: f64,
        out_p: f64,
        modalities: &str,
    ) {
        conn.execute(
            "INSERT INTO models (id, provider_id, model_id, tool_call, context_limit, input_price_per_mtok, output_price_per_mtok, input_modalities, enabled)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,1)",
            rusqlite::params![id, provider, model, tool, ctx, in_p, out_p, modalities],
        )
        .unwrap();
    }

    #[test]
    fn test_picks_cheapest_model() {
        let conn = setup();
        insert(&conn, "1", "p1", "premium", 1, 30.0, 60.0, 1);
        insert(&conn, "2", "p1", "cheap", 1, 2.0, 8.0, 1);
        insert(&conn, "3", "p1", "mid", 1, 10.0, 20.0, 1);
        assert_eq!(pick_economy_model(&conn, "p1", "premium").as_deref(), Some("cheap"));
    }

    #[test]
    fn test_no_cheaper_returns_none() {
        let conn = setup();
        insert(&conn, "1", "p1", "cheap", 1, 2.0, 8.0, 1);
        insert(&conn, "2", "p1", "premium", 1, 30.0, 60.0, 1);
        assert_eq!(pick_economy_model(&conn, "p1", "cheap"), None);
    }

    #[test]
    fn test_ignores_disabled_and_no_tool_models() {
        let conn = setup();
        insert(&conn, "1", "p1", "premium", 1, 30.0, 60.0, 1);
        // 更便宜但禁用 / 不支持工具调用：不可选
        insert(&conn, "2", "p1", "disabled", 1, 0.1, 0.1, 0);
        insert(&conn, "3", "p1", "no_tool", 0, 0.1, 0.1, 1);
        assert_eq!(pick_economy_model(&conn, "p1", "premium"), None);
    }

    #[test]
    fn test_unknown_main_model_returns_none() {
        let conn = setup();
        insert(&conn, "1", "p1", "cheap", 1, 2.0, 8.0, 1);
        assert_eq!(pick_economy_model(&conn, "p1", "ghost"), None);
    }

    #[test]
    fn test_other_provider_ignored() {
        let conn = setup();
        insert(&conn, "1", "p1", "main", 1, 30.0, 60.0, 1);
        insert(&conn, "2", "p2", "cheap_elsewhere", 1, 0.1, 0.1, 1);
        assert_eq!(pick_economy_model(&conn, "p1", "main"), None);
    }

    #[test]
    fn test_pick_vision_fallback_prefers_default_then_cheapest() {
        let conn = setup();
        // 默认模型：priced_default（默认 + 有价格）；候选有更便宜的 vision 模型
        insert_full(&conn, "1", "p1", "main", 1, 8192, 30.0, 60.0, "[\"text\"]");
        insert_full(&conn, "2", "p1", "vision_cheap", 0, 8192, 1.0, 2.0, "[\"text\",\"image\"]");
        insert_full(&conn, "3", "p1", "vision_default", 1, 8192, 5.0, 10.0, "[\"text\",\"image\"]");
        assert_eq!(
            pick_vision_fallback(&conn, "p1", "main").as_deref(),
            Some("vision_cheap")
        );
    }

    #[test]
    fn test_pick_vision_fallback_requires_image_modality() {
        let conn = setup();
        insert_full(&conn, "1", "p1", "main", 1, 8192, 30.0, 60.0, "[\"text\"]");
        // 唯一候选不支持 image：返回 None
        insert_full(&conn, "2", "p1", "text_only", 1, 8192, 1.0, 2.0, "[\"text\"]");
        assert_eq!(pick_vision_fallback(&conn, "p1", "main"), None);
    }

    #[test]
    fn test_pick_vision_fallback_unpriced_by_context() {
        let conn = setup();
        insert_full(&conn, "1", "p1", "main", 1, 8192, 30.0, 60.0, "[\"text\"]");
        // 未定价（0+0）的 vision 模型：仍可被选中（模板添加的模型常无价格），大上下文优先
        insert_full(&conn, "2", "p1", "vision_small", 1, 8192, 0.0, 0.0, "[\"text\",\"image\"]");
        insert_full(&conn, "3", "p1", "vision_big", 1, 128_000, 0.0, 0.0, "[\"text\",\"image\"]");
        assert_eq!(
            pick_vision_fallback(&conn, "p1", "main").as_deref(),
            Some("vision_big")
        );
    }

    #[test]
    fn test_pick_vision_fallback_ignores_disabled_and_other_provider() {
        let conn = setup();
        insert_full(&conn, "1", "p1", "main", 1, 8192, 30.0, 60.0, "[\"text\"]");
        // 禁用 / 其他 Provider：不可选
        insert_full(&conn, "2", "p1", "vision_disabled", 1, 8192, 1.0, 2.0, "[\"text\",\"image\"]");
        conn.execute(
            "UPDATE models SET enabled = 0 WHERE id = '2'",
            [],
        )
        .unwrap();
        insert_full(&conn, "3", "p2", "vision_elsewhere", 1, 8192, 1.0, 2.0, "[\"text\",\"image\"]");
        assert_eq!(pick_vision_fallback(&conn, "p1", "main"), None);
    }

    #[test]
    fn test_classify_task() {
        assert_eq!(classify_task("你好", false, false), TaskKind::Chat);
        assert_eq!(classify_task("帮我写一个函数", false, false), TaskKind::Code);
        assert_eq!(classify_task("hvigor 构建报错", false, false), TaskKind::Code);
        assert_eq!(classify_task("", true, false), TaskKind::Vision);
        assert_eq!(classify_task("随便", false, true), TaskKind::Fast);
    }

    #[test]
    fn test_vision_requires_image_modality() {
        let conn = setup();
        insert_full(&conn, "1", "p1", "main", 1, 8192, 30.0, 60.0, "[\"text\"]");
        insert_full(&conn, "2", "p1", "vision", 0, 8192, 5.0, 10.0, "[\"text\",\"image\"]");
        insert_full(&conn, "3", "p1", "text_only", 1, 8192, 1.0, 2.0, "[\"text\"]");
        assert_eq!(
            pick_model_for_task(&conn, "p1", "main", TaskKind::Vision).as_deref(),
            Some("vision")
        );
    }

    #[test]
    fn test_code_prefers_larger_context() {
        let conn = setup();
        insert_full(&conn, "1", "p1", "main", 1, 32_768, 30.0, 60.0, "[\"text\"]");
        insert_full(&conn, "2", "p1", "big_ctx", 1, 128_000, 40.0, 80.0, "[\"text\"]");
        insert_full(&conn, "3", "p1", "small_ctx", 1, 8192, 1.0, 2.0, "[\"text\"]");
        // 选上下文最大且不小于主模型的 big_ctx，而非最便宜的 small_ctx
        assert_eq!(
            pick_model_for_task(&conn, "p1", "main", TaskKind::Code).as_deref(),
            Some("big_ctx")
        );
    }

    #[test]
    fn test_chat_picks_cheaper_within_main_price() {
        let conn = setup();
        insert_full(&conn, "1", "p1", "main", 1, 8192, 30.0, 60.0, "[\"text\"]");
        insert_full(&conn, "2", "p1", "cheap", 1, 8192, 2.0, 8.0, "[\"text\"]");
        insert_full(&conn, "3", "p1", "pricier", 1, 8192, 50.0, 90.0, "[\"text\"]");
        assert_eq!(
            pick_model_for_task(&conn, "p1", "main", TaskKind::Chat).as_deref(),
            Some("cheap")
        );
    }

    #[test]
    fn test_fast_picks_cheapest_regardless_of_tool() {
        let conn = setup();
        insert_full(&conn, "1", "p1", "main", 1, 8192, 30.0, 60.0, "[\"text\"]");
        insert_full(&conn, "2", "p1", "no_tool_cheap", 0, 8192, 0.5, 1.0, "[\"text\"]");
        insert_full(&conn, "3", "p1", "tool_mid", 1, 8192, 2.0, 4.0, "[\"text\"]");
        assert_eq!(
            pick_model_for_task(&conn, "p1", "main", TaskKind::Fast).as_deref(),
            Some("no_tool_cheap")
        );
    }
}
