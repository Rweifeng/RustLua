# check-matrix-policy.ps1 —— 平台矩阵档位与到期策略（P4-03）
#
# cell 档位与到期日在 metrics/promotion.json（被跟踪）；matrix.yml 是执行面，
# 本脚本是裁定面：blocking 必须绿；advisory 有观察期，到期仍 advisory/红 -> 判 2。
#
# 用法：
#   pwsh -NoProfile -File scripts/verify/check-matrix-policy.ps1 -List      # 概要
#   pwsh -NoProfile -File scripts/verify/check-matrix-policy.ps1 -Check     # 到期裁定（CI/定时）
#   pwsh -NoProfile -File scripts/verify/check-matrix-policy.ps1 -Summarize '<json>'  # matrix-summary job
#
# 退出码：0 = 通过；1 = 配置错误；2 = 有到期仍黄/红的 advisory cell，或 blocking 失败。
# -Summarize 的 JSON 是 GitHub Actions needs.matrix.result（{cell_id: result}）。

[CmdletBinding()]
param(
    [switch]$List,
    [switch]$Check,
    [string]$Summarize = "",
    [string]$SummarizeFile = ""
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$promoPath = Join-Path $projectRoot "metrics\promotion.json"
if (-not (Test-Path -LiteralPath $promoPath)) {
    Write-Host "[FAIL] promotion.json 缺失: $promoPath" -ForegroundColor Red
    exit 1
}
$cells = (([IO.File]::ReadAllText($promoPath)) | ConvertFrom-Json).cells

$blocking = @($cells | Where-Object { $_.tier -eq "blocking" })
$advisory = @($cells | Where-Object { $_.tier -eq "advisory" })
$now = Get-Date

if ($Summarize -or $SummarizeFile) {
    # CI：needs.matrix.result 是所有 matrix 单元格的结果（continue-on-error 的 failure 也在）。
    # 长 JSON 用 -SummarizeFile 传文件路径（Actions 表达式有 200KB 上限，一般直接 -Summarize 够用）。
    if ($SummarizeFile) {
        $Summarize = [IO.File]::ReadAllText($SummarizeFile)
    }
    $results = $Summarize | ConvertFrom-Json
    $failedBlocking = @()
    $advisoryRed = @()
    $advisoryRedExpiring = @()
    foreach ($c in $cells) {
        $r = $results.($c.id)
        if ($null -eq $r) {
            Write-Host "[FAIL] cell $($c.id) 无结果（matrix 未产出该 cell）" -ForegroundColor Red
            exit 1
        }
        if ($c.tier -eq "blocking" -and $r -ne "success") { $failedBlocking += $c.id }
        if ($c.tier -eq "advisory" -and $r -ne "success") {
            $advisoryRed += $c.id
            $d = $null
            try { $d = [datetime]::Parse($c.review_by) } catch {}
            if ($null -ne $d -and $d -lt $now) { $advisoryRedExpiring += $c.id }
        }
    }
    foreach ($f in $failedBlocking) { Write-Host ("[RED] blocking cell " + $f + " 失败") -ForegroundColor Red }
    foreach ($f in $advisoryRedExpiring) { Write-Host ("[RED] advisory cell " + $f + " 仍红且已到期（review_by=$($c.review_by)）") -ForegroundColor Red }
    if ($failedBlocking.Count -gt 0) {
        Write-Host ("matrix_failed blocking=" + $blocking.Count + " passed=" + ($blocking.Count - $failedBlocking.Count) + " advisory_red=" + $advisoryRed.Count + " advisory_red_expiring=" + $advisoryRedExpiring.Count) -ForegroundColor Red
        exit 2
    }
    if ($advisoryRedExpiring.Count -gt 0) {
        Write-Host ("matrix_failed blocking=" + $blocking.Count + " passed=" + $blocking.Count + " advisory_red=" + $advisoryRed.Count + " advisory_red_expiring=" + $advisoryRedExpiring.Count) -ForegroundColor Red
        exit 2
    }
    Write-Host ("matrix_ok blocking=" + $blocking.Count + " passed=" + $blocking.Count + " advisory_red=" + $advisoryRed.Count + " advisory_red_expiring=0") -ForegroundColor Green
    exit 0
}

# 本地/定时：到期裁定
$expired = @()
foreach ($c in $advisory) {
    if ([string]::IsNullOrWhiteSpace($c.review_by) -or [string]::IsNullOrWhiteSpace($c.owner) -or [string]::IsNullOrWhiteSpace($c.reason)) {
        $expired += "advisory cell $($c.id) 缺 owner/reason/review_by 三件套"
        continue
    }
    $d = $null
    try { $d = [datetime]::Parse($c.review_by) } catch {}
    if ($null -ne $d -and $d -lt $now) {
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
