# 局域网访问功能设计文档（LAN Access）

> 目标：DevEco Switch 在进程内开一个 HTML 服务器，局域网内手机/平板/其他电脑通过浏览器
> （如 `http://192.168.0.190:12345/`）对软件会话进行操作：切换项目、新建会话、跟踪现有会话、
> 发送消息、审批工具调用等。
>
> 状态：设计定稿，待实现。分 P1/P2/P3 三个阶段交付，P4 为展望区（前三个阶段可用、好用之后再评估）。

---

## 1. 目标与范围

### 1.1 目标定稿

- **操作范围**：完整读写 + 审批。网页端可发消息、停止生成、审批工具调用、批准计划、回答提问、管理会话（重命名/归档/置顶/删除）、搜索消息。
- **鉴权**：**多令牌**（每个设备/人员独立 6 位数字 token），支持备注名称、有效期（永久/N 天/自定义日期）、单独撤销；登录失败锁定。
- **桌面端**：完整设置页（开关/端口/token/二维码/只读模式）。
- **原则**：新模块零业务逻辑，全部复用现有命令函数与全局事件系统；不暴露会话域以外的任何能力。

### 1.2 范围边界（红线）

**只暴露"会话域"接口**（项目/会话/消息/审批/搜索/成本统计）。以下一律不注册到 LAN 路由：

- 终端执行、文件系统**写**操作、设备控制（hdc）、环境配置、Provider/API Key、维护清理等命令
- 所有敏感配置（token 除外）不返回给网页
- **只读文件查看属于例外**：为支持"跟踪 Agent 改了哪些文件"，允许经 `read_project_file` 读取**项目工作区内**文本文件（受路径边界约束、≤5MB、只读），但**不做任何写/删/移动**操作

### 1.3 为什么可行

- 已有 hyper HTTP 服务器模板：`src-tauri/src/services/proxy_service.rs`（启停/端口顺延/状态查询全有）
- 全部业务逻辑已是 Rust 命令函数（`list_projects` / `create_conversation` / `send_message` / `stream_chat` …），HTTP 层直接调用，不重复实现
- `app.emit` 全局事件系统，桌面与网页同进程、天然同步；SSE 桥接即可实时跟踪

### 1.4 不做什么

- 不做 HTTPS（局域网内信任域，v1 用 HTTP；若未来需要可加）
- 不暴露终端/文件**写**/设备类命令
- 不要求局域网外访问（不做内网穿透）；外网场景推荐用户自行用 Tailscale / ZeroTier 把手机拉进虚拟局域网（见 P4）
- 不复制现有 React 前端，网页端用独立的极简原生实现（P3 起加 PWA Service Worker，纯 Web 兑底，不依赖原生 App 分发）

---

## 2. 总体架构

```
浏览器（手机 / 平板 / 其他电脑）
   │  http://192.168.x.x:12345/   （token 鉴权）
   ▼
hyper LAN Server（进程内，绑定 0.0.0.0）【新增】
   ├── 鉴权中间件：Bearer <6位数字 token> + 失败锁定
   ├── 静态 Web UI（原生 HTML/CSS/JS，include_str! 内嵌）
   ├── REST /api/* → 直接调用现有命令函数（AppHandle 取 State）
   └── SSE /api/events → app.listen_any 桥接全局事件 → 推给网页
        │
        ├── 现有命令函数层（复用，不修改）
        ├── 全局事件系统 app.emit
        └── SQLite 会话库
```

关键点：

1. **零业务逻辑**：LAN Server 只做"鉴权壳 + 薄路由 + 事件桥"。
2. **流式任务**：`stream_chat` 是整段 async（跑完整任务才返回），HTTP 层必须 `tauri::async_runtime::spawn` 后立即返回 202，事件走 SSE，绝不能同步 await（否则连接挂到任务结束）。
3. **事件桥常驻**：LAN Server 启用时即注册全局监听器并维护按会话的有界缓冲，解决"网页中途打开正在运行的会话时拿不到半截增量"的问题。

---

## 3. 后端设计（Rust）

### 3.1 新模块 `src-tauri/src/services/lan_server.rs`

仿照 `ProxyServer` 结构：

```rust
pub struct LanServer {
    shutdown_tx: Option<oneshot::Sender<()>>,
    status: Arc<TokioMutex<LanStatus>>,
}

pub struct LanStatus {
    pub running: bool,
    pub listen_address: String,  // 0.0.0.0
    pub listen_port: u16,        // 实际端口（顺延后）
    pub read_only: bool,
}
```

- `start` / `stop` / `get_status`，绑定 `0.0.0.0`，端口占用自动顺延（复用 proxy 现成模式，最多尝试 20 个端口）
- 静态资源经 `include_str!` 内嵌（`index.html` / `app.js` / `style.css` 三个文件）
- 多开保护：沿用 `ProxyLock` 模式，仅锁持有者启动；退出时（`RunEvent::ExitRequested`）随 proxy 一起停止

### 3.2 配置持久化

新增迁移 `042_lan_config.sql`（全局状态）+ `043_lan_tokens.sql`（多令牌）+ `044_lan_sessions.sql`（使用会话记录）：

```sql
-- 042：全局开关/端口/只读/失败锁定（id=1）
CREATE TABLE IF NOT EXISTS lan_config (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL DEFAULT 0,      -- 总开关
    port INTEGER NOT NULL DEFAULT 12345,     -- 监听端口
    read_only INTEGER NOT NULL DEFAULT 0,    -- 只读模式
    fail_count INTEGER NOT NULL DEFAULT 0,   -- 连续鉴权失败次数
    lock_until INTEGER NOT NULL DEFAULT 0    -- 锁定截止时间戳（unix，0=未锁定）
);

-- 043：多令牌（每个设备一个，可单独撤销/设有效期）
CREATE TABLE IF NOT EXISTS lan_tokens (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL DEFAULT '',           -- 备注名称（如"手机""平板"）
    token_hash TEXT NOT NULL,                -- sha256(salt+token) 十六进制
    token_salt TEXT NOT NULL,                -- 每令牌独立随机盐
    expires_at INTEGER NOT NULL DEFAULT 0,   -- 到期时间戳（unix，0=永久）
    created_at INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL DEFAULT 0
);
-- 旧版单令牌自动迁移为 lan_tokens 第一条（"默认令牌"，永久）

-- 044：使用会话（SSE 连接=一次使用：设备/UA/起止/时长）
CREATE TABLE IF NOT EXISTS lan_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token_id INTEGER NOT NULL,
    device TEXT NOT NULL DEFAULT '',         -- 解析后的设备类型
    user_agent TEXT NOT NULL DEFAULT '',
    started_at INTEGER NOT NULL,
    ended_at INTEGER NOT NULL DEFAULT 0,
    duration_secs INTEGER NOT NULL DEFAULT 0
);
```

### 3.3 命令

| 命令 | 说明 |
|---|---|
| `start_lan_server` | 启动（enabled=1；校验至少存在一个令牌；返回实际端口与错误信息） |
| `stop_lan_server` | 停止 |
| `get_lan_server_status` | 状态 + 令牌列表（含明文 `token_plain` 供有效期内重显二维码、最近使用设备/时长） |
| `update_lan_server_config` | 端口 / 只读模式 / 开关 |
| `create_lan_token` | 创建令牌（名称 + 有效期），返回明文并**落库**（`token_plain`） |
| `list_lan_tokens` | 令牌列表（含明文） |
| `revoke_lan_token` | 撤销令牌：立即失效 + 定向断开该令牌全部 SSE 连接 |
| `get_lan_ips` | 枚举本机局域网 IP 列表（过滤虚拟网卡，设置页展示访问地址 + 二维码用） |
| `list_conversation_files` | 聚合会话内全部 `modified_files_json` 的去重文件列表（LAN 内部函数，非 IPC 命令；P2，供网页"修改文件"栏） |

> 桌面设置页也可直接读 `lan_config` 表（现有 `read_config/write_config` 风格），命令式接口与表读写二选一，倾向命令式（含锁定逻辑收敛在 Rust 侧）。

### 3.4 鉴权中间件（多令牌 + 有效期 + 失败锁定）

- 创建时生成随机 6 位数字（`uuid` 截取），盐 + sha256 哈希落 `lan_tokens`；**明文同时落库 `token_plain`**（仅本机 sqlite），供有效期内随时重显二维码（旧令牌该列为 NULL，无法恢复二维码，前端提示重建）
- 所有 `/api/*` 与 SSE 要求 `Authorization: Bearer <6位数字>`（SSE 额外兼容 `?token=`，因 EventSource 无法自定义请求头）
- **多令牌匹配**：遍历全部**未过期**令牌做恒定时间比较（逐字节 XOR 累加，防时序侧信道）；命中即通过并记录 `last_used_at`
- **有效期**：`expires_at` 到期后令牌自动失效（登录拒绝 + 已建立 SSE 连接由心跳主动断开）
- **撤销**：`revoke_lan_token` 删除记录即立即失效，并定向向该令牌的所有 SSE 连接发送 `session-expired`（网页端回登录页）
- **失败锁定**（全局）：
  - 连续 5 次失败 → 锁定 30 秒；之后每次再失败锁定时间翻倍（30s/60s/120s/…）
  - 锁定期间一律 401，响应体带 `retry_after` 秒数
  - 成功鉴权清零 `fail_count` / `lock_until`

### 3.5 路由层调用现有命令

- handler 持有 `AppHandle`，通过 `app.state::<DbState>()` 等 Tauri 状态后**直接调用现有命令函数**
- 注意 `State<'_, T>` guard 的生命周期：在 async handler 内局部获取、局部使用，不跨 await 悬挂
- 只读模式下所有写接口统一返回 403（SSE 仍可用）
- **路由白名单硬编码**，只注册会话域接口（见 §4）

### 3.6 SSE 事件桥 + 中途加入缓冲

- **常驻监听**：LAN Server 启动即 `app.listen_any(...)`，非"有连接才监听"——否则中途加入无缓冲可用
- **按会话有界缓冲**（`Mutex<HashMap<conv_id, Buf>>`）：
  - 只缓冲 `chat-stream` / `chat-reasoning` 增量，上限最近 5 分钟或 ≤100KB（先到先截断）
  - 附带当前工具执行状态（最近一条 `chat-tool-start/done`）与最新 todo 快照
  - `chat-done` / `chat-error` / `chat-stopped` 到达时清空对应会话缓冲
- **SSE 连接流程**：
  1. 鉴权（token）
  2. 客户端声明关注的会话（SSE 首帧或 query 参数）
  3. 先回放该会话缓冲（历史增量）→ 再实时转发
  4. 每 20s 心跳 `: ping` 注释行（防代理/移动网络断连）
- **重连**：EventSource 自动重连；前端重连后补拉一次最新消息页 + 回放缓冲，保证内容连续
- **转发事件白名单**：

```
chat-stream            chat-reasoning          chat-done
chat-error             chat-stopped            chat-tool-start
chat-tool-done         chat-tool-approval      chat-plan
chat-plan-resolved     chat-ask                agent:todo
agent:log              chat-agent-start        chat-agent-done
chat-job-done          conversation-renamed    conversation-deleted
projects-changed       chat-compact
```

### 3.7 关键事件通知（P3，浏览器通知替代 Web Push）

手机端跟踪的体验短板：切后台几十秒 SSE 连接就被移动浏览器挂起，用户看不到"任务跑完了/需要审批了"。

**技术限制（实现时确认）**：Web Push / Service Worker 要求安全上下文（HTTPS）。局域网访问是 `http://192.168.x.x`，浏览器拒绝注册 SW 与订阅 Push。因此 P3 落地为**网页在线时的系统通知（Notification API）**：

- 网页保持打开（前台或后台 tab 且 JS 未被挂起）时，SSE 事件到达 → `Notification` 弹系统通知（标题 + 一句话摘要），点击跳转对应会话
- 触发事件：`chat-done` / `chat-error` / `chat-tool-approval` / `chat-ask` / `chat-plan`（`chat-stream` 增量不推，避免刷屏）
- 当前会话且页面聚焦时静默（避免打扰）；通知 10s 自动关闭
- 通知权限在登录成功后由用户手势静默请求
- 真正离线推送（网页关闭也能收到）需要 HTTPS（自签证书体验差，不推荐）或原生 App——列入 P4 触发条件

> 新增依赖 `web-push` crate、`push_subscriptions` 表、`lan_push_subscribe/unsubscribe` 命令**均不再需要**（P3 不实现 Web Push）。若未来切换到 HTTPS 或原生 App，再按本节省略的部分补回。

---

## 4. REST / SSE API 定稿

### 4.1 读接口

| 方法 | 路径 | 后端命令 | 说明 |
|---|---|---|---|
| GET | `/api/projects` | `list_projects` | 项目列表 |
| GET | `/api/projects/:id/conversations` | `list_conversations` | 会话列表（支持 `?archived=&keyword=`） |
| GET | `/api/conversations/:id/messages` | `list_messages_page` | 消息分页（`?before=&limit=`，默认最近 60 条） |
| GET | `/api/conversations/:id/todos` | `get_todos` | 任务清单 |
| GET | `/api/projects/:id/pending` | `list_pending_confirmations` | 待审批/计划/提问角标 |
| GET | `/api/conversations/:id/cost` | `conversation_cost_stats` | token/成本统计 |
| GET | `/api/projects/:id/search?q=` | `search_messages` | 消息全文搜索（项目内） |
| GET | `/api/search?q=` | `search_messages_all_projects` | 跨项目消息搜索（P2） |
| GET | `/api/conversations/:id/files` | `list_conversation_files` | 会话修改过的文件列表（由 `modified_files_json` 聚合，P2） |
| GET | `/api/projects/:id/file?path=` | `read_project_file` | 只读读取项目工作区内文本文件（≤5MB，路径边界校验，P2） |

### 4.2 写接口

| 方法 | 路径 | 后端命令 | 说明 |
|---|---|---|---|
| POST | `/api/projects/:id/conversations` | `create_conversation` | 新建会话（可带 title / worktree） |
| POST | `/api/conversations/:id/messages` | `send_message` | 仅入库 user 消息（不触发任务） |
| POST | `/api/conversations/:id/stream` | `stream_chat` | 发起任务：**spawn 后立即 202**，事件走 SSE |
| POST | `/api/conversations/:id/stop` | `stop_chat` | 停止当前生成 |
| POST | `/api/approvals/:requestId` | `resolve_tool_approval` / `resolve_plan_review` / `resolve_ask_user` | 三合一：按 body 的 `kind` 分发 |
| POST | `/api/conversations/:id/rename` | `rename_conversation` | 改名 |
| POST | `/api/conversations/:id/pin` | `update_conversation` | 置顶/取消 |
| POST | `/api/conversations/:id/archive` | `update_conversation` | 归档/取消 |
| POST | `/api/conversations/:id/delete` | `delete_conversation` | 删除；若会话有任务运行中，会先停止 + abort 任务并释放项目锁再级联删除 |

> ~~`/api/push/subscribe` / `/api/push/unsubscribe`~~：Web Push 在 HTTP 局域网不可用，P3 不实现（见 §3.7）。

### 4.3 实时

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/events` | SSE 事件流（`?token=` + `?ua=`（自动采集设备信息），支持 `?conversation=` 聚焦某个会话） |

SSE 额外事件：
- `session-expired`：令牌被撤销或到期，服务端主动断开——网页端收到后清除登录态回到登录页。

### 4.4 审批三合一请求体

```json
POST /api/approvals/:requestId
{
  "kind": "approval" | "plan" | "ask",
  "approved": true,
  "remember": false,        // approval 专用：始终允许
  "feedback": null,         // approval 专用：拒绝理由
  "scope": "session",       // approval 专用
  "answer": "…"             // ask 专用：回答（空串=跳过）
}
```

### 4.5 令牌管理（桌面 IPC，不暴露网页 REST）

令牌的创建/列表/撤销**只走桌面端 tauri command**（`create_lan_token` / `list_lan_tokens` / `revoke_lan_token`），**不注册到网页 REST 路由**——否则任何持有旧令牌的网页都能自我续命或撤销其他令牌，破坏撤销授权语义。网页端只消费"登录鉴权"。

---

## 5. 网页端设计（原生 HTML/CSS/JS）

### 5.1 为什么不用现有 React

现有前端深度耦合 Tauri `invoke/listen`（IPC），桥接成本高于重写；且网页端只需"登录 + 项目/会话 + 消息 + 待处理"四块功能。用原生实现，约 1000 行，拆三个文件，`include_str!` 内嵌：

```
src-tauri/src/services/lan_ui/index.html   ← 布局 + 样式引用
src-tauri/src/services/lan_ui/style.css
src-tauri/src/services/lan_ui/app.js
```

### 5.2 页面结构（移动端优先）

1. **登录页**
   - 6 格验证码式数字输入（自动跳格、整串粘贴自动填、输入完自动提交）
   - 扫码直达：二维码内容为 `http://<ip>:<port>/#<token>`（fragment 不进服务器日志、不出现在 Referer），前端读取 `location.hash` 自动填充并跳转
2. **会话列表页**（默认路由 `/`）
   - 项目切换（顶部下拉 / 侧栏）
   - 会话列表：标题、更新时间、置顶/归档、未读待处理角标
   - 新建会话按钮
   - 消息搜索框（项目内 + 跨项目两个入口，P2）
3. **会话页**（路由 `/chat/:id`）
   - 消息列表：分页加载（向上滚动翻页），user/assistant 气泡，Markdown 简渲（`<pre>`+ 文本，不做完整渲染）
   - 流式增量：SSE 逐帧追加当前会话输出，光标末尾
   - 工具执行：当前工具名 + 进度条（读 `chat-tool-start/done` 推状态）
   - **修改文件列表 + 文件查看（P2）**：assistant 消息下方展示该回复改过的文件（`modified_files_json`），点开只读查看内容，可对比会话历史版本（走消息版本表）
   - 输入框（Enter 发送，Shift+Enter 换行）+ 发送/停止按钮（运行中切换为停止）
   - **语音输入（P3）**：输入框旁麦克风按钮，用 Web Speech API（`webkitSpeechRecognition`）转写填充；不支持时隐藏
   - **图片发送（P3）**：输入框旁图片按钮（拍照/相册），data URL 随 `stream_chat` 的 `images` 参数发送（多模态）
4. **待处理卡片**（会话页内浮动层 + 顶部角标）
   - 工具审批：工具名/参数/风险等级 + 允许/拒绝/始终允许
   - 计划审查：计划文本 + 批准/拒绝
   - 提问：问题 + 选项（单选）/ 自由回答

### 5.3 技术要点

- `fetch` + `EventSource`（SSE 自动重连），无第三方 JS 依赖（二维码由桌面端生成）
- 鉴权失败（401）→ 回登录页并提示锁定剩余时间
- 只读模式下隐藏输入框与操作按钮
- 会话内消息渲染对长文本做 `white-space: pre-wrap` + 溢出裁剪；文件内容纯文本 `<pre>` 展示（代码高亮可在后续增强，非必需）
- **深色模式**：跟随 `prefers-color-scheme`，CSS 变量切换（P2 已落地）
- **添加到主屏幕（P3）**：`manifest.json` + `apple-touch-icon`（SVG 图标），获得全屏独立窗口体验；**无 Service Worker**（HTTP 局域网无法注册，见 §3.7）
- 语音输入走浏览器原生 Web Speech API（零后端成本）；不支持（如部分桌面浏览器）时自动隐藏入口
- 图片发送走浏览器 `<input type="file" capture>`（拍照/相册）→ dataURL → `stream_chat` 的 `images` 参数；最多 4 张，发送前缩略图预览
- **关键事件通知（P3）**：`Notification` API（网页在线时），事件触发见 §3.7；`Notification.permission` 拒绝/不支持时静默降级

---

## 6. 桌面设置页设计

在现有 React 工程（ConfigPage / 独立 LAN 页）新增两个分区卡片，并排挂载：**「局域网访问」服务卡片**（LanPanel）+ **「已发放令牌」管理卡片**（LanTokenPanel）。

**LanPanel（服务卡片）**：

| 控件 | 行为 |
|---|---|
| 总开关 | 默认关；开启时校验至少存在一个令牌并启动服务 |
| 端口输入 | 默认 12345；启动失败提示端口占用并显示实际顺延端口 |
| 只读模式开关 | 切换后写接口 403（需重启服务生效或热生效，P1 先热生效） |
| 访问地址 | 自动列出本机 IP 列表（`get_lan_ips`，已过滤虚拟网卡）+ 端口，逐条可复制 |
| 防火墙提示 | 首次启动弹提示："Windows 防火墙可能拦截局域网访问，请允许此程序通过专用网络" |
| 状态行 | 运行中/已停止 + 最近一次错误（端口占用/绑定失败等） |

**LanTokenPanel（已发放令牌卡片，自包含拉取状态）**：

| 控件 | 行为 |
|---|---|
| 生成令牌 | 名称 + 有效期（永久/7/30/90 天/自定义日期）→ 明文高亮展示一次（可复制）+ 独立二维码 |
| 令牌列表 | 每个令牌一行：名称、有效期（永久/剩 X 天/已过期）、最近使用设备与时长、撤销按钮；**有效期内常驻展示该令牌二维码**（每 IP 一个，可直接扫码进入；服务停止时也展示，方便预生成）；已过期不展示；旧令牌（无明文）提示重建 |

### 新增依赖

- Rust：`local-ip-address`（枚举本机局域网 IP，轻量无 C++）
- 前端（桌面设置页）：`qrcode.react`
- Web UI 静态资源：语音输入走浏览器原生 Web Speech API、图片走 `<input type="file">`、通知走 Notification API，均无第三方依赖

---

## 7. 安全设计汇总

| 项目 | 方案 |
|---|---|
| 鉴权 | **多令牌**：每个设备独立 6 位数字 token（`sha256(salt+token)` 落 `lan_tokens`；明文另存 `token_plain` 列，仅本机 sqlite，用于有效期内重显二维码） |
| 有效期 | 每令牌 `expires_at`（0=永久）；到期登录拒绝 + SSE 心跳主动断开 |
| 撤销 | `revoke_lan_token` 删除记录立即失效 + 定向断开该令牌全部 SSE 连接（`session-expired`） |
| 防爆破 | 连续 5 次失败锁定 30s，之后翻倍（30/60/120…），恒定时间比较 |
| 使用审计 | `lan_sessions` 记录每次连接：设备类型（UA 解析）、起止时间、时长；设置页按令牌展示 |
| 访问边界 | 路由白名单仅会话域；终端/文件写/设备/配置类命令一律不注册；令牌管理仅桌面 IPC |
| 只读模式 | 写接口 403，SSE 仍可用 |
| 敏感信息 | API Key、Provider 配置等不回传；token 明文仅存本机 sqlite（`token_plain`），仅经 IPC 回传桌面设置页用于二维码展示 |
| 传输 | 局域网信任域 HTTP；二维码 token 走 URL fragment 不进日志 |
| 多开 | ProxyLock 同款互斥，仅锁持有者启动 |

---

## 8. 阶段与验收

### P1 后端 + 设置页

内容：
- `lan_config` 迁移 + `lan_server.rs` 服务骨架（启停/端口顺延/状态）
- 鉴权中间件（6 位 token + 失败锁定 + 恒定时间比较）
- 全部 REST/SSE 接口（含 `stream_chat` spawn、审批三合一、事件桥 + 缓冲）
- 命令注册（start/stop/status/config/create+list+revoke token/ips）+ `local-ip-address` 依赖
- 桌面设置页（开关/端口/只读/token/二维码/防火墙提示）

验收：
- 设置页开关后，`curl -H "Authorization: Bearer <token>" http://127.0.0.1:12345/api/projects` 返回列表
- 带 token POST stream 能触发流式，SSE 能收到 `chat-stream` 事件
- 连续 5 次错误 token → 401 + 锁定；解锁后恢复
- 无 token / 错误 token 一律 401

### P2 网页端完整功能

内容：
- 登录页 + 项目/会话列表 + 消息流式渲染 + 审批/计划/提问卡片 + 停止 + 会话管理
- 消息搜索（项目内 + 跨项目 `search_messages_all_projects`）
- **修改文件列表 + 只读文件查看**（`list_conversation_files` + `read_project_file`，路径边界校验，代码高亮）
- 只读模式（写接口 403 的前端隐藏与提示）

验收（手机真机走通全流程）：
- 扫码或手动输入 token 登录
- 新建会话 → 发消息 → 实时看流式输出 → 审批放行工具 → 停止生成
- 会话改名/归档/删除/置顶可用；项目内与跨项目搜索可用
- Agent 改过的文件在会话中可见、可点开只读查看（超出工作区/非文本被拒）
- 桌面运行中，手机中途打开会话能看到半截内容（缓冲回放）并继续实时跟踪
- 只读模式下所有写操作被拒并提示

### P3 打磨 + 移动端体验

内容：
- SSE 重连与心跳健壮性（心跳已有）、连接状态指示（顶部圆点）、连接日志（可选：`lan_config` 增加访问计数）、桌面/网页同开同步高亮
- **关键事件通知**：`Notification` API（网页在线时），`chat-done`/审批/提问/计划触发（见 §3.7）
- **语音输入**（Web Speech API）+ **图片发送**（`images` 参数，多模态，≤4 张）
- **深色模式**（跟随系统，P2 已落地）+ 移动端样式细节 + manifest 添加到主屏幕

验收：
- 弱网/切后台重连后消息不丢、不重复；只读模式下所有写操作被拒
- 网页打开（前台/后台 tab）时，任务完成或需要审批能收到系统通知，点击跳转对应会话
- 语音转写能填充输入框；图片能随消息发送并展示预览
- 手机"添加到主屏幕"后以全屏独立窗口运行；连接状态点绿/红指示实时状态
- 整体体验无感、流畅

### P4 展望区（暂缓，前三阶段可用好用后再评估）

以下内容已评估过，**明确暂不做**，等 P1–P3 交付并实际使用后再回来决定是否值得投入：

| 候选 | 评估 | 触发条件 |
|---|---|---|
| **会话分享 / 公开链接** | 生成只读临时链接发给同事；涉及隐私边界，需过期时间/密码/IP 限制设计 | 多人协作被实际需要 |
| **外网访问** | 不做内网穿透；官方文档引导用户用 Tailscale / ZeroTier 把手机拉进虚拟局域网（手机 4G 也能连）。**注意：6 位数字令牌仅适配局域网信任域，若暴露公网必须升级为 HTTPS + 强令牌** | 用户真实跨网络使用诉求出现 |
| **多用户 / 协作** | 单用户场景足够；多用户引入权限/冲突/审计，是另一个量级 | 明确的多用户需求 |
| **真正的远程桌面** | 鼠标键盘控制桌面，是另一个产品（向日葵/ToDesk 定位），与本功能定位不符 | 基本不做，除非产品定位改变 |
| **真离线推送（Web Push / 原生 App）** | HTTP 局域网下 Web Push 不可用（安全上下文限制）；离线推送需 HTTPS（自签证书体验差）或原生 App（分发/维护成本高） | 明确需要"网页关闭也能收到"时再评估 |
| **令牌到期前提醒** | 当前到期即默默断连（对新令牌足够）；对长期令牌，可提前 3 天在桌面端提示"令牌即将到期/续期" | 长期令牌成为常态用法 |
| **登录历史审计列表** | 目前 lan_sessions 只聚合展示**最近一次**使用（设备/时长）；完整历史（每次登录时间/设备/IP 的列表）对"谁用过、什么时候用"更透明 | 出现安全排查/追溯诉求 |
| **二维码可用性引导** | 多 IP 会生成多个二维码（每 IP 一个）；可加"主 IP 一键选择/折叠"；**历史旧令牌（045 前）二维码无法重显（无明文），引导一键重建** | 用户反馈二维码困惑时 |

### 已知取舍与限制（设计确认，不改）

- **6 位数字令牌强度**：仅 10^6 组合，安全性靠全局失败锁定（5 次 → 30s 起、翻倍封顶）兜底；**仅限局域网信任域**，公网暴露必须升级（见上表"外网访问"）
- **使用时长统计误差 ≤20s**：会话结束靠 SSE 心跳（20s 间隔）探测，切后台/断网时统计偏乐观；作为"使用情况"参考而非计费依据
- **二维码明文落库（仅本机 sqlite `token_plain`）**：换取"有效期内二维码随时可扫"的体验；代价是本机数据库可读时令牌明文可被提取（令牌为 6 位数字 + 有效期内 + 可撤销，撤销即失效；046 之前的旧令牌无明文，需重建）
- **撤销/过期即断开**：被撤销或到期令牌的所有已连接设备立即回登录页（`session-expired`），旧二维码/URL 随即失效

---

## 9. 测试策略

- **Rust 单元测试**（`cargo test`）：
  - 鉴权：正确/错误/锁定/锁定过期/恒定时间比较
  - 路由：路径匹配、只读 403、未知路径 404、方法不匹配 405
  - 事件缓冲：回放顺序、容量截断、`chat-done` 清空
  - 文件查看：工作区内放行、路径穿越（`../`、绝对路径、符号链接）拒绝、超限拒绝
  - 现有测试全量保留
- **前端**：`vitest` 现有用例不动；Web UI 无构建链，靠 P2 真机手动验收
- **手动冒烟脚本**：`scripts/` 下补一个 curl 序列验证核心接口

---

## 10. 风险与对策

| 风险 | 对策 |
|---|---|
| 暴露会话域外能力 | 路由白名单硬编码 + 代码评审清单 |
| 6 位数字被爆破 | 失败锁定 + 翻倍 + 恒定时间比较 |
| 中途加入丢增量 | 常驻监听 + 有界缓冲回放 |
| `stream_chat` 阻塞 HTTP | spawn + 202 立即返回 |
| Tauri State guard 生命周期 | handler 内局部借用，不跨 await 悬挂 |
| Windows 防火墙首次弹窗困惑 | 设置页引导文案 |
| 多开实例冲突 | ProxyLock 同款互斥 |
| 长任务时 SSE 连接断 | 20s 心跳 + EventSource 自动重连 + 重连补拉 |
| 文件查看路径穿越（`../`/绝对路径/符号链接） | 复用 `read_project_file` 的路径边界校验（工作区限定 + 规范化），P2 加单测锁死 |
| 网页切后台收不到任务进展 | 关键事件 `Notification` 通知（网页在线时）；真正离线推送列入 P4（需 HTTPS/原生 App） |
| 通知打扰 | 仅关键事件触发；当前会话且页面聚焦时静默；通知 10s 自动关闭 |
