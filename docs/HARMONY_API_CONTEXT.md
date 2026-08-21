# 工程绑定的 API 检索

API 检索必须回答“这个工程现在能不能用”，而不只回答“文档里有没有”。`search_sdk_api`、`read_sdk_api_module`、`search_api` 和 `get_api_detail` 因此统一绑定当前工程、产品与本机 SDK。

## 上下文选择

工具优先使用显式 `product`；否则选择 `default` 产品，再回退首个产品。上下文同时展示：

- `compileSdkVersion` 的 API Level：决定本机类型检查/编译能否识别 API；
- `compatibleSdkVersion`：工程声明的最低运行 API，用于判断是否需要运行时守卫；
- `targetSdkVersion`：目标行为版本，作为迁移与行为审查依据；
- 当前配置的本机 SDK API：证明声明来自哪套已安装 SDK。

## 判定规则

| 条件 | 标记 |
| --- | --- |
| 引入版本高于 compile API（或无法解析 compile 时高于本机 SDK） | 不可用：高于当前编译 SDK |
| 引入版本高于 compatible API，但不高于 compile API | 条件可用：必须增加 API Level 运行时守卫 |
| 本机声明或官方参考标记 deprecated | 可用但已废弃 |
| 官方变更库标记 removed | 不可用：已移除 |
| 以上均不命中 | 可用 |

废弃替代只接受本机声明中的 `@useinstead` 或官方变更/参考正文中的明确证据。没有明确替代时，结果要求继续读取声明或官方详情，不会根据名字相似度臆造迁移方案。

## 来源优先级

1. 本机 `.d.ts`：当前编译环境的签名、注释和 `@useinstead`；
2. 当前工程语义模型：产品 API Level；
3. 本地持久化的官方 API diff：引入、修改、废弃、移除与来源 URL；
4. 本地持久化的官方参考正文：权限、SystemCapability、设备类型、成员和示例。

当来源冲突时，工具应同时展示差异；本机声明决定“现在能否编译”，官方记录解释版本历史，二者不能互相静默覆盖。

## 验收

单元测试覆盖高于编译 SDK、需运行时守卫、废弃与正常可用四种判定。全量门禁验证三个检索入口、声明读取、工具 schema、前端与 Worker 恢复无回归。
