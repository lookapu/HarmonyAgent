-- Migration 035: Skill 仓库平台区分（GitHub/Gitee 均支持，查重与目录按平台隔离）
ALTER TABLE skills ADD COLUMN repo_host TEXT NOT NULL DEFAULT 'github';
ALTER TABLE skill_repos ADD COLUMN host TEXT NOT NULL DEFAULT 'github';
