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

    entry = 1 + (kind-1)                        # 满块档：关 autotile 的恒等寻址 + arena 复用的那 6 格
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

SCENE_RECIPES = {
    "manor": (MANOR_BASE, MANOR_EDGES),
    "village": (VILLAGE_BASE, VILLAGE_EDGES),
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
        # 和 arena 的恒等寻址拿到的都是真美术，而不是空白或另一种构图。
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
