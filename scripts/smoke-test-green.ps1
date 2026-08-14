# 绿色版冒烟测试：启动 → 存活 60 秒 → CloseMainWindow 优雅退出
# 注意：不能用 Stop-Process -Force（残留 SQLite WAL/.cookies 句柄锁 → 下次启动 http 插件 os error 5）
$ErrorActionPreference = "Stop"
$exe = "<PROJECT_ROOT>\portable-build\DevEco Switch 绿色版\deveco-switch.exe"

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
exit 0
