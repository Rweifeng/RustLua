# -----------------------------------------------------------------------------
# 打包阶段：清单与说明（deps/build-info/README、多分包根）
# 自 scripts/build/package.ps1 原样迁出（原 L2057-L2296，2026-11-02 按功能拆分）。
# 执行语义：本文件被入口 dot-source，跑在入口的脚本作用域里——阶段之间共享变量、
#   ErrorActionPreference 与 throw 行为都跟拆分前一致。它不是函数，别当函数调。
#   PSScriptRoot 已统一换成入口设置的 packageScriptRoot（本文件不在 scripts/build/ 下）。
# -----------------------------------------------------------------------------

    # ---------------------------------------------------------------- 清单与说明
    $engineVersion = (Get-Content (Join-Path $engineRoot "Cargo.toml") | Select-String '^version = "0\.' | Select-Object -First 1).ToString().Trim().Split('=')[1].Trim().Trim('"')
    $toolchain = & rustc --version 2>$null
    # deps.txt 是**清单**，不是判据：cargo tree 在包缓存被别的 cargo 占用时会把
    # "Blocking waiting for file lock on package cache" 写到 **stderr**，而本脚本全局
    # `$ErrorActionPreference = "Stop"` 会把原生 stderr 当成**终止性错误** ⇒ 整个打包失败。
    # 后果不是"少一行清单"，而是调用方那一步 exit=1、**恢复/收尾没跑完**（产物被留在上一档，
    # 例如 gpu2d 内核）。与上面 binaryen 那处同一手法：只在这条命令上降级，之后立刻还原。
    $savedEapDeps = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        cargo tree -e normal --prefix none 2>$null | Sort-Object -Unique | Set-Content (Join-Path $distRoot "deps.txt") -Encoding UTF8
    } finally {
        $ErrorActionPreference = $savedEapDeps
    }
    $depsPath = Join-Path $distRoot "deps.txt"
    if ((-not (Test-Path $depsPath)) -or ((Get-Item $depsPath).Length -eq 0)) {
        # 不终止：清单落空要**看得见**，但不该把一次成功的打包判成失败（这正是上面那条注释的教训）。
        Write-Warning "deps.txt 为空或未生成（cargo tree 失败或被锁阻塞）：$depsPath"
    }

    if ($Platform -eq "wechat-minigame") {
        # 发布根 = <工程>/builds/<平台>/，里面 launcher/ 是首包、assets/ 是 CDN 资源包（仅 remote 形态）；
        # $distRoot 始终指向 launcher/，所以体积统计/必需文件检查都只看首包。
        $packageContents = "launcher/: game.js, game.json, project.config.json, wechat-adapter.js, pkg/ (game_web.js 胶水 + wasm 内核), pack/ (skeleton when thin), run.txt, deps.txt"
        if ($settingsPackaged) {
            $packageContents = "launcher/: game.js, game.json, project.config.json, wechat-adapter.js, pkg/ (game_web.js 胶水 + wasm 内核), pack/ (project.toml), run.txt, deps.txt"
        }
        if ($isSubpackagePack) {
            # 分包形态的主包 = launcher 自身：首屏资源在 mainpack/，其余在 pack/ 分包里。
            # wasm 内核本体也一起进分包（主包 4 MB 装不下它 + 2 MB 内容），主包只留胶水 pkg/game_web.js。
            $packageContents = "launcher/: game.js, game.json (subpackages=[pack-sub, pack-wasm]), pack.json, pack/ (首屏资源), pack-sub/ (非启动资源分包 root), pack-wasm/ (wasm 内核 root), project.config.json, wechat-adapter.js, pkg/game_web.js (胶水), run.txt, deps.txt"
        }
        if ($AssetPack -eq "remote") {
            $packageContents = "$packageContents; assets/pack-sub/** = full resource tree for CDN (重资源走远端 $RemotePackBaseUrl)"
        }
        if (-not [string]::IsNullOrWhiteSpace($RemotePackBaseUrl)) {
            $packageContents = "remote-assets.json (远端回落), $packageContents"
        }
    }
    elseif ($Platform -eq "webgl") {
        $packageContents = "index.html, test-webgl.bat, pkg/game_web.js, pkg/game_web_bg.wasm, pack/ (data), run.txt, deps.txt"
        if ($settingsPackaged) {
            $packageContents = "index.html, test-webgl.bat, pkg/game_web.js, pkg/game_web_bg.wasm, pack/ (data, project.toml), run.txt, deps.txt"
        }
    }
    else {
        # PC 不复制 17 MB 完整字体到发布根：assetc 已把工程字体打进 pack/（运行时以 pack 为工程根，
        # 按 project.toml 的 font 路径读取）。webgl 同样不随包带字体（走浏览器系统字体，
        # `__FONT_PATH__` 为空；见上方 webgl 分支）。只有微信走打包期子集字体（247 KB）。
        # 这条**真的去查**包内字体，而不是只在 build-info.txt 里声称一句——此前那行 `font_check`
        # 对 pc 写的是"fonts/$fontName present and non-empty"，而 pc 的发布根根本没有 fonts/：
        # 自述与现实相反，读的人无从分辨（验收那边也照着这句写成了根下字体，于是正确的 pc 发布
        # 过不了自己的门）。
        $packedPcFont = Join-Path $packRoot ("assets\fonts\" + $fontName)
        if (-not (Test-Path -LiteralPath $packedPcFont)) {
            throw "PC package requires the project font inside pack/: $packedPcFont"
        }
        if ((Get-Item -LiteralPath $packedPcFont).Length -le 0) {
            throw "PC packaged font is empty: $packedPcFont"
        }
        $packageContents = "game.exe, pack/ (data, assets/fonts/$fontName), run.txt, deps.txt"
        if ($settingsPackaged) {
            $packageContents = "game.exe, pack/ (data, project.toml$(if ($releaseMarked) { ', release=true' }), assets/fonts/$fontName), run.txt, deps.txt"
        }
    }

    # 子集字体在包内的相对路径随形态变（分包形态下字体属于首屏资源，已被切到 mainpack/）。
    $wechatFontRelative = if ($isSubpackagePack) { "pack/assets/fonts/$fontName" } else { "$packDirName/assets/fonts/$fontName" }
    @(
        "RustLua game engine release package",
        "publish_name: $PublishName",
        "build_time: $timestamp",
        "platform: $Platform",
        "engine version: $engineVersion",
        "toolchain: $toolchain",
        "contents: $packageContents",
        "font_check: $(if ($Platform -eq 'wechat-minigame') { "$wechatFontRelative (subset, packaged into launcher/)" } elseif ($Platform -eq 'webgl') { "fonts/$fontName present and non-empty (index.html 的字体路径指向它)" } else { "pack/assets/fonts/$fontName present and non-empty（发布根不带完整字体）" })",
        "settings_check: $(if ($settingsPackaged) { "project.toml packaged from project root$(if ($releaseMarked) { ' + release = true marker read back from pack/project.toml' } else { '（无 release 标记：webgl/微信宿主没有文件系统，按包内工程根读取）' })；微信形态按白名单改名成 project.toml.txt" } else { 'no [input]/[audio] in project.toml; engine defaults apply' })",
        "player_data: $(if ($Platform -eq 'wechat-minigame') { 'wx storage / USER_DATA_PATH; the code package is read-only' } elseif ($Platform -eq 'webgl') { 'browser storage; no filesystem player data' } else { "$PlayerDataDir (override: RUSTLUA_USER_DIR); pack/ is read-only and never written" })"
    ) | Set-Content (Join-Path $distRoot "build-info.txt") -Encoding UTF8

    if ($Platform -eq "wechat-minigame") {
        # 发布根下有几块随形态变：subpackage 只有 launcher/（首包 + 分包），remote 还多一个
        # assets/（上传 CDN 的资源包）。第 0 条必须按形态写清楚，否则使用者会把整个发布根传上去。
        $wechatRootLine = if ($isSubpackagePack) {
            "0) 发布根下只有 launcher/：它同时是**主包**与**分包**的宿主，整目录一次上传微信；不需要服务器、备案或 downloadFile 域名白名单"
        }
        else {
            "0) 发布根下有两块：launcher/ = 首包（上传微信 / 导入开发者工具的就是它），assets/ = 资源包（整目录上传 CDN）"
        }
        $wechatAppidLine = if ($WechatAppId -eq "touristappid") {
            "2) 未配置 appid：launcher/project.config.json 里是 touristappid，只能本地预览。上传前在编辑器「发布项目」面板填微信 appid 重新发布（或直接改这个文件）"
        } else {
            "2) appid 已由发布脚本写进 launcher/project.config.json（$WechatAppId），可直接上传；换小程序主体时改面板里的 appid 再发布一次"
        }
        $wechatRunLines = @(
            "Run instructions (WeChat Mini Game)",
            $wechatRootLine,
            "1) 打开微信开发者工具 -> 导入项目 -> 选择本目录下的 launcher（已含 game.json / project.config.json）",
            $wechatAppidLine,
            "3) 手机预览：左半屏为虚拟摇杆（WASD），右半屏为动作键（space）；微信 PC 端同时支持键盘"
        )
        if ($isSubpackagePack) {
            $wechatRunLines += "4) launcher/pack-sub/ 是**分包**（game.json 的 subpackages[0].root）：目录下所有文件都进分包，主包里一个都不留；root 下必须有 pack-sub/game.js 入口，缺了开发者工具会拒绝打开工程"
            $wechatRunLines += "5) 主包侧的首屏资源在 launcher/pack/（与 pack/ 同级的兄弟目录），路由依据是 launcher/pack.json：清单内的走主包，其余走分包。下载一次永久缓存"
            $wechatRunLines += "5b) **wasm 内核本体（pack-wasm/game_web_bg.wasm）在独立分包根里**（主包 4 MB 装不下它 + 2 MB 内容）：启动次序是「先等 pack-wasm 挂载 → JS 画进度条 → 实例化 wasm → 出标题画面」，pack-sub/ 资源根在 boot 后后台挂载。胶水 pkg/game_web.js 在主包（它被 require 同步加载，进分包要换一套 require 语义）"
            $wechatRunLines += "6) 字体使用首屏子集字体（assetc 子集化产物，已随首屏切到 pack/assets/fonts/），完整字体不随包分发以控制体积"
            $wechatRunLines += "7) 首包里不含 PC/WebGL 入口（game.exe / index.html），不要混用发布目录"
            $wechatRunLines += "8) 首次导入开发者工具后**必须**核对「详情 -> 代码包大小」面板：主包 / 分包两块数值应与本次输出行的 main_kb / subpackage_kb 一致（本机 harness 无法证明微信的真实打包语义，这里是唯一能证伪的地方）"
        }
        elseif (-not $wasmInSubpackage) {
            $wechatRunLines += "4) launcher/pack/ 不是分包（game.json 不含 subpackages）：它是主包内的普通目录，运行时直接读取，不需要 wx.loadSubpackage，也没有分包入口文件"
            $wechatRunLines += "5) 字体使用 pack/ 内的子集字体（assetc 子集化产物），完整字体不随包分发以控制体积"
            $wechatRunLines += "6) 首包里不含 PC/WebGL 入口（game.exe / index.html），不要混用发布目录"
        }
        else {
            # remote + -WasmInSubpackage：分包里只有内核与骨架，重资源在 CDN。
            $wechatRunLines += "4) launcher/pack-sub/ **是分包**（game.json 的 subpackages[0].root，root 下有 pack-sub/game.js 入口）：里面只有 wasm 内核与骨架（manifest.txt/chars.txt/project.toml.txt），重资源全在 CDN"
            $wechatRunLines += "5) **内核在分包里**：启动时适配层先 wx.loadSubpackage('pack') 再实例化 wasm，标题画面与下载进度都排在分包之后（进度界面是 JS 侧画的，不依赖 wasm/Lua）"
            $wechatRunLines += "6) 字体使用 pack/ 内的子集字体（assetc 子集化产物），完整字体不随包分发以控制体积"
            $wechatRunLines += "7) launcher/pack.json 是**空清单 + layout.subpackage_root=pack**：适配层靠它判断内核在分包；首屏资源没有包内路由（都在 CDN）"
            $wechatRunLines += "8) 首包里不含 PC/WebGL 入口（game.exe / index.html），不要混用发布目录"
        }
        # 这里必须写**最终**目录（$finalRoot）：run.txt 是在 staging 里生成的，
        # 而原子交换会把 staging 改名/删除，写 staging 路径等于写一个随后就不存在的目录。
        $wechatRunLines += "9) 无开发者工具时的运行时验证：powershell -NoProfile -File scripts\verify\verify-wechat-minigame.ps1 -BundleRoot `"$finalRoot\launcher`""
        if ($AssetPack -eq "remote") {
            $wechatRunLines += "10) 本包是**瘦首包**：launcher/pack/ 只含骨架（manifest.txt/chars.txt 原名，project.toml 因后缀不在微信上传白名单而改名为 project.toml.txt），重资源从 $RemotePackBaseUrl 下载，按 pack/manifest.txt 校验后缓存到 USER_DATA_PATH"
            $wechatRunLines += "11) 上传顺序：先把 assets/ 整份传到 CDN（使 packBaseUrl 可用），再上传 launcher/ 作为代码包；两侧不一致时校验和会拒绝下载（不会静默用错资源）"
            $wechatRunLines += "12) 运行时验证要带上资源包：scripts\verify\verify-wechat-minigame.ps1 -BundleRoot `"$finalRoot\launcher`" -AssetPackRoot `"$finalRoot\assets`""
            $wechatRunLines += "13) 本地预览（不用自己起服务）：双击 scripts\dev\serve-wechat-pack.bat，再按上面的方式导入 launcher/"
        }
        $wechatRunLines | Set-Content (Join-Path $distRoot "run.txt") -Encoding UTF8
    }
    elseif ($Platform -eq "webgl") {
        @(
            "Run instructions",
            "1) Serve this directory with an HTTP server, for example: node <repo>\scripts\dev\serve-static.js <this dir> 8000",
            "2) Open http://localhost:8000/index.html in a WebGL2-capable browser",
            "3) Do not open index.html directly from file:// because browsers block WASM/resource fetches",
            "4) The page title and browser tab title use publish_name from project.toml",
            "5) Windows: double-click test-webgl.bat to verify the bundle and serve it on http://127.0.0.1:8137/",
            "   (it finds the repository root by itself; outside a checkout, serve the directory manually as in 1)"
        ) | Set-Content (Join-Path $distRoot "run.txt") -Encoding UTF8
    }
    else {
        @(
            "Run instructions",
            "1) Double-click game.exe, or run: game.exe --pack pack --title `"$PublishName`"",
            "2) Controls: WASD move, SPACE attack, F1 save, F2 load, F4 switch scene",
            "3) F3 toggles the developer fps/latency overlay (off by default in release builds)",
            "4) Logs and crash reports go to the player data directory: $PlayerDataDir\logs",
            "   (a release pack never writes into its own directory, so a read-only install is fine)",
            "5) This package has no source or toolchain dependency and runs without Rust/IDE",
            "6) Player data (savegames and settings) is written to: $PlayerDataDir",
            "   The pack/ directory is read-only for the runner and never receives player data;",
            "   set RUSTLUA_USER_DIR to relocate the player data directory (used by tests and portable installs)"
        ) | Set-Content (Join-Path $distRoot "run.txt") -Encoding UTF8
    }

    if ($MultiSub) {
        # **在原子交换之前**拆根：apply 失败时最终发布目录完全没被碰过（staging 由 catch 清理），
        # 不会留下"拆了一半"的 release。拆完再测体积，4 MB 门看的才是最终 mainpack。
        $layoutScript = Join-Path $packageScriptRoot "apply-bundle-layout.ps1"
        if (-not (Test-Path -LiteralPath $layoutScript)) { throw "缺少 $layoutScript" }
        $layoutArgs = @{ Launcher = $distRoot }
        if ($assetc) { $layoutArgs["Assetc"] = $assetc }
        # remote 形态：cdn 组的容器发布在 CDN 镜像目录里（本次发布的 staging），闭包要按它核对。
        if ($script:cdnMirrorStage) { $layoutArgs["CdnDir"] = $script:cdnMirrorStage }
        & $layoutScript @layoutArgs
        Write-Host "==> 多分包根已应用（-MultiSub，staging 内）" -ForegroundColor Yellow
        # run.txt 在第 2040 行之前已生成；拆根后补一条实际根名，避免文档还写"单 pack"。
        $gameConfigForRun = Get-Content -LiteralPath (Join-Path $distRoot "game.json") -Raw -Encoding UTF8 | ConvertFrom-Json
        $rootNames = @($gameConfigForRun.subpackages | ForEach-Object { [string]$_.root })
        $runPath = Join-Path $distRoot "run.txt"
        if (Test-Path -LiteralPath $runPath) {
            $line = "13) 多分包根（-MultiSub）：$($rootNames -join ', ')；容器按 bundle_plan.json 分布在这些根里，适配层 boot 前全部挂载"
            [System.IO.File]::AppendAllText($runPath, "`r`n" + $line, (New-Object System.Text.UTF8Encoding($false)))
        }
    }

    # 微信小游戏：收尾处（run.txt/deps.txt/build-info.txt 都写完之后）重新测量并输出最终体积。
    # 这一步不能省：微信上传时这些说明文件也计入主包，早期只在生成它们之前测过一次，
    # 会把主包少算 ~9 KB（实测 main_kb=303 而最终目录是 312）——守卫必须盯着最终产物。
    if ($Platform -eq "wechat-minigame") {
        $wechatFinalTotal = (Get-ChildItem -LiteralPath $distRoot -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
        # 多根布局下"主包 = launcher − 全部分包根"，不能只减 pack/（那会把 pack-<name>/ 错算进主包）。
        $wechatFinalPack = Measure-WeChatSubpackageBytes -LauncherRoot $distRoot
        $wechatFinalVolume = Assert-WeChatPackageVolume -TotalBytes $wechatFinalTotal -PackBytes $wechatFinalPack `
            -Form $(if ($wasmInSubpackage) { "subpackage" } else { $AssetPack }) -Suffix "（本值是权威值：run.txt/deps.txt/build-info.txt 都已写入发布目录）"
        Write-Host ("wechat_minigame_package_ok files={0} main_kb={1:N0} main_of_limit={2:P1} subpackage_kb={3:N0} subpackage_of_limit={4:P1} pack_kb={5:N0} total_kb={6:N0} total_of_limit={7:P1} asset_pack={8} split={9}" -f `
            $requiredWeChatFiles.Count, ($wechatFinalVolume.Main / 1KB), ($wechatFinalVolume.Main / 4MB), `
            $(if ($wasmInSubpackage) { $wechatFinalVolume.Pack / 1KB } else { 0 }), `
            $(if ($wasmInSubpackage) { $wechatFinalVolume.Pack / 20MB } else { 0 }), `
            ($wechatFinalPack / 1KB), ($wechatFinalVolume.Total / 1KB), ($wechatFinalVolume.Total / 20MB), $AssetPack, `
            $(if ($isSubpackagePack) { $(if ($MultiSub) { "pack+multi" } else { "pack+pack-sub" }) } elseif ($wasmInSubpackage) { "wasm+pack" } else { "none" })) -ForegroundColor Green
    }

    # 瘦首包（`-AssetPack remote`）的容器闭包：**必须在最终形态上、对着资源包核**。
    # 上一段的瘦身已经把 `pack/` 里的资源目录整体删掉，此刻包内一个容器都没有；而资源包
    # （`<发布根>/assets/<packDirName>/…`，整目录传 CDN）里才是真正要发布的容器。
    # 不在这里核 = 瘦首包形态的容器内容从来没被任何门看过（合成夹具又都长在包内）。
    if ($Platform -eq "wechat-minigame" -and $AssetPack -eq "remote" -and $assetPackStage) {
        $closureAssetc = if ($assetc) { $assetc } else { Join-Path $nativeTargetRoot "assetc.exe" }
        if (-not (Test-Path -LiteralPath $closureAssetc)) {
            throw ("容器闭包检查需要 assetc（找不到 $closureAssetc）：瘦首包的容器全在资源包里，这一门不允许跳过")
        }
        $plannedBundlesForClosure = @()
        $planForClosure = Join-Path $packRoot "bundle_plan.json"
        if (Test-Path -LiteralPath $planForClosure) {
            $plannedBundlesForClosure = @((Get-Content -LiteralPath $planForClosure -Raw -Encoding UTF8 |
                        ConvertFrom-Json).groups | ForEach-Object { [string]$_.bundle } | Where-Object { $_ })
        }
        $exportRootForClosure = ""
        foreach ($candidate in @((Join-Path $assetPackStage $packDirName), $assetPackStage)) {
            if ([string]::IsNullOrWhiteSpace($candidate) -or -not (Test-Path -LiteralPath $candidate)) { continue }
            $complete = $plannedBundlesForClosure.Count -gt 0
            foreach ($rel in $plannedBundlesForClosure) {
                $probe = Join-Path $candidate $rel
                if (-not (Test-Path -LiteralPath $probe) -and -not (Test-Path -LiteralPath ($probe + ".txt"))) {
                    $complete = $false
                    break
                }
            }
            if ($complete) { $exportRootForClosure = $candidate; break }
        }
        if ([string]::IsNullOrWhiteSpace($exportRootForClosure)) {
            throw ("瘦首包的资源包里找不齐计划声明的 {0} 个容器（在 {1} 下）：资源包导出不完整就是远端 404" -f $plannedBundlesForClosure.Count, $assetPackStage)
        }
        $closureOutputForThin = & $closureAssetc closure $distRoot --cdn $exportRootForClosure 2>&1
        $closureExitForThin = $LASTEXITCODE
        if ($closureExitForThin -ne 0) {
            throw ("瘦首包容器闭包检查失败（assetc closure exit {0}）: {1}" -f $closureExitForThin, (($closureOutputForThin | ForEach-Object { "$_" }) -join "`n"))
        }
        Write-Host (($closureOutputForThin | Where-Object { $_ -match '^assetc_closure_ok' } | Select-Object -First 1)) -ForegroundColor Green
    }

    # M20-03：全部内容与自检通过后，原子替换最终目录（旧版本备份→移入新版本
    # →清理备份）。交换失败会回滚旧版本并抛错，staging 保留以便诊断。
