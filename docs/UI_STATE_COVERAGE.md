# UI 状态覆盖规范（Q-04）

本规范定义 HarmonyAgent 前端**新增 UI 状态**必须覆盖的状态集合、页面声明约定与门禁校验方式，对应 `ROADMAP.md` 横向质量任务 `Q-04`。

## 目标

新增或修改页面时，状态处理不能只覆盖"数据正常"一条路径。任何承载异步数据或权限语义的页面，都必须显式考虑以下六类状态并给出对应处理；纯容器页面（状态全部由子组件承载）也必须显式声明，避免"未考虑状态"被误认为"不需要状态"。

## 六类状态定义

| 状态 | 含义 | 典型处理 |
|---|---|---|
| `loading` | 数据/操作进行中 | 加载指示、按钮禁用、骨架屏 |
| `empty` | 数据为空（首次/过滤后无结果） | 空态文案与引导操作 |
| `partial` | 批量/多源操作部分成功 | 汇总"成功 N / 失败 M"、逐项结果、失败不影响主面板的降级提示 |
| `failed` | 请求或操作失败 | 错误信息展示（禁止只 `console.error` 吞掉） |
| `retry` | 失败后可恢复 | 重试按钮/刷新入口，重试时清除错误态 |
| `permission` | 无权限/未授权/凭据无效 | 无权限提示、授权入口或跳转配置页 |

## 页面声明约定

`src/pages/*.tsx` 每个页面文件必须在**头部（前 30 行内）**声明状态覆盖清单：

```tsx
// @ui-states: loading, empty, failed, retry
```

- 状态名合法集合：`loading`、`empty`、`partial`、`failed`、`retry`、`permission`。
- 声明必须与代码一致：声明的每个状态在文件中必须有对应代码证据（模式清单见门禁脚本 `scripts/check-ui-states.py` 的 `STATE_PATTERNS`）。
- 纯容器页面（无 `useState`/`useEffect`，状态由子组件承载）声明为：

```tsx
// @ui-states: delegated
```

- 未声明、声明非法状态、声明与代码证据不符都会使门禁失败。

## 新增页面检查清单

1. 分析页面承载的数据/操作：哪些状态可能发生（至少评估 loading 与 failed）。
2. 为每个适用状态实现 UI 处理（对照上表）。
3. 在文件头部声明 `@ui-states`。
4. 本地运行 `python scripts/check-ui-states.py` 确认通过。
5. 批量操作页面必须实现 `partial`（逐项结果与失败项可重试）；涉及设备/凭据/授权的页面必须评估 `permission`。

## 门禁

- `scripts/check-ui-states.py`：扫描 `src/pages/*.tsx`，校验声明存在性、状态名合法性、声明与代码证据一致性；支持 `--self-test` 合成样例回归与 `--report` 审计报告。
- CI：`quality.yml` Frontend 步骤运行该脚本，任一页面未声明或不一致即阻断合并。

## 现状审计矩阵（2026-08-22）

| 页面 | loading | empty | partial | failed | retry | permission |
|---|---|---|---|---|---|---|
| ApiKnowledgePage | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| ConfigPage | ✓ | – | – | ✓ | ✓ | – |
| CostPage | ✓ | ✓ | ✓ | ✓ | ✓ | – |
| HealthPage | ✓ | – | – | ✓ | ✓ | – |
| Home | ✓ | ✓ | ✓ | ✓ | ✓ | – |
| KnowledgePage | ✓ | ✓ | – | ✓ | – | – |
| LanPage | delegated（状态由 LanPanel/LanTokenPanel 承载） | | | | | |
| LimitsPage | ✓ | ✓ | – | ✓ | – | – |
| McpPage | ✓ | ✓ | – | ✓ | ✓ | – |
| OhpmPage | ✓ | ✓ | – | – | ✓ | – |
| ProvidersPage | – | ✓ | – | ✓ | ✓ | – |
| ProxyPage | ✓ | – | – | ✓ | – | – |
| ReproductionBundlesPage | ✓ | ✓ | – | ✓ | ✓ | – |
| SkillsPage | ✓ | ✓ | – | ✓ | ✓ | ✓ |
| TeamSharingPage | ✓ | ✓ | – | ✓ | ✓ | – |
| VersionsPage | ✓ | ✓ | – | ✓ | – | – |

说明：

- 审计依据为声明与代码证据一致性校验（门禁脚本），"–" 表示该页面当前未声明/未实现该状态。
- 已知改进方向（不阻塞门禁）：OhpmPage 的失败处理目前以 catch 静默降级为主，建议补充失败提示与重试入口；ProvidersPage 缺少显式 loading 态；KnowledgePage 与 LimitsPage 建议补重试入口。
- 门禁保证"新增与修改"不再出现未声明的状态盲区；存量缺口按上述方向渐进补齐。
