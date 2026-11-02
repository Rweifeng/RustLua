# -----------------------------------------------------------------------------
# 打包阶段：资源包（assetc package、容器形态、微信音频）
# 自 scripts/build/package.ps1 原样迁出（原 L688-L872，2026-11-02 按功能拆分）。
# 执行语义：本文件被入口 dot-source，跑在入口的脚本作用域里——阶段之间共享变量、
#   ErrorActionPreference 与 throw 行为都跟拆分前一致。它不是函数，别当函数调。
#   PSScriptRoot 已统一换成入口设置的 packageScriptRoot（本文件不在 scripts/build/ 下）。
# -----------------------------------------------------------------------------

    Write-Host "==> assetc package (platform=$Platform, data resource pack, compress=$Compress encrypt=$Encrypt)" -ForegroundColor Cyan
    $assetc = Join-Path $nativeTargetRoot "assetc.exe"
    if (Test-Path $packRoot) { Remove-Item -LiteralPath $packRoot -Recurse -Force }
    # 容器形态（docs/67 批 C2 第 2 步）：**暂不剥离 Lua 注释**，原因已实测（见下）。
    #
    # 原计划是把剥离前移到打包之前（"assetc 从已剥离的临时树直接造容器"），实测走不通：
    # `strip-lua-comments.mjs` 的设计前提是**目标目录里有 manifest.txt**（它按清单逐条校验
    # 改写结果，`找不到清单` 就直接退出 1）。打包之前的工程树没有清单，所以这条路径必然失败。
    # 而"打包之后"又改不了容器里的 .lua（条目在块里，改文件既不生效也改不动块）。
    #
    # 实测收益（供将来决定要不要在 Rust 里重写一个剥离器）：
    #   `game_jhyt` 的 155 个脚本 1,638,166 B → 1,074,212 B，**raw 省 551 KB（34%）**。
    #   但在容器里这部分会进 zstd 块（lua 块整体压到 0.249），注释本身高度可压，
    #   实际落盘收益估计只有一二百 KB 量级——所以"容器形态不剥离"是当前可接受的取舍。
    # 结论：容器形态下剥离要么在 assetc 内部实现（Rust 版 Lua 词法剥离），要么不做。
    # 不做的时候包更大但仍然正确；绝不能退回"改文件不同步清单"的老做法（那是坏包）。
    if ($BundleMode -ne "none" -and $isWebPlatform) {
        Write-Host "==> 容器形态：跳过 Lua 注释剥离（工具依赖 manifest.txt；raw 可省 551 KB，落盘预估一二百 KB）" -ForegroundColor DarkGray
    }
    $assetcProjectRoot = $gameRoot
    $assetcArgs = @("package", $assetcProjectRoot, "-o", $packRoot)
    if ($BundleMode -ne "none") {
        # 容器形态（docs/67）：压缩由容器逐块负责，所以 --compress 无意义，传了也会被拒绝提示；
        # 这里只在目录形态下才带它，避免"以为压过、其实没压"的错觉。
        $assetcArgs += @("--bundle", $BundleMode)
        Write-Host "==> assetc bundle mode = $BundleMode（docs/67；目录形态后处理会跳过）" -ForegroundColor Yellow
    }
    # 容器形态与 `-Compress` 互斥：压缩由块负责，逐文件再压一层是白做功。
    # `-Encrypt` 则**支持**（docs/67 批 F）：走块级 AEAD，贴图/音频/字体一起进加密域，
    # 且 Web/微信也能开——解密在 wasm/Rust 侧做，不依赖 JS 读明文。
    if ($BundleMode -ne "none" -and $Compress) {
        throw ("-BundleMode $BundleMode 与 -Compress 不能同时用：" +
            "容器的压缩由块负责，逐文件再压一层是白做功。" +
            "确实需要逐文件 gzip 请显式加 -BundleMode none（那就是 C2 之前的产物形态）。")
    }
    if ($Compress -and $BundleMode -eq "none") { $assetcArgs += "--compress" }
    if ($BundleMode -ne "none" -and $BlockSizeKb -gt 0) {
        $assetcArgs += @("--block-size", "$BlockSizeKb")
    }
    # 容器形态：字体**留在目录里**（所有平台统一，`--keep`）。
    # 硬理由只有一个平台，但统一更省事也不亏：微信 `wx.loadFont(path)` 只吃真实文件路径，
    # 容器条目它读不到（不 keep 就被 `WeChat package requires the subset font inside pack/` 拦下）；
    # PC 的断言同样要求字体在 pack/ 里。统一 keep 让"字体是真实文件"这条不变量在两端都成立，
    # 代价只有 282 KB 不进口袋。
    if ($BundleMode -ne "none") {
        $assetcArgs += @("--keep", "assets/fonts/")
        # 首包字体预热清单也必须留在目录里：`runtime_scene/fonts.rs` 按 `<pack>/chars.txt`
        # 的真实路径读它，进容器后读不到；PC 发布门（`jhyt_m31_pc_release` ①）也点名要这个文件。
        # 代价只有几 KB，和字体同一条不变量：**boot 期按真实路径读的东西不进容器**。
        $assetcArgs += @("--keep", "chars.txt")
        # 注意：`scripts/main.lua` **不能** keep——assetc 要求启动入口必须在 `unit=main` 的组里
        # （Web 上 require 同步，放到目录会让启动即失败）。所以发布门对脚本改查 manifest
        # （见 `jhyt_m31_pc_release` ①），不要求它们是真实文件。
        # 微信音频也必须留在目录里：`InnerAudioContext.src` 只吃真实文件路径，容器条目
        # （`bundles/audio-*.rlb`）宿主自己读不到，表现就是"进游戏完全没声音"。
        # 实测 devtools 复现：音频 16/16 在容器里，但 play 静默失败。
        if ($Platform -eq "wechat-minigame") {
            $assetcArgs += @("--keep", "assets/audio/")
        }
        # 微信本地包不携带 unit=cdn 的组：brand/splash 已由 package-branding 压成主包
        # branding/splash.jpg；再打一份进 pack/ 就是纯重复（实测 1,224 KB）。
        # remote 形态例外：cdn 组正是要导到 assets/ 上传 CDN 的内容。
        if ($Platform -eq "wechat-minigame" -and $AssetPack -ne "remote") {
            $assetcArgs += "--exclude-cdn"
        }
    }
    if ($Encrypt) { $assetcArgs += "--encrypt" }
    # 剔除未声明字体（docs/52 配额门的真根因，2026-10-12）：开发树里的停用全量字体
    # （jhyt 的 NotoSansSC-VF.ttf 17.7 MB）没有 `font`/`font_fallback` 声明、也没有任何
    # Lua/场景引用，宿主不会读它，随包发就把分包顶到 25,626 KB > 20,480 KB。剔除发生在
    # manifest.txt 生成之前，清单与磁盘自洽。
    # PC 不开（桌面无体积门，换字体走设置还要它）；**webgl 必须开**（2026-10-17 真机实测）：
    # 浏览器没有本地文件系统，整个 pack/ 都要经网络下来——真机首屏实测 `preload_bytes=22,811,051`、
    # 其中 17.7 MB 就是这份死重，手机因此卡在闪屏几十秒。注释里"桌面无体积门"的理由在 webgl
    # 这一档根本不成立：它的体积门就是 WiFi 那一跳的等待时间。
    if ($Platform -eq "wechat-minigame" -or $Platform -eq "webgl") { $assetcArgs += "--drop-undeclared-fonts" }
    & $assetc @assetcArgs
    if ($LASTEXITCODE -ne 0) { throw "assetc package failed." }

    # 计划表声明内核的独立根：pack-wasm 不是 assetc 的资源组（wasm 是发布链后搬进去的），
    # 但适配层/layout 都靠这个字段决定"boot 等哪个根、容器放哪"。
    if ($wasmInSubpackage) {
        $planPath = Join-Path $packRoot "bundle_plan.json"
        if (Test-Path -LiteralPath $planPath) {
            $plan = Get-Content -LiteralPath $planPath -Raw -Encoding UTF8 | ConvertFrom-Json
            $plan | Add-Member -NotePropertyName wasm_root -NotePropertyValue $packWasmDirName -Force
            [IO.File]::WriteAllText($planPath, ($plan | ConvertTo-Json -Depth 12 -Compress),
                (New-Object Text.UTF8Encoding($false)))
            # 根计划是适配层的多根开关：默认形态也放一份（带 launcher_root 标记），
            # 这样 pack-wasm/pack 的懒加载、路由和 check-bundle-mounts 走同一套计划表。
            $rootPlan = $plan | ConvertTo-Json -Depth 12 -Compress | ConvertFrom-Json
            $rootPlan | Add-Member -NotePropertyName launcher_root -NotePropertyValue $true -Force
            [IO.File]::WriteAllText((Join-Path $distRoot "bundle_plan.json"),
                ($rootPlan | ConvertTo-Json -Depth 12 -Compress), (New-Object Text.UTF8Encoding($false)))
        }
    }

    # 容器形态（docs/67）与 web/微信的发布后处理互斥：下面三段都按"pack/ 里是散文件"操作
    # （Lua 注释剥离要改 .lua 并同步改清单、首屏切分要按实际文件搬运、后缀改名要按文件名建映射）。
    # 与其让它们对着一堆 .rlb 静默绕过去产出一个"看起来正常"的包，不如在这里直接拒绝。
    # 只有微信还拦着：首屏切分（mainpack/ 搬运）与代码包后缀改名都按散文件工作，
    # 需要先迁到 bundle_plan.json（docs/67 批 C2）。WebGL 没有 4 MB 上限也没有分包，
    # 容器整包下发即可，`game_web` 已能按 bundle 预载——所以 webgl 放行。
    if ($false) {  # 容器形态的微信支持见 docs/67 批 C2：首屏闭包已改为读计划表
        throw ("-BundleMode $BundleMode 目前只支持 -Platform pc：" +
            "web/微信的发布后处理（Lua 注释剥离 / 首屏切分 / 代码包后缀改名）仍按散文件工作，" +
            "需要先迁移到 bundle_plan.json（docs/67 §9 批 C 的剩余项）。")
    }

    # Lua 注释剥离（仅 web 平台；紧随 assetc 之后、首屏切分与代码包改名之前）：
    # 微信主包 4,096 KB 是硬门，2026-10-12 复跑实测 2,571 KB / 62.8%（`wechat_minigame_package_ok`
    # 那行的字节和口径，见 docs/52 §6.4；旧读数 3,635 KB / 88.7% 是 §6.1 那次、内核还在主包时的
    # 账，别用 du——它对本工程 700 个小文件虚高数百 KB）。**编译侧**的杠杆已全部用完
    # （wasm：opt-level=z + lto=fat + codegen-units=1 + wasm-opt -Oz + --remove-name-section；JS：terser），
    # 所以余量只能从**内容侧**买：主包里的 Lua 共 862 KB、其中 283 KB 是给人看的注释
    # （§6.1 那次读数；本工程脚本已涨到 155 个，同一条命令实测 `saved=581KB`，剥后主包
    # scripts/ 共 1,070 KB）。玩家不会读发布包里的注释，这是这批体积里唯一完全不改变运行时行为的减法。
    # 两条必须由工具一起满足的约束（见该文件头注释）：
    #   · 同步重写 manifest.txt 的字节数与 FNV-1a-64——运行时逐条校验清单，只改文件不改清单
    #     等于首屏每个脚本都校验失败；
    #   · **保留行数**（剥文字留换行）——发布包里 Lua 栈回溯的行号仍要能对着仓库源码读。
    # 源工程 `examples/<game>/scripts/**.lua` 一个字都不改，改的只是 pack/ 里的产物副本。
    # PC 平台不剥：桌面包没有体积门，而"pack 内容与源文件逐字节相同"在排查时很有用。
    if ($isWebPlatform -and $BundleMode -eq "none") {
        $stripLuaTool = Join-Path $packageScriptRoot "strip-lua-comments.mjs"
        if (-not (Test-Path -LiteralPath $stripLuaTool)) {
            throw "缺少 Lua 注释剥离脚本: $stripLuaTool（主包体积靠它，发布不能在没有它的情况下继续）"
        }
        $nodeForStripLua = Get-Command node -ErrorAction SilentlyContinue
        if (-not $nodeForStripLua) { throw "Lua 注释剥离需要 node" }
        # 与首屏清单那一手同一个坑：工具正常也会往 stderr 写**拒绝原因**（清单复现不了、
        # 某个文件词法不成形），而 PS 5.1 在 Stop 下会把原生程序的 stderr 变成终止性错误，
        # 让"工具拒绝"和"工具崩了"看起来一模一样。这里临时降级、合流，只看退出码。
        $savedEapStrip = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $stripLuaOutput = & $nodeForStripLua.Source $stripLuaTool $packRoot "--write" 2>&1
            $stripLuaExit = $LASTEXITCODE
        }
        finally {
            $ErrorActionPreference = $savedEapStrip
        }
        if ($stripLuaExit -ne 0) {
            throw ("Lua 注释剥离失败（exit {0}）: {1}" -f $stripLuaExit, (($stripLuaOutput | ForEach-Object { "$_" }) -join ' '))
        }
        Write-Host (($stripLuaOutput | ForEach-Object { "$_" }) -join ' ') -ForegroundColor DarkGray
    }

    # 微信小游戏：音频统一为 MP3 格式（InnerAudioContext 在 iOS 和 Android 均支持，
    # 不再需要 OGG+WAV 双格式）。微信发布用 `--keep assets/audio/` 把 MP3 留在真实文件路径上
    # （容器条目 InnerAudioContext 读不到），所以这里同时接受"目录里的 raw MP3"与
    # "计划表 audio 组"两种来源；两者都为空才算没有音频，有资源但无 .mp3 直接硬失败。
    if ($Platform -eq "wechat-minigame") {
        $audioPaths = @()
        $audioDir = Join-Path $packRoot "assets\audio"
        $rawMp3 = @(Get-ChildItem -LiteralPath $audioDir -Filter "*.mp3" -File -ErrorAction SilentlyContinue)
        if ($rawMp3.Count -gt 0) {
            $audioPaths += @($rawMp3 | ForEach-Object { "assets/audio/" + $_.Name })
        }
        if ($BundleMode -ne "none") {
            $planPath = Join-Path $packRoot "bundle_plan.json"
            if (Test-Path -LiteralPath $planPath) {
                $audioPlan = Get-Content -LiteralPath $planPath -Raw -Encoding UTF8 | ConvertFrom-Json
                foreach ($group in @($audioPlan.groups)) {
                    if ($group.name -eq "audio" -or @($group.paths | Where-Object { $_ -like "assets/audio/*" }).Count -gt 0) {
                        $audioPaths += @($group.paths)
                    }
                }
            }
        }
        $audioPaths = @($audioPaths | Select-Object -Unique)
        $mp3Paths = @($audioPaths | Where-Object { $_ -match '\.mp3$' })
        if ($audioPaths.Count -gt 0 -and $mp3Paths.Count -eq 0) {
            throw ("WeChat audio: audio 组有 {0} 条资源，但没有任何 .mp3（iOS InnerAudioContext 只认 MP3；" +
                "补 MP3 或删掉 audio 组，不能留一个 1.4 MB 的静音分包）") -f $audioPaths.Count
        }
        if ($mp3Paths.Count -eq 0) {
            Write-Host "==> WeChat audio: 未声明音频组（无 BGM/SFX）" -ForegroundColor DarkGray
        }
        else {
            Write-Host ("==> WeChat audio: {0}/{1} 条为 .mp3（真实文件；InnerAudioContext 可直接读）" -f `
                $mp3Paths.Count, $audioPaths.Count) -ForegroundColor DarkGray
        }
    }

