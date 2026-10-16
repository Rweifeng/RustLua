# -*- coding: utf-8 -*-
"""《江湖有田》青云山道场景生成（scenes/mountain_path.scene）——山道采药版。

用法（工程根目录 `examples/game_jhyt` 下）：

    python tools/gen_mountain_scene.py                 # 覆盖盘上场景（默认路径）
    python tools/gen_mountain_scene.py --out .temp/x.scene   # 对拍/试跑，不碰盘上场景

**本文件是这张图的唯一事实源**（M72-11 起）。以前这张图由 `gen_wilds_scene.py` 和
`set_*.py` 就地补丁两条路各写一半：生成器只管"地表 6 列 + 每件手填 solid"，而盘上的
HUD 尺寸、五件采集物（莲蓬/芦苇/茶丛/野果树/树桩）与贼寨那一格的 cell 都是后来手工调的
——`MOUNTAIN_BUILDINGS` 里 BanditCamp 写 (29,7) 而盘上是 (25,7)，重跑生成器就会把贼寨
搬回去、把 solid 留在原处。现在与 `gen_manor_scene.py` / `gen_village_scene.py` 同构：
**同一条代码路径**（`building_kit` / `set_foreground_occluders` / `set_cast_shadows` /
`decor_kit` / `set_terrain_autotile` 的抖动与撒布），所以"重跑生成器"与"就地工具
`--check`"必须给出同一个字节序列。`gen_wilds_scene.py` 从此只出寒泉洞那一张。

改布局改本文件，改画面改 `gen_wilds_art.py` / `gen_terrain_tiles.py`，两边各跑一次即可复现。

引擎 Tilemap 约定（scene_version=2，见 engine crates/rustlua-ecs/src/tilemap.rs）：
  tile_size=WxH;width=W;height=H;texture=PATH;tiles_format=1;atlas_cols=12;atlas_rows=9;
  tiles=<逗号分隔的 kind id，row-major，row 0 在最底>;solid=<row-major 0/1>
  开逻辑瓦时再追加 `tile_kinds=6;variant_count=1;autotile=1`（+ 稀疏 `tints`/`tints_rgb`）。
  * 地表层存的是 **kind id**（1..6），运行期由 `Tilemap::resolve_atlas_index` 按 4-bit
    邻居掩码解成 12×9 图集的条目（位序 bit0=y-1、bit1=x+1、bit2=y+1、bit3=x-1）；
  * 单格 32 世界单位，y 朝上；索引 index = y*width+x。
列映射（与 `terrain_ramp.json` 的 `scenes.mountain_path.kinds` 一致，**不可随意改**）：
  1=落叶松林地 2=碎石土路 3=石阶 4=岩壁 5=竹林地 6=溪水
  编号语义与山庄/村庄同一套（`building_kit.TILE_NATIVE_SOLID=(4,6)` 靠的就是它）：这里的
  4 是**岩壁**、6 是**山涧溪水**，两者天然实心。5 在这张图上是"竹林地"而不是屋顶——
  竹林的叶子画在瓦里，所以 `decor_kit.BASE_P[5]=0`（那一块不撒装饰）读起来才对。

山道布局（cell，0-index，y 朝上，含端点）：
  南山门   ：x18..21, y0..2（贴图压住南墙两行，门洞 x19 两格 = 出口格 (19,1) 与其北邻通道格
             (19,2)；出生点不在洞里，在门北两格的山道 (19,4)——洞那一格横向只剩 4 px，#28）
  进山路   ：x18..21, y1..6 碎石 → 折西 x14..21, y6..7 → 石阶 x10..21, y7..8
             → 西行纵路 x10..13, y7..15 → 石阶 x10..20, y14..15 → 北台地 x10..20, y15..19
             → 洞口路 x20..23, y17..20（东北洞口 = 出口格 (22,20)，从洞回来落 (22,19)）
  东溪     ：x23..27, y4..19 一带溪水（实心，**按行手填的不规则岸线**，形状见 `STREAM_ROWS`）
             ；栈道 x23..27, y10..11 铺石板 = **过溪唯一通道**；水格外一圈草地刷成碎石（`碎石岸`）
  西涧     ：y14 是 x2..7、y15 是 x3..7 的一窄段死水（两行错开一格，见 `GORGE_ROWS`）；
             断桥 x3..6 压在上面（`BrokenBridge` 是建筑，实心）
             ——以前这一带整片是岩壁 (2,10,7,20)，"断桥"横跨在**一滴水都没有的崖坡**上，
             所以把西崖拆成 y10..13 / y16..20 两段，中间留出涧；采集带随之挪到东岸 x8..9。
  西岸采集 ：x8..9 走廊（松树 (8,12)、芦苇 (8,14)、野果树 (8,17)、莲蓬落在水里 (7,15)）
  竹林     ：x2..8 y3..9 与 x29..38 y16..20 两处（kind 5，可走，不撒装饰）——**按行手填的
             不规则边界**（`BAMBOO_W_ROWS`/`BAMBOO_E_ROWS`），里面立六丛竹（`BambooA..F`）
  崖壁     ：x2..7 y10..13、x2..7 y16..20（夹着西涧）、x30..37 y1..5 与 x33..37 y6..9（东坡）
  贼寨     ：x29..33, y7..9（东岸台地）；茶棚 x14..17, y9..11（中途）

**与 data 侧的四份契约**（每次生成自检，失败非零退出）：
  1. `data/maps.lua` 的 spawn / 两个出口格 / 从寒泉洞回来的落格必须可走且从出生点可达，
     而**放置格**（spawn 与每一个 `to_cell`）还必须有 `building_kit.placement_problems` 那条
     横向净空（一格 32 px）——旧 spawn (19,2) 与山门洞同格，人被两柱夹住只挪得动 4 px（#28）；
  2. `data/places.lua` 里 mountain_path 的地点格必须可走可达，且**不与出口格四邻相邻**
     （出口 priority 20 会抢走对话/商店 priority 10，理由见 `gen_village_scene` 同条注释）；
  3. `data/gather.lua` 的每个采集格走的是 `facing_cell`：格子本身可以是实心（莲蓬就长在水里），
     但必须至少有一个**从出生点走得到**的可走 4 邻格，否则那格货永远采不到；
  4. `data/enemies.lua` 里本图敌人的 home 格必须可走可达（home 是出生格，实心就是敌人卡在墙里）。
     自检把这四份都读进来判，`--freeze` 之类的手工同步不再需要。
"""
import argparse
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

# 工程根（`examples/game_jhyt`）。**事实输入**（places.lua / gather.lua 这类决定留白与可达
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

# 地图尺寸（`pixel_kit.camera_line` 与 `scripts/world/camera_view.lua` 的 bounds 校验按
# 40x23 处理，不要改）。23 行是为横屏 1280×720 设计视口定的：40×22 = 1280×704 比屏幕矮
# 16 像素，相机夹到地图下沿后顶/底仍露出 clear color（#0A121F）的深色横条。
W, H = 40, 23
TILE = 32

# 场景键（= `name=` 头 = `data/maps.lua` 的地图键 = `terrain_ramp.json` 的 `scenes` 档名）。
# `building_kit` 的门洞表/横件碰撞表、色阶表都按它点名取用。
SCENE = "mountain_path"
GROUND_TEXTURE = tk.ground_texture(SCENE)

# 列映射（写 scene tile 用的 kind id，语义见模块 docstring）
GRASS, DIRT, STONE, WALL, ROOF, WATER = 1, 2, 3, 4, 5, 6

# ---- 分区 ----
SOUTH_ROAD = (18, 1, 21, 6)      # 山门内直路
WEST_ROAD = (14, 6, 21, 7)       # 折向西
RIDGE_ROAD = (10, 7, 13, 15)     # 西行纵路
PLATEAU_ROAD = (10, 15, 20, 19)  # 北台地横路
MOUTH_ROAD = (20, 17, 23, 20)    # 东北洞口路
PATHS = (SOUTH_ROAD, WEST_ROAD, RIDGE_ROAD, PLATEAU_ROAD, MOUTH_ROAD)
# 两段石阶：第一段压进 x10..21 让整个折口都是石阶（比"只铺纵路那一格宽"更接近"上山的路"），
# 第二段是台地的门槛。石板（3）在抖动里**只加不减**（`JITTER_DONORS` 含 3、`JITTER_TARGETS`
# 不含 3），所以这两块永远还是路——`data/buildings.lua` 判"tile==3 是道路"靠的就是这条。
STAIRS = ((10, 7, 21, 8), (10, 14, 20, 15))
# 东溪（南北向，把山道切成东西两半）：**按行手填的不规则岸线**（M72-11，对齐山庄 `POND_ROWS`
# 那条口径），不再是 `x23..27 × y4..19` 的矩形——5×16 的完美方盒在屏上读成一条运河，
# 是离屏预览抓出来的（`.temp/mp_preview2.png`）。三条硬约束：
#   ① 每行的 x 集都必须**落在旧矩形 x23..27 之内**：水只减不加 ⇒ 原本可走的格一格都不会
#      变成水，岸上任何一条通路（去贼寨、去矿石堆、去东北洞口）都不可能因为这次改形状而断。
#   ② 相邻两行必须有公共列：溪要连成一条，不许断成两截死水。
#   ③ y10..11 保持满跨 x23..27：那是栈道的两行，`self_check` 判"栈道每格非 solid"，
#      水退到 x23..27 之外就等于把桥的一头搁在岸上。
# 形状读数：68 格水/石（含栈道 10 格），外接框 5×16=80 ⇒ 填充率 0.850（旧版 80/80=1.000）；
# 真正的溪水格 58（68 减去栈道盖掉的 10）。
STREAM_ROWS = {
    4: (24, 25, 26),
    5: (23, 24, 25, 26),
    6: (23, 24, 25, 26, 27),
    7: (24, 25, 26, 27),
    8: (23, 24, 25, 26, 27),
    9: (23, 24, 25, 26),
    10: (23, 24, 25, 26, 27),
    11: (23, 24, 25, 26, 27),
    12: (24, 25, 26, 27),
    13: (23, 24, 25, 26),
    14: (23, 24, 25, 26, 27),      # 溪潭（最宽的一档，深度档就压在这一带）
    15: (24, 25, 26, 27),
    16: (23, 24, 25, 26),
    17: (23, 24, 25, 26, 27),
    18: (24, 25, 26, 27),
    19: (24, 25, 26),
}
STREAM_CELLS = {(x, y) for y, xs in STREAM_ROWS.items() for x in xs}
STREAM = (23, 4, 27, 19)           # 外接框（校验与注释用）
# 西涧（断桥跨它；一窄段死水，两头都被崖壁夹住）。同样只许往旧矩形 x2..7 y14..15 里退。
# 两行各退一头：y14 满跨 x2..7（西岸到崖根），y15 退掉 x2 让出一小块西岸草地。
# y15 必须留住 x7：那是 `LotusPond` 的水格（`over_water=True`，可采邻格 (8,15)），
# 把它退成草地就等于"莲蓬长在岸上"，而这条正是上一轮从 (29,5) 挪进涧里的理由。
# 反过来若退掉 y14 的 x7，(7,14) 会被水（6,14 与 7,15）、崖（7,13）与芦苇 (8,14) 的占地
# 四面夹成一个 1 格走不到的岛——生成器印的"进不去的小块"当场就会多出一条。
GORGE_ROWS = {
    14: (2, 3, 4, 5, 6, 7),
    15: (3, 4, 5, 6, 7),
}
GORGE_CELLS = {(x, y) for y, xs in GORGE_ROWS.items() for x in xs}
GORGE = (2, 14, 7, 15)             # 外接框（校验与注释用）
BOARDWALK = (23, 10, 27, 11)       # 栈道（溪上铺石板，可走）——**过溪的唯一通道**
# 竹林：**按行手填的不规则岸线**（同东溪那条口径）。kind 5 在这张图上是"竹林地"，
# 它是**可走的地板**而不是遮挡物，所以改形状**一格碰撞都不动**（`TILE_NATIVE_SOLID` 只有
# 4 与 6）——这是它比岩壁便宜的地方，也是本轮先动它而把岩壁登记成未完成项的真正理由：
# 岩壁那四块一改就是改物理，走廊宽度、贼寨与矿石堆的可达性要整片重审。
# 读数：西竹林 43 格（旧 7×7=49 的矩形）、东竹林 39 格（旧 8×5=40），两块的行宽都在
# 6..8 之间摆，外接框填充率 43/49=0.878 与 39/40=0.975——东竹林本来就贴着北墙与东坡，
# 可摆的边只剩西、南两条，所以它的填充率天然高，形状读起来是"被崖挤出来的一角"而不是方盒。
BAMBOO_W_ROWS = {
    3: (3, 4, 5, 6, 7),
    4: (2, 3, 4, 5, 6, 7, 8),
    5: (3, 4, 5, 6, 7, 8),
    6: (2, 3, 4, 5, 6, 7),
    7: (2, 3, 4, 5, 6, 7, 8),
    8: (3, 4, 5, 6, 7, 8),
    9: (2, 3, 4, 5, 6, 7),
}
BAMBOO_E_ROWS = {
    16: (31, 32, 33, 34, 35, 36, 37),
    17: (30, 31, 32, 33, 34, 35, 36, 37),
    18: (29, 30, 31, 32, 33, 34, 35, 36),
    19: (30, 31, 32, 33, 34, 35, 36, 37),
    20: (31, 32, 33, 34, 35, 36, 37, 38),
}
BAMBOO_CELLS = ({(x, y) for y, xs in BAMBOO_W_ROWS.items() for x in xs}
                | {(x, y) for y, xs in BAMBOO_E_ROWS.items() for x in xs})
CLIFFS = ((2, 10, 7, 13), (2, 16, 7, 20), (30, 1, 37, 5), (33, 6, 37, 9))

SPAWN = (19, 4)                   # 与 `data/maps.lua` mountain_path.spawn 一致（旧值 (19,2) 在山门洞里，横向只剩 4 px）
# 出口格与落格：`mountain_to_village` 站在山门洞里 (19,1)、`mountain_to_cave` 在东北洞口
# (22,20)，而从寒泉洞回来落在 (22,19)——它不是出口，是**别人送进来时的落脚格**，
# 那一格若被标成实心，玩家一出洞就卡进岩壁里。所以四格一起进可达性自检。
# 本图 spawn (19,4) 也是"别人送进来时的落脚格"（`village_to_mountain.to_cell`），旧值 (19,2)
# 与山门洞同格——24×24 的脚底球在两柱之间左右各只剩 4 px（#28），所以放在门洞北两格的山道上。
EXITS = {"to_village": (19, 1), "to_cave": (22, 20)}
# `village_to_mountain` 送进来的落格就是本图 spawn 那一格（(19,4)，#28 起它在山门洞北两格），
# 所以两处都登记：`LANDINGS` 喂装饰留白，`SPAWN` 喂主角 Transform 与可达性起点。
LANDINGS = (("village", (19, 4)), ("cold_spring_cave", (22, 19)))
# 抖动冻结矩形：**空**。山庄冻的是药田（`farm.lua` 判可开垦只能 tile==2），村庄冻的是茶园
# （语义地块），山道两块都没有——这里的碎石路本来就该和林地咬出犬牙，冻住反而退回"直线交界"。
FREEZE = []

# ---- 建筑与道具 ----
# texture：gen_wilds_art.py 的产物；size 与图片像素 1:1（`check_texture_sizes` 逐条核）；
# pivot=(0,0) 即 transform 为左下角。cell：贴图左下角所在 cell。
#
# **这里没有 `solid` 字段**（M72-07 的规矩）：占地只认 `building_kit` 一条规则——
#   建筑 = 本体∪同名拆层件（屋顶）的**不透明像素**覆盖格 − 门洞；道具 = `entity_meta cell`
#   那一格，横着占两格的按 `building_kit.SCENES[SCENE].prop_collision` 点名走 `box`/`walk`。
# 门洞声明在那张表里（山门 (19,1)(19,2)），两边任何一处改了都有反查会红。
ENTITIES = [
    {"name": "MountainGate", "texture": "assets/art/buildings/wilds_mountain_gate.png",
     "size": (128, 96), "cell": (18, 0), "order": 5, "role": "building"},
    {"name": "TeaShed", "texture": "assets/art/buildings/wilds_tea_shed.png",
     "size": (128, 80), "cell": (14, 9), "order": 0, "role": "building"},
    # 贼寨 cell 取 (29,7)：盘上那条是 (25,7)，而 (25,7) 压在溪上（x23..27 是水）——
    # 一座寨子立在河中央，而按旧手填 solid 它挡的还是 (25,7) 那片水。这里登记的是
    # **东岸台地**（x28 以东是干地），与 `places.lua:mountain_camp` (31,6) 对得上。
    {"name": "BanditCamp", "texture": "assets/art/buildings/wilds_bandit_camp.png",
     "size": (160, 96), "cell": (29, 7), "order": 0, "role": "building"},
    # 断桥压在**西涧**上（见模块 docstring：西崖拆两段、中间留出 y14..15 那一窄段水）。
    # 它是 `role=building` 而不是道具：一座断桥就该挡人——能走过去它就不是断桥了。
    # `over_water=True` 不是让步而是这件事本身：桥面就该铺在水上，而它的锚点 (3,14) 恰在
    # 涧里（`visual 4 格`全落在水面上，solid 真值与水的天然实心重合）。
    {"name": "BrokenBridge", "texture": "assets/art/buildings/wilds_broken_bridge.png",
     "size": (128, 48), "cell": (3, 14), "order": 10, "role": "building", "over_water": True},
    # 栈道：铺在溪面上，碰撞模式 `walk`——过河的唯一通道，标成实心就把东岸的贼寨、
    # 矿石堆与洞口整块封死（旧版手填 `solid_cells=[]` 判的是同一件事，只是那时没有名字）。
    {"name": "Boardwalk", "texture": "assets/art/buildings/wilds_boardwalk.png",
     "size": (128, 64), "cell": (23, 10), "order": 12, "role": "prop", "over_water": True},
    # 松树三件：本体是拆层后的 `wilds_pine_trunk.png`，树冠由 `set_foreground_occluders`
    # 按 `layer_kit` 的登记表挂上去——这里登记**整图**，拆层是那条代码路径的事。
    {"name": "PineA", "texture": "assets/art/props/wilds_pine.png",
     "size": (64, 88), "cell": (9, 3), "order": 20, "role": "prop"},
    # PineB 从盘上的 (14,16) 挪到西岸走廊 (8,12)：x8..9 那条走廊原本只有两个采集点，
    # 一路空到北台地；一棵 88px 高的松树正好让走道有前景可遮（金标口径：视线落点）。
    {"name": "PineB", "texture": "assets/art/props/wilds_pine.png",
     "size": (64, 88), "cell": (8, 12), "order": 20, "role": "prop"},
    {"name": "PineC", "texture": "assets/art/props/wilds_pine.png",
     "size": (64, 88), "cell": (35, 14), "order": 20, "role": "prop"},
    # 竹丛六件（M72-11 补）：kind 5 那张瓦画的是**竹林的地面**（落叶层 + 竹鞭 + 一撇撇
    # 竹叶），而这张图上一棵竹子都没有——两片 49/40 格的矩形空地读起来就是两块晒谷场。
    # 复用山庄的 `manor_bamboo.png`（村庄同样复用它的老树与水缸，口径是"一件多用"）：
    # 竹子不是某一处专属的建筑，而山道这套 `gen_wilds_art.py` 从来没有出过竹件，
    # 为六棵竹丛新开一条出图管线不划算——新开的话 `LAYER_SPLITS`、`check_texture_sizes`
    # 与资产基线都要跟着动，那是 M72-10（室内图）之后再付的账。
    # 碰撞走默认的 `root`（一格）：一丛竹子的秆就那么粗。
    {"name": "BambooA", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (3, 4), "order": 20, "role": "prop"},
    {"name": "BambooB", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (7, 6), "order": 20, "role": "prop"},
    {"name": "BambooC", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (4, 8), "order": 20, "role": "prop"},
    {"name": "BambooD", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (32, 17), "order": 20, "role": "prop"},
    {"name": "BambooE", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (36, 19), "order": 20, "role": "prop"},
    {"name": "BambooF", "texture": "assets/art/props/manor_bamboo.png",
     "size": (48, 76), "cell": (30, 18), "order": 20, "role": "prop"},
    # 山石两件走 `box`（48×32 横占两格，只标锚点等于右半边看得见却穿得过）。
    # A 从盘上的 (11,12) 挪到 (19,12)：`box` 后它横压 x11..12 两格，正好把
    # `places.lua:mountain_herbs` (12,12) 那格地点压成实心——NPC/地点格走不出来是硬错。
    # B 从 (19,13) 挪到东岸 (29,13)：东岸 x28..32 那一片原来一件大东西都没有。
    {"name": "BoulderA", "texture": "assets/art/props/wilds_boulder.png",
     "size": (48, 32), "cell": (19, 12), "order": 20, "role": "prop"},
    {"name": "BoulderB", "texture": "assets/art/props/wilds_boulder.png",
     "size": (48, 32), "cell": (29, 13), "order": 20, "role": "prop"},
    # 藤蔓两件贴在崖壁上（岩壁本来就是实心格，`root` 单格不额外挡路）。
    {"name": "VineA", "texture": "assets/art/props/wilds_vine.png",
     "size": (32, 64), "cell": (7, 12), "order": 20, "role": "prop"},
    {"name": "VineB", "texture": "assets/art/props/wilds_vine.png",
     "size": (32, 64), "cell": (31, 2), "order": 20, "role": "prop"},
    {"name": "TorchA", "texture": "assets/art/props/wilds_torch.png",
     "size": (24, 56), "cell": (17, 5), "order": 20, "role": "prop"},
    # TorchB 从 (22,18) 挪到洞口路 (21,20)：火把该给"这里是入口"作标志，
    # `places.lua:mountain_cave_mouth` 因此挪到它南邻 (21,19)（不能贴着出口格）。
    {"name": "TorchB", "texture": "assets/art/props/wilds_torch.png",
     "size": (24, 56), "cell": (21, 20), "order": 20, "role": "prop"},
    {"name": "Signpost", "texture": "assets/art/props/wilds_signpost.png",
     "size": (40, 48), "cell": (21, 5), "order": 20, "role": "prop"},
    {"name": "Campfire", "texture": "assets/art/props/wilds_campfire.png",
     "size": (48, 40), "cell": (30, 11), "order": 20, "role": "prop"},
    {"name": "Crate", "texture": "assets/art/props/wilds_crate.png",
     "size": (32, 28), "cell": (33, 10), "order": 20, "role": "prop"},
    # ---- 采集物（`data/gather.lua` 的十个山道节点，一格一件，标志物与格子同源）----
    {"name": "Berry", "texture": "assets/art/gather/wilds_berry.png",
     "size": (32, 32), "cell": (9, 16), "order": 22, "role": "prop"},
    {"name": "HerbCluster", "texture": "assets/art/gather/wilds_herb.png",
     "size": (32, 32), "cell": (12, 11), "order": 22, "role": "prop"},
    {"name": "Firewood", "texture": "assets/art/gather/wilds_firewood.png",
     "size": (40, 32), "cell": (15, 3), "order": 22, "role": "prop"},
    {"name": "OreRock", "texture": "assets/art/gather/wilds_ore.png",
     "size": (40, 32), "cell": (28, 17), "order": 22, "role": "prop"},
    {"name": "PoisonHerb", "texture": "assets/art/gather/wilds_poison_herb.png",
     "size": (32, 32), "cell": (29, 15), "order": 22, "role": "prop"},
    # 莲蓬**长在水里**（`over_water=True` 是显式豁免）：以前它立在 (29,5) 的干草地上，
    # 而 gather.lua 那句注释写的可站格 (32,5)/(34,5) 离它三格远——facing_cell 根本采不到。
    # 挪到西涧东头 (7,15)，可采邻格是 (8,15)。
    {"name": "LotusPond", "texture": "assets/art/gather/wilds_lotus.png",
     "size": (32, 32), "cell": (7, 15), "order": 22, "role": "prop", "over_water": True},
    # 芦苇丛从 (5,13) 挪出西崖：那一格整片在崖壁里（x2..7 y10..13 是岩壁），
    # 四个邻格全实心 ⇒ 生境与可达性两边都不成立。挪到涧东岸 (8,14)。
    {"name": "ReedPatch", "texture": "assets/art/gather/wilds_reed.png",
     "size": (32, 40), "cell": (8, 14), "order": 22, "role": "prop"},
    # 茶丛用自己的图（以前复用 `wilds_herb.png`，而药草丛就在七格之外、同一屏两棵一样的灌木）。
    {"name": "TeaGrove", "texture": "assets/art/gather/wilds_tea_bush.png",
     "size": (32, 26), "cell": (19, 10), "order": 22, "role": "prop"},
    # 野果树同理从西崖里 (6,16) 挪到走廊 (8,17)；它的图复用野果丛（一丛矮果树，读得通）。
    {"name": "WildFruitTree", "texture": "assets/art/gather/wilds_berry.png",
     "size": (32, 32), "cell": (8, 17), "order": 22, "role": "prop"},
    # 树桩以前直接引整张 `wilds_pine.png`（一棵完整松树标成"木桩"，`size=` 还写 64×88），
    # 现在有 `wilds_stump.png` 那张自己的图。
    {"name": "TreeStump", "texture": "assets/art/props/wilds_stump.png",
     "size": (32, 28), "cell": (10, 4), "order": 22, "role": "prop"},
]


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

    拆层要先过一遍 `fo.plan`：占地是"本体∪同名屋顶件"的不透明格并集，而屋顶那件在盘上
    是独立实体（M72-06），只喂未拆层的 ENTITIES 会少算屋檐压出去的那一圈。
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
    """solid 布尔矩阵：`occupied_cells` 的格集换算回来。阻挡行走、且每格都有可视依据。"""
    cells, readings = occupied_cells(ground)
    return [[1 if (x, y) in cells else 0 for x in range(W)] for y in range(H)], readings


def rect(region):
    x0, y0, x1, y1 = region
    return [(x, y) for y in range(y0, y1 + 1) for x in range(x0, x1 + 1)]


def build_tiles():
    """地表 tile 值矩阵 tiles[y][x]（**抖动前**的形状：这里只画分区，犬牙交给 `jitter`）。"""
    ground = [[GRASS] * W for _ in range(H)]
    for (x, y) in BAMBOO_CELLS:
        ground[y][x] = ROOF       # 5 = 竹林地（可走，不撒装饰）
    for region in CLIFFS:
        for (x, y) in rect(region):
            ground[y][x] = WALL
    for region in PATHS:
        for (x, y) in rect(region):
            ground[y][x] = DIRT
    for region in STAIRS:
        for (x, y) in rect(region):
            ground[y][x] = STONE
    water_cells = STREAM_CELLS | GORGE_CELLS
    for (x, y) in water_cells:
        ground[y][x] = WATER
    # 碎石岸：水格的 4 邻域里**仍是草地**的那些刷成碎石（kind 2）。这一步对齐山庄的
    # "石砌岸"（`gen_manor_scene.build_tiles`）：水↔草直接相邻时 autotile 在岸上读成
    # "一摊蓝浮在绿布上"，中间必须垫一档硬质地表。山道上垫的是**碎石**而不是青石板——
    # 石板（3）在这张图上有语义（`data/buildings.lua` 判 tile==3 是道路），沿溪撒一圈
    # 等于凭空多出一条没人走的路；碎石没有语义，而且它身在 `JITTER_TARGETS`，
    # 会和两侧草地自己咬出犬牙，不必再手摆。
    for (x, y) in water_cells:
        for nx, ny in ((x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)):
            if 0 <= nx < W and 0 <= ny < H and ground[ny][nx] == GRASS:
                ground[ny][nx] = DIRT
    # 栈道：溪水上铺石板，成为可过的通道（必须在溪水之后写，否则被水盖回去）。
    for (x, y) in rect(BOARDWALK):
        ground[y][x] = STONE
    # 外圈崖壁（这是山路，四面都是山体）。底部两行都是壁：H 从 22 补到 23 时保持原下沿
    # （H-2）仍是壁，新行压在最底——南墙那两行由山门贴图盖住，出口格 (19,1) 在壁内那一列。
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

    输入全部来自本文件的布局常量与 `data/{places,gather}.lua`，所以
    `set_scatter_decor.py` 只要 import 这里，两条路就必然撒出同一段装饰。

    **硬留白是空集**：山庄那一块是药田（作物画在 FarmGrid 上，装饰一压上去就看不清哪块
    浇过），山道没有农田、也没有茶园那种成行的语义地块。竹林（kind 5）不需要留白——
    `decor_kit.BASE_P[ROOF]` 本来就是 0，那一块的地面内容由瓦自己画。
    """
    places = [(cx, cy) for _, cx, cy in read_place_cells()]
    nodes = [node["cell"] for node in read_gather_nodes()]
    props = [entry["cell"] for entry in ENTITIES]
    keys = [SPAWN, *EXITS.values(), *(cell for _, cell in LANDINGS)]
    return set(), dk.mid_avoid(W, H, props + nodes, places, keys)


def cell_to_world(cx, cy):
    """cell 左下角世界坐标（y 朝上，row 0 在最底）"""
    return (cx + 0.5) * TILE, (cy + 0.5) * TILE


def row_major(grid):
    return "".join(str(grid[y][x]) for y in range(H) for x in range(W))


def transform_line(entity_id, x, y):
    # M32：统一的 Transform 形态由 `pixel_kit.transform_line` 单点产出
    # （引擎读侧只认 `position/rotation/scale`，2D 旧形态已删除；见该函数注释）。
    from pixel_kit import transform_line as spawn_transform

    return spawn_transform(entity_id, x, y)


def sprite_line(entity_id, texture, width, height, layer, order):
    return (
        f"component\t{entity_id}\tSprite\ttexture={texture};size={width}x{height};"
        f"pivot=0.000,0.000;tint=1.000,1.000,1.000,1.000;sorting_layer={layer};order={order}"
    )


def build_scene(ground, solid):
    solid_str = row_major(solid)
    table = tk.load_ramp(SCENE)
    cells = [ground[y][x] for y in range(H) for x in range(W)]
    # 抖动（犬牙交界）与 tint 撒布走 `set_terrain_autotile` 那一份实现：两条路
    # （这里从零生成 / 那里就地改盘上场景）必须产出同一个字节序列，口径见该文件 docstring。
    cells, _changed = jitter(cells, solid_str, W, H, freeze_cells(FREEZE, W, H))
    tints = scatter_tints(W, H, [tint_id for tint_id, _ in tk.ground_tints(table)])
    # 水面按**形状**压一档 `shaded`（M72-07 在山庄池塘上量的那条）：改不规则岸线之后
    # "四邻皆水"的内部格从 5×16 矩形那一段的成串缩到 24 格（东溪 y5..8 与 y13..18 两截，
    # 西涧只有 2 行高、永远凑不出内部格），所以深度档现在是"一段一段的渚"而不是
    # "一整条均匀的暗带"——这正是要的效果。
    tints.update(interior_tints(cells, W, H, tk.kind_id(table, "water"),
                                tk.ground_tints(table)[0][0]))
    lines = [
        "scene_version=2",
        # 场景名必须等于地图键（data/maps.lua 的 `mountain_path`）：main.lua 用
        # `is_playable(scene.reload())` 判定玩法地图，写错就进不去。
        "name=mountain_path",
        "meta\tdisplay_name\t江湖有田·青云山道",
        "meta\tgame\tjianghu_youtian",
        "meta\ttile_size\t32",
        # 横屏方案A：设计分辨率 1280×720（与 `scripts/ui/layout.lua` 一致）。
        "meta\tui_reference_height\t720",
        "meta\tui_reference_width\t1280",
        "entity\t1\tGroundMap",
        transform_line(1, 0.0, 0.0),
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
    lines.append(f"component\t{player_id}\tCollider\t{bk.player_ball_collider()}")
    lines.append(transform_line(player_id, spawn_wx, spawn_wy))
    lines.append(f"entity_meta\t{player_id}\trole\tplayer_spawn")
    lines.append(f"entity_meta\t{player_id}\tpersistent\ttrue")
    next_id += 1
    title_id = next_id
    lines.append(f"entity\t{title_id}\tHudTitle")
    lines.append(
        f"component\t{title_id}\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;"
        "offsets=24.00,20.00,672.00,44.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000"
    )
    lines.append(
        "component\t{0}\tText\ttext=青云山道 · 采药与护送;font_size=18.00;color=0.949,0.918,0.847"
        .format(title_id)
    )
    lines.append(transform_line(title_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{title_id}\trole\thud_title")
    hint_id = next_id + 1
    lines.append(f"entity\t{hint_id}\tHudHint")
    lines.append(
        # 地图自带操作提示放在**地图标题正下方**（y=50）；宽度收到 672 与其余两张图一致，
        # 底部信息区整块留给 HUD 的动态反馈/目标/键位行。
        f"component\t{hint_id}\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;"
        "offsets=24.00,50.00,672.00,72.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000"
    )
    lines.append(
        f"component\t{hint_id}\tText\ttext=WASD 移动 · E 交互(采集/出口) · T 睡觉 · Q 吐纳 · J 普攻 · K 武学"
        ";font_size=14.00;color=0.850,0.900,1.000"
    )
    lines.append(transform_line(hint_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{hint_id}\trole\thud_hint")
    # 场景相机（本地图唯一的 `Camera` 实体）：把 2D 取景范围锁在整张地图的矩形内，
    # 运行期由 `scripts/world/camera_view.lua` 让相机跟随玩家（`camera2d.follow`）。
    camera_id = hint_id + 1
    lines.append(f"entity\t{camera_id}\tGameCamera")
    lines.append(camera_line(camera_id, W, H))
    lines.append(transform_line(camera_id, 0.0, 0.0))
    lines.append(f"entity_meta\t{camera_id}\trole\tgame_camera")
    # M72-05：装饰散布。id 从**装饰段之前的最后一张结构实体**之后起，一个既有 id 都不动。
    # 输入用**抖动后**的格子图（盘上就是这个），不然"生成器看到的草/土分布"与实机不一致。
    jittered = [cells[y * W:(y + 1) * W] for y in range(H)]
    exclude, avoid = decor_reserved()
    placements = dk.scatter(W, H, jittered, solid, exclude, mid_avoid=avoid)
    lines.extend(dk.decor_lines(camera_id + 1, placements, table,
                                lambda cell: tints.get(cell[1] * W + cell[0], 0)))
    # M72-06：前景遮断拆层（树冠/屋顶）。与 `set_foreground_occluders.py`（就地改盘上场景）
    # **共用同一条 `plan()`+`apply_patch()`**：这一段的存在感极低（拼回去画面逐字节相同），
    # 一旦重跑生成器把它抹掉，盘上只是"前景没了"，没有任何一张图会报错。
    fo.assert_id_band_free(lines)
    base_updates, occluders = fo.plan(lines, fo.lk.load())
    lines = fo.apply_patch(lines, base_updates, occluders)
    # M72-06 C 段：地面投影贴片。必须在 `fo.apply_patch` **之后**——贴片按拆层后的本体
    # 贴图算落地行，也按拆层后的实体排除树冠/屋顶块（空中块不配自己的投影）。
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


def _read_lua(name, pattern):
    path = os.path.join(PROJECT_ROOT, "scripts", "data", name)
    if not os.path.exists(path):
        raise FileNotFoundError(
            f"{path} 读不到：那里的格子是可达性自检与散布留白的**输入**，宽容地当成"
            "「一条也没有」继续跑，画面照样出图，只是玩家会走进一处没人验证过的布局"
        )
    out = []
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            match = re.match(pattern, line)
            if match:
                out.append(match)
    return out


def read_place_cells():
    """`data/places.lua` 里属于本图的地点格（`{name, x, y}`）。"""
    return [(hit.group(1), int(hit.group(2)), int(hit.group(3)))
            for hit in _read_lua(
                "places.lua",
                r'\s*(\w+)\s*=\s*\{\s*map\s*=\s*"mountain_path"\s*,\s*cell\s*=\s*'
                r'\{\s*x\s*=\s*(\d+)\s*,\s*y\s*=\s*(\d+)\s*\}')]


def read_gather_nodes():
    """`data/gather.lua` 里属于本图的采集节点（`{id, cell}`）。

    那里是多行一条记录，所以按 `id = "..."` 起、`cell = { x = .., y = .. }` 收，逐条配对；
    只属于本图 `map = "mountain_path"` 的那些。
    """
    path = os.path.join(PROJECT_ROOT, "scripts", "data", "gather.lua")
    if not os.path.exists(path):
        raise FileNotFoundError(f"{path} 读不到：采集格是本图可达性自检的输入")
    nodes, pending = [], None
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            hit = re.match(r'\s*id\s*=\s*"(\w+)"', line)
            if hit:
                pending = {"id": hit.group(1), "mountain": False, "cell": None}
                nodes.append(pending)
                continue
            if pending is None:
                continue
            if re.match(r'\s*map\s*=\s*"mountain_path"', line):
                pending["mountain"] = True
            cell = re.match(r'\s*cell\s*=\s*\{\s*x\s*=\s*(\d+)\s*,\s*y\s*=\s*(\d+)\s*\}', line)
            if cell and pending["cell"] is None:
                pending["cell"] = (int(cell.group(1)), int(cell.group(2)))
    for node in list(nodes):
        if not node["mountain"] or node["cell"] is None:
            nodes.remove(node)
    return nodes


def pockets(solid):
    """进不去的可走格块（除最大连通块以外的那些）。

    为什么要把连通块报出来而不是只测 places：一张图里"看得见但走不进去"的草地是玩家
    会用脚投票的东西，而它不会让任何一条 places 断言变红——只有连通块分解能把它摆出来。
    本包的口径：与金标 manor 同档（manor 实测 4 个小块，最大的 5 格），不追求零死格。
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


def walkable_summary(solid):
    """每行可走列的紧凑描述，用于同步 data/{places,gather}.lua 的"可行走格事实"注释。"""
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


def check_texture_sizes():
    """交叉校验：scene 里声明的 sprite 尺寸必须与贴图像素尺寸一致。

    引擎会把贴图缩放到 `size=` 指定的尺寸，声明错了不会报错、只会被悄悄拉伸
    （芦苇就踩过：art 是 32×40、盘上写 32×32 ⇒ 一丛芦苇被压矮两格）。
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


def standable(solid, walk, cell):
    """采集格的 `facing_cell` 判据：它自己实不实心都行，但要有一个走得到的可走邻格。"""
    x, y = cell
    for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
        nx, ny = x + dx, y + dy
        if 0 <= nx < W and 0 <= ny < H and not solid[ny][nx] and (nx, ny) in walk:
            return (nx, ny)
    return None


def read_enemy_homes():
    """`data/enemies.lua` 里本图敌人的 `home` 格（`{name, cell}`）。

    与 places/gather 同一类契约：home 是出生格，标成实心就是"一只卡在岩壁里的山贼"——
    它不报错，只是玩家在山道上打不齐第一章的那三条击杀目标。
    """
    path = os.path.join(PROJECT_ROOT, "scripts", "data", "enemies.lua")
    if not os.path.exists(path):
        raise FileNotFoundError(f"{path} 读不到：home 格是本图可达性自检的输入")
    out, pending = [], None
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            # 一条记录以 `name = "..."` 起（那是每条的第一个字段），`home` 收尾。
            hit = re.match(r'\s*name\s*=\s*"([^"]+)"', line)
            if hit:
                pending = {"name": hit.group(1), "mountain": False}
                continue
            if pending is None:
                continue
            if re.match(r'\s*map\s*=\s*"mountain_path"', line):
                pending["mountain"] = True
            home = re.match(r'\s*home\s*=\s*\{\s*x\s*=\s*(\d+)\s*,\s*y\s*=\s*(\d+)\s*\}', line)
            if home and pending["mountain"]:
                out.append((pending["name"], (int(home.group(1)), int(home.group(2)))))
                pending = None
    return out


def self_check(ground, solid, walk):
    """契约自检：出生点/出口/落格/地点格/采集格可走可达、门洞可站、栈道可走、锚点不泡水。

    `walk` 由调用方算一次传进来（main 印"采集格的可站邻格"用的是同一个集合，两处必须同源）。
    """
    problems = []
    for name, cell in (("spawn", SPAWN), *EXITS.items()):
        x, y = cell
        if solid[y][x]:
            problems.append(f"{name} ({x},{y}) 变成 solid（出口格不可走）")
        elif cell not in walk:
            problems.append(f"{name} ({x},{y}) 从出生点不可达")
    for name, cell in LANDINGS:
        x, y = cell
        if solid[y][x]:
            problems.append(f"从 {name} 回来的落格 ({x},{y}) 是 solid：一出洞就卡进岩壁")
        elif cell not in walk:
            problems.append(f"从 {name} 回来的落格 ({x},{y}) 从出生点不可达")
    for name, x, y in read_place_cells():
        if solid[y][x]:
            problems.append(f"places.lua:{name} ({x},{y}) 是 solid（NPC 会走不出来）")
        elif (x, y) not in walk:
            problems.append(f"places.lua:{name} ({x},{y}) 从出生点不可达")
        # **地点格不许与出口格四邻相邻**（M72-11 实测出来的契约，不是口味）：出口的
        # priority 20 高于对话/商店 10，而它的 query 既接受"站在出口格上"也接受"面向的那一
        # 格是出口"。山道这里踩过两处：`mountain_gate` 贴着 (19,1)、`mountain_cave_mouth`
        # 贴着 (22,20)，切图落地按 E 时被出口抢走。
        for exit_name, (exit_x, exit_y) in EXITS.items():
            if abs(exit_x - x) + abs(exit_y - y) == 1:
                problems.append(f"places.lua:{name} ({x},{y}) 与出口 {exit_name} "
                                f"({exit_x},{exit_y}) 四邻相邻："
                                "按 E 会被出口（priority 20）抢走对话/商店（priority 10）")
    # 采集节点走 `systems/gather.lua` 的 `facing_cell`：格本身可以实心（莲蓬在水里），
    # 但必须有一个从出生点走得到的可走邻格，否则那格货永远采不到。
    for node in read_gather_nodes():
        hit = standable(solid, walk, node["cell"])
        if hit is None:
            problems.append(f"gather.lua:{node['id']} {node['cell']} 没有任何可从出生点走到的"
                            "可走邻格：`facing_cell` 采不到（这一格的货永久消失）")
    # 敌人 home 与采集格同一类契约，但判据更硬：home 是出生格，实心就是"一只卡在岩壁里的
    # 山贼"——不报错，只是第一章"在山道击败山贼/毒蝎/玄冥会徒"那三条目标打不齐。
    for name, cell in read_enemy_homes():
        x, y = cell
        if solid[y][x]:
            problems.append(f"enemies.lua:{name} home {cell} 是 solid：敌人出生在墙里")
        elif cell not in walk:
            problems.append(f"enemies.lua:{name} home {cell} 从出生点不可达")
    # 门洞：`building_kit.SCENES[SCENE].openings` 声明的洞必须**可站且从出生点走到**——
    # 洞画对了但被别的东西围死，玩家一样出不去（山门是山↔村的出口格，断了就切不了图）。
    for name, cells in bk.openings_table(SCENE).items():
        for cell in cells:
            cx, cy = cell
            if solid[cy][cx]:
                problems.append(f"{name} 的门洞 {cell} 又被标成 solid：占地规则与 openings 打架")
            elif (cx, cy) not in walk:
                problems.append(f"{name} 的门洞 {cell} 从出生点不可达")
    # 建筑/道具锚点（贴图左下角那一格）不许是**水**：锚点在水里就是整栋泡在溪里。
    # 例外必须点名：`over_water=True`（栈道/莲蓬那类"本来就铺在水面上"的美术）。
    for entry in ENTITIES:
        cx, cy = entry["cell"]
        if ground[cy][cx] == WATER and not entry.get("over_water"):
            problems.append(f"{entry['name']} 的锚点 ({cx},{cy}) 在水格上：整件泡在溪里"
                            f"（真要铺在水面上就显式写 over_water=True）")
    # 栈道必须可走（过溪唯一通道），且它真的**跨在水上**——溪被挪走而栈道还在，
    # 画面就是"一条栈道铺在草地上"，而这一条断言只查得出这种事（断桥同理查西涧）。
    for (x, y) in rect(BOARDWALK):
        if solid[y][x]:
            problems.append(f"栈道 ({x},{y}) 是 solid，过不了溪")
    if not any(ground[y][x] == WATER for (x, y) in rect(STREAM)):
        problems.append("溪水里一格都没有：栈道就铺在干地上")
    if not any(ground[y][x] == WATER for (x, y) in rect(GORGE)):
        problems.append("西涧一格水都没有：断桥就跨在一片草地上（M72-11 挪西崖的理由）")
    problems.extend(check_texture_sizes())
    problems += lateral_problems(solid, walk)
    return problems


def lateral_problems(solid, walk):
    """落点：出生点 + 别的图落到本图的每一格，必须可走、可达，且横向走得开一格（判据在 `building_kit`）。

    出口格与门洞那一档**不在这条里**：人是从开阔处走进门洞的，贴脸那一格不需要横向余量；
    而"落到那一格"是另一回事——`to_cell` 是传送的直接产物，落进去动不了就等于卡死。
    本文件的 `EXITS`/`LANDINGS` 与 maps.lua 同时改过才会红，两处抄错一处就抓住。
    """
    registry = bk.read_map_registry(SCENE)
    problems = []
    if registry["spawn"] != SPAWN:
        problems.append(f"data/maps.lua {SCENE}.spawn={registry['spawn']} 与本文件 "
                        f"SPAWN={SPAWN} 不一致：放置格有两个出处")
    declared = {cell for _name, cell in LANDINGS}
    incoming = {(x, y) for _label, x, y in registry["landings"]}
    if declared != incoming:
        problems.append(f"本文件 LANDINGS={sorted(declared)} 与 maps.lua 落到 {SCENE} 的 "
                        f"to_cell={sorted(incoming)} 不一致：落格有两个出处")
    cells = [("spawn", SPAWN)]
    cells += [(label, (x, y)) for label, x, y in registry["landings"]]
    for label, cell in cells:
        if not (0 <= cell[0] < W and 0 <= cell[1] < H):
            continue  # 越界由 `placement_problems` 报，这里不重复也不索引
        if solid[cell[1]][cell[0]] or cell not in walk:
            problems.append(f"{label} 的落点 {cell} 在本图不可走或从出生点不可达")
    got, _readings = bk.placement_problems(W, H, bk.solid_bits(solid), cells)
    return problems + got


def main():
    parser = argparse.ArgumentParser(description="从零装配 scenes/mountain_path.scene")
    parser.add_argument("--out", default=os.path.join("scenes", "mountain_path.scene"),
                        help="写出路径。对拍请指到 .temp 再与盘上场景逐行 diff。")
    args = parser.parse_args()
    ground = build_tiles()
    solid, readings = build_solid(ground)
    text = build_scene(ground, solid)
    with open(args.out, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)
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
    places = read_place_cells()
    nodes = read_gather_nodes()
    homes = read_enemy_homes()
    print(f"契约自检 ok：出生点/出口/落格/门洞/栈道/采集点/建筑锚点，"
          f"places.lua {len(places)} 个山道地点格、gather.lua {len(nodes)} 个采集格、"
          f"enemies.lua {len(homes)} 个 home 格全部可达")
    main_block, dead = pockets(solid)
    print(f"可行走连通块：主块 {len(main_block)} 格，进不去的小块 {len(dead)} 个 "
          + " ".join(f"{len(c)}格@{min(p[0] for p in c)},{min(p[1] for p in c)}"
                     for c in dead))
    # 岸线形状读数（口径同 `gen_manor_scene` 的池塘那一条：形状格 / 外接框格）。
    channel = len(STREAM_CELLS)
    box = (STREAM[2] - STREAM[0] + 1) * (STREAM[3] - STREAM[1] + 1)
    widths = [len(xs) for xs in STREAM_ROWS.values()]
    print(f"   东溪 {channel} 格（含栈道压在水上的 {len(rect(BOARDWALK))} 格）"
          f"/ 外接框 {box} 格 ⇒ 填充率 {channel / box:.3f}"
          f"（旧版 x23..27 × y4..19 的完美方盒是 {box}/{box}=1.000）；"
          f"西涧 {len(GORGE_CELLS)}/12 = {len(GORGE_CELLS) / 12:.3f}；"
          f"每行宽 {min(widths)}..{max(widths)} 格")
    # 每个采集格**实测**的可站邻格（`facing_cell` 取的第一命中）。gather.lua 那些"玩家可站
    # (a)/(b) 面向它"的注释以前是手写的，其中莲子塘那一套差着三格、根本成立不了——
    # 现在这句话由这里印出来，改布局就跟着换，不再靠回忆。
    print("采集格的可站邻格（gather.lua 注释的出处）：")
    for node in nodes:
        print(f"    {node['id']:<18s} 格 {node['cell']} → 面向 "
              f"{standable(solid, walk, node['cell'])}")
    print("可行走格事实（同步 data/{places,gather}.lua 注释用）：")
    for line in walkable_summary(solid):
        print(line)


if __name__ == "__main__":
    main()
