//! 构建/部署/依赖域工具：build_project / deploy / deploy_all / ohpm 系列。
//! 共享辅助函数（run_cmd / run_hdc_shell / default_device_id / tail 等）仍定义在父模块 mod.rs，
//! 本模块通过 `use super::*` 继承访问。

use super::*;

/// 构建请求（宽松）：LLM 直接给出的参数，字段均可选；默认值与校验集中在 `resolve()` 显式落地。
#[derive(serde::Deserialize, Default)]
pub(super) struct BuildRequest {
    /// 构建模式：debug | release（缺省 debug）
    pub mode: Option<String>,
    /// 指定模块名（缺省用工程 entry 模块）
    pub module: Option<String>,
    /// 构建前先 hvigor clean 清理缓存（缺省 false）
    pub clean: Option<bool>,
}

impl BuildRequest {
    /// 从工具入参解析宽松请求：容忍未知字段与缺省字段，不在此处做业务校验。
    pub(super) fn from_args(args: &Value) -> Result<Self, String> {
        serde_json::from_value(args.clone()).map_err(|e| format!("build_project 参数解析失败：{e}"))
    }

    /// 显式 resolve：默认值落地 + 参数校验（mode 枚举、module 存在性），产出执行用严格规范。
    pub(super) fn resolve(self, root: &Path, entry_module: Option<&str>) -> Result<BuildSpec, String> {
        let mode = self.mode.unwrap_or_else(|| "debug".to_string());
        if mode != "debug" && mode != "release" {
            return Err("mode 仅支持 debug 或 release".into());
        }
        let module = match self.module.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(m) => {
                let available = harmony_modules(root);
                if !available.iter().any(|x| x == m) {
                    return Err(format!(
                        "指定模块 {m} 不存在，工程可构建模块：{}",
                        if available.is_empty() {
                            "(未能识别，请检查 build-profile.json5 的 modules 配置)".to_string()
                        } else {
                            available.join(", ")
                        }
                    ));
                }
                Some(m.to_string())
            }
            None => entry_module.map(|s| s.to_string()),
        };
        Ok(BuildSpec { mode, module, clean: self.clean.unwrap_or(false) })
    }
}

/// 构建规范（严格）：由 `BuildRequest::resolve()` 产出，默认值与校验已完成，run 内只消费本结构。
pub(super) struct BuildSpec {
    /// 已校验的构建模式（debug/release）
    pub mode: String,
    /// 已校验存在的模块名（None 表示交给构建系统按工程默认处理）
    pub module: Option<String>,
    /// 是否先执行 clean
    pub clean: bool,
}

pub(super) async fn build_project(
    args: &Value,
    roots: &[String],
    ctx: &crate::agent::exec_ctx::ToolCtx,
    _project_id: &str,
) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录，无法构建".into());
    }
    // 前置校验：目标目录必须是鸿蒙工程（混合工作区中由主工程解析兜底），避免对前端/后端目录误跑 hvigor
    if !crate::services::workspace::classify(Path::new(project_path))
        .is_some_and(|k| k == crate::services::workspace::ModuleKind::Harmony)
    {
        return Err(format!(
            "目标目录不是 HarmonyOS 工程（{}）：未找到 build-profile.json5 / oh-package.json5。\n若这是混合工作区（项目根下有多个子工程），请在工程分析面板指定鸿蒙主工程；其它语言工程请用 run_command 构建（如 npm run build / go build / cargo build）。",
            project_path
        ));
    }
    let project_id = project_id_for_path(ctx, project_path);
    let root = Path::new(project_path);
    // 解析工程以确定 entry 模块（多模块工程需要 --mode module）
    let info = crate::services::harmony::parse_project(root);
    // Request/Spec 分离：宽松参数 BuildRequest → 显式 resolve() 产出严格规范 BuildSpec
    // （默认值/校验集中于此，run 内不再出现隐式 ?? 默认）
    let spec = BuildRequest::from_args(args)?.resolve(root, info.entry_module.as_deref())?;
    let mode = spec.mode.as_str();
    let module = spec.module.as_deref();
    let cmd_args = crate::services::harmony::assemble_args(module, mode);
    // clean=true 时先执行 hvigor clean 清理缓存，用于缓存导致的诡异构建失败
    let do_clean = spec.clean;
    // 全局并发护栏：同一时间只允许一个构建（其他调用排队等待）
    let _gate = crate::services::tool_limits::acquire_gate("build_project").await;
    // 构建耗时统计（含 clean，供成功提示展示）
    let build_started = std::time::Instant::now();

    // 流式构建：日志逐行推送 agent:log 并落盘
    let log_path = crate::agent::exec_ctx::new_build_log_path(project_path);
    // Windows 下优先 node 直调 hvigor-wrapper.js 绕过 cmd/.bat 弹窗；找不到时回退工程内 hvigorw.bat，
    // 再兜底 DevEco Studio 内置 hvigor 工具链（工程缺构建脚本时仍可构建）；
    // env 自动注入 DEVECO_SDK_HOME（未设置且探测到 DevEco 内置 SDK 时），否则 hvigor 报 00303217/00303312
    let hvigor = crate::services::harmony::hvigor_command(root)
        .map_err(|e| with_advice("build_project", e))?;
    let program = hvigor.program;
    let prefix = hvigor.args;
    let envs = if hvigor.env.is_empty() {
        None
    } else {
        Some(hvigor.env.as_slice())
    };
    // 预检：用户显式设置的 DEVECO_SDK_HOME 若指向 sdk 内层目录（sdk\default 或其子目录）
    // 或路径不存在，hvigor 只扫描 {SDK_ROOT}/<子目录>/sdk-pkg.json，将必然报 00303217/00303312，
    // 提前给出明确警告，省一轮必然失败的构建。
    #[cfg(windows)]
    if let Ok(sdk_home) = std::env::var("DEVECO_SDK_HOME") {
        let p = Path::new(&sdk_home);
        if !p.join("default").join("sdk-pkg.json").is_file()
            && (p.join("sdk-pkg.json").is_file() || p.join("openharmony").is_dir() || !p.exists())
        {
            ctx.emit_log(
                "system",
                &format!(
                    "警告：DEVECO_SDK_HOME 当前为 {}，指向可能有误。\n应指向 sdk 根目录（含 default\\sdk-pkg.json 的父目录），如 C:\\Program Files\\Huawei\\DevEco Studio\\sdk；否则 hvigor 将报 00303312 找不到 SDK 组件。",
                    p.display()
                ),
            );
        }
    }
    // 可选：先 clean 清理构建缓存
    if do_clean {
        let mut clean_full = prefix.clone();
        clean_full.extend(crate::services::harmony::clean_args());
        ctx.emit_log("system", &format!("清理构建缓存：{program} {}", clean_full.join(" ")));
        match crate::agent::exec_ctx::run_cmd_streaming_env(
            ctx, &program, &clean_full, Some(root), 120, None, envs,
        ).await {
            Ok(o) if o.status.success() => {
                ctx.emit_log("system", "缓存清理完成，开始构建");
            }
            Ok(o) => {
                let err = smart_decode(&o.stderr);
                ctx.emit_log("system", &format!("清理缓存失败（继续构建）：{}", tail(&err, 500)));
            }
            Err(e) => {
                ctx.emit_log("system", &format!("清理缓存异常（继续构建）：{e}"));
            }
        }
    }
    let mut full_args = prefix;
    full_args.extend(cmd_args);
    ctx.emit_log("system", &format!("开始构建（{mode}）：{program} {}", full_args.join(" ")));
    let output = crate::agent::exec_ctx::run_cmd_streaming_env(
        ctx,
        &program,
        &full_args,
        Some(root),
        600,
        Some(&log_path),
        envs,
    )
    .await;
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            ctx.emit_log("system", &format!("构建异常：{e}"));
            return Err(with_advice("build_project", e));
        }
    };
    let stdout = smart_decode(&output.stdout);
    let stderr = smart_decode(&output.stderr);
    let combined = if stderr.trim().is_empty() {
        stdout.clone()
    } else if stdout.trim().is_empty() {
        stderr.clone()
    } else {
        format!("{stdout}\n{stderr}")
    };

    if output.status.success() {
        let elapsed = build_started.elapsed().as_secs_f32();
        let mut summary = format!("构建成功（{mode}，耗时 {elapsed:.1}s）。\n");
        if let Some(dir) = &info.hap_output_dir {
            if dir.exists() {
                summary.push_str(&format!("产物目录: {}\n", dir.display()));
            }
        }
        summary.push_str(&format!("构建日志已保存: {}\n", log_path.display()));
        summary.push_str("如需部署到真机/模拟器，可调用 deploy 工具安装（需已签名且设备已连接）\n");
        ctx.emit_log("system", &format!("构建成功 ✓（耗时 {elapsed:.1}s）"));
        // 构建成功：清除该项目的历史构建失败归因；若此前有失败记录，
        // 说明本次修复成功，推送一条“修复经验候选”，前端可一键沉淀到知识库。
        let removed = crate::agent::diagnostics::clear_source(project_path, "build_project");
        if !removed.is_empty() {
            emit_knowledge_candidate(ctx, project_path, "build_project", &removed, &combined);
        }
        // 工具结果只回传尾部，避免上下文爆炸；完整日志可通过 get_build_log 读取
        summary.push_str(&tail(&combined, 2000));
        Ok(summary)
    } else {
        let errors = crate::services::harmony::parse_build_errors(&combined);
        ctx.emit_log("system", &format!("构建失败（退出码 {:?}）", output.status.code()));
        if errors.is_empty() {
            return Err(with_advice(
                "build_project",
                structured_tool_error(
                    "build_project",
                    "build_failed",
                    &format!("{mode} 构建失败，但日志未解析出结构化错误"),
                    &[],
                    &["用 get_build_log 读取完整日志定位根因，不要盲目重复构建"],
                    Some(&log_path.display().to_string()),
                    &tail(&combined, 1500),
                    &[],
                ),
            ));
        }
        // 按根因分类统计，取主导类别决定下一步
        let mut cat_count: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for e in &errors {
            *cat_count.entry(e.category.as_str()).or_insert(0) += 1;
        }
        let dominant = cat_count.iter().max_by_key(|(_, &v)| v).map(|(k, _)| *k);
        let locations: Vec<ErrorLocation> = errors
            .iter()
            .take(8)
            .map(|e| {
                let msg = if e.suggestion.trim().is_empty() {
                    e.message.clone()
                } else {
                    format!("{}（建议: {}）", e.message, e.suggestion)
                };
                ErrorLocation { file: e.file.clone(), line: e.line, message: msg }
            })
            .collect();
        let next: Vec<&str> = match dominant {
            Some("dependency") => vec![
                "调用 ohpm_install 安装缺失依赖",
                "若依赖声明有误，edit_file 修正 oh-package.json5",
                "重新 build_project 验证",
            ],
            Some("sdk") | Some("api_level") => vec![
                "调用 check_sdk_alignment 核对 compatibleSdkVersion 与已装 SDK",
                "缺失 API 用 show_diagnose_card(category=sdk) 提示用户安装",
            ],
            Some("signing") => vec![
                "不要改代码，调用 show_diagnose_card(category=signing) 引导用户在 DevEco Studio 配置签名",
            ],
            Some("ohpm") => vec![
                "调用 ohpm_install 验证 ohpm 是否可用",
                "必要时 show_diagnose_card 提示重装 ohpm 工具链",
            ],
            Some("type") | Some("syntax") => vec![
                "对每个定位条目用 read_file 读取对应文件行号",
                "用 edit_file 逐一修正语法/类型错误",
                "重新 build_project 验证",
            ],
            Some("resource") => vec![
                "检查 $r() 引用的资源名是否存在于 resources 目录",
                "缺失则补充资源或修正引用，再 build_project",
            ],
            _ => vec!["阅读定位与完整日志找到根因后再修复，不要直接重复相同构建"],
        };
        let dom_cat = dominant.unwrap_or("build_failed");
        crate::agent::diagnostics::record(
            project_path,
            crate::agent::diagnostics::Diagnosis {
                source: "build_project".into(),
                category: dom_cat.into(),
                summary: format!("{mode} 构建失败，{} 个错误（主导类别: {dom_cat}）", errors.len()),
                detail: locations.iter().take(5).map(|l| {
                    let pos = match (&l.file, l.line) {
                        (Some(f), Some(n)) => format!("{f}:{n}"),
                        (Some(f), None) => f.clone(),
                        _ => "未知位置".into(),
                    };
                    format!("{pos}: {}", l.message)
                }).collect::<Vec<_>>().join("\n"),
                at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0),
            },
        );
        let user_kb = load_user_knowledge(ctx, project_id.as_deref());
        let (matched, hit_ids) = crate::services::harmony_knowledge::match_knowledge_with_user(&combined, 3, &user_kb);
        for id in &hit_ids {
            bump_knowledge_hit(ctx, id);
        }
        let err = structured_tool_error(
            "build_project",
            dom_cat,
            &format!("{mode} 构建失败，检测到 {} 个错误（主导类别: {dom_cat}）", errors.len()),
            &locations,
            &next,
            Some(&log_path.display().to_string()),
            "",
            &matched,
        );
        Err(err)
    }
}

pub(super) fn harmony_modules(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    // 1) build-profile.json5 的 modules 数组
    if let Some(text) = std::fs::read_to_string(root.join("build-profile.json5")).ok() {
        let trimmed = text.trim();
        // 取 "modules": [ {"name": "xxx", ...} ... ] 中的 name 值（不引入 json5 解析器依赖）
        if let Some(idx) = trimmed.find("\"modules\"") {
            let rest = &trimmed[idx..];
            if let Some(start) = rest.find('[') {
                if let Some(end) = rest[start..].find(']') {
                    let arr = &rest[start + 1..start + end];
                    for seg in arr.split('{') {
                        if let Some(ni) = seg.find("\"name\"") {
                            let after = &seg[ni + "\"name\"".len()..];
                            if let Some(c) = after.find(':') {
                                let v = &after[c + 1..];
                                let v = v.trim().trim_start_matches('"');
                                if let Some(end_q) = v.find('"') {
                                    let n = v[..end_q].trim();
                                    if !n.is_empty() {
                                        names.push(n.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // 2) 回退：扫描直接子目录（含 oh-package.json5 且非 AppScope/.hvigor/.ohpm）
    if names.is_empty() {
        if let Ok(rd) = std::fs::read_dir(root) {
            for e in rd.flatten() {
                if !e.path().is_dir() {
                    continue;
                }
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') || name.eq_ignore_ascii_case("AppScope") {
                    continue;
                }
                if e.path().join("oh-package.json5").is_file() || e.path().join("build-profile.json5").is_file() {
                    names.push(name);
                }
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

pub(super) async fn deploy(
    args: &Value,
    roots: &[String],
    ctx: &crate::agent::exec_ctx::ToolCtx,
    project_id: &str,
) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录，无法部署".into());
    }
    let root = Path::new(project_path);
    let info = crate::services::harmony::parse_project(root);

    // 定位 hap：优先使用用户指定路径（相对路径基于项目根解析，进程 CWD 不是项目根），
    // 其次推导产物目录，最后递归查找
    let hap = if let Some(h) = args["hap"].as_str() {
        let p = PathBuf::from(h);
        if p.is_absolute() { p } else { root.join(p) }
            .to_string_lossy()
            .to_string()
    } else {
        crate::services::harmony::find_latest_hap(root, info.hap_output_dir.as_deref())
            .ok_or_else(|| "未找到 .hap 构建产物，请先执行 build_project".to_string())?
            .to_string_lossy()
            .to_string()
    };
    if !Path::new(&hap).exists() {
        return Err(format!("hap 文件不存在: {hap}"));
    }
    let is_signed = Path::new(&hap)
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.contains("-signed"));

    // 全局并发护栏：同一时间只允许一个部署
    let _gate = crate::services::tool_limits::acquire_gate("deploy").await;

    // 1. 选择设备：优先参数指定，否则取默认设备记忆 / 第一个在线设备
    let device_id = if let Some(d) = args["device"].as_str() {
        d.to_string()
    } else {
        default_device_id().await?
    };
    // per-device 门控：与 deploy_all 中同设备的任务互斥，不同设备不阻塞
    let _dev_gate = crate::services::tool_limits::acquire_named_gate(&format!("deploy:{device_id}")).await;
    ctx.emit_log("system", &format!("部署到设备: {device_id}"));
    let mut out = String::new();
    out.push_str(&format!("目标设备: {device_id}\n"));
    if let Some(b) = &info.bundle_name {
        out.push_str(&format!("应用包名: {b}\n"));
    }
    // 设备信息
    if let Ok(model) = run_hdc_shell(&device_id, &["param", "get", "const.product.model"], 15).await {
        let m = model.trim();
        if !m.is_empty() {
            out.push_str(&format!("设备型号: {m}\n"));
        }
    }
    if !is_signed {
        out.push_str("⚠ 未签名产物（unsigned），真机可能无法安装\n");
    }

    // 2. 冲突检测：查询是否已安装同包名
    let mut already_installed = false;
    if let Some(bundle) = &info.bundle_name {
        if let Ok(dump) = run_hdc_shell(&device_id, &["bm", "dump", "-n", bundle], 30).await {
            if dump.contains(bundle) && !dump.contains("not found") {
                already_installed = true;
            }
        }
    }
    if already_installed {
        out.push_str("检测到已安装同包名应用，执行覆盖安装（-r）\n");
        ctx.emit_log("system", "检测到已安装版本，覆盖安装");
    }

    // 3. 安装（流式推送安装输出）
    ctx.emit_log("system", &format!("安装 {}", Path::new(&hap).file_name().and_then(|s| s.to_str()).unwrap_or(&hap)));
    let install_args = if already_installed {
        vec!["-t".to_string(), device_id.clone(), "install".to_string(), "-r".to_string(), hap.clone()]
    } else {
        vec!["-t".to_string(), device_id.clone(), "install".to_string(), hap.clone()]
    };
    let install_out = crate::agent::exec_ctx::run_cmd_streaming(
        ctx, "hdc", &install_args, None, 300, None,
    )
    .await
    .map_err(|e| with_advice("deploy", e))?;
    let install_text = smart_decode(&install_out.stdout) + &smart_decode(&install_out.stderr);
    out.push_str(&install_text);
    if !install_out.status.success() {
        let (cat, msg) = classify_deploy_error(&install_text, is_signed);
        let user_kb = load_user_knowledge(ctx, Some(project_id));
        let (matched, hit_ids) = crate::services::harmony_knowledge::match_knowledge_with_user(&install_text, 2, &user_kb);
        for id in &hit_ids {
            bump_knowledge_hit(ctx, id);
        }
        let mut msg = msg;
        msg.push_str(&crate::services::harmony_knowledge::format_matched(&matched));
        crate::agent::diagnostics::record(
            project_path,
            crate::agent::diagnostics::Diagnosis {
                source: "deploy_hap".into(),
                category: cat.clone(),
                summary: format!("HAP 安装失败（{cat}）"),
                detail: tail(&install_text, 600),
                at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0),
            },
        );
        return Err(msg);
    }
    // 安装成功：清除该项目部署失败归因；若此前有失败，推送修复经验候选
    let removed = crate::agent::diagnostics::clear_source(project_path, "deploy_hap");
    if !removed.is_empty() {
        emit_knowledge_candidate(ctx, project_path, "deploy_hap", &removed, &install_text);
    }

    // 4. 拉起应用（鸿蒙用 aa start，不是 am start）
    let bundle = match info.bundle_name.as_deref() {
        Some(b) => b,
        None => {
            out.push_str("\n⚠ 未能从工程解析 bundleName，跳过自动拉起。请确认 AppScope/app.json5 配置。\n");
            return Ok(out);
        }
    };
    let ability = info.main_element.as_deref().unwrap_or("EntryAbility");
    ctx.emit_log("system", &format!("拉起应用: {bundle}/{ability}"));
    let start = run_hdc_shell(
        &device_id,
        &["aa", "start", "-b", bundle, "-a", ability],
        30,
    )
    .await
    .map_err(|e| format!("拉起失败: {e}"))?;
    out.push_str(&format!("\n拉起应用: aa start -b {bundle} -a {ability}\n"));
    out.push_str(&start);

    // 5. 启动后存活探测：在 8 秒内分 3 次检查 ability 栈，捕获"启动即崩"和"启动后短时间内崩溃"。
    //    只有全程存活才算部署成功；中途消失则抓取 faultlog + hilog 做结构化崩溃归因。
    ctx.emit_log("system", "拉起应用: 观察启动稳定性…");
    let mut alive_at = None;
    let mut crashed = false;
    for (idx, wait) in [2u64, 3, 3].iter().enumerate() {
        tokio::time::sleep(std::time::Duration::from_secs(*wait)).await;
        match run_hdc_shell(&device_id, &["aa", "dump", "-l"], 30).await {
            Ok(dump) if dump.contains(bundle) => {
                alive_at = Some(idx);
            }
            _ => {
                // 首次未起来或后续消失都视为崩溃
                crashed = true;
                break;
            }
        }
    }

    if !crashed && alive_at.is_some() {
        out.push_str(&format!("\n✓ 应用已在设备 {device_id} 启动并稳定运行。\n"));
        ctx.emit_log("system", "应用已启动并稳定运行 ✓");
        // 成功启动：挂起运行日志监听，把用户操作期间的 error/崩溃实时回流
        crate::agent::runtime_log::start(project_path, ctx, &device_id, bundle);
        ctx.emit_log("system", "已开启运行日志监听（error 级），运行期异常会自动回流");
        // 清除该项目的历史崩溃归因；若此前有崩溃记录则推送修复经验候选
        let removed = crate::agent::diagnostics::clear_source(project_path, "crash_analysis");
        if !removed.is_empty() {
            let crash_log = removed.iter().map(|d| format!("{}\n{}", d.summary, d.detail)).collect::<Vec<_>>().join("\n");
            emit_knowledge_candidate(ctx, project_path, "crash_analysis", &removed, &crash_log);
        }
    } else {
        out.push_str("\n❌ 应用启动后崩溃（未在 ability 栈中持续存活）。\n");
        ctx.emit_log("system", "应用启动后崩溃，正在抓取 faultlog 与 hilog…");

        // 优先拉 faultlog（结构化程度高），回退 hilog -x
        let faultlog = fetch_recent_faultlog(&device_id, bundle).await.unwrap_or_default();
        let hilog = run_hdc_shell(&device_id, &["hilog", "-x"], 25).await.unwrap_or_default();
        let report = crate::agent::crash::analyze(bundle, &faultlog, &hilog);

        // 写入跨轮诊断，下一轮 model 能看到"上次运行时崩溃是什么"
        crate::agent::diagnostics::record(
            project_path,
            crate::agent::diagnostics::Diagnosis {
                source: "crash_analysis".into(),
                category: report.category.clone(),
                summary: report.summary.clone(),
                detail: if report.locations.is_empty() {
                    tail(&report.snippet, 600)
                } else {
                    format!("定位: {}\n{}", report.locations.join(", "), tail(&report.snippet, 500))
                },
                at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0),
            },
        );

        // 匹配用户知识库（运行时崩溃经验），并累加命中
        let user_kb = load_user_knowledge(ctx, Some(project_id));
        let (matched, hit_ids) = crate::services::harmony_knowledge::match_knowledge_with_user(
            &format!("{}\n{}\n{}", report.summary, report.message, report.snippet),
            3,
            &user_kb,
        );
        for id in &hit_ids {
            bump_knowledge_hit(ctx, id);
        }

        let mut next = vec![report.advice.clone()];
        if !report.locations.is_empty() {
            next.insert(0, format!("根据以下定位读取并修复源码：{}", report.locations.join("; ")));
        }
        let err_locs: Vec<ErrorLocation> = report
            .locations
            .iter()
            .map(|l| {
                let (f, line) = match l.rsplit_once(':') {
                    Some((f, n)) => (Some(f.to_string()), n.parse::<i64>().ok()),
                    None => (Some(l.clone()), None),
                };
                ErrorLocation { file: f, line, message: report.message.clone() }
            })
            .collect();
        let err = structured_tool_error(
            "deploy_hap",
            &report.category,
            &report.summary,
            &err_locs,
            &next.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            None,
            &tail(&report.snippet, 1200),
            &matched,
        );
        out.push_str(&err);
        return Err(out);
    }
    // 记住本次使用的设备
    let _ = std::env::set_var("DEVECO_DEFAULT_DEVICE", &device_id);
    save_default_device(&device_id);
    Ok(out)
}

pub(super) async fn deploy_all(
    args: &Value,
    roots: &[String],
    ctx: &crate::agent::exec_ctx::ToolCtx,
    project_id: &str,
) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录，无法部署".into());
    }
    let root = Path::new(project_path);
    let info = crate::services::harmony::parse_project(root);

    // 定位 hap（与 deploy 一致）
    let hap = if let Some(h) = args["hap"].as_str() {
        let p = PathBuf::from(h);
        if p.is_absolute() { p } else { root.join(p) }
            .to_string_lossy().to_string()
    } else {
        crate::services::harmony::find_latest_hap(root, info.hap_output_dir.as_deref())
            .ok_or_else(|| "未找到 .hap 构建产物，请先执行 build_project".to_string())?
            .to_string_lossy().to_string()
    };
    if !Path::new(&hap).exists() {
        return Err(format!("hap 文件不存在: {hap}"));
    }
    let is_signed = Path::new(&hap).file_name().and_then(|s| s.to_str()).is_some_and(|s| s.contains("-signed"));
    let bundle = info.bundle_name.clone().unwrap_or_default();
    let ability = info.main_element.clone().unwrap_or_else(|| "EntryAbility".to_string());

    // 解析目标设备列表
    let devices: Vec<String> = if let Some(arr) = args["devices"].as_array() {
        arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
    } else if let Ok(devs) = crate::commands::devices::list_devices().await {
        devs.iter().filter(|d| is_device_online(&d.state)).map(|d| d.id.clone()).collect()
    } else {
        Vec::new()
    };
    if devices.is_empty() {
        return Err("没有可用的在线设备。请连接设备并开启 USB 调试，或用 list_devices 查看。".into());
    }

    let hap = hap.clone();
    let ctx = ctx.clone();
    let bundle_c = bundle.clone();
    let ability_c = ability.clone();
    ctx.emit_log("system", &format!("并行部署到 {} 台设备: {}", devices.len(), devices.join(", ")));

    // 每设备一个并行任务
    let futures = devices.iter().cloned().map(|dev| {
        let hap = hap.clone();
        let ctx = ctx.clone();
        let bundle = bundle_c.clone();
        let ability = ability_c.clone();
        let project_path = project_path.to_string();
        let project_id = project_id.to_string();
        tokio::spawn(async move {
            let res = deploy_one_device(
                &ctx, &project_path, &project_id, &dev, &hap, is_signed, &bundle, &ability,
            ).await;
            (dev, res)
        })
    });
    let results = futures_util::future::join_all(futures).await;

    // 按设备门控：同一设备的 deploy_all 不与单设备 deploy 并发（靠 per-device gate 名）
    let mut ok_count = 0usize;
    let mut fail_count = 0usize;
    let mut summary = format!("多设备部署结果（共 {} 台）：\n", devices.len());
    for item in &results {
        match item {
            Ok((dev, Ok(msg))) => {
                ok_count += 1;
                summary.push_str(&format!("\n✓ {dev}\n"));
                for line in msg.lines().filter(|l| l.starts_with("✓") || l.contains("启动") || l.starts_with("设备型号")) {
                    summary.push_str(&format!("  {line}\n"));
                }
            }
            Ok((dev, Err(e))) => {
                fail_count += 1;
                summary.push_str(&format!("\n✗ {dev}: {}\n", tail(e, 300)));
            }
            Err(join_err) => {
                fail_count += 1;
                summary.push_str(&format!("\n✗ 任务异常: {join_err}\n"));
            }
        }
    }
    summary.push_str(&format!("\n成功 {ok_count} 台，失败 {fail_count} 台。"));
    ctx.emit_log("system", &format!("多设备部署完成：成功 {ok_count}，失败 {fail_count}"));
    if fail_count > 0 && ok_count == 0 {
        Err(summary)
    } else {
        Ok(summary)
    }
}

pub(super) async fn deploy_one_device(
    ctx: &crate::agent::exec_ctx::ToolCtx,
    project_path: &str,
    project_id: &str,
    device_id: &str,
    hap: &str,
    is_signed: bool,
    bundle: &str,
    ability: &str,
) -> Result<String, String> {
    // per-device 并发护栏：不同设备可并行，同设备串行（与单设备 deploy 互不踩踏）
    let gate_name = format!("deploy:{device_id}");
    let _gate = crate::services::tool_limits::acquire_named_gate(&gate_name).await;

    let mut out = format!("设备 {device_id}\n");

    // 冲突检测
    let mut already_installed = false;
    if !bundle.is_empty() {
        if let Ok(dump) = run_hdc_shell(device_id, &["bm", "dump", "-n", bundle], 30).await {
            if dump.contains(bundle) && !dump.contains("not found") {
                already_installed = true;
            }
        }
    }
    let install_args: Vec<String> = if already_installed {
        vec!["-t", device_id, "install", "-r", hap].into_iter().map(String::from).collect()
    } else {
        vec!["-t", device_id, "install", hap].into_iter().map(String::from).collect()
    };
    let install_out = crate::agent::exec_ctx::run_cmd_streaming(
        ctx, "hdc", &install_args, None, 300, None,
    ).await.map_err(|e| with_advice("deploy", e))?;
    let install_text = smart_decode(&install_out.stdout) + &smart_decode(&install_out.stderr);
    if !install_out.status.success() {
        let (cat, msg) = classify_deploy_error(&install_text, is_signed);
        return Err(format!("[{cat}] {}", msg.lines().next().unwrap_or("安装失败")));
    }
    out.push_str(" 安装成功\n");

    if bundle.is_empty() {
        out.push_str("（未解析到 bundleName，跳过拉起）\n");
        return Ok(out);
    }

    // 拉起
    run_hdc_shell(device_id, &["aa", "start", "-b", bundle, "-a", ability], 30)
        .await.map_err(|e| format!("拉起失败: {e}"))?;

    // 存活探测
    let mut alive = false;
    for wait in [2u64, 3, 3] {
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
        match run_hdc_shell(device_id, &["aa", "dump", "-l"], 30).await {
            Ok(dump) if dump.contains(bundle) => alive = true,
            _ => { alive = false; break; }
        }
    }

    if alive {
        out.push_str(" 启动并稳定运行 ✓\n");
        // 仅在这是"默认/第一台成功设备"时挂运行日志监听，避免多设备互相 abort
        if let Ok(default_dev) = default_device_id().await {
            if default_dev == device_id {
                crate::agent::runtime_log::start(project_path, ctx, device_id, bundle);
            }
        }
        let _ = project_id;
        Ok(out)
    } else {
        // 崩溃归因
        let faultlog = fetch_recent_faultlog(device_id, bundle).await.unwrap_or_default();
        let hilog = run_hdc_shell(device_id, &["hilog", "-x"], 25).await.unwrap_or_default();
        let report = crate::agent::crash::analyze(bundle, &faultlog, &hilog);
        crate::agent::diagnostics::record(
            project_path,
            crate::agent::diagnostics::Diagnosis {
                source: "crash_analysis".into(),
                category: report.category.clone(),
                summary: report.summary.clone(),
                detail: tail(&report.snippet, 600),
                at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0),
            },
        );
        Err(format!("启动后崩溃 [{}]: {}", report.category, tail(&report.summary, 200)))
    }
}

async fn fetch_recent_faultlog(device: &str, bundle: &str) -> Result<String, String> {
    // 列目录，过滤出本应用且类型为崩溃/JS异常的文件
    let ls = run_hdc_shell(device, &["ls", "-t", "/data/log/faultlog/temp/"], 15).await?;
    let candidates: Vec<&str> = ls
        .lines()
        .map(str::trim)
        .filter(|l| l.contains(bundle) && (l.starts_with("JsError") || l.starts_with("CppCrash") || l.contains("crash")))
        .collect();
    let Some(name) = candidates.first() else {
        return Ok(String::new());
    };
    let path = format!("/data/log/faultlog/temp/{name}");
    run_hdc_shell(device, &["cat", &path], 20).await
}

pub(super) fn classify_deploy_error(output: &str, is_signed: bool) -> (String, String) {
    let lower = output.to_lowercase();
    let (category, advice) = if lower.contains("not find")
        || lower.contains("device not found")
        || lower.contains("connected") && lower.contains("no devices")
        || lower.contains("offline")
        || lower.contains("can not connect")
    {
        ("device_offline", "设备未连接或离线。请先调用 list_devices 确认设备在线；提示用户用 USB 连接设备、开启开发者模式与 USB 调试，或重新插拔。不要改代码。")
    } else if is_signed == false
        || lower.contains("signature")
        || lower.contains("sign verify")
        || lower.contains("not authorized")
        || lower.contains("9568339")
        || lower.contains("code:95683")
    {
        ("signing", "签名校验失败或产物未签名。不要改代码：调用 show_diagnose_card(category=signing) 引导用户在 DevEco Studio 配置自动签名；或先用 release 模式重新构建已签名产物再部署。")
    } else if lower.contains("downgrade")
        || lower.contains("version downgrade")
        || lower.contains("install_failed_version_downgrade")
        || lower.contains("higher version")
    {
        ("version_downgrade", "设备上已安装更高版本，无法降级。让用户在设备上卸载旧版，或确认要安装的版本号；可提示用户执行 hdc uninstall <bundleName> 后重试。")
    } else if lower.contains("storage")
        || lower.contains("no space")
        || lower.contains("insufficient")
    {
        ("insufficient_storage", "设备存储空间不足。提示用户清理设备空间后重试，不要改代码。")
    } else if lower.contains("incompatible")
        || lower.contains("abi")
        || lower.contains("architecture")
        || lower.contains("not support")
    {
        ("incompatible", "设备架构/API 不兼容。检查工程 modules 配置与设备 API 级别，可调用 check_sdk_alignment 核对 SDK 版本。")
    } else {
        ("install_failed", "安装失败，原因未明确归类。请阅读下面的原始输出定位；若是签名问题参考 signing 建议，若是设备问题检查连接。不要盲目重复部署。")
    };
    let out = structured_tool_error(
        "deploy_hap",
        category,
        &format!("HAP 安装到设备失败（{category}）"),
        &[],
        &[advice],
        None,
        &tail(output, 800),
        &[],
    );
    (category.to_string(), out)
}

pub(super) async fn ohpm_search(args: &Value) -> Result<String, String> {
    let keyword = args["keyword"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());
    let Some(keyword) = keyword else {
        return Err("ohpm_search 需要 keyword（包名或关键字）".into());
    };
    let detail = args["detail"].as_bool().unwrap_or(false);
    let search_out = run_cmd("ohpm", &["search".into(), keyword.to_string()], None, 60).await
        .map_err(|e| with_advice("ohpm_search", e))?;
    if search_out.trim().is_empty() {
        return Ok(format!(
            "ohpm 仓库未找到与「{keyword}」匹配的包。\n建议：检查包名拼写；用 web_search 查该库的鸿蒙支持情况；或考虑替代库。"
        ));
    }
    let mut s = format!("ohpm 搜索结果（{keyword}）：\n{}\n", search_out.trim_end());
    if detail {
        let info = run_cmd("ohpm", &["info".into(), keyword.to_string()], None, 60).await
            .unwrap_or_else(|e| format!("ohpm info 失败：{e}"));
        s.push_str(&format!("\n--- ohpm info {keyword} ---\n{}\n", info.trim_end()));
    }
    s.push_str(&format!(
        "\n确认可用后：ohpm_install package={keyword}（或先 edit_file 更新 oh-package.json5 依赖再 ohpm_install）。"
    ));
    Ok(s)
}

pub(super) async fn ohpm_install(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录，无法安装依赖".into());
    }
    // 全局并发护栏：与构建/部署互斥，避免并发写 .ohpm
    let _gate = crate::services::tool_limits::acquire_gate("ohpm_install").await;
    let mut cmd_args = vec!["install".to_string()];
    if let Some(pkg) = args["package"].as_str() {
        cmd_args.push(pkg.to_string());
    }
    run_in_project(project_path, "ohpm", &cmd_args, 300)
        .await
        .map_err(|e| with_advice("ohpm_install", e))
}

