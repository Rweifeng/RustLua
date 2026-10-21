# install-metrics-tools.ps1 —— 按 tools.json 安装固定版本的度量工具（P4-00）
#
# 守护对象：cargo-llvm-cov（覆盖率，Windows MSVC 下唯一可靠方案）与
#           llvm-tools-preview（LLVM 覆盖率工具链）。
# 版本唯一事实源是 scripts/release/tools.json 的 cargoLlvmCov。
#
# 用法：
#   pwsh -NoProfile -File scripts/release/install-metrics-tools.ps1 -Check   # 只读校验
#   pwsh -NoProfile -File scripts/release/install-metrics-tools.ps1          # 安装到 CARGO_HOME
#
# 为什么用 `cargo install --locked --version <pin>`：门禁工具的判据随版本变化，
# 今天绿的结论明天可能因工具升级变红/变绿；版本必须精确到 semver，不允许 latest。
#
# 退出码：0 = 工具齐且版本一致；1 = 缺失/版本不符/安装失败。
# 语义映射：门的调用方把"缺工具"翻译成退出码 3（BLOCKED，见 metrics/README.md §4）；
# 本脚本只报告工具状态，不替门做"跳过即通过"的决定。

[CmdletBinding()]
param(
    [switch]$Check,
    # 可选：cargo 镜像配置（crates.io CDN 不可达时用）。只透传给 cargo install。
    [string]$CargoConfig = ""
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$toolsPath = Join-Path $projectRoot "scripts\release\tools.json"
if (-not (Test-Path -LiteralPath $toolsPath)) {
    Write-Host "[FAIL] tools.json missing: $toolsPath" -ForegroundColor Red
    exit 1
}
$tools = Get-Content -LiteralPath $toolsPath -Raw | ConvertFrom-Json
$pin = [string]$tools.cargoLlvmCov
if ([string]::IsNullOrWhiteSpace($pin) -or $pin -notmatch '^\d+\.\d+\.\d+$') {
    Write-Host "[FAIL] tools.json cargoLlvmCov must be exact semver, got: '$pin'" -ForegroundColor Red
    exit 1
}
$pythonPin = [string]$tools.python

# ── 读实际版本 ────────────────────────────────────────────────────────────────
function Get-CargoLlvmCovVersion {
    $raw = & cargo llvm-cov --version 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $raw) { return "" }
    $m = [regex]::Match(("$raw" -join " "), '(\d+\.\d+\.\d+)')
    if ($m.Success) { return $m.Groups[1].Value }
    return ""
}

function Test-LlvmToolsPreview {
    $raw = & rustup component list --installed 2>$null
    if ($LASTEXITCODE -ne 0) { return $false }
    foreach ($line in $raw) {
        # rustup 显示为 llvm-tools-<target>（如 llvm-tools-x86_64-pc-windows-msvc）；
        # 前缀匹配 llvm-tools 即可，该组件在 rustup 里的正式名就是 llvm-tools-preview。
        if ($line -match '^\s*llvm-tools[\s-]') { return $true }
    }
    return $false
}

$covVersion = Get-CargoLlvmCovVersion
$llvmToolsOk = Test-LlvmToolsPreview

# ── 本机 python：>=3.11 即可（CI 用 actions/setup-python@v5 固定 tools.json 版本）──
$localPython = ""
try {
    $raw = & python --version 2>$null
    if ($LASTEXITCODE -eq 0 -and $raw) {
        $m = [regex]::Match(("$raw" -join " "), '(\d+)\.(\d+)\.(\d+)')
        if ($m.Success) { $localPython = $m.Groups[1].Value + "." + $m.Groups[2].Value + "." + $m.Groups[3].Value }
    }
} catch { $localPython = "" }

Write-Host ("pinned cargoLlvmCov = " + $pin)
Write-Host ("local  cargo-llvm-cov = " + $(if ($covVersion) { $covVersion } else { "(not found)" }))
Write-Host ("local  llvm-tools-preview = " + $(if ($llvmToolsOk) { "installed" } else { "NOT installed" }))
Write-Host ("pinned python (CI) = " + $pythonPin + " | local python = " + $(if ($localPython) { $localPython } else { "(not found)" }))

$ok = $true
if (-not $covVersion) {
    Write-Host "[FAIL] cargo-llvm-cov not found (run without -Check to install)" -ForegroundColor Red
    $ok = $false
} elseif ($covVersion -ne $pin) {
    Write-Host "[FAIL] cargo-llvm-cov $covVersion != pinned $pin" -ForegroundColor Red
    $ok = $false
}
if (-not $llvmToolsOk) {
    Write-Host "[FAIL] llvm-tools-preview not installed (run without -Check to install)" -ForegroundColor Red
    $ok = $false
}
if ($localPython -and $localPython -notmatch '^3\.(1[1-9]|[2-9][0-9])\.') {
    Write-Host "[FAIL] local python $localPython < 3.11 (trend_db.py requires 3.11+)" -ForegroundColor Red
    $ok = $false
}
if (-not $localPython) {
    Write-Host "[WARN] python not found on PATH; trend_db.py requires python 3.11+" -ForegroundColor Yellow
}

# ── -Check：到此为止，不安装 ─────────────────────────────────────────────────
if ($Check) {
    if ($ok) {
        Write-Host "metrics_tools_ok cargoLlvmCov=$pin llvm_tools=1 python_local=$localPython python_pin=$pythonPin" -ForegroundColor Green
        exit 0
    }
    Write-Host "metrics_tools_failed" -ForegroundColor Red
    exit 1
}

# ── 安装 ──────────────────────────────────────────────────────────────────────
if (-not $covVersion -or $covVersion -ne $pin) {
    Write-Host ("==> installing cargo-llvm-cov " + $pin) -ForegroundColor Cyan
    $installArgs = @("install", "--locked", "cargo-llvm-cov", "--version", $pin)
    if (-not [string]::IsNullOrWhiteSpace($CargoConfig)) {
        $installArgs += @("--config", (Resolve-Path -LiteralPath $CargoConfig).Path)
    }
    & cargo @installArgs
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[FAIL] cargo install cargo-llvm-cov failed" -ForegroundColor Red
        exit 1
    }
}
if (-not $llvmToolsOk) {
    Write-Host "==> adding llvm-tools-preview via rustup" -ForegroundColor Cyan
    & rustup component add llvm-tools-preview
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[FAIL] rustup component add llvm-tools-preview failed" -ForegroundColor Red
        exit 1
    }
}

# ── 安装后复核 ───────────────────────────────────────────────────────────────
$covVersion2 = Get-CargoLlvmCovVersion
$llvmToolsOk2 = Test-LlvmToolsPreview
if ($covVersion2 -eq $pin -and $llvmToolsOk2) {
    Write-Host "metrics_tools_ok cargoLlvmCov=$pin llvm_tools=1" -ForegroundColor Green
    exit 0
}
Write-Host "[FAIL] post-install verification failed: cov=$covVersion2 llvm_tools=$llvmToolsOk2" -ForegroundColor Red
exit 1
