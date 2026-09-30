<#
.SYNOPSIS
    wasm Lua 运行期验证门：spike（vendored mlua 移植）+ no-modules 桥（game_web Lua 桥）。

.DESCRIPTION
    把此前只在 artifacts/ 里手动跑的两个 Node 验证脚本纳入可重复的门：
      ① scripts/wasm-lua/run-spike.mjs            —— 直接实例化 wasm_lua_spike.wasm，
         验证 vendored mlua 的 wasm32-unknown-unknown 移植（EH/longjmp、libc shim、%g 精度）。
      ② scripts/wasm-lua/verify-no-modules-bridge.mjs —— 在 no-modules 胶水（微信形态）下
         验证 game_web 的 Lua 桥导出（lua_boot/lua_do_string/lua_call_global/…）。

    退出码语义与 verify-webgl*.ps1 一致（供 verify.ps1 / CI 消费）：
      0 = 全部通过；
      1 = 真失败（构建不过，或 Node 用例断言失败）——判红；
      3 = 环境受限（缺 node / wasm32 target / zig / wasm-bindgen）——显式打印原因后跳过，
          绝不把"跳过"说成"通过"（与仓库既有 -Web 门同一哲学）。

    为什么需要 zig：vendored Lua 的 C 源用 `zig cc --target=wasm32-freestanding` 编译
    （见 engine/vendor/mlua-sys/build/main_inner.rs 的 find_zig）。engine/.cargo/config.toml
    里的 ZIG 是**本机绝对路径**，在别的机器/CI 上不存在；因此本脚本先解析一个真实可用的
    zig 并写进 $env:ZIG（cargo 的 [env] 非 force，进程环境优先），解析不到就按环境受限跳过。

.PARAMETER StatusFile
    调用方（verify.ps1）用它拿退出码：Start-Process -PassThru 的 ExitCode 在重定向下是 $null。

.PARAMETER SkipBuild
    复用已存在的 wasm 产物，只重跑 Node 验证（本地快速迭代用）。
#>

[CmdletBinding()]
param(
    [string]$StatusFile = "",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

function Exit-With([int]$Code) {
    if (-not [string]::IsNullOrWhiteSpace($StatusFile)) {
        "exit=$Code" | Set-Content -LiteralPath $StatusFile -Encoding ASCII
    }
    exit $Code
}

trap {
    Write-Host "[RED] wasm-lua verification aborted: $($_.Exception.Message) @line $($_.InvocationInfo.ScriptLineNumber)"
    Exit-With 1
}

$workspaceRoot = Split-Path -Parent $PSScriptRoot
$engineRoot = Join-Path $workspaceRoot "engine"
$scriptDir = Join-Path $PSScriptRoot "wasm-lua"
$wasmReleaseRoot = Join-Path $engineRoot "target\wasm32-unknown-unknown\release"
$glueDir = Join-Path $workspaceRoot "artifacts\wasm-lua-verify\no-modules-test"
$spikeWasm = Join-Path $wasmReleaseRoot "wasm_lua_spike.wasm"
$gameWebWasm = Join-Path $wasmReleaseRoot "game_web.wasm"

$logRoot = Join-Path $workspaceRoot "artifacts\logs\wasm-lua"
New-Item -ItemType Directory -Force -Path $logRoot | Out-Null
$logPath = Join-Path $logRoot ("wasm-lua-{0}.log" -f (Get-Date -Format "yyyyMMdd-HHmmss"))

function Write-Log([string]$Message, [string]$Color = "Gray") {
    if ($Color -eq "Yellow" -or $Color -eq "Red" -or $Color -eq "Green" -or $Color -eq "Cyan") {
        Write-Host ("[{0}] {1}" -f $Color.ToUpperInvariant(), $Message)
    } else {
        Write-Host $Message
    }
    Add-Content -Path $logPath -Value $Message -Encoding UTF8
}

# 原生程序（cargo/rustup/node）会把进度写进 stderr；PS 5.1 在 $ErrorActionPreference='Stop'
# 下会把它当终止性错误（NativeCommandError）。调原生程序时临时降为 Continue。
function Invoke-Native([string]$File, [string[]]$Arguments) {
    Write-Log "> $File $($Arguments -join ' ')"
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $File @Arguments 2>&1 | ForEach-Object { Write-Log $_.ToString() }
    }
    finally {
        $ErrorActionPreference = $prev
    }
    return $LASTEXITCODE
}

function Resolve-Zig {
    if ($env:ZIG -and (Test-Path -LiteralPath $env:ZIG)) { return $env:ZIG }
    $cmd = Get-Command zig -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $cfg = Join-Path $engineRoot ".cargo\config.toml"
    if (Test-Path -LiteralPath $cfg) {
        $match = Select-String -Path $cfg -Pattern 'ZIG\s*=\s*"(.+?)"' | Select-Object -First 1
        if ($match) {
            $declared = $match.Matches[0].Groups[1].Value -replace '\\\\', '\'
            if (Test-Path -LiteralPath $declared) { return $declared }
        }
    }
    return $null
}

# 跑一个 Node 脚本并返回其退出码。必须在 EAP=Continue 下调用：Node 用 stderr 报
# 断言失败/异常，PS 5.1 在 EAP=Stop 下会把重定向来的 stderr 当终止性错误抛进 trap，
# 于是"测试失败"变成"脚本崩溃"、真实原因被吞。这里把输出逐行写日志、只认退出码。
function Invoke-NodeScript([string[]]$Flags, [string]$Script, [string]$Argument) {
    Write-Log "> node $($Flags -join ' ') $Script $Argument"
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & node @Flags $Script $Argument 2>&1 | ForEach-Object { Write-Log $_.ToString() }
    }
    finally {
        $ErrorActionPreference = $prev
    }
    return $LASTEXITCODE
}

Write-Log "==> wasm Lua verification (spike + no-modules bridge)" Cyan

# ---------- 0. 环境前置检查：缺任何一个都按"环境受限"跳过（exit 3），不判红 ----------
if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
    Write-Log "wasm_lua_skipped reason=no_node" Yellow
    Exit-With 3
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Log "wasm_lua_skipped reason=no_cargo" Yellow
    Exit-With 3
}

Push-Location $engineRoot
try {
    # 注意：$installed 是多行数组，不能用 `-notmatch`（数组 -match 是**过滤**语义，
    # 会把其它 target 都当"不匹配"返回，非空即 truthy → 永远误判未安装）。用 -notcontains。
    $installed = @(& rustup target list --installed 2>$null)
    if ($installed -notcontains "wasm32-unknown-unknown") {
        Write-Log "wasm32-unknown-unknown 未安装，尝试 rustup target add …"
        $null = Invoke-Native rustup @("target", "add", "wasm32-unknown-unknown")
        $installed = @(& rustup target list --installed 2>$null)
        if ($installed -notcontains "wasm32-unknown-unknown") {
            Write-Log "wasm_lua_skipped reason=no_wasm32_target" Yellow
            Exit-With 3
        }
    }
}
finally {
    Pop-Location
}

$zig = Resolve-Zig
if (-not $zig) {
    Write-Log "wasm_lua_skipped reason=no_zig (vendored Lua 的 wasm C 编译需要 zig；设 `$env:ZIG 或把 zig 放进 PATH)" Yellow
    Exit-With 3
}
# 进程环境优先于 config.toml 的 [env]（非 force）——把解析到的真实 zig 交给 build.rs。
$env:ZIG = $zig
Write-Log "zig = $zig"

if (-not (Get-Command wasm-bindgen -ErrorAction SilentlyContinue)) {
    Write-Log "wasm-bindgen 未找到，尝试 cargo install wasm-bindgen-cli --version 0.2.129 …"
    $installExit = Invoke-Native cargo @("install", "wasm-bindgen-cli", "--version", "0.2.129")
    if (-not (Get-Command wasm-bindgen -ErrorAction SilentlyContinue)) {
        Write-Log "wasm_lua_skipped reason=no_wasm_bindgen" Yellow
        Exit-With 3
    }
}

# Node 的 wasm EH flag 在不同版本里可能被移除/默认开启：探测一次，避免"bad option"误判。
$nodeFlags = @()
$probe = & node --experimental-wasm-exnref -e "0" 2>&1
if ($LASTEXITCODE -eq 0) { $nodeFlags += "--experimental-wasm-exnref" }
else { Write-Log "node 不接受 --experimental-wasm-exnref（可能已默认开启），改用无 flag 调用" Yellow }

# ---------- 1. 构建 ----------
if (-not $SkipBuild) {
    # 保留 target_features 自定义段（含 reference-types），否则 wasm-bindgen 报
    # "externref table required for catch wrappers"（见 package.ps1 同款处理）。
    $savedStrip = $env:CARGO_PROFILE_RELEASE_STRIP
    $env:CARGO_PROFILE_RELEASE_STRIP = "none"

    Push-Location $engineRoot
    try {
        Write-Log "==> 构建 wasm-lua-spike (release, wasm32)"
        $spikeBuild = Invoke-Native cargo @("build", "-p", "wasm-lua-spike", "--release", "--target", "wasm32-unknown-unknown")
        if ($spikeBuild -ne 0) { throw "wasm-lua-spike wasm 构建失败 (exit $spikeBuild)" }

        Write-Log "==> 构建 game_web (release, wasm32)"
        $webBuild = Invoke-Native cargo @("build", "-p", "game_web", "--release", "--target", "wasm32-unknown-unknown")
        if ($webBuild -ne 0) { throw "game_web wasm 构建失败 (exit $webBuild)" }
    }
    finally {
        Pop-Location
        if ($null -eq $savedStrip) { Remove-Item Env:\CARGO_PROFILE_RELEASE_STRIP -ErrorAction SilentlyContinue }
        else { $env:CARGO_PROFILE_RELEASE_STRIP = $savedStrip }
    }

    Write-Log "==> wasm-bindgen --target no-modules → $glueDir"
    New-Item -ItemType Directory -Force -Path $glueDir | Out-Null
    $bindgen = Invoke-Native wasm-bindgen @($gameWebWasm, "--out-dir", $glueDir, "--target", "no-modules", "--no-typescript", "--remove-name-section")
    if ($bindgen -ne 0) { throw "wasm-bindgen (no-modules) 失败 (exit $bindgen)" }
}

# ---------- 2. spike 验证 ----------
if (-not (Test-Path -LiteralPath $spikeWasm)) {
    Write-Log "wasm_lua_skipped reason=spike_wasm_missing ($spikeWasm)" Yellow
    Exit-With 3
}
Write-Log "==> run-spike.mjs"
$spikeExit = Invoke-NodeScript $nodeFlags (Join-Path $scriptDir "run-spike.mjs") $spikeWasm

# ---------- 3. no-modules 桥验证 ----------
Write-Log "==> verify-no-modules-bridge.mjs"
$bridgeExit = Invoke-NodeScript $nodeFlags (Join-Path $scriptDir "verify-no-modules-bridge.mjs") $glueDir

Write-Log "==> 汇总: spike_exit=$spikeExit bridge_exit=$bridgeExit"

# 任一为 3（输入缺失/环境受限）且无真失败 → 整体按环境受限跳过。
if (($spikeExit -eq 3 -or $bridgeExit -eq 3) -and $spikeExit -ne 1 -and $bridgeExit -ne 1) {
    Write-Log "wasm_lua_skipped reason=node_input_missing" Yellow
    Exit-With 3
}
if ($spikeExit -ne 0 -or $bridgeExit -ne 0) {
    Write-Log "wasm_lua_FAILED spike_exit=$spikeExit bridge_exit=$bridgeExit" Red
    Exit-With 1
}

Write-Log "wasm_lua_ok spike=pass bridge=pass" Green
Exit-With 0
