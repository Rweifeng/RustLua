# -----------------------------------------------------------------------------
# 打包阶段：调试残留拦截 + 首屏清单 + 代码包改名映射
# 自 scripts/build/package.ps1 原样迁出（原 L1510-L1606，2026-11-02 按功能拆分）。
# 执行语义：本文件被入口 dot-source，跑在入口的脚本作用域里——阶段之间共享变量、
#   ErrorActionPreference 与 throw 行为都跟拆分前一致。它不是函数，别当函数调。
#   PSScriptRoot 已统一换成入口设置的 packageScriptRoot（本文件不在 scripts/build/ 下）。
# -----------------------------------------------------------------------------

    # ------------------------------------------------ 调试残留拦截（全平台，早于所有体积统计）
    # `debug-overlay.txt` 是**开发者工具里排查真机黑屏**用的开关（适配层把它当"开启屏幕自证覆盖层"
    # 的信号）。它一旦随包发出去，玩家的屏幕上就会常年挂着一层诊断色块与文字——而它是纯文本
    # 标记，发布脚本不拦就没人拦。删掉不影响任何自动化用例：harness 的覆盖层用例走的是
    # `wx.setStorageSync('rustlua_debug_overlay','1')`，不依赖这个文件。
    # 必须早于体积统计（文件被删会让主包小几十字节，晚了输出行就对不上真实产物）。
    $debugMarkers = @(Get-ChildItem -LiteralPath $stageRoot -Recurse -File -Force -Filter "debug-overlay.txt" -ErrorAction SilentlyContinue)
    foreach ($marker in $debugMarkers) {
        Remove-Item -LiteralPath $marker.FullName -Force
        Write-Warning "==> 已从发布目录移除调试残留（会让玩家屏幕挂上诊断覆盖层）: $($marker.FullName.Substring($stageRoot.Length).TrimStart('\'))"
    }

    # ------------------------------------ 首屏清单 + 代码包改名映射（微信全形态）
    # 这一步**必须早于 CDN 导出与任何改名**：清单与映射都是从**切分前的完整 pack/** 推导的，
    # 而 CDN 导出（remote 形态）与改名会改变磁盘上的名字；反过来，改名又必须晚于 CDN 导出
    # （远端 URL / 缓存 / 校验和全按逻辑路径算，先改名会让它们静默 404）。
    # 产物：
    #   pack.json      —— 仅分包形态：首屏路由（读主包还是读分包）
    #   code-package.json  —— 全形态：逻辑路径 → 会随代码包上传的宿主文件名（后缀白名单合规）
    $firstScreenTool = Join-Path $packageScriptRoot "first-screen-pack.js"
    $codePackageMap = Join-Path $distRoot "code-package.json"
    if ($Platform -eq "wechat-minigame") {
        if (-not (Test-Path -LiteralPath $firstScreenTool)) {
            throw "微信形态需要首屏清单生成器: $firstScreenTool"
        }
        $nodeForPack = Get-Command node -ErrorAction SilentlyContinue
        if (-not $nodeForPack) {
            throw "微信形态需要 node 运行首屏清单生成器（$firstScreenTool 从 pack/ 推导首屏闭包与改名映射）"
        }
        # 工具正常也会往 stderr 写警告（清单列了但磁盘上没有的文件），而 PS 5.1 在
        # $ErrorActionPreference='Stop' 下会把原生程序的 stderr 变成**终止性错误**——
        # 那样"工具有警告"和"工具自己崩了"看起来一模一样。这里临时降级为 Continue 并合并流，
        # 只看退出码（0 通过 / 1 清单不可靠 / 3 未找到 pack 根）。
        $savedEap = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $firstScreenArgs = @($firstScreenTool, $packRoot, "--rename-map", $codePackageMap, "--subpackage-root", $packDirName)
            if ($isSubpackagePack) { $firstScreenArgs += @("--out", $mainPackManifest) }
            $firstScreenOutput = & $nodeForPack.Source @firstScreenArgs 2>&1
            $firstScreenExit = $LASTEXITCODE
        }
        finally {
            $ErrorActionPreference = $savedEap
        }
        if ($firstScreenExit -ne 0) {
            throw ("首屏清单生成失败（exit {0}）: {1}" -f $firstScreenExit, (($firstScreenOutput | ForEach-Object { "$_" }) -join ' '))
        }
        Write-Host (($firstScreenOutput | Where-Object { $_ -match '^first_screen_pack_ok' } | Select-Object -First 1)) -ForegroundColor DarkGray
        if ($wasmInSubpackage -and (Test-Path -LiteralPath $mainPackManifest)) {
            # 适配层没有根计划表时，靠 pack.json 的 layout 判断内核根（pack-wasm）。
            $firstScreen = Get-Content -LiteralPath $mainPackManifest -Raw -Encoding UTF8 | ConvertFrom-Json
            $layoutObject = $firstScreen.layout
            if ($null -eq $layoutObject) {
                $layoutObject = [pscustomobject]@{}
                $firstScreen | Add-Member -NotePropertyName layout -NotePropertyValue $layoutObject -Force
            }
            $layoutObject | Add-Member -NotePropertyName subpackage_root -NotePropertyValue $packDirName -Force
            $layoutObject | Add-Member -NotePropertyName wasm_root -NotePropertyValue $packWasmDirName -Force
            [IO.File]::WriteAllText($mainPackManifest, ($firstScreen | ConvertTo-Json -Depth 8),
                (New-Object Text.UTF8Encoding($false)))
            # boot 元数据：计划表不再只描述"资源怎么分"，还明确写出"启动闭包是哪些根/组/条目"。
            $planPath = Join-Path $packRoot "bundle_plan.json"
            if (Test-Path -LiteralPath $planPath) {
                $plan = Get-Content -LiteralPath $planPath -Raw -Encoding UTF8 | ConvertFrom-Json
                $mainGroups = @($plan.groups | Where-Object { $_.unit -eq "main" } | ForEach-Object { $_.name })
                $bootEntries = @($firstScreen.entries)
                $bootObject = [pscustomobject]@{
                    roots = @($mainPackDirName, $packWasmDirName)
                    groups = $mainGroups
                    wasm = "$packWasmDirName/game_web_bg.wasm"
                    entries = $bootEntries
                }
                $plan | Add-Member -NotePropertyName boot -NotePropertyValue $bootObject -Force
                # 计划表按**紧凑** JSON 写出：它是逐条列出 1,357 个路径的明文表，按缩进排版时
                # 单是空白就占 ~100 KB，而 launcher 根那一份是算进微信主包 4 MB 配额的。
                [IO.File]::WriteAllText($planPath, ($plan | ConvertTo-Json -Depth 12 -Compress),
                    (New-Object Text.UTF8Encoding($false)))
                $rootPlan = $plan | ConvertTo-Json -Depth 12 -Compress | ConvertFrom-Json
                $rootPlan | Add-Member -NotePropertyName launcher_root -NotePropertyValue $true -Force
                [IO.File]::WriteAllText((Join-Path $distRoot "bundle_plan.json"),
                    ($rootPlan | ConvertTo-Json -Depth 12 -Compress), (New-Object Text.UTF8Encoding($false)))
            }
        }
        if (-not (Test-Path -LiteralPath $codePackageMap)) {
            throw "代码包改名映射没有被生成: $codePackageMap"
        }
        # 逻辑路径 → 宿主文件名。键的域与清单 entries 相同（pack 相对路径，`/` 分隔）。
        $script:codePackageRenamed = @{}
        $mapObject = Get-Content -LiteralPath $codePackageMap -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($mapObject.renamed) {
            foreach ($property in $mapObject.renamed.PSObject.Properties) {
                $script:codePackageRenamed[$property.Name] = [string]$property.Value
            }
        }
        Write-Host ("==> 代码包后缀合规：{0} 个文件需要改名（后缀不在微信上传白名单内，如 .lua/.toml/.md）" -f $script:codePackageRenamed.Count) -ForegroundColor DarkGray
    }

