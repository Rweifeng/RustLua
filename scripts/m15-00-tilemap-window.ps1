[CmdletBinding()]
param(
    [int]$Width = 100,
    [int]$Height = 100,
    [string]$OutDir = "",
    [int]$DurationSeconds = 15,
    [int]$SampleIntervalSeconds = 3,
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$workspaceRoot = Split-Path -Parent $PSScriptRoot
$sourceProject = Join-Path $workspaceRoot "game_test_2d_production"
$variant = "{0}x{1}" -f $Width, $Height
$benchRoot = Join-Path $workspaceRoot ("artifacts\m15\tilemap_window_project_{0}" -f $variant)

if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path $workspaceRoot "docs\acceptance-evidence\m15"
}

if ($Width -le 0 -or $Height -le 0) {
    throw "Width and Height must be greater than zero."
}

function Reset-Dir([string]$Path) {
    if (Test-Path $Path) {
        Remove-Item -LiteralPath $Path -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $Path | Out-Null
}

function New-TilemapComponent([int]$MapWidth, [int]$MapHeight) {
    $tileChars = New-Object System.Collections.Generic.List[char]
    $solidChars = New-Object System.Collections.Generic.List[char]
    $tileChars.Capacity = $MapWidth * $MapHeight
    $solidChars.Capacity = $MapWidth * $MapHeight
    for ($y = 0; $y -lt $MapHeight; $y++) {
        for ($x = 0; $x -lt $MapWidth; $x++) {
            $tileValue = (($x + $y) % 6) + 1
            $solidValue = if ((($x + $y) % 7) -eq 0) { '1' } else { '0' }
            $tileChars.Add([char]([int][char]'0' + $tileValue))
            $solidChars.Add([char]$solidValue)
        }
    }

    $tiles = [string]::new($tileChars.ToArray())
    $solid = [string]::new($solidChars.ToArray())
    return "tile_size=32x32;width=$MapWidth;height=$MapHeight;texture=assets/textures/tileset.png;tiles=$tiles;solid=$solid"
}

Reset-Dir $benchRoot
New-Item -ItemType Directory -Force -Path (Join-Path $benchRoot "assets\textures") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $benchRoot "assets\fonts") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $benchRoot "scenes") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $benchRoot "scripts") | Out-Null

Copy-Item -Path (Join-Path $sourceProject "assets\textures\tileset.png") -Destination (Join-Path $benchRoot "assets\textures\tileset.png") -Force
Copy-Item -Path (Join-Path $sourceProject "assets\fonts\NotoSansSC-VF.ttf") -Destination (Join-Path $benchRoot "assets\fonts\NotoSansSC-VF.ttf") -Force

@"
[project]
name = "m15_tilemap_window_$variant"
version = "0.1.0"
engine = "p2"
main_script = "scripts/main.lua"
startup_scene = "scenes/main.scene"
"@ | Set-Content -Path (Join-Path $benchRoot "project.toml") -Encoding ASCII

@"
scene_version=2
name=tilemap_$variant
entity  1       Tilemap
component       1       Tilemap $(New-TilemapComponent -MapWidth $Width -MapHeight $Height)
component       1       Transform       x=0.000000000;y=0.000000000;rotation=0.000000000;scale_x=1.000000000;scale_y=1.000000000
"@ | Set-Content -Path (Join-Path $benchRoot "scenes\main.scene") -Encoding ASCII

@"
function update()
    return "tilemap_window_idle"
end
"@ | Set-Content -Path (Join-Path $benchRoot "scripts\main.lua") -Encoding ASCII

$realWindowScript = Join-Path $workspaceRoot "scripts\m15-00-real-window.ps1"
$args = @(
    "-ExecutionPolicy", "Bypass",
    "-File", $realWindowScript,
    "-ProjectRoot", $benchRoot,
    "-OutDir", $OutDir,
    "-SkipEditor",
    "-DurationSeconds", $DurationSeconds,
    "-SampleIntervalSeconds", $SampleIntervalSeconds
)
if ($SkipBuild) {
    $args += "-SkipBuild"
}

Write-Host "Tilemap window benchmark variant: $variant" -ForegroundColor Cyan
Write-Host "Tilemap window benchmark project: $benchRoot" -ForegroundColor Cyan
& powershell @args
if ($LASTEXITCODE -ne 0) {
    throw "Tilemap window benchmark failed."
}
