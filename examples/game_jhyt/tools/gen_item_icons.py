# -*- coding: utf-8 -*-
"""《江湖有田》物品图标生成（docs/31 §3.3 + docs/35 §5：32×32、透明底、1px 描边、光源左上）。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/gen_item_icons.py            # 生成全部图标 + 预览
    python tools/gen_item_icons.py --check    # 只校验尺寸/存在性

产出：`assets/art/items/<id>.png`（32×32）+ `assets/art/items/_preview.png`（3× 预览 + 标签）

覆盖范围（批次 1 的 16 个图标 + M23 补齐 2 个）：

  作物产物    rice 稻米 / lingzhi 灵芝 / tea_leaf 春茶
  种子        seed_rice 稻种 / seed_lingzhi 灵芝菌种
  药剂        heal_pill 回复药 / herb_tonic 行气散
  采集物      herb 药草 / wild_fruit 野果 / lotus_seed 莲子 / reed 芦苇 / wood 柴薪 / ore 矿石
  材料        timber 木料
  工具武器    hoe 锄头 / axe 斧 / pick 镐 / blade 柴刀

命名规则：文件名 = `data/items.lua` 的物件 id（后续加物品只需在本文件补一个画法）。
**清单必须覆盖 `data/items.lua` 的每个 `icon`**：M23 之前 `herb_tonic`（M18-04）
与 `timber`（M18-02）两个物品的图标路径指向不存在的文件，背包/商店只会显示缺图
兜底；现在 `jhyt_m23_data_validation` 会逐个断言图标文件真实存在。
图标内**不写文字**（docs/35 §5），数量/等级一律走 UiText 角标。

依赖：Python 3 标准库 + pixel_kit.py。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import Canvas, draw_text, rgb, shade  # noqa: E402
from gen_manor_art import fill_ellipse  # noqa: E402  椭圆填充（跨工具复用）

SIZE = 32
OUT_DIR = os.path.join("assets", "art", "items")

INK = rgb("#1F2328")
PAPER = rgb("#F2EAD8")
PAPER_DIM = rgb("#D8CDB2")
GOLD = rgb("#C9A05A")
GOLD_LIGHT = rgb("#E4C078")
GOLD_DARK = rgb("#A8813F")
CINNABAR = rgb("#C3452F")
CINNABAR_LIGHT = rgb("#E06B50")
CINNABAR_DARK = rgb("#8F2E1F")
BAMBOO = rgb("#789262")
BAMBOO_LIGHT = rgb("#A9C28F")
BAMBOO_DARK = rgb("#5B7048")
WOOD = rgb("#8A6A46")
WOOD_LIGHT = rgb("#A6855C")
WOOD_MID = rgb("#6B4F3A")
WOOD_DARK = rgb("#4A3628")
STONE = rgb("#8E8B80")
STONE_LIGHT = rgb("#A5A197")
STONE_DARK = rgb("#6E6B62")
IRON = rgb("#C8CCD4")
IRON_DARK = rgb("#7E8391")
WATER = rgb("#7FA3D4")
EARTH = rgb("#7C5C3E")


# ---------------------------------------------------------------------------
# 通用小工具
# ---------------------------------------------------------------------------


def blob(canvas, cx, cy, rx, ry, color):
    fill_ellipse(canvas, cx, cy, rx, ry, color)


def shade_blob(canvas, cx, cy, rx, ry, base):
    """带左上高光 / 右下暗边的椭圆块（本项目的光源约定）。"""
    blob(canvas, cx, cy, rx, ry, base)
    blob(canvas, cx - rx * 0.3, cy - ry * 0.3, rx * 0.5, ry * 0.45, shade(base, 1.22))
    blob(canvas, cx + rx * 0.35, cy + ry * 0.35, rx * 0.4, ry * 0.35, shade(base, 0.78))


def stick(canvas, x0, y0, x1, y1, color, thickness=2):
    """任意方向粗线（Bresenham + 方头），用于工具柄/穗杆。"""
    steps = max(abs(x1 - x0), abs(y1 - y0)) + 1
    for step in range(steps):
        t = step / max(1, steps - 1)
        x = int(round(x0 + (x1 - x0) * t))
        y = int(round(y0 + (y1 - y0) * t))
        for dy in range(thickness):
            for dx in range(thickness):
                canvas.set(x + dx, y + dy, color)


# ---------------------------------------------------------------------------
# 作物产物
# ---------------------------------------------------------------------------


def icon_rice():
    """稻米：一束金穗 + 麻绳。"""
    cv = Canvas(SIZE, SIZE)
    stick(cv, 16, 28, 16, 10, GOLD_DARK, 2)
    for side in (-1, 1):
        for step in range(4):
            y = 9 + step * 4
            x = 16 + side * 4
            shade_blob(cv, x, y, 3, 2, GOLD)
            cv.set(x - 1, y - 1, GOLD_LIGHT)
        del side
    shade_blob(cv, 16, 8, 3, 3, GOLD)
    cv.set(15, 7, GOLD_LIGHT)
    stick(cv, 11, 24, 21, 24, WOOD_MID, 2)
    cv.outline(INK)
    return cv


def icon_lingzhi():
    """灵芝：红褐菌盖 + 环纹菌柄。"""
    cv = Canvas(SIZE, SIZE)
    stick(cv, 16, 28, 16, 16, rgb("#B98A5A"), 4)
    for y in (20, 24):
        cv.hline(13, 19, y, rgb("#8A5F38"))
    blob(cv, 16, 14, 11, 6, CINNABAR_DARK)
    blob(cv, 16, 12, 10, 5, CINNABAR)
    cv.hline(7, 25, 10, CINNABAR_LIGHT)
    cv.hline(9, 23, 13, CINNABAR_DARK)
    cv.set(11, 9, rgb("#F0A08A"))
    cv.outline(INK)
    return cv


def icon_tea_leaf():
    """春茶：三片嫩叶 + 芽尖。"""
    cv = Canvas(SIZE, SIZE)
    stick(cv, 16, 28, 16, 14, BAMBOO_DARK, 2)
    for side, y in ((-1, 14), (1, 18), (-1, 22)):
        cx = 16 + side * 6
        blob(cv, cx, y, 6, 3, BAMBOO)
        cv.hline(cx - 5, cx + 1 if side < 0 else cx + 5, y - 2, BAMBOO_LIGHT)
        cv.set(cx + side * 4, y - 1, BAMBOO_LIGHT)
    blob(cv, 16, 10, 2, 3, BAMBOO_LIGHT)
    cv.outline(INK)
    return cv


# ---------------------------------------------------------------------------
# 种子
# ---------------------------------------------------------------------------


def icon_seed_rice():
    """稻种：纸包 + 溢出的三粒谷。"""
    cv = Canvas(SIZE, SIZE)
    cv.rect(8, 12, 23, 27, PAPER)
    cv.rect(8, 12, 23, 15, PAPER_DIM)
    cv.vline(8, 12, 27, PAPER_DIM)
    cv.vline(23, 12, 27, PAPER_DIM)
    cv.hline(8, 23, 27, PAPER_DIM)
    stick(cv, 8, 14, 23, 14, BAMBOO, 1)
    for x, y in ((12, 20), (16, 24), (20, 21)):
        shade_blob(cv, x, y, 2, 2, GOLD)
    for x, y in ((24, 16), (26, 19)):
        cv.set(x, y, GOLD)
    cv.outline(INK)
    return cv


def icon_seed_lingzhi():
    """菌种：陶罐 + 纸封 + 孢子点。"""
    cv = Canvas(SIZE, SIZE)
    for y in range(10, 28):
        t = (y - 10) / 18.0
        half = 6 + int(round(4 * (1 - abs(t - 0.35) * 1.6)))
        half = max(6, min(10, half))
        cv.hline(16 - half, 16 + half, y, rgb("#9A7A5A") if (y // 3) % 2 else rgb("#8A6A4A"))
    cv.rect(8, 8, 24, 12, PAPER)
    cv.hline(8, 24, 8, PAPER_DIM)
    cv.rect(11, 14, 21, 20, rgb("#4A3A2A"))
    for x, y in ((13, 16), (17, 18), (20, 15)):
        cv.set(x, y, CINNABAR)
    cv.outline(INK)
    return cv


# ---------------------------------------------------------------------------
# 药剂
# ---------------------------------------------------------------------------


def icon_heal_pill():
    """回复药：药囊 + 朱砂印（不写字）+ 一颗药丸。"""
    cv = Canvas(SIZE, SIZE)
    cv.rect(7, 13, 23, 28, PAPER)
    cv.hline(7, 23, 13, PAPER_DIM)
    cv.vline(7, 13, 28, PAPER_DIM)
    cv.rect(11, 17, 19, 25, CINNABAR)
    cv.hline(12, 18, 19, CINNABAR_LIGHT)
    cv.hline(12, 18, 23, CINNABAR_DARK)
    stick(cv, 7, 15, 23, 15, WOOD_MID, 1)
    shade_blob(cv, 24, 10, 4, 4, rgb("#E8E0D0"))
    cv.set(23, 9, rgb("#FFFFFF"))
    cv.outline(INK)
    return cv


# ---------------------------------------------------------------------------
# 采集物
# ---------------------------------------------------------------------------


def icon_herb():
    """药草：三株捆扎的草药。"""
    cv = Canvas(SIZE, SIZE)
    for x, top in ((12, 8), (16, 5), (20, 9)):
        stick(cv, x, 26, x, top, BAMBOO_DARK, 2)
        for side in (-1, 1):
            for step in range(3):
                y = top + 3 + step * 4
                cv.set(x + side * (2 + step), y, BAMBOO_LIGHT if side < 0 else BAMBOO)
                cv.set(x + side * (3 + step), y + 1, BAMBOO)
    stick(cv, 10, 24, 22, 24, WOOD_MID, 2)
    cv.outline(INK)
    return cv


def icon_wild_fruit():
    """野果：两枚红果 + 叶。"""
    cv = Canvas(SIZE, SIZE)
    stick(cv, 16, 10, 16, 16, WOOD_MID, 2)
    cv.set(13, 9, BAMBOO)
    cv.set(19, 9, BAMBOO)
    cv.set(20, 8, BAMBOO_LIGHT)
    shade_blob(cv, 12, 20, 6, 6, CINNABAR)
    shade_blob(cv, 21, 22, 5, 5, CINNABAR_DARK)
    cv.set(10, 18, CINNABAR_LIGHT)
    cv.set(20, 20, CINNABAR_LIGHT)
    cv.outline(INK)
    return cv


def icon_lotus_seed():
    """莲子：莲蓬 + 两粒莲子。"""
    cv = Canvas(SIZE, SIZE)
    stick(cv, 16, 28, 16, 20, BAMBOO_DARK, 2)
    blob(cv, 16, 15, 9, 7, BAMBOO)
    cv.hline(8, 24, 10, BAMBOO_LIGHT)
    for x, y in ((12, 13), (16, 11), (20, 13), (14, 17), (18, 17)):
        cv.set(x, y, BAMBOO_DARK)
        cv.set(x, y - 1, rgb("#3E5230"))
    shade_blob(cv, 25, 24, 3, 4, rgb("#D8CDB2"))
    cv.outline(INK)
    return cv


def icon_reed():
    """芦苇：两根带穗的芦苇。"""
    cv = Canvas(SIZE, SIZE)
    for x, top in ((13, 6), (20, 10)):
        stick(cv, x, 28, x, top + 6, rgb("#9A8B5A"), 2)
        blob(cv, x, top + 3, 3, 5, rgb("#C9B47A"))
        for step in range(3):
            cv.set(x - 2, top + 2 + step * 2, rgb("#E0D0A0"))
            cv.set(x + 2, top + 3 + step * 2, rgb("#B8A36A"))
        cv.set(x, top - 2, rgb("#E0D0A0"))
    cv.hline(6, 26, 29, BAMBOO_DARK)
    cv.outline(INK)
    return cv


def icon_wood():
    """柴薪：三根捆好的圆木。"""
    cv = Canvas(SIZE, SIZE)
    for cx, cy in ((12, 22), (21, 22), (16, 15)):
        blob(cv, cx, cy, 6, 5, WOOD)
        cv.hline(cx - 5, cx + 5, cy - 4, WOOD_LIGHT)
        blob(cv, cx, cy, 3, 2, WOOD_DARK)
        cv.set(cx - 1, cy, WOOD_LIGHT)
    stick(cv, 8, 25, 24, 25, WOOD_DARK, 1)
    stick(cv, 10, 19, 22, 19, WOOD_DARK, 1)
    cv.outline(INK)
    return cv


def icon_ore():
    """矿石：石头 + 金铁脉。"""
    cv = Canvas(SIZE, SIZE)
    blob(cv, 16, 19, 11, 9, STONE)
    cv.hline(6, 26, 11, STONE_LIGHT)
    blob(cv, 21, 22, 5, 5, STONE_DARK)
    for x, y in ((11, 16), (14, 20), (19, 17), (16, 23)):
        cv.set(x, y, GOLD)
        cv.set(x + 1, y, GOLD_LIGHT)
    cv.set(13, 15, IRON)
    cv.outline(INK)
    return cv


# ---------------------------------------------------------------------------
# 工具与武器
# ---------------------------------------------------------------------------


def icon_hoe():
    cv = Canvas(SIZE, SIZE)
    stick(cv, 9, 27, 22, 10, WOOD, 3)
    stick(cv, 9, 27, 12, 23, WOOD_LIGHT, 1)
    cv.rect(18, 6, 28, 10, STONE_DARK)
    cv.rect(18, 6, 28, 7, IRON)
    cv.hline(18, 28, 10, IRON_DARK)
    cv.outline(INK)
    return cv


def icon_axe():
    cv = Canvas(SIZE, SIZE)
    stick(cv, 9, 27, 21, 9, WOOD, 3)
    cv.rect(17, 5, 27, 15, IRON_DARK)
    cv.rect(17, 5, 25, 9, IRON)
    cv.hline(20, 27, 15, IRON_DARK)
    cv.set(26, 6, rgb("#FFFFFF"))
    cv.outline(INK)
    return cv


def icon_pick():
    cv = Canvas(SIZE, SIZE)
    stick(cv, 16, 28, 16, 10, WOOD, 3)
    for side in (-1, 1):
        for step in range(6):
            cv.set(16 + side * (2 + step), 8 + step, IRON if step % 2 else IRON_DARK)
            cv.set(16 + side * (2 + step), 9 + step, IRON_DARK)
    cv.set(16, 7, IRON)
    cv.outline(INK)
    return cv


def icon_blade():
    """柴刀：弯背刀身 + 木柄。"""
    cv = Canvas(SIZE, SIZE)
    stick(cv, 8, 26, 14, 20, WOOD_MID, 3)
    cv.set(7, 27, WOOD_DARK)
    for step in range(12):
        x = 13 + step
        y = 20 - step
        cv.set(x, y, IRON)
        cv.set(x, y + 1, IRON_DARK)
        if step > 2:
            cv.set(x, y - 1, IRON)
    cv.set(26, 8, IRON_DARK)
    cv.set(13, 18, rgb("#FFFFFF"))
    cv.outline(INK)
    return cv


def icon_timber():
    """木料（M18-02 建筑升级材料）：三根方料叠放，与柴薪的圆木区分。"""
    cv = Canvas(SIZE, SIZE)
    for x0, x1, y0, y1 in ((5, 27, 20, 26), (8, 24, 12, 18), (11, 21, 4, 10)):
        cv.rect(x0, y0, x1, y1, WOOD)
        cv.hline(x0, x1, y0, WOOD_LIGHT)
        cv.hline(x0, x1, y1, WOOD_DARK)
        cv.vline(x0, y0, y1, WOOD_LIGHT)
        cv.vline(x1, y0, y1, WOOD_DARK)
        # 木纹：截面上的两三道短线，读起来是"刨过的方料"而不是圆木
        mid = (x0 + x1) // 2
        cv.hline(mid - 4, mid + 4, (y0 + y1) // 2, WOOD_MID)
    cv.outline(INK)
    return cv


def icon_herb_tonic():
    """行气散（M18-04 临时 Buff 药）：纸包 + 朱砂印 + 溢出的一撮药末。"""
    cv = Canvas(SIZE, SIZE)
    # 纸包主体（上窄下宽的折角袋）
    cv.rect(9, 12, 23, 28, PAPER)
    for index, x in enumerate(range(9, 24)):
        cv.set(x, 11, PAPER_DIM if index % 2 == 0 else PAPER)
    cv.hline(9, 23, 28, PAPER_DIM)
    cv.vline(9, 12, 28, PAPER_DIM)
    cv.vline(23, 12, 28, PAPER_DIM)
    # 折口：两道斜压痕
    cv.hline(11, 21, 15, PAPER_DIM)
    cv.hline(12, 20, 17, PAPER_DIM)
    # 朱砂印（不写字，docs/35 §5）
    cv.rect(13, 20, 19, 25, CINNABAR)
    cv.rect(14, 21, 18, 24, CINNABAR_LIGHT)
    cv.set(15, 22, CINNABAR_DARK)
    cv.set(17, 23, CINNABAR_DARK)
    # 顶上一撮药末 + 一片草叶
    cv.set(15, 8, BAMBOO_DARK)
    cv.set(16, 7, BAMBOO)
    cv.set(17, 8, BAMBOO_LIGHT)
    cv.set(14, 9, BAMBOO_DARK)
    cv.hline(13, 19, 10, BAMBOO_DARK)
    cv.hline(14, 18, 9, BAMBOO)
    cv.outline(INK)
    return cv


def icon_wolf_pelt():
    """狼皮（M28 掉落物）：摊开的整皮，四爪 + 尾部，灰白毛边。"""
    cv = Canvas(SIZE, SIZE)
    fur = rgb("#8C8F98")
    fur_light = rgb("#B9BCC4")
    fur_dark = rgb("#5E6169")
    # 皮张主体：上窄下宽的梯形
    for y in range(7, 27):
        t = (y - 7) / 19.0
        half = int(round(5 + 8 * t))
        cv.hline(16 - half, 16 + half, y, fur)
    cv.hline(11, 21, 8, fur_light)
    # 四爪：左右各两只小凸起
    for x, y in ((7, 14), (25, 14), (8, 23), (24, 23)):
        blob(cv, x, y, 3, 2, fur_dark)
    # 尾部：右下伸出的一截
    stick(cv, 22, 24, 28, 27, fur_dark, 2)
    # 毛边高光
    for x in range(9, 24, 3):
        cv.set(x, 9, fur_light)
        cv.set(x + 1, 26, fur_dark)
    cv.outline(INK)
    return cv


def icon_boar_meat():
    """兽肉（M28 掉落物）：带骨的一块鲜肉，白筋膜收边。"""
    cv = Canvas(SIZE, SIZE)
    meat = rgb("#B4544A")
    meat_light = rgb("#D2796B")
    meat_dark = rgb("#7E3229")
    blob(cv, 17, 19, 10, 8, meat)
    cv.hline(9, 25, 13, meat_light)
    blob(cv, 19, 23, 6, 4, meat_dark)
    # 骨头：从左上斜插出来
    stick(cv, 8, 9, 17, 16, PAPER, 3)
    blob(cv, 7, 8, 3, 2, PAPER)
    blob(cv, 9, 6, 3, 2, PAPER)
    # 筋膜纹理
    for y in (16, 19, 22):
        cv.hline(10, 24, y, meat_light)
    cv.outline(INK)
    return cv


def icon_ape_core():
    """山魈内丹（M28 Boss 掉落）：青碧丹丸 + 灵气环，主线物品。"""
    cv = Canvas(SIZE, SIZE)
    core = rgb("#6FA8A0")
    core_light = rgb("#A7D6CE")
    core_dark = rgb("#3C6B66")
    blob(cv, 16, 17, 10, 10, core_dark)
    blob(cv, 16, 17, 8, 8, core)
    blob(cv, 13, 14, 4, 3, core_light)
    # 灵气环：上下两道弧
    for x in range(6, 27):
        cv.set(x, 5 + abs(x - 16) // 2, core_light)
    for x in range(8, 25):
        cv.set(x, 29 - abs(x - 16) // 3, core_dark)
    cv.set(9, 22, core_light)
    cv.set(23, 12, core_light)
    cv.outline(INK)
    return cv


def icon_radish():
    """萝卜：白身锥形 + 尾须 + 顶叶（快熟作物，白色主体在深色背包里好认）。"""
    cv = Canvas(SIZE, SIZE)
    white = rgb("#F4F1EA")
    white_dim = rgb("#D8D2C4")
    white_dark = rgb("#B8B0A0")
    for y in range(12, 29):
        t = (y - 12) / 16.0
        half = int(round(2 + 8 * (1 - t) ** 1.4))
        cv.hline(16 - half, 16 + half, y, white)
    cv.hline(9, 23, 14, white_dim)
    cv.hline(10, 22, 20, white_dim)
    # 尾须
    stick(cv, 15, 27, 14, 30, white_dark, 1)
    stick(cv, 17, 27, 18, 30, white_dark, 1)
    # 顶叶三片
    for side, y in ((-1, 10), (1, 12)):
        blob(cv, 16 + side * 5, y, 5, 3, BAMBOO)
        cv.hline(16 + side * 5 - 4, 16 + side * 5 + 2, y - 2, BAMBOO_LIGHT)
    blob(cv, 16, 8, 3, 3, BAMBOO_LIGHT)
    cv.outline(INK)
    return cv


def _seed_pack(spots, spot_color, note=""):
    """通用种子纸包：折口纸包 + 三粒溢出种子（颜色随种子）。"""
    cv = Canvas(SIZE, SIZE)
    cv.rect(8, 12, 23, 27, PAPER)
    cv.rect(8, 12, 23, 15, PAPER_DIM)
    cv.vline(8, 12, 27, PAPER_DIM)
    cv.vline(23, 12, 27, PAPER_DIM)
    cv.hline(8, 23, 27, PAPER_DIM)
    stick(cv, 8, 14, 23, 14, BAMBOO, 1)
    for x, y in spots:
        shade_blob(cv, x, y, 2, 2, spot_color)
    for x, y in ((24, 16), (26, 19)):
        cv.set(x, y, spot_color)
    cv.outline(INK)
    return cv


def icon_seed_radish():
    """萝卜籽：纸包 + 白粒（教学快熟）。"""
    return _seed_pack(((12, 20), (16, 24), (20, 21)), rgb("#F4F1EA"), "radish")


def icon_seed_herb():
    """草药籽：纸包 + 绿粒。"""
    return _seed_pack(((12, 20), (16, 24), (20, 21)), BAMBOO_LIGHT, "herb")


def icon_polygonum():
    """何首乌：暗紫块根一对 + 细须（8 天高价药材）。"""
    cv = Canvas(SIZE, SIZE)
    root = rgb("#6E4A63")
    root_light = rgb("#8A6078")
    root_dark = rgb("#4A2E42")
    for cx, cy, rx, ry in ((12, 21, 5, 7), (21, 23, 4, 6)):
        blob(cv, cx, cy, rx, ry, root)
        blob(cv, cx - 1, cy - 2, rx * 0.5, ry * 0.5, root_light)
    # 分叉须
    stick(cv, 12, 27, 10, 30, root_dark, 1)
    stick(cv, 13, 27, 15, 30, root_dark, 1)
    stick(cv, 21, 28, 19, 30, root_dark, 1)
    stick(cv, 22, 28, 24, 30, root_dark, 1)
    # 藤叶
    blob(cv, 16, 8, 6, 3, BAMBOO)
    cv.hline(10, 22, 6, BAMBOO_LIGHT)
    cv.outline(INK)
    return cv


def icon_seed_polygonum():
    """何首乌苗：纸包 + 紫粒。"""
    return _seed_pack(((12, 20), (16, 24), (20, 21)), rgb("#8A6078"), "polygonum")


def icon_ginseng():
    """人参：淡黄人形根 + 参须 + 顶叶（10 天最高价药材）。"""
    cv = Canvas(SIZE, SIZE)
    root = rgb("#E8D9A8")
    root_dim = rgb("#D4BF85")
    root_dark = rgb("#A8905C")
    # 主根（人形）
    blob(cv, 16, 19, 6, 8, root)
    blob(cv, 13, 13, 3, 4, root)
    blob(cv, 19, 13, 3, 4, root)
    cv.hline(13, 19, 15, root_dim)
    # 参须
    stick(cv, 11, 11, 7, 8, root_dark, 1)
    stick(cv, 21, 11, 25, 8, root_dark, 1)
    stick(cv, 14, 27, 11, 30, root_dark, 1)
    stick(cv, 18, 27, 21, 30, root_dark, 1)
    # 顶叶
    blob(cv, 16, 7, 5, 3, BAMBOO)
    cv.hline(11, 21, 5, BAMBOO_LIGHT)
    cv.outline(INK)
    return cv


def icon_seed_ginseng():
    """人参籽：纸包 + 淡黄粒。"""
    return _seed_pack(((12, 20), (16, 24), (20, 21)), rgb("#E8D9A8"), "ginseng")


# ---------------------------------------------------------------------------
# 清单
# ---------------------------------------------------------------------------

ICONS = [
    ("rice", icon_rice, "稻米 / CROP"),
    ("lingzhi", icon_lingzhi, "灵芝 / CROP"),
    ("tea_leaf", icon_tea_leaf, "春茶 / CROP"),
    ("seed_rice", icon_seed_rice, "稻种 / SEED"),
    ("seed_lingzhi", icon_seed_lingzhi, "菌种 / SEED"),
    ("heal_pill", icon_heal_pill, "回复药 / MEDICINE"),
    ("herb_tonic", icon_herb_tonic, "行气散 / MEDICINE"),
    ("herb", icon_herb, "药草 / GATHER"),
    ("wild_fruit", icon_wild_fruit, "野果 / GATHER"),
    ("lotus_seed", icon_lotus_seed, "莲子 / GATHER"),
    ("reed", icon_reed, "芦苇 / GATHER"),
    ("wood", icon_wood, "柴薪 / GATHER"),
    ("timber", icon_timber, "木料 / MATERIAL"),
    ("ore", icon_ore, "矿石 / GATHER"),
    ("wolf_pelt", icon_wolf_pelt, "狼皮 / DROP"),
    ("boar_meat", icon_boar_meat, "兽肉 / DROP"),
    ("ape_core", icon_ape_core, "山魈内丹 / DROP"),
    ("radish", icon_radish, "萝卜 / CROP"),
    ("seed_radish", icon_seed_radish, "萝卜籽 / SEED"),
    ("seed_herb", icon_seed_herb, "草药籽 / SEED"),
    ("polygonum", icon_polygonum, "何首乌 / CROP"),
    ("seed_polygonum", icon_seed_polygonum, "何首乌苗 / SEED"),
    ("ginseng", icon_ginseng, "人参 / CROP"),
    ("seed_ginseng", icon_seed_ginseng, "人参籽 / SEED"),
    ("hoe", icon_hoe, "锄头 / TOOL"),
    ("axe", icon_axe, "斧 / TOOL"),
    ("pick", icon_pick, "镐 / TOOL"),
    ("blade", icon_blade, "柴刀 / WEAPON"),
]


def build_preview(icons, scale=3):
    cols = 4
    pad = 10
    label_h = 20
    cell_w = SIZE * scale + pad * 2
    cell_h = SIZE * scale + pad * 2 + label_h
    rows = (len(icons) + cols - 1) // cols
    cv = Canvas(cell_w * cols + 8, 22 + cell_h * rows + 8)
    cv.rect(0, 0, cv.w - 1, cv.h - 1, rgb("#14171A"))
    draw_text(cv, 6, 6, "ITEM ICONS 32PX X3", GOLD_LIGHT, 1)
    for index, (name, canvas, note) in enumerate(icons):
        row, col = divmod(index, cols)
        x0 = 4 + col * cell_w
        y0 = 22 + row * cell_h
        draw_text(cv, x0 + pad, y0, name, BAMBOO_LIGHT, 1)
        draw_text(cv, x0 + pad, y0 + 9, note, PAPER_DIM, 1)
        top = y0 + label_h
        for by in range(0, SIZE * scale, 8):
            for bx in range(0, SIZE * scale, 8):
                tone = rgb("#262C33") if ((bx // 8 + by // 8) % 2 == 0) else rgb("#31383F")
                cv.rect(x0 + pad + bx, top + by, x0 + pad + bx + 7, top + by + 7, tone)
        cv.blit(canvas.scaled(scale), x0 + pad, top)
    return cv


def generate(quiet=False):
    os.makedirs(OUT_DIR, exist_ok=True)
    icons = []
    for name, factory, note in ICONS:
        canvas = factory()
        icons.append((name, canvas, note))
        with open(os.path.join(OUT_DIR, name + ".png"), "wb") as handle:
            handle.write(canvas.to_png())
    with open(os.path.join(OUT_DIR, "_preview.png"), "wb") as handle:
        handle.write(build_preview(icons).to_png())
    if not quiet:
        print(f"wrote {len(icons)} 个物品图标 → {OUT_DIR}")
        for name, canvas, note in icons:
            print(f"  {name}.png {canvas.w}x{canvas.h}  {note}")
        print("  _preview.png")


def check():
    from pixel_kit import decode_png

    issues = []
    for name, _, _ in ICONS:
        path = os.path.join(OUT_DIR, name + ".png")
        if not os.path.exists(path):
            issues.append("missing " + path)
            continue
        width, height, _, _ = decode_png(path)
        if (width, height) != (SIZE, SIZE):
            issues.append(f"{path}: {width}x{height} != {SIZE}x{SIZE}")
    if not os.path.exists(os.path.join(OUT_DIR, "_preview.png")):
        issues.append("missing " + os.path.join(OUT_DIR, "_preview.png"))
    return issues


def main():
    parser = argparse.ArgumentParser(description="《江湖有田》物品图标生成")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        problems = check()
        if problems:
            for problem in problems:
                print("[FAIL]", problem)
            raise SystemExit(1)
        print(f"[OK] 物品图标校验通过（{len(ICONS)} 个 32×32）")
        return
    generate()


if __name__ == "__main__":
    main()
