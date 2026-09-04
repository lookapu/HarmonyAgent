# Structure-first 代码导航设计

> 状态：MVP 已接入 `search_symbols`；全库目录、结构节点和结构关系已使用独立 SQLite 持久化、索引和游标分页；TS/TSX/JS/JSX 与 ArkTS 已接入 Tree-sitter，语法层 `extends/implements` 已落图，相对命名 import、根 `tsconfig` path alias、HarmonyOS `file:/link:` 本地包和有界命名 re-export/barrel 链可精确绑定；星号再导出、物理分片及调用关系仍在后续阶段。

## 1. 结论

“先看 Structure，再读目标代码块，必要时才读全文”可以落地，也适合作为大仓 Agent 的默认工作方式。它解决的是**上下文选择**问题：仓库可以有百万文件，但一次任务通常只需要少量实体、逻辑及其邻接关系。

它不能单独解决百万仓库问题。完整能力还需要全库文件目录、增量语法索引、分片存储、覆盖度与新鲜度、词法/LSP fallback，以及版本安全的窗口读写。

“索引全部文件”应理解为把可检索元数据持久化到本地索引，而不是把所有方法和文件正文一次性送进模型上下文。查询必须分页，正文必须按窗口读取。

## 2. 双层结构模型

面向 Agent 暴露两个稳定角色，同时保留语言原生 `kind`：

| 角色 | 典型 kind | 回答的问题 |
| --- | --- | --- |
| `entity` | class、component、interface、struct、enum、type、route、state | 系统里有哪些对象、页面、数据和边界？ |
| `logic` | function、method、constructor、handler、hook | 行为在哪里实现，应该读取或修改哪个代码块？ |

不能只保存“实体/逻辑”二分类。二分类适合 Agent 规划，但精确查询、重构和跨语言互操作仍需要原始 `kind`。

每个结构项至少包含：

- `name`、`role`、`kind`、`parent`；
- `file`、`line`、`end_line`；
- 单行 `signature`；
- 已包含 `language`、`source_layer` 和声明关系；后续补充稳定 `symbol_id`、`index_revision` 和解析置信度。

## 3. 默认访问协议

```text
用户目标
  -> search_symbols(role/query/file，分页)
  -> 选择候选实体或逻辑
  -> read_file(start=line, lines=end_line-line+1)
  -> 必要时读取相邻结构、调用者、测试或配置
  -> 只有满足全文条件时才分窗读取全文
```

适合直接读全文的情况包括：小型配置文件、跨多个结构共享的模块状态、生成代码调查、语法索引失败，以及必须理解完整顺序或副作用的脚本。大文件仍应分页，不因“全文”目标取消单次预算。

索引无结果、覆盖不足、结果陈旧或修改风险较高时，Agent 必须用 `codebase_search`、精确路径、LSP 或即时扫描补查，不能把“索引没返回”解释为“仓库中不存在”。

## 4. 本轮 MVP

现有 `search_symbols` 已兼容升级为结构入口：

- 支持 `role=entity|logic`、`kind`、`file`、`query` 组合过滤；
- 支持 `page` 和 `limit`，每页最多 200 项；
- 返回签名、父级和定义块的起止行；
- 返回匹配总数、下一页、已索引文件/结构数、新鲜度与 coverage；
- Agent 能力包和全局工具协议已调整为结构优先。

TS/TSX/JS/JSX/ArkTS 的范围来自 Tree-sitter AST；语法错误文件和 Rust/Python 等未接 grammar 的语言仍使用轻量规则扫描，其 `end_line` 在多行签名、宏或复杂语法下可能退化或近似。当前 coverage 因此明确标记为 best-effort；达到 4,000 文件首批解析预算时标记 partial，而不静默声称全库完整。

## 5. 百万仓演进

### 5.1 文件目录层

先为每个允许访问的文件建立持久目录记录：`file_id/path/language/size/hash/mtime/module/shard/index_state`。Git tracked files、ignore 规则与 watcher 共同维护目录，查询前不再全量 walk。

超大源码也必须在目录中可达；可以暂不建立全文倒排，但不能从目录和 coverage 中消失。

目录层第一版已经完成：所有未忽略文件流式写入独立 SQLite，不把全量路径堆进内存；记录 `indexed/deferred/oversized/unsupported/symlink/unreadable` 状态和一级目录 shard。`find_files` 优先查询该目录，支持状态过滤和分页；结构查询同时报告发现、解析、延期、超大和不可读数量。结构节点也已进入同一仓库数据库，按 file、role/kind、name 和 shard 建索引，`search_symbols` 优先执行数据库过滤、排序、计数与分页，内存列表只作为兼容 fallback。

结构图关系层也已落地：根据解析器明确给出的 `parent` 生成 `contains`（实体 → 方法/逻辑）关系，并从 AST heritage clause 生成 `extends/implements` 声明边，按起点、终点和 shard 建索引；结构查询会返回与当前页节点相连的边。同文件中名称唯一的实体会立即绑定到精确行号；相对命名 import（含别名和 ArkTS `import lazy`）、根 `tsconfig.json` 的 `baseUrl + paths` 精确/单星号映射，以及离源文件最近的 `oh-package.json5` 中 `file:/link:` 本地依赖加目标包明确 `main` 入口，都会在查询时根据当前目录与符号表确认唯一目标。命名 re-export 证据单独写入 SQLite，支持别名桶文件和最多 8 层的 barrel 链；查询会检测循环，任一层多目标即保持未解析。目标新增、删除、桶文件或清单改指向、以及 deferred 解析完成后均不要求重写全库入边。多个同等 alias 规则、多个映射文件或多个同名实体一律保持未解析。远程 ohpm 包、没有明确 `main` 的本地包、越出仓库的路径和尚未支持的 `export *` 不会猜测绑定。其他尚未经过名称解析的类型目标使用空文件和 0 行号显式标记为“语法声明，目标待解析”。单文件外部变化会在同一增量流程中重建源文件节点、声明边与再导出证据；按导入目标名建立的部分索引支持映射变化后的反向关系重新确认。当前仍不使用正则猜测 `calls`。

原生跨平台 watcher 也已接入：第一次建立索引时懒启动，忽略纯访问事件，将编辑器常见的 create/modify/remove/rename 事件按 200 ms 合并。普通文件变化现在会在缓存锁外用 SQLite 事务直接 upsert/delete 全库目录记录，并只替换对应文件的结构节点，不再安排全仓 walk；目录级变化、数据库失败、系统 `Rescan` 标志、空路径事件或监听错误才把一致性校验延迟到下一次真实查询。目录代次还会对全量节点重建做 revision fencing，避免并发外部编辑被较旧的扫描结果覆盖。稳定仓库不再每 30 秒反复 walk；watcher 启动失败时保留 30 秒周期扫描，active 时仍每 5 分钟低频校验，防止监听器“创建成功但不投递”或静默失效。最多保留 16 个项目 watcher，按最近使用淘汰，避免系统句柄泄漏。

watcher 不是唯一真相：网络文件系统可能没有事件，Linux 可能达到 inotify watch 限额，大目录也可能发生事件队列溢出。索引查询现在还会比较 Git HEAD/index 指纹；HEAD 变化时通过 `git diff --name-only -z --no-renames` 精确枚举 checkout/rebase 涉及的旧、新路径，直接复用文件级增量更新；Git 不可用、输出异常、单次超过 20,000 路径或 8 MiB、以及无法确定旧 tree 的 index-only 变化才回退一致性扫描。[notify 官方文档](https://docs.rs/notify/latest/notify/)也明确要求在 `need_rescan` 时重建内存状态，并提示大型目录可能漏事件。

2026-09-04 在当前 Apple Silicon/macOS 开发机用真实小文件复测：10k 档生成文件约 809 ms，目录扫描、4,000 文件结构解析及 SQLite 节点/边写入合计约 715 ms，结构精确查询约 15 ms，单文件目录+节点+边增量更新约 11 ms；100k 档对应约 8.7 s、3.2 s、14 ms 和 29 ms。100k 档目录完整收录 100,000 个文件，首屏结构解析按预算明确报告 96,000 个 deferred。结果表明当前单 SQLite 在 100k 目录量级尚无需立刻物理拆库。以上是开发态单机基线，不是跨平台 SLO。

同日增加了不创建海量实体文件的 SQLite 结构图基准，用来把“文件系统遍历/语法解析”和“持久图容量/查询”分开测量。最终 1M 档包含 1,000,000 条文件目录、1,250,000 个结构节点和 250,000 条 `contains` 关系：开发态单事务冷写入约 26–27 s，数据库约 572 MiB，精确名称定位约 1–2 ms，单节点邻接关系约 1–5 ms；按 `kind` 浏览到 90% 位置的第 18,000 页（每页 50 条），传统 `OFFSET` 在多次冷/热缓存测量中约 229–1,068 ms，而相同位置的 keyset cursor 续页约 0–2 ms。对应查询已增加名称索引、`kind/file/line/name` 覆盖排序索引和执行计划回归测试；关系总量改为事务触发器维护的 O(1) 计数，避免每页扫描全边表。

`search_symbols` 现在第一页同时返回兼容的 `next_page` 和推荐的 opaque `next_cursor`；后续原样传回 cursor 后，以 `(file, line, name, row_id)` 从复合索引直接 seek，不再扫描前面所有页，也不重复计算总数。游标绑定项目、查询条件、精确/模糊匹配模式和结构索引 revision：换过滤条件、游标损坏或 watcher/渐进解析在翻页期间更新了结构库时会拒绝续读，并要求从第一页重建视图，避免静默遗漏或重复。旧的 `page/limit` 协议继续可用。

生成型百万实体文件验收也已完成：1,000 个 shard 中实际创建并登记 1,000,000 个 ArkTS 文件，生成约 82.4 s，全库目录登记、首批 4,000 文件解析及节点/边写入约 49.0 s；SQLite（含 WAL/SHM）约 223 MiB，进程峰值 RSS 约 24.5 MiB，单文件目录+节点+边增量约 227 ms。128 文件渐进批次在人为持有写锁约 80 ms 的情况下总耗时 343 ms，其中可观测锁等待 88 ms；取消在第 33 个文件检查点返回约 4 ms，没有提交已取消批次。为避免每批对近百万 deferred 候选重新排序，文件目录新增 `(state, shard, path)` 覆盖顺序索引；这使数据库相对未加索引时增加约 61 MiB，但换来了稳定的批次领取和取消延迟。详细原始数据见 [大仓索引基线](./INDEX_SCALE_BASELINE.md)。

这组数据验证的是单库结构图的容量和查询路径，不等同于 100 万真实源码文件的完整冷扫描，也没有给出符号召回率。当前决策是暂不引入物理多库分片：Agent 常用的“精确结构 → 邻接关系 → 窗口读取”已保持毫秒级，而深页 `OFFSET` 已接近 1 秒，应先改成稳定游标/keyset 分页；只有在真实 1M 仓库的写放大、数据库锁竞争或单库体积越过目标 SLO 后，再按 module/shard 拆物理库。

deferred 渐进解析现已接入：首次结构查询快速返回基础 4,000 文件后，每个仓库最多启动一个后台 worker，以 128 文件为一批从 SQLite 领取任务；源码读取和解析发生在事务外，提交时再次核对 path、size、mtime 和 `state='deferred'`，避免覆盖 watcher 已处理的外部变化。每批只提交对应节点与结构边；指纹漂移或数据库异常会停止任务并请求一致性扫描。后续全目录校验会保留指纹未变的后台成果，不会重新降级。进程退出时无需排空任务，SQLite 中的状态可在下次查询后继续推进。

后台治理也已补齐：批次耗时低于 75 ms 时让出 20 ms，随后按 50/100/200 ms 分级增加背压，避免慢盘或复杂源码持续争抢前台 CPU/IO；`search_symbols` 会报告 active 状态、本轮提升文件数、批次数、剩余文件、最近批次耗时和当前背压。清除索引会设置取消令牌并解除 worker 注册，worker 在批次开始、每个文件解析前和写事务前检查取消，防止旧任务回写已作废结果。

### 5.2 语法结构层

使用 Tree-sitter/编译器解析器增量提取实体和逻辑，轻量扫描只做容错 fallback。按文件或模块 shard 保存，单文件变化只替换对应文档和关系边。

Tree-sitter 语法层已经扩展为固定兼容的 `tree-sitter 0.24.7`、`tree-sitter-typescript 0.23.2` 和 [`tree-sitter-arkts 0.2.0`](https://github.com/harmony-contrib/tree-sitter-arkts)。前者覆盖 `.ts/.tsx/.js/.jsx` 中的 class、interface、type alias、enum、function、generator、method、interface method signature 和绑定到变量的 arrow/function expression；ArkTS grammar 覆盖 `.ets` 的组件 `struct`、方法、状态装饰器和 ArkUI 扩展语法。节点范围直接使用语法树起止位置，因此字符串、注释或装饰器不会再破坏方法范围和声明定位；类、接口和组件成员会记录可靠 parent，并继续生成 `contains` 边。

每个节点新增 `language` 和 `source_layer=tree_sitter|lightweight`，Agent 输出会展示来源，不能把 fallback 结果冒充 AST 事实。支持语言只有在语法树无错误时采用 Tree-sitter；解析失败时整文件回退原轻量扫描，Rust/Python 等未接 grammar 的语言也继续使用 fallback。SQLite 启动时兼容增加 provenance 和声明关系列；parser schema version 从旧版本升级时清空旧节点、把已解析文件恢复为 deferred，并配合磁盘缓存版本升级重新建立首批结构，避免悄悄复用旧轻量结果。

fixture 回归覆盖多行接口/类方法、ArkTS 组件/状态装饰器与 `import lazy`、字符串内大括号、箭头函数、parent 归属、JS/JSX/TSX 入口、语法错误 fallback、`extends/implements` 声明边、相对命名 import/别名证据持久化和旧 SQLite 自动迁移。当前仍不能称为完整语义索引：声明关系只代表语法事实，跨文件引用还没有编译器级绑定。

10,000 个实体 `.ts` 文件的同机基准中，冷目录与首批 4,000 文件 AST 解析/写入约 0.95 s，产生 8,000 个 Tree-sitter 节点且没有 fallback；峰值 RSS 约 23.7 MiB，单文件增量约 12 ms。该结果只证明首批 grammar 的吞吐未突破既有资源边界，不替代真实代码的召回率评测。

10,000 个生成型 `.ets` 文件复测中，冷目录与首批 4,000 文件 ArkTS AST 解析/写入约 0.99 s，产生 12,000 个 Tree-sitter 节点、4,128 条结构关系且没有 fallback；峰值 RSS 约 27.1 MiB，128 文件渐进批次约 135 ms（含约 95 ms 人为锁等待），单文件增量约 11 ms。

### 5.3 关系层

在结构节点之间逐步加入：

- `contains`、`imports`、`extends`、`implements`；
- `calls`、`reads_state`、`writes_state`、`emits`；
- `navigates_to`、`tested_by`、`configured_by`。

这时 Structure 才会从“符号列表”升级为代码结构图，支持修改影响面和多跳上下文组装。

### 5.4 精确语义层

优先复用 ArkTS LSP 和各语言 LSP/SCIP 数据获得精确定义、引用和类型；不可用或超时时回退语法索引与词法检索。每条结果注明来源，避免把近似结果冒充编译器级事实。

### 5.5 读写闭环

- 读取：支持 `symbol_id` 或行区间窗口，并返回文件版本和游标；
- 修改：优先对结构块做带 `expected_hash` 的锚点 patch；
- 校验：重新解析受影响文件，检查结构是否仍存在，再运行静态检查、测试和构建；
- 大范围机械修改：在隔离工作树中用受限脚本执行，通过 diff 审查，不让模型逐文件复制全文。

## 6. 验收标准

- 对 10k/100k/1M 文件仓库随机抽样，所有未排除文件均有目录记录；
- 精确符号查询报告 Recall@5/20，不能只报延迟；
- 查询结果始终带 coverage、staleness/source revision 和可引用行区间；
- 单文件修改后结构查询 P95 在目标机器上小于 1 秒；
- 首次索引未完成时 10 秒内可渐进查询，并能指出哪些 shard 尚未就绪；
- 从结构定位到窗口读取，不把无关文件正文注入模型；
- 版本冲突时拒绝覆盖，并提示重新查询/读取。

## 7. 下一实现顺序

1. ~~为结构浏览增加稳定游标/keyset 分页，消除深页 `OFFSET` 的线性扫描，并保留现有页码接口作为兼容层。~~ 已完成。
2. ~~在生成仓运行渐进解析 1M 验收，记录冷扫描吞吐、写放大、锁等待、峰值内存和取消延迟。~~ 已完成；仍需补充真实混合语言 monorepo 的 Recall@5/20 与任务轨迹验收，达到单库 SLO 边界时再启用物理分片。
3. ~~引入 Tree-sitter 语法树，并把当前轻量规则保留为解析失败时的 fallback。~~ TS/TSX/JS/JSX 与 ArkTS、`extends/implements` 声明关系、同文件唯一目标、相对命名 import/别名/ArkTS `import lazy`、根 `tsconfig` path alias、`oh-package.json5 file:/link:` 本地包入口及有界命名 re-export/barrel 链均已完成；下一步评估保守的 `export *` 闭包与调用关系。
4. 让 `read_file` 接受结构查询返回的区间/后续 `symbol_id`，补强 hash 冲突保护。
5. 建调用、状态和测试关系边，再实现统一 `repo_query` planner。
6. 用 10k/100k/1M 基准和真实任务轨迹持续验证 Recall@5/20、延迟与上下文节省量。
