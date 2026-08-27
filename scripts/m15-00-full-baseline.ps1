[CmdletBinding()]
param(
    [string]$ProjectRoot = "",
    [string]$OutDir = "",
    [switch]$Release,
    [switch]$SkipHeadless,
    [switch]$SkipRealWindow,
    [switch]$SkipThumbnailWindow,
    [switch]$SkipPlayStopWindow,
    [switch]$SkipPreviewWindow,
    [switch]$SkipTilemapWindow,
    [int]$DurationSeconds = 600,
    [int]$SampleIntervalSeconds = 5
)

$ErrorActionPreference = "Stop"
$workspaceRoot = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($ProjectRoot)) {
    $ProjectRoot = Join-Path $workspaceRoot "game_test_2d_production"
}
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path $workspaceRoot "docs\acceptance-evidence\m15"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$reportPath = Join-Path $OutDir "m15-00-full-baseline-$timestamp.json"

function Invoke-Script([string]$ScriptPath, [string[]]$Arguments) {
    Write-Host "> powershell -File $ScriptPath $($Arguments -join ' ')" -ForegroundColor DarkGray
    & powershell -ExecutionPolicy Bypass -File $ScriptPath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed (exit $LASTEXITCODE): $ScriptPath $($Arguments -join ' ')"
    }
}

function Get-LatestReport([string]$Pattern, [datetime]$After) {
    Get-ChildItem -Path $OutDir -Filter $Pattern |
        Where-Object { $_.LastWriteTime -ge $After } |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}

$report = [ordered]@{
    report_version = "m15-00-full-baseline-v1"
    generated_at = (Get-Date).ToString("s")
    project_root = $ProjectRoot
    out_dir = $OutDir
    release = [bool]$Release
    duration_seconds = $DurationSeconds
    sample_interval_seconds = $SampleIntervalSeconds
    steps = [ordered]@{}
    notes = @(
        "Unified entry for headless baseline, real-window, preview-window, thumbnail-window, playstop-window, and tilemap-window sampling.",
        "Release pack startup confirmation remains the main open formalization gap."
    )
}

$common = @("-ProjectRoot", $ProjectRoot, "-OutDir", $OutDir)
if ($Release) {
    $common += "-Release"
}

if (-not $SkipHeadless) {
    $started = Get-Date
    Invoke-Script (Join-Path $workspaceRoot "scripts\m15-00-baseline.ps1") $common
    $report.steps["headless_baseline"] = [ordered]@{
        report = Get-LatestReport "m15-00-baseline-*.json" $started
        status = "completed"
    }
}

if (-not $SkipRealWindow) {
    $started = Get-Date
    $args = @(
        "-ProjectRoot", $ProjectRoot,
        "-OutDir", $OutDir,
        "-DurationSeconds", $DurationSeconds,
        "-SampleIntervalSeconds", $SampleIntervalSeconds
    )
    Invoke-Script (Join-Path $workspaceRoot "scripts\m15-00-real-window.ps1") $args
    $report.steps["real_window"] = [ordered]@{
        report = Get-LatestReport "m15-00-real-window-*.json" $started
        status = "completed"
    }
}

if (-not $SkipPreviewWindow) {
    $started = Get-Date
    $args = @(
        "-ProjectRoot", $ProjectRoot,
        "-OutDir", $OutDir,
        "-DurationSeconds", ([Math]::Min($DurationSeconds, 30))
    )
    Invoke-Script (Join-Path $workspaceRoot "scripts\m15-00-preview-window.ps1") $args
    $report.steps["preview_window"] = [ordered]@{
        report = Get-LatestReport "m15-00-preview-window-*.json" $started
        status = "completed"
    }
}

if (-not $SkipThumbnailWindow) {
    $started = Get-Date
    $args = @(
        "-ProjectRoot", $ProjectRoot,
        "-OutDir", $OutDir
    )
    Invoke-Script (Join-Path $workspaceRoot "scripts\m15-00-thumbnail-window.ps1") $args
    $report.steps["thumbnail_window"] = [ordered]@{
        report = Get-LatestReport "m15-00-thumbnail-window-*.json" $started
        status = "completed"
    }
}

if (-not $SkipPlayStopWindow) {
    $started = Get-Date
    $args = @(
        "-ProjectRoot", $ProjectRoot,
        "-OutDir", $OutDir
    )
    Invoke-Script (Join-Path $workspaceRoot "scripts\m15-00-playstop-window.ps1") $args
    $report.steps["playstop_window"] = [ordered]@{
        report = Get-LatestReport "m15-00-playstop-window-*.json" $started
        status = "completed"
    }
}

if (-not $SkipTilemapWindow) {
    $started = Get-Date
    $args = @(
        "-ProjectRoot", $ProjectRoot,
        "-OutDir", $OutDir
    )
    Invoke-Script (Join-Path $workspaceRoot "scripts\m15-00-tilemap-window.ps1") $args
    $report.steps["tilemap_window"] = [ordered]@{
        report = Get-LatestReport "m15-00-tilemap-window-*.json" $started
        status = "completed"
    }
}

$report | ConvertTo-Json -Depth 8 | Set-Content -Path $reportPath -Encoding UTF8
Write-Host "M15-00 full baseline report: $reportPath" -ForegroundColor Green
