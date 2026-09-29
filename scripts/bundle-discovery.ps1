# bundle-discovery.ps1：定位「某个游戏工程的某个平台的发布产物目录」。
#
# 为什么需要它：发布产物落在 `examples\<工程>\builds\<平台>\`（2026-09-28 起不再按 publish_name
# 建子目录；旧布局 `<publish_name>\` 子目录仍被兼容识别）。验收/试玩脚本此前把
# `examples\game_jhyt\builds\webgl\jianghu_youtian` 写死成默认值——换一个游戏工程、
# 换一个 publish_name 就失效。这里把「一个合格的发布包长什么样」收成一处，供
# verify-webgl-bundle.ps1 / verify-wechat-minigame.ps1 / test-bundle.ps1 共同点源。
#
# 注意：本文件**不**启用 Set-StrictMode——它会被点源，不应改变调用方的变量解析语义
# （同 release-atomic.ps1 的既有约定）。
#
# 用法：
#   . scripts\bundle-discovery.ps1
#   $root = Resolve-GameBundle -Platform webgl        # 自动发现；找不到/有歧义时返回 $null 并打印诊断
#   $report = Find-GameBundle -Platform webgl -All    # 纯数据：候选清单（新到旧）+ 被排除项与原因

$script:StagingPrefix = ".staging-"
$script:BackupPrefix = ".backup-"

function Get-BundleMarkerFiles {
    <#
    .SYNOPSIS
    某平台发布产物必须齐备的条目清单（相对产物根）。
    .DESCRIPTION
    这是一条「这个包跑得起来吗」的判据，必须与 scripts/package.ps1 的产物校验数组按**同一口径**
    对齐（webgl: $requiredWebFiles；微信小游戏: $requiredWeChatFiles），两处任一改动另一处必须同步：
    一旦「发布脚本拒收、发现器却接受」，被当成有效产物的包就会让验收报出假的通过。

    唯一的**有意**差异：清单只收「缺了就跑不起来/验收不了」的条目。像 webgl 的包内双击壳
    `test-webgl.bat` 属于便利项（缺了不影响产物能否运行与验收），因此发布时 `package.ps1` 严格
    要求它、而本清单不要求它 —— 否则今天之前发布过的、仍然完全可验收的包会被发现器挡掉。
    微信清单在 M32 起**重新要求** `pkg\game_web.js` / `pkg\game_web_bg.wasm`：该平台曾一度
    改走"纯 JS + Fengari"（不发行 wasm），M32 起改为随包发行 wasm 核心并在启动时同步实例化
    （见 package.ps1 的微信必需清单与 plugins/wechat-minigame/README.md 的「wasm 核心」）。
    两处清单必须同步——任一处漏改，就会出现"发布脚本拒收、发现器却接受"的假通过。

    微信的平台标记仍是 **8 项**，但其中骨架两项（manifest.txt / project.toml）的**位置随形态变**：
    分包形态（-AssetPack subpackage）下它们是首屏资源，构建期已被搬到 mainpack/，pack/ 里
    按设计就不该有——把它们写死成 `pack\` 会把唯一一种主包可直接启动的产物挡在发现之外。
    所以这两项由 -ArtifactRoot 现场解析（判据仍是"这个文件存在且非空"，强度不变）。
    #>
    param(
        [Parameter(Mandatory = $true)][string]$Platform,
        # 产物根：给了才能解析随形态变化的条目（目前只有微信的骨架两项）。不给则按旧形态（pack\）判。
        [string]$ArtifactRoot = ""
    )

    switch ($Platform) {
        "webgl" {
            # 关键一项是 pkg\game_web_bg.wasm：WP10 迁移后 Lua 执行层就在 wasm 里
            # （mlua 移植，随包发行），少了它游戏在浏览器里根本进不了 Lua——必须靠这一项
            # 把旧产物（2026-09-29 之前的 fengari 时代包）挡在发现之外，否则
            # 「index.html + wasm 都在」会让一个跑不起来的包被选中。
            return @(
                "index.html",
                "web-runtime.js",
                "pkg\game_web.js",
                "pkg\game_web_bg.wasm",
                "pack\manifest.txt",
                "pack\project.toml"
            )
        }
        "wechat-minigame" {
            # 小游戏：入口 game.js + wechat-adapter.js + **wasm 核心**（WP10 迁移后即 Lua
            # 执行层，mlua 移植随包发行）。
            # wasm 两项与 webgl 同名同义（`--target no-modules` 的 CommonJS 化胶水 + 模块本体），
            # 少任何一个都说明产物与当前发布形态不一致——这也是"发现器只收非它不可的条目"口径下的
            # 必需项：适配层启动时会读它并实例化，缺了 Lua 根本跑不起来。
            # 前六项在三种资源形态（full / thin / subpackage）下位置都相同，是稳定的平台标记；
            # 骨架两项的**位置**随形态变（有 mainpack.json 就是分包形态，骨架在 mainpack/，否则在 pack/），
            # **文件名**也可能被改名：微信代码包只上传后缀在白名单内的文件，构建期把 `.toml` 之类
            # 改成 `*.toml.txt`。名字从产物根的 code-package.json 查表得到——不在这儿重算规则，
            # 否则改不改名、查哪张表就会有两处说法（发现器"找不到产物"这种误报最难查）。
            $skeleton = "pack"
            if (-not [string]::IsNullOrWhiteSpace($ArtifactRoot) -and (Test-Path -LiteralPath (Join-Path $ArtifactRoot "mainpack.json"))) {
                $skeleton = "mainpack"
            }
            $resolve = { param([string]$logical) $logical }
            if (-not [string]::IsNullOrWhiteSpace($ArtifactRoot)) {
                $mapPath = Join-Path $ArtifactRoot "code-package.json"
                if (Test-Path -LiteralPath $mapPath) {
                    $renamed = @{}
                    try {
                        $mapObject = Get-Content -LiteralPath $mapPath -Raw -Encoding UTF8 | ConvertFrom-Json
                        if ($mapObject.renamed) {
                            foreach ($property in $mapObject.renamed.PSObject.Properties) { $renamed[$property.Name] = [string]$property.Value }
                        }
                    }
                    catch { $renamed = @{} }
                    $captured = $renamed
                    $resolve = { param([string]$logical) if ($captured.ContainsKey($logical)) { return $captured[$logical] } return $logical }
                }
            }
            return @(
                "game.js",
                "game.json",
                "project.config.json",
                "wechat-adapter.js",
                "web-runtime.js",
                "pkg\game_web.js",
                "pkg\game_web_bg.wasm",
                ($skeleton + "\" + (& $resolve "manifest.txt")),
                ($skeleton + "\" + (& $resolve "project.toml"))
            )
        }
        default {
            throw "bundle-discovery: 未知平台 '$Platform'（可选 webgl / wechat-minigame）"
        }
    }
}

function Resolve-BundleArtifactRoot {
    <#
    .SYNOPSIS
    从发布根解析出「平台验收脚本要吃的那个目录」。
    微信小游戏的发布根下是 launcher/（首包）+ assets/（CDN 资源包）两个兄弟目录，
    验收脚本吃的是 launcher/；其它平台产物就在发布根下，原样返回。
    #>
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Platform
    )

    if ($Platform -eq "wechat-minigame") {
        $launcher = Join-Path $Path "launcher"
        if (Test-Path -LiteralPath (Join-Path $launcher "game.js")) { return $launcher }
    }
    return $Path
}

function Get-BundleRejectReason {
    <#
    .SYNOPSIS
    目录不合格的原因（第一个缺失或为空的条目），合格时返回 $null。
    #>
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Platform
    )

    $artifactRoot = Resolve-BundleArtifactRoot -Path $Path -Platform $Platform
    foreach ($item in (Get-BundleMarkerFiles -Platform $Platform -ArtifactRoot $artifactRoot)) {
        $file = Join-Path $artifactRoot $item
        if (-not (Test-Path -LiteralPath $file)) { return "缺少 $item" }
        $entry = Get-Item -LiteralPath $file -Force
        if ($entry.PSIsContainer) { return "$item 是目录，应为文件" }
        if ($entry.Length -le 0) { return "$item 为空" }
    }
    return $null
}

function Get-BundleBuildInfo {
    <#
    .SYNOPSIS
    读产物自述文件 build-info.txt 里的身份与发布时刻。
    .DESCRIPTION
    排序**不能**用文件系统时间：Copy-Item 会保留源文件的 LastWriteTime（产物里的
    web-runtime.js 时间戳来自 plugins\webgl\web-runtime.js，不是发布时间），而发布目录自身的
    时间戳会被「创建/删除内部 .staging-* 子目录」刷掉。build-info.txt 的 build_time 是
    package.ps1 每次发布显式写下的发布时刻，是这里唯一可信的时钟。
    缺该文件时返回 $null 字段，由调用方回落。
    #>
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [string]$Platform = ""
    )

    $result = [pscustomobject]@{ PublishName = $null; BuildTime = $null; Platform = $null }
    # 微信小游戏的 build-info.txt 在 launcher/ 里（发布根下是 launcher/ + assets/）。
    $infoRoot = if ($Platform) { Resolve-BundleArtifactRoot -Path $Path -Platform $Platform } else { $Path }
    $infoPath = Join-Path $infoRoot "build-info.txt"
    if (-not (Test-Path -LiteralPath $infoPath)) { return $result }
    # 显式 -Encoding UTF8：Set-Content -Encoding UTF8 在 PS 5.1 下会写 BOM，不指定则按 ANSI
    # 解码（中文 publish_name 会读坏，见 package.ps1 对 project.config.json 的同类注释）。
    foreach ($line in (Get-Content -LiteralPath $infoPath -Encoding UTF8)) {
        if ($line -match '^\s*publish_name:\s*(.+?)\s*$') { $result.PublishName = $Matches[1]; continue }
        if ($line -match '^\s*platform:\s*(.+?)\s*$') { $result.Platform = $Matches[1]; continue }
        if ($line -match '^\s*build_time:\s*(.+?)\s*$') {
            $parsed = [datetime]::MinValue
            if ([datetime]::TryParse($Matches[1], [ref]$parsed)) { $result.BuildTime = $parsed }
            continue
        }
    }
    return $result
}

function Get-DisplayPath {
    <#
    .SYNOPSIS
    把绝对路径折成相对 workspace 的短路径，便于日志与报错；不在其下则原样返回。
    #>
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$WorkspaceRoot
    )

    if ($Path.StartsWith($WorkspaceRoot, [StringComparison]::OrdinalIgnoreCase)) {
        return $Path.Substring($WorkspaceRoot.Length).TrimStart("\")
    }
    return $Path
}

function Find-GameBundle {
    <#
    .SYNOPSIS
    扫描 examples 下的发布产物，返回纯数据报告（不做策略选择、不抛错）。
    .OUTPUTS
    [pscustomobject]@{ Platform; WorkspaceRoot; ScanRoots; Candidates; Rejected }
      Candidates 每项：@{ Path; PublishName; BuildTime; Modified }
      已按 build_time（缺则回落目录时间）新到旧排序；Rejected 每项为 "相对路径  (原因)" 字符串。
    #>
    param(
        [Parameter(Mandatory = $true)][string]$Platform,
        [string]$WorkspaceRoot = "",
        [string]$ProjectRoot = ""
    )

    if ([string]::IsNullOrWhiteSpace($WorkspaceRoot)) { $WorkspaceRoot = Split-Path -Parent $PSScriptRoot }
    $WorkspaceRoot = (Resolve-Path -LiteralPath $WorkspaceRoot).Path

    $projectDirs = @()
    if (-not [string]::IsNullOrWhiteSpace($ProjectRoot)) {
        $resolved = Resolve-Path -LiteralPath $ProjectRoot -ErrorAction SilentlyContinue
        if ($resolved) { $projectDirs = @($resolved.Path) }
    }
    else {
        $examplesRoot = Join-Path $WorkspaceRoot "examples"
        if (Test-Path -LiteralPath $examplesRoot) {
            $projectDirs = @(Get-ChildItem -LiteralPath $examplesRoot -Directory -ErrorAction SilentlyContinue |
                ForEach-Object { $_.FullName })
        }
    }

    $scanRoots = New-Object System.Collections.Generic.List[string]
    $candidates = New-Object System.Collections.Generic.List[object]
    $rejected = New-Object System.Collections.Generic.List[string]

    foreach ($project in $projectDirs) {
        $platformRoot = Join-Path $project ("builds" + [IO.Path]::DirectorySeparatorChar + $Platform)
        if (-not (Test-Path -LiteralPath $platformRoot)) { continue }
        $scanRoots.Add($platformRoot)

        # 新布局（2026-09-28 起）：产物直接落在 `builds\<平台>\`，平台根自身就是发布根。
        # 平台根合格时只认它（微信的 launcher/ 是其内部结构，不能再当独立候选数一遍）。
        $rootReason = Get-BundleRejectReason -Path $platformRoot -Platform $Platform
        if (-not $rootReason) {
            $info = Get-BundleBuildInfo -Path $platformRoot -Platform $Platform
            $candidates.Add([pscustomobject]@{
                    # Path = 验收脚本要吃的目录（微信是发布根下的 launcher/），ReleaseRoot = 发布根。
                    Path        = (Resolve-BundleArtifactRoot -Path $platformRoot -Platform $Platform)
                    ReleaseRoot = $platformRoot
                    PublishName = $info.PublishName
                    BuildTime   = $info.BuildTime
                    Modified    = (Get-Item -LiteralPath $platformRoot -Force).LastWriteTime
                })
            continue
        }

        # 旧布局（`<publish_name>\` 子目录）兼容：平台根自身不合格时，枚举子目录候选。
        # -Force 让隐藏项也进入枚举：staging/backup 目录带前导点。
        foreach ($candidate in (Get-ChildItem -LiteralPath $platformRoot -Directory -Force -ErrorAction SilentlyContinue)) {
            # 只跳过「名字本身就是 staging/backup」的候选。注意 staging 是建在发布目录**内部**的
            # （release-atomic.ps1 用 -FinalRoot = 发布目录），所以一个带着上次中断留下的
            # .staging-* 子目录的发布目录仍然是有效产物——不能因为它有这种子目录就否掉它。
            if ($candidate.Name.StartsWith($script:StagingPrefix) -or $candidate.Name.StartsWith($script:BackupPrefix)) { continue }

            $reason = Get-BundleRejectReason -Path $candidate.FullName -Platform $Platform
            if ($reason) {
                $rejected.Add(("{0}  ({1})" -f (Get-DisplayPath -Path $candidate.FullName -WorkspaceRoot $WorkspaceRoot), $reason))
                continue
            }

            $info = Get-BundleBuildInfo -Path $candidate.FullName -Platform $Platform
            $candidates.Add([pscustomobject]@{
                    # Path = 验收脚本要吃的目录（微信是发布根下的 launcher/），ReleaseRoot = 发布根。
                    Path        = (Resolve-BundleArtifactRoot -Path $candidate.FullName -Platform $Platform)
                    ReleaseRoot = $candidate.FullName
                    PublishName = $info.PublishName
                    BuildTime   = $info.BuildTime
                    Modified    = $candidate.LastWriteTime
                })
        }
    }

    # 新到旧：显式发布时刻优先，缺失时用目录时间兜底（绝不用裸文件时间——见 Get-BundleBuildInfo）。
    $ordered = @($candidates |
            Sort-Object -Property `
                @{ Expression = { if ($_.BuildTime) { $_.BuildTime } else { [datetime]::MinValue } }; Descending = $true }, `
                @{ Expression = { $_.Modified }; Descending = $true })

    return [pscustomobject]@{
        Platform      = $Platform
        WorkspaceRoot = $WorkspaceRoot
        ScanRoots     = @($scanRoots)
        Candidates    = $ordered
        Rejected      = @($rejected)
    }
}

function Write-BundleReport {
    <#
    .SYNOPSIS
    打印「没有可用产物 / 有歧义」的诊断：候选、被排除项与原因、以及下一步该敲什么。
    #>
    param(
        [Parameter(Mandatory = $true)]$Report,
        [string]$Hint = ""
    )

    $required = (Get-BundleMarkerFiles -Platform $Report.Platform) -join ", "
    # 微信骨架两项的位置随资源形态变（分包形态在 mainpack/，其余在 pack/）：诊断里必须说清
    # "两种位置都算合格"，否则会把人引向"以为产物缺骨架"的错误排查方向。
    if ($Report.Platform -eq "wechat-minigame") {
        $required += "（其中 manifest.txt / project.toml 在 pack\ 或 mainpack\ 其一）"
    }
    Write-Host ("[RED] 未定位到可用的 {0} 发布产物（已扫描 {1} 处 builds\{0}）" -f $Report.Platform, $Report.ScanRoots.Count)
    Write-Host "  合格产物必须齐备：$required"
    if (-not [string]::IsNullOrWhiteSpace($Hint)) { Write-Host "  $Hint" }
    if ($Report.Candidates.Count -gt 0) {
        Write-Host "  候选（新到旧，需用 -BundleRoot 显式指定）："
        foreach ($candidate in $Report.Candidates) {
            $when = if ($candidate.BuildTime) { $candidate.BuildTime.ToString("yyyy-MM-dd HH:mm:ss") } else { "未知发布时刻" }
            $name = if ($candidate.PublishName) { $candidate.PublishName } else { Split-Path -Leaf $candidate.Path }
            Write-Host ("    {0}  [{1}, {2}]" -f (Get-DisplayPath -Path $candidate.Path -WorkspaceRoot $Report.WorkspaceRoot), $name, $when)
        }
    }
    # 被排除项必须无条件列出：旧产物（例如缺 pkg\game_web_bg.wasm 的 fengari 时代历史包）
    # 被静默忽略时，用户看到的只是「找不到产物」，而真正该看到的是「你那个包缺什么」。
    if ($Report.Rejected.Count -gt 0) {
        Write-Host ("  已排除 {0} 项：" -f $Report.Rejected.Count)
        foreach ($item in $Report.Rejected) { Write-Host "    $item" }
    }
}

function Resolve-GameBundle {
    <#
    .SYNOPSIS
    自动发现唯一的发布产物目录；找不到或有歧义时打印诊断并返回 $null。
    .DESCRIPTION
    只在候选唯一时静默选中。候选多于一个说明这台机器上发布过多个包——此时猜错就是
    「拿 A 的包验证 B 并报出通过」，因此退回让调用方显式传 -BundleRoot（仓库既有的
    「不静默跳过」口径，见 verify.ps1 对 MVP 样例的处理）。
    #>
    param(
        [Parameter(Mandatory = $true)][string]$Platform,
        [string]$WorkspaceRoot = "",
        [string]$ProjectRoot = ""
    )

    $report = Find-GameBundle -Platform $Platform -WorkspaceRoot $WorkspaceRoot -ProjectRoot $ProjectRoot
    if ($report.Candidates.Count -eq 1) {
        $picked = $report.Candidates[0]
        $when = if ($picked.BuildTime) { $picked.BuildTime.ToString("yyyy-MM-dd HH:mm:ss") } else { "未知发布时刻" }
        Write-Host ("==> bundle: {0}  [{1}, {2}]" -f (Get-DisplayPath -Path $picked.Path -WorkspaceRoot $report.WorkspaceRoot), $picked.PublishName, $when)
        return $picked.Path
    }

    $hint = if ($report.Candidates.Count -eq 0) {
        "先发布一个，例如：powershell -NoProfile -File scripts\package.ps1 -ProjectRoot examples\game_jhyt -Platform $Platform"
    }
    else {
        "本机存在 {0} 个可用产物，自动选择会有歧义。" -f $report.Candidates.Count
    }
    Write-BundleReport -Report $report -Hint $hint
    return $null
}
