# -*- coding: utf-8 -*-
"""《江湖有田》云泽村（山谷村落）场景美术生成：地表 tileset + 建筑/道具 sprite。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/gen_village_art.py            # 生成全部（tileset + 建筑道具 + 预览）
    python tools/gen_village_art.py --check    # 只校验已生成资源

产出：

    assets/art/tilesets/village/village_ground.png  6 列 × 32px（地表）
    assets/art/tilesets/village/_preview.png        地表 4× 放大预览
    assets/art/buildings/village_clinic.png         药庐 192×160（6×5 格）
    assets/art/buildings/village_shop.png           杂货铺 192×160（6×5 格）
    assets/art/buildings/village_forge.png          铁匠铺 128×64（4×2 格）
    assets/art/buildings/village_teastall.png       茶摊 128×64（4×2 格）
    assets/art/buildings/village_house_a.png        民居·白墙黛瓦 128×96（4×3 格）
    assets/art/buildings/village_house_b.png        民居·夯土茅顶 128×96（4×3 格）
    assets/art/buildings/village_shrine.png         土地庙 64×64（2×2 格）
    assets/art/props/village_bridge.png             石桥 64×64（2×2 格，跨溪）
    assets/art/props/village_pier.png               渡口木栈桥 64×48
    assets/art/props/village_stall.png              市集摊位 64×40
    assets/art/props/village_banner.png             招幌 32×56
    assets/art/props/village_lantern.png            灯笼杆 24×64
    assets/art/props/village_rock.png               山石 64×40
    assets/art/buildings/_village_preview.png       建筑道具 2× 放大预览

村庄与山庄的美术区分（同一套 32px / 1px 墨色描边 / 同一色板体系，但各有身份）：

  * 地表：山庄是"夯土 + 青石院墙"，村庄是"青石板街 + 白墙黛瓦"；
    村庄草地更深、更杂（山谷野草），石板街用大块磨损石板 + 车辙。
  * 建筑：山庄正房是灰瓦土墙的庄院；村庄用徽派**白墙黛瓦 + 马头墙**，
    商铺是木板门面 + 招幌，铁匠铺是半开放工棚 + 炉火。
  * 山谷元素：村北一条溪水 + 石桥、渡口木栈桥与乌篷船、竹林山石沿北缘成山脊。

复用（不重复出图，见 docs/39 §5）：`props/manor_tree.png`、`props/manor_bamboo.png`、
`props/manor_fence.png`、`props/manor_jar.png` —— 通用的树/竹/篱/缸一件多用。

引擎硬约束（与山庄一致）：
  * tileset 只能 6 列、高度必须 32（`core/tilemap.rs` 的 TILE_PALETTE 固定 6 项，V 全幅采样）；
  * 列语义（1 草地 2 夯土 3 石板 4 院墙 5 青瓦 6 水）三个场景共用，不能重排；
  * 建筑/道具的碰撞靠 GroundMap 的 solid 位，sprite 本身不参与碰撞。

依赖：Python 3 标准库 + pixel_kit.py + gen_manor_art.py 的通用绘制工具。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import Canvas, draw_text, hash_stream, mix, rgb, shade, text_width  # noqa: E402
from gen_manor_art import (  # noqa: E402  复用通用绘制工具与基础色
    BAMBOO,
    BAMBOO_DARK,
    BAMBOO_LIGHT,
    CINNABAR,
    GOLD,
    GOLD_LIGHT,
    INK,
    PAPER,
    PAPER_DIM,
    STONE,
    STONE_DARK,
    STONE_LIGHT,
    STONE_MORTAR,
    TILE,
    TILE_ROOF,
    TILE_ROOF_DARK,
    TILE_ROOF_LIGHT,
    WATER,
    WATER_DEEP,
    WATER_HI,
    WATER_LIGHT,
    WOOD,
    WOOD_DARK,
    WOOD_LIGHT,
    WOOD_MID,
    fill_ellipse,
    roof_rows,
    speckle,
    tiled_roof,
    wood_post,
)


def frame(canvas, x0, y0, x1, y1, color, thickness=1):
    """画一圈边框（含端点）——与 gen_ui_art 同名同义，这里本地保留避免跨工具依赖。"""
    for step in range(thickness):
        canvas.hline(x0 + step, x1 - step, y0 + step, color)
        canvas.hline(x0 + step, x1 - step, y1 - step, color)
        canvas.vline(x0 + step, y0 + step, y1 - step, color)
        canvas.vline(x1 - step, y0 + step, y1 - step, color)


def wood_band_h(canvas, x0, x1, y0, y1):
    """水平木纹带（横向拉伸用：纹理沿 x 走线）。"""
    canvas.rect(x0, y0, x1, y1, WOOD_MID)
    for y in range(y0 + 1, y1, 3):
        canvas.hline(x0, x1, y, WOOD_DARK)
    canvas.hline(x0, x1, y0, WOOD_LIGHT)


def wood_band_v(canvas, x0, x1, y0, y1):
    """竖直木纹带（纵向拉伸用：纹理沿 y 走线）。"""
    canvas.rect(x0, y0, x1, y1, WOOD_MID)
    for x in range(x0 + 1, x1, 3):
        canvas.vline(x, y0, y1, WOOD_DARK)
    canvas.vline(x0, y0, y1, WOOD_LIGHT)

# 村庄专属色（在山庄色板基础上偏移，保证"一个世界、两种地方")
GRASS_V = rgb("#547242")        # 山谷野草：比山庄深
GRASS_V_DARK = rgb("#425A33")
GRASS_V_LIGHT = rgb("#668454")
GRASS_V_HI = rgb("#7A9A62")

DIRT_V = rgb("#8A7048")         # 茶园/土路：偏沙
DIRT_V_DARK = rgb("#6E5836")
DIRT_V_LIGHT = rgb("#A08858")

FLAG = rgb("#A09884")           # 青石板街：暖灰、磨损
FLAG_LIGHT = rgb("#B5AD98")
FLAG_DARK = rgb("#7C7466")
FLAG_CRACK = rgb("#6A6358")

WHITEWASH = rgb("#D8D2C4")      # 徽派白墙
WHITEWASH_DARK = rgb("#B3AC9C")
WHITEWASH_STAIN = rgb("#C2BBAA")

SLATE = rgb("#3A444C")          # 黛瓦
SLATE_LIGHT = rgb("#4E5A63")
SLATE_DARK = rgb("#28313A")

STREAM = rgb("#3E6485")         # 溪水
STREAM_LIGHT = rgb("#5D82A0")
STREAM_HI = rgb("#7FA3D4")
STREAM_DEEP = rgb("#2F4E6B")

THATCH = rgb("#B99A5E")         # 茅草（民居 B）
THATCH_DARK = rgb("#96793F")
FIRE = rgb("#E07B3A")           # 炉火
FIRE_HI = rgb("#F2C05A")

TILE_DIR = os.path.join("assets", "art", "tilesets", "village")
BUILDING_DIR = os.path.join("assets", "art", "buildings")
PROP_DIR = os.path.join("assets", "art", "props")


# ---------------------------------------------------------------------------
# 地表 tileset（6 列 × 32px，列语义与山庄一致）
# ---------------------------------------------------------------------------


def tile_grass_valley(seed=211):
    """1 草地：山谷野草——比山庄更深、更杂，带落叶与三叶草。"""
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, GRASS_V)
    rnd = hash_stream(seed)
    for _ in range(34):
        cv.set(rnd(TILE), rnd(TILE), GRASS_V_DARK)
    for _ in range(22):
        cv.set(rnd(TILE), rnd(TILE), GRASS_V_LIGHT)
    for _ in range(6):
        x = 3 + rnd(TILE - 7)
        y = 3 + rnd(TILE - 7)
        cv.set(x, y, GRASS_V_HI)
        cv.set(x - 1, y + 1, GRASS_V_LIGHT)
        cv.set(x + 1, y + 1, GRASS_V_LIGHT)
    # 落叶（两张小叶）
    for _ in range(3):
        x = 4 + rnd(TILE - 10)
        y = 4 + rnd(TILE - 10)
        cv.set(x, y, THATCH)
        cv.set(x + 1, y + 1, THATCH_DARK)
    if rnd(2) == 0:
        x = 6 + rnd(TILE - 12)
        y = 6 + rnd(TILE - 12)
        cv.set(x, y, PAPER)
        cv.set(x + 1, y, GOLD_LIGHT)
    return cv


def tile_dirt_tea(seed=222):
    """2 夯土：茶园/土路——偏沙的踩实土，带碎石与耙痕。"""
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, DIRT_V)
    rnd = hash_stream(seed)
    for _ in range(28):
        cv.set(rnd(TILE), rnd(TILE), DIRT_V_DARK)
    for _ in range(20):
        cv.set(rnd(TILE), rnd(TILE), DIRT_V_LIGHT)
    # 耙痕：横向浅沟，暗示茶垄
    for y in (6, 16, 26):
        cv.hline(0, TILE - 1, y, DIRT_V_DARK)
        cv.hline(0, TILE - 1, y + 1, DIRT_V_LIGHT)
    for _ in range(7):
        x = 2 + rnd(TILE - 5)
        y = 2 + rnd(TILE - 5)
        cv.set(x, y, STONE_MORTAR)
        cv.set(x + 1, y, DIRT_V_LIGHT)
    return cv


def tile_flagstone(seed=233):
    """3 青石板街：大块磨损石板 + 车辙缝。"""
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, FLAG)
    tones = [FLAG, FLAG_LIGHT, shade(FLAG, 0.95), FLAG]
    # 2×2 大石板，尺寸不一以显得旧
    cv.rect(1, 1, 20, 13, tones[0])
    cv.rect(22, 1, 30, 13, tones[1])
    cv.rect(1, 15, 13, 30, tones[2])
    cv.rect(15, 15, 30, 30, tones[3])
    for y in (0, 14, 31):
        cv.hline(0, TILE - 1, y, FLAG_CRACK)
    cv.vline(0, 0, TILE - 1, FLAG_CRACK)
    cv.vline(21, 0, 13, FLAG_CRACK)
    cv.vline(14, 14, TILE - 1, FLAG_CRACK)
    cv.hline(22, 30, 0, FLAG_LIGHT)
    rnd = hash_stream(seed)
    for _ in range(18):
        cv.set(rnd(TILE), rnd(TILE), FLAG_DARK)
    for _ in range(12):
        cv.set(rnd(TILE), rnd(TILE), FLAG_LIGHT)
    # 车辙：两道浅亮痕
    for x in (9, 24):
        cv.vline(x, 0, TILE - 1, shade(FLAG, 1.06))
    return cv


def tile_village_wall(seed=244):
    """4 院墙：白墙 + 黛瓦压顶（徽派），与山庄的石砌院墙区分。"""
    cv = Canvas(TILE, TILE)
    # 黛瓦压顶
    cv.rect(0, 0, TILE - 1, 1, SLATE_DARK)
    cv.rect(0, 2, TILE - 1, 6, SLATE)
    cv.hline(0, TILE - 1, 3, SLATE_LIGHT)
    cv.hline(0, TILE - 1, 7, SLATE_DARK)
    # 白墙
    cv.rect(0, 8, TILE - 1, 27, WHITEWASH)
    cv.hline(0, TILE - 1, 8, WHITEWASH_DARK)
    rnd = hash_stream(seed)
    for _ in range(20):
        cv.set(rnd(TILE), 9 + rnd(18), WHITEWASH_STAIN)
    # 墙脚：青石基
    cv.rect(0, 28, TILE - 1, TILE - 1, STONE_DARK)
    cv.hline(0, TILE - 1, 28, STONE_MORTAR)
    for x in range(4, TILE, 10):
        cv.vline(x, 29, TILE - 1, STONE_MORTAR)
    for _ in range(10):
        cv.set(rnd(TILE), 29 + rnd(3), STONE_LIGHT)
    return cv


def tile_slate_roof(seed=255):
    """5 青瓦：黛瓦垄（与山庄第 5 列同语义，色调更冷）。"""
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, SLATE)
    for x in range(0, TILE, 4):
        cv.vline(x, 0, TILE - 1, SLATE_LIGHT)
        cv.vline(x + 1, 0, TILE - 1, SLATE_DARK)
    for y in (0, 15, 31):
        cv.hline(0, TILE - 1, y, SLATE_DARK)
    cv.hline(0, TILE - 1, 1, SLATE_LIGHT)
    speckle(cv, 0, 0, TILE - 1, TILE - 1, shade(SLATE, 0.9), 20, seed)
    return cv


def tile_stream(seed=266):
    """6 水：溪水——比山庄池塘更清、有流向与浅滩反光。"""
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, STREAM)
    cv.hline(0, TILE - 1, 0, STREAM_DEEP)
    for y in (5, 13, 21, 29):
        for x in range(1, TILE - 2, 7):
            cv.set(x, y, STREAM_LIGHT)
            cv.set(x + 1, y - 1, STREAM_LIGHT)
            cv.set(x + 2, y, STREAM_LIGHT)
            cv.set(x + 3, y + 1, STREAM_HI)
    rnd = hash_stream(seed)
    for _ in range(16):
        cv.set(rnd(TILE), 2 + rnd(TILE - 4), STREAM_DEEP)
    for _ in range(8):
        cv.set(rnd(TILE), rnd(TILE), STREAM_HI)
    return cv


def ground_tiles():
    return [tile_grass_valley(), tile_dirt_tea(), tile_flagstone(),
            tile_village_wall(), tile_slate_roof(), tile_stream()]


def pack_tileset(tiles):
    sheet = Canvas(TILE * len(tiles), TILE)
    for index, tile in enumerate(tiles):
        sheet.blit(tile, index * TILE, 0)
    return sheet


# ---------------------------------------------------------------------------
# 建筑
# ---------------------------------------------------------------------------


def white_wall_house(width, height, *, gable=True, windows=2, door=True, banner=False):
    """徽派白墙黛瓦民居骨架：马头墙 + 悬山顶 + 木格窗 + 板门。"""
    cv = Canvas(width, height)
    roof_top = 6
    roof_bottom = int(height * 0.52)
    rows = roof_rows(roof_top, roof_bottom, 18, width - 18, 4, width - 4)
    tiled_roof(cv, rows, column=14)
    # 正脊
    cv.rect(14, 2, width - 14, 14, SLATE_LIGHT)
    cv.hline(14, width - 14, 2, SLATE)
    cv.hline(14, width - 14, 14, SLATE_DARK)
    cv.rect(6, 0, 16, 14, SLATE_DARK)
    cv.rect(width - 16, 0, width - 6, 14, SLATE_DARK)
    # 檐口
    cv.rect(2, roof_bottom - 4, width - 3, roof_bottom + 2, SLATE_DARK)
    cv.hline(2, width - 3, roof_bottom - 5, SLATE_LIGHT)
    # 白墙
    wall_top = roof_bottom + 2
    wall_bottom = height - 14
    cv.rect(10, wall_top, width - 11, wall_bottom, WHITEWASH)
    cv.rect(10, wall_top, width - 11, wall_top + 6, mix(WHITEWASH, INK, 0.22))
    if gable:
        # 马头墙：左右阶梯状山墙
        for step, (x0, x1) in enumerate(((4, 10), (6, 12))):
            top = roof_top + 10 + step * 16
            cv.rect(x0, top, x1, wall_bottom, WHITEWASH)
            cv.rect(x0, top - 4, x1, top - 1, SLATE)
            cv.vline(x0, top, wall_bottom, WHITEWASH_DARK)
            cv.vline(4, top, wall_bottom, WHITEWASH_DARK)
            mirror = width - x1
            cv.rect(width - x1 - 1, top, width - x0 - 1, wall_bottom, WHITEWASH)
            cv.rect(width - x1 - 1, top - 4, width - x0 - 1, top - 1, SLATE)
            cv.vline(width - x1 - 1, top, wall_bottom, WHITEWASH_DARK)
            cv.vline(width - 5, top, wall_bottom, WHITEWASH_DARK)
            del mirror
    # 木柱
    wood_post(cv, 10, wall_top, 16, wall_bottom)
    wood_post(cv, width - 17, wall_top, width - 11, wall_bottom)
    # 窗
    for index in range(windows):
        wx = 26 + index * ((width - 60) // max(1, windows - 1) if windows > 1 else 0)
        cv.rect(wx, wall_top + 18, wx + 26, wall_top + 42, WOOD_MID)
        cv.rect(wx + 2, wall_top + 20, wx + 24, wall_top + 40, PAPER)
        for gx in range(wx + 3, wx + 24, 4):
            cv.vline(gx, wall_top + 20, wall_top + 40, WOOD_MID)
        cv.hline(wx + 2, wx + 24, wall_top + 30, WOOD_MID)
    # 门
    if door:
        dx = width // 2 - 16
        cv.rect(dx, wall_bottom - 40, dx + 32, wall_bottom, WOOD_MID)
        cv.rect(dx + 3, wall_bottom - 36, dx + 29, wall_bottom, WOOD_DARK)
        cv.vline(dx + 16, wall_bottom - 36, wall_bottom, WOOD)
        cv.set(dx + 10, wall_bottom - 20, GOLD)
        cv.set(dx + 22, wall_bottom - 20, GOLD)
    # 檐下挑出的招幌
    if banner:
        bx = width - 34
        cv.vline(bx, wall_top, wall_top + 20, WOOD_DARK)
        cv.rect(bx - 6, wall_top + 16, bx + 8, wall_top + 40, CINNABAR)
        cv.hline(bx - 6, bx + 8, wall_top + 16, GOLD)
        cv.hline(bx - 6, bx + 8, wall_top + 40, GOLD)
    # 石台基与台阶
    cv.rect(4, wall_bottom, width - 5, height - 6, STONE)
    cv.hline(4, width - 5, wall_bottom, STONE_LIGHT)
    cv.hline(4, width - 5, height - 6, STONE_MORTAR)
    for x in range(10, width - 10, 22):
        cv.vline(x, wall_bottom + 1, height - 7, STONE_MORTAR)
    cv.rect(width // 2 - 14, height - 6, width // 2 + 14, height - 1, STONE_LIGHT)
    cv.hline(width // 2 - 14, width // 2 + 14, height - 1, STONE_MORTAR)
    cv.outline(INK)
    return cv


def building_clinic():
    """药庐 192×160：白墙黛瓦 + 马头墙 + 药幌（不写字）。"""
    return white_wall_house(192, 160, gable=True, windows=2, door=True, banner=True)


def building_shop():
    """杂货铺 192×160：木板门面 + 货架 + 挑檐 + 招幌。"""
    cv = Canvas(192, 160)
    rows = roof_rows(6, 78, 16, 176, 2, 189)
    tiled_roof(cv, rows, column=14)
    cv.rect(12, 2, 178, 14, SLATE_LIGHT)
    cv.hline(12, 178, 2, SLATE)
    cv.rect(4, 0, 14, 14, SLATE_DARK)
    cv.rect(176, 0, 188, 14, SLATE_DARK)
    cv.rect(0, 74, 191, 80, SLATE_DARK)
    cv.hline(0, 191, 73, SLATE_LIGHT)
    # 门面：敞开式柜台
    cv.rect(12, 80, 179, 138, WOOD_MID)
    for x in range(14, 179, 8):
        cv.vline(x, 80, 138, WOOD_DARK)
    cv.rect(12, 80, 179, 90, mix(WOOD_MID, INK, 0.3))
    # 柜台
    cv.rect(20, 124, 170, 140, WOOD)
    cv.hline(20, 170, 124, WOOD_LIGHT)
    cv.hline(20, 170, 140, WOOD_DARK)
    # 货架与货箱
    cv.rect(28, 94, 80, 120, WOOD_DARK)
    for y in (100, 110):
        cv.hline(28, 80, y, WOOD)
        for x in range(32, 78, 12):
            cv.rect(x, y - 8, x + 8, y - 1, GOLD if (x // 12) % 2 else CINNABAR)
    cv.rect(100, 100, 128, 124, WOOD)
    cv.rect(134, 104, 162, 124, WOOD_LIGHT)
    # 招幌
    bx = 156
    cv.vline(bx, 80, 104, WOOD_DARK)
    cv.rect(bx - 8, 100, bx + 8, 130, CINNABAR)
    cv.hline(bx - 8, bx + 8, 100, GOLD)
    cv.hline(bx - 8, bx + 8, 130, GOLD)
    # 台基
    cv.rect(6, 138, 185, 150, STONE)
    cv.hline(6, 185, 138, STONE_LIGHT)
    cv.hline(6, 185, 150, STONE_MORTAR)
    for x in range(14, 180, 24):
        cv.vline(x, 139, 149, STONE_MORTAR)
    cv.outline(INK)
    return cv


def building_forge():
    """铁匠铺 128×64：半开放工棚 + 炉火 + 铁砧 + 柴堆。"""
    cv = Canvas(128, 64)
    rows = roof_rows(4, 30, 12, 116, 0, 127)
    for y, x0, x1 in rows:
        cv.hline(x0, x1, y, THATCH if (y // 3) % 2 == 0 else THATCH_DARK)
    for x in range(6, 124, 9):
        cv.vline(x, 8 + (x % 6), 28, THATCH_DARK)
    cv.hline(0, 127, 30, THATCH_DARK)
    # 木柱与后墙
    wood_post(cv, 6, 30, 14, 58)
    wood_post(cv, 114, 30, 122, 58)
    cv.rect(14, 30, 114, 44, mix(WOOD_DARK, INK, 0.35))
    # 炉火
    cv.rect(24, 40, 56, 58, STONE_DARK)
    cv.rect(28, 44, 52, 56, shade(STONE_DARK, 0.8))
    fill_ellipse(cv, 40, 50, 10, 7, FIRE)
    fill_ellipse(cv, 40, 52, 5, 4, FIRE_HI)
    cv.rect(34, 34, 46, 42, STONE_MORTAR)  # 烟囱
    # 铁砧
    cv.rect(74, 48, 92, 52, STONE_DARK)
    cv.rect(80, 52, 86, 58, STONE_DARK)
    cv.hline(74, 92, 48, STONE_LIGHT)
    # 柴堆
    for index, y in enumerate(range(46, 58, 4)):
        cv.rect(98, y, 118, y + 3, WOOD if index % 2 == 0 else WOOD_LIGHT)
        cv.hline(98, 118, y + 3, WOOD_DARK)
    cv.rect(2, 58, 126, 63, STONE)
    cv.hline(2, 126, 58, STONE_LIGHT)
    cv.outline(INK)
    return cv


def building_teastall():
    """茶摊 128×64：布棚 + 茶桌长凳 + 茶旗。"""
    cv = Canvas(128, 64)
    # 布棚
    cv.rect(8, 6, 120, 26, PAPER)
    cv.hline(8, 120, 6, PAPER_DIM)
    for x in range(10, 120, 16):
        cv.vline(x, 7, 25, PAPER_DIM)
    cv.rect(6, 24, 122, 28, PAPER_DIM)
    cv.hline(6, 122, 27, mix(PAPER_DIM, INK, 0.25))
    # 支柱
    wood_post(cv, 8, 26, 14, 58)
    wood_post(cv, 114, 26, 120, 58)
    # 茶桌与长凳
    cv.rect(30, 38, 98, 44, WOOD)
    cv.hline(30, 98, 38, WOOD_LIGHT)
    cv.hline(30, 98, 44, WOOD_DARK)
    for x in (34, 92):
        cv.vline(x, 44, 56, WOOD_MID)
    cv.rect(24, 48, 104, 52, WOOD_MID)
    cv.hline(24, 104, 48, WOOD_LIGHT)
    # 茶具
    cv.rect(44, 32, 54, 38, PAPER)
    cv.rect(62, 33, 70, 38, BAMBOO)
    cv.rect(80, 32, 88, 38, PAPER_DIM)
    # 茶旗
    cv.vline(110, 28, 50, WOOD_DARK)
    cv.rect(102, 30, 110, 48, BAMBOO)
    cv.hline(102, 110, 30, BAMBOO_LIGHT)
    cv.outline(INK)
    return cv


def building_house_a():
    """民居·白墙黛瓦 128×96。"""
    return white_wall_house(128, 96, gable=False, windows=1, door=True)


def building_house_b():
    """民居·夯土茅顶 128×96：更穷的农家。"""
    cv = Canvas(128, 96)
    rows = roof_rows(6, 48, 14, 114, 2, 125)
    for y, x0, x1 in rows:
        cv.hline(x0, x1, y, THATCH if (y // 3) % 2 == 0 else THATCH_DARK)
    for x in range(6, 122, 8):
        cv.vline(x, 10 + (x % 7), 46, THATCH_DARK)
    cv.hline(2, 125, 48, THATCH_DARK)
    # 夯土墙
    cv.rect(10, 50, 117, 82, DIRT_V)
    for x in range(12, 117, 6):
        cv.vline(x, 50, 82, DIRT_V_DARK)
    cv.rect(54, 54, 84, 82, WOOD_DARK)
    cv.rect(58, 58, 80, 82, shade(WOOD_DARK, 0.85))
    cv.rect(24, 58, 44, 74, WOOD_MID)
    cv.rect(26, 60, 42, 72, PAPER_DIM)
    cv.outline(INK)
    return cv


def building_shrine():
    """土地庙 64×64：石台 + 小瓦顶 + 香炉 + 供品。"""
    cv = Canvas(64, 64)
    rows = roof_rows(6, 24, 10, 54, 2, 61)
    tiled_roof(cv, rows, column=10)
    cv.rect(8, 4, 56, 12, SLATE_LIGHT)
    cv.hline(8, 56, 4, SLATE)
    cv.rect(2, 22, 61, 28, SLATE_DARK)
    # 庙身
    cv.rect(14, 28, 50, 52, WHITEWASH)
    cv.vline(14, 28, 52, WHITEWASH_DARK)
    cv.vline(50, 28, 52, WHITEWASH_DARK)
    cv.rect(24, 34, 40, 52, mix(WOOD_DARK, INK, 0.5))
    cv.rect(28, 38, 36, 52, shade(WOOD_DARK, 0.7))
    # 供台与香炉
    cv.rect(8, 52, 56, 58, STONE)
    cv.hline(8, 56, 52, STONE_LIGHT)
    cv.rect(26, 44, 38, 52, STONE_DARK)
    cv.hline(26, 38, 44, STONE_LIGHT)
    cv.set(31, 42, FIRE_HI)
    cv.set(32, 41, FIRE)
    cv.rect(12, 46, 20, 52, CINNABAR)
    cv.rect(44, 46, 52, 52, GOLD)
    cv.outline(INK)
    return cv


# ---------------------------------------------------------------------------
# 道具
# ---------------------------------------------------------------------------


def prop_bridge():
    """石桥 64×64（2×2 格，跨溪）：拱桥 + 石栏。"""
    cv = Canvas(64, 64)
    # 桥面
    cv.rect(0, 18, 63, 44, STONE)
    cv.hline(0, 63, 18, STONE_LIGHT)
    cv.hline(0, 63, 44, STONE_MORTAR)
    for x in range(4, 64, 10):
        cv.vline(x, 19, 43, STONE_MORTAR)
    # 拱洞
    fill_ellipse(cv, 32, 52, 22, 16, STREAM_DEEP)
    fill_ellipse(cv, 32, 54, 18, 13, shade(STREAM, 0.8))
    # 石栏
    cv.rect(0, 14, 63, 20, STONE_LIGHT)
    for x in range(2, 64, 12):
        cv.rect(x, 4, x + 6, 18, STONE)
        cv.hline(x, x + 6, 4, STONE_LIGHT)
    cv.vline(0, 14, 20, STONE_MORTAR)
    cv.outline(INK)
    return cv


def prop_pier():
    """渡口木栈桥 96×32：木板栈桥 + 木桩 + 系泊的乌篷船（与 scene 声明尺寸一致）。"""
    cv = Canvas(96, 32)
    # 栈桥面
    cv.rect(4, 8, 58, 18, WOOD)
    for x in range(4, 58, 7):
        cv.vline(x, 8, 18, WOOD_DARK)
    cv.hline(4, 58, 8, WOOD_LIGHT)
    cv.hline(4, 58, 18, WOOD_DARK)
    for x in (8, 26, 44):
        wood_post(cv, x, 18, x + 4, 30)
    cv.rect(0, 6, 6, 10, WOOD_MID)
    # 乌篷船：船身 + 竹篷 + 缆绳
    cv.rect(64, 18, 92, 24, WOOD_MID)
    cv.hline(64, 92, 18, WOOD_LIGHT)
    cv.hline(64, 92, 24, WOOD_DARK)
    cv.set(63, 20, WOOD_MID)
    cv.set(93, 20, WOOD_MID)
    cv.rect(70, 10, 86, 19, BAMBOO)
    for x in range(70, 86, 4):
        cv.vline(x, 10, 18, BAMBOO_DARK)
    cv.hline(70, 86, 10, BAMBOO_LIGHT)
    cv.vline(88, 6, 18, WOOD_DARK)
    cv.hline(50, 64, 16, WOOD_LIGHT)
    cv.outline(INK)
    return cv


def prop_stall():
    """市集摊位 64×40：木桌 + 笸箩 + 布棚。"""
    cv = Canvas(64, 40)
    cv.rect(2, 8, 62, 18, PAPER)
    cv.hline(2, 62, 8, PAPER_DIM)
    cv.rect(0, 17, 63, 21, PAPER_DIM)
    cv.rect(4, 21, 60, 27, WOOD)
    cv.hline(4, 60, 21, WOOD_LIGHT)
    cv.hline(4, 60, 27, WOOD_DARK)
    cv.vline(8, 27, 36, WOOD_MID)
    cv.vline(56, 27, 36, WOOD_MID)
    # 笸箩与货物
    for x, color in ((12, GOLD), (28, CINNABAR), (44, BAMBOO)):
        fill_ellipse(cv, x + 5, 17, 6, 4, color)
        cv.hline(x, x + 10, 13, shade(color, 1.2))
    cv.outline(INK)
    return cv


def prop_banner():
    """招幌 32×56：木杆 + 布幌（金边，不写字）。"""
    cv = Canvas(32, 56)
    wood_post(cv, 6, 4, 10, 55)
    cv.rect(10, 8, 28, 40, CINNABAR)
    cv.hline(10, 28, 8, GOLD)
    cv.hline(10, 28, 40, GOLD)
    cv.vline(28, 8, 40, GOLD)
    cv.hline(10, 28, 22, GOLD_LIGHT)
    cv.rect(14, 26, 24, 34, PAPER)  # 留白匾心，文字运行时渲染
    cv.outline(INK)
    return cv


def prop_lantern():
    """灯笼杆 24×64：木杆 + 两盏朱砂灯笼。"""
    cv = Canvas(24, 64)
    wood_post(cv, 10, 2, 14, 62)
    cv.rect(2, 6, 22, 10, WOOD_DARK)
    for y in (12, 34):
        cv.vline(12, y - 4, y, WOOD_DARK)
        cv.rect(4, y, 20, y + 14, CINNABAR)
        cv.hline(4, 20, y, GOLD)
        cv.hline(4, 20, y + 14, GOLD)
        cv.vline(12, y, y + 14, shade(CINNABAR, 0.8))
        cv.vline(12, y + 15, y + 18, CINNABAR)
    cv.outline(INK)
    return cv


def prop_rock():
    """山石 64×40：苔石堆（山谷北缘用）。"""
    cv = Canvas(64, 40)
    fill_ellipse(cv, 22, 26, 18, 12, STONE_DARK)
    fill_ellipse(cv, 44, 28, 14, 10, shade(STONE_DARK, 0.88))
    fill_ellipse(cv, 34, 18, 12, 9, STONE)
    fill_ellipse(cv, 20, 20, 7, 5, STONE_LIGHT)
    rnd = hash_stream(909)
    for _ in range(26):
        x = rnd(64)
        y = 8 + rnd(30)
        if cv.get(x, y)[:3] == STONE_DARK[:3]:
            cv.set(x, y, shade(STONE_DARK, 0.85))
    # 苔痕
    for x, y in ((12, 30), (18, 32), (40, 34), (48, 30), (30, 24)):
        cv.set(x, y, BAMBOO_DARK)
        cv.set(x + 1, y, BAMBOO)
    cv.outline(INK)
    return cv


def building_gate():
    """村口牌坊 96×96（3×3 格）：木牌坊 + 匾额（不写字）+ 石阶，压在村南院墙上。"""
    cv = Canvas(96, 96)
    # 牌坊顶：黛瓦小顶 + 两端起翘
    rows = roof_rows(6, 28, 16, 80, 4, 92)
    tiled_roof(cv, rows, column=12)
    cv.rect(12, 2, 84, 12, SLATE_LIGHT)
    cv.hline(12, 84, 2, SLATE)
    cv.rect(4, 0, 16, 12, SLATE_DARK)
    cv.rect(80, 0, 92, 12, SLATE_DARK)
    cv.rect(2, 26, 93, 32, SLATE_DARK)
    cv.hline(2, 93, 25, SLATE_LIGHT)
    # 立柱
    wood_post(cv, 8, 32, 22, 88)
    wood_post(cv, 74, 32, 88, 88)
    cv.rect(8, 32, 22, 36, WOOD_DARK)
    cv.rect(74, 32, 88, 36, WOOD_DARK)
    # 匾额（留白，文字运行时渲染）
    cv.rect(28, 38, 68, 60, WOOD_DARK)
    cv.rect(30, 40, 66, 58, WHITEWASH)
    frame(cv, 30, 40, 66, 58, GOLD, 1)
    # 灯笼
    for lx in (26, 66):
        cv.vline(lx + 3, 60, 66, WOOD_DARK)
        cv.rect(lx, 66, lx + 8, 78, CINNABAR)
        cv.hline(lx, lx + 8, 66, GOLD)
        cv.hline(lx, lx + 8, 78, GOLD)
    # 石阶
    cv.rect(4, 88, 92, 95, STONE)
    cv.hline(4, 92, 88, STONE_LIGHT)
    cv.hline(4, 92, 95, STONE_MORTAR)
    for x in range(12, 90, 20):
        cv.vline(x, 89, 94, STONE_MORTAR)
    cv.outline(INK)
    return cv


def buildings():
    return {
        "clinic": building_clinic(),
        "shop": building_shop(),
        "forge": building_forge(),
        "teastall": building_teastall(),
        "house_a": building_house_a(),
        "house_b": building_house_b(),
        "shrine": building_shrine(),
        "gate": building_gate(),
    }


def props():
    return {
        "bridge": prop_bridge(),
        "pier": prop_pier(),
        "stall": prop_stall(),
        "banner": prop_banner(),
        "lantern": prop_lantern(),
        "rock": prop_rock(),
    }


# ---------------------------------------------------------------------------
# 输出
# ---------------------------------------------------------------------------


def write_png(path, canvas):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(canvas.to_png())


def build_tiles_preview(tiles, scale=4):
    labels = ["GRASS_VALLEY", "DIRT_TEA", "FLAGSTONE", "WHITEWASH_WALL", "SLATE_ROOF", "STREAM"]
    tile = TILE * scale
    label_w = text_width("WHITEWASH_WALL", 1) + 10
    width = label_w + tile + 8
    row_h = tile + 4
    height = 20 + len(tiles) * row_h + 8
    cv = Canvas(width, height)
    cv.rect(0, 0, width - 1, height - 1, rgb("#14171A"))
    draw_text(cv, 4, 4, "VILLAGE TILESET 32PX X4", GOLD_LIGHT, 1)
    for index, tile_canvas in enumerate(tiles):
        y = 20 + index * row_h
        draw_text(cv, 4, y + tile // 2 - 2, labels[index], BAMBOO_LIGHT, 1)
        cv.rect(label_w - 1, y - 1, label_w + tile, y + tile, rgb("#262C33"))
        cv.blit(tile_canvas.scaled(scale), label_w, y)
    return cv


def build_sprite_preview(sprites, scale=2):
    items = sorted(sprites.items())
    pad = 10
    cols = 4
    label_h = 12
    cell_w = max(sprite.w for _, sprite in items) * scale + pad * 2
    cell_h = max(sprite.h for _, sprite in items) * scale + pad * 2 + label_h
    rows = (len(items) + cols - 1) // cols
    cv = Canvas(cell_w * cols + 8, 20 + cell_h * rows + 8)
    cv.rect(0, 0, cv.w - 1, cv.h - 1, rgb("#14171A"))
    draw_text(cv, 4, 4, "VILLAGE BUILDINGS + PROPS", GOLD_LIGHT, 1)
    for index, (name, sprite) in enumerate(items):
        row, col = divmod(index, cols)
        x0 = 4 + col * cell_w
        y0 = 20 + row * cell_h
        draw_text(cv, x0 + pad, y0, name, BAMBOO_LIGHT, 1)
        for by in range(0, sprite.h * scale, 8):
            for bx in range(0, sprite.w * scale, 8):
                tone = rgb("#262C33") if ((bx // 8 + by // 8) % 2 == 0) else rgb("#31383F")
                cv.rect(x0 + pad + bx, y0 + label_h + by,
                        x0 + pad + bx + 7, y0 + label_h + by + 7, tone)
        cv.blit(sprite.scaled(scale), x0 + pad, y0 + label_h)
    return cv


def generate(quiet=False):
    tiles = ground_tiles()
    sheet = pack_tileset(tiles)
    write_png(os.path.join(TILE_DIR, "village_ground.png"), sheet)
    write_png(os.path.join(TILE_DIR, "_preview.png"), build_tiles_preview(tiles))
    all_sprites = buildings()
    all_sprites.update(props())
    for name, sprite in buildings().items():
        write_png(os.path.join(BUILDING_DIR, f"village_{name}.png"), sprite)
    for name, sprite in props().items():
        write_png(os.path.join(PROP_DIR, f"village_{name}.png"), sprite)
    write_png(os.path.join(BUILDING_DIR, "_village_preview.png"), build_sprite_preview(all_sprites))
    if not quiet:
        print(f"tileset  village_ground.png {sheet.w}x{sheet.h} (6 列)")
        for name, sprite in sorted(all_sprites.items()):
            print(f"sprite   village_{name}.png {sprite.w}x{sprite.h}")
        print(f"preview  {TILE_DIR}/_preview.png, {BUILDING_DIR}/_village_preview.png")


def check():
    from pixel_kit import decode_png

    issues = []
    path = os.path.join(TILE_DIR, "village_ground.png")
    if not os.path.exists(path):
        issues.append("missing " + path)
    else:
        width, height, _, _ = decode_png(path)
        if (width, height) != (TILE * 6, TILE):
            issues.append(f"{path}: {width}x{height} != 192x32（引擎要求正好 6 列 × 32px）")
    for name in buildings():
        p = os.path.join(BUILDING_DIR, f"village_{name}.png")
        if not os.path.exists(p):
            issues.append("missing " + p)
    for name in props():
        p = os.path.join(PROP_DIR, f"village_{name}.png")
        if not os.path.exists(p):
            issues.append("missing " + p)
    for extra in (os.path.join(TILE_DIR, "_preview.png"),
                  os.path.join(BUILDING_DIR, "_village_preview.png")):
        if not os.path.exists(extra):
            issues.append("missing " + extra)
    return issues


def main():
    parser = argparse.ArgumentParser(description="《江湖有田》云泽村场景美术生成")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        problems = check()
        if problems:
            for problem in problems:
                print("[FAIL]", problem)
            raise SystemExit(1)
        print("[OK] 云泽村美术资源校验通过")
        return
    generate()


if __name__ == "__main__":
    main()
