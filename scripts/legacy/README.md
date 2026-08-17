# scripts/legacy

开发期临时调试脚本与输出归档。

仅供查证历史问题，**新功能不应再依赖此目录**。

## 归档分类

### 1. OCR 调试（项目初期 UI 像素级定位）
- `ocr_*.ps1` — PowerShell 调 Tesseract 跑截图 OCR
- `ocr_*.txt` — OCR 输出（`ocr_final*` / `ocr_now*` 是多次重跑结果）

**已废弃原因**：OCR 方案对 Tauri 窗口文字识别精度太差，已改用更稳的方案（具体路径见项目源码）。

### 2. 像素扫描（PowerShell 调 Win32 API）
- `pixel_*.ps1` / `pixel_*.txt` — 屏幕颜色采样，定位 badge / tabbar / button 等 UI 元素坐标
- `scan_h.ps1` / `scan_h55.ps1` / `scan_v.ps1` — 屏幕区域水平/垂直扫描

**已废弃原因**：早期为了做"无障碍测试机器人"调试时留下的临时方案。当前 UI 测试已用 DevEco 内置能力（见 `device_tools.rs` / `ui_tools.rs`）。

### 3. 图像裁剪/截屏残留
- `crop_badge.ps1` / `crop_tabbar.ps1` — 从全屏截图中裁出指定区域
- `tabbar_crop.png` / `top_badge_crop.png` / `_tmp_testhy_screen.jpeg` / `add_project_dialog.png`

**已废弃原因**：调试产物，已无参考价值。

### 4. Python 一次性分析脚本
- `analyze_diff.py` / `analyze_sessions.py` / `analyze_session_detail.py` — 对话数据库分析
- `analyze_build_profile.py` / `analyze_build_profile2.py` / `analyze_bp3.py` — hvigor 配置分析
- `analyze_todo_ohpm.py` — ohpm 依赖分析

**已废弃原因**：临时分析脚本，结果已沉淀进产品代码或 wiki。

### 5. 其他零散
- `min_test.exe` / `min_test.pdb` — Rust 编译试验产物
- `vite.log` — Vite 构建日志
- `vfix_ready.flag` — 一次性标记文件
- `bad_imports.txt` / `good_imports.txt` — import 路径对照
- `msg_766e2dfa_backup.txt` — 一次会话消息的备份
- `0byte-empty-file-marker.txt` — 根目录原 `-` 0 字节文件改名归档

## 何时删除整个目录

- 若 6 个月内无任何文件被 git 检索引用
- 仓库对外发布 v1.0 后可考虑

## 新调试脚本去哪

- **持续使用的工具脚本** → `scripts/`（如 `build-portable.ps1` / `smoke-test-green.ps1`）
- **临时单次脚本** → 用完即删，不要进仓库
