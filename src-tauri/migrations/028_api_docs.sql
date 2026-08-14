-- 鸿蒙 API 全量知识库：从官方各版本 API diff 页面聚合而来。
-- 每行代表某版本中一个 API 声明的变更（新增/删除/废弃/变更）。
-- 聚合全表即可回答"某个 API 在哪个 API level 引入/废弃/变更"。

CREATE TABLE IF NOT EXISTS api_docs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- 归属 Kit，如 "Ability Kit"
    kit TEXT NOT NULL,
    -- 所属 d.ts 文件，如 "@ohos.app.ability.scriptManager.d.ts"
    dts_file TEXT,
    -- 模块名（从 d.ts 推导），如 "@ohos.app.ability.scriptManager"
    module TEXT,
    -- 类/命名空间名，如 "scriptManager"、"SkillInfo"；顶层直接挂 global 时为 "global"
    class_name TEXT,
    -- API 声明文本（声明语句本身），如 "function getSkillInfo(...): Promise<SkillInfo>;"
    declaration TEXT NOT NULL,
    -- API 名称（从声明中提取的函数/属性/枚举值名），便于精准搜索
    api_name TEXT,
    -- 变更类型：added / removed / deprecated / modified / new_kit
    change_type TEXT NOT NULL,
    -- 变更所属版本标签，如 "26.0.0 Beta1"、"6.1.1(24) Release"
    version_label TEXT NOT NULL,
    -- 对应的数字 API level（从版本映射推导），如 26
    api_level INTEGER,
    -- 旧版本声明（删除/变更时有值）
    old_declaration TEXT,
    -- 官方文档页面 URL
    source_url TEXT,
    -- 抓取时间戳
    fetched_at INTEGER NOT NULL,
    -- 唯一键：同版本同 Kit 同文件同类同声明只保留一条
    UNIQUE(version_label, kit, dts_file, class_name, declaration)
);

-- 关键字段建索引，加速搜索与兼容扫描
CREATE INDEX IF NOT EXISTS idx_api_docs_module ON api_docs(module);
CREATE INDEX IF NOT EXISTS idx_api_docs_api_name ON api_docs(api_name);
CREATE INDEX IF NOT EXISTS idx_api_docs_api_level ON api_docs(api_level);
CREATE INDEX IF NOT EXISTS idx_api_docs_kit ON api_docs(kit);
CREATE INDEX IF NOT EXISTS idx_api_docs_change_type ON api_docs(change_type);

-- 元数据表：记录抓取进度
CREATE TABLE IF NOT EXISTS api_docs_meta (
    key TEXT PRIMARY KEY,
    value TEXT
);
