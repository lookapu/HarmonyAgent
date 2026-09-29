# DeepSeek Harness 增量分析报告

**基线**：原 `references/deepseek-harness` @ `47f943859b`（2026-08-13 19:38，PR #2519）
**当前**：`4878cdabd`（2026-09-28 19:48，`release(dsh): 0.2.0-rc.1` #5387）
**跨度**：6 周 15 天 / **7990 个提交** / 14941 文件 / **+1,845,579 −274,381 行**
**包组数**：49 → 55

> 快照已于 2026-09-29 刷新至 `0.2.0-rc.1`。
> 本报告基于源码与文档实读；§六为复核后对本报告早期结论的更正。

---

## 一、先看方向性信号：他们删掉了什么

删除比新增更能说明判断。6 周里他们**主动删掉**的东西：

| 删除 | 含义 |
|---|---|
| `e2b/e2b`、`fs-e2b`、`subprocess-e2b` | **云沙箱（E2B）整条线砍掉**。他们试过远程执行并放弃了 |
| `session/session-persistence-sqlite` | **会话存储从 SQLite 退回 JSONL 多代格式**（`session.v0→v4`）。⚠️ 详见 §五.1 |
| `code-runtime/code-runtime-worker-thread`、`workflow/workflow-worker-thread` | worker-thread 运行时整体换成 `ptc-runtime` |
| `client/runtime`、`client/web-react`、`client/schema-form` | 客户端从 3 个大包拆成 20+ 个 `ui-*` 细包 |
| `preset/agent-presets` | 拆成 `agent-preset` + `agent-preset-registry` |
| `host/apiproxy`、`settings/settings-file` | 换成 `api/*-controller` 家族与新 settings 体系 |
| `subagent/tool-subagent-report` | 子 agent 上报工具下线 |

**E2B 整条线被砍**这一条最值得注意：他们做过"把执行世界指向远程"，最后退回来了。
对我们评估"远程 HarmonyOS 设备调试"有直接参考价值 —— 见 §三.2。

---

## 二、6 周新增中，与本项目真正相关的

### 2.1 `docs/persistence-changes/` —— 已抄进本项目

**全新的一整个目录**，之前不存在。规则：任何持久化类型的结构变化都必须留一份带日期的记录，
每条 4 个文件（英文确认书 / 中文对照 / 双语一致性记录 / after-schema 快照），
配套一张**固定判定表**：

| 检测到的变化 | 最低判定 |
|---|---|
| 增加可选事件属性（含其完整子树） | `same-version` |
| 必需属性改为可选 | `same-version` |
| 增加普通事件类型 | `same-version` |
| 事件加更高数字 `data.version` 且旧 payload 分支全不变 | `same-version` |
| 属性改必填 / 增必填 / 改类型 / 删改属性或事件 | `version-bump` |
| 改会话头或事件信封 | `version-bump` |

原话很关键：「规则作用于**完整变化**，所以一个被允许的变化不能掩盖同一次提交里的破坏性变化」。

**已落地**：本项目版本见 `docs/PERSISTENCE_CHANGE_RULES.md`（按 SQLite 场景裁剪，保留判定表 + 两条硬规则，
去掉 schema 快照/digest 链——那是 JSONL 代际才需要的重量）。

### 2.2 SSH 家族 —— "一个执行世界"落地了

新增 `packages/ssh/{ssh,fs-ssh,subprocess-ssh,sandbox-ssh}` + `docs/subsystems/ssh.md`。

核心设计（`docs/subsystems/ssh.md`）：

> The Harness, model transport and Session storage remain on the host. The family
> implements the existing filesystem, subprocess and sandbox APIs; it introduces no
> SSH-specific model tools.

即：**换 provider 而不换 seam**。fs / subprocess / sandbox 三套 API 原样实现，
所有执行坐标（文件标识、可执行查找、进程 cwd、沙箱 workspace root、LSP 的 file URL）
统一指向 SSH 主机，路径在**文件实际存在的一侧**做 canonicalize（保住 `symlink/..` 的文件系统语义）。

两条诚实声明值得抄：
- `processPathFromHostPath()` 对 SSH **不可用** —— "装一个远端产物不等于任意本机路径可移植"
- **"Web 端假设本机文件系统访问的视图需要单独集成；只换 provider 不会让那些视图自动支持远端"**

对我们：**这是"远程 HarmonyOS 设备/远端构建机"架构的现成答案**。
但先读 §一 —— 他们自己把 E2B 那条线砍了，说明执行世界抽象对 SSH 有价值、
对"每请求一个云沙箱"不成立。

### 2.3 `sandbox/sandbox-windows-acl` —— Windows 写沙箱

`AclSandbox`：Windows 上把子进程的**写与删除**限制在工作区 + 私有临时目录。
每次授权 = 三件事合起来：

1. capability-SID allow ACE
2. **拒绝环境自带的父目录删除权**（`FILE_DELETE_CHILD`）
   —— 否则一个受限子进程能删掉**另一个**已授权根里的文件
3. 降级 token 必须匹配的 Low integrity label

三条给我们的直接结论：
- `init()` **在任何 Win32 失败时抛异常，绝不无限制 spawn**
- **环境临时根从不是隐式授权**，必须显式传私有 `tempDir` + 独立 `tempWriteSid`，或 `tempDir: null`
- `workspace-write` **要求工作区与私有临时目录两个身份互不相同**

⚠️ 文档自己写明的局限：`WRITE_RESTRICTED` **只截获写类访问，不挡读**。
子进程能读任何调用方可读的文件。而且 Low integrity label **比应用活得更久**，
对同用户的其他低完整性进程放宽了工作区。

我们产品是 Windows 优先，这条的**机制**（三合一授权 + 拒绝 `FILE_DELETE_CHILD`）值得抄，
但**不能对外宣称"安全边界"** —— 它只挡写。

### 2.4 `deliverables/workspace-changes` —— 每轮改了哪些文件

新增 `packages/deliverables/{workspace-changes,tool-present}`。

`turn/start` 时用 git `add --all` 进**临时 index**（从仓库 index 播种）+ `write-tree` 取基线树；
`agent/turn-stopping` 时再取一次，`diff-tree -r -M --numstat` 比对。
每个 `tools/pre-execute` 都**等基线队列完成**才让工具跑，所以不会有变更发生在基线之前。

- 每个 git 命令带 `GIT_OBJECT_DIRECTORY` 指向临时目录 +
  `GIT_ALTERNATE_OBJECT_DIRECTORIES` 指向仓库 objects
  → **仓库自己的 index/objects/worktree/refs 全程只读**，用户之前的未提交改动不会混进摘要
- 非 git 文件（被 ignore 的、仓库外的）靠 **file 工具编辑前整文件复制**兜底，副本按内容 SHA-1 命名去重
- 一次文件编辑重复多次只计一次
- 只追加一个 log-only `workspace/changes` 事件，**不进模型上下文**

#### 更正：本项目已有这套能力，初版把它写成了「缺失」

初版结论是「我们的 `acceptance.rs` 已经在做写后验证，这个补的是**给人看的**那张变更卡」——
**这个措辞暗示本项目没有这张卡，是错的。** 实读后发现本项目已经实现了它的主体：

| 上游设计 | 本项目对应 | 位置 |
|---|---|---|
| 每轮汇总改动的文件 | `DesktopRoundState.modified_files`，按任务去重、存相对路径 | `chat.rs:4692` |
| 落库并可回看 | `messages.modified_files_json` | `chat.rs:7304` |
| 用户可见 | 消息底部的文件列表（字段注释即写明用途） | `chat.rs:4691` |
| shell 改动也覆盖 | `run_command` 后走 `scan_recent_changes`：按 mtime + 长度 + FNV-64 扫目录，排除 `.git/node_modules/build/…` | `tools/mod.rs:4990` |
| 独立于工具自述 | 部分达成——`run_command` 那路是**结果**（扫出来），文件工具那路是**意图**（模型报了哪个路径） | `chat.rs:11106` |

上游真正多出来的是：**回合首尾的 git 树快照**（比我们的目录扫描更强的一点是能
把「用户本轮开始前就有的未提交改动」摘出去）、**每文件增删行数**、**每文件对比服务**、
**Web 卡片**。前两项要落进本项目属中等偏大工程（要先解决临时 object 目录的生命周期），
且用户的 HarmonyOS 工程未必都在 git 仓库里，收益不确定。

**这一节的教训和 §六 是同一类**：初版据上游 README 推断了本项目的缺口，
没有回到本项目确认。真正动手前先实读，发现主体已存在，
于是把范围收窄成「补真实缺口」而不是「新建一套」。

#### 复核时发现的真实缺口（已修）

`chat.rs` 记录文件列表时只认 `edit_file` / `write_file` 两个工具，
而**同一时刻** `verification_planner` 把 `write_file / edit_file / delete_file /
multi_edit / lsp_rename` 五个都算作变更。两份清单分叉的直接后果是：
模型用 `multi_edit` 或 `lsp_rename` 改的文件，**验收侧认为变了、交付给用户的那张
清单里却没有**——而那张清单正是用户做最终验收的依据。

已修：把这份清单收敛成 `verification_planner::is_mutation_tool` 单一真源，
`chat.rs` 改用它，并复用同一套路径解析（`paths_from_args`，覆盖 `path`/`file`/`from`/`to`
与 `multi_edit` 的 `edits[]` 逐条）。
顺带收紧一处：`write_file` 的 `content` 是整份文件正文，**不再**按补丁头扫描，
否则写入内容里恰好出现 `+++ b/` 就会凭空造出一个假变更。

> **2026-09-29 更正**：本节初稿把 `apply_patch` 也算进这六条，**而 `apply_patch` 根本不是
> 注册工具**（`TOOL_SPECS` 207 个工具里没有它；全树唯一同名的 `eval_patch::apply_patch`
> 是评测 harness 的函数）。真实清单是五个。差集不影响本节结论——`multi_edit` 确实是
> 注册工具，分叉与后果都成立——但「六个」这个数字和据此写出的补丁头解析都是虚的，
> 已一并清理。根因与核对方法见 [postmortem 的「工具名清单里的幽灵条目」](./postmortem/README.md)。


### 2.5 其他

| 新增 | 说明 |
|---|---|
| `experimental/agent-team` | 持久化 roster + 任务 DAG（`revision` 做 CAS）+ 邮箱。**收件回执只在目标 inbox 条目持久化后才确认**，`queued − delivered` 即恢复邮箱。对应我们的 `team_sharing.rs` |
| `compaction/compaction-image-offload` | 图片卸载，解决截图/预览占上下文 |
| `session/session-turn-outline` | 会话大纲 |
| `document/office-to-pdf`、`skill/skill-office` | Office 转 PDF |
| `telemetry/otel`、`host/product-telemetry-otel` | OpenTelemetry |
| `util/{crypto,brand,workspace-path,code-language,http-proxy}` | 通用工具库 |
| `client/ui-approval` | 审批 UI 独立成包（对比我们的 `broker_approval.rs`） |

---

## 三、护栏与审批路径复核（本轮实读 `guards.rs`）

### 3.1 钩子顺序：本项目比上游更严

本项目 `ensure_registered()` 的 pre 钩子顺序是：

```
pre_budget → pre_blacklist → pre_approval → (execute)
```

上游 dsh 是 `tools/pre-execute` waterfall → 审批 → **单调 guard** → execute。

**结论：本项目不存在"用户批准翻过护栏"的漏洞** —— 预算与黑名单在弹窗之前就跑完了，
用户点"允许"只能放行后面已经没人拦的调用。顺序比上游更严，符合要达到的性质。

### 3.2 一处反向的 fail-open（当前不可达，但是埋雷）

`guards.rs:111-113`：

```rust
let Some(app) = inv.ctx.app.as_ref() else {
    return Ok(()); // 无事件环境（测试/离线）：直接放行
};
```

这与上游「**缺应答器 = deny**」是**相反**的语义：

```ts
const approval = this.ctx.get('approval')
if (approval === undefined) {
  return { decision: { kind: 'deny', reason: ... }, ... }
}
```

**当前风险评级：低。** 实读确认：

- `ToolCtx::empty()`（`exec_ctx.rs:109`）带 `#[allow(dead_code)]`，只在 `guards.rs` 测试里用
- `headless_driver.rs` **完全不构造 `ToolCtx`**，走的是另一条执行路径
- 桌面端 `app` 恒为 `Some`

所以今天没有实际漏洞。但它是个**方向相反的默认值**：将来若给 headless 接入
`ToolCtx`（那是很自然的一步），审批会**默认全开**而不是默认全关。
建议在 headless 接线之前，把这个 `else` 分支改成拒绝，或至少留一条显式注释说明它不可用于生产路径。

### 3.3 一处与上游相反但属产品决策

`pre_approval` 有会话级 `SessionToolAllowState`（"始终允许此工具"），
上游的 `ApprovalOutcome` **刻意只有 `allowed-once`，没有 always-allow**。

这是桌面 IDE 的合理便利选择（会话级、有明确 UI、非全局），本次**不改**，
仅记录差异以便日后 review 时知道这是有意为之而非疏漏。

---

## 四、压缩路径复核

见 §六 更正。结论：**配对规则本项目已实现且比上游更细**，本次只补了一条上游有、本项目缺的闸门。

### 4.1 补：摘要必须严格小于被替换内容

上游 `compaction-basic/src/region.ts:414-422`：

```ts
const framedSummaryTokenCount = dependencies.meter.estimateMessage(checkpointMessage)
if (framedSummaryTokenCount >= prepared.shadowedRouteTokenCount) {
  throw new Error(`summary is not smaller than the shadowed content (…)`)
}
```

**本项目此前没有这条。** 风险场景：`old_limit - keep` 很小时，2000 字符上限的摘要
可能比被丢弃的消息还大 —— 压缩反而让上下文变大，反复压缩滚雪球。

**已实现**（`chat.rs` `summarize_rolling_history`）：

- 在待摘要窗口定型、配对修正之后，计量 `shadowed_chars`（真实被丢弃量，
  不是被逐条截断+全局预算裁剪后的 `out` 长度）
- 旧摘要随后被移入提示词，先记 `prev_summary_chars`
- 摘要生成并 `reconcile_summary` 之后比较 `summary_chars >= shadowed_chars + prev_summary_chars`
- 命中则记 `context_summary_not_smaller` 并返回 `None`

**退路是安全的**：两个调用点（`chat.rs:3806` active 触发、`chat.rs:4241` overflow 触发）
对 `None` 的处理都是"不落摘要"，而 `history_limit` 照常下调 ——
即退回**纯裁剪**，上下文仍然收缩，提示文案自动切到"精简对话历史"。

`cargo check` 通过（9.7s）。

---

## 五、两个需要单独决策的点

### 5.1 我们用 SQLite 存 session_events，上游把 SQLite 删了

他们删掉 `session/session-persistence-sqlite`，改成 `session.v0.jsonl` → `v4` 多代格式。
**但这不等于 SQLite 是错的**：

- 他们那套是为**插件市场 + 第三方 session 读取**设计的，JSONL 可被任意工具直接读
- 我们的 `session_events`、`agent_runs`、`execution_steps` 全在 SQLite，已有 50+ migration —— **不要动**
- 值得抄的是 `SessionEvent` 的**逻辑**（append-only + `derive_messages` 投影 + `asOfSeq` 一致性切面），
  不是存储引擎

唯一真风险已处理：他们给每个持久化类型变更都留带摘要的记录，
我们改了 50+ migration 但从没系统记录"旧记录还能不能读" → 判定表已落 `docs/PERSISTENCE_CHANGE_RULES.md`。

### 5.2 项目根下没有 `docs/postmortem/`

他们 `docs/postmortem/` 的 4 篇**这 6 周全部被修订过**（不是新增，是持续补充复发模式）。
最有价值的两条：

- **测试路径 ≠ 出货路径**：178 个全绿测试、100% 行覆盖、0 用户
  —— 测试手工造 `{name, inject, apply}`，自己注入了 `inject`，所以**根本复现不了那个 bug**
- **宽泛的错误包装抹掉了结构化错误码**：一个泛化的 `SEARCH_FAILED` 盖住了 `SANDBOX_UNAVAILABLE`，
  调用方拿到的是错误的启动诊断

已建 `docs/postmortem/`（含纪律与模板）。**具体事故条目需要团队提供**，
本报告不代为编造 —— 见该目录 README。

---

## 六、对本报告早期结论的更正

**我在初版分析中判断错误的一条**：曾把"补压缩切点配对规则"列为第一优先，
理由是"我们按最近 N 条截断，完全可能把 tool_call 留在窗口而把 tool_result 截掉"。

**实读 `summarize_rolling_history` 后确认这是错的。** 本项目已实现：

- `PAIR_LOOKBACK = 8`：窗口多取 8 条旧消息作配对余量
- 窗口起点修正：`while list.first().is_some_and(|(r,_)| r == "tool") { list.remove(0) }`
  丢弃孤立工具结果
- 消息跨度表 `spans: Vec<(usize, usize, u8)>`，kind 0=普通 / 1=工具调用 / 2=工具结果
- head/tail 剪切点的**四向配对平衡吸附**（`chat.rs:12191-12230`）：
  - head 落在未闭合工具调用内 → 扩展到其配对结果之后
  - head 落在孤立结果内 → 收缩到该消息前
  - tail 落在工具调用内 → 回退到调用起点
  - tail 落在孤立结果内 → 跳过该结果
- 吸附后 head/tail 重叠的退化处理
- 错误标记/补丁标记/工具调用+结果 全文 pin，其余头尾截断

**本项目的配对实现比上游描述的更细**（上游描述的是按在途调用计数算平衡，
本项目是按字符跨度做剪切点吸附 + 窗口起点修正）。该建议作废。

教训：判断"上游有而我们没有"之前，必须先读我们的实现。
本项目 `HARNESS_ENHANCEMENTS.md` 里那 23 条已全部 ✅，说明团队早就做过一轮对齐，
初版分析把"已实现"误判成"缺失"了。

**同一个错误在初版犯了两次**，第二次是 §2.4 的变更卡——同样是读了上游 README 就
断定本项目没有对应能力，动手前实读才发现主体已实现（`modified_files` +
`scan_recent_changes` 已在跑）。两次的成因相同：**上游文档描述的是"他们的方案"，
不是"我们的缺口"**，把前者当成后者读就会系统性地高估差距。
**动一行代码之前先花一次实读成本，永远比按错误前提做完一整轮再返工便宜。**


---

## 七、建议动作（复核后）

**状态列于 2026-09-29 逐条核实**：不是照抄初版判断，而是对每条动作回到仓库确认产物存在、
改动落地。写清"怎么核的"，是为了让下一位读者能自己重核而不是只能相信这张表。

| # | 动作 | 状态（2026-09-29 核实） | 核实方式 |
|---|---|---|---|
| 1 | 补压缩切点配对规则 | ❌ **作废** —— 已实现且比上游更细（§六） | 实读 `summarize_rolling_history`：`PAIR_LOOKBACK=8` + 窗口起点孤立结果丢弃 + 跨度表 + head/tail 四向配对平衡吸附 |
| 2 | 补"摘要必须比被替换内容便宜" | ✅ 已实现 | `shadowed_chars` 计量 + 断言，失败记 `context_summary_not_smaller` |
| 3 | 抄持久化变更判定表 | ✅ 已落 | `docs/PERSISTENCE_CHANGE_RULES.md` 存在 |
| 4 | 审 `guards.rs` 审批路径 | ✅ 已审；**§3.2 的建议已部分落地** | 见下方处置说明 |
| 5 | 建 `docs/postmortem/` | ✅ **已完成**（初版写的"待团队提供"已过时） | 目录 + 模板 + 2 篇事故条目，均从 `CHANGELOG.md` 既有记录反推，非新编 |
| 6 | 刷新 `references/deepseek-harness` | ✅ | `dsh-v0.2.0-rc.1` / `4878cdabd8`（2026-09-28） |
| 7 | 评估 SSH 执行世界适配远程设备调试 | ⬜ **仍未评估** | 无产物 |
| 8 | `deliverables/workspace-changes` 变更卡 | ◐ **主体本项目已有**（初版写 ⬜ 是错的，见 §2.4 更正）；已补真实缺口：写工具清单与验证范围统一 | 本次核对：文件列表与 `verification_planner` 曾认 6 个 / 2 个写工具，已收敛为单一真源 |

### §3.2 那处反向 fail-open 的处置

初版给的是二选一："改成拒绝，**或**至少留一条显式注释"。**实际取了弱的那个，
且是有意识地取的**：`guards.rs:111-117` 现在带完整注释（当前为何安全、
headless 接入 `ToolCtx` 前必须改成拒绝）加一条 `approval_bypassed_no_app_handle`
日志，把静默默认变成可见事件。**行为零变化**——`ToolCtx::empty()` 没有审批 UI 是
真实现状而非漏洞，改 deny 会直接打断离线/测试执行。
强选项（改 deny）留作 headless 接线的前置动作，不适合现在做。

### 初版报告之后的追加工作（不来自上游对照）

初版之后又做了一轮**项目内自查审计**，与 deepseek-harness 无关。
列在这里是为了不让它从索引里消失——它已经改了 10 余个 commit，
但来源是本仓库自身的验收闸门缺陷，不是对照结论：

- **「工具跑成功」被当成「验证出好结果」**——`verify_ui` / `run_lint` / `check_code` /
  `get_app_info` / `run_command` 五处同族缺陷，全部已修。其中 `run_lint` 与
  `check_code` 是 `required: true` 的必需验证步骤。
- **验收证据可被命令行措辞伪造**——`acceptance.rs` 的 `is_command` 按整段参数子串匹配，
  `git commit -m "fix build"` 能顶掉构建判据。
- 沙箱不可用被错分类成超时并给出"调大 timeout"的错误建议；压缩摘要闸门；
  ohpm / 鸿蒙文档接口的空值不吞。

逐条机制与取舍见 `docs/postmortem/README.md`（该目录的"已核实并已修"一节）。

**不建议**：跟 Cordis DI、seam 三件套、v0→v4 多代迁移、`!!js` 表达式
（他们自己因此出过事故 postmortem 0002）。

---

## 附：证据来源

- 基线与当前提交、diffstat、包级增删、`docs/` 增删：`git diff` 直接得出（已 `--unshallow`，12293 提交）
- `docs/persistence-changes/README.md`、`packages/deliverables/workspace-changes/README.md`、
  `docs/subsystems/ssh.md`、`packages/sandbox/sandbox-windows-acl/README.md`：实读全文
- `docs/defensive-patterns.md`、`AGENTS.md`、`SAFETY.md`：实读全文
- 本项目 `chat.rs` `summarize_rolling_history`（12064-12365）、`guards.rs`、`exec_ctx.rs`：实读
- 工具循环 / 审批安全 / 上下文与子 agent / 组合与工程实践：4 个只读子代理并行深挖
- `cargo check` 通过（9.70s）

**§七 状态列的复核（2026-09-29）**：`docs/PERSISTENCE_CHANGE_RULES.md`、
`docs/postmortem/0001-*.md`、`0002-*.md` 三个文件实测存在；
`references/deepseek-harness` 实测 `git describe` = `dsh-v0.2.0-rc.1`、
HEAD = `4878cdabd8`（2026-09-28）；`guards.rs:111-117` 的注释与
`approval_bypassed_no_app_handle` 日志实读确认。#7 / #8 无任何产物，如实标 ⬜。

