[CmdletBinding()]
param(
    [string]$ProjectRoot = "",
    [string]$OutDir = "",
    [int]$DurationSeconds = 3600,
    [int]$SampleIntervalSeconds = 5,
    [switch]$SkipBuild,
    [switch]$SkipEditor,
    [switch]$SkipGame,
    [switch]$DisableWindowKeepAlive
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
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$distRoot = Join-Path $workspaceRoot "artifacts\m15\window-release"
$packRoot = Join-Path $distRoot "pack"
$reportPath = Join-Path $OutDir "m15-00-real-window-$timestamp.json"

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
New-Item -ItemType Directory -Force -Path $distRoot | Out-Null

if (-not ("NativeWindow" -as [type])) {
    Add-Type @"
using System;
using System.Runtime.InteropServices;

public static class NativeWindow {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [DllImport("user32.dll")]
    public static extern bool ShowWindowAsync(IntPtr hWnd, int nCmdShow);

    [DllImport("user32.dll")]
    public static extern bool IsIconic(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);

    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);
}
"@
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

function Prepare-ReleasePack() {
    $assetc = Join-Path $engineRoot "target\release\assetc.exe"
    $gameExe = Join-Path $engineRoot "target\release\game.exe"
    $editorExe = Join-Path $engineRoot "target\release\editor.exe"
    $fontSource = Join-Path $ProjectRoot "assets\fonts\NotoSansSC-VF.ttf"
    if (-not (Test-Path $assetc)) { throw "assetc.exe not found: $assetc" }
    if (-not (Test-Path $gameExe)) { throw "game.exe not found: $gameExe" }
    if (-not (Test-Path $editorExe)) { throw "editor.exe not found: $editorExe" }
    if (-not (Test-Path $fontSource)) { throw "font not found: $fontSource" }

    if (Test-Path $packRoot) {
        Remove-Item -Recurse -Force $packRoot
    }
    Invoke-Checked $assetc @("package", $ProjectRoot, "-o", $packRoot) $engineRoot | Out-Null

    Copy-Item -Path $gameExe -Destination (Join-Path $distRoot "game.exe") -Force
    $fontTargetDir = Join-Path $distRoot "fonts"
    New-Item -ItemType Directory -Force -Path $fontTargetDir | Out-Null
    Copy-Item -Path $fontSource -Destination (Join-Path $fontTargetDir "NotoSansSC-VF.ttf") -Force

    return @{
        editor_exe = $editorExe
        game_exe = (Join-Path $distRoot "game.exe")
        pack_root = $packRoot
    }
}

function Wait-WindowReady([System.Diagnostics.Process]$Process, [int]$TimeoutMs) {
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    $inputIdleMs = $null
    $windowMs = $null
    while ($timer.ElapsedMilliseconds -lt $TimeoutMs) {
        if ($Process.HasExited) {
            throw "Process exited before creating a window (exit $($Process.ExitCode))."
        }
        if ($null -eq $inputIdleMs) {
            try {
                if ($Process.WaitForInputIdle(200)) {
                    $inputIdleMs = [math]::Round($timer.Elapsed.TotalMilliseconds, 3)
                }
            }
            catch {
                # Some processes do not expose an input-idle state; keep polling.
            }
        }
        $Process.Refresh()
        if ($Process.MainWindowHandle -ne 0) {
            $windowMs = [math]::Round($timer.Elapsed.TotalMilliseconds, 3)
            break
        }
        Start-Sleep -Milliseconds 100
    }
    if ($null -eq $windowMs) {
        throw "Timed out waiting for process window."
    }
    return @{
        input_idle_ms = $inputIdleMs
        main_window_ms = $windowMs
        pid = $Process.Id
    }
}

function New-MemorySample([System.Diagnostics.Process]$Process, [double]$ElapsedSeconds) {
    $Process.Refresh()
    return [pscustomobject][ordered]@{
        elapsed_s = [math]::Round($ElapsedSeconds, 3)
        working_set_mb = [math]::Round($Process.WorkingSet64 / 1MB, 3)
        private_mb = [math]::Round($Process.PrivateMemorySize64 / 1MB, 3)
        paged_mb = [math]::Round($Process.PagedMemorySize64 / 1MB, 3)
        virtual_mb = [math]::Round($Process.VirtualMemorySize64 / 1MB, 3)
        handles = $Process.HandleCount
    }
}

function Ensure-WindowVisible([System.Diagnostics.Process]$Process) {
    $Process.Refresh()
    $handle = $Process.MainWindowHandle
    if ($handle -eq 0) {
        return [pscustomobject][ordered]@{
            applied = $false
            minimized = $false
            invisible = $false
            zero_size = $false
        }
    }

    $rect = New-Object NativeWindow+RECT
    $hasRect = [NativeWindow]::GetWindowRect($handle, [ref]$rect)
    $width = if ($hasRect) { $rect.Right - $rect.Left } else { -1 }
    $height = if ($hasRect) { $rect.Bottom - $rect.Top } else { -1 }
    $minimized = [NativeWindow]::IsIconic($handle)
    $visible = [NativeWindow]::IsWindowVisible($handle)
    $zeroSize = $hasRect -and ($width -le 0 -or $height -le 0)
    $needsRestore = $minimized -or (-not $visible) -or $zeroSize

    if ($needsRestore) {
        [void][NativeWindow]::ShowWindowAsync($handle, 9)
        [void][NativeWindow]::SetForegroundWindow($handle)
    }

    return [pscustomobject][ordered]@{
        applied = $needsRestore
        minimized = $minimized
        invisible = (-not $visible)
        zero_size = $zeroSize
    }
}

function Get-SampleStats([object[]]$Samples, [string]$Field) {
    $values = @($Samples | ForEach-Object { [double]$_.$Field })
    if ($values.Count -eq 0) {
        return $null
    }
    return [ordered]@{
        min = [math]::Round(($values | Measure-Object -Minimum).Minimum, 3)
        max = [math]::Round(($values | Measure-Object -Maximum).Maximum, 3)
        avg = [math]::Round(($values | Measure-Object -Average).Average, 3)
    }
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

function Measure-WindowProcess(
    [string]$Label,
    [string]$FilePath,
    [string[]]$ArgumentList,
    [string]$WorkingDirectory
) {
    Write-Host "==> Measure $Label" -ForegroundColor Cyan
    $process = Start-Process -FilePath $FilePath -ArgumentList $ArgumentList -WorkingDirectory $WorkingDirectory -PassThru
    $startup = Wait-WindowReady $process 30000
    $restoreCount = 0
    $minimizedCount = 0
    $invisibleCount = 0
    $zeroSizeCount = 0
    if (-not $DisableWindowKeepAlive) {
        $windowState = Ensure-WindowVisible $process
        if ($windowState.applied) { $restoreCount += 1 }
        if ($windowState.minimized) { $minimizedCount += 1 }
        if ($windowState.invisible) { $invisibleCount += 1 }
        if ($windowState.zero_size) { $zeroSizeCount += 1 }
    }
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    $samples = [System.Collections.Generic.List[object]]::new()
    $samples.Add((New-MemorySample $process 0.0))
    while ($timer.Elapsed.TotalSeconds -lt $DurationSeconds) {
        if ($process.HasExited) {
            break
        }
        Start-Sleep -Seconds $SampleIntervalSeconds
        if ($process.HasExited) {
            break
        }
        if (-not $DisableWindowKeepAlive) {
            $windowState = Ensure-WindowVisible $process
            if ($windowState.applied) { $restoreCount += 1 }
            if ($windowState.minimized) { $minimizedCount += 1 }
            if ($windowState.invisible) { $invisibleCount += 1 }
            if ($windowState.zero_size) { $zeroSizeCount += 1 }
        }
        $samples.Add((New-MemorySample $process $timer.Elapsed.TotalSeconds))
    }
    $process.Refresh()
    $exitedEarly = $process.HasExited
    $exitCode = if ($exitedEarly) { $process.ExitCode } else { $null }
    if (-not $exitedEarly) {
        Stop-WindowProcess $process
    }
    $csvPath = Join-Path $OutDir ("m15-00-{0}-memory-{1}.csv" -f $Label, $timestamp)
    $samples | Export-Csv -Path $csvPath -NoTypeInformation -Encoding UTF8
    return [ordered]@{
        label = $Label
        command = @($FilePath) + $ArgumentList
        startup = $startup
        duration_seconds = $DurationSeconds
        sample_interval_seconds = $SampleIntervalSeconds
        sample_count = $samples.Count
        exited_early = $exitedEarly
        exit_code = $exitCode
        working_set_mb = Get-SampleStats $samples "working_set_mb"
        private_mb = Get-SampleStats $samples "private_mb"
        paged_mb = Get-SampleStats $samples "paged_mb"
        virtual_mb = Get-SampleStats $samples "virtual_mb"
        handle_count = Get-SampleStats $samples "handles"
        window_keepalive = [ordered]@{
            enabled = (-not $DisableWindowKeepAlive)
            restore_count = $restoreCount
            minimized_count = $minimizedCount
            invisible_count = $invisibleCount
            zero_size_count = $zeroSizeCount
        }
        csv_report = $csvPath
    }
}

Ensure-ReleaseBuild
$prepared = Prepare-ReleasePack

$report = [ordered]@{
    report_version = "m15-00-window-v1"
    generated_at = (Get-Date).ToString("s")
    project_root = $ProjectRoot
    duration_seconds = $DurationSeconds
    sample_interval_seconds = $SampleIntervalSeconds
    metrics = [ordered]@{}
}

if (-not $SkipEditor) {
    $report.metrics["editor_window_memory"] = Measure-WindowProcess `
        -Label "editor" `
        -FilePath $prepared.editor_exe `
        -ArgumentList @("--project", $ProjectRoot) `
        -WorkingDirectory $engineRoot
}

if (-not $SkipGame) {
    $report.metrics["release_pack_window"] = Measure-WindowProcess `
        -Label "game" `
        -FilePath $prepared.game_exe `
        -ArgumentList @("--pack", $prepared.pack_root) `
        -WorkingDirectory $distRoot
}

$report | ConvertTo-Json -Depth 8 | Set-Content -Path $reportPath -Encoding UTF8
Write-Host "M15-00 real-window report: $reportPath" -ForegroundColor Green
