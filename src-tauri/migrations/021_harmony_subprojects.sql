-- 混合工作区：根目录下识别到的鸿蒙子工程路径（JSON 数组，相对项目根，正斜杠）。
-- 用于让工具在一个包含前端/Java/鸿蒙等多类型项目的工作区中，明确知晓哪些子目录是鸿蒙工程。
ALTER TABLE projects ADD COLUMN harmony_subprojects TEXT;
