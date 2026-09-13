# -*- coding: utf-8 -*-
"""《江湖有田》批次 4：四季地表变体（材质换色，不重画）。

用法：
    python tools/gen_season_variants.py
    python tools/gen_season_variants.py --check

产出：
    assets/art/tilesets/manor/manor_ground_<season>.png     （春/夏/秋/冬）
    assets/art/tilesets/village/village_ground_<season>.png
    assets/art/tilesets/season_preview.png                  四色对照（草/土/石/水四列）

做法：**调色板替换**——把基础 tileset 里出现过的颜色按季节映射成新色，
没进映射表的颜色原样保留。这样四季变体与基础 tileset 的构图、tile 边界完全一致，
不会出现"秋季版本把某块石头画歪"这种返工；改季节只改映射表。

对应 docs/30 §12（四季 × 每季 28 天的底层结构）与 docs/31 §3.1（四季变体属 P2）。
引擎侧只需要在切季节时把 Tilemap 的 `texture=` 换成对应季节的图。

依赖：Python 3 标准库 + pixel_kit.py。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import Canvas, draw_text, load_canvas, rgb, text_width  # noqa: E402

SEASONS = ["spring", "summer", "autumn", "winter"]
SEASON_CN = {"spring": "春", "summer": "夏", "autumn": "秋", "winter": "冬"}

TILE = 32
MANOR = os.path.join("assets", "art", "tilesets", "manor", "manor_ground.png")
VILLAGE = os.path.join("assets", "art", "tilesets", "village", "village_ground.png")

# 季节映射：{基础色: 该季节的替换色}。基础色取自两个 tileset 生成器里的常量。
MAPS = {
    "spring": {
        # 草木转嫩绿，其余保持
        "#5E7C46": "#6E9455", "#4A6636": "#577A42", "#6F8F52": "#84A868", "#7FA35E": "#93B775",
        "#547242": "#668A52", "#425A33": "#4F6B3E", "#668454": "#7EA065", "#7A9A62": "#8FB077",
        "#8A7048": "#94794E", "#6E5836": "#7A6240",
    },
    "summer": {
        # 深绿浓荫
        "#5E7C46": "#4C6C38", "#4A6636": "#3A5528", "#6F8F52": "#5C7F45", "#7FA35E": "#6D9450",
        "#547242": "#446034", "#425A33": "#344C27", "#668454": "#547444", "#7A9A62": "#648854",
        "#3E6485": "#33607F",
    },
    "autumn": {
        # 草黄、土暖、水偏墨绿
        "#5E7C46": "#9A8C46", "#4A6636": "#7C6E34", "#6F8F52": "#B0A055", "#7FA35E": "#C0B063",
        "#547242": "#8A7C40", "#425A33": "#6E622E", "#668454": "#A08E4E", "#7A9A62": "#B2A05E",
        "#7C5C3E": "#8A6440", "#63482F": "#6E5034", "#8F6E4A": "#9C7A54",
        "#3E6485": "#416E6A", "#2F4E6B": "#325A57", "#5D82A0": "#5E8C84", "#7FA3D4": "#86B0A4",
    },
    "winter": {
        # 雪盖：草木转雪白偏青，土转冻土，水转冰
        "#5E7C46": "#CFD8D4", "#4A6636": "#B4C0BC", "#6F8F52": "#E2E8E4", "#7FA35E": "#EFF3F0",
        "#547242": "#C6D0CC", "#425A33": "#AAB6B2", "#668454": "#DAE2DE", "#7A9A62": "#E8EEEA",
        "#7C5C3E": "#9A9086", "#63482F": "#7E766C", "#8F6E4A": "#ADA398",
        "#8A7048": "#9C9288", "#6E5836": "#807668", "#A08858": "#B8AEA2",
        "#3E6485": "#8FB4CC", "#2F4E6B": "#7AA2BC", "#5D82A0": "#AFD0E2", "#7FA3D4": "#D2E8F4",
        "#3E4A52": "#5A666E", "#55626B": "#6E7A82", "#2C363C": "#46525A",  # 瓦上落雪偏亮
        "#C9C1B0": "#E4E0D6", "#AFA795": "#CEC8BC",
    },
}


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
    """四色对照：行=季节，列=草/土/石/水。"""
    columns = [0, 1, 2, 5]
    labels = ["GRASS", "DIRT", "STONE", "WATER"]
    tile = TILE * scale
    label_w = text_width("SUMMER", 1) + 12
    cv = Canvas(label_w + len(columns) * (tile + 6) + 8, 20 + len(variants) * (tile + 22) + 8)
    cv.rect(0, 0, cv.w - 1, cv.h - 1, rgb("#14171A"))
    draw_text(cv, 6, 6, "SEASON VARIANTS 32PX X3", rgb("#E4C078"), 1)
    y = 20
    for season, tileset in variants:
        draw_text(cv, 6, y + 8, season.upper(), rgb("#A9C28F"), 1)
        for index, column in enumerate(columns):
            x = label_w + index * (tile + 6)
            draw_text(cv, x, y, labels[index], rgb("#A89F8A"), 1)
            cv.rect(x - 1, y + 10, x + tile, y + 10 + tile, rgb("#262C33"))
            tile_canvas = Canvas(TILE, TILE)
            for ty in range(TILE):
                for tx in range(TILE):
                    tile_canvas.set(tx, ty, tileset.get(column * TILE + tx, ty))
            cv.blit(tile_canvas.scaled(scale), x, y + 10)
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


def check():
    from pixel_kit import decode_png

    issues = []
    for base_path in (MANOR, VILLAGE):
        stem = os.path.splitext(base_path)[0]
        for season in SEASONS:
            path = f"{stem}_{season}.png"
            if not os.path.exists(path):
                issues.append("missing " + path)
                continue
            width, height, _, _ = decode_png(path)
            if (width, height) != (TILE * 6, TILE):
                issues.append(f"{path}: {width}x{height} != 192x32")
    if not os.path.exists(os.path.join("assets", "art", "tilesets", "season_preview.png")):
        issues.append("missing season_preview.png")
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
