# -----------------------------------------------------------------------------
# 打包阶段：Release 构建（cargo build / -SkipBuild 复用）
# 自 scripts/build/package.ps1 原样迁出（原 L524-L687，2026-11-02 按功能拆分）。
# 执行语义：本文件被入口 dot-source，跑在入口的脚本作用域里——阶段之间共享变量、
#   ErrorActionPreference 与 throw 行为都跟拆分前一致。它不是函数，别当函数调。
#   PSScriptRoot 已统一换成入口设置的 packageScriptRoot（本文件不在 scripts/build/ 下）。
# -----------------------------------------------------------------------------

    Write-Host "==> Release build ($Platform)" -ForegroundColor Cyan
    if ($SkipBuild) {
        # M20-03: repeated publish without rebuilding; require existing artifacts.
        $requiredArtifacts = @((Join-Path $nativeTargetRoot "assetc.exe"))
    if ($isWebPlatform) {
            # 两个 web 平台都要 wasm 产物：webgl 发行 `--target web` 的浏览器胶水，
            # 微信小游戏发行 `--target no-modules` 的 CommonJS 化胶水（见下方 pkg/ 说明）。
            $requiredArtifacts += (Join-Path $wasmTargetRoot "game_web.wasm")
        } elseif (-not $isWebPlatform) {
            $requiredArtifacts += (Join-Path $nativeTargetRoot "game.exe")
        }
        foreach ($artifact in $requiredArtifacts) {
            if (-not (Test-Path -LiteralPath $artifact)) {
                throw "SkipBuild requires existing release artifact: $artifact"
            }
        }
        Write-Host "==> Skipping cargo build (-SkipBuild): reusing existing artifacts" -ForegroundColor DarkGray
    }
    elseif ($isWebPlatform) {
        $savedEap = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            cargo build -p assetc --release -q 2>&1 | Out-Null
        }
        finally {
            $ErrorActionPreference = $savedEap
        }
        if ($LASTEXITCODE -ne 0) { throw "assetc release build failed." }
        # webgl 与微信小游戏**共用同一份原始 WASM 构建**（`-p game_web` 只构建一次；胶水按平台
        # 各生成一份，微信侧最终二进制另经剥 Global / 剥 Tag / noextern 降级等兼容后处理——见
        # 下方 strip/downgrade 段。两平台的 _bg.wasm 不逐字节相同，哈希不同是预期行为）。
        # PS 5.1 陷阱：$ErrorActionPreference='Stop' 下原生程序写 stderr 会变成**终止性
        # 错误**（NativeCommandError），rustup/cargo 的正常进度输出也在 stderr —— 必须临时
        # 降为 Continue 再调，否则打包在构建第一步就误报失败。
        #
        # zig：mlua-sys 的 build.rs 用 `zig cc` 编 vendored Lua 的 C 源，**只有 web 平台**需要。
        # 必须显式写进 $env:ZIG：以前靠 engine/.cargo/config.toml 的 [env] 指某台机器的绝对路径，
        # 换一台机器就 panic `run zig cc: NotFound`，再被下面的 `| Out-Null` 吞成
        # "WASM build failed"（实测 2026-09-30）。解析与按需下载见 scripts/build/zig-toolchain.ps1。
        $zig = Resolve-ZigToolchain -Install
        if (-not $zig) {
            throw "zig not found（webgl/微信 需要它编 wasm 侧的 Lua C 源）：跑 scripts\build\zig-toolchain.ps1 自动装，或设 `$env:ZIG=<path>\zig.exe。"
        }
        $env:ZIG = $zig
        # wasm 侧固定用 1.88.0（见下方 WP10 的 EH 口径），所以 wasm32 目标要装到**这个
        # toolchain**上：`rustup target add` 不带 `--toolchain` 只装默认 stable，1.88.0
        # 依然报 "can't find crate for `core`"（实测 2026-09-30）。
        $wasmToolchain = "+1.88.0"
        $savedEap = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            rustup target add wasm32-unknown-unknown --toolchain 1.88.0 2>&1 | Out-Null
        }
        finally {
            $ErrorActionPreference = $savedEap
        }
        if ($LASTEXITCODE -ne 0) { throw "Install wasm32 target failed (toolchain 1.88.0)." }
        # 全量引擎 wasm（mlua wasm 移植 + wasm-bindgen）必须保留 `target_features` 自定义段
        # （含 reference-types）：wasm-bindgen-cli 靠该段检测模块特性，决定是否生成 externref
        # table 与 catch wrappers。workspace release profile 的 `strip = true` 会移除该段，
        # 导致打包时 wasm-bindgen 报 "externref table required for catch wrappers"（实测，
        # 见 artifacts/package-webgl-*.log 排查记录）。因此 wasm 构建临时用环境变量覆盖 strip
        # 为 none；桌面构建（assetc/game.exe）不受影响，仍按 workspace profile 走 strip。
        #
        # WP10 迁移（EH 兼容）：**两个 web 平台都必须用 rustc 1.88（LLVM 18）构建**。
        # 1.97（LLVM 19）对 vendored Lua C 的 setjmp 生成 try_table（new EH）、对 Rust 侧
        # catch_unwind 生成 legacy try（0x06），同一模块里新旧 EH 指令混合，V8 直接拒绝编译
        # （"module uses a mix of legacy and new exception handling instructions"，实测
        # 2026-09-29：1.88 产物 Edge 跑通、1.97 产物报混合）。1.88 产物为纯 try_table，
        # Edge/V8 支持。
        # 微信侧**同样**要用 1.88（2026-09-30 修正）：早期这里给微信留了默认 toolchain，理由是
        # "WXWebAssembly 对混合 EH 的接受度沿用阶段二发布基线"——那个前提来自 42 KB 的旧 wasm
        # （只有 start/memory，没有 mlua）。微信侧接进 mlua Lua 桥之后，1.97 产物在**本机 Node
        # 的 V8** 就报 `Invalid opcode 0x1f`（实测：错误落在 mlua 的函数里，wasm-opt 之前就已
        # 经坏了），于是 wasm 装不进来 → `__RUSTLUA_LUA_BRIDGE` 缺失 → boot=failed、0 帧。
        # 也就是说旧口径同时失去真机依据和本机可验证性，把发布期的硬缺陷推迟到提审之后。
        # 顺带证伪了"1.88 未优化产物太大"的担忧：binaryen 133 的 `-Oz` **会保留**纯 try_table
        # （2026-10-01 复测：3,151,173 → 2,743,359 字节，产物仍可被 V8 加载），所以微信侧照旧跑 -Oz 瘦身。
        #
        # 体积：workspace 的 release profile 是 `opt-level = "s"`，而微信主包 4 MB 门在 `s` + `-Oz`
        # 下实测差 47 KB（主包 4,143 KB）。`opt-level = "z"` 只给 wasm 构建临时开：同一份代码
        # `-Oz` 后 **2,743,359 → 2,424,516 字节（省 311 KB）**，`wasm_host_ok … exports=121
        # globals=0 bridge_exports=7` 照旧过。pc/editor 不受影响（不走这条分支），代码速度换体积
        # 这笔账只记在 web 平台头上。
        #
        # 体积（2026-10-03 复核）：profile 的 `lto = false` + `codegen-units = 16` 会把跨 crate 的
        # 死代码留在二进制里。`bc9ac8f Sprite Atlas 打包` 给 rustlua-assets 新挂 `image` 后，落包
        # wasm 从 2,436 KB（ff036cb）涨到 2,705 KB，主包随之逼近 4,096 KB 门——而打包侧能动的只有
        # 编译开关：同一条 wasm 分支再覆盖 `lto = "fat"` + `codegen-units = 1`，同一份代码 `-Oz`
        # + 剥 global 后 **2,729 → 2,330 KB**（本次发布产物实测 `pkg/game_web_bg.wasm` 2,347 KB），
        # `verify-wasm-host.mjs --platform=wechat-minigame` 仍报
        # `wasm_host_ok … exports=121 globals=0 bridge_exports=7`
        # （手工复现：`cd engine; $env:CARGO_PROFILE_RELEASE_LTO="fat";
        # $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS="1"` 后照本分支那三行环境变量编 game_web）。
        # 代价：wasm 构建从并行 16 单元退成单单元，本机增量约 100 s。EH 口径没动——仍是 1.88 产物的
        # 纯 try_table，binaryen 133 的 `-Oz` 照旧保留；这只在本机 V8 验过，真机 WXWebAssembly 要靠
        # scripts/verify/verify-wechat-minigame.ps1。
        # **纠正一条当时的结论**：那句"主包 4,269 KB 顶穿 4,096 KB"是用 `du -sk` 算的，对 700 个
        # 小文件虚高数百 KB；体积门看的是文件**字节和**（`wechat_minigame_package_ok` 那行），
        # 真实主包从未超限（旧读数 3,635 KB / 88.7%，docs/52 §6.1；10-06 内核搬分包后该口径失效，
        # 2026-10-12 复跑实测 2,571 KB / 62.8%，见 docs/52 §6.4）。
        $savedStrip = $env:CARGO_PROFILE_RELEASE_STRIP
        $env:CARGO_PROFILE_RELEASE_STRIP = "none"
        $savedOptLevel = $env:CARGO_PROFILE_RELEASE_OPT_LEVEL
        $env:CARGO_PROFILE_RELEASE_OPT_LEVEL = "z"
        $savedLto = $env:CARGO_PROFILE_RELEASE_LTO
        $env:CARGO_PROFILE_RELEASE_LTO = "fat"
        $savedCodegenUnits = $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS
        $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "1"
        $savedEap = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            # M73-00 WP5 第五片：`-Gpu2d` 时才把 wgpu 的 GL 后端与 `game_web` 的 GPU 宿主编进产物。
            # **默认关**：打开会让每个 web 产物都背上这段代码（体积账见 `engine/Cargo.toml`
            # 的 webgl 段），而 GPU 路径目前只在页面 URL 带 `?rustlua-gpu=1` 时才启用。
            $wasmFeatures = @()
            if ($Gpu2d) { $wasmFeatures = @("--features", "gpu2d") }
            # M81 批 3：api = "2d" 时关掉默认 feature（3d）；gpu2d 是渲染宿主开关，与维度无关，
            # 显式 --features 不会重开默认 feature，两条可叠加。
            if ($ApiDimension -eq "2d") { $wasmFeatures = @("--no-default-features") + $wasmFeatures }
            cargo $wasmToolchain build -p game_web --release --target wasm32-unknown-unknown @wasmFeatures -q 2>&1 | Out-Null
        }
        finally {
            $ErrorActionPreference = $savedEap
        }
        if ($null -eq $savedStrip) { Remove-Item Env:\CARGO_PROFILE_RELEASE_STRIP -ErrorAction SilentlyContinue }
        else { $env:CARGO_PROFILE_RELEASE_STRIP = $savedStrip }
        if ($null -eq $savedOptLevel) { Remove-Item Env:\CARGO_PROFILE_RELEASE_OPT_LEVEL -ErrorAction SilentlyContinue }
        else { $env:CARGO_PROFILE_RELEASE_OPT_LEVEL = $savedOptLevel }
        if ($null -eq $savedLto) { Remove-Item Env:\CARGO_PROFILE_RELEASE_LTO -ErrorAction SilentlyContinue }
        else { $env:CARGO_PROFILE_RELEASE_LTO = $savedLto }
        if ($null -eq $savedCodegenUnits) { Remove-Item Env:\CARGO_PROFILE_RELEASE_CODEGEN_UNITS -ErrorAction SilentlyContinue }
        else { $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = $savedCodegenUnits }
        if ($LASTEXITCODE -ne 0) {
            throw @"
WASM build failed ($Platform)。
上面看不到原因，是因为构建输出被 `| Out-Null` 吞了（PS 5.1 下 cargo 的进度走 stderr，
不吞就会把正常进度变成终止性错误）。手动复现看真相：
  cd engine; `$env:ZIG=<zig.exe>; cargo +1.88.0 build -p game_web --release --target wasm32-unknown-unknown
常见三种：缺 zig、1.88.0 这个 toolchain 没装 wasm32-unknown-unknown 目标、工作区处于半重构状态编不过。
"@
        }
        # wasm-bindgen CLI 的版本必须与 game_web 依赖的 wasm-bindgen crate **精确一致**
        # （事实源：engine/Cargo.lock，见 scripts/build/wasm-bindgen.ps1）。旧实现只判断
        # "PATH 上有没有 wasm-bindgen"，版本不对也照用，于是本机那份 0.2.127 一路走到
        # 胶水生成才炸（schema version 不匹配）。
        $wasmBindgenCli = Resolve-WasmBindgenCli -Provision
        if (-not $wasmBindgenCli) { throw "wasm-bindgen CLI（版本需与 Cargo.lock 的 wasm-bindgen 一致）解析失败。" }
    }
    else {
        # PC 发布只需要两个原生产物：`assetc.exe`（资源打包）与 `game.exe`（运行器）。
        # **不要**构建整个 workspace：`game_web` 等 web 侧 crate 与 PC 包无关，它编译失败
        # 时 PC 发布会被连带拦下（实测 2026-09-30：`--workspace` 因 game_web 的 JsValue
        # 报错而中断，而 game.exe/assetc.exe 本身是好的）。
        # 同 web 分支：PS 5.1 在 `$ErrorActionPreference='Stop'` 下会把 cargo 写到 stderr
        # 的正常进度变成终止性错误，必须临时降为 Continue 再调。
        $savedEap = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            # M81 批 3：api = "2d" 时 game 关掉默认 feature（3d）；assetc 与维度无关，单独构建。
            if ($ApiDimension -eq "2d") {
                cargo build -p assetc --release -q 2>&1 | Out-Null
                if ($LASTEXITCODE -ne 0) { throw "Release build failed (pc: assetc)." }
                cargo build -p game --release --no-default-features -q 2>&1 | Out-Null
            } else {
                cargo build -p assetc -p game --release -q 2>&1 | Out-Null
            }
        }
        finally {
            $ErrorActionPreference = $savedEap
        }
        if ($LASTEXITCODE -ne 0) { throw "Release build failed (pc: assetc+game)." }
    }

