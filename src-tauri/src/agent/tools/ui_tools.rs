//! 性能基准/UI 层级/设备控制域工具：run_perf_benchmark / dump_ui_hierarchy / record_ui / replay_ui / analyze_hap_size 等。
//! 共享辅助函数（run_hdc_shell / default_device_id / truncate_out / sample_* 系列 等）仍定义在父模块 mod.rs，
//! 本模块通过 `use super::*` 继承访问。

use super::*;

pub(super) async fn resolve_authorized_device(requested: Option<&str>, capability: &str) -> Result<String, String> {
    let devices = crate::commands::devices::list_devices().await.map_err(|error| format!("无法发现设备：{error}"))?;
    let selected = if let Some(requested) = requested.map(str::trim).filter(|id| !id.is_empty()) {
        devices.iter().find(|device| device.id == requested).ok_or_else(|| format!("未发现指定设备 {requested}；请调用 list_devices 刷新设备状态。"))?
    } else {
        devices
            .iter()
            .find(|device| device.is_default && device.connection == "online" && device.authorized)
            .or_else(|| devices.iter().find(|device| device.connection == "online" && device.authorized))
            .ok_or_else(|| "未检测到已授权在线设备，请连接设备并确认调试授权".to_string())?
    };
    if selected.connection != "online" || !selected.authorized {
        return Err(format!("设备 {} 当前不可操作（raw={} connection={} authorized={}）", selected.id, selected.state, selected.connection, selected.authorized));
    }
    if !selected.capabilities.iter().any(|available| available == capability) {
        return Err(format!("设备 {} 缺少 {capability} 能力", selected.id));
    }
    Ok(selected.id.clone())
}

/// 性能基准快照（同设备同应用对比用）。
#[derive(Clone)]
struct BenchSnapshot {
    startup_ms: Option<f64>,
    cpu: f64,
    pss: f64,
    sys_cpu: f64,
    temp: f64,
    fps: Option<f64>,
    battery_delta: Option<f64>,
    package_bytes: Option<u64>,
}

static BENCH_STORE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, BenchSnapshot>>> = std::sync::OnceLock::new();

/// run_perf_benchmark：运行操作流程 + 采样性能，与上一次基准做差值对比。
pub(super) async fn run_perf_benchmark(
    args: &Value,
    roots: &[String],
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录，无法运行性能基准".into());
    }
    let device = resolve_authorized_device(args["device"].as_str(), "ability").await?;
    let bundle = match args["package"].as_str() {
        Some(p) => p.to_string(),
        None => crate::services::harmony::parse_project(Path::new(project_path)).bundle_name.unwrap_or_default(),
    };
    let label = args["label"].as_str().unwrap_or("").trim().to_string();
    let seconds = args["seconds"].as_u64().unwrap_or(6).clamp(3, 30) as usize;

    let ability = crate::services::harmony::parse_project(Path::new(project_path))
        .main_element
        .unwrap_or_else(|| "EntryAbility".into());
    let startup_ms = if !bundle.is_empty() && args["measure_startup"].as_bool().unwrap_or(true) {
        measure_startup(&device, &bundle, &ability).await.ok()
    } else {
        None
    };
    let battery_before = sample_battery_percent(&device).await.ok();
    let package_bytes = benchmark_package_bytes(args, Path::new(project_path));

    // 1. 可选：先跑一遍 UI 操作流程（让应用进入被测状态）
    let mut flow_report = String::new();
    if let Some(steps) = args["steps"].as_array() {
        if !steps.is_empty() {
            flow_report = super::test_tools::execute_ui_steps(&device, steps).await.join("\n");
            if flow_report.contains("→ 失败") {
                return Err(format!("性能基准的前置 UI 流程失败，已停止采样：\n{flow_report}"));
            }
        }
    }

    // 2. 采样（复用 collect_perf 的采样函数）
    let samples = seconds.max(2);
    let mut proc_cpu: Vec<f64> = Vec::new();
    let mut pss_vals: Vec<f64> = Vec::new();
    let mut sys_cpu: Vec<f64> = Vec::new();
    let mut sys_mem: Vec<f64> = Vec::new();
    let mut temp_vals: Vec<f64> = Vec::new();
    for i in 0..samples {
        if i > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        if let Ok(c) = sample_cpu(&device).await {
            sys_cpu.push(c);
        }
        if let Ok(m) = sample_sys_mem(&device).await {
            sys_mem.push(m);
        }
        if let Ok(t) = sample_temp(&device).await {
            temp_vals.push(t);
        }
        if !bundle.is_empty() {
            if let Ok(pid) = pid_of(&device, &bundle).await {
                if let Ok((pcpu, pss)) = sample_proc(&device, &pid).await {
                    proc_cpu.push(pcpu);
                    pss_vals.push(pss);
                }
            }
        }
    }

    // 3. FPS（尽力而为，设备/系统不支持时跳过）
    let fps = sample_fps(&device).await.ok();
    let battery_after = sample_battery_percent(&device).await.ok();
    let battery_delta = battery_before.zip(battery_after).map(|(before, after)| after - before);

    let snap = BenchSnapshot {
        startup_ms,
        cpu: mean(&proc_cpu),
        pss: mean(&pss_vals),
        sys_cpu: mean(&sys_cpu),
        temp: mean(&temp_vals),
        fps,
        battery_delta,
        package_bytes,
    };

    // 4. 读取上一次基准并写入本次（同锁内读改写，并发 benchmark 不会覆盖彼此基准）
    let key = format!("{project_path}|{device}|{bundle}");
    let store = BENCH_STORE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let prev = {
        let mut m = store.lock().map_err(|e| e.to_string())?;
        let prev = m.get(&key).cloned();
        m.insert(key, snap.clone());
        prev
    };

    // 5. 组装报告
    let label_str = if label.is_empty() { "（未命名）".to_string() } else { format!("「{label}」") };
    let mut out = format!("性能基准报告（设备 {device}，{samples} 次采样，标签 {label_str}）：\n");
    if !flow_report.is_empty() {
        out.push_str(&format!("操作流程：\n{flow_report}\n\n"));
    }
    if !bundle.is_empty() {
        out.push_str(&format!("- 应用包名：{bundle}\n"));
    }
    match snap.startup_ms {
        Some(value) => out.push_str(&format!("- 冷启动状态确认：{value:.0}ms\n")),
        None => out.push_str("- 冷启动状态确认：不可用\n"),
    }
    // 均值/峰值展示：采样为空时显示「不可用」，避免把无数据误导成 0%
    let fmt_avg = |v: &[f64], unit: &str| -> String {
        if v.is_empty() {
            "不可用".to_string()
        } else {
            format!("{:.0}{unit}", mean(v))
        }
    };
    let fmt_peak = |v: &[f64], unit: &str| -> String {
        if v.is_empty() {
            "不可用".to_string()
        } else {
            format!("{:.0}{unit}", max_of(v))
        }
    };
    let fmt_temp = |v: &[f64]| -> String {
        if v.is_empty() {
            "不可用".to_string()
        } else {
            format!("{:.1}℃", mean(v))
        }
    };
    out.push_str(&format!("- 应用进程 CPU：均值 {}，峰值 {}\n", fmt_avg(&proc_cpu, "%"), fmt_peak(&proc_cpu, "%")));
    out.push_str(&format!("- 应用内存(PSS 近似)：均值 {}，峰值 {}\n", fmt_avg(&pss_vals, "MB"), fmt_peak(&pss_vals, "MB")));
    out.push_str(&format!("- 系统 CPU：均值 {}\n", fmt_avg(&sys_cpu, "%")));
    out.push_str(&format!("- 系统内存：均值 {}\n", fmt_avg(&sys_mem, "%")));
    out.push_str(&format!("- 设备温度：均值 {}\n", fmt_temp(&temp_vals)));
    match snap.fps {
        Some(f) => out.push_str(&format!("- FPS：{f:.1}\n")),
        None => out.push_str("- FPS：无法采集（设备不支持 hidumper RenderService）\n"),
    }
    match snap.battery_delta {
        Some(value) => out.push_str(&format!("- 采样窗口电量变化：{value:+.1}%\n")),
        None => out.push_str("- 采样窗口电量变化：不可用\n"),
    }
    match snap.package_bytes {
        Some(value) => out.push_str(&format!("- HAP 文件大小：{}\n", format_bytes(value))),
        None => out.push_str("- HAP 文件大小：不可用（未能唯一选择产物）\n"),
    }

    if let Some(p) = &prev {
        out.push_str("\n与上次基准对比（Δ = 本次 − 上次）：\n");
        out.push_str(&format!("- 应用 CPU：{:.0}% → {:.0}%（{:+}%）\n", p.cpu, snap.cpu, snap.cpu - p.cpu));
        out.push_str(&format!("- 应用内存：{:.0}MB → {:.0}MB（{:+}MB）\n", p.pss, snap.pss, snap.pss - p.pss));
        out.push_str(&format!("- 系统 CPU：{:.0}% → {:.0}%（{:+}%）\n", p.sys_cpu, snap.sys_cpu, snap.sys_cpu - p.sys_cpu));
        out.push_str(&format!("- 设备温度：{:.1}℃ → {:.1}℃（{:+.1}℃）\n", p.temp, snap.temp, snap.temp - p.temp));
        if let (Some(pf), Some(sf)) = (p.fps, snap.fps) {
            out.push_str(&format!("- FPS：{pf:.1} → {sf:.1}（{:+}）\n", sf - pf));
        }
        if let (Some(before), Some(after)) = (p.startup_ms, snap.startup_ms) {
            out.push_str(&format!("- 启动：{before:.0}ms → {after:.0}ms（{:+.0}ms）\n", after - before));
        }
        if let (Some(before), Some(after)) = (p.package_bytes, snap.package_bytes) {
            out.push_str(&format!("- HAP：{} → {}（{:+} bytes）\n", format_bytes(before), format_bytes(after), after as i128 - before as i128));
        }
        let mut verdict = Vec::new();
        if snap.cpu - p.cpu > 15.0 {
            verdict.push("应用 CPU 明显上升，疑似性能回归（主线程忙/重绘增多）");
        } else if p.cpu - snap.cpu > 15.0 {
            verdict.push("应用 CPU 明显下降，性能有优化");
        }
        if snap.pss - p.pss > 50.0 {
            verdict.push("应用内存明显上升，疑似内存泄漏或缓存增长");
        } else if p.pss - snap.pss > 50.0 {
            verdict.push("应用内存明显下降");
        }
        if snap.temp - p.temp > 3.0 {
            verdict.push("温度明显上升，关注发热");
        }
        if p.startup_ms.zip(snap.startup_ms).is_some_and(|(before, after)| after - before > 300.0) {
            verdict.push("启动状态确认变慢超过 300ms");
        }
        if p.package_bytes.zip(snap.package_bytes).is_some_and(|(before, after)| after > before + 512 * 1024) {
            verdict.push("HAP 增长超过 512KB");
        }
        if verdict.is_empty() {
            verdict.push("各项指标变化在噪声范围内，未见明显回归");
        }
        out.push_str(&format!("\n结论：{}\n", verdict.join("；")));
    } else {
        out.push_str("\n（首次基准，已记录；再跑一次可自动对比前后变化）\n");
    }
    ctx.record_run_event(
        "harmony.performance.measured",
        serde_json::json!({
            "project_path": project_path,
            "device_id": device,
            "bundle": bundle,
            "label": label,
            "samples": samples,
            "startup_ms": snap.startup_ms,
            "app_cpu_avg": snap.cpu,
            "app_cpu_peak": (!proc_cpu.is_empty()).then(|| max_of(&proc_cpu)),
            "app_pss_mb": snap.pss,
            "app_pss_peak_mb": (!pss_vals.is_empty()).then(|| max_of(&pss_vals)),
            "system_cpu_avg": snap.sys_cpu,
            "system_memory_avg": (!sys_mem.is_empty()).then(|| mean(&sys_mem)),
            "temperature_c": snap.temp,
            "fps": snap.fps,
            "battery_delta_percent": snap.battery_delta,
            "package_bytes": snap.package_bytes,
        }),
    );
    Ok(out)
}

async fn measure_startup(device: &str, bundle: &str, ability: &str) -> Result<f64, String> {
    let _ = run_hdc_shell(device, &["aa", "force-stop", bundle], 20).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = std::time::Instant::now();
    run_hdc_shell(device, &["aa", "start", "-b", bundle, "-a", ability], 30).await?;
    for _ in 0..40 {
        if run_hdc_shell(device, &["aa", "dump", "-l"], 10)
            .await
            .is_ok_and(|dump| dump.contains(bundle))
        {
            return Ok(started.elapsed().as_secs_f64() * 1000.0);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err("10 秒内未观察到 Ability 状态".into())
}

async fn sample_battery_percent(device: &str) -> Result<f64, String> {
    let output = run_hdc_shell(device, &["hidumper", "-s", "BatteryService", "-a", "-i"], 20).await?;
    parse_battery_percent(&output).ok_or_else(|| "未读取到有效电量".into())
}

fn parse_battery_percent(output: &str) -> Option<f64> {
    output
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim() == "capacity").then(|| value.trim().parse::<f64>().ok()).flatten()
        })
        .filter(|value| (0.0..=100.0).contains(value))
}

fn benchmark_package_bytes(args: &Value, root: &Path) -> Option<u64> {
    let canonical_root = root.canonicalize().ok()?;
    let path = if let Some(raw) = args["hap"].as_str().map(str::trim).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(raw);
        if path.is_absolute() { path } else { root.join(path) }
    } else {
        crate::services::harmony_build::select_deploy_artifact(
            root,
            args["product"].as_str(),
            args["module"].as_str(),
        )
        .ok()?
        .absolute_path
    };
    let canonical_path = path.canonicalize().ok()?;
    if !canonical_path.starts_with(&canonical_root) {
        return None;
    }
    std::fs::metadata(canonical_path).ok().map(|metadata| metadata.len())
}

/// 采样当前窗口 FPS（hidumper RenderService fps），不支持时返回 Err。
pub(super) async fn sample_fps(device: &str) -> Result<f64, String> {
    let out = run_hdc_shell(device, &["hidumper", "-s", "RenderService", "-a", "fps"], 20).await?;
    for line in out.lines() {
        let lower = line.to_lowercase();
        if !lower.contains("fps") {
            continue;
        }
        if let Some(v) = first_number(line) {
            if v > 0.0 && v <= 240.0 {
                return Ok(v);
            }
        }
    }
    Err("no fps".into())
}

/// 从字符串中提取第一个数字（浮点）。
pub(super) fn first_number(s: &str) -> Option<f64> {
    let mut start = None;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() || c == '.' || (c == '-' && start.is_none()) {
            if start.is_none() {
                start = Some(i);
            }
        } else if start.is_some() {
            let seg = &s[start.unwrap()..i];
            if let Ok(v) = seg.parse::<f64>() {
                return Some(v);
            }
            start = None;
        }
    }
    if let Some(i) = start {
        s[i..].parse::<f64>().ok()
    } else {
        None
    }
}

pub(super) fn max_of(vals: &[f64]) -> f64 {
    vals.iter().cloned().fold(0.0f64, f64::max)
}

#[cfg(test)]
mod performance_tests {
    use super::parse_battery_percent;

    #[test]
    fn parses_battery_capacity() {
        assert_eq!(parse_battery_percent("BatteryService:\n  capacity: 87\n"), Some(87.0));
    }

    #[test]
    fn rejects_invalid_battery_capacity() {
        assert_eq!(parse_battery_percent("capacity: 101\n"), None);
        assert_eq!(parse_battery_percent("capacity: unknown\n"), None);
    }
}

// ---------- UI 控件树 / 启动 Ability / 应用数据清理 / 内存分析 / 应用查询 / 卸载 / 权限 / 网络 / 录屏 ----------

/// dump_ui_hierarchy：导出当前界面控件树 JSON，保存到工程目录并返回摘要。
pub(super) async fn dump_ui_hierarchy(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let (local_path, content) = capture_ui_hierarchy(project_path, &device).await?;
    let local_file = local_path.to_string_lossy();
    let total_nodes = count_json_nodes(&content);
    let summary = summarize_ui_tree(&content);

    let mut out = format!("UI 控件树导出成功（设备 {device}）\n");
    out.push_str(&format!("文件路径：{local_file}\n"));
    out.push_str(&format!("节点总数（约）：{total_nodes}\n"));
    out.push_str(&format!("{summary}\n"));
    out.push_str("\n前 2000 字符预览：\n");
    out.push_str(&tail(&content, 2000));
    out.push_str("\n\n使用建议：结合 read_file 读取完整 JSON；要按文字/类型查找控件可用 search_file 搜索；要点击对应控件可用 dump 中的 centerX/centerY 配合 run_ui_flow 的 tap 操作。");
    Ok(out)
}

pub(super) async fn capture_ui_hierarchy(project_path: &str, device: &str) -> Result<(PathBuf, String), String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dev_file = format!("/data/local/tmp/ui_dump_{}.json", ts);
    run_hdc_shell(&device, &["uitest", "dumpLayout", "-p", &dev_file], 30).await
        .map_err(|e| format!("控件树导出失败：{e}"))?;

    let local_dir = if project_path.is_empty() {
        std::env::temp_dir().to_string_lossy().to_string()
    } else {
        // 与截图口径一致：.deveco-agent 目录（不用 .trae，避免 IDE 清缓存丢产物）
        Path::new(project_path)
            .join(".deveco-agent")
            .to_string_lossy()
            .to_string()
    };
    std::fs::create_dir_all(&local_dir).ok();
    // 文件名：毫秒时间戳 + 设备号（与截图口径一致，多设备/连续导出不覆盖）
    let ts_ms = chrono::Local::now().format("%Y%m%d-%H%M%S%3f");
    let dev_safe: String = device
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    let local_file = PathBuf::from(format!("{local_dir}/ui_hierarchy-{ts_ms}-{dev_safe}.json"));

    // 通过 hdc file recv 拉到本地
    let hdc_args: Vec<String> = vec![
        "-s".to_string(), device.to_string(), "file".to_string(), "recv".to_string(),
        dev_file.clone(), local_file.to_string_lossy().to_string(),
    ];
    run_cmd("hdc", &hdc_args, None, 30).await
        .map_err(|e| format!("拉取控件树文件失败: {e}"))?;
    if !local_file.exists() {
        return Err("拉取控件树文件失败：本地文件未生成".into());
    }

    let content = std::fs::read_to_string(&local_file).unwrap_or_default();
    Ok((local_file, content))
}

/// 粗略统计 JSON 中对象节点数量（估算"{}"对数）。
pub(super) fn count_json_nodes(json: &str) -> usize {
    let mut count = 0usize;
    let mut in_str = false;
    let mut escape = false;
    for c in json.chars() {
        if escape {
            escape = false;
            continue;
        }
        if in_str {
            if c == '\\' { escape = true; continue; }
            if c == '"' { in_str = false; }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => count += 1,
            _ => {}
        }
    }
    count
}

/// 从控件树 JSON 中抽取关键控件摘要（按钮/输入框/文本/列表等）。
pub(super) fn summarize_ui_tree(json: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut type_counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut texts: Vec<String> = Vec::new();

    // 提取 type / text 字段（宽松字符串解析，不依赖正则）
    for (_, val) in scan_json_string_field(json, "type") {
        *type_counts.entry(val).or_insert(0) += 1;
    }
    for val in scan_json_string_field(json, "text").into_iter().map(|(_, v)| v) {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() && !texts.iter().any(|x| x == &trimmed) && texts.len() < 15 {
            texts.push(trimmed);
        }
    }

    if !type_counts.is_empty() {
        lines.push("控件类型统计：".to_string());
        for (t, n) in &type_counts {
            lines.push(format!("  {t}: {n}"));
        }
    }
    if !texts.is_empty() {
        lines.push("可见文字片段（最多 15 条）：".to_string());
        for t in &texts {
            let short = if t.chars().count() > 60 {
                let mut s = t.chars().take(60).collect::<String>();
                s.push('…');
                s
            } else {
                t.clone()
            };
            lines.push(format!("  • {short}"));
        }
    }
    lines.join("\n")
}

/// 从 JSON 文本中扫描 `"field": "value"` 形式的字段，返回所有匹配（位置 + 值）。
pub(super) fn scan_json_string_field(json: &str, field: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let needle = format!("\"{field}\"");
    let bytes = json.as_bytes();
    let nb = needle.as_bytes();
    let mut i = 0;
    while i + nb.len() <= bytes.len() {
        if &bytes[i..i + nb.len()] == nb {
            // 跳过冒号与空白
            let mut j = i + nb.len();
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\n' || bytes[j] == b'\r') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b':' {
                j += 1;
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\n' || bytes[j] == b'\r') {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'"' {
                    // 读取字符串值
                    let start = j + 1;
                    let mut k = start;
                    let mut escape = false;
                    while k < bytes.len() {
                        if escape {
                            escape = false;
                        } else if bytes[k] == b'\\' {
                            escape = true;
                        } else if bytes[k] == b'"' {
                            break;
                        }
                        k += 1;
                    }
                    if k < bytes.len() && bytes[k] == b'"' {
                        let val = &json[start..k];
                        out.push((i, unescape_json(val)));
                    }
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

/// 最简 JSON 字符串反转义（处理 \" \\ \n \t \r ）。
pub(super) fn unescape_json(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('/') => out.push('/'),
                Some(other) => { out.push('\\'); out.push(other); }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// start_ability：显式或隐式拉起 Ability。
pub(super) async fn start_ability(args: &Value, roots: &[String]) -> Result<String, String> {
    let device = resolve_authorized_device(args["device"].as_str(), "ability").await?;
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    let bundle = match args["bundle"].as_str() {
        Some(b) => b.to_string(),
        None => {
            if project_path.is_empty() {
                return Err("未指定 bundle 且当前会话未绑定工程".into());
            }
            crate::services::harmony::parse_project(Path::new(project_path)).bundle_name.unwrap_or_default()
        }
    };
    let ability = args["ability"].as_str().unwrap_or("").to_string();
    let uri = args["uri"].as_str().unwrap_or("").to_string();

    if bundle.is_empty() && uri.is_empty() {
        return Err("start_ability 至少需要 bundle 或 uri 其一".into());
    }

    let mut cmd: Vec<&str> = vec!["aa", "start"];
    let mut owned: Vec<String> = Vec::new();
    if !bundle.is_empty() {
        owned.push("-b".to_string());
        owned.push(bundle.clone());
    }
    if !ability.is_empty() {
        owned.push("-a".to_string());
        owned.push(ability.clone());
    }
    if !uri.is_empty() {
        owned.push("-D".to_string());
        owned.push(uri.clone());
    }
    for o in &owned {
        cmd.push(o.as_str());
    }

    let out = run_hdc_shell(&device, &cmd, 20).await
        .map_err(|e| format!("启动 Ability 失败：{e}"))?;

    // 状态确认：显式 bundle 必须在多次 Ability 栈观测中至少出现一次。
    let mut observed = bundle.is_empty();
    let mut foreground = false;
    for wait in [800u64, 1200, 2000] {
        tokio::time::sleep(Duration::from_millis(wait)).await;
        if let Ok(dump) = run_hdc_shell(&device, &["aa", "dump", "-l"], 10).await {
            if bundle.is_empty() || dump.contains(&bundle) { observed = true; }
            if dump.lines().any(|line| line.contains(&bundle) && line.contains("foreground")) {
                foreground = true;
                break;
            }
        }
    }
    if !observed {
        let hilog = run_hdc_shell(&device, &["hilog", "-x"], 25).await.unwrap_or_default();
        let evidence = hilog.lines().filter(|line| line.contains(&bundle)).collect::<Vec<_>>().join("\n");
        return Err(format!(
            "Ability 启动命令已返回，但状态确认未观察到 {bundle}。\n日志证据：{}",
            if evidence.is_empty() { "（未捕获到相关 hilog）".to_string() } else { tail(&evidence, 1200) }
        ));
    }

    let mut report = format!("启动 Ability（设备 {device}）\n");
    if !bundle.is_empty() { report.push_str(&format!("包名：{bundle}\n")); }
    if !ability.is_empty() { report.push_str(&format!("Ability：{ability}\n")); }
    if !uri.is_empty() { report.push_str(&format!("URI：{uri}\n")); }
    report.push_str(&format!("结果：{}\n", out.trim()));
    report.push_str(&format!("前台状态：{}\n", if foreground { "应用已进入前台 ✅" } else { "未检测到前台（可能还在启动或启动失败）" }));
    Ok(report)
}

/// clear_app_data：清除缓存 / 数据 / 全部。
pub(super) async fn clear_app_data(args: &Value, roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    let bundle = match args["bundle"].as_str() {
        Some(b) => b.to_string(),
        None => {
            if project_path.is_empty() {
                return Err("未指定 bundle 且当前会话未绑定工程".into());
            }
            crate::services::harmony::parse_project(Path::new(project_path)).bundle_name.unwrap_or_default()
        }
    };
    if bundle.is_empty() {
        return Err("无法确定应用包名".into());
    }
    let target = args["target"].as_str().unwrap_or("both");

    let mut results: Vec<String> = Vec::new();
    let mut any_ok = false;
    if target == "cache" || target == "both" {
        match run_hdc_shell(&device, &["bm", "clean", "-c", "-n", &bundle], 20).await {
            Ok(o) => { results.push(format!("清除缓存：{}", o.trim())); any_ok = true; }
            Err(e) => results.push(format!("清除缓存失败：{e}")),
        }
    }
    if target == "data" || target == "both" {
        match run_hdc_shell(&device, &["bm", "clean", "-d", "-n", &bundle], 20).await {
            Ok(o) => { results.push(format!("清除数据：{}", o.trim())); any_ok = true; }
            Err(e) => results.push(format!("清除数据失败：{e}")),
        }
    }

    let mut out = format!("清空应用数据（设备 {device}，包名 {bundle}，目标 {target}）\n");
    for r in &results {
        out.push_str(&format!("- {r}\n"));
    }
    if !any_ok {
        return Err(out);
    }
    out.push_str("\n提示：清除后应用将恢复到首次安装状态，登录信息/缓存/本地数据库都会被清空。");
    Ok(out)
}

/// dump_memory：读取应用内存使用情况并结构化报告。
pub(super) async fn dump_memory(args: &Value, roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    let bundle = match args["bundle"].as_str() {
        Some(b) => b.to_string(),
        None => {
            if project_path.is_empty() {
                return Err("未指定 bundle 且当前会话未绑定工程".into());
            }
            crate::services::harmony::parse_project(Path::new(project_path)).bundle_name.unwrap_or_default()
        }
    };
    if bundle.is_empty() {
        return Err("无法确定应用包名".into());
    }
    let pid = pid_of(&device, &bundle).await?;

    // 1) smaps 解析（尽力而为，需要权限）
    let smaps_raw = run_hdc_shell(&device, &["cat", &format!("/proc/{pid}/smaps")], 20).await
        .unwrap_or_default();
    let smaps_summary = parse_smaps_summary(&smaps_raw);

    // 2) hidumper --mem <pid>（尽力而为）
    let hidumper_raw = run_hdc_shell(&device, &["hidumper", "--mem", &pid.to_string()], 20)
        .await
        .unwrap_or_else(|_| String::new());
    let hi_summary = parse_hidumper_mem(&hidumper_raw);

    // 3) /proc/<pid>/status
    let status_raw = run_hdc_shell(&device, &["cat", &format!("/proc/{pid}/status")], 10)
        .await
        .unwrap_or_default();
    let rss_kb = extract_kb(&status_raw, "VmRSS:");
    let vm_size = extract_kb(&status_raw, "VmSize:");

    let mut out = format!("内存分析报告（设备 {device}，包名 {bundle}，PID {pid}）\n");
    out.push_str(&format!("- VmRSS：{rss_kb:.0} KB（{:.1} MB）\n", rss_kb / 1024.0));
    out.push_str(&format!("- VmSize：{vm_size:.0} KB（{:.1} MB）\n", vm_size / 1024.0));
    out.push_str(&format!("- /proc/{pid}/smaps 摘要：\n"));
    if smaps_summary.is_empty() {
        out.push_str("  （无法读取 smaps，需要 root 或 userdebug 权限）\n");
    } else {
        for (k, v) in &smaps_summary {
            out.push_str(&format!("  {k}: {v:.0} KB\n"));
        }
    }
    if !hi_summary.is_empty() {
        out.push_str("- hidumper 摘要：\n");
        for (k, v) in &hi_summary {
            out.push_str(&format!("  {k}: {v}\n"));
        }
    }
    out.push_str("\n提示：若 smaps 读不到，可先确认设备是否 root 或为 userdebug 版本；可用 collect_perf 做基础监控。");
    Ok(out)
}

pub(super) fn extract_kb(text: &str, key: &str) -> f64 {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            if let Some(v) = first_number(rest) {
                return v;
            }
        }
    }
    0.0
}

/// 解析 smaps 关键分类汇总（PSS / RSS / Shared / Private 等）。
pub(super) fn parse_smaps_summary(smaps: &str) -> std::collections::BTreeMap<String, f64> {
    let mut map: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    let keys = ["Pss:", "Rss:", "Shared_Clean:", "Shared_Dirty:", "Private_Clean:", "Private_Dirty:", "SwapPss:"];
    for line in smaps.lines() {
        let t = line.trim();
        for k in &keys {
            if let Some(rest) = t.strip_prefix(k) {
                if let Some(v) = first_number(rest) {
                    *map.entry(k.trim_end_matches(':').to_string()).or_insert(0.0) += v;
                }
            }
        }
    }
    map
}

/// 从 hidumper --mem 输出中提取关键行。
pub(super) fn parse_hidumper_mem(raw: &str) -> std::collections::BTreeMap<String, String> {
    let mut map: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let keywords = ["total pss", "java heap", "native heap", "code", "stack", "graphics", "private other", "system"];
    for line in raw.lines() {
        let lower = line.to_lowercase();
        for kw in &keywords {
            if lower.contains(kw) {
                map.insert(kw.to_string(), line.trim().to_string());
                break;
            }
        }
    }
    map
}

/// get_installed_apps：列出已安装应用。
pub(super) async fn get_installed_apps(args: &Value, _roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let filter = args["filter"].as_str().unwrap_or("").to_lowercase();
    let raw = run_hdc_shell(&device, &["bm", "dump", "-a"], 30).await
        .map_err(|e| format!("查询已安装应用失败：{e}"))?;

    let mut pkgs: Vec<String> = Vec::new();
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("ID:") || t.starts_with('[') || t.contains(':') && !t.contains('.') {
            continue;
        }
        if t.contains('.') && !t.contains(' ') && !t.contains('{') {
            if filter.is_empty() || t.to_lowercase().contains(&filter) {
                pkgs.push(t.to_string());
            }
        }
    }

    let mut out = format!("已安装应用（设备 {device}，共 {} 个", pkgs.len());
    if !filter.is_empty() {
        out.push_str(&format!("，过滤关键字 \"{filter}\""));
    }
    out.push_str("）：\n");
    let limit = 60;
    for (i, p) in pkgs.iter().take(limit).enumerate() {
        out.push_str(&format!("{}. {p}\n", i + 1));
    }
    if pkgs.len() > limit {
        out.push_str(&format!("... 还有 {} 个，可缩小 filter 关键词查看更多\n", pkgs.len() - limit));
    }
    Ok(out)
}

/// get_app_info：查询应用详情。
pub(super) async fn get_app_info(args: &Value, roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    let bundle = match args["bundle"].as_str() {
        Some(b) => b.to_string(),
        None => {
            if project_path.is_empty() {
                return Err("未指定 bundle 且当前会话未绑定工程".into());
            }
            crate::services::harmony::parse_project(Path::new(project_path)).bundle_name.unwrap_or_default()
        }
    };
    if bundle.is_empty() {
        return Err("无法确定应用包名".into());
    }
    let raw = run_hdc_shell(&device, &["bm", "dump", "-n", &bundle], 30)
        .await
        .map_err(|e| format!("查询应用信息失败：{e}"))?;

    let version_code = extract_json_num(&raw, "versionCode");
    let version_name = extract_json_str(&raw, "versionName");
    let api_target = extract_json_num(&raw, "apiTargetVersion");
    let app_type = extract_json_str(&raw, "bundleType");
    let priv_level = extract_json_str(&raw, "appPrivilegeLevel");
    let provision = extract_json_str(&raw, "appProvisionType");

    let mut out = format!("应用信息（设备 {device}，包名 {bundle}）\n");
    out.push_str(&format!("- 版本号（code）：{}\n", version_code.unwrap_or_default()));
    out.push_str(&format!("- 版本名（name）：{}\n", version_name.unwrap_or_default()));
    out.push_str(&format!("- 目标 API：{}\n", api_target.unwrap_or_default()));
    out.push_str(&format!("- 类型：{}\n", app_type.unwrap_or_default()));
    out.push_str(&format!("- 特权等级：{}\n", priv_level.unwrap_or_default()));
    out.push_str(&format!("- 签名类型：{}\n", provision.unwrap_or_default()));
    out.push_str("\n原始输出前 1500 字：\n");
    out.push_str(&tail(&raw, 1500));
    out.push_str("\n\n提示：完整内容可用 run_shell 自行跑 `bm dump -n <包名>` 查看。");
    Ok(out)
}

pub(super) fn extract_json_str(text: &str, field: &str) -> Option<String> {
    scan_json_string_field(text, field).into_iter().next().map(|(_, v)| v)
}

pub(super) fn extract_json_num(text: &str, field: &str) -> Option<String> {
    // 宽松匹配 "field": 数字
    let needle = format!("\"{field}\"");
    let bytes = text.as_bytes();
    let nb = needle.as_bytes();
    let mut i = 0;
    while i + nb.len() <= bytes.len() {
        if &bytes[i..i + nb.len()] == nb {
            let mut j = i + nb.len();
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\n' || bytes[j] == b'\r') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b':' {
                j += 1;
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\n' || bytes[j] == b'\r') {
                    j += 1;
                }
                let start = j;
                while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'.' || bytes[j] == b'-') {
                    j += 1;
                }
                if j > start {
                    return Some(text[start..j].to_string());
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    None
}

/// uninstall_app：卸载应用。
pub(super) async fn uninstall_app(args: &Value, roots: &[String]) -> Result<String, String> {
    let device = resolve_authorized_device(args["device"].as_str(), "install").await?;
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    let bundle = match args["bundle"].as_str() {
        Some(b) => b.to_string(),
        None => {
            if project_path.is_empty() {
                return Err("未指定 bundle 且当前会话未绑定工程".into());
            }
            crate::services::harmony::parse_project(Path::new(project_path)).bundle_name.unwrap_or_default()
        }
    };
    if bundle.is_empty() {
        return Err("无法确定应用包名".into());
    }
    let keep_data = args["keep_data"].as_bool().unwrap_or(false);

    let args = if keep_data {
        vec!["bm", "uninstall", "-k", "-n", &bundle]
    } else {
        vec!["bm", "uninstall", "-n", &bundle]
    };
    let out = run_hdc_shell(&device, &args, 30).await
        .map_err(|e| format!("卸载失败：{e}"))?;

    let still_installed = run_hdc_shell(&device, &["bm", "dump", "-n", &bundle], 20).await
        .is_ok_and(|dump| dump.contains(&bundle) && !dump.contains("not found"));
    if still_installed {
        return Err(format!("卸载命令已返回，但状态确认仍显示 {bundle} 已安装。结果：{}", out.trim()));
    }
    if !project_path.is_empty() {
        crate::agent::runtime_log::stop(project_path);
    }

    Ok(format!("卸载完成并已确认应用不存在（设备 {device}，包名 {bundle}，保留数据：{keep_data}）\n结果：{}", out.trim()))
}

/// grant_permission：授予权限。
pub(super) async fn grant_permission(args: &Value, roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    let bundle = match args["bundle"].as_str() {
        Some(b) => b.to_string(),
        None => {
            if project_path.is_empty() {
                return Err("未指定 bundle 且当前会话未绑定工程".into());
            }
            crate::services::harmony::parse_project(Path::new(project_path)).bundle_name.unwrap_or_default()
        }
    };
    let perm = args["permission"].as_str().ok_or("grant_permission 需要参数 {\"permission\":\"<权限名>\"}")?.to_string();
    if bundle.is_empty() {
        return Err("无法确定应用包名".into());
    }

    // 尝试 bm grant（需要权限/root），失败则给备选方案
    let result = run_hdc_shell(&device, &["bm", "grant-permission", "-n", &bundle, "-p", &perm], 20).await;
    match result {
        Ok(o) => Ok(format!("授予权限成功（设备 {device}，包名 {bundle}，权限 {perm}）\n结果：{}", o.trim())),
        Err(e) => {
            // 兼容某些版本使用 grant 命令
            let result2 = run_hdc_shell(&device, &["bm", "grant", &bundle, &perm], 20).await;
            match result2 {
                Ok(o) => Ok(format!("授予权限成功（设备 {device}，包名 {bundle}，权限 {perm}）\n结果：{}", o.trim())),
                Err(e2) => Err(format!(
                    "授予权限失败：{e}\n备选方案同样失败：{e2}\n\n提示：user 版本可能不支持 bm grant，需要 root/userdebug；或手动在系统设置 → 应用 → 权限中开启。"
                )),
            }
        }
    }
}

/// set_wifi_state：切换 Wi-Fi 开关（尽力而为）。
pub(super) async fn set_wifi_state(args: &Value, _roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let enable = args["enable"].as_bool().unwrap_or(true);
    let val = if enable { "1" } else { "0" };

    // 尝试几种常见方式
    let attempts = vec![
        ("cmd wifi set_wifi_enable", vec!["cmd", "wifi", "set_wifi_enable", val]),
        ("wpa_cli", vec!["wpa_cli", "-i", "wlan0", if enable { "ifup" } else { "ifdown" }]),
        ("svc wifi", vec!["svc", "wifi", if enable { "enable" } else { "disable" }]),
    ];
    let mut errors: Vec<String> = Vec::new();
    for (name, cmd) in &attempts {
        match run_hdc_shell(&device, cmd, 10).await {
            Ok(o) => {
                let low = o.to_lowercase();
                if !low.contains("not found") && !low.contains("unknown") && !low.contains("failed") && !low.contains("无此命令") {
                    return Ok(format!("Wi-Fi 已{}（设备 {device}，方式：{name}）\n输出：{}", if enable { "打开" } else { "关闭" }, o.trim()));
                }
                errors.push(format!("{name}: {}", o.trim()));
            }
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    Err(format!(
        "切换 Wi-Fi 失败（设备 {device}），所有途径均不可用：\n{}\n\n提示：user 版本设备可能不支持从 hdc 直接操作网络，请手动操作。",
        errors.join("\n")
    ))
}

/// set_airplane_mode：切换飞行模式（尽力而为）。
pub(super) async fn set_airplane_mode(args: &Value, _roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let enable = args["enable"].as_bool().unwrap_or(true);
    let val = if enable { "1" } else { "0" };

    let attempts = vec![
        ("cmd airplane_mode", vec!["cmd", "power", "set-airplane-mode", val]),
        ("settings put global", vec!["settings", "put", "global", "airplane_mode_on", val]),
    ];
    let mut errors: Vec<String> = Vec::new();
    for (name, cmd) in &attempts {
        match run_hdc_shell(&device, &cmd, 10).await {
            Ok(o) => {
                let low = o.to_lowercase();
                if !low.contains("not found") && !low.contains("unknown") && !low.contains("failed") && !low.contains("无此命令") {
                    return Ok(format!("飞行模式已{}（设备 {device}，方式：{name}）\n输出：{}", if enable { "打开" } else { "关闭" }, o.trim()));
                }
                errors.push(format!("{name}: {}", o.trim()));
            }
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    Err(format!(
        "切换飞行模式失败（设备 {device}）：\n{}\n\n提示：user 版本设备可能不支持从 hdc 直接操作，请手动操作。",
        errors.join("\n")
    ))
}

/// screen_record：开始/停止录屏。
static RECORD_STORE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, RecordHandle>>> = std::sync::OnceLock::new();

struct RecordHandle {
    device_file: String,
    /// 后台执行 screenrecord 的任务：录制会持续到 --time-limit 或被 pkill，
    /// 不能同步 await（会把 start 卡住 60~600 秒），stop 时杀掉后 await 收尾。
    task: tokio::task::JoinHandle<()>,
}

pub(super) async fn screen_record(args: &Value, roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let action = args["action"].as_str().unwrap_or("start");
    let project_path = roots.first().map(String::as_str).unwrap_or("").to_string();

    let store = RECORD_STORE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));

    if action == "start" {
        let max = args["max_seconds"].as_u64().unwrap_or(60).clamp(1, 600);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dev_file = format!("/data/local/tmp/record_{ts}.mp4");

        // 快速探测设备是否支持 screenrecord（--help 立即返回，不阻塞）
        let probe = run_hdc_shell(&device, &["screenrecord", "--help"], 5).await;
        let supported = match &probe {
            Ok(o) => {
                let low = o.to_lowercase();
                !low.contains("not found") && !low.contains("unrecognized") && !low.contains("unknown")
            }
            Err(_) => false,
        };
        if !supported {
            return Err(format!(
                "启动录屏失败（设备 {device}）：设备不支持 screenrecord 命令。\n提示：可多次调用 take_screenshot/verify_ui 以多帧截图替代视频。"
            ));
        }

        let _ = run_hdc_shell(&device, &["rm", "-f", &dev_file], 5).await;
        // 后台执行：screenrecord 一直录到 --time-limit 上限才退出，
        // 同步 await 会把工具调用卡住 60~600 秒，且无法边录边执行 UI 操作。
        let d = device.clone();
        let df = dev_file.clone();
        let m = max;
        // 检查 + spawn + 登记同一锁内原子完成：并发 start 若在检查后插入，
        // 后一个会覆盖前一个 handle，导致第一次录屏失控（无法 stop）
        let mut guard = store.lock().map_err(|e| e.to_string())?;
        if guard.contains_key(&device) {
            return Err(format!("设备 {device} 已有进行中的录屏，先调用 action=stop 结束。"));
        }
        let task = tokio::spawn(async move {
            let _ = run_hdc_shell(
                &d,
                &["screenrecord", "--time-limit", &m.to_string(), "--size", "1080x1920", &df],
                m + 10,
            )
            .await;
        });
        guard.insert(device.clone(), RecordHandle { device_file: dev_file, task });
        Ok(format!("录屏已开始（设备 {device}，最大时长 {max}s），用 screen_record action=stop 结束并保存视频到工程目录。"))
    } else if action == "stop" {
        let handle = {
            let m = store.lock().ok();
            m.and_then(|mut g| g.remove(&device))
        };
        let Some(h) = handle else {
            return Err(format!("当前设备 {device} 没有进行中的录屏，先调用 action=start 开始。"));
        };

        // 停止录屏：SIGINT 结束 screenrecord，等待后台任务收尾（文件 flush）
        let _ = run_hdc_shell(&device, &["pkill", "-2", "screenrecord"], 5).await;
        tokio::time::sleep(Duration::from_millis(800)).await;
        let _ = h.task.await;

        // 拉到本地：.deveco-agent 目录（与截图口径一致），文件名毫秒+设备号（多设备不覆盖）
        let local_dir = if project_path.is_empty() {
            std::env::temp_dir().to_string_lossy().to_string()
        } else {
            Path::new(&project_path)
                .join(".deveco-agent")
                .to_string_lossy()
                .to_string()
        };
        std::fs::create_dir_all(&local_dir).ok();
        let ts_ms = chrono::Local::now().format("%Y%m%d-%H%M%S%3f");
        let dev_safe: String = device
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .take(32)
            .collect();
        let local_file = format!("{local_dir}/screen_record-{ts_ms}-{dev_safe}.mp4");

        let hdc_args: Vec<String> = vec![
            "-t".to_string(), device.clone(), "file".to_string(), "recv".to_string(),
            h.device_file.clone(), local_file.clone(),
        ];
        let recv = run_cmd("hdc", &hdc_args, None, 60).await;
        let ok = recv.is_ok() && std::path::Path::new(&local_file).exists();

        if ok {
            Ok(format!("录屏已保存（设备 {device}）\n本地路径：{local_file}\n可在资源管理器中播放查看。"))
        } else {
            Err(format!("录屏文件拉取失败（设备 {device}），视频可能未生成。"))
        }
    } else {
        Err("action 必须是 start 或 stop".into())
    }
}

// ---------- UI 录制回放 / HAP 包分析 / 日志搜索 / Lint / 弱网 / 签名 / 电量 / API兼容 / 自动遍历 ----------

static RECORD_UI_STORE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, RecordUiHandle>>> = std::sync::OnceLock::new();

#[derive(Clone)]
struct RecordUiHandle {
    device_file: String,
}

/// record_ui：开始/停止 UI 操作录制。
/// 清洗用户提供的文件名：去除路径分隔符与 Windows 非法字符，防止路径穿越；
/// 空/纯非法字符时回退 "default"，超长截断到 64 字符。
pub(super) fn safe_file_name(raw: &str) -> String {
    let cleaned: String = raw.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' { c }
            else if c.is_whitespace() { '_' }
            else { '-' }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-').trim_matches('_');
    if trimmed.is_empty() { "default".to_string() } else { trimmed.chars().take(64).collect() }
}

pub(super) async fn record_ui(args: &Value, roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let action = args["action"].as_str().unwrap_or("start");
    let name = safe_file_name(args["name"].as_str().unwrap_or("default"));
    let project_path = roots.first().map(String::as_str).unwrap_or("").to_string();

    let store = RECORD_UI_STORE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let store_key = format!("{device}|{name}");

    if action == "start" {
        if let Ok(m) = store.lock() {
            if m.contains_key(&store_key) {
                return Err(format!("设备 {device} 已有名为 \"{name}\" 的录制进行中，请先调用 record_ui action=stop 结束。"));
            }
        }
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let dev_file = format!("/data/local/tmp/ui_record_{ts}.csv");

        let _ = run_hdc_shell(&device, &["rm", "-f", &dev_file], 5).await;
        // 启动 uiRecord 录制（后台执行）
        let out = run_hdc_shell(&device, &["uitest", "uiRecord", "record", "-p", &dev_file], 5).await;
        match out {
            Ok(o) if !o.to_lowercase().contains("not found") && !o.to_lowercase().contains("fail") => {}
            Ok(o) => return Err(format!("启动 UI 录制失败：{o}")),
            Err(e) => return Err(format!("启动 UI 录制失败：{e}")),
        }

        // 登记与检查同一锁内：并发 start 在检查后登记会互相覆盖 handle，
        // 导致前一个录制 stop 时找不到；这里双重检查后原子插入
        let mut guard = store.lock().map_err(|e| e.to_string())?;
        if guard.contains_key(&store_key) {
            return Err(format!("设备 {device} 已有名为 \"{name}\" 的录制进行中，请先调用 record_ui action=stop 结束。"));
        }
        guard.insert(store_key, RecordUiHandle { device_file: dev_file });
        Ok(format!(
            "UI 录制已开始（设备 {device}，名称：{name}）\n请在设备上操作你想录制的流程，完成后调用 record_ui action=stop 结束录制。"
        ))
    } else if action == "stop" {
        let handle = {
            let m = store.lock().ok();
            m.and_then(|g| g.get(&store_key).cloned())
        };
        let Some(h) = handle else {
            return Err(format!("没有找到名称为 \"{name}\" 的录制，先调用 record_ui action=start 开始。"));
        };

        // 停止录制：发送停止指令
        let stop_out = run_hdc_shell(&device, &["uitest", "uiRecord", "stop"], 10).await;
        let _ = stop_out;
        tokio::time::sleep(Duration::from_millis(500)).await;

        // 把录制文件拉到本地
        let local_dir = if project_path.is_empty() {
            std::env::temp_dir().to_string_lossy().to_string()
        } else {
            format!("{project_path}/.deveco-agent/ui_records")
        };
        std::fs::create_dir_all(&local_dir).ok();
        let local_csv = format!("{local_dir}/{name}.csv");
        let local_json = format!("{local_dir}/{name}.json");

        let hdc_args: Vec<String> = vec![
            "-s".to_string(), device.clone(), "file".to_string(), "recv".to_string(),
            h.device_file.clone(), local_csv.clone(),
        ];
        let recv = run_cmd("hdc", &hdc_args, None, 30).await;
        if recv.is_err() || !std::path::Path::new(&local_csv).exists() {
            if let Ok(mut m) = store.lock() {
                m.remove(&store_key);
            }
            return Err(format!(
                "录制文件拉取失败（设备 {device}）。可能设备不支持 uitest uiRecord（模拟器常见），或录制文件已丢失。"
            ));
        }

        // 读取 csv 并解析为步骤 JSON
        let csv_content = std::fs::read_to_string(&local_csv).unwrap_or_default();
        let (steps, duration_ms) = parse_ui_record_csv(&csv_content);
        if steps.is_empty() {
            if let Ok(mut m) = store.lock() {
                m.remove(&store_key);
            }
            return Err(format!(
                "录制文件已拉取但未解析到任何操作步骤（{} 行）。可能 uitest uiRecord 输出格式与预期不符。原始文件：{local_csv}",
                csv_content.lines().count()
            ));
        }
        let json_out = serde_json::json!({
            "name": name,
            "device": device,
            "total_steps": steps.len(),
            "duration_ms": duration_ms,
            "steps": steps,
        });
        std::fs::write(&local_json, serde_json::to_string_pretty(&json_out).unwrap_or_default())
            .unwrap_or(());

        if let Ok(mut m) = store.lock() {
            m.remove(&store_key);
        }

        let mut out = format!("UI 录制完成（设备 {device}，名称 {name}）\n");
        out.push_str(&format!("共 {} 步，总时长约 {:.1} 秒\n", steps.len(), duration_ms as f64 / 1000.0));
        out.push_str(&format!("CSV 原始文件：{local_csv}\n"));
        out.push_str(&format!("JSON 步骤文件：{local_json}\n"));
        out.push_str("\n步骤预览（前 10 步）：\n");
        for (i, s) in steps.iter().take(10).enumerate() {
            out.push_str(&format!("  {}. {}\n", i + 1, s["desc"].as_str().unwrap_or("?")));
        }
        if steps.len() > 10 {
            out.push_str(&format!("  ... 还有 {} 步\n", steps.len() - 10));
        }
        out.push_str("\n使用 replay_ui 可回放此录制。");
        Ok(out)
    } else {
        Err("action 必须是 start 或 stop".into())
    }
}

/// 解析 uiRecord CSV 为步骤列表与总时长。
pub(super) fn parse_ui_record_csv(csv: &str) -> (Vec<serde_json::Value>, u64) {
    let mut steps: Vec<serde_json::Value> = Vec::new();
    let mut first_ts: Option<u64> = None;
    let mut last_ts: u64 = 0;

    for line in csv.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("time,") {
            continue;
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 2 {
            continue;
        }
        let ts: u64 = cols[0].parse().unwrap_or(0);
        if first_ts.is_none() {
            first_ts = Some(ts);
        }
        last_ts = ts;
        let action = cols[1];
        let step = match action {
            "click" | "tap" => {
                let x = cols.get(2).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                let y = cols.get(3).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                serde_json::json!({
                    "action": "tap",
                    "x": x, "y": y,
                    "desc": format!("点击 ({x}, {y})"),
                    "ts": ts,
                })
            }
            "swipe" => {
                let x1 = cols.get(2).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                let y1 = cols.get(3).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                let x2 = cols.get(4).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                let y2 = cols.get(5).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                serde_json::json!({
                    "action": "swipe",
                    "x1": x1, "y1": y1, "x2": x2, "y2": y2,
                    "desc": format!("滑动 ({x1},{y1}) → ({x2},{y2})"),
                    "ts": ts,
                })
            }
            "longClick" | "long_press" => {
                let x = cols.get(2).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                let y = cols.get(3).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                serde_json::json!({
                    "action": "long_press",
                    "x": x, "y": y,
                    "desc": format!("长按 ({x}, {y})"),
                    "ts": ts,
                })
            }
            "text" | "input" => {
                let t = cols.get(2).unwrap_or(&"").to_string();
                serde_json::json!({
                    "action": "text",
                    "text": t,
                    "desc": format!("输入「{t}」"),
                    "ts": ts,
                })
            }
            "keyEvent" | "key" => {
                let k = cols.get(2).unwrap_or(&"").to_string();
                serde_json::json!({
                    "action": "key",
                    "name": k,
                    "desc": format!("按键 {k}"),
                    "ts": ts,
                })
            }
            _ => continue,
        };
        steps.push(step);
    }
    let duration = if last_ts > 0 && first_ts.is_some() {
        last_ts - first_ts.unwrap_or(0)
    } else {
        0
    };
    (steps, duration)
}

/// replay_ui：回放录制的 UI 操作。
pub(super) async fn replay_ui(args: &Value, roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let project_path = roots.first().map(String::as_str).unwrap_or("").to_string();
    let speed = args["speed"].as_f64().unwrap_or(1.0).clamp(0.25, 4.0);

    // 读取步骤文件
    let steps = if let Some(path) = args["path"].as_str() {
        let resolved = resolve_in_roots(roots, path)?;
        let text = std::fs::read_to_string(&resolved).map_err(|e| format!("读取步骤文件失败: {e}"))?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("解析步骤 JSON 失败: {e}"))?;
        v["steps"].as_array().cloned().unwrap_or_default()
    } else if let Some(name) = args["name"].as_str() {
        let name = safe_file_name(name);
        let json_path = format!("{project_path}/.deveco-agent/ui_records/{name}.json");
        if !std::path::Path::new(&json_path).exists() {
            return Err(format!("录制文件不存在：{json_path}。请先用 record_ui 录制。"));
        }
        let text = std::fs::read_to_string(&json_path).map_err(|e| format!("读取录制文件失败: {e}"))?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("解析录制 JSON 失败: {e}"))?;
        v["steps"].as_array().cloned().unwrap_or_default()
    } else {
        return Err("replay_ui 需要参数 name 或 path".into());
    };

    if steps.is_empty() {
        return Err("没有可回放的步骤".into());
    }

    let mut results: Vec<String> = Vec::new();
    let mut prev_ts: Option<u64> = None;
    for (i, step) in steps.iter().enumerate() {
        // 按录制时间间隔等待（除以速度倍率）
        if let Some(prev) = prev_ts {
            let ts = step["ts"].as_u64().unwrap_or(prev);
            let diff = ts.saturating_sub(prev);
            if diff > 0 {
                let wait_ms = (diff as f64 / speed) as u64;
                if wait_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(wait_ms.min(5000))).await;
                }
            }
        }
        prev_ts = step["ts"].as_u64();

        let desc = step["desc"].as_str().unwrap_or(&super::test_tools::describe_step(step)).to_string();
        match super::test_tools::execute_ui_step(&device, step).await {
            Ok(info) => {
                let suffix = if info.is_empty() { String::new() } else { format!("（{info}）") };
                results.push(format!("{}. {desc} → 成功{suffix}", i + 1));
            }
            Err(e) => {
                results.push(format!("{}. {desc} → 失败：{e}", i + 1));
                results.push("（后续步骤已跳过）".to_string());
                break;
            }
        }
    }

    let mut out = format!("UI 回放完成（设备 {device}，共 {} 步，速度 {speed}x）\n", steps.len());
    for r in &results {
        out.push_str(&format!("{r}\n"));
    }
    Ok(out)
}

/// [54] gesture_perform：单次触摸/输入手势注入（tap/swipe/longPress/doubleTap/text/key）。
/// 坐标可直接使用 ui_locator 输出中的推荐点击坐标（bounds 中心点）。
pub(super) async fn gesture_perform(args: &Value, roots: &[String]) -> Result<String, String> {
    let device = match args["device"].as_str() {
        Some(d) => d.to_string(),
        None => default_device_id().await?,
    };
    let action = args["action"].as_str().ok_or("需要参数 {\"action\":\"tap|swipe|longPress|doubleTap|text|key\", ...}")?;
    let _ = roots;
    let mut step = serde_json::json!({});
    match action {
        "tap" | "click" => {
            let x = args["x"].as_i64().ok_or("tap 需要参数 x（像素坐标）")?;
            let y = args["y"].as_i64().ok_or("tap 需要参数 y（像素坐标）")?;
            step = serde_json::json!({"action": "tap", "x": x, "y": y});
        }
        "swipe" => {
            let x1 = args["x1"].as_i64().ok_or("swipe 需要参数 x1/y1（起点）")?;
            let y1 = args["y1"].as_i64().ok_or("swipe 需要参数 y1")?;
            let x2 = args["x2"].as_i64().ok_or("swipe 需要参数 x2/y2（终点）")?;
            let y2 = args["y2"].as_i64().ok_or("swipe 需要参数 y2")?;
            let speed = args["speed"].as_i64().unwrap_or(600);
            step = serde_json::json!({"action": "swipe", "x1": x1, "y1": y1, "x2": x2, "y2": y2, "speed": speed});
        }
        "longPress" | "long_press" => {
            let x = args["x"].as_i64().ok_or("longPress 需要参数 x/y（像素坐标）")?;
            let y = args["y"].as_i64().ok_or("longPress 需要参数 y")?;
            step = serde_json::json!({"action": "long_press", "x": x, "y": y});
        }
        "doubleTap" | "double_tap" => {
            let x = args["x"].as_i64().ok_or("doubleTap 需要参数 x/y（像素坐标）")?;
            let y = args["y"].as_i64().ok_or("doubleTap 需要参数 y")?;
            // 双击：两次 tap，间隔 80ms（就地执行，不回传 step）
            for _ in 0..2 {
                super::test_tools::execute_ui_step(
                    &device,
                    &serde_json::json!({"action": "tap", "x": x, "y": y}),
                )
                .await?;
                tokio::time::sleep(Duration::from_millis(80)).await;
            }
            return Ok(format!("双击完成（设备 {device}，坐标 {x},{y}）"));
        }
        "text" => {
            let t = args["text"].as_str().ok_or("text 需要参数 text（要输入的文本）")?;
            step = serde_json::json!({"action": "text", "text": t});
        }
        "key" => {
            let name = args["name"].as_str().unwrap_or("back");
            step = serde_json::json!({"action": "key", "name": name});
        }
        other => {
            return Err(format!(
                "未知 action \"{other}\"。可用：tap/swipe/longPress/doubleTap/text/key"
            ))
        }
    }
    super::test_tools::execute_ui_step(&device, &step).await.map(|info| {
        let mut out = format!("手势已执行（设备 {device}，action={action}）\n");
        if !info.is_empty() {
            out.push_str(&format!("补充：{info}\n"));
        }
        out
    })
}

/// analyze_hap_size：分析 HAP 包大小构成。
pub(super) async fn analyze_hap_size(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("").to_string();
    let top_n = args["top"].as_u64().unwrap_or(15) as usize;

    let hap_path = match args["path"].as_str() {
        Some(p) => {
            let resolved = resolve_in_roots(roots, p)?;
            resolved.to_string_lossy().to_string()
        }
        None => {
            if project_path.is_empty() {
                return Err("未指定 HAP 路径且当前会话未绑定工程".into());
            }
            // 自动查找最新 HAP
            let info = crate::services::harmony::parse_project(Path::new(&project_path));
            match crate::services::harmony::find_latest_hap(Path::new(&project_path), info.hap_output_dir.as_deref()) {
                Some(p) => p.to_string_lossy().to_string(),
                None => return Err("未找到 HAP 产物，请先 build_project 构建".into()),
            }
        }
    };

    let file = std::fs::File::open(&hap_path).map_err(|e| format!("打开 HAP 文件失败: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("解析 HAP/ZIP 失败: {e}"))?;

    let mut total_size: u64 = 0;
    let mut category_sizes: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    let mut file_sizes: Vec<(String, u64)> = Vec::new();

    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(|e| format!("读取 zip 条目失败: {e}"))?;
        let size = entry.size();
        let name = entry.name().to_string();
        if entry.is_dir() || size == 0 {
            continue;
        }
        total_size += size;
        file_sizes.push((name.clone(), size));
        let cat = classify_hap_entry(&name);
        *category_sizes.entry(cat).or_insert(0) += size;
    }

    file_sizes.sort_by(|a, b| b.1.cmp(&a.1));

    let mut out = format!("HAP 包大小分析报告\n");
    out.push_str(&format!("文件：{hap_path}\n"));
    out.push_str(&format!("总大小：{}（{:.2} MB）\n", format_bytes(total_size), total_size as f64 / (1024.0 * 1024.0)));
    out.push_str(&format!("文件数：{}\n\n", file_sizes.len()));

    out.push_str("分类占比：\n");
    let mut sorted_cats: Vec<(&String, &u64)> = category_sizes.iter().collect();
    sorted_cats.sort_by(|a, b| b.1.cmp(a.1));
    for (cat, sz) in &sorted_cats {
        let pct = if total_size > 0 { (**sz as f64 / total_size as f64) * 100.0 } else { 0.0 };
        let bar_len = (pct / 5.0).round() as usize;
        let bar: String = "█".repeat(bar_len) + &"░".repeat(20 - bar_len);
        out.push_str(&format!("  {bar} {cat:<20} {pct:>5.1}%  {}\n", format_bytes(**sz)));
    }

    out.push_str(&format!("\nTop {top_n} 大文件：\n"));
    for (i, (name, sz)) in file_sizes.iter().take(top_n).enumerate() {
        out.push_str(&format!("  {:>2}. {}  {}\n", i + 1, format_bytes(*sz), shorten_path(name, 80)));
    }

    out.push_str("\n瘦身建议：\n");
    let suggestions = gen_size_suggestions(&category_sizes, &file_sizes);
    for s in &suggestions {
        out.push_str(&format!("  • {s}\n"));
    }
    Ok(out)
}

/// [34] size_diff：对比两个 HAP 包（或同一工程两次构建产物）的大小构成差异。
/// 输出总大小变化、分类占比变化、文件级增删/变大/变小 Top 清单，
/// 用于定位“这次构建为什么大了 X MB”。
pub(super) fn size_diff(args: &Value, roots: &[String]) -> Result<String, String> {
    let raw_a = args["path_a"].as_str().ok_or("需要参数 {\"path_a\":\"<基线 HAP>\",\"path_b\":\"<新 HAP>\"}")?;
    let raw_b = args["path_b"].as_str().ok_or("需要参数 path_b（新 HAP 路径）")?;
    let top_n = args["top"].as_u64().unwrap_or(10) as usize;
    let pa = resolve_in_roots(roots, raw_a)?;
    let pb = resolve_in_roots(roots, raw_b)?;
    let (total_a, cats_a, files_a) = scan_hap_sizes(&pa)?;
    let (total_b, cats_b, files_b) = scan_hap_sizes(&pb)?;

    let mut out = String::new();
    out.push_str(&format!("HAP 大小对比：\n  {}  {}（{:.2} MB）\n  {}  {}（{:.2} MB）\n",
        pa.display(), format_bytes(total_a), total_a as f64 / (1024.0 * 1024.0),
        pb.display(), format_bytes(total_b), total_b as f64 / (1024.0 * 1024.0)));
    let delta = total_b as i64 - total_a as i64;
    let sign = if delta >= 0 { "+" } else { "-" };
    out.push_str(&format!("总大小变化：{sign}{}（{:.2} MB，{}{:.2}%）\n\n",
        format_bytes(delta.unsigned_abs()),
        delta.unsigned_abs() as f64 / (1024.0 * 1024.0),
        if delta >= 0 { "+" } else { "-" },
        if total_a > 0 { delta as f64 / total_a as f64 * 100.0 } else { 0.0 }));

    // 分类变化
    out.push_str("分类变化：\n");
    let mut all_cats: Vec<String> = cats_a.keys().chain(cats_b.keys()).cloned().collect();
    all_cats.sort();
    all_cats.dedup();
    for cat in all_cats {
        let sa = cats_a.get(&cat).copied().unwrap_or(0);
        let sb = cats_b.get(&cat).copied().unwrap_or(0);
        if sb == sa {
            continue;
        }
        let d = sb as i64 - sa as i64;
        let mark = if d > 0 { "▲" } else { "▼" };
        out.push_str(&format!("  {mark} {cat:<20} {} → {}（{}）\n",
            format_bytes(sa), format_bytes(sb),
            if d > 0 { format!("+{}", format_bytes(d as u64)) } else { format!("-{}", format_bytes(d.unsigned_abs())) }));
    }

    // 文件级：新增 / 删除 / 变大 / 变小
    let map_a: std::collections::HashMap<&str, u64> = files_a.iter().map(|(n, s)| (n.as_str(), *s)).collect();
    let map_b: std::collections::HashMap<&str, u64> = files_b.iter().map(|(n, s)| (n.as_str(), *s)).collect();
    let mut added: Vec<(&str, u64)> = map_b.iter().filter(|(n, _)| !map_a.contains_key(*n)).map(|(n, s)| (*n, *s)).collect();
    let mut removed: Vec<(&str, u64)> = map_a.iter().filter(|(n, _)| !map_b.contains_key(*n)).map(|(n, s)| (*n, *s)).collect();
    let mut grew: Vec<(&str, i64)> = map_a
        .iter()
        .filter_map(|(n, sa)| map_b.get(n).map(|sb| (*n, *sb as i64 - *sa as i64)))
        .filter(|(_, d)| *d > 0)
        .collect();
    let mut shrank: Vec<(&str, i64)> = map_a
        .iter()
        .filter_map(|(n, sa)| map_b.get(n).map(|sb| (*n, *sb as i64 - *sa as i64)))
        .filter(|(_, d)| *d < 0)
        .collect();
    added.sort_by(|a, b| b.1.cmp(&a.1));
    removed.sort_by(|a, b| b.1.cmp(&a.1));
    grew.sort_by(|a, b| b.1.cmp(&a.1));
    shrank.sort_by(|a, b| a.1.cmp(&b.1));

    out.push_str(&format!("\n新增文件（{} 个）：\n", added.len()));
    for (n, s) in added.iter().take(top_n) {
        out.push_str(&format!("  + {}  {}\n", format_bytes(*s), shorten_path(n, 80)));
    }
    if added.len() > top_n {
        out.push_str(&format!("  … 其余 {} 个\n", added.len() - top_n));
    }
    out.push_str(&format!("\n删除文件（{} 个）：\n", removed.len()));
    for (n, s) in removed.iter().take(top_n) {
        out.push_str(&format!("  − {}  {}\n", format_bytes(*s), shorten_path(n, 80)));
    }
    if removed.len() > top_n {
        out.push_str(&format!("  … 其余 {} 个\n", removed.len() - top_n));
    }
    out.push_str(&format!("\n变大 Top {}：\n", top_n.min(grew.len())));
    for (n, d) in grew.iter().take(top_n) {
        out.push_str(&format!("  ▲ +{}  {}\n", format_bytes(*d as u64), shorten_path(n, 80)));
    }
    out.push_str(&format!("\n变小 Top {}：\n", top_n.min(shrank.len())));
    for (n, d) in shrank.iter().take(top_n) {
        out.push_str(&format!("  ▼ −{}  {}\n", format_bytes(d.unsigned_abs()), shorten_path(n, 80)));
    }

    // 结论
    if delta > 0 && !grew.is_empty() {
        let top = grew.first().unwrap();
        out.push_str(&format!("\n主要增长来源：{}（+{}）→ 优先检查该文件是否可压缩/按需加载。\n",
            shorten_path(top.0, 60), format_bytes(top.1 as u64)));
    } else if delta < 0 {
        out.push_str("\n包体变小，无异常增长。\n");
    }
    Ok(out)
}

/// 扫描单个 HAP 的 (总大小, 分类大小, 文件清单)
fn scan_hap_sizes(path: &std::path::Path) -> Result<(u64, std::collections::BTreeMap<String, u64>, Vec<(String, u64)>), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开 {} 失败: {e}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("解析 HAP/ZIP {} 失败: {e}", path.display()))?;
    let mut total: u64 = 0;
    let mut cats: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    let mut files: Vec<(String, u64)> = Vec::new();
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(|e| format!("读取 zip 条目失败: {e}"))?;
        let size = entry.size();
        let name = entry.name().to_string();
        if entry.is_dir() || size == 0 {
            continue;
        }
        total += size;
        files.push((name.clone(), size));
        *cats.entry(classify_hap_entry(&name)).or_insert(0) += size;
    }
    Ok((total, cats, files))
}

/// [53] ui_locator：按文字/类型在设备当前界面控件树中定位元素，返回坐标与可点击信息。
/// 数据来源：path 参数给本地 dumpLayout JSON（离线复用），或现场 hdc 采集后自动清理。
/// 输出匹配项清单 + 推荐项中心坐标（可直接给 run_ui_flow 的 tap 使用）。
pub(super) async fn ui_locator(args: &Value, roots: &[String]) -> Result<String, String> {
    let text = args["text"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let ctype = args["type"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let index = args["index"].as_u64().unwrap_or(0) as usize;
    if text.is_none() && ctype.is_none() {
        return Err("需要筛选条件：{\"text\":\"<文字>\"} 或 {\"type\":\"<控件类型>\"}，可选 {\"index\":<第几个匹配>,\"path\":\"<本地控件树 JSON>\"}".into());
    }
    // 1. 获取控件树 JSON 文本
    let json_text = match args["path"].as_str() {
        Some(p) => {
            let resolved = resolve_in_roots(roots, p)?;
            std::fs::read_to_string(&resolved).map_err(|e| format!("读取 {} 失败: {e}", resolved.display()))?
        }
        None => {
            let device = match args["device"].as_str() {
                Some(d) => d.to_string(),
                None => default_device_id().await?,
            };
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let dev_file = format!("/data/local/tmp/ui_dump_{}.json", ts);
            run_hdc_shell(&device, &["uitest", "dumpLayout", "-p", &dev_file], 30).await
                .map_err(|e| format!("控件树导出失败：{e}"))?;
            let tmp = std::env::temp_dir().join(format!("ui_dump_{ts}.json"));
            let hdc_args = vec![
                "-s".to_string(), device.clone(), "file".to_string(), "recv".to_string(),
                dev_file.clone(), tmp.to_string_lossy().to_string(),
            ];
            run_cmd("hdc", &hdc_args, None, 30).await
                .map_err(|e| format!("拉取控件树失败: {e}"))?;
            let content = std::fs::read_to_string(&tmp).map_err(|e| format!("读取控件树失败: {e}"))?;
            let _ = std::fs::remove_file(&tmp);
            content
        }
    };
    // 2. 解析节点（递归收集 attributes 节点）
    let root: serde_json::Value =
        serde_json::from_str(&json_text).map_err(|e| format!("控件树 JSON 解析失败: {e}"))?;
    let mut nodes: Vec<UiNode> = Vec::new();
    collect_ui_nodes(&root, &mut nodes);
    if nodes.is_empty() {
        return Err("控件树为空或格式不识别（确认是 uitest dumpLayout 输出）".into());
    }
    // 3. 按 text（部分匹配，含 content-desc）与 type（忽略大小写）过滤
    let matched: Vec<&UiNode> = nodes
        .iter()
        .filter(|n| {
            let t_ok = text.as_deref().map(|t| n.text.contains(t) || n.desc.contains(t)).unwrap_or(true);
            let c_ok = ctype.as_deref().map(|c| n.ctype.eq_ignore_ascii_case(c)).unwrap_or(true);
            t_ok && c_ok
        })
        .collect();
    if matched.is_empty() {
        let cond = match (&text, &ctype) {
            (Some(t), Some(c)) => format!("type={c} 且文字含「{t}」"),
            (Some(t), None) => format!("文字含「{t}」"),
            (None, Some(c)) => format!("type={c}"),
            (None, None) => String::new(),
        };
        return Ok(format!(
            "未找到匹配控件（共解析 {} 个节点，条件：{cond}）。\n建议：① 文字支持部分匹配，可换更短的关键字 ② 用 dump_ui_hierarchy 查看实际控件类型与文字 ③ 界面可能未加载完，稍后重试。",
            nodes.len()
        ));
    }
    // 4. 输出清单（最多 10 个）+ 推荐项
    let mut out = format!("控件定位成功：条件 {}，共 {} 个匹配（共 {} 节点）\n",
        match (&text, &ctype) {
            (Some(t), Some(c)) => format!("type={c} 且文字含「{t}」"),
            (Some(t), None) => format!("文字含「{t}」"),
            (None, Some(c)) => format!("type={c}"),
            (None, None) => String::new(),
        },
        matched.len(),
        nodes.len());
    for (i, n) in matched.iter().take(10).enumerate() {
        let b = match n.bounds {
            Some((x1, y1, x2, y2)) => format!("[{x1},{y1}][{x2},{y2}] → 中心 ({}, {})", (x1 + x2) / 2, (y1 + y2) / 2),
            None => "无坐标".to_string(),
        };
        out.push_str(&format!(
            "  [{i}] {} 文字「{}」{} {} {}\n",
            n.ctype,
            truncate_text(&n.text, 24),
            if n.clickable { "[可点击]" } else { "" },
            if n.desc.is_empty() { String::new() } else { format!("desc「{}」", truncate_text(&n.desc, 24)) },
            b
        ));
    }
    if matched.len() > 10 {
        out.push_str(&format!("  … 其余 {} 个匹配\n", matched.len() - 10));
    }
    // 推荐项（index 指定或第一个可点击）
    let pick = matched.get(index).or_else(|| matched.iter().find(|n| n.clickable)).unwrap_or(&matched[0]);
    if let Some((x1, y1, x2, y2)) = pick.bounds {
        let (cx, cy) = ((x1 + x2) / 2, (y1 + y2) / 2);
        out.push_str(&format!(
            "\n推荐点击坐标：({cx}, {cy})（{}\n调用 run_ui_flow {{\"action\":\"tap\",\"x\":{cx},\"y\":{cy},\"desc\":\"点击 {}\"}} 执行。",
            match &pick.text {
                t if !t.is_empty() => format!("文字「{}」）", truncate_text(t, 20)),
                _ => format!("type={}）", pick.ctype),
            },
            truncate_text(&pick.text, 20)
        ));
    } else {
        out.push_str("\n匹配项无坐标信息（容器节点），可尝试更具体的文字/类型条件。\n");
    }
    Ok(out)
}

/// 控件树节点（扁平化后的最小信息）
struct UiNode {
    ctype: String,
    text: String,
    desc: String,
    bounds: Option<(i32, i32, i32, i32)>,
    clickable: bool,
}

/// 递归收集控件树 JSON 中所有 attributes 节点（兼容顶层 attributes / node 包装两种格式）。
fn collect_ui_nodes(value: &serde_json::Value, out: &mut Vec<UiNode>) {
    if let Some(attrs) = value.get("attributes") {
        let get = |k: &str| attrs.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let bounds = parse_bounds(&get("bounds"));
        out.push(UiNode {
            ctype: get("type"),
            text: get("text"),
            desc: get("content-desc"),
            bounds,
            clickable: get("clickable") == "true" || get("clickable") == "1",
        });
    }
    if let Some(arr) = value.as_array() {
        for c in arr {
            collect_ui_nodes(c, out);
        }
    }
    if let Some(obj) = value.as_object() {
        for (_, v) in obj {
            if v.is_array() || v.is_object() {
                collect_ui_nodes(v, out);
            }
        }
    }
}

/// 解析 "[x1,y1][x2,y2]" 形式 bounds → (x1, y1, x2, y2)
fn parse_bounds(s: &str) -> Option<(i32, i32, i32, i32)> {
    let nums: Vec<i32> = s
        .split(|c: char| !c.is_ascii_digit() && c != '-')
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse().ok())
        .collect();
    if nums.len() >= 4 {
        Some((nums[0], nums[1], nums[2], nums[3]))
    } else {
        None
    }
}

/// 截断文本（按字符数，超长加省略号）
fn truncate_text(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

pub(super) fn classify_hap_entry(path: &str) -> String {
    let lower = path.to_lowercase();
    if lower.starts_with("ets/") || lower.ends_with(".abc") {
        "ArkTS 字节码".to_string()
    } else if lower.starts_with("resources/") || lower.starts_with("entry/resources/") {
        if lower.contains("/media/") || lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg") || lower.ends_with(".webp") || lower.ends_with(".gif") {
            "图片资源".to_string()
        } else if lower.ends_with(".json") || lower.ends_with(".json5") {
            "配置资源".to_string()
        } else {
            "其他资源".to_string()
        }
    } else if lower.starts_with("libs/") || lower.ends_with(".so") {
        "原生库 (so)".to_string()
    } else if lower.starts_with("assets/") {
        "assets 资源".to_string()
    } else if lower.ends_with(".json") || lower.ends_with(".json5") || lower.contains("config") {
        "配置文件".to_string()
    } else if lower.ends_with(".hap") || lower.ends_with(".app") {
        "嵌套包".to_string()
    } else {
        "其他".to_string()
    }
}

pub(super) fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.2} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

pub(super) fn shorten_path(path: &str, max_len: usize) -> String {
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= max_len {
        return path.to_string();
    }
    // 按字符（而非字节）切片，避免中文等多字节文件名在边界处被截断导致 panic
    let keep = max_len.saturating_sub(3);
    let head: String = chars[..keep / 2].iter().collect();
    let tail: String = chars[chars.len() - (keep - keep / 2)..].iter().collect();
    format!("{head}...{tail}")
}

pub(super) fn gen_size_suggestions(cats: &std::collections::BTreeMap<String, u64>, files: &[(String, u64)]) -> Vec<String> {
    let mut suggestions: Vec<String> = Vec::new();
    let total: u64 = cats.values().sum();
    if total == 0 {
        return suggestions;
    }
    if let Some(img) = cats.get("图片资源") {
        let pct = (*img as f64 / total as f64) * 100.0;
        if pct > 20.0 {
            suggestions.push(format!("图片资源占 {:.1}%，可考虑将大图转 WebP/AVIF、删除未用图片、用 @media 按密度切分资源", pct));
        }
    }
    if let Some(native) = cats.get("原生库 (so)") {
        let pct = (*native as f64 / total as f64) * 100.0;
        if pct > 25.0 {
            suggestions.push(format!("原生库占 {:.1}%，可考虑按 ABI 分包（arm64/arm/x86）、移除调试符号、使用动态下发", pct));
        }
    }
    if let Some(ets) = cats.get("ArkTS 字节码") {
        let pct = (*ets as f64 / total as f64) * 100.0;
        if pct > 30.0 {
            suggestions.push(format!("ArkTS 字节码占 {:.1}%，可考虑使用懒加载/按需导入、拆分包（HSP/ATP）减少主包体积", pct));
        }
    }
    let big_images: Vec<_> = files.iter().filter(|(n, _)| {
        let l = n.to_lowercase();
        l.ends_with(".png") || l.ends_with(".jpg") || l.ends_with(".jpeg")
    }).take(3).collect();
    if !big_images.is_empty() {
        let names: Vec<_> = big_images.iter().map(|(n, s)| format!("{} ({})", shorten_path(n, 40), format_bytes(*s))).collect();
        suggestions.push(format!("Top 大图片：{} → 检查是否可压缩或替换为矢量图", names.join("、")));
    }
    if suggestions.is_empty() {
        suggestions.push("包大小分布较健康，继续保持。若需进一步瘦身，可考虑：①资源按需分包 ②移除未用依赖 ③开启混淆/压缩".to_string());
    }
    suggestions
}

/// [31] screenshot_diff：逐像素对比两张截图（PNG），输出差异率、差异区域包围盒与位置提示。
/// 用于 UI 改动前后验证（先 take_screenshot 存基线，改动后再截一张对比）。
/// 纯只读：不写盘、不连设备，本地解码比较。
/// PNG 解码（上限 4096）+ 逐像素对比为 CPU 密集操作（千万级像素），整体放
/// spawn_blocking，避免钉死 tokio worker（timer driver 停转 → 流式超时全部失效）。
pub(super) async fn screenshot_diff(args: &Value, roots: &[String]) -> Result<String, String> {
    let raw_a = args["path_a"].as_str().ok_or("需要参数 {\"path_a\":\"<基线截图>\",\"path_b\":\"<变更截图>\",\"threshold\":<可选容差>}")?;
    let raw_b = args["path_b"].as_str().ok_or("需要参数 path_b（变更截图路径）")?;
    let threshold = args["threshold"].as_u64().unwrap_or(10) as i64;
    let roots_owned: Vec<String> = roots.to_vec();
    let raw_a_owned = raw_a.to_string();
    let raw_b_owned = raw_b.to_string();
    tokio::task::spawn_blocking(move || {
        let pa = crate::agent::tools::resolve_in_roots(&roots_owned, &raw_a_owned)?;
        let pb = crate::agent::tools::resolve_in_roots(&roots_owned, &raw_b_owned)?;
        if !pa.exists() || !pb.exists() {
            return Err(format!(
                "截图不存在：{} / {}",
                if pa.exists() { "".to_string() } else { pa.display().to_string() },
                if pb.exists() { "".to_string() } else { pb.display().to_string() }
            ));
        }
        let data_a = std::fs::read(&pa).map_err(|e| format!("读取 {} 失败: {e}", pa.display()))?;
        let data_b = std::fs::read(&pb).map_err(|e| format!("读取 {} 失败: {e}", pb.display()))?;
        let img_a = crate::utils::png::decode_png(&data_a, 4096).map_err(|e| format!("解析 {} 失败: {e}", pa.display()))?;
        let img_b = crate::utils::png::decode_png(&data_b, 4096).map_err(|e| format!("解析 {} 失败: {e}", pb.display()))?;
        if img_a.width != img_b.width || img_a.height != img_b.height {
            return Err(format!(
                "两张截图尺寸不一致：{} {}x{} vs {} {}x{}（先确认分辨率相同，或用 image_inspect 检查）",
                pa.display(),
                img_a.width,
                img_a.height,
                pb.display(),
                img_b.width,
                img_b.height
            ));
        }
        let w = img_a.width as usize;
        let h = img_a.height as usize;
        let rgb_a = &img_a.rgb;
        let rgb_b = &img_b.rgb;
        let mut diff_count = 0usize;
        let mut min_x = usize::MAX;
        let mut min_y = usize::MAX;
        let mut max_x = 0usize;
        let mut max_y = 0usize;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                let d = (rgb_a[i] as i64 - rgb_b[i] as i64).abs()
                    + (rgb_a[i + 1] as i64 - rgb_b[i + 1] as i64).abs()
                    + (rgb_a[i + 2] as i64 - rgb_b[i + 2] as i64).abs();
                if d > threshold {
                    diff_count += 1;
                    if x < min_x {
                        min_x = x;
                    }
                    if y < min_y {
                        min_y = y;
                    }
                    if x > max_x {
                        max_x = x;
                    }
                    if y > max_y {
                        max_y = y;
                    }
                }
            }
        }
        let total = w * h;
        if diff_count == 0 {
            return Ok(format!(
                "两张截图完全一致（{}x{}，共 {total} 像素，阈值 {threshold}），界面无变化。\n路径：{} | {}",
                w,
                h,
                pa.display(),
                pb.display()
            ));
        }
        // 差异区域粗定位：按上下左右半区归属
        let mid_x = w / 2;
        let mid_y = h / 2;
        let mut zone = Vec::new();
        if min_y <= mid_y {
            zone.push("上方");
        }
        if max_y >= mid_y {
            zone.push("下方");
        }
        if min_x <= mid_x {
            zone.push("左侧");
        }
        if max_x >= mid_x {
            zone.push("右侧");
        }
        let zone_desc = zone.join("、");
        let pct = diff_count as f64 / total as f64 * 100.0;
        Ok(format!(
            "截图对比完成（阈值 {threshold}）：差异像素 {diff_count} / {total}（{pct:.2}%）\n\
             差异区域包围盒：x {min_x}..{max_x}，y {min_y}..{max_y}（集中于{zone_desc}）\n\
             基线：{}\n变更：{}\n\
             判读：差异 <0.1% 多为状态栏时钟/滚动条等动态元素可忽略；\
             若与预期不符，用 dump_ui_hierarchy + verify_ui 定位具体控件。",
            pa.display(),
            pb.display()
        ))
    })
    .await
    .map_err(|e| format!("截图对比任务异常: {e}"))?
}
