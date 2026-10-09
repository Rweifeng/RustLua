<#
.SYNOPSIS
比较两张页面截图，输出"两层画面是否一致"的客观读数（M73-00 WP6-02）。

.DESCRIPTION
为什么需要它：GPU 层与 Canvas2D 层画的是**同一个工程**，但走的是两套实现（Rust 渲染器 vs
`web-runtime.js` 的 Canvas2D 绘制）。"画面一样吗"这个问题，靠读属性、看帧率都答不了——
属性可以说 `presented`、帧率可以很好，画面却完全是另一幅。两张 PNG 的差异是唯一客观的答案。

本脚本**不判红绿**（差异里必然含动画相位差、AA 与文字栅格化差异），它只把差异量、
分区均值色、以及几条结构事实（等比适配盒的黑边、内容盒是否非空）摆出来，供人判读。

用法：
  scripts/compare-shots.ps1 -A a.png -B b.png [-Diff out.png] [-LabelA ...] [-LabelB ...]
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$A,
    [Parameter(Mandatory = $true)][string]$B,
    [string]$LabelA = "A",
    [string]$LabelB = "B",
    [string]$Diff = "",
    # 差异阈（0-255）：超过它才算"这个像素不一致"。
    [int]$Threshold = 32
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

function Read-Pixels([string]$path) {
    $bitmap = [System.Drawing.Bitmap]::FromFile((Resolve-Path $path).Path)
    $rect = New-Object System.Drawing.Rectangle 0, 0, $bitmap.Width, $bitmap.Height
    $data = $bitmap.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly,
        [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $bytes = New-Object byte[] ($data.Stride * $bitmap.Height)
    [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $bytes, 0, $bytes.Length)
    $bitmap.UnlockBits($data)
    $result = [pscustomobject]@{
        Width  = $bitmap.Width
        Height = $bitmap.Height
        Stride = $data.Stride
        Bytes  = $bytes
        Source = $bitmap
    }
    return $result
}

function Get-Pixel($img, [int]$x, [int]$y) {
    # 32bppArgb 在内存里是 BGRA 顺序（小端 ARGB）。
    $i = $y * $img.Stride + $x * 4
    return @($img.Bytes[$i + 2], $img.Bytes[$i + 1], $img.Bytes[$i], $img.Bytes[$i + 3])
}

$imgA = Read-Pixels $A
$imgB = Read-Pixels $B
Write-Output "A=$LabelA  $($imgA.Width)x$($imgA.Height)"
Write-Output "B=$LabelB  $($imgB.Width)x$($imgB.Height)"
if ($imgA.Width -ne $imgB.Width -or $imgA.Height -ne $imgB.Height) {
    Write-Output "尺寸不同，只比较左上公共区域。"
}
$w = [Math]::Min($imgA.Width, $imgB.Width)
$h = [Math]::Min($imgA.Height, $imgB.Height)

# 页面背景（左上角）——用来切出"画布区域"：截图是整页，画布只是其中一块。
$pageBg = Get-Pixel $imgA 0 0
$minX = $w; $minY = $h; $maxX = -1; $maxY = -1
for ($y = 0; $y -lt $h; $y++) {
    for ($x = 0; $x -lt $w; $x++) {
        $p = Get-Pixel $imgA $x $y
        if ([Math]::Abs($p[0] - $pageBg[0]) + [Math]::Abs($p[1] - $pageBg[1]) + [Math]::Abs($p[2] - $pageBg[2]) -gt 24) {
            if ($x -lt $minX) { $minX = $x }; if ($y -lt $minY) { $minY = $y }
            if ($x -gt $maxX) { $maxX = $x }; if ($y -gt $maxY) { $maxY = $y }
        }
    }
}
if ($maxX -lt 0) { Write-Output "A 里没找到非页面背景的内容（截图可能是空页）。"; exit 1 }
Write-Output "画布区域(A) = ($minX,$minY)-($maxX,$maxY)  尺寸 $($maxX - $minX + 1)x$($maxY - $minY + 1)"

$diffBitmap = if ($Diff) { New-Object System.Drawing.Bitmap $w, $h } else { $null }
$total = 0; $differ = 0; $sumAbs = 0.0; $maxAbs = 0
$rows = @{}
for ($y = 0; $y -lt $h; $y++) {
    $rowDiffer = 0
    for ($x = 0; $x -lt $w; $x++) {
        $pa = Get-Pixel $imgA $x $y
        $pb = Get-Pixel $imgB $x $y
        $d = [Math]::Abs($pa[0] - $pb[0]) + [Math]::Abs($pa[1] - $pb[1]) + [Math]::Abs($pa[2] - $pb[2])
        $d3 = [Math]::Max([Math]::Abs($pa[0] - $pb[0]), [Math]::Max([Math]::Abs($pa[1] - $pb[1]), [Math]::Abs($pa[2] - $pb[2])))
        $total++
        $sumAbs += $d / 3.0
        if ($d3 -gt $maxAbs) { $maxAbs = $d3 }
        if ($d3 -gt $Threshold) { $differ++; $rowDiffer++ }
        if ($diffBitmap) {
            $v = [Math]::Min(255, $d3 * 3)
            $diffBitmap.SetPixel($x, $y, [System.Drawing.Color]::FromArgb(255, $v, [Math]::Min(255, $v / 2), 0))
        }
    }
    $rows[$y] = $rowDiffer
}
Write-Output ("整图差异: 平均通道差={0:N2}/255  最大通道差={1}  超过阈值的像素={2}/{3} ({4:N2}%)" -f `
        ($sumAbs / $total), $maxAbs, $differ, $total, (100.0 * $differ / $total))

# 分区均值色（3x3）：能看出"是整幅不同"还是"只有某一块不同"。
Write-Output "分区均值色（左=A / 右=B）:"
$cellW = [int]($w / 3); $cellH = [int]($h / 3)
for ($cy = 0; $cy -lt 3; $cy++) {
    for ($cx = 0; $cx -lt 3; $cx++) {
        $sa = @(0, 0, 0); $sb = @(0, 0, 0); $n = 0
        for ($y = $cy * $cellH; $y -lt [Math]::Min(($cy + 1) * $cellH, $h); $y += 2) {
            for ($x = $cx * $cellW; $x -lt [Math]::Min(($cx + 1) * $cellW, $w); $x += 2) {
                $pa = Get-Pixel $imgA $x $y; $pb = Get-Pixel $imgB $x $y
                $sa[0] += $pa[0]; $sa[1] += $pa[1]; $sa[2] += $pa[2]
                $sb[0] += $pb[0]; $sb[1] += $pb[1]; $sb[2] += $pb[2]
                $n++
            }
        }
        Write-Output ("  [{0},{1}] A=({2,3},{3,3},{4,3})  B=({5,3},{6,3},{7,3})" -f $cx, $cy, `
                [int]($sa[0] / $n), [int]($sa[1] / $n), [int]($sa[2] / $n), `
                [int]($sb[0] / $n), [int]($sb[1] / $n), [int]($sb[2] / $n))
    }
}

# 结构事实：等比适配盒的黑边。2D 层用暗色填黑边（实测约 #0B0F1B）；GPU 层只覆盖内容盒，
# 所以**内容盒外**的上下若干行应当仍是暗色——哪一层若铺满了视口，这里会先露馅。
# 注意取内容盒**外面**的行：盒子边缘那一行属于内容，颜色当然不是黑边色。
function Test-BarColor($img, [string]$name, [int]$midX) {
    $colors = @()
    foreach ($y in @(0, 1, ($img.Height - 2), ($img.Height - 1))) {
        $p = Get-Pixel $img $midX $y
        $colors += ("y$y=({0},{1},{2})" -f $p[0], $p[1], $p[2])
    }
    Write-Output "  $name 视口上下边缘行（内容盒外，应为黑边色）: $($colors -join ' ')"
}

# 颜色种类数（5 位量化）：一眼区分"有细节的画面"与"近似单色的一片"。
function Get-ColorCount($img, [int]$top, [int]$bottom) {
    $seen = @{}
    for ($y = $top; $y -le $bottom; $y += 2) {
        for ($x = 0; $x -lt $img.Width; $x += 2) {
            $p = Get-Pixel $img $x $y
            $key = (($p[0] -shr 3) -shl 10) -bor (($p[1] -shr 3) -shl 5) -bor ($p[2] -shr 3)
            $seen[$key] = 1
        }
    }
    return $seen.Count
}
Write-Output "结构事实（黑边/边缘色）:"
Test-BarColor $imgA $LabelA ([int](($minX + $maxX) / 2))
Test-BarColor $imgB $LabelB ([int](($minX + $maxX) / 2))
Write-Output "内容盒量化颜色种类数: $LabelA=$((Get-ColorCount $imgA $minY $maxY))  $LabelB=$((Get-ColorCount $imgB $minY $maxY))"

if ($diffBitmap) {
    $diffBitmap.Save($Diff, [System.Drawing.Imaging.ImageFormat]::Png)
    $diffBitmap.Dispose()
    Write-Output "差异热图: $Diff"
}
$imgA.Source.Dispose()
$imgB.Source.Dispose()
