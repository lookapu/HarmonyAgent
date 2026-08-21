# HarmonyOS 阶段三验收记录

本文记录 `ROADMAP.md` 5.5 的实际执行证据。验收遵循“自动化能力通过不等于真实环境通过”：涉及真实多模块、签名、设备或多设备的条目，缺少对应环境时保持未完成。

## 验收环境

- 日期：2026-08-21（Asia/Shanghai）。
- DevEco Studio：`/Applications/DevEco-Studio.app`。
- HarmonyOS SDK：API 26，SDK 包版本 `26.0.0.23`。
- OHPM：`26.0.0.410`。
- 真实工程：`/Users/mac/sns/hongmeng-app`，Gitee 来源，验收提交 `cea3beda92dd13b8e7b1b968387e8f89069cda0b`。
- 工程 API：compile/compatible/target API 23。
- 设备：`hdc list targets -v` 返回 `[Empty]`，当前没有已连接真机或模拟器。

验收过程没有修改真实工程源码；原工程构建后 `git status --short` 为空。故障注入仅发生在 `/private/tmp` 下的一次性副本中。

## 结果总览

| 验收项 | 状态 | 结论 |
|---|---|---|
| 真实多模块工程关系 | 阻塞 | 真实工程只有 `entry` 一个声明模块，不能替代多模块实证 |
| ArkTS/Hvigor 诊断修复闭环 | 通过 | 在真实工程临时副本注入类型错误，准确定位、修复并重新构建成功 |
| 真机运行诊断闭环 | 阻塞 | 无在线设备，且当前产物未签名，不能安装 |
| SDK/API Level 不兼容诊断 | 通过 | 本地定义与官方变更联合诊断回归通过；真实 API 23 工程可由已装 SDK 工具链正常构建 |
| 多设备隔离与恢复 | 部分通过 | 独立结果和同产物成功设备防重放测试通过；缺少两台真实设备，尚未完成外部验收 |

## 真实工程结构与构建基线

真实工程解析结果：

- 根构建配置声明一个 `entry` 模块和默认 product。
- `entry` 是 HAP 入口模块，主元素为 `EntryAbility`，支持 `default` 与 `tablet` 设备类型。
- 页面 profile 声明 22 个页面路由；入口页最终跳转到 `pages/main/MainHomePage`。
- 清单仅声明 `ohos.permission.INTERNET`。
- 源码使用 Ability、ArkUI、ArkData、Network、CryptoArchitecture、BasicServices 等 Kit/API。

执行命令：

```text
/Applications/DevEco-Studio.app/Contents/tools/hvigor/bin/hvigorw --mode module -p product=default -p module=entry@default assembleHap --no-daemon
```

结果为 `BUILD SUCCESSFUL`，生成 `entry/build/default/outputs/default/entry-default-unsigned.hap`，大小 55,970,427 字节。构建同时给出既有 ArkTS 异常处理警告和 `No signingConfig found for product default`，因此该产物只证明编译/打包闭环，不证明可安装性。

真实工程的构建配置还存在签名敏感字段和机器绝对路径风险。验收没有读取、复制或记录字段值，也没有擅自改动外部工程；后续应以隔离凭据和显式审批单独治理。

## ArkTS 故障、诊断、修复与复验

在真实工程的新建临时副本中向 `Index.ets` 注入确定性类型错误：把字符串赋给 `number`。第一次构建在 `:entry:default@CompileArkTS` 失败，编译器给出 `10505001`，并定位 `Type 'string' is not assignable to type 'number'`。

删除该错误赋值后执行同一命令，结果为 `BUILD SUCCESSFUL`，HAP 重新生成。该闭环证明：

1. Hvigor 非零退出和 ArkTS 编译阶段不会被误判为成功；
2. 错误码、文件、行列、类型根因可直接形成修复依据；
3. 修复必须由同一构建目标重新验证。

## SDK/API Level 不兼容

执行回归：

```text
cargo test --locked maps_type_error_to_local_definition_and_official_change --lib
```

用例把 API 14 引入的本地声明放入 compile/compatible API 12 上下文，验证诊断同时输出：本机 `.d.ts` 定义、当前 API 不可用、官方引入版本，以及“改用当前编译 SDK 可用 API或升级 compileSdkVersion”“compatible API 更低时增加运行时守卫和回退”等恢复路径。结果 `1 passed; 0 failed`。

真实工程声明 API 23，而已安装工具链为 API 26；实际完整构建成功，未出现真实不兼容错误。因此这里通过的是“不兼容识别与替代建议能力”，不是声称该工程存在不兼容。

## 多模块与多设备边界

多模块语义模型回归：

```text
cargo test --locked parses_products_nested_modules_artifacts_abilities_and_dependency_edges --lib
```

结果 `1 passed; 0 failed`，覆盖 product、嵌套模块、HAP/HSP/HAR、Ability 和依赖边；但真实项目不是多模块，路线图对应条目继续保持未完成。

多设备恢复新增内容哈希门禁：读取当前 Run 及有限深度父 Run 的 `harmony.deploy.batch.completed` 事件，只跳过相同 HAP 哈希且状态为 `completed` 的设备；失败设备仍重试，HAP 变化则不复用旧成功证据。自动化测试覆盖“成功设备跳过、失败设备重试、产物变化全部重做”。没有连接至少两台设备前，多设备条目保持“部分通过”。

## 完成剩余验收所需条件

1. 提供或选择一个可安全构建的真实多模块 HarmonyOS 工程，用于核对入口、依赖、路由、权限和各模块产物。
2. 在 DevEco Device Manager 启动模拟器，或连接并授权真机；提供仅用于测试的有效签名配置。
3. 至少连接两台设备（可含模拟器），人为制造一台失败，恢复后确认成功设备没有重复安装。
4. 真机完成安装、Ability 启动、Hilog/异常采集、修复和重新验证后，再勾选剩余条目。
