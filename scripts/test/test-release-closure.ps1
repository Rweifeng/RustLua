#Requires -Version 5.1
<#
.SYNOPSIS
发布闭包回归：合成一份 grouped 产物，验证 `assetc closure` 会通过、也会判红。

.DESCRIPTION
`assetc closure` 是"容器文件都在，但计划里的某条资源没进容器"的唯一拦截点。它此前只在
**发布机上真有产物**时才跑（`apply-bundle-layout.ps1` / `verify-wechat-minigame.ps1`），
于是 CI 与日常 `verify.ps1` 都覆盖不到——本用例把它变成任何机器上都能秒级复现的门：
不需要 wasm、不需要浏览器、不需要 GPU，只要一个 `assetc` 可执行文件。

九条断言（少一条都不算测到）：
  1. 正例：正常产物 → exit 0，且摘要行 `assetc_closure_ok` 出现；
  2. 反例·容器损坏：改容器最后一个字节 → 必须 exit ≠ 0；
  3. 反例·缺容器：删掉一个容器 → 必须点名"容器不在"；
  4. 反例·多余容器：塞一个计划外容器 → 必须点名"计划外的容器"；
  5. 黄金表落地：跨端路径表（`scripts/test/fixtures/asset-routing-golden.json`）里的每条路径，
     在真实 grouped 产物里都必须进计划且能过闭包；
  6. **平台形态**：`assetc layout` → `bundle-layout.js --apply` 真搬真改名（`.rlb` → `.rlb.txt`）→
     `check-bundle-mounts.js` → 多根 `assetc closure`（必须报 `roots=2`，不是退回单根）；
  7. 反例·多根缺件：删掉分包容器 → 必须点名 `pack-sub/`；
  8. **keep**：`--keep` 命中的文件必须留在目录里（计划表记录具体路径），删掉它必须判红；
  9. **cdn**：容器在包内时**必须就地核对**（`cdn_local=1`，不能因为组名是 cdn 就要求 `--cdn`）；
     把容器搬到镜像目录后不给 `--cdn` 必须判"未检查"（跳过 ≠ 通过），给了就逐组核对，
     删掉远端容器必须点名 `<cdn>` 缺件。
只有正例的测试证明不了"这道门会拦"——本项目反复踩过"门永远绿"；只有单根正例的测试证明不了
"这道门在**平台的目录坐标**下仍然成立"（拆根与改名都会换掉容器的物理位置）；只有本地组的测试
证明不了远端与 keep 这两条"不在容器里/不在发布根里"的分支没有被写成静默跳过。

.PARAMETER Assetc
assetc 可执行文件路径。缺省依次找 `engine\target\release\assetc.exe`、`debug\assetc.exe`。

.PARAMETER RequireAssetc
找不到 assetc 时**失败**而不是跳过。CI 用它（同一步里刚构建过，缺了就是配置错，
静默跳过会把"门没跑"伪装成"门通过"）。

.PARAMETER KeepTemp
保留临时目录，便于失败后手工复现。

.EXAMPLE
powershell -NoProfile -File scripts\test\test-release-closure.ps1
#>
[CmdletBinding()]
param(
    [string]$Assetc = "",
    [switch]$RequireAssetc,
    [switch]$KeepTemp
)

$ErrorActionPreference = "Stop"
# 断言里有中文（「容器不在」「计划外的容器」），而 5.1 控制台默认按 ANSI 解码原生命令输出：
# assetc 写 UTF-8，不显式设 UTF-8 就会把正确报错读成乱码、误判成"没点名原因"。
try { [Console]::OutputEncoding = New-Object Text.UTF8Encoding($false) } catch { }
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not (Test-Path (Join-Path $repo "engine\Cargo.toml"))) {
    throw "找不到仓库根（从 $PSScriptRoot 上溯两层是 $repo）"
}

function Resolve-AssetcExe {
    param([string]$Explicit, [switch]$Required)
    $candidates = @()
    if ($Explicit) { $candidates += $Explicit }
    foreach ($profile in @("release", "debug")) {
        $candidates += (Join-Path $repo "engine\target\$profile\assetc.exe")
    }
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) { return (Resolve-Path -LiteralPath $candidate).Path }
    }
    if ($Required) {
        throw "找不到 assetc 可执行文件（试过：$($candidates -join ', ')）；请先 cargo build -p assetc --release"
    }
    return $null
}

$assetcExe = Resolve-AssetcExe -Explicit $Assetc -Required:$RequireAssetc
if (-not $assetcExe) {
    Write-Host "release_closure_test_skipped reason=no-assetc (先 cargo build -p assetc，或用 -Assetc 指定)" -ForegroundColor DarkYellow
    exit 0
}

# 一次调用 → (exit, 文本)。PS 5.1 下原生命令写 stderr 会抛 NativeCommandError，
# 这里显式降级 ErrorActionPreference 再还原，避免"命令本身失败"和"输出走 stderr"混为一谈。
function Invoke-Assetc {
    param([string[]]$Arguments)
    $saved = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $lines = & $assetcExe @Arguments 2>&1 | ForEach-Object { "$_" }
        $code = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $saved
    }
    return [pscustomobject]@{ ExitCode = $code; Text = ($lines -join "`n") }
}

$tempRoot = Join-Path $env:TEMP ("rustlua-release-closure-" + [guid]::NewGuid().ToString("N"))
$projectDir = Join-Path $tempRoot "project"
$releaseDir = Join-Path $tempRoot "release"
New-Item -ItemType Directory -Path $projectDir, (Join-Path $projectDir "scripts"), (Join-Path $projectDir "scenes"), (Join-Path $projectDir "assets\textures") -Force | Out-Null

try {
    # ---------- 1. 合成工程（与 engine/crates/assetc/src/closure.rs 的测试夹具同形） ----------
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText(
        (Join-Path $projectDir "project.toml"),
        "[project]`nname = ""release-closure""`nversion = ""0.1.0""`nengine = ""RustLua""`nmain_script = ""scripts/main.lua""`nstartup_scene = ""scenes/main.scene""`n",
        $utf8)
    [System.IO.File]::WriteAllText((Join-Path $projectDir "scripts\main.lua"), "function update() end`n", $utf8)
    [System.IO.File]::WriteAllText((Join-Path $projectDir "scenes\main.scene"), "scene_version=2`nname=main`n", $utf8)
    # 1×1 红点 PNG（70 字节）：内容不重要，重要的是"有一个真正的二进制资源进容器"。
    $redPng = [byte[]](
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82)
    [System.IO.File]::WriteAllBytes((Join-Path $projectDir "assets\textures\red.png"), $redPng)

    # ---------- 2. 打一份 grouped 产物（`assetc package` 的原始输出，没有 pack/ 那一层） ----------
    $packed = Invoke-Assetc @("package", $projectDir, "-o", $releaseDir, "--bundle", "grouped")
    if ($packed.ExitCode -ne 0) {
        throw ("grouped 打包失败（exit $($packed.ExitCode)）：`n" + $packed.Text)
    }
    $planPath = Join-Path $releaseDir "bundle_plan.json"
    if (-not (Test-Path -LiteralPath $planPath)) {
        throw "grouped 打包没有产出 bundle_plan.json：$releaseDir"
    }
    $containers = @(Get-ChildItem -LiteralPath (Join-Path $releaseDir "bundles") -File -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -like "*.rlb" -or $_.Name -like "*.rlb.txt" })
    if ($containers.Count -lt 1) {
        throw "grouped 打包没有产出容器：$releaseDir\bundles"
    }
    $cases = 0

    # ---------- 3. 正例 ----------
    $positive = Invoke-Assetc @("closure", $releaseDir)
    if ($positive.ExitCode -ne 0) {
        throw ("正常产物被闭包检查判红（假红）：`n" + $positive.Text)
    }
    if ($positive.Text -notmatch "assetc_closure_ok") {
        throw ("闭包检查没有打摘要行（无法确认它真的跑过）：`n" + $positive.Text)
    }
    Write-Host (($positive.Text -split "`n" | Where-Object { $_ -match "assetc_closure_ok" } | Select-Object -First 1)) -ForegroundColor DarkGray
    $cases++

    # ---------- 4. 反例·容器损坏 ----------
    $victim = $containers[0]
    $original = [System.IO.File]::ReadAllBytes($victim.FullName)
    try {
        $corrupted = [byte[]]$original.Clone()
        $corrupted[$corrupted.Length - 1] = [byte]([int]$corrupted[$corrupted.Length - 1] -bxor 0xFF)
        [System.IO.File]::WriteAllBytes($victim.FullName, $corrupted)
        $broken = Invoke-Assetc @("closure", $releaseDir)
        if ($broken.ExitCode -eq 0) {
            throw ("容器被改坏后闭包检查仍然通过（假绿）：`n" + $broken.Text)
        }
        Write-Host "release_closure_corrupt_ok 改坏 $($victim.Name) 后判红（exit $($broken.ExitCode)）" -ForegroundColor DarkGray
        $cases++
    }
    finally {
        [System.IO.File]::WriteAllBytes($victim.FullName, $original)
    }

    # ---------- 5. 反例·缺容器（在副本上做，避免影响上面的正例产物） ----------
    $missingDir = Join-Path $tempRoot "release-missing"
    Copy-Item -LiteralPath $releaseDir -Destination $missingDir -Recurse -Force
    $victimForMissing = Get-ChildItem -LiteralPath (Join-Path $missingDir "bundles") -File |
        Where-Object { $_.Name -like "*.rlb" -or $_.Name -like "*.rlb.txt" } |
        Select-Object -First 1
    # 计划里引用了它，文件却没了：这正是"搬运把容器漏在别的根/没上传"的场景。
    Remove-Item -LiteralPath $victimForMissing.FullName -Force
    $missing = Invoke-Assetc @("closure", $missingDir)
    if ($missing.ExitCode -eq 0) {
        throw ("容器缺失后闭包检查仍然通过（假绿）：`n" + $missing.Text)
    }
    if ($missing.Text -notmatch "容器不在") {
        throw ("容器缺失的报错没有点明原因（期望包含「容器不在」）：`n" + $missing.Text)
    }
    Write-Host "release_closure_missing_ok 删掉 $($victimForMissing.Name) 后判红（exit $($missing.ExitCode)）" -ForegroundColor DarkGray
    $cases++

    # ---------- 6. 反例·计划外容器 ----------
    $extraDir = Join-Path $tempRoot "release-extra"
    Copy-Item -LiteralPath $releaseDir -Destination $extraDir -Recurse -Force
    $donor = Get-ChildItem -LiteralPath (Join-Path $extraDir "bundles") -File |
        Where-Object { $_.Name -like "*.rlb" -or $_.Name -like "*.rlb.txt" } |
        Select-Object -First 1
    Copy-Item -LiteralPath $donor.FullName -Destination (Join-Path $extraDir "bundles\ghost-0000000000000000.rlb") -Force
    $extra = Invoke-Assetc @("closure", $extraDir)
    if ($extra.ExitCode -eq 0) {
        throw ("计划外容器没有被拒绝（假绿）：`n" + $extra.Text)
    }
    if ($extra.Text -notmatch "计划外的容器") {
        throw ("计划外容器的报错没有点明原因（期望包含「计划外的容器」）：`n" + $extra.Text)
    }
    Write-Host "release_closure_extra_ok 塞入 ghost 容器后判红（exit $($extra.ExitCode)）" -ForegroundColor DarkGray
    $cases++

    # ---------- 7. 黄金表落地：跨端路径表里的每条路径，必须在真实 grouped 产物里能被解析 ----------
    # 黄金表（scripts/test/fixtures/asset-routing-golden.json）同时被 Rust 单测与 JS 路由测试读取，
    # 但两边都只证明"键写得对"。这一条把它摁到产物上：同一批路径真的打成容器、真的能过
    # `assetc closure`——否则"Rust/JS 认为的键"与"打包器实际写进容器的键"可以各对一半。
    $goldenFile = Join-Path $repo "scripts\test\fixtures\asset-routing-golden.json"
    if (-not (Test-Path -LiteralPath $goldenFile)) { throw "找不到跨端黄金表：$goldenFile" }
    $golden = Get-Content -LiteralPath $goldenFile -Raw -Encoding UTF8 | ConvertFrom-Json
    $goldenProject = Join-Path $tempRoot "golden-project"
    $goldenRelease = Join-Path $tempRoot "golden-release"
    New-Item -ItemType Directory -Path $goldenProject -Force | Out-Null
    [System.IO.File]::WriteAllText(
        (Join-Path $goldenProject "project.toml"),
        "[project]`nname = ""golden-routing""`nversion = ""0.1.0""`nengine = ""RustLua""`nmain_script = ""scripts/main.lua""`nstartup_scene = ""scenes/main.scene""`n",
        $utf8)
    foreach ($case in $golden.paths) {
        $file = Join-Path $goldenProject $case.path
        New-Item -ItemType Directory -Path (Split-Path -Parent $file) -Force | Out-Null
        # 内容按扩展名给"像那么回事"的最小字节：二进制资源走字节写，文本资源走 UTF-8。
        switch ([System.IO.Path]::GetExtension($case.path)) {
            ".png" { [System.IO.File]::WriteAllBytes($file, $redPng) }
            ".jpg" { [System.IO.File]::WriteAllBytes($file, [byte[]](0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46)) }
            ".ogg" { [System.IO.File]::WriteAllBytes($file, [byte[]](0x4F, 0x67, 0x67, 0x53, 0x00, 0x02, 0x00, 0x00)) }
            ".lua" { [System.IO.File]::WriteAllText($file, "function update() end`n", $utf8) }
            ".scene" { [System.IO.File]::WriteAllText($file, "scene_version=2`nname=main`n", $utf8) }
            default { [System.IO.File]::WriteAllText($file, "golden`n", $utf8) }
        }
    }
    $goldenPacked = Invoke-Assetc @("package", $goldenProject, "-o", $goldenRelease, "--bundle", "grouped")
    if ($goldenPacked.ExitCode -ne 0) {
        throw ("黄金表工程打包失败（exit $($goldenPacked.ExitCode)）：`n" + $goldenPacked.Text)
    }
    $goldenPlanFile = Join-Path $goldenRelease "bundle_plan.json"
    if (-not (Test-Path -LiteralPath $goldenPlanFile)) { throw "黄金表产物没有计划表：$goldenPlanFile" }
    $goldenPlan = Get-Content -LiteralPath $goldenPlanFile -Raw -Encoding UTF8 | ConvertFrom-Json
    $planned = @{}
    foreach ($group in $goldenPlan.groups) { foreach ($item in $group.paths) { $planned[$item] = $group.name } }
    foreach ($case in $golden.paths) {
        if (-not $planned.ContainsKey($case.path)) {
            throw ("黄金路径没有进产物计划：$($case.path)（实际计划：$((($planned.Keys | Sort-Object) -join ', '))）")
        }
    }
    $goldenClosure = Invoke-Assetc @("closure", $goldenRelease)
    if ($goldenClosure.ExitCode -ne 0) {
        throw ("黄金表产物闭包检查判红：`n" + $goldenClosure.Text)
    }
    Write-Host "release_closure_golden_ok paths=$($golden.paths.Count) containers=$((Get-ChildItem -LiteralPath (Join-Path $goldenRelease 'bundles') -File).Count)" -ForegroundColor DarkGray
    $cases++

    # ---------- 8. 平台形态：拆成多根 + `.rlb.txt` 改名之后，闭包仍然成立 ----------
    # 前七条都在 assetc 的**原始输出**上跑：单根、未改名。而微信发布链会做两件改变坐标的事——
    # ① 按布局把容器搬进 `pack/`、`pack-sub/`、`pack-<name>/`；② 按上传白名单把 `.rlb` 改名 `.rlb.txt`。
    # 这两件事之后 `assetc closure` 才是"平台发布闭包"；否则 CI 只证明了检查器在自家门口能跑。
    # 这里用**真工具**造形态：`assetc layout` 出布局 → `bundle-layout.js --apply` 真搬真改名 →
    # `check-bundle-mounts.js` 平台门 → `assetc closure` 多根闭包；最后删掉分包容器必须判红。
    $node = Get-Command node -ErrorAction SilentlyContinue
    if (-not $node) { throw "多根形态用例需要 node（bundle-layout.js / check-bundle-mounts.js）" }
    $layoutTool = Join-Path $repo "scripts\build\bundle-layout.js"
    $mountTool = Join-Path $repo "scripts\build\check-bundle-mounts.js"
    foreach ($tool in @($layoutTool, $mountTool)) {
        if (-not (Test-Path -LiteralPath $tool)) { throw "找不到 $tool" }
    }
    $multi = Join-Path $tempRoot "release-multiroot"
    New-Item -ItemType Directory -Path (Join-Path $multi "pack\bundles"), (Join-Path $multi "pack-sub\bundles") -Force | Out-Null
    Copy-Item -Path (Join-Path $releaseDir "bundles\*") -Destination (Join-Path $multi "pack\bundles") -Force
    # 上传白名单改名：`.rlb` → `.rlb.txt`（真实微信代码包的做法；闭包的容器识别按格式定界，必须认得）。
    Get-ChildItem -LiteralPath (Join-Path $multi "pack\bundles") -File | ForEach-Object {
        Rename-Item -LiteralPath $_.FullName -NewName ($_.Name + ".txt")
    }
    Copy-Item -LiteralPath (Join-Path $releaseDir "bundle_plan.json") -Destination (Join-Path $multi "pack\bundle_plan.json") -Force
    # `bundle-layout --apply` 从 pack-sub 取计划表并落到 launcher 根（适配层挂载前就要读它）。
    Copy-Item -LiteralPath (Join-Path $releaseDir "bundle_plan.json") -Destination (Join-Path $multi "pack-sub\bundle_plan.json") -Force
    [System.IO.File]::WriteAllText((Join-Path $multi "pack\game.js"), "// main entry`n", $utf8)
    [System.IO.File]::WriteAllText((Join-Path $multi "pack-sub\game.js"), "// sub entry`n", $utf8)
    [System.IO.File]::WriteAllText((Join-Path $multi "game.json"), "{`n  ""deviceOrientation"": ""landscape""`n}`n", $utf8)
    $layoutJson = Invoke-Assetc @("layout", (Join-Path $multi "pack"), "--json")
    if ($layoutJson.ExitCode -ne 0) {
        throw ("assetc layout 失败（exit $($layoutJson.ExitCode)）：`n" + $layoutJson.Text)
    }
    $layoutFile = Join-Path $tempRoot "layout.json"
    [System.IO.File]::WriteAllText($layoutFile, $layoutJson.Text, (New-Object System.Text.UTF8Encoding($false)))
    $applyOutput = & $node.Source $layoutTool --file $layoutFile --apply $multi 2>&1 | ForEach-Object { "$_" }
    if ($LASTEXITCODE -ne 0) {
        throw ("bundle-layout --apply 失败（exit $LASTEXITCODE）：`n" + ($applyOutput -join "`n"))
    }
    $subContainers = @(Get-ChildItem -LiteralPath (Join-Path $multi "pack-sub\bundles") -File -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -like "*.rlb.txt" })
    if ($subContainers.Count -lt 1) {
        throw "bundle-layout --apply 没有把分包容器搬进 pack-sub/（产物形态不对）"
    }
    $mountOutput = & $node.Source $mountTool --launcher $multi 2>&1 | ForEach-Object { "$_" }
    if ($LASTEXITCODE -ne 0) {
        throw ("check-bundle-mounts 在多根形态上失败（exit $LASTEXITCODE）：`n" + ($mountOutput -join "`n"))
    }
    Write-Host (($mountOutput | Where-Object { $_ -match "bundle_mounts_ok" } | Select-Object -First 1)) -ForegroundColor DarkGray
    $multiClosure = Invoke-Assetc @("closure", $multi)
    if ($multiClosure.ExitCode -ne 0) {
        throw ("多根 + 改名后的闭包检查判红：`n" + $multiClosure.Text)
    }
    # 必须真的走了多根分支（roots=2：pack + pack-sub）；`roots=1` 说明它退回了单根回退，
    # 那样这条用例就只是把上面第 3 条重跑了一遍。
    if ($multiClosure.Text -notmatch "roots=2\b") {
        throw ("多根闭包没有报 roots=2（可能退回了单根回退）：`n" + $multiClosure.Text)
    }
    Write-Host (($multiClosure.Text -split "`n" | Where-Object { $_ -match "assetc_closure_ok" } | Select-Object -First 1)) -ForegroundColor DarkGray
    Write-Host "release_closure_multiroot_ok roots=2 sub_containers=$($subContainers.Count) renamed=.rlb.txt" -ForegroundColor DarkGray
    $cases++

    # 反例：把分包容器删掉，多根闭包必须点名 `pack-sub/` 缺件（而不是"没找到就跳过"）。
    $brokenRoot = Join-Path $tempRoot "release-multiroot-broken"
    Copy-Item -LiteralPath $multi -Destination $brokenRoot -Recurse -Force
    $victimSub = Get-ChildItem -LiteralPath (Join-Path $brokenRoot "pack-sub\bundles") -File |
        Where-Object { $_.Name -like "*.rlb.txt" } | Select-Object -First 1
    Remove-Item -LiteralPath $victimSub.FullName -Force
    $brokenMulti = Invoke-Assetc @("closure", $brokenRoot)
    if ($brokenMulti.ExitCode -eq 0) {
        throw ("删掉分包容器后多根闭包仍然通过（假绿）：`n" + $brokenMulti.Text)
    }
    if ($brokenMulti.Text -notmatch "pack-sub") {
        throw ("多根闭包缺件时没有点明根名（期望包含 pack-sub）：`n" + $brokenMulti.Text)
    }
    Write-Host "release_closure_multiroot_missing_ok 删掉 $($victimSub.Name) 后判红（exit $($brokenMulti.ExitCode)）" -ForegroundColor DarkGray
    $cases++

    # ---------- 9. keep：必须留在目录里的真实文件（微信字体/音频） ----------
    # `wx.loadFont(path)` / `InnerAudioContext.src` 只吃**真实文件路径**，容器条目宿主读不到。
    # 所以这些条目留在目录里，计划表记录具体路径；闭包必须断言"真的还在目录里"。
    # 反过来若它既不在目录、又不在容器里，运行期就是"没声音/字体回退"——静默，不报错。
    $keepRelease = Join-Path $tempRoot "release-keep"
    $keepPacked = Invoke-Assetc @("package", $projectDir, "-o", $keepRelease, "--bundle", "grouped", "--keep", "assets/")
    if ($keepPacked.ExitCode -ne 0) {
        throw ("带 --keep 的 grouped 打包失败（exit $($keepPacked.ExitCode)）：`n" + $keepPacked.Text)
    }
    $keepPlan = Get-Content -LiteralPath (Join-Path $keepRelease "bundle_plan.json") -Raw -Encoding UTF8 | ConvertFrom-Json
    if (-not $keepPlan.keep -or @($keepPlan.keep).Count -lt 1) {
        throw "计划表没有记录 keep 路径：发布期就没有依据去断言「这些文件必须留在目录里」"
    }
    $keptRelative = [string](@($keepPlan.keep)[0])
    $keptCount = @($keepPlan.keep).Count
    $keptFile = Join-Path $keepRelease $keptRelative
    if (-not (Test-Path -LiteralPath $keptFile -PathType Leaf)) {
        throw "keep 路径不在产物目录里：$keptRelative（--keep 没生效）"
    }
    $keptInsideContainer = @($keepPlan.groups | ForEach-Object { $_.paths } | Where-Object { $_ -eq $keptRelative })
    if ($keptInsideContainer.Count -gt 0) {
        throw "keep 路径同时被计划进了容器：$keptRelative（双份占配额）"
    }
    $keepClosure = Invoke-Assetc @("closure", $keepRelease)
    if ($keepClosure.ExitCode -ne 0) {
        throw ("带 keep 的正常产物被闭包检查判红（假红）：`n" + $keepClosure.Text)
    }
    # 摘要现在还带 `keep_remote=`（瘦首包形态下 keep 只在资源包里），所以按"两个字段各自出现"断言，
    # 不写死它们的相邻顺序——否则给摘要加一个字段就会误伤这条用例。
    if ($keepClosure.Text -notmatch ("keep=" + $keptCount + "\b") -or $keepClosure.Text -notmatch "keep_contract=true\b") {
        throw ("闭包摘要没有报 keep 条目数/契约（期望 keep=$keptCount … keep_contract=true）：`n" + $keepClosure.Text)
    }
    $keepVictimOriginal = [System.IO.File]::ReadAllBytes($keptFile)
    try {
        Remove-Item -LiteralPath $keptFile -Force
        $keepBroken = Invoke-Assetc @("closure", $keepRelease)
        if ($keepBroken.ExitCode -eq 0) {
            throw ("keep 文件被删掉后闭包仍然通过（假绿）：`n" + $keepBroken.Text)
        }
        if ($keepBroken.Text -notmatch "keep 条目不在目录里") {
            throw ("keep 缺件的报错没有点明原因（期望包含「keep 条目不在目录里」）：`n" + $keepBroken.Text)
        }
        Write-Host ("release_closure_keep_ok keep={0} 删掉 {1} 后判红（exit {2}）" -f $keptCount, [System.IO.Path]::GetFileName($keptFile), $keepBroken.ExitCode) -ForegroundColor DarkGray
    }
    finally {
        [System.IO.File]::WriteAllBytes($keptFile, $keepVictimOriginal)
    }
    $cases++

    # ---------- 10. cdn（远端分发）：跳过 ≠ 通过 ----------
    # `unit = "cdn"` 的组有两种落点，契约按**字节在哪**判定（2026-10-20 真实 WebGL 产物体检后修正）：
    #   a. 容器就在包内（WebGL/PC 把 cdn 组当普通内容打；微信 remote 也是先落 pack/ 再镜像）
    #      → 必须就地核对，`cdn_local=1`；
    #   b. 容器被搬到了镜像目录（发布根里没有）→ 必须 `--cdn <目录>`，否则判红（要显式 --allow-cdn-skip）。
    # 旧行为是**静默跳过**（只数个数），于是"远端少了一个容器 / 容器内容与计划不一致"在发布日志里
    # 与"通过"长得一模一样。
    $cdnProject = Join-Path $tempRoot "cdn-project"
    Copy-Item -LiteralPath $projectDir -Destination $cdnProject -Recurse -Force
    $cdnBundlesConfig = @"
{
  "schema": 1,
  "default_group": "core",
  "default_unit": "main",
  "allow_unowned": true,
  "groups": [
    { "name": "brand", "unit": "cdn", "include": ["assets/textures/"] }
  ]
}
"@
    [System.IO.File]::WriteAllText((Join-Path $cdnProject "bundles.json"), $cdnBundlesConfig, $utf8)
    $cdnRelease = Join-Path $tempRoot "release-cdn"
    $cdnPacked = Invoke-Assetc @("package", $cdnProject, "-o", $cdnRelease, "--bundle", "grouped")
    if ($cdnPacked.ExitCode -ne 0) {
        throw ("cdn 形态打包失败（exit $($cdnPacked.ExitCode)）：`n" + $cdnPacked.Text)
    }
    $cdnPlan = Get-Content -LiteralPath (Join-Path $cdnRelease "bundle_plan.json") -Raw -Encoding UTF8 | ConvertFrom-Json
    $cdnGroup = @($cdnPlan.groups | Where-Object { $_.unit -eq "cdn" })
    if ($cdnGroup.Count -lt 1) {
        throw "bundles.json 里的 cdn 组没有进计划（夹具本身失效，这条断言等于没跑）"
    }
    # a) 容器还在包内（WebGL/PC 形态）：**不用** `--cdn` 也必须就地核对，而不是判"未检查"。
    $cdnLocal = Invoke-Assetc @("closure", $cdnRelease)
    if ($cdnLocal.ExitCode -ne 0) {
        throw ("cdn 组容器就在包内，闭包却判红（片面上要求 --cdn 会把正确产物判坏）：`n" + $cdnLocal.Text)
    }
    if ($cdnLocal.Text -notmatch "cdn_local=1\b") {
        throw ("闭包摘要没有报 cdn_local=1（说明包内的 cdn 组没被核对）：`n" + $cdnLocal.Text)
    }
    # b) 把容器搬到镜像目录（就是真实 remote 形态的落点）：此时不给 `--cdn` 必须判"未检查"。
    $cdnMirror = Join-Path $tempRoot "cdn-mirror"
    $cdnBundleRelative = [string]$cdnGroup[0].bundle
    $cdnMirrorContainer = Join-Path $cdnMirror $cdnBundleRelative
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $cdnMirrorContainer) | Out-Null
    Move-Item -LiteralPath (Join-Path $cdnRelease $cdnBundleRelative) -Destination $cdnMirrorContainer
    $cdnNoDir = Invoke-Assetc @("closure", $cdnRelease)
    if ($cdnNoDir.ExitCode -eq 0) {
        throw ("计划里有 cdn 组、容器又不在包内、也没给 --cdn，闭包仍然通过（把「没检查」写成了「通过」）：`n" + $cdnNoDir.Text)
    }
    if ($cdnNoDir.Text -notmatch "未检查") {
        throw ("cdn 未检查的报错没有点明原因（期望包含「未检查」）：`n" + $cdnNoDir.Text)
    }
    $cdnChecked = Invoke-Assetc @("closure", $cdnRelease, "--cdn", $cdnMirror)
    if ($cdnChecked.ExitCode -ne 0) {
        throw ("给了 --cdn（镜像齐全）后闭包仍判红：`n" + $cdnChecked.Text)
    }
    if ($cdnChecked.Text -notmatch "cdn_checked=1\b") {
        throw ("闭包摘要没有报 cdn_checked=1（说明远端组没真被核对）：`n" + $cdnChecked.Text)
    }
    # 反例：远端少了容器 → 必须点名 `<cdn>`（而不是"远端没找到就算了"）。
    Remove-Item -LiteralPath (Join-Path $cdnMirror $cdnBundleRelative) -Force
    $cdnMissing = Invoke-Assetc @("closure", $cdnRelease, "--cdn", $cdnMirror)
    if ($cdnMissing.ExitCode -eq 0) {
        throw ("远端容器缺失后闭包仍然通过（假绿）：`n" + $cdnMissing.Text)
    }
    if ($cdnMissing.Text -notmatch "<cdn>") {
        throw ("远端缺件的报错没有点明远端坐标（期望包含 <cdn>）：`n" + $cdnMissing.Text)
    }
    Write-Host "release_closure_cdn_ok cdn_groups=$($cdnGroup.Count) 包内就地核(cdn_local=1) / 搬走未给 --cdn 判红 / 给了逐组核对 / 远端缺件点名 <cdn>" -ForegroundColor DarkGray
    $cases++

    # ---------- 11. 瘦首包（-AssetPack remote）：容器全在资源包里 ----------
    # `package.ps1 -AssetPack remote` 会把 `pack/` 裁成骨架，**所有**组的容器都随资源包导出到
    # `<发布根>/assets/<pack 目录名>/bundles/…`。这时本地一个容器都没有：
    #   a. 不给远端目录 → 必须逐组判红（不是"没找到就跳过"）；
    #   b. 给了资源包目录 → 全部在远端核对通过，摘要里 remote_checked 必须等于参与检查的组数。
    $thinRelease = Join-Path $tempRoot "release-thin"
    Copy-Item -LiteralPath $releaseDir -Destination $thinRelease -Recurse -Force
    $thinExport = Join-Path $thinRelease "assets\pack-sub"
    New-Item -ItemType Directory -Force -Path (Join-Path $thinExport "bundles") | Out-Null
    foreach ($bundleFile in @(Get-ChildItem -LiteralPath (Join-Path $thinRelease "bundles") -File -ErrorAction SilentlyContinue)) {
        Move-Item -LiteralPath $bundleFile.FullName -Destination (Join-Path $thinExport ("bundles\" + $bundleFile.Name))
    }
    Remove-Item -LiteralPath (Join-Path $thinRelease "bundles") -Recurse -Force -ErrorAction SilentlyContinue
    $thinLocal = Invoke-Assetc @("closure", $thinRelease)
    if ($thinLocal.ExitCode -eq 0) {
        throw ("瘦首包（容器全在资源包里）不给 --cdn，闭包仍然通过（把「没检查」写成了「通过」）：`n" + $thinLocal.Text)
    }
    $thinRemote = Invoke-Assetc @("closure", $thinRelease, "--cdn", $thinExport)
    if ($thinRemote.ExitCode -ne 0) {
        throw ("给了资源包目录（容器齐全）后闭包仍判红：`n" + $thinRemote.Text)
    }
    if ($thinRemote.Text -notmatch "remote_checked=[1-9]\d*\b") {
        throw ("闭包摘要没有报 remote_checked=N（说明瘦首包的容器没在远端被核对）：`n" + $thinRemote.Text)
    }
    # 反例：资源包里少一个容器 → 必须点名（远端缺件与本地缺件一样判红）。
    $thinVictim = @(Get-ChildItem -LiteralPath (Join-Path $thinExport "bundles") -File | Select-Object -First 1)
    Remove-Item -LiteralPath $thinVictim.FullName -Force
    $thinMissing = Invoke-Assetc @("closure", $thinRelease, "--cdn", $thinExport)
    if ($thinMissing.ExitCode -eq 0) {
        throw ("资源包里少了容器，闭包仍然通过（假绿）：`n" + $thinMissing.Text)
    }
    Write-Host ("release_closure_thin_ok containers={0} 本地缺件判红 / --cdn 逐组核对(remote_checked) / 远端缺件判红" -f $thinVictim.Name) -ForegroundColor DarkGray
    $cases++

    Write-Host "release_closure_test_ok cases=$cases containers=$($containers.Count) assetc=$assetcExe" -ForegroundColor Green
}
finally {
    if ($KeepTemp) {
        Write-Host "release_closure_temp_kept path=$tempRoot" -ForegroundColor DarkYellow
    }
    else {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
