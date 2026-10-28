# Web / 微信 UI 证据 harness 的构建与运行入口。
#
# 为什么需要它：`ui/hosts.json` 的三宿主证据里，pc 由编辑器的
# `m76_ui_matrix` 例子生产；Web/微信此前没有生产端，于是永远是 `untested`。
# 本脚本构建 `rustlua-ui-host-harness`（wasm32），用两个宿主各自的胶水形态装载，
# 跑同一份 `.ui.json` 的版面/块序读数，落成产物并写回清单对应条目。
#
# 证明什么、不证明什么：
#   * 证明：同一份文档在 **wasm32 执行**下由同一台 RuntimeScene 导出/解算/提交，读数与
#     pc 逐键一致；revision 由 wasm 侧对字节现算（不是 JS 抄一个数字）；胶水形态是
#     web 的 `--target web` 与微信的 `--target no-modules`（package.ps1 同款补丁）。
#   * 不证明：浏览器 canvas/WebGL、小游戏 canvas、字体图集、WXWebAssembly 真机行为。
#     产物 `sdf_vertices=0` 与 note 都把这条边界摊开；没跑过的东西不会被写成"已测"。
#
# 退出码与 scripts/verify/verify-wasm-lua.ps1 同一套语义：
#   0 = 两个宿主都绿且产物已落盘；1 = 真失败；3 = 环境受限（缺 node/wasm32/zig/wasm-bindgen）。
#
# 用法：
#   powershell -NoProfile -File scripts\verify\ui-host-evidence.ps1 -Target both
#   powershell -NoProfile -File scripts\verify\ui-host-evidence.ps1 -Target web -SkipBuild
[CmdletBinding()]
param(
    [ValidateSet("web", "wechat", "both")]
    [string]$Target = "both",
    # 文档与工程都按仓库相对路径传（产物里的 document 就是它）。
    [string]$Document = "ui/matrix.ui.json",
    [string]$Project = "examples/game_api",
    # 复用已生成的 wasm/胶水（本地迭代用）；默认重建，避免拿旧产物冒充本次读数。
    [switch]$SkipBuild,
    # 只跑读数、不动 ui/hosts.json（反面对照/演练用）。
    [switch]$NoHosts,
    [string]$StatusFile = ""
)

$ErrorActionPreference = "Stop"
try { [Console]::OutputEncoding = New-Object Text.UTF8Encoding($false) } catch { }

function Exit-With([int]$Code) {
    if (-not [string]::IsNullOrWhiteSpace($StatusFile)) {
        "exit=$Code" | Set-Content -LiteralPath $StatusFile -Encoding ASCII
    }
    exit $Code
}

trap {
    Write-Host "[RED] ui host evidence aborted: $($_.Exception.Message) @line $($_.InvocationInfo.ScriptLineNumber)"
    Exit-With 1
}

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$engineRoot = Join-Path $repoRoot "engine"
$glueRoot = Join-Path $repoRoot "artifacts\ui-host-harness"
$wasmFile = Join-Path $engineRoot "target\wasm32-unknown-unknown\release\rustlua_ui_host_harness.wasm"

# 工具链解析与 package.ps1 / verify-wasm-lua.ps1 共用同一份逻辑；本脚本**只探测不下载**，
# 解析不到按环境受限退出（质量门不该为了跑一次验证去拉网络）。
. (Join-Path $PSScriptRoot "..\build\zig-toolchain.ps1")
. (Join-Path $PSScriptRoot "..\build\wasm-bindgen.ps1")

if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
    Write-Host "ui_host_evidence_skipped reason=no_node" -ForegroundColor Yellow
    Exit-With 3
}
$installed = @(& rustup target list --installed 2>$null)
if ($installed -notcontains "wasm32-unknown-unknown") {
    Write-Host "ui_host_evidence_skipped reason=no_wasm32_target" -ForegroundColor Yellow
    Exit-With 3
}
$zig = Resolve-ZigToolchain
if (-not $zig) {
    Write-Host "ui_host_evidence_skipped reason=no_zig (跑 scripts\build\zig-toolchain.ps1 装，或设 `$env:ZIG)" -ForegroundColor Yellow
    Exit-With 3
}
$env:ZIG = $zig
$wasmBindgen = Resolve-WasmBindgenCli -Provision
if (-not $wasmBindgen) {
    Write-Host "ui_host_evidence_skipped reason=no_wasm_bindgen" -ForegroundColor Yellow
    Exit-With 3
}

$targets = if ($Target -eq "both") { @("web", "wechat") } else { @($Target) }

# wasm 产物带 exnref 指令；Node 的 flag 在不同版本里可能已默认开启或已移除，先探测再传，
# 避免 "bad option" 被误判成 harness 失败（与 verify-wasm-lua.ps1 同一处理）。
$nodeFlags = @()
& node --experimental-wasm-exnref -e "0" 2>$null
if ($LASTEXITCODE -eq 0) { $nodeFlags += "--experimental-wasm-exnref" }

if (-not $SkipBuild) {
    Write-Host "==> 构建 rustlua-ui-host-harness (release, wasm32)"
    # wasm-bindgen 需要 target_features 自定义段（含 reference-types），否则报
    # "externref table required for catch wrappers"（与 verify-wasm-lua.ps1 同款）。
    $savedStrip = $env:CARGO_PROFILE_RELEASE_STRIP
    $env:CARGO_PROFILE_RELEASE_STRIP = "none"
    Push-Location $engineRoot
    try {
        & cargo build -p rustlua-ui-host-harness --release --target wasm32-unknown-unknown
        if ($LASTEXITCODE -ne 0) { throw "rustlua-ui-host-harness wasm 构建失败 (exit $LASTEXITCODE)" }
    }
    finally {
        Pop-Location
        if ($null -eq $savedStrip) { Remove-Item Env:\CARGO_PROFILE_RELEASE_STRIP -ErrorAction SilentlyContinue }
        else { $env:CARGO_PROFILE_RELEASE_STRIP = $savedStrip }
    }
    if (-not (Test-Path -LiteralPath $wasmFile)) { throw "wasm 产物不存在：$wasmFile" }

    foreach ($hostId in $targets) {
        # 两个宿主用各自的真实胶水形态：web 是 ESM（浏览器 `<script type=module>` 用的那份），
        # 微信是 no-modules（小游戏代码包里 require 的那份）。
        $mode = if ($hostId -eq "web") { "web" } else { "no-modules" }
        $glueDir = Join-Path $glueRoot $hostId
        New-Item -ItemType Directory -Force -Path $glueDir | Out-Null
        Write-Host "==> wasm-bindgen --target $mode → $glueDir"
        & $wasmBindgen $wasmFile "--target" $mode "--no-typescript" "--out-name" "ui_host_harness" "--out-dir" $glueDir
        if ($LASTEXITCODE -ne 0) { throw "wasm-bindgen ($mode) 失败 (exit $LASTEXITCODE)" }
    }
}

foreach ($hostId in $targets) {
    $relativeGlue = "artifacts/ui-host-harness/$hostId"
    $relativeOutput = "$Project/artifacts/ui-matrix/$hostId.json"
    $command = "powershell -NoProfile -File scripts/verify/ui-host-evidence.ps1 -Target $hostId"
    $nodeArgs = @(
        "scripts/verify/ui-host-evidence.mjs",
        "--host", $hostId,
        "--document", $Document,
        "--project", $Project,
        "--glue-dir", $relativeGlue,
        "--output", $relativeOutput,
        "--command", $command
    )
    if (-not $NoHosts) { $nodeArgs += "--write-hosts" }
    Write-Host "==> node $($nodeFlags -join ' ') $($nodeArgs -join ' ')"
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & node @nodeFlags @nodeArgs 2>&1 | ForEach-Object { Write-Host $_.ToString() }
    }
    finally {
        $ErrorActionPreference = $prev
    }
    if ($LASTEXITCODE -ne 0) {
        Write-Host "ui_host_evidence_failed host=$hostId exit=$LASTEXITCODE" -ForegroundColor Red
        Exit-With 1
    }
}

Write-Host "ui_host_evidence_ok targets=$($targets -join ',') document=$Document"
Exit-With 0
