//! ohpm 三方库推荐缓存 —— 官方 landscape 推荐区（开源技术图谱）的本地镜像。
//!
//! 数据源：`https://ohpm.openharmony.cn/ohpm/tech-map/ide-page`
//! （ohpm 官网 landscape 页面调用的 IDE 版接口：免登录、无鉴权，一次请求返回全量 1000+ 包。）
//! 每次刷新全量替换（量小无需 diff）；包字段含四级中英文分类 / 描述 / 关键词 /
//! 60 天下载量 / 评分，支持离线检索与按热度/分类推荐。

use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::utils::net::build_client_auto;

/// landscape 官网前端调用的数据接口（IDE 版：扁平包列表）
const API_URL: &str = "https://ohpm.openharmony.cn/ohpm/tech-map/ide-page";

/// 官网搜索接口（支持 sortedType=likes/popularity/latest，全库约 3500 包）
/// 用于补齐点赞数 / 流行度 / 最新发布时间三项官方排序指标。
const SEARCH_API: &str = "https://ohpm.openharmony.cn/ohpmweb/registry/oh-package/openapi/v1/search";

/// search 接口单页上限（实测 150+ 报 217005）
const SEARCH_PAGE_SIZE: usize = 100;

/// 单个三方库条目（ide-page 返回字段，camelCase 反序列化 / snake_case 序列化）
///
/// 注意：`rename(deserialize = ...)` 只影响反序列化；序列化时输出 Rust 字段名
/// （snake_case），保证前端与 Agent 工具拿到一致的 snake_case JSON。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OhpmPkg {
    #[serde(rename(deserialize = "packageName"))]
    pub package_name: String,
    #[serde(rename(deserialize = "version"))]
    pub version: String,
    #[serde(rename(deserialize = "authorName"), default)]
    pub author_name: String,
    #[serde(rename(deserialize = "score"), default)]
    pub score: i64,
    #[serde(rename(deserialize = "license"), default)]
    pub license: String,
    #[serde(rename(deserialize = "downLoadCount60days"), default)]
    pub down_count_60d: i64,
    #[serde(rename(deserialize = "description"), default)]
    pub description: String,
    #[serde(rename(deserialize = "keywords"), default)]
    pub keywords: String,
    #[serde(rename(deserialize = "fileNums"), default)]
    pub file_nums: i64,
    #[serde(rename(deserialize = "fileSize"), default)]
    pub file_size: i64,
    #[serde(rename(deserialize = "levelOneCategoryCn"), default)]
    pub level1_cn: String,
    #[serde(rename(deserialize = "levelOneCategoryEn"), default)]
    pub level1_en: String,
    #[serde(rename(deserialize = "levelTwoCategoryCn"), default)]
    pub level2_cn: String,
    #[serde(rename(deserialize = "levelTwoCategoryEn"), default)]
    pub level2_en: String,
    #[serde(rename(deserialize = "levelThreeCategoryCn"), default)]
    pub level3_cn: String,
    #[serde(rename(deserialize = "levelThreeCategoryEn"), default)]
    pub level3_en: String,
    #[serde(rename(deserialize = "levelFourCategoryCn"), default)]
    pub level4_cn: String,
    #[serde(rename(deserialize = "levelFourCategoryEn"), default)]
    pub level4_en: String,
    /// 点赞数（最受欢迎排序，来自官网搜索接口）
    #[serde(rename(deserialize = "likes"), default)]
    pub likes: i64,
    /// 流行度（最流行排序，来自官网搜索接口）
    #[serde(rename(deserialize = "popularity"), default)]
    pub popularity: i64,
    /// 最新发布时间（毫秒时间戳，最新发布排序，来自官网搜索接口）
    #[serde(rename(deserialize = "latestPublishTime"), default)]
    pub latest_publish_time: i64,
}

impl OhpmPkg {
    /// ohpm 官网详情页 URL（`#/cn/detail/<包名>`，`/` 编码为 `%2F`）
    pub fn detail_url(&self) -> String {
        format!(
            "https://ohpm.openharmony.cn/#/cn/detail/{}",
            self.package_name.replace('/', "%2F")
        )
    }

    /// 一级分类（中文优先）
    pub fn level1(&self) -> &str {
        if !self.level1_cn.is_empty() {
            &self.level1_cn
        } else {
            &self.level1_en
        }
    }

    /// 完整分类路径（如「网络通信 / 应用页面路由 / 动态路由」）
    pub fn category_path(&self) -> String {
        [
            self.level1_cn.as_str(),
            self.level2_cn.as_str(),
            self.level3_cn.as_str(),
            self.level4_cn.as_str(),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" / ")
    }
}

/// 官网搜索接口返回的单包排序指标（按包名 join 到 landscape 精选集）
#[derive(Debug, Clone, Deserialize)]
struct SearchRow {
    #[serde(rename(deserialize = "name"))]
    pub package_name: String,
    #[serde(rename(deserialize = "likes"), default)]
    pub likes: i64,
    #[serde(rename(deserialize = "popularity"), default)]
    pub popularity: i64,
    #[serde(rename(deserialize = "latestPublishTime"), default)]
    pub latest_publish_time: i64,
}

/// 搜索接口响应（body.rows 为包列表）
#[derive(Debug, Deserialize)]
struct SearchResponse {
    body: SearchBody,
}

#[derive(Debug, Deserialize)]
struct SearchBody {
    #[serde(default)]
    rows: Vec<SearchRow>,
    #[serde(default)]
    total: usize,
}

/// 刷新报告
#[derive(Debug, Clone, Serialize)]
pub struct RefreshReport {
    pub total: usize,
    pub updated_at: i64,
}

/// 缓存状态（前端展示用）
#[derive(Debug, Clone, Serialize)]
pub struct OhpmStatus {
    pub total: i64,
    pub updated_at: Option<i64>,
    /// 一级分类数量
    pub categories: i64,
}

/// 一级分类统计（含二级子分类，供分类浏览）
#[derive(Debug, Clone, Serialize)]
pub struct CategoryStat {
    pub name_cn: String,
    pub name_en: String,
    pub count: i64,
    pub children: Vec<CategoryStat>,
}

// ───────────────────────── 拉取与入库 ─────────────────────────

/// 拉取远端全量包列表（无鉴权）
pub async fn fetch_remote() -> Result<Vec<OhpmPkg>, String> {
    let client = build_client_auto()?;
    let resp = client
        .get(API_URL)
        .send()
        .await
        .map_err(|e| format!("请求 ohpm landscape 接口失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("ohpm landscape 接口返回 {}", resp.status()));
    }
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取 ohpm landscape 响应失败：{e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("解析 ohpm landscape 响应失败：{e}"))?;
    let arr = v["data"]["ohpmIdePkgCategories"]
        .as_array()
        .ok_or_else(|| "响应缺少 data.ohpmIdePkgCategories 字段".to_string())?;
    let pkgs: Vec<OhpmPkg> = serde_json::from_value(serde_json::Value::Array(arr.clone()))
        .map_err(|e| format!("解析包列表失败：{e}"))?;
    if pkgs.is_empty() {
        return Err("ohpm landscape 返回空包列表（可能接口结构变化）".into());
    }
    Ok(pkgs)
}

/// 并发拉取官网搜索接口全量排序指标（sortedType=likes 分页，全库约 3500 包）
/// 返回 包名 → 排序指标 映射，供 refresh 与 landscape 精选集按包名 join。
/// 任一页失败仅记日志跳过（排序指标是增强数据，不阻断刷新主流程）。
async fn fetch_search_meta(client: &reqwest::Client) -> std::collections::HashMap<String, SearchRow> {
    use std::collections::HashMap;

    let mut map = HashMap::new();
    // 先取第一页拿 total，再并发拉剩余页
    let first: SearchResponse = match (async {
        let resp = client
            .get(SEARCH_API)
            .query(&[
                ("condition", ""),
                ("pageNum", "1"),
                ("pageSize", SEARCH_PAGE_SIZE.to_string().as_str()),
                ("sortedType", "likes"),
                ("isHomePage", "false"),
            ])
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?;
        resp.json().await.map_err(|e| e.to_string())
    })
    .await
    {
        Ok(v) => v,
        Err(e) => {
            eprintln!("ohpm 搜索接口首页失败，跳过排序指标：{e}");
            return map;
        }
    };
    for row in &first.body.rows {
        map.insert(row.package_name.clone(), row.clone());
    }
    let total = first.body.total;
    if total <= SEARCH_PAGE_SIZE {
        return map;
    }
    let pages = total.div_ceil(SEARCH_PAGE_SIZE);
    let mut tasks = Vec::new();
    for page in 2..=pages {
        let client = client.clone();
        tasks.push(tokio::spawn(async move {
            let resp: Result<SearchResponse, String> = (async {
                let resp = client
                    .get(SEARCH_API)
                    .query(&[
                        ("condition", ""),
                        ("pageNum", &page.to_string()),
                        ("pageSize", &SEARCH_PAGE_SIZE.to_string()),
                        ("sortedType", "likes"),
                        ("isHomePage", "false"),
                    ])
                    .send()
                    .await
                    .map_err(|e| e.to_string())?
                    .error_for_status()
                    .map_err(|e| e.to_string())?;
                resp.json().await.map_err(|e| e.to_string())
            })
            .await;
            resp
        }));
    }
    for task in tasks {
        match task.await {
            Ok(Ok(resp)) => {
                for row in resp.body.rows {
                    map.insert(row.package_name.clone(), row.clone());
                }
            }
            Ok(Err(e)) => eprintln!("ohpm 搜索接口部分页失败，跳过对应排序指标：{e}"),
            Err(e) => eprintln!("ohpm 搜索并发任务失败：{e}"),
        }
    }
    map
}

/// 拉取远端并全量替换本地缓存（单事务）
/// 主数据源 landscape 精选集（含分类/下载量）+ 官网搜索接口补齐点赞/流行度/发布时间。
pub async fn refresh(db: &Arc<Mutex<Connection>>) -> Result<RefreshReport, String> {
    let pkgs = fetch_remote().await?;
    let client = build_client_auto()?;
    let meta = fetch_search_meta(&client).await;
    let now = chrono::Utc::now().timestamp();
    {
        let mut conn = db.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM ohpm_landscape", [])
            .map_err(|e| e.to_string())?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR REPLACE INTO ohpm_landscape (
                        package_name, version, author_name, score, license, down_count_60d,
                        description, keywords, file_nums, file_size,
                        level1_cn, level1_en, level2_cn, level2_en,
                        level3_cn, level3_en, level4_cn, level4_en,
                        likes, popularity, latest_publish_time, updated_at
                     ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)",
                )
                .map_err(|e| e.to_string())?;
            for p in &pkgs {
                let m = meta.get(&p.package_name);
                stmt.execute(params![
                    p.package_name, p.version, p.author_name, p.score, p.license,
                    p.down_count_60d, p.description, p.keywords, p.file_nums, p.file_size,
                    p.level1_cn, p.level1_en, p.level2_cn, p.level2_en,
                    p.level3_cn, p.level3_en, p.level4_cn, p.level4_en,
                    m.map(|x| x.likes).unwrap_or(0),
                    m.map(|x| x.popularity).unwrap_or(0),
                    m.map(|x| x.latest_publish_time).unwrap_or(0),
                    now,
                ])
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    Ok(RefreshReport {
        total: pkgs.len(),
        updated_at: now,
    })
}

// ───────────────────────── 仓库地址查询（按需） ─────────────────────────

/// ohpm registry 元数据基址（npm 风格 Fetch Metadata：`/ohpm/@group/pkg`）
const REGISTRY_BASE: &str = "https://ohpm.openharmony.cn/ohpm";

/// 从 registry 元数据中提取仓库主页 URL
/// （repository 字段可能是字符串，也可能是 npm 风格 { type, url } 对象）
pub fn extract_repo_from_metadata(v: &serde_json::Value) -> Option<String> {
    let versions = v["versions"].as_object()?;
    let latest = v["dist-tags"]["latest"].as_str();
    let ver = match latest {
        Some(tag) => versions.get(tag),
        None => versions.values().next_back(),
    }?;
    let repo = &ver["repository"];
    let raw = match repo {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(m) => m.get("url")?.as_str()?.to_string(),
        _ => return None,
    };
    let raw = raw.trim().to_string();
    if raw.is_empty() {
        None
    } else {
        Some(raw)
    }
}

/// 查询指定包的最新版元数据，返回仓库主页 URL（无仓库或查询失败返回 None）
pub async fn repo_url(package_name: &str) -> Result<Option<String>, String> {
    let client = build_client_auto()?;
    let url = format!("{REGISTRY_BASE}/{package_name}");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("查询 {package_name} 元数据失败：{e}"))?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析 {package_name} 元数据失败：{e}"))?;
    Ok(extract_repo_from_metadata(&v))
}

// ───────────────────────── 查询 ─────────────────────────

const PKG_COLS: &str = "package_name, version, author_name, score, license, down_count_60d,
        description, keywords, file_nums, file_size,
        level1_cn, level1_en, level2_cn, level2_en,
        level3_cn, level3_en, level4_cn, level4_en,
        likes, popularity, latest_publish_time, updated_at";

fn row_to_pkg(r: &rusqlite::Row) -> rusqlite::Result<OhpmPkg> {
    Ok(OhpmPkg {
        package_name: r.get(0)?,
        version: r.get(1)?,
        author_name: r.get(2)?,
        score: r.get(3)?,
        license: r.get(4)?,
        down_count_60d: r.get(5)?,
        description: r.get(6)?,
        keywords: r.get(7)?,
        file_nums: r.get(8)?,
        file_size: r.get(9)?,
        level1_cn: r.get(10)?,
        level1_en: r.get(11)?,
        level2_cn: r.get(12)?,
        level2_en: r.get(13)?,
        level3_cn: r.get(14)?,
        level3_en: r.get(15)?,
        level4_cn: r.get(16)?,
        level4_en: r.get(17)?,
        likes: r.get(18)?,
        popularity: r.get(19)?,
        latest_publish_time: r.get(20)?,
    })
}

/// 排序白名单 → SQL ORDER BY 子句；未知值回退下载量排序
fn order_sql(order: &str) -> &'static str {
    match order {
        "likes" => "likes DESC, down_count_60d DESC",
        "popularity" => "popularity DESC, down_count_60d DESC",
        "latest" => "latest_publish_time DESC, down_count_60d DESC",
        _ => "down_count_60d DESC, score DESC",
    }
}

/// 缓存状态
pub fn status(conn: &Connection) -> Result<OhpmStatus, String> {
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM ohpm_landscape", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let updated_at: Option<i64> = conn
        .query_row("SELECT MAX(updated_at) FROM ohpm_landscape", [], |r| r.get(0))
        .ok()
        .flatten();
    let categories: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT level1_cn) FROM ohpm_landscape WHERE level1_cn != ''",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(OhpmStatus {
        total,
        updated_at,
        categories,
    })
}

/// 是否需要刷新：无数据，或距上次更新超过 days 天
pub fn needs_refresh(conn: &Connection, days: i64) -> bool {
    let Ok(st) = status(conn) else {
        return true;
    };
    match st.updated_at {
        None => st.total == 0,
        Some(t) => {
            let now = chrono::Utc::now().timestamp();
            now - t > days * 86400
        }
    }
}

/// 关键词检索：包名 / 描述 / 关键词 / 作者 / 四级分类；order 控制排序（见 order_sql）
/// offset 用于分页（LIMIT ?2 OFFSET ?3）
pub fn search(
    conn: &Connection,
    query: &str,
    order: &str,
    limit: usize,
    offset: usize,
) -> Result<Vec<OhpmPkg>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let like = format!("%{q}%");
    let order_sql = order_sql(order);
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {PKG_COLS} FROM ohpm_landscape
             WHERE package_name LIKE ?1 OR description LIKE ?1 OR keywords LIKE ?1
                OR author_name LIKE ?1
                OR level1_cn LIKE ?1 OR level1_en LIKE ?1
                OR level2_cn LIKE ?1 OR level2_en LIKE ?1
                OR level3_cn LIKE ?1 OR level3_en LIKE ?1
                OR level4_cn LIKE ?1 OR level4_en LIKE ?1
             ORDER BY {order_sql}
             LIMIT ?2 OFFSET ?3"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![like, limit as i64, offset as i64], row_to_pkg)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// 热门推荐：order 控制排序（见 order_sql）；offset 用于分页
pub fn hot(
    conn: &Connection,
    order: &str,
    limit: usize,
    offset: usize,
) -> Result<Vec<OhpmPkg>, String> {
    let order_sql = order_sql(order);
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {PKG_COLS} FROM ohpm_landscape
             ORDER BY {order_sql}
             LIMIT ?1 OFFSET ?2"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![limit as i64, offset as i64], row_to_pkg)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// 按一级分类取包；level2 非空时进一步按二级分类过滤；order 控制排序（见 order_sql）；offset 用于分页
pub fn by_category(
    conn: &Connection,
    cat: &str,
    level2: &str,
    order: &str,
    limit: usize,
    offset: usize,
) -> Result<Vec<OhpmPkg>, String> {
    let order_sql = order_sql(order);
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {PKG_COLS} FROM ohpm_landscape
             WHERE (level1_cn = ?1 OR level1_en = ?1)
               AND (?2 = '' OR level2_cn = ?2 OR level2_en = ?2)
             ORDER BY {order_sql}
             LIMIT ?3 OFFSET ?4"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![cat, level2, limit as i64, offset as i64], row_to_pkg)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// 一二级分类树（按包数量降序）
pub fn categories(conn: &Connection) -> Result<Vec<CategoryStat>, String> {
    let mut l1_stmt = conn
        .prepare(
            "SELECT level1_cn, level1_en, COUNT(*) FROM ohpm_landscape
             WHERE level1_cn != ''
             GROUP BY level1_cn, level1_en ORDER BY COUNT(*) DESC",
        )
        .map_err(|e| e.to_string())?;
    let l1: Vec<(String, String, i64)> = l1_stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;

    let mut out = Vec::with_capacity(l1.len());
    for (name_cn, name_en, count) in l1 {
        let mut l2_stmt = conn
            .prepare(
                "SELECT level2_cn, level2_en, COUNT(*) FROM ohpm_landscape
                 WHERE level1_cn = ?1 AND level2_cn != ''
                 GROUP BY level2_cn, level2_en ORDER BY COUNT(*) DESC",
            )
            .map_err(|e| e.to_string())?;
        let children = l2_stmt
            .query_map(params![name_cn], |r| {
                Ok(CategoryStat {
                    name_cn: r.get(0)?,
                    name_en: r.get(1)?,
                    count: r.get(2)?,
                    children: Vec::new(),
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        out.push(CategoryStat {
            name_cn,
            name_en,
            count,
            children,
        });
    }
    Ok(out)
}

/// 统计匹配包数（过滤条件与 search / by_category 完全一致），用于页码分页
pub fn count(conn: &Connection, query: &str, cat: &str, level2: &str) -> Result<i64, String> {
    let q = query.trim();
    if !q.is_empty() {
        let like = format!("%{q}%");
        return conn
            .query_row(
                "SELECT COUNT(*) FROM ohpm_landscape
                 WHERE package_name LIKE ?1 OR description LIKE ?1 OR keywords LIKE ?1
                    OR author_name LIKE ?1
                    OR level1_cn LIKE ?1 OR level1_en LIKE ?1
                    OR level2_cn LIKE ?1 OR level2_en LIKE ?1
                    OR level3_cn LIKE ?1 OR level3_en LIKE ?1
                    OR level4_cn LIKE ?1 OR level4_en LIKE ?1",
                params![like],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string());
    }
    if !cat.is_empty() {
        return conn
            .query_row(
                "SELECT COUNT(*) FROM ohpm_landscape
                 WHERE (level1_cn = ?1 OR level1_en = ?1)
                   AND (?2 = '' OR level2_cn = ?2 OR level2_en = ?2)",
                params![cat, level2],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string());
    }
    conn.query_row("SELECT COUNT(*) FROM ohpm_landscape", [], |r| r.get(0))
        .map_err(|e| e.to_string())
}

/// 单包精确查询（ohpm_recommend 按包名确认时用）
pub fn get(conn: &Connection, package_name: &str) -> Result<Option<OhpmPkg>, String> {
    let mut stmt = conn
        .prepare(&format!("SELECT {PKG_COLS} FROM ohpm_landscape WHERE package_name = ?1"))
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map(params![package_name], row_to_pkg)
        .map_err(|e| e.to_string())?;
    rows.next()
        .transpose()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实接口响应片段（2026-08 抓取自 ide-page，字段/中文均原样保留）
    const SAMPLE: &str = r#"[
      {"packageName":"@zhongrui/app_router","version":"1.0.7","authorName":"zhongrui","score":30,"license":"MIT","downLoadCount60days":183,"description":"AppRouter通过Navigation+hvigor插件实现的动态路由方案，便于项目各模块之间的页面跳转(达到解耦的效果)","keywords":"[NavDestination, 页面跳转, router, Navigation, 路由, Harmony, 动态路由, Router, OpenHarmony, HarmonyOS]","fileNums":24,"fileSize":13568,"levelOneCategoryCn":"网络通信","levelOneCategoryEn":"Network Communication","levelTwoCategoryCn":"应用页面路由","levelTwoCategoryEn":"Application Page Routing","levelThreeCategoryCn":"动态路由","levelThreeCategoryEn":"Dynamic Routing","levelFourCategoryCn":"","levelFourCategoryEn":""},
      {"packageName":"@wolfx/fill_class","version":"2.0.1","authorName":"wolfx","score":50,"license":"MIT","downLoadCount60days":113,"description":"Fill JSON data into class instance.","keywords":"[utils, transformer]","fileNums":27,"fileSize":4818,"levelOneCategoryCn":"工具库","levelOneCategoryEn":"Tool Library","levelTwoCategoryCn":"编程语言工具","levelTwoCategoryEn":"Programming Language Utilities","levelThreeCategoryCn":"字符串处理","levelThreeCategoryEn":"String Processing","levelFourCategoryCn":"字符串类型转换","levelFourCategoryEn":"String Type Conversion"},
      {"packageName":"@ohos/dialogs","version":"1.0.3","authorName":"ohos_tpc","score":25,"license":"Apache License 2.0","downLoadCount60days":64,"description":"基于OpenHarmony的弹窗组件库，支持自定义弹窗，封装通用的弹窗业务场景","keywords":"[XPopup, OpenHarmony, HarmonyOS]","fileNums":91,"fileSize":40277,"levelOneCategoryCn":"UI","levelOneCategoryEn":"UI","levelTwoCategoryCn":"弹窗组件","levelTwoCategoryEn":"Popup Component","levelThreeCategoryCn":"列表选择弹窗","levelThreeCategoryEn":"Picker Dialog Box","levelFourCategoryCn":"","levelFourCategoryEn":""},
      {"packageName":"@nzy/logger","version":"1.0.7","authorName":"niezhiyang","score":30,"license":"Apache-2.0","downLoadCount60days":167,"description":"OpenHarmony 中简单、漂亮、强大的日志","keywords":"[log, printer, logger, OpenHarmony]","fileNums":32,"fileSize":11945,"levelOneCategoryCn":"工具库","levelOneCategoryEn":"Tool Library","levelTwoCategoryCn":"日志记录和管理","levelTwoCategoryEn":"Log Management","levelThreeCategoryCn":"日志打印","levelThreeCategoryEn":"Log Printing","levelFourCategoryCn":"","levelFourCategoryEn":""}
    ]"#;

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/048_ohpm_landscape.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/049_ohpm_landscape_sort.sql"))
            .unwrap();
        conn
    }

    /// 测试用排序指标（包名 → (likes, popularity, latest_publish_time)）
    fn sort_meta(name: &str) -> (i64, i64, i64) {
        match name {
            "@zhongrui/app_router" => (10, 500, 2000),
            "@nzy/logger" => (30, 800, 1000),
            "@wolfx/fill_class" => (20, 300, 3000),
            _ => (40, 100, 500), // @ohos/dialogs
        }
    }

    fn seed(conn: &Connection, now: i64) -> Vec<OhpmPkg> {
        let pkgs: Vec<OhpmPkg> = serde_json::from_str(SAMPLE).unwrap();
        conn.execute("DELETE FROM ohpm_landscape", []).unwrap();
        {
            let mut stmt = conn
                .prepare(
                    "INSERT OR REPLACE INTO ohpm_landscape (
                        package_name, version, author_name, score, license, down_count_60d,
                        description, keywords, file_nums, file_size,
                        level1_cn, level1_en, level2_cn, level2_en,
                        level3_cn, level3_en, level4_cn, level4_en,
                        likes, popularity, latest_publish_time, updated_at
                     ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)",
                )
                .unwrap();
            for p in &pkgs {
                let (likes, popularity, latest) = sort_meta(&p.package_name);
                stmt.execute(params![
                    p.package_name, p.version, p.author_name, p.score, p.license,
                    p.down_count_60d, p.description, p.keywords, p.file_nums, p.file_size,
                    p.level1_cn, p.level1_en, p.level2_cn, p.level2_en,
                    p.level3_cn, p.level3_en, p.level4_cn, p.level4_en,
                    likes, popularity, latest, now,
                ])
                .unwrap();
            }
        }
        pkgs
    }

    #[test]
    fn parse_real_payload() {
        let pkgs: Vec<OhpmPkg> = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(pkgs.len(), 4);
        let p = &pkgs[0];
        // 字段映射（camelCase → snake_case）
        assert_eq!(p.package_name, "@zhongrui/app_router");
        assert_eq!(p.version, "1.0.7");
        assert_eq!(p.license, "MIT");
        assert_eq!(p.down_count_60d, 183);
        assert_eq!(p.level1_cn, "网络通信");
        assert_eq!(p.level2_en, "Application Page Routing");
        assert!(p.description.contains("动态路由"));
        // 辅助方法
        assert_eq!(p.level1(), "网络通信");
        assert_eq!(p.category_path(), "网络通信 / 应用页面路由 / 动态路由");
        assert_eq!(p.detail_url(), "https://ohpm.openharmony.cn/#/cn/detail/@zhongrui%2Fapp_router");
    }

    #[test]
    fn refresh_replaces_all() {
        let conn = mem_db();
        seed(&conn, 1000);
        // 模拟第二次刷新：只回 2 条，旧数据应被清空
        let pkgs: Vec<OhpmPkg> = serde_json::from_str(SAMPLE).unwrap();
        let two = pkgs[..2].to_vec();
        conn.execute("DELETE FROM ohpm_landscape", []).unwrap();
        {
            let mut stmt = conn
                .prepare(
                    "INSERT OR REPLACE INTO ohpm_landscape (
                        package_name, version, author_name, score, license, down_count_60d,
                        description, keywords, file_nums, file_size,
                        level1_cn, level1_en, level2_cn, level2_en,
                        level3_cn, level3_en, level4_cn, level4_en,
                        likes, popularity, latest_publish_time, updated_at
                     ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)",
                )
                .unwrap();
            for p in &two {
                stmt.execute(params![
                    p.package_name, p.version, p.author_name, p.score, p.license,
                    p.down_count_60d, p.description, p.keywords, p.file_nums, p.file_size,
                    p.level1_cn, p.level1_en, p.level2_cn, p.level2_en,
                    p.level3_cn, p.level3_en, p.level4_cn, p.level4_en,
                    0, 0, 0, 2000,
                ])
                .unwrap();
            }
        }
        let st = status(&conn).unwrap();
        assert_eq!(st.total, 2);
        assert_eq!(st.updated_at, Some(2000));
    }

    #[test]
    fn search_hot_category() {
        let conn = mem_db();
        seed(&conn, chrono::Utc::now().timestamp());

        // 状态：4 包 / 3 个一级分类
        let st = status(&conn).unwrap();
        assert_eq!(st.total, 4);
        assert_eq!(st.categories, 3);
        assert!(!needs_refresh(&conn, 7));

        // 关键词检索：命中描述（中文）
        let hits = search(&conn, "日志", "", 10, 0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].package_name, "@nzy/logger");
        // 关键词检索：命中英文分类
        let hits = search(&conn, "router", "", 10, 0).unwrap();
        assert!(hits.iter().any(|p| p.package_name == "@zhongrui/app_router"));
        // 关键词检索：空查询返回空
        assert!(search(&conn, "  ", "", 10, 0).unwrap().is_empty());

        // 热门：按 60 天下载量降序；offset 分页取第 2 页（从第 2 条开始）
        let top_hot = hot(&conn, "", 10, 0).unwrap();
        let names: Vec<&str> = top_hot.iter().map(|p| p.package_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["@zhongrui/app_router", "@nzy/logger", "@wolfx/fill_class", "@ohos/dialogs"]
        );
        let hot_p2 = hot(&conn, "", 2, 2).unwrap();
        assert_eq!(hot_p2.len(), 2);
        assert_eq!(hot_p2[0].package_name, "@wolfx/fill_class");
        assert_eq!(hot_p2[1].package_name, "@ohos/dialogs");
        // 超出范围 → 空页
        assert!(hot(&conn, "", 10, 100).unwrap().is_empty());

        // 官方排序：最受欢迎（likes）/ 最流行（popularity）/ 最新发布（latest）
        let by_likes = hot(&conn, "likes", 10, 0).unwrap();
        assert_eq!(by_likes[0].package_name, "@ohos/dialogs");
        assert_eq!(by_likes[0].likes, 40);
        let by_pop = hot(&conn, "popularity", 10, 0).unwrap();
        assert_eq!(by_pop[0].package_name, "@nzy/logger");
        let by_latest = hot(&conn, "latest", 10, 0).unwrap();
        assert_eq!(by_latest[0].package_name, "@wolfx/fill_class");
        // 未知排序回退下载量
        assert_eq!(hot(&conn, "bogus", 1, 0).unwrap()[0].package_name, "@zhongrui/app_router");
        // 排序同样作用于分类浏览与搜索
        let cats_likes = by_category(&conn, "工具库", "", "likes", 10, 0).unwrap();
        assert_eq!(cats_likes[0].package_name, "@nzy/logger");
        assert_eq!(search(&conn, "路由", "latest", 10, 0).unwrap()[0].package_name, "@zhongrui/app_router");

        // 分类：一级分类取包
        let cats = by_category(&conn, "工具库", "", "", 10, 0).unwrap();
        assert_eq!(cats.len(), 2);
        // 英文分类名同样可查
        let cats_en = by_category(&conn, "Tool Library", "", "", 10, 0).unwrap();
        assert_eq!(cats_en.len(), 2);
        // 二级分类过滤（工具库 / 编程语言工具 → 1 个）
        let cats_l2 = by_category(&conn, "工具库", "编程语言工具", "", 10, 0).unwrap();
        assert_eq!(cats_l2.len(), 1);
        assert_eq!(cats_l2[0].package_name, "@wolfx/fill_class");
        // 一级命中但二级不匹配 → 空
        assert!(by_category(&conn, "工具库", "弹窗组件", "", 10, 0).unwrap().is_empty());

        // 统计：热门总数 / 分类过滤数 / 搜索命中数（与检索条件一致）
        assert_eq!(count(&conn, "", "", "").unwrap(), 4);
        assert_eq!(count(&conn, "", "工具库", "").unwrap(), 2);
        assert_eq!(count(&conn, "", "工具库", "编程语言工具").unwrap(), 1);
        assert_eq!(count(&conn, "日志", "", "").unwrap(), 1);
        // 分类树：工具库含 2 个子分类
        let tree = categories(&conn).unwrap();
        let tool = tree.iter().find(|c| c.name_cn == "工具库").unwrap();
        assert_eq!(tool.count, 2);
        assert_eq!(tool.children.len(), 2);

        // 单包精确查询
        let p = get(&conn, "@ohos/dialogs").unwrap().unwrap();
        assert_eq!(p.level2_cn, "弹窗组件");
        assert!(get(&conn, "@nonexistent/pkg").unwrap().is_none());
    }

    #[test]
    fn needs_refresh_logic() {
        let conn = mem_db();
        // 空库 → 需要刷新
        assert!(needs_refresh(&conn, 7));
        let now = chrono::Utc::now().timestamp();
        seed(&conn, now);
        // 刚更新 → 不需要
        assert!(!needs_refresh(&conn, 7));
        // 更新于 8 天前 → 需要
        seed(&conn, now - 8 * 86400);
        assert!(needs_refresh(&conn, 7));
    }

    /// 真实联网链路（官方接口结构变更时手动验证）：
    ///   cargo test --lib ohpm_landscape -- --ignored
    #[test]
    #[ignore = "联网：依赖官方接口可达"]
    fn live_fetch_and_refresh() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let pkgs = rt.block_on(fetch_remote()).unwrap();
        assert!(pkgs.len() > 500, "真实包数应 500+，实际 {}", pkgs.len());
        let db = Arc::new(Mutex::new(mem_db()));
        let rep = rt.block_on(refresh(&db)).unwrap();
        assert_eq!(rep.total, pkgs.len());
        let conn = db.lock().unwrap();
        let st = status(&conn).unwrap();
        assert_eq!(st.total as usize, pkgs.len());
        // 随机抽一条验证入库字段完整
        let sample = hot(&conn, "", 1, 0).unwrap();
        assert!(!sample[0].package_name.is_empty());
        assert!(!sample[0].level1().is_empty());
    }

    #[test]
    fn serialize_snake_case() {
        // 序列化必须输出 snake_case（前端 OhpmPkg 类型与 Agent 工具依赖此约定）
        let pkgs: Vec<OhpmPkg> = serde_json::from_str(SAMPLE).unwrap();
        let v = serde_json::to_value(&pkgs[0]).unwrap();
        let obj = v.as_object().unwrap();
        assert!(obj.contains_key("package_name"), "缺少 package_name: {obj:?}");
        assert!(obj.contains_key("down_count_60d"));
        assert!(obj.contains_key("level1_cn"));
        assert!(obj.contains_key("level2_en"));
        assert!(!obj.contains_key("packageName"));
        assert_eq!(obj["package_name"], "@zhongrui/app_router");
        assert_eq!(obj["level1_cn"], "网络通信");
    }

    #[test]
    fn extract_repo_metadata() {
        // repository 为字符串（如 @pura/harmony-dialog 实际返回）
        let v: serde_json::Value = serde_json::json!({
            "name": "@pura/harmony-dialog",
            "dist-tags": { "latest": "1.1.8" },
            "versions": {
                "1.0.0": { "version": "1.0.0", "repository": "https://old.example.com/a.git" },
                "1.1.8": { "version": "1.1.8", "repository": "https://gitee.com/tongyuyan/harmony-utils" }
            }
        });
        assert_eq!(
            extract_repo_from_metadata(&v).as_deref(),
            Some("https://gitee.com/tongyuyan/harmony-utils")
        );
        // repository 为 npm 风格 { type, url } 对象
        let v: serde_json::Value = serde_json::json!({
            "dist-tags": { "latest": "2.2.14" },
            "versions": {
                "2.2.14": {
                    "repository": {
                        "type": "git",
                        "url": "https://gitcode.com/CPF-ApplicationTPC/ohos_axios.git"
                    }
                }
            }
        });
        assert_eq!(
            extract_repo_from_metadata(&v).as_deref(),
            Some("https://gitcode.com/CPF-ApplicationTPC/ohos_axios.git")
        );
        // 无 repository / 空字符串 → None
        let v: serde_json::Value =
            serde_json::json!({ "dist-tags": { "latest": "1.0.0" }, "versions": { "1.0.0": {} } });
        assert!(extract_repo_from_metadata(&v).is_none());
        let v: serde_json::Value = serde_json::json!({
            "dist-tags": { "latest": "1.0.0" },
            "versions": { "1.0.0": { "repository": "   " } }
        });
        assert!(extract_repo_from_metadata(&v).is_none());
        // 结构异常（无 versions）→ None
        assert!(extract_repo_from_metadata(&serde_json::json!({})).is_none());
    }
}
