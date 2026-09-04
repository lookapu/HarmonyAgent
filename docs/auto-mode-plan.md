# Auto 模式实现路线（主流工具形态）

> 本文档按主流 AI 编程工具的 auto 模式形态规划：auto 不是一个"省钱开关"，
> 而是把"该用哪个模型"这件事交给系统——核心动机是**质量匹配、额度/限流保护、延迟**，
> 省钱只是 API 计费用户场景下的子集收益。
>
> 状态说明：P0 已实现并验证（后端 + 前端入口 + provider 池三态）；§4.4 可观测性与统计页为后续项。

## 1. 主流工具的 auto 是什么

主流工具里其实存在**两种不同的 auto**，规划时分开对待：

| 类型 | 代表 | 行为 | 主要动机 |
|---|---|---|---|
| A. 主模型自动路由 | Copilot agent 的 auto、Cursor 类 | 由系统在候选模型（多为同一计划内的前沿模型）中按任务类型挑一个 | 质量匹配、厂商可用性/限流、用户不用自己追模型版本 |
| B. 辅助杂活的降级模型 | Claude Code 用 haiku 做标题/摘要；aider 用 weak model 做 repo map/commit | 后台、非用户可见质量链的调用，固定走便宜/快的小模型 | 延迟（杂活不拖主对话）、不占用高级模型额度；省钱是副产品 |

关键洞察：主流 auto **不做显式的能力排名**——厂商在自家模型间路由时排名内置；跨厂商时它们也不比较。因此我们**不需要 `quality_rank` 那套**，只需把"候选池"和"路由规则"设计好。

## 2. 本产品的 auto 定义

前端模型选择器增加「自动」选项，选中后：

1. **主对话**（用户每条消息 → 回复）：走 A 类——在候选模型池内按任务类型路由。
2. **辅助调用**（标题、摘要、上下文压缩、子 Agent 检索类杂活）：走 B 类——固定路由到池内最便宜、不占主模型额度的模型（`pick_economy_model` 的跨 provider 泛化）。

价值主张（对不同用户）：

- 包月/coding-plan 用户：辅助杂活不再烧高级请求额度，主对话延迟不被杂活拖累。
- API 计费用户：主对话保持高质量，杂活全部压价 → 顺带省钱。
- 所有人：不必手动在多个模型/provider 间切换，系统按任务自动匹配。

## 3. 现状（已有基础设施）

| 能力 | 位置 | 说明 |
|---|---|---|
| 任务分类 | `model_router.rs classify_task()` | Chat / Code / Fast / Vision |
| 同 provider 任务路由 | `model_router.rs pick_model_for_task()` | 只在一个 provider_id 内、排除主模型；Code 选大上下文、Chat 选不更贵的便宜款、Fast/Vision 最便宜 |
| 同 provider 经济模型 | `model_router.rs pick_economy_model()` | 比主模型便宜的 tool 款；已用于标题/摘要等 |
| 默认路径自动路由 | `chat.rs` 未指定模型分支 | 仅当用户**未指定**模型时触发；命中后**不写回** conversations.model_id，每轮重新分类 |
| 视觉兜底 | `pick_vision_fallback()` | 同 provider 内 image 款 |
| 元数据 | `models` 表 | tool_call / context_limit / input_modalities / 单价 / is_default / enabled |

## 4. 设计（已实现部分）

### 4.1 候选模型池（Auto Pool）

- `providers` 表新增 `auto_pool_mode INTEGER NOT NULL DEFAULT 0`（迁移 `078_provider_auto_pool.sql`）：
  - `0` = 不参与
  - `1` = 仅主对话
  - `2` = 主对话 + 杂活
- 池构成（`chat.rs auto_pool_ids(min_mode)`）：
  - **active provider 恒在池内**（无论其 auto_pool_mode 为何，作为锚点来源）；
  - 其余 provider 按 `auto_pool_mode >= min_mode` 加入。
  - 主对话路由用 `min_mode=1`，辅助调用用 `min_mode=2`。
- 所有池内模型过滤条件统一：`enabled = 1`。
- 前端 provider 管理页提供三态切换（不参与 / 仅主对话 / 主对话+杂活）。

> 未实现：auto 选项旁的"池概况"（N 个 provider / M 个模型）提示，归入后续。

### 4.2 A 类：主对话按任务路由（实现口径）

**路由时机**：`opts.model_id == "auto"` 时（`stream_chat_inner` 分支）。

- 锚点 = active provider 默认模型（`ORDER BY is_default DESC, created_at ASC LIMIT 1`）。
- 候选 = `auto_pool_ids(min_mode=1)`，调用 `pick_model_for_task_in_pool`：
  - Code → 上下文窗口最大者（要求 tool_call、不小于锚点上下文）
  - Chat → 不超过锚点价格的最便宜 tool 款
  - Fast / Vision → 最便宜（Vision 要求 image 输入）
- 命中且落在其他 provider 时，加载该 provider 的 `ProviderEndpoint`（含 keyring key）+ 模型 `ModelChoice`；否则回退锚点模型。
- **每轮重新分类路由，不写回会话锚点**（与现有"未选模型"路径行为一致，仅扩展到池）。会话绑定写回逻辑跳过 `"auto"`，避免污染 `conversations.model_id`。

> 与早期草案的偏差：草案曾设想"会话锚点写回 + 会话内锁定"的稳定策略，最终实现为**按任务每轮路由**——理由：符合 Copilot 式主流 per-task 路由、改动更小、行为确定（池 + 锚点不变时结果确定）。若后续需要"会话内锁定、只在带图/手动改选时换"，可在此基础上补锚点写回，很小。

### 4.3 B 类：辅助调用跨池经济路由

- 新增 `pick_economy_in_pool(pool, main_provider_id, main_model)`：等价 `pick_economy_model` 去掉 provider 过滤，在池内取比主模型便宜的最便宜 tool 款；候选为空时回退主模型（保守）。
- 新增 `chat.rs resolve_aux_economy(state, main_provider, main_choice)`：统一出口，命中跨 provider 时返回对应端点 + 模型。
- 已接入的辅助调用点：子 Agent 默认模型（`resolve_agent_model`）、滚动摘要（`summarize_rolling_history`）、手动压缩（`compact_conversation`）、自动标题（`generate_conversation_title`）、记忆提取（`summarize_memory`）。
- 辅助池 = `auto_pool_ids(min_mode=2)`。
- 经济路由只考虑**已定价**模型（`input + output > 0`），因此未定价的包月/本地模型天然不会参与杂活压价——避免把杂活错误地丢到 coding plan 上再烧一份额度。

### 4.4 可观测性（未实现，后续）

- 每条 assistant 消息记录实际模型（SSE `model-selected` 事件：`{ call_site: main|aux, model_id, provider_id }`），气泡上显示小标签——auto 模式下尤其必要，用户要能看见系统选了谁。
- 统计页：池内各模型被选次数、辅助调用节省估算（API provider 用单价×token 估算；coding plan 按请求次数口径，提示"约 N 次杂活未占用高级请求"）。

## 5. 实现改动点（已落地）

| 文件 | 改动 |
|---|---|
| `migrations/078_provider_auto_pool.sql` | `providers.auto_pool_mode` 三态字段 |
| `db/models.rs` / `db/queries.rs` | `Provider` 结构体 + 读写 SQL 同步新字段 |
| `commands/provider.rs` | `UpdateProviderInput.auto_pool_mode` + 校验（0/1/2） |
| `model_router.rs` | 新增 `RoutedModel`、`pick_model_for_task_in_pool`、`pick_economy_in_pool` |
| `commands/chat.rs` | `"auto"` 主对话分支；`auto_pool_ids` / `resolve_aux_economy` 辅助；会话写回跳过 `"auto"` |
| `api/provider.ts` | `Provider.auto_pool_mode` + `UpdateProviderInput.auto_pool_mode` 类型 |
| `ProvidersPage.tsx` | provider 卡片 Auto 池三态切换 |
| `plan.tsx` / `Home.tsx` | 模型选择器「自动」入口 + 说明；auto 模式跳过会话绑定 |

## 6. 实施阶段

| 优先级 | 内容 | 状态 |
|---|---|---|
| P0 | B 类：auto 池字段 + 前端三态 + `pick_economy_in_pool` + 替换辅助调用点 | ✅ 已实现 |
| P0 | A 类：`"auto"` 主对话分支 + 跨池按任务路由 | ✅ 已实现 |
| P0 | 前端「自动」入口 + provider 池三态 | ✅ 已实现 |
| P1 | 消息模型标签（model-selected 事件） | ⬜ 未实现 |
| P1 | 统计页（请求次数口径，兼容 coding plan 无金额） | ⬜ 未实现 |
| P2 | 池概况提示（auto 选项旁） | ⬜ 未实现 |
| P2 | 回归验证（model_router 池用例已补 4 个） | ✅ 已完成 |

## 7. 明确不做（防止范围蔓延）

- **能力排名 / 跨 provider 主模型质量比较**：主流不解决、我们不引入。A 类只在授权池内按任务/价格路由，不跨 provider 判断"谁更强"。
- **包月额度余量探测**：平台不提供 API，只做请求次数统计提示，不做硬门禁。
- **会话锚点写回 / 按难度重路由**：首版按任务每轮路由；等有真实使用反馈后再评估是否需要锁定锚点。

## 8. 风险与边界

| 风险 | 应对 |
|---|---|
| auto 池配置不当（只勾了 active provider，无第二 provider）→ 跨池收益有限 | 默认 active 恒在池内；无第二 provider 时行为等价于原"未选模型"路由 |
| 每轮分类导致主模型在 Code/Chat 间切换 | 与现有"未选模型"行为一致，结果确定；如需锁定再补锚点写回 |
| 用户看不到自动选了谁 | 消息模型标签 + model-selected 事件（§4.4，待实现） |
| 跨 provider 请求发到未配好的 key | 池默认只含 active provider，其余需显式设为 1/2 |
| 未定价包月/本地模型被误用于杂活 | 经济路由只考虑已定价模型，天然排除 |
| 与原默认路由（用户不选模型）语义冲突 | 统一为"auto"入口；未选模型仍走原默认路径，两分支可共存 |
