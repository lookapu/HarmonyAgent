#Requires -Version 7
<#
.SYNOPSIS
DevEco Switch 快速构建脚本：按改动范围跳过不必要步骤，测试用绿色 exe。

用法（在项目根目录执行 ./scripts/build-quick.ps1）：
  ./scripts/build-quick.ps1                 # 自动检测 git 改动范围，只构建涉及的部分（默认推荐）
  ./scripts/build-quick.ps1 -Backend        # 只构建后端（cargo build --release --features embedding），前端复用现有 dist
  ./scripts/build-quick.ps1 -Frontend       # 只构建前端 + 重新链接后端（嵌入新资源）
  ./scripts/build-quick.ps1 -SkipTsc        # 跳过 tsc 类型检查，仅 vite build（更快的 UI 调试循环）
  ./scripts/build-quick.ps1 -Nsis           # 构建后打 NSIS 安装包（发版用，等价完整 tauri build）
  ./scripts/build-quick.ps1 -Full           # 全量重建：sccache 缓存加速 + 打安装包

说明：
- 普通模式（无 -Nsis/-Full）只产出绿色版 exe，跳过 NSIS 打包（测试用绿色 exe 即可，省时）。
- 后端命令带 --features embedding（与 tauri.conf.json build.features 一致），否则 embedding 功能缺失。
- ⚠️ 必须串行（前端 → 后端）：cargo build 的 build.rs 在编译早期把 ../dist 资源嵌入 exe，
  若与 vite build 并行（vite 启动会先清空 dist），嵌入的是不完整资源，exe 运行时报
  "localhost 拒绝连接"。前端改动后必须重新链接后端，exe 才会包含最新前端。
- 只改后端时可跳过前端（dist 已有完整资源，增量链接很快）。
- 全量重建时自动启用 sccache（RUSTC_WRAPPER），并关闭 cargo incremental 避免冲突；
  日常增量构建默认走 release incremental（见 src-tauri/Cargo.toml [profile.release]）。
#>
param(
  [switch]$Backend,
  [switch]$Frontend,
  [switch]$SkipTsc,
  [switch]$Nsis,
  [switch]$Full
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$tauriDir = Join-Path $root 'src-tauri'
$distDir = Join-Path $root 'dist'
$exe = Join-Path $tauriDir 'target\release\deveco-switch.exe'
$frontLog = Join-Path $env:TEMP 'deveco-quick-front.log'
$backLog = Join-Path $env:TEMP 'deveco-quick-back.log'

# ── 1. 确定需要构建的部分 ─────────────────────────────────────────
if ($Nsis) { $Full = $true }
$needFrontend = $Frontend -or $Full
$needBackend = $Backend -or $Full

if (-not $needFrontend -and -not $needBackend) {
  $dirty = @(git -C $root diff --name-only HEAD 2>$null)
  $dirty += @(git -C $root ls-files --others --exclude-standard 2>$null)
  $srcChanged = [bool]($dirty | Where-Object { $_ -like 'src/*' -or $_ -eq 'index.html' -or $_ -eq 'package.json' -or $_ -like 'vite.config.*' -or $_ -like 'public/*' })
  $rustChanged = [bool]($dirty | Where-Object { $_ -like 'src-tauri/*' -or $_ -like '.cargo/*' -or $_ -eq 'Cargo.toml' })
  # dist 完整性检查：index.html + assets 目录都必须存在（Tauri 嵌入 dist 资源）
  $distOk = (Test-Path (Join-Path $distDir 'index.html')) -and (Test-Path (Join-Path $distDir 'assets'))
  $needFrontend = $srcChanged -or -not $distOk
  # 前端改动后必须重新链接后端（嵌入新资源）；后端改动必须重建；exe 不存在必须重建
  $needBackend = $srcChanged -or $rustChanged -or -not (Test-Path $exe)
}

if (-not $needFrontend -and -not $needBackend) {
  Write-Host '无可构建的改动（前端/后端均无变更）。如需强制构建：-Frontend / -Backend / -Full' -ForegroundColor Yellow
  exit 0
}

Write-Host "==> 前端构建: $needFrontend | 后端构建: $needBackend | NSIS: $Nsis" -ForegroundColor Cyan

# ── 2. 串行执行：先前端后后端（dist 嵌入竞态要求串行） ────────────
$failed = $false

if ($needFrontend) {
  $frontCmd = if ($SkipTsc) { 'npx vite build' } else { 'npm run build' }
  Write-Host '==> 步骤 1/2：前端构建中（tsc + vite）...' -ForegroundColor Cyan
  $p = Start-Process -FilePath 'cmd.exe' -ArgumentList '/c', $frontCmd -WorkingDirectory $root `
    -RedirectStandardOutput $frontLog -RedirectStandardError "$frontLog.err" -PassThru -WindowStyle Hidden
  Wait-Process -Id $p.Id
  if ($p.ExitCode -ne 0) {
    $failed = $true
    Write-Host '==> [frontend] 失败，日志尾部：' -ForegroundColor Red
    if (Test-Path $frontLog) { Get-Content $frontLog -Tail 25 | ForEach-Object { Write-Host $_ } }
  } else {
    Write-Host '==> [frontend] 完成' -ForegroundColor Green
  }
}

if (-not $failed -and $needBackend) {
  if ($needFrontend) {
    # 前端重建过：touch tauri.conf.json 强制 build.rs 重跑（tauri-build 不监控 dist 目录，
    # 不 touch 则 cargo 判定无变化直接跳过，exe 不会嵌入最新前端资源，运行时报 localhost 拒绝连接）
    (Get-Item (Join-Path $tauriDir 'tauri.conf.json')).LastWriteTime = Get-Date
  }
  if ($Full) {
    # 全量重建：sccache 缓存 + 关闭 incremental（二者互斥，sccache 优先）
    $env:RUSTC_WRAPPER = 'sccache'
    $env:CARGO_INCREMENTAL = '0'
    Write-Host '==> 步骤 2/2：后端全量重建中（sccache 缓存）...' -ForegroundColor Cyan
  } else {
    Remove-Item Env:RUSTC_WRAPPER -ErrorAction SilentlyContinue
    Remove-Item Env:CARGO_INCREMENTAL -ErrorAction SilentlyContinue
    Write-Host '==> 步骤 2/2：后端增量构建中（release incremental）...' -ForegroundColor Cyan
  }
  $p = Start-Process -FilePath 'cargo' -ArgumentList 'build', '--release', '--features', 'embedding' `
    -WorkingDirectory $tauriDir -RedirectStandardOutput $backLog -RedirectStandardError "$backLog.err" `
    -PassThru -WindowStyle Hidden
  Wait-Process -Id $p.Id
  if ($p.ExitCode -ne 0) {
    $failed = $true
    Write-Host '==> [backend] 失败，日志尾部：' -ForegroundColor Red
    if (Test-Path $backLog) { Get-Content $backLog -Tail 25 | ForEach-Object { Write-Host $_ } }
  } else {
    Write-Host '==> [backend] 完成' -ForegroundColor Green
  }
}

if ($failed) {
  Write-Host '构建失败，请检查上方日志。' -ForegroundColor Red
  exit 1
}

# ── 3. 收尾：绿色 exe / NSIS 安装包 ───────────────────────────────
if (Test-Path $exe) {
  Write-Host "`n==> 绿色版（测试用）: $exe" -ForegroundColor Green
} else {
  Write-Host '警告：未找到 deveco-switch.exe' -ForegroundColor Yellow
}

if ($Nsis) {
  Write-Host '==> 打 NSIS 安装包中...' -ForegroundColor Cyan
  $env:HTTP_PROXY = 'http://127.0.0.1:7890'
  $env:HTTPS_PROXY = 'http://127.0.0.1:7890'
  node (Join-Path $root 'node_modules\@tauri-apps\cli\tauri.js') build --bundles nsis 2>&1 | Select-Object -Last 10
  if ($LASTEXITCODE -ne 0) { Write-Host 'NSIS 打包失败' -ForegroundColor Red; exit 1 }
  Write-Host "==> 安装包: $tauriDir\target\release\bundle\nsis\DevEco Switch_0.1.0_x64-setup.exe" -ForegroundColor Green
}
