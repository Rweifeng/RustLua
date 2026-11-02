# -----------------------------------------------------------------------------
# 打包阶段：平台产物校验（微信/WebGL 骨架与体积）
# 自 scripts/build/package.ps1 原样迁出（原 L1839-L2056，2026-11-02 按功能拆分）。
# 执行语义：本文件被入口 dot-source，跑在入口的脚本作用域里——阶段之间共享变量、
#   ErrorActionPreference 与 throw 行为都跟拆分前一致。它不是函数，别当函数调。
#   PSScriptRoot 已统一换成入口设置的 packageScriptRoot（本文件不在 scripts/build/ 下）。
# -----------------------------------------------------------------------------

    # ---------------------------------------------------------------- 平台产物校验
    if ($Platform -eq "wechat-minigame") {
        # 骨架的位置随形态变：manifest.txt / project.toml 都在首屏清单里，所以分包形态下
        # 它们已经被搬到 mainpack/，pack/ 里**不该**再有同名副本（有就说明切分漏了）。
        # 名字一律用**宿主文件名**（改名已在上一步完成：project.toml → project.toml.txt）。
        $skeletonRoot = if ($isSubpackagePack) { $mainPackRoot } else { $packRoot }
        # wasm 内核的位置同样随形态变（判据与上面同源，都是 $isSubpackagePack，两处一起改才不会漂移）：
        # 分包形态下它是 pack/ 分包根里的那个文件，主包只留胶水。
        $wasmCoreRelative = if ($wasmInSubpackage) { "$packWasmDirName\game_web_bg.wasm" } else { "pkg\game_web_bg.wasm" }
        function Resolve-CodePackName([string]$logical) {
            if ($script:codePackageRenamed -and $script:codePackageRenamed.ContainsKey($logical)) {
                return $script:codePackageRenamed[$logical]
            }
            return $logical
        }
        $requiredWeChatFiles = @(
            (Join-Path $distRoot "game.js"),
            (Join-Path $distRoot "game.json"),
            (Join-Path $distRoot "project.config.json"),
            (Join-Path $distRoot "wechat-adapter.js"),
            (Join-Path $distRoot "startup-branding.js"),
            (Join-Path $distRoot "bundle-roots.js"),
            (Join-Path $distRoot "branding\icon.png"),
            (Join-Path $distRoot "branding\splash.jpg"),
            (Join-Path $distRoot "pkg\game_web.js"),
            (Join-Path $distRoot "pkg\eh-probe.wasm"),
            (Join-Path $distRoot $wasmCoreRelative),
            (Join-Path $distRoot "code-package.json"),
            (Join-Path $skeletonRoot (Resolve-CodePackName "manifest.txt")),
            (Join-Path $skeletonRoot (Resolve-CodePackName "project.toml"))
        )
        if ($wasmInSubpackage) {
            # 内核入分包（subpackage 形态，或 remote + -WasmInSubpackage）多两项：
            # 清单（适配层判断"内核在分包"的唯一依据）与分包入口（微信的硬要求）。
            $requiredWeChatFiles += @(
                (Join-Path $distRoot "pack.json"),
                (Join-Path $packRoot "game.js"),
                (Join-Path $packWasmRoot "game.js")
            )
            # 内核搬走的另一半证明：pkg/ 里那份必须**不存在**。留着它一声不响——适配层读的是 pack/
            # 那份，游戏照常跑，代价只是主包又超回 4 MB，而"超限"要到上传那一刻才暴露。
            # （与形态无关：只要内核入了分包，主包里就不许再有第二份。）
            if (Test-Path -LiteralPath (Join-Path $distRoot "pkg\game_web_bg.wasm")) {
                throw "内核入了分包，但 pkg/game_web_bg.wasm 仍留在主包里：搬运步骤没跑（主包会按旧公式超限）"
            }
            if (Test-Path -LiteralPath (Join-Path $packRoot "game_web_bg.wasm")) {
                throw "内核应只在 $packWasmDirName/，但 $packDirName/game_web_bg.wasm 仍存在（两个根会漂移）"
            }
        }
        if ($isSubpackagePack) {
            # 逻辑名与宿主名**都不该**出现在 pack/ 里：前者说明切分漏了，后者说明搬运与改名对不上。
            foreach ($mustNotBeInPack in @("manifest.txt", "project.toml")) {
                foreach ($candidate in @($mustNotBeInPack, (Resolve-CodePackName $mustNotBeInPack))) {
                    if (Test-Path -LiteralPath (Join-Path $packRoot $candidate)) {
                        throw "分包形态下 $candidate 只应存在于主包（mainpack/），pack/ 里出现说明切分/改名不完整: $packRoot\$candidate"
                    }
                }
            }
            # 清单项必须"在 mainpack/ 里、不在 pack/ 里"（按**宿主文件名**判定）。
            # 这是本形态在本机唯一能验证的形态判据：真机的打包语义（root 目录整体进分包）本机证明不了，
            # 只能靠"两份互斥"把产物形态锁死。
            $manifestEntries = @((Get-Content -LiteralPath $mainPackManifest -Raw -Encoding UTF8 | ConvertFrom-Json).entries)
            foreach ($entry in $manifestEntries) {
                $hostName = Resolve-CodePackName $entry
                $entryRelative = $hostName -replace '/', '\'
                if (-not (Test-Path -LiteralPath (Join-Path $mainPackRoot $entryRelative))) {
                    throw "首屏清单项（宿主名 $hostName）不在主包里: mainpack/$hostName"
                }
                if (Test-Path -LiteralPath (Join-Path $packRoot $entryRelative)) {
                    throw "首屏清单项同时留在分包里（会与主包那份争抢读取路径）: pack/$hostName"
                }
            }
            # 内核搬走的另一半证明：pkg/ 里那份必须**不存在**。留着它一声不响——适配层读的是 pack/
            # 那份，游戏照常跑，代价只是主包又超回 4 MB，而"超限"要到上传那一刻才暴露。
        }
        # 配了远端回落就必须真的有配置文件（否则适配层不会启用远端，行为与预期不符）。
        if (-not [string]::IsNullOrWhiteSpace($RemotePackBaseUrl)) {
            $requiredWeChatFiles += (Join-Path $distRoot "remote-assets.json")
        }
        # 三形态互斥的另一半：分包形态不得带远端配置，否则适配层会走 CDN 回落，
        # "包内读到了"就永远发现不了"分包根本没挂载上"。
        if ($isSubpackagePack -and (Test-Path -LiteralPath (Join-Path $distRoot "remote-assets.json"))) {
            throw "分包形态不得带 remote-assets.json（它属于 -AssetPack remote）"
        }
        foreach ($file in $requiredWeChatFiles) {
            if (-not (Test-Path -LiteralPath $file)) { throw "WeChat package is missing: $file" }
            if ((Get-Item -LiteralPath $file).Length -le 0) { throw "WeChat package file is empty: $file" }
        }
        # PC/WebGL 的入口不得混入小游戏包：分支选错平台时会在这里失败，而不是发到线上才发现。
        foreach ($forbidden in @("game.exe", "index.html", "assetc.exe")) {
            if (Test-Path -LiteralPath (Join-Path $distRoot $forbidden)) {
                throw "WeChat package must not contain $forbidden (wrong platform branch?)"
            }
        }
        # wasm 胶水（M32）：小游戏包里**允许且要求**有 `pkg\game_web.js` 与 wasm 内核本体，
        # 但那份胶水必须是 `--target no-modules` 的纯脚本产物。内核的位置跟着形态变
        # （分包形态在 pack/ 里，见上面那次搬运），所以按 $wasmCoreRelative 判而不是写死两处。
        #
        # 这里不再按"文件名黑名单"禁止它（那是 wasm 未接入时代的写法）：黑名单挡不住
        # "构建分支写错、把 `--target web` 的 ESM 胶水打进来"这个真正的风险——
        # 它只会让**正确**的产物被误杀。现在的口径是**内容判定**：
        # 下面那条通用兜底会用 `vm.Script`（始终按脚本解析）逐个验证每个 .js，
        # 顶层 `export`/`import` 一律判红，因此"ESM 混进小游戏包"依然被拦在发布期，
        # 而且拦的是真东西而不是一个文件名。
        foreach ($wasmAsset in @("pkg\game_web.js", $wasmCoreRelative)) {
            $wasmAssetPath = Join-Path $distRoot $wasmAsset
            if (-not (Test-Path -LiteralPath $wasmAssetPath)) {
                throw "WeChat package is missing $wasmAsset (wasm 核心应随包分发；检查 wasm-bindgen 步骤)"
            }
            if ((Get-Item -LiteralPath $wasmAssetPath).Length -le 0) {
                throw "WeChat package file is empty: $wasmAsset"
            }
        }
        if (Test-Path -LiteralPath (Join-Path $distRoot "game_web.js")) {
            throw "launcher 根不应再有 game_web.js：胶水固定在 pkg\game_web.js"
        }
        # 通用兜底：小游戏包里每个 .js 都必须能按**脚本**（CommonJS）解析。
        # 注意不能用 `node --check`：Node 会按内容自动识别 ESM，对顶层 export 反而"通过"，
        # 必须用 vm.Script（始终按 script 解析）或等价手段。
        $wechatScripts = Get-ChildItem -LiteralPath $distRoot -Recurse -File -Filter *.js -Force
        $nodeForSyntax = Get-Command node -ErrorAction SilentlyContinue
        if ($nodeForSyntax) {
            # 探针**只写 stdout**：PS 5.1 在 $ErrorActionPreference='Stop' 下会把原生程序的 stderr
            # 变成终止性错误，一旦用 2>&1 捕获，脚本会在检查退出码之前就中断，
            # 于是"守卫触发了"和"守卫自己崩了"看起来一模一样（实测踩过）。
            $syntaxProbe = "const fs=require('fs'),vm=require('vm'),p=process.argv[1];try{new vm.Script(fs.readFileSync(p,'utf8'),{filename:p});process.exit(0)}catch(e){process.stdout.write(String(e.message).split('\n')[0]);process.exit(1)}"
            foreach ($scriptFile in $wechatScripts) {
                $probe = & $nodeForSyntax.Source -e $syntaxProbe $scriptFile.FullName
                if ($LASTEXITCODE -ne 0) {
                    throw ("WeChat package contains a non-CommonJS script: {0} -> {1}" -f $scriptFile.Name, ($probe -join ' '))
                }
            }
            Write-Host ("==> WeChat scripts are CommonJS-parseable: {0} files" -f $wechatScripts.Count) -ForegroundColor DarkGray
            # 再加一档**上传器**的解析上限：上面那条用的是本机 Node（什么新语法都认），而微信
            # 上传器的解析器低一档——实测（2026-10-03，稳定版 2.02.2608070）两次上传被拒：
            #   game_web.js, 989:72 → wasm-bindgen 胶水的 ES2021 逻辑赋值 `??=`
            #   web-runtime.js, 1:105804 → 自维护运行时的 ES2020 空值合并 `??`（该文件已随 B8 删除，
            #      这条是 2026-10-03 的历史观测记录；门自己按 files=N 现算，不写死这个数）
            # （开发者工具里跑得好好的，只有上传炸）。口径与"为什么是 ES2019"见
            # scripts/verify/check-upload-ecma.mjs 头部。
            $ecmaTool = Join-Path $packageScriptRoot "..\verify\check-upload-ecma.mjs"
            if (-not (Test-Path -LiteralPath $ecmaTool)) { throw "缺少上传语法门脚本: $ecmaTool" }
            $savedEapEcma = $ErrorActionPreference
            $ErrorActionPreference = "Continue"
            try {
                # 同样只收 stdout（stderr 在 EAP=Stop 下会变成终止性错误，见上面的探针注释）。
                $ecmaOutput = & $nodeForSyntax.Source $ecmaTool 2019 @($wechatScripts.FullName)
                $ecmaExit = $LASTEXITCODE
            }
            finally { $ErrorActionPreference = $savedEapEcma }
            if ($ecmaExit -ne 0) {
                $ecmaDetail = (($ecmaOutput | ForEach-Object { "$_" }) -join ' ')
                if (-not $ecmaDetail) { $ecmaDetail = "退出码 $ecmaExit 且 stdout 无输出（多半是 acorn 没解析到：跑 scripts/build/minify-toolchain.ps1 或设 `$env:ACORN）" }
                throw ("发布包里有微信上传器解析不了的语法（口径 ES2019）: {0}" -f $ecmaDetail)
            }
            Write-Host ("==> {0}" -f (($ecmaOutput | Where-Object { $_ -match '^upload_ecma_ok' } | Select-Object -First 1))) -ForegroundColor DarkGray
        }
        else {
            # 没有 node 时退回文本检查：顶层 export/import 语句（压缩单行文件不会误伤，
            # 因为按行首匹配，且它们在行首不是合法 CommonJS）。
            foreach ($scriptFile in $wechatScripts) {
                $match = Select-String -LiteralPath $scriptFile.FullName -Pattern '^\s*(export\s|import\s+[^\(])' | Select-Object -First 1
                if ($match) {
                    throw "WeChat package contains a non-CommonJS script: $($scriptFile.Name) (line $($match.LineNumber))"
                }
            }
        }
        # 代码包体积上限：三形态的算法与配额都不同，统一交给 Assert-WeChatPackageVolume。
        # 注意这里只是**提前拦截**（run.txt/deps.txt/build-info.txt 还没生成），
        # 最终数值在收尾处重新测量并输出 wechat_minigame_package_ok。
        $wechatTotalSize = (Get-ChildItem -LiteralPath $distRoot -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
        $wechatPackSize = Measure-WeChatSubpackageBytes -LauncherRoot $distRoot
        $wechatVolume = Assert-WeChatPackageVolume -TotalBytes $wechatTotalSize -PackBytes $wechatPackSize `
            -Form $(if ($wasmInSubpackage) { "subpackage" } else { $AssetPack }) -Suffix "（本值测于 run.txt/deps.txt/build-info.txt 生成之前，最终值见收尾的 wechat_minigame_package_ok）"
        Write-Host ("wechat_minigame_size_precheck files={0} main_kb={1:N0} main_of_limit={2:P1} subpackage_kb={3:N0} subpackage_of_limit={4:P1} pack_kb={5:N0} total_kb={6:N0} total_of_limit={7:P1} asset_pack={8}" -f `
            $requiredWeChatFiles.Count, ($wechatVolume.Main / 1KB), ($wechatVolume.Main / 4MB), `
            $(if ($wasmInSubpackage) { $wechatVolume.Pack / 1KB } else { 0 }), `
            $(if ($wasmInSubpackage) { $wechatVolume.Pack / 20MB } else { 0 }), `
            ($wechatPackSize / 1KB), ($wechatVolume.Total / 1KB), ($wechatVolume.Total / 20MB), $AssetPack) -ForegroundColor DarkGray
    }
    elseif ($Platform -eq "webgl") {
        $requiredWebFiles = @(
            (Join-Path $distRoot "index.html"),
            (Join-Path $distRoot "test-webgl.bat"),
            (Join-Path $distRoot "branding\icon.png"),
            (Join-Path $distRoot "branding\splash.png"),
            (Join-Path $distRoot "pkg\game_web.js"),
            (Join-Path $distRoot "pkg\game_web_bg.wasm"),
            (Join-Path $packRoot "manifest.txt"),
            (Join-Path $packRoot "project.toml")
        )
        foreach ($file in $requiredWebFiles) {
            if (-not (Test-Path -LiteralPath $file)) { throw "WebGL package is missing: $file" }
        }
        # 0 字节的壳等于没有壳：存在性检查放过它，双击时只会一闪而过。
        $testBat = Join-Path $distRoot "test-webgl.bat"
        if ((Get-Item -LiteralPath $testBat).Length -le 0) { throw "WebGL package shell is empty: test-webgl.bat" }
        Write-Host "webgl_package_ok files=$($requiredWebFiles.Count)" -ForegroundColor Green
    }
    else {
        # 注意：发布版 `game.exe` 是 **Windows GUI 子系统**程序
        # （`crates/game/src/main.rs` 的 `windows_subsystem = "windows"`，双击时不弹控制台）。
        # 这类程序用 `& exe args` 调用时 PowerShell **既不等待也不捕获 stdout**，
        # `$LASTEXITCODE` 会保留上一条命令的值——于是"发布包自检"会静默空跑成永远通过。
        # 必须用 Start-Process -Wait + 重定向文件来真实取回退出码与输出。
        $packCheck = Invoke-PackagedCheck -Runner (Join-Path $distRoot "game.exe") -PackRoot $packRoot
        Write-Host $packCheck.Output
        if ($packCheck.ExitCode -ne 0) {
            throw "Release package check failed (exit $($packCheck.ExitCode))."
        }
        $packRootsLine = $packCheck.Output | Where-Object { $_ -match "player_data_roots" } | Select-Object -First 1
        if (-not $packRootsLine) { throw "Release package check did not report player_data_roots." }
        if ($packRootsLine -notmatch "packaged=true") {
            throw "Release package did not report packaged=true: $packRootsLine"
        }
        Write-Host "Release package player data roots: $packRootsLine" -ForegroundColor DarkGray
    }

