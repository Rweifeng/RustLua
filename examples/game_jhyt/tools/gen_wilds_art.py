# -*- coding: utf-8 -*-
"""《江湖有田》批次 2 美术：青云山道 + 寒泉洞 + 山道建筑道具 + 采集物。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/gen_wilds_art.py            # 生成全部 + 预览
    python tools/gen_wilds_art.py --check    # 只校验尺寸/存在性

产出：

    assets/art/tilesets/mountain/mountain_ground.png  6 列 × 32px（山道地表）
    assets/art/tilesets/cave/cave_ground.png          6 列 × 32px（洞窟地表）
    assets/art/buildings/wilds_*.png                  山门 / 栈道 / 茶棚 / 营寨 / 断桥
    assets/art/props/wilds_*.png                      松树 / 岩石 / 藤蔓 / 火把 / 路牌 / 营火 / 木箱
    assets/art/gather/wilds_*.png                     8 种采集物（野果丛 / 药草 / 柴薪 / 矿石 / 雪莲 / 芦苇 / 莲蓬 / 毒草）
    assets/art/tilesets/{mountain,cave}/_preview.png  地表 4× 预览
    assets/art/buildings/_wilds_preview.png           建筑道具 2× 预览
    assets/art/gather/_preview.png                    采集物 3× 预览（中文名）

对应 docs/31 §3.1 的"山林 2 套 / 洞窟 1 套"与 §7.1 的采集区域；
对应 docs/30 §7.1 的四种采集区域（村外湿地 / 青云山道 / 青云山腰 / 寒泉洞）。

列语义（每个地图一套自己的 tileset，引擎写死 6 列）：
  mountain：1=林地草地 2=碎石土路 3=石阶 4=岩壁 5=竹林地 6=溪水
  cave    ：1=洞底石地 2=碎石堆 3=暗河水 4=洞壁 5=矿脉 6=冰晶地

文字：路牌 / 山门匾额 / 茶棚幌子 / 营寨旗 都用**中文**（走 tools/cjk_text.py 的 GDI+ 栅格化）。

依赖：Python 3 标准库 + pixel_kit.py + gen_manor_art.py（通用绘制）+ cjk_text.py（中文）。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import Canvas, draw_text, hash_stream, mix, rgb, shade, text_width  # noqa: E402
from gen_manor_art import (  # noqa: E402
    BAMBOO, BAMBOO_DARK, BAMBOO_LIGHT, CINNABAR, GOLD, GOLD_LIGHT, INK, PAPER, PAPER_DIM,
    STONE, STONE_DARK, STONE_LIGHT, STONE_MORTAR, TILE, TILE_ROOF, TILE_ROOF_DARK,
    TILE_ROOF_LIGHT, WATER, WATER_HI, WATER_LIGHT, WOOD, WOOD_DARK, WOOD_LIGHT, WOOD_MID,
    fill_ellipse, roof_rows, tiled_roof, wood_post,
)
from cjk_text import draw_cjk, prewarm  # noqa: E402

# 山道色板
FOREST = rgb("#4E6A3E")
FOREST_DARK = rgb("#3C5430")
FOREST_LIGHT = rgb("#5F7C4A")
GRAVEL = rgb("#8A8072")
GRAVEL_DARK = rgb("#6B6357")
GRAVEL_LIGHT = rgb("#A2988A")
STAIR = rgb("#948C7C")
STAIR_DARK = rgb("#6E675B")
CLIFF = rgb("#5C5750")
CLIFF_DARK = rgb("#403C37")
CLIFF_LIGHT = rgb("#736D64")
ROCK = rgb("#6E6B62")
ROCK_LIGHT = rgb("#8A867C")
ROCK_DARK = rgb("#4E4B44")
PINE = rgb("#3A5230")
PINE_DARK = rgb("#2C3F24")
PINE_LIGHT = rgb("#4C6A3C")
VINE = rgb("#5B7048")

# 洞窟色板
CAVE_FLOOR = rgb("#54514B")
CAVE_FLOOR_DARK = rgb("#403E39")
CAVE_FLOOR_LIGHT = rgb("#66635C")
CAVE_WALL = rgb("#3A3833")
CAVE_WALL_DARK = rgb("#282621")
CAVE_WALL_LIGHT = rgb("#4C4943")
ORE = rgb("#C9A05A")
ORE_LIGHT = rgb("#E4C078")
ICE = rgb("#A9C4D8")
ICE_LIGHT = rgb("#D6E6F2")
DARKWATER = rgb("#2A3E52")
DARKWATER_LIGHT = rgb("#3E6485")

TILE_DIR = os.path.join("assets", "art", "tilesets")
BUILDING_DIR = os.path.join("assets", "art", "buildings")
PROP_DIR = os.path.join("assets", "art", "props")
GATHER_DIR = os.path.join("assets", "art", "gather")


# ---------------------------------------------------------------------------
# 山道地表
# ---------------------------------------------------------------------------


def tile_forest(seed=311):
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, FOREST)
    rnd = hash_stream(seed)
    for _ in range(30):
        cv.set(rnd(TILE), rnd(TILE), FOREST_DARK)
    for _ in range(20):
        cv.set(rnd(TILE), rnd(TILE), FOREST_LIGHT)
    for _ in range(4):
        x = 4 + rnd(TILE - 9)
        y = 4 + rnd(TILE - 9)
        cv.set(x, y, VINE)
        cv.set(x - 1, y + 1, BAMBOO_DARK)
        cv.set(x + 1, y + 1, VINE)
    for _ in range(3):  # 松针落叶
        x = 4 + rnd(TILE - 8)
        y = 4 + rnd(TILE - 8)
        cv.hline(x, x + 2, y, PINE_DARK)
    return cv


def tile_gravel(seed=322):
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, GRAVEL)
    rnd = hash_stream(seed)
    for _ in range(34):
        cv.set(rnd(TILE), rnd(TILE), GRAVEL_DARK)
    for _ in range(24):
        cv.set(rnd(TILE), rnd(TILE), GRAVEL_LIGHT)
    for _ in range(6):  # 碎石块
        x = 2 + rnd(TILE - 7)
        y = 2 + rnd(TILE - 7)
        cv.rect(x, y, x + 2, y + 1, ROCK)
        cv.hline(x, x + 2, y - 1, ROCK_LIGHT)
    return cv


def tile_stairs(seed=333):
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, STAIR)
    for step, y in enumerate(range(0, TILE, 8)):
        cv.rect(0, y, TILE - 1, y + 5, shade(STAIR, 1.08) if step % 2 == 0 else STAIR)
        cv.hline(0, TILE - 1, y + 6, STAIR_DARK)
        cv.hline(0, TILE - 1, y + 7, shade(STAIR, 0.82))
    rnd = hash_stream(seed)
    for _ in range(16):
        cv.set(rnd(TILE), rnd(TILE), STAIR_DARK)
    return cv


def tile_cliff(seed=344):
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, CAVE_WALL if False else CLIFF)
    rnd = hash_stream(seed)
    for _ in range(30):
        cv.set(rnd(TILE), rnd(TILE), CLIFF_DARK)
    for _ in range(18):
        cv.set(rnd(TILE), rnd(TILE), CLIFF_LIGHT)
    for _ in range(4):  # 岩缝
        x = 3 + rnd(TILE - 8)
        y = 3 + rnd(TILE - 10)
        cv.vline(x, y, y + 5 + rnd(4), CLIFF_DARK)
    cv.hline(0, TILE - 1, 0, CLIFF_DARK)
    return cv


def tile_bamboo_floor(seed=355):
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, FOREST_DARK)
    rnd = hash_stream(seed)
    for _ in range(26):
        cv.set(rnd(TILE), rnd(TILE), FOREST)
    for x in range(2, TILE, 9):  # 落竹
        y = 3 + rnd(18)
        cv.vline(x, y, y + 6, BAMBOO)
        cv.set(x, y - 1, BAMBOO_LIGHT)
    for _ in range(3):
        x = 4 + rnd(TILE - 10)
        y = 4 + rnd(TILE - 10)
        cv.set(x, y, BAMBOO_LIGHT)
        cv.set(x + 1, y + 1, BAMBOO)
    return cv


def tile_mountain_stream(seed=366):
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, rgb("#3E6485"))
    for y in (5, 13, 21, 29):
        for x in range(2, TILE - 3, 6):
            cv.set(x, y, WATER_LIGHT)
            cv.set(x + 1, y - 1, rgb("#B8D2F0"))
            cv.set(x + 2, y, WATER_LIGHT)
    rnd = hash_stream(seed)
    for _ in range(18):
        cv.set(rnd(TILE), rnd(TILE), rgb("#2F4E6B"))
    for _ in range(8):
        cv.set(rnd(TILE), rnd(TILE), WATER_HI)
    return cv


# ---------------------------------------------------------------------------
# 洞窟地表
# ---------------------------------------------------------------------------


def tile_cave_floor(seed=411):
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, CAVE_FLOOR)
    rnd = hash_stream(seed)
    for _ in range(30):
        cv.set(rnd(TILE), rnd(TILE), CAVE_FLOOR_DARK)
    for _ in range(18):
        cv.set(rnd(TILE), rnd(TILE), CAVE_FLOOR_LIGHT)
    for _ in range(5):
        x = 3 + rnd(TILE - 8)
        y = 3 + rnd(TILE - 8)
        cv.set(x, y, CAVE_WALL_DARK)
        cv.set(x + 1, y, CAVE_FLOOR_LIGHT)
    return cv


def tile_cave_rubble(seed=422):
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, CAVE_FLOOR_DARK)
    rnd = hash_stream(seed)
    for _ in range(6):
        x = 2 + rnd(TILE - 8)
        y = 2 + rnd(TILE - 8)
        cv.rect(x, y, x + 3, y + 2, ROCK_DARK)
        cv.hline(x, x + 3, y, ROCK)
    for _ in range(24):
        cv.set(rnd(TILE), rnd(TILE), CAVE_FLOOR)
    return cv


def tile_dark_water(seed=433):
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, DARKWATER)
    for y in (6, 16, 26):
        for x in range(1, TILE - 2, 8):
            cv.set(x, y, DARKWATER_LIGHT)
            cv.set(x + 1, y, DARKWATER_LIGHT)
            cv.set(x + 2, y - 1, rgb("#5D82A0"))
    rnd = hash_stream(seed)
    for _ in range(16):
        cv.set(rnd(TILE), rnd(TILE), rgb("#1E2E3E"))
    return cv


def tile_cave_wall(seed=444):
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, CAVE_WALL)
    rnd = hash_stream(seed)
    for _ in range(28):
        cv.set(rnd(TILE), rnd(TILE), CAVE_WALL_DARK)
    for _ in range(16):
        cv.set(rnd(TILE), rnd(TILE), CAVE_WALL_LIGHT)
    for x in range(0, TILE, 11):  # 竖向岩层
        cv.vline(x, 0, TILE - 1, CAVE_WALL_DARK)
        cv.vline(x + 1, 0, TILE - 1, CAVE_WALL_LIGHT)
    cv.hline(0, TILE - 1, 0, rgb("#1E1C18"))
    return cv


def tile_ore_vein(seed=455):
    cv = tile_cave_wall(seed)
    for x, y in ((6, 10), (9, 12), (14, 8), (20, 14), (24, 11), (13, 20), (21, 22)):
        cv.set(x, y, ORE)
        cv.set(x + 1, y, ORE_LIGHT)
        cv.set(x, y + 1, shade(ORE, 0.75))
    cv.set(10, 13, ORE_LIGHT)
    cv.set(22, 15, ORE)
    return cv


def tile_ice(seed=466):
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, ICE)
    for y in range(0, TILE, 7):
        cv.hline(0, TILE - 1, y, ICE_LIGHT)
        cv.hline(0, TILE - 1, y + 1, shade(ICE, 0.9))
    rnd = hash_stream(seed)
    for _ in range(14):
        cv.set(rnd(TILE), rnd(TILE), rgb("#FFFFFF"))
    for _ in range(10):
        cv.set(rnd(TILE), rnd(TILE), shade(ICE, 0.82))
    return cv


def mountain_tiles():
    return [tile_forest(), tile_gravel(), tile_stairs(), tile_cliff(),
            tile_bamboo_floor(), tile_mountain_stream()]


def cave_tiles():
    return [tile_cave_floor(), tile_cave_rubble(), tile_dark_water(), tile_cave_wall(),
            tile_ore_vein(), tile_ice()]


def pack(tiles):
    sheet = Canvas(TILE * len(tiles), TILE)
    for index, tile in enumerate(tiles):
        sheet.blit(tile, index * TILE, 0)
    return sheet


# ---------------------------------------------------------------------------
# 山道建筑（匾额/幌子用中文）
# ---------------------------------------------------------------------------


def build_mountain_gate():
    """山门 128×96：木牌坊 + 匾额「青云山」。"""
    cv = Canvas(128, 96)
    rows = roof_rows(6, 30, 20, 108, 6, 122)
    tiled_roof(cv, rows, column=12)
    cv.rect(16, 2, 112, 12, TILE_ROOF_LIGHT)
    cv.hline(16, 112, 2, TILE_ROOF)
    cv.rect(6, 0, 18, 12, TILE_ROOF_DARK)
    cv.rect(110, 0, 122, 12, TILE_ROOF_DARK)
    cv.rect(2, 28, 125, 34, TILE_ROOF_DARK)
    wood_post(cv, 12, 34, 26, 92)
    wood_post(cv, 102, 34, 116, 92)
    # 匾额：木底 + 中文
    cv.rect(34, 38, 94, 62, WOOD_DARK)
    cv.rect(36, 40, 92, 60, WOOD_MID)
    cv.hline(36, 92, 40, GOLD)
    cv.hline(36, 92, 59, GOLD)
    draw_cjk(cv, 0, 0, "青云山", size=14, color=PAPER, outline=INK) if False else None
    text = _cjk("青云山", 14, PAPER)
    cv.blit(text, 63 - text.w // 2, 51 - text.h // 2)
    cv.rect(4, 92, 124, 95, STONE)
    cv.hline(4, 124, 92, STONE_LIGHT)
    cv.outline(INK)
    return cv


def _cjk(text, size, color, outline=None):
    """取一段中文的字形画布（供本文件的建筑/道具复用）。"""
    from cjk_text import text_canvas

    canvas = text_canvas(text, size=size, mode="pixel", color=color)
    if outline is not None and canvas.w > 1:
        base = Canvas(canvas.w + 2, canvas.h + 2)
        for gy in range(canvas.h):
            for gx in range(canvas.w):
                if canvas.get(gx, gy)[3] > 0:
                    for dx, dy in ((0, 1), (2, 1), (1, 0), (1, 2), (0, 0), (2, 0), (0, 2), (2, 2)):
                        base.set(gx + dx, gy + dy, outline)
        base.blit(canvas, 1, 1)
        return base
    return canvas


def build_boardwalk():
    """栈道 128×64：贴崖木栈道 + 立柱。"""
    cv = Canvas(128, 64)
    cv.rect(0, 10, 127, 26, WOOD)
    for x in range(0, 128, 9):
        cv.vline(x, 10, 26, WOOD_DARK)
    cv.hline(0, 127, 10, WOOD_LIGHT)
    cv.hline(0, 127, 26, WOOD_DARK)
    for x in range(6, 128, 24):  # 立柱伸进崖壁
        wood_post(cv, x, 26, x + 5, 60)
    cv.rect(0, 6, 127, 10, WOOD_MID)
    for x in range(2, 128, 12):  # 护栏
        cv.rect(x, 0, x + 3, 8, WOOD_MID)
        cv.hline(x, x + 3, 0, WOOD_LIGHT)
    cv.outline(INK)
    return cv


def build_tea_shed():
    """山道茶棚 128×80：茅草顶 + 幌子「茶」。"""
    cv = Canvas(128, 80)
    rows = roof_rows(6, 34, 18, 110, 4, 124)
    for y, x0, x1 in rows:
        cv.hline(x0, x1, y, rgb("#B99A5E") if (y // 3) % 2 == 0 else rgb("#96793F"))
    for x in range(8, 122, 8):
        cv.vline(x, 10 + (x % 6), 32, rgb("#96793F"))
    cv.hline(2, 125, 34, rgb("#96793F"))
    wood_post(cv, 10, 34, 20, 76)
    wood_post(cv, 108, 34, 118, 76)
    cv.rect(20, 40, 108, 66, mix(WOOD_DARK, INK, 0.3))
    cv.rect(30, 46, 70, 60, WOOD)          # 茶案
    cv.hline(30, 70, 46, WOOD_LIGHT)
    for x, color in ((36, PAPER), (48, PAPER_DIM), (60, BAMBOO)):
        cv.rect(x, 40, x + 8, 46, color)   # 茶碗
    # 幌子「茶」
    cv.vline(96, 36, 46, WOOD_DARK)
    cv.rect(86, 42, 106, 62, BAMBOO)
    cv.hline(86, 106, 42, BAMBOO_LIGHT)
    tea = _cjk("茶", 14, PAPER)
    cv.blit(tea, 96 - tea.w // 2, 52 - tea.h // 2)
    cv.outline(INK)
    return cv


def build_bandit_camp():
    """山贼营寨 160×96：木栅 + 帐篷 + 旗「寨」。"""
    cv = Canvas(160, 96)
    # 木栅
    for x in range(4, 156, 12):
        wood_post(cv, x, 30, x + 5, 84)
    cv.rect(4, 44, 155, 48, WOOD_MID)
    cv.rect(4, 64, 155, 68, WOOD_MID)
    # 帐篷
    for index, (cx, w) in enumerate(((44, 44), (104, 40))):
        rows = roof_rows(18, 62, cx - 6, cx + 6, cx - w // 2, cx + w // 2)
        for y, x0, x1 in rows:
            cv.hline(x0, x1, y, rgb("#7C6A52") if (y // 4 + index) % 2 else rgb("#5F5040"))
        cv.rect(cx - 6, 48, cx + 6, 62, mix(WOOD_DARK, INK, 0.45))
    # 旗「寨」
    wood_post(cv, 76, 10, 81, 84)
    cv.rect(82, 14, 118, 40, CINNABAR)
    cv.hline(82, 118, 14, GOLD)
    cv.hline(82, 118, 39, GOLD)
    flag = _cjk("寨", 16, PAPER)
    cv.blit(flag, 100 - flag.w // 2, 27 - flag.h // 2)
    cv.outline(INK)
    return cv


def build_broken_bridge():
    """断桥 128×48：塌了半边的木桥。"""
    cv = Canvas(128, 48)
    cv.rect(0, 14, 78, 26, WOOD)
    for x in range(0, 78, 8):
        cv.vline(x, 14, 26, WOOD_DARK)
    cv.hline(0, 78, 14, WOOD_LIGHT)
    # 断口
    cv.set(78, 14, WOOD_DARK)
    for step in range(10):
        cv.set(78 + step, 18 + step, WOOD_MID)
        cv.set(79 + step, 19 + step, WOOD_DARK)
    # 残余横板
    cv.rect(96, 20, 126, 26, WOOD_MID)
    cv.hline(96, 126, 20, WOOD_LIGHT)
    cv.rect(96, 26, 126, 30, WOOD_DARK)
    for x in (20, 56, 104):
        wood_post(cv, x, 26, x + 4, 44)
    cv.outline(INK)
    return cv


# ---------------------------------------------------------------------------
# 山道道具
# ---------------------------------------------------------------------------


def prop_pine():
    """松树 64×88。"""
    cv = Canvas(64, 88)
    wood_post(cv, 29, 56, 36, 87, light=WOOD_LIGHT, base=WOOD_MID, dark=WOOD_DARK)
    for index, (cy, rx, ry) in enumerate(((22, 22, 13), (38, 26, 12), (54, 22, 10))):
        fill_ellipse(cv, 32, cy, rx, ry, PINE if index % 2 == 0 else PINE_DARK)
        fill_ellipse(cv, 26, cy - 4, rx * 0.5, ry * 0.45, PINE_LIGHT)
    rnd = hash_stream(505)
    for _ in range(40):
        x = rnd(64)
        y = rnd(70)
        if cv.get(x, y)[:3] in (PINE[:3], PINE_DARK[:3]):
            cv.set(x, y, PINE_LIGHT if rnd(2) else PINE_DARK)
    cv.outline(INK)
    return cv


def prop_boulder():
    """山石 48×32（比村庄那块更锐）。"""
    cv = Canvas(48, 32)
    fill_ellipse(cv, 18, 20, 14, 10, ROCK)
    fill_ellipse(cv, 34, 22, 11, 8, ROCK_DARK)
    fill_ellipse(cv, 24, 12, 10, 7, ROCK_LIGHT)
    cv.hline(6, 30, 10, ROCK_LIGHT)
    for x, y in ((12, 22), (20, 24), (30, 24)):
        cv.set(x, y, PINE_DARK)
        cv.set(x + 1, y, VINE)
    cv.outline(INK)
    return cv


def prop_vine():
    """藤蔓 32×64（挂崖）。"""
    cv = Canvas(32, 64)
    for x in (10, 20):
        cv.vline(x, 0, 62, VINE)
        for y in range(4, 62, 8):
            cv.set(x + (1 if (y // 8) % 2 else -1), y, BAMBOO_LIGHT)
            cv.set(x + (2 if (y // 8) % 2 else -2), y + 1, VINE)
    for x, y in ((6, 8), (26, 18), (8, 34), (24, 46), (14, 56)):
        cv.set(x, y, BAMBOO_LIGHT)
    cv.outline(INK)
    return cv


def prop_torch():
    """崖壁火把 24×56。"""
    cv = Canvas(24, 56)
    wood_post(cv, 10, 18, 14, 55)
    cv.rect(8, 12, 16, 20, STONE_DARK)
    fill_ellipse(cv, 12, 8, 5, 6, rgb("#E07B3A"))
    fill_ellipse(cv, 12, 6, 3, 4, rgb("#F2C05A"))
    cv.set(12, 2, rgb("#F2EAD8"))
    cv.outline(INK)
    return cv


def prop_signpost():
    """路牌 40×48：木牌 + 中文「山道」。"""
    cv = Canvas(40, 48)
    wood_post(cv, 18, 20, 24, 47)
    cv.rect(2, 4, 38, 26, WOOD_DARK)
    cv.rect(4, 6, 36, 24, WOOD_MID)
    cv.hline(4, 36, 6, GOLD)
    cv.hline(4, 36, 23, GOLD)
    text = _cjk("山道", 13, PAPER)
    cv.blit(text, 20 - text.w // 2, 15 - text.h // 2)
    cv.outline(INK)
    return cv


def prop_campfire():
    """营火 48×40。"""
    cv = Canvas(48, 40)
    for index, (x, y) in enumerate(((12, 30), (24, 32), (36, 30))):
        cv.rect(x - 6, y, x + 6, y + 3, WOOD if index % 2 else WOOD_MID)
        cv.hline(x - 6, x + 6, y, WOOD_LIGHT)
    fill_ellipse(cv, 24, 24, 9, 10, rgb("#E07B3A"))
    fill_ellipse(cv, 24, 21, 6, 7, rgb("#F2C05A"))
    fill_ellipse(cv, 24, 18, 3, 4, rgb("#F2EAD8"))
    cv.outline(INK)
    return cv


def prop_crate():
    """木箱 32×28。"""
    cv = Canvas(32, 28)
    cv.rect(2, 8, 29, 26, WOOD)
    cv.rect(2, 8, 29, 11, WOOD_LIGHT)
    cv.hline(2, 29, 26, WOOD_DARK)
    cv.vline(2, 8, 26, WOOD_LIGHT)
    cv.vline(29, 8, 26, WOOD_DARK)
    cv.hline(3, 28, 15, WOOD_DARK)
    cv.vline(15, 12, 25, WOOD_DARK)
    cv.outline(INK)
    return cv


# ---------------------------------------------------------------------------
# 采集物（8 种）
# ---------------------------------------------------------------------------


def gather_berry():
    cv = Canvas(32, 32)
    fill_ellipse(cv, 16, 22, 11, 8, FOREST_DARK)
    for x, y in ((10, 20), (16, 17), (22, 21), (13, 25), (20, 26)):
        cv.set(x, y, CINNABAR)
        cv.set(x + 1, y, CINNABAR)
        cv.set(x, y + 1, shade(CINNABAR, 0.8))
        cv.set(x, y - 1, rgb("#E06B50"))
    for x, y in ((8, 14), (16, 10), (24, 14)):
        cv.set(x, y, BAMBOO)
        cv.set(x + 1, y - 1, BAMBOO_LIGHT)
    cv.outline(INK)
    return cv


def gather_herb_cluster():
    cv = Canvas(32, 32)
    for x, top in ((10, 12), (16, 8), (22, 13)):
        cv.vline(x, top, 28, BAMBOO_DARK)
        for side in (-1, 1):
            for step in range(3):
                cv.set(x + side * (2 + step), top + 3 + step * 3, BAMBOO if side > 0 else BAMBOO_LIGHT)
    cv.hline(8, 24, 28, BAMBOO_DARK)
    cv.outline(INK)
    return cv


def gather_firewood():
    cv = Canvas(40, 32)
    for index, y in enumerate((18, 24, 30)):
        cv.rect(4, y - 4, 36, y, WOOD if index % 2 == 0 else WOOD_MID)
        cv.hline(4, 36, y - 4, WOOD_LIGHT)
        cv.hline(4, 36, y, WOOD_DARK)
    cv.outline(INK)
    return cv


def gather_ore_rock():
    cv = Canvas(40, 32)
    fill_ellipse(cv, 20, 20, 15, 10, ROCK)
    fill_ellipse(cv, 16, 16, 8, 5, ROCK_LIGHT)
    for x, y in ((12, 20), (18, 24), (26, 18), (23, 25)):
        cv.set(x, y, ORE)
        cv.set(x + 1, y, ORE_LIGHT)
    cv.outline(INK)
    return cv


def gather_snow_lotus():
    cv = Canvas(32, 32)
    cv.rect(4, 24, 28, 30, ICE)
    cv.hline(4, 28, 24, ICE_LIGHT)
    for angle_x in range(-2, 3):
        fill_ellipse(cv, 16 + angle_x * 4, 18, 4, 6, PAPER_DIM)
    fill_ellipse(cv, 16, 16, 5, 5, PAPER)
    fill_ellipse(cv, 16, 16, 2, 2, BAMBOO_LIGHT)
    cv.outline(rgb("#5E7A8C"))
    return cv


def gather_reed_bed():
    cv = Canvas(32, 40)
    for x, top in ((10, 6), (16, 10), (22, 4)):
        cv.vline(x, top, 38, rgb("#9A8B5A"))
        fill_ellipse(cv, x, top + 3, 3, 5, rgb("#C9B47A"))
        cv.set(x, top - 2, rgb("#E0D0A0"))
    cv.hline(6, 26, 38, BAMBOO_DARK)
    cv.outline(INK)
    return cv


def gather_lotus_pod():
    cv = Canvas(32, 32)
    cv.vline(16, 20, 30, BAMBOO_DARK)
    fill_ellipse(cv, 16, 15, 9, 7, BAMBOO)
    cv.hline(8, 24, 10, BAMBOO_LIGHT)
    for x, y in ((12, 13), (16, 11), (20, 13), (14, 17), (18, 17)):
        cv.set(x, y, BAMBOO_DARK)
        cv.set(x, y - 1, rgb("#3E5230"))
    cv.outline(INK)
    return cv


def gather_poison_herb():
    cv = Canvas(32, 32)
    cv.vline(16, 14, 29, rgb("#4A3A52"))
    for x, y in ((10, 20), (22, 20), (16, 12)):
        fill_ellipse(cv, x, y, 5, 4, rgb("#6B4A78"))
        cv.set(x, y - 2, rgb("#9A76AC"))
    for x, y in ((12, 12), (20, 12)):
        cv.set(x, y, rgb("#C9B47A"))
    cv.outline(INK)
    return cv


# ---------------------------------------------------------------------------
# 输出
# ---------------------------------------------------------------------------

BUILDINGS = [
    ("wilds_mountain_gate", build_mountain_gate, "山门 青云山"),
    ("wilds_boardwalk", build_boardwalk, "栈道"),
    ("wilds_tea_shed", build_tea_shed, "山道茶棚"),
    ("wilds_bandit_camp", build_bandit_camp, "山贼营寨"),
    ("wilds_broken_bridge", build_broken_bridge, "断桥"),
]

PROPS = [
    ("wilds_pine", prop_pine, "松树"),
    ("wilds_boulder", prop_boulder, "山石"),
    ("wilds_vine", prop_vine, "藤蔓"),
    ("wilds_torch", prop_torch, "火把"),
    ("wilds_signpost", prop_signpost, "路牌 山道"),
    ("wilds_campfire", prop_campfire, "营火"),
    ("wilds_crate", prop_crate, "木箱"),
]

GATHERS = [
    ("wilds_berry", gather_berry, "野果丛"),
    ("wilds_herb", gather_herb_cluster, "药草丛"),
    ("wilds_firewood", gather_firewood, "柴薪堆"),
    ("wilds_ore", gather_ore_rock, "矿石堆"),
    ("wilds_snow_lotus", gather_snow_lotus, "雪莲"),
    ("wilds_reed", gather_reed_bed, "芦苇丛"),
    ("wilds_lotus", gather_lotus_pod, "莲蓬"),
    ("wilds_poison_herb", gather_poison_herb, "毒草丛"),
]


def write_png(path, canvas):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(canvas.to_png())


def build_tiles_preview(tiles, title, labels, scale=4):
    tile = TILE * scale
    label_w = text_width("WHITEWASH_WALL", 1) + 10
    cv = Canvas(label_w + tile + 8, 20 + len(tiles) * (tile + 4) + 8)
    cv.rect(0, 0, cv.w - 1, cv.h - 1, rgb("#14171A"))
    draw_text(cv, 4, 4, title, GOLD_LIGHT, 1)
    for index, tile_canvas in enumerate(tiles):
        y = 20 + index * (tile + 4)
        draw_text(cv, 4, y + tile // 2 - 2, labels[index], BAMBOO_LIGHT, 1)
        cv.rect(label_w - 1, y - 1, label_w + tile, y + tile, rgb("#262C33"))
        cv.blit(tile_canvas.scaled(scale), label_w, y)
    return cv


def build_sprite_preview(entries, title, scale=2, cols=4):
    items = sorted(entries, key=lambda item: item[0])
    pad = 10
    label_h = 14
    cell_w = max(canvas.w for _, canvas, _ in items) * scale + pad * 2
    cell_h = max(canvas.h for _, canvas, _ in items) * scale + pad * 2 + label_h
    rows = (len(items) + cols - 1) // cols
    cv = Canvas(cell_w * cols + 8, 22 + cell_h * rows + 8)
    cv.rect(0, 0, cv.w - 1, cv.h - 1, rgb("#14171A"))
    draw_text(cv, 6, 6, title, GOLD_LIGHT, 1)
    for index, (name, canvas, display) in enumerate(items):
        row, col = divmod(index, cols)
        x0 = 4 + col * cell_w
        y0 = 22 + row * cell_h
        draw_text(cv, x0 + pad, y0, name.replace("wilds_", ""), BAMBOO_LIGHT, 1)
        draw_cjk(cv, x0 + pad, y0 + 8, display, size=11, color=PAPER_DIM) if display else None
        top = y0 + label_h
        for by in range(0, canvas.h * scale, 8):
            for bx in range(0, canvas.w * scale, 8):
                tone = rgb("#262C33") if ((bx // 8 + by // 8) % 2 == 0) else rgb("#31383F")
                cv.rect(x0 + pad + bx, top + by, x0 + pad + bx + 7, top + by + 7, tone)
        cv.blit(canvas.scaled(scale), x0 + pad, top)
    return cv


def generate(quiet=False):
    # 中文先一次性渲染（GDI+ 批量调用，避免每段文字一次 PowerShell）
    prewarm([("青云山", 14, "pixel", True), ("茶", 14, "pixel", True),
             ("寨", 16, "pixel", True), ("山道", 13, "pixel", True)] +
            [(display, 11, "pixel", True) for _, _, display in BUILDINGS + PROPS + GATHERS])
    mountain = mountain_tiles()
    cave = cave_tiles()
    write_png(os.path.join(TILE_DIR, "mountain", "mountain_ground.png"), pack(mountain))
    write_png(os.path.join(TILE_DIR, "cave", "cave_ground.png"), pack(cave))
    write_png(os.path.join(TILE_DIR, "mountain", "_preview.png"),
              build_tiles_preview(mountain, "MOUNTAIN PATH 32PX X4",
                                  ["FOREST", "GRAVEL", "STAIRS", "CLIFF", "BAMBOO", "STREAM"]))
    write_png(os.path.join(TILE_DIR, "cave", "_preview.png"),
              build_tiles_preview(cave, "COLD SPRING CAVE 32PX X4",
                                  ["FLOOR", "RUBBLE", "DARKWATER", "WALL", "ORE VEIN", "ICE"]))

    built = []
    for name, factory, display in BUILDINGS:
        canvas = factory()
        built.append((name, canvas, display))
        write_png(os.path.join(BUILDING_DIR, name + ".png"), canvas)
    props = []
    for name, factory, display in PROPS:
        canvas = factory()
        props.append((name, canvas, display))
        write_png(os.path.join(PROP_DIR, name + ".png"), canvas)
    gathers = []
    for name, factory, display in GATHERS:
        canvas = factory()
        gathers.append((name, canvas, display))
        write_png(os.path.join(GATHER_DIR, name + ".png"), canvas)

    write_png(os.path.join(BUILDING_DIR, "_wilds_preview.png"),
              build_sprite_preview(built + props, "MOUNTAIN BUILDINGS + PROPS X2"))
    write_png(os.path.join(GATHER_DIR, "_preview.png"),
              build_sprite_preview(gathers, "GATHERABLES X3", scale=3, cols=4))
    if not quiet:
        for name, canvas, display in built + props + gathers:
            print(f"  {name}.png {canvas.w}x{canvas.h}  {display}")
        print("  2 tilesets + 3 previews")


EXPECTED = {
    "wilds_mountain_gate": (128, 96),
    "wilds_boardwalk": (128, 64),
    "wilds_tea_shed": (128, 80),
    "wilds_bandit_camp": (160, 96),
    "wilds_broken_bridge": (128, 48),
    "wilds_pine": (64, 88),
    "wilds_boulder": (48, 32),
    "wilds_vine": (32, 64),
    "wilds_torch": (24, 56),
    "wilds_signpost": (40, 48),
    "wilds_campfire": (48, 40),
    "wilds_crate": (32, 28),
    "wilds_berry": (32, 32),
    "wilds_herb": (32, 32),
    "wilds_firewood": (40, 32),
    "wilds_ore": (40, 32),
    "wilds_snow_lotus": (32, 32),
    "wilds_reed": (32, 40),
    "wilds_lotus": (32, 32),
    "wilds_poison_herb": (32, 32),
}


def check():
    from pixel_kit import decode_png

    issues = []
    for folder, name in ((TILE_DIR, "mountain/mountain_ground"), (TILE_DIR, "cave/cave_ground")):
        path = os.path.join(folder, name + ".png")
        if not os.path.exists(path):
            issues.append("missing " + path)
            continue
        width, height, _, _ = decode_png(path)
        if (width, height) != (TILE * 6, TILE):
            issues.append(f"{path}: {width}x{height} != 192x32（引擎要求 6 列 × 32px）")
    for name, (want_w, want_h) in EXPECTED.items():
        folder = BUILDING_DIR if name in {n for n, _, _ in BUILDINGS} else (
            PROP_DIR if name in {n for n, _, _ in PROPS} else GATHER_DIR)
        path = os.path.join(folder, name + ".png")
        if not os.path.exists(path):
            issues.append("missing " + path)
            continue
        width, height, _, _ = decode_png(path)
        if (width, height) != (want_w, want_h):
            issues.append(f"{path}: {width}x{height} != {want_w}x{want_h}")
    for path in (os.path.join(TILE_DIR, "mountain", "_preview.png"),
                 os.path.join(TILE_DIR, "cave", "_preview.png"),
                 os.path.join(BUILDING_DIR, "_wilds_preview.png"),
                 os.path.join(GATHER_DIR, "_preview.png")):
        if not os.path.exists(path):
            issues.append("missing " + path)
    return issues


def main():
    parser = argparse.ArgumentParser(description="《江湖有田》批次 2：山道与洞窟美术生成")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        problems = check()
        if problems:
            for problem in problems:
                print("[FAIL]", problem)
            raise SystemExit(1)
        print(f"[OK] 批次 2 校验通过（2 tileset + {len(BUILDINGS)} 建筑 + {len(PROPS)} 道具 + {len(GATHERS)} 采集物）")
        return
    generate()


if __name__ == "__main__":
    main()
