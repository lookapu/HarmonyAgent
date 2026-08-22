# HarmonyOS 阶段三验收记录

本文记录 `ROADMAP.md` 5.5 的实际执行证据。验收遵循“自动化能力通过不等于真实环境通过”：涉及真实多模块、签名、设备或多设备的条目，缺少对应环境时保持未完成。

## 验收环境

- 日期：2026-08-21 至 2026-08-22（Asia/Shanghai）。
- DevEco Studio：`/Applications/DevEco-Studio.app`。
- HarmonyOS SDK：API 26，SDK 包版本 `26.0.0.23`。
- OHPM：`26.0.0.410`。
- 真实工程：`/Users/mac/sns/hongmeng-app`，Gitee 来源，验收提交 `cea3beda92dd13b8e7b1b968387e8f89069cda0b`。
- 工程 API：compile/compatible/target API 23。
- 签名：DevEco Studio 自动签名（2026-08-22 生成，`~/.ohos/config/default_hongmeng-app_*.p12/.cer/.p7b`），debug 类型 profile，有效期 2026-08-22 至 2027-08-22，bundle `com.sns.harmony`。构建环境需 `JAVA_HOME=/Applications/DevEco-Studio.app/Contents/jbr/Contents/Home`（签名工具为 Java 实现）。
- 设备：华为 CHZ-AL00（HarmonyOS 6.1.0.135(SP8C00E126R2P4)），`hdc list targets` 返回 `6UNBB26507103971`（USB）。

验收过程没有修改真实工程源码；原工程构建后 `git status --short` 为空。故障注入仅发生在 `/private/tmp` 下的一次性副本中，验收后已删除。真实工程的 `build-profile.json5` 未被 git 跟踪，为完成签名构建在其中补了 product→signingConfig 引用（`"signingConfig": "default"`），不污染工程源码。

## 结果总览

| 验收项 | 状态 | 结论 |
|---|---|---|
| 真实多模块工程关系 | 阻塞 | 真实工程只有 `entry` 一个声明模块，不能替代多模块实证 |
| ArkTS/Hvigor 诊断修复闭环 | 通过 | 在真实工程临时副本注入类型错误，准确定位、修复并重新构建成功 |
| 真机运行诊断闭环 | 通过 | 真机完成签名构建、安装、启动、hilog 基线、故障注入、异常定位、修复与重新验证（见下文） |
| SDK/API Level 不兼容诊断 | 通过 | 本地定义与官方变更联合诊断回归通过；真实 API 23 工程可由已装 SDK 工具链正常构建 |
| 多设备隔离与恢复 | 部分通过 | 独立结果和同产物成功设备防重放测试通过；仍缺少两台设备，尚未完成外部验收 |

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

## 真机运行诊断闭环

2026-08-22 在华为 CHZ-AL00 真机（HarmonyOS 6.1.0.135）上完成全链路验收：

1. **签名构建**：DevEco 自动签名材料就绪后，`hvigorw --mode module -p product=default -p module=entry@default assembleHap --no-daemon` 产出 `entry-default-signed.hap`（63,836,095 字节，`SignHap` 通过）。前置修复：product 补 `signingConfig` 引用；`JAVA_HOME` 指向 DevEco 自带 JBR（缺 Java 时报 00308018）。
2. **安装**：`hdc install entry-default-signed.hap` 返回 `install bundle successfully`，签名与 debug profile 被系统接受。
3. **启动**：`hdc shell aa start -a EntryAbility -b com.sns.harmony` 返回 `start ability successfully`；进程存活（PID 7904/8017），前台 Mission 为 `com.sns.harmony:entry:EntryAbility`。
4. **日志基线**：`hilog -r` 清空后采集启动链路：`[AMC153]StartAbility` → sceneboard Session 创建 → 应用 `EntryAbility onCreate`/`onWindowStageCreate`/`Succeeded in loading the content.`，无异常。
5. **故障注入**（`/private/tmp` 一次性副本）：在 `EntryAbility.onCreate` 注入 `JSON.parse('{bad json')`（模拟统计开关配置解析 bug），签名重建并 `hdc install -r` 覆盖安装。
6. **异常定位**：启动后进程崩溃退出；hilog 捕获 `SyntaxError: Unexpected Object Prop in JSON`、`Error message:Unexpected Object Prop in JSON at position 1`（错误消息与注入输入特征吻合）、`com.sns.harmony is about to exit due to RuntimeError`，并上报 `[FRAMEWORK,JS_ERROR]` hisysevent。
7. **修复与重新验证**：删除注入代码，签名重建、重新安装、重新启动；进程存活（PID 11885），hilog 显示 `onCreate` → `onWindowStageCreate` → `Succeeded in loading the content.`，无任何崩溃日志。

该闭环证明：签名 HAP 可在真实设备安装运行；hilog/崩溃事件足以定位运行时异常根因；修复必须由同一安装→启动→日志链路重新验证。

## 多模块与多设备边界

多模块语义模型回归：

```text
cargo test --locked parses_products_nested_modules_artifacts_abilities_and_dependency_edges --lib
```

结果 `1 passed; 0 failed`，覆盖 product、嵌套模块、HAP/HSP/HAR、Ability 和依赖边；但真实项目不是多模块，路线图对应条目继续保持未完成。

多设备恢复新增内容哈希门禁：读取当前 Run 及有限深度父 Run 的 `harmony.deploy.batch.completed` 事件，只跳过相同 HAP 哈希且状态为 `completed` 的设备；失败设备仍重试，HAP 变化则不复用旧成功证据。自动化测试覆盖“成功设备跳过、失败设备重试、产物变化全部重做”。没有连接至少两台设备前，多设备条目保持“部分通过”。

## 完成剩余验收所需条件

1. 提供或选择一个可安全构建的真实多模块 HarmonyOS 工程，用于核对入口、依赖、路由、权限和各模块产物。
2. 至少连接两台设备（真机或含模拟器），人为制造一台失败，恢复后确认成功设备没有重复安装。
