# check-toolchain-lock.ps1 —— 工具链与安全工具版本锁定门
#
# 守护对象：`scripts/release/tools.json`（单一事实源）与
#   - `engine/rust-toolchain.toml` 的 channel
#   - `.github/workflows/*.yml` 的 `dtolnay/rust-toolchain@<版本>` 与 `actions/setup-node`
#
# 为什么需要：`channel = "stable"` + CI 的 `@stable` 意味着"同一提交在两个月后可能用不同
# rustc 跑门"。本仓的 fmt/clippy/rustdoc/性能基线都要求可复现，浮动工具链会把"版本变了"
# 伪装成"代码回归"。安全工具的版本同理：cargo-audit / cargo-deny 的判据随版本变化。
#
# 用法：
#   pwsh -NoProfile -File scripts/verify/check-toolchain-lock.ps1
#
# 退出码：0 = 一致；1 = 漂移（逐条 [FAIL]）。
# 已接入 verify.ps1 的便宜段（-Quick 也跑），不需要网络。

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$workflowRoot = Join-Path $projectRoot ".github\workflows"
$toolsPath = Join-Path $projectRoot "scripts\release\tools.json"
$toolchainPath = Join-Path $projectRoot "engine\rust-toolchain.toml"

$failed = @()
$passed = 0

function Add-Check([string]$Name, [bool]$Ok, [string]$FailReason) {
    if ($Ok) {
        $script:passed++
        Write-Host ("[PASS] " + $Name)
    } else {
        $script:failed += ("[FAIL] " + $Name + " -- " + $FailReason)
        Write-Host ("[FAIL] " + $Name + " -- " + $FailReason) -ForegroundColor Red
    }
}

function Get-TomlString([string]$Path, [string]$Key) {
    if (-not (Test-Path -LiteralPath $Path)) { return "" }
    $m = Select-String -LiteralPath $Path -Pattern ('^\s*' + [regex]::Escape($Key) + '\s*=\s*"([^"]+)"') |
        Select-Object -First 1
    if ($m -and $m.Matches.Count -gt 0) { return $m.Matches[0].Groups[1].Value }
    return ""
}

if (-not (Test-Path -LiteralPath $toolsPath)) {
    Write-Host "[FAIL] tools.json missing: $toolsPath" -ForegroundColor Red
    exit 1
}
$tools = Get-Content -LiteralPath $toolsPath -Raw | ConvertFrom-Json
$rust = [string]$tools.rust
$node = [string]$tools.node

# ── A. tools.json 自身：精确 semver，不用 latest/lts/浮动值 ───────────────────
$semver = '^\d+\.\d+\.\d+$'
Add-Check "tools.rust 是精确版本 ($rust)" ($rust -match $semver) "expect X.Y.Z"
Add-Check "tools.cargoAudit 是精确版本 ($($tools.cargoAudit))" ([string]$tools.cargoAudit -match $semver) "expect X.Y.Z"
Add-Check "tools.cargoDeny 是精确版本 ($($tools.cargoDeny))" ([string]$tools.cargoDeny -match $semver) "expect X.Y.Z"
Add-Check "tools.cargoCyclonedx 是精确版本 ($($tools.cargoCyclonedx))" ([string]$tools.cargoCyclonedx -match $semver) "expect X.Y.Z"
Add-Check "tools.node 是主版本锁定 ($node)" ([string]$node -match '^\d+$') "expect a major version like 20"

# ── B. rust-toolchain.toml：精确版本且与 tools.json 一致 ─────────────────────
$channel = Get-TomlString $toolchainPath "channel"
Add-Check "rust-toolchain.toml channel 是精确版本 ($channel)" ($channel -match $semver) "must not be stable/beta/nightly"
Add-Check "rust-toolchain channel == tools.rust" ($channel -eq $rust) "toolchain=$channel tools=$rust"

# ── C. workflows：所有 rust 工具链引用必须同版本 ─────────────────────────────
if (-not (Test-Path -LiteralPath $workflowRoot)) {
    Add-Check "workflows 目录存在" $false "missing $workflowRoot"
} else {
    $workflowFiles = @(Get-ChildItem -LiteralPath $workflowRoot -Filter *.yml -File -ErrorAction SilentlyContinue)
    Add-Check "workflows 目录非空" ($workflowFiles.Count -gt 0) "no *.yml under $workflowRoot"
    $rustRefs = 0
    $nodeRefs = 0
    foreach ($wf in $workflowFiles) {
        $text = Get-Content -LiteralPath $wf.FullName -Raw
        $lineNo = 0
        foreach ($line in ($text -split "`n")) {
            $lineNo++
            if ($line -match 'uses:\s*dtolnay/rust-toolchain@([^\s#]+)') {
                $ref = $Matches[1].Trim()
                $rustRefs++
                Add-Check ("{0}:{1} rust toolchain ref = {2}" -f $wf.Name, $lineNo, $ref) `
                    ($ref -eq $rust) "expect $rust (not stable/master); pin the exact toolchain"
            }
            if ($line -match 'node-version:\s*"?([^"\s]+)"?') {
                $nodeRefs++
                Add-Check ("{0}:{1} setup-node = {2}" -f $wf.Name, $lineNo, $Matches[1]) `
                    ($Matches[1] -eq $node) "expect $node (tools.json)"
            }
        }
    }
    Add-Check "至少有一条 rust 工具链引用" ($rustRefs -gt 0) "no dtolnay/rust-toolchain reference found"
    Add-Check "至少有一条 setup-node 引用" ($nodeRefs -gt 0) "no actions/setup-node node-version found"
}

# ── D. workflows 里的安全工具版本：`cargo-audit@0.22.2` 这类 ref 必须等于 tools.json ──
# 只认 `tool@x.y.z` 形式（taiki-e/install-action 与 `cargo install --version` 都覆盖）；
# 允许同一版本在 quality.yml 与 release.yml 各出现一次。
$securityTools = [ordered]@{
    "cargo-audit"      = [string]$tools.cargoAudit
    "cargo-deny"       = [string]$tools.cargoDeny
    "cargo-cyclonedx"  = [string]$tools.cargoCyclonedx
}
$toolRefCounts = @{}
foreach ($name in $securityTools.Keys) { $toolRefCounts[$name] = 0 }
if (Test-Path -LiteralPath $workflowRoot) {
    $securityWorkflowFiles = @(Get-ChildItem -LiteralPath $workflowRoot -Filter *.yml -File -ErrorAction SilentlyContinue)
    foreach ($wf in $securityWorkflowFiles) {
        $text = Get-Content -LiteralPath $wf.FullName -Raw
        $lineNo = 0
        foreach ($line in ($text -split "`n")) {
            $lineNo++
            foreach ($name in $securityTools.Keys) {
                $expected = $securityTools[$name]
                $pattern = [regex]::Escape($name) + '@([0-9]+\.[0-9]+\.[0-9]+)'
                if ($line -match $pattern) {
                    $actual = $Matches[1]
                    $toolRefCounts[$name]++
                    Add-Check ("{0}:{1} {2}@{3}" -f $wf.Name, $lineNo, $name, $actual) `
                        ($actual -eq $expected) "expect $name@$expected (tools.json)"
                }
            }
        }
    }
    foreach ($name in $securityTools.Keys) {
        Add-Check ("workflow 至少固定一次 " + $name) ($toolRefCounts[$name] -gt 0) `
            "no $name@<exact semver> reference found; CI must not use latest"
    }
}

# ── E. 本机 rustc（若可用）只作提示，不判红：发行机可能没有 rustup ───────────
$localRust = ""
try {
    $raw = (& rustc --version 2>$null)
    if ($LASTEXITCODE -eq 0 -and $raw) { $localRust = (($raw -split '\s+')[1]) }
} catch { $localRust = "" }
if ($localRust -and $localRust -ne $rust) {
    Write-Host ("[WARN] 本机 rustc=$localRust 与锁定 $rust 不同；rustup 会按 rust-toolchain.toml 切换，构建前可跑 rustup show") -ForegroundColor Yellow
}

Write-Host ""
Write-Host ("toolchain lock: " + $passed + " passed, " + $failed.Count + " failed")
if ($failed.Count -gt 0) {
    Write-Host "toolchain_lock_failed" -ForegroundColor Red
    exit 1
}
Write-Host ("toolchain_lock_ok rust=" + $rust + " node=" + $node + " tools=3") -ForegroundColor Green
exit 0
