-- 鸿蒙官方 API 参考正文详情：从 harmonyos-references 页面抓取，
-- 与 api_docs（版本 diff）配合，提供每个 API 的描述/参数/返回值/示例/权限。

CREATE TABLE IF NOT EXISTS api_details (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- 页面主体标识：通常是模块名，如 @ohos.batteryInfo
    module TEXT NOT NULL,
    -- 页面 slug，如 js-apis-battery-info
    slug TEXT NOT NULL UNIQUE,
    -- 页面标题（H1）
    title TEXT,
    -- 所属 Kit（从面包屑推导）
    kit TEXT,
    -- 首批 API version（页面里“本模块首批接口从API version X开始支持”）
    since_api_level INTEGER,
    -- 是否已废弃
    deprecated INTEGER NOT NULL DEFAULT 0,
    -- 导入语句（原文，可能多行）
    import_snippet TEXT,
    -- 系统能力 SystemCapability.*
    syscap TEXT,
    -- 权限要求（原文，可能含多段）
    permissions TEXT,
    -- 模型约束（Phone/PC/2in1/Tablet/Wearable 等，逗号分隔）
    device_types TEXT,
    -- 正文纯文本（去掉脚本/样式/导航后的主体内容，截断到 200KB）
    body TEXT,
    -- 示例代码（拼接所有 ``` 代码块）
    examples TEXT,
    -- 该页面抽取的子项（class/interface/enum/method）JSON 数组
    members TEXT,
    source_url TEXT NOT NULL,
    fetched_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_api_details_module ON api_details(module);
CREATE INDEX IF NOT EXISTS idx_api_details_kit ON api_details(kit);
CREATE INDEX IF NOT EXISTS idx_api_details_since ON api_details(since_api_level);

-- 子项详情：class/interface/enum 的方法/属性，供精准语法检索
CREATE TABLE IF NOT EXISTS api_members (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    detail_slug TEXT NOT NULL,
    module TEXT,
    parent_name TEXT,
    member_name TEXT NOT NULL,
    kind TEXT NOT NULL,          -- method/property/enum_value/const/class/interface/enum
    declaration TEXT,
    description TEXT,
    since_api_level INTEGER,
    deprecated INTEGER NOT NULL DEFAULT 0,
    syscap TEXT,
    permission TEXT,
    source_url TEXT,
    UNIQUE(detail_slug, parent_name, member_name, kind)
);

CREATE INDEX IF NOT EXISTS idx_api_members_name ON api_members(member_name);
CREATE INDEX IF NOT EXISTS idx_api_members_module ON api_members(module);
CREATE INDEX IF NOT EXISTS idx_api_members_parent ON api_members(parent_name);
