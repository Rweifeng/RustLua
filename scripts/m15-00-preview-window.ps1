[CmdletBinding()]
param(
    [string]$ProjectRoot = "",
    [string]$OutDir = "",
    [int]$DurationSeconds = 10,
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
$reportPath = Join-Path $OutDir "m15-00-preview-window-$timestamp.json"
$editorExe = Join-Path $engineRoot "target\release\editor.exe"
$editorLog = Join-Path $engineRoot "target\release\editor.log"
$autoplayScene = "scenes/main.scene"
if (Test-Path (Join-Path $ProjectRoot "scenes\production.scene")) {
    $autoplayScene = "scenes/production.scene"
} else {
    $projectToml = Join-Path $ProjectRoot "project.toml"
    if (Test-Path $projectToml) {
        $startup = Select-String -Path $projectToml -Pattern '^\s*startup_scene\s*=\s*"([^"]+)"' | Select-Object -First 1
        if ($startup) {
            $autoplayScene = $startup.Matches[0].Groups[1].Value
        }
    }
}

function Invoke-Checked([string]$Exe, [string[]]$Rest, [string]$WorkingDirectory) {
    Write-Host "> $Exe $($Rest -join ' ')" -ForegroundColor DarkGray
    Push-Location $WorkingDirectory
    try {
        $output = & $Exe @Rest
        if ($LASTEXITCODE -ne 0) {
            throw "Command failed (exit $LASTEXITCODE): $Exe $($Rest -join ' ')"
        }
        return @($output)
    }
    finally {
        Pop-Location
    }
}

function Ensure-ReleaseBuild() {
    if ($SkipBuild) {
        return
    }
    Invoke-Checked "cargo" @("build", "--workspace", "--release") $engineRoot | Out-Null
}

function Wait-WindowReady([System.Diagnostics.Process]$Process, [int]$TimeoutMs) {
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    while ($timer.ElapsedMilliseconds -lt $TimeoutMs) {
        if ($Process.HasExited) {
            throw "Editor exited before creating a window (exit $($Process.ExitCode))."
        }
        $Process.Refresh()
        if ($Process.MainWindowHandle -ne 0) {
            return [math]::Round($timer.Elapsed.TotalMilliseconds, 3)
        }
        Start-Sleep -Milliseconds 100
    }
    throw "Timed out waiting for editor window."
}

function Stop-WindowProcess([System.Diagnostics.Process]$Process) {
    if ($Process.HasExited) {
        return
    }
    try {
        $null = $Process.CloseMainWindow()
        if (-not $Process.WaitForExit(10000)) {
            $Process.Kill()
            $Process.WaitForExit(5000) | Out-Null
        }
    }
    catch {
        if (-not $Process.HasExited) {
            $Process.Kill()
            $Process.WaitForExit(5000) | Out-Null
        }
    }
}

Ensure-ReleaseBuild
if (-not (Test-Path $editorExe)) {
    throw "editor.exe not found: $editorExe"
}
if (Test-Path $editorLog) {
    Remove-Item -Force $editorLog
}

$previousAutoplay = $env:P2_EDITOR_AUTOPLAY
$previousAutoplayScene = $env:P2_EDITOR_AUTOPLAY_SCENE
$env:P2_EDITOR_AUTOPLAY = "1"
$env:P2_EDITOR_AUTOPLAY_SCENE = $autoplayScene
try {
    $process = Start-Process -FilePath $editorExe -ArgumentList @("--project", $ProjectRoot) -WorkingDirectory $engineRoot -PassThru
    $windowMs = Wait-WindowReady $process 30000
    Start-Sleep -Seconds $DurationSeconds
    Stop-WindowProcess $process
}
finally {
    if ($null -eq $previousAutoplay) {
        Remove-Item Env:P2_EDITOR_AUTOPLAY -ErrorAction SilentlyContinue
    } else {
        $env:P2_EDITOR_AUTOPLAY = $previousAutoplay
    }
    if ($null -eq $previousAutoplayScene) {
        Remove-Item Env:P2_EDITOR_AUTOPLAY_SCENE -ErrorAction SilentlyContinue
    } else {
        $env:P2_EDITOR_AUTOPLAY_SCENE = $previousAutoplayScene
    }
}

$fpsLines = @()
if (Test-Path $editorLog) {
    $fpsLines = Select-String -Path $editorLog -Pattern "preview fps: .* = ([0-9]+(?:\.[0-9]+)?) fps" | ForEach-Object {
        $value = [double]$_.Matches[0].Groups[1].Value
        [pscustomobject]@{
            line = $_.Line.Trim()
            fps = [math]::Round($value, 3)
        }
    }
}

$fpsValues = @($fpsLines | ForEach-Object { $_.fps })
$report = [ordered]@{
    report_version = "m15-00-preview-window-v1"
    generated_at = (Get-Date).ToString("s")
    project_root = $ProjectRoot
    duration_seconds = $DurationSeconds
    autoplay_scene = $autoplayScene
    editor_exe = $editorExe
    editor_log = $editorLog
    window_ready_ms = $windowMs
    fps_sample_count = $fpsValues.Count
    fps = if ($fpsValues.Count -gt 0) {
        [ordered]@{
            min = [math]::Round(($fpsValues | Measure-Object -Minimum).Minimum, 3)
            max = [math]::Round(($fpsValues | Measure-Object -Maximum).Maximum, 3)
            avg = [math]::Round(($fpsValues | Measure-Object -Average).Average, 3)
            samples = @($fpsLines)
        }
    } else {
        $null
    }
}

$report | ConvertTo-Json -Depth 8 | Set-Content -Path $reportPath -Encoding UTF8
Write-Host "M15-00 preview-window report: $reportPath" -ForegroundColor Green
