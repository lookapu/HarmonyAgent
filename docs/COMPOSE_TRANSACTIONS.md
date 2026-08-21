# 多步工具事务

`compose` 将多步工具流视为逻辑事务。它不能让 Git、设备或远端服务获得数据库式原子性，但会明确事务边界并保存足够的恢复证据。

自定义步骤支持：

```json
{
  "steps": [
    {
      "tool": "edit_file",
      "args": { "path": "src/main.ets" },
      "fallback": "write_file",
      "fallback_args": { "path": "src/main.ets" },
      "compensate": { "tool": "undo_edit", "args": {} }
    }
  ],
  "transaction": true,
  "rollback_on_error": true,
  "stop_on_error": true
}
```

- 每个成功步骤写入 `compose.checkpoint`，包含事务 ID、下一步骤、完成清单和待补偿栈。
- 主工具失败且声明 `fallback` 时执行降级工具；降级成功属于可见的 degraded success。
- 整体失败且启用回滚时，显式 `compensate` 按成功步骤逆序执行。
- 没有补偿声明的已完成副作用不会被假定已回滚，而是列入人工核验清单。
- 任一步未处理失败时 `compose` 整体返回失败。
- 禁止在步骤或补偿中嵌套 `compose` / `smoke_test`，避免递归事务边界与补偿栈失控。
- `chain`、`steps`、事务控制字段不会透传给子工具；只有真正的链级业务参数（例如 `device`）会合并。
