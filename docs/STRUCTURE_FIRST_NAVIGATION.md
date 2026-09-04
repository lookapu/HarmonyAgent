# Structure-first 代码导航设计

> 状态：MVP 已接入 `search_symbols`；全库文件目录和结构节点已使用独立 SQLite 持久化、索引和分页；Tree-sitter/LSP、物理分片和关系边仍在后续阶段。

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
- 后续补充稳定 `symbol_id`、`language`、`index_revision`、`source_layer` 和置信度。

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

MVP 的范围估算仍是轻量规则扫描。大括号出现在字符串/注释、多行签名、宏、复杂 ArkTS/TypeScript/Python 语法时，`end_line` 可能退化或近似。当前 coverage 因此明确标记为 best-effort；达到 4,000 文件硬上限时标记 partial，而不静默声称全库完整。

## 5. 百万仓演进

### 5.1 文件目录层

先为每个允许访问的文件建立持久目录记录：`file_id/path/language/size/hash/mtime/module/shard/index_state`。Git tracked files、ignore 规则与 watcher 共同维护目录，查询前不再全量 walk。

超大源码也必须在目录中可达；可以暂不建立全文倒排，但不能从目录和 coverage 中消失。

目录层第一版已经完成：所有未忽略文件流式写入独立 SQLite，不把全量路径堆进内存；记录 `indexed/deferred/oversized/unsupported/symlink/unreadable` 状态和一级目录 shard。`find_files` 优先查询该目录，支持状态过滤和分页；结构查询同时报告发现、解析、延期、超大和不可读数量。结构节点也已进入同一仓库数据库，按 file、role/kind、name 和 shard 建索引，`search_symbols` 优先执行数据库过滤、排序、计数与分页，内存列表只作为兼容 fallback。

原生跨平台 watcher 也已接入：第一次建立索引时懒启动，忽略纯访问事件，将编辑器常见的 create/modify/remove/rename 事件按 200 ms 合并。普通文件变化现在会在缓存锁外用 SQLite 事务直接 upsert/delete 全库目录记录，并只替换对应文件的结构节点，不再安排全仓 walk；目录级变化、数据库失败、系统 `Rescan` 标志、空路径事件或监听错误才把一致性校验延迟到下一次真实查询。目录代次还会对全量节点重建做 revision fencing，避免并发外部编辑被较旧的扫描结果覆盖。稳定仓库不再每 30 秒反复 walk；watcher 启动失败时保留 30 秒周期扫描，active 时仍每 5 分钟低频校验，防止监听器“创建成功但不投递”或静默失效。最多保留 16 个项目 watcher，按最近使用淘汰，避免系统句柄泄漏。

watcher 不是唯一真相：网络文件系统可能没有事件，Linux 可能达到 inotify watch 限额，大目录也可能发生事件队列溢出。索引查询现在还会比较 Git HEAD/index 指纹；HEAD 变化时通过 `git diff --name-only -z --no-renames` 精确枚举 checkout/rebase 涉及的旧、新路径，直接复用文件级增量更新；Git 不可用、输出异常、单次超过 20,000 路径或 8 MiB、以及无法确定旧 tree 的 index-only 变化才回退一致性扫描。[notify 官方文档](https://docs.rs/notify/latest/notify/)也明确要求在 `need_rescan` 时重建内存状态，并提示大型目录可能漏事件。

当前机器的 10k 合成源码复测中，全目录发现 10,000 个文件，冷扫描约 266 ms、热查询约 0 ms；结构层解析 4,000 个并明确报告 6,000 个 deferred。该结果证明目录不再静默截断，但不等于已经通过 100k/1M 验收；后两档必须在物理分片和 watcher 完成后正式发布数据。

### 5.2 语法结构层

使用 Tree-sitter/编译器解析器增量提取实体和逻辑，轻量扫描只做容错 fallback。按文件或模块 shard 保存，单文件变化只替换对应文档和关系边。

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

1. 在现有 shard 字段与索引之上验证 100k/1M 数据量，再按测量结果决定何时拆成物理数据库；补充结构关系边表。
2. 引入 Tree-sitter 增量语法树，并把当前轻量规则保留为解析失败时的 fallback。
3. 让 `read_file` 接受结构查询返回的区间/后续 `symbol_id`，补强 hash 冲突保护。
4. 接入 Tree-sitter/ArkTS 容错解析，替换 MVP 的范围估算。
5. 建调用、状态和测试关系边，再实现统一 `repo_query` planner。
6. 用 10k/100k/1M 基准和真实任务轨迹持续验证召回率、延迟与上下文节省量。
