# 大仓索引基线

> 状态：百万实体文件基准已执行
> 更新日期：2026-09-04

## 1. 目的

该基准用于客观记录当前结构索引在不同文件规模下的文件生成、全库目录登记、首批结构解析、渐进批次、缓存查询、单文件精确失效和取消性能。`MAX_FILES = 4000` 是首批解析预算，不再是文件可达上限；其余源码以 `deferred` 状态进入持久目录，由后台分批提升。

基准默认标记为 ignored，不进入普通 CI，也不会在项目目录生成数据。临时仓库创建在系统临时目录，完成后删除。

## 2. 运行方式

从仓库根运行：

```bash
HARMONY_INDEX_BENCH_FILES=10000 \
  cargo test --manifest-path src-tauri/Cargo.toml --lib \
  services::symbol_index::tests::large_repo_baseline \
  -- --ignored --exact --nocapture
```

依次运行三个规模：

```bash
HARMONY_INDEX_BENCH_FILES=10000 cargo test --manifest-path src-tauri/Cargo.toml --lib services::symbol_index::tests::large_repo_baseline -- --ignored --exact --nocapture
HARMONY_INDEX_BENCH_FILES=100000 cargo test --manifest-path src-tauri/Cargo.toml --lib services::symbol_index::tests::large_repo_baseline -- --ignored --exact --nocapture
HARMONY_INDEX_BENCH_FILES=1000000 cargo test --manifest-path src-tauri/Cargo.toml --lib services::symbol_index::tests::large_repo_baseline -- --ignored --exact --nocapture
```

可用 `HARMONY_INDEX_BENCH_EXT=ts`（也支持 `ets/tsx/js/jsx`）切换 Tree-sitter 语法层，并通过输出中的 `tree_sitter_symbols/lightweight_symbols` 验证来源；缺省为 `ets`，现使用 ArkTS 专用 grammar。

百万文件会消耗较多时间、inode 和临时磁盘，只应在专用基准机运行。首次开发验证建议使用 10,000 文件。

## 3. 输出

测试输出中查找单行 `HARMONY_INDEX_BASELINE=<json>`。schema v4 主要包含：

- `requested_files`：生成的文件数；
- `configured_max_files`：当前索引硬上限；
- `indexed_files`：实际进入符号结果的文件数；
- `catalog_discovered_files/deferred_source_files`：完整目录覆盖和待渐进解析数量；
- `cold_index_ms`：全库目录登记、首批解析及节点/边写入时间；
- `warm_query_ms`：持久结构精确查询时间；
- `single_file_incremental_ms`：单文件精确失效到可查询的时间；
- `progressive_batch_ms/progressive_lock_wait_ms`：128 文件批次总耗时与 SQLite 写锁等待；
- `cancellation_latency_ms/cancellation_checks`：取消被观察并安全返回的延迟和检查次数；
- `database_bytes/peak_rss_kib`：SQLite（含 WAL/SHM）占用和进程峰值 RSS；
- 平台和 CPU 架构。

## 4. 当前基准的边界

- 生成的是尺寸相近的 ArkTS 源文件，不代表真实 monorepo 的目录、语言、文件大小和依赖分布；
- 只测符号索引，不测 `codebase_search` 的文件/行 Recall@k；
- warm 查询走持久节点索引；
- 单文件增量走工具主动失效路径，不代表 watcher 或 Git checkout；
- 峰值 RSS 来自进程级 `getrusage`，会包含测试运行时本身；当前仍不采集 CPU time、P95 或符号 Recall@k；
- 渐进批次前人为持有 SQLite 写锁约 80 ms，用来确认锁等待可观测且有界；
- 百万文件完成后的临时目录清理耗时不计入 `generation_ms/cold_index_ms`。

## 5. v2.1.1 最新记录

运行环境：macOS / Apple Silicon（`aarch64`），2026-09-04。该记录用于验证数量级和发现瓶颈，不用于跨机器性能承诺。

| 文件 | 完整目录 / 首批解析 | 生成 | 冷目录+首批 | 渐进批次（锁等待） | 取消 | 单文件增量 | DB | 峰值 RSS |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 10,000 | 10,000 / 4,000 | 0.84 s | 0.84 s | 136 ms（96 ms） | 7 ms | 12 ms | 7.4 MiB | 25.5 MiB |
| 100,000 | 100,000 / 4,000 | 8.22 s | 4.57 s | 142 ms（66 ms） | 6 ms | 30 ms | 27.6 MiB | 27.6 MiB |
| 1,000,000 | 1,000,000 / 4,000 | 82.44 s | 48.99 s | 343 ms（88 ms） | 4 ms | 227 ms | 223.4 MiB | 24.5 MiB |

Tree-sitter 接入后的 10,000 个 `.ts` 实体文件复测：生成约 0.86 s，冷目录+首批 4,000 文件 AST 解析及写入约 0.95 s，得到 8,000 个节点且 `tree_sitter_symbols=8000/lightweight_symbols=0`；128 文件渐进批次在约 90 ms 人为锁等待下总计 120 ms，取消约 3 ms，单文件增量约 12 ms，峰值 RSS 约 23.7 MiB。这说明首批 TS AST 精确范围没有改变当前渐进索引的数量级；真实混合语法和 Recall@5/20 仍需独立验证。

ArkTS 专用 grammar 接入后的 10,000 个 `.ets` 实体文件复测：生成约 0.82 s，冷目录+首批 4,000 文件 AST 解析及写入约 0.99 s，得到 12,000 个节点、4,128 条结构关系且 `tree_sitter_symbols=12000/lightweight_symbols=0`；128 文件渐进批次在约 95 ms 人为锁等待下总计 135 ms，取消约 3 ms，单文件增量约 11 ms，峰值 RSS 约 27.1 MiB。该生成样本验证解析吞吐和来源完整性，不代表真实 ArkUI 项目的语法召回率。

原始输出：

```json
{"architecture":"aarch64","cancellation_checks":33,"cancellation_latency_ms":4,"cancelled_batch_promoted":0,"catalog_discovered_files":1000000,"catalog_source_files":1000000,"cold_index_ms":48987,"cold_symbols":16000,"configured_max_files":4000,"coverage":"partial_996000_source_files_deferred_by_parse_budget","database_bytes":234254336,"deferred_after_progressive_batch":995872,"deferred_source_files":996000,"generation_ms":82444,"incremental_symbols":4,"indexed_files":4000,"indexed_relations":4128,"peak_rss_kib":25040,"platform":"macos","progressive_batch_files":128,"progressive_batch_ms":343,"progressive_lock_wait_ms":88,"requested_files":1000000,"schema_version":4,"single_file_incremental_ms":227,"structure_parse_is_partial":true,"warm_query_matches":1,"warm_query_ms":1,"warm_query_page_items":1}
```

## 6. 后续升级

下一阶段应在不改变 schema 既有字段含义的前提下增加：

- manifest/Git 文件发现时间；
- watcher 与 Git checkout 修复延迟；
- 真实混合语言 monorepo 的分片索引进度和渐进覆盖率；
- lexical/AST/LSP/SCIP 各层查询 P50/P95；
- file/line Recall@5/20；
- 峰值内存、CPU time 和索引磁盘占用。

百万级目标采用“全库可达、按需读取”：目录和索引覆盖全部合规文件，但每次读取仍按行、字节或符号块分页，并返回游标与文件版本。单次限制用于保护上下文和内存，不能表现为文件永久不可访问。机械式跨文件修改应在隔离工作树中批量执行，再以 diff 和测试验证，而不是把所有文件全文送入模型。

相关路线见 [Agent 能力演进路线](AGENT_EVOLUTION_ROADMAP_2026.md)。
