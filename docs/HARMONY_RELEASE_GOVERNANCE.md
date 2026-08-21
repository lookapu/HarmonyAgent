# 鸿蒙发布与签名治理

发布不是普通构建的别名。HarmonyAgent 将 release 构建、签名复用、证书导入、OTA 出包、凭据读取和应用市场发布视为独立安全域：操作必须可归因、凭据不进入工程或审计记录、失败后不自动重放。

## 每次显式审批

以下调用无论当前是 `allow_all`、`auto`、`ask` 或 `first_write`，也无论项目/会话是否已经加入白名单，都必须为本次调用重新确认：

| 操作 | 触发条件 | 恢复策略 |
| --- | --- | --- |
| release 构建 | `build_project mode=release` | 复验产物，不复用本次审批 |
| 签名参考 | `create_harmony_project copy_signing_from=...` | 重新确认来源和材料边界 |
| OTA 出包 | `ota_pack` | 人工检查输出，不自动重放 |
| 凭据读取 | `secret_get` | 不缓存审批 |
| 发布/签名命令 | `run_command` 命中 `ohpm/npm publish`、`signhapsigner`、`packagingtool` 或 AppGallery 模式 | 人工检查外部状态 |
| 后续专用能力 | `sign_hap`、`certificate_import`、`app_market_publish` | 默认 L2、每次确认、人工恢复 |

普通 debug 构建不因此升级。应用市场专用发布工具当前尚未开放；如果未来接入，必须先满足同一门禁，不能借用通用网络或 MCP 工具规避审批。

## 凭据隔离

`copy_signing_from` 不是凭据迁移器。它只允许读取本次授权根内的参考工程，并按字段白名单复用：

- 配置级：`name`、`type`；
- 材料级：`certpath`、`profile`、`storeFile`、`keyAlias`、`signAlg`；
- 材料文件必须位于参考工程目录内，符号链接解析后也不能越界；
- 任意层出现 password、passwd、pwd、token、secret、credential 或 private-key 语义字段，整次复制直接失败；
- 未知字段默认丢弃，不因新工具链字段出现而扩大权限。

允许的路径只是材料引用，不等于 Agent 持有签名凭据，也不保证 release 构建可签名。密码和私钥访问应由 DevEco Studio、操作系统钥匙串或独立凭据服务在运行时提供。工程文件、模型上下文、工具结果和持久审计均不得保存它们。

## 审批与审计展示

审批弹窗展示完成操作判断所需的参数，但 JSON 字段和自由文本先经过统一脱敏。`profile_path`、password、token、certificate、device serial 等敏感值不会原样进入前端事件；待恢复交互和最终工具审计使用相同脱敏口径。

审批只授权当前一次调用，不代表：

1. 产物已通过签名、完整性或市场规则校验；
2. 可以在失败、中断或进程恢复后自动重放；
3. 可以向其他项目、设备、证书或市场账号扩展授权；
4. 可以把凭据写入日志、命令行、配置、记忆或复现包。

## 验证边界

自动化回归覆盖参数级门禁、allow-all/白名单不可绕过的决策入口、凭据字段拒绝、未知字段白名单和工程外材料拒绝。实际发布仍需用户核对应用身份、版本、证书有效期、发布轨道、隐私合规和市场回执；没有外部回执时不得声称“发布成功”。
