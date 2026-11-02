# -----------------------------------------------------------------------------
# 打包阶段：微信分包形态（首屏资源切分到 mainpack/）
# 自 scripts/build/package.ps1 原样迁出（原 L1607-L1705，2026-11-02 按功能拆分）。
# 执行语义：本文件被入口 dot-source，跑在入口的脚本作用域里——阶段之间共享变量、
#   ErrorActionPreference 与 throw 行为都跟拆分前一致。它不是函数，别当函数调。
#   PSScriptRoot 已统一换成入口设置的 packageScriptRoot（本文件不在 scripts/build/ 下）。
# -----------------------------------------------------------------------------

    # ------------------------------------ 分包形态：首屏资源切分到 mainpack/（subpackage）
    # 微信规则：subpackages[i].root 目录下的文件**全部**进分包，主包里一个都不能留。
    # 所以"主包保留首屏资源"不能表现为"在 pack/ 里留一部分"（那样留的在真机上仍属分包、
    # 首屏照样要等分包下载），只能把清单内的文件搬到**兄弟目录 mainpack/**（同构相对路径），
    # 由适配层按清单静态路由：首屏读 mainpack/，其余读 pack/。
    # 切分依据是 scripts/build/first-screen-pack.js 生成的 pack.json —— 构建期、运行时、harness
    # 三方共用的唯一事实来源，从 pack/ 的实际磁盘内容推导，不写死工程名或目录层级。
    # 与 remote 的关键差别：remote 是 Remove-Item（资源只在远端），这里是 Move-Item（资源一份不少）。
    # 注意：这里全程用**逻辑路径**搬运（改名在这之后、对两棵树一起做），所以下面的断言也都在
    # 逻辑名下做；"宿主名"相关的断言在改名之后单独跑一遍。
    if ($isSubpackagePack) {
        $packFileCountBefore = (Get-ChildItem -LiteralPath $packRoot -Recurse -File -Force | Measure-Object).Count
        $packBytesBefore = (Get-ChildItem -LiteralPath $packRoot -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum

        if (-not (Test-Path -LiteralPath $mainPackManifest)) {
            throw "首屏清单没有被生成: $mainPackManifest"
        }
        $firstScreenPlan = Get-Content -LiteralPath $mainPackManifest -Raw -Encoding UTF8 | ConvertFrom-Json
        $movedEntries = @($firstScreenPlan.entries)
        # 清单为空 = 首屏清单工具失效（它至少要放进骨架与启动场景）。若真发生，主包会只剩
        # launcher 自身，首屏全部依赖分包——这正是本形态要消除的情形，必须当场失败。
        if ($movedEntries.Count -eq 0) {
            throw "首屏清单为空（$mainPackManifest）：分包形态下首屏资源必须全部留在主包，空清单等于把首屏交给分包。"
        }

        New-Item -ItemType Directory -Force -Path $mainPackRoot | Out-Null
        $movedBytes = 0
        foreach ($entry in $movedEntries) {
            # 清单里的路径一律用 `/`（与 URL 风格一致），落到磁盘要转成 `\`。
            $relative = $entry -replace '/', '\'
            $from = Join-Path $packRoot $relative
            if (-not (Test-Path -LiteralPath $from)) {
                throw "首屏清单列了 pack/ 里不存在的文件: $entry"
            }
            $to = Join-Path $mainPackRoot $relative
            $toDir = Split-Path -Parent $to
            if (-not (Test-Path -LiteralPath $toDir)) { New-Item -ItemType Directory -Force -Path $toDir | Out-Null }
            $movedBytes += (Get-Item -LiteralPath $from).Length
            Move-Item -LiteralPath $from -Destination $to -Force
        }

        # 只删**因为搬运而空掉**的目录（自底向上反复清理到不动点）：任何还有文件留下的目录
        # （例如 assets/art 下既有首屏贴图又有非首屏贴图）天然保留，不会被误删。
        $emptiedDirs = @()
        $sweepAgain = $true
        while ($sweepAgain) {
            $sweepAgain = $false
            foreach ($dir in @(Get-ChildItem -LiteralPath $packRoot -Recurse -Directory -Force)) {
                if (-not (Test-Path -LiteralPath $dir.FullName)) { continue }
                if (@(Get-ChildItem -LiteralPath $dir.FullName -Force | Select-Object -First 1).Count -ne 0) { continue }
                Remove-Item -LiteralPath $dir.FullName -Recurse -Force
                $emptiedDirs += $dir.FullName.Substring($packRoot.Length).TrimStart('\')
                $sweepAgain = $true
            }
        }

        # 字节守恒：切分前后 pack/ + mainpack/ 必须与切分前的 pack/ **逐一对应**。
        # 这条断言是"切分不全 / 清单漏列"唯一的守门人——漏列的文件会被路由到分包，未挂载时
        # 首屏就是一片 Image.onerror（屏幕红条 + imageFailed>0），而那时已经发到线上了。
        $packBytesAfter = (Get-ChildItem -LiteralPath $packRoot -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
        $packFileCountAfter = (Get-ChildItem -LiteralPath $packRoot -Recurse -File -Force | Measure-Object).Count
        $mainPackBytes = (Get-ChildItem -LiteralPath $mainPackRoot -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
        $mainPackFileCount = (Get-ChildItem -LiteralPath $mainPackRoot -Recurse -File -Force | Measure-Object).Count
        if ($mainPackBytes -ne $movedBytes -or $mainPackFileCount -ne $movedEntries.Count) {
            throw ("首屏切分搬运不完整: mainpack/ 是 {0} 文件/{1} 字节，清单是 {2} 条/{3} 字节" -f `
                $mainPackFileCount, $mainPackBytes, $movedEntries.Count, $movedBytes)
        }
        if (($mainPackBytes + $packBytesAfter) -ne $packBytesBefore -or ($mainPackFileCount + $packFileCountAfter) -ne $packFileCountBefore) {
            throw ("首屏切分没有守恒: mainpack/ {0} + pack/ {1} = {2} 字节，切分前 pack/ 是 {3} 字节" -f `
                $mainPackBytes, $packBytesAfter, ($mainPackBytes + $packBytesAfter), $packBytesBefore)
        }
        # 分包入口是**声明与入口成对**的产物：少了它开发者工具直接拒开工程，所以在这里
        # 立刻断言，而不是等最后那条必需文件检查（那条也要留，用于全形态统一兜底）。
        if (-not (Test-Path -LiteralPath (Join-Path $packRoot "game.js"))) {
            throw "分包形态缺少分包入口 pack/game.js（微信要求 subpackages[i].root 目录下有 game.js）"
        }
        Write-Host ("wechat_minigame_split_ok pack_files={0} pack_kb={1:N0} subpackage_files={2} subpackage_kb={3:N0} swept_empty_dirs={4}" -f `
            $mainPackFileCount, ($mainPackBytes / 1KB), $packFileCountAfter, ($packBytesAfter / 1KB), $emptiedDirs.Count) -ForegroundColor Cyan
        Write-Host ("==> 首屏资源已切到 pack/：主包直接可读，其余 {0:N0} KB 在 pack-sub/ 分包里按需挂载" -f ($packBytesAfter / 1KB)) -ForegroundColor Cyan
        # 现在 mainpack/ 已真正落盘，才能验 boot 闭包（清单/计划表/文件三方对账）。
        if ($wasmInSubpackage) {
            $bootCheck = Join-Path $packageScriptRoot "check-boot-closure.js"
            $nodeForBoot = Get-Command node -ErrorAction SilentlyContinue
            if (-not $nodeForBoot) { throw "boot 闭包检查需要 node" }
            $bootArgs = @($bootCheck, "--launcher", $distRoot)
            if (-not [string]::IsNullOrWhiteSpace($BootTrace)) {
                $bootArgs += @("--trace", $BootTrace)
            }
            $savedEapBoot = $ErrorActionPreference
            $ErrorActionPreference = "Continue"
            try { $bootOutput = & $nodeForBoot.Source @bootArgs 2>&1; $bootExit = $LASTEXITCODE }
            finally { $ErrorActionPreference = $savedEapBoot }
            if ($bootExit -ne 0) {
                throw ("boot 闭包检查失败（exit {0}）: {1}" -f $bootExit, (($bootOutput | ForEach-Object { "$_" }) -join ' '))
            }
            Write-Host (($bootOutput | Where-Object { $_ -match '^boot_closure_ok' } | Select-Object -First 1)) -ForegroundColor Green
        }
    }

