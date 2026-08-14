-- 会话"鸿蒙主工程"：混合工作区中实际进行鸿蒙开发的子工程。
-- 值为相对项目根的正斜杠路径（如 apps/HarmonyApp）或绝对路径；空 = 使用项目根本身。
ALTER TABLE projects ADD COLUMN harmony_project_path TEXT;
