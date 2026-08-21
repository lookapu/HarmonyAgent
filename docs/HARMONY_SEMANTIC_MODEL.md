# HarmonyOS 工程语义模型

`HarmonySemanticModel` 是 HarmonyAgent 理解 HarmonyOS 工程结构的单一真源。构建、部署、Workspace 概览和能力分析不得自行维护另一套模块枚举规则。

## 模型边界

当前 schema 版本为 `3`，统一表示：

| 实体 | 主要来源 | 关键字段 |
| --- | --- | --- |
| 应用 | `AppScope/app.json5` | bundle、版本、标签 |
| 产品 | 根 `build-profile.json5` | SDK 版本、签名配置、参与模块 |
| 模块 | 根模块清单与递归发现的 `module.json5` | 相对路径、类型、设备、target |
| 产物类型 | 模块类型 | entry/feature → HAP，shared/HSP → HSP，HAR → HAR |
| Ability | `abilities` | 名称、入口源码、导出状态 |
| ExtensionAbility | `extensionAbilities` | 名称、扩展类型、入口源码、导出状态 |
| 依赖边 | 根与模块 `oh-package.json5` | 来源模块、scope、约束、工作区目标模块 |
| 锁文件 | 根/模块/targetName `oh-package-*-lock.json5` | lock 版本、specifier、精确包版本、来源、完整性和传递依赖 |
| 清单来源 | 所有上述 JSON5 文件 | 相对路径、Owner、解析状态和错误 |
| 工程关系图 | profile、模块清单、OHPM 与 ArkTS/TS 源码 | 页面、权限、系统能力、模块依赖和真实 import 边 |

模块发现支持最多八层嵌套，并跳过依赖、构建缓存和 IDE 目录。本地 `file:`/`link:` 依赖与包名依赖都会尽可能解析到工作区模块；解析不到时仍保留外部依赖声明。

OHPM 锁文件兼容常见 v1/v3 的 `specifiers` + `packages` 结构，也识别 targetName 专用锁文件。依赖边同时保留声明约束与锁定精确版本，并记录产生该结果的锁文件；无锁、本地未解析或损坏锁文件不会伪造精确版本。损坏清单进入 `manifests` 的 `invalid` 状态与错误字段，其余可解析清单仍继续形成模型。

关系图从以下证据形成可追溯边：

- `main_pages.json`、profile 中的 `routerMap` 以及 `@Entry`/`@Router` 源文件形成模块—页面边；
- `requestPermissions` 形成模块—权限边，并保留 `usedScene` 的 Ability 与时机；
- 源码中的 `SystemCapability.*` 运行时检查形成模块—系统能力边；
- 工作区包名 import 与跨模块相对 import 形成真实引用边，OHPM 本地依赖形成声明依赖边；
- 每条边保存清单路径或源码 `file:line`，Workspace 无需从展示文本反推来源。

## 兼容视图

既有 `HarmonyProject` 仍作为构建和部署的精简接口，但数据全部由语义模型派生：

- entry 优先选择 `type=entry`，再回退名为 `entry` 或首个 HAP 模块；
- API Level 优先读取默认产品的 `compatibleSdkVersion`，再回退其 `compileSdkVersion` 或其它含 SDK 声明的产品；
- 签名只有在产品引用的配置确实存在时才视为已配置；
- HAP 输出目录使用真实 entry 相对路径，支持嵌套模块。

能力分析命令额外返回完整 `semantic_model`，前端可在不重新猜测工程结构的前提下展示产品、模块与依赖关系。

## 验证基线

自动化夹具包含两个产品、四个嵌套模块、HAP/HSP/HAR 三类产物、普通 Ability、ExtensionAbility、权限 usedScene、main pages、router map、SystemCapability 检查、真实跨模块 import、两级本地依赖边、v1 targetName 锁、v3 根锁和损坏清单，并验证旧部署摘要与统一模型一致。

后续 HM-04 在此模型上补齐编译模式、产品差异与更完整的签名配置语义。
