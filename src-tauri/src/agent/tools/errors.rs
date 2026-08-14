//! 工具错误处理层：可重试判断、失败模式诊断与结构化错误信封。
//! 纯函数层，不依赖工具实现，可独立测试。



/// 判断错误是否值得自动重试（超时 / 网络类，重试可恢复）
pub fn is_retryable_err(e: &str) -> bool {
    const KEYS: [&str; 6] = ["超时", "请求失败", "timed out", "连接", "network", "timeout"];
    KEYS.iter().any(|k| e.to_lowercase().contains(k))
}

// ---------- 错误模式诊断（常见错误 → 修复建议，帮助 Agent 快速定位，减少打转） ----------

/// 按工具 + 错误文本匹配高频失败模式，返回针对性修复建议
fn diagnose_tool_error(tool: &str, err: &str) -> Option<&'static str> {
    let l = err.to_lowercase();
    let has_any = |keys: &[&str]| keys.iter().any(|k| l.contains(k));
    match tool {
        "build_project" => {
            if has_any(&["arkts", "compile", "编译", "类型错误"]) {
                Some("ArkTS 编译错误：请阅读日志中 ERROR 行定位具体文件与行号，修复类型/API 用法后重新构建")
            } else if has_any(&["signing", "keystore", "certificate", "签名"]) {
                Some("签名配置问题：检查 build-profile.json5 中 signingConfigs 的 keystore 路径与密码")
            } else if has_any(&["compatibleSdkVersion", "api version", "sdk 版本", "sdk version"]) {
                Some("SDK 版本不匹配：检查工程要求的 SDK 版本与本地 DevEco SDK 是否一致，必要时修改 compileSdkVersion")
            } else if has_any(&["out of memory", "heap", "oom"])
                || l.contains("memoryerror")
            {
                Some("构建内存不足：增大 hvigor 构建内存（hvigorw.js nodeOptions）或关闭其他占用内存的应用后重试")
            } else if has_any(&["permission denied", "being used", "lock", "拒绝访问"]) {
                Some("文件被占用或权限不足：关闭 DevEco Studio 后重试，或检查工程目录写入权限")
            } else if has_any(&["ohpm"]) {
                Some("依赖安装问题：检查网络/代理设置，必要时配置 ohpm 镜像源后重试")
            } else if has_any(&["build-profile", "product", "模块配置"]) {
                Some("构建配置问题：检查 build-profile.json5 的 product 定义与模块归属是否正确")
            } else {
                None
            }
        }
        "ohpm_install" => {
            if has_any(&["network", "proxy", "certificate", "ssl", "socket"])
                || l.contains("fetch fail")
            {
                Some("网络问题：检查网络/代理设置，或切换 ohpm registry 镜像源（ohpm config set registry ...）后重试")
            } else if has_any(&["not found", "version", "depend", "冲突", "missing"]) {
                Some("依赖不存在或版本冲突：检查 oh-package.json5 中的包名与版本号，必要时调整版本范围")
            } else if has_any(&["permission denied", "lock", "拒绝访问"]) {
                Some("目录被占用或权限不足：关闭 DevEco Studio 后重试")
            } else {
                None
            }
        }
        "deploy" => {
            if has_any(&["no devices", "not found", "empty", "没有设备"]) {
                Some("没有可用设备：请连接真机或启动模拟器，确认 hdc list targets 有设备输出")
            } else if has_any(&["signature", "certificate", "签名"]) {
                Some("签名不一致：设备上已有不同签名版本的应用，需先卸载旧版本或改用相同签名构建")
            } else if has_any(&["install", "failed", "失败"]) && has_any(&["space", "full", "空间"]) {
                Some("设备空间不足：清理设备存储后重试")
            } else if has_any(&["timeout", "超时"]) {
                Some("安装超时：设备响应慢，可尝试重新连接设备后重试")
            } else {
                None
            }
        }
        "list_devices" => {
            if has_any(&["not recognized", "not found", "无法", "no such file"]) {
                Some("hdc 不在 PATH：请安装 DevEco Studio，或将 hdc 所在目录加入系统 PATH")
            } else if has_any(&["failed", "error", "失败"]) {
                Some("hdc 服务异常：可执行 hdc kill 后重试，或检查设备驱动与 USB 连接")
            } else {
                None
            }
        }
        "list_dir" | "read_file" | "find_files" | "grep_files" => {
            if has_any(&["not found", "不存在", "not a file", "不是文件", "不是目录"]) {
                Some("路径不存在：请先用 list_dir 浏览工程目录结构，确认路径后重试")
            } else if has_any(&["binary", "二进制", "过大", "too large", "5mb"]) {
                Some("文件为二进制或过大：跳过该文件，改用 grep_files 搜索内容或 list_dir 获取元信息")
            } else if has_any(&["outside", "超出", "范围", "拒绝访问"]) {
                Some("路径超出项目目录范围：仅可访问当前工程内的文件，请使用相对路径")
            } else if has_any(&["permission denied", "拒绝", "denied"]) {
                Some("权限不足：检查目录读写权限，或换个目录重试")
            } else {
                None
            }
        }
        "write_file" | "edit_file" => {
            if has_any(&["outside", "超出", "范围", "拒绝访问"]) {
                Some("路径超出项目目录范围：仅可修改当前工程内的文件，请使用相对路径")
            } else if has_any(&["not found", "不存在", "找不到", "不匹配"]) {
                Some("目标内容不匹配：edit_file 的 old 文本须与文件内容完全一致（含缩进/引号），可先 read_file 确认原文")
            } else if has_any(&["过大", "too large", "1mb"]) {
                Some("文件过大：拆分为多次写入，或改用 run_command 执行脚本处理")
            } else if has_any(&["permission denied", "denied", "拒绝"]) {
                Some("写入权限不足：检查文件只读属性或目录权限（DevEco Studio 占用时关闭后重试）")
            } else {
                None
            }
        }
        "run_command" => {
            if has_any(&["危险", "blacklist", "拒绝执行"]) {
                Some("命令被安全策略拒绝：删除/格式化类命令禁止执行，请改用 write_file/edit_file 或 git 工具完成")
            } else if has_any(&["找不到程序", "not found", "no such file"]) {
                Some("程序不存在：确认命令名正确且已安装（如 hvigorw.bat 在工程根目录），或使用完整路径")
            } else if has_any(&["超时", "timed out", "timeout"]) {
                Some("命令超时：构建/测试类命令需要更长时间，可调大 timeout 参数（最长 300 秒）重试")
            } else {
                None
            }
        }
        "git_status" | "git_diff" | "git_commit" | "git_log" | "git_restore" | "git_branch" | "git_blame" | "git_tag" => {
            if has_any(&["not a git repository", "不是 git", "git repository"]) {
                Some("当前目录不是 git 仓库：可在项目根目录执行 git init 初始化后再操作")
            } else if has_any(&["identity", "user.name", "user.email"]) {
                Some("git 未配置身份：需要用户在本机执行 git config --global user.name / user.email 后重试")
            } else if has_any(&["nothing to commit", "没有要提交"]) {
                Some("工作区没有改动：先用 write_file/edit_file 修改代码，或确认改动已在其他分支")
            } else {
                None
            }
        }
        "run_tests" => {
            if has_any(&["hvigorw", "not found", "找不到"]) {
                Some("工程缺少 hvigorw 脚本或模块名错误：确认是 HarmonyOS 工程且 module 参数正确")
            } else if has_any(&["failed", "失败"]) && has_any(&["test", "测试"]) {
                Some("存在失败用例：阅读日志定位失败测试与断言，修复后重跑验证")
            } else {
                None
            }
        }
        "read_logcat" => {
            if has_any(&["no devices", "not found", "没有设备", "targets"]) {
                Some("没有可用设备或 hdc 不可用：请连接真机/启动模拟器，或检查 hdc 是否在 PATH")
            } else if has_any(&["-t", "-T", "invalid", "usage"]) {
                Some("hdc logcat 参数不受支持：可改用 read_logcat 缺省参数（不传 filter）重试")
            } else {
                None
            }
        }
        "web_fetch" => {
            if has_any(&["network", "timeout", "超时", "连接", "proxy", "ssl", "certificate"]) {
                Some("网络问题：检查网络/系统代理是否可用，或换用 web_search 获取摘要")
            } else if has_any(&["403", "forbidden", "blocked", "拒绝"]) {
                Some("目标站点拒绝抓取：尝试其他来源页面或改用 web_search")
            } else if has_any(&["2mb", "超过"])
                || l.contains("页面超过")
            {
                Some("页面过大：换用 web_search 获取摘要，或抓取更具体的子页面")
            } else {
                None
            }
        }
        "take_screenshot" | "delete_file" => {
            if has_any(&["not found", "不存在", "no such file"]) {
                Some("路径不存在：先用 list_dir 确认文件/目录路径后重试")
            } else if has_any(&["outside", "超出", "项目目录"]) {
                Some("路径超出项目范围：仅可操作当前工程内的路径")
            } else if has_any(&["受保护", "不允许"]) {
                Some("目标受保护：版本库/依赖/产物/IDE 配置目录不允许通过工具删除")
            } else {
                None
            }
        }
        "web_search" => {
            if has_any(&["network", "timeout", "超时", "连接", "proxy", "ssl"]) {
                Some("网络问题：检查网络/系统代理，或稍后重试")
            } else if has_any(&["未搜索到", "结果"]) {
                Some("未找到结果：更换更具体/更通用的搜索词，或改用 web_fetch 直接抓取已知页面")
            } else {
                None
            }
        }
        "save_memory" => {
            if has_any(&["未绑定", "项目目录"]) {
                Some("当前会话未绑定项目：在项目内开启会话后使用")
            } else if l.contains("title 过长") {
                Some("title 过长：精简到 60 字符内")
            } else if l.contains("content 过长") {
                Some("content 过长：精简到 2000 字符内")
            } else {
                None
            }
        }
        "git_stash" => {
            if has_any(&["not a git repository", "不是 git"]) {
                Some("当前目录不是 git 仓库：在项目根目录初始化 git 后重试")
            } else if has_any(&["no local changes", "没有本地", "nothing"])
                || l.contains("没有要保存的改动")
            {
                Some("工作区没有可 stash 的改动：先修改文件再执行")
            } else {
                None
            }
        }
        "get_build_log" => {
            if has_any(&["暂无构建日志", "无构建日志", "读取日志失败"]) {
                Some("暂无构建日志：先执行 build_project 生成日志，或确认 name 参数正确")
            } else {
                None
            }
        }
        "search_symbols" => {
            if has_any(&["未绑定", "项目目录"]) {
                Some("当前会话未绑定项目：在项目内开启会话后使用")
            } else {
                None
            }
        }
        "check_code" | "deep_scan" | "codebase_search" | "get_symbol_details" => {
            if has_any(&["未绑定", "项目目录"]) {
                Some("当前会话未绑定项目：在项目内开启会话后使用")
            } else if has_any(&["扫描目录不存在", "不存在"]) {
                Some("扫描路径不存在：先用 list_dir 确认目录结构，或改用项目根扫描")
            } else {
                None
            }
        }
        "copy_file" => {
            if has_any(&["目标已存在", "覆盖"]) {
                Some("目标路径已存在：换个目标路径，或先 delete_file 清理旧文件")
            } else if has_any(&["受保护", "拒绝"]) {
                Some("路径受保护：版本库/依赖/产物/IDE 配置目录不允许复制")
            } else {
                None
            }
        }
        "get_file_info" => {
            if has_any(&["无法读取", "不存在"]) {
                Some("文件不存在：先用 list_dir/find_files 确认路径后重试")
            } else {
                None
            }
        }
        "http_request" => {
            if has_any(&["超时"]) {
                Some("请求超时：调大 timeout_secs 重试，或确认目标服务可达（本地服务是否已启动）")
            } else if has_any(&["连接", "拒绝", "失败"]) {
                Some("请求失败：检查 URL 是否正确、服务是否已启动（已自动读取系统代理）")
            } else {
                None
            }
        }
        "multi_edit" => {
            if has_any(&["未找到"]) {
                Some("替换原文未找到：先 read_file 确认目标文件当前内容（注意缩进/引号/空白完全一致）")
            } else if has_any(&["冲突", "修改"]) {
                Some("编辑冲突：文件被外部修改，先 read_file 解除冲突保护再重试")
            } else {
                None
            }
        }
        "device_perf" => {
            if has_any(&["设备"]) {
                Some("性能采样失败：确认设备已连接（list_devices），或该设备不支持读取对应指标")
            } else {
                None
            }
        }
        "run_ui_flow" | "run_perf_benchmark" | "dump_ui_hierarchy" => {
            if has_any(&["uitest", "注入失败", "no such file", "not found", "未检测到"]) {
                Some("设备不支持 uitest 命令注入：确认设备已解锁亮屏、开发者模式与 USB 调试已开启；部分精简设备/模拟器可能未内置 uitest，可换真机重试")
            } else {
                None
            }
        }
        "write_unit_tests" => {
            if has_any(&["module.json5", "模块根"]) {
                Some("未定位到鸿蒙模块：确认 path 指向模块内的源码文件（如 entry/src/main/ets/...）")
            } else {
                None
            }
        }
        "start_ability" | "clear_app_data" | "uninstall_app" | "get_installed_apps" | "get_app_info" | "grant_permission" => {
            if has_any(&["bm clean", "bm uninstall", "bm dump", "bm grant", "not found", "no such file"]) {
                Some("应用包名错误或未安装：先用 get_installed_apps 确认包名，或用 deploy 重新安装")
            } else {
                None
            }
        }
        "set_wifi_state" | "set_airplane_mode" => {
            if has_any(&["permission", "权限", "denied", "not found"]) {
                Some("设备不支持通过 hdc 直接切换网络状态：需要 root 版本或特定系统能力；可手动在设备上操作")
            } else {
                None
            }
        }
        "screen_record" => {
            if has_any(&["no such file", "not found", "permission"]) {
                Some("录屏失败：确认设备亮屏解锁，或设备不支持 screenrecord 命令，可用截图多次替代")
            } else {
                None
            }
        }
        "dump_memory" => {
            if has_any(&["permission", "denied", "权限"]) {
                Some("读取内存信息需要 root 权限或设备是 userdebug 版本：user 版本下可先用 collect_perf 获取基础指标")
            } else {
                None
            }
        }
        "set_network_condition" => {
            if has_any(&["permission", "denied", "权限", "not found", "no such file"]) {
                Some("网络条件模拟需要 root 或 userdebug 权限：user 版本设备无法使用 tc 命令，可手动操作或用真机测试")
            } else {
                None
            }
        }
        "record_ui" | "replay_ui" => {
            if has_any(&["uitest", "uiRecord", "not found", "no such file"]) {
                Some("设备不支持 uiRecord 录制：确认设备已解锁亮屏，且系统版本支持 uitest 命令")
            } else {
                None
            }
        }
        "auto_explore" => {
            if has_any(&["uitest", "dumpLayout", "not found", "no such file"]) {
                Some("自动遍历依赖 uitest dumpLayout 能力：确认设备系统支持 uitest 命令（部分精简设备不支持）")
            } else {
                None
            }
        }
        "run_lint" => {
            if has_any(&["未找到可用的 lint", "codelinter", "hvigor"]) {
                Some("环境缺少 Lint 工具：请安装 DevEco Studio 的 Code Linter，或在工程目录下配置 code-linter.json5 并确保 codelinter 在 PATH 中")
            } else {
                None
            }
        }
        "analyze_hap_size" => {
            if has_any(&["未找到 HAP", "构建产物"]) {
                Some("未找到 HAP 产物：请先用 build_project 构建应用，或通过 path 参数显式指定 HAP 文件路径")
            } else {
                None
            }
        }
        _ => None,
    }
}

/// 失败输出追加诊断建议（无匹配模式时保持原文）
pub(crate) fn with_advice(tool: &str, err: String) -> String {
    match diagnose_tool_error(tool, &err) {
        Some(adv) => format!("{err}\n\n【诊断建议】{adv}"),
        None => err,
    }
}

/// 一个可定位的错误条目（文件:行 + 信息）
pub struct ErrorLocation {
    pub file: Option<String>,
    pub line: Option<i64>,
    pub message: String,
}

/// 构建/部署等工具失败时的统一结构化信封。
/// 模型在自动修复循环中可稳定解析 category 决定下一步工具、按 locations 逐个修文件，
/// 比自由文本更不易"看完就忘"或盲目重复构建。
pub fn structured_tool_error(
    tool: &str,
    category: &str,
    summary: &str,
    locations: &[ErrorLocation],
    next_steps: &[&str],
    log_path: Option<&str>,
    raw_tail: &str,
    knowledge: &[crate::services::harmony_knowledge::MatchedEntry],
) -> String {
    let mut s = String::new();
    s.push_str(&format!("【工具失败】{tool}\n"));
    s.push_str(&format!("category: {category}\n"));
    s.push_str(&format!("摘要: {summary}\n"));
    if !locations.is_empty() {
        s.push_str("定位（按此逐一 read_file + edit_file 修复，修完再重新构建）:\n");
        for loc in locations {
            let pos = match (&loc.file, loc.line) {
                (Some(f), Some(l)) => format!("{f}:{l}"),
                (Some(f), None) => f.clone(),
                _ => "未知位置".to_string(),
            };
            s.push_str(&format!("- {pos}: {loc_message}\n", loc_message = loc.message));
        }
    }
    if !knowledge.is_empty() {
        s.push_str("知识库（团队经验，优先参考）:\n");
        for k in knowledge {
            s.push_str(&format!("- {}：{}\n", k.title, k.fix));
        }
    }
    if !next_steps.is_empty() {
        s.push_str("推荐下一步（按顺序）:\n");
        for (i, step) in next_steps.iter().enumerate() {
            s.push_str(&format!("{}. {step}\n", i + 1));
        }
    }
    if let Some(p) = log_path {
        s.push_str(&format!("完整日志: {p}\n"));
    }
    if !raw_tail.trim().is_empty() {
        s.push_str("原始日志尾部:\n");
        s.push_str(raw_tail.trim());
        s.push('\n');
    }
    s
}
