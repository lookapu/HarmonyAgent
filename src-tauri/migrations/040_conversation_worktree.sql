-- 040_conversation_worktree: 会话级工作目录（本地 / worktree 模式）
-- 每个会话可绑定到项目主仓库（本地）或某个 worktree 目录（worktree 模式）。
-- work_mode: 'local' | 'worktree'；worktree_path 为该 worktree 的绝对路径；
-- worktree_branch 为分支名（列表徽标展示用，worktree 被删后仍能显示历史分支名）。
-- 与 projects.worktree_path（旧项目级绑定）解耦：agent 工作目录改从会话读取。

ALTER TABLE conversations ADD COLUMN work_mode TEXT NOT NULL DEFAULT 'local';
ALTER TABLE conversations ADD COLUMN worktree_path TEXT;
ALTER TABLE conversations ADD COLUMN worktree_branch TEXT;
