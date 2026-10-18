<#
.SYNOPSIS
把工程 `project.toml` 的 `icon_image` 打进**发布暂存目录**的 `game.exe`（`package-icon.py` 的 PowerShell 版）。

.DESCRIPTION
只动发布产物，**绝不动** cargo 产物（`engine/target/.../game.exe`）：别的工程与引擎工具
继续用默认图标。整条链是"构建期就能判红"的：

  1. `project.toml` 没声明 `icon_image` ⇒ 原样保留编译期图标，打印
     `project_icon_default: leaving compiled engine icon unchanged`，退出 0；
  2. 声明了 ⇒ 生成 16/24/32/48/256px 的 ICO（256 走 PNG 条目、其余走 32bpp 自底向上 DIB +
     全零 AND 掩码，与 `engine/tools/gen_engine_icon.py` 同规格），在暂存 exe 的**同目录副本**
     上改 PE 资源（原地改一旦失败会毁掉暂存产物），
  3. **读回来逐字节对账**：把副本的 PE 资源树重新解析，按 `RT_GROUP_ICON` 的清单配 `RT_ICON`
     数据块，与刚生成的 ICO 逐档比（尺寸/平面/位深/字节），对不上就红；
  4. 只有第 3 步过了才把副本拷回暂存 exe，并**再对账一次**（UpdateResource 在某些 Windows
     上会短暂占着旧文件句柄，所以用拷贝覆盖而不是 `Move-Item` 替换）。

退出码契约（与旧的 .py 一致，调用方按 `$LASTEXITCODE` 判）：
  0 = 成功（`project_icon_default:` 或 `project_icon_ok: …` 一行）；
  1 = 失败（`FAIL project icon: <原因>` 一行）。

为什么不再用 Python：打包链上多一个解释器就多一类环境故障（Store 别名 + 真实安装让
`Get-Command python` 返回数组，`& $cmd.Source` 拼出不存在的命令名；Pillow 缺失又会在
"声明了图"时把 web/微信两条发布一起拦下）。这里只依赖 .NET 与 kernel32 的两个资源 API。
#>

[CmdletBinding()]
param(
    # 游戏工程根（含 project.toml）。
    [Parameter(Mandatory = $true)][string]$ProjectRoot,
    # 暂存目录里的 game.exe（绝不传 cargo 产物）。
    [Parameter(Mandatory = $true)][string]$Exe
)

$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "..\lib\project-image-key.ps1")

$RT_ICON = 3
$RT_GROUP_ICON = 14
$ICON_SIZES = @(16, 24, 32, 48, 256)

# ── 小端读取 ────────────────────────────────────────────────────────────────
function Read-U16 { param([byte[]]$Blob, [int]$Offset) [BitConverter]::ToUInt16($Blob, $Offset) }
function Read-U32 { param([byte[]]$Blob, [int]$Offset) [BitConverter]::ToUInt32($Blob, $Offset) }

# ── PE 解析（只到"按 RVA 取字节"与"按类型遍历资源树"两层，与 check_exe_icon.py 同口径）──
function Get-PeView {
    param([byte[]]$Blob, [string]$Origin, [switch]$AllowMissingRsrc)

    if ($Blob.Length -lt 0x40 -or $Blob[0] -ne 0x4D -or $Blob[1] -ne 0x5A) {
        throw "$Origin`: not a DOS/PE file"
    }
    $peOffset = [int](Read-U32 $Blob 0x3C)
    if ($Blob.Length -lt ($peOffset + 24) -or $Blob[$peOffset] -ne 0x50 -or $Blob[$peOffset + 1] -ne 0x45 -or
        $Blob[$peOffset + 2] -ne 0 -or $Blob[$peOffset + 3] -ne 0) {
        throw "$Origin`: PE header not found"
    }
    $sectionCount = [int](Read-U16 $Blob ($peOffset + 6))
    $optionalSize = [int](Read-U16 $Blob ($peOffset + 20))
    $optional = $peOffset + 24
    $magic = Read-U16 $Blob $optional
    if ($magic -ne 0x10B -and $magic -ne 0x20B) {
        throw ("{0}: unsupported PE optional header magic {1:X}" -f $Origin, $magic)
    }
    # 数据目录项宽 8 字节：PE32 从可选头内偏移 96 起、PE32+ 从 112 起；资源目录是第 3 项。
    $directoryOffset = $optional + $(if ($magic -eq 0x10B) { 96 } else { 112 })
    $rsrcRva = [int](Read-U32 $Blob ($directoryOffset + 2 * 8))
    if ($rsrcRva -eq 0 -and -not $AllowMissingRsrc) {
        throw "$Origin`: no .rsrc section, the icon was never linked in"
    }
    $sections = @()
    for ($i = 0; $i -lt $sectionCount; $i++) {
        $entry = $optional + $optionalSize + 40 * $i
        $sections += [pscustomobject]@{
            VirtualAddress = [int](Read-U32 $Blob ($entry + 12))
            VirtualSize    = [int](Read-U32 $Blob ($entry + 8))
            RawPointer     = [int](Read-U32 $Blob ($entry + 20))
            RawSize        = [int](Read-U32 $Blob ($entry + 16))
        }
    }
    return [pscustomobject]@{
        Blob     = $Blob
        Origin   = $Origin
        RsrcRva  = $rsrcRva
        Sections = $sections
    }
}

function Get-PeRvaBytes {
    param($Pe, [int]$Rva, [int]$Size)
    foreach ($section in $Pe.Sections) {
        $span = [Math]::Max($section.VirtualSize, $section.RawSize)
        if ($Rva -ge $section.VirtualAddress -and $Rva -lt ($section.VirtualAddress + $span)) {
            $offset = $section.RawPointer + ($Rva - $section.VirtualAddress)
            if (($offset + $Size) -gt $Pe.Blob.Length) {
                throw ("{0}: RVA {1:X}+{2} runs past the end of the file" -f $Pe.Origin, $Rva, $Size)
            }
            $out = New-Object byte[] $Size
            [Array]::Copy($Pe.Blob, $offset, $out, 0, $Size)
            return $out
        }
    }
    throw ("{0}: RVA {1:X}+{2} is outside every section" -f $Pe.Origin, $Rva, $Size)
}

function Get-PeChildRva {
    param($Pe, [uint32]$Packed)
    # 目录项里的 OffsetToData 是"相对 .rsrc 段首"的偏移（数据项那种才是相对镜像基址的 RVA）。
    return [int]($Pe.RsrcRva + ($Packed -band 0x7FFFFFFF))
}

function Get-PeDirectoryEntries {
    param($Pe, [int]$DirectoryRva)
    # IMAGE_RESOURCE_DIRECTORY 16 字节头，条目表从 +16 起，先名字表后 ID 表；
    # 名字条目首字段是"名称 RVA|0x80000000"，一律给 $null 占位，免得把名称当资源 ID 用。
    $header = Get-PeRvaBytes -Pe $Pe -Rva $DirectoryRva -Size 16
    $named = [int](Read-U16 $header 12)
    $ided = [int](Read-U16 $header 14)
    $entries = @()
    for ($i = 0; $i -lt $named; $i++) {
        $item = Get-PeRvaBytes -Pe $Pe -Rva ($DirectoryRva + 16 + 8 * $i) -Size 8
        $entries += [pscustomobject]@{ Name = $null; Packed = Read-U32 $item 4 }
    }
    for ($i = 0; $i -lt $ided; $i++) {
        $item = Get-PeRvaBytes -Pe $Pe -Rva ($DirectoryRva + 16 + 8 * $named + 8 * $i) -Size 8
        $name = Read-U32 $item 0
        $entries += [pscustomobject]@{
            Name   = $(if (($name -band 0x80000000) -ne 0) { $null } else { [int]$name })
            Packed = Read-U32 $item 4
        }
    }
    return $entries
}

function Get-PeTypeData {
    param($Pe, [int]$TypeId)
    # root（按类型）→ 类型目录（按名字）→ 名字目录（按语言）；名字目录的条目直接指向数据项。
    # **不要把枚举表达式写成引用它自己的循环变量**（`foreach ($x in ... $x.Packed ...)`）：首次
    # 求值时那个变量还是 $null，于是这里会把 root 目录当成名字层读，症状是"语言层下面还有子目录"
    # （实测踩到过一次）。所以每一层先落到独立变量再遍历。
    foreach ($typeEntry in Get-PeDirectoryEntries -Pe $Pe -DirectoryRva $Pe.RsrcRva) {
        if ($typeEntry.Name -ne $TypeId) { continue }
        $results = @()
        $nameEntries = @(Get-PeDirectoryEntries -Pe $Pe -DirectoryRva (Get-PeChildRva -Pe $Pe -Packed $typeEntry.Packed))
        foreach ($nameEntry in $nameEntries) {
            $blocks = @()
            $languageEntries = @(Get-PeDirectoryEntries -Pe $Pe -DirectoryRva (Get-PeChildRva -Pe $Pe -Packed $nameEntry.Packed))
            foreach ($languageEntry in $languageEntries) {
                if (($languageEntry.Packed -band 0x80000000) -ne 0) {
                    throw "$($Pe.Origin): a sub-directory below the language level is not an icon layout this tool accepts"
                }
                $dataEntry = Get-PeRvaBytes -Pe $Pe -Rva (Get-PeChildRva -Pe $Pe -Packed $languageEntry.Packed) -Size 8
                $rva = [int](Read-U32 $dataEntry 0)
                $size = [int](Read-U32 $dataEntry 4)
                $blocks += , (Get-PeRvaBytes -Pe $Pe -Rva $rva -Size $size)
            }
            $results += [pscustomobject]@{ NameId = $nameEntry.Name; Blocks = $blocks }
        }
        return $results
    }
    return @()
}

# ── ICO 读写 ────────────────────────────────────────────────────────────────
function Get-IcoEntries {
    param([string]$Path)
    $data = [IO.File]::ReadAllBytes($Path)
    $reserved = Read-U16 $data 0
    $kind = Read-U16 $data 2
    $count = [int](Read-U16 $data 4)
    if ($reserved -ne 0 -or $kind -ne 1) {
        throw "$Path`: not an ICO (reserved=$reserved, type=$kind)"
    }
    $entries = @()
    for ($i = 0; $i -lt $count; $i++) {
        $record = 6 + 16 * $i
        $edgeW = [int]$data[$record]
        $edgeH = [int]$data[$record + 1]
        $planes = Read-U16 $data ($record + 4)
        $bpp = Read-U16 $data ($record + 6)
        $length = [int](Read-U32 $data ($record + 8))
        $offset = [int](Read-U32 $data ($record + 12))
        if ($edgeW -ne $edgeH) { throw "$Path`: entry $i is not square ($edgeW x $edgeH)" }
        $body = New-Object byte[] $length
        [Array]::Copy($data, $offset, $body, 0, $length)
        $entries += [pscustomobject]@{
            Edge   = $(if ($edgeW -eq 0) { 256 } else { $edgeW })
            Planes = $planes
            Bpp    = $bpp
            Blob   = $body
        }
    }
    return $entries
}

# 32bpp 自底向上 DIB + 全零 AND 掩码：与 gen_engine_icon.py / Pillow 那条 ICO 编码同规格。
function Get-BmpIconBody {
    param($Bitmap, [int]$Size)
    $rect = New-Object System.Drawing.Rectangle(0, 0, $Size, $Size)
    $locked = $Bitmap.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    try {
        # Stride 可以为负（自底向上的位图）：内存里第一行就是图像最后一行。按符号决定落位，
        # 否则图标会上下颠倒——这个错在 Explorer 里要缩到 16px 才看得出来，所以这里不靠运气。
        $stride = $locked.Stride
        $topDown = $stride -ge 0
        $rowPitch = [Math]::Abs($stride)
        $rowBytes = $Size * 4
        $pixels = New-Object byte[] ($rowBytes * $Size)
        $line = New-Object byte[] $rowPitch
        for ($y = 0; $y -lt $Size; $y++) {
            [Runtime.InteropServices.Marshal]::Copy([IntPtr]::Add($locked.Scan0, $y * $stride), $line, 0, $rowPitch)
            $destinationRow = if ($topDown) { $Size - 1 - $y } else { $y }
            [Array]::Copy($line, 0, $pixels, $destinationRow * $rowBytes, $rowBytes)
        }
    }
    finally { $Bitmap.UnlockBits($locked) }
    $andMask = New-Object byte[] ((([int][Math]::Floor(($Size + 31) / 32)) * 4) * $Size)

    $stream = New-Object System.IO.MemoryStream
    $writer = New-Object System.IO.BinaryWriter($stream)
    try {
        $writer.Write([uint32]40)          # biSize = BITMAPINFOHEADER
        $writer.Write([int]$Size)          # biWidth
        $writer.Write([int]($Size * 2))    # biHeight = XOR + AND 两张图
        $writer.Write([uint16]1)           # biPlanes
        $writer.Write([uint16]32)          # biBitCount
        $writer.Write([uint32]0)           # biCompression = BI_RGB
        $writer.Write([uint32]0)           # biSizeImage
        $writer.Write([int]0)              # biXPelsPerMeter
        $writer.Write([int]0)              # biYPelsPerMeter
        $writer.Write([uint32]0)           # biClrUsed
        $writer.Write([uint32]0)           # biClrImportant
        $writer.Write($pixels)
        $writer.Write($andMask)
        # 前置逗号：PowerShell 会把函数返回值**按管道枚举**，byte[] 会被拆成一堆 Byte 对象，
        # 调用方拿到的就是 Object[]（`BinaryWriter.Write` 于是按单字节重载写，只落 1 字节）。
        return , $stream.ToArray()
    }
    finally { $writer.Dispose(); $stream.Dispose() }
}

function New-IcoBytes {
    param([string]$ImagePath)

    Add-Type -AssemblyName System.Drawing
    $source = [System.Drawing.Image]::FromFile($ImagePath)
    try {
        $master = New-Object System.Drawing.Bitmap($source.Width, $source.Height, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
        try {
            $graphics = [System.Drawing.Graphics]::FromImage($master)
            try {
                $graphics.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
                $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
                $graphics.DrawImage($source, (New-Object System.Drawing.Rectangle(0, 0, $source.Width, $source.Height)))
            }
            finally { $graphics.Dispose() }

            $bodies = @()
            foreach ($size in $ICON_SIZES) {
                $scaled = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
                try {
                    $scaleGraphics = [System.Drawing.Graphics]::FromImage($scaled)
                    try {
                        $scaleGraphics.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
                        $scaleGraphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                        $scaleGraphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
                        $scaleGraphics.DrawImage($master, (New-Object System.Drawing.Rectangle(0, 0, $size, $size)))
                    }
                    finally { $scaleGraphics.Dispose() }

                    if ($size -eq 256) {
                        # 256px 走 PNG 条目：BMP 条目在这里会大到没人愿意看，Windows Vista+ 收 PNG。
                        $pngStream = New-Object System.IO.MemoryStream
                        try {
                            $scaled.Save($pngStream, [System.Drawing.Imaging.ImageFormat]::Png)
                            $bodies += , [byte[]]$pngStream.ToArray()
                        }
                        finally { $pngStream.Dispose() }
                    }
                    else {
                        # 显式 `[byte[]]`：函数返回的字节数组必须钉成 byte[]，不能退化成 Object[]。
                        $bodies += , [byte[]](Get-BmpIconBody -Bitmap $scaled -Size $size)
                    }
                }
                finally { $scaled.Dispose() }
            }
        }
        finally { $master.Dispose() }
    }
    finally { $source.Dispose() }

    $count = $ICON_SIZES.Count
    $stream = New-Object System.IO.MemoryStream
    $writer = New-Object System.IO.BinaryWriter($stream)
    try {
        $writer.Write([uint16]0)      # reserved
        $writer.Write([uint16]1)      # type = icon
        $writer.Write([uint16]$count)
        $offset = 6 + 16 * $count
        for ($i = 0; $i -lt $count; $i++) {
            $size = $ICON_SIZES[$i]
            $body = $bodies[$i]
            $edge = [byte]($size % 256)   # 256 在旧式字节里写 0
            $writer.Write($edge)
            $writer.Write($edge)
            $writer.Write([byte]0)
            $writer.Write([byte]0)
            $writer.Write([uint16]1)
            $writer.Write([uint16]32)
            $writer.Write([uint32]$body.Length)
            $writer.Write([uint32]$offset)
            $offset += $body.Length
        }
        foreach ($body in $bodies) { $writer.Write([byte[]]$body) }
        return , $stream.ToArray()
    }
    finally { $writer.Dispose(); $stream.Dispose() }
}

# ── PE 图标资源：列清单 / 写入 / 读回对账 ───────────────────────────────────
function Get-IconResourceIds {
    # 现有的（含陈旧的）图标资源清单：写入前逐条删掉，避免 Explorer 选中旧的那份。
    # 没有 `.rsrc` 的旧产物（-SkipBuild 的某些档）返回空表：给它们加第一份图标是允许的。
    param([string]$ExePath)
    $blob = [IO.File]::ReadAllBytes($ExePath)
    $pe = Get-PeView -Blob $blob -Origin $ExePath -AllowMissingRsrc
    if ($pe.RsrcRva -eq 0) { return @() }
    $resources = @()
    foreach ($typeEntry in Get-PeDirectoryEntries -Pe $pe -DirectoryRva $pe.RsrcRva) {
        if ($typeEntry.Name -ne $RT_ICON -and $typeEntry.Name -ne $RT_GROUP_ICON) { continue }
        foreach ($nameEntry in Get-PeDirectoryEntries -Pe $pe -DirectoryRva (Get-PeChildRva -Pe $pe -Packed $typeEntry.Packed)) {
            foreach ($languageEntry in Get-PeDirectoryEntries -Pe $pe -DirectoryRva (Get-PeChildRva -Pe $pe -Packed $nameEntry.Packed)) {
                if ($null -eq $nameEntry.Name -or $null -eq $languageEntry.Name -or
                    $nameEntry.Name -gt 65535 -or $languageEntry.Name -gt 65535 -or
                    ($languageEntry.Packed -band 0x80000000) -ne 0) {
                    throw "$ExePath`: named/nested icon resources are unsupported; refusing a partial override"
                }
                $resources += [pscustomobject]@{ Kind = $typeEntry.Name; Name = $nameEntry.Name; Language = $languageEntry.Name }
            }
        }
    }
    return $resources
}

if (-not ("RustLuaPeIconResource" -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class RustLuaPeIconResource
{
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern IntPtr BeginUpdateResource(string fileName, bool deleteExistingResources);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool UpdateResource(IntPtr hUpdate, IntPtr lpType, IntPtr lpName, ushort wLanguage, byte[] lpData, uint cbData);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool EndUpdateResource(IntPtr hUpdate, bool discard);
}
'@
}

function Test-ByteArrayEqual {
    param([byte[]]$Left, [byte[]]$Right)
    if ($Left.Length -ne $Right.Length) { return $false }
    for ($i = 0; $i -lt $Left.Length; $i++) { if ($Left[$i] -ne $Right[$i]) { return $false } }
    return $true
}

function New-Win32Failure {
    param([string]$What)
    $code = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
    return (New-Object System.ComponentModel.Win32Exception($code, "$What failed (Win32 error $code)"))
}

function Set-PeIconResources {
    param([string]$ExePath, [string]$IcoPath)

    $resources = Get-IconResourceIds -ExePath $ExePath
    $languages = @($resources | ForEach-Object { $_.Language } | Sort-Object -Unique)
    if ($languages.Count -eq 0) { $languages = @(0) }
    $entries = Get-IcoEntries -Path $IcoPath

    $group = New-Object System.IO.MemoryStream
    $groupWriter = New-Object System.IO.BinaryWriter($group)
    try {
        $groupWriter.Write([uint16]0)
        $groupWriter.Write([uint16]1)
        $groupWriter.Write([uint16]$entries.Count)
        for ($i = 0; $i -lt $entries.Count; $i++) {
            $entry = $entries[$i]
            $edge = [byte]($entry.Edge % 256)
            $groupWriter.Write($edge)
            $groupWriter.Write($edge)
            $groupWriter.Write([byte]0)
            $groupWriter.Write([byte]0)
            $groupWriter.Write([uint16]$entry.Planes)
            $groupWriter.Write([uint16]$entry.Bpp)
            $groupWriter.Write([uint32]$entry.Blob.Length)
            $groupWriter.Write([uint16]($i + 1))   # 资源 ID：1 起
        }
        $groupBytes = $group.ToArray()
    }
    finally { $groupWriter.Dispose(); $group.Dispose() }

    # $false = 保留其余资源（引擎图标之外的一切都不许动）。
    $handle = [RustLuaPeIconResource]::BeginUpdateResource($ExePath, $false)
    if ($handle -eq [IntPtr]::Zero) { throw (New-Win32Failure -What "BeginUpdateResource($ExePath)") }
    try {
        foreach ($resource in $resources) {
            if (-not [RustLuaPeIconResource]::UpdateResource($handle, [IntPtr]$resource.Kind, [IntPtr]$resource.Name, [uint16]$resource.Language, $null, 0)) {
                throw (New-Win32Failure -What "UpdateResource(delete $($resource.Kind)/$($resource.Name))")
            }
        }
        # 每种语言都写一遍（不只写检查器看到的第一种），并保留一个默认组（ID 1）。
        foreach ($language in $languages) {
            for ($i = 0; $i -lt $entries.Count; $i++) {
                $body = $entries[$i].Blob
                if (-not [RustLuaPeIconResource]::UpdateResource($handle, [IntPtr]$RT_ICON, [IntPtr]($i + 1), [uint16]$language, $body, [uint32]$body.Length)) {
                    throw (New-Win32Failure -What "UpdateResource(RT_ICON #$($i + 1))")
                }
            }
            if (-not [RustLuaPeIconResource]::UpdateResource($handle, [IntPtr]$RT_GROUP_ICON, [IntPtr]1, [uint16]$language, $groupBytes, [uint32]$groupBytes.Length)) {
                throw (New-Win32Failure -What "UpdateResource(RT_GROUP_ICON #1)")
            }
        }
        if (-not [RustLuaPeIconResource]::EndUpdateResource($handle, $false)) {
            throw (New-Win32Failure -What "EndUpdateResource(commit)")
        }
        $handle = [IntPtr]::Zero
    }
    finally {
        if ($handle -ne [IntPtr]::Zero) {
            # 失败路径：丢弃待写更新，暂存 exe 保持原样。
            [void][RustLuaPeIconResource]::EndUpdateResource($handle, $true)
        }
    }
}

function Get-ExeIconEntries {
    # exe → [(边长, 平面数, 位深, RT_ICON 字节)]，按 GRPICONDIR 的清单配上各 RT_ICON 块。
    param([string]$ExePath)
    $blob = [IO.File]::ReadAllBytes($ExePath)
    $pe = Get-PeView -Blob $blob -Origin $ExePath

    $groups = @(Get-PeTypeData -Pe $pe -TypeId $RT_GROUP_ICON)
    if ($groups.Count -eq 0) { throw "$ExePath`: no RT_GROUP_ICON in PE resources (icon was never linked in)" }
    if ($groups.Count -gt 1) { throw "$ExePath`: $($groups.Count) icon groups; only the default-icon layout is checked" }

    $grp = $groups[0].Blocks[0]
    $reserved = Read-U16 $grp 0
    $kind = Read-U16 $grp 2
    $count = [int](Read-U16 $grp 4)
    if ($reserved -ne 0 -or $kind -ne 1) { throw "$ExePath`: GRPICONDIR header is wrong (type=$kind)" }

    $icons = @{}
    foreach ($item in Get-PeTypeData -Pe $pe -TypeId $RT_ICON) {
        if ($null -ne $item.NameId) { $icons[[int]$item.NameId] = $item.Blocks[0] }
    }

    $entries = @()
    for ($i = 0; $i -lt $count; $i++) {
        $record = 6 + 14 * $i
        $edgeW = [int]$grp[$record]
        $edgeH = [int]$grp[$record + 1]
        $planes = Read-U16 $grp ($record + 4)
        $bpp = Read-U16 $grp ($record + 6)
        $length = [int](Read-U32 $grp ($record + 8))
        if ($edgeW -ne $edgeH) { throw "$ExePath`: entry $i in the group is not square" }
        $resourceId = [int](Read-U16 $grp ($record + 12))
        if (-not $icons.ContainsKey($resourceId)) { throw "$ExePath`: the group references a missing RT_ICON #$resourceId" }
        $iconBlob = $icons[$resourceId]
        if ($iconBlob.Length -ne $length) {
            throw "$ExePath`: RT_ICON #$resourceId declares $length bytes but has $($iconBlob.Length)"
        }
        $entries += [pscustomobject]@{
            Edge   = $(if ($edgeW -eq 0) { 256 } else { $edgeW })
            Planes = $planes
            Bpp    = $bpp
            Blob   = $iconBlob
        }
    }
    return $entries
}

function Compare-ExeIcon {
    param([string]$ExePath, [string]$IcoPath)
    $expected = @(Get-IcoEntries -Path $IcoPath)
    $actual = @(Get-ExeIconEntries -ExePath $ExePath)
    $problems = @()

    $expectedTable = @($expected | ForEach-Object { "{0}/{1}/{2}/{3}" -f $_.Edge, $_.Planes, $_.Bpp, $_.Blob.Length } | Sort-Object)
    $actualTable = @($actual | ForEach-Object { "{0}/{1}/{2}/{3}" -f $_.Edge, $_.Planes, $_.Bpp, $_.Blob.Length } | Sort-Object)
    if (($expectedTable -join ",") -ne ($actualTable -join ",")) {
        $problems += ("size table mismatch: expected [{0}], actual [{1}]" -f ($expectedTable -join "; "), ($actualTable -join "; "))
    }

    $byEdge = @{}
    foreach ($entry in $expected) { $byEdge[$entry.Edge] = $entry }
    foreach ($entry in $actual) {
        if (-not $byEdge.ContainsKey($entry.Edge)) {
            $problems += "$($entry.Edge)px is not present in the generated ICO"
            continue
        }
        $expectedEntry = $byEdge[$entry.Edge]
        if ($entry.Planes -ne $expectedEntry.Planes -or $entry.Bpp -ne $expectedEntry.Bpp) {
            $problems += ("{0}px format mismatch: expected {1} planes/{2}bpp, actual {3}/{4}" -f $entry.Edge, $expectedEntry.Planes, $expectedEntry.Bpp, $entry.Planes, $entry.Bpp)
        }
        if (-not (Test-ByteArrayEqual -Left ([byte[]]$entry.Blob) -Right ([byte[]]$expectedEntry.Blob))) {
            $problems += "$($entry.Edge)px pixel bytes differ from the generated ICO"
        }
    }
    return [pscustomobject]@{ Problems = $problems; Entries = $actual }
}

# ── 主流程 ──────────────────────────────────────────────────────────────────
try {
    $image = Get-ProjectConfiguredImage -Root $ProjectRoot -Key "icon_image"
    if (-not $image) {
        Write-Output "project_icon_default: leaving compiled engine icon unchanged"
        exit 0
    }

    $exePath = (Resolve-Path -LiteralPath $Exe).Path
    $icoBytes = [byte[]](New-IcoBytes -ImagePath $image)

    # 在暂存 exe 的同目录里做副本：UpdateResource 是就地改，改坏也不能碰 cargo 产物或
    # 上一次的发布产物。
    $work = Join-Path (Split-Path -Parent $exePath) (".project-icon-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $work | Out-Null
    try {
        $icoPath = Join-Path $work "project.ico"
        [IO.File]::WriteAllBytes($icoPath, $icoBytes)
        $candidate = Join-Path $work "game.exe"
        Copy-Item -LiteralPath $exePath -Destination $candidate -Force

        Set-PeIconResources -ExePath $candidate -IcoPath $icoPath
        $readback = Compare-ExeIcon -ExePath $candidate -IcoPath $icoPath
        if ($readback.Problems.Count -gt 0) {
            throw ("PE icon readback failed: " + ($readback.Problems -join "; "))
        }
        # 用拷贝覆盖而不是替换：UpdateResource 会短暂占着旧 exe 的句柄。
        Copy-Item -LiteralPath $candidate -Destination $exePath -Force
        $final = Compare-ExeIcon -ExePath $exePath -IcoPath $icoPath
        if ($final.Problems.Count -gt 0) {
            throw ("PE icon final readback failed: " + ($final.Problems -join "; "))
        }
    }
    finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }

    $sizes = ($ICON_SIZES | Sort-Object) -join "/"
    Write-Output ("project_icon_ok: {0} -> {1} ({2}px, verified)" -f $image, $exePath, $sizes)
    exit 0
}
catch {
    Write-Output ("FAIL project icon: " + $_.Exception.Message)
    # 现场排查用：`$env:RUSTLUA_PACKAGE_DEBUG=1` 时把调用栈也打出来（默认不污染那一行判据）。
    if ($env:RUSTLUA_PACKAGE_DEBUG) {
        Write-Output ("FAIL project icon trace: " + $_.ScriptStackTrace)
    }
    exit 1
}
