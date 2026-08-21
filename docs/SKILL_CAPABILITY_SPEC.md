# HarmonyAgent Skill 与能力包规范 v1

第三方 Skill 的来源、分离签名、调用配额、熔断和审计另见 [第三方扩展供应链与运行治理](EXTENSION_GOVERNANCE.md)。

本规范定义 EC07 的稳定边界：Skill 是可安装的指令包，能力包是应用内置的最小工具选择策略。两者都必须独立版本化、声明兼容范围与权限上限，但声明本身不能扩大 HarmonyAgent 的实际授权。

## Skill manifest v1

Skill 入口必须是 UTF-8 `SKILL.md`。HarmonyAgent 在 YAML frontmatter 中识别以下字段：

```yaml
---
name: harmony-build
description: 构建并诊断 HarmonyOS 工程
harmony_agent_schema: 1
version: 1.2.3
harmony_agent_compat: ">=2.0.0,<3.0.0"
permissions:
  - project.read
  - project.write
  - process.exec
---
```

| 字段 | 规则 |
| --- | --- |
| `harmony_agent_schema` | 必填整数；当前只接受 `1`，未知版本拒绝导入 |
| `version` | 必填 SemVer `major.minor.patch`，预发布/构建后缀可存在但不参与当前兼容比较 |
| `harmony_agent_compat` | 必填；支持精确版本、`^x.y.z`，以及逗号连接的 `>= > <= < =` 约束 |
| `permissions` | 必填数组；没有额外权限时写 `[]`，未知权限拒绝导入 |

允许的权限标识为：

- `project.read`、`project.write`；
- `process.exec`；
- `network.read`、`network.write`；
- `device.read`、`device.write`；
- `secrets.read`；
- `release.publish`。

权限是最大需求声明，不是授权。Skill 调用的每个工具仍经过项目根、执行阶段、工具契约、审批模式、发布逐次审批和恢复策略；声明 `release.publish` 不会获得自动发布权限。

## 兼容与旧格式

安装/更新时记录 `manifest_schema`、Skill 版本、Agent 约束、权限 JSON、兼容状态和 `SKILL.md` SHA-256。状态只有：

- `compatible`：v1 清单有效且覆盖当前 Agent 版本；
- `incompatible`：清单有效但不覆盖当前版本，保存后保持禁用且不能手动启用；
- `legacy_unverified`：未声明任何 v1 字段的旧 Skill，可继续使用，但权限和兼容范围未经清单证明。

只要出现 `harmony_agent_schema` 或 `harmony_agent_compat`，就视为开始声明 v1，并必须补齐全部必填字段，禁止半声明后退回 legacy。外部生态自有的普通 `version`/`permissions` 字段不会单独激活本规范，避免误伤既有格式。安装后的 `SKILL.md` 在调用与上下文注入前重新解析并比较内容哈希；发生漂移时不注入指令、不记录调用，要求重新审核导入。旧安装记录没有历史哈希时保留兼容路径，并在界面与工具结果中显示未验证状态。

## 能力包 manifest

内置 `CapabilityPack` 使用独立元数据：

- `schema_version=1`；
- 每个包自己的 SemVer `version`；
- `min_agent_version`；
- `permission_ceiling=read_only|project_write|device_write|delivery`；
- 既有触发词、工具集合、推荐顺序、停止条件和验收条件。

当前能力包的权限上限：

| 能力包 | 权限上限 |
| --- | --- |
| `project_understanding` | `read_only` |
| `compile_fix`、`feature_development`、`refactor` | `project_write` |
| `build_deploy`、`device_diagnostics` | `device_write` |
| `git_delivery` | `delivery` |

能力包版本描述“选择策略协议”，工具自身版本与执行契约仍由工具注册表管理。修改工具集合、权限上限、停止条件或验收语义时至少提升能力包 minor；破坏字段或解释方式时提升 schema/major，并保留旧 Run 的解析路径。

## 生命周期

1. 导入：定位 `SKILL.md`，解析清单，计算哈希，保存兼容与权限事实；
2. 启用：incompatible 状态拒绝启用，legacy 明示风险；
3. 注入：只注入启用、兼容且哈希未漂移的内容；
4. 调用：`use_skill` 再次复验后记录使用，并回显版本、状态和声明权限；
5. 升级：不得把分支名当版本，后续 EC08 必须比较清单版本、展示权限差异并提供回滚；
6. 移除：删除数据库引用与无共享引用的目录，历史使用记录保留名称与时间。

## 非目标与边界

- v1 不把 Skill 指令当可执行代码，也不自动运行仓库脚本；
- 兼容声明只证明作者声明覆盖当前 Agent，不证明 Skill 正确、安全或适配某个 HarmonyOS API；
- HarmonyOS API/设备兼容仍由项目配置、本机 SDK、构建和设备证据验证；
- Git 来源、commit 固定、升级审批和回滚属于 EC08/EC10 的后续供应链治理。
