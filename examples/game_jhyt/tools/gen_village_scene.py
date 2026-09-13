# -*- coding: utf-8 -*-
"""《江湖有田》云泽村场景生成（scenes/village.scene）——山谷村落版。

用法：工程根目录 python tools/gen_village_scene.py

美术：地表用 tools/gen_village_art.py 的 6 列像素 tileset（`assets/art/tilesets/village/`），
建筑/道具用同工具产出的独立 sprite（引擎 sprite 不能选 UV 区域，一件一张图）。
改布局改本文件，改画面改 gen_village_art.py，两边都跑一次即可复现。

引擎 Tilemap 约定（scene_version=2，见 engine core/tilemap.rs）：
  tile_size=WxH;width=W;height=H;texture=PATH;tiles=<row-major 0..6>;solid=<row-major 0/1>
  * tile 值 0 = 空，1..6 = tileset 第 N 列（引擎 TILE_PALETTE 写死 6 列）；
  * tileset 必须正好 6 列宽、高度 32；
  * 单格 32 世界单位，y 朝上，row 0 在最底。
列语义（三场景共用，不可重排）：1=草地 2=夯土 3=石板路 4=院墙 5=青瓦 6=水

山谷村落布局（cell，0-index，含端点）：
  村口牌坊 ：x18..20, y0..2（贴图压住南院墙，门洞 x19 可走 = 出口格 19,1）
  主街     ：x16..23, y1..9 石板；市集广场 x12..27, y3..5；中街 x10..29, y9..10
  药庐     ：x3..8, y15..19（6×5）solid；前院石板 x2..9, y13..14
  杂货铺   ：x31..36, y15..19（6×5）solid；前院石板 x30..37, y13..14
  铁匠铺   ：x25..28, y11..12（4×2）solid；茶摊 x31..34, y10..11（4×2）solid
  民居     ：house_a x16..19, y18..20；house_b x27..30, y18..20（各 4×3）solid
  土地庙   ：x22..23, y19..20（2×2）solid
  茶园     ：x10..14, y11..14 夯土，可走
  溪水     ：y16..17, x11..29（x18..21 加宽到 y15..17），solid
  石桥     ：x24..25, y16..17 铺石板（可走）→ 北岸唯一正门通道
  北岸     ：y18..20 草地（民居/土地庙/渡口），西侧经 x9..10 走廊回到村内

**与 data/places.lua 的契约**：village 的 16 个地点格必须可走且从出生点可达，
本工具每次生成都会做 flood fill 自检，失败直接非零退出。
"""
import os

import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from character_art import player_art  # noqa: E402  主角形象（与 manifest 同源）
import re

# 地图尺寸（maps.lua 的 spawn/exit 边界校验按 40x22 处理，不要改）
W, H = 40, 22
TILE = 32

# 列映射（写 scene tile 用）
GRASS, DIRT, STONE, WALL, ROOF, WATER = 1, 2, 3, 4, 5, 6

GROUND_TEXTURE = "assets/art/tilesets/village/village_ground.png"

# ---- 分区 ----
STREET = {"x0": 16, "x1": 23, "y0": 1, "y1": 9}          # 南北主街
MARKET = {"x0": 12, "x1": 27, "y0": 3, "y1": 5}          # 市集广场
MID_STREET = {"x0": 10, "x1": 29, "y0": 9, "y1": 10}     # 东西中街
CLINIC_YARD = {"x0": 2, "x1": 9, "y0": 13, "y1": 14}     # 药庐前院
SHOP_YARD = {"x0": 30, "x1": 37, "y0": 13, "y1": 14}     # 杂货铺前院
TEA_FIELD = {"x0": 10, "x1": 14, "y0": 11, "y1": 14}     # 茶园（可走）
STREAM = {"x0": 11, "x1": 29, "y0": 16, "y1": 17}        # 溪水
STREAM_WIDE = {"x0": 18, "x1": 21, "y0": 15, "y1": 17}   # 溪水加宽（水潭）
BRIDGE = {"x0": 24, "x1": 25, "y0": 16, "y1": 17}        # 石桥（石板，可走）
SPAWN = {"x": 19, "y": 2}                                # 与 data/maps.lua village.spawn 一致
EXIT = {"x": 19, "y": 1}                                 # 与 data/maps.lua village 出口格一致

# ---- 建筑（sprite 尺寸与占格一致；solid = 整块占格）----
BUILDINGS = [
    {"name": "VillageClinic", "texture": "assets/art/buildings/village_clinic.png",
     "size": (192, 160), "cell": (3, 15), "order": 0, "solid": (3, 8, 15, 19)},
    {"name": "VillageShop", "texture": "assets/art/buildings/village_shop.png",
     "size": (192, 160), "cell": (31, 15), "order": 0, "solid": (31, 36, 15, 19)},
    {"name": "VillageForge", "texture": "assets/art/buildings/village_forge.png",
     "size": (128, 64), "cell": (25, 11), "order": 0, "solid": (25, 28, 11, 12)},
    {"name": "VillageTeaStall", "texture": "assets/art/buildings/village_teastall.png",
     "size": (128, 64), "cell": (31, 10), "order": 0, "solid": (31, 34, 10, 11)},
    {"name": "VillageHouseA", "texture": "assets/art/buildings/village_house_a.png",
     "size": (128, 96), "cell": (16, 18), "order": 0, "solid": (16, 19, 18, 20)},
    {"name": "VillageHouseB", "texture": "assets/art/buildings/village_house_b.png",
     "size": (128, 96), "cell": (27, 18), "order": 0, "solid": (27, 30, 18, 20)},
    {"name": "VillageShrine", "texture": "assets/art/buildings/village_shrine.png",
     "size": (64, 64), "cell": (22, 19), "order": 0, "solid": (22, 23, 19, 20)},
    # 村口牌坊：只挡两根立柱，门洞 (19,1)(19,2) 保持可走（出口格 + 出生点）
    {"name": "VillageGate", "texture": "assets/art/buildings/village_gate.png",
     "size": (96, 96), "cell": (18, 0), "order": 5,
     "solid_cells": [(18, 1), (18, 2), (20, 1), (20, 2)]},
]

# ---- 道具（cell = 贴图左下角；solid_cells = 需要标 1 的格）----
PROPS = [
    # 石桥：铺在溪水上，自身不加 solid（那两格已被改成石板，可走）
    {"name": "VillageBridge", "texture": "assets/art/props/village_bridge.png",
     "size": (64, 64), "cell": (24, 16), "cell_dy": 0, "order": 8, "solid_cells": []},
    # 渡口木栈桥（含小船）：贴在水面 y17 一行，北岸紧邻
    {"name": "VillagePier", "texture": "assets/art/props/village_pier.png",
     "size": (96, 32), "cell": (12, 17), "order": 8, "solid_cells": []},
    # 市集摊位（两张，桌面前沿各占 2 格）
    {"name": "VillageStallA", "texture": "assets/art/props/village_stall.png",
     "size": (64, 40), "cell": (13, 2), "order": 10, "solid_cells": [(13, 2), (14, 2)]},
    {"name": "VillageStallB", "texture": "assets/art/props/village_stall.png",
     "size": (64, 40), "cell": (22, 2), "order": 10, "solid_cells": [(22, 2), (23, 2)]},
    # 招幌（商铺幌子，不写字）
    {"name": "VillageBannerA", "texture": "assets/art/props/village_banner.png",
     "size": (32, 56), "cell": (15, 4), "order": 10, "solid_cells": [(15, 4)]},
    {"name": "VillageBannerB", "texture": "assets/art/props/village_banner.png",
     "size": (32, 56), "cell": (22, 4), "order": 10, "solid_cells": [(22, 4)]},
    # 灯笼杆（村口两侧）
    {"name": "VillageLanternA", "texture": "assets/art/props/village_lantern.png",
     "size": (24, 64), "cell": (17, 2), "order": 10, "solid_cells": [(17, 2)]},
    {"name": "VillageLanternB", "texture": "assets/art/props/village_lantern.png",
     "size": (24, 64), "cell": (21, 2), "order": 10, "solid_cells": [(21, 2)]},
    # 水缸（药庐前院）
    {"name": "VillageJar", "texture": "assets/art/props/manor_jar.png",
     "size": (24, 30), "cell": (9, 13), "cell_dx": 4, "order": 10, "solid_cells": [(9, 13)]},
    # 山石（山谷北缘）
    {"name": "VillageRockA", "texture": "assets/art/props/village_rock.png",
     "size": (64, 40), "cell": (11, 18), "order": 20, "solid_cells": [(11, 18)]},
    {"name": "VillageRockB", "texture": "assets/art/props/village_rock.png",
     "size": (64, 40), "cell": (37, 18), "order": 20, "solid_cells": [(37, 18)]},
    # 竹丛 / 老树（复用山庄的通用道具，一件多用）
    {"name": "VillageBambooA", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (34, 20), "order": 20, "solid_cells": [(34, 20)]},
    {"name": "VillageBambooB", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (20, 20), "order": 20, "solid_cells": [(20, 20)]},
    {"name": "VillageBambooC", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (33, 20), "order": 20, "solid_cells": [(33, 20)]},
    {"name": "VillageTreeA", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (5, 20), "order": 20, "solid_cells": [(5, 20)]},
    {"name": "VillageTreeB", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (26, 20), "order": 20, "solid_cells": [(26, 20)]},
    {"name": "VillageTreeC", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (36, 20), "order": 20, "solid_cells": [(36, 20)]},
    # 村北山门（通往青云山道）：只挡 y19 两根立柱，门洞 (10,19)(10,20) 可走 = 出口格。
    # 早期把 (9,20)(12,20) 也标了 solid，结果把北岸 x12..15 那段走廊切断，
    # 契约自检报出 village_dock 不可达 —— 立柱只留在 y19，y20 保持贯通。
    {"name": "VillageMountainGate", "texture": "assets/art/buildings/wilds_mountain_gate.png",
     "size": (128, 96), "cell": (9, 19), "order": 5,
     "solid_cells": [(9, 19), (12, 19)]},
]


def rect_cells(bounds):
    x0, x1, y0, y1 = bounds
    return [(x, y) for y in range(y0, y1 + 1) for x in range(x0, x1 + 1)]


def inside(cell, region):
    return region["x0"] <= cell[0] <= region["x1"] and region["y0"] <= cell[1] <= region["y1"]


def build_tiles():
    """地表 tile 值矩阵 tiles[y][x]。"""
    ground = [[GRASS] * W for _ in range(H)]
    for region in (MARKET, MID_STREET, CLINIC_YARD, SHOP_YARD):
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
    # 石桥：溪水上铺石板，成为可走的过河通道（必须在溪水之后写）
    for y in range(BRIDGE["y0"], BRIDGE["y1"] + 1):
        for x in range(BRIDGE["x0"], BRIDGE["x1"] + 1):
            ground[y][x] = STONE
    # 外圈院墙（南侧留村口，x18..20 仍是墙，靠牌坊贴图盖住）
    for x in range(W):
        ground[0][x] = WALL
        ground[H - 1][x] = WALL
    for y in range(H):
        ground[y][0] = WALL
        ground[y][W - 1] = WALL
    return ground


def build_solid(ground):
    """solid 矩阵：院墙/水 + 建筑整块 + 道具占格（sprite 本身不参与碰撞）。"""
    solid = [[0] * W for _ in range(H)]
    for y in range(H):
        for x in range(W):
            if ground[y][x] in (WALL, WATER):
                solid[y][x] = 1
    for building in BUILDINGS:
        if "solid" in building:
            for (x, y) in rect_cells(building["solid"]):
                solid[y][x] = 1
        for (x, y) in building.get("solid_cells", []):
            solid[y][x] = 1
    for prop in PROPS:
        for (x, y) in prop["solid_cells"]:
            solid[y][x] = 1
    return solid


def cell_to_world(cx, cy):
    return (cx + 0.5) * TILE, (cy + 0.5) * TILE


def row_major(grid):
    return "".join(str(grid[y][x]) for y in range(H) for x in range(W))


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
    lines = [
        "scene_version=2",
        "name=village",
        "meta\tdisplay_name\t江湖有田·云泽村",
        "meta\tgame\tjianghu_youtian",
        "meta\ttile_size\t32",
        "meta\tui_reference_height\t720",
        "meta\tui_reference_width\t1280",
        "entity\t1\tGroundMap",
        transform_line(1, 0.0, 0.0),
        f"component\t1\tTilemap\ttile_size=32x32;width={W};height={H};texture={GROUND_TEXTURE}"
        f";tiles={row_major(ground)};solid={row_major(solid)}",
        "entity_meta\t1\trole\tground_tilemap",
    ]
    next_id = 2
    for entry in BUILDINGS + PROPS:
        cell_x, cell_y = entry["cell"]
        world_x = cell_x * TILE + entry.get("cell_dx", 0)
        world_y = cell_y * TILE + entry.get("cell_dy", 0)
        width, height = entry["size"]
        role = "building" if entry in BUILDINGS else "prop"
        lines.append(f"entity\t{next_id}\t{entry['name']}")
        lines.append(sprite_line(next_id, entry["texture"], width, height,
                                 entry.get("layer", "world"), entry["order"]))
        lines.append(transform_line(next_id, float(world_x), float(world_y)))
        lines.append(f"entity_meta\t{next_id}\trole\t{role}")
        lines.append(f"entity_meta\t{next_id}\tcell\t{cell_x},{cell_y}")
        next_id += 1
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
        "offsets=24.00,20.00,640.00,44.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000"
    )
    lines.append(
        f"component\t{title_id}\tText\ttext=云泽村 · 南市街;font_size=18.00;color=0.949,0.918,0.847"
    )
    lines.append(transform_line(title_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{title_id}\trole\thud_title")
    hint_id = next_id + 1
    lines.append(f"entity\t{hint_id}\tHudHint")
    lines.append(
        f"component\t{hint_id}\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;"
        "offsets=24.00,686.00,1000.00,708.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000"
    )
    lines.append(
        f"component\t{hint_id}\tText\ttext=WASD 移动 · E 交互(对话/商店/出口) · T 睡觉 · Q 吐纳 · J 普攻 · K 武学"
        ";font_size=14.00;color=0.850,0.900,1.000"
    )
    lines.append(transform_line(hint_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{hint_id}\trole\thud_hint")
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------
# 契约自检
# ---------------------------------------------------------------------------


def reachable_from(solid, start):
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


def read_village_place_cells(path=os.path.join("scripts", "data", "places.lua")):
    cells = []
    if not os.path.exists(path):
        return cells
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            match = re.match(
                r'\s*(\w+)\s*=\s*\{\s*map\s*=\s*"village"\s*,\s*cell\s*=\s*\{\s*x\s*=\s*(\d+)\s*,\s*y\s*=\s*(\d+)\s*\}',
                line,
            )
            if match:
                cells.append((match.group(1), int(match.group(2)), int(match.group(3))))
    return cells


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
    for entry in BUILDINGS + PROPS:
        path = entry["texture"].replace("/", os.sep)
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
    problems = []
    walk = reachable_from(solid, (SPAWN["x"], SPAWN["y"]))
    for name, cell in (("spawn", SPAWN), ("exit", EXIT)):
        x, y = cell["x"], cell["y"]
        if solid[y][x]:
            problems.append(f"{name} ({x},{y}) 变成 solid")
        elif (x, y) not in walk:
            problems.append(f"{name} ({x},{y}) 从出生点不可达")
        elif ground[y][x] != STONE:
            problems.append(f"{name} ({x},{y}) 地表 tile={ground[y][x]}，村口应为石板(3)")
    # 批次 2/3 新增出口格：上山门洞 + 药庐门 + 铁匠铺门（必须可走且可达）
    for name, (x, y) in (("mountain_gate_exit", (10, 20)), ("herb_door", (5, 13)),
                         ("forge_door", (26, 10))):
        if solid[y][x]:
            problems.append(f"{name} ({x},{y}) 是 solid（出口格不可走）")
        elif (x, y) not in walk:
            problems.append(f"{name} ({x},{y}) 从出生点不可达")
    for name, x, y in read_village_place_cells():
        if solid[y][x]:
            problems.append(f"places.lua:{name} ({x},{y}) 是 solid（NPC 会走不出来）")
        elif (x, y) not in walk:
            problems.append(f"places.lua:{name} ({x},{y}) 从出生点不可达")
    for entry in BUILDINGS + PROPS:
        cells = rect_cells(entry["solid"]) if "solid" in entry else entry.get("solid_cells", [])
        for (x, y) in cells:
            if ground[y][x] in (WALL, WATER):
                problems.append(f"{entry['name']} 占用了墙/水格 ({x},{y})")
    # 桥面必须可走，溪水其余部分必须实心
    for y in range(BRIDGE["y0"], BRIDGE["y1"] + 1):
        for x in range(BRIDGE["x0"], BRIDGE["x1"] + 1):
            if solid[y][x]:
                problems.append(f"桥面 ({x},{y}) 是 solid，过不了河")
    problems.extend(check_texture_sizes())
    return problems


def main():
    ground = build_tiles()
    solid = build_solid(ground)
    text = build_scene(ground, solid)
    out = os.path.join("scenes", "village.scene")
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print(f"wrote {out} ({W}x{H}, {len(BUILDINGS)} 建筑 + {len(PROPS)} 道具)")
    problems = self_check(ground, solid)
    if problems:
        for problem in problems:
            print("[FAIL]", problem)
        raise SystemExit(1)
    places = read_village_place_cells()
    print(f"契约自检 ok：spawn/exit/桥面，places.lua {len(places)} 个村庄地点格全部可走且可达")
    print("可行走格事实（同步 data/places.lua 注释用）：")
    for line in walkable_summary(solid):
        print(line)


if __name__ == "__main__":
    main()
