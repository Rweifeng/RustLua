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
    # 玩法驱动（微信侧没有 URL 旋钮，用 harness 的宿主驱动）：点开始新游戏 → 按住方向键 →
    # 断言主角坐标真的变了。拆旧 JS 世界（WP8）之前必须先有这条机器判据。
    [switch]$DriveJhyt,
    # 要求这份产物必须带 GPU 内核（`package.ps1 -Gpu2d` 打的那份）：引擎画布探针必须走到
    # `failed` + 引擎原文 `probe: …`，报 `unavailable` 直接判失败（避免"我以为是 GPU 档"）。
    [switch]$ExpectGpuKernel,
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
# 第四种形态（第 53 轮，`package.ps1 -WasmInSubpackage`）：**remote + 内核入分包**。
# 它故意让两种资源来源共存：内核与骨架在分包里，重资源在 CDN。判据全部取产物自带的标记，
# 不猜：①分包已声明且有清单；②清单 entries **为空**（remote 的首屏资源在 CDN，没有包内路由）；
# ③有 remote-assets.json；④内核确实在 pack/ 里（在 pkg/ 就还是"内核在主包"的旧 remote 形态）。
# ①②单独出现 = 分包形态，②③④单独出现 = 旧 remote 形态，都保持原来的互斥断言。
$mainPackEntries = @()
if ($hasMainPackManifest) {
    try { $mainPackEntries = @((Get-Content -LiteralPath $mainPackManifestPath -Raw -Encoding UTF8 | ConvertFrom-Json).entries) }
    catch { $mainPackEntries = @() }
}
$isWasmInPackRemote = $isSubpackagePack -and $hasRemoteConfig -and
    ($mainPackEntries.Count -eq 0) -and (Test-Path -LiteralPath (Join-Path $root "pack\game_web_bg.wasm"))
# 三形态互斥的兜底断言：分包形态的资源全在包里，出现远端配置就说明产物是两种形态混出来的——
# 那时适配层会走 CDN 回落，"包内读到了"永远掩盖"分包根本没挂载上"。
if ($isSubpackagePack -and $hasRemoteConfig -and -not $isWasmInPackRemote) {
    throw "分包形态不得带 remote-assets.json（它属于 -AssetPack remote）：两种资源来源混在一个包里会让失败互相掩盖"
}
if ($isSubpackagePack -and $isThinPack -and -not $isWasmInPackRemote) {
    throw "产物同时具有分包与瘦首包的特征（pack/ 无资源目录 + remote-assets.json + 分包声明 + mainpack.json），无法判定形态"
}
if ($isWasmInPackRemote) {
    # 这一形态**同时**要打开两条轴：内核在分包（要等挂载）+ 重资源在远端（要带 --remote thin）。
    $isThinPack = $true
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

# 骨架文件（manifest.txt / project.toml）的位置随形态变：**经典**分包形态（首屏切到 mainpack/）
# 下它们在 mainpack/ 里，构建期已被搬过去，pack/ 里**不该**再有同名副本；
# 而"remote + 内核入分包"（第 53 轮）没有切分首屏，骨架仍留在 pack/（也就是分包里）。
$skeletonSubdir = if ($isSubpackagePack -and -not $isWasmInPackRemote) { "mainpack" } else { "pack" }
# wasm 内核的位置同样随形态变：主包 4 MB 门装不下内核，所以内核入分包时它在 pack/ 里
# （经典分包形态与 remote + -WasmInSubpackage 都是），full / remote（内核在主包）仍在 pkg/。
# **两边都只许有一处**：pkg/ 里那份残留不会抛错，只会让主包又超回去（而 4 MB 超限的表现是
# 上传被拒，本机什么都看不到），所以这里除了"该有的在"还要验"不该的不在"。
$wasmCoreRelative = if ($isSubpackagePack) { "pack\game_web_bg.wasm" } else { "pkg\game_web_bg.wasm" }
$required = @(
    "game.js",
    "game.json",
    "project.config.json",
    "wechat-adapter.js",
    "web-runtime.js",
    "pkg\game_web.js",
    $wasmCoreRelative,
    ($skeletonSubdir + "\" + (Resolve-CodePackName "manifest.txt")),
    ($skeletonSubdir + "\" + (Resolve-CodePackName "project.toml")),
    "code-package.json"
)
if ($isSubpackagePack) {
    # 分包形态多两项：路由依据（清单）与分包入口（微信的硬要求）。
    $required += @("mainpack.json", "pack\game.js")
    if (Test-Path -LiteralPath (Join-Path $root "pkg\game_web_bg.wasm")) {
        throw "分包形态下 pkg\game_web_bg.wasm 仍在主包里：内核没有被搬进 pack/，主包体积会按旧公式超限（两份同名内核迟早漂移，这里拒绝而不是取其一）"
    }
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

# 上面那条只证明"Node 能解析"，而**上传器的解析器比开发者工具低一档**：实测（2026-10-03，
# 稳定版 2.02.2608070）同一份包两次点上传被拒，都是 `SyntaxError: Unexpected token ?`——
# `pkg/game_web.js, 989:72` 的 ES2021 逻辑赋值 `??=`，和 `web-runtime.js, 1:105804` 的 ES2020
# 空值合并 `??`。工具里跑得通、开发者也看得见画面，只有点"上传"才炸。这里按上传器实测接受的
# 那一档（ES2019，口径与两次现场的对照见 scripts/check-upload-ecma.mjs 头部）逐个 .js 复述一遍
# 解析，把它变成发布期的错误。
if ($nodeForSyntax) {
    $ecmaTool = Join-Path $PSScriptRoot "check-upload-ecma.mjs"
    if (-not (Test-Path -LiteralPath $ecmaTool)) { throw "缺少上传语法门脚本: $ecmaTool" }
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        # 只收 stdout：PS 5.1 在 EAP=Stop 下会把原生程序的 stderr 当终止性错误，`2>&1` 会让
        # 门在读取结果之前自己先炸（与上面语法探针同一口径）。
        $ecmaOutput = & $nodeForSyntax.Source $ecmaTool 2019 @($scriptFiles.FullName)
        $ecmaExit = $LASTEXITCODE
    }
    finally { $ErrorActionPreference = $savedEap }
    if ($ecmaExit -ne 0) {
        $detail = (($ecmaOutput | ForEach-Object { "$_" }) -join ' ')
        if (-not $detail) { $detail = "工具退出码 $ecmaExit 且 stdout 无输出（多半是 acorn 没解析到：跑 scripts/minify-toolchain.ps1 或设 `$env:ACORN，细节在它的 stderr 里）" }
        throw ("发布包里有微信上传器解析不了的语法（口径: ES2019；要改这一档得动 scripts/check-upload-ecma.mjs 的参数并留实测证据）: " + $detail)
    }
    Write-Host (($ecmaOutput | Where-Object { $_ -match '^upload_ecma_ok' } | Select-Object -First 1)) -ForegroundColor DarkGray
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
# 主包超限的出路按形态给，**方向必须写对**：主包 = launcher − pack/，而 mainpack/（首屏资源）
# 是**算进主包**的，所以"把更多资源列进首屏清单"是加大主包的那一手——曾经就这么写反过，
# 超限的人照着做只会更超（`scripts/package.ps1` 的 `Assert-WeChatPackageVolume` 已改正，
# 这里曾经是它唯一的反口径副本，两处现在同一个说法）。
$mainHint = if ($isSubpackagePack) {
    "**收窄**首屏清单（--include 前缀去掉非首屏目录，被排除的资源留在 pack/ 分包里：主包变小、分包变大；列进清单是反方向），或改用 -AssetPack remote"
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
        # WP7-88：把 L3 行为契约接进七步门。此前它只在"手工跑 harness 时带上开关"才会执行
        # （离线可跑、无需浏览器，33 条用例逐行对桌面基线），门里却一直没有它 —— 于是"两边名字都在、
        # 但返回值形状/状态归属分叉"这类缺陷，门抓不到。
        $caseArgs += @("--check-api-contract")
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

    # 1a3) 引擎画布探针（WP8 第二片：把 wasm 的 `probe_canvas_host` 接到真实调用点）。
    #      两条各钉一件事：
    #      · 生产口径：不带开关时必须 `engine_probe=off`——探针默认关，老路径一行不动；
    #      · 开关通路：结论必须**按产物档位**落在两种具名结果之一（"没编进去"与"编进去但宿主机
    #        不给"必须能分开读，否则现场拿到空读数不知道该往哪查）：
    #          默认档内核 ⇒ `unavailable` + `kernel-without-gpu2d`；
    #          GPU 档内核 ⇒ `failed` + 以 `probe: ` 开头的**引擎原文**（离线宿主不给 GL，
    #                        这条正证明画布真的送到了 wgpu）。
    #      也就是说：**同一条断言**在两种产物上都成立，`-Gpu2d` 产物（package.ps1 -Gpu2d）跑本脚本
    #      就自动覆盖 GPU 档；要强制要求"这份产物必须带 GPU 内核"就加 `-ExpectGpuKernel`。
    $probeOffLine = $case.Lines | Where-Object { $_ -match "engine_probe=" } | Select-Object -First 1
    if (-not ($probeOffLine -match 'engine_probe=off')) {
        throw ("生产口径下引擎画布探针必须是 off（默认关），实际：{0}" -f $probeOffLine)
    }
    $gpuProbeCase = Invoke-HarnessCase -ExtraArgs @("--frames", "30", "--gpu-probe") -Label "gpu-probe-switch"
    if ($gpuProbeCase.ExitCode -ne 0) { throw ("--gpu-probe 用例失败（exit {0}）" -f $gpuProbeCase.ExitCode) }
    $gpuProbeOk = $gpuProbeCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_ok" } | Select-Object -First 1
    if (-not $gpuProbeOk) { throw "gpu-probe-switch 用例未输出 wechat_minigame_harness_ok" }
    $probeStatus = if ($gpuProbeOk -match '(?:^|\s)engine_probe=(\S+)') { $matches[1] } else { "缺失" }
    $probeReason = if ($gpuProbeOk -match 'engine_probe_reason="([^"]*)"') { $matches[1] } else { "" }
    switch ($probeStatus) {
        "unavailable" {
            if ($probeReason -notmatch '^kernel-without-gpu2d$') {
                throw ("unavailable 必须带具名理由 kernel-without-gpu2d，实际：{0}" -f $probeReason)
            }
            if ($ExpectGpuKernel) {
                throw "本产物没编 GPU 内核（unavailable），但调用方要求了 -ExpectGpuKernel：请用 package.ps1 -Gpu2d 重新打包"
            }
        }
        "failed" {
            if ($probeReason -notmatch '^probe: ') {
                throw ("GPU 档内核的失败必须带引擎原文（以 'probe: ' 开头），实际：{0}" -f $probeReason)
            }
        }
        "ok" { throw "离线的假宿主不该给出 ok（探针读数可疑）：{0}" -f $gpuProbeOk }
        default { throw ("开探针后必须落在 unavailable/failed 之一，实际 status={0}" -f $probeStatus) }
    }
    Write-Host ("引擎画布探针：status={0} reason={1}" -f $probeStatus, $probeReason) -ForegroundColor DarkGray

    # 1a4) 引擎宿主（WP8 主线第二片：真的起一个宿主并按帧驱动它）。
    #      同样两条，同样**按产物档位**落在两种具名结果之一：
    #        · 生产口径：不带开关时必须 `engine_host=off`（每帧多一次 GPU 提交，默认关）；
    #        · 开关通路：默认档内核 ⇒ `unavailable` + `kernel-without-gpu2d`；
    #                    GPU 档内核 ⇒ `failed` + 以 `host: ` 开头的**引擎原文**（离线假宿主不给 GL，
    #                    这条正证明宿主画布真的送到了 wgpu）。
    #      `running` 只能由真机/带 GL 的宿主给出 ⇒ 在这里出现就是读数可疑，直接判失败。
    $hostOffLine = $case.Lines | Where-Object { $_ -match "engine_host=" } | Select-Object -First 1
    if (-not ($hostOffLine -match '(?:^|\s)engine_host=off')) {
        throw ("生产口径下引擎宿主必须是 off（默认关），实际：{0}" -f $hostOffLine)
    }
    $hostCase = Invoke-HarnessCase -ExtraArgs @("--frames", "30", "--engine-host") -Label "engine-host-switch"
    if ($hostCase.ExitCode -ne 0) { throw ("--engine-host 用例失败（exit {0}）" -f $hostCase.ExitCode) }
    $hostOk = $hostCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_ok" } | Select-Object -First 1
    if (-not $hostOk) { throw "engine-host-switch 用例未输出 wechat_minigame_harness_ok" }
    $hostStatus = if ($hostOk -match '(?:^|\s)engine_host=(\S+)') { $matches[1] } else { "缺失" }
    $hostSource = if ($hostOk -match '(?:^|\s)engine_host_source=(\S+)') { $matches[1] } else { "缺失" }
    $hostReason = if ($hostOk -match 'engine_host_reason="([^"]*)"') { $matches[1] } else { "" }
    switch ($hostStatus) {
        "unavailable" {
            if ($hostReason -notmatch '^kernel-without-gpu2d$') {
                throw ("unavailable 必须带具名理由 kernel-without-gpu2d，实际：{0}" -f $hostReason)
            }
            if ($ExpectGpuKernel) {
                throw "本产物没编 GPU 内核（unavailable），但调用方要求了 -ExpectGpuKernel：请用 package.ps1 -Gpu2d 重新打包"
            }
        }
        "failed" {
            if ($hostReason -notmatch '^host: ') {
                throw ("GPU 档内核的失败必须带引擎原文（以 'host: ' 开头），实际：{0}" -f $hostReason)
            }
        }
        "running" { throw "离线的假宿主不该给出 running（宿主读数可疑）：{0}" -f $hostOk }
        default { throw ("开启引擎宿主后必须落在 unavailable/failed 之一，实际 status={0}" -f $hostStatus) }
    }
    Write-Host ("引擎宿主：status={0} source={1} reason={2}" -f $hostStatus, $hostSource, $hostReason) -ForegroundColor DarkGray

    # 1a5) 宿主接管固定步（WP7-55）：这条路**必须**显式开关才走，因为老 JS 世界每帧也在跑固定步
    #      （两边同时跑 = 世界被推进两次）。这里钉两件事：
    #        · 生产口径：宿主不接管（`engine_host_tick=none`）——默认绝不允许出现第二个 tick；
    #        · 开关通路：`--engine-host-tick` 之下结论仍必须落在 unavailable/failed 之一
    #          （离线宿主的期望结果不变），并且如实报出"谁在跑固定步"。
    if ($hostOffLine -notmatch 'engine_host_tick=none') {
        throw ("生产口径下引擎宿主不得接管固定步（必须是 engine_host_tick=none），实际：{0}" -f $hostOffLine)
    }
    $hostTickCase = Invoke-HarnessCase -ExtraArgs @("--frames", "30", "--engine-host-tick") -Label "engine-host-tick"
    if ($hostTickCase.ExitCode -ne 0) { throw ("--engine-host-tick 用例失败（exit {0}）" -f $hostTickCase.ExitCode) }
    $hostTickOk = $hostTickCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_ok" } | Select-Object -First 1
    if (-not $hostTickOk) { throw "engine-host-tick 用例未输出 wechat_minigame_harness_ok" }
    $tickStatus = if ($hostTickOk -match '(?:^|\s)engine_host=(\S+)') { $matches[1] } else { "缺失" }
    $tickReason = if ($hostTickOk -match 'engine_host_reason="([^"]*)"') { $matches[1] } else { "" }

    # WP7-90：**单一 tick 所有者**不变式 —— 两个开关用例此前只查"宿主落在哪个具名结果"，
    # 没查"世界有没有被推进两次"。接管/交还（WP8）的前提正是这条：任何时刻只有一个 tick 拥有者，
    # 输入注入只在**运行时**那条路径上发生一次（否则两套状态各推一步，症状是"帧数对但世界跑快了"）。
    # 三条都取自已报字段，判据是**具名取值**而不是"看起来正常"。
    function Assert-SingleTickOwner([string]$line, [string]$label) {
        if ($line -notmatch 'engine_host_tick_conflict=(\S+)') { throw ($label + "：自报里缺 engine_host_tick_conflict") }
        $conflict = $matches[1]
        if ($conflict -ne 'no') { throw ($label + "：tick 冲突=" + $conflict + "（有两个 tick 拥有者 ⇒ 世界会被推进两次）") }
        if ($line -notmatch 'engine_host_input_applied=(\d+)') { throw ($label + "：自报里缺 engine_host_input_applied") }
        if ($matches[1] -ne '0') { throw ($label + "：宿主也注入了输入（input_applied=" + $matches[1] + "）⇒ 输入被应用两次") }
        if ($line -notmatch 'engine_host_tick=(\S+)') { throw ($label + "：自报里缺 engine_host_tick") }
        $owner = $matches[1]
        if (($owner -ne 'none') -and ($owner -ne 'runtime') -and ($owner -ne 'host')) { throw ($label + "：engine_host_tick 不是具名值：" + $owner) }
        Write-Host ("    [单一tick] " + $label + "：conflict=no input_applied=0 tick=" + $owner) -ForegroundColor DarkGray
    }
    Assert-SingleTickOwner $hostOk "engine-host-switch"
    Assert-SingleTickOwner $hostTickOk "engine-host-tick"

    # WP7-94：**接管 → 交还**往返（离线能证的部分）。第 15 帧把"谁推进世界"的标记还回运行时，
    # 之后必须：① 只有运行时在推进（timing 段以 tick=runtime 收尾）；② 没有第二个 tick 拥有者；
    # ③ 宿主没有替运行时注入输入。触发点挂在 rAF 垫片的语句位置 ⇒ 与是否被驱动无关
    # （第 127 轮的失败正是因为放在了驱动路径的泵帧循环里，GPU 档 move=not-driven 触发不到）。
    # **离线限制**：离线档位下宿主从未 running（unavailable/failed），所以本用例证明的是
    # "标记语义与推进归属正确"，**不是**"真宿主交还"——后者属设备侧。
    $handbackCase = Invoke-HarnessCase -ExtraArgs @("--frames", "30", "--engine-host-off-at", "15") -Label "engine-host-handback"
    if ($handbackCase.ExitCode -ne 0) { throw ("--engine-host-off-at 用例失败（exit " + $handbackCase.ExitCode + "）") }
    $handbackOk = $handbackCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_ok" } | Select-Object -First 1
    if (-not $handbackOk) { throw "engine-host-handback 用例未输出 wechat_minigame_harness_ok" }
    $handbackLog = $handbackCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_handback" } | Select-Object -First 1
    if (-not $handbackLog) { throw "往返用例没有真的交还（日志里没有 wechat_minigame_harness_handback）—— 开关没生效" }
    Assert-SingleTickOwner $handbackOk "engine-host-handback"
    if ($handbackOk -notmatch 'timing="[^"]*tick=runtime') {
        throw ("交还后世界必须由运行时推进（timing 段应以 tick=runtime 收尾），实际：" + ([regex]::Match($handbackOk, 'timing="[^"]*"').Value))
    }
    Write-Host ("    [交还] " + $handbackLog.Trim() + "；timing=" + ([regex]::Match($handbackOk, 'timing="[^"]*"').Value)) -ForegroundColor DarkGray


    $tickAbi = if ($hostTickOk -match '(?:^|\s)engine_host_abi=(\S+)') { $matches[1] } else { "缺失" }
    switch ($tickStatus) {
        "unavailable" {
            if ($tickReason -notmatch '^kernel-without-gpu2d$') {
                throw ("unavailable 必须带具名理由 kernel-without-gpu2d，实际：{0}" -f $tickReason)
            }
        }
        "failed" {
            if ($tickReason -notmatch '^host: ') {
                throw ("GPU 档内核的失败必须带引擎原文（以 'host: ' 开头），实际：{0}" -f $tickReason)
            }
        }
        "running" { throw "离线的假宿主不该给出 running（宿主读数可疑）：{0}" -f $hostTickOk }
        default { throw ("指定宿主接管固定步后必须落在 unavailable/failed 之一，实际 status={0}" -f $tickStatus) }
    }
    Write-Host ("引擎宿主接管固定步：status={0} abi={1} reason={2}" -f $tickStatus, $tickAbi, $tickReason) -ForegroundColor DarkGray

    # 1a6) 「一个世界只能有一个 tick」（WP7-57）。两条断言，缺一条都等于没验：
    #      · `tick_conflict=no`：宿主接管时运行时不许再跑 update。离线宿主起不来 ⇒ 标记根本没交出去，
    #        所以这里的 no 恰好证明"起不来 ⇒ 不接管"这条降级路径通了；
    #      · `runtime_run_steps>0`：证明运行时**确实还在推进世界**。少了它，"标记交出去了但宿主死了"
    #        会表现成"脚本照画、逻辑不动"——画面正常，最难查。所以宁可用一个非零计数把"世界没冻住"
    #        写死在这里。
    foreach ($probe in @([pscustomobject]@{ Name = '生产口径'; Line = $hostOffLine },
                         [pscustomobject]@{ Name = '--engine-host-tick'; Line = $hostTickOk })) {
        if ($probe.Line -match 'engine_host_tick_conflict=yes') {
            throw ("{0}：运行时与宿主同时推进了世界（tick_conflict=yes）——这必须是不可达状态：{1}" -f $probe.Name, $probe.Line)
        }
        $runSteps = if ($probe.Line -match 'engine_host_runtime_run_steps=(\d+)') { [int]$matches[1] } else { -1 }
        if ($runSteps -le 0) {
            throw ("{0}：运行时的固定步计数应为正（世界必须有人推进），实际 engine_host_runtime_run_steps={1}" -f $probe.Name, $runSteps)
        }
        # 常驻读数本身也要被验：`sim=`/`tick=` 是"谁在推进世界"的真机可读证据。离线宿主起不来
        # ⇒ 推进者必须是 `runtime`（真机上宿主起来时这里会是 `host`，那属于设备侧的读数）。
        $timing = if ($probe.Line -match 'timing="([^"]*)"') { $matches[1] } else { '' }
        if ($timing -notmatch 'sim=\d+') {
            throw ("{0}：data-rustlua-timing 必须带 sim=（本帧真正推进世界的步数），实际：{1}" -f $probe.Name, $timing)
        }
        if ($timing -notmatch 'tick=runtime$') {
            throw ("{0}：离线（宿主起不来）时推进者必须是 runtime，实际：{1}" -f $probe.Name, $timing)
        }
        # 1a7) 输入注入也只有一个所有者（WP7-58）：「谁注入输入」必须与「谁推进世界」同源。
        #      断言两件事：运行时**确实**在注入（>0）；宿主**一次都没**注入（input_applied=0）。
        #      这两条合起来就是"注入来自恰好一处"。离线宿主起不来 ⇒ input_applied 恒 0 是必然的，
        #      但这条契约必须写进产物读数里——真机上宿主起来时它才真正开始起作用（那属于设备侧）。
        $inputFrames = if ($probe.Line -match 'engine_host_runtime_input_frames=(\d+)') { [int]$matches[1] } else { -1 }
        if ($inputFrames -le 0) {
            throw ("{0}：运行时必须在注入输入批量（否则整局对按键没反应），实际 engine_host_runtime_input_frames={1}" -f $probe.Name, $inputFrames)
        }
        $hostApplied = if ($probe.Line -match 'engine_host_input_applied=(\d+)') { [int]$matches[1] } else { -1 }
        if ($hostApplied -ne 0) {
            throw ("{0}：宿主不得注入输入（注入的唯一所有者是运行时），实际 engine_host_input_applied={1}" -f $probe.Name, $hostApplied)
        }
        # 1a8) 「恰好一次」的不变量（WP7-58；先测出真实关系再收紧，不从直觉收紧）。
        #      推进者是运行时（`tick=runtime`）时，注入与推进**同条件**（都是 `steps > 0`）
        #      ⇒ 两个计数必须相等。这不是"实测凑巧相等"，而是同一条判断的两个分支；
        #      谁把注入挪进别处，这里立刻红。（真机上宿主接管时两者不再相等——注入每帧都做、
        #      推进归宿主，所以这条只在 `tick=runtime` 的产物上断言。）
        if ($timing -match 'tick=runtime$' -and $inputFrames -ne $runSteps) {
            throw ("{0}：推进者是运行时 ⇒ 注入次数必须等于推进次数，实际 input_frames={1} run_steps={2}" -f `
                $probe.Name, $inputFrames, $runSteps)
        }
        # 1a9) 隐藏文本集跟着状态载荷到达（WP7-59）：帧循环不再每帧单独拉一次。
        #      这条断言之所以必要，是因为"少一次跨界"和"文字不再被隐藏"在读数上长得一模一样
        #      （都得看画面）——所以用 pulls 计数把"老路还在不在"变成可断言的数：
        #      pulls 必须**小于帧数**（正常为 0；只有兼容层覆盖了导出函数才会回退到老路）。
        $pulls = if ($probe.Line -match 'export_readout="pulls=(\d+)') { [int]$matches[1] } else { -1 }
        if ($pulls -lt 0) {
            throw ("{0}：自报里没有 data-rustlua-export 读数（pulls=），无法判断隐藏文本集走的是哪条路" -f $probe.Name)
        }
        $framesSeen = if ($probe.Line -match '(?:^|\s)frames=(\d+)') { [int]$matches[1] } else { 0 }
        if ($framesSeen -le 0) { throw ("{0}：自报里没有 frames= 读数，无法比较 pulls" -f $probe.Name) }
        if ($pulls -ge $framesSeen) {
            throw ("{0}：隐藏文本集仍在被每帧单独拉取（pulls={1} >= frames={2}）——状态载荷里的 H 行没生效" -f `
                $probe.Name, $pulls, $framesSeen)
        }
        # 1a10) `T` 行按变化发（WP7-60）。不变量：**全量重发 = 帧数 × 存活实体数**，所以
        #       `tlines` 必须**严格小于**这个上限，否则说明"按变化发"根本没生效（回到了每帧全发）。
        #       这条同时把"基线读数真的在采"这件事验了：`state=` 必须是正数。
        $tlines = if ($probe.Line -match 'export_readout="pulls=\d+ state=(\d+) tlines=(\d+)') { [int]$matches[2] } else { -1 }
        $stateBytesRead = if ($probe.Line -match 'export_readout="pulls=\d+ state=(\d+)') { [int]$matches[1] } else { -1 }
        if ($tlines -lt 0 -or $stateBytesRead -lt 0) {
            throw ("{0}：自报里缺 state=/tlines= 读数（状态载荷基线没在采）" -f $probe.Name)
        }
        $entities = if ($probe.Line -match 'scene=(\d+)') { [int]$matches[1] } else { 0 }
        if ($entities -le 0) { throw ("{0}：world 读数里没有 scene= 实体数，无法判断 tlines 是否真的减少了" -f $probe.Name) }
        $fullEmit = $framesSeen * $entities
        # 空 的 空洞陷阱：默认用例跑的是**标题画面**，而标题画面的实体来自 `.scene` 文件、
        # 由 JS 侧装载，Lua 侧的变换表里一个都没有（world 读数会写 alive=0）⇒ `tlines` 本来是 0，
        # 于是"0 < 上限"这种断言**无论按变化发有没有生效都会绿**。真正可断言的是**因果关系**：
        #   · `alive=0` ⇒ 走 Lua 变换通道的实体本来就没有 ⇒ `tlines` 必须是 0（不多不少）；
        #   · `alive>0` ⇒ 必须 `0 < tlines < 上限`（真的按变化发）。
        # 严格版另有驱动用例那条路（那时主角真的在走，`tlines` 必然为正）。
        $alive = if ($probe.Line -match 'alive=(\d+)') { [int]$matches[1] } else { -1 }
        if ($alive -lt 0) { throw ("{0}：world 读数里没有 alive= 计数，无法判断 tlines=0 是不是「没实体可发」" -f $probe.Name) }
        if ($alive -eq 0) {
            if ($tlines -ne 0) {
                throw ("{0}：Lua 侧没有存活实体（alive=0）却发出了 {1} 行 T——状态载荷的实体来源可疑" -f $probe.Name, $tlines)
            }
        } else {
            if ($tlines -le 0) {
                throw ("{0}：有 {1} 个存活实体却一行 T 都没发（首帧/换场景也必须各发一次）" -f $probe.Name, $alive)
            }
            if ($tlines -ge $fullEmit) {
                throw ("{0}：T 行看起来仍是每帧全量重发（tlines={1} >= 帧数×实体数={2}）——按变化发没生效" -f `
                    $probe.Name, $tlines, $fullEmit)
            }
        }
        # 1a11) 状态回写有唯一所有者（WP7-61）。帧循环那次导出**每帧至多一次**：
        #       以前 3 个调用点各自直调 `__rustlua_export_state`，而它一进来就把 Lua 侧的 op
        #       队列清空 ⇒ "谁取走了什么"取决于调用顺序。收敛成一个所有者之后，
        #       `frame=` 计数必须 ≤ 帧数；若有**第二份实现**冒出来，这个数就会超过帧数。
        #       非空洞的条件：当 `runtime_run_steps == 帧数`（每帧都走到了帧级阶段、没有提前返回）
        #       时，"至多一次"必须加强成"恰好一次"——否则说明有帧**漏了**导出。
        $frameExports = [regex]::Match($probe.Line, 'exports=\d+ frame=(\d+)').Groups[1].Value
        if (-not $frameExports) { throw ("{0}：自报里缺 exports=/frame= 读数（状态回写的所有者没在计数）" -f $probe.Name) }
        $frameExports = [int]$frameExports
        if ($frameExports -le 0) { throw ("{0}：帧循环一次状态回写都没发生（所有者没被用上）" -f $probe.Name) }
        if ($frameExports -gt $framesSeen) {
            throw ("{0}：状态回写在帧循环里发生了 {1} 次（帧数 {2}）——每帧至多一次，可能出现了第二份实现" -f `
                $probe.Name, $frameExports, $framesSeen)
        }
        if ($runSteps -eq $framesSeen -and $frameExports -ne $framesSeen) {
            throw ("{0}：每帧都推进了世界（run_steps={1}=帧数）却只导出了 {2} 次状态——有帧漏了回写" -f `
                $probe.Name, $runSteps, $frameExports)
        }
    }
    $offState = if ($hostOffLine -match 'state=(\d+)') { $matches[1] } else { '?' }
    $offTlines = if ($hostOffLine -match 'tlines=(\d+)') { $matches[1] } else { '?' }
    $offFull = ([int]$(if ($hostOffLine -match 'frames=(\d+)') { $matches[1] } else { 0 })) * ([int]$(if ($hostOffLine -match 'scene=(\d+)') { $matches[1] } else { 0 }))
    $tickState = if ($hostTickOk -match 'state=(\d+)') { $matches[1] } else { '?' }
    $tickTlines = if ($hostTickOk -match 'tlines=(\d+)') { $matches[1] } else { '?' }
    $tickFull = ([int]$(if ($hostTickOk -match 'frames=(\d+)') { $matches[1] } else { 0 })) * ([int]$(if ($hostTickOk -match 'scene=(\d+)') { $matches[1] } else { 0 }))
    Write-Host ("状态载荷基线：生产 state={0} B / tlines={1}（全量上限 {2}）— 接管用例 state={3} B / tlines={4}（上限 {5}）" -f `
        $offState, $offTlines, $offFull, $tickState, $tickTlines, $tickFull) -ForegroundColor DarkGray
    # 两处修正（本轮的教训）：① `pulls=` 后面**还有** state=/tlines=，所以模式不能要求右引号
    # 紧跟数字（否则永远印 `?`——仪器看起来坏了，其实是被测串变长了）；② 不要用 `$matches`
    # 自动变量：它会被后面任何一次 -match 覆盖。直接取 [regex]::Match 的结果。
    Write-Host ("状态导出：隐藏文本集单独拉取次数 生产 presses={0} / 接管用例 presses={1}（帧数 {2}/{3}）" -f `
        [regex]::Match($hostOffLine, 'pulls=(\d+)').Groups[1].Value, `
        [regex]::Match($hostTickOk, 'pulls=(\d+)').Groups[1].Value, `
        [regex]::Match($hostOffLine, '(?:^|\s)frames=(\d+)').Groups[1].Value, `
        [regex]::Match($hostTickOk, '(?:^|\s)frames=(\d+)').Groups[1].Value) -ForegroundColor DarkGray
    Write-Host ("输入注入的唯一所有者：生产 input_frames={0} / 接管用例 input_frames={1}（宿主 input_applied=0）" -f `
        $(if ($hostOffLine -match 'engine_host_runtime_input_frames=(\d+)') { $matches[1] } else { '?' }), `
        $(if ($hostTickOk -match 'engine_host_runtime_input_frames=(\d+)') { $matches[1] } else { '?' })) -ForegroundColor DarkGray
    Write-Host ("单 tick 与推进者：生产 runtime_run_steps={0} / 接管用例 runtime_run_steps={1}" -f `
        $(if ($hostOffLine -match 'engine_host_runtime_run_steps=(\d+)') { $matches[1] } else { '?' }), `
        $(if ($hostTickOk -match 'engine_host_runtime_run_steps=(\d+)') { $matches[1] } else { '?' })) -ForegroundColor DarkGray

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
        # 第四形态（remote + 内核入分包，第 53 轮）：分包轴的每条用例都必须**同时**带上远端——
        # 这一形态的包内只有内核与骨架，重资源在 CDN，不接远端时用例会以"资源 404"失败，
        # 那测的就不是分包轴而是资源来源了（实测：`subpackage-ok` 不带远端时 boot 拒绝
        # `pack/scenes/main.scene HTTP 404`）。经典分包形态（资源全在包里）不带远端，保持原样。
        $subRemoteArgs = if ($isWasmInPackRemote) { @("--remote", "thin", "--remote-root", $assetPackDir) } else { @() }
        $subpackageCases = @(
            @{ Label = "subpackage-ok"; Args = ($subRemoteArgs + @("--subpackage", "ok")); Token = "wechat_minigame_harness_ok"; Evidence = 'subpackage=pack' },
            @{ Label = "subpackage-delay"; Args = ($subRemoteArgs + $subFrames + @("--subpackage", "delay", "--subpackage-delay-frames", "6", "--drive-new-game", "--expect-runtime-spawns")); Token = "wechat_minigame_harness_ok"; Evidence = 'loading_bar=ok' },
            @{ Label = "subpackage-delay-move"; Args = ($subRemoteArgs + $subFrames + @("--subpackage", "delay", "--subpackage-delay-frames", "6", "--drive-new-game", "--expect-runtime-spawns", "--drive-move", "20")); Token = "wechat_minigame_harness_ok"; Evidence = 'mount_frame=6' },
            @{ Label = "subpackage-never"; Args = ($subRemoteArgs + @("--subpackage", "never", "--expect-pack-failure")); Token = "wechat_minigame_harness_ok"; Evidence = 'pack_state=failed' },
            @{ Label = "subpackage-fail-once"; Args = ($subRemoteArgs + $subFrames + @("--subpackage", "fail-once", "--drive-new-game", "--expect-runtime-spawns")); Token = "wechat_minigame_harness_ok"; Evidence = 'subpackage_loads=2' },
            # 适配层**不得**依赖 `game.json`：它在真机上读不到（平台配置不向 FileSystemManager 开放），
            # 而"要不要挂分包"曾经拿它当唯一判据 → 真机上分包静默不挂（实测）。这条断言把该依赖钉死。
            @{ Label = "no-game-json-dependency"; Args = ($subRemoteArgs + @("--frames", "40", "--expect-no-read", "game.json")); Token = "wechat_minigame_harness_ok"; Evidence = 'pack_state=ready' }
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
            # 第四形态（内核在分包 + 重资源远端）没有 mainpack/ 路由，这条改成它自己的等价物：
            # 清单必须**被读到并声明了 pack 分包**（否则适配层不会去挂分包），而首屏读取允许
            # 走分包（那是这一形态的设计：内核与骨架都在包里，重资源走远端）。
            if ($isWasmInPackRemote) {
                if (-not ($okLine -match 'subpackage=pack')) {
                    throw ("第四形态用例没有从首屏清单读到 pack 分包声明：{0}" -f $okLine)
                }
            }
            elseif (-not ($okLine -match 'mainpack_reads=[1-9]')) {
                throw ("分包形态用例没有走主包 mainpack/ 读取（首屏清单没生效）：{0}" -f $okLine)
            }
            # 启动期间不得有 pack/ 读取被未挂载的分包挡下（那正是"清单漏列"的直接症状）。
            # 第四形态的等价物：被挡的读取必须**全部发生在挂载之前**（denied_before_boot ≤
            # denied_before_mount）。若 boot 段还多出挂载之后的那部分，说明"等分包"没覆盖到某条路径。
            if ($isWasmInPackRemote) {
                $deniedBoot = if ($okLine -match 'denied_before_boot=(\d+)') { [int]$matches[1] } else { -1 }
                $deniedMount = if ($okLine -match 'denied_before_mount=(\d+)') { [int]$matches[1] } else { -1 }
                if ($deniedBoot -lt 0 -or $deniedMount -lt 0) {
                    throw ("第四形态用例缺少 denied_before_* 读数（无法判断「等分包」是否覆盖启动期）：{0}" -f $okLine)
                }
                if ($deniedBoot -gt $deniedMount) {
                    throw ("第四形态用例在分包挂载**之后**仍有 {0} 次 pack/ 读取被挡下：{1}" -f ($deniedBoot - $deniedMount), $okLine)
                }
            }
            elseif (-not ($okLine -match 'denied_before_boot=0')) {
                throw ("分包形态用例出现「启动期间读不到 pack/」——首屏清单漏列了首屏资源：{0}" -f $okLine)
            }
            Write-Host $okLine -ForegroundColor DarkGray
        }
    }

    # 2) 远端回落：包内读不到 pack/ 时，素材必须真的从服务下载、按清单校验、落地缓存并复用。
    #    harness 会自己起一个本地静态服务（复用 scripts/pack-server.js），因此这里不依赖外部进程。
    #    瘦首包下这个"完整资源"只能来自导出目录，所以要把 --remote-root 指过去。
    #
    #    **经典分包形态不跑这一组**：那一形态按设计就没有 remote-assets.json（发布脚本互斥拒绝），
    #    硬塞 `--remote auto` 只会得到"包内读不到 + 没有远端配置"的假失败。remote 形态的
    #    回归由 thin/full 形态的产物各自负责（同一次质量门里会分别验证）。
    #    **第四形态（remote + 内核入分包）反过来**：远端就是它的主路径（重资源全在 CDN），
    #    这一组必须跑——跳过它等于把校验和、缓存复用、服务不可达这几条守卫全丢掉。
    $remoteOkLine = $null
    $negativeCases = @()
    if ($isSubpackagePack -and -not $isWasmInPackRemote) {
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
    if ($isWasmInPackRemote) {
        # 第四形态：需要改写 src 的原因是**重资源在 CDN**（不是 mainpack→pack 前缀），
        # 所以这条用例必须带远端。不带远端时 boot 会先因"包内只有骨架"404 失败，
        # 掩盖掉真正要验的那件事（宿主不让改写 src 时必须响亮失败）——实测踩过。
        $frozenLoudArgs = @("--image-src-frozen", "--remote", "thin", "--remote-root", $assetPackDir)
    }
    elseif ($isSubpackagePack) {
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
    #
    # 第 77/78 轮修：只给 `--image-src-silent` **造不出卡死** —— 实测 7 张贴图仍 pending 而 boot
    # 照样 settle（`images_loaded=0 image_failures=0 boot=booted`）。适配层的启动链是
    # 「分包挂载 → wasm → 字体（同步）→ Lua boot」，**贴图不在等待集合里**（首屏不等贴图是刻意
    # 设计）。所以卡死要造在 boot 真等的那一环上，而每个形态"真等的那一环"不同：
    #   · 分包形态：内核就在 pack/ 里 ⇒ 等分包下载 ⇒ `--subpackage silent`（宿主收下
    #     loadSubpackage 与进度注册、永不回调）；
    #   · 瘦首包/远端形态：骨架在包里、重资源在 CDN ⇒ 等那一次下载 ⇒ `--remote silent`
    #     （宿主收下 request/downloadFile、永不回调；配置里的 timeoutMs 特意放大到 120s，
    #     否则"挂起"会被客户端自己的 3s 超时终结，那就不是挂起了）。
    # 两个静默一起给：形态判定取产物自身（`game.json` 分包声明 / `remote-assets.json`），不猜。
    $stallArgs = @("--image-src-silent", "--expect-boot-stall", "--boot-timeout-ms", "1000", "--boot-deadline-ms", "6000")
    if ($isSubpackagePack) { $stallArgs += @("--subpackage", "silent") }
    if ($isThinPack) { $stallArgs += @("--remote", "silent", "--remote-root", $assetPackDir) }
    $stallCase = Invoke-HarnessCase -ExtraArgs $stallArgs -Label "boot-stall-watchdog"
    if ($stallCase.ExitCode -ne 0) { throw "启动卡死时看门狗没有兜住（--image-src-silent --expect-boot-stall, exit $($stallCase.ExitCode)）" }
    if (-not ($stallCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_negative_ok case=boot-stall" })) {
        $stallVacuous = @($stallCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_negative_vacuous case=boot-stall" })
        if ($stallVacuous.Count -gt 0) {
            # 卡死没造出来：boot 在截止时间前就正常结算了。剩下的唯一解释是 **full 形态**
            # （既没有分包、也没有远端配置）：资源全在代码包里，boot 的等待集合
            # （分包 → wasm → 字体 → Lua boot）全是本地快速路径，**没有网络等待可静默**。
            # 这一形态今天覆盖不到看门狗 —— 明写出来，而不是把断言放宽成"空转也通过"。
            # （分包形态、瘦首包/远端形态都必须真的卡死，否则等于看门狗不再被验证。）
            if (-not $isSubpackagePack -and -not $isThinPack) {
                Write-Host "    [跳过] 启动看门狗自证：full 形态 boot 的等待集合里没有可静默的网络资源（$($stallVacuous[0].Trim())）"
            }
            else {
                throw "启动卡死本该发生却没发生，看门狗自证没有被验证：$($stallVacuous[0].Trim())"
            }
        }
        else {
            throw "启动卡死用例未输出 wechat_minigame_harness_negative_ok case=boot-stall（看门狗自证没有被验证）"
        }
    }
    elseif (-not ($stallCase.Lines | Where-Object { $_ -match "on_screen=yes" })) {
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

    # 6.5) 玩法驱动（`-DriveJhyt`）：微信侧**没有 URL 旋钮**，所以"点开始新游戏 → 按住方向键
    #      主角真的走"这条玩法链路一直只有 webgl 的 `-DriveJhyt` 在测。harness 侧的驱动本来就有
    #      （`--drive-new-game --drive-move N`：标题按确认 → `scene.load` 换场景 → 按住 'w' N 帧 →
    #      比对 `data-rustlua-world` 里的主角坐标），缺的是验收脚本这条通道。
    # 为什么它对 WP8 是前置：拆旧 JS 世界之前，必须先有"微信上玩法真的能玩"的机器判据，
    # 否则拆完只剩"启动没报错"，而玩法是半死的（历史上正是"能进图但不能玩"这种状态全绿过）。
    # 断言两层，缺一不可（第 77 轮的教训：用例"跑了但没生效"必须变成红灯）：
    #   ① 用例 exit=0（harness 自己在"一动没动"时会报红）；
    #   ② 自报里必须真的出现非零位移 `move=… d=dx,dy`，否则说明这段驱动根本没执行。
    if ($DriveJhyt) {
        $driveArgs = @("--frames", "120", "--drive-new-game", "--drive-move", "12")
        if ($isThinPack) { $driveArgs += @("--remote", "thin", "--remote-root", $assetPackDir) }
        $driveCase = Invoke-HarnessCase -ExtraArgs $driveArgs -Label "drive-gameplay"
        if ($driveCase.ExitCode -ne 0) { throw "微信侧玩法驱动失败（--drive-new-game --drive-move 12, exit $($driveCase.ExitCode)）" }
        $driveOk = @($driveCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_ok" })
        if ($driveOk.Count -eq 0) { throw "玩法驱动用例未输出 wechat_minigame_harness_ok（世界/运动断言没有被验证）" }
        $moveField = ($driveOk[0] -split 'move=')[1]
        if (-not $moveField) { throw "玩法驱动用例的自报里没有 move= 字段（驱动没有执行）" }
        $moveText = ($moveField -split ' touch_stick=')[0].Trim().Trim('"')
        $dm = [regex]::Match($moveText, 'd=(-?\d+),(-?\d+)')
        if (-not $dm.Success) { throw "玩法驱动的位移读数无法解析（move=$moveText）" }
        $dx = [int]$dm.Groups[1].Value; $dy = [int]$dm.Groups[2].Value
        if (($dx -eq 0) -and ($dy -eq 0)) {
            throw "玩法驱动：按住方向键 12 帧后主角一动没动（move=$moveText）—— 输入桥接→物理→回写这条链路在微信侧没生效"
        }
        Write-Host ("    [玩法] " + $moveText)
        # WP7-60 的**非空洞**断言落在这里：这条用例真的按了方向键、主角真的在走，所以
        # Lua 侧的变换表里必然有存活实体在变 ⇒ `tlines` 必须为正，且仍要小于"每帧全量重发"的
        # 上限（否则说明按变化发没生效，只是恰好这一局没红）。默认用例跑的是标题画面
        # （`alive=0`，没有走 Lua 变换通道的实体），那里的 `tlines` 只能是 0——两者合起来
        # 才把"机制生效"和"空洞通过"分开。
        $driveLine = $driveOk[0]
        if ($driveLine -match 'export_readout="pulls=\d+ state=(\d+) tlines=(\d+)') {
            $driveTlines = [int]$matches[2]
            $driveAlive = if ($driveLine -match 'alive=(\d+)') { [int]$matches[1] } else { 0 }
            $driveFrames = if ($driveLine -match '(?:^|\s)frames=(\d+)') { [int]$matches[1] } else { 0 }
            if ($driveAlive -le 0) {
                throw ("玩法驱动：world 读数里 alive={0}（按变化发的机制在这条用例上仍然空洞）" -f $driveAlive)
            }
            if ($driveTlines -le 0) {
                throw ("玩法驱动：alive={0} 却一行 T 都没发——按变化发的影子表把该发的也吞了" -f $driveAlive)
            }
            if ($driveTlines -ge ($driveFrames * $driveAlive)) {
                throw ("玩法驱动：T 行仍是每帧全量重发（tlines={0} >= 帧数×实体数={1}）——按变化发没生效" -f `
                    $driveTlines, ($driveFrames * $driveAlive))
            }
            Write-Host ("    [状态] tlines={0}（alive={1} 帧数={2}，全量上限 {3}）" -f `
                $driveTlines, $driveAlive, $driveFrames, ($driveFrames * $driveAlive))
              # WP7-63：把**进图之后**的载荷基线也印出来（标题画面那组读数不代表真的玩起来的时候）。
              # 第 91 轮只在这里印了 tlines，于是"进图后状态/UI 载荷多大"一直是个洞——
              # 而"要不要换二进制批量"正是靠它判定的，所以先补上这个洞。
              $driveBytes = [regex]::Match($driveLine, 'state=(\d+)').Groups[1].Value
              $driveUi = [regex]::Match($driveLine, 'ui=(\d+)').Groups[1].Value
              $driveUiSame = [regex]::Match($driveLine, 'uiSame=(\d+)').Groups[1].Value
              $driveUiI = [regex]::Match($driveLine, 'uiI=(\d+):(\d+)')
              Write-Host ("    [载荷] state={0} B / ui={1} B / uiSame={2} 帧 / 图片 {3} 行 {4} B（共 {5} 帧）" -f `
                  $driveBytes, $driveUi, $driveUiSame, $driveUiI.Groups[1].Value, $driveUiI.Groups[2].Value, $driveFrames)
              # WP7-64：这两个导出的**实测开销**（微秒累计 + 单帧最大；只在诊断开启时有值）。
              # 前面几轮量到的是规模（字节/行数），从这里开始量时间——"值不值得优化"要靠它判。
              $driveStateUs = [regex]::Match($driveLine, 'stateUs=(\d+)').Groups[1].Value
              $driveUiUs = [regex]::Match($driveLine, 'uiUs=(\d+)').Groups[1].Value
              $driveStateMax = [regex]::Match($driveLine, 'stateMaxUs=(\d+)').Groups[1].Value
              $driveUiMax = [regex]::Match($driveLine, 'uiMaxUs=(\d+)').Groups[1].Value
              Write-Host ("    [开销] state 导出 {0} µs 累计 / 单帧最大 {1} µs；ui 导出 {2} µs 累计 / 单帧最大 {3} µs（{4} 帧）" -f `
                  $driveStateUs, $driveStateMax, $driveUiUs, $driveUiMax, $driveFrames)
        # WP7-89：**确定性**断言 —— 同一份输入剧本跑两遍，**画布无关的计数**必须逐项相同。
        # 为什么它是 WP8 的前置：接管（引擎宿主 / GPU 渲染）之后的 A/B 比较，前提是"同一输入序列 ⇒ 同一读数"；
        # 否则两遍之间的差异分不清是"接管造成的"还是"这一局本来就不一样"。
        # 只比计数，不比时间：µs/slow2ms 在本机抖动约 5%，比它等于自找红灯（第 105 轮的测量纪律）。
        $driveCase2 = Invoke-HarnessCase -ExtraArgs $driveArgs -Label "drive-gameplay-repeat"
        if ($driveCase2.ExitCode -ne 0) { throw "确定性用例（第二遍）失败（exit $($driveCase2.ExitCode)）" }
        $driveOk2 = @($driveCase2.Lines | Where-Object { $_ -match "wechat_minigame_harness_ok" })
        if ($driveOk2.Count -eq 0) { throw "确定性用例（第二遍）未输出 wechat_minigame_harness_ok" }
        $driveLine2 = $driveOk2[0]
        $detFields = @('state=(\d+)', 'tlines=(\d+)', 'ui=(\d+)', 'uiSame=(\d+)', 'uiP=(\d+):(\d+)', 'xc=([\d/]+)', 'tiles=([\d/]+)', 'culled=(\d+)', 'drawn=(\d+)', 'draw_ops=(\d+)', 'alive=(\d+)', 'frames=(\d+)')
        $detDiffs = @()
        foreach ($detPat in $detFields) {
            $detA = [regex]::Match($driveLine, $detPat).Value
            $detB = [regex]::Match($driveLine2, $detPat).Value
            if ($detA -ne $detB) { $detDiffs += ("{0}: 第一遍 [{1}] / 第二遍 [{2}]" -f $detPat, $detA, $detB) }
        }
        if ($detDiffs.Count -gt 0) {
            throw ("玩法驱动**不确定**：同一份输入剧本两遍读数不同 —— 接管前后的 A/B 无法成立：`n  - " + ($detDiffs -join "`n  - "))
        }
        Write-Host ("    [确定性] 同一输入剧本两遍读数逐项相同（{0} 个计数）" -f $detFields.Count)
        } else {

            throw "玩法驱动：自报里缺 state=/tlines= 读数，无法验证按变化发"
        }
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

    # 存档落盘（docs/51）：web/微信的 `save` 不是引擎那份 `std::fs` 后端，而是 web-runtime.js
    # 里 Lua 补丁自己实现的**第三份**（键值行编码 + Lua 任意值长度前缀树 + 宿主存储读写回调）。
    # 它坏掉的形状是"全部成功、零报错"：`commit` 只改内存，玩家退出即丢进度，控制台一个字不打。
    # 因此断言落在**用户可见的症状**上：宿主存储里真的有 save_version=2 的 payload、键名与引擎
    # 对齐（`__blob:`/`__map`/`__mapstate:`/`__map:spawn:`）、排序确定性、缺档/坏档四种损坏各自
    # 的返回值，以及真实游戏 save_manager 提交上去的字节数。
    $saveArgs = @("--drive-new-game", "--expect-runtime-spawns", "--check-save-persist", "--frames", "240")
    if ($isThinPack) { $saveArgs += @("--remote", "thin", "--remote-root", $assetPackDir) }
    $saveCase = Invoke-HarnessCase -ExtraArgs $saveArgs -Label "save-persist"
    if ($saveCase.ExitCode -ne 0) { throw "存档落盘用例失败（--drive-new-game --check-save-persist, exit $($saveCase.ExitCode)）" }
    $saveOkLine = $saveCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_ok" } | Select-Object -First 1
    if (-not $saveOkLine) { throw "存档落盘用例未输出 wechat_minigame_harness_ok" }
    # 证据必须带两段字节数：probe= 是隔离探针的 payload，game= 是**真实游戏** save_manager
    # 提交上去的 payload。只看到 ok 分不清"游戏那条链路真的写到了宿主存储"还是探针自己玩自己。
    if (-not ($saveOkLine -match 'save_persist="ok,probe=\d+,game=(\d+)"')) {
        throw "存档落盘用例没有自报 payload 证据（缺少 save_persist=「ok,probe=N,game=M」）：$saveOkLine"
    }
    if ([int]$Matches[1] -lt 1000) {
        throw ("存档落盘用例的**真实游戏** payload 只有 {0} 字节（新档快照应有十几个 provider 键，" +
            "这个数说明什么都没写进去）" -f $Matches[1])
    }
    Write-Host $saveOkLine -ForegroundColor DarkGray

    # UI 两层的绘制序（实机 2026-10-07）：预设卡底 `ui_skill_card_9s.png` 主体全不透明，一旦
    # 画在动态招式图标之后就把 24×24 的图标整块埋掉——两个宿主一起瞎，而状态层读数照样自报
    # "图标存在"。原生侧由 jhyt_m70 的 submission_order 钉 `mesh.draws` 下标；web 的 drawUi 是
    # 另写的一份循环，所以这里读**假 ctx 记下的真实 drawImage 先后 + 落点矩形**，只比几何上
    # 真的压在卡里的图标（同一张卡皮还被触摸层「更多」按钮复用，按全局序号判会假红）。
    $uiLayerArgs = @("--drive-new-game", "--expect-runtime-spawns", "--check-ui-layer-order", "--frames", "240")
    if ($isThinPack) { $uiLayerArgs += @("--remote", "thin", "--remote-root", $assetPackDir) }
    $uiLayerCase = Invoke-HarnessCase -ExtraArgs $uiLayerArgs -Label "ui-layer-order"
    if ($uiLayerCase.ExitCode -ne 0) { throw "UI 分层序用例失败（--drive-new-game --check-ui-layer-order, exit $($uiLayerCase.ExitCode)）" }
    $uiLayerOkLine = $uiLayerCase.Lines | Where-Object { $_ -match "wechat_minigame_harness_ok" } | Select-Object -First 1
    if (-not $uiLayerOkLine) { throw "UI 分层序用例未输出 wechat_minigame_harness_ok" }
    # overlap_pairs 是这条门的"有没有在量东西"：招式栏 7 格，少于 7 组重叠就是前置塌了
    # （布局改了/卡底没生成），宁可红也不要空过。
    if (-not ($uiLayerOkLine -match 'ui_layer_order="ok,card_ops=(\d+),icon_ops=(\d+),overlap_pairs=(\d+)')) {
        throw "UI 分层序用例没有自报重叠对数证据（缺少 ui_layer_order=「ok,card_ops=…,icon_ops=…,overlap_pairs=…」）：$uiLayerOkLine"
    }
    if ([int]$Matches[3] -lt 7 -or [int]$Matches[2] -lt 7) {
        throw ("UI 分层序用例只量到 {0} 组重叠、{1} 张图标（招式栏应有 7 格，图标与卡底都得够 7）" +
            "——前置不成立，这条门等于没跑") -f $Matches[3], $Matches[2]
    }
    Write-Host $uiLayerOkLine -ForegroundColor DarkGray

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
    Write-Host ("wechat_minigame_package_ok files={0} runtime=ok path_styles={1} asset_pack={2} {3} negatives={4} tag_polyfill_probe={6} eh_probe={7} host_globals=ok,locked render_selfcheck=ok play_path=ok,touch_stick=ok,save_persist=ok{5}" -f `
        $required.Count, ($pathStyles -join ','), $formName, $remoteField, (($negativeCases + @("image-src-frozen", "image-src-prototype", "image-callback-bridge", "boot-stall", "host-document-frozen")) -join ','), $formEvidence, $tagPolyfillVerdict, $ehProbeVerdict) -ForegroundColor Green
    if ($remoteOkLine) { Write-Host $remoteOkLine -ForegroundColor DarkGray }
} else {
    Write-Host ("wechat_minigame_package_ok files={0} runtime=skipped" -f $required.Count) -ForegroundColor Yellow
}
