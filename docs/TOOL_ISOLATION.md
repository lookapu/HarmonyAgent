# 工具执行隔离与故障注入

本文定义工具执行内核在卡死、取消失效、输出洪泛和子进程失联时必须保持的安全边界，并给出 TC-17 的自动化验收证据。

## 隔离边界

| 故障 | 隔离机制 | 必须保持的不变量 | 回归测试 |
| --- | --- | --- | --- |
| 工具线程卡死 | 每次原生工具调用进入命名的专用 OS 线程；调用方只等待有界时间 | 卡死线程不能阻塞 Tokio 调用方或 UI；超时后可继续调度其他工作 | `hung_execution_lane_times_out_without_blocking_caller` |
| 调用不可取消 | 调用方超时后标记 `stuck_detected`，租约失效后按副作用类型恢复 | 已失去租约的迟到结果不能把 `verification_required` 覆盖为成功 | `uncancellable_late_result_is_fenced_after_recovery` |
| 输出洪泛 | 后台任务仅保留有界尾部缓冲；单行也有独立上限 | 内存占用不随输出无限增长，最新诊断证据仍可读取 | `output_flood_stays_bounded_and_preserves_latest_evidence` |
| 孤儿进程 | 超时、显式终止和会话清理统一终止进程树 | 包装器和直接子进程均不得在会话清理后继续运行 | `conversation_cleanup_kills_parent_and_orphan_candidate` |
| 执行线程 panic | 线程内捕获 panic，并把调用交给 Durable Recovery | panic 不得终止宿主进程；写操作不能被盲目重放 | `panicked_execution_thread_is_isolated_and_reported` |

## 恢复语义

1. 读取工具可以进入 `recovery_required` 后安全重放。
2. 写入工具进入 `verification_required`，必须先读取真实状态再决定补做或收敛。
3. 破坏性或不可逆工具进入 `manual_review`，默认失败关闭。
4. 每次尝试由 `worker_id + lease_token + attempt` fencing；恢复或接管清除租约后，旧线程只能丢弃迟到结果。
5. `stuck_detected` 是故障归因，不是成功或失败终态；最终状态仍由副作用感知恢复协议决定。

## 进程与输出策略

- Unix 清理先终止直接子进程，短暂允许包装器 `wait` 回收，再兜底终止包装器，避免同时强杀造成僵尸 PID。
- Windows 使用 `taskkill /T /F` 终止完整进程树；普通任务被 drop 时至少通过 `kill_on_drop` 终止直接子进程。
- 后台任务输出保留最新尾部，缓冲不会超过 `2 × JOB_OUTPUT_CAP`；触发裁剪后收敛到约一个 `JOB_OUTPUT_CAP`。
- 超长单行只保留末尾 64KB，并显式写入省略字节数，避免没有换行的工具绕过总量保护。

## 验收命令

从 `src-tauri` 目录运行：

```bash
TAURI_CONFIG='{"build":{"features":[]},"bundle":{"resources":[]}}' cargo test --locked
```

Unix 孤儿进程用例会启动一个真实的 shell 包装器和 `sleep` 子进程，并在断言前执行会话清理；测试自身不会故意留下长生命周期进程。
