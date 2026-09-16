[CmdletBinding()]
param(
    # Game project root to package.
    [string]$ProjectRoot = "",
    # Custom release name. Defaults to [project].name in project.toml.
    [string]$PublishName = "",
    # Target publish platform: pc, webgl or wechat-minigame.
    [ValidateSet("pc", "webgl", "wechat-minigame")]
    [string]$Platform = "pc",
    # Run the directory-isolation self test (copy to a temp dir with spaces,
    # launch the packaged game --pack without any source/target/.cache).
    [switch]$SelfTest,
    # Compress supported assets in the release pack (present = on).
    [switch]$Compress,
    # Encrypt Lua/scene resources in the release pack (present = on).
    [switch]$Encrypt,
    # M20-03: reuse existing release binaries (skip cargo builds) so repeated
    # publishes are fast and the staging/swap/rollback flow can be verified.
    [switch]$SkipBuild,
    # 微信小游戏：资源远端回落基址（包内读不到时才用）。留空 = 纯包内，不落配置文件。
    # 例：-RemotePackBaseUrl http://127.0.0.1:8787/pack/（配 scripts/serve-pack.ps1 本地测试）
    [string]$RemotePackBaseUrl = "",
    # 微信小游戏：资源包形态（三选一，只对 -Platform wechat-minigame 有意义）。
    #   subpackage = **微信默认**。主包 + 单分包：首屏资源切到兄弟目录 mainpack/（随主包上传），
    #                其余资源留在 pack/（声明成 game.json 的 subpackages），运行时按需
    #                wx.loadSubpackage 挂载后本地沙箱读取。零服务器、零备案、零域名白名单。
    #   full       = pack/ 随主包一起分发全部资源（整个包都计入 4MB 主包上限）
    #   remote     = 瘦首包：pack/ 只留骨架（project.toml.txt/settings.toml.txt/manifest.txt/chars.txt），
    #                重资源全部从远端下载；必须同时给 -RemotePackBaseUrl
    [ValidateSet("full", "subpackage", "remote")]
    [string]$AssetPack = $(if ($Platform -eq "wechat-minigame") { "subpackage" } else { "full" }),
    # 微信小游戏：资源包的**镜像**导出目录（形如 <目录>/pack/**），用于直接同步到 CDN。
    # 权威位置始终是 <发布根>/assets（与 launcher/ 同级、同一次原子交换落地）；
    # 给了这个参数就在 assets/ 之外**再**导出一份到该目录。
    [string]$CdnPackDir = ""
)

$ErrorActionPreference = "Stop"
$workspaceRoot = Split-Path -Parent $PSScriptRoot
$engineRoot = Join-Path $workspaceRoot "engine"
if ([string]::IsNullOrWhiteSpace($ProjectRoot)) {
    # examples 重组后样板工程在子目录里（根目录已无 project.toml）。
    $ProjectRoot = Join-Path $workspaceRoot "examples\game_jhyt"
}
$gameRoot = (Resolve-Path -LiteralPath $ProjectRoot).Path
$projectToml = Join-Path $gameRoot "project.toml"
if (-not (Test-Path $projectToml)) { throw "Game project.toml was not found: $projectToml" }
if ([string]::IsNullOrWhiteSpace($PublishName)) {
    $nameLine = Get-Content -LiteralPath $projectToml | Where-Object { $_ -match '^\s*name\s*=\s*"(.+)"\s*$' } | Select-Object -First 1
    if ($nameLine -and $nameLine -match '^\s*name\s*=\s*"(.+)"\s*$') { $PublishName = $Matches[1] }
    if ([string]::IsNullOrWhiteSpace($PublishName)) { $PublishName = Split-Path -Leaf $gameRoot }
}
$invalidNameChars = [IO.Path]::GetInvalidFileNameChars()
foreach ($ch in $invalidNameChars) { $PublishName = $PublishName.Replace([string]$ch, "_") }
$PublishName = $PublishName.Trim()
if ([string]::IsNullOrWhiteSpace($PublishName)) { $PublishName = "game" }
# M31/P3：玩家数据目录（存档 + 玩家设置）。与引擎 `core::user_dir` 的规则一致：
# Windows 为 `%APPDATA%\<publish_name>`，其它平台取各自的数据目录；`RUSTLUA_USER_DIR` 可覆盖。
# 这里只用于**文档与自检断言**（发布包目录里不写任何玩家数据）。
$PlayerDataDir = if ($env:RUSTLUA_USER_DIR) {
    $env:RUSTLUA_USER_DIR
} elseif ($env:APPDATA) {
    Join-Path $env:APPDATA $PublishName
} else {
    Join-Path $env:USERPROFILE (".local\share\" + $PublishName)
}
$distRoot = Join-Path $gameRoot ("builds\" + $Platform + "\" + $PublishName)
$packRoot = Join-Path $distRoot "pack"
# M20-03：原子发布。所有内容先写进 staging，全部成功后再原子替换最终目录；
# 失败时最终目录保持旧版本不变（不会出现半更新状态）。
. (Join-Path $PSScriptRoot "release-atomic.ps1")

# webgl 与微信小游戏共用同一套 WASM/静态产物，差别只在入口文件、资源配额与平台配置。
$isWebPlatform = $Platform -in @("webgl", "wechat-minigame")

# 三种形态都只对微信小游戏有意义（只有微信有"主包 / 分包 / 远端资源"这套划分），
# 且互斥：subpackage 资源全在包内，remote 资源全在远端，full 谁都不用。
# 混着给参数不会"更保险"，只会让两套资源来源互相掩盖失败（本地读到了就永远发现不了远端是坏的）。
if ($Platform -ne "wechat-minigame" -and $AssetPack -ne "full") {
    throw "-AssetPack $AssetPack 只支持 -Platform wechat-minigame（当前: $Platform）"
}
if ($AssetPack -eq "subpackage") {
    if (-not [string]::IsNullOrWhiteSpace($RemotePackBaseUrl)) {
        throw "-AssetPack subpackage 与 -RemotePackBaseUrl 互斥：分包形态的资源全部随包分发，配上远端回落会让'包内读到了'掩盖'分包装载失败'。"
    }
    if (-not [string]::IsNullOrWhiteSpace($CdnPackDir)) {
        throw "-AssetPack subpackage 与 -CdnPackDir 互斥：分包形态没有独立资源包可导出（资源在代码包里随包上传）。"
    }
}
# 瘦首包必须配远端基址：否则被裁掉的资源没有任何来源，产出的包是"能发布但跑不起来"。
if ($AssetPack -eq "remote" -and [string]::IsNullOrWhiteSpace($RemotePackBaseUrl)) {
    throw "-AssetPack remote 必须同时给 -RemotePackBaseUrl：pack/ 只留骨架，重资源只能从远端取。"
}
if (-not [string]::IsNullOrWhiteSpace($CdnPackDir) -and $Platform -ne "wechat-minigame") {
    throw "-CdnPackDir 只支持 -Platform wechat-minigame（当前: $Platform）"
}
# 资源包导出目录在 $finalRoot 确定后再算（见下），这里只做合法性检查。

# 字体解析必须早于 index.html 生成与运行器复制：index.html 需要真实字体路径，
# 且小游戏平台只随包分发 pack/ 里的子集字体（不复制完整字体）。
$fontLine = Get-Content -LiteralPath $projectToml | Where-Object { $_ -match '^\s*font\s*=\s*"(.+)"\s*$' } | Select-Object -First 1
$fontRelative = if ($fontLine -and $fontLine -match '^\s*font\s*=\s*"(.+)"\s*$') { $Matches[1] } else { "assets/fonts/NotoSansSC-VF.ttf" }
$fontSource = Join-Path $gameRoot ($fontRelative -replace '/', '\\')
if (-not (Test-Path $fontSource)) { throw "Game font resource was not found: $fontSource" }
if ((Get-Item $fontSource).Length -le 0) { throw "Game font resource is empty: $fontSource" }
$fontName = Split-Path -Leaf $fontSource
$publishNameJson = $PublishName | ConvertTo-Json -Compress

# webgl 与微信小游戏共用 plugins/webgl/web-runtime.js，而这份 JS 运行时只读明文 pack/
# （不做 P2PK1 解包、也不做 gzip 解压——那是 Rust 运行时的能力）。因此 -Encrypt/-Compress
# 会产出"打包成功但运行不起来"的包，这里直接拒绝，而不是发出去再排查。
if ($isWebPlatform -and ($Encrypt -or $Compress)) {
    throw "Platform '$Platform' cannot use -Encrypt/-Compress: the shared web runtime reads plaintext pack/ only (see plugins/webgl/README.md)."
}

# M31/P3：发布版运行器是 Windows GUI 子系统程序，`& exe args` 不等待也不捕获输出
# （`$LASTEXITCODE` 会保留前一条命令的值 ⇒ 自检空跑）。统一用 Start-Process -Wait +
# 重定向取回真实退出码与输出。返回 `[pscustomobject]@{ ExitCode; Output }`。
function Invoke-PackagedCheck {
    param(
        [Parameter(Mandatory = $true)][string]$Runner,
        [Parameter(Mandatory = $true)][string]$PackRoot
    )
    $stdoutFile = Join-Path $env:TEMP ("p2-pack-check-" + [guid]::NewGuid().ToString("N") + ".out")
    $stderrFile = $stdoutFile + ".err"
    try {
        # `Start-Process -ArgumentList` 会把数组按空格拼成命令行且**不加引号**，因此含空格的
        # 路径必须自己带引号（自测就是故意用 "p2 release test …" 这种带空格的路径）。
        $quotedPack = '"' + $PackRoot + '"'
        $process = Start-Process -FilePath $Runner `
            -ArgumentList @("--pack", $quotedPack, "--check") `
            -PassThru -Wait -NoNewWindow `
            -RedirectStandardOutput $stdoutFile -RedirectStandardError $stderrFile
        $lines = @()
        if (Test-Path -LiteralPath $stdoutFile) { $lines += Get-Content -LiteralPath $stdoutFile }
        if (Test-Path -LiteralPath $stderrFile) { $lines += Get-Content -LiteralPath $stderrFile }
        return [pscustomobject]@{ ExitCode = $process.ExitCode; Output = $lines }
    }
    finally {
        Remove-Item -LiteralPath $stdoutFile -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $stderrFile -Force -ErrorAction SilentlyContinue
    }
}

# 微信小游戏三种形态的体积配额不同，断言也必须不同：
#   subpackage：主包 = launcher 总量 − pack/（分包），三档都要断言（主包 4MB / 单分包 4MB / 总计 20MB）
#   full      ：pack/ 也在主包里，整个包都算主包 4MB（没有分包配额可用）
#   remote    ：pack/ 只剩骨架，主包 4MB；重资源在 CDN 上不占包体配额
# 超限时的**出路**必须写进报错里，否则使用者只看到"超了"却不知道该往哪走。
# 返回 [pscustomobject]@{ Main; Pack; Total }（字节），供调用方打印同一套口径。
function Assert-WeChatPackageVolume {
    param(
        [Parameter(Mandatory = $true)][double]$TotalBytes,
        [Parameter(Mandatory = $true)][double]$PackBytes,
        [Parameter(Mandatory = $true)][string]$Form,
        [string]$Suffix = ""
    )
    # 报文里的 $Suffix 用 `+` 拼接、不占 `-f` 的位置索引：这几条 throw 在日常发布里**走不到**
    # （分包 73.5%、瘦首包 11.7%），占位符写错只能等哪天真超限时才炸——那种守卫比没有守卫更糟。
    # **坑（实测）**：PowerShell 里 `+` 的结合力**高于** `-f`，所以 `"A" + "B" -f 1` 等价于
    # `"A" + ("B" -f 1)`——A 里的 `{0}` 会原样留在报文里，守卫变成一句没人看得懂的字面量。
    # 必须先把整条 `+` 表达式括起来再 `-f`；下面每条都显式括了双层。
    $mainBytes = if ($Form -eq "subpackage") { $TotalBytes - $PackBytes } else { $TotalBytes }
    if ($Form -eq "subpackage") {
        if ($mainBytes -gt 4MB) {
            throw ((("微信小游戏主包超限: {0:N0} KB > 4,096 KB（主包 = launcher 总量 {1:N0} KB − 分包 pack/ {2:N0} KB）。" +
                "出路（按优先级）：① 把更多资源列进首屏清单搬到 mainpack/（清单变大 ⇒ 主包变大、分包变小，" +
                "两侧此消彼长）；② 改用 -AssetPack remote -RemotePackBaseUrl <CDN> 把重资源移出包内。") -f `
                ($mainBytes / 1KB), ($TotalBytes / 1KB), ($PackBytes / 1KB)) + $Suffix)
        }
        if ($PackBytes -gt 4MB) {
            throw ((("微信小游戏单个分包超限: {0:N0} KB > 4,096 KB（pack/ 是 subpackages[0].root，目录下所有文件都计入）。" +
                "出路（按优先级）：① 把更多资源列进首屏清单搬去 mainpack/；② 拆第二个分包；" +
                "③ 改用 -AssetPack remote -RemotePackBaseUrl <CDN>。") -f ($PackBytes / 1KB)) + $Suffix)
        }
        # 这一档在当前产物形态下**走不到**（如实标注，别让人以为它被验证过）：主包 ≤ 4MB 且
        # 单分包 ≤ 4MB ⇒ 总量 ≤ 8MB < 20MB，而上面两条会先抛。留着是因为微信的 20MB 是
        # "主包 + 全部分包之和"的上限——一旦将来真的拆出第二个分包，这条才会变成活的门。
        if ($TotalBytes -gt 20MB) {
            throw (("微信小游戏代码包总量超限: {0:N0} KB > 20,480 KB（主包 + 全部分包之和）。" -f ($TotalBytes / 1KB)) + $Suffix)
        }
    }
    elseif ($mainBytes -gt 4MB) {
        throw ((("微信小游戏主包超限: {0:N0} KB > 4,096 KB（-AssetPack {1}：pack/ 不是分包，整个包都算主包；当前 pack/ {2:N0} KB）。" +
            "出路：改用 -AssetPack subpackage（主包 + 单分包，微信原生，零服务器零备案零域名白名单），" +
            "或 -AssetPack remote -RemotePackBaseUrl <CDN> 把重资源移出包内。") -f `
            ($mainBytes / 1KB), $Form, ($PackBytes / 1KB)) + $Suffix)
    }
    return [pscustomobject]@{ Main = $mainBytes; Pack = $PackBytes; Total = $TotalBytes }
}

$finalRoot = $distRoot
$staleRemoved = Clear-StaleReleaseStaging -FinalRoot $finalRoot -MaxAgeHours 24
foreach ($stale in $staleRemoved) {
    Write-Host "==> Removed stale release staging: $stale" -ForegroundColor DarkGray
}
# 微信小游戏的发布根下分两个兄弟目录（约定的产物形态）：
#   launcher/  首包：导入微信开发者工具 / 上传微信的内容（含 pack/ 与 mainpack/、remote-assets.json）
#   assets/    资源包：整目录上传 CDN，形如 <发布根>/assets/pack/**（只有 -AssetPack remote 才有）
# 其它平台没有这个划分，产物直接放在发布根下。
# 关键：两者都在**同一个 staging** 里生成，所以首包与资源包是一次原子交换同时生效——
# 否则"新首包 + 旧资源包"这种半更新状态只能等远端校验和事后暴露（真机表现为整批丢资源）。
$launcherSubdir = if ($Platform -eq "wechat-minigame") { "launcher" } else { "" }
$exportsAssetPack = ($Platform -eq "wechat-minigame") -and ($AssetPack -eq "remote")
# 分包形态：首屏资源从 pack/ 切到兄弟目录 mainpack/。**必须在复制平台文件之前定下来**，
# 因为 launcher 下的 game.json / 入口文件是否要处理分包声明，取决于这个布尔值。
$isSubpackagePack = ($Platform -eq "wechat-minigame") -and ($AssetPack -eq "subpackage")
# -CdnPackDir：除发布根下的 assets/ 之外**再**导出一份到该目录（给直接 rsync / s3 同步用）。
$cdnPackMirror = ""
if (-not [string]::IsNullOrWhiteSpace($CdnPackDir)) {
    $cdnPackMirror = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($CdnPackDir)
}
$stageRoot = New-ReleaseStaging -FinalRoot $finalRoot
$distRoot = if ($launcherSubdir) { Join-Path $stageRoot $launcherSubdir } else { $stageRoot }
$packRoot = Join-Path $distRoot "pack"
$cdnPackRoot = if ($exportsAssetPack) { Join-Path $finalRoot "assets" } else { "" }
$assetPackStage = if ($exportsAssetPack) { Join-Path $stageRoot "assets" } else { "" }
$script:cdnMirrorStage = ""
if ($cdnPackMirror) {
    foreach ($stale in (Clear-StaleReleaseStaging -FinalRoot $cdnPackMirror -MaxAgeHours 24)) {
        Write-Host "==> Removed stale asset-pack staging: $stale" -ForegroundColor DarkGray
    }
    Write-Host "==> 资源包镜像目录: $cdnPackMirror（权威位置是发布根下的 assets/）" -ForegroundColor Cyan
}
if ($exportsAssetPack) {
    Write-Host "==> 资源包随发布一起产出: $cdnPackRoot（形如 <发布根>/assets/pack/**，整目录上传 CDN）" -ForegroundColor Cyan
}
$script:swapAttempted = $false
Write-Host "==> Staging release at $stageRoot (atomic swap on success)" -ForegroundColor Cyan
$timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found. Install Rust stable and ensure cargo is on PATH."
}

New-Item -ItemType Directory -Force -Path $distRoot | Out-Null
Push-Location $engineRoot
try {
    Write-Host "==> Release build ($Platform)" -ForegroundColor Cyan
    if ($SkipBuild) {
        # M20-03: repeated publish without rebuilding; require existing artifacts.
        $requiredArtifacts = @((Join-Path $engineRoot "target\release\assetc.exe"))
        if ($Platform -eq "webgl") {
            # 只有 webgl 需要 wasm-bindgen 产物；微信小游戏不发行它（见下方 pkg/ 说明）。
            $requiredArtifacts += (Join-Path $engineRoot "target\wasm32-unknown-unknown\release\game_web.wasm")
        } elseif (-not $isWebPlatform) {
            $requiredArtifacts += (Join-Path $engineRoot "target\release\game.exe")
        }
        foreach ($artifact in $requiredArtifacts) {
            if (-not (Test-Path -LiteralPath $artifact)) {
                throw "SkipBuild requires existing release artifact: $artifact"
            }
        }
        Write-Host "==> Skipping cargo build (-SkipBuild): reusing existing artifacts" -ForegroundColor DarkGray
    }
    elseif ($isWebPlatform) {
        cargo build -p assetc --release
        if ($LASTEXITCODE -ne 0) { throw "assetc release build failed." }
        if ($Platform -eq "webgl") {
            rustup target add wasm32-unknown-unknown
            if ($LASTEXITCODE -ne 0) { throw "Install wasm32 target failed." }
            cargo build -p game_web --release --target wasm32-unknown-unknown
            if ($LASTEXITCODE -ne 0) { throw "WASM build failed ($Platform)." }
            if (-not (Get-Command wasm-bindgen -ErrorAction SilentlyContinue)) {
                Write-Host "==> Install wasm-bindgen-cli" -ForegroundColor Cyan
                cargo install wasm-bindgen-cli --version 0.2.127
                if ($LASTEXITCODE -ne 0) { throw "wasm-bindgen-cli install failed." }
            }
        }
        else {
            # 微信小游戏只用到 assetc 产物 + 共享 JS 运行时（Fengari 跑 Lua），
            # 不构建、也不发行 wasm 核心：少一步构建，也避免 ESM 胶水混进小游戏包。
            Write-Host "==> WeChat Mini Game build: assetc only (no wasm-bindgen artifacts)" -ForegroundColor DarkGray
        }
    }
    else {
        cargo build --workspace --release
        if ($LASTEXITCODE -ne 0) { throw "Release build failed." }
    }

    Write-Host "==> assetc package (platform=$Platform, data resource pack, compress=$Compress encrypt=$Encrypt)" -ForegroundColor Cyan
    $assetc = Join-Path $engineRoot "target\release\assetc.exe"
    if (Test-Path $packRoot) { Remove-Item -LiteralPath $packRoot -Recurse -Force }
    $assetcArgs = @("package", $gameRoot, "-o", $packRoot)
    if ($Compress) { $assetcArgs += "--compress" }
    if ($Encrypt) { $assetcArgs += "--encrypt" }
    & $assetc @assetcArgs
    if ($LASTEXITCODE -ne 0) { throw "assetc package failed." }

    Write-Host "==> Copy runner and docs" -ForegroundColor Cyan
    # 发布包只含运行器；assetc 为开发工具，不随包分发（并清理历史残留）。
    Remove-Item -Path (Join-Path $distRoot "assetc.exe") -Force -ErrorAction SilentlyContinue

    if ($isWebPlatform) {
        Remove-Item -Path (Join-Path $distRoot "game.exe") -Force -ErrorAction SilentlyContinue
        # pkg/：随包分发的 Lua VM（不请求 CDN，离线可运行）；webgl 另外放 wasm-bindgen 产物。
        $pkgRoot = Join-Path $distRoot "pkg"
        if (Test-Path $pkgRoot) { Remove-Item -LiteralPath $pkgRoot -Recurse -Force }
        New-Item -ItemType Directory -Force -Path $pkgRoot | Out-Null
        if ($Platform -eq "webgl") {
            $wasmInput = Join-Path $engineRoot "target\wasm32-unknown-unknown\release\game_web.wasm"
            wasm-bindgen $wasmInput --out-dir $pkgRoot --target web --no-typescript
            if ($LASTEXITCODE -ne 0) { throw "wasm-bindgen failed." }
        }
        # 微信小游戏**不随包分发** wasm-bindgen 胶水：它是浏览器 ESM（顶层 `export { … }`），
        # 小游戏运行时按 CommonJS 解析，加载即报
        #   WAGame.js:1 SyntaxError: MiniProgramError Unexpected token 'export'
        # 而这条胶水本来就没被使用（wechat-adapter 明确不加载 wasm 核心），
        # 所以直接不发——顺带把主包体积从 ~333KB 降到 ~275KB。
        $fengariSource = Join-Path $workspaceRoot "plugins\webgl\pkg\fengari-web.js"
        if (-not (Test-Path -LiteralPath $fengariSource)) {
            throw "Web platform package requires the vendored Lua runtime: plugins\webgl\pkg\fengari-web.js"
        }
        if ((Get-Item -LiteralPath $fengariSource).Length -le 0) {
            throw "Vendored Lua runtime is empty: plugins\webgl\pkg\fengari-web.js"
        }
        Copy-Item $fengariSource (Join-Path $pkgRoot "fengari-web.js") -Force
        # webgl 与微信小游戏共用同一份 Lua 运行时：两个平台的游戏逻辑不会各自漂移。
        $webRuntimeTemplate = Join-Path $workspaceRoot "plugins\webgl\web-runtime.js"
        if (-not (Test-Path -LiteralPath $webRuntimeTemplate)) {
            throw "Web platform package requires the shared Lua runtime: plugins\webgl\web-runtime.js"
        }
        Copy-Item $webRuntimeTemplate (Join-Path $distRoot "web-runtime.js") -Force

        if ($Platform -eq "wechat-minigame") {
            # 微信小游戏入口：game.js（入口）+ wechat-adapter.js（wx 宿主适配）+ 两个配置。
            # 平台文件都在 plugins/wechat-minigame/，发布脚本只负责复制、改名与校验。
            $wechatPlugin = Join-Path $workspaceRoot "plugins\wechat-minigame"
            foreach ($pluginFile in @("game.js", "game.json", "project.config.json", "wechat-adapter.js")) {
                $pluginSource = Join-Path $wechatPlugin $pluginFile
                if (-not (Test-Path -LiteralPath $pluginSource)) {
                    throw "WeChat platform plugin is missing: $pluginSource"
                }
                Copy-Item -LiteralPath $pluginSource -Destination (Join-Path $distRoot $pluginFile) -Force
            }
            # game.json 的 subpackages **按形态注入，不写死在模板里**：
            # 模板带上分包声明会让另外两种形态（full / remote）被微信直接拒开——声明的 root
            # 必须真的存在且含入口，否则开发者工具报「未找到 ["subpackages"][0]["root"] 对应的
            # /pack/game.js 文件」并拒绝打开工程（实测踩过，也是历史上取消分包的直接原因）。
            $gameConfigPath = Join-Path $distRoot "game.json"
            $gameConfig = Get-Content -LiteralPath $gameConfigPath -Raw -Encoding UTF8 | ConvertFrom-Json
            # 两种拼写都要清：小游戏用 subpackages，小程序用 subPackages，宿主对拼写敏感。
            $gameConfig.PSObject.Properties.Remove("subpackages")
            $gameConfig.PSObject.Properties.Remove("subPackages")
            if ($isSubpackagePack) {
                # 单分包 pack/：root 目录下**所有**文件都进分包（主包里一个都不能留），
                # 所以首屏资源已经被切到兄弟目录 mainpack/ 了，这里声明的 pack/ 就只剩非首屏资源。
                $subpackageEntry = New-Object PSObject
                $subpackageEntry | Add-Member -NotePropertyName name -NotePropertyValue "pack"
                $subpackageEntry | Add-Member -NotePropertyName root -NotePropertyValue "pack"
                $gameConfig | Add-Member -NotePropertyName subpackages -NotePropertyValue @($subpackageEntry) -Force
            }
            $gameConfigJson = $gameConfig | ConvertTo-Json -Depth 10
            [IO.File]::WriteAllText($gameConfigPath, $gameConfigJson, (New-Object Text.UTF8Encoding($false)))
            # 分包入口 pack/game.js：微信要求 subpackages[i].root 下有 game.js，挂载后会**执行**它
            # （不只是存在性占位）。它由发布脚本生成而不是 assetc 打进去的：assetc 只认工程目录，
            # 而这是发布形态的产物。包内每个 .js 都会被下面三处按 CommonJS 解析，必须合法。
            $subpackageEntryPath = Join-Path $packRoot "game.js"
            if ($isSubpackagePack) {
                $subpackageEntryBody = @(
                    "'use strict';",
                    "// 微信小游戏的**分包入口**：subpackages[i].root === 'pack'，挂载后由宿主执行。",
                    "// 这个文件不加载任何资源——分包里的资源由主包侧 launcher/wechat-adapter.js 按",
                    "// mainpack.json 静态路由后读取。它存在的意义只有一个：微信强制要求 root 目录下有",
                    "// game.js，缺了开发者工具连工程都打不开（报错形如「未找到 subpackages[0].root",
                    "// 对应的 /pack/game.js 文件」）。",
                    "module.exports = { subpackage: 'pack', mount: 'resources-only' };",
                    ""
                ) -join "`r`n"
                [IO.File]::WriteAllText($subpackageEntryPath, $subpackageEntryBody, (New-Object Text.UTF8Encoding($false)))
            }
            elseif (Test-Path -LiteralPath $subpackageEntryPath) {
                # 非分包形态下 pack/game.js 是个**危险的假货**：它既不是主包入口，又会被
                # `subpackage-relative` 方言读成主包入口 `game.js`（见适配层的方言注释）。
                Remove-Item -LiteralPath $subpackageEntryPath -Force
                Write-Warning "==> 已移除 pack/game.js（-AssetPack $AssetPack 不是分包形态，留着会盖住主包入口）"
            }
            # mainpack/ 只属于分包形态：其它形态下留着会让开发者工具按目录结构误判。
            $mainPackRoot = Join-Path $distRoot "mainpack"
            $mainPackManifest = Join-Path $distRoot "mainpack.json"
            if (-not $isSubpackagePack) {
                foreach ($stalePath in @($mainPackRoot, $mainPackManifest)) {
                    if (Test-Path -LiteralPath $stalePath) {
                        Remove-Item -LiteralPath $stalePath -Recurse -Force
                        Write-Warning "==> 已移除残留的 $stalePath（-AssetPack $AssetPack 不是分包形态）"
                    }
                }
            }
            # project.config.json 的工程名跟随 publish_name，方便开发者工具里区分工程。
            $projectConfigPath = Join-Path $distRoot "project.config.json"
            # PS 5.1 的 Get-Content 默认按 ANSI 解码：必须显式 UTF8，否则中文工程名会被写坏。
            $projectConfig = Get-Content -LiteralPath $projectConfigPath -Raw -Encoding UTF8 | ConvertFrom-Json
            $projectConfig.projectname = $PublishName
            $projectConfigJson = $projectConfig | ConvertTo-Json -Depth 10
            # 开发者工具的 JSON 解析对 BOM 敏感：写无 BOM 的 UTF-8。
            [IO.File]::WriteAllText($projectConfigPath, $projectConfigJson, (New-Object Text.UTF8Encoding($false)))
            # 资源远端回落配置：只在显式给了 -RemotePackBaseUrl 时落盘；没给就**必须清掉**，
            # 否则上一次发布的远端配置会悄悄留在包里（行为不一致且难以排查）。
            $remoteConfigPath = Join-Path $distRoot "remote-assets.json"
            if ([string]::IsNullOrWhiteSpace($RemotePackBaseUrl)) {
                if (Test-Path -LiteralPath $remoteConfigPath) { Remove-Item -LiteralPath $remoteConfigPath -Force }
            } else {
                if ($RemotePackBaseUrl -notmatch '^https?://') {
                    throw "-RemotePackBaseUrl 必须以 http:// 或 https:// 开头: $RemotePackBaseUrl"
                }
                # 归一化：基址指向 pack/ 的**内容**目录（适配层会把 pack/ 前缀去掉再拼接）。
                $baseUrl = $RemotePackBaseUrl.TrimEnd('/')
                if ($baseUrl -notmatch '/pack$') { $baseUrl = "$baseUrl/pack" }
                $baseUrl = "$baseUrl/"
                $remoteConfig = [ordered]@{ packBaseUrl = $baseUrl; verifyChecksum = $true; timeoutMs = 15000 }
                # ASCII 内容 + 无 BOM：JSON.parse 与开发者工具都不会踩到编码坑。
                [IO.File]::WriteAllText($remoteConfigPath, ($remoteConfig | ConvertTo-Json -Depth 5), (New-Object Text.UTF8Encoding($false)))
                Write-Host ("==> 远端资源回落: {0}（包内仍是首选，读不到才下载）" -f $baseUrl) -ForegroundColor DarkGray
            }
            # 小游戏包不复制完整字体：assetc 已把用到的字符子集化进 pack/。
            $packedFont = Join-Path $packRoot ("assets\fonts\" + $fontName)
            if (-not (Test-Path -LiteralPath $packedFont)) {
                throw "WeChat package requires the subset font inside pack/: $packedFont"
            }
            if ((Get-Item -LiteralPath $packedFont).Length -le 0) {
                throw "WeChat subset font is empty: $packedFont"
            }
            Write-Host ("==> WeChat Mini Game entry staged (subset font {0:N0} KB; full font not shipped)" -f ((Get-Item -LiteralPath $packedFont).Length / 1KB)) -ForegroundColor DarkGray
        }
        else {
            # 浏览器页面来自平台插件模板：占位符替换 publish_name 与真实字体路径。
            $indexTemplate = Join-Path $workspaceRoot "plugins\webgl\index.html"
            if (-not (Test-Path -LiteralPath $indexTemplate)) {
                throw "WebGL platform plugin is missing: plugins\webgl\index.html"
            }
            $indexHtml = Get-Content -LiteralPath $indexTemplate -Raw -Encoding UTF8
            $indexHtml = $indexHtml.Replace('__PUBLISH_NAME_JSON__', $publishNameJson)
            $indexHtml = $indexHtml.Replace('__PUBLISH_NAME__', $PublishName)
            $indexHtml = $indexHtml.Replace('__FONT_PATH__', "./fonts/$fontName")
            [IO.File]::WriteAllText((Join-Path $distRoot "index.html"), $indexHtml, (New-Object Text.UTF8Encoding($false)))
            $fontTarget = Join-Path $distRoot ("fonts\\" + $fontName)
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $fontTarget) | Out-Null
            Copy-Item -Path $fontSource -Destination $fontTarget -Force
            if (-not (Test-Path -LiteralPath $fontTarget) -or (Get-Item -LiteralPath $fontTarget).Length -le 0) {
                throw "Release package font is missing or empty: fonts/$fontName"
            }
            # 包内双击壳：与 index.html 一样来自平台插件模板，必须由发布脚本生成——
            # Complete-ReleaseSwap 会把发布根下所有非 .staging-*/.backup-* 条目移入备份后删除，
            # 手工丢进产物目录的脚本每次发布会消失。模板本身与游戏工程名、目录深度无关
            # （它从自身位置向上找仓库根），因此任何工程发布 webgl 包都自动带上它。
            $testBatTemplate = Join-Path $workspaceRoot "plugins\webgl\test-webgl.bat"
            if (-not (Test-Path -LiteralPath $testBatTemplate)) {
                throw "WebGL platform plugin is missing: plugins\webgl\test-webgl.bat"
            }
            Copy-Item -LiteralPath $testBatTemplate -Destination (Join-Path $distRoot "test-webgl.bat") -Force
        }
    }
    else {
        Copy-Item -Path (Join-Path $engineRoot "target\release\game.exe") `
            -Destination (Join-Path $distRoot "game.exe") -Force
    }

    # 工程级设置（settings.toml：手柄改键与死区）随包分发：发布包以 pack 目录作为
    # 工程根，运行器按 `<root>/settings.toml` 读取，否则打包版会回落到键盘默认绑定，
    # 出现「编辑器里改好的手柄绑定在发布包里失效」。
    $settingsSource = Join-Path $gameRoot "settings.toml"
    $settingsPackaged = $false
    if (Test-Path -LiteralPath $settingsSource) {
        Copy-Item -Path $settingsSource -Destination (Join-Path $packRoot "settings.toml") -Force
        $settingsPackaged = $true
        Write-Host "==> Packaged settings.toml (input bindings / deadzone)" -ForegroundColor DarkGray
    }

    @(
        "publish_name = `"$PublishName`""
    ) | Set-Content (Join-Path $distRoot "release.toml") -Encoding UTF8

    # ------------------------------------------------ 调试残留拦截（全平台，早于所有体积统计）
    # `debug-overlay.txt` 是**开发者工具里排查真机黑屏**用的开关（适配层把它当"开启屏幕自证覆盖层"
    # 的信号）。它一旦随包发出去，玩家的屏幕上就会常年挂着一层诊断色块与文字——而它是纯文本
    # 标记，发布脚本不拦就没人拦。删掉不影响任何自动化用例：harness 的覆盖层用例走的是
    # `wx.setStorageSync('p2_debug_overlay','1')`，不依赖这个文件。
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
    #   mainpack.json      —— 仅分包形态：首屏路由（读主包还是读分包）
    #   code-package.json  —— 全形态：逻辑路径 → 会随代码包上传的宿主文件名（后缀白名单合规）
    $firstScreenTool = Join-Path $PSScriptRoot "first-screen-pack.js"
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
            $firstScreenArgs = @($firstScreenTool, $packRoot, "--rename-map", $codePackageMap)
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

    # ------------------------------------ 分包形态：首屏资源切分到 mainpack/（subpackage）
    # 微信规则：subpackages[i].root 目录下的文件**全部**进分包，主包里一个都不能留。
    # 所以"主包保留首屏资源"不能表现为"在 pack/ 里留一部分"（那样留的在真机上仍属分包、
    # 首屏照样要等分包下载），只能把清单内的文件搬到**兄弟目录 mainpack/**（同构相对路径），
    # 由适配层按清单静态路由：首屏读 mainpack/，其余读 pack/。
    # 切分依据是 scripts/first-screen-pack.js 生成的 mainpack.json —— 构建期、运行时、harness
    # 三方共用的唯一事实来源，从 pack/ 的实际磁盘内容推导，不写死工程名或目录层级。
    # 与 remote 的关键差别：remote 是 Remove-Item（资源只在远端），这里是 Move-Item（资源一份不少）。
    # 注意：这里全程用**逻辑路径**搬运（改名在这之后、对两棵树一起做），所以下面的断言也都在
    # 逻辑名下做；"宿主名"相关的断言在改名之后单独跑一遍。
    if ($isSubpackagePack) {
        $packFileCountBefore = (Get-ChildItem -LiteralPath $packRoot -Recurse -File -Force | Measure-Object).Count
        $packBytesBefore = (Get-ChildItem -LiteralPath $packRoot -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum

        if (-not (Test-Path -LiteralPath $mainPackManifest)) {
            throw "首屏清单没有被生成: $mainPackManifest"
        }
        $firstScreenPlan = Get-Content -LiteralPath $mainPackManifest -Raw -Encoding UTF8 | ConvertFrom-Json
        $movedEntries = @($firstScreenPlan.entries)
        # 清单为空 = 首屏清单工具失效（它至少要放进骨架与启动场景）。若真发生，主包会只剩
        # launcher 自身，首屏全部依赖分包——这正是本形态要消除的情形，必须当场失败。
        if ($movedEntries.Count -eq 0) {
            throw "首屏清单为空（$mainPackManifest）：分包形态下首屏资源必须全部留在主包，空清单等于把首屏交给分包。"
        }

        New-Item -ItemType Directory -Force -Path $mainPackRoot | Out-Null
        $movedBytes = 0
        foreach ($entry in $movedEntries) {
            # 清单里的路径一律用 `/`（与 web-runtime.js 的 URL 风格一致），落到磁盘要转成 `\`。
            $relative = $entry -replace '/', '\'
            $from = Join-Path $packRoot $relative
            if (-not (Test-Path -LiteralPath $from)) {
                throw "首屏清单列了 pack/ 里不存在的文件: $entry"
            }
            $to = Join-Path $mainPackRoot $relative
            $toDir = Split-Path -Parent $to
            if (-not (Test-Path -LiteralPath $toDir)) { New-Item -ItemType Directory -Force -Path $toDir | Out-Null }
            $movedBytes += (Get-Item -LiteralPath $from).Length
            Move-Item -LiteralPath $from -Destination $to -Force
        }

        # 只删**因为搬运而空掉**的目录（自底向上反复清理到不动点）：任何还有文件留下的目录
        # （例如 assets/art 下既有首屏贴图又有非首屏贴图）天然保留，不会被误删。
        $emptiedDirs = @()
        $sweepAgain = $true
        while ($sweepAgain) {
            $sweepAgain = $false
            foreach ($dir in @(Get-ChildItem -LiteralPath $packRoot -Recurse -Directory -Force)) {
                if (-not (Test-Path -LiteralPath $dir.FullName)) { continue }
                if (@(Get-ChildItem -LiteralPath $dir.FullName -Force | Select-Object -First 1).Count -ne 0) { continue }
                Remove-Item -LiteralPath $dir.FullName -Recurse -Force
                $emptiedDirs += $dir.FullName.Substring($packRoot.Length).TrimStart('\')
                $sweepAgain = $true
            }
        }

        # 字节守恒：切分前后 pack/ + mainpack/ 必须与切分前的 pack/ **逐一对应**。
        # 这条断言是"切分不全 / 清单漏列"唯一的守门人——漏列的文件会被路由到分包，未挂载时
        # 首屏就是一片 Image.onerror（屏幕红条 + imageFailed>0），而那时已经发到线上了。
        $packBytesAfter = (Get-ChildItem -LiteralPath $packRoot -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
        $packFileCountAfter = (Get-ChildItem -LiteralPath $packRoot -Recurse -File -Force | Measure-Object).Count
        $mainPackBytes = (Get-ChildItem -LiteralPath $mainPackRoot -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
        $mainPackFileCount = (Get-ChildItem -LiteralPath $mainPackRoot -Recurse -File -Force | Measure-Object).Count
        if ($mainPackBytes -ne $movedBytes -or $mainPackFileCount -ne $movedEntries.Count) {
            throw ("首屏切分搬运不完整: mainpack/ 是 {0} 文件/{1} 字节，清单是 {2} 条/{3} 字节" -f `
                $mainPackFileCount, $mainPackBytes, $movedEntries.Count, $movedBytes)
        }
        if (($mainPackBytes + $packBytesAfter) -ne $packBytesBefore -or ($mainPackFileCount + $packFileCountAfter) -ne $packFileCountBefore) {
            throw ("首屏切分没有守恒: mainpack/ {0} + pack/ {1} = {2} 字节，切分前 pack/ 是 {3} 字节" -f `
                $mainPackBytes, $packBytesAfter, ($mainPackBytes + $packBytesAfter), $packBytesBefore)
        }
        # 分包入口是**声明与入口成对**的产物：少了它开发者工具直接拒开工程，所以在这里
        # 立刻断言，而不是等最后那条必需文件检查（那条也要留，用于全形态统一兜底）。
        if (-not (Test-Path -LiteralPath (Join-Path $packRoot "game.js"))) {
            throw "分包形态缺少分包入口 pack/game.js（微信要求 subpackages[i].root 目录下有 game.js）"
        }
        Write-Host ("wechat_minigame_split_ok mainpack_files={0} mainpack_kb={1:N0} subpackage_files={2} subpackage_kb={3:N0} swept_empty_dirs={4}" -f `
            $mainPackFileCount, ($mainPackBytes / 1KB), $packFileCountAfter, ($packBytesAfter / 1KB), $emptiedDirs.Count) -ForegroundColor Cyan
        Write-Host ("==> 首屏资源已切到 mainpack/：主包直接可读，其余 {0:N0} KB 在 pack/ 分包里按需挂载" -f ($packBytesAfter / 1KB)) -ForegroundColor Cyan
    }

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
            Copy-Item -LiteralPath $packRoot -Destination (Join-Path $assetPackStage "pack") -Recurse -Force
            $fullFiles = (Get-ChildItem -LiteralPath (Join-Path $assetPackStage "pack") -Recurse -File -Force | Measure-Object).Count
            $fullBytes = (Get-ChildItem -LiteralPath (Join-Path $assetPackStage "pack") -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
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
            # 骨架缺一不可：瘦首包形态下 pack/ 不是分包，但 manifest/project.toml 仍是运行入口，
            # 缺了就只能白屏（本形态没有分包声明，所以也没有 pack/game.js 入口）。
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
            if ($thinBytes -gt 512KB) {
                throw ("瘦首包裁剪未生效: pack/ 仍有 {0:N0} KB（pack/ 根目录下出现了大文件，需要更新骨架白名单）" -f ($thinBytes / 1KB))
            }
            Write-Host ("==> 瘦首包: pack/ 仅保留骨架（{0:N0} KB，移除目录: {1}），重资源走远端 {2}" -f `
                ($thinBytes / 1KB), ($removedDirs -join ','), $RemotePackBaseUrl) -ForegroundColor Cyan
        }
    }

    # ------------------------------------ 代码包后缀合规：按映射改名（**必须最后做**）
    # 微信代码包只上传后缀在白名单内的文件（scripts/code-package-rules.js，官方《代码包》文档）：
    # `.lua` / `.toml` / `.md` 传不上去，真机上 `readFileSync` 得到 permission denied，
    # 而这个游戏的全部 Lua 逻辑都靠逐文件读取 —— 实测真机启动直接失败（`pack/project.toml HTTP 404`）。
    # 所以这里把产物里每个需要改名的文件改成 `<原名>.txt`（末位后缀入白名单，原名可追溯），
    # 并把"逻辑路径 → 宿主文件名"的映射落在产物根的 code-package.json；运行时只查表、不重算规则。
    #
    # **位置固定在最后**：CDN 导出（assets/ 与 -CdnPackDir 镜像）与瘦身都按**逻辑路径**工作。
    # CDN 是普通 HTTPS 服务器、不受白名单约束，而远端 URL / 用户目录缓存路径 / manifest.txt 校验和
    # 全部按逻辑路径算——先改名会让远端回落静默 404（同类顺序事故在本文件已有先例，见"必须早于裁剪"）。
    if ($Platform -eq "wechat-minigame") {
        $renamedCount = 0
        $renameTrees = @($packRoot)
        if ($mainPackRoot -and (Test-Path -LiteralPath $mainPackRoot)) { $renameTrees = @($mainPackRoot) + $renameTrees }
        foreach ($tree in $renameTrees) {
            foreach ($key in @($script:codePackageRenamed.Keys)) {
                $relative = $key -replace '/', '\'
                $from = Join-Path $tree $relative
                if (-not (Test-Path -LiteralPath $from)) { continue }
                $to = Join-Path $tree ($script:codePackageRenamed[$key] -replace '/', '\')
                $toDir = Split-Path -Parent $to
                if (-not (Test-Path -LiteralPath $toDir)) { New-Item -ItemType Directory -Force -Path $toDir | Out-Null }
                if (Test-Path -LiteralPath $to) {
                    throw ("改名目标已存在，会静默覆盖: {0} → {1}" -f $from, $to)
                }
                Move-Item -LiteralPath $from -Destination $to
                $renamedCount += 1
            }
        }
        if ($renamedCount -ne $script:codePackageRenamed.Count -and $isSubpackagePack) {
            # 分包形态下两棵树合起来应当恰好命中映射里的每一条（首屏的在 mainpack/、其余在 pack/）；
            # remote/full 形态只有一棵树，数量可能少于映射（被裁掉的资源本来就不在包里），不在此断言之列。
            throw ("改名没有全覆盖: 实际改了 {0} 个，映射有 {1} 条" -f $renamedCount, $script:codePackageRenamed.Count)
        }
        # 收尾断言：代码包里不得再有非白名单后缀的文件——将来往分包里放 .lua 会在这里当场失败，
        # 而不是等真机上"启动失败、日志里只有一串 permission denied"。
        $whitelistDirs = @($packRoot)
        if ($mainPackRoot -and (Test-Path -LiteralPath $mainPackRoot)) { $whitelistDirs = @($mainPackRoot) + $whitelistDirs }
        if (-not $nodeForPack) { $nodeForPack = Get-Command node -ErrorAction SilentlyContinue }
        $savedEap2 = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $whitelistOutput = & $nodeForPack.Source $firstScreenTool --check-whitelist @whitelistDirs 2>&1
            $whitelistExit = $LASTEXITCODE
        }
        finally {
            $ErrorActionPreference = $savedEap2
        }
        if ($whitelistExit -ne 0) {
            throw ("代码包里仍有非白名单后缀的文件（真机不会上传它们）: " + (($whitelistOutput | ForEach-Object { "$_" }) -join ' '))
        }
        Write-Host ("==> 已按白名单改名 {0} 个文件；{1}" -f $renamedCount, (($whitelistOutput | Where-Object { $_ -match '^code_package_whitelist_ok' } | Select-Object -First 1))) -ForegroundColor DarkGray
    }

    # ---------------------------------------------------------------- 平台产物校验
    if ($Platform -eq "wechat-minigame") {
        # 骨架的位置随形态变：manifest.txt / project.toml 都在首屏清单里，所以分包形态下
        # 它们已经被搬到 mainpack/，pack/ 里**不该**再有同名副本（有就说明切分漏了）。
        # 名字一律用**宿主文件名**（改名已在上一步完成：project.toml → project.toml.txt）。
        $skeletonRoot = if ($isSubpackagePack) { $mainPackRoot } else { $packRoot }
        function Resolve-CodePackName([string]$logical) {
            if ($script:codePackageRenamed -and $script:codePackageRenamed.ContainsKey($logical)) {
                return $script:codePackageRenamed[$logical]
            }
            return $logical
        }
        $requiredWeChatFiles = @(
            (Join-Path $distRoot "game.js"),
            (Join-Path $distRoot "game.json"),
            (Join-Path $distRoot "project.config.json"),
            (Join-Path $distRoot "wechat-adapter.js"),
            (Join-Path $distRoot "web-runtime.js"),
            (Join-Path $distRoot "pkg\fengari-web.js"),
            (Join-Path $distRoot "code-package.json"),
            (Join-Path $skeletonRoot (Resolve-CodePackName "manifest.txt")),
            (Join-Path $skeletonRoot (Resolve-CodePackName "project.toml"))
        )
        if ($isSubpackagePack) {
            # 分包形态多两项：清单（适配层的静态路由依据）与分包入口（微信的硬要求）。
            $requiredWeChatFiles += @(
                (Join-Path $distRoot "mainpack.json"),
                (Join-Path $packRoot "game.js")
            )
            # 逻辑名与宿主名**都不该**出现在 pack/ 里：前者说明切分漏了，后者说明搬运与改名对不上。
            foreach ($mustNotBeInPack in @("manifest.txt", "project.toml")) {
                foreach ($candidate in @($mustNotBeInPack, (Resolve-CodePackName $mustNotBeInPack))) {
                    if (Test-Path -LiteralPath (Join-Path $packRoot $candidate)) {
                        throw "分包形态下 $candidate 只应存在于主包（mainpack/），pack/ 里出现说明切分/改名不完整: $packRoot\$candidate"
                    }
                }
            }
            # 清单项必须"在 mainpack/ 里、不在 pack/ 里"（按**宿主文件名**判定）。
            # 这是本形态在本机唯一能验证的形态判据：真机的打包语义（root 目录整体进分包）本机证明不了，
            # 只能靠"两份互斥"把产物形态锁死。
            $manifestEntries = @((Get-Content -LiteralPath $mainPackManifest -Raw -Encoding UTF8 | ConvertFrom-Json).entries)
            foreach ($entry in $manifestEntries) {
                $hostName = Resolve-CodePackName $entry
                $entryRelative = $hostName -replace '/', '\'
                if (-not (Test-Path -LiteralPath (Join-Path $mainPackRoot $entryRelative))) {
                    throw "首屏清单项（宿主名 $hostName）不在主包里: mainpack/$hostName"
                }
                if (Test-Path -LiteralPath (Join-Path $packRoot $entryRelative)) {
                    throw "首屏清单项同时留在分包里（会与主包那份争抢读取路径）: pack/$hostName"
                }
            }
        }
        # 配了远端回落就必须真的有配置文件（否则适配层不会启用远端，行为与预期不符）。
        if (-not [string]::IsNullOrWhiteSpace($RemotePackBaseUrl)) {
            $requiredWeChatFiles += (Join-Path $distRoot "remote-assets.json")
        }
        # 三形态互斥的另一半：分包形态不得带远端配置，否则适配层会走 CDN 回落，
        # "包内读到了"就永远发现不了"分包根本没挂载上"。
        if ($isSubpackagePack -and (Test-Path -LiteralPath (Join-Path $distRoot "remote-assets.json"))) {
            throw "分包形态不得带 remote-assets.json（它属于 -AssetPack remote）"
        }
        foreach ($file in $requiredWeChatFiles) {
            if (-not (Test-Path -LiteralPath $file)) { throw "WeChat package is missing: $file" }
            if ((Get-Item -LiteralPath $file).Length -le 0) { throw "WeChat package file is empty: $file" }
        }
        # PC/WebGL 的入口不得混入小游戏包：分支选错平台时会在这里失败，而不是发到线上才发现。
        foreach ($forbidden in @("game.exe", "index.html", "assetc.exe")) {
            if (Test-Path -LiteralPath (Join-Path $distRoot $forbidden)) {
                throw "WeChat package must not contain $forbidden (wrong platform branch?)"
            }
        }
        # 浏览器 ESM 胶水同样不得混入：小游戏按 CommonJS 解析，顶层 `export` 会让运行时
        # 在 WAGame.js 里抛 SyntaxError: Unexpected token 'export'（实测踩过）。
        foreach ($forbidden in @("pkg\game_web.js", "pkg\game_web_bg.wasm")) {
            if (Test-Path -LiteralPath (Join-Path $distRoot $forbidden)) {
                throw "WeChat package must not contain $forbidden (browser ESM glue; the mini game runtime parses CommonJS only)"
            }
        }
        # 通用兜底：小游戏包里每个 .js 都必须能按**脚本**（CommonJS）解析。
        # 注意不能用 `node --check`：Node 会按内容自动识别 ESM，对顶层 export 反而"通过"，
        # 必须用 vm.Script（始终按 script 解析）或等价手段。
        $wechatScripts = Get-ChildItem -LiteralPath $distRoot -Recurse -File -Filter *.js -Force
        $nodeForSyntax = Get-Command node -ErrorAction SilentlyContinue
        if ($nodeForSyntax) {
            # 探针**只写 stdout**：PS 5.1 在 $ErrorActionPreference='Stop' 下会把原生程序的 stderr
            # 变成终止性错误，一旦用 2>&1 捕获，脚本会在检查退出码之前就中断，
            # 于是"守卫触发了"和"守卫自己崩了"看起来一模一样（实测踩过）。
            $syntaxProbe = "const fs=require('fs'),vm=require('vm'),p=process.argv[1];try{new vm.Script(fs.readFileSync(p,'utf8'),{filename:p});process.exit(0)}catch(e){process.stdout.write(String(e.message).split('\n')[0]);process.exit(1)}"
            foreach ($scriptFile in $wechatScripts) {
                $probe = & $nodeForSyntax.Source -e $syntaxProbe $scriptFile.FullName
                if ($LASTEXITCODE -ne 0) {
                    throw ("WeChat package contains a non-CommonJS script: {0} -> {1}" -f $scriptFile.Name, ($probe -join ' '))
                }
            }
            Write-Host ("==> WeChat scripts are CommonJS-parseable: {0} files" -f $wechatScripts.Count) -ForegroundColor DarkGray
        }
        else {
            # 没有 node 时退回文本检查：顶层 export/import 语句（压缩单行文件不会误伤，
            # 因为按行首匹配，且它们在行首不是合法 CommonJS）。
            foreach ($scriptFile in $wechatScripts) {
                $match = Select-String -LiteralPath $scriptFile.FullName -Pattern '^\s*(export\s|import\s+[^\(])' | Select-Object -First 1
                if ($match) {
                    throw "WeChat package contains a non-CommonJS script: $($scriptFile.Name) (line $($match.LineNumber))"
                }
            }
        }
        # 代码包体积上限：三形态的算法与配额都不同，统一交给 Assert-WeChatPackageVolume。
        # 注意这里只是**提前拦截**（run.txt/deps.txt/build-info.txt 还没生成），
        # 最终数值在收尾处重新测量并输出 wechat_minigame_package_ok。
        $wechatTotalSize = (Get-ChildItem -LiteralPath $distRoot -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
        $wechatPackSize = (Get-ChildItem -LiteralPath $packRoot -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
        $wechatVolume = Assert-WeChatPackageVolume -TotalBytes $wechatTotalSize -PackBytes $wechatPackSize `
            -Form $AssetPack -Suffix "（本值测于 run.txt/deps.txt/build-info.txt 生成之前，最终值见收尾的 wechat_minigame_package_ok）"
        Write-Host ("wechat_minigame_size_precheck files={0} main_kb={1:N0} main_of_limit={2:P1} subpackage_kb={3:N0} subpackage_of_limit={4:P1} pack_kb={5:N0} total_kb={6:N0} total_of_limit={7:P1} asset_pack={8}" -f `
            $requiredWeChatFiles.Count, ($wechatVolume.Main / 1KB), ($wechatVolume.Main / 4MB), `
            $(if ($isSubpackagePack) { $wechatVolume.Pack / 1KB } else { 0 }), `
            $(if ($isSubpackagePack) { $wechatVolume.Pack / 4MB } else { 0 }), `
            ($wechatPackSize / 1KB), ($wechatVolume.Total / 1KB), ($wechatVolume.Total / 20MB), $AssetPack) -ForegroundColor DarkGray
    }
    elseif ($Platform -eq "webgl") {
        $requiredWebFiles = @(
            (Join-Path $distRoot "index.html"),
            (Join-Path $distRoot "test-webgl.bat"),
            (Join-Path $distRoot "web-runtime.js"),
            (Join-Path $distRoot "pkg\game_web.js"),
            (Join-Path $distRoot "pkg\game_web_bg.wasm"),
            (Join-Path $distRoot "pkg\fengari-web.js"),
            (Join-Path $packRoot "manifest.txt"),
            (Join-Path $packRoot "project.toml")
        )
        foreach ($file in $requiredWebFiles) {
            if (-not (Test-Path -LiteralPath $file)) { throw "WebGL package is missing: $file" }
        }
        # 0 字节的壳等于没有壳：存在性检查放过它，双击时只会一闪而过。
        $testBat = Join-Path $distRoot "test-webgl.bat"
        if ((Get-Item -LiteralPath $testBat).Length -le 0) { throw "WebGL package shell is empty: test-webgl.bat" }
        Write-Host "webgl_package_ok files=$($requiredWebFiles.Count)" -ForegroundColor Green
    }
    else {
        # 注意：发布版 `game.exe` 是 **Windows GUI 子系统**程序
        # （`crates/game/src/main.rs` 的 `windows_subsystem = "windows"`，双击时不弹控制台）。
        # 这类程序用 `& exe args` 调用时 PowerShell **既不等待也不捕获 stdout**，
        # `$LASTEXITCODE` 会保留上一条命令的值——于是"发布包自检"会静默空跑成永远通过。
        # 必须用 Start-Process -Wait + 重定向文件来真实取回退出码与输出。
        $packCheck = Invoke-PackagedCheck -Runner (Join-Path $distRoot "game.exe") -PackRoot $packRoot
        Write-Host $packCheck.Output
        if ($packCheck.ExitCode -ne 0) {
            throw "Release package check failed (exit $($packCheck.ExitCode))."
        }
        $packRootsLine = $packCheck.Output | Where-Object { $_ -match "player_data_roots" } | Select-Object -First 1
        if (-not $packRootsLine) { throw "Release package check did not report player_data_roots." }
        if ($packRootsLine -notmatch "packaged=true") {
            throw "Release package did not report packaged=true: $packRootsLine"
        }
        Write-Host "Release package player data roots: $packRootsLine" -ForegroundColor DarkGray
    }

    # ---------------------------------------------------------------- 清单与说明
    $engineVersion = (Get-Content (Join-Path $engineRoot "Cargo.toml") | Select-String '^version = "0\.' | Select-Object -First 1).ToString().Trim().Split('=')[1].Trim().Trim('"')
    $toolchain = & rustc --version 2>$null
    cargo tree -e normal --prefix none 2>$null | Sort-Object -Unique | Set-Content (Join-Path $distRoot "deps.txt") -Encoding UTF8

    if ($Platform -eq "wechat-minigame") {
        # 发布根 = <PublishName>/，里面 launcher/ 是首包、assets/ 是 CDN 资源包（仅 remote 形态）；
        # $distRoot 始终指向 launcher/，所以体积统计/必需文件检查都只看首包。
        $packageContents = "launcher/: game.js, game.json, project.config.json, wechat-adapter.js, web-runtime.js, pkg/, pack/ (skeleton when thin), release.toml, run.txt, deps.txt"
        if ($settingsPackaged) {
            $packageContents = "launcher/: game.js, game.json, project.config.json, wechat-adapter.js, web-runtime.js, pkg/, pack/ (settings.toml), release.toml, run.txt, deps.txt"
        }
        if ($isSubpackagePack) {
            # 分包形态的主包 = launcher 自身：首屏资源在 mainpack/，其余在 pack/ 分包里。
            $packageContents = "launcher/: game.js, game.json (subpackages=[pack]), mainpack.json, mainpack/ (首屏资源), pack/ (分包 root，含 game.js 入口), project.config.json, wechat-adapter.js, web-runtime.js, pkg/, release.toml, run.txt, deps.txt"
        }
        if ($AssetPack -eq "remote") {
            $packageContents = "$packageContents; assets/pack/** = full resource tree for CDN (重资源走远端 $RemotePackBaseUrl)"
        }
        if (-not [string]::IsNullOrWhiteSpace($RemotePackBaseUrl)) {
            $packageContents = "remote-assets.json (远端回落), $packageContents"
        }
    }
    elseif ($Platform -eq "webgl") {
        $packageContents = "index.html, test-webgl.bat, pkg/game_web.js, pkg/game_web_bg.wasm, pkg/fengari-web.js, web-runtime.js, fonts/$fontName, pack/ (data), release.toml, run.txt, deps.txt"
        if ($settingsPackaged) {
            $packageContents = "index.html, test-webgl.bat, pkg/game_web.js, pkg/game_web_bg.wasm, pkg/fengari-web.js, web-runtime.js, fonts/$fontName, pack/ (data, settings.toml), release.toml, run.txt, deps.txt"
        }
    }
    else {
        # PC 不复制 17 MB 完整字体到发布根：assetc 已把工程字体打进 pack/（运行时以 pack 为工程根，
        # 按 settings.toml 的 font 路径读取），`fonts/` 那一份只有 webgl 的 index.html 需要。
        $packageContents = "game.exe, pack/ (data, assets/fonts/$fontName), release.toml, run.txt, deps.txt"
        if ($settingsPackaged) {
            $packageContents = "game.exe, pack/ (data, settings.toml, assets/fonts/$fontName), release.toml, run.txt, deps.txt"
        }
    }

    # 子集字体在包内的相对路径随形态变（分包形态下字体属于首屏资源，已被切到 mainpack/）。
    $wechatFontRelative = if ($isSubpackagePack) { "mainpack/assets/fonts/$fontName" } else { "pack/assets/fonts/$fontName" }
    @(
        "RustLua game engine release package",
        "publish_name: $PublishName",
        "build_time: $timestamp",
        "platform: $Platform",
        "engine version: $engineVersion",
        "toolchain: $toolchain",
        "contents: $packageContents",
        "font_check: $(if ($Platform -eq 'wechat-minigame') { "$wechatFontRelative (subset, packaged into launcher/)" } else { "fonts/$fontName present and non-empty" })",
        "settings_check: $(if ($settingsPackaged) { 'settings.toml packaged from project root（微信形态按白名单改名成 settings.toml.txt）' } else { 'no project settings.toml; engine defaults apply' })",
        "player_data: $(if ($Platform -eq 'wechat-minigame') { 'wx storage / USER_DATA_PATH; the code package is read-only' } elseif ($Platform -eq 'webgl') { 'browser storage; no filesystem player data' } else { "$PlayerDataDir (override: RUSTLUA_USER_DIR); pack/ is read-only and never written" })"
    ) | Set-Content (Join-Path $distRoot "build-info.txt") -Encoding UTF8

    if ($Platform -eq "wechat-minigame") {
        # 发布根下有几块随形态变：subpackage 只有 launcher/（首包 + 分包），remote 还多一个
        # assets/（上传 CDN 的资源包）。第 0 条必须按形态写清楚，否则使用者会把整个发布根传上去。
        $wechatRootLine = if ($isSubpackagePack) {
            "0) 发布根下只有 launcher/：它同时是**主包**与**分包**的宿主，整目录一次上传微信；不需要服务器、备案或 downloadFile 域名白名单"
        }
        else {
            "0) 发布根下有两块：launcher/ = 首包（上传微信 / 导入开发者工具的就是它），assets/ = 资源包（整目录上传 CDN）"
        }
        $wechatRunLines = @(
            "Run instructions (WeChat Mini Game)",
            $wechatRootLine,
            "1) 打开微信开发者工具 -> 导入项目 -> 选择本目录下的 launcher（已含 game.json / project.config.json）",
            "2) 在 launcher/project.config.json 里填入自己的 appid（默认 touristappid 仅用于本地预览）",
            "3) 手机预览：左半屏为虚拟摇杆（WASD），右半屏为动作键（space）；微信 PC 端同时支持键盘"
        )
        if ($isSubpackagePack) {
            $wechatRunLines += "4) launcher/pack/ 是**分包**（game.json 的 subpackages[0].root）：目录下所有文件都进分包，主包里一个都不留；root 下必须有 pack/game.js 入口，缺了开发者工具会拒绝打开工程"
            $wechatRunLines += "5) 主包侧的首屏资源在 launcher/mainpack/（与 pack/ 同级的兄弟目录），路由依据是 launcher/mainpack.json：清单内的走主包，其余走分包。启动先出标题画面，分包在后台 wx.loadSubpackage 下载（带进度条），下载一次永久缓存"
            $wechatRunLines += "6) 字体使用首屏子集字体（assetc 子集化产物，已随首屏切到 mainpack/assets/fonts/），完整字体不随包分发以控制体积"
            $wechatRunLines += "7) 首包里不含 PC/WebGL 入口（game.exe / index.html），不要混用发布目录"
            $wechatRunLines += "8) 首次导入开发者工具后**必须**核对「详情 -> 代码包大小」面板：主包 / 分包两块数值应与本次输出行的 main_kb / subpackage_kb 一致（本机 harness 无法证明微信的真实打包语义，这里是唯一能证伪的地方）"
        }
        else {
            $wechatRunLines += "4) launcher/pack/ 不是分包（game.json 不含 subpackages）：它是主包内的普通目录，运行时直接读取，不需要 wx.loadSubpackage，也没有分包入口文件"
            $wechatRunLines += "5) 字体使用 pack/ 内的子集字体（assetc 子集化产物），完整字体不随包分发以控制体积"
            $wechatRunLines += "6) 首包里不含 PC/WebGL 入口（game.exe / index.html），不要混用发布目录"
        }
        # 这里必须写**最终**目录（$finalRoot）：run.txt 是在 staging 里生成的，
        # 而原子交换会把 staging 改名/删除，写 staging 路径等于写一个随后就不存在的目录。
        $wechatRunLines += "9) 无开发者工具时的运行时验证：powershell -NoProfile -File scripts\verify-wechat-minigame.ps1 -BundleRoot `"$finalRoot\launcher`""
        if ($AssetPack -eq "remote") {
            $wechatRunLines += "10) 本包是**瘦首包**：launcher/pack/ 只含骨架（manifest.txt/chars.txt 原名，project.toml/settings.toml 因后缀不在微信上传白名单而改名为 *.toml.txt），重资源从 $RemotePackBaseUrl 下载，按 pack/manifest.txt 校验后缓存到 USER_DATA_PATH"
            $wechatRunLines += "11) 上传顺序：先把 assets/ 整份传到 CDN（使 packBaseUrl 可用），再上传 launcher/ 作为代码包；两侧不一致时校验和会拒绝下载（不会静默用错资源）"
            $wechatRunLines += "12) 运行时验证要带上资源包：scripts\verify-wechat-minigame.ps1 -BundleRoot `"$finalRoot\launcher`" -AssetPackRoot `"$finalRoot\assets`""
            $wechatRunLines += "13) 本地预览（不用自己起服务）：双击 scripts\serve-wechat-pack.bat，再按上面的方式导入 launcher/"
        }
        $wechatRunLines | Set-Content (Join-Path $distRoot "run.txt") -Encoding UTF8
    }
    elseif ($Platform -eq "webgl") {
        @(
            "Run instructions",
            "1) Serve this directory with an HTTP server, for example: python -m http.server 8000",
            "2) Open http://localhost:8000/index.html in a WebGL2-capable browser",
            "3) Do not open index.html directly from file:// because browsers block WASM/resource fetches",
            "4) The page title and browser tab title use publish_name from release.toml",
            "5) Windows: double-click test-webgl.bat to verify the bundle and serve it on http://127.0.0.1:8137/",
            "   (it finds the repository root by itself; outside a checkout, serve the directory manually as in 1)"
        ) | Set-Content (Join-Path $distRoot "run.txt") -Encoding UTF8
    }
    else {
        @(
            "Run instructions",
            "1) Double-click game.exe, or run: game.exe --pack pack --title `"$PublishName`"",
            "2) Controls: WASD move, SPACE attack, F1 save, F2 load, F4 switch scene",
            "3) F3 toggles the developer fps/latency overlay (off by default in release builds)",
            "4) Logs and crash reports go to the player data directory: $PlayerDataDir\logs",
            "   (a release pack never writes into its own directory, so a read-only install is fine)",
            "5) This package has no source or toolchain dependency and runs without Rust/IDE",
            "6) Player data (savegames and settings) is written to: $PlayerDataDir",
            "   The pack/ directory is read-only for the runner and never receives player data;",
            "   set RUSTLUA_USER_DIR to relocate the player data directory (used by tests and portable installs)"
        ) | Set-Content (Join-Path $distRoot "run.txt") -Encoding UTF8
    }

    # 微信小游戏：收尾处（run.txt/deps.txt/build-info.txt 都写完之后）重新测量并输出最终体积。
    # 这一步不能省：微信上传时这些说明文件也计入主包，早期只在生成它们之前测过一次，
    # 会把主包少算 ~9 KB（实测 main_kb=303 而最终目录是 312）——守卫必须盯着最终产物。
    if ($Platform -eq "wechat-minigame") {
        $wechatFinalTotal = (Get-ChildItem -LiteralPath $distRoot -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
        $wechatFinalPack = (Get-ChildItem -LiteralPath $packRoot -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
        $wechatFinalVolume = Assert-WeChatPackageVolume -TotalBytes $wechatFinalTotal -PackBytes $wechatFinalPack `
            -Form $AssetPack -Suffix "（本值是权威值：run.txt/deps.txt/build-info.txt 都已写入发布目录）"
        Write-Host ("wechat_minigame_package_ok files={0} main_kb={1:N0} main_of_limit={2:P1} subpackage_kb={3:N0} subpackage_of_limit={4:P1} pack_kb={5:N0} total_kb={6:N0} total_of_limit={7:P1} asset_pack={8} split={9}" -f `
            $requiredWeChatFiles.Count, ($wechatFinalVolume.Main / 1KB), ($wechatFinalVolume.Main / 4MB), `
            $(if ($isSubpackagePack) { $wechatFinalVolume.Pack / 1KB } else { 0 }), `
            $(if ($isSubpackagePack) { $wechatFinalVolume.Pack / 4MB } else { 0 }), `
            ($wechatFinalPack / 1KB), ($wechatFinalVolume.Total / 1KB), ($wechatFinalVolume.Total / 20MB), $AssetPack, `
            $(if ($isSubpackagePack) { "mainpack+pack" } else { "none" })) -ForegroundColor Green
    }

    # M20-03：全部内容与自检通过后，原子替换最终目录（旧版本备份→移入新版本
    # →清理备份）。交换失败会回滚旧版本并抛错，staging 保留以便诊断。
    Write-Host "==> Atomic release swap" -ForegroundColor Cyan
    $script:swapAttempted = $true
    $swap = Complete-ReleaseSwap -FinalRoot $finalRoot -StageRoot $stageRoot
    # $distRoot 回到最终目录下的 launcher（微信）/ 发布根（其它平台）：launcher/ 与 assets/ 是
    # 同一次交换落地的，所以此刻首包与资源包一定互相配套。
    $distRoot = if ($launcherSubdir) { Join-Path $finalRoot $launcherSubdir } else { $finalRoot }
    $packRoot = Join-Path $distRoot "pack"
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
        if (-not (Test-Path -LiteralPath (Join-Path $cdnPackRoot "pack"))) {
            throw "资源包导出失败: $cdnPackRoot\pack 不存在（首包与资源包必须在同一次发布里产出）"
        }
        Write-Host ("wechat_asset_pack_ok dir={0} files={1} pack_kb={2:N0} cdn_base={3}" -f `
            $cdnPackRoot, $script:cdnPackFullFiles, ($script:cdnPackFullBytes / 1KB), $RemotePackBaseUrl) -ForegroundColor Green
        Write-Host ("==> 资源包已就绪：把 {0} 整个目录上传到 CDN，使 packBaseUrl 指向 {1}" -f $cdnPackRoot, $RemotePackBaseUrl) -ForegroundColor Cyan
    }

    Write-Host "Release package ready: $finalRoot" -ForegroundColor Green
    $total = Get-ChildItem -Recurse -File $distRoot | Measure-Object -Property Length -Sum
    Write-Host ("Package size: {0:N1} KB, {1} files ({2})" -f ($total.Sum / 1KB), $total.Count, $(if ($launcherSubdir) { "launcher/, 上传微信的代码包" } else { "release root" }))
}
catch {
    # 交换之前失败：最终目录从未被改动，直接丢弃 staging，并明确告知旧版本仍在。
    if (-not $script:swapAttempted -and (Test-Path -LiteralPath $stageRoot)) {
        Write-Warning "==> Release failed before swap; existing release kept at $finalRoot"
        Write-Warning "==> Discarding staging: $stageRoot"
        Undo-ReleaseStaging -StageRoot $stageRoot | Out-Null
    }
    elseif ($script:swapAttempted -and (Test-Path -LiteralPath $stageRoot)) {
        Write-Warning "==> Swap failed; old release restored. Staging kept for diagnosis: $stageRoot"
    }
    # -CdnPackDir 镜像的 staging 建在发布根之外，不清理就会在镜像目录里留下 .staging-*。
    if ($script:cdnMirrorStage -and (Test-Path -LiteralPath $script:cdnMirrorStage)) {
        Write-Warning "==> Discarding asset-pack mirror staging: $($script:cdnMirrorStage)"
        Undo-ReleaseStaging -StageRoot $script:cdnMirrorStage | Out-Null
    }
    throw
}
finally {
    Pop-Location
}

# M20-03: best-effort removal of the isolated test copy. Windows may still hold
# the freshly launched runner (AV/indexer), so retry briefly and never fail the
# publish because of cleanup; leftovers are reported with their path.
function Remove-IsolatedRelease([string]$Path, [string]$Label) {
    for ($attempt = 1; $attempt -le 5; $attempt++) {
        if (-not (Test-Path -LiteralPath $Path)) { return }
        try {
            Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction Stop
            return
        }
        catch {
            Start-Sleep -Milliseconds (200 * $attempt)
        }
    }
    Write-Warning "==> Leftover $Label not removed (file in use); remove manually: $Path"
}

if ($SelfTest) {
    Write-Host "`n==> Directory-isolation self test (no source/target/.cache, path with spaces)" -ForegroundColor Cyan
    $isolated = Join-Path $env:TEMP ("p2 release test " + (Get-Date -Format "HHmmss"))
    New-Item -ItemType Directory -Force -Path $isolated | Out-Null
    # 复制**发布根**（不是 $distRoot）：微信发布根下是 launcher/ + assets/ 两个目录
    # （分包形态下只有 launcher/，主包与分包都在它里面），隔离自测要连资源包一起搬走，
    # 才能验证"首包与资源包配套"。其它平台 $finalRoot 就是 $distRoot，行为不变。
    Copy-Item -LiteralPath $finalRoot -Destination $isolated -Recurse -Force
    if ($Platform -eq "wechat-minigame") {
        # 小游戏是静态包：断言隔离目录里运行时与配置齐全，并用无头 harness 实跑一次。
        # 发布根下有 launcher/（首包）与 assets/（资源包）两个兄弟目录，验证脚本吃的是这两个。
        $isolatedRoot = Join-Path (Join-Path $isolated $PublishName) "launcher"
        $isolatedSkeletonRoot = if ($isSubpackagePack) { Join-Path $isolatedRoot "mainpack" } else { Join-Path $isolatedRoot "pack" }
        foreach ($file in @(
            (Join-Path $isolatedRoot "game.js"),
            (Join-Path $isolatedRoot "game.json"),
            (Join-Path $isolatedRoot "project.config.json"),
            (Join-Path $isolatedRoot "wechat-adapter.js"),
            (Join-Path $isolatedRoot "web-runtime.js"),
            (Join-Path $isolatedRoot "pkg\fengari-web.js"),
            (Join-Path $isolatedSkeletonRoot "manifest.txt")
        )) {
            if (-not (Test-Path -LiteralPath $file)) { throw "isolated WeChat launcher is missing: $file" }
        }
        if ($isSubpackagePack) {
            # 切分真实性：逐条读隔离副本里的 mainpack.json，每条都必须在 mainpack/ 下**存在**、
            # 且在 pack/ 下**不存在**。这条断言是"复制到隔离目录的过程把形态搞坏了"唯一的哨兵；
            # 主包/分包两侧同时留同一份文件会让读取路径变得不确定（谁先命中谁生效）。
            $isolatedManifestPath = Join-Path $isolatedRoot "mainpack.json"
            if (-not (Test-Path -LiteralPath $isolatedManifestPath)) {
                throw "isolated subpackage launcher is missing mainpack.json"
            }
            if (-not (Test-Path -LiteralPath (Join-Path $isolatedRoot "pack\game.js"))) {
                throw "isolated subpackage launcher is missing the subpackage entry: pack\game.js"
            }
            # 隔离副本里也要再读一次改名映射（它跟着产物一起复制过来，是运行时的查询依据）。
            $isolatedMapPath = Join-Path $isolatedRoot "code-package.json"
            if (-not (Test-Path -LiteralPath $isolatedMapPath)) {
                throw "isolated WeChat launcher is missing code-package.json（运行时的改名查询依据）"
            }
            $isolatedRenamed = @{}
            $isolatedMap = Get-Content -LiteralPath $isolatedMapPath -Raw -Encoding UTF8 | ConvertFrom-Json
            if ($isolatedMap.renamed) {
                foreach ($property in $isolatedMap.renamed.PSObject.Properties) { $isolatedRenamed[$property.Name] = [string]$property.Value }
            }
            $isolatedEntries = @((Get-Content -LiteralPath $isolatedManifestPath -Raw -Encoding UTF8 | ConvertFrom-Json).entries)
            if ($isolatedEntries.Count -eq 0) { throw "isolated mainpack.json has an empty entries list" }
            $misplaced = @()
            foreach ($entry in $isolatedEntries) {
                $hostName = if ($isolatedRenamed.ContainsKey($entry)) { $isolatedRenamed[$entry] } else { $entry }
                $relative = $hostName -replace '/', '\'
                if (-not (Test-Path -LiteralPath (Join-Path $isolatedRoot ("mainpack\" + $relative)))) { $misplaced += "missing in mainpack/: $hostName" }
                if (Test-Path -LiteralPath (Join-Path $isolatedRoot ("pack\" + $relative))) { $misplaced += "still in pack/: $hostName" }
            }
            if ($misplaced.Count -ne 0) {
                throw ("isolated subpackage split is not authentic ({0} problems):`n  - {1}" -f $misplaced.Count, ($misplaced -join "`n  - "))
            }
            # 分包形态必须没有远端配置：有就意味着适配层会走 CDN 回落，掩盖"分包没挂载上"。
            if (Test-Path -LiteralPath (Join-Path $isolatedRoot "remote-assets.json")) {
                throw "isolated subpackage launcher must not contain remote-assets.json"
            }
            Write-Host ("Check: wechat_minigame_split_ok mainpack_files={0}（隔离副本里逐条复核）" -f $isolatedEntries.Count) -ForegroundColor DarkGray
        }
        $isolatedAssets = Join-Path (Join-Path $isolated $PublishName) "assets"
        if ($AssetPack -eq "remote") {
            # 瘦首包必须在隔离目录里也是"瘦"的，否则本地自测等于在测一个不存在的形态。
            foreach ($heavy in @("pack\assets", "pack\scenes", "pack\scripts")) {
                if (Test-Path -LiteralPath (Join-Path $isolatedRoot $heavy)) {
                    throw "isolated thin WeChat pack still contains $heavy (isolation copy is not thin)"
                }
            }
            if (-not (Test-Path -LiteralPath (Join-Path $isolatedRoot "remote-assets.json"))) {
                throw "isolated thin WeChat package is missing remote-assets.json"
            }
            if (-not (Test-Path -LiteralPath (Join-Path $isolatedAssets "pack\manifest.txt"))) {
                throw "isolated release is missing assets/pack/** (首包与资源包必须一起落地)"
            }
        }
        $verifier = Join-Path $PSScriptRoot "verify-wechat-minigame.ps1"
        if ((Get-Command node -ErrorAction SilentlyContinue) -and (Test-Path -LiteralPath $verifier)) {
            # 瘦首包的重资源在 assets/ 里：自测必须把它当"CDN"喂给验证脚本，
            # 否则隔离目录里只剩骨架、运行时验证必然读不到资源。
            $verifierArgs = @("-NoProfile", "-File", $verifier, "-BundleRoot", $isolatedRoot)
            if ($AssetPack -eq "remote" -and (Test-Path -LiteralPath (Join-Path $isolatedAssets "pack"))) {
                $verifierArgs += @("-AssetPackRoot", $isolatedAssets)
                Write-Host "Check: powershell -File scripts\verify-wechat-minigame.ps1 -BundleRoot $isolatedRoot -AssetPackRoot $isolatedAssets" -ForegroundColor DarkGray
            }
            else {
                Write-Host "Check: powershell -File scripts\verify-wechat-minigame.ps1 -BundleRoot $isolatedRoot" -ForegroundColor DarkGray
            }
            & powershell @verifierArgs
            if ($LASTEXITCODE -ne 0) { throw "isolated WeChat runtime verification failed (exit $LASTEXITCODE)" }
        }
        else {
            Write-Warning "==> node or verifier missing; skipped isolated WeChat runtime check"
        }
        Remove-IsolatedRelease -Path $isolated -Label "WeChat self-test copy"
        Write-Host ("Directory-isolation self test passed (WeChat launcher + runtime OK; asset_pack={0})" -f $AssetPack) -ForegroundColor Green
    }
    elseif ($Platform -eq "webgl") {
        $isolatedRoot = Join-Path $isolated $PublishName
        foreach ($file in @(
            (Join-Path $isolatedRoot "index.html"),
            (Join-Path $isolatedRoot "web-runtime.js"),
            (Join-Path $isolatedRoot "pkg\game_web.js"),
            (Join-Path $isolatedRoot "pkg\game_web_bg.wasm"),
            (Join-Path $isolatedRoot "pack\manifest.txt")
        )) {
            if (-not (Test-Path $file)) { throw "isolated WebGL dir is missing: $file" }
        }
        Remove-IsolatedRelease -Path $isolated -Label "WebGL self-test copy"
        Write-Host "Directory-isolation self test passed (WebGL static package OK)" -ForegroundColor Green
    }
    else {
        $isolatedRunner = Join-Path $isolated ("$PublishName\game.exe")
        $isolatedPack = Join-Path $isolated ("$PublishName\pack")
        if (-not (Test-Path $isolatedRunner)) { throw "isolated dir is missing the runner" }
        $isolatedFont = Join-Path $isolated ("$PublishName\pack\assets\fonts\$fontName")
        if (-not (Test-Path -LiteralPath $isolatedFont) -or (Get-Item -LiteralPath $isolatedFont).Length -le 0) {
            # PC 的字体在 pack/ 里（assetc 打包产物）；发布根下的 fonts/ 只属于 webgl 的 index.html。
            throw "isolated dir is missing the packaged font: pack/assets/fonts/$fontName"
        }

        Write-Host "Check: $isolatedRunner --pack $isolatedPack --check" -ForegroundColor DarkGray
        # 用 --check 做无窗口校验：退出码 0 表示发布包完整（含空格路径 + 无源码环境）。
        # M31/P3：同时把玩家数据根指到一个临时目录，断言发布包形态确实解析到**用户数据目录**
        # （而不是安装目录），并断言安装目录里不出现 .cache/save/settings 等玩家数据。
        $isolatedUserDir = Join-Path $env:TEMP ("p2 user data test " + (Get-Date -Format "HHmmss"))
        New-Item -ItemType Directory -Force -Path $isolatedUserDir | Out-Null
        $env:RUSTLUA_USER_DIR = $isolatedUserDir
        try {
            $isolatedCheck = Invoke-PackagedCheck -Runner $isolatedRunner -PackRoot $isolatedPack
            Write-Host $isolatedCheck.Output
            if ($isolatedCheck.ExitCode -ne 0) {
                throw "isolated check failed (exit $($isolatedCheck.ExitCode)): package incomplete or path incompatible."
            }
            $rootsLine = $isolatedCheck.Output | Where-Object { $_ -match "player_data_roots" } | Select-Object -First 1
            if (-not $rootsLine) { throw "isolated check did not report player_data_roots." }
            Write-Host "Player data roots: $rootsLine" -ForegroundColor DarkGray
            if ($rootsLine -notmatch "packaged=true") {
                throw "release package did not report packaged=true: $rootsLine"
            }
            if ($rootsLine -notmatch [regex]::Escape($isolatedUserDir)) {
                throw "release package ignored RUSTLUA_USER_DIR=$isolatedUserDir : $rootsLine"
            }
            $strayPlayerData = Get-ChildItem -LiteralPath (Join-Path $isolated $PublishName) -Recurse -Force -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -in @(".cache", ".data") -or $_.Extension -in @(".sav", ".log") }
            if ($strayPlayerData) {
                throw "release directory contains player data: $($strayPlayerData.FullName -join ', ')"
            }
        }
        finally {
            Remove-Item Env:\RUSTLUA_USER_DIR -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath $isolatedUserDir -Recurse -Force -ErrorAction SilentlyContinue
        }
        Remove-IsolatedRelease -Path $isolated -Label "self-test copy"
        Write-Host "Directory-isolation self test passed (spaces path + no-source check + user data dir OK)" -ForegroundColor Green
    }
}
