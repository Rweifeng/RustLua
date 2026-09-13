# -*- coding: utf-8 -*-
"""《江湖有田》演武场场景生成（scenes/combat_arena.scene）——山庄练武场版。

用法：工程根目录 python tools/gen_arena_scene.py

美术：地表复用山庄 6 列 tileset（`assets/art/tilesets/manor/manor_ground.png`，
列语义与三场景共用：1=草地 2=夯土 3=石板路 4=院墙 5=青瓦 6=水），
建筑/道具复用山庄与云泽村的通用 sprite（引擎 sprite 不能选 UV 区域，一件一张图）。
改布局改本文件，跑一次即可复现；画面想换再动 gen_manor_art.py。

引擎 Tilemap 约定（scene_version=2，见 engine core/tilemap.rs）：
  tile_size=WxH;width=W;height=H;texture=PATH;tiles=<row-major 0..6>;solid=<row-major 0/1>
  * tile 值 0 = 空，1..6 = tileset 第 N 列（引擎 TILE_PALETTE 写死 6 列）；
  * 单格 32 世界单位，y 朝上，row 0 在最底。

**战斗验收锚点（不可移动）**：headless 验收 `jhyt_m18_combat` 硬编码了
演武场的战斗几何，本工具每次生成都自检它们可走、可达、地表为石板：
  * 出生格 spawn (9,5)   → 世界 (304,176)，战败 R 重开落点（data/maps.lua）
  * 训练木人 home (14,5)，玩家试招站位 (13,5)（面向右普攻/武学/击杀验收）
  * 灰狼 home (2,9)，引敌站位 (6,8)（A* 追击/受击/战败验收，两点间必须可寻路）
改动锚点 = 三处同步：本文件、`data/maps.lua`、`engine/crates/game/examples/jhyt_m18_combat.rs`。

**farm.lua 防误垦**：farm.lua 的 FARM 矩形（x14..33, y4..13）没有场景门禁，
判定"可开垦"只看 GroundMap tile==2（夯土）。演武场全场禁止夯土 tile，
避免玩家在演武坪里按 E 锄地。本工具把"无 tile 2"作为硬性契约自检。

布局分区（cell，0-index，含端点）：
  外围院墙   ：row0 / row21 / col0 / col39（tile 4，solid）
  演武坪     ：x1..19, y1..12 石板，战斗锚点全部在坪内
  南门       ：manor_gate x8..10, y0..2，门洞 (9,1)(9,2) 可走 = 出口格 (9,1)
  器械廊     ：y13 草地一排：兵器架×2 / 草垛 / 演武旗×2，石板引道 x9..10
  东侧小径   ：x20..29, y7..8 石板，接茶水棚与水畔
  池塘       ：x30..36, y14..17（tile 6，solid）+ 木栈桥
  山林带     ：y19..20 老树/竹丛/山石压北墙形成纵深
"""
import os

import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from character_art import player_art  # noqa: E402  主角形象（与 manifest 同源）
import re

# 地图尺寸：与山庄/云泽村一致（40×22 = 1280×704，正好铺满一屏）。
W, H = 40, 22
TILE = 32

# 列映射（写 scene tile 用；三场景共用语义，不可重排）
GRASS, DIRT, STONE, WALL, ROOF, WATER = 1, 2, 3, 4, 5, 6

GROUND_TEXTURE = "assets/art/tilesets/manor/manor_ground.png"

# ---- 分区 ----
YARD = {"x0": 1, "x1": 19, "y0": 1, "y1": 12}        # 演武坪（石板）
SPUR = {"x0": 9, "x1": 10, "y0": 13, "y1": 13}       # 器械廊引道（石板）
EAST_PATH = {"x0": 20, "x1": 29, "y0": 7, "y1": 8}   # 东侧小径（石板）
POND = {"x0": 30, "x1": 36, "y0": 14, "y1": 17}      # 池塘（水，solid）

# ---- 战斗验收锚点（与 jhyt_m18_combat.rs / data/maps.lua 三方一致）----
SPAWN = {"x": 9, "y": 5}            # 出生/战败重开落点 (304,176)
EXIT = {"x": 9, "y": 1}             # 出口格：南门门洞内
GATE_DOOR = [(9, 1), (9, 2)]        # 门洞两格必须可走
GATE_PILLARS = [(8, 1), (8, 2), (10, 1), (10, 2)]  # 门楼立柱 solid
ANCHORS = {
    "spawn": (SPAWN["x"], SPAWN["y"]),
    "exit": (EXIT["x"], EXIT["y"]),
    "dummy_home": (14, 5),          # 训练木人 home（data/enemies.lua）
    "player_before_dummy": (13, 5), # 试招站位（jhyt_m18_combat PLAYER_BEFORE_DUMMY）
    "wolf_home": (2, 9),            # 灰狼 home（data/enemies.lua）
    "wolf_lure": (6, 8),            # 引敌站位（jhyt_m18_combat WOLF_LURE）
}

# ---- 建筑 ----
BUILDINGS = [
    # 南门（复用山庄门楼）：只挡两根立柱，门洞 (9,1)(9,2) 保持可走（出口格 + 门内格）
    {"name": "ArenaGate", "texture": "assets/art/buildings/manor_gate.png",
     "size": (96, 96), "cell": (8, 0), "order": 5,
     "solid_cells": GATE_PILLARS},
]

# ---- 道具（cell = 贴图左下角；solid_cells = 需要标 1 的格）----
PROPS = [
    # 南门内侧灯杆一对
    {"name": "ArenaLanternA", "texture": "assets/art/props/village_lantern.png",
     "size": (24, 64), "cell": (6, 1), "order": 10, "solid_cells": [(6, 1)]},
    {"name": "ArenaLanternB", "texture": "assets/art/props/village_lantern.png",
     "size": (24, 64), "cell": (12, 1), "order": 10, "solid_cells": [(12, 1)]},
    # 东侧小径口的告示木牌
    {"name": "ArenaSign", "texture": "assets/art/props/manor_sign.png",
     "size": (32, 42), "cell": (22, 9), "order": 10, "solid_cells": [(22, 9)]},
    # 器械廊（演武坪北侧 y13）：兵器架×2 + 草垛 + 演武旗×2
    {"name": "ArenaRackA", "texture": "assets/art/props/manor_rack.png",
     "size": (64, 46), "cell": (4, 13), "order": 10,
     "solid_cells": [(4, 13), (5, 13)]},
    {"name": "ArenaRackB", "texture": "assets/art/props/manor_rack.png",
     "size": (64, 46), "cell": (7, 13), "order": 10,
     "solid_cells": [(7, 13), (8, 13)]},
    {"name": "ArenaHaystackA", "texture": "assets/art/props/manor_haystack.png",
     "size": (36, 34), "cell": (12, 13), "order": 10, "solid_cells": [(12, 13)]},
    {"name": "ArenaBannerA", "texture": "assets/art/props/village_banner.png",
     "size": (32, 56), "cell": (15, 13), "order": 10, "solid_cells": [(15, 13)]},
    {"name": "ArenaBannerB", "texture": "assets/art/props/village_banner.png",
     "size": (32, 56), "cell": (18, 13), "order": 10, "solid_cells": [(18, 13)]},
    # 演武坪北缘旗杆一对（西北/东北角；南缘靠院墙不再立旗）
    {"name": "ArenaBannerC", "texture": "assets/art/props/village_banner.png",
     "size": (32, 56), "cell": (19, 12), "order": 10, "solid_cells": [(19, 12)]},
    {"name": "ArenaBannerD", "texture": "assets/art/props/village_banner.png",
     "size": (32, 56), "cell": (1, 12), "order": 10, "solid_cells": [(1, 12)]},
    # 石锁阵：坪东侧两只 + 西南一只 + 水畔一只（山石图充当石锁/障碍）
    {"name": "ArenaRockA", "texture": "assets/art/props/village_rock.png",
     "size": (64, 40), "cell": (17, 9), "order": 10, "solid_cells": [(17, 9)]},
    {"name": "ArenaRockB", "texture": "assets/art/props/village_rock.png",
     "size": (64, 40), "cell": (17, 6), "order": 10, "solid_cells": [(17, 6)]},
    {"name": "ArenaRockC", "texture": "assets/art/props/village_rock.png",
     "size": (64, 40), "cell": (3, 3), "order": 10, "solid_cells": [(3, 3)]},
    {"name": "ArenaRockD", "texture": "assets/art/props/village_rock.png",
     "size": (64, 40), "cell": (35, 13), "order": 10, "solid_cells": [(35, 13)]},
    # 东侧小径南缘：茶水棚 + 水缸 + 草垛（歇脚处）
    {"name": "ArenaStall", "texture": "assets/art/props/village_stall.png",
     "size": (64, 40), "cell": (26, 10), "order": 10,
     "solid_cells": [(26, 10), (27, 10)]},
    {"name": "ArenaJar", "texture": "assets/art/props/manor_jar.png",
     "size": (24, 30), "cell": (28, 10), "cell_dx": 4, "order": 10,
     "solid_cells": [(28, 10)]},
    {"name": "ArenaHaystackB", "texture": "assets/art/props/manor_haystack.png",
     "size": (36, 34), "cell": (24, 11), "order": 10, "solid_cells": [(24, 11)]},
    # 池塘木栈桥（贴水面，水本身 solid，不再加 solid）
    {"name": "ArenaPier", "texture": "assets/art/props/village_pier.png",
     "size": (96, 32), "cell": (31, 14), "order": 8, "solid_cells": []},
    # 山林带（y19 一排压北院墙，形成纵深；不放 y20 以免超出地图上沿被裁）
    {"name": "ArenaTreeA", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (2, 19), "order": 20, "solid_cells": [(2, 19)]},
    {"name": "ArenaBambooA", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (5, 19), "order": 20, "solid_cells": [(5, 19)]},
    {"name": "ArenaBambooB", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (11, 19), "order": 20, "solid_cells": [(11, 19)]},
    {"name": "ArenaTreeB", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (15, 19), "order": 20, "solid_cells": [(15, 19)]},
    {"name": "ArenaRockN", "texture": "assets/art/props/village_rock.png",
     "size": (64, 40), "cell": (8, 20), "order": 20, "solid_cells": [(8, 20)]},
    {"name": "ArenaBambooC", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (25, 19), "order": 20, "solid_cells": [(25, 19)]},
    {"name": "ArenaTreeC", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (28, 19), "order": 20, "solid_cells": [(28, 19)]},
    {"name": "ArenaTreeD", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (37, 19), "order": 20, "solid_cells": [(37, 19)]},
]

HUD_TITLE = "山庄 · 演武场"
HUD_HINT = "WASD 移动 · E 交互(出口) · J 普攻 · K 武学 · R 战败重开"


def rect_cells(bounds):
    x0, x1, y0, y1 = bounds["x0"], bounds["x1"], bounds["y0"], bounds["y1"]
    return [(x, y) for y in range(y0, y1 + 1) for x in range(x0, x1 + 1)]


def build_tiles():
    """地表 tile 值矩阵 tiles[y][x]（演武场全场无夯土，防 farm.lua 误垦）。"""
    ground = [[GRASS] * W for _ in range(H)]
    for region in (YARD, SPUR, EAST_PATH):
        for (x, y) in rect_cells(region):
            ground[y][x] = STONE
    for (x, y) in rect_cells(POND):
        ground[y][x] = WATER
    # 外圈院墙
    for x in range(W):
        ground[0][x] = WALL
        ground[H - 1][x] = WALL
    for y in range(H):
        ground[y][0] = WALL
        ground[y][W - 1] = WALL
    return ground


def build_solid(ground):
    """solid 矩阵：院墙/水 + 建筑与道具占格（sprite 本身不参与碰撞）。"""
    solid = [[0] * W for _ in range(H)]
    for y in range(H):
        for x in range(W):
            if ground[y][x] in (WALL, WATER):
                solid[y][x] = 1
    for entry in BUILDINGS + PROPS:
        for (x, y) in entry["solid_cells"]:
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
        "name=combat_arena",
        "meta\tdisplay_name\t江湖有田·演武场",
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
        f"component\t{title_id}\tText\ttext={HUD_TITLE};font_size=18.00;color=0.949,0.918,0.847"
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
        f"component\t{hint_id}\tText\ttext={HUD_HINT};font_size=14.00;color=0.850,0.900,1.000"
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


def read_arena_registry(path=os.path.join("scripts", "data", "maps.lua")):
    """从 data/maps.lua 读 combat_arena 的 spawn/exit cell，做三方一致性校验。"""
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    marker = text.find("combat_arena = {")
    if marker < 0:
        raise SystemExit("data/maps.lua 里找不到 combat_arena 注册块")
    chunk = text[marker:]
    end = chunk.find("\n  }")
    if end >= 0:
        chunk = chunk[:end]

    def cell_of(pattern, label):
        match = re.search(pattern, chunk)
        if not match:
            raise SystemExit(f"data/maps.lua:combat_arena 解析不到 {label}")
        return int(match.group(1)), int(match.group(2))

    return {
        "spawn": cell_of(r"spawn\s*=\s*\{\s*x\s*=\s*(\d+)\s*,\s*y\s*=\s*(\d+)", "spawn"),
        "exit": cell_of(r"cell\s*=\s*\{\s*x\s*=\s*(\d+)\s*,\s*y\s*=\s*(\d+)", "exit cell"),
    }


def check_texture_sizes():
    """交叉校验：scene 里声明的 sprite 尺寸必须与贴图像素尺寸一致。

    引擎会把贴图缩放到 `size=` 指定的尺寸，声明错了不会报错、只会被悄悄拉伸。
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
    # 1) 行列长度与 tile 值域（preview_scene.py 同款校验，本地先跑一遍）
    if len(ground) != H or any(len(row) != W for row in ground):
        problems.append("tiles 矩形尺寸不为 40x22")
    if any(tile < 1 or tile > 6 for row in ground for tile in row):
        problems.append("tile 值超出 1..6")
    # 2) farm.lua 防误垦：全场禁止夯土（FARM 矩形无场景门禁，tile==2 即可开垦）
    for y in range(H):
        for x in range(W):
            if ground[y][x] == DIRT:
                problems.append(f"({x},{y}) 是夯土(tile 2)：farm.lua 会把它当可开垦田")
    # 3) 锚点：可走 + 石板 + 从出生点可达
    walk = reachable_from(solid, ANCHORS["spawn"])
    for name, (x, y) in ANCHORS.items():
        if solid[y][x]:
            problems.append(f"锚点 {name} ({x},{y}) 是 solid")
        elif (x, y) not in walk:
            problems.append(f"锚点 {name} ({x},{y}) 从出生点不可达")
        elif ground[y][x] != STONE:
            problems.append(f"锚点 {name} ({x},{y}) 地表 tile={ground[y][x]}，战斗格应为石板(3)")
    # 4) 灰狼寻路：home → 引敌站位必须连通（A* 追击验收的前提）
    if ANCHORS["wolf_lure"] not in reachable_from(solid, ANCHORS["wolf_home"]):
        problems.append("灰狼 home (2,9) 到引敌位 (6,8) 不可达，追击验收会失败")
    # 5) 门楼：门洞可走、立柱必须挡人
    for (x, y) in GATE_DOOR:
        if solid[y][x]:
            problems.append(f"门洞 ({x},{y}) 被挡住，出口进不去")
    for (x, y) in GATE_PILLARS:
        if not solid[y][x]:
            problems.append(f"门柱 ({x},{y}) 未标 solid，会穿模")
    # 6) 建筑与道具不许占用墙/水格
    for entry in BUILDINGS + PROPS:
        for (x, y) in entry["solid_cells"]:
            if ground[y][x] in (WALL, WATER):
                problems.append(f"{entry['name']} 占用了墙/水格 ({x},{y})")
    # 7) maps.lua 三方一致性
    registry = read_arena_registry()
    if registry["spawn"] != (SPAWN["x"], SPAWN["y"]):
        problems.append(f"maps.lua spawn={registry['spawn']} 与本文件 SPAWN 不一致")
    if registry["exit"] != (EXIT["x"], EXIT["y"]):
        problems.append(f"maps.lua 出口格={registry['exit']} 与本文件 EXIT 不一致")
    # 8) 贴图尺寸
    problems.extend(check_texture_sizes())
    return problems


def walkable_summary(solid):
    """每行可走列的紧凑描述，用于文档同步。"""
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


def main():
    ground = build_tiles()
    solid = build_solid(ground)
    text = build_scene(ground, solid)
    out = os.path.join("scenes", "combat_arena.scene")
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print(f"wrote {out} ({W}x{H}, {len(BUILDINGS)} 建筑 + {len(PROPS)} 道具)")
    problems = self_check(ground, solid)
    if problems:
        for problem in problems:
            print("[FAIL]", problem)
        raise SystemExit(1)
    anchors = " ".join(f"{name}({x},{y})" for name, (x, y) in ANCHORS.items())
    print(f"契约自检 ok：战斗锚点 {anchors} 全部可走、可达、石板；无夯土；门洞通畅；maps.lua 一致")
    print("可行走格事实（文档同步用）：")
    for line in walkable_summary(solid):
        print(line)


if __name__ == "__main__":
    main()
