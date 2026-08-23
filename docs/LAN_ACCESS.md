# DevEco Switch 局域网访问

> 当前状态：已实现。本文描述 2026-08-21 `main` 分支的实际接口与安全边界。

简体中文 | [English](LAN_ACCESS.en.md)

## 1. 功能范围

DevEco Switch 可在应用进程内启动 HTTP 服务，默认监听 `0.0.0.0:12345`。同一局域网中的手机、平板或电脑可通过浏览器：

- 查看项目和会话；
- 创建、重命名、置顶、归档和删除会话；
- 分页查看与搜索消息；
- 发送消息、启动/停止 Agent 任务；
- 查看 Todo、成本和会话修改文件；
- 处理工具审批、计划审查和 Agent 提问；
- 通过 SSE 跟踪流式消息与任务事件；
- 只读查看项目工作区内的文本文件。

LAN 路由不暴露终端执行、任意文件写入、设备控制、Provider/API key、应用配置或维护清理能力。

## 2. 架构

```text
浏览器
  ├─ 静态原生 Web UI（HTML/CSS/JS/manifest/icon）
  ├─ REST /api/*
  └─ SSE  /api/events
           │
           ▼
Hyper LAN Server（Tauri 进程内）
  ├─ token 鉴权与失败锁定
  ├─ read_only 写请求拦截
  ├─ 会话域路由白名单
  └─ 复用 Rust commands / SQLite / Tauri events
```

实现位置：

- 服务：`src-tauri/src/services/lan_server.rs`；
- 桌面 IPC：`src-tauri/src/commands/lan.rs`；
- Web UI：`src-tauri/src/services/lan_ui/`；
- 桌面设置页：`src/pages/LanPage.tsx`；
- 数据库：迁移 `042`—`046`。

Web UI 使用内嵌原生资源，不复用 React/Tauri IPC 前端。原因是浏览器端只需要局域网会话功能，独立实现能保持体积和权限面最小。

## 3. 启动与配置

桌面端可以：

- 启动/停止 LAN 服务；
- 修改监听端口；
- 开启只读模式；
- 创建、查看和撤销访问 token；
- 查看可访问的本机 IP 与二维码。

`lan_config.enabled = 1` 时应用启动后自动开启服务。端口冲突时服务会尝试顺延；实际地址以桌面状态页返回值为准。

每个设备或使用者应分配独立 token，便于单独备注、设置有效期和撤销。

## 4. 鉴权

所有 `/api/*` 请求必须携带 token：

```http
Authorization: Bearer 123456
```

SSE/EventSource 无法方便地设置 Authorization header，也支持查询参数：

```text
/api/events?token=123456
```

鉴权行为：

- token 为随机 6 位数字；
- 数据库存储 salt + SHA-256 hash；
- 为了在本机桌面端重显二维码，当前实现同时保存 `token_plain`；这意味着拥有本机数据库读取权限的人可以取得 LAN token；
- token 支持永久或过期时间、备注、单独撤销和最近使用时间；
- 比较使用 constant-time helper；
- 连续失败触发全局短时锁定并返回 `retry_after`；
- SSE 连接绑定 token hash，token 撤销或过期后连接会断开。

LAN 使用 HTTP 而不是 HTTPS，token 会在局域网链路上传输。只应在可信网络使用；跨公网访问应通过用户自行管理的 VPN/零信任隧道，并优先限制防火墙来源。

## 5. 只读模式与文件边界

只读模式开启后，除 GET/HEAD 和 SSE 外的请求统一返回 `403`，因此不能创建会话、发消息、停止任务或审批。

文件读取是唯一文件系统例外：

- 只允许项目工作区内路径；
- 复用 `read_project_file` 的路径归一化和边界校验；
- 只返回受支持的文本内容；
- 单文件上限 5MB；
- LAN 没有写、删、复制、移动或命令接口。

“会话修改文件列表”只聚合消息中的相对路径，不读取文件；用户明确打开某个文件时才进入只读文件接口。

## 6. REST API

### 6.1 读取

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/lan/status` | LAN 配置摘要/健康检查 |
| GET | `/api/projects` | 项目列表 |
| GET | `/api/projects/:id/conversations` | 会话列表，可带 `archived`、`keyword` |
| GET | `/api/projects/:id/pending` | 项目待处理审批/计划/提问 |
| GET | `/api/projects/:id/search?q=` | 项目内消息搜索 |
| GET | `/api/search?q=` | 跨项目消息搜索 |
| GET | `/api/conversations/:id/messages` | 消息分页，可带 `before`、`limit`（最大 200） |
| GET | `/api/conversations/:id/todos` | Todo |
| GET | `/api/conversations/:id/cost` | 会话成本 |
| GET | `/api/conversations/:id/files` | 会话修改文件路径 |
| GET | `/api/projects/:id/file?path=` | 项目内只读文件 |

### 6.2 写入

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/api/projects/:id/conversations` | 创建会话 |
| POST | `/api/conversations/:id/messages` | 只保存消息 |
| POST | `/api/conversations/:id/stream` | 启动 Agent，立即返回 `202` |
| POST | `/api/conversations/:id/stop` | 停止任务 |
| POST | `/api/approvals/:request_id` | 处理 `approval` / `plan` / `ask` |
| POST | `/api/conversations/:id/rename` | 重命名 |
| POST | `/api/conversations/:id/pin` | 置顶/取消置顶 |
| POST | `/api/conversations/:id/archive` | 归档/取消归档 |
| POST | `/api/conversations/:id/delete` | 删除会话并收敛运行任务 |

`stream` 不同步等待整个 Agent 任务：服务在 Tauri runtime 中 spawn 后立即返回，进展和终态通过 SSE 发送。

## 7. SSE

`GET /api/events` 建立事件流。服务维护有界事件缓冲，浏览器中途加入或重连时配合消息补拉恢复视图。

事件来源与桌面端一致，包括聊天增量、工具、审批、计划、完成、停止和错误。网页端必须按 `conversation_id` 分桶，不能假设事件只属于当前打开的会话。

浏览器 Notification API 只在页面在线且用户授权时可用。由于普通局域网地址使用 HTTP，不能依赖需要安全上下文的 Web Push/Service Worker 推送。

## 8. 请求限制与错误

- 非 GET 请求体最大 50MB，用于支持多模态图片 data URL；
- 空消息、无效审批类型和非法路径返回 `400`；
- 缺失/无效/锁定 token 返回 `401`；
- 只读模式写请求返回 `403`；
- 未注册路由返回 `404`；
- 数据库或内部错误按统一 JSON error envelope 返回。

前端不应根据中文错误文本判断状态，应优先使用 HTTP status 和结构化字段。

## 9. 安全检查清单

修改 LAN 功能时必须确认：

1. 新路由是否严格属于会话域；
2. `/api` 下是否始终先鉴权；
3. read-only 是否能阻止全部副作用请求；
4. 路径是否复用项目边界校验，不能手工拼接；
5. 返回体是否可能包含 API key、系统路径、环境变量或工具原始敏感输出；
6. 删除/停止是否同步收敛后台任务；
7. SSE 是否在 token 撤销/过期后断开；
8. 新请求体是否有大小上限；
9. 是否补充 token、锁定、只读和路由单测。

当前 LAN 设计的主要风险不是网页 UI，而是 HTTP 明文 token 和会话写权限。默认部署应限制在可信局域网，并为每台设备使用短期、可单独撤销的 token。
