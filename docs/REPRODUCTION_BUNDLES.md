# 问题复现包规范 v1

本文定义 `EC-12` 的问题复现包采集、预览、确认、脱敏、完整性校验和本地存储边界。设置页“问题复现包”和 Agent 工具 `reproduction_bundle` 共用同一服务。

## 1. 目标与非目标

复现包把问题描述、工程环境、可选会话、工具调用、Agent Run 事件和用户指定文本附件收敛为一个可交付 ZIP。它用于让同事或后续评测理解“在什么环境、经过什么步骤、出现了什么结果”，同时满足：

- 默认脱敏，所有文本和 JSON 在进入 ZIP 前处理；
- 先预览精确条目、大小、脱敏状态、遗漏项和 SHA-256，再由用户确认；
- 生成时重新采集，内容与预览摘要不一致就失败关闭；
- ZIP 内 manifest 对每个载荷条目记录大小和 SHA-256，生成后立即自校验；
- 文件只写到当前项目，不自动上传、发送、发布或打开外部应用。

v1 不打包完整源码、构建产物、设备录屏、截图或任意二进制，也不承诺自动执行复现步骤。需要这些材料时，用户应通过受控渠道单独审阅和分享。

## 2. 请求与采集范围

请求结构如下：

```json
{
  "title": "Release build fails",
  "description": "The release product fails after dependency upgrade.",
  "steps": ["Install dependencies", "Run release build"],
  "expected": "A signed HAP is generated.",
  "actual": "Hvigor exits during signing validation.",
  "conversation_id": "optional-conversation-id",
  "run_id": "optional-agent-run-id",
  "include_messages": true,
  "include_tool_runs": true,
  "include_run_events": true,
  "attachments": ["logs/release-build.log"]
}
```

采集上限：

| 内容 | 上限 | ZIP 条目 |
| --- | ---: | --- |
| 问题描述与步骤 | 标题 120 字；步骤 50 条 | `issue.md` |
| 工程环境 | 语义模型中的产品、模块、依赖和清单摘要 | `context/environment.json` |
| 会话消息 | 最近 50 条 | `context/messages.json` |
| 工具调用 | 最近 100 次 | `diagnostics/tool-runs.json` |
| Run 事件 | 最近 200 个 | `diagnostics/run.json` |
| 文本附件 | 20 个；单个读取前上限 1 MiB | `attachments/<相对路径>` |
| 脱敏后载荷 | 单条目 2 MiB；合计 8 MiB；总条目 128 | — |

指定的会话和 Run 必须属于当前项目；同时指定时还必须属于同一上下文。未指定 Run 时使用所选会话最近的 Run。没有会话仍可生成只含问题与工程环境的包，预览会明确提示哪些内容未包含。

## 3. 默认脱敏与附件拒绝

所有 JSON 先使用统一的 `utils::redact::redact_json_value` 按字段递归脱敏，自由文本使用 `redact_text`。复现包随后额外把当前工程绝对根路径替换为 `<PROJECT_ROOT>`，把常见 macOS/Linux/Windows 用户目录替换为 `<HOME>`。

附件必须同时满足：

- 使用项目内相对路径，规范化后的真实文件仍位于项目根内；
- 是普通、可读取的 UTF-8 文本；
- 不超过 1 MiB；
- 不是 `.env*`、`local.properties`、`.npmrc`、`.pypirc`、私钥、证书、keystore、provisioning 或签名目录材料。

不符合条件的附件不会进入包，也不会让整个预览静默失败；它会出现在 `omitted_attachments` 中并说明原因。这样用户能在确认前看见信息缺口。二进制材料不能通过改扩展名绕过，因为内容还必须通过 UTF-8 解码。

结构化应用日志现在也在落盘前调用统一 JSON 脱敏，并在 Unix 上以 `0600` 创建；超长工具产物继续遵循“先脱敏、后落盘”的既有顺序。

## 4. 预览绑定与显式确认

`preview` 读取当前数据库和附件，但不写文件。每个载荷条目返回路径、类型、字节数、SHA-256 和 `redacted` 标志；所有条目的路径、类型与完整字节按稳定顺序计算 `preview_digest`。

`generate` 必须同时满足：

1. 请求携带 `confirmed=true`；
2. Agent 调用取得本次新鲜显式审批，不能用历史白名单跳过；
3. 携带刚才展示给用户的 `preview_digest`；
4. 后端重新采集后的摘要与该值完全一致。

会话新增消息、工具结果变化、Run 事件变化或附件修改都会导致摘要变化，生成操作要求重新预览。UI 的确认对话框显示条目数和脱敏后字节数。

## 5. ZIP、清单与本地记录

ZIP 写入 `.deveco-agent/repro-bundles/`。目录规范化后必须仍在项目根内，防止通过符号链接逃逸；临时文件使用不可覆盖创建，在 Unix 上权限为 `0600`。生成顺序为：

1. 写临时 ZIP；
2. 重新读取 `manifest.json` 和全部载荷，拒绝路径穿越、重复条目、超限内容；
3. 逐项复验字节数和 SHA-256；
4. 原子重命名为最终文件；
5. 计算整个 ZIP 的 SHA-256，写入 `reproduction_bundles` 和统一审计。

`manifest.json` 包含 schema、格式标记、bundle ID、标题、预览摘要、生成器版本、生成时间以及全部载荷清单。数据库仅保存项目内相对路径、摘要、大小和统计，不复制问题正文、消息或附件内容。数据库或审计写入失败时删除刚生成的文件，避免无记录孤儿包。

历史页和 `validate` 会先比较整个 ZIP 与数据库记录的摘要，再执行清单逐项校验。文件被修改、截断、替换或缺少条目都会失败。

## 6. 接口与回滚

- UI：设置 → 问题复现包，提供表单、采集开关、附件列表、预览、确认生成、历史和重新校验。
- Agent：`reproduction_bundle` 支持 `preview | generate | list | validate`。
- IPC：`preview_reproduction_bundle`、`generate_reproduction_bundle`、`list_reproduction_bundles`、`validate_reproduction_bundle`。

迁移 `074_reproduction_bundles.sql` 只新增历史表和索引并可重复执行。回滚应用版本时保留该表不影响旧代码；用户可以直接删除项目内 ZIP，但历史校验会明确报告文件缺失，不会把记录误判为有效。

## 7. 已知边界

- 正则和字段语义脱敏不能证明对任意自然语言秘密达到数学完备；确认界面仍要求用户审阅条目和遗漏项。
- manifest 提供完整性而非发布者身份认证；需要跨组织分发时应由外部可信渠道签名。
- v1 不自动收集设备日志或截图，避免未提示地带出设备标识、通知、账号和屏幕隐私。
- 真实失败样本转成可执行评测场景见 [失败样本回流](FAILURE_REFLOW.md)（`EC-16`），不会在导出时自动回流。
