# -*- coding: utf-8 -*-
"""《江湖有田》云泽村场景生成（scenes/village.scene）——山谷村落版。

用法（工程根目录 `examples/game_jhyt` 下）：

    python tools/gen_village_scene.py                 # 覆盖盘上场景（默认路径）
    python tools/gen_village_scene.py --out .temp/x.scene   # 对拍/试跑，不碰盘上场景

**本文件是这张图的唯一事实源**（M72-11 起）。以前它只写到"地表 6 列 + 每件手填 solid"
那一层，剩下的（autotile 载荷、per-tile tint、前景遮断、地面投影、装饰散布）全靠
`set_*.py` 就地补丁，于是生成器与盘上分叉：横屏方案A 把杂货铺/茶摊搬进市集，
solid 串却留在原位 ⇒ 盘上 38 格"撞空气"的隐形墙 + 画出来的铺面一格碰撞都没有。
现在与 `gen_manor_scene.py` 同构：**同一条代码路径**（`building_kit` / `set_foreground_occluders`
/ `set_cast_shadows` / `decor_kit` / `set_terrain_autotile` 的抖动与撒布），所以
"重跑生成器"与"就地工具 `--check`"必须给出同一个字节序列。

改布局改本文件，改画面改 `gen_village_art.py` / `gen_terrain_tiles.py`，两边各跑一次即可复现。

引擎 Tilemap 约定（scene_version=2，见 engine crates/rustlua-ecs/src/tilemap.rs）：
  tile_size=WxH;width=W;height=H;texture=PATH;tiles_format=1;atlas_cols=N;atlas_rows=M;
  tiles=<逗号分隔的图集索引，row-major，row 0 在最底>;solid=<row-major 0/1>
  开逻辑瓦时再追加 `tile_kinds=K;variant_count=1;autotile=1`（+ 稀疏 `tints`/`tints_rgb`）。
  * 地表层存的是 **kind id**（1..6），运行期由 `Tilemap::resolve_atlas_index` 按 4-bit
    邻居掩码解成 12×9 图集的条目（位序 bit0=y-1、bit1=x+1、bit2=y+1、bit3=x-1）；
  * 单格 32 世界单位，y 朝上；索引 index = y*width+x。
列映射（与 `terrain_ramp.json` 的 `scenes.village.kinds` 一致，**不可随意改**）：
  1=山谷野草 2=茶园土 3=青石板 4=白墙黛瓦院墙 5=黛瓦 6=溪水
  （村庄没有农田层：`farm.lua` 的 FARM 矩形只在山庄，所以这张图只有 GroundMap 一张 Tilemap。）

山谷村落布局（cell，0-index，y 朝上，含端点）：
  村口牌坊 ：x18..20, y0..2（贴图压住南院墙，门洞 x19 可走 = 山庄↔云泽村出口格 19,1）
  主街     ：x16..23, y1..9 石板；市集广场 x12..27, y3..5；中街 x10..29, y9..10
  药庐     ：x3..8, y15..19（6×5）；前院石板 x2..9, y13..14
  杂货铺   ：x21..26, y6..10（6×5，横屏方案A 从东院搬进市集）
  铁匠铺   ：x25..28, y11..12（4×2）；茶摊 x21..24, y12..13（4×2）
  民居     ：house_a x16..19, y18..20；house_b x27..30, y18..20（各 4×3）
  土地庙   ：x22..23, y19..20（2×2）
  茶园     ：x10..14, y11..14 夯土，可走（**抖动时冻住**，理由见 `FREEZE`）
  溪水     ：y16..17, x11..29（x18..21 加宽到 y15..17），实心
  石桥     ：x24..25, y16..17 铺石板（可走）→ 北岸唯一正门通道
  北岸     ：y18..20 草地（民居/土地庙/渡口）。渡口那块 x13..15 走**沿溪岸的 y18 走廊**
            （x9 → x12 → x13）回村，不是走山门那一侧——山门的石体压住 y20..22 一整排。
  山门     ：x9..12, y20..22 石体（贴着北墙），门洞 (10,20) = 村↔青云山出口格，
            落点在其南邻 (10,19)（`maps.lua` 的 mountain_to_village.to_cell）

**与 data/places.lua 的契约**：那里的 village 地点格必须可走且从出生点可达，本文件每次
生成都跑 flood fill 自检（`self_check`），失败直接非零退出。
"""
import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

# 工程根（`examples/game_jhyt`）。**事实输入**（places.lua 这类决定留白契约的数据）按这里定位，
# 不跟着 cwd 走：写死的相对路径会让工具在别的目录下静默少一份输入（见 gen_manor_scene 同条注释）。
PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

from character_art import player_art  # noqa: E402  主角形象（与 manifest 同源）
import building_kit as bk  # noqa: E402  M72-07：建筑/道具碰撞的单一实现
import decor_kit as dk  # noqa: E402  M72-05：装饰散布的单一实现
import set_cast_shadows as cs  # noqa: E402  M72-06 C 段：地面投影贴片的单一实现
import set_foreground_occluders as fo  # noqa: E402  M72-06：前景遮断拆层的单一实现
import terrain_kit as tk  # noqa: E402  M72-04：逻辑瓦 + autotile + tint 的口径
from pixel_kit import camera_line  # noqa: E402  场景 2D 相机（取景锁在地图内）
from set_terrain_autotile import freeze_cells, interior_tints, jitter, scatter_tints  # noqa: E402

# 地图尺寸（maps.lua 的 spawn/exit 边界校验按 40x23 处理，不要改）。
# 23 行是为横屏 1280×720 设计视口定的：40×22 = 1280×704 比屏幕矮 16 像素，
# 相机夹到地图下沿后顶/底仍露出 clear color（#0A121F）的深色横条。
W, H = 40, 23
TILE = 32

# 场景键（= `name=` 头 = `data/maps.lua` 的地图键 = `terrain_ramp.json` 的 `scenes` 档名）。
# `building_kit` 的门洞表/横件碰撞表、色阶表都按它点名取用。
SCENE = "village"
GROUND_TEXTURE = tk.ground_texture(SCENE)

# 列映射（写 scene tile 用的 kind id）
GRASS, DIRT, STONE, WALL, ROOF, WATER = 1, 2, 3, 4, 5, 6

# ---- 分区 ----
STREET = {"x0": 16, "x1": 23, "y0": 1, "y1": 9}          # 南北主街
MARKET = {"x0": 12, "x1": 27, "y0": 3, "y1": 5}          # 市集广场
MID_STREET = {"x0": 10, "x1": 29, "y0": 9, "y1": 10}     # 东西中街
CLINIC_YARD = {"x0": 2, "x1": 9, "y0": 13, "y1": 14}     # 药庐前院
TEA_FIELD = {"x0": 10, "x1": 14, "y0": 11, "y1": 14}     # 茶园（可走）
STREAM = {"x0": 11, "x1": 29, "y0": 16, "y1": 17}        # 溪水
STREAM_WIDE = {"x0": 18, "x1": 21, "y0": 15, "y1": 17}   # 溪水加宽（水潭）
BRIDGE = {"x0": 24, "x1": 25, "y0": 16, "y1": 17}        # 石桥（石板，可走）
SPAWN = {"x": 19, "y": 2}                                 # 与 data/maps.lua village.spawn 一致
# 三个出口格（与 `data/maps.lua` 的 village.exits 同源）：南门↔山庄、山门↔青云山，
# 另两个是室内门。它们决定 `self_check` 的可达性与散布的留白，所以只在这里写一次。
EXITS = {
    "gate": (19, 1),
    "mountain_gate": (10, 20),
    "herb_door": (5, 13),
    "forge_door": (27, 10),
}
# 抖动冻结矩形：**茶园**。`set_terrain_autotile` 的抖动会把草/土换成邻居的 kind，而村庄
# 唯一一块"必须是土"的语义区就是茶园——实测不冻时 25 格里 10 格被换成草/石板（40%），
# 画面读成"茶园不存在了"；靠街那一排还会被抖成石板。山庄冻的是药田（farm.lua 判可开垦），
# 村庄没有农田，所以只有这一块。就地工具要传同一段：`--freeze 10,11,14,14`。
FREEZE = [(TEA_FIELD["x0"], TEA_FIELD["y0"], TEA_FIELD["x1"], TEA_FIELD["y1"])]

# ---- 建筑与道具 ----
# texture：gen_village_art.py / gen_wilds_art.py 的产物；size 与图片像素 1:1；
# pivot=(0,0) 即 transform 为左下角。cell：贴图左下角所在 cell（可带 cell_dx/cell_dy 微调）。
#
# **这里没有 `solid` 字段**（M72-07 的规矩，本包补上）：占地只认 `building_kit` 一条规则——
#   建筑 = 本体∪同名拆层件（屋顶）的**不透明像素**覆盖格 − 门洞；道具 = `entity_meta cell`
#   那一格，横着占两格的按 `building_kit.SCENES["village"].prop_collision` 点名走 `box`/`walk`。
# 门洞声明在同一张表里（南门 (19,1)(19,2)、山门 (10,19)），两边任何一处改了都有反查会红。
ENTITIES = [
    {"name": "VillageClinic", "texture": "assets/art/buildings/village_clinic.png",
     "size": (192, 160), "cell": (3, 15), "order": 0, "role": "building"},
    # 杂货铺与茶摊：横屏方案A（b3132ac4）把这两件从东院 x31../x31.. 搬进市集，
    # 当时只改了 sprite 没重跑生成器 ⇒ 盘上留了一片按旧位置算的隐形墙。这里登记的是
    # **盘上真值**，solid 从此跟着 sprite 走。
    {"name": "VillageShop", "texture": "assets/art/buildings/village_shop.png",
     "size": (192, 160), "cell": (21, 6), "order": 0, "role": "building"},
    {"name": "VillageForge", "texture": "assets/art/buildings/village_forge.png",
     "size": (128, 64), "cell": (25, 11), "order": 0, "role": "building"},
    {"name": "VillageTeaStall", "texture": "assets/art/buildings/village_teastall.png",
     "size": (128, 64), "cell": (21, 12), "order": 0, "role": "building"},
    {"name": "VillageHouseA", "texture": "assets/art/buildings/village_house_a.png",
     "size": (128, 96), "cell": (16, 18), "order": 0, "role": "building"},
    {"name": "VillageHouseB", "texture": "assets/art/buildings/village_house_b.png",
     "size": (128, 96), "cell": (27, 18), "order": 0, "role": "building"},
    {"name": "VillageShrine", "texture": "assets/art/buildings/village_shrine.png",
     "size": (64, 64), "cell": (22, 19), "order": 0, "role": "building"},
    # 村口牌坊：门洞 (19,1)(19,2) 是 `data/maps.lua` 的村↔山庄出口格，图上那一格必须是洞
    # （`gen_village_art.building_gate` 把「云泽村」匾挂在额枋就是为了留出通道）。
    {"name": "VillageGate", "texture": "assets/art/buildings/village_gate.png",
     "size": (96, 96), "cell": (18, 0), "order": 5, "role": "building"},
    # ---- 道具 ----
    # 石桥：铺在溪水上，碰撞模式 `walk`——它是过河的唯一通道，标成实心就等于封死村北。
    {"name": "VillageBridge", "texture": "assets/art/props/village_bridge.png",
     "size": (64, 64), "cell": (24, 16), "order": 8, "role": "prop"},
    # 渡口木栈桥（含乌篷船）：贴在水面 y17 一行，`root` 单格根碰撞。
    # **`over_water` 是显式豁免**（见 `self_check` 的锚点判据）：栈桥的木板本来就该铺在
    # 溪面上，把它挪到岸上它就不再是栈桥。锚点那格已由溪水天然实心，不额外挡人。
    {"name": "VillagePier", "texture": "assets/art/props/village_pier.png",
     "size": (96, 32), "cell": (12, 17), "order": 8, "role": "prop", "over_water": True},
    # 市集摊位/招幌/灯笼杆：摊位与山石 64×40 横占两格 → `box`；杆与幌单格。
    {"name": "VillageStallA", "texture": "assets/art/props/village_stall.png",
     "size": (64, 40), "cell": (13, 2), "order": 10, "role": "prop"},
    {"name": "VillageStallB", "texture": "assets/art/props/village_stall.png",
     "size": (64, 40), "cell": (22, 2), "order": 10, "role": "prop"},
    {"name": "VillageBannerA", "texture": "assets/art/props/village_banner.png",
     "size": (32, 56), "cell": (15, 4), "order": 10, "role": "prop"},
    {"name": "VillageBannerB", "texture": "assets/art/props/village_banner.png",
     "size": (32, 56), "cell": (22, 4), "order": 10, "role": "prop"},
    {"name": "VillageLanternA", "texture": "assets/art/props/village_lantern.png",
     "size": (24, 64), "cell": (17, 2), "order": 10, "role": "prop"},
    {"name": "VillageLanternB", "texture": "assets/art/props/village_lantern.png",
     "size": (24, 64), "cell": (21, 2), "order": 10, "role": "prop"},
    # 水缸：复用山庄的通用道具（一件多用，见 gen_village_art 模块 docstring）。
    {"name": "VillageJar", "texture": "assets/art/props/manor_jar.png",
     "size": (24, 30), "cell": (9, 13), "cell_dx": 4, "order": 10, "role": "prop"},
    # 山石（山谷北缘）。A 以前写 (11,18)：`box` 模式让它横占 x11..12 两行，正好把
    # **渡口那一块 y18..20 的唯一去路**（沿溪岸 x9..10 → x13）堵死，`places.lua:village_dock`
    # 从出生点不可达。挪到 (13,20) = 渡口空地的北边一角，挨着北墙，不碰任何通道。
    {"name": "VillageRockA", "texture": "assets/art/props/village_rock.png",
     "size": (64, 40), "cell": (13, 20), "order": 20, "role": "prop"},
    {"name": "VillageRockB", "texture": "assets/art/props/village_rock.png",
     "size": (64, 40), "cell": (37, 18), "order": 20, "role": "prop"},
    {"name": "VillageBambooA", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (34, 20), "order": 20, "role": "prop"},
    {"name": "VillageBambooB", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (20, 20), "order": 20, "role": "prop"},
    {"name": "VillageBambooC", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (33, 20), "order": 20, "role": "prop"},
    # 老树三件：本体是拆层后的 `manor_tree_trunk.png`，树冠由 `set_foreground_occluders`
    # 按 `layer_kit` 的登记表挂上去——这里登记**整图**，拆层是那条代码路径的事。
    {"name": "VillageTreeA", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (5, 20), "order": 20, "role": "prop"},
    {"name": "VillageTreeB", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (26, 20), "order": 20, "role": "prop"},
    {"name": "VillageTreeC", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (36, 20), "order": 20, "role": "prop"},
    # 村北山门（通往青云山道）：128×96 的石体压 x9..12 三行、中间留一个真洞 (10,20)。
    # 走 `box` 而不是 `root`：只标锚点 (9,20) 会让整座门楼只有左边一格挡人。
    # cell 在 M72-11 从 (9,19) 挪到 (9,20)：**门楼要压在北墙上**（y21..22 是院墙两行），
    # 这样它最底那一行才是"人站着的那格" = `maps.lua` 的出口格 (10,20)（实测占比 0.156，是洞）。
    # 挪之前洞在 (10,19)、出口格 (10,20) 是门楼的石体——画着一道山门却走不进去。
    {"name": "VillageMountainGate", "texture": "assets/art/buildings/wilds_mountain_gate.png",
     "size": (128, 96), "cell": (9, 20), "order": 5, "role": "prop"},
]


def prop_entity_lines(start_id=2):
    """ENTITIES → (实体行, 下一个可用 id)。

    `build_scene` 与 `occupied_cells` 都调这一个函数：算碰撞用的那批 sprite 必须与写进
    场景的那批是同一批，否则"按图算的占地"与"画出来的房子"会各算各的。
    """
    lines = []
    next_id = start_id
    for entry in ENTITIES:
        cell_x, cell_y = entry["cell"]
        world_x = cell_x * TILE + entry.get("cell_dx", 0)
        world_y = cell_y * TILE + entry.get("cell_dy", 0)
        width, height = entry["size"]
        lines.append(f"entity\t{next_id}\t{entry['name']}")
        lines.append(sprite_line(next_id, entry["texture"], width, height,
                                 entry.get("layer", "world"), entry["order"]))
        lines.append(transform_line(next_id, float(world_x), float(world_y)))
        lines.append(f"entity_meta\t{next_id}\trole\t{entry['role']}")
        lines.append(f"entity_meta\t{next_id}\tcell\t{cell_x},{cell_y}")
        next_id += 1
    return lines, next_id


def occupied_cells(ground):
    """每栋建筑/每件道具该实心哪些格——**只认 `building_kit` 这一条规则**。

    拆层要先过一遍 `fo.plan`：占地是"本体∪同名屋顶件"的不透明格并集，而屋顶那件在盘上
    是独立实体（M72-06），只喂未拆层的 ENTITIES 会少算屋檐压出去的那一圈。
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
    """solid 布尔矩阵：`occupied_cells` 的格集换算回来。阻挡行走、且每格都有可视依据。"""
    cells, readings = occupied_cells(ground)
    return [[1 if (x, y) in cells else 0 for x in range(W)] for y in range(H)], readings


def build_tiles():
    """地表 tile 值矩阵 tiles[y][x]（**抖动前**的形状：这里只画分区，犬牙交给 `jitter`）。"""
    ground = [[GRASS] * W for _ in range(H)]
    for region in (MARKET, MID_STREET, CLINIC_YARD):
        for y in range(region["y0"], region["y1"] + 1):
            for x in range(region["x0"], region["x1"] + 1):
                ground[y][x] = STONE
    for y in range(STREET["y0"], STREET["y1"] + 1):
        for x in range(STREET["x0"], STREET["x1"] + 1):
            ground[y][x] = STONE
    for y in range(TEA_FIELD["y0"], TEA_FIELD["y1"] + 1):
        for x in range(TEA_FIELD["x0"], TEA_FIELD["x1"] + 1):
            ground[y][x] = DIRT
    for region in (STREAM, STREAM_WIDE):
        for y in range(region["y0"], region["y1"] + 1):
            for x in range(region["x0"], region["x1"] + 1):
                ground[y][x] = WATER
    # 石桥：溪水上铺石板，成为可走的过河通道（必须在溪水之后写）。
    for y in range(BRIDGE["y0"], BRIDGE["y1"] + 1):
        for x in range(BRIDGE["x0"], BRIDGE["x1"] + 1):
            ground[y][x] = STONE
    # 外圈院墙（南侧留村口，x18..20 仍是墙，靠牌坊贴图盖住）。
    # 底部两行都是墙：H 从 22 补到 23 时保持原下沿（H-2）仍是墙，新行压在最底。
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
    import 这里，两条路就必然撒出同一段装饰。

    **硬留白是空集**：山庄那一块是药田（作物画在 FarmGrid 上、湿痕在第三张，装饰一压上去
    就看不清哪块浇过）。村庄没有农田，所以没有"必须整块空"的区域——但茶园要挡中景带，
    不然一棵灌木站在茶行中间。
    """
    places = [(cx, cy) for _, cx, cy in read_village_place_cells()]
    props = [entry["cell"] for entry in ENTITIES]
    keys = [(SPAWN["x"], SPAWN["y"])] + list(EXITS.values())
    tea = [(x, y) for y in range(TEA_FIELD["y0"], TEA_FIELD["y1"] + 1)
           for x in range(TEA_FIELD["x0"], TEA_FIELD["x1"] + 1)]
    return (set(), dk.mid_avoid(W, H, props + tea, places, keys))


def cell_to_world(cx, cy):
    """cell 左下角世界坐标（y 朝上，row 0 在最底）"""
    return (cx + 0.5) * TILE, (cy + 0.5) * TILE


def row_major(grid):
    return "".join(str(grid[y][x]) for y in range(H) for x in range(W))


def transform_line(entity_id, x, y):
    # M32：统一的 Transform 形态由 `pixel_kit.transform_line` 单点产出
    # （引擎读侧只认 `position/rotation/scale`，2D 旧形态已删除；见该函数注释）。
    from pixel_kit import transform_line as spawn_transform

    return spawn_transform(entity_id, x, y)


def sprite_line(entity_id, texture, width, height, layer, order):
    return (
        f"component\t{entity_id}\tSprite\ttexture={texture};size={width}x{height};"
        f"pivot=0.000,0.000;tint=1.000,1.000,1.000,1.000;sorting_layer={layer};order={order}"
    )


def build_scene(ground, solid):
    solid_str = row_major(solid)
    table = tk.load_ramp(SCENE)
    cells = [ground[y][x] for y in range(H) for x in range(W)]
    # 抖动（犬牙交界）与 tint 撒布走 `set_terrain_autotile` 那一份实现：两条路
    # （这里从零生成 / 那里就地改盘上场景）必须产出同一个字节序列，口径见该文件 docstring。
    cells, _changed = jitter(cells, solid_str, W, H, freeze_cells(FREEZE, W, H))
    tints = scatter_tints(W, H, [tint_id for tint_id, _ in tk.ground_tints(table)])
    # 水潭心按形状压一档 `shaded`（M72-07 在山庄池塘上量的那条）：溪流是长条，只有
    # x18..21 那块加宽的水潭有"四邻皆水"的内部格，这一档让它读成深一点的一汪水。
    tints.update(interior_tints(cells, W, H, tk.kind_id(table, "water"),
                                tk.ground_tints(table)[0][0]))
    lines = [
        "scene_version=2",
        # 场景名必须等于地图键（data/maps.lua 的 `village`）：main.lua 用
        # `is_playable(scene.reload())` 判定玩法地图，写错就进不去。
        "name=village",
        "meta\tdisplay_name\t江湖有田·云泽村",
        "meta\tgame\tjianghu_youtian",
        "meta\ttile_size\t32",
        # 横屏方案A：设计分辨率 1280×720（与 `scripts/ui/layout.lua` 一致）。
        "meta\tui_reference_height\t720",
        "meta\tui_reference_width\t1280",
        "entity\t1\tGroundMap",
        transform_line(1, 0.0, 0.0),
        "component\t1\tTilemap\t"
        + tk.tilemap_component(TILE, W, H, GROUND_TEXTURE, cells, solid_str,
                               table["cols"], table["rows"],
                               tile_kinds=table["kind_count"], tints=tints,
                               tints_rgb=tk.tints_rgb_string(table)),
        "entity_meta\t1\trole\tground_tilemap",
    ]
    # 2..N 建筑与道具：与 `occupied_cells` 走同一个 `prop_entity_lines`，
    # 所以占地算的就是画出来的这批。
    prop_lines, next_id = prop_entity_lines()
    lines.extend(prop_lines)
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
    title_id = next_id
    lines.append(f"entity\t{title_id}\tHudTitle")
    lines.append(
        f"component\t{title_id}\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;"
        "offsets=24.00,20.00,672.00,44.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000"
    )
    lines.append(
        f"component\t{title_id}\tText\ttext=云泽村 · 南市街;font_size=18.00;color=0.949,0.918,0.847"
    )
    lines.append(transform_line(title_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{title_id}\trole\thud_title")
    hint_id = next_id + 1
    lines.append(f"entity\t{hint_id}\tHudHint")
    lines.append(
        # 地图自带操作提示放在**地图标题正下方**（y=50）；M32 竖版：宽度收到 672，
        # 底部信息区整块留给 HUD 的动态反馈/目标/键位行。
        f"component\t{hint_id}\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;"
        "offsets=24.00,50.00,672.00,72.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000"
    )
    lines.append(
        f"component\t{hint_id}\tText\ttext=WASD 移动 · E 交互(对话/商店/出口) · T 睡觉 · Q 吐纳 · J 普攻 · K 武学"
        ";font_size=14.00;color=0.850,0.900,1.000"
    )
    lines.append(transform_line(hint_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{hint_id}\trole\thud_hint")
    # 场景相机（本地图唯一的 `Camera` 实体）：把 2D 取景范围锁在整张地图的矩形内，
    # 运行期由 `scripts/world/camera_view.lua` 让相机跟随玩家（`camera2d.follow`）。
    camera_id = hint_id + 1
    lines.append(f"entity\t{camera_id}\tGameCamera")
    lines.append(camera_line(camera_id, W, H))
    lines.append(transform_line(camera_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{camera_id}\trole\tgame_camera")
    # M72-05：装饰散布。id 从**装饰段之前的最后一张结构实体**之后起，一个既有 id 都不动
    # ——挪 id 会让 Lua 侧 `entity.find` 与黄金表全部重排，而装饰是纯加法。
    # 输入用**抖动后**的格子图（盘上就是这个），不然"生成器看到的草/土分布"与实机不一致。
    jittered = [cells[y * W:(y + 1) * W] for y in range(H)]
    exclude, avoid = decor_reserved()
    placements = dk.scatter(W, H, jittered, solid, exclude, mid_avoid=avoid)
    lines.extend(dk.decor_lines(camera_id + 1, placements, table,
                                lambda cell: tints.get(cell[1] * W + cell[0], 0)))
    # M72-06：前景遮断拆层（树冠/屋顶）。与 `set_foreground_occluders.py`（就地改盘上场景）
    # **共用同一条 `plan()`+`apply_patch()`**：这一段的存在感极低（拼回去画面逐字节相同），
    # 一旦重跑生成器把它抹掉，盘上只是"前景没了"，没有任何一张图会报错。
    fo.assert_id_band_free(lines)
    base_updates, occluders = fo.plan(lines, fo.lk.load())
    lines = fo.apply_patch(lines, base_updates, occluders)
    # M72-06 C 段：地面投影贴片。必须在 `fo.apply_patch` **之后**——贴片按拆层后的本体
    # 贴图算落地行，也按拆层后的实体排除树冠/屋顶块（空中块不配自己的投影）。
    cs.assert_id_band_free(lines)
    shadows, canvases = cs.build(lines, cs.lk.load(), cs.load_art(lines))
    lines = cs.apply_patch(lines, cs.shadow_lines(shadows))
    cs.write_art(canvases, ".")
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------
# 契约自检
# ---------------------------------------------------------------------------


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


def read_village_place_cells(
    path=os.path.join(PROJECT_ROOT, "scripts", "data", "places.lua"),
):
    """从 data/places.lua 里抠出 map = "village" 的地点格（契约校验用，不执行 Lua）。"""
    import re

    if not os.path.exists(path):
        raise FileNotFoundError(
            f"{path} 读不到：places.lua 的地点格是散布的**留白输入**，宽容地当成"
            "「一个地点也没有」继续跑，草就撒到 NPC 门口"
        )
    cells = []
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            match = re.match(
                r'\s*(\w+)\s*=\s*\{\s*map\s*=\s*"village"\s*,\s*cell\s*=\s*\{\s*x\s*=\s*(\d+)\s*,\s*y\s*=\s*(\d+)\s*\}',
                line,
            )
            if match:
                cells.append((match.group(1), int(match.group(2)), int(match.group(3))))
    return cells


def pockets(solid):
    """进不去的可走格块（除最大连通块以外的那些）。

    为什么要把连通块报出来而不是只测 places：一张图里"看得见但走不进去"的草地是玩家
    会用脚投票的东西，而它不会让任何一条 places 断言变红——只有连通块分解能把它摆出来。
    本包的口径：与金标 manor 同档（manor 实测 4 个小块，最大的 5 格），不追求零死格。
    """
    seen, comps = set(), []
    for y in range(H):
        for x in range(W):
            if solid[y][x] or (x, y) in seen:
                continue
            stack, comp = [(x, y)], [(x, y)]
            seen.add((x, y))
            while stack:
                cx, cy = stack.pop()
                for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    nx, ny = cx + dx, cy + dy
                    if 0 <= nx < W and 0 <= ny < H and not solid[ny][nx] and (nx, ny) not in seen:
                        seen.add((nx, ny))
                        comp.append((nx, ny))
                        stack.append((nx, ny))
            comps.append(comp)
    comps.sort(key=len, reverse=True)
    return comps[0], comps[1:]


def walkable_summary(solid):
    """每行可走列的紧凑描述，用于同步 data/places.lua 的"可行走格事实"注释。"""
    lines = []
    for y in range(H - 1, -1, -1):
        ranges = []
        start = None
        for x in range(W):
            if not solid[y][x]:
                if start is None:
                    start = x
            else:
                if start is not None:
                    ranges.append((start, x - 1))
                    start = None
        if start is not None:
            ranges.append((start, W - 1))
        text = " ".join(f"{a}" if a == b else f"{a}-{b}" for a, b in ranges)
        lines.append(f"    y{y:>2}: {text if text else '(全实心)'}")
    return lines


def check_texture_sizes():
    """交叉校验：scene 里声明的 sprite 尺寸必须与贴图像素尺寸一致。

    引擎会把贴图缩放到 `size=` 指定的尺寸，声明错了不会报错、只会被悄悄拉伸
    （渡口栈桥就踩过：art 是 64×48、scene 写 96×32）。
    """
    from pixel_kit import decode_png

    problems = []
    for entry in ENTITIES:
        path = os.path.join(PROJECT_ROOT, *entry["texture"].split("/"))
        if not os.path.exists(path):
            problems.append(f"{entry['name']}: 贴图缺失 {entry['texture']}")
            continue
        width, height, _, _ = decode_png(path)
        want_w, want_h = entry["size"]
        if (width, height) != (want_w, want_h):
            problems.append(
                f"{entry['name']}: 贴图 {width}x{height} != scene 声明 {want_w}x{want_h}"
                f"（会被引擎拉伸）—— 改 {entry['texture']} 或本文件 size"
            )
    return problems


def self_check(ground, solid):
    """契约自检：出生点/出口/地点格可走、门洞可站可达、桥面可走、茶园是土、建筑锚点不泡水。"""
    problems = []
    walk = reachable_from(solid, (SPAWN["x"], SPAWN["y"]))
    for name, cell in (("spawn", (SPAWN["x"], SPAWN["y"])), *EXITS.items()):
        x, y = cell
        if solid[y][x]:
            problems.append(f"{name} ({x},{y}) 变成 solid（出口格不可走）")
        elif (x, y) not in walk:
            problems.append(f"{name} ({x},{y}) 从出生点不可达")
    for name, x, y in read_village_place_cells():
        if solid[y][x]:
            problems.append(f"places.lua:{name} ({x},{y}) 是 solid（NPC 会走不出来）")
        elif (x, y) not in walk:
            problems.append(f"places.lua:{name} ({x},{y}) 从出生点不可达")
        # **地点格不许与出口格四邻相邻**（M72-11 实测出来的契约，不是口味）：
        # `world/map_manager.lua:register_exit_target` 给出口的 priority=20，高于对话/商店(10)，
        # 而它的 query 既接受"站在出口格上"也接受"面向的那一格是出口格"。NPC 的地点格只要紧贴
        # 出口格，玩家被传送到 NPC 脚下按 E 时面向格就是出口 ⇒ 抢赢对话。红法实录：
        # `village_clinic` 曾按"贴着药庐门前"放在 (5,14)，而村→药庐的出口格是 (5,13)，
        # `jhyt_m18_stats` 当场报 `mode = "Transition", want "Dialogue"`。
        for exit_name, (exit_x, exit_y) in EXITS.items():
            if abs(exit_x - x) + abs(exit_y - y) == 1:
                problems.append(f"places.lua:{name} ({x},{y}) 与出口 {exit_name} "
                                f"({exit_x},{exit_y}) 四邻相邻："
                                "按 E 会被出口（priority 20）抢走对话/商店（priority 10）")
    # 门洞：`building_kit.SCENES[SCENE].openings` 声明的洞必须**可站且从出生点走到**——
    # 洞画对了但被别的东西围死，玩家一样出不去（南门是村↔山庄的出口格，断了就切图）。
    for name, cells in bk.openings_table(SCENE).items():
        for cell in cells:
            cx, cy = cell
            if solid[cy][cx]:
                problems.append(f"{name} 的门洞 {cell} 又被标成 solid：占地规则与 openings 打架")
            elif (cx, cy) not in walk:
                problems.append(f"{name} 的门洞 {cell} 从出生点不可达")
    # 建筑锚点（贴图左下角那一格）不许是**水**：锚点在水里就是整栋泡在溪里。
    # 例外必须点名：`over_water=True`（栈桥/渡口那类"本来就铺在水面上"的美术）。
    # 判据不写成"只查 role=building"——那等于承认道具可以泡水里，而绝大多数道具确实不行。
    for entry in ENTITIES:
        cx, cy = entry["cell"]
        if ground[cy][cx] == WATER and not entry.get("over_water"):
            problems.append(f"{entry['name']} 的锚点 ({cx},{cy}) 在水格上：整栋泡在溪里"
                            f"（真要铺在水面上就显式写 over_water=True）")
    # 桥面必须可走（过河唯一通道），茶园整块必须是夯土（抖动冻住的复核）。
    for y in range(BRIDGE["y0"], BRIDGE["y1"] + 1):
        for x in range(BRIDGE["x0"], BRIDGE["x1"] + 1):
            if solid[y][x]:
                problems.append(f"桥面 ({x},{y}) 是 solid，过不了河")
    if not all(ground[y][x] == DIRT
               for y in range(TEA_FIELD["y0"], TEA_FIELD["y1"] + 1)
               for x in range(TEA_FIELD["x0"], TEA_FIELD["x1"] + 1)):
        problems.append("茶园矩形里出现非夯土格（抖动没冻住它）")
    problems.extend(check_texture_sizes())
    return problems


def main():
    parser = argparse.ArgumentParser(description="从零装配 scenes/village.scene")
    parser.add_argument("--out", default=os.path.join("scenes", "village.scene"),
                        help="写出路径。对拍请指到 .temp 再与盘上场景逐行 diff。")
    args = parser.parse_args()
    ground = build_tiles()
    solid, readings = build_solid(ground)
    text = build_scene(ground, solid)
    with open(args.out, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)
    print(f"wrote {args.out} ({W}x{H}, {len(ENTITIES)} 个建筑/道具实体)")
    for reading in readings:
        print("   %-22s 视觉 %-3d 格 solid %-3d 格 门洞 %s%s" % (
            reading["name"], reading["visual"], reading["solid"], reading["openings"] or "-",
            "（拆层件：" + ",".join(reading["mates"]) + "）" if reading["mates"] else ""))
    problems = self_check(ground, solid)
    if problems:
        for problem in problems:
            print("[FAIL]", problem)
        raise SystemExit(1)
    places = read_village_place_cells()
    print(f"契约自检 ok：出生点/出口/门洞/桥面/茶园/建筑锚点，"
          f"places.lua {len(places)} 个村庄地点格全部可走且可达")
    main_block, dead = pockets(solid)
    print(f"可行走连通块：主块 {len(main_block)} 格，进不去的小块 {len(dead)} 个 "
          + " ".join(f"{len(c)}格@{min(p[0] for p in c)},{min(p[1] for p in c)}"
                     for c in dead))
    print("可行走格事实（同步 data/places.lua 注释用）：")
    for line in walkable_summary(solid):
        print(line)


if __name__ == "__main__":
    main()
