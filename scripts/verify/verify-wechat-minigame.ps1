# 微信小游戏发布包验证（scripts/verify/verify-wechat-minigame.ps1）
#
# 两层验证：
#   1) 静态布局/配额：入口与配置齐全、pkg/ 与 pack/ 就位、不含 PC/WebGL 的入口文件、
#      主包与总体积满足微信小游戏限制（主包 ≤ 4MB、全部 ≤ 20MB）。
#   2) 运行时行为：交给 scripts/verify/wechat-minigame-harness.js —— 用 Node 伪造 wx API，
#      真实执行 game.js → wechat-adapter.js → web-runtime.js，跑若干帧并断言
#      pack 资源被读取、Lua 主循环在绘制、触摸/键盘事件被桥接。
#
# 用法：powershell -NoProfile -File scripts\verify\verify-wechat-minigame.ps1 `
#         -BundleRoot examples\game_jhyt\builds\wechat-minigame
[CmdletBinding()]
param(
    [string]$BundleRoot = "",
    [int]$Frames = 12,
    # 资源包（CDN 导出）目录：瘦首包（-AssetPack remote）必须给，
    # 否则包内 pack/ 只有骨架，运行时验证读不到重资源。
    [string]$AssetPackRoot = "",
    # 玩法驱动（微信侧没有 URL 旋钮，用 harness 的宿主驱动）：**B8 之后已失效** —— 它的判据读
    # `data-rustlua-world` / `move=`，而那两个读数的写者是已删除的旧 JS 世界（见下面 6.5 节的退休说明）。
    # 保留开关只为不动既有调用面。
    [switch]$DriveJhyt,
    # 要求这份产物必须带 GPU 内核（`package.ps1 -Gpu2d` 打的那份）：引擎画布探针必须走到
    # `failed` + 引擎原文 `probe: …`，报 `unavailable` 直接判失败（避免"我以为是 GPU 档"）。
    [switch]$ExpectGpuKernel,
    # **要求这一档真的验证"世界在推进 / 画面在出帧"**（B8 新增）。默认关：离线假宿主按设计不给
    # WebGL2（见 `scripts/verify/wechat-minigame-harness.js` 的 `engineHostNoGl`），而旧 JS 世界已删 ⇒
    # 这一档**没有帧所有者**是环境限制，不是产物回归。带 GL 的宿主（开发者工具/真机/带 GPU 的
    # CI 机）上跑本脚本时请加它：那时"没有帧所有者"从环境限制变成**失败**。
    [switch]$RequireRuntimeHost,
    # 只做静态检查（没有 node 时使用）。
    [switch]$SkipRuntime
)

$ErrorActionPreference = "Stop"
# 子进程（node harness / Lua 日志）输出是 UTF-8，PS 5.1 控制台默认按 ANSI 解码会显示成乱码。
try { [Console]::OutputEncoding = New-Object Text.UTF8Encoding($false) } catch { }
$workspaceRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
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
#   subpackage = game.json 声明了 root=pack 的分包 **且** 主包侧首屏清单 pack.json 存在；
#   thin       = pack/ 下没有资源目录（assets/scenes/scripts）却存在 remote-assets.json；
#   full       = 其余。
# 注意不能只靠"声明了分包"这一条：分包声明是发布脚本注入的，而 pack.json 是切分的凭据，
# 两者必须同时成立才是我方产出的分包形态（缺一个就说明产物是手工改过或半更新的）。
# 改名映射（产物根的 code-package.json）：逻辑路径 → 会随代码包上传的宿主文件名。
# 微信代码包只上传后缀在白名单内的文件，构建期把 `.lua`/`.toml`/`.md` 改名成 `*.txt`
# （见 scripts/lib/code-package-rules.js）；验收侧**只查表**，不重算规则，否则两处规则必然漂移。
# 必须解析完 $root 就立刻加载：下面的瘦首包分支（包内骨架与资源包逐字节比对）就要用它。
$subpackageRootName = "pack-sub"
# 微信内核独立根（package.ps1 的默认布局）；旧产物可能仍把内核放在 pack/。
$packWasmRootName = "pack-wasm"
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
$packAssetDirs = @("assets", "scenes", "scripts") | Where-Object { Test-Path -LiteralPath (Join-Path $root ("$subpackageRootName\" + $_)) }
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
            if ($item -and ([string]$item.root).TrimEnd('/') -eq $subpackageRootName) { $packDeclaredAsSubpackage = $true; break }
        }
    }
    catch { $packDeclaredAsSubpackage = $false }
}
$mainPackManifestPath = Join-Path $root "pack.json"
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
    ($mainPackEntries.Count -eq 0) -and (Test-Path -LiteralPath (Join-Path $root "$packWasmRootName\game_web_bg.wasm"))
# 三形态互斥的兜底断言：分包形态的资源全在包里，出现远端配置就说明产物是两种形态混出来的——
# 那时适配层会走 CDN 回落，"包内读到了"永远掩盖"分包根本没挂载上"。
if ($isSubpackagePack -and $hasRemoteConfig -and -not $isWasmInPackRemote) {
    throw "分包形态不得带 remote-assets.json（它属于 -AssetPack remote）：两种资源来源混在一个包里会让失败互相掩盖"
}
if ($isSubpackagePack -and $isThinPack -and -not $isWasmInPackRemote) {
    throw "产物同时具有分包与瘦首包的特征（pack/ 无资源目录 + remote-assets.json + 分包声明 + pack.json），无法判定形态"
}
if ($isWasmInPackRemote) {
    # 这一形态**同时**要打开两条轴：内核在分包（要等挂载）+ 重资源在远端（要带 --remote thin）。
    $isThinPack = $true
}
if ($packDeclaredAsSubpackage -and -not $hasMainPackManifest) {
    throw "game.json 声明了 pack 分包，但主包侧首屏清单 pack.json 不存在：首屏资源没有切到主包，真机上会先等分包下载"
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
            "（约定位置：与 launcher/ 同级的 assets/，形如 <发布根>/assets/pack-sub/**）")
    }
    $assetPackDir = (Resolve-Path -LiteralPath $AssetPackRoot).Path
    # 两侧必须一一对应：资源包和包内 manifest 不一致，远端校验会把文件全丢掉。
    $bundlePackFiles = (Get-ChildItem -LiteralPath (Join-Path $root $subpackageRootName) -Recurse -File -Force | Measure-Object).Count
    $assetPackFiles = (Get-ChildItem -LiteralPath (Join-Path $assetPackDir $subpackageRootName) -Recurse -File -Force | Measure-Object).Count
    $manifest = Join-Path $root "$subpackageRootName\manifest.txt"
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
        $inBundle = Join-Path $root ("$subpackageRootName\" + (Resolve-CodePackName $shared))
        $inPack = Join-Path $assetPackDir ("pack\" + $shared)
        if (-not (Test-Path -LiteralPath $inPack)) {
            throw ("资源包缺少骨架文件: $subpackageRootName\{0}（远端校验依赖它）" -f $shared)
        }
        $a = [IO.File]::ReadAllBytes($inBundle)
        $b = [IO.File]::ReadAllBytes($inPack)
        $same = ($a.Length -eq $b.Length)
        if ($same) {
            for ($i = 0; $i -lt $a.Length; $i++) { if ($a[$i] -ne $b[$i]) { $same = $false; break } }
        }
        if (-not $same) {
            $detail = if ($a.Length -eq $b.Length) { "{0} B（字节数相同但内容不同）" -f $a.Length } else { "{0} B vs {1} B" -f $a.Length, $b.Length }
            throw (("包内 $subpackageRootName\{0} 与资源包里的不是同一份：{1}。远端校验会用错误的清单，" -f $shared, $detail) +
                "真机上会把下载文件整批丢弃。请用同一次 -AssetPack remote 发布同时产出首包与资源包。")
        }
    }
    Write-Host ("资源包形态: 瘦首包（包内 $subpackageRootName/ {0} 个文件，资源包 {1} 个文件 / manifest {2} 条）" -f `
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
$skeletonSubdir = if ($isSubpackagePack -and -not $isWasmInPackRemote) { "pack" } else { $subpackageRootName }
# wasm 内核的位置同样随形态变：主包 4 MB 门装不下内核，所以内核入分包时它在 pack/ 里
# （经典分包形态与 remote + -WasmInSubpackage 都是），full / remote（内核在主包）仍在 pkg/。
# **两边都只许有一处**：pkg/ 里那份残留不会抛错，只会让主包又超回去（而 4 MB 超限的表现是
# 上传被拒，本机什么都看不到），所以这里除了"该有的在"还要验"不该的不在"。
$wasmCoreRelative = if ($isSubpackagePack) { "$packWasmRootName\game_web_bg.wasm" } else { "pkg\game_web_bg.wasm" }
$required = @(
    "game.js",
    "game.json",
    "project.config.json",
    "wechat-adapter.js",
    "bundle-roots.js",
    "pkg\game_web.js",
    "pkg\eh-probe.wasm",
    $wasmCoreRelative,
    ($skeletonSubdir + "\" + (Resolve-CodePackName "manifest.txt")),
    ($skeletonSubdir + "\" + (Resolve-CodePackName "project.toml")),
    "code-package.json"
)
if ($isSubpackagePack) {
    # 分包形态多两项：路由依据（清单）与两个分包入口（pack 资源根、pack-wasm 内核根）。
    $required += @("pack.json", "$subpackageRootName\game.js", "$packWasmRootName\game.js")
    if (Test-Path -LiteralPath (Join-Path $root "pkg\game_web_bg.wasm")) {
        throw "分包形态下 pkg\game_web_bg.wasm 仍在主包里：内核没有被搬进 $packWasmRootName/，主包体积会按旧公式超限（两份同名内核迟早漂移，这里拒绝而不是取其一）"
    }
}
foreach ($file in $required) {
    $full = Join-Path $root $file
    if (-not (Test-Path -LiteralPath $full)) { throw "微信小游戏发布包缺少: $file" }
    if ((Get-Item -LiteralPath $full).Length -le 0) { throw "微信小游戏发布包文件为空: $file" }
}
# 胶水固定在 pkg\game_web.js（`require('./pkg/game_web.js')` 同步加载）；launcher 根旧布局不许再出现。
if (Test-Path -LiteralPath (Join-Path $root "game_web.js")) {
    throw "产物里仍有 launcher\game_web.js：胶水应固定在 pkg\game_web.js（重跑 package.ps1）"
}
# EH 真机探针固定 pkg\eh-probe.wasm（required 里已列）；launcher 根旧布局不许再出现胶水。

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
# `game_web.js, 989:72` 的 ES2021 逻辑赋值 `??=`，和 `web-runtime.js, 1:105804` 的 ES2020
# 空值合并 `??`。工具里跑得通、开发者也看得见画面，只有点"上传"才炸。这里按上传器实测接受的
# 那一档（ES2019，口径与两次现场的对照见 scripts/verify/check-upload-ecma.mjs 头部）逐个 .js 复述一遍
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
        if (-not $detail) { $detail = "工具退出码 $ecmaExit 且 stdout 无输出（多半是 acorn 没解析到：跑 scripts/build/minify-toolchain.ps1 或设 `$env:ACORN，细节在它的 stderr 里）" }
        throw ("发布包里有微信上传器解析不了的语法（口径: ES2019；要改这一档得动 scripts/verify/check-upload-ecma.mjs 的参数并留实测证据）: " + $detail)
    }
    Write-Host (($ecmaOutput | Where-Object { $_ -match '^upload_ecma_ok' } | Select-Object -First 1)) -ForegroundColor DarkGray
}

# 代码包后缀白名单（真机只上传后缀在白名单内的文件，见 scripts/lib/code-package-rules.js）：
# 资源树里不得有"会被白名单拒掉"的文件——真机上它们根本不存在，运行时读到的是一串 permission denied。
# 判据不在这里重算：交给同一份规则模块（node 侧），这里只看结论。
$resourceDirs = @()
foreach ($name in @("pack", $subpackageRootName)) {
    $dir = Join-Path $root $name
    if (Test-Path -LiteralPath $dir) { $resourceDirs += $dir }
}
if ($resourceDirs.Count -gt 0) {
    if (-not $nodeForSyntax) {
        Write-Warning "==> 未找到 node，跳过代码包后缀白名单检查（真机可能因此读不到文件）"
    }
    else {
        $whitelistTool = Join-Path $PSScriptRoot "..\build\first-screen-pack.js"
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
# 字体在包内的位置同样随形态变：分包形态下字体是首屏资源，已被切到 pack/assets/fonts。
$fontSearchRoots = @((Join-Path $root ($skeletonSubdir + "\assets\fonts")))
if ($isThinPack -and $assetPackDir) { $fontSearchRoots += (Join-Path $assetPackDir "pack\assets\fonts") }
$packedFonts = @()
foreach ($fontRoot in $fontSearchRoots) {
    $packedFonts += Get-ChildItem -LiteralPath $fontRoot -File -Filter *.ttf -ErrorAction SilentlyContinue
}
if (-not $packedFonts) { throw "找不到子集字体（$subpackageRootName/assets/fonts 或 pack/assets/fonts，assetc 未产出？瘦首包则该在资源包里）" }
if ($isThinPack) {
    Write-Host ("字体随资源包分发: {0}（{1:N0} KB，首启下载后注册）" -f $packedFonts[0].Name, ($packedFonts[0].Length / 1KB)) -ForegroundColor DarkGray
}

$gameJson = Get-Content -LiteralPath (Join-Path $root "game.json") -Raw -Encoding UTF8 | ConvertFrom-Json
# 多根布局（-MultiSub / pack-wasm 独立内核根）：launcher 根有 bundle_plan.json，或存在
# pack-wasm/game_web_bg.wasm。game.json 可以有多条 subpackages。
$multiRoot = (Test-Path -LiteralPath (Join-Path $root "bundle_plan.json")) -or `
    (Test-Path -LiteralPath (Join-Path $root "pack-wasm\game_web_bg.wasm"))
# 分包声明按形态**三分支**断言。这里两个方向都必须显式断言：只留"没有声明就没事"的话，
# 分包形态悄悄退化成 full 形态（首屏又开始等分包）不会有任何提示。
# 同样只查一种拼写（PS 属性访问不区分大小写，两种拼法会解析到同一个属性）。
$subpackageList = @($gameJson.subpackages)
$subpackageNames = @($subpackageList | Where-Object { $_ } | ForEach-Object { $_.name })
$subpackageRoots = @($subpackageList | Where-Object { $_ } | ForEach-Object { ([string]$_.root).TrimEnd('/') })
if ($isSubpackagePack) {
    if ($multiRoot) {
        if ($subpackageList.Count -lt 1) { throw "多根布局的 game.json 没有 subpackages" }
        foreach ($subRoot in $subpackageRoots) {
            if ([string]::IsNullOrWhiteSpace($subRoot)) { throw "多根布局的 subpackages 有空 root" }
            $entry = Join-Path $root "$subRoot\game.js"
            if (-not (Test-Path -LiteralPath $entry)) { throw "多根布局的分包根 $subRoot 缺少 game.js 入口" }
            if ((Get-Item -LiteralPath $entry).Length -le 0) { throw "多根布局的 $subRoot/game.js 是空文件" }
        }
    }
    else {
        if ($subpackageList.Count -ne 1) {
            throw ("分包形态期望**有且仅有 1 个**分包，实际 {0} 个：{1}" -f $subpackageList.Count, ($subpackageNames -join ','))
        }
        if ($subpackageRoots[0] -ne $subpackageRootName) { throw "分包 root 规范化后是 '$($subpackageRoots[0])'（期望 $subpackageRootName）" }
        if ($subpackageNames[0] -ne $subpackageRootName) { throw "分包 name 是 '$($subpackageNames[0])'（期望 $subpackageRootName）" }
        $subpackageEntry = Join-Path $root "$subpackageRootName\game.js"
        if (-not (Test-Path -LiteralPath $subpackageEntry)) {
            throw "分包形态缺少分包入口 $subpackageRootName\game.js（微信要求 subpackages[0].root 目录下有 game.js，缺了开发者工具拒绝打开工程）"
        }
        if ((Get-Item -LiteralPath $subpackageEntry).Length -le 0) { throw "分包入口 $subpackageRootName\game.js 是空文件" }
    }
}
elseif ($subpackageNames -contains $subpackageRootName) {
    throw "game.json 把 $subpackageRootName 声明为分包，但这一形态（full/thin）不该有该声明：非分包形态声明分包会引入分包入口要求"
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
$packSize = Get-DirSize -Path (Join-Path $root $subpackageRootName)
# 多根：主包 = launcher − 全部 pack*/（不能只减 pack/，否则 pack-<name>/ 会被错算进主包）。
$subpackageSize = if ($isSubpackagePack) {
    if ($multiRoot) {
        $sum = 0
        foreach ($subRoot in $subpackageRoots) {
            if ([string]::IsNullOrWhiteSpace($subRoot)) { continue }
            $sum += (Get-DirSize -Path (Join-Path $root $subRoot))
        }
        $sum
    }
    else { $packSize }
}
else { 0 }
$mainSize = $totalSize - $subpackageSize
# 主包超限的出路按形态给，**方向必须写对**：主包 = launcher − pack/，而 mainpack/（首屏资源）
# 是**算进主包**的，所以"把更多资源列进首屏清单"是加大主包的那一手——曾经就这么写反过，
# 超限的人照着做只会更超（`scripts/build/package.ps1` 的 `Assert-WeChatPackageVolume` 已改正，
# 这里曾经是它唯一的反口径副本，两处现在同一个说法）。
$mainHint = if ($isSubpackagePack) {
    "**收窄**首屏清单（--include 前缀去掉非首屏目录，被排除的资源留在 $subpackageRootName/ 分包里：主包变小、分包变大；列进清单是反方向），或改用 -AssetPack remote"
}
elseif ($isThinPack) { "$subpackageRootName/ 骨架已经在 512KB 白名单内，若仍超限说明骨架白名单该更新了" }
else { "改用 -AssetPack subpackage（主包 + 单分包）或 -AssetPack remote 把重资源移出包内" }
if ($mainSize -gt $MainPackageLimit) {
    throw ("主包体积超限: {0:N0} KB > {1:N0} KB。出路：{2}" -f ($mainSize / 1KB), ($MainPackageLimit / 1KB), $mainHint)
}
# 单个普通分包：官方 2023-10 起不限制大小（主包 ≤ 4M、单个普通分包不限、总量 ≤ 20M/30M，
# 见 docs/52 与 scripts/build/package.ps1 的体积门）。因此不再用主包 4MB 门卡分包，
# 由下方总量门（20MB）兜底。
if ($isSubpackagePack -and $subpackageSize -gt $TotalLimit) {
    throw ("单个分包体积超限: {0:N0} KB > {1:N0} KB（$subpackageRootName/ 是 subpackages[0].root，目录下所有文件都计入）。出路：把更多资源列进首屏清单，或拆第二个分包" -f `
        ($subpackageSize / 1KB), ($TotalLimit / 1KB))
}
# 总量门（官方：主包 + 全部分包之和 ≤ 20MB；开通虚拟支付可到 30MB，按无内购口径保守取 20MB）。
# 主包 4MB 门在上面独立生效；单个普通分包不限制大小（见上一条注释），因此总量门是分包的兜底。
if ($totalSize -gt $TotalLimit) {
    throw ("发布包总体积超限: {0:N0} KB > {1:N0} KB（主包 + 全部分包之和）" -f ($totalSize / 1KB), ($TotalLimit / 1KB))
}

$formName = if ($isSubpackagePack) { "subpackage" } elseif ($isThinPack) { "thin" } else { "full" }
if ($multiRoot) {
    # 多根额外对账：以计划表为准，每个根有目录/入口/声明的容器（缺件 exit 1）。
    $mountCheck = Join-Path $PSScriptRoot "..\build\check-bundle-mounts.js"
    if (-not (Test-Path -LiteralPath $mountCheck)) { throw "缺少多根一致性检查: $mountCheck" }
    $nodeForMount = Get-Command node -ErrorAction SilentlyContinue
    if (-not $nodeForMount) { throw "多根一致性检查需要 node" }
    $savedEapMount = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $mountOutput = & $nodeForMount.Source $mountCheck --launcher $root 2>&1
        $mountExit = $LASTEXITCODE
    }
    finally { $ErrorActionPreference = $savedEapMount }
    if ($mountExit -ne 0) {
        throw ("多根产物与 bundle_plan.json 不一致: " + (($mountOutput | ForEach-Object { "$_" }) -join ' '))
    }
    Write-Host (($mountOutput | Where-Object { $_ -match '^bundle_mounts_ok' } | Select-Object -First 1)) -ForegroundColor DarkGray
    # 计划表的 boot 元数据 + 真实目录/首屏清单闭包（trace 由 package.ps1 -BootTrace 采）。
    $bootCheck = Join-Path $PSScriptRoot "..\build\check-boot-closure.js"
    if (-not (Test-Path -LiteralPath $bootCheck)) { throw "缺少 boot 闭包检查: $bootCheck" }
    try {
        $bootOutput = & $nodeForMount.Source $bootCheck --launcher $root 2>&1
        $bootExit = $LASTEXITCODE
    }
    finally { $ErrorActionPreference = $savedEapMount }
    if ($bootExit -ne 0) {
        throw ("boot 闭包检查失败: " + (($bootOutput | ForEach-Object { "$_" }) -join ' '))
    }
    Write-Host (($bootOutput | Where-Object { $_ -match '^boot_closure_ok' } | Select-Object -First 1)) -ForegroundColor DarkGray
}
Write-Host ("wechat_minigame_static_ok main={0:N0}KB subpackage={1:N0}KB total={2:N0}KB subpackages={3} asset_pack={4} upload_rules=ok main_of_limit={5:P1} subpackage_of_limit={6:P1} total_of_limit={7:P1}" -f `
    ($mainSize / 1KB), ($subpackageSize / 1KB), ($totalSize / 1KB), $(if ($subpackageNames.Count) { $subpackageNames -join ',' } else { 'none' }), $formName, `
    ($mainSize / $MainPackageLimit), $(if ($subpackageSize -gt 0) { $subpackageSize / $TotalLimit } else { 0 }), ($totalSize / $TotalLimit)) -ForegroundColor Green
if ($isSubpackagePack) {
    Write-Host ("主包 {0:N0}KB（含首屏 mainpack/ {1:N0}KB）/ 上限 4,096KB（{2:P1}）；分包 $subpackageRootName/ {3:N0}KB / 总量上限 20,480KB（{4:P1}，单分包官方不限大小）" -f `
        ($mainSize / 1KB), ((Get-DirSize -Path (Join-Path $root "pack")) / 1KB), ($mainSize / $MainPackageLimit), `
        ($subpackageSize / 1KB), ($subpackageSize / $TotalLimit)) -ForegroundColor DarkGray
}
elseif ($isThinPack) {
    Write-Host ("包内骨架 {0:N0}KB / 上限 4,096KB（{1:P1}）；资源包 {2:N0}KB 在 CDN，不占配额" -f `
        ($packSize / 1KB), ($packSize / $MainPackageLimit), ((Get-DirSize -Path (Join-Path $assetPackDir $subpackageRootName)) / 1KB)) -ForegroundColor DarkGray
}
else {
    Write-Host ("$subpackageRootName/ 计入主包: {0:N0}KB / 上限 4,096KB（{1:P1}）" -f ($packSize / 1KB), ($packSize / $MainPackageLimit)) -ForegroundColor DarkGray
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
    # ---- B8：这一档到底有没有"帧所有者"（决定后面哪些用例还有主体）------------------------
    # 判据取自 harness 的自报字段（不是推断）：`env_limited=no-webgl2-in-fake-host` 表示
    # "引擎宿主因为假宿主不给 GL 而起不来、旧 JS 世界又已删除 ⇒ 没有任何人推进世界/出帧"。
    # 这一档里"世界/帧/画面/UI 载荷"那些用例**没有主体**，按仓库纪律如实记为环境限制
    # （"若环境无法提供，明确记为未验证，不以 Node 模拟代替"——tasks/plan/M73-00 的验收方式），
    # 而不是把它们判红（那会让真正的回归淹没在环境噪声里），也不是静默跳过（下面逐处打印原因，
    # 末尾那行 `runtime=` 也会如实写成 env-limited）。
    $runtimeEnvLimited = [bool]($case.Lines | Where-Object { $_ -match 'env_limited=no-webgl2-in-fake-host' })
    if ($runtimeEnvLimited -and $RequireRuntimeHost) {
        throw ("本次要求验证'世界在推进/画面在出帧'（-RequireRuntimeHost），但这一档没有帧所有者：" +
            "离线假宿主不给 WebGL2（harness 自报 env_limited=no-webgl2-in-fake-host）。" +
            "请在能给 GL 的宿主上跑（微信开发者工具/真机），或去掉 -RequireRuntimeHost 接受环境限制口径。")
    }
    if ($runtimeEnvLimited) {
        Write-Host "[环境限制] 本档没有帧所有者：离线假宿主不给 WebGL2（harness 自报 env_limited=no-webgl2-in-fake-host），" -ForegroundColor Yellow
        Write-Host "           而 B8 之后旧的 JS 世界已删除 ⇒ 世界/帧/画面/UI 载荷这几组**没有主体**，本次如实记为未验证。" -ForegroundColor Yellow
        Write-Host "           仍然验证的部分：静态布局与配额、代码包/分包路径方言、资源路由与上传模型、字体、wasm 通道、L3 行为契约（Lua 经引擎执行）、单一 tick 所有者。" -ForegroundColor Yellow
        Write-Host "           画面/输入/玩法的机器证据在别处：浏览器门（同一份 game_web 内核，-Gpu2d -SimRoute script -InputScript）与微信开发者工具/真机探针（任务#27 的记录）。" -ForegroundColor Yellow
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
    #      **默认形态已经翻转**（P1 第二片）：Rust 宿主的原子模式是**生产默认**，不再需要开关。
    #      断言随之从"不带开关必须 off"改成"不带开关必须是**接管形态**，且结论永远具名"：
    #        · `engine_host_source` 必须含 `visible-takeover`——证明原子模式真的开着、而且绑的是主画布；
    #        · 结论按产物档位/宿主能力落进具名结果：默认档内核 ⇒ `unavailable` + `kernel-without-gpu2d`；
    #          GPU 档内核 + 离线假宿主（不给 GL）⇒ `failed` + 以 `host: ` 开头的引擎原文；
    #          真机/带 GL 的宿主 ⇒ `running`（这才是这条路的目标态）。
    #        · 失败时**理由不得为空**：默认翻过去之后"起不来"就是可见的黑屏 + 具名读数，
    #          空理由会把黑屏变成查不出来的现象（用户裁：默认开、不回退）。
    #      旧形态（宿主关）不再是默认，由下面 1a4b 的显式关用例继续守着。
    $hostOffLine = $case.Lines | Where-Object { $_ -match "engine_host=" } | Select-Object -First 1
    $defaultHostStatus = if ($hostOffLine -match '(?:^|\s)engine_host=(\S+)') { $matches[1] } else { "缺失" }
    $defaultHostSource = if ($hostOffLine -match '(?:^|\s)engine_host_source=(\S+)') { $matches[1] } else { "缺失" }
    $defaultHostReason = if ($hostOffLine -match 'engine_host_reason="([^"]*)"') { $matches[1] } else { "" }
    if ($defaultHostSource -notmatch 'visible-takeover') {
        throw ("生产默认必须是 Rust 宿主的接管形态（engine_host_source 含 visible-takeover），实际 source={0}：{1}" -f $defaultHostSource, $hostOffLine)
    }
    switch ($defaultHostStatus) {
        "running" { }
        "unavailable" {
            if ($defaultHostReason -notmatch '^kernel-without-gpu2d$') {
                throw ("默认形态下 unavailable 必须带具名理由 kernel-without-gpu2d，实际：{0}" -f $defaultHostReason)
            }
            if ($ExpectGpuKernel) {
                throw "本产物没编 GPU 内核（unavailable），但调用方要求了 -ExpectGpuKernel：请用 package.ps1 -Gpu2d 重新打包"
            }
        }
        "failed" {
            if ($defaultHostReason -notmatch '^host: ') {
                throw ("默认形态下失败必须带引擎原文（以 'host: ' 开头），实际：{0}" -f $defaultHostReason)
            }
        }
        default { throw ("默认形态必须落在 running/unavailable/failed 之一，实际 status={0}" -f $defaultHostStatus) }
    }
    Write-Host ("引擎宿主（生产默认）：status={0} source={1} reason={2}" -f $defaultHostStatus, $defaultHostSource, $defaultHostReason) -ForegroundColor DarkGray

    # **逃生门 `--rust-scene-host-off` 的守门用例已随 B8 退休**：那条门守的是"显式关掉原子模式后
    # 仍能逐字回到旧形态（engine_host=off + reason=switch-off）"。旧世界删除后，关掉原子模式**没有
    # 可回退的形态**了 —— 这条门连同逃生门一起退休（适配层那一侧也已把该全局读掉）。
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

    # **WP7-94 的"接管 → 交还"往返用例已随 B8 退休。** 它验的是"第 15 帧把
    # `__RUSTLUA_ENGINE_HOST_OWNS_TICK` 置回 false ⇒ 世界交回**运行时**推进（`timing … tick=runtime`）"。
    # 那个标记的**唯一读者是旧 JS 运行时**（`web-runtime.js` 的 `hostOwnsFixedTick()`，已删除），
    # `timing` 读数也随它一起消失 ⇒ 用例的"交还"在今天的产物上不再有任何可观测后果（harness 里
    # `--engine-host-off-at` 只剩打一行留痕）。今天的等价问题——"谁在推进世界、有没有两个所有者"——
    # 由下面的单一 tick 所有者断言（`engine_host_tick_conflict`/`input_applied`/`tick`）与
    # 设备侧的 `engine_host_tick=host` 读数负责。
    # （历史读数与判据原文见 `tasks/plan/M73-00` 的 WP7-90…WP7-97。）


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

    # 1a6) 「一个世界只能有一个 tick」（WP7-57；**B8 改判**）。
    #      原断言有两条，其中"世界必须有人推进"那一条**今天没有主体**：
    #        · `runtime_run_steps > 0` / `timing … tick=runtime` / `runtime_input_frames > 0`
    #          的写者都是**已删除的旧 JS 世界**（`web-runtime.js`）⇒ 在今天的产物上它们恒为 0/off；
    #        · 今天唯一的推进者是**引擎宿主**，而它在离线假宿主里必然起不来（不给 GL，
    #          见上面 `$runtimeEnvLimited`）。所以"离线必须有人推进世界"变成了不可满足的要求。
    #      改判后剩下的不变量仍然是"恰好一个所有者"，且与产物档位无关：
    #        · 两个所有者同时推进 ⇒ `tick_conflict=yes`：不可达，照旧红；
    #        · 宿主没起来 ⇒ 谁都不许有计数（`host_input_frames=0`、`runtime_*=0`）；
    #        · 宿主起来（真机/带 GL 的宿主）⇒ **它**必须真的在跑（`host_input_frames>0`）。
    #      "世界在推进/画面在出帧"这条正面证据归**设备侧**（开发者工具/真机）与浏览器门；
    #      要在这条门上强制它，请加 `-RequireRuntimeHost`（那时没有帧所有者直接判红）。
    foreach ($probe in @([pscustomobject]@{ Name = '生产口径'; Line = $hostOffLine },
                         [pscustomobject]@{ Name = '--engine-host-tick'; Line = $hostTickOk })) {
        if ($probe.Line -match 'engine_host_tick_conflict=yes') {
            throw ("{0}：运行时与宿主同时推进了世界（tick_conflict=yes）——这必须是不可达状态：{1}" -f $probe.Name, $probe.Line)
        }
        # 旧运行时的两个计数器今天应当**恒为 0**（写者已删）。它们不为 0 只有两种可能：
        # 有人把第二份实现接回来了却没接进宿主协议，或读数被伪造 —— 两种都该红。
        $runSteps = if ($probe.Line -match 'engine_host_runtime_run_steps=(\d+)') { [int]$matches[1] } else { -1 }
        $inputFrames = if ($probe.Line -match 'engine_host_runtime_input_frames=(\d+)') { [int]$matches[1] } else { -1 }
        if ($runSteps -lt 0 -or $inputFrames -lt 0) {
            throw ("{0}：自报里缺 engine_host_runtime_run_steps / _input_frames 读数（无法判断有没有第二个推进者）" -f $probe.Name)
        }
        if ($runSteps -ne 0 -or $inputFrames -ne 0) {
            throw ("{0}：旧运行时的推进/注入计数应为 0（它的写者已随 B8 删除），实际 run_steps={1} input_frames={2} —— 出现了来源不明的第二个所有者" -f `
                $probe.Name, $runSteps, $inputFrames)
        }
        # **已退休**：原来这里要求 `timing` 带 `sim=` 且以 `tick=runtime` 收尾——那是旧 JS 世界
        # 写的常驻读数（`data-rustlua-timing`），已随它删除。今天"谁在推进"的证据是适配层自报的
        # `engine_host_tick` / `engine_host_frames`（下面照旧断言），设备侧另有 `data-rustlua-gpu`。
        # 1a7) 输入注入的所有者（WP7-58；**B8 改判**）：注入必须来自**恰好一处**。
        #      旧运行时的注入计数（`engine_host_runtime_input_frames`）已在上面的 1a6 里断成 0；
        #      今天唯一的注入生产者是宿主自己（`engine_host_input_*` + 每帧 flush），
        #      所以"宿主起来时它必须在跑、没起来时它必须是 0"就是这条不变量的全部内容。
        $probeHostRunning = $probe.Line -match '(?:^|\s)engine_host=running'
        # B4 交接后新增的一条：**新**通道必须真的在跑。它恒 0 与上面那条合起来就是"两边都没动"，
        # 而那正是"按键没反应"的形态——所以两条必须一起断言，只看一条会放过它。
        # （契约变化：交接前这里断言"运行时在注入 >0"，前提是注入归旧 JS 运行时；B4 把所有权
        #  交给宿主自己的生产者 `engine_host_input_*` + 每帧 `engine_host_input_flush`。）
        $hostInputFrames = if ($probe.Line -match 'engine_host_host_input_frames=(\d+)') { [int]$matches[1] } else { -1 }
        # **只在宿主真的起来时**断言这一条：离线假宿主不给 GL ⇒ 宿主到不了 `running` ⇒ 它的输入泵
        # 也不可能跑（`host_input_frames` 恒 0 是**环境**的结果，不是接线错）。这条正面证据在浏览器
        # 那条门上已有同一个量：`scene.input.seen_frames`（§9-A，实测 120 = 剧本窗口宽度）。
        # 宿主在跑时**新**通道必须也真的在跑：它恒 0 与"旧通道停用"合起来才是"两边都没动"。
        if ($probeHostRunning -and $hostInputFrames -le 0) {
            throw ("{0}：宿主在跑但它的输入生产者一帧都没跑（否则整局对按键没反应），实际 engine_host_host_input_frames={1}" -f $probe.Name, $hostInputFrames)
        }
        $probeHostStatus = if ($probe.Line -match '(?:^|\s)engine_host=(\S+)') { $matches[1] } else { "缺失" }
        if ($probeHostRunning -and $inputFrames -ne 0) {
            throw ("{0}：宿主在跑 ⇒ 旧运行时的注入计数必须保持 0（它的写者已删），实际 engine_host_runtime_input_frames={1}" -f $probe.Name, $inputFrames)
        }
        if ($probeHostStatus -ne "running" -and $hostInputFrames -gt 0) {
            throw ("{0}：宿主没起来却有输入泵帧数（读数可疑），status={1} host_input_frames={2}" -f $probe.Name, $probeHostStatus, $hostInputFrames)
        }
        $hostApplied = if ($probe.Line -match 'engine_host_input_applied=(\d+)') { [int]$matches[1] } else { -1 }
        if ($hostApplied -ne 0) {
            throw ("{0}：宿主不得注入输入（注入的唯一所有者是运行时），实际 engine_host_input_applied={1}" -f $probe.Name, $hostApplied)
        }
        # **1a8–1a11 整组已随 B8 退休**，理由一条就够：它们的被测物是**旧 JS 世界的 Lua 导出协议**
        # （`data-rustlua-export` 的 `pulls=`/`state=`/`tlines=`/`exports=`/`frame=`）——
        # 那些读数由 `web-runtime.js` 的补丁层写，文件已删 ⇒ 今天恒为 `none`。
        # 退休的几条断言各自守的关切与今天的归宿：
        #   · 1a8「注入次数 == 推进次数」（旧通道同条件）：两个计数器都已不存在，等式无意义；
        #     今天的对位件是宿主侧的 `host_input_frames`（上面 1a7）+ 浏览器门的
        #     `scene.input.seen_frames`。
        #   · 1a9 隐藏文本集随状态载荷到达（`pulls < frames`）：那条批量协议随补丁层一起消失；
        #     Rust 侧的对应物是 `rust_scene.rs` / `SceneMesh` 的文本几何（WP5 各片，
        #     由浏览器门的 `vector=`/`prefab_text=` 读数守）。
        #   · 1a10 `T` 行按变化发（`0 < tlines < 帧数×实体数`）：同上，属补丁层的状态镜像协议。
        #   · 1a11 状态回写唯一所有者（`frame ≤ frames`）：同上。
        # 历史读数与逐条判据原文见 `tasks/plan/M73-00` 的 WP7-59…WP7-61 各节（那些数字是
        # **旧形态**的基线，不要在今天的产物上重读）。
    }
    # 单一 tick 所有者的证据行（B8 改判后的口径：旧运行时计数恒 0、宿主侧另有一套）。
    Write-Host ("单一 tick 所有者：生产 tick={0} conflict={1} host_input_frames={2} runtime_run_steps={3}" -f `
        ([regex]::Match($hostOffLine, 'engine_host_tick=(\S+)').Groups[1].Value), `
        ([regex]::Match($hostOffLine, 'engine_host_tick_conflict=(\S+)').Groups[1].Value), `
        ([regex]::Match($hostOffLine, 'engine_host_host_input_frames=(\d+)').Groups[1].Value), `
        ([regex]::Match($hostOffLine, 'engine_host_runtime_run_steps=(\d+)').Groups[1].Value)) -ForegroundColor DarkGray
    Write-Host ("旧运行时通道：runtime_run_steps={0} runtime_input_frames={1}（应为 0：写者已随 B8 删除）" -f `
        ([regex]::Match($hostOffLine, 'engine_host_runtime_run_steps=(\d+)').Groups[1].Value), `
        ([regex]::Match($hostOffLine, 'engine_host_runtime_input_frames=(\d+)').Groups[1].Value)) -ForegroundColor DarkGray

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

    # EH 真机探针（同一次 --wasm-tag-polyfill-probe 驱动）：产物保留本地 tag 段 + legacy
    # try/catch，探针经 WXWebAssembly.compile('pkg/eh-probe.wasm') 问设备引擎认不认 EH。
    # 本机 V8 accepted 只是正对照；exec 必须返回 7，挡住"不含 EH 的桩"。
    $ehProbeExecLine = $tagProbeCase.Lines | Where-Object { $_ -match "eh_probe_exec=" } | Select-Object -First 1
    if (-not $ehProbeExecLine) {
        throw "EH 探针未输出 eh_probe_exec= 读数（随包探针没被驱动，这条守卫形同虚设）"
    }
    if (-not ($ehProbeExecLine -match "eh_probe_exec=7")) {
        throw ("EH 探针的 exec 正对照没通过（f() 不返回 7）：{0}" -f $ehProbeExecLine)
    }
    Write-Host $ehProbeExecLine -ForegroundColor DarkGray
    $ehProbeOkLine = $tagProbeCase.Lines | Where-Object { $_ -match "eh_probe=" -and $_ -notmatch "eh_probe_exec=" } | Select-Object -First 1
    if (-not $ehProbeOkLine) {
        throw "EH 探针未输出 eh_probe= 读数（探针编译通道没走到，这条守卫形同虚设）"
    }
    if (-not ($ehProbeOkLine -match "eh_probe=accepted")) {
        throw ("EH 真机探针的正对照没通过（本机 compile 不是 accepted）：{0}" -f $ehProbeOkLine)
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
        # **B8 改判（这一段的证据字段随之变）**：delay 的"N 帧"从 `wx.loadSubpackage` 被调用那一刻起算
        # （原来从 boot Promise 建好那刻起算，适配层前面的活一多就退化成立即挂载、进度条没有窗口可画）；
        # 而 `--drive-move` / `--expect-runtime-spawns` 的位移读数写者（旧 JS 世界）已删除 ⇒ 离线这一档
        # 它们如实自报 `move="env-limited:no-webgl2"`，**不再**作为这些用例的证据字段（用例的主体是
        # 分包挂载/路由/进度条，不是玩法）。玩法证据归浏览器门与微信开发者工具/真机（任务#27 的记录）。
        $subpackageCases = @(
            @{ Label = "subpackage-ok"; Args = ($subRemoteArgs + @("--subpackage", "ok")); Token = "wechat_minigame_harness_ok"; Evidence = 'subpackage=pack-sub' },
            @{ Label = "subpackage-delay"; Args = ($subRemoteArgs + $subFrames + @("--subpackage", "delay", "--subpackage-delay-frames", "6", "--drive-new-game", "--expect-runtime-spawns")); Token = "wechat_minigame_harness_ok"; Evidence = 'loading_bar=ok' },
            @{ Label = "subpackage-delay-move"; Args = ($subRemoteArgs + $subFrames + @("--subpackage", "delay", "--subpackage-delay-frames", "6", "--drive-new-game", "--expect-runtime-spawns", "--drive-move", "20")); Token = "wechat_minigame_harness_ok"; Evidence = 'loading_bar=ok' },
            @{ Label = "subpackage-never"; Args = ($subRemoteArgs + @("--subpackage", "never", "--expect-pack-failure")); Token = "wechat_minigame_harness_ok"; Evidence = 'pack_state=failed' },
            @{ Label = "subpackage-fail-once"; Args = ($subRemoteArgs + $subFrames + @("--subpackage", "fail-once", "--drive-new-game", "--expect-runtime-spawns")); Token = "wechat_minigame_harness_ok"; Evidence = 'subpackage_loads=' },
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
                if (-not ($okLine -match 'subpackage=pack-sub')) {
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
            elseif (-not $multiRoot -and -not ($okLine -match 'denied_before_boot=0')) {
                throw ("分包形态用例出现「启动期间读不到 pack/」——首屏清单漏列了首屏资源：{0}" -f $okLine)
            }
            Write-Host $okLine -ForegroundColor DarkGray
        }
    }

    # 2) 远端回落：包内读不到 pack/ 时，素材必须真的从服务下载、按清单校验、落地缓存并复用。
    #    harness 会自己起一个本地静态服务（复用 scripts/dev/pack-server.js），因此这里不依赖外部进程。
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

    # **6.5) 玩法驱动整块已随 B8 退休**（原 `if ($DriveJhyt) { … }`：`--drive-new-game --drive-move`
    #   + 跑两遍比"确定性"）。它的判据全部读**旧 JS 世界**写的读数——`data-rustlua-world` 的
    #   `move="keys=… d=dx,dy"`、`export_readout` 的 `state=/tlines=/ui=`、`data-rustlua-timing`
    #   的 `pulls=`；那些写者随 `web-runtime.js` 一起删除 ⇒ 今天恒 `world=off move=not-driven`。
    #   等价证据在别处，别在这里补一个读不到数的门：
    #     · 引擎侧玩法/位移：浏览器门 `-Gpu2d -SimRoute script -InputScript … -WorldExpr/-ExpectWorld`
    #       （同一份 `game_web` 内核 + `RustScene::refresh_world`，见 tasks/plan/M73-00 的 WP3 各片）；
    #     · 微信真机侧重合：开发者工具探针（任务#21/#22/#27 的 `walk_cycle`/`v27_cycle` 读数）。
    #   `-DriveJhyt` 开关保留只为不动既有调用面（带它跑也不会再有这一块）。

    # 7) 渲染自证：小游戏宿主没有 DOM 控制台，"流程正常但屏幕全黑" 必须能自证。
    #    --host-canvas offscreen：上屏画布挂在全局、wx.createCanvas() 返回的是另一个（离屏）
    #      对象（引导代码先调过一次就会这样）→ 适配层必须挑上屏画布，绘制只能落在它上面；
    #    --debug-overlay：覆盖层 + 像素回读（画 #ff00ff 再 getImageData 读回来）必须命中；
    #    --remote down --expect-image-band：重资源拿不到时屏幕必须出现红色警示条。
    # **三条"旧世界在屏行为"用例已随 B8 退休**（原 `$legacyWorldArgs` + canvas-provenance /
    # render-overlay / render-overlay-no-frames / image-failure-band）：它们断言的是"旧 JS 世界在它
    # 自己形态下把画面上到主画布、像素回读命中、零帧也有图案"。旧世界删除后那个形态不存在了 ——
    # 默认形态由 Rust 宿主把 2D 改道到离屏画布（主画布留给 webgl2），这些断言的前提随之消失。
    # 它们守的关切没有丢：像素回读/自证图案的等价断言由**默认形态**那几条用例与浏览器门覆盖。

    # **8) 玩家玩法路径整块已随 B8 退休**（原 `--drive-new-game` 的四条用例：
    #   `play-path-move` / `play-path-touchstick` / `save-persist` / `ui-layer-order`）。
    #   它们的前提是"世界由旧 JS 世界推进、UI 由它的 `drawUi` 画"——`--drive-new-game` 要读
    #   `data-rustlua-world` 里的主角坐标判位移，`--check-ui-layer-order` 读假 2D ctx 记下的
    #   `drawImage` 先后（那是 `drawUi` 的循环），`--check-save-persist` 也要游戏先真的跑起来。
    #   这一整套写者已随 B8 删除，离线更不会有帧所有者 ⇒ 判据没有主体。
    #   关切去哪了：画面/UI 分层序由浏览器门的引擎侧读数（`draws=`/`vector=`/`prefab_text=`）与
    #   原生侧 `jhyt_m70` 的 `submission_order` 守；存档由引擎侧 `rustlua-gameplay::save` 的用例守
    #   （`docs/51` 的口径已按"只剩引擎一种格式"改写）；微信侧输入/玩法要真机或开发者工具复跑。

    # 远端配置的存在性要自洽：配了必须有、没配必须没有，且内容可解析。
    if ($hasRemoteConfig) {
        $remoteConfig = Get-Content -LiteralPath $remoteConfigPath -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($remoteConfig.packBaseUrl -notmatch '^https?://') { throw "remote-assets.json 的 packBaseUrl 不是 http(s) 地址" }
        if ($remoteConfig.packBaseUrl -notmatch '/$') { throw "remote-assets.json 的 packBaseUrl 必须以 / 结尾" }
        Write-Host ("远端回落配置: {0}（包内仍是首选；本地服务用 scripts/dev/serve-pack.ps1 起）" -f $remoteConfig.packBaseUrl) -ForegroundColor DarkGray
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
    # 头一个字段（`runtime=`）必须**如实**说这一次到底验到了什么：
    #   · env-limited ⇒ 没有帧所有者（离线假宿主不给 GL + 旧 JS 世界已删），世界/帧/画面/UI 载荷
    #     那几组**没有主体**（本轮已退休，见上面 1a6/6.5/8 三处说明）⇒ 不能写 ok；
    #   · host ⇒ 宿主真的在跑（带 GL 的宿主 / -RequireRuntimeHost 档），那时帧/画面对应的读数才有主体。
    # 画面类字段（play_path/touch_stick/save_persist）**不再出现**：那几条用例的被测物已随 B8 删除，
    # 印出来只会让人以为验过。
    $runtimeField = if ($runtimeEnvLimited) { "env-limited:no-webgl2" } else { "host-driven" }
    Write-Host ("wechat_minigame_package_ok files={0} runtime={1} path_styles={2} asset_pack={3} {4} negatives={5} tag_polyfill_probe={6} eh_probe={7} host_globals=ok,locked l3_api_contract={8}{9}" -f `
        $required.Count, $runtimeField, ($pathStyles -join ','), $formName, $remoteField, (($negativeCases + @("image-src-frozen", "image-src-prototype", "image-callback-bridge", "boot-stall", "host-document-frozen")) -join ','), $tagPolyfillVerdict, $ehProbeVerdict, $(if ($runtimeEnvLimited) { 'ok(经引擎 Lua，与世界推进无关)' } else { 'ok' }), $formEvidence) -ForegroundColor Green
    if ($remoteOkLine) { Write-Host $remoteOkLine -ForegroundColor DarkGray }
} else {
    Write-Host ("wechat_minigame_package_ok files={0} runtime=skipped" -f $required.Count) -ForegroundColor Yellow
}

