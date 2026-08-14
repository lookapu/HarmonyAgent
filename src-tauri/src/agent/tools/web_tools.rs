//! 联网搜索域：web_search（DuckDuckGo / Bing RSS 抓取与解析）。
//! 共享辅助函数在父模块 mod.rs，通过 `use super::*` 继承。

use super::*;

pub(super) async fn web_search(args: &Value) -> Result<String, String> {
    let query = args["query"].as_str().unwrap_or("").trim().to_string();
    if query.is_empty() {
        return Err("web_search 需要参数 {\"query\":\"<搜索词>\"}".into());
    }
    let count = args["count"].as_u64().unwrap_or(5).clamp(1, 10) as usize;
    let client = crate::utils::net::build_client_auto()?;
    let encoded = urlencode(&query);

    // 1) DuckDuckGo HTML（无 API Key，结果含标题/链接/摘要）
    if let Ok(html) = fetch_text(&client, &format!("https://html.duckduckgo.com/html/?q={encoded}")).await {
        let results = parse_ddg_results(&html, count);
        if !results.is_empty() {
            return Ok(format_results("DuckDuckGo", &results));
        }
    }

    // 2) Bing RSS 回退
    let rss = fetch_text(&client, &format!("https://www.bing.com/search?q={encoded}&format=rss&count={count}"))
        .await
        .map_err(|e| format!("联网搜索失败: {e}"))?;
    let results = parse_bing_rss(&rss, count);
    if results.is_empty() {
        return Err("未搜索到结果，请尝试更换搜索词".into());
    }
    Ok(format_results("Bing", &results))
}

pub(super) fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub(super) async fn fetch_text(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let resp = client.get(url).send().await.map_err(|e| format!("请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let text = resp.text().await.map_err(|e| format!("读取响应失败: {e}"))?;
    Ok(text.chars().take(200_000).collect())
}

pub(super) fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

pub(super) struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

pub(super) fn parse_ddg_results(html: &str, count: usize) -> Vec<SearchResult> {
    let mut out = Vec::new();
    let mut rest = html;
    while out.len() < count {
        let Some(start) = rest.find("class=\"result__a\"") else { break };
        // 标题
        let Some(title_start) = rest[start..].find(">") else { break };
        let after_tag = &rest[start + title_start + 1..];
        let Some(title_end) = after_tag.find("</a>") else { break };
        let title = html_unescape(&after_tag[..title_end]);
        // href：result__a 标签内
        let href = rest[start..start + title_start]
            .find("href=\"")
            .and_then(|h| {
                let v = &rest[start + h + 6..];
                let end = v.find('\"')?;
                Some(html_unescape(&v[..end]))
            })
            .unwrap_or_default();
        let url = normalize_url(&href);
        // 摘要：标题之后最近的 result__snippet（DDG 的摘要位于标题下方）
        let snippet = after_tag
            .find("class=\"result__snippet\"")
            .and_then(|p| {
                let v = &after_tag[p..];
                let after = v.find(">")?;
                let end = v[after + 1..].find("</a>")?;
                Some(html_unescape(&v[after + 1..after + 1 + end]))
            })
            .unwrap_or_default();
        if !title.is_empty() && !url.is_empty() {
            out.push(SearchResult { title, url, snippet });
        }
        rest = &after_tag[title_end..];
    }
    out
}

pub(super) fn parse_bing_rss(rss: &str, count: usize) -> Vec<SearchResult> {
    let mut out = Vec::new();
    let mut rest = rss;
    while out.len() < count {
        let Some(start) = rest.find("<item>") else { break };
        let body = &rest[start + 6..];
        let Some(end) = body.find("</item>") else { break };
        let item = &body[..end];
        let extract = |open: &str, close: &str| -> String {
            item.find(open)
                .and_then(|i| {
                    let v = &item[i + open.len()..];
                    let e = v.find(close)?;
                    Some(html_unescape(&v[..e]))
                })
                .unwrap_or_default()
        };
        let title = extract("<title>", "</title>");
        let url = extract("<link>", "</link>");
        let snippet = extract("<description>", "</description>");
        if !title.is_empty() && !url.is_empty() {
            out.push(SearchResult { title, url, snippet });
        }
        rest = &body[end..];
    }
    out
}

pub(super) fn normalize_url(href: &str) -> String {
    if let Some(pos) = href.find("uddg=") {
        let v = &href[pos + 5..];
        let v = v.split('&').next().unwrap_or(v);
        return percent_decode(v);
    }
    if href.starts_with("//") {
        return format!("https:{href}");
    }
    href.to_string()
}

pub(super) fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

pub(super) fn format_results(source: &str, results: &[SearchResult]) -> String {
    let mut s = format!("（来源: {source}，{} 条结果）\n", results.len());
    for (i, r) in results.iter().enumerate() {
        s.push_str(&format!("{}. {}\n   链接: {}\n   摘要: {}\n", i + 1, r.title, r.url, r.snippet));
    }
    if s.chars().count() > 3000 {
        s = s.chars().take(3000).collect::<String>() + "\n…(结果已截断)";
    }
    s
}

