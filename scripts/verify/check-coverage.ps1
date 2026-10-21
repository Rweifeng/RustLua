# check-coverage.ps1 —— Rust 代码覆盖率棘轮门（P4-01）
#
# 回答"哪些 workspace 源码没被测到、这次改动是否让覆盖率掉了"，而不是追求好看的总数。
# 只做棘轮（不许掉）：总行覆盖掉 >0.5pp 判红；单 crate（源码行 >= 200）掉 >2.0pp 判红。
#
# 用法：
#   pwsh -NoProfile -File scripts/verify/check-coverage.ps1 -RequireTool
#   pwsh -NoProfile -File scripts/verify/check-coverage.ps1 -Current artifacts/metrics/coverage-summary.json
#   pwsh -NoProfile -File scripts/verify/check-coverage.ps1 -Update -Reason "新增 X 覆盖测试，行覆盖 +1.2pp"
#
# 退出码：0 = 通过；1 = 配置/输入错误或测试失败；2 = 有证据的覆盖率回退；
#         3 = BLOCKED（缺工具/缺基线），不等于通过。
#
# 口径（metrics/README.md §10）：只统计 workspace 源码；vendor/target/OUT_DIR 不计入；
# 内联测试代码计入行覆盖，报告写"含测试代码"。不允许为了绿去删测试或写无意义断言。

[CmdletBinding()]
param(
    [switch]$RequireTool,
    # 跳过采集，直接用给定的 coverage-v1 JSON 做对比（自测用）。
    [string]$Current = "",
    # 更新基线：必须带 -Reason，且工作树必须干净（git_dirty=false）。
    [switch]$Update,
    [string]$Reason = "",
    # 基线文件路径（默认 metrics/baselines/coverage.json；自测时传临时文件）。
    [string]$Baseline = ""
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$engineRoot = Join-Path $projectRoot "engine"
$metricsRoot = Join-Path $projectRoot "metrics"
if ([string]::IsNullOrWhiteSpace($Baseline)) {
    $Baseline = Join-Path $metricsRoot "baselines\coverage.json"
}
$baselinePath = $Baseline
$artifactsRoot = Join-Path $projectRoot "artifacts\metrics"
New-Item -ItemType Directory -Force -Path $artifactsRoot | Out-Null

$semver = '^\d+\.\d+\.\d+$'
$toolsPath = Join-Path $projectRoot "scripts\release\tools.json"
$pin = ""
if (Test-Path -LiteralPath $toolsPath) {
    $tools = Get-Content -LiteralPath $toolsPath -Raw | ConvertFrom-Json
    $pin = [string]$tools.cargoLlvmCov
}

function Test-ToolReady {
    $raw = & cargo llvm-cov --version 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $raw) { return $false }
    if ($pin -and $raw -notmatch [regex]::Escape($pin)) {
        Write-Host "[WARN] cargo-llvm-cov 版本与 tools.json ($pin) 不一致：$raw" -ForegroundColor Yellow
    }
    $components = & rustup component list --installed 2>$null
    foreach ($line in $components) {
        if ($line -match '^\s*llvm-tools[\s-]') { return $true }
    }
    return $false
}

function Get-GitState {
    $commit = (& git rev-parse --short HEAD 2>$null)
    $dirty = (& git status --porcelain 2>$null | Measure-Object).Count -gt 0
    return @{ commit = [string]$commit; dirty = $dirty }
}

function Invoke-CoverageCollect {
    Write-Host "==> collecting workspace coverage (llvm-cov, --no-fail-fast)"
    # cargo-llvm-cov 总会向 stderr 打 `info: ... cfg(coverage)`；脚本全局是 Stop，
    # PowerShell 5.1 会把这类原生 stderr 当成终止错误。命令成功与否只认 $LASTEXITCODE。
    $ErrorActionPreference = "Continue"
    # 2026-10-21 CI：默认并行 test 线程下 `engine --lib` 偶发 0xC0000005
    # （STATUS_ACCESS_VIOLATION）；同一测试二进制单线程可稳定跑完。覆盖率采集不需要
    # 并行，固定单线程换取可复现性（本地复现：204 passed / 0 failed）。
    $env:RUST_TEST_THREADS = "1"
    $covJson = Join-Path $artifactsRoot "coverage.json"
    $lcov = Join-Path $artifactsRoot "lcov.info"
    $htmlDir = Join-Path $artifactsRoot "html"
    # workspace 根在 engine/（项目根没有 Cargo.toml）；必须在 engine/ 目录跑 cargo，
    # 否则 cargo llvm-cov --workspace 会因找不到 manifest 直接失败。
    Push-Location $engineRoot
    try {
        & cargo llvm-cov --workspace --all-features --locked --no-fail-fast `
            --ignore-filename-regex 'engine[\\/]vendor[\\/]|engine[\\/]target[\\/]' `
            --json --summary-only --output-path $covJson
        if ($LASTEXITCODE -ne 0) {
            # --no-fail-fast 下测试失败会让 cargo 返回非零；覆盖率数字不能掩盖红测试。
            Write-Host "[FAIL] llvm-cov 采集失败或存在测试失败（exit $LASTEXITCODE）；覆盖率门不掩盖红测试" -ForegroundColor Red
            return $null
        }
        # lcov + HTML 作为 artifact 证据（尽力而为，失败不判红但打印原因）。
        & cargo llvm-cov --workspace --all-features --locked --no-fail-fast `
            --ignore-filename-regex 'engine[\\/]vendor[\\/]|engine[\\/]target[\\/]' `
            --lcov --output-path $lcov 2>$null | Out-Null
        if ($LASTEXITCODE -ne 0) { Write-Host "[WARN] lcov 生成失败（artifact 缺失，不影响判据）" -ForegroundColor Yellow }
        & cargo llvm-cov --workspace --all-features --locked --no-fail-fast `
            --ignore-filename-regex 'engine[\\/]vendor[\\/]|engine[\\/]target[\\/]' `
            --html --output-dir $htmlDir 2>$null | Out-Null
        if ($LASTEXITCODE -ne 0) { Write-Host "[WARN] HTML 生成失败（artifact 缺失，不影响判据）" -ForegroundColor Yellow }
    } finally {
        Pop-Location
    }
    return $covJson
}

# ── 1. 工具检查：缺工具退 3（BLOCKED），不是通过 ──────────────────────────────
if ($RequireTool -and -not (Test-ToolReady)) {
    Write-Host "coverage_blocked reason=missing_tool（缺 cargo-llvm-cov / llvm-tools-preview；先跑 scripts/release/install-metrics-tools.ps1）" -ForegroundColor Yellow
    exit 3
}

# ── 2. 采集或读取当前报告 ────────────────────────────────────────────────────
$git = Get-GitState
$currentJson = ""
if ($Current) {
    $currentJson = $Current
    if (-not (Test-Path -LiteralPath $currentJson)) {
        Write-Host "[FAIL] -Current 文件不存在: $currentJson" -ForegroundColor Red
        exit 1
    }
} else {
    if (-not (Test-ToolReady)) {
        Write-Host "coverage_blocked reason=missing_tool（缺 cargo-llvm-cov / llvm-tools-preview）" -ForegroundColor Yellow
        exit 3
    }
    $currentJson = Invoke-CoverageCollect
    if (-not $currentJson) { exit 1 }
    # 归一化
    $summaryOut = Join-Path $artifactsRoot "coverage-summary.json"
    $summaryMd = Join-Path $artifactsRoot "coverage-summary.md"
    & node (Join-Path $projectRoot "scripts\metrics\coverage-summary.mjs") `
        --input $currentJson --out $summaryOut --markdown $summaryMd `
        --env-fingerprint "local" --git-commit $git.commit --git-dirty $git.dirty
    if ($LASTEXITCODE -ne 0) { exit 1 }
    $currentJson = $summaryOut
}

# ── 3. 读取并校验当前报告 ────────────────────────────────────────────────────
$cur = $null
try { $cur = Get-Content -LiteralPath $currentJson -Raw -Encoding UTF8 | ConvertFrom-Json } catch {
    Write-Host "[FAIL] 当前覆盖率报告无法解析: $currentJson（$($_.Exception.Message)）" -ForegroundColor Red
    exit 1
}
if ($cur.schema -ne "coverage-v1" -or $null -eq $cur.overall -or $null -eq $cur.overall.lines) {
    Write-Host "[FAIL] 当前覆盖率报告 schema 不是 coverage-v1" -ForegroundColor Red
    exit 1
}
if ($null -eq $cur.crates -or -not $cur.crates) {
    Write-Host "[FAIL] 覆盖率报告缺 per-crate 表（crates 为空）" -ForegroundColor Red
    exit 1
}

$curLines = [double]$cur.overall.lines.percent

# ── 4. -Update：首份基线或显式更新（必须先于“缺基线退 3”）──────────────────
if ($Update) {
    if ([string]::IsNullOrWhiteSpace($Reason)) {
        Write-Host "[FAIL] -Update 必须带 -Reason（提交信息要写清为什么这次增长/变化是合理的）" -ForegroundColor Red
        exit 1
    }
    if ($git.dirty) {
        Write-Host "[FAIL] -Update 要求工作树干净（git_dirty=true 的 run 不能生成/更新基线）；先提交或 stash 在途改动" -ForegroundColor Red
        exit 1
    }
    $cur | Add-Member -NotePropertyName "baseline_reason" -NotePropertyValue $Reason
    $cur | Add-Member -NotePropertyName "baseline_created_at" -NotePropertyValue (Get-Date -Format "yyyy-MM-ddTHH:mm:ssZ")
    $cur | Add-Member -NotePropertyName "baseline_commit" -NotePropertyValue $git.commit
    $cur | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $baselinePath -Encoding UTF8
    Write-Host ("coverage_baseline_updated lines=" + $curLines.ToString('0.0') + "% commit=" + $git.commit)
    exit 0
}

# ── 5. 基线：不存在退 3 ───────────────────────────────────────────────────────
if (-not (Test-Path -LiteralPath $baselinePath)) {
    Write-Host "coverage_blocked reason=no_baseline（metrics/baselines/coverage.json 不存在；首轮先出报告，人工审阅后 -Update 一次）" -ForegroundColor Yellow
    exit 3
}
$base = Get-Content -LiteralPath $baselinePath -Raw -Encoding UTF8 | ConvertFrom-Json
if ($base.schema -ne "coverage-v1" -or $null -eq $base.overall.lines) {
    Write-Host "[FAIL] 基线文件 schema 不是 coverage-v1: $baselinePath" -ForegroundColor Red
    exit 1
}

# ── 6. 棘轮判定 ───────────────────────────────────────────────────────────────
$failures = @()
$baseLines = [double]$base.overall.lines.percent
$delta = $curLines - $baseLines
if ($delta -lt -0.5) {
    $failures += "总行覆盖掉了 $([math]::Abs($delta).ToString('0.0'))pp（阈值 0.5pp）"
}
$baseCrates = @{}
foreach ($c in $base.crates) { $baseCrates[$c.name] = [double]$c.lines.percent }
foreach ($c in $cur.crates) {
    if ([int]$c.lines.count -lt 200) { continue } # 小 crate 不做单 crate 判红
    if ($baseCrates.ContainsKey($c.name)) {
        $cd = [double]$c.lines.percent - $baseCrates[$c.name]
        if ($cd -lt -2.0) {
            $failures += "crate $($c.name) 行覆盖掉了 $([math]::Abs($cd).ToString('0.0'))pp（阈值 2.0pp，源码行 $($c.lines.count)）"
        }
    }
}

# ── 7. 输出与退出 ─────────────────────────────────────────────────────────────
if ($failures.Count -gt 0) {
    foreach ($f in $failures) { Write-Host ("[RED] " + $f) -ForegroundColor Red }
    Write-Host ("coverage_regression lines=" + $curLines.ToString('0.0') + "% baseline=" + $baseLines.ToString('0.0') + "% delta=" + $delta.ToString('0.0') + "pp crates=" + $cur.crates.Count) -ForegroundColor Red
    exit 2
}
Write-Host ("coverage_ok lines=" + $curLines.ToString('0.0') + "% functions=" + $cur.overall.functions.percent.ToString('0.0') + "% regions=" + $cur.overall.regions.percent.ToString('0.0') + "% baseline=" + $baseLines.ToString('0.0') + "% delta=" + $delta.ToString('0.0') + "pp crates=" + $cur.crates.Count) -ForegroundColor Green
exit 0
