-- Migration 046: lan_tokens 持久化令牌明文
-- 需求：令牌未失效前二维码始终可展示（扫码直达 http://ip:port/#token）。
-- 原设计只存哈希，明文仅创建时回传一次，历史令牌无法恢复二维码 URL。
-- 本列存 6 位数字明文（仅本机 sqlite，令牌有有效期、可撤销，撤销立即失效）。
-- 旧令牌（045 之前创建）该列为 NULL，无法恢复二维码，前端提示重新生成。
ALTER TABLE lan_tokens ADD COLUMN token_plain TEXT;
