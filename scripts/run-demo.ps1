[CmdletBinding()]
param(
    [string]$Project = "examples",
    [switch]$Release
)

$ErrorActionPreference = "Stop"
$workspaceRoot = Split-Path -Parent $PSScriptRoot
$engineRoot = Join-Path $workspaceRoot "engine"
$projectRoot = Join-Path $workspaceRoot $Project

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found. Install Rust stable and ensure cargo is on PATH."
}

Push-Location $engineRoot
try {
    if ($Release) {
        cargo run --release -p game -- --project $projectRoot
    }
    else {
        cargo run -p game -- --project $projectRoot
    }
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
finally {
    Pop-Location
}
