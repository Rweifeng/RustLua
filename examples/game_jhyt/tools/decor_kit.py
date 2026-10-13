# -*- coding: utf-8 -*-
"""装饰散布的**单一实现**（M72-05）。

`tools/gen_manor_scene.py`（从零生成）与 `tools/set_scatter_decor.py`（就地改盘上场景）
都从这里取同一份 `scatter()` 与 `decor_lines()`——M72-04 的教训是这两件事必须产出
**逐字节相同**的场景段，否则"重跑生成器"会抹掉盘上场景，对拍也就失去了意义。

三件事：

1. **确定性**：每格独立哈希取随机数（`hash_stream(质数混合)`），**不是一条顺序流**。
   顺序流会让"多撒一格"把后面所有格子的序列整体重排（review 时看不出改了什么，
   黄金表一次红全图）。同输入必出同输出——这条由 `jhyt_m72_scatter_decor` 钉住。
2. **玩法留白**：药田矩形、places.lua 的地点格、出生/出口、既有道具格周围一律不散布。
   症状可复现：湿痕与作物画在 FarmGrid 层，装饰压上去就是"看不清哪块浇过"；
   NPC 落点被灌木盖住则是"人站在一丛草里"。
3. **染色跟随地表**：装饰的 sprite tint 直接取**它所在格的地表 per-tile tint**
   （`tints=`/`tints_rgb=`，M72-02 的稀疏载荷）。于是阴里的灌木和阴里的草地是同一档
   明度——这是"两张图看起来是一个世界"的机制，而不是各自随机。

分层（都在 `world` 层秩内，靠 `order` 分带；见 `scripts/world/depth.lua` 的层秩×1000+order）：
  * `debris`（order -30）：画在所有建筑/道具**之前**，永不遮挡任何东西；
  * `mid`（order 12）：夹在小道具（10）与树（20）之间。
  **为什么中景不放到 `actor` 层去按 Y 遮人**：actor 层秩 = 1、world = -1，主排序键是
  层秩×1000+order，所以 actor 带的任何值都**恒**压过 world 带的正房与树——一根长在房子
  北边的灌木会浮在屋顶上。要按 Y 遮人必须让静态美术也进 actor 带并各自折 Y，那是
  M72-06 前景层的活，见该文件与本包「未完成项」。

依赖：Python 3 标准库 + 同目录 `pixel_kit.py`、`terrain_kit.py`。
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import hash_stream, transform_line  # noqa: E402
import terrain_kit as tk  # noqa: E402

# 换这个常数 = 全图重排。门里的**反面对照**就是改它：必须真的测出内容变了。
DECOR_SEED = 0x5C0713
TILE = 32
# 地表 kind（与 `terrain_ramp.json` 的 kinds 同序号，改这里必须同步那张表）。
GRASS, DIRT, STONE, WALL, ROOF, WATER = 1, 2, 3, 4, 5, 6
DECOR_DIR = "assets/art/decor"
# 与既有道具同层名：`world`（层秩 -1），碎屑再靠负 order 排到所有正数 order 之前。
SORTING_LAYER = "world"
BAND_DEBRIS = -30
BAND_MID = 12

# 每格的"要不要撒"基线概率。墙/瓦上不可能长东西，直接 0；水面（M72-07）只为睡莲开一格——
# 目录里只有它带 `WATER` 权重，所以这一格不会让草从水里长出来。
#
# M72-07 把这三档往上挪了一档，理由不是"想更密"，而是这一包**改的是地面本身**：
#   * 建筑碰撞按"本体∪屋顶"并集重算后多了 26 格实心（原来能撒草的屋檐外圈），
#     池塘压了石砌岸、后院铺了石板晒场，两者都是**草地换石板**——
#     `jhyt_m72_scatter_decor` §② 的每屏中位数因此从 43 掉到 36（门槛 40）。
#   * 补的方式是挪基线概率，不是挪门槛：草地 0.70→0.75 顶掉被划成实心的那几格；
#     石板 0.40→0.55 读作"老院子石板缝生苔、夹碎石"（可撒的石板格有 138 个，
#     这一档实测撒满 43%，剩下的石板仍然干净）；水面 0.55→0.85 是因为 27 格的池子
#     只有 9 张莲叶时读作"蓝布上撒了几点绿"，睡莲要盖得住池面。
#   * 改后读数：装饰 204 件、每屏中位数 41（min 7 / max 75），13 种形状全部出现。
BASE_P = {GRASS: 0.75, DIRT: 0.35, STONE: 0.55, WALL: 0.0, ROOF: 0.0, WATER: 0.85}
# 实心格一般不撒装饰（撒在墙里/屋里就是看不见），但**水面是实心格里的例外**：
# `WATER` 在 `building_kit.TILE_NATIVE_SOLID` 里（玩家走不进池塘），而睡莲正该长在水面上。
# 不加这条例外，睡莲这个条目永远抽不到，画一张没人看的图。
DECOR_ON_SOLID = (WATER,)
# 疏密按 2×2 格块走（和 `set_terrain_autotile.scatter_tints` 同一个理由：逐格独立随机
# 在 32px 网格上读作噪点，成块才读作"这一片草密/那一片秃"）。(上界, 概率系数)
PATCH_BANDS = ((14, 0.0), (44, 0.55), (85, 1.0), (101, 1.45))
PATCH = 2
# 中景候选的权重缩放：灌木/孤石是"物件"不是"纹理"。第一版给 0.20，实测 175 件里只有
# **2 件**中景（一丛花一块石），整张图还是"只有纹理没有物件"——碎屑解决"地面有没有东西"，
# 视线落点靠中景。1.0 实测约 19%（33 件），一屏 2 件上下。
MID_SHARE = 1.0
# 临水格子的香蒲权重乘数。不加这条偏置，全图只有池塘边 10 格能长香蒲，抽中概率 2.6%，
# 实测**一次都不出**（等于白画一张图）——"水边就该长这个"不是随机里的小概率，是偏置。
# 6 试过仍为 0：那 10 格里约 6 格过生长阈值，22% 份额下不出香蒲的概率还有 22%。生境只有
# 两排格子，就必须在**那几格**里占大头（12 → 约 45%），否则这条目永远只是目录里的一行。
EDGE_BONUS = 12

# ---------------------------------------------------------------------------
# 目录：一张 sprite 一条。**尺寸必须与 `gen_decor_art.py` 画出来的 PNG 一致**，
# 那张表由后者的 `check()` 逐条比对（散布指向一张不存在或尺寸不同的图，症状是
# 装饰画不出来或者压到邻格，两种都不报错）。
# ---------------------------------------------------------------------------
CATALOG = (
    # name            文件名                        尺寸      带        各地表 kind 的权重
    ("GrassShort", "decor_grass_short", (16, 10), "debris", {GRASS: 34, DIRT: 10}),
    ("GrassTuft", "decor_grass_tuft", (18, 14), "debris", {GRASS: 26, DIRT: 6}),
    ("Twigs", "decor_twigs", (20, 9), "debris", {GRASS: 14, DIRT: 16, STONE: 3}),
    ("Pebbles", "decor_pebbles", (16, 7), "debris", {GRASS: 10, DIRT: 12, STONE: 8}),
    ("Petals", "decor_petals", (14, 6), "debris", {GRASS: 12}),
    ("LeafPile", "decor_leaf_pile", (18, 10), "debris", {GRASS: 12, DIRT: 6}),
    # 青苔在石板上是"脏"，不是"长满"：石板只占 87 格，权重给到和草皮同级会把路吃干净。
    ("MossPatch", "decor_moss_patch", (20, 8), "debris", {GRASS: 8, STONE: 10}),
    ("Bush", "decor_bush", (28, 22), "mid", {GRASS: 8, DIRT: 2}),
    ("Rock", "decor_rock", (24, 18), "mid", {GRASS: 5, DIRT: 6, STONE: 4}),
    ("Flowers", "decor_flowers", (22, 18), "mid", {GRASS: 6}),
    ("Mushroom", "decor_mushroom", (16, 14), "mid", {GRASS: 4, DIRT: 2}),
    # 香蒲/芦苇：**必须临水**（有 4 邻域水格）。夯土也要能长——池塘四周一圈实测大多是
    # 夯土与石板，只给草地权重的话这个条目一次都不会被抽到（等于白画一张图）。
    # M72-07 给石板也开一格：池塘压了石砌岸之后，水边那一圈从夯土变成石板，没有这一格
    # 香蒲就无处可长（它只认 4 邻域有水，不认地表）。
    ("Reed", "decor_reed", (20, 28), "mid", {GRASS: 6, DIRT: 4, STONE: 4}, "water"),
    # 睡莲：唯一长**在水面上**的条目（M72-07 的不规则池子）。走碎屑带（order -30）：
    # 它是水面纹理，不是能挡住谁的物件。
    ("Lily", "decor_lily", (22, 14), "debris", {WATER: 30}),
)


# ---------------------------------------------------------------------------
# 逐场景覆盖：**目录与基线概率是生态事实，不是口味**
# ---------------------------------------------------------------------------
# `CATALOG` 是四张已铺量场景共用的户外草甸清单（草、花、灌木、香蒲、睡莲）。寒泉洞是地下
# 场景：洞里不长草、不开花、没有香蒲与睡莲，把同一份清单原样撒进去会撒出一洞的绿草皮，
# 而 `BASE_P` 那一档"草地 0.75"读的是"阳光下的草甸该密"，暗场景照抄就成了噪点。
#
# **为什么是覆盖而不是新目录**：加形状会改所有已提交场景的散布字节（`scatter()` 逐格独立
# 哈希，但候选池一变，`pick_item` 的加权区间整体重排）——所以洞穴只做减法与降档，不新增条目。
# 没登记的场景一个字节都不受影响（`scene=None` ⇒ 覆盖表空 ⇒ 与旧路径同算式）。
SCENE_DECOR = {
    "cold_spring_cave": {
        # 洞底石板（kind 1）与凿平甬道（kind 3）各降一档：暗场景的低对比度下，密装饰会糊成
        # 一片脏纹理，而"这一片有东西/那一片空着"在暗处反而更靠块面读。
        "base_p": {GRASS: 0.55, STONE: 0.55},
        # 八件户外专属：草皮两件 + 花瓣/落叶（洞里无落叶层）+ 灌木 + 花 + 临水香蒲 + 睡莲
        # （暗河是**地下静水**，无光 ⇒ 没有光合作用那一条；`DECOR_ON_SOLID` 那格因此空置）。
        "exclude": frozenset(("GrassShort", "GrassTuft", "Petals", "LeafPile",
                              "Bush", "Flowers", "Reed", "Lily")),
    },
    # 铁匠铺内（M72-11 §推进顺序 5 第一片）：**作坊室内**，与洞穴那条"地下无光"不同因、
    # 同一种做法（只做减法与降档，绝不新增条目）。
    "interior_forge": {
        # 夯土地面（kind 1）与石板（kind 3）各压两档：屋子里的"密"应该由**家具**读，
        # 不是由草屑读；`BASE_P` 的 0.75 那一档读的是"阳光下的草甸该密"，照抄到 240 格的
        # 作坊里就是满地噪点。石板带（门前厅 + 北墙根 + 砌岸）压得更低，那是铺出来的地面。
        "base_p": {GRASS: 0.45, STONE: 0.35},
        # 摘掉的还是那八件户外专属，理由要按室内重写一遍：草皮两件（屋里不长草）、花瓣与
        # 落叶（屋内有顶，无落叶层）、灌木与花（没有花圃）、香蒲（临水条目——淬火槽是
        # **作坊设备**不是生态水面，边上扎芦苇读起来是"屋外"）、睡莲（同一句理由，
        # 且 `DECOR_ON_SOLID` 那格因此空置）。
        # 留下的五件正是"作坊地上该有的东西"：木屑枝桠（Twigs）、碎渣（Pebbles）、
        # 墙角青苔（MossPatch）、垫脚石块（Rock）、潮角落的蘑菇（Mushroom）。
        "exclude": frozenset(("GrassShort", "GrassTuft", "Petals", "LeafPile",
                              "Bush", "Flowers", "Reed", "Lily")),
    },
}


def overrides(scene):
    """这张图的覆盖档（`None`/未登记 ⇒ 空 dict ⇒ 走户外默认清单）。"""
    return SCENE_DECOR.get(scene, {})


def base_p(scene=None):
    """该场景的逐格基线概率表（默认 `BASE_P` 加场景覆盖）。"""
    out = dict(BASE_P)
    out.update(overrides(scene).get("base_p", {}))
    return out


def catalog_entry(name):
    for row in CATALOG:
        if row[0] == name:
            return row
    raise KeyError(f"装饰目录里没有 {name!r}")


def texture_path(stem, mirrored):
    # 镜像另出一张 PNG（`gen_decor_art.py` 用 `Canvas.mirrored()` 复制一份），不在场景数据里
    # 用 `flip_x`：引擎的规范序列化 `Sprite::to_component_str` **不写** flip 字段，作者串带上
    # flip_x 就过不了"生成 == 引擎回写"的对拍；两个 web 宿主也没接这条。
    suffix = "_r" if mirrored else ""
    return f"{DECOR_DIR}/{stem}{suffix}.png"


def _rand(x, y, salt, seed):
    """一格一个独立流：与邻居、与迭代顺序都无关（见模块 docstring 第 1 条）。"""
    return hash_stream(73856093 * (x + 1) ^ 19349663 * (y + 1) ^ (seed * 2654435761) ^ salt)(1000003)


def patch_factor(x, y, seed):
    """2×2 格块的疏密系数（0 = 整块留白，读作"这片地秃"）。"""
    bx = x - x % PATCH
    by = y - y % PATCH
    roll = hash_stream(1103515245 * (bx + 1) ^ 2654435761 * (by + 1) ^ seed)(100)
    for upper, factor in PATCH_BANDS:
        if roll < upper:
            return factor
    return 1.0


def neighbours_water(ground, width, height, x, y):
    for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
        nx, ny = x + dx, y + dy
        if 0 <= nx < width and 0 <= ny < height and ground[ny][nx] == WATER:
            return True
    return False


def pick_item(ground, width, height, x, y, kind, seed, allow_mid=True, scene=None):
    """按 kind 权重（中景再乘 `MID_SHARE`）加权抽一件；带临水约束的条目只在条件成立时进候选。

    `scene` 决定这张图的生态（见 `SCENE_DECOR`）：被点名的条目**不进候选池**，而不是
    "抽到了再丢弃"——丢弃会让候选池的加权区间与盘上不一致，`--check` 的逐字节对拍就废了。
    """
    hidden = overrides(scene).get("exclude", frozenset())
    pool = []
    total = 0
    for row in CATALOG:
        name, stem, size, band, weights = row[:5]
        if name in hidden:
            continue
        weight = weights.get(kind, 0)
        if not weight:
            continue
        if band == "mid" and not allow_mid:
            continue
        if len(row) > 5 and row[5] == "water":
            if not neighbours_water(ground, width, height, x, y):
                continue
            # 带环境偏置的条目吃 `EDGE_BONUS` 而不是 `MID_SHARE`：它的稀有是"生境稀缺"，
            # 不是"抽中概率要低"——在满足生境的那几格里它就该压倒性地长出来。
            scaled = weight * EDGE_BONUS
        else:
            scaled = weight * (MID_SHARE if band == "mid" else 1.0)
        total += scaled
        pool.append((total, name, stem, size, band))
    if not pool:
        return None
    roll = _rand(x, y, 0x9E37, seed) % int(total * 100)
    for upper, name, stem, size, band in pool:
        if roll < upper * 100:
            return {"name": name, "stem": stem, "size": size, "band": band}
    return None


def scatter(width, height, ground, solid, exclude, seed=DECOR_SEED, mid_avoid=frozenset(),
            scene=None):
    """返回装饰清单（row-major 顺序，所以同输入下实体 id 分配也是确定的）。

    `ground[y][x]` 是地表 kind，`solid[y][x]` 是 0/1，`exclude` 是两带都不许下的格子。
    `scene` 是这张图的场景键（`name=` / `maps.lua` 的地图键），决定基线概率与生态清单
    （见 `SCENE_DECOR`）；不传就是户外默认档，四张已铺量的图因此一个字节都不变。

    **留白跟着遮挡力走，不是一律保守**：`mid_avoid` 只挡中景带（order 12，会盖在井/木牌/
    晒谷架这些小道具 order 10 上面），碎屑带（order -30，画在所有物件之前、连玩家也永远在
    它上面）不受这条约束。早先版本给两带统一套半径 2 的留白，920 格里吃掉 180 格，
    一屏中位数只有 22 件——"画面几乎没东西"就是这么来的，而它防的症状根本不存在。
    """
    out = []
    params = base_p(scene)
    for y in range(height):
        for x in range(width):
            kind = ground[y][x]
            if kind not in params or not params[kind]:
                continue
            if solid[y][x] and kind not in DECOR_ON_SOLID:
                continue
            if (x, y) in exclude:
                continue
            factor = patch_factor(x, y, seed)
            if factor <= 0.0:
                continue
            if _rand(x, y, 0x51A3, seed) % 1000 >= int(params[kind] * factor * 1000):
                continue
            item = pick_item(ground, width, height, x, y, kind, seed,
                             allow_mid=(x, y) not in mid_avoid, scene=scene)
            if item is None:
                continue
            w, h = item["size"]
            # 偏移限制在本格内：相邻装饰因此**永不互相压**，同带同 order 的宿主间
            # tie-break 差异也就无从显现（web 按 entity id 破平，原生按插入序）。
            item["x"] = x * TILE + _rand(x, y, 0x2B71, seed) % (TILE - w + 1)
            item["y"] = y * TILE + _rand(x, y, 0x1F13, seed) % (TILE - h + 1)
            item["cell"] = (x, y)
            out.append(item)
    return out


def sprite_line(entity_id, texture, width, height, order, tint, layer=SORTING_LAYER):
    """字段顺序与引擎 `Sprite::to_component_str` 逐字符一致（对拍靠这条）。

    `layer` 默认 `world`（装饰就用它）；M72-06 的前景遮断块传 `foreground`——**同一行
    格式器**，否则"字段顺序"这条对拍契约就有了第二个版本。
    """
    return (
        f"component\t{entity_id}\tSprite\ttexture={texture};size={width}x{height};"
        f"pivot=0.000,0.000;tint={tint};sorting_layer={layer};order={order}"
    )


def decor_lines(start_id, placements, table=None, tint_of=None, seed=DECOR_SEED):
    """把散布清单打成 `.scene` 的行；`start_id` 必须是盘上未被占用的最小 id。

    `tint_of(cell)` 返回该格的**地表 tint id**（0/None = 白档），装饰就照抄那一档的 rgba，
    于是同一屏的地表和装饰永远处在同一个明度档里。
    `seed` 必须与 `scatter()` 用的是同一个：镜像档也在决定"这一件用哪张图"，
    换种子时若它不跟着变，`--check` 的反面对照就只测到位置、测不到贴图。
    """
    table = table or tk.load_ramp()
    palette = {tint_id: rgba for tint_id, rgba in tk.ground_tints(table)}
    lines = []
    for offset, item in enumerate(placements):
        entity_id = start_id + offset
        w, h = item["size"]
        rgba = palette.get(tint_of(item["cell"]) if tint_of else 0, (1.0, 1.0, 1.0, 1.0))
        tint = ",".join(f"{channel:.3f}" for channel in rgba)
        mirrored = _rand(item["cell"][0], item["cell"][1], 0x77C1, seed) % 2 == 1
        lines.append(f"entity\t{entity_id}\tDecor_{item['name']}_{entity_id}")
        lines.append(sprite_line(entity_id, texture_path(item["stem"], mirrored), w, h,
                                 BAND_DEBRIS if item["band"] == "debris" else BAND_MID, tint))
        lines.append(transform_line(entity_id, float(item["x"]), float(item["y"])))
        lines.append(f"entity_meta\t{entity_id}\trole\tdecor")
    return lines


def ring(cells, radius, width, height):
    """一圈"半径内不散布"的格子集合（切比雪夫距离，含中心格）。"""
    out = set()
    for cx, cy in cells:
        for dy in range(-radius, radius + 1):
            for dx in range(-radius, radius + 1):
                nx, ny = cx + dx, cy + dy
                if 0 <= nx < width and 0 <= ny < height:
                    out.add((nx, ny))
    return out


def rect_cells(rect, width, height):
    x0, y0, x1, y1 = rect
    return {(x, y) for y in range(max(0, y0), min(height, y1 + 1))
            for x in range(max(0, x0), min(width, x1 + 1))}


def hard_exclude(width, height, farm_rect):
    """两带共同的硬留白：只有药田矩形。

    为什么药田必须整块空：作物画在 `FarmGrid`（第二张 tilemap）上、湿痕在第三张，而
    tilemap 的世界通道在 sprite 之下——装饰一压上去就是"看不清哪块浇过"。这个症状
    运行时**不会报错**，只能靠留白规则守。
    """
    return rect_cells(farm_rect, width, height)


def mid_avoid(width, height, prop_cells, place_cells, key_cells):
    """只挡中景带（order 12）：既有道具一圈 + NPC 落点一圈 + 出生/出口一圈。

    半径 1 就够：中景会盖住的只有 order 0..10 的静态物件（井/木牌/晒谷架），而树（20）
    和玩家（actor 带）本来就在它上面。碎屑带（order -30）不吃这条，理由见 `scatter`。
    """
    return (ring(prop_cells, 1, width, height)
            | ring(place_cells, 1, width, height)
            | ring(key_cells, 1, width, height))


def window_counts(placements, width, height, tile=TILE, window=(20, 11.25), step_px=8):
    """按相机矩形统计"一屏可见对象数"：滑过所有可停靠窗口，返回 (min, median, max, min 窗口左下角)。

    判据口径写在这里而不是靠肉眼：窗口是**世界坐标矩形**（`zoom=2` 的 1280×720 设计视口
    = 20×11.25 格，docs/51 §2.2），装饰算的是矩形相交，所以一个 28px 宽的灌木探进窗口
    边缘就算一个（与引擎的裁剪一致）。`step_px=8` = 每 1/4 格取一个窗口，整数步进
    （浮点累加会把窗口序列的长度都抖掉）。

    为什么判据是**中位数**而不是最小值：硬留白（药田 200 格 + 正房/柴房/池塘/院墙）让
    "任何一屏都 ≥40"在这张图上做不到——最坏那一屏正好压在药田上，窗口 89% 的面积按玩法
    契约必须空着。所以报三个数，并把最坏窗口的位置一起交出去，让它可被复核而不是被忽略。
    """
    win_w = window[0] * tile
    win_h = int(round(window[1] * tile))
    max_x = width * tile - win_w
    max_y = height * tile - win_h
    if max_x < 0 or max_y < 0:
        raise ValueError(f"地图比窗口还小：{width}x{height} 格 vs 窗口 {window}")
    rects = [(item["x"], item["y"], item["size"][0], item["size"][1]) for item in placements]
    counts = []
    worst = (None, 0)
    for left in range(0, max_x + 1, step_px):
        right = left + win_w
        for bottom in range(0, max_y + 1, step_px):
            top = bottom + win_h
            hits = sum(1 for x, y, w, h in rects
                       if x + w > left and x < right and y + h > bottom and y < top)
            counts.append(hits)
            if worst[0] is None or hits < worst[1]:
                worst = ((left, bottom), hits)
    counts.sort()
    if not counts:
        return (0, 0, 0, (0, 0))
    return (counts[0], counts[len(counts) // 2], counts[-1], worst[0])
