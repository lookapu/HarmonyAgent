# HarmonyOS 基础性能基线

`run_perf_benchmark` 将真机上的基础性能验证接入当前 Agent Run，用同一份报告覆盖启动、CPU、内存、电量、温度、FPS 和 HAP 包体积。它用于发现明显回归，不替代专业功耗、帧时序或实验室级性能分析。

## 测量流程

1. 通过统一设备快照选择具备 Ability 能力的在线、已授权设备。
2. 默认停止目标应用，重新启动 Ability，并轮询 Ability 状态，记录“命令发出到可观测状态”的毫秒数；`measure_startup: false` 可跳过这一有状态步骤。
3. 可选执行 `steps` UI 流程。任一步失败都会终止基准，避免把错误页面的采样当成有效结果。
4. 在 3—30 秒窗口内采集应用 CPU、PSS 近似内存、系统 CPU/内存与温度；FPS 和电量在设备支持时尽力读取。
5. 读取显式 `hap`，或按 `product` / `module` 从可信产物清单唯一选择 HAP，记录文件大小。
6. 与进程内同工程、设备、应用的上一次快照比较，并把完整指标写入 `harmony.performance.measured` Run 事件。

示例参数：

```json
{
  "device": "<serial>",
  "package": "com.example.app",
  "product": "default",
  "module": "entry",
  "measure_startup": true,
  "seconds": 10,
  "label": "before-optimization"
}
```

## 结果语义

- 启动时间是 Ability 状态确认延迟，不宣称等同于首帧或完全可交互时间。
- PSS、系统内存、温度、FPS 和电量依赖设备暴露的诊断接口；缺失时明确标记“不可用”，不会用零值冒充样本。
- 默认提示阈值是 CPU 上升 15 个百分点、PSS 上升 50 MB、温度上升 3℃、启动变慢 300 ms 或 HAP 增长 512 KB。它们是线索，不是跨设备的统一发布门禁。
- 上一次对比快照仅在当前桌面进程内保存；跨重启分析应读取持久 `harmony.performance.measured` 事件并按标签、设备和环境建立团队基线。

## 安全边界

默认启动测量会停止并重新启动目标应用，UI steps 也会改变界面状态，因此该工具属于有状态诊断。它不会安装、卸载或修改工程文件。显式 HAP 路径仍受项目根目录与产物选择规则约束。

## 验收

单元测试覆盖电量容量解析和越界拒绝；阶段门禁覆盖全量 Rust 测试、Worker 崩溃恢复 E2E、前端测试、ESLint、生产构建与差异检查。真机验收时至少执行两次同标签基准，确认指标、不可用说明、差值报告和 Run 事件一致。
