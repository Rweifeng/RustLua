# -*- coding: utf-8 -*-
"""把前景遮断块（树冠 / 屋顶）写进一张已存在的 `.scene`（M72-06，就地改、幂等）。

用法（工程根目录 `examples/game_jhyt` 下）：

    python tools/set_foreground_occluders.py scenes/manor.scene           # 只报告
    python tools/set_foreground_occluders.py scenes/manor.scene --apply   # 写盘
    python tools/set_foreground_occluders.py scenes/manor.scene --check   # 盘上对拍

做了什么：每张引用了**已拆层原图**（`manor_tree.png` / `manor_house.png`）的 Sprite，
本体换成"下面那一块"（树干 / 墙身+台基，留在原来的层与 order），并在它上方**多挂一个
实体**画"上面那一块"（树冠 / 屋顶），层是 `foreground`、order 取 `data/lighting.lua` 的
`foreground_order.occluder`。两块共用同一个 x，遮断块的 y = 本体 y + 本体高 ⇒
按行拼回去就是原图（这条等式由 `gen_manor_art.py --check` 钉住）。

为什么是"就地改盘上场景"而不是重跑 `gen_manor_scene.py`：盘上的 `manor.scene` 早就和
生成器分叉（M72-04「未完成项」记的 HUD/transform 手调），重跑会抹掉那批改动。这条与
`set_scatter_decor.py` 是同一条理由。

**id 段**：遮断实体用 `5001` 起的保留段，不参与"盘上最大 id + 1"的自然分配。不这么做
的话，`set_scatter_decor.py` 重跑时 id 起点会因本工具插进来的实体整体位移，它那条
"盘上装饰段 == 重算逐字节相同"的对拍就红在跟装饰毫无关系的改动上。

切点、贴图路径、遮断 order **全部从别处取**（`gen_manor_art.LAYER_SPLITS` 与
`light_kit`），本文件不写死任何一个数：写死就有第二套真相，而漂移的表现是"树冠浮在
树干上方一截"，不会报错。

依赖：Python 3 标准库 + 同目录 `pixel_kit.py`、`decor_kit.py`、`gen_manor_art.py`、`light_kit.py`。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import decor_kit as dk  # noqa: E402  sprite_line：字段顺序与引擎 `Sprite::to_component_str` 一致
import gen_manor_art as ga  # noqa: E402  LAYER_SPLITS：切点与原图路径的定义处
import light_kit as lk  # noqa: E402  foreground_order 的定义处
from pixel_kit import transform_line  # noqa: E402

ROLE = "occluder"
SORTING_LAYER = "foreground"
# 保留 id 段：见模块 docstring「id 段」。低于此值的都是自然分配的可玩法实体与装饰。
OCCLUDER_ID_BASE = 5001
# 遮断实体名后缀在 `gen_manor_art.occluder_suffix`（树是 Canopy、其余是 Roof）：
# 门侧靠它认"哪块是遮断物"，所以只许有一处定义。
WHITE_TINT = "1.000,1.000,1.000,1.000"


def posix(path):
    return path.replace("\\", "/")


def groups():
    """`{整图名: {source, base, occluder, suffix}}`，路径一律 posix（场景里只认正斜杠）。

    `LAYER_SPLITS` 每项是 `(目录, ((部件名, y0, y1), ...))`，且**第 0 项是上半张**
    （图像行号小 = 靠顶 = 世界更高）。本体取末项、遮断取首项——顺序本身就是契约。
    键是**整图文件名**（`manor_house` / `build_herb_hut`…）：切点、两块路径、遮断后缀
    全从 `gen_manor_art` 那一处取，本文件不写死任何一个数。
    """
    out = {}
    for stem in ga.LAYER_SPLITS:
        pieces = [(posix(path), y1 - y0 + 1) for path, y0, y1 in ga.split_paths(stem)]
        out[stem] = {
            "source": posix(ga.full_path(stem)),
            "occluder": pieces[0],
            "base": pieces[-1],
            "suffix": ga.occluder_suffix(stem),
        }
    return out


def parse_fields(value):
    return dict(part.split("=", 1) for part in value.split(";") if "=" in part)


def read_entities(lines):
    """返回 (Sprite 载荷按 id, Transform 载荷按 id, 实体名按 id)。"""
    sprites, transforms, names = {}, {}, {}
    for line in lines:
        fields = line.split("\t")
        if len(fields) < 3:
            continue
        if fields[0] == "entity":
            names[fields[1]] = fields[2]
        elif fields[0] == "component" and fields[2] == "Sprite":
            sprites[fields[1]] = fields[3]
        elif fields[0] == "component" and fields[2] == "Transform":
            transforms[fields[1]] = fields[3]
    return sprites, transforms, names


def transform_xy(payload):
    position = parse_fields(payload).get("position", "0,0,0").split(",")
    return float(position[0]), float(position[1])


def swap_sprite(payload, texture, width, height):
    """只换 `texture` 与 `size`，其余字段（pivot/tint/层/order）原样留着——
    本体留在原来的层与 order 正是设计的一部分（树 20 / 房 0），本包只往上加遮断块。"""
    fields = parse_fields(payload)
    out = payload.replace(f"texture={fields['texture']}", f"texture={texture}")
    return out.replace(f"size={fields['size']}", f"size={width}x{height}")


def occluder_ids(lines):
    return {
        line.split("\t")[1]
        for line in lines
        if len(line.split("\t")) == 4 and line.split("\t")[0] == "entity_meta"
        and line.split("\t")[2] == "role" and line.split("\t")[3] == ROLE
    }


def occluder_section(lines):
    ids = occluder_ids(lines)
    return [line for line in lines
            if len(line.split("\t")) > 1 and line.split("\t")[0] in ("entity", "component", "entity_meta")
            and line.split("\t")[1] in ids]


def strip_occluders(lines):
    ids = occluder_ids(lines)
    kept = [line for line in lines
            if not (len(line.split("\t")) > 1
                    and line.split("\t")[0] in ("entity", "component", "entity_meta")
                    and line.split("\t")[1] in ids)]
    return kept, len(ids)


def role_owned_ids(lines):
    """任何工具用 `entity_meta role` 认领的实体 id（遮断块 5001+、投影贴片 5101+ …）。"""
    return {
        line.split("\t")[1]
        for line in lines
        if len(line.split("\t")) == 4 and line.split("\t")[0] == "entity_meta"
        and line.split("\t")[2] == "role"
    }


def assert_id_band_free(lines):
    """保留段必须真的空着：别的工具若把 id 分到 5000 以上，两边会撞在同一个实体上。

    带 `entity_meta role` 的 id 不算占用——那是**别的就地工具**（投影贴片）认领的段，
    它们各自有自己的保留段与防撞检查。这条要防的是"自然分配的可玩实体爬进保留段"。
    """
    owned = role_owned_ids(lines)
    for line in lines:
        fields = line.split("\t")
        if len(fields) > 2 and fields[0] in ("entity", "component") \
                and fields[1].isdigit() and int(fields[1]) >= OCCLUDER_ID_BASE \
                and fields[1] not in owned:
            raise SystemExit(f"[FAIL] id >= {OCCLUDER_ID_BASE} 已被占用：{line}")


def plan(lines, lighting):
    """盘上现状 → (本体行改写表, 追加的遮断段)。幂等：原图与本体块都算"本体侧"。"""
    sprites, transforms, names = read_entities(lines)
    table = groups()
    alias = {}
    for name, group in table.items():
        alias[group["source"]] = (name, "base")
        alias[group["base"][0]] = (name, "base")
        alias[group["occluder"][0]] = (name, "occluder")
    order = int(lighting["foreground_order"]["occluder"])
    own = occluder_ids(lines)
    base_updates = {}
    placements = []
    for entity_id, payload in sprites.items():
        hit = alias.get(parse_fields(payload)["texture"])
        if not hit:
            continue
        name, side = hit
        if side == "occluder":
            if entity_id not in own:
                raise SystemExit(
                    f"[FAIL] 实体 {entity_id} 画着遮断块 {payload}，却没有 "
                    f"entity_meta role\t{ROLE}：本工具只认自己那一段，别的一半改的"
                    "遮断实体不会被幂等重写，会静默留在盘上")
            continue
        group = table[name]
        base_texture, base_height = group["base"]
        occ_texture, occ_height = group["occluder"]
        declared = parse_fields(payload)["size"].split("x")
        width = int(declared[0])
        x, y = transform_xy(transforms[entity_id])
        base_updates[entity_id] = swap_sprite(payload, base_texture, width, base_height)
        placements.append({
            "base_id": entity_id,
            "name": f"{names[entity_id]}{group['suffix']}",
            "x": x,
            "y": y + base_height,
            "texture": occ_texture,
            "width": width,
            "height": occ_height,
            "order": order,
        })
    placements.sort(key=lambda item: int(item["base_id"]))
    return base_updates, occluder_lines(placements)


def occluder_lines(placements):
    lines = []
    for index, item in enumerate(placements):
        entity_id = OCCLUDER_ID_BASE + index
        lines.append(f"entity\t{entity_id}\t{item['name']}")
        lines.append(dk.sprite_line(entity_id, item["texture"], item["width"],
                                    item["height"], item["order"], WHITE_TINT,
                                    SORTING_LAYER))
        lines.append(transform_line(entity_id, item["x"], item["y"]))
        lines.append(f"entity_meta\t{entity_id}\trole\t{ROLE}")
    return lines


def load_lines(scene):
    with open(scene, encoding="utf-8", newline="") as handle:
        content = handle.read()
    newline = "\r\n" if "\r\n" in content else "\n"
    return content.replace("\r\n", "\n").split("\n"), newline


def apply_patch(lines, base_updates, section):
    """把 `plan()` 的产物并进场景行：改写本体那几行，末尾追加遮断段。

    `--apply` 与 `gen_manor_scene.py`（从零生成）**共用这一个函数**：两条路必须产出
    同一个字节序列，否则重跑生成器会把盘上的遮断段抹掉——M72-04/M72-05 就是靠这条
    对拍立住的（生成器与就地工具各写一遍拼接逻辑，早晚会漂）。
    """
    merged = []
    for line in lines:
        fields = line.split("\t")
        want = base_updates.get(fields[1]) if len(fields) == 4 and fields[0] == "component" \
            and fields[2] == "Sprite" else None
        merged.append(f"{fields[0]}\t{fields[1]}\t{fields[2]}\t{want}" if want else line)
    merged.extend(section)
    return merged


def check(lines, base_updates, section, removed):
    problems = []
    for line in lines:
        fields = line.split("\t")
        if len(fields) != 4 or fields[0] != "component" or fields[2] != "Sprite":
            continue
        want = base_updates.get(fields[1])
        if want and fields[3] != want:
            problems.append(f"本体 {fields[1]} 还是整张原图：\n  盘上 {fields[3]}\n  应为 {want}")
    on_disk = occluder_section(lines)
    if on_disk != section:
        drift = [(a, b) for a, b in zip(on_disk, section) if a != b]
        detail = f"{len(drift)} 行不同，首条：\n  盘 {drift[0][0]}\n  算 {drift[0][1]}" if drift \
            else f"行数不同：盘上 {len(on_disk)} vs 重算 {len(section)}"
        problems.append(f"遮断段与重算不一致：{detail}")
    for problem in problems:
        print(f"[FAIL] {problem}")
    print(f"check {'FAIL' if problems else 'ok'}：遮断 {len(section) // 4} 件、"
          f"本体改写 {len(base_updates)} 行、旧的 {removed} 件已清、"
          f"id 保留段 {OCCLUDER_ID_BASE}+ 干净")
    return 1 if problems else 0


def main():
    parser = argparse.ArgumentParser(description="把前景遮断块写进 .scene（M72-06）")
    parser.add_argument("scene")
    parser.add_argument("--apply", action="store_true", help="写回文件（默认只报告）")
    parser.add_argument("--check", action="store_true",
                        help="只测不改：本体已归一、遮断段与重算逐字节相同、id 保留段干净")
    args = parser.parse_args()

    lighting = lk.load()
    lines, newline = load_lines(args.scene)
    stripped, removed = strip_occluders(lines)
    assert_id_band_free(stripped)
    base_updates, section = plan(stripped, lighting)

    if args.check:
        return check(lines, base_updates, section, removed)

    merged = apply_patch(stripped, base_updates, section)
    count = len(section) // 4
    print(f"{args.scene}: 遮断 {count} 件（本体改写 {len(base_updates)} 行、清掉旧的 {removed} 件）"
          f" id {OCCLUDER_ID_BASE}..{OCCLUDER_ID_BASE + count - 1}"
          f" 层={SORTING_LAYER} order={lighting['foreground_order']['occluder']}")
    if args.apply:
        with open(args.scene, "w", encoding="utf-8", newline="") as handle:
            handle.write(newline.join(merged))
        print("  applied")
    else:
        print("  dry-run（加 --apply 才写盘）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
