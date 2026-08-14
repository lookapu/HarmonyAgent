# DevEco Switch

DevEco Code Provider 管理工具 — 管理多个 AI Provider、版本、配置、费用追踪、MCP、Skill，支持本地代理和自动故障转移。

## 功能

- **Provider 管理** — 添加/切换/测试多个 AI Provider（华为免费模型、智谱、阿里通义等）
- **版本管理** — 安装/切换/回滚 DevEco Code 版本
- **配置编辑** — 可视化编辑 deveco.jsonc
- **费用追踪** — 请求日志、token 用量、每日费用统计
- **本地代理** — HTTP 代理 + 熔断器 + 自动 failover
- **MCP 管理** — 添加/启用/禁用 MCP 服务器
- **Skill 管理** — 浏览/安装/启用 Skill
- **健康监控** — Provider 连通性检测
- **自动更新** — 检查 GitHub Releases 新版本
- **多语言** — 中文/英文切换
- **主题** — 暗色/亮色切换

## 安装

从 [Releases](https://github.com/like3213934360-lab/Deveco-code-swich/releases) 页面下载安装包：

- **macOS**: 下载 `.dmg` 或 `.app.tar.gz`
- **Windows**: 下载 `.msi` 或 `.exe`

### macOS 首次打开

由于应用未签名，macOS 会阻止打开。解决方法：

**方法一（终端）：**
```bash
xattr -cr "/Applications/DevEco Switch.app"
```

**方法二（系统设置）：**
系统设置 → 隐私与安全性 → 滚动到底部 → 点击"仍要打开"

## 从源码构建

```bash
# 安装依赖
npm install

# 开发模式
npx tauri dev

# 构建安装包
npx tauri build
```

## 技术栈

- **框架**: Tauri 2.x
- **后端**: Rust (hyper, rusqlite, tokio)
- **前端**: React 18 + TypeScript + Vite + Tailwind CSS
- **数据库**: SQLite

## License

MIT
