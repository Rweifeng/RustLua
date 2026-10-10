# -*- coding: utf-8 -*-
"""《江湖有田》云泽村（山谷村落）建筑与道具美术生成。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/gen_village_art.py            # 生成建筑/道具 + 按登记表切遮断块
    python tools/gen_village_art.py --check    # 只校验盘上资源（含拆层逐字节对拍）

产出：

    assets/art/buildings/village_clinic.png       药庐 192×160（6×5 格）
    assets/art/buildings/village_shop.png         杂货铺 192×160（6×5 格）
    assets/art/buildings/village_forge.png        铁匠铺 128×64（4×2 格）
    assets/art/buildings/village_teastall.png     茶摊 128×64（4×2 格）
    assets/art/buildings/village_house_a.png      民居·白墙黛瓦 128×96（4×3 格）
    assets/art/buildings/village_house_b.png      民居·夯土茅顶 128×96（4×3 格）
    assets/art/buildings/village_shrine.png       土地庙 64×64（2×2 格）
    assets/art/buildings/village_gate.png         村口牌坊 96×96（3×3 格，**中间是真门洞**）
    assets/art/buildings/village_<名>_{roof,canopy}.png
                                                  遮断块（`layer_kit.LAYER_SPLITS` 按行切出）
    assets/art/props/village_bridge.png           石桥 64×64（2×2 格，跨溪）
    assets/art/props/village_pier.png             渡口木栈桥 96×32
    assets/art/props/village_stall.png            市集摊位 64×40
    assets/art/props/village_banner.png           招幌 32×56
    assets/art/props/village_lantern.png          灯笼杆 24×64
    assets/art/props/village_rock.png             山石 64×40
    assets/art/buildings/_village_preview.png     建筑道具 2× 放大预览（`_` 前缀=开发件，不进包）

**地表不在这个文件里**（M72-11 起）：以前这里画 6 张 32px 逻辑瓦并打包成 6 列 × 1 行的
`village_ground.png`，那是 M72-02 的口径。金标规范（M72-04 起）要的是 12×9 图集 =
6 张逻辑瓦 + 每族 16 个 autotile 过渡块，由 `tools/gen_terrain_tiles.py --scene village`
产出，取色口径是 `tools/terrain_ramp.json` 的 `scenes.village`。所以本工具只出建筑与道具，
两边共用同一张色阶表（`ramp()`/`utility()` 那两条函数就是接口的地方）：
村庄屋宇的黛瓦与院墙压顶从此是同一种颜色，而不是两套各写死的近似值。

村庄与山庄的美术区分（同一套 32px / 1px 墨色描边 / 同一色阶表，但各有身份）：

  * 地表：山庄是"夯土 + 青石院墙"，村庄是"青石板街 + 白墙黛瓦"；
    村庄草地更深、更杂（山谷野草），石板街用大块磨损石板 + 车辙。
  * 建筑：山庄正房是灰瓦土墙的庄院；村庄用徽派**白墙黛瓦 + 马头墙**，
    商铺是木板门面 + 招幌，铁匠铺是半开放工棚 + 炉火。
  * 山谷元素：村北一条溪水 + 石桥、渡口木栈桥与乌篷船、竹林山石沿北缘成山脊。

复用（不重复出图，见 docs/39 §5）：`props/manor_tree.png`、`props/manor_bamboo.png`、
`props/manor_fence.png`、`props/manor_jar.png` —— 通用的树/竹/篱/缸一件多用。
这三张的**拆层登记也在 `layer_kit` 里**，归 `gen_manor_art` 切（`OWNER_BY_PREFIX`），
所以本工具不切 manor_* 那几张：谁画整图谁切块，村庄只是借用。

引擎硬约束（与山庄一致）：
  * 建筑的 quad 由 `scenes/village.scene` 的 `Sprite size=` 声明，改尺寸要同步改场景；
  * 建筑/道具的碰撞靠 GroundMap 的 solid 位，sprite 本身不参与碰撞，
    而那一串 solid 由 `tools/building_kit.py` 从**不透明像素**反推（门洞除外）。

依赖：Python 3 标准库 + pixel_kit.py + terrain_kit.py（色阶表）+ layer_kit.py（拆层）
+ gen_manor_art.py 的通用绘制工具。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import Canvas, draw_text, hash_stream, mix, rgb, shade  # noqa: E402
import layer_kit as layers  # noqa: E402  拆层登记表（哪些屋宇要切遮断块、切在第几行）
import terrain_kit as tk  # noqa: E402  M72-11：村庄四族 + 水的唯一取色口径
from gen_manor_art import (  # noqa: E402  复用通用绘制工具与山庄自有的装饰色
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
    WOOD,
    WOOD_DARK,
    WOOD_LIGHT,
    WOOD_MID,
    fill_ellipse,
    roof_rows,
    tiled_roof,
    wood_post,
)
from cjk_text import draw_plaque, prewarm  # noqa: E402  中文匾额（GDI+ 栅格化）


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

# ---------------------------------------------------------------------------
# 色板：村庄四族 + 水从 `terrain_ramp.json` 的 `scenes.village` 取，其余是本工具自有装饰色
# ---------------------------------------------------------------------------

# M72-11 起本文件不再现写材质 hex（M72-04 给山庄定的规矩，铺到村庄）：凡是被地表图集
# 用到的颜色，唯一出处是那张表。**图集与屋宇共用同一个族**才是关键——改前院墙压顶用表里的
# `slate`（#33424D）、屋顶却用本文件写死的 #3A444C，同一张图里两种"黛瓦"，玩家看不出、
# 但"一个材质一个色"这条已经破了。索引 = 该族"深→浅"的第几档，单调性由
# `gen_terrain_tiles.check()` 断言。
RAMP = tk.load_ramp("village")


def ramp(family, index):
    """色阶表里某材质族的一档（0=最深）。表里没有这一族就直接报错，不兜底。"""
    return rgb(tk.family_hexes(RAMP, family)[index])


GRASS_V = ramp("grass", 1)          # 山谷野草：比山庄深
GRASS_V_DARK = ramp("grass", 0)
GRASS_V_LIGHT = ramp("grass", 2)
GRASS_V_HI = ramp("grass", 3)

DIRT_V = ramp("tea_soil", 1)        # 茶园/土路：偏沙
DIRT_V_DARK = ramp("tea_soil", 0)
DIRT_V_LIGHT = ramp("tea_soil", 2)

FLAG = ramp("flagstone", 1)         # 青石板街：暖灰、磨损
FLAG_LIGHT = ramp("flagstone", 2)
FLAG_DARK = ramp("flagstone", 0)

WHITEWASH = ramp("plaster", 2)      # 徽派白墙
WHITEWASH_DARK = ramp("plaster", 1)

SLATE = ramp("slate", 1)            # 黛瓦（与图集的院墙压顶同族）
SLATE_LIGHT = ramp("slate", 2)
SLATE_DARK = ramp("slate", 0)
# `tiled_roof` 要的三档 (瓦面, 高光, 暗部)。不传就走山庄的 `roof` 族——那是"瓦"这个材质在
# 色阶表里的山庄写法，村庄的瓦是 `slate` 族，两套各写死一份就会出现同一张图里两种黛瓦。
SLATE_TILES = (SLATE, SLATE_LIGHT, SLATE_DARK)

STREAM = ramp("water", 1)           # 溪水（与山庄池塘共用同一族水色，流向不同）
STREAM_LIGHT = ramp("water", 2)
STREAM_HI = ramp("water", 3)
STREAM_DEEP = ramp("water", 0)

# 下面四组**不在色阶表里**，是本工具自有的细节色：各只出现在 1~2px 的车辙、污渍、
# 茅草与炉火上，不参与任何一族的明度梯度（山庄侧的同位角色是 `PLASTER_DARK`、`WOOD`）。
# 表里有这些族的时候应并进去——`gen_terrain_tiles` 的"图集色必须全在表内"那条门管的是图集。
FLAG_CRACK = rgb("#6A6358")         # 石板缝与车辙
WHITEWASH_STAIN = rgb("#C2BBAA")    # 白墙水渍
THATCH = rgb("#B99A5E")             # 茅草（民居 B）
THATCH_DARK = rgb("#96793F")
FIRE = rgb("#E07B3A")               # 炉火
FIRE_HI = rgb("#F2C05A")

BUILDING_DIR = os.path.join("assets", "art", "buildings")
PROP_DIR = os.path.join("assets", "art", "props")
# 本工具产出的遮断块由 `layer_kit` 按 stem 决定落在哪个目录，这里只登记"哪些图归本工具切"。
VILLAGE_STEMS = layers.stems_of("gen_village_art")


# ---------------------------------------------------------------------------
# 建筑
# ---------------------------------------------------------------------------


def white_wall_house(width, height, *, gable=True, windows=2, door=True, banner=False):
    """徽派白墙黛瓦民居骨架：马头墙 + 悬山顶 + 木格窗 + 板门。"""
    cv = Canvas(width, height)
    roof_top = 6
    roof_bottom = int(height * 0.52)
    rows = roof_rows(roof_top, roof_bottom, 18, width - 18, 4, width - 4)
    tiled_roof(cv, rows, column=14, palette=SLATE_TILES)
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
    """药庐 192×160：白墙黛瓦 + 马头墙 + 门楣匾额「药庐」。"""
    cv = white_wall_house(192, 160, gable=True, windows=2, door=True, banner=True)
    draw_plaque(cv, 64, 90, 128, 108, "药庐", size=13)
    return cv


def building_shop():
    """杂货铺 192×160：木板门面 + 货架 + 挑檐 + 招幌。"""
    cv = Canvas(192, 160)
    rows = roof_rows(6, 78, 16, 176, 2, 189)
    tiled_roof(cv, rows, column=14, palette=SLATE_TILES)
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
    # 货架与货箱（下移 6px，给门楣匾额让位）
    cv.rect(28, 100, 80, 126, WOOD_DARK)
    for y in (108, 118):
        cv.hline(28, 80, y, WOOD)
        for x in range(32, 78, 12):
            cv.rect(x, y - 8, x + 8, y - 1, GOLD if (x // 12) % 2 else CINNABAR)
    cv.rect(100, 102, 128, 128, WOOD)
    cv.rect(134, 106, 162, 128, WOOD_LIGHT)
    # 招幌
    bx = 156
    cv.vline(bx, 80, 104, WOOD_DARK)
    cv.rect(bx - 8, 100, bx + 8, 130, CINNABAR)
    cv.hline(bx - 8, bx + 8, 100, GOLD)
    cv.hline(bx - 8, bx + 8, 130, GOLD)
    # 门楣匾额「杂货铺」
    draw_plaque(cv, 52, 84, 140, 108, "杂货铺", size=13)
    # 台基
    cv.rect(6, 138, 185, 150, STONE)
    cv.hline(6, 185, 138, STONE_LIGHT)
    cv.hline(6, 185, 150, STONE_MORTAR)
    for x in range(14, 180, 24):
        cv.vline(x, 139, 149, STONE_MORTAR)
    cv.outline(INK)
    return cv


def building_forge():
    """铁匠铺 128×64：半开放工棚 + 炉火 + 铁砧 + 柴堆 + 匾额「铁匠铺」。"""
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
    draw_plaque(cv, 52, 30, 104, 44, "铁匠铺", size=11)
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
    tiled_roof(cv, rows, column=10, palette=SLATE_TILES)
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
    """村口牌坊 96×96（3×3 格）：门楼瓦顶 + 额枋匾额「云泽村」+ **真门洞**。

    照 M72-07 南门的模板画：`building_kit.SCENES["village"].openings` 声明 (19,1) 是人穿过去
    的那一格（`data/maps.lua` 的村↔山庄出口），图上那一格 = 图像行 32..63、列 32..63，
    不透明占比必须 ≤ 0.35。旧版把「云泽村」匾画在行 38..60 的通道正中（实测 0.84）：
    玩家看到的是一扇关着的门，走不过去。所以匾挂在**额枋**（行 16..31，全在通道格之上，
    那一格 (19,2) 是门楼的脸、本来就该画满），通道口只留一道檐内阴影。
    """
    cv = Canvas(96, 96)
    # 屋顶：正脊 + 两端起翘，檐口压到行 15（额枋以上）
    rows = roof_rows(4, 13, 14, 82, 0, 95)
    tiled_roof(cv, rows, column=12, palette=SLATE_TILES)
    cv.rect(10, 0, 86, 5, SLATE_LIGHT)
    cv.hline(10, 86, 0, SLATE_DARK)
    cv.rect(2, 0, 14, 10, SLATE_DARK)
    cv.rect(82, 0, 94, 10, SLATE_DARK)
    cv.hline(0, 95, 14, SLATE_DARK)
    cv.hline(0, 95, 15, shade(SLATE_DARK, 0.8))
    # 额枋 + 匾额「云泽村」（行 16..31 = cell (19,2)，全在通道格 32..63 之外）
    cv.rect(6, 16, 89, 31, WOOD_MID)
    cv.hline(6, 89, 16, WOOD_LIGHT)
    cv.hline(6, 89, 31, WOOD_DARK)
    draw_plaque(cv, 18, 16, 78, 31, "云泽村", size=12)
    # 檐内阴影：通道顶上一道，读成"门里面有进深"，只吃掉通道格约 12% 的面积
    cv.rect(20, 32, 76, 35, shade(INK, 1.6))
    # 立柱（通道在列 32..63 之间，柱脚以下留门槛）
    wood_post(cv, 6, 32, 20, 83)
    wood_post(cv, 76, 32, 90, 83)
    # 灯笼挂在两柱内侧（列 22..30 / 66..74，都在通道格之外）
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
    # 中文招牌先批量栅格化（一次 PowerShell 调用）
    prewarm([("药庐", 13, "pixel", True), ("杂货铺", 13, "pixel", True),
             ("铁匠铺", 11, "pixel", True), ("云泽村", 12, "pixel", True)])
    all_sprites = buildings()
    all_sprites.update(props())
    for name, sprite in buildings().items():
        write_png(os.path.join(BUILDING_DIR, f"village_{name}.png"), sprite)
    for name, sprite in props().items():
        write_png(os.path.join(PROP_DIR, f"village_{name}.png"), sprite)
    write_png(os.path.join(BUILDING_DIR, "_village_preview.png"),
              build_sprite_preview(all_sprites))
    # 整图刚落盘就按行切出 roof/body：分块与整图同源同刻（`check_splits` 逐字节对拍）。
    # `village_gate` 不在这张表里（它是通道不是屋子，见 `layer_kit` 那条注释）。
    splits = layers.write_splits(VILLAGE_STEMS)
    if not quiet:
        for name, sprite in sorted(all_sprites.items()):
            print(f"sprite   village_{name}.png {sprite.w}x{sprite.h}")
        for stem, path, rows in splits:
            print(f"split    {path} {rows} 行")
        print(f"preview  {BUILDING_DIR}/_village_preview.png")


def check():
    issues = []
    for name in buildings():
        p = os.path.join(BUILDING_DIR, f"village_{name}.png")
        if not os.path.exists(p):
            issues.append("missing " + p)
    for name in props():
        p = os.path.join(PROP_DIR, f"village_{name}.png")
        if not os.path.exists(p):
            issues.append("missing " + p)
    if not os.path.exists(os.path.join(BUILDING_DIR, "_village_preview.png")):
        issues.append("missing " + os.path.join(BUILDING_DIR, "_village_preview.png"))
    # 拆层对拍**查整张表**（含 manor/build 那十三张）：谁改了别人的原图而没重切，
    # 只跑本工具的 --check 也当场红，不必几个生成器都跑过一遍。
    issues.extend(layers.check_splits())
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
