# -----------------------------------------------------------------------------
# 打包阶段：发布目录准备（锁检查、残留清理、staging 命名）
# 自 scripts/build/package.ps1 原样迁出（原 L464-L521，2026-11-02 按功能拆分）。
# 执行语义：本文件被入口 dot-source，跑在入口的脚本作用域里——阶段之间共享变量、
#   ErrorActionPreference 与 throw 行为都跟拆分前一致。它不是函数，别当函数调。
#   PSScriptRoot 已统一换成入口设置的 packageScriptRoot（本文件不在 scripts/build/ 下）。
# -----------------------------------------------------------------------------

$finalRoot = $distRoot
# **发布根一确定就先探锁**：否则要等几分钟打包完、到交换那一步才报错（实测那次就是这么白干的）。
# 探针与交换做的是同一个操作（目录重命名），所以它红了，交换也一定会红。
Assert-ReleaseDirNotLocked -FinalRoot $finalRoot
$staleRemoved = Clear-StaleReleaseStaging -FinalRoot $finalRoot -MaxAgeHours 24
foreach ($stale in $staleRemoved) {
    Write-Host "==> Removed stale release staging: $stale" -ForegroundColor DarkGray
}
# 微信小游戏的发布根下分两个兄弟目录（约定的产物形态）：
#   launcher/  首包：导入微信开发者工具 / 上传微信的内容（含 pack/ 与 mainpack/、remote-assets.json）
#   assets/    资源包：整目录上传 CDN，形如 <发布根>/assets/pack-sub/**（只有 -AssetPack remote 才有）
# 其它平台没有这个划分，产物直接放在发布根下。
# 关键：两者都在**同一个 staging** 里生成，所以首包与资源包是一次原子交换同时生效——
# 否则"新首包 + 旧资源包"这种半更新状态只能等远端校验和事后暴露（真机表现为整批丢资源）。
$launcherSubdir = if ($Platform -eq "wechat-minigame") { "launcher" } else { "" }
$exportsAssetPack = ($Platform -eq "wechat-minigame") -and ($AssetPack -eq "remote")
# 分包形态：首屏资源从 pack/ 切到兄弟目录 mainpack/。**必须在复制平台文件之前定下来**，
# 因为 launcher 下的 game.json / 入口文件是否要处理分包声明，取决于这个布尔值。
$isSubpackagePack = ($Platform -eq "wechat-minigame") -and ($AssetPack -eq "subpackage")
# 内核入分包：subpackage 形态无条件搬；remote 形态加 -WasmInSubpackage 才搬（第 53 轮）。
# 判据只用这一个布尔值——"game.json 要不要声明分包""pack/ 要不要写 game.js 入口"
# "体积门按哪种算法算"都必须与它一致，否则会出现"声明了分包却没入口"这类开发者工具直接拒开的状态。
$wasmInSubpackage = $isSubpackagePack -or `
    (($Platform -eq "wechat-minigame") -and ($AssetPack -eq "remote") -and $WasmInSubpackage)
# -CdnPackDir：除发布根下的 assets/ 之外**再**导出一份到该目录（给直接 rsync / s3 同步用）。
$cdnPackMirror = ""
if (-not [string]::IsNullOrWhiteSpace($CdnPackDir)) {
    $cdnPackMirror = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($CdnPackDir)
}
$stageRoot = New-ReleaseStaging -FinalRoot $finalRoot
$distRoot = if ($launcherSubdir) { Join-Path $stageRoot $launcherSubdir } else { $stageRoot }
$packRoot = Join-Path $distRoot $packDirName
$mainPackRoot = Join-Path $distRoot $mainPackDirName
$packWasmRoot = Join-Path $distRoot $packWasmDirName
# 资源包根 = `<发布根>/assets/<packDirName>`（微信 = `assets/pack-sub/`：导出目录名与分包
# root 同名）。旧值停在 `assets` 那一层，于是收尾断言去找 `assets/pack` ——**每一份 remote 产物**
# 都会在原子交换之后抛 exit 1（产物其实已经换进去了，工具却报失败）。2026-10-20 实测。
$cdnPackRoot = if ($exportsAssetPack) { Join-Path (Join-Path $finalRoot "assets") $packDirName } else { "" }
$assetPackStage = if ($exportsAssetPack) { Join-Path $stageRoot "assets" } else { "" }
$script:cdnMirrorStage = ""
if ($cdnPackMirror) {
    foreach ($stale in (Clear-StaleReleaseStaging -FinalRoot $cdnPackMirror -MaxAgeHours 24)) {
        Write-Host "==> Removed stale asset-pack staging: $stale" -ForegroundColor DarkGray
    }
    Write-Host "==> 资源包镜像目录: $cdnPackMirror（权威位置是发布根下的 assets/）" -ForegroundColor Cyan
}
if ($exportsAssetPack) {
    Write-Host "==> 资源包随发布一起产出: $cdnPackRoot（形如 <发布根>/assets/pack-sub/**，整目录上传 CDN）" -ForegroundColor Cyan
}
$script:swapAttempted = $false
Write-Host "==> Staging release at $stageRoot (atomic swap on success)" -ForegroundColor Cyan
$timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found. Install Rust stable and ensure cargo is on PATH."
}

New-Item -ItemType Directory -Force -Path $distRoot | Out-Null
