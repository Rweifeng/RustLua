<#
.SYNOPSIS
把早启动品牌图（icon/splash）落到发布根的 `branding/` 下（`package-branding.py` 的 PowerShell 版）。

.DESCRIPTION
产物形态：
  * 工程 `project.toml` 声明了 `<kind>_image` ⇒ 用它（PNG 原样复制，其它格式解码后转存 PNG）；
  * `-SplashFormat jpg`（微信发布）⇒ splash 缩到 ≤960×540、JPEG q80，主包不再背 1.2 MB PNG；
  * 没声明 ⇒ 用引擎默认图 `engine/assets/engine_<kind>.png`。

键扫描口径与 `package-icon.ps1` 共用 `scripts/lib/project-image-key.ps1`（唯一一份）。

退出码契约（与旧的 .py 一致，调用方按 `$LASTEXITCODE` 判）：
  0 = 成功，`branding_ok <kind>=<来源> bytes=<n>` 逐张打在成功流上；
  1 = 失败，`branding_failed: <原因>` 打一行。

为什么不再用 Python：打包链上多一个解释器就多一类环境故障（PATH 上同时有 Microsoft Store
的 `python.exe` 应用执行别名与真实安装时 `Get-Command python` 返回数组，`& $cmd.Source`
拼出不存在的命令名；Pillow 缺失又会在"声明了图"时把 web/微信两条发布一起拦下）。
这里只依赖 .NET（`System.Drawing`），Windows 上恒在。
#>

[CmdletBinding()]
param(
    # 游戏工程根（含 project.toml）。
    [Parameter(Mandatory = $true)][string]$ProjectRoot,
    # 品牌图输出目录（发布根的 branding/）。
    [Parameter(Mandatory = $true)][string]$Out,
    # splash 输出格式：webgl 用 png（index.html 固定引用），微信用 jpg（主包 4 MB 门）。
    [ValidateSet("png", "jpg")][string]$SplashFormat = "png"
)

$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "..\lib\project-image-key.ps1")

function Copy-ImageAsPng {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Target
    )
    Add-Type -AssemblyName System.Drawing

    $image = [System.Drawing.Image]::FromFile($Source)
    try {
        # 转 PNG 前先落到 32bppArgb 位图上：源图可能是索引色 / CMYK / 带调色板透明，
        # 直接 Save(Png) 在部分编码器上会丢 alpha。
        $bitmap = New-Object System.Drawing.Bitmap($image.Width, $image.Height, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
        try {
            $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
            try {
                $graphics.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
                $graphics.DrawImage($image, (New-Object System.Drawing.Rectangle(0, 0, $image.Width, $image.Height)))
            }
            finally { $graphics.Dispose() }
            $bitmap.Save($Target, [System.Drawing.Imaging.ImageFormat]::Png)
        }
        finally { $bitmap.Dispose() }
    }
    finally { $image.Dispose() }
}

function Get-ImageFormatName {
    param([Parameter(Mandatory = $true)][string]$Path)
    Add-Type -AssemblyName System.Drawing
    $image = [System.Drawing.Image]::FromFile($Path)
    try { return $image.RawFormat.Guid.ToString() }
    finally { $image.Dispose() }
}

# 微信 splash：1280×720 PNG 实测 1,225 KB，把主包顶到 2,626 KB。缩到 ≤960×540 再按
# JPEG q80 编码约 82 KB，主包 ~1,483 KB；splash 只是 preboot 画面，960 宽足够。
function Save-JpegSplash {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Target,
        [int]$MaxWidth = 960,
        [int]$MaxHeight = 540,
        [int]$Quality = 80
    )
    Add-Type -AssemblyName System.Drawing
    $image = [System.Drawing.Image]::FromFile($Source)
    try {
        $scale = [Math]::Min(1.0, [Math]::Min($MaxWidth / $image.Width, $MaxHeight / $image.Height))
        $width = [Math]::Max(1, [int][Math]::Round($image.Width * $scale))
        $height = [Math]::Max(1, [int][Math]::Round($image.Height * $scale))
        $bitmap = New-Object System.Drawing.Bitmap($width, $height, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
        try {
            $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
            try {
                $graphics.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
                $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
                $graphics.Clear([System.Drawing.Color]::FromArgb(15, 25, 43))
                $graphics.DrawImage($image, (New-Object System.Drawing.Rectangle(0, 0, $width, $height)))
            }
            finally { $graphics.Dispose() }
            $codec = [System.Drawing.Imaging.ImageCodecInfo]::GetImageEncoders() |
                Where-Object { $_.MimeType -eq "image/jpeg" } | Select-Object -First 1
            if (-not $codec) { throw "System.Drawing 没有 JPEG 编码器" }
            $encoder = New-Object System.Drawing.Imaging.EncoderParameter(
                [System.Drawing.Imaging.Encoder]::Quality, [long]$Quality)
            $parameters = New-Object System.Drawing.Imaging.EncoderParameters(1)
            try {
                $parameters.Param[0] = $encoder
                $bitmap.Save($Target, $codec, $parameters)
            }
            finally {
                $encoder.Dispose()
                $parameters.Dispose()
            }
        }
        finally { $bitmap.Dispose() }
    }
    finally { $image.Dispose() }
}

try {
    Add-Type -AssemblyName System.Drawing
    $engineAssets = Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) "engine\assets"
    if (-not (Test-Path -LiteralPath $engineAssets -PathType Container)) {
        throw "engine/assets not found next to the packaging scripts: $engineAssets"
    }
    New-Item -ItemType Directory -Force -Path $Out | Out-Null

    $pngGuid = ([System.Drawing.Imaging.ImageFormat]::Png).Guid.ToString()
    foreach ($kind in @("icon", "splash")) {
        $configured = Get-ProjectConfiguredImage -Root $ProjectRoot -Key ($kind + "_image")
        $source = $configured
        if (-not $source) {
            $fallback = Join-Path $engineAssets ("engine_" + $kind + ".png")
            if (-not (Test-Path -LiteralPath $fallback -PathType Leaf)) {
                throw "engine default $kind image is missing: $fallback"
            }
            $source = $fallback
        }
        $useJpeg = ($kind -eq "splash" -and $SplashFormat -eq "jpg")
        $extension = if ($useJpeg) { ".jpg" } else { ".png" }
        $target = Join-Path $Out ($kind + $extension)
        if ($useJpeg) {
            Save-JpegSplash -Source $source -Target $target
        }
        elseif ((Get-ImageFormatName -Path $source) -eq $pngGuid) {
            # PNG 原样复制：重编码会改字节，而"包内字节 == 工程声明的字节"是最省事的口径。
            Copy-Item -LiteralPath $source -Destination $target -Force
        }
        else {
            Copy-ImageAsPng -Source $source -Target $target
        }
        $origin = if ($configured) { $configured } else { "engine-default" }
        Write-Output ("branding_ok {0}={1} bytes={2}" -f $kind, $origin, (Get-Item -LiteralPath $target).Length)
    }
    exit 0
}
catch {
    Write-Output ("branding_failed: " + $_.Exception.Message)
    exit 1
}
