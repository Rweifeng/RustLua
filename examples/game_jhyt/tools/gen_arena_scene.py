# -*- coding: utf-8 -*-
"""《江湖有田》演武场场景生成（scenes/combat_arena.scene）——山庄练武场版。

用法（工程根目录 `examples/game_jhyt` 下）：

    python tools/gen_arena_scene.py                  # 覆盖盘上场景（默认路径）
    python tools/gen_arena_scene.py --out .temp/a.scene   # 对拍/试跑，不碰盘上场景

**本文件是这张图的唯一事实源**（M72-11 ④ 起）。这张图以前是七张里唯一没上金标管线的一张：
地表**借用**山庄图集（`manor/manor_ground.png`）、按**恒等寻址**只取六族满块档、
每件道具的 `solid_cells` 手填。现在与 `gen_manor_scene.py` / `gen_village_scene.py` /
`gen_mountain_scene.py` 同构——同一条代码路径（`building_kit` / `set_foreground_occluders`
/ `set_cast_shadows` / `decor_kit` / `set_terrain_autotile` 的抖动与撒布），所以
"重跑生成器"与"就地工具 `--check`"必须给出同一个字节序列。

**解图集的账要写清**（这次借图欠下的两笔，M72-11 ④ 才还）：
  * 演武场用的是**山庄的色**：`terrain_ramp.json` 给它登记 `combat_arena` 档之后，它的
    草/石板/院墙走自己那几族（courtyard_turf / worn_flagstone / grey_brick / drill_loam /
    water），与山庄的夯土色、院墙色不再是同一条 hex——改山庄调色不会再顺手改演武场。
  * 恒等寻址拿不到 autotile：满块档画的是"一整块材质"，坪与草的交界就是一条按格子对齐的
    直线（docs/71 §1 点名的"像 demo"头号成因）。开 `tile_kinds/autotile` 之后交界按 4-bit
    邻居掩码选过渡块，再叠 `jitter` 的犬牙。代价登记在 `scene_readings` 那一侧：
    autotile 开着时引擎**不查** `variant_for`，四邻皆同类的内部格全部落进同一个条目。

**演武坪为什么缩到 x2..18 × y2..11（旧版 x1..19 × y1..12）**：金标读数门判的是任意
20×12 窗口里**最坏**那一屏的可见外观种数（`scene_readings.MIN_SCREEN_TILES=24`）。
19×12 的坪放得进一整屏纯坪——那一片全是 mask=15 的内部格，三档 tint 最多撑 3 种；
金标 manor 实测最坏 25 种 / 12 个条目，余量本来就只有 1。坪缩一格之后，任何 20×12 窗口
都必须跨出坪外，就一定会吃到石↔草犬牙、院墙带与（东半屏）池塘岸线。
战斗几何一格没动（`ANCHORS` 全在坪内），这次改的是**坪外那一圈**从石板变成草地。

引擎 Tilemap 约定（scene_version=2，见 engine crates/rustlua-ecs/src/tilemap.rs）：
  tile_size=WxH;width=W;height=H;texture=PATH;tiles_format=1;atlas_cols=12;atlas_rows=9;
  tiles=<逗号分隔的 kind id，row-major，row 0 在最底>;solid=<row-major 0/1>
  开逻辑瓦时再追加 `tile_kinds=6;variant_count=1;autotile=1`（+ 稀疏 `tints`/`tints_rgb`）。
  * 地表层存的是 **kind id**（1..6），运行期由 `Tilemap::resolve_atlas_index` 按 4-bit
    邻居掩码解成 12×9 图集的条目（位序 bit0=y-1、bit1=x+1、bit2=y+1、bit3=x-1）；
  * 单格 32 世界单位，y 朝上；索引 index = y*width+x。
列映射（与 `terrain_ramp.json` 的 `scenes.combat_arena.kinds` 一致，**不可随意改**）：
  1=坪外草地 2=练武夯土 3=石板坪地 4=院墙 5=青瓦 6=池水
  编号语义与山庄/村庄/青云山道同一套（`building_kit.TILE_NATIVE_SOLID=(4,6)` 靠的就是它）。
  **kind 2 在这张图上一格都不铺**（见下面 farm.lua 那条）；kind 5 是屋顶语义、地面不用
  （manor 同样不用），但图集仍照 6 族 × 17 条目画满——布局不许按"用了哪几族"裁剪图集。

**战斗验收锚点（不可移动）**：headless 验收 `jhyt_m18_combat` 硬编码了演武场的战斗几何，
本工具每次生成都自检它们可走、可达、地表为石板：
  * 出生格 spawn (9,5)   → 世界 (304,176)，战败 R 重开落点（data/maps.lua）
  * 训练木人 home (14,5)，玩家试招站位 (13,5)（面向右普攻/武学/击杀验收）
  * 灰狼 home (2,9)，引敌站位 (6,8)（A* 追击/受击/战败验收，两点间必须可寻路）
改动锚点 = 三处同步：本文件、`data/maps.lua`、`engine/crates/game/examples/jhyt_m18_combat.rs`。
木人与灰狼的 home 由 `data/enemies.lua` 读进来判（与山道的 places/gather 同一类契约），
出口格由 `data/maps.lua` 读进来对拍——这两个数在本文件再抄一遍就成了第二套真相。

**farm.lua 防误垦**：farm.lua 的 FARM 矩形（x14..33, y4..13）没有场景门禁，判定"可开垦"
只看 GroundMap `tile==2`（夯土）。演武场全场禁止夯土 tile，避免玩家在演武坪里按 E 锄地。
这条现在是**抖动之后**测的（见 `surface`）：`JITTER_TARGETS=(1,2)` 只改写草/土，来源限
`JITTER_DONORS=(1,2,3)`——这张图一格 kind 2 都没有，"能当供体的夯土"不存在，抖完仍不可能
长出夯土。旧版在抖动前测（那时根本没有抖动），结论一样但证据弱一档。

**南门与院墙打架这件事**（旧版靠把坪画到 y1 绕开，没写下来；这次由数据说清）：
`TILE_NATIVE_SOLID` 含 WALL，所以门洞若压在墙行上就**永远实心**。现在
`GATE_ROW=(8,1,10,2)` 显式把门洞所在那一列（含立柱底面）刷成石板：
  * 门洞 (9,1)(9,2) 地表是石、非实心 ⇒ 可站可达，就是 `maps.lua` 的出口格；
  * 立柱 (8,1)(8,2)(10,1)(10,2) 由 `manor_gate` 的不透明像素实心（它是 `role=building`，
    走"本体不透明格 − 门洞"），与旧版手填 GATE_PILLARS 逐格相同；
  * 门楼 9 格里 (8,0)(9,0)(10,0) 压在 row0 院墙上：row0 本来就是墙（天然实心），
    所以"每格实心都有可视依据"在两侧同时成立，不存在悬空碰撞。

布局分区（cell，0-index，y 朝上，含端点）：
  外围院墙   ：row0 / row21..22 / col0 / col39（kind 4，天然实心）
  演武坪     ：x2..18, y2..11 石板，战斗锚点全部在坪内
  南门       ：manor_gate x8..10, y0..2，门洞 (9,1)(9,2) 可走 = 出口格 (9,1)
  器械廊     ：y13 草地一排：兵器架×2 / 草垛 / 演武旗×2，石板引道 x9..10
  东侧小径   ：x19..29, y7..8 石板，接茶水棚与水畔
  池塘       ：**按行手填的不规则岸线**（`POND_ROWS`，外接框 x30..36 × y14..17）+ 木栈桥
               ；水格外一圈草地刷成石板（"石砌岸"，与山庄同一理由）
  山林带     ：y18 一排老树/竹丛 + y19 一只山石（**为什么不是 y19 压北墙**：见 `ENTITIES`
               里山林带那条——树冠压在墙行上会让拆层变成碰撞的旁观者，门当场红）
"""
import argparse
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

# 工程根（`examples/game_jhyt`）。**事实输入**（maps.lua / enemies.lua 这类决定锚点与留白
# 的数据）按这里定位，不跟着 cwd 走：写死的相对路径会让工具在别的目录下静默少一份输入。
PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

from character_art import player_art  # noqa: E402  主角形象（与 manifest 同源）
import building_kit as bk  # noqa: E402  M72-07：建筑/道具碰撞的单一实现
import decor_kit as dk  # noqa: E402  M72-05：装饰散布的单一实现
import set_cast_shadows as cs  # noqa: E402  M72-06 C 段：地面投影贴片的单一实现
import set_foreground_occluders as fo  # noqa: E402  M72-06：前景遮断拆层的单一实现
import terrain_kit as tk  # noqa: E402  M72-04：逻辑瓦 + autotile + tint 的口径
from pixel_kit import camera_line  # noqa: E402  场景 2D 相机（取景锁在地图内）
from set_terrain_autotile import freeze_cells, interior_tints, jitter, scatter_tints  # noqa: E402

# 地图尺寸：与山庄/云泽村/青云山道一致（40×23 = 1280×736，盖过横屏一屏 720；
# 40×22 = 1280×704 会在屏幕顶/底露出 clear color 的深色横条）。
W, H = 40, 23
TILE = 32

# 场景键（= `name=` 头 = `data/maps.lua` 的地图键 = `terrain_ramp.json` 的 `scenes` 档名）。
# `building_kit` 的门洞表/横件碰撞表、色阶表与地表贴图路径都按它点名取用。
SCENE = "combat_arena"
GROUND_TEXTURE = tk.ground_texture(SCENE)

# 列映射（写 scene tile 用的 kind id，语义见模块 docstring；四场景共用，不可重排）
GRASS, DIRT, STONE, WALL, ROOF, WATER = 1, 2, 3, 4, 5, 6

# ---- 分区（(x0, y0, x1, y1)，含端点）----
YARD = (2, 2, 18, 11)           # 演武坪（石板；尺寸理由见"演武坪为什么缩"）
GATE_ROW = (8, 1, 10, 2)        # 南门那一列（门洞 + 立柱底面都必须是硬地，见 docstring）
SPUR = (9, 13, 10, 13)          # 器械廊引道（石板）
EAST_PATH = (19, 7, 29, 8)      # 东侧小径（石板）
# 池塘：**按行手填的不规则岸线**（对齐山庄 `POND_ROWS` / 山道 `STREAM_ROWS` 那条口径）。
# 形状读数：23 格，外接框 7×4=28 ⇒ 填充率 0.821（旧版 7×4 的完美方盒是 28/28=1.000）。
# 三条硬约束照抄山道：① 每行的 x 集都落在旧矩形 x30..36 之内（水只减不加 ⇒ 原本可走的
# 格一格都不会变成水）；② 相邻两行必须有公共列（池子要连成一摊，不许断成两个水坑）；
# ③ y14 保持 x31..35：栈桥 (31..33,14) 铺在它上面，水退出去就等于把桥头搁在岸上。
POND_ROWS = {
    14: (31, 32, 33, 34, 35),
    15: (30, 31, 32, 33, 34, 35, 36),
    16: (30, 31, 32, 33, 34, 35),
    17: (32, 33, 34, 35, 36),
}
POND_CELLS = {(x, y) for y, xs in POND_ROWS.items() for x in xs}
POND = (30, 14, 36, 17)         # 外接框（读数与自检用）
# 木栈桥的桥面（`ArenaPier` 96×32 正好压 x31..33 那一格高）：水面**不实心**，而桥面那三格
# 的地表写成石板，于是"桥"是真能踩的。这一步只**减少** solid（水本来实心，改石板变可走），
# 不可能因此断开任何一条通路。诚实度在这：`walk` 模式说的是"这件美术不挡人"，那地表就得
# 真让人走过去——旧版 Pier 写 `solid_cells=[]` 而底下三格仍是水，画面上是一座走不进去的桥。
PIER = (31, 14, 33, 14)
# 抖动冻结集：**空**。山庄冻的是药田（抖掉一格就少一格田），这张图没有可垦地；而战斗锚点
# 全是 kind 3，`JITTER_TARGETS=(1,2)` 根本不碰石板（"只加路不减路"），坪内不需要额外保护。
# 写成空集而不是删掉这条，是为了让"这里考虑过冻什么"可见。
FREEZE = []

# ---- 战斗验收锚点（与 jhyt_m18_combat.rs / data/maps.lua / data/enemies.lua 三方一致）----
SPAWN = (9, 5)                  # 出生/战败重开落点 (304,176)
EXITS = {"arena_to_main": (9, 1)}   # 出口格：南门门洞内（与 maps.lua 对拍）
ANCHORS = {
    "spawn": SPAWN,
    "exit": EXITS["arena_to_main"],
    "dummy_home": (14, 5),      # 训练木人 home（data/enemies.lua）
    "player_before_dummy": (13, 5),  # 试招站位（jhyt_m18_combat PLAYER_BEFORE_DUMMY）
    "wolf_home": (2, 9),        # 灰狼 home（data/enemies.lua）
    "wolf_lure": (6, 8),        # 引敌站位（jhyt_m18_combat WOLF_LURE）
}

# ---- 建筑 / 道具 ----
# texture：gen_manor_art.py / gen_village_art.py 的产物；size 与图片像素 1:1
# （`check_texture_sizes` 逐条核）；pivot=(0,0) 即 transform 为左下角；cell：贴图左下角所在 cell。
#
# **这里没有 `solid_cells` 字段了**（M72-07 的规矩，这张图是最后一张手填的）：占地只认
# `building_kit` 一条规则——建筑 = 本体∪同名拆层件的不透明像素覆盖格 − 门洞；道具 = `cell`
# 那一格，横着占两格的按 `building_kit.SCENES[SCENE].prop_collision` 点名走 `box`/`walk`。
# 名字/顺序/cell 与旧版逐条相同 ⇒ 盘上 id 2..28 一个都不动（挪 id 会让 Lua 侧
# `entity.find` 与黄金表全部重排）；新增的那件建筑**追加在表尾**（id 29），玩家顺延到 30。
ENTITIES = [
    # 南门（复用山庄门楼）：只挡立柱，门洞 (9,1)(9,2) 保持可走 = 出口格与门内格。
    # 门洞声明在 `building_kit.SCENES["combat_arena"].openings`，两边都有反查。
    {"name": "ArenaGate", "texture": "assets/art/buildings/manor_gate.png",
     "size": (96, 96), "cell": (8, 0), "order": 5, "role": "building"},
    # 南门内侧灯杆一对
    {"name": "ArenaLanternA", "texture": "assets/art/props/village_lantern.png",
     "size": (24, 64), "cell": (6, 1), "order": 10, "role": "prop"},
    {"name": "ArenaLanternB", "texture": "assets/art/props/village_lantern.png",
     "size": (24, 64), "cell": (12, 1), "order": 10, "role": "prop"},
    # 东侧小径口的告示木牌
    {"name": "ArenaSign", "texture": "assets/art/props/manor_sign.png",
     "size": (32, 42), "cell": (22, 9), "order": 10, "role": "prop"},
    # 器械廊（演武坪北侧 y13）：兵器架×2 + 草垛 + 演武旗×2（架子 64×46 横占两格 ⇒ `box`）
    {"name": "ArenaRackA", "texture": "assets/art/props/manor_rack.png",
     "size": (64, 46), "cell": (4, 13), "order": 10, "role": "prop"},
    {"name": "ArenaRackB", "texture": "assets/art/props/manor_rack.png",
     "size": (64, 46), "cell": (7, 13), "order": 10, "role": "prop"},
    {"name": "ArenaHaystackA", "texture": "assets/art/props/manor_haystack.png",
     "size": (36, 34), "cell": (12, 13), "order": 10, "role": "prop"},
    {"name": "ArenaBannerA", "texture": "assets/art/props/village_banner.png",
     "size": (32, 56), "cell": (15, 13), "order": 10, "role": "prop"},
    {"name": "ArenaBannerB", "texture": "assets/art/props/village_banner.png",
     "size": (32, 56), "cell": (18, 13), "order": 10, "role": "prop"},
    # 演武坪北缘旗杆一对（西北/东北角；南缘靠院墙不再立旗）
    {"name": "ArenaBannerC", "texture": "assets/art/props/village_banner.png",
     "size": (32, 56), "cell": (19, 12), "order": 10, "role": "prop"},
    {"name": "ArenaBannerD", "texture": "assets/art/props/village_banner.png",
     "size": (32, 56), "cell": (1, 12), "order": 10, "role": "prop"},
    # 石锁阵：坪东侧两只 + 西南一只 + 水畔一只（山石 64×40 横占两格 ⇒ `box`）
    {"name": "ArenaRockA", "texture": "assets/art/props/village_rock.png",
     "size": (64, 40), "cell": (17, 9), "order": 10, "role": "prop"},
    {"name": "ArenaRockB", "texture": "assets/art/props/village_rock.png",
     "size": (64, 40), "cell": (17, 6), "order": 10, "role": "prop"},
    {"name": "ArenaRockC", "texture": "assets/art/props/village_rock.png",
     "size": (64, 40), "cell": (3, 3), "order": 10, "role": "prop"},
    {"name": "ArenaRockD", "texture": "assets/art/props/village_rock.png",
     "size": (64, 40), "cell": (35, 13), "order": 10, "role": "prop"},
    # 东侧小径北缘：茶水棚 + 水缸 + 草垛（歇脚处）
    {"name": "ArenaStall", "texture": "assets/art/props/village_stall.png",
     "size": (64, 40), "cell": (26, 10), "order": 10, "role": "prop"},
    {"name": "ArenaJar", "texture": "assets/art/props/manor_jar.png",
     "size": (24, 30), "cell": (28, 10), "cell_dx": 4, "order": 10, "role": "prop"},
    {"name": "ArenaHaystackB", "texture": "assets/art/props/manor_haystack.png",
     "size": (36, 34), "cell": (24, 11), "order": 10, "role": "prop"},
    # 木栈桥（贴水面）：碰撞模式 `walk`，桥面三格的地表在 `build_tiles` 里刷成石板——
    # 见 `PIER` 那条注释。锚点在**水**上，所以显式 `over_water=True`（它就该铺在水上）。
    {"name": "ArenaPier", "texture": "assets/art/props/village_pier.png",
     "size": (96, 32), "cell": (31, 14), "order": 8, "role": "prop", "over_water": True},
    # 山林带（y18 一排；**这一排从 y19 挪下来一格是有门钉着的**，不是口味）：
    # `manor_tree` 88px = 2.75 格，base 在 y19 时树冠（顶 40 行）落在 y20 上半 + y21，
    # 而 y21/y22 是院墙（`building_kit.TILE_NATIVE_SOLID` 天然实心）、y20 下半又被树干那
    # 24px 覆盖 ⇒ 摘掉遮断件后**一格碰撞都不少**，`jhyt_m72_building_footprint` 的
    # 反面对照 B 就报"屋顶并进并集这条其实没在参与计算"。挪到 y18 后树冠独占 y20 那
    # 24px，拆层第一次真的在决定玩法；代价只是北墙前多出一条能走过去的草地，而这条
    # 读数（墙前有空隙）比"树与墙叠成一面"更好读。
    # 老树登记**整图**：`manor_tree` 在 `layer_kit.LAYER_SPLITS` 里，拆成树干 + 树冠遮断件
    # 是 `set_foreground_occluders` 那条代码路径的事（山庄/村庄同一条）。
    # 竹丛不拆：`LAYER_SPLITS` 里没有 `manor_bamboo`（竹梢不配当遮断件，山道六丛竹同判）。
    {"name": "ArenaTreeA", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (2, 18), "order": 20, "role": "prop"},
    {"name": "ArenaBambooA", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (5, 18), "order": 20, "role": "prop"},
    {"name": "ArenaBambooB", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (11, 18), "order": 20, "role": "prop"},
    {"name": "ArenaTreeB", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (15, 18), "order": 20, "role": "prop"},
    {"name": "ArenaRockN", "texture": "assets/art/props/village_rock.png",
     "size": (64, 40), "cell": (8, 19), "order": 20, "role": "prop"},
    {"name": "ArenaBambooC", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (25, 18), "order": 20, "role": "prop"},
    {"name": "ArenaTreeC", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (28, 18), "order": 20, "role": "prop"},
    {"name": "ArenaTreeD", "texture": "assets/art/props/manor_tree.png",
     "size": (64, 88), "cell": (37, 18), "order": 20, "role": "prop"},
    # 军械棚（M72-11 ④ 新增，**追加在表尾**：盘上 id 2..28 一个都不动，它拿 29、玩家顺延到 30）。
    # 这张图原来一件 `role=building` 都没有（南门是唯一的建筑、又不拆层），于是四棵树的
    # 遮断件在碰撞上**一格都不参与**——道具走的是"单格根碰撞"，树冠那几格本来就不实心。
    # `jhyt_m72_building_footprint` 的反面对照 B（"摘掉拆层件必须报出缺碰撞"）因此空转，
    # 门当场红：这不是门的口径错，是这张图确实没有"屋檐压住玩法"这件事。补一件真建筑
    # 既填了器械廊背后那片空草地，也让拆层在这张图上有可证的碰撞后果（山庄柴房同一张图、
    # 同一个拆点，`layer_kit.LAYER_SPLITS["manor_shed"]`）。
    {"name": "ArenaArmory", "texture": "assets/art/buildings/manor_shed.png",
     "size": (160, 128), "cell": (2, 14), "order": 0, "role": "building"},
]

HUD_TITLE = "山庄 · 演武场"
HUD_HINT = "WASD 移动 · E 交互(出口) · J 普攻 · K 武学 · R 战败重开"
# 这两行 UiLayout 的数值以**盘上场景**为准（与 manor 同一套坐标；M72-04 的对拍靠
# "生成器输出 == 盘上场景"这条成立，重跑生成器不得把 HUD 弹回旧版）。
HUD_TITLE_OFFSETS = "24.00,12.00,500.00,36.00"
HUD_HINT_OFFSETS = "24.00,40.00,560.00,64.00"


def rect(region):
    x0, y0, x1, y1 = region
    return [(x, y) for y in range(y0, y1 + 1) for x in range(x0, x1 + 1)]


def prop_entity_lines(start_id=2):
    """ENTITIES → (实体行, 下一个可用 id)。

    `build_scene` 与 `occupied_cells` 都调这一个函数：算碰撞用的那批 sprite 必须与写进
    场景的那批是同一批，否则"按图算的占地"与"画出来的房子"会各算各的。
    """
    lines = []
    next_id = start_id
    for entry in ENTITIES:
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


def occupied_cells(ground):
    """每件建筑/道具该实心哪些格——**只认 `building_kit` 这一条规则**。

    拆层要先过一遍 `fo.plan`：占地是"本体∪同名遮断件"的不透明格并集，而树冠那件在盘上是
    独立实体（M72-06），只喂未拆层的 ENTITIES 会少算树冠压出去的那一圈。
    """
    lines, _ = prop_entity_lines()
    updates, occluders = fo.plan(lines, fo.lk.load())
    patched = fo.apply_patch(lines, updates, occluders)
    sprites = bk.placed_sprites(bk.read_entities(patched))
    cells, readings, _ents = bk.visual_and_solid(bk.project_root(), sprites, W, H, SCENE)
    native = {(x, y) for y in range(H) for x in range(W)
              if ground[y][x] in bk.TILE_NATIVE_SOLID}
    return cells | native, readings


def build_solid(ground):
    """solid 布尔矩阵：`occupied_cells` 的格集换算回来。阻挡行走、且每格都有可视依据。

    旧版在这里把"每件 prop 手填的 solid_cells"叠上去——手填的串跟着 sprite 走，sprite 一挪
    坐标就在原地留下一片撞空气的悬空格（山庄那 24 格就是这么来的）。现在串是算出来的。
    """
    cells, readings = occupied_cells(ground)
    return [[1 if (x, y) in cells else 0 for x in range(W)] for y in range(H)], readings


def build_tiles():
    """地表 tile 值矩阵 tiles[y][x]（**抖动前**的形状：这里只画分区，犬牙交给 `jitter`）。

    全场不铺 kind 2（夯土）是硬契约，见模块 docstring 的 farm.lua 那条。
    """
    ground = [[GRASS] * W for _ in range(H)]
    for region in (YARD, GATE_ROW, SPUR, EAST_PATH):
        for (x, y) in rect(region):
            ground[y][x] = STONE
    # 池塘：不规则岸线按行填（形状见 `POND_ROWS`），水格天然实心（WATER 在
    # `building_kit.TILE_NATIVE_SOLID` 里），四邻皆水的内部格再压一档 `shaded` 顶点色
    # 读深度——见 `surface` 里的 `interior_tints`。
    for (x, y) in POND_CELLS:
        ground[y][x] = WATER
    # 石砌岸：水格的 4 邻域里**仍是草地**的那些刷成石板（只认 GRASS，所以既不会碰坪里
    # 已有的石板、也不会碰墙）。与山庄那条同一理由：水↔草直接相邻时 autotile 在岸上读成
    # "一摊蓝浮在绿布上"，中间必须垫一档硬质地表。
    for (x, y) in POND_CELLS:
        for nx, ny in ((x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)):
            if 0 <= nx < W and 0 <= ny < H and ground[ny][nx] == GRASS:
                ground[ny][nx] = STONE
    # 栈桥桥面：水面上铺石板，成为真能踩的三格（必须在写水之后，否则被水盖回去）。
    for (x, y) in rect(PIER):
        ground[y][x] = STONE
    # 整圈院墙（底部两行都是墙：H 22→23 时原下沿 H-2 保持墙，新行压在最底）。
    # 放在最后写：墙是地图边界，任何分区（包括石砌岸与桥面）都不许把墙改成别的 kind。
    for x in range(W):
        ground[0][x] = WALL
        ground[H - 2][x] = WALL
        ground[H - 1][x] = WALL
    for y in range(H):
        ground[y][0] = WALL
        ground[y][W - 1] = WALL
    return ground


def decor_reserved():
    """M72-05 的玩法留白：返回 (两带共同的硬留白, 只挡中景带的留白)。

    输入全部来自本文件的布局常量与 `data/enemies.lua`，所以 `set_scatter_decor.py` 只要
    import 这里，两条路就必然撒出同一段装饰。

    **硬留白是空集**：山庄那一块是药田（作物画在 FarmGrid 上，装饰一压上去就看不清哪块
    浇过），演武场没有农田；而**演武坪也不留白**——坪是石板地（kind 3），`decor_kit` 在
    石板上撒的是碎石/青苔（`BASE_P[3]=0.55`，M72-07 读作"老院子石板缝生苔"），碎屑带
    order -30 画在所有物件之前、连玩家也永远在它上面，压不住战斗可读性。真正会挡视线的是
    **中景带**（order 12 的灌木/孤石），所以出生点、出口格与两只敌人的 home 都进 `mid_avoid`。
    """
    props = [entry["cell"] for entry in ENTITIES]
    homes = [cell for _, cell in read_enemy_homes()]
    keys = [SPAWN, *EXITS.values()]
    return set(), dk.mid_avoid(W, H, props + homes, [], keys)


def surface(ground, solid):
    """抖动 + tint 撒布（**一个函数、两条出口**）。

    `build_scene` 用它产出写盘的那份载荷，`main` 也用它取"抖动后的格子图"跑自检——
    契约必须测在**盘上真值**上。`gen_mountain_scene.py` 把 self_check 喂在抖动前的 draft 上
    （那边没有能让 kind 2 长出来的路径，所以没暴露这个缺口），这里不抄那一档弱口径。
    与 `set_terrain_autotile.py` 用同一套函数，所以"重跑生成器"与"就地重排盘上场景"得到
    同一个字节序列。
    """
    solid_str = row_major(solid)
    cells = [ground[y][x] for y in range(H) for x in range(W)]
    cells, changed = jitter(cells, solid_str, W, H, freeze_cells(FREEZE, W, H))
    table = tk.load_ramp(SCENE)
    tint_ids = [tint_id for tint_id, _ in tk.ground_tints(table)]
    tints = scatter_tints(W, H, tint_ids)
    # 池心按**形状**压一档 `shaded`（表里第 1 档就是那档冷暗的）：桥面把 y14 那一排打断之后，
    # "四邻皆水"只剩 5 格，深度是"一段一段的渚"而不是一条均匀暗带——这正是要的效果。
    tints.update(interior_tints(cells, W, H, tk.kind_id(table, "water"), tint_ids[0]))
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
        # 场景名必须等于地图键（data/maps.lua 的 `combat_arena`）：main.lua 用
        # `is_playable(scene.reload())` 判定玩法地图，写错就整局进不去。
        "name=combat_arena",
        "meta\tdisplay_name\t江湖有田·演武场",
        "meta\tgame\tjianghu_youtian",
        "meta\ttile_size\t32",
        # 横屏方案A：设计分辨率 1280×720（与 `scripts/ui/layout.lua` 一致）。
        "meta\tui_reference_height\t720",
        "meta\tui_reference_width\t1280",
        "entity\t1\tGroundMap",
        transform_line(1, 0.0, 0.0),
        # 逻辑瓦 + autotile + per-tile tint（口径见 `terrain_kit.py` 与 M72-04）。旧版这里是
        # 恒等寻址（`tiles` 直接就是图集条目号、不开 autotile），借的就是山庄那张图集。
        "component\t1\tTilemap\t"
        + tk.tilemap_component(TILE, W, H, GROUND_TEXTURE, cells, solid_str,
                               table["cols"], table["rows"],
                               tile_kinds=table["kind_count"], tints=tints,
                               tints_rgb=tk.tints_rgb_string(table)),
        "entity_meta\t1\trole\tground_tilemap",
    ]
    # 2..N 建筑与道具：与 `occupied_cells` 走同一个 `prop_entity_lines`，
    # 所以占地算的就是画出来的这批。
    prop_lines, next_id = prop_entity_lines()
    lines.extend(prop_lines)
    # 玩家本体：场景内可碰撞动态刚体（脚本经 entity.find("JianghuPlayer") 使用）
    player_id = next_id
    spawn_wx, spawn_wy = cell_to_world(*SPAWN)
    lines.append(f"entity\t{player_id}\tJianghuPlayer")
    # 主角形象（M21）：单帧 + 序列帧都从角色美术 manifest 取，与
    # data/characters.lua 是同一套串；运行期 player.apply() 会按
    # state.appearance（男/女/未来的自定义）纠正并重挂，此处给的是默认形象。
    player_sprite, player_animation = player_art("player_male")
    lines.append(f"component\t{player_id}\tSprite\t{player_sprite}")
    if player_animation:
        lines.append(f"component\t{player_id}\tAnimation\t{player_animation}")
    lines.append(f"component\t{player_id}\tCollider\tshape=ball;radius=12;offset=0,-16")
    lines.append(transform_line(player_id, spawn_wx, spawn_wy))
    lines.append(f"entity_meta\t{player_id}\trole\tplayer_spawn")
    lines.append(f"entity_meta\t{player_id}\tpersistent\ttrue")
    next_id += 1
    title_id = next_id
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
    hint_id = next_id + 1
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
    # "场景 bounds == 地图尺寸"。
    camera_id = hint_id + 1
    lines.append(f"entity\t{camera_id}\tGameCamera")
    lines.append(camera_line(camera_id, W, H))
    lines.append(transform_line(camera_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{camera_id}\trole\tgame_camera")
    # M72-05：装饰散布。id 从**装饰段之前的最后一张结构实体**之后起，一个既有 id 都不动。
    # 输入用**抖动后**的格子图（盘上就是这个），不然"生成器看到的草/石分布"与实机不一致。
    jittered = [cells[y * W:(y + 1) * W] for y in range(H)]
    exclude, avoid = decor_reserved()
    placements = dk.scatter(W, H, jittered, solid, exclude, mid_avoid=avoid)
    lines.extend(dk.decor_lines(camera_id + 1, placements, table,
                                lambda cell: tints.get(cell[1] * W + cell[0], 0)))
    # M72-06：前景遮断拆层（树冠）。与 `set_foreground_occluders.py`（就地改盘上场景）
    # **共用同一条 `plan()`+`apply_patch()`**：这一段的存在感极低（拼回去画面逐字节相同），
    # 一旦重跑生成器把它抹掉，盘上只是"前景没了"，没有任何一张图会报错。
    fo.assert_id_band_free(lines)
    base_updates, occluders = fo.plan(lines, fo.lk.load())
    lines = fo.apply_patch(lines, base_updates, occluders)
    # M72-06 C 段：建筑/树/道具的地面投影贴片。必须在 `fo.apply_patch` **之后**——贴片按
    # 拆层后的本体贴图算落地行，也按拆层后的实体排除树冠块（空中块不配自己的投影）。
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

    与山道同口径（那边注释写的同一条理由）：一张图里"看得见但走不进去"的草地不会让任何
    一条锚点断言变红，只有连通块分解能把它摆出来。演武场是战斗图，死格带的后果是"狼被引到
    坪外就再也不回来"，所以这块必须报数而不是肉眼扫。
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


def read_enemy_homes():
    """`data/enemies.lua` 里本图敌人的 `home` 格（`[(name, cell)]`）。

    木人与灰狼的 home 就是 `ANCHORS` 里那两个点，但**这里读的是数据侧那一份**：两边各写
    一遍就是两套真相，改 enemies.lua 而生成器不知道，症状是"验收打不齐"而不是报错。
    home 是出生格，标成实心就是"一只卡在院墙里的灰狼"。
    """
    path = os.path.join(PROJECT_ROOT, "scripts", "data", "enemies.lua")
    if not os.path.exists(path):
        raise FileNotFoundError(f"{path} 读不到：home 格是本图可达性自检的输入")
    out, pending = [], None
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            # 一条记录以 `name = "..."` 起（那是每条的第一个字段），`home` 在后。
            hit = re.match(r'\s*name\s*=\s*"([^"]+)"', line)
            if hit:
                pending = {"name": hit.group(1), "arena": False}
                continue
            if pending is None:
                continue
            if re.match(r'\s*map\s*=\s*"combat_arena"', line):
                pending["arena"] = True
            home = re.match(r'\s*home\s*=\s*\{\s*x\s*=\s*(\d+)\s*,\s*y\s*=\s*(\d+)\s*\}', line)
            if home and pending["arena"]:
                out.append((pending["name"], (int(home.group(1)), int(home.group(2)))))
                pending = None
    return out


def read_arena_registry(
    path=os.path.join(PROJECT_ROOT, "scripts", "data", "maps.lua"),
):
    """从 data/maps.lua 读 combat_arena 的 spawn/出口格，做三方一致性校验（不执行 Lua）。"""
    if not os.path.exists(path):
        raise FileNotFoundError(f"{path} 读不到：spawn/出口格是三方一致契约的另一侧")
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    marker = text.find("combat_arena = {")
    if marker < 0:
        raise SystemExit("data/maps.lua 里找不到 combat_arena 注册块")
    chunk = text[marker:]
    end = chunk.find("\n  }")
    if end >= 0:
        chunk = chunk[:end]

    def cell_of(pattern, label):
        match = re.search(pattern, chunk)
        if not match:
            raise SystemExit(f"data/maps.lua:combat_arena 解析不到 {label}")
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
    for entry in ENTITIES:
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


def self_check(ground, solid, walk):
    """契约自检：锚点可走可达是石板、门洞可站、桥真能踩、全场无夯土、三方一致。

    `ground` 是**抖动后**的格子图（调用方传 `surface()` 的结果），`walk` 由 main 算一次
    传进来（打印连通块用的是同一个集合，两处必须同源）。
    """
    problems = []
    # 1) kind 值域：地表层只许 1..6（越界就是引擎按条目号切 UV 的那一类错位画面）。
    for y in range(H):
        for x in range(W):
            if ground[y][x] < 1 or ground[y][x] > 6:
                problems.append(f"({x},{y}) kind={ground[y][x]} 超出 1..6")
    # 2) farm.lua 防误垦：全场禁止夯土（抖动后测，供体论证见模块 docstring）。
    dirt = [(x, y) for y in range(H) for x in range(W) if ground[y][x] == DIRT]
    if dirt:
        problems.append(f"出现夯土 {len(dirt)} 格（首格 {dirt[0]}）：farm.lua 会把它当可开垦田")
    # 3) 战斗锚点：可走 + 从出生点可达 + 地表是石板。
    for name, (x, y) in ANCHORS.items():
        if solid[y][x]:
            problems.append(f"锚点 {name} ({x},{y}) 是 solid")
        elif (x, y) not in walk:
            problems.append(f"锚点 {name} ({x},{y}) 从出生点不可达")
        elif ground[y][x] != STONE:
            problems.append(f"锚点 {name} ({x},{y}) 地表 kind={ground[y][x]}，战斗格应为石板(3)")
    # 4) 灰狼寻路：home → 引敌站位必须连通（A* 追击验收的前提）。
    if ANCHORS["wolf_lure"] not in reachable_from(solid, ANCHORS["wolf_home"]):
        problems.append("灰狼 home (2,9) 到引敌位 (6,8) 不可达，追击验收会失败")
    # 5) 南门：门洞可站可达。洞的**坐标只认 building_kit 那张表**（读表而不是再抄一遍），
    #    而本文件的 GATE_DOOR 与那张表必须一致——两处各写一份就会漂移。
    table_openings = set(sum(bk.openings_table(SCENE).values(), ()))
    for name, cells in bk.openings_table(SCENE).items():
        for cell in cells:
            cx, cy = cell
            if solid[cy][cx]:
                problems.append(f"{name} 的门洞 {cell} 又被标成 solid：占地规则与 openings 打架")
            elif (cx, cy) not in walk:
                problems.append(f"{name} 的门洞 {cell} 从出生点不可达")
    if set(EXITS.values()) - table_openings:
        problems.append(f"出口格 {sorted(EXITS.values())} 不在门洞表 {sorted(table_openings)} 里："
                        "南门被挪走而 building_kit 没跟着改")
    # 6) 建筑/道具锚点（贴图左下角那一格）不许是**水**：锚点在水里就是整件泡在池子里。
    #    例外必须点名：`over_water=True`（栈桥那种"本来就铺在水面上"的美术）。
    for entry in ENTITIES:
        cx, cy = entry["cell"]
        if ground[cy][cx] == WATER and not entry.get("over_water"):
            problems.append(f"{entry['name']} 的锚点 ({cx},{cy}) 在水格上：整件泡在池子里"
                            f"（真要铺在水面上就显式写 over_water=True）")
    # 7) 栈桥：桥面每格可走（`walk` 的诚实性由地表侧兜住，见 `PIER`），且池子真的有水。
    for (x, y) in rect(PIER):
        if solid[y][x]:
            problems.append(f"栈桥桥面 ({x},{y}) 是 solid：一件 walk 模式的美术挡住了人")
    if not any(ground[y][x] == WATER for (x, y) in rect(POND)):
        problems.append("池塘外接框里一格水都没有：栈桥就铺在干地上")
    # 8) 敌人 home（enemies.lua 那一份）：可走可达。
    for name, cell in read_enemy_homes():
        x, y = cell
        if solid[y][x]:
            problems.append(f"enemies.lua:{name} home {cell} 是 solid：敌人出生在墙里")
        elif cell not in walk:
            problems.append(f"enemies.lua:{name} home {cell} 从出生点不可达")
    # 9) maps.lua 三方一致（spawn / 出口格）。
    registry = read_arena_registry()
    if registry["spawn"] != SPAWN:
        problems.append(f"maps.lua spawn={registry['spawn']} 与本文件 SPAWN={SPAWN} 不一致")
    for exit_id, cell in EXITS.items():
        if registry["exit"] != cell:
            problems.append(f"maps.lua 出口格={registry['exit']} 与本文件 {exit_id}={cell} 不一致")
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
    parser = argparse.ArgumentParser(description="从零装配 scenes/combat_arena.scene")
    parser.add_argument("--out", default=os.path.join("scenes", "combat_arena.scene"),
                        help="写出路径。对拍请指到 .temp 再与盘上场景逐行 diff。")
    args = parser.parse_args()
    ground = build_tiles()
    solid, readings = build_solid(ground)
    text = build_scene(ground, solid)
    with open(args.out, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)
    jittered, tints, _solid_str, changed = surface(ground, solid)
    # `surface` 返回的是**写进场景的那串 row-major 载荷**（`tilemap_component` 与
    # `interior_tints` 都要那一串），自检与读数要的是 2D 矩阵——同一份数据换一种索引，
    # 不是第二套真相。
    ground = to_matrix(jittered)
    print(f"wrote {args.out} ({W}x{H}, {len(ENTITIES)} 个建筑/道具实体)")
    for reading in readings:
        print("   %-22s 视觉 %-3d 格 solid %-3d 格 门洞 %s%s" % (
            reading["name"], reading["visual"], reading["solid"], reading["openings"] or "-",
            "（拆层件：" + ",".join(reading["mates"]) + "）" if reading["mates"] else ""))
    walk = reachable_from(solid, SPAWN)
    problems = self_check(ground, solid, walk)
    if problems:
        for problem in problems:
            print("[FAIL]", problem)
        raise SystemExit(1)
    homes = read_enemy_homes()
    anchors = " ".join(f"{name}({x},{y})" for name, (x, y) in ANCHORS.items())
    print(f"契约自检 ok（测在**抖动后**载荷上，{changed} 格被抖）：战斗锚点 {anchors} 全部可走、"
          f"可达、石板；无夯土；门洞通畅；栈桥可踩；enemies.lua {len(homes)} 个 home 格可达；"
          f"maps.lua 一致")
    main_block, dead = pockets(solid)
    print(f"可行走连通块：主块 {len(main_block)} 格，进不去的小块 {len(dead)} 个 "
          + " ".join(f"{len(c)}格@{min(p[0] for p in c)},{min(p[1] for p in c)}" for c in dead))
    kinds = {}
    for row in ground:
        for value in row:
            kinds[value] = kinds.get(value, 0) + 1
    print(f"地表 kind 直方图（抖动后）{kinds}  solid {sum(sum(r) for r in solid)} 格  "
          f"tint 非白档 {len(tints)} 格")
    pond = sum(1 for y in range(H) for x in range(W) if ground[y][x] == WATER)
    box = (POND[2] - POND[0] + 1) * (POND[3] - POND[1] + 1)
    widths = [len(xs) for xs in POND_ROWS.values()]
    print(f"   池塘 {pond} 格 / 外接框 {box} 格 ⇒ 填充率 {pond / box:.3f}"
          f"（旧版 x30..36 × y14..17 的完美方盒是 {box}/{box}=1.000）；每行宽 "
          f"{min(widths)}..{max(widths)} 格；桥面 {len(rect(PIER))} 格可踩")
    print("可行走格事实（文档同步用）：")
    for line in walkable_summary(solid):
        print(line)


if __name__ == "__main__":
    main()
