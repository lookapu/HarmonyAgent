# 当前状态单页（2026-09-18）

> 本页是**当前能力的唯一状态口径**：某项能力「实现了没有、证据在哪、边界是什么」看这里。
> 历史过程与逐批发现记录在 [主线实现与验收盘点](./MAINLINE_AUDIT_2026-09-14.md)，两者不重复：
> 盘点文档是日志，本页是快照。改代码后请同步更新本页对应行，而不是继续往盘点里追加小节。

## 1. 验证记录（本轮实测，命令可复现）

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| 后端库回归 | `cargo test --manifest-path src-tauri/Cargo.toml --lib` | 1,127 通过、10 忽略（总计 1,137；macOS 本机，2026-09-18 拉取 `6c8fbbe` 后实测） |
| 后端库回归（Windows 本机，2026-09-18 本轮） | 同上 | 1,125 通过、0 失败、9 忽略（较 1,102 基线 +23 条新用例：正文接口/HTML→Markdown、目录树查表、SemVer 解析、种子修订号补入） |
| 鸿蒙文档抓取联网验证（Windows 本机，2026-09-18 本轮） | `cargo test --manifest-path src-tauri/Cargo.toml --lib -- --ignored` 中的 `harmony_api_diff::tests::e2e_fetch_parse_store_search`、`harmony_api_ref::tests::e2e_fetch_parse_store_query` 与 `harmony_api_ref::tests::e2e_catalog_index_resolves_hms_modules` | 通过（实抓 26.0.0 Release 的 `js-apidiff-basicserviceskit-7003`、`@ohos.batteryInfo` 参考页，目录树命中 `@hms.*`；解析→入库→查回全链路；见盘点 §45/§46） |
| Worker 崩溃恢复 | `cargo test --manifest-path src-tauri/Cargo.toml --test worker_crash_e2e` | 3 通过 |
| Tool Worker 崩溃恢复 | `cargo test --manifest-path src-tauri/Cargo.toml --test tool_worker_crash_e2e` | 3 通过 |
| 后端库回归（Windows 本机，设备预览批次 2026-09-18） | 同上 | 1,139 通过 / 0 失败 / 10 忽略（较上一批 +10 条预览用例：引擎参数、帧格式、WebSocket 帧解析、产物定位、默认页解析） |
| 设备预览端到端（Windows 本机，2026-09-18） | `cargo test --manifest-path src-tauri/Cargo.toml --lib -- --ignored` 中的 `previewer::tests::e2e_preview_frames_from_real_engine`（需 `DEVECO_PREVIEW_E2E_PROJECT`） | 通过（真实工程 → 构建 → 起引擎 → 取回 JPEG 帧；见盘点 §54） |
| 前端测试 | `npm test` | 15 文件、133 通过（含 `previewPanel.test.tsx`：启动/停止、帧渲染、失败不静默、卸载解绑与停引擎） |
| 前端 lint / 类型 | `npm run lint`、`npx tsc -b` | 通过 |
| Web 构建与体积门禁 | `npm run build` | 通过（Home 697.0/750KB、Markdown 1500.8/1550KB、index 547.5/575KB） |
| Rust 编译告警 | `cargo check --lib`、`cargo test --no-run` | 0 警告（macOS + Windows 本机） |
| 文档漂移门禁 | `python3 scripts/check-docs.py` | 通过 |
| clippy 基线门禁 | `python3 scripts/check-warnings.py` | 通过（57/57，仅结构类告警；macOS + Windows 本机均通过——8 条只在 Windows 触发的机械类告警已修复，基线未调整） |
| Windows 质量矩阵 | GitHub Actions `quality.yml`（macOS + Windows） | v2.2.0 发版时全绿（此前 16 处 Windows 失败已清零） |

平台：macOS 15.7.9（arm64，Darwin 24G830）；另有 Windows 本机（2026-09-17 起用于编码/路径类缺陷复现，同日后端库全量：1,102 通过 / 8 忽略——与 macOS 的 1,107/9 差异来自平台门控用例集，不是回归）。拉取 `6c8fbbe`（文档抓取重建 + 目录树查表 + SemVer，与 Windows 侧同一批）后 macOS 侧复验：后端库 1,127/10、两组崩溃恢复各 3 项、`cargo check --lib` 0 告警、`check-warnings.py` 57/57、`check-docs.py` 通过、`check-ui-states.py` 通过；同批 Windows 侧为 1,125/9、三个联网 e2e 通过、其余门禁全绿——两平台用例数差异同样来自平台门控用例集。本批 push 后 CI（`Quality`）在跑，结论以 CI 为准。上表的 Windows 质量矩阵行证据来自 CI；**编码类缺陷已在本机 Windows + 随包 Temurin 17 复现并验证修复**（见第 3 节）。**未运行**：Docker/OCI、真实 Provider 模型、真机/模拟器、Windows/Linux 目标本机编译、安装包与签名验收。

## 2. 写入门禁覆盖矩阵

所有文件修改工具（`write_file`/`edit_file`/`multi_edit`/LSP WorkspaceEdit）在落盘前经过统一门禁：
先做通用配平守卫，再按语言升级到真实解析或外部检查；**外部工具缺席或超时一律降级为「未做该项检查」
并记录事件，不阻塞写入、也不冒充已校验**。

| 语言 | 语法层 | 语义/执行层 | 降级条件 |
| --- | --- | --- | --- |
| `.ets` | tree-sitter（ArkTS） | — | — |
| `.ts`/`.tsx`/`.js`/`.jsx` | tree-sitter | — | — |
| `.java` | tree-sitter + 声明/注解目标检查 | `javac` 差分（并入同源码根下未编辑的调用方；编译固定 `-encoding UTF-8`） | 无 javac、编译超时；源码非 UTF-8 编码时按 UTF-8 解码失真（本机实测：GBK 源码报 `unmappable character`），可能**误报拒写** |
| `.dart` | tree-sitter | `dart analyze` 差分 | 无 dart、不在包内、超时 |
| `.go` | tree-sitter | `go vet` 差分（临时模块 + **同包兄弟文件一起带上** + `GOPROXY=off`，超时 45s 以容纳冷缓存） | 无 go、不在模块内、包过大、只剩上下文缺失错误、超时 |
| `.py` | tree-sitter | `pyflakes` 差分 | 无 pyflakes/python3、超时 |
| `.rs` | tree-sitter | —（临时 crate 隔离会把同 crate 其它模块的引用误判为未定义，实测后回退，见盘点 §24） | — |
| `.kt`/`.kts` | tree-sitter（kotlin-ng） | —（本机无 kotlinc） | — |
| `.c`/`.h`/`.cpp`/`.cc`/`.cxx`/`.hpp`/`.hh`/`.hxx` | tree-sitter（cpp，C 的超集） | — | — |
| `.swift` | tree-sitter | — | — |
| `.sql` | 配平回退 | `sqlite3 :memory:` 执行差分（含副作用语句时拒绝执行） | 无 sqlite3、超限、含 ATTACH/VACUUM INTO/点命令、超时 |
| 其它 | 配平回退（显式标注 `delimiter_fallback`） | — | — |

## 3. 发布过程中修出的真实缺陷（v2.2.0）

以下三条**不是静态审查发现的**，是让代码真正跑在 macOS + Windows 双平台 CI 与发版流水线里才逼出来的；
每条都已修复，但都留下一条仍未闭环的边界，不再当作「只是验收没做」。

| 缺陷 | 现象 | 修复 | 残留边界 |
| --- | --- | --- | --- |
| javac 按平台默认编码读源码 | Windows 默认 cp1252，UTF-8 源码里的中文注释/字符串被误解码，产生**不存在的编译错误**，于是干净写入被 Java 门禁误拦（误报而非漏报） | `java_compiler.rs` 编译参数固定 `-encoding UTF-8`（另抽出 `javac_args`，用参数级用例钉住编码与诊断语言） | 源码本身非 UTF-8（GBK/Big5 等）时按 UTF-8 解码失真：本机 Windows + 随包 Temurin 17 实测为**误报拒写**（38 处 `unmappable character`，见盘点 §28）。门禁硬假定 UTF-8；该参数在随包 JDK 17 上是载荷性的，不能以「JDK ≥ 18 默认 UTF-8」为由删除 |
| OTA 参数作用域路径前缀不一致 | Windows `canonicalize` 返回 `\\?\C:\...` 逐字（verbatim）前缀，与普通路径比较永不相等 → **工作区内的合法 OTA 参数被判越界而拒批**（真实生产路径，不是测试问题） | `ota_scope.rs` 在根匹配与相对化两侧统一走 `normalize_path`（剥离 verbatim 前缀） | 只归一了前缀；符号链接、8.3 短名等其它 Windows 路径形态未专门覆盖 |
| 评测基线门禁从未真正比对基线 | env 变量写作 `src-tauri/target/eval-baseline.json`，而 `cargo test` 的工作目录已在 `src-tauri/`：写出的基线读不回、要对的基线读不到 → 门禁长期「产出但不比对」的假绿 | env 改为 `target/eval-baseline.json`，写前 `create_dir_all` 父目录 | 修好后才暴露它对 runner 计时抖动敏感，已改成取三次最优 + `duration_factor` 2.5（见盘点 §26）；同机耗时对比在繁忙 CI 上仍非绝对稳定 |

## 4. 能力状态

| 能力域 | 状态 | 主要证据 | 边界（未做/未验） |
| --- | --- | --- | --- |
| 目标与计划驱动 | 已实现核心链路 | `commands/chat.rs` 的 GoalContract、`activate_approved_plan`、计划继承；前端计划确认卡与测试 | 真实模型「计划→多步→中断→交付」完整轨迹未验 |
| 长会话恢复与执行治理 | 基础实现 + 本机回归充分 | `agent/kernel_executor.rs` 检查点/安全点、预算继承、两组 crash E2E | 桌面 IO adapter 未统一：迁移方案与两次更正见 [headless 驱动文档 §17/§18/§19](./HEADLESS_AGENT_DRIVER.md)（主循环体 305 行（原 2,107，口径见驱动文档 §20 的更正说明）、循环内 `.emit(` **0** 处、`.0.lock()` 1 处；**已实测否掉「先补 Tauri 测试替身」**：mock 需全仓 AppHandle 泛型化、代价更大；正确顺序是先抽取端口、再用假端口加行为快照，抽取期间靠编译器+回归+手动桌面验收把关）；**按段搬运已全部完成**，第 7 步进行中（状态已收进 `DesktopRoundState`，7/14 个段函数改收 `&mut DesktopRoundState`；剩余合段本身需桌面验收窗口）；真实长任务未验 |
| Headless Agent | 核心实现 | `HeadlessIoPort` 交给 `KernelIoRunLoop::run`；评测契约与桩端到端 | 真实 trial 的 manifest/trajectory/成本未产出 |
| 大仓索引可达性 | 已实现 + 历史基准 | `services/symbol_index.rs` 全库目录/延迟解析/watcher；`docs/INDEX_SCALE_BASELINE.md` | 真实混合仓全量收敛耗时、Recall@k、前台 P95 未测 |
| 结构查询与影响面 | 部分 | `repo_query` 的 `auto`/`impact` 分流、SCIP/LSP/AST 边、分页与覆盖状态 | 统一依赖重排 planner、跨语言正确率未做 |
| 按块修改与写入门禁 | 见第 2 节矩阵 | 结构句柄 v3、Java 字节级单/批量事务、多文件逆序条件回滚、各语言门禁真实入口测试 | Java 类型语义未闭环（无 JDT/Maven/Gradle）；Dart/Go/Python/SQL 均单文件，无跨文件/跨模块覆盖 |
| 工具治理与常驻预算 | 已实现 | `capabilities`/`tool_ranking`，生产排名后上限 20、固定保底入口、验证阶段保护必需工具 | 同模型 A/B 成功率未做；不是完整程序化编排 |
| 原生隔离与资源限制 | 部分 | `agent/native_limits.rs`（`RLIMIT_CPU` 实测生效）、`sandbox.rs` 原生后端、宿主直跑限额可用 `HARMONY_HOST_DIRECT_*` 显式开启 | macOS 内核不接受 `RLIMIT_AS`（内存限不了）；进程数/CPU 配额/临时磁盘总量未限；Windows 生命周期未实现；OCI 端到端需 Docker |
| Host Capability Broker | 已实现核心契约 | 审批凭据 v4（与能力无关）、执行期 fail-closed 复核、自描述撤销判定、影响契约进审批卡片与审计、13 个变更类能力的显式契约清单 | 效果证据（安装版本/设备出现）需真机；**闭环逻辑**（签发→复核→撤销→停止失效）已用生产函数跑回归，仅 guards/execute 的**实际调用点**因需 Tauri AppHandle 未覆盖 |
| OTA 审批安全链 | 核心链路实现 | 只读副本 + 内容摘要绑定 + 固定 argv + 持久撤销 + 发布前复验 | 真实打包/签名/升级未验；不支持工具版本矩阵 |
| HarmonyOS 工程与设备闭环 | 大量实现 | 工程/SDK/构建/部署/UI/性能工具与固定录制场景、沙箱边界 smoke | 真机/模拟器版本矩阵（离线、重连、安装冲突、恢复）未验 |
| 设备预览（驱动 SDK Previewer 取真实帧） | 已实现（Windows 本机验证） | `services/previewer.rs`（定位引擎/组装参数/切帧/取帧客户端）+ `commands/preview.rs`（预览构建 → 起引擎 → 帧推给前端）+ 预览面板；配方与实测见盘点 §52/§54 | 交互事件回传（点击/滑动送回引擎）、热重载、多设备档切换未做；macOS 侧未验；引擎缺失时退回 Web 预览（显式报错，不静默失败） |
| 鸿蒙官方文档 / API 知识库 | 已重建抓取链路 + 数据补齐 | `services/harmony_doc_api.rs`（`getDocumentById` 正文接口、`getCatalogTree` 目录树查表、HTML 表格/锚点/HTML→Markdown）；版本 diff 与 API 参考两条链路联网 e2e 通过；出厂种子库（空库重建）14 版本 / 1,014 页 / `api_docs` 44,856 行 / 向量 44,856 条 / 280MB；参考正文 681 页（库内 681 行 / 13,705 条成员），17 个人工取证定案的模块映射见盘点 §51/§53 | 参考候选未命中 127（模块级 25 个，经文档站搜索逐个确认无专属参考页；清单见盘点 §53）；目录树匹配依赖标题形态（末段消歧在标题改动后可能失效）；`refresh_api_db` 依赖华为接口形态，站点再变即失效 |
| 评测与发行 | 基础设施已建 | 固定 25-ID 清单、eval harness、release workflow、更新签名配置 | HarmonyBench 50/100+、真实 SWE 报告、平台签名/SBOM/provenance/新机验收未做 |
| 文档与 CI 门禁 | 全绿 | `check-docs.py`（数量/链接/CI 接口）、`check-warnings.py`（clippy 基线 57） | Windows/Linux 目标本机未编译验证（仅代码审查） |

## 5. 本机无法闭环的清单（不要把它们记成「只剩验收」）

- **真实设备**：安装/启动/UI/日志/停机恢复、Broker 效果证据、审批卡片真机验收。
- **真实模型**：任务成功率、常驻工具 A/B、eval 报告与成本。
- **Docker/Podman**：OCI 沙箱端到端与逃逸套件、artifact 导出。
- **签名与发行**：平台代码签名、公证、SBOM/provenance、干净机器安装验收。
- **Windows/Linux**：目标编译校验（本机只有 macOS 工具链；MinGW 只能覆盖 windows-gnu，不等于 CI 用的 MSVC）。
- **只在 CI 成立、本机不复现**：Windows 质量矩阵结论（16 处 Windows 失败清零）。**编码类门禁行为已于 2026-09-17 在本机 Windows 复现**（见第 3 节），从本清单移出；路径形态类（符号链接、8.3 短名）与 Windows/Linux 目标编译仍未覆盖。改动这些路径时须以 CI 结果为准，不能用本机绿推断。

## 6. 维护约定

- 能力状态变化时**只改本页对应行**，并在 [盘点文档](./MAINLINE_AUDIT_2026-09-14.md) 追加一条带提交号的记录（日志保持只增不改）。
- 新增语言门禁时同步第 2 节矩阵与 `docs/TOOL_ENHANCEMENTS.md` 的口径表。
- 发版过程中修出的缺陷写进第 3 节（含残留边界），不要在能力表里只写「已修复」。
- 版本相关事实（如 tree-sitter grammar 的 ABI 上限、`dart analyze` 输出格式）记录在代码注释与盘点文档，本页只写结论。
