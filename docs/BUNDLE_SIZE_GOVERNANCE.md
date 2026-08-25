# 前端构建体积门禁

`npm run build` 完成后会自动执行 `scripts/check-bundle-size.mjs`，检查 `dist/assets` 中的 JavaScript 分块。

当前预算用于阻止已有大型懒加载块继续膨胀：

| 分块 | 上限 |
|---|---:|
| `Home-*.js` | 750 KB |
| `Markdown-*.js` | 1,550 KB |
| `index-*.js` | 575 KB |
| 任意单个 JavaScript 分块 | 1,600 KB |

预算按未压缩的生产构建产物字节数计算。超过预算时应优先通过动态导入、组件拆分或按需加载降低体积；只有确认新增体积合理且已评估启动与加载影响后，才同步调整预算。

该门禁不会掩盖 Vite 的 500 KB 提示。现有 `Home`、`Markdown` 和主入口仍应继续拆分，预算只是防回退边界。
