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

# 先装到临时目录、校验通过后再换位：绝不先删成品。
# 早先「先 Remove-Item -Recurse 再拷」的写法在目录被占用时（资源管理器窗口开在里面、
# 或应用正在运行）会把成品删到一半就中断——exe 与 resources 都没了，只剩一个删不掉的空目录，
# 此时用户运行到的那份绿色版就没有 seed，表现为"API 知识库内容全没了"（本机实际发生过）。
$staging = "$out.new"
$previous = "$out.old"

Write-Host "==> 输出目录: $out"
if (Test-Path $staging) { Remove-Item $staging -Recurse -Force }
New-Item -ItemType Directory -Path $staging -Force | Out-Null

Write-Host "==> 复制 exe ($([math]::Round((Get-Item $exe).Length/1MB,1)) MB)"
Copy-Item $exe (Join-Path $staging "deveco-switch.exe")

foreach ($m in $map) {
    $src = Join-Path $srcTauri $m.Src
    $dst = Join-Path $staging "resources\$($m.Dst)"
    if (-not (Test-Path $src)) {
        Write-Host "    跳过（源缺失）: $($m.Src)"
        continue
    }
    New-Item -ItemType Directory -Path $dst -Force | Out-Null
    Write-Host "==> 复制 $($m.Src) ..."
    robocopy $src $dst /E /NJH /NJS /NFL /NDL /NP | Out-Null
    if ($LASTEXITCODE -ge 8) { throw "robocopy 失败: $($m.Src) (exit $LASTEXITCODE)" }
}

# 校验关键文件：在换上去之前做，避免把不完整的目录落到成品位置
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
    if (-not (Test-Path (Join-Path $staging $c))) { $missing += $c }
}
if ($missing.Count -gt 0) {
    Remove-Item $staging -Recurse -Force -ErrorAction SilentlyContinue
    throw "关键文件缺失: $($missing -join ', ')"
}

# 换位：优先「改名挪开旧的 → 把新的改名就位」（原子、不留残留）。
# 改名失败说明目录本身被占用（典型情形：资源管理器窗口开在里面，或用户正从这个目录
# 启动绿色版）——此时**回退为合并拷贝**：往现有目录里逐文件覆盖，不删目录本身。
# 合并的代价是可能残留本次未产出的旧文件，所以只作为回退并明确提示。
$swapped = $false
if (Test-Path $out) {
    if (Test-Path $previous) { Remove-Item $previous -Recurse -Force -ErrorAction SilentlyContinue }
    try {
        Rename-Item -LiteralPath $out -NewName (Split-Path $previous -Leaf) -ErrorAction Stop
    } catch {
        Write-Host "==> 成品目录被占用（$($_.Exception.Message.Trim())）"
        Write-Host "==> 回退为合并拷贝：不动原目录，逐文件覆盖"
        robocopy $staging $out /E /NJH /NJS /NFL /NDL /NP | Out-Null
        if ($LASTEXITCODE -ge 8) { throw "合并拷贝失败（exit $LASTEXITCODE），原目录未被改动" }
        Remove-Item $staging -Recurse -Force -ErrorAction SilentlyContinue
        $swapped = $true
        Write-Host "==> 已完成（合并方式换位；若有本次已移除的旧文件会残留在成品目录里）"
    }
}
if (-not $swapped) {
    Rename-Item -LiteralPath $staging -NewName (Split-Path $out -Leaf)
    if (Test-Path $previous) {
        Remove-Item $previous -Recurse -Force -ErrorAction SilentlyContinue
        if (Test-Path $previous) { Write-Host "提示：旧目录未能删除（仍被占用），可稍后手动删：$previous" }
    }
}

$total = (Get-ChildItem $out -Recurse -File | Measure-Object Length -Sum).Sum
Write-Host ""
Write-Host "=== 绿色版打包完成 ==="
Write-Host "目录: $out"
Write-Host "体积: $([math]::Round($total/1MB,1)) MB"
Write-Host "说明: 整个目录拷贝到任意 Win10 1809+/Win11 机器即可运行；"
Write-Host "      Win11 自带 WebView2 Runtime；Win10 若缺失请安装 Microsoft Edge WebView2 Runtime"
Write-Host "      （或使用 NSIS 安装版：src-tauri\target\release\bundle\nsis\，已内置 WebView2 离线安装器）。"
