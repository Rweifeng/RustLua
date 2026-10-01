# wasm-bindgen CLI 的定位与按需安装（被 package.ps1 / verify-wasm-lua.ps1 共用）。
#
# 为什么必须管版本：CLI 与 wasm 模块里的 wasm-bindgen 元数据 schema **必须精确一致**。
# 本机 PATH 上的 CLI 版本与 crate 版本错位是最常见的失败模式：
#   - CLI 偏旧：报 "rust Wasm file schema version: 0.2.129 / this binary schema version: 0.2.127"
#     并直接拒绝生成胶水；
#   - 同族但不匹配 catch 处理：报 "externref table required for catch wrappers"。
# 原实现只判断"PATH 上有没有 wasm-bindgen"，版本不对也照用，于是一台机器上的旧 CLI
# 会把 webgl 与微信两条发布路径一起弄挂（实测 2026-09-30：PATH 0.2.127 vs crate 0.2.129）。
#
# 版本的**唯一事实源**是 engine/Cargo.lock 里 `wasm-bindgen` 那条 —— 升级 crate 时
# 不用回来改这里的魔数。
#
# 解析顺序（先命中且**版本精确匹配**才返回）：
#   1. $env:WASM_BINDGEN                              —— 显式覆盖
#   2. engine\.tools\wasm-bindgen\bin\...             —— 本脚本装进仓外缓存的那份
#   3. engine\vendor\wasm-bindgen-cli 已构建出的那份  —— 版本对得上就用（离线机器友好）
#   4. PATH 上的 wasm-bindgen
#   5. 全不匹配且允许安装时：cargo install 精确版本到仓外缓存
#      `--root engine\.tools\wasm-bindgen`，**不碰全局 `~\.cargo\bin`**：既不顶掉别人
#      装的全局工具，也不留下"只有这台机器能打包"的隐式依赖。
#
# 命令行用法：
#   powershell -File scripts\wasm-bindgen.ps1              # 解析（必要时安装）并打印
#   powershell -File scripts\wasm-bindgen.ps1 -PrintPath   # 只打印 exe 路径
#   powershell -File scripts\wasm-bindgen.ps1 -NoInstall   # 只解析，绝不安装（环境探测）
[CmdletBinding()]
param(
    [switch]$NoInstall,
    [switch]$PrintPath
)

$workspaceRoot = Split-Path -Parent $PSScriptRoot
$engineRoot = Join-Path $workspaceRoot "engine"
$cargoLock = Join-Path $engineRoot "Cargo.lock"
$toolsRoot = Join-Path $engineRoot ".tools\wasm-bindgen"
$toolsExe = Join-Path $toolsRoot "bin\wasm-bindgen.exe"
$vendorCliExe = Join-Path $engineRoot "vendor\wasm-bindgen-cli\cli\target\release\wasm-bindgen.exe"

function Get-WasmBindgenCrateVersion {
    if (-not (Test-Path -LiteralPath $cargoLock -PathType Leaf)) { return $null }
    $lines = @(Get-Content -LiteralPath $cargoLock)
    for ($i = 0; $i -lt $lines.Count; $i += 1) {
        if ($lines[$i] -match '^name\s*=\s*"wasm-bindgen"\s*$') {
            # Cargo.lock 的格式：name 行之后紧跟 version 行。
            if ($i + 1 -lt $lines.Count -and $lines[$i + 1] -match '^version\s*=\s*"(.+?)"\s*$') {
                return $Matches[1]
            }
        }
    }
    return $null
}

function Get-WasmBindgenCliVersion([string]$Exe) {
    if ([string]::IsNullOrWhiteSpace($Exe)) { return $null }
    if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) { return $null }
    # 原生工具的正常输出也可能落 stderr；PS 5.1 在 EAP=Stop 下会把它变成终止性错误，
    # 所以逐次调用都临时降级（同 package.ps1 调 cargo 的既有模式）。
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $out = & $Exe --version 2>$null
        if ($LASTEXITCODE -ne 0 -or -not $out) { return $null }
        $text = ($out | Select-Object -First 1).ToString()
        if ($text -match '(\d+\.\d+\.\d+)') { return $Matches[1] }
        return $null
    }
    catch { return $null }
    finally { $ErrorActionPreference = $savedEap }
}

function Test-WasmBindgenCli([string]$Exe, [string]$WantVersion) {
    if ([string]::IsNullOrWhiteSpace($Exe)) { return $false }
    return (Get-WasmBindgenCliVersion $Exe) -eq $WantVersion
}

function Install-WasmBindgenCli([string]$WantVersion) {
    New-Item -ItemType Directory -Force -Path $toolsRoot | Out-Null
    Write-Host "==> Installing wasm-bindgen-cli $WantVersion into engine\.tools (cargo install --root)" -ForegroundColor Cyan
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $exit = 1
    try {
        Push-Location $engineRoot
        try {
            cargo install wasm-bindgen-cli --version $WantVersion --root $toolsRoot --locked 2>&1 |
                ForEach-Object { Write-Host $_ }
            $exit = $LASTEXITCODE
        }
        finally { Pop-Location }
    }
    finally { $ErrorActionPreference = $savedEap }
    if ($exit -ne 0) { throw "wasm-bindgen-cli $WantVersion install failed (exit $exit)." }
    if (-not (Test-WasmBindgenCli $toolsExe $WantVersion)) {
        throw "wasm-bindgen-cli installed but version check failed: $toolsExe"
    }
    Write-Host "wasm-bindgen ready: $toolsExe ($WantVersion)" -ForegroundColor Green
    return $toolsExe
}

function Resolve-WasmBindgenCli {
    param([switch]$Provision)

    $want = Get-WasmBindgenCrateVersion
    if (-not $want) {
        throw "cannot read wasm-bindgen version from $cargoLock（crate 版本是 CLI 版本的事实源）"
    }

    $candidates = @()
    if ($env:WASM_BINDGEN) { $candidates += $env:WASM_BINDGEN }
    $candidates += $toolsExe
    $candidates += $vendorCliExe
    $onPath = Get-Command wasm-bindgen -ErrorAction SilentlyContinue
    if ($onPath) { $candidates += $onPath.Source }

    foreach ($cand in $candidates) {
        if (Test-WasmBindgenCli $cand $want) { return $cand }
    }

    # 把"为什么全被否掉"说出来：版本错位是唯一常见原因，沉默会让人以为只是没装。
    $seen = @()
    foreach ($cand in $candidates) {
        $v = Get-WasmBindgenCliVersion $cand
        if ($v) { $seen += ("{0} = {1}" -f $cand, $v) }
    }
    $seenText = 'none'
    if ($seen.Count -gt 0) { $seenText = $seen -join ', ' }
    if (-not $Provision) {
        if ($seen.Count) {
            Write-Host "==> no wasm-bindgen matches crate version $want (saw: $seenText)" -ForegroundColor Yellow
        }
        return $null
    }
    Write-Host "==> no wasm-bindgen matches crate version $want (saw: $seenText); installing the pinned version" -ForegroundColor Yellow
    return Install-WasmBindgenCli $want
}

# 只有作为脚本直接运行时才走 CLI；被 `. scripts\wasm-bindgen.ps1` 点源时只导出函数，
# 不改调用者的输出与退出码。
if ($MyInvocation.InvocationName -ne '.' -and $MyInvocation.InvocationName -ne 'source') {
    $ErrorActionPreference = "Stop"
    $resolved = Resolve-WasmBindgenCli -Provision:(-not $NoInstall)
    if (-not $resolved) {
        if ($PrintPath) { exit 2 }
        throw "wasm-bindgen CLI 解析失败（版本必须与 Cargo.lock 里的 wasm-bindgen 一致）。自动安装：powershell -File scripts\wasm-bindgen.ps1"
    }
    if ($PrintPath) { Write-Output $resolved }
    else { Write-Host ("wasm-bindgen = {0} ({1})" -f $resolved, (Get-WasmBindgenCliVersion $resolved)) -ForegroundColor Green }
    exit 0
}
