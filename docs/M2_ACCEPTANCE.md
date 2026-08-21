# M2 可靠恢复与分支验收

本报告记录长会话 M2 的可重复自动化验收证据。耗时场景使用持久时间戳前移模拟四小时经过，避免 CI 真实等待数小时；恢复路径、SQLite 状态和 lease fencing 与真实等待使用同一生产代码。

| 验收项 | 自动化证据 | 结果 |
|---|---|---|
| 120 条消息压缩后保持任务状态 | `agent::context::tests::long_session_checkpoint_survives_reopen_and_fact_reconciliation` 重开 SQLite 后核对目标、完成项、待办、阻塞项和来源事实 | 通过 |
| 多小时后从安全检查点恢复 | `agent::scheduler::tests::multi_hour_checkpoint_recovers_once_with_fencing` 模拟四小时失联，验证只回收一次、保留 checkpoint、attempt 递增和旧 lease 拒写 | 通过 |
| 压缩后保持文件、测试、Git 与用户约束 | 同一长会话测试核对失败测试、dirty 文件、Git HEAD 与“暂不提交”固定决策 | 通过 |
| 目标变化停止旧步骤 | `agent::acceptance::tests::recovery_instruction_produces_auditable_contract_diff` 与 `agent::coordinator::tests::replacement_goal_cancels_only_unfinished_inherited_plan` | 通过 |
| 崩溃后不重复副作用 | `agent::recovery::tests::fixture_driven_fault_matrix_stays_safe`、`recovery_gates_side_effects_until_read_evidence_exists`、`agent::tool_runtime::tests::duplicate_side_effect_is_blocked_before_execution` | 通过 |
| 关键结论可追溯 | Context V2 事实、产物和固定项均保存 `source_kind/source_ref/digest`；分支合并清单与 `SubAgentResultV2` 回归测试验证来源化输出 | 通过 |

完整门禁命令：

```bash
cd src-tauri
TAURI_CONFIG='{"build":{"features":[]},"bundle":{"resources":[]}}' cargo test --lib --locked
cd ..
npm test -- --run
npm run lint
npm run build
```

真实桌面进程、设备和网络的长时间 soak 仍属于发布前评测，而不是用来替代上述确定性回归。出现恢复协议、迁移或 lease 语义变更时，必须同时重跑 soak 与本报告中的测试。
