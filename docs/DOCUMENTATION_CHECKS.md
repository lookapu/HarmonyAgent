# 文档漂移校验（Q-08）

> 状态：已生效
> 适用范围：中英文 README、架构文档、路线图、CI 工作流与代码真源的一致性
> 入口：`scripts/check-docs.py`，已接入 `.github/workflows/quality.yml` 的 Documentation drift check 步骤（macOS/Windows 双平台）

## 1. 目标

文档中的数量、路径、接口与状态必须与代码真源一致，避免以下漂移再次发生：

- 新增工具、迁移、页面或模块后，README/ARCHITECTURE 中的计数停留在旧值。
- 文档链接指向已删除或更名的文件，路线图勾选条目引用的实现路径不存在。
- CI 工作流引用了不存在的测试或脚本，导致门禁永远无法真正生效或直接常红。

校验结果由工具证据裁决，不依赖人工核对：`check-docs.py` 每次运行都从代码真源重新提取数量，再与文档逐模式比对；任一漂移即退出码 1，CI 阻断合并。

## 2. 数量校验（真源 → 文档模式）

真源与文档模式的对应关系如下，新增/删除文件后运行一次 `python3 scripts/check-docs.py` 即可同步：

| 真源 | 提取方式 | 校验的文档模式 |
|---|---|---|
| 工具数 | `src-tauri/src/agent/tools/mod.rs` 中 `ToolSpec {` 计数 | 中英文 README、ARCHITECTURE、TOOL_ENHANCEMENTS，以及中文 TOOLCHAIN_ACCEPTANCE、TOOL_RESULT_V2、CHANGELOG |
| 迁移数 | `migrations/*.sql` 文件数与 `db/mod.rs` 注册数（两者必须相等） | 中英文 README、ARCHITECTURE、CHANGELOG，以及 VERSION_COMPATIBILITY |
| IPC 入口 | `lib.rs` 的 `generate_handler![...]` 块内 `commands::` 行数 | 中英文 README、ARCHITECTURE |
| 页面数 | `src/pages/*.tsx` 文件数 | 中英文 ARCHITECTURE |
| commands 模块数 | `src-tauri/src/commands/*.rs`（不含 `mod.rs`） | 中英文 README、ARCHITECTURE |
| services 模块数 | `src-tauri/src/services/*.rs`（不含 `mod.rs`） | 中英文 README、ARCHITECTURE |
| agent 模块数 | `src-tauri/src/agent/*.rs`（不含 `mod.rs`） | 中英文 README、ARCHITECTURE |
| tools 文件数 | `src-tauri/src/agent/tools/*.rs` 总数（含注册表）与减一（不含）两个口径 | 中英文 README、ARCHITECTURE、TOOL_ENHANCEMENTS |

校验脚本只识别显式数字模式（如 `**201 个 Agent 工具**`、`\| 数据库迁移 \| 75 \|`）；历史时间线记录（如 TOOL_ENHANCEMENTS 中"增至 201"）属于当时快照，不在校验范围。

## 3. 链接与路径校验

- `docs/*.md` 中的相对 Markdown 链接目标必须存在（外部 URL、锚点和图片链接除外）。
- ROADMAP 勾选条目中反引号内的代码/脚本/迁移路径（`.rs`、`.py`、`.sql`、`.json`、`.tsx`、`.ts`、`.md`）必须存在。
- `.github/workflows/quality.yml` 引用的 `agent::evals::tests::*` 测试名必须在 `evals.rs` 中存在，`--test <name>` 引用的集成测试文件必须在 `src-tauri/tests/` 中存在。
- `.github/workflows/release.yml` 引用的 `scripts/*.py` 必须存在。

## 4. 使用方式

```bash
python3 scripts/check-docs.py        # 校验当前仓库，退出码 0/1
python3 scripts/check-docs.py --self-test   # 合成仓库回归：篡改中/英文数字、删除链接目标、
                                            # 改坏 CI 测试名、引用不存在路径均必须被检出
python3 scripts/check-docs.py --repo <路径> # 校验其它仓库副本
```

`--self-test` 不访问真实仓库：在临时目录构造迷你仓库并逐项篡改，断言每次篡改都被检出，防止校验逻辑本身失效。

## 5. 接入与门禁

- `.github/workflows/quality.yml` 在 Frontend tests 之后、Lint/Build 之前运行 `python scripts/check-docs.py`（`actions/setup-python@v5` 固定 Python 3.12，双平台一致）。
- 校验失败时步骤退出非零，CI 合并被阻断；修复文档或真源后重跑。
- 文档新增数量性表述时，应在脚本中补充对应模式（真源 + 文档模式成对出现），避免新表述再次失去校验。

## 6. 演进规则

- 新增迁移、工具、页面、commands/services/agent 模块后，运行一次校验并按提示同步文档。
- 重命名或删除文档、脚本、测试时，运行校验确认无悬挂引用。
- 修改 CI 工作流中引用的测试名或脚本路径时，运行校验确认接口一致。
