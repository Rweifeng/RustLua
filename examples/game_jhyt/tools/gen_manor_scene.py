# -*- coding: utf-8 -*-
"""《江湖有田》山庄主场景生成（scenes/manor.scene）

用法：工程根目录 python tools/gen_manor_scene.py
对拍/试跑请用 `--out .temp/xxx.scene`——盘上场景已和生成器分叉，默认路径会直接覆盖手调内容。

美术：地表图集由 `tools/gen_terrain_tiles.py` 产出（12 列 × 9 行，逻辑瓦 + autotile），
农田/湿度两张仍是 `gen_manor_art.py` 的 6 列 tileset；建筑/道具用同工具产出的独立 sprite
（引擎 sprite 不能选 UV 区域，所以一件一张图）。改布局改本文件，改画面改上面两个工具。

引擎 Tilemap 组件约定（scene_version=2，见 engine crates/rustlua-ecs/src/tilemap.rs）：
  tile_size=WxH;width=W;height=H;texture=PATH;tiles_format=1;atlas_cols=N;atlas_rows=M;
  tiles=<逗号分隔的图集索引，row-major，row 0 在最底>;solid=<row-major 0/1>
  开逻辑瓦时再追加 `tile_kinds=K;variant_count=1;autotile=1`（+ 稀疏 `tints`/`tints_rgb`）。
  * `tiles_format=0`（旧编码）是一格一个 ASCII 数字、图集固定 6 列且 V 方向整幅拉伸——
    M72-01 已经把它换掉，因为 6 列寻址不到多行图集，也表达不了"一格 = 一种材质"；
  * 地表层现在存的是 **kind id**（1..6），运行期由 `Tilemap::resolve_atlas_index` 按
    4-bit 邻居掩码解成图集条目（位序 bit0=y-1、bit1=x+1、bit2=y+1、bit3=x-1）；
  * 单格 32 世界单位，y 朝上；索引 index = y*width+x。
列映射（与 farm.lua / gen_terrain_tiles.py 的 terrain_ramp.json 一致，**不可随意改**）：
  manor_ground：1=草地 2=夯土 3=石板路 4=院墙 5=青瓦 6=水
  manor_farm：  1=已耕 2=已耕+浇水 3=苗期 4=生长期 5=成熟 6=稻茬(保留)
  manor_farm_moist（第三层 FarmMoist，画在 FarmGrid 之上、只占第 1 列）：1=当日已浇
  * farm.lua 用 `GroundMap tile == 2` 判定可开垦；overlay 列语义由 farm.lua 的 tile_for() 决定。

布局（cell，0-index，y 朝上，含端点）：
  院墙      ：整圈边界（row0 / row21..22 / col0 / col39），solid
  正房      ：x2..8, y5..11（7×7）solid；其上是 224×224 的 manor_house 贴图
  屋前石院  ：x1..12, y1..4 石板，可走（含出生点与 NPC 落点）
  柴房      ：x4..8, y15..18（5×4）solid；贴图 manor_shed
  药田      ：x14..33, y4..13 夯土，可走（farm.lua 的 FARM 矩形必须与之一致）
  田前石路  ：y=3, x13..33；南门引道 x16, y1..2
  池塘      ：x34..38, y4..9 水，solid
  南门      ：x15..17, y0..2（贴图压住南侧院墙），门洞 x16 可走（出口格 16,1）
  道具      ：水井/水缸/草堆/晒谷架/木牌/竹篱/竹丛/老树，见 PROPS；实心的在 solid 里标 1

**与 data/places.lua 的契约**：那里的 main 地点格必须仍然可走（本工具改动后要复核）：
  manor_gate(16,2) manor_home(12,6) manor_home_east(20,8) manor_herb_field(16,5)
  manor_study(24,10) manor_sword_ground(26,12) manor_forge(28,12) manor_practice(30,14)
  manor_garden(10,16) manor_dock(16,19)
"""
import argparse
import os

import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

# 工程根（`examples/game_jhyt`）。**事实输入**（places.lua 这类决定留白契约的数据）按这里定位，
# 不跟着 cwd 走：写死的相对路径会让人以为工具是确定性的，其实它在换个目录跑的时候就少了一份输入。
# 输出路径（`--out`）保持相对 cwd，那是给人用的。
PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

from character_art import player_art  # noqa: E402  主角形象（与 manifest 同源）
# M72-05：装饰散布的单一实现。这里与 `set_scatter_decor.py`（就地改盘上场景）必须共用它，
# 否则"重跑生成器"会抹掉盘上场景的装饰段——M72-04 就是靠这条对拍成立的。
import decor_kit as dk  # noqa: E402
# M72-06 C 段：地面投影贴片的单一实现（这里与 `set_cast_shadows.py` 同一条 `build()`）。
import set_cast_shadows as cs  # noqa: E402
# M72-06：前景遮断拆层的单一实现。这里（从零生成）与 `set_foreground_occluders.py`
# （就地改盘上场景）必须走同一条 `plan()`+`apply_patch()`，否则重跑生成器会静默丢掉那 5 件。
import set_foreground_occluders as fo  # noqa: E402
# M32：`Transform` 的统一形态序列化器（场景数据门只认这一种写法）。
from pixel_kit import transform_line as spawn_transform  # noqa: E402
from pixel_kit import camera_line  # noqa: E402  场景 2D 相机（取景锁在地图内）
# M72-04：地表是逻辑瓦 + autotile + tint，条目算术/串格式/抖动/撒布全走公共层，
# 与 `set_terrain_autotile.py`（就地改盘上场景）共用同一份实现。
import terrain_kit as tk  # noqa: E402
from set_terrain_autotile import freeze_cells, jitter, scatter_tints  # noqa: E402

# 地图尺寸（maps.lua 的 spawn/exit 边界校验按 40x23 处理，不要改）。
# 23 行是为横屏 1280×720 设计视口定的：40×22 = 1280×704 比屏幕矮 16 像素，
# 相机夹到地图下沿后顶/底仍露出 clear color（#0A121F）的深色横条。
W, H = 40, 23

# 列映射（写 scene tile 用）
GRASS, DIRT, STONE, WALL, ROOF, WATER = 1, 2, 3, 4, 5, 6

GROUND_TEXTURE = "assets/art/tilesets/manor/manor_ground.png"
FARM_TEXTURE = "assets/art/tilesets/manor/manor_farm.png"
# 湿度层（叠在 FarmGrid 之上）：只有第 1 列有内容（1=当日已浇的半透明湿痕）。
# 单开一层而不是给 manor_farm 加"湿版"列，原因见 gen_manor_art.py 的 farm_moist_mark。
MOIST_TEXTURE = "assets/art/tilesets/manor/manor_farm_moist.png"
TILE = 32
# 农田层与湿度层仍是"6 列 × 1 行"的老图集（M72-04 只重做地表；农田归 M72-08）。
FARM_ATLAS_COLS, FARM_ATLAS_ROWS = 6, 1

# ---- 分区 ----
HOUSE = {"x0": 2, "x1": 8, "y0": 5, "y1": 11}          # 正房（solid）
SHED = {"x0": 4, "x1": 8, "y0": 15, "y1": 18}          # 柴房（solid）
YARD = {"x0": 1, "x1": 12, "y0": 1, "y1": 4}           # 屋前石院（可走）
FARM = {"x0": 14, "x1": 33, "y0": 4, "y1": 13}         # 药田（可走，须为 DIRT）
POND = {"x0": 34, "x1": 38, "y0": 4, "y1": 9}          # 池塘（solid）
PATH_Y = 3
PATH = {"x0": 13, "x1": 33}                            # 田前石路
GATE_SPUR = {"x": 16, "y0": 1, "y1": 2}                # 南门引道（出口格 16,1）
SPAWN = {"x": 16, "y": PATH_Y}                         # 与 data/maps.lua main.spawn 一致

# ---- 建筑 / 道具 ----
# texture：gen_manor_art.py 的产物；size 与图片像素 1:1；pivot=(0,0) 即 transform 为左下角。
# cell：贴图左下角所在 cell（可带 cell_dx/cell_dy 像素微调）；solid：需要标 1 的格。
PROPS = [
    {"name": "ManorHouse", "texture": "assets/art/buildings/manor_house.png",
     "size": (224, 224), "cell": (2, 5), "order": 0, "role": "building",
     "solid": [(x, y) for y in range(HOUSE["y0"], HOUSE["y1"] + 1)
               for x in range(HOUSE["x0"], HOUSE["x1"] + 1)]},
    {"name": "ManorShed", "texture": "assets/art/buildings/manor_shed.png",
     "size": (160, 128), "cell": (4, 15), "order": 0, "role": "building",
     "solid": [(x, y) for y in range(SHED["y0"], SHED["y1"] + 1)
               for x in range(SHED["x0"], SHED["x1"] + 1)]},
    {"name": "ManorGate", "texture": "assets/art/buildings/manor_gate.png",
     "size": (96, 96), "cell": (15, 0), "order": 5, "role": "building",
     "solid": [(15, 1), (15, 2), (17, 1), (17, 2)]},  # 门洞 (16,1)(16,2) 保持可走=出口格
    {"name": "ManorFence", "texture": "assets/art/props/manor_fence.png",
     "size": (24, 64), "cell": (13, 1), "cell_dx": 4, "order": 10, "role": "prop",
     "solid": [(13, 1), (13, 2)]},
    {"name": "ManorWell", "texture": "assets/art/props/manor_well.png",
     "size": (32, 42), "cell": (11, 2), "order": 10, "role": "prop",
     "solid": [(11, 2)]},
    {"name": "ManorJar", "texture": "assets/art/props/manor_jar.png",
     "size": (24, 30), "cell": (10, 1), "cell_dx": 4, "order": 10, "role": "prop",
     "solid": [(10, 1)]},
    {"name": "ManorHaystack", "texture": "assets/art/props/manor_haystack.png",
     "size": (36, 34), "cell": (10, 8), "order": 10, "role": "prop",
     "solid": [(10, 8)]},
    {"name": "ManorRack", "texture": "assets/art/props/manor_rack.png",
     "size": (64, 46), "cell": (11, 10), "order": 10, "role": "prop",
     "solid": [(11, 10), (12, 10)]},
    {"name": "ManorSign", "texture": "assets/art/props/manor_sign.png",
     "size": (32, 42), "cell": (13, 5), "order": 10, "role": "prop",
     "solid": [(13, 5)]},
    {"name": "ManorTreeWest", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (3, 19), "order": 20, "role": "prop",
     "solid": [(3, 19)]},
    {"name": "ManorTreeNorth", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (25, 20), "order": 20, "role": "prop",
     "solid": [(25, 20)]},
    {"name": "ManorTreeEast", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (38, 19), "order": 20, "role": "prop",
     "solid": [(38, 19)]},
    {"name": "ManorBambooWest", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (9, 20), "order": 20, "role": "prop",
     "solid": [(9, 20)]},
    {"name": "ManorBambooEast", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (27, 19), "order": 20, "role": "prop",
     "solid": [(27, 19)]},
    # 东南角（池塘南侧空地）补两件，避免右半张地图空荡
    # 注：位置在批次 3 加可建造建筑时挪过（原 (22,19)/(35,14)/(12,19)/(36,19) 会被建筑压住）
    {"name": "ManorTreeSoutheast", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (37, 15), "order": 20, "role": "prop",
     "solid": [(37, 15)]},
    {"name": "ManorBambooSoutheast", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (32, 16), "order": 20, "role": "prop",
     "solid": [(32, 16)]},
    # ---- 批次 3：可建造建筑（docs/30 §11.2）----
    # 这 5 座是"已建成"状态摆进山庄，用来验收美术与 footprint；
    # 真正的建造玩法（配方/占地规则/施工进度）是独立的玩法任务，见 docs/43。
    {"name": "BuildTrainingHall", "texture": "assets/art/buildings/build_training_hall.png",
     "size": (144, 120), "cell": (20, 16), "order": 0, "role": "building",
     "solid": [(x, y) for y in range(16, 20) for x in range(20, 25)]},
    {"name": "BuildStorehouse", "texture": "assets/art/buildings/build_storehouse.png",
     "size": (128, 112), "cell": (26, 15), "order": 0, "role": "building",
     "solid": [(x, y) for y in range(15, 19) for x in range(26, 30)]},
    {"name": "BuildHerbHut", "texture": "assets/art/buildings/build_herb_hut.png",
     "size": (128, 112), "cell": (33, 14), "order": 0, "role": "building",
     "solid": [(x, y) for y in range(14, 18) for x in range(33, 37)]},
    {"name": "BuildTeaShed", "texture": "assets/art/buildings/build_tea_shed.png",
     "size": (112, 96), "cell": (1, 18), "order": 0, "role": "building",
     "solid": [(x, y) for y in range(18, 21) for x in range(1, 5)]},
    {"name": "BuildForgeStand", "texture": "assets/art/buildings/build_forge_stand.png",
     "size": (112, 104), "cell": (12, 18), "order": 0, "role": "building",
     "solid": [(x, y) for y in range(18, 21) for x in range(12, 16)]},
    # 施工演示：地基（本体）+ 脚手架（叠加层，order 更大所以画在上面）
    {"name": "BuildFoundation", "texture": "assets/art/buildings/build_foundation.png",
     "size": (128, 64), "cell": (33, 19), "order": 0, "role": "building_site",
     "solid": [(x, y) for y in range(19, 21) for x in range(33, 37)]},
    {"name": "BuildScaffold", "texture": "assets/art/buildings/build_scaffold.png",
     "size": (128, 96), "cell": (33, 19), "order": 1, "role": "building_site",
     "solid": []},
]


def inside(cell, x0, x1, y0, y1):
    return x0 <= cell[0] <= x1 and y0 <= cell[1] <= y1


def build_tiles():
    """地表 tile 值矩阵 tiles[y][x]"""
    ground = [[GRASS] * W for _ in range(H)]
    # 屋前石院（先画，后面路/引道不会覆盖它）
    for y in range(YARD["y0"], YARD["y1"] + 1):
        for x in range(YARD["x0"], YARD["x1"] + 1):
            ground[y][x] = STONE
    # 正房/柴房基底（夯土；sprite 会盖住，只露边缘）
    for region in (HOUSE, SHED):
        for y in range(region["y0"], region["y1"] + 1):
            for x in range(region["x0"], region["x1"] + 1):
                ground[y][x] = DIRT
    # 药田整块夯土底（可走；farm.lua 靠 tile==2 判定可开垦）
    for y in range(FARM["y0"], FARM["y1"] + 1):
        for x in range(FARM["x0"], FARM["x1"] + 1):
            ground[y][x] = DIRT
    # 池塘
    for y in range(POND["y0"], POND["y1"] + 1):
        for x in range(POND["x0"], POND["x1"] + 1):
            ground[y][x] = WATER
    # 田前石路 + 南门引道
    for x in range(PATH["x0"], PATH["x1"] + 1):
        ground[PATH_Y][x] = STONE
    for y in range(GATE_SPUR["y0"], GATE_SPUR["y1"] + 1):
        ground[y][GATE_SPUR["x"]] = STONE
    # 整圈院墙（底部两行都是墙：H 22→23 时原下沿 H-2 保持墙，新行压在最底）
    for x in range(W):
        ground[0][x] = WALL
        ground[H - 2][x] = WALL
        ground[H - 1][x] = WALL
    for y in range(H):
        ground[y][0] = WALL
        ground[y][W - 1] = WALL
    return ground


def build_solid(ground):
    """solid 布尔矩阵 solid[y][x]：阻挡行走。

    规则：院墙/水/屋舍基底 + 道具占地阻挡；草地/夯土/石板可走。
    建筑与道具的碰撞必须在这里标（sprite 本身不参与碰撞）。"""
    solid = [[0] * W for _ in range(H)]
    for y in range(H):
        for x in range(W):
            if ground[y][x] in (WALL, WATER) or inside((x, y), HOUSE["x0"], HOUSE["x1"], HOUSE["y0"], HOUSE["y1"]) \
                    or inside((x, y), SHED["x0"], SHED["x1"], SHED["y0"], SHED["y1"]):
                solid[y][x] = 1
    for prop in PROPS:
        for (x, y) in prop["solid"]:
            solid[y][x] = 1
    return solid


def decor_reserved():
    """M72-05 的玩法留白：返回 (两带共同的硬留白, 只挡中景带的留白)。

    输入全部来自本文件的布局常量与 `data/places.lua`，所以 `set_scatter_decor.py` 只要
    import 这里，两条路就必然撒出同一段装饰（为什么药田是硬的、道具/NPC 只挡中景，
    口径写在 `decor_kit.hard_exclude` 与 `mid_avoid`）。
    """
    places = [(cx, cy) for _, cx, cy in read_manor_place_cells()]
    props = [(prop["cell"][0], prop["cell"][1]) for prop in PROPS]
    keys = [(SPAWN["x"], SPAWN["y"]), (GATE_SPUR["x"], GATE_SPUR["y0"])]
    farm = (FARM["x0"], FARM["y0"], FARM["x1"], FARM["y1"])
    return (dk.hard_exclude(W, H, farm), dk.mid_avoid(W, H, props, places, keys))


def cell_to_world(cx, cy):
    """cell 左下角世界坐标（y 朝上，row 0 在最底）"""
    return (cx + 0.5) * TILE, (cy + 0.5) * TILE


def row_major(grid):
    out = []
    for y in range(H):
        for x in range(W):
            out.append(str(grid[y][x]))
    return "".join(out)


def transform_line(entity_id, x, y):
    # M32：统一的 Transform 形态由 `pixel_kit.transform_line` 单点产出
    # （引擎读侧只认 `position/rotation/scale`，2D 旧形态已删除；见该函数注释）。
    return spawn_transform(entity_id, x, y)


def sprite_line(entity_id, texture, width, height, layer, order):
    return (
        f"component\t{entity_id}\tSprite\ttexture={texture};size={width}x{height};"
        f"pivot=0.000,0.000;tint=1.000,1.000,1.000,1.000;sorting_layer={layer};order={order}"
    )


def build_scene(ground, solid):
    solid_str = row_major(solid)
    lines = []
    lines.append("scene_version=2")
    # 场景名必须等于地图键（data/maps.lua 的 `manor`）：main.lua 用
    # `is_playable(scene.reload())` 判定玩法地图，写成 main 会导致整局进不去。
    lines.append("name=manor")
    lines.append("meta\tdisplay_name\t江湖有田·山庄")
    lines.append("meta\tgame\tjianghu_youtian")
    lines.append("meta\ttile_size\t32")
    # 横屏方案A：设计分辨率 1280×720（与 `scripts/ui/layout.lua` 的 DESIGN_W/H 一致，
    # 由 `ui/layout_check.lua` 在 `--check` 与启动期守住）。
    lines.append("meta\tui_reference_height\t720")
    lines.append("meta\tui_reference_width\t1280")
    # 1 地表：逻辑瓦 + autotile + per-tile tint（口径见 `terrain_kit.py` 与 M72-04）。
    # 与 `set_terrain_autotile.py` 用同一套抖动与撒布，所以"重跑生成器"和"就地重排盘上
    # 场景"得到同一个字节序列——这两件事不一致过一次，M72-01 迁编码时才撞出对拍假差异。
    table = tk.load_ramp()
    cells = [ground[y][x] for y in range(H) for x in range(W)]
    cells, _ = jitter(cells, solid_str, W, H,
                      freeze_cells([(FARM["x0"], FARM["y0"], FARM["x1"], FARM["y1"])], W, H))
    tints = scatter_tints(W, H, [tint_id for tint_id, _ in tk.ground_tints(table)])
    lines.append("entity\t1\tGroundMap")
    lines.append(transform_line(1, 0.0, 0.0))
    lines.append(
        "component\t1\tTilemap\t"
        + tk.tilemap_component(TILE, W, H, GROUND_TEXTURE, cells, solid_str,
                               table["cols"], table["rows"],
                               tile_kinds=table["kind_count"], tints=tints,
                               tints_rgb=tk.tints_rgb_string(table))
    )
    lines.append("entity_meta\t1\trole\tground_tilemap")
    # 2 FarmGrid 覆盖耕地 overlay（初始全空，运行期 farm.lua 写 tile/meta）
    empty = "0" * (W * H)
    lines.append("entity\t2\tFarmGrid")
    lines.append(transform_line(2, 0.0, 0.0))
    lines.append(
        "component\t2\tTilemap\t"
        + tk.tilemap_component(TILE, W, H, FARM_TEXTURE, [0] * (W * H), empty,
                               FARM_ATLAS_COLS, FARM_ATLAS_ROWS)
    )
    lines.append("entity_meta\t2\trole\tfarm_tilemap")
    # 3..N 建筑与道具：pivot=(0,0) → transform 即贴图左下角世界坐标
    next_id = 3
    prop_ids = {}
    for prop in PROPS:
        cell_x, cell_y = prop["cell"]
        world_x = cell_x * TILE + prop.get("cell_dx", 0)
        world_y = cell_y * TILE + prop.get("cell_dy", 0)
        width, height = prop["size"]
        lines.append(f"entity\t{next_id}\t{prop['name']}")
        lines.append(sprite_line(next_id, prop["texture"], width, height,
                                 prop.get("layer", "world"), prop["order"]))
        lines.append(transform_line(next_id, float(world_x), float(world_y)))
        lines.append(f"entity_meta\t{next_id}\trole\t{prop['role']}")
        lines.append(f"entity_meta\t{next_id}\tcell\t{cell_x},{cell_y}")
        prop_ids[prop["name"]] = next_id
        next_id += 1
    # 玩家本体：场景内可碰撞动态刚体（脚本经 entity.find("JianghuPlayer") 使用）
    player_id = next_id
    spawn_wx, spawn_wy = cell_to_world(SPAWN["x"], SPAWN["y"])
    lines.append(f"entity\t{player_id}\tJianghuPlayer")
    # 主角形象（M21）：单帧 + 序列帧都从角色美术 manifest 取，与
    # data/characters.lua 是同一套串；运行期 player.apply() 会按
    # state.appearance（男/女/未来的自定义）纠正并重挂，此处给的是默认形象。
    player_sprite, player_animation = player_art("player_male")
    lines.append(f"component\t{player_id}\tSprite\t{player_sprite}")
    if player_animation:
        lines.append(f"component\t{player_id}\tAnimation\t{player_animation}")
    lines.append(f"component\t{player_id}\tCollider\tshape=ball;radius=12")
    lines.append(transform_line(player_id, spawn_wx, spawn_wy))
    lines.append(f"entity_meta\t{player_id}\trole\tplayer_spawn")
    lines.append(f"entity_meta\t{player_id}\tpersistent\ttrue")
    next_id += 1
    # HUD 文本（脚本动态刷新用 ui.text_at；场景内置两个静态说明/标题）
    title_id = next_id
    lines.append(f"entity\t{title_id}\tHudTitle")
    lines.append(
        # 这两行 UiLayout 的数值以**盘上场景为准**（UI 那几轮直接在 .scene 上收过尺寸）。
        # 生成器必须跟着改，否则重跑一次就把 HUD 弹回旧版——M72-04 的对拍就是靠
        # "生成器输出 == 盘上场景"这条成立的。
        f"component\t{title_id}\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;"
        "offsets=24.00,12.00,500.00,36.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000"
    )
    lines.append(
        f"component\t{title_id}\tText\ttext=山庄 · 药田垂直切片;font_size=18.00;color=0.949,0.918,0.847"
    )
    lines.append(transform_line(title_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{title_id}\trole\thud_title")
    hint_id = next_id + 1
    lines.append(f"entity\t{hint_id}\tHudHint")
    lines.append(
        # 地图自带操作提示放在**地图标题正下方**（与其它地图同一套坐标）；数值以盘上场景
        # 为准（见上面 HudTitle 那条注释），底部信息区整块留给 HUD 的动态反馈/键位行。
        f"component\t{hint_id}\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;"
        "offsets=24.00,40.00,560.00,64.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000"
    )
    lines.append(
        f"component\t{hint_id}\tText\ttext=WASD 移动 · E 朝面前格劳作 · T 睡觉到清晨 · Q 吐纳练功 · J 普攻 · K 武学"
        ";font_size=14.00;color=0.850,0.900,1.000"
    )
    lines.append(transform_line(hint_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{hint_id}\trole\thud_hint")
    # 场景相机（本地图唯一的 `Camera` 实体）：把 2D 取景范围锁在整张地图的矩形内，
    # 运行期由 `scripts/world/camera_view.lua` 让相机跟随玩家（`camera2d.follow`）。
    # 几何与理由见 `pixel_kit.camera_line`；`scripts/world/camera_view.lua` 的
    # `validate_all` 会在启动/`--check` 时核对"场景 bounds == 地图尺寸"。
    camera_id = hint_id + 1
    lines.append(f"entity\t{camera_id}\tGameCamera")
    lines.append(camera_line(camera_id, W, H))
    lines.append(transform_line(camera_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{camera_id}\trole\tgame_camera")
    # 湿度层（最后一张 Tilemap）：id 必须大于 FarmGrid(2)——原生按 ECS 插入序、web 按
    # entity id 升序排 tilemap 批次，两者都把这张画在 FarmGrid 之上，湿痕才不会压住作物。
    moist_id = camera_id + 1
    lines.append(f"entity\t{moist_id}\tFarmMoist")
    lines.append(transform_line(moist_id, 0.0, 0.0))
    lines.append(
        f"component\t{moist_id}\tTilemap\t"
        + tk.tilemap_component(TILE, W, H, MOIST_TEXTURE, [0] * (W * H), empty,
                               FARM_ATLAS_COLS, FARM_ATLAS_ROWS)
    )
    lines.append(f"entity_meta\t{moist_id}\trole\tfarm_moist_tilemap")
    # M72-05：装饰散布。id 从 `moist_id + 1` 起**追加在末尾**，一个既有 id 都不动——
    # 挪 id 会让 Lua 侧 `entity.find` 与黄金表全部重排，而装饰是纯加法。
    # 输入用**抖动后**的格子图（盘上就是这个），不然"生成器看到的草/土分布"与实机不一致。
    jittered = [cells[y * W:(y + 1) * W] for y in range(H)]
    exclude, avoid = decor_reserved()
    placements = dk.scatter(W, H, jittered, solid, exclude, mid_avoid=avoid)
    lines.extend(dk.decor_lines(moist_id + 1, placements, table,
                                lambda cell: tints.get(cell[1] * W + cell[0], 0)))
    # M72-06：前景遮断拆层（树冠/屋顶）。这里与 `set_foreground_occluders.py`（就地改盘上
    # 场景）**共用同一条 `plan()`+`apply_patch()` 代码路径**，不是"各自实现一遍看起来一样"：
    # 遮断块的存在感极低（拼回去画面逐字节相同），一旦重跑生成器把这一段抹掉，
    # 盘上只是"前景没了"，没有任何一张图会报错。
    fo.assert_id_band_free(lines)
    base_updates, occluders = fo.plan(lines, fo.lk.load())
    lines = fo.apply_patch(lines, base_updates, occluders)
    # M72-06 C 段：建筑/树/道具的地面投影贴片。同一条理由——与 `set_cast_shadows.py`
    # 共用 `build()`+`apply_patch()`，并且贴片 PNG 也走同一条 `write_art`（内容没变不重写）。
    # 必须在 `fo.apply_patch` **之后**：贴片按拆层后的本体贴图算落地行，也按拆层后的
    # 实体排除树冠/屋顶块（空中块不配自己的投影）。
    cs.assert_id_band_free(lines)
    shadows, canvases = cs.build(lines, cs.lk.load(), cs.load_art(lines))
    lines = cs.apply_patch(lines, cs.shadow_lines(shadows))
    cs.write_art(canvases, ".")
    return "\n".join(lines) + "\n"


def reachable_from(solid, start):
    """从 start 起在可走格上做 4 邻域 flood fill。"""
    seen = {start}
    stack = [start]
    while stack:
        x, y = stack.pop()
        for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            nx, ny = x + dx, y + dy
            if 0 <= nx < W and 0 <= ny < H and not solid[ny][nx] and (nx, ny) not in seen:
                seen.add((nx, ny))
                stack.append((nx, ny))
    return seen


def read_manor_place_cells(
    path=os.path.join(PROJECT_ROOT, "scripts", "data", "places.lua"),
):
    """从 data/places.lua 里抠出 map = "manor" 的地点格（契约校验用，不执行 Lua）。"""
    import re

    if not os.path.exists(path):
        raise FileNotFoundError(
            f"{path} 读不到：places.lua 的地点格是散布的**留白输入**，宽容地当成"
            "「一个地点也没有」继续跑，草就撒到 NPC 门口（改这里之前正是这么漏的——"
            "默认路径跟着 cwd 走，从 `engine/` 下跑 `--check` 时 exists 为假、静默返回空表，"
            "盘上 24 行对不上却看不出为什么）"
        )
    cells = []
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            match = re.match(
                r'\s*(\w+)\s*=\s*\{\s*map\s*=\s*"manor"\s*,\s*cell\s*=\s*\{\s*x\s*=\s*(\d+)\s*,\s*y\s*=\s*(\d+)\s*\}',
                line,
            )
            if match:
                cells.append((match.group(1), int(match.group(2)), int(match.group(3))))
    return cells


def self_check(ground, solid):
    """契约自检：出生点/出口/地点格可走，药田是夯土，道具没占墙/水。"""
    problems = []
    walk = reachable_from(solid, (SPAWN["x"], SPAWN["y"]))
    checks = [
        ("spawn", SPAWN["x"], SPAWN["y"], STONE),
        ("exit", GATE_SPUR["x"], GATE_SPUR["y0"], STONE),
    ]
    for name, cx, cy, want in checks:
        if solid[cy][cx]:
            problems.append(f"{name} ({cx},{cy}) 变成 solid")
        elif ground[cy][cx] != want:
            problems.append(f"{name} ({cx},{cy}) tile={ground[cy][cx]} 期望 {want}")
        elif (cx, cy) not in walk:
            problems.append(f"{name} ({cx},{cy}) 从出生点不可达")
    for name, cx, cy in read_manor_place_cells():
        if solid[cy][cx]:
            problems.append(f"places.lua:{name} ({cx},{cy}) 是 solid（NPC 会走出不来）")
        elif (cx, cy) not in walk:
            problems.append(f"places.lua:{name} ({cx},{cy}) 从出生点不可达")
    if not all(ground[y][x] == DIRT
               for y in range(FARM["y0"], FARM["y1"] + 1)
               for x in range(FARM["x0"], FARM["x1"] + 1)):
        problems.append("药田矩形里出现非夯土格（farm.lua 靠 tile==2 判定可开垦）")
    for prop in PROPS:
        for (cx, cy) in prop["solid"]:
            if ground[cy][cx] in (WATER, WALL):
                problems.append(f"{prop['name']} 占用了墙/水格 ({cx},{cy})")
    return problems


def main():
    parser = argparse.ArgumentParser(description="从零装配 scenes/manor.scene")
    parser.add_argument("--out", default=os.path.join("scenes", "manor.scene"),
                        help="写出路径。**对拍请指到 .temp**：盘上的 manor.scene 已和生成器分叉"
                             "（M72-04「未完成项」第 6 条：entity 21/24/25 的 Transform 是之后手调的），"
                             "直接覆盖会抹掉那批改动——装饰段用 tools/set_scatter_decor.py 就地改。")
    args = parser.parse_args()
    ground = build_tiles()
    solid = build_solid(ground)
    text = build_scene(ground, solid)
    out = args.out
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print(f"wrote {out} ({W}x{H}, {len(PROPS)} 个建筑/道具实体)")
    problems = self_check(ground, solid)
    if problems:
        for problem in problems:
            print("[FAIL]", problem)
        raise SystemExit(1)
    places = read_manor_place_cells()
    print(f"契约自检 ok：出生点/出口/药田/道具占位，places.lua {len(places)} 个山庄地点格全部可走且可达")


if __name__ == "__main__":
    main()
