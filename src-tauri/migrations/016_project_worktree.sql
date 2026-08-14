-- 项目 worktree 绑定：绑定后 Agent 任务在绑定的 worktree 目录中执行（隔离分支操作），
-- 配合 Git 面板的 worktree 管理（创建/切换/合并回主分支）
ALTER TABLE projects ADD COLUMN worktree_path TEXT;
