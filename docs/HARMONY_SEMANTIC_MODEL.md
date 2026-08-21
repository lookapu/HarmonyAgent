# HarmonyOS 工程语义模型

`HarmonySemanticModel` 是 HarmonyAgent 理解 HarmonyOS 工程结构的单一真源。构建、部署、Workspace 概览和能力分析不得自行维护另一套模块枚举规则。

## 模型边界

当前 schema 版本为 `4`，统一表示：

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
| 构建矩阵 | 根/模块 `build-profile.json5` | SDK/API Level、runtime OS、apiType、build mode、签名完整度与产品差异 |

模块发现支持最多八层嵌套，并跳过依赖、构建缓存和 IDE 目录。本地 `file:`/`link:` 依赖与包名依赖都会尽可能解析到工作区模块；解析不到时仍保留外部依赖声明。

OHPM 锁文件兼容常见 v1/v3 的 `specifiers` + `packages` 结构，也识别 targetName 专用锁文件。依赖边同时保留声明约束与锁定精确版本，并记录产生该结果的锁文件；无锁、本地未解析或损坏锁文件不会伪造精确版本。损坏清单进入 `manifests` 的 `invalid` 状态与错误字段，其余可解析清单仍继续形成模型。

关系图从以下证据形成可追溯边：

- `main_pages.json`、profile 中的 `routerMap` 以及 `@Entry`/`@Router` 源文件形成模块—页面边；
- `requestPermissions` 形成模块—权限边，并保留 `usedScene` 的 Ability 与时机；
- 源码中的 `SystemCapability.*` 运行时检查形成模块—系统能力边；
- 工作区包名 import 与跨模块相对 import 形成真实引用边，OHPM 本地依赖形成声明依赖边；
- 每条边保存清单路径或源码 `file:line`，Workspace 无需从展示文本反推来源。

构建矩阵将 `compileSdkVersion`、`compatibleSdkVersion` 和 `targetSdkVersion` 同时保留原文与解析后的 API Level；模块记录 `apiType`、设备类型和可用 build mode。签名模型只记录材料、证书、profile、keystore、alias 是否配置以及签名算法，不读取或返回密码、私钥内容与材料路径。非默认产品会列出相对基线发生变化的 SDK、runtime OS、签名和模块集合字段。

## 兼容视图

既有 `HarmonyProject` 仍作为构建和部署的精简接口，但数据全部由语义模型派生：

- entry 优先选择 `type=entry`，再回退名为 `entry` 或首个 HAP 模块；
- API Level 优先读取默认产品的 `compatibleSdkVersion`，再回退其 `compileSdkVersion` 或其它含 SDK 声明的产品；
- 签名只有在产品引用的配置确实存在时才视为已配置；
- HAP 输出目录使用真实 entry 相对路径，支持嵌套模块。

能力分析命令额外返回完整 `semantic_model`，前端可在不重新猜测工程结构的前提下展示产品、模块与依赖关系。

## 增量更新与影响范围

语义模型按工程根缓存。Agent 的文件写入、编辑、删除、移动、复制和批量编辑成功后，会使用真实变更路径刷新缓存：

- 普通模块文件只重解析所属模块，并重建依赖、锁文件、清单来源和关系图；未受影响模块沿用上一版本的结构化记录；
- 根 `build-profile.json5`、`AppScope/app.json5` 或无法归属模块的结构清单发生变化时，回退全量解析；首次收到变更但尚无缓存基线时同样全量解析，不伪装成增量更新；
- 受影响模块从直接变更模块开始，沿 OHPM 工作区依赖和真实跨模块 import 反向闭包扩展；
- 验证范围同时给出相关模块、产品和建议检查。ArkTS/TS 变化要求 build、lint、test，依赖与 profile 变化额外要求 dependency sync 或 configuration 检查；
- 绝对路径和相对路径统一规范化为工程内相对路径，供审计、Workspace 展示和后续验证复用。

增量结果使用独立的 `HarmonyModelUpdate` 信封，包含 `mode`、`changed_files`、`affected_modules`、`verification` 和更新后的 `model`。语义模型自身字段未变化，因此 schema 仍为 v4。

## Workspace 概览与影响分析

Workspace 的“工程分析”面板直接消费统一模型，并提供以下可追溯视图：

- 产品构建矩阵展示 SDK/API、runtime OS、签名状态、参与模块及相对基线的产品差异；
- 模块视图展示 HAP/HSP/HAR 产物类型、源码根、设备类型和 Ability/ExtensionAbility；
- 配置与源码证据视图逐项展示解析过或损坏的清单，并汇总关系边数量，解析错误不会被隐藏；
- “影响”页接受每行一个工程内文件路径，只预览、不写文件，返回增量或全量验证模式、受影响模块与产品、建议检查和传播证据。

影响传播证据区分直接文件、OHPM 依赖、真实 import 和工程结构四类来源。每个间接模块都会指向它所依赖的已受影响模块，并保留 `oh-package.json5` 或 ArkTS/TS `file:line` 来源；因此用户可以从验证结论回溯到真实配置或源码，而不是依赖黑盒评分。

## 验证基线

自动化夹具包含两个产品、四个嵌套模块、HAP/HSP/HAR 三类产物、普通 Ability、ExtensionAbility、权限 usedScene、main pages、router map、SystemCapability 检查、真实跨模块 import、两级本地依赖边、v1 targetName 锁、v3 根锁和损坏清单，并验证旧部署摘要与统一模型一致。

夹具还会修改嵌套 feature 的 ArkTS import，验证只重解析直接模块、反向标记依赖它的 entry、保留无关模块、更新真实引用图，并覆盖根配置全量回退、绝对路径规范化与缓存失效链路。
