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

当前实现进度：`src-tauri/src/agent/sandbox.rs` 已提供 `SandboxBackend`/`OciBackend`、固定 digest 的 OCI 安全 argv、wall-time/取消处理、命名容器清理、输出预算和运行事件；同时提供稳定的原生/OCI capability 清单与 Tauri 探测入口，环境管理页以能力卡片展示真实状态和失败原因。macOS/Linux 探测会实际启动无副作用的最小隔离域，Windows AppContainer 在 token/profile/ACL 生命周期完成前保持 unavailable；探测不会下载镜像或执行用户命令。`run_command` 在显式设置 `HARMONY_SANDBOX_BACKEND=native` 时会进入 macOS `sandbox-exec` 或 Linux bubblewrap：使用干净环境、工作区定向挂载/规则、独立临时目录和默认断网；未知后端值、域名 allowlist、Windows 未实现路径及隔离模式下的后台任务均失败关闭。

能力报告把资源治理拆成 `wall_time_limit`、`output_limit`、`cpu_limit`、`memory_limit`、`pids_limit` 和 `writable_tmp_limit`。兼容字段 `resource_limits` 只有在六项全部强制时才为 `true`。原生后端当前只声明父进程真实执行的墙钟和输出上限；不会用 POSIX `RLIMIT_NPROC` 冒充任务级进程树隔离，也不会把尚未实现的 CPU、内存和临时盘配额显示为可用。环境管理页会同时展示已强制项和缺失项。

环境管理页还可手动调用 `verify_native_sandbox_boundary`。该命令只在应用内部创建 UUID 临时测试根目录，不读取或修改用户项目，并实际验证工作区写入、工作区外读取拒绝、只读工作区写入拒绝和符号链接逃逸拒绝。报告固定标记为 `filesystem_smoke_v1`；原生运行时不可用或任一检查失败时不会给出通过结论。该 smoke 不验证网络、凭据、进程树或资源限制，不能替代各平台的完整逃逸套件。

真机或 CI 可用同一实现生成机器可读报告：

```bash
cargo run --manifest-path src-tauri/Cargo.toml --features eval-cli --bin harmony-agent -- sandbox verify --json
```

全部检查通过时退出码为 `0`；运行时不可用或任一边界未建立时为 `1`；参数或内部错误为 `2`。调用方不得忽略非零退出码，也不得在失败后回退到宿主执行。该命令不需要 Docker，不会尝试拉取镜像。

原生路径尚未设为默认，且完整跨平台逃逸套件仍待宿主/CI 验证，因此缺省产品能力仍按第 2 节的宿主执行边界对外说明。

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

当前接线进度：`connect_device` 的连接/断开，以及单设备 `deploy` 和 `deploy_all` 的 HAP 安装/ability 启动已经通过 `execute_host_capability` 执行。Broker 只生成固定 `hdc` argv；HAP 在执行前 canonicalize，并再次确认是工作区内普通文件，符号链接逃逸失败关闭；bundle/ability 使用受限标识符语法。`host_capability.started|finished|rejected` 进入 Durable Run 事件，设备目标只记录 SHA-256 短摘要，校验失败不记录恶意原参数。上层工具审批、按工作区/设备的并发门禁、批量部署恢复和启动后存活验证保持不变。

这仍是部分实现：设备查询/日志、模拟器、签名与发布尚未迁移；Broker 事件目前从上层工具审计继承 tool call 身份，尚未把 `tool_call_id` 和独立幂等键固化进自身请求结构，因此不得宣称第 7 节契约已经全部完成。

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

1. 建立 `SandboxBackend`/`SandboxSpec`/`SandboxCapabilities`，不改变现有执行路径；**已完成策略模型、能力声明、原生/OCI 真实探测入口和 OCI argv 构造器。**
2. 增加 OCI backend，并以能力探测方式接入；**已完成，保持显式可选，不作为桌面默认。**
3. 实现 macOS/Linux 原生执行 argv 与 Windows AppContainer 生命周期，逐平台验证工作区、断网和资源边界；**macOS/Linux argv、显式 `run_command` 路由、干净环境和后台绕过门禁已完成；Windows 与完整资源限制待完成。**
4. 建立 Host Capability Broker，先迁移 `hdc`/deploy；
5. 逃逸套件通过后，将 `run_command` 的 build/test/shell 默认路由到当前平台原生 backend；
6. 当支持矩阵和逃逸门禁通过后，再把 UI 中的“临时副本试运行”升级为“沙箱执行”。

相关文档：[工具故障隔离](TOOL_ISOLATION.md)、[MCP 项目授权](MCP_PROJECT_AUTHORIZATION.md)、[演进路线](AGENT_EVOLUTION_ROADMAP_2026.md)。
