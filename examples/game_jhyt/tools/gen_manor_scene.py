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
  manor_farm（FarmGrid 层）：条目语义由 farm.lua 的 tile_for() 决定
  manor_farm_moist（FarmMoist 层，只占第 1 列）：1=当日已浇的半透明湿痕，深浅靠 per-tile tint
  crops/manor_crops（CropGrid 层，8 列 × 29 行）：瓦值 = 图集条目号（1 基），
    一行一种作物（5 个阶段槽 + 3 帧收成抖动），号由 `scripts/data/crop_art.lua` 给；
    29 种 × 8 = 232 条 < 255，因为瓦值是 u8——再加第 32 种必须先减列或另起一张表。
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
# M72-07：建筑/道具碰撞的单一实现。这里（从零生成）与 `building_kit.py --apply`
# （就地改盘上场景的 solid 串）必须走同一条规则，否则"重跑生成器"会把 solid 弹回
# 按框整片实心——那 24 格悬空碰撞就是这么来的。
import building_kit as bk  # noqa: E402
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
from set_terrain_autotile import freeze_cells, interior_tints, jitter, scatter_tints  # noqa: E402

# 地图尺寸（maps.lua 的 spawn/exit 边界校验按 40x23 处理，不要改）。
# 23 行是为横屏 1280×720 设计视口定的：40×22 = 1280×704 比屏幕矮 16 像素，
# 相机夹到地图下沿后顶/底仍露出 clear color（#0A121F）的深色横条。
W, H = 40, 23

# 列映射（写 scene tile 用）
GRASS, DIRT, STONE, WALL, ROOF, WATER = 1, 2, 3, 4, 5, 6

# 场景键（= `name=` 头 = `data/maps.lua` 的地图键 = `tools/terrain_ramp.json` 的 `scenes` 档名）。
# `building_kit` 的门洞表/横件碰撞表按它点名取用。
SCENE = "manor"
GROUND_TEXTURE = tk.ground_texture(SCENE)
# 农田层（叠在地表之上，运行期由 farm.lua 画）：M72-08 起**只有两列**（1=犁地 2=干涸），
# 出图归 `tools/gen_farm_soil.py`；以前 6 列里那三列 SPROUT/GROWING/RIPE 是"29 种作物
# 共用一张阶段图"的根因，作物贴图搬去了下面的 CropGrid。
FARM_TEXTURE = "assets/art/tilesets/manor/manor_farm.png"
# 湿度层（叠在 FarmGrid 之上）：只有第 1 列有内容，是一张半透明湿痕；深浅不画列，
# 走 M72-02 的 per-tile 顶点色 tint（档表在 `scripts/data/farm_soil.lua`）。
MOIST_TEXTURE = "assets/art/tilesets/manor/manor_farm_moist.png"
# 作物层（叠在湿痕之上）：一行一种作物，8 列 = 5 个阶段槽 + 3 帧收成抖动，
# 瓦值就是图集条目号（1 基）。寻址表 `scripts/data/crop_art.lua`、出图 `gen_crop_art.py`。
CROP_TEXTURE = "assets/art/crops/manor_crops.png"
TILE = 32
FARM_ATLAS_COLS, FARM_ATLAS_ROWS = 2, 1
MOIST_ATLAS_COLS, MOIST_ATLAS_ROWS = 1, 1
CROP_ATLAS_COLS, CROP_ATLAS_ROWS = 8, 29

# ---- 分区 ----
HOUSE = {"x0": 2, "x1": 8, "y0": 5, "y1": 11}          # 正房（本体∪屋顶的格实心）
SHED = {"x0": 4, "x1": 8, "y0": 15, "y1": 18}          # 柴房（同上）
YARD = {"x0": 1, "x1": 12, "y0": 1, "y1": 4}           # 屋前石院（可走）
FARM = {"x0": 14, "x1": 33, "y0": 4, "y1": 13}         # 药田（可走，须为 DIRT）
# 池塘（M72-07 重做）：**按行手填的不规则岸线**，不再是 `x0..x1 × y0..y1` 的矩形。
# 形状读数：27 格，外接框 5×8=40 ⇒ 填充率 0.675（旧版 30 格 / 5×6 = 1.000 的完美方盒）。
# 只许往 x34..38 这一带填：x≤33 是 `farm.lua` 的药田矩形，水一进田就少一格可垦地。
POND_ROWS = {
    3: (36, 37),
    4: (36, 37, 38),
    5: (35, 36, 37),
    6: (35, 36, 37, 38),
    7: (34, 35, 36, 37, 38),
    8: (34, 35, 36, 37),
    9: (35, 36, 37, 38),
    10: (36, 37),
}
POND_CELLS = {(x, y) for y, xs in POND_ROWS.items() for x in xs}
POND = {"x0": 34, "x1": 38, "y0": 3, "y1": 10}         # 外接框（校验与注释用）
# 石砌岸（M72-07，实现在 `build_tiles`）：水格外一圈 4 邻域刷成石板。两个理由，都不是"好看"：
#   * 没有它，水↔草直接相邻，autotile 在岸上读成"一摊蓝浮在绿布上"；
#   * `places.lua:manor_dock` 要一个**站得住的临水格**，石板就是那一格（草也站得住，
#     但"码头"该是硬地面），而香蒲/石头在石板上也有权重（`decor_kit.CATALOG`）。
# 石板晒场（M72-07）：晒谷架 + 草堆脚下那一块。晒场必须是硬地（不然谷子摊在草上），
# 而它顺带把后院那一片"整屏只有草"的窗口打开——`jhyt_m72_manor_terrain` §② 判的是
# 任意 20×12 窗口的瓦种类，最坏那一屏正好压在这里。
THRESH = {"x0": 9, "x1": 13, "y0": 8, "y1": 12}        # 可走，石板
PATH_Y = 3
PATH = {"x0": 13, "x1": 33}                            # 田前石路
GATE_SPUR = {"x": 16, "y0": 1, "y1": 2}                # 南门引道（出口格 16,1）
SPAWN = {"x": 16, "y": PATH_Y}                         # 与 data/maps.lua main.spawn 一致

# ---- 建筑 / 道具 ----
# texture：gen_manor_art.py / gen_interior_art.py 的产物；size 与图片像素 1:1；
# pivot=(0,0) 即 transform 为左下角。cell：贴图左下角所在 cell（可带 cell_dx/cell_dy 微调）。
#
# **这里没有 `solid` 字段了**（M72-07）。以前每件手填一串占地格，结果是"视觉多大填多大"
# 的整框实心 + 一次手挪坐标就留下一片撞空气的悬空格（盘上的药庐/地基从 x33 挪到 x16，
# solid 串留在原位 ⇒ 24 格隐形墙）。现在占地只认 `building_kit` 的一条规则：
#   建筑 = 本体∪同名拆层件（屋顶）的**不透明像素**覆盖格 − 门洞；道具 = `entity_meta cell`
#   那一格（`PROP_COLLISION` 里的横件例外）。改盘用 `building_kit.py --apply`，两条路
# 算的是同一张表，门 `jhyt_m72_building_footprint` 再独立复算一遍。
PROPS = [
    {"name": "ManorHouse", "texture": "assets/art/buildings/manor_house.png",
     "size": (224, 224), "cell": (2, 5), "order": 0, "role": "building"},
    {"name": "ManorShed", "texture": "assets/art/buildings/manor_shed.png",
     "size": (160, 128), "cell": (4, 15), "order": 0, "role": "building"},
    # 南门：门洞 (16,1)(16,2) 是 `data/maps.lua` 的山庄↔云泽村出口格，声明在
    # `building_kit.SCENES["manor"].openings` 里——它必须既"图上真是洞"又"能走到"，
    # 两边都建有反查。
    {"name": "ManorGate", "texture": "assets/art/buildings/manor_gate.png",
     "size": (96, 96), "cell": (15, 0), "order": 5, "role": "building"},
    {"name": "ManorFence", "texture": "assets/art/props/manor_fence.png",
     "size": (24, 64), "cell": (13, 1), "cell_dx": 4, "order": 10, "role": "prop"},
    {"name": "ManorWell", "texture": "assets/art/props/manor_well.png",
     "size": (32, 42), "cell": (11, 2), "order": 10, "role": "prop"},
    {"name": "ManorJar", "texture": "assets/art/props/manor_jar.png",
     "size": (24, 30), "cell": (10, 1), "cell_dx": 4, "order": 10, "role": "prop"},
    {"name": "ManorHaystack", "texture": "assets/art/props/manor_haystack.png",
     "size": (36, 34), "cell": (10, 8), "order": 10, "role": "prop"},
    {"name": "ManorRack", "texture": "assets/art/props/manor_rack.png",
     "size": (64, 46), "cell": (11, 10), "order": 10, "role": "prop"},
    {"name": "ManorSign", "texture": "assets/art/props/manor_sign.png",
     "size": (32, 42), "cell": (13, 5), "order": 10, "role": "prop"},
    {"name": "ManorTreeWest", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (3, 19), "order": 20, "role": "prop"},
    {"name": "ManorTreeNorth", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (25, 20), "order": 20, "role": "prop"},
    {"name": "ManorTreeEast", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (38, 19), "order": 20, "role": "prop"},
    {"name": "ManorBambooWest", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (9, 20), "order": 20, "role": "prop"},
    {"name": "ManorBambooEast", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (27, 19), "order": 20, "role": "prop"},
    # 东南角（池塘北侧空地）补两件，避免右半张地图空荡
    # 注：位置在批次 3 加可建造建筑时挪过（原 (22,19)/(35,14)/(12,19)/(36,19) 会被建筑压住）
    {"name": "ManorTreeSoutheast", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (37, 15), "order": 20, "role": "prop"},
    {"name": "ManorBambooSoutheast", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (32, 16), "order": 20, "role": "prop"},
    # ---- 批次 3：可建造建筑（docs/30 §11.2）----
    # 这 5 座是"已建成"状态摆进山庄，用来验收美术与 footprint；
    # 真正的建造玩法（配方/占地规则/施工进度）是独立的玩法任务，见 docs/43。
    # 药庐与施工演示的 cell 是 M72-04 之后手挪过的（原 (33,14)/(33,19)：那一带要让给
    # 竹丛与池塘北岸），这里按**盘上真值**登记，别再让生成器把建筑搬回 x33。
    {"name": "BuildTrainingHall", "texture": "assets/art/buildings/build_training_hall.png",
     "size": (144, 120), "cell": (20, 16), "order": 0, "role": "building"},
    {"name": "BuildStorehouse", "texture": "assets/art/buildings/build_storehouse.png",
     "size": (128, 112), "cell": (26, 15), "order": 0, "role": "building"},
    {"name": "BuildHerbHut", "texture": "assets/art/buildings/build_herb_hut.png",
     "size": (128, 112), "cell": (16, 14), "order": 0, "role": "building"},
    {"name": "BuildTeaShed", "texture": "assets/art/buildings/build_tea_shed.png",
     "size": (112, 96), "cell": (1, 18), "order": 0, "role": "building"},
    {"name": "BuildForgeStand", "texture": "assets/art/buildings/build_forge_stand.png",
     "size": (112, 104), "cell": (12, 18), "order": 0, "role": "building"},
    # 施工演示：地基（本体）+ 脚手架（叠加层，order 更大所以画在上面）
    {"name": "BuildFoundation", "texture": "assets/art/buildings/build_foundation.png",
     "size": (128, 64), "cell": (16, 19), "order": 0, "role": "building_site"},
    {"name": "BuildScaffold", "texture": "assets/art/buildings/build_scaffold.png",
     "size": (128, 96), "cell": (16, 19), "order": 1, "role": "building_site"},
]


def prop_entity_lines(start_id=3):
    """PROPS → (实体行, 下一个可用 id)。

    `build_scene` 与 `occupied_cells` 都调这一个函数：算碰撞用的那批 sprite 必须与
    写进场景的那批是同一批，否则"按图算的占地"与"画出来的房子"会各算各的。
    """
    lines = []
    next_id = start_id
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
        next_id += 1
    return lines, next_id


def occupied_cells(ground):
    """每栋建筑/每件道具该实心哪些格——**只认 `building_kit` 这一条规则**。

    拆层要先过一遍 `fo.plan`：占地是"本体∪同名屋顶件"的不透明格并集，而屋顶那件
    在盘上是独立实体（M72-06），只喂未拆层的 PROPS 会少算正房那一圈屋檐。
    """
    lines, _ = prop_entity_lines()
    updates, occluders = fo.plan(lines, fo.lk.load())
    patched = fo.apply_patch(lines, updates, occluders)
    sprites = bk.placed_sprites(bk.read_entities(patched))
    cells, readings, _ents = bk.visual_and_solid(bk.project_root(), sprites, W, H, SCENE)
    native = {(x, y) for y in range(H) for x in range(W)
              if ground[y][x] in bk.TILE_NATIVE_SOLID}
    return cells | native, readings


def build_solid(ground):
    """solid 布尔矩阵：`occupied_cells` 的格集换算回来。阻挡行走、且每格都有可视依据。

    旧版在这里按"屋基矩形 + 每件 prop 手填的 solid 串"堆实心，那正是 24 格撞空气的
    来源：手填的串跟着 sprite 走，sprite 一挪坐标就漏。现在串是算出来的。
    """
    cells, readings = occupied_cells(ground)
    return [[1 if (x, y) in cells else 0 for x in range(W)] for y in range(H)], readings


def build_tiles():
    """地表 tile 值矩阵 tiles[y][x]"""
    ground = [[GRASS] * W for _ in range(H)]
    # 屋前石院（先画，后面路/引道不会覆盖它）
    for y in range(YARD["y0"], YARD["y1"] + 1):
        for x in range(YARD["x0"], YARD["x1"] + 1):
            ground[y][x] = STONE
    # 正房/柴房底下**不再刷夯土**（M72-07 设计要点 5）：旧写法给两栋建筑整框刷 DIRT，
    # 在图上读成"棕色方盒套蓝方盒"，而 DIRT 是 `farm.lua` 的可开垦判据，等于在房子底下
    # 留了一片能种地的地。建筑盖住的格看不见地表，留草地就行。
    # 药田整块夯土底（可走；farm.lua 靠 tile==2 判定可开垦）
    for y in range(FARM["y0"], FARM["y1"] + 1):
        for x in range(FARM["x0"], FARM["x1"] + 1):
            ground[y][x] = DIRT
    # 池塘：不规则岸线按行填（形状见 `POND_ROWS`），水格天然实心（WATER 在
    # `building_kit.TILE_NATIVE_SOLID` 里），四邻皆水的内部格再压一档 `shaded` 顶点色
    # 读深度——见 `build_scene` 里的 `water_depth_tints`。
    for (x, y) in POND_CELLS:
        ground[y][x] = WATER
    # 石砌岸：把水格的 4 邻域里**仍是草地**的那些刷成石板（只认 GRASS，所以既不会
    # 吃掉药田的 DIRT——那等于少一格可垦地——也不会碰路/墙）。理由见 `POND_CELLS` 上方那条注释。
    for (x, y) in POND_CELLS:
        for nx, ny in ((x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)):
            if 0 <= nx < W and 0 <= ny < H and ground[ny][nx] == GRASS:
                ground[ny][nx] = STONE
    # 石板晒场：草堆/晒谷架脚下那一块硬地（见 `THRESH` 的注释）。与正房 x2..8、
    # 药田 x14..33 都不相交，刷它不会挪动任何已有分区。
    for y in range(THRESH["y0"], THRESH["y1"] + 1):
        for x in range(THRESH["x0"], THRESH["x1"] + 1):
            ground[y][x] = STONE
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
    # 池心按**形状**压一档 `shaded`（表里第 1 档就是那档冷暗的）：哈希撒布管"这块地被
    # 晒到/在阴里"的低频斑驳，这一层管"水在哪里更深"——不规则岸线没有深度就还是块蓝布。
    tints.update(interior_tints(cells, W, H, WATER, tk.ground_tints(table)[0][0]))
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
    # 3..N 建筑与道具：pivot=(0,0) → transform 即贴图左下角世界坐标。
    # 与 `occupied_cells` 走同一个 `prop_entity_lines`，所以占地算的就是画出来的这批。
    prop_lines, next_id = prop_entity_lines()
    lines.extend(prop_lines)
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
    # 湿度层：id 必须大于 FarmGrid(2)——原生按 ECS 插入序、web 按 entity id 升序排 tilemap
    # 批次，两者都把这张画在 FarmGrid 之上，湿痕才不会压住土面（作物层在它之上，见下）。
    moist_id = camera_id + 1
    lines.append(f"entity\t{moist_id}\tFarmMoist")
    lines.append(transform_line(moist_id, 0.0, 0.0))
    lines.append(
        f"component\t{moist_id}\tTilemap\t"
        + tk.tilemap_component(TILE, W, H, MOIST_TEXTURE, [0] * (W * H), empty,
                               MOIST_ATLAS_COLS, MOIST_ATLAS_ROWS)
    )
    lines.append(f"entity_meta\t{moist_id}\trole\tfarm_moist_tilemap")
    # 作物层（最后一张 Tilemap，M72-08）：id 必须大于 FarmMoist——原生按 ECS 插入序、
    # web 按 entity id 升序，两者都把这张画在湿痕与土面之上。开局全空，运行期由
    # farm.lua 按 `scripts/data/crop_art.lua` 的条目号写格。
    crop_id = moist_id + 1
    lines.append(f"entity\t{crop_id}\tCropGrid")
    lines.append(transform_line(crop_id, 0.0, 0.0))
    lines.append(
        f"component\t{crop_id}\tTilemap\t"
        + tk.tilemap_component(TILE, W, H, CROP_TEXTURE, [0] * (W * H), empty,
                               CROP_ATLAS_COLS, CROP_ATLAS_ROWS)
    )
    lines.append(f"entity_meta\t{crop_id}\trole\tcrop_tilemap")
    # M72-05：装饰散布。id 从**装饰段之前的最后一张结构实体**之后起，一个既有 id 都不动——
    # 挪 id 会让 Lua 侧 `entity.find` 与黄金表全部重排，而装饰是纯加法。
    # （M72-08 在这段之前插了 CropGrid，所以装饰整段顺移一位：这是本包唯一动到的 id。）
    # 输入用**抖动后**的格子图（盘上就是这个），不然"生成器看到的草/土分布"与实机不一致。
    #
    jittered = [cells[y * W:(y + 1) * W] for y in range(H)]
    exclude, avoid = decor_reserved()
    placements = dk.scatter(W, H, jittered, solid, exclude, mid_avoid=avoid)
    lines.extend(dk.decor_lines(crop_id + 1, placements, table,
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
    """契约自检：出生点/出口/地点格可走、门洞可站可达、药田是夯土、建筑锚点不泡水。"""
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
    # 门洞：`building_kit.SCENES[scene].openings` 声明的洞必须**可站且从出生点走到**——洞画对
    # 了但被别的东西围死，玩家一样出不去（南门是山庄↔云泽村的出口格，这条断了就切图）。
    for name, cells in bk.openings_table(SCENE).items():
        for cell in cells:
            cx, cy = cell
            if solid[cy][cx]:
                problems.append(f"{name} 的门洞 {cell} 又被标成 solid：占地规则与 OPENINGS 打架")
            elif (cx, cy) not in walk:
                problems.append(f"{name} 的门洞 {cell} 从出生点不可达")
    # 建筑锚点（贴图左下角那一格）不许是**水**：锚点在水里就是整栋泡在池子里。
    # 查锚点而不是查全部占地——屋檐压到院墙那一行是正常的（南门本来就嵌在院墙里、
    # 脚手架压在墙行上），而"泡水"是只有锚点才会出现的错法。
    for prop in PROPS:
        cx, cy = prop["cell"]
        if ground[cy][cx] == WATER:
            problems.append(f"{prop['name']} 的锚点 ({cx},{cy}) 在水格上：整栋泡在池子里")
    return problems


def main():
    parser = argparse.ArgumentParser(description="从零装配 scenes/manor.scene")
    parser.add_argument("--out", default=os.path.join("scenes", "manor.scene"),
                        help="写出路径。M72-07 已把盘上唯一那处分叉（entity 21/24/25 的 "
                             "Transform 手调）登记回本文件，所以这里默认覆盖盘上是**预期行为**；"
                             "对拍请指到 .temp，然后与 scenes/manor.scene 逐行 diff。")
    args = parser.parse_args()
    ground = build_tiles()
    solid, readings = build_solid(ground)
    text = build_scene(ground, solid)
    out = args.out
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print(f"wrote {out} ({W}x{H}, {len(PROPS)} 个建筑/道具实体)")
    for reading in readings:
        print("   %-20s 视觉 %-3d 格 solid %-3d 格 门洞 %s%s" % (
            reading["name"], reading["visual"], reading["solid"], reading["openings"] or "-",
            "（拆层件：" + ",".join(reading["mates"]) + "）" if reading["mates"] else ""))
    problems = self_check(ground, solid)
    if problems:
        for problem in problems:
            print("[FAIL]", problem)
        raise SystemExit(1)
    places = read_manor_place_cells()
    pond = len(POND_CELLS)
    box = (POND["x1"] - POND["x0"] + 1) * (POND["y1"] - POND["y0"] + 1)
    print(f"契约自检 ok：出生点/出口/药田/门洞/建筑锚点，places.lua {len(places)} 个山庄地点格全部可走且可达")
    print(f"   池塘 {pond} 格 / 外接框 {box} 格 ⇒ 填充率 {pond / box:.3f}（旧版 5×6 矩形是 30/30=1.000）")


if __name__ == "__main__":
    main()
