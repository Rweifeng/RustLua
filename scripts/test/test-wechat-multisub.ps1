# 微信多分包根端到端用例（P0-A：boot 前预挂全部非主包根）。
#
# 从一份**已发布**的微信 launcher 复制出临时夹具，把计划表里第一个非主包组改成
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
    # 提升哪个组**不该写死**：`bundles.json` 改过一次分组（audio 从 sub 变成别的键），
    # 写死组名会让这个用例静默跳过（`reason=no-audio-group`），于是"多根"这条覆盖就没了。
    # 这里取第一个非 main、非 cdn、且还不是 `sub:audio` 的组来扮演具名分包。
    $promoted = @($plan.groups | Where-Object {
            $_.unit -ne "cdn" -and $_.unit -ne "main" -and $_.unit -ne "sub:audio"
        } | Select-Object -First 1)
    if ($plan.groups | Where-Object { $_.unit -eq "sub:audio" }) {
        # 产物已经带具名分包：直接用现成的，不需要再提升。
        $promoted = @($plan.groups | Where-Object { $_.unit -eq "sub:audio" } | Select-Object -First 1)
    }
    if ($promoted.Count -lt 1) {
        Write-Host "wechat_multisub_test_skipped reason=no-promotable-group launcher=$launcherRoot" -ForegroundColor DarkYellow
        exit 0
    }
    $promoted[0].unit = "sub:audio"
    # 手改了 groups，计划表自校验的 checksum 必然失配：显式清空（= "手改过、无自校验" 的合法旧形态），
    # 否则 `assetc closure` 会在解析阶段就拒绝它，测不到后面真正要测的容器闭包。
    $plan.checksum = ""
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

    # 容器内容闭包：拆根之后每条逻辑路径仍必须能从**它那个根**的容器里取出来。
    # 上面两道门只证明"容器文件在"，不打开容器；这一门由 assetc 实现（索引解析只有一份）。
    $assetcExe = $null
    foreach ($candidate in @(
            (Join-Path $repo "engine\target\release\assetc.exe"),
            (Join-Path $repo "engine\target\debug\assetc.exe")
        )) {
        if (Test-Path -LiteralPath $candidate) { $assetcExe = $candidate; break }
    }
    if (-not $assetcExe) {
        Write-Host "wechat_multisub_test_assetc_missing reason=skip-closure（先 cargo build -p assetc --release）" -ForegroundColor DarkYellow
    }
    else {
        $savedEap = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $closureOut = & $assetcExe closure $tempLauncher 2>&1
            $closureExit = $LASTEXITCODE
        }
        finally { $ErrorActionPreference = $savedEap }
        if ($closureExit -ne 0) {
            throw ("拆根后容器闭包不成立（assetc closure exit {0}）：{1}" -f $closureExit, (($closureOut | ForEach-Object { "$_" }) -join "`n"))
        }
        Write-Host (($closureOut | Where-Object { $_ -match '^assetc_closure_ok' } | Select-Object -First 1)) -ForegroundColor DarkGray

        # 反向用例：容器字节被改坏必须判红。没有这一条，上面的断言只能说"跑过了"，
        # 证明不了"它真会拦下来"（本项目反复踩过"门永远绿"）。
        $victim = Get-ChildItem -LiteralPath (Join-Path $tempLauncher "pack-audio\bundles") -File -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -like "*.rlb" -or $_.Name -like "*.rlb.txt" } |
            Select-Object -First 1
        if (-not $victim) { throw "pack-audio\bundles 下没有容器（含 .rlb.txt 改名形态），反向用例无法构造" }
        $originalBytes = [System.IO.File]::ReadAllBytes($victim.FullName)
        try {
            $corrupted = [byte[]]$originalBytes.Clone()
            $corrupted[$corrupted.Length - 1] = $corrupted[$corrupted.Length - 1] -bxor 0xFF
            [System.IO.File]::WriteAllBytes($victim.FullName, $corrupted)
            $ErrorActionPreference = "Continue"
            try {
                $negOut = & $assetcExe closure $tempLauncher 2>&1
                $negExit = $LASTEXITCODE
            }
            finally { $ErrorActionPreference = $savedEap }
            if ($negExit -eq 0) {
                throw ("容器被改坏后 `assetc closure` 仍然通过（假绿）：" + (($negOut | ForEach-Object { "$_" }) -join "`n"))
            }
            Write-Host "wechat_multisub_closure_negative_ok 改坏 $($victim.Name) 后闭包检查判红（exit $negExit）" -ForegroundColor DarkGray
        }
        finally {
            [System.IO.File]::WriteAllBytes($victim.FullName, $originalBytes)
        }
    }

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
