# 待决清单（需你拍板，本轮未动）

> 记录时间：2026-09-30 · 分支 `main` 领先 `origin/main` 51 · 工作区干净 · **两个远端均未 push**
> 本清单只列**我判断不该由我单方面决定**的事项。每条都给了「为什么需要你」和「不改的实际风险」。

---

## A. 结构性重构（涉及面大，需要授权）

### A1. `chat.rs` 补 `impl KernelIoPort` —— 桌面侧 IO 循环并入统一 run-loop

- **现状**：`KernelIoRunLoop::with_started` 在 UI 与 headless 两处都是同一 executor，
  router / governor / 计数 / 终止归因 / 最终快照本来就是同一套。
  **唯一分叉是「谁来驱动 IO 循环」**：headless 走 `KernelIoRunLoop::run`，
  桌面用 `desktop_round` 系列 adapter 自己转。
- **为什么需要你**：这是动 `chat.rs`（全仓最大、最核心文件）核心控制流的重构，
  且 `harmony-agent` bin 在 `eval-cli` feature 之后，**CI 门禁量的是 headless 那条统一路径**，
  桌面侧不在门禁覆盖内——我改它就没有自动化验证兜底。
- **不改的风险**：不阻断任何功能。决策面已统一，IO 驱动分叉是已知且已文档化的状态。

### A2. eval fixture 哨兵校验

- **现状**：评测 fixture 声明的产物没有被真校验。
- **为什么需要你**：改评测口径会影响 CI 基线与历史结果的可比性，属于「定义什么算通过」的问题。
- **不改的风险**：评测可能放过「声明了但没产出」的 fixture，结论偏乐观。

### A3. `declared_validator` 命令匹配仍是整段子串包含

- **现状**：`declared_validator` 判定某命令是否覆盖某测试，用的是整段子串包含。
- **为什么需要你**：收紧它会改变「哪些任务被判定为已验证」——直接影响验收闸门的宽严，
  属于策略而非 bug。
- **不改的风险**：可能误命中（另一条命令恰好包含该子串就算覆盖）。

### A4. `cat test_notes.md` 是否满足 Tests 判据

- **现状**：agent 可以用 `cat` 读一个 md 文件来「满足」测试判据。
- **为什么需要你**：这本质是判据定义问题——`run_tests` 的证据要求应该多严。
  收紧会阻断一些当前能走通的路径。
- **不改的风险**：验收可用「读一个文件」代替真跑测试。

---

## B. 已知但我判定为「不值得改」（记录以免日后当漏项）

| 位置 | 现象 | 不改的理由 |
|---|---|---|
| `fs_tools.rs` `format_undo_diff` | 当前文件读不到时把旧行全标成删除 | 纯展示、无破坏性后果；据此做的 `undo_edit` 是安全方向。显示不够精确属风格级 |
| `services/generic_project.rs` .NET 探测 | `read_dir` 失败即跳过 | **兜底探测**，失败返回带下一步的 `Err`，不构成虚假断言 |
| `services/harmony.rs` `find_latest_hap` | 遍历读不到时可能挑到较旧的 HAP | 只用于 HAP 体积分析（允许显式传路径），失败退化为「未找到 + 可执行建议」；deploy 侧真值门已由指纹完整性收紧 |
| `agent/context.rs` 匹配「测试通过/tests passed」 | 看着像拿模型的话当证据 | 实读确认相反：只在存在 `passed == Some(false)` 的权威 fact 时才匹配 summary 的正向声明，**用工具结果反证模型自我声明** |
| `symbol_index.rs` 的 `read_to_string().unwrap_or_default()` | 读失败得到空串 | 前面已有 `file_sha256_base64(…)?` 兜底（字节读成功才走到），且 `edit_file` 对非 UTF-8 严格拒绝 → 是「晚发现的诚实报错」而非错误编辑 |
| `lsp_diagnostics` 验收臂用 `find` | 与兄弟臂取值方式不同 | 只读工具 + 受 `last_mutation` 窗口保护，触发条件极窄 |
| `agent/broker_approval.rs` 整体 | —— | 安全敏感且已扎实（版本钉死 / `tool_request_key` 绑请求 / 执行前二次核对 / 停会话按代次失效且有测试）。**我不动它** |
| `agent/postconditions.rs` 整体 | —— | 质量最高的一批：`verify_ui` 黑白屏都返回 Ok 已用 `reports_negative_verdict` 专门堵住；返回 `None` 时上层 fail closed |

---

## C. 红线（永久，不再建议）

- 不做单元/集成测试、不做支付集成、不做 i18n
- 三方插件仅限 BSD-3 / MIT / Apache2 / ISC / Unlicense / CC0
- 本轮所有修复**未新增任何永久测试文件**，验证用临时探针跑完即删

---

## D. 不碰的区域

- Cordis DI 接线
- seam 三件套
- session v0→v4 多代迁移
- `!!js` 表达式

---

## D1. 关于「4 个失败用例」的核实结论（收口前补验）

- **真实失败原因**：`agent::eval_runner` 的 4 个用例 panic 于
  `called Result::unwrap() on an Err value: "grader 启动失败：program not found"`。
  测试构造的 grader 命令是 `["grep", "-q", "fixed", "a.txt"]`，
  而 `grep` 在 Windows 上**不是独立可执行文件**（没有 `grep.exe`），
  `Command::new("grep").spawn()` 必然失败。
- **结论**：这 4 个是**环境项，不是被长期忽略的真缺陷**。生产代码路径
  `run_command_grader` 在 spawn 失败时用 `?` 传播并报「grader 启动失败：{error}」，
  是 fail-closed 的正确行为，没有「执行器报成功但其实失败」的问题。
- **但有一个需要你知道的事实**：在这台机器上，这 4 个测试**从未跑过 spawn 之后的任何逻辑**——
  `run_command_grader` 的实际执行、超时、分级判定路径本地是**零覆盖**。
  本轮我修的所有东西都不在这条路径上（工具层 / 闸门层 / 文件层），
  但「eval grader 执行路径在 Windows 本地未被验证」这件事应当记录在案。

---

## E. 如果要继续扫，还剩什么

已系统审过：全部工具模块、破坏性写入全路径、覆盖/披露族（7 处）、
摘要指纹族、MCP 面、环境/文档面、验收闸门本身、`postconditions`、
`capability_broker`、`broker_approval`、`symbol_index`。

**尚未进过**：`agent/pipeline.rs`（编排）、`agent/tools/protocol.rs`（结构化结果协议）、
`agent/execution_loop.rs`、`services/harmony_knowledge.rs`、`services/sdk_api.rs`。
按最近三轮的命中率（8+ 个面：5 个「已做对」、1 个真缺陷、1 个自身收尾漏项），
继续扫的边际收益已经很低，且误改风险在上升。
