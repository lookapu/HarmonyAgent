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
- 设备：① 华为 CHZ-AL00（HarmonyOS 6.1.0.135(SP8C00E126R2P4)），`hdc list targets` 返回 `6UNBB26507103971`（USB）；② DevEco 模拟器（OpenHarmony 7.0.0.23(SP11DEVC00E45R4P11)），TCP `127.0.0.1:5555`。
- 工程模块：`entry`（type=entry 入口 HAP 模块）+ `application`（type=feature HAP 模块，用户 2026-08-22 新建）。

验收过程没有修改真实工程源码；原工程构建后 `git status --short` 为空。故障注入仅发生在 `/private/tmp` 下的一次性副本中，验收后已删除。真实工程的 `build-profile.json5` 未被 git 跟踪，为完成签名构建在其中补了 product→signingConfig 引用（`"signingConfig": "default"`），不污染工程源码。

## 结果总览

| 验收项 | 状态 | 结论 |
|---|---|---|
| 真实多模块工程关系 | 通过 | 双模块工程（entry+application）入口、依赖、路由、权限与产物关系完整解析并双模块签名构建成功（见下文） |
| ArkTS/Hvigor 诊断修复闭环 | 通过 | 在真实工程临时副本注入类型错误，准确定位、修复并重新构建成功 |
| 真机运行诊断闭环 | 通过 | 真机完成签名构建、安装、启动、hilog 基线、故障注入、异常定位、修复与重新验证（见上文） |
| SDK/API Level 不兼容诊断 | 通过 | 本地定义与官方变更联合诊断回归通过；真实 API 23 工程可由已装 SDK 工具链正常构建 |
| 多设备隔离与恢复 | 通过 | 真机+模拟器双设备外部验收：单台故障不污染另一台；防重放自动化测试通过（见下文） |

## 真实工程结构与构建基线

真实工程解析结果（2026-08-22，双模块）：

- 根构建配置声明 `entry`（`./entry`）与 `application`（`./application`）两个模块，均属于默认 product，compile/compatible/target API 23。
- 应用级 `AppScope/app.json5`：bundleName `com.sns.harmony`，versionName 1.0.0。
- `entry` 是 HAP 入口模块（type=entry），主元素 `EntryAbility`，支持 `default` 与 `tablet`；路由 profile `mvp_pages.json` 声明 **124 个页面**，入口 `pages/Index`，首跳 `pages/main/MainHomePage`；清单仅声明 `ohos.permission.INTERNET`。
- `application` 是 feature 类型 HAP 模块（type=feature），Ability 为 `ApplicationAbility`（`./ets/applicationability/ApplicationAbility.ets`），路由 `pages/Index`，支持 `default` 与 `tablet`；无权限声明。
- 依赖关系：根 `oh-package.json5` 无第三方依赖；`entry` 有注释状态下的本地 IM SDK（`file:./libs/imsdk-ohos-7.7.5294.har`，未启用）；模块间无依赖边。
- 构建产物：`entry` 与 `application` 分别 `assembleHap` 均 `BUILD SUCCESSFUL` 且 `SignHap` 通过，产出 `entry-default-signed.hap`（63,836,095 字节）与 `application-default-signed.hap`；两个模块的产物相互独立，模块路径 `entry/build/...` 与 `application/build/...`。

执行命令：

```text
/Applications/DevEco-Studio.app/Contents/tools/hvigor/bin/hvigorw --mode module -p product=default -p module=entry@default assembleHap --no-daemon
/Applications/DevEco-Studio.app/Contents/tools/hvigor/bin/hvigorw --mode module -p product=default -p module=application@default assembleHap --no-daemon
```

构建同时给出既有 ArkTS 异常处理警告；`application` 模块首次构建即带签名（`SignHap` 通过）。历史基线（单模块、未签名、55,970,427 字节）记录于 2026-08-21 版本，仅证明编译/打包闭环；当前双模块签名产物已在真机与模拟器验证可安装。

真实工程的构建配置还存在签名敏感字段和机器绝对路径风险。验收没有读取、复制或记录字段值，也没有擅自改动外部工程源码；后续应以隔离凭据和显式审批单独治理。

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

结果 `1 passed; 0 failed`，覆盖 product、嵌套模块、HAP/HSP/HAR、Ability 和依赖边；真实双模块工程（entry+application）的结构解析与双模块签名构建在上文已实证，与语义模型一致。

多设备恢复新增内容哈希门禁：读取当前 Run 及有限深度父 Run 的 `harmony.deploy.batch.completed` 事件，只跳过相同 HAP 哈希且状态为 `completed` 的设备；失败设备仍重试，HAP 变化则不复用旧成功证据。自动化测试覆盖“成功设备跳过、失败设备重试、产物变化全部重做”。

双设备外部验收（2026-08-22，真机 + 模拟器）：

1. **双设备部署**：同一 `entry-default-signed.hap` 分别安装到真机 `6UNBB26507103971` 与模拟器 `127.0.0.1:5555`，均 `install bundle successfully`；两台设备启动 `EntryAbility` 均成功且进程存活（真机 PID 11885、模拟器 PID 6220）。
2. **单台故障注入**：卸载模拟器上的应用（`bm uninstall -n com.sns.harmony` 返回 `uninstall bundle successfully`，`bm dump` 确认应用消失），模拟该设备部署结果丢失/失败。
3. **另一台不受污染**：故障期间真机进程 PID 11885 全程原样存活——未被杀、未重装、未重启；应用状态与故障前完全一致。
4. **恢复与重试**：模拟器重新安装同一 HAP（相同 SHA-256）并启动成功（新 PID 6857）；双设备终态均正常运行。
5. **防重放自动化**：`cargo test --locked multi_device --lib` 通过 `multi_device_strategy_is_bounded_and_explicit` 与 `multi_device_recovery_skips_only_successes_for_the_same_artifact`（2 passed; 0 failed），覆盖同产物成功设备跳过、失败设备重试、并行度有界。

结论：单台设备失败不污染其他设备结果（设备级实证）；恢复后不重复成功部署由内容哈希门禁保证（agent 级自动化测试实证，设备级表现为故障期间成功设备全程无安装动作）。

## 完成剩余验收所需条件

5.5 全部条目已通过（2026-08-22）。后续如工程或工具链变化，应重新执行对应验收并更新本记录。
