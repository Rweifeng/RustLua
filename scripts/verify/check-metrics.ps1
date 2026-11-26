# check-metrics.ps1 —— 度量总检查（P4-08 / verify.ps1 -Metrics 分支）
#
# 对趋势库做回归检查：回归判 2；同指纹样本不足退 3（BLOCKED）。
# 首次跑在干净树上时先**从提交进仓库的 metrics/history 重建派生库**，不再直接 BLOCKED
# （旧行为：artifacts/metrics/trend.sqlite 不存在就 exit 3，而 verify.ps1 -Metrics 从不 ingest，
#  于是这个门在干净树上结构性 BLOCKED —— 2026-11-26 修复）。
# 可选对已有产物（binaries/packages/coverage-summary）采集入库（-Collect）。
#
# 红线自证：每次先跑 `trend_db.py selftest`（正反 9 例）。键位错配曾让"覆盖率 -0.5pp / 包体 +3%"
# 从未触发，自测失败按 **exit 1** 处理（不是 BLOCKED、不是通过）——看不见的红线等于没有红线。
#
# 用法：
#   pwsh -NoProfile -File scripts/verify/check-metrics.ps1
#   pwsh -NoProfile -File scripts/verify/check-metrics.ps1 -Collect
#
# 退出码：0 = 通过；1 = 错误/自测失败；2 = 回归；3 = 缺历史/样本不足（BLOCKED）。

[CmdletBinding()]
param(
    [Alias("Collect")]
    [switch]$DoCollect,
    [string]$PolicyFile = "",
    [string]$DbPath = ""
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $PolicyFile) { $PolicyFile = Join-Path $projectRoot "metrics\policy.json" }
if (-not $DbPath) { $DbPath = Join-Path $projectRoot "artifacts\metrics\trend.sqlite" }
$trend = Join-Path $projectRoot "scripts\metrics\trend_db.py"
$history = Join-Path $projectRoot "metrics\history"
$py = "python"

# ── 0. 红线自证（正反 9 例）──────────────────────────────────────────────
Write-Host "==> trend_db self-test（红线必须真的会红）"
& $py $trend selftest
if ($LASTEXITCODE -ne 0) {
    Write-Host "[FAIL] trend_db 自测失败：趋势红线不可信，先修 trend_db（不要把本步当通过）" -ForegroundColor Red
    exit 1
}

# ── 1. 派生库不存在则从提交进仓库的 history 重建 ─────────────────────────
if (-not (Test-Path -LiteralPath $DbPath)) {
    if (Test-Path -LiteralPath $history) {
        Write-Host "==> 趋势库不存在，先从 metrics/history 重建：$DbPath"
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $DbPath) | Out-Null
        & $py $trend rebuild --history $history --db $DbPath
        if ($LASTEXITCODE -ne 0) {
            Write-Host "[FAIL] trend_db rebuild 失败（exit $LASTEXITCODE）" -ForegroundColor Red
            exit 1
        }
    } else {
        Write-Host "[BLOCKED] 趋势库不存在且 metrics/history 缺失：$DbPath" -ForegroundColor Yellow
        exit 3
    }
}
$runCount = & $py $trend query --sql "select count(*) as n from runs" --db $DbPath 2>$null
if ($LASTEXITCODE -ne 0 -or -not ($runCount | Select-String '"n": [1-9]')) {
    Write-Host "[BLOCKED] 趋势库无 run（runs=0）—— 缺历史退 3；先跑 trend_db.py ingest 至少 1 个 run" -ForegroundColor Yellow
    exit 3
}

# ── 2. 可选采集已有产物（尺寸产物 + 覆盖率汇总）──────────────────────────
if ($DoCollect) {
    $measure = Join-Path $projectRoot "scripts\metrics\measure-artifacts.mjs"
    $inputs = @(
        @{ f = "binaries.json"; args = @("--kind", "binary") },
        @{ f = "packages.json"; args = @("--kind", "package", "--project", "examples/game_api") }
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
    # 覆盖率汇总由 coverage job 落 artifacts/metrics/coverage-summary.json；存在就入库。
    $coverageSummary = Join-Path $projectRoot "artifacts\metrics\coverage-summary.json"
    if (Test-Path -LiteralPath $coverageSummary) {
        Write-Host "  collecting coverage-summary.json ..."
        & $py $trend ingest --input $coverageSummary --db $DbPath 2>$null
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
