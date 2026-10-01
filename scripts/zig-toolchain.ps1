# zig 工具链的定位与按需安装（被 package.ps1 / verify-wasm-lua.ps1 共用，也可命令行调用）。
#
# 为什么需要 zig：vendored Lua 的 C 源用 `zig cc --target=wasm32-freestanding` 编成
# wasm（见 engine/vendor/mlua-sys/build/main_inner.rs 的 find_zig），所以**只有 web 平台**
# （webgl / wechat-minigame）的构建会用到它；pc 构建完全不碰 zig。
#
# 为什么不在 engine/.cargo/config.toml 里写死路径：那是版本管理的文件，绝对路径属于
# 单机事实（历史上写死了某台机器的 `C:\Users\Administrator\...`，换一台机器就 panic，
# 而且失败信息被 `cargo ... | Out-Null` 吞掉，看起来像"WASM build failed"而非"缺 zig"）。
# 现在改为按需解析 + 按需下载**固定版本**到 `engine\.tools\zig\`（已 gitignore）：
# 仓库里只留脚本，不留二进制。
#
# 解析顺序（先命中先返回）：
#   1. $env:ZIG                —— 显式覆盖，进程环境优先于 cargo 的 [env]
#   2. engine\.tools\zig\...   —— 本脚本下载过的固定版本
#   3. PATH 上的 zig           —— 接受任意可用版本（warn），因为 build.rs 只用 `zig cc`
#   4. 下载固定版本并校验 `zig version` 后返回（-NoDownload 时跳过这步）
#
# 命令行用法（作为脚本直接运行时默认**会**按需下载，这就是"一次性装好"的入口）：
#   powershell -File scripts\zig-toolchain.ps1                 # 解析（必要时下载）并打印
#   powershell -File scripts\zig-toolchain.ps1 -PrintPath      # 只打印 zig.exe 路径，供 CI 取值
#   powershell -File scripts\zig-toolchain.ps1 -NoDownload     # 只解析，绝不下载（环境探测）
[CmdletBinding()]
param(
    # 只解析、不下载：质量门用它把"缺 zig"报成环境受限跳过，而不是当场拉网络。
    [switch]$NoDownload,
    # 只打印最终 zig.exe 路径（供 CI 里 `$zig = & scripts\zig-toolchain.ps1 -PrintPath` 取值）。
    [switch]$PrintPath
)

$workspaceRoot = Split-Path -Parent $PSScriptRoot
$engineRoot = Join-Path $workspaceRoot "engine"

# 固定版本：换版本要重新验证 wasm shim（zig 的 wasm32-freestanding 目标与 libc 头会随版本变）。
$ZigVersion = "0.16.0"
$ZigArchiveName = "zig-x86_64-windows-$ZigVersion.zip"
$ZigUnzipDirName = "zig-x86_64-windows-$ZigVersion"
$ZigDownloadUrl = "https://ziglang.org/download/$ZigVersion/$ZigArchiveName"
$ZigCacheRoot = Join-Path $engineRoot ".tools\zig"
$ZigCachedExe = Join-Path (Join-Path $ZigCacheRoot $ZigUnzipDirName) "zig.exe"

function Get-ZigVersionOf([string]$Exe) {
    if ([string]::IsNullOrWhiteSpace($Exe)) { return $null }
    if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) { return $null }
    # zig 的正常输出走 stdout，但为防 PS 5.1 在 EAP=Stop 下把原生 stderr 变成终止性错误，
    # 这里同样临时降级（同 package.ps1 里调用 cargo 的既有模式）。
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $out = & $Exe version 2>$null
        if ($LASTEXITCODE -ne 0) { return $null }
        if (-not $out) { return $null }
        return ($out | Select-Object -First 1).ToString().Trim()
    }
    catch { return $null }
    finally { $ErrorActionPreference = $savedEap }
}

function Install-ZigToolchain {
    if (Test-Path -LiteralPath $ZigCachedExe -PathType Leaf) {
        $v = Get-ZigVersionOf $ZigCachedExe
        if ($v -eq $ZigVersion) { return $ZigCachedExe }
        Write-Host "==> Cached zig version mismatch ('$v' != $ZigVersion); re-downloading" -ForegroundColor DarkGray
        Remove-Item -LiteralPath (Join-Path $ZigCacheRoot $ZigUnzipDirName) -Recurse -Force -ErrorAction SilentlyContinue
    }
    New-Item -ItemType Directory -Force -Path $ZigCacheRoot | Out-Null
    $zip = Join-Path $ZigCacheRoot $ZigArchiveName
    Write-Host "==> Downloading zig $ZigVersion (needed for wasm32 Lua C build)" -ForegroundColor Cyan
    Write-Host "    $ZigDownloadUrl" -ForegroundColor DarkGray
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $ZigDownloadUrl -OutFile $zip -UseBasicParsing
        $downloadExit = $?
    }
    finally { $ErrorActionPreference = $savedEap }
    if (-not $downloadExit -or -not (Test-Path -LiteralPath $zip -PathType Leaf)) {
        throw "zig download failed: $ZigDownloadUrl（网络不通就手动放置 zig $ZigVersion 后设 `$env:ZIG=<path>\zig.exe）"
    }
    Write-Host "==> Extracting zig to $ZigCacheRoot" -ForegroundColor Cyan
    Expand-Archive -LiteralPath $zip -DestinationPath $ZigCacheRoot -Force
    Remove-Item -LiteralPath $zip -Force -ErrorAction SilentlyContinue
    $v = Get-ZigVersionOf $ZigCachedExe
    if ($v -ne $ZigVersion) {
        throw "installed zig version check failed (got '$v', expected '$ZigVersion'): $ZigCachedExe"
    }
    Write-Host "zig ready: $ZigCachedExe (version $v)" -ForegroundColor Green
    return $ZigCachedExe
}

function Resolve-ZigToolchain {
    param([switch]$Install)

    # 1. 显式覆盖：只在**确实是 zig 且能跑**时采纳（否则 config.toml/环境里的陈旧路径会
    #    把"缺 zig"伪装成 wasm 构建失败）。
    if ($env:ZIG) {
        $v = Get-ZigVersionOf $env:ZIG
        if ($v) { return $env:ZIG }
        Write-Host "==> `$env:ZIG points to an unusable zig ('$env:ZIG'); ignoring and re-resolving" -ForegroundColor DarkGray
    }

    # 2. 仓内缓存的固定版本。
    if (Test-Path -LiteralPath $ZigCachedExe -PathType Leaf) {
        $v = Get-ZigVersionOf $ZigCachedExe
        if ($v) { return $ZigCachedExe }
    }

    # 3. PATH 上的 zig：版本可能不等于固定版本，`zig cc` 通常仍可用，放行但提醒。
    $cmd = Get-Command zig -ErrorAction SilentlyContinue
    if ($cmd) {
        $v = Get-ZigVersionOf $cmd.Source
        if ($v) {
            if ($v -ne $ZigVersion) {
                Write-Host "==> zig on PATH is $v (this repo pins $ZigVersion); continuing, pin via -Install if the shim build misbehaves" -ForegroundColor Yellow
            }
            return $cmd.Source
        }
    }

    if ($Install) { return Install-ZigToolchain }
    return $null
}

if ($MyInvocation.InvocationName -ne '.' -and $MyInvocation.InvocationName -ne 'source') {
    $ErrorActionPreference = "Stop"
    $resolved = Resolve-ZigToolchain -Install:(-not $NoDownload)
    if (-not $resolved) {
        if ($PrintPath) { exit 2 }
        throw @"
zig not found — webgl/wechat-minigame 的构建需要它把 vendored Lua 的 C 源编成 wasm32。
三种解法（任选其一）：
  1) powershell -File scripts\zig-toolchain.ps1            （自动下载 zig $ZigVersion 到 engine\.tools\zig）
  2) 把已有的 zig 放进 PATH
  3) `$env:ZIG = "<path-to-zig.exe>"
"@
    }
    if ($PrintPath) { Write-Output $resolved }
    else { Write-Host "zig = $resolved (version $(Get-ZigVersionOf $resolved))" -ForegroundColor Green }
    exit 0
}
