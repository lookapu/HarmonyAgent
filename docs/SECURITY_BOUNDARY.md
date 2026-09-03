# 安全边界与威胁模型

> 状态：当前实现基线（v2.1.1）与下一阶段契约  
> 更新日期：2026-09-03

本文说明 HarmonyAgent 当前能够强制的安全边界、不能保证的事项，以及真实执行沙箱必须满足的最低契约。它是安全声明的真源；README、工具描述和 UI 不得给出比本文更强的承诺。

## 1. 当前结论

HarmonyAgent 当前具备工作区路径校验、权限分级、审批、危险命令拒绝、审计、超时、输出限制、进程树清理和工具线程故障隔离。

HarmonyAgent 当前**不具备默认 OS 级命令沙箱**：

- `run_command` 的 `cwd` 必须位于已绑定项目内，但子进程仍以启动应用的宿主用户身份运行；
- 工作目录限制不能阻止命令通过绝对路径、父进程环境、socket 或网络访问其他宿主资源；
- 兼容工具名 `sandbox_exec` 仅在显式提供 `source` 时把它复制到临时目录后执行；未提供 `source` 时只预览、不执行。临时目录仍不是安全边界；
- 工具专用 OS 线程隔离的是 panic、卡死和调度故障，不是文件系统、网络或用户权限；
- worktree 隔离的是 Git 修改和并行任务，不是宿主权限。

因此，在真实沙箱后端交付前，不应执行来源不可信的仓库脚本、依赖安装脚本或 MCP 可执行文件。

## 2. 保护对象

- 项目源码、未提交修改和 Git 凭据；
- SSH key、云凭据、Provider API key、签名证书和系统 Keychain；
- 用户目录中的其他项目和个人文件；
- 本机网络服务、局域网设备和公网资源；
- HarmonyOS 真机、模拟器、已安装应用和签名/发布权限；
- Agent run、审批、审计和评测记录的完整性；
- 宿主 CPU、内存、磁盘、进程数和可用性。

## 3. 威胁来源

系统必须把下列输入视为不可信：

- 模型生成的命令、脚本、路径和工具参数；
- 用户打开的仓库及其中的构建脚本、Git hooks、依赖生命周期脚本和文档指令；
- 网页、检索结果、日志、issue、代码注释和 README 中的 prompt injection；
- MCP server、Skill、工作流模板和第三方扩展；
- 依赖管理器下载的包、二进制和安装脚本；
- 子 Agent 返回的建议和产物。

用户显式输入仍可能包含误操作，不因来源是用户就跳过路径、资源和不可逆操作保护。

## 4. 当前控制与边界

| 控制 | 当前保证 | 不保证 |
| --- | --- | --- |
| `resolve_in_roots` | 工具声明的 `cwd`/文件目标在绑定根内 | 子进程只能访问该根 |
| 命令危险模式 | 拒绝一组已知破坏性命令 | 拒绝所有等价变体、解释器脚本或未知程序 |
| 工具权限/审批 | 按工具和命令级别要求信任或确认 | 已批准命令不会访问超出预期的资源 |
| 临时副本试运行 | `simulate` 强制要求 `source`，避免命令直接修改传入的原目录；未传时不执行 | 文件系统、网络、凭据、进程或 syscall 隔离 |
| 专用工具线程 | panic/卡死不阻塞主要异步运行时，迟到结果被 fencing | 强制终止所有不可取消线程或限制其系统权限 |
| 进程树清理 | 已知前台/后台任务停止时清理直接或派生进程 | 对抗主动逃逸、脱离进程组或宿主服务接管 |
| MCP 项目授权 | 限制暴露工具、声明目录、网络策略和环境继承 | 当前策略等同于 OS 防火墙或强制文件沙箱 |
| Host 特权工具 | 类型化参数、审批、审计和后置验证 | 任意 Shell 获得等价宿主权限后的安全性 |

## 5. 用户可见的安全模式

真实沙箱上线后，产品只使用以下三个稳定模式：

| 模式 | 文件系统 | 网络 | 宿主能力 | 默认用途 |
| --- | --- | --- | --- | --- |
| `read-only` | 项目只读，独立 `/tmp` 可写 | 禁止 | 禁止 | 分析、审查、检索 |
| `workspace-write` | 仅任务工作树可写 | 默认禁止，可按域审批 | 仅类型化 broker | 修改、构建、测试 |
| `host-direct` | 宿主用户权限 | 宿主网络 | 可用 | 兼容模式，显式选择并持续警告 |

任何平台如果不能建立所声明的边界，必须返回 `sandbox_unavailable` 并失败关闭，不能静默切换到 `host-direct`。

## 6. Sandbox Backend 最低契约

### 6.1 文件系统

- 工作区以显式 mount 提供，默认只读；
- 写任务使用独立 worktree 或 copy-on-write 层；
- 禁止访问用户目录、SSH、Keychain、系统凭据和其他项目；
- 对符号链接、硬链接、bind mount、junction、UNC、设备路径和大小写差异做逃逸测试；
- 完成后只导出声明的 patch、日志和 artifact。

### 6.2 网络

- 默认 `none`，包括 DNS、回环地址、Unix socket/Named Pipe 和局域网；
- `allowlist` 绑定域名、端口、协议、审批 ID 和有效期；
- 网络代理不能把宿主凭据透明注入沙箱；
- 每次连接产生可审计的目标和结果摘要。

### 6.3 进程与资源

- 无特权用户、限制进程数、CPU、内存、磁盘、wall time 和输出；
- 禁止获得宿主 PID namespace、容器 socket 或设备访问；
- 取消和超时必须销毁整个执行域；
- 沙箱崩溃后 Agent run 可从外部 checkpoint 恢复。

### 6.4 凭据

- 默认不继承宿主环境；
- Agent 只看到 opaque credential handle，不看到原始 secret；
- 需要凭据的操作通过 Host Capability Broker 完成；
- secret 不进入 prompt、trajectory、stdout/stderr 或 reproduction bundle。

## 7. Host Capability Broker

下列能力不应通过沙箱内任意 Shell 暴露：

- `hdc` 设备查询、安装、启动和日志读取；
- 模拟器创建、启动和停止；
- 签名证书、Keychain 和发布令牌；
- 应用市场发布、Git push 和其他远端写操作；
- 打开宿主应用、系统设置或任意 URL handler。

Broker 请求必须包含 `run_id`、`tool_call_id`、精确动作、目标、影响摘要、审批策略和幂等键。审批只能授权该次规范化请求，不能授权一段可变化的 Shell 字符串。

## 8. 安全测试门禁

`sandbox-adversarial` 套件至少覆盖：

- 文件路径与链接逃逸；
- SSH/云凭据/环境变量读取；
- DNS、公网、回环、局域网和 socket；
- fork bomb、资源洪泛和孤儿进程；
- 恶意包管理器 lifecycle script、构建脚本和 MCP server；
- approval 重放、call id 混淆、TOCTOU 和 backend 降级；
- 沙箱销毁后的迟到写入和 side-effect recovery。

发布门槛：默认模式逃逸成功数为 0；所有拒绝都有稳定错误码与审计事件；同一攻击在 Windows、macOS 和 Linux 支持矩阵中分别报告。

## 9. 下一步实现顺序

1. 建立 `SandboxBackend`/`SandboxSpec`/`SandboxCapabilities`，不改变现有执行路径；**已完成策略模型、能力声明和 OCI argv 构造器，backend 生命周期接口待补。**
2. 增加 OCI backend，并以能力探测方式接入；
3. 将 `run_command` 的 build/test/shell 路径默认路由到 OCI backend；
4. 建立 Host Capability Broker，先迁移 `hdc`/deploy；
5. 为各平台增加轻量本机 backend；
6. 当支持矩阵和逃逸门禁通过后，再把 UI 中的“临时副本试运行”升级为“沙箱执行”。

相关文档：[工具故障隔离](TOOL_ISOLATION.md)、[MCP 项目授权](MCP_PROJECT_AUTHORIZATION.md)、[演进路线](AGENT_EVOLUTION_ROADMAP_2026.md)。
