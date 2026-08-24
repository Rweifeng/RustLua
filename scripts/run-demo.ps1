[CmdletBinding()]
param(
    [string]$Name = "game",
    [switch]$Release
)

$ErrorActionPreference = "Stop"
$engineRoot = Join-Path (Split-Path -Parent $PSScriptRoot) "engine"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found. Install Rust stable and ensure cargo is on PATH."
}

Push-Location $engineRoot
try {
    if ($Release) {
        cargo run --release -p $Name
    }
    else {
        cargo run -p $Name
    }
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
finally {
    Pop-Location
}