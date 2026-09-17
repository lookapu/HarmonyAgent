-- 撤销记录泛化为通用宿主能力审批：补 tool 列记录被撤销的工具名。
-- 只加列不重建表：旧二进制仍能按 call_id 读到撤销记录，降级不会绕过已撤销的调用。
ALTER TABLE ota_approval_revocations ADD COLUMN tool TEXT NOT NULL DEFAULT 'ota_pack';
