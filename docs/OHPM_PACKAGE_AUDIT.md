# ohpm 包选择与审计

HarmonyAgent 的 ohpm 能力分为离线发现与在线审计两层：

- `ohpm_recommend` 使用官方 landscape 本地缓存发现候选包，按分类、下载量、点赞、流行度或发布时间排序。
- `ohpm_search` 读取官方 registry 的单包元数据，对候选包做采用前审计。

候选推荐不能替代包审计。缓存中的版本、许可证和热度可能滞后；安装前应对精确包名调用 `ohpm_search`。

## 版本比较

`ohpm_search` 接受可选 `version`。未显式指定时，如果当前工程已声明该依赖且锁文件中存在精确版本，工具自动使用锁定版本与 registry `dist-tags.latest` 比较，并返回：

- 选定版本和 latest；
- 当前、落后或异常高于 latest 的关系；
- 最近 12 个版本（`detail=true`）；
- 工程声明约束、锁定版本、依赖作用域和来源模块。

工具只比较 registry 中真实存在的精确版本。不存在的版本不会被静默替换为 latest。

## HarmonyOS 兼容性

包元数据如果声明 `compatibleSdkVersion`、`compatibleSdk`、`apiVersion`、`apiLevel` 或 HarmonyOS/OpenHarmony engine，工具会保留原始声明，并尽可能归一为最低 API Level。

比较 API 的优先级为：

1. 调用方显式提供的 `api_level`；
2. 当前绑定工程默认 product 的 `compatibleSdkVersion`；
3. 无工程证据时仅展示包声明，不做兼容结论。

包未声明机器可读兼容范围时，状态必须是“待安装后构建验证”，不能因为包名或描述包含 HarmonyOS 就判定兼容。即使 API Level 判定通过，也仍需运行 `check_sdk_alignment`、lint、测试与 `build_project`。

## 许可证

许可证优先读取选定版本，缺失时回退包级元数据。报告区分：

- 常见宽松许可证：提示保留版权与声明义务；
- GPL/AGPL/LGPL/EUPL/MPL 等 Copyleft：提示核对传播与链接义务；
- proprietary/unlicensed：高风险；
- 缺失或未知标识：不得推定可商用，需审阅许可证正文。

该分类是工程风险提示，不构成法律意见。OHPM 页面同时直接展示缓存中的许可证标识，便于候选阶段筛选。

## 安全边界

审计会检查 registry 元数据可证明的供应链信号：

- tarball 是否提供 `integrity` 或 `shasum`；
- 是否包含 `preinstall`、`install`、`postinstall` 生命周期脚本；
- 是否存在 Git、HTTP、file/link/workspace 等绕过 registry 的依赖；
- 包或选定版本是否标记 deprecated；
- 源码仓库地址和 registry 证据 URL。

ohpm registry 元数据当前不提供可核验的漏洞公告源，因此报告始终明确漏洞状态未知。“没有风险信号”不等于“没有漏洞”。采用三方库前仍需查看源码仓库安全公告和维护状态、锁定精确版本、保存完整性证据，并通过工程构建与测试。

当 registry 网络或结构异常时，工具会降级到本机 `ohpm view`，但降级结果只证明 CLI 可以查询该包，许可证、兼容性和安全性全部保持未验证。
