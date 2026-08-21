# 文件变更验证策略

统一执行循环从成功的 `write_file`、`edit_file`、`delete_file`、`apply_patch`、`multi_edit` 和 `lsp_rename` 参数中提取真实变更文件，并按文件类型生成验证计划。

| 变更类型 | 自动选择的验证 |
| --- | --- |
| ArkTS / ETS | `lsp_format`（建议）→ `run_lint` → `run_tests` → `build_project` → `git_diff` |
| TypeScript / JavaScript / Rust / Java / Kotlin / Python / C/C++ | `lsp_format`（建议）→ `check_code` → `run_tests` → `build_generic` → `git_diff` |
| HarmonyOS `json5` 与工程配置 | `lsp_format`（建议）→ `build_project` → `git_diff` |
| SQL | `run_tests` → `git_diff` |
| Markdown 与其他文档 | `git_diff` |

验证计划作为统一执行循环快照的一部分，每轮重新注入。格式化只消除格式噪声，不计作独立验收；所有标记为必需的步骤完成后仍需核对最终差异。执行失败的写入不进入变更范围，`apply_patch` 则从补丁文件头提取路径。
