# 对话流畅性审计（2026-10-22）

> 目标：让对话像 ChatGPT / Trae / WorkBuddy 那样自然流动，消除「卡住」。
> 方法：三路并行深度扫描（前端流式生命周期 / 前端阻塞交互门 / 后端主循环挂起点），
> 关键因果链已人工核验（标注 ✅ 的为亲自读码确认，未标注的为子代理报告结论）。

---

## 一、全链路卡点总览

| 严重度 | 数量 | 典型表现 |
| --- | --- | --- |
| 🔴 高（硬卡死 / 长时静默） | 8 | 长回答被截断、界面静默数分钟、发不出消息 |
| 🟠 中（偶发卡顿） | 14 | 内容消失、停止不生效、切会话丢卡片 |
| ⚪ 低（体验瑕疵） | 6 | 提示不出现、角标残留 |

---

## 二、🔴 高危问题（按修复收益排序）

### P0-1 流式请求共用 120s 总超时，长回答必被截断 ✅
`src-tauri/src/utils/net.rs:87`（配套 `chat.rs:8335 / 10197 / 10452`）

```rust
// 总超时 120s：流式响应读取也受此约束
let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(120));
```

这是最根本的一条：**按整段响应计时的总超时被套在流式请求上**。
后果链：
1. 模型持续输出超过 120 秒（长代码、长思考、慢 Provider）→ `stream.next()` 返回 `Err`；
2. 本轮直接判失败，用户看到已生成的内容拦腰断掉；
3. 上层退避重试 → **从头重新请求**，已生成内容重复消耗 token。

对比 ChatGPT / Trae：它们用的是 **idle 超时**（多久没有新字节才判死），不是总时长。
本项目其实已经有正确机制（`agent_kernel.rs` 的 `silent_timeout=60s` 停滞线），
但被这层 reqwest 总超时抢先触发，等于「有 idle 看门狗，却被总超时先杀掉」。

**修复**：流式客户端不设 `timeout()`，改 `.connect_timeout(30s)` + `.read_timeout(60s)`
（reqwest 0.12 支持 `read_timeout`）；已有 kernel 停滞线继续做业务级判死。

---

### P0-2 重试阶梯过长且全程静默，用户干瞪眼 10 分钟 ✅
`agent_kernel.rs:738-769`、`kernel_loop.rs:27-37,292-303`

```
KERNEL_STREAM_SILENT_TIMEOUT   = 60s    // 静默判死
KERNEL_STREAM_REASONING_GRACE  = 180s   // 纯思考封顶
KERNEL_MAX_STREAM_REPLAYS      = 5      // 冻结重放
KERNEL_MAX_INTERRUPT_RETRY_ROUNDS = 3   // 中断续写
KERNEL_MAX_CONTINUATION_ROUNDS = 8      // 截断续写
```

每一层都有上限（不会死循环 ✅），但**层与层串联**：最坏情况 60s × 5 次重放 ≈ 5 分钟无产出，
叠加 120s 总超时后单轮可空转 10 分钟。这期间前端：
- 无「第 N 次重试」提示；
- 心跳每 5s 刷新 `lastDeltaAt`（见 P0-3），导致连「模型思考中」的静默提示都不出现。

用户感受就是**一直转圈，不知道在干什么**。

**修复**：
1. 重放/续写时通过 `chat-stream` 发一条 `notice` 事件（「网络中断，正在第 2/5 次重试…」），前端渲染为居中小字；
2. 给整个重试阶梯加**总时限**（如 6 分钟），到点直接 `chat-error` 并保留已生成内容；
3. 重放次数从 5 降到 3，`first_reasoning` 宽限从 180s 降到 90s。

---

### P0-3 两套看门狗互相掩盖 ✅
`src/stores/slices/chatSlice.ts:313-395`、`471-492`；`src-tauri/src/utils/task_registry.rs:30,38,40`

后端阈値：`TASK_STALL_MS=480s`、`STREAM_STALL_MS=300s`、`STOP_GRACE_MS=40s`，每 5s 巡检 ✅
前端阈値：无内容 4 分钟 / 有内容 8 分钟，30s 巡检 ✅

问题：心跳处理器**无条件**刷新前端活性：
```ts
listen('chat-heartbeat', (event) => {
  if (bucket) { setBucket(id, { error: null, errorDetail: null, lastDeltaAt: Date.now() }) ; return }
```
监管线程每 5s 必发心跳 ⇒ 前端 `now - lastDeltaAt` 永远 < 阈值 ⇒ **前端看门狗只在监管线程死掉时才生效**。
于是「模型不回字、监管线程正常」这个最常见的卡死场景，前端完全不兜底，
只能等后端 300s 判死 + P0-2 的重试阶梯 → 合计最长十几分钟。

**修复**：心跳不再刷新 `lastDeltaAt`，改为单独维护 `lastHeartbeatAt`；
前端看门狗的判据拆成两条：
- 监管线程死（`now - lastHeartbeatAt > 60s`）→ 立刻置错；
- 监管线程活着但无产出（`now - lastDeltaAt > 120s`）→ 显示可感知的「仍在等待 / 第 N 次重试」而**不是**静默转圈。

---

### P0-4 审批 / 提问遮罩无出口，且可能根本不出现 ✅(路径) / 子代理(行为)
`Home.tsx:7300-7412`（工具审批）、`Home.tsx:7247-7297`（ask 提问卡）

- 遮罩是 `fixed inset-0`，**没有关闭按钮、没有 Esc**，直接盖住停止按钮和侧栏；
- 只能点「允许」或「拒绝」；
- 一次只显示队列首项，无「全部批准」、无待处理计数角标；
- 若 `isCurrent` 为假或事件早于监听注册（WebView 重载）→ **弹窗不出现**，后端照样等 5 分钟后静默拒绝。

对比 ChatGPT/Trae：审批是**内联在消息流里**的卡片（可滚动、可忽略、可批量），从不全屏遮挡。

**修复**：
1. 审批/提问改为消息流内联卡片（`messageBlocks` 体系），移除全屏遮罩；
2. 加 Esc 关闭 / 「本次会话内全部允许」；
3. 恢复路径（`refreshPendingConfirmations`）补齐 `toolApprovals/pendingPlan/askCard` 回填；
4. 侧栏加统一的「N 项待你处理」角标。

---

### P0-5 僵尸流式桶导致 4 分钟内发不出消息 ✅
`chatSlice.ts:194-199`（出错时 `conversationId=null`）+ `:471-492`（心跳只清 error）+ `:1996-2000`（发送前置判定）

链路：
1. 出错 → `setBucket` 把 `conversationId` 置 null（`isStreaming=false`），但保留 `error`；
2. 心跳到达 → `setBucket({error:null})` → `isError=false` → `conversationId` 被**恢复成真实 id**，但 `startedAt` 仍是 null；
3. 此时 `existing && !existing.error` 为真 ⇒ `sendUserMessage` **返回 false**，用户点发送无任何反应；
4. 同时 `isStreaming=true` 但内容为空，停止按钮逻辑依赖桶状态，表现为「转圈但发不出、也停不了」。

> 说明：这一条是从代码路径推导的（`startedAt=null` 是确定的），**尚未在真机复现**。
> 但它解释了本机反馈过的「需要来回切会话才能再发」现象，值得优先兜底。

**修复**：`sendUserMessage` 的前置判定改为依据**后端任务是否存活**（`isStreaming && startedAt` 或新增 `runActive` 标志），
而不是「桶是否存在且无 error」；心跳恢复桶时同步补 `startedAt`。

---

### P0-6 子代理派生绕过契约超时 + 不打心跳 ← 会被自己的看门狗误杀
`chat.rs:13111-13129`（`spawn_agents` 直接调 `run_spawn_agents`）、`chat.rs:11861-11875`（cancel/registry 传 None）、`contracts.rs:222`

- 契约超时（20 分钟）**从未被强制执行**；
- 子代理运行期间不打心跳 ⇒ 连续 8 分钟无心跳被 `task_registry` **误杀健康任务**；
- `cancel=None` ⇒ 此阶段点「停止」无效，只能等 40s 宽限后强杀。

**修复**：`spawn_agents` 走统一的 `run_tool_in_lane`（享受契约超时 + 心跳 + cancel）；
`cancel`/registry 必须透传，不允许 `None`。

---

### P0-7 上下文压缩全程静默数分钟
`chat.rs:5808-5823`、`14473-14503`

压缩最多 3 × 90s 非流式请求 + 退避，**全程无 `chat-stream`、无心跳**（本轮心跳只在 `8700` 打一次）。
长会话触发压缩时，界面完全静默。

**修复**：压缩前后发 `notice`（「正在压缩上下文…」），压缩中使用独立心跳。

---

### P0-8 解析线程 10s 超时伪造 Done，丢弃工具调用
`chat.rs:10798-10817` + `10602-10618`

10s 未回 Done 就用快照造伪 Done，**丢弃 usage 与 native tool_calls**。
大输出时本轮工具调用被静默吞掉，主循环误判「无工具调用」而提前收尾。
这是「明明该动手却只回了一句话」的典型原因。

**修复**：超时造伪 Done 时至少保留已解析的 `tool_calls`；或延长阈值并改为可配置。

---

## 三、🟠 中危问题（体验摩擦，按主题归并）

### A. 流式展示失真
- `chatSlice.ts:360-371`、`:2114`：超时/停止用 `emptyStreaming()` 覆盖桶 ⇒ **已生成正文瞬间消失**，只剩错误卡。应保留 content/reasoning（`:194-199` 已经做对了，这两处没跟上）。
- `chatSlice.ts:2092-2136`：停止后只清桶，不动 `toolRuns/terminalEntries` ⇒ 残留 `running` 工具卡，而「停止工具」按钮随 `isStreaming=false` 隐藏 ⇒ 卡片永久卡在运行中。
- `Home.tsx:1884-1904` + `:1819`：自动贴底用 rAF 轮询 `stickToBottomRef`，内容一次增高 >120px 或上滑一帧就 `return` 且 effect 不重跑 ⇒ 本轮剩余输出不再跟随。流式输出「跑出可视区」的元凶。
- `messageBlocks.tsx:585-608`：120ms 节流的尾随 setTimeout 不补 ⇒ 终态切换时最后一段增量可能不渲染。

### B. 事件与状态时序
- `chatSlice.ts:497`：乐观桶 `runId=null`，`acceptsRunEvent(null, run_id)` 返回 false ⇒ **run-started 之前的首包 delta 被丢**（预算预警、首个 token）。应为 `runId==null` 时无条件接受并补写 runId。
- `chatSlice.ts:608-690`：`chat-done` 不清 `askCard` ⇒ 后端已超时继续跑完，遮罩仍永久盖住输入区。
- `chatSlice.ts:422,475`：`chat-done` 丢失后桶带旧 runId，下一轮 `run-started` 因 runId 不等 return ⇒ 新 token 全丢，UI 停在旧内容，最长卡 8 分钟。
- `chat.rs:4091-4113`：任务 panic 时只写 runtime 状态、**不 emit `chat-error`**，且 `registered.finish()` 先注销 ⇒ 看门狗失去兜底，后台窗口收不到终态。

### C. 恢复路径不完整
- `projectSlice.ts:132` vs `chatSlice.ts:1656`：`refreshPendingConfirmations` 用 `void` 触发，`openConversation` 立刻读取到空表；且该函数只过滤不回填视图 ⇒ 切回会话后审批弹窗/计划卡消失，需再切一次。
- `chat.rs:745`、`guards.rs:542`：审批/计划超时只写 `interactions` 不发结束事件，靠前端 30s 轮询才消失。

### D. 排队与计划模式
- `chat.rs:6986-7073`：同项目第二会话排队上限 600s，只发一条排队提示，排队期心跳正常 ⇒ 用户以为没反应。
- `chatSlice.ts:2161-2195`：`queueUserMessage` 无超时无重试，任务异常结束后消息永久 `queued`。
- `Home.tsx:2584,4094,2802-2804`：`/plan` 把 `plan_mode` 写进全局 localStorage 选项 ⇒ 此后每条消息都要人工批准，而开关藏在模型设置里，极易误认为卡死。应改为**消息级单次生效**。

---

## 四、⚪ 低危

- `streamingStatus.tsx:147-177`：静默提示以 `lastDeltaAt` 为基准，心跳每 5s 刷新 ⇒ `silentSeconds` 永远 < 15 ⇒ 「模型思考中」永不出现，**反而掩盖真卡住**（与 P0-3 同源）。
- `chatSlice.ts:313`：全局 30s `setInterval` 永不清除（HMR 会叠加）。
- `chat.rs:5394,5432,6308`：预算软降级置 `used_fallback=true` 后，后续网络错误不再降级重试，直接失败；降级提示走单条 delta，前端无 bucket 时用户不知模型已被换掉。
- `Home.tsx:7219`：诊断卡全部用 `fixed bottom-24 right-4` 同一坐标 ⇒ 多张卡完全重叠，下层按钮点不到。
- `messageBlocks.tsx:247`：工具卡用 `window.confirm` 阻塞 JS 线程。

---

## 五、修复方案（分批，建议顺序）

### 批次 1「止血」：让卡死可感知、可恢复（后端 2 处 + 前端 3 处）
1. P0-1 流式客户端改 idle 超时（`connect_timeout` + `read_timeout`，去掉总 `timeout`）
2. P0-3 心跳与活性解耦（新增 `lastHeartbeatAt`，看门狗双判据）
3. P0-5 发送前置判定改为基于任务存活 + 心跳补 `startedAt`
4. A 组「超时/停止不清正文」两处保留 content/reasoning
5. 低危静默提示改基 `lastDeltaAt` 而非心跳

### 批次 2「可见」：不再静默转圈
6. P0-2 重试阶梯发 `notice` + 总时限 + 阈值下调
7. P0-7 压缩上下文发 `notice` + 独立心跳
8. B 组 `runId==null` 首包接受、`chat-done` 清 `askCard`、panic 补 `chat-error`

### 批次 3「交互门改造」：从全屏遮罩到内联卡片
9. P0-4 审批/提问改内联卡片 + Esc + 批量允许 + 角标
10. C 组恢复路径回填 + `await` 时序修正
11. `/plan` 改消息级单次生效

### 批次 4「收尾」
12. P0-6 子代理走统一 lane（透传 cancel/registry 与心跳）
13. P0-8 伪 Done 保留 tool_calls
14. A 组自动贴底重写（scroll 事件驱动 + 用户上滑才解除跟随）
15. 停止后清理工具卡状态

每批完成后跑 `npx tsc -b` + `cargo check`，并优先补前端测试（当前覆盖极低）。

---

## 六、结论

**不是「没有机制」，而是「机制互相打架」**。

这个项目的看门狗、停滞判定、重试续写、崩溃恢复其实比多数同类工具都完善，
问题出在三处系统性错位：

1. **总超时 vs idle 超时**：网络层用总时长判死，抢在业务层 idle 判定之前杀请求（P0-1）；
2. **心脏跳动 vs 真实产出**：心跳刷新了「有产出」的时间戳，把两件事混成一件事（P0-3 / 低危静默提示）；
3. **门禁 vs 流动**：审批/计划/提问用全屏遮罩实现，天然与「自然流动」冲突（P0-4）。

把这三处对齐，再补上「重试/压缩要说话」，体验就能接近 ChatGPT / Trae。
