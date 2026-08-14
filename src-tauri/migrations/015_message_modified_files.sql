-- 任务修改文件列表：assistant 消息关联本次任务修改过的文件（edit_file/write_file 目标），
-- 前端在消息底部以折叠卡片展示（ChatGPT 风格）
ALTER TABLE messages ADD COLUMN modified_files_json TEXT;
