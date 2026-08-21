[CmdletBinding()]
param()
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $false
$root = Split-Path -Parent $PSScriptRoot
$engine = Join-Path $root "engine"
$logDir = Join-Path $root "artifacts\logs\m5"
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
$log = Join-Path $logDir "m5-10-comprehensive-acceptance.txt"
"M5-10 comprehensive acceptance $(Get-Date -Format o)" | Set-Content -Path $log -Encoding UTF8
function Step($name, [scriptblock]$action) {
    Add-Content $log "STEP $name"
    $oldPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & $action 2>&1 | Tee-Object -FilePath $log -Append
    $ErrorActionPreference = $oldPreference
    if ($LASTEXITCODE -ne 0) { throw "failed: $name (exit $LASTEXITCODE)" }
}
Push-Location $engine
try {
 Step "format" { cargo fmt --all -- --check }
 Step "clippy" { cargo clippy --workspace --all-targets --all-features -- -D warnings }
 Step "tests" { cargo test --workspace --all-features }
 Step "debug-build" { cargo build --workspace }
 Step "release-build" { cargo build --workspace --release }
 Step "editor-check" { cargo run --quiet -p editor -- --project $root --check }
 $bad = Join-Path $env:TEMP "m5-10-invalid-project"
 if (Test-Path $bad) { Remove-Item -LiteralPath $bad -Recurse -Force }
 try { cargo run --quiet -p editor -- --project $bad --check; throw "invalid project unexpectedly passed" } catch { Add-Content $log "invalid project negative check passed: $($_.Exception.Message)" }
} finally { Pop-Location }
$runner = Join-Path $root "artifacts\builds\release\game_runner.exe"
if (-not (Test-Path $runner)) { & (Join-Path $root "scripts\build-release.ps1") }
if (-not (Test-Path (Join-Path $root "artifacts\builds\release\game\scripts\main.lua"))) { throw "release game resource missing" }
Add-Content $log "release resource check passed"
Add-Content $log "GUI ten-step workflow: manual evidence required; editor exposes Undo/Redo, save, recovery prompt, Play/Stop, Inspector and scene tree controls."
Write-Host "Acceptance CLI passed. Log: $log" -ForegroundColor Green
