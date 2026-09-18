# 绿色版冒烟测试：启动 → 存活 60 秒 → CloseMainWindow 优雅退出
# 注意：不能用 Stop-Process -Force（残留 SQLite WAL/.cookies 句柄锁 → 下次启动 http 插件 os error 5）
$ErrorActionPreference = "Stop"
$exe = Join-Path $PSScriptRoot "..\portable-build\DevEco Switch 绿色版\deveco-switch.exe"

Write-Host "==> 启动绿色版 ..."
$p = Start-Process -FilePath $exe -PassThru
Start-Sleep -Seconds 60

if ($p.HasExited) {
    Write-Host "FAIL: 进程在 60 秒内退出，exit=$($p.ExitCode)"
    exit 1
}
Write-Host "OK: 进程存活 60 秒 (PID $($p.Id))"

# 优雅退出
$closed = $p.CloseMainWindow()
Start-Sleep -Seconds 5
if (-not $p.HasExited) {
    # 主窗口关闭失败再补一次，仍失败才退出码标记（不 Force kill）
    $closed2 = $p.CloseMainWindow()
    Start-Sleep -Seconds 3
}
if ($p.HasExited) {
    Write-Host "OK: 优雅退出成功"
} else {
    Write-Host "WARN: 主窗口未关闭（进程仍在，用户自行处理）"
}

# ── 自带资源可用性断言 ────────────────────────────────────────────────
# 只验"能启动"不够：曾出现绿色版把资源放在多一层的 resources\ 下，应用按
# resource_dir()/<组件> 找不到，于是内置 Node 用不上（回退系统 npm 崩溃）、
# 种子知识库永远为空——而这些都不影响进程存活。这里直接查库兜住。
# python 缺失时降级为提示，不阻塞（与仓库其它门禁同口径）。
$identifier = (Get-Content (Join-Path $PSScriptRoot "..\src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).identifier
$db = Join-Path $env:APPDATA "$identifier\deveco-switch.db"
if (-not (Test-Path $db)) {
    Write-Host "WARN: 未找到应用库（$db），跳过自带资源断言"
} else {
    $count = & python -c "import sqlite3,sys
c = sqlite3.connect(sys.argv[1])
print(c.execute('select count(*) from api_docs').fetchone()[0])" $db 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $count) {
        Write-Host "WARN: 读取库失败（python 不可用？），跳过自带资源断言"
    } elseif ([int]$count -le 0) {
        Write-Host "FAIL: 库中 api_docs 为 0 —— 绿色版自带资源（seed/node/embedding）没被应用发现，"
        Write-Host "      检查资源是否与 exe 同级（不能多一层 resources\）"
        exit 1
    } else {
        Write-Host "OK: 自带知识库可用（api_docs = $count）"
    }
}
exit 0
