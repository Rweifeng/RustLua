[CmdletBinding()]
param(
    # 完整 60 秒 ECS 长跑；默认 10 秒快速冒烟（CI/日常）。
    [switch]$Full,
    # 归档目录（默认为 docs/acceptance-evidence/m7）
    [string]$OutDir = ""
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot
$engineRoot = Join-Path $projectRoot "engine"
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path $projectRoot "docs\acceptance-evidence\m7"
}
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

function Invoke-Checked([string]$Exe, [string[]]$Rest) {
    Write-Host "> $Exe $($Rest -join ' ')" -ForegroundColor DarkGray
    $output = & $Exe @Rest
    if ($LASTEXITCODE -ne 0) {
        throw "命令失败 (exit $LASTEXITCODE): $Exe $($Rest -join ' ')"
    }
    return $output
}

Push-Location $engineRoot
try {
    Write-Host "`n==> M7-04 性能基线" -ForegroundColor Cyan

    Write-Host "`n--> ECS stress: 10000 实体（$($(if ($Full) { 60 } else { 10 })) 秒）" -ForegroundColor Cyan
    $seconds = if ($Full) { 60 } else { 10 }
    $ecsReport = Join-Path $OutDir "ecs-stress-$timestamp.json"
    Invoke-Checked "cargo" @(
        "run", "--release", "-p", "engine_core", "--example", "ecs_stress",
        "--", "--entities", "10000", "--seconds", "$seconds", "--output", $ecsReport
    )
    Write-Host "ECS 报告: $ecsReport" -ForegroundColor Green

    Write-Host "`n--> Sprite stress: 1000 精灵（CPU 批处理吞吐）" -ForegroundColor Cyan
    $spriteOut = Invoke-Checked "cargo" @(
        "run", "--release", "-p", "engine_core", "--example", "sprite_stress"
    )
    $spriteReport = Join-Path $OutDir "sprite-stress-$timestamp.txt"
    $spriteOut | Set-Content -Path $spriteReport -Encoding UTF8
    Write-Host "Sprite 报告: $spriteReport" -ForegroundColor Green

    Write-Host "`n==> 基线完成。结果归档到 $OutDir" -ForegroundColor Green
}
finally {
    Pop-Location
}
