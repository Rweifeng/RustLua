# -*- coding: utf-8 -*-
"""《江湖有田》批次 2 美术：青云山道 + 寒泉洞 + 山道建筑道具 + 采集物。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/gen_wilds_art.py            # 生成全部 + 预览
    python tools/gen_wilds_art.py --check    # 只校验尺寸/存在性

产出：

    （**两张地表都不在本文件出图**：M72-11 起山道归 `gen_terrain_tiles.py --scene mountain_path`、
      寒泉洞归 `gen_terrain_tiles.py --scene cold_spring_cave`，各产出自己的 12×9 图集
      `assets/art/tilesets/<场景>/<场景>_ground.png`）
    assets/art/buildings/wilds_*.png                  山门 / 栈道 / 茶棚 / 营寨 / 断桥
    assets/art/props/wilds_*.png                      松树 / 山石 / 藤蔓 / 火把 / 路牌 / 营火 / 木箱 / 树桩
    assets/art/gather/wilds_*.png                     9 种采集物（野果丛 / 药草 / 柴薪 / 矿石 / 雪莲 / 芦苇 / 莲蓬 / 毒草 / 茶树丛）
    assets/art/buildings/_wilds_preview.png           建筑道具 2× 预览
    assets/art/gather/_preview.png                    采集物 3× 预览（中文名）
    assets/art/props/wilds_pine_{canopy,trunk}.png     松树按行切的两块（切点见 `layer_kit`）
    assets/art/buildings/wilds_tea_shed_{roof,body}.png 茶棚按行切的两块

拆层（M72-11）：`wilds_pine` 与 `wilds_tea_shed` 两张整图归本文件切，切点登记在
`layer_kit.LAYER_SPLITS`（`stems_of("gen_wilds_art")`），`generate()` 末尾按行切出
`*_canopy/_trunk`、`*_roof/_body` 四张分块，`check()` 连带跑 `check_splits()` 的逐字节等式。
整张原图**不删也不改**：场景生成器登记的就是整图，装配时由 `set_foreground_occluders`
把它换成 body 块并另挂一件 canopy/roof；而原图正是 `check_splits` 那条等式的被减数。

对应 docs/31 §3.1 的"山林 2 套 / 洞窟 1 套"与 §7.1 的采集区域；
对应 docs/30 §7.1 的四种采集区域（村外湿地 / 青云山道 / 青云山腰 / 寒泉洞）。

kind 语义（**两张地表都不再由本文件出图**，M72-11 起）：
  mountain：`mountain_path/mountain_path_ground.png` 12×9 图集 + autotile，由
            `gen_terrain_tiles.py --scene mountain_path` 出图，色阶只从 `tools/terrain_ramp.json`
            的 `scenes.mountain_path` 取——1=落叶松林地 2=碎石土路 3=石阶 4=岩壁 5=竹林地 6=溪水
  cave    ：`cold_spring_cave/cold_spring_cave_ground.png`，同一命令换 `--scene`；
            kind 语义已换序（旧说法 1=洞底石地 2=碎石堆 3=暗河水 4=洞壁 5=矿脉 6=冰晶地 →
            新实测 1=洞底石板 2=塌落碎石(零格) 3=凿平甬道 4=洞壁 5=冰晶地 6=暗河），
            逐条理由与"为什么水必须是 6、墙必须是 4"写在 `gen_cave_scene.py` 的模块 docstring。
            旧那张 6 列 × 1 行的直接寻址图与它的 `_preview.png` 随本批从产出表里摘掉
            （`assets/art/tilesets/cave/` 整目录不再存在，盘上不留第二套地表真相）。

文字：路牌 / 山门匾额 / 茶棚幌子 / 营寨旗 都用**中文**（走 tools/cjk_text.py 的 GDI+ 栅格化）。

依赖：Python 3 标准库 + pixel_kit.py + gen_manor_art.py（通用绘制）+ cjk_text.py（中文）
+ layer_kit.py（拆层登记表）。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import Canvas, draw_text, hash_stream, mix, rgb, shade  # noqa: E402
from gen_manor_art import (  # noqa: E402
    BAMBOO, BAMBOO_DARK, BAMBOO_LIGHT, CINNABAR, GOLD, GOLD_LIGHT, INK, PAPER, PAPER_DIM,
    STONE, STONE_DARK, STONE_LIGHT, STONE_MORTAR, TILE, TILE_ROOF, TILE_ROOF_DARK,
    TILE_ROOF_LIGHT, WATER, WOOD, WOOD_DARK, WOOD_LIGHT, WOOD_MID,
    fill_ellipse, roof_rows, tiled_roof, wood_post,
)
from cjk_text import draw_cjk, draw_plaque, prewarm  # noqa: E402
import layer_kit as layers  # noqa: E402  拆层登记表（松树/茶棚哪一行是切点、归谁切）

# 山道色板
FOREST_DARK = rgb("#3C5430")
STAIR = rgb("#948C7C")
STAIR_DARK = rgb("#6E675B")
ROCK = rgb("#6E6B62")
ROCK_LIGHT = rgb("#8A867C")
ROCK_DARK = rgb("#4E4B44")
PINE = rgb("#3A5230")
PINE_DARK = rgb("#2C3F24")
PINE_LIGHT = rgb("#4C6A3C")
VINE = rgb("#5B7048")

# 洞窟色板：只留**采集物**还需要的两组（`wilds_ore` 的矿砂、`wilds_snow_lotus` 的冰面），
# 洞窟地表那六族现在住在 `tools/terrain_ramp.json:scenes.cold_spring_cave.families`。
ORE = rgb("#C9A05A")
ORE_LIGHT = rgb("#E4C078")
ICE = rgb("#A9C4D8")
ICE_LIGHT = rgb("#D6E6F2")

TILE_DIR = os.path.join("assets", "art", "tilesets")
BUILDING_DIR = os.path.join("assets", "art", "buildings")
PROP_DIR = os.path.join("assets", "art", "props")
GATHER_DIR = os.path.join("assets", "art", "gather")


# ---------------------------------------------------------------------------
# 山道建筑 / 道具 / 采集物（**两张地表图集都不再由本文件出图**：金标规范下图集的唯一
# 产出点是 `gen_terrain_tiles.py --scene <场景>`，这里再画一张 6×1 会把那张门的三条判据
# 直接压红。M72-11 起山道与寒泉洞都搬过去了，本文件旧的那六格 `tile_cave_*` 与
# `cave_tiles()/pack()` 一起摘除。）
# ---------------------------------------------------------------------------



# ---------------------------------------------------------------------------
# 山道建筑（匾额/幌子用中文）
# ---------------------------------------------------------------------------


def build_mountain_gate():
    """山门 128×96：木牌坊 + 额枋匾「青云山」+ **真门洞**（M72-11 重画）。

    这张图被**两张**场景引用，所以它得同时把两条通道都留出来（贴图左下角在 cell 时，
    图像行 0..31 = cell y+2、32..63 = y+1、64..95 = y；列 0..31 = x，依此类推）：

      * 山道 `building_kit.SCENES["mountain_path"].openings["MountainGate"]` 压在 (18,0)：
        出口格 (19,1)(20,1) = 行 32..63 × 列 32..95 是通道，(19,2)(20,2) = 行 0..31 的中间
        两格是门前的檐下；出生点 (19,2) 就在其中一格。
      * 村侧北山门 `SCENES["village"].openings["VillageMountainGate"]` 压在 (9,20)：那张图
        的通道是**最底一行** (10,20) = 行 64..95 × 列 32..63，上面两行是门楼的身与顶。

    旧版把「青云山」匾画在行 38..62、列 34..94 的通道正中（实测不透明占比 0.940）：
    山道侧玩家看到的是一扇关着的门，走不过去。现在匾挂在**额枋**（行 16..31，走
    `cjk_text.draw_plaque` 这一处共用实现，与云泽村牌楼/各铺面招牌同一套木底金边），
    通道行 32..63 只留一道檐内阴影。改后两处门洞各自实测 0.156 ≤ `HOLE_MAX_OPAQUE_RATIO`
    （0.35）：山道 (19,1) 与村 (10,20) 都是"檐口 4 行 + `outline()` 外扩的 1 行" = 5/32。
    紧包围盒还是行 0..95 × 列 1..126（屋顶起翘够到行 0、台基够到行 95、列 2..125），
    所以村侧那 12 格占地 / 11 格实心一字未动，`village.scene` 的 solid 串不受影响。
    """
    cv = Canvas(128, 96)
    # 屋顶（行 0..15，全在两条通道之上）：正脊 + 两端起翘，檐口收在行 14/15
    rows = roof_rows(3, 13, 22, 106, 8, 120)
    tiled_roof(cv, rows, column=12)
    cv.rect(20, 2, 108, 4, TILE_ROOF_LIGHT)
    cv.hline(20, 108, 2, TILE_ROOF)
    cv.rect(2, 0, 16, 9, TILE_ROOF_DARK)
    cv.rect(112, 0, 126, 9, TILE_ROOF_DARK)
    cv.hline(2, 125, 14, TILE_ROOF_DARK)
    cv.hline(2, 125, 15, shade(TILE_ROOF_DARK, 0.8))
    # 额枋 + 匾「青云山」（行 16..31）：山道侧那是门楼的脸、村侧那是门楼的身，都该画满
    cv.rect(6, 16, 121, 31, WOOD_MID)
    cv.hline(6, 121, 16, WOOD_LIGHT)
    cv.hline(6, 121, 31, WOOD_DARK)
    draw_plaque(cv, 30, 17, 97, 30, "青云山", size=12)
    # 檐内阴影：通道顶上一道，读成"门里有进深"。它连同 outline 外扩的那一行是门洞里
    # 唯一的不透明像素（5/32 = 0.156），再厚就把通道糊死了。
    cv.rect(32, 32, 95, 35, shade(INK, 1.6))
    # 立柱：列 6..20 / 107..121 —— 落在 cell x 与 x+3 两格，通道那两格中间没有柱
    wood_post(cv, 6, 32, 20, 91)
    wood_post(cv, 107, 32, 121, 91)
    # 石台基（行 92..95）：村侧的通道格就是被这一条压着的
    cv.rect(2, 92, 125, 95, STONE)
    cv.hline(2, 125, 92, STONE_LIGHT)
    cv.hline(2, 125, 95, STONE_MORTAR)
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


def prop_stump():
    """树桩 32×28：`gather.lua:wild_timber_1`（松木桩）的标志物。

    盘上 `TreeStump`（id 32）以前引的是整张 `wilds_pine.png` 并按 64×88 声明——一棵完整的
    松树标成"木桩"，玩家对着树冠找采伐点。采伐点得有自己的图。
    """
    cv = Canvas(32, 28)
    cv.rect(8, 12, 23, 26, WOOD_DARK)            # 桩身
    for x in (10, 15, 20):                       # 树皮竖纹
        cv.vline(x, 15, 26, shade(WOOD_DARK, 0.8))
    fill_ellipse(cv, 16, 12, 7, 4, WOOD_MID)     # 断面
    fill_ellipse(cv, 16, 12, 4, 2, WOOD_LIGHT)   # 年轮
    cv.set(16, 12, WOOD_DARK)
    cv.hline(8, 23, 26, shade(WOOD_DARK, 0.8))   # 桩脚
    for x, y in ((5, 24), (25, 23), (27, 25)):   # 脚边苔
        cv.set(x, y, BAMBOO_DARK)
        cv.set(x + 1, y, BAMBOO)
    cv.outline(INK)
    return cv


# ---------------------------------------------------------------------------
# 采集物（9 种）
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


def gather_tea_bush():
    """茶树丛 32×26：`gather.lua:wild_tea_1` 的标志物。

    以前 `TeaGrove` 复用的是 `wilds_herb.png`（药草丛），而 `HerbCluster` 就在同一张图里
    七格之外——同一屏上两棵一模一样的灌木，一个采灵芝一个采茶叶。茶丛的形状语言是
    **矮、宽、剪平**（采茶要够得着），药草是**直茎高挑**，两者必须一眼分得开。
    """
    cv = Canvas(32, 26)
    fill_ellipse(cv, 16, 15, 12, 8, BAMBOO_DARK)
    fill_ellipse(cv, 16, 14, 10, 6, BAMBOO)
    cv.hline(8, 24, 8, BAMBOO_LIGHT)          # 剪平的顶
    for x in (10, 16, 22):
        cv.vline(x, 9, 11, BAMBOO_LIGHT)      # 顶上一茬嫩梢
    for x, y in ((12, 14), (19, 16), (25, 18)):
        cv.set(x, y, PAPER)                   # 茶花（白花是"这是茶树"最省的一笔）
        cv.set(x + 1, y, PAPER_DIM)
    cv.hline(9, 23, 24, rgb("#5A4632"))       # 根部土线
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
    ("wilds_stump", prop_stump, "树桩"),
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
    ("wilds_tea_bush", gather_tea_bush, "茶树丛"),
]

# 本生成器该切的拆层件（松树、茶棚）：`layer_kit.LAYER_SPLITS` 那张表的视图。
WILDS_STEMS = layers.stems_of("gen_wilds_art")


def write_png(path, canvas):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(canvas.to_png())


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
    prewarm([("青云山", 12, "pixel", True), ("茶", 14, "pixel", True),
             ("寨", 16, "pixel", True), ("山道", 13, "pixel", True)] +
            [(display, 11, "pixel", True) for _, _, display in BUILDINGS + PROPS + GATHERS])
    # 两张地表都**不在这里出图**（M72-11）：金标规范下图集的唯一产出点是
    # `gen_terrain_tiles.py --scene <场景>`（12×9 + 只从 `terrain_ramp.json` 取色），
    # 这里再画一张 6×1 会把那张门的三条判据直接压红。

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
    # 整图刚落盘就按行切出分块：分块与整图同源同刻（`check_splits` 逐字节对拍）。
    splits = layers.write_splits(WILDS_STEMS)
    if not quiet:
        for name, canvas, display in built + props + gathers:
            print(f"  {name}.png {canvas.w}x{canvas.h}  {display}")
        for stem, path, rows in splits:
            print(f"  split {path} {rows} 行")
        print("  2 张预览（两张地表图集都不由本文件出图，见模块 docstring）")


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
    "wilds_stump": (32, 28),
    "wilds_berry": (32, 32),
    "wilds_herb": (32, 32),
    "wilds_firewood": (40, 32),
    "wilds_ore": (40, 32),
    "wilds_snow_lotus": (32, 32),
    "wilds_reed": (32, 40),
    "wilds_lotus": (32, 32),
    "wilds_poison_herb": (32, 32),
    "wilds_tea_bush": (32, 26),
}


def check():
    from pixel_kit import decode_png

    issues = []
    # **反面对照**：旧那张 6 列 × 1 行的洞窟图（含它的预览）必须**不在盘上**。M72-11 起
    # 寒泉洞的地表由 `gen_terrain_tiles.py --scene cold_spring_cave` 出 12×9 那一张，留着 6×1
    # 就是两套地表真相——谁把它恢复回来（或把本文件的出图代码加回来），红在这里。
    # 山道地表同一条理由早已搬走，两边都不在这张表里再判尺寸。
    for stale in (os.path.join(TILE_DIR, "cave", "cave_ground.png"),
                  os.path.join(TILE_DIR, "cave", "_preview.png")):
        if os.path.exists(stale):
            issues.append(f"多出来的一套地表真相：{stale} 还在盘上——寒泉洞图集只由 "
                          "gen_terrain_tiles.py --scene cold_spring_cave 产出")
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
    for path in (os.path.join(BUILDING_DIR, "_wilds_preview.png"),
                 os.path.join(GATHER_DIR, "_preview.png")):
        if not os.path.exists(path):
            issues.append("missing " + path)
    # 拆层对拍**查整张表**（manor/village/build 那几件也一起）：谁改了山道原图而没重切，
    # 只跑本工具的 --check 也当场红，不必几个生成器都跑过一遍。
    issues.extend(layers.check_splits())
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
        print(f"[OK] 批次 2 校验通过（{len(BUILDINGS)} 建筑 + {len(PROPS)} 道具 + {len(GATHERS)} 采集物"
              f" + 拆层 {len(WILDS_STEMS)} 件 + 2 预览；地表图集归 gen_terrain_tiles.py）")
        return
    generate()


if __name__ == "__main__":
    main()
