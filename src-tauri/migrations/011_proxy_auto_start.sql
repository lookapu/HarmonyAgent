-- 011_proxy_auto_start: 本地代理默认随应用启动自动开启
-- 手动点击「启动」后也会自动置 1（start_proxy 成功后持久化）；
-- 本迁移保证新装与已有安装默认即开启自动启动，用户可在代理页取消勾选。
UPDATE proxy_config SET enabled = 1 WHERE id = 1;
