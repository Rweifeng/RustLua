# -----------------------------------------------------------------------------
# 打包阶段：资源包导出与瘦身（pack/、CDN 镜像）
# 自 scripts/build/package.ps1 原样迁出（原 L1706-L1784，2026-11-02 按功能拆分）。
# 执行语义：本文件被入口 dot-source，跑在入口的脚本作用域里——阶段之间共享变量、
#   ErrorActionPreference 与 throw 行为都跟拆分前一致。它不是函数，别当函数调。
#   PSScriptRoot 已统一换成入口设置的 packageScriptRoot（本文件不在 scripts/build/ 下）。
# -----------------------------------------------------------------------------

    # ------------------------------------------------- 资源包（pack/）导出与瘦身
    # 微信上传分两块：代码包（launcher/）与 CDN 上的资源包（assets/）。把资源放到 CDN 就需要
    # 一份与包内 manifest **完全对应**的独立资源树；瘦首包（-AssetPack remote）则进一步把
    # 包内 pack/ 裁到只剩骨架，改由远端提供重资源。
    $script:cdnPackFullFiles = 0
    $script:cdnPackFullBytes = 0
    if ($Platform -eq "wechat-minigame") {
        # 1) 完整资源树先落到 <发布根>/assets（必须早于裁剪：裁完包内就只剩骨架了）。
        #    它和 launcher/ 在同一个 staging 里 → 首包与资源包一次原子交换同时生效。
        #    只有 remote 模式才有 assets/：full 模式的资源本来就在包内，再复制一份是纯浪费。
        if ($assetPackStage) {
            New-Item -ItemType Directory -Force -Path $assetPackStage | Out-Null
            Copy-Item -LiteralPath $packRoot -Destination (Join-Path $assetPackStage $packDirName) -Recurse -Force
            $fullFiles = (Get-ChildItem -LiteralPath (Join-Path $assetPackStage $packDirName) -Recurse -File -Force | Measure-Object).Count
            $fullBytes = (Get-ChildItem -LiteralPath (Join-Path $assetPackStage $packDirName) -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
            $sourceFiles = (Get-ChildItem -LiteralPath $packRoot -Recurse -File -Force | Measure-Object).Count
            $sourceBytes = (Get-ChildItem -LiteralPath $packRoot -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
            # 资源树必须与包内那份逐一对应：数量/字节不一致就意味着上传 CDN 的东西和
            # manifest 里的校验和对不上，远端校验会把它们全部丢弃（宁可这里失败）。
            if ($fullFiles -ne $sourceFiles -or $fullBytes -ne $sourceBytes) {
                throw ("资源包导出不完整: {0} 文件/{1} 字节，源 pack/ 是 {2} 文件/{3} 字节" -f $fullFiles, $fullBytes, $sourceFiles, $sourceBytes)
            }
            $script:cdnPackFullFiles = $fullFiles
            $script:cdnPackFullBytes = $fullBytes
        }
        # 1b) -CdnPackDir 镜像：无论 remote/full，都是"把 pack/ 复制一份到该目录"。
        #     必须在下面的瘦身裁剪**之前**做，否则镜像里只剩骨架。
        if ($cdnPackMirror) {
            $script:cdnMirrorStage = New-ReleaseStaging -FinalRoot $cdnPackMirror
            Copy-Item -LiteralPath $packRoot -Destination (Join-Path $script:cdnMirrorStage "pack") -Recurse -Force
            $mirrorFiles = (Get-ChildItem -LiteralPath (Join-Path $script:cdnMirrorStage "pack") -Recurse -File -Force | Measure-Object).Count
            $mirrorBytes = (Get-ChildItem -LiteralPath (Join-Path $script:cdnMirrorStage "pack") -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
            $sourceFiles = (Get-ChildItem -LiteralPath $packRoot -Recurse -File -Force | Measure-Object).Count
            $sourceBytes = (Get-ChildItem -LiteralPath $packRoot -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
            if ($mirrorFiles -ne $sourceFiles -or $mirrorBytes -ne $sourceBytes) {
                throw ("资源包镜像导出不完整: {0} 文件/{1} 字节，源 pack/ 是 {2} 文件/{3} 字节" -f $mirrorFiles, $mirrorBytes, $sourceFiles, $sourceBytes)
            }
        }
        # 2) 瘦首包：pack/ 只留根目录下的文件（骨架），资源目录整体移除。
        if ($AssetPack -eq "remote") {
            $removedDirs = @()
            foreach ($entry in Get-ChildItem -LiteralPath $packRoot -Force) {
                if ($entry.PSIsContainer) {
                    Remove-Item -LiteralPath $entry.FullName -Recurse -Force
                    $removedDirs += $entry.Name
                }
            }
            # 骨架缺一不可：瘦首包形态下 manifest/project.toml 仍是运行入口，缺了就只能白屏
            # （不带 -WasmInSubpackage 时本形态没有分包声明，所以也没有 pack/game.js 入口；
            #   带了就有，见上面 game.json 与入口文件那两段）。
            foreach ($skeleton in @("project.toml", "manifest.txt")) {
                $skeletonPath = Join-Path $packRoot $skeleton
                if (-not (Test-Path -LiteralPath $skeletonPath) -or (Get-Item -LiteralPath $skeletonPath).Length -le 0) {
                    throw "瘦首包缺少骨架文件: $skeletonPath"
                }
            }
            if (-not $assetPackStage) {
                throw "-AssetPack remote 必须导出资源包（<发布根>/assets），否则被裁掉的资源无处可取。"
            }
            # 裁剪白名单是"根目录下的所有文件"：将来 assetc 若把大资源放在 pack/ 根下，
            # 这里必须报错要求更新白名单，而不是静默发出一个体积没降下来的"瘦"包。
            $thinBytes = (Get-ChildItem -LiteralPath $packRoot -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
            # 内核入分包时它在独立的 `pack-wasm/game_web_bg.wasm`（不在 pack/ 骨架里）；
            # 这里仍按旧位置回退检查，保证瘦首包 512 KB 门只量骨架。
            $thinCoreBytes = 0
            if ($wasmInSubpackage) {
                $thinCorePath = Join-Path $packWasmRoot "game_web_bg.wasm"
                if (Test-Path -LiteralPath $thinCorePath) { $thinCoreBytes = (Get-Item -LiteralPath $thinCorePath).Length }
            }
            $thinSkeletonBytes = $thinBytes - $thinCoreBytes
            if ($thinSkeletonBytes -gt 512KB) {
                throw ("瘦首包裁剪未生效: pack/ 骨架仍有 {0:N0} KB（pack/ 根目录下出现了大文件，需要更新骨架白名单）" -f ($thinSkeletonBytes / 1KB))
            }
            $thinCoreNote = if ($thinCoreBytes -gt 0) { "，内核 {0:N0} KB 已放进分包" -f ($thinCoreBytes / 1KB) } else { "" }
            Write-Host ("==> 瘦首包: pack/ 骨架 {0:N0} KB（移除目录: {1}）{2}，重资源走远端 {3}" -f `
                ($thinSkeletonBytes / 1KB), ($removedDirs -join ','), $thinCoreNote, $RemotePackBaseUrl) -ForegroundColor Cyan
        }
    }

