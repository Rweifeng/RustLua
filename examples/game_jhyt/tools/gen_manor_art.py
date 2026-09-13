# -*- coding: utf-8 -*-
"""《江湖有田》山庄主场景美术生成：地表 tileset + 建筑/道具 sprite。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/gen_manor_art.py            # 生成全部（tileset + 建筑 + 预览）
    python tools/gen_manor_art.py --check    # 只校验已生成资源

产出：

    assets/art/tilesets/manor/manor_ground.png   6 列 × 32px（地表，1..6 列语义见下）
    assets/art/tilesets/manor/manor_farm.png     6 列 × 32px（农田 overlay）
    assets/art/tilesets/manor/_preview.png       两套 tileset 放大预览（带标签）
    assets/art/buildings/manor_house.png         正房 224×224（7×7 格）
    assets/art/buildings/manor_shed.png          柴房 160×128（5×4 格）
    assets/art/buildings/manor_gate.png          南门 96×96（3×3 格）
    assets/art/buildings/_preview.png            建筑/道具放大预览（带标签）
    assets/art/props/manor_*.png                 水井/水缸/草堆/晒谷架/木牌/竹篱/竹丛/老树

引擎硬约束（写死在本工具里，改前先改引擎）：

  * `core/tilemap.rs` 的 `TILE_PALETTE` 只有 **6 项**：tile 值 1..6 对应 tileset 的第 1..6 列，
    UV-U = (n-1)/6..n/6，UV-V 恒为 0..1。**所以 tileset 必须正好 6 列、高度 32**，
    第 7 列永远采不到，高度不是 32 会被拉伸。
  * `scripts/systems/farm.lua` 判定"可开垦"用的是 **GroundMap tile == 2（夯土）**，
    且 overlay 列语义固定：1=已垦干土 2=已垦湿土 3=苗期 4=生长期 5=成熟。
    这两条不能改，否则农田玩法直接坏掉。
  * 建筑/道具的碰撞来自 GroundMap 的 solid 位（不是 sprite），
    所以"看起来实心"的 sprite 必须在 scene 的 solid 串里标 1。

依赖：Python 3 标准库 + 同目录 pixel_kit.py。
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

TILE = 32
TILE_COLUMNS = 6  # 引擎 TILE_PALETTE 长度，不可改

# ---------------------------------------------------------------------------
# 色板（与 docs/35 的基础色同源，白天山庄色调）
# ---------------------------------------------------------------------------

INK = rgb("#1F2328")          # 统一描边
PAPER = rgb("#F2EAD8")        # paper/50
PAPER_DIM = rgb("#D8CDB2")    # paper/200
GOLD = rgb("#C9A05A")         # gold/500
GOLD_LIGHT = rgb("#E4C078")   # gold/300
CINNABAR = rgb("#C3452F")     # cinnabar/500

GRASS = rgb("#5E7C46")
GRASS_DARK = rgb("#4A6636")
GRASS_LIGHT = rgb("#6F8F52")
GRASS_HI = rgb("#7FA35E")

DIRT = rgb("#7C5C3E")
DIRT_DARK = rgb("#63482F")
DIRT_LIGHT = rgb("#8F6E4A")
DIRT_WET = rgb("#4E3A26")

STONE = rgb("#8E8B80")
STONE_LIGHT = rgb("#A5A197")
STONE_DARK = rgb("#6E6B62")
STONE_MORTAR = rgb("#5C5A52")

PLASTER = rgb("#C9C1B0")
PLASTER_DARK = rgb("#AFA795")

TILE_ROOF = rgb("#3E4A52")
TILE_ROOF_LIGHT = rgb("#55626B")
TILE_ROOF_DARK = rgb("#2C363C")
TILE_RIDGE = rgb("#4A5560")

WOOD = rgb("#8A6A46")
WOOD_LIGHT = rgb("#A6855C")
WOOD_MID = rgb("#6B4F3A")
WOOD_DARK = rgb("#4A3628")

WATER_DEEP = rgb("#2F4E6B")
WATER = rgb("#3E6485")
WATER_LIGHT = rgb("#5D82A0")
WATER_HI = rgb("#7FA3D4")

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
# 地表 tileset（6 列 × 32px）
# ---------------------------------------------------------------------------


def tile_grass(seed=11):
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, GRASS)
    rnd = hash_stream(seed)
    for _ in range(26):
        cv.set(rnd(TILE), rnd(TILE), GRASS_DARK)
    for _ in range(18):
        cv.set(rnd(TILE), rnd(TILE), GRASS_LIGHT)
    # 草簇：3px 的 v 形
    for _ in range(4):
        x = 3 + rnd(TILE - 8)
        y = 3 + rnd(TILE - 8)
        cv.set(x, y, GRASS_HI)
        cv.set(x - 1, y + 1, GRASS_LIGHT)
        cv.set(x + 1, y + 1, GRASS_LIGHT)
    # 零星小花（米白/旧金），低频避免重复时扎眼
    if rnd(2) == 0:
        x = 6 + rnd(TILE - 12)
        y = 6 + rnd(TILE - 12)
        cv.set(x, y, PAPER)
        cv.set(x + 1, y, GOLD_LIGHT)
    return cv


def tile_dirt(seed=22):
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, DIRT)
    rnd = hash_stream(seed)
    for _ in range(30):
        cv.set(rnd(TILE), rnd(TILE), DIRT_DARK)
    for _ in range(20):
        cv.set(rnd(TILE), rnd(TILE), DIRT_LIGHT)
    # 土块：2×1 暗底 + 亮顶（大块夯土整片重复时要靠这个撑住质感）
    for _ in range(9):
        x = 2 + rnd(TILE - 6)
        y = 2 + rnd(TILE - 6)
        cv.rect(x, y, x + 2, y + 1, DIRT_DARK)
        cv.hline(x, x + 2, y - 1, DIRT_LIGHT)
    for _ in range(6):
        x = 1 + rnd(TILE - 3)
        y = 1 + rnd(TILE - 3)
        cv.set(x, y, STONE_MORTAR)
        cv.set(x + 1, y, DIRT_LIGHT)
    return cv


def tile_stone(seed=33):
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, STONE)
    tones = [STONE, STONE_LIGHT, STONE, shade(STONE, 0.94)]
    index = 0
    for sy in (0, 16):
        for sx in (0, 16):
            cv.rect(sx + 1, sy + 1, sx + 14, sy + 14, tones[index % len(tones)])
            index += 1
    # 石板缝
    cv.vline(0, 0, TILE - 1, STONE_MORTAR)
    cv.vline(16, 0, TILE - 1, STONE_MORTAR)
    cv.hline(0, TILE - 1, 0, STONE_MORTAR)
    cv.hline(0, TILE - 1, 16, STONE_MORTAR)
    rnd = hash_stream(seed)
    for _ in range(16):
        cv.set(rnd(TILE), rnd(TILE), STONE_DARK)
    for _ in range(10):
        cv.set(rnd(TILE), rnd(TILE), STONE_LIGHT)
    return cv


def tile_wall(seed=44):
    """院墙：薄压顶瓦 + 错缝石砌墙身，方向通用（做地图包围边）。

    早期版本用"白灰墙身 + 大压顶"，同一列逐格重复时会形成刺眼的横向亮带；
    现在压顶只占 4px、墙身用错缝石砌 + 噪点，四个方向的边界都不会"闪条纹"。
    """
    cv = Canvas(TILE, TILE)
    # 薄压顶
    cv.rect(0, 0, TILE - 1, 1, shade(TILE_ROOF_DARK, 0.85))
    cv.rect(0, 2, TILE - 1, 5, TILE_ROOF)
    cv.hline(0, TILE - 1, 3, TILE_ROOF_LIGHT)
    cv.hline(0, TILE - 1, 6, shade(TILE_ROOF_DARK, 0.9))
    # 错缝石砌墙身
    cv.rect(0, 7, TILE - 1, 27, STONE_DARK)
    for y in (7, 15, 23):
        cv.hline(0, TILE - 1, y, STONE_MORTAR)
    for index, y in enumerate((8, 16, 24)):
        offset = 0 if index % 2 == 0 else 8
        for x in range(offset, TILE, 16):
            cv.vline(x, y, min(y + 6, 27), STONE_MORTAR)
    rnd = hash_stream(seed)
    for _ in range(26):
        cv.set(rnd(TILE), 8 + rnd(19), shade(STONE_DARK, 0.88))
    for _ in range(14):
        cv.set(rnd(TILE), 8 + rnd(19), STONE)
    # 墙脚阴影
    cv.rect(0, 28, TILE - 1, TILE - 1, shade(STONE_DARK, 0.72))
    return cv


def tile_roof(seed=55):
    """青瓦屋面（第 5 列）：保留与 village 一致的"瓦顶"语义。"""
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, TILE_ROOF)
    for x in range(0, TILE, 4):
        cv.vline(x, 0, TILE - 1, TILE_ROOF_LIGHT)
        cv.vline(x + 1, 0, TILE - 1, TILE_ROOF_DARK)
    for y in (0, 15, 31):
        cv.hline(0, TILE - 1, y, TILE_ROOF_DARK)
    cv.hline(0, TILE - 1, 1, TILE_ROOF_LIGHT)
    speckle(cv, 0, 0, TILE - 1, TILE - 1, shade(TILE_ROOF, 0.9), 20, seed)
    return cv


def tile_water(seed=66):
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, WATER)
    cv.hline(0, TILE - 1, 0, WATER_DEEP)
    # 波纹：横向弧线 + 高光点
    for y in (6, 15, 24):
        for x in range(2, TILE - 2, 8):
            cv.set(x, y, WATER_LIGHT)
            cv.set(x + 1, y - 1, WATER_LIGHT)
            cv.set(x + 2, y, WATER_LIGHT)
            cv.set(x + 3, y + 1, WATER_HI)
    rnd = hash_stream(seed)
    for _ in range(14):
        cv.set(rnd(TILE), 2 + rnd(TILE - 4), WATER_DEEP)
    return cv


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


def ground_tiles():
    return [tile_grass(), tile_dirt(), tile_stone(), tile_wall(), tile_roof(), tile_water()]


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
    assert len(tiles) == TILE_COLUMNS, "tileset 必须正好 6 列（引擎 TILE_PALETTE 长度）"
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


def build_tiles_preview(ground, farm_tile_list, scale=4):
    labels = ["GRASS", "DIRT", "STONE", "WALL", "ROOF", "WATER",
              "TILLED", "WATERED", "SPROUT", "GROWING", "RIPE", "STUBBLE"]
    tile = TILE * scale
    label_w = text_width("WATERED", 1) + 10
    width = label_w + tile + 8
    row_h = tile + 4
    height = 20 + 12 * row_h + 8
    cv = Canvas(width, height)
    cv.rect(0, 0, width - 1, height - 1, rgb("#14171A"))
    draw_text(cv, 4, 4, "MANOR TILESETS 32PX X4", GOLD_LIGHT, 1)
    for index in range(12):
        y = 20 + index * row_h
        draw_text(cv, 4, y + tile // 2 - 2, labels[index], BAMBOO_LIGHT, 1)
        source = ground[index] if index < 6 else farm_tile_list[index - 6]
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
    ground = ground_tiles()
    farm_tile_list = farm_tiles()
    ground_sheet = pack_tileset(ground)
    farm_sheet = pack_tileset(farm_tile_list)
    write_png(os.path.join(TILE_DIR, "manor_ground.png"), ground_sheet)
    write_png(os.path.join(TILE_DIR, "manor_farm.png"), farm_sheet)
    write_png(os.path.join(TILE_DIR, "_preview.png"), build_tiles_preview(ground, farm_tile_list))

    for name, sprite in buildings().items():
        write_png(os.path.join(BUILDING_DIR, f"manor_{name}.png"), sprite)
    for name, sprite in props().items():
        write_png(os.path.join(PROP_DIR, f"manor_{name}.png"), sprite)

    all_sprites = dict(buildings())
    all_sprites.update(props())
    write_png(os.path.join(BUILDING_DIR, "_preview.png"), build_prop_preview(all_sprites))

    if not quiet:
        print(f"tileset  manor_ground.png {ground_sheet.w}x{ground_sheet.h} (6 列)")
        print(f"tileset  manor_farm.png   {farm_sheet.w}x{farm_sheet.h} (6 列)")
        for name, sprite in sorted(all_sprites.items()):
            print(f"sprite   manor_{name}.png {sprite.w}x{sprite.h}")
        print(f"preview  {TILE_DIR}/_preview.png, {BUILDING_DIR}/_preview.png")


def check():
    from pixel_kit import decode_png

    issues = []
    for path, want_w, want_h in (
        (os.path.join(TILE_DIR, "manor_ground.png"), TILE * TILE_COLUMNS, TILE),
        (os.path.join(TILE_DIR, "manor_farm.png"), TILE * TILE_COLUMNS, TILE),
    ):
        if not os.path.exists(path):
            issues.append("missing " + path)
            continue
        width, height, _, _ = decode_png(path)
        if (width, height) != (want_w, want_h):
            issues.append(f"{path}: {width}x{height} != {want_w}x{want_h}（引擎要求正好 6 列 × 32px）")
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
