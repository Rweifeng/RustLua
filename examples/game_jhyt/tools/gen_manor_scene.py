# -*- coding: utf-8 -*-
"""《江湖有田》山庄主场景生成（scenes/manor.scene）

用法：工程根目录 python tools/gen_manor_scene.py

美术：地表用 tools/gen_manor_art.py 产出的 6 列像素 tileset，
建筑/道具用同工具产出的独立 sprite（引擎 sprite 不能选 UV 区域，所以一件一张图）。
改布局改本文件，改画面改 gen_manor_art.py，两边都跑一次即可复现。

引擎 Tilemap 组件约定（scene_version=2，见 engine core/tilemap.rs）：
  tile_size=WxH;width=W;height=H;texture=PATH;tiles=<row-major 0..6>;solid=<row-major 0/1>
  * tile 值 0 = 空，1..6 = tileset 第 N 列（引擎 TILE_PALETTE 写死 6 列，不能加第 7 列）；
  * UV-U = (N-1)/6..N/6，UV-V 恒为整幅 0..1 → tileset 必须正好 6 列宽、高度 32；
  * 单格 32 世界单位，y 朝上；索引 index = y*width+x（row-major，y 为行、row 0 在最底）。
列映射（与 farm.lua / gen_manor_art.py 一致，**不可随意改**）：
  manor_ground：1=草地 2=夯土 3=石板路 4=院墙 5=青瓦 6=水
  manor_farm：  1=已耕 2=已耕+浇水 3=苗期 4=生长期 5=成熟 6=稻茬(保留)
  * farm.lua 用 `GroundMap tile == 2` 判定可开垦；overlay 列语义由 farm.lua 的 tile_for() 决定。

布局（cell，0-index，y 朝上，含端点）：
  院墙      ：整圈边界（row0 / row21 / col0 / col39），solid
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
import os

import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from character_art import player_art  # noqa: E402  主角形象（与 manifest 同源）

# 地图尺寸（maps.lua 的 spawn/exit 边界校验按 40x22 处理，不要改）
W, H = 40, 22

# 列映射（写 scene tile 用）
GRASS, DIRT, STONE, WALL, ROOF, WATER = 1, 2, 3, 4, 5, 6

GROUND_TEXTURE = "assets/art/tilesets/manor/manor_ground.png"
FARM_TEXTURE = "assets/art/tilesets/manor/manor_farm.png"
TILE = 32

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
    # 整圈院墙
    for x in range(W):
        ground[0][x] = WALL
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
    return (
        f"component\t{entity_id}\tTransform\tx={x:.6f};y={y:.6f};rotation=0.000000000"
        ";scale_x=1.000000000;scale_y=1.000000000"
    )


def sprite_line(entity_id, texture, width, height, layer, order):
    return (
        f"component\t{entity_id}\tSprite\ttexture={texture};size={width}x{height};"
        f"pivot=0.000,0.000;tint=1.000,1.000,1.000,1.000;sorting_layer={layer};order={order}"
    )


def build_scene(ground, solid):
    tile_str = row_major(ground)
    solid_str = row_major(solid)
    lines = []
    lines.append("scene_version=2")
    # 场景名必须等于地图键（data/maps.lua 的 `manor`）：main.lua 用
    # `is_playable(scene.reload())` 判定玩法地图，写成 main 会导致整局进不去。
    lines.append("name=manor")
    lines.append("meta\tdisplay_name\t江湖有田·山庄")
    lines.append("meta\tgame\tjianghu_youtian")
    lines.append("meta\ttile_size\t32")
    lines.append("meta\tui_reference_height\t720")
    lines.append("meta\tui_reference_width\t1280")
    # 1 地表
    lines.append("entity\t1\tGroundMap")
    lines.append(transform_line(1, 0.0, 0.0))
    lines.append(
        f"component\t1\tTilemap\ttile_size=32x32;width={W};height={H};texture={GROUND_TEXTURE}"
        f";tiles={tile_str};solid={solid_str}"
    )
    lines.append("entity_meta\t1\trole\tground_tilemap")
    # 2 FarmGrid 覆盖耕地 overlay（初始全空，运行期 farm.lua 写 tile/meta）
    empty = "0" * (W * H)
    lines.append("entity\t2\tFarmGrid")
    lines.append(transform_line(2, 0.0, 0.0))
    lines.append(
        f"component\t2\tTilemap\ttile_size=32x32;width={W};height={H};texture={FARM_TEXTURE}"
        f";tiles={empty};solid={empty}"
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
        f"component\t{title_id}\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;"
        "offsets=24.00,20.00,640.00,44.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000"
    )
    lines.append(
        f"component\t{title_id}\tText\ttext=山庄 · 药田垂直切片;font_size=18.00;color=0.949,0.918,0.847"
    )
    lines.append(transform_line(title_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{title_id}\trole\thud_title")
    hint_id = next_id + 1
    lines.append(f"entity\t{hint_id}\tHudHint")
    lines.append(
        f"component\t{hint_id}\tUiLayout\tanchors=0.0000,1.0000,0.0000,1.0000;"
        "offsets=49.07,-66.11,1025.07,-44.11;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,1.0000"
    )
    lines.append(
        f"component\t{hint_id}\tText\ttext=WASD 移动 · E 朝面前格劳作 · T 睡觉到清晨 · Q 吐纳练功 · J 普攻 · K 武学"
        ";font_size=14.00;color=0.850,0.900,1.000"
    )
    lines.append(transform_line(hint_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{hint_id}\trole\thud_hint")
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


def read_manor_place_cells(path=os.path.join("scripts", "data", "places.lua")):
    """从 data/places.lua 里抠出 map = "manor" 的地点格（契约校验用，不执行 Lua）。"""
    import re

    cells = []
    if not os.path.exists(path):
        return cells
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
    ground = build_tiles()
    solid = build_solid(ground)
    text = build_scene(ground, solid)
    out = os.path.join("scenes", "manor.scene")
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
