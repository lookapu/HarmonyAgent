# Postmortem

事故复盘。**目标不是追责，是把复发模式钉成规则。**

## 什么时候写

只在**同时**满足这三条时写：

- **微妙** —— 读代码看不出来，必须跑起来或读日志才发现
- **系统性** —— 不是笔误，是一类问题的实例
- **重新发现的代价高** —— 下一个人会再踩一遍，且踩的时候不知道自己踩过

不满足就写进 commit message 或 CHANGELOG，别开一篇。

反面例子：「忘了加 null 检查」——不是微妙，写了浪费。
正面例子：「测试手工注入 `inject`，所以根本复现不了那个 bug」——微妙、系统性、代价高。

## 写法

用**复发模式**做标题，不带日期和版本号。
`docs/` 里的文件名一旦被引用就成了常青链接，带日期的标题半年后没人搜得到。

```
docs/postmortem/000N-<复发模式 slug>.md
```

`.md` 正文用中文；保留必要的英文标识符（事件名、字段名、命令）。

## 必答的五问

1. **现象** —— 观察到什么（不是推测的原因）
2. **为什么没被立刻发现** —— 绿灯的测试 / 监控 / review 在哪
3. **真正的机制** —— 为什么会这样，精确到代码路径或数据流
4. **复发模式** —— 这一类问题的**通用规则**，不是这一个 bug 的修法
5. **钉住了什么** —— 哪条机械检查 / 不变量 / review 清单条目拦住了它

第 5 问是关键。「我们会更小心」不算答案。
答案应该是「`verify-xxx` 脚本现在拒绝 Y」或「`docs/ZZZ.md` 的清单加了第 N 条」。

第 3 问容易写成猜测。**信 trace，不信 theory。**

## 修订

复盘是活的。发现当初的机制判断错了就改，不要新开一篇。
归档的复盘冻结，不删不改。

---

## 本项目复盘

- [0001 — 幂等写入的冲突键随数据形态漂移，UPSERT 退化成 INSERT](0001-upsert-conflict-key-drifts-with-data-shape.md)
- [0002 — 依赖非契约外部端点，端点下线后整条链路静默空跑](0002-external-endpoint-without-contract-fails-silent.md)

两篇都从 `CHANGELOG.md` 的既有记录反推，不是新编的事故。
凡证据不足的判断一律写 `待补` 而不是补全——**缺口本身是信息**。

### 已核实并已修（2026-09-29）

- **沙箱不可用被错分类**：`sandbox.rs` 把错误类型做成 `String` 前缀
  （`sandbox_unavailable: …`，6 处），下游只能按关键词反推。探测超时的文案含「超时」，
  于是 `classify_error` 报成 `TOOL_TIMEOUT` + 可重试、`is_retryable_err` 判为瞬态、
  `run_command` 的建议还让模型「调大 timeout 参数」——而那 3s 是**后端探测**超时，
  与命令超时无关，调大只会烧掉重试预算。其余变体（无原生后端 / AppContainer 未实现）
  则落到 `TOOL_EXECUTION_FAILED`。**基础设施缺失被报成任务失败或命令超时，两种都导致错误决策。**
  已修：`structured_result.rs` 增加 `SANDBOX_UNAVAILABLE` 分类臂（排在超时分支之前），
  `errors.rs` 对 `sandbox_unavailable` 前缀短路可重试判断与建议分支。

  **同一语义的第三处（本轮补上）**：`reflexion.rs::suggest` 是**复盘建议表**，
  按关键词「先命中先返回」给模型下一步指引，表里有裸词 `"超时"`（第 7 位），
  却没有 `sandbox_unavailable`——于是同一类错误在这里被复盘成第三条路。
  实测（修复前，`suggest("run_command", "sandbox_unavailable: hdc 能力探测超时（3s）")`）：

  | | 实际给出的建议 |
  |---|---|
  | 修复前 | 「任务/命令超时后应拆分为更小的步骤重试，不要原样重复长命令」 |
  | 修复后 | 「沙箱不可用：…改用不需要沙箱隔离的工具，或在设置里改用其他执行方式」 |

  前者让模型把**宿主能力缺失**当成命令慢，拆多少次都不会成功。
  这处比前两处更靠后也更要紧：`classify_error` 只影响错误分类，
  `is_retryable_err` 只影响重试预算，而 `suggest` 决定的是模型**接下来做什么**。
  已修：表首加 `sandbox_unavailable` 条目，并写明必须排在 `"超时"` 之前。
  修复后同时确认**真命令超时**（`命令执行超时（300s）`）仍命中原来那条，没有被误伤。

  **这一族的审计形状**：「某个错误文本有两种含义，而下游用关键词反推」——
  每加一个下游消费者就得记得加一次短路。已确认**三处全部覆盖**：
  `structured_result::classify_error`（分类）、`errors::is_retryable_err`（可重试）、
  `reflexion::suggest`（建议）。新增第四个消费者时，
  先 grep `sandbox_unavailable` 看这三处是否都被覆盖。

- **"验证工具跑成功"被当成"它验证出了好结果"**：`verify_ui` 无论判定黑屏/白屏还是正常，
  都返回 `Ok(report)`——它确实成功截到了图，所以 `succeeded` 为 true。而
  `postconditions.rs` 的写后读确认矩阵只看 `succeeded`，于是
  「`deploy` 成功 + 在黑屏上 `verify_ui`」被判成"已从设备读取界面状态并确认"，
  模型可以据此宣布部署完成。**这正是把请求成功误当任务成功**——与本目录 0002 同一族，
  只是这次错位发生在验收闸门而不是数据链路。
  讽刺的是 `verify_ui` 内部的黑屏/白屏/纯色检测做得很扎实，判定也带 ❌ 标记，
  **但这个结论没能穿过工具边界变成失败信号**，只留在给模型看的文本里。
  已修：`postconditions.rs` 引入 `verifier_confirmed`——验证工具除了 `succeeded`
  还必须没有负面结论才计入确认。范围刻意收窄：只有 `verify_ui` 自带结论，
  只有 ❌ 阻断，`⚠️ 异常纯色` 不阻断（启动页/纯色遮罩本来就可能是平的）。
  刻意**没有**改 `verify_ui` 本身去返回 `Err`——那样会让工具报错路径接管，
  截图多半不再自动进入模型视野，反而丢掉最好的诊断信息。

- **同一族的第二处：静态检查闸门对 `succeeded` 完全失明**。`verification_planner::completion`
  对 `lsp_diagnostics` 和 `check_sdk_alignment` 都做了结论感知（要求输出里出现
  「无诊断错误」/「0 error」），但对 `run_lint` / `check_code` 落到兜底分支
  `runs.last().map(|_| (true, …))`——**只看 `succeeded`**。而这两个工具恰恰是
  「跑通即 `Ok`」的：
  - `run_lint`（`debug_tools.rs:350`）只要 lint 工具本身启动成功就 `Ok(out)`，
    报告正文白纸黑字写着「共发现 37 个问题 / 错误 (error)：12」；
  - `check_code`（`scanner.rs:270`）是规则扫描，命中再多也 `Ok`，
    开头就是「静态检查完成：扫描 210 个文件，48 条命中」。

  两者在计划里都是 `required: true`（`verification_planner.rs:67` / `:69`），
  于是「改了 ETS → 跑一次 `run_lint`，报告 12 个 error」会被判成
  **「静态规则检查已完成」**，`pending_required()` 为空，验收闸门放行。
  **这不是「检查没发现问题」，是「检查发现了问题但闸门看不见」。**
  已修：`completion` 为这两个工具各加一条结论感知臂——
  `run_lint` 要求输出出现「错误 (error)：0」，且**带 `severity` 过滤只筛 warning 的
  运行不算数**（那种运行报告里的 0 是统计口径造成的空值，不是干净结论）；
  `check_code` 只把**高危/中危**当阻断项（`debug-log`、`plaintext-http` 这类提示/低危
  在任何真实仓库都会命中，一并阻断会让这个必需步骤永远无法完成），
  且**输出被 `scanner::cut` 截断时一律不算通过**——高危分组可能整段没进输出，
  「没看到高危」不等于「没有高危」，此时证据里直接要求缩小扫描范围重跑。
  与 `verify_ui` 同理，**刻意不改工具本身**：`run_lint` / `check_code` 改为返回
  `Err` 会把问题清单塞进错误通道，而完整报告正是模型修复所需的输入。

  同批核实为**正确**、无需改动的另一半：`run_tests` 走 `run_cmd`（非零退出即 `Err`）、
  `build_generic` 显式判 `output.status.success()`（`mod.rs:2818`）、
  `build_project` 失败两子路径都 `return Err`，这三者失败时 `succeeded` 确实为 false，
  兜底分支对它们是可信的。也就是说兜底分支本身没错，
  **错的是把两个「跑通即 Ok」的工具一起塞进了兜底**。

- **同一族的第三处：执行循环把「跑过一条命令」当成「验证过改动」**。`execution_loop.rs`
  的 `last_verifier` 判据是 `contract(item.tool).validator.is_some()`，**只查工具名**。
  而 `contracts::validator` 里 `run_command` 恒为 `Some(ValidatorKind::Command)`
  ——契约描述的是「这类工具**可以**当验证器」，不是「这次调用**就是**验证」，
  两个语义被当成了一个。于是 `edit_file` 之后跑一条 `run_command("echo hi")`，
  `last_verifier` 就落在 `last_effect` 之后，`needs_post_effect_verification` 变 false，
  循环从 `Verify` 提前跳到 `Execute`。**执行过命令 ≠ 验证过改动。**

  讽刺的是正确判据**项目里早就写好了**：`structured_result.rs::declared_validator`
  已经把 `run_command` 收窄成「命令本身确实含 test / build / cargo check /
  git diff / git status」才算一次验证，`echo hi` / `ls` / `cat` 拿不到 `Command` 标签——
  只是 `execution_loop` 没有复用它。已修：把 `declared_validator` 提为 `pub(crate)`
  并在 `execution_loop` 里复用。**刻意复用而不是另写一份**：结构化结果信封会把这个
  标签展示给模型，两处各判各的就会出现「信封说这不是验证器、循环却当它是」的漂移，
  那本身就是一个新 bug。

  残留（比原设想窄得多）：`declared_validator` 的命令匹配仍是整段子串包含，
  `git commit -m "fix build"` 仍能拿到 `Command` 标签。但这类命令**自身是 Destructive
  副作用**，会同时成为 `last_effect`，而 `last_verifier <= last_effect` 的判据要求验证器
  严格在副作用之后——同一条记录两者索引相等，所以它顶不掉「写完之后还要验证」这一关。
  真正把关的 `acceptance.rs` 判据上一轮已单独收紧。

- **同一族的第四处：设备侧空读被当成部署已确认**。`postconditions` 的 deploy 写后读
  确认器包含 `get_app_info`，只要它 `succeeded` 就算「已从设备读取应用状态并确认」。
  而 `get_app_info`（`ui_tools.rs`）对不存在的包名会拿到一份**没有应用记录**的
  `bm dump` 输出——exit 0，也不含 `error:` / `[Fail]` / `not found` 这些
  `hdc_shell_failed` 认识的文本特征——于是六个字段全部 `unwrap_or_default()` 成空串，
  照样 `Ok` 一份「查到了但什么都是空」的报告。**空读不是确认。**
  触发路径不需要部署真的失败：包名与工程元数据不一致、查询落到另一台设备、
  安装后包未注册，都会走到这里。
  已修：拿到 `raw` 后先判 `"bundleName"` 这个键在不在，不在就返回 `Err`。
  判据不是猜的——`commands/devices.rs` 的 `list_installed_apps` 解析的是**同一份**
  `bm dump`，它认的键就是 `"bundleName"`（`strip_prefix("\"bundleName\" : \"")`），
  所以「dump 里没有 bundleName」等价于「这次查询没拿到应用记录」，
  且该检查不依赖冒号两侧的空格写法。
  同文件的 `sample_battery_percent` 早就用 `.ok_or_else(|| "未读取到有效电量")`
  堵过同一类空读，这次是把同一口径补到 `get_app_info` 上。

- **兜底分支把自己的输入当成了结论**：`structured_result::argument_artifacts` 解析工具
  产物路径时，先用一个 `walk` 递归找出「键名里含 `path` 或 `file`、或键名正好是 `hap`」
  的字符串值。`apply_patch` 的参数是 `{"patch":"*** Update File: src/lib.rs\n…"}`，
  而 `"patch".contains("path")` 是 **false**（第 4 位是 `c` 不是 `h`），于是 walk 一个路径都收不到。
  接着落到兜底：`args.contains('/') || args.contains('\\') || args.rsplit_once('.').is_some()`。
  **这个条件对几乎任何含路径的 JSON 都恒真**，于是整段 args 被当成文件路径写进了产物。
  实测（修复前）：`artifacts[0].path` 与 `side_effects[0]` 都是那串 JSON 原文。

  > **2026-09-29 更正**：本条当时把触发工具写成了 `apply_patch`，**而 `apply_patch` 根本不是注册工具**
  > （证据见下方「工具名清单里的幽灵条目」）。触发源是当时构造的合成证据，生产里走不到。
  > **但下面这个修复本身仍然承重**，触发源换成真实工具：任何**没有 path 类字段**的调用都会走 walk
  > 空收，再撞上恒真的兜底——`web_fetch` 的 `{"url":"https://…/a.html"}`、`run_command` 的
  > `{"command":"python -m pytest tests/"}` 都会把整段 JSON 当成文件路径。
  > 错的是**理由**（点名了一个不存在的工具），不是**判据**（兜底确实过宽）。

  后果有两处：
  1. `side_effects` / `modifications` 对外披露的改动目标是假的；
  2. `acceptance::evaluate_contract` 的 `mutation_targets` 取自信封 `artifacts`，
     读回校验拿这个假路径去和真实 `read_file` 路径比对，**永远匹配不上**——
     于是「`apply_patch` 改文件 + 读回验证」这条路径无法满足 Verification 判据
     （只有先跑过构建/测试/`git_diff` 走全局验证器那条分支才能绕开）。

  已修：补丁头解析抽成 `verification_planner::patch_paths_from_args` 共用（`paths_from_args`
  本来就有这套解析，两处各写各的才会分叉）；兜底条件改为 `looks_like_bare_path`，
  **显式排除 JSON 形态**（含 `{` / `"` / 换行，或超长）。

  **复发模式**：兜底分支必须先回答「输入会不会其实是别的东西」。
  原兜底想表达的是「参数本身就是一个裸路径串」，
  但它实际也覆盖了「参数是结构化对象、只是没有路径字段」——
  **而这两种情况的含义恰好相反**：前者是「路径 = 整段参数」，
  后者是「这次没碰文件」。前者产出正确结果，后者产出假证据。
  判据写成「含斜杠」这类**对宽松输入恒真**的形状时，
  兜底就会在它最不该触发的地方触发。

- **验收闸门用「更早的证据」验证「更晚的改动」**：`acceptance::is_mutation` 与
  `verification_planner::is_mutation_tool` 是同一概念（哪些工具算变更）的两份手写清单，
  且**双向都不同**。前者是
  `write_file|edit_file|delete_file|apply_patch|create_project|git_merge|db_migrate`，
  后者是 `write_file|edit_file|delete_file|apply_patch|multi_edit|lsp_rename`——
  **`multi_edit` 与 `lsp_rename` 只在后者里**，而两者都是注册工具、ToolSpec 明写
  「副作用：修改项目内文件」。

  `evaluate_contract` 用 `is_mutation` 定位 `last_mutation`，Verification 判据要求
  证据出现在 `last_mutation` **之后**。`multi_edit` 不被认成变更时，这个「最后」还停在
  上一次 `edit_file`／`lsp_rename`——于是**在 multi_edit 之前跑过的 `git_diff`／构建／
  测试会被当成它之后的验证而放行**。模型改完文件不用再验证，验收照样通过。

  修复前后实测（同一序列 `edit_file(a.rs)` → `git_diff` → `multi_edit(b.rs, c.rs)`）：

  | | 结果 |
  |---|---|
  | 旧清单（漏 `multi_edit`） | `passed=true`，`blockers=[]` ← 误放行 |
  | 收敛后 | `passed=false`，`blockers=["变更后已读取、差异检查、构建或测试验证"]` |

  已修：`is_mutation` 改为 `is_mutation_tool(tool) || <非文件类变更>`，让
  **「文件变更工具集 ⊆ 变更工具集」成为结构性保证**而不是靠人记得同步。
  反向多出的 `git_merge` / `db_migrate` 是有意的分工——它们不写工作区文件，
  但同样让既有验证失效；`verification_planner` 只回答「哪些文件要验证」，
  这里回答「哪些操作让已有验证作废」。两个函数名的差别就是这个分工，不是重复。
  （`create_project` 目前不是注册工具，TOOL_SPECS 无此项，保留为防御性条目。）

  **与前一条同源**：同一份判定写两遍就必然分叉，而分叉的后果发生在**下游闸门**上——
  两处都不报错、都不崩，只是各自安静地按自己的理解工作。
  这类缺陷靠读单个函数看不出来，必须把两个函数并排比。

- **同一族的第五处：审批闸门也自己抄了一份写工具清单，`first_write` 模式形同虚设**。
  `guards.rs::pre_approval` 里的
  `is_write_tool = matches!(tool, "edit_file" | "write_file" | "delete_file")`
  是第三份手写清单，漏掉 `multi_edit` 与 `lsp_rename`——两者都是注册工具，
  ToolSpec 明写「副作用：修改项目内文件」，`is_mutation_tool` 也早把它们算作变更。

  后果不是「少弹一次窗」，是**这个模式的核心承诺失效**。分支结构是：

  ```
  if recovery || sensitive_operations        -> true   （弹窗）
  else if first_write && is_write_tool       -> !approved
  else if first_write                        -> false  ← 非写工具直接放行
  ```

  `is_write_tool` 漏掉两个工具后，它们全部落进第三条 → `needs_approval = false` → 直接放行。
  **用户特意选了最保守的权限模式，Agent 用 `multi_edit` 改工作区却一次都不确认。**
  两条兜底臂都确认过不覆盖它们：`permissions::requires_fresh_explicit_approval`
  只处理发布安全域（`ota_pack`/`sign_hap`/`secret_get` 等），
  `recovery::requires_confirmation_global` 只在存在 AwaitConfirmation 恢复计划时才为真。

  讽刺的是 `multi_edit` 正是项目自己推荐给模型用的编辑工具
  （`errors.rs:310` 的「替换原文未找到」建议、`reflexion.rs` 的连续失败兜底都在引导它）。

  修复前后实测（同一判据对同一组工具）：

  | 工具 | 旧清单（是否弹窗） | 收敛后（是否弹窗） |
  |---|---|---|
  | `write_file` / `edit_file` / `delete_file` | true | true |
  | `multi_edit` | **false** | true |
  | `lsp_rename` | **false** | true |
  | `run_command` | false | false ← 按设计仍免审（first_write 只管写文件） |

  已修：`is_write_tool` 直接复用 `verification_planner::is_mutation_tool`，
  与验收侧共用同一份真源。**这是收紧不是放宽**：多弹一次窗，方向朝安全侧。

  **复发模式补充**：这一族前四次都发生在**验收**侧（判定「验证过没有」），
  这次发生在**审批**侧（判定「该不该拦」）。同一个概念的清单会沿着调用链向下传染，
  修完一处不等于修完这条链——`is_mutation_tool` 被 `acceptance` / `chat.rs` 复用之后，
  下一个不知道它存在的人照样会在旁边新写一份。

- **工具名清单里的幽灵条目：`apply_patch` 根本不是注册工具**。
  它被当作写工具列在**五处判据**里（`verification_planner::is_mutation_tool`、
  `context.rs` 两处缓存失效、`structured_result::argument_artifacts` 的 operation 映射、
  `contracts::recovery_action`），但全仓核实：

  - `TOOL_SPECS` 共 207 个工具，`name` 里含 `patch` 的**零个**；
  - `run_tool` 没有 `"apply_patch" =>` 派发臂；
  - 全树唯一叫 `apply_patch` 的函数是 `agent/eval_patch.rs::apply_patch`——
    评测 harness 往 git 工作树里应用补丁，与工具目录无关；
  - `chat.rs:12135` 里的 `"apply_patch"` 与 `"diff --git"`、`"*** begin patch"`
    并列，是**压缩钉住用的文本特征**：模型可能在正文里吐出裸补丁，靠子串识别。
  - 没有任何注册工具声明 `patch` 参数，因此
    `verification_planner::patch_paths_from_args`（唯一调用方 `structured_result.rs:479`）
    在生产里恒返回空——为幽灵工具写的解析路径走不到。

  **直接后果**：为它写的修复是给幽灵治病的。上文「兜底分支把自己的输入当成了结论」
  一条的 `patch_paths_from_args` 复用即属此类（该条已就地标注更正）。
  运行时无害（死条目不改变任何判定），但它有两个真实代价：
  1. 让人据此写下整段修复并以为验证过——本项目的 `4f1e746` 就是这么来的；
  2. 让「6 个写工具」这个说法虚高，`is_mutation_tool` 自称**唯一真源**却含一个不存在的成员。

  同一份文档里 `create_project` 早已被标注为「非注册工具，保留为防御性条目」，
  说明这条纪律存在过，只是没被套用。

  **复发模式**：给「某个工具的行为」写修复前，**先确认这个工具存在**。
  在本项目里这是一条命令的成本：

  ```
  rg -o --no-filename 'name: "[a-z0-9_]+"' src-tauri/src/agent/tools/mod.rs | Sort-Object -Unique
  ```

  拿到的目录就是全集。**任何工具名清单（含 `is_mutation_tool` 这类自称真源的）
  里的每个名字都应当在这份目录里查得到**；查不到的，要么删掉，要么注明是为
  尚未注册的能力预留的防御性条目——不能两者都不说，让它看起来像已接线。

  与「同一份判定写两遍必然分叉」互补：那一族是**同一概念出现多次**，
  这一族是**同一清单里混进了不存在的东西**。共同点是都靠肉眼维护，都不报错。

- **同一族的第六处：跨文件改名后，会话事实与项目记忆都停在改之前的结论**。
  `context.rs::record_tool_evidence` 里有**两份内容相同**的手写清单
  （`invalidated_kinds` 与 `file_changed` 事件各一份），都漏掉 `lsp_rename`。
  而 `lsp_rename` 的 ToolSpec 明写「基于 AST 找出全部引用并同步修改（**跨文件**）…
  副作用：修改文件」——**它是改动面最广的那个，恰恰是唯一被漏掉的那个**。

  后果有两层：
  1. `invalidated_kinds` 取 `&[]` → 会话内 `verification` / `workspace` 两类事实
     不失效 → 后续轮次注入的是改之前的结论；
  2. `event` 取 `None` → `invalidate_project_memories` **根本不调用** →
     durable project memory 的 `file_changed` 失效链整条不触发。
     用户亲手写下的「`build-profile.json5` 修改时失效」这类记忆，
     在一次跨文件重命名之后，依然被当作有效事实喂给模型。

  修复前后实测（真实 SQLite + 真实 `record_tool_evidence` 调用路径，
  记忆条件设为「`src/main.ets` 修改时失效」，工具参数 `{"path":"src/main.ets",…}`）：

  | | `project_memories.invalidated_at` |
  |---|---|
  | 旧清单 | `None`，断言 panic |
  | 收敛后 | `Some(…)`，通过 |

  已修：两处共用同一个 `is_mutation`，且都来自 `verification_planner::is_mutation_tool`。
  **在同一个函数作用域里先算出 `is_mutation` 再复用两次，比"两次调用同一函数"
  更能防住再次分叉**——第二次根本没有机会写出不同的名字。

  顺带：`structured_result::argument_artifacts` 的 `operation` 映射同样漏了
  `multi_edit` / `lsp_rename`，还带着两个幽灵条目（`apply_patch` / `create_project`），
  于是这两个工具的产物被标成 `produce`——**写操作报成「产出」**。
  该字段是 display-only（只进信封与 context 标签，没有闸门读它），
  但给模型的标签不该是错的，一并修。

  **这一族已数到第七处，且每一处都在不同的下游**：
  `acceptance` 变更集 → `chat.rs` 变更清单 → `postconditions` 确认器 →
  `execution_loop` 验证器 → 审批 `first_write` → `context` 事实失效 →
  **`task_guard` 进展判定**。共同点始终不变：不报错、不崩、测试全绿，
  只是各自安静地按自己的理解工作。

- **同一族的第七处：模型成功改了文件，却被告知「你没有进展」**。
  `services/task_guard.rs::edited_path` 是**又一份**手写清单，只认
  `write_file|edit_file|delete_file` 的 `path` 字段，漏掉 `multi_edit`（走 `edits[]`）
  与 `lsp_rename`（走 `path` 但不在名单里）。

  这一处漏项的后果**不是陈旧状态，而是往模型上下文里注入假信号**：
  1. `is_progress_action` 把成功的写文件调用判成「无进展」→ `since_progress` 照涨 →
     达到阈值后注入
     「已连续 N 次工具调用未产生实质进展（**无写文件**/构建/部署/测试）…不要长时间停留在只读探索上」。
     失速警告的文案白纸黑字写着「无写文件」，而 `multi_edit` 恰恰是写文件——
     **模型正在正确干活，却被劝去换思路。**
  2. 「同一文件连续编辑 N 次仍未验证，请立即构建验证」的强制验证提示对这两个工具
     **永远不触发**，而 `multi_edit` 正是项目自己的错误诊断（`errors.rs:310`）
     引导模型使用的编辑工具。

  修复前后实测（真实 `record_tool` 调用路径，连续 `STALL_TOOL_THRESHOLD + 3` 次
  成功的 `multi_edit`，每次改 `src/a.ts` + `src/b.ts`）：

  | | 结果 |
  |---|---|
  | 旧清单 | 第 N 轮即 `stall_warning` 出现，断言 panic |
  | 收敛后 | 全程无失速警告，`edit_counts` 两个文件各累计 13 次 |

  已修：`edited_path` → `edited_paths`，工具判据与路径提取**双双**复用
  `verification_planner` 的唯一真源（`is_mutation_tool` + `paths_from_args`）。
  改成返回全部命中文件而非单个：一次 `multi_edit` 可以改多个文件，
  只记第一个会让同文件连续编辑次数被系统性低估。
  `is_progress_action` 的第三个参数相应收敛为 `bool`（是否改过任何文件），
  避免又把「单个 Option」这种会诱发分叉的形状带进来。
  分层上 `services` → `crate::agent` 已有先例（`team_sharing` / `reproduction_bundle`
  等 5 处），不新增依赖方向。

- **写文件不变式只接了 fs 侧，LSP 写路径整条绕开**：`agent::invariants` 的模块文档写着
  「环境约束 > Prompt 约束——写文件前必须满足的硬性不变式…**全部写路径自动生效，
  无需改动各调用点**」。核实后发现：真正调用 `check_write` 的 6 处**全在 `fs_tools.rs`**
  （`write_file` / `edit_file` / `delete` / `move` / `copy` / `multi_edit`），
  `lsp_client.rs` **一处都没有**——整个模块没有任何 `check_write` / `is_protected_file` /
  保护名单，而它的唯一落盘点 `apply_text_edits`（`std::fs::write` 全模块只出现这一次）
  是**无条件写盘**。

  于是 `lsp_rename` / `lsp_format` / `format_file` / `lsp_code_action` 全部可以改
  `.env*`、`.key|.pem|.pfx|.p12` 与已应用的迁移 SQL——而 `invariants.rs:4` 那行注释
  恰恰把覆盖范围写成了 `write_file/edit_file/delete/move/copy/multi_edit`，
  读的人只会以为「名单之外的都是 LSP，本来就不归它管」，
  不会意识到那句「全部写路径自动生效」是假的。

  修复前后实测（真实 `apply_text_edits` 调用，对一个含 `TOKEN=old` 的 `.env` 施加文本编辑）：

  | | 结果 |
  |---|---|
  | 修复前 | 写入**成功返回 Ok**，`.env` 内容被改（`unwrap_err` 直接 panic） |
  | 修复后 | 返回 `写入被安全策略拒绝（secrets_env 不变式）：…`，文件内容原样 |

  已修：在 `apply_text_edits` 落盘前接 `check_write`。选这一处而不是四处各接，
  因为它是 LSP 写路径的**唯一收口**（`apply_workspace_edit` 的 4 个调用点全经它）。
  同时确认正常文件仍放行并真正写入（`var a = 1` → `var b = 1`），没有误伤。

  **与上面「写前门禁」的既有说明区分开**：`apply_text_edits` 里早就有
  `validate_code_mutation`（校验**候选文本**的括号配平 / Tree-sitter），
  容易误以为「写前门禁已覆盖」。那是**文本合法性**，这一条是**目标文件该不该被改**，
  两道不同的门，缺一不可。

  **复发模式**：**闸门的「覆盖范围」是一个会被文档固化、却没人复核的断言。**
  「新增一条不变式 = 往 INVARIANTS 追加，全部写路径自动生效」这句话，
  让后来人以为覆盖是自动维持的——而**接入点是手工的**。
  正确写法是把接入点显式列出来并在文档里点名文件（本轮已改）：
  新增任何能落盘的工具时，必须在它的落盘点补一次 `check_write`。
  **判据：宁可写「已接入的写路径是 A/B/C」，也不要写「全部写路径」。**
  覆盖率这种断言一旦写「全部」，就再也没人会去数。

- **验收证据可以靠"命令行里出现关键词"伪造**：`acceptance.rs` 允许 `run_command`
  充当构建/测试的验证证据，判定方式是**参数子串匹配**，而 Build / Tests 用的
  是无分隔符的裸词 `build` / `test`。于是
  `run_command("git commit -m \"fix build\"")` 满足「构建成功」，
  `run_command("git commit -m \"add tests\"")` 满足「测试通过」。
  两条判据都是 `required: true`（目标里提到"构建"/"测试"就必生成），
  **一次提交就能同时顶掉构建与测试两项**。
  已修：`is_command` 改为按 shell 操作符分段、只取每段**真正被执行的前 4 个词**再匹配，
  引号里的说明文字不再算证据。同时保留 `cd frontend && npm run build`
  这类常见写法（第二段头部仍是 build）。
  **残留**：`cat test_notes.md` 这类"文件名词里含关键词"仍会匹配——
  再收紧就要判断"这个命令到底跑不跑测试"，那是产品决策不是字符串匹配问题，
  留待单独评估。

  同批核实为**正确、无需改动**的：`build_project` 构建失败时两个子路径都 `return Err`
  （`build_tools.rs:490`）、`run_cmd` 非零退出码返回 `Err`（`mod.rs:1596-1599`），
  所以 `succeeded` 对这两条主验证路径是可信的。

- **目录树空列表被当成"接口正常但没内容"**：`harmony_doc_api.rs` 对
  `code:0` 且 `catalogTreeList` 为空数组的响应返回 `Ok(vec![])`，调用方建出空索引后
  所有模块静默退回 slug 猜测且不留痕迹——与"接口失败"不可区分，正是 0002 的形状。
  已修：空列表改为 `Err`。**行为不变**——调用方对 `Err` 的退路（`harmony_api_ref.rs`
  的 Err 分支）同样是退回 slug 猜测，只是多留一条 `catalog_note`，故障从静默变可见。

- **ohpm 仓库查询把"仓库挂了"报成"该包没有仓库地址"**：`ohpm_landscape.rs::repo_url`
  对所有非成功状态返回 `Ok(None)`，5xx 与 404 不可区分。已修：404（包不存在）仍是
  `Ok(None)`，其余非成功状态改为 `Err` 并带 HTTP 状态码。唯一调用方是 Tauri command，
  错误通道本来就存在，不改签名。

### 同一族扫描的收口：哪些闸门真的承重，哪些只是提示

这一族四处已修之后，逐个把 postconditions 矩阵里的确认器重新核实了一遍，
结论分三档。

**① `postconditions::pending()` 是 display-only，不是闸门。**
`execution_loop.rs:56` 只是把它存进快照，`directive()` 拼成提示文本，
**从不影响 `acceptance.passed`、`stage` 或 `blockers`**（对比同文件 49-55 行：
`pending_verification` 是会 push 进 blockers 的硬闸门）。
承重的只有一条路径——`acceptance::evaluate_contract` 对
Deploy / GitCommit / GitPush 三类判据走 `postconditions::criterion_evidence_indices`，
它同样用 `verifier_confirmed`，返回 `None` 就清空 evidence → `passed=false` → **硬阻断**。
所以 `verify_ui` 与 `get_app_info` 两处修复是通过这条路径生效的，
判定它们「无效」会是错的；而挂在 `manage_memory` / `manage_knowledge` /
`db_migrate` / `secret_store` / `http_request` 上的那些确认器，改成结论感知
只是多一句提示，值不值得改要单独算，不能和前两者混为一谈。

**② `search_knowledge` 查不到被当成写入已确认 —— 成立，但决定不改。**
`memory_tools.rs:197` 在 0 命中时返回 `Ok("知识库中没有匹配「X」的条目。…")`，
而它正是 `manage_memory` / `manage_knowledge` 的确认器，
于是「查不到」被读成「写入已确认」——模型搜了个对不上的词就算交差。
**但这条矩阵是 ① 里的 display-only**，改成「必须命中才算确认」会在
`manage_memory action=delete|disable` 时变成永远满足不了的提示
（条目已删/已禁用，再搜必然搜不到），把一个提示问题换成死循环。**记为待评估项。**

**③ 硬路径上的其余确认器全部核实为正确。**
`git_status` 的 `branch` 与 `status` 两条 `run_cmd` 分支都用 `?` 传播错误
（`git_tools.rs:40,45`），`git_diff` 同理（`:68-70`），因此在非 Git 仓库里
exit code 非零 → `Err` → 不进 `is_global_verifier`。
`run_tests` / `build_project` / `build_generic` 已在前一条记过。
`db_query`（0 行是合法的 schema 确认）、`secret_get`（掩码值仍确认存在）、
`read_runtime_logs`（部署后暂无日志是合法观测）按原样保留——
这三个若按「必须有非空结果」收紧都会造成误阻断。

### 顺带自检：上一轮新增的判据不会被输出落盘改写

结论感知判据读的是 `item.output`，而 `guards.rs::post_spill` 在输出超过
`SPILL_THRESHOLD = 20_000` 字符时会把结果**换成 head+tail 预览**——
如果触顶，`run_lint` 的「错误 (error)：N」和 `check_code` 的规则分组都会被抹掉，
新判据就会永远匹配不上、把必需步骤变成永久阻断。核对两侧上限后确认**不会触发**：
`check_code` 被 `scanner::cut` 封在 15_000，`run_lint` 报告最多列 50 条问题，
都远低于 20_000。

同时确认「输出被截断一律不算通过」这条**不是理论防御而是承重的**：
`scanner::RULES` 里唯一的高危规则 `hardcoded-secret` 排在**第三位**
（前两条 `debug-log`、`todo-mark` 都是 Info），
在 console.log / TODO 较多的仓库里，它确实会被 15_000 的**头部**截断整段吃掉。
所以「没看到高危」在这种仓库里真的可能是假的，该拦。

### 跨文件写入不原子：工具报失败，磁盘已改了一半

`lsp_rename` 一次可改十几个文件（跨文件 WorkspaceEdit），旧实现是
「边校验边落盘 + `?` 短路」：第 N 个文件被写前门禁拒绝时，前 N-1 个**已经在磁盘上**，
而返回值是 `Err`，`lsp_rename` 原样把错误抛给模型。

实测（临时探针，跑完即删）：一个跨 2 文件的 WorkspaceEdit，第二个是 `.env`，
返回 `Err("写入被安全策略拒绝（secrets_env 不变式）…")`，
而第一个文件的内容已从 `let AAA = 1;` 变成 `let BBB = 1;`。

**为什么没被立刻发现**：错误文案本身完全正常，读日志只会看到一条清晰的拒绝说明。
模型侧更难察觉——它读到「写入被安全策略拒绝」，合理地推断整个重命名没发生，
于是重试或转去做别的事，磁盘上却是半改状态（`a.ets` 改了、`b.ets` 没改、
符号引用对不上），而没有任何一处告诉它发生过这件事。

同族对照：`fs_tools::commit_prepared_edits`（`multi_edit`）早就做对了——
提交前全量基线核对 + 逐个写入失败即回滚 + 整批成功才登记 undo。
**同一个仓里两套写提交语义，弱的那套在 LSP 侧。**

**已修**（`lsp_client.rs`）：

- `apply_workspace_edit` 改成两阶段——`prepare_text_edits`（读原文件 → `check_write`
  不变式 → 合成候选文本 → `validate_code_mutation` 候选门禁，**不落盘**）全部通过后，
  才由 `commit_text_edits` 统一落盘。任一文件过不了门禁，整批一个字节都不写。
- 提交阶段复用 `fs_tools::verify_write_baseline` / `write_candidate_with_restore`
  两个既有原语（提为 `pub(crate)`），补上提交前基线核对与失败回滚，
  **不在 lsp_client 里另写一份弱版本**。顺带让 LSP 写过的文件进入 `stamp_put`
  写指纹缓存，之后的 `edit_file` 能正确识别外部改写。
- undo 快照改为整批成功后才登记，与 fs 侧一致（事务失败不留不可用的撤销记录）。

同 URI 在 `documentChanges` 里出现多次时（协议允许，且后一条本应作用于前一条之上）
按顺序 rebase 到同一条待写记录上，undo 快照仍保留磁盘原文——
这与旧实现「每次重新读已落盘磁盘」的行为等价，但去掉了重复文件计数。

### 假成功：跳过的编辑被算成「完成」

`apply_workspace_edit` 静默跳过它不认识的 `documentChanges` 条目
（只含 `RenameFile` 操作的响应就属于这一类），返回 `(0, 0, 0)`；
而 `lsp_rename` 无条件拼出「重命名完成（涉及 0 个文件，+0 −0 字符），
建议随后用 check_code 或构建验证无残留引用」。

**实测**：只含 `RenameFile` 的 WorkspaceEdit 与空对象，都得到 `Ok((0, 0, 0))`。

危害不在于「没改名」，而在于把它**包装成成功并给出下一步建议**——
模型会照着去跑一轮 `check_code`/构建，或更糟：认定符号已改名，据此写出错误的引用。

`lsp_format`（`edits.is_empty()` → 「文件已符合格式」）与
`lsp_code_action`（`else` → 「没有可应用的编辑」）本来都有这个分支，**只有 `lsp_rename` 漏了**。
已补：0 个文件时如实返回「未写入任何文件」并说明可能只收到 RenameFile 操作。

**复发模式**：批量操作里，「跳过了 N 条」和「完成了 N 条」是相反的结论，
但如果跳过是静默的 `continue`，两者在返回值上长得一模一样。
**凡是有 `continue`/`if let Some(..) else { continue }` 跳过条目的批量路径，
都必须能在返回里区分「全部处理」与「部分/全部跳过」。**
判据不是「这两份清单不一样」，而是——**漏掉之后会不会出事**：会，因为失败被读成了成功。

### 依赖清单把「读不到」当成「没有」：许可证检查伪装成合规通过

`license_check` 解析四个候选依赖文件（`oh-package.json5` / `oh-package-lock.json5` /
`Cargo.toml` / `pyproject.toml`），四处**全部**写成
`if path.exists() { if let Ok(text) = read_to_string(path) { … } }`——
读失败与不存在产生完全相同的可观察后果：「没扫到依赖」。两种失效模式：

- **四个全读不动** → `findings` 为空 → 返回「未发现可扫描的依赖文件」。
  一次权限错误 / 非 UTF-8 / I/O 故障，被报成**本项目没有三方依赖**。
- **部分读不动** → 报告照常输出「共 N 个依赖」与 ALLOW/DENY/待查汇总，
  只是被跳过的文件整个消失，**DENY 数偏低且无任何提示**。

这是许可证合规检查，也就是「三方插件仅限 BSD-3/MIT/Apache2/ISC/Unlicense/CC0」
那条红线的执行点，把**检查没做成**说成**检查通过**，比误报更危险。

**已修**：新增 `read_dep_file`（三态：不存在 / 读到 / 存在但读失败），
读失败的原因逐条记入 `unreadable`。全部读不动时如实返回「⚠️ 扫描不完整，
不能据此认为本项目没有三方依赖」；部分读不动时在报告头部挂 ⚠️ 横幅并点名文件、
注明「DENY 数可能偏低」。

**顺带查出的第二个缺陷（同一函数）**：oh-package 解析对**每一行**都调 `parse_dep_line`，
而 `parse_dep_line` 的 JSON5 分支是 `"key": "value"` 通配，于是
`"dependencies": {` 这个**容器键**被解析成一条名为 `dependencies`、版本为 `{` 的依赖。
标准 `oh-package.json5` 必带该键，**每个鸿蒙工程都会稳定多出一条幽灵依赖**，
虚高依赖总数与三项汇总。实测报告里出现过 `| ohpm | dependencies | { | ⚠️ 待查 |`。

已改为块状态跟踪：只认 `dependencies` / `devDependencies` **块内部**的叶子条目，
用大括号深度区分顶层容器键与嵌套容器（`overrides` 里的子对象不会被误收）。
注意深度判据是 `depth == 1` 而不是 0——根对象自身已经把计数抬到 1；
第一次写成 `depth == 0` 时整份依赖清单直接清空，靠探针才发现（报告变成「扫描不完整」）。

**同族第二处**：`vuln_scan` 三个清单文件（`oh-package-lock.json5` / `Cargo.lock` /
`requirements.txt`）用的**完全相同**的写法，后果比许可证那条更重——
`found.is_empty()` 时输出的是「**✅** 未发现已知漏洞」。

也就是说：三个清单全部存在却读不动 → 一次**根本没跑**的依赖漏洞扫描，
拿到一张盖了 ✅ 的安全合格证。✅ 恰恰是让人停止继续排查的那个符号。

已复用同一 `read_dep_file`（不另写第二份）：新增 `scanned_sources` 记录真正读到并
比对了哪几类清单。`found.is_empty()` 分支据此三分：

- 一类都没扫成 → 「❌ 未扫描任何依赖清单…本次**没有得出任何安全结论**」
- 扫过且干净 → 「✅ **已比对 N 类清单（cargo / …）**，未发现已知漏洞」，
  ✅ 从此只在真的扫过时才出现，并注明比对了什么
- 部分清单读不动 → 报告头部 ⚠️ 横幅点名文件并注明未纳入比对

顺带删掉 `Cargo.lock` 解析里一个**纯空转的死循环**（调用 `extract_toml_string`
后 `let _ =` 丢弃，注释自称「简单占位解析（实际逻辑在下方）」）。
它不产生任何效果，却长得像一段解析——与「文档里的权威措辞不构成证据」同类：
留着一段假装在解析的代码，会让下一个读代码的人以为解析已经做过了。

**复发模式**：`if path.exists() { if let Ok(..) = read(..) { … } }`
这个形状把「没有」和「读不到」压成同一个状态。凡是**检查类**工具（安全扫描、
合规检查、校验器）用了它，输出就失去了证否能力——
**检查没做成必须是一种可区分、可上报的状态，而不是沉默的空结果。**
判据不是「这样写不优雅」，而是——**漏掉之后会不会出事**：会，因为失败被读成了通过。
**带 ✅/「通过」/「无问题」措辞的检查工具尤其危险**：失败被读成通过之后，
那个符号本身就成了终止排查的理由。

### 「跑成功且什么都没发现」还有第三种可能：它压根没读到

前面两处解决的是「工具返回 `Ok`，但结论其实不好办」。本条是**更靠后的一层**：
工具返回 `Ok`、结论也确实干净，但**结论的覆盖面小于它字面声称的范围**——
因为有一批文件/目录没被读到，且报告从不说明。

`check_code` 就是这个位置：它是 `verification_planner::plan` 在非 ETS 代码变更时
安排的**必需**验证步，也是 `RULES` 里唯一高危规则 `hardcoded-secret` 的唯一载体。
而它的收集层 `scanner::walk` 有三处静默丢失，外加两处静默丢弃：

| 位置 | 写法 | 丢了什么 |
|---|---|---|
| `walk:50` | `let Ok(entries) = read_dir(dir) else { return }` | 读不到的目录**整棵子树**静默放弃 |
| `walk:51` | `entries.flatten()` | 读失败的目录条目逐个丢弃 |
| `walk:59` | `e.metadata().map(…).unwrap_or(false)` | 取不到元数据 ≡「文件过大」，两类混成「跳过」 |
| `check_code` | `let Ok(text) = read_to_string(f) else { continue }` | 读不到内容的文件跳过，**而 `scanned` 在 `continue` 之前已自增** |
| `check_code` | `files.iter().take(300)` | 超上限的文件静默截断，输出只说「扫描 300 个文件」 |

最后一条尤其恶劣：**读不到的文件被算成「已扫描」**，
于是「扫描 N 个文件」高估真实覆盖面，「未发现规则命中，代码整体较整洁」高估结论强度。

**为什么绿灯全在**：闸门侧当时**看起来是对的**——`completion` 对 `check_code`
已经是结论感知的（要求给出命中数、拦 `SCAN_TRUNCATED`、只把高危/中危当阻断项），
唯独从没有一个「这次到底覆盖了多少」的输入。闸门能判断「报告说了什么」，
不能判断「报告没说的东西」。

**已修（两半都要，缺一无效）**：

- *工具侧*：`collect_src_files` 回 `CollectedSrcFiles { files, unreadable_dirs, metadata_failed }`，
  不再只回 `Vec`；`check_code` 另计 `read_failed` 与超上限的 `dropped_by_cap`。
  任一非零即在报告头部写 `{SCAN_INCOMPLETE}` 稳定标记并逐条列明缺口，
  且把「代码整体较整洁」降级为「已扫描到的文件中未发现规则命中（覆盖不完整，见上）」。
  标记常量定义在 `scanner::SCAN_INCOMPLETE`，闸门侧**引用同一个常量**，不复制字串。
- *闸门侧*：`completion` 新增一臂——输出含 `SCAN_INCOMPLETE` 即 `passed = false`，
  理由里带上缺口明细。口径与既有的 `SCAN_TRUNCATED` 臂一致：
  **「没看到高危」不等于「没有高危」，读不到的文件上同样看不到。**

**验证（临时探针，跑完即删）**：350 个干净文件 → 报告含
「⚠️ 扫描覆盖不完整：50 个文件超过单次 300 上限未扫描」，闸门 `passed=false`；
删到恰好 300 个 → 无标记，闸门 `passed=true`。
**对照组同样重要**：闸门只在真有缺口时拦，不误阻断正常扫描。
`unreadable_dirs` / `metadata_failed` / `read_failed` 三条分支走的是同一段
`gaps` → 标记 → 闸门代码，但本机 Windows 上无法可移植地构造不可读目录，
**这三条未做端到端实测，仅有代码级依据**——按本仓纪律记为 `待补`，不写成已验证。

**同链第三处（同一轮继续扫出来的）**：`secret_scan` 有**五处**同类丢弃，
而且 `walk_conf` 是 `walk` 的**第二份拷贝**——上一轮刚把 `walk` 修好，
它仍保持旧形态。这正是「修完一处 ≠ 修完这条链」的当场演示：
`walk` 与 `walk_conf` 唯一的差别只是文件名过滤谓词，遍历逻辑本该只有一份。

`secret_scan` 也是承重的：它出现在 `capabilities.rs:112` 的 **commit 前推荐流程**
（`git_status → git_diff → review_changes → run_tests → build_project → secret_scan → git_commit`）里，
而它的结论是「未发现疑似密钥，**安全状况良好**」。

已收敛：`walk` 泛化为 `walk_filtered(dir, max_size, keep, out)`，
`collect_src_files` 与配置文件扫描都走它；缺口披露抽成 `CoverageGaps::render(cap)`
共用（两条结论路径若各写一份披露格式，改一处必然漏另一处）；
上限提为 `SCAN_FILE_CAP` / `SECRET_FILE_CAP` / `SECRET_CONF_CAP` 三个具名常量，
不再散落字面量。覆盖不全时「安全状况良好」降级为
「已检查到的文件中未发现疑似密钥（覆盖不完整，见上）」。

**验证（临时探针，跑完即删）**：520 个干净文件 → 披露
「⚠️ 扫描覆盖不完整：20 个文件超过单次 500 上限未扫描」；
删到 500 → 无标记，「安全状况良好」保留（不误报）。

**复发模式**：闸门只对「工具报告里写了什么」敏感，对「工具没写什么」天生不敏感。
凡是结论型输出（「无命中」「通过」「整洁」「未发现问题」），
**报告本身必须同时携带自己的覆盖面**（读了多少、漏了多少、截断在哪），
否则一个诚实的工具 + 一个尽责的闸门，仍会合力产出一张覆盖面不足的合格证。

### 先弹栈再判越界：撤销能力被永久销毁，还告诉用户「没有可撤销的修改」

`undo_edit` 按会话可见根过滤（跨项目快照不可恢复）。旧写法是**先 `pop_undo`
再判越界**：

```rust
let Some(s) = undo::pop_undo(conversation_id) else { break };
if !allowed { continue; }   // 快照已经出栈了，这里再丢就是永久销毁
```

三重后果叠加：
1. 越界快照被 `pop` 出来、`continue` 丢弃，**不可逆**——用户那次撤销能力凭空消失，
   重新绑定对应工程后也再也回不到改前状态。
2. `restored.is_empty()` 分支统一返回「没有可撤销的修改（**本会话尚无 Agent 文件写入记录**）」。
   栈里明明有记录，只是这次被判越界——这句话对调用方是**不实描述**，
   而模型会据此认为「这个会话没写过文件，撤销不了」，改用别的方式（比如直接手改磁盘）。
3. 跳过了几条，完全不可见。

**已修**：在 `undo` 新增 `pop_undo_filtered(conversation_id, count, keep)`——
只弹出通过筛选的，**跳过的按原顺序留在栈内**（栈是 FIFO 淘汰 + LIFO 弹出，顺序必须保持）。
`undo_edit` 改为：先用 `peek_at` 收集被跳过的路径（不动栈），
再用 `pop_undo_filtered` 取该恢复的。两种空结果分开报告：
全部越界 → 如实说明「未撤销任何修改…这些快照**仍保留在撤销栈中，未丢失**，
重新绑定对应工程后可直接再次 undo_edit」并点名路径；
部分越界 → 在成功报告尾部追加同样口径的跳过清单。

**验证（临时探针，跑完即删）**：快照路径在另一个根下、`roots` 只给当前根 →
修复前 `undo_count` 归 0 且回复「尚无 Agent 文件写入记录」；
修复后 `undo_count` 仍为 1、回复说明跳过且未丢失，
把根换成包含该路径的根后再调 `undo_edit` 正常恢复。

**复发模式**：`pop`/`take`/`drain` 这类**破坏性取值与校验的先后顺序**本身就是缺陷来源。
**先校验再取出**；实在要先取出（比如筛选依赖栈内容），就必须保证
**被否决的条目放回原处**并**如实报告跳过了几条**。
判据：**破坏性操作不可逆 —— 被否决的条目绝不能已经离开它的容器。**

**同族扫描结论（范围内只有这一例）**：`pop()` / `take()` / `drain()` / `remove()`
全仓扫过，其余命中分三类且都正确——FIFO 容量淘汰（`agent_board` / `ask` /
`crash` / `diagnostics` / `TaskLedger` 的 `drain(..excess)`）、
`Option::take()` 移动惯例、`stdin/stdout.take()` 取子进程管道。
**「仓里只有一例」本身是有用信息**：说明 undo 那处是异类而非普遍写法，
不必按系统性缺陷上报。

**配套的收口纪律**：修完之后 `undo::pop_undo`（无条件弹栈）只剩自己的单元测试在用，
生产路径全走 `pop_undo_filtered`。已**删掉** `pop_undo` 并把原有测试改走存活的原语——
留着它等于给同一个破坏性操作开出**第二条没有护栏的路**，下一个写代码的人
看到「有个 pop 函数」就会顺手用它，把刚修好的 bug 原样带回来。
（既有的三个 undo 测试属删除函数后的机械改写，不是新增测试。）

### 视觉闭环：两份手写名单已经漂移，多标记时解析出垃圾路径

截图视觉闭环靠工具输出里的 `[VISION_IMAGE: <路径>]` 标记，把图片编码成
多模态 data URL 附给下一轮模型请求。这条链上有**三处独立缺陷**：

**一、同一份名单被手写两遍，且已经漂移。**

- `tools/guards.rs::NO_SPILL_TOOLS`（3 个：`take_screenshot`/`verify_ui`/`run_ui_flow`）
  决定「输出不许被 `post_spill` 截断」——标记被截掉，图就丢了
- `commands/chat.rs` 的消费方分支（4 个：上述三个 + `view_image`）
  决定「剥离标记并把图编码进模型视野」

`view_image` 同样产出标记，却不在免落盘名单里；而 `chart_extract`
（`doc_tools.rs` 末尾循环，**一次可发多个标记**）压根不在消费方名单里。
后果：`chart_extract` 的输出文案白纸黑字写着「随下轮请求进入模型视野」，
而消费方从不认它的标记——**该功能对图表是彻底死的**。

**二、解析函数的前提被真实调用方违反。**
`extract_vision_image_path` 先 `find` 定位**第一个**标记，再用 `rfind(']')`
找**整段输出里最后一个** `]`。它注释里写明理由与前提：
「Windows 合法文件名可含 `]`（如 `C:\a[b]\shot.png`），标记固定位于行尾」。

`rfind` 的理由成立（实测 `C:\a[b]\shot.png` 解析正确），
但**「标记固定位于行尾」只对单标记工具成立**。`chart_extract` 在循环里发多个，
于是 `rfind` 一路找到最后一个标记的收尾括号，返回横跨两个标记的垃圾路径。
实测旧实现：`"...[VISION_IMAGE: C:\a\c1.png]\n[VISION_IMAGE: C:\a\c2.png]"`
→ `"C:\a\c1.png]\n[VISION_IMAGE: C:\a\c2.png"`。

这条**当时打不到**（`chart_extract` 不在消费方名单里），但它是一颗上膛的枪：
任何人把 `chart_extract` 加进名单就会立刻踩到。

**三、标记格式有五份手写副本**（`doc_tools` ×2 / `mod` ×2 / `test_tools`）。
标记格式是生产者与消费者之间的契约，抄五份就一定会漏改。

**已修（收敛成单一真源）**：

- `tools/mod.rs` 新增 `VISION_MARKER` / `vision_marker(path)` / `VISION_MARKER_TOOLS`
  三件套。**五个生产方全部改调 `vision_marker()`**；`NO_SPILL_TOOLS` 与
  `chat.rs` 的消费方分支都改为引用 `VISION_MARKER_TOOLS`。
  名单里写了每项对应哪个函数，以及校验命令
  `rg -n 'vision_marker\(' src-tauri/src/agent/tools/`——**新增工具只改这一处**。
- `extract_vision_image_path` → `extract_vision_image_paths`（返回 `Vec`），
  改为**按行**取该行最后一个 `]`：既保住 Windows 路径含 `]` 的正确性，
  又不受后续标记干扰。
- 消费方改为循环附加，受单任务 4 张上限约束；**只剥离已附加的标记**，
  未附加的保留原文并列出路径（模型可用 `read_file` 兜底）。

**单标记工具的行为逐字不变**（三个既有生产方都只发一个末尾标记），
这一点是本次改动的主要不回归保证。

**验证（临时探针，跑完即删）**：单标记 → `["C:\a\shot.png"]`；
路径含 `]` → `["C:\a[b]\shot.png"]`；多标记 → `["C:\a\c1.png", "C:\a\c2.png"]`
（修复前是横跨两标记的垃圾串）；无标记 → `[]`。

**复发模式**：**「哪些工具参与某条结构化协议」这件事一旦靠手写名单维护，
就一定会漂移。** 而且漂移方向取决于每份名单服务于哪个消费方：
生产方名单漏一个 → 标记被截断；消费方名单漏一个 → 功能静默失效。
正确做法是**先找消费方（谁在解析），再找生产方（谁在输出），
把并集写成一份常量**，两个消费方都引用它。
**注释里写明的「前提」也要按真实调用方逐个核对**——
`rfind` 的理由是对的，错的是那条「标记固定位于行尾」没有覆盖多标记形态。

### 已核实并证伪（风险面缩小）

- **工具响应缓存会返回写入前的旧内容** —— **不成立，且设计上就免疫**。
  `tool_cache.rs` 的文档写着「任意成功的非缓存工具调用都会由调用方清空缓存」，
  看起来是「清单驱动」，实读 `tools/mod.rs:1436-1440` 发现它**根本不看工具名**：
  `if is_ok && is_cacheable(name) { put } else if is_ok { clear() }`——
  任何一次成功的非缓存调用都**整体清空**，与「哪些工具会改文件」这份清单无关。
  缓存键也含 `project_id` + 有效根目录范围 + 参数全文（`key()` 三者都 hash）。
  **这是本项目里对该族最稳的一处写法，值得作为模板**：宁可整体失效，
  也不维护「工具 → 资源」依赖图。上一族六处缺陷全部源于维护那份依赖图。

- **符号索引在 `lsp_rename` 后残留旧符号** —— **不成立**。
  `tools/mod.rs:1445-1465` 的增量失效清单确实漏了 `lsp_rename`
  （只列 `write_file|edit_file|delete_file|move_file|copy_file|multi_edit`），
  看起来会让索引存旧数据。**但兜底是承重的**：文件指纹用
  **mtime 纳秒 + 字节数**（`symbol_index.rs:1606`，注释明写「NTFS 精度 100ns，
  可察觉同秒内改写」），而 `index_project_cached` 在热路径上每次查询都会跑
  （`scanner.rs` 三处、`commands/index.rs` 四处、`repo_watcher` 两处）。
  符号改名恰好是最容易「同秒 + 同字节数」的操作（`foo`→`bar` 长度不变），
  纳秒精度仍能抓住。
  **结论：这里只是性能损失（下次查询全量重扫该文件）而非正确性问题，故不改。**
  判据仍是那条：「漏掉之后会不会出事」——不会出事就不动。

- **ToolSpec 承诺的 `undo_edit` 回退是空头支票** —— **不成立，三个工具全部兑现**。
  `format_file` / `lsp_format` / `lsp_rename` 的描述都写着「可 undo_edit 回退」。
  全仓 `undo::snapshot` 只有 6 个调用点，逐个确认覆盖：
  LSP 侧全部经 `lsp_client::apply_workspace_edit`（4 处调用）→ `apply_text_edits`
  → 落盘前 snapshot（`lsp_client.rs:992`）；fs 侧经 `write_candidate_with_restore`
  （3 处）与 `commit_prepared_edits`（`multi_edit`，1 处）。
  `format_file` 的 `dry_run=true` 分支明确标注「不落盘、不入 undo」，与描述一致。
  **「共享辅助函数 + 在里面统一落快照」是这类承诺的正确实现方式**，
  比在每个工具里各写一遍更不容易漏。

- **评测期望产物在断言前被刷新** —— **不成立**。`ci_baseline_gate` 的实际顺序是
  run → 读基线 → `compare_with_baseline` → `assert!` → 才 `std::fs::write` 落基线；
  `assert!` panic 会让测试在写之前中止，CI 里 IN == OUT 所以回退run无法自我祝福。
  固定 fixture 由 `include_str!` 编译进二进制，**仓库里没有任何写入它们的代码路径**，
  `git log -p` 也显示 5 次改动全是新增、没有一次把 `expected` 改成匹配既有运行。
  另有更强的两道：16 个内核场景的 `expected` 被 `governance.rs` 钉到生产治理函数
  `reliability_disposition()` 上（不是录制的 transcript），未知 id 落 `"unhandled"`
  而 `reliability_gate` 要求 `score == 1.0`，无法通过。
  **残留的窄口**（比原假设弱得多）：没有哨兵校验防止**人**手改 fixture 去匹配坏运行；
  基线缺失时按设计跳过比较（CI 缓存被清空会静默关掉回归门禁）。

- **大模块错误分类被统一 catch 抹平** —— **大部分不成立**。
  `capability_broker.rs` 的审计 `reason` 是 snake_case 机器码而非散文
  （`approval_scope_or_snapshot_failed` 等 6 种），`TOOL_POLICY_BLOCKED` 真实可达，
  `KernelRunTermination` / `SandboxRunStatus` / `RecoveryAction` / `ToolErrorCategory`
  都是真枚举，`ToolError.raw` 保留原串使沙箱前缀仍可读。全仓 `map_err(|_| …)` 扫描只命中
  `strip_prefix` / `from_utf8` / 锁中毒 / `timeout` 这类内部错误本就不带类型的地方，
  **没有丢弃已类型化错误种子的 `map_err(|_|)`**。唯一的真实缺口就是上面已修的沙箱那条。

### 已核实，成立，但边界比原记录窄

- **UI 与 headless 的循环分叉** —— **成立，且已开在文档里**，不是隐藏地雷。
  `docs/HEADLESS_AGENT_DRIVER.md` 的切片表 A—BA 共 53 项**全部 COMPLETED**，
  其中 AG（共用 IO 循环壳）与 AQ（headless 生产端口迁移）已落地。
  实读代码后的准确边界是：**决策面已统一，IO 驱动未统一**。

  | | UI (`chat.rs`) | headless (`headless_driver.rs`) |
  |---|---|---|
  | executor 实例 | `KernelIoRunLoop::with_started`（:6418） | `KernelIoRunLoop::with_started`（:963） |
  | 决策方法 | 直接调（经 Deref 到同一 executor） | 直接调 |
  | `impl KernelIoPort` | **无** | 有（:547） |
  | 由 `KernelIoRunLoop::run` 驱动 | **否** | **是** |

  也就是说 router / governor / 计数 / 终止归因 / 最终快照本来就是同一套，
  分叉只在**谁来驱动 IO 循环**：桌面用 `desktop_round` 系列 adapter 函数自己转，
  headless 走统一 run-loop。切片表里没有「桌面生产端口迁移」这一项。

  另注：`harmony-agent` bin 在 `required-features = ["eval-cli"]` 之后，
  驱动的是 headless/ProcessAgentDriver，**所以 CI 门禁量的是这条统一路径**，
  未被门禁覆盖的是桌面侧自己转的那圈。风险点是具体的，不是"整体未统一"。

  （本条最初依据文档 L16「尚未由同一个 run-loop executor 驱动」写成"整体未统一"，
  那句话有歧义；已按代码改正 L16 的措辞并收窄本条。）

### 已核实，决定不改

- **MCP「测试连接」**：空 `tools` 数组仍返回「连接成功 ✓ 未返回工具列表」。
  文案本身如实披露了未返回工具列表，传输失败也走正常错误路径，
  且**零工具的 MCP server 是合法配置**——把空列表判成失败是错的。
  真正"该报错没报"的是 agent 侧的 `mcp_client.rs`：`tools/call` 空文本已判 `Err`，
  `tools/list` 空数组返回 `Ok(vec![])`，但该结果对 Agent 无害（就是没有工具可调）。

### 外部对照

DeepSeek Harness `docs/postmortem/` 四篇披露的复发模式，与本项目风险面重合度最高的两条
（测试手工注入依赖导致无法复现、宽泛错误包装盖住结构化码）均已核实：
后者在本项目基本不成立，前者的近亲（UI/headless 路径分叉）成立且项目自认。

---

## 模板

```markdown
# <复发模式 slug>

<一段话说明这一类缺陷的通用形态，不带具体人名/日期。>

## 现象

<观察到什么。>

## 为什么没被立刻发现

<绿灯在哪：哪条测试 / 哪个指标 / 哪轮 review 放过了。>

## 机制

<精确到代码路径或数据流。信 trace，不信 theory。>

## 复发模式

<这一类问题的通用规则。>

## 钉住了什么

<哪条机械检查 / 不变量 / 清单条目拦住了它。
「会更小心」不算答案。>
```
