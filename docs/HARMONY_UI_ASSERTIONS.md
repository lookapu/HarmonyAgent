# HarmonyOS UI 流程与页面断言

`run_ui_flow` 将交互和验收放在同一次设备现场中：先按序执行操作，再导出 UI 树做机器断言，最后按需保存截图。它不再把“某一步已经失败、后续已跳过”的文本包装成成功结果。

## 调用结构

```json
{
  "device": "可选设备 id",
  "steps": [
    { "action": "tap", "x": 540, "y": 1800 },
    { "action": "wait", "ms": 800 }
  ],
  "assertions": [
    { "kind": "text", "value": "首页" },
    { "kind": "type", "value": "Button" },
    { "kind": "id", "value": "fatal_error", "present": false },
    { "kind": "bundle", "value": "com.example.app" }
  ],
  "verify": true
}
```

`kind` 支持：

- `text`：扫描 `text`、`content`、`accessibilityText`，缺省使用包含匹配。
- `type`：扫描控件类型，缺省精确匹配。
- `id`：扫描 `id`、`resourceId`，缺省精确匹配。
- `bundle`：扫描 `bundleName`、`package`、`bundle`，缺省精确匹配。

`present` 缺省为 true；设为 false 可断言错误提示或禁止控件不存在。`exact` 可显式覆盖默认匹配口径。

## 结果语义

- 任一操作失败：停止后续操作，保存可用现场证据，并返回失败。
- 任一页面断言失败：列出逐条结果，保存 UI 树和截图，并返回失败。
- 全部通过：返回操作、断言、UI 树和截图路径，并写入 `harmony.ui_flow.completed`。
- `smoke_test` 原样透传 assertions，因此构建和部署成功但关键页面不符合预期时，冒烟结论为失败。

## 安全与验收

显式设备不能绕过在线、授权和 `ui_automation` 能力门禁。单元测试覆盖存在/不存在、包含/精确匹配与非法断言拒绝；阶段门禁包含全量 Rust、两组 worker 崩溃恢复 E2E、前端测试、ESLint、生产构建与差异检查。
