# -*- coding: utf-8 -*-
"""《江湖有田》批次 2 场景装配：寒泉洞（scenes/cold_spring_cave.scene）。

用法：工程根目录 `examples/game_jhyt` 下

    python tools/gen_wilds_scene.py

**本文件现在只出寒泉洞那一张**。青云山道以前也住在这里，M72-11 起搬到
`tools/gen_mountain_scene.py`——那张图要接金标管线（逻辑瓦 autotile、按贴图不透明像素真算
的 solid、拆层遮断、地面投影、确定性装饰散布），而这里的表是"6 列直接寻址 + 每件手填
`solid_cells`"，两套口径塞在一个文件里只会让改山道的人跑错生成器（盘上确实出现过：山道
的 HUD 与采集物是手工调的，重跑本文件把那批改动抹掉、还把 BanditCamp 搬回溪中央）。

寒泉洞本身**还没**升到金标口径：它仍用 6×1 图集直接寻址与手填占地，是 M72-11 剩下那几张
图里的一张（plan §铺量顺序：洞穴要按 §风险 另定每屏对象分档，工具不替它猜数）。

地图与列语义：
  cold_spring_cave 20×12  1=洞底石地 2=碎石堆 3=暗河水 4=洞壁 5=矿脉 6=冰晶地

布局：20×12 小型副本，暗河水潭在西南、冰晶地在东南、碎石堆与矿脉在北边，洞口在南侧。

契约（每次生成自检，失败非零退出）：
  1. spawn / exit 必须可走，且从 spawn 可达；
  2. data/places.lua 里属于本图的地点格必须可走且从 spawn 可达；
  3. 道具的 solid 不得把出口或地点格封死（`solid_cells` 手填，改一件就要重跑自检）；
  4. 每件 sprite 声明的 `size=` 必须与贴图像素一致（不一致只会被引擎悄悄拉伸，不报错）。

依赖：Python 3 标准库 + `pixel_kit.py`（图片尺寸交叉校验、Transform/Camera 行）+
`terrain_kit.py`（Tilemap 字段顺序，与引擎 `to_component_str` 逐字符一致）。
"""

import argparse
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from character_art import player_art  # noqa: E402  主角形象（与 manifest 同源）
import terrain_kit as tk  # noqa: E402  Tilemap 那一行的字段顺序只有一份
from pixel_kit import decode_png  # noqa: E402
# M32：`Transform` 的统一形态序列化器（场景数据门只认这一种写法）。
from pixel_kit import transform_line as spawn_transform  # noqa: E402
from pixel_kit import camera_line  # noqa: E402  场景 2D 相机（取景锁在地图内）

TILE = 32

# 列映射（洞窟一套，图集 6 列 × 1 行、直接寻址）
GRASS, DIRT, WATER, WALL, ORE, ICE = 1, 2, 3, 4, 5, 6

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

# 洞窟地表：**没有** `entity_meta role decor` 那一段，也没有遮断/投影——金标管线还没接它。
# id 段因此是本文件唯一的分配者：1 GroundMap、2..N 道具、然后玩家/标题/提示/相机、最后
# `CAVE_EXTRAS`（盘上手工追加在相机之后的那批，见它的注释）。
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

# 登记在**相机之后**的那批实体：以前是手工 `Edit` 追加到文件尾的（b3132ac4 横屏方案A 那笔），
# 于是重跑本工具就会把它们整段抹掉——而盘上没有任何一张图会报错，只是 `interior_stove.png`
# 从"被引用"变成孤儿。现在按同一段、同一个 id 登记进来，重跑 = 盘上。
# `Stove` 在资源接线清单里的作用就是让家具图集有一处真实引用（docs/48），不是装饰。
CAVE_EXTRAS = [
    {"name": "Stove", "texture": "assets/art/furniture/interior_stove.png", "size": (48, 40),
     "cell": (10, 6), "order": 10, "role": "furniture", "solid_cells": []},
]


def rect(region):
    x0, y0, x1, y1 = region
    return [(x, y) for y in range(y0, y1 + 1) for x in range(x0, x1 + 1)]


def build_cave():
    w, h = CAVE["w"], CAVE["h"]
    ground = [[GRASS] * w for _ in range(h)]
    for region in CAVE_DARKWATER:
        for (x, y) in rect(region):
            ground[y][x] = WATER  # 暗河水
    for region in CAVE_ICE:
        for (x, y) in rect(region):
            ground[y][x] = ICE  # 冰晶地
    for region in CAVE_RUBBLE:
        for (x, y) in rect(region):
            ground[y][x] = DIRT  # 碎石堆
    for (x, y) in CAVE_ORE_WALLS:
        ground[y][x] = ORE  # 矿脉（洞壁上的矿）
    for x in range(w):
        ground[0][x] = WALL
        ground[h - 1][x] = WALL
    for y in range(h):
        ground[y][0] = WALL
        ground[y][w - 1] = WALL
    return ground


def build_cave_solid(ground, props):
    """洞壁与暗河水天然实心，其余按道具手填 `solid_cells`（金标口径见模块 docstring）。"""
    w, h = CAVE["w"], CAVE["h"]
    solid = [[0] * w for _ in range(h)]
    for y in range(h):
        for x in range(w):
            value = ground[y][x]
            if value == WALL or value == WATER:
                solid[y][x] = 1
    for entry in props + CAVE_EXTRAS:
        for (x, y) in entry.get("solid_cells", []):
            solid[y][x] = 1
    return solid


def row_major(grid, w, h):
    return "".join(str(grid[y][x]) for y in range(h) for x in range(w))


def transform_line(entity_id, x, y):
    # M32：统一的 Transform 形态由 `pixel_kit.transform_line` 单点产出
    # （引擎读侧只认 position/rotation/scale，2D 旧形态已删除；见该函数注释）。
    return spawn_transform(entity_id, x, y)


def sprite_line(entity_id, texture, width, height, layer, order):
    return (
        f"component\t{entity_id}\tSprite\ttexture={texture};size={width}x{height};"
        f"pivot=0.000,0.000;tint=1.000,1.000,1.000,1.000;sorting_layer={layer};order={order}"
    )


def entity_lines(entity_id, entry):
    """一件 sprite 的四行（实体名/Sprite/Transform/两条 entity_meta）。

    `CAVE_PROPS` 与 `CAVE_EXTRAS` 走同一个函数：分段只是为了 id 顺序，不是为了两套写法。
    """
    cell_x, cell_y = entry["cell"]
    width, height = entry["size"]
    return [
        f"entity\t{entity_id}\t{entry['name']}",
        sprite_line(entity_id, entry["texture"], width, height,
                    entry.get("layer", "world"), entry["order"]),
        transform_line(entity_id, float(cell_x * TILE), float(cell_y * TILE)),
        f"entity_meta\t{entity_id}\trole\t{entry.get('role', 'prop')}",
        f"entity_meta\t{entity_id}\tcell\t{cell_x},{cell_y}",
    ]


def build_scene(meta, ground, solid, entities, extras, w, h):
    cells = [ground[y][x] for y in range(h) for x in range(w)]
    lines = [
        "scene_version=2",
        f"name={meta['name']}",
        f"meta\tdisplay_name\t{meta['display']}",
        "meta\tgame\tjianghu_youtian",
        "meta\ttile_size\t32",
        # 横屏方案A：设计分辨率 1280×720（与 `scripts/ui/layout.lua` 一致）。
        "meta\tui_reference_height\t720",
        "meta\tui_reference_width\t1280",
        "entity\t1\tGroundMap",
        transform_line(1, 0.0, 0.0),
        # 字段顺序走 `terrain_kit.tilemap_component`（引擎 `Tilemap::to_component_str` 的
        # 同一份顺序）。以前这里自写一条 f-string 用 `tiles=4444…` 的定宽数字串，而盘上早就
        # 是 `tiles_format=1` + 逗号分隔 + `atlas_cols/rows`（e26a0508 那笔重编码），于是
        # "重跑生成器"会把新编码退回旧编码——引擎两种都读得懂，所以不报错，只是下次对拍全是假差异。
        "component\t1\tTilemap\t" + tk.tilemap_component(
            TILE, w, h, meta["texture"], cells, row_major(solid, w, h), 6, 1),
        "entity_meta\t1\trole\tground_tilemap",
    ]
    next_id = 2
    for entry in entities:
        lines.extend(entity_lines(next_id, entry))
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
        "offsets=24.00,20.00,672.00,44.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000"
    )
    lines.append(
        f"component\t{title_id}\tText\ttext={meta['title']};font_size=18.00;color=0.949,0.918,0.847"
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
        f"component\t{hint_id}\tText\ttext=WASD 移动 · E 交互(采集/出口) · T 睡觉 · Q 吐纳 · J 普攻 · K 武学"
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
    lines.append(camera_line(camera_id, w, h))
    lines.append(transform_line(camera_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{camera_id}\trole\tgame_camera")
    next_id = camera_id + 1
    for entry in extras:
        lines.extend(entity_lines(next_id, entry))
        next_id += 1
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
    parser = argparse.ArgumentParser(description="生成寒泉洞场景（山道见 gen_mountain_scene.py）")
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()

    problems = []
    os.makedirs("scenes", exist_ok=True)

    ground = build_cave()
    w, h = CAVE["w"], CAVE["h"]
    solid = build_cave_solid(ground, CAVE_PROPS)
    text = build_scene(CAVE, ground, solid, CAVE_PROPS, CAVE_EXTRAS, w, h)
    with open(os.path.join("scenes", "cold_spring_cave.scene"), "w",
              encoding="utf-8", newline="\n") as handle:
        handle.write(text)
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
    problems += check_texture_sizes(CAVE_PROPS + CAVE_EXTRAS)
    print(f"wrote scenes/cold_spring_cave.scene ({w}x{h}, {len(CAVE_PROPS)} 道具 + "
          f"{len(CAVE_EXTRAS)} 家具)")

    if problems:
        for problem in problems:
            print("[FAIL]", problem)
        raise SystemExit(1)
    print("契约自检 ok：洞窟 spawn/exit/places")
    if not args.quiet:
        print("洞窟可走区间（同步 data/places.lua 注释用）：")
        for line in walkable_summary(solid, w, h):
            print(line)


if __name__ == "__main__":
    main()
