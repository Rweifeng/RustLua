[CmdletBinding()]
param(
    [string]$ProjectRoot = "",
    [string]$OutDir = "",
    [switch]$Release
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
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

function Invoke-Checked([string]$Exe, [string[]]$Rest) {
    Write-Host "> $Exe $($Rest -join ' ')" -ForegroundColor DarkGray
    $output = & $Exe @Rest
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed (exit $LASTEXITCODE): $Exe $($Rest -join ' ')"
    }
    return $output
}

function Measure-Checked([string]$Exe, [string[]]$Rest) {
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    $output = Invoke-Checked $Exe $Rest
    $timer.Stop()
    return @{
        elapsed_ms = [math]::Round($timer.Elapsed.TotalMilliseconds, 3)
        output = @($output)
    }
}

function Invoke-Cargo([string[]]$ArgList) {
    return Measure-Checked -Exe "cargo" -Rest $ArgList
}

Push-Location $engineRoot
try {
    $cargoMode = if ($Release) { @("--release") } else { @() }
    $report = [ordered]@{
        report_version = "m15-00-v1"
        generated_at = (Get-Date).ToString("s")
        release = [bool]$Release
        project_root = $ProjectRoot
        metrics = [ordered]@{}
        pending_manual = @(
            "editor_game_view_memory_60m",
            "release_pack_startup_time_real_window"
        )
    }

    $sprite1kArgs = @("run") + $cargoMode + @("-p", "engine", "--example", "sprite_stress", "--", "--sprites", "1000", "--cycles", "60")
    $sprite1k = Invoke-Cargo $sprite1kArgs
    $report.metrics["sprite_1000"] = [ordered]@{
        elapsed_ms = $sprite1k.elapsed_ms
        output = $sprite1k.output
    }

    $sprite10kArgs = @("run") + $cargoMode + @("-p", "engine", "--example", "sprite_stress", "--", "--sprites", "10000", "--cycles", "60")
    $sprite10k = Invoke-Cargo $sprite10kArgs
    $report.metrics["sprite_10000"] = [ordered]@{
        elapsed_ms = $sprite10k.elapsed_ms
        output = $sprite10k.output
    }

    $ecsReport = Join-Path $OutDir "m15-00-ecs-stress-$timestamp.json"
    $ecsArgs = @("run") + $cargoMode + @("-p", "engine", "--example", "ecs_stress", "--", "--entities", "10000", "--seconds", "10", "--output", $ecsReport)
    $ecs = Invoke-Cargo $ecsArgs
    $report.metrics["ecs_10000_10s"] = [ordered]@{
        elapsed_ms = $ecs.elapsed_ms
        output = $ecs.output
        json_report = $ecsReport
    }

    $tilemap100Args = @("run") + $cargoMode + @("-p", "engine", "--example", "tilemap_stress", "--", "--width", "100", "--height", "100", "--cycles", "3")
    $tilemap100 = Invoke-Cargo $tilemap100Args
    $report.metrics["tilemap_100x100"] = [ordered]@{
        elapsed_ms = $tilemap100.elapsed_ms
        output = $tilemap100.output
    }

    $tilemap500Args = @("run") + $cargoMode + @("-p", "engine", "--example", "tilemap_stress", "--", "--width", "500", "--height", "500", "--cycles", "1")
    $tilemap500 = Invoke-Cargo $tilemap500Args
    $report.metrics["tilemap_500x500"] = [ordered]@{
        elapsed_ms = $tilemap500.elapsed_ms
        output = $tilemap500.output
    }

    $thumbnailArgs = @("run") + $cargoMode + @("-p", "editor", "--example", "thumbnail_stress")
    $thumbnail = Invoke-Cargo $thumbnailArgs
    $report.metrics["thumbnail_1000"] = [ordered]@{
        elapsed_ms = $thumbnail.elapsed_ms
        output = $thumbnail.output
    }

    $playStopArgs = @("run") + $cargoMode + @("-p", "editor", "--example", "play_stop_stress")
    $playStop = Invoke-Cargo $playStopArgs
    $report.metrics["play_stop_100"] = [ordered]@{
        elapsed_ms = $playStop.elapsed_ms
        output = $playStop.output
    }

    $assetcImportArgs = @("run") + $cargoMode + @("-p", "assetc", "--", "import", $ProjectRoot)
    $assetcImport = Invoke-Cargo $assetcImportArgs
    $report.metrics["assetc_import"] = [ordered]@{
        elapsed_ms = $assetcImport.elapsed_ms
        output = $assetcImport.output
    }

    $assetcDepsArgs = @("run") + $cargoMode + @("-p", "assetc", "--", "--check-deps", $ProjectRoot)
    $assetcDeps = Invoke-Cargo $assetcDepsArgs
    $report.metrics["assetc_check_deps"] = [ordered]@{
        elapsed_ms = $assetcDeps.elapsed_ms
        output = $assetcDeps.output
    }

    $editorCheckArgs = @("run") + $cargoMode + @("-p", "editor", "--", "--check", "--project", $ProjectRoot)
    $editorCheck = Invoke-Cargo $editorCheckArgs
    $report.metrics["editor_check"] = [ordered]@{
        elapsed_ms = $editorCheck.elapsed_ms
        output = $editorCheck.output
    }

    $gameCheckArgs = @("run") + $cargoMode + @("-p", "game", "--", "--check", "--project", $ProjectRoot)
    $gameCheck = Invoke-Cargo $gameCheckArgs
    $report.metrics["game_check"] = [ordered]@{
        elapsed_ms = $gameCheck.elapsed_ms
        output = $gameCheck.output
    }

    $packDir = Join-Path $workspaceRoot "artifacts\m15\game_test_2d_production_pack"
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $packDir) | Out-Null
    if (Test-Path $packDir) {
        Remove-Item -Recurse -Force $packDir
    }
    $assetcPackageArgs = @("run") + $cargoMode + @("-p", "assetc", "--", "package", $ProjectRoot, "-o", $packDir)
    $assetcPackage = Invoke-Cargo $assetcPackageArgs
    $report.metrics["assetc_package"] = [ordered]@{
        elapsed_ms = $assetcPackage.elapsed_ms
        output = $assetcPackage.output
    }

    $gamePackCheckArgs = @("run") + $cargoMode + @("-p", "game", "--", "--pack", $packDir, "--check")
    $gamePackCheck = Invoke-Cargo $gamePackCheckArgs
    $report.metrics["game_pack_check"] = [ordered]@{
        elapsed_ms = $gamePackCheck.elapsed_ms
        output = $gamePackCheck.output
    }

    $packSize = (Get-ChildItem -Recurse -File $packDir | Measure-Object -Property Length -Sum).Sum
    $packFiles = (Get-ChildItem -Recurse -File $packDir).Count
    $report.metrics["pack_size"] = [ordered]@{
        files = $packFiles
        bytes = $packSize
        mib = [math]::Round($packSize / 1MB, 3)
    }

    $reportPath = Join-Path $OutDir "m15-00-baseline-$timestamp.json"
    $report | ConvertTo-Json -Depth 8 | Set-Content -Path $reportPath -Encoding UTF8
    Write-Host "M15-00 baseline report: $reportPath" -ForegroundColor Green
}
finally {
    Pop-Location
}
