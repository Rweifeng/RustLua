# -----------------------------------------------------------------------------
# 打包阶段：原子交换（staging -> 发布根，成功后打印体积）
# 自 scripts/build/package.ps1 原样迁出（原 L2297-L2329，2026-11-02 按功能拆分）。
# 执行语义：本文件被入口 dot-source，跑在入口的脚本作用域里——阶段之间共享变量、
#   ErrorActionPreference 与 throw 行为都跟拆分前一致。它不是函数，别当函数调。
#   PSScriptRoot 已统一换成入口设置的 packageScriptRoot（本文件不在 scripts/build/ 下）。
# -----------------------------------------------------------------------------

    Write-Host "==> Atomic release swap" -ForegroundColor Cyan
    $script:swapAttempted = $true
    $swap = Complete-ReleaseSwap -FinalRoot $finalRoot -StageRoot $stageRoot
    # $distRoot 回到最终目录下的 launcher（微信）/ 发布根（其它平台）：launcher/ 与 assets/ 是
    # 同一次交换落地的，所以此刻首包与资源包一定互相配套。
    $distRoot = if ($launcherSubdir) { Join-Path $finalRoot $launcherSubdir } else { $finalRoot }

    $packRoot = Join-Path $distRoot $packDirName
$mainPackRoot = Join-Path $distRoot $mainPackDirName
    $packWasmRoot = Join-Path $distRoot $packWasmDirName
    Write-Host ("==> Release swapped in: {0} entries replaced, {1} entries moved aside" -f $swap.MovedIn.Count, $swap.MovedAside.Count) -ForegroundColor Cyan

    # -CdnPackDir 指定的镜像目录同样走 staging + 原子交换（避免镜像停在半更新状态）。
    if ($script:cdnMirrorStage) {
        if (-not (Test-Path -LiteralPath $script:cdnMirrorStage)) {
            throw "资源包镜像导出失败: staging 不存在（$($script:cdnMirrorStage)）"
        }
        $mirrorSwap = Complete-ReleaseSwap -FinalRoot $cdnPackMirror -StageRoot $script:cdnMirrorStage
        Write-Host ("==> 资源包镜像已更新: {0}" -f $cdnPackMirror) -ForegroundColor DarkGray
    }

    if ($cdnPackRoot) {
        if (-not (Test-Path -LiteralPath $cdnPackRoot)) {
            throw "资源包导出失败: $cdnPackRoot 不存在（首包与资源包必须在同一次发布里产出）"
        }
        Write-Host ("wechat_asset_pack_ok dir={0} files={1} pack_kb={2:N0} cdn_base={3}" -f `
            $cdnPackRoot, $script:cdnPackFullFiles, ($script:cdnPackFullBytes / 1KB), $RemotePackBaseUrl) -ForegroundColor Green
        Write-Host ("==> 资源包已就绪：把 {0} 里的**内容**上传到 CDN，使 packBaseUrl 指向 {1}" -f $cdnPackRoot, $RemotePackBaseUrl) -ForegroundColor Cyan
    }

    Write-Host "Release package ready: $finalRoot" -ForegroundColor Green
    $total = Get-ChildItem -Recurse -File $distRoot | Measure-Object -Property Length -Sum
    Write-Host ("Package size: {0:N1} KB, {1} files ({2})" -f ($total.Sum / 1KB), $total.Count, $(if ($launcherSubdir) { "launcher/, 上传微信的代码包" } else { "release root" }))
