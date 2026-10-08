# -*- coding: utf-8 -*-
"""《江湖有田》山庄主场景美术生成：农田/湿度 overlay + 建筑/道具 sprite。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/gen_manor_art.py            # 生成全部（overlay + 建筑 + 预览）
    python tools/gen_manor_art.py --check    # 只校验已生成资源

产出：

    assets/art/tilesets/manor/manor_farm.png     6 列 × 32px（农田 overlay）
    assets/art/tilesets/manor/manor_farm_moist.png 6 列 × 32px（湿度层，仅第 1 列有内容）
    assets/art/tilesets/manor/_preview.png       两张 overlay 放大预览（带标签）
    assets/art/buildings/manor_house.png         正房 224×224（7×7 格）
    assets/art/buildings/manor_shed.png          柴房 160×128（5×4 格）
    assets/art/buildings/manor_gate.png          南门 96×96（3×3 格）
    assets/art/buildings/_preview.png            建筑/道具放大预览（带标签）
    assets/art/props/manor_*.png                 水井/水缸/草堆/晒谷架/木牌/竹篱/竹丛/老树

**地表不在这里**：`manor_ground.png`（12 列 × 9 行的逻辑瓦 + 边缘过渡图集）自 M72-04 起
由 `tools/gen_terrain_tiles.py` 产出，色阶口径是 `tools/terrain_ramp.json`。本工具只从同一
张表取色画建筑与道具，所以两边永远是一个世界。

引擎约束（改前先对齐 `crates/rustlua-ecs/src/tilemap.rs`）：

  * Tilemap 按组件里声明的 `atlas_cols/atlas_rows` 切 UV（`tiles_format=1`），贴图尺寸必须
    正好等于 `cols×rows×tile`。本工具的 overlay 是恒等寻址（一格一列、无过渡块），所以
    仍是 6 列 × 1 行；改列数要连 `manor.scene` 里那两行一起改。
    （旧口径"`TILE_PALETTE` 写死 6 列、V 方向整幅拉伸"已在 M72-01 废掉。）
  * `scripts/systems/farm.lua` 判定"可开垦"用的是 **GroundMap tile == 2（夯土）**，
    且 overlay 列语义固定：1=已垦干土 2=已垦湿土 3=苗期 4=生长期 5=成熟。
    这两条不能改，否则农田玩法直接坏掉。
  * 建筑/道具的碰撞来自 GroundMap 的 solid 位（不是 sprite），
    所以"看起来实心"的 sprite 必须在 scene 的 solid 串里标 1。

依赖：Python 3 标准库 + 同目录 pixel_kit.py + terrain_kit.py（色阶表）。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import (  # noqa: E402  (path 插入后才能 import)
    Canvas,
    draw_text,
    hash_stream,
    mix,
    rgb,
    shade,
    text_width,
)
from cjk_text import draw_plaque, prewarm  # noqa: E402  中文匾额（GDI+ 栅格化）
import terrain_kit as tk  # noqa: E402  M72-04 色阶表：地表五族 + 通用四色的唯一口径

TILE = 32
# 农田/湿度两张 tileset 的列数。它现在只是**本工具与场景组件的约定**：引擎按组件里声明的
# `atlas_cols/atlas_rows` 切 UV（M72-01 起），所以改这个数必须同时改 `manor.scene` 里那两行。
TILE_COLUMNS = 6

# ---------------------------------------------------------------------------
# 色板（地表五族 + 通用四色从 `terrain_ramp.json` 取，其余是本工具自有的装饰色）
# ---------------------------------------------------------------------------

# M72-04 把色阶定成了机器可读的一张表：`tools/terrain_ramp.json`。凡是被地表图集用到的
# 颜色，一律从表里取，**生成器里不再现写 hex**——否则改一档明度要同时改两处，迟早分叉。
# 索引 = 该族"深→浅"的第几档；"深→浅"本身由 `gen_terrain_tiles.check()` 断言单调。
RAMP = tk.load_ramp()


def ramp(family, index):
    """色阶表里某材质族的一档（0=最深）。表里没有这一族就直接报错，不兜底。"""
    return rgb(tk.family_hexes(RAMP, family)[index])


def utility(name):
    """色阶表 `utility` 里的通用色（墨/纸/金/木）。"""
    return rgb(RAMP["utility"][name])


INK = utility("ink")            # 统一描边
PAPER = utility("paper")        # paper/50
PAPER_DIM = rgb("#D8CDB2")      # paper/200
GOLD = utility("gold")          # gold/500
GOLD_LIGHT = rgb("#E4C078")     # gold/300
CINNABAR = rgb("#C3452F")       # cinnabar/500

GRASS = ramp("grass", 1)
GRASS_DARK = ramp("grass", 0)
GRASS_LIGHT = ramp("grass", 2)
GRASS_HI = ramp("grass", 3)

DIRT = ramp("dirt", 2)
DIRT_DARK = ramp("dirt", 1)
DIRT_LIGHT = ramp("dirt", 3)
DIRT_WET = ramp("dirt", 0)

STONE = ramp("stone", 2)
STONE_LIGHT = ramp("stone", 3)
STONE_DARK = ramp("stone", 1)
STONE_MORTAR = ramp("stone", 0)

PLASTER = rgb("#C9C1B0")
PLASTER_DARK = rgb("#AFA795")

TILE_ROOF = ramp("roof", 1)
TILE_ROOF_LIGHT = ramp("roof", 3)
TILE_ROOF_DARK = ramp("roof", 0)
TILE_RIDGE = ramp("roof", 2)

WOOD = rgb("#8A6A46")
WOOD_LIGHT = rgb("#A6855C")
WOOD_MID = rgb("#6B4F3A")
WOOD_DARK = utility("wood")

WATER_DEEP = ramp("water", 0)
WATER = ramp("water", 1)
WATER_LIGHT = ramp("water", 2)
WATER_HI = ramp("water", 3)

BAMBOO = rgb("#789262")
BAMBOO_LIGHT = rgb("#A9C28F")
BAMBOO_DARK = rgb("#5B7048")
LEAF_DARK = rgb("#3A5230")
LEAF = rgb("#4A6636")
LEAF_LIGHT = rgb("#5E7C46")

STRAW = rgb("#C9A05A")
STRAW_DARK = rgb("#A8813F")
STRAW_LIGHT = rgb("#E4C078")


# ---------------------------------------------------------------------------
# 通用小工具
# ---------------------------------------------------------------------------


def fill_ellipse(canvas, cx, cy, rx, ry, color):
    for y in range(int(cy - ry), int(cy + ry) + 1):
        for x in range(int(cx - rx), int(cx + rx) + 1):
            dx = (x - cx) / max(rx, 0.001)
            dy = (y - cy) / max(ry, 0.001)
            if dx * dx + dy * dy <= 1.0:
                canvas.set(x, y, color)


def speckle(canvas, x0, y0, x1, y1, color, count, seed):
    rnd = hash_stream(seed)
    for _ in range(count):
        x = x0 + rnd(max(1, x1 - x0 + 1))
        y = y0 + rnd(max(1, y1 - y0 + 1))
        canvas.set(x, y, color)


def wood_post(canvas, x0, y0, x1, y1, light=WOOD_LIGHT, base=WOOD_MID, dark=WOOD_DARK):
    canvas.rect(x0, y0, x1, y1, base)
    canvas.vline(x0, y0, y1, light)
    canvas.vline(x1, y0, y1, dark)


def roof_rows(top_y, bottom_y, top_x0, top_x1, bottom_x0, bottom_x1):
    """梯形屋顶的逐行左右边界（上沿=正脊，下沿=檐口）。"""
    rows = []
    for y in range(top_y, bottom_y + 1):
        t = (y - top_y) / max(1, bottom_y - top_y)
        x0 = int(round(top_x0 + (bottom_x0 - top_x0) * t))
        x1 = int(round(top_x1 + (bottom_x1 - top_x1) * t))
        rows.append((y, x0, x1))
    return rows


def tiled_roof(canvas, polygon_rows, column=16):
    """按行填充瓦面：polygon_rows = [(y, x0, x1), ...]（梯形），再叠瓦垄竖纹。

    瓦垄必须沿坡面从正脊连到檐口，所以按"上沿列位置 → 下沿列位置"做线性插值画线；
    直接按行取模画竖线会得到斜网格（试过，像席子不像瓦顶）。
    """
    for y, x0, x1 in polygon_rows:
        canvas.hline(x0, x1, y, TILE_ROOF)
    for index, (y, x0, x1) in enumerate(polygon_rows):
        phase = index % 3
        if phase == 0:
            canvas.hline(x0, x1, y, TILE_ROOF_LIGHT)
        elif phase == 2:
            canvas.hline(x0, x1, y, shade(TILE_ROOF, 0.92))
    if not polygon_rows:
        return
    top_y, top_x0, top_x1 = polygon_rows[0]
    bottom_y, bottom_x0, bottom_x1 = polygon_rows[-1]
    count = max(2, (top_x1 - top_x0) // column)
    steps = max(1, bottom_y - top_y)
    for k in range(1, count):
        t = k / count
        x_top = top_x0 + (top_x1 - top_x0) * t
        x_bottom = bottom_x0 + (bottom_x1 - bottom_x0) * t
        for i in range(steps + 1):
            y = top_y + i
            x = int(round(x_top + (x_bottom - x_top) * (i / steps)))
            canvas.set(x, y, TILE_ROOF_LIGHT)
            canvas.set(x + 1, y, TILE_ROOF_DARK)


# ---------------------------------------------------------------------------
# 农田 overlay tileset（6 列 × 32px）
#
# 地表五族（草/土/石/墙/瓦/水）不在这个文件里：M72-04 起由 `gen_terrain_tiles.py` 按
# `terrain_ramp.json` 画成 12 列 × 9 行的逻辑瓦图集（含 16 格边缘过渡）。本工具只留农田
# 与湿度两张 overlay——它们是恒等寻址（一格一列，没有过渡块），所以列数仍是 6。
# ---------------------------------------------------------------------------


def tile_farm(base, furrow, wet=False, seed=77):
    """农田底：3 道垄沟（overlay 只在 FarmGrid 上画，其余透出 GroundMap 夯土）。"""
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, base)
    for y in (5, 13, 21, 29):
        cv.rect(0, y, TILE - 1, y + 2, furrow)
        cv.hline(0, TILE - 1, y - 1, shade(base, 1.12) if not wet else DIRT)
    rnd = hash_stream(seed + (1 if wet else 0))
    for _ in range(22):
        cv.set(rnd(TILE), rnd(TILE), shade(base, 0.88))
    for _ in range(14):
        cv.set(rnd(TILE), rnd(TILE), shade(base, 1.1))
    if wet:
        for y in (4, 12, 20, 28):
            for x in range(1, TILE - 1, 5):
                cv.set(x, y, mix(furrow, WATER_DEEP, 0.35))
    return cv


def farm_sprout(seed=88):
    """苗期：垄沟上 3 株小苗。"""
    cv = tile_farm(DIRT, DIRT_DARK, seed=seed)
    rnd = hash_stream(seed)
    for _ in range(3):
        x = 4 + rnd(TILE - 10)
        y = 6 + rnd(TILE - 12)
        cv.vline(x, y, y + 3, BAMBOO_LIGHT)
        cv.set(x - 1, y + 1, BAMBOO)
        cv.set(x + 1, y + 1, BAMBOO)
        cv.set(x, y - 1, BAMBOO_LIGHT)
    return cv


def farm_growing(seed=99):
    """生长期：2 株更大的作物。"""
    cv = tile_farm(DIRT, DIRT_DARK, seed=seed)
    rnd = hash_stream(seed)
    for _ in range(2):
        x = 6 + rnd(TILE - 14)
        y = 8 + rnd(TILE - 16)
        cv.vline(x, y, y + 6, LEAF)
        cv.set(x, y - 1, LEAF_LIGHT)
        for offset in (0, 2, 4):
            cv.set(x - 1, y + offset, LEAF_LIGHT)
            cv.set(x + 1, y + offset + 1, LEAF)
            cv.set(x - 2, y + offset + 1, LEAF)
    return cv


def farm_ripe(seed=111):
    """成熟：金色穗头（稻/灵芝的成熟意象）。"""
    cv = tile_farm(DIRT, DIRT_DARK, seed=seed)
    rnd = hash_stream(seed)
    for _ in range(2):
        x = 6 + rnd(TILE - 14)
        y = 9 + rnd(TILE - 16)
        cv.vline(x, y, y + 7, STRAW_DARK)
        cv.rect(x - 2, y - 3, x + 2, y, STRAW)
        cv.hline(x - 2, x + 2, y - 3, STRAW_LIGHT)
        cv.set(x, y - 4, GOLD_LIGHT)
        cv.set(x - 1, y + 4, STRAW_LIGHT)
        cv.set(x + 1, y + 5, STRAW_LIGHT)
    for _ in range(6):
        cv.set(2 + rnd(TILE - 4), 2 + rnd(TILE - 4), GOLD_LIGHT)
    return cv


def farm_stubble(seed=122):
    """第 6 列（保留位）：收割后的稻茬，暂未接入玩法。"""
    cv = tile_farm(DIRT, DIRT_DARK, seed=seed)
    for x in range(3, TILE - 2, 5):
        cv.vline(x, 16, 19, STRAW_DARK)
        cv.vline(x + 1, 24, 27, STRAW)
    return cv


def farm_moist_mark(seed=133):
    """湿度层第 1 列（值 1 = 当日已浇）：只画四角湿痕与水滴，中心留空。

    为什么单开一张图而不给 FarmGrid 加"湿版"作物贴图：引擎的 tile 值上限是 6
    （`core/tilemap.rs` 的 `TILE_PALETTE` 长 6，UV 按 6 列切），而 FarmGrid 六列已被
    干垦/湿垦/苗/生长/成熟/稻茬占满，"三阶段 × 干湿"要 8 槽塞不下。湿态因此画成
    叠在 FarmGrid 之上的独立一层（半透明，不遮作物），而不是新增列。
    """
    cv = Canvas(TILE, TILE)
    rim = (22, 32, 44, 116)
    corner = (18, 27, 38, 150)
    dash = (26, 38, 50, 74)
    # 四周 2px 湿边：把"土色变深"读在边界上，作物所在的中部完全留给 FarmGrid 贴图。
    for x in range(TILE):
        for y in (0, 1, TILE - 2, TILE - 1):
            cv.set(x, y, rim)
    for y in range(2, TILE - 2):
        for x in (0, 1, TILE - 2, TILE - 1):
            cv.set(x, y, rim)
    # 四角加深：让湿痕有"积在角落"的体积感。
    for cx, cy in ((0, 0), (TILE - 4, 0), (0, TILE - 4), (TILE - 4, TILE - 4)):
        cv.rect(cx, cy, cx + 3, cy + 3, corner)
    # 垄沟位置上的短横段：与 FarmGrid 的三道垄（y=5/13/21/29）同排，读作沟里渗水。
    for y in (6, 14, 22):
        for x in range(3, TILE - 5, 6):
            cv.rect(x, y, x + 2, y + 1, dash)
    rnd = hash_stream(seed)
    for _ in range(4):
        x = 2 + rnd(TILE - 6)
        y = 2 + rnd(TILE - 8)
        cv.rect(x, y + 1, x + 1, y + 3, (86, 140, 190, 205))
        cv.set(x, y, (86, 140, 190, 205))
        cv.set(x + 1, y + 2, (178, 214, 238, 230))
    return cv


def farm_moist_tiles():
    """湿度层 tileset：只有第 1 列有内容，其余 5 列全透明占位（列数须与组件声明一致）。"""
    return [farm_moist_mark()] + [Canvas(TILE, TILE) for _ in range(TILE_COLUMNS - 1)]


def farm_tiles():
    return [
        tile_farm(DIRT, DIRT_DARK, seed=77),
        tile_farm(DIRT_DARK, DIRT_WET, wet=True, seed=78),
        farm_sprout(),
        farm_growing(),
        farm_ripe(),
        farm_stubble(),
    ]


def pack_tileset(tiles):
    assert len(tiles) == TILE_COLUMNS, f"tileset 必须正好 {TILE_COLUMNS} 列"
    sheet = Canvas(TILE * TILE_COLUMNS, TILE)
    for index, tile in enumerate(tiles):
        sheet.blit(tile, index * TILE, 0)
    return sheet


# ---------------------------------------------------------------------------
# 建筑
# ---------------------------------------------------------------------------


def building_house():
    """正房 224×224（7×7 格）：悬山瓦顶 + 白灰墙 + 木格窗 + 板门 + 石台基。"""
    cv = Canvas(224, 224)
    # --- 屋顶：上窄下宽（檐口出挑）---
    rows = roof_rows(10, 124, 36, 188, 8, 216)
    tiled_roof(cv, rows)
    # 正脊 + 鸱吻
    cv.rect(30, 6, 194, 20, TILE_RIDGE)
    cv.hline(30, 194, 6, TILE_ROOF_LIGHT)
    cv.hline(30, 194, 20, TILE_ROOF_DARK)
    cv.rect(22, 2, 34, 20, TILE_ROOF_DARK)
    cv.rect(190, 2, 202, 20, TILE_ROOF_DARK)
    cv.set(26, 4, TILE_ROOF_LIGHT)
    cv.set(198, 4, TILE_ROOF_LIGHT)
    # 檐口
    cv.rect(6, 118, 217, 126, TILE_ROOF_DARK)
    cv.hline(6, 217, 117, TILE_ROOF_LIGHT)
    # --- 墙身 ---
    cv.rect(20, 126, 203, 196, PLASTER)
    # 檐下阴影
    cv.rect(20, 126, 203, 134, mix(PLASTER, INK, 0.28))
    cv.hline(20, 203, 135, mix(PLASTER, INK, 0.14))
    # 木柱
    wood_post(cv, 20, 126, 30, 196)
    wood_post(cv, 96, 126, 106, 196)
    wood_post(cv, 193, 126, 203, 196)
    # 窗（木格 + 纸）
    for wx in (40, 146):
        cv.rect(wx, 146, wx + 38, 172, WOOD_MID)
        cv.rect(wx + 2, 148, wx + 36, 170, PAPER)
        for gx in range(wx + 2, wx + 37, 4):
            cv.vline(gx, 148, 170, WOOD_MID)
        cv.hline(wx + 2, wx + 36, 159, WOOD_MID)
    # 门
    cv.rect(94, 138, 130, 196, WOOD_MID)
    cv.rect(98, 142, 126, 196, WOOD_DARK)
    cv.vline(112, 142, 196, WOOD)
    cv.set(106, 162, GOLD)
    cv.set(107, 162, GOLD_LIGHT)
    cv.set(118, 162, GOLD)
    cv.set(117, 162, GOLD_LIGHT)
    cv.hline(94, 130, 138, WOOD)
    # 门楣匾额「山庄」（中文，走 cjk_text 的 GDI+ 栅格化）
    draw_plaque(cv, 84, 122, 140, 138, "山庄", size=12)
    # 灯笼（门两侧）
    for lx in (84, 136):
        cv.vline(lx + 5, 128, 136, WOOD_DARK)
        cv.rect(lx, 136, lx + 11, 150, CINNABAR)
        cv.hline(lx, lx + 11, 136, GOLD)
        cv.hline(lx, lx + 11, 150, GOLD)
        cv.vline(lx + 5, 151, 154, CINNABAR)
    # --- 台基与台阶 ---
    cv.rect(12, 196, 211, 207, STONE)
    cv.hline(12, 211, 196, STONE_LIGHT)
    cv.hline(12, 211, 207, STONE_MORTAR)
    for x in range(20, 211, 24):
        cv.vline(x, 197, 206, STONE_MORTAR)
    for step in range(3):
        y0 = 208 + step * 4
        x0 = 96 - step * 3
        x1 = 128 + step * 3
        cv.rect(x0, y0, x1, y0 + 3, STONE_LIGHT if step % 2 == 0 else STONE)
        cv.hline(x0, x1, y0 + 3, STONE_MORTAR)
    cv.outline(INK)
    return cv


def building_shed():
    """柴房 160×128（5×4 格）：茅草顶 + 木板墙 + 柴堆。"""
    cv = Canvas(160, 128)
    rows = roof_rows(8, 68, 30, 130, 6, 154)
    for y, x0, x1 in rows:
        cv.hline(x0, x1, y, STRAW if (y // 3) % 2 == 0 else STRAW_DARK)
    for x in range(10, 152, 7):
        cv.vline(x, 10 + (x % 5), 66, STRAW_DARK)
    cv.hline(6, 154, 68, STRAW_DARK)
    # 木板墙
    cv.rect(24, 72, 136, 112, WOOD_MID)
    for x in range(26, 136, 6):
        cv.vline(x, 72, 112, WOOD_DARK)
    cv.rect(60, 76, 100, 112, WOOD_DARK)
    cv.rect(64, 80, 96, 112, shade(WOOD_DARK, 0.82))
    # 柴堆
    for index, y in enumerate(range(96, 112, 5)):
        cv.rect(104, y, 134, y + 4, WOOD if index % 2 == 0 else WOOD_LIGHT)
        cv.hline(104, 134, y + 4, WOOD_DARK)
    # 台基
    cv.rect(18, 112, 142, 121, STONE)
    cv.hline(18, 142, 112, STONE_LIGHT)
    cv.hline(18, 142, 121, STONE_MORTAR)
    cv.outline(INK)
    return cv


def building_gate():
    """南门 96×96（3×3 格）：门楼瓦顶 + 木柱 + 匾额「山庄」。"""
    cv = Canvas(96, 96)
    rows = roof_rows(8, 34, 14, 82, 0, 95)
    tiled_roof(cv, rows, column=12)
    cv.rect(10, 4, 86, 14, TILE_RIDGE)
    cv.hline(10, 86, 4, TILE_ROOF_LIGHT)
    cv.rect(4, 2, 14, 16, TILE_ROOF_DARK)
    cv.rect(82, 2, 92, 16, TILE_ROOF_DARK)
    cv.rect(0, 30, 95, 36, TILE_ROOF_DARK)
    # 立柱
    wood_post(cv, 8, 36, 22, 88)
    wood_post(cv, 74, 36, 88, 88)
    # 匾额「山庄」
    draw_plaque(cv, 26, 38, 70, 60, "山庄", size=13)
    # 灯笼
    cv.vline(28, 58, 64, WOOD_DARK)
    cv.rect(24, 64, 33, 76, CINNABAR)
    cv.hline(24, 33, 64, GOLD)
    cv.hline(24, 33, 76, GOLD)
    # 石阶
    cv.rect(14, 88, 82, 95, STONE)
    cv.hline(14, 82, 88, STONE_LIGHT)
    cv.hline(14, 82, 95, STONE_MORTAR)
    cv.outline(INK)
    return cv


# ---------------------------------------------------------------------------
# 道具
# ---------------------------------------------------------------------------


def prop_well():
    cv = Canvas(32, 42)
    # 井台石圈
    cv.rect(4, 22, 28, 36, STONE)
    cv.hline(4, 28, 22, STONE_LIGHT)
    cv.hline(4, 28, 36, STONE_MORTAR)
    for x in range(6, 28, 6):
        cv.vline(x, 23, 35, STONE_MORTAR)
    cv.rect(10, 22, 22, 28, WATER_DEEP)
    cv.hline(10, 22, 23, WATER_LIGHT)
    # 木架 + 辘轳
    wood_post(cv, 3, 6, 7, 24)
    wood_post(cv, 25, 6, 29, 24)
    cv.rect(3, 6, 29, 10, WOOD)
    cv.hline(3, 29, 6, WOOD_LIGHT)
    cv.rect(10, 10, 22, 16, WOOD_MID)
    cv.vline(22, 12, 14, WOOD_DARK)
    # 绳与桶
    cv.vline(15, 16, 24, WOOD_LIGHT)
    cv.rect(11, 24, 21, 32, WOOD)
    cv.hline(11, 21, 24, WOOD_LIGHT)
    cv.hline(11, 21, 28, WOOD_DARK)
    cv.outline(INK)
    return cv


def prop_jar():
    cv = Canvas(24, 30)
    for y in range(6, 28):
        t = (y - 6) / 21.0
        half = 4 + int(round(6 * (1 - abs(t - 0.4) * 1.8)))
        half = max(4, min(9, half))
        cv.hline(12 - half, 12 + half, y, TILE_ROOF)
    cv.rect(4, 4, 20, 8, TILE_ROOF_DARK)
    cv.hline(4, 20, 4, TILE_ROOF_LIGHT)
    cv.rect(7, 8, 17, 10, WATER_DEEP)
    cv.vline(7, 10, 22, TILE_ROOF_LIGHT)
    cv.outline(INK)
    return cv


def prop_haystack():
    cv = Canvas(36, 34)
    for y in range(6, 32):
        t = (y - 6) / 25.0
        half = int(round(4 + 14 * (1 - (1 - t) ** 2)))
        cv.hline(18 - half, 18 + half, y, STRAW if (y // 3) % 2 == 0 else STRAW_DARK)
    cv.vline(18, 0, 10, WOOD_MID)
    for x in range(6, 32, 5):
        cv.vline(x, 12, 30, STRAW_LIGHT)
    cv.hline(2, 34, 32, shade(STRAW_DARK, 0.8))
    cv.outline(INK)
    return cv


def prop_rack():
    """晒谷架 64×46：木架 + 竹席 + 摊晒的谷粒。"""
    cv = Canvas(64, 46)
    for x in (3, 19, 39, 55):
        wood_post(cv, x, 14, x + 4, 44)
    cv.rect(3, 14, 59, 18, WOOD)
    cv.rect(3, 26, 59, 30, WOOD)
    cv.rect(2, 6, 62, 26, BAMBOO)
    cv.hline(2, 62, 6, BAMBOO_LIGHT)
    for x in range(4, 62, 4):
        cv.vline(x, 7, 25, BAMBOO_DARK)
    for x in range(5, 60, 3):
        cv.set(x, 12 + (x % 5), GOLD_LIGHT)
        cv.set(x + 1, 18 + (x % 4), GOLD)
    cv.outline(INK)
    return cv


def prop_sign():
    """药田木牌 32×42：牌子写「药田」。"""
    cv = Canvas(32, 42)
    wood_post(cv, 14, 16, 18, 40)
    draw_plaque(cv, 2, 2, 30, 20, "药田", size=11)
    cv.outline(INK)
    return cv


def prop_fence():
    """竹篱 24×64（南北走向的一段）：竖栏 + 三道横档。"""
    cv = Canvas(24, 64)
    wood_post(cv, 2, 0, 6, 63, light=BAMBOO_LIGHT, base=BAMBOO, dark=BAMBOO_DARK)
    wood_post(cv, 18, 0, 22, 63, light=BAMBOO_LIGHT, base=BAMBOO, dark=BAMBOO_DARK)
    for y in (10, 30, 50):
        cv.rect(2, y, 22, y + 3, BAMBOO_LIGHT)
        cv.hline(2, 22, y + 3, BAMBOO_DARK)
    for y in (16, 36, 56):
        cv.hline(2, 22, y, BAMBOO_DARK)
    cv.outline(INK)
    return cv


def prop_bamboo():
    """竹丛 48×76。"""
    cv = Canvas(48, 76)
    for x0, top in ((6, 0), (18, 4), (32, 2)):
        x1 = x0 + 6
        cv.rect(x0, top, x1, 75, BAMBOO)
        cv.vline(x0, top, 75, BAMBOO_LIGHT)
        cv.vline(x1, top, 75, BAMBOO_DARK)
        for y in range(top + 6, 74, 14):
            cv.hline(x0, x1, y, BAMBOO_DARK)
            cv.hline(x0, x1, y - 1, BAMBOO_LIGHT)
    # 竹叶：上部斜向短笔
    for x, y in ((2, 14), (10, 8), (16, 20), (24, 12), (30, 18), (36, 8), (40, 16), (12, 28), (28, 30)):
        cv.hline(x, x + 3, y, LEAF_LIGHT)
        cv.hline(x + 1, x + 4, y + 1, LEAF)
        cv.set(x + 4, y + 2, LEAF_LIGHT)
    cv.outline(INK)
    return cv


def prop_tree():
    """老树 64×88：树干 + 三层树冠。"""
    cv = Canvas(64, 88)
    wood_post(cv, 28, 54, 38, 88, light=WOOD_LIGHT, base=WOOD_MID, dark=WOOD_DARK)
    cv.rect(24, 84, 42, 87, WOOD_DARK)
    fill_ellipse(cv, 32, 30, 24, 22, LEAF)
    fill_ellipse(cv, 15, 44, 14, 12, LEAF)
    fill_ellipse(cv, 49, 44, 14, 12, LEAF)
    fill_ellipse(cv, 26, 22, 12, 10, LEAF_LIGHT)
    rnd = hash_stream(404)
    for _ in range(34):
        x = rnd(64)
        y = rnd(60)
        if cv.get(x, y)[:3] == LEAF[:3]:
            cv.set(x, y, LEAF_LIGHT if rnd(2) == 0 else LEAF_DARK)
    cv.outline(INK)
    return cv


def props():
    return {
        "well": prop_well(),
        "jar": prop_jar(),
        "haystack": prop_haystack(),
        "rack": prop_rack(),
        "sign": prop_sign(),
        "fence": prop_fence(),
        "bamboo": prop_bamboo(),
        "tree": prop_tree(),
    }


def buildings():
    return {
        "house": building_house(),
        "shed": building_shed(),
        "gate": building_gate(),
    }


# ---------------------------------------------------------------------------
# 输出
# ---------------------------------------------------------------------------

TILE_DIR = os.path.join("assets", "art", "tilesets", "manor")
BUILDING_DIR = os.path.join("assets", "art", "buildings")
PROP_DIR = os.path.join("assets", "art", "props")


def write_png(path, canvas):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(canvas.to_png())


def build_tiles_preview(farm_tile_list, moist_tile_list, scale=4):
    """两张 overlay 的放大预览：农田 6 列 + 湿度第 1 列。

    地表不再出现在这里——它的预览是 `gen_terrain_tiles.py` 产的 `_ground_preview.png`，
    那张带 16 格边缘过渡，按"6 列一行一瓦"的老排法已经画不下了。
    """
    labels = (["TILLED", "WATERED", "SPROUT", "GROWING", "RIPE", "STUBBLE"]
              + ["MOIST-1"])
    rows = list(enumerate(farm_tile_list)) + [(None, moist_tile_list[0])]
    tile = TILE * scale
    label_w = text_width("WATERED", 1) + 10
    width = label_w + tile + 8
    row_h = tile + 4
    height = 20 + len(rows) * row_h + 8
    cv = Canvas(width, height)
    cv.rect(0, 0, width - 1, height - 1, rgb("#14171A"))
    draw_text(cv, 4, 4, "MANOR OVERLAYS 32PX X4", GOLD_LIGHT, 1)
    for row, (index, source) in enumerate(rows):
        y = 20 + row * row_h
        draw_text(cv, 4, y + tile // 2 - 2, labels[row], BAMBOO_LIGHT, 1)
        cv.rect(label_w - 1, y - 1, label_w + tile, y + tile, rgb("#262C33"))
        cv.blit(source.scaled(scale), label_w, y)
    return cv


def build_prop_preview(sprites, scale=2):
    items = sorted(sprites.items())
    pad = 10
    cols = 4
    label_h = 12
    cell_w = max(sprite.w for _, sprite in items) * scale + pad * 2
    cell_h = max(sprite.h for _, sprite in items) * scale + pad * 2 + label_h
    rows = (len(items) + cols - 1) // cols
    width = cell_w * cols + 8
    height = 20 + cell_h * rows + 8
    cv = Canvas(width, height)
    cv.rect(0, 0, width - 1, height - 1, rgb("#14171A"))
    draw_text(cv, 4, 4, "MANOR BUILDINGS + PROPS", GOLD_LIGHT, 1)
    for index, (name, sprite) in enumerate(items):
        row = index // cols
        col = index % cols
        x0 = 4 + col * cell_w
        y0 = 20 + row * cell_h
        draw_text(cv, x0 + pad, y0, name, BAMBOO_LIGHT, 1)
        # 棋盘底，便于看透明边缘
        for by in range(0, sprite.h * scale, 8):
            for bx in range(0, sprite.w * scale, 8):
                tone = rgb("#262C33") if ((bx // 8 + by // 8) % 2 == 0) else rgb("#31383F")
                cv.rect(x0 + pad + bx, y0 + label_h + by,
                        x0 + pad + bx + 7, y0 + label_h + by + 7, tone)
        cv.blit(sprite.scaled(scale), x0 + pad, y0 + label_h)
    return cv


def generate(quiet=False):
    # 中文招牌先批量栅格化（一次 PowerShell 调用，避免逐字调用）
    prewarm([("山庄", 12, "pixel", True), ("山庄", 13, "pixel", True), ("药田", 11, "pixel", True)])
    farm_tile_list = farm_tiles()
    moist_tile_list = farm_moist_tiles()
    farm_sheet = pack_tileset(farm_tile_list)
    moist_sheet = pack_tileset(moist_tile_list)
    write_png(os.path.join(TILE_DIR, "manor_farm.png"), farm_sheet)
    write_png(os.path.join(TILE_DIR, "manor_farm_moist.png"), moist_sheet)
    write_png(os.path.join(TILE_DIR, "_preview.png"),
              build_tiles_preview(farm_tile_list, moist_tile_list))

    for name, sprite in buildings().items():
        write_png(os.path.join(BUILDING_DIR, f"manor_{name}.png"), sprite)
    for name, sprite in props().items():
        write_png(os.path.join(PROP_DIR, f"manor_{name}.png"), sprite)

    all_sprites = dict(buildings())
    all_sprites.update(props())
    write_png(os.path.join(BUILDING_DIR, "_preview.png"), build_prop_preview(all_sprites))

    if not quiet:
        print(f"tileset  manor_farm.png   {farm_sheet.w}x{farm_sheet.h} (6 列)")
        print(f"tileset  manor_farm_moist.png {moist_sheet.w}x{moist_sheet.h} (6 列，仅第 1 列有内容)")
        for name, sprite in sorted(all_sprites.items()):
            print(f"sprite   manor_{name}.png {sprite.w}x{sprite.h}")
        print(f"preview  {TILE_DIR}/_preview.png, {BUILDING_DIR}/_preview.png")


def check():
    from pixel_kit import decode_png

    issues = []
    for path, want_w, want_h in (
        (os.path.join(TILE_DIR, "manor_farm.png"), TILE * TILE_COLUMNS, TILE),
        (os.path.join(TILE_DIR, "manor_farm_moist.png"), TILE * TILE_COLUMNS, TILE),
    ):
        if not os.path.exists(path):
            issues.append("missing " + path)
            continue
        width, height, _, _ = decode_png(path)
        if (width, height) != (want_w, want_h):
            # 尺寸必须等于"组件声明的 atlas_cols/atlas_rows × 32"，否则引擎按声明切 UV 会错位。
            issues.append(f"{path}: {width}x{height} != {want_w}x{want_h}"
                          f"（须与 manor.scene 里声明的 {TILE_COLUMNS} 列 × 1 行一致）")
    for name in buildings():
        path = os.path.join(BUILDING_DIR, f"manor_{name}.png")
        if not os.path.exists(path):
            issues.append("missing " + path)
    for name in props():
        path = os.path.join(PROP_DIR, f"manor_{name}.png")
        if not os.path.exists(path):
            issues.append("missing " + path)
    return issues


def main():
    parser = argparse.ArgumentParser(description="《江湖有田》山庄场景美术生成")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        problems = check()
        if problems:
            for problem in problems:
                print("[FAIL]", problem)
            raise SystemExit(1)
        print("[OK] 山庄美术资源校验通过")
        return
    generate()


if __name__ == "__main__":
    main()
