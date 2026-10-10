# -*- coding: utf-8 -*-
"""**拆层登记表的唯一事实源**：哪张整图切成哪两块、切在第几行、谁负责重切。

M72-06 建、当时住在 `gen_manor_art.py` 里（那批图确实都是山庄的）。M72-11 铺量时搬出来，
因为这张表从一开始就不是"山庄的表"而是**全项目的表**：键是整图文件名（stem），值带目录，
所以 `build_*`（归 `gen_interior_art`）、村庄的屋宇（归 `gen_village_art`）与山道的松树/茶棚
（归 `gen_wilds_art`）本来就同在一张表里。
再往 manor 那个文件里加别的场景的 stem，就等于把"这套规范只属于山庄"写进目录结构——
后续六张图会一直撞见它。

三个使用方，都只认这一处：

  * 画整图的那个生成器（`gen_manor_art` / `gen_interior_art` / `gen_village_art` /
    `gen_wilds_art`）在自己收尾时调 `split_stem()` 重切分块 ⇒ 分块与整图同源同刻；
  * `set_foreground_occluders.py` 从 `splits_registry()` 取"哪个实体该拆、遮断件叫什么"；
  * `check_splits()` 是盘上对拍：**两块按行拼回去 == 原图逐字节**（`gen_*_art.py --check`
    与 Rust 遮挡门 §③ 各守一份）。

行号是**图像行**（0 = 顶行）。世界 +y 朝上 ⇒ 顶行是世界最高处，所以"上半张"的
Transform y = 本体 y + 本体高，两块共用同一个 x 原点，宽度也不变——这条等式由
`check_splits` 里的尺寸断言钉住。

用法：`import layer_kit as layers` 于同目录工具中（`lk` 这个短名已经被 `light_kit` 占了，
`set_foreground_occluders` / `set_cast_shadows` 两个文件里两者都要 import）；纯标准库。
"""

import os

from pixel_kit import decode_png, load_canvas, write_png

BUILDING_DIR = os.path.join("assets", "art", "buildings")
PROP_DIR = os.path.join("assets", "art", "props")

# ---------------------------------------------------------------------------
# 切点：不是美学选择，两处各有实测出处
# ---------------------------------------------------------------------------
#   * 老树：切在第 39/40 行之间（树冠 = 顶 40 行）。**这一条是遮挡门量出来的**：树冠每多
#     带走一行，玩家可达位置里的最大覆盖率就往上涨（§④ 扫窗实测：顶 40 行 → 0.604、
#     顶 57 行 → 0.861，见 `lighting.occlusion.max_cover_ratio` 的那一段；0.604 是脚下接触
#     阴影改成"镜像后盖章"（C-1）之后重量的，0.861 是当时那一档的读数、未在新画法下复测，
#     两者不同基线但差值量级（≈0.26）不受这一档漂移影响）。57 行那一档
#     切在"侧叶团下沿 + 树干顶 3 行"，按行拼回去最干净，但画面上人在树冠里只剩一条边；
#     反过来 26 行读成"人站在树前面"。三档渲染出来对着看取的是中间那档：露腿、头在树后。
#     代价只有两块分块 PNG 与 `manor.scene` 里那 5 件实体——整张 `manor_tree.png` 一个字节
#     没动，所以引用整图的 village / combat_arena 不受影响。
#   * 正房：檐口那一条是 `rect(6, 118, 217, 126)`，第 126 行是檐底；切在 127 ⇒
#     屋顶整块（正脊、鸱吻、出挑檐口）进 foreground，墙身+台基留 world。这条没被覆盖率
#     判过死刑：屋顶最深 0.307，是全场第二浅的一处。
#   * 柴房（M72-07）：檐口那条暗边是 `hline(6, 154, 68)`，切在 69 ⇒ 茅草顶进 foreground、
#     木板墙+柴堆+台基留 world。
#   * 五座可建造建筑（M72-07）：切点是 `gen_interior_art.buildable()` 的**匾下沿**。那条函数
#     里 `roof_bottom = max(18, int(height * 0.46))`，匾占 `roof_bottom-2 .. roof_bottom+13`，
#     所以最后一行屋顶 = `roof_bottom + 13`：屋檐带、正脊、**整块匾额**进 foreground，
#     门/窗/柱/台基留 world。切在匾中间会把一块匾按行劈成两半（画面上是匾缺一道），
#     切在檐底 (roof_bottom+2) 又会让匾留在 world、压不住站在匾前的人。
#     这两个数是**抄来的**（一处写死在表里、一处在画图的函数里），漂移的兜底就是下面那条
#     逐字节等式：`buildable()` 改了高度或匾的位置，整图变而分块没变 ⇒ 门当场红。
# **两块的重叠带**：无论切在哪，两块都是原图**按行切开**的，所以有一条恒可判的等式：
#   **两块按行拼回去 == 原图逐字节**（见 `check_splits`，门 §③ 是它的第二份独立实现）。
# **为什么前景块用 `foreground` 而不是按 Y 折进 actor 带**：折 Y 要给每张静态美术
# 各配一份"按脚位算 order"的运行期逻辑（见 `scripts/world/depth.lua` 只做 actor），
# 而本包要的是"树冠/屋顶能压住人"这一条最小可画面效果。代价是可量化的：玩家站在
# 树的**南侧**（本该在人前面）时也会被树冠压到一块——这条由遮挡门按
# `lighting.lua` 的 `occlusion.max_cover_ratio` 钉住上限，不是靠目测放行。
# **键是整图的文件名（stem）**，不是简称：一张表覆盖所有场景的屋宇与树。
#   * 村庄屋宇（M72-11）：**切点 = 檐口下沿；若匾额压在檐口线上，切到匾的下沿**。
#     这条是从 `build_*` 那条"不许按行劈开一块匾"收窄来的：村庄的匾有三种位置——
#      forge 的「铁匠铺」`rect(52..104, y30..44)` 正好骑在茅棚檐线 `hline(…,y30)` 上 ⇒
#     切 44/45（檐下沿会劈开匾）；clinic 的「药庐」(y90..108) 与 shop 的「杂货铺」
#     (y84..108) 全在檐底之下、挂在墙上 ⇒ 切檐底，匾留 world：那是**门面**，站在街上的
#     人本来就在它前面，把门面做成遮断件会让人的头被匾吃掉。
#     其余五件没匾：teastall 布棚底 28、house_a 檐底 51、house_b 茅檐底 48、
#     shrine 檐底 28、clinic 檐底 85。`village_gate` **不拆**：它是通道不是屋子，
#     拆出个"屋顶"会把门洞上方那格额枋画到人脸前面。
LAYER_SPLITS = {
    "manor_tree": (PROP_DIR, (("canopy", 0, 39), ("trunk", 40, 87))),
    "manor_house": (BUILDING_DIR, (("roof", 0, 126), ("body", 127, 223))),
    "manor_shed": (BUILDING_DIR, (("roof", 0, 68), ("body", 69, 127))),
    "build_herb_hut": (BUILDING_DIR, (("roof", 0, 64), ("body", 65, 111))),
    "build_training_hall": (BUILDING_DIR, (("roof", 0, 68), ("body", 69, 119))),
    "build_storehouse": (BUILDING_DIR, (("roof", 0, 64), ("body", 65, 111))),
    "build_tea_shed": (BUILDING_DIR, (("roof", 0, 57), ("body", 58, 95))),
    "build_forge_stand": (BUILDING_DIR, (("roof", 0, 60), ("body", 61, 103))),
    "village_clinic": (BUILDING_DIR, (("roof", 0, 85), ("body", 86, 159))),
    "village_shop": (BUILDING_DIR, (("roof", 0, 80), ("body", 81, 159))),
    "village_forge": (BUILDING_DIR, (("roof", 0, 44), ("body", 45, 63))),
    "village_teastall": (BUILDING_DIR, (("roof", 0, 28), ("body", 29, 63))),
    "village_house_a": (BUILDING_DIR, (("roof", 0, 51), ("body", 52, 95))),
    "village_house_b": (BUILDING_DIR, (("roof", 0, 48), ("body", 49, 95))),
    "village_shrine": (BUILDING_DIR, (("roof", 0, 28), ("body", 29, 63))),
    # 山道两件（M72-11 接山道时补）：
    #   * 松树**照 `manor_tree` 同一行、同一档**切，不是按松树的叶团下沿另定一个切点——
    #     `lighting.occlusion.max_cover_ratio` 那个 0.604 是在"顶 40 行"这个切点上量的，
    #     两张都是 64×88 的树用同一行，遮挡门量的才是同一件事。松树的第三个叶团下沿在
    #     行 64，所以它留在 body 侧：人从松树底下走过去时露腿，与山庄老树同读法。
    #   * 茶棚切在檐口收边那一行（`build_tea_shed` 的 `hline(2, 125, 34)` 是茅顶最底一行）
    #     ⇒ 行 0..34 进 foreground、行 35..79 的棚身/立柱/茶案留 world。
    # `wilds_mountain_gate` **不拆**（同 `village_gate`：它是通道不是屋子，拆出个"屋顶"
    # 会把额枋画到人脸前面）；`wilds_bandit_camp`/`wilds_boardwalk`/`wilds_broken_bridge`
    # 也不拆：敞棚木栅与桥面没有"压住人的顶"。
    "wilds_pine": (PROP_DIR, (("canopy", 0, 39), ("trunk", 40, 87))),
    "wilds_tea_shed": (BUILDING_DIR, (("roof", 0, 34), ("body", 35, 79))),
}

# 遮断实体的名字后缀（也是门侧认"哪块是遮断物"的标识）。树是"冠"，其余都是"顶"。
OCCLOUDER_SUFFIX = {"manor_tree": "Canopy", "wilds_pine": "Canopy"}

# 谁画整图，谁就在自己收尾时重切那张图的分块：分块只有一个来源（这张表）、一个动作
# （`split_stem`），不存在"另一套行号"。按 stem 前缀认归属——村庄的屋宇叫 `village_*`，
# 可建造建筑叫 `build_*`，山庄自己的叫 `manor_*`。
OWNER_BY_PREFIX = (
    ("village_", "gen_village_art"),
    ("build_", "gen_interior_art"),
    ("wilds_", "gen_wilds_art"),
)
DEFAULT_OWNER = "gen_manor_art"


def owner_of(stem):
    for prefix, module in OWNER_BY_PREFIX:
        if stem.startswith(prefix):
            return module
    return DEFAULT_OWNER


OWNER_OF_STEM = {stem: owner_of(stem) for stem in LAYER_SPLITS}


def stems_of(owner):
    """某个生成器负责的 stem 列表（它在 `generate()` 末尾就该把这些切完）。"""
    return tuple(stem for stem in LAYER_SPLITS if OWNER_OF_STEM[stem] == owner)


def occluder_suffix(stem):
    return OCCLOUDER_SUFFIX.get(stem, "Roof")


def split_paths(stem):
    """`<stem>_<part>.png` 的两块（top, bottom）——门侧与工具侧都从这里取切点，
    所以"场景里那两个实体"和"盘上那两张图"不可能各按一套行号切。"""
    out = []
    directory, parts = LAYER_SPLITS[stem]
    for part, y0, y1 in parts:
        out.append((os.path.join(directory, f"{stem}_{part}.png"), y0, y1))
    return out


def full_path(stem):
    directory, _parts = LAYER_SPLITS[stem]
    return os.path.join(directory, f"{stem}.png")


def split_stem(stem):
    """从**盘上的整图**按行切出分块并写盘。返回 `[(path, 行数)]`。

    为什么从盘上读而不是从内存里的 Canvas 读：`build_*` 那五张由 `gen_interior_art.py` 画，
    本模块拿不到它们的 Canvas；而"整图 → 分块"这一步本来就只需要那三行行号。从盘上读还
    白送一条保证：分块与整图同源同刻，不会出现"用旧内存对象切、盘上却是新图"。
    """
    pieces = split_paths(stem)
    path = full_path(stem)
    if not os.path.exists(path):
        raise SystemExit(f"[FAIL] 整图 {path} 不在：分块无从切起（先跑产出它的那个生成器）")
    width, height, _channels, _raw = decode_png(path)
    # 切点必须铺满整图：留一行空白就是"拼回去少一行"，画面上是树冠比树干高一截。
    if pieces[0][1] != 0 or pieces[-1][2] != height - 1 \
            or any(pieces[i][2] + 1 != pieces[i + 1][1] for i in range(len(pieces) - 1)):
        raise SystemExit(f"[FAIL] {stem} 切分没铺满 0..{height - 1}：{pieces}")
    canvas = load_canvas(path)
    written = []
    for piece_path, y0, y1 in pieces:
        write_png(piece_path, canvas.sliced(0, y0, width - 1, y1))
        written.append((piece_path, y1 - y0 + 1))
    return written


def write_splits(stems):
    """把拆层块写盘。**原图继续保留并继续被使用**：`village.scene` 与
    `combat_arena.scene` 还在引用整张 `manor_tree.png`，删掉就是那两张图缺文件；
    `data/buildings.lua` 的 `levels[].sprite` 引用的也是整张 `build_*.png`（建造模式摆出来的
    房子仍是一件），所以一张图都不删。而两块本来就由整图按行切出，同源 ⇒ 不会漂移。
    """
    out = []
    for stem in sorted(stems):
        for path, rows in split_stem(stem):
            out.append((stem, path, rows))
    return out


def check_splits(stems=None):
    """拆层对拍：两块按行拼回去必须与原图**逐字节相同**。

    为什么值得一条等式而不是"两张都在"：拆层的承诺是"画面一个像素不变，只是多了一条
    遮挡带"。若之后只改了原图（调色、加一层叶）而忘了重切，盘上就同时存在两套行——
    画面上是"树冠比树干宽一圈"或"屋顶浮在半空"，而两张图各自都"存在且尺寸合法"，
    任何按文件存在性判的门都测不出来。

    `stems=None` 查**整张表**（含 `build_*` 那五张）：所以只跑一个生成器的 `--check` 也覆盖
    得到别人画的那几张图，不必几个生成器都跑过一遍才敢说分块没漂。
    """
    issues = []
    for stem in sorted(stems if stems is not None else LAYER_SPLITS):
        original = full_path(stem)
        if not os.path.exists(original):
            issues.append("missing " + original)
            continue
        width, height, _, buffer = decode_png(original)
        rebuilt = bytearray(height * width * 4)
        for path, y0, y1 in split_paths(stem):
            if not os.path.exists(path):
                issues.append(f"missing {path}（跑一次 tools/{OWNER_OF_STEM[stem]}.py）")
                continue
            piece_w, piece_h, _, piece = decode_png(path)
            if (piece_w, piece_h) != (width, y1 - y0 + 1):
                issues.append(f"{path}: {piece_w}x{piece_h} != {width}x{y1 - y0 + 1}"
                              "（拆层只切行：宽度与 x 原点不变，两块才共用同一个 Transform.x）")
                continue
            start = y0 * width * 4
            rebuilt[start:start + len(piece)] = piece
        if bytes(rebuilt) != bytes(buffer):
            diff = sum(1 for a, b in zip(rebuilt, buffer) if a != b)
            issues.append(f"{stem}: 两块拼回去与原图差 {diff} 字节——原图改过而拆层没重切")
    return issues
