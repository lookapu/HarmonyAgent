# HarmonyOS 工程一致性审计

`check_sdk_alignment` 除了比较工程与本机 SDK 版本，现在还会执行只读的一致性审计，把源码实际 import、本机 SDK 声明、产品 API Level、模块清单和官方参考证据放在同一份报告中。

## 检查范围

### API 与版本

- 扫描各模块 `src/main` 下的 `.ets` / `.ts`，识别 `@ohos.*` 与 `@kit.*` import；
- 精确匹配命名 import 与本机 `.d.ts` 符号，检查引入版本是否高于当前 product 的 compile API；
- 标记本机声明中的 deprecated，并仅在 `@useinstead` 明示时给出替代；
- 本机未配置 SDK 时明确标记该部分已降级，不把所有 import 误报为不存在。

### 权限与能力

- 精确命中的 API 若声明 `@permission`，检查所属模块 `requestPermissions`；
- `usedScene.abilities` 必须指向模块内真实 Ability 或 ExtensionAbility；
- 带 usedScene 的权限缺少 `reason` 时给出配置警告；
- 多设备模块使用带 `@syscap` 的精确 API 时，检查源码是否存在对应 `SystemCapability.*` 守卫；
- 只能定位到模块、无法定位到具体成员时，模块级权限仅作为 `info` 提醒复核，不形成确定性缺失结论。

### 模块与设备

- 模块必须被至少一个 product 纳入；
- HAP 的 `mainElement` 必须匹配已声明 Ability；
- `deviceTypes` 为空时提示目标设备范围不可证明；
- 官方 API 参考库存在 `device_types` 证据时，与模块声明设备类型交叉检查。

## 严重级别

| 级别 | 含义 |
| --- | --- |
| `error` | 有确定来源证据的不一致，如精确 API 权限缺失、API 高于编译 SDK、入口或 usedScene 引用不存在 |
| `warning` | 需要开发者处理或确认的兼容风险，如废弃 API、多设备能力未守卫、设备类型不匹配 |
| `info` | 静态分析无法精确证明的复核项或环境降级，不应据此自动修改工程 |

每条问题都包含稳定 code、模块、源码或配置位置、说明和最多三条证据，便于 Agent 后续读取、修复并重新运行同一检查。

## 安全与边界

审计不会自动增加权限、提高 API Level 或缩小设备范围。源码扫描有目录深度和 2000 文件上限，跳过符号链接；官方参考数据库或 SDK 索引不可用时分别降级，模块清单检查仍继续执行。

## 验收

单元测试覆盖 API 高于 compile SDK、精确权限缺失、多设备 SystemCapability 未守卫、无效 mainElement、无效 permission usedScene 与命名 import 解析。阶段门禁还包括 Rust 全量测试、Worker 恢复、前端测试、lint、生产构建和 diff 检查。
