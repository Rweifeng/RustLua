[CmdletBinding()]
param(
    [string]$ProjectRoot = "",
    [string]$OutDir = "",
    [int]$Iterations = 100,
    [string]$Scene = "scenes/production.scene",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$workspaceRoot = Split-Path -Parent $PSScriptRoot
$engineRoot = Join-Path $workspaceRoot "engine"
if ([string]::IsNullOrWhiteSpace($ProjectRoot)) {
    $ProjectRoot = Join-Path $workspaceRoot "game_test_2d_production"
}
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path $workspaceRoot "docs\acceptance-evidence\m15"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$reportPath = Join-Path $OutDir "m15-00-playstop-window-$timestamp.json"
$editorExe = Join-Path $engineRoot "target\release\editor.exe"
$editorLog = Join-Path $engineRoot "target\release\editor.log"

function Invoke-Checked([string]$Exe, [string[]]$Rest, [string]$WorkingDirectory) {
    Push-Location $WorkingDirectory
    try {
        & $Exe @Rest
        if ($LASTEXITCODE -ne 0) {
            throw "Command failed (exit $LASTEXITCODE): $Exe $($Rest -join ' ')"
        }
    }
    finally {
        Pop-Location
    }
}

if (-not $SkipBuild) {
    Invoke-Checked "cargo" @("build", "-p", "editor", "--release") $engineRoot
}
if (-not (Test-Path $editorExe)) {
    throw "editor.exe not found: $editorExe"
}
if (Test-Path $editorLog) {
    Remove-Item -Force $editorLog
}

$previousBench = $env:P2_EDITOR_PLAYSTOP_BENCH
$previousScene = $env:P2_EDITOR_AUTOPLAY_SCENE
$env:P2_EDITOR_PLAYSTOP_BENCH = "$Iterations"
$env:P2_EDITOR_AUTOPLAY_SCENE = $Scene
try {
    $process = Start-Process -FilePath $editorExe -ArgumentList @("--project", $ProjectRoot) -WorkingDirectory $engineRoot -PassThru
    if (-not $process.WaitForExit(60000)) {
        $process.Kill()
        throw "Timed out waiting for playstop benchmark to finish."
    }
}
finally {
    if ($null -eq $previousBench) {
        Remove-Item Env:P2_EDITOR_PLAYSTOP_BENCH -ErrorAction SilentlyContinue
    } else {
        $env:P2_EDITOR_PLAYSTOP_BENCH = $previousBench
    }
    if ($null -eq $previousScene) {
        Remove-Item Env:P2_EDITOR_AUTOPLAY_SCENE -ErrorAction SilentlyContinue
    } else {
        $env:P2_EDITOR_AUTOPLAY_SCENE = $previousScene
    }
}

$match = Select-String -Path $editorLog -Pattern "playstop window: iterations=(\d+) total_ms=([0-9.]+) avg_ms=([0-9.]+) entities=(\d+)$" | Select-Object -Last 1
$report = [ordered]@{
    report_version = "m15-00-playstop-window-v1"
    generated_at = (Get-Date).ToString("s")
    project_root = $ProjectRoot
    iterations = $Iterations
    scene = $Scene
    editor_exe = $editorExe
    editor_log = $editorLog
    matched = $false
}
if ($match) {
    $groups = $match.Matches[0].Groups
    $report.matched = $true
    $report.result = [ordered]@{
        iterations = [int]$groups[1].Value
        total_ms = [double]$groups[2].Value
        avg_ms = [double]$groups[3].Value
        entities = [int]$groups[4].Value
        line = $match.Line.Trim()
    }
}

$report | ConvertTo-Json -Depth 8 | Set-Content -Path $reportPath -Encoding UTF8
Write-Host "M15-00 playstop-window report: $reportPath" -ForegroundColor Green
