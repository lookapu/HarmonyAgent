# Android、Web 与 TypeScript 迁移建议

`search_api` 支持迁移模式：传入 `source_platform=android|web|typescript` 和 `concept`，返回绑定当前工程与本机 SDK 的 HarmonyOS 迁移建议。

示例参数：

```json
{"source_platform":"android","concept":"SharedPreferences","product":"default"}
```

```json
{"source_platform":"web","concept":"fetch"}
```

```json
{"source_platform":"typescript","concept":"Node fs"}
```

## 当前模式库

Android 覆盖 Activity/Fragment/Intent/Navigation、SharedPreferences/DataStore、Room/SQLite、Retrofit/OkHttp、BroadcastReceiver/EventBus；Web 覆盖 localStorage/sessionStorage、fetch/XMLHttpRequest/Axios、WebSocket、History/Router；TypeScript 覆盖 Node fs、EventEmitter、Worker/worker_threads。

规则描述的是架构语义，不宣称平台 API 一一对应。例如 Activity 与 Fragment 不会被机械替换成一个 HarmonyOS 类，浏览器 URL 历史也不会被直接复制成原生页面栈。

## 验证等级

每个 HarmonyOS 候选都会独立返回：

| 状态 | 含义 |
| --- | --- |
| `verified` | 当前本机 SDK 存在目标模块，规则要求的全部代表符号均已命中，且不高于工程编译 API |
| `conditional` | 本机定义存在，但引入版本高于 compatible API，需要运行时 API Level 守卫和低版本路径 |
| `unavailable` | 当前 SDK 中不存在模块，或目标高于 compile API |
| `unverified` | SDK 索引缺失、代表符号不完整，不能据此直接生成代码 |

证据同时列出本机 `.d.ts` 路径、Kit、引入版本、工程可用性，以及本地官方参考或 API 变更来源 URL。官方知识库缺条目时明确提示刷新，不用名称相似度伪造来源。

## 安全边界

- 迁移模式只返回建议，不写源码、依赖或清单；
- `unavailable` / `unverified` 候选不能作为代码生成依据；
- 不自动提高 compile/compatible API，不自动添加权限，也不复制 Android Context、浏览器 CORS、Node 主机文件系统等不成立的运行时假设；
- 数据迁移、网络重试、后台存活、跨线程传输和订阅生命周期必须按报告中的风险边界重新设计。

## 验证闭环

报告固定要求：读取本机声明 → 运行 LSP 诊断 → 运行 `check_sdk_alignment` 一致性审计 → `build_project` → 涉及设备能力时真机验证允许、拒绝与恢复路径。HM-25 将进一步把生成后的这组验证步骤编排为统一闭环。

## 验收

单元测试覆盖 Android SharedPreferences 到 Preferences 的本机 SDK/官方来源双重验证、未知平台拒绝与未知概念的可发现提示。阶段门禁包括工具 schema、Rust 全量测试、Worker 恢复、前端测试、lint、生产构建和 diff 检查。
