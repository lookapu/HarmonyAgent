//! 工程创建域工具：create_harmony_project —— 一次性生成完整标准 HarmonyOS 工程骨架。
//!
//! 设计目标：把"创建工程"从模型逐文件手写拼凑（易漏 .gitignore/README/hvigorw 脚本/
//! 单测骨架/图标资源等）变成模板化一次生成，创建即完整、构建即通过。
//!
//! 模板占位符（写入前 replace，避免 format! 与 JSON 花括号冲突）：
//! - __APP_NAME__：应用显示名
//! - __APP_NAME_LOWER__：应用名小写（包名缺省值用）
//! - __BUNDLE_NAME__：包名
//! - __MODULE__：入口模块名（缺省 entry）
//! - __SDK_VERSION__：SDK 版本字符串，形如 6.1.1(24)

use super::*;

/// create_harmony_project：创建完整标准 HarmonyOS 工程（Stage 模型）。
pub async fn create_harmony_project(args: &Value, roots: &[String]) -> Result<String, String> {
    // ---------- 参数解析 ----------
    let raw_path = args["path"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty());
    let root: PathBuf = match raw_path {
        Some(p) => resolve_for_write(roots, p)?,
        None => roots
            .first()
            .map(PathBuf::from)
            .ok_or("create_harmony_project 需要 {\"path\":\"<工程目录>\"} 或绑定项目目录".to_string())?,
    };
    let dir_name = root
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("无法从路径取目录名：{}", root.display()))?;
    let app_name = args["name"]
        .as_str()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(dir_name)
        .to_string();
    let app_name_lower = app_name.to_lowercase();
    let bundle_name = args["bundle_name"]
        .as_str()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| format!("com.example.{app_name_lower}"));
    if !bundle_name
        .split('.')
        .all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    {
        return Err(format!("bundle_name 非法：{bundle_name}（应形如 com.example.app，各段仅允许字母/数字/下划线）"));
    }
    // copy_signing_from：复用参考工程的包名与签名配置（签名材料与 profile 绑定 bundleName，
    // 因此包名以参考工程为准；显式 bundle_name 与之冲突时以参考工程为准并提示）
    let (bundle_name, signing_json, signing_name, signing_warnings) = resolve_signing_reference(
        args["copy_signing_from"].as_str(),
        roots,
        &bundle_name,
    )?;
    let module = args["module"]
        .as_str()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("entry")
        .to_string();
    if !module.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("module 名非法：{module}（仅允许字母/数字/下划线）"));
    }
    let with_tests = args["with_tests"].as_bool().unwrap_or(true);

    // ---------- 目标目录校验：不存在或为空，禁止覆盖已有工程 ----------
    if root.exists() {
        if !root.is_dir() {
            return Err(format!("目标路径已存在且不是目录：{}", root.display()));
        }
        let entries: Vec<_> = std::fs::read_dir(&root)
            .map_err(|e| format!("读取目录失败 {}：{e}", root.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name() != ".git")
            .collect();
        if !entries.is_empty() {
            return Err(format!(
                "目标目录非空（{} 个条目），create_harmony_project 仅支持空目录/不存在目录，避免覆盖已有工程：{}",
                entries.len(),
                root.display()
            ));
        }
    }
    std::fs::create_dir_all(&root).map_err(|e| format!("创建目录失败 {}：{e}", root.display()))?;

    // ---------- SDK 版本探测（显式参数 > DEVECO_SDK_HOME > DevEco 工具链） ----------
    let sdk_version = detect_sdk_version(args["sdk_version"].as_str())?;

    // ---------- 生成全部文件 ----------
    let mut created: Vec<String> = Vec::new();
    let mut push = |rel: &str, content: &str| -> Result<(), String> {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录失败 {}：{e}", parent.display()))?;
        }
        std::fs::write(&p, content)
            .map_err(|e| format!("写入文件失败 {}：{e}", p.display()))?;
        created.push(rel.to_string());
        Ok(())
    };

    // 根配置文件；copy_signing_from 时向 build-profile.json5 注入参考工程签名配置，
    // 并在 products[0] 挂上 signingConfig 引用，使新工程直接产出签名 HAP
    let root_bp_content = match &signing_json {
        Some(sig_json) => {
            let out = fill(&TEMPLATE_ROOT_BUILD_PROFILE, &app_name, &bundle_name, &module, &sdk_version);
            inject_signing_into_root_build_profile(&out, &sdk_version, sig_json, &signing_name)
        }
        None => fill(&TEMPLATE_ROOT_BUILD_PROFILE, &app_name, &bundle_name, &module, &sdk_version),
    };
    push("build-profile.json5", &root_bp_content)?;
    push("oh-package.json5", &fill(&TEMPLATE_ROOT_OH_PACKAGE, &app_name, &bundle_name, &module, &sdk_version))?;
    push("hvigorfile.ts", &fill(&TEMPLATE_ROOT_HVIGORFILE, &app_name, &bundle_name, &module, &sdk_version))?;
    // hvigor-config.json5 必须位于 hvigor/ 子目录（hvigor 按此路径查找，根目录同名字文件不生效）
    push("hvigor/hvigor-config.json5", &fill(&TEMPLATE_HVIGOR_CONFIG, &app_name, &bundle_name, &module, &sdk_version))?;
    push("code-linter.json5", &fill(&TEMPLATE_CODE_LINTER, &app_name, &bundle_name, &module, &sdk_version))?;
    push(".gitignore", TEMPLATE_GITIGNORE)?;
    push(".hvigorignore", TEMPLATE_HVIGORIGNORE)?;
    push("README.md", &fill(TEMPLATE_README, &app_name, &bundle_name, &module, &sdk_version))?;

    // hvigor 启动脚本：优先从 DevEco 工具链拷贝（配对可靠），失败降级生成内置脚本。
    // 拷贝结果先暂存，文件清单在闭包结束后合并（避免与闭包捕获 created 的可变借用冲突）
    let copied_hvigorw = install_hvigorw(&root);
    if copied_hvigorw.is_empty() {
        push("hvigorw.bat", TEMPLATE_HVIGORW_BAT)?;
        push("hvigor/hvigor-wrapper.js", TEMPLATE_HVIGOR_WRAPPER)?;
    }

    // AppScope
    push("AppScope/app.json5", &fill(TEMPLATE_APP_JSON, &app_name, &bundle_name, &module, &sdk_version))?;
    push(
        "AppScope/resources/base/element/string.json",
        &fill(TEMPLATE_APP_STRING, &app_name, &bundle_name, &module, &sdk_version),
    )?;
    push(
        "AppScope/resources/zh_CN/element/string.json",
        &fill(TEMPLATE_APP_STRING_ZH, &app_name, &bundle_name, &module, &sdk_version),
    )?;
    push(
        "AppScope/resources/en_US/element/string.json",
        &fill(TEMPLATE_APP_STRING_EN, &app_name, &bundle_name, &module, &sdk_version),
    )?;
    // 入口模块
    push(
        &format!("{module}/build-profile.json5"),
        &fill(TEMPLATE_MODULE_BUILD_PROFILE, &app_name, &bundle_name, &module, &sdk_version),
    )?;
    push(
        &format!("{module}/hvigorfile.ts"),
        &fill(TEMPLATE_MODULE_HVIGORFILE, &app_name, &bundle_name, &module, &sdk_version),
    )?;
    push(
        &format!("{module}/oh-package.json5"),
        &fill(TEMPLATE_MODULE_OH_PACKAGE, &app_name, &bundle_name, &module, &sdk_version),
    )?;
    push(
        &format!("{module}/src/main/module.json5"),
        &fill(TEMPLATE_MODULE_JSON, &app_name, &bundle_name, &module, &sdk_version),
    )?;
    push(
        &format!("{module}/src/main/ets/entryability/EntryAbility.ets"),
        &fill(TEMPLATE_ENTRY_ABILITY, &app_name, &bundle_name, &module, &sdk_version),
    )?;
    push(
        &format!("{module}/src/main/ets/pages/Index.ets"),
        &fill(TEMPLATE_INDEX, &app_name, &bundle_name, &module, &sdk_version),
    )?;
    push(
        &format!("{module}/src/main/resources/base/element/string.json"),
        &fill(TEMPLATE_MODULE_STRING, &app_name, &bundle_name, &module, &sdk_version),
    )?;
    push(
        &format!("{module}/src/main/resources/zh_CN/element/string.json"),
        &fill(TEMPLATE_MODULE_STRING_ZH, &app_name, &bundle_name, &module, &sdk_version),
    )?;
    push(
        &format!("{module}/src/main/resources/en_US/element/string.json"),
        &fill(TEMPLATE_MODULE_STRING_EN, &app_name, &bundle_name, &module, &sdk_version),
    )?;
    push(
        &format!("{module}/src/main/resources/base/element/color.json"),
        TEMPLATE_MODULE_COLOR,
    )?;
    push(
        &format!("{module}/src/main/resources/base/profile/main_pages.json"),
        TEMPLATE_MAIN_PAGES,
    )?;
    // 单元测试骨架（hypium）
    if with_tests {
        push(
            &format!("{module}/src/test/module.json5"),
            &fill(TEMPLATE_TEST_MODULE_JSON, &app_name, &bundle_name, &module, &sdk_version),
        )?;
        push(
            &format!("{module}/src/test/oh-package.json5"),
            &fill(TEMPLATE_TEST_OH_PACKAGE, &app_name, &bundle_name, &module, &sdk_version),
        )?;
        push(
            &format!("{module}/src/test/List.test.ets"),
            &fill(TEMPLATE_TEST_LIST, &app_name, &bundle_name, &module, &sdk_version),
        )?;
        push(
            &format!("{module}/src/test/ets/testability/TestAbility.ets"),
            &fill(TEMPLATE_TEST_ABILITY, &app_name, &bundle_name, &module, &sdk_version),
        )?;
        push(
            &format!("{module}/src/test/ets/pages/Index.ets"),
            TEMPLATE_TEST_INDEX,
        )?;
        push(
            &format!("{module}/src/test/resources/base/element/string.json"),
            &fill(TEMPLATE_TEST_STRING, &app_name, &bundle_name, &module, &sdk_version),
        )?;
        push(
            &format!("{module}/src/test/resources/base/element/color.json"),
            TEMPLATE_MODULE_COLOR,
        )?;
        push(
            &format!("{module}/src/test/resources/base/profile/test_pages.json"),
            TEMPLATE_TEST_PAGES,
        )?;
    }

    // 图标（PNG 纯色占位，代码生成避免手写二进制）：统一放在所有 push 之后写入，
    // 避免闭包捕获 created 的可变借用与直接操作冲突
    let app_icon = crate::utils::png::encode_solid_png(256, 256, &[10, 89, 247])?;
    for rel in [
        "AppScope/resources/base/media/app_icon.png".to_string(),
        format!("{module}/src/main/resources/base/media/icon.png"),
    ] {
        let p = root.join(&rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败 {}：{e}", parent.display()))?;
        }
        std::fs::write(&p, &app_icon).map_err(|e| format!("写入图标失败 {}：{e}", p.display()))?;
        created.push(rel);
    }
    created.extend(copied_hvigorw); // hvigorw 启动脚本清单（拷贝成功时）

    // ---------- 创建后自检：关键文件齐全 + 根配置可解析 ----------
    let mut problems = Vec::new();
    for must in [
        "build-profile.json5",
        "oh-package.json5",
        "hvigorfile.ts",
        "AppScope/app.json5",
        &format!("{module}/src/main/module.json5"),
    ] {
        if !root.join(must).is_file() {
            problems.push(must.to_string());
        }
    }
    let root_bp = std::fs::read_to_string(root.join("build-profile.json5"))
        .map_err(|e| format!("读取生成的 build-profile.json5 失败：{e}"))?;
    crate::services::harmony::parse_json5(&root_bp)
        .map_err(|e| format!("生成的 build-profile.json5 无法解析（模板缺陷）：{e}\n{}", root_bp))?;
    if !problems.is_empty() {
        return Err(format!("创建后自检失败，缺失关键文件：{}", problems.join(", ")));
    }

    let tests_note = if with_tests {
        format!(
            "- 单元测试骨架：{module}/src/test/（hypium，依赖在 {module}/oh-package.json5 devDependencies，需执行 ohpm install 后可用）\n"
        )
    } else {
        String::new()
    };
    let signing_note = if signing_json.is_some() {
        format!(
            "- 签名：已复用参考工程签名配置（signingConfigs={signing_name}，包名 {bundle_name}），构建产物可直接安装真机\n{}",
            if signing_warnings.is_empty() {
                String::new()
            } else {
                format!("- 签名警告：{}\n", signing_warnings.join("；"))
            }
        )
    } else {
        "- 签名：signingConfigs 为空，当前仅能产出 unsigned HAP；部署真机前需在 DevEco Studio（File → Project Structure → Signing Configs）配置自动签名，或创建时传 copy_signing_from 复用已有工程的签名\n".to_string()
    };
    Ok(format!(
        "已创建完整 HarmonyOS 工程（Stage 模型）：{}\n\n生成 {} 个文件：\n{}\n\
         关键信息：\n\
         - 应用名：{app_name}，包名：{bundle_name}，SDK：{sdk_version}\n\
         - 入口模块：{module}（EntryAbility + 首页 Index）\n\
         {tests_note}\
         {signing_note}\
         待办提示：\n\
         - 下一步建议：调用 build_project 构建验证（mode=debug）；有测试依赖时先 ohpm_install\n\
         - 运行 hvigor 测试：hvigorw.bat test",
        root.display(),
        created.len(),
        created.iter().map(|f| format!("  {f}")).collect::<Vec<_>>().join("\n"),
    ))
}

/// 向根 build-profile.json5 模板内容注入签名配置：products[0] 挂 signingConfig 引用，
/// app.signingConfigs 填入实际配置。返回注入后的内容。
fn inject_signing_into_root_build_profile(
    content: &str,
    sdk_version: &str,
    sig_json: &str,
    sig_name: &str,
) -> String {
    let anchor = format!(
        "        \"name\": \"default\",\n        \"compatibleSdkVersion\": \"{sdk_version}\","
    );
    let mut out = content.to_string();
    if out.contains(&anchor) {
        out = out.replacen(
            &anchor,
            &format!(
                "        \"name\": \"default\",\n        \"signingConfig\": \"{sig_name}\",\n        \"compatibleSdkVersion\": \"{sdk_version}\","
            ),
            1,
        );
    }
    out.replacen(
        "\"signingConfigs\": [],",
        &format!("\"signingConfigs\": {sig_json},"),
        1,
    )
}

/// 解析 copy_signing_from 参考工程：复用其包名与 signingConfigs（含签名材料路径处理）。
/// 签名材料与 provisioning profile 绑定 bundleName，因此返回的包名以参考工程为准。
/// 返回 (包名, 签名配置 JSON 字符串, 签名名, 材料缺失警告列表)；未指定或参考工程无签名配置时
/// 返回 (原包名, None, "", 空)。
fn resolve_signing_reference(
    raw: Option<&str>,
    roots: &[String],
    fallback_bundle: &str,
) -> Result<(String, Option<String>, String, Vec<String>), String> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok((fallback_bundle.to_string(), None, String::new(), Vec::new()));
    };
    let ref_root = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        resolve_in_roots(roots, raw)?
    };
    if !ref_root.is_dir() {
        return Err(format!("copy_signing_from 参考工程目录不存在：{}", ref_root.display()));
    }
    // 包名：参考工程 AppScope/app.json5 的 bundleName
    let mut bundle = fallback_bundle.to_string();
    let app_json = ref_root.join("AppScope").join("app.json5");
    if let Ok(text) = std::fs::read_to_string(&app_json) {
        if let Ok(v) = crate::services::harmony::parse_json5(&text) {
            if let Some(b) = v
                .get("app")
                .and_then(|a| a.get("bundleName"))
                .and_then(|b| b.as_str())
                .filter(|b| !b.is_empty())
            {
                bundle = b.to_string();
            }
        }
    }
    // 签名配置：参考工程 build-profile.json5 的 app.signingConfigs
    let bp = ref_root.join("build-profile.json5");
    let bp_text = std::fs::read_to_string(&bp)
        .map_err(|e| format!("读取参考工程 build-profile.json5 失败 {}：{e}", bp.display()))?;
    let v = crate::services::harmony::parse_json5(&bp_text)
        .map_err(|e| format!("解析参考工程 build-profile.json5 失败 {}：{e}", bp.display()))?;
    let Some(cfgs) = v
        .get("app")
        .and_then(|a| a.get("signingConfigs"))
        .and_then(|c| c.as_array())
        .filter(|c| !c.is_empty())
    else {
        return Ok((bundle, None, String::new(), Vec::new()));
    };
    // 材料路径：相对路径转为绝对（相对参考工程根），缺失材料记入警告
    let mut warnings = Vec::new();
    let mut out = Vec::new();
    for c in cfgs {
        let mut c = c.clone();
        if let Some(mat) = c.get_mut("material").and_then(|m| m.as_object_mut()) {
            for key in ["certpath", "profile", "storeFile"] {
                if let Some(p) = mat
                    .get(key)
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    let abs = if Path::new(p).is_absolute() {
                        PathBuf::from(p)
                    } else {
                        ref_root.join(p)
                    };
                    if !abs.is_file() {
                        warnings.push(format!("{key} 不存在：{}", abs.display()));
                    }
                    mat.insert(key.to_string(), serde_json::Value::String(abs.to_string_lossy().to_string()));
                }
            }
        }
        out.push(c);
    }
    let sig_name = out
        .first()
        .and_then(|c| c.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("default")
        .to_string();
    let json = serde_json::to_string_pretty(&serde_json::Value::Array(out))
        .map_err(|e| format!("序列化签名配置失败：{e}"))?;
    Ok((bundle, Some(json), sig_name, warnings))
}

/// 从 DevEco Studio 工具链或软件内置 toolkits 拷贝 hvigorw 启动脚本
/// （hvigorw.bat / hvigorw / hvigorw.js）到工程根。
/// 返回拷贝成功的文件名列表；为空表示失败（调用方降级生成内置脚本）。
fn install_hvigorw(root: &Path) -> Vec<String> {
    // 候选工具链：DevEco Studio 内置 > 软件内置 toolkits（Command Line Tools 自带 hvigor）
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some((hvigorw_js, _sdk)) = crate::services::harmony::find_deveco_toolchain() {
        candidates.push(hvigorw_js);
    }
    if let Some(tk) = crate::services::harmony_env::get_bundled_cli_dir() {
        candidates.push(tk.join("hvigor").join("bin").join("hvigorw.js"));
        if let Some(parent) = tk.parent() {
            candidates.push(parent.join("hvigor").join("bin").join("hvigorw.js"));
        }
    }
    for engine in candidates {
        if !engine.is_file() {
            continue;
        }
        let Some(bin) = engine.parent() else {
            continue;
        };
        let mut copied = Vec::new();
        for name in ["hvigorw.bat", "hvigorw", "hvigorw.js"] {
            let src = bin.join(name);
            let dst = root.join(name);
            if std::fs::copy(&src, &dst).is_ok() {
                copied.push(name.to_string());
            }
        }
        if !copied.is_empty() {
            return copied;
        }
    }
    Vec::new()
}

/// 探测 SDK 版本字符串（形如 6.1.1(24)）：
/// 显式参数 > DEVECO_SDK_HOME > DevEco Studio 工具链 SDK。
fn detect_sdk_version(explicit: Option<&str>) -> Result<String, String> {
    if let Some(v) = explicit.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if !is_sdk_version_like(v) {
            return Err(format!("sdk_version 格式非法：{v}（应为 平台版本(API版本) 字符串，如 6.1.1(24)）"));
        }
        return Ok(v.to_string());
    }
    // DEVECO_SDK_HOME（hvigor 构建实际使用的 SDK）
    if let Ok(home) = std::env::var("DEVECO_SDK_HOME") {
        let p = Path::new(&home).join("default").join("sdk-pkg.json");
        if let Some(v) = sdk_version_from_pkg(&p) {
            return Ok(v);
        }
    }
    // DevEco Studio 工具链 SDK
    if let Some((_, sdk)) = crate::services::harmony::find_deveco_toolchain() {
        if let Some(v) = sdk_version_from_pkg(&sdk.join("default").join("sdk-pkg.json")) {
            return Ok(v);
        }
    }
    // command-line-tools SDK（盘符扫描/手动配置/软件内置导入均可能）——与构建端
    // hvigor_env 的 DEVECO_SDK_HOME 兜底顺序一致，保证生成的项目匹配构建实际使用的 SDK
    if let Some(v) = cli_sdk_version() {
        return Ok(v);
    }
    Err("无法探测本机 HarmonyOS SDK 版本：DEVECO_SDK_HOME 未设置且未找到 DevEco Studio 内置 SDK。\n请安装/配置 DevEco Studio（或 SDK），或用 sdk_version 参数显式指定（如 6.1.1(24)）。".into())
}

/// 从探测缓存的 command-line-tools 读取 SDK 版本（<cli_root>/sdk/default/sdk-pkg.json）。
fn cli_sdk_version() -> Option<String> {
    let cli = crate::services::harmony_env::cached_cli_root()?;
    sdk_version_from_pkg(&cli.join("sdk").join("default").join("sdk-pkg.json"))
}

/// 从 sdk-pkg.json 读取 platformVersion 与 apiVersion，拼成 "6.1.1(24)"。
fn sdk_version_from_pkg(pkg_path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(pkg_path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let platform = v["data"]["platformVersion"].as_str()?;
    let api = v["data"]["apiVersion"].as_str()?;
    if platform.is_empty() || api.is_empty() {
        return None;
    }
    Some(format!("{platform}({api})"))
}

/// SDK 版本字符串形如 "6.1.1(24)" 或 "5.0.0(12)"：数字点分 + 括号数字。
fn is_sdk_version_like(s: &str) -> bool {
    let Some(open) = s.find('(') else { return false };
    let Some(close) = s.rfind(')') else { return false };
    if !s.ends_with(')') {
        return false;
    }
    let platform = &s[..open];
    let api = &s[open + 1..close];
    platform
        .split('.')
        .all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_ascii_digit()))
        && !api.is_empty()
        && api.chars().all(|c| c.is_ascii_digit())
}

/// 模板占位符替换
fn fill(tpl: &str, app_name: &str, bundle_name: &str, module: &str, sdk_version: &str) -> String {
    tpl.replace("__APP_NAME__", app_name)
        .replace("__APP_NAME_LOWER__", &app_name.to_lowercase())
        .replace("__BUNDLE_NAME__", bundle_name)
        .replace("__MODULE__", module)
        .replace("__SDK_VERSION__", sdk_version)
}

// ==================== 模板常量（占位符 __XX__） ====================

/// 根 build-profile.json5：signingConfigs 留空（部署真机前需配置签名），SDK 版本注入。
const TEMPLATE_ROOT_BUILD_PROFILE: &str = r#"{
  "app": {
    "signingConfigs": [],
    "products": [
      {
        "name": "default",
        "compatibleSdkVersion": "__SDK_VERSION__",
        "runtimeOS": "HarmonyOS",
        "buildOption": {
          "strictMode": {
            "caseSensitiveCheck": true,
            "useNormalizedOHMUrl": true
          }
        },
        "targetSdkVersion": "__SDK_VERSION__"
      }
    ],
    "buildModeSet": [
      {
        "name": "debug"
      },
      {
        "name": "release"
      }
    ]
  },
  "modules": [
    {
      "name": "__MODULE__",
      "srcPath": "./__MODULE__",
      "targets": [
        {
          "name": "default",
          "applyToProducts": [
            "default"
          ]
        }
      ]
    }
  ]
}
"#;

/// 根 oh-package.json5
const TEMPLATE_ROOT_OH_PACKAGE: &str = r#"{
  "modelVersion": "5.0.1",
  "description": "Please describe the basic information.",
  "dependencies": {},
  "devDependencies": {}
}
"#;

/// 根 hvigorfile.ts
const TEMPLATE_ROOT_HVIGORFILE: &str = r#"import { appTasks } from '@ohos/hvigor-ohos-plugin';

export default {
    system: appTasks,  /* Built-in plugin of Hvigor. It cannot be modified. */
    plugins:[]         /* Custom plugin to extend the functionality of Hvigor. */
}
"#;

/// hvigor-config.json5（位于 hvigor/ 子目录）
const TEMPLATE_HVIGOR_CONFIG: &str = r#"{
  "modelVersion": "5.0.1",
  "dependencies": {
  }
}
"#;

/// code-linter.json5
const TEMPLATE_CODE_LINTER: &str = r#"{
  "files": [
    "**/*.ets"
  ],
  "ignore": [
    "**/src/ohosTest/**/*",
    "**/src/test/**/*",
    "**/src/mock/**/*",
    "**/node_modules/**/*",
    "**/oh_modules/**/*",
    "**/build/**/*",
    "**/.preview/**/*"
  ],
  "ruleSet": [
    "plugin:@performance/recommended",
    "plugin:@typescript-eslint/recommended"
  ],
  "rules": {
  }
}
"#;

/// 根 .gitignore（DevEco 标准 + DevEco Switch 辅助目录）
const TEMPLATE_GITIGNORE: &str = r#"/node_modules
/oh_modules
/local.properties
/.idea
**/build
/.hvigor
.cxx
/.clangd
/.clang-format
/.clang-tidy
**/.test
/.appanalyzer
/.deveco-agent
/dist
"#;

/// 根 .hvigorignore
const TEMPLATE_HVIGORIGNORE: &str = r#"/.hvigor
**/build/**
**/.preview/**
**/.test/**
"#;

/// 根 README.md
const TEMPLATE_README: &str = r#"# __APP_NAME__

基于 DevEco Switch 创建的标准 HarmonyOS Stage 模型工程（API 版本见 build-profile.json5，SDK：__SDK_VERSION__）。

## 工程结构

- `AppScope/`：应用级配置（bundleName、版本、图标、多语言）
- `__MODULE__/`：入口模块（EntryAbility + 首页 + 资源）
- `__MODULE__/src/test/`：hypium 单元测试骨架

## 构建

- 命令行：`hvigorw.bat assembleHap`（Windows）/ `./hvigorw assembleHap`（macOS/Linux），构建产物位于 `__MODULE__/build/default/outputs/default/`
- DevEco Studio：File → Open 打开本目录，等待 Sync 完成后可直接构建运行

## 部署

真机/模拟器安装前需在 DevEco Studio 配置签名（File → Project Structure → Signing Configs → 自动签名）；未签名 HAP（unsigned）无法安装。

## 测试

`hvigorw.bat test` 运行 hypium 单元测试（首次需先执行 ohpm install 安装测试依赖）。
"#;

/// 降级 hvigorw.bat（无 DevEco 工具链可拷贝时）：调用 hvigor/hvigor-wrapper.js
const TEMPLATE_HVIGORW_BAT: &str = r#"@echo off
setlocal
set DIRNAME=%~dp0
if "%DIRNAME%" == "" set DIRNAME=.
set WRAPPER=%DIRNAME%\hvigor\hvigor-wrapper.js
if not exist "%WRAPPER%" (
  echo ERROR: %WRAPPER% not found.
  exit /b 1
)
node "%WRAPPER%" %*
exit /b %ERRORLEVEL%
"#;

/// 降级 hvigor/hvigor-wrapper.js：定位 DevEco Studio 工具链 hvigor 并转发
const TEMPLATE_HVIGOR_WRAPPER: &str = r#"// hvigor-wrapper.js —— DevEco Switch 生成的最小包装脚本。
// 定位 DevEco Studio 工具链 hvigor（tools/hvigor/bin/hvigorw.js）并转发；
// 未找到时给出明确提示（也可在 DevEco Studio 中打开工程让其补全标准脚本）。
'use strict';
const fs = require('fs');
const path = require('path');
const childProcess = require('child_process');

function findHvigorwJs() {
  const roots = [process.env.DEVECO_HOME, process.env.DEVECO_STUDIO_HOME]
    .filter(Boolean);
  if (process.platform === 'win32') {
    roots.push('C:\\Program Files\\Huawei\\DevEco Studio');
    try {
      for (const d of fs.readdirSync('C:\\Program Files\\Huawei')) {
        if (d.startsWith('DevEco')) {
          roots.push(path.join('C:\\Program Files\\Huawei', d));
        }
      }
    } catch (e) { /* 目录不存在时忽略 */ }
  }
  for (const root of roots) {
    const p = path.join(root, 'tools', 'hvigor', 'bin', 'hvigorw.js');
    if (fs.existsSync(p)) return p;
  }
  return null;
}

const hvigorwJs = findHvigorwJs();
if (!hvigorwJs) {
  console.error('未找到 DevEco Studio 工具链 hvigor（hvigorw.js）。请安装 DevEco Studio，或在 IDE 中打开本工程让其补全构建脚本。');
  process.exit(1);
}
const result = childProcess.spawnSync(process.execPath, [hvigorwJs].concat(process.argv.slice(2)), { stdio: 'inherit' });
process.exit(result.status === null ? 1 : result.status);
"#;

/// AppScope/app.json5
const TEMPLATE_APP_JSON: &str = r#"{
  "app": {
    "bundleName": "__BUNDLE_NAME__",
    "vendor": "example",
    "versionCode": 1000000,
    "versionName": "1.0.0",
    "icon": "$media:app_icon",
    "label": "$string:app_name"
  }
}
"#;

/// AppScope/resources/base/element/string.json
const TEMPLATE_APP_STRING: &str = r#"{
  "string": [
    {
      "name": "app_name",
      "value": "__APP_NAME__"
    }
  ]
}
"#;

/// AppScope/resources/zh_CN/element/string.json
const TEMPLATE_APP_STRING_ZH: &str = r#"{
  "string": [
    {
      "name": "app_name",
      "value": "__APP_NAME__"
    }
  ]
}
"#;

/// AppScope/resources/en_US/element/string.json
const TEMPLATE_APP_STRING_EN: &str = r#"{
  "string": [
    {
      "name": "app_name",
      "value": "__APP_NAME__"
    }
  ]
}
"#;

/// 入口模块 build-profile.json5（stageMode，无混淆配置，release 默认不混淆）
const TEMPLATE_MODULE_BUILD_PROFILE: &str = r#"{
  "apiType": "stageMode",
  "targets": [
    {
      "name": "default"
    }
  ]
}
"#;

/// 入口模块 hvigorfile.ts
const TEMPLATE_MODULE_HVIGORFILE: &str = r#"import { hapTasks } from '@ohos/hvigor-ohos-plugin';

export default {
    system: hapTasks,  /* Built-in plugin of Hvigor. It cannot be modified. */
    plugins:[]         /* Custom plugin to extend the functionality of Hvigor. */
}
"#;

/// 入口模块 oh-package.json5（hypium 测试依赖随 with_tests 一并声明）
const TEMPLATE_MODULE_OH_PACKAGE: &str = r#"{
  "name": "__MODULE__",
  "version": "1.0.0",
  "description": "Please describe the basic information.",
  "main": "",
  "author": "",
  "license": "",
  "dependencies": {},
  "devDependencies": {
    "@ohos/hypium": "1.0.19",
    "@ohos/hamock": "1.0.0"
  }
}
"#;

/// 入口模块 src/main/module.json5
const TEMPLATE_MODULE_JSON: &str = r#"{
  "module": {
    "name": "__MODULE__",
    "type": "entry",
    "description": "$string:module_desc",
    "mainElement": "EntryAbility",
    "deviceTypes": [
      "phone",
      "tablet",
      "2in1"
    ],
    "deliveryWithInstall": true,
    "installationFree": false,
    "pages": "$profile:main_pages",
    "abilities": [
      {
        "name": "EntryAbility",
        "srcEntry": "./ets/entryability/EntryAbility.ets",
        "description": "$string:EntryAbility_desc",
        "icon": "$media:icon",
        "label": "$string:EntryAbility_label",
        "startWindowIcon": "$media:icon",
        "startWindowBackground": "$color:start_window_background",
        "exported": true,
        "skills": [
          {
            "entities": [
              "entity.system.home"
            ],
            "actions": [
              "action.system.home"
            ]
          }
        ]
      }
    ]
  }
}
"#;

/// EntryAbility.ets（API 12+ 标准模板）
const TEMPLATE_ENTRY_ABILITY: &str = r#"import { AbilityConstant, UIAbility, Want } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { window } from '@kit.ArkUI';

const TAG: string = 'EntryAbility';

export default class EntryAbility extends UIAbility {
  onCreate(want: Want, launchParam: AbilityConstant.LaunchParam): void {
    hilog.info(0x0000, TAG, 'Ability onCreate');
  }

  onDestroy(): void {
    hilog.info(0x0000, TAG, 'Ability onDestroy');
  }

  onWindowStageCreate(windowStage: window.WindowStage): void {
    hilog.info(0x0000, TAG, 'Ability onWindowStageCreate');
    windowStage.loadContent('pages/Index', (err) => {
      if (err.code) {
        hilog.error(0x0000, TAG, 'Failed to load the content. Cause: %{public}s', JSON.stringify(err) ?? '');
        return;
      }
      hilog.info(0x0000, TAG, 'Succeeded in loading the content.');
    });
  }

  onWindowStageDestroy(): void {
    hilog.info(0x0000, TAG, 'Ability onWindowStageDestroy');
  }

  onForeground(): void {
    hilog.info(0x0000, TAG, 'Ability onForeground');
  }

  onBackground(): void {
    hilog.info(0x0000, TAG, 'Ability onBackground');
  }
}
"#;

/// 首页 Index.ets
const TEMPLATE_INDEX: &str = r#"import { hilog } from '@kit.PerformanceAnalysisKit';

const TAG: string = 'Index';

@Entry
@Component
struct Index {
  @State message: string = 'Hello __APP_NAME__';

  build() {
    Row() {
      Column() {
        Text(this.message)
          .fontSize(40)
          .fontWeight(FontWeight.Bold)
      }
      .width('100%')
      .height('100%')
      .justifyContent(FlexAlign.Center)
    }
    .width('100%')
    .height('100%')
    .onAppear(() => {
      hilog.info(0x0000, TAG, 'Index page appeared');
    })
  }
}
"#;

/// 入口模块 base string.json
const TEMPLATE_MODULE_STRING: &str = r#"{
  "string": [
    {
      "name": "module_desc",
      "value": "module description"
    },
    {
      "name": "EntryAbility_desc",
      "value": "description"
    },
    {
      "name": "EntryAbility_label",
      "value": "__APP_NAME__"
    }
  ]
}
"#;

/// 入口模块 zh_CN string.json
const TEMPLATE_MODULE_STRING_ZH: &str = r#"{
  "string": [
    {
      "name": "module_desc",
      "value": "模块描述"
    },
    {
      "name": "EntryAbility_desc",
      "value": "描述"
    },
    {
      "name": "EntryAbility_label",
      "value": "__APP_NAME__"
    }
  ]
}
"#;

/// 入口模块 en_US string.json
const TEMPLATE_MODULE_STRING_EN: &str = r#"{
  "string": [
    {
      "name": "module_desc",
      "value": "module description"
    },
    {
      "name": "EntryAbility_desc",
      "value": "description"
    },
    {
      "name": "EntryAbility_label",
      "value": "__APP_NAME__"
    }
  ]
}
"#;

/// 入口模块 color.json（含 "# 序列，需三重引号原始字符串）
const TEMPLATE_MODULE_COLOR: &str = r##"{
  "color": [
    {
      "name": "start_window_background",
      "value": "#FFFFFF"
    }
  ]
}
"##;

/// main_pages.json
const TEMPLATE_MAIN_PAGES: &str = r#"{
  "src": [
    "pages/Index"
  ]
}
"#;

/// 测试模块 module.json5（不引用 media 图标，保证资源自洽可构建）
const TEMPLATE_TEST_MODULE_JSON: &str = r#"{
  "module": {
    "name": "__MODULE___test",
    "type": "feature",
    "description": "$string:module_test_desc",
    "mainElement": "TestAbility",
    "deviceTypes": [
      "phone",
      "tablet",
      "2in1"
    ],
    "deliveryWithInstall": true,
    "installationFree": false,
    "pages": "$profile:test_pages",
    "abilities": [
      {
        "name": "TestAbility",
        "srcEntry": "./ets/testability/TestAbility.ets",
        "description": "$string:TestAbility_desc",
        "label": "$string:TestAbility_label",
        "startWindowBackground": "$color:start_window_background",
        "exported": true,
        "skills": [
          {
            "entities": [
              "entity.system.home"
            ],
            "actions": [
              "action.system.home"
            ]
          }
        ]
      }
    ],
    "dependencies": [
      {
        "module": "__MODULE__"
      }
    ]
  }
}
"#;

/// 测试模块 oh-package.json5
const TEMPLATE_TEST_OH_PACKAGE: &str = r#"{
  "name": "__MODULE___test",
  "version": "1.0.0",
  "description": "Please describe the basic information.",
  "main": "",
  "author": "",
  "license": "",
  "dependencies": {},
  "devDependencies": {}
}
"#;

/// List.test.ets（hypium 冒烟测试）
const TEMPLATE_TEST_LIST: &str = r#"import { describe, expect, it } from '@ohos/hypium';

export default function abilityTest() {
  describe('ActsAbilityTest', () => {
    it('assertContain', () => {
      expect('Hello DevEco Switch').assertContain('DevEco');
    });
  });
}
"#;

/// TestAbility.ets
const TEMPLATE_TEST_ABILITY: &str = r#"import { AbilityConstant, UIAbility, Want } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { window } from '@kit.ArkUI';

const TAG: string = 'TestAbility';

export default class TestAbility extends UIAbility {
  onCreate(want: Want, launchParam: AbilityConstant.LaunchParam): void {
    hilog.info(0x0000, TAG, 'TestAbility onCreate');
  }

  onDestroy(): void {
    hilog.info(0x0000, TAG, 'TestAbility onDestroy');
  }

  onWindowStageCreate(windowStage: window.WindowStage): void {
    hilog.info(0x0000, TAG, 'TestAbility onWindowStageCreate');
    windowStage.loadContent('pages/Index', (err) => {
      if (err.code) {
        hilog.error(0x0000, TAG, 'Failed to load the content. Cause: %{public}s', JSON.stringify(err) ?? '');
        return;
      }
    });
  }
}
"#;

/// 测试壳页面（test_pages.json 引用，保证模块资源自洽）
const TEMPLATE_TEST_INDEX: &str = r#"@Entry
@Component
struct Index {
  build() {
    Column() {
      Text('Test Module')
        .fontSize(30)
        .fontWeight(FontWeight.Bold)
    }
    .width('100%')
    .height('100%')
    .justifyContent(FlexAlign.Center)
  }
}
"#;

/// 测试模块 string.json
const TEMPLATE_TEST_STRING: &str = r#"{
  "string": [
    {
      "name": "module_test_desc",
      "value": "test module description"
    },
    {
      "name": "TestAbility_desc",
      "value": "description"
    },
    {
      "name": "TestAbility_label",
      "value": "testLabel"
    }
  ]
}
"#;

/// 测试模块 test_pages.json
const TEMPLATE_TEST_PAGES: &str = r#"{
  "src": [
    "pages/Index"
  ]
}
"#;

// ==================== 单元测试 ====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_replaces_all_placeholders() {
        let out = fill(
            "name=__APP_NAME__ bundle=__BUNDLE_NAME__ mod=__MODULE__ sdk=__SDK_VERSION__ lower=__APP_NAME_LOWER__",
            "MyApp",
            "com.example.myapp",
            "entry",
            "6.1.1(24)",
        );
        assert_eq!(out, "name=MyApp bundle=com.example.myapp mod=entry sdk=6.1.1(24) lower=myapp");
    }

    #[test]
    fn inject_signing_wires_products_and_configs() {
        // copy_signing_from 注入后：products[0] 挂 signingConfig 引用，signingConfigs 有实际配置，且整体可解析
        let tpl = fill(TEMPLATE_ROOT_BUILD_PROFILE, "X", "com.example.x", "entry", "6.1.1(24)");
        let sig = r#"[{"name":"default","type":"HarmonyOS","material":{"certpath":"C:\\u.cer","keyAlias":"debugKey","profile":"C:\\u.p7b","storeFile":"C:\\u.p12"}}]"#;
        let out = inject_signing_into_root_build_profile(&tpl, "6.1.1(24)", sig, "default");
        let v = crate::services::harmony::parse_json5(&out).expect("注入后必须可解析");
        assert_eq!(v["app"]["signingConfigs"][0]["name"], "default");
        assert_eq!(v["app"]["products"][0]["signingConfig"], "default");
        assert_eq!(v["app"]["products"][0]["compatibleSdkVersion"], "6.1.1(24)");
    }

    #[test]
    fn resolve_signing_reference_copies_bundle_and_materials() {
        // 参考工程：包名 + 绝对路径材料 → 完整复用，无警告
        let tmp = std::env::temp_dir().join(format!("sig-ref-{}", std::process::id()));
        let ref_root = tmp.join("ref");
        std::fs::create_dir_all(ref_root.join("AppScope")).unwrap();
        std::fs::write(
            ref_root.join("AppScope/app.json5"),
            r#"{"app":{"bundleName":"com.sns.harmony"}}"#,
        )
        .unwrap();
        let mat = tmp.join("m");
        std::fs::create_dir_all(&mat).unwrap();
        for f in ["a.cer", "a.p7b", "a.p12"] {
            std::fs::write(mat.join(f), "x").unwrap();
        }
        let esc = |p: std::path::PathBuf| p.to_string_lossy().replace('\\', "\\\\");
        std::fs::write(
            ref_root.join("build-profile.json5"),
            format!(
                r#"{{"app":{{"signingConfigs":[{{"name":"default","type":"HarmonyOS","material":{{"certpath":"{cer}","keyAlias":"debugKey","profile":"{p7b}","storeFile":"{p12}"}}}}]}}}}"#,
                cer = esc(mat.join("a.cer")),
                p7b = esc(mat.join("a.p7b")),
                p12 = esc(mat.join("a.p12")),
            ),
        )
        .unwrap();
        let (bundle, json, name, warns) =
            resolve_signing_reference(Some(ref_root.to_str().unwrap()), &[], "com.example.fallback")
                .expect("参考工程应解析成功");
        assert_eq!(bundle, "com.sns.harmony");
        assert_eq!(name, "default");
        assert!(warns.is_empty(), "材料存在不应警告: {:?}", warns);
        let v: serde_json::Value = serde_json::from_str(json.as_deref().unwrap()).unwrap();
        assert_eq!(v[0]["material"]["certpath"], mat.join("a.cer").to_string_lossy().to_string());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_signing_reference_missing_material_warns() {
        // 材料缺失 → 三个字段均警告，但不阻断复用
        let tmp = std::env::temp_dir().join(format!("sig-ref-warn-{}", std::process::id()));
        let ref_root = tmp.join("ref");
        std::fs::create_dir_all(ref_root.join("AppScope")).unwrap();
        std::fs::write(
            ref_root.join("AppScope/app.json5"),
            r#"{"app":{"bundleName":"com.x"}}"#,
        )
        .unwrap();
        std::fs::write(
            ref_root.join("build-profile.json5"),
            r#"{"app":{"signingConfigs":[{"name":"default","material":{"certpath":"C:\\nope.cer","keyAlias":"debugKey","profile":"C:\\nope.p7b","storeFile":"C:\\nope.p12"}}]}}"#,
        )
        .unwrap();
        let (_, _, _, warns) =
            resolve_signing_reference(Some(ref_root.to_str().unwrap()), &[], "com.example.x")
                .expect("材料缺失不应报错，应记入警告");
        assert_eq!(warns.len(), 3);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn e2e_create_project_with_signing_reuse() {
        // 端到端：空目录 → create_harmony_project（copy_signing_from 复用参考工程签名/包名）
        // 参考工程取真实 I:\\SNS\\harmony-sns；SDK/工具链缺失时跳过。
        // 产物保留在 H:\\work\\code\\_e2e_new_harmony 供后续构建/部署验证。
        if crate::services::harmony::find_deveco_toolchain().is_none() {
            eprintln!("skip: 未找到 DevEco Studio 工具链");
            return;
        }
        let ref_proj = Path::new(r"<REF_PROJECT>\harmony-sns");
        if !ref_proj.is_dir() {
            eprintln!("skip: 参考工程不存在");
            return;
        }
        let target = Path::new(r"<PROJECT_ROOT>\_e2e_new_harmony");
        let roots = [r"<PROJECT_ROOT>".to_string()];
        let _ = std::fs::remove_dir_all(target);
        let args = serde_json::json!({
            "path": target.to_string_lossy(),
            "copy_signing_from": ref_proj.to_string_lossy(),
            "with_tests": false,
        });
        let out = create_harmony_project(&args, &roots)
            .await
            .expect("create_harmony_project 应成功");
        assert!(out.contains("com.sns.harmony"), "应复用参考工程包名: {out}");
        assert!(out.contains("已复用参考工程签名配置"), "应提示签名复用: {out}");
        for must in [
            "build-profile.json5",
            "AppScope/app.json5",
            "entry/src/main/module.json5",
            "hvigorfile.ts",
            "oh-package.json5",
            "hvigor/hvigor-config.json5",
        ] {
            assert!(target.join(must).is_file(), "缺失 {must}");
        }
        // 签名注入与包名
        let bp = std::fs::read_to_string(target.join("build-profile.json5")).unwrap();
        let v = crate::services::harmony::parse_json5(&bp).expect("build-profile 可解析");
        assert_eq!(v["app"]["signingConfigs"][0]["name"], "default");
        assert_eq!(v["app"]["products"][0]["signingConfig"], "default");
        let cert = v["app"]["signingConfigs"][0]["material"]["certpath"]
            .as_str()
            .unwrap_or("");
        assert!(cert.contains(".cer") && cert.contains(".ohos"), "certpath 异常: {cert}");
        let app_json = std::fs::read_to_string(target.join("AppScope/app.json5")).unwrap();
        let av = crate::services::harmony::parse_json5(&app_json).unwrap();
        assert_eq!(av["app"]["bundleName"], "com.sns.harmony");
    }

    #[test]
    fn install_hvigorw_falls_back_to_toolkits() {
        // 软件内置 toolkits 自带 hvigor 引擎时，启动脚本从 toolkits 拷贝（未装 DevEco 也能生成工程）
        let tmp = std::env::temp_dir().join(format!("hvigorw-tk-{}", std::process::id()));
        let tk = tmp.join("toolkits").join("command-line-tools");
        std::fs::create_dir_all(tk.join("hvigor").join("bin")).unwrap();
        std::fs::write(tk.join("hvigor/bin/hvigorw.js"), "// engine").unwrap();
        std::fs::write(tk.join("hvigor/bin/hvigorw.bat"), "@echo off").unwrap();
        let prev = crate::services::harmony_env::get_bundled_cli_dir();
        crate::services::harmony_env::set_bundled_cli_dir(Some(tk.clone()));
        let proj = tmp.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let copied = install_hvigorw(&proj);
        crate::services::harmony_env::set_bundled_cli_dir(prev);
        assert!(copied.contains(&"hvigorw.js".to_string()), "copied={:?}", copied);
        assert!(proj.join("hvigorw.js").is_file());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn generated_root_build_profile_is_valid_json5() {
        let out = fill(TEMPLATE_ROOT_BUILD_PROFILE, "TestHy", "com.example.testhy", "entry", "6.1.1(24)");
        let v = crate::services::harmony::parse_json5(&out).expect("根 build-profile.json5 模板必须可解析");
        let app = v.get("app").expect("app 段");
        assert_eq!(app["products"][0]["compatibleSdkVersion"], "6.1.1(24)");
        assert_eq!(app["products"][0]["targetSdkVersion"], "6.1.1(24)");
        assert_eq!(v["modules"][0]["name"], "entry");
        assert_eq!(v["modules"][0]["srcPath"], "./entry");
    }

    #[test]
    fn generated_module_and_app_configs_are_valid_json5() {
        for tpl in [
            TEMPLATE_APP_JSON,
            TEMPLATE_MODULE_JSON,
            TEMPLATE_TEST_MODULE_JSON,
            TEMPLATE_ROOT_OH_PACKAGE,
            TEMPLATE_MODULE_OH_PACKAGE,
            TEMPLATE_HVIGOR_CONFIG,
        ] {
            let out = fill(tpl, "TestHy", "com.example.testhy", "entry", "6.1.1(24)");
            crate::services::harmony::parse_json5(&out)
                .unwrap_or_else(|e| panic!("模板不可解析：\n{e}\n{out}"));
        }
    }

    #[test]
    fn generated_json_resources_are_valid_json() {
        // string.json / color.json / main_pages.json 是严格 JSON（json5 解析器兼容）
        for tpl in [
            TEMPLATE_APP_STRING,
            TEMPLATE_APP_STRING_ZH,
            TEMPLATE_APP_STRING_EN,
            TEMPLATE_MODULE_STRING,
            TEMPLATE_MODULE_STRING_ZH,
            TEMPLATE_MODULE_STRING_EN,
            TEMPLATE_MODULE_COLOR,
            TEMPLATE_MAIN_PAGES,
            TEMPLATE_TEST_STRING,
            TEMPLATE_TEST_PAGES,
            TEMPLATE_CODE_LINTER,
        ] {
            let out = fill(tpl, "TestHy", "com.example.testhy", "entry", "6.1.1(24)");
            serde_json::from_str::<serde_json::Value>(&out)
                .unwrap_or_else(|e| panic!("模板不是合法 JSON：\n{e}\n{out}"));
        }
    }

    #[test]
    fn detect_sdk_version_parses_pkg() {
        // 使用内联构造的 sdk-pkg.json 校验拼接格式
        let tmp = std::env::temp_dir().join(format!("dsv_pkg_{}.json", std::process::id()));
        std::fs::write(
            &tmp,
            r#"{"data":{"apiVersion":"24","platformVersion":"6.1.1","version":"6.1.1.125"}}"#,
        )
        .unwrap();
        let v = sdk_version_from_pkg(&tmp);
        std::fs::remove_file(&tmp).ok();
        assert_eq!(v.as_deref(), Some("6.1.1(24)"));
    }

    #[test]
    fn cli_sdk_version_reads_cached_cli_root() {
        // 无 DevEco Studio 场景：CLT 自带 SDK（<cli_root>/sdk/default/sdk-pkg.json）
        // 应能被探测缓存复用（与构建端 DEVECO_SDK_HOME 兜底同一来源）
        let root = std::env::temp_dir().join(format!("dsv_cli_{}", std::process::id()));
        let sdk = root.join("sdk").join("default");
        std::fs::create_dir_all(&sdk).unwrap();
        std::fs::write(
            sdk.join("sdk-pkg.json"),
            r#"{"data":{"apiVersion":"24","platformVersion":"6.1.1","version":"6.1.1.125"}}"#,
        )
        .unwrap();
        crate::services::harmony_env::set_cached_cli_root_for_test(Some(root.clone()));
        assert_eq!(cli_sdk_version().as_deref(), Some("6.1.1(24)"));
        crate::services::harmony_env::set_cached_cli_root_for_test(None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sdk_version_like_validation() {
        assert!(is_sdk_version_like("6.1.1(24)"));
        assert!(is_sdk_version_like("5.0.0(12)"));
        assert!(!is_sdk_version_like("24"));
        assert!(!is_sdk_version_like("6.1.1(24"));
        assert!(!is_sdk_version_like("(24)"));
        assert!(!is_sdk_version_like("6.1.1()"));
    }
}
