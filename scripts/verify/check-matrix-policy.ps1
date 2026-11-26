# check-matrix-policy.ps1 —— 平台矩阵档位与到期策略（P4-03）
#
# cell 档位与到期日在 metrics/promotion.json（被跟踪）；matrix.yml 是执行面，
# 本脚本是裁定面：blocking 必须绿；advisory 有观察期，**观察期一过就判 2**——
# 不论该 cell 当轮是绿还是红。"绿着但一直不升档"同样是不允许的挂账，
# 否则到期机制只会对"红"生效（历史漏洞：-Summarize 只在 r != success 时看 review_by）。
#
# 用法：
#   pwsh -NoProfile -File scripts/verify/check-matrix-policy.ps1 -List
#   pwsh -NoProfile -File scripts/verify/check-matrix-policy.ps1 -Check      # 到期裁定（CI/定时）
#   pwsh -NoProfile -File scripts/verify/check-matrix-policy.ps1 -Summarize '<json>'
#   pwsh -NoProfile -File scripts/verify/check-matrix-policy.ps1 -SelfTest   # 正反自测（CI 也跑）
#   可选 `-PromotionFile <path>`：测试/演练用，默认 metrics/promotion.json。
#
# 退出码：0 = 通过；1 = 配置错误 / 自测失败；2 = 有到期仍 advisory 的 cell，或 blocking 失败。
# -Summarize 的 JSON 是 GitHub Actions needs.matrix.result（{cell_id: result}）。

[CmdletBinding()]
param(
    [switch]$List,
    [switch]$Check,
    [string]$Summarize = "",
    [string]$SummarizeFile = "",
    [string]$PromotionFile = "",
    [switch]$SelfTest
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $PromotionFile) { $PromotionFile = Join-Path $projectRoot "metrics\promotion.json" }
if (-not (Test-Path -LiteralPath $PromotionFile)) {
    Write-Host "[FAIL] promotion.json 缺失: $PromotionFile" -ForegroundColor Red
    exit 1
}

function Read-Cells([string]$path) {
    return (([IO.File]::ReadAllText($path)) | ConvertFrom-Json).cells
}

function Test-Expired($cell, [datetime]$now) {
    $raw = $cell.review_by
    if ([string]::IsNullOrWhiteSpace($raw)) { return $false }
    try { $d = [datetime]::Parse($raw) } catch { return $false }
    return ($d -lt $now)
}

$cells = Read-Cells $PromotionFile
$blocking = @($cells | Where-Object { $_.tier -eq "blocking" })
$advisory = @($cells | Where-Object { $_.tier -eq "advisory" })
$now = Get-Date

# ---------------------------------------------------------------------------
# 自测：正反两向都要证"门真的会红"。CI 在 matrix-summary 里跑（秒级、零依赖）。
# ---------------------------------------------------------------------------
if ($SelfTest) {
    $pwshExe = if (Get-Command pwsh -ErrorAction SilentlyContinue) { "pwsh" } else { "powershell" }
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("matrix-policy-selftest-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null

    function Write-SelfTestPromotion([string]$path, [string]$reviewBy) {
        $obj = [ordered]@{
            schema = 1
            cells  = @(
                [ordered]@{ id = "blocking-a"; tier = "blocking"; owner = "P4"; reason = "self-test" },
                [ordered]@{ id = "adv-a"; tier = "advisory"; experimental = $true; owner = "P4"; reason = "self-test"; review_by = $reviewBy }
            )
        }
        [IO.File]::WriteAllText($path, ($obj | ConvertTo-Json -Depth 5))
    }
    function Write-SelfTestResults([string]$path, [string]$advisoryResult) {
        $obj = [ordered]@{ "blocking-a" = "success"; "adv-a" = $advisoryResult }
        [IO.File]::WriteAllText($path, ($obj | ConvertTo-Json -Compress))
    }
    function Invoke-SelfTestChild([string]$promo, [string]$results, [switch]$CheckOnly) {
        if ($CheckOnly) {
            & $pwshExe -NoProfile -ExecutionPolicy Bypass -File $PSCommandPath -Check -PromotionFile $promo | Out-Null
        } else {
            & $pwshExe -NoProfile -ExecutionPolicy Bypass -File $PSCommandPath -SummarizeFile $results -PromotionFile $promo | Out-Null
        }
        return $LASTEXITCODE
    }

    $future = (Get-Date).AddDays(90).ToString("yyyy-MM-dd")
    $past = (Get-Date).AddDays(-1).ToString("yyyy-MM-dd")
    $promoFuture = Join-Path $tmp "promotion-future.json"
    $promoPast = Join-Path $tmp "promotion-past.json"
    $resultsGreen = Join-Path $tmp "results-green.json"
    $resultsRed = Join-Path $tmp "results-red.json"
    Write-SelfTestPromotion $promoFuture $future
    Write-SelfTestPromotion $promoPast $past
    Write-SelfTestResults $resultsGreen "success"
    Write-SelfTestResults $resultsRed "failure"

    $failures = @()
    $cases = @(
        @{ name = "全绿 + 未到期 ⇒ 0"; want = 0; got = (Invoke-SelfTestChild $promoFuture $resultsGreen) },
        @{ name = "全绿 + 已到期 ⇒ 2（本批补的洞）"; want = 2; got = (Invoke-SelfTestChild $promoPast $resultsGreen) },
        @{ name = "advisory 红 + 未到期 ⇒ 0（观察期允许红）"; want = 0; got = (Invoke-SelfTestChild $promoFuture $resultsRed) },
        @{ name = "advisory 红 + 已到期 ⇒ 2"; want = 2; got = (Invoke-SelfTestChild $promoPast $resultsRed) },
        @{ name = "-Check + 已到期 ⇒ 2"; want = 2; got = (Invoke-SelfTestChild $promoPast "" -CheckOnly) },
        @{ name = "-Check + 未到期 ⇒ 0"; want = 0; got = (Invoke-SelfTestChild $promoFuture "" -CheckOnly) }
    )
    foreach ($case in $cases) {
        if ($case.got -ne $case.want) {
            $failures += ("{0}：期望 {1}，实际 {2}" -f $case.name, $case.want, $case.got)
        } else {
            Write-Host ("[PASS] self-test: " + $case.name) -ForegroundColor Green
        }
    }
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    if ($failures.Count -gt 0) {
        foreach ($f in $failures) { Write-Host ("[FAIL] " + $f) -ForegroundColor Red }
        exit 1
    }
    Write-Host "matrix_policy_selftest_ok cases=6" -ForegroundColor Green
    exit 0
}

# ---------------------------------------------------------------------------
# CI：needs.matrix.result 是所有 matrix 单元格的结果（continue-on-error 的 failure 也在）。
# 长 JSON 用 -SummarizeFile 传文件路径（Actions 表达式有 200KB 上限，一般 -Summarize 够用）。
# ---------------------------------------------------------------------------
if ($Summarize -or $SummarizeFile) {
    if ($SummarizeFile) { $Summarize = [IO.File]::ReadAllText($SummarizeFile) }
    $results = $Summarize | ConvertFrom-Json
    $failedBlocking = @()
    $advisoryRed = @()
    $advisoryExpired = @()
    $missing = @()
    foreach ($c in $cells) {
        $r = $results.($c.id)
        if ($null -eq $r) { $missing += $c.id; continue }
        if ($c.tier -eq "blocking" -and $r -ne "success") { $failedBlocking += $c.id }
        if ($c.tier -eq "advisory") {
            if ($r -ne "success") { $advisoryRed += $c.id }
            # 洞：过去这里嵌在 `r -ne success` 里面，绿着到期永远不红；现在对所有 advisory 判到期。
            if (Test-Expired $c $now) { $advisoryExpired += $c.id }
        }
    }
    foreach ($c in $missing) { Write-Host ("[FAIL] cell " + $c + " 无结果（matrix 未产出该 cell）") -ForegroundColor Red }
    foreach ($f in $failedBlocking) { Write-Host ("[RED] blocking cell " + $f + " 失败") -ForegroundColor Red }
    foreach ($f in $advisoryRed) { Write-Host ("[YELLOW] advisory cell " + $f + " 红（观察期内允许）") -ForegroundColor Yellow }
    foreach ($f in $advisoryExpired) {
        Write-Host ("[RED] advisory cell " + $f + " 观察期已过（review_by 到期）仍为 advisory：升档或明确裁定不支持") -ForegroundColor Red
    }
    $summary = ("matrix blocking={0} passed={1} advisory_red={2} advisory_expired={3}" -f `
            $blocking.Count, ($blocking.Count - $failedBlocking.Count), $advisoryRed.Count, $advisoryExpired.Count)
    if ($missing.Count -gt 0) {
        Write-Host ("matrix_failed " + $summary + " missing=" + ($missing -join ",")) -ForegroundColor Red
        exit 1
    }
    if ($failedBlocking.Count -gt 0 -or $advisoryExpired.Count -gt 0) {
        Write-Host ("matrix_failed " + $summary) -ForegroundColor Red
        exit 2
    }
    Write-Host ("matrix_ok " + $summary) -ForegroundColor Green
    exit 0
}

# ---------------------------------------------------------------------------
# 本地/定时：到期裁定（对全部 advisory 判到期，与 -Summarize 的口径一致）
# ---------------------------------------------------------------------------
$expired = @()
foreach ($c in $advisory) {
    if ([string]::IsNullOrWhiteSpace($c.review_by) -or [string]::IsNullOrWhiteSpace($c.owner) -or [string]::IsNullOrWhiteSpace($c.reason)) {
        $expired += "advisory cell $($c.id) 缺 owner/reason/review_by 三件套"
        continue
    }
    if (Test-Expired $c $now) {
        $expired += "advisory cell $($c.id) 观察期已过（review_by=$($c.review_by)）仍为 advisory：晋升或明确裁定不支持，不允许无限期黄着"
    }
}
foreach ($c in $blocking) {
    if ([string]::IsNullOrWhiteSpace($c.owner) -or [string]::IsNullOrWhiteSpace($c.reason)) {
        $expired += "blocking cell $($c.id) 缺 owner/reason"
    }
}

if ($Check -or (-not $List)) {
    foreach ($e in $expired) { Write-Host ("[RED] " + $e) -ForegroundColor Red }
    if ($expired.Count -gt 0) { exit 2 }
}
if ($List -or (-not $Check)) {
    Write-Host ("matrix_cells=" + $cells.Count + " blocking=" + $blocking.Count + " advisory=" + $advisory.Count) -ForegroundColor Green
}
exit 0
