# 项目生成物清理

开发、测试和打包会持续生成 Rust 编译缓存与前端产物，其中 `src-tauri/target` 在多种 feature、debug/release 组合长期叠加后可能达到数十 GiB。项目提供固定范围的跨平台清理命令：

```bash
# 只预览，不删除
npm run clean:generated:dry-run

# 删除可重新生成的缓存与产物
npm run clean:generated
```

命令只处理以下目标：

- `src-tauri/target`
- `dist`
- `coverage`
- `node_modules/.vite`
- 项目源码目录中的 `.DS_Store`

脚本拒绝项目外路径，跳过 `.git`、`node_modules` 主体、源码和配置。清理后第一次 Rust 编译会较慢；前端依赖无需重新安装。
