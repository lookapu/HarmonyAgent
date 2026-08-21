# HarmonyOS / OpenHarmony 索引来源证明

HarmonyAgent 在生成或修复 ArkTS 代码前，会组合本机 SDK 声明、HarmonyOS 官方 API 变更与参考数据，以及 OpenHarmony 官方文档镜像。`environment_check` 现在统一报告这些来源的版本、更新时间、条目数和来源覆盖率。

## 来源层级

| 来源 | 版本依据 | 更新时间依据 | 可追溯依据 |
| --- | --- | --- | --- |
| 本机 SDK 声明 | 当前默认 SDK API Level | `.d.ts` 索引建立时间 | `ets/api` 绝对路径 |
| 官方 API 变更索引 | 已入库的 API 版本范围 | 官方页面抓取时间 | 每条记录的 `source_url` |
| 官方 API 参考索引 | `since_api_level` 覆盖范围 | 官方页面抓取时间 | 每个模块的 `source_url` |
| OpenHarmony 文档镜像 | Git HEAD 提交 | 最近 fetch/仓库观测时间 | origin URL、提交和本地 Markdown 文件 |

来源证明只对已有索引做只读对账，不创建另一套平行数据。SDK 仍由文件级增量扫描器维护；官方变更和参考正文仍分别由 `refresh_api_db`、`refresh_api_details` 更新；OpenHarmony 文档仍使用 sparse checkout 镜像。

## 状态语义

- `可信`：存在可用条目，来源和版本可定位，并且更新时间在 30 天内。
- `过期`：证据链完整，但最近更新时间超过 30 天；应先刷新再用于易变事实。
- `不可追溯`：有内容但缺少来源 URL、版本或更新时间；不得作为生成代码的唯一依据。
- `缺失`：本机未安装/下载，或对应知识表尚无条目。

状态是证据门禁，不表示内容本身必然正确。涉及当前工程时，还必须把结果与 product 的 compile/compatible/target API、本机 SDK 声明、LSP、一致性审计和实际构建共同验证。

## 使用方式

调用现有 `environment_check` 即可获得 `[SDK / 官方文档来源证明]` 段落，不新增 Agent 工具。报告会显示每个来源的状态、版本、条目、覆盖率、来源地址和 RFC 3339 更新时间。

建议在以下时机运行：

1. 新建或接手 HarmonyOS 工程时；
2. SDK 或 DevEco Studio 升级后；
3. 生成依赖新 API 的代码前；
4. 官方 API 检索结果与本机 `.d.ts` 冲突时；
5. 长会话恢复后，需要确认原有知识证据仍然有效时。

当报告为过期或缺失时，先使用已有刷新入口更新相应来源。刷新失败时应保留失败事实并降级到本机声明、工程配置和构建证据，不能把模型记忆伪装成官方结论。
