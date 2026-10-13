# -*- coding: utf-8 -*-
"""M72-04 建、M72-11 铺量：**地表图集生成器**（四族色阶 + 4-bit autotile 过渡块）。

用法（工程根目录 `examples/game_jhyt` 下）：

    python tools/gen_terrain_tiles.py --scene village   # 出图 village_ground.png + 预览 + 自检
    python tools/gen_terrain_tiles.py                  # 不带 --scene 就是 manor（M72-04 的默认）
    python tools/gen_terrain_tiles.py --scene village --check    # 只校盘上那张 PNG，不重画
    python tools/gen_terrain_tiles.py --layout         # 打印条目表（人读，验收对表用）

产出 `assets/art/tilesets/<scene>/<scene>_ground.png`（384×288 = 12 列 × 9 行 × 32px）。

**机制一份、场景一档**：图集排布、条目算术、掩码位序、四边收口的深度带、出图后处理
全场景共享；每个场景带来的差别只有"取哪几族色"（`terrain_ramp.json` 的 `scenes` 段）
与"六种材质各怎么画"（`SCENE_RECIPES`）。这正是 M72-11 要证的事：金标规范是**可复制的
机制**，不是山庄的一张图。

**为什么颜色只从 `terrain_ramp.json` 取**：docs/57 §1.2 把草/土/石/水四档写成了规范，
但出图侧各自写 hex（`gen_manor_art.py` 的 `GRASS_LIGHT`、四季变体 `MAPS` 又是第三份），
于是"色阶表"与"实际用色"从来没有共同来源。本文件与 `--check`、`gen_season_variants.py`、
Rust 门都读同一张表；表外的颜色算越界，出图后处理报数、生成器直接拒绝落盘。

**图集排布与引擎同构**（运行时解算在 M72-02 定死，条目算术在 `terrain_kit.py`）：

    entry = 1 + (kind-1)                        # 满块档：关掉 autotile 时的恒等寻址
    entry = 1 + kinds + (kind-1)*16 + mask      # 过渡块：bit0=y-1(下) bit1=x+1(右) bit2=y+1(上) bit3=x-1(左)
    6 kinds × (1 + 16) = 102 条 ≤ 12×9 = 108，尾部 103..108 留全透明。

美术口径来自 docs/53 §3（看图所得，不是自创）：
  * §3.1 交界**碎点压边**：不画 1px 分界线，而是犬牙深度带 + 断口 + 碎点，两族各自的边相接成缝；
  * §3.3 墙/崖**三段式**：圆石砌体 + 顶部草帽沿 + 底部暗基线（村庄的同一段是白灰墙 +
    黛瓦压顶 + 青石基——三段式是结构，材料跟场景走）；
  * §3.7 水陆**先画 rim 再画水**：岸带压在瓦内侧，水靠同族色阶分档而非半透明；
  * docs/57 §1 **统一左上光源**：石块/碎屑左上亮、右下暗；描边只用墨色断点，不画灰边。

依赖：Python 3 标准库 + 同目录 `pixel_kit.py`、`terrain_kit.py`。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import terrain_kit as tk  # noqa: E402
from pixel_kit import Canvas, draw_text, hash_stream, load_canvas, text_width  # noqa: E402

TILE = 32


def ground_path(scene):
    """地表图集的路径口径：`assets/art/tilesets/<scene>/<scene>_ground.png`。

    M72-04 时这里写死 manor 一个名；M72-11 铺量后每个场景都走这一条，因为场景生成器
    （`gen_village_scene.py` 等）与四季变体、门都要按同一个式子找到那张图，
    写死两次就会出现"改名只改了一处"的断链。**命名式子本身在 `tk.ground_texture`**
    （`.scene` 里要的是正斜杠），这里只是它的 OS 分隔符版本，用于读写文件。
    """
    return os.path.join(*tk.ground_texture(scene).split("/"))


def ground_preview_path(scene):
    """预览走 `_ground_preview.png`（山庄那张就是这个名字）：目录已经点名场景，
    文件名再带一遍场景名就是 `village/village_ground_village_preview` 这种绕口的东西。"""
    return os.path.join("assets", "art", "tilesets", scene, "_ground_preview.png")

# 碎点压边的深度带：3-6px 随机犬牙，约 1/9 概率断口。起值取 3 而不是星露谷的 1-2px，
# 因为这里 1 瓦 = 32px（星露谷 16px），同一视觉重量要乘 2；断口是硬要求——一条连续等深
# 的带本身就是直线，只是换了个方向。
EDGE_DEPTH_BASE = 3
EDGE_DEPTH_JITTER = 3
EDGE_GAP_ODDS = 9

# 掩码位 → 该位在**瓦片图像**上的哪条边，以及沿该边的取样坐标（t = 沿边像素，d = 向内深度）。
# bit0=y-1 是下沿、bit2=y+1 是上沿：地图 row 0 在最底而图集 row 0 在最顶，这两件事别混。
EDGE_COORD = {
    "bottom": lambda t, d: (t, TILE - 1 - d),
    "right": lambda t, d: (TILE - 1 - d, t),
    "top": lambda t, d: (t, d),
    "left": lambda t, d: (d, t),
}


# ---------------------------------------------------------------------------
# 取色
# ---------------------------------------------------------------------------


def family_palette(table, family):
    """一族色阶按"深→暗→中→亮"命名成绘制用的调色字典（顺序即 docs/57 §1.2 的档位语义）。"""
    levels = tk.family_levels(table, family)
    return {"deep": levels[0], "dark": levels[1], "mid": levels[2], "light": levels[3]}


def palettes(table):
    """一次备好所有取色入口：每族四档 + 通用墨/纸/金/木。"""
    families = {family: family_palette(table, family) for family in table["families"]}
    utility = {key: tk.rgb(value) for key, value in table["utility"].items()}
    return families, utility


# ---------------------------------------------------------------------------
# 基础材质（满块 = mask 15）
# ---------------------------------------------------------------------------


def _speckles(canvas, rand, colors, count):
    for _ in range(count):
        canvas.set(rand(TILE), rand(TILE), colors[rand(len(colors))])


def _pebble(canvas, rand, pal, x=None, y=None):
    """散落碎石：2-3×2 暗块，顶行最亮、底行最深（左上光源）。"""
    width = 2 + rand(2)
    x = rand(TILE - width) if x is None else x
    y = rand(TILE - 2) if y is None else y
    canvas.rect(x, y, x + width - 1, y + 1, pal["dark"])
    canvas.hline(x, x + width - 1, y, pal["light"])
    canvas.hline(x, x + width - 1, y + 1, pal["deep"])


def _twig(canvas, rand, wood):
    x, y = rand(TILE - 6), 1 + rand(TILE - 2)
    canvas.hline(x, x + 2 + rand(3), y, wood)
    canvas.set(x + 1, y - 1, wood)


def paint_grass(canvas, rand, pal):
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["mid"])
    _speckles(canvas, rand, [pal["deep"], pal["dark"], pal["light"]], 150)
    for _ in range(4):  # 草簇：三叶一组，中间齐、两侧矮
        x, y = 1 + rand(TILE - 3), 2 + rand(TILE - 5)
        canvas.vline(x, y - 2, y, pal["dark"])
        canvas.vline(x - 1, y - 1, y, pal["deep"])
        canvas.vline(x + 1, y - 1, y, pal["light"])


def paint_dirt(canvas, rand, pal, stone, wood):
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["mid"])
    _speckles(canvas, rand, [pal["deep"], pal["dark"], pal["light"]], 180)
    for _ in range(3):
        _pebble(canvas, rand, stone)
    for _ in range(2):
        _twig(canvas, rand, wood)


def paint_masonry(canvas, rand, pal, row_height):
    """圆石砌体：垫层是深档，石头去角（不留 1px 直缝），左上亮、右下暗。

    石板路用 `row_height=6`（矮而宽，读作铺地），墙体用 4（高而密，读作砌体）：同一族
    色阶两种砌法，比给墙另配色相更省，也不会让 wall 与 dirt 相邻时糊成一片。
    """
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["deep"])
    y = 0
    while y < TILE:
        height = row_height + rand(3)
        bottom = min(TILE - 1, y + height - 2)
        x = 0
        while x < TILE:
            width = 6 + rand(7)
            right = min(TILE - 1, x + width - 1)
            canvas.rect(x, y, right, bottom, pal["mid"])
            canvas.hline(x, right, y, pal["light"])
            canvas.hline(x, right, bottom, pal["dark"])
            canvas.vline(right, y, bottom, pal["deep"])
            if rand(3) == 0:
                canvas.set(right, y, pal["deep"])
            if rand(3) == 0:
                canvas.set(x, bottom, pal["deep"])
            x += width
        y += height - 1


def paint_water(canvas, rand, pal):
    """水：主体暗档 + 深部斑块 + 亮档短波纹。分档靠色阶，不靠透明度。"""
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["dark"])
    _speckles(canvas, rand, [pal["deep"], pal["mid"]], 80)
    for _ in range(2):
        x, y = rand(TILE - 9), rand(TILE - 7)
        canvas.rect(x, y, x + 6 + rand(3), y + 3 + rand(3), pal["deep"])
    for _ in range(5):
        x, y = rand(TILE - 7), rand(TILE - 2)
        width = 2 + rand(4)
        canvas.hline(x, x + width, y, pal["light"])
        canvas.hline(x, x + width - 1, y + 1, pal["mid"])


def paint_roof(canvas, rand, pal):
    """青瓦：竖向瓦垄，每条垄左亮右暗；垄宽随机，避免读成壁纸重复。"""
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["dark"])
    x = 0
    while x < TILE:
        width = 5 + rand(3)
        right = min(TILE - 1, x + width - 2)
        canvas.rect(x, 0, right, TILE - 1, pal["mid"])
        canvas.vline(x, 0, TILE - 1, pal["light"])
        canvas.vline(right, 0, TILE - 1, pal["deep"])
        x += width
    _speckles(canvas, rand, [pal["dark"], pal["deep"]], 24)


# ---------------------------------------------------------------------------
# 碎点压边
# ---------------------------------------------------------------------------


def edge_profile(rand):
    """沿一条边的犬牙深度：0 = 断口（该列不画带，只留零星碎点）。"""
    return [0 if rand(EDGE_GAP_ODDS) == 0 else EDGE_DEPTH_BASE + rand(EDGE_DEPTH_JITTER)
            for _ in range(TILE)]


def paint_soil_edge(canvas, rand, edge, pal):
    """草/土的暴露边：内侧深档犬牙带，外沿留 1 列碎点（草叶/土块往邻家溢出 1-2px）。"""
    coord = EDGE_COORD[edge]
    fringe = []
    for t, depth in enumerate(edge_profile(rand)):
        if depth == 0:
            for d in range(1 + rand(2)):
                canvas.set(*coord(t, d), pal["deep"])
            continue
        for d in range(depth):
            canvas.set(*coord(t, d), pal["deep"] if d < depth - 1 else pal["dark"])
        fringe.append((t, depth + 1 + rand(2)))
    for t, d in fringe:  # 碎点单独一遍：免得后一列的带把前一列的碎点盖掉
        canvas.set(*coord(t, d), pal["light"] if (t + d) % 2 else pal["mid"])


def paint_paving_edge(canvas, rand, edge, pal, bedding):
    """石板路的暴露边：铺地收边成垫层碎砾，再压几块半嵌的石，读作"路到这儿散了"。"""
    coord = EDGE_COORD[edge]
    spots = []
    for t, depth in enumerate(edge_profile(rand)):
        span = max(depth, 2)
        for d in range(span):
            canvas.set(*coord(t, d), bedding if d < span - 1 else pal["deep"])
        if rand(3) == 0:
            spots.append((t, span))
    for t, d in spots:
        canvas.set(*coord(t, d), pal["mid"])
        canvas.set(*coord(t, d - 1), pal["light"])


def paint_shore_edge(canvas, rand, edge, water, dirt, stone):
    """水陆交界（docs/53 §3.7）：由外向内是 土岸 rim → 浅色水花 → 深水带 → 水体。"""
    coord = EDGE_COORD[edge]
    for t in range(TILE):
        rim = 2 + rand(3)
        foam = rim + 1 + rand(2)
        band = foam + 3 + rand(3)
        for d in range(band):
            if d < rim:
                canvas.set(*coord(t, d), dirt["deep"] if d == 0 else dirt["mid"])
            elif d <= foam:
                if rand(3):
                    canvas.set(*coord(t, d), water["light"])
            else:
                canvas.set(*coord(t, d), water["deep"])
        if rand(6) == 0:
            canvas.set(*coord(t, rim), stone["light"])


# ---------------------------------------------------------------------------
# 墙与屋顶：按边的结构收口（不是碎点，语义不同）
# ---------------------------------------------------------------------------


def paint_wall_cap(canvas, rand, grass, hedge):
    """三段式第一段：草帽沿。顶 4-6px 草（最上一行最亮），草与石之间一条 hedge 深档暗沿。"""
    for x in range(TILE):
        depth = 4 + rand(3)
        canvas.rect(x, 0, x, depth - 2, grass["mid"])
        canvas.set(x, 0, grass["light"])
        canvas.set(x, depth - 1, grass["dark"])
        canvas.set(x, depth, hedge["deep"])
        if rand(4) == 0:
            canvas.set(x, depth + 1, hedge["dark"])


def paint_wall_base(canvas, rand, stone, ink):
    """三段式第三段：底部暗基线（2-4px 犬牙，最外一行在墨色与深档间跳，不是一条直线）。"""
    for x in range(TILE):
        depth = 2 + rand(3)
        for d in range(depth):
            canvas.set(x, TILE - 1 - d, ink if d == 0 else stone["deep"])
        if rand(5) == 0:
            canvas.set(x, TILE - 1 - depth, stone["dark"])


def paint_wall_end(canvas, rand, edge, stone, ink):
    """墙的端面（左/右暴露）：端面两列更深，墨色轮廓按 6/7 概率断点出现（硬轮廓但不封死）。"""
    coord = EDGE_COORD[edge]
    for t in range(TILE):
        canvas.set(*coord(t, 0), stone["deep"])
        canvas.set(*coord(t, 1), stone["dark"])
        if rand(7):
            canvas.set(*coord(t, 2), ink)


def paint_roof_eave(canvas, rand, pal, ink):
    """出檐（下沿暴露）：瓦端一排扇贝 + 檐下断点墨影（docs/53 §3.4 的"屋檐压出暗带"）。"""
    for t in range(TILE):
        scallop = 3 if (t % 5) in (1, 2, 3) else 1
        canvas.rect(t, TILE - 1 - scallop, t, TILE - 2, pal["deep"])
        canvas.set(t, TILE - 1, ink if scallop == 3 else pal["dark"])


def paint_roof_ridge(canvas, rand, pal):
    """屋脊（上沿暴露）：最亮一档两行收头，其下一条暗缝。"""
    canvas.hline(0, TILE - 1, 0, pal["light"])
    canvas.hline(0, TILE - 1, 1, pal["mid"])
    for x in range(0, TILE, 3):
        canvas.set(x, 2, pal["deep"])


def paint_roof_gable(canvas, rand, edge, pal, ink):
    """山墙端（左/右暴露）：瓦垄在此收头，一条竖界由深档 + 断点墨色组成。"""
    coord = EDGE_COORD[edge]
    for t in range(TILE):
        canvas.set(*coord(t, 0), pal["deep"])
        canvas.set(*coord(t, 1), pal["mid"])
        if rand(8) == 0:
            canvas.set(*coord(t, 0), ink)


# ---------------------------------------------------------------------------
# 村庄（云泽村）的六种材质画法：机制与山庄完全同一套，只有身份不同（docs/39 §2）
# ---------------------------------------------------------------------------


def paint_village_grass(canvas, rand, pal, ctx):
    """山谷野草：比山庄的草更深更杂，带三叶草与落叶。"""
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["mid"])
    _speckles(canvas, rand, [pal["deep"], pal["dark"], pal["light"]], 190)
    for _ in range(5):  # 三叶草：中间一颗最亮，两侧压暗
        x, y = 2 + rand(TILE - 5), 2 + rand(TILE - 5)
        canvas.set(x, y, pal["light"])
        canvas.set(x - 1, y + 1, pal["dark"])
        canvas.set(x + 1, y + 1, pal["dark"])
    for _ in range(3):  # 落叶：金档两片，叶柄落回木色
        x, y = 3 + rand(TILE - 8), 3 + rand(TILE - 8)
        canvas.hline(x, x + 1, y, ctx["utility"]["gold"])
        canvas.set(x + 1, y + 1, ctx["utility"]["wood"])
    for _ in range(3):  # 一撮 tall grass：山谷草比山庄高半格
        x, y = 2 + rand(TILE - 4), 4 + rand(TILE - 7)
        canvas.vline(x, y - 3, y, pal["deep"])
        canvas.set(x - 1, y - 1, pal["dark"])
        canvas.set(x + 1, y - 2, pal["light"])


def paint_village_soil(canvas, rand, pal, ctx):
    """茶园土：偏沙的踩实土 + 耙痕（茶垄）+ 碎石。耙痕逐段抖一列，不是一条 32px 直线。"""
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["mid"])
    _speckles(canvas, rand, [pal["deep"], pal["dark"], pal["light"]], 150)
    for row in (6, 16, 26):
        x = 0
        while x < TILE:
            width = 5 + rand(6)
            y = row + (rand(3) - 1)
            right = min(TILE - 1, x + width)
            canvas.hline(x, right, y, pal["deep"])
            canvas.hline(x, right, y + 1, pal["light"])
            x = right + 1
    for _ in range(4):
        _pebble(canvas, rand, ctx["paving"])


def paint_village_flagstone(canvas, rand, pal, ctx):
    """青石板街：四块大石板共用一条深缝，板面各占一档，两道车辙走浅点。

    山庄的石是"圆石砌体"（小块、去角、密缝），村庄是"铺地大板"（宽缝、磨损面）：
    同属石族两种砌法，读起来一个是院墙根、一个是街。
    """
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["deep"])
    xs = (0, 14 + rand(4), TILE - 1)
    ys = (0, 12 + rand(6), TILE - 1)
    tones = (pal["mid"], pal["light"], pal["dark"], pal["mid"])
    index = 0
    for row in range(2):
        for col in range(2):
            x0, y0 = xs[col] + 1, ys[row] + 1
            x1, y1 = xs[col + 1] - 1, ys[row + 1] - 1
            canvas.rect(x0, y0, x1, y1, tones[index])
            canvas.hline(x0, x1, y0, pal["light"])       # 左上受光：板面顶行最亮
            canvas.hline(x0, x1, y1, pal["dark"])        # 右下收暗
            canvas.vline(x1, y0, y1, pal["deep"])
            index += 1
    for _ in range(20):  # 板面磨损：碎点只打在暗档与中档上
        x, y = rand(TILE), rand(TILE)
        if canvas.get(x, y)[:3] != pal["deep"][:3]:
            canvas.set(x, y, pal["dark"])
    for rut in (9, 24):  # 车辙：断续的浅点，推成"走久了磨出来的一条"
        y = rand(3)
        while y < TILE - 1:
            canvas.set(rut, y, pal["light"])
            if rand(2):
                canvas.set(rut, y + 1, pal["light"])
            y += 3 + rand(4)


def paint_village_wall(canvas, rand, pal, ctx):
    """徽派白墙的本体：白灰面 + 雨水渍 + 墙根青石基一线。"""
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["mid"])
    for _ in range(24):  # 灰面斑驳：暗档与深档交替，不画成一张白纸
        canvas.set(rand(TILE), rand(TILE), pal["dark"] if rand(3) else pal["deep"])
    for _ in range(3):  # 雨水渍：自墙面向下拖的两三条竖痕
        x = 2 + rand(TILE - 5)
        height = 6 + rand(10)
        y = 8 + rand(TILE - 14 - height)
        canvas.vline(x, y, y + height, pal["dark"])
        canvas.set(x + 1, y + height, pal["deep"])
    canvas.hline(0, TILE - 1, TILE - 5, ctx["paving"]["dark"])
    for x in range(2, TILE, 7):
        canvas.vline(x, TILE - 4, TILE - 1, ctx["paving"]["deep"])


def paint_village_stream(canvas, rand, pal, ctx):
    """溪水：横着走的浅滩波纹（山庄池塘是斑块水，这里是流得动的山溪）。"""
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["dark"])
    _speckles(canvas, rand, [pal["deep"]], 70)
    for row in (5, 13, 21, 29):
        x = rand(5)
        while x < TILE - 4:
            width = 3 + rand(4)
            y = row + (rand(3) - 1)
            canvas.hline(x, min(TILE - 1, x + width), y, pal["mid"])
            canvas.set(min(TILE - 1, x + width), y - 1, pal["light"])
            x += width + 3 + rand(5)
    for _ in range(2):  # 浅滩下露出的卵石
        x, y = 2 + rand(TILE - 6), 2 + rand(TILE - 6)
        canvas.rect(x, y, x + 2, y + 1, ctx["paving"]["dark"])
        canvas.hline(x, x + 2, y, ctx["paving"]["light"])


def paint_village_wall_edges(canvas, seed, mask, pal, ctx):
    """白墙的三段式收口：黛瓦压顶（上沿）/ 青石基（下沿）/ 马头墙端（左右沿）。

    与山庄院墙同一套结构（顶、身、基），换的是材料：墙顶从"草帽沿"换成黛瓦，
    墙基从暗基线换成青石，端面从圆石换成白灰收边 + 墨线断点。
    """
    exposed = tk.mask_edges(mask)
    slate = ctx["families"]["slate"]
    if "top" in exposed:
        for x in range(TILE):
            depth = 5 + (seed + x) % 3
            canvas.rect(x, 0, x, depth - 2, slate["dark"])
            canvas.set(x, 0, slate["light"])
            canvas.set(x, depth - 1, slate["deep"])
            if (seed + x * 7) % 11 == 0:
                canvas.set(x, depth, slate["mid"])
    if "bottom" in exposed:
        paving = ctx["paving"]
        for x in range(TILE):
            depth = 3 + (seed + x * 3) % 3
            for d in range(depth):
                canvas.set(x, TILE - 1 - d, paving["deep"] if d else paving["dark"])
            if (seed + x * 5) % 9 == 0:
                canvas.set(x, TILE - 1 - depth, paving["light"])
    for bit, edge in ((3, "left"), (1, "right")):
        if edge in exposed:
            coord = EDGE_COORD[edge]
            for t in range(TILE):
                canvas.set(*coord(t, 0), pal["deep"])
                canvas.set(*coord(t, 1), pal["dark"])
                if (seed + t * 13) % 7:
                    canvas.set(*coord(t, 2), ctx["utility"]["ink"])


# ---------------------------------------------------------------------------
# 青云山道（M72-11 第二张铺量的图）
#
# 身份差别写在 `terrain_ramp.json:scenes.mountain_path` 的 meaning 里，画法上的差别是：
# 山庄的绿是"人造园艺 + 晒场"，村是"阔叶山谷"，山道是**针叶林下**——所以地面母题是松针、
# 片岩碎块、凿痕石阶、层理岩壁与竹叶层，一条直线铺装都不许出现。
# ---------------------------------------------------------------------------


def paint_mountain_grass(canvas, rand, pal, ctx):
    """针叶林下林地：冷绿草皮上落松针，苔斑只给最亮一档（读起来是"潮"，不是"花"）。"""
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["mid"])
    _speckles(canvas, rand, [pal["deep"], pal["dark"], pal["light"]], 170)
    for _ in range(6):  # 松针：一根 3~4px 的斜针，下面压一根同族暗档
        x, y = 2 + rand(TILE - 5), 2 + rand(TILE - 5)
        length = 3 + rand(2)
        canvas.hline(x, x + length, y, ctx["utility"]["wood"])
        canvas.hline(x, x + length - 1, y + 1, pal["deep"])
    for _ in range(2):
        x, y = 3 + rand(TILE - 8), 3 + rand(TILE - 8)
        canvas.rect(x, y, x + 3, y + 2, pal["light"])
        canvas.set(x + 1, y, pal["mid"])
    for _ in range(2):  # 一撮硬草
        x, y = 2 + rand(TILE - 4), 5 + rand(TILE - 8)
        canvas.vline(x, y - 3, y, pal["dark"])
        canvas.set(x - 1, y - 2, pal["light"])


def paint_mountain_scree(canvas, rand, pal, ctx):
    """碎石土路：踩实土里嵌**扁的**片岩碎块（村溪卵是圆的，这里的读感是"硌脚"）。"""
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["mid"])
    _speckles(canvas, rand, [pal["deep"], pal["dark"], pal["light"]], 140)
    for _ in range(7):
        x, y = 1 + rand(TILE - 4), 1 + rand(TILE - 4)
        width = 2 + rand(3)
        canvas.rect(x, y, x + width, y + 1, ctx["paving"]["dark"])
        canvas.hline(x, x + width, y, ctx["paving"]["light"])
        canvas.set(x + width, y + 1, ctx["paving"]["deep"])
    for _ in range(2):  # 冲沟：斜着走的一条暗痕，雨水把碎石往下带
        x, y = 2 + rand(TILE - 8), 6 + rand(TILE - 12)
        for step in range(5):
            canvas.set(x + step, y - step, pal["deep"])


def paint_mountain_granite(canvas, rand, pal, ctx):
    """石阶：两条大条石上下叠，上一层的顶行走最亮档——那是"踩上去的阶面"。"""
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["deep"])
    ys = (0, 13 + rand(6), TILE - 1)
    for row in range(2):
        y0, y1 = ys[row] + 1, ys[row + 1] - 1
        canvas.rect(0, y0, TILE - 1, y1, pal["mid"] if row else pal["light"])
        canvas.hline(0, TILE - 1, y0, pal["light"] if row else pal["mid"])
        canvas.hline(0, TILE - 1, y1, pal["dark"])
    for _ in range(3):  # 凿痕：断续的竖点，一亮一暗交替才读得出是錾子活的方向
        x, y = 2 + rand(TILE - 5), 2 + rand(TILE - 8)
        canvas.vline(x, y, y + 2, pal["dark"] if rand(2) else pal["light"])


def paint_mountain_cliff(canvas, rand, pal, ctx):
    """岩壁本体：水平层理 + **竖向节理**，缝里生苔。

    第一版只画层理（整条亮档/暗档横跨 32 像素、每格独立随机），274 格铺开后读成"瓦楞铁皮"
    而不是山岩——离屏预览目验抓出来的（`.temp/mp_preview.png`，M72-11 山道段）。
    两处修法：① 亮暗档之间垫 `mid`，把相邻层的对比从 light↔dark 降到 light↔mid；
    ② 每层再砍 1–2 道竖向节理（`deep` 竖线 + 右侧一列 `light` 高光），层理因此被切成石块，
    读感从"条纹"变"凿面"。苔仍只在暗档层顶。
    """
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["mid"])
    y = 0
    index = 0
    while y < TILE:
        height = 4 + rand(4)
        bottom = min(TILE - 1, y + height - 1)
        face = pal["light"] if index % 2 else pal["mid"]
        canvas.rect(0, y, TILE - 1, bottom, face)
        canvas.hline(0, TILE - 1, bottom, pal["deep"])
        for x in range(2, TILE, 5 + rand(4)):
            canvas.set(x, y + rand(max(1, bottom - y)), pal["mid"])
        joints = 1 + rand(2)
        for joint in range(joints):  # 竖向节理：把这一层切成两三块
            jx = 3 + joint * (TILE // (joints + 1)) + rand(4)
            if jx >= TILE - 1:
                continue
            canvas.vline(jx, y, bottom - 1, pal["deep"])
            if jx + 1 < TILE:  # 节理右壁接光：一条竖线不读成裂缝，配上右壁才读成"劈开的石面"
                canvas.vline(jx + 1, y, bottom - 1, pal["light"] if index % 2 else pal["mid"])
        if index % 2:  # 苔只长在暗档那一层的顶上（阴面才留得住水）
            canvas.hline(1 + rand(6), 1 + rand(6) + 3, y, ctx["families"]["grass"]["light"])
        y = bottom + 1
        index += 1


def paint_mountain_cliff_edges(canvas, seed, mask, pal, ctx):
    """岩壁收口：顶沿是苔草、墙脚是塌下来的碎石坡、两端是墨线断点。

    山庄院墙顶是草帽沿、村庄白墙顶是黛瓦压顶，这里顶沿直接是**林下的草**——山道的墙不是
    砌出来的，是凿出来的，所以"墙顶"就是山体表面接住落土的那一条。
    """
    exposed = tk.mask_edges(mask)
    grass, scree = ctx["families"]["grass"], ctx["families"]["scree"]
    if "top" in exposed:
        for x in range(TILE):
            depth = 3 + (seed + x) % 3
            canvas.rect(x, 0, x, depth - 1, grass["dark"])
            canvas.set(x, 0, grass["light"])
            if (seed + x * 7) % 5 == 0:
                canvas.set(x, depth, grass["mid"])
    if "bottom" in exposed:
        for x in range(TILE):
            depth = 2 + (seed + x * 3) % 3
            for d in range(depth):
                canvas.set(x, TILE - 1 - d, scree["dark"] if d else scree["light"])
    for bit, edge in ((3, "left"), (1, "right")):
        if edge in exposed:
            coord = EDGE_COORD[edge]
            for t in range(TILE):
                canvas.set(*coord(t, 0), pal["deep"])
                canvas.set(*coord(t, 1), pal["dark"])
                if (seed + t * 11) % 6:
                    canvas.set(*coord(t, 2), ctx["utility"]["ink"])


def paint_mountain_bamboo(canvas, rand, pal, ctx):
    """竹林地：黄绿落叶层 + 两条横走竹鞭 + 一撇一撇的竹叶。"""
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["mid"])
    _speckles(canvas, rand, [pal["deep"], pal["dark"], pal["light"]], 160)
    for row in (8, 22):
        y = row + (rand(3) - 1)
        # 竹鞭走**断线**而不是满宽 2 像素实线：满宽实线在 89 格铺开后就成了一把尺子，
        # 读不出"地里横走的根茎"（山道目验抓出来的第二条，同 paint_mountain_cliff 那个毛病）。
        start = rand(4)
        for x in range(start, TILE):
            if (x - start) % 4 == 3:
                continue
            canvas.set(x, y, ctx["utility"]["wood"])
            canvas.set(x, y + 1, pal["deep"])
        for x in range(3, TILE, 9):
            canvas.set(x, y, pal["light"])
    for _ in range(4):
        x, y = 2 + rand(TILE - 6), 4 + rand(TILE - 8)
        canvas.set(x, y, pal["light"])
        canvas.set(x + 1, y - 1, pal["dark"])
        canvas.set(x + 2, y - 2, pal["deep"])


def paint_mountain_stream(canvas, rand, pal, ctx):
    """山涧：比村溪急——水口白带更密、露出的圆石更大。"""
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["dark"])
    _speckles(canvas, rand, [pal["deep"]], 60)
    for row in (4, 12, 20, 28):
        x = rand(4)
        while x < TILE - 5:
            width = 4 + rand(4)
            y = row + (rand(3) - 1)
            right = min(TILE - 1, x + width)
            canvas.hline(x, right, y, pal["mid"])
            canvas.hline(x + 1, right, y - 1, pal["light"])
            x += width + 2 + rand(4)
    for _ in range(3):
        x, y = 2 + rand(TILE - 6), 2 + rand(TILE - 6)
        canvas.rect(x, y, x + 2, y + 2, ctx["paving"]["dark"])
        canvas.hline(x, x + 2, y, ctx["paving"]["light"])
        canvas.set(x + 2, y + 2, ctx["paving"]["deep"])


# ---------------------------------------------------------------------------
# 山庄·演武场（M72-11 第三张铺量的图）
#
# 身份差别写在 `terrain_ramp.json:scenes.combat_arena`。三张户外图里只有演武场是**人造场地**：
# docs/50 的口径是场院铺大方砖、院墙顺砌青砖、墙头压黛瓦，所以母题是"踩"与"砌"——草皮有
# 修剪纹与踩秃的土斑，砖位由常数定死而不是随 rand 漂。
#
# 砖缝是直的不违反 docs/71 §1 那条"直线交界像 demo"：那条判的是**两种 kind 的交界**画成
# 一把尺子，而人造铺地本来就有直角缝；交界的犬牙仍由 `_edge_stone` 与
# `set_terrain_autotile.jitter` 负责。这里为砖块专门写了一条 `跨瓦一致` 的规矩（见
# `paint_arena_flagstone` 的 `ti`）：一张瓦只有一个图像（同一 kind 的 17 档共用同一张本体），
# 所以半块砖骑在瓦边上时，两半的**色档必须由同一个键算出来**，否则缝两侧会各拼出一块
# 不同明度的砖，铺开后是锯齿而不是一条线。
# ---------------------------------------------------------------------------


def paint_arena_turf(canvas, rand, pal, ctx):
    """庭院草皮：横向修剪纹 + 踩秃的土斑 + 矮草簇。"""
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["mid"])
    for x in range(TILE):
        # 带界每 8 列挪 2px：式子只看瓦内列号，所以相邻瓦的纹首尾接得上，而边界不是一条尺子线。
        shift = ((x // 8) % 2) * 3
        for y in range(TILE):
            if ((y + shift) // 8) % 2 == 0:
                canvas.set(x, y, pal["light"])
    _speckles(canvas, rand, [pal["deep"], pal["dark"], pal["light"]], 170)
    for _ in range(2):  # 踩秃的土斑：露出夯土族。**抖动**着露，方块边读成"啃出来的"而不是贴纸
        x, y = 2 + rand(TILE - 10), 3 + rand(TILE - 7)
        width, height = 4 + rand(4), 2 + rand(3)
        for dy in range(height):
            for dx in range(width):
                if (dx + dy + x + y) % 3 == 2:
                    continue
                canvas.set(x + dx, y + dy, ctx["soil"]["dark"] if dy == 0 else ctx["soil"]["mid"])
    for _ in range(4):  # 草簇：与山庄同款三叶，但矮半格（庭院是被踩出来的）
        x, y = 1 + rand(TILE - 3), 2 + rand(TILE - 5)
        canvas.vline(x, y - 1, y, pal["dark"])
        canvas.vline(x - 1, y, y, pal["deep"])
        canvas.vline(x + 1, y, y, pal["light"])


def paint_arena_loam(canvas, rand, pal, ctx):
    """夯土练武位：三道耙痕 + 两只脚印 + 嵌进土的碎石。

    这一档在本场景**零格**（docs/50 铁律 2：farm.lua 判 tile==2 可垦），但满块档必须有真
    美术——`check` 查的就是"关掉 autotile 会不会画出空白"，而恒等寻址是回滚路线。
    """
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["mid"])
    _speckles(canvas, rand, [pal["deep"], pal["dark"], pal["light"]], 150)
    for row in (5, 14, 23):
        x = 0
        while x < TILE:
            width = 4 + rand(5)
            y = row + (rand(3) - 1)
            right = min(TILE - 1, x + width)
            canvas.hline(x, right, y, pal["deep"])
            canvas.hline(x, right, y + 1, pal["light"])
            x = right + 1
    for _ in range(2):  # 脚印：一掌宽的暗坑，前掌亮一档才读得出是踩下去的
        x, y = 2 + rand(TILE - 5), 3 + rand(TILE - 8)
        canvas.rect(x, y, x + 1, y + 2, pal["deep"])
        canvas.hline(x, x + 1, y, pal["dark"])
    for _ in range(3):
        _pebble(canvas, rand, ctx["paving"])
    for _ in range(2):
        _twig(canvas, rand, ctx["utility"]["wood"])


def paint_arena_flagstone(canvas, rand, pal, ctx):
    """场院大方砖：16×6 的砖按顺缝排，隔行错半块，缝是直的、缝里点白灰、砖面中心磨亮。"""
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["deep"])
    faces = (pal["mid"], pal["light"], pal["mid"], pal["dark"])
    for row in range(4):
        y0 = row * 8
        for x0 in ((0, 16) if row % 2 == 0 else (-8, 8, 24)):
            # 同一块砖在左右两张瓦里算出同一个键：`-8` 与 `24` 是同一块砖的两半。
            # 四格一循环（不是三格）是目验定的：三档里 `light` 占了两席，整片场院铺开后
            # 白得压过了草皮，读成"刚铺的"而不是"踩亮的"。
            ti = (((x0 + 16) % 32) // 16 + 2 * row) % 4
            left, right = max(0, x0 + 1), min(TILE - 1, x0 + 14)
            canvas.rect(left, y0 + 1, right, y0 + 6, faces[ti])
            canvas.hline(left, right, y0 + 1, pal["light"])
            canvas.hline(left, right, y0 + 6, pal["dark"])
            canvas.vline(right, y0 + 1, y0 + 6, pal["deep"])
    for x in range(TILE):  # 勾缝断续：只在横缝上点白灰，实线缝读成尺子（四点一点，密了就成缝线）
        for y in (7, 15, 23, 31):
            if rand(4) == 0:
                canvas.set(x, y, ctx["utility"]["paper"])
    for _ in range(18):  # 中心磨亮：碎点只打在暗档与中档砖面上
        x, y = rand(TILE), rand(TILE)
        if canvas.get(x, y)[:3] in (pal["mid"][:3], pal["dark"][:3]):
            canvas.set(x, y, pal["light"])


def paint_arena_brick(canvas, rand, pal, ctx):
    """青砖顺砌：5px 一层的通长砖，隔层错半块。

    山庄院墙是圆石砌体（`paint_masonry`：宽高全随机 = 随手垒的），演武场是烧制砖：砖位由
    常数决定，**只有砖面斑驳与缝里的苔是随机的**——那是岁月，不是砌法。
    """
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["deep"])
    faces = (pal["mid"], pal["dark"], pal["mid"], pal["light"])
    for index, y0 in enumerate(range(0, TILE, 5)):
        y1 = min(TILE - 1, y0 + 3)
        for x0 in ((0, 16) if index % 2 == 0 else (-8, 8, 24)):
            ti = (((x0 + 16) % 32) // 16 + index) % 4
            left, right = max(0, x0 + 1), min(TILE - 1, x0 + 14)
            canvas.rect(left, y0, right, y1, faces[ti])
            canvas.hline(left, right, y0, pal["mid"] if ti == 3 else pal["light"])
            canvas.hline(left, right, y1, pal["deep"])
            canvas.vline(right, y0, y1, pal["deep"])
    for _ in range(22):  # 砖面斑驳
        canvas.set(rand(TILE), rand(TILE), pal["dark"] if rand(3) else pal["deep"])
    turf = ctx["families"]["courtyard_turf"]
    for _ in range(4):  # 缝里生苔：两档草色，只落在横缝那一行
        x, y = rand(TILE), rand(6) * 5 + 4
        canvas.set(x, min(TILE - 1, y), turf["dark"])
        canvas.set(x + 1 if x + 1 < TILE else x - 1, min(TILE - 1, y), turf["light"])


def paint_arena_pool(canvas, rand, pal, ctx):
    """园池：山庄池塘那种斑块水 + 水下青石 + 两片浮萍（演武场的池是园景，不是野溪）。"""
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["dark"])
    _speckles(canvas, rand, [pal["deep"], pal["mid"]], 70)
    for _ in range(2):
        x, y = rand(TILE - 9), rand(TILE - 7)
        canvas.rect(x, y, x + 6 + rand(3), y + 3 + rand(3), pal["deep"])
    for _ in range(2):  # 水下青石：浅得能看见底（一瓦两块就够，多了读成河床而不是一池水）
        x, y = 2 + rand(TILE - 8), 2 + rand(TILE - 8)
        canvas.rect(x, y, x + 3, y + 2, ctx["paving"]["dark"])
        canvas.hline(x, x + 2, y, ctx["paving"]["mid"])
    turf = ctx["families"]["courtyard_turf"]
    for _ in range(1):  # 浮萍：草族两档的圆片，缺角那一格留回水色
        x, y = 2 + rand(TILE - 6), 2 + rand(TILE - 6)
        canvas.rect(x, y, x + 2, y + 2, turf["dark"])
        canvas.hline(x, x + 2, y, turf["light"])
        canvas.set(x + 2, y + 2, pal["mid"])
    for _ in range(4):
        x, y = rand(TILE - 7), rand(TILE - 2)
        width = 2 + rand(4)
        canvas.hline(x, x + width, y, pal["light"])
        canvas.hline(x, x + width - 1, y + 1, pal["mid"])


def paint_arena_brick_edges(canvas, seed, mask, pal, ctx):
    """青砖墙的三段式收口：压顶黛瓦（上）/ 青石基脚（下）/ 砖端面（左右）。

    结构与徽派白墙那一段一模一样（docs/53 §3.3：顶、身、基），换的是材料与一个母题——
    墙头的瓦每四列画一枚**瓦当**（`mid` 圆头）：演武场在山庄墙内，瓦作是官式做法，
    村里那种马头墙收边在这里读起来就不对。
    """
    exposed = tk.mask_edges(mask)
    tile = ctx["families"]["blue_tile"]
    if "top" in exposed:
        for x in range(TILE):
            depth = 5 + (seed + x) % 3
            canvas.rect(x, 0, x, depth - 2, tile["dark"])
            canvas.set(x, 0, tile["light"])
            canvas.set(x, depth - 1, tile["deep"])
            if (seed + x) % 4 == 0:
                canvas.set(x, depth - 2, tile["mid"])
    if "bottom" in exposed:
        paving = ctx["paving"]
        for x in range(TILE):
            depth = 4 + (seed + x * 3) % 3
            for d in range(depth):
                canvas.set(x, TILE - 1 - d, paving["deep"] if d == 0 else paving["mid"])
            if (seed + x * 5) % 8 == 0:
                canvas.set(x, TILE - depth, paving["light"])
    for edge in ("left", "right"):
        if edge in exposed:
            coord = EDGE_COORD[edge]
            for t in range(TILE):
                canvas.set(*coord(t, 0), pal["deep"])
                canvas.set(*coord(t, 1), pal["dark"])
                if (seed + t * 13) % 7:
                    canvas.set(*coord(t, 2), ctx["utility"]["ink"])


# ---------------------------------------------------------------------------
# 寒泉洞（M72-11 第四张铺量的图，第一张室内/地下图）
#
# 身份差别写在 `terrain_ramp.json:scenes.cold_spring_cave`。这张要回答的是 plan §推进顺序 4
# 点名的那条：**"单一光向"规则在暗场景怎么落**。洞里既没有太阳也没有第二光源
# （`scripts/world/lighting.lua` 不分场景档；火把的局部暖点要等光系统，本包不发明它），
# 所以这里的落法不是"再画一层阴影"，而是**把亮度层次压进色阶表的使用方式里**：
#   * 本体满块的底色一律 `dark`（户外三图用 `mid`）——整幅的读感先暗下来；
#   * `light` 只给**沿**：层顶、板块上沿、冰针尖端、收口碎点，宽度 ≤2px，绝不铺成面
#     （铺成面在暗场景里读成过曝，不读成"这里有光"）；
#   * 层次由 deep↔dark↔mid 三档承担，于是"亮处"是**少数像素**而明度跨度仍在——
#     `check` 的"每族至少用上 3 档"与"每条暴露边改 ≥8 像素"两条门同时成立。
# 全图最深的一族给洞壁（`cave_rock` 最深档亮度 25.7，比山庄夯土的最深档 40.1 还低）、
# 最亮的一族给冰晶地（`ice_crust` 最亮档 169.3）：这两族不是"一块暗石和一块亮石"，
# 而是光在洞里的**去向**——水与冰聚住光，岩石吃掉光。
# ---------------------------------------------------------------------------


def paint_cave_floor(canvas, rand, pal, ctx):
    """洞底石板：暗档打底 + 滴水蚀坑 + 一层从洞壁塌下来的扁碎屑。"""
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["dark"])
    _speckles(canvas, rand, [pal["deep"], pal["mid"]], 150)
    for _ in range(2):  # 滴水蚀坑：坑底最深，只有**上沿**抬一档（受光的是朝上那张脸）
        x, y = 2 + rand(TILE - 8), 3 + rand(TILE - 8)
        width, height = 4 + rand(4), 3 + rand(3)
        canvas.rect(x, y, x + width, y + height, pal["deep"])
        canvas.hline(x, x + width, y, pal["mid"])
    for _ in range(5):  # 塌落碎屑：宽 3 高 1 的扁片（洞壁是片岩，崩下来就不是圆的）
        x, y = 1 + rand(TILE - 5), 1 + rand(TILE - 3)
        width = 2 + rand(2)
        canvas.hline(x, x + width, y, ctx["soil"]["dark"])
        canvas.set(x, y, ctx["soil"]["light"])
    for _ in range(2):  # 水痕：暗河回水带上来的湿线，比蚀坑更深一档
        x, y = 1 + rand(TILE - 3), 4 + rand(TILE - 8)
        canvas.vline(x, y, y + 3 + rand(3), pal["deep"])


def paint_cave_scree(canvas, rand, pal, ctx):
    """塌落碎石堆：叠起来的扁片岩，缝里是全图最深的洞壁色。

    这一档在本场景**零格**（理由写在色阶表的 comment：farm.lua 判 tile==2 可垦、没有场景门），
    但满块档必须有真美术——恒等寻址是回滚路线，`check` 查的就是关掉 autotile 会不会画空白。
    """
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["deep"])
    _speckles(canvas, rand, [pal["dark"], pal["mid"]], 120)
    for _ in range(9):  # 扁片岩：顶行最亮、底行最深（单一光向：朝上的那张脸）
        x, y = 1 + rand(TILE - 6), 1 + rand(TILE - 4)
        width = 3 + rand(4)
        canvas.rect(x, y, x + width, y + 1, pal["mid"])
        canvas.hline(x, x + width, y, pal["light"])
        canvas.hline(x, x + width, y + 1, pal["deep"])
    rock = ctx["families"]["cave_rock"]
    for _ in range(3):  # 缝里的湿暗：石堆的空隙读作洞壁色，碎石因此是"从这个壁上掉下来的"
        x, y = rand(TILE), rand(TILE)
        canvas.rect(x, y, min(TILE - 1, x + 1), min(TILE - 1, y + 1), rock["deep"])
    for _ in range(2):  # 坑道木：洞里的枯枝是塌下来的支护，不是落叶
        _twig(canvas, rand, ctx["utility"]["wood"])


def paint_cave_hewn(canvas, rand, pal, ctx):
    """凿平甬道：一块大錾面 + 一道**断续**的导水槽 + 几道竖向錾口。

    洞里的"路"不是铺出来的，是从石床上錾出来的，所以没有村街那种大块拼花；`building.lua`
    认 tile==3 是道路，这条錾平石就是洞穴的主通道。水槽用比面还深的两档画，读作"水从这儿走"。

    第一版是"两块通长条石 + 每条石满宽亮上沿"，28 格的甬道一铺开就读成**一叠木板**——
    满宽横线每 32px 重复一次就是尺子，跟洞壁那面的格栅是同一个成因。改法三条：缝只留一道、
    并让它沿列号漂 ±1px 且带断口；下半块的面压到 `dark`；`light` 全瓦只剩上沿那一行、
    且四列里断两列。
    """
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["deep"])
    seam = 12 + rand(8)
    canvas.rect(0, 0, TILE - 1, seam - 2, pal["mid"])
    canvas.rect(0, seam + 2, TILE - 1, TILE - 1, pal["dark"])
    for x in range(TILE):  # 导水槽：带界随列号小步漂，每五列断一次
        y = seam + ((x // 7) % 3) - 1
        if (x + seam) % 5 == 0:
            continue
        canvas.set(x, y - 1, pal["deep"])
        canvas.set(x, y, pal["deep"])
        canvas.set(x, y + 1, pal["mid"])  # 槽底只反光一档，不给 light
    for x in range(0, TILE, 11):  # 竖向錾口：把大面切成几块，缝是深的
        canvas.vline(x + 3 + rand(3), 0, seam - 2, pal["deep"])
    for x in range(TILE):  # 受光沿：断续的四列两列，满宽亮线在暗场景里最扎眼
        if (x + seam) % 4 < 2:
            canvas.set(x, 0, pal["light"])
    for _ in range(5):  # 錾痕：3px 的斜撇，深档压在中档面上才读得出錾子的方向
        x, y = 2 + rand(TILE - 6), 3 + rand(TILE - 4)
        tone = pal["deep"] if y < seam else pal["mid"]
        for step in range(3):
            canvas.set(x + step, y - step, tone)


def paint_cave_wall(canvas, rand, pal, ctx):
    """洞壁：层理切成石块 + 短节理 + 流石挂面，全图最深的一族。

    与山道岩壁（`paint_mountain_cliff`）的区别不是"更暗一点的同一种石头"：山道的壁是**山体**
    （层理被节理切成块、缝里生苔），这里的壁是**被水掏出来的洞壁**（缝里长苔这件事在没有光的
    洞里不成立，所以苔一档都不给）。

    第一版铺 66 格就读成**金属格栅**：每条层的顶行拉满 32px 的亮档、节理又通高，横竖都是尺子。
    山道那次已经为同一个毛病改过一轮（"瓦楞铁皮"，见 `paint_mountain_cliff` 的注释），
    这里更严重是因为洞壁是全场格数最多的一族。改法：① 层加厚（6-10px）并把每层再切成
    2-3 块石，块间一道深缝——亮档因此只剩几段而不是一条线；② 受光沿只给偶数层、且过半概率、
    且沿列断口；③ 节理改成"只穿过一到两层"的短线；④ 流石挂面压到 `mid`，`light` 留给层沿。
    """
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["dark"])
    y = 0
    index = 0
    while y < TILE:
        height = 6 + rand(5)
        bottom = min(TILE - 1, y + height - 1)
        canvas.rect(0, y, TILE - 1, bottom, pal["mid"] if index % 2 == 0 else pal["dark"])
        x = 0
        while x < TILE:  # 把这一层切成石块：缝比面深一档，块顶才谈得上受光
            width = 9 + rand(9)
            right = min(TILE - 1, x + width - 1)
            canvas.hline(x, right, y, pal["deep"])
            if index % 2 == 0 and rand(2):
                for t in range(x, right + 1):
                    if (t + index * 5) % 3:
                        canvas.set(t, y, pal["light"] if t == x else pal["mid"])
            if rand(3) == 0:  # 层内的横向裂隙：短，不给第二道光
                x0 = x + 1 + rand(max(1, right - x - 1))
                canvas.hline(x0, min(right, x0 + 3 + rand(4)),
                             y + 2 + rand(max(1, bottom - y - 2)), pal["deep"])
            if right < TILE - 1:
                canvas.vline(right + 1, y, bottom, pal["deep"])
            x = right + 2
        y = bottom + 1
        index += 1
    for _ in range(2):  # 竖向节理：只穿过一到两层，右壁接一档光
        x = 2 + rand(TILE - 4)
        top = rand(TILE - 12)
        bottom = min(TILE - 1, top + 7 + rand(8))
        canvas.vline(x, top, bottom, pal["deep"])
        canvas.vline(x - 1 if x else x, top, bottom, pal["mid"])
    for _ in range(2):  # 流石挂面：自壁顶垂下来的一线湿痕
        x = rand(TILE)
        canvas.vline(x, 0, 4 + rand(6), pal["mid"])


def paint_cave_ice(canvas, rand, pal, ctx):
    """冰晶地：暗底冰壳 + 一簇簇斜插的冰针，全图最亮的一族。像山道的竹林地一样真铺在地表。

    第一版底色 `dark` + 130 个均匀碎点 + 两根横缝 + 两汪融水，20 格铺开后读成**泳池瓷砖**：
    均匀的盐噪配上规则的暗块，没有方向也没有"结在石头上"的关系。改法是压底色、撤碎点密度、
    把亮集中到**针尖**（`light` 全瓦只有针尖那几个像素），暗缝改成斜着走、底下读出洞底石色。
    """
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["deep"])
    _speckles(canvas, rand, [pal["dark"], pal["mid"]], 90)
    for _ in range(2):  # 壳下的暗缝：斜着走的一道，露出洞底石
        x, y = 1 + rand(TILE - 9), 1 + rand(TILE - 6)
        lean = rand(3) - 1
        for step in range(5 + rand(4)):
            canvas.set(x + step, y + (step * lean) // 2,
                       ctx["families"]["cave_floor"]["deep"])
    for _ in range(8):  # 冰针：2-3 根一簇，根部暗、针身 mid、只有尖端给 light
        x, y = 2 + rand(TILE - 6), 5 + rand(TILE - 9)
        for i in range(2 + rand(2)):
            top = y - rand(2)
            canvas.set(x + i, top, pal["light"])
            canvas.set(x + i, top + 1, pal["mid"])
            canvas.set(x + i, top + 2, pal["dark"])
        canvas.set(x - 1, y + 1, ctx["families"]["cave_water"]["deep"])  # 根部的融水


def paint_cave_darkwater(canvas, rand, pal, ctx):
    """暗河：静水。户外的水有五到六条亮波纹（太阳下的碎光），这里只留**一条**受光带。

    水面之下的东西在洞里读不出来，读得出来的只有反光的方向——所以水下石只给 `deep`/`dark`
    两档，一律不给亮；那条受光带按列号小步漂，满宽直线在暗场景里最扎眼（docs/71 §1）。
    """
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["deep"])
    _speckles(canvas, rand, [pal["dark"]], 70)
    for _ in range(2):  # 回水涡：比主体还深一档
        x, y = rand(TILE - 9), rand(TILE - 7)
        canvas.rect(x, y, x + 6 + rand(3), y + 3 + rand(3), pal["deep"])
    band = 6 + rand(20)
    for x in range(TILE):  # 受光带：一线 mid，其上 1px light
        y = band + ((x // 5) % 3) - 1
        canvas.set(x, y, pal["mid"])
        canvas.set(x, y - 1, pal["light"])
    for _ in range(2):  # 水下石：只读得出轮廓，读不出颜色
        x, y = 3 + rand(TILE - 8), 3 + rand(TILE - 8)
        canvas.rect(x, y, x + 3, y + 2, ctx["families"]["cave_rock"]["deep"])
        canvas.hline(x, x + 2, y, ctx["paving"]["dark"])


def paint_cave_wall_edges(canvas, seed, mask, pal, ctx):
    """洞壁收口：顶沿是壁面朝里弯的流石、墙脚是塌下来的碎石坡、两端是墨线断点。

    山庄院墙顶是草帽沿、村庄白墙顶是黛瓦压顶、山道岩壁顶是林下的草——洞穴的壁顶什么都没有，
    只有一道被水磨圆的檐，所以上沿直接是本族色阶；下沿反过来用 `ctx["soil"]`（cave_scree），
    因为墙脚那圈碎石是墙自己的产出，不是地面的草或土。
    """
    exposed = tk.mask_edges(mask)
    scree = ctx["soil"]
    if "top" in exposed:
        for x in range(TILE):
            depth = 4 + (seed + x) % 3
            canvas.rect(x, 0, x, depth - 2, pal["mid"])
            canvas.set(x, 0, pal["light"])
            canvas.set(x, depth - 1, pal["deep"])
            if (seed + x * 5) % 6 == 0:
                canvas.set(x, depth, scree["dark"])
    if "bottom" in exposed:
        for x in range(TILE):
            depth = 3 + (seed + x * 3) % 3
            for d in range(depth):
                canvas.set(x, TILE - 1 - d,
                           scree["deep"] if d == 0 else
                           (scree["light"] if d == depth - 1 else scree["dark"]))
    for edge in ("left", "right"):
        if edge in exposed:
            coord = EDGE_COORD[edge]
            for t in range(TILE):
                canvas.set(*coord(t, 0), pal["deep"])
                canvas.set(*coord(t, 1), pal["dark"])
                if (seed + t * 11) % 7:
                    canvas.set(*coord(t, 2), ctx["utility"]["ink"])


def paint_cave_ice_edges(canvas, seed, mask, pal, ctx):
    """冰壳的崩边：断裂面压最深档、内侧留 mid，零星一根 `light` 针尖朝外。"""
    for bit in range(4):
        edge = tk.EDGE_BY_BIT[bit]
        if edge not in tk.mask_edges(mask):
            continue
        rand = hash_stream(seed + bit * 131 + 9)
        coord = EDGE_COORD[edge]
        for t, depth in enumerate(edge_profile(rand)):
            span = max(depth, 2)
            for d in range(span):
                canvas.set(*coord(t, d), pal["deep"] if d == span - 1 else pal["mid"])
            if rand(3) == 0:
                canvas.set(*coord(t, 0), pal["light"])
            if rand(5) == 0:
                canvas.set(*coord(t, 1), ctx["families"]["cave_water"]["deep"])


def paint_cave_shore_edges(canvas, seed, mask, pal, ctx):
    """暗河岸：docs/53 §3.7 的"先画 rim 再画水"照旧，换两件事——rim 是**湿石**不是土岸，
    水侧不给 `light` 浪花（洞水没有白浪），只给一线 `mid` 反光 + 深档压边。"""
    for bit in range(4):
        edge = tk.EDGE_BY_BIT[bit]
        if edge not in tk.mask_edges(mask):
            continue
        rand = hash_stream(seed + bit * 131 + 7)
        coord = EDGE_COORD[edge]
        for t in range(TILE):
            rim = 2 + rand(3)
            for d in range(rim):  # 湿石 rim：外列最深，内列给凿石族的暗档
                canvas.set(*coord(t, d), ctx["soil"]["deep"] if d == 0 else ctx["paving"]["dark"])
            if rand(4) == 0:
                canvas.set(*coord(t, rim), pal["mid"])
            else:
                canvas.set(*coord(t, rim), pal["deep"])
            for d in range(rim + 1, rim + 4):
                canvas.set(*coord(t, d), pal["deep"])
            if rand(7) == 0:  # 岸上的冰碴：寒泉的冷气只在贴岸那一圈结得出东西
                canvas.set(*coord(t, rim + 1), ctx["families"]["ice_crust"]["dark"])


# ---------------------------------------------------------------------------
# 铁匠铺内（M72-11 §推进顺序 5 的第一张：室内图自己的图集）
#
# 身份差别写在 `terrain_ramp.json:scenes.interior_forge`（换序的三条理由与 kind 2 零格那条
# 都在那里，这里只记画法）。这张与前四张的**机制**一字不差（12×9、1+16 条目算术、4-bit
# 位序、碎点压边、满块档 = mask 15），要回答的是另一问：**一张室内的地表怎么读得出"这是
# 作坊，不是随便一间屋"**。三条落法：
#   * 六族里只有两族是"地面"（夯土、石板），其余四格各有身份：炭渣是炉子的**产出**、
#     耐火砖是**炉前那两块被火烧亮、被锤屑砸出坑的方砖**、砖墙是外圈、水槽是不能走的方槽。
#     所以本体画法不是一套配方换六遍色：每种材质各写一条"只有这张图会这样画"的母题
#     （夯土的夯层缝、炭渣的余烬、耐火砖的锤坑、砖墙的缺角砖与烟炱、水槽的水下铁条）。
#   * **暖色只有两处**：`cinder_scree` 的最亮档（余烬橙）与 `hearth_brick` 整族。前者落在
#     零格的那一列上（所以全图唯一的暖面是炉前），后者是炉前——这两件事合起来就是
#     "铁匠铺的亮是火光不是天光"，也是 `scene_readings` 的 D5 复核要看的那档（满块格数
#     够不够多、tint 跨度能不能读出来）。
#   * 墙的三段式在室内换材料不换结构（docs/53 §3.3）：顶是**砖檐压顶 + 檐下木梁 + 梁下
#     一条墨影**，底是**青石基与地面相接**，左右是砖端面 + 墨断点。山庄用草帽沿、村庄用
#     黛瓦、演武场用官式瓦当、洞穴什么都没有——室内本来就没有檐口外的天空，所以第一段
#     收的是"屋子里侧的檐"，不是屋外那条线。
# 人造铺地的直缝不违反 docs/71 §1（那条判的是**两种 kind 的交界**画成一把尺子）；沿用的
# 仍是演武场那条 `跨瓦一致` 规矩：同一块砖被瓦边切成两半时，两半的色档由同一个键算出来。
# ---------------------------------------------------------------------------


def paint_forge_earth(canvas, rand, pal, ctx):
    """夯土地面：三遍夯层的横向暗沿 + 踩秃的浅坑 + 铁屑压痕。

    与演武场的夯土练武位（`paint_arena_loam`）不是一回事：那里是耙痕（人为整地的动作），
    这里是**夯层**（一层一层填土砸实的界面）——缝是横向、成层、间距不等的，而坑是长期
    踩踏磨出来的。煤渣点来自 `ctx["soil"]`（cinder 族的深档）：作坊地上的黑点是炉子掉的，
    不是泥土本身的颜色。
    """
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["mid"])
    for y in (7, 15, 24):  # 夯层缝：三条不等距，每条沿列号小步漂、带断口
        x = 0
        while x < TILE:
            width = 5 + rand(6)
            right = min(TILE - 1, x + width)
            line = y + (rand(3) - 1)
            if rand(6):
                canvas.hline(x, right, line, pal["dark"])
                canvas.hline(x, right, line + 1, pal["light"])  # 层界面砸实时露出的湿砂，比面亮
            x = right + 2
    _speckles(canvas, rand, [pal["deep"], pal["dark"], pal["light"]], 130)
    for _ in range(2):  # 踩秃的浅坑：坑底最深，只有上沿抬一档（受光的是朝上那张脸）
        x, y = 2 + rand(TILE - 10), 3 + rand(TILE - 9)
        width, height = 4 + rand(5), 3 + rand(3)
        canvas.rect(x, y, x + width, y + height, pal["deep"])
        canvas.hline(x, x + width, y, pal["dark"])
        for dx in range(width):
            if rand(4) == 0:
                canvas.set(x + dx, y + height, pal["mid"])
    for _ in range(5):  # 铁屑与煤渣压痕：2-3px 斜撇，深档压在中档面上才读得出方向
        x, y = 2 + rand(TILE - 6), 4 + rand(TILE - 7)
        for step in range(2 + rand(2)):
            canvas.set(x + step, y - step,
                       ctx["soil"]["deep"] if rand(3) else pal["deep"])


def paint_forge_cinder(canvas, rand, pal, ctx):
    """炭渣与铁屑堆：黑渣成堆、余烬只给零星针尖大的橙。

    这一档在本场景**零格**（理由写在色阶表 comment 的 farm.lua 那三条），但满块档必须有真
    美术——恒等寻址是回滚路线，`check` 查的就是关掉 autotile 会不会画空白。与山庄 dirt /
    演武场 loam 同一口径登记。

    `light` 档是余烬橙（#8A4A26），所以这里 `light` 一律**不成面**：一只余烬只有两三个像素，
    一整片橙读成"地上刷了漆"而不是一堆还有火心的渣。铁屑用石板族的亮档：那是从铁砧上
    崩下来的金属，与石头的明度同一档，读起来才对。
    """
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["dark"])
    _speckles(canvas, rand, [pal["deep"], pal["mid"]], 150)
    for _ in range(4):  # 渣块：扁片状，顶行亮、底行最深（单一光向：朝上的那张脸）
        x, y = 1 + rand(TILE - 7), 2 + rand(TILE - 6)
        width = 3 + rand(4)
        canvas.rect(x, y, x + width, y + 1, pal["mid"])
        canvas.hline(x, x + width, y, ctx["paving"]["dark"])
        canvas.hline(x, x + width, y + 2, pal["deep"])
    for _ in range(3):  # 余烬：2-3 个像素一撮，橙只出现在渣堆的缝隙里
        x, y = 2 + rand(TILE - 5), 3 + rand(TILE - 7)
        canvas.set(x, y, pal["light"])
        if rand(2):
            canvas.set(x + 1, y, ctx["paving"]["deep"])
        canvas.set(x, y + 1, pal["deep"])
    for _ in range(3):  # 铁屑：石板族亮档的短横，一瓦两道就够读成"扫不干净"
        x, y = 2 + rand(TILE - 8), 3 + rand(TILE - 6)
        canvas.hline(x, x + 2 + rand(3), y, ctx["paving"]["light"])
    for _ in range(2):  # 烧裂的煤块：一圈深档、中心留中档，读得出是裂开不是个洞
        x, y = 3 + rand(TILE - 9), 3 + rand(TILE - 9)
        canvas.rect(x, y, x + 3, y + 2, pal["deep"])
        canvas.set(x + 1, y + 1, pal["mid"])


def paint_forge_flagstone(canvas, rand, pal, ctx):
    """门前与炉前的铺石：16×16 大块青石，隔行错半块，缝里点白灰，块面中心磨亮。

    与演武场的方砖（`paint_arena_flagstone`，16×8 长条）换的是**尺度**：作坊的石头要经得起
    铁块掉下来，所以是一整块整块的方石；长条砖读成"砌出来的地面"，方石读成"凿出来的"。
    跨瓦一致沿用 `ti` 那条规矩（同一块砖被瓦边切成两半时两半同档），四格一循环、`light`
    只占一席也是演武场目验定的那条（白得压过邻族就读成"刚铺的"）。
    """
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["deep"])
    faces = (pal["mid"], pal["light"], pal["dark"], pal["mid"])
    for row in range(2):
        y0 = row * 16
        for x0 in ((0, 16) if row % 2 == 0 else (-8, 8, 24)):
            ti = (((x0 + 16) % 32) // 16 + 2 * row) % 4
            left, top = max(0, x0 + 1), y0 + 1
            right, bottom = min(TILE - 1, x0 + 14), y0 + 14
            canvas.rect(left, top, right, bottom, faces[ti])
            canvas.hline(left, right, top, pal["light"] if ti != 1 else pal["mid"])
            canvas.hline(left, right, bottom, pal["dark"])
            canvas.vline(right, top, bottom, pal["deep"])
    for x in range(TILE):  # 勾缝断续：横竖缝都只在四列里点一列白灰，满线读成尺子
        for y in (15, 31):
            if rand(4) == 0:
                canvas.set(x, y, ctx["utility"]["paper"])
        if rand(5) == 0:
            canvas.set(x, 0 if rand(2) else 16, ctx["utility"]["paper"])
    for _ in range(16):  # 中心磨亮：碎点只打在暗档与中档石面上
        x, y = rand(TILE), rand(TILE)
        if canvas.get(x, y)[:3] in (pal["mid"][:3], pal["dark"][:3]):
            canvas.set(x, y, pal["light"])
    for _ in range(2):  # 石面崩角：一块深坑压在中档面上，缝边才读得出是石头不是瓷砖
        x, y = 3 + rand(TILE - 8), 3 + rand(TILE - 8)
        canvas.rect(x, y, x + 2, y + 1, pal["deep"])
        canvas.set(x, y, pal["dark"])


def paint_forge_brick(canvas, rand, pal, ctx):
    """砖墙：5px 一层的顺砌砖，隔层错半块；缺角砖与烟炱是随机的，砖位由常数决定。

    与演武场青砖（`paint_arena_brick`）同一套砌法（砖位常数 = 烧制砖的规矩），换两件事：
    缝里生的不是苔而是**烟炱**（作坊里黑的是熏的，不是长的：`ctx["soil"]` 的深档），
    以及一瓦里许一到两块**缺角砖**（`deep` 一小口）——被吊运铁件磕过的墙，砖面不该是完好的。
    """
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["deep"])
    faces = (pal["mid"], pal["dark"], pal["mid"], pal["light"])
    for index, y0 in enumerate(range(0, TILE, 5)):
        y1 = min(TILE - 1, y0 + 3)
        for x0 in ((0, 16) if index % 2 == 0 else (-8, 8, 24)):
            ti = (((x0 + 16) % 32) // 16 + index) % 4
            left, right = max(0, x0 + 1), min(TILE - 1, x0 + 14)
            canvas.rect(left, y0, right, y1, faces[ti])
            canvas.hline(left, right, y0, pal["mid"] if ti == 3 else pal["light"])
            canvas.hline(left, right, y1, pal["deep"])
            canvas.vline(right, y0, y1, pal["deep"])
    for _ in range(2):  # 缺角砖：一块砖的左上或右下啃掉一口，露出垫层的深档
        x, y = rand(TILE - 6) + 1, rand(6) * 5
        canvas.rect(x, y, x + 2, min(TILE - 1, y + 2), pal["deep"])
        canvas.set(x, y, pal["dark"])
    for _ in range(20):  # 砖面斑驳
        canvas.set(rand(TILE), rand(TILE), pal["dark"] if rand(3) else pal["deep"])
    for _ in range(3):  # 烟炱：顺着砖缝淌下来的一线黑，比斑驳更竖
        x, y = rand(TILE), rand(TILE - 10)
        canvas.vline(x, y, y + 4 + rand(5), ctx["soil"]["deep"])


def paint_forge_hearth(canvas, rand, pal, ctx):
    """炉前耐火砖：8×8 方砖 + 锤坑 + 烧裂的壳 + 溅落的火星点。

    这一档是这张图的身份，也是全图**最亮的一族**（最亮档亮度 145.7，墙族最亮档才 100.1）：
    铁砧与炉子前面那一块被火烧得发亮、被锤屑砸出坑的方砖地。像山道竹林地、洞穴冰晶地
    那样真铺在地表，所以本体与边都走自家画法（不复用 `_base_roof` 的瓦垄）。

    `light` 给砖面的**受光那一半**而不是整片：耐火砖是暗里被烤亮的，读感是"这块地面热"，
    不是"这块地面新"。锤坑与火星分两档：坑是砸下去的（坑底最深、只有上沿抬一档），
    火星是溅上来的（一两像素的亮点，落在缝里）。
    """
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["deep"])
    faces = (pal["mid"], pal["light"], pal["dark"], pal["mid"])
    for row in range(4):
        y0 = row * 8
        for x0 in ((0, 16) if row % 2 == 0 else (-8, 8, 24)):
            ti = (((x0 + 16) % 32) // 8 + 3 * row) % 4
            left, right = max(0, x0 + 1), min(TILE - 1, x0 + 6)
            canvas.rect(left, y0 + 1, right, y0 + 6, faces[ti])
            canvas.hline(left, right, y0 + 1, pal["light"] if ti != 1 else pal["mid"])
            canvas.hline(left, right, y0 + 6, pal["dark"])
            canvas.vline(right, y0 + 1, y0 + 6, pal["deep"])
    for _ in range(2):  # 锤坑：砸进砖面的一口，坑底 deep、上沿 dark 才读得出是个坑不是个洞
        x, y = 3 + rand(TILE - 9), 3 + rand(TILE - 9)
        width = 3 + rand(3)
        canvas.rect(x, y, x + width, y + 2, pal["deep"])
        canvas.hline(x, x + width, y, pal["dark"])
    for _ in range(3):  # 烧裂的壳：砖面起皮，裂口是深档的一条斜线
        x, y = 2 + rand(TILE - 6), 3 + rand(TILE - 6)
        for step in range(2 + rand(3)):
            canvas.set(x + step, y + (step // 2), pal["deep"])
    for _ in range(4):  # 溅落的火星：一两像素的亮档，缝里那一颗最亮
        x, y = rand(TILE), rand(TILE)
        canvas.set(x, y, pal["light"] if (x + y) % 8 in (0, 7) else pal["mid"])
    for _ in range(2):  # 铁屑嵌进砖缝：石板族的亮档，只有黑渣里才看得见
        x, y = 2 + rand(TILE - 6), 1 + rand(3) * 8
        canvas.set(x, y, ctx["paving"]["light"])


def paint_forge_trough(canvas, rand, pal, ctx):
    """淬火水槽：静水 + 水下一根淬过的铁条 + 一条按列号小步漂的反光带。

    与演武场的园池（`paint_arena_pool`）相反：那是给太阳看的（浮萍、水下青石、碎光），
    这里的读感必须"不动"——一槽死水，只在炉火的方位上反一道光。所以：
      * 波纹不给（`light` 只出现在那一条带上，且带界随列号漂，满宽直线最扎眼）；
      * 水下青石换成**一根铁条**（`utility.ink`：比本族最深档还黑，水下的东西读得出轮廓、
        读不出颜色，与寒泉洞的水下石同一判法）；
      * 槽底不画斑块而画**沉渣**：deep 档的扁片，比主体深一档，是淬火掉进去的氧化皮。
    """
    canvas.rect(0, 0, TILE - 1, TILE - 1, pal["dark"])
    _speckles(canvas, rand, [pal["deep"], pal["mid"]], 70)
    for _ in range(3):  # 沉渣：扁片贴在槽底
        x, y = 2 + rand(TILE - 8), 3 + rand(TILE - 8)
        canvas.rect(x, y, x + 4 + rand(3), y + 1, pal["deep"])
    x0, y0 = 3 + rand(TILE - 14), 6 + rand(TILE - 14)
    lean = rand(3) - 1
    for step in range(9 + rand(6)):  # 水下一根铁条：只有轮廓，边缘比本体还深一档
        x = min(TILE - 1, x0 + step)
        y = y0 + (step * lean) // 3
        if 0 <= y < TILE:
            canvas.set(x, y, ctx["utility"]["ink"])
            canvas.set(x, min(TILE - 1, y + 1), pal["deep"])
    band = 8 + rand(14)
    for x in range(TILE):  # 受光带：一线 mid，其上 1px light，带界随列号漂
        y = band + ((x // 6) % 3) - 1
        canvas.set(x, y, pal["mid"])
        if rand(3):
            canvas.set(x, y - 1, pal["light"])


def paint_forge_wall_edges(canvas, seed, mask, pal, ctx):
    """室内砖墙的三段式：砖檐压顶 + 檐下木梁（上）/ 青石基与地面相接（下）/ 砖端面（左右）。

    结构照 docs/53 §3.3，材料全换：山庄墙顶是草帽沿、村庄是黛瓦压顶、演武场是官式瓦当、
    寒泉洞什么都没有——**室内本来就没有檐口外的天空**，所以第一段收的是屋子里侧的檐：
    砖立砌压顶（顶行 `light`，因为受光的是朝上那张脸）→ 檐下那根木梁（`utility.wood`）→
    梁底一条墨影（梁把墙面压暗，那是这张图上唯一一处"人造的暗"）。下沿反过来：墙脚与
    地面相接的那圈是青石基（`ctx["paving"]`），不是土也不是草。
    """
    exposed = tk.mask_edges(mask)
    wood = ctx["utility"]["wood"]
    paving = ctx["paving"]
    if "top" in exposed:
        for x in range(TILE):
            depth = 6 + (seed + x) % 3
            canvas.rect(x, 0, x, 1, pal["light"] if (seed + x) % 3 else pal["mid"])
            canvas.set(x, 2, pal["deep"])
            for d in range(3, depth - 1):
                canvas.set(x, d, wood)
            canvas.set(x, depth - 1, ctx["utility"]["ink"])  # 梁底的墨影
            if (seed + x * 5) % 7 == 0:
                canvas.set(x, depth, ctx["utility"]["gold"])  # 梁头的钉帽
    if "bottom" in exposed:
        for x in range(TILE):
            depth = 4 + (seed + x * 3) % 3
            for d in range(depth):
                canvas.set(x, TILE - 1 - d,
                           ctx["utility"]["ink"] if d == 0 else
                           (paving["deep"] if d == 1 else paving["mid"]))
            if (seed + x * 5) % 8 == 0:
                canvas.set(x, TILE - 1 - depth, paving["light"])
    for edge in ("left", "right"):
        if edge in exposed:
            coord = EDGE_COORD[edge]
            for t in range(TILE):
                canvas.set(*coord(t, 0), pal["deep"])
                canvas.set(*coord(t, 1), pal["dark"])
                if (seed + t * 11) % 7:
                    canvas.set(*coord(t, 2), ctx["utility"]["ink"])


def paint_forge_hearth_edges(canvas, seed, mask, pal, ctx):
    """耐火砖地的收口：外圈是溅出去的炭渣，贴着砖的那圈是烧裂的壳，零星一颗砖屑亮档。"""
    for bit in range(4):
        edge = tk.EDGE_BY_BIT[bit]
        if edge not in tk.mask_edges(mask):
            continue
        rand = hash_stream(seed + bit * 131 + 9)
        coord = EDGE_COORD[edge]
        for t, depth in enumerate(edge_profile(rand)):
            span = max(depth, 2)
            for d in range(span):
                canvas.set(*coord(t, d),
                           ctx["soil"]["deep"] if d < 2 else pal["deep"])
            if rand(4) == 0:
                canvas.set(*coord(t, span), pal["light"])  # 崩出来的砖屑，受光的只有这一颗
            if rand(7) == 0:
                canvas.set(*coord(t, 1), ctx["utility"]["ink"])


def paint_forge_trough_edges(canvas, seed, mask, pal, ctx):
    """槽沿（docs/53 §3.7 的"先画沿再画水"，换材料）：沿是青石，不是土岸。

    水侧不给 `light` 浪花：槽沿内侧那圈是**水渍**（deep 档压边），反光只留在槽心那条带里
    （见 `paint_forge_trough`）。沿的外列最深、内列给石板族中档，读得出是一条凿出来的方槽。
    """
    for bit in range(4):
        edge = tk.EDGE_BY_BIT[bit]
        if edge not in tk.mask_edges(mask):
            continue
        rand = hash_stream(seed + bit * 131 + 7)
        coord = EDGE_COORD[edge]
        for t in range(TILE):
            rim = 2 + rand(3)
            for d in range(rim):  # 石沿：外列最深，内列抬两档
                canvas.set(*coord(t, d),
                           ctx["paving"]["deep"] if d == 0 else ctx["paving"]["mid"])
            canvas.set(*coord(t, rim), pal["deep"])  # 水渍
            if rand(5) == 0:
                canvas.set(*coord(t, rim), ctx["utility"]["paper"])  # 沿口的白霜（水碱）
            for d in range(rim + 1, rim + 4):
                canvas.set(*coord(t, d), pal["deep"])
            if rand(6) == 0:
                canvas.set(*coord(t, rim + 2), pal["mid"])


# ---------------------------------------------------------------------------
# 配方表与单瓦装配
# ---------------------------------------------------------------------------
#
# 从前这里是一串 `if name == "grass" ... elif`，也就是"山庄那六种材质"硬编码在派发里。
# M72-11 要把同一套机制铺到别的场景，改法是**把派发变成一张表**：kind 名 → 画法，
# 每个场景一份，缺的名字走默认段（village 的青瓦就故意复用山庄的瓦垄，只换色阶）。
# 六个 kind 名本身不许换（`terrain_kit.load_ramp` 硬校验），因为引擎与 farm.lua/building.lua
# 认的是 kind 编号，编号语义靠名字分派——改名等于悄悄换语义。
#
# 签名两个：本体 `(canvas, rand, pal, ctx)`，边收口 `(canvas, seed, mask, pal, ctx)`。
# `ctx` 带 `families` / `utility` / `soil` / `paving` 四项：后两项是**从本场景的色阶表
# 解出来的族调色板**（dirt kind 用哪族、stone kind 用哪族），所以"水陆交界的土岸"
# 在山庄读 dirt、在村庄读 tea_soil，不靠 if 场景名。


def paint_base(canvas, rand, name, pal, ctx, base_recipes):
    paint = base_recipes.get(name)
    if paint is None:
        raise KeyError(f"没有这种地表 kind：{name!r}")
    paint(canvas, rand, pal, ctx)


def paint_edges(canvas, seed, name, mask, pal, ctx, edge_recipes):
    """逐边收口：位为 0（该方向没有同类邻居，或是地图边）才画，四个方向互不干涉。"""
    paint = edge_recipes.get(name)
    if paint is None:
        raise KeyError(f"没有这种地表 kind 的边收口：{name!r}")
    paint(canvas, seed, mask, pal, ctx)


def _base_grass(canvas, rand, pal, ctx):
    paint_grass(canvas, rand, pal)


def _base_dirt(canvas, rand, pal, ctx):
    paint_dirt(canvas, rand, pal, ctx["paving"], ctx["utility"]["wood"])


def _base_stone(canvas, rand, pal, ctx):
    paint_masonry(canvas, rand, pal, 6)


def _base_wall(canvas, rand, pal, ctx):
    paint_masonry(canvas, rand, pal, 4)
    canvas.hline(0, TILE - 1, 0, pal["dark"])


def _base_roof(canvas, rand, pal, ctx):
    paint_roof(canvas, rand, pal)


def _base_water(canvas, rand, pal, ctx):
    paint_water(canvas, rand, pal)


def _edge_soil(canvas, seed, mask, pal, ctx):
    for bit in range(4):
        edge = tk.EDGE_BY_BIT[bit]
        if edge in tk.mask_edges(mask):
            paint_soil_edge(canvas, hash_stream(seed + bit * 131), edge, pal)


def _edge_stone(canvas, seed, mask, pal, ctx):
    for bit in range(4):
        edge = tk.EDGE_BY_BIT[bit]
        if edge in tk.mask_edges(mask):
            paint_paving_edge(canvas, hash_stream(seed + bit * 131 + 5), edge, pal,
                              ctx["soil"]["dark"])


def _edge_water(canvas, seed, mask, pal, ctx):
    for bit in range(4):
        edge = tk.EDGE_BY_BIT[bit]
        if edge in tk.mask_edges(mask):
            paint_shore_edge(canvas, hash_stream(seed + bit * 131 + 7), edge,
                             pal, ctx["soil"], ctx["paving"])


def _edge_wall_masonry(canvas, seed, mask, pal, ctx):
    """山庄院墙：草帽沿（草族）+ 底部暗基线 + 端面墨断点。"""
    exposed = tk.mask_edges(mask)
    if "top" in exposed:
        paint_wall_cap(canvas, hash_stream(seed + 11), ctx["families"]["grass"],
                       ctx["families"]["hedge"])
    if "bottom" in exposed:
        paint_wall_base(canvas, hash_stream(seed + 17), pal, ctx["utility"]["ink"])
    for bit, edge in ((3, "left"), (1, "right")):
        if edge in exposed:
            paint_wall_end(canvas, hash_stream(seed + bit * 131), edge, pal,
                           ctx["utility"]["ink"])


def _edge_roof(canvas, seed, mask, pal, ctx):
    exposed = tk.mask_edges(mask)
    if "bottom" in exposed:
        paint_roof_eave(canvas, hash_stream(seed + 19), pal, ctx["utility"]["ink"])
    if "top" in exposed:
        paint_roof_ridge(canvas, hash_stream(seed + 23), pal)
    for bit, edge in ((3, "left"), (1, "right")):
        if edge in exposed:
            paint_roof_gable(canvas, hash_stream(seed + bit * 131), edge, pal,
                             ctx["utility"]["ink"])


MANOR_BASE = {"grass": _base_grass, "dirt": _base_dirt, "stone": _base_stone,
              "wall": _base_wall, "roof": _base_roof, "water": _base_water}

MANOR_EDGES = {"grass": _edge_soil, "dirt": _edge_soil, "stone": _edge_stone,
               "wall": _edge_wall_masonry, "roof": _edge_roof, "water": _edge_water}

VILLAGE_BASE = {"grass": paint_village_grass, "dirt": paint_village_soil,
                "stone": paint_village_flagstone, "wall": paint_village_wall,
                "roof": _base_roof, "water": paint_village_stream}

VILLAGE_EDGES = {"grass": _edge_soil, "dirt": _edge_soil, "stone": _edge_stone,
                 "wall": paint_village_wall_edges, "roof": _edge_roof,
                 "water": _edge_water}

# 山道的 kind 5 走自家画法（竹林地是地面，不是瓦垄），所以这里不复用 `_base_roof`；
# 边收口反过来复用 `_edge_soil`——落叶层与土路的沿读起来是同一件事：软面压住硬面。
MOUNTAIN_BASE = {"grass": paint_mountain_grass, "dirt": paint_mountain_scree,
                 "stone": paint_mountain_granite, "wall": paint_mountain_cliff,
                 "roof": paint_mountain_bamboo, "water": paint_mountain_stream}

MOUNTAIN_EDGES = {"grass": _edge_soil, "dirt": _edge_soil, "stone": _edge_stone,
                  "wall": paint_mountain_cliff_edges, "roof": _edge_soil,
                  "water": _edge_water}

# 演武场的 5 号（顶层叫 roof 的那列）与山庄/村庄一样只是占位，所以本体复用 `_base_roof`、
# 只把色阶换成 `blue_tile`；水反过来走自家画法（园池要画浮萍与水下青石）。
ARENA_BASE = {"grass": paint_arena_turf, "dirt": paint_arena_loam,
              "stone": paint_arena_flagstone, "wall": paint_arena_brick,
              "roof": _base_roof, "water": paint_arena_pool}

ARENA_EDGES = {"grass": _edge_soil, "dirt": _edge_soil, "stone": _edge_stone,
               "wall": paint_arena_brick_edges, "roof": _edge_roof,
               "water": _edge_water}

# 洞穴的 1/2/3 号（洞底石、碎石堆、凿平甬道）三种地面的边收口**全部复用通用件**：软面压硬面
# 这件事在洞里同样是"碎屑压在暴露边上"，没有第二种读法。5 号（冰晶地）像山道的竹林地那样
# 真铺在地表，所以本体与边都走自家画法；4/6 号（洞壁、暗河）也一样——这两样在户外三图里
# 不存在对应的东西，通用件里没有它们的份。
CAVE_BASE = {"grass": paint_cave_floor, "dirt": paint_cave_scree,
             "stone": paint_cave_hewn, "wall": paint_cave_wall,
             "roof": paint_cave_ice, "water": paint_cave_darkwater}

CAVE_EDGES = {"grass": _edge_soil, "dirt": _edge_soil, "stone": _edge_stone,
              "wall": paint_cave_wall_edges, "roof": paint_cave_ice_edges,
              "water": paint_cave_shore_edges}

# 铁匠铺的 1/2/3 号（夯土、炭渣、石板）的边收口**全部复用通用件**：软面压硬面这件事在屋里
# 同样是"碎屑压在暴露边上"。4/5/6 号（砖墙、耐火砖地、淬火水槽）走自家画法——墙顶在室内
# 收的是屋子里侧的檐、耐火砖的边是烧裂的壳、水槽的沿是凿出来的石沿，通用件里没有这三样的份。
FORGE_BASE = {"grass": paint_forge_earth, "dirt": paint_forge_cinder,
              "stone": paint_forge_flagstone, "wall": paint_forge_brick,
              "roof": paint_forge_hearth, "water": paint_forge_trough}

FORGE_EDGES = {"grass": _edge_soil, "dirt": _edge_soil, "stone": _edge_stone,
               "wall": paint_forge_wall_edges, "roof": paint_forge_hearth_edges,
               "water": paint_forge_trough_edges}

SCENE_RECIPES = {
    "manor": (MANOR_BASE, MANOR_EDGES),
    "village": (VILLAGE_BASE, VILLAGE_EDGES),
    "mountain_path": (MOUNTAIN_BASE, MOUNTAIN_EDGES),
    "combat_arena": (ARENA_BASE, ARENA_EDGES),
    "cold_spring_cave": (CAVE_BASE, CAVE_EDGES),
    "interior_forge": (FORGE_BASE, FORGE_EDGES),
}


def scene_ctx(scene, table):
    """一次备好取色入口：每族四档 + 通用色 + 从本场景 kinds 解出的 soil/paving 两族。"""
    if scene not in SCENE_RECIPES:
        raise KeyError(f"没有场景 {scene!r} 的地表配方，已登记：{sorted(SCENE_RECIPES)}")
    families, utility = palettes(table)
    by_name = {kind["name"]: kind["family"] for kind in table["kinds"]}
    ctx = {"table": table, "families": families, "utility": utility,
           "soil": families[by_name["dirt"]], "paving": families[by_name["stone"]]}
    return ctx, SCENE_RECIPES[scene]


def build_tile(kind, mask, ctx, recipes):
    kind_id = int(kind["id"])
    base_recipe, edge_recipe = recipes
    canvas = Canvas(TILE, TILE)
    # 基础层与过渡层用两条独立的流：改掩码只影响边上的收口，不会把整张瓦的碎点重排
    # （重排会让 16 格看起来像噪声，读不出"这条边和那条边"的差别）。
    paint_base(canvas, hash_stream(1000 * kind_id + 1), kind["name"],
               ctx["families"][kind["family"]], ctx, base_recipe)
    paint_edges(canvas, 1000 * kind_id + 17 * mask + 3, kind["name"], mask,
                ctx["families"][kind["family"]], ctx, edge_recipe)
    return canvas


def build_sheet(table, scene=None):
    scene = scene or table["scene"]
    ctx, recipes = scene_ctx(scene, table)
    sheet = tk.AtlasSheet(table)
    for kind in table["kinds"]:
        kind_id = int(kind["id"])
        interior = None
        for mask in range(tk.AUTOTILE_SLOTS):
            tile = build_tile(kind, mask, ctx, recipes)
            if mask == tk.AUTOTILE_SLOTS - 1:
                interior = tile
            sheet.paint(tk.entry_transition(table, kind_id, mask), tile)
        # 满块档 = 四邻皆同类，与过渡块的 mask 15 逐像素相同：这样"关掉 autotile 立刻回滚"
        # 拿到的就是真美术，而不是空白或另一种构图。
        sheet.paint(tk.entry_base(table, kind_id), interior)
    return sheet


# ---------------------------------------------------------------------------
# 预览（开发产物，`_` 前缀按 docs/48 不进发行包）
# ---------------------------------------------------------------------------


def build_preview(table, sheet_canvas):
    label_w = text_width("grass mask 0..15", 1) + 10
    width = label_w + tk.AUTOTILE_SLOTS * TILE + 8
    height = 24 + (len(table["kinds"]) + 1) * (TILE + 14) + sheet_canvas.h + 14
    cv = Canvas(width, height)
    cv.rect(0, 0, width - 1, height - 1, tk.rgb("#14171A"))
    draw_text(cv, 4, 6, "%s GROUND ATLAS %dx%d  32PX"
              % (table["scene"].upper(), table["cols"], table["rows"]),
              tk.rgb("#C9A05A"), 1)
    y = 24
    for kind in table["kinds"]:
        kind_id = int(kind["id"])
        draw_text(cv, 4, y + TILE // 2 - 4, "%-6s mask 0..15" % kind["name"].upper(),
                  tk.rgb("#A9C28F"), 1)
        for mask in range(tk.AUTOTILE_SLOTS):
            cv.blit(tk.entry_region(sheet_canvas, table, tk.entry_transition(table, kind_id, mask)),
                    label_w + mask * TILE, y)
        y += TILE + 14
    draw_text(cv, 4, y + 4, "FULL SHEET", tk.rgb("#A9C28F"), 1)
    cv.blit(sheet_canvas, label_w, y)
    for col in range(table["cols"] + 1):
        for py in range(sheet_canvas.h):
            cv.set(label_w + col * TILE, y + py, (255, 255, 255, 28))
    for row in range(table["rows"] + 1):
        for px in range(sheet_canvas.w):
            cv.set(label_w + px, y + row * TILE, (255, 255, 255, 28))
    return cv


def write_png(path, canvas):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(canvas.to_png())


# ---------------------------------------------------------------------------
# 校验：盘上 PNG 对色阶表与条目排布
# ---------------------------------------------------------------------------


def edge_region_diff(a, b, edge, depth=4):
    """过渡块与该 kind 满块档在"该边外侧 depth 像素"条带内的不同像素数。"""
    coord = EDGE_COORD[edge]
    return sum(1 for t in range(TILE) for d in range(depth)
               if a.get(*coord(t, d)) != b.get(*coord(t, d)))


def check(scene="manor", path=None, table=None):
    """返回问题列表（空 = 通过）；检查项与 M72-04 的验收条目一一对应。"""
    table = table if table is not None else tk.load_ramp(scene)
    path = path if path is not None else ground_path(scene)
    issues = []
    needed, capacity = tk.capacity_check(table)
    if needed > capacity:
        issues.append(f"图集容量不足：needed {needed} > cols*rows {capacity}")
    if not os.path.exists(path):
        return issues + ["missing " + path]

    canvas = load_canvas(path)
    want = (table["cols"] * TILE, table["rows"] * TILE)
    if (canvas.w, canvas.h) != want:
        return issues + [f"{path}: {canvas.w}x{canvas.h} != {want[0]}x{want[1]}"
                         "（引擎按 atlas_cols*32 / atlas_rows*32 切图，对不上就是错位）"]

    allowed = tk.allowed_colors(table)
    offenders = 0
    soft_alpha = 0
    for y in range(canvas.h):
        for x in range(canvas.w):
            color = canvas.get(x, y)
            if color[3] not in (0, 255):
                soft_alpha += 1
                continue
            if color[3] == 255 and color[:3] not in allowed:
                offenders += 1
    if offenders:
        issues.append(f"{path}: {offenders} 个不透明像素不在色阶表内（docs/57 §1.2）")
    if soft_alpha:
        issues.append(f"{path}: {soft_alpha} 个像素 alpha 非 0/255（RGBA8 硬指标）")

    for family in table["uses"]:
        monotonic, levels = tk.ramp_is_monotonic(table, family)
        if not monotonic:
            issues.append(f"色阶表 {family}: {len(levels)} 档不是按亮度单调递增排的")

    used = {}
    for kind in table["kinds"]:
        kind_id = int(kind["id"])
        base = tk.entry_region(canvas, table, tk.entry_base(table, kind_id))
        if not any(base.get(x, y)[3] for y in range(TILE) for x in range(TILE)):
            issues.append(f"kind {kind['name']}: 满块档 entry {tk.entry_base(table, kind_id)} 是空的"
                          "（关掉 autotile 会画不出东西）")
        seen = {}
        for mask in range(tk.AUTOTILE_SLOTS):
            patch = tk.entry_region(canvas, table, tk.entry_transition(table, kind_id, mask))
            key = bytes(patch.px)
            if key in seen:
                issues.append(f"kind {kind['name']} mask {mask:02d} 与 mask {seen[key]:02d} "
                              "逐像素相同（16 格必须互不相同）")
            else:
                seen[mask] = key
            for bit in range(4):
                edge = tk.EDGE_BY_BIT[bit]
                if edge in tk.mask_edges(mask):
                    diff = edge_region_diff(patch, base, edge)
                    if diff < 8:
                        issues.append(f"kind {kind['name']} mask {mask:02d}: {edge} 边的收口只改了 "
                                      f"{diff} 个像素（这条边等于没画）")
            for level in tk.observed_levels(patch, tk.family_levels(table, kind["family"])):
                used.setdefault(kind["family"], set()).add(level)
    for family, levels in used.items():
        if len(levels) < 3:
            issues.append(f"材质族 {family}: 只用上 {len(levels)} 档色阶（M72-04 要求 >=3）")

    for entry in range(1 + table["kind_count"] * (1 + tk.AUTOTILE_SLOTS),
                       table["cols"] * table["rows"] + 1):
        patch = tk.entry_region(canvas, table, entry)
        if any(patch.get(x, y)[3] for y in range(TILE) for x in range(TILE)):
            issues.append(f"尾部空槽 entry {entry} 被画上了东西（超出 kinds*(1+16) 必须留全透明）")
    return issues


# ---------------------------------------------------------------------------
# 入口
# ---------------------------------------------------------------------------


def generate(scene="manor", quiet=False):
    table = tk.load_ramp(scene)
    path = ground_path(scene)
    sheet = build_sheet(table, scene)
    offenders = sheet.quantize()
    if offenders:
        raise SystemExit(f"[FAIL] 生成器画出 {offenders} 个表外颜色——hex 只能来自 terrain_ramp.json")
    write_png(path, sheet.canvas)
    write_png(ground_preview_path(scene), build_preview(table, sheet.canvas))
    problems = check(scene, path, table)
    if not quiet:
        print(f"tileset  {path} {sheet.canvas.w}x{sheet.canvas.h} "
              f"({table['cols']}列 × {table['rows']}行，{len(table['kinds'])} kinds × 17 档)")
        print(f"preview  {ground_preview_path(scene)}")
    if problems:
        for problem in problems:
            print("[FAIL]", problem)
        raise SystemExit(1)


def print_layout(scene="manor"):
    table = tk.load_ramp(scene)
    needed, capacity = tk.capacity_check(table)
    print(f"[{scene}] atlas {table['cols']}x{table['rows']} = {capacity} 条，"
          f"needed {needed}，空槽 {capacity - needed}")
    for kind in table["kinds"]:
        kind_id = int(kind["id"])
        base = tk.entry_base(table, kind_id)
        first = tk.entry_transition(table, kind_id, 0)
        last = tk.entry_transition(table, kind_id, 15)
        col, row = tk.entry_rect(table, base)
        print(f"  kind {kind_id} {kind['name']:<6} family={kind['family']:<10} "
              f"满块 {base} (col{col},row{row}) 过渡 {first}-{last}（mask 0..15）")
    print("  mask 位序：bit0=y-1(下沿) bit1=x+1(右沿) bit2=y+1(上沿) bit3=x-1(左沿)，按 index 命名不按方位")


def main():
    parser = argparse.ArgumentParser(description="《江湖有田》地表图集生成（M72-04 建 / M72-11 铺量）")
    parser.add_argument("--scene", default=tk.DEFAULT_SCENE, choices=tk.scenes(),
                        help=f"取哪一个场景档（色阶表 scenes 段里登记的），默认 {tk.DEFAULT_SCENE}")
    parser.add_argument("--check", action="store_true", help="只校验盘上 PNG，不重画")
    parser.add_argument("--layout", action="store_true", help="打印条目表")
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()
    if args.layout:
        print_layout(args.scene)
        return
    if args.check:
        problems = check(args.scene)
        if problems:
            for problem in problems:
                print("[FAIL]", problem)
            raise SystemExit(1)
        print(f"[OK] {args.scene} 地表图集校验通过（色阶 / alpha / 16 过渡块 / 四边收口 / 空槽）")
        return
    generate(args.scene, quiet=args.quiet)


if __name__ == "__main__":
    main()
