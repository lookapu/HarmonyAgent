# HarmonyOS 统一设备状态

设备面板、Agent `list_devices`、默认设备选择和后续部署/诊断流程统一读取 `commands::devices::list_devices`，不再各自解析 `hdc list targets`。

## 快照字段

每台设备保留 hdc 的原始 `state`，并提供以下归一字段：

| 字段 | 口径 |
| --- | --- |
| `connection` | `online`、`offline`、`unauthorized` 或 `unknown`；不覆盖原始状态 |
| `authorized` | 只有 Connected/Ready/Online 才为 true；未授权与离线设备不执行 shell 探测 |
| `os_version` / `api_level` | 优先 `const.ohos.fullname`，回退版本和产品名，并单独保留数值 API Level |
| `architecture` | 优先 ABI 列表，回退主 ABI |
| `resolution` | 从 `wm size` 的 Physical size 提取，格式不可信时留空 |
| `capabilities` | 授权设备提供 shell/install/ability/hilog；截图、UI 自动化、诊断和性能能力需要对应命令或屏幕证据 |
| `observed_at` | 本次只读探测完成时的 Unix 秒时间 |

所有在线属性和命令能力并发读取；每条 hdc 调用都有真实超时，单项缺失只使对应字段为空，不伪造默认值，也不阻塞其它设备。

## 用户路径

- Workspace 设备卡直接展示系统版本、架构、屏幕和能力标签；原有详情、截图、应用、日志和性能面板保持兼容。
- Agent `list_devices` 回传原始状态、归一连接、授权和完整能力，后续工具可据此拒绝不受支持的操作。
- 多台在线设备仍要求显式指定目标；默认设备只影响未指定 `device` 的操作，不改变设备事实。

## 验收

单元测试覆盖目标行解析、在线/离线/未授权归一、分辨率和 API Level 的保守解析。阶段门禁包含全量 Rust、两组 worker 崩溃 E2E、前端测试、ESLint、生产构建与差异检查。
