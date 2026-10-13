# -*- coding: utf-8 -*-
"""建筑/道具**占位与碰撞**的单一事实源（M72-07 建，M72-11 起服务多张图）。

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
  * **树/竹/单件道具**（`role=prop`）：按 `SCENES[*].prop_collision` 点名的模式判，默认
    **单格根碰撞**——只实心 `entity_meta cell` 那一格，树冠/竹梢那几格由 M72-06 的前景
    遮断件负责盖人，不该挡人。横着占几格的（摊位/山石/山门）走 `box`（=建筑那条算法，
    也认门洞），桥面走 `walk`（可走的美术，一格都不许实心）。
  * **装饰**（`role=decor`）：不参与碰撞（M72-05 定的，`jhyt_m72_scatter_decor` §⑤ 在守）。

**门洞**（`SCENES[*].openings`）是唯一一处"手填"的数据，所以它自带一条反查：声明是洞，
贴图在那几格上就必须**真是洞**（不透明占比 ≤ `HOLE_MAX_OPAQUE_RATIO`），
而且洞必须与外面可走的地格连通。两边任何一边改了都会当场红，不会静默穿模。

**M72-11 起这是多场景模块**：判据（不透明像素、拆层配对、单格根、门洞反查）一张图一张图
完全相同，只有三处随场景变——地表贴图、门洞表、横着占两格的道具表。贴图从场景自己的
`role=ground_tilemap` 那条 Tilemap 读（所以这里不再存 manor 的路径），后两样收在 `SCENES`
里按场景的 `name=` 取。**没登记的场景当场红**，不会静默用 manor 的门洞去量云泽村。

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
GROUND_ROLE = "ground_tilemap"
# "压够一格"的门槛：1/4 格（8px）。见 `cells_covered` 的 docstring。
MIN_CELL_OVERLAP = TILE // 4

# 地表 kind 编号（与 `gen_manor_scene.py` 的列映射、`data/buildings.lua` 的 ground_tiles 一致）。
GRASS, DIRT, STONE, WALL, ROOF, WATER = 1, 2, 3, 4, 5, 6
# 这两族的格**天然实心**：墙与人是瓦画出来的，没有对应的 sprite 实体。
# 编号语义是引擎与数据侧共用的（`terrain_ramp.json` 的 `scenes[*].kinds` 硬校验同名同序），
# 所以这一条对每张图都成立，不必按场景分。
TILE_NATIVE_SOLID = (WALL, WATER)

BUILDING_ROLES = ("building", "building_site")
ROOT_ROLES = ("prop",)
# 道具的三种碰撞模式（`root` 是默认，另两种按场景在 `SCENES[*].prop_collision` 里点名）：
#   * `root` 单格根碰撞——只实心 `entity_meta cell` 那一格（树/竹/井/罐/杆：冠梢那几格
#     由 M72-06 的遮断件负责盖人，本来就不该挡人）。
#   * `box`  按视觉框碰撞——本体就横着占几格的东西（篱笆、货架、山石、摊位、山门），
#             只标锚点会让另一半"看得见却穿得过"。与建筑同一条算法，所以也认门洞。
#   * `walk` 可走的美术——桥面那一格**必须**能踩上去。它是"看得见但不实心"的唯一一类：
#             判据说的是"每格实心都得有可视依据"，不是"看得见的都得实心"。
PROP_MODES = ("root", "box", "walk")

# 遮断件所在的层：它的 quad 也属于建筑的视觉框（见模块 docstring 的第一条判法）。
OCCLUDER_LAYER = "foreground"

# 门洞（`openings`）：实体名 -> 必须可走的绝对 cell，建筑和 `box` 道具都能声明。
# 声明为洞的格上，贴图不透明像素占比的上限。**只查每列最下面那一格**（人站着的那格）：
# 它上面那格是屋檐/匾额所在，本来就该画满（南门实测 0.91），要求它是洞等于要求门楼没有顶。
# 0.35 是量出来的：门洞那格重画前 0.94（匾额正好挂在通道上）、重画后 0.00，
# 而"看着有门其实实心"的写法（药庐那种棕色门板）在 0.86~1.00，中间隔着一条宽缝。
HOLE_MAX_OPAQUE_RATIO = 0.35

# 每张图专有的三样（判据本身全场景相同，见模块 docstring）：
#   * `openings`：这张图的门洞格。村庄与山道那三个洞的出处都是 `data/maps.lua` 的出口格——
#     南门 (19,1)(19,2) 是村↔山庄出口，山门 (10,20) 是村↔青云山出口，
#     山道的 MountainGate (19,1)(19,2) 是山↔村出口格与山的落点（= spawn）。
#     山门那条按**数据侧的出口格**放：它压 x9..12 三行（128×96 贴北墙），只有最底一行 (10,20) 实测
#     是洞（不透明占比 0.156），上面两行是门楼的身与顶（0.94~0.99）。人从 (10,19) 走进去、
#     在那一格按 E 切图，落点回 (10,19)——所以 (10,20) 必须是可站的洞，而它上面那格
#     本来就该是实体，否则门楼没有顶。
#   * `prop_collision`：默认 `root` 之外的例外，逐件点名（写错模式当场红，见 `PROP_MODES`）。
# 盘上对拍：manor 的 `box` 两件算出来的格与引入 `box` 之前逐格相同（篱 (13,1)(13,2)、
# 架 (11,10)(12,10)），1/4 格阈值前后 likewise 41 格实心不变。
SCENES = {
    "manor": {
        "openings": {
            "ManorGate": ((16, 1), (16, 2)),
        },
        "prop_collision": {
            "ManorFence": "box",
            "ManorRack": "box",
        },
    },
    "village": {
        "openings": {
            "VillageGate": ((19, 1), (19, 2)),
            "VillageMountainGate": ((10, 20),),
        },
        "prop_collision": {
            # 石桥是过河的唯一通道（溪水 y16..17 只在 x24..25 断开）：标成实心就把村北封死。
            "VillageBridge": "walk",
            # 摊位 64×40 横占两格（台面真的有两格宽），只标锚点等于右半边能穿过去。
            "VillageStallA": "box",
            "VillageStallB": "box",
            "VillageRockA": "box",
            "VillageRockB": "box",
            # 山门 128×96：石体压 11 格、中间留一个真洞（见上面 openings 里那条读数）。
            "VillageMountainGate": "box",
        },
    },
    # 青云山道（M72-11）。**门洞格与出口格同源**：`data/maps.lua` 的 `mountain_to_village`
    # 出口格是 (19,1)，`village_to_mountain` 的落点（= 本图 spawn）是 (19,2)，所以山门那一列
    # 必须有两格可站的洞。贴图 128×96 压在 cell (18,0)：紧包围盒 x18..21 × y0..2 共 12 格，
    # 减这两个洞 = 10 格实心（`gen_wilds_art.build_mountain_gate` 把「青云山」匾挂在额枋
    # 行 16..31、通道行 32..63 只留一道檐下阴影，就是为了让 (19,1) 真是洞）。
    "mountain_path": {
        "openings": {
            "MountainGate": ((19, 1), (19, 2)),
        },
        "prop_collision": {
            # 栈道（128×64 铺在 x23..27 的溪面上）是**过溪的唯一通道**：标成实心就把东岸的
            # 贼寨、矿石堆与洞口整块封死。它是"看得见但不实心"的桥面那一类。
            "Boardwalk": "walk",
            # 山石 48×32 横占两格，只标锚点等于右半边能穿过去。A 从盘上的 (11,12) 挪走：
            # 那一格正是 `places.lua:mountain_herbs` (12,12) 的位置，`box` 会把地点格压成实心。
            "BoulderA": "box",
            "BoulderB": "box",
        },
    },
    # 演武场（M72-11 ④，这张图以前是七张里唯一还在逐件手填 `solid_cells` 的）。
    # **门洞格与出口格同源**：`data/maps.lua` 的 `arena_to_main` 出口格是 (9,1)，而门洞
    # 必须有两格可站的洞（(9,1) 与门内 (9,2)），否则南门就是一道画在墙上的假门。贴图
    # `manor_gate` 96×96 压 cell (8,0) ⇒ 紧包围盒 x8..10 × y0..2 共 9 格，减这两个洞 = 7 格
    # 实心，与旧版手填的 `GATE_PILLARS`（四根立柱）加 row0 天然墙逐格相同——这条对拍是
    # "把 `box`/门洞算法引进来没有悄悄改碰撞"的证据，不是巧合（manor 用同一张图、同一结构）。
    # 关键前提在生成器那边：`GATE_ROW` 把门洞那一列的地表刷成**石板**而不是院墙，因为
    # `TILE_NATIVE_SOLID` 含 WALL，压在墙行上的洞永远实心。
    "combat_arena": {
        "openings": {
            "ArenaGate": ((9, 1), (9, 2)),
        },
        "prop_collision": {
            # 兵器架 64×46、山石 64×40、茶水棚 64×40：横着占两格的实体，只标锚点等于
            # 右半边"看得见却穿得过"（旧版手填的就是这两格，引进 `box` 才能顺带把
            # 46/40px 高度压出的**第二行**也按可视依据算出来）。
            "ArenaRackA": "box",
            "ArenaRackB": "box",
            "ArenaRockA": "box",
            "ArenaRockB": "box",
            "ArenaRockC": "box",
            "ArenaRockD": "box",
            "ArenaRockN": "box",
            "ArenaStall": "box",
            # 木栈桥铺在池面上，是**上北岸的唯一通道**：与山道的 Boardwalk 同一类
            # "看得见但不实心"。生成器那边还把桥面三格的地表刷成石板，两侧合起来才是
            # "一座真能走上去的桥"（见 `gen_arena_scene.PIER`）。
            "ArenaPier": "walk",
        },
    },
    # 寒泉洞（M72-11 §推进顺序 4，第一张**室内/地下**图）。
    # **`openings` 是空的**，而且这是登记不是漏登记：这个字段只服务"石体压几格、中间留一个
    # 真洞"的门楼（山门/南门），本图唯一的建筑是石龛（`village_shrine` 64×64，四面都是墙），
    # 洞口则是地图边界上的一列凿平石——`gen_cave_scene.ENTRANCE` 不压在洞壁行/列上，天然实心
    # 只来自 `TILE_NATIVE_SOLID`（墙与水），没有需要人工挖开的格。空表在
    # `jhyt_m72_building_footprint` §③ 与 `check_openings` 下都是**平凡通过**（不报错、也不
    # 假装测过），这条读法在山道/演武场那两批已核过。
    "cold_spring_cave": {
        "openings": {},
        "prop_collision": {
            # 山石 48×32 横占两格：只标锚点等于右半边"看得见却穿得过"（与山道 BoulderA/B、
            # 演武场 ArenaRock* 同一判法）。
            "CaveRockA": "box",
            "CaveRockB": "box",
            # 矿脉 40×32 压 (8,9)+(9,9)，`cave_ore` 那个地点格在 (9,8) 隔壁，不冲突。
            "CaveOreVein": "box",
            # 火把 24×56、木箱 32×28、雪莲 32×32 的紧包围盒都被 `MIN_CELL_OVERLAP`（1/4 格）
            # 收在锚点那一格内 ⇒ 走默认 root，不登记；`Stove` 是 `role=furniture`，
            # `ROOT_ROLES` 根本不看它（那条"看得见却穿得过"记在 `gen_cave_scene.EXTRAS`，
            # 归室内图那批一起收）。
        },
    },
}


def scene_name(lines):
    """场景键（与 `data/maps.lua` 的地图键、`scenes/<name>.scene` 文件名同一个）。"""
    for line in lines:
        if line.startswith("name="):
            return line.split("=", 1)[1].strip()
    raise SystemExit("[FAIL] 场景缺 `name=` 头：无法确定它属于哪张图的门洞表")


def profile(scene):
    """取这张图的（门洞表, 道具碰撞表）。没登记就红——用错表比没有表更糟。"""
    if scene not in SCENES:
        raise SystemExit(f"[FAIL] `building_kit.SCENES` 里没有场景 {scene!r} 的条目："
                         f"请登记它的 openings / prop_collision（没有门洞就写空 dict）")
    return SCENES[scene]


def openings_table(scene):
    """这张图声明的门洞：{实体名: (cell, ...)}。生成器的可达性自检与门洞反查共用它。"""
    return profile(scene)["openings"]


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
    """不透明像素矩形**压进至少 1/4 格**的那些 cell（两轴各算，短边不算覆盖）。

    为什么不写"与格子有交集就算"：道具贴图的高宽普遍不是 32 的整数倍（摊位 64×40、
    山石 64×40、招幌 32×56），而 `Canvas.outline()` 沿不透明像素外扩一圈——紧框因此比实体
    本身多出 1 行像素，那一行落在相邻格里只占 **1/32 高**。按交集判就会为这一像素的棚顶
    檐边把整格街面标成实心（村庄市集实测：两个摊位各吃掉一格街面 (13,3)/(23,3)）。
    按"每轴至少压进 `MIN_CELL_OVERLAP`"判：檐边那一行不算，而真正横着占两格的实体
    （摊位台面、篱笆、山门）照算。山庄 `--check` 在改这条前后逐格相同（41 件实心没动），
    所以这条不是为村庄开的口子，是把"覆盖"量准。
    """
    x0, y0, x1, y1 = rect
    out = set()
    for cx in range(int(x0 // TILE), int(-(-x1 // TILE))):
        if min(x1, (cx + 1) * TILE) - max(x0, cx * TILE) < MIN_CELL_OVERLAP:
            continue
        for cy in range(int(y0 // TILE), int(-(-y1 // TILE))):
            if min(y1, (cy + 1) * TILE) - max(y0, cy * TILE) < MIN_CELL_OVERLAP:
                continue
            out.add((cx, cy))
    return out


# ---------------------------------------------------------------------------
# 判据
# ---------------------------------------------------------------------------


def visual_cells(root, item, sprites):
    """(一件实体的视觉框格, 它的同名拆层件)。

    同名拆层件：M72-06 把正房拆成 `ManorHouse`(world) + `ManorHouseRoof`(foreground
    遮断件)，视觉框是两件并集。配对规则是实体名前缀，**不按 role 筛**——
    屋顶那件的 role 是 `occluder`，按 role 筛会把它漏掉，那 21 格屋檐就会被判成
    "悬空碰撞"而删掉实心（玩家能走进屋檐里）。`*Shadow` 是投影贴片，必须排除。
    """
    cells = cells_covered(opaque_world_rect(root, item))
    mates = [other for other in sprites
             if other is not item and other["role"] not in ("decor", "cast_shadow")
             and other["name"].startswith(item["name"]) and other["name"] != item["name"]]
    for mate in mates:
        cells |= cells_covered(opaque_world_rect(root, mate))
    return cells, mates


def declared_openings(item, cells, openings_by_name):
    """这张图给这件实体声明的门洞（并核对洞还在视觉框里）。"""
    openings = set(openings_by_name.get(item["name"], ()))
    unknown = openings - cells
    if unknown:
        raise SystemExit(f"[FAIL] {item['name']} 声明的门洞 {sorted(unknown)} "
                         f"不在它的视觉框里：洞挪了位置而表没跟着改")
    return openings


def visual_and_solid(root, sprites, width, height, scene):
    """返回 (应实心格, 每栋建筑的读数, 每件的可见格)。`scene` 决定门洞/横件碰撞表。"""
    ents_by_name = {}
    for item in sprites:
        ents_by_name.setdefault(item["name"], []).append(item)

    openings_by_name = openings_table(scene)
    prop_collision = profile(scene)["prop_collision"]
    solid = set()
    readings = []
    for item in sprites:
        if item["role"] not in BUILDING_ROLES:
            continue
        if item["layer"] == OCCLUDER_LAYER:
            continue  # 屋顶那一件在下面按名字配对
        cells, mates = visual_cells(root, item, sprites)
        openings = declared_openings(item, cells, openings_by_name)
        solid |= cells - openings
        readings.append({"name": item["name"], "visual": len(cells), "solid": len(cells - openings),
                         "openings": sorted(openings), "mates": [m["name"] for m in mates]})
    for item in sprites:
        if item["role"] not in ROOT_ROLES:
            continue
        mode = prop_collision.get(item["name"], "root")
        if mode not in PROP_MODES:
            raise SystemExit(f"[FAIL] {item['name']} 的碰撞模式 {mode!r} 不认识："
                             f"只许 {PROP_MODES}（写错一个字母就会让一件东西整件没有碰撞）")
        if mode == "walk":
            continue
        if mode == "box":
            # 横着占多格的道具与建筑同一条算法：视觉框减门洞。山门那种"石体压着三格、
            # 中间留一个洞"的东西，只有走这条才既不留悬空碰撞、也不把通道堵死。
            cells, _mates = visual_cells(root, item, sprites)
            solid |= cells - declared_openings(item, cells, openings_by_name)
            continue
        if item["cell"] is None:
            raise SystemExit(f"[FAIL] {item['name']} 是 prop 却没有 entity_meta cell："
                             "单格根碰撞没有锚点可标")
        solid.add(item["cell"])
    clipped = {cell for cell in solid if 0 <= cell[0] < width and 0 <= cell[1] < height}
    return clipped, readings, ents_by_name


def ground_line(lines):
    """(行号, 那一行的 tab 分段)——按 `role=ground_tilemap` 认出地表那张 Tilemap。

    以前各处按贴图路径认（写死 manor），换一个场景就得再写一个常量；而每张玩法图的
    GroundMap 都带 `entity_meta <id> role ground_tilemap`（七张全一致），按角色认对场景数是零的。
    "哪一行是地表"这件事只在这里判一次（`set_terrain_autotile.py` 也 import 它）。
    """
    ground_ids = {parts[1] for parts in (line.split("\t") for line in lines)
                  if len(parts) == 4 and parts[0] == "entity_meta" and parts[2] == "role"
                  and parts[3] == GROUND_ROLE}
    hits = []
    for index, line in enumerate(lines):
        parts = line.split("\t")
        if len(parts) == 4 and parts[0] == "component" and parts[2] == "Tilemap" \
                and parts[1] in ground_ids:
            hits.append((index, parts))
    if len(hits) != 1:
        raise SystemExit(f"[FAIL] 期望恰好一条 role={GROUND_ROLE} 的 Tilemap，实到 {len(hits)} 条")
    return hits[0]


def ground_component(lines):
    """(行号, 字段 dict, width, height, kinds, solid 串)。"""
    index, parts = ground_line(lines)
    fields = _fields("\t".join(parts))
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
    expected, readings, _ = visual_and_solid(root, sprites, width, height, scene_name(lines))
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
    openings_by_name = profile(scene_name(lines))["openings"]
    sprites = placed_sprites(read_entities(lines))
    out = []
    for item in sprites:
        cells = openings_by_name.get(item["name"], ())
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
