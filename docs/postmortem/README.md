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
