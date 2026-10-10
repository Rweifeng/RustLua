# -*- coding: utf-8 -*-
"""《江湖有田》地面碎屑与中景装饰 sprite 库（`assets/art/decor/`，M72-05）。

用法（工程根目录 `examples/game_jhyt` 下）：

    python tools/gen_decor_art.py            # 出图
    python tools/gen_decor_art.py --check    # 复核盘上 PNG（尺寸/色表/alpha 三条）

为什么装饰是**一件一张 PNG**而不是图集里的行：引擎的 Sprite 组件选不了 UV 区域
（`docs/37` 里那条约定，建筑与道具同理），所以"同一种东西的第 2 个样子"只能另存一张。
本工具因此给每个条目再产一张**水平镜像** `_r.png`——镜像由 `Canvas.mirrored()` 在数据侧
做，而不是场景串里写 `flip_x`：引擎规范序列化 `Sprite::to_component_str` 根本不输出 flip
字段，作者串带上它就过不了"生成器 == 引擎回写"的对拍。12 张本体 + 12 张镜像 = 24 个
形状，加上"同一张图配地表 tint 的两档明度"，实测等价外观 48 种。

三条硬契约（都有门）：

1. **条目表与散布同源**：形状清单/尺寸一律从 `decor_kit.CATALOG` 读，画完逐条比对
   ——散布指到一张不存在或尺寸不符的图，症状是"装饰没画出来"或"压到邻格"，两者都不报错。
2. **不许现写 hex**：颜色只从两处取——`terrain_ramp.json`（地表六族 + 墨/纸/金/木）和
   `gen_manor_art.py` 既有的道具色板（叶/竹/草垛/木）。后者是道具的**定义处**，这里只 import，
   所以灌木的绿和 `manor_tree.png` 的绿永远是同一个数。出图前过一次 `quantize_to_ramp`，
   越表像素数必须为 0。
3. **光源一致**：docs/53 §2 的"光自左上"在这里是**行号小的一侧亮**（PNG 第 0 行是贴图顶部），
   高亮一律画在靠上的几行、暗部压在最底一行；搞反的症状不是报错，是"影子在头顶"。

依赖：Python 3 标准库 + 同目录 `pixel_kit.py`、`terrain_kit.py`、`decor_kit.py`、`gen_manor_art.py`。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import decor_kit as dk  # noqa: E402
import gen_manor_art as ga  # noqa: E402
import terrain_kit as tk  # noqa: E402
from pixel_kit import Canvas, hash_stream, load_canvas, rgb  # noqa: E402

# 盘上的目录按**文件位置**定位，不跟着 cwd 走：`--check` 换个目录跑就会把"读不到"报成"缺图"
# （实测在 `tools/` 下跑时 24 张全报缺，图本身一张没少）。
# 写进 `.scene` 的引用路径是 `decor_kit.DECOR_DIR`（相对工程根，进文本数据），与这里无关。
OUT_DIR = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "assets", "art", "decor"
)
PREVIEW = os.path.join(OUT_DIR, "_preview.png")

RAMP = ga.RAMP
# 色阶表已经覆盖 grass/dirt/stone/water/roof/hedge 六族与 ink/paper/gold/wood 四色；
# 下面这批是 `gen_manor_art.py` 里道具自有的颜色，灌木/枯枝/落花要跟它们同色才像一个世界。
PROP_ONLY = ("LEAF_DARK", "BAMBOO", "BAMBOO_LIGHT", "BAMBOO_DARK",
             "STRAW", "STRAW_DARK", "STRAW_LIGHT",
             "WOOD", "WOOD_LIGHT", "WOOD_MID", "GOLD_LIGHT", "CINNABAR", "PAPER_DIM")
# 比对用的允许集是 **RGB 三元组**（`terrain_kit.allowed_colors` 的口径：PNG 读出来是 RGBA，
# 带 alpha 比就变成"每个不透明像素都越表"的假阳性）。
DECOR_ALLOWED = tk.allowed_colors(RAMP) | {getattr(ga, name)[:3] for name in PROP_ONLY}

# 下面这些保持 `gen_manor_art.py` 的 RGBA 四元组形态：`Canvas.set` 写的是 `bytes(color)`，
# 三元组画进去就是 alpha=0 的隐形像素。
INK = ga.INK
GRASS = ga.GRASS
GRASS_DARK = ga.GRASS_DARK
GRASS_LIGHT = ga.GRASS_LIGHT
GRASS_HI = ga.GRASS_HI
LEAF = ga.LEAF
LEAF_DARK = ga.LEAF_DARK
LEAF_LIGHT = ga.LEAF_LIGHT
DIRT_WET = ga.DIRT_WET
STONE = ga.STONE
STONE_LIGHT = ga.STONE_LIGHT
STONE_DARK = ga.STONE_DARK
STONE_MORTAR = ga.STONE_MORTAR
HEDGE = [rgb(hex_value) for hex_value in tk.family_hexes(RAMP, "hedge")]
PAPER = ga.PAPER
PAPER_DIM = ga.PAPER_DIM
GOLD = ga.GOLD
GOLD_LIGHT = ga.GOLD_LIGHT
CINNABAR = ga.CINNABAR
STRAW = ga.STRAW
STRAW_DARK = ga.STRAW_DARK
STRAW_LIGHT = ga.STRAW_LIGHT
WOOD = ga.WOOD
WOOD_MID = ga.WOOD_MID
WOOD_DARK = ga.WOOD_DARK
WOOD_LIGHT = ga.WOOD_LIGHT
BAMBOO_LIGHT = ga.BAMBOO_LIGHT


# ---------------------------------------------------------------------------
# 画法小工具（都按"PNG 第 0 行 = 贴图顶部"来写）
# ---------------------------------------------------------------------------


def blade(cv, x, y_base, rise, lean, body, tip=None, base=None):
    """一根草叶：底部 2px 宽、往上收到 1px，`lean` 决定梢部往哪边偏。

    二次曲线（`t*t`）而不是线性：直线画出来的草读作斜杠，只有下部直、上部弯才像被压过。
    `base` 只压**根节点那一个像素**：早先版本把整行压暗，结果五根草的根连成一条深色横带，
    16px 宽的小碎屑读作"矮栅栏"而不是"一撮草"。
    """
    for i in range(rise):
        t = i / max(rise - 1, 1)
        y = y_base - i
        if y < 0:
            break
        column = x + int(round(lean * t * t))
        color = base if (i == 0 and base) else (tip if (i >= rise - 2 and tip) else body)
        cv.set(column, y, color)
        if t < 0.55:
            cv.set(column + 1, y, color if i else body)


def stick(cv, x0, y0, x1, y1, body, shade_color):
    """一段枯枝：走直线，下半像素压暗一档，端点不闭环（闭合的枝子读作线段）。"""
    steps = max(abs(x1 - x0), abs(y1 - y0), 1)
    for i in range(steps + 1):
        x = x0 + round((x1 - x0) * i / steps)
        y = y0 + round((y1 - y0) * i / steps)
        cv.set(x, y, body)
        if i % 3 != 2:
            cv.set(x, y + 1, shade_color)


def blob(cv, cx, cy, rx, ry, body, light=None, dark=None):
    """一团叶子/石头：椭圆实心 + 上缘提亮 + 下缘压暗（左上光源，所以亮环取靠上的那半圈）。"""
    for dy in range(-ry, ry + 1):
        for dx in range(-rx, rx + 1):
            if (dx / max(rx, 1)) ** 2 + (dy / max(ry, 1)) ** 2 > 1.0:
                continue
            x, y = cx + dx, cy + dy
            if 0 <= x < cv.w and 0 <= y < cv.h:
                cv.set(x, y, body)
    for dy in range(-ry, ry + 1):
        for dx in range(-rx, rx + 1):
            if (dx / max(rx, 1)) ** 2 + (dy / max(ry, 1)) ** 2 > 1.0:
                continue
            edge = (abs(dx) >= rx - 1 and dy > 0) or dy >= ry - 1
            highlight = (dy <= -ry + 1 and abs(dx) <= rx - 1) or (dx <= -rx + 1 and dy <= 0)
            if edge and dark:
                cv.set(cx + dx, cy + dy, dark)
            elif highlight and light:
                cv.set(cx + dx, cy + dy, light)


def contact_shadow(cv, y, x0, x1, color):
    """贴地一行暗影：没有它，碎屑读作"飘在地面上方"。"""
    for x in range(x0, x1 + 1):
        if cv.get(x, y)[3] > 0:
            cv.set(x, y, color)


# ---------------------------------------------------------------------------
# 碎屑（散布在 debris 带：不描边、不遮挡任何东西）
#
# 为什么不描边：`Canvas.outline` 给每个不透明像素补一圈墨，1px 宽的草叶会胀成 3px 的墨块，
# 满屏碎屑就变成一片黑点。中景物件才描边（与 `gen_manor_art.py` 的道具同一套）。
# ---------------------------------------------------------------------------


def grass_short():
    cv = Canvas(16, 10)
    # 四根、间距 4、lean 有正有负：五根等距的草在 32px 网格里会排成栅栏
    for x, rise, lean, body in ((1, 6, -2, GRASS), (5, 9, 1, GRASS_LIGHT),
                                (9, 7, 3, GRASS), (13, 5, -1, GRASS_HI)):
        blade(cv, x, 9, rise, lean, body, base=GRASS_DARK)
    return cv


def grass_tuft():
    cv = Canvas(18, 14)
    for x, rise, lean, body in ((1, 9, -3, GRASS), (5, 12, 1, GRASS_LIGHT),
                                (9, 13, 0, GRASS_HI), (13, 10, 3, GRASS),
                                (16, 6, -1, GRASS_DARK)):
        blade(cv, x, 13, rise, lean, body, base=GRASS_DARK)
    # 两枝结籽的：穗用金色点在梢部，"草"和"杂草"的区别就在这两粒点上
    for x, y in ((5, 2), (9, 1)):
        cv.set(x, y, GOLD_LIGHT)
        cv.set(x, y + 1, GOLD)
    return cv


def twigs():
    cv = Canvas(20, 9)
    stick(cv, 1, 6, 12, 2, WOOD_MID, WOOD_DARK)
    stick(cv, 5, 8, 19, 4, WOOD, WOOD_DARK)
    cv.set(9, 3, WOOD_LIGHT)
    cv.set(15, 6, WOOD_LIGHT)
    return cv


def pebbles():
    cv = Canvas(16, 7)
    # 三粒之间**留缝**：上一版 (4,·)(9,·)(13,·) 的椭圆边缘相接，16px 上读作一坨灰泥而不是碎石
    for cx, cy, rx, ry in ((3, 4, 2, 2), (8, 3, 2, 2), (13, 5, 2, 1)):
        blob(cv, cx, cy, rx, ry, STONE, STONE_LIGHT, STONE_MORTAR)
    return cv


def petals():
    cv = Canvas(14, 6)
    rnd = hash_stream(0xDA12)
    for _ in range(9):
        x, y = rnd(13), rnd(5) + 1
        cv.set(x, y, PAPER if rnd(3) else PAPER_DIM)
    for x, y in ((3, 4), (10, 2)):
        cv.set(x, y, GOLD_LIGHT)
    return cv


def leaf_pile():
    cv = Canvas(18, 10)
    for cx, cy, color in ((4, 7, STRAW), (9, 6, LEAF_DARK), (13, 8, STRAW_DARK),
                          (6, 4, LEAF), (12, 4, GOLD)):
        for dx in range(-2, 3):
            y = cy + (1 if dx in (-2, 2) else 0)
            cv.set(cx + dx, y, color)
        cv.set(cx, y - 1, PAPER_DIM if color != PAPER_DIM else STRAW_LIGHT)
    return cv


def moss_patch():
    """贴石青苔：一坨一坨地长，不是横带。

    第一版按"每列一个高度"铺满 20px 宽，出来是等距竖纹的**梳子**（和草叶那个栅栏是同一
    类错误：32px 网格里凡是"每列都填到底"的形状都会读成栅栏）。这里改成 3px 宽的小团，
    团与团之间允许缺，边缘自然犬牙。
    """
    cv = Canvas(20, 8)
    rnd = hash_stream(0xA551)
    for cx in range(1, 18, 3):
        if rnd(4) == 0:
            continue
        top = 1 + rnd(3)
        for x in range(cx, min(cx + 3, 20)):
            for y in range(top + (x - cx) % 2, 8):
                depth = (y - top) / max(7 - top, 1)
                cv.set(x, y, HEDGE[1] if depth < 0.45 else HEDGE[2])
    for _ in range(14):
        x, y = 1 + rnd(18), rnd(8)
        if cv.get(x, y)[3] > 0:
            cv.set(x, y, HEDGE[3] if rnd(2) else HEDGE[0])
    return cv


# ---------------------------------------------------------------------------
# 中景（mid 带：描边，与小道具同权重）
# ---------------------------------------------------------------------------


def bush():
    cv = Canvas(28, 22)
    blob(cv, 14, 12, 12, 8, LEAF, LEAF_LIGHT, LEAF_DARK)
    blob(cv, 8, 9, 7, 6, LEAF, LEAF_LIGHT, LEAF_DARK)
    blob(cv, 20, 10, 6, 5, LEAF_DARK, LEAF, LEAF_DARK)
    for x, y in ((6, 4), (10, 3), (16, 5), (21, 6)):
        cv.set(x, y, GRASS_HI)
        cv.set(x, y + 1, GRASS_LIGHT)
    contact_shadow(cv, 21, 4, 24, DIRT_WET)
    cv.outline(INK)
    return cv


def rock():
    """孤石：下宽上窄的 mound，不是六边形。

    第一版画成"最宽在中间"的对称菱形，读作一颗灰色宝石；石头要有重量，所以宽度单调
    递增到底，顶部只留一条平棱当受光面。
    """
    cv = Canvas(24, 18)
    for y in range(3, 17):
        t = (y - 3) / 13.0
        half = 4 + int(round(7 * t ** 0.72))
        cv.hline(12 - half, 12 + half, y, STONE)
    cv.hline(9, 15, 3, STONE_LIGHT)
    cv.hline(7, 17, 4, STONE_LIGHT)
    for y in range(5, 16, 2):
        offset = (y - 5) // 2
        cv.set(10 - offset, y, STONE_DARK)
        cv.set(11 - offset, y + 1, STONE_DARK)
    for x in range(5, 20):
        cv.set(x, 16, STONE_MORTAR)
    cv.outline(INK)
    return cv


def flowers():
    cv = Canvas(22, 18)
    for x, rise, lean in ((4, 11, -1), (11, 13, 0), (17, 10, 1)):
        blade(cv, x, 17, rise, lean, GRASS, GRASS_LIGHT)
    for cx, cy, petal in ((4, 5, GOLD), (11, 3, CINNABAR), (17, 6, PAPER)):
        for dx, dy in ((-1, 0), (1, 0), (0, -1), (0, 1), (0, 0)):
            cv.set(cx + dx, cy + dy, petal)
        cv.set(cx, cy, PAPER_DIM if petal != PAPER else CINNABAR)
    contact_shadow(cv, 17, 3, 18, GRASS_DARK)
    cv.outline(INK)
    return cv


def mushroom():
    cv = Canvas(16, 14)
    for cx, cap_y, height in ((5, 5, 6), (11, 8, 4)):
        cv.rect(cx - 1, cap_y, cx + 1, cap_y + height, PAPER)
        cv.vline(cx + 1, cap_y, cap_y + height, PAPER_DIM)
        for offset in range(-3, 4):
            row_y = cap_y - 3 + abs(offset) // 2
            cv.vline(cx + offset, cap_y - 3, row_y + 2, CINNABAR)
        cv.set(cx - 1, cap_y - 2, PAPER)
        cv.set(cx + 2, cap_y - 1, PAPER)
    contact_shadow(cv, 13, 3, 13, DIRT_WET)
    cv.outline(INK)
    return cv


def reed():
    cv = Canvas(20, 28)
    for x, rise, lean in ((2, 19, -2), (7, 24, 0), (12, 22, 1), (17, 16, 3)):
        blade(cv, x, 27, rise, lean, BAMBOO_LIGHT, LEAF_LIGHT, base=LEAF_DARK)
    # 香蒲穗：深木色的胶囊，两格宽、五格高——没有它这就是一丛高草
    for cx, top in ((7, 3), (12, 6)):
        cv.rect(cx - 1, top, cx + 1, top + 4, WOOD_MID)
        cv.vline(cx - 1, top, top + 4, WOOD_DARK)
        cv.set(cx, top, WOOD)
    cv.outline(INK)
    return cv


def lily_pad():
    """睡莲：浮在水面的圆叶 + 一道从叶心切到边缘的 V 形缺口，左上开一朵白花。

    缺口不是装饰，是**认得出来**的那一条：没有它这就是一团绿椭圆，跟草地上的叶堆
    （`leaf_pile`）在 22px 上无从分辨。缺口切在右侧、花落在左侧，`_r.png` 镜像后
    两者一起翻，所以两个朝向都读得出"这是睡莲"。
    """
    cv = Canvas(22, 14)
    blob(cv, 10, 7, 10, 6, LEAF, light=LEAF_LIGHT, dark=LEAF_DARK)
    # V 形缺口：从叶心 (10,7) 向右张开 2px，切到边缘
    for step in range(11):
        x = 10 + step
        y = 7 - step // 6
        cv.set(x, y, (0, 0, 0, 0))
        cv.set(x, y + 1, (0, 0, 0, 0))
    # 叶脉：从叶心放射的三条暗线，圆叶没有脉就读成一块绿饼
    for dx, dy in ((-6, -3), (-7, 2), (-3, 5), (-8, -1)):
        for t in range(3):
            x, y = 10 + dx * t // 3, 7 + dy * t // 3
            if cv.get(x, y)[3] > 0:
                cv.set(x, y, LEAF_DARK)
    for x, y in ((4, 4), (5, 4), (4, 5), (5, 5), (3, 5), (6, 5), (4, 3), (5, 3)):
        cv.set(x, y, PAPER)
    cv.set(4, 4, GOLD_LIGHT)
    return cv


SHAPES = {
    "decor_grass_short": grass_short,
    "decor_grass_tuft": grass_tuft,
    "decor_twigs": twigs,
    "decor_pebbles": pebbles,
    "decor_petals": petals,
    "decor_leaf_pile": leaf_pile,
    "decor_moss_patch": moss_patch,
    "decor_bush": bush,
    "decor_rock": rock,
    "decor_flowers": flowers,
    "decor_mushroom": mushroom,
    "decor_reed": reed,
    "decor_lily": lily_pad,
}


def render(name):
    """画一个条目并压回色表：返回 (canvas, 越表像素数)。"""
    canvas = SHAPES[name]()
    return tk.quantize_to_ramp(canvas, DECOR_ALLOWED)


def write_png(path, canvas):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(canvas.to_png())


def generate():
    total = 0
    offenders = 0
    for row in dk.CATALOG:
        stem, size, band = row[1], row[2], row[3]
        canvas, bad = render(stem)
        offenders += bad
        if (canvas.w, canvas.h) != size:
            raise SystemExit(f"[FAIL] {stem}：画出来 {canvas.w}x{canvas.h}，目录里写的是 {size}")
        write_png(os.path.join(OUT_DIR, f"{stem}.png"), canvas)
        write_png(os.path.join(OUT_DIR, f"{stem}_r.png"), canvas.mirrored())
        total += 2
        print(f"  {stem}: {canvas.w}x{canvas.h} {band} 本体+镜像")
    if offenders:
        raise SystemExit(f"[FAIL] {offenders} 个像素不在色阶表/道具色板里——生成器里不许现写 hex")
    preview()
    print(f"[OK] 装饰库 {total} 张 → {OUT_DIR}/（12 形状 × 本体+镜像）")


def preview(scale=3, gap=4):
    """接触表：每个条目占"本体 + 镜像"两行，六个条目一行。目验用（docs/53 的并排口径）。"""
    columns = 6
    cell_w = max(row[2][0] for row in dk.CATALOG)
    cell_h = max(row[2][1] for row in dk.CATALOG)
    sheet_w = columns * (cell_w * scale + gap) + gap
    rows = (len(dk.CATALOG) + columns - 1) // columns * 2
    sheet_h = rows * (cell_h * scale + gap) + gap
    sheet = Canvas(sheet_w, sheet_h, INK)
    for index, row in enumerate(dk.CATALOG):
        canvas, _ = render(row[1])
        for column, source in enumerate((canvas, canvas.mirrored())):
            left = (index % columns) * (cell_w * scale + gap) + gap
            top = (index // columns * 2 + column) * (cell_h * scale + gap) + gap
            for y in range(source.h):
                for x in range(source.w):
                    color = source.get(x, y)
                    if color[3] == 0:
                        continue
                    sheet.rect(left + x * scale, top + y * scale,
                               left + (x + 1) * scale - 1, top + (y + 1) * scale - 1, color)
    write_png(PREVIEW, sheet)


def check():
    """复核盘上产物：条目齐全、尺寸对得上、像素全在色表内、alpha 只有 0/255。"""
    problems = []
    for row in dk.CATALOG:
        for suffix in ("", "_r"):
            path = os.path.join(OUT_DIR, f"{row[1]}{suffix}.png")
            if not os.path.exists(path):
                problems.append(f"缺图 {path}")
                continue
            canvas = load_canvas(path)
            if (canvas.w, canvas.h) != row[2]:
                problems.append(f"{path} 尺寸 {canvas.w}x{canvas.h} != 目录 {row[2]}")
                continue
            for y in range(canvas.h):
                for x in range(canvas.w):
                    color = canvas.get(x, y)
                    if color[3] not in (0, 255):
                        problems.append(f"{path} ({x},{y}) alpha={color[3]} 未二值化")
                        break
                    if color[3] and color[:3] not in DECOR_ALLOWED:
                        problems.append(f"{path} ({x},{y}) 色 {color[:3]} 不在色阶表/道具色板里")
                        break
                else:
                    continue
                break
    if problems:
        return problems
    return []


def main():
    parser = argparse.ArgumentParser(description="地面碎屑与中景装饰 sprite 库（M72-05）")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        problems = check()
        if problems:
            for problem in problems:
                print("[FAIL]", problem)
            raise SystemExit(1)
        print(f"[OK] 装饰库校验通过（{len(dk.CATALOG) * 2} 张）")
        return
    generate()


if __name__ == "__main__":
    main()
