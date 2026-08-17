//! 自动探索/API 详情域工具：auto_explore / refresh_api_db / search_api / get_api_detail / diff_api_versions 等。
//! 共享辅助函数（run_hdc_shell / default_device_id 等）仍定义在父模块 mod.rs，
//! 本模块通过 `use super::*` 继承访问。

use super::*;
/// 从一行 import 语句中提取模块名（支持 from '...'、require('...')、import '...'）
pub(super) fn extract_import_modules(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' || bytes[i] == b'"' {
            let quote = bytes[i];
            let start = i + 1;
            if let Some(end_rel) = line[start..].find(quote as char) {
                let m = &line[start..start + end_rel];
                if m.starts_with('@') || m.starts_with('.') == false {
                    out.push(m.to_string());
                }
                i = start + end_rel + 1;
            } else {
                break;
            }
        } else {
            i += 1;
        }
    }
    out
}

pub(super) fn collect_source_files(dir: &std::path::Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "node_modules" || name == "oh_modules" || name == ".git"
                || name == "build" || name == ".hvigor" || name.starts_with('.') {
                continue;
            }
            collect_source_files(&path, out);
        } else if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if ext == "ets" || ext == "ts" {
                    out.push(path.to_string_lossy().to_string());
                }
            }
        }
    }
}

/// auto_explore：自动遍历应用页面。
pub(super) async fn auto_explore(args: &Value, roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let project_path = roots.first().map(String::as_str).unwrap_or("").to_string();
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录，无法保存遍历结果".into());
    }
    let max_pages = args["max_pages"].as_u64().unwrap_or(20).min(100) as usize;
    let max_depth = args["max_depth"].as_u64().unwrap_or(4).min(10) as usize;
    let delay_ms = args["delay_ms"].as_u64().unwrap_or(800).min(5000);

    let out_dir = format!("{project_path}/.deveco-agent/explore");
    std::fs::create_dir_all(&out_dir).ok();

    let mut pages: Vec<ExploredPage> = Vec::new();
    let mut visited_signatures: Vec<String> = Vec::new();

    // 第 0 页：初始页面
    let init_page = capture_and_analyze(&device, &out_dir, 0, 0, None, "初始页面").await?;
    let sig = compute_page_signature(&init_page.tree_summary, init_page.clickable_count);
    visited_signatures.push(sig);
    let init_id = pages.len();
    pages.push(init_page);
    let mut current_page_id = Some(init_id);

    // BFS 用队列
    use std::collections::VecDeque;
    let mut queue: VecDeque<(usize, usize, usize)> = VecDeque::new(); // (from_page_id, depth, clickable_index)

    if pages[init_id].depth < max_depth {
        for i in 0..pages[init_id].clickable_count.min(8) {
            queue.push_back((init_id, pages[init_id].depth, i));
        }
    }

    let mut actions_taken: Vec<(usize, usize, usize)> = Vec::new(); // from_page, click_idx, to_page

    while let Some((from_id, depth, click_idx)) = queue.pop_front() {
        if pages.len() >= max_pages {
            break;
        }
        if depth >= max_depth {
            continue;
        }

        // 如果当前页不是 from_id，需要导航回去
        if current_page_id != Some(from_id) {
            // 简单策略：按返回键回首页（回到根页签名匹配即停，防止退过头退出应用），再重新导航
            // （完整的导航树回溯较复杂，这里用回首页 + 重走的简化策略）
            back_to_root(&device, &visited_signatures[0], (depth + 2).min(max_depth + 3)).await;
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            current_page_id = Some(0);

            // 从首页按已知路径到 from_id（BFS 找路径）
            let path = find_path(&actions_taken, from_id);
            for (page_id, _) in &path {
                if *page_id == 0 { continue; }
                // 找到从当前页到 page_id 的点击
                if let Some(&(_, cidx, _)) = actions_taken.iter().find(|(f, _, t)| *f == current_page_id.unwrap_or(0) && *t == *page_id) {
                    // 执行点击
                    if let Err(_) = click_nth_clickable(&device, cidx).await {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    current_page_id = Some(*page_id);
                }
            }
        }

        // 点击第 click_idx 个可点击元素
        if let Err(e) = click_nth_clickable(&device, click_idx).await {
            eprintln!("click failed: {e}");
            continue;
        }
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;

        // 捕获新页面
        let new_page = match capture_and_analyze(
            &device, &out_dir, pages.len(), depth + 1,
            Some(from_id), &format!("点击第 {} 个可点击元素", click_idx + 1)
        ).await {
            Ok(p) => p,
            Err(_) => continue,
        };

        let sig = compute_page_signature(&new_page.tree_summary, new_page.clickable_count);

        // 检查是否已访问过
        if visited_signatures.iter().any(|s| s == &sig) {
            // 已访问，返回
            let _ = super::test_tools::execute_ui_step(&device, &serde_json::json!({"action": "key", "name": "back"})).await;
            tokio::time::sleep(Duration::from_millis(delay_ms / 2)).await;
            continue;
        }

        visited_signatures.push(sig);
        let new_id = pages.len();
        pages.push(new_page);
        actions_taken.push((from_id, click_idx, new_id));
        current_page_id = Some(new_id);

        if depth + 1 < max_depth {
            let n_clickable = pages[new_id].clickable_count.min(6);
            for i in 0..n_clickable {
                queue.push_back((new_id, depth + 1, i));
            }
        }
    }

    // 生成报告
    let mut out = format!("自动遍历完成（设备 {device}）\n");
    out.push_str(&format!("发现页面：{} 个（上限 {max_pages}）\n", pages.len()));
    out.push_str(&format!("最大深度：{max_depth}\n"));
    out.push_str(&format!("输出目录：{out_dir}\n\n"));

    out.push_str("页面列表：\n");
    for (i, page) in pages.iter().enumerate() {
        let depth_str = "  ".repeat(page.depth);
        out.push_str(&format!("  [{i}] {depth_str}页面 {}（深度 {}，{} 个可点击元素）\n",
            if page.title.is_empty() { format!("P{i}") } else { page.title.clone() },
            page.depth,
            page.clickable_count
        ));
        out.push_str(&format!("       截图：{}\n", page.screenshot));
        if let Some(fp) = page.from_page {
            out.push_str(&format!("       来源：页面 {fp}，操作：{}\n", page.from_action));
        }
    }

    out.push_str("\n跳转关系（from → to）：\n");
    for (from, _, to) in &actions_taken {
        out.push_str(&format!("  P{from} → P{to}\n"));
    }

    out.push_str("\n使用建议：\n");
    out.push_str("  • 用 read_file 读取各页面的控件树 JSON 文件做进一步分析\n");
    out.push_str("  • 发现异常页面（黑屏/空白/崩溃）时，结合 read_runtime_logs 排查\n");
    out.push_str("  • 对感兴趣的页面可用 run_ui_flow 做更详细的交互测试\n");

    Ok(out)
}

pub(super) fn compute_page_signature(summary: &str, clickable_count: usize) -> String {
    format!("{}|{}", summary.lines().take(5).collect::<Vec<_>>().join("|"), clickable_count)
}

pub(super) fn find_path(actions: &[(usize, usize, usize)], target: usize) -> Vec<(usize, usize)> {
    // BFS 找从 0 到 target 的路径
    use std::collections::{VecDeque, HashMap};
    let mut queue: VecDeque<usize> = VecDeque::new();
    let mut parent: HashMap<usize, (usize, usize)> = HashMap::new(); // page -> (from_page, click_idx)
    queue.push_back(0);

    while let Some(current) = queue.pop_front() {
        if current == target {
            // 回溯路径
            let mut path: Vec<(usize, usize)> = Vec::new();
            let mut cur = target;
            while cur != 0 {
                if let Some(&(from, cidx)) = parent.get(&cur) {
                    path.push((cur, cidx));
                    cur = from;
                } else { break; }
            }
            path.push((0, 0));
            path.reverse();
            return path;
        }
        for (from, cidx, to) in actions {
            if *from == current && !parent.contains_key(to) {
                parent.insert(*to, (current, *cidx));
                queue.push_back(*to);
            }
        }
    }
    vec![]
}

/// 按返回键回到根页面：每次 back 后对比当前页面签名，与根页签名一致即停止，
/// 避免固定次数 back 在浅层时退出应用；超过 max_back 次仍未匹配则强制停止。
pub(super) async fn back_to_root(device: &str, root_signature: &str, max_back: usize) {
    for _ in 0..max_back.max(1) {
        let _ = super::test_tools::execute_ui_step(device, &serde_json::json!({"action": "key", "name": "back"})).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        if let Some(sig) = page_signature_now(device).await {
            if &sig == root_signature {
                break;
            }
        }
    }
}

/// 只 dump 控件树并计算页面签名（不截图，用于回退导航时的页面识别）
pub(super) async fn page_signature_now(device: &str) -> Option<String> {
    let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
    let dev_file = format!("/data/local/tmp/nav_{ts}.json");
    run_hdc_shell(device, &["uitest", "dumpLayout", "-p", &dev_file], 10).await.ok()?;
    let local = std::env::temp_dir().join("nav_check.json");
    let hdc_args: Vec<String> = vec![
        "-s".to_string(), device.to_string(), "file".to_string(), "recv".to_string(),
        dev_file, local.to_string_lossy().into_owned(),
    ];
    let _ = run_cmd("hdc", &hdc_args, None, 15).await;
    let content = std::fs::read_to_string(&local).ok()?;
    let summary = super::ui_tools::summarize_ui_tree(&content);
    Some(compute_page_signature(&summary, count_clickable(&content)))
}

pub(super) async fn click_nth_clickable(device: &str, idx: usize) -> Result<(), String> {
    // 先 dump 控件树，找到第 idx 个可点击元素的坐标
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dev_file = format!("/data/local/tmp/explore_{ts}.json");
    run_hdc_shell(device, &["uitest", "dumpLayout", "-p", &dev_file], 10).await?;

    let out_tmp = std::env::temp_dir().to_string_lossy().to_string();
    let local_file = format!("{out_tmp}/explore_click.json");
    let hdc_args: Vec<String> = vec![
        "-s".to_string(), device.to_string(), "file".to_string(), "recv".to_string(),
        dev_file.clone(), local_file.clone(),
    ];
    let _ = run_cmd("hdc", &hdc_args, None, 15).await;

    let content = std::fs::read_to_string(&local_file).unwrap_or_default();
    let (x, y) = find_nth_clickable_center(&content, idx)
        .ok_or_else(|| format!("未找到第 {} 个可点击元素", idx + 1))?;

    super::test_tools::execute_ui_step(device, &serde_json::json!({"action": "tap", "x": x, "y": y})).await?;
    Ok(())
}

pub(super) fn find_nth_clickable_center(json: &str, n: usize) -> Option<(i64, i64)> {
    // 简化：找带 clickable:true 的节点，取其 bounds 中心点
    let nodes: Vec<_> = super::ui_tools::scan_json_string_field(json, "type").into_iter().enumerate().collect();
    // 更简单的策略：按 centerX/centerY 字段找
    let centers_x = super::ui_tools::scan_json_string_field(json, "centerX");
    let centers_y = super::ui_tools::scan_json_string_field(json, "centerY");

    // 尝试找 clickable 字段
    let clickables = super::ui_tools::scan_json_string_field(json, "clickable");

    let mut count = 0;
    for (i, (_, cx)) in centers_x.iter().enumerate() {
        if i >= centers_y.len() { break; }
        let is_clickable = if clickables.len() > i {
            let (_, c) = &clickables[i];
            c == "true" || c == "True"
        } else {
            // 如果没有 clickable 字段，假设按钮/列表项/输入框等可点击
            false
        };
        if !is_clickable {
            // 也检查 type 是否是可点击类型
            if nodes.len() > i {
                let t = &nodes[i].1.1;
                let lower = t.to_lowercase();
                if lower.contains("button") || lower.contains("list") || lower.contains("item") 
                    || lower.contains("input") || lower.contains("textfield")
                    || lower.contains("image") || lower.contains("icon")
                    || lower.contains("tab") || lower.contains("menu") {
                    if count == n {
                        let cx = super::ui_tools::first_number(cx).unwrap_or(0.0) as i64;
                        let cy = super::ui_tools::first_number(&centers_y[i].1).unwrap_or(0.0) as i64;
                        return Some((cx, cy));
                    }
                    count += 1;
                }
            }
            continue;
        }
        if count == n {
            let cx = super::ui_tools::first_number(cx).unwrap_or(0.0) as i64;
            let cy = super::ui_tools::first_number(&centers_y[i].1).unwrap_or(0.0) as i64;
            return Some((cx, cy));
        }
        count += 1;
    }
    None
}

pub(super) async fn capture_and_analyze(
    device: &str,
    out_dir: &str,
    page_id: usize,
    depth: usize,
    from_page: Option<usize>,
    action: &str,
) -> Result<ExploredPage, String> {
    // 截图（snapshot_display 默认输出 jpeg 且按后缀校验，须显式 -t png 才能写 .png）
    let dev_png = format!("/data/local/tmp/explore_{page_id}.png");
    let _ = run_hdc_shell(device, &["snapshot_display", "-t", "png", dev_png.as_str()], 10).await;
    let local_screenshot = format!("{out_dir}/page_{page_id:03}.png");
    let hdc_args: Vec<String> = vec![
        "-s".to_string(), device.to_string(), "file".to_string(), "recv".to_string(),
        dev_png.clone(), local_screenshot.clone(),
    ];
    let _ = run_cmd("hdc", &hdc_args, None, 15).await;
    // 清理设备端临时文件
    let _ = run_hdc_shell(device, &["rm", dev_png.as_str()], 10).await;

    // 控件树
    let dev_json = format!("/data/local/tmp/explore_{page_id}.json");
    let _ = run_hdc_shell(device, &["uitest", "dumpLayout", "-p", &dev_json], 10).await;
    let local_tree = format!("{out_dir}/page_{page_id:03}.json");
    let hdc_args2: Vec<String> = vec![
        "-s".to_string(), device.to_string(), "file".to_string(), "recv".to_string(),
        dev_json.clone(), local_tree.clone(),
    ];
    let _ = run_cmd("hdc", &hdc_args2, None, 15).await;

    let content = std::fs::read_to_string(&local_tree).unwrap_or_default();
    let summary = super::ui_tools::summarize_ui_tree(&content);
    let clickable = count_clickable(&content);

    Ok(ExploredPage {
        depth,
        screenshot: local_screenshot,
        tree_summary: summary,
        clickable_count: clickable,
        title: String::new(),
        from_page,
        from_action: action.to_string(),
    })
}

pub(super) fn count_clickable(json: &str) -> usize {
    // 统计 clickable 为 true 的节点数
    let mut count = 0;
    let fields = super::ui_tools::scan_json_string_field(json, "clickable");
    for (_, val) in &fields {
        if val == "true" || val == "True" || val == "1" {
            count += 1;
        }
    }
    if count == 0 {
        // 没有 clickable 字段时，按 type 估算
        let types = super::ui_tools::scan_json_string_field(json, "type");
        for (_, t) in &types {
            let lower = t.to_lowercase();
            if lower.contains("button") || lower.contains("list-item") || lower.contains("tab") 
                || lower.contains("menu") || lower.contains("image") || lower.contains("icon") {
                count += 1;
            }
        }
    }
    count
}

pub(super) struct ExploredPage {
    depth: usize,
    screenshot: String,
    tree_summary: String,
    clickable_count: usize,
    title: String,
    from_page: Option<usize>,
    from_action: String,
}

// ---------- 鸿蒙官方 API 知识库 ----------

/// refresh_api_db：抓取各版本官方 API diff 聚合入库。
pub(super) async fn refresh_api_db(
    db: &crate::db::DbState,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let app = ctx.app.clone();
    let conv = ctx.conversation_id.clone();
    // 进度回调通过 Tauri 事件推给前端
    let app2 = app.clone();
    let conv2 = conv.clone();
    let cb: crate::services::harmony_api_diff::ProgressCb = Box::new(move |p| {
        if let Some(handle) = &app2 {
            use tauri::Emitter;
            let _ = handle.emit(
                "api-refresh-progress",
                serde_json::json!({
                    "conversation_id": conv2,
                    "phase": p.phase,
                    "current": p.current,
                    "total": p.total,
                    "message": p.message,
                }),
            );
        }
    });
    let report = crate::services::harmony_api_diff::refresh_all(db, Some(cb)).await?;
    let mut out = format!(
        "API 知识库刷新完成：扫描 {} 个版本，抓取 {} 个 Kit 页面，入库 {} 条 API 变更。\n",
        report.versions_fetched, report.pages_fetched, report.entries_inserted
    );
    if !report.errors.is_empty() {
        out.push_str(&format!("\n其中 {} 个页面抓取失败（可能是网络问题，可稍后重试）：\n", report.errors.len()));
        for e in report.errors.iter().take(10) {
            out.push_str(&format!("  • {e}\n"));
        }
    }
    out.push_str("\n现在可以用 search_api 搜索任意 API 的声明、版本与所属模块。");
    Ok(out)
}

/// search_api：在官方 API 知识库中搜索。
pub(super) fn search_api(args: &Value, db: &crate::db::DbState) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let total = crate::services::harmony_api_diff::count(&conn)?;
    if total == 0 {
        return Err("API 知识库为空，请先调用 refresh_api_db 从官方文档抓取数据（首次抓取需联网，耗时较长）。".into());
    }
    let query = crate::services::harmony_api_diff::SearchQuery {
        keyword: args["keyword"].as_str().map(|s| s.to_string()),
        module: args["module"].as_str().map(|s| s.to_string()),
        kit: args["kit"].as_str().map(|s| s.to_string()),
        api_level: args["api_level"].as_u64().map(|n| n as u32),
        change_type: args["change_type"].as_str().map(|s| s.to_string()),
        limit: Some((args["limit"].as_u64().unwrap_or(50) as usize).min(200)),
    };
    // 向量增强块会按 RRF 融合结果重排 entries（见下），故声明为可变；
    // 未启用 embedding feature 时无重排代码，编译期放行 unused_mut
    #[cfg_attr(not(feature = "embedding"), allow(unused_mut))]
    let mut entries = crate::services::harmony_api_diff::search(&conn, &query)?;
    // 向量增强：有 keyword 时用语义向量召回与关键词命中做 RRF 融合重排，
    // 让"语义相关但无字面命中"的 API 也能浮上来；向量索引/模型不可用时自动降级为纯关键词结果。
    // 注意：依赖 candle 的 vector_search 仅在 embedding feature 下可用（见 embedding 模块说明），
    // 故整个增强块用 cfg(feature = "embedding") 隔离；未启用时走纯关键词路径。
    #[cfg(feature = "embedding")]
    {
        let keyword = query.keyword.clone().unwrap_or_default();
        if !keyword.trim().is_empty() {
            if let Ok(Some(vec_hits)) =
                crate::services::embedding::vector_search(&conn, &keyword, 50)
            {
                let kw_hits: Vec<(i64, f32)> = entries
                    .iter()
                    .filter_map(|e| e.id)
                    .map(|id| (id, 0.0))
                    .collect();
                let fused = crate::services::embedding::rrf_fuse(
                    &vec_hits,
                    &kw_hits,
                    query.limit.unwrap_or(50).max(10),
                );
                if !fused.is_empty() {
                    use std::collections::{HashMap, HashSet};
                    let by_id: HashMap<i64, crate::services::harmony_api_diff::ApiEntry> = entries
                        .iter()
                        .filter_map(|e| e.id.map(|id| (id, e.clone())))
                        .collect();
                    let mut seen = HashSet::new();
                    let mut reordered: Vec<crate::services::harmony_api_diff::ApiEntry> =
                        Vec::with_capacity(fused.len());
                    for (id, _) in fused {
                        if !seen.insert(id) {
                            continue;
                        }
                        if let Some(e) = by_id.get(&id) {
                            reordered.push(e.clone());
                        } else if let Ok(Some(e)) = fetch_api_entry_by_id(&conn, id) {
                            reordered.push(e);
                        }
                    }
                    if !reordered.is_empty() {
                        entries = reordered;
                    }
                }
            }
        }
    }
    drop(conn);

    let mut out = String::new();
    out.push_str(&format!("共找到 {} 条匹配（知识库总量 {total} 条）：\n\n", entries.len()));
    for (i, e) in entries.iter().take(50).enumerate() {
        out.push_str(&format!("{}. [{}] {}\n", i + 1, e.change_type, e.declaration));
        out.push_str(&format!(
            "   Kit：{} | 模块：{} | 类：{}\n",
            e.kit,
            e.module.as_deref().unwrap_or("-"),
            e.class_name.as_deref().unwrap_or("-")
        ));
        out.push_str(&format!(
            "   版本：{} (API level {:?})\n",
            e.version_label, e.api_level
        ));
        if let Some(dts) = &e.dts_file {
            out.push_str(&format!("   d.ts：{dts}\n"));
        }
        if let Some(old) = &e.old_declaration {
            if !old.is_empty() && old != "NA" {
                out.push_str(&format!("   旧声明：{}\n", old));
            }
        }
        out.push_str(&format!("   文档：{}\n\n", e.source_url));
    }
    if entries.len() > 50 {
        out.push_str(&format!("... 还有 {} 条，可缩小关键词或加 module/kit 过滤。\n", entries.len() - 50));
    }
    Ok(out)
}

/// 按 doc_id 取单条 api_docs 记录（search_api 向量增强时补全"向量独有命中"用）。
/// 仅 embedding feature 下被 search_api 的向量增强块调用。
#[cfg(feature = "embedding")]
pub(super) fn fetch_api_entry_by_id(
    conn: &rusqlite::Connection,
    id: i64,
) -> Result<Option<crate::services::harmony_api_diff::ApiEntry>, String> {
    use crate::services::harmony_api_diff::ApiEntry;
    let mut stmt = conn
        .prepare(
            "SELECT id, kit, dts_file, module, class_name, declaration, api_name,
                    change_type, version_label, api_level, old_declaration, source_url
             FROM api_docs WHERE id = ?1",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map([id], |row| {
            Ok(ApiEntry {
                id: row.get(0)?,
                kit: row.get(1)?,
                dts_file: row.get(2)?,
                module: row.get(3)?,
                class_name: row.get(4)?,
                declaration: row.get(5)?,
                api_name: row.get(6)?,
                change_type: row.get(7)?,
                version_label: row.get(8)?,
                api_level: row.get(9)?,
                old_declaration: row.get(10)?,
                source_url: row.get(11)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.next().transpose().map_err(|e| e.to_string())
}

// ---------- 鸿蒙官方 API 参考正文 ----------

/// refresh_api_details：抓取 harmonyos-references 正文页入库。
pub(super) async fn refresh_api_details(
    db: &crate::db::DbState,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let app = ctx.app.clone();
    let conv = ctx.conversation_id.clone();
    let cb: crate::services::harmony_api_ref::ProgressCb = Box::new(move |p| {
        if let Some(handle) = &app {
            use tauri::Emitter;
            let _ = handle.emit(
                "api-details-progress",
                serde_json::json!({
                    "conversation_id": conv,
                    "phase": p.phase,
                    "current": p.current,
                    "total": p.total,
                    "message": p.message,
                }),
            );
        }
    });
    let report = crate::services::harmony_api_ref::refresh_all(db, Some(cb)).await?;
    let mut out = format!(
        "API 参考正文刷新完成：抓取 {} 个页面，入库 {} 个模块，共 {} 个子项（类/接口/方法/属性）。\n",
        report.pages_fetched, report.pages_stored, report.members_stored
    );
    if !report.errors.is_empty() {
        out.push_str(&format!(
            "\n其中 {} 个页面抓取失败（部分模块 slug 与命名不一致属正常，可通过 search_api 先确认确切模块名）：\n",
            report.errors.len()
        ));
        for e in report.errors.iter().take(10) {
            out.push_str(&format!("  • {e}\n"));
        }
    }
    out.push_str("\n现在可以用 get_api_detail 查询任意模块的官方用法、参数、权限、示例。");
    Ok(out)
}

/// get_api_detail：查询 API 参考详情。
pub(super) fn get_api_detail(args: &Value, db: &crate::db::DbState) -> Result<String, String> {
    let module = args["module"].as_str().map(|s| s.to_string());
    let keyword = args["keyword"].as_str().map(|s| s.to_string());
    if module.is_none() && keyword.is_none() {
        return Err("必须提供 module 或 keyword 之一。".into());
    }
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let (d_count, m_count) = crate::services::harmony_api_ref::count_details(&conn)?;
    if d_count == 0 {
        return Err(
            "API 参考正文库为空，请先调用 refresh_api_details 抓取官方参考页面（首次需联网，耗时较长）。"
                .into(),
        );
    }
    let q = crate::services::harmony_api_ref::DetailQuery {
        module,
        keyword,
        limit: Some((args["limit"].as_u64().unwrap_or(5) as usize).min(50)),
    };
    let hits = crate::services::harmony_api_ref::query_details(&conn, &q)?;
    drop(conn);

    if hits.is_empty() {
        return Ok(format!(
            "未在 {d_count} 个模块 / {m_count} 个子项中找到匹配。可尝试：①用 search_api 确认模块名；②refresh_api_details 重新抓取；③放宽关键词。"
        ));
    }

    let mut out = String::new();
    out.push_str(&format!(
        "在 {d_count} 个模块 / {m_count} 个子项中找到 {} 个匹配：\n\n",
        hits.len()
    ));
    for (i, h) in hits.iter().enumerate() {
        out.push_str(&format!(
            "═══ {}. {} ═══\n",
            i + 1,
            h.title.as_deref().unwrap_or(&h.module)
        ));
        out.push_str(&format!("模块：{}\n", h.module));
        if let Some(kit) = &h.kit {
            out.push_str(&format!("Kit：{kit}\n"));
        }
        if let Some(lvl) = h.since_api_level {
            out.push_str(&format!("首批 API：version {lvl}\n"));
        }
        if h.deprecated {
            out.push_str("⚠️ 该模块已标记废弃\n");
        }
        if let Some(devs) = &h.device_types {
            out.push_str(&format!("设备：{devs}\n"));
        }
        if let Some(imp) = &h.import_snippet {
            out.push_str(&format!("导入：\n{imp}\n"));
        }
        if let Some(sys) = &h.syscap {
            out.push_str(&format!("系统能力：{sys}\n"));
        }
        if let Some(perm) = &h.permissions {
            out.push_str(&format!("权限：{perm}\n"));
        }
        if let Some(snip) = &h.snippet {
            out.push_str(&format!("\n正文片段：\n{snip}\n"));
        }
        if !h.members.is_empty() {
            out.push_str(&format!("\n子项（{} 个）：\n", h.members.len()));
            for m in h.members.iter().take(60) {
                let parent = m
                    .parent_name
                    .as_deref()
                    .map(|p| format!("{p}."))
                    .unwrap_or_default();
                let deprec = if m.deprecated { " [废弃]" } else { "" };
                let since = m
                    .since_api_level
                    .map(|v| format!("(API {v}+)"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "  • [{}{}] {}{}{deprec}{since}\n",
                    parent, m.kind, m.member_name, ""
                ));
                if let Some(desc) = &m.description {
                    let brief: String = desc.chars().take(120).collect();
                    out.push_str(&format!("      {brief}\n"));
                }
            }
            if h.members.len() > 60 {
                out.push_str(&format!("  ... 还有 {} 个子项\n", h.members.len() - 60));
            }
        }
        if let Some(ex) = &h.examples {
            let brief: String = ex.chars().take(600).collect();
            out.push_str(&format!("\n示例：\n{brief}\n"));
            if ex.len() > 600 {
                out.push_str("...（示例已截断，可访问官方文档查看完整代码）\n");
            }
        }
        out.push_str(&format!("\n官方文档：{}\n\n", h.source_url));
    }
    Ok(out)
}

// ---------- 多版本 API diff ----------

/// diff_api_versions：对比两个 API level 的变更。
pub(super) fn diff_api_versions(args: &Value, db: &crate::db::DbState) -> Result<String, String> {
    let from = args["from_level"]
        .as_u64()
        .ok_or("必须提供 from_level（数字 API level）")? as u32;
    let to = args["to_level"]
        .as_u64()
        .ok_or("必须提供 to_level（数字 API level）")? as u32;
    if to <= from {
        return Err("to_level 必须大于 from_level".into());
    }
    let kit = args["kit"].as_str().map(|s| s.to_string());
    let module = args["module"].as_str().map(|s| s.to_string());
    let change_filter = args["change_type"].as_str().map(|s| s.to_string());
    let limit = args["limit"].as_u64().unwrap_or(200) as usize;

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let total = crate::services::harmony_api_diff::count(&conn)?;
    if total == 0 {
        return Err(
            "API 知识库为空，请先调用 refresh_api_db 抓取各版本变更清单。".into()
        );
    }

    // 聚合区间内的变更：api_level 在 (from, to]
    // 用 GROUP BY 去重：同一 (kit, declaration, change_type) 在多个子版本（如 Beta/Release）
    // 出现时，只保留最低 api_level 与最早的 version_label。
    let mut sql = String::from(
        "SELECT kit, module, class_name, declaration, api_name, change_type,
                MIN(version_label), MIN(api_level),
                (SELECT old_declaration FROM api_docs d2
                  WHERE d2.kit = api_docs.kit
                    AND d2.declaration = api_docs.declaration
                    AND d2.change_type = api_docs.change_type
                    AND d2.api_level IS NOT NULL
                    AND d2.api_level > ?1 AND d2.api_level <= ?2
                  ORDER BY d2.api_level ASC LIMIT 1) AS old_decl,
                source_url, dts_file
         FROM api_docs
         WHERE api_level IS NOT NULL AND api_level > ?1 AND api_level <= ?2",
    );
    let mut qargs: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(from), Box::new(to)];
    if let Some(k) = &kit {
        sql.push_str(&format!(" AND kit LIKE ?{}", qargs.len() + 1));
        qargs.push(Box::new(format!("%{k}%")));
    }
    if let Some(m) = &module {
        sql.push_str(&format!(
            " AND (module LIKE ?{} OR dts_file LIKE ?{})",
            qargs.len() + 1,
            qargs.len() + 1
        ));
        qargs.push(Box::new(format!("%{m}%")));
    }
    if let Some(c) = &change_filter {
        sql.push_str(&format!(" AND change_type = ?{}", qargs.len() + 1));
        qargs.push(Box::new(c.clone()));
    }
    sql.push_str(
        " GROUP BY kit, declaration, change_type
          ORDER BY MIN(api_level) ASC, kit, change_type",
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> = qargs.iter().map(|b| b.as_ref()).collect();
    let collected: Vec<(
        String,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
        String,
        String,
        Option<u32>,
        Option<String>,
        String,
        Option<String>,
    )> = {
        let rows = stmt
            .query_map(params_ref.as_slice(), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<u32>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut v = Vec::new();
        for r in rows {
            v.push(r.map_err(|e| e.to_string())?);
        }
        v
    };
    drop(stmt);
    drop(conn);

    #[derive(Default)]
    struct Bucket {
        added: Vec<String>,
        removed: Vec<String>,
        deprecated: Vec<String>,
        modified: Vec<String>,
    }
    let mut bucket: std::collections::HashMap<String, Bucket> = std::collections::HashMap::new();
    let mut total_count = 0usize;
    for (k, module, class, decl, _api_name, ct, ver, lvl, old, _url, _dts) in collected {
        total_count += 1;
        let b = bucket.entry(k).or_default();
        let tag = match lvl {
            Some(v) => format!("[API {v}] "),
            None => String::new(),
        };
        let line = match ct.as_str() {
            "added" => {
                format!("{tag}{}", decl)
            }
            "removed" => {
                if let Some(old) = &old {
                    if old != "NA" && !old.trim().is_empty() {
                        format!("{tag}{}（旧：{}）", decl, old)
                    } else {
                        format!("{tag}{}", decl)
                    }
                } else {
                    format!("{tag}{}", decl)
                }
            }
            "deprecated" => format!("{tag}{}（{} 起废弃）", decl, ver),
            "modified" => {
                if let Some(old) = &old {
                    if old != "NA" && !old.trim().is_empty() {
                        format!("{tag}{}（旧：{}）", decl, old)
                    } else {
                        format!("{tag}{}", decl)
                    }
                } else {
                    format!("{tag}{}", decl)
                }
            }
            _ => continue,
        };
        let _ = module;
        let _ = class;
        match ct.as_str() {
            "added" => b.added.push(line),
            "removed" => b.removed.push(line),
            "deprecated" => b.deprecated.push(line),
            "modified" => b.modified.push(line),
            _ => {}
        }
    }

    let mut out = String::new();
    out.push_str(&format!(
        "API {from} → API {to} 版本变更（区间内共 {total_count} 条，按 Kit 分组）：\n\n"
    ));

    if bucket.is_empty() {
        out.push_str("该区间内没有匹配的变更记录。可能原因：①from/to 不在已抓取范围；②过滤条件过严；③需要 refresh_api_db。\n");
        return Ok(out);
    }

    let mut kits: Vec<&String> = bucket.keys().collect();
    kits.sort();
    let mut shown = 0usize;
    for k in kits {
        let b = &bucket[k];
        out.push_str(&format!("━━━ {} ━━━\n", k));
        if !b.added.is_empty() {
            out.push_str(&format!("【新增 {}】\n", b.added.len()));
            for l in b.added.iter().take(30) {
                out.push_str(&format!("  + {l}\n"));
                shown += 1;
            }
            if b.added.len() > 30 {
                out.push_str(&format!("  ... 还有 {} 条新增\n", b.added.len() - 30));
            }
        }
        if !b.removed.is_empty() {
            out.push_str(&format!("【删除 {}】\n", b.removed.len()));
            for l in b.removed.iter().take(20) {
                out.push_str(&format!("  - {l}\n"));
                shown += 1;
            }
            if b.removed.len() > 20 {
                out.push_str(&format!("  ... 还有 {} 条删除\n", b.removed.len() - 20));
            }
        }
        if !b.deprecated.is_empty() {
            out.push_str(&format!("【废弃 {}】\n", b.deprecated.len()));
            for l in b.deprecated.iter().take(20) {
                out.push_str(&format!("  ⚠ {l}\n"));
                shown += 1;
            }
            if b.deprecated.len() > 20 {
                out.push_str(&format!("  ... 还有 {} 条废弃\n", b.deprecated.len() - 20));
            }
        }
        if !b.modified.is_empty() {
            out.push_str(&format!("【修改 {}】\n", b.modified.len()));
            for l in b.modified.iter().take(20) {
                out.push_str(&format!("  ~ {l}\n"));
                shown += 1;
            }
            if b.modified.len() > 20 {
                out.push_str(&format!("  ... 还有 {} 条修改\n", b.modified.len() - 20));
            }
        }
        out.push('\n');
        if shown >= limit {
            out.push_str(&format!("... 已达输出上限 {limit}，可加 kit/module/change_type 过滤。\n"));
            break;
        }
    }

    out.push_str("═══ 迁移建议 ═══\n");
    out.push_str(&format!(
        "1. 目标版本为 API {to}：新增 API（added）仅在 ≥ 对应 API level 的设备上可用，低版本需做版本判断（canIUse 或 try/catch）。\n"
    ));
    out.push_str("2. 删除的 API（removed）必须替换：用 search_api 查同名 API 的替代声明，或参考新版本的 release note。\n");
    out.push_str("3. 废弃的 API（deprecated）仍可用但将来会删除，建议逐步迁移到新接口。\n");
    out.push_str("4. 修改的 API（modified）注意签名/行为变化，重点回归相关功能。\n");
    out.push_str(&format!(
        "5. 用 get_api_detail 查具体 API 的用法、权限与示例；用 scan_api_compat 扫描本工程是否用到了高版本 API。\n"
    ));

    Ok(out)
}

