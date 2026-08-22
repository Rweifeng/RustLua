# Generate the sample-game characters/items: a 16x16 hero (idle/walk/attack),
# a slime enemy, and a gold coin. Upgraded programmatic art: dark outlines,
# shading, and face details. Frame layout matches the scene Animation clips.
# ASCII-only to avoid ANSI parsing issues.

Add-Type -AssemblyName System.Drawing

$root = Split-Path -Parent $PSScriptRoot
$assets = Join-Path $root "game\assets\textures"
$cell = 16

function Out-Rect([System.Drawing.Graphics]$g, [int]$x, [int]$y, [int]$w, [int]$h,
    [System.Drawing.Color]$fill, [System.Drawing.Color]$edge) {
    $g.FillRectangle((New-Object System.Drawing.SolidBrush $fill), $x, $y, $w, $h)
    if ($edge.ToArgb() -ne 0) {
        $pen = New-Object System.Drawing.Pen $edge, 1
        $g.DrawRectangle($pen, $x, $y, $w - 1, $h - 1)
        $pen.Dispose()
    }
}

function Out-Ellipse([System.Drawing.Graphics]$g, [int]$x, [int]$y, [int]$w, [int]$h,
    [System.Drawing.Color]$fill, [System.Drawing.Color]$edge) {
    $g.FillEllipse((New-Object System.Drawing.SolidBrush $fill), $x, $y, $w, $h)
    if ($edge.ToArgb() -ne 0) {
        $pen = New-Object System.Drawing.Pen $edge, 1
        $g.DrawEllipse($pen, $x, $y, $w, $h)
        $pen.Dispose()
    }
}

$edge = [System.Drawing.Color]::FromArgb(28, 30, 34)
$edgeSoft = [System.Drawing.Color]::FromArgb(40, 44, 50)

function New-Hero($frame, $mode) {
    # mode: "idle" | "move" | "attack"
    $bmp = New-Object System.Drawing.Bitmap $cell, $cell
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.Clear([System.Drawing.Color]::Transparent)
    $skin = [System.Drawing.Color]::FromArgb(244, 204, 160)
    $skinShade = [System.Drawing.Color]::FromArgb(214, 172, 128)
    $hair = [System.Drawing.Color]::FromArgb(60, 48, 40)
    $shirt = [System.Drawing.Color]::FromArgb(70, 130, 220)
    $shirtDark = [System.Drawing.Color]::FromArgb(48, 98, 180)
    $pants = [System.Drawing.Color]::FromArgb(55, 65, 90)
    $pantsDark = [System.Drawing.Color]::FromArgb(40, 48, 68)
    $blade = [System.Drawing.Color]::FromArgb(210, 215, 225)
    $bob = if ($mode -eq "idle" -and ($frame % 2) -eq 1) { -1 } else { 0 }

    # head
    Out-Rect $g 5 (2 + $bob) 6 5 $skin $edge
    Out-Rect $g 5 (2 + $bob) 6 2 $hair $edge          # hair
    Out-Rect $g 6 (5 + $bob) 1 1 $skinShade $edgeSoft   # ear shade
    # face: eyes
    Out-Rect $g 6 (4 + $bob) 1 1 ([System.Drawing.Color]::White) $edge
    Out-Rect $g 9 (4 + $bob) 1 1 ([System.Drawing.Color]::White) $edge
    Out-Rect $g 6 (4 + $bob) 1 1 ([System.Drawing.Color]::FromArgb(30, 30, 40)) ([System.Drawing.Color]::Transparent)
    Out-Rect $g 9 (4 + $bob) 1 1 ([System.Drawing.Color]::FromArgb(30, 30, 40)) ([System.Drawing.Color]::Transparent)
    # torso (with bottom shade)
    Out-Rect $g 4 (7 + $bob) 8 6 $shirt $edge
    Out-Rect $g 4 (11 + $bob) 8 2 $shirtDark ([System.Drawing.Color]::Transparent)
    # belt
    Out-Rect $g 4 (10 + $bob) 8 1 ([System.Drawing.Color]::FromArgb(120, 90, 40)) ([System.Drawing.Color]::Transparent)
    # legs (stride on move frames)
    $lift = if ($mode -eq "move") { (($frame + 1) % 4) } else { 0 }
    $legShift = @(0, 2, 0, -2)[$lift]
    Out-Rect $g (5 + $legShift) (13 + $bob) 2 3 $pants $edge
    Out-Rect $g (9 - $legShift) (13 + $bob) 2 3 $pants $edge
    Out-Rect $g (5 + $legShift) (15 + $bob) 2 1 $pantsDark ([System.Drawing.Color]::Transparent)
    Out-Rect $g (9 - $legShift) (15 + $bob) 2 1 $pantsDark ([System.Drawing.Color]::Transparent)
    if ($mode -eq "attack") {
        # sword extends forward on attack frames
        $swordLen = 5 + $frame * 3
        Out-Rect $g 8 (5 + $bob) $swordLen 1 $blade $edge
        Out-Rect $g (7 + $swordLen) (5 + $bob) 1 1 ([System.Drawing.Color]::FromArgb(120, 130, 140)) $edge
    } else {
        Out-Rect $g 3 (9 + $bob) 2 3 $skin $edge        # left arm
        Out-Rect $g 11 (9 + $bob) 2 3 $skin $edge       # right arm
        Out-Rect $g 3 (11 + $bob) 2 1 $skinShade ([System.Drawing.Color]::Transparent)
        Out-Rect $g 11 (11 + $bob) 2 1 $skinShade ([System.Drawing.Color]::Transparent)
    }
    $g.Dispose()
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

# slime: green blob with outline, highlight, eyes and a mouth; 2 squish frames
$slime = New-Object System.Drawing.Bitmap ($cell * 2), $cell
$g = [System.Drawing.Graphics]::FromImage($slime)
$base = [System.Drawing.Color]::FromArgb(110, 190, 80)
$dark = [System.Drawing.Color]::FromArgb(70, 140, 50)
$light = [System.Drawing.Color]::FromArgb(150, 220, 110)
$white = [System.Drawing.Color]::White
$black = [System.Drawing.Color]::FromArgb(25, 25, 30)
foreach ($f in 0, 1) {
    $x = $f * $cell
    $h = 8 + $f * 3
    # body (two-tone: darker bottom)
    Out-Ellipse $g ($x + 3) ($cell - $h) 10 $h $base $edge
    Out-Ellipse $g ($x + 3) ($cell - 3) 10 3 $dark ([System.Drawing.Color]::Transparent)
    # highlight on top
    Out-Ellipse $g ($x + 5) ($cell - $h + 1) 3 2 $light ([System.Drawing.Color]::Transparent)
    # eyes + pupils
    Out-Ellipse $g ($x + 5) ($cell - $h + 2) 2 2 $white $edge
    Out-Ellipse $g ($x + 9) ($cell - $h + 2) 2 2 $white $edge
    Out-Ellipse $g ($x + 5) ($cell - $h + 2) 1 1 $black ([System.Drawing.Color]::Transparent)
    Out-Ellipse $g ($x + 9) ($cell - $h + 2) 1 1 $black ([System.Drawing.Color]::Transparent)
    # mouth
    Out-Ellipse $g ($x + 7) ($cell - 3) 2 1 $black ([System.Drawing.Color]::Transparent)
}
$g.Dispose()
$slime.Save((Join-Path $assets "slime.png"), [System.Drawing.Imaging.ImageFormat]::Png)
$slime.Dispose()
Write-Output "slime.png saved"

# coin: gold disc with outline, top highlight, and a shine dot
$coin = New-Object System.Drawing.Bitmap $cell, $cell
$g = [System.Drawing.Graphics]::FromImage($coin)
$g.Clear([System.Drawing.Color]::Transparent)
$gold = [System.Drawing.Color]::FromArgb(235, 195, 55)
$goldDark = [System.Drawing.Color]::FromArgb(190, 150, 30)
$goldLight = [System.Drawing.Color]::FromArgb(255, 230, 140)
Out-Ellipse $g 2 2 12 12 $gold $edge
Out-Ellipse $g 3 3 10 10 ([System.Drawing.Color]::Transparent) ([System.Drawing.Color]::FromArgb(200, 170, 60))
Out-Ellipse $g 5 5 6 6 $goldLight ([System.Drawing.Color]::Transparent)
Out-Ellipse $g 5 5 6 6 $goldDark ([System.Drawing.Color]::Transparent)
Out-Ellipse $g 6 6 4 4 $gold ([System.Drawing.Color]::Transparent)
# shine
Out-Ellipse $g 5 5 1 1 ([System.Drawing.Color]::White) ([System.Drawing.Color]::Transparent)
Out-Ellipse $g 10 10 1 1 $goldLight ([System.Drawing.Color]::Transparent)
$g.Dispose()
$coin.Save((Join-Path $assets "coin.png"), [System.Drawing.Imaging.ImageFormat]::Png)
$coin.Dispose()
Write-Output "coin.png saved"
