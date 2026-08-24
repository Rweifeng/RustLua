[CmdletBinding()]
param(
    # Run the directory-isolation self test (copy to a temp dir with spaces,
    # launch the packaged game --pack without any source/target/.cache).
    [switch]$SelfTest,
    # Compress Lua/scene resources in the release pack (present = on).
    [switch]$Compress,
    # Encrypt Lua/scene resources in the release pack (present = on).
    [switch]$Encrypt
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

    Write-Host "==> assetc package (data resource pack, compress=$Compress encrypt=$Encrypt)" -ForegroundColor Cyan
    $assetc = Join-Path $engineRoot "target\release\assetc.exe"
    if (Test-Path $packRoot) { Remove-Item -LiteralPath $packRoot -Recurse -Force }
    $assetcArgs = @("package", (Join-Path $projectRoot "game_test"), "-o", $packRoot)
    if ($Compress) { $assetcArgs += "--compress" }
    if ($Encrypt) { $assetcArgs += "--encrypt" }
    & $assetc @assetcArgs
    if ($LASTEXITCODE -ne 0) { throw "assetc package failed." }

    Write-Host "==> Copy runner and docs" -ForegroundColor Cyan
    # 发布包只含运行器；assetc 为开发工具，不随包分发（并清理历史残留）。
    Remove-Item -Path (Join-Path $distRoot "assetc.exe") -Force -ErrorAction SilentlyContinue
    Copy-Item -Path (Join-Path $engineRoot "target\release\game.exe") `
        -Destination (Join-Path $distRoot "game.exe") -Force

    $fontSource = Join-Path $projectRoot "game_test\assets\fonts\NotoSansSC-VF.ttf"
    if (-not (Test-Path $fontSource)) { throw "Game CJK font resource was not found: $fontSource" }
    if ((Get-Item $fontSource).Length -le 0) { throw "Game CJK font resource is empty: $fontSource" }
    $fontTarget = Join-Path $distRoot "fonts\NotoSansSC-VF.ttf"
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $fontTarget) | Out-Null
    Copy-Item -Path $fontSource -Destination $fontTarget -Force
    if (-not (Test-Path $fontTarget) -or (Get-Item $fontTarget).Length -le 0) {
        throw "Release package font is missing or empty: fonts/NotoSansSC-VF.ttf"
    }

    $packCheckOutput = & (Join-Path $distRoot "game.exe") --pack $packRoot --check 2>&1
    $packCheckExit = $LASTEXITCODE
    Write-Host $packCheckOutput
    if ($packCheckExit -ne 0) { throw "Release package check failed (exit $packCheckExit)." }

    $engineVersion = (Get-Content (Join-Path $engineRoot "Cargo.toml") | Select-String '^version = "0\.' | Select-Object -First 1).ToString().Trim().Split('=')[1].Trim().Trim('"')
    $toolchain = & rustc --version 2>$null
    cargo tree -e normal --prefix none 2>$null | Sort-Object -Unique | Set-Content (Join-Path $distRoot "deps.txt") -Encoding UTF8
    @(
        "RustLua game engine release package",
        "build_time: $timestamp",
        "engine version: $engineVersion",
        "toolchain: $toolchain",
        "contents: game.exe, fonts/NotoSansSC-VF.ttf, pack/ (data), run.txt, deps.txt",
        "font_check: fonts/NotoSansSC-VF.ttf present and non-empty"
    ) | Set-Content (Join-Path $distRoot "build-info.txt") -Encoding UTF8

    @(
        "Run instructions",
        "1) Double-click game.exe, or run: game.exe --pack pack",
        "2) Controls: WASD move, SPACE attack, F1 save, F2 load, F4 switch scene",
        "3) Logs and crash reports are written next to game.exe",
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
    $isolatedRunner = Join-Path $isolated "p2-game\game.exe"
    $isolatedPack = Join-Path $isolated "p2-game\pack"
    if (-not (Test-Path $isolatedRunner)) { throw "isolated dir is missing the runner" }
    $isolatedFont = Join-Path $isolated "p2-game\fonts\NotoSansSC-VF.ttf"
    if (-not (Test-Path $isolatedFont) -or (Get-Item $isolatedFont).Length -le 0) {
        throw "isolated dir is missing a non-empty fonts/NotoSansSC-VF.ttf"
    }

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
