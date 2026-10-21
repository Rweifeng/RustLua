# test-cfg-coverage.ps1 —— cfg 覆盖门自测（P4-02，4 个反例）
#
# 用合成 sites/cells/tree 驱动 check-cfg-coverage.ps1，验证 4 条判据必须判红：
#   1) 删掉一个 wasm cell 里的包（game_web）→ 判红
#   2) 删掉 gpu-web-probe → 判红
#   3) 清单里加一条未覆盖的 wasm cfg（crate 不在闭包）→ 判红
#   4) 豁免过期（review_by 已过）→ 判 2
# 另有一条基座用例（合成数据全绿 → exit 0），确保自测本身不是恒红/恒绿。
#
# 用法：pwsh -NoProfile -File scripts/test/test-cfg-coverage.ps1
# 输出：cfg_coverage_selftest_ok cases=5（全过）或逐条 [FAIL] + 非零退出。

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$gate = Join-Path $projectRoot "scripts\verify\check-cfg-coverage.ps1"
$realSites = Join-Path $projectRoot "metrics\inventory\cfg-sites.json"
$tmp = Join-Path $projectRoot "artifacts\tmp\test-cfg-coverage"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

# 合成闭包（wasm 站点 crate 都在这里；game_web 靠 wasm-default cell 的 packages 覆盖）
$tree = Join-Path $tmp "closure.txt"
@("rustlua-infra", "rustlua-audio", "rustlua-render", "engine", "rustlua-scripting", "gpu-web-probe", "wasm-lua-spike", "rustlua-assets", "rustlua-math") |
    Set-Content -LiteralPath $tree -Encoding UTF8

# 合成 sites：真实清单里只保留 wasm / not-wasm 站点（覆盖断言只关心这两类）
$real = Get-Content -LiteralPath $realSites -Raw | ConvertFrom-Json
$filtered = @($real.sites | Where-Object { $_.kind -eq "wasm" -or $_.kind -eq "not-wasm" })
$sitesBase = Join-Path $tmp "sites-base.json"
@{ schema = 1; counts = $real.counts; sites = $filtered } | ConvertTo-Json -Depth 6 |
    Set-Content -LiteralPath $sitesBase -Encoding UTF8

# 注入一条未覆盖的 wasm 站点（crate=editor，不在任何 cell/闭包）
$sitesInjected = Join-Path $tmp "sites-injected.json"
$extra = @{
    key = 'editor|src/lib.rs|target_arch = "wasm32"|fn|1'
    crate = "editor"
    file = "src/lib.rs"
    predicate = 'target_arch = "wasm32"'
    item = "fn"
    occurrence = 1
    kind = "wasm"
    attr_or_macro = "attr"
}
$filteredInj = $filtered + @($extra)
@{ schema = 1; counts = $real.counts; sites = $filteredInj } | ConvertTo-Json -Depth 6 |
    Set-Content -LiteralPath $sitesInjected -Encoding UTF8

function Get-CellsJson([string]$Path, [scriptblock]$Mutate) {
    $cells = @{
        schema = 1
        cells = @(
            @{ id = "wasm-default"; cmd = "cargo check -p game_web -p gpu-web-probe -p wasm-lua-spike --target wasm32-unknown-unknown"; packages = @("game_web", "gpu-web-probe", "wasm-lua-spike"); role = "blocking" },
            @{ id = "wasm-gpu2d-off"; cmd = "cargo check -p game_web --no-default-features --target wasm32-unknown-unknown"; packages = @("game_web"); role = "blocking" },
            @{ id = "native-windows"; cmd = "cargo check --workspace --all-targets"; packages = @("*"); role = "blocking" }
        )
        exclusions = @(
            @{ crate = "assetc"; reason = "原生资源工具"; owner = "P4"; review_by = "2027-01-31" }
        )
    }
    & $Mutate $cells
    $cells | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $Path -Encoding UTF8
}

$cellsBase = Join-Path $tmp "cells-base.json"
Get-CellsJson $cellsBase { param($c) }

$cellsNoGameWeb = Join-Path $tmp "cells-no-game-web.json"
Get-CellsJson $cellsNoGameWeb {
    param($c)
    $c.cells[0].packages = @("gpu-web-probe", "wasm-lua-spike")
    $c.cells[1].packages = @()
}

$cellsNoProbe = Join-Path $tmp "cells-no-probe.json"
Get-CellsJson $cellsNoProbe {
    param($c)
    $c.cells[0].packages = @("game_web", "wasm-lua-spike")
}

$cellsExpired = Join-Path $tmp "cells-expired.json"
Get-CellsJson $cellsExpired {
    param($c)
    $c.exclusions = @(
        @{ crate = "assetc"; reason = "原生资源工具"; owner = "P4"; review_by = "2020-01-01" }
    )
}

$evidence = Join-Path $tmp "evidence.json"
@{ schema = 1; evidence = @(
    @{ target = "game_web"; kind = "runtime"; commands = @("verify-webgl-bundle.ps1") },
    @{ target = "rustlua-infra wasm VFS"; kind = "runtime"; commands = @("verify-webgl-bundle.ps1") }
) } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $evidence -Encoding UTF8

function Invoke-CfgCase([string]$Name, [int]$Expected, [string]$Sites, [string]$Cells) {
    $args = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $gate, `
        "-SitesFile", $Sites, "-CellsFile", $Cells, "-EvidenceFile", $evidence, "-TreeFile", $tree)
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
$failed += Invoke-CfgCase "基座全绿 (exit 0)" 0 $sitesBase $cellsBase
$failed += Invoke-CfgCase "删掉 game_web cell 判红 (exit 2)" 2 $sitesBase $cellsNoGameWeb
$failed += Invoke-CfgCase "删掉 gpu-web-probe 判红 (exit 2)" 2 $sitesBase $cellsNoProbe
$failed += Invoke-CfgCase "未覆盖 cfg 站点判红 (exit 2)" 2 $sitesInjected $cellsBase
$failed += Invoke-CfgCase "豁免过期判红 (exit 2)" 2 $sitesBase $cellsExpired

Write-Host ""
if ($failed -gt 0) {
    Write-Host ("cfg_coverage_selftest_failed cases=5 failures=" + $failed) -ForegroundColor Red
    exit 1
}
Write-Host "cfg_coverage_selftest_ok cases=5" -ForegroundColor Green
exit 0
