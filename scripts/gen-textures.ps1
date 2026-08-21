# One-off dev tool: generate placeholder textures for the M6 texture slice.
# tileset.png  = 192x32, 6 cells of 32px, colors matching the old code palette
# player.png   = 32x32, cyan with a white border + cross so UV mapping is visible
# {idle,move}-sheet.png = 1x4 frame strips, {attack,hit}-sheet.png = 1x2 strips.
# Each frame carries a white bar whose position marks the frame index.
Add-Type -AssemblyName System.Drawing

$root = Split-Path -Parent $PSScriptRoot
$assets = Join-Path $root "game\assets\textures"

# palette order: cyan, orange, green, red, purple, yellow
$colors = @(
    [System.Drawing.Color]::FromArgb(46, 184, 255),
    [System.Drawing.Color]::FromArgb(242, 166, 38),
    [System.Drawing.Color]::FromArgb(89, 217, 102),
    [System.Drawing.Color]::FromArgb(230, 89, 115),
    [System.Drawing.Color]::FromArgb(179, 115, 242),
    [System.Drawing.Color]::FromArgb(230, 217, 89)
)

# --- tileset: 6x1 strip of solid cells ---
$cell = 32
$bmp = New-Object System.Drawing.Bitmap ($cell * $colors.Length), $cell
for ($i = 0; $i -lt $colors.Length; $i++) {
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $brush = New-Object System.Drawing.SolidBrush $colors[$i]
    $g.FillRectangle($brush, $i * $cell, 0, $cell, $cell)
    # darker edge on each cell so UV boundaries are visible
    $edge = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, [math]::Max(0, $colors[$i].R - 60), [math]::Max(0, $colors[$i].G - 60), [math]::Max(0, $colors[$i].B - 60)))
    $g.FillRectangle($edge, $i * $cell, 0, $cell, 2)
    $g.FillRectangle($edge, $i * $cell, $cell - 2, $cell, 2)
    $g.FillRectangle($edge, $i * $cell, 0, 2, $cell)
    $g.FillRectangle($edge, ($i + 1) * $cell - 2, 0, 2, $cell)
    $g.Dispose()
    $brush.Dispose()
    $edge.Dispose()
}
$tilesetPath = Join-Path $assets "tileset.png"
$bmp.Save($tilesetPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "tileset -> $tilesetPath"

# --- player: 32x32 cyan with white cross ---
$bmp = New-Object System.Drawing.Bitmap $cell, $cell
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.Clear([System.Drawing.Color]::FromArgb(46, 184, 255))
$white = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
# white cross through the center
$g.FillRectangle($white, 13, 0, 6, $cell)
$g.FillRectangle($white, 0, 13, $cell, 6)
$g.Dispose()
$white.Dispose()
$playerPath = Join-Path $assets "player.png"
$bmp.Save($playerPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "player -> $playerPath"

# --- animation frame strips: one row per state, white bar shows the frame index ---
$stateFrames = @(
    @{ name = "idle";   cols = 4; color = $colors[0] },
    @{ name = "move";   cols = 4; color = $colors[1] },
    @{ name = "attack"; cols = 2; color = $colors[2] },
    @{ name = "hit";    cols = 2; color = $colors[3] }
)
$white = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
foreach ($state in $stateFrames) {
    $bmp = New-Object System.Drawing.Bitmap ($cell * $state.cols), $cell
    for ($f = 0; $f -lt $state.cols; $f++) {
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $brush = New-Object System.Drawing.SolidBrush $state.color
        $g.FillRectangle($brush, $f * $cell, 0, $cell, $cell)
        # white bar moves left-to-right across frames so playback is visible
        $g.FillRectangle($white, $f * $cell + $f * 8, 8, 6, 16)
        $g.Dispose()
        $brush.Dispose()
    }
    $stripPath = Join-Path $assets ($state.name + "-sheet.png")
    $bmp.Save($stripPath, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Output ("{0}-sheet -> {1}" -f $state.name, $stripPath)
}
$white.Dispose()
