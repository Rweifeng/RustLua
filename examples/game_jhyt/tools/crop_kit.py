# -*- coding: utf-8 -*-
"""作物的**形态契约层**（M72-08）：一张 CATALOG 同时生出图、图集与运行期寻址表。

为什么要有这个文件：29 种作物以前共用 `manor_farm.png` 的 SPROUT/GROWING/RIPE 三列，
"灵芝和稻谷长得一模一样"。拆开时最省事的写法是给每种作物换一个 hue——那种图在
"两两不重复"的哈希门面前必过（颜色不同就算不同），肉眼却仍是一张图。所以这里把
"长得不一样"写成可核对的参数轴：archetype（株型骨架）、height、stems、leaf、head、
canopy、crown、spacing、accent，`check()` 既断言形态签名两两不同（**不看颜色**），
也断言全部阶段图两两字节哈希不同。

三处同源，缺一个就分叉：

  * `gen_crop_art.py`：出 29×N 张独立 `crops/<id>/stage_N.png`（docs/58 台账契约）
    与一张 8×29 的打包图集 `crops/manor_crops.png`（CropGrid 用）。
  * `scripts/data/crop_art.lua`：运行期寻址（crop → 图集行号），`systems/farm.lua` 只认它。
  * `scenes/manor.scene` 的 `CropGrid`：`atlas_cols=8;atlas_rows=29`。

条目算术与引擎 `Tilemap::atlas_uv` 同构（1 基、row-major）：
`entry = 1 + (row-1)*COLS + slot`；slot 0..4 = 阶段 1..5，slot 5..7 = 收成抖动三帧。
"""

import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import terrain_kit as tk  # noqa: E402
from pixel_kit import Canvas, hash_stream, rgb  # noqa: E402

TILE = 32
# 一行一种作物：5 个生长阶段 + 3 帧收成抖动 = 8 槽；29 行 = 232 条 < 255（tile 值是 u8）。
COLS = 8
ROWS = 29
SHAKE_SLOTS = 3
STAGE_SLOTS = COLS - SHAKE_SLOTS

ATLAS_TEXTURE = "assets/art/crops/manor_crops.png"
CROPS_DIR = "assets/art/crops"
GROUND = 29  # 32px 格里的地表行；最底两行留给接触影

RAMP = tk.load_ramp()
CROP_TABLE = RAMP["crops"]

FIELD = 4  # 田粮：4 档
MEDICINE = 5  # 灵药：5 档
TREE = 3  # 果树：3 档（tree_days 的苗 / 幼树 / 成树）

# ---------------------------------------------------------------------------
# CATALOG：29 种 × 形态参数（键顺序必须与 data/crops.lua 一致，见 validate_order）
# ---------------------------------------------------------------------------

CATALOG = {
    # 键顺序 = `data/crops.lua` 的顶层键顺序（`validate_order` 逐条对拍）：
    # 图集行号就是这个顺序，改了行号整套图会静默错位。
    "lingzhi": {
        "name": "灵芝", "stages": MEDICINE, "archetype": "fungus", "produce": "medicine_red",
        "height": 12, "stems": 2, "cap_r": 8, "accent": 0, "spacing": 7,
    },
    "rice": {
        "name": "稻谷", "stages": FIELD, "archetype": "grain", "produce": "grain_amber",
        "height": 24, "stems": 5, "head": "panicle", "accent": 7, "lean": 1, "spacing": 3,
    },
    "radish": {
        "name": "萝卜", "stages": FIELD, "archetype": "root", "produce": "root_red",
        "height": 16, "stems": 4, "leaf": 5, "accent": 0, "spacing": 5, "crown": "swollen",
    },
    "herb": {
        "name": "草药", "stages": MEDICINE, "archetype": "herb", "produce": "medicine_pale",
        "height": 15, "stems": 5, "accent": 0, "bud": 3, "spacing": 4,
    },
    "polygonum": {
        "name": "何首乌", "stages": MEDICINE, "archetype": "root", "produce": "medicine_red",
        "height": 18, "stems": 3, "leaf": 4, "accent": 0, "spacing": 5, "crown": "twisted",
    },
    "ginseng": {
        "name": "人参", "stages": MEDICINE, "archetype": "root", "produce": "medicine_pale",
        "height": 19, "stems": 3, "leaf": 5, "accent": 3, "spacing": 6, "crown": "whorl",
        "umbel": True,
    },
    "mint": {
        "name": "薄荷", "stages": MEDICINE, "archetype": "herb", "produce": "leaf_cool",
        "height": 16, "stems": 6, "accent": 0, "bud": 4, "spacing": 3, "serrated": True,
    },
    "mulberry": {
        "name": "桑葚", "stages": FIELD, "archetype": "trellis", "produce": "nightshade_violet",
        "height": 21, "stems": 2, "head": "drop", "accent": 4, "spacing": 6, "leaf": 5,
        "serrated": True,
    },
    "pea": {
        "name": "青豌豆", "stages": FIELD, "archetype": "trellis", "produce": "legume_green",
        "height": 22, "stems": 3, "head": "pod", "accent": 4, "lean": -1, "spacing": 6,
        "tendril": True,
    },
    "cucumber": {
        "name": "青瓜", "stages": FIELD, "archetype": "vine", "produce": "cucumber_jade",
        "height": 14, "stems": 3, "leaf": 6, "head": "gourd", "accent": 2, "spacing": 7,
    },
    "mung_bean": {
        "name": "绿豆", "stages": FIELD, "archetype": "bush", "produce": "legume_green",
        "height": 18, "stems": 6, "head": "pod", "accent": 5, "spacing": 4, "pod_axis": "up",
    },
    "prunella": {
        "name": "夏枯草", "stages": MEDICINE, "archetype": "spike", "produce": "nightshade_violet",
        "height": 23, "stems": 4, "head": "whorl", "accent": 5, "spacing": 4, "leaf": 3,
    },
    "eggplant": {
        "name": "茄子", "stages": FIELD, "archetype": "bush", "produce": "nightshade_violet",
        "height": 20, "stems": 5, "head": "drop", "accent": 3, "spacing": 5, "leaf": 6,
    },
    "pumpkin": {
        "name": "南瓜", "stages": FIELD, "archetype": "vine", "produce": "gourd_gold",
        "height": 12, "stems": 2, "leaf": 5, "head": "gourd", "accent": 1, "spacing": 8,
        "gourd_r": 7,
    },
    "gouqi": {
        "name": "枸杞", "stages": MEDICINE, "archetype": "bush", "produce": "berry_crimson",
        "height": 20, "stems": 6, "head": "berry", "accent": 6, "spacing": 5, "leaf": 5,
    },
    "cabbage": {
        "name": "白菜", "stages": FIELD, "archetype": "rosette", "produce": "brassica_pale",
        "height": 13, "leaf": 12, "accent": 0, "spacing": 4, "closed": True,
    },
    "chrysanthemum": {
        "name": "菊花", "stages": MEDICINE, "archetype": "spike", "produce": "blossom_warm",
        "height": 20, "stems": 3, "head": "disc", "accent": 1, "spacing": 6, "leaf": 5,
        "petals": 12,
    },
    "leek": {
        "name": "韭葱", "stages": FIELD, "archetype": "allium", "produce": "allium_white",
        "height": 22, "stems": 7, "accent": 0, "spacing": 3, "fan": 9,
    },
    "winter_wheat": {
        "name": "冬麦", "stages": FIELD, "archetype": "grain", "produce": "grain_amber",
        "height": 26, "stems": 4, "head": "spike", "accent": 6, "lean": 0, "spacing": 4,
        "awn": True,
    },
    "xueli": {
        "name": "雪里蕻", "stages": FIELD, "archetype": "rosette", "produce": "brassica_pale",
        "height": 17, "leaf": 9, "accent": 0, "spacing": 5, "curl": True, "closed": False,
    },
    "winter_shen": {
        "name": "冬参", "stages": MEDICINE, "archetype": "root", "produce": "leaf_cool",
        "height": 14, "stems": 2, "leaf": 7, "accent": 0, "spacing": 4, "crown": "whorl",
        "frosted": True,
    },
    "plum": {
        "name": "腊梅", "stages": MEDICINE, "archetype": "spike", "produce": "blossom_warm",
        "height": 21, "stems": 4, "head": "branch_blossom", "accent": 4, "spacing": 5,
        "leaf": 3, "woody": True,
    },
    "winter_cabbage": {
        "name": "冬白菜", "stages": FIELD, "archetype": "rosette", "produce": "brassica_pale",
        "height": 15, "leaf": 8, "accent": 0, "spacing": 6, "closed": False, "outer_dark": True,
    },
    "evergreen": {
        "name": "冬青", "stages": MEDICINE, "archetype": "bush", "produce": "root_red",
        "height": 22, "stems": 7, "head": "berry", "accent": 5, "spacing": 4, "needle": True,
    },
    "peach": {
        "name": "桃树", "stages": TREE, "archetype": "tree", "produce": "tree_peach",
        "height": 30, "canopy": "round", "accent": 5, "spacing": 6,
    },
    "mume": {
        "name": "青梅树", "stages": TREE, "archetype": "tree", "produce": "tree_mume",
        "height": 28, "canopy": "open", "accent": 4, "spacing": 7,
    },
    "loquat": {
        "name": "枇杷树", "stages": TREE, "archetype": "tree", "produce": "gourd_gold",
        "height": 31, "canopy": "tall", "accent": 3, "spacing": 5,
    },
    "jujube": {
        "name": "枣树", "stages": TREE, "archetype": "tree", "produce": "berry_crimson",
        "height": 27, "canopy": "gnarl", "accent": 6, "spacing": 4,
    },
    "ginkgo": {
        "name": "银杏", "stages": TREE, "archetype": "tree", "produce": "tree_ginkgo",
        "height": 31, "canopy": "fan", "accent": 4, "spacing": 6,
    },
}

ORDER = list(CATALOG)

# 参与"形态签名"的轴：故意不含 produce，否则"同型换个色"也能通过断言。
# `serrated` / `tendril` / `pod_axis` / `awn` / `bud` 都是真的画法分叉（见各 arch_* 里读它们的
# 那一行），列进来才拦得住"两条同株型只靠 flag 区分"的作物被签名当成同一个。
SIGNATURE_KEYS = ("archetype", "stages", "height", "stems", "leaf", "head",
                  "canopy", "crown", "spacing", "accent",
                  "serrated", "tendril", "pod_axis", "awn", "bud",
                  "closed", "curl", "outer_dark", "needle", "umbel", "frosted")


def crop_family_hexes(name):
    levels = CROP_TABLE["families"].get(name)
    if not levels:
        raise KeyError(f"crops.families 里没有作物身份色族 {name!r}")
    return list(levels)


def crop_allowed_colors():
    """作物图集允许出现的不透明色：作物身份色族 + 地表全族 + 通用墨/纸/金/木。

    地表族是**故意放开**的：叶子与土本来就该和草地同色，作物田才像同一个世界画出来的。
    """
    return tk.allowed_colors(RAMP) | {
        rgb(hex_value)[:3] for levels in CROP_TABLE["families"].values() for hex_value in levels
    }


def validate_order(crops_lua_path):
    """CATALOG 顺序必须逐条等于 `data/crops.lua` 的顶层键顺序。

    行号写进图集与 `crop_art.lua`，顺序一变整套图就错位——症状是"水稻长出灵芝"，
    而且一张都不报错，所以出图前就炸。
    """
    with open(crops_lua_path, encoding="utf-8") as handle:
        source = re.findall(r"^  ([a-z_0-9]+) = \{", handle.read(), re.M)
    if source != ORDER:
        raise SystemExit(
            "crop_kit.CATALOG 顺序与 data/crops.lua 不一致：\n"
            f"  crops.lua : {source}\n"
            f"  crop_kit  : {ORDER}"
        )
    if len(ORDER) != ROWS:
        raise SystemExit(f"作物行数 {len(ORDER)} 与图集 atlas_rows={ROWS} 不符")
    return source


def stages_of(crop_id):
    return CATALOG[crop_id]["stages"]


def row_of(crop_id):
    return ORDER.index(crop_id) + 1


def entry_for(crop_id, slot):
    """(作物, 0 基槽位) → 图集条目号（1 基）。slot 0..4 是阶段，5..7 是收成抖动帧。"""
    if not 0 <= slot < COLS:
        raise ValueError(f"槽位越界：{crop_id} slot={slot}")
    return 1 + (row_of(crop_id) - 1) * COLS + slot


def entry_for_stage(crop_id, stage):
    stages = stages_of(crop_id)
    if not 1 <= stage <= stages:
        raise ValueError(f"{crop_id} 没有阶段 {stage}（该作物 {stages} 档）")
    return entry_for(crop_id, stage - 1)


def shake_entries(crop_id):
    return [entry_for(crop_id, STAGE_SLOTS + frame) for frame in range(SHAKE_SLOTS)]


def stage_count():
    return sum(stages_of(crop_id) for crop_id in ORDER)


# ---------------------------------------------------------------------------
# 调色：叶/茎/接触影走地表族，果实走作物身份族
# ---------------------------------------------------------------------------


def palette_for(crop_id):
    produce = [rgb(hex_value) for hex_value in crop_family_hexes(CATALOG[crop_id]["produce"])]
    grass = [rgb(hex_value) for hex_value in tk.family_hexes(RAMP, "grass")]
    hedge = [rgb(hex_value) for hex_value in tk.family_hexes(RAMP, "hedge")]
    dirt = [rgb(hex_value) for hex_value in tk.family_hexes(RAMP, "dirt")]
    return {
        "produce": produce,
        "leaf": grass[1],
        "leaf_mid": grass[2],
        "leaf_light": grass[3],
        "leaf_dark": grass[0],
        "hedge": hedge[2],
        "hedge_dark": hedge[0],
        "stem": grass[2],
        "wood": rgb(RAMP["utility"]["wood"]),
        "soil_shadow": dirt[0],
        "ink": rgb(RAMP["utility"]["ink"]),
        "paper": rgb(RAMP["utility"]["paper"]),
    }


# ---------------------------------------------------------------------------
# 画法基元（PNG 第 0 行 = 贴图顶部；"光自左上"= 靠上的像素亮一档）
# ---------------------------------------------------------------------------


def curve(cv, x_base, y_from, y_to, lean, color, tip=None):
    """一根弯叶/弯茎：从 y_from 向上走到 y_to，横向按 t^2 偏移（下部直、梢部弯）。"""
    steps = max(y_from - y_to, 1)
    for i in range(steps + 1):
        t = i / steps
        y = y_from - i
        if y < 0:
            break
        cv.set(x_base + round(lean * t * t), y, tip if (i >= steps - 1 and tip) else color)


def blade_leaf(cv, x, y, dx, dy, mid, dark, width=2, serrated=False):
    """一片阔叶：以 (x,y) 为根向 (dx,dy) 展开的楔形，根部宽、梢部收细压暗。

    `serrated=True` 在叶缘再咬两个缺口（薄荷 / 桑叶的锯齿缘）：这是**形态**差异，
    不是颜色差异——同一株型只换 hue 的两种作物在去重门眼里就是同一张图。
    """
    steps = max(abs(dx), abs(dy), 1)
    for i in range(steps + 1):
        t = i / steps
        px, py = x + round(dx * t), y + round(dy * t)
        spread = max(0, width - round(1.6 * t))
        color = mid if i < steps else dark
        for offset in range(-spread, spread + 1):
            cv.set(px + offset, py, color)
        if serrated and spread > 0 and 0 < i < steps:
            cv.set(px + spread * (1 if i % 2 else -1), py, (0, 0, 0, 0))


def dot(cv, cx, cy, radius, body, highlight):
    """一颗果实/穗粒：实心小圆 + 左上高光。"""
    for y in range(cy - radius, cy + radius + 1):
        for x in range(cx - radius, cx + radius + 1):
            if abs(x - cx) + abs(y - cy) <= radius + 1:
                cv.set(x, y, body)
    cv.set(cx - max(radius - 1, 0), cy - max(radius - 1, 0), highlight)


def ellipse(cv, cx, cy, rx, ry, body, dark=None):
    for y in range(cy - ry, cy + ry + 1):
        for x in range(cx - rx, cx + rx + 1):
            nx = (x - cx) / max(rx, 1)
            ny = (y - cy) / max(ry, 1)
            if nx * nx + ny * ny <= 1.05:
                shaded = dark is not None and (y >= cy + ry - 1 or x <= cx - rx)
                cv.set(x, y, dark if shaded else body)


def contact(cv, cx, half, color):
    """株底下的接触影：一条短带，把作物'坐'在 FarmGrid 的土上而不是浮着。"""
    cv.hline(cx - half, cx + half, GROUND + 1, color)
    cv.hline(cx - half + 1, cx + half - 1, GROUND + 2, color)


def spread_of(spec):
    return spec.get("spacing", 5)


def grow(base, fraction):
    """某一轴在第 k 档长多大：半程向上取整。

    不用 Python 的 `round()`：它是银行家取整，`round(2.5) == 2`，于是第 1 档与第 2 档
    会算出同一个叶子数——症状是"两种作物有两张图字节完全一样"，去重门判红。
    """
    return max(1, int(base * fraction + 0.5))


def fruit(spec, pal, cv, top, radius=None):
    """成熟期果实：`head` 决定**形状与挂载方式**，不只是颜色。

    * `drop`：从枝顶垂下来的长椭圆果（茄子）；
    * `pod`：贴着茎竖立的豆荚（绿豆 / 豌豆），`pod_axis=up` 时更长更直；
    * `berry`：叶腋间的小圆果点（枸杞 / 冬青）；
    * 其余（`grain`/`whorl`/None）：顶部散点。

    为什么不用一颗 `dot(size=2)` 通吃：三种挂法在 32 px 上是三种剪影，共用一种就变成
    "绿杆顶着一团色块"，颜色不同而形状相同——正是去重门要判红的"换色冒充另一种"。
    `radius` 非空 = 果树树冠内的果点，按冠径铺开。
    """
    colors = pal["produce"]
    head = spec.get("head")
    count = spec.get("accent", 0)
    for index in range(count):
        rank = index // 2
        if radius is not None:
            dot(cv, 16 + round(((index % 3) - 1) * (radius + 1)),
                top + ((index // 3) - 1) * 2, 2, colors[2], colors[3])
            continue
        x = min(26, max(5, 16 + (1 if index % 2 else -1) * (2 + rank * 2)))
        if head == "pod":
            base = top + 3 + rank
            for step in range(5 if spec.get("pod_axis") == "up" else 4):
                cv.set(x, base - step, colors[1] if step else colors[3])
                cv.set(x + 1, base - step, colors[2])
            continue
        if head == "drop":
            y = top + 6 + rank * 2
            cv.vline(x, top + 1, y - 3, pal["hedge_dark"])
            ellipse(cv, x, y, 2, 3, colors[2], colors[0])
            cv.set(x - 1, y - 2, colors[3])
            continue
        if head == "berry":
            dot(cv, x, top + 3 + rank * 2, 1, colors[2], colors[3])
            continue
        dot(cv, x, top + 2 + rank * 2, 1, colors[2], colors[3])


# ---------------------------------------------------------------------------
# 株型骨架：一个 archetype 一个函数，(spec, pal, cv, fraction, ripe) → 画在 cv 上
# ---------------------------------------------------------------------------


def arch_grain(spec, pal, cv, fraction, ripe):
    """禾本科：中心分叉的窄叶 + 顶上穗头（稻谷穗下垂、冬麦穗直立带芒）。"""
    height = max(3, grow(spec["height"], fraction))
    top = GROUND - height
    stems = grow(spec["stems"], fraction)
    for index in range(stems):
        offset = (index - stems // 2) * 2
        lean = spec.get("lean", 0) + offset // 3
        curve(cv, 16 + offset, GROUND - 1, top + (1 if index % 2 else 0), lean,
              pal["stem"], pal["leaf_light"])
        if height > 8:
            curve(cv, 16 + offset, GROUND - 3, GROUND - 3 - height // 2, -lean - 2, pal["leaf"])
    if not ripe:
        return
    colors = pal["produce"]
    if spec.get("head") == "panicle":
        for side in (-1, 1):
            for index in range(spec["accent"]):
                t = index / max(spec["accent"] - 1, 1)
                dot(cv, 16 + side * round(2 + 4 * t), top + round(1 + 7 * t), 1,
                    colors[2 if index % 2 else 3], colors[3])
    else:
        for index in range(spec["accent"]):
            dot(cv, 15, top + index, 1, colors[2], colors[3])
            dot(cv, 17, top + index, 1, colors[1], colors[2])
        if spec.get("awn"):
            cv.vline(14, top - 3, top - 1, colors[3])
            cv.vline(18, top - 3, top - 1, colors[3])


def arch_bush(spec, pal, cv, fraction, ripe):
    """灌木状：多分枝短茎 + 叶簇，成熟时挂果（绿豆上举荚 / 枸杞果点 / 茄子下垂果）。"""
    height = max(3, grow(spec["height"], fraction))
    top = GROUND - height
    stems = grow(spec["stems"], fraction)
    needle = spec.get("needle")
    for index in range(stems):
        offset = ((index % 3) - 1) * spread_of(spec) // 2 + index // 3
        x = 16 + offset // 2
        lean = offset // 2
        curve(cv, x, GROUND - 1, top + index % 3, lean, pal["hedge"], pal["leaf_light"])
        for step in range(2):
            y = GROUND - 4 - step * 4 - index % 2
            if y <= top:
                break
            blade_leaf(cv, x, y, -3 - step, -2, pal["leaf"], pal["leaf_dark"], width=1 if needle else 2)
            blade_leaf(cv, x, y, 3 + step, -2, pal["leaf_mid"], pal["leaf"], width=1 if needle else 2)
    if ripe:
        fruit(spec, pal, cv, top)


def arch_herb(spec, pal, cv, fraction, ripe):
    """草本药草：直立单茎、叶在茎上**成对**（与 bush 的分枝叶簇是两条不同的轴），上部开花蕾。

    第 1 档必须画出基部那对子叶：`height` 在 1/5 档只有 3 px，若叶从 `GROUND-3` 起画会立刻
    被 `y <= top` 打断，症状是"第 1 档只剩一根茎 + 接触影"，两种草药因此在最小档撞成同一张图。
    """
    height = max(3, grow(spec["height"], fraction))
    top = GROUND - height
    stems = grow(spec["stems"], fraction)
    for index in range(stems):
        x = 16 + ((index - stems // 2) * spread_of(spec)) // 2
        cv.vline(x, top + (index % 2) * 2, GROUND - 1, pal["stem"])
        for pair in range(max(1, (height + 2) // 5)):
            y = GROUND - 1 - pair * 5 - (index % 2)
            if y < top:
                break
            width = 3 + pair
            notch = spec.get("serrated", False)
            blade_leaf(cv, x, y, -width, -2, pal["leaf"], pal["leaf_dark"], serrated=notch)
            blade_leaf(cv, x, y, width, -2, pal["leaf_mid"], pal["leaf"], serrated=notch)
    if spec.get("bud") and fraction >= 0.6:
        for index in range(spec["bud"]):
            dot(cv, 12 + index * 3, top + index % 3, 1,
                pal["produce"][2], pal["produce"][3])
    if ripe:
        fruit(spec, pal, cv, top)


def arch_rosette(spec, pal, cv, fraction, ripe):
    """十字花科叶丛：三档**轮廓**分化，不是同一把扇形换色。

    三种菜共用 `brassica_pale` 一套绿，若只调叶数与半径，成熟态在 32 px 上读起来是同一棵
    （症状就是本包要治的"灵芝和稻谷长得一模一样"）。所以把 flag 直接接在轮廓上：

    * `closed`（白菜）：矮宽的包心球，中心压出浅色菜心；
    * `curl`（雪里蕻）：窄高上举，叶尖朝天、边缘翻卷打亮；
    * 其余（冬白菜 `outer_dark`）：摊开的双层莲座，外圈长而暗。
    """
    count = max(3, grow(spec["leaf"], fraction))
    radius = max(2, grow(spec["height"] // 2, fraction))
    dark = pal["hedge_dark"] if spec.get("outer_dark") else pal["leaf_dark"]
    if spec.get("closed"):
        cy = GROUND - 3 - radius // 2
        ellipse(cv, 16, cy, radius + 2, radius, pal["leaf"], dark)
        ellipse(cv, 16, cy, max(1, radius - 2), max(1, radius // 2), pal["paper"])
        for side in (-1, 1):
            blade_leaf(cv, 16, GROUND - 2, side * (radius + 2), -radius // 2,
                       pal["leaf_mid"], dark, width=3)
    elif spec.get("curl"):
        for index in range(count):
            side = 1 if index % 2 else -1
            dx = side * round(radius * 0.4 * (index % 3))
            dy = -round(radius * (1.0 + 0.6 * ((index % 4) / 3)))
            blade_leaf(cv, 16, GROUND - 2, dx, dy,
                       pal["leaf_mid"] if index % 3 else pal["leaf"], dark, width=2)
            cv.set(16 + dx + side, GROUND - 2 + dy - 1, pal["leaf_light"])
    else:
        for index in range(count):
            side = 1 if index % 2 else -1
            dx = side * round(radius * (1.5 if index % 4 < 2 else 0.8))
            dy = -round(radius * (0.4 + 0.9 * ((index % 4) / 3)))
            blade_leaf(cv, 16, GROUND - 2, dx, dy,
                       pal["leaf_mid"] if index % 3 else pal["leaf"], dark, width=3)
    if ripe:
        fruit(spec, pal, cv, GROUND - radius)


def arch_allium(spec, pal, cv, fraction, ripe):
    """葱韭类：扁平直叶呈扇形张开 + 基部一段葱白。区分轴是 fan（别的株型都没有葱白）。"""
    height = max(3, grow(spec["height"], fraction))
    top = GROUND - height
    stems = grow(spec["stems"], fraction)
    fan = spec.get("fan", 8)
    for index in range(stems):
        lean = round((index - stems / 2) * fan / stems)
        curve(cv, 16 + round(lean * 0.3), GROUND - 1, top + abs(lean), lean,
              pal["leaf_mid"], pal["leaf_light"])
    for y in range(GROUND, GROUND - 4, -1):
        color = pal["produce"][3] if y < GROUND - 1 else pal["produce"][2]
        cv.hline(14, 18, y, color)
    if ripe:
        for index in range(spec.get("accent", 0)):
            dot(cv, 12 + index * 3, top + 2, 1, pal["produce"][0], pal["produce"][1])


def arch_vine(spec, pal, cv, fraction, ripe):
    """爬地藤：贴地蔓延的掌状大叶 + 落在叶之间的果实（株高天生矮，靠横向铺）。"""
    leaves = max(2, grow(spec["leaf"], fraction))
    for index in range(leaves):
        side = 1 if index % 2 else -1
        dx = side * round(spread_of(spec) * (1 + index // 2))
        cx = min(29, max(2, 16 + dx))
        cy = GROUND - 3 - (index // 2) * 2
        ellipse(cv, cx, cy, 4, 3, pal["leaf"], pal["leaf_dark"])
        cv.set(cx - 1, cy - 1, pal["leaf_light"])
        cv.set(cx, cy, pal["hedge_dark"])
    if not ripe:
        return
    count = spec.get("accent", 1)
    radius = spec.get("gourd_r", 3)
    for index in range(count):
        cx = 16 + (index - count // 2) * (spread_of(spec) + 2)
        colors = pal["produce"]
        ellipse(cv, cx, GROUND - radius, radius, radius - 1, colors[2], colors[0])
        for row in range(-radius + 1, radius, 2):
            cv.hline(cx - radius + 1, cx + radius - 1, GROUND - radius + row, colors[1])
        cv.set(cx - radius // 2, GROUND - 2 * radius, colors[3])


def arch_trellis(spec, pal, cv, fraction, ripe):
    """攀援类（豌豆 / 桑葚）：两根主蔓 + 卷须 + 叶序，果实成串挂蔓侧（与 bush 的区分是**蔓**）。"""
    height = max(3, grow(spec["height"], fraction))
    top = GROUND - height
    # 每根蔓的叶数走 `leaf` 轴：桑葚（5 片密叶）与豌豆（3 片 + 卷须）在营养期就该看得出差别，
    # 而不是等到果期靠颜色区分——同一 archetype 只换 hue 正是去重门要判红的形态。
    for index in range(max(1, spec["stems"] // 2 + 1)):
        x = 13 + index * 6
        curve(cv, x, GROUND - 1, top, (index - 0.5) * 2, pal["hedge"], pal["leaf_light"])
        steps = spec.get("leaf", 3)
        for step in range(steps):
            y = GROUND - 4 - step * max(2, height // steps)
            if y <= top:
                break
            side = -1 if (step + index) % 2 else 1
            blade_leaf(cv, x, y, side * 3, -1, pal["leaf"], pal["leaf_dark"],
                       serrated=spec.get("serrated", False))
    if spec.get("tendril") and fraction > 0.5:
        cv.set(20, top + 1, pal["leaf_light"])
        cv.set(21, top + 2, pal["leaf_light"])
        cv.set(21, top + 3, pal["leaf"])
    if ripe:
        fruit(spec, pal, cv, top)


def arch_spike(spec, pal, cv, fraction, ripe):
    """花茎类（夏枯草 / 菊花 / 腊梅）：直茎托起一个花序，叶只在下部几片。"""
    height = max(3, grow(spec["height"], fraction))
    top = GROUND - height
    stem_color = pal["wood"] if spec.get("woody") else pal["stem"]
    for index in range(max(1, spec["stems"] // 2)):
        x = 16 + (index - spec["stems"] // 4) * spread_of(spec) // 2
        cv.vline(x, top + index, GROUND - 1, stem_color)
        for leaf_index in range(spec.get("leaf", 3)):
            y = GROUND - 3 - leaf_index * 4
            if y <= top + 3:
                break
            blade_leaf(cv, x, y, -3, -2, pal["leaf"], pal["leaf_dark"])
            blade_leaf(cv, x, y, 3, -2, pal["leaf_mid"], pal["leaf"])
    if ripe or fraction >= 0.8:
        head = spec.get("head", "disc")
        colors = pal["produce"]
        if head == "disc":
            petals = spec.get("petals", 12)
            for index in range(petals):
                angle = index * 6.28318 / petals
                x = 16 + round(7 * ((angle % 3.14159) / 3.14159 * 2 - 1))
                y = top + 1 + round(4 * (((index % 4) / 3) - 0.5))
                dot(cv, min(27, max(4, x)), y, 2, colors[2 if index % 2 else 3], colors[3])
            ellipse(cv, 16, top + 1, 2, 2, colors[0])
        elif head == "whorl":
            for ring in range(spec.get("accent", 5)):
                dot(cv, 15, top + ring, 1, colors[1 + ring % 2], colors[3])
                dot(cv, 17, top + ring, 1, colors[1 + ring % 2], colors[3])
        elif head == "branch_blossom":
            for index in range(spec.get("accent", 4)):
                x = 16 + ((index % 2) * 2 - 1) * (3 + index)
                dot(cv, x, top + index, 2, colors[3], pal["paper"])
                cv.set(x - 1, top + index, colors[2])
        else:
            fruit(spec, pal, cv, top)


def arch_fungus(spec, pal, cv, fraction, ripe):
    """菌类（灵芝）：短柄 + 扇形菌盖 + 盖面年轮圈。它和其余 28 种的区分是**一片叶都没有**。"""
    height = max(3, round(spec["height"] * fraction))
    top = GROUND - height
    radius = max(2, grow(spec["cap_r"], fraction))
    colors = pal["produce"]
    cv.rect(15, top + radius, 17, GROUND - 1, colors[0])
    for row in range(radius + 1):
        width = round(radius * 1.6 * (1 - (row / max(radius, 1)) ** 2 * 0.35))
        cv.hline(16 - width, 16 + width, top + row, colors[1] if row < radius // 2 else colors[0])
    cv.hline(16 - radius, 16 + radius, top, colors[3])
    cv.hline(16 - radius + 2, 16 + radius - 2, top + 2, colors[2])
    if ripe:
        cv.hline(16 - radius + 1, 16 + radius - 1, top + 1, pal["paper"])


def arch_root(spec, pal, cv, fraction, ripe):
    """根茎类（人参 / 何首乌 / 萝卜 / 冬参）：地上掌状叶顶，地下根**露出 crown 一段**。"""
    height = max(3, grow(spec["height"], fraction))
    top = GROUND - height
    crown = spec.get("crown", "swollen")
    colors = pal["produce"]
    if crown == "swollen":
        ellipse(cv, 16, GROUND - 1, 4, 3, colors[2], colors[0])
        cv.vline(16, GROUND + 1, GROUND + 3, colors[1])
    elif crown == "twisted":
        for index in range(4):
            cv.set(14 + index, GROUND - 1 + (index % 2), colors[1 + index % 3])
        cv.vline(17, GROUND, GROUND + 2, colors[0])
    else:
        for index in range(3):
            cv.hline(14 + index, 18 - index, GROUND - 1 + index, colors[2])
    stem_color = pal["wood"] if crown == "twisted" else pal["stem"]
    for index in range(max(1, spec["stems"])):
        x = 16 + (index - spec["stems"] // 2) * 2
        curve(cv, x, GROUND - 3, top, (index - spec["stems"] / 2) * 2, stem_color, pal["leaf_light"])
    leaves = max(2, grow(spec.get("leaf", 5), fraction))
    for index in range(leaves):
        side = 1 if index % 2 else -1
        dx = side * round(spread_of(spec) * (1 + index // 2) * 0.7)
        blade_leaf(cv, 16, top + 2, dx, -round(height * 0.35) - (index // 2) * 2,
                   pal["leaf"], pal["leaf_dark"])
    if spec.get("umbel") and (ripe or fraction >= 0.8):
        for index in range(spec.get("accent", 3)):
            dot(cv, 13 + index * 3, top + 1, 1, colors[3], pal["paper"])
    if spec.get("frosted"):
        for index in range(4):
            cv.set(12 + index * 3, top + index % 2, colors[3])


def arch_tree(spec, pal, cv, fraction, ripe):
    """果树三段：苗（细干小冠）→ 幼树 → 成树（冠形 + 果点）。冠形是五种树的主区分轴。"""
    height = max(3, grow(spec["height"], fraction))
    top = GROUND - height
    trunk_w = 1 if fraction < 0.6 else 2
    for dx in range(trunk_w):
        cv.vline(15 + dx, top + 6, GROUND - 1, pal["wood"])
    canopy = spec.get("canopy", "round")
    radius = max(3, round((height - 8) / 2))
    crown_y = top + radius
    if canopy == "round":
        ellipse(cv, 16, crown_y, radius + 1, radius, pal["leaf"], pal["leaf_dark"])
    elif canopy == "open":
        for offset_x, offset_y in ((-radius, 0), (radius, 1), (0, -radius // 2)):
            ellipse(cv, 16 + offset_x, crown_y + offset_y, radius - 1, radius - 2,
                    pal["leaf_mid"], pal["leaf"])
    elif canopy == "tall":
        ellipse(cv, 16, crown_y - 2, radius - 2, radius + 2, pal["hedge"], pal["hedge_dark"])
    elif canopy == "gnarl":
        for index in range(4):
            curve(cv, 16, GROUND - height // 2, top + 2, round((-1 + index * 0.7) * 4), pal["wood"])
        ellipse(cv, 16, crown_y + 1, radius, radius - 2, pal["leaf"], pal["leaf_dark"])
    else:
        for index in range(5):
            fan_x = 16 + (index - 2) * round(radius * 0.7)
            ellipse(cv, fan_x, crown_y + abs(index - 2), 3, 2,
                    pal["produce"][3], pal["produce"][2])
    if fraction > 0.5:
        for index in range(3):
            cv.set(13 + index * 3, crown_y - radius + index, pal["leaf_light"])
    if ripe:
        fruit(spec, pal, cv, crown_y, radius)


ARCHETYPES = {
    "grain": arch_grain,
    "bush": arch_bush,
    "herb": arch_herb,
    "rosette": arch_rosette,
    "allium": arch_allium,
    "vine": arch_vine,
    "trellis": arch_trellis,
    "spike": arch_spike,
    "fungus": arch_fungus,
    "root": arch_root,
    "tree": arch_tree,
}


def stage_fraction(crop_id, stage):
    return stage / stages_of(crop_id)


def draw_stage(crop_id, stage):
    """某作物某阶段的一张 32×32 图（阶段 1 基；成熟档 = 该作物的 stages 那一档）。"""
    spec = CATALOG[crop_id]
    pal = palette_for(crop_id)
    canvas = Canvas(TILE, TILE)
    contact(canvas, 16, max(2, spread_of(spec) // 2), pal["soil_shadow"])
    ARCHETYPES[spec["archetype"]](spec, pal, canvas, stage_fraction(crop_id, stage),
                                  stage == spec["stages"])
    canvas.outline(pal["ink"])
    return canvas


def _lean(canvas, dx, from_row):
    """把 from_row 以上的像素整体平移 dx 列：植株'歪一下'，而不是整格错位。"""
    rows = [[canvas.get(x, y) for x in range(TILE)] for y in range(from_row)]
    for y in range(from_row):
        for x in range(TILE):
            if rows[y][x][3]:
                canvas.set(min(TILE - 1, max(0, x + dx)), y, rows[y][x])


def _dropped_fruit(canvas, pal, count, seed):
    rnd = hash_stream(seed)
    colors = pal["produce"]
    for _ in range(count):
        dot(canvas, 6 + rnd(TILE - 12), TILE - 6 - rnd(5), 1, colors[2], colors[3])


def draw_shake(crop_id, frame):
    """收成抖动的三个槽位：歪一下 → 反向歪 + 落果 → 只剩茬口。

    为什么只出 3 张而不是 6 张：'4–6 帧'是**运行期时序**（farm.lua 连打
    `成熟 → 抖1 → 抖2 → 抖3 → 清空`），由门按时序断言；贴图给 3 帧足够凑满 5 帧。
    """
    spec = CATALOG[crop_id]
    pal = palette_for(crop_id)
    if frame == 2:
        canvas = Canvas(TILE, TILE)
        height = max(3, spec["height"] // 4)
        for index in range(4):
            x = 12 + index * 3
            canvas.vline(x, GROUND - height + (index % 2), GROUND - 1, pal["hedge_dark"])
        contact(canvas, 16, max(2, spread_of(spec) // 2), pal["soil_shadow"])
        canvas.outline(pal["ink"])
        return canvas
    canvas = draw_stage(crop_id, spec["stages"])
    _lean(canvas, 1 if frame == 0 else -1, TILE - 8)
    _dropped_fruit(canvas, pal, 3 if frame == 0 else 5, 0x5D08 + frame * 7919)
    canvas.outline(pal["ink"])
    return canvas


def signature(crop_id):
    """形态签名（**不含颜色**）：专治'同一株型换个 hue 冒充另一种作物'。"""
    spec = CATALOG[crop_id]
    return tuple(spec.get(key) for key in SIGNATURE_KEYS)


def crops_lua_path(root):
    return os.path.join(root, "scripts", "data", "crops.lua")
