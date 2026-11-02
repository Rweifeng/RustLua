# -----------------------------------------------------------------------------
# 打包阶段：工具函数（wasm 优化与降级、JS 压缩、体积断言、清单校验）
# 自 scripts/build/package.ps1 原样迁出（原 L231-L463，2026-11-02 按功能拆分）。
# 执行语义：本文件被入口 dot-source，跑在入口的脚本作用域里——阶段之间共享变量、
#   ErrorActionPreference 与 throw 行为都跟拆分前一致。它不是函数，别当函数调。
#   PSScriptRoot 已统一换成入口设置的 packageScriptRoot（本文件不在 scripts/build/ 下）。
# -----------------------------------------------------------------------------

function Optimize-WasmBin {
    param(
        [Parameter(Mandatory = $true)][string]$WasmPath
    )
    # 用 binaryen 的 wasm-opt -Oz 对 wasm-bindgen 产物做体积优化（2026-10-01 实测
    # 2,853,068 → 2,424,558 字节，顺带移除 name section）。微信小游戏主包 4 MB 上限需要它；
    # webgl 亦受益。
    # **必须锁 binaryen 133**：其它版本的 pass 可能把 1.88 产物的纯 try_table 改回 legacy
    # try，造成新旧 EH 混合、V8 拒绝编译（见 docs/52 R11）。版本校验在
    # scripts/build/minify-toolchain.ps1 里做，这里只消费；解析不到（例如离线）才降级为警告。
    if (-not (Test-Path -LiteralPath $WasmPath)) { return }
    $wasmOptPath = Resolve-WasmOpt -Provision
    if (-not $wasmOptPath) {
        Write-Warning "wasm-opt（binaryen 133）未找到：$WasmPath 保持未优化体积（微信小游戏主包很可能超 4 MB）。跑 scripts\build\minify-toolchain.ps1 自动装，或设 `$env:WASM_OPT。"
        return
    }
    # **第 53 轮修**：binaryen 会**拒绝**含 bulk-memory / EH 的模块，除非显式允许（1.88 的
    # mlua+引擎产物里 `memory.copy`/`memory.fill` 与 tag 段都有）。这里原来只跑一次 `-Oz`，
    # 失败就只留一行警告 ⇒ 产物**带着 debug/name 段**发布（实测 4.3 MB，其中约 2.8 MB 是
    # 自定义段），主包体积白白多算一倍。现在：先按原样跑（保持既有行为与既有读数字面量），
    # 失败再带特性开关重试；两次都失败才降级为"未优化发布"，并留下日志路径。
    # 首次失败会把 binaryen 的 validator 长文打到 stderr（可达数 MB），所以两次尝试都重定向到
    # 临时文件：成功即删，失败才把路径报出来，发布日志保持可读。
    $wasmFeatureArgs = @(
        "--enable-bulk-memory",
        "--enable-bulk-memory-opt",
        "--enable-nontrapping-float-to-int",
        "--enable-reference-types",
        "--enable-multivalue",
        "--enable-sign-ext",
        "--enable-exception-handling",
        "--enable-mutable-globals",
        "--enable-simd"
    )
    $optOut = $WasmPath + ".opt"
    $optLog = [IO.Path]::GetTempFileName()
    $optRetried = $false
    # **必须临时把 EAP 降成 Continue**：`*>` 会把 native stderr 转成 ErrorRecord，而脚本顶上是
    # `$ErrorActionPreference = 'Stop'` ⇒ 首次失败时 binaryen 的 validator 长文会直接终止发布
    # （实测踩过：日志停在重定向这一行，连重试都没跑到）。同一手法在下面的 node 校验段已有先例。
    $savedEapOpt = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $wasmOptPath -Oz $WasmPath -o $optOut *> $optLog
        $optExit = $LASTEXITCODE
        if ($optExit -ne 0) {
            Remove-Item -LiteralPath $optOut -Force -ErrorAction SilentlyContinue
            $optRetried = $true
            & $wasmOptPath -Oz @wasmFeatureArgs $WasmPath -o $optOut *> $optLog
            $optExit = $LASTEXITCODE
        }
    }
    finally {
        $ErrorActionPreference = $savedEapOpt
    }
    if ($optExit -eq 0) {
        Move-Item -Force -LiteralPath $optOut -Destination $WasmPath
        Remove-Item -LiteralPath $optLog -Force -ErrorAction SilentlyContinue
        Write-Host ("==> wasm-opt -Oz{0}: {1:N0} KB" -f `
            $(if ($optRetried) { "（带 bulk-memory/EH 特性开关重试后成功）" } else { "" }), `
            ((Get-Item -LiteralPath $WasmPath).Length / 1KB)) -ForegroundColor DarkGray
    }
    else {
        Remove-Item -LiteralPath $optOut -Force -ErrorAction SilentlyContinue
        Write-Warning "wasm-opt 处理失败（$WasmPath），按未优化体积继续发布。binaryen 输出见 $optLog"
    }
}
function Downgrade-WasmNoExtern {
    param(
        [Parameter(Mandatory = $true)][string]$WasmPath
    )
    # 把 `ref.null noextern`(D0 72) 降级成扁平的 `ref.null extern`(D0 6F)：真机 WXWebAssembly 不认
    # GC 终版 typed null（实测 `Unknown heap type -14`，-14 == noextern），本机 V8/开发者工具却都认，
    # 于是门全绿也照样真机白屏。用 binaryen 133 `wasm-opt --disable-gc` 重编码，实测只改 6 字节、
    # 大小不变、Node V8 编译通过（首个改动偏移 == 真机报错 @+1486867）。
    # 完整理由、两个反例（--all-features 写非法 import kind、自动检测因 target-features 已剥而退回
    # MVP）、显式特性清单、以及"大小不变 + 每处改动都是 0x72→0x6f"的字节级校验，全在
    # scripts/lib/wasm-lua/downgrade-noextern.mjs 里（那里做校验比 PS 逐字节循环快几个数量级）。
    if (-not (Test-Path -LiteralPath $WasmPath)) { return }
    $wasmOptPath = Resolve-WasmOpt -Provision
    if (-not $wasmOptPath) {
        throw "降级 noextern 需要 wasm-opt（binaryen 133）：$WasmPath 仍带 heap type -14，真机会 Unknown heap type -14。跑 scripts\build\minify-toolchain.ps1 自动装，或设 `$env:WASM_OPT。"
    }
    $downgradeTool = Join-Path $packageScriptRoot "..\lib\wasm-lua\downgrade-noextern.mjs"
    if (-not (Test-Path -LiteralPath $downgradeTool)) {
        throw "缺少 noextern 降级脚本: $downgradeTool（发布不能在没有它的情况下继续）"
    }
    $nodeForDowngrade = Get-Command node -ErrorAction SilentlyContinue
    if (-not $nodeForDowngrade) { throw "noextern 降级需要 node" }
    $savedEapDown = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $downOutput = & $nodeForDowngrade.Source $downgradeTool $wasmOptPath $WasmPath 2>&1
        $downExit = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $savedEapDown
    }
    if ($downExit -ne 0) {
        throw ("noextern→extern 降级失败（{0}）：{1}" -f `
            $WasmPath, (($downOutput | ForEach-Object { "$_" }) -join ' '))
    }
    Write-Host (($downOutput | Where-Object { $_ -match '^downgrade_noextern_ok' } | Select-Object -First 1)) -ForegroundColor DarkGray
}
function Minify-JsFile {
    param(
        [Parameter(Mandatory = $true)][string]$JsPath
    )
    # 用 terser 压缩工程自维护的发布期 JS（**只剩 `wechat-adapter.js`** 这一份：旧世界
    # `web-runtime.js` 已随 B8 删除）：
    # 发布产物更小（合计省 ~180 KB，微信小游戏主包 4 MB 上限需要），源码保持可读。
    # 解析与按需安装（npm --prefix 到 gitignore 的 engine\.tools\terser）都在
    # scripts/build/minify-toolchain.ps1；没有 npm 的机器才降级为警告。
    if (-not (Test-Path -LiteralPath $JsPath)) { return }
    $terserPath = Resolve-Terser -Provision
    if (-not $terserPath) {
        Write-Warning "terser 未找到：$JsPath 保持未压缩（微信小游戏主包可能超 4 MB）。跑 scripts\build\minify-toolchain.ps1 自动装，或设 `$env:TERSER。"
        return
    }
    $minOut = $JsPath + ".min"
    & $terserPath $JsPath -o $minOut
    if ($LASTEXITCODE -eq 0) {
        Move-Item -Force -LiteralPath $minOut -Destination $JsPath
        Write-Host ("==> terser minify: {0:N0} KB" -f ((Get-Item -LiteralPath $JsPath).Length / 1KB)) -ForegroundColor DarkGray
    }
    else {
        Remove-Item -LiteralPath $minOut -Force -ErrorAction SilentlyContinue
        Write-Warning "terser 压缩失败（$JsPath），按原样继续。"
    }
}
function Invoke-PackagedCheck {
    param(
        [Parameter(Mandatory = $true)][string]$Runner,
        [Parameter(Mandatory = $true)][string]$PackRoot
    )
    $stdoutFile = Join-Path $env:TEMP ("rustlua-pack-check-" + [guid]::NewGuid().ToString("N") + ".out")
    $stderrFile = $stdoutFile + ".err"
    try {
        # `Start-Process -ArgumentList` 会把数组按空格拼成命令行且**不加引号**，因此含空格的
        # 路径必须自己带引号（自测就是故意用 "rustlua release test …" 这种带空格的路径）。
        $quotedPack = '"' + $PackRoot + '"'
        $process = Start-Process -FilePath $Runner `
            -ArgumentList @("--pack", $quotedPack, "--check") `
            -PassThru -Wait -NoNewWindow `
            -RedirectStandardOutput $stdoutFile -RedirectStandardError $stderrFile
        $lines = @()
        if (Test-Path -LiteralPath $stdoutFile) { $lines += Get-Content -LiteralPath $stdoutFile }
        if (Test-Path -LiteralPath $stderrFile) { $lines += Get-Content -LiteralPath $stderrFile }
        return [pscustomobject]@{ ExitCode = $process.ExitCode; Output = $lines }
    }
    finally {
        Remove-Item -LiteralPath $stdoutFile -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $stderrFile -Force -ErrorAction SilentlyContinue
    }
}

# 微信小游戏三种形态的体积配额不同，断言也必须不同：
#   subpackage：主包 = launcher 总量 − pack/（分包），断言主包 4MB / 单个普通分包不设 4MB 门 / 总计 20MB
#   （官方 2023-10 起：主包 ≤ 4M、单个普通分包不限制大小、总量 ≤ 20M/30M，见 docs/52 与 package.ps1 体积门）
#   full      ：pack/ 也在主包里，整个包都算主包 4MB（没有分包配额可用）
#   remote    ：pack/ 只剩骨架，主包 4MB；重资源在 CDN 上不占包体配额
# 超限时的**出路**必须写进报错里，否则使用者只看到"超了"却不知道该往哪走。
# 返回 [pscustomobject]@{ Main; Pack; Total }（字节），供调用方打印同一套口径。
function Assert-WeChatPackageVolume {
    param(
        [Parameter(Mandatory = $true)][double]$TotalBytes,
        [Parameter(Mandatory = $true)][double]$PackBytes,
        [Parameter(Mandatory = $true)][string]$Form,
        [string]$Suffix = ""
    )
    # 报文里的 $Suffix 用 `+` 拼接、不占 `-f` 的位置索引：这几条 throw 在日常发布里**走不到**
    # （分包 73.5%、瘦首包 11.7%），占位符写错只能等哪天真超限时才炸——那种守卫比没有守卫更糟。
    # **坑（实测）**：PowerShell 里 `+` 的结合力**高于** `-f`，所以 `"A" + "B" -f 1` 等价于
    # `"A" + ("B" -f 1)`——A 里的 `{0}` 会原样留在报文里，守卫变成一句没人看得懂的字面量。
    # 必须先把整条 `+` 表达式括起来再 `-f`；下面每条都显式括了双层。
    $mainBytes = if ($Form -eq "subpackage") { $TotalBytes - $PackBytes } else { $TotalBytes }
    if ($Form -eq "subpackage") {
        if ($mainBytes -gt 4MB) {
            # 出路 ① 的方向**必须写对**：主包 = launcher − pack/，而 mainpack/（首屏资源）是**算进主包**的，
            # 所以"把更多资源列进首屏清单"是加大主包的那一手——曾经就这么写反过，超限的人照着做只会更超。
            throw ((("微信小游戏主包超限: {0:N0} KB > 4,096 KB（主包 = launcher 总量 {1:N0} KB − 分包 pack/ {2:N0} KB）。" +
                "出路（按优先级）：① **收窄**首屏清单（--include 前缀去掉非首屏目录、或让启动场景少引用资源）——" +
                "被排除的资源留在 pack/ 分包里，主包变小、分包变大；注意列进清单是**反方向**，会让主包更大。" +
                "② 改用 -AssetPack remote -RemotePackBaseUrl <CDN> 把重资源移出包内。" +
                "（pack/ 里 Lua 的注释已由 scripts/build/strip-lua-comments.mjs 在构建期剥掉 ~283 KB，" +
                "剩下的基本是真内容；编译侧 opt-level=z + lto=fat + codegen-units=1 + terser 也已全部生效。）") -f `
                ($mainBytes / 1KB), ($TotalBytes / 1KB), ($PackBytes / 1KB)) + $Suffix)
        }
        # 单个普通分包：官方 2023-10 起不再限 4MB（微信官方《分包加载》：主包 ≤ 4M、
        # 单个普通分包不限制大小、总量 ≤ 20M/30M）。此处保留**总量**门（下方 20MB 那条），
        # 单分包不再单独设 4MB 门——iOS 备用 wav 音频（InnerAudioContext 在 iOS 不支持 ogg）
        # 需要进分包，旧 4MB 门会误杀合规产物。
        if ($PackBytes -gt 20MB) {
            throw ((("微信小游戏单个分包过大: {0:N0} KB > 20,480 KB（pack/ 是 subpackages[0].root，目录下所有文件都计入）。" +
                "出路（按优先级）：① 把更多资源列进首屏清单搬去 mainpack/；② 拆第二个分包；" +
                "③ 改用 -AssetPack remote -RemotePackBaseUrl <CDN>。") -f ($PackBytes / 1KB)) + $Suffix)
        }
        # 总量门（官方：主包 + 全部分包之和 ≤ 20MB；开通虚拟支付后可到 30MB，脚本按无内购口径
        # 保守取 20MB）。主包 4MB 门在上面独立生效，单个普通分包不限大小（见上一条的注释）。
        if ($TotalBytes -gt 20MB) {
            throw (("微信小游戏代码包总量超限: {0:N0} KB > 20,480 KB（主包 + 全部分包之和）。" -f ($TotalBytes / 1KB)) + $Suffix)
        }
    }
    elseif ($mainBytes -gt 4MB) {
        throw ((("微信小游戏主包超限: {0:N0} KB > 4,096 KB（-AssetPack {1}：pack/ 不是分包，整个包都算主包；当前 pack/ {2:N0} KB）。" +
            "出路：改用 -AssetPack subpackage（主包 + 单分包，微信原生，零服务器零备案零域名白名单），" +
            "或 -AssetPack remote -RemotePackBaseUrl <CDN> 把重资源移出包内。") -f `
            ($mainBytes / 1KB), $Form, ($PackBytes / 1KB)) + $Suffix)
    }
    return [pscustomobject]@{ Main = $mainBytes; Pack = $PackBytes; Total = $TotalBytes }
}

# 微信 launcher 里所有**分包根**的字节和：单根时就是 pack/，多根时是 pack/ + pack-<name>/。
# 主包 = launcher 总量 − 这个和；只减 pack/ 会把具名根错算进主包（4 MB 门的方向就反了）。
function Measure-WeChatSubpackageBytes {
    param([Parameter(Mandatory = $true)][string]$LauncherRoot)
    $sum = 0
    $gameFile = Join-Path $LauncherRoot "game.json"
    if (Test-Path -LiteralPath $gameFile) {
        try {
            $game = Get-Content -LiteralPath $gameFile -Raw -Encoding UTF8 | ConvertFrom-Json
            foreach ($sub in @($game.subpackages)) {
                if (-not $sub -or [string]::IsNullOrWhiteSpace([string]$sub.root)) { continue }
                $dir = Join-Path $LauncherRoot ([string]$sub.root).TrimEnd('/')
                if (Test-Path -LiteralPath $dir) {
                    $sum += (Get-ChildItem -LiteralPath $dir -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
                }
            }
        } catch { return 0 }
    }
    return $sum
}

