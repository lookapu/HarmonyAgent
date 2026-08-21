//! 构建/部署/依赖域工具：build_project / deploy / deploy_all / ohpm 系列。
//! 共享辅助函数（run_cmd / run_hdc_shell / default_device_id / tail 等）仍定义在父模块 mod.rs，
//! 本模块通过 `use super::*` 继承访问。

use super::*;
use futures_util::StreamExt;

/// 构建请求（宽松）：LLM 直接给出的参数，字段均可选；默认值与校验集中在 `resolve()` 显式落地。
#[derive(serde::Deserialize, Default)]
pub(super) struct BuildRequest {
    /// 构建模式：debug | release（缺省 debug）
    pub mode: Option<String>,
    /// 指定模块名（缺省用工程 entry 模块）
    pub module: Option<String>,
    /// 指定产品名（缺省 default 或工程首个产品）
    pub product: Option<String>,
    /// 本次待验证的变更文件；提供后自动计算最小模块/产品集合
    pub changed_files: Option<Vec<String>>,
    /// 构建前先 hvigor clean 清理缓存（缺省 false）
    pub clean: Option<bool>,
    /// 依赖阶段：auto（缺失时安装）/ force（始终安装）/ skip（显式跳过）
    pub dependencies: Option<String>,
}

impl BuildRequest {
    /// 从工具入参解析宽松请求：容忍未知字段与缺省字段，不在此处做业务校验。
    pub(super) fn from_args(args: &Value) -> Result<Self, String> {
        serde_json::from_value(args.clone()).map_err(|e| format!("build_project 参数解析失败：{e}"))
    }

    /// 显式 resolve：默认值落地 + 基础枚举校验；模块/产品归属由统一语义模型规划器校验。
    pub(super) fn resolve(
        self,
        _root: &Path,
        entry_module: Option<&str>,
    ) -> Result<BuildSpec, String> {
        let mode = self.mode.unwrap_or_else(|| "debug".to_string());
        if mode != "debug" && mode != "release" {
            return Err("mode 仅支持 debug 或 release".into());
        }
        let module_explicit = self
            .module
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());
        let module = match self
            .module
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(m) => Some(m.to_string()),
            None => entry_module.map(|s| s.to_string()),
        };
        let dependencies = self.dependencies.unwrap_or_else(|| "auto".into());
        if !matches!(dependencies.as_str(), "auto" | "force" | "skip") {
            return Err("dependencies 仅支持 auto、force 或 skip".into());
        }
        Ok(BuildSpec {
            mode,
            module,
            module_explicit,
            product: self
                .product
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            changed_files: self
                .changed_files
                .unwrap_or_default()
                .into_iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect(),
            clean: self.clean.unwrap_or(false),
            dependencies,
        })
    }
}

/// 构建规范（严格）：由 `BuildRequest::resolve()` 产出，默认值与校验已完成，run 内只消费本结构。
pub(super) struct BuildSpec {
    /// 已校验的构建模式（debug/release）
    pub mode: String,
    /// 规范化后的模块名；存在性与产品归属在构建计划阶段校验
    pub module: Option<String>,
    /// module 是否由调用方显式指定（与缺省 entry 区分）
    pub module_explicit: bool,
    /// 调用方显式指定的产品
    pub product: Option<String>,
    /// 用于影响分析的变更文件
    pub changed_files: Vec<String>,
    /// 是否先执行 clean
    pub clean: bool,
    /// 依赖安装策略（auto/force/skip）
    pub dependencies: String,
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
    let semantic_model = crate::services::harmony_model::cached(root);
    let info = crate::services::harmony::project_summary(root, &semantic_model);
    // Request/Spec 分离：宽松参数 BuildRequest → 显式 resolve() 产出严格规范 BuildSpec
    // （默认值/校验集中于此，run 内不再出现隐式 ?? 默认）
    let spec = BuildRequest::from_args(args)?.resolve(root, info.entry_module.as_deref())?;
    let mode = spec.mode.as_str();
    let requested_module = if spec.module_explicit {
        spec.module.as_deref()
    } else {
        None
    };
    let plan = crate::services::harmony_build::plan_build(
        root,
        &semantic_model,
        requested_module,
        spec.product.as_deref(),
        mode,
        &spec.changed_files,
    )?;
    // clean=true 时先执行 hvigor clean 清理缓存，用于缓存导致的诡异构建失败
    let do_clean = spec.clean;
    // 全局并发护栏：同一时间只允许一个构建（其他调用排队等待）
    let _gate = crate::services::tool_limits::acquire_workspace_gate(root).await;
    let target_key = plan
        .targets
        .iter()
        .map(|target| format!("{}@{}:{}", target.module, target.product, target.mode))
        .collect::<Vec<_>>()
        .join(",");
    let workflow_key = format!(
        "{}:{}:{}:{}:{}",
        plan.scope, target_key, mode, spec.clean, spec.dependencies
    );
    let fingerprint = crate::services::harmony_build::project_fingerprint(root);
    ctx.record_run_event(
        "harmony.build.planned",
        serde_json::json!({
            "project_path": project_path,
            "project_fingerprint": fingerprint,
            "scope": plan.scope,
            "mode": mode,
            "targets": plan.targets.iter().map(|target| serde_json::json!({
                "module": target.module,
                "product": target.product,
                "mode": target.mode,
                "task": target.task,
            })).collect::<Vec<_>>(),
            "workflow_key": workflow_key,
        }),
    );
    let (mut checkpoint, resumed) =
        crate::services::harmony_build::begin(root, &workflow_key, &fingerprint);
    if resumed {
        ctx.emit_log(
            "system",
            &format!(
                "恢复构建工作流：已完成阶段 [{}]，从 {} 继续",
                checkpoint.completed_stages.join(", "),
                checkpoint.current_stage
            ),
        );
    }
    // 构建耗时统计（含 clean，供成功提示展示）
    let build_started = std::time::Instant::now();

    // 流式构建：日志逐行推送 agent:log 并落盘
    let log_path = crate::agent::exec_ctx::new_build_log_path(project_path);
    // Windows 下优先 node 直调 hvigor-wrapper.js 绕过 cmd/.bat 弹窗；找不到时回退工程内 hvigorw.bat，
    // 再兜底 DevEco Studio 内置 hvigor 工具链（工程缺构建脚本时仍可构建）；
    // env 自动注入 DEVECO_SDK_HOME（未设置且探测到 DevEco 内置 SDK 时），否则 hvigor 报 00303217/00303312
    let hvigor = match crate::services::harmony::hvigor_command(root) {
        Ok(command) => command,
        Err(error) => {
            crate::services::harmony_build::stage_failed(
                root,
                &mut checkpoint,
                "environment",
                &error,
            );
            return Err(with_advice("build_project", error.to_string()));
        }
    };
    crate::services::harmony_build::stage_completed(root, &mut checkpoint, "environment");
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
    // 依赖阶段：默认只在声明的外部包缺失时执行 ohpm install；force 可强制同步，
    // skip 用于离线或调用方明确管理依赖的场景。安装后必须再次核对文件系统证据。
    let dependency_state = crate::services::harmony_build::dependency_state(root, &semantic_model);
    let should_install = spec.dependencies == "force"
        || (spec.dependencies == "auto" && !dependency_state.missing.is_empty());
    if should_install {
        ctx.emit_log(
            "system",
            &format!(
                "同步 OHPM 依赖：声明 {} 项，缺失 {} 项",
                dependency_state.declared,
                dependency_state.missing.len()
            ),
        );
        if let Err(error) = run_in_project(project_path, "ohpm", &["install".into()], 300).await {
            crate::services::harmony_build::stage_failed(
                root,
                &mut checkpoint,
                "dependencies",
                &error,
            );
            return Err(with_advice("ohpm_install", error));
        }
        let refreshed_model = crate::services::harmony_model::invalidate_files(
            root,
            &["oh-package-lock.json5".into()],
        )
        .model;
        let verified = crate::services::harmony_build::dependency_state(root, &refreshed_model);
        if !verified.missing.is_empty() {
            let error = format!(
                "OHPM 安装后仍缺少 {} 项依赖：{}",
                verified.missing.len(),
                verified.missing.join(", ")
            );
            crate::services::harmony_build::stage_failed(
                root,
                &mut checkpoint,
                "dependencies",
                &error,
            );
            return Err(with_advice("ohpm_install", error));
        }
        checkpoint.project_fingerprint = crate::services::harmony_build::project_fingerprint(root);
    } else if spec.dependencies == "skip" && !dependency_state.missing.is_empty() {
        ctx.emit_log(
            "system",
            &format!(
                "已按请求跳过 OHPM 安装；仍缺少 {} 项依赖，Hvigor 可能失败",
                dependency_state.missing.len()
            ),
        );
    }
    crate::services::harmony_build::stage_completed(root, &mut checkpoint, "dependencies");
    // 可选：先 clean 清理构建缓存
    if do_clean {
        let mut clean_full = prefix.clone();
        clean_full.extend(crate::services::harmony::clean_args());
        ctx.emit_log(
            "system",
            &format!("清理构建缓存：{program} {}", clean_full.join(" ")),
        );
        match crate::agent::exec_ctx::run_cmd_streaming_env(
            ctx,
            &program,
            &clean_full,
            Some(root),
            120,
            None,
            envs,
        )
        .await
        {
            Ok(o) if o.status.success() => {
                ctx.emit_log("system", "缓存清理完成，开始构建");
            }
            Ok(o) => {
                let err = smart_decode(&o.stderr);
                ctx.emit_log(
                    "system",
                    &format!("清理缓存失败（继续构建）：{}", tail(&err, 500)),
                );
            }
            Err(e) => {
                ctx.emit_log("system", &format!("清理缓存异常（继续构建）：{e}"));
            }
        }
    }
    ctx.emit_log(
        "system",
        &format!(
            "构建计划：scope={}，{} 个目标：{}",
            plan.scope,
            plan.targets.len(),
            plan.targets
                .iter()
                .map(|target| format!(
                    "{}@{}/{}({})",
                    target.module, target.product, target.mode, target.task
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
    let mut combined_parts = Vec::new();
    let mut build_succeeded = true;
    let mut failed_exit_code = None;
    for (index, target) in plan.targets.iter().enumerate() {
        let mut full_args = prefix.clone();
        full_args.extend(crate::services::harmony::assemble_target_args(
            &target.task,
            Some(&target.module),
            &target.product,
            &target.mode,
        ));
        ctx.emit_log(
            "system",
            &format!(
                "开始构建目标 {}/{}（{}@{} / {}）：{program} {}",
                index + 1,
                plan.targets.len(),
                target.module,
                target.product,
                target.mode,
                full_args.join(" ")
            ),
        );
        let output = match crate::agent::exec_ctx::run_cmd_streaming_env(
            ctx,
            &program,
            &full_args,
            Some(root),
            600,
            Some(&log_path),
            envs,
        )
        .await
        {
            Ok(output) => output,
            Err(error) => {
                ctx.emit_log("system", &format!("构建异常：{error}"));
                crate::services::harmony_build::stage_failed(
                    root,
                    &mut checkpoint,
                    "build",
                    &error,
                );
                return Err(with_advice("build_project", error));
            }
        };
        let stdout = smart_decode(&output.stdout);
        let stderr = smart_decode(&output.stderr);
        combined_parts.push(format!(
            "===== {}@{} / {} =====\n{}\n{}",
            target.module, target.product, target.mode, stdout, stderr
        ));
        if !output.status.success() {
            build_succeeded = false;
            failed_exit_code = output.status.code();
            break;
        }
    }
    let combined = combined_parts.join("\n");

    if build_succeeded {
        crate::services::harmony_build::stage_completed(root, &mut checkpoint, "build");
        let manifest = match crate::services::harmony_build::record_artifact_manifest(
            root,
            &semantic_model,
            &plan,
            &workflow_key,
            &checkpoint.project_fingerprint,
        ) {
            Ok(manifest) => manifest,
            Err(error) => {
                crate::services::harmony_build::stage_failed(
                    root,
                    &mut checkpoint,
                    "artifacts",
                    &error,
                );
                return Err(with_advice("build_project", error));
            }
        };
        let artifacts = manifest.artifacts;
        if artifacts.is_empty() {
            let error = "Hvigor 返回成功，但未发现 HAP/HSP/HAR 产物";
            crate::services::harmony_build::stage_failed(root, &mut checkpoint, "artifacts", error);
            return Err(with_advice("build_project", error.to_string()));
        }
        crate::services::harmony_build::completed(root, &mut checkpoint, artifacts.clone());
        let elapsed = build_started.elapsed().as_secs_f32();
        let mut summary = format!(
            "构建成功（{mode}，{} 个目标，耗时 {elapsed:.1}s）。\n",
            plan.targets.len()
        );
        summary.push_str(&format!(
            "影响计划：scope={}；{}\n",
            plan.scope,
            plan.targets
                .iter()
                .map(|target| format!("{}@{}/{}", target.module, target.product, target.mode))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        summary.push_str(&format!(
            "工作流完成：environment → dependencies → build → artifacts（发现 {} 个产物）\n",
            artifacts.len()
        ));
        for artifact in artifacts.iter().take(8) {
            summary.push_str(&format!(
                "- {} · {} bytes · {} · product={} · signing={} · sha256={}\n",
                artifact.path,
                artifact.size,
                artifact.kind,
                artifact.product.as_deref().unwrap_or("unknown"),
                artifact.signing_status,
                &artifact.sha256[..12]
            ));
        }
        summary.push_str("产物清单: .deveco-agent/harmony-artifacts.json\n");
        // 未签名产物预警：构建日志出现 No signingConfig 说明产出 unsigned HAP，
        // 真机部署必然报 9568319——提前告知并给出自动修复路径，避免部署失败后再回查
        if combined.contains("No signingConfig") || combined.contains("no signingConfig found") {
            summary.push_str(
                "⚠ 本次构建产物未签名（构建日志：No signingConfig found）——部署真机将报 9568319 签名校验失败。\n请先调用 diagnose_signing 自检签名配置并按建议修复（或确认工程已配置签名），再重新构建。\n",
            );
        }
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
        ctx.record_run_event(
            "harmony.build.completed",
            serde_json::json!({
                "project_path": project_path,
                "mode": mode,
                "elapsed_ms": build_started.elapsed().as_millis(),
                "log_path": log_path,
                "artifacts": artifacts.iter().map(|artifact| serde_json::json!({
                    "path": artifact.path,
                    "kind": artifact.kind,
                    "module": artifact.module,
                    "product": artifact.product,
                    "signing_status": artifact.signing_status,
                    "sha256": artifact.sha256,
                })).collect::<Vec<_>>(),
            }),
        );
        // 工具结果只回传尾部，避免上下文爆炸；完整日志可通过 get_build_log 读取
        summary.push_str(&tail(&combined, 2000));
        Ok(summary)
    } else {
        crate::services::harmony_build::stage_failed(
            root,
            &mut checkpoint,
            "build",
            &tail(&combined, 2000),
        );
        let errors = crate::services::harmony::parse_build_errors(&combined);
        ctx.emit_log(
            "system",
            &format!("构建失败（退出码 {failed_exit_code:?}）"),
        );
        if errors.is_empty() {
            ctx.record_run_event(
                "harmony.build.failed",
                serde_json::json!({
                    "project_path": project_path,
                    "mode": mode,
                    "category": "build_failed",
                    "exit_code": failed_exit_code,
                    "log_path": log_path,
                    "evidence": tail(&combined, 1500),
                }),
            );
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
        let mut cat_count: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for e in &errors {
            *cat_count.entry(e.category.as_str()).or_insert(0) += 1;
        }
        let dominant = cat_count.iter().max_by_key(|(_, &v)| v).map(|(k, _)| *k);
        let mut locations: Vec<ErrorLocation> = errors
            .iter()
            .take(8)
            .map(|e| {
                let trace = match &e.error_code {
                    Some(code) => format!("[stage={} code={code}] ", e.stage),
                    None => format!("[stage={}] ", e.stage),
                };
                let msg = if e.suggestion.trim().is_empty() {
                    format!("{trace}{}", e.message)
                } else {
                    format!("{trace}{}（建议: {}）", e.message, e.suggestion)
                };
                ErrorLocation {
                    file: e.file.clone(),
                    line: e.line,
                    message: msg,
                }
            })
            .collect();
        let api_mappings = collect_arkts_api_mappings(
            ctx,
            root,
            spec.product.as_deref(),
            &errors,
        );
        for mapping in &api_mappings {
            let source = errors.get(mapping.error_index);
            for evidence in mapping.evidence.iter().take(6) {
                locations.push(ErrorLocation {
                    file: source.and_then(|error| error.file.clone()),
                    line: source.and_then(|error| error.line),
                    message: format!(
                        "[arkts_api={} confidence={:.2}] {evidence}",
                        mapping.kind, mapping.confidence
                    ),
                });
            }
        }
        let diagnoses = crate::services::harmony_diagnosis::diagnose_failure(
            root,
            &semantic_model,
            &combined,
            &errors,
        );
        for diagnosis in &diagnoses {
            for evidence in diagnosis.evidence.iter().take(3) {
                locations.push(ErrorLocation {
                    file: None,
                    line: None,
                    message: format!(
                        "[diagnosis={} confidence={:.2}] {evidence}",
                        diagnosis.kind, diagnosis.confidence
                    ),
                });
            }
        }
        let mut next: Vec<&str> = match dominant {
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
                "调用 diagnose_signing 自动核对签名配置/材料与设备匹配性，按建议修复 build-profile.json5（或 bundleName）",
                "修复后重新 build_project 验证",
                "仅当材料库完全无匹配（需登录华为账号生成证书）时才 show_diagnose_card(category=signing) 提示用户",
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
        for diagnosis in &diagnoses {
            next.extend(diagnosis.recovery_steps.iter().map(String::as_str));
        }
        for mapping in &api_mappings {
            next.extend(mapping.recovery_steps.iter().map(String::as_str));
        }
        let dom_cat = dominant.unwrap_or("build_failed");
        let diagnosis_summary = diagnoses
            .iter()
            .map(|diagnosis| diagnosis.kind.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        crate::agent::diagnostics::record(
            project_path,
            crate::agent::diagnostics::Diagnosis {
                source: "build_project".into(),
                category: dom_cat.into(),
                summary: format!(
                    "{mode} 构建失败，{} 个错误（主导类别: {dom_cat}）",
                    errors.len()
                ),
                detail: locations
                    .iter()
                    .take(5)
                    .map(|l| {
                        let pos = match (&l.file, l.line) {
                            (Some(f), Some(n)) => format!("{f}:{n}"),
                            (Some(f), None) => f.clone(),
                            _ => "未知位置".into(),
                        };
                        format!("{pos}: {}", l.message)
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            },
        );
        let user_kb = load_user_knowledge(ctx, project_id.as_deref());
        let (matched, hit_ids) =
            crate::services::harmony_knowledge::match_knowledge_with_user(&combined, 3, &user_kb);
        for id in &hit_ids {
            bump_knowledge_hit(ctx, id);
        }
        let err = structured_tool_error(
            "build_project",
            dom_cat,
            &format!(
                "{mode} 构建失败，检测到 {} 个错误（主导类别: {dom_cat}{}）",
                errors.len(),
                if diagnosis_summary.is_empty() {
                    String::new()
                } else {
                    format!("；专项诊断: {diagnosis_summary}")
                }
            ),
            &locations,
            &next,
            Some(&log_path.display().to_string()),
            "",
            &matched,
        );
        ctx.record_run_event(
            "harmony.build.failed",
            serde_json::json!({
                "project_path": project_path,
                "mode": mode,
                "category": dom_cat,
                "exit_code": failed_exit_code,
                "log_path": log_path,
                "errors": errors.iter().take(20).map(|error| serde_json::json!({
                    "category": error.category,
                    "stage": error.stage,
                    "code": error.error_code,
                    "file": error.file,
                    "line": error.line,
                    "message": error.message,
                })).collect::<Vec<_>>(),
                "arkts_api_mappings": api_mappings,
            }),
        );
        Err(err)
    }
}

fn collect_arkts_api_mappings(
    ctx: &crate::agent::exec_ctx::ToolCtx,
    root: &Path,
    product: Option<&str>,
    errors: &[crate::services::harmony::BuildError],
) -> Vec<crate::services::harmony_api_diagnosis::ArktsApiMapping> {
    let Some(app) = ctx.app.as_ref() else {
        let context = crate::services::sdk_api::project_api_context(Some(root), product, None);
        return crate::services::harmony_api_diagnosis::map_errors(
            errors,
            &context,
            None,
            None,
        );
    };
    let db: tauri::State<crate::db::DbState> = tauri::Manager::state(app);
    let env = crate::services::harmony_env::detect(&db);
    let context = crate::services::sdk_api::project_api_context(
        Some(root),
        product,
        env.default_api.as_deref(),
    );
    let index = crate::services::harmony_env::default_api_dir(&env)
        .map(|api_dir| crate::services::sdk_api::index_api_dir(&api_dir));
    let mappings = match db.0.lock() {
        Ok(conn) => crate::services::harmony_api_diagnosis::map_errors(
            errors,
            &context,
            index.as_ref(),
            Some(&conn),
        ),
        Err(_) => crate::services::harmony_api_diagnosis::map_errors(
            errors,
            &context,
            index.as_ref(),
            None,
        ),
    };
    mappings
}

fn resolve_hap_for_deploy(args: &Value, root: &Path) -> Result<(String, bool, String), String> {
    if let Some(requested) = args["hap"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let path = PathBuf::from(requested);
        let path = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        if !path.exists() {
            return Err(format!("hap 文件不存在: {}", path.display()));
        }
        let verification = crate::services::harmony_build::verify_artifact_file(&path, "hap")?;
        if verification.signing_status == "unsigned" {
            return Err(format!(
                "{} 是明确未签名产物——真机安装将报签名校验失败。请修复 signingConfigs 后重新 build_project",
                path.display()
            ));
        }
        let is_signed = matches!(
            verification.signing_status.as_str(),
            "verified_signed" | "claimed_signed"
        );
        return Ok((
            path.to_string_lossy().to_string(),
            is_signed,
            format!(
                "使用用户显式确认的 HAP：{}（signing={} sha256={}）",
                path.display(),
                verification.signing_status,
                &verification.sha256[..12]
            ),
        ));
    }
    let selected = crate::services::harmony_build::select_deploy_artifact(
        root,
        args["product"].as_str(),
        args["module"].as_str(),
    )?;
    Ok((
        selected.absolute_path.to_string_lossy().to_string(),
        true,
        format!(
            "从清单选择已复验签名 HAP：{}（module={} product={} sha256={}）",
            selected.artifact.path,
            selected.artifact.module.as_deref().unwrap_or("unknown"),
            selected.artifact.product.as_deref().unwrap_or("unknown"),
            &selected.artifact.sha256[..12]
        ),
    ))
}

fn ensure_deploy_device_ready(device: &crate::commands::devices::DeviceInfo) -> Result<(), String> {
    if device.connection != "online" || !device.authorized {
        return Err(format!(
            "设备 {} 当前不可部署（raw={} connection={} authorized={}）。请先调用 list_devices 恢复连接与调试授权。",
            device.id, device.state, device.connection, device.authorized
        ));
    }
    let missing: Vec<&str> = ["install", "ability", "hilog"]
        .into_iter()
        .filter(|capability| {
            !device
                .capabilities
                .iter()
                .any(|available| available == capability)
        })
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "设备 {} 缺少部署闭环能力：{}。请检查系统工具与调试权限。",
            device.id,
            missing.join(", ")
        ));
    }
    Ok(())
}

async fn resolve_deploy_device(requested: Option<&str>) -> Result<String, String> {
    let devices = crate::commands::devices::list_devices()
        .await
        .map_err(|error| format!("无法发现设备：{error}"))?;
    let selected = if let Some(requested) = requested.map(str::trim).filter(|id| !id.is_empty()) {
        devices
            .iter()
            .find(|device| device.id == requested)
            .ok_or_else(|| {
                format!("未发现指定设备 {requested}；请调用 list_devices 刷新设备状态。")
            })?
    } else {
        devices
            .iter()
            .find(|device| device.is_default && device.connection == "online" && device.authorized)
            .or_else(|| {
                devices
                    .iter()
                    .find(|device| device.connection == "online" && device.authorized)
            })
            .ok_or_else(|| "未检测到已授权在线设备，请连接设备并确认调试授权".to_string())?
    };
    ensure_deploy_device_ready(selected)?;
    Ok(selected.id.clone())
}

async fn recover_fresh_install(device_id: &str, bundle: &str) -> String {
    if bundle.is_empty() {
        return "恢复：无法确定 bundleName，未执行自动卸载。".into();
    }
    match run_hdc_shell(device_id, &["bm", "uninstall", "-n", bundle], 30).await {
        Ok(output) => {
            let still_installed = run_hdc_shell(device_id, &["bm", "dump", "-n", bundle], 20)
                .await
                .is_ok_and(|dump| dump.contains(bundle) && !dump.contains("not found"));
            if still_installed {
                format!(
                    "恢复失败：已请求卸载本次新装的 {bundle}，但状态确认仍显示已安装。输出：{}",
                    tail(&output, 200)
                )
            } else {
                format!("恢复完成：已卸载本次新装的 {bundle}，并确认设备不再报告该应用。")
            }
        }
        Err(error) => format!("恢复失败：无法卸载本次新装的 {bundle}：{error}"),
    }
}

fn should_recover_fresh_install(already_installed: bool) -> bool {
    !already_installed
}

fn multi_deploy_concurrency(
    args: &Value,
    device_count: usize,
) -> Result<(&'static str, usize), String> {
    let strategy = args["strategy"].as_str().unwrap_or("parallel");
    match strategy {
        "serial" => Ok(("serial", 1)),
        "parallel" => Ok((
            "parallel",
            args["max_parallel"]
                .as_u64()
                .unwrap_or(2)
                .clamp(1, 4)
                .min(device_count.max(1) as u64) as usize,
        )),
        _ => Err("strategy 仅支持 serial 或 parallel".into()),
    }
}

async fn start_failure_evidence(device_id: &str, bundle: &str) -> String {
    let hilog = run_hdc_shell(device_id, &["hilog", "-x"], 25)
        .await
        .unwrap_or_default();
    let relevant = hilog
        .lines()
        .filter(|line| bundle.is_empty() || line.contains(bundle))
        .collect::<Vec<_>>()
        .join("\n");
    tail(
        if relevant.is_empty() {
            &hilog
        } else {
            &relevant
        },
        1200,
    )
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

    let (hap, is_signed, selection_note) = resolve_hap_for_deploy(args, root)?;
    ctx.emit_log("system", &selection_note);

    // 全局并发护栏：同一时间只允许一个部署
    let _gate = crate::services::tool_limits::acquire_workspace_gate(Path::new(project_path)).await;

    // 1. 选择设备：优先参数指定，否则取默认设备记忆 / 第一个在线设备
    let device_id = resolve_deploy_device(args["device"].as_str()).await?;
    // per-device 门控：与 deploy_all 中同设备的任务互斥，不同设备不阻塞
    let _dev_gate =
        crate::services::tool_limits::acquire_named_gate(&format!("deploy:{device_id}")).await;
    ctx.emit_log("system", &format!("部署到设备: {device_id}"));
    let mut out = String::new();
    out.push_str(&format!("{selection_note}\n"));
    out.push_str(&format!("目标设备: {device_id}\n"));
    if let Some(b) = &info.bundle_name {
        out.push_str(&format!("应用包名: {b}\n"));
    }
    // 设备信息
    if let Ok(model) = run_hdc_shell(&device_id, &["param", "get", "const.product.model"], 15).await
    {
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
    ctx.record_run_event(
        "harmony.deploy.started",
        serde_json::json!({
            "project_path": project_path,
            "device_id": device_id,
            "bundle": info.bundle_name,
            "artifact_path": hap,
            "artifact_signed": is_signed,
            "selection": selection_note,
            "install_mode": if already_installed { "replace" } else { "fresh" },
        }),
    );

    // 3. 安装（流式推送安装输出）
    ctx.emit_log(
        "system",
        &format!(
            "安装 {}",
            Path::new(&hap)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&hap)
        ),
    );
    let install_args = if already_installed {
        vec![
            "-t".to_string(),
            device_id.clone(),
            "install".to_string(),
            "-r".to_string(),
            hap.clone(),
        ]
    } else {
        vec![
            "-t".to_string(),
            device_id.clone(),
            "install".to_string(),
            hap.clone(),
        ]
    };
    let install_out =
        crate::agent::exec_ctx::run_cmd_streaming(ctx, "hdc", &install_args, None, 300, None)
            .await
            .map_err(|e| with_advice("deploy", e))?;
    let install_text = smart_decode(&install_out.stdout) + &smart_decode(&install_out.stderr);
    out.push_str(&install_text);
    if !install_out.status.success() {
        let (cat, msg) = classify_deploy_error(&install_text, is_signed);
        let user_kb = load_user_knowledge(ctx, Some(project_id));
        let (matched, hit_ids) = crate::services::harmony_knowledge::match_knowledge_with_user(
            &install_text,
            2,
            &user_kb,
        );
        for id in &hit_ids {
            bump_knowledge_hit(ctx, id);
        }
        let mut msg = msg;
        msg.push_str(&crate::services::harmony_knowledge::format_matched(
            &matched,
        ));
        crate::agent::diagnostics::record(
            project_path,
            crate::agent::diagnostics::Diagnosis {
                source: "deploy_hap".into(),
                category: cat.clone(),
                summary: format!("HAP 安装失败（{cat}）"),
                detail: tail(&install_text, 600),
                at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            },
        );
        ctx.record_run_event(
            "harmony.deploy.failed",
            serde_json::json!({
                "project_path": project_path,
                "device_id": device_id,
                "bundle": info.bundle_name,
                "stage": "install",
                "category": cat,
                "evidence": tail(&install_text, 1200),
            }),
        );
        return Err(msg);
    }
    // 安装成功：清除该项目部署失败归因；若此前有失败，推送修复经验候选
    let removed = crate::agent::diagnostics::clear_source(project_path, "deploy_hap");
    if !removed.is_empty() {
        emit_knowledge_candidate(ctx, project_path, "deploy_hap", &removed, &install_text);
    }
    ctx.record_run_event(
        "harmony.deploy.installed",
        serde_json::json!({
            "project_path": project_path,
            "device_id": device_id,
            "bundle": info.bundle_name,
            "install_mode": if already_installed { "replace" } else { "fresh" },
            "evidence": tail(&install_text, 500),
        }),
    );

    // 4. 拉起应用（鸿蒙用 aa start，不是 am start）
    let bundle = match info.bundle_name.as_deref() {
        Some(b) => b,
        None => {
            out.push_str(
                "\n⚠ 未能从工程解析 bundleName，跳过自动拉起。请确认 AppScope/app.json5 配置。\n",
            );
            return Ok(out);
        }
    };
    let ability = info.main_element.as_deref().unwrap_or("EntryAbility");
    ctx.emit_log("system", &format!("拉起应用: {bundle}/{ability}"));
    let start = match run_hdc_shell(
        &device_id,
        &["aa", "start", "-b", bundle, "-a", ability],
        30,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => {
            let evidence = start_failure_evidence(&device_id, bundle).await;
            let recovery = if should_recover_fresh_install(already_installed) {
                recover_fresh_install(&device_id, bundle).await
            } else {
                "恢复：部署前应用已存在，保留覆盖安装后的应用，避免误删用户原有安装。".to_string()
            };
            ctx.record_run_event(
                "harmony.deploy.failed",
                serde_json::json!({
                    "project_path": project_path,
                    "device_id": device_id,
                    "bundle": bundle,
                    "stage": "ability_start",
                    "category": "ability_start_failed",
                    "evidence": evidence,
                    "recovery": recovery,
                }),
            );
            return Err(format!(
                "拉起失败: {error}\n日志证据:\n{}\n{recovery}",
                if evidence.is_empty() {
                    "（未捕获到相关 hilog）"
                } else {
                    &evidence
                }
            ));
        }
    };
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
        ctx.emit_log(
            "system",
            "已开启运行日志监听（error 级），运行期异常会自动回流",
        );
        // 清除该项目的历史崩溃归因；若此前有崩溃记录则推送修复经验候选
        let removed = crate::agent::diagnostics::clear_source(project_path, "crash_analysis");
        if !removed.is_empty() {
            let crash_log = removed
                .iter()
                .map(|d| format!("{}\n{}", d.summary, d.detail))
                .collect::<Vec<_>>()
                .join("\n");
            emit_knowledge_candidate(ctx, project_path, "crash_analysis", &removed, &crash_log);
        }
        ctx.record_run_event(
            "harmony.deploy.completed",
            serde_json::json!({
                "project_path": project_path,
                "device_id": device_id,
                "bundle": bundle,
                "ability": ability,
                "status": "stable",
                "runtime_log": "watching",
            }),
        );
    } else {
        out.push_str("\n❌ 应用启动后崩溃（未在 ability 栈中持续存活）。\n");
        ctx.emit_log("system", "应用启动后崩溃，正在抓取 faultlog 与 hilog…");

        // 优先拉 faultlog（结构化程度高），回退 hilog -x
        let faultlog = fetch_recent_faultlog(&device_id, bundle)
            .await
            .unwrap_or_default();
        let hilog = run_hdc_shell(&device_id, &["hilog", "-x"], 25)
            .await
            .unwrap_or_default();
        let report = crate::agent::crash::analyze(bundle, &faultlog, &hilog);
        // 历史崩溃模式：同类崩溃反复出现时提示参考既往修复经验
        let nth = crate::agent::crash::record_pattern(project_path, &report);

        // 写入跨轮诊断，下一轮 model 能看到"上次运行时崩溃是什么"
        crate::agent::diagnostics::record(
            project_path,
            crate::agent::diagnostics::Diagnosis {
                source: "crash_analysis".into(),
                category: report.category.clone(),
                summary: if nth > 1 {
                    format!(
                        "{}（同类崩溃历史第 {nth} 次，建议参考既往修复经验避免重复踩坑）",
                        report.summary
                    )
                } else {
                    report.summary.clone()
                },
                detail: if report.locations.is_empty() {
                    tail(&report.snippet, 600)
                } else {
                    format!(
                        "定位: {}\n{}",
                        report.locations.join(", "),
                        tail(&report.snippet, 500)
                    )
                },
                at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
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
            next.insert(
                0,
                format!(
                    "根据以下定位读取并修复源码：{}",
                    report.locations.join("; ")
                ),
            );
        }
        let err_locs: Vec<ErrorLocation> = report
            .locations
            .iter()
            .map(|l| {
                let (f, line) = match l.rsplit_once(':') {
                    Some((f, n)) => (Some(f.to_string()), n.parse::<i64>().ok()),
                    None => (Some(l.clone()), None),
                };
                ErrorLocation {
                    file: f,
                    line,
                    message: report.message.clone(),
                }
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
        out.push('\n');
        if should_recover_fresh_install(already_installed) {
            out.push_str(&recover_fresh_install(&device_id, bundle).await);
            out.push('\n');
        } else {
            out.push_str("恢复：部署前应用已存在，保留覆盖安装后的应用，避免误删用户原有安装。\n");
        }
        ctx.record_run_event(
            "harmony.runtime.anomaly",
            serde_json::json!({
                "project_path": project_path,
                "device_id": device_id,
                "bundle": bundle,
                "source": if faultlog.is_empty() { "hilog" } else { "faultlog+hilog" },
                "category": report.category,
                "summary": report.summary,
                "locations": report.locations,
                "evidence": tail(&report.snippet, 1200),
            }),
        );
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

    let (hap, is_signed, selection_note) = resolve_hap_for_deploy(args, root)?;
    ctx.emit_log("system", &selection_note);
    let bundle = info.bundle_name.clone().unwrap_or_default();
    let ability = info
        .main_element
        .clone()
        .unwrap_or_else(|| "EntryAbility".to_string());

    // 解析并复验目标设备列表；显式设备也不能绕过连接、授权与能力门禁。
    let snapshots = crate::commands::devices::list_devices()
        .await
        .map_err(|error| format!("无法发现设备：{error}"))?;
    let mut devices: Vec<String> = if let Some(arr) = args["devices"].as_array() {
        let mut selected = Vec::new();
        for requested in arr.iter().filter_map(Value::as_str) {
            let requested = requested.trim();
            if requested.is_empty() {
                continue;
            }
            let snapshot = snapshots
                .iter()
                .find(|device| device.id == requested)
                .ok_or_else(|| {
                    format!("未发现指定设备 {requested}；请调用 list_devices 刷新设备状态。")
                })?;
            ensure_deploy_device_ready(snapshot)?;
            selected.push(snapshot.id.clone());
        }
        selected
    } else {
        snapshots
            .iter()
            .filter(|device| ensure_deploy_device_ready(device).is_ok())
            .map(|device| device.id.clone())
            .collect()
    };
    devices.sort();
    devices.dedup();
    if devices.is_empty() {
        return Err(
            "没有可用的在线设备。请连接设备并开启 USB 调试，或用 list_devices 查看。".into(),
        );
    }

    let (strategy, concurrency) = multi_deploy_concurrency(args, devices.len())?;

    let hap = hap.clone();
    let ctx = ctx.clone();
    let bundle_c = bundle.clone();
    let ability_c = ability.clone();
    ctx.emit_log(
        "system",
        &format!(
            "按 {strategy} 策略部署到 {} 台设备（并发上限 {concurrency}）: {}",
            devices.len(),
            devices.join(", ")
        ),
    );
    ctx.record_run_event(
        "harmony.deploy.batch.started",
        serde_json::json!({
            "project_path": project_path,
            "project_id": project_id,
            "strategy": strategy,
            "max_parallel": concurrency,
            "devices": devices,
            "artifact_path": hap,
        }),
    );

    // 有界并发：不预先 spawn 全部设备；serial 使用同一路径但并发数为 1。
    let futures = futures_util::stream::iter(devices.iter().cloned().map(|dev| {
        let hap = hap.clone();
        let ctx = ctx.clone();
        let bundle = bundle_c.clone();
        let ability = ability_c.clone();
        let project_path = project_path.to_string();
        let project_id = project_id.to_string();
        async move {
            let res = deploy_one_device(
                &ctx,
                &project_path,
                &project_id,
                &dev,
                &hap,
                is_signed,
                &bundle,
                &ability,
            )
            .await;
            (dev, res)
        }
    }))
    .buffer_unordered(concurrency);
    let mut results: Vec<(String, Result<String, String>)> = futures.collect().await;
    results.sort_by(|left, right| left.0.cmp(&right.0));

    // 按设备门控：同一设备的 deploy_all 不与单设备 deploy 并发（靠 per-device gate 名）
    let mut ok_count = 0usize;
    let mut fail_count = 0usize;
    let mut summary = format!(
        "多设备部署结果（共 {} 台）：\n{}\n",
        devices.len(),
        selection_note
    );
    for item in &results {
        match item {
            (dev, Ok(msg)) => {
                ok_count += 1;
                summary.push_str(&format!("\n✓ {dev}\n"));
                for line in msg.lines().filter(|l| {
                    l.starts_with("✓") || l.contains("启动") || l.starts_with("设备型号")
                }) {
                    summary.push_str(&format!("  {line}\n"));
                }
            }
            (dev, Err(e)) => {
                fail_count += 1;
                summary.push_str(&format!("\n✗ {dev}: {}\n", tail(e, 300)));
            }
        }
    }
    summary.push_str(&format!("\n成功 {ok_count} 台，失败 {fail_count} 台。"));
    ctx.record_run_event(
        "harmony.deploy.batch.completed",
        serde_json::json!({
            "project_path": project_path,
            "project_id": project_id,
            "strategy": strategy,
            "max_parallel": concurrency,
            "succeeded": ok_count,
            "failed": fail_count,
            "results": results.iter().map(|(device, result)| serde_json::json!({
                "device_id": device,
                "status": if result.is_ok() { "completed" } else { "failed" },
                "summary": match result { Ok(output) => tail(output, 500), Err(error) => tail(error, 500) },
            })).collect::<Vec<_>>(),
        }),
    );
    ctx.emit_log(
        "system",
        &format!("多设备部署完成：成功 {ok_count}，失败 {fail_count}"),
    );
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
    ctx.record_run_event(
        "harmony.deploy.started",
        serde_json::json!({
            "project_path": project_path,
            "project_id": project_id,
            "device_id": device_id,
            "bundle": bundle,
            "artifact_path": hap,
            "artifact_signed": is_signed,
            "install_mode": if already_installed { "replace" } else { "fresh" },
            "multi_device": true,
        }),
    );
    let install_args: Vec<String> = if already_installed {
        vec!["-t", device_id, "install", "-r", hap]
            .into_iter()
            .map(String::from)
            .collect()
    } else {
        vec!["-t", device_id, "install", hap]
            .into_iter()
            .map(String::from)
            .collect()
    };
    let install_out =
        crate::agent::exec_ctx::run_cmd_streaming(ctx, "hdc", &install_args, None, 300, None)
            .await
            .map_err(|e| with_advice("deploy", e))?;
    let install_text = smart_decode(&install_out.stdout) + &smart_decode(&install_out.stderr);
    if !install_out.status.success() {
        let (cat, msg) = classify_deploy_error(&install_text, is_signed);
        ctx.record_run_event(
            "harmony.deploy.failed",
            serde_json::json!({
                "project_path": project_path,
                "project_id": project_id,
                "device_id": device_id,
                "bundle": bundle,
                "stage": "install",
                "category": cat,
                "evidence": tail(&install_text, 1200),
                "multi_device": true,
            }),
        );
        return Err(format!(
            "[{cat}] {}",
            msg.lines().next().unwrap_or("安装失败")
        ));
    }
    out.push_str(" 安装成功\n");
    ctx.record_run_event(
        "harmony.deploy.installed",
        serde_json::json!({
            "project_path": project_path,
            "device_id": device_id,
            "bundle": bundle,
            "install_mode": if already_installed { "replace" } else { "fresh" },
            "multi_device": true,
        }),
    );

    if bundle.is_empty() {
        out.push_str("（未解析到 bundleName，跳过拉起）\n");
        return Ok(out);
    }

    // 拉起
    if let Err(error) =
        run_hdc_shell(device_id, &["aa", "start", "-b", bundle, "-a", ability], 30).await
    {
        let evidence = start_failure_evidence(device_id, bundle).await;
        let recovery = if should_recover_fresh_install(already_installed) {
            recover_fresh_install(device_id, bundle).await
        } else {
            "恢复：部署前应用已存在，保留覆盖安装后的应用，避免误删用户原有安装。".to_string()
        };
        ctx.record_run_event(
            "harmony.deploy.failed",
            serde_json::json!({
                "project_path": project_path,
                "device_id": device_id,
                "bundle": bundle,
                "stage": "ability_start",
                "category": "ability_start_failed",
                "evidence": evidence,
                "recovery": recovery,
                "multi_device": true,
            }),
        );
        return Err(format!(
            "拉起失败: {error}；日志证据: {}；{recovery}",
            if evidence.is_empty() {
                "（未捕获到相关 hilog）"
            } else {
                &evidence
            }
        ));
    }

    // 存活探测
    let mut alive = false;
    for wait in [2u64, 3, 3] {
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
        match run_hdc_shell(device_id, &["aa", "dump", "-l"], 30).await {
            Ok(dump) if dump.contains(bundle) => alive = true,
            _ => {
                alive = false;
                break;
            }
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
        ctx.record_run_event(
            "harmony.deploy.completed",
            serde_json::json!({
                "project_path": project_path,
                "project_id": project_id,
                "device_id": device_id,
                "bundle": bundle,
                "ability": ability,
                "status": "stable",
                "multi_device": true,
            }),
        );
        Ok(out)
    } else {
        // 崩溃归因
        let faultlog = fetch_recent_faultlog(device_id, bundle)
            .await
            .unwrap_or_default();
        let hilog = run_hdc_shell(device_id, &["hilog", "-x"], 25)
            .await
            .unwrap_or_default();
        let report = crate::agent::crash::analyze(bundle, &faultlog, &hilog);
        let nth = crate::agent::crash::record_pattern(project_path, &report);
        crate::agent::diagnostics::record(
            project_path,
            crate::agent::diagnostics::Diagnosis {
                source: "crash_analysis".into(),
                category: report.category.clone(),
                summary: if nth > 1 {
                    format!(
                        "{}（同类崩溃历史第 {nth} 次，建议参考既往修复经验避免重复踩坑）",
                        report.summary
                    )
                } else {
                    report.summary.clone()
                },
                detail: tail(&report.snippet, 600),
                at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            },
        );
        let recovery = if should_recover_fresh_install(already_installed) {
            recover_fresh_install(device_id, bundle).await
        } else {
            "恢复：部署前应用已存在，保留覆盖安装后的应用，避免误删用户原有安装。".to_string()
        };
        ctx.record_run_event(
            "harmony.runtime.anomaly",
            serde_json::json!({
                "project_path": project_path,
                "project_id": project_id,
                "device_id": device_id,
                "bundle": bundle,
                "source": if faultlog.is_empty() { "hilog" } else { "faultlog+hilog" },
                "category": report.category,
                "summary": report.summary,
                "evidence": tail(&report.snippet, 1200),
                "recovery": recovery,
                "multi_device": true,
            }),
        );
        Err(format!(
            "启动后崩溃 [{}]: {}；{recovery}",
            report.category,
            tail(&report.summary, 200)
        ))
    }
}

async fn fetch_recent_faultlog(device: &str, bundle: &str) -> Result<String, String> {
    // 列目录，过滤出本应用且类型为崩溃/JS异常的文件
    let ls = run_hdc_shell(device, &["ls", "-t", "/data/log/faultlog/temp/"], 15).await?;
    let candidates: Vec<&str> = ls
        .lines()
        .map(str::trim)
        .filter(|l| {
            l.contains(bundle)
                && (l.starts_with("JsError") || l.starts_with("CppCrash") || l.contains("crash"))
        })
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
    } else if lower.contains("unauthorized")
        || lower.contains("not authorized")
        || lower.contains("authorization denied")
        || lower.contains("debug authorization")
    {
        ("device_authorization_denied", "设备拒绝或尚未确认调试授权。请解锁设备并确认 USB/无线调试授权，再调用 list_devices 验证 authorized=true；不要通过改签名或重复安装绕过授权门禁。")
    } else if lower.contains("signature mismatch")
        || lower.contains("inconsistent signature")
        || lower.contains("conflicting package")
        || lower.contains("install_failed_update_incompatible")
    {
        ("install_conflict", "设备上的同包名应用与当前 HAP 签名或更新身份冲突。先用 get_app_info 核对现有版本/签名；只有确认可丢弃旧应用和数据后才卸载重装，默认不得自动卸载。")
    } else if is_signed == false
        || lower.contains("signature")
        || lower.contains("sign verify")
        || lower.contains("9568339")
        || lower.contains("code:95683")
    {
        ("signing", "签名校验失败或产物未签名。先调用 diagnose_signing 自动核对签名材料与设备/bundle 匹配性并按建议修复；若构建日志提示 No signingConfig（unsigned 产物）直接 build_project 重新构建后再部署；材料库完全无匹配时才需要 DevEco 重新签名。")
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
        (
            "insufficient_storage",
            "设备存储空间不足。提示用户清理设备空间后重试，不要改代码。",
        )
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
    let keyword = args["keyword"]
        .as_str()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let Some(keyword) = keyword else {
        return Err("ohpm_search 需要 keyword（包名或关键字）".into());
    };
    let detail = args["detail"].as_bool().unwrap_or(false);
    // ohpm 6.x 已移除 search 子命令：优先 view 查询包信息（返回版本/描述/依赖），
    // 兼容旧版先试 search、报 unknown command 时自动回退 view
    let search_out = match run_cmd("ohpm", &["search".into(), keyword.to_string()], None, 60).await
    {
        Ok(o) => o,
        Err(e) if e.contains("unknown command") || e.contains("unknown option") => String::new(),
        Err(e) => return Err(with_advice("ohpm_search", e)),
    };
    let search_out = search_out.trim();
    let mut s = String::new();
    if search_out.is_empty() {
        s.push_str(&format!(
            "ohpm 无 search 命令（6.x 起移除），改用 view 查询包「{keyword}」信息：\n"
        ));
    } else {
        s.push_str(&format!("ohpm 搜索结果（{keyword}）：\n{}\n", search_out));
    }
    let view = run_cmd("ohpm", &["view".into(), keyword.to_string()], None, 60)
        .await
        .unwrap_or_else(|e| format!("ohpm view 失败：{e}"));
    if view.contains("error") || view.contains("not found") {
        s.push_str(&format!("ohpm 仓库未找到与「{keyword}」匹配的包（view 无结果）。\n建议：检查包名拼写；用 web_search 查该库的鸿蒙支持情况；或考虑替代库。\n"));
        return Ok(s);
    }
    s.push_str(&format!(
        "--- ohpm view {keyword} ---\n{}\n",
        view.trim_end()
    ));
    if detail {
        let info = run_cmd("ohpm", &["info".into(), keyword.to_string()], None, 60)
            .await
            .unwrap_or_else(|e| format!("ohpm info 失败：{e}"));
        s.push_str(&format!(
            "\n--- ohpm info {keyword} ---\n{}\n",
            info.trim_end()
        ));
    }
    s.push_str(&format!(
        "\n确认可用后：ohpm_install package={keyword}（或先 edit_file 更新 oh-package.json5 依赖再 ohpm_install）。"
    ));
    Ok(s)
}

/// 截断展示用文本（保留前 n 字符）
fn display_truncate(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= n {
        s.to_string()
    } else {
        let cut: String = chars[..n].iter().collect();
        format!("{cut}…")
    }
}

/// ohpm_recommend：基于本地 landscape 缓存的离线三方库推荐/检索。
/// 数据来自 ohpm 官方 landscape（开源技术图谱）接口的定期镜像：
/// 含四级分类 / 描述 / 关键词 / 60 天下载量 / 评分，按热度排序。
pub(super) async fn ohpm_recommend(
    args: &Value,
    db: &crate::db::DbState,
) -> Result<String, String> {
    use crate::services::ohpm_landscape as ls;

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let st = ls::status(&conn)?;
    if st.total == 0 {
        return Ok(
            "本地三方库推荐缓存为空（还没拉取过）。\n处理建议：请用户在健康检查页「三方库推荐」点一次刷新；应用启动后也会自动拉取。\n在此之前可用 ohpm_search 在线查询包是否存在。".to_string(),
        );
    }

    let keyword = args["keyword"]
        .as_str()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let category = args["category"]
        .as_str()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let top = args["top"]
        .as_u64()
        .map(|n| n as usize)
        .unwrap_or(8)
        .min(15);
    // 排序：likes（最受欢迎）/ popularity（最流行）/ latest（最新发布），默认下载量
    let order = args["order"]
        .as_str()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("");

    let (pkgs, scope) = if let Some(kw) = keyword {
        (ls::search(&conn, kw, order, top, 0)?, format!("「{kw}」"))
    } else if let Some(cat) = category {
        (
            ls::by_category(&conn, cat, "", order, top, 0)?,
            format!("分类「{cat}」"),
        )
    } else {
        (ls::hot(&conn, order, top, 0)?, "热门".to_string())
    };

    if pkgs.is_empty() {
        return Ok(
            "本地三方库缓存中未找到匹配项。\n建议：换更短/更宽泛的关键词（含英文名）；先不带参数列出热门库看分类命名；或用 ohpm_search 在线查询。"
                .to_string(),
        );
    }

    let updated = st
        .updated_at
        .map(|t| {
            let s = chrono::DateTime::from_timestamp(t, 0)
                .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default();
            format!("更新于 {s}")
        })
        .unwrap_or_default();
    let mut out = format!(
        "本地三方库推荐（ohpm 官方 landscape 缓存，共 {} 个包，{}）：\n\n",
        st.total, updated
    );
    for (i, p) in pkgs.iter().enumerate() {
        out.push_str(&format!(
            "{}. **{}** v{} | ⬇ {} 次/60天 | {} | {}\n    {}\n",
            i + 1,
            p.package_name,
            p.version,
            p.down_count_60d,
            if p.license.is_empty() {
                "-".to_string()
            } else {
                p.license.clone()
            },
            p.level1(),
            display_truncate(&p.description, 90),
        ));
    }
    let order_name = match order {
        "likes" => "最受欢迎（点赞数）",
        "popularity" => "最流行（流行度）",
        "latest" => "最新发布（发布时间）",
        _ => "下载量",
    };
    out.push_str(&format!(
        "\n以上为{scope} Top{}（按{order_name}排序）。\n需要安装：ohpm_install package=<包名>；需要最新版本/依赖详情：ohpm_search keyword=<包名> detail=true；浏览其他分类：ohpm_recommend category=<一级分类名>。",
        pkgs.len()
    ));
    Ok(out)
}

pub(super) async fn ohpm_install(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录，无法安装依赖".into());
    }
    // 全局并发护栏：与构建/部署互斥，避免并发写 .ohpm
    let _gate = crate::services::tool_limits::acquire_workspace_gate(Path::new(project_path)).await;
    let mut cmd_args = vec!["install".to_string()];
    if let Some(pkg) = args["package"].as_str() {
        cmd_args.push(pkg.to_string());
    }
    run_in_project(project_path, "ohpm", &cmd_args, 300)
        .await
        .map_err(|e| with_advice("ohpm_install", e))
}

// ==================== 签名自检（diagnose_signing） ====================

/// 解析 p7b profile 元数据：DER 编码内嵌 JSON 的可读字符串，宽松提取关键字段。
fn parse_profile_meta(bytes: &[u8]) -> serde_json::Value {
    let text = String::from_utf8_lossy(bytes);
    let mut meta = serde_json::Map::new();
    for key in ["bundle-name", "type", "developer-id", "device-ids"] {
        let needle = format!("\"{key}\"");
        let Some(idx) = text.find(&needle) else {
            continue;
        };
        let after = &text[idx + needle.len()..];
        let Some(colon) = after.find(':') else {
            continue;
        };
        let v = after[colon + 1..].trim_start();
        if v.starts_with('"') {
            if let Some(end) = v[1..].find('"') {
                meta.insert(
                    key.to_string(),
                    serde_json::Value::String(v[1..1 + end].to_string()),
                );
            }
        } else if v.starts_with('[') {
            // device-ids 数组：收集全部引号字符串
            let mut ids: Vec<serde_json::Value> = Vec::new();
            let mut rest_v = v;
            while let Some(q) = rest_v.find('"') {
                let tail = &rest_v[q + 1..];
                match tail.find('"') {
                    Some(qe) => {
                        ids.push(serde_json::Value::String(tail[..qe].to_string()));
                        rest_v = &tail[qe + 1..];
                    }
                    None => break,
                }
            }
            meta.insert(key.to_string(), serde_json::Value::Array(ids));
        }
    }
    serde_json::Value::Object(meta)
}

/// 扫描用户签名材料目录（~/.ohos/config），返回 [(文件名, profile 元数据)]。
fn scan_sign_materials() -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from);
    let Some(home) = home else { return out };
    let dir = home.join(".ohos").join("config");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        let is_p7b = p
            .extension()
            .map(|s| s.to_string_lossy().eq_ignore_ascii_case("p7b"))
            .unwrap_or(false);
        if !is_p7b {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&p) {
            let meta = parse_profile_meta(&bytes);
            if meta
                .get("bundle-name")
                .map(|v| v.is_string())
                .unwrap_or(false)
            {
                out.push((
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    meta,
                ));
            }
        }
    }
    out
}

/// 检查 build-profile.json5 的签名配置引用的材料文件是否齐全。
fn signing_material_status(cfg: &serde_json::Value, root: &Path) -> (Vec<String>, Vec<String>) {
    let mut ok = Vec::new();
    let mut missing = Vec::new();
    for (key, field) in [
        ("certpath", "证书"),
        ("profile", "profile"),
        ("storeFile", "密钥库"),
    ] {
        let v = cfg.get(key).and_then(|v| v.as_str()).unwrap_or("");
        if v.is_empty() {
            missing.push(format!("{field}（{key}）未配置"));
            continue;
        }
        let p = Path::new(v);
        let p = if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        };
        if p.is_file() {
            ok.push(field.to_string());
        } else {
            missing.push(format!("{field}文件不存在：{}", p.display()));
        }
    }
    (ok, missing)
}

/// diagnose_signing：签名自检——核对工程签名配置、签名材料与设备 UDID 的匹配关系，
/// 输出结构化诊断与修复指引（优先给出可自动执行的修复路径：复用匹配材料/跨工程签名配置）。
pub(super) async fn diagnose_signing(args: &Value, roots: &[String]) -> Result<String, String> {
    let root = match args["path"].as_str() {
        Some(p) if !p.trim().is_empty() => resolve_in_roots(roots, p)?,
        _ => PathBuf::from(roots.first().map(String::as_str).unwrap_or("")),
    };
    if !root.is_dir() {
        return Err(format!("工程目录不存在：{}", root.display()));
    }
    let mut out = String::new();
    out.push_str(&format!("签名自检报告（{}）：\n\n", root.display()));

    // 1) 工程 bundleName
    let app_text = std::fs::read_to_string(root.join("AppScope").join("app.json5"))
        .or_else(|_| std::fs::read_to_string(root.join("app.json5")))
        .unwrap_or_default();
    let bundle = crate::services::harmony::parse_json5(&app_text)
        .ok()
        .and_then(|v| {
            let app = v.get("app").or_else(|| v.get("bundle"));
            app.and_then(|a| a.get("bundleName"))
                .and_then(|b| b.as_str())
                .map(String::from)
        });
    out.push_str(&format!(
        "1. 工程 bundleName：{}\n",
        bundle.as_deref().unwrap_or("（未能解析 app.json5）")
    ));

    // 2) 当前签名配置
    let bp_text = std::fs::read_to_string(root.join("build-profile.json5")).unwrap_or_default();
    let bp = crate::services::harmony::parse_json5(&bp_text).ok();
    let cfgs: Vec<serde_json::Value> = bp
        .as_ref()
        .and_then(|v| v.get("app"))
        .and_then(|v| v.get("signingConfigs"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    out.push_str(&format!("2. 签名配置：{} 项\n", cfgs.len()));
    let mut current_profile: Option<String> = None;
    if cfgs.is_empty() {
        out.push_str("   ⚠ 未配置 signingConfigs——构建产物为 unsigned HAP，部署真机将报 9568319\n");
    } else {
        for (i, cfg) in cfgs.iter().enumerate() {
            let name = cfg.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let (ok, missing) = signing_material_status(cfg, &root);
            out.push_str(&format!(
                "   [{i}] {name}：材料齐全（{}），缺失：{}\n",
                ok.join("+"),
                if missing.is_empty() {
                    "无".to_string()
                } else {
                    missing.join("; ")
                }
            ));
            if let Some(pp) = cfg.get("profile").and_then(|v| v.as_str()) {
                let p = Path::new(pp);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    root.join(p)
                }
                .is_file()
                .then(|| current_profile = Some(pp.to_string()));
            }
        }
    }

    // 3) 设备与 profile 匹配性
    let device = default_device_id().await.ok();
    let mut device_udid: Option<String> = None;
    if let Some(dev) = &device {
        if let Ok(u) = run_hdc_shell(dev, &["bm", "get", "-u"], 30).await {
            let u = u.trim();
            if !u.is_empty() && !u.contains("error") {
                device_udid = Some(u.to_string());
            }
        }
    }
    out.push_str(&format!(
        "3. 设备：{}（UDID：{})\n",
        device
            .as_deref()
            .unwrap_or("未检测到在线设备（请连接真机/模拟器）"),
        device_udid.as_deref().unwrap_or("未知")
    ));

    // 4) 签名材料库扫描（~/.ohos/config）
    let materials = scan_sign_materials();
    out.push_str(&format!(
        "4. 本地签名材料（~/.ohos/config）：{} 套\n",
        materials.len()
    ));
    let mut matched_material: Option<String> = None;
    for (fname, meta) in &materials {
        let mb = meta
            .get("bundle-name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let mtype = meta.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let ids: Vec<&str> = meta
            .get("device-ids")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
            .unwrap_or_default();
        let bundle_ok = bundle.as_deref().is_some_and(|b| mb == b);
        let device_ok = device_udid
            .as_ref()
            .is_some_and(|u| ids.contains(&u.as_str()));
        let flag = if bundle_ok && device_ok {
            "✅ 完全匹配"
        } else if bundle_ok {
            "⚠ bundle 匹配，设备未绑定"
        } else if device_ok {
            "⚠ 设备匹配，bundle 不匹配"
        } else {
            "✗ 不匹配"
        };
        out.push_str(&format!(
            "   - {fname}：bundle={mb}，type={mtype}，绑定额外设备 {} 台，当前设备 {} → {flag}\n",
            ids.len(),
            device_udid
                .as_deref()
                .map(|u| u.chars().take(12).collect::<String>() + "…")
                .unwrap_or_default()
        ));
        if bundle_ok && device_ok && matched_material.is_none() {
            matched_material = Some(fname.clone());
        }
    }
    // 当前配置引用的 profile 是否在材料库中且匹配
    let current_ok = current_profile
        .as_ref()
        .and_then(|p| Path::new(p).file_name())
        .and_then(|f| materials.iter().find(|(n, _)| n == &f.to_string_lossy()))
        .is_some_and(|(_, m)| {
            let mb = m.get("bundle-name").and_then(|v| v.as_str()).unwrap_or("");
            let ids: Vec<String> = m
                .get("device-ids")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            bundle.as_deref().is_some_and(|b| b == mb)
                && device_udid.as_ref().is_some_and(|u| ids.contains(u))
        });

    // 5) 结论与修复指引
    out.push_str("\n5. 结论与建议：\n");
    if cfgs.is_empty() {
        if let Some(m) = &matched_material {
            out.push_str(&format!("   • 未配置签名，但材料库中存在匹配材料 {m}。\n"));
            out.push_str("   • 修复路径：请在 build-profile.json5 的 app.signingConfigs 添加配置并引用该材料\n");
            out.push_str("     （certpath/profile/storeFile 指向 ~/.ohos/config 下对应 .cer/.p7b/.p12，keyAlias=debugKey，signAlg=SHA256withECDSA）；\n");
            out.push_str("     或复用工作区内其他鸿蒙工程（同 bundle）已配置的 signingConfigs（含密码字段，复制即可）。\n");
        } else {
            out.push_str("   • 材料库中没有与当前 bundle+设备匹配的 profile。\n");
            out.push_str("   • 只能通过 DevEco Studio（File → Project Structure → Signing Configs）登录华为账号自动生成签名（生成后材料自动落入 ~/.ohos/config，下次自检即可自动修复）。\n");
        }
        out.push_str("   • 建议：配置完成后调用 build_project 重新构建，再 deploy。\n");
    } else if current_ok {
        out.push_str("   ✅ 当前签名配置与 bundle、设备完全匹配——直接 build_project 重新构建即可产出已签名 HAP，随后 deploy。\n");
        out.push_str(
            "     注意：若此前部署报 9568319，多为部署了旧的 unsigned HAP 产物，重新构建可解决。\n",
        );
    } else if let Some(m) = &matched_material {
        out.push_str(&format!(
            "   • 当前配置的 profile 与设备/bundle 不匹配，但材料库 {m} 匹配。\n"
        ));
        out.push_str("   • 修复路径：用 edit_file 把 build-profile.json5 的 signingConfigs 中 certpath/profile/storeFile 改为材料库匹配项（或整体复用匹配工程的配置），再 build_project + deploy。\n");
    } else {
        out.push_str("   • 当前签名配置与设备/bundle 不匹配，且材料库无匹配项。\n");
        out.push_str("   • 可选：调整 AppScope/app.json5 的 bundleName 与现有 profile 一致（需评估应用身份影响）；或用 DevEco Studio 重新生成签名。\n");
    }
    Ok(out)
}

#[cfg(test)]
mod build_workflow_tests {
    use super::*;

    fn device(
        connection: &str,
        authorized: bool,
        capabilities: &[&str],
    ) -> crate::commands::devices::DeviceInfo {
        crate::commands::devices::DeviceInfo {
            id: "device-1".into(),
            state: if authorized {
                "Connected"
            } else {
                "Unauthorized"
            }
            .into(),
            model: String::new(),
            os_version: String::new(),
            connection: connection.into(),
            authorized,
            api_level: Some(18),
            architecture: "arm64-v8a".into(),
            resolution: "1080x2400".into(),
            capabilities: capabilities
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            observed_at: 1,
            is_default: false,
        }
    }

    #[test]
    fn build_request_validates_dependency_policy() {
        let root = std::env::temp_dir().join(format!("build-request-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("entry")).unwrap();
        std::fs::write(
            root.join("build-profile.json5"),
            r#"{"modules":[{"name":"entry","srcPath":"./entry"}]}"#,
        )
        .unwrap();
        let spec = BuildRequest::from_args(&serde_json::json!({}))
            .unwrap()
            .resolve(&root, Some("entry"))
            .unwrap();
        assert_eq!(spec.dependencies, "auto");
        assert!(
            BuildRequest::from_args(&serde_json::json!({"dependencies":"sometimes"}))
                .unwrap()
                .resolve(&root, Some("entry"))
                .is_err()
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn deploy_requires_online_authorized_device_capabilities() {
        let ready = device("online", true, &["shell", "install", "ability", "hilog"]);
        assert!(ensure_deploy_device_ready(&ready).is_ok());

        let offline = device("offline", false, &[]);
        assert!(ensure_deploy_device_ready(&offline)
            .unwrap_err()
            .contains("不可部署"));

        let incomplete = device("online", true, &["shell", "install"]);
        let error = ensure_deploy_device_ready(&incomplete).unwrap_err();
        assert!(error.contains("ability"));
        assert!(error.contains("hilog"));
    }

    #[test]
    fn recovery_only_removes_an_app_created_by_this_deploy() {
        assert!(should_recover_fresh_install(false));
        assert!(!should_recover_fresh_install(true));
    }

    #[test]
    fn deployment_failures_distinguish_authorization_and_install_conflicts() {
        assert_eq!(classify_deploy_error("device unauthorized", true).0, "device_authorization_denied");
        assert_eq!(classify_deploy_error("INSTALL_FAILED_UPDATE_INCOMPATIBLE: signature mismatch", true).0, "install_conflict");
        assert_eq!(classify_deploy_error("sign verify failed", true).0, "signing");
        assert_eq!(classify_deploy_error("INSTALL_FAILED_VERSION_DOWNGRADE", true).0, "version_downgrade");
    }

    #[test]
    fn multi_device_strategy_is_bounded_and_explicit() {
        assert_eq!(
            multi_deploy_concurrency(&serde_json::json!({}), 8).unwrap(),
            ("parallel", 2)
        );
        assert_eq!(
            multi_deploy_concurrency(
                &serde_json::json!({"strategy":"parallel","max_parallel":99}),
                3,
            )
            .unwrap(),
            ("parallel", 3)
        );
        assert_eq!(
            multi_deploy_concurrency(&serde_json::json!({"strategy":"serial"}), 4).unwrap(),
            ("serial", 1)
        );
        assert!(multi_deploy_concurrency(&serde_json::json!({"strategy":"burst"}), 2).is_err());
    }
}
