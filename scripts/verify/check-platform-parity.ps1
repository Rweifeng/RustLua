# check-platform-parity.ps1 —— CI 的 wasm cell 与本地质量门的命令一致性。
#
# 为什么有它（2026-11-02 实测）：HEAD `7d951c84` 上 CI 的 **4 个 wasm cell 全红**，根因是
# `rustlua-gameplay/src/net_lua.rs:685` 的 `#[cfg(target_arch = "wasm32")] let stats = None;`
# 推不出类型（E0282）；本地门与 CI 各写一条 wasm 命令，谁先被改都可能让另一边变成
# "看起来绿"的摆设。这份脚本把"两边命令集漂移"从"等下一次 push"变成当场红灯。
#
# 判据（忽略空白与 `--locked`——本地为了可复现多带它，CI 不带）：
#   1. `matrix.yml` 里每个 `target: wasm32-unknown-unknown` 的 cell 的 `cmd`，必须在本
#      `verify.ps1` 里逐一出现；
#   2. 反过来，`verify.ps1` 里每条 wasm32 的 `cargo check` 也必须在 matrix.yml 里有对应 cell
#      （避免本地偷偷加一条 CI 不跑的检查，让"本地绿"超出 CI 覆盖面）。
#
# 用法：
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify/check-platform-parity.ps1
#
# 退出码：0 = 一致；1 = 漂移（打印两侧差集）；3 = 文件缺失/BLOCKED（不是通过）。

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$matrixPath = Join-Path $projectRoot ".github\workflows\matrix.yml"
$verifyPath = Join-Path $projectRoot "scripts\verify\verify.ps1"

foreach ($path in @($matrixPath, $verifyPath)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        Write-Host "[BLOCKED] 缺文件: $path" -ForegroundColor Yellow
        exit 3
    }
}

# 归一化：去掉多余空白与 `--locked`，其余逐字比较（包集与 target 必须一致）。
function Get-Normalized([string]$CommandLine) {
    $tokens = @($CommandLine -split '\s+' | Where-Object { $_ -ne "" -and $_ -ne "--locked" })
    return ($tokens -join " ")
}

# --- 1) matrix.yml 的 wasm cell -------------------------------------------------
$cells = @()
$current = $null
foreach ($line in [IO.File]::ReadAllLines($matrixPath)) {
    if ($line -match '^\s*-\s+id:\s*(\S+)\s*$') {
        if ($null -ne $current) { $cells += $current }
        $current = [ordered]@{ id = $Matches[1]; cmd = ""; target = "" }
        continue
    }
    if ($null -eq $current) { continue }
    if ($line -match '^\s+cmd:\s*(.+?)\s*$' -and $current.cmd -eq "") {
        $current.cmd = $Matches[1].Trim().Trim('"').Trim("'")
        continue
    }
    if ($line -match '^\s+target:\s*(.*?)\s*$' -and $current.target -eq "") {
        $current.target = $Matches[1].Trim().Trim('"').Trim("'")
        continue
    }
}
if ($null -ne $current) { $cells += $current }

$wasmCells = @($cells | Where-Object { $_.target -eq "wasm32-unknown-unknown" -and $_.cmd -ne "" })
if ($wasmCells.Count -eq 0) {
    Write-Host "[FAIL] matrix.yml 里找不到任何 wasm32-unknown-unknown cell（平台门等于没有覆盖）" -ForegroundColor Red
    exit 1
}

# --- 2) verify.ps1 里真正跑的 wasm 命令 -----------------------------------------
$verifyText = [IO.File]::ReadAllText($verifyPath)
$localCommands = @()
foreach ($match in [regex]::Matches($verifyText, 'Invoke-Checked\s+cargo\s+@\((?<args>[^)]*)\)')) {
    $tokens = @([regex]::Matches($match.Groups['args'].Value, '"([^"]*)"') | ForEach-Object { $_.Groups[1].Value })
    if ($tokens -contains "wasm32-unknown-unknown") {
        $localCommands += (Get-Normalized ("cargo " + ($tokens -join " ")))
    }
}
$localCommands = @($localCommands | Where-Object { $_ -ne "" } | Select-Object -Unique)
if ($localCommands.Count -eq 0) {
    Write-Host "[FAIL] verify.ps1 里没有任何 wasm32 的 cargo check（平台编译又变回 opt-in）" -ForegroundColor Red
    exit 1
}

# --- 3) 双向比差集 ---------------------------------------------------------------
$matrixCommands = @($wasmCells | ForEach-Object { Get-Normalized $_.cmd } | Select-Object -Unique)
$onlyInCi = @($matrixCommands | Where-Object { $localCommands -notcontains $_ })
$onlyLocal = @($localCommands | Where-Object { $matrixCommands -notcontains $_ })

if ($onlyInCi.Count -gt 0 -or $onlyLocal.Count -gt 0) {
    foreach ($c in $onlyInCi) { Write-Host "[RED] 只有 CI 跑、本地门没跑：$c" -ForegroundColor Red }
    foreach ($c in $onlyLocal) { Write-Host "[RED] 只有本地门跑、CI 没跑：$c" -ForegroundColor Red }
    Write-Host ("platform_parity_failed ci_commands=" + $matrixCommands.Count + " local_commands=" + $localCommands.Count + " only_in_ci=" + $onlyInCi.Count + " only_local=" + $onlyLocal.Count) -ForegroundColor Red
    exit 1
}

Write-Host ("platform_parity_ok wasm_cells=" + $wasmCells.Count + " commands=" + $matrixCommands.Count + " (" + ($matrixCommands -join " | ") + ")") -ForegroundColor Green
exit 0
