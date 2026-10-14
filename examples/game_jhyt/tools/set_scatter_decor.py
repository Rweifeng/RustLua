# -*- coding: utf-8 -*-
"""把装饰散布写进一张已存在的 `.scene`（M72-05 建，M72-11 起服务多张图；就地改、幂等）。

用法（工程根目录 `examples/game_jhyt` 下）：

    python tools/set_scatter_decor.py scenes/village.scene          # 只报告
    python tools/set_scatter_decor.py scenes/manor.scene --apply    # 写盘
    python tools/set_scatter_decor.py scenes/manor.scene --seed 0x5C0714 --apply   # 换种子对照

为什么要有这个工具而不是直接重跑 `gen_manor_scene.py`：盘上的 `manor.scene` 早就和生成器
分叉了（HUD 尺寸、几处 transform 是之后手工调的，见 M72-04「未完成项」），重跑会把那批改动
抹掉。本工具**只在文件末尾追加装饰段**，其余字节原样写回，换行风格也跟着保留。

和 `gen_manor_scene.py` 共用 `decor_kit` 的 `scatter()`/`decor_lines()` 与同一份留白输入
（`RESERVED_BY_SCENE` 指向那张图自己的生成器），所以"生成器产出的装饰段"与"本工具写进
盘上的装饰段"必须逐字节相同——这条由 `jhyt_m72_scatter_decor` 的门钉住，不是靠人记得。

留白输入为什么按场景点名：它读的是那张图的**布局常量**（农田矩形、出生点、NPC 地点格、
道具格），这些东西 manor / village / mountain_path 一个都不共用。机制（哈希、带宽、避讳
规则）在 `decor_kit` 里共享，输入必须来自场景自己，否则村子的草会撒在山庄的药田里。

幂等：先按 `entity_meta role decor` 把旧的装饰实体整段删掉，再按当前盘上的地表重撒。
地表本身（`tiles`/`solid`/`tints`）一个字节都不改，所以碰撞、寻路、places 契约不受影响。

依赖：Python 3 标准库 + 同目录 `decor_kit.py`、`terrain_kit.py`、各场景的生成器。
"""

import argparse
import importlib
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import building_kit as bk  # noqa: E402  "哪一行是地表"与场景键的唯一判法
import decor_kit as dk  # noqa: E402
import terrain_kit as tk  # noqa: E402

DECOR_ROLE = "decor"

# 场景键 -> 持有那张图布局常量的生成器模块（它必须导出 `decor_reserved()`）。
# 没登记就红：留白输入静默变成空集，画面照样出图，只是草撒到 NPC 门口。
RESERVED_BY_SCENE = {
    "manor": "gen_manor_scene",
    "village": "gen_village_scene",
    "mountain_path": "gen_mountain_scene",
    # M72-11 ④：演武场的留白是**战斗几何**——六个验收锚点（出生/出口/木人/试招/狼窝/引敌）
    # 与两件器械的占地，撒了草垛就挡住了 `jhyt_m18_combat` 的硬编码站位。
    "combat_arena": "gen_arena_scene",
    # M72-11 §推进顺序 4：寒泉洞的留白是**契约锚点**（出生/出口/四个 places 格/首领 home）
    # 加每件道具的锚点格；这张图只有 240 格，中景压在交互点上是最亏的写法。
    "cold_spring_cave": "gen_cave_scene",
    # M72-11 §推进顺序 5 第一片：铁匠铺内的留白是**切图与交互契约**（出生/出口/forge_inside）
    # 加十一件家具与石龛的锚点格；240 格的屋子里中景一共没几件，压在铁砧前面或门口那一列
    # 是最亏的写法（留白输入的实现见 `gen_forge_scene.decor_reserved()`）。
    "interior_forge": "gen_forge_scene",
    # M72-11 §推进顺序 5 第四片（药庐场景侧）：药庐内的留白与铁匠铺同构（出生/出口/herb_hut_inside 加十二件
    # 家具与石龛的锚点格），差别在**这张图有 kind 2**：`gen_herb_scene.FREEZE` 冻的是
    # `farm.lua` 那个 FARM 矩形与本图的交集，而留白集不吃那条（留白管装饰落点，冻分管抖动）。
    "interior_herb": "gen_herb_scene",
}


def reserved_provider(scene):
    """取这张图的 `decor_reserved()`（返回 (硬留白格集, 只挡中景的格集)）。"""
    module_name = RESERVED_BY_SCENE.get(scene)
    if module_name is None:
        raise SystemExit(f"[FAIL] `set_scatter_decor.RESERVED_BY_SCENE` 里没有场景 {scene!r}："
                         f"请登记它的生成器模块（那张图的留白输入没有别的来源）")
    return importlib.import_module(module_name).decor_reserved


def parse_fields(value):
    return dict(part.split("=", 1) for part in value.split(";") if "=" in part)


def read_ground(lines):
    """从盘上场景里取地表层：返回 (行号, 字段表, cells, solid, tints, width, height)。"""
    index, parts = bk.ground_line(lines)
    parsed = parse_fields(parts[3])
    fmt = int(parsed.get("tiles_format", "0") or 0)
    raw = parsed["tiles"]
    cells = [int(ch) for ch in raw] if fmt == 0 else [int(p) for p in raw.split(",") if p.strip()]
    width, height = int(parsed["width"]), int(parsed["height"])
    solid = parsed["solid"]
    if len(cells) != width * height or len(solid) != width * height:
        raise SystemExit(f"[FAIL] tiles/solid 长度对不上 {width}x{height}")
    tints = {}
    for entry in parsed.get("tints", "").split(","):
        if not entry:
            continue
        cell, tint_id = entry.split(":")
        tints[int(cell)] = int(tint_id)
    return index, parsed, cells, solid, tints, width, height


def strip_decor(lines):
    """删掉旧的装饰实体（`entity`/`component`/`entity_meta` 三类行里 id 属于装饰的那些）。"""
    decor_ids = set()
    for line in lines:
        fields = line.split("\t")
        if len(fields) == 4 and fields[0] == "entity_meta" and fields[2] == "role" \
                and fields[3] == DECOR_ROLE:
            decor_ids.add(fields[1])
    kept = []
    for line in lines:
        fields = line.split("\t")
        if fields[0] in ("entity", "component", "entity_meta", "meta") and fields[1] in decor_ids:
            continue
        kept.append(line)
    return kept, len(decor_ids)


def reserved_ids(lines):
    """别的工具挂在固定段上的实体 id：M72-06 的 `role=occluder`（5001+）与
    `role=cast_shadow`（5101+）。"""
    roles = ("occluder", "cast_shadow")
    return {
        line.split("\t")[1]
        for line in lines
        if len(line.split("\t")) == 4 and line.split("\t")[0] == "entity_meta"
        and line.split("\t")[2] == "role" and line.split("\t")[3] in roles
    }


def next_id(lines):
    """装饰段的起始 id：**跳过别的工具的保留段**（见 `reserved_ids`）。

    不跳的话，前景遮断实体（5001..5005）会把装饰起点从 31 顶到 5006，于是"往盘上加
    5 件纯美术实体"会让 175 件装饰**整体改号**，`--check` 的逐字节对拍红在一个跟装饰
    毫无关系的改动上——这条是加完遮断段实测撞出来的（700 行不同）。
    """
    skip = reserved_ids(lines)
    highest = 0
    for line in lines:
        fields = line.split("\t")
        if fields[0] == "entity" and fields[1].isdigit() and fields[1] not in skip:
            highest = max(highest, int(fields[1]))
    return highest + 1


def decor_section(lines):
    """盘上的装饰段（按行原样取出，用于逐字节比对——不是比长度，见 `check`）。"""
    ids = {
        field[1]
        for field in (line.split("\t") for line in lines)
        if len(field) == 4 and field[0] == "entity_meta" and field[2] == "role"
        and field[3] == DECOR_ROLE
    }
    return [
        line
        for line in lines
        if line.split("\t")[0] in ("entity", "component", "entity_meta")
        and len(line.split("\t")) > 1
        and line.split("\t")[1] in ids
    ]


def check(scene_path, table):
    """确定性、盘上对拍与**反面对照**三件一起测（plan §验收方式）。

    反面对照必须红：换种子之后装饰段内容变了。没有这一条，"逐字节相同"其实可以靠
    "两次都产出空段"或者"只比了行数"蒙过去。
    """
    with open(scene_path, encoding="utf-8", newline="") as handle:
        content = handle.read()
    lines = content.replace("\r\n", "\n").split("\n")
    _, _, cells, solid_str, tints, width, height = read_ground(lines)
    ground = [cells[y * width:(y + 1) * width] for y in range(height)]
    solid = [[int(solid_str[y * width + x]) for x in range(width)] for y in range(height)]
    scene = bk.scene_name(lines)
    exclude, avoid = reserved_provider(scene)()

    def section_for(seed):
        placements = dk.scatter(width, height, ground, solid, exclude,
                                seed=seed, mid_avoid=avoid, scene=scene)
        return dk.decor_lines(1, placements, table,
                              lambda cell: tints.get(cell[1] * width + cell[0], 0), seed)

    problems = []
    # 起点用盘上真实会用的 id，于是三段比对全是**逐字节**的（plan §验收方式的措辞），
    # 不需要"归一化掉 id 再比"那种弱一档的口径。
    start = next_id(strip_decor(lines)[0])

    def section_for(seed):
        placements = dk.scatter(width, height, ground, solid, exclude,
                                seed=seed, mid_avoid=avoid, scene=scene)
        return dk.decor_lines(start, placements, table,
                              lambda cell: tints.get(cell[1] * width + cell[0], 0), seed)

    first, second = section_for(dk.DECOR_SEED), section_for(dk.DECOR_SEED)
    if first != second:
        problems.append(f"同种子两次产出不一致：{len(first)} 行 vs {len(second)} 行")
    other = section_for(dk.DECOR_SEED + 1)
    if other == first:
        problems.append("换种子内容一个字没变：散布根本没读 seed，反面对照失手")
    on_disk = decor_section(lines)
    if on_disk != first:
        drift = [(a, b) for a, b in zip(on_disk, first) if a != b]
        detail = f"{len(drift)} 行不同，首格：\n  盘 {drift[0][0]}\n  算 {drift[0][1]}" if drift else \
            f"行数不同：盘上 {len(on_disk)} vs 重算 {len(first)}"
        problems.append(f"盘上装饰段与重算不一致：{detail}")
    for problem in problems:
        print(f"[FAIL] {problem}")
    print(f"check {'FAIL' if problems else 'ok'}：{len(first)} 行装饰段、"
          f"同种子两次逐字节相同、换种子已变（{len(other)} 行）、与盘上逐字节相同")
    return 1 if problems else 0


def main():
    parser = argparse.ArgumentParser(description="把装饰散布写进 .scene（M72-05）")
    parser.add_argument("scene")
    parser.add_argument("--apply", action="store_true", help="写回文件（默认只报告）")
    parser.add_argument("--seed", default=None, help="覆盖 DECOR_SEED（反面对照用）")
    parser.add_argument("--check", action="store_true",
                        help="只测不改：同种子两次一致、换种子必须变、盘上段与重算逐字节相同")
    args = parser.parse_args()

    with open(args.scene, encoding="utf-8", newline="") as handle:
        content = handle.read()
    newline = "\r\n" if "\r\n" in content else "\n"
    lines = content.replace("\r\n", "\n").split("\n")
    # 装饰贴图的取色按场景的色阶档走，留白输入按场景自己的生成器走，两边都听 `name=` 的。
    scene = bk.scene_name(lines)
    table = tk.load_ramp(scene)
    if args.check:
        raise SystemExit(check(args.scene, table))

    _, _, cells, solid_str, tints, width, height = read_ground(lines)
    ground = [cells[y * width:(y + 1) * width] for y in range(height)]
    solid = [[int(solid_str[y * width + x]) for x in range(width)] for y in range(height)]
    seed = int(args.seed, 0) if args.seed else dk.DECOR_SEED

    lines, removed = strip_decor(lines)
    exclude, avoid = reserved_provider(scene)()
    placements = dk.scatter(width, height, ground, solid, exclude, seed=seed,
                            mid_avoid=avoid, scene=scene)
    start = next_id(lines)
    lines.extend(dk.decor_lines(start, placements, table,
                                lambda cell: tints.get(cell[1] * width + cell[0], 0), seed))

    counts = dk.window_counts(placements, width, height)
    print(f"{args.scene}: 装饰 {len(placements)} 件（清掉旧的 {removed} 件）"
          f" id {start}..{start + len(placements) - 1}  seed={hex(seed)}")
    print(f"  一屏可见对象 min/median/max = {counts[0]}/{counts[1]}/{counts[2]}"
          f"（窗口 20×11.25 格，每 1/4 格取一窗；最坏窗口左下角 {counts[3]}）")
    # 形状分布一起报：某个条目一次都没被抽到 = 白画一张图（香蒲就撞过一次——它要求临水，
    # 而池塘四周一圈实测是夯土与石板，权重只给草地时永远进不了候选）。
    # 但**本场景生态里被点名为"不该出现"的条目不算**（洞穴不该长出草皮与睡莲，
    # 见 `decor_kit.SCENE_DECOR`）：把它们混进"白画一张图"会让一条正确的减法读起来像缺陷。
    histogram = {}
    for item in placements:
        histogram[item["name"]] = histogram.get(item["name"], 0) + 1
    hidden = dk.overrides(scene).get("exclude", frozenset())
    unused = [row[0] for row in dk.CATALOG if row[0] not in histogram and row[0] not in hidden]
    mid = sum(1 for item in placements if item["band"] == "mid")
    print("  形状 " + " ".join(f"{name}={histogram[name]}" for name in sorted(histogram)))
    print(f"  碎屑/中景 = {len(placements) - mid}/{mid}"
          + (f"  一次没抽到：{unused}" if unused else "")
          + (f"  本图生态排除 {len(hidden)} 件" if hidden else ""))
    if args.apply:
        with open(args.scene, "w", encoding="utf-8", newline="") as handle:
            handle.write(newline.join(lines))
        print("  applied")
    else:
        print("  dry-run（加 --apply 才写盘）")


if __name__ == "__main__":
    main()
