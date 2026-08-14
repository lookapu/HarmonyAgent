# 绿色版一键打包：deveco-switch.exe + 完整 resources 自包含目录，拷贝即用。
# 用法：pwsh scripts/build-portable.ps1 [-Config release]
# 产物：portable-build\DevEco Switch 绿色版\（deveco-switch.exe + resources\{node,git,jdk,seed,embedding}）
# 布局与安装版一致（tauri 的 resource_dir 在 Windows 上 = exe 所在目录），
# exe 已静态链接 VC 运行库 + 内置 comctl32 v6 manifest，不依赖系统任何第三方运行时。

param(
    [string]$Config = "release"
)

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$srcTauri = Join-Path $root "src-tauri"
$exe = Join-Path $srcTauri "target\$Config\deveco-switch.exe"
$out = Join-Path $root "portable-build\DevEco Switch 绿色版"

if (-not (Test-Path $exe)) {
    throw "未找到 $exe，请先构建：node node_modules/@tauri-apps/cli/tauri.js build"
}

# 资源映射（与 tauri.conf.json bundle.resources 一致）：源 → 目标子目录
$map = @(
    @{ Src = "runtime\node";       Dst = "node" },
    @{ Src = "runtime\git";        Dst = "git" },
    @{ Src = "runtime\jdk";        Dst = "jdk" },
    @{ Src = "resources\seed";     Dst = "seed" },
    @{ Src = "resources\embedding"; Dst = "embedding" }
)

Write-Host "==> 输出目录: $out"
if (Test-Path $out) { Remove-Item $out -Recurse -Force }
New-Item -ItemType Directory -Path $out -Force | Out-Null

Write-Host "==> 复制 exe ($([math]::Round((Get-Item $exe).Length/1MB,1)) MB)"
Copy-Item $exe (Join-Path $out "deveco-switch.exe")

foreach ($m in $map) {
    $src = Join-Path $srcTauri $m.Src
    $dst = Join-Path $out "resources\$($m.Dst)"
    if (-not (Test-Path $src)) {
        Write-Host "    跳过（源缺失）: $($m.Src)"
        continue
    }
    New-Item -ItemType Directory -Path $dst -Force | Out-Null
    Write-Host "==> 复制 $($m.Src) ..."
    robocopy $src $dst /E /NJH /NJS /NFL /NDL /NP | Out-Null
    if ($LASTEXITCODE -ge 8) { throw "robocopy 失败: $($m.Src) (exit $LASTEXITCODE)" }
}

# 校验关键文件
$checks = @(
    "deveco-switch.exe",
    "resources\node\node.exe",
    "resources\git\cmd\git.exe",
    "resources\jdk\bin\java.exe",
    "resources\seed\knowledge.db",
    "resources\embedding\bge-small-zh-v1.5\model.safetensors"
)
$missing = @()
foreach ($c in $checks) {
    if (-not (Test-Path (Join-Path $out $c))) { $missing += $c }
}
if ($missing.Count -gt 0) {
    throw "关键文件缺失: $($missing -join ', ')"
}

$total = (Get-ChildItem $out -Recurse -File | Measure-Object Length -Sum).Sum
Write-Host ""
Write-Host "=== 绿色版打包完成 ==="
Write-Host "目录: $out"
Write-Host "体积: $([math]::Round($total/1MB,1)) MB"
Write-Host "说明: 整个目录拷贝到任意 Win10 1809+/Win11 机器即可运行；"
Write-Host "      Win11 自带 WebView2 Runtime；Win10 若缺失请安装 Microsoft Edge WebView2 Runtime"
Write-Host "      （或使用 NSIS 安装版：src-tauri\target\release\bundle\nsis\，已内置 WebView2 离线安装器）。"
