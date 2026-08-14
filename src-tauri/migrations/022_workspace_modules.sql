-- 工作区模块：根目录作为一个项目，其下识别到的各类型子工程（Vue/Java/Go/HarmonyOS 等）。
-- JSON 数组：[{"rel_path":"frontend","kind":"vue","name":"xxx","manual":false}]
ALTER TABLE projects ADD COLUMN workspace_modules TEXT;
