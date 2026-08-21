# HarmonyAgent 工作流模板规范 v1

本规范定义 EC08 的项目级工作流模板生命周期。模板描述一组有序工具步骤、参数和逐步验收条件；它是经过校验的计划资产，不是脚本，导入、启用或升级都不会自动执行步骤。

## 格式

模板使用 JSON，存放在项目 `.deveco-agent/workflow-templates/<id>.json`：

```json
{
  "schema": 1,
  "id": "harmony-build-check",
  "name": "Harmony build check",
  "version": "1.0.0",
  "harmony_agent_compat": ">=2.0.0,<3.0.0",
  "permissions": ["project.read", "project.write"],
  "enabled": true,
  "steps": [
    {
      "id": "inspect-project",
      "tool": "get_project_info",
      "args": {},
      "acceptance": "识别产品、模块、API Level 与构建入口"
    },
    {
      "id": "build-debug",
      "tool": "build_project",
      "args": { "mode": "debug" },
      "acceptance": "构建成功并生成可复验产物清单"
    }
  ]
}
```

| 字段 | 规则 |
| --- | --- |
| `schema` | 必须为 `1`；未知 schema 拒绝 |
| `id`、步骤 `id` | 1—64 位小写字母、数字、`-`、`_`；步骤内不能重复 |
| `version` | 合法 SemVer，升级版本必须严格高于已安装版本 |
| `harmony_agent_compat` | 支持精确版本、`^x.y.z` 和逗号连接的比较约束 |
| `permissions` | 使用与 Skill manifest v1 相同的权限枚举；必须覆盖每一步工具所需权限 |
| `steps` | 1—64 步；工具必须已注册，`args` 必须是对象，`acceptance` 不能为空 |

模板禁止调用 `workflow_template`，避免递归管理链。工具名、权限和当前 Agent 兼容性在校验、导入、升级以及读取已存模板时都会重新检查；仅凭模板声明不能增加项目、设备、网络、凭据或发布权限。

## 生命周期

`workflow_template` 工具提供六个动作：

1. `validate`：只校验给定模板，不写入项目；
2. `import`：校验后导入一个尚未安装的模板；
3. `list`：列出版本、启用状态、步骤数和权限摘要，损坏模板按无效项暴露；
4. `enable`：把已安装模板标记为可供后续选择，不执行模板；
5. `disable`：停止后续选择该模板，不删除模板或历史版本；
6. `upgrade`：校验同一 `id` 的更高版本，归档旧版本后替换当前版本。

导入和每次升级都必须取得本次显式审批，不能被 allow-all、白名单或历史授权绕过。若升级新增权限，还必须在审阅权限差异后传入 `allow_permission_escalation=true`。该参数只确认此次清单差异，步骤真正执行时仍逐项经过工具契约、项目根限制、审批和恢复门禁。

升级前版本归档到 `.deveco-agent/workflow-templates/history/<id>/<version>.json`。v1 不提供自动回滚动作，避免用一次管理调用覆盖当前状态；恢复时应人工审阅归档内容，并作为一次新的导入或升级重新走校验与审批。

## 鸿蒙上下文与验收设计

工作流应使用多特征交叉证据判断鸿蒙工程，避免只看扩展名或单一关键词：

- Stage 配置：`app.json5`、`module.json5`、Ability 与产品/模块关系；
- ArkTS/ArkUI：`.ets`、装饰器、声明式组件和生命周期；
- SDK/API：`@kit.*` / `@ohos.*`、API Level、权限与 SystemCapability；
- 工具链：Hvigor、OHPM、HDC、构建产物及设备状态。

建议把“识别工程”“检查 API/权限”“构建”“设备验证”拆成独立步骤，并为每一步写可机器复验的 `acceptance`。模板可以表达推荐流程，但项目语义模型、本机 SDK、实际构建和设备证据仍是事实真源。

## 边界

- v1 管理模板资产，不提供自动调度、条件分支、循环或并发执行；
- v1 不从网络仓库下载模板，也不执行模板携带的代码；
- 来源签名、供应链审计、速率限制和故障隔离属于 EC10；
- 模板执行编排、逐步 checkpoint 与恢复应复用现有 Durable Run 和工具契约，不能另建绕过审批的执行通道。
