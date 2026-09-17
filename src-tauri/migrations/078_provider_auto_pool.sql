-- Provider auto 池三态标记：0=不参与，1=仅主对话，2=主对话+杂活
ALTER TABLE providers ADD COLUMN auto_pool_mode INTEGER NOT NULL DEFAULT 0;
