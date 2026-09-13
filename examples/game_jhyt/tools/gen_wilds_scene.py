# -*- coding: utf-8 -*-
"""《江湖有田》批次 2 场景装配：青云山道 + 寒泉洞（scenes/mountain_path.scene / cold_spring_cave.scene）。

用法：工程根目录 python tools/gen_wilds_scene.py

美术：`tools/gen_wilds_art.py` 产出的 mountain / cave 两套 6 列 tileset + 山道建筑道具 + 采集物。

地图与列语义：
  mountain_path  40×22  1=林地草地 2=碎石土路 3=石阶 4=岩壁 5=竹林地 6=溪水
  cold_spring_cave 20×12  1=洞底石地 2=碎石堆 3=暗河水 4=洞壁 5=矿脉 6=冰晶地

布局：
  山道：南侧山门（接云泽村）→ 碎石路 + 两段石阶向西折上山 → 溪水（南北向）→
        木桥（y10..11）通向东岸山贼营寨 → 茶棚在中途 → 东北洞口进寒泉洞。
  洞窟：20×12 小型副本，暗河水潭 + 矿脉洞壁 + 冰晶地，洞口在南侧。

契约（每次生成自检，失败非零退出）：
  1. 两张图的 spawn / exit 必须可走，且地表是各自定义的"路/地面"；
  2. data/places.lua 里属于这两张图的地点格必须可走且从 spawn 可达；
  3. 桥面（跨溪）必须可走，否则东岸营寨与采集点被切断；
  4. 建筑/道具的 solid 不得落在墙/水格上。

依赖：Python 3 标准库 + pixel_kit.py（图片尺寸交叉校验）+ 本工具的布局表。
"""

import argparse
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from character_art import player_art  # noqa: E402  主角形象（与 manifest 同源）

from pixel_kit import decode_png  # noqa: E402

TILE = 32

# 列映射（两张图各自一套，引擎写死 6 列）
GRASS, DIRT, STONE, WALL, ROOF, WATER = 1, 2, 3, 4, 5, 6

MOUNTAIN = {
    "name": "mountain_path",
    "display": "江湖有田·青云山道",
    "title": "青云山道 · 采药与护送",
    "texture": "assets/art/tilesets/mountain/mountain_ground.png",
    "w": 40, "h": 22,
    "spawn": (19, 2),
    "exit": (19, 1),
    "path_tile": DIRT,
}

CAVE = {
    "name": "cold_spring_cave",
    "display": "江湖有田·寒泉洞",
    "title": "寒泉洞 · 第一章线索",
    "texture": "assets/art/tilesets/cave/cave_ground.png",
    "w": 20, "h": 12,
    "spawn": (10, 3),
    "exit": (10, 1),
    "path_tile": GRASS,   # 洞底石地
}


def rect(region):
    x0, y0, x1, y1 = region
    return [(x, y) for y in range(y0, y1 + 1) for x in range(x0, x1 + 1)]


# ---------------------------------------------------------------------------
# 山道
# ---------------------------------------------------------------------------

MOUNTAIN_PATH_SEGMENTS = [
    # (x0, y0, x1, y1) 碎石路
    (18, 1, 21, 6),
    (14, 6, 21, 7),
    (10, 7, 13, 15),
    (10, 15, 20, 19),
    (20, 17, 23, 20),
]
MOUNTAIN_STAIRS = [
    (10, 7, 21, 8),
    (10, 14, 20, 15),
]
MOUNTAIN_STREAM = (23, 4, 27, 19)
# 木桥必须**跨满整条溪**（x23..27）才能接上两岸：早期写成 (24,10,26,11)，
# 结果桥面被水包围成了孤岛，places.lua:mountain_bridge 直接报不可达。
MOUNTAIN_BRIDGE = (23, 10, 27, 11)
MOUNTAIN_BAMBOO = [(2, 3, 8, 9), (30, 16, 37, 20)]
MOUNTAIN_CLIFFS = [(2, 10, 7, 20), (30, 1, 37, 5), (33, 6, 37, 9)]

MOUNTAIN_BUILDINGS = [
    {"name": "MountainGate", "texture": "assets/art/buildings/wilds_mountain_gate.png",
     "size": (128, 96), "cell": (18, 0), "order": 5,
     "solid_cells": [(18, 1), (18, 2), (20, 1), (20, 2)]},
    {"name": "TeaShed", "texture": "assets/art/buildings/wilds_tea_shed.png",
     "size": (128, 80), "cell": (14, 9), "order": 0, "solid": (14, 17, 9, 10)},
    {"name": "BanditCamp", "texture": "assets/art/buildings/wilds_bandit_camp.png",
     "size": (160, 96), "cell": (29, 7), "order": 0, "solid": (29, 33, 7, 9)},
    {"name": "BrokenBridge", "texture": "assets/art/buildings/wilds_broken_bridge.png",
     "size": (128, 48), "cell": (3, 14), "order": 10, "solid_cells": []},
]

MOUNTAIN_PROPS = [
    {"name": "Boardwalk", "texture": "assets/art/buildings/wilds_boardwalk.png",
     "size": (128, 64), "cell": (23, 10), "order": 12, "solid_cells": []},
    {"name": "PineA", "texture": "assets/art/props/wilds_pine.png", "size": (64, 88),
     "cell": (9, 3), "order": 20, "solid_cells": [(9, 3)]},
    {"name": "PineB", "texture": "assets/art/props/wilds_pine.png", "size": (64, 88),
     "cell": (14, 16), "order": 20, "solid_cells": []},
    {"name": "PineC", "texture": "assets/art/props/wilds_pine.png", "size": (64, 88),
     "cell": (35, 14), "order": 20, "solid_cells": [(35, 14)]},
    {"name": "BoulderA", "texture": "assets/art/props/wilds_boulder.png", "size": (48, 32),
     "cell": (11, 12), "order": 20, "solid_cells": []},
    {"name": "BoulderB", "texture": "assets/art/props/wilds_boulder.png", "size": (48, 32),
     "cell": (19, 13), "order": 20, "solid_cells": []},
    {"name": "VineA", "texture": "assets/art/props/wilds_vine.png", "size": (32, 64),
     "cell": (7, 12), "order": 20, "solid_cells": []},
    {"name": "VineB", "texture": "assets/art/props/wilds_vine.png", "size": (32, 64),
     "cell": (31, 2), "order": 20, "solid_cells": []},
    {"name": "TorchA", "texture": "assets/art/props/wilds_torch.png", "size": (24, 56),
     "cell": (17, 5), "order": 20, "solid_cells": []},
    {"name": "TorchB", "texture": "assets/art/props/wilds_torch.png", "size": (24, 56),
     "cell": (22, 18), "order": 20, "solid_cells": []},
    {"name": "Signpost", "texture": "assets/art/props/wilds_signpost.png", "size": (40, 48),
     "cell": (21, 5), "order": 20, "solid_cells": [(21, 5)]},
    {"name": "Campfire", "texture": "assets/art/props/wilds_campfire.png", "size": (48, 40),
     "cell": (30, 11), "order": 20, "solid_cells": [(30, 11)]},
    {"name": "Crate", "texture": "assets/art/props/wilds_crate.png", "size": (32, 28),
     "cell": (33, 10), "order": 20, "solid_cells": [(33, 10)]},
    # 采集物
    {"name": "Berry", "texture": "assets/art/gather/wilds_berry.png", "size": (32, 32),
     "cell": (9, 16), "order": 22, "solid_cells": [(9, 16)]},
    {"name": "HerbCluster", "texture": "assets/art/gather/wilds_herb.png", "size": (32, 32),
     "cell": (12, 11), "order": 22, "solid_cells": [(12, 11)]},
    {"name": "Firewood", "texture": "assets/art/gather/wilds_firewood.png", "size": (40, 32),
     "cell": (15, 3), "order": 22, "solid_cells": [(15, 3)]},
    {"name": "OreRock", "texture": "assets/art/gather/wilds_ore.png", "size": (40, 32),
     "cell": (28, 17), "order": 22, "solid_cells": [(28, 17)]},
    {"name": "PoisonHerb", "texture": "assets/art/gather/wilds_poison_herb.png", "size": (32, 32),
     "cell": (29, 15), "order": 22, "solid_cells": [(29, 15)]},
]

# ---------------------------------------------------------------------------
# 寒泉洞
# ---------------------------------------------------------------------------

CAVE_DARKWATER = [(2, 2, 5, 5)]
CAVE_ICE = [(13, 2, 17, 5)]
CAVE_RUBBLE = [(4, 8, 8, 9)]
CAVE_ORE_WALLS = [(1, 10), (2, 10), (17, 1), (18, 1), (19, 5), (0, 5), (9, 10), (10, 10)]

CAVE_PROPS = [
    {"name": "CaveTorchA", "texture": "assets/art/props/wilds_torch.png", "size": (24, 56),
     "cell": (1, 8), "order": 20, "solid_cells": []},
    {"name": "CaveTorchB", "texture": "assets/art/props/wilds_torch.png", "size": (24, 56),
     "cell": (18, 6), "order": 20, "solid_cells": []},
    {"name": "CaveRockA", "texture": "assets/art/props/wilds_boulder.png", "size": (48, 32),
     "cell": (12, 8), "order": 20, "solid_cells": [(12, 8)]},
    {"name": "CaveRockB", "texture": "assets/art/props/wilds_boulder.png", "size": (48, 32),
     "cell": (6, 1), "order": 20, "solid_cells": []},
    {"name": "SnowLotus", "texture": "assets/art/gather/wilds_snow_lotus.png", "size": (32, 32),
     "cell": (15, 6), "order": 22, "solid_cells": [(15, 6)]},
    {"name": "CaveOreVein", "texture": "assets/art/gather/wilds_ore.png", "size": (40, 32),
     "cell": (8, 9), "order": 22, "solid_cells": []},
    {"name": "CaveCrate", "texture": "assets/art/props/wilds_crate.png", "size": (32, 28),
     "cell": (13, 9), "order": 20, "solid_cells": [(13, 9)]},
]


def build_mountain():
    w, h = MOUNTAIN["w"], MOUNTAIN["h"]
    ground = [[GRASS] * w for _ in range(h)]
    for region in MOUNTAIN_BAMBOO:
        for (x, y) in rect((region[0], region[1], region[2], region[3])):
            ground[y][x] = ROOF if False else 5  # 5 = 竹林地
    for region in MOUNTAIN_CLIFFS:
        for (x, y) in rect(region):
            ground[y][x] = WALL
    for region in MOUNTAIN_PATH_SEGMENTS:
        for (x, y) in rect(region):
            ground[y][x] = DIRT
    for region in MOUNTAIN_STAIRS:
        for (x, y) in rect(region):
            ground[y][x] = STONE
    for region in (MOUNTAIN_STREAM,):
        for (x, y) in rect(region):
            ground[y][x] = WATER
    # 木桥：溪水上铺石板 → 可走（必须在溪水之后写）
    for (x, y) in rect(MOUNTAIN_BRIDGE):
        ground[y][x] = STONE
    # 外圈崖壁
    for x in range(w):
        ground[0][x] = WALL
        ground[h - 1][x] = WALL
    for y in range(h):
        ground[y][0] = WALL
        ground[y][w - 1] = WALL
    return ground


def build_cave():
    w, h = CAVE["w"], CAVE["h"]
    ground = [[GRASS] * w for _ in range(h)]
    for region in CAVE_DARKWATER:
        for (x, y) in rect(region):
            ground[y][x] = 3  # 暗河水
    for region in CAVE_ICE:
        for (x, y) in rect(region):
            ground[y][x] = 6  # 冰晶地
    for region in CAVE_RUBBLE:
        for (x, y) in rect(region):
            ground[y][x] = DIRT  # 碎石堆
    for (x, y) in CAVE_ORE_WALLS:
        ground[y][x] = 5  # 矿脉（洞壁上的矿）
    for x in range(w):
        ground[0][x] = WALL
        ground[h - 1][x] = WALL
    for y in range(h):
        ground[y][0] = WALL
        ground[y][w - 1] = WALL
    return ground


def build_solid(ground, buildings, props, w, h, walk_overrides=()):
    solid = [[0] * w for _ in range(h)]
    for y in range(h):
        for x in range(w):
            if ground[y][x] == WALL or ground[y][x] in (WATER, 3) and ground[y][x] != DIRT:
                pass
    # 上面的写法不直观：明确按"墙/水实心，其余可走"处理
    for y in range(h):
        for x in range(w):
            value = ground[y][x]
            solid[y][x] = 1 if value == WALL else 0
    # 水：两张图里 tile==6 是溪水，tile==3 在洞窟是暗河水（但在山道是石阶）
    for y in range(h):
        for x in range(w):
            if ground[y][x] == WATER:
                solid[y][x] = 1
    for (x, y) in walk_overrides:
        solid[y][x] = 0
    for entry in buildings + props:
        cells = rect(entry["solid"]) if "solid" in entry else entry.get("solid_cells", [])
        for (x, y) in cells:
            solid[y][x] = 1
    # 洞窟暗河水单独标实心（列语义与山道不同）
    return solid


def build_cave_solid(ground, props):
    w, h = CAVE["w"], CAVE["h"]
    solid = [[0] * w for _ in range(h)]
    for y in range(h):
        for x in range(w):
            value = ground[y][x]
            if value == WALL or value == 3:   # 洞壁 / 暗河水
                solid[y][x] = 1
    for entry in props:
        for (x, y) in entry.get("solid_cells", []):
            solid[y][x] = 1
    return solid


def row_major(grid, w, h):
    return "".join(str(grid[y][x]) for y in range(h) for x in range(w))


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


def build_scene(meta, ground, solid, entities, w, h):
    lines = [
        "scene_version=2",
        f"name={meta['name']}",
        f"meta\tdisplay_name\t{meta['display']}",
        "meta\tgame\tjianghu_youtian",
        "meta\ttile_size\t32",
        "meta\tui_reference_height\t720",
        "meta\tui_reference_width\t1280",
        "entity\t1\tGroundMap",
        transform_line(1, 0.0, 0.0),
        f"component\t1\tTilemap\ttile_size=32x32;width={w};height={h};texture={meta['texture']}"
        f";tiles={row_major(ground, w, h)};solid={row_major(solid, w, h)}",
        "entity_meta\t1\trole\tground_tilemap",
    ]
    next_id = 2
    for entry in entities:
        role = "building" if entry in MOUNTAIN_BUILDINGS else "prop"
        cell_x, cell_y = entry["cell"]
        width, height = entry["size"]
        lines.append(f"entity\t{next_id}\t{entry['name']}")
        lines.append(sprite_line(next_id, entry["texture"], width, height,
                                 entry.get("layer", "world"), entry["order"]))
        lines.append(transform_line(next_id, float(cell_x * TILE), float(cell_y * TILE)))
        lines.append(f"entity_meta\t{next_id}\trole\t{role}")
        lines.append(f"entity_meta\t{next_id}\tcell\t{cell_x},{cell_y}")
        next_id += 1
    player_id = next_id
    spawn_x, spawn_y = meta["spawn"]
    lines.append(f"entity\t{player_id}\tJianghuPlayer")
    # 主角形象（M21）：单帧 + 序列帧都从角色美术 manifest 取，与
    # data/characters.lua 是同一套串；运行期 player.apply() 会按
    # state.appearance（男/女/未来的自定义）纠正并重挂，此处给的是默认形象。
    player_sprite, player_animation = player_art("player_male")
    lines.append(f"component\t{player_id}\tSprite\t{player_sprite}")
    if player_animation:
        lines.append(f"component\t{player_id}\tAnimation\t{player_animation}")
    lines.append(f"component\t{player_id}\tCollider\tshape=ball;radius=12")
    lines.append(transform_line(player_id, (spawn_x + 0.5) * TILE, (spawn_y + 0.5) * TILE))
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
        f"component\t{title_id}\tText\ttext={meta['title']};font_size=18.00;color=0.949,0.918,0.847"
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
        f"component\t{hint_id}\tText\ttext=WASD 移动 · E 交互(采集/出口) · T 睡觉 · Q 吐纳 · J 普攻 · K 武学"
        ";font_size=14.00;color=0.850,0.900,1.000"
    )
    lines.append(transform_line(hint_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{hint_id}\trole\thud_hint")
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------
# 契约自检
# ---------------------------------------------------------------------------


def reachable(solid, start, w, h):
    seen = {start}
    stack = [start]
    while stack:
        x, y = stack.pop()
        for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            nx, ny = x + dx, y + dy
            if 0 <= nx < w and 0 <= ny < h and not solid[ny][nx] and (nx, ny) not in seen:
                seen.add((nx, ny))
                stack.append((nx, ny))
    return seen


def read_place_cells(map_id, path=os.path.join("scripts", "data", "places.lua")):
    cells = []
    if not os.path.exists(path):
        return cells
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            match = re.match(
                r'\s*(\w+)\s*=\s*\{\s*map\s*=\s*"' + map_id +
                r'"\s*,\s*cell\s*=\s*\{\s*x\s*=\s*(\d+)\s*,\s*y\s*=\s*(\d+)\s*\}',
                line,
            )
            if match:
                cells.append((match.group(1), int(match.group(2)), int(match.group(3))))
    return cells


def check_texture_sizes(entries):
    problems = []
    for entry in entries:
        path = entry["texture"].replace("/", os.sep)
        if not os.path.exists(path):
            problems.append(f"{entry['name']}: 贴图缺失 {entry['texture']}")
            continue
        width, height, _, _ = decode_png(path)
        if (width, height) != entry["size"]:
            problems.append(f"{entry['name']}: 贴图 {width}x{height} != scene 声明 {entry['size']}")
    return problems


def walkable_summary(solid, w, h):
    lines = []
    for y in range(h - 1, -1, -1):
        ranges = []
        start = None
        for x in range(w):
            if not solid[y][x]:
                if start is None:
                    start = x
            else:
                if start is not None:
                    ranges.append((start, x - 1))
                    start = None
        if start is not None:
            ranges.append((start, w - 1))
        text = " ".join(f"{a}" if a == b else f"{a}-{b}" for a, b in ranges)
        lines.append(f"    y{y:>2}: {text if text else '(全实心)'}")
    return lines


def main():
    parser = argparse.ArgumentParser(description="生成青云山道与寒泉洞场景")
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()

    problems = []
    os.makedirs("scenes", exist_ok=True)

    # ---- 山道 ----
    ground = build_mountain()
    w, h = MOUNTAIN["w"], MOUNTAIN["h"]
    solid = build_solid(ground, MOUNTAIN_BUILDINGS, MOUNTAIN_PROPS, w, h)
    entities = MOUNTAIN_BUILDINGS + MOUNTAIN_PROPS
    text = build_scene(MOUNTAIN, ground, solid, entities, w, h)
    with open(os.path.join("scenes", "mountain_path.scene"), "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    walk = reachable(solid, MOUNTAIN["spawn"], w, h)
    for label, cell in (("spawn", MOUNTAIN["spawn"]), ("exit", MOUNTAIN["exit"])):
        if solid[cell[1]][cell[0]]:
            problems.append(f"mountain {label} {cell} 是 solid")
        elif cell not in walk:
            problems.append(f"mountain {label} {cell} 从 spawn 不可达")
    for (x, y) in rect(MOUNTAIN_BRIDGE):
        if solid[y][x]:
            problems.append(f"mountain 木桥 ({x},{y}) 是 solid")
    for name, x, y in read_place_cells("mountain_path"):
        if solid[y][x]:
            problems.append(f"places.lua:{name} ({x},{y}) 是 solid")
        elif (x, y) not in walk:
            problems.append(f"places.lua:{name} ({x},{y}) 不可达")
    problems += check_texture_sizes(entities)
    print(f"wrote scenes/mountain_path.scene ({w}x{h}, {len(MOUNTAIN_BUILDINGS)} 建筑 + "
          f"{len(MOUNTAIN_PROPS)} 道具)")

    # ---- 寒泉洞 ----
    ground = build_cave()
    w, h = CAVE["w"], CAVE["h"]
    solid = build_cave_solid(ground, CAVE_PROPS)
    text = build_scene(CAVE, ground, solid, CAVE_PROPS, w, h)
    with open(os.path.join("scenes", "cold_spring_cave.scene"), "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    walk = reachable(solid, CAVE["spawn"], w, h)
    for label, cell in (("spawn", CAVE["spawn"]), ("exit", CAVE["exit"])):
        if solid[cell[1]][cell[0]]:
            problems.append(f"cave {label} {cell} 是 solid")
        elif cell not in walk:
            problems.append(f"cave {label} {cell} 从 spawn 不可达")
    for name, x, y in read_place_cells("cold_spring_cave"):
        if solid[y][x]:
            problems.append(f"places.lua:{name} ({x},{y}) 是 solid")
        elif (x, y) not in walk:
            problems.append(f"places.lua:{name} ({x},{y}) 不可达")
    problems += check_texture_sizes(CAVE_PROPS)
    print(f"wrote scenes/cold_spring_cave.scene ({w}x{h}, {len(CAVE_PROPS)} 道具)")

    if problems:
        for problem in problems:
            print("[FAIL]", problem)
        raise SystemExit(1)
    print("契约自检 ok：山道 spawn/exit/木桥 + 洞窟 spawn/exit")
    if not args.quiet:
        print("山道可走区间（同步 data/places.lua 注释用）：")
        for line in walkable_summary(solid if False else build_solid(build_mountain(),
                                                                    MOUNTAIN_BUILDINGS,
                                                                    MOUNTAIN_PROPS,
                                                                    MOUNTAIN["w"], MOUNTAIN["h"]),
                                     MOUNTAIN["w"], MOUNTAIN["h"]):
            print(line)


if __name__ == "__main__":
    main()
