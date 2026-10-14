# -*- coding: utf-8 -*-
"""《江湖有田》UI / 物品图标普查（只读数、不写盘），Python 3.9+ 标准库。

用法（工程根目录 examples/game_jhyt 下运行）：

    python -B -X utf8 tools/ui_asset_census.py            # 人读格式
    python -B -X utf8 tools/ui_asset_census.py --json      # 机读
    python -B -X utf8 tools/ui_asset_census.py --self-test  # 量具自检

退出码：0 = 读数跑完（** findings 里的"已知债"不参与退出码**，见下）；2 = 输入/解码错误。
为什么不把债判红：本工具今天交付的是**计数**，不是判据——"92 张图标里 64 张的出处不在
`gen_item_icons.py`"与"manifest 的 98 条 path 一条都不在盘上"都是要登记的债，把它们做成
退出码就等于把已知红入库（本批一路的口径）。债入门要等修法落地后再起，见
`tasks/done/M72-10` 的"普查入门"那一格。

三组读数，各自都能单独复跑：

  1. **像素档**：逐张解 RGBA，数不透明像素、不同 RGBA 档数（全图与"仅不透明"两个口径
     都打，差一档就是那张透明背景）、单例色占比、色彩复用率、alpha 取值集合、
     每通道档数上限与 6bit 量化是否达标（两条候选读法各打一遍，见 `pixel_row` 的注释）。
  2. **出处档**：`assets/art/items/` 的每张是否与 `gen_item_icons.ICONS` 的清单对得上——
     对得上的是"这条代码能重画"的，对不上的是工具外产物（AI 生成/一次性工具）。
  3. **登记档**：`assets/art/ui/manifest.json` 的 `assets[]` 条目与盘上 PNG 互相点名
     （条目 path 命中数、盘上未登记数），九宫格边距在**清单 / `scripts/ui/skin.lua` /
     `assets/prefabs/*.prefab`** 三处各写一遍的对拍，以及 `scripts/`、`scenes/`、
     `assets/prefabs/` 的引用面。
"""

from __future__ import annotations

import argparse
import ast
import json
import re
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))

from pixel_kit import decode_png  # noqa: E402

ITEMS_DIR = ROOT / "assets" / "art" / "items"
UI_DIR = ROOT / "assets" / "art" / "ui"
MANIFEST = UI_DIR / "manifest.json"
# manifest 的 root 段是 `assets/art/ui/jianghu`，盘上没有 jianghu 这一层（见登记档读数）。
STALE_SEGMENT = "assets/art/ui/jianghu/"
ICON_SIZES = ((32, 32), (64, 64), (40, 40), (48, 48), (24, 24), (16, 16))


def pixels(path):
    """返回逐像素 RGBA 元组列表 + 不透明像素数。"""
    width, height, channels, raw = decode_png(path)
    px = []
    for i in range(0, width * height * channels, channels):
        if channels == 4:
            px.append((raw[i], raw[i + 1], raw[i + 2], raw[i + 3]))
        else:
            px.append((raw[i], raw[i + 1], raw[i + 2], 255))
    return width, height, px


def pixel_row(path, rel):
    width, height, px = pixels(path)
    colors = Counter(px)
    opaque = [c for c in px if c[3] > 0]
    opaque_colors = Counter(opaque)
    singleton = sum(1 for count in opaque_colors.values() if count == 1)
    # `quantized_6bit` 的口径要钉死，两条候选读法在盘上给出**相反**的结论：
    #   甲 "每通道不同取值 ≤64 档"（=6bit 能表达的档数）→ 92/92 达标；
    #   乙 "每通道取值都是 4 的倍数"（自适应调色板会把档位落在非对齐位置）→ 0/92。
    # docs/58 §6 那句"每通道 6bit 颜色量化"说的是档数，所以取甲；乙留在 `aligned_4`
    # 这个字段里，免得下一个人把"0 命中"重新当成一条违约读数。
    levels = [len({c[channel] for c in opaque}) for channel in range(3)]
    return dict(
        path=rel,
        size=f"{width}x{height}",
        pixels=len(px),
        opaque_px=len(opaque),
        distinct_all=len(colors),
        distinct_opaque=len(opaque_colors),
        singleton_share=round(singleton / len(opaque), 4) if opaque else None,
        reuse=round(len(opaque) / len(opaque_colors), 3) if opaque_colors else None,
        alpha_values=sorted({c[3] for c in px}),
        levels_per_channel_max=max(levels) if levels else 0,
        quantized_6bit=bool(opaque) and max(levels) <= 64,
        aligned_4=all(value % 4 == 0 for color in colors for value in color[:3]),
    )


def reference_text():
    """把引用面拼成一段文本（scripts/**.lua、scenes/*.scene、assets/prefabs/*.prefab）。"""
    parts = []
    for pattern in ("scripts/**/*.lua", "scenes/*.scene", "assets/prefabs/*.prefab"):
        for path in sorted(ROOT.glob(pattern)):
            parts.append(path.read_text(encoding="utf-8", errors="replace"))
    return "\n".join(parts)


def literal_refs(text, prefix):
    """全路径字面量（不要求引号——prefab/scene 写的是 `texture=assets/…png;`）。

    口径边界：这是**文本面出现**，含注释与校验用的模式串，不是运行期可达性证明。
    """
    return set(re.findall("(" + re.escape(prefix) + r"[\w./-]+\.png)", text))


def concatenated_refs(text, prefix):
    """Lua 拼接式登记（`DIR .. "common/x.png"`，DIR = "assets/art/ui/"）。

    不认这一档，ui 的引用面会读成 6 张而不是实际张数——`scripts/ui/skin.lua` 的整张
    `M.tex` 表（90 条）都写成拼接式，字面量扫描对它全瞎。只收**拼出来确实在盘上**的，
    免得把别处目录的同形状拼接（`portrait.lua`、`items_fish.lua` 各一条）算进这条资产面。
    """
    found = set()
    for tail in set(re.findall(r'\.\.\s*"([\w./-]+\.png)"', text)):
        candidate = prefix + tail
        if (ROOT / candidate).is_file():
            found.add(candidate)
    return found


def generator_icons():
    """读 gen_item_icons.py 的 ICONS 清单（AST，不执行该模块）。

    不 import 是因为它会连带 import gen_manor_art 并在模块级画一批东西；普查工具
    只该读盘，不该有出图副作用。
    """
    source = (ROOT / "tools" / "gen_item_icons.py").read_text(encoding="utf-8")
    tree = ast.parse(source)
    names = []
    for node in tree.body:
        if not isinstance(node, ast.Assign):
            continue
        for target in node.targets:
            if isinstance(target, ast.Name) and target.id == "ICONS":
                for element in node.value.elts:
                    if isinstance(element, ast.Tuple) and element.elts:
                        first = element.elts[0]
                        if isinstance(first, ast.Constant) and isinstance(first.value, str):
                            names.append(first.value)
    return names


def census_items():
    files = sorted(p for p in ITEMS_DIR.glob("*.png"))
    rows = [pixel_row(p, p.relative_to(ROOT).as_posix()) for p in files]
    icons = [row for row in rows if row["size"] in {f"{w}x{h}" for w, h in ICON_SIZES}]
    return rows, icons


def census_ui():
    files = sorted(p for p in UI_DIR.rglob("*.png"))
    return [pixel_row(p, p.relative_to(ROOT).as_posix()) for p in files]


def census_manifest():
    data = json.loads(MANIFEST.read_text(encoding="utf-8"))
    assets = data.get("assets", [])
    as_written = 0
    stripped = 0
    registered = set()
    margins = {}
    for entry in assets:
        path = entry.get("path", "")
        if (ROOT / path).is_file():
            as_written += 1
        fallback = path.replace(STALE_SEGMENT, "assets/art/ui/")
        if (ROOT / fallback).is_file():
            stripped += 1
            registered.add(fallback)
        # `nine_slice` 这个**键**在 98 条上都有，但 71 条的值是 `None`——只数键的存在
        # 会把"清单里每条都是九宫格"读成一个不存在的事实。这里只收值是四整数列表的。
        declared = entry.get("nine_slice")
        if isinstance(declared, list) and len(declared) == 4:
            margins[fallback] = tuple(int(value) for value in declared)
    disk = {p.relative_to(ROOT).as_posix() for p in UI_DIR.rglob("*.png")}
    return dict(
        root_field=data.get("root"),
        entries=len(assets),
        nine_slice_keys=sum(1 for entry in assets if "nine_slice" in entry),
        nine_slice_declared=len(margins),
        nine_slice_null=len(assets) - len(margins),
        path_hits_as_written=as_written,
        path_hits_after_stripping=stripped,
        disk_png=len(disk),
        disk_unregistered=sorted(disk - registered),
        registered=sorted(registered),
        margin_paths=sorted(margins),
        margins={key: list(value) for key, value in margins.items()},
    )


def census_skin():
    """`scripts/ui/skin.lua` 的 `M.tex`：路径 + 硬写的 `slice` 边距。"""
    source = (ROOT / "scripts" / "ui" / "skin.lua").read_text(encoding="utf-8")
    tex = {}
    for match in re.finditer(
        r'\w+\s*=\s*\{\s*path\s*=\s*DIR\s*\.\.\s*"([\w./-]+\.png)"([^}]*)\}', source
    ):
        slice_match = re.search(r'slice\s*=\s*"(\d+),(\d+),(\d+),(\d+)"', match.group(2))
        tex["assets/art/ui/" + match.group(1)] = (
            tuple(int(value) for value in slice_match.groups()) if slice_match else None
        )
    return tex


def census_prefab_margins():
    """prefab 侧的 `nine_slice=` 声明（第三份边距）。"""
    found = {}
    for path in sorted(ROOT.glob("assets/prefabs/*.prefab")):
        for match in re.finditer(
            r"texture=(assets/art/ui/[\w./-]+\.png)[^\n]*?nine_slice=(\d+),(\d+),(\d+),(\d+)",
            path.read_text(encoding="utf-8", errors="replace"),
        ):
            found.setdefault(match.group(1), set()).add(
                tuple(int(value) for value in match.groups()[1:5])
            )
    return {key: sorted(values) for key, values in found.items()}


def summarize(rows, icons):
    if not icons:
        raise ValueError("没有图标可普查（rows=" + str(len(rows)) + "）——资产目录读错了")

    def median(values):
        values = sorted(values)
        if not values:
            return None
        mid = len(values) // 2
        return values[mid] if len(values) % 2 else round((values[mid - 1] + values[mid]) / 2, 3)

    alpha = Counter()
    for row in rows:
        alpha[tuple(row["alpha_values"])] += 1
    alpha_sets = {("/".join(str(value) for value in key) or "empty"): count
                  for key, count in alpha.items()}
    return dict(
        files=len(rows),
        icons=len(icons),
        flat_block_le2=sum(1 for row in icons if row["distinct_opaque"] <= 2),
        distinct_all_min=min(row["distinct_all"] for row in icons),
        distinct_all_median=median([row["distinct_all"] for row in icons]),
        distinct_opaque_min=min(row["distinct_opaque"] for row in icons),
        reuse_median=median([row["reuse"] for row in icons if row["reuse"]]),
        levels_max_median=median([row["levels_per_channel_max"] for row in icons]),
        quantized_6bit_hits=sum(1 for row in icons if row["quantized_6bit"]),
        aligned_4_hits=sum(1 for row in icons if row["aligned_4"]),
        alpha_value_sets=alpha_sets,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()

    text = reference_text()
    item_rows, item_icons = census_items()
    ui_rows = census_ui()
    owned = set(generator_icons())
    item_paths = [row["path"] for row in item_rows]
    refs_items = sorted(literal_refs(text, "assets/art/items/"))
    refs_ui = sorted(literal_refs(text, "assets/art/ui/")
                     | concatenated_refs(text, "assets/art/ui/"))

    def group(rows_list):
        values = sorted(row["reuse"] for row in rows_list if row["reuse"])
        if not values:
            return dict(n=0)
        mid = len(values) // 2
        median = values[mid] if len(values) % 2 else round((values[mid - 1] + values[mid]) / 2, 3)
        return dict(n=len(values), reuse_median=median,
                    flat_block_le2=sum(1 for row in rows_list if row["distinct_opaque"] <= 2),
                    quantized_6bit_hits=sum(1 for row in rows_list if row["quantized_6bit"]))

    owned_rows = [row for row in item_icons if Path(row["path"]).stem in owned]
    foreign_rows = [row for row in item_icons if Path(row["path"]).stem not in owned]

    # 边距（九宫格 `nine_slice`）目前有**三处各自写一遍**：清单、Lua 换肤表、prefab 组件。
    # 三处一比对就能读出"哪一条声明根本没被消费"，这是 M72-10 边距契约那一片的题面。
    manifest = census_manifest()
    skin = census_skin()
    prefabs = census_prefab_margins()
    margin_declared = set(manifest["margin_paths"])
    skin_sliced = {key for key, value in skin.items() if value}
    margins = dict(
        manifest_declared=len(margin_declared),
        skin_entries=len(skin),
        skin_sliced=len(skin_sliced),
        prefab_sliced=len(prefabs),
        declared_not_consumed=sorted(margin_declared - skin_sliced - set(prefabs)),
        skin_sliced_not_declared=sorted(skin_sliced - margin_declared),
        prefab_values_differ=sorted(
            path for path in prefabs if path in margin_declared and tuple(prefabs[path][0]) !=
            tuple(manifest["margins"][path])
        ),
    )

    report = dict(
        schema_version=1,
        items=dict(
            summary=summarize(item_rows, item_icons),
            owned=group(owned_rows),
            foreign=group(foreign_rows),
            referenced=len(refs_items),
            referenced_missing=[p for p in refs_items if not (ROOT / p).is_file()],
            unreferenced=[p for p in item_paths if p not in refs_items
                          and Path(p).name != "_preview.png"],
        ),
        ui=dict(
            summary=summarize(ui_rows, ui_rows),
            referenced=len(refs_ui),
            referenced_missing=[p for p in refs_ui if not (ROOT / p).is_file()],
            unreferenced=[row["path"] for row in ui_rows if row["path"] not in refs_ui],
            manifest=manifest,
            margins=margins,
        ),
        rows=dict(items=item_rows, ui=ui_rows),
    )

    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
        return 0

    items, ui = report["items"], report["ui"]
    print(f"items 文件 {items['summary']['files']}（其中图标 {items['summary']['icons']}）"
          f"| 引用 {items['referenced']} 缺 {len(items['referenced_missing'])}"
          f" 未引用 {len(items['unreferenced'])}"
          f" | 出处：工具可重画 {items['owned']['n']} / 工具外 {items['foreign']['n']}")
    print("  纯色块（非背景 RGBA ≤2）命中 "
          f"{items['summary']['flat_block_le2']} 张"
          f" | RGBA 档数 最少 {items['summary']['distinct_all_min']}"
          f" 中位 {items['summary']['distinct_all_median']}"
          f" | 仅不透明口径最少 {items['summary']['distinct_opaque_min']}"
          f" | 复用率中位 {items['summary']['reuse_median']}"
          f"（工具可重画 {items['owned']['reuse_median']}"
          f" vs 工具外 {items['foreign']['reuse_median']}）"
          f" | 6bit 量化（甲：每通道 ≤64 档）达标 {items['summary']['quantized_6bit_hits']}"
          f"，档位上限中位 {items['summary']['levels_max_median']}"
          f" | 乙：取值全为 4 的倍数 达标 {items['summary']['aligned_4_hits']}"
          f" | alpha 取值集合 {items['summary']['alpha_value_sets']}")
    print(f"ui 文件 {ui['summary']['files']} | 引用 {ui['referenced']}"
          f" 缺 {len(ui['referenced_missing'])} 未引用 {len(ui['unreferenced'])}"
          f" | 纯色块命中 {ui['summary']['flat_block_le2']}")
    manifest = ui["manifest"]
    print(f"  manifest：root={manifest['root_field']} 条目 {manifest['entries']}"
          f"（nine_slice 键 {manifest['nine_slice_keys']}："
          f"值为四元组 {manifest['nine_slice_declared']} / 为 null {manifest['nine_slice_null']}）"
          f"| path 原样命中 {manifest['path_hits_as_written']}"
          f" | 去掉 `{STALE_SEGMENT}` 段后命中 {manifest['path_hits_after_stripping']}"
          f" | 盘上 PNG {manifest['disk_png']} 未登记 {len(manifest['disk_unregistered'])}")
    margins = ui["margins"]
    print(f"  边距三处：清单声明 {margins['manifest_declared']}"
          f" | skin.lua 条目 {margins['skin_entries']}（带 slice {margins['skin_sliced']}）"
          f" | prefab 组件带 nine_slice {margins['prefab_sliced']}"
          f" | 声明了却两处都不消费 {len(margins['declared_not_consumed'])}"
          f" | skin.lua 有 slice 而清单没声明 {len(margins['skin_sliced_not_declared'])}"
          f" | 清单与 prefab 取值不一致 {len(margins['prefab_values_differ'])}")
    if margins["declared_not_consumed"]:
        print("    声明未消费：" + ", ".join(margins["declared_not_consumed"]))
    if manifest["disk_unregistered"]:
        print("    未登记：" + ", ".join(manifest["disk_unregistered"]))
    gradient = [row for row in report["rows"]["items"] if row["reuse"]]
    worst = sorted(gradient, key=lambda row: row["reuse"])[:8]
    print("  复用率最低八张（≈每像素一色）："
          + ", ".join(f"{Path(row['path']).stem}:{row['reuse']}" for row in worst))
    return 0


def self_test():
    """量具自检：钉住三个会静默走偏的口径。"""
    flat = Counter([(10, 20, 30, 255)] * 5 + [(0, 0, 0, 0)] * 3)
    assert sum(1 for c in flat if c[3] > 0) == 1
    # ① "仅不透明"与"全图"两口径必须差一档（背景那一档只在 all 里被数到）
    assert len(flat) == 2 and len({c for c in flat if c[3] > 0}) == 1
    # ② manifest 的旧前缀只该被剥一次，且剥完才是盘上路径
    stale = "assets/art/ui/jianghu/common/ui_panel_main_9s.png"
    stripped = stale.replace(STALE_SEGMENT, "assets/art/ui/")
    assert stripped == "assets/art/ui/common/ui_panel_main_9s.png"
    assert (ROOT / stripped).is_file(), stripped
    # ③ ICONS 清单是 AST 读的，不是 import 的：条数与结构必须对得上
    names = generator_icons()
    assert len(names) == len(set(names)) and len(names) >= 20, len(names)
    # ④ 6bit 两条读法在盘上真的相反（rice.png = 生成器自己画的那张）：档数达标、
    #    取值不对齐。钉住这条是防下一次有人把"0 命中"当成违约读数。
    rice = pixel_row(ITEMS_DIR / "rice.png", "assets/art/items/rice.png")
    assert rice["quantized_6bit"] and not rice["aligned_4"], rice
    assert rice["levels_per_channel_max"] <= 64 and rice["distinct_opaque"] > 100, rice
    # ⑤ `nine_slice` 这个**键**每条都有，值多数是 `None`：只数键会把清单读成"98 张全是
    #    九宫格"。钉住"键数 > 声明数"，防这条读数被下一次改动悄悄抹平。
    man = census_manifest()
    assert man["nine_slice_keys"] == man["entries"], man
    assert man["nine_slice_keys"] > man["nine_slice_declared"], man
    print(f"PASS: 两口径差一档 / 旧前缀只剥一次且落盘 / ICONS 走 AST 读到 {len(names)} 条"
          f" / 6bit 甲乙两读法相反（rice 档数上限 {rice['levels_per_channel_max']}）"
          f" / nine_slice 键 {man['nine_slice_keys']} ≠ 声明 {man['nine_slice_declared']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
