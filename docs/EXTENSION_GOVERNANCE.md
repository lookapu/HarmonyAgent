# 第三方扩展供应链与运行治理

本文定义路线图 EC10 的统一治理边界。Skill、MCP 和工作流模板继续使用各自的格式与权限模型，但来源、完整性、签名、配额、熔断和审计统一由 `extension_governance` 持久账本裁决。

## 真实性分层

系统不会把内容哈希或“签名能通过”夸大为可信发布者：

| 状态 | 含义 | 运行行为 |
| --- | --- | --- |
| `unsigned` | 记录了来源与 SHA-256，但没有完整分离签名 | 可在既有项目权限内运行，受配额和熔断约束 |
| `verified` | Ed25519 对规范载荷验证成功 | 证明签名私钥持有者签署了载荷；发布者身份仍为 `unresolved` |
| `invalid` | 签名字段不完整、编码错误或验签失败 | 失败关闭并隔离 |
| `drifted` | 运行时内容与登记摘要不一致 | 失败关闭，必须重新审核和导入 |

`signer_key_id` 是发布者声明，不是信任锚。EC10 不提供静默 TOFU，也不把扩展自带公钥自动加入信任库。未来团队信任库必须单独版本化，并要求显式管理员审批。

## 签名载荷与来源

- Skill 对 `SKILL.md` 原始字节验签。仓库可在技能目录放置 `SKILL.md.sig.json`，字段为 `source_uri`、`source_revision`、`algorithm=ed25519`、`signer_key_id`、`public_key_base64` 和 `signature_base64`。导入同时记录实际 Git commit，而不是只记录可移动分支名。
- MCP 对规范化的 `name`、`server_type`、`command`、`args` 和 `homepage` JSON 验签；签名证明启动配置未被替换，不授权工具、目录、网络或凭据。项目授权接口接受可选 `attestation`。
- 工作流对经过 schema 校验后的模板紧凑 JSON 验签；`attestation` 与模板分离传入，不进入可执行步骤。升级先验签，再归档和覆盖现有文件。

所有载荷同时保存 `sha256:` 摘要。旧 Skill/MCP 在迁移时以 `unsigned` 回填，保持可见和兼容；重新导入或授权后换成真实载荷摘要。

## 配额与故障隔离

默认每个扩展实例每分钟最多 60 次调用。连续失败 5 次后打开 60 秒熔断器；参数可通过治理 IPC 在受限范围内调整：

- `calls_per_minute`: 1—10000；
- `failure_threshold`: 1—100；
- `cooldown_seconds`: 1—86400。

窗口、连续失败次数和 `circuit_open_until` 保存在 SQLite，因此应用重启不会清除故障事实。实例按 `(extension_kind, extension_id)` 隔离：一个 MCP 或 Skill 失败不会使其它扩展退场。MCP 原有的进程内指数连接退避继续作为短周期资源保护，不能替代持久熔断。

Skill 在返回指令前重新解析 manifest 和内容哈希；漂移后立即隔离。MCP 在实际 `tools/call` 前同时复验项目授权与扩展配额。工作流管理动作使用同一账本；未来模板执行器必须复用该门禁，不能另建旁路。

## 审计与展示

登记、验签结果、调用成功/失败、限流、漂移和策略修改写入 `agent_audit_events`，资源键采用 `<kind>:<id>`。审计详情只保存来源、摘要、状态和有界错误，不保存私钥或环境变量值。

Skill 与 MCP 页面显示签名状态、速率上限和连续失败计数；`list_extension_governance` 提供完整只读状态，`configure_extension_governance` 调整实例策略。删除扩展时同步删除其治理状态，历史审计仍保留。

## 验收与恢复

- 修改已签名载荷后验签失败；Skill 导入后直接修改 `SKILL.md` 会进入 `drifted`。
- 达到分钟配额后新调用被拒绝；达到失败阈值后跨重启仍处于熔断期。
- 无效签名和漂移不能通过启用、MCP 重连或工作流升级绕过。
- 恢复方式是修复来源并重新导入/授权；不能直接清零失败或把状态改为 `verified`。
- 数据库迁移 `072_extension_governance.sql` 只新增表和索引，并为旧扩展显式回填 `unsigned` 状态。
