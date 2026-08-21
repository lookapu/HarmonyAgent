use serde::{Deserialize, Serialize};

/// 协议端点（同一厂商可提供 OpenAI / Anthropic / Gemini 多套端点，如 DeepSeek）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointDef {
    pub protocol: String, // openai | anthropic | gemini
    pub base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub provider_type: String,
    pub protocol: String, // openai | anthropic | gemini（主协议）
    pub base_url: String,
    /// 多协议端点（可选；对话按所选协议匹配端点）
    pub endpoints: Vec<EndpointDef>,
    pub api_key: Option<String>,
    pub npm_package: Option<String>,
    pub is_active: bool,
    pub in_failover_queue: bool,
    pub priority: i32,
    pub cost_multiplier: f64,
    pub limit_daily_cny: Option<f64>,
    pub limit_monthly_cny: Option<f64>,
    pub settings_json: String,
    pub notes: Option<String>,
    pub icon: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub provider_id: String,
    pub model_id: String,
    pub display_name: Option<String>,
    pub tool_call: bool,
    pub context_limit: i64,
    pub output_limit: i64,
    pub input_modalities: String,
    pub output_modalities: String,
    pub input_price_per_mtok: f64,
    pub output_price_per_mtok: f64,
    pub is_default: bool,
    pub use_proxy: bool, // 是否走系统代理
    pub enabled: bool,   // 是否启用（禁用后不可在对话中选择）
    pub created_at: i64,
    /// 手动排序序号（默认模型强制置顶后，其余按此升序排列）
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Version {
    pub id: i64,
    pub version: String,
    pub install_path: Option<String>,
    pub is_active: bool,
    pub npm_tag: Option<String>,
    pub installed_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestLog {
    pub id: String,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub total_cost_cny: f64,
    pub latency_ms: Option<i64>,
    pub first_token_ms: Option<i64>,
    pub status_code: Option<i32>,
    pub error_message: Option<String>,
    pub session_id: Option<String>,
    pub is_streaming: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyUsage {
    pub date: String,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub request_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_cost_cny: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub server_type: String,
    pub command: String,
    pub args: String,
    pub env: String,
    pub enabled: bool,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub created_at: i64,
    /// 最近一次连接测试结果（None=尚未测试）
    pub last_test_ok: Option<bool>,
    pub last_test_at: Option<i64>,
    pub last_test_error: Option<String>,
    /// 作用域：NULL=用户级（全局，对所有项目生效）；非空=仅该项目生效
    pub project_id: Option<String>,
}

/// 用户自定义鸿蒙知识条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub id: String,
    /// 关键词，逗号分隔
    pub keywords: String,
    pub title: String,
    pub cause: String,
    pub fix: String,
    pub enabled: bool,
    /// 内置条目不可删除，只能启用/禁用
    pub builtin: bool,
    /// 作用域：NULL=全局；非空=仅该项目生效
    pub project_id: Option<String>,
    /// 被错误匹配命中的累计次数（用于排序：越常用越靠前）
    pub hit_count: i64,
    pub created_at: i64,
    pub updated_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub directory: Option<String>,
    pub repo_owner: Option<String>,
    pub repo_name: Option<String>,
    /// 仓库平台（github / gitee），用于查重与展示
    pub repo_host: Option<String>,
    pub repo_branch: String,
    /// 技能在仓库内的子目录（NULL/空=仓库根）
    pub subdir: Option<String>,
    pub enabled: bool,
    pub content_hash: Option<String>,
    pub installed_at: i64,
    pub updated_at: Option<i64>,
    /// 作用域：NULL=用户级（全局，对所有项目生效）；非空=仅该项目生效
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSummary {
    pub total_requests: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cost_cny: f64,
    pub by_provider: Vec<ProviderCost>,
    pub by_model: Vec<ModelCost>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCost {
    pub provider_id: String,
    pub provider_name: String,
    pub request_count: i64,
    pub total_cost_cny: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCost {
    pub model: String,
    pub request_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_cost_cny: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub kind: String,
    pub trusted: bool,
    pub default_provider_id: Option<String>,
    pub default_model_id: Option<String>,
    pub index_state: String,
    pub rules: Option<String>,
    pub last_opened_at: Option<i64>,
    pub created_at: i64,
    /// 绑定的 worktree 目录（绑定后 Agent 任务在该目录执行）；None=未绑定
    pub worktree_path: Option<String>,
    /// 工作区下识别到的鸿蒙子工程相对路径列表（JSON 数组字符串，正斜杠）；空表示无/未扫描
    pub harmony_subprojects: Option<String>,
    /// 工作区下识别到的各类子工程模块（JSON 数组，WorkspaceModule）；支持 Vue/Java/Go/HarmonyOS 等
    pub workspace_modules: Option<String>,
    /// 会话"鸿蒙主工程"：混合工作区中实际进行鸿蒙开发的子工程（相对项目根路径或绝对路径）；空=用项目根本身
    pub harmony_project_path: Option<String>,
    /// 该项目的会话数量（含归档；list_projects 子查询填充，其余场景为 0）
    pub conversation_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub system_prompt_version: Option<i64>,
    pub is_pinned: bool, // 置顶（排序优先）
    pub archived: bool,  // 归档（默认列表隐藏）
    /// 标签：逗号分隔字符串（如 "bug,refactor,urgent"），空串=无标签
    #[serde(default)]
    pub tags: String,
    /// 工作模式：'local'（项目主仓库）| 'worktree'（绑定的 worktree 目录）
    #[serde(default = "default_work_mode")]
    pub work_mode: String,
    /// worktree 模式时的 worktree 绝对路径（本地模式为 None）
    pub worktree_path: Option<String>,
    /// worktree 模式时的分支名（列表徽标展示；worktree 被删后仍保留历史分支名）
    pub worktree_branch: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

fn default_work_mode() -> String {
    "local".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub conversation_id: String,
    pub role: String,
    pub content: String,
    pub references_json: Option<String>,
    pub model: Option<String>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub created_at: i64,
    /// AI 思考过程（DeepSeek 推理模型等，无则 NULL）
    pub reasoning: Option<String>,
    /// 排队状态：1=流式运行中提交、尚未提交给模型（0=正常）
    pub queued: i64,
    /// 挂起类型：1=发送到 Agent（任务内安全点并入）；0=普通排队（任务结束后自动续跑）
    pub agent_owned: i64,
    /// 本次任务修改过的文件列表 JSON（edit_file/write_file 目标，assistant 消息专用）
    pub modified_files_json: Option<String>,
    /// 本次任务耗时（ms，assistant 消息专用；前端在回复上方展示用时）
    pub duration_ms: Option<i64>,
}

/// 消息反馈（点赞/点踩 + 原因）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageFeedback {
    pub id: String,
    pub message_id: String,
    pub conversation_id: String,
    /// like | dislike
    pub feedback: String,
    pub reason: Option<String>,
    pub comment: Option<String>,
    pub created_at: i64,
}

/// 回复版本（重新生成时旧回复移入；同一用户消息可有多版）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageVersion {
    pub id: String,
    pub conversation_id: String,
    pub user_message_id: String,
    pub content: String,
    pub reasoning: Option<String>,
    pub model: Option<String>,
    pub created_at: i64,
}

/// 添加项目前的目录探测结果（信任对话框展示）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInspect {
    pub path: String,
    pub name: String,
    pub is_harmony: bool,
    pub file_count: i64,
    pub has_git: bool,
    pub already_added: bool,
    pub app_name: Option<String>,
    pub bundle_name: Option<String>,
}

/// 项目长期记忆（用户手动维护，注入 system_prompt）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMemory {
    pub id: String,
    pub project_id: String,
    /// general|architecture|build_command|module_role|user_preference|decision|...
    pub category: String,
    pub title: String,
    pub content: String,
    pub enabled: bool,
    pub source_kind: String,
    pub source_ref: String,
    pub scope: String,
    pub confidence: f64,
    pub version: i64,
    pub confirmed: bool,
    pub pinned: bool,
    pub invalidation_condition: String,
    pub invalidated_at: Option<i64>,
    pub invalidation_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 工具调用统计（Evaluation：按工具聚合）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStat {
    pub tool_name: String,
    /// 总调用次数
    pub call_count: i64,
    /// 成功次数（status = 'ok'）
    pub success_count: i64,
    /// 失败次数（status IN ('error','cancelled')）
    pub fail_count: i64,
    /// 平均耗时（毫秒，duration_ms 非空时）
    pub avg_duration_ms: Option<i64>,
    /// 最近一次调用时间
    pub last_called_at: Option<i64>,
}

/// Skill 调用统计（use_skill 工具按技能聚合：次数 / 最近调用时间）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillUsageStat {
    pub skill_id: String,
    pub skill_name: String,
    /// 总调用次数
    pub call_count: i64,
    /// 最近一次调用时间（unix 秒）
    pub last_called_at: Option<i64>,
}

/// Skill 调用明细（时间线：一次 use_skill 调用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillUsageEvent {
    pub id: String,
    pub skill_id: String,
    pub skill_name: String,
    /// 触发会话标题（会话已删除时为空）
    pub conversation_title: String,
    /// 归属项目（'' = 无项目会话）
    pub project_id: String,
    pub created_at: i64,
}

/// 模型 token 消耗统计（request_logs 按模型聚合，工具统计增强 token 维度）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelTokenStat {
    pub model: String,
    /// 请求次数
    pub request_count: i64,
    /// 输入 token 总量
    pub input_tokens: i64,
    /// 输出 token 总量
    pub output_tokens: i64,
    /// 缓存 token 总量（读 + 写）
    pub cache_tokens: i64,
    /// 总费用（元）
    pub total_cost_cny: f64,
    /// 平均耗时（毫秒，非流式请求时）
    pub avg_latency_ms: Option<i64>,
}

/// 工具 token 消耗统计（[69]：request_logs.tool_name 按工具聚合，代理链路口径）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolTokenStat {
    pub tool_name: String,
    /// 带标注的 LLM 请求次数
    pub request_count: i64,
    /// 输入 token 总量
    pub input_tokens: i64,
    /// 输出 token 总量
    pub output_tokens: i64,
    /// 总费用（元）
    pub total_cost_cny: f64,
}

/// MCP 服务器使用统计（tool_runs 按 mcp__服务器名__工具 聚合到服务器维度）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpUsageStat {
    /// 服务器名（不含 #n 实例后缀）
    pub server_name: String,
    /// 总调用次数
    pub call_count: i64,
    /// 成功次数（status = 'ok'）
    pub success_count: i64,
    /// 失败次数（status IN ('error','cancelled')）
    pub fail_count: i64,
    /// 平均耗时（毫秒，duration_ms 非空时）
    pub avg_duration_ms: Option<i64>,
    /// 最近一次调用时间
    pub last_called_at: Option<i64>,
    /// 该服务器下各工具明细（按调用次数降序）
    pub tools: Vec<McpToolUsage>,
}

/// MCP 单个工具的使用明细
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolUsage {
    /// 工具名（不含 mcp__服务器名__ 前缀）
    pub tool_name: String,
    pub call_count: i64,
    pub success_count: i64,
    pub fail_count: i64,
    pub avg_duration_ms: Option<i64>,
    pub last_called_at: Option<i64>,
}

/* ============ 任务级 Trace（010） ============ */

/// 单次 Agent 任务执行轨迹（stream_chat 每次调用记录一条）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRun {
    pub id: String,
    pub conversation_id: String,
    pub project_id: String,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    /// success | incomplete | error | cancelled
    pub status: String,
    /// 错误分类（errors::ErrorKind::as_str）
    pub error_kind: Option<String>,
    pub error_message: Option<String>,
    pub tool_rounds: i64,
    pub retry_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_cny: f64,
    pub duration_ms: i64,
    pub started_at: i64,
    pub finished_at: i64,
}

/// 任务指标聚合（成功率 / P50 / P95 / 成本 / 错误分布）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStats {
    pub total_tasks: i64,
    pub success_count: i64,
    pub error_count: i64,
    pub cancelled_count: i64,
    /// 成功率 0~1（成功 / 非取消任务）
    pub success_rate: f64,
    /// 耗时 P50（毫秒）
    pub p50_ms: Option<i64>,
    /// 耗时 P95（毫秒）
    pub p95_ms: Option<i64>,
    pub avg_duration_ms: Option<i64>,
    pub total_cost_cny: f64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    /// 错误分类分布（kind -> 次数，按次数倒序）
    pub by_error_kind: Vec<ErrorKindCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorKindCount {
    pub kind: String,
    pub count: i64,
}
