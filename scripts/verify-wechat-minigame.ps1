# 微信小游戏发布包验证（scripts/verify-wechat-minigame.ps1）
#
# 两层验证：
#   1) 静态布局/配额：入口与配置齐全、pkg/ 与 pack/ 就位、不含 PC/WebGL 的入口文件、
#      主包与总体积满足微信小游戏限制（主包 ≤ 4MB、全部 ≤ 20MB）。
#   2) 运行时行为：交给 scripts/wechat-minigame-harness.js —— 用 Node 伪造 wx API，
#      真实执行 game.js → wechat-adapter.js → web-runtime.js，跑若干帧并断言
#      pack 资源被读取、Lua 主循环在绘制、触摸/键盘事件被桥接。
#
# 用法：powershell -NoProfile -File scripts\verify-wechat-minigame.ps1 `
#         -BundleRoot examples\game_jhyt\builds\wechat-minigame
[CmdletBinding()]
param(
    [string]$BundleRoot = "",
    [int]$Frames = 12,
    # 资源包（CDN 导出）目录：瘦首包（-AssetPack remote）必须给，
    # 否则包内 pack/ 只有骨架，运行时验证读不到重资源。
    [string]$AssetPackRoot = "",
    # 只做静态检查（没有 node 时使用）。
    [switch]$SkipRuntime
)

$ErrorActionPreference = "Stop"
# 子进程（node harness / Lua 日志）输出是 UTF-8，PS 5.1 控制台默认按 ANSI 解码会显示成乱码。
try { [Console]::OutputEncoding = New-Object Text.UTF8Encoding($false) } catch { }
$workspaceRoot = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($BundleRoot)) {
    $BundleRoot = Join-Path $workspaceRoot "examples\game_jhyt\builds\wechat-minigame"
}
if (-not (Test-Path -LiteralPath $BundleRoot)) { throw "发布目录不存在: $BundleRoot" }
$root = (Resolve-Path -LiteralPath $BundleRoot).Path
# 允许直接传**发布根**（形如 builds/<平台>/，内含 launcher/ 与可选的 assets/）：微信发布根下
# 真正的代码包是 launcher/，这里自动下沉一层。否则直接跑本脚本（不传 -BundleRoot）会得到
# "缺少 game.js" 这种无从下手的报错——而默认值指向的正是发布根。
if (-not (Test-Path -LiteralPath (Join-Path $root "game.js"))) {
    $launcherCandidate = Join-Path $root "launcher"
    if (Test-Path -LiteralPath (Join-Path $launcherCandidate "game.js")) {
        $root = (Resolve-Path -LiteralPath $launcherCandidate).Path
        Write-Host ("发布根自动下沉到 launcher/: {0}" -f $root) -ForegroundColor DarkGray
    }
}

# 资源包形态自动判定，三形态互斥（判据只看产物本身，不依赖发布时的命令行）：
#   subpackage = game.json 声明了 root=pack 的分包 **且** 主包侧首屏清单 mainpack.json 存在；
#   thin       = pack/ 下没有资源目录（assets/scenes/scripts）却存在 remote-assets.json；
#   full       = 其余。
# 注意不能只靠"声明了分包"这一条：分包声明是发布脚本注入的，而 mainpack.json 是切分的凭据，
# 两者必须同时成立才是我方产出的分包形态（缺一个就说明产物是手工改过或半更新的）。
# 改名映射（产物根的 code-package.json）：逻辑路径 → 会随代码包上传的宿主文件名。
# 微信代码包只上传后缀在白名单内的文件，构建期把 `.lua`/`.toml`/`.md` 改名成 `*.txt`
# （见 scripts/code-package-rules.js）；验收侧**只查表**，不重算规则，否则两处规则必然漂移。
# 必须解析完 $root 就立刻加载：下面的瘦首包分支（包内骨架与资源包逐字节比对）就要用它。
$codePackageRenamed = @{}
$codePackageMapPath = Join-Path $root "code-package.json"
if (Test-Path -LiteralPath $codePackageMapPath) {
    $mapObject = Get-Content -LiteralPath $codePackageMapPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($mapObject.renamed) {
        foreach ($property in $mapObject.renamed.PSObject.Properties) {
            $codePackageRenamed[$property.Name] = [string]$property.Value
        }
    }
}
function Resolve-CodePackName([string]$logical) {
    if ($codePackageRenamed.ContainsKey($logical)) { return $codePackageRenamed[$logical] }
    return $logical
}

$remoteConfigPath = Join-Path $root "remote-assets.json"
$hasRemoteConfig = Test-Path -LiteralPath $remoteConfigPath
$packAssetDirs = @("assets", "scenes", "scripts") | Where-Object { Test-Path -LiteralPath (Join-Path $root ("pack\" + $_)) }
$isThinPack = ($packAssetDirs.Count -eq 0) -and $hasRemoteConfig
$packDeclaredAsSubpackage = $false
$gameJsonPathEarly = Join-Path $root "game.json"
if (Test-Path -LiteralPath $gameJsonPathEarly) {
    try {
        $gameJsonEarly = Get-Content -LiteralPath $gameJsonPathEarly -Raw -Encoding UTF8 | ConvertFrom-Json
        # 只查一种拼写：PowerShell 的属性访问**不区分大小写**，`subPackages`（小程序拼法）
        # 与 `subpackages`（小游戏拼法）会解析到同一个属性。两种都写一遍会在 `+` 时重复计数
        # （实测：1 个分包被数成 2 个，然后"有且仅有 1 个"这条断言自己失败）。
        $declaredList = @($gameJsonEarly.subpackages)
        foreach ($item in $declaredList) {
            if ($item -and ([string]$item.root).TrimEnd('/') -eq "pack") { $packDeclaredAsSubpackage = $true; break }
        }
    }
    catch { $packDeclaredAsSubpackage = $false }
}
$mainPackManifestPath = Join-Path $root "mainpack.json"
$hasMainPackManifest = Test-Path -LiteralPath $mainPackManifestPath
$isSubpackagePack = $packDeclaredAsSubpackage -and $hasMainPackManifest
# 三形态互斥的兜底断言：分包形态的资源全在包里，出现远端配置就说明产物是两种形态混出来的——
# 那时适配层会走 CDN 回落，"包内读到了"永远掩盖"分包根本没挂载上"。
if ($isSubpackagePack -and $hasRemoteConfig) {
    throw "分包形态不得带 remote-assets.json（它属于 -AssetPack remote）：两种资源来源混在一个包里会让失败互相掩盖"
}
if ($isSubpackagePack -and $isThinPack) {
    throw "产物同时具有分包与瘦首包的特征（pack/ 无资源目录 + remote-assets.json + 分包声明 + mainpack.json），无法判定形态"
}
if ($packDeclaredAsSubpackage -and -not $hasMainPackManifest) {
    throw "game.json 声明了 pack 分包，但主包侧首屏清单 mainpack.json 不存在：首屏资源没有切到主包，真机上会先等分包下载"
}
$assetPackDir = ""
if ($isThinPack) {
    if ([string]::IsNullOrWhiteSpace($AssetPackRoot)) {
        $candidates = @(
            (Join-Path (Split-Path -Parent $root) "assets"),
            (Join-Path (Split-Path -Parent $root) ((Split-Path -Leaf $root) + "-assets"))
        )
        foreach ($candidate in $candidates) {
            if (Test-Path -LiteralPath (Join-Path $candidate "pack")) { $AssetPackRoot = $candidate; break }
        }
    }
    if ([string]::IsNullOrWhiteSpace($AssetPackRoot) -or -not (Test-Path -LiteralPath (Join-Path $AssetPackRoot "pack"))) {
        throw ("这是瘦首包（pack/ 只有骨架），必须用 -AssetPackRoot <资源包目录> 指出重资源在哪" +
            "（约定位置：与 launcher/ 同级的 assets/，形如 <发布根>/assets/pack/**）")
    }
    $assetPackDir = (Resolve-Path -LiteralPath $AssetPackRoot).Path
    # 两侧必须一一对应：资源包和包内 manifest 不一致，远端校验会把文件全丢掉。
    $bundlePackFiles = (Get-ChildItem -LiteralPath (Join-Path $root "pack") -Recurse -File -Force | Measure-Object).Count
    $assetPackFiles = (Get-ChildItem -LiteralPath (Join-Path $assetPackDir "pack") -Recurse -File -Force | Measure-Object).Count
    $manifest = Join-Path $root "pack\manifest.txt"
    $manifestEntries = (Get-Content -LiteralPath $manifest -Encoding UTF8 | Measure-Object).Count - 1
    if ($assetPackFiles -lt $manifestEntries) {
        throw ("资源包不完整: 只有 {0} 个文件，manifest 声明了 {1} 条" -f $assetPackFiles, $manifestEntries)
    }
    # 文件数够还不够：**包内骨架必须与资源包里的那一份逐字节相同**。运行时会拿包内 manifest.txt
    # 里的体积/FNV 校验和去核对下载回来的文件，骨架和 CDN 一旦不是同一份（比如只重新导出了资源
    # 却没重新发布），远端文件会在真机上被整批丢弃——本机不查就只能等真机白屏才发现。
    foreach ($shared in @("manifest.txt", "project.toml")) {
        # 包内那份已按白名单改名（`project.toml` → `project.toml.txt`），资源包（CDN）那份保持
        # **逻辑名**不变——CDN 不受白名单约束，远端 URL / 缓存路径 / manifest 校验和全按逻辑路径算。
        $inBundle = Join-Path $root ("pack\" + (Resolve-CodePackName $shared))
        $inPack = Join-Path $assetPackDir ("pack\" + $shared)
        if (-not (Test-Path -LiteralPath $inPack)) {
            throw ("资源包缺少骨架文件: pack\{0}（远端校验依赖它）" -f $shared)
        }
        $a = [IO.File]::ReadAllBytes($inBundle)
        $b = [IO.File]::ReadAllBytes($inPack)
        $same = ($a.Length -eq $b.Length)
        if ($same) {
            for ($i = 0; $i -lt $a.Length; $i++) { if ($a[$i] -ne $b[$i]) { $same = $false; break } }
        }
        if (-not $same) {
            $detail = if ($a.Length -eq $b.Length) { "{0} B（字节数相同但内容不同）" -f $a.Length } else { "{0} B vs {1} B" -f $a.Length, $b.Length }
            throw (("包内 pack\{0} 与资源包里的不是同一份：{1}。远端校验会用错误的清单，" -f $shared, $detail) +
                "真机上会把下载文件整批丢弃。请用同一次 -AssetPack remote 发布同时产出首包与资源包。")
        }
    }
    Write-Host ("资源包形态: 瘦首包（包内 pack/ {0} 个文件，资源包 {1} 个文件 / manifest {2} 条）" -f `
        $bundlePackFiles, $assetPackFiles, $manifestEntries) -ForegroundColor DarkGray
    Write-Host ("资源包目录: {0}" -f $assetPackDir) -ForegroundColor DarkGray
}


# 主包（除 pack/ 分包之外的一切）与分包的体积上限（微信小游戏：主包 4MB、全部 20MB）。
$MainPackageLimit = 4MB
$TotalLimit = 20MB

function Get-DirSize {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return 0 }
    $sum = (Get-ChildItem -LiteralPath $Path -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
    if ($null -eq $sum) { return 0 }
    return $sum
}

# 骨架文件（manifest.txt / project.toml）的位置随形态变：分包形态下它们在首屏清单里，
# 构建期已被搬到 mainpack/，pack/ 里**不该**再有同名副本。
$skeletonSubdir = if ($isSubpackagePack) { "mainpack" } else { "pack" }
$required = @(
    "game.js",
    "game.json",
    "project.config.json",
    "wechat-adapter.js",
    "web-runtime.js",
    "pkg\game_web.js",
    "pkg\game_web_bg.wasm",
    ($skeletonSubdir + "\" + (Resolve-CodePackName "manifest.txt")),
    ($skeletonSubdir + "\" + (Resolve-CodePackName "project.toml")),
    "code-package.json",
    "release.toml"
)
if ($isSubpackagePack) {
    # 分包形态多两项：路由依据（清单）与分包入口（微信的硬要求）。
    $required += @("mainpack.json", "pack\game.js")
}
foreach ($file in $required) {
    $full = Join-Path $root $file
    if (-not (Test-Path -LiteralPath $full)) { throw "微信小游戏发布包缺少: $file" }
    if ((Get-Item -LiteralPath $full).Length -le 0) { throw "微信小游戏发布包文件为空: $file" }
}

# 小游戏运行时按 CommonJS 解析，包里任何 ES Module（顶层 export/import）都会让启动直接失败：
#   WAGame.js:1 SyntaxError: MiniProgramError Unexpected token 'export'
# 实测踩过：wasm-bindgen --target web 的浏览器胶水就是这样被打进包里的。这里逐个脚本按
# **脚本模式**解析（不能用 node --check：Node 会自动识别 ESM，对顶层 export 反而"通过"）。
$scriptFiles = Get-ChildItem -LiteralPath $root -Recurse -File -Filter *.js -Force
$nodeForSyntax = Get-Command node -ErrorAction SilentlyContinue
if ($nodeForSyntax) {
    # 探针只写 stdout：PS 5.1 在 $ErrorActionPreference='Stop' 下会把原生程序 stderr 当终止性错误，
    # 用 2>&1 捕获会让脚本在检查退出码前中断（"守卫触发"与"守卫自己崩了"就分不清了）。
    $syntaxProbe = "const fs=require('fs'),vm=require('vm'),p=process.argv[1];try{new vm.Script(fs.readFileSync(p,'utf8'),{filename:p});process.exit(0)}catch(e){process.stdout.write(String(e.message).split('\n')[0]);process.exit(1)}"
    foreach ($scriptFile in $scriptFiles) {
        $rel = $scriptFile.FullName.Substring($root.Length).TrimStart('\')
        $probe = & $nodeForSyntax.Source -e $syntaxProbe $scriptFile.FullName
        if ($LASTEXITCODE -ne 0) {
            throw "微信小游戏发布包含有非 CommonJS 脚本: $rel -> $($probe -join ' ')"
        }
    }
    Write-Host ("脚本均为 CommonJS 可解析: {0} 个 .js" -f $scriptFiles.Count) -ForegroundColor DarkGray
}
else {
    foreach ($scriptFile in $scriptFiles) {
        $match = Select-String -LiteralPath $scriptFile.FullName -Pattern '^\s*(export\s|import\s+[^\(])' | Select-Object -First 1
        if ($match) { throw "微信小游戏发布包含有非 CommonJS 脚本: $($scriptFile.Name) (line $($match.LineNumber))" }
    }
}

# 代码包后缀白名单（真机只上传后缀在白名单内的文件，见 scripts/code-package-rules.js）：
# 资源树里不得有"会被白名单拒掉"的文件——真机上它们根本不存在，运行时读到的是一串 permission denied。
# 判据不在这里重算：交给同一份规则模块（node 侧），这里只看结论。
$resourceDirs = @()
foreach ($name in @("mainpack", "pack")) {
    $dir = Join-Path $root $name
    if (Test-Path -LiteralPath $dir) { $resourceDirs += $dir }
}
if ($resourceDirs.Count -gt 0) {
    if (-not $nodeForSyntax) {
        Write-Warning "==> 未找到 node，跳过代码包后缀白名单检查（真机可能因此读不到文件）"
    }
    else {
        $whitelistTool = Join-Path $PSScriptRoot "first-screen-pack.js"
        $savedEap = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $whitelistOutput = & $nodeForSyntax.Source $whitelistTool --check-whitelist @resourceDirs 2>&1
            $whitelistExit = $LASTEXITCODE
        }
        finally { $ErrorActionPreference = $savedEap }
        if ($whitelistExit -ne 0) {
            throw ("代码包里仍有非白名单后缀的文件（真机不会上传它们）: " + (($whitelistOutput | ForEach-Object { "$_" }) -join ' '))
        }
        Write-Host (($whitelistOutput | Where-Object { $_ -match '^code_package_whitelist_ok' } | Select-Object -First 1)) -ForegroundColor DarkGray
    }
}

# 小游戏包不允许混入 PC 运行器或 WebGL 网页入口——那说明发布脚本落回了错误分支。
foreach ($forbidden in @("game.exe", "index.html", "assetc.exe")) {
    if (Test-Path -LiteralPath (Join-Path $root $forbidden)) {
        throw "微信小游戏发布包不应包含 $forbidden（发布分支选错平台？）"
    }
}

# 子集字体：小游戏不随包分发 17MB 完整字体。瘦首包下字体属于"重资源"，
# 会随资源包放到 CDN（适配层启动时会先下载到本地再 wx.loadFont），所以两种形态都要能找到。
# 字体在包内的位置同样随形态变：分包形态下字体是首屏资源，已被切到 mainpack/assets/fonts。
$fontSearchRoots = @((Join-Path $root ($skeletonSubdir + "\assets\fonts")))
if ($isThinPack -and $assetPackDir) { $fontSearchRoots += (Join-Path $assetPackDir "pack\assets\fonts") }
$packedFonts = @()
foreach ($fontRoot in $fontSearchRoots) {
    $packedFonts += Get-ChildItem -LiteralPath $fontRoot -File -Filter *.ttf -ErrorAction SilentlyContinue
}
if (-not $packedFonts) { throw "找不到子集字体（pack/assets/fonts，assetc 未产出？瘦首包则该在资源包里）" }
if ($isThinPack) {
    Write-Host ("字体随资源包分发: {0}（{1:N0} KB，首启下载后注册）" -f $packedFonts[0].Name, ($packedFonts[0].Length / 1KB)) -ForegroundColor DarkGray
}

$gameJson = Get-Content -LiteralPath (Join-Path $root "game.json") -Raw -Encoding UTF8 | ConvertFrom-Json
# 分包声明按形态**三分支**断言。这里两个方向都必须显式断言：只留"没有声明就没事"的话，
# 分包形态悄悄退化成 full 形态（首屏又开始等分包）不会有任何提示。
# 同样只查一种拼写（PS 属性访问不区分大小写，两种拼法会解析到同一个属性）。
$subpackageList = @($gameJson.subpackages)
$subpackageNames = @($subpackageList | Where-Object { $_ } | ForEach-Object { $_.name })
$subpackageRoots = @($subpackageList | Where-Object { $_ } | ForEach-Object { ([string]$_.root).TrimEnd('/') })
if ($isSubpackagePack) {
    if ($subpackageList.Count -ne 1) {
        throw ("分包形态期望**有且仅有 1 个**分包，实际 {0} 个：{1}" -f $subpackageList.Count, ($subpackageNames -join ','))
    }
    if ($subpackageRoots[0] -ne "pack") { throw "分包 root 规范化后是 '$($subpackageRoots[0])'（期望 pack）" }
    if ($subpackageNames[0] -ne "pack") { throw "分包 name 是 '$($subpackageNames[0])'（期望 pack）" }
    $subpackageEntry = Join-Path $root "pack\game.js"
    if (-not (Test-Path -LiteralPath $subpackageEntry)) {
        throw "分包形态缺少分包入口 pack\game.js（微信要求 subpackages[0].root 目录下有 game.js，缺了开发者工具拒绝打开工程）"
    }
    if ((Get-Item -LiteralPath $subpackageEntry).Length -le 0) { throw "分包入口 pack\game.js 是空文件" }
}
elseif ($subpackageNames -contains "pack") {
    throw "game.json 把 pack 声明为分包，但这一形态（full/thin）不该有该声明：非分包形态声明分包会引入分包入口要求"
}

$projectConfig = Get-Content -LiteralPath (Join-Path $root "project.config.json") -Raw -Encoding UTF8 | ConvertFrom-Json
if ($projectConfig.compileType -ne "game") { throw "project.config.json 的 compileType 必须是 game，实际: $($projectConfig.compileType)" }
if ([string]::IsNullOrWhiteSpace($projectConfig.projectname)) { throw "project.config.json 缺少 projectname" }

# 体积**三档**断言，三形态的算法不同：
#   subpackage：主包 = 总量 − pack/（分包），三档都要查（主包 4MB / 单分包 4MB / 总计 20MB）；
#   thin      ：pack/ 只剩骨架，主包 = 总量，重资源在 CDN 上不占包体配额；
#   full      ：pack/ 也在主包里，主包 = 总量。
# 不用字符串前缀比较路径：临时目录的 8.3 短名（ADMINI~1）与长名不一致时前缀匹配会失效。
$totalSize = Get-DirSize -Path $root
$packSize = Get-DirSize -Path (Join-Path $root "pack")
$subpackageSize = if ($isSubpackagePack) { $packSize } else { 0 }
$mainSize = $totalSize - $subpackageSize
# 主包超限的出路按形态给：分包形态的杠杆是"把资源列进首屏清单"（清单变大 ⇒ 主包变大、
# 分包变小，两侧此消彼长），full 形态只能换形态，thin 形态基本不会超。
$mainHint = if ($isSubpackagePack) {
    "把更多资源列进首屏清单 mainpack.json（清单变大 ⇒ 主包变大、分包变小，两侧此消彼长），或改用 -AssetPack remote"
}
elseif ($isThinPack) { "pack/ 骨架已经在 512KB 白名单内，若仍超限说明骨架白名单该更新了" }
else { "改用 -AssetPack subpackage（主包 + 单分包）或 -AssetPack remote 把重资源移出包内" }
if ($mainSize -gt $MainPackageLimit) {
    throw ("主包体积超限: {0:N0} KB > {1:N0} KB。出路：{2}" -f ($mainSize / 1KB), ($MainPackageLimit / 1KB), $mainHint)
}
# 单个普通分包：官方 2023-10 起不限制大小（主包 ≤ 4M、单个普通分包不限、总量 ≤ 20M/30M，
# 见 docs/52 与 scripts/package.ps1 的体积门）。因此不再用主包 4MB 门卡分包，
# 由下方总量门（20MB）兜底。
if ($isSubpackagePack -and $subpackageSize -gt $TotalLimit) {
    throw ("单个分包体积超限: {0:N0} KB > {1:N0} KB（pack/ 是 subpackages[0].root，目录下所有文件都计入）。出路：把更多资源列进首屏清单，或拆第二个分包" -f `
        ($subpackageSize / 1KB), ($TotalLimit / 1KB))
}
# 总量门（官方：主包 + 全部分包之和 ≤ 20MB；开通虚拟支付可到 30MB，按无内购口径保守取 20MB）。
# 主包 4MB 门在上面独立生效；单个普通分包不限制大小（见上一条注释），因此总量门是分包的兜底。
if ($totalSize -gt $TotalLimit) {
    throw ("发布包总体积超限: {0:N0} KB > {1:N0} KB（主包 + 全部分包之和）" -f ($totalSize / 1KB), ($TotalLimit / 1KB))
}

$formName = if ($isSubpackagePack) { "subpackage" } elseif ($isThinPack) { "thin" } else { "full" }
Write-Host ("wechat_minigame_static_ok main={0:N0}KB subpackage={1:N0}KB total={2:N0}KB subpackages={3} asset_pack={4} upload_rules=ok main_of_limit={5:P1} subpackage_of_limit={6:P1} total_of_limit={7:P1}" -f `
    ($mainSize / 1KB), ($subpackageSize / 1KB), ($totalSize / 1KB), $(if ($subpackageNames.Count) { $subpackageNames -join ',' } else { 'none' }), $formName, `
    ($mainSize / $MainPackageLimit), $(if ($subpackageSize -gt 0) { $subpackageSize / $TotalLimit } else { 0 }), ($totalSize / $TotalLimit)) -ForegroundColor Green
if ($isSubpackagePack) {
    Write-Host ("主包 {0:N0}KB（含首屏 mainpack/ {1:N0}KB）/ 上限 4,096KB（{2:P1}）；分包 pack/ {3:N0}KB / 总量上限 20,480KB（{4:P1}，单分包官方不限大小）" -f `
        ($mainSize / 1KB), ((Get-DirSize -Path (Join-Path $root "mainpack")) / 1KB), ($mainSize / $MainPackageLimit), `
        ($subpackageSize / 1KB), ($subpackageSize / $TotalLimit)) -ForegroundColor DarkGray
}
elseif ($isThinPack) {
    Write-Host ("包内骨架 {0:N0}KB / 上限 4,096KB（{1:P1}）；资源包 {2:N0}KB 在 CDN，不占配额" -f `
        ($packSize / 1KB), ($packSize / $MainPackageLimit), ((Get-DirSize -Path (Join-Path $assetPackDir "pack")) / 1KB)) -ForegroundColor DarkGray
}
else {
    Write-Host ("pack/ 计入主包: {0:N0}KB / 上限 4,096KB（{1:P1}）" -f ($packSize / 1KB), ($packSize / $MainPackageLimit)) -ForegroundColor DarkGray
}

if (-not $SkipRuntime) {
    $node = Get-Command node -ErrorAction SilentlyContinue
    if (-not $node) { throw "未找到 node，无法执行运行时验证（或使用 -SkipRuntime 只做静态检查）" }
    $harness = Join-Path $PSScriptRoot "wechat-minigame-harness.js"
    if (-not (Test-Path -LiteralPath $harness)) { throw "缺少运行时验证脚本: $harness" }

    # 每个用例都在独立进程里跑；这里统一封装 Start-Process 的细节。
    # 不能直接用 `& node ... 2>&1`：PS 5.1 会把原生程序写到 stderr 的正常诊断当成错误记录，
    # 在 $ErrorActionPreference='Stop' 下直接把脚本打断。所以重定向到文件再读回。
    function Invoke-HarnessCase {
        param([string[]]$ExtraArgs, [string]$Label)
        Write-Host ("==> Runtime verification: node $harness $root --frames $Frames {0}" -f ($ExtraArgs -join ' ')) -ForegroundColor Cyan
        $outFile = Join-Path $env:TEMP ("rustlua-wx-verify-" + [guid]::NewGuid().ToString("N") + ".out")
        $errFile = $outFile + ".err"
        try {
            # `Start-Process -ArgumentList` 会把数组按空格拼接且**不加引号**：含空格的路径
            # （自测用的 "rustlua release test …" 隔离目录）必须自己带上引号——不只是 $root，
            # 附加参数里的路径（--remote-root <资源包目录>）同样会从空格处被截断。
            $quoted = foreach ($arg in $ExtraArgs) {
                if ($arg -match '"') { throw "harness 参数里不允许出现双引号: $arg" }
                if ($arg -match '\s') { '"' + $arg + '"' } else { $arg }
            }
            $arguments = @(('"' + $harness + '"'), ('"' + $root + '"'), "--frames", $Frames) + @($quoted)
            $process = Start-Process -FilePath $node.Source -ArgumentList $arguments `
                -PassThru -Wait -NoNewWindow `
                -RedirectStandardOutput $outFile -RedirectStandardError $errFile
            $lines = @()
            if (Test-Path -LiteralPath $outFile) { $lines += Get-Content -LiteralPath $outFile -Encoding UTF8 }
            if (Test-Path -LiteralPath $errFile) { $lines += Get-Content -LiteralPath $errFile -Encoding UTF8 }
            $lines | ForEach-Object { Write-Host $_ }
            return @{ ExitCode = $process.ExitCode; Lines = $lines; Label = $Label }
        }
        finally {
            Remove-Item -LiteralPath $outFile -Force -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath $errFile -Force -ErrorAction SilentlyContinue
        }
    }

    # 1) 宿主对**代码包/分包路径形式**的接受规则并不统一（实测开发者工具里 pack/project.toml
    #    读不到，上层只报 "HTTP 404"，根因就是路径形式/挂载差异）。每种方言都要跑通：
    #    适配层必须能自己探明可用形式，而不是碰巧对上某一种。
    $pathStyles = @("root", "slash", "sub-root", "no-utf8")
    foreach ($style in $pathStyles) {
        # 瘦首包：方言用例同时跑真实远端（否则包内只剩骨架，重资源读不到必然失败）。
        # harness 会临时改写 remote-assets.json 指向它自己起的本地服务，用完恢复发布用配置。
        $caseArgs = @("--wx-path-style", $style)
        if ($isThinPack) { $caseArgs += @("--remote", "thin", "--remote-root", $assetPackDir) }
        # 分包形态：方言用例必须**同时**把挂载这一轴显式打开（--subpackage ok），否则
        # "哪一档方言配哪一种分包行为"就说不清了，用例之间会互相掩盖。
        if ($isSubpackagePack) { $caseArgs += @("--subpackage", "ok") }
        $case = Invoke-HarnessCase -ExtraArgs $caseArgs -Label $style
        if ($case.ExitCode -ne 0) { throw "微信小游戏运行时验证失败（--wx-path-style $style, exit $($case.ExitCode)）" }
        if (-not ($case.Lines | Where-Object { $_ -match "wechat_minigame_harness_ok" })) {
            throw "运行时验证未输出 wechat_minigame_harness_ok（--wx-path-style $style）"
        }
    }
    $probeLine = $case.Lines | Where-Object { $_ -match "资源读取自检" } | Select-Object -First 1
    if ($probeLine) { Write-Host $probeLine -ForegroundColor DarkGray }

    # 1a2) wasm 宿主不可用（--no-wasm：模拟开发者工具/极旧基础库没有可用的 WXWebAssembly）：
    #      WP10 之后 wasm **就是** Lua 执行层，缺它必须响亮失败。fengari 时代那条"wasm 失败
    #      不影响 Lua 启动（clock 走 Lua 兜底、boot 照常成功）"的降级契约已经不存在——
    #      没有解释器就没有兜底。这里要的证据是"失败可归因"：boot=failed + 拒绝原因指名 wasm 桥
    #      + 控制台有启动失败这一行 + 诊断如实报 no-webassembly（而不是含糊的"没跑起来"）。
    $noWasmArgs = @("--frames", "60", "--no-wasm")
    $noWasmCase = Invoke-HarnessCase -ExtraArgs $noWasmArgs -Label "no-wasm-loud-failure"
    if ($noWasmCase.ExitCode -ne 0) { throw "wasm 缺失时未按契约响亮失败（--no-wasm, exit $($noWasmCase.ExitCode)）" }
    $noWasmOk = $noWasmCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_ok" } | Select-Object -First 1
    if (-not $noWasmOk) { throw "no-wasm-loud-failure 用例未输出 wechat_minigame_harness_ok" }
    if (-not ($noWasmOk -match 'wasm=no-webassembly')) {
        throw ("no-wasm-loud-failure 用例的 wasm 诊断未如实报告 no-webassembly：{0}" -f $noWasmOk)
    }
    Write-Host $noWasmOk -ForegroundColor DarkGray

    # 1a3) Tag polyfill 探针（决定性守卫）：wasm-bridge.js 的 PolyfillTag 能不能顶替宿主缺失的
    #      WebAssembly.Tag，把带 tag 导入的 wasm 实例化起来？这条决定"要不要剥 wasm 二进制"。
    #      harness 用一份只导入单个 tag 的极小模块走宿主那条路径式 instantiate，同时给一条
    #      **真 tag 正对照**（真 tag 必须过，否则说明探针接错、读数会撒谎——正是历史上"本机全绿、
    #      工具里起不来"的来源：默认用例转发的是 V8 真 Tag，从没真正测过 polyfill 路径）。
    #      本机读到的 `polyfill_tag_probe=rejected[...tag import requires a WebAssembly.Tag]` 证明
    #      "只靠 JS polyfill 顶不住 tag 导入"，因此 strip-wasm-tags 的剥二进制必须留在链路里。
    #      真机/开发者工具同一次读数即为设备裁决：若哪天变成 accepted，才能考虑退役剥二进制方案。
    $tagProbeArgs = @("--frames", "30", "--wasm-tag-polyfill-probe")
    if ($isThinPack) { $tagProbeArgs += @("--remote", "thin", "--remote-root", $assetPackDir) }
    $tagProbeCase = Invoke-HarnessCase -ExtraArgs $tagProbeArgs -Label "wasm-tag-polyfill-probe"
    if ($tagProbeCase.ExitCode -ne 0) { throw "tag polyfill 探针用例失败（--wasm-tag-polyfill-probe, exit $($tagProbeCase.ExitCode)）" }
    $tagProbeLine = $tagProbeCase.Lines | Where-Object { $_ -match "polyfill_tag_probe=" } | Select-Object -First 1
    if (-not $tagProbeLine) {
        throw "tag polyfill 探针未输出 polyfill_tag_probe= 读数（探针没被真正驱动，这条守卫形同虚设）"
    }
    # 正对照必须是 accepted：否则探针的 instantiate 通道本身是坏的，rejected 读数的方向性就不可信。
    if (-not ($tagProbeLine -match "real_tag_probe=accepted")) {
        throw ("tag polyfill 探针的正对照没通过（real_tag_probe 不是 accepted）——探针接错，读数不可信：{0}" -f $tagProbeLine)
    }
    Write-Host $tagProbeLine -ForegroundColor DarkGray
    # 汇总成紧凑 token 附到最终 package_ok 行：polyfill=rejected 是"必须剥二进制"的机检凭据。
    $tagPolyfillVerdict = if ($tagProbeLine -match "polyfill_tag_probe=(\w+)") { $matches[1] } else { "unknown" }

    # EH 真机探针（同一次 --wasm-tag-polyfill-probe 驱动）：产物剥掉两个 externref tag 导入后，
    # 仍留着**本地 tag 段** $tag$0 + legacy try/catch（实测在产物里 throw/catch $tag$0 各 1 条、
    # try=92 / catch_all=501 / rethrow=92）——那是 Rust catch_unwind / Lua pcall 的载体，不能剥。
    # 于是真机上的决定性问题从"引擎认不认 tag 导入"换成"引擎到底实没实现 wasm 异常处理本身"。
    # wasm-bridge 的 probeEHAcceptance 经 WXWebAssembly.compile 编译随包的极小模块 pkg/eh-probe.wasm
    # （零导入 + 一个本地 tag + legacy try/catch，把变量收到只剩 EH），读数经适配层落进诊断的
    # wasm.ehProbe、由 harness 汇成 ok 行里的 `eh_probe=`：
    #   accepted ⇒ 引擎实现 EH，剥完的产物真机能落地、pcall 有地方走；
    #   reject[...] ⇒ 引擎连本地 tag 段都拒，剥二进制救不了微信包，只能换"根本不用 EH"的产物形态。
    # 本机 V8 支持 EH，所以这里的 accepted 是**正对照**（证明探针接对、不是空转），设备那一侧只能真机读。
    # 另加一条 exec 凭据：直接实例化随包探针调 f()，只有真跑过 `throw $tag`→`catch $tag` 才返回 7，
    # 挡住"探针其实是份不含 EH 的桩、compile 当然过"这种假绿（与 real_tag_probe=accepted 同一手法）。
    $ehProbeExecLine = $tagProbeCase.Lines | Where-Object { $_ -match "eh_probe_exec=" } | Select-Object -First 1
    if (-not $ehProbeExecLine) {
        throw "EH 探针未输出 eh_probe_exec= 读数（随包探针没被驱动，这条守卫形同虚设）"
    }
    if (-not ($ehProbeExecLine -match "eh_probe_exec=7")) {
        throw ("EH 探针的 exec 正对照没通过（f() 不返回 7：随包探针没真的承载 legacy EH，读数不可信）：{0}" -f $ehProbeExecLine)
    }
    Write-Host $ehProbeExecLine -ForegroundColor DarkGray
    $ehProbeOkLine = $tagProbeCase.Lines | Where-Object { $_ -match "eh_probe=" -and $_ -notmatch "eh_probe_exec=" } | Select-Object -First 1
    if (-not $ehProbeOkLine) {
        throw "EH 探针未输出 eh_probe= 读数（探针编译通道没走到，这条守卫形同虚设）"
    }
    if (-not ($ehProbeOkLine -match "eh_probe=accepted")) {
        throw ("EH 真机探针的正对照没通过（本机 compile 不是 accepted，探针通道接错、读数不可信）：{0}" -f $ehProbeOkLine)
    }
    $ehProbeVerdict = if ($ehProbeOkLine -match "eh_probe=(\w[\w-]*)") { $matches[1] } else { "unknown" }

    # 1b) 分包形态专项（新形态的核心承诺，四条各自钉住一件事）：
    #     · 形态自证：声明有且仅有 1 个 root=pack 的分包、入口 pack/game.js 可执行、
    #       首屏读取全部走主包 mainpack/（= 首屏不需要分包就绪）；
    #     · 首屏不等分包：延迟到第 6 帧才挂载，启动必须在挂载之前就完成（标题照常出画面），
    #       且进度条真的消费了 onProgressUpdate 并画到画布上；
    #     · 玩法路径触发挂载：进 manor（非首屏）必须读 pack/，那时才需要分包；
    #     · 分包永远挂不上：首屏照常启动，失败**三重可见**（属性 + 控制台 + 屏幕红条），
    #       且没有未处理的 Promise 拒绝。
    if ($isSubpackagePack) {
        # 帧预算必须显式抬起来：`--drive-move N` 会从总预算里扣 N+1 帧，而延迟挂载又要等到第 6 帧，
        # 用默认的 $Frames（12）会把帧预算吃光 → 一帧都不跑 → 分包永远挂不上（实测踩过，报"无回调超时"）。
        $subFrames = @("--frames", "120")
        $subpackageCases = @(
            @{ Label = "subpackage-ok"; Args = @("--subpackage", "ok"); Token = "wechat_minigame_harness_ok"; Evidence = 'subpackage=pack' },
            @{ Label = "subpackage-delay"; Args = ($subFrames + @("--subpackage", "delay", "--subpackage-delay-frames", "6", "--drive-new-game", "--expect-runtime-spawns")); Token = "wechat_minigame_harness_ok"; Evidence = 'loading_bar=ok' },
            @{ Label = "subpackage-delay-move"; Args = ($subFrames + @("--subpackage", "delay", "--subpackage-delay-frames", "6", "--drive-new-game", "--expect-runtime-spawns", "--drive-move", "20")); Token = "wechat_minigame_harness_ok"; Evidence = 'mount_frame=6' },
            @{ Label = "subpackage-never"; Args = @("--subpackage", "never", "--expect-pack-failure"); Token = "wechat_minigame_harness_ok"; Evidence = 'pack_state=failed' },
            @{ Label = "subpackage-fail-once"; Args = ($subFrames + @("--subpackage", "fail-once", "--drive-new-game", "--expect-runtime-spawns")); Token = "wechat_minigame_harness_ok"; Evidence = 'subpackage_loads=2' },
            # 适配层**不得**依赖 `game.json`：它在真机上读不到（平台配置不向 FileSystemManager 开放），
            # 而"要不要挂分包"曾经拿它当唯一判据 → 真机上分包静默不挂（实测）。这条断言把该依赖钉死。
            @{ Label = "no-game-json-dependency"; Args = @("--frames", "40", "--expect-no-read", "game.json"); Token = "wechat_minigame_harness_ok"; Evidence = 'pack_state=ready' }
        )
        foreach ($spec in $subpackageCases) {
            $case = Invoke-HarnessCase -ExtraArgs $spec.Args -Label $spec.Label
            if ($case.ExitCode -ne 0) { throw ("分包形态用例失败（{0}, exit {1}）" -f $spec.Label, $case.ExitCode) }
            $okLine = $case.Lines | Where-Object { $_ -match $spec.Token } | Select-Object -First 1
            if (-not $okLine) { throw ("分包形态用例未输出 {0}（{1}）" -f $spec.Token, $spec.Label) }
            if (-not ($okLine -match [regex]::Escape($spec.Evidence))) {
                throw ("分包形态用例缺少证据 '{0}'（{1}）：{2}" -f $spec.Evidence, $spec.Label, $okLine)
            }
            # 首屏清单必须真的生效：mainpack_reads=0 意味着首屏仍在从分包读（退化成"等下载"）。
            if (-not ($okLine -match 'mainpack_reads=[1-9]')) {
                throw ("分包形态用例没有走主包 mainpack/ 读取（首屏清单没生效）：{0}" -f $okLine)
            }
            # 启动期间不得有 pack/ 读取被未挂载的分包挡下（那正是"清单漏列"的直接症状）。
            if (-not ($okLine -match 'denied_before_boot=0')) {
                throw ("分包形态用例出现「启动期间读不到 pack/」——首屏清单漏列了首屏资源：{0}" -f $okLine)
            }
            Write-Host $okLine -ForegroundColor DarkGray
        }
    }

    # 2) 远端回落：包内读不到 pack/ 时，素材必须真的从服务下载、按清单校验、落地缓存并复用。
    #    harness 会自己起一个本地静态服务（复用 scripts/pack-server.js），因此这里不依赖外部进程。
    #    瘦首包下这个"完整资源"只能来自导出目录，所以要把 --remote-root 指过去。
    #
    #    **分包形态不跑这一组**：那一形态按设计就没有 remote-assets.json（发布脚本互斥拒绝），
    #    硬塞 `--remote auto` 只会得到"包内读不到 + 没有远端配置"的假失败。remote 形态的
    #    回归由 thin/full 形态的产物各自负责（同一次质量门里会分别验证）。
    $remoteOkLine = $null
    $negativeCases = @()
    if ($isSubpackagePack) {
        Write-Host "==> 分包形态：跳过远端回落下沉用例（该形态按设计不带 remote-assets.json）" -ForegroundColor DarkGray
    }
    else {
        $autoArgs = @("--remote", "auto")
        if ($isThinPack) { $autoArgs += @("--remote-root", $assetPackDir) }
        $remoteCase = Invoke-HarnessCase -ExtraArgs $autoArgs -Label "remote-auto"
        if ($remoteCase.ExitCode -ne 0) { throw "远端回落验证失败（--remote auto, exit $($remoteCase.ExitCode)）" }
        $remoteOkLine = $remoteCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_ok" } | Select-Object -First 1
        if (-not $remoteOkLine) { throw "远端回落验证未输出 wechat_minigame_harness_ok" }

        # 3) 反向用例：被篡改的内容必须被校验和拦下（服务端只翻转 1 字节、长度不变），
        #    服务不可达必须快速失败。二者都必须"按预期失败"，否则说明守卫是摆设。
        $negativeCases = @("corrupt", "down")
        foreach ($mode in $negativeCases) {
            $negativeArgs = @("--remote", $mode)
            # 瘦首包：服务根要指向导出目录，否则"被篡改的 project.toml"根本不在服务范围内。
            if ($isThinPack -and $mode -ne "down") { $negativeArgs += @("--remote-root", $assetPackDir) }
            $case = Invoke-HarnessCase -ExtraArgs $negativeArgs -Label $mode
            if ($case.ExitCode -ne 0) { throw "远端回落反向用例未按预期失败（--remote $mode, exit $($case.ExitCode)）" }
            if (-not ($case.Lines | Where-Object { $_ -match "wechat_minigame_harness_negative_ok" })) {
                throw "反向用例未输出 wechat_minigame_harness_negative_ok（--remote $mode）"
            }
        }
    }

    # 4) 瘦首包专项：包内骨架必须**包内命中**（证明"包内优先"不是口号），重资源必须真的
    #    从资源包下载、按清单校验、落地缓存——并且本地 pack/ 里确实不该有那些重资源。
    if ($isThinPack) {
        $case = Invoke-HarnessCase -ExtraArgs @("--remote", "thin", "--remote-root", $assetPackDir) -Label "thin"
        if ($case.ExitCode -ne 0) { throw "瘦首包运行时验证失败（--remote thin, exit $($case.ExitCode)）" }
        $thinOkLine = $case.Lines | Where-Object { $_ -match "wechat_minigame_harness_ok" } | Select-Object -First 1
        if (-not $thinOkLine) { throw "瘦首包验证未输出 wechat_minigame_harness_ok" }
        Write-Host $thinOkLine -ForegroundColor DarkGray
    }

    # 5) 宿主不允许改写 Image.src 时（旧基础库）：**需要改写 src 的形态必须响亮失败**，
    #    不需要改写的形态必须照常启动。这条契约把"贴图静默消失"钉死成可测行为。
    #    三种形态是否需要改写：
    #      thin       → 需要（重资源在 CDN，src 得指向下载后的本地缓存）；
    #      subpackage → 需要（首屏贴图在 mainpack/，运行时要的是 pack/ 前缀，不改写就是整批丢图）；
    #      full       → 不需要（资源都在 pack/ 原地，宿主自己就能读）。
    $needsSrcRewrite = $isThinPack -or $isSubpackagePack
    if ($isSubpackagePack) {
        $frozenLoudArgs = @("--image-src-frozen")
    }
    else {
        $frozenLoudArgs = @("--image-src-frozen", "--remote", $(if ($isThinPack) { "thin" } else { "auto" }))
        if ($isThinPack) { $frozenLoudArgs += @("--remote-root", $assetPackDir) }
    }
    $frozenLoud = Invoke-HarnessCase -ExtraArgs $frozenLoudArgs -Label "image-src-frozen-loud"
    if ($frozenLoud.ExitCode -ne 0) { throw "Image.src 冻结 + 需要改写 src 的形态未按预期响亮失败（exit $($frozenLoud.ExitCode)）" }
    if (-not ($frozenLoud.Lines | Where-Object { $_ -match "case=image-src-frozen" })) {
        throw "Image.src 冻结用例未输出 wechat_minigame_harness_negative_ok case=image-src-frozen"
    }
    if (-not $needsSrcRewrite) {
        # 包内完整时不能因为宿主不让改写 src 就起不来（资源本来就在包里）。
        $frozenTolerant = Invoke-HarnessCase -ExtraArgs @("--image-src-frozen", "--remote", "off") -Label "image-src-frozen-tolerated"
        if ($frozenTolerant.ExitCode -ne 0) { throw "Image.src 冻结 + 包内完整时不应失败（exit $($frozenTolerant.ExitCode)）" }
    }

    # 5b) 宿主把 `Image.src` 定义在**原型**上——这是微信基础库的真实形态
    #     （`class Image { setSrc(v){…} getSrc(){…} }`，`Object.getOwnPropertyDescriptor(实例,'src')`
    #     返回 undefined）。只看 own 描述符的实现会静默退化成写 `image._src`，而基础库是在
    #     `setSrc` 里发起加载的：宿主永远不开始加载 → 运行时的贴图 Promise 永不 settle →
    #     启动卡死 + 屏幕全黑 + 0 error + 日志全绿（开发者工具实测就是这个症状）。
    #     断言"接管方式 = prototype-accessor 且贴图真的加载了"，而不是"看起来没报错"。
    $protoSrcArgs = @("--image-src-prototype")
    if ($isThinPack) { $protoSrcArgs += @("--remote", "thin", "--remote-root", $assetPackDir) }
    $protoSrcCase = Invoke-HarnessCase -ExtraArgs $protoSrcArgs -Label "image-src-prototype"
    if ($protoSrcCase.ExitCode -ne 0) { throw "宿主把 Image.src 定义在原型上时启动失败（--image-src-prototype, exit $($protoSrcCase.ExitCode)）" }
    if (-not ($protoSrcCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_ok" })) {
        throw "原型 Image.src 用例未输出 wechat_minigame_harness_ok（贴图接管没有被验证）"
    }

    # 5d) 宿主把 `onload`/`onerror` 也做成**原型访问器**，回调存进私有字段（基础库 2.32.3 的
    #     `k$` 包装类：`set onload(v){ this.__onload = v }`，真实 image 的回调在**构造时**接成
    #     `this.__onload`）。用自己的 accessor 盖住 `onload`＝把宿主的桥拆掉：贴图加载成功也传不回
    #     运行时，症状与"src 没交到宿主"一模一样（卡死 + 0 错误 + 全黑）。断言接管方式 = host-setter。
    $bridgeArgs = @("--image-callback-bridge")
    if ($isThinPack) { $bridgeArgs += @("--remote", "thin", "--remote-root", $assetPackDir) }
    $bridgeCase = Invoke-HarnessCase -ExtraArgs $bridgeArgs -Label "image-callback-bridge"
    if ($bridgeCase.ExitCode -ne 0) { throw "宿主的 onload/onerror 是原型访问器时启动失败（--image-callback-bridge, exit $($bridgeCase.ExitCode)）" }
    if (-not ($bridgeCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_ok" })) {
        throw "回调桥接用例未输出 wechat_minigame_harness_ok（回调接管没有被验证）"
    }

    # 5c) 宿主**收下** src 却永远不回调（弱网/CDN 挂起、基础库不触发 onload/onerror）：
    #     这是"没有异常、没有错误、就是不动"的一类卡死。启动看门狗必须报警，而且必须
    #     **把"启动超时"画到画布上**——真机/开发者工具里没有控制台，只 console.error 等于
    #     还是黑屏。超时走存储注入压到 1s（默认 10s），否则一条用例要等 10 秒进不了质量门。
    $stallArgs = @("--image-src-silent", "--expect-boot-stall", "--boot-timeout-ms", "1000", "--boot-deadline-ms", "6000")
    if ($isThinPack) { $stallArgs += @("--remote", "thin", "--remote-root", $assetPackDir) }
    $stallCase = Invoke-HarnessCase -ExtraArgs $stallArgs -Label "boot-stall-watchdog"
    if ($stallCase.ExitCode -ne 0) { throw "启动卡死时看门狗没有兜住（--image-src-silent --expect-boot-stall, exit $($stallCase.ExitCode)）" }
    if (-not ($stallCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_negative_ok case=boot-stall" })) {
        throw "启动卡死用例未输出 wechat_minigame_harness_negative_ok case=boot-stall（看门狗自证没有被验证）"
    }
    if (-not ($stallCase.Lines | Where-Object { $_ -match "on_screen=yes" })) {
        throw "启动卡死用例里看门狗只报了警、没画到屏幕上（玩家看到的仍是黑屏）"
    }

    # 6) 宿主自带残缺/不可写的全局：微信开发者工具的壳里**已经有一个不可写的 document**
    #    （实测报错 "TypeError: document.getElementById is not a function"），有的还带一个
    #    独立 window。适配层必须"接替或就地补齐"宿主对象后照常启动；补不齐（冻结对象）
    #    必须响亮失败——绝不能让 TypeError 留到运行时才炸。
    $hostArgs = @("--host-document", "stub", "--host-window", "separate")
    if ($isThinPack) { $hostArgs += @("--remote", "thin", "--remote-root", $assetPackDir) }
    $hostCase = Invoke-HarnessCase -ExtraArgs $hostArgs -Label "host-globals"
    if ($hostCase.ExitCode -ne 0) { throw "宿主自带残缺全局时启动失败（--host-document stub --host-window separate, exit $($hostCase.ExitCode)）" }
    if (-not ($hostCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_host_globals_ok" })) {
        throw "宿主自带残缺全局的用例未输出 wechat_minigame_harness_host_globals_ok（补齐路径没有被验证）"
    }
    #    更贴近开发者工具真实形态的一档：全局不可写 + 宿主对象**不可扩展**，只有已存在的
    #    自有属性还能重定义。实测它正是 "document.getElementById is not a function" 与
    #    "__RustLuaWechat is not defined"（加载期中止）的来源；这里断言"照常跑起来 + 补不动的
    #    成员被明确报出"，也就是不许再用加载期抛错来"响亮失败"。
    $lockedHostArgs = @("--host-document", "locked")
    if ($isThinPack) { $lockedHostArgs += @("--remote", "thin", "--remote-root", $assetPackDir) }
    $lockedHost = Invoke-HarnessCase -ExtraArgs $lockedHostArgs -Label "host-globals-locked"
    if ($lockedHost.ExitCode -ne 0) { throw "宿主全局不可扩展时游戏没能跑起来（--host-document locked, exit $($lockedHost.ExitCode)）" }
    if (-not ($lockedHost.Lines | Where-Object { $_ -match "wechat_minigame_harness_host_globals_ok document=locked" })) {
        throw "不可扩展宿主用例未输出 wechat_minigame_harness_host_globals_ok document=locked"
    }
    $frozenHostArgs = @("--host-document", "frozen")
    if ($isThinPack) { $frozenHostArgs += @("--remote", "thin", "--remote-root", $assetPackDir) }
    $frozenHost = Invoke-HarnessCase -ExtraArgs $frozenHostArgs -Label "host-globals-frozen"
    if ($frozenHost.ExitCode -ne 0) { throw "宿主全局被冻结时未按预期响亮失败（exit $($frozenHost.ExitCode)）" }
    if (-not ($frozenHost.Lines | Where-Object { $_ -match "case=host-document-frozen" })) {
        throw "宿主全局被冻结的用例未输出 wechat_minigame_harness_negative_ok case=host-document-frozen"
    }

    # 7) 渲染自证：小游戏宿主没有 DOM 控制台，"流程正常但屏幕全黑" 必须能自证。
    #    --host-canvas offscreen：上屏画布挂在全局、wx.createCanvas() 返回的是另一个（离屏）
    #      对象（引导代码先调过一次就会这样）→ 适配层必须挑上屏画布，绘制只能落在它上面；
    #    --debug-overlay：覆盖层 + 像素回读（画 #ff00ff 再 getImageData 读回来）必须命中；
    #    --remote down --expect-image-band：重资源拿不到时屏幕必须出现红色警示条。
    $canvasArgs = @("--host-canvas", "offscreen")
    $overlayArgs = @("--debug-overlay")
    if ($isThinPack) {
        $canvasArgs += @("--remote", "thin", "--remote-root", $assetPackDir)
        $overlayArgs += @("--remote", "thin", "--remote-root", $assetPackDir)
    }
    $canvasCase = Invoke-HarnessCase -ExtraArgs $canvasArgs -Label "canvas-provenance"
    if ($canvasCase.ExitCode -ne 0) { throw "上屏画布选择用例失败（--host-canvas offscreen, exit $($canvasCase.ExitCode)）" }
    if (-not ($canvasCase.Lines | Where-Object { $_ -match "offscreen_ops=0" })) {
        throw "上屏画布用例没有证明绘制都落在上屏画布上（未输出 offscreen_ops=0）"
    }
    $overlayCase = Invoke-HarnessCase -ExtraArgs $overlayArgs -Label "render-overlay"
    if ($overlayCase.ExitCode -ne 0) { throw "渲染自证覆盖层用例失败（--debug-overlay, exit $($overlayCase.ExitCode)）" }
    if (-not ($overlayCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_overlay_ok" })) {
        throw "覆盖层用例没有输出 wechat_minigame_harness_overlay_ok（像素回读没有被验证）"
    }
    #    --no-frames --debug-overlay：**一帧都不跑**时屏幕上仍必须有自证图案。
    #    真机上最常见的黑屏就是"流程走完、一帧都没画"；如果自证图案只挂在帧循环上，
    #    这种黑屏就无法与"画到了看不见的画布"区分开。
    $noFrameArgs = @("--no-frames", "--debug-overlay")
    if ($isThinPack) { $noFrameArgs += @("--remote", "thin", "--remote-root", $assetPackDir) }
    $noFrameCase = Invoke-HarnessCase -ExtraArgs $noFrameArgs -Label "render-overlay-no-frames"
    if ($noFrameCase.ExitCode -ne 0) { throw "零帧自证用例失败（--no-frames --debug-overlay, exit $($noFrameCase.ExitCode)）" }
    if (-not ($noFrameCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_overlay_ok frames=0" })) {
        throw "零帧自证用例没有输出 overlay_ok frames=0（脱离帧循环的自证图案没有被验证）"
    }
    if ($isThinPack) {
        # 瘦首包 + 远端不可达：骨架读得到、重资源拿不到 → 贴图必然失败，屏幕必须有警示条。
        $bandCase = Invoke-HarnessCase -ExtraArgs @("--remote", "png-fail", "--remote-root", $assetPackDir, "--wx-path-style", "root", "--expect-image-band") -Label "image-failure-band"
        if ($bandCase.ExitCode -ne 0) { throw "贴图失败警示条用例失败（--remote down --expect-image-band, exit $($bandCase.ExitCode)）" }
        if (-not ($bandCase.Lines | Where-Object { $_ -match "case=image-failure-band band=yes" })) {
            throw "贴图失败时屏幕没有出现警示条（静默黑屏）"
        }
    }

    # 8) 玩家的真实玩法路径：标题按确认 → 换场景 → 世界里有人 → **按住方向键主角会走**。
    #    前面所有用例（含默认用例）都停在启动场景，所以"点开始新游戏"这条路径曾经是覆盖盲区：
    #    实测症状是标题界面 600 帧全绿、一按确认就崩（scene.load 是排队的场景切换，运行时
    #    若不换场景数据，update() 会立刻按"已在玩法地图"分支执行）。
    #    这里的断言分三层，每层都对着一种"能进图但不能玩"的半死状态：
    #      · 世界存在：地图换了、存活实体不少于场景声明数、主角落在落点（physics.set_position 真的搬人）；
    #      · 运行期生成：alive > scene（entity.spawn + sprite.set + animation.attach 这条链路，
    #        敌人/NPC 这类不在场景里写死的角色全靠它）；
    #      · 世界在动：按住**运行时自己从触摸摇杆派发出来的键**若干帧，主角坐标必须变化，
    #        且 Lua 世界的位置与渲染器侧的位置必须一致（回写通道 T 行没生效时画面上的人会钉死）。
    #    按哪个键由运行时产出，用例不硬编码游戏按键——断的是宿主契约，不是这一款游戏的键位。
    $playArgs = @("--drive-new-game", "--expect-runtime-spawns", "--drive-move", "40", "--frames", "240")
    if ($isThinPack) { $playArgs += @("--remote", "thin", "--remote-root", $assetPackDir) }
    $playCase = Invoke-HarnessCase -ExtraArgs $playArgs -Label "play-path-move"
    if ($playCase.ExitCode -ne 0) { throw "玩法路径用例失败（--drive-new-game --expect-runtime-spawns --drive-move, exit $($playCase.ExitCode)）" }
    $playOkLine = $playCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_ok" } | Select-Object -First 1
    if (-not $playOkLine) { throw "玩法路径用例未输出 wechat_minigame_harness_ok" }
    if (-not ($playOkLine -match 'move="keys=[^"]* d=-?\d+,-?\d+"')) {
        throw "玩法路径用例没有自报位移证据（缺少 move=「keys=… d=x,y」这段）：$playOkLine"
    }
    if ($playOkLine -match 'd=0,0') { throw "玩法路径用例自报位移为 0（主角没动却仍然通过）：$playOkLine" }
    Write-Host $playOkLine -ForegroundColor DarkGray

    # 摇杆链路（docs/34 质量门第 6 条）：`--drive-move` 验证的是**键盘**注入；触屏是微信小游戏
    # 的主输入，单独成用例——触摸 → 指针事件 → virtual_pad 命中 → 键注入 → 动作层 → 世界移动，
    # 位移 d≠0 才通过。此前只能手动驱动 harness 验证，回归会静默漏过，故这里接进门禁。
    $stickArgs = @("--drive-new-game", "--expect-runtime-spawns", "--touch-stick-move", "40", "--frames", "240")
    if ($isThinPack) { $stickArgs += @("--remote", "thin", "--remote-root", $assetPackDir) }
    $stickCase = Invoke-HarnessCase -ExtraArgs $stickArgs -Label "play-path-touchstick"
    if ($stickCase.ExitCode -ne 0) { throw "摇杆链路用例失败（--drive-new-game --expect-runtime-spawns --touch-stick-move, exit $($stickCase.ExitCode)）" }
    $stickOkLine = $stickCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_ok" } | Select-Object -First 1
    if (-not $stickOkLine) { throw "摇杆链路用例未输出 wechat_minigame_harness_ok" }
    if (-not ($stickOkLine -match 'touch_stick="stick before=\d+,\d+ after=\d+,\d+ d=-?\d+,-?\d+"')) {
        throw "摇杆链路用例没有自报位移证据（缺少 touch_stick=「stick before=… after=… d=x,y」这段）：$stickOkLine"
    }
    if ($stickOkLine -match 'd=0,0') { throw "摇杆链路用例自报位移为 0（主角没动却仍然通过）：$stickOkLine" }
    Write-Host $stickOkLine -ForegroundColor DarkGray

    # 远端配置的存在性要自洽：配了必须有、没配必须没有，且内容可解析。
    if ($hasRemoteConfig) {
        $remoteConfig = Get-Content -LiteralPath $remoteConfigPath -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($remoteConfig.packBaseUrl -notmatch '^https?://') { throw "remote-assets.json 的 packBaseUrl 不是 http(s) 地址" }
        if ($remoteConfig.packBaseUrl -notmatch '/$') { throw "remote-assets.json 的 packBaseUrl 必须以 / 结尾" }
        Write-Host ("远端回落配置: {0}（包内仍是首选；本地服务用 scripts/serve-pack.ps1 起）" -f $remoteConfig.packBaseUrl) -ForegroundColor DarkGray
    }
    if ($isThinPack -and -not $hasRemoteConfig) {
        throw "瘦首包必须带 remote-assets.json，否则被裁掉的资源没有任何来源"
    }

    # 输出行按形态给各自的证据字段：分包形态的关键证据是"首屏走主包 + 挂载门 + 进度条"，
    # 远端形态是"下载/校验/缓存"。两种形态用同一行会让人分不清哪些真的验过了。
    $formEvidence = if ($isSubpackagePack) {
        " subpackage_case=ok,delay,delay-move,never,fail-once,no-game-json"
    }
    elseif ($isThinPack) { " thin_case=ok" }
    else { "" }
    $remoteField = if ($isSubpackagePack) { "remote_fallback=n/a(分包形态不带远端配置)" } else { "remote_fallback=ok" }
    Write-Host ("wechat_minigame_package_ok files={0} runtime=ok path_styles={1} asset_pack={2} {3} negatives={4} tag_polyfill_probe={6} eh_probe={7} host_globals=ok,locked render_selfcheck=ok play_path=ok,touch_stick=ok{5}" -f `
        $required.Count, ($pathStyles -join ','), $formName, $remoteField, (($negativeCases + @("image-src-frozen", "image-src-prototype", "image-callback-bridge", "boot-stall", "host-document-frozen")) -join ','), $formEvidence, $tagPolyfillVerdict, $ehProbeVerdict) -ForegroundColor Green
    if ($remoteOkLine) { Write-Host $remoteOkLine -ForegroundColor DarkGray }
} else {
    Write-Host ("wechat_minigame_package_ok files={0} runtime=skipped" -f $required.Count) -ForegroundColor Yellow
}
