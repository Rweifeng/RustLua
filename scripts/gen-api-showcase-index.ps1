# 重新生成示例工程的**入口菜单场景** `examples/game_api/scenes/index.scene`。
#
# 为什么需要它：入口菜单的卡片是**场景实体**（点击命中要 `UiEvents` 组件在读几何），
# 而卡片清单必须与 `scripts/demos.lua`（演示清单的唯一真源）一致——两处手写必然漂移。
# 这个生成器直接读 `demos.lua`，把 `scene` / `label` 铺成 2 列网格：
#
#     pwsh -File scripts/gen-api-showcase-index.ps1
#
# 一致性还有一条门兜底：`cargo test -p engine sample_project_index_menu_lists_every_scene`
# 断言"demos.lua ↔ scenes/*.scene ↔ 入口菜单卡片"三者互相对齐。
#
# 布局参数（2 列 × N 行）：卡片 560×50，列左边缘 x=120 / x=700，行距 58，首行 y=148。
# 屏幕基准 1280×720，`UiLayout` 的锚点是屏幕中心，偏移因此是"相对中心的像素"。
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$demosLua = Join-Path $repoRoot 'examples/game_api/scripts/demos.lua'
$target = Join-Path $repoRoot 'examples/game_api/scenes/index.scene'

$entries = @()
$current = @{}
foreach ($line in Get-Content $demosLua) {
    $trimmed = $line.Trim()
    if ($trimmed -match '^scene = "([^"]+)"') { $current.scene = $Matches[1] }
    elseif ($trimmed -match '^label = "([^"]+)"') {
        $current.label = $Matches[1]
        $entries += [pscustomobject]$current
        $current = @{}
    }
}
if ($entries.Count -lt 10) { throw "从 demos.lua 只解析出 $($entries.Count) 条演示，清单格式变了？" }
foreach ($entry in $entries) {
    if (-not $entry.scene -or -not $entry.label) { throw "demos.lua 条目缺少 scene/label：$($entry | ConvertTo-Json -Compress)" }
    if (-not (Test-Path (Join-Path $repoRoot "examples/game_api/scenes/$($entry.scene).scene"))) {
        throw "demos.lua 列出的场景不存在：$($entry.scene)"
    }
}

$identity = 'position=0.000000000,0.000000000,0.000000000;rotation=0.000000000,0.000000000,0.000000000,1.000000000;scale=1.000000000,1.000000000,1.000000000'
$colors = @('0.130,0.360,0.640','0.210,0.260,0.470','0.180,0.380,0.350')
$layout = 'anchors=0.5000,0.5000,0.5000,0.5000;offsets={0},{1},{2},{3};min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.5000,0.5000'
$point = 'anchors=0.5000,0.5000,0.5000,0.5000;offsets={0},{1},{0},{1};min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.5000,0.5000'

$out = New-Object System.Collections.Generic.List[string]
$out.Add('scene_version=2')
$out.Add('name=index')
$rootIds = @('1','2')
$body = New-Object System.Collections.Generic.List[string]
# 背景**不是场景实体**（历史决定，这里记下它当年的前提与今天还剩哪一半）：
# 2026-10-07 之前 `push_scene_ui` 排在全部 `push_ui_*` 之后 ⇒ 场景里的 `Image`/`Text` 一律画在
# Lua 动态 UI **之上**，铺满屏幕的场景背景会把标题/状态/提示行整片盖住。现在图趟挪到了动态层
# 之下（`push_scene_ui_images` → `push_ui_*` → `push_scene_ui_text`），文字趟仍在最最后 ⇒
# "场景 `Text` 盖住动态 UI"这半仍然成立，背景这一半已经翻过来。
# 保留动态面板而不搬回场景实体：一搬就要改 `.scene` 字节、把一屏已迁的层序重排一遍，而它今天
# 买不到任何好处；`ui.panel` 在动态批里排最前（id 字典序），既压不住卡片，也不会被动态文本盖住。
# 这条约束有门兜底（`sample_project_shared_overlay_is_not_covered`，其注释写明它仍按大面积相交
# 一刀切：黄金读数分不清那块矩形落在哪一档，宁可严)。
$body.Add("entity`t1`tTitle")
$body.Add("component`t1`tText`ttext=RustLua API Showcase;font_size=30.00;color=0.950,0.870,0.500")
$body.Add("component`t1`tTransform`t$identity")
$body.Add("component`t1`tUiLayout`t" + ($point -f '-165.00','-330.00'))
$body.Add("entity`t2`tSubtitle")
$body.Add("component`t2`tText`ttext=One focused scene per API module - click a card (or ENTER on the focused one), ESC returns here;font_size=13.00;color=0.610,0.700,0.850")
$body.Add("component`t2`tTransform`t$identity")
$body.Add("component`t2`tUiLayout`t" + ($point -f '-273.00','-296.00'))

$id = 2
for ($i = 0; $i -lt $entries.Count; $i++) {
    $column = $i % 2
    $row = [math]::Floor($i / 2)
    $top = 148 + $row * 58
    $left = if ($column -eq 0) { 120 } else { 700 }
    $card = $id + 1
    $labelId = $id + 2
    $id = $labelId
    $name = $entries[$i].scene
    $body.Add("entity`t$card`t$name")
    $body.Add("component`t$card`tImage`ttexture=__white__;color=$($colors[$i % 3])")
    $body.Add("component`t$card`tTransform`t$identity")
    # 实体名 = 目标场景名，`function=on_click` 收实体名，因此点击回调可以直接 scene.load(name)。
    $body.Add("component`t$card`tUiEvents`tevent=click;function=on_click")
    $body.Add("component`t$card`tUiLayout`t" + ($layout -f (($left - 640).ToString('0.00')), (($top - 360).ToString('0.00')), (($left + 560 - 640).ToString('0.00')), (($top + 50 - 360).ToString('0.00'))))
    $body.Add("entity`t$labelId`t${name}_label`t$card")
    $body.Add("component`t$labelId`tText`ttext=$($entries[$i].label);font_size=14.00;color=0.960,0.970,1.000")
    $body.Add("component`t$labelId`tTransform`t$identity")
    $body.Add("component`t$labelId`tUiLayout`t" + ($point -f (($left + 18 - 640).ToString('0.00')), (($top + 15 - 360).ToString('0.00'))))
    $rootIds += "$card"
}

$footer = $id + 1
$body.Add("entity`t$footer`tFooter")
$body.Add("component`t$footer`tText`ttext=Arrows/TAB move the focus ring - wheel scrolls the API list - F10 quits the app;font_size=12.00;color=0.560,0.640,0.760")
$body.Add("component`t$footer`tTransform`t$identity")
$body.Add("component`t$footer`tUiLayout`t" + ($point -f '-250.00','250.00'))
$rootIds += "$footer"

$out.Add("meta`teditor.root_order`t" + ($rootIds -join ','))
$out.AddRange($body)
[System.IO.File]::WriteAllText($target, ($out -join "`n") + "`n", (New-Object System.Text.UTF8Encoding($false)))
Write-Host "wrote $target ($($entries.Count) cards, last entity id $footer)"
