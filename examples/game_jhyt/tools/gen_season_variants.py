# -*- coding: utf-8 -*-
"""《江湖有田》批次 4：四季地表变体（材质换色，不重画）。M72-11 起按场景分档。

用法（工程根目录 `examples/game_jhyt` 下）：

    python tools/gen_season_variants.py                   # 色阶表里登记且有四季表的场景
    python tools/gen_season_variants.py --scene village    # 只出某一张（可重复点名）
    python tools/gen_season_variants.py --check

产出（每个场景一份，`<scene>` 取自 `terrain_ramp.json` 的 `scenes` 段）：

    assets/art/tilesets/<scene>/<scene>_ground_<season>.png   （春/夏/秋/冬）
    assets/art/tilesets/<scene>/season_preview.png             四色对照（草/土/石/水四个满块瓦）

做法：**调色板替换**——把基础 tileset 里出现过的颜色按季节映射成新色，
没进映射表的颜色原样保留。这样四季变体与基础 tileset 的构图、tile 边界完全一致，
不会出现"秋季版本把某块石头画歪"这种返工；改季节只改映射表。

**每个场景各吃自己的表，不再合并成一张全局表**（本包改的就是这一条）：以前是
`MAPS = {**VILLAGE_MAPS, **manor_map(season)}` 一张全局替换表，对两张图集都施加一遍，
于是"village 的某个旧色恰好等于 manor 某一档"就会被悄悄换色，而这条替换没有任何地方
登记过（实测旧 village 图集里就有 6 个色与 manor 的族色同 hex）。按场景分档后跨场景串色
在结构上出不来，一个场景换季也不会连带把另一张图改了。

**季节表按材质族写，基准色一律从该场景的色阶表取**：某个场景重做完（图集色全部出自
`terrain_ramp.json`）就能并进族表，覆盖面随即自动扩大——某季少一族、某族少一档、或图集里出现
一个**出自色阶表**却既没映射、族这一季又没写 `KEEP`、也没登记成季节中性的色，都会报红。
旧图集那些表外色不在覆盖范围里（给它们现编四季目标没意义，下一版就整张换掉），由 `--check`
以 `[note]` 报数。新增场景（mountain_path 及以后）不需要动本文件的结构：色阶表加一档 `uses`，
这里加一张族表。

图集尺寸不是常量：`manor_ground.png` 与重做后的 `village_ground.png` 是 12 列 × 9 行
（384×288，逻辑瓦 + 过渡块），还没重做的旧图集还是 6 列 × 1 行。四季变体**必须与基础图同
尺寸**——引擎按组件里的 `atlas_cols/atlas_rows` 寻址，换季只换 `texture=`，尺寸一变整张地图
就错位采到别的块。

**盘上的变体还必须等于"基础图 × 当前族表"的重算**（`freshness_issues`，逐字节）：变体是三方
的派生物（基础图、族表、季节列表），只改表不重出图 = 盘上留下一版映射的图，四张各自单看都
"合法"，并排目验才发现。所以本工具改族表后必须跑一次生成，与 `set_scatter_decor.py --check`
"盘上段 == 重算逐字节相同"是同一条纪律。

对应 docs/30 §12（四季 × 每季 28 天的底层结构）与 docs/31 §3.1（四季变体属 P2）。
引擎侧只需要在切季节时把 Tilemap 的 `texture=` 换成对应季节的图，登记在
`scripts/data/season_art.lua`。`check()` 两头都对账：出了变体却没登记 = 变体生成出来没人引用，
就是 docs/48 说的"该接线或该下架"事故；登记了却没变体 = 运行期把 `texture=` 指向一张不存在的图。

依赖：Python 3 标准库 + pixel_kit.py + terrain_kit.py（色阶表）。
"""

import argparse
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import Canvas, draw_text, load_canvas, rgb, text_width, write_png  # noqa: E402
import terrain_kit as tk  # noqa: E402

SEASONS = ["spring", "summer", "autumn", "winter"]
SEASON_CN = {"spring": "春", "summer": "夏", "autumn": "秋", "winter": "冬"}

TILE = 32
SEASON_ART_LUA = os.path.join("scripts", "data", "season_art.lua")

# ---------------------------------------------------------------------------
# 季节映射：**每个场景一张族表**（`SEASON_TABLES`），族内按色阶表"深→浅"同序列出目标色。
#
# 一族要么整族 `KEEP`，要么给满该族在色阶表里的那几档——**中间不留 None**：一族的明度梯度
# 是整体搬的，压掉一档就断一档。这样"表里新增一族/一档"只会在这里显形为一个报错，不会悄悄
# 让新瓦停在基准色上。
#
# 两景共用的东西只写一份：`water` 族与通用色木 `#4A3628` 出自色阶表**同一批基准 hex**
# （village 的 kinds 里 water 就挂在顶层 water 族上），换色目标自然也相同——抄两遍就成了
# 第二个事实来源。
# ---------------------------------------------------------------------------

KEEP = "keep"

# 水：山庄池塘与村溪同族同色，只换流向不换色，所以四季目标一份两景共用。
WATER_SEASONS = {
    "spring": KEEP,
    "summer": ("#2A5273", "#33607F", "#4C7A9C", "#6E9AC4"),   # 盛夏把水整体往深里压
    "autumn": ("#325A57", "#416E6A", "#5E8C84", "#86B0A4"),   # 秋水深且偏墨绿
    "winter": ("#7AA2BC", "#8FB4CC", "#AFD0E2", "#D2E8F4"),   # 结冰：提亮并偏青
}

# 插在土里的枯枝/木桩（通用色 `wood`）：秋天转暖、冬天冻成灰木，春夏不换。
WOOD_SEASONS = {
    "spring": KEEP,
    "summer": KEEP,
    "autumn": "#5C4632",
    "winter": "#7A7268",
}

# 族名 → 该季的目标色（顺序 = terrain_ramp.json 里该族的"深→浅"顺序）。
# `KEEP` = 整族这一季不换色，是**写下来的决定**而不是遗漏：`season_map` 要求本场景 `uses`
# 里的每一族在每一季都必须出现，色阶表新增一族时这里会直接少一行而报错。
MANOR_FAMILIES = {
    "spring": {
        # 草木转嫩绿；土/石/水/瓦还是本来的样子。
        "grass": ("#577A42", "#6E9455", "#84A868", "#93B775"),
        "hedge": ("#0A4A34", "#145A3E", "#2A6B48", "#3A7B55"),
        "dirt": KEEP, "stone": KEEP, "roof": KEEP,
        "water": WATER_SEASONS["spring"],
    },
    "summer": {
        # 深绿浓荫，水整族往深里压；碎石路青瓦不动（星露谷同样只换草木与水）。
        "grass": ("#3A5528", "#4C6C38", "#5C7F45", "#6D9450"),
        "hedge": ("#003526", "#00452C", "#164E2E", "#225C38"),
        "dirt": KEEP, "stone": KEEP, "roof": KEEP,
        "water": WATER_SEASONS["summer"],
    },
    "autumn": {
        # 草黄、土暖、水偏墨绿；灰阶的石与瓦秋天不换。
        "grass": ("#7C6E34", "#9A8C46", "#B0A055", "#C0B063"),
        "hedge": ("#3A4A24", "#4A5A2E", "#5C6B38", "#6E7C44"),
        "dirt": ("#5A4028", "#6E5034", "#8A6440", "#9C7A54"),
        "stone": KEEP, "roof": KEEP,
        "water": WATER_SEASONS["autumn"],
    },
    "winter": {
        # 雪盖：草木转雪白偏青，土转冻土，水转冰，石与瓦先在色温上转冷。
        "grass": ("#B4C0BC", "#CFD8D4", "#E2E8E4", "#EFF3F0"),
        "hedge": ("#8FA39C", "#A2B4AD", "#B6C6BF", "#C8D6CF"),
        "dirt": ("#68615A", "#7E766C", "#9A9086", "#ADA398"),
        "stone": ("#6A7078", "#7E848C", "#A2A8B0", "#BAC0C6"),
        "roof": ("#46525A", "#5A666E", "#616D75", "#6E7A82"),
        "water": WATER_SEASONS["winter"],
    },
}

# 云泽村（docs/39 §2）：四季幅度与山庄同一套（只换草木水土、冬天落雪），但每一族的目标色
# 跟着本场景的基准族走——茶园的沙土、青石板、白灰墙、黛瓦都不是山庄那几族的色。
VILLAGE_FAMILIES = {
    "spring": {
        # 春草转嫩；茶园土只在暗部松一点（新叶期的地表还是去年踩实的那层）。
        "grass": ("#4F6B3E", "#668A52", "#7EA065", "#8FB077"),
        "tea_soil": ("#7A6240", "#94794E", "#AA9266", "#C0A87A"),
        "flagstone": KEEP, "plaster": KEEP, "slate": KEEP,
        "water": WATER_SEASONS["spring"],
    },
    "summer": {
        # 浓绿 + 水深；石板街与白墙夏天不褪色（徽派白墙常年就是那个白）。
        "grass": ("#344C27", "#446034", "#547444", "#648854"),
        "tea_soil": KEEP, "flagstone": KEEP, "plaster": KEEP, "slate": KEEP,
        "water": WATER_SEASONS["summer"],
    },
    "autumn": {
        # 草黄、茶园土往暖里走一档（秋收的晒场）；石/墙/瓦仍不换。
        "grass": ("#6E622E", "#8A7C40", "#A08E4E", "#B2A05E"),
        "tea_soil": ("#7A5A32", "#96713F", "#AD8750", "#C29C66"),
        "flagstone": KEEP, "plaster": KEEP, "slate": KEEP,
        "water": WATER_SEASONS["autumn"],
    },
    "winter": {
        # 雪盖：草转雪白偏青，茶园土冻成灰，青石板在色温上转冷（雪落进石缝而不是整块盖白——
        # 石板街要能看出是街）；白墙提亮偏冷，黛瓦结霜。白墙最浅一档停在 #EDF2F1：日光档
        # 乘 1.03 后仍在 255 以内（docs/53 §2 不许吹白）。
        "grass": ("#AAB6B2", "#C6D0CC", "#DAE2DE", "#E8EEEA"),
        "tea_soil": ("#807668", "#9C9288", "#B8AEA2", "#CDC3B6"),
        "flagstone": ("#7E868A", "#9CA6AC", "#B4BEC4", "#C8D2D8"),
        "plaster": ("#9AA0A4", "#C4CCCE", "#E2EAE9", "#EDF2F1"),
        "slate": ("#2A3A46", "#3E5060", "#52687A", "#6E8698"),
        "water": WATER_SEASONS["winter"],
    },
}

# 色阶表里不属于任何材质族的通用色（`utility`）：按基准 hex 单列，同样四季都要表态。
# 墨线 `#1F2328` 四季都不换，但它由 `SEASON_NEUTRAL` 登记，不在这里重复。
# 灯笼金 `#C9A05A`（只有 village 的图集用到）是点缀色——它标的是人造物的高光，不属地表
# 材质，所以四季 `KEEP` 同样是写下来的决定而不是漏。
MANOR_UTILITY = {season: {"#4A3628": WOOD_SEASONS[season]} for season in SEASONS}
VILLAGE_UTILITY = {season: {**MANOR_UTILITY[season], "#C9A05A": KEEP} for season in SEASONS}

# 四季都不换的色：统一 1px 墨线。列在这里而不是"没写进表"，是因为 `coverage_issues()`
# 要求图集中每个不透明色要么有映射、要么这一季整族 `KEEP`、要么在这里登记为季节中性，
# 三者都不沾就报错。
SEASON_NEUTRAL = {"#1F2328"}

# 场景 → 族表 + 通用色表。没在这里点名的场景直接报红而不是兜底成 manor：少写一季的结果是
# "这张图四季长得一模一样"，而那正是最难从画面上发现的一种漏。
SEASON_TABLES = {
    "manor": {"families": MANOR_FAMILIES, "utility": MANOR_UTILITY},
    "village": {"families": VILLAGE_FAMILIES, "utility": VILLAGE_UTILITY},
}


def base_path(scene):
    """某场景的基础地表图集（四季变体的母图）。路径式子只在 `tk.ground_texture` 里写一次。"""
    return os.path.join(*tk.ground_texture(scene).split("/"))


def variant_path(scene, season):
    return f"{os.path.splitext(base_path(scene))[0]}_{season}.png"


def preview_path(scene):
    """四色对照跟着场景走（以前两张图共用 `tilesets/season_preview.png`，实际只画了 manor）。"""
    return os.path.join(os.path.dirname(base_path(scene)), "season_preview.png")


def spec_of(scene):
    spec = SEASON_TABLES.get(scene)
    if spec is None:
        raise SystemExit(f"[FAIL] 场景 {scene!r} 没有季节换色表（`SEASON_TABLES` 里点名了 "
                         f"{sorted(SEASON_TABLES)}）——色阶表新增一档场景不会自动有四季，"
                         f"要么补表要么别出图")
    return spec


def season_map(scene, table, season):
    """某场景某季的扁平 {基准 hex: 目标 hex}。基准侧一律从该场景的色阶表取。

    这里校验的是"族表与色阶表对不对得上"，不是"画面好不好看"：某季少一族、某族少一档、
    通用色写了个表里没有的 hex，都在这里炸掉——图集里因此不可能出现"某一族整族没表态"。
    """
    spec = spec_of(scene)
    out = {}
    families = spec["families"][season]
    declared = set(families)
    wanted = set(table["uses"])
    if declared != wanted:
        raise ValueError(
            f"[{scene}] {season}：色阶表的 uses 是 {sorted(wanted)}，季节表缺 "
            f"{sorted(wanted - declared)}、多出 {sorted(declared - wanted)}"
        )
    for family, targets in families.items():
        if targets is KEEP:
            continue
        base = tk.family_hexes(table, family)
        if len(base) != len(targets) or any(t is None for t in targets):
            raise ValueError(
                f"[{scene}] {season}/{family}：要么整族 KEEP，要么给满 {len(base)} 档目标色"
                f"（不允许中间留 None：一族的明度梯度是整体搬的，压掉一档就断一档）"
            )
        out.update(zip(base, targets))
    utility = spec["utility"][season]
    known = {hex_value.upper() for hex_value in table["utility"].values()}
    if set(utility) - known:
        raise ValueError(f"[{scene}] {season}：通用色表里的 {sorted(set(utility) - known)} "
                         f"不在色阶表 utility 里")
    for base_hex, target in utility.items():
        if target is not KEEP:
            out[base_hex] = target
    return out


def parse_hex(value):
    value = value.lstrip("#")
    if len(value) != 6:
        raise ValueError(f"非法颜色：{value}")
    return (int(value[0:2], 16), int(value[2:4], 16), int(value[4:6], 16), 255)


def apply_season(canvas, mapping):
    table = {}
    for base, target in mapping.items():
        table[parse_hex(base)[:3]] = parse_hex(target)
    out = Canvas(canvas.w, canvas.h)
    for y in range(canvas.h):
        for x in range(canvas.w):
            color = canvas.get(x, y)
            if color[3] == 0:
                continue
            out.set(x, y, table.get(color[:3], color))
    return out


def build_preview(table, variants, scale=3):
    """四色对照：行=季节，列=草/土/石/水的**满块档**条目。

    取样按条目号而不是"第几列"：M72-04 之后 atlas 是 12 列 × 9 行，列号算不出草在哪。
    `tk.entry_base` 就是引擎恒等寻址（`tiles==id`）用的那一格，所以这张对照图同时也是
    "关掉 autotile 立刻回滚"的目验入口。

    列标题从**本场景的 kinds** 取：village 的 id 2 名字仍叫 dirt，画的是茶园沙土
    （tea_soil 族）；照 manor 的写法把标签写死，读图的人就会以为那是夯土。
    """
    columns = [1, 2, 3, 6]  # kind id：草/土/石/水
    labels = [tk.kind_of(table, kind)["name"].upper() for kind in columns]
    tile = TILE * scale
    label_w = text_width("SUMMER", 1) + 12
    cv = Canvas(label_w + len(columns) * (tile + 6) + 8, 20 + len(variants) * (tile + 22) + 8)
    cv.rect(0, 0, cv.w - 1, cv.h - 1, rgb("#14171A"))
    draw_text(cv, 6, 6, f"{table['scene'].upper()} SEASON VARIANTS 32PX X3", rgb("#E4C078"), 1)
    y = 20
    for season, tileset in variants:
        draw_text(cv, 6, y + 8, season.upper(), rgb("#A9C28F"), 1)
        for index, kind in enumerate(columns):
            x = label_w + index * (tile + 6)
            draw_text(cv, x, y, labels[index], rgb("#A89F8A"), 1)
            cv.rect(x - 1, y + 10, x + tile, y + 10 + tile, rgb("#262C33"))
            patch = tk.entry_region(tileset, table, tk.entry_base(table, kind))
            cv.blit(patch.scaled(scale), x, y + 10)
        y += tile + 22
    return cv


def scenes_to_do(only=None):
    """本轮要出图的场景 = 色阶表里登记的那些（`--scene` 只能收窄，不能凭空加）。

    登记了却还没有四季表的场景在这里筛掉并报数，而不是 generate 到一半炸：铺量是逐场景推进
    的，中途"某张图还没换色表"是正常状态，但它必须被说出来而不是被静悄悄跳过。
    """
    known = tk.scenes()
    if only:
        unknown = sorted(set(only) - set(known))
        if unknown:
            raise SystemExit(f"[FAIL] 色阶表里没有场景 {unknown}，可用：{known}")
        for scene in only:
            spec_of(scene)      # 点名了却没表 = 直接报错，不静默跳过
        return list(only)
    picked = [scene for scene in known if scene in SEASON_TABLES]
    missing = [scene for scene in known if scene not in SEASON_TABLES]
    if missing:
        print(f"[warn] 色阶表登记了但还没有四季表：{missing}（重做完再并进来）", file=sys.stderr)
    return picked


def generate(only=None, quiet=False):
    for scene in scenes_to_do(only):
        table = tk.load_ramp(scene)
        path = base_path(scene)
        if not os.path.exists(path):
            print(f"[warn] 缺少基础 tileset：{path}"
                  f"（先跑 gen_terrain_tiles.py --scene {scene}）", file=sys.stderr)
            continue
        base = load_canvas(path)
        mappings = {season: season_map(scene, table, season) for season in SEASONS}
        variants = []
        for season in SEASONS:
            canvas = apply_season(base, mappings[season])
            out = variant_path(scene, season)
            write_png(out, canvas)
            variants.append((season, canvas))
            if not quiet:
                print(f"  {out} {canvas.w}x{canvas.h}")
        write_png(preview_path(scene), build_preview(table, variants))
        if not quiet:
            print(f"  {preview_path(scene)}")


def opaque_colors(canvas):
    """图集里出现过的不透明色（大写 hex 集合）。"""
    out = set()
    for y in range(canvas.h):
        for x in range(canvas.w):
            color = canvas.get(x, y)
            if color[3]:
                out.add("#%02X%02X%02X" % color[:3])
    return out


def coverage_issues(scene, table):
    """四季对该场景图集的换色覆盖率。

    一个**出自色阶表**的色算"表过态"当且仅当：它有换色目标、它所属的族这一季是 `KEEP`、
    它的通用色条目写的是 `KEEP`、或它登记在 `SEASON_NEUTRAL`。四种都不沾 = 要么漏了映射、
    要么色阶表新增了东西而季节表没跟上。

    范围只圈色阶表里的色，而不是"图集里每个不透明色"（M72-04 的口径）：还没重做的旧图集
    有一堆表外的旧色，给它们现编四季目标没有意义（下一版就要整张换掉），而"旧色算不算漏"
    这件事本来就不归季节表管——图集色必须全部出自色阶表是 `gen_terrain_tiles.py --check`
    的职责。于是这里只测"表里有的色有没有表态"，一张图重做完（表内色变多）覆盖面就自动扩大，
    不需要这里再记一份"哪些场景算重做完了"的状态。余下的表外色由 `legacy_colors` 报数。

    `family_of` 只按本场景的 `uses` 收口：不属于本场景的族若在图集里出现，那是越表色（上一段
    说的就是它），这里不能因为"它在库里且某季 KEEP"就放行。
    """
    path = base_path(scene)
    if not os.path.exists(path):
        return [f"missing base {path}"]
    colors = opaque_colors(load_canvas(path))
    family_of = {}
    for family in table["uses"]:
        for hex_value in table["families"][family]:
            family_of[hex_value.upper()] = family
    in_table = set(family_of) | {hex_value.upper() for hex_value in table["utility"].values()}
    spec = spec_of(scene)
    issues = []
    for season in SEASONS:
        mapping = season_map(scene, table, season)
        kept_utility = {hex_value for hex_value, target in spec["utility"][season].items()
                        if target is KEEP}
        undecided = []
        for hex_value in sorted(colors & in_table):
            if hex_value in mapping or hex_value in SEASON_NEUTRAL:
                continue
            if hex_value in kept_utility:
                continue
            family = family_of.get(hex_value)
            if family is not None and spec["families"][season][family] is KEEP:
                continue
            undecided.append(hex_value)
        if undecided:
            issues.append(f"{scene}/{season}: {len(undecided)} 个表内色没有换色目标、所属族也不是 "
                          f"KEEP、未登记为季节中性 → {undecided}")
    return issues


def legacy_colors(scene, table):
    """该场景图集里**不在色阶表里**的旧色（覆盖率管不着的那些）。重做完应当是空集。"""
    path = base_path(scene)
    if not os.path.exists(path):
        return []
    in_table = {hex_value.upper() for family in table["uses"]
                for hex_value in table["families"][family]}
    in_table |= {hex_value.upper() for hex_value in table["utility"].values()}
    return sorted(opaque_colors(load_canvas(path)) - in_table)


def freshness_issues(scene, table):
    """盘上的变体必须等于"当前基础图 × 当前族表"重算的结果（逐字节）。

    为什么值得逐字节而不止"存在且同尺寸"：本工具的产出是**派生物**，输入有三处（基础图、
    族表、季节列表）。只改了族表没重出图，盘上就是"上一版映射 × 这一版图"，四季各自少换
    一族而每张图单看都"合法"——目验要四张并排看才发现。逐字节比把这条变成一条等式：
    重出图（`python tools/gen_season_variants.py`）之后必须绿。
    """
    from pixel_kit import decode_png

    path = base_path(scene)
    if not os.path.exists(path):
        return [f"missing base {path}"]
    issues = []
    base = load_canvas(path)
    want_size = decode_png(path)[:2]
    for season in SEASONS:
        variant = variant_path(scene, season)
        if not os.path.exists(variant):
            issues.append("missing " + variant)
            continue
        # 变体必须与基础图同尺寸：引擎按 atlas_cols/rows 寻址，尺寸一变就整张地图错位。
        got_size = decode_png(variant)[:2]
        if got_size != want_size:
            issues.append(f"{variant}: {got_size[0]}x{got_size[1]} != 基础图 "
                          f"{want_size[0]}x{want_size[1]}")
            continue
        expected = apply_season(base, season_map(scene, table, season)).to_png()
        with open(variant, "rb") as handle:
            if handle.read() != expected:
                issues.append(f"{variant} 与「基础图 × 当前族表」的重算不一致（改了族表或基础图"
                              f"却没重出图）——跑一次 python tools/gen_season_variants.py "
                              f"--scene {scene}")
    return issues


def wired_scenes():
    """`season_art.lua` 里登记了地表换季的地图键（运行期真会去 load 的那几张）。

    读法只认 `M.ground = { ... }` 这一段里两空格缩进的 `key = {`：整份 Lua 不执行，
    表结构一改这里就报红（见 `wiring_issues` 的兜底那条），不会静默读成空集。
    """
    if not os.path.exists(SEASON_ART_LUA):
        return None
    with open(SEASON_ART_LUA, encoding="utf-8") as handle:
        content = handle.read()
    block = re.search(r"^M\.ground\s*=\s*\{(.*?)^\}", content, re.S | re.M)
    if block is None:
        return None
    return set(re.findall(r"^  (\w+)\s*=\s*\{", block.group(1), re.M))


def wiring_issues(scenes):
    """"本工具出图的场景"与"`season_art.lua` 登记了地表换季的场景"必须对得上。

    两个方向的事故不一样，所以两条分别判：

      * 出了变体却没登记 = **死资源**（docs/48 的口径：接线或下架），画面上永远是春；
      * 登记了却没变体 = 运行期把 `texture=` 指向一张不存在的图。

    第二条按**文件**判而不是按"有没有四季表"判：还没并进族表的场景（village 在 M72-11 的
    前一个 commit 之前就是这种）盘上仍有旧变体，运行期不炸；这时它缺的是**族表**而不是图，
    那件事由 `scenes_to_do` 的 `[warn]` 说，不该在这里报红——本包是逐场景推进的，
    "一半已重做、一半还没"是过程中的正常状态。
    """
    wired = wired_scenes()
    if wired is None:
        return [f"{SEASON_ART_LUA} 缺失或找不到 `M.ground = {{ ... }}` 那一段，"
                f"无法核对地表换季登记"]
    have = set(scenes)
    issues = []
    if have - wired:
        issues.append(f"出了四季变体但 `season_art.lua` 没登记（等于死资源）：{sorted(have - wired)}")
    from pixel_kit import decode_png

    for scene in sorted(wired - have):
        path = base_path(scene)
        if not os.path.exists(path):
            issues.append(f"`season_art.lua` 登记了 {scene}，但基础图集不在：{path}")
            continue
        want = decode_png(path)[:2]
        for season in SEASONS:
            variant = variant_path(scene, season)
            if not os.path.exists(variant):
                issues.append(f"`season_art.lua` 登记了 {scene}，缺 {variant}")
                continue
            got = decode_png(variant)[:2]
            if got != want:
                issues.append(f"{variant}: {got[0]}x{got[1]} != 基础图 {want[0]}x{want[1]}")
    return issues


def check(only=None):
    """返回 (issues, notes)：issues 非空就是红，notes 只说"还差什么"而不拦。

    notes 只有一类：未重做的旧图集那些**表外色**。它们不该让门红（季节表管不着），但必须被
    说出来——否则"village 重做了却没有四季"和"village 还没重做"这两种状态在外表上一模一样。
    """
    scenes = scenes_to_do(only)
    issues, notes = [], []
    for scene in scenes:
        table = tk.load_ramp(scene)
        if not os.path.exists(base_path(scene)):
            issues.append(f"missing base {base_path(scene)}")
            continue
        try:
            issues.extend(freshness_issues(scene, table))
            issues.extend(coverage_issues(scene, table))
        except ValueError as error:      # 族表与色阶表对不上：拿不到映射，后面也测不了
            issues.append(str(error))
        if not os.path.exists(preview_path(scene)):
            issues.append(f"missing {preview_path(scene)}")
        legacy = legacy_colors(scene, table)
        if legacy:
            notes.append(f"{scene}: 图集仍有 {len(legacy)} 个色不在色阶表里（旧图集未重做）→ "
                         f"这些色不受季节表约束，重做完会自动进覆盖率校验")
    # 地表换季登记的对照只在**全量**跑时成立：`--scene manor` 时 village 本来就不在盘上比对面里。
    if not only:
        issues.extend(wiring_issues(scenes))
    return issues, notes


def main():
    parser = argparse.ArgumentParser(description="《江湖有田》四季地表变体生成（按场景分档）")
    parser.add_argument("--scene", action="append", choices=tk.scenes(),
                        help="只做点名的场景（可重复）；默认=色阶表里登记且有四季表的场景")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        problems, notes = check(args.scene)
        for note in notes:
            print("[note]", note)
        if problems:
            for problem in problems:
                print("[FAIL]", problem)
            raise SystemExit(1)
        picked = scenes_to_do(args.scene)
        print(f"[OK] 四季变体校验通过（{len(SEASONS)} 季 × {len(picked)} tileset：{picked}）")
        return
    print("生成四季地表变体 →", SEASON_CN)
    generate(args.scene)


if __name__ == "__main__":
    main()
