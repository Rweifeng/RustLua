# check-wasm-release-recipe.ps1 —— Web/微信 wasm 发布 recipe 的一致性门（M86-01）
#
# 回答的问题：**打包用的那条 wasm 构建命令现在还能不能编、编出来还能不能进 V8**。
# 为什么需要：wasm toolchain 写死在 `scripts/build/package/30-build.ps1`，此前没有任何 CI 覆盖；
# M84 引入 `spine2d@0.5`（manifest 声明 rust-version=1.95）后，打包链在**没人注意**的情况下编不出来，
# 直到 2026-11-26 做包体归因时才撞到（M86-01）。这条门把 recipe 的关键约束搬到 CI。
#
# 三道：
#   ① 工具链与 wasm32 target 就绪（缺 → exit 3 BLOCKED，不是通过）；
#   ② 按打包 recipe 构建 `game_web`（工具链来自 scripts/build/wasm-toolchain.ps1 的单一事实源；
#      profile 覆盖与 30-build.ps1 相同；`--ignore-rust-version` 是**有意**的，理由见 docs/84）；
#   ③ 产物必须能在 V8 **默认参数**下编译（纯 legacy EH；new EH/混合 EH 都判红）。
#
# 用法：
#   pwsh -NoProfile -File scripts/verify/check-wasm-release-recipe.ps1
#   pwsh ... -StatusFile artifacts/logs/wasm-recipe/ci.status
#
# 退出码：0 = 通过；1 = 红；3 = 环境受限（缺 rustup 工具链 / wasm32 target / zig）。

[CmdletBinding()]
param(
    [string]$StatusFile = ""
)

$ErrorActionPreference = "Stop"

function Exit-With([int]$Code) {
    if (-not [string]::IsNullOrWhiteSpace($StatusFile)) {
        $dir = Split-Path -Parent $StatusFile
        if ($dir) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
        "exit=$Code" | Set-Content -LiteralPath $StatusFile -Encoding ASCII
    }
    exit $Code
}

$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$engineRoot = Join-Path $projectRoot "engine"
. (Join-Path $projectRoot "scripts\build\wasm-toolchain.ps1")
$toolchain = Get-RustLuaWasmToolchain
$toolchainVersion = $toolchain.TrimStart('+')

# ── ① 工具链 / target / zig ────────────────────────────────────────────────
$ErrorActionPreference = "Continue"
& rustup run $toolchainVersion rustc --version 2>&1 | Out-Null
$rustcOk = ($LASTEXITCODE -eq 0)
$targets = & rustup target list --toolchain $toolchainVersion --installed 2>&1
$targetOk = ($LASTEXITCODE -eq 0) -and (($targets | Select-String -SimpleMatch "wasm32-unknown-unknown") -ne $null)
$ErrorActionPreference = "Stop"
if (-not $rustcOk) {
    Write-Host "[BLOCKED] rustc $toolchainVersion 未安装（Web/微信 wasm recipe 的固定工具链）。安装：rustup toolchain install $toolchainVersion --profile minimal" -ForegroundColor Yellow
    Exit-With 3
}
if (-not $targetOk) {
    Write-Host "[BLOCKED] toolchain $toolchainVersion 缺 wasm32-unknown-unknown target。安装：rustup target add wasm32-unknown-unknown --toolchain $toolchainVersion" -ForegroundColor Yellow
    Exit-With 3
}
$zig = $null
try {
    $zig = (& powershell -NoProfile -File (Join-Path $projectRoot "scripts\build\zig-toolchain.ps1") -NoDownload -PrintPath 2>$null | Select-Object -Last 1)
} catch { $zig = $null }
if ([string]::IsNullOrWhiteSpace($zig) -or -not (Test-Path -LiteralPath $zig)) {
    Write-Host "[BLOCKED] 找不到 zig（vendored Lua 的 wasm C 源需要它）。跑 scripts\build\zig-toolchain.ps1 安装，或设 ZIG" -ForegroundColor Yellow
    Exit-With 3
}

# ── ② 按打包 recipe 构建 ──────────────────────────────────────────────────
$targetDir = Join-Path $projectRoot "artifacts\verify-wasm-recipe\target"
$wasmPath = Join-Path $targetDir "wasm32-unknown-unknown\release\game_web.wasm"
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $wasmPath) | Out-Null

$saved = @{
    ZIG = $env:ZIG
    STRIP = $env:CARGO_PROFILE_RELEASE_STRIP
    OPT = $env:CARGO_PROFILE_RELEASE_OPT_LEVEL
    LTO = $env:CARGO_PROFILE_RELEASE_LTO
    CGU = $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS
    TARGET = $env:CARGO_TARGET_DIR
}
try {
    $env:ZIG = $zig
    $env:CARGO_PROFILE_RELEASE_STRIP = "none"
    $env:CARGO_PROFILE_RELEASE_OPT_LEVEL = "z"
    $env:CARGO_PROFILE_RELEASE_LTO = "fat"
    $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "1"
    $env:CARGO_TARGET_DIR = $targetDir
    Push-Location $engineRoot
    try {
        Write-Host ("==> wasm release recipe: cargo {0} build -p game_web --release --target wasm32-unknown-unknown --ignore-rust-version" -f $toolchain)
        $ErrorActionPreference = "Continue"
        cargo $toolchain build -p game_web --release --target wasm32-unknown-unknown --ignore-rust-version 2>&1 | Select-Object -Last 15 | ForEach-Object { Write-Host ("    " + $_) }
        $buildCode = $LASTEXITCODE
        $ErrorActionPreference = "Stop"
        if ($buildCode -ne 0) {
            Write-Host "[RED] wasm release recipe 构建失败（cargo exit $buildCode）。若是因为依赖 MSRV 变更，按 docs/84 重新评估路线（不要随手升/降工具链）" -ForegroundColor Red
            Exit-With 1
        }
    }
    finally {
        Pop-Location
    }
}
finally {
    $env:ZIG = $saved.ZIG
    $env:CARGO_PROFILE_RELEASE_STRIP = $saved.STRIP
    $env:CARGO_PROFILE_RELEASE_OPT_LEVEL = $saved.OPT
    $env:CARGO_PROFILE_RELEASE_LTO = $saved.LTO
    $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = $saved.CGU
    $env:CARGO_TARGET_DIR = $saved.TARGET
}

if (-not (Test-Path -LiteralPath $wasmPath)) {
    Write-Host "[RED] recipe 构建成功但没有产物：$wasmPath" -ForegroundColor Red
    Exit-With 1
}

# ── ③ V8 默认参数下必须能编译（纯 legacy EH）───────────────────────────────
$ErrorActionPreference = "Continue"
& node (Join-Path $projectRoot "scripts\verify\wasm-eh-check.mjs") $wasmPath
$ehCode = $LASTEXITCODE
$ErrorActionPreference = "Stop"
if ($ehCode -ne 0) {
    Write-Host "[RED] wasm 产物的 EH 形态不合规：V8 默认参数下编译失败（见上方 wasm_eh_check_failed 的 reason）。若是 'mix of legacy and new'，说明工具链/依赖变了，按 docs/84 处置" -ForegroundColor Red
    Exit-With 1
}

$bytes = (Get-Item -LiteralPath $wasmPath).Length
Write-Host ("wasm_recipe_ok toolchain={0} bytes={1} eh=legacy target_dir={2}" -f $toolchainVersion, $bytes, $targetDir) -ForegroundColor Green
Exit-With 0
