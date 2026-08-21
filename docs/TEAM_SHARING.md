# 团队共享项目上下文规范 v1

本文定义 `EC-11` 的团队共享包、导入冲突、版本升级、变更历史、撤销和固定评测边界。实现入口是设置页的“团队共享”和 Agent 工具 `team_share`；两者共用同一校验与存储服务。

## 1. 设计目标

- 共享项目记忆、工程约定和评测集，但不把共享内容冒充用户本地事实。
- 每个包绑定来源 URI、精确修订、SemVer 和规范化 SHA-256 摘要。
- 应用前必须预览；Agent 执行 `apply`、`revert` 时必须取得本次显式审批。
- 每次导入形成独立批次和逐项变更记录，能够审计和按批次撤销。
- 评测集只能组合应用内已注册的确定性场景，不能从包内注入脚本或工具调用。

## 2. 包格式

共享包使用严格的 JSON schema v1；未知字段会导致校验失败。

```json
{
  "schema": 1,
  "package_id": "mobile-team",
  "name": "Mobile team conventions",
  "version": "1.0.0",
  "source": {
    "uri": "https://example.com/mobile/context",
    "revision": "git:8a6f51d"
  },
  "memories": [
    {
      "key": "debug-build",
      "category": "build_command",
      "title": "Debug build",
      "content": "hvigorw assembleHap --mode module -p product=default",
      "confidence": 0.9,
      "invalidation_condition": "build-profile changes"
    }
  ],
  "conventions": [
    {
      "key": "feature-modules",
      "category": "architecture",
      "title": "Module boundary",
      "content": "Cross-feature APIs live in shared HAR modules.",
      "confidence": 0.8,
      "invalidation_condition": "architecture decision superseded"
    }
  ],
  "eval_sets": [
    {
      "key": "long-session-recovery",
      "name": "Long-session recovery",
      "cases": [
        {
          "scenario_id": "stream_disconnect_before_delta",
          "expected": "replay_same_request"
        }
      ]
    }
  ]
}
```

约束如下：

- `package_id` 和所有 `key` 是稳定标识；只允许小写字母、数字、点、下划线和连字符。
- `version` 必须是三段 SemVer；同一 `package_id + source.uri` 只能升级到更高版本。
- 同版本内容摘要不可变化。内容需要修正时必须发布更高版本，防止“版本相同、内容不同”。
- `source.uri` 不得携带用户名或密码，`revision` 必须指向发布方可复核的精确修订。
- 记忆与约定合计最多 500 项，评测集最多 100 个，每个评测集最多 100 个场景。
- `conventions` 导入后统一归类为 `architecture`，不信任包内用 category 提升语义。
- 评测项的 `scenario_id` 和 `expected` 必须与本机注册表完全一致。应用升级改变契约后，旧包会校验失败并要求发布兼容版本。

## 3. 预览与冲突规则

`preview` 不写数据库，逐项返回以下动作：

| 动作 | 含义 | 应用结果 |
| --- | --- | --- |
| `insert` | 没有同源项，也未与本地事实冲突 | 新增为已确认、启用的团队项 |
| `update` | 稳定来源相同，但内容发生变化 | 更新同一记录并保存更新前快照 |
| `conflict` | 与非团队来源的本地事实同名或同内容 | 新建禁用且未确认的团队副本，本地事实不变 |
| `unchanged` | 同源内容一致 | 只记录批次历史，不改内容 |

稳定来源由 `source.uri + package_id + item kind + key` 组成，不包含 revision 或版本，因此合法升级会更新同一团队项。不同来源永远不会静默覆盖彼此。

## 4. 应用、历史与撤销

应用成功后，`team_share_imports` 保存包版本、来源、修订、摘要和状态；`team_share_changes` 保存每一项的动作、导入后摘要以及更新前快照。应用和撤销分别写入 `team_share.apply`、`team_share.revert` 审计事件，审计详情只包含项目、包、版本、摘要、冲突数或恢复数，不复制共享正文。

撤销采用保守的 compare-before-restore：

1. 仅能撤销属于当前项目且仍为 `applied` 的批次。
2. 新增项只有在来源仍是 `team_share`、稳定 key 与导入后摘要都未改变时才删除。
3. 更新项满足相同条件时恢复导入前完整快照。
4. 用户接管来源或修改内容后，撤销跳过该项并保留用户版本。
5. 批次无论恢复了多少项都会标记 `reverted`，返回的恢复数量用于识别被保护而跳过的项。

撤销较旧批次不会覆盖较新内容，因为摘要不匹配时会安全跳过。若要回到某一历史版本，应从最新批次开始逆序撤销。

## 5. 导出与评测

导出只包含当前项目中已确认、已启用、未失效的记忆，以及当前启用的团队评测集；调用者必须提供新的包标识、SemVer、来源 URI 和精确修订。导出结果只返回到界面或 Agent 上下文，不会自动写文件或发布到网络。

运行共享评测集是只读操作，逐项调用应用内注册的可靠性场景执行器并返回通过数与证据。共享包不能定义代码、命令、阈值或任意预期值，因此它只是一个可版本化的场景选择清单，不是远程执行载体。

## 6. 操作接口

- UI：设置 → 团队共享。支持粘贴 JSON、预览、确认应用、导出、查看批次与逐项变更、确认撤销、列出并运行评测集。
- Agent：`team_share` 支持 `validate | preview | apply | revert | list | export | run_eval`。
- IPC：`preview_team_share`、`apply_team_share`、`revert_team_share`、`list_team_share_imports`、`list_team_share_changes`、`export_team_share`、`list_team_eval_sets`、`run_team_eval_set`。

迁移 `073_team_sharing.sql` 仅新增表和索引，使用 `IF NOT EXISTS`，可重复执行。回滚应用版本时保留这些表不会影响旧代码；需要业务回滚时应先通过界面按批次撤销，不应直接删除历史表。

## 7. 已知边界

- v1 不提供远程仓库同步、团队身份签名或自动拉取；来源真实性仍需由团队的发布渠道保证。
- 冲突副本需要用户在记忆管理中审阅和确认，本阶段不做自动合并。
- 变更历史存储更新前正文以支持精确恢复；数据库备份和访问控制应按项目源码同等级保护。
- 固定评测当前复用内置可靠性场景。鸿蒙工程、真机和发布门禁的扩展评测由 `EC-13`—`EC-16` 继续建设。
