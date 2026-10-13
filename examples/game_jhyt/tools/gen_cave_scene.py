# -*- coding: utf-8 -*-
"""《江湖有田》寒泉洞场景生成（scenes/cold_spring_cave.scene）——第一张上金标的**暗场景**。

用法（工程根目录 `examples/game_jhyt` 下）：

    python tools/gen_cave_scene.py                       # 覆盖盘上场景（默认路径）
    python tools/gen_cave_scene.py --out .temp/c.scene   # 对拍/试跑，不碰盘上场景

**本文件是这张图的唯一事实源**（M72-11 §推进顺序 4 起），与 `gen_manor_scene.py` /
`gen_village_scene.py` / `gen_mountain_scene.py` / `gen_arena_scene.py` 同构：同一条代码路径
（`building_kit` / `set_foreground_occluders` / `set_cast_shadows` / `decor_kit` /
`set_terrain_autotile` 的抖动与撒布），所以"重跑生成器"与"就地工具 `--check`"必须给出同一个
字节序列。文件名是 M72-11 接山道时 `git mv` 自 `gen_wilds_scene.py` 的（那张当时只剩洞穴，
山道早已搬去 `gen_mountain_scene.py`），本批起名字与内容对齐。

**解图集的账**（这次还清的两笔）：
  * 旧版地表借用 `assets/art/tilesets/cave/cave_ground.png`（6 列 × 1 行、**恒等寻址**，由
    `gen_wilds_art.py` 出图）。恒等寻址只有"一整块材质"那一档，于是洞壁↔地面的交界是一条
    按格子对齐的直线（docs/71 §1 点名的"像 demo"头号成因）。现在用这张图自己的
    `assets/art/tilesets/cold_spring_cave/cold_spring_cave_ground.png`（12×9，六族 × 17 条目，
    出自 `gen_terrain_tiles.py` 的 `CAVE_BASE/CAVE_EDGES`），交界按 4-bit 邻居掩码选过渡块，
    再叠 `jitter` 的犬牙。旧那张 6×1 图集与它的 `_preview.png` 一并从产出表里摘掉（盘上文件
    同批删，不留"两套地表真相"）。
  * 旧版每件道具**手填 `solid_cells`**（七件里三件填了、四件填空），改一件 sprite 的坐标就在
    原地留一片撞空气的悬空格。现在串是 `building_kit` 按贴图不透明像素算出来的。

**kind 语义位换序**（旧说法 → 新实测，两边都是 1..6 但含义不同）：
  * 旧：`1=洞底石地 2=碎石堆 3=暗河水 4=洞壁 5=矿脉 6=冰晶地`
  * 新：`1=cave_floor 洞底石板 2=cave_scree 塌落碎石（本图零格）3=hewn_stone 凿平甬道
    4=cave_rock 洞壁 5=ice_crust 冰晶地 6=cave_water 暗河`
  换序不是审美：`building_kit.TILE_NATIVE_SOLID=(4,6)`（墙与水天然实心）与
  `decor_kit.BASE_P` 的档位都是**按编号**跨图说话的，所以"水必须是 6、墙必须是 4"。旧 kind 5
  的矿脉**从地表语义里删掉**——矿是玩法对象不是材质，这件事由 `CaveOreVein` 那件实体承担
  （旧版同时有 kind 5 的格与那件实体，画面上是"地上刷一块矿、上面再站一枚矿石贴图"）。

**这张图必须有一件真建筑**（`CaveShrine`）：`jhyt_m72_building_footprint` 的反面对照 B
（"把同名拆层件从并集里摘掉必须报出缺碰撞"）在没有 `role=building` + 拆层件的图上是**空转**
的——道具走单格根碰撞，遮断件那几格本来就不实心，摘掉它一格碰撞都不少，门会直接红。演武场
接金标时踩过同一件事（记在 `gen_arena_scene.py` 的 `ArenaArmory` 那条注释）。这里复用
`assets/art/buildings/village_shrine.png`（64×64 = 2×2 格，已在 `layer_kit.LAYER_SPLITS`）：
洞里立一座石龛比"再搭一间屋"贴画面，而且四格占地不会把这 240 格的小图吃掉太多。

**单一光向在暗场景怎么落**（plan §推进顺序 4 要试的就是这一条）：判据不在这里写，读数是
`lighting.lua` 的两个数——投影偏移长 = 物体总高 × `shadow_reach`(0.12)，够 `min_reach_units`(8)
才配挂地面投影。本图摆在 `assets/art/{buildings,props}` 里的东西**总高最大是 56px 的火把**
（reach 6.72），石龛 64px ⇒ 7.68 ⇒ **一件都不够格**，所以盘上投影段是空的（`set_cast_shadows.py
--check` 测的是"空段与重算逐字节相同"，不是跳过）。这条要说清它不是手工豁免：把光向规则
原样搬到洞穴，它自己就把"平行光投影"排除掉了——洞里没有太阳，暗与透在这张图上交给
`terrain_ramp.json` 那六族**冷档**色阶与 `ground_tints` 的两档顶点色（阴 0.96/0.98/1.0、
阳 1.03/1.02/0.99）去读。够格的东西一旦进图（比如将来给洞口加一根 88px 的石柱），贴片会
自动跟着出现，不需要谁来改名单。

**farm.lua 防误垦**（48 格账）：`scripts/systems/farm.lua` 的 `FARM` 矩形是
`{x0=14,x1=33,y0=4,y1=13}` 且**没有场景门禁**，判"可开垦"只看 GroundMap `tile==2`。它与本图
（20×12）的交集是 x14..19 × y4..11 = **48 格**——照旧语义（kind 2=碎石堆）就等于"在 boss 洞里
按 E 能开出四十八格田"。所以全场禁止 kind 2，且这条测在**抖动之后**的载荷上
（`JITTER_TARGETS=(1,2)` 只改写草/土，而一格供体都不存在 ⇒ 抖完也长不出碎石）。

**契约锚点**（每次生成自检，失败非零退出；数值一律从数据侧读回来对拍，不在这里再抄一遍真相）：
  * `data/maps.lua`：`cold_spring_cave.spawn = (10,3)`、出口 `cave_to_mountain` 格 `(10,1)`
    → `mountain_path` (22,19)（山道那侧的落点由 `gen_mountain_scene.LANDINGS` 管）；
  * `data/places.lua`：`cave_entry (10,3)`、`cave_pool (7,4)`、`cave_ore (9,8)`、
    `cave_snow_lotus (15,7)` 四格必须可走且从出生点可达；
  * `data/enemies.lua`：三条 `map="cold_spring_cave"`（山魈首领 + 两只召唤幼崽槽位）的
    `home` 都是 (10,6)——它在甬道上，标成实心就是"首领卡在一堵墙里"；
  * 相机 bounds 640×384 = 20×12 格（`pixel_kit.camera_line` 按 W/H 产出，
    `scripts/world/camera_view.lua` 的 `validate_all` 核对"bounds == 地图尺寸"）。
    这张图的**地图就等于一屏**：`scene_readings` 的 20×12 判据窗口与全图重合
    （视口 640×360 世界像素 ⊂ 640×384），所以 min/median/max 三个读数同形，不是放水。

布局分区（cell，0-index，y 朝上，含端点）：
  洞壁带     ：row0 / row11 / col0 / col19（kind 4，天然实心）+ 三处岩凸
               （西 (3,7)(4,7)(3,8)、东北 (16,9)(16,10)(17,10)）——岩凸存在的理由不是口味：
               一圈矩形墙只会产出 4 种墙↔地掩码，同屏瓦种类那条门（≥24）就靠这类形状差撑
  洞口厅     ：x9..11, y1..3 凿平石（出生格与出口格都在这里，见上面"南门与院墙打架"那条同款
               前提：洞口的**通道格不许压在洞壁行上**，`TILE_NATIVE_SOLID` 含 WALL ⇒ 永远实心）
  甬道       ：x9..10, y4..8 凿平石，接厅与矿洞厅；Boss home (10,6) 在它上面
  矿洞厅     ：x6..13, y9..10 凿平石（`cave_ore` 与 `CaveOreVein` 在那一带）
  暗河       ：**按行手填的不规则岸线**（`POND_ROWS`，17 格，外接框 5×4 ⇒ 填充率 0.85）
               ；水格外一圈洞底石板刷成凿平石（"砌岸"，与山庄/演武场同一理由）
  冰晶地     ：`ICE_ROWS` 15 格，外接框 5×4 ⇒ 0.75（kind 5，可走；`decor_kit.BASE_P[5]=0`
               读作"冰面上不长东西"，与山道竹林地同一档）
  石龛       ：`CaveShrine` (16,6) 64×64 ⇒ 占地 (16,6)(17,6)(16,7)(17,7)，拆层见上面那条
"""
import argparse
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

# 工程根（`examples/game_jhyt`）。**事实输入**（maps.lua / places.lua / enemies.lua）按这里
# 定位，不跟着 cwd 走：写死的相对路径会让工具在别的目录下静默少一份输入（旧版就是这样——
# `read_place_cells` 的默认路径按 cwd 解析，换个目录跑就少一份留白输入）。
PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

from character_art import player_art  # noqa: E402  主角形象（与 manifest 同源）
import building_kit as bk  # noqa: E402  建筑/道具碰撞的单一实现（本图与另四张共用判据）
import decor_kit as dk  # noqa: E402  装饰散布的单一实现（洞穴那档生态见 `SCENE_DECOR`）
import light_kit as lk  # noqa: E402  光向与投影够格门槛的定义处（读数在本文件只打印不改写）
import set_cast_shadows as cs  # noqa: E402  地面投影贴片的单一实现
import set_foreground_occluders as fo  # noqa: E402  前景遮断拆层的单一实现
import terrain_kit as tk  # noqa: E402  逻辑瓦 + autotile + tint 的口径
from pixel_kit import camera_line  # noqa: E402  场景 2D 相机（取景锁在地图内）
from set_terrain_autotile import freeze_cells, interior_tints, jitter, scatter_tints  # noqa: E402

W, H = 20, 12
TILE = 32

# 场景键（= `name=` 头 = `data/maps.lua` 的地图键 = `terrain_ramp.json` 的 `scenes` 档名 =
# `decor_kit.SCENE_DECOR` 的键）。
SCENE = "cold_spring_cave"
GROUND_TEXTURE = tk.ground_texture(SCENE)

# 列映射（语义见模块 docstring 的"旧说法→新实测"；编号语义跨图不可重排）。
FLOOR, SCREE, HEWN, ROCK, ICE, WATER = 1, 2, 3, 4, 5, 6
# `terrain_ramp.json` 的水按**逻辑 kind 名**认（六族那张表里每一档都叫 water/dirt/…，
# 场景差异体现在 `family` 上——本图的 `water` 属于 `cave_water` 族）。`load_ramp` 只保证 id
# 连续且名字集合一致，所以这里不写数字 6，换序要连着 data 侧一起改。
WATER_NAME = "water"

# ---- 分区（(x0, y0, x1, y1)，含端点）----
ENTRANCE = (9, 1, 11, 3)        # 洞口厅（出生格 (10,3) 与出口格 (10,1) 都在这里）
CORRIDOR = (9, 4, 10, 8)        # 甬道（Boss home (10,6) 在它上面）
CHAMBER = (6, 9, 13, 10)        # 矿洞厅
# 岩凸：见模块 docstring 布局那一节。写成 cell 列表而不是矩形，因为它们是三处不规则的角。
OUTCROPS = ((3, 7), (4, 7), (3, 8), (16, 9), (16, 10), (17, 10))
# 暗河：**按行手填的不规则岸线**（口径同山庄 `POND_ROWS` / 演武场）。三条硬约束照抄：
# ① 相邻两行必须有公共列（一摊水，不许断成两个水坑）；② 形状不许盖住 `cave_pool (7,4)`
# （places.lua 的地点格，玩家要能站过去）；③ 每行的 x 集都落在外接框 x2..6 之内。
POND_ROWS = {
    2: (3, 4, 5),
    3: (2, 3, 4, 5, 6),
    4: (2, 3, 4, 5, 6),
    5: (3, 4, 5, 6),
}
POND_CELLS = {(x, y) for y, xs in POND_ROWS.items() for x in xs}
POND = (2, 2, 6, 5)             # 外接框（读数与自检用）
# 冰晶地：与暗河同一档的不规则岸线（外接框 x13..17 × y2..5，15 格 ⇒ 0.75）。
ICE_ROWS = {
    2: (14, 15, 16),
    3: (13, 14, 15, 16, 17),
    4: (13, 14, 15, 16),
    5: (14, 15, 16),
}
ICE_CELLS = {(x, y) for y, xs in ICE_ROWS.items() for x in xs}
# 外接框叫 `ICE_BOX` 不叫 `ICE`：`ICE` 是上面那行六元解包出来的 kind 名，同名会把形状常量
# 写进 kind 位（第一版就是这么撞上自检 1 的 `'<': tuple vs int`）。
ICE_BOX = (13, 2, 17, 5)
# 抖动冻结集：**空**。这张图没有农田（kind 2 全图禁用，见模块 docstring 的 farm.lua 那条），
# 而契约锚点全是 kind 3，`JITTER_TARGETS=(1,2)` 根本不碰它。写成空集而不是删掉这条，是为了
# 让"这里考虑过冻什么"可见。
FREEZE = []

# ---- 契约锚点（与 data/maps.lua / data/places.lua / data/enemies.lua 三方一致）----
SPAWN = (10, 3)                 # `mountain_to_cave` 的落点，也是本图出生格
EXITS = {"cave_to_mountain": (10, 1)}
# 这三格要**站在凿平甬道上**（不是碎石/冰面）：它们是交互与切图的落点，地表读起来必须是
# "人凿出来的路"。`cave_pool`/`cave_snow_lotus` 不吃这条（一个在砌岸边、一个在洞底石板上）。
HARD_PATH_ANCHORS = ("spawn", "exit", "boss_home")

# ---- 建筑 / 道具 ----
# texture：另几个生成器的产物；size 与图片像素 1:1（`check_texture_sizes` 逐条核）；
# pivot=(0,0) 即 transform 为左下角；cell：贴图左下角所在 cell。
#
# **这里没有 `solid_cells` 字段**（M72-07 的规矩）：占地只认 `building_kit` 一条规则——
# 建筑 = 本体∪同名拆层件的不透明像素覆盖格 − 门洞；道具 = `cell` 那一格，横着占几格的按
# `building_kit.SCENES[SCENE].prop_collision` 点名走 `box`/`walk`。
# 名字/顺序/cell/order 与旧版逐条相同 ⇒ 盘上 id 2..8、13 一个都不动（挪 id 会让 Lua 侧
# `entity.find` 与黄金表全部重排）；新增的石龛**追加在 extras 表尾**（id 14），装饰从 15 起。
ENTITIES = [
    {"name": "CaveTorchA", "texture": "assets/art/props/wilds_torch.png",
     "size": (24, 56), "cell": (1, 8), "order": 20, "role": "prop"},
    {"name": "CaveTorchB", "texture": "assets/art/props/wilds_torch.png",
     "size": (24, 56), "cell": (18, 6), "order": 20, "role": "prop"},
    # 山石 48×32 横占两格：只标锚点等于右半边"看得见却穿得过"（与山道 BoulderA/B 同一判法）。
    {"name": "CaveRockA", "texture": "assets/art/props/wilds_boulder.png",
     "size": (48, 32), "cell": (12, 8), "order": 20, "role": "prop"},
    {"name": "CaveRockB", "texture": "assets/art/props/wilds_boulder.png",
     "size": (48, 32), "cell": (6, 1), "order": 20, "role": "prop"},
    # 雪莲与矿脉是**采集物**（`assets/art/gather`，不在 `set_cast_shadows.GROUND_DIRS` 里 ⇒
    # 永远不配投影贴图）：order 22 压在道具带上，与旧版同档。
    {"name": "SnowLotus", "texture": "assets/art/gather/wilds_snow_lotus.png",
     "size": (32, 32), "cell": (15, 6), "order": 22, "role": "prop"},
    {"name": "CaveOreVein", "texture": "assets/art/gather/wilds_ore.png",
     "size": (40, 32), "cell": (8, 9), "order": 22, "role": "prop"},
    {"name": "CaveCrate", "texture": "assets/art/props/wilds_crate.png",
     "size": (32, 28), "cell": (13, 9), "order": 20, "role": "prop"},
]

# 登记在**相机之后**的那批实体（旧版是手工 `Edit` 追加到文件尾的，于是重跑旧生成器就把它们
# 整段抹掉，而盘上没有任何一张图会报错，只是 `interior_stove.png` 从"被引用"变成孤儿）。
# id 顺序是盘上事实：13 = Stove、14 = CaveShrine（本批新增）。
EXTRAS = [
    # `Stove` 在资源接线清单里的作用是让家具图集有一处真实引用（docs/48），不是装饰。
    # `role=furniture` **不参与占地**（`building_kit` 只判 `BUILDING_ROLES` 与 `ROOT_ROLES`），
    # 所以它是"看得见却穿得过"的那一类：48px 宽的炉子只压在锚点那一格之外还能走过去。
    # 这条本批不动它（改成 `prop`+`box` 会顺手把 Boss 走道 (10,6) 那格标成实心），登记给室内
    # 图那批一起收（`interior_forge` / `interior_herb` 的家具才是它的正牌落点）。
    {"name": "Stove", "texture": "assets/art/furniture/interior_stove.png",
     "size": (48, 40), "cell": (10, 6), "order": 10, "role": "furniture"},
    # 石龛（本批新增，理由见模块 docstring「这张图必须有一件真建筑」）：借村里土地庙那张
    # 64×64，`layer_kit.LAYER_SPLITS["village_shrine"]` 已有拆点（顶 29 行进 foreground）⇒
    # 遮段件 `CaveShrineRoof` 由 `set_foreground_occluders` 那条路径挂上，本文件不写它的行。
    # 总高 64 ⇒ reach 7.68 < 8 ⇒ **不配投影**，与村里三栋矮屋走同一条判据（不是名单）。
    {"name": "CaveShrine", "texture": "assets/art/buildings/village_shrine.png",
     "size": (64, 64), "cell": (16, 6), "order": 0, "role": "building"},
]

HUD_TITLE = "寒泉洞 · 第一章线索"
HUD_HINT = "WASD 移动 · E 交互(采集/出口) · T 睡觉 · Q 吐纳 · J 普攻 · K 武学"
# 这两行 UiLayout 的数值以**盘上场景**为准（旧版 HUD 是手工调过的那一档，重跑不得弹回）。
HUD_TITLE_OFFSETS = "24.00,20.00,672.00,44.00"
HUD_HINT_OFFSETS = "24.00,50.00,672.00,72.00"


def rect(region):
    x0, y0, x1, y1 = region
    return [(x, y) for y in range(y0, y1 + 1) for x in range(x0, x1 + 1)]


def entity_lines_of(start_id, entries):
    """一批 sprite → 每件五行（实体名/Sprite/Transform/role/cell），返回 (行, 下一个 id)。

    `build_scene` 与 `occupied_cells` 都走这里：算碰撞用的那批必须与写进场景的那批是同一批，
    否则"按图算的占地"与"画出来的房子"会各算各的。
    """
    lines = []
    next_id = start_id
    for entry in entries:
        cell_x, cell_y = entry["cell"]
        world_x = cell_x * TILE + entry.get("cell_dx", 0)
        world_y = cell_y * TILE + entry.get("cell_dy", 0)
        width, height = entry["size"]
        lines.append(f"entity\t{next_id}\t{entry['name']}")
        lines.append(sprite_line(next_id, entry["texture"], width, height,
                                 entry.get("layer", "world"), entry["order"]))
        lines.append(transform_line(next_id, float(world_x), float(world_y)))
        lines.append(f"entity_meta\t{next_id}\trole\t{entry['role']}")
        lines.append(f"entity_meta\t{next_id}\tcell\t{cell_x},{cell_y}")
        next_id += 1
    return lines, next_id


def structural_lines():
    """盘上 id 2..8 的那批道具（在玩家之前），与 `occupied_cells` 同源。"""
    return entity_lines_of(2, ENTITIES)[0]


def occupied_cells(ground):
    """每件建筑/道具该实心哪些格——**只认 `building_kit` 这一条规则**。

    拆层要先过一遍 `fo.plan`：占地是"本体∪同名遮断件"的不透明格并集，石龛屋顶那两格由它
    决定——少了这一步，屋檐就会判成"悬空碰撞"而被删掉实心（玩家可以走进石龛里）。

    第三个返回值 `{建筑名: {拆层件名: 覆盖格}}` 是给自检 6 的：`visual_and_solid` 的读数只给
    **格数**不给格子，而那条自检要的是"屋顶自己那几格压在什么地表上"。这里按件分开算，用的
    还是 `building_kit` 那一条 `cells_covered(opaque_world_rect(...))`，不是第二套算法。
    """
    lines, _ = entity_lines_of(2, ENTITIES)
    extras, _ = entity_lines_of(13, EXTRAS)
    lines = lines + extras
    updates, occluders = fo.plan(lines, fo.lk.load())
    patched = fo.apply_patch(lines, updates, occluders)
    sprites = bk.placed_sprites(bk.read_entities(patched))
    cells, readings, _ents = bk.visual_and_solid(bk.project_root(), sprites, W, H, SCENE)
    native = {(x, y) for y in range(H) for x in range(W)
              if ground[y][x] in bk.TILE_NATIVE_SOLID}
    return cells | native, readings, mate_cells(sprites)


def mate_cells(sprites):
    """每件 `role=building` 的**拆层件各自**覆盖哪几格（本体不算在内）。

    配对规则与 `building_kit.visual_cells` 一字不差：实体名前缀、不按 role 筛、排除
    `decor`/`cast_shadow`。抄这条是为了让自检 6 能单独看屋顶，而不是看并集。
    """
    root = bk.project_root()
    out = {}
    for item in sprites:
        if item["role"] not in bk.BUILDING_ROLES or item["layer"] == bk.OCCLUDER_LAYER:
            continue
        mates = {other["name"]: bk.cells_covered(bk.opaque_world_rect(root, other))
                 for other in sprites
                 if other is not item and other["role"] not in ("decor", "cast_shadow")
                 and other["name"].startswith(item["name"]) and other["name"] != item["name"]}
        if mates:
            out[item["name"]] = mates
    return out


def build_solid(ground):
    """solid 布尔矩阵：`occupied_cells` 的格集换算回来。阻挡行走、且每格都有可视依据。"""
    cells, readings, mates = occupied_cells(ground)
    return [[1 if (x, y) in cells else 0 for x in range(W)] for y in range(H)], readings, mates


def build_tiles():
    """地表 tile 值矩阵 tiles[y][x]（**抖动前**的形状：这里只画分区，犬牙交给 `jitter`）。

    全场不铺 kind 2（塌落碎石）是硬契约，见模块 docstring 的 farm.lua 那条。
    """
    ground = [[FLOOR] * W for _ in range(H)]
    for region in (ENTRANCE, CORRIDOR, CHAMBER):
        for (x, y) in rect(region):
            ground[y][x] = HEWN
    for (x, y) in POND_CELLS:
        ground[y][x] = WATER
    for (x, y) in ICE_CELLS:
        ground[y][x] = ICE
    # 暗河砌岸：水格的 4 邻域里**仍是洞底石板**的那些刷成凿平石（只认 FLOOR，所以既不会碰
    # 甬道已有的石板、也不会碰冰面与墙）。与山庄/演武场同一理由：水↔石板直接相邻时 autotile
    # 在岸上读成"一摊黑浮在灰布上"，中间必须垫一档硬质地表；洞里则是"人围着泉眼凿过一圈"。
    for (x, y) in POND_CELLS:
        for nx, ny in ((x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)):
            if 0 <= nx < W and 0 <= ny < H and ground[ny][nx] == FLOOR:
                ground[ny][nx] = HEWN
    # 洞壁最后写：地图边界与岩凸都是墙，任何分区（包括砌岸）都不许把墙改成别的 kind。
    for x in range(W):
        ground[0][x] = ROCK
        ground[H - 1][x] = ROCK
    for y in range(H):
        ground[y][0] = ROCK
        ground[y][W - 1] = ROCK
    for (x, y) in OUTCROPS:
        ground[y][x] = ROCK
    return ground


def decor_reserved():
    """M72-05 的玩法留白：返回 (两带共同的硬留白, 只挡中景带的留白)。

    输入全部来自本文件的布局常量与 `data/{places,enemies}.lua`，所以
    `set_scatter_decor.py` 只要 import 这里，两条路就必然撒出同一段装饰。

    **硬留白是空集**：这张图没有农田（kind 2 全图禁用）。会挡视线的是**中景带**（order 12 的
    碎石/孤石），所以出生格、出口格、四格地点、Boss 的 home 与每件道具的锚点都进 `mid_avoid`
    ——240 格的小图里中景一共没几件，把它压在交互点上是最亏的写法。
    """
    props = [entry["cell"] for entry in ENTITIES + EXTRAS]
    homes = [cell for _, cell in read_enemy_homes()]
    places = [(x, y) for _, x, y in read_place_cells()]
    keys = [SPAWN, *EXITS.values()]
    return set(), dk.mid_avoid(W, H, props + homes, places, keys)


def surface(ground, solid):
    """抖动 + tint 撒布（**一个函数、两条出口**）。

    `build_scene` 用它产出写盘的那份载荷，`main` 也用它取"抖动后的格子图"跑自检——契约必须
    测在**盘上真值**上。与 `set_terrain_autotile.py` 用同一套函数，所以"重跑生成器"与
    "就地重排盘上场景"得到同一个字节序列。
    """
    solid_str = row_major(solid)
    cells = [ground[y][x] for y in range(H) for x in range(W)]
    cells, changed = jitter(cells, solid_str, W, H, freeze_cells(FREEZE, W, H))
    table = tk.load_ramp(SCENE)
    tint_ids = [tint_id for tint_id, _ in tk.ground_tints(table)]
    tints = scatter_tints(W, H, tint_ids)
    # 河心按**形状**压一档 `shaded`（表里第 1 档是那档最冷的）：暗河读深度的机制与山庄池塘
    # 同一条——autotile 开着时四邻皆同类的内部格全部落进同一个图集条目，换瓦要动色阶表与
    # 图集布局，而现成的那一档冷色能立刻用（代价是深度差只有 3-4%）。
    tints.update(interior_tints(cells, W, H, tk.kind_id(table, WATER_NAME), tint_ids[0]))
    return cells, tints, solid_str, changed


def cell_to_world(cx, cy):
    """cell 左下角世界坐标（y 朝上，row 0 在最底）"""
    return (cx + 0.5) * TILE, (cy + 0.5) * TILE


def row_major(grid):
    return "".join(str(grid[y][x]) for y in range(H) for x in range(W))


def to_matrix(cells):
    """row-major 串 → `ground[y][x]` 矩阵（与 `row_major` 互为逆，只此一处索引换算）。"""
    return [cells[y * W:(y + 1) * W] for y in range(H)]


def transform_line(entity_id, x, y):
    # M32：统一的 Transform 形态由 `pixel_kit.transform_line` 单点产出
    # （引擎读侧只认 position/rotation/scale，2D 旧形态已删除；见该函数注释）。
    from pixel_kit import transform_line as spawn_transform

    return spawn_transform(entity_id, x, y)


def sprite_line(entity_id, texture, width, height, layer, order):
    return (
        f"component\t{entity_id}\tSprite\ttexture={texture};size={width}x{height};"
        f"pivot=0.000,0.000;tint=1.000,1.000,1.000,1.000;sorting_layer={layer};order={order}"
    )


def build_scene(ground, solid):
    cells, tints, solid_str, _changed = surface(ground, solid)
    table = tk.load_ramp(SCENE)
    lines = [
        "scene_version=2",
        # 场景名必须等于地图键（data/maps.lua 的 `cold_spring_cave`）：main.lua 用
        # `is_playable(scene.reload())` 判定玩法地图，写错就整局进不去。
        f"name={SCENE}",
        "meta\tdisplay_name\t江湖有田·寒泉洞",
        "meta\tgame\tjianghu_youtian",
        "meta\ttile_size\t32",
        # 横屏方案A：设计分辨率 1280×720（与 `scripts/ui/layout.lua` 一致）。
        "meta\tui_reference_height\t720",
        "meta\tui_reference_width\t1280",
        "entity\t1\tGroundMap",
        transform_line(1, 0.0, 0.0),
        # 逻辑瓦 + autotile + per-tile tint（口径见 `terrain_kit.py` 与 M72-04）。旧版这里是
        # 6 列 × 1 行的**恒等寻址**，用的是 `tilesets/cave/cave_ground.png` 那张借用图。
        "component\t1\tTilemap\t"
        + tk.tilemap_component(TILE, W, H, GROUND_TEXTURE, cells, solid_str,
                               table["cols"], table["rows"],
                               tile_kinds=table["kind_count"], tints=tints,
                               tints_rgb=tk.tints_rgb_string(table)),
        "entity_meta\t1\trole\tground_tilemap",
    ]
    # 2..N 道具：与 `occupied_cells` 走同一个 `entity_lines_of`，占地算的就是画出来的这批。
    lines.extend(structural_lines())
    # 玩家本体：场景内可碰撞动态刚体（脚本经 entity.find("JianghuPlayer") 使用）
    player_id = 9
    spawn_wx, spawn_wy = cell_to_world(*SPAWN)
    lines.append(f"entity\t{player_id}\tJianghuPlayer")
    # 主角形象（M21）：单帧 + 序列帧都从角色美术 manifest 取，与 data/characters.lua 是
    # 同一套串；运行期 player.apply() 会按 state.appearance 纠正并重挂，此处给默认形象。
    player_sprite, player_animation = player_art("player_male")
    lines.append(f"component\t{player_id}\tSprite\t{player_sprite}")
    if player_animation:
        lines.append(f"component\t{player_id}\tAnimation\t{player_animation}")
    lines.append(f"component\t{player_id}\tCollider\tshape=ball;radius=12;offset=0,-16")
    lines.append(transform_line(player_id, spawn_wx, spawn_wy))
    lines.append(f"entity_meta\t{player_id}\trole\tplayer_spawn")
    lines.append(f"entity_meta\t{player_id}\tpersistent\ttrue")
    title_id = 10
    lines.append(f"entity\t{title_id}\tHudTitle")
    lines.append(
        f"component\t{title_id}\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;"
        f"offsets={HUD_TITLE_OFFSETS};min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000"
    )
    lines.append(
        f"component\t{title_id}\tText\ttext={HUD_TITLE};font_size=18.00;color=0.949,0.918,0.847"
    )
    lines.append(transform_line(title_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{title_id}\trole\thud_title")
    hint_id = 11
    lines.append(f"entity\t{hint_id}\tHudHint")
    lines.append(
        # 地图自带操作提示放在**地图标题正下方**；底部信息区整块留给 HUD 的动态反馈/键位行。
        f"component\t{hint_id}\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;"
        f"offsets={HUD_HINT_OFFSETS};min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000"
    )
    lines.append(
        f"component\t{hint_id}\tText\ttext={HUD_HINT};font_size=14.00;color=0.850,0.900,1.000"
    )
    lines.append(transform_line(hint_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{hint_id}\trole\thud_hint")
    # 场景相机（本地图唯一的 `Camera` 实体）：把 2D 取景范围锁在整张地图的矩形内，
    # 运行期由 `scripts/world/camera_view.lua` 让相机跟随玩家（`camera2d.follow`）。
    # 几何与理由见 `pixel_kit.camera_line`；`validate_all` 会在启动/`--check` 时核对
    # "场景 bounds == 地图尺寸"。这张图 20×12 ⇒ bounds 640×384 = **地图就是一屏**。
    camera_id = 12
    lines.append(f"entity\t{camera_id}\tGameCamera")
    lines.append(camera_line(camera_id, W, H))
    lines.append(transform_line(camera_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{camera_id}\trole\tgame_camera")
    # 13.. 相机之后那批（Stove / CaveShrine）：**id 与旧版盘上一致**，装饰因此从 15 起
    # （`set_scatter_decor.next_id` 取的是"盘上最大非保留 id + 1"，两条路必须给同一个数）。
    extras_lines, decor_start = entity_lines_of(13, EXTRAS)
    lines.extend(extras_lines)
    # M72-05：装饰散布。输入用**抖动后**的格子图（盘上就是这个），不然"生成器看到的
    # 洞底石/凿平石分布"与实机不一致。本图的生态档（排除草皮/花/香蒲/睡莲）在
    # `decor_kit.SCENE_DECOR[SCENE]`，靠 `scene=SCENE` 生效。
    jittered = to_matrix(cells)
    exclude, avoid = decor_reserved()
    placements = dk.scatter(W, H, jittered, solid, exclude, mid_avoid=avoid, scene=SCENE)
    lines.extend(dk.decor_lines(decor_start, placements, table,
                                lambda cell: tints.get(cell[1] * W + cell[0], 0)))
    # M72-06：前景遮断拆层（石龛屋顶）。与 `set_foreground_occluders.py`（就地改盘上场景）
    # **共用同一条 `plan()`+`apply_patch()`**：这一段的存在感极低（拼回去画面逐字节相同），
    # 一旦重跑生成器把它抹掉，盘上只是"前景没了"，没有任何一张图会报错。
    fo.assert_id_band_free(lines)
    base_updates, occluders = fo.plan(lines, fo.lk.load())
    lines = fo.apply_patch(lines, base_updates, occluders)
    # M72-06 C 段：地面投影贴片。必须在 `fo.apply_patch` **之后**——贴片按拆层后的本体贴图算
    # 落地行，也按拆层后的实体排除遮断块（空中块不配自己的投影）。本图够格件数为 0（判据见
    # 模块 docstring），所以这里追加的是**空段**；跑这条路径而不是跳过它，`--check` 才测得到
    # "空段与重算逐字节相同"这件事。
    cs.assert_id_band_free(lines)
    shadows, canvases = cs.build(lines, cs.lk.load(), cs.load_art(lines))
    lines = cs.apply_patch(lines, cs.shadow_lines(shadows))
    cs.write_art(canvases, ".")
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------
# 契约自检
# ---------------------------------------------------------------------------


def reachable_from(solid, start):
    """从 start 起在可走格上做 4 邻域 flood fill。"""
    seen = {start}
    stack = [start]
    while stack:
        x, y = stack.pop()
        for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            nx, ny = x + dx, y + dy
            if 0 <= nx < W and 0 <= ny < H and not solid[ny][nx] and (nx, ny) not in seen:
                seen.add((nx, ny))
                stack.append((nx, ny))
    return seen


def pockets(solid):
    """进不去的可走格块（除最大连通块以外的那些）。

    与山道/演武场同口径：一张图里"看得见但走不进去"的洞底石板不会让任何一条锚点断言变红，
    只有连通块分解能把它摆出来。洞穴尤其要紧——这张图的可走面积本来就只有 240 格的一小半，
    一块进不去的角落等于白画一片地表。
    """
    seen, comps = set(), []
    for y in range(H):
        for x in range(W):
            if solid[y][x] or (x, y) in seen:
                continue
            stack, comp = [(x, y)], [(x, y)]
            seen.add((x, y))
            while stack:
                cx, cy = stack.pop()
                for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    nx, ny = cx + dx, cy + dy
                    if 0 <= nx < W and 0 <= ny < H and not solid[ny][nx] and (nx, ny) not in seen:
                        seen.add((nx, ny))
                        comp.append((nx, ny))
                        stack.append((nx, ny))
            comps.append(comp)
    comps.sort(key=len, reverse=True)
    return comps[0], comps[1:]


def read_place_cells(path=os.path.join(PROJECT_ROOT, "scripts", "data", "places.lua")):
    """`data/places.lua` 里属于本图的地点格（`[(名字, x, y)]`）。

    读数据侧那一份而不是在本文件再抄四个点：改 places.lua 而生成器不知道，症状是
    "NPC 地点落在一堵墙里"而不是报错。
    """
    if not os.path.exists(path):
        raise FileNotFoundError(f"{path} 读不到：地点格是本图可达性自检的输入")
    cells = []
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            match = re.match(
                r'\s*(\w+)\s*=\s*\{\s*map\s*=\s*"' + SCENE +
                r'"\s*,\s*cell\s*=\s*\{\s*x\s*=\s*(\d+)\s*,\s*y\s*=\s*(\d+)\s*\}',
                line,
            )
            if match:
                cells.append((match.group(1), int(match.group(2)), int(match.group(3))))
    return cells


def read_enemy_homes(path=os.path.join(PROJECT_ROOT, "scripts", "data", "enemies.lua")):
    """`data/enemies.lua` 里本图敌人的 `home` 格（`[(name, cell)]`）。

    三条记录（首领 + 两只召唤幼崽槽位）的 home 全是同一格 (10,6)——幼崽的 home 只是初始值
    （召出时按 `summon_offset` 相对主人算），但**它照样要可走**：`scene_spawn` 是按 home 落
    刚体的，那格若是实心，二阶段召出来的就是两只卡墙里的幼崽。
    """
    if not os.path.exists(path):
        raise FileNotFoundError(f"{path} 读不到：home 格是本图可达性自检的输入")
    out, pending = [], None
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            hit = re.match(r'\s*name\s*=\s*"([^"]+)"', line)
            if hit:
                pending = {"name": hit.group(1), "cave": False}
                continue
            if pending is None:
                continue
            if re.match(r'\s*map\s*=\s*"' + SCENE + r'"', line):
                pending["cave"] = True
            home = re.match(r'\s*home\s*=\s*\{\s*x\s*=\s*(\d+)\s*,\s*y\s*=\s*(\d+)\s*\}', line)
            if home and pending["cave"]:
                out.append((pending["name"], (int(home.group(1)), int(home.group(2)))))
                pending = None
    return out


def read_cave_registry(path=os.path.join(PROJECT_ROOT, "scripts", "data", "maps.lua")):
    """从 data/maps.lua 读 cold_spring_cave 的 spawn/出口格，做三方一致性校验（不执行 Lua）。"""
    if not os.path.exists(path):
        raise FileNotFoundError(f"{path} 读不到：spawn/出口格是三方一致契约的另一侧")
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    marker = text.find(f"{SCENE} = {{")
    if marker < 0:
        raise SystemExit(f"data/maps.lua 里找不到 {SCENE} 注册块")
    chunk = text[marker:]
    end = chunk.find("\n  }")
    if end >= 0:
        chunk = chunk[:end]

    def cell_of(pattern, label):
        match = re.search(pattern, chunk)
        if not match:
            raise SystemExit(f"data/maps.lua:{SCENE} 解析不到 {label}")
        return int(match.group(1)), int(match.group(2))

    return {
        "spawn": cell_of(r"spawn\s*=\s*\{\s*x\s*=\s*(\d+)\s*,\s*y\s*=\s*(\d+)", "spawn"),
        "exit": cell_of(r"cell\s*=\s*\{\s*x\s*=\s*(\d+)\s*,\s*y\s*=\s*(\d+)", "exit cell"),
    }


def check_texture_sizes():
    """交叉校验：scene 里声明的 sprite 尺寸必须与贴图像素尺寸一致。

    引擎会把贴图缩放到 `size=` 指定的尺寸，声明错了不会报错、只会被悄悄拉伸。
    """
    from pixel_kit import decode_png

    problems = []
    for entry in ENTITIES + EXTRAS:
        path = os.path.join(PROJECT_ROOT, *entry["texture"].split("/"))
        if not os.path.exists(path):
            problems.append(f"{entry['name']}: 贴图缺失 {entry['texture']}")
            continue
        width, height, _, _ = decode_png(path)
        want_w, want_h = entry["size"]
        if (width, height) != (want_w, want_h):
            problems.append(
                f"{entry['name']}: 贴图 {width}x{height} != scene 声明 {want_w}x{want_h}"
                f"（会被引擎拉伸）—— 改 {entry['texture']} 或本文件 size"
            )
    return problems


def shadow_readings():
    """每件"站在地面上"的实体的投影够格读数（总高 × `shadow_reach` vs `min_reach_units`）。

    这里**只打印不断言**：够不够格由 `lighting.lua` 那两个数与 `set_cast_shadows` 那条路径
    决定，本文件不写第二套判据。打这一行是为了让"洞穴没有投影"这件事在日志里是**一个读数**
    而不是一句散文（plan §推进顺序 4 要的答案就是这个数）。
    """
    src = lk.load()
    min_reach = float(src["cast_shadow"]["min_reach_units"])
    rows = []
    for entry in ENTITIES + EXTRAS:
        texture = entry["texture"]
        if posix_dir(texture) not in cs.GROUND_DIRS:
            continue
        reach = lk.shadow_offset(entry["size"][1], src)
        length = (reach[0] ** 2 + reach[1] ** 2) ** 0.5
        rows.append((entry["name"], entry["size"][1], length, length >= min_reach))
    return rows, min_reach


def posix_dir(texture):
    return texture.replace("\\", "/").rsplit("/", 1)[0]


def self_check(ground, solid, walk, mate_map):
    """契约自检：锚点可走可达、碎石零格、石龛的拆层真的有碰撞后果、三方一致。

    `ground` 是**抖动后**的格子图（调用方传 `surface()` 的结果），`mate_map` 是
    `occupied_cells` 给的 `{建筑名: {拆层件名: 覆盖格}}`。
    """
    problems = []
    # 1) kind 值域：地表层只许 1..6（越界就是引擎按条目号切 UV 的那一类错位画面）。
    for y in range(H):
        for x in range(W):
            if ground[y][x] < 1 or ground[y][x] > 6:
                problems.append(f"({x},{y}) kind={ground[y][x]} 超出 1..6")
    # 2) farm.lua 防误垦：全场禁止碎石（kind 2 在 FARM 矩形交集里有 48 格，见模块 docstring）。
    scree = [(x, y) for y in range(H) for x in range(W) if ground[y][x] == SCREE]
    if scree:
        problems.append(f"出现塌落碎石 {len(scree)} 格（首格 {scree[0]}）："
                        "farm.lua 的 FARM 矩形没有场景门禁，tile==2 就被当可开垦田")
    # 3) 契约锚点：可走 + 从出生点可达。
    boss_homes = [cell for _, cell in read_enemy_homes()]
    anchors = {"spawn": SPAWN, "exit": EXITS["cave_to_mountain"]}
    anchors.update({name: (x, y) for name, x, y in read_place_cells()})
    for index, cell in enumerate(boss_homes):
        anchors[f"boss_home" if index == 0 else f"whelp_slot_{index}"] = cell
    for name, (x, y) in anchors.items():
        if solid[y][x]:
            problems.append(f"锚点 {name} ({x},{y}) 是 solid")
        elif (x, y) not in walk:
            problems.append(f"锚点 {name} ({x},{y}) 从出生点不可达")
        elif name in HARD_PATH_ANCHORS and ground[y][x] != HEWN:
            problems.append(f"锚点 {name} ({x},{y}) 地表 kind={ground[y][x]}，"
                            "走线三格（出生/出口/Boss home）应为凿平甬道(3)")
    # 4) 出口格必须在洞口厅里：厅被挪走而 maps.lua 没跟着改 = 一脚踏进洞壁。
    if set(EXITS.values()) - {(x, y) for (x, y) in rect(ENTRANCE)}:
        problems.append(f"出口格 {sorted(EXITS.values())} 不在洞口厅 {ENTRANCE} 里")
    # 5) 建筑/道具锚点（贴图左下角那一格）不许是**水**：锚点在水里就是整件泡在暗河里。
    for entry in ENTITIES + EXTRAS:
        cx, cy = entry["cell"]
        if ground[cy][cx] == WATER:
            problems.append(f"{entry['name']} 的锚点 ({cx},{cy}) 在水格上：整件泡在暗河里")
    # 6) 石龛的拆层必须有碰撞后果：屋顶那几格要落在**非天然实心**的地表上。否则
    #    `jhyt_m72_building_footprint` 的反面对照 B 空转（演武场山林带那条同一个坑）。
    shrine = next(entry for entry in EXTRAS if entry["name"] == "CaveShrine")
    roof_cells = sorted((cell for mates in mate_map.get(shrine["name"], {}).values()
                         for cell in mates
                         if ground[cell[1]][cell[0]] not in bk.TILE_NATIVE_SOLID))
    if not roof_cells:
        problems.append("CaveShrine 的拆层件压在墙/水行上或根本没有覆盖格：摘掉它一格碰撞都不少，"
                        "反面对照 B 会空转——把石龛挪到可走石板旁边")
    # 7) 暗河与冰面都得真有格子（形状常量与分区写错时最容易"看着填了其实没填"）。
    if not any(ground[y][x] == WATER for (x, y) in rect(POND)):
        problems.append("暗河外接框里一格水都没有：POND_ROWS 被挪出外接框了")
    if not any(ground[y][x] == ICE for (x, y) in rect(ICE_BOX)):
        problems.append("冰晶地外接框里一格冰都没有：ICE_ROWS 被挪出外接框了")
    # 8) 暗河要连成一摊：相邻两行必须有公共列（否则是两个水坑，岸线读起来是断的）。
    rows = sorted(POND_ROWS)
    for a, b in zip(rows, rows[1:]):
        if not set(POND_ROWS[a]) & set(POND_ROWS[b]):
            problems.append(f"暗河 y{a} 与 y{b} 两行没有公共列：水断成两摊")
    # 9) maps.lua 三方一致（spawn / 出口格）。
    registry = read_cave_registry()
    if registry["spawn"] != SPAWN:
        problems.append(f"maps.lua spawn={registry['spawn']} 与本文件 SPAWN={SPAWN} 不一致")
    if registry["exit"] != EXITS["cave_to_mountain"]:
        problems.append(f"maps.lua 出口格={registry['exit']} 与本文件 "
                        f"cave_to_mountain={EXITS['cave_to_mountain']} 不一致")
    # 10) 贴图尺寸。
    problems.extend(check_texture_sizes())
    return problems


def walkable_summary(solid):
    """每行可走列的紧凑描述，用于文档同步。"""
    lines = []
    for y in range(H - 1, -1, -1):
        ranges = []
        start = None
        for x in range(W):
            if not solid[y][x]:
                if start is None:
                    start = x
            else:
                if start is not None:
                    ranges.append((start, x - 1))
                    start = None
        if start is not None:
            ranges.append((start, W - 1))
        text = " ".join(f"{a}" if a == b else f"{a}-{b}" for a, b in ranges)
        lines.append(f"    y{y:>2}: {text if text else '(全实心)'}")
    return lines


def main():
    parser = argparse.ArgumentParser(description="从零装配 scenes/cold_spring_cave.scene")
    parser.add_argument("--out", default=os.path.join("scenes", "cold_spring_cave.scene"),
                        help="写出路径。对拍请指到 .temp 再与盘上场景逐行 diff。")
    args = parser.parse_args()
    ground = build_tiles()
    solid, readings, mates = build_solid(ground)
    text = build_scene(ground, solid)
    with open(args.out, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)
    jittered, tints, _solid_str, changed = surface(ground, solid)
    # `surface` 返回的是**写进场景的那串 row-major 载荷**，自检与读数要的是 2D 矩阵——
    # 同一份数据换一种索引，不是第二套真相。
    ground = to_matrix(jittered)
    print(f"wrote {args.out} ({W}x{H}, {len(ENTITIES)} 个道具 + {len(EXTRAS)} 件"
          f"（家具/石龛），id 2..{12 + len(EXTRAS)} 不动）")
    for reading in readings:
        print("   %-22s 视觉 %-3d 格 solid %-3d 格 门洞 %s%s" % (
            reading["name"], reading["visual"], reading["solid"], reading["openings"] or "-",
            "（拆层件：" + ",".join(reading["mates"]) + "）" if reading["mates"] else ""))
    for name, groups_map in sorted(mates.items()):
        for mate, cells in sorted(groups_map.items()):
            print(f"   {mate} 覆盖格 {sorted(cells)}")
    walk = reachable_from(solid, SPAWN)
    problems = self_check(ground, solid, walk, mates)
    if problems:
        for problem in problems:
            print("[FAIL]", problem)
        raise SystemExit(1)
    shadow_rows, min_reach = shadow_readings()
    anchors = {"spawn": SPAWN, "exit": EXITS["cave_to_mountain"],
               **{name: (x, y) for name, x, y in read_place_cells()}}
    print(f"契约自检 ok（测在**抖动后**载荷上，{changed} 格被抖）：锚点 "
          f"{' '.join(f'{name}({x},{y})' for name, (x, y) in anchors.items())} "
          f"全部可走、可达；无碎石；出口格在洞口厅；places.lua "
          f"{len(read_place_cells())} 格、enemies.lua {len(read_enemy_homes())} 个 home 可达；"
          f"maps.lua 一致")
    print("   投影够格读数（总高 × shadow_reach vs min_reach_units="
          f"{min_reach}）：" + " ".join(f"{name}({height}px={reach:.2f}{'' if ok else ' 不挂'})"
                                       for name, height, reach, ok in shadow_rows)
          + f"  够格 {sum(1 for row in shadow_rows if row[3])} 件")
    main_block, dead = pockets(solid)
    print(f"可行走连通块：主块 {len(main_block)} 格，进不去的小块 {len(dead)} 个 "
          + " ".join(f"{len(c)}格@{min(p[0] for p in c)},{min(p[1] for p in c)}" for c in dead))
    kinds = {}
    for row in ground:
        for value in row:
            kinds[value] = kinds.get(value, 0) + 1
    print(f"地表 kind 直方图（抖动后）{kinds}  solid {sum(sum(r) for r in solid)} 格  "
          f"tint 非白档 {len(tints)} 格")
    for label, rows, box in (("暗河", POND_ROWS, POND), ("冰晶地", ICE_ROWS, ICE_BOX)):
        cells = sum(len(xs) for xs in rows.values())
        area = (box[2] - box[0] + 1) * (box[3] - box[1] + 1)
        widths = [len(xs) for xs in rows.values()]
        print(f"   {label} {cells} 格 / 外接框 {area} 格 ⇒ 填充率 {cells / area:.3f}"
              f"（每行宽 {min(widths)}..{max(widths)} 格，不是方盒）")
    print("可行走格事实（文档同步用）：")
    for line in walkable_summary(solid):
        print(line)


if __name__ == "__main__":
    main()
