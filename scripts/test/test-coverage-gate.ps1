# test-coverage-gate.ps1 —— 覆盖率棘轮门自测（P4-01，5 个用例）
#
# 用合成 coverage-v1 JSON 驱动 check-coverage.ps1（-Current/-Baseline 路径），
# 覆盖 5 条判据：
#   1) 持平通过（exit 0）
#   2) 总行覆盖掉 0.6pp 判红（exit 2）
#   3) 单 crate（源码行 >= 200）掉 2.1pp 判红（exit 2）
#   4) 缺基线退 3（BLOCKED，不等于通过）
#   5) 坏 schema 退 1
#
# 用法：pwsh -NoProfile -File scripts/test/test-coverage-gate.ps1
# 输出：coverage_gate_selftest_ok cases=5（全过）或逐条 [FAIL] + 非零退出。

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$gate = Join-Path $projectRoot "scripts\verify\check-coverage.ps1"
$tmp = Join-Path $projectRoot "artifacts\tmp\test-coverage-gate"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

function New-CoverageJson([string]$Path, [double]$LinesPct, [string]$Schema = "coverage-v1", [array]$Crates = @()) {
    if (-not $Crates) {
        $Crates = @(@{ name = "sample"; lines = @{ count = 100; covered = 50; percent = 50.0 }; functions = @{ count = 10; covered = 5; percent = 50.0 }; regions = @{ count = 10; covered = 5; percent = 50.0 }; source_lines = 100 })
    }
    $obj = @{
        schema = $Schema
        generated_at = "2026-10-20T00:00:00Z"
        overall = @{
            lines = @{ count = 10000; covered = [int]([math]::Round($LinesPct / 100 * 10000)); percent = $LinesPct }
            functions = @{ count = 1000; covered = 500; percent = 50.0 }
            regions = @{ count = 1000; covered = 500; percent = 50.0 }
        }
        crates = $Crates
    }
    $obj | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $Path -Encoding UTF8
}

function Invoke-GateCase([string]$Name, [int]$Expected, [string]$Current, [string]$Baseline, [string[]]$ExtraArgs = @()) {
    $args = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $gate, "-Current", $Current, "-Baseline", $Baseline) + $ExtraArgs
    & powershell @args 2>&1 | Out-Null
    $code = $LASTEXITCODE
    if ($code -eq $Expected) {
        Write-Host ("[PASS] {0} (exit {1})" -f $Name, $code) -ForegroundColor Green
        return 0
    }
    Write-Host ("[FAIL] {0} -- expected exit {1}, got {2}" -f $Name, $Expected, $code) -ForegroundColor Red
    return 1
}

$failed = 0
$base = Join-Path $tmp "baseline.json"
$cur = Join-Path $tmp "current.json"

# 1) 持平通过
New-CoverageJson $base 50.0
New-CoverageJson $cur 50.0
$failed += Invoke-GateCase "持平通过 (exit 0)" 0 $cur $base

# 2) 总行覆盖掉 0.6pp
New-CoverageJson $base 50.0
New-CoverageJson $cur 49.4
$failed += Invoke-GateCase "总覆盖掉 0.6pp 判红 (exit 2)" 2 $cur $base

# 3) 单 crate 掉 2.1pp（源码行 500 >= 200；overall 持平避免总覆盖判红干扰）
$bigBase = @(@{ name = "bigcrate"; lines = @{ count = 500; covered = 400; percent = 80.0 }; functions = @{ count = 50; covered = 40; percent = 80.0 }; regions = @{ count = 50; covered = 40; percent = 80.0 }; source_lines = 500 })
$bigCur = @(@{ name = "bigcrate"; lines = @{ count = 500; covered = 390; percent = 77.9 }; functions = @{ count = 50; covered = 40; percent = 80.0 }; regions = @{ count = 50; covered = 40; percent = 80.0 }; source_lines = 500 })
New-CoverageJson $base 50.0 "coverage-v1" $bigBase
New-CoverageJson $cur 50.0 "coverage-v1" $bigCur
$failed += Invoke-GateCase "单 crate 掉 2.1pp 判红 (exit 2)" 2 $cur $base

# 4) 缺基线退 3
New-CoverageJson $cur 50.0
$missingBase = Join-Path $tmp "no-such-baseline.json"
$failed += Invoke-GateCase "缺基线退 3 (exit 3)" 3 $cur $missingBase

# 5) 坏 schema 退 1
New-CoverageJson $base 50.0
New-CoverageJson $cur 50.0 "wrong-schema"
$failed += Invoke-GateCase "坏 schema 退 1 (exit 1)" 1 $cur $base

Write-Host ""
if ($failed -gt 0) {
    Write-Host ("coverage_gate_selftest_failed cases=5 failures=" + $failed) -ForegroundColor Red
    exit 1
}
Write-Host "coverage_gate_selftest_ok cases=5" -ForegroundColor Green
exit 0
