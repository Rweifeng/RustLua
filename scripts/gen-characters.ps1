# Generate the M6-06 demo characters/items: a 16x16 hero (idle/walk/attack),
# a slime enemy, and a gold coin. Plain placeholder art until real character
# sprites are wired; the environment tiles come from Kenney CC0 assets.
Add-Type -AssemblyName System.Drawing

$assets = "C:\GitHub\p2\game\assets\textures"
$cell = 16

function New-Hero($frame, $mode) {
    # mode: "idle" | "move" | "attack"; frame is the column index
    $bmp = New-Object System.Drawing.Bitmap $cell, $cell
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.Clear([System.Drawing.Color]::Transparent)
    $skin = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(244, 204, 160))
    $shirt = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(70, 130, 220))
    $pants = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(55, 65, 90))
    $dark = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(40, 45, 55))
    $bob = if ($mode -eq "idle" -and ($frame % 2) -eq 1) { -1 } else { 0 }
    # head
    $g.FillRectangle($skin, 5, 2 + $bob, 6, 5)
    $g.FillRectangle($dark, 5, 2 + $bob, 6, 2)   # hair
    # torso
    $g.FillRectangle($shirt, 4, 7 + $bob, 8, 6)
    # legs (stride for move)
    $lift = if ($mode -eq "move") { (($frame + 1) % 4) } else { 0 }
    $legShift = @(0, 2, 0, -2)[$lift]
    $g.FillRectangle($pants, 5 + $legShift, 13 + $bob, 2, 3)
    $g.FillRectangle($pants, 9 - $legShift, 13 + $bob, 2, 3)
    # sword extends forward on attack frames
    if ($mode -eq "attack") {
        $g.FillRectangle($dark, 8, 5 + $bob, 6 + $frame * 3, 1)
    } else {
        $g.FillRectangle($skin, 3, 9 + $bob, 2, 3)  # arm
        $g.FillRectangle($skin, 11, 9 + $bob, 2, 3)
    }
    $g.Dispose(); $skin.Dispose(); $shirt.Dispose(); $pants.Dispose(); $dark.Dispose()
    return $bmp
}

function Write-Strip($name, $mode, $cols) {
    $strip = New-Object System.Drawing.Bitmap ($cell * $cols), $cell
    $g = [System.Drawing.Graphics]::FromImage($strip)
    for ($f = 0; $f -lt $cols; $f++) {
        $hero = New-Hero $f $mode
        $g.DrawImage($hero, $f * $cell, 0, $cell, $cell)
        $hero.Dispose()
    }
    $g.Dispose()
    $path = Join-Path $assets $name
    $strip.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $strip.Dispose()
    Write-Output "$name saved"
}

Write-Strip "hero-idle.png" "idle" 4
Write-Strip "hero-move.png" "move" 4
Write-Strip "hero-attack.png" "attack" 2

# slime: green blob with eyes, 2 squish frames
$slime = New-Object System.Drawing.Bitmap ($cell * 2), $cell
$g = [System.Drawing.Graphics]::FromImage($slime)
$green = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(120, 200, 90))
$white = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
$black = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::Black)
foreach ($f in 0,1) {
    $x = $f * $cell
    $h = 8 + $f * 3   # frame 0 taller, frame 1 squished
    $g.FillEllipse($green, $x + 3, $cell - $h, 10, $h)
    $g.FillEllipse($white, $x + 5, $cell - $h + 2, 2, 2)
    $g.FillEllipse($white, $x + 9, $cell - $h + 2, 2, 2)
    $g.FillEllipse($black, $x + 5, $cell - $h + 2, 1, 1)
    $g.FillEllipse($black, $x + 9, $cell - $h + 2, 1, 1)
}
$g.Dispose(); $green.Dispose(); $white.Dispose(); $black.Dispose()
$slime.Save((Join-Path $assets "slime.png"), [System.Drawing.Imaging.ImageFormat]::Png)
$slime.Dispose()
Write-Output "slime.png saved"

# coin: gold circle with inner ring
$coin = New-Object System.Drawing.Bitmap $cell, $cell
$g = [System.Drawing.Graphics]::FromImage($coin)
$g.Clear([System.Drawing.Color]::Transparent)
$gold = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(240, 200, 60))
$light = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(250, 225, 130))
$g.FillEllipse($gold, 2, 2, 12, 12)
$g.FillEllipse($light, 4, 4, 8, 8)
$g.Dispose(); $gold.Dispose(); $light.Dispose()
$coin.Save((Join-Path $assets "coin.png"), [System.Drawing.Imaging.ImageFormat]::Png)
$coin.Dispose()
Write-Output "coin.png saved"
