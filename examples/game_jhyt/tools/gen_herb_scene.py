# -*- coding: utf-8 -*-
"""《江湖有田》药庐内场景生成（scenes/interior_herb.scene）——第二张上金标的室内图。

用法（工程根目录 `examples/game_jhyt` 下）：

    python tools/gen_herb_scene.py                      # 覆盖盘上场景（默认路径）
    python tools/gen_herb_scene.py --out .temp/h.scene  # 对拍/试跑，不碰盘上场景

**本文件是这张图的唯一事实源**（M72-11 §推进顺序 5 第四片起；片序按落地标题数：第一片=铁匠铺图集侧
`6c58aa04`、第二片=铁匠铺场景侧 `3f99bb8f`、第三片=药庐图集侧 `3710bd9f`、第四片=本笔药庐场景侧），
与 `gen_manor_scene.py` /
`gen_village_scene.py` / `gen_mountain_scene.py` / `gen_arena_scene.py` / `gen_cave_scene.py` /
`gen_forge_scene.py` 同构：同一条代码路径（`building_kit` / `set_foreground_occluders` /
`set_cast_shadows` / `decor_kit` / `set_terrain_autotile` 的抖动与撒布），所以"重跑生成器"与
"就地工具 `set_scatter_decor.py --check`"必须给出同一个字节序列。

**这个文件为什么是新建而不是改名**（旧说法 → 新实测）：这条路径以前是
`tools/gen_interior_scenes.py`，一次 `build_scene` 写**两张**图（herb + forge 各一档）。
铁匠铺那一片（`3f99bb8f`）把它 `git mv` 成 `gen_forge_scene.py` 并**只重写 forge 那一档**，
herb 没有跟着搬——当时的三条理由（旧产物与盘上分叉 3 行、herb 那档把 `"wall"` 记 kind 5
木隔断而 `TILE_NATIVE_SOLID` 要求墙必 4、它的正牌交付物就是本文件）记在
`gen_forge_scene.py` 的 docstring 里，旧实现按
`git show 3f99bb8f^:examples/game_jhyt/tools/gen_interior_scenes.py` 取回。本文件从那条
实现出发、走与另六张同一条管线，**墙因此从 kind 5 搬到 kind 4**（跨图语义位，见下面的换序）。

**解图集的账**（这张图还清的两笔，与铁匠铺同一族）：
  * 旧地表 `assets/art/tilesets/interior_herb/interior_ground.png`（6 列 × 1 行、**恒等寻址**、
    由 `gen_interior_art.py` 出图，773 B）。恒等寻址只有"一整块材质"那一档，于是
    木隔断↏木地板、席垫↏地板的交界全是按格子对齐的直线（docs/71 §1 点名的"像 demo"头号
    成因）。现在用这张图自己的 `assets/art/tilesets/interior_herb/interior_herb_ground.png`
    （12×9，六族 × 17 条目，出自 `gen_terrain_tiles.py` 的 herb 那六段满块 + 三段边画法），
    交界按 4-bit 邻居掩码选过渡块，再叠 `jitter` 的犬牙。旧的 6 列 × 1 行那张随本批下架。
  * 旧版每件家具**手填 `solid_cells`**，改一件 sprite 的坐标就在原地留一片撞空气的悬空格。
    现在串是 `building_kit` 按贴图不透明像素算出来的。

**kind 语义位换序**（旧说法 → 新实测，两边都是 1..6 但含义不同；完整理由记在
`terrain_ramp.json:scenes.interior_herb.comment`，这里是画面侧的那一半）：
  * 旧：`1=木地板 2=青砖地 3=席垫 4=白墙 5=木隔断 6=柜台`，盘上（20×12）实际铺的是
    `1(木地板) 162 格 + 2(青砖地) 18 格 + 5(木隔断) 60 格`，**3/4/6 三列在盘上零格**
    （复现：读该场景 Tilemap 行的 `tiles=` 串按逗号切开做直方图 = `{1:162, 2:18, 5:60}`）。
    那 18 格青砖全部在 y10 那一横排（x1..18）——"靠墙带换档"旧版就有，形状本张保留。
  * 新：`1=herb_plank 木地板 2=herb_dregs 药渣与切屑 3=herb_brick 门厅青砖
    4=herb_partition 木隔断（墙） 5=herb_mat 席垫 6=herb_counter 药柜台`
  换序的四条理由：① **墙必 4**——`TILE_NATIVE_SOLID=(4,6)` 按编号跨图说话，旧档那 60 格"墙"
  记在 5 上，在跨图语义里根本不是墙（这是这张图比铁匠铺**多出来**的一条债：铁匠铺旧 4 已经
  是砖墙、编号本来就对，药庐的墙要搬号，材料一字不换）；② **3 是道路位**
  （`scripts/data/building.lua` 判 tile==3 为道路）⇒ 旧那 18 格青砖搬到 3，门前厅也铺青砖；
  ③ 六列里两列画墙（4=白墙 5=木隔断）⇒ 地面只有三档可用，与铁匠铺同一条病，两张的治法
  合起来是一条规则：**留下盘上真铺的那一列，下架零格的那一列**（铁匠铺下架 5=石墙，本张
  下架 4=白墙）；④ 5 给席垫、6 给药柜台——这两样旧档都是零格列，本张第一次真铺它们。

**药渣（kind 2）为什么这张图不能全场禁，以及它禁在哪里**：铁匠铺那一片把 kind 2 全场禁死，
因为它的身份色住在 5=炉前耐火砖上，2=cinder_scree 只是图集里备着的一族。本图相反——
**身份色就住在 kind 2 这一列上**（`herb_dregs` 是六族里唯一每档 G>R 的绿，读作药碾崩出的末
与切下的茎），全场禁就等于把这张图最像"药庐"的那一档抹掉。但 2 同时是
`scripts/systems/farm.lua` 判可垦的夯土编号位，而 `FARM = {x0=14,x1=33,y0=4,y1=13}`
**没有场景门禁**，它与本图（20×12）的交集是 x14..19 × y4..11 = **48 格**。所以本张的规矩是
**药渣只铺 x13 以西**（`DREGS_ROWS` 外接框 x8..13），并把那 48 格整个冻进 `FREEZE`——
`JITTER_TARGETS=(1,2)` 会把 kind 1 抖成 kind 2（供体含 2），不冻就会有一格药渣自己走进
FARM 矩形。自检 2 测在**抖动后**的载荷上（`surface()` 的结果），不是测常量表。
这不改变"实际 0 格能种地"：运行期开垦还要 `entity.find("FarmGrid")`（docs/58 §15 第 7 条的
三段串联，本图没有那块），冻矩形与禁 kind 2 都是**编号卫生**，措辞按 §推进顺序 4 定下的
口径写，不夸大也不掩盖。

**室内三处必须"有主体"的门约束**（不是给门让路，是让门真的测到东西；三条与铁匠铺同构，
但第 3 条在本图多一层内容）：
  1. `jhyt_m72_building_footprint` 的**反面对照 B**（"把同名拆层件从并集里摘掉必须报出缺
     碰撞"）在没有 `role=building` + 拆层件的图上是**空转**的。所以本图追加一件 `HerbShrine`
     （借 `assets/art/buildings/village_shrine.png` 64×64，`layer_kit.LAYER_SPLITS` 已有拆点：
     顶 29 行进 foreground ⇒ 遮断件 `HerbShrineRoof` 由那条路径挂上）。它屋顶覆盖的
     (7,10)(8,10) 落在**北墙根青砖带**上（不是天然实心）⇒ 摘掉它确实少两格碰撞。演武场
     `ArenaArmory`、洞穴 `CaveShrine`、铁匠铺 `ForgeShrine` 是被同一件事逼出来的，本张第四次。
     **位置为什么是 (7,9) 而不是 (8,9)**（与铁匠铺同号那一格的实测）：石龛 64×64 在 (8,9) 时
     本体不透明像素 x258..318 / y292..323，而灯笼 `Lantern` (9,8) 的 24×44 是 x291..310 /
     y258..299 ⇒ 重叠 **x291..310（20px）× y292..299（8px）**，画面上是龛顶压住灯笼。挪西
     一格后与 `Bookshelf` (4,8)（x129..183）不相干、与灯笼只共边 ⇒ 零重叠。
  2. `building_kit.SCENES["interior_herb"].openings` 是**空表，而且这是登记**：门洞表只服务
     "石体压几格、中间留一个真洞"的门楼，本图唯一的建筑是石龛（四面是墙），而门口格 (10,1)
     站在**开阔门厅青砖**上——`PORCH` 不压在墙行上，天然实心只来自 `TILE_NATIVE_SOLID`
     （隔断 4 与柜台 6）。空表在 `check_openings` 与 Rust 门 §③ 下都是平凡通过（不报错、也不
     假装测过），与洞穴/铁匠铺同一条读法。
  3. `scene_readings.py --gate` 判"装饰件数不得为 0"（散布管线漏跑的证据），所以本张要走
     `decor_kit`，而 `decor_kit.CATALOG` 十三件**全是户外条目** ⇒ 必须在
     `decor_kit.SCENE_DECOR["interior_herb"]` 做**减法**（只摘条目、只挪基线概率档，绝不新增：
     新增会让六张已提交场景的装饰字节全漂）。本图这一档**比铁匠铺多摘一条理由**：柜台在跨图
     编号上占 6=水那一档，而 `DECOR_ON_SOLID=(WATER,)` 与 `neighbours_water()` 都按**编号**
     说话 ⇒ 睡莲会长在药柜台上、香蒲会绕着药柜台扎。这两件必须摘（铁匠铺摘它的理由是"淬火槽
     是作坊设备不是生态水面"，本图摘它是"编号位会自己长出生态"——同一个 exclude 集合，
     不同一条因）。

**家具为什么全部改 `role=prop`**：`building_kit.ROOT_ROLES=("prop",)` 而
`BUILDING_ROLES=("building","building_site")` ⇒ 盘上原来的 `role=furniture`（十一件）/
`role=facility`（两件）**完全不参与占地**，症状正是洞穴那批登记过的"看得见却穿得过"
（48px 宽的方桌只压锚点一格，右边那半格能走过去）。改成 `prop` 后，横着占几格的按
`prop_collision` 点名走 `box`，其余走单格根碰撞（灯笼那类紧包围盒多出上面一格但那不是可撞
体量，与洞穴火把同一档读法）。`furniture`/`facility` 两个 role 名在本图消失，`data/` 与
`scripts/` 侧没有任何一处按 role 筛这两张室内图的家具（`entity.find` 用的是实体名），所以这
是纯碰撞语义修正。

**两件家具必须挪位**（读数不是口味；口径必须是 `building_kit.cells_covered(opaque_world_rect(...))`
——"每轴至少压进 `MIN_CELL_OVERLAP`=8px 的格"，**不是** sprite 的朴素 quad。量具在
`.temp/probe_herb_cells2.py`，与铁匠铺那批的"朴素 quad 会多数出整行"是同一个坑）：
  * `Table` (10,2) → (3,2)：48×40 的方桌在 (10,2) 时不透明像素 x321..367 / y64..95 ⇒ 真口径
    覆盖 **(10,2)(11,2)**（朴素 quad 会多数出整行 y3 的 (10,3)(11,3)，那是虚账）。要紧的是
    (10,2)：它是门前厅 x10 那一列上、出口格 (10,1) 与出生格 (10,3) **之间**的那一格，把它标成
    实心等于把这张图唯一的通路斩断 ⇒ 自检 3（可达性）当场红。与铁匠铺 `Table` 那一条同病，
    盘上两张室内图各自把桌子摆在门口同一列上不是巧合，是旧版"手填 `solid_cells` 没人算过"
    留下的同一种错。
  * `Chair` (11,3) → (4,3)：挪走方桌之后，椅子留在 (11,3) 就是"一把对着空地的椅子"，而它的
    28×40 占地正压门前厅 (11,3) 那一格青砖。椅子 28×40 在 (4,3) 时不透明像素 x131..153 /
    y96..135 ⇒ 只压锚点那一格（root 与 box 同解），与挪过来的方桌 x97..143 在 y 轴上
    **共边不重叠**（桌 64..95、椅 96..135）⇒ 画面上是椅子摆在桌前。
  原位与 `box` 名单一起由自检 3（锚点可走 + 可达）钉住，不靠注释。

**单一光向在室内怎么落**：判据不在这里写，读数是 `lighting.lua` 那两个数——投影偏移长 =
物体总高 × `shadow_reach`(0.12)，够 `min_reach_units`(8) 才配挂地面投影，而
`set_cast_shadows.GROUND_DIRS` 只认 `assets/art/{buildings,props}` 两个目录。本图摆在
`assets/art/{furniture,facilities}` 的十二件（最高的 `Bookshelf` 与 `HerbStove` 72px）因此
**根本不进候选**，唯一进候选的 `HerbShrine` 64px ⇒ reach 7.68 < 8 ⇒ **投影 0 件**，盘上投影段
是空的（`set_cast_shadows.py --check` 测的是"空段与重算逐字节相同"，不是跳过）。这条与铁匠铺
那一条**同一个理由、同一份账**（都是美术目录接线，不是光向判据的答案；洞穴那条才是
"够不够格自己把平行光排除了"），登记给 M72-12 的室内光照收口。

**契约锚点**（每次生成自检，失败非零退出；数值一律从数据侧读回来对拍，不在这里再抄一遍真相）：
  * `data/maps.lua`：`interior_herb.spawn = (10,3)`、出口 `herb_to_village` 格 `(10,1)`
    → `village` (5,14)（村那侧的落点由 `gen_village_scene` 管）；
  * `data/places.lua`：`herb_hut_inside (10,4)` 必须可走、从出生点可达、且站在**青砖**上
    （它是交互点，地表读起来要像"门口铺出来的路"；注意名字是 `herb_hut_inside`，与铁匠铺那
    一条 `forge_inside` 不同名，所以 `HARD_PATH_ANCHORS` 里写的是它自己的名字）；
  * `data/enemies.lua`：本图**没有任何行**（`grep -c interior_herb = 0`）⇒ 没有 home 锚点要
    保护，这条也自检（读回来是空表才继续）；
  * `scripts/core/audio.lua`：`interior_herb = "bgm_village"`（数据侧接线，本文件不改）；
  * 相机 bounds 640×384 = 20×12 格（`pixel_kit.camera_line` 按 W/H 产出，
    `scripts/world/camera_view.lua` 的 `validate_all` 核对"bounds == 地图尺寸"；
    `jhyt_m32_camera.rs` 断言的正是 **interior_herb** 的 20×12——这张图的尺寸一动那条 Rust
    门就红，两张室内图尺寸必须同档）。这张图的**地图就等于一屏**：`scene_readings` 的 20×12
    判据窗口与全图重合，所以 min/median/max 三个读数同形，不是放水（洞穴/铁匠铺那条同款）。

布局分区（cell，0-index，y 朝上，含端点）：
  隔断墙带   ：row0 / row11 / col0 / col19（kind 4，天然实心，60 格）——最后写，任何分区
               （包括席垫与柜台）都不许把墙改成别的 kind
  药庐主地面 ：kind 1 木地板（除下面四块之外全场）
  门前厅     ：x9..11, y1..4 青砖（出生格 (10,3)、出口格 (10,1)、`herb_hut_inside (10,4)`
               都在这里；出口格不许压在墙行上，同演武场 `GATE_ROW` 那条前提）
  北墙根带   ：x1..18, y10 青砖（**旧版就有这一横排**，本张保留其形状、把编号从 2 搬到 3）
  席垫       ：`MAT_ROWS` 按行手填 8 格，外接框 x2..5 × y4..6 = 12 格 ⇒ 填充率 0.667，
               每行宽 3..5 格（榻前与桌前那块铺席；kind 5 在 `BASE_P` 里权重 0 ⇒ 席上不撒装饰）
  药渣与切屑 ：`DREGS_ROWS` 15 格，外接框 x8..13 × y5..7 = 18 格 ⇒ 0.833，每行宽 4..6 格
               （不是方盒；药炉 (9,6) 与药碾台 (12,6) 站在它上面）；**西界 x13 = FARM 矩形以东
               一格都不许有**，见上面"药渣为什么不能全场禁"那一段
  药柜台     ：`COUNTER_ROWS` 按行手填的不规则岸线 7 格，外接框 x16..18 × y4..6 = 9 格
               ⇒ 0.778；**只有 (17,5) 四邻皆柜台** ⇒ `interior_tints` 在本图恰好读 1 格
               （柜台是跨图实心位，沿用池塘/暗河/淬火槽那同一条机制给"看进去最深的那一格"压
               一档冷色；席垫与木地板**不砌岸**——柜台不是水，`paint_herb_counter_edges` 那条
               画法专门画"柜沿压住地板"，与铁匠铺的砌岸是两件事）
  石龛       ：`HerbShrine` (7,9) 64×64 ⇒ 本体 (7,9)(8,9) + 屋顶 (7,10)(8,10)，拆层见上面第 1 条
"""
import argparse
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

# 工程根（`examples/game_jhyt`）。**事实输入**（maps.lua / places.lua / enemies.lua）按这里
# 定位，不跟着 cwd 走：写死的相对路径会让工具在别的目录下静默少一份输入。
PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

from character_art import player_art  # noqa: E402  主角形象（与 manifest 同源）
import building_kit as bk  # noqa: E402  建筑/道具碰撞的单一实现（本图与另六张共用判据）
import decor_kit as dk  # noqa: E402  装饰散布的单一实现（室内那档生态见 `SCENE_DECOR`）
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
SCENE = "interior_herb"
GROUND_TEXTURE = tk.ground_texture(SCENE)

# 列映射（语义见模块 docstring 的"旧说法→新实测"；编号语义跨图不可重排）。
PLANK, DREGS, BRICK, WALL, MAT, COUNTER = 1, 2, 3, 4, 5, 6
# 柜台在跨图编号上占 `terrain_ramp.json` 的 water 那一档（六族表里每一档都叫
# water/dirt/…，场景差异体现在 `family` 上——本图的 `water` 属于 `herb_counter` 族）。
# `load_ramp` 只保证 id 连续且名字集合一致，所以这里不写数字 6，换序要连着 data 侧一起改。
WATER_NAME = "water"

# `scripts/systems/farm.lua` 的 `FARM = {x0=14,x1=33,y0=4,y1=13}` 与本图（20×12）的交集。
# 这个常量**只用来冻抖动**，不是本图的分区（药渣一律铺在它以西）；数值出处见模块 docstring。
FARM_OVERLAP = (14, 4, 19, 11)

# ---- 分区（(x0, y0, x1, y1)，含端点）----
PORCH = (9, 1, 11, 4)           # 门前厅（出生格 (10,3)、出口格 (10,1)、herb_hut_inside (10,4)）
BAND = (1, 10, 18, 10)          # 北墙根青砖带（盘上旧那 18 格原样，编号 2→3）
# 席垫：**按行手填**（外接框 4×3 = 12 格而真铺 8 格 ⇒ 每行宽 3..5，读起来是"榻前摊开的那块"
# 而不是榻榻米间）。三条硬约束同山庄 `POND_ROWS`：相邻两行有公共列、每行 x 集落在外接框内、
# **恰好一格四邻皆席**（自检 7 的读数主体；补成 2×4 方盒就变 2 格，"席心"那一档也没了）。
MAT_ROWS = {
    4: (2, 3, 4),
    5: (2, 3, 4, 5),
    6: (4,),
}
MAT_CELLS = {(x, y) for y, xs in MAT_ROWS.items() for x in xs}
MAT_BOX = (2, 4, 5, 6)
# 药渣与切屑：**只铺 x13 以西**（FARM 矩形那 48 格一格都不许沾，见模块 docstring）。
# 15 格 / 外接框 18 格 ⇒ 0.833，每行宽 4..6（不是方盒；药炉 (9,6) 与药碾台 (12,6) 站在上面）。
DREGS_ROWS = {
    5: (9, 10, 11, 12),
    6: (8, 9, 10, 11, 12, 13),
    7: (9, 10, 11, 12, 13),
}
DREGS_CELLS = {(x, y) for y, xs in DREGS_ROWS.items() for x in xs}
DREGS_BOX = (8, 5, 13, 7)
# 药柜台：西侧那一列往南错开的不规则形状（7 格 / 外接框 9 格 ⇒ 0.778）。**只有 (17,5)
# 四邻皆柜台**，所以 `interior_tints` 在本图读回 1 格——这个数字是自检 7 的主体，形状一改
# （补成 3×3 方盒）就变成 1 格内部格加两边共边，而"柜心压一档"的读法一并失真。
COUNTER_ROWS = {
    4: (16, 17),
    5: (16, 17, 18),
    6: (17, 18),
}
COUNTER_CELLS = {(x, y) for y, xs in COUNTER_ROWS.items() for x in xs}
COUNTER_BOX = (16, 4, 18, 6)
# 抖动冻结集：**FARM 矩形与本图的那 48 格交集**。这张图有 kind 2（铁匠铺没有），而
# `JITTER_TARGETS=(1,2)` 会把 kind 1 抖成 kind 2（`JITTER_DONORS` 含 2）⇒ 不冻就有药渣自己
# 走进"按编号算可垦"的那一列。契约锚点（门前厅与北墙根带）全是 kind 3，抖动根本不碰。
FREEZE = [FARM_OVERLAP]

# ---- 契约锚点（与 data/maps.lua / data/places.lua 一致；enemies.lua 本图没有行）----
SPAWN = (10, 3)                 # `village_to_herb` 的落点，也是本图出生格
EXITS = {"herb_to_village": (10, 1)}
# 这三格要**站在青砖上**（不是木地板/席垫）：它们是切图与交互的落点，地表读起来必须是
# "门口铺出来的路"（`building.lua` 判 tile==3 为道路位，正是这个读法）。名字里那一条是
# `places.lua` 的键名，本图叫 `herb_hut_inside`（铁匠铺那档叫 `forge_inside`）。
HARD_PATH_ANCHORS = ("spawn", "exit", "herb_hut_inside")

# ---- id 段（盘上事实：2..11 furniture、12..13 facility、14 玩家、15/16 HUD、17 相机；18 起是本批追加的）----
FIRST_PROP_ID = 2

# **这里没有 `solid_cells` 字段**（M72-07 的规矩）：占地只认 `building_kit` 一条规则——
# 建筑 = 本体∪同名拆层件的不透明像素覆盖格 − 门洞；道具 = `cell` 那一格，横着占几格的按
# `building_kit.SCENES[SCENE].prop_collision` 点名走 `box`/`walk`。
# 名字/顺序/尺寸/order 与旧版盘上逐条相同 ⇒ id 2..13 一个都不动（挪 id 会让 Lua 侧
# `entity.find` 与黄金表全部重排）；只有 `Table`/`Chair` 的 **cell** 变了，理由记在模块
# docstring 的"两件家具必须挪位"。新增的石龛**追加在 extras 表尾**（id 18），装饰从 19 起。
ENTITIES = [
    {"name": "MedicineCabinet", "texture": "assets/art/furniture/interior_medicine_cabinet.png",
     "size": (64, 64), "cell": (2, 8), "order": 10, "role": "prop"},
    {"name": "Bookshelf", "texture": "assets/art/furniture/interior_bookshelf.png",
     "size": (56, 72), "cell": (4, 8), "order": 10, "role": "prop"},
    {"name": "Bed", "texture": "assets/art/furniture/interior_bed.png",
     "size": (64, 48), "cell": (6, 2), "order": 10, "role": "prop"},
    {"name": "Table", "texture": "assets/art/furniture/interior_table.png",
     "size": (48, 40), "cell": (3, 2), "order": 10, "role": "prop"},
    {"name": "Chair", "texture": "assets/art/furniture/interior_chair.png",
     "size": (28, 40), "cell": (4, 3), "order": 10, "role": "prop"},
    {"name": "Screen", "texture": "assets/art/furniture/interior_screen.png",
     "size": (56, 56), "cell": (14, 3), "order": 10, "role": "prop"},
    {"name": "HerbMill", "texture": "assets/art/furniture/interior_herb_mill.png",
     "size": (40, 28), "cell": (13, 2), "order": 10, "role": "prop"},
    {"name": "Lantern", "texture": "assets/art/furniture/interior_lantern.png",
     "size": (24, 44), "cell": (9, 8), "order": 20, "role": "prop"},
    {"name": "LanternB", "texture": "assets/art/furniture/interior_lantern.png",
     "size": (24, 44), "cell": (12, 8), "order": 20, "role": "prop"},
    {"name": "Chest", "texture": "assets/art/furniture/interior_chest.png",
     "size": (36, 30), "cell": (17, 2), "order": 10, "role": "prop"},
    {"name": "HerbStove", "texture": "assets/art/facilities/facility_herb_stove.png",
     "size": (64, 72), "cell": (9, 6), "order": 5, "role": "prop"},
    {"name": "GrindTable", "texture": "assets/art/facilities/facility_grind_table.png",
     "size": (56, 56), "cell": (12, 6), "order": 5, "role": "prop"},
]

# 登记在**相机之后**的那批实体（盘上这一段是空的，所以旧实现没有抹掉什么的风险；本批加的
# 是一件真建筑，写在相机之后与另六张同一条拼接顺序）。
EXTRAS = [
    # 石龛（理由见模块 docstring「室内三处必须有主体的门约束」第 1 条）：借村里土地庙那张
    # 64×64，`layer_kit.LAYER_SPLITS["village_shrine"]` 已有拆点（顶 29 行进 foreground）⇒
    # 遮断件 `HerbShrineRoof` 由 `set_foreground_occluders` 那条路径挂上，本文件不写它的行。
    # 总高 64 ⇒ reach 7.68 < 8 ⇒ **不配投影**，与村里三栋矮屋/铁匠铺石龛走同一条判据（不是名单）。
    {"name": "HerbShrine", "texture": "assets/art/buildings/village_shrine.png",
     "size": (64, 64), "cell": (7, 9), "order": 0, "role": "building"},
]

PLAYER_ID = FIRST_PROP_ID + len(ENTITIES)
TITLE_ID = PLAYER_ID + 1
HINT_ID = TITLE_ID + 1
CAMERA_ID = HINT_ID + 1
EXTRAS_ID = CAMERA_ID + 1
DECOR_START_ID = EXTRAS_ID + len(EXTRAS)

HUD_TITLE = "药庐 · 药材与药炉"
HUD_HINT = "WASD 移动 · E 使用设施/出门 · T 睡觉 · Q 吐纳"
# 这两行 UiLayout 的数值以**盘上场景**为准（旧版 HUD 是手工调过的那一档，重跑不得弹回）。
HUD_TITLE_OFFSETS = "24.00,12.00,500.00,36.00"
HUD_HINT_OFFSETS = "24.00,40.00,560.00,64.00"


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
    """盘上 id 2..13 的那批家具/设施（在玩家之前），与 `occupied_cells` 同源。"""
    return entity_lines_of(FIRST_PROP_ID, ENTITIES)[0]


def occupied_cells(ground):
    """每件建筑/道具该实心哪些格——**只认 `building_kit` 这一条规则**。

    拆层要先过一遍 `fo.plan`：占地是"本体∪同名遮断件"的不透明格并集，石龛屋顶那两格由它
    决定——少了这一步，屋檐就会判成"悬空碰撞"而被删掉实心（玩家可以走进石龛里）。

    第三个返回值 `{建筑名: {拆层件名: 覆盖格}}` 是给自检 6 的：`visual_and_solid` 的读数只给
    **格数**不给格子，而那条自检要的是"屋顶自己那几格压在什么地表上"。这里按件分开算，用的
    还是 `building_kit` 那一条 `cells_covered(opaque_world_rect(...))`，不是第二套算法。
    """
    lines, _ = entity_lines_of(FIRST_PROP_ID, ENTITIES)
    extras, _ = entity_lines_of(EXTRAS_ID, EXTRAS)
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

    药渣（kind 2）只铺在 `DREGS_ROWS` 那 x13 以西的 15 格里，见模块 docstring 的 farm.lua 段。
    """
    ground = [[PLANK] * W for _ in range(H)]
    for region in (PORCH, BAND):
        for (x, y) in rect(region):
            ground[y][x] = BRICK
    for (x, y) in MAT_CELLS:
        ground[y][x] = MAT
    for (x, y) in DREGS_CELLS:
        ground[y][x] = DREGS
    for (x, y) in COUNTER_CELLS:
        ground[y][x] = COUNTER
    # **这里没有"砌岸"那一步**（铁匠铺的淬火水槽 4 邻域里仍是夯土的会刷成石板）：柜台不是水，
    # 木地板↔柜台的交界由 `gen_terrain_tiles.paint_herb_counter_edges` 自己画"柜沿压住地板"，
    # 中间垫一档青砖反而读成"柜台砌在砖地里"。席垫同理——席是**摊在板上**的，边画法露的就是板。
    # 隔断墙最后写：地图边界四行/四列都是墙，任何分区都不许把墙改成别的 kind。
    for x in range(W):
        ground[0][x] = WALL
        ground[H - 1][x] = WALL
    for y in range(H):
        ground[y][0] = WALL
        ground[y][W - 1] = WALL
    return ground


def decor_reserved():
    """M72-05 的玩法留白：返回 (两带共同的硬留白, 只挡中景带的留白)。

    输入全部来自本文件的布局常量与 `data/places.lua`，所以 `set_scatter_decor.py` 只要
    import 这里，两条路就必然撒出同一段装饰。

    **硬留白是空集**：这张图没有农田地块（药渣那一档是**地表**不是田，见模块 docstring），
    也没有 NPC 站桩与敌人 home。会挡视线的是**中景带**（order 12 那一档），所以出生格、
    出口格、`herb_hut_inside` 与每件道具的锚点都进 `mid_avoid`——240 格的屋子里中景一共没几件，
    把它压在交互点与药炉前面是最亏的写法。
    """
    props = [entry["cell"] for entry in ENTITIES + EXTRAS]
    places = [(x, y) for _, x, y in read_place_cells()]
    keys = [SPAWN, *EXITS.values()]
    return set(), dk.mid_avoid(W, H, props, places, keys)


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
    # 柜心按**形状**压一档 `shaded`（表里第 1 档是那档最冷的）：机制与山庄池塘/洞穴暗河/
    # 铁匠铺淬火槽那一条完全相同——autotile 开着时四邻皆同类的内部格全部落进同一个图集条目，
    # 换瓦要动色阶表与图集布局，而现成的那一档冷色能立刻用（代价是深度差只有 3-4%）。
    # **本图的读法要换个说法**：柜台不是水，压暗的那一格读的是"柜面往里那一片的阴影"。本图
    # 只有 (17,5) 一格满足"四邻皆柜台"，所以这一条只压 1 格（形状一改读数就跟着改，见自检 7）。
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
        # 场景名必须等于地图键（data/maps.lua 的 `interior_herb`）：main.lua 用
        # `is_playable(scene.reload())` 判定玩法地图，写错就整局进不去。
        f"name={SCENE}",
        "meta\tdisplay_name\t云泽村·药庐内",
        "meta\tgame\tjianghu_youtian",
        "meta\ttile_size\t32",
        # 横屏方案A：设计分辨率 1280×720（与 `scripts/ui/layout.lua` 一致）。
        "meta\tui_reference_height\t720",
        "meta\tui_reference_width\t1280",
        "entity\t1\tGroundMap",
        transform_line(1, 0.0, 0.0),
        # 逻辑瓦 + autotile + per-tile tint（口径见 `terrain_kit.py` 与 M72-04）。旧版这里是
        # 6 列 × 1 行的**恒等寻址**，且 `tiles=` 之外没有 `tiles_format/atlas_cols/tints` 三段。
        "component\t1\tTilemap\t"
        + tk.tilemap_component(TILE, W, H, GROUND_TEXTURE, cells, solid_str,
                               table["cols"], table["rows"],
                               tile_kinds=table["kind_count"], tints=tints,
                               tints_rgb=tk.tints_rgb_string(table)),
        "entity_meta\t1\trole\tground_tilemap",
    ]
    # 2..13 家具/设施：与 `occupied_cells` 走同一个 `entity_lines_of`，占地算的就是画出来的这批。
    lines.extend(structural_lines())
    # 玩家本体：场景内可碰撞动态刚体（脚本经 entity.find("JianghuPlayer") 使用）
    spawn_wx, spawn_wy = cell_to_world(*SPAWN)
    lines.append(f"entity\t{PLAYER_ID}\tJianghuPlayer")
    # 主角形象（M21）：单帧 + 序列帧都从角色美术 manifest 取，与 data/characters.lua 是
    # 同一套串；运行期 player.apply() 会按 state.appearance 纠正并重挂，此处给默认形象。
    player_sprite, player_animation = player_art("player_male")
    lines.append(f"component\t{PLAYER_ID}\tSprite\t{player_sprite}")
    if player_animation:
        lines.append(f"component\t{PLAYER_ID}\tAnimation\t{player_animation}")
    lines.append(f"component\t{PLAYER_ID}\tCollider\tshape=ball;radius=12;offset=0,-16")
    lines.append(transform_line(PLAYER_ID, spawn_wx, spawn_wy))
    lines.append(f"entity_meta\t{PLAYER_ID}\trole\tplayer_spawn")
    lines.append(f"entity_meta\t{PLAYER_ID}\tpersistent\ttrue")
    lines.append(f"entity\t{TITLE_ID}\tHudTitle")
    lines.append(
        f"component\t{TITLE_ID}\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;"
        f"offsets={HUD_TITLE_OFFSETS};min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000"
    )
    lines.append(
        f"component\t{TITLE_ID}\tText\ttext={HUD_TITLE};font_size=18.00;color=0.949,0.918,0.847"
    )
    lines.append(transform_line(TITLE_ID, 0.0, 0.0))
    lines.append(f"entity_meta\t{TITLE_ID}\trole\thud_title")
    lines.append(f"entity\t{HINT_ID}\tHudHint")
    lines.append(
        # 地图自带操作提示放在**地图标题正下方**；底部信息区整块留给 HUD 的动态反馈/键位行。
        f"component\t{HINT_ID}\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;"
        f"offsets={HUD_HINT_OFFSETS};min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000"
    )
    lines.append(
        f"component\t{HINT_ID}\tText\ttext={HUD_HINT};font_size=14.00;color=0.850,0.900,1.000"
    )
    lines.append(transform_line(HINT_ID, 0.0, 0.0))
    lines.append(f"entity_meta\t{HINT_ID}\trole\thud_hint")
    # 场景相机（本地图唯一的 `Camera` 实体）：把 2D 取景范围锁在整张地图的矩形内，
    # 运行期由 `scripts/world/camera_view.lua` 让相机跟随玩家（`camera2d.follow`）。
    # 几何与理由见 `pixel_kit.camera_line`；`validate_all` 会在启动/`--check` 时核对
    # "场景 bounds == 地图尺寸"。这张图 20×12 ⇒ bounds 640×384 = **地图就是一屏**，
    # 也正是 `jhyt_m32_camera.rs` 钉死 interior_herb 的那一组数。
    lines.append(f"entity\t{CAMERA_ID}\tGameCamera")
    lines.append(camera_line(CAMERA_ID, W, H))
    lines.append(transform_line(CAMERA_ID, 0.0, 0.0))
    lines.append(f"entity_meta\t{CAMERA_ID}\trole\tgame_camera")
    # 18.. 相机之后那批（石龛）：**id 段与另六张同一套拼接顺序**，装饰因此从 19 起
    # （`set_scatter_decor.next_id` 取的是"盘上最大非保留 id + 1"，两条路必须给同一个数）。
    extras_lines, decor_start = entity_lines_of(EXTRAS_ID, EXTRAS)
    lines.extend(extras_lines)
    if decor_start != DECOR_START_ID:
        raise SystemExit(f"[FAIL] 装饰起始 id {decor_start} != 常量 DECOR_START_ID "
                         f"{DECOR_START_ID}：id 段与 set_scatter_decor 的算法分叉了")
    # M72-05：装饰散布。输入用**抖动后**的格子图（盘上就是这个），不然"生成器看到的
    # 木地板/青砖分布"与实机不一致。本图的生态档（户外十三件里摘掉哪几件、哪一档概率挪到
    # 多少）在 `decor_kit.SCENE_DECOR[SCENE]`，靠 `scene=SCENE` 生效。
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
    # 落地行，也按拆层后的实体排除遮断块（空中块不配自己的投影）。本图够格件数为 0（理由与
    # 铁匠铺那一条**同一条**、与洞穴那条**不同一条**，见模块 docstring），所以这里追加的是
    # **空段**；跑这条路径而不是跳过它，`--check` 才测得到"空段与重算逐字节相同"这件事。
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

    与山道/演武场/洞穴/铁匠铺同口径：一张图里"看得见但走不进去"的木地板不会让任何一条锚点
    断言变红，只有连通块分解能把它摆出来。室内尤其要紧——屋子本来就只有 240 格，一块进不去
    的角落等于白画一片地表，而家具密排的图恰恰最容易把一角围死。
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

    读数据侧那一份而不是在本文件再抄一个点：改 places.lua 而生成器不知道，症状是
    "地点落在一堵墙里"而不是报错。
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
    """`data/enemies.lua` 里本图敌人的 `home` 格（本图应为**空表**）。

    这条读回来而不是省略，是为了让"药庐里没有敌人"这件事是一个**读数**：它一旦变成非空
    （将来在屋里摆个练功靶），home 格就得和 spawn 一样进可达性自检，而那时候本文件的
    `decor_reserved()` 也要跟着把它挡在中景带之外。
    """
    if not os.path.exists(path):
        raise FileNotFoundError(f"{path} 读不到：home 格是本图可达性自检的输入")
    out, pending = [], None
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            hit = re.match(r'\s*name\s*=\s*"([^"]+)"', line)
            if hit:
                pending = {"name": hit.group(1), "here": False}
                continue
            if pending is None:
                continue
            if re.match(r'\s*map\s*=\s*"' + SCENE + r'"', line):
                pending["here"] = True
            home = re.match(r'\s*home\s*=\s*\{\s*x\s*=\s*(\d+)\s*,\s*y\s*=\s*(\d+)\s*\}', line)
            if home and pending["here"]:
                out.append((pending["name"], (int(home.group(1)), int(home.group(2)))))
                pending = None
    return out


def read_registry(path=os.path.join(PROJECT_ROOT, "scripts", "data", "maps.lua")):
    """从 data/maps.lua 读 interior_herb 的 spawn/出口格，做三方一致性校验（不执行 Lua）。"""
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
    （含 `GROUND_DIRS` 那两个目录）决定，本文件不写第二套判据。打这一行是为了让"室内没有
    投影"这件事在日志里是**一个读数**而不是一句散文。
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


def anchor_map():
    """本图所有契约锚点（名字 → 格子）：自检与打印共用一份。"""
    anchors = {"spawn": SPAWN, "exit": EXITS["herb_to_village"]}
    anchors.update({name: (x, y) for name, x, y in read_place_cells()})
    for index, cell in enumerate(cell for _, cell in read_enemy_homes()):
        anchors[f"enemy_home_{index + 1}"] = cell
    return anchors


def in_farm_overlap(cell):
    """这一格是否落在 `farm.lua` 那个 `FARM` 矩形与本图的交集里（只用于自检 2）。"""
    x0, y0, x1, y1 = FARM_OVERLAP
    return x0 <= cell[0] <= x1 and y0 <= cell[1] <= y1


def self_check(ground, solid, walk, mate_map):
    """契约自检：锚点可走可达、药渣不进 FARM 矩形、石龛的拆层真的有碰撞后果、三方一致。

    `ground` 是**抖动后**的格子图（调用方传 `surface()` 的结果），`mate_map` 是
    `occupied_cells` 给的 `{建筑名: {拆层件名: 覆盖格}}`。
    """
    problems = []
    # 1) kind 值域：地表层只许 1..6（越界就是引擎按条目号切 UV 的那一类错位画面）。
    for y in range(H):
        for x in range(W):
            if ground[y][x] < 1 or ground[y][x] > 6:
                problems.append(f"({x},{y}) kind={ground[y][x]} 超出 1..6")
    # 2) farm.lua 防误垦：**药渣不许进 FARM 矩形那 48 格**（测在抖动后的载荷上）。这条与
    #    铁匠铺"全场禁 kind 2"不同——本图的身份色就住在 kind 2，禁不得，只能圈住它。
    dregs_in_farm = [(x, y) for y in range(H) for x in range(W)
                     if ground[y][x] == DREGS and in_farm_overlap((x, y))]
    if dregs_in_farm:
        problems.append(f"药渣(kind 2)落进 farm.lua 的 FARM 矩形 {len(dregs_in_farm)} 格"
                        f"（首格 {dregs_in_farm[0]}）：那一列按编号读作可开垦田，"
                        "FREEZE 没冻住 ⇒ 检查 DREGS_ROWS 的西界与 FARM_OVERLAP")
    #    反向对照：药渣这一档是本图的身份色，一格都没有就是分区被挪飞了（与上面一条成对）。
    if not any(ground[y][x] == DREGS for (x, y) in rect(DREGS_BOX)):
        problems.append("药渣外接框里一格都没有：DREGS_ROWS 被挪出外接框了")
    # 3) 契约锚点：可走 + 从出生点可达 + 走线三格站青砖。
    anchors = anchor_map()
    for name, (x, y) in anchors.items():
        if solid[y][x]:
            problems.append(f"锚点 {name} ({x},{y}) 是 solid")
        elif (x, y) not in walk:
            problems.append(f"锚点 {name} ({x},{y}) 从出生点不可达")
        elif name in HARD_PATH_ANCHORS and ground[y][x] != BRICK:
            problems.append(f"锚点 {name} ({x},{y}) 地表 kind={ground[y][x]}，"
                            "走线三格（出生/出口/herb_hut_inside）应为青砖(3)")
    # 4) 出口格必须在门前厅里：厅被挪走而 maps.lua 没跟着改 = 一脚踏进隔断墙。
    if set(EXITS.values()) - {(x, y) for (x, y) in rect(PORCH)}:
        problems.append(f"出口格 {sorted(EXITS.values())} 不在门前厅 {PORCH} 里")
    # 5) 家具/建筑锚点（贴图左下角那一格）不许是**柜台**：锚点在柜台上就是整件摆在柜台里。
    for entry in ENTITIES + EXTRAS:
        cx, cy = entry["cell"]
        if ground[cy][cx] == COUNTER:
            problems.append(f"{entry['name']} 的锚点 ({cx},{cy}) 在柜台格上：整件坐在药柜台里")
    # 6) 石龛的拆层必须有碰撞后果：屋顶那几格要落在**非天然实心**的地表上。否则
    #    `jhyt_m72_building_footprint` 的反面对照 B 空转（演武场/洞穴/铁匠铺那条同一个坑）。
    shrine = next(entry for entry in EXTRAS if entry["name"] == "HerbShrine")
    roof_cells = sorted((cell for mates in mate_map.get(shrine["name"], {}).values()
                         for cell in mates
                         if ground[cell[1]][cell[0]] not in bk.TILE_NATIVE_SOLID))
    if not roof_cells:
        problems.append("HerbShrine 的拆层件压在墙/柜台行上或根本没有覆盖格：摘掉它一格碰撞都不少，"
                        "反面对照 B 会空转——把石龛挪到可走青砖旁边")
    # 7) 席垫、柜台都得真有格子，且**席心与柜心各恰好一格四邻皆同类**（`interior_tints` 的
    #    读数与 `decor_kit` 的"席上不撒装饰"那一档都住在这两个数字上）。数的是抖动后的载荷：
    #    `JITTER_TARGETS=(1,2)` 根本不碰 kind 5/6，所以这两个数字与常量表同形——测在载荷上
    #    是为了让"形状被挪"这一类改动当场红，而不是靠注释保证。
    if not any(ground[y][x] == MAT for (x, y) in rect(MAT_BOX)):
        problems.append("席垫外接框里一格都没有：MAT_ROWS 被挪出外接框了")
    for label, kind, want in (("席垫", MAT, 1), ("药柜台", COUNTER, 1)):
        inner = [(x, y) for y in range(H) for x in range(W)
                 if ground[y][x] == kind and all(
                     0 <= x + dx < W and 0 <= y + dy < H and ground[y + dy][x + dx] == kind
                     for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)))]
        if len(inner) != want:
            problems.append(f"{label}四邻皆同类的内部格有 {len(inner)} 格 {sorted(inner)}，"
                            f"本图按形状只有 {want} 格：改了形状，`interior_tints`/装饰那一档的"
                            "读数就跟着改（见模块 docstring）")
    # 8) 席垫/柜台/药渣三块各自要连成一片：相邻两行必须有公共列（否则是两块）。
    for label, rows in (("席垫", MAT_ROWS), ("药渣", DREGS_ROWS), ("药柜台", COUNTER_ROWS)):
        order = sorted(rows)
        for a, b in zip(order, order[1:]):
            if not set(rows[a]) & set(rows[b]):
                problems.append(f"{label} y{a} 与 y{b} 两行没有公共列：断成两块")
    # 9) maps.lua 三方一致（spawn / 出口格）；本图 enemies.lua 应为空表（见函数注释）。
    registry = read_registry()
    if registry["spawn"] != SPAWN:
        problems.append(f"maps.lua spawn={registry['spawn']} 与本文件 SPAWN={SPAWN} 不一致")
    if registry["exit"] != EXITS["herb_to_village"]:
        problems.append(f"maps.lua 出口格={registry['exit']} 与本文件 "
                        f"herb_to_village={EXITS['herb_to_village']} 不一致")
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
    parser = argparse.ArgumentParser(description="从零装配 scenes/interior_herb.scene")
    parser.add_argument("--out", default=os.path.join("scenes", "interior_herb.scene"),
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
    print(f"wrote {args.out} ({W}x{H}, {len(ENTITIES)} 个家具/设施 + {len(EXTRAS)} 件建筑，"
          f"id 2..{CAMERA_ID} 与盘上同号、石龛 {EXTRAS_ID}、装饰从 {DECOR_START_ID} 起)")
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
    anchors = {"spawn": SPAWN, "exit": EXITS["herb_to_village"],
               **{name: (x, y) for name, x, y in read_place_cells()}}
    print(f"契约自检 ok（测在**抖动后**载荷上，{changed} 格被抖）：锚点 "
          f"{' '.join(f'{name}({x},{y})' for name, (x, y) in anchors.items())} "
          f"全部可走、可达；药渣 {sum(1 for row in ground for v in row if v == DREGS)} 格"
          f"且 FARM 矩形 {FARM_OVERLAP} 内 0 格；出口格在门前厅；places.lua "
          f"{len(read_place_cells())} 格、enemies.lua {len(read_enemy_homes())} 个 home；"
          f"maps.lua 一致")
    print("   投影够格读数（总高 × shadow_reach vs min_reach_units="
          f"{min_reach}；候选只有 assets/art/{{buildings,props}}）："
          + " ".join(f"{name}({height}px={reach:.2f}{'' if ok else ' 不挂'})"
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
    for label, rows, box, kind in (("席垫", MAT_ROWS, MAT_BOX, MAT),
                                   ("药渣", DREGS_ROWS, DREGS_BOX, DREGS),
                                   ("药柜台", COUNTER_ROWS, COUNTER_BOX, COUNTER)):
        cells = sum(len(xs) for xs in rows.values())
        area = (box[2] - box[0] + 1) * (box[3] - box[1] + 1)
        widths = [len(xs) for xs in rows.values()]
        laid = sum(1 for y in range(H) for x in range(W) if ground[y][x] == kind)
        print(f"   {label} 常量 {cells} 格 / 外接框 {area} 格 ⇒ 填充率 {cells / area:.3f}"
              f"（每行宽 {min(widths)}..{max(widths)} 格，不是方盒）；抖动后实铺 {laid} 格")
    print("可行走格事实（文档同步用）：")
    for line in walkable_summary(solid):
        print(line)


if __name__ == "__main__":
    main()
