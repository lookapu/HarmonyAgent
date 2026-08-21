# GitHub / Gitee 鸿蒙工程模式分析

HarmonyAgent 可以对工作区内已经打开或检出的 HarmonyOS/OpenHarmony 仓库运行来源化模式分析。调用现有 `get_project_info`，设置 `patterns=true`；分析子目录时同时传入 `path`。该能力不新增 Agent 工具，也不会自动克隆、执行或修改第三方代码。

## 输入与信任边界

分析只采信三类证据：

1. 统一 `HarmonySemanticModel` 解析出的 product、HAP/HSP/HAR 模块、依赖、页面、Ability、ExtensionAbility、权限和 SystemCapability；
2. ArkTS/TypeScript、配置、测试与 Native 文件中的精确源码命中；
3. `.git` checkout 的 origin、HEAD 分支和提交哈希。

README、仓库描述、Star 数和宣传性文本不作为“已经实现某模式”的证据。没有 origin 或提交时，仓库会标为不可完整追溯；仍可分析本地结构，但不得冒充 GitHub/Gitee 的特定版本结论。

`path` 必须位于当前绑定工作区内，避免借开源分析读取任意系统目录。分析只读文件，跳过符号链接、`.git`、构建目录、依赖缓存和生成物；最多扫描 2500 个相关文件，单文件上限 512 KiB。达到上限时 `truncated=true`，结论明确属于不完整样本。origin 若包含 HTTP userinfo 或非常规 SCP 用户名，凭据部分会在进入报告前删除或替换为 `[redacted]`。

## 可提取模式

当前报告覆盖：

- HAP/HSP/HAR 模块化与依赖方向；
- 多 product 的 API、模块和构建差异；
- 页面清单、router map、Navigation/NavPathStack；
- Ability 与 ExtensionAbility 生命周期入口；
- ohpm/本地模块依赖与锁文件治理；
- ArkUI 状态管理和应用级存储；
- 网络访问与数据持久化层；
- Hypium、单元测试和 ohosTest 组织；
- C/C++、CMake、N-API 等 Native 互操作；
- deviceTypes、SystemCapability、窗口与资源限定的多设备适配。

每个模式都包含稳定 id、置信度、摘要、最多 8 条文件/配置证据、复用步骤、适用条件和风险。报告按模式 id 排序，目录扫描也保持确定性，因此同一 checkout 的结果可重复比较。

## 正确的复用流程

模式报告是候选设计证据，不是复制许可，也不是兼容性证明。采用前必须：

1. 用报告中的 origin 和 commit 固定来源版本；
2. 审阅仓库及相关 ohpm 包许可证，不复制签名、令牌、证书、用户数据或发布配置；
3. 将候选模式映射到目标工程 product、模块边界和 API Level；
4. 用本机 SDK `.d.ts`、官方 API 证据和 `check_sdk_alignment` 校验调用；
5. 对第三方依赖运行 `ohpm_search` 包审计并重新生成目标工程锁文件；
6. 完成 LSP、lint、测试、`build_project`，涉及设备行为时继续真机验证。

模块名、目录结构和代码片段不能脱离上下文机械复制。尤其是 exported Ability、权限、系统能力守卫、Native ABI、数据库迁移与网络安全配置，必须以目标工程事实重新设计。
