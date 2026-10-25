# 重新生成示例工程的**入口菜单场景** `examples/game_api/scenes/index.scene`。
#
# 为什么需要它：入口菜单的卡片是**场景实体**（点击命中要 `UiEvents` 组件在读几何），
# 而卡片清单必须与 `scripts/demos.lua`（演示清单的唯一真源）一致——两处手写必然漂移。
# 这个生成器直接读 `demos.lua`，把 scene / category / title / label / steps 铺成
# **3 列 × 5 行**卡片网格，右侧留出运行期 API 索引（由 `demo_index.lua` 动态绘制）。
#
#     pwsh -File scripts/docgen/gen-api-showcase-index.ps1
#
# 一致性还有一条门兜底：`cargo test -p engine sample_project_index_menu_lists_every_scene`
# 断言"demos.lua ↔ scenes/*.scene ↔ 入口菜单卡片"三者互相对齐。
#
# 网格几何（列数、卡片尺寸、卡内三行的位置与字号）来自 `scripts/menu_grid.lua`——
# 焦点环在 Lua 侧 require 同一份，两处不可能漂移。
#
# **子实体的 UiLayout 偏移是相对父矩形的**（Unity 式锚点）：卡片挂在 Canvas 上，
# Canvas 是"屏幕中心的零尺寸矩形"，所以卡片偏移等于屏幕坐标；而卡片里的三行文字挂在
# 卡片上，偏移必须写成"相对卡片中心"。`pivot=0,0` 让点锚点成为文字的左上角
# （零尺寸锚点下 pivot 0.5 会把文字**居中**到该点，长标题因此会偏出屏幕左缘）。
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$demosLua = Join-Path $repoRoot 'examples/game_api/scripts/demos.lua'
$gridLua = Join-Path $repoRoot 'examples/game_api/scripts/menu_grid.lua'
$target = Join-Path $repoRoot 'examples/game_api/scenes/index.scene'

$text = [System.IO.File]::ReadAllText($demosLua)
# 字段顺序由 demos.lua 的写法固定：scene → module → category → title → label → summary → steps。
$pattern = 'scene\s*=\s*"([^"]+)"[\s\S]*?module\s*=\s*"([^"]+)"[\s\S]*?category\s*=\s*"([^"]+)"[\s\S]*?title\s*=\s*"([^"]+)"[\s\S]*?label\s*=\s*"([^"]+)"[\s\S]*?summary\s*=\s*"[^"]*"[\s\S]*?steps\s*=\s*(\d+)'
$entries = @()
foreach ($m in [regex]::Matches($text, $pattern)) {
    $entries += [pscustomobject]@{
        scene    = $m.Groups[1].Value
        module   = $m.Groups[2].Value
        category = $m.Groups[3].Value
        title    = $m.Groups[4].Value
        label    = $m.Groups[5].Value
        steps    = [int]$m.Groups[6].Value
    }
}
if ($entries.Count -lt 10) { throw "从 demos.lua 只解析出 $($entries.Count) 条演示，清单格式变了？" }
foreach ($entry in $entries) {
    if (-not $entry.scene -or -not $entry.label) { throw "demos.lua 条目缺少 scene/label：$($entry | ConvertTo-Json -Compress)" }
    if (-not (Test-Path (Join-Path $repoRoot "examples/game_api/scenes/$($entry.scene).scene"))) {
        throw "demos.lua 列出的场景不存在：$($entry.scene)"
    }
}

# 网格几何：从 menu_grid.lua 取扁平数字，缺一个就报错而不是回退到默认值。
$gridText = [System.IO.File]::ReadAllText($gridLua)
$grid = @{}
foreach ($gm in [regex]::Matches($gridText, '(?m)^\s*(\w+)\s*=\s*([0-9.]+)\s*,?\s*$')) {
    $grid[$gm.Groups[1].Value] = [double]$gm.Groups[2].Value
}
foreach ($key in @('columns', 'left', 'top', 'card_w', 'card_h', 'col_w', 'row_h', 'pad_x',
                   'title_dy', 'label_dy', 'meta_dy', 'font_title', 'font_label', 'font_meta')) {
    if (-not $grid.ContainsKey($key)) { throw "menu_grid.lua 缺少字段 $key" }
}
$columns = [int]$grid['columns']
$cardW = $grid['card_w']
$cardH = $grid['card_h']

$identity = 'position=0.000000000,0.000000000,0.000000000;rotation=0.000000000,0.000000000,0.000000000,1.000000000;scale=1.000000000,1.000000000,1.000000000'
$layout = 'anchors=0.5000,0.5000,0.5000,0.5000;offsets={0},{1},{2},{3};min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.5000,0.5000'
# 左上角锚点：pivot=0,0 让点锚点就是文字的左上角（见文件头）。
$point = 'anchors=0.5000,0.5000,0.5000,0.5000;offsets={0},{1},{0},{1};min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000'

# 每个分类一套卡底色（场景 Image 的颜色是 0..1 的 tint）。
$palette = @{
    '2D'       = '0.140,0.330,0.410'
    '3D'       = '0.230,0.270,0.500'
    'Gameplay' = '0.180,0.360,0.300'
    'Input'    = '0.160,0.300,0.540'
    'Audio'    = '0.320,0.220,0.470'
    'UI'       = '0.150,0.370,0.440'
    'Script'   = '0.380,0.280,0.160'
    'Core'     = '0.240,0.260,0.330'
    'Fixture'  = '0.160,0.170,0.200'
}

$out = New-Object System.Collections.Generic.List[string]
$out.Add('scene_version=2')
$out.Add('name=index')
$out.Add('entity	0	Canvas')
$out.Add('component	0	UiLayout	anchors=0.500000,0.500000,0.500000,0.500000;offsets=0.00,0.00,0.00,0.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.500000,0.500000')
$out.Add('entity_meta	0	canvas.reference	1280,720')
$out.Add('entity_meta	0	canvas.safe_area	0,0,0,0')
$out.Add('entity_meta	0	canvas.scale_mode	fit_both')
$out.Add('entity_meta	0	ui.root	true')

# 页眉/页脚也挂在 Canvas 上 → 偏移相对**屏幕中心**（-592 = 48 - 640）。
$out.Add('entity	1	Title	0')
$out.Add('component	1	Text	text=RustLua API Showcase;font_size=36.00;color=0.950,0.820,0.420')
$out.Add('component	1	Transform	' + $identity)
$out.Add('component	1	UiLayout	' + ($point -f '-592.00','-342.00'))

# 计数从 demos.lua 现算：夹具（steps=0）不是 Lab，不能混进 "labs" 的说法里。
$labs = ($entries | Where-Object { $_.steps -gt 0 }).Count
$out.Add('entity	2	Subtitle	0')
$out.Add("component`t2`tText`ttext=$($entries.Count) scenes - $labs labs with checked steps - ESC returns here;font_size=17.00;color=0.620,0.720,0.860")
$out.Add('component	2	Transform	' + $identity)
$out.Add('component	2	UiLayout	' + ($point -f '-592.00','-302.00'))

$id = 2
$rootIds = @('0','1','2')
for ($i = 0; $i -lt $entries.Count; $i++) {
    $entry = $entries[$i]
    $column = $i % $columns
    $row = [math]::Floor($i / $columns)
    $left = $grid['left'] + $column * $grid['col_w']
    $top = $grid['top'] + $row * $grid['row_h']
    $color = if ($palette.ContainsKey($entry.category)) { $palette[$entry.category] } else { '0.20,0.22,0.26' }

    $card = $id + 1
    $titleId = $id + 2
    $detailId = $id + 3
    $metaId = $id + 4
    $id = $metaId
    $name = $entry.scene

    $out.Add("entity`t$card`t$name`t0")
    $out.Add("component`t$card`tImage`ttexture=assets/textures/ui/lab_panel.png;color=$color;nine_slice=14,14,14,14")
    $out.Add("component`t$card`tTransform`t$identity")
    # 实体名 = 目标场景名，`function=on_click` 收实体名，因此点击回调可以直接 scene.load(name)。
    $out.Add("component`t$card`tUiEvents`tevent=click;function=on_click")
    # 卡片挂在 Canvas（屏幕中心的零尺寸矩形）上 → 偏移等价于"相对屏幕中心"的屏幕坐标。
    $out.Add("component`t$card`tUiLayout`t" + ($layout -f (($left - 640).ToString('0.00')), (($top - 360).ToString('0.00')), (($left + $cardW - 640).ToString('0.00')), (($top + $cardH - 360).ToString('0.00'))))

    # 三行文字挂在卡片上 → 偏移相对**卡片中心**，于是拖动卡片文字跟着走。
    $x = ($grid['pad_x'] - $cardW / 2).ToString('0.00')
    foreach ($line in @(
        @{ suffix = 'title';  dy = $grid['title_dy']; text = $entry.title;  size = $grid['font_title']; color = '0.960,0.970,1.000' },
        @{ suffix = 'detail'; dy = $grid['label_dy']; text = $entry.label;  size = $grid['font_label']; color = '0.660,0.750,0.880' },
        @{ suffix = 'meta';   dy = $grid['meta_dy'];  text = '';            size = $grid['font_meta'];  color = '0.980,0.720,0.340' }
    )) {
        $entityId = if ($line.suffix -eq 'title') { $titleId } elseif ($line.suffix -eq 'detail') { $detailId } else { $metaId }
        $value = $line.text
        if ($line.suffix -eq 'meta') {
            $value = "$($entry.category) / " + $(if ($entry.steps -gt 0) { "$($entry.steps) steps" } else { 'golden' })
        }
        $y = ($line.dy - $cardH / 2).ToString('0.00')
        $out.Add("entity`t$entityId`t${name}_$($line.suffix)`t$card")
        $out.Add("component`t$entityId`tText`ttext=$value;font_size=$($line.size.ToString('0.00'));color=$($line.color)")
        $out.Add("component`t$entityId`tTransform`t$identity")
        $out.Add("component`t$entityId`tUiLayout`t" + ($point -f $x, $y))
    }

    $rootIds += "$card"
}

$footer = $id + 1
$out.Add("entity`t$footer`tFooter`t0")
$out.Add('component	' + $footer + '	Text	text=Arrows / TAB move focus - ENTER opens - click a card - F10 quits the app;font_size=16.00;color=0.600,0.680,0.800')
$out.Add('component	' + $footer + '	Transform	' + $identity)
$out.Add('component	' + $footer + '	UiLayout	' + ($point -f '-592.00','288.00'))
$rootIds += "$footer"

$out.Insert(2, 'meta	editor.root_order	' + ($rootIds -join ','))
[System.IO.File]::WriteAllText($target, ($out -join "`n") + "`n", (New-Object System.Text.UTF8Encoding($false)))
Write-Host "wrote $target ($($entries.Count) cards, last entity id $footer)"
