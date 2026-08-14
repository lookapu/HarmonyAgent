-- Migration 009: Provider multi-protocol endpoints（同一厂商的 OpenAI/Anthropic 等不同端点）
ALTER TABLE providers ADD COLUMN endpoints_json TEXT DEFAULT '[]';
