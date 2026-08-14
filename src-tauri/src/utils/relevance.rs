//! 轻量召回相关性打分（无第三方依赖）。
//!
//! 用途：记忆/知识条目召回时，对候选条目按与查询关键词的相关度打分排序，
//! 替代"只按命中数排序"的粗糙做法。核心成分：
//! - **TF 加权**：关键词在字段中出现次数越多分越高（而非仅"是否命中"）
//! - **位置权重**：标题命中 > 内容命中（标题更凝练地表达条目主题）
//! - **词长加权**：越长的词（如 4 字词）特异性越高，得分权重越大
//! - **时间衰减**：近期更新/创建的条目优先（记忆场景：近期经验更贴近当前工程状态）
//! - **类别加权**：特定任务场景下某些类别更重要（如构建失败时 pitfall/build 类）

use std::time::{SystemTime, UNIX_EPOCH};

/// 打分参数：控制各成分的相对权重
pub struct RankParams {
    /// 标题命中权重倍率（相对内容命中 1.0）
    pub title_weight: f64,
    /// 时间衰减半衰期（秒）：条目 age 达到该时长时时间分衰减一半
    pub half_life_secs: f64,
    /// 关键词长度权重：词长达到该值按满权重，短词降权
    pub max_len_weight: usize,
    /// 类别加权：任务场景下某些类别额外加分（build 失败场景的 pitfall/build 等）
    pub cat_boost: f64,
}

impl Default for RankParams {
    fn default() -> Self {
        Self {
            title_weight: 2.0,
            half_life_secs: 7.0 * 24.0 * 3600.0, // 7 天半衰期
            max_len_weight: 4,
            cat_boost: 0.0,
        }
    }
}

/// 对单个条目打分。keywords 为查询关键词；title/content 为条目字段；
/// updated_at 为条目最近更新时间（unix 秒），None 表示未知（时间分取满）。
pub fn rank_entry(
    keywords: &[String],
    title: &str,
    content: &str,
    updated_at: Option<i64>,
    params: &RankParams,
) -> f64 {
    let mut score = 0.0f64;
    for kw in keywords {
        if kw.is_empty() {
            continue;
        }
        // 词长权重：4 字词满权重，2 字词约半权重
        let len_w = (kw.chars().count() as f64 / params.max_len_weight as f64).min(1.0);
        // 标题命中（TF + 位置权重）
        let t_hits = count_occurrences(title, kw);
        if t_hits > 0 {
            score += t_hits as f64 * params.title_weight * (0.5 + 0.5 * len_w);
        }
        // 内容命中（TF，权重 1.0）
        let c_hits = count_occurrences(content, kw);
        if c_hits > 0 {
            score += c_hits as f64 * (0.5 + 0.5 * len_w);
        }
    }
    // 时间衰减：越新越高（0~1）
    if let Some(ts) = updated_at {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let age = (now - ts).max(0) as f64;
        let decay = (-age / params.half_life_secs).exp();
        score += decay * 0.5;
    } else {
        score += 0.5;
    }
    // 类别加权
    score + params.cat_boost
}

/// 统计子串出现次数（朴素的字节级匹配；文本量小，性能足够）
fn count_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() || haystack.len() < needle.len() {
        return 0;
    }
    let mut count = 0;
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        count += 1;
        start += pos + needle.len();
    }
    count
}

/// 对候选集合排序取 top N（原地排序，返回排序后的前 n 项下标顺序）。
/// 供调用方按返回的下标取原始候选。分数高者在前，同分保持原顺序（稳定）。
pub fn rank_candidates(
    keywords: &[String],
    candidates: &[(String, String, Option<i64>)], // (title, content, updated_at)
    params: &RankParams,
    top_n: usize,
) -> Vec<usize> {
    let mut scored: Vec<(f64, usize)> = candidates
        .iter()
        .enumerate()
        .map(|(i, (t, c, u))| (rank_entry(keywords, t, c, *u, params), i))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(top_n).map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_hits_weigh_more_than_content() {
        let kw = vec!["加密".to_string()];
        let p = RankParams::default();
        let a = rank_entry(&kw, "数据加密", "无关内容", None, &p); // 标题命中 1 次
        let b = rank_entry(&kw, "无关标题", "这里提到加密", None, &p); // 内容命中 1 次
        assert!(a > b, "标题命中应高于内容命中：a={a} b={b}");
    }

    #[test]
    fn tf_matters() {
        let kw = vec!["构建".to_string()];
        let p = RankParams::default();
        let once = rank_entry(&kw, "x", "构建失败的处理方法", None, &p);
        let three = rank_entry(&kw, "x", "构建失败 构建超时 构建错误处理", None, &p);
        assert!(three > once, "关键词出现次数多者分高：once={once} three={three}");
    }

    #[test]
    fn recency_boosts() {
        let kw: Vec<String> = Vec::new();
        let p = RankParams::default();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let fresh = rank_entry(&kw, "t", "c", Some(now), &p);
        let old = rank_entry(&kw, "t", "c", Some(now - 30 * 24 * 3600), &p);
        assert!(fresh > old, "新条目应更优先：fresh={fresh} old={old}");
    }

    #[test]
    fn cat_boost_adds() {
        let kw: Vec<String> = Vec::new();
        let p = RankParams { cat_boost: 3.0, ..Default::default() };
        let boosted = rank_entry(&kw, "t", "c", None, &p);
        let p2 = RankParams::default();
        let normal = rank_entry(&kw, "t", "c", None, &p2);
        assert!(boosted > normal);
    }
}
