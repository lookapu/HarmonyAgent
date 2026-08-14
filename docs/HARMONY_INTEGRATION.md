# DevEco Agent — 鸿蒙集成设计（M3 实现细节）

> 本文是 [ARCHITECTURE.md](ARCHITECTURE.md) §7.3 的展开实现规格，覆盖三块：
> **① 工程结构解析器规则 ② hvigor 命令矩阵 ③ hdc 部署流程**，外加构建错误解析与工具链探测。
> 目标：按本文可实现、可测试，无需再查鸿蒙资料（版本差异容错除外）。

---

## 1. 工程结构解析器规则

### 1.1 扫描范围与优先级

打开项目（`add_project`）后，Rust 后台异步扫描。扫描根 = 项目根目录，**不递归扫描依赖目录**（`oh_modules` / `node_modules` / `build` / `.git` 排除）。

| 目标文件 | 用途 | 解析出 |
|---|---|---|
| `AppScope/app.json5` | 应用级信息 | bundleName / versionCode / versionName / 应用名(label) |
| `build-profile.json5`（根） | 构建配置 | 签名、products、compatibleSdkVersion、modules |
| `oh-package.json5`（根） | 依赖 | dependencies 列表 |
| `*/src/main/module.json5` | 模块 | name / type / mainElement / abilities / deviceTypes |
| `*/src/main/ets/**/*.ets` | 页面源码 | 页面清单 + @Router 路径 |
| `*/src/main/resources/base/profile/main_pages.json` | 路由表 | 权威路由列表 |
| `hvigorfile.ts` / `hvigor-config.json5` | 构建脚本 | 构建信息（可选，失败不阻塞） |
| `.git` 存在性 + `git branch` | 版本控制 | isRepo / branch |

模块发现规则：遍历根目录**一层**子目录，存在 `src/main/module.json5` 即视为模块；`type` 字段区分 `entry`（isEntry=true）/ `feature` / `har` / `hsp`。若多个 entry 或没有 entry，取 `deviceTypes` 含 phone 的第一个作构建入口，并在索引中标记 warning。

### 1.2 逐文件解析细则

#### 1.2.1 `AppScope/app.json5`

```json5
{ "app": { "bundleName": "com.example.myapp", "vendor": "example",
           "versionCode": 1000000, "versionName": "1.0.0",
           "icon": "$media:app_icon", "label": "$string:app_name" } }
```

解析字段：`bundleName`（部署必需）、`versionCode/versionName`（部署汇报用）、`label`。
注意 JSON5 允许注释与尾逗号 —— Rust 侧用 `json5` crate 或复用现有 `strip_jsonc_comments` + serde_json。

#### 1.2.2 `build-profile.json5`（工程根）

```json5
{
  "app": {
    "signingConfigs": [ { "name": "default", "type": "HarmonyOS", "material": { "certpath": "...", "storeFile": "..." } } ],
    "products": [
      { "name": "default", "signingConfig": "default",
        "compatibleSdkVersion": "5.0.0(12)", "runtimeOS": "HarmonyOS" }
    ],
    "buildModeSet": [ { "name": "debug" }, { "name": "release" } ]
  },
  "modules": [ { "name": "entry", "srcPath": "./entry",
                 "targets": [ { "name": "default", "applyToProducts": ["default"] } ] } ]
}
```

解析字段：
- `signing.signingConfigs` 非空 且 存在被某 product 引用的配置 → `signing.configured = true`；否则 false
- `products[0].compatibleSdkVersion` → `apiVersion`（见 1.4 推断）
- `products[].name` → 可用产品列表（构建参数 `-p product=`）
- `modules[].targets[]` → 模块目标（如 `entry@default`）

#### 1.2.3 `oh-package.json5`（工程根）

```json5
{ "name": "myapp", "version": "1.0.0",
  "dependencies": { "@ohos/hypium": "1.0.19" },
  "devDependencies": {} }
```

解析 `dependencies` 并入 `dependencies[]`；`@ohos/*` 前缀标记为 SDK 内置包（错误解析时用于区分"缺依赖"与"缺 SDK"）。

#### 1.2.4 `module.json5`（每模块）

```json5
{ "module": { "name": "entry", "type": "entry", "mainElement": "EntryAbility",
              "deviceTypes": ["phone", "tablet"], "pages": "$profile:main_pages",
              "abilities": [ { "name": "EntryAbility", "srcEntry": "./ets/entryability/EntryAbility.ets" } ] } }
```

解析字段：
- `name` / `type` / `mainElement`（**启动 Ability，部署必需**）
- `abilities[].name` + `srcEntry` → abilityNames + srcEntry 路径
- `pages` 值如 `$profile:main_pages` → 定位到 `resources/base/profile/main_pages.json`
- `deviceTypes` 是否含 phone/tablet（无 phone 模块标记 warning）

#### 1.2.5 `main_pages.json`（路由权威源）

```json
{ "src": [ "pages/Index", "pages/Login" ] }
```

解析 `src[]` 每个元素 → 路由路径（如 `pages/Login`），并映射到文件：
`{模块}/src/main/ets/{路径}.ets`（如 `entry/src/main/ets/pages/Login.ets`），文件不存在时标记 `missingFile: true`（路由在但文件缺失 → 构建必败，提示 Agent）。

#### 1.2.6 装饰器扫描（路由兜底源）

对每个 `.ets` 文件做轻量正则扫描（不解析 AST，性能优先）：

| 装饰器 | 正则（Rust） | 含义 |
|---|---|---|
| @Entry | `(?m)^\s*@Entry\b` | 页面入口 → 该文件加入 pages |
| @Router | `@Router\s*\(\s*\{[^}]*?\bpath\s*:\s*['"]([^'"]+)['"]` | 注册路由路径 |
| @Component | `(?m)^\s*@Component\b` | 组件（非页面） |
| @Entry + @Component 同时存在 | 同上两个匹配 | 页面文件判定 = @Entry 存在 |

文件 → 路由路径转换：`src/main/ets/` 之后去掉 `.ets` 后缀，如
`entry/src/main/ets/pages/Login.ets` → `pages/Login`。

### 1.3 路由合并算法（权威 + 兜底）

```
routes = []
seen = {}

for module in modules:
    if main_pages.json 存在:
        for path in main_pages.src:
            合并，来源标记 'main_pages'
    for file in module.ets 文件（@Entry 且含 @Router path）:
        path = 转换后的路由路径
        若 path 不在 seen → 合并，来源标记 '@Router'
    # 纯 @Entry 无 @Router 的文件：
    for file in module.ets 文件（@Entry 且无 @Router）:
        路径由文件路径推导（去 ets 前缀），来源标记 'inferred'

输出: routes[] 按模块分组、路径排序；same path 不同来源 → 保留 main_pages 优先
```

### 1.4 API 版本推断（三级）

1. `build-profile.json5` products[].compatibleSdkVersion（如 `"5.0.0(12)"` → 取括号内数字 12，`"5.0.0(12)"` / `"4.1.0(11)"` 均适用）
2. 缺失 → `oh-package.json5` 中 `@ohos/*` 依赖版本（如 `@ohos/hypium: 1.0.19` 不可靠，降级）
3. 仍缺失 → `apiVersion = null`，标记 warning "无法推断 API 版本，可能影响构建参数"

### 1.5 增量更新映射（notify 监听）

| 变更事件 | 重建范围 | 触发条件 |
|---|---|---|
| `main_pages.json` 变化 | 该模块 routes | 路径含 `profile/main_pages.json` |
| `module.json5` 变化 | 该模块信息（abilities/mainElement/type） | 路径含 `module.json5` |
| `.ets` 新增/删除/修改 | 该模块 pages（重扫装饰器） | 扩展名 == .ets |
| `build-profile.json5` 变化 | signing / apiVersion / build | 根目录下该文件 |
| `app.json5` / `oh-package.json5` 变化 | project 基本信息 / dependencies | 对应文件 |
| 其他 | 忽略（含 build/、oh_modules/ 排除） | — |

- 更新采用**合并写**：改 `data_json` 中对应 kind 段，全量索引重建仅在 schemaVersion 升级时执行。
- notify 事件节流：500ms 去抖窗口合并多次变更。

### 1.6 容错与降级

| 失败场景 | 行为 |
|---|---|
| 某文件解析失败 | 该项置 `null` + `parsed: false`，索引其余部分照常 |
| 非鸿蒙目录（无 module.json5） | `add_project` 拒绝并提示"不是鸿蒙工程（未找到 module.json5）"，或按"普通项目"降级（kind=other，仅文件工具可用） |
| 全部解析失败 | index_state = failed，右侧面板显示原因 + 重试按钮 |
| 解析成功但缺失关键字段 | warning 列表（如"无 entry 模块"），不阻塞 |

### 1.7 ProjectIndex 完整 Schema（实现级）

```typescript
interface ProjectIndex {
  schemaVersion: 1
  state: 'pending' | 'ready' | 'failed'           // 与 projects.index_state 同步
  warnings: string[]
  project: {
    name: string                                   // 目录名
    bundleName: string | null
    versionCode?: number
    versionName?: string
    appLabel?: string
  }
  apiVersion: number | null
  modules: ModuleInfo[]
  routes: RouteInfo[]
  dependencies: { name: string; version: string; builtin: boolean }[]
  signing: {
    configured: boolean
    productUsed?: string                           // 引用了签名配置的产品名
    certPath?: string                              // 从 signingConfigs material 取
  }
  build: {
    entryModule: string | null
    products: string[]                             // ["default", ...]
    buildModes: string[]                           // ["debug", "release"]
    assembleCmd: string                            // 见 §2.2 生成
    hapOutputDir: string | null                    // 见 §2.3 推导
  }
  git: { isRepo: boolean; branch?: string; dirty?: boolean }
  buildErrors: BuildError[]                        // 最近一次构建，见 §4.2
}

interface ModuleInfo {
  name: string
  type: 'entry' | 'feature' | 'har' | 'hsp' | 'unknown'
  isEntry: boolean
  srcMain: string                                  // 绝对路径 {root}/{name}/src/main
  etsRoot: string                                  // {srcMain}/ets
  pages: PageInfo[]
  abilityNames: string[]
  mainElement: string | null                       // 启动 Ability
  deviceTypes: string[]
  parsed: boolean
}

interface PageInfo {
  routePath: string                                // 如 pages/Login
  file: string                                     // 绝对路径
  source: 'main_pages' | 'router' | 'inferred'
  missingFile: boolean
}

interface RouteInfo extends PageInfo { module: string }

interface BuildError {
  kind: 'arkts' | 'resource' | 'dependency' | 'signing' | 'sdk' | 'ohpm' | 'other'
  file?: string
  line?: number
  column?: number
  message: string                                  // 原始消息，截断 500 字
  suggestion: string
  rawLine?: string
}
```

---

## 2. hvigor 命令矩阵

### 2.1 命令基础

- **入口**：项目根目录 `hvigorw.bat`（wrapper，内部调用 DevEco 内置 node + `hvigor/hvigor-wrapper.js`）。执行时工作目录必须是项目根。（Windows 实际执行**不经过 bat**，直调 node 绕 cmd 防闪窗，见 §2.4。）
- **环境**：wrapper 依赖 DevEco Studio 的 node 与 hvigor 库；若 `hvigorw.bat` 缺失（罕见），fallback 到 `{DevEco}\tools\hvigor\bin\hvigorw.bat`。
- **通用参数**：

| 参数 | 说明 | 示例 |
|---|---|---|
| `--mode project\|module` | project=全工程（默认）；module=单模块，需配 `-p module=` | `--mode module` |
| `-p module=entry@default` | 模块@目标 | `-p module=entry@default` |
| `-p product=default` | 构建产品 | `-p product=default` |
| `--no-daemon` | 单次执行不驻留（构建慢时可避免端口占用） | 默认加 |
| `--parallel` | 并行任务 | 可选 |
| `--stacktrace` | 详细堆栈（排查用） | 失败重试时加 |

### 2.2 命令矩阵

| 场景 | 命令 | 何时用 |
|---|---|---|
| 全量构建 | `hvigorw assembleHap --no-daemon` | 默认 `build_hap` 无参数时 |
| 单模块构建 | `hvigorw --mode module -p module=entry@default assembleHap --no-daemon` | 多模块工程只改某模块 |
| 指定产品 | 上述命令追加 `-p product=default` | 多产品工程 |
| 清理 | `hvigorw clean --no-daemon` | 构建产物异常时（L2 警示） |
| 任务列表 | `hvigorw tasks --no-daemon` | 诊断 |
| 版本 | `hvigorw --version` | 工具链校验 |

`assembleCmd` 生成规则（写入 ProjectIndex.build）：
- 单 entry 模块 → `hvigorw assembleHap --no-daemon`
- 多模块 → `hvigorw --mode module -p module={entryModule}@default assembleHap --no-daemon`
- 用户可在设置页覆盖（存 settings 表 `project_{id}_assemble_cmd`）

### 2.3 产物定位

```
{module}/build/{product}/outputs/{target}/{module}-{product}-{signed|unsigned}.hap
例：entry/build/default/outputs/default/entry-default-signed.hap
```

- `hapOutputDir` 推导：`{entryModule}/build/{默认产品}/outputs/{默认目标}/`
- 部署时文件选择优先级：`*-signed.hap` > `*-unsigned.hap`（unsigned 需提示"未签名，真机可能无法安装"）
- 构建完成后扫描该目录最新 hap（按 mtime），避免文件名变体（如 `entry-default-signed.hap` vs `entry-phone-signed.hap`）

### 2.4 执行与日志

- 执行：Rust `tokio::process::Command`，`creation_flags` 含 `CREATE_NO_WINDOW`；stdout/stderr 合并流式读行 → `agent:log` 事件（绑定构建卡片）+ 追加写盘 `{项目}/.deveco-agent/logs/build-{timestamp}.log`，同时维护 `latest.log` 软链接语义（复制）。
- **Windows 防闪窗**：hvigorw 执行**不经过 `hvigorw.bat`/cmd.exe**——工具链探测时记录 DevEco 内置 node 路径（`{DevEco}\tools\node\node.exe`），直调 `node {项目}/hvigor/hvigor-wrapper.js [args]`（等价于 hvigorw.bat 内部逻辑）；仅在 node 直调失败时回退 `cmd /c hvigorw.bat` + CREATE_NO_WINDOW。
- 退出码：0 成功；非 0 → 立即触发 §4 错误解析，构建卡片红色 + 错误摘要区块。
- 超时：默认 600s（首构可能几分钟），设置可调；超时 kill 进程树并标记 cancelled。
- 取消：用户点停止 → `taskkill /PID {pid} /T /F`。

### 2.5 常见失败与提示（写入构建卡片）

| 失败现象 | 根因 | 处理 |
|---|---|---|
| `hvigor ERROR: Failed to resolve dependency` | 依赖缺失 | 建议 `ohpm install` 后重试 |
| `hvigor ERROR: ... signed.hap` / sign 相关 | 签名配置错误/证书过期 | 检查 signingConfigs；提示在 DevEco 重新配置签名 |
| `FAILURE: Build failed` + `SDK not found` | SDK 路径/版本不匹配 | 检查 build-profile compatibleSdkVersion vs 已装 SDK |
| node 相关错误（wrapper 起不来） | DevEco node 缺失/路径错 | 重新探测工具链（§5） |
| 端口占用 / daemon 冲突 | 上次构建未退出 | 加 `--no-daemon` 重试 |

---

## 3. hdc 部署流程

### 3.1 设备管理

| 操作 | 命令 | 说明 |
|---|---|---|
| 列出设备 | `hdc list targets` | 解析行如 `NLA-AN00 192.168.1.5:5555` 或 `NLA-AN00  device` |
| 无线连接 | `hdc tconn {ip}:{port}`（默认 5555） | 需设备开启无线调试 |
| 断开 | `hdc tdisconn {ip}:{port}` | — |
| 设备信息 | `hdc shell param get const.product.model` | 型号确认 |

`Device` 结构：`{ serial, model?, state: 'online'|'offline', wireless: bool }`。
默认设备记忆：上次部署成功的设备 serial 存 settings（`default_device`），部署卡片下拉可选。

### 3.2 安装

| 操作 | 命令 | 说明 |
|---|---|---|
| 安装 | `hdc -t {serial} install {hap}` | 新装 |
| 覆盖安装 | `hdc -t {serial} install -r {hap}` | 已存在同 bundleName 时（红色警示，见 §3.4） |
| 卸载 | `hdc -t {serial} uninstall {bundleName}` | — |
| 查询已装 | `hdc -t {serial} shell aa dump -l \| grep {bundleName}` | 冲突检测 |

### 3.3 启动 App（重要：鸿蒙是 `aa start`，不是 Android 的 `am start`）

```
hdc -t {serial} shell aa start -b {bundleName} -a {MainAbility}
```

- `MainAbility` 解析：entry 模块 `module.json5` 的 `mainElement`（如 `EntryAbility`）→ 缺失则取 `abilities[0].name` → 再缺失则报错并建议 `aa dump -l` 查询。
- **防呆**：启动后 2s 用 `hdc shell aa dump -l | grep {bundleName}` 验证进程/Ability 已拉起；失败给出 `hilog` 尾部日志。
- 若用户习惯说"am start"（Android 术语）：系统提示词与错误提示中统一纠正为 aa start，避免 Agent 生成错误命令（命令白名单限制下会直接拒绝并提示）。

### 3.4 部署完整流程（install_launch 内部时序）

```
1. build_hap（若产物不存在或失败）→ 失败则中止，构建卡片给出错误摘要
2. 定位 hap：hapOutputDir 下最新 *-signed.hap（无 signed 用 unsigned + 提示）
3. 设备：list_devices → 取默认设备；无设备 → 部署卡片提示"未连接设备，请插线/开无线调试"
4. 冲突检测：aa dump -l 查 bundleName
   ├─ 不存在 → 直接 install
   └─ 已存在 → 记录已装旧版本号（写入 tool_runs.result_json，汇报/回退提示用，对应 §3.2"覆盖安装先记录旧版本"）→ 红色警示卡片（默认模式自动 install -r；严格模式询问）
5. 安装成功 → aa start 拉起
6. 验证：aa dump -l 确认；可选 hilog 尾部 20 行附到部署卡片
7. 汇报：设备 / bundleName / hap 路径 / 安装耗时 / 启动结果
```

### 3.5 日志与崩溃定位

| 需求 | 命令 | 说明 |
|---|---|---|
| 应用日志 | `hdc -t {serial} shell hilog \| grep {bundleName}` | 部署后验证 |
| 崩溃栈 | 上述 + `grep -E "FATAL\|Error"` | 崩溃时取尾部 50 行 |
| 清空日志 | `hdc shell hilog -r` | 验证前清空便于过滤 |

### 3.6 部署错误与解决建议（部署卡片内嵌）

| 错误 | 根因 | 建议 |
|---|---|---|
| `Failed to install` / `INSTALL_PARSE_FAILED` | hap 损坏/签名不符 | 重新构建；unsigned 需签名 |
| `INSTALL_FAILED_SIGNATURE_INVALID` | 签名与设备已装版本不一致 | 卸载旧版或用同签名 |
| `no targets` / device offline | 未连接/未授权 | 检查 USB 调试授权；hdc tconn |
| `aa start` 报 ability 不存在 | MainAbility 名错 | 读 module.json5 mainElement；aa dump -l 核对 |
| 启动即闪退 | 运行时崩溃 | hilog 抓崩溃栈；检查 API 版本兼容 |

---

## 4. 构建错误解析（正则库）

### 4.1 错误模式表（Rust 正则，按序匹配，首中即止）

| # | 模式（正则） | kind | 提取 | suggestion |
|---|---|---|---|---|
| 1 | `(?m)ArkTS:ERROR\s+File:\s*(\S+?):(\d+):(\d+)\s*\n?(.*)` | arkts | file,line,col,message（多行捕获到下一个 ERROR 或空行） | "读 {file}:{line} 修复语法/类型错误后重建" |
| 2 | `(?m)ERROR File:\s*(\S+?):(\d+):(\d+)` | arkts | file,line,col | 同上（旧版格式兜底） |
| 3 | `(?i)failed to resolve dependency|Cannot find module ['"]@?(\w[\w\-/]*)` | dependency | module 名 | "执行 ohpm install 后重试；检查 oh-package.json5" |
| 4 | `(?i)sign(ing)?\s+(fail|error)|Signing configuration ['"]([^'"]+)['"]\s+not found|certificate.*expired` | signing | 配置名 | "检查 build-profile.json5 signingConfigs，在 DevEco 重新配置" |
| 5 | `(?i)SDK not found|compatibleSdkVersion.*(not|require)|cannot find sdk` | sdk | — | "检查 compatibleSdkVersion 与已装 SDK（设置页可查）" |
| 6 | `(?i)resource.*not found|can't resolve resource|\\$r\(['"]app\.(\w+)\.(\w+)` | resource | 资源名 | "检查 resources/base/ 下对应资源文件" |
| 7 | `(?i)ohpm.*(error|ENOENT)` | ohpm | message | "检查 ohpm 工具链路径（§5）" |
| 8 | `(?m)^> hvigor ERROR:?\s*(.+)$` | other | message | "定位失败步骤，重试或查看完整日志" |

- 匹配在**日志流式到达时**进行（不等待结束），命中即产出 BuildError 并立即随 `agent:log` 推送，Agent 可提前开始修复；构建退出码确认失败后再次汇总 `buildErrors[]` 写 ProjectIndex 缓存。
- 日志行号仅取 **ArkTS 编译错误**（模式 1/2）；其余错误 kind 无 file/line。

### 4.2 BuildError 输出（写 ProjectIndex.buildErrors + 构建卡片）

```json
{
  "kind": "arkts",
  "file": "D:/apps/MyApp/entry/src/main/ets/pages/Home.ets",
  "line": 23, "column": 5,
  "message": "ArkTS:ERROR: Object literal must correspond to some explicitly declared class or interface.",
  "suggestion": "读 Home.ets:23 修复语法/类型错误后重建",
  "rawLine": "> hvigor ERROR: ArkTS:ERROR File: D:/apps/MyApp/.../Home.ets:23:5"
}
```

---

## 5. 工具链探测

### 5.1 探测顺序（Windows 为主，macOS 类比）

| 工具 | 探测顺序 | 备注 |
|---|---|---|
| DevEco Studio 根 | ① 注册表 `HKCU\Software\Huawei\DevEcoStudio` 与 `HKLM\SOFTWARE\Huawei\DevEcoStudio`（version 键） ② 常见路径 `C:\Program Files\Huawei\DevEco Studio*` ③ `%LOCALAPPDATA%\Huawei\DevEco Studio*` | 找到即缓存 |
| hdc | ① DevEco 设置中 SDK 路径（settings 表缓存） ② `{DevEco}\sdk\*\*\toolchains\hdc.exe` 递归 glob ③ `%USERPROFILE%\AppData\Local\Huawei\Sdk\*\toolchains\hdc.exe` ④ PATH | 取第一个存在 |
| ohpm | ① `{DevEco}\tools\ohpm\bin\ohpm.exe` ② `{DevEco}\ohpm\bin\ohpm.exe` ③ PATH | DevEco Studio 自带 |
| hvigorw | ① 项目内 `hvigorw.bat` ② `{DevEco}\tools\hvigor\bin\hvigorw.bat` | 项目 wrapper 优先 |

### 5.2 SDK 目录结构参考（API 12+，HarmonyOS SDK）

```
{SDK 根}/
  default/
    openharmony/          # OpenHarmony SDK（hdc 在 toolchains/）
      toolchains/hdc.exe
    hms/                  # HMS SDK
    ...
  12/ 或 5.0.0(12)/       # 按版本目录（部分安装布局）
```

> 版本差异容错：`toolchains/hdc.exe` 的查找用递归 glob，不硬编码版本目录名。

> **macOS 类比**：DevEco Studio 默认 `/Applications/DevEco Studio.app/Contents`（内置 node 在 `Contents/tools/node/bin/node`，hvigor/ohpm 同 tools 目录结构）；hdc 仍在 SDK toolchains；git 用系统（Xcode CLT / homebrew），探测顺序先 PATH。

### 5.3 缓存与失效校验

- 结果存 `settings` 表：`toolchain_deveco` / `toolchain_hdc` / `toolchain_ohpm` / `toolchain_hvigor`。
- 应用启动时校验存在性（`Path::exists`），全部有效 → 跳过探测；任一失效 → 全量重探测。
- 设置页：显示探测结果 + 手动指定路径 + 「重新探测」按钮。
- **UI 联动**：探测结果统一走 `check_environment`（状态分级 🟢/🔴/🟠/🟡 + 下载地址/教程引导），渲染在设置页「环境健康」区（ARCHITECTURE.md §7.4.3）；启动时必需项缺失会出顶部横幅引导。

---

## 6. M3 验收用例

| # | 用例 | 通过标准 |
|---|---|---|
| TC1 | 打开真实 API 12/13 工程（多模块） | ProjectIndex ready；模块/类型/entry 识别 100%；bundleName/签名状态正确 |
| TC2 | 路由合并 | main_pages.json 与 @Router 合并去重，missingFile 标记正确 |
| TC3 | 增量更新 | 修改 main_pages.json 后 2s 内 routes 更新，全量索引不重建 |
| TC4 | 构建 | `build_hap` 成功，构建卡片流式日志，hap 产物正确定位（signed 优先） |
| TC5 | 部署闭环 | NLA-AN00：install → aa start → 验证拉起；覆盖安装走红色警示路径 |
| TC6 | 错误修复循环 | 注入编译错误 → buildErrors 正确解析（file/line）→ Agent 修复 ≤3 轮 |
| TC7 | 容错 | 删除 module.json5 模拟损坏 → 索引 failed 但不崩溃，右侧面板显示原因 |
| TC8 | 工具链 | 无 DevEco 环境 → 探测失败给出引导；有 → 全部工具链就绪 |
