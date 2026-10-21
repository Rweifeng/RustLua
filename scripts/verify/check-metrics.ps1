# check-metrics.ps1 —— 度量总检查（P4-08 / verify.ps1 -Metrics 分支）
#
# 对趋势库做回归检查：缺历史退 3（BLOCKED）；回归判 2；样本不足退 3。
# 可选对已有产物（coverage-summary/binaries/packages）采集入库（-Collect），
# 让"跑过但没进库"的数据可以被归因。
#
# 用法：
#   pwsh -NoProfile -File scripts/verify/check-metrics.ps1
#   pwsh -NoProfile -File scripts/verify/check-metrics.ps1 -Collect
#
# 退出码：0 = 通过；1 = 错误；2 = 回归；3 = 缺历史/样本不足（BLOCKED）。

[CmdletBinding()]
param(
    [switch]$DoCollect,
    [string]$PolicyFile = "",
    [string]$DbPath = ""
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $PolicyFile) { $PolicyFile = Join-Path $projectRoot "metrics\policy.json" }
if (-not $DbPath) { $DbPath = Join-Path $projectRoot "artifacts\metrics\trend.sqlite" }
$trend = Join-Path $projectRoot "scripts\metrics\trend_db.py"
$py = "python"

# ── 1. 缺历史退 3 ─────────────────────────────────────────────────────────
if (-not (Test-Path -LiteralPath $DbPath)) {
    Write-Host "[BLOCKED] 趋势库不存在: $DbPath —— 先跑 trend_db.py ingest 至少 1 个 run（首次没有基线是预期的，产出报告人工审阅后 --update）" -ForegroundColor Yellow
    exit 3
}
$runCount = & $py $trend query --sql "select count(*) as n from runs" --db $DbPath 2>$null
if ($LASTEXITCODE -ne 0 -or -not ($runCount | Select-String '"n": [1-9]')) {
    Write-Host "[BLOCKED] 趋势库无 run（runs=0）—— 缺历史退 3" -ForegroundColor Yellow
    exit 3
}

# ── 2. 可选采集已有产物 ───────────────────────────────────────────────────
if ($DoCollect) {
    $measure = Join-Path $projectRoot "scripts\metrics\measure-artifacts.mjs"
    $inputs = @(
        @{ f = "binaries.json"; args = @("--kind", "binary") },
        @{ f = "packages.json"; args = @("--kind", "package", "--project", "examples/game_jhyt") }
    )
    foreach ($in in $inputs) {
        $p = Join-Path $projectRoot "artifacts\metrics\$($in.f)"
        if (-not (Test-Path -LiteralPath $p)) { continue }
        Write-Host ("  collecting $($in.f) ...")
        & node $measure @($in.args + @("--out", $p)) 2>$null
        if ($LASTEXITCODE -eq 0) {
            & $py $trend ingest --input $p --db $DbPath 2>$null
        }
    }
}

# ── 3. 回归检查 ────────────────────────────────────────────────────────────
& $py $trend check --policy $PolicyFile --db $DbPath
$code = $LASTEXITCODE
if ($code -eq 0) {
    Write-Host "metrics_ok regressions=0 insufficient=0 db=$DbPath" -ForegroundColor Green
    exit 0
}
if ($code -eq 2) {
    Write-Host "metrics_regression（trend_db check 判 2）—— 见上方输出，按 allowlist/基线流程处理" -ForegroundColor Red
    exit 2
}
if ($code -eq 3) {
    Write-Host "[BLOCKED] metrics 样本不足（trend_db check 退 3）—— 同指纹样本 < min_samples，继续采集" -ForegroundColor Yellow
    exit 3
}
Write-Host "[FAIL] trend_db check 异常退出 $code" -ForegroundColor Red
exit 1
