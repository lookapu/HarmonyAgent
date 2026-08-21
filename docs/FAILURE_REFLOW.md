# 失败样本回流流程

本文定义 `EC-16`：把真实失败样本脱敏后转化为固定评测回归场景的闭环流程。采集侧由 [问题复现包规范 v1](REPRODUCTION_BUNDLES.md)（`EC-12`）承担，回流工具为 `scripts/reflow_failure.py`，转化后的场景必须进入仓库版本化 fixture 并由生产内核执行。

## 1. 目标与非目标

目标：真实失败不会只停留在工单或对话里，而是沉淀为可重复、可追踪、防再犯的评测场景，并被 CI 基线门禁持续守护。

非目标：回流不是自动执行复现包（复现包 v1 不承诺自动复现），也不允许外部包注入可执行评测代码（与 [固定评测集](FIXED_EVALUATION_SUITE.md) 的注册场景约束一致）。

## 2. 回流流程

1. **采集**：真实失败经 `reproduction_bundle` 生成本地 ZIP（默认脱敏、显式确认、逐项摘要）。
2. **校验**：`reflow_failure.py --validate <zip>` 复验 manifest 版本/格式、条目数与载荷一致、逐项 SHA-256，并扫描凭据、私钥、证书与绝对用户路径泄露模式；任何一项失败都不得进入下一步。
3. **提炼**：`reflow_failure.py --draft <zip>` 从问题描述与工具调用中提取失败类别与错误签名，生成场景草案（`reflow_<domain>_<hash>`）。草案的 `expected` 为占位值，保证未实现执行器的场景失败关闭。
4. **注册**：把草案 id 与预期结论加入 `src-tauri/tests/fixtures/` 对应 JSON（版本化输入，变更必须连同生产策略、文档与测试评审），并在 `src-tauri/src/agent/evals.rs` 的 `simulate_harmony_scenario` 中实现执行器——执行器必须穿过与生产内核相同的解析、诊断、恢复或契约代码，禁止把 fixture id 直接映射到期望字符串。
5. **门禁**：`reliability_gate` 要求全部注册场景 100% 通过；CI 基线比较把场景数纳入覆盖指标，新增场景后旧基线自动跟随下次 main 运行，缩水才会阻断。

## 3. 类别映射

草案按错误签名归类，domain 与既有 fixture 对齐：

| domain | 代表签名 |
|---|---|
| `compile_repair` | `ArkTS:ERROR`、`ArkTSCheckError`、Hvigor 构建失败 |
| `device_diagnosis` | `SIGSEGV/SIGABRT`、CppCrash、faultlog、AppFreeze/ANR、部署/安装失败 |
| `recovery` | 重启、checkpoint、resume、会话丢失 |
| `idempotency` | duplicate、replayed、重复副作用 |
| `approval` | 审批超时、权限拒绝、未授权 |
| `tool` | 超时、重试、worker 崩溃/panic |
| `new_project` / `cross_module_change` / `runtime` | 工程创建、跨模块依赖、其它 |

## 4. 脱敏校验清单

`--validate` 拒绝以下载荷进入回流：

- manifest 版本、格式或条目数不匹配，条目 SHA-256 与内容不一致；
- 任何形式的私钥、证书、keystore、签名材料；
- 明文凭据字段（password/secret/api key/token 等赋值）；
- 未替换的绝对用户目录路径（`/Users/`、`/home/`、`C:\Users\`）。

校验通过不等于内容可公开：复现包导出时已按 [脱敏规则](DATA_REDACTION.md) 处理，回流时再次确认。

## 5. 本地命令

```bash
python3 scripts/reflow_failure.py --validate bundle.zip     # 0=通过；1=失败并列出缺项
python3 scripts/reflow_failure.py --draft bundle.zip        # 输出场景草案 JSON
python3 scripts/reflow_failure.py --check-id <id>           # 检查 id 是否已注册
python3 scripts/reflow_failure.py --self-test               # 合成包自测
```

## 6. 边界与承诺

- 回流场景的 `expected` 必须先由生产内核真实输出确认，不允许把占位值直接提交。
- 团队共享评测集（`EC-11`）只能组合本机已注册场景；共享包不承载回流代码，回流一律落仓库 fixture。
- 场景草案自动生成，执行器实现与期望裁决必须人工评审，回流不降低"完成必须由工具证据裁决"的标准。
