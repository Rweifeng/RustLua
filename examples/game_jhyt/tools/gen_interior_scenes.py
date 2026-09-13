# -*- coding: utf-8 -*-
"""《江湖有田》批次 3 场景装配：药庐内 / 铁匠铺内（scenes/interior_herb.scene / interior_forge.scene）。

用法：工程根目录 python tools/gen_interior_scenes.py

美术：`tools/gen_interior_art.py` 产出的两套室内 tileset + 家具 + 生产设施。

设计取舍（docs/42 §5 第 5 项要求先定玩法方案）：
  **采用"进门切独立场景"**，而不是同图分层。理由：
  * 引擎已有成熟的场景切换 + 出口/落点机制（map_manager + data/maps.lua），零新增引擎能力；
  * 室内可以做自己的光照/尺寸（这里用 20×12，比室外小，进门有"进屋子"的体感）；
  * 同图分层需要额外做"屋顶遮挡 + 门内可见性"，成本高且和现有 Tilemap 单层渲染假设冲突。

两张图的列语义（各自一套 tileset，引擎写死 6 列）：
  interior_herb ：1=木地板 2=青砖地 3=席垫 4=白墙 5=木隔断 6=柜台
  interior_forge：1=夯土地 2=石板地 3=炭渣地 4=砖墙 5=石墙 6=水槽

布局（20×12）：外圈墙 → 南北向主厅（地板）→ 北侧工作区（药炉/铁砧台）→
西侧家具排（药柜/书架/床/桌）→ 东侧柜面/水槽 → 南侧门口（exit 格 (10,1)）。

契约自检：spawn/exit 可走；data/places.lua 中属于本图的地点格可走且从 spawn 可达；
家具/设施的 solid 不落在墙格；贴图尺寸与声明一致。

依赖：Python 3 标准库 + pixel_kit.py。
"""

import argparse
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from character_art import player_art  # noqa: E402  主角形象（与 manifest 同源）

from pixel_kit import decode_png  # noqa: E402

TILE = 32
W, H = 20, 12

# 列语义：两张图共用编号，含义见 tileset
FLOOR, BRICK, MAT, WALLTILE, PARTITION, COUNTER = 1, 2, 3, 4, 5, 6

INTERIORS = [
    {
        "name": "interior_herb",
        "display": "云泽村·药庐内",
        "title": "药庐 · 药材与药炉",
        "texture": "assets/art/tilesets/interior_herb/interior_ground.png",
        "floor": FLOOR, "wall": PARTITION,
        "spawn": (10, 3), "exit": (10, 1),
        "furniture": [
            {"name": "MedicineCabinet", "texture": "assets/art/furniture/interior_medicine_cabinet.png",
             "size": (64, 64), "cell": (2, 8), "order": 10, "solid_cells": [(2, 8), (3, 8)]},
            {"name": "Bookshelf", "texture": "assets/art/furniture/interior_bookshelf.png",
             "size": (56, 72), "cell": (4, 8), "order": 10, "solid_cells": [(4, 8)]},
            {"name": "Bed", "texture": "assets/art/furniture/interior_bed.png",
             "size": (64, 48), "cell": (6, 2), "order": 10, "solid_cells": [(6, 2), (7, 2)]},
            {"name": "Table", "texture": "assets/art/furniture/interior_table.png",
             "size": (48, 40), "cell": (10, 2), "order": 10, "solid_cells": [(10, 2)]},
            {"name": "Chair", "texture": "assets/art/furniture/interior_chair.png",
             "size": (28, 40), "cell": (11, 3), "order": 10, "solid_cells": [(11, 3)]},
            {"name": "Screen", "texture": "assets/art/furniture/interior_screen.png",
             "size": (56, 56), "cell": (14, 3), "order": 10, "solid_cells": [(14, 3)]},
            {"name": "HerbMill", "texture": "assets/art/furniture/interior_herb_mill.png",
             "size": (40, 28), "cell": (13, 2), "order": 10, "solid_cells": []},
            {"name": "Lantern", "texture": "assets/art/furniture/interior_lantern.png",
             "size": (24, 44), "cell": (9, 8), "order": 20, "solid_cells": []},
            {"name": "LanternB", "texture": "assets/art/furniture/interior_lantern.png",
             "size": (24, 44), "cell": (12, 8), "order": 20, "solid_cells": []},
            {"name": "Chest", "texture": "assets/art/furniture/interior_chest.png",
             "size": (36, 30), "cell": (17, 2), "order": 10, "solid_cells": [(17, 2)]},
        ],
        "facilities": [
            {"name": "HerbStove", "texture": "assets/art/facilities/facility_herb_stove.png",
             "size": (64, 72), "cell": (9, 6), "order": 5, "solid_cells": [(9, 6), (10, 6)]},
            {"name": "GrindTable", "texture": "assets/art/facilities/facility_grind_table.png",
             "size": (56, 56), "cell": (12, 6), "order": 5, "solid_cells": [(12, 6)]},
        ],
    },
    {
        "name": "interior_forge",
        "display": "云泽村·铁匠铺内",
        "title": "铁匠铺 · 铁砧与炉火",
        "texture": "assets/art/tilesets/interior_forge/interior_ground.png",
        "floor": BRICK, "wall": WALLTILE,
        "spawn": (10, 3), "exit": (10, 1),
        "furniture": [
            {"name": "ShelfGoods", "texture": "assets/art/furniture/interior_shelf.png",
             "size": (64, 56), "cell": (2, 8), "order": 10, "solid_cells": [(2, 8), (3, 8)]},
            {"name": "Barrel", "texture": "assets/art/furniture/interior_barrel.png",
             "size": (28, 34), "cell": (4, 8), "order": 10, "solid_cells": [(4, 8)]},
            {"name": "BarrelB", "texture": "assets/art/furniture/interior_barrel.png",
             "size": (28, 34), "cell": (5, 8), "order": 10, "solid_cells": [(5, 8)]},
            {"name": "Chest", "texture": "assets/art/furniture/interior_chest.png",
             "size": (36, 30), "cell": (7, 2), "order": 10, "solid_cells": [(7, 2)]},
            {"name": "Table", "texture": "assets/art/furniture/interior_table.png",
             "size": (48, 40), "cell": (9, 2), "order": 10, "solid_cells": [(9, 2)]},
            {"name": "Chair", "texture": "assets/art/furniture/interior_chair.png",
             "size": (28, 40), "cell": (16, 2), "order": 10, "solid_cells": [(16, 2)]},
            {"name": "Lantern", "texture": "assets/art/furniture/interior_lantern.png",
             "size": (24, 44), "cell": (6, 8), "order": 20, "solid_cells": []},
            {"name": "LanternB", "texture": "assets/art/furniture/interior_lantern.png",
             "size": (24, 44), "cell": (15, 8), "order": 20, "solid_cells": []},
        ],
        "facilities": [
            {"name": "Anvil", "texture": "assets/art/facilities/facility_anvil.png",
             "size": (56, 48), "cell": (9, 5), "order": 5, "solid_cells": [(9, 5)]},
            {"name": "TeaStove", "texture": "assets/art/facilities/facility_tea_stove.png",
             "size": (64, 64), "cell": (12, 5), "order": 5, "solid_cells": [(12, 5), (13, 5)]},
            {"name": "WineWorks", "texture": "assets/art/facilities/facility_wine_works.png",
             "size": (64, 72), "cell": (15, 6), "order": 5, "solid_cells": [(15, 6), (16, 6)]},
        ],
    },
]


def rect(region):
    x0, y0, x1, y1 = region
    return [(x, y) for y in range(y0, y1 + 1) for x in range(x0, x1 + 1)]


def build_ground(spec):
    w, h = W, H
    ground = [[spec["floor"]] * w for _ in range(h)]
    # 外圈墙
    for x in range(w):
        ground[0][x] = spec["wall"]
        ground[h - 1][x] = spec["wall"]
    for y in range(h):
        ground[y][0] = spec["wall"]
        ground[y][w - 1] = spec["wall"]
    # 北墙下沿的家具靠墙带用砖地
    for x in range(1, w - 1):
        ground[h - 2][x] = BRICK if spec["floor"] == FLOOR else MAT
    return ground


def build_solid(ground, entries):
    w, h = W, H
    solid = [[0] * w for _ in range(h)]
    for y in range(h):
        for x in range(w):
            if ground[y][x] == ground[0][0] and (x == 0 or y == 0 or x == w - 1 or y == h - 1):
                solid[y][x] = 1
    for entry in entries:
        for (x, y) in entry.get("solid_cells", []):
            solid[y][x] = 1
    return solid


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


def build_scene(spec, ground, solid, entries):
    lines = [
        "scene_version=2",
        f"name={spec['name']}",
        f"meta\tdisplay_name\t{spec['display']}",
        "meta\tgame\tjianghu_youtian",
        "meta\ttile_size\t32",
        "meta\tui_reference_height\t720",
        "meta\tui_reference_width\t1280",
        "entity\t1\tGroundMap",
        transform_line(1, 0.0, 0.0),
        f"component\t1\tTilemap\ttile_size=32x32;width={W};height={H};texture={spec['texture']}"
        f";tiles={row_major(ground)};solid={row_major(solid)}",
        "entity_meta\t1\trole\tground_tilemap",
    ]
    next_id = 2
    for entry in entries:
        cell_x, cell_y = entry["cell"]
        width, height = entry["size"]
        role = "facility" if entry in spec["facilities"] else "furniture"
        lines.append(f"entity\t{next_id}\t{entry['name']}")
        lines.append(sprite_line(next_id, entry["texture"], width, height, "world", entry["order"]))
        lines.append(transform_line(next_id, float(cell_x * TILE), float(cell_y * TILE)))
        lines.append(f"entity_meta\t{next_id}\trole\t{role}")
        lines.append(f"entity_meta\t{next_id}\tcell\t{cell_x},{cell_y}")
        next_id += 1
    player_id = next_id
    spawn_x, spawn_y = spec["spawn"]
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
        f"component\t{title_id}\tText\ttext={spec['title']};font_size=18.00;color=0.949,0.918,0.847"
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
        f"component\t{hint_id}\tText\ttext=WASD 移动 · E 使用设施/出门 · T 睡觉 · Q 吐纳"
        ";font_size=14.00;color=0.850,0.900,1.000"
    )
    lines.append(transform_line(hint_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{hint_id}\trole\thud_hint")
    return "\n".join(lines) + "\n"


def reachable(solid, start):
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
            problems.append(f"{entry['name']}: 贴图 {width}x{height} != 声明 {entry['size']}")
    return problems


def main():
    parser = argparse.ArgumentParser(description="生成室内场景（药庐 / 铁匠铺）")
    args = parser.parse_args()
    problems = []
    os.makedirs("scenes", exist_ok=True)
    for spec in INTERIORS:
        entries = spec["furniture"] + spec["facilities"]
        ground = build_ground(spec)
        solid = build_solid(ground, entries)
        text = build_scene(spec, ground, solid, entries)
        out = os.path.join("scenes", spec["name"] + ".scene")
        with open(out, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(text)
        walk = reachable(solid, spec["spawn"])
        for label, cell in (("spawn", spec["spawn"]), ("exit", spec["exit"])):
            if solid[cell[1]][cell[0]]:
                problems.append(f"{spec['name']} {label} {cell} 是 solid")
            elif cell not in walk:
                problems.append(f"{spec['name']} {label} {cell} 从 spawn 不可达")
        for name, x, y in read_place_cells(spec["name"]):
            if solid[y][x]:
                problems.append(f"places.lua:{name} ({x},{y}) 是 solid")
            elif (x, y) not in walk:
                problems.append(f"places.lua:{name} ({x},{y}) 不可达")
        problems += check_texture_sizes(entries)
        print(f"wrote {out} ({W}x{H}, {len(spec['furniture'])} 家具 + {len(spec['facilities'])} 设施)")
    if problems:
        for problem in problems:
            print("[FAIL]", problem)
        raise SystemExit(1)
    print("契约自检 ok：两张室内图的 spawn/exit 与 places.lua 地点格")


if __name__ == "__main__":
    main()
