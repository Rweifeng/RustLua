# -*- coding: utf-8 -*-
"""《江湖有田》山庄主场景美术生成：建筑 / 道具 sprite。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/gen_manor_art.py            # 生成全部（建筑 + 道具 + 预览）
    python tools/gen_manor_art.py --check    # 只校验已生成资源

产出：

    assets/art/buildings/manor_house.png         正房 224×224（7×7 格）
    assets/art/buildings/manor_shed.png          柴房 160×128（5×4 格）
    assets/art/buildings/manor_gate.png          南门 96×96（3×3 格）
    assets/art/buildings/_preview.png            建筑/道具放大预览（带标签）
    assets/art/props/manor_*.png                 水井/水缸/草堆/晒谷架/木牌/竹篱/竹丛/老树

**地表不在这里**：`manor_ground.png`（12 列 × 9 行的逻辑瓦 + 边缘过渡图集）自 M72-04 起
由 `tools/gen_terrain_tiles.py` 产出，色阶口径是 `tools/terrain_ramp.json`。本工具只从同一
张表取色画建筑与道具，所以两边永远是一个世界。

**农田三态也不在这里**：`manor_farm*.png` 自 M72-08 起由 `tools/gen_farm_soil.py` 产出，
作物阶段图由 `tools/gen_crop_art.py` 产出。搬走的理由是归属而不是审美——M72-07 的在制品
压在本文件的建筑/屋顶区，农田区若同文件并行改，提交时挑不出自己的 hunk。

引擎约束（改前先对齐 `crates/rustlua-ecs/src/tilemap.rs`）：

  * Tilemap 按组件里声明的 `atlas_cols/atlas_rows` 切 UV（`tiles_format=1`），贴图尺寸必须
    正好等于 `cols×rows×tile`（旧口径"`TILE_PALETTE` 写死 6 列、V 方向整幅拉伸"已在
    M72-01 废掉）。本工具出的是 sprite 不是 tilemap，所以只有拆层那几处受这条约束。
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
    write_png,
)
from cjk_text import draw_plaque, prewarm  # noqa: E402  中文匾额（GDI+ 栅格化）
from light_kit import grade_toward_light, lit_sides  # noqa: E402  M72-07：贴图明暗只认 lighting.lua 的 sun
import layer_kit as layers  # noqa: E402  M72-06/11：拆层登记表（LAYER_SPLITS）与切块机器
from layer_kit import BUILDING_DIR, PROP_DIR, check_splits, write_splits  # noqa: E402
import terrain_kit as tk  # noqa: E402  M72-04 色阶表：地表五族 + 通用四色的唯一口径

TILE = 32

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

DIRT_LIGHT = ramp("dirt", 3)

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
    """木柱/木梁：受光边在光源那一侧，背光边落影（两侧跟着 `lighting.lua` 的 sun 翻）。"""
    canvas.rect(x0, y0, x1, y1, base)
    near, far = (light, dark) if lit_sides()["left"] else (dark, light)
    canvas.vline(x0, y0, y1, near)
    canvas.vline(x1, y0, y1, far)


def lattice_window(canvas, x0, y0, x1, y1, mullion=4):
    """木格窗：木框 + 纸 + 竖棂 + 一道中横，框的左上受光、右下落影。

    正房、柴房、可建造建筑共用这一份，不然"同一个山庄里三种窗"迟早出现。
    明暗边按 `lighting.lua` 的 sun 走（光从左上 ⇒ 顶边/左边亮）；换光向这里跟着翻。
    """
    lit = lit_sides()
    edge_light = WOOD_LIGHT if lit["left"] else WOOD_DARK
    edge_dark = WOOD_DARK if lit["left"] else WOOD_LIGHT
    canvas.rect(x0, y0, x1, y1, WOOD_MID)
    canvas.rect(x0 + 2, y0 + 2, x1 - 2, y1 - 2, PAPER)
    for gx in range(x0 + 2, x1 - 1, mullion):
        canvas.vline(gx, y0 + 2, y1 - 2, WOOD_MID)
    canvas.hline(x0 + 2, x1 - 2, (y0 + y1) // 2, WOOD_MID)
    canvas.hline(x0, x1, y0, edge_light)
    canvas.hline(x0, x1, y1, edge_dark)
    canvas.vline(x0, y0, y1, edge_light)
    canvas.vline(x1, y0, y1, edge_dark)


def roof_rows(top_y, bottom_y, top_x0, top_x1, bottom_x0, bottom_x1):
    """梯形屋顶的逐行左右边界（上沿=正脊，下沿=檐口）。"""
    rows = []
    for y in range(top_y, bottom_y + 1):
        t = (y - top_y) / max(1, bottom_y - top_y)
        x0 = int(round(top_x0 + (bottom_x0 - top_x0) * t))
        x1 = int(round(top_x1 + (bottom_x1 - top_x1) * t))
        rows.append((y, x0, x1))
    return rows


def tiled_roof(canvas, polygon_rows, column=16, palette=None):
    """按行填充瓦面：polygon_rows = [(y, x0, x1), ...]（梯形），再叠瓦垄竖纹。

    瓦垄必须沿坡面从正脊连到檐口，所以按"上沿列位置 → 下沿列位置"做线性插值画线；
    直接按行取模画竖线会得到斜网格（试过，像席子不像瓦顶）。

    `palette` 是 (瓦面, 高光, 暗部) 三档（M72-11 加）：默认取山庄的 `roof` 族，村庄的屋宇
    传自己的 `slate` 族。以前这里写死山庄三档，于是村庄的屋顶是山庄青瓦、正脊与檐口是
    村庄黛瓦——**同一张图里两种瓦**，而"黛瓦"这个材质在色阶表里只有一个族。参数化之后
    山庄一侧不传就是原样（逐字节相同），村庄一侧和图集院墙压顶同色。
    """
    base, light, dark = palette if palette else (TILE_ROOF, TILE_ROOF_LIGHT, TILE_ROOF_DARK)
    for y, x0, x1 in polygon_rows:
        canvas.hline(x0, x1, y, base)
    for index, (y, x0, x1) in enumerate(polygon_rows):
        phase = index % 3
        if phase == 0:
            canvas.hline(x0, x1, y, light)
        elif phase == 2:
            canvas.hline(x0, x1, y, shade(base, 0.92))
    if not polygon_rows:
        return
    top_y, top_x0, top_x1 = polygon_rows[0]
    bottom_y, bottom_x0, bottom_x1 = polygon_rows[-1]
    count = max(2, (top_x1 - top_x0) // column)
    steps = max(1, bottom_y - top_y)
    near, far = ((light, dark) if lit_sides()["left"] else (dark, light))
    for k in range(1, count):
        t = k / count
        x_top = top_x0 + (top_x1 - top_x0) * t
        x_bottom = bottom_x0 + (bottom_x1 - bottom_x0) * t
        for i in range(steps + 1):
            y = top_y + i
            x = int(round(x_top + (x_bottom - x_top) * (i / steps)))
            canvas.set(x, y, near)
            canvas.set(x + 1, y, far)


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
        lattice_window(cv, wx, 146, wx + 38, 172)
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
    grade_toward_light(cv)
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
    grade_toward_light(cv)
    cv.outline(INK)
    return cv


def building_gate():
    """南门 96×96（3×3 格）：门楼瓦顶 + 额枋匾额 + **真门洞**。

    门洞必须真是洞。`building_kit.OPENINGS` 声明 (16,1) 是人穿过去的那一格（它是
    `data/maps.lua` 山庄↔云泽村出口 (16,1) 所在），而那一格在图上（图像行 32..64、
    列 32..64）的不透明占比必须 ≤ 0.35。旧版把「山庄」匾画在行 38..60，正好糊在通道口上
    （实测 0.94）：玩家看到的是一扇关着的门，走不过去却又被画在人前面。所以匾额挂在
    **额枋**（行 18..30，全在通道格之上），通道口只留一道檐内阴影。
    """
    cv = Canvas(96, 96)
    # 屋顶：正脊 + 起翘的鸱吻，檐口压到行 15（额枋以上）
    rows = roof_rows(4, 13, 14, 82, 0, 95)
    tiled_roof(cv, rows, column=12)
    cv.rect(10, 0, 86, 5, TILE_RIDGE)
    cv.hline(10, 86, 0, TILE_ROOF_LIGHT)
    cv.rect(2, 0, 14, 10, TILE_ROOF_DARK)
    cv.rect(82, 0, 94, 10, TILE_ROOF_DARK)
    cv.hline(0, 95, 14, TILE_ROOF_DARK)
    cv.hline(0, 95, 15, shade(TILE_ROOF_DARK, 0.8))
    # 额枋 + 匾额「山庄」（行 16..31，全在通道格 32..63 之外）
    cv.rect(6, 16, 89, 31, WOOD_MID)
    cv.hline(6, 89, 16, WOOD_LIGHT)
    cv.hline(6, 89, 31, WOOD_DARK)
    draw_plaque(cv, 24, 16, 72, 31, "山庄", size=12)
    # 檐内阴影：通道顶上一道，读成"门里面有进深"，只吃掉通道格 12% 的面积
    cv.rect(20, 32, 76, 35, shade(INK, 1.6))
    # 立柱
    wood_post(cv, 6, 32, 20, 87)
    wood_post(cv, 76, 32, 90, 87)
    # 灯笼挂在两柱内侧（列 22..30 / 66..74，都在通道格 32..63 之外）
    for cx in (22, 66):
        cv.vline(cx + 4, 32, 35, WOOD_DARK)
        cv.rect(cx, 35, cx + 8, 46, CINNABAR)
        cv.hline(cx, cx + 8, 35, GOLD)
        cv.hline(cx, cx + 8, 46, GOLD)
    # 门槛 + 台基（行 84 以下是 y=0 那一格，不是通道格）
    cv.rect(20, 84, 76, 87, STONE_DARK)
    cv.hline(20, 76, 84, STONE)
    cv.rect(14, 88, 82, 95, STONE)
    cv.hline(14, 82, 88, STONE_LIGHT)
    cv.hline(14, 82, 95, STONE_MORTAR)
    grade_toward_light(cv)
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

# 拆层（前景遮断）的登记表与切块机器自 M72-11 起住在 `layer_kit.py`：那张表从一开始就是
# **全项目的表**（键是整图 stem、值带目录，`build_*` 与村庄的屋宇同在一张表里），继续留在
# 山庄的美术生成器里，后面六张图就会往"manor"这个文件名里塞别人的资产。这里只 import。
MANOR_STEMS = layers.stems_of("gen_manor_art")


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
    for name, sprite in buildings().items():
        write_png(os.path.join(BUILDING_DIR, f"manor_{name}.png"), sprite)
    for name, sprite in props().items():
        write_png(os.path.join(PROP_DIR, f"manor_{name}.png"), sprite)

    all_sprites = dict(buildings())
    all_sprites.update(props())
    write_png(os.path.join(BUILDING_DIR, "_preview.png"), build_prop_preview(all_sprites))
    splits = write_splits(MANOR_STEMS)

    if not quiet:
        for name, sprite in sorted(all_sprites.items()):
            print(f"sprite   manor_{name}.png {sprite.w}x{sprite.h}")
        for stem, path, rows in splits:
            print(f"split    {path} {rows} 行（{stem} 按行切出）")
        print(f"preview  {BUILDING_DIR}/_preview.png")


def check():
    issues = []
    # 农田两张 overlay 不在这里对拍：M72-08 起由 `gen_farm_soil.py`  own（逐字节比，比尺寸
    # 等式更强），本工具只出建筑与道具。
    for name in buildings():
        path = os.path.join(BUILDING_DIR, f"manor_{name}.png")
        if not os.path.exists(path):
            issues.append("missing " + path)
    for name in props():
        path = os.path.join(PROP_DIR, f"manor_{name}.png")
        if not os.path.exists(path):
            issues.append("missing " + path)
    issues.extend(check_splits())
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
