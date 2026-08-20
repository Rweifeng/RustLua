[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot
$engineRoot = Join-Path $projectRoot "engine"
$buildRoot = Join-Path $projectRoot "artifacts\builds\release"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found. Install Rust stable and ensure cargo is on PATH."
}

New-Item -ItemType Directory -Force -Path $buildRoot | Out-Null
Push-Location $engineRoot
try {
    cargo build --workspace --release
    if ($LASTEXITCODE -ne 0) { throw "Release build failed." }

    $runner = Join-Path $engineRoot "target\release\game_runner.exe"
    if (-not (Test-Path $runner)) {
        throw "Release runner was not found: $runner"
    }

    Copy-Item -Force $runner (Join-Path $buildRoot "game_runner.exe")
    Write-Host "Release build complete: $buildRoot" -ForegroundColor Green
}
finally {
    Pop-Location
}