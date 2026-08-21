# HarmonyOS 真机韧性场景

HM-19 将设备离线、授权拒绝、安装冲突、权限拒绝、后台恢复和弱网纳入同一套“先决条件 → 故障/状态变化 → 业务断言 → 状态恢复 → Run 证据”验收口径。场景复用现有原子工具，避免隐藏的批量破坏操作。

## 场景矩阵

| 场景 | 执行与判定 | 恢复与证据 |
| --- | --- | --- |
| 设备离线 | `list_devices` 保留 hdc 原始状态并归一为 `offline`；部署、Ability、权限和网络工具在命令前拒绝非在线设备 | 不自动重连或改动设备；单元测试固定门禁行为 |
| 授权拒绝 | `authorized=false` 时统一拒绝；安装输出中的 unauthorized 独立归为 `device_authorization_denied`，不再误报签名失败 | 要求用户在已解锁设备确认调试授权，再以 `list_devices` 读回 |
| 安装冲突 | 部署前发现同包名时仅做 `-r` 覆盖；签名/更新身份冲突归为 `install_conflict`，版本降级归为 `version_downgrade` | 默认不卸载已有应用；只有本次首次安装且启动失败才补偿卸载 |
| 权限拒绝 | `grant_permission action=revoke` 显式撤销权限，运行目标 UI 流程，并从 UI 断言、Hilog 或 `permission_missing` 异常判断降级路径 | 记录 `harmony.permission.changed`；测试后按场景前状态显式 `grant` 或保持 `revoke`，不得猜测原状态 |
| 后台恢复 | `start_ability resume_after_background=true` 发送 Home、等待、重新拉起，并要求 Ability 栈确认前台 | 成功写入 `harmony.lifecycle.background_recovered`；失败保留应用现场供日志/UI 排查 |
| 弱网 | `set_network_condition` 选择实际在线接口，注入 weak/slow/lossy/custom 后用 `tc qdisc show` 确认真正生效，再运行 UI 断言 | 必须以 `mode=normal` 收尾并再次读回确认；两次状态均写 `harmony.network.condition` |

## 推荐执行顺序

1. 先用 `list_devices` 固定设备、授权和能力快照；离线与未授权场景只验证门禁，不主动断开 HDC，因为这会同时切断恢复通道。
2. 正常部署一次，再部署同一可信 HAP，验证覆盖安装路径不删除已有应用；需要复现签名冲突时使用专用测试设备，不在保存用户数据的设备上卸载。
3. 记录目标权限原状态，执行 `action=revoke`，启动/操作应用并用 `run_ui_flow` 断言拒绝或降级界面；随后显式恢复原状态。
4. 调用后台恢复模式，再用关键页面断言确认业务状态是否按产品约定保留。
5. 设置弱网，执行加载、重试、超时与缓存断言；无论业务断言成功或失败，最后都调用 `mode=normal`，并以读回证据为准。
6. 用当前 Run 时间线核对状态变化、部署/异常、UI 证据和恢复事件的真实顺序。

## 安全边界

- 不自动制造 HDC 离线或撤销调试授权，因为 Agent 会失去可靠恢复通道。
- 不以“解决冲突”为由自动卸载已有同包名应用；签名冲突可能包含不可恢复的用户数据。
- 权限命令和 `tc netem` 在 user 版设备上可能不可用，工具应明确失败，不把命令返回文本当成状态成功。
- 弱网影响设备上的所有应用；恢复失败时停止后续测试并提示人工关闭限速规则或重启网络。

## 验收

自动测试覆盖离线/授权/能力门禁、授权与签名/安装冲突分类、首次安装补偿边界、权限异常分类、网络接口与 qdisc 读回判定。真实设备验收还需逐项保存 UI/日志与 Run 事件；依赖 root/userdebug 的弱网场景若不可用，应记录为环境不支持而不是伪造通过。
