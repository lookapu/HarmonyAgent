# DevEco Switch 工具集增强规划（全面版 · 对齐 tool-enhancement-backlog）

> 参考 `docs/tool-enhancement-backlog.txt`（工具集增强完整清单 v1 终版），对本项目工具能力做全量盘点、逐项映射与分批实施规划。
> 盘点口径（2026-08-16 工作区代码实测）：
> - `TOOL_SPECS`（`src-tauri/src/agent/tools/mod.rs`）：**182 个对外工具声明**（name + desc，全部含「副作用」段）
> - 工具分派表 dispatch：170 个匹配分支（多 9 个为内部标记命令）
> - 状态三档：✅ 已有（能力已实现）｜🟡 部分（有相近能力，缺关键特性）｜❌ 缺失（需新实现）
> - **2026-08-16 第二批实施完成**：9 个新工具（06/07/16/17/19/20/35/70/73）+ 6 项 B 类治理（61/66/69/74/75/76），详见 §11 实施记录
> - **2026-08-20 第四批（八仓库盘点）**：+5 个新工具（memorize / ui_focus / schedule_create / schedule_list / schedule_delete），TOOL_SPECS 达 **198**，详见 §11

---

## 1. 现状总览

| 项 | 数值 |
|---|---|
| 对外工具（TOOL_SPECS） | **198** |
| 工具分派分支（含内部命令） | 170+ |
| 自动跑框架（已有） | Reflexion（agent/reflexion.rs）、cost_guard、LSP 常驻、任务看门狗（utils/task_registry.rs） |
| 关键基础设施（已有） | undo 可逆性（undo.rs + undo_edit 工具）、job 系统（job_list/kill/output）、HealthPage（check_all_health）、tool_stats 统计（list_tool_stats + list_tool_token_stats）、session_events 事件流、secret 钥匙串（secret_store/get/delete）、Reflexion 反思卡片、tool_cache 响应缓存、redact 脱敏、TOOL_GROUP 分组 |
| txt 计划项（A 56 + B 20） | 76 |
| 其中 ✅ 已有 | 39 |
| 其中 🟡 部分 | 17 |
| 其中 ❌ 缺失 | 20 |

---

## 2. 现有工具全景（152 个，按域分组）

> 工具清单以 `TOOL_SPECS` 为准，按实现模块分组（`src-tauri/src/agent/tools/`）。

### 2.1 设备域（device_tools.rs）
`list_devices` / `connect_device` / `manage_hdc` / `list_emulators` / `start_emulator` / `create_emulator` / `device_shell` / `device_file` / `get_installed_apps` / `start_ability` / `stop_app` / `uninstall_app` / `clear_app_data` / `grant_permission` / `set_airplane_mode` / `set_network_condition` / `set_wifi_state` / `device_perf`

### 2.2 工程域（project_tools.rs）
`get_project_info` / `list_modules` / `read_module_config` / `create_harmony_project` / `analyze_generic_project` / `analyze_crash` / `scan_api_compat` / `diff_api_versions` / `check_sdk_alignment` / `diagnose_signing` / `check_signature` / `environment_check`

### 2.3 构建域（build_tools.rs）
`build_project` / `build_generic` / `build_hap` / `build_profile` / `run_lint` / `check_code` / `ohpm_install` / `ohpm_search` / `oh_package` / `run_tests` / `write_unit_tests` / `analyze_hap_size`

### 2.4 文件与编辑域（fs_tools.rs + explore_tools.rs）
`read_file` / `write_file` / `edit_file` / `multi_edit` / `delete_file` / `copy_file` / `move_file` / `undo_edit` / `preview_edit` / `list_dir` / `glob` / `grep_files` / `find_files` / `get_file_info` / `type_or_syntax` / `codebase_search` / `deep_scan` / `review_changes`

### 2.5 命令域（cmd_tools.rs）
`run_command` / `check_code` / `http_request` / `job_list` / `job_output` / `job_kill` / `job_template`

### 2.6 Git 域（git_tools.rs）
`git_status` / `git_commit` / `git_push` / `git_pull` / `git_fetch` / `git_branch` / `git_merge` / `git_stash` / `git_tag` / `git_log` / `git_diff` / `git_blame` / `git_restore` / `git_show`（分支策略：agent/* 前缀）

### 2.7 LSP 域（debug_tools.rs）
`lsp_definition` / `lsp_references` / `lsp_rename` / `lsp_format` / `lsp_code_action` / `lsp_completion` / `lsp_signature` / `lsp_hover` / `lsp_diagnostics` / `lsp_symbols` / `search_symbols` / `get_symbol_details`

### 2.8 调试 / 真机域（debug_tools.rs + device_tools.rs）
`debug_probe` / `stack_dump` / `analyze_crash` / `collect_perf` / `run_perf_benchmark` / `dump_memory` / `dump_battery` / `read_logcat` / `read_runtime_logs` / `search_hilog` / `get_diagnostics` / `show_diagnose_card`

### 2.9 UI / 媒体域（ui_tools.rs + media_tools.rs）
`take_screenshot` / `read_clipboard_image` / `view_image` / `verify_ui` / `record_ui` / `replay_ui` / `run_ui_flow` / `dump_ui_hierarchy` / `screen_record` / `image_inspect` / `read_document` / `read_pdf`

### 2.10 记忆 / 知识域（memory_tools.rs）
`save_memory` / `manage_memory` / `search_knowledge` / `manage_knowledge` / `ask_history` / `plan_task` / `todo_get` / `todo_write` / `export_data` / `get_cost_summary`

### 2.11 元能力域（meta_tools.rs）
`tool_list` / `tool_help` / `tool_history` / `ask_user` / `ask_history`

### 2.12 Web / 协作 / 其他
`web_fetch` / `web_search` / `share_session` / `import_session` / `db_query` / `trace_export` / `secret_store` / `secret_get` / `secret_delete` / `list_mcp_servers` / `search_harmony_docs` / `read_harmony_doc` / `read_sdk_api_module` / `search_sdk_api` / `search_api` / `get_api_detail` / `refresh_api_db` / `refresh_api_details` / `list_agents` / `agent_publish` / `agent_subscribe` / `auto_explore` / `run_app` / `deploy` / `deploy_all` / `install_launch` / `get_build_log` / `get_app_info` / `get_env_info` / `get_api_detail` 等

---

## 3. A 类映射（工具补全 56 项）

### A1. 工具元能力 —— 3 项（全部 ✅）

| # | 工具 | 状态 | 现状与复用点 |
|---|---|---|---|
| [01] | tool_list | ✅ | `meta_tools::tool_list`（按 TOOL_SPECS 输出，含 desc） |
| [02] | tool_help | ✅ | `meta_tools::tool_help`（按名查参数/副作用/返回） |
| [03] | tool_history | ✅ | `meta_tools::tool_history`（会话内最近调用含结果） |

### A2. 编辑体验 —— 4 项

| # | 工具 | 状态 | 现状与复用点 |
|---|---|---|---|
| [04] | preview_edit | ✅ | `preview_edit`（edit_file 前返回 diff 不落盘） |
| [05] | format_file | ✅ | `lsp_client::format_file`（ArkTS 语言服务格式化：路径/规则/大小写/空格规范，`dry_run` 只返回 diff 不落盘） |
| [06] | snippet_insert | ✅ | `quality_tools::snippet_insert`（snippets 表 + insert/list/get/search/update/delete CRUD，name 唯一、body≤64KB） |
| [07] | code_metrics | ✅ | `quality_tools::code_metrics`（启发式：圈复杂度/注释率/最大嵌套/函数数，Top 文件 + JSON 输出） |

### A3. LSP 能力补完 —— 5 项（全部 ✅，且超配）

| # | 工具 | 状态 | 现状 |
|---|---|---|---|
| [08] | lsp_rename | ✅ | 符号+引用跨文件同步，可 undo_edit 回退 |
| [09] | lsp_format | ✅ | 服务端格式化（tab_size 可调） |
| [10] | lsp_code_action | ✅ | quick fix |
| [11] | lsp_completion | ✅ | trigger 自动补全 |
| [12] | lsp_signature | ✅ | 签名+文档提示 |

> 超配：另有 lsp_definition / lsp_hover / lsp_references / lsp_diagnostics / lsp_symbols，LSP 域已全覆盖。

### A4. 可观测 / 查询 —— 5 项

| # | 工具 | 状态 | 现状与复用点 |
|---|---|---|---|
| [13] | db_query | ✅ | 只读白名单查 SQLite（30+ 表） |
| [14] | log_query | ✅ | `search_hilog`（since/until/priority/keyword）+ `read_logcat` / `read_runtime_logs` |
| [15] | trace_export | ✅ | 已有导出 |
| [16] | metric_export | ✅ | `quality_tools::metric_export`（Prometheus text：tool 调用/耗时/失败 + LLM 请求/Token/费用 + 工具 token 维度） |
| [17] | log_aggregate | ✅ | `quality_tools::log_aggregate`（hilog + runtime + faultlog 三源单次归并，max_lines/since 可调） |

### A5. API 工作流闭环 —— 4 项

| # | 工具 | 状态 | 现状与复用点 |
|---|---|---|---|
| [18] | api_mock | ✅ | `quality_tools::api_mock`（解析 OpenAPI 3 → 提取路由与响应样例（2xx 优先/default 兜底）→ 内置 node 起零依赖 mock 服务，后台任务常驻，返回端口/job_id/curl 示例） |
| [19] | api_test | ✅ | `quality_tools::api_test`（OpenAPI 3 或显式 cases 批量断言：状态码 + 超时 + 自动提取 GET 冒烟） |
| [20] | api_health | ✅ | `quality_tools::api_health`（批量 URL 探测：状态码 + 耗时健康表） |
| [21] | figma_import | ❌ | **缺口**：Figma URL → 组件树 → ArkTS 骨架。依赖 Figma API token，成本高，排第 4 批 |

### A6. 多模态 —— 6 项

| # | 工具 | 状态 | 现状与复用点 |
|---|---|---|---|
| [22] | read_pdf | ✅ | read_document（pdf-extract 内存提取） |
| [23] | docx_read | ✅ | read_document 支持 docx/pptx/xlsx/pdf/txt/md/csv |
| [24] | ocr_image | ✅ | `media_tools::ocr_image`（Windows.Media.Ocr 系统引擎：内嵌 C# 源首次调用用 csc.exe 编译为 exe 缓存，纯 ASCII JSON 输出规避代码页乱码；png/jpg/jpeg/bmp，无需外置模型） |
| [25] | image_inspect | ✅ | 尺寸/格式/EXIF 元数据 |
| [26] | audio_transcribe | ❌ | **缺口**：语音转文字。依赖 whisper 模型（体积大），排第 4 批 |
| [27] | chart_extract | ✅ | `doc_tools::chart_extract`（视觉模型多模态读图提取图表结构化数据，支持多图批量） |

### A7. 调试 / 真机 —— 5 项

| # | 工具 | 状态 | 现状与复用点 |
|---|---|---|---|
| [28] | attach_debugger | 🟡 | 已有 debug_probe/stack_dump/analyze_crash（崩溃栈分析闭环）。**缺口**：hdc 交互断点 attach；依赖 HarmonyOS 调试协议，排第 3 批 |
| [29] | step_debug | ❌ | **缺口**：单步/继续。依赖 [28]，排第 3 批 |
| [30] | memory_snapshot | ✅ | `dump_memory`（内存快照，泄漏定位） |
| [31] | screenshot_diff | ✅ | `ui_tools::screenshot_diff`（逐像素对比 PNG：差异率/包围盒/位置提示，本地只读不连设备） |
| [32] | flaky_test_detect | ✅ | 重复执行测试 N 次（2-5，默认 3）对比各轮结果，识别不稳定（flaky）用例（复用 run_tests） |

### A8. 构建 / 部署 —— 5 项

| # | 工具 | 状态 | 现状与复用点 |
|---|---|---|---|
| [33] | bundle_analyzer | ✅ | `analyze_hap_size`（HAP 内容/资源大小） |
| [34] | size_diff | ✅ | 对比两个 HAP 大小：总量/目录占比变化 + 文件新增/删除/大小变化 Top 清单 |
| [35] | obfuscate | ✅ | `quality_tools::obfuscate`（build-profile.json5 obfuscation 开关读写 status/enable/disable，写前备份到 .deveco-agent/backups/） |
| [36] | ota_pack | ❌ | **缺口**：OTA 升级包。依赖签名/打包流程，排第 3 批 |
| [37] | smoke_test | ✅ | 构建后自动冒烟链：可选 deploy + run_ui_flow 断言 → 冒烟报告（复用 [68] compose） |

### A9. 知识 / 记忆 / 上下文 —— 4 项

| # | 工具 | 状态 | 现状与复用点 |
|---|---|---|---|
| [38] | conversation_search | ✅ | 全库历史对话语义搜索（按消息内容/会话/时间过滤，复用 embedding 服务） |
| [39] | fact_extract | ✅ | 任务收尾时把值得长期记住的事实（约定/偏好/踩坑）沉淀为项目记忆 |
| [40] | prompt_optimize | ✅ | `meta_tools::prompt_optimize`（离线失败模式分析：tool_runs/task_runs 按错误聚合失败样本 + 复用 diagnose_tool_error 输出修复建议；不调 LLM 改写 system prompt） |
| [41] | reflexion_query/pin | ✅ | `meta_tools` 显式查/钉 Reflexion 卡片（query 查失败模式与对策，pin 钉住 1 小时时间窗） |

### A10. 协作 / 导出 —— 5 项

| # | 工具 | 状态 | 现状与复用点 |
|---|---|---|---|
| [42] | share_session | ✅ | 脱敏导出 JSON/Markdown |
| [43] | import_session | ✅ | 导入接续 |
| [44] | export_report | ✅ | Markdown 报告导出 HTML/PDF（内置 node 渲染；PDF 走 Edge/Chrome headless 打印） |
| [45] | feishu_task_sync | ❌ | 外部平台集成，第 4 批 |
| [46] | jira_sync | ❌ | 外部平台集成，第 4 批 |

### A11. 安全 / 合规 —— 4 项

| # | 工具 | 状态 | 现状与复用点 |
|---|---|---|---|
| [47] | secret_scan | ✅ | 密钥泄漏专项扫描：全仓硬编码密钥/密码（复用 check_code 的 hardcoded-secret 规则）+ 敏感文件（.env/local.properties 等） |
| [48] | license_check | ❌ | 依赖解析 oh-package.json + 许可证库，第 4 批 |
| [49] | vuln_scan | ❌ | 依赖漏洞库对接（ohpm audit 若有），第 4 批 |
| [50] | permission_audit | ✅ | 工具使用安全审计：聚合调用统计 + 权限分级（L0/L1/L2）审计报告（使用量/成功率/危险占比） |

### A12. 安全存储 —— 2 项（全部 ✅）

| # | 工具 | 状态 | 现状 |
|---|---|---|---|
| [51] | secret_store | ✅ | 系统钥匙串（keyring 已接入） |
| [52] | secret_get | ✅ | 读取（另有 secret_delete） |

### A13. UI 自动化 —— 2 项

| # | 工具 | 状态 | 现状与复用点 |
|---|---|---|---|
| [53] | ui_locator | ✅ | `ui_locator`（hierarchy 解析 + 属性/文本过滤返回稳定定位元素与推荐坐标，可直接给 run_ui_flow 用） |
| [54] | gesture_perform | ✅ | 单次手势注入：tap/swipe/longPress/doubleTap/text/key 直连设备屏幕（替代 record_ui/replay_ui 录制回放） |

### A14. 数据库 / 状态 —— 2 项

| # | 工具 | 状态 | 现状与复用点 |
|---|---|---|---|
| [55] | db_migrate | ✅ | 数据库迁移管理：status/apply 查看与执行未应用迁移（与启动自动迁移同一清单） |
| [56] | state_snapshot | ✅ | 应用状态快照：关键表（settings/projects/project_memories/knowledge_entries/mcp_servers/providers 等）导出 JSON 备份 |

---

## 4. B 类映射（健壮性 / 治理 20 项）

| # | 能力 | 状态 | 现状与复用点 |
|---|---|---|---|
| [57] | 工具输出脱敏 redact | ✅ | `utils/redact.rs` 正则表（密钥/JWT/邮箱/手机号/身份证/私钥/内网 IP），mod.rs dispatch 统一出口包裹全部工具 |
| [58] | dry-run 模式 | ✅ | fs_tools 写类工具（write_file/edit_file/delete_file/move_file/copy_file 等）全部支持 `dry_run: true`：返回 diff/影响清单不落盘 + git rollback 兜底 |
| [59] | 单工具取消 UI | ✅ | toolRuns.tsx 工具卡片 abort 按钮 → invoke stop_tool（消费式中断标志，强杀进程树，后端 job_kill 就绪） |
| [60] | 副作用标注 lint | ✅ | 测试静态断言：全部 TOOL_SPECS desc 必含「副作用：」与「参数：」段（新增工具缺失即测试失败，2 个用例） |
| [61] | desc 长度规范 | ✅ | 测试断言 `desc_length_within_band`：全部 desc 80-800 字符（too_short/too_long 双断言，编译期保护） |
| [62] | task_group 字段 | ✅ | `TOOL_GROUP`（pub const，182 条全量登记）+ `TASK_GROUPS` + `tool_group()` 查询，tool_list 已支持按组过滤 |
| [63] | timeout_hint + cost_hint | ✅ | `meta_tools.rs` TOOL_META：timeout_hint/retry_policy/cost_hint 稀疏标注（tool_help 展示，未覆盖工具走默认值） |
| [64] | fallback 链 | ✅ | 被 [68] compose 覆盖：组合链中单步失败自动给出修复建议与替代路径（不单独实现 try_with_fallback 宏） |
| [65] | 结构化错误统一 | ✅ | `errors.rs::diagnose_tool_error`（错误模式→建议规则表）+ `with_advice` 统一包装：工具失败输出自动附修复建议（全工具生效） |
| [66] | tools_health() ping | ✅ | 前端启动 5s 自动 ping `tools_health` 命令，关键工具链缺失时顶部横幅（点击跳转 HealthPage） |
| [67] | 工具响应缓存 | ✅ | `services/tool_cache.rs`：仅 L0 只读工具按 (tool, project, args_hash) 缓存，dispatch 出口统一写入 |
| [68] | 组合工具层 | ✅ | `compose` 工具：build_and_deploy / smoke / test_and_report 等预置链按序串行，每步成功/失败摘要，支持自定义链 |
| [69] | 工具统计 | ✅ | list_tool_stats（次数/成功失败/平均耗时/最近调用）+ **list_tool_token_stats（最耗 token 维度**：request_logs.tool_name 按工具聚合，migration 037）+ 前端排行小节 |
| [70] | 工具级 trace | ✅ | session_events（032 迁移）+ `replay_trace` 工具（quality_tools：按 trace_id 回放调用链，未指定时列出最近 10 个任务） |
| [71] | TOOL_SPECS 抽 JSON | ✅ | `export_tools_meta` 导出全量工具声明 JSON（schema/desc/group/level/timeout_hint 等，写 .deveco-agent/tools_meta.json）供外部消费；runtime 加载未做（风险高，保持静态数组） |
| [72] | 快捷键绑定 | ✅ | 窗口内快捷键：Ctrl+Shift+S 截图验证（take_screenshot）/ Ctrl+Shift+R 运行命令（run_command，填提示词并聚焦）；既有 Ctrl+Shift+B/D/N/K 保留 |
| [73] | sandbox 模拟 | ✅ | `quality_tools::sandbox_exec`（危险命令静态分析 + preview/simulate：临时沙箱目录真执行，≤200 文件/50MB，白名单校验） |
| [74] | tools_health 命令 | ✅ | `commands/tools.rs::tools_health`（复用 check_harmony_toolchain 过滤 project_structure，毫秒级，供 [66] 启动 ping） |
| [75] | 按任务分组 UI | ✅ | `list_tool_groups` 命令暴露 TOOL_GROUP + 统计面板按 build/fix/explore/deploy/refactor/test/other 分组折叠 |
| [76] | 工具调用链可视化 | ✅ | TimelinePanel「调用链」视图：tool_call/tool_result 配对建链，实线=顺序执行、虚线=失败重试，节点含耗时/输出展开 |

---

## 5. 缺口统计

| 批次 | ✅ 已有 | 🟡 部分 | ❌ 缺失 | 合计 |
|---|---|---|---|---|
| A 类（工具补全） | 47 | 1 | 8 | 56 |
| B 类（健壮性） | 20 | 0 | 0 | 20 |
| **合计** | **67** | **1** | **8** | **76** |

---

## 6. 实施路线（价值密度 × 实现成本 × 依赖关系）

### 第 1 批：立即价值（P0/P1，3-5 天）—— 已全部完成 ✅

| 优先级 | 项 | 复用资产 | 估时 | 验收标准 |
|---|---|---|---|---|
| P0 | [57] redact 脱敏 | utils/errors.rs 风格 | 0.5 天 | 读含密钥 .env 返回 *** 遮蔽；正常内容零误伤（正则测试集） |
| P1 | [58] dry-run 模式 | preview_edit 雏形 | 1 天 | write_file/edit_file 带 dry_run 返回 diff 不落盘；delete_file 返回影响清单 |
| P1 | [60] 副作用标注 lint | TOOL_SPECS 146/152 | 0.5 天 | 新增工具缺「副作用」段编译期/测试报警 |
| P1 | [62] task_group 字段 | ToolSpec 结构 | 1 天 | 152 工具全部有分组；tool_list 支持按组过滤 |
| P1 | [05] format_file | lsp_format 内核 | 0.5 天 | 独立工具按 ArkTS 风格格式化单文件返回 diff |
| P1 | [31] screenshot_diff | utils/png.rs | 0.5 天 | 两张截图输出像素差异率 + 差异图路径 |
| P1 | [34] size_diff | analyze_hap_size | 0.5 天 | 两次构建输出大小 delta |
| P1 | [53] ui_locator | dump_ui_hierarchy | 0.5 天 | 按 text/属性返回稳定定位信息 |
| P1 | [59] 单工具取消 UI | job_kill 后端 | 0.5 天 | tool_run 卡片 abort 可停单个工具 |

### 第 2 批：1 周（中成本，多数依赖第 1 批）—— 已全部完成 ✅

| 项 | 复用资产 | 说明 |
|---|---|---|
| [06] snippet_insert | TOOL_SPECS + snippets 表 | 自定义片段库 + 插入 |
| [07] code_metrics | lsp_symbols 解析链路 | 圈复杂度/注释率/嵌套深度 |
| [16] metric_export | list_tool_stats SQL | Prometheus text 格式 |
| [17] log_aggregate | search_hilog + read_runtime_logs | 三源归并 |
| [27] chart_extract | 多模态 image_url 链路 | 视觉模型读图出数据 |
| [32] flaky_test_detect | run_tests | N 次执行波动率 |
| [37] smoke_test | run_ui_flow + [68] | 部署后自动冒烟（✅） |
| [38] conversation_search | embedding 服务 | messages 语义检索（✅） |
| [39] fact_extract | Reflexion 收尾钩子 | 自动事实沉淀（✅） |
| [41] reflexion_query/pin | agent/reflexion.rs | 显式查/钉卡片（✅） |
| [44] export_report | Markdown → PDF | 工作报告导出（✅） |
| [47] secret_scan | scanner.rs 规则 | 独立全仓扫描（✅） |
| [50] permission_audit | list_tool_stats + permissions | 审计报告（✅） |
| [54] gesture_perform | record_ui 链路 | 单次手势注入（✅） |
| [55] db_migrate | migrations runner | 手动迁移/回滚（✅） |
| [56] state_snapshot | export_data 思路 | 状态备份/恢复（✅） |
| [63] timeout_hint + cost_hint | ToolSpec（[62] 后） | 元数据字段（✅） |
| [64] fallback 链 | guards.rs | 由 [68] compose 覆盖（✅） |
| [67] 工具响应缓存 | L0 只读工具 | 10-30s 缓存（✅） |
| [68] 组合工具层 | plan_task 模式 | build_and_deploy 等（✅） |
| [69] 工具统计增强 | request_logs | 最耗 token 维度（✅） |

### 第 3 批：1-2 周（重成本 / 高依赖）

| 项 | 依赖 | 说明 |
|---|---|---|
| [28] attach_debugger / [29] step_debug | hdc 调试协议 | 交互调试（依赖外部调试协议，未做） |
| [36] ota_pack | 签名/打包链路 | 发布链（依赖 DevEco OTA 签名流程，未做） |
| [71] runtime 加载 | export_tools_meta（已做导出） | tools_meta.json runtime 加载（风险高，保持静态数组） |

> 其余第 3 批项（[18][19][20][40][61][65][66][70][72][73][74][75][76]）均已实现，见第 11 节实施记录。

### 第 4 批：持续（外部依赖 / 集成）

[21] figma_import（Figma token）、[26] audio_transcribe（whisper 体积）、[45] feishu_task_sync、[46] jira_sync、[48] license_check、[49] vuln_scan（依赖 ohpm audit 生态）

---

## 7. 依赖关系与执行顺序

```
P0 [57] redact ────────────► 全部批次（安全底座，最先做）
P1 [62] task_group ────────► [75] 分组 UI
P1 [60] 副作用 lint ───────► [61] desc 规范
P1 [59] 取消 UI（独立）─────► [70] replay（事件完备性）
[58] dry-run ──────────────► [73] sandbox（预览先行）
[68] 组合层 ───────────────► [37] smoke_test
[28] attach_debugger ──────► [29] step_debug
[71] TOOL_SPECS JSON ──────► 用户自定义工具（远期）
```

---

## 8. 风险与注意事项

| 风险 | 说明 | 缓解 |
|---|---|---|
| redact 误伤 | 正则过严会遮蔽正常代码（如示例密码、测试数据） | 只遮蔽「上下文明确」的模式（等号赋值/引号包裹/文件扩展名 .env 白名单文件全量遮蔽）；测试集覆盖误伤率 |
| [67] 缓存脏读 | 工具响应缓存导致过期结果 | 仅缓存 L0 只读工具；TTL 10-30s；hash 含 project + args |
| [71] 抽 JSON 回归 | 152 个工具声明迁移引入兼容问题 | 保留 Rust 静态数组为 fallback；JSON 加载失败自动回退；A/B 对比 tool_list 输出 |
| [39] 事实抽取噪声 | 自动沉淀低价值记忆 | 模型评分阈值 + 用户确认入口（复用记忆草稿 UI） |
| [40] prompt 优化失控 | 改写劣化 system prompt | 仅对失败会话生成「补丁段」而非整体改写；版本化可回退 |
| [38] embedding 成本 | 全量历史消息建索引耗时 | 增量索引 + 后台任务（复用 job 系统） |

---

## 9. 度量与验证体系

| 维度 | 指标 | 数据来源 |
|---|---|---|
| 工具可靠性 | 失败率 / 平均耗时 / 重试率 | tool_runs（list_tool_stats 已支持） |
| 安全 | 密钥泄露事件数（redact 拦截数） | redact 日志计数 |
| 效率 | 工具调用节省 token（缓存命中率） | tool_cache 命中统计 |
| 质量 | 副作用段覆盖率 100%、task_group 覆盖率 100% | 静态 lint 断言 |
| 满意度 | 工具取消使用率、dry-run 预览率 | session_events |

---

## 10. 建议下一步（按当前缺口优先级）

**[57] redact 脱敏已完成**（dispatch 统一出口，全工具生效）。**第 1~3 批其余项也全部完成**（见第 11 节实施记录）。剩余缺口全部为外部依赖 / 调试协议类：
1. **[28] attach_debugger / [29] step_debug**：hdc 交互断点 attach，依赖 HarmonyOS 调试协议（已有崩溃栈分析闭环 debug_probe/stack_dump/analyze_crash）
2. **[36] ota_pack**：OTA 升级包，依赖 DevEco 签名/打包流程
3. **第 4 批外部集成**：[21] figma_import（Figma token）、[26] audio_transcribe（whisper 体积）、[45] feishu_task_sync、[46] jira_sync、[48] license_check、[49] vuln_scan（ohpm audit 生态）

---

## 11. 实施记录（2026-08-16 第三批）

### 新工具（`src-tauri/src/agent/tools/quality_tools.rs`，9 个）

| # | 工具 | 实现 | 关键点 |
|---|---|---|---|
| [06] | snippet_insert | snippets 表 CRUD | migration 036；name 唯一、body≤64KB |
| [07] | code_metrics | 启发式静态度量 | 圈复杂度/注释率/最大嵌套/函数数，Top 文件 + JSON |
| [16] | metric_export | Prometheus text | 工具调用/耗时/失败 + LLM 请求/Token/费用 + 工具 token |
| [17] | log_aggregate | 三源归并 | hilog + runtime + faultlog 单次调用 |
| [19] | api_test | OpenAPI 批量断言 | spec 文件/内联 + 显式 cases + 自动 GET 冒烟 |
| [20] | api_health | URL 探测 | ≤10 端点状态码 + 耗时健康表 |
| [35] | obfuscate | 混淆开关读写 | build-profile.json5，写前备份 backups/ |
| [70] | replay_trace | 事件回放 | 按 trace_id 1:1 还原调用链；缺省列最近 10 任务 |
| [73] | sandbox_exec | 危险命令干跑 | preview 静态分析 / simulate 临时沙箱真执行（≤200 文件/50MB） |

### B 类治理

| # | 能力 | 实现 |
|---|---|---|
| [61] | desc 长度规范 | 测试 `desc_length_within_band`：80-800 字符双断言 |
| [66]+[74] | tools_health 启动 ping + 横幅 | `commands/tools.rs::tools_health`（复用 check_harmony_toolchain 过滤 project_structure）+ 前端启动 5s 自动 ping，缺失时顶部横幅跳转 HealthPage |
| [69] | 最耗 token 维度 | migration 037（request_logs.tool_name）+ proxy_service 请求头 x-deveco-tool 标注 + list_tool_token_stats 命令 + 统计面板排行小节 |
| [75] | 按 task_group 分组 UI | `list_tool_groups` 命令暴露 TOOL_GROUP + ToolStatsPanel 按 7 组折叠（组头 + 展开/收起） |
| [76] | 调用链 DAG | TimelinePanel「调用链」视图：tool_call/tool_result 配对建链、失败重试虚线、节点耗时/输出展开 |

### 登记与配套

- 9 个新工具全部登记：TOOL_SPECS（含「副作用」段）、TOOL_GROUP（explore/other/test/build）、dispatch 分支、permissions.rs 级别（L0：code_metrics/metric_export/log_aggregate/replay_trace；L1：snippet_insert/obfuscate/api_test/api_health；L2：sandbox_exec）
- 新 migration：036_snippets.sql、037_request_logs_tool.sql

### 第三批（2026-08-16）：剩余缺口补齐

| # | 工具/能力 | 实现 | 关键点 |
|---|---|---|---|
| [18] | api_mock | quality_tools.rs | OpenAPI 3 解析 → 路由/响应样例提取（2xx 优先/default 兜底）→ 内置 node 零依赖 mock 服务（jobs 后台常驻 12h），返回端口/job_id/curl 示例 |
| [24] | ocr_image | media_tools.rs | Windows.Media.Ocr 系统引擎：内嵌 C# 源首次调用 csc.exe 编译为 exe 缓存（%TEMP%\deveco-agent\ocr_v1.exe）；stdout 纯 ASCII（\uXXXX 转义）规避代码页乱码；需系统 OCR 语言包 |
| [40] | prompt_optimize | meta_tools.rs | 离线失败模式分析：tool_runs/task_runs 按错误聚合（days/min_fail/limit），复用 diagnose_tool_error 输出修复建议；不调 LLM 改写 |
| [60] | 副作用 lint | mod.rs tests | 静态断言：全部 TOOL_SPECS desc 必含「副作用：」+「参数：」段（顺带修复 collect_perf/preview_edit 2 处真实违规） |
| [71] | export_tools_meta | meta_tools.rs | 全量工具声明导出 JSON（schema: deveco-agent/tools_meta/v1，含 name/desc/group/level/hint），写 .deveco-agent/tools_meta.json |
| [72] | 快捷键 | Home.tsx + i18n | Ctrl+Shift+S 截图验证 / Ctrl+Shift+R 运行命令：填入提示词并聚焦输入框（既有 B/D/N/K 保留） |

### 剩余缺口（❌ 8 项 + 🟡 1 项）

- **外部依赖（❌）**：[21] figma_import（Figma token）、[26] audio_transcribe（whisper 体积）、[45] feishu_task_sync、[46] jira_sync、[48] license_check、[49] vuln_scan（ohpm audit 生态）
- **调试协议（❌）**：[29] step_debug（依赖 [28]）；[36] ota_pack（依赖 DevEco OTA 签名流程）
- **部分实现（🟡）**：[28] attach_debugger（已有崩溃栈分析闭环，hdc 交互断点依赖调试协议）
- **风险项**：[71] runtime 加载未做（export_tools_meta 导出已实现；动态加载风险高，保留 Rust 静态数组为唯一事实源）

### 第四批（2026-08-20，八仓库盘点落地）

| # | 工具/能力 | 实现 | 关键点 |
|---|---|---|---|
| — | memorize | memory_tools.rs + chat.rs `replay_memories` | 对齐 Qwen-Agent MemoAssistant：从历史消息重放 memorize 调用重建键值状态，每轮作为 system 注入 |
| — | ui_focus | ui_tools.rs + Home.tsx | 对齐 OpenHands canvas_ui_control：Agent 产出后驱动 UI 聚焦（切右面板/开文件预览），L0 权限 |
| — | schedule_create / list / delete | schedule_tools.rs + services/reminders.rs | 对齐 deepseek-harness schedule：after/at/every 三类会话内提醒，30s 轮询派发 + 桌面通知 |

- 新工具全部登记：TOOL_SPECS（含「副作用」段）、dispatch 分支、permissions.rs 级别（memorize L0 / ui_focus L0 / schedule_* L1）
- 新 migration：051_conversation_snapshots.sql、052_reminders_feedback_terms.sql（见 CHANGELOG v2.2）

---

*生成日期：2026-08-20（第四批实施后更新）。源文件：docs/tool-enhancement-backlog.txt（v1 终版）。盘点基于工作区代码实测（TOOL_SPECS=198）。*
