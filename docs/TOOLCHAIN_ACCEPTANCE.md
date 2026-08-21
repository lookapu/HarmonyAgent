# Agent 工具链阶段验收

阶段二以自动化证据关闭，不以路线图勾选或自然语言声明作为完成依据。

| 验收项 | 自动化证据 | 结论 |
| --- | --- | --- |
| 全部注册工具具有完整元数据和结构化结果 | `every_registered_tool_has_complete_execution_metadata`、`every_registered_tool_emits_complete_v2_shape`、`v2_reads_legacy_records_and_preserves_unknown_fields` | 198 个注册工具共享契约真源；旧 V2 和未知未来字段可读 |
| 高频工具覆盖成功、失败、超时、取消、重试和恢复 | `high_frequency_tools_cover_success_failure_timeout_cancel_retry_and_recovery_protocols`；真实卡死、取消与崩溃测试见 `TOOL_ISOLATION.md` | 12 个高频工具通过统一故障协议矩阵，执行内核另有真实故障注入 |
| 工具暴露数量下降且典型任务仍可完成 | `selection_is_bounded_and_task_specific`、`phase_selection_unlocks_side_effects_only_when_needed`、`bounded_phase_selection_keeps_representative_tasks_acceptable` | 每阶段最多暴露 32 个工具；编译修复、部署和 Git 交付的验收证据仍全部可达 |
| 副作用重复可度量且崩溃时为零 | `duplicate_side_effect_is_blocked_before_execution`、`crashed_tool_worker_leaves_effect_for_verification`、`side_effect_repeat_rate` SLO | 重复成功副作用进入指标；幂等门禁和崩溃恢复阻止盲目重放 |
| 失败结果解释原因、影响、已完成部分和下一步 | 高频故障矩阵与 `ToolResultV2` 完整字段测试 | `error` 给出原因，`impact` 给出状态影响，`modifications/artifacts` 给出完成部分，`recovery/suggestions` 给出下一步 |

## 高频工具集合

阶段门禁覆盖：`read_file`、`list_dir`、`grep_files`、`codebase_search`、`edit_file`、`write_file`、`run_command`、`build_project`、`run_tests`、`git_status`、`git_diff`、`list_devices`。

统一矩阵验证协议行为；具体实现层继续由文件编辑、Shell、构建、Git、设备、后台 Job、Tool Worker crash 和多进程 Worker crash 测试覆盖。两层同时存在，避免只测试展示信封或只测试底层实现。

## 阶段门禁

合入前要求：

1. `cargo test --locked` 全量通过，联网用例只允许显式 ignored。
2. Tool Worker crash 与多进程 Worker crash E2E 通过。
3. `npm test -- --run`、`npm run lint`、`npm run build` 通过；既有 warning 必须保持 0 error，不能新增本阶段 warning。
4. `git diff --check` 通过。
5. 数据库迁移只新增新编号，已发布迁移不可修改。
