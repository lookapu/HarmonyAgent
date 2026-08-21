# HarmonyOS 可恢复构建工作流

`build_project` 是 HarmonyAgent 的统一 HarmonyOS 构建入口。它按固定顺序执行：

1. `environment`：确认工程身份，解析统一语义模型，并解析可执行的 Hvigor 启动器与 SDK 环境；
2. `dependencies`：从语义模型读取所有模块的外部 OHPM 声明，核对模块级或根级 `oh_modules`，按策略安装并写后读验证；
3. `build`：在 Workspace 并发门禁内执行可选 clean 与 Hvigor 构建，流式保存完整日志；
4. `artifacts`：递归发现 HAP/HSP/HAR。Hvigor 返回成功但没有产物时，工作流仍判为失败。

## 参数与依赖策略

既有 `mode`、`module` 和 `clean` 参数保持兼容，并支持 `product`、`changed_files` 与 `dependencies`：

- `module` / `product`：显式指定时形成严格构建边界，并校验模块是否属于产品、模式是否受工程和模块支持；
- `changed_files`：未显式限定模块/产品时，从统一语义模型计算直接模块、依赖/import 反向闭包及受影响产品；
- `mode`：缺省 `debug`，显式 `release` 可用于发布验证；不在根或模块 build mode 中的值会在执行前拒绝；

- `auto`（默认）：仅在声明的外部依赖未出现在模块级或根级 `oh_modules` 时运行 `ohpm install`；
- `force`：无论当前安装状态如何都同步依赖；
- `skip`：明确跳过安装，适合离线或由外部流程管理依赖的场景；若发现缺失依赖会在日志中预警。

工作区内 `file:`/`link:` 依赖和已解析到本地模块的依赖不要求出现在 `oh_modules`，不会触发错误安装。

## 影响驱动的构建计划

构建执行前会生成并记录 `scope` 和目标列表，每个目标包含模块、产品、模式与 Hvigor task：

- HAP、HSP、HAR 分别使用 `assembleHap`、`assembleHsp`、`assembleHar`，不再把所有模块都误作 HAP；
- 变化从 HAR/HSP 传播到上游 HAP 时，只保留受影响依赖图中的顶层产物，由 Hvigor 在该闭包内构建底层模块，避免重复构建；
- 两个互不依赖的顶层产物或多个受影响产品会形成多个确定性目标，并按产品、模块路径稳定排序；
- 根配置等结构变化使用 `full` 影响范围，但仍按产品与顶层产物拆分，不退回无边界的全工程猜测；
- 没有 `changed_files` 时保持兼容，选择 default（或首个）产品的入口 HAP；显式 `module` / `product` 优先于自动规划。

目标集合进入 checkpoint workflow key，因此产品、模块、模式或影响计划变化后不会错误恢复旧构建。

## 产物清单

构建成功必须生成 `.deveco-agent/harmony-artifacts.json`；清单与 checkpoint 分离，包含 schema、生成时间、workflow key、工程指纹以及工作区内每个 HAP/HSP/HAR 的：

- 工程相对路径、类型、字节数、文件修改时间和本次发现时间；
- 对实际文件内容流式计算的 SHA-256；任一文件不可读时 artifacts 阶段失败，不写出残缺成功结论；
- 从最长模块路径和产品输出路径推导的模块、产品，以及与本次构建目标匹配后的 mode 和来源 step；无法唯一归属时保留 `unknown` / `workspace_discovery`，不猜测；
- `signing_status`：HAP/HSP 同时存在 `META-INF/*.SF` 与签名块时为 `verified_signed`，文件名明确含 unsigned 时为 `unsigned`，仅有 signed 文件名但缺少结构证据时为 `claimed_signed`，其余为 `unknown`；HAR 为 `not_applicable`。

这里的 `verified_signed` 表示归档内签名材料结构可验证存在，并不声称已完成证书链信任验证；部署侧仍需设备安装结果作为最终验收证据。文件名本身不能升级为已验证状态。

## Checkpoint 与恢复

工作流将脱敏后的状态写入 `.deveco-agent/harmony-build-workflow.json`，记录 schema、构建参数键、工程指纹、完成阶段、当前阶段、错误摘要和产物证据。

- 工程指纹覆盖 ArkTS/TS、配置、资源描述和 Native 源码，跳过依赖、缓存、构建产物和 Agent 自身状态；
- 只有参数键和工程指纹均一致，且上一次状态为 `running` 或 `failed`，才进入恢复模式；
- 工程源码、配置或构建参数变化后自动创建新流程，不复用旧结论；
- 环境始终重新确认，依赖阶段使用文件系统证据跳过已完成安装，构建阶段重新执行，避免把中断时的半成品当作成功；
- `completed` checkpoint 仅用于审计，不会让用户下一次主动构建被短路。

Checkpoint 写入失败不会遮蔽真实构建结果；构建失败仍由结构化错误与完整日志负责诊断。

## 结构化错误

Hvigor 与 ArkTS 日志统一解析为 `BuildError`：

- `file`、`line`、`column` 保存可定位源码位置，并兼容 Windows 盘符路径；
- `error_code` 提取方括号或 `Error Code:` 形式的数字/命名错误码，例如 `00303312`、`ArkTSCheckError`；
- `stage` 归一为 environment、dependency、configuration、compile、package、signing 或 build，并可从前序 Hvigor task 行继承；
- `category` 是根因类别，覆盖 type、syntax、dependency、ohpm、sdk、api_level、signing、resource 和 other；
- ArkTS 定位行没有内联消息时，会读取紧随其后的 `Error Message:`，再据此完成根因分类和建议。

`build_project` 的 Agent 错误信封会把 stage 与 error code 写入每条定位证据；Workspace 构建错误卡也展示相同字段，避免两端使用不同解析口径。

## 专项诊断

结构化错误之后会执行日志—模型联合诊断，并按置信度输出证据和顺序化恢复步骤：

| 诊断 | 日志证据 | 模型证据 | 恢复边界 |
| --- | --- | --- | --- |
| `dependency_conflict` | version/conflicting/resolve/peer dependency | 同一包在不同模块的约束或锁定版本分裂 | 先统一约束再受控同步；不自动猜版本 |
| `cache_corruption` | corrupt/integrity/checksum/unexpected EOF | 无需伪造模型结论 | 可先自动执行 Hvigor clean；禁止删除整个工程或 SDK |
| `sdk_missing` | SDK 错误类别、`DEVECO_SDK_HOME`、SDK not found | 各产品 compile/compatible/target SDK | 必须先补齐外部 SDK 条件，不用改源码掩盖 |
| `signing_failure` | signing/certificate/profile 失败 | 脱敏的签名材料完整度，不读取密码、私钥或路径 | 运行签名自检，需账号生成材料时交还用户 |
| `api_incompatible` | requires API / unsupported version | 各产品 compatible/target API Level | 优先替代 API 或版本守卫，谨慎提高最低版本 |

专项证据会进入 `build_project` 失败信封，恢复步骤追加在通用类别建议之后。日志证据在持久化或返回前统一脱敏；只有缓存损坏的保守 clean 被标记为可自动恢复，其余涉及版本选择、SDK、签名身份或产品兼容策略的决策不会静默执行。

## 验收

自动化测试覆盖参数策略校验、影响闭包到顶层产物的收敛、多产品目标、独立 HSP 任务、产品/模式拒绝、产物哈希/签名/产品/来源清单、相同指纹失败恢复、源码变化拒绝恢复、外部依赖缺失/安装证据、HAP 产物发现、ArkTS 跨行错误、命名/数字错误码和 Hvigor 阶段继承。全仓 Rust、崩溃 E2E、前端测试、lint 和生产构建作为阶段门禁。
