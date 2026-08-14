-- 023_tool_approval_whitelist: 项目级工具审批白名单（用户审批弹窗选择"本项目始终允许"后持久化）
-- 主键 (project_id, tool)：同一项目同一工具仅一条记录，跨会话、跨重启生效
CREATE TABLE IF NOT EXISTS tool_approval_whitelist (
    project_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (project_id, tool)
);
