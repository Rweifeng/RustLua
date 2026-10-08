# -*- coding: utf-8 -*-
"""《江湖有田》批次 4：四季地表变体（材质换色，不重画）。

用法：
    python tools/gen_season_variants.py
    python tools/gen_season_variants.py --check

产出：
    assets/art/tilesets/manor/manor_ground_<season>.png     （春/夏/秋/冬）
    assets/art/tilesets/village/village_ground_<season>.png
    assets/art/tilesets/season_preview.png                  四色对照（草/土/石/水四格）

做法：**调色板替换**——把基础 tileset 里出现过的颜色按季节映射成新色，
没进映射表的颜色原样保留。这样四季变体与基础 tileset 的构图、tile 边界完全一致，
不会出现"秋季版本把某块石头画歪"这种返工；改季节只改映射表。

图集尺寸不是常量：M72-04 起 `manor_ground.png` 是 12 列 × 9 行（384×288，逻辑瓦 +
过渡块），village 仍是 6 列 × 1 行。四季变体**必须与基础图同尺寸**——引擎按组件里的
`atlas_cols/atlas_rows` 寻址，换季只换 `texture=`，尺寸一变整张地图就错位采到别的块。
`check()` 就是拿基础图的实测尺寸逐个比对变体。

对应 docs/30 §12（四季 × 每季 28 天的底层结构）与 docs/31 §3.1（四季变体属 P2）。
引擎侧只需要在切季节时把 Tilemap 的 `texture=` 换成对应季节的图。

依赖：Python 3 标准库 + pixel_kit.py + terrain_kit.py（色阶表）。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import Canvas, draw_text, load_canvas, rgb, text_width  # noqa: E402
import terrain_kit as tk  # noqa: E402

SEASONS = ["spring", "summer", "autumn", "winter"]
SEASON_CN = {"spring": "春", "summer": "夏", "autumn": "秋", "winter": "冬"}

TILE = 32
MANOR = os.path.join("assets", "art", "tilesets", "manor", "manor_ground.png")
VILLAGE = os.path.join("assets", "art", "tilesets", "village", "village_ground.png")

# ---------------------------------------------------------------------------
# 季节映射分两张表，因为两个 tileset 的"基准色"来源不同：
#
# * `MANOR_FAMILIES`：manor 自 M72-04 起所有地表色都出自 `terrain_ramp.json` 的族色阶，
#   所以映射按**族**写，元组顺序必须与表内"深→浅"同序。一族要么整族 `KEEP`，要么四档
#   一起给目标色——**中间不留 None**：明度梯度是整体搬的，压掉一档就断一档。
#   这样"表里新增一族/一档"只会在这里显形为一个报错，不会悄悄让新瓦停在基准色上。
# * `VILLAGE_MAPS`：village（以及尚未重做的其余 5 族 tileset）用的还是旧单档色，
#   那些 hex 不在色阶表里，只能按老写法列。M72-11 重做它们时再并进族表。
# * 两张表合并成一张全局替换表：village 的水/石与 manor 共用同色，合并后行为不变。
# ---------------------------------------------------------------------------

# 族名 → 该季的四档目标色（顺序 = terrain_ramp.json 内该族的"深→浅"顺序）。
# `KEEP` = 整族这一季不换色，是**写下来的决定**而不是遗漏：`check()` 要求六个族在每一季
# 都必须出现，色阶表新增一族时这里会直接少一行而报错，不会悄悄让新瓦停在基准色上。
KEEP = "keep"

MANOR_FAMILIES = {
    "spring": {
        # 草木转嫩绿；土/石/水/瓦还是本来的样子。
        "grass": ("#577A42", "#6E9455", "#84A868", "#93B775"),
        "hedge": ("#0A4A34", "#145A3E", "#2A6B48", "#3A7B55"),
        "dirt": KEEP, "stone": KEEP, "water": KEEP, "roof": KEEP,
    },
    "summer": {
        # 深绿浓荫，水整族往深里压；碎石路青瓦不动（星露谷同样只换草木与水）。
        "grass": ("#3A5528", "#4C6C38", "#5C7F45", "#6D9450"),
        "hedge": ("#003526", "#00452C", "#164E2E", "#225C38"),
        "water": ("#2A5273", "#33607F", "#4C7A9C", "#6E9AC4"),
        "dirt": KEEP, "stone": KEEP, "roof": KEEP,
    },
    "autumn": {
        # 草黄、土暖、水偏墨绿；灰阶的石与瓦秋天不换。
        "grass": ("#7C6E34", "#9A8C46", "#B0A055", "#C0B063"),
        "hedge": ("#3A4A24", "#4A5A2E", "#5C6B38", "#6E7C44"),
        "dirt": ("#5A4028", "#6E5034", "#8A6440", "#9C7A54"),
        "water": ("#325A57", "#416E6A", "#5E8C84", "#86B0A4"),
        "stone": KEEP, "roof": KEEP,
    },
    "winter": {
        # 雪盖：草木转雪白偏青，土转冻土，水转冰，石与瓦先在色温上转冷。
        "grass": ("#B4C0BC", "#CFD8D4", "#E2E8E4", "#EFF3F0"),
        "hedge": ("#8FA39C", "#A2B4AD", "#B6C6BF", "#C8D6CF"),
        "dirt": ("#68615A", "#7E766C", "#9A9086", "#ADA398"),
        "stone": ("#6A7078", "#7E848C", "#A2A8B0", "#BAC0C6"),
        "water": ("#7AA2BC", "#8FB4CC", "#AFD0E2", "#D2E8F4"),
        "roof": ("#46525A", "#5A666E", "#616D75", "#6E7A82"),
    },
}

# 色阶表里不属于任何材质族的通用色（`utility`）：按基准 hex 单列，同样四季都要表态。
# 墨线 `#1F2328` 四季都不换，但它由 `SEASON_NEUTRAL` 登记，不在这里重复。
MANOR_UTILITY = {
    "spring": {"#4A3628": KEEP},
    "summer": {"#4A3628": KEEP},
    "autumn": {"#4A3628": "#5C4632"},   # 插在土里的枯枝转暖
    "winter": {"#4A3628": "#7A7268"},   # 冻成灰木
}

# 四季都不换的色：统一 1px 墨线。列在这里而不是"没写进表"，是因为 `check()`
# 要求图集中每个不透明色要么有映射、要么在这里登记为季节中性，两者都不沾就报错。
SEASON_NEUTRAL = {"#1F2328"}

VILLAGE_MAPS = {
    "spring": {
        "#547242": "#668A52", "#425A33": "#4F6B3E", "#668454": "#7EA065", "#7A9A62": "#8FB077",
        "#8A7048": "#94794E", "#6E5836": "#7A6240",
    },
    "summer": {
        "#547242": "#446034", "#425A33": "#344C27", "#668454": "#547444", "#7A9A62": "#648854",
    },
    "autumn": {
        "#547242": "#8A7C40", "#425A33": "#6E622E", "#668454": "#A08E4E", "#7A9A62": "#B2A05E",
    },
    "winter": {
        "#547242": "#C6D0CC", "#425A33": "#AAB6B2", "#668454": "#DAE2DE", "#7A9A62": "#E8EEEA",
        "#8A7048": "#9C9288", "#6E5836": "#807668", "#A08858": "#B8AEA2",
        "#C9C1B0": "#E4E0D6", "#AFA795": "#CEC8BC",
    },
}

RAMP = tk.load_ramp()


def manor_map(season):
    """族表 + 通用色表 → 扁平的 {基准 hex: 目标 hex}，基准侧一律从色阶表取。"""
    out = {}
    declared = set(MANOR_FAMILIES[season])
    wanted = set(RAMP["families"])
    if declared != wanted:
        raise ValueError(
            f"{season}：色阶表的族是 {sorted(wanted)}，季节表缺 {sorted(wanted - declared)}、"
            f"多出 {sorted(declared - wanted)}"
        )
    for family, targets in MANOR_FAMILIES[season].items():
        if targets is KEEP:
            continue
        base = tk.family_hexes(RAMP, family)
        if len(base) != len(targets) or any(t is None for t in targets):
            raise ValueError(
                f"{season}/{family}：要么整族 KEEP，要么给满 {len(base)} 档目标色"
                f"（不允许中间留 None：一族的明度梯度是整体搬的，压掉一档就断一档）"
            )
        out.update(zip(base, targets))
    utility = set(MANOR_UTILITY[season])
    known = {h.upper() for h in RAMP["utility"].values()}
    if utility - known:
        raise ValueError(f"{season}：通用色表里的 {sorted(utility - known)} 不在色阶表 utility 里")
    for base_hex, target in MANOR_UTILITY[season].items():
        if target is not KEEP:
            out[base_hex] = target
    return out


MAPS = {season: {**VILLAGE_MAPS[season], **manor_map(season)} for season in SEASONS}


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


def build_preview(variants, scale=3):
    """四色对照：行=季节，列=草/土/石/水的**满块档**条目。

    取样按条目号而不是"第几列"：M72-04 之后 atlas 是 12 列 × 9 行，列号算不出草在哪。
    `tk.entry_base` 就是引擎恒等寻址（`tiles==id`）用的那一格，所以这张对照图同时也是
    "关掉 autotile 立刻回滚"的目验入口。
    """
    columns = [1, 2, 3, 6]  # kinds：草/土/石/水（terrain_ramp.json 的 id）
    labels = ["GRASS", "DIRT", "STONE", "WATER"]
    tile = TILE * scale
    label_w = text_width("SUMMER", 1) + 12
    cv = Canvas(label_w + len(columns) * (tile + 6) + 8, 20 + len(variants) * (tile + 22) + 8)
    cv.rect(0, 0, cv.w - 1, cv.h - 1, rgb("#14171A"))
    draw_text(cv, 6, 6, "SEASON VARIANTS 32PX X3", rgb("#E4C078"), 1)
    y = 20
    for season, tileset in variants:
        draw_text(cv, 6, y + 8, season.upper(), rgb("#A9C28F"), 1)
        for index, kind in enumerate(columns):
            x = label_w + index * (tile + 6)
            draw_text(cv, x, y, labels[index], rgb("#A89F8A"), 1)
            cv.rect(x - 1, y + 10, x + tile, y + 10 + tile, rgb("#262C33"))
            patch = tk.entry_region(tileset, RAMP, tk.entry_base(RAMP, kind))
            cv.blit(patch.scaled(scale), x, y + 10)
        y += tile + 22
    return cv


def write_png(path, canvas):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(canvas.to_png())


def generate(quiet=False):
    variants = []
    for base_path in (MANOR, VILLAGE):
        if not os.path.exists(base_path):
            print(f"[warn] 缺少基础 tileset：{base_path}（先跑对应的 gen_*_art.py）", file=sys.stderr)
            continue
        base = load_canvas(base_path)
        stem = os.path.splitext(base_path)[0]
        for season in SEASONS:
            canvas = apply_season(base, MAPS[season])
            out = f"{stem}_{season}.png"
            write_png(out, canvas)
            if base_path == MANOR:
                variants.append((season, canvas))
            if not quiet:
                print(f"  {out} {canvas.w}x{canvas.h}")
    if variants:
        write_png(os.path.join("assets", "art", "tilesets", "season_preview.png"),
                  build_preview(variants))
        if not quiet:
            print("  assets/art/tilesets/season_preview.png")


def opaque_colors(canvas):
    """图集里出现过的不透明色（大写 hex 集合）。"""
    out = set()
    for y in range(canvas.h):
        for x in range(canvas.w):
            color = canvas.get(x, y)
            if color[3]:
                out.add("#%02X%02X%02X" % color[:3])
    return out


def coverage_issues():
    """四季对 manor 图集的换色覆盖率。

    一个色算"表过态"当且仅当：它有换色目标、它所属的族这一季是 `KEEP`、它的通用色条目
    写的是 `KEEP`、或它登记在 `SEASON_NEUTRAL`。四种都不沾 = 要么漏了映射、要么色阶表
    新增了东西而季节表没跟上。

    只查 manor：它是唯一"基准色全部来自色阶表"的 tileset。village 等旧图集还有一堆不在
    表里的色，M72-11 重做时再纳入，现在强行要求覆盖只会逼着给旧色现编目标。
    """
    issues = []
    if not os.path.exists(MANOR):
        return [f"missing base {MANOR}"]
    colors = opaque_colors(load_canvas(MANOR))
    family_of = {}
    for family, levels in RAMP["families"].items():
        for hex_value in levels:
            family_of[hex_value.upper()] = family
    for season in SEASONS:
        kept_utility = {hex_value for hex_value, target in MANOR_UTILITY[season].items()
                        if target is KEEP}
        undecided = []
        for hex_value in sorted(colors):
            if hex_value in MAPS[season] or hex_value in SEASON_NEUTRAL:
                continue
            if hex_value in kept_utility:
                continue
            family = family_of.get(hex_value)
            if family is not None and MANOR_FAMILIES[season][family] is KEEP:
                continue
            undecided.append(hex_value)
        if undecided:
            issues.append(f"{season}: {len(undecided)} 个图集色没有换色目标、所属族也不是 KEEP、"
                          f"未登记为季节中性 → {undecided}")
    return issues


def check():
    from pixel_kit import decode_png

    issues = []
    for base_path in (MANOR, VILLAGE):
        if not os.path.exists(base_path):
            issues.append(f"missing base {base_path}")
            continue
        # 变体必须与基础图同尺寸：引擎按 atlas_cols/rows 寻址，尺寸一变就整张地图错位。
        want = decode_png(base_path)[:2]
        stem = os.path.splitext(base_path)[0]
        for season in SEASONS:
            path = f"{stem}_{season}.png"
            if not os.path.exists(path):
                issues.append("missing " + path)
                continue
            got = decode_png(path)[:2]
            if got != want:
                issues.append(f"{path}: {got[0]}x{got[1]} != 基础图 {want[0]}x{want[1]}")
    if not os.path.exists(os.path.join("assets", "art", "tilesets", "season_preview.png")):
        issues.append("missing season_preview.png")
    issues.extend(coverage_issues())
    return issues


def main():
    parser = argparse.ArgumentParser(description="《江湖有田》四季地表变体生成")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        problems = check()
        if problems:
            for problem in problems:
                print("[FAIL]", problem)
            raise SystemExit(1)
        print(f"[OK] 四季变体校验通过（{len(SEASONS)} 季 × 2 tileset）")
        return
    print("生成四季地表变体 →", SEASON_CN)
    generate()


if __name__ == "__main__":
    main()
