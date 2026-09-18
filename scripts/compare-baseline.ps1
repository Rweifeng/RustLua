[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Current,
    [Parameter(Mandatory = $true)][string]$Previous,
    # docs/54 §7 红线：p95 慢超过 5% 或 p99 慢超过 10% 即判红（默认不启用该改动）。
    [double]$P95LimitPercent = 5,
    [double]$P99LimitPercent = 10,
    # docs/54 §7 红线：内存峰值增加超过 10% 需额外评审。
    [double]$MemoryLimitPercent = 10,
    # docs/54 §8-⑩：只比**跨机可比**的指标（几何量 + 分配峰值），跳过 p95/p99 时间。
    # 进门（`verify.ps1 -Benchmarks`）用它对比提交进版本库的基线：时间是机器相关的量，跨机器比
    # 时间必然假红；同机 A/B 请用完整模式（把上一次的本地报告当 `-Previous`）。详见 baselines/README.md。
    [switch]$GeometryOnly
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
# 判定口径（docs/54 §7）：这里的超限**不是 warning**。统一改造若让固定场景的 p95/p99 或内存峰值
# 超出红线，默认就是不启用——因此脚本必须以非零退出把结果交给门，而不是打印一行黄字后放行。
$regressions = 0
$checks = 0
foreach ($now in $currentReport.stairs) {
    $old = $previousReport.stairs | Where-Object { $_.mesh_count -eq $now.mesh_count } | Select-Object -First 1
    if ($null -eq $old) { throw "Missing previous stair mesh_count=$($now.mesh_count)" }
    foreach ($metric in @("draw_calls", "instances", "triangles")) {
        $checks++
        if ($now.render.$metric -ne $old.render.$metric) {
            Write-Host "REGRESSION mesh_count=$($now.mesh_count) $metric $($old.render.$metric) -> $($now.render.$metric)" -ForegroundColor Red
            $regressions++
        }
    }
    foreach ($percentile in @(@("p95_us", $P95LimitPercent), @("p99_us", $P99LimitPercent))) {
        $field = $percentile[0]
        $limit = [double]$percentile[1]
        if ($GeometryOnly) {
            # 时间不进跨机判定，但**必须**打印出来：不然"跑过了"会被误读成"时间也被检查过了"。
            $delta = Percent-Delta $old.samples.$field $now.samples.$field
            Write-Host ("INFO mesh_count={0} {1} {2:N2}% (timestamp only; -GeometryOnly)" -f $now.mesh_count, $field, $delta) -ForegroundColor DarkGray
            continue
        }
        $checks++
        $delta = Percent-Delta $old.samples.$field $now.samples.$field
        if ($delta -gt $limit) {
            Write-Host ("REGRESSION mesh_count={0} {1} {2:N2}% > {3}%" -f $now.mesh_count, $field, $delta, $limit) -ForegroundColor Red
            $regressions++
        } else {
            Write-Host ("OK mesh_count={0} {1} {2:N2}%" -f $now.mesh_count, $field, $delta) -ForegroundColor Green
        }
    }
    # 内存峰值：两条报告都带 `cpu_peak_bytes` 时才比较（老报告缺字段不算回归，但要说明）。
    $checks++
    if ($null -ne $old.cpu_peak_bytes -and $null -ne $now.cpu_peak_bytes) {
        $delta = Percent-Delta ([double]$old.cpu_peak_bytes) ([double]$now.cpu_peak_bytes)
        if ($delta -gt $MemoryLimitPercent) {
            Write-Host ("REGRESSION mesh_count={0} cpu_peak_bytes {1:N2}% > {2}%" -f $now.mesh_count, $delta, $MemoryLimitPercent) -ForegroundColor Red
            $regressions++
        } else {
            Write-Host ("OK mesh_count={0} cpu_peak_bytes {1:N2}%" -f $now.mesh_count, $delta) -ForegroundColor Green
        }
    } else {
        Write-Host "SKIP mesh_count=$($now.mesh_count) cpu_peak_bytes (missing in one report)" -ForegroundColor DarkGray
    }
}
Write-Host ("baseline_compare_ok checks={0} regressions={1} mode={2}" -f $checks, $regressions, $(if ($GeometryOnly) { "geometry-only" } else { "full" }))
if ($regressions -gt 0) { exit 2 }
exit 0
