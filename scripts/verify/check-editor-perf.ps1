# check-editor-perf.ps1 —— 编辑器性能基线的毫秒预算门（docs/55 阶段 0 / ER-01 补强）
#
# `editor --perf-baseline` 只判场景自带的不变量（result=ok / failed=0），不约束耗时；
# 本脚本把每个场景的 p95 与 metrics/editor-perf-budgets.json 的硬上限比较。三类情况判红：
#   1. 场景 p95 超预算；
#   2. 报告里出现没有预算的场景（新增场景必须同时补预算，否则门形同虚设）；
#   3. 预算里的场景在报告里消失（避免"删掉慢场景"绕过门）。
#
# 用法：
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify/check-editor-perf.ps1 `
#       -Report baselines/editor-baseline.txt -Budget metrics/editor-perf-budgets.json
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify/check-editor-perf.ps1 -SelfTest
#
# 退出码：0 = 通过；1 = 输入错误/预算文件坏；2 = 超预算或预算覆盖率不符；3 = 报告缺失（BLOCKED）。

[CmdletBinding()]
param(
    [string]$Report = "",
    [string]$Budget = "",
    [switch]$SelfTest
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $Budget) { $Budget = Join-Path $projectRoot "metrics\editor-perf-budgets.json" }

function Read-BudgetFile {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { throw "budget file missing: $Path" }
    $json = Get-Content -Raw -LiteralPath $Path -Encoding UTF8 | ConvertFrom-Json
    if ($null -eq $json -or $json.schema -ne "editor-perf-budgets-v1") {
        throw "budget schema mismatch (expected editor-perf-budgets-v1): $Path"
    }
    if ($null -eq $json.budgets) { throw "budget file has no budgets object: $Path" }
    $map = @{}
    foreach ($property in $json.budgets.PSObject.Properties) {
        if ($null -eq $property.Value.p95_ms) { throw "budget missing p95_ms: $($property.Name)" }
        $value = [double]$property.Value.p95_ms
        if ($value -le 0) { throw "budget must be positive: $($property.Name)" }
        $map[$property.Name] = $value
    }
    if ($map.Count -eq 0) { throw "budget file is empty: $Path" }
    $profiles = @()
    if ($null -ne $json.profiles) {
        $profiles = @($json.profiles | ForEach-Object { [string]$_ })
    }
    return [pscustomobject]@{ Budgets = $map; Profiles = $profiles }
}

function Invoke-EditorPerfGate {
    param(
        [Parameter(Mandatory = $true)][string]$ReportPath,
        [Parameter(Mandatory = $true)][hashtable]$Budgets,
        [string[]]$AllowedProfiles = @()
    )

    $messages = New-Object System.Collections.Generic.List[string]
    $violations = New-Object System.Collections.Generic.List[string]

    if (-not (Test-Path -LiteralPath $ReportPath)) {
        $messages.Add("editor_perf_budget_blocked report_missing=$ReportPath")
        return [pscustomobject]@{ Code = 3; Messages = $messages }
    }

    $scenarioPattern = '^(?<name>\S+)\s+n=(?<n>\d+)\s+last=\s*(?<last>[\d.]+)\s+p50=\s*(?<p50>[\d.]+)\s+p95=\s*(?<p95>[\d.]+)\s+p99=\s*(?<p99>[\d.]+)\s+max=\s*(?<max>[\d.]+)\s+ms\s+(?<status>ok|FAILED)\s*$'
    $invariant = [Globalization.CultureInfo]::InvariantCulture
    $profile = ""
    $summary = $null
    $scenarios = @{}
    $observed = New-Object System.Collections.Generic.List[string]

    foreach ($raw in Get-Content -LiteralPath $ReportPath -Encoding UTF8) {
        $line = $raw.Trim()
        if ($line.Length -eq 0) { continue }
        if ($line.StartsWith("profile=")) {
            $profile = ($line.Substring(8) -split '\s+')[0]
            continue
        }
        if ($line.StartsWith("scenarios=")) {
            if ($line -match '^scenarios=(?<count>\d+)\s+failed=(?<failed>\d+)\s+result=(?<result>\w+)$') {
                $summary = [pscustomobject]@{
                    Count  = [int]$Matches['count']
                    Failed = [int]$Matches['failed']
                    Result = $Matches['result']
                }
            }
            else {
                $violations.Add("summary_malformed line=$line")
            }
            continue
        }
        if ($line -match $scenarioPattern) {
            $name = $Matches['name']
            if ($scenarios.ContainsKey($name)) {
                $violations.Add("scenario_duplicate name=$name")
                continue
            }
            $scenarios[$name] = [pscustomobject]@{
                P95    = [double]::Parse($Matches['p95'], $invariant)
                Status = $Matches['status']
            }
            $observed.Add($name)
        }
    }

    if ([string]::IsNullOrEmpty($profile)) {
        $violations.Add("profile_missing")
    }
    elseif ($AllowedProfiles.Count -gt 0 -and -not ($AllowedProfiles -contains $profile)) {
        $violations.Add("profile_unknown profile=$profile allowed=$($AllowedProfiles -join ',')")
    }
    if ($null -eq $summary) {
        $violations.Add("summary_missing")
    }
    else {
        if ($summary.Result -ne "ok") { $violations.Add("report_result=$($summary.Result)") }
        if ($summary.Failed -ne 0) { $violations.Add("report_failed=$($summary.Failed)") }
        if ($scenarios.Count -ne $summary.Count) {
            $violations.Add("scenario_count_mismatch parsed=$($scenarios.Count) declared=$($summary.Count)")
        }
    }

    $worstName = ""
    $worstRatio = 0.0
    foreach ($name in $observed) {
        $entry = $scenarios[$name]
        if ($entry.Status -ne "ok") { $violations.Add("scenario_failed name=$name") }
        if (-not $Budgets.ContainsKey($name)) {
            $violations.Add(("missing_budget name={0} p95_ms={1:F3}" -f $name, $entry.P95))
            continue
        }
        $budget = [double]$Budgets[$name]
        $ratio = $entry.P95 / $budget
        if ($ratio -gt $worstRatio) {
            $worstRatio = $ratio
            $worstName = $name
        }
        if ($entry.P95 -gt $budget) {
            $violations.Add(("over_budget name={0} p95_ms={1:F3} budget_ms={2:F3}" -f $name, $entry.P95, $budget))
        }
    }
    foreach ($key in $Budgets.Keys) {
        if (-not $scenarios.ContainsKey($key)) {
            $violations.Add("unknown_budget_entry name=$key")
        }
    }

    if ($violations.Count -gt 0) {
        foreach ($violation in $violations) {
            $messages.Add("editor_perf_budget_fail $violation")
        }
        return [pscustomobject]@{ Code = 2; Messages = $messages }
    }

    $worstP95 = 0.0
    $worstBudget = 0.0
    if ($worstName.Length -gt 0) {
        $worstP95 = $scenarios[$worstName].P95
        $worstBudget = [double]$Budgets[$worstName]
    }
    $messages.Add(("editor_perf_budget_ok profile={0} scenarios={1} budgets={2} worst={3} p95_ms={4:F3} budget_ms={5:F3}" -f `
                $profile, $scenarios.Count, $Budgets.Count, $worstName, $worstP95, $worstBudget))
    return [pscustomobject]@{ Code = 0; Messages = $messages }
}

function New-ScenarioLine {
    param([string]$Name, [double]$P95, [string]$Status = "ok")
    return ("{0,-34} n={1,-5} last={2,9:F3} p50={3,9:F3} p95={4,9:F3} p99={5,9:F3} max={6,9:F3} ms  {7}" -f `
            $Name, 3, $P95, $P95, $P95, $P95, $P95, $Status)
}

function Write-SelfTestReport {
    param(
        [string]$Path,
        [string[]]$ScenarioLines,
        [string]$Summary,
        [string]$Profile = "debug"
    )
    $content = @(
        "=== RustLua editor performance baseline (docs/55 ER-01) ===",
        "profile=$Profile os=windows",
        "note: absolute values only compare on identical hardware and profile",
        ""
    ) + $ScenarioLines + @("", $Summary)
    Set-Content -LiteralPath $Path -Value $content -Encoding UTF8
}

function Invoke-EditorPerfSelfTest {
    $cases = 0
    $failures = New-Object System.Collections.Generic.List[string]
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("rustlua-editor-perf-gate-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    try {
        $policyPath = Join-Path $tmp "budget.json"
        $policyContent = @'
{
  "schema": "editor-perf-budgets-v1",
  "profiles": ["debug", "release"],
  "budgets": {
    "demo_a": { "p95_ms": 10 },
    "demo_b": { "p95_ms": 20 }
  }
}
'@
        Set-Content -LiteralPath $policyPath -Value $policyContent -Encoding UTF8
        $policy = Read-BudgetFile -Path $policyPath

        # 1. 合规报告必须通过
        $cases++
        $report = Join-Path $tmp "pass.txt"
        Write-SelfTestReport -Path $report -ScenarioLines @(
            (New-ScenarioLine -Name "demo_a" -P95 5.0),
            (New-ScenarioLine -Name "demo_b" -P95 10.0)
        ) -Summary "scenarios=2 failed=0 result=ok"
        $result = Invoke-EditorPerfGate -ReportPath $report -Budgets $policy.Budgets -AllowedProfiles $policy.Profiles
        if ($result.Code -ne 0) { $failures.Add("case1 expected 0 got $($result.Code)") }

        # 2. p95 超预算必须判红
        $cases++
        $report = Join-Path $tmp "over.txt"
        Write-SelfTestReport -Path $report -ScenarioLines @(
            (New-ScenarioLine -Name "demo_a" -P95 10.5),
            (New-ScenarioLine -Name "demo_b" -P95 10.0)
        ) -Summary "scenarios=2 failed=0 result=ok"
        $result = Invoke-EditorPerfGate -ReportPath $report -Budgets $policy.Budgets -AllowedProfiles $policy.Profiles
        if ($result.Code -ne 2 -or -not ($result.Messages -join "`n").Contains("over_budget")) {
            $failures.Add("case2 expected over_budget/2 got $($result.Code): $($result.Messages -join '; ')")
        }

        # 3. 新场景没有预算必须判红（否则门会被新场景绕过）
        $cases++
        $report = Join-Path $tmp "missing.txt"
        Write-SelfTestReport -Path $report -ScenarioLines @(
            (New-ScenarioLine -Name "demo_a" -P95 5.0),
            (New-ScenarioLine -Name "demo_b" -P95 10.0),
            (New-ScenarioLine -Name "demo_c" -P95 5.0)
        ) -Summary "scenarios=3 failed=0 result=ok"
        $result = Invoke-EditorPerfGate -ReportPath $report -Budgets $policy.Budgets -AllowedProfiles $policy.Profiles
        if ($result.Code -ne 2 -or -not ($result.Messages -join "`n").Contains("missing_budget")) {
            $failures.Add("case3 expected missing_budget/2 got $($result.Code): $($result.Messages -join '; ')")
        }

        # 4. 预算里的场景从报告消失必须判红（否则删掉慢场景就能过门）
        $cases++
        $report = Join-Path $tmp "stale.txt"
        Write-SelfTestReport -Path $report -ScenarioLines @(
            (New-ScenarioLine -Name "demo_a" -P95 5.0)
        ) -Summary "scenarios=1 failed=0 result=ok"
        $result = Invoke-EditorPerfGate -ReportPath $report -Budgets $policy.Budgets -AllowedProfiles $policy.Profiles
        if ($result.Code -ne 2 -or -not ($result.Messages -join "`n").Contains("unknown_budget_entry")) {
            $failures.Add("case4 expected unknown_budget_entry/2 got $($result.Code): $($result.Messages -join '; ')")
        }

        # 5. 报告自身 result=FAILED 必须判红
        $cases++
        $report = Join-Path $tmp "failed.txt"
        Write-SelfTestReport -Path $report -ScenarioLines @(
            (New-ScenarioLine -Name "demo_a" -P95 5.0),
            (New-ScenarioLine -Name "demo_b" -P95 10.0 -Status "FAILED")
        ) -Summary "scenarios=2 failed=1 result=FAILED"
        $result = Invoke-EditorPerfGate -ReportPath $report -Budgets $policy.Budgets -AllowedProfiles $policy.Profiles
        if ($result.Code -ne 2) {
            $failures.Add("case5 expected 2 got $($result.Code)")
        }

        # 6. 摘要场景数与实际行数不符必须判红（防止报告被截断后仍通过）
        $cases++
        $report = Join-Path $tmp "mismatch.txt"
        Write-SelfTestReport -Path $report -ScenarioLines @(
            (New-ScenarioLine -Name "demo_a" -P95 5.0),
            (New-ScenarioLine -Name "demo_b" -P95 10.0)
        ) -Summary "scenarios=3 failed=0 result=ok"
        $result = Invoke-EditorPerfGate -ReportPath $report -Budgets $policy.Budgets -AllowedProfiles $policy.Profiles
        if ($result.Code -ne 2 -or -not ($result.Messages -join "`n").Contains("scenario_count_mismatch")) {
            $failures.Add("case6 expected scenario_count_mismatch/2 got $($result.Code): $($result.Messages -join '; ')")
        }
    }
    finally {
        Remove-Item -Recurse -Force -LiteralPath $tmp -ErrorAction SilentlyContinue
    }

    if ($failures.Count -gt 0) {
        foreach ($failure in $failures) {
            Write-Host "editor_perf_budget_selftest_fail $failure"
        }
        return 1
    }
    Write-Host "editor_perf_budget_selftest_ok cases=$cases"
    return 0
}

if ($SelfTest) {
    exit (Invoke-EditorPerfSelfTest)
}

if ([string]::IsNullOrEmpty($Report)) {
    Write-Host "editor_perf_budget_error reason=report_argument_missing"
    exit 1
}

try {
    $policy = Read-BudgetFile -Path $Budget
    $result = Invoke-EditorPerfGate -ReportPath $Report -Budgets $policy.Budgets -AllowedProfiles $policy.Profiles
}
catch {
    Write-Host ("editor_perf_budget_error {0}" -f $_.Exception.Message)
    exit 1
}

foreach ($message in $result.Messages) { Write-Host $message }
exit $result.Code
