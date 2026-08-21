# 鸿蒙生态知识层

HarmonyAgent 将团队经验、三方包兼容审计、常见工具链错误和设备差异统一为带条件与来源的知识记录。目标不是让模型记住更多结论，而是让每条建议能够回答：适用于哪个版本/API/设备、由什么证据验证、什么时候必须降级为未知。

## 工作流

生态知识参与任务时遵循以下顺序：

1. 从目标契约提取包名/版本、product API、设备类型和错误指纹；
2. 读取仍有效的项目/团队经验；
3. 用 `search_knowledge` 检索结构化生态条目，具体包再由 `ohpm_search` 查询官方 registry；
4. 按真实构建/设备结果、本机 SDK、官方来源、回归经验、模型常识的顺序消解冲突；
5. 只生成适配当前工程的最小方案；
6. 通过一致性检查、lint、测试、Hvigor 和必要的真机证据验证；
7. 写操作通过后续状态读取确认，结果进入可恢复 Run。

检索来源不会被隐藏。输出会明确显示验证状态、来源和限制，避免把旧经验或缺失元数据包装成确定事实。

## 鸿蒙上下文指纹

知识检索不能只由“鸿蒙”单个关键词触发。上下文识别应组合工程、代码、日志和对话信号：

- 高权重：真实 `.ets` 文件，以及 `app.json5`、`module.json5`、`build-profile.json5`、Hvigor/OHPM 配置的组合；
- 中权重：`@kit.*` / `@ohos.*` import、ArkUI 装饰器组合、Ability/ExtensionAbility 和 ArkTS/Hvigor 错误格式；
- 低权重：对话中出现鸿蒙术语、单个组件名、生命周期名或孤立代码片段。

识别结果应保留置信度、命中文件/行、Stage/FA/未知模型、API 证据来源和降级原因，并据此选择 Harmony 能力包与验证门禁。注释、README、迁移示例和测试 fixture 中的命中不能单独证明当前工作区技术栈；文件、分支、product 或 SDK 改变后，旧识别事实必须失效并重新计算。

`@kit.*`、`@ohos.*` 或某个装饰器不能硬编码为特定 API 代际结论。具体引入、废弃和替代版本始终回到当前工程 API 与本机 SDK `.d.ts` 验证。

## 记录模型

`EcosystemKnowledgeRecord` 包含：

- 稳定 id、`package_compatibility|common_error|device_difference` 类别和标题；
- 可选包名/精确版本、API 上下界、设备类型和错误指纹；
- 现象、根因、处理步骤与适用条件；
- `regression_verified`、`build_and_regression_verified`、`registry_compatible`、`registry_incompatible` 或 `compatibility_unknown` 等验证状态；
- 来源类型、引用、版本和观测时间；
- 不能由当前证据证明的限制。

内置规则随应用版本发布，来源绑定仓库中的固定回归场景。它们不保存凭据，不声明未经验证的具体设备行为。三方包记录由 `ohpm_search` 的官方 registry 审计即时生成并绑定选定版本、工程 API、来源 URL 和观测时间。

## 查询方式

主动检索示例：

```json
{
  "keyword": "device",
  "device_type": "tablet",
  "error_code": "unauthorized",
  "api_level": 23,
  "limit": 5
}
```

`search_knowledge` 同时返回当前项目/全局团队条目与生态条目。团队经验仍保留作用域与命中统计；生态条目额外展示条件、来源、验证状态和未知边界。

查询具体三方包时使用：

```json
{
  "keyword": "@scope/package",
  "version": "1.2.3",
  "api_level": 23,
  "detail": true
}
```

`ohpm_search` 会比较 latest/选定版本、最低 API、许可证、完整性、安装脚本、外部来源依赖和废弃状态，并生成同模型的包兼容记录。registry 没有 API 或漏洞信息时，状态必须保持 `compatibility_unknown`，后续以安装、构建和设备验证收敛。

## 设备差异边界

设备结论至少区分连接、授权、系统/API Level、ABI、屏幕、SystemCapability、已安装应用身份和本轮执行结果。`deviceTypes` 清单声明不能替代能力探测；单台设备安装冲突不能污染其他设备结论；多设备恢复只复用同一 HAP 哈希的成功事实。

静态知识只能给出门禁与恢复策略，不能证明某个布局、硬件能力或性能指标已在具体设备通过。涉及设备行为时，最终证据仍必须来自对应设备的安装、启动、Hilog/异常、断言和复验结果。
