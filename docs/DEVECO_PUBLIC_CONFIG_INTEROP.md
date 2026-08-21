# DevEco 公共配置互操作

HarmonyAgent 与 DevEco Studio 通过工程中的公开 HarmonyOS/Hvigor/OHPM 配置互操作，不读取或依赖 IDE 的窗口、缓存、最近文件、索引等私有状态。这样同一 checkout 可以在 DevEco 中打开，也能在无 GUI 的命令行、CI 或恢复任务中得到一致的 product、模块、SDK、依赖与构建结论。

## 公共构建契约

互操作报告对以下文件做存在性检查并生成确定性 SHA-256 指纹：

- `AppScope/app.json5`；
- 根和模块 `build-profile.json5`；
- 根和模块 `oh-package.json5` / `oh-package-lock.json5`；
- 根和模块 `hvigorfile.ts`；
- `hvigor/hvigor-config.json5`；
- 各模块 `src/main/module.json5`。

指纹绑定文件相对路径与内容，用于判断 DevEco/CLI 两次观察是否来自同一公开配置。单文件超过 2 MiB 时不进入指纹，避免异常配置造成无界读取。

报告同时复用 `HarmonySemanticModel` 输出 product、HAP/HSP/HAR 模块，并判断工程 Hvigor wrapper 或外部 DevEco/Command Line Tools 是否足以支持命令行复现。

## 明确排除的私有状态

- `.idea/**`：只报告目录是否存在，不读取任何内容；
- `local.properties`：只报告文件是否存在，不读取值，也不进入公共指纹；
- DevEco 的窗口布局、最近文件、索引、运行面板和用户缓存；
- 签名口令、令牌、证书内容和私钥。

修改 `.idea/workspace.xml` 或 `local.properties` 不会改变公共配置指纹。HarmonyAgent 的构建、恢复和验收不能以这些文件作为唯一事实。

## 可移植性与敏感配置检查

公开 JSON5 配置会做键级扫描：

- 字符串为 Unix 绝对路径、UNC 路径或 Windows 盘符路径时，只输出其 JSON 字段路径；
- 字段名包含 password、token、secret 等敏感标记时，只输出字段路径；
- 值不会进入报告、日志或文档。

发现机器绝对路径时，应改用相对路径、环境变量或隔离凭据引用。签名配置属于 EC-06 的显式审批与凭据隔离范围，EC-05 只负责发现风险，不自动搬运或改写材料。

## 使用方式

调用：

```json
{
  "path": "/path/to/harmony-project"
}
```

`environment_check` 会在 SDK 来源与工程 API 对齐之后输出 `[DevEco 公共配置互操作]`，包括公共配置指纹、product/模块、Hvigor 来源、CLI 可复现状态、被忽略的私有状态、机器路径/敏感字段路径和风险。

验收顺序仍以 CLI 为准：OHPM 依赖核对、Hvigor 构建、产物清单；需要设备时继续安装、Ability 启动和运行证据。DevEco 打开或 Sync 成功不能替代这些后置条件。
