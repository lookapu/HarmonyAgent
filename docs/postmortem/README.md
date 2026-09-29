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

### 已核实，风险真实但项目自认未统一

- **UI 与 headless 的循环分叉** —— **成立，且已开在文档里**，不是隐藏地雷。
  `docs/HEADLESS_AGENT_DRIVER.md:16` 明写「它仍然不等价于 Tauri UI 的完整 Agent loop：
  两个 adapter 已共享关键策略组件，但尚未由同一个 run-loop executor 驱动」，
  `:474-476` 是未完成清单（流响应读取/停滞治理、消息历史、tool loop、reflexion、
  governance、recovery 待抽取）。策略层（`kernel_loop.rs` / `kernel_history.rs`）
  确实无 Tauri 依赖，共享是干净的；分叉只在**效果层**（事件、DB 写入、continue/break）。
  评测侧 `harmony-agent` bin 在 `required-features = ["eval-cli"]` 之后，
  走的是 headless/ProcessAgentDriver，**所以 CI 门禁量的是内核路径，不是桌面 UI 路径**。
  收敛方向已在 Phase B-A 的切片表里排好，无需重新设计。

### 已核实，仍未修（低影响，记录备查）

- **`ohpm` registry 仓库地址**：`ohpm_landscape.rs:364-366` 对非 2xx 返回 `Ok(None)`，
  与「该包没有 repository 字段」不可区分。唯一调用方是 Tauri command，
  **没有 agent 工具用它**，影响面限于一个 UI 展示位。
- **MCP「测试连接」**：空 `tools` 数组仍返回「连接成功 ✓ 未返回工具列表」——
  文案本身如实披露了未返回工具列表，且传输失败会正常报错，**不打算改**
  （零工具的 MCP server 是合法配置）。
- **文档目录树空值**：`harmony_doc_api.rs:89-95` 在 `code:0` 但目录为空时返回
  `Ok(vec![])`，随后 `harmony_api_ref.rs:1090` 静默退回 slug 猜测且不记错误。
  这是 0002 那类形状的残留，但**行为与改为 `Err` 相同**（`Err` 也走 slug 猜测回退），
  差别仅在于是否留痕。

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
