//! 命令面板（Cmd+K）静态命令注册表：
//! 前端 CommandPalette 打开时调用 `list_palette_commands` 拉取可执行动作清单，
//! 与前端本地动态命令（会话切换 / 模型切换 / Provider 列表）合并后做 fuzzy 搜索。
//!
//! 静态命令按 id 前缀分两类（前端据此执行）：
//! - `nav:<path>` —— 路由跳转（前端 navigate 到 <path>）
//! - `action:<name>` —— 前端动作（前端按 name 映射到本地回调；未知 id 静默忽略，
//!   保证新增后端命令不破坏旧前端）
//!
//! 设计原则：后端只负责"注册哪些动作存在 + 展示文案"，执行全部在前端，
//! 避免命令面板动作与界面状态脱节（会话/模型等动态数据天然在前端）。

use serde::Serialize;

/// 命令面板条目（静态注册表的一项）
#[derive(Debug, Clone, Serialize)]
pub struct PaletteEntry {
    /// 唯一标识：nav:<path> 或 action:<name>
    pub id: String,
    /// 展示标题
    pub title: String,
    /// 副标题（路径/说明）
    pub subtitle: String,
    /// 分组名（前端按组渲染与排序）
    pub group: String,
    /// 图标名（与前端 IconName 对齐）
    pub icon: String,
}

/// 返回全部静态命令（导航 + 动作）。动态命令（会话/模型/Provider）由前端注入。
#[tauri::command]
pub fn list_palette_commands() -> Vec<PaletteEntry> {
    let nav = |path: &str, title: &str, group: &str, icon: &str| PaletteEntry {
        id: format!("nav:{path}"),
        title: title.to_string(),
        subtitle: path.to_string(),
        group: group.to_string(),
        icon: icon.to_string(),
    };
    let action = |name: &str, title: &str, subtitle: &str, group: &str, icon: &str| PaletteEntry {
        id: format!("action:{name}"),
        title: title.to_string(),
        subtitle: subtitle.to_string(),
        group: group.to_string(),
        icon: icon.to_string(),
    };

    let items = vec![
        // ---- 导航：设置/管理页面 ----
        nav("/providers", "服务商管理", "导航", "bolt"),
        nav("/versions", "版本与 SDK", "导航", "package"),
        nav("/config", "全局配置", "导航", "settings"),
        nav("/cost", "成本与预算", "导航", "payments"),
        nav("/proxy", "代理设置", "导航", "proxy"),
        nav("/mcp", "MCP 服务", "导航", "mcp"),
        nav("/skills", "技能管理", "导航", "skill"),
        nav("/skills/stats", "技能调用统计", "导航", "history"),
        nav("/knowledge", "知识库", "导航", "skill"),
        nav("/api-knowledge", "API 知识库", "导航", "package"),
        nav("/health", "运行健康检查", "导航", "health"),
        // ---- 动作：会话/任务操作 ----
        action("new_chat", "新建会话", "开始一个新对话", "会话", "add-circle"),
        action("compact", "压缩当前会话历史", "用经济模型把较早历史总结为摘要", "会话", "archive"),
        action("rollback", "回滚当前任务", "git 硬重置到任务起点（需确认）", "会话", "refresh"),
        action("rules", "编辑指令 Rules", "全局指令 + 项目级规则，注入 system prompt", "会话", "settings"),
        // ---- 动作：斜杠快捷指令（插入预置 prompt 后发送） ----
        action("slash_build", "构建项目", "快速构建：hvigorw assembleHap", "指令", "bolt"),
        action("slash_deploy", "部署到设备", "安装 hap 并拉起应用", "指令", "devices"),
        action("slash_plan", "计划模式", "先出任务计划，确认后执行", "指令", "lightbulb"),
        action("slash_fix", "修复错误", "针对最近构建/运行错误给出修复", "指令", "refresh"),
        action("slash_review", "代码审查", "审查最近改动并给出意见", "指令", "check"),
        // ---- 动作：界面 ----
        action("toggle_theme", "切换主题", "浅色 / 深色", "界面", "spark"),
        // ---- 动作：Agent 工具快捷入口（v2 收尾：189 工具里的高频项）----
        // 调试 / 排查
        action("slash_log_query", "结构化日志查询", "跨 hilog/runtime/faultlog 按时间/级别/关键词/正则过滤", "调试", "search"),
        action("slash_memory_snapshot", "内存快照", "take/list/diff：抓两次对比定位内存泄漏", "调试", "archive"),
        action("slash_attach_debugger", "Attach 调试器", "把 debuggerd attach 到目标进程", "调试", "bolt"),
        action("slash_step_debug", "单步调试", "step/next/continue/interrupt/where", "调试", "chevron-right"),
        // 代码理解
        action("slash_lsp_rename", "LSP 重命名", "重命名符号 + 所有引用", "重构", "edit"),
        action("slash_lsp_format", "LSP 格式化", "按 LSP 服务端格式化当前文件", "重构", "check"),
        action("slash_lsp_code_action", "LSP 快速修复", "自动修导入缺失 / 类型错误", "重构", "lightbulb"),
        action("slash_code_metrics", "代码度量", "圈复杂度 / 行数 / 注释率 / 嵌套深度", "重构", "info"),
        action("slash_format_file", "ArkTS 格式化", "按 ArkTS 风格格式化单个文件（支持 dry_run）", "重构", "check"),
        // 构建 / 部署
        action("slash_obfuscate", "代码混淆", "开启/关闭/查询 build-profile.json5 混淆", "构建", "package"),
        action("slash_smoke_test", "冒烟测试链", "build → deploy → run_ui_flow → 截图", "构建", "check"),
        action("slash_ota_pack", "制作 OTA 包", "基于 HAP 出 .pkg（自动找 packaging_tool）", "部署", "package"),
        // 安全 / 合规
        action("slash_license_check", "依赖许可证检查", "扫 ohpm/Cargo/uv 依赖 allow/deny 黑白名单", "安全", "info"),
        action("slash_vuln_scan", "依赖漏洞扫描", "基于内置漏洞库查 lock 文件", "安全", "search"),
        action("slash_secret_scan", "密钥泄露扫描", "扫代码里的 API key / JWT / 私钥", "安全", "search"),
        action("slash_redact_preview", "输出脱敏预览", "看 redact 规则会如何遮当前内容", "安全", "eye"),
        // 知识 / 协作
        action("slash_conversation_search", "搜索历史对话", "语义搜索过往会话", "知识", "search"),
        action("slash_reflexion_query", "查反思卡片", "看项目有哪些失败教训卡片", "知识", "lightbulb"),
        action("slash_export_report", "导出工作报告", "把任务过程导出 PDF / Markdown", "知识", "download"),
        action("slash_share_session", "分享会话", "导出会话为脱敏 JSON/Markdown", "知识", "send"),
        // 数据 / 状态
        action("slash_db_query", "查项目数据库", "只读查询 SQLite（白名单）", "数据", "search"),
        action("slash_state_snapshot", "状态快照备份", "备份应用状态", "数据", "archive"),
        // 治理
        action("slash_tool_list", "列出全部工具", "动态拉取所有可用工具（按任务分组）", "治理", "list"),
        action("slash_tool_help", "查工具帮助", "tool_help name=<工具名>", "治理", "info"),
        action("slash_tool_history", "查工具调用历史", "查最近 N 次工具调用记录", "治理", "clock"),
        action("slash_tools_health", "工具链体检", "轻量 ping hvigorw/hdc/ohpm 等", "治理", "health"),
        action("slash_sandbox_exec", "临时副本试运行", "复制到临时目录执行；不提供系统级隔离", "治理", "archive"),
        // 多模态
        action("slash_docx_read", "读 Word 文档", "读 .docx 正文", "多模态", "document"),
        action("slash_audio_transcribe", "语音转文字", "调 whisper.cpp 转录音频", "多模态", "send"),
        action("slash_ocr_image", "截图 OCR", "从截图识别文字", "多模态", "eye"),
    ];
    items
}
