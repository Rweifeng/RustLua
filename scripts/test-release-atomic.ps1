# test-release-atomic.ps1（M20-03）：原子发布交换与回滚的独立回归。
#
# 用临时目录模拟发布根，覆盖：
#   1. 成功交换：旧内容全部替换为新内容，无 staging/backup 残留；
#   2. 失败回滚（文件占用）：任一步失败后最终目录仍是完整旧版本，且不残留新内容；
#   3. 失败丢弃（staging 未完成）：最终目录完全不受影响；
#   4. 遗留 staging/backup 清理（只清超过阈值的、只按前缀匹配）。
#
# 用法：powershell -NoProfile -File scripts/test-release-atomic.ps1

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "release-atomic.ps1")

$failures = @()
function Assert-True([bool]$Condition, [string]$Message) {
    if ($Condition) {
        Write-Host "  ok - $Message" -ForegroundColor DarkGray
    }
    else {
        Write-Host "  FAIL - $Message" -ForegroundColor Red
        $script:failures += $Message
    }
}

function New-TestRelease([string]$name) {
    $root = Join-Path $env:TEMP ("release-atomic-" + $name + "-" + [guid]::NewGuid().ToString("N").Substring(0, 6))
    $final = Join-Path $root "game"
    New-Item -ItemType Directory -Force -Path (Join-Path $final "pack") | Out-Null
    Set-Content -Path (Join-Path $final "game.exe") -Value "old-runner" -Encoding UTF8
    Set-Content -Path (Join-Path $final "pack/manifest.txt") -Value "old-manifest" -Encoding UTF8
    return @{ Root = $root; Final = $final }
}

function New-StagedContent([string]$stage) {
    New-Item -ItemType Directory -Force -Path (Join-Path $stage "pack") | Out-Null
    Set-Content -Path (Join-Path $stage "game.exe") -Value "new-runner" -Encoding UTF8
    Set-Content -Path (Join-Path $stage "pack/manifest.txt") -Value "new-manifest" -Encoding UTF8
}

Write-Host "== 1. 成功交换 ==" -ForegroundColor Cyan
$case = New-TestRelease "success"
$stage = New-ReleaseStaging -FinalRoot $case.Final
New-StagedContent -Stage $stage
$summary = Complete-ReleaseSwap -FinalRoot $case.Final -StageRoot $stage
Assert-True ((Get-Content (Join-Path $case.Final "game.exe") -Raw).Trim() -eq "new-runner") "最终目录为新版本 runner"
Assert-True ((Get-Content (Join-Path $case.Final "pack/manifest.txt") -Raw).Trim() -eq "new-manifest") "最终目录为新版本资源清单"
Assert-True (-not (Test-Path -LiteralPath $stage)) "staging 已清理"
Assert-True (@(Get-ChildItem -LiteralPath $case.Final -Force -Directory | Where-Object { $_.Name -like ".backup-*" }).Count -eq 0) "无 backup 残留"
Assert-True ($summary.MovedIn.Count -ge 2) "交换摘要记录移入项"
Remove-Item -LiteralPath $case.Root -Recurse -Force

Write-Host "== 2. 失败回滚（文件占用） ==" -ForegroundColor Cyan
$case = New-TestRelease "rollback"
$stage = New-ReleaseStaging -FinalRoot $case.Final
New-StagedContent -Stage $stage
# 占住最终目录里的一个文件，使「旧内容移入 backup」这一步失败。
$locked = Join-Path $case.Final "game.exe"
$handle = [System.IO.File]::Open($locked, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::None)
$threw = $false
try {
    Complete-ReleaseSwap -FinalRoot $case.Final -StageRoot $stage | Out-Null
}
catch {
    $threw = $true
    Assert-True ($_.Exception.Message -like "*release swap failed*") "失败信息标明交换失败"
}
finally {
    $handle.Close()
}
Assert-True $threw "文件占用时交换抛错（不静默成功）"
Assert-True ((Get-Content (Join-Path $case.Final "game.exe") -Raw).Trim() -eq "old-runner") "回滚后旧 runner 仍在原位"
Assert-True ((Get-Content (Join-Path $case.Final "pack/manifest.txt") -Raw).Trim() -eq "old-manifest") "回滚后旧资源清单仍在原位"
Assert-True (@(Get-ChildItem -LiteralPath $case.Final -Force -Directory | Where-Object { $_.Name -like ".backup-*" }).Count -eq 0) "回滚后无 backup 残留"
Assert-True (Test-Path -LiteralPath $stage) "回滚后 staging 仍保留（可诊断/重试）"
Remove-Item -LiteralPath $stage -Recurse -Force
Remove-Item -LiteralPath $case.Root -Recurse -Force

Write-Host "== 3. 失败丢弃（staging 未完成） ==" -ForegroundColor Cyan
$case = New-TestRelease "discard"
$stage = New-ReleaseStaging -FinalRoot $case.Final
New-StagedContent -Stage $stage
Assert-True (Undo-ReleaseStaging -StageRoot $stage) "staging 丢弃成功"
Assert-True (-not (Test-Path -LiteralPath $stage)) "staging 已删除"
Assert-True ((Get-Content (Join-Path $case.Final "game.exe") -Raw).Trim() -eq "old-runner") "丢弃后最终目录完全不受影响"
Remove-Item -LiteralPath $case.Root -Recurse -Force

Write-Host "== 4. 遗留 staging/backup 清理 ==" -ForegroundColor Cyan
$case = New-TestRelease "stale"
$staleStage = Join-Path $case.Final ".staging-20200101-000000-deadbe"
$staleBackup = Join-Path $case.Final ".backup-20200101-000000-deadbe"
New-Item -ItemType Directory -Force -Path $staleStage, $staleBackup | Out-Null
(Get-Item -LiteralPath $staleStage).LastWriteTime = (Get-Date).AddDays(-3)
(Get-Item -LiteralPath $staleBackup).LastWriteTime = (Get-Date).AddDays(-3)
$freshStage = New-ReleaseStaging -FinalRoot $case.Final
$removed = Clear-StaleReleaseStaging -FinalRoot $case.Final -MaxAgeHours 24
Assert-True ($removed.Count -eq 2) "清理了 2 个过期目录"
Assert-True (-not (Test-Path -LiteralPath $staleStage)) "过期 staging 已删除"
Assert-True (-not (Test-Path -LiteralPath $staleBackup)) "过期 backup 已删除"
Assert-True (Test-Path -LiteralPath $freshStage) "新 staging 未被误删"
Assert-True (Test-Path -LiteralPath (Join-Path $case.Final "game.exe")) "发布内容未被误删"
Remove-Item -LiteralPath $case.Root -Recurse -Force

if ($failures.Count -gt 0) {
    Write-Host "`nrelease-atomic test FAILED: $($failures.Count) 项" -ForegroundColor Red
    exit 1
}
Write-Host "`nrelease_atomic_ok cases=4" -ForegroundColor Green
exit 0
