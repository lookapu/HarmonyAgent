# 本机 HarmonyOS SDK 索引

HarmonyAgent 以本机 `ets/api` 声明文件为权威来源，建立 API 模块、类型符号、权限、SystemCapability 和引入版本索引。远端参考正文与版本 diff 是补充信息，不覆盖本机声明。

## 索引内容

每个 `@ohos.*.d.ts` / `@kit.*.d.ts` 模块记录：

- 模块名、Kit、声明文件绝对路径；
- 最小/最大 `@since` 与模块是否含 `@deprecated`；
- 顶层 namespace、class、interface、enum、function、type 和 const；
- 每个已索引符号的种类、`@since`、`@deprecated`、SystemCapability 与权限；
- 文件内全部 `@syscap` 和 `@permission`，不再只保留第一个能力。

全局反向索引支持从 Kit、SystemCapability、权限或 API Level 定位模块与符号。`search_sdk_api` 同时搜索模块、Kit、类型、能力和权限，并继续用 `read_sdk_api_module` 返回本机完整签名。

## 增量更新

索引缓存保存每个声明文件的长度和纳秒级修改时间。每次查询都会重新枚举 SDK 声明目录：

1. 文件签名未变时复用已解析模块；
2. 新增或变化文件单独重扫；
3. 删除文件立即从模块和全部反向索引移除；
4. SDK 配置路径变化时清空缓存并从新路径重建。

扫描跳过符号链接并支持声明目录的嵌套子目录，避免越出本机 SDK 根。查询报告公开本轮重扫、复用与移除数量，便于判断结果是否已刷新。

## 边界

- 本阶段索引声明文件中可静态识别的类型级符号；精确成员签名仍以 `read_sdk_api_module` 原文为准。
- `@permission`、`@syscap`、`@since` 和 `@deprecated` 依赖 SDK 注释质量；缺失信息保持未知，不从网络资料臆补。
- 当前缓存是进程内增量缓存；本机 SDK 文件本身始终是重建真源，因此无需数据库迁移，也不会产生跨 SDK 版本污染。

## 验收

测试使用临时 SDK 声明验证首次扫描、未变复用、变化重扫、删除失效，以及类型、权限、SystemCapability 与版本反向索引。阶段门禁同时覆盖全量 Rust、Worker 崩溃恢复 E2E、前端测试、ESLint、生产构建和差异检查。
