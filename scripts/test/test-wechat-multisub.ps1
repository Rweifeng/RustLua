# 微信多分包根端到端用例（P0-A：boot 前预挂全部非主包根）。
#
# 从一份**已发布**的微信 launcher 复制出临时夹具，把 `bundles.json` 里的 audio 组改成
# `sub:audio`，用 bundle-layout.js 真正拆出 `pack-audio/`，再用无头 harness 跑完整启动：
#   · 适配层必须读到 launcher 根的 bundle_plan.json；
#   · boot 前必须把 pack + pack-audio 都 loadSubpackage 过；
#   · 引擎 mount_from_reader 必须能读全 manifest 里的容器（含 pack-audio 的容器）。
#
# 这里复制 `plugins/wechat-minigame/{wechat-adapter,bundle-roots}.js` 到夹具，测的是**当前源码**
# 而不是旧产物里的 minified 副本；其余文件（wasm / startup-branding / game.js）复用产物。
#
# 用法：
#   powershell -NoProfile -File scripts\test\test-wechat-multisub.ps1
#   powershell -NoProfile -File scripts\test\test-wechat-multisub.ps1 -Launcher <launcher 目录> -Frames 5
#
# 没有可用产物时打印 `wechat_multisub_test_skipped` 并 exit 0（本机/CI 还没打包时不算失败）。

[CmdletBinding()]
param(
    [string]$Launcher = "",
    [int]$Frames = 5
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) { throw "需要 node 运行多根布局与无头 harness" }

function Resolve-Launcher([string]$candidate) {
    if ([string]::IsNullOrWhiteSpace($candidate)) { return $null }
    if (-not (Test-Path -LiteralPath $candidate)) { return $null }
    $path = (Resolve-Path -LiteralPath $candidate).Path
    if (Test-Path -LiteralPath (Join-Path $path "launcher")) { return (Join-Path $path "launcher") }
    return $path
}

if ([string]::IsNullOrWhiteSpace($Launcher)) {
    $Launcher = Join-Path $repo "examples\game_jhyt\builds\wechat-minigame\launcher"
}
$launcherRoot = Resolve-Launcher $Launcher
if (-not $launcherRoot) {
    Write-Host "wechat_multisub_test_skipped reason=no-launcher path=$Launcher" -ForegroundColor DarkYellow
    exit 0
}
$sourcePlan = $null
foreach ($candidate in @(
    (Join-Path $launcherRoot "bundle_plan.json"),
    (Join-Path $launcherRoot "pack-sub\bundle_plan.json"),
    (Join-Path $launcherRoot "pack\bundle_plan.json")
)) {
    if (Test-Path -LiteralPath $candidate) { $sourcePlan = $candidate; break }
}
if (-not $sourcePlan) {
    Write-Host "wechat_multisub_test_skipped reason=no-bundle-plan launcher=$launcherRoot" -ForegroundColor DarkYellow
    exit 0
}

$temp = Join-Path $env:TEMP ("rustlua-wechat-multisub-" + [guid]::NewGuid().ToString("N"))
$tempLauncher = Join-Path $temp "launcher"
$utf8 = New-Object System.Text.UTF8Encoding($false)

function Write-Utf8([string]$path, [string]$text) {
    [System.IO.File]::WriteAllText($path, $text, $utf8)
}

function Read-Utf8([string]$path) {
    return [System.IO.File]::ReadAllText($path, $utf8)
}

try {
    New-Item -ItemType Directory -Path $temp | Out-Null
    Copy-Item -LiteralPath $launcherRoot -Destination $tempLauncher -Recurse -Force

    # 旧产物可能仍用 subpack/ 目录名；统一成 pack/，后续布局工具只认 pack/。
    $packRoot = Join-Path $tempLauncher "pack-sub"
    $legacyPack = Join-Path $tempLauncher "subpack"
    if (-not (Test-Path -LiteralPath $packRoot) -and (Test-Path -LiteralPath $legacyPack)) {
        Rename-Item -LiteralPath $legacyPack -NewName "pack-sub"
        foreach ($name in @("game.json", "pack.json")) {
            $file = Join-Path $tempLauncher $name
            if (-not (Test-Path -LiteralPath $file)) { continue }
            $text = Read-Utf8 $file
            $text = $text.Replace('"subpack"', '"pack-sub"').Replace('subpack/', 'pack-sub/')
            Write-Utf8 $file $text
        }
    }
    if (-not (Test-Path -LiteralPath $packRoot)) {
        throw "launcher 里没有 pack/：$tempLauncher"
    }

    # 计划表放回 pack/，并把 mainpack/ 里已切走的 main 容器收回 pack/bundles/，
    # 让 bundle-layout 从「单根 + 计划表」的完整形态重新拆一次。
    $planPath = Join-Path $packRoot "bundle_plan.json"
    if (-not (Test-Path -LiteralPath $planPath)) {
        Copy-Item -LiteralPath $sourcePlan -Destination $planPath -Force
    }
    $mainBundles = Join-Path $tempLauncher "pack\bundles"
    if (Test-Path -LiteralPath $mainBundles) {
        $packBundles = Join-Path $packRoot "bundles"
        New-Item -ItemType Directory -Force -Path $packBundles | Out-Null
        Get-ChildItem -LiteralPath $mainBundles -File -Force | ForEach-Object {
            Move-Item -LiteralPath $_.FullName -Destination (Join-Path $packBundles $_.Name) -Force
        }
    }

    $plan = Read-Utf8 $planPath | ConvertFrom-Json
    $audioGroups = @($plan.groups | Where-Object { $_.name -eq "audio" })
    if ($audioGroups.Count -ne 1) {
        Write-Host "wechat_multisub_test_skipped reason=no-audio-group launcher=$launcherRoot" -ForegroundColor DarkYellow
        exit 0
    }
    $audioGroups[0].unit = "sub:audio"
    Write-Utf8 $planPath ($plan | ConvertTo-Json -Depth 10)

    # 由计划表生成 layout（与 assetc layout 同形；本用例不依赖本机 assetc 是否为含 layout 的新版）。
    $byRoot = [ordered]@{}
    $entries = 0
    $totalBytes = 0
    foreach ($group in $plan.groups) {
        if ($group.unit -eq "cdn") { continue }
        if ($group.unit -eq "main") { $rootName = "pack" }
        elseif ($group.unit -eq "sub") { $rootName = "pack-sub" }
        elseif ($group.unit -like "sub:*") { $rootName = "pack-" + $group.unit.Substring(4) }
        else { throw "不认识的 unit: $($group.unit)" }
        if (-not $byRoot.Contains($rootName)) {
            $byRoot[$rootName] = [pscustomobject]@{ unit = $group.unit; root = $rootName; bundles = @(); entries = 0; bytes = 0 }
        }
        $byRoot[$rootName].bundles += [string]$group.bundle
        $byRoot[$rootName].entries += [int]$group.entries
        $byRoot[$rootName].bytes += [int]$group.bundle_bytes
        $entries += [int]$group.entries
        $totalBytes += [int]$group.bundle_bytes
    }
    $roots = @($byRoot.Values)
    $layout = [pscustomobject]@{
        schema = 1
        entries = $entries
        total_bytes = $totalBytes
        roots = $roots
    }
    $layoutFile = Join-Path $tempLauncher ".bundle-layout.json"
    Write-Utf8 $layoutFile ($layout | ConvertTo-Json -Depth 8)

    $layoutTool = Join-Path $repo "scripts\build\bundle-layout.js"
    $mountCheck = Join-Path $repo "scripts\build\check-bundle-mounts.js"
    & $node.Source $layoutTool --file $layoutFile --apply $tempLauncher | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "bundle-layout --apply 失败（exit $LASTEXITCODE）" }
    & $node.Source $layoutTool --verify $tempLauncher | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "bundle-layout --verify 失败（exit $LASTEXITCODE）" }
    & $node.Source $mountCheck --launcher $tempLauncher | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "check-bundle-mounts 失败（exit $LASTEXITCODE）" }

    # 用当前源码覆盖夹具里的适配层：本用例要钉的是**这一笔**的 pre-mount 行为。
    Copy-Item -LiteralPath (Join-Path $repo "plugins\wechat-minigame\wechat-adapter.js") -Destination (Join-Path $tempLauncher "wechat-adapter.js") -Force
    Copy-Item -LiteralPath (Join-Path $repo "plugins\wechat-minigame\bundle-roots.js") -Destination (Join-Path $tempLauncher "bundle-roots.js") -Force

    $harness = Join-Path $repo "scripts\verify\wechat-minigame-harness.js"
    $output = & $node.Source $harness $tempLauncher --frames $Frames --subpackage ok 2>&1
    $harnessExit = $LASTEXITCODE
    $text = ($output | ForEach-Object { "$_" }) -join "`n"
    if ($harnessExit -ne 0) {
        Write-Host $text
        throw "无头 harness 失败（exit $harnessExit）"
    }
    if ($text -notmatch "wechat_minigame_harness_ok") {
        Write-Host $text
        throw "无头 harness 没有输出 wechat_minigame_harness_ok"
    }
    if ($text -notmatch "多根布局 pack,pack-audio,pack-sub") {
        Write-Host $text
        throw "适配层没有读到 pack,pack-audio,pack-sub 多根布局"
    }
    if ($text -notmatch "补挂 pack-audio 容器" -and $text -notmatch "pack-audio 分包已挂载") {
        Write-Host $text
        throw "pack-audio 没有挂载/补挂容器（新内核看 partial+补挂，老内核看全量预挂）"
    }
    $loads = 0
    if ($text -match "subpackage_loads=(\d+)") { $loads = [int]$Matches[1] }
    if ($loads -lt 2) {
        Write-Host $text
        throw "subpackage_loads=$loads（期望 >=2：pack-sub + pack-audio）"
    }
    Write-Host ("wechat_multisub_test_ok roots={0} subpackage_loads={1} launcher={2}" -f $roots.Count, $loads, $launcherRoot) -ForegroundColor Green
}
finally {
    if (Test-Path -LiteralPath $temp) {
        if ($env:RUSTLUA_KEEP_TEMP) {
            Write-Host "wechat_multisub_test_temp=$temp" -ForegroundColor DarkYellow
        } else {
            Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}
