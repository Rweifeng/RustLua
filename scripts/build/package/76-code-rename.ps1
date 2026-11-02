# -----------------------------------------------------------------------------
# 打包阶段：代码包后缀合规（按白名单改名）
# 自 scripts/build/package.ps1 原样迁出（原 L1785-L1838，2026-11-02 按功能拆分）。
# 执行语义：本文件被入口 dot-source，跑在入口的脚本作用域里——阶段之间共享变量、
#   ErrorActionPreference 与 throw 行为都跟拆分前一致。它不是函数，别当函数调。
#   PSScriptRoot 已统一换成入口设置的 packageScriptRoot（本文件不在 scripts/build/ 下）。
# -----------------------------------------------------------------------------

    # ------------------------------------ 代码包后缀合规：按映射改名（**必须最后做**）
    # 微信代码包只上传后缀在白名单内的文件（scripts/lib/code-package-rules.js，官方《代码包》文档）：
    # `.lua` / `.toml` / `.md` 传不上去，真机上 `readFileSync` 得到 permission denied，
    # 而这个游戏的全部 Lua 逻辑都靠逐文件读取 —— 实测真机启动直接失败（`pack/project.toml HTTP 404`）。
    # 所以这里把产物里每个需要改名的文件改成 `<原名>.txt`（末位后缀入白名单，原名可追溯），
    # 并把"逻辑路径 → 宿主文件名"的映射落在产物根的 code-package.json；运行时只查表、不重算规则。
    #
    # **位置固定在最后**：CDN 导出（assets/ 与 -CdnPackDir 镜像）与瘦身都按**逻辑路径**工作。
    # CDN 是普通 HTTPS 服务器、不受白名单约束，而远端 URL / 用户目录缓存路径 / manifest.txt 校验和
    # 全部按逻辑路径算——先改名会让远端回落静默 404（同类顺序事故在本文件已有先例，见"必须早于裁剪"）。
    if ($Platform -eq "wechat-minigame") {
        $renamedCount = 0
        $renameTrees = @($packRoot)
        if ($mainPackRoot -and (Test-Path -LiteralPath $mainPackRoot)) { $renameTrees = @($mainPackRoot) + $renameTrees }
        foreach ($tree in $renameTrees) {
            foreach ($key in @($script:codePackageRenamed.Keys)) {
                $relative = $key -replace '/', '\'
                $from = Join-Path $tree $relative
                if (-not (Test-Path -LiteralPath $from)) { continue }
                $to = Join-Path $tree ($script:codePackageRenamed[$key] -replace '/', '\')
                $toDir = Split-Path -Parent $to
                if (-not (Test-Path -LiteralPath $toDir)) { New-Item -ItemType Directory -Force -Path $toDir | Out-Null }
                if (Test-Path -LiteralPath $to) {
                    throw ("改名目标已存在，会静默覆盖: {0} → {1}" -f $from, $to)
                }
                Move-Item -LiteralPath $from -Destination $to
                $renamedCount += 1
            }
        }
        if ($renamedCount -ne $script:codePackageRenamed.Count -and $isSubpackagePack) {
            # 分包形态下两棵树合起来应当恰好命中映射里的每一条（首屏的在 mainpack/、其余在 pack/）；
            # remote/full 形态只有一棵树，数量可能少于映射（被裁掉的资源本来就不在包里），不在此断言之列。
            throw ("改名没有全覆盖: 实际改了 {0} 个，映射有 {1} 条" -f $renamedCount, $script:codePackageRenamed.Count)
        }
        # 收尾断言：代码包里不得再有非白名单后缀的文件——将来往分包里放 .lua 会在这里当场失败，
        # 而不是等真机上"启动失败、日志里只有一串 permission denied"。
        $whitelistDirs = @($packRoot)
        if ($mainPackRoot -and (Test-Path -LiteralPath $mainPackRoot)) { $whitelistDirs = @($mainPackRoot) + $whitelistDirs }
        if (-not $nodeForPack) { $nodeForPack = Get-Command node -ErrorAction SilentlyContinue }
        $savedEap2 = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $whitelistOutput = & $nodeForPack.Source $firstScreenTool --check-whitelist @whitelistDirs 2>&1
            $whitelistExit = $LASTEXITCODE
        }
        finally {
            $ErrorActionPreference = $savedEap2
        }
        if ($whitelistExit -ne 0) {
            throw ("代码包里仍有非白名单后缀的文件（真机不会上传它们）: " + (($whitelistOutput | ForEach-Object { "$_" }) -join ' '))
        }
        Write-Host ("==> 已按白名单改名 {0} 个文件；{1}" -f $renamedCount, (($whitelistOutput | Where-Object { $_ -match '^code_package_whitelist_ok' } | Select-Object -First 1))) -ForegroundColor DarkGray
    }

