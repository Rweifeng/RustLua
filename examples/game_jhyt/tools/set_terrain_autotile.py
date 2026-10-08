# -*- coding: utf-8 -*-
"""把玩法地图的**地表层**升到逻辑瓦：autotile 载荷 + 犬牙交界（M72-04）。

用法（工程根目录 `examples/game_jhyt` 下）：

    python tools/set_terrain_autotile.py scenes/manor.scene --freeze 14,4,33,13
    python tools/set_terrain_autotile.py scenes/manor.scene --freeze 14,4,33,13 --apply
    python tools/set_terrain_autotile.py scenes/combat_arena.scene --identity --apply

为什么是"就地改一张已存在的 .scene"而不是重跑 `gen_manor_scene.py`：盘上的 `manor.scene`
早就和生成器分叉了（建筑/道具/HUD 是之后手工调的），重跑生成器会把那批改动抹掉。
本工具**只碰地表层那一行 `component`**，其余字节原样写回。

两件事：

1. **换载荷**：`tiles` 里存的从"图集条目号"变成"逻辑 kind id"（1..6 一个没动，因为
   旧图集恰好是 6 列、id 与 kind 同号），另加 `tile_kinds=6;variant_count=1;autotile=1`
   与 `atlas_cols=12;atlas_rows=9`。字段顺序与引擎 `Tilemap::to_component_str` 逐字符一致，
   否则"生成器产物 vs 引擎回写产物"对拍会出现假差异。
2. **抖交界**：直线交界是"像 demo"的头号成因（docs/71 §1 第 2 行）。把**草/土**格子按
   确定性哈希改成邻居的 kind（两遍，1-2 格深的犬牙），且只接受 `solid` 位相同的邻居，因此：
   * `solid` 串一个 bit 都不动 → 碰撞、寻路、出口、places 全不受影响；
   * 墙（4）、瓦（5）、水（6）永不被改写；石板路（3）**只加不减**（只允许别的格子变成路），
     所以 `building.lua` 的"tile==3 判道路"不会因为重排而断；
   * 农田矩形（`--freeze`，来自 `scripts/systems/farm.lua` 的 `FARM`）整块冻住：
     `farm.lua` 用 `tilemap.get_cell(ground, cx, cy) == 2` 判可开垦，抖掉一格就少一格田。
   代价是草↔土石互换会让建筑可放置格 ±1（`buildings.lua` 的 `ground_tiles={1,3}`），
   这是地表重排本身应有的效果，不是回归。
3. **撒 tint**：写 `tints=` + `tints_rgb=`（调色表来自 `terrain_ramp.json`）。autotile 开着
   时引擎不查 `variant_for`，四邻皆同类的内部格全部落进同一个图集条目（实测 920 格里
   185 格同条目），所以"整片平涂"要靠顶点色乘法压下去——这是 M72-02 已接好、数据侧
   就能用的那根杠杆，不是新机制。

幂等：已经带 `autotile=1` 的层只重排字段、不再抖第二遍（抖动的输入是"直接寻址时代"的
格子图，重跑会越抖越糊）；tint 撒布是纯哈希，重跑逐字节相同。

依赖：Python 3 标准库 + 同目录 `pixel_kit.py`、`terrain_kit.py`。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import terrain_kit as tk  # noqa: E402
from pixel_kit import hash_stream  # noqa: E402

GROUND_TEXTURE = "assets/art/tilesets/manor/manor_ground.png"
# 两遍：第一遍把交界改成邻居的 kind（约 60%），第二遍以第一遍的结果为输入再推一层（约 30%），
# 于是交界是 1-2 格深的犬牙而不是一条"抖了一格"的直线。
JITTER_PASSES = (60, 30)
JITTER_TARGETS = (1, 2)  # 只有草/土会被改写
JITTER_DONORS = (1, 2, 3)  # 来源可以是草/土/石板：**只加路不减路**
TINT_PATCH = 2  # tint 按 2×2 格块撒（每格独立会跳成噪点，见 `scatter_tints`）
NEIGHBOR_ORDER = ((1, 0), (-1, 0), (0, 1), (0, -1))


def find_ground(lines, texture=GROUND_TEXTURE):
    """返回 (行号, 组件串)。只认地表贴图那一条，别的层（农田/湿润）不碰。"""
    hits = []
    for index, line in enumerate(lines):
        fields = line.split("\t")
        if len(fields) == 4 and fields[0] == "component" and fields[2] == "Tilemap" \
                and f"texture={texture}" in fields[3]:
            hits.append((index, fields))
    if len(hits) != 1:
        raise SystemExit(f"[FAIL] 期望恰好一条 {texture} 的 Tilemap，实到 {len(hits)} 条")
    return hits[0]


def parse_fields(value):
    return dict(part.split("=", 1) for part in value.split(";") if "=" in part)


def read_cells(fields):
    fmt = int(fields.get("tiles_format", "0") or 0)
    raw = fields["tiles"]
    if fmt == 0:
        return [int(ch) for ch in raw]
    return [int(part) for part in raw.split(",") if part.strip() != ""]


def jitter_pass(cells, solid, width, height, freeze, odds, rand):
    """一遍交界抖动：只改 `JITTER_TARGETS` 里的格子，来源限于 `JITTER_DONORS` 的邻居。

    每格先取一个定值再判候选，所以候选方向的存在与否不会让整图的随机序列重排
    （那会让"多抖一格"变成"全图重排"，review 时看不出改了什么）。
    """
    out = list(cells)
    changed = 0
    for y in range(height):
        for x in range(width):
            roll = rand(100)
            index = y * width + x
            kind = cells[index]
            if kind not in JITTER_TARGETS or solid[index] != "0" or (x, y) in freeze:
                continue
            for dx, dy in NEIGHBOR_ORDER:
                nx, ny = x + dx, y + dy
                if nx < 0 or ny < 0 or nx >= width or ny >= height:
                    continue
                n_index = ny * width + nx
                n_kind = cells[n_index]
                if n_kind not in JITTER_DONORS or n_kind == kind:
                    continue
                if solid[n_index] != solid[index]:
                    continue
                if roll < odds:
                    out[index] = n_kind
                    changed += 1
                break
    return out, changed


def jitter(cells, solid, width, height, freeze):
    """草/土/石之间把直线交界抖成犬牙；墙、瓦、水与所有 solid 位一个都不动。"""
    out = list(cells)
    rand = hash_stream(0x7E440)
    total = 0
    for odds in JITTER_PASSES:
        out, changed = jitter_pass(out, solid, width, height, freeze, odds, rand)
        total += changed
    return out, total


def scatter_tints(width, height, tint_ids):
    """按 2×2 格块撒 per-tile tint：把"整片同一个格子"压成大块明度差，而不是每格独立抖动。

    为什么要在这里撒：autotile 开着时引擎**不查** `variant_for`（见 `tilemap.rs` 的
    `resolve_atlas_index` 分支），所以四邻皆同类的内部格全部落在同一个图集条目上——
    实测山庄 920 格里 185 格是同一个条目。变体机制对内部格是空的，而 tint 是 M72-02
    已经接好、数据侧就能用的另一根杠杆（docs/53 §2："同族 3 档明度，不是一块平涂"）。

    为什么按块而不是按格：docs/53 §1.2 的"大面积底色保持低频"——每格独立随机会在屏幕上
    跳成噪点（32px 一格、缩放 2 倍就是 64px 的骰子面），2×2 块才读作"这块地被晒到/在阴里"。
    """
    tints = {}
    for by in range(0, height, TINT_PATCH):
        for bx in range(0, width, TINT_PATCH):
            roll = hash_stream(73856093 * (bx + 1) ^ 19349663 * (by + 1))(100)
            if roll >= 60:
                continue  # 白档：不写，稀疏编码省字
            tint_id = tint_ids[0] if roll < 30 else tint_ids[-1]
            for y in range(by, min(height, by + TINT_PATCH)):
                for x in range(bx, min(width, bx + TINT_PATCH)):
                    tints[y * width + x] = tint_id
    return tints


def build_component(fields, cells, tint_ids_by_index, autotile, table):
    """按引擎 `to_component_str` 的字段顺序重排一行组件串（拼串在 `terrain_kit` 里，三个生成器共用）。"""
    return tk.tilemap_component(
        int(fields["tile_size"].split("x")[0]),
        fields["width"],
        fields["height"],
        fields["texture"],
        cells,
        fields["solid"],
        table["cols"],
        table["rows"],
        tile_kinds=table["kind_count"] if autotile else 0,
        tints=tint_ids_by_index,
        tints_rgb=tk.tints_rgb_string(table) if tint_ids_by_index else "",
    )


def parse_freeze(values):
    rects = []
    for value in values:
        x0, y0, x1, y1 = (int(part) for part in value.split(","))
        rects.append((x0, y0, x1, y1))
    return rects


def freeze_cells(rects, width, height):
    cells = set()
    for x0, y0, x1, y1 in rects:
        for y in range(max(0, y0), min(height, y1 + 1)):
            for x in range(max(0, x0), min(width, x1 + 1)):
                cells.add((x, y))
    return cells


def main():
    parser = argparse.ArgumentParser(description="地表层升到逻辑瓦（M72-04）")
    parser.add_argument("scene")
    parser.add_argument("--apply", action="store_true", help="写回文件（默认只报告）")
    parser.add_argument("--identity", action="store_true",
                        help="只改图集行列（恒等寻址，不开 autotile）——arena 复用同一张图时用")
    parser.add_argument("--freeze", action="append", default=[],
                        help="冻结矩形 x0,y0,x1,y1（可重复）：农田区必须冻")
    args = parser.parse_args()

    table = tk.load_ramp()
    with open(args.scene, encoding="utf-8", newline="") as handle:
        content = handle.read()
    newline = "\r\n" if "\r\n" in content else "\n"
    lines = content.replace("\r\n", "\n").split("\n")

    index, fields_line = find_ground(lines)
    fields = parse_fields(fields_line[3])
    width, height = int(fields["width"]), int(fields["height"])
    solid = fields["solid"]
    cells = read_cells(fields)
    if len(cells) != width * height or len(solid) != width * height:
        raise SystemExit(f"[FAIL] tiles/solid 长度对不上 {width}x{height}")
    already = fields.get("autotile", "") in ("1", "true")
    autotile = not args.identity

    if autotile and not already:
        cells, changed = jitter(cells, solid, width, height,
                                freeze_cells(parse_freeze(args.freeze), width, height))
    else:
        changed = 0

    tints = {}
    if autotile:
        tint_ids = [tint_id for tint_id, _ in tk.ground_tints(table)]
        if len(tint_ids) < 2:
            raise SystemExit("[FAIL] terrain_ramp.json 的 ground_tints 少于 2 档，撒不出明度差")
        tints = scatter_tints(width, height, tint_ids)

    for value in cells:
        if value < 1 or value > table["kind_count"]:
            raise SystemExit(f"[FAIL] 格子里有 kind {value}，超出 tile_kinds={table['kind_count']}")

    component = build_component(fields, cells, tints, autotile, table)
    lines[index] = "\t".join([fields_line[0], fields_line[1], fields_line[2], component])
    histogram = {}
    for value in cells:
        histogram[value] = histogram.get(value, 0) + 1
    print(f"{args.scene}: {'autotile' if autotile else 'identity'} "
          f"atlas={table['cols']}x{table['rows']} 抖动 {changed} 格 tint {len(tints)} 格 "
          f"kinds={histogram}")
    if args.apply:
        with open(args.scene, "w", encoding="utf-8", newline="") as handle:
            handle.write(newline.join(lines))
        print("  applied")
    else:
        print("  dry-run（加 --apply 才写盘）")


if __name__ == "__main__":
    main()
