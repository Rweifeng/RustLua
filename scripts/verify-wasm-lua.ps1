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
    （见 engine/vendor/mlua-sys/build/main_inner.rs 的 find_zig）。zig 的位置由
    scripts/zig-toolchain.ps1 统一解析（$env:ZIG → engine\.tools\zig → PATH），本脚本
    **只探测不下载**（质量门不该为了跑而去拉 100 MB 网络），解析不到就按环境受限跳过。
    wasm-bindgen CLI 同理走 scripts/wasm-bindgen.ps1：版本必须与 Cargo.lock 里的
    wasm-bindgen 精确一致，PATH 上那份旧 CLI 不能凑合用。

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
# 第二档胶水（带 `gpu2d` 特性，WP8 微信半边第一片）：与默认档产物**分开**放，
# 两档的导出清单不同（默认档没有 GPU 那条路上的任何导出）。
$glueGpuDir = Join-Path $workspaceRoot "artifacts\wasm-lua-verify\no-modules-gpu"
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

# zig 与 wasm-bindgen 的解析统一在 scripts/ 下的两个脚本里（与 package.ps1 共用同一份逻辑）。
# 这里只探测不安装：质量门不能为了跑一次验证去拉 ~100 MB 工具链。
. (Join-Path $PSScriptRoot "zig-toolchain.ps1")
. (Join-Path $PSScriptRoot "wasm-bindgen.ps1")

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

$zig = Resolve-ZigToolchain
if (-not $zig) {
    Write-Log "wasm_lua_skipped reason=no_zig (vendored Lua 的 wasm C 编译需要 zig；跑 scripts\zig-toolchain.ps1 装，或设 `$env:ZIG=<path>\zig.exe / 放进 PATH)" Yellow
    Exit-With 3
}
# 进程环境优先于 config.toml 的 [env]（非 force）——把解析到的真实 zig 交给 build.rs。
$env:ZIG = $zig
Write-Log "zig = $zig"

# 版本必须与 Cargo.lock 里的 wasm-bindgen 精确一致；PATH 上那份旧 CLI 不能凑合（详见
# scripts/wasm-bindgen.ps1）。这里允许安装到仓外缓存 engine\.tools\wasm-bindgen，
# 因为本门的产物（no-modules 胶水）没有它就是**假通过**——沿用原来"当场 cargo install"
# 的取向，只是不再污染全局 ~/.cargo/bin。
$wasmBindgen = Resolve-WasmBindgenCli -Provision
if (-not $wasmBindgen) {
    Write-Log "wasm_lua_skipped reason=no_wasm_bindgen (需与 Cargo.lock 的 wasm-bindgen 同版本；跑 scripts\wasm-bindgen.ps1 装)" Yellow
    Exit-With 3
}
Write-Log "wasm-bindgen = $wasmBindgen"

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
    $bindgen = Invoke-Native $wasmBindgen @($gameWebWasm, "--out-dir", $glueDir, "--target", "no-modules", "--no-typescript", "--remove-name-section")
    if ($bindgen -ne 0) { throw "wasm-bindgen (no-modules) 失败 (exit $bindgen)" }

    # 第二档：**带上 `gpu2d` 特性**再构建一次（WP8 微信半边第一片暴露出的门缺口）。
    # 为什么必须两档都验：`gpu2d` 是可选特性（`dep:wgpu` 只在它下面），发布时由
    # `package.ps1 -Gpu2d` 打开。此前本门只构建默认档 ⇒ "GPU 那条路上的导出"在门里
    # **根本不存在**，于是探针用例只会报"不可调用"——那是门自己造成的假红，
    # 而不是被测代码的问题。两档产物分开输出：导出清单不同，混用会互相掩盖。
    #
    # 注意两个自己踩过的坑：①cargo 必须在 `engine/` 下跑（上面那个 Push-Location 已经
    # Pop 掉了，这里要自己再进一次——实测直接跑会得到"could not find Cargo.toml"）；
    # ②`CARGO_PROFILE_RELEASE_STRIP=none` 同样要重设，否则 wasm-bindgen 会因缺
    # `target_features` 段报 "externref table required for catch wrappers"。
    $savedStripGpu = $env:CARGO_PROFILE_RELEASE_STRIP
    $env:CARGO_PROFILE_RELEASE_STRIP = "none"
    Push-Location $engineRoot
    try {
        Write-Log "==> 构建 game_web (release, wasm32, --features gpu2d)"
        $gpuBuild = Invoke-Native cargo @("build", "-p", "game_web", "--release", "--target", "wasm32-unknown-unknown", "--features", "gpu2d")
        if ($gpuBuild -ne 0) { throw "game_web wasm (gpu2d) 构建失败 (exit $gpuBuild)" }
    }
    finally {
        Pop-Location
        if ($null -eq $savedStripGpu) { Remove-Item Env:\CARGO_PROFILE_RELEASE_STRIP -ErrorAction SilentlyContinue }
        else { $env:CARGO_PROFILE_RELEASE_STRIP = $savedStripGpu }
    }

    Write-Log "==> wasm-bindgen --target no-modules (gpu2d) → $glueGpuDir"
    New-Item -ItemType Directory -Force -Path $glueGpuDir | Out-Null
    $bindgenGpu = Invoke-Native $wasmBindgen @($gameWebWasm, "--out-dir", $glueGpuDir, "--target", "no-modules", "--no-typescript", "--remove-name-section")
    if ($bindgenGpu -ne 0) { throw "wasm-bindgen (no-modules, gpu2d) 失败 (exit $bindgenGpu)" }

    # 收尾：把磁盘上的 wasm 还原成**默认档**。上面那一档会把
    # `target\wasm32-unknown-unknown\release\game_web.wasm` 覆盖成带 `gpu2d` 的那一份，
    # 而 `package.ps1 -SkipBuild` 正是直接复用这个路径 ⇒ 不改回来，下一次打包就会得到
    # "页面不带 -Gpu2d、wasm 却是 GPU 档"的错配产物（而且它不会自己报错）。
    # 这一步只把状态复原成本门跑之前的样子，不是新的验证内容。
    $savedStripRestore = $env:CARGO_PROFILE_RELEASE_STRIP
    $env:CARGO_PROFILE_RELEASE_STRIP = "none"
    Push-Location $engineRoot
    try {
        Write-Log "==> 还原默认档 wasm（避免 -SkipBuild 打包到 gpu2d 档）"
        $restoreBuild = Invoke-Native cargo @("build", "-p", "game_web", "--release", "--target", "wasm32-unknown-unknown")
        if ($restoreBuild -ne 0) { throw "默认档 wasm 还原构建失败 (exit $restoreBuild)" }
    }
    finally {
        Pop-Location
        if ($null -eq $savedStripRestore) { Remove-Item Env:\CARGO_PROFILE_RELEASE_STRIP -ErrorAction SilentlyContinue }
        else { $env:CARGO_PROFILE_RELEASE_STRIP = $savedStripRestore }
    }
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

# ---------- 4. 输入批量线格式的语义对拍 ----------
# 新协议（`RLFI` 批量二进制）必须与旧协议（每帧编译源码）逐步等价：两边都从出厂源码里抽
# （旧的序列化器 / 新的打包器），配上真 wasm。省下的开销容易量，语义漂移不会自己变红。
Write-Log "==> input-frame-abi-test.mjs"
$abiExit = Invoke-NodeScript $nodeFlags (Join-Path $scriptDir "input-frame-abi-test.mjs") $glueDir

# ---------- 5. 宿主画布探针的契约（WP8 微信半边第一片） ----------
# `probe_canvas_host` 是"引擎能不能在小游戏那种没有浏览器构造器的宿主上起渲染栈"的判据。
# 它只在 `gpu2d` 特性下存在 ⇒ 用上面那一档（带特性的）胶水来验。离线能钉死的部分：
# 入口可调用、**普通对象**能进到 GL 层（⇒ 路径里没有 `dyn_into` 那种构造器判定）、
# 失败给具名原因而不是 undefined/挂起。真机那一半（GL 到底可不可用）只能在设备上回答，
# 见 `wechat-adapter.js` 的 `glCapability`。
if (Test-Path -LiteralPath (Join-Path $glueGpuDir "game_web.js")) {
    Write-Log "==> verify-canvas-host-probe.mjs (gpu2d)"
    $probeExit = Invoke-NodeScript $nodeFlags (Join-Path $scriptDir "verify-canvas-host-probe.mjs") $glueGpuDir
}
else {
    Write-Log "wasm_lua_warn: 缺 gpu2d 胶水（$glueGpuDir），宿主画布探针用例未跑（-SkipBuild 时会这样）" Yellow
    $probeExit = 3
}

Write-Log "==> 汇总: spike_exit=$spikeExit bridge_exit=$bridgeExit abi_exit=$abiExit probe_exit=$probeExit"

# 任一为 3（输入缺失/环境受限）且无真失败 → 整体按环境受限跳过；其余非零一律判红。
$exits = @($spikeExit, $bridgeExit, $abiExit, $probeExit)
$realFailures = @($exits | Where-Object { $_ -ne 0 -and $_ -ne 3 })
if (($exits -contains 3) -and ($realFailures.Count -eq 0)) {
    Write-Log "wasm_lua_skipped reason=node_input_missing" Yellow
    Exit-With 3
}
if ($realFailures.Count -gt 0) {
    Write-Log "wasm_lua_FAILED spike_exit=$spikeExit bridge_exit=$bridgeExit abi_exit=$abiExit probe_exit=$probeExit" Red
    Exit-With 1
}

Write-Log "wasm_lua_ok spike=pass bridge=pass abi=pass probe=pass" Green
Exit-With 0
