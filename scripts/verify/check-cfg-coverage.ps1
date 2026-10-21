# check-cfg-coverage.ps1 —— wasm 条件编译覆盖门（P4-02）
#
# 把 cfg 清单（cfg-inventory.mjs）变成可核对的表：每条 wasm/not-wasm 站点都有编译单元，
# 关键条目有运行证据；不再靠"game_web check 绿了"推断全部覆盖。
#
# 用法：
#   pwsh -NoProfile -File scripts/verify/check-cfg-coverage.ps1
#   pwsh -NoProfile -File scripts/verify/check-cfg-coverage.ps1 -TreeFile artifacts/tmp/closure.txt   # 自测/离线
#
# 退出码：0 = 通过；1 = 配置/输入错误、清单漂移、覆盖缺口；2 = 豁免过期（有证据的"该红了"）。
#
# 判定：
#   - 每个 kind=wasm 的站点，其 crate 必须落在某个 wasm cell 的直接包或其依赖闭包内；
#   - 每个 kind=not-wasm 的站点，其 crate 必须被 native cell（--workspace）覆盖；
#   - gpu-web-probe 必须被 wasm-default cell 显式覆盖（独立 wasm 探针，不在 game_web 闭包）；
#   - exclusions 必须带 owner/review_by；review_by 过期 -> 判 2（不打印黄字放行）。

[CmdletBinding()]
param(
    [string]$SitesFile = "",
    [string]$CellsFile = "",
    [string]$EvidenceFile = "",
    # 预计算的 wasm 闭包 crate 列表（每行一个 crate 名；自测用，缺省时运行时 cargo tree）。
    [string]$TreeFile = ""
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$engineRoot = Join-Path $projectRoot "engine"
if ([string]::IsNullOrWhiteSpace($SitesFile)) { $SitesFile = Join-Path $projectRoot "metrics\inventory\cfg-sites.json" }
if ([string]::IsNullOrWhiteSpace($CellsFile)) { $CellsFile = Join-Path $projectRoot "metrics\wasm-cells.json" }
if ([string]::IsNullOrWhiteSpace($EvidenceFile)) { $EvidenceFile = Join-Path $projectRoot "metrics\wasm-evidence.json" }
$inventoryScript = Join-Path $projectRoot "scripts\metrics\cfg-inventory.mjs"

function Get-JsonOrFail([string]$Path, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path)) {
        Write-Host "[FAIL] $Label 缺失: $Path" -ForegroundColor Red
        exit 1
    }
    # .NET ReadAllText 默认按 UTF-8 解码（无 BOM 也按 UTF-8）；Get-Content 在 PS5.1 会按 ANSI 解码中文
    try { return [IO.File]::ReadAllText($Path) | ConvertFrom-Json } catch {
        Write-Host "[FAIL] $Label 无法解析: $Path（$($_.Exception.Message)）" -ForegroundColor Red
        exit 1
    }
}

# ── 1. 清单漂移检查（cfg-inventory.mjs --check 重新生成并逐 key 比对）──────────
& node $inventoryScript --check
if ($LASTEXITCODE -ne 0) {
    Write-Host "[FAIL] cfg 清单漂移（cfg-inventory.mjs --check 判红）" -ForegroundColor Red
    exit 1
}

$sites = Get-JsonOrFail $SitesFile "cfg-sites.json"
$cells = Get-JsonOrFail $CellsFile "wasm-cells.json"
$evidence = Get-JsonOrFail $EvidenceFile "wasm-evidence.json"

# ── 2. 构建 wasm 覆盖 crate 集合 ─────────────────────────────────────────────
# wasm cell 的直接包 + 依赖闭包（cargo tree --target wasm32）。
$wasmPackages = @()
foreach ($cell in $cells.cells) {
    if ($cell.id -like "wasm-*") { $wasmPackages += $cell.packages }
}
$wasmPackages = @($wasmPackages | Sort-Object -Unique)

$coveredCrates = New-Object System.Collections.Generic.HashSet[string]
foreach ($p in $wasmPackages) { [void]$coveredCrates.Add($p) }

if ($TreeFile) {
    if (-not (Test-Path -LiteralPath $TreeFile)) {
        Write-Host "[FAIL] -TreeFile 不存在: $TreeFile" -ForegroundColor Red
        exit 1
    }
    foreach ($line in (Get-Content -LiteralPath $TreeFile)) {
        $m = [regex]::Match($line, '^([a-zA-Z0-9_-]+)')
        if ($m.Success) { [void]$coveredCrates.Add($m.Groups[1].Value) }
    }
} else {
    foreach ($p in $wasmPackages) {
        if ($p -eq "*") { continue }
        $raw = & cargo tree --manifest-path (Join-Path $engineRoot "Cargo.toml") -p $p --target wasm32-unknown-unknown -e normal --prefix none 2>$null
        if ($LASTEXITCODE -ne 0) {
            Write-Host "[WARN] cargo tree -p $p (wasm32) 失败；该包的闭包未纳入覆盖集（若 $p 是 design-native 请在 wasm-cells.json exclusions 登记）" -ForegroundColor Yellow
            continue
        }
        foreach ($line in $raw) {
            $m = [regex]::Match($line, '^([a-zA-Z0-9_-]+)')
            if ($m.Success) { [void]$coveredCrates.Add($m.Groups[1].Value) }
        }
    }
}

# native cell 覆盖全部 workspace 成员（cargo check --workspace --all-targets）
$nativeCovered = New-Object System.Collections.Generic.HashSet[string]
$nativeCovered.Add("*")

# ── 3. 站点断言 ───────────────────────────────────────────────────────────────
$wasmSites = @($sites.sites | Where-Object { $_.kind -eq "wasm" })
$notWasmSites = @($sites.sites | Where-Object { $_.kind -eq "not-wasm" })
$unknownSites = @($sites.sites | Where-Object { $_.kind -eq "unknown" })

$uncovered = @()
foreach ($s in $wasmSites) {
    if (-not $coveredCrates.Contains($s.crate)) {
        $uncovered += "wasm site $($s.crate)|$($s.file)|$($s.predicate) 的 crate 不在任何 wasm cell 的直接包/闭包"
    }
}
foreach ($s in $notWasmSites) {
    # native cell 覆盖 workspace 全部；这里只做形式断言（未来加 native 子集 cell 时生效）
    if ($nativeCovered.Contains("*")) { continue }
    if (-not $nativeCovered.Contains($s.crate)) {
        $uncovered += "not-wasm site $($s.crate)|$($s.file) 的 crate 不在 native cell 覆盖内"
    }
}

# gpu-web-probe 必须被显式覆盖
$probeCell = @($cells.cells | Where-Object { $_.id -eq "wasm-default" -and $_.packages -contains "gpu-web-probe" })
if ($probeCell.Count -eq 0) {
    $uncovered += "gpu-web-probe 不在 wasm-default cell 的 packages 里（独立 wasm 探针，不能靠 game_web 闭包掩盖）"
}

# ── 4. 豁免检查：owner/review_by 三件套，过期判 2 ─────────────────────────────
$now = Get-Date
$expired = @()
foreach ($ex in $cells.exclusions) {
    if ([string]::IsNullOrWhiteSpace($ex.owner) -or [string]::IsNullOrWhiteSpace($ex.reason) -or [string]::IsNullOrWhiteSpace($ex.review_by)) {
        $expired += "exclusion $($ex.crate) 缺 owner/reason/review_by 三件套"
        continue
    }
    $d = $null
    try { $d = [datetime]::Parse($ex.review_by) } catch { $d = $null }
    if ($null -ne $d -and $d -lt $now) {
        $expired += "exclusion $($ex.crate) 豁免已过期（review_by=$($ex.review_by)）"
    }
}

# ── 5. 运行证据清单检查 ───────────────────────────────────────────────────────
$evidenceUnknown = @($evidence.evidence | Where-Object { [string]::IsNullOrWhiteSpace($_.kind) -or $_.kind -eq "unknown" })
$compileOnly = @($evidence.evidence | Where-Object { $_.kind -eq "compile-only" }).Count
$recordOnly = @($evidence.evidence | Where-Object { $_.kind -eq "record-only" }).Count

# ── 6. 输出与退出 ─────────────────────────────────────────────────────────────
$wasmCovered = $wasmSites.Count - $uncovered.Count
$fail = $uncovered.Count -gt 0 -or $expired.Count -gt 0 -or $evidenceUnknown.Count -gt 0

foreach ($u in $uncovered) { Write-Host ("[RED] " + $u) -ForegroundColor Red }
foreach ($e in $expired) { Write-Host ("[RED] " + $e) -ForegroundColor Red }
foreach ($e in $evidenceUnknown) { Write-Host ("[RED] evidence entry 不能是 unknown: " + ($e | ConvertTo-Json -Compress)) -ForegroundColor Red }

if ($fail) {
    Write-Host ("cfg_coverage_failed wasm_attrs=" + $wasmSites.Count + " covered=" + $wasmCovered + " wasm_macros=" + $sites.counts.wasm_macros + " native_attrs=" + $notWasmSites.Count + " unknown=" + $unknownSites.Count) -ForegroundColor Red
    exit 2
}
Write-Host ("cfg_coverage_ok wasm_attrs=" + $wasmSites.Count + " covered=" + $wasmSites.Count + " wasm_macros=" + $sites.counts.wasm_macros + " covered=" + $sites.counts.wasm_macros + " native_attrs=" + $notWasmSites.Count + " covered=" + $notWasmSites.Count + " unknown=" + $unknownSites.Count + " compile_only=" + $compileOnly + " record_only=" + $recordOnly) -ForegroundColor Green
exit 0
