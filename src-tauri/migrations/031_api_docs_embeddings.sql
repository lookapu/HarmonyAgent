-- Migration 031: API 知识库向量索引（embedding 语义检索）
-- 为 api_docs 建立 512 维向量，支持"语义相近"召回（如"获取屏幕宽高"→ windowSize），
-- 与现有 TF 关键词打分融合使用（RRF）。
CREATE TABLE IF NOT EXISTS api_docs_embeddings (
    doc_id      INTEGER PRIMARY KEY REFERENCES api_docs(id) ON DELETE CASCADE,
    model       TEXT NOT NULL,            -- 模型标识（bge-small-zh-v1.5），换模型时全量重建
    vector      BLOB NOT NULL,            -- 512 × f32 LE = 2048 字节
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_emb_model ON api_docs_embeddings(model);
