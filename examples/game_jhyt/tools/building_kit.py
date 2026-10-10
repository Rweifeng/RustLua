# -*- coding: utf-8 -*-
"""建筑/道具**占位与碰撞**的单一事实源（M72-07）。

一句话：**每一格 `solid` 都必须有可视依据**——要么那一格的地表本身就是水/墙（瓦自己画的），
要么有一个 sprite 的**不透明像素**压在那一格上。除此之外一律不许实心。

为什么按"不透明像素"而不是按 quad 框：sprite 的 quad 是矩形，而建筑贴图的 quad 四角
几乎总是全透明（屋顶是梯形、檐口是斜的）。按框算会把"看得见的是墙脚、看不见的是空气"
那几格也标成实心，玩家就会在屋外两步远的地方撞空气——这正是本包要消灭的症状。

三类实体三种判法（`53` §3.5 第 5 条在建筑侧的展开，理由写进 M72-07 计划的设计要点）：

  * **建筑**（`role=building` / `building_site`，本体在 `world` 层）：视觉多大就撞多大
    ——本体 quad 与屋顶（`foreground` 遮断件）quad 覆盖的格并集，再挖掉**门洞**。
    层秩 `world(-1) < actor(+1)`（`crates/rustlua-render/src/sprite.rs:105`），
    所以墙永远遮不住站在它后面的人：那些格不实心，玩家就会走进墙里并被画在墙上。
  * **树/竹/单件道具**（`role=prop`）：**单格根碰撞**——只实心 `entity_meta cell` 那一格，
    树冠/竹梢那几格由 M72-06 的前景遮断件负责盖人，不该挡人。
  * **装饰**（`role=decor`）：不参与碰撞（M72-05 定的，`jhyt_m72_scatter_decor` §⑤ 在守）。

**门洞**（`OPENINGS`）是唯一一处"手填"的数据，所以它自带一条反查：声明是洞，
贴图在那几格上就必须**真是洞**（不透明占比 ≤ `HOLE_MAX_OPAQUE_RATIO`），
而且洞必须与外面可走的地格连通。两边任何一边改了都会当场红，不会静默穿模。

用法（工程根目录 `examples/game_jhyt` 下）：

    python tools/building_kit.py scenes/manor.scene            # 只报告（差多少格）
    python tools/building_kit.py scenes/manor.scene --check    # 有差就退出码 1
    python tools/building_kit.py scenes/manor.scene --apply    # 重写 GroundMap 的 solid 串

依赖：Python 3 标准库 + 同目录 `pixel_kit.py`（PNG 解码）。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import decode_png  # noqa: E402

TILE = 32
GROUND_TEXTURE = "assets/art/tilesets/manor/manor_ground.png"

# 地表 kind 编号（与 `gen_manor_scene.py` 的列映射、`data/buildings.lua` 的 ground_tiles 一致）。
GRASS, DIRT, STONE, WALL, ROOF, WATER = 1, 2, 3, 4, 5, 6
# 这两族的格**天然实心**：墙与人是瓦画出来的，没有对应的 sprite 实体。
TILE_NATIVE_SOLID = (WALL, WATER)

BUILDING_ROLES = ("building", "building_site")
ROOT_ROLES = ("prop",)

# 遮断件所在的层：它的 quad 也属于建筑的视觉框（见模块 docstring 的第一条判法）。
OCCLUDER_LAYER = "foreground"

# 门洞：实体名 -> 必须可走的绝对 cell。只有南门有洞（它是 `data/maps.lua`
# 山庄→云泽村出口格 (16,1) 所在，玩家从村里走进山庄要穿过它）。
OPENINGS = {
    "ManorGate": ((16, 1), (16, 2)),
}
# 声明为洞的格上，贴图不透明像素占比的上限。**只查每列最下面那一格**（人站着的那格）：
# 它上面那格是屋檐/匾额所在，本来就该画满（南门实测 0.91），要求它是洞等于要求门楼没有顶。
# 0.35 是量出来的：门洞那格重画前 0.94（匾额正好挂在通道上）、重画后 0.00，
# 而"看着有门其实实心"的写法（药庐那种棕色门板）在 0.86~1.00，中间隔着一条宽缝。
HOLE_MAX_OPAQUE_RATIO = 0.35

# 道具的碰撞模式：默认 `root`（单格根碰撞，树/竹/井/罐…），下面是两个例外——
# 它们本体就横着占两格，只标锚点那一格会让另一半"看得见却穿得过"。
# 盘上对拍：`box` 模式下这两件算出来的格与改前逐格相同（篱 (13,1)(13,2)、架 (11,10)(12,10)）。
PROP_COLLISION = {
    "ManorFence": "box",
    "ManorRack": "box",
}


def project_root():
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


# ---------------------------------------------------------------------------
# 场景读取
# ---------------------------------------------------------------------------


def _fields(component_line):
    parts = component_line.split("\t")
    out = {}
    for chunk in ";".join(parts[3:]).split(";"):
        if "=" in chunk:
            key, value = chunk.split("=", 1)
            out[key] = value
    return out


def read_entities(lines):
    """返回 {entity_id: {'name','role','cell','sprite','transform'}}，只收有 Sprite 的那些。"""
    ents = {}
    for line in lines:
        parts = line.split("\t")
        tag = parts[0]
        if tag == "entity":
            ents[int(parts[1])] = {"name": parts[2], "role": "", "cell": None,
                                   "sprite": None, "transform": None}
        elif tag == "entity_meta":
            ent = ents.get(int(parts[1]))
            if ent is None:
                continue
            if parts[2] == "role":
                ent["role"] = parts[3]
            elif parts[2] == "cell":
                x, y = parts[3].split(",")
                ent["cell"] = (int(x), int(y))
        elif tag == "component":
            ent = ents.get(int(parts[1]))
            if ent is None:
                continue
            kind = parts[2]
            if kind == "Sprite":
                ent["sprite"] = _fields(line)
            elif kind == "Transform":
                ent["transform"] = _fields(line)
    return ents


def placed_sprites(ents):
    """有 Sprite + Transform 的实体，换算成世界矩形（pivot=(0,0) 即 transform 为左下角）。"""
    out = []
    for eid, ent in sorted(ents.items()):
        sp, tr = ent["sprite"], ent["transform"]
        if not sp or not tr:
            continue
        width, height = (int(v) for v in sp["size"].split("x"))
        pos = [float(v) for v in tr["position"].split(",")]
        out.append({
            "id": eid, "name": ent["name"], "role": ent["role"], "cell": ent["cell"],
            "texture": sp["texture"], "layer": sp.get("sorting_layer", "default"),
            "order": int(sp.get("order", "0")),
            "x": pos[0], "y": pos[1], "w": width, "h": height,
        })
    return out


# ---------------------------------------------------------------------------
# 不透明像素的紧包围盒
# ---------------------------------------------------------------------------


def opaque_box(root, rel_path):
    """(w, h, (x0, y0, x1, y1))；包围盒在**图像坐标**（y 向下，顶行为 0）。全透明则 None。"""
    key = rel_path
    if not hasattr(opaque_box, "_cache"):
        opaque_box._cache = {}
    if key in opaque_box._cache:
        return opaque_box._cache[key]
    path = os.path.join(root, rel_path.replace("/", os.sep))
    width, height, channels, raw = decode_png(path)
    if channels < 4:
        value = (width, height, (0, 0, width, height))
        opaque_box._cache[key] = value
        return value
    alpha = raw[3::4]
    xs = [i % width for i, a in enumerate(alpha) if a > 0]
    if not xs:
        raise SystemExit(f"[FAIL] {rel_path} 整张全透明：它不该被摆进场景")
    ys = [i // width for i, a in enumerate(alpha) if a > 0]
    value = (width, height, (min(xs), min(ys), max(xs) + 1, max(ys) + 1))
    opaque_box._cache[key] = value
    return value


def opaque_world_rect(root, sprite):
    """不透明像素换算到世界矩形 (x0, y0, x1, y1)，y 向上。None = 全透明。"""
    width, height, (bx0, by0, bx1, by1) = opaque_box(root, sprite["texture"])
    # 图像 y 向下、世界 y 向上：图像顶行 by0 对应世界 y = sprite.y + (h - by0)
    return (sprite["x"] + bx0,
            sprite["y"] + (height - by1),
            sprite["x"] + bx1,
            sprite["y"] + (height - by0))


def cells_covered(rect):
    """与世界矩形有交集的所有 cell（开区间比较：只贴到边不算）。"""
    x0, y0, x1, y1 = rect
    out = set()
    for cx in range(int(x0 // TILE), int(-(-x1 // TILE))):
        if x1 <= cx * TILE:
            continue
        for cy in range(int(y0 // TILE), int(-(-y1 // TILE))):
            if y1 <= cy * TILE:
                continue
            out.add((cx, cy))
    return out


# ---------------------------------------------------------------------------
# 判据
# ---------------------------------------------------------------------------


def visual_and_solid(root, sprites, width, height):
    """返回 (应实心格, 每栋建筑的读数, 每件的可见格)。"""
    ents_by_name = {}
    for item in sprites:
        ents_by_name.setdefault(item["name"], []).append(item)

    solid = set()
    readings = []
    for item in sprites:
        if item["role"] not in BUILDING_ROLES:
            continue
        if item["layer"] == OCCLUDER_LAYER:
            continue  # 屋顶那一件在下面按名字配对
        box = opaque_world_rect(root, item)
        cells = cells_covered(box)
        # 同名拆层件：M72-06 把正房拆成 `ManorHouse`(world) + `ManorHouseRoof`(foreground
        # 遮断件)，视觉框是两件并集。配对规则是实体名前缀，**不按 role 筛**——
        # 屋顶那件的 role 是 `occluder`，按 role 筛会把它漏掉，那 21 格屋檐就会被判成
        # "悬空碰撞"而删掉实心（玩家能走进屋檐里）。`*Shadow` 是投影贴片，必须排除。
        mates = [other for other in sprites
                 if other is not item and other["role"] not in ("decor", "cast_shadow")
                 and other["name"].startswith(item["name"]) and other["name"] != item["name"]]
        for mate in mates:
            cells |= cells_covered(opaque_world_rect(root, mate))
        openings = set(OPENINGS.get(item["name"], ()))
        unknown = openings - cells
        if unknown:
            raise SystemExit(f"[FAIL] {item['name']} 声明的门洞 {sorted(unknown)} "
                             f"不在它的视觉框里：洞挪了位置而表没跟着改")
        solid |= cells - openings
        readings.append({"name": item["name"], "visual": len(cells), "solid": len(cells - openings),
                         "openings": sorted(openings), "mates": [m["name"] for m in mates]})
    for item in sprites:
        if item["role"] not in ROOT_ROLES:
            continue
        mode = PROP_COLLISION.get(item["name"], "root")
        if mode == "box":
            solid |= cells_covered(opaque_world_rect(root, item))
            continue
        if item["cell"] is None:
            raise SystemExit(f"[FAIL] {item['name']} 是 prop 却没有 entity_meta cell："
                             "单格根碰撞没有锚点可标")
        solid.add(item["cell"])
    clipped = {cell for cell in solid if 0 <= cell[0] < width and 0 <= cell[1] < height}
    return clipped, readings, ents_by_name


def ground_component(lines):
    """(行号, 字段 dict, width, height, kinds, solid 串)。"""
    hits = []
    for index, line in enumerate(lines):
        parts = line.split("\t")
        if len(parts) == 4 and parts[0] == "component" and parts[2] == "Tilemap" \
                and f"texture={GROUND_TEXTURE}" in parts[3]:
            hits.append((index, _fields(line)))
    if len(hits) != 1:
        raise SystemExit(f"[FAIL] 期望恰好一条 {GROUND_TEXTURE} 的 Tilemap，实到 {len(hits)} 条")
    index, fields = hits[0]
    width, height = (int(v) for v in fields["tile_size"].split("x"))
    width = int(fields["width"])
    height = int(fields["height"])
    kinds = [int(v) for v in fields["tiles"].split(",") if v.strip() != ""]
    solid = fields["solid"]
    if len(kinds) != width * height:
        raise SystemExit(f"[FAIL] tiles 串长度 {len(kinds)} != {width}x{height}")
    if len(solid) != width * height:
        raise SystemExit(f"[FAIL] solid 串长度 {len(solid)} != {width}x{height}")
    return index, fields, width, height, kinds, solid


def analyse(scene_path, root=None):
    root = root or project_root()
    lines = open(scene_path, encoding="utf-8").read().splitlines()
    index, _fields_, width, height, kinds, solid = ground_component(lines)
    sprites = placed_sprites(read_entities(lines))
    expected, readings, _ = visual_and_solid(root, sprites, width, height)
    on_disk = {(i % width, i // width) for i, ch in enumerate(solid) if ch == "1"}
    native = {(i % width, i // width) for i, k in enumerate(kinds) if k in TILE_NATIVE_SOLID}
    want = expected | native
    # native 与 expected 的差要分开报：墙/水那部分是"瓦自己画的"，不该被本工具改写。
    missing = sorted(want - on_disk)
    orphan = sorted(on_disk - want)
    return {"lines": lines, "row": index, "width": width, "height": height,
            "kinds": kinds, "on_disk": on_disk, "want": want, "native": native,
            "expected": expected, "readings": readings,
            "missing": missing, "orphan": orphan, "sprites": sprites}


def rewrite_solid(result):
    lines = list(result["lines"])
    width, height = result["width"], result["height"]
    chars = ["1" if (x, y) in result["want"] else "0"
             for y in range(height) for x in range(width)]
    parts = lines[result["row"]].split("\t")
    chunks = parts[3].split(";")
    for position, chunk in enumerate(chunks):
        if chunk.startswith("solid="):
            chunks[position] = "solid=" + "".join(chars)
    parts[3] = ";".join(chunks)
    lines[result["row"]] = "\t".join(parts)
    return lines


# ---------------------------------------------------------------------------
# 门洞反查：声明是洞，图上就必须真是洞
# ---------------------------------------------------------------------------


def opening_holes(scene_path, root=None):
    """逐个门洞格量"贴图在那一格上的不透明像素占比"。返回 [(实体, 格, 占比, 是否人站的那格)]。

    同一列里最低的那一格才是人站着穿过的那格（`stands_on=True`），只有它要求是洞；
    它上面那格是屋檐/匾额，本来就该画满。
    """
    root = root or project_root()
    lines = open(scene_path, encoding="utf-8").read().splitlines()
    sprites = placed_sprites(read_entities(lines))
    out = []
    for item in sprites:
        cells = OPENINGS.get(item["name"], ())
        if not cells:
            continue
        lowest = {}
        for (cx, cy) in cells:
            if cx not in lowest or cy < lowest[cx]:
                lowest[cx] = cy
        width, height, _box = opaque_box(root, item["texture"])
        alpha = decode_png(os.path.join(root, item["texture"].replace("/", os.sep)))[3][3::4]
        for (cx, cy) in sorted(cells):
            wx0, wy0 = cx * TILE, cy * TILE
            ix0 = max(0, int(wx0 - item["x"]))
            ix1 = min(width, int(wx0 + TILE - item["x"]))
            # 世界 y 向上、图像行 0 在顶：行 r 覆盖世界 y ∈ [y+h-r-1, y+h-r]
            # ⇒ 格 [wy0, wy0+TILE] 对应行区间 [y+h-wy0-TILE, y+h-wy0)。
            # （曾经写作 `h - wy0 - TILE - y`，只在 `y==0` 时碰巧对——南门正好贴在地图下边界
            #  y=0，所以盘上没人撞过它。下一个门洞若挂在 `y≠0` 的建筑上，量的是别的行。）
            iy0 = max(0, int(item["y"] + height - (wy0 + TILE)))
            iy1 = min(height, int(item["y"] + height - wy0))
            if ix1 <= ix0 or iy1 <= iy0:
                ratio = None
            else:
                total = (ix1 - ix0) * (iy1 - iy0)
                opaque = sum(1 for yy in range(iy0, iy1)
                             for xx in range(ix0, ix1) if alpha[yy * width + xx] > 0)
                ratio = opaque / total
            out.append((item["name"], (cx, cy), ratio, lowest[cx] == cy))
    return out


def check_openings(scene_path, root=None):
    problems = []
    for name, cell, ratio, stands_on in opening_holes(scene_path, root):
        if not stands_on:
            continue
        if ratio is None:
            problems.append(f"{name} 门洞 {cell} 落在贴图矩形之外")
        elif ratio > HOLE_MAX_OPAQUE_RATIO:
            problems.append(f"{name} 门洞 {cell} 的不透明占比 {ratio:.2f} > "
                            f"{HOLE_MAX_OPAQUE_RATIO}：声明是人穿过去的通道，图上却画满了")
    return problems


def main(argv):
    parser = argparse.ArgumentParser(description="建筑占位/碰撞对拍")
    parser.add_argument("scene")
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args(argv[1:])

    result = analyse(args.scene)
    for reading in result["readings"]:
        print("   %-20s 视觉 %-3d 格 solid %-3d 格 门洞 %s%s" % (
            reading["name"], reading["visual"], reading["solid"],
            reading["openings"] or "-",
            "（拆层件：" + ",".join(reading["mates"]) + "）" if reading["mates"] else ""))
    print("   盘上 solid=%d 应实心=%d 缺=%d 悬空=%d" % (
        len(result["on_disk"]), len(result["want"]),
        len(result["missing"]), len(result["orphan"])))
    if result["orphan"]:
        print("   悬空格（撞空气）：", result["orphan"])
    if result["missing"]:
        print("   缺碰撞格：", result["missing"])
    for problem in check_openings(args.scene):
        print("   [FAIL]", problem)

    if args.apply:
        lines = rewrite_solid(result)
        with open(args.scene, "w", encoding="utf-8", newline="\n") as handle:
            handle.write("\n".join(lines) + "\n")
        print(f"   applied -> {args.scene}")
    elif args.check and (result["missing"] or result["orphan"] or check_openings(args.scene)):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
