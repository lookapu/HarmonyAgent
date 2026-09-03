# 大仓索引基线

> 状态：Phase 0 基准入口  
> 更新日期：2026-09-03

## 1. 目的

该基准用于客观记录当前符号索引在不同文件规模下的冷启动、缓存查询和单文件精确失效性能。它不会把当前实现描述成百万级可用：现有 `MAX_FILES = 4000` 仍会截断结果，报告中的 `truncated_by_current_limit` 会明确显示这一事实。

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

百万文件会消耗较多时间、inode 和临时磁盘，只应在专用基准机运行。首次开发验证建议使用 10,000 文件。

## 3. 输出

测试输出中查找单行 `HARMONY_INDEX_BASELINE=<json>`。schema v1 包含：

- `requested_files`：生成的文件数；
- `configured_max_files`：当前索引硬上限；
- `indexed_files`：实际进入符号结果的文件数；
- `cold_index_ms`：失效缓存后的首次索引时间；
- `warm_query_ms`：冷却期内内存结果查询时间；
- `single_file_incremental_ms`：单文件精确失效到可查询的时间；
- `truncated_by_current_limit`：结果是否被当前硬上限截断；
- 平台和 CPU 架构。

## 4. 当前基准的边界

- 生成的是尺寸相近的 Rust 源文件，不代表真实 monorepo 的目录、语言和文件大小分布；
- 只测符号索引，不测 `codebase_search` 的文件/行 Recall@k；
- warm 查询命中两秒冷却窗口，主要衡量内存克隆成本；
- 单文件增量走工具主动失效路径，不代表 watcher 或 Git checkout；
- 当前基准不采集峰值 RSS、CPU time 或磁盘缓存大小。

## 5. v2.1.1 首次记录

运行环境：macOS / Apple Silicon（`aarch64`），2026-09-03。该记录用于确认基准入口和当前硬上限，不用于跨机器性能承诺。

| 请求文件 | 实际索引文件/符号 | 冷索引 | warm 查询 | 单文件精确失效 | 截断 |
| ---: | ---: | ---: | ---: | ---: | --- |
| 10,000 | 4,000 | 234 ms | < 1 ms | < 1 ms | 是（`MAX_FILES=4000`） |

原始输出：

```json
{"architecture":"aarch64","cold_index_ms":234,"cold_symbols":4000,"configured_max_files":4000,"generation_ms":790,"incremental_symbols":4000,"indexed_files":4000,"platform":"macos","requested_files":10000,"schema_version":1,"single_file_incremental_ms":0,"truncated_by_current_limit":true,"warm_query_ms":0,"warm_symbols":4000}
```

## 6. 后续升级

`INDEX-02` 应在不改变 schema 既有字段含义的前提下增加：

- manifest/Git 文件发现时间；
- watcher 与 Git checkout 修复延迟；
- 分片索引进度和渐进覆盖率；
- lexical/AST/LSP/SCIP 各层查询 P50/P95；
- file/line Recall@5/20；
- 峰值内存、CPU time 和索引磁盘占用。

相关路线见 [Agent 能力演进路线](AGENT_EVOLUTION_ROADMAP_2026.md)。
