# -----------------------------------------------------------------------------
# 打包阶段：目录隔离自测（-SelfTest）与收尾清理
# 自 scripts/build/package.ps1 原样迁出（原 L2351-L2555，2026-11-02 按功能拆分）。
# 执行语义：本文件被入口 dot-source，跑在入口的脚本作用域里——阶段之间共享变量、
#   ErrorActionPreference 与 throw 行为都跟拆分前一致。它不是函数，别当函数调。
#   PSScriptRoot 已统一换成入口设置的 packageScriptRoot（本文件不在 scripts/build/ 下）。
# -----------------------------------------------------------------------------


# M20-03: best-effort removal of the isolated test copy. Windows may still hold
# the freshly launched runner (AV/indexer), so retry briefly and never fail the
# publish because of cleanup; leftovers are reported with their path.
function Remove-IsolatedRelease([string]$Path, [string]$Label) {
    for ($attempt = 1; $attempt -le 5; $attempt++) {
        if (-not (Test-Path -LiteralPath $Path)) { return }
        try {
            Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction Stop
            return
        }
        catch {
            Start-Sleep -Milliseconds (200 * $attempt)
        }
    }
    Write-Warning "==> Leftover $Label not removed (file in use); remove manually: $Path"
}

if ($SelfTest) {
    Write-Host "`n==> Directory-isolation self test (no source/target/.cache, path with spaces)" -ForegroundColor Cyan
    $isolated = Join-Path $env:TEMP ("rustlua release test " + (Get-Date -Format "HHmmss"))
    New-Item -ItemType Directory -Force -Path $isolated | Out-Null
    # 复制**发布根**（不是 $distRoot）：微信发布根下是 launcher/ + assets/ 两个目录
    # （分包形态下只有 launcher/，主包与分包都在它里面），隔离自测要连资源包一起搬走，
    # 才能验证"首包与资源包配套"。其它平台 $finalRoot 就是 $distRoot，行为不变。
    # 复制到隔离目录时，Copy-Item 按**源目录名**（`builds\<平台>` 的最后一段）建容器目录，
    # 而不是按 $PublishName：微信发布根下是 launcher/ + assets/，容器名就是 `wechat-minigame`。
    # 早期这里写死 `Join-Path $isolated $PublishName`，于是微信侧的隔离自测永远在找一个
    # 不存在的路径（`...\jianghu_youtian\launcher\game.js`），把"产物是好的"报成"缺文件"。
    $isolatedReleaseRoot = Join-Path $isolated (Split-Path -Leaf $finalRoot)
    Copy-Item -LiteralPath $finalRoot -Destination $isolated -Recurse -Force
    if ($Platform -eq "wechat-minigame") {
        # 小游戏是静态包：断言隔离目录里运行时与配置齐全，并用无头 harness 实跑一次。
        # 发布根下有 launcher/（首包）与 assets/（资源包）两个兄弟目录，验证脚本吃的是这两个。
        $isolatedRoot = Join-Path $isolatedReleaseRoot "launcher"
        $isolatedSkeletonRoot = if ($isSubpackagePack) { Join-Path $isolatedRoot "mainpack" } else { Join-Path $isolatedRoot $packDirName }
        # wasm 内核与主包骨架同一个判据：内核入分包时它在 pack/ 里（见上面那次搬运），
        # 隔离副本按旧位置找就会把"产物是好的"报成"缺文件"。
        # 判据是 `$wasmInSubpackage` 而不是 `$isSubpackagePack`：remote + -WasmInSubpackage 也搬内核（第 53 轮）。
        $isolatedWasmCore = Join-Path $isolatedRoot $(if ($wasmInSubpackage) { "$packWasmDirName\game_web_bg.wasm" } else { "pkg\game_web_bg.wasm" })
        foreach ($file in @(
            (Join-Path $isolatedRoot "game.js"),
            (Join-Path $isolatedRoot "game.json"),
            (Join-Path $isolatedRoot "project.config.json"),
            (Join-Path $isolatedRoot "wechat-adapter.js"),
            (Join-Path $isolatedRoot "bundle-roots.js"),
            (Join-Path $isolatedRoot "pkg\game_web.js"),
            (Join-Path $isolatedRoot "pkg\eh-probe.wasm"),
            $isolatedWasmCore,
            (Join-Path $isolatedSkeletonRoot "manifest.txt")
        )) {
            if (-not (Test-Path -LiteralPath $file)) { throw "isolated WeChat launcher is missing: $file" }
        }
        if ($wasmInSubpackage) {
            if (-not (Test-Path -LiteralPath (Join-Path $isolatedRoot "$packDirName\game.js"))) {
                throw "isolated subpackage launcher is missing the subpackage entry: $packDirName\game.js"
            }
            if (-not (Test-Path -LiteralPath (Join-Path $isolatedRoot "$packWasmDirName\game.js"))) {
                throw "isolated subpackage launcher is missing the wasm subpackage entry: $packWasmDirName\game.js"
            }
            $isolatedManifestPath = Join-Path $isolatedRoot "pack.json"
            if (-not (Test-Path -LiteralPath $isolatedManifestPath)) {
                throw "isolated subpackage launcher is missing pack.json"
            }
        }
        if ($isSubpackagePack) {
            # 切分真实性：逐条读隔离副本里的 pack.json，每条都必须在 mainpack/ 下**存在**、
            # 且在 pack/ 下**不存在**。这条断言是"复制到隔离目录的过程把形态搞坏了"唯一的哨兵；
            # 主包/分包两侧同时留同一份文件会让读取路径变得不确定（谁先命中谁生效）。
            # 隔离副本里也要再读一次改名映射（它跟着产物一起复制过来，是运行时的查询依据）。
            $isolatedMapPath = Join-Path $isolatedRoot "code-package.json"
            if (-not (Test-Path -LiteralPath $isolatedMapPath)) {
                throw "isolated WeChat launcher is missing code-package.json（运行时的改名查询依据）"
            }
            $isolatedRenamed = @{}
            $isolatedMap = Get-Content -LiteralPath $isolatedMapPath -Raw -Encoding UTF8 | ConvertFrom-Json
            if ($isolatedMap.renamed) {
                foreach ($property in $isolatedMap.renamed.PSObject.Properties) { $isolatedRenamed[$property.Name] = [string]$property.Value }
            }
            $isolatedEntries = @((Get-Content -LiteralPath $isolatedManifestPath -Raw -Encoding UTF8 | ConvertFrom-Json).entries)
            if ($isolatedEntries.Count -eq 0) { throw "isolated pack.json has an empty entries list" }
            $misplaced = @()
            foreach ($entry in $isolatedEntries) {
                $hostName = if ($isolatedRenamed.ContainsKey($entry)) { $isolatedRenamed[$entry] } else { $entry }
                $relative = $hostName -replace '/', '\'
                if (-not (Test-Path -LiteralPath (Join-Path $isolatedRoot ("mainpack\" + $relative)))) { $misplaced += "missing in mainpack/: $hostName" }
                if (Test-Path -LiteralPath (Join-Path $isolatedRoot ("$packDirName\" + $relative))) { $misplaced += "still in ${packDirName}/: $hostName" }
            }
            if ($misplaced.Count -ne 0) {
                throw ("isolated subpackage split is not authentic ({0} problems):`n  - {1}" -f $misplaced.Count, ($misplaced -join "`n  - "))
            }
            # 分包形态必须没有远端配置：有就意味着适配层会走 CDN 回落，掩盖"分包没挂载上"。
            if (Test-Path -LiteralPath (Join-Path $isolatedRoot "remote-assets.json")) {
                throw "isolated subpackage launcher must not contain remote-assets.json"
            }
            Write-Host ("Check: wechat_minigame_split_ok mainpack_files={0}（隔离副本里逐条复核）" -f $isolatedEntries.Count) -ForegroundColor DarkGray
        }
        if ($wasmInSubpackage -and -not $isSubpackagePack) {
            # remote + -WasmInSubpackage：清单在本形态下只回答"分包叫什么"，entries 必须为空
            # （remote 的首屏资源在 CDN，包内没有 mainpack/ 可路由）。这两条正是适配层判断
            # "内核在分包"的全部输入，写错了它在隔离副本里也会去主包找内核。
            $isolatedRemoteManifest = Get-Content -LiteralPath (Join-Path $isolatedRoot "pack.json") -Raw -Encoding UTF8 | ConvertFrom-Json
            if ([string]$isolatedRemoteManifest.layout.subpackage_root -ne $packDirName) {
                throw ("isolated remote launcher 的 pack.json 没把分包写成 ${packDirName}: '{0}'" -f `
                        $isolatedRemoteManifest.layout.subpackage_root)
            }
            if (@($isolatedRemoteManifest.entries).Count -ne 0) {
                throw "isolated remote launcher 的 pack.json 不该有首屏路由条目（remote 的首屏资源在 CDN）"
            }
        }
        $isolatedAssets = Join-Path $isolatedReleaseRoot "assets"
        if ($AssetPack -eq "remote") {
            # 瘦首包必须在隔离目录里也是"瘦"的，否则本地自测等于在测一个不存在的形态。
            foreach ($heavy in @("$packDirName\assets", "$packDirName\scenes", "$packDirName\scripts")) {
                if (Test-Path -LiteralPath (Join-Path $isolatedRoot $heavy)) {
                    throw "isolated thin WeChat pack still contains $heavy (isolation copy is not thin)"
                }
            }
            if (-not (Test-Path -LiteralPath (Join-Path $isolatedRoot "remote-assets.json"))) {
                throw "isolated thin WeChat package is missing remote-assets.json"
            }
            if (-not (Test-Path -LiteralPath (Join-Path $isolatedAssets "pack\manifest.txt"))) {
                throw "isolated release is missing assets/pack-sub/** (首包与资源包必须一起落地)"
            }
        }
        $verifier = Join-Path $packageScriptRoot "..\verify\verify-wechat-minigame.ps1"
        if ((Get-Command node -ErrorAction SilentlyContinue) -and (Test-Path -LiteralPath $verifier)) {
            # 瘦首包的重资源在 assets/ 里：自测必须把它当"CDN"喂给验证脚本，
            # 否则隔离目录里只剩骨架、运行时验证必然读不到资源。
            $verifierArgs = @("-NoProfile", "-File", $verifier, "-BundleRoot", $isolatedRoot)
            if ($AssetPack -eq "remote" -and (Test-Path -LiteralPath (Join-Path $isolatedAssets "pack"))) {
                $verifierArgs += @("-AssetPackRoot", $isolatedAssets)
                Write-Host "Check: powershell -File scripts\verify\verify-wechat-minigame.ps1 -BundleRoot $isolatedRoot -AssetPackRoot $isolatedAssets" -ForegroundColor DarkGray
            }
            else {
                Write-Host "Check: powershell -File scripts\verify\verify-wechat-minigame.ps1 -BundleRoot $isolatedRoot" -ForegroundColor DarkGray
            }
            & powershell @verifierArgs
            if ($LASTEXITCODE -ne 0) { throw "isolated WeChat runtime verification failed (exit $LASTEXITCODE)" }
        }
        else {
            Write-Warning "==> node or verifier missing; skipped isolated WeChat runtime check"
        }
        Remove-IsolatedRelease -Path $isolated -Label "WeChat self-test copy"
        Write-Host ("Directory-isolation self test passed (WeChat launcher + runtime OK; asset_pack={0})" -f $AssetPack) -ForegroundColor Green
    }
    elseif ($Platform -eq "webgl") {
        $isolatedRoot = $isolatedReleaseRoot
        foreach ($file in @(
            (Join-Path $isolatedRoot "index.html"),
            (Join-Path $isolatedRoot "pkg\game_web.js"),
            (Join-Path $isolatedRoot "pkg\game_web_bg.wasm"),
            (Join-Path $isolatedRoot "pack\manifest.txt")
        )) {
            if (-not (Test-Path $file)) { throw "isolated WebGL dir is missing: $file" }
        }
        Remove-IsolatedRelease -Path $isolated -Label "WebGL self-test copy"
        Write-Host "Directory-isolation self test passed (WebGL static package OK)" -ForegroundColor Green
    }
    else {
        $isolatedRunner = Join-Path $isolatedReleaseRoot "game.exe"
        $isolatedPack = Join-Path $isolatedReleaseRoot "pack"
        if (-not (Test-Path $isolatedRunner)) { throw "isolated dir is missing the runner" }
        $isolatedFont = Join-Path $isolatedReleaseRoot ("pack\assets\fonts\" + $fontName)
        if (-not (Test-Path -LiteralPath $isolatedFont) -or (Get-Item -LiteralPath $isolatedFont).Length -le 0) {
            # PC 的字体在 pack/ 里（assetc 打包产物）；发布根下的 fonts/ 只属于 webgl 的 index.html。
            throw "isolated dir is missing the packaged font: pack/assets/fonts/$fontName"
        }

        Write-Host "Check: $isolatedRunner --pack $isolatedPack --check" -ForegroundColor DarkGray
        # 用 --check 做无窗口校验：退出码 0 表示发布包完整（含空格路径 + 无源码环境）。
        # M31/P3：同时把玩家数据根指到一个临时目录，断言发布包形态确实解析到**用户数据目录**
        # （而不是安装目录），并断言安装目录里不出现 .cache/save/settings 等玩家数据。
        $isolatedUserDir = Join-Path $env:TEMP ("rustlua user data test " + (Get-Date -Format "HHmmss"))
        New-Item -ItemType Directory -Force -Path $isolatedUserDir | Out-Null
        $env:RUSTLUA_USER_DIR = $isolatedUserDir
        try {
            $isolatedCheck = Invoke-PackagedCheck -Runner $isolatedRunner -PackRoot $isolatedPack
            Write-Host $isolatedCheck.Output
            if ($isolatedCheck.ExitCode -ne 0) {
                throw "isolated check failed (exit $($isolatedCheck.ExitCode)): package incomplete or path incompatible."
            }
            $rootsLine = $isolatedCheck.Output | Where-Object { $_ -match "player_data_roots" } | Select-Object -First 1
            if (-not $rootsLine) { throw "isolated check did not report player_data_roots." }
            Write-Host "Player data roots: $rootsLine" -ForegroundColor DarkGray
            if ($rootsLine -notmatch "packaged=true") {
                throw "release package did not report packaged=true: $rootsLine"
            }
            if ($rootsLine -notmatch [regex]::Escape($isolatedUserDir)) {
                throw "release package ignored RUSTLUA_USER_DIR=$isolatedUserDir : $rootsLine"
            }
            $strayPlayerData = Get-ChildItem -LiteralPath $isolatedReleaseRoot -Recurse -Force -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -in @(".cache", ".data") -or $_.Extension -in @(".sav", ".log") }
            if ($strayPlayerData) {
                throw "release directory contains player data: $($strayPlayerData.FullName -join ', ')"
            }
        }
        finally {
            Remove-Item Env:\RUSTLUA_USER_DIR -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath $isolatedUserDir -Recurse -Force -ErrorAction SilentlyContinue
        }
        Remove-IsolatedRelease -Path $isolated -Label "self-test copy"
        Write-Host "Directory-isolation self test passed (spaces path + no-source check + user data dir OK)" -ForegroundColor Green
    }
}
