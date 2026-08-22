//! 轻量召回相关性打分（无第三方依赖）。
//!
//! 用途：记忆/知识条目召回时，对候选条目按与查询关键词的相关度打分排序，
//! 替代"只按命中数排序"的粗糙做法。核心成分：
//! - **Okapi BM25**（k1=1.2, b=0.75）：关键词按文档频率/长度归一打分，
//!   对齐 Qwen-Agent keyword_search 的 rank_bm25（BM25Okapi）语义
//! - **位置权重**：标题命中 > 内容命中（标题更凝练地表达条目主题；
//!   BM25 无字段位置概念，用标题双份注入近似 title_weight）
//! - **时间衰减**：近期更新/创建的条目优先（记忆场景：近期经验更贴近当前工程状态）
//! - **类别加权**：特定任务场景下某些类别更重要（如构建失败时 pitfall/build 类）

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

/// Okapi BM25 常量（经典默认值，与 rank_bm25 一致）
pub const BM25_K1: f64 = 1.2;
pub const BM25_B: f64 = 0.75;

/// 打分参数：控制各成分的相对权重
pub struct RankParams {
    /// 标题命中权重倍率（BM25 无字段位置概念，按此倍数重复注入标题近似位置权重）
    pub title_weight: f64,
    /// 时间衰减半衰期（秒）：条目 age 达到该时长时时间分衰减一半
    pub half_life_secs: f64,
    /// 类别加权：任务场景下某些类别额外加分（build 失败场景的 pitfall/build 等）
    pub cat_boost: f64,
}

impl Default for RankParams {
    fn default() -> Self {
        Self {
            title_weight: 2.0,
            half_life_secs: 7.0 * 24.0 * 3600.0, // 7 天半衰期
            cat_boost: 0.0,
        }
    }
}

// ---------- 分词（查询与文档两侧共用，保证 BM25 词袋一致）----------

const STOPS: &[&str] = &[
    "的", "了", "我", "你", "他", "她", "它", "是", "在", "有", "和", "与", "就", "都", "而", "及",
    "或", "个", "这", "那", "一", "不", "要", "会", "能", "也", "很", "把", "被", "从", "到", "对",
    "为", "等", "上", "下", "中", "我们", "你们", "他们", "可以", "需要", "使用", "进行", "一下",
    "什么", "怎么", "如何", "为什么", "请", "帮我", "帮忙", "相关", "问题", "情况", "目前", "现在",
    "这个", "那个", "已经", "还是", "一下",
];

/// 轻量分词：中文 2-4 字滑窗 n-gram（近似 jieba）+ 英文整词切分（小写），
/// 过滤停用词。查询与文档两侧必须共用同一分词器，BM25 的词袋统计才有意义。
pub fn tokenize_query(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cjk = String::new(); // 连续中文段
    let mut ascii = String::new(); // 连续英文/数字段
    for ch in text.chars().take(2000) {
        let cp = ch as u32;
        let is_cjk = (0x4e00..=0x9fff).contains(&cp);
        let is_ascii_word = ch.is_ascii_alphanumeric();
        if is_cjk {
            flush_ascii(&mut ascii, &mut out);
            cjk.push(ch);
        } else if is_ascii_word {
            flush_cjk(&mut cjk, &mut out);
            ascii.push(ch.to_ascii_lowercase());
        } else {
            flush_cjk(&mut cjk, &mut out);
            flush_ascii(&mut ascii, &mut out);
        }
    }
    flush_cjk(&mut cjk, &mut out);
    flush_ascii(&mut ascii, &mut out);
    out
}

/// 中文段：2-4 字滑窗 n-gram，过滤停用词
fn flush_cjk(seg: &mut String, out: &mut Vec<String>) {
    if seg.is_empty() {
        return;
    }
    let chars: Vec<char> = seg.chars().collect();
    let n = chars.len();
    for w in 2..=4.min(n) {
        for i in 0..=(n - w) {
            let gram: String = chars[i..i + w].iter().collect();
            if !STOPS.contains(&gram.as_str()) {
                out.push(gram);
            }
        }
    }
    seg.clear();
}

/// 英文/数字段：整词切分（长度 ≥ 2 且非停用词）
fn flush_ascii(seg: &mut String, out: &mut Vec<String>) {
    if seg.len() >= 2 && !STOPS.contains(&seg.as_str()) {
        out.push(std::mem::take(seg));
    } else {
        seg.clear();
    }
}

// ---------- Okapi BM25 ----------

/// Okapi BM25 索引：对文档集合做词频/文档频率统计，按查询打分。
/// 公式与 rank_bm25 的 BM25Okapi 一致：idf = ln(1 + (N-df+0.5)/(df+0.5))，
/// tf 饱和项 = tf*(k1+1) / (tf + k1*(1 - b + b*dl/avgdl))。
pub struct Bm25Index {
    n_docs: usize,
    avg_dl: f64,
    df: HashMap<String, usize>,
}

impl Bm25Index {
    /// 从文档集合构建索引（docs 为原始文本，内部用 tokenize_query 分词）。
    /// 文档为空时返回空索引（score 恒 0，调用方自然退化）。
    pub fn build(docs: &[String]) -> Self {
        let mut df: HashMap<String, usize> = HashMap::new();
        let mut total_len = 0usize;
        for d in docs {
            let mut seen: HashSet<String> = HashSet::new();
            for t in tokenize_query(d) {
                total_len += 1;
                if seen.insert(t.clone()) {
                    *df.entry(t).or_insert(0) += 1;
                }
            }
        }
        let n = docs.len();
        Self {
            n_docs: n,
            avg_dl: if n == 0 { 0.0 } else { total_len as f64 / n as f64 },
            df,
        }
    }

    /// 查询关键词集合对单篇文档的打分（idf 与长度归一来自索引统计）。
    /// 查询词未出现在任何文档（df=0）时跳过，避免无意义 idf 干扰。
    pub fn score(&self, query: &[String], doc: &str) -> f64 {
        if self.n_docs == 0 {
            return 0.0;
        }
        let mut tf: HashMap<String, usize> = HashMap::new();
        let mut dl = 0usize;
        for t in tokenize_query(doc) {
            dl += 1;
            *tf.entry(t).or_insert(0) += 1;
        }
        if dl == 0 {
            return 0.0;
        }
        let n = self.n_docs as f64;
        let avgdl = self.avg_dl.max(1.0);
        let mut s = 0.0f64;
        for q in query {
            let df = *self.df.get(q).unwrap_or(&0) as f64;
            if df == 0.0 {
                continue;
            }
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
            let tfq = *tf.get(q).unwrap_or(&0) as f64;
            if tfq == 0.0 {
                continue;
            }
            let denom = tfq + BM25_K1 * (1.0 - BM25_B + BM25_B * dl as f64 / avgdl);
            s += idf * tfq * (BM25_K1 + 1.0) / denom;
        }
        s
    }

    /// 对一批文档打分（与 docs 同序返回）
    pub fn score_many(&self, query: &[String], docs: &[String]) -> Vec<f64> {
        docs.iter().map(|d| self.score(query, d)).collect()
    }
}

// ---------- 排序入口 ----------

/// 对候选集合排序取 top N（原地排序，返回排序后的前 n 项下标顺序）。
/// 供调用方按返回的下标取原始候选。分数高者在前，同分保持原顺序（稳定）。
/// 关键词命中部分用 Okapi BM25（标题双份注入近似位置权重），叠加时间衰减与类别加权。
pub fn rank_candidates(
    keywords: &[String],
    candidates: &[(String, String, Option<i64>)], // (title, content, updated_at)
    params: &RankParams,
    top_n: usize,
) -> Vec<usize> {
    if candidates.is_empty() {
        return Vec::new();
    }
    if keywords.is_empty() {
        // 无关键词：调用方通常不走本函数（自行按时间倒序），防御性保序取前 N
        return (0..candidates.len().min(top_n)).collect();
    }
    // 标题按 title_weight 取整重复注入近似位置权重（BM25 无字段位置概念）
    let title_repeat = (params.title_weight.max(1.0).round() as usize).max(1);
    let docs: Vec<String> = candidates
        .iter()
        .map(|(t, c, _)| {
            let title_part = std::iter::repeat_n(t.as_str(), title_repeat).collect::<Vec<_>>().join(" ");
            format!("{title_part} {c}")
        })
        .collect();
    let idx = Bm25Index::build(&docs);
    let mut scored: Vec<(f64, usize)> = candidates
        .iter()
        .enumerate()
        .map(|(i, (_, _, u))| {
            let mut s = idx.score(keywords, &docs[i]);
            // 时间衰减：越新越高（0~1），与 BM25 分叠加
            if let Some(ts) = u {
                let age = (now_secs() - ts).max(0) as f64;
                s += (-age / params.half_life_secs).exp() * 0.5;
            } else {
                s += 0.5;
            }
            (s + params.cat_boost, i)
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(top_n).map(|(_, i)| i).collect()
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 旧 TF 单条路径（仅测试保留）----------

    /// 单条条目 TF 加权打分：标题命中 > 内容命中（title_weight），词长 4 字满权重，
    /// 叠加时间衰减与类别加权。生产集合排序走 rank_candidates（BM25），此处仅测试保留。
    fn rank_entry(
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
            let len_w = (kw.chars().count() as f64 / 4.0).min(1.0);
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
            let now = now_secs();
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

    #[test]
    fn tokenize_splits_cjk_and_ascii() {
        // 中文滑窗 + 英文整词 + 停用词过滤
        let t = tokenize_query("请修复 build 错误");
        assert!(t.contains(&"build".to_string()), "{t:?}");
        assert!(t.contains(&"修复".to_string()), "{t:?}");
        assert!(t.contains(&"错误".to_string()), "{t:?}");
        assert!(t.contains(&"请修".to_string()), "{t:?}");
        assert!(!t.contains(&"请".to_string()), "单字/停用词应过滤: {t:?}");
        // 英文小写化
        assert!(tokenize_query("API Level").contains(&"api".to_string()));
        assert!(tokenize_query("API Level").contains(&"level".to_string()));
    }

    #[test]
    fn bm25_tf_monotonic_and_length_normalized() {
        // 同长度量级下 tf 高者分高（BM25 tf 饱和但单调）
        let docs = vec![
            "构建 构建 构建".to_string(),
            "构建".to_string(),
        ];
        let idx = Bm25Index::build(&docs);
        let q = vec!["构建".to_string()];
        let more = idx.score(&q, &docs[0]);
        let one = idx.score(&q, &docs[1]);
        assert!(more > 0.0 && one > 0.0);
        assert!(more > one, "tf 高者分高：more={more} one={one}");
        // 长度归一：同等 tf 下短文档应获得更高分（b=0.75 惩罚长文档）
        let docs2 = vec![
            "构建 填充词 填充词 填充词 填充词".to_string(),
            "构建".to_string(),
        ];
        let idx2 = Bm25Index::build(&docs2);
        let s_long = idx2.score(&q, &docs2[0]);
        let s_short = idx2.score(&q, &docs2[1]);
        assert!(s_short > s_long, "短文档应受长度归一优待：long={s_long} short={s_short}");
    }

    #[test]
    fn bm25_rare_terms_weigh_more() {
        // 词 A 只在一篇文档出现（稀有，idf 高）；词 B 两篇都有（普遍，idf 低）
        let docs = vec![
            "独有词 常见词".to_string(),
            "常见词".to_string(),
        ];
        let idx = Bm25Index::build(&docs);
        let qa = vec!["独有词".to_string()];
        let qb = vec!["常见词".to_string()];
        let sa_doc0 = idx.score(&qa, &docs[0]);
        let sb_doc0 = idx.score(&qb, &docs[0]);
        let sb_doc1 = idx.score(&qb, &docs[1]);
        assert!(sa_doc0 > 0.0);
        assert!(sb_doc0 > 0.0 && sb_doc1 > 0.0);
        // 稀有词 idf 应大于常见词（同 tf 下）
        assert!(sa_doc0 > sb_doc0, "稀有词分应更高：rare={sa_doc0} common={sb_doc0}");
    }

    #[test]
    fn rank_candidates_prefers_title_hits() {
        // 标题命中（双份注入）应排内容命中之前
        let kw = vec!["加密".to_string()];
        let cands = vec![
            ("无关标题".to_string(), "这里提到加密".to_string(), None),
            ("数据加密".to_string(), "无关内容".to_string(), None),
        ];
        let idxs = rank_candidates(&kw, &cands, &RankParams::default(), 2);
        assert_eq!(idxs[0], 1, "标题命中应排前：{idxs:?}");
    }

    #[test]
    fn rank_candidates_keeps_recency_and_cat_boost() {
        let kw = vec!["构建".to_string()];
        let now = now_secs();
        let cands = vec![
            ("构建".to_string(), "旧内容".to_string(), Some(now - 30 * 24 * 3600)),
            ("构建".to_string(), "新内容".to_string(), Some(now)),
        ];
        let idxs = rank_candidates(&kw, &cands, &RankParams::default(), 2);
        assert_eq!(idxs[0], 1, "时间衰减应让新条目排前：{idxs:?}");
        // 类别加权：旧条目被 boost 后反超
        let p = RankParams { cat_boost: 5.0, ..Default::default() };
        // 类别加权对全部候选统一加分（此处验证不报错且仍稳定返回 top N）
        let idxs2 = rank_candidates(&kw, &cands, &p, 2);
        assert_eq!(idxs2.len(), 2);
    }

    #[test]
    fn rank_candidates_empty_and_no_keyword() {
        let cands = vec![("a".to_string(), "b".to_string(), None)];
        assert!(rank_candidates(&["构建".to_string()], &[], &RankParams::default(), 5).is_empty());
        assert_eq!(
            rank_candidates(&[], &cands, &RankParams::default(), 5),
            vec![0],
            "无关键词保序取前 N"
        );
    }

    #[test]
    fn title_hits_weigh_more_than_content() {
        // 旧 TF 路径保留测试（rank_entry 单条语义不变）
        let kw = vec!["加密".to_string()];
        let p = RankParams::default();
        let a = rank_entry(&kw, "数据加密", "无关内容", None, &p);
        let b = rank_entry(&kw, "无关标题", "这里提到加密", None, &p);
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
        let now = now_secs();
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
