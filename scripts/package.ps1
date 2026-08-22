[CmdletBinding()]
param(
    # Run the directory-isolation self test (copy to a temp dir with spaces,
    # launch the packaged game_runner --pack without any source/target/.cache).
    [switch]$SelfTest
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot
$engineRoot = Join-Path $projectRoot "engine"
$distRoot = Join-Path $projectRoot "artifacts\builds\release\p2-game"
$packRoot = Join-Path $distRoot "pack"
$timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found. Install Rust stable and ensure cargo is on PATH."
}

New-Item -ItemType Directory -Force -Path $distRoot | Out-Null
Push-Location $engineRoot
try {
    Write-Host "==> Release build" -ForegroundColor Cyan
    cargo build --workspace --release
    if ($LASTEXITCODE -ne 0) { throw "Release build failed." }

    Write-Host "==> assetc package (data resource pack)" -ForegroundColor Cyan
    $assetc = Join-Path $engineRoot "target\release\assetc.exe"
    if (Test-Path $packRoot) { Remove-Item -LiteralPath $packRoot -Recurse -Force }
    & $assetc package (Join-Path $projectRoot "game") -o $packRoot
    if ($LASTEXITCODE -ne 0) { throw "assetc package failed." }

    Write-Host "==> Copy runner and docs" -ForegroundColor Cyan
    Copy-Item -Force (Join-Path $engineRoot "target\release\game_runner.exe") (Join-Path $distRoot "game_runner.exe")
    Copy-Item -Force (Join-Path $engineRoot "target\release\assetc.exe") (Join-Path $distRoot "assetc.exe")

    $engineVersion = (Get-Content (Join-Path $engineRoot "Cargo.toml") | Select-String '^version = "0\.' | Select-Object -First 1).ToString().Trim().Split('=')[1].Trim().Trim('"')
    $toolchain = & rustc --version 2>$null
    cargo tree -e normal --prefix none 2>$null | Sort-Object -Unique | Set-Content (Join-Path $distRoot "deps.txt") -Encoding UTF8
    @(
        "RustLua game engine release package",
        "build_time: $timestamp",
        "engine_core version: $engineVersion",
        "toolchain: $toolchain",
        "contents: game_runner.exe, assetc.exe, pack/ (data), run.txt, deps.txt"
    ) | Set-Content (Join-Path $distRoot "build-info.txt") -Encoding UTF8

    @(
        "Run instructions",
        "1) Double-click game_runner.exe, or run: game_runner.exe --pack pack",
        "2) Controls: WASD move, SPACE attack, F1 save, F2 load, F4 switch scene",
        "3) Logs and crash reports are written next to game_runner.exe",
        "4) This package has no source or toolchain dependency and runs without Rust/IDE"
    ) | Set-Content (Join-Path $distRoot "run.txt") -Encoding UTF8

    Write-Host "Release package ready: $distRoot" -ForegroundColor Green
    $total = Get-ChildItem -Recurse -File $distRoot | Measure-Object -Property Length -Sum
    Write-Host ("Package size: {0:N1} KB, {1} files" -f ($total.Sum / 1KB), $total.Count)
}
finally {
    Pop-Location
}

if ($SelfTest) {
    Write-Host "`n==> Directory-isolation self test (no source/target/.cache, path with spaces)" -ForegroundColor Cyan
    $isolated = Join-Path $env:TEMP ("p2 release test " + (Get-Date -Format "HHmmss"))
    New-Item -ItemType Directory -Force -Path $isolated | Out-Null
    Copy-Item -LiteralPath $distRoot -Destination $isolated -Recurse -Force
    $isolatedRunner = Join-Path $isolated "p2-game\game_runner.exe"
    $isolatedPack = Join-Path $isolated "p2-game\pack"
    if (-not (Test-Path $isolatedRunner)) { throw "isolated dir is missing the runner" }

    Write-Host "Check: $isolatedRunner --pack $isolatedPack --check" -ForegroundColor DarkGray
    # 用 --check 做无窗口校验：退出码 0 表示发布包完整（含空格路径 + 无源码环境）。
    $checkOutput = & $isolatedRunner --pack $isolatedPack --check 2>&1
    $checkExit = $LASTEXITCODE
    Write-Host $checkOutput
    Remove-Item -LiteralPath $isolated -Recurse -Force
    if ($checkExit -ne 0) {
        throw "isolated check failed (exit $checkExit): package incomplete or path incompatible."
    }
    Write-Host "Directory-isolation self test passed (spaces path + no-source check OK)" -ForegroundColor Green
}
