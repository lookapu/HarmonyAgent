-- 005_provider_proxy: 模型级代理开关 + Provider 请求协议
-- 注意：本文件发布后不可修改，变更走 006 递增编号（迁移纪律）

-- 模型是否走系统代理（1=走系统代理；用于国内无法直连的模型）
ALTER TABLE models ADD COLUMN use_proxy INTEGER NOT NULL DEFAULT 0;

-- Provider 请求协议：openai(OpenAI 兼容) | anthropic(原生) | gemini(原生)
ALTER TABLE providers ADD COLUMN protocol TEXT NOT NULL DEFAULT 'openai';
