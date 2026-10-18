# 发布期压缩工具（binaryen 的 wasm-opt 与 terser）的定位与按需安装。
#
# 为什么和 zig 一样要管起来：这两个工具**不是可选美化**，而是微信小游戏主包 4 MB 上限的
# 组成部分（docs/52 R11 的体积口径：1.88 未优化 wasm 3,150 KB → `wasm-opt -Oz` 后 2,428 KB；
# terser 压缩 web-runtime.js + wechat-adapter.js 约回收 180 KB）。原实现把两者的位置写成
# **单机路径**（`$env:USERPROFILE\.tools\binaryen\...`、`$env:APPDATA\npm\terser.cmd`），
# 换一台机器就双双落空，而脚本只 `Write-Warning` 继续走 —— 于是产出一个注定超标的包，
# 要等到最后的体积门才炸（实测 2026-10-01：主包 4,660 KB > 4,096 KB）。
#
# binaryen 必须**锁版本**：`wasm-opt` 的任意 pass 都可能改写 EH 编码，把 1.88 产物的纯
# `try_table` 部分转回 legacy `try`，造成新旧 EH 混合（docs/32/docs/52）。仓库口径是
# binaryen version_133 —— 实测它的 `-Oz` **会保留** try_table，产物仍可被 V8 加载。
# 因此这里只认 133，装完还要跑 `wasm-opt --version` 复核，版本不对就当没找到。
#
# 解析顺序（两者一致）：显式环境变量 → `engine\.tools\<...>` 缓存 → PATH/npm 全局 → 按需安装。
# 安装产物一律落在 gitignore 的 `engine\.tools\` 下：仓库只留脚本，不留二进制，也不污染
# 全局（`~\.cargo\bin`、`%APPDATA%\npm`）。
#
# 命令行用法：
#   powershell -File scripts\build\minify-toolchain.ps1                 # 解析两者（必要时安装）
#   powershell -File scripts\build\minify-toolchain.ps1 -PrintWasmOpt   # 只打印 wasm-opt 路径
#   powershell -File scripts\build\minify-toolchain.ps1 -NoInstall      # 只解析，绝不下载
[CmdletBinding()]
param(
    [switch]$NoInstall,
    [switch]$PrintWasmOpt,
    [switch]$PrintTerser
)

$workspaceRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$engineRoot = Join-Path $workspaceRoot "engine"
$toolsRoot = Join-Path $engineRoot ".tools"

# binaryen 固定版本：与 docs/52 的 EH 口径绑定，不是"新版本更好"。
$BinaryenVersion = "version_133"
# Windows 侧官方资产是 **tar.gz**（不是 zip；实测 2026-10-01 用 .zip 直接 404），
# 解压走 Windows 自带的 bsdtar（见 Get-BsdtarExe 的说明）。
$BinaryenArchiveName = "binaryen-$BinaryenVersion-x86_64-windows.tar.gz"
$BinaryenBaseUrl = "https://github.com/WebAssembly/binaryen/releases/download/$BinaryenVersion"
$BinaryenDownloadUrl = "$BinaryenBaseUrl/$BinaryenArchiveName"
$BinaryenShaUrl = "$BinaryenDownloadUrl.sha256"
# 备用通道：GitHub Releases API 的资产端点（`Accept: application/octet-stream`，服务端 302 到
# 资产 CDN）。留着不是因为它"更好"，而是本机实测**官方直链会间歇性失败**：同一个 106 MB 资产
# 一次拉成功、下一次直接连接失败，而 `api.github.com` 与资产 CDN 稳定可达（2026-10-01）。
# 换 id 不用写死：按资产名从 release 元数据里查，一次 API 调用；匿名额度 60 次/小时，而这条
# 路径只在缓存为空时才走，撞不上限。
$BinaryenApiReleaseUrl = "https://api.github.com/repos/WebAssembly/binaryen/releases/tags/$BinaryenVersion"
# 下载源的**期望摘要**，取自官方 `…/download/version_133/binaryen-version_133-x86_64-windows.tar.gz.sha256`
# 并经实测核对（2026-10-01）。把它写进仓库而不是只依赖 `.sha256` 文本：那个文件和二进制来自
# 同一个通道，只信它就等于"用来源证明自己"；换资产必须先换摘要，否则校验会响亮失败。
$BinaryenExpectedSha256 = "17a2cbeac6b5693c5fbafab3838d3c65fd9c1eb38b05f5baec6c657e8c84995b"
# 资产解出来自带的顶层目录名就是 `binaryen-version_133/`（不是 `version_133/`），缓存路径按它拼。
$BinaryenExtractRoot = Join-Path $toolsRoot "binaryen"
$BinaryenCacheDir = Join-Path $BinaryenExtractRoot "binaryen-$BinaryenVersion"
$WasmOptExe = Join-Path $BinaryenCacheDir "bin\wasm-opt.exe"

# terser 主版本固定（5.x）：压缩行为稳定，且 package.ps1 用的是 `<in> -o <out>` 的 v5 CLI 面。
$TerserSpec = "terser@5"
$TerserCacheDir = Join-Path $toolsRoot "terser"
$TerserCmd = Join-Path $TerserCacheDir "node_modules\.bin\terser.cmd"

# 参数名不能叫 $Args——那是 PowerShell 的自动变量，声明成参数会让 splat 拿到空表，
# 版本探测静默失败（实测：wasm-opt 明明可跑，`--version` 却返回空，于是每次都重下 106 MB）。
function Get-NativeVersionOf([string]$Exe, [string[]]$VersionArgs) {
    if ([string]::IsNullOrWhiteSpace($Exe)) { return $null }
    if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) { return $null }
    # 原生工具的版本信息可能写 stderr；PS 5.1 在 EAP=Stop 下会当终止性错误，逐次降级。
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $out = & $Exe @VersionArgs 2>$null
        if ($LASTEXITCODE -ne 0 -or -not $out) { return $null }
        return (($out | Select-Object -First 1).ToString()).Trim()
    }
    catch { return $null }
    finally { $ErrorActionPreference = $savedEap }
}

function Get-RemoteText {
    param([string]$Url, [hashtable]$Headers)
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        if ($Headers) { return (Invoke-WebRequest -Uri $Url -Headers $Headers -UseBasicParsing).Content }
        return (Invoke-WebRequest -Uri $Url -UseBasicParsing).Content
    }
    catch { return $null }
    finally { $ErrorActionPreference = $savedEap }
}

function Invoke-RemoteFile {
    param([string]$Url, [string]$OutFile, [hashtable]$Headers)
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        if ($Headers) { Invoke-WebRequest -Uri $Url -Headers $Headers -OutFile $OutFile -UseBasicParsing }
        else { Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing }
        return (Test-Path -LiteralPath $OutFile -PathType Leaf)
    }
    catch { return $false }
    finally { $ErrorActionPreference = $savedEap }
}

function Get-BsdtarExe {
    # 必须是 Windows 自带的 bsdtar，不能只看 PATH：从 Git Bash 里跑脚本时 PATH 上先命中
    # `/usr/bin/tar`（GNU tar），它把 `C:\...` 的盘符当成远程主机名，报
    # `tar (child): Cannot connect to C: resolve failed`（实测 2026-10-01）。
    $systemTar = Join-Path $env:SystemRoot "System32\tar.exe"
    if (Test-Path -LiteralPath $systemTar -PathType Leaf) { return $systemTar }
    $cmd = Get-Command tar -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

function Get-BinaryenApiAssetUrl([string]$AssetName) {
    $text = Get-RemoteText -Url $BinaryenApiReleaseUrl -Headers @{
        "Accept"               = "application/vnd.github+json"
        "X-GitHub-Api-Version" = "2022-11-28"
    }
    if (-not $text) { return $null }
    try { $release = $text | ConvertFrom-Json } catch { return $null }
    foreach ($asset in @($release.assets)) {
        if ($asset.name -eq $AssetName) {
            return "https://api.github.com/repos/WebAssembly/binaryen/releases/assets/$($asset.id)"
        }
    }
    return $null
}

function Resolve-WasmOpt {
    param([switch]$Provision)

    # 认版本而不是只认存在：非 133 的 wasm-opt 可能改写 EH，产出一个"更小但 V8 拒绝编译"的包。
    $candidates = @()
    $candidates += $env:WASM_OPT
    $candidates += $WasmOptExe
    $cmd = Get-Command wasm-opt -ErrorAction SilentlyContinue
    if ($cmd) { $candidates += $cmd.Source }

    foreach ($cand in $candidates) {
        $v = Get-NativeVersionOf $cand @("--version")
        if ($v -and $v -match ([regex]::Escape($BinaryenVersion))) { return $cand }
    }
    if (-not $Provision) { return $null }

    $tarExe = Get-BsdtarExe
    if (-not $tarExe) {
        throw "tar 未找到（Windows 10 1803+ 自带 bsdtar）：无法解压 binaryen。请手动放置 binaryen $BinaryenVersion 后设 `$env:WASM_OPT=<path>\wasm-opt.exe"
    }
    New-Item -ItemType Directory -Force -Path $toolsRoot | Out-Null
    $archive = Join-Path $toolsRoot $BinaryenArchiveName
    Remove-Item -LiteralPath $archive -Force -ErrorAction SilentlyContinue
    Write-Host "==> Downloading binaryen $BinaryenVersion (wasm-opt -Oz; 微信主包 4 MB 上限需要)" -ForegroundColor Cyan

    # 下载的是**会被塞进发布产物的二进制**，所以只认硬编码摘要；对不上就丢弃。
    # 拿一份来路不明的 wasm-opt 继续打包，等于把"包超标"换成更难查的"包坏了"。
    # 官方 `.sha256` 只是额外佐证：它和锁定摘要不一致，说明仓库里的锁过期了，直接失败。
    $shaText = Get-RemoteText -Url $BinaryenShaUrl
    $official = $null
    if ($shaText -and $shaText -match '([0-9a-fA-F]{64})') { $official = $Matches[1].ToLowerInvariant() }
    if ($official -and $official -ne $BinaryenExpectedSha256) {
        throw ("binaryen 摘要与仓库锁定值不一致（官方 {0} != 锁定 {1}）：$BinaryenVersion 的资产被改动过，先更新脚本里的锁定摘要并重新验证 EH。" -f $official, $BinaryenExpectedSha256)
    }

    # 两条通道按顺序试，第一条不通就走第二条：官方直链在本机会**间歇性**连接失败（实测
    # 2026-10-01 同一个 106 MB 资产一次成功一次失败），而 Releases API 那条稳定。
    # 无论走哪条，产物都必须对上写死的摘要——通道只是搬运，不充当可信来源。
    $channels = @(@{ Url = $BinaryenDownloadUrl; Headers = $null })
    $apiUrl = Get-BinaryenApiAssetUrl $BinaryenArchiveName
    if ($apiUrl) {
        $channels += @{
            Url     = $apiUrl
            Headers = @{ "Accept" = "application/octet-stream"; "X-GitHub-Api-Version" = "2022-11-28" }
        }
    }
    $downloaded = $false
    foreach ($ch in $channels) {
        Write-Host "    $($ch.Url)" -ForegroundColor DarkGray
        Remove-Item -LiteralPath $archive -Force -ErrorAction SilentlyContinue
        $downloaded = Invoke-RemoteFile -Url $ch.Url -OutFile $archive -Headers $ch.Headers
        if ($downloaded) { break }
    }
    if (-not $downloaded) {
        throw "binaryen 下载失败：官方直链与 Releases API 资产通道都没拉到（$BinaryenDownloadUrl）。离线环境请手动放置 binaryen $BinaryenVersion 后设 `$env:WASM_OPT=<path>\wasm-opt.exe"
    }

    Write-Host "==> Verifying binaryen sha256" -ForegroundColor Cyan
    $actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $BinaryenExpectedSha256) {
        Remove-Item -LiteralPath $archive -Force -ErrorAction SilentlyContinue
        throw ("binaryen sha256 mismatch（期望 {0}，实际 {1}）：已删除下载文件，不使用未校验的二进制。" -f $BinaryenExpectedSha256, $actual)
    }

    # bsdtar 解出的顶层目录就是 binaryen-version_133/，直接落在 engine\.tools\binaryen\ 下。
    New-Item -ItemType Directory -Force -Path $BinaryenExtractRoot | Out-Null
    Remove-Item -LiteralPath $BinaryenCacheDir -Recurse -Force -ErrorAction SilentlyContinue
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $tarExe -xzf $archive -C $BinaryenExtractRoot 2>&1 | ForEach-Object { Write-Host $_ }
        $tarExit = $LASTEXITCODE
    }
    finally { $ErrorActionPreference = $savedEap }
    Remove-Item -LiteralPath $archive -Force -ErrorAction SilentlyContinue
    if ($tarExit -ne 0) { throw "binaryen 解压失败（tar exit $tarExit）。" }
    $v = Get-NativeVersionOf $WasmOptExe @("--version")
    if (-not ($v -and $v -match ([regex]::Escape($BinaryenVersion)))) {
        throw "binaryen 解压后版本校验失败（期望含 $BinaryenVersion，实际 '$v'）：$WasmOptExe"
    }
    Write-Host "wasm-opt ready: $WasmOptExe ($v)" -ForegroundColor Green
    return $WasmOptExe
}

function Resolve-Terser {
    param([switch]$Provision)

    # terser 的候选是 **cmd 包装**（npm 的风格）。只认 5.x：与上面固定安装的 `$TerserSpec`
    # 同口径，也保证 package.ps1 用的 `terser <in> -o <out>` CLI 面不变。
    $candidates = @()
    $candidates += $env:TERSER
    $candidates += $TerserCmd
    $cmd = Get-Command terser -ErrorAction SilentlyContinue
    if ($cmd) { $candidates += $cmd.Source }

    foreach ($cand in $candidates) {
        $v = Get-NativeVersionOf $cand @("--version")
        if ($v -and $v -match '\b5\.\d+\.\d+') { return $cand }
    }
    if (-not $Provision) { return $null }

    if (-not (Get-Command npm -ErrorAction SilentlyContinue)) {
        Write-Host "==> npm not found; skipping terser install (JS 将保持未压缩)" -ForegroundColor Yellow
        return $null
    }
    New-Item -ItemType Directory -Force -Path $TerserCacheDir | Out-Null
    Write-Host "==> Installing $TerserSpec into engine\.tools\terser (npm install --prefix)" -ForegroundColor Cyan
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $exit = 1
    try {
        & npm install --no-fund --no-audit --loglevel error --prefix $TerserCacheDir $TerserSpec 2>&1 |
            ForEach-Object { Write-Host $_ }
        $exit = $LASTEXITCODE
    }
    finally { $ErrorActionPreference = $savedEap }
    if ($exit -ne 0) { throw "terser install failed (npm exit $exit)." }
    $v = Get-NativeVersionOf $TerserCmd @("--version")
    if (-not $v) { throw "terser installed but not runnable: $TerserCmd" }
    Write-Host "terser ready: $TerserCmd ($v)" -ForegroundColor Green
    return $TerserCmd
}

if ($MyInvocation.InvocationName -ne '.' -and $MyInvocation.InvocationName -ne 'source') {
    $ErrorActionPreference = "Stop"
    $provision = -not $NoInstall
    $wasmOpt = Resolve-WasmOpt -Provision:$provision
    $terser = Resolve-Terser -Provision:$provision

    if ($PrintWasmOpt) { if ($wasmOpt) { Write-Output $wasmOpt; exit 0 } else { exit 2 } }
    if ($PrintTerser) { if ($terser) { Write-Output $terser; exit 0 } else { exit 2 } }

    if ($wasmOpt) { Write-Host "wasm-opt = $wasmOpt" -ForegroundColor Green }
    else { Write-Host "wasm-opt = 未找到（微信主包可能超 4 MB；跑本脚本允许安装，或设 `$env:WASM_OPT）" -ForegroundColor Yellow }
    if ($terser) { Write-Host "terser   = $terser" -ForegroundColor Green }
    else { Write-Host "terser   = 未找到（运行时 JS 保持未压缩，约多占 180 KB；需 npm）" -ForegroundColor Yellow }
    if (-not $wasmOpt -and -not $terser) { exit 2 }
    exit 0
}
