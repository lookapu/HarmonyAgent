# HarmonyOS 工程语义模型

`HarmonySemanticModel` 是 HarmonyAgent 理解 HarmonyOS 工程结构的单一真源。构建、部署、Workspace 概览和能力分析不得自行维护另一套模块枚举规则。

## 模型边界

当前 schema 版本为 `1`，统一表示：

| 实体 | 主要来源 | 关键字段 |
| --- | --- | --- |
| 应用 | `AppScope/app.json5` | bundle、版本、标签 |
| 产品 | 根 `build-profile.json5` | SDK 版本、签名配置、参与模块 |
| 模块 | 根模块清单与递归发现的 `module.json5` | 相对路径、类型、设备、target |
| 产物类型 | 模块类型 | entry/feature → HAP，shared/HSP → HSP，HAR → HAR |
| Ability | `abilities` | 名称、入口源码、导出状态 |
| ExtensionAbility | `extensionAbilities` | 名称、扩展类型、入口源码、导出状态 |
| 依赖边 | 根与模块 `oh-package.json5` | 来源模块、scope、约束、工作区目标模块 |

模块发现支持最多八层嵌套，并跳过依赖、构建缓存和 IDE 目录。本地 `file:`/`link:` 依赖与包名依赖都会尽可能解析到工作区模块；解析不到时仍保留外部依赖声明。

## 兼容视图

既有 `HarmonyProject` 仍作为构建和部署的精简接口，但数据全部由语义模型派生：

- entry 优先选择 `type=entry`，再回退名为 `entry` 或首个 HAP 模块；
- API Level 优先读取默认产品的 `compatibleSdkVersion`，再回退其 `compileSdkVersion` 或其它含 SDK 声明的产品；
- 签名只有在产品引用的配置确实存在时才视为已配置；
- HAP 输出目录使用真实 entry 相对路径，支持嵌套模块。

能力分析命令额外返回完整 `semantic_model`，前端可在不重新猜测工程结构的前提下展示产品、模块与依赖关系。

## 验证基线

自动化夹具包含两个产品、四个嵌套模块、HAP/HSP/HAR 三类产物、普通 Ability、ExtensionAbility 和两级本地依赖边，并验证旧部署摘要与统一模型一致。

后续 HM-02 在此模型上增加锁文件与逐清单来源信息；HM-03 再扩展路由、页面、权限、系统能力和跨模块引用图。
