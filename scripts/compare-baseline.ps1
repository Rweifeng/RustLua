[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Current,
    [Parameter(Mandatory = $true)][string]$Previous,
    [double]$P95WarningPercent = 5,
    [double]$P99WarningPercent = 10
)

$ErrorActionPreference = "Stop"
function Read-Json([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { throw "Baseline not found: $Path" }
    return Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
}
function Percent-Delta([double]$Old, [double]$New) {
    if ($Old -eq 0) { return $(if ($New -eq 0) { 0 } else { 100 }) }
    return (($New - $Old) / $Old) * 100
}

$currentReport = Read-Json $Current
$previousReport = Read-Json $Previous
if ($currentReport.report_version -ne $previousReport.report_version) {
    throw "Baseline schema mismatch: $($previousReport.report_version) -> $($currentReport.report_version)"
}
$warnings = 0
$errors = 0
foreach ($now in $currentReport.stairs) {
    $old = $previousReport.stairs | Where-Object { $_.mesh_count -eq $now.mesh_count } | Select-Object -First 1
    if ($null -eq $old) { throw "Missing previous stair mesh_count=$($now.mesh_count)" }
    foreach ($metric in @("draw_calls", "instances", "triangles")) {
        if ($now.render.$metric -ne $old.render.$metric) {
            Write-Host "ERROR mesh_count=$($now.mesh_count) $metric $($old.render.$metric) -> $($now.render.$metric)" -ForegroundColor Red
            $errors++
        }
    }
    foreach ($percentile in @(@("p95_us", $P95WarningPercent), @("p99_us", $P99WarningPercent))) {
        $field = $percentile[0]
        $limit = [double]$percentile[1]
        $delta = Percent-Delta $old.samples.$field $now.samples.$field
        if ($delta -gt $limit) {
            Write-Host ("WARNING mesh_count={0} {1} {2:N2}% > {3}%" -f $now.mesh_count, $field, $delta, $limit) -ForegroundColor Yellow
            $warnings++
        } else {
            Write-Host ("OK mesh_count={0} {1} {2:N2}%" -f $now.mesh_count, $field, $delta) -ForegroundColor Green
        }
    }
}
Write-Host "baseline_compare_ok warnings=$warnings errors=$errors"
if ($errors -gt 0) { exit 2 }
exit 0
