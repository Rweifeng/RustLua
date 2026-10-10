# check_asset_baseline.ps1
# 《江湖有田》阶段4 资产占位验收基线（独立脚本，不进 verify.ps1）。
#
# 用途：按 docs/58 资产台账约定的占位路径，扫描项目真实注册表要求的资产是否已落地，
#       输出「已存在 / 缺失」清单，并以退出码反映基线是否全绿。
#
# 用法：
#   powershell -File scripts/check_asset_baseline.ps1                # 默认脚本所在项目根
#   powershell -File scripts/check_asset_baseline.ps1 C:\path\game_jhyt
#
# 退出码：0 = 台账登记的占位资产路径全部存在；1 = 存在缺失（基线未达，属阶段4预期状态）。
#
# 注意：本脚本只读不写；不修改 verify.ps1 / main.lua / validate_data.lua 或任何 Lua 数据。

param(
    [string]$ProjectRoot = ""
)

$ErrorActionPreference = "Stop"

# 定位项目根：默认 = 脚本目录的上一级（scripts/ 的父目录）。
if ([string]::IsNullOrEmpty($ProjectRoot)) {
    $ProjectRoot = Split-Path -Parent $PSScriptRoot
}
$ProjectRoot = (Resolve-Path $ProjectRoot).Path

function P($rel) { return Join-Path $ProjectRoot $rel }

$script:present = @()   # 已存在
$script:missing = @()   # 缺失

function Test-Asset([string]$rel) {
    $full = P $rel
    if (Test-Path $full) { $script:present += $rel } else { $script:missing += $rel }
}

Write-Host "== 江湖有田 阶段4 资产占位基线 =="
Write-Host ("项目根: " + $ProjectRoot)
Write-Host ""

# ---------------------------------------------------------------------------
# 1) 场景：scenes/<id>.scene（应全部存在）+ 对应 tileset 目录
# ---------------------------------------------------------------------------
Write-Host "[1/5] 场景 tileset（来源 scenes/*.scene）"
$sceneDir = P "scenes"
if (Test-Path $sceneDir) {
    $scenes = Get-ChildItem $sceneDir -Filter *.scene | ForEach-Object { $_.BaseName }
    # scene id -> tileset 目录映射（实测目录名，见 docs/58 §1.1）
    $tileMap = @{
        "manor"="manor"; "village"="village";
        # 山道目录随 M72-11 的逻辑瓦图集改名：tilesets/mountain（6 列直接寻址）→
        # tilesets/mountain_path（12x9 autotile）。留着旧名会把一个已删的目录算成 MISSING。
        "mountain_path"="mountain_path";
        "cold_spring_cave"="cave"; "interior_herb"="interior_herb";
        "interior_forge"="interior_forge"
    }
    foreach ($s in ($scenes | Sort-Object)) {
        Test-Asset ("scenes\" + $s + ".scene")
        $td = $tileMap[$s]
        if ($td) { Test-Asset ("assets\art\tilesets\" + $td) }
    }
}

# ---------------------------------------------------------------------------
# 2) 角色序列帧：data/npcs.lua 的 persist_id -> characters/<id>/static_s.png
# ---------------------------------------------------------------------------
Write-Host "[2/5] 角色序列帧（来源 data/npcs.lua）"
$npcsFile = P "scripts\data\npcs.lua"
if (Test-Path $npcsFile) {
    $raw = Get-Content $npcsFile -Encoding UTF8
    foreach ($line in $raw) {
        if ($line -match 'persist_id\s*=\s*"npc_([a-z0-9_]+)"') {
            $char = $Matches[1]
            Test-Asset ("assets\art\characters\" + $char + "\static_s.png")
        }
    }
    # 主角两套形象
    Test-Asset "assets\art\characters\player_male\static_s.png"
    Test-Asset "assets\art\characters\player_female\static_s.png"
}

# ---------------------------------------------------------------------------
# 3) 作物阶段 sprite：data/crops.lua 顶层键 -> crops/<id>/stage_1.png
# ---------------------------------------------------------------------------
Write-Host "[3/5] 作物阶段 sprite（来源 data/crops.lua）"
$cropsFile = P "scripts\data\crops.lua"
if (Test-Path $cropsFile) {
    $raw = Get-Content $cropsFile -Encoding UTF8
    foreach ($line in $raw) {
        if ($line -match '^\s{2}([a-z0-9_]+)\s*=\s*\{') {
            $cid = $Matches[1]
            Test-Asset ("assets\art\crops\" + $cid + "\stage_1.png")
        }
    }
}

# ---------------------------------------------------------------------------
# 4) 物品图标：各 data/items*.lua 的 icon= "assets/art/items/<id>.png"
# ---------------------------------------------------------------------------
Write-Host "[4/5] 物品图标（来源 data/items*.lua 的 icon= 字段）"
$dataDir = P "scripts\data"
$itemFiles = Get-ChildItem $dataDir -Filter "items*.lua"
foreach ($f in $itemFiles) {
    $raw = Get-Content $f.FullName -Encoding UTF8
    foreach ($line in $raw) {
        if ($line -match 'icon\s*=\s*"([^"]+)"') {
            $iconRel = $Matches[1] -replace '/', '\'
            Test-Asset $iconRel
        }
    }
}

# ---------------------------------------------------------------------------
# 5) 动物 / 鱼类：data/animals.lua、data/fish.lua 顶层键
# ---------------------------------------------------------------------------
Write-Host "[5/5] 动物与鱼类（来源 data/animals.lua、data/fish.lua）"
$animalsFile = P "scripts\data\animals.lua"
if (Test-Path $animalsFile) {
    $raw = Get-Content $animalsFile -Encoding UTF8
    foreach ($line in $raw) {
        if ($line -match '^\s{2}([a-z0-9_]+)\s*=\s*\{') {
            $aid = $Matches[1]
            Test-Asset ("assets\art\animals\" + $aid + "\static_s.png")
        }
    }
}
$fishFile = P "scripts\data\fish.lua"
if (Test-Path $fishFile) {
    $raw = Get-Content $fishFile -Encoding UTF8
    foreach ($line in $raw) {
        if ($line -match '^\s{2}([a-z0-9_]+)\s*=\s*\{[^}]*water\s*=') {
            $fid = $Matches[1]
            Test-Asset ("assets\art\items\" + $fid + ".png")
        }
    }
}

# ---------------------------------------------------------------------------
# 汇总
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "------------------------------------------------------------"
Write-Host ("已存在 (OK): " + $script:present.Count)
Write-Host ("缺失   (MISSING): " + $script:missing.Count)
Write-Host "------------------------------------------------------------"

if ($script:missing.Count -gt 0) {
    Write-Host ""
    Write-Host "缺失清单（前 40 条，完整见上方计数）："
    $script:missing | Select-Object -First 40 | ForEach-Object { Write-Host ("  MISS  " + $_) }
    Write-Host ""
    Write-Host "基线结果: 未达（存在缺失项）。阶段4 美术本体为外部委托，属预期状态。"
    exit 1
} else {
    Write-Host "基线结果: 全绿。"
    exit 0
}
