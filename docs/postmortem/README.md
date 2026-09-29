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

  **这一族已数到第六处，且每一处都在不同的下游**：
  `acceptance` 变更集 → `chat.rs` 变更清单 → `postconditions` 确认器 →
  `execution_loop` 验证器 → 审批 `first_write` → **`context` 事实失效**。
  共同点始终不变：不报错、不崩、测试全绿，只是各自安静地按自己的理解工作。

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

### 已核实并证伪（风险面缩小）

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
