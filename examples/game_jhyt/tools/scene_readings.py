# -*- coding: utf-8 -*-
"""一张 `.scene` 的量化读数（M72-11 铺量用；只读，不改任何字节）。

用法（工程根目录 `examples/game_jhyt` 下）：

    python tools/scene_readings.py scenes/manor.scene
    python tools/scene_readings.py scenes/village.scene
    python tools/scene_readings.py scenes/village.scene --gate   # 两条判据变硬失败（verify 用）

为什么要单独一个读数工具，而不是靠肉眼或截图：`tasks/plan/M72-11` 的验收方式是
**逐图**量化（同屏可见瓦种类 ≥24、每屏可见对象 ≥40），七张图各要一组能复核的数，
而 `jhyt_m72_manor_terrain` 那道门按设计只管 manor（它的 §① 查的就是 manor 那张 12×9
图集的族色阶）。这里把同一套口径搬到 Python、搬到任意一张图。

两个数各自的口径都与既有的那扇门 / 那套散布**逐条对齐**，不是重新发明：

* **同屏可见瓦种类**：外观 = (图集条目, per-tile tint 档)。这是"屏幕上两片像素相同"的
  等价类（对应 Rust `Tilemap::resolve_atlas_index` + 顶点色乘法），寻址走
  `preview_scene.resolve_atlas_index` 那一份已对拍的实现。窗口取 **20×12 格**、扫所有
  可停靠位置取**最小值**——与 `jhyt_m72_manor_terrain.rs` 的 `SCREEN_W/SCREEN_H/MIN_SCREEN_TILES`
  同一组数（任何一屏都包含一个这样的窗口，所以读数只会更高，不会放水）。
* **每屏可见对象数**：矩形相交，窗口 **20×11.25 格**、每 1/4 格取一窗，报
  min/median/max —— 与 `decor_kit.window_counts` 共用同一个函数（不是抄一份），
  所以 manor 当年那组数与这里的数可以并排比。分两档报：只数装饰、数全部 sprite
  （建筑/道具/遮断件/投影都算），因为 plan 里的 ≥40 当年是按装饰那一档判的。

瓦是 32px 一格、sprite 的 quad 是像素，两者窗口不一样宽，所以各自用自己的那套步进，
不强行合成一个"对象"计数。
"""
import argparse
import os
import statistics
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import decor_kit as dk  # noqa: E402  每屏对象数：共用 `window_counts`
import preview_scene as ps  # noqa: E402  图集寻址 + 场景解析：共用已对拍的那一份
import terrain_kit as tk  # noqa: E402  D5 复核要按名字取色阶族与 tint 表

# 与 `jhyt_m72_manor_terrain.rs` 的常量同名同值（改那里要改这里，两边都是判据不是巧合）。
SCREEN_TILES_W, SCREEN_TILES_H = 20, 12
MIN_SCREEN_TILES = 24
# 对象数的窗口：`decor_kit.window_counts` 的默认档（zoom=2 的 1280×720 设计视口 =
# 20×11.25 格，docs/51 §2.2）。写出来是为了让"瓦窗口 20×12"与"对象窗口 20×11.25"
# 这两个不同口径在同一份文件里各自可见。
OBJECT_WINDOW = (20, 11.25)

# `--gate` 用的每图分档。**判 median 不判 min**——与 `jhyt_m72_scatter_decor` 同一口径
# （药田/茶园那类"按玩法契约必须留白"的地块会把最坏一屏压到接近空，min 判的是留白是否
# 故意，不是画面是否撑得住）。**按场景登记，不给默认值**：`tasks/plan/M72-11` §风险 明写
# "每屏对象 ≥40 在室内小地图上可能不合理（室内本来就该空）⇒ 指标要按场景类型分档"，
# 所以没登记的场景直接判"先登记再门"，而不是拿 40 去压一张洞穴图。登记时必须在旁边
# 写上这个数是怎么来的（户外按 plan 的 ≥40；室内要按分档另定）。
MIN_DECOR_OBJECTS = {
    "manor": 40,    # plan §验收方式：户外主场景 ≥40；实测 median 41（min 7 是药田留白那一屏）
    # 复现：`python tools/scene_readings.py scenes/<图>.scene` 读"每屏可见装饰"那一行。
    # village 实测 326 件、min/median/max = 58/94/128（没有整块留白的地块，所以 min 就高）。
    "village": 40,
    # mountain_path 实测 245 件、min/median/max = 49/80/106（M72-11 ④ 复跑同一命令重读）：
    # 竹林地（kind 5）在 `decor_kit.BASE_P` 里权重 0（竹叶画在瓦里），那是**故意**的少，不是漏撒。
    # 旧说法：登记时（48b4f496）写的是 253 件、45/84/109。新实测的证据是"生成器重跑 == 盘上场景"
    # （`python tools/gen_mountain_scene.py --out .temp/m.scene` 与 `scenes/mountain_path.scene`
    # 逐行相同，仅行尾 CRLF 差），所以 245 是这条管线现在的产物、253 是当时的中间产物；差因未追，
    # 按新读数更正而不是把两个数都留着。
    "mountain_path": 40,
    # combat_arena（M72-11 ④）实测 340 件、min/median/max = 70/96/133
    # （复现：`python tools/scene_readings.py scenes/combat_arena.scene`）。
    # 比 manor 高一倍不是撒多了：这张图没有整块留白的玩法地块（manor 的 min 7 就是药田那一屏），
    # min 70 是"坪内草地本来就少、装饰集中在坪外"的正常形状。分档仍取 plan §验收方式 的户外 ≥40，
    # 不在这里给本图单独抬高门槛——那条线管的是"管线跑没跑"，不是"这张图该多密"。
    "combat_arena": 40,
    # cold_spring_cave（M72-11 §推进顺序 4，第一张暗场景）实测 57 件、min/median/max = 57/57/57
    # （复现：`python tools/scene_readings.py scenes/cold_spring_cave.scene`）。
    # **三个数同形不是放水，是这张图的几何**：地图本身就是 20×12，与瓦判据窗口同尺寸（窗口
    # 只有一个取样位，所以"最坏窗口"==全图），而对象窗口 20×11.25 在这张图上滑不出第二格 ⇒
    # "每屏装饰"在这里读的就是**总件数**，不等于户外那三张图的"每屏密度"。分档仍取
    # plan §验收方式 的 ≥40 而不是按"室内该空"往下调：plan §风险 担心的是一间屋只放七八件，
    # 而这是一张 240 格、可走 144 格的洞窟，57 件摊上去本来就在 40 上面——把门槛降到 20 只会
    # 让"装饰管线漏跑一张"变得看不见。
    "cold_spring_cave": 40,
    # interior_forge（M72-11 §推进顺序 5 第一片，第一张**作坊**室内）实测 43 件、
    # min/median/max = 43/43/43（复现：`python tools/scene_readings.py scenes/interior_forge.scene`）。
    # 三个数同形与洞穴同一条几何理由：地图本身就是 20×12，与瓦判据窗口同尺寸 ⇒ "每屏装饰"
    # 在这里读的就是**总件数**。分档仍取 plan §验收方式 的 ≥40 不往下调：43 是
    # `decor_kit.SCENE_DECOR["interior_forge"]` 把基线压到 0.45/0.35、并摘掉八件户外条目之后
    # 的实测，把门槛降到 20 只会让"装饰管线漏跑一张室内图"变得看不见。
    # 与洞穴那条的**不同**要记清楚：洞穴是"240 格里可走只有 144"，本图是 240 格里可走 146
    # 而其中 101 格是夯土地面——室内的"密"本来该由十一件家具与石龛读（全部 sprite 那一档
    # 实测 57 件），装饰只是地上的木屑碎渣，所以这里的 43 不是"密度刚好够线"，是
    # "减法做完之后地上还剩多少"。
    "interior_forge": 40,
    # interior_herb（M72-11 §推进顺序 5 第四片=药庐场景侧，图集侧是第三片 `3710bd9f`；第二张上金标的室内图）实测 41 件、
    # min/median/max = 41/41/41（复现：`python tools/scene_readings.py scenes/interior_herb.scene`）。
    # 三个数同形与洞穴/铁匠铺同一条几何理由（20×12 与瓦判据窗口同尺寸 ⇒ "每屏装饰"读的是总件数）。
    # 分档仍取 plan §验收方式 的 ≥40 不往下调。41 只比线高 1 件，这个数不是"刚好卡过"：
    # 它是 `decor_kit.SCENE_DECOR["interior_herb"]` 把 1/3 两档对齐铁匠铺（0.45/0.35）、
    # 只把 2=药渣 抬到 0.55 之后的实测；起草档 0.40/0.30 读 37 件，当时改的方向是"给本图
    # 单独降门槛"，被否掉了——降门槛会让"装饰管线漏跑一张室内图"看不见，而两张室内图本来
    # 就该在同一个语义位上同档。全部 sprite 那一档实测 56 件（铁匠铺 57），室内密度的主体
    # 仍是十二件家具与石龛，不是地上的药渣碎屑。
    "interior_herb": 40,
}

# ---------------------------------------------------------------------------
# D5 复核（M72-12 前哨回炉，见 plan §未完成项 1）
# ---------------------------------------------------------------------------
# 三档 tint 是**顶点色乘法**（`ground_tints`：阴 0.96/0.98/1.0、阳 1.03/1.02/0.99），
# 实测在任何一个族上都只挪 ±3% 亮度 ≈ 4~8 个 8bit 级，而**同族色阶相邻档之间**是
# 18~38 级——tint 的跨度不到一档色阶的一半，肉眼读作"同一块平涂"。所以"同屏 ≥24 种"
# 这个数**有多少是 tint 撑起来的**必须单独报，否则计数门会掩盖视觉单调。
MONOTONOUS_BLOCK_CELLS = 24  # 同一图集条目的连通块 ≥ 这么多格 = 一屏内基本全是它 = 满块
TINT_STEP_RATIO_MIN = 0.5  # 三档跨度 / 色阶相邻档差 的下限；低于它判"tint 读不出来"


def ground_tilemap(entities):
    """地表层（`entity_meta role=ground_tilemap`）的 layout。"""
    for entity in entities:
        if entity["meta"].get("role") == "ground_tilemap" and "Tilemap" in entity["components"]:
            errors = []
            layout = ps.tile_layout(ps.parse_fields(entity["components"]["Tilemap"]),
                                    errors, entity["name"])
            if errors:
                raise SystemExit(f"[FAIL] {entity['name']}: {errors}")
            return layout
    return None


def appearance_grid(layout):
    """(图集条目, tint 档) 网格。空瓦（0）不参与计数，与那道门的 `entries[index] != 0` 一致。"""
    cells = layout["cells"]
    width, height = layout["width"], layout["height"]
    entries = [ps.resolve_atlas_index(layout, cells, x, y)
               for y in range(height) for x in range(width)]
    return entries, layout["tints"], width, height


def worst_tile_window(entries, tints, width, height):
    """最坏的 20×12 窗口：返回 (外观种数, 只算图集条目的种数, 左上角格)。

    两个数一起报是 D5 的口径：外观种数里有一大块是 tint 撑起来的（同一张瓦乘 3% 亮度
    算"另一种"），而屏幕上读不读得出来是另一回事。只报前者等于让计数门掩盖单调。
    """
    best = (None, None, (0, 0))
    for oy in range(0, height - SCREEN_TILES_H + 1):
        for ox in range(0, width - SCREEN_TILES_W + 1):
            seen = set()
            plain = set()
            for y in range(oy, oy + SCREEN_TILES_H):
                for x in range(ox, ox + SCREEN_TILES_W):
                    index = y * width + x
                    if entries[index] != 0:
                        seen.add((entries[index], tints[index]))
                        plain.add(entries[index])
            if best[0] is None or len(seen) < best[0]:
                best = (len(seen), len(plain), (ox, oy))
    return best


def flat_blocks(entries, tints, width, height):
    """同一图集条目的 4 邻域连通块（≥ `MONOTONOUS_BLOCK_CELLS` 格），按大小降序。

    为什么按"条目"而不是按"kind"：autotile 开着时 kind 相同、四邻不同 → 条目不同，
    那种边界犬牙正是画面有层次的地方。真正单调的是**整片落进同一个条目**的内部格
    （`variant_for` 在 autotile 分支下不参与寻址，所以那一整片是同一张 32px 图）。
    """
    seen = set()
    blocks = []
    for start in range(width * height):
        if start in seen or entries[start] == 0:
            continue
        key = entries[start]
        queue = [start]
        seen.add(start)
        cells = []
        while queue:
            index = queue.pop()
            cells.append(index)
            x, y = index % width, index // width
            for nx, ny in ((x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)):
                if 0 <= nx < width and 0 <= ny < height:
                    other = ny * width + nx
                    if other not in seen and entries[other] == key:
                        seen.add(other)
                        queue.append(other)
        if len(cells) >= MONOTONOUS_BLOCK_CELLS:
            blocks.append((key, cells))
    blocks.sort(key=lambda item: -len(item[1]))
    return blocks


def tint_vs_step(ramp, family):
    """D5 的比值：三档 tint 的亮度跨度 / 同族色阶相邻档差的最小值。

    分子取"该族中间那一档"被三档 tint 乘出来的亮度极差，分母取该族四档本体亮度的最小
    相邻差——两者都是 8bit 亮度级，比值 <1 就意味着"晒到/在阴里"的差**不到一档换色**，
    而换色是看得见的。这个数是色阶表 + tint 表的纯函数，与某张图无关，所以它判的是
    "这根杠杆本身有多强"，`flat_blocks` 判的是"这根杠杆需要多强"。
    """
    levels = ramp["families"][family]
    lights = [tk.luminance(tk.rgb(hex_value)) for hex_value in levels]
    steps = [lights[i + 1] - lights[i] for i in range(len(lights) - 1)]
    base = tk.rgb(levels[len(levels) // 2])
    spanned = [tk.luminance(base)]
    for _, rgba in tk.ground_tints(ramp):
        spanned.append(tk.luminance(tuple(min(255.0, base[i] * rgba[i]) for i in range(3))))
    span = max(spanned) - min(spanned)
    return span, min(steps), span / min(steps)


def sprite_rects(entities):
    """所有带 Sprite + Transform 的实体 → `decor_kit.window_counts` 要的 placement 形状。"""
    rects = []
    for entity in entities:
        sprite = entity["components"].get("Sprite")
        transform = entity["components"].get("Transform")
        if not sprite or not transform:
            continue
        fields = ps.parse_fields(sprite)
        size = fields.get("size", "")
        if "x" not in size:
            continue
        width, height = (int(part) for part in size.split("x"))
        # 统一形态（M32）：`position=x,y,z;rotation=…;scale=…`，pivot=(0,0) → 那就是左下角。
        parts = dict(entry.split("=", 1) for entry in transform.split(";") if "=" in entry)
        xy = (parts.get("position", "0,0").split(",") + ["0", "0"])[:2]
        rects.append({"name": entity["name"], "x": float(xy[0]), "y": float(xy[1]),
                      "size": (width, height), "band": entity["meta"].get("role", "")})
    return rects


def report(scene_path, gate=False):
    header, entities = ps.parse_scene(scene_path)
    layout = ground_tilemap(entities)
    scene = header.get("name") or os.path.splitext(os.path.basename(scene_path))[0]
    print(f"{scene_path}  name={header.get('name', '?')}")
    if layout is None:
        # 0 tilemap 的门面图（main.scene）走这条：它本来就没有地表，报出来而不是崩。
        print("  地表 tilemap：无（门面图？）——没有地图尺寸就不算窗口类读数")
        print(f"  sprite 实体 {len(sprite_rects(entities))} 件")
        if gate:
            # 门不许对"没有地表"这件事静默放行：要么这张图确实是门面（那就别把它列进
            # `--gate` 的清单），要么装配漏了 GroundMap（那就是真红）。
            raise SystemExit(f"[FAIL] {scene}：没有地表 tilemap，窗口类读数无从判起"
                             "——门面图就别进 --gate 清单，真场景就是漏装 GroundMap")
        return False
    entries, tints, width, height = appearance_grid(layout)
    kinds = {}
    for value in layout["cells"]:
        kinds[value] = kinds.get(value, 0) + 1
    solid = ps.parse_fields(
        next(e["components"]["Tilemap"] for e in entities
             if e["meta"].get("role") == "ground_tilemap"))["solid"]
    distinct = len({(e, t) for e, t in zip(entries, tints) if e != 0})
    worst, plain, at = worst_tile_window(entries, tints, width, height)
    flags = ("autotile" if layout["autotile"] else "direct")
    print(f"  地表 {width}x{height} 图集 {layout['atlas_cols']}x{layout['atlas_rows']} "
          f"{flags} kinds={layout['tile_kinds']} variants={layout['variant_count']}")
    print(f"  瓦 kind 直方图 {kinds}  solid {sum(int(c) for c in solid)} 格  "
          f"tint 非白档 {sum(1 for t in tints if t)} 格")
    verdict = "ok" if worst >= MIN_SCREEN_TILES else f"低于 {MIN_SCREEN_TILES}"
    print(f"  同屏可见瓦种类：全图 {distinct} 种 / 最坏 {SCREEN_TILES_W}x{SCREEN_TILES_H} 窗口 "
          f"{worst} 种（左上角格 {at}）→ {verdict}")
    print(f"    其中只算图集条目（把三档 tint 折回同一张瓦）{plain} 种"
          f"——tint 撑起 {worst - plain} 种，占 {100 * (worst - plain) // max(1, worst)}%")
    decor = [r for r in sprite_rects(entities) if r["band"] == "decor"]
    decor_median = None
    for label, items in (("装饰", decor), ("全部 sprite", sprite_rects(entities))):
        counts = dk.window_counts(items, width, height, window=OBJECT_WINDOW)
        print(f"  每屏可见{label}（{len(items)} 件）min/median/max = "
              f"{counts[0]}/{counts[1]}/{counts[2]}（最坏窗口左下角 {counts[3]}）")
        if label == "装饰":
            decor_median = counts[1]
    if gate:
        gate_checks(scene, worst, decor_median, len(decor))
    d5(scene_path, layout, entries, tints, width, height)
    return True


def gate_checks(scene, worst_tiles, decor_median, decor_count):
    """`--gate` 的两条硬判据：同屏瓦种类、每屏装饰 median。红就点名是哪个数。"""
    if scene not in MIN_DECOR_OBJECTS:
        raise SystemExit(f"[FAIL] {scene}：没登记每屏对象分档"
                         f"（已登记：{' '.join(sorted(MIN_DECOR_OBJECTS))}）"
                         "——plan §风险 说室内要按分档另判，这里不替它猜一个数")
    failures = []
    if worst_tiles < MIN_SCREEN_TILES:
        failures.append(f"同屏可见瓦种类 {worst_tiles} < {MIN_SCREEN_TILES}")
    if decor_count == 0:
        failures.append("这张图一件装饰都没有，散布管线没跑")
    elif decor_median < MIN_DECOR_OBJECTS[scene]:
        failures.append(f"每屏装饰 median {decor_median} < {MIN_DECOR_OBJECTS[scene]}"
                        f"（{scene} 登记的分档）")
    if failures:
        raise SystemExit(f"[FAIL] {scene}：" + "；".join(failures))


def d5(scene_path, layout, entries, tints, width, height):
    """D5 逐图复核：满块在哪、tint 那根杠杆在那个族上有多强。"""
    scene = os.path.splitext(os.path.basename(scene_path))[0]
    try:
        ramp = tk.load_ramp(scene)
    except KeyError:
        print(f"  D5 复核：色阶表还没登记场景 {scene!r}"
              f"（已登记：{' '.join(tk.scenes())}）——先补表再判")
        return
    names = {int(e["id"]): e["name"] for e in ramp["kinds"]}
    families = {int(e["id"]): e["family"] for e in ramp["kinds"]}
    cells = layout["cells"]
    print("  D5 复核（同条目满块 ≥{} 格；tint 跨度/色阶步长 <{} 判读不出来）".format(
        MONOTONOUS_BLOCK_CELLS, TINT_STEP_RATIO_MIN))
    ratio_cache = {}
    flagged = 0
    for key, block in flat_blocks(entries, tints, width, height):
        kind = cells[block[0]]
        family = families.get(kind, "?")
        if family not in ratio_cache:
            ratio_cache[family] = tint_vs_step(ramp, family)
        span, step, ratio = ratio_cache[family]
        tiers = {}
        for index in block:
            tiers[tints[index]] = tiers.get(tints[index], 0) + 1
        xs = [index % width for index in block]
        ys = [index // width for index in block]
        mark = "读不出来" if ratio < TINT_STEP_RATIO_MIN else "可接受"
        if mark == "读不出来":
            flagged += 1
        print(f"    entry {key:3d} {len(block):3d} 格 x{min(xs)}..{max(xs)} y{min(ys)}..{max(ys)} "
              f"kind {kind}({names.get(kind, '?')}/{family}) tint 档 "
              f"白{tiers.get(0, 0)}/阴{tiers.get(1, 0)}/阳{tiers.get(2, 0)} "
              f"→ 该族三档跨度 {span:.1f} vs 色阶步长 {step:.1f}（{ratio:.2f}×）{mark}")
    print(f"    结论：{flagged} 片满块落在 tint 读不出来的族上"
          + ("——画面靠装饰与交界犬牙撑，不靠 tint" if flagged else ""))


def main():
    parser = argparse.ArgumentParser(description="一张 .scene 的量化读数（M72-11）")
    parser.add_argument("scene")
    parser.add_argument("--gate", action="store_true",
                        help="把两条量化判据变成硬失败（verify.ps1 用）；不带就是纯读数")
    args = parser.parse_args()
    if not os.path.exists(args.scene):
        raise SystemExit(f"[FAIL] 读不到 {args.scene}（在工程根目录 examples/game_jhyt 下跑）")
    report(args.scene, gate=args.gate)


if __name__ == "__main__":
    main()
