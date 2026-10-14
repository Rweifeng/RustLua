# -*- coding: utf-8 -*-
"""《江湖有田》批次 3 美术：室内家具 + 生产设施 + 可建造建筑（**地表 tileset 已全部不在本文件**）。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/gen_interior_art.py
    python tools/gen_interior_art.py --check

产出：

    （**两张室内地表都不在本文件出图**：M72-11 §推进顺序 5 起，铁匠铺归
      `gen_terrain_tiles.py --scene interior_forge`、药庐归
      `gen_terrain_tiles.py --scene interior_herb`，各出 12×9 图集
      `assets/art/tilesets/<图>/<图>_ground.png`。旧那两套 6 列 × 1 行的
      `interior_ground.png` 与它们的 `_preview.png` 已随本批从盘上下架，`check()`
      对两个目录各有反面对照）
    assets/art/furniture/interior_*.png                     12 件家具（含中文招牌的两件）
    assets/art/facilities/facility_*.png                    5 件生产设施（docs/30 §6.4）
    assets/art/buildings/build_*.png                        5 座可建造建筑 + 地基 + 脚手架
    assets/art/{furniture,facilities,buildings}/*_preview.png

列语义（两张室内图现在都是 12×9 + autotile，本文件一张地表都不出）：
  interior_herb ：本文件**不再出图**。旧说法（1=木地板 2=青砖地 3=席垫 4=白墙 5=木隔断 6=柜台，
            6 列 × 1 行恒等寻址）→ 新实测（1=木地板 2=药渣与切屑(新增族) 3=青砖地 4=木隔断
            5=席垫 6=药柜台，白墙那一列从地表语义里下架）。换序的头一条理由与铁匠铺同源而
            更硬：`building_kit.TILE_NATIVE_SOLID=(4,6)` 按编号说话，而旧档把整圈墙记在
            5=木隔断上 ⇒ 盘上那 60 格"墙"在跨图语义里根本不是墙。逐条理由写在
            `terrain_ramp.json:scenes.interior_herb.comment` 与 `gen_herb_scene.py` 的模块
            docstring。旧那张 6 列 × 1 的图与它的 `_preview.png` 随本批从盘上下架，本文件那
            六格 `tile_plank/tile_brick_floor/tile_mat/tile_white_wall/tile_partition/
            tile_counter` 连着 `build_tiles_preview`/`_pack` 一起摘除。
  interior_forge：同一批下架，旧说法（1=夯土地 2=石板地 3=炭渣地 4=砖墙 5=石墙 6=水槽）
            → 新实测（1=夯土 2=炭渣碎石(零格) 3=石板 4=砖墙 5=耐火砖 6=水槽），逐条
            理由写在 `gen_forge_scene.py` 的模块 docstring。

对应 docs/30 §6.4（药炉/茶灶/酒坊/研磨台/铁砧）与 §11.2（药庐/练功房/仓房/茶棚/铁匠台）。
可建造建筑的施工两态共用「地基」「脚手架」两张图（脚手架是叠加层），
避免 5 座建筑 × 3 态 = 15 张图的重复出图。

文字：药柜「药」、书架「书」、可建造建筑匾额「药庐」「练功房」「仓房」「茶棚」「铁匠台」等，
中文走 tools/cjk_text.py。

依赖：Python 3 标准库 + pixel_kit.py + gen_manor_art.py + cjk_text.py。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import Canvas, draw_text, hash_stream, mix, rgb, shade, text_width  # noqa: E402
from light_kit import grade_toward_light  # noqa: E402  M72-07：明暗只认 lighting.lua 的 sun
from gen_manor_art import (  # noqa: E402
    BAMBOO, BAMBOO_DARK, BAMBOO_LIGHT, CINNABAR, GOLD, GOLD_LIGHT, INK, PAPER, PAPER_DIM,
    PLASTER, PLASTER_DARK, STONE, STONE_DARK, STONE_LIGHT, STONE_MORTAR, TILE, TILE_ROOF,
    TILE_ROOF_DARK, TILE_ROOF_LIGHT, WATER, WOOD, WOOD_DARK, WOOD_LIGHT, WOOD_MID,
    fill_ellipse, lattice_window, roof_rows, tiled_roof, wood_post,
)
from layer_kit import split_stem, stems_of  # noqa: E402  M72-06/11 拆层表（住在 layer_kit）
from cjk_text import draw_cjk, prewarm, text_canvas  # noqa: E402

TILE_DIR = os.path.join("assets", "art", "tilesets")
FURNITURE_DIR = os.path.join("assets", "art", "furniture")
FACILITY_DIR = os.path.join("assets", "art", "facilities")
BUILDING_DIR = os.path.join("assets", "art", "buildings")

# 室内地表那十行颜色（`PLANK*/BRICK*/MAT*/PARTITION/COUNTER`）跟着 6×1 图集一起摘除：
# 它们除了那六格瓦**没有任何别的使用者**（复现：`grep -n "\bPLANK\b\|\bCOUNTER\b" tools/*.py`
# 在摘除前只命中本文件的 `tile_*` 六函数与预览标签串）。现在这些颜色住在
# `tools/terrain_ramp.json:scenes.interior_herb.families` 的色阶档里，其中
# `herb_plank.light #A6855C`、`herb_mat.light #C9B47A`、`herb_partition.light #6B4F3A`、
# `herb_counter.light #8A6A46` 就是旧那四列的平涂色，搬进 12×9 没换颜色。
# `PLASTER`/`PLASTER_DARK` 两行**留下**（从 `gen_manor_art` import）：`build_herb_hut` 与
# `build_training_hall` 的可建造墙体还在用它们，删了等于顺手改了建筑的画——与下面铁匠铺那
# 三行注释是同一条纪律。
FORGE_EARTH = rgb("#6E5A44")
FORGE_EARTH_DARK = rgb("#544434")
CINDER = rgb("#3E3A34")
BRICKWALL = rgb("#7A5A46")
# 铁匠铺地表那六族现在住在 `tools/terrain_ramp.json:scenes.interior_forge.families`。
# 上面四行留下不是因为图集：`furn_stove`/`facility_herb_stove`/`facility_tea_stove` 的炉膛、
# `facility_tea_stove` 与 `build_forge_stand` 的墙体、`build_foundation` 与 `build_storehouse`
# 的夯土还在用同一套颜色，删了它们等于顺手改了家具与建筑的画。
# 只跟着 6×1 图集一起下架的是 `FORGE_SLAB/CINDER_LIGHT/STONEWALL/TROUGH` 四行——
# 它们除了那六格瓦没有别的使用者。


def _cjk(text, size, color, outline=INK):
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


def plaque(cv, x0, y0, x1, y1, text, size=12, board=WOOD_DARK, face=WOOD_MID, ink=PAPER):
    """匾额：木底 + 金边 + 中文（本批所有招牌都走这里，保证风格一致）。"""
    cv.rect(x0, y0, x1, y1, board)
    cv.rect(x0 + 2, y0 + 2, x1 - 2, y1 - 2, face)
    cv.hline(x0 + 2, x1 - 2, y0 + 2, GOLD)
    cv.hline(x0 + 2, x1 - 2, y1 - 2, GOLD)
    glyph = _cjk(text, size, ink)
    cv.blit(glyph, (x0 + x1) // 2 - glyph.w // 2, (y0 + y1) // 2 - glyph.h // 2)


# ---------------------------------------------------------------------------
# 室内地表：**本文件不再出图**（M72-11 §推进顺序 5）。旧的两批各六格——铁匠铺
# `tile_forge_earth/tile_forge_slab/tile_cinder/tile_brick_wall/tile_stone_wall/tile_trough`、
# 药庐 `tile_plank/tile_brick_floor/tile_mat/tile_white_wall/tile_partition/tile_counter`——
# 连着它们的 `_pack(...)` 与 `build_tiles_preview` 一起摘除：金标规范下图集的唯一产出点是
# `gen_terrain_tiles.py --scene <图>`（12×9 + autotile + per-tile tint），这里再画一张 6×1
# 就是把"两套地表真相"留在盘上。`check()` 对两个目录各有反面对照：回盘就报红。
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# 家具
# ---------------------------------------------------------------------------


def furn_medicine_cabinet():
    """药柜 64×64：抽屉墙 + 匾「药」。"""
    cv = Canvas(64, 64)
    cv.rect(2, 10, 61, 62, WOOD_MID)
    for row in range(2):
        for col in range(3):
            x = 6 + col * 18
            y = 30 + row * 16
            cv.rect(x, y, x + 15, y + 13, WOOD)
            cv.hline(x, x + 15, y, WOOD_LIGHT)
            cv.hline(x, x + 15, y + 13, WOOD_DARK)
            cv.set(x + 7, y + 6, GOLD)
    plaque(cv, 10, 14, 54, 28, "药", size=12)
    cv.outline(INK)
    return cv


def furn_herb_mill():
    """药碾 40×28。"""
    cv = Canvas(40, 28)
    cv.rect(2, 14, 37, 25, STONE)
    cv.hline(2, 37, 14, STONE_LIGHT)
    cv.hline(2, 37, 25, STONE_MORTAR)
    fill_ellipse(cv, 20, 12, 11, 5, STONE_DARK)
    fill_ellipse(cv, 20, 11, 9, 4, shade(STONE, 0.9))
    cv.rect(18, 4, 22, 12, WOOD_MID)
    cv.rect(12, 2, 28, 5, WOOD)
    cv.outline(INK)
    return cv


def furn_bed():
    """木床 64×48。"""
    cv = Canvas(64, 48)
    cv.rect(2, 10, 61, 46, WOOD_MID)
    cv.rect(4, 12, 59, 44, PAPER_DIM)
    cv.hline(4, 59, 12, PAPER)
    cv.rect(6, 14, 24, 28, PAPER)          # 枕
    cv.rect(26, 12, 59, 44, rgb("#7C6A52"))  # 被
    for x in range(28, 59, 8):
        cv.vline(x, 13, 43, shade(rgb("#7C6A52"), 0.85))
    cv.rect(2, 44, 61, 46, WOOD_DARK)
    cv.outline(INK)
    return cv


def furn_table():
    """方桌 48×40。"""
    cv = Canvas(48, 40)
    cv.rect(2, 10, 45, 20, WOOD)
    cv.hline(2, 45, 10, WOOD_LIGHT)
    cv.hline(2, 45, 20, WOOD_DARK)
    for x in (6, 38):
        cv.rect(x, 21, x + 4, 38, WOOD_MID)
    cv.outline(INK)
    return cv


def furn_chair():
    """木椅 28×40。"""
    cv = Canvas(28, 40)
    cv.rect(4, 8, 23, 20, WOOD_MID)
    cv.hline(4, 23, 8, WOOD_LIGHT)
    cv.rect(4, 20, 23, 26, WOOD)
    for x in (5, 19):
        cv.rect(x, 26, x + 3, 38, WOOD_MID)
    cv.rect(4, 2, 23, 8, WOOD)
    cv.vline(6, 2, 8, WOOD_LIGHT)
    cv.vline(21, 2, 8, WOOD_DARK)
    cv.outline(INK)
    return cv


def furn_bookshelf():
    """书架 56×72：三层书 + 匾「书」。"""
    cv = Canvas(56, 72)
    cv.rect(2, 8, 53, 70, WOOD_MID)
    for row in range(3):
        y = 30 + row * 13
        cv.rect(4, y, 51, y + 11, shade(WOOD_DARK, 0.85))
        for index, x in enumerate(range(6, 50, 5)):
            height = 8 + (index % 3)
            color = (CINNABAR, BAMBOO, GOLD, rgb("#4A6FA5"))[index % 4]
            cv.rect(x, y + 11 - height, x + 3, y + 10, color)
            cv.hline(x, x + 3, y + 11 - height, shade(color, 1.2))
    plaque(cv, 8, 12, 48, 27, "书", size=12)
    cv.outline(INK)
    return cv


def furn_screen():
    """屏风 56×56。"""
    cv = Canvas(56, 56)
    cv.rect(2, 6, 53, 50, WOOD_DARK)
    cv.rect(5, 9, 50, 47, PAPER)
    for x in range(5, 51, 16):
        cv.vline(x, 9, 47, WOOD_MID)
    cv.rect(12, 16, 28, 30, mix(BAMBOO, PAPER, 0.4))   # 山水意象
    cv.rect(24, 22, 44, 30, mix(BAMBOO_DARK, PAPER, 0.3))
    cv.hline(5, 50, 47, PAPER_DIM)
    cv.rect(2, 50, 53, 53, WOOD_MID)
    cv.outline(INK)
    return cv


def furn_lantern_indoor():
    """室内灯笼 24×44。"""
    cv = Canvas(24, 44)
    cv.vline(12, 2, 10, WOOD_DARK)
    cv.rect(4, 10, 20, 30, CINNABAR)
    cv.hline(4, 20, 10, GOLD)
    cv.hline(4, 20, 30, GOLD)
    cv.vline(12, 10, 30, shade(CINNABAR, 0.8))
    cv.rect(9, 30, 15, 34, GOLD)
    cv.vline(12, 34, 40, CINNABAR)
    cv.outline(INK)
    return cv


def furn_stove():
    """灶台 48×40。"""
    cv = Canvas(48, 40)
    cv.rect(2, 12, 45, 38, STONE_DARK)
    for x in range(4, 44, 6):
        cv.vline(x, 13, 37, STONE_MORTAR)
    cv.rect(6, 6, 22, 14, STONE)          # 灶眼
    cv.rect(9, 8, 19, 13, CINDER)
    fill_ellipse(cv, 14, 10, 4, 3, rgb("#E07B3A"))
    cv.rect(26, 4, 42, 14, STONE_DARK)    # 烟囱
    cv.outline(INK)
    return cv


def furn_shelf_goods():
    """货架 64×56。"""
    cv = Canvas(64, 56)
    cv.rect(2, 8, 61, 54, WOOD_MID)
    for row in range(2):
        y = 24 + row * 15
        cv.rect(4, y, 59, y + 13, shade(WOOD_DARK, 0.85))
        for index, x in enumerate(range(6, 58, 8)):
            color = (GOLD, CINNABAR, BAMBOO, PAPER_DIM)[(index + row) % 4]
            cv.rect(x, y + 4, x + 6, y + 12, color)
            cv.hline(x, x + 6, y + 4, shade(color, 1.15))
    cv.rect(2, 8, 61, 22, WOOD)
    cv.outline(INK)
    return cv


def furn_barrel():
    """木桶 28×34。"""
    cv = Canvas(28, 34)
    for y in range(8, 32):
        t = (y - 8) / 24.0
        half = 8 + int(round(3 * (1 - abs(t - 0.5) * 2)))
        cv.hline(14 - half, 14 + half, y, WOOD if (y // 3) % 2 else WOOD_MID)
    cv.hline(4, 24, 8, WOOD_LIGHT)
    for y in (12, 20, 28):
        cv.hline(5, 23, y, IRON if False else STONE_DARK)
    cv.hline(6, 22, 10, WATER)  # 水面
    cv.outline(INK)
    return cv


def furn_chest():
    """室内木箱 36×30（带铜锁）。"""
    cv = Canvas(36, 30)
    cv.rect(2, 10, 33, 28, WOOD)
    cv.rect(2, 8, 33, 14, WOOD_MID)
    cv.hline(2, 33, 8, WOOD_LIGHT)
    cv.hline(2, 33, 14, WOOD_DARK)
    cv.hline(2, 33, 28, WOOD_DARK)
    cv.rect(15, 14, 20, 19, GOLD)
    cv.set(17, 16, GOLD_LIGHT)
    cv.outline(INK)
    return cv


# ---------------------------------------------------------------------------
# 生产设施（docs/30 §6.4）
# ---------------------------------------------------------------------------


def facility_herb_stove():
    """药炉 64×72：炉身 + 药罐 + 蒸汽。"""
    cv = Canvas(64, 72)
    cv.rect(6, 30, 57, 68, STONE_DARK)
    for x in range(10, 54, 8):
        cv.vline(x, 32, 66, STONE_MORTAR)
    cv.rect(14, 44, 49, 56, CINDER)
    fill_ellipse(cv, 31, 50, 9, 6, rgb("#E07B3A"))
    fill_ellipse(cv, 31, 48, 5, 4, rgb("#F2C05A"))
    cv.rect(20, 18, 44, 32, shade(STONE, 0.9))   # 药罐
    cv.rect(18, 14, 46, 20, STONE)
    cv.hline(18, 46, 14, STONE_LIGHT)
    for x, y in ((26, 10), (32, 6), (38, 9)):    # 蒸汽
        cv.set(x, y, PAPER_DIM)
        cv.set(x + 1, y - 2, PAPER)
    plaque(cv, 8, 58, 56, 68, "药炉", size=11)
    cv.outline(INK)
    return cv


def facility_tea_stove():
    """茶灶 64×64：砖灶 + 铜壶 + 蒸汽。"""
    cv = Canvas(64, 64)
    cv.rect(6, 30, 57, 60, BRICKWALL)
    for row, y in enumerate(range(30, 60, 7)):
        offset = 0 if row % 2 == 0 else 6
        cv.hline(6, 57, y, shade(BRICKWALL, 0.75))
        for x in range(6 + offset, 57, 12):
            cv.vline(x, y, y + 6, shade(BRICKWALL, 0.75))
    cv.rect(16, 40, 47, 52, CINDER)
    fill_ellipse(cv, 31, 46, 8, 5, rgb("#E07B3A"))
    cv.rect(22, 20, 42, 34, GOLD)                # 铜壶
    cv.hline(22, 42, 20, GOLD_LIGHT)
    cv.hline(22, 42, 34, shade(GOLD, 0.75))
    cv.rect(42, 22, 50, 26, GOLD)                # 壶嘴
    cv.rect(26, 14, 38, 20, GOLD_LIGHT)
    for x, y in ((28, 8), (34, 4)):
        cv.set(x, y, PAPER_DIM)
        cv.set(x + 1, y - 2, PAPER)
    plaque(cv, 8, 50, 56, 60, "茶灶", size=11)
    cv.outline(INK)
    return cv


def facility_wine_works():
    """酒坊 64×72：酒瓮三只 + 蒸馏甑。"""
    cv = Canvas(64, 72)
    for cx, w in ((18, 10), (32, 11), (46, 10)):
        cv.rect(cx - w // 2, 40, cx + w // 2, 64, rgb("#5E4A3A"))
        cv.hline(cx - w // 2, cx + w // 2, 40, rgb("#7A6248"))
        cv.hline(cx - w // 2, cx + w // 2, 46, rgb("#3E3226"))
        cv.rect(cx - 3, 36, cx + 3, 41, PAPER_DIM)   # 瓮盖
    cv.rect(22, 18, 42, 36, STONE)                   # 蒸馏甑
    cv.hline(22, 42, 18, STONE_LIGHT)
    cv.rect(30, 10, 34, 19, STONE_DARK)
    cv.set(32, 7, PAPER_DIM)
    plaque(cv, 8, 58, 56, 68, "酒坊", size=11)
    cv.outline(INK)
    return cv


def facility_grind_table():
    """研磨台 56×56：石磨 + 药粉。"""
    cv = Canvas(56, 56)
    cv.rect(4, 32, 51, 52, WOOD_MID)
    cv.hline(4, 51, 32, WOOD_LIGHT)
    cv.hline(4, 51, 52, WOOD_DARK)
    fill_ellipse(cv, 28, 26, 16, 8, STONE)
    fill_ellipse(cv, 28, 24, 13, 6, STONE_LIGHT)
    fill_ellipse(cv, 28, 22, 4, 2, STONE_MORTAR)
    cv.rect(24, 8, 32, 20, WOOD)          # 磨杆
    cv.rect(34, 12, 44, 16, WOOD_MID)
    cv.hline(10, 20, 40, PAPER_DIM)       # 药粉
    cv.set(12, 41, PAPER)
    cv.outline(INK)
    return cv


def facility_anvil():
    """铁砧台 56×48：铁砧 + 锤 + 淬火桶。"""
    cv = Canvas(56, 48)
    cv.rect(4, 30, 40, 46, WOOD_MID)
    cv.hline(4, 40, 30, WOOD_LIGHT)
    cv.rect(10, 22, 34, 30, STONE_DARK)   # 砧身
    cv.rect(6, 18, 38, 23, STONE_DARK)
    cv.hline(6, 38, 18, STONE_LIGHT)
    cv.rect(34, 20, 40, 25, STONE_MORTAR) # 砧角
    cv.rect(16, 8, 20, 18, WOOD)          # 锤柄
    cv.rect(12, 4, 26, 10, STONE_DARK)    # 锤头
    cv.rect(42, 28, 54, 44, WOOD_MID)     # 淬火桶
    cv.hline(42, 54, 28, WOOD_LIGHT)
    cv.hline(43, 53, 31, WATER)
    cv.outline(INK)
    return cv


# ---------------------------------------------------------------------------
# 可建造建筑（docs/30 §11.2）
# ---------------------------------------------------------------------------


def buildable(width, height, roof_color, wall_color, text, *, thatch=False, door=True):
    """可建造建筑通用外形：屋面 + 墙体 + 檐下匾额（中文）+ 门两侧木格窗 + 石台基。

    匾挂檐下、窗开在门两侧与门同高：墙带只有 38~51 行，竖着再叠一层放不下（第一版把
    匾画在 `wall_top+4..wall_top+20`，正好压在门的上半截上，看着像门关着）。
    """
    cv = Canvas(width, height)
    roof_top = 6
    roof_bottom = max(roof_top + 12, int(height * 0.46))
    rows = roof_rows(roof_top, roof_bottom, 20, width - 20, 6, width - 6)
    if thatch:
        for y, x0, x1 in rows:
            cv.hline(x0, x1, y, roof_color if (y // 3) % 2 == 0 else shade(roof_color, 0.78))
        for x in range(8, width - 8, 8):
            cv.vline(x, roof_top + 4 + (x % 6), roof_bottom - 2, shade(roof_color, 0.78))
    else:
        tiled_roof(cv, rows, column=14)
        cv.rect(16, 2, width - 16, 14, TILE_ROOF_LIGHT)
        cv.hline(16, width - 16, 2, TILE_ROOF)
    cv.rect(4, roof_bottom - 4, width - 5, roof_bottom + 2, shade(roof_color, 0.6))
    wall_top = roof_bottom + 2
    wall_bottom = height - 12
    cv.rect(12, wall_top, width - 13, wall_bottom, wall_color)
    cv.rect(12, wall_top, width - 13, wall_top + 5, mix(wall_color, INK, 0.25))
    wood_post(cv, 12, wall_top, 19, wall_bottom)
    wood_post(cv, width - 20, wall_top, width - 13, wall_bottom)
    door_top = wall_bottom - 30
    if door:
        # 窗先画、匾后画：匾压在窗角上比窗框住匾好看（两者在窄房上只差 1 行）
        for wx in (22, width - 41):
            lattice_window(cv, wx, door_top + 4, wx + 18, door_top + 22)
        dx = width // 2 - 12
        cv.rect(dx, door_top, dx + 24, wall_bottom, WOOD_MID)
        cv.rect(dx + 2, door_top + 3, dx + 22, wall_bottom, WOOD_DARK)
        cv.set(dx + 6, wall_bottom - 16, GOLD)
        cv.set(dx + 18, wall_bottom - 16, GOLD)
    plaque(cv, width // 2 - 26, roof_bottom - 2, width // 2 + 26, roof_bottom + 13,
           text, size=11 if len(text) > 2 else 12)
    cv.rect(6, wall_bottom, width - 7, height - 6, STONE)
    cv.hline(6, width - 7, wall_bottom, STONE_LIGHT)
    cv.hline(6, width - 7, height - 6, STONE_MORTAR)
    cv.rect(width // 2 - 12, height - 6, width // 2 + 12, height - 1, STONE_LIGHT)
    grade_toward_light(cv)
    cv.outline(INK)
    return cv


def build_foundation():
    """地基 128×64（施工第一态）：碎石 + 木桩放线。"""
    cv = Canvas(128, 64)
    cv.rect(4, 30, 123, 58, FORGE_EARTH)
    cv.hline(4, 123, 30, shade(FORGE_EARTH, 1.2))
    cv.hline(4, 123, 58, FORGE_EARTH_DARK)
    for x in range(8, 122, 6):
        cv.set(x, 34 + (x % 5), STONE_DARK)
        cv.set(x + 2, 46 + (x % 7), STONE)
    for x in (6, 30, 62, 94, 121):   # 放线木桩 + 绳
        wood_post(cv, x - 2, 22, x + 2, 46)
    cv.hline(6, 121, 24, PAPER_DIM)
    grade_toward_light(cv)
    cv.outline(INK)
    return cv


def build_scaffold():
    """脚手架 128×96（施工叠加层）：竹竿 + 横档 + 斜撑。"""
    cv = Canvas(128, 96)
    for x in (6, 40, 84, 121):
        cv.rect(x - 2, 6, x + 2, 92, BAMBOO)
        cv.vline(x - 2, 6, 92, BAMBOO_LIGHT)
        cv.vline(x + 2, 6, 92, BAMBOO_DARK)
    for y in (16, 44, 74):
        cv.rect(4, y, 123, y + 3, BAMBOO)
        cv.hline(4, 123, y, BAMBOO_LIGHT)
        cv.hline(4, 123, y + 3, BAMBOO_DARK)
    for step in range(38):  # 斜撑
        cv.set(6 + step, 44 + step // 2, BAMBOO_DARK)
        cv.set(84 + step // 2, 74 - step, BAMBOO_DARK)
    grade_toward_light(cv)
    cv.outline(INK)
    return cv


FURNITURE = [
    ("interior_medicine_cabinet", furn_medicine_cabinet, "药柜"),
    ("interior_herb_mill", furn_herb_mill, "药碾"),
    ("interior_bed", furn_bed, "木床"),
    ("interior_table", furn_table, "方桌"),
    ("interior_chair", furn_chair, "木椅"),
    ("interior_bookshelf", furn_bookshelf, "书架"),
    ("interior_screen", furn_screen, "屏风"),
    ("interior_lantern", furn_lantern_indoor, "灯笼"),
    ("interior_stove", furn_stove, "灶台"),
    ("interior_shelf", furn_shelf_goods, "货架"),
    ("interior_barrel", furn_barrel, "木桶"),
    ("interior_chest", furn_chest, "木箱"),
]

FACILITIES = [
    ("facility_herb_stove", facility_herb_stove, "药炉"),
    ("facility_tea_stove", facility_tea_stove, "茶灶"),
    ("facility_wine_works", facility_wine_works, "酒坊"),
    ("facility_grind_table", facility_grind_table, "研磨台"),
    ("facility_anvil", facility_anvil, "铁砧台"),
]

BUILDABLES = [
    ("build_herb_hut", lambda: buildable(128, 112, TILE_ROOF, PLASTER, "药庐"), "药庐"),
    ("build_training_hall", lambda: buildable(144, 120, TILE_ROOF, PLASTER, "练功房"), "练功房"),
    ("build_storehouse", lambda: buildable(128, 112, rgb("#B99A5E"), FORGE_EARTH, "仓房", thatch=True), "仓房"),
    ("build_tea_shed", lambda: buildable(112, 96, rgb("#B99A5E"), WOOD_MID, "茶棚", thatch=True), "茶棚"),
    ("build_forge_stand", lambda: buildable(112, 104, TILE_ROOF, BRICKWALL, "铁匠台"), "铁匠台"),
    ("build_foundation", build_foundation, "地基"),
    ("build_scaffold", build_scaffold, "脚手架"),
]


def write_png(path, canvas):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(canvas.to_png())


def build_sprite_preview(entries, title, scale=2, cols=4):
    items = list(entries)
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
        draw_text(cv, x0 + pad, y0, name.replace("interior_", "").replace("facility_", "").replace("build_", ""),
                  BAMBOO_LIGHT, 1)
        if display:
            draw_cjk(cv, x0 + pad, y0 + 8, display, size=11, color=PAPER_DIM)
        top = y0 + label_h
        for by in range(0, canvas.h * scale, 8):
            for bx in range(0, canvas.w * scale, 8):
                tone = rgb("#262C33") if ((bx // 8 + by // 8) % 2 == 0) else rgb("#31383F")
                cv.rect(x0 + pad + bx, top + by, x0 + pad + bx + 7, top + by + 7, tone)
        cv.blit(canvas.scaled(scale), x0 + pad, top)
    return cv


# 本文件画的整图里需要拆层的那几张（M72-07：五座可建造建筑分 body/roof）。地基与脚手架
# 没有顶，不在这张表里。**切点行号只写在 `layer_kit.LAYER_SPLITS` 一处**，
# 本文件只负责"画完整图就立刻重切自己的分块"。
BUILD_STEMS = stems_of("gen_interior_art")


def generate(quiet=False):
    prewarm([(text, size, "pixel", True)
             for text, size in (("药", 12), ("书", 12), ("药炉", 11), ("茶灶", 11), ("酒坊", 11),
                                ("药庐", 12), ("练功房", 12), ("仓房", 12), ("茶棚", 12), ("铁匠台", 12))
             ] + [(display, 11, "pixel", True) for _, _, display in FURNITURE + FACILITIES + BUILDABLES])
    # 室内地表的出图代码（旧 `herb = [tile_plank(), ...]` 那三行与 `_pack`/`build_tiles_preview`）
    # 随 §推进顺序 5 一起摘除：两张室内图各自的地表只由 `gen_terrain_tiles.py --scene <图>` 产出。

    rendered = []
    for name, factory, display in FURNITURE:
        canvas = factory()
        rendered.append((name, canvas, display))
        write_png(os.path.join(FURNITURE_DIR, name + ".png"), canvas)
    write_png(os.path.join(FURNITURE_DIR, "_preview.png"),
              build_sprite_preview(rendered, "FURNITURE X2"))
    facs = []
    for name, factory, display in FACILITIES:
        canvas = factory()
        facs.append((name, canvas, display))
        write_png(os.path.join(FACILITY_DIR, name + ".png"), canvas)
    write_png(os.path.join(FACILITY_DIR, "_preview.png"),
              build_sprite_preview(facs, "PRODUCTION FACILITIES X2", cols=3))
    builds = []
    for name, factory, display in BUILDABLES:
        canvas = factory()
        builds.append((name, canvas, display))
        write_png(os.path.join(BUILDING_DIR, name + ".png"), canvas)
    write_png(os.path.join(BUILDING_DIR, "_buildable_preview.png"),
              build_sprite_preview(builds, "BUILDABLE BUILDINGS X2", cols=3))
    # 整图刚落盘就按行切出 body/roof：分块与整图同源同刻（`gen_manor_art.check_splits` 逐字节对拍）。
    splits = [split_stem(stem) for stem in BUILD_STEMS]
    if not quiet:
        print(f"  0 tileset（两张室内地表都不出图）+ {len(FURNITURE)} 家具 + "
              f"{len(FACILITIES)} 设施 + {len(BUILDABLES)} 可建造")
        print("  3 previews")
        for stem, pieces in zip(BUILD_STEMS, splits):
            print(f"  split    {stem}: " + " + ".join(f"{os.path.basename(p)} {rows} 行"
                                                      for p, rows in pieces))


EXPECTED = {
    "interior_medicine_cabinet": (64, 64), "interior_herb_mill": (40, 28), "interior_bed": (64, 48),
    "interior_table": (48, 40), "interior_chair": (28, 40), "interior_bookshelf": (56, 72),
    "interior_screen": (56, 56), "interior_lantern": (24, 44), "interior_stove": (48, 40),
    "interior_shelf": (64, 56), "interior_barrel": (28, 34), "interior_chest": (36, 30),
    "facility_herb_stove": (64, 72), "facility_tea_stove": (64, 64), "facility_wine_works": (64, 72),
    "facility_grind_table": (56, 56), "facility_anvil": (56, 48),
    "build_herb_hut": (128, 112), "build_training_hall": (144, 120), "build_storehouse": (128, 112),
    "build_tea_shed": (112, 96), "build_forge_stand": (112, 104),
    "build_foundation": (128, 64), "build_scaffold": (128, 96),
}


def check():
    from pixel_kit import decode_png

    issues = []
    # **反面对照**：旧那两张 6 列 × 1 行的室内地表（连同各自的 `_preview.png`）必须**不在盘上**。
    # M72-11 §推进顺序 5 起两张室内图的地表各由 `gen_terrain_tiles.py --scene <图>` 出 12×9
    # 那一张，留着 6×1 就是两套地表真相——谁把它恢复回来（或把本文件的出图代码加回来），红
    # 在这里。铁匠铺那两条是第一片（3f99bb8f）加的，本片把药庐并进来并把它们合成一个循环：
    # 从此"室内"这一档的形状是**这四张文件都不许存在**，逐图点名会让第三张室内图漏登记。
    for stale_scene in ("interior_forge", "interior_herb"):
        for stale in (os.path.join(TILE_DIR, stale_scene, "interior_ground.png"),
                      os.path.join(TILE_DIR, stale_scene, "_preview.png")):
            if os.path.exists(stale):
                issues.append(f"多出来的一套地表真相：{stale} 还在盘上——{stale_scene} 图集只由 "
                              f"gen_terrain_tiles.py --scene {stale_scene} 产出")
    for name, (want_w, want_h) in EXPECTED.items():
        if name.startswith("interior_"):
            folder = FURNITURE_DIR
        elif name.startswith("facility_"):
            folder = FACILITY_DIR
        else:
            folder = BUILDING_DIR
        path = os.path.join(folder, name + ".png")
        if not os.path.exists(path):
            issues.append("missing " + path)
            continue
        width, height, _, _ = decode_png(path)
        if (width, height) != (want_w, want_h):
            issues.append(f"{path}: {width}x{height} != {want_w}x{want_h}")
    return issues


def main():
    parser = argparse.ArgumentParser(description="《江湖有田》批次 3：室内/家具/设施/可建造建筑")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        problems = check()
        if problems:
            for problem in problems:
                print("[FAIL]", problem)
            raise SystemExit(1)
        print(f"[OK] 批次 3 校验通过（0 tileset（两张室内图的 6×1 只判它们不在盘上）+ "
              f"{len(FURNITURE)} 家具 + {len(FACILITIES)} 设施 + {len(BUILDABLES)} 可建造）")
        return
    print("生成批次 3 美术 → 室内 / 家具 / 生产设施 / 可建造建筑")
    generate()


if __name__ == "__main__":
    main()
