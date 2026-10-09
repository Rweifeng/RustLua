# -*- coding: utf-8 -*-
"""把建筑/树/道具的**地面投影贴片**写进 `.scene`，并生成那些贴片 PNG（M72-06 C 段）。

用法（工程根目录 `examples/game_jhyt` 下）：

    python tools/set_cast_shadows.py scenes/manor.scene            # 只报告
    python tools/set_cast_shadows.py scenes/manor.scene --apply    # 写盘（PNG + 场景）
    python tools/set_cast_shadows.py scenes/manor.scene --check    # 盘上对拍（PNG 与场景都测）

一句话：**影子的方向只有一份，就是 `data/lighting.lua` 的 `sun`**。这个文件不写任何一个
`(dx, dy)`——偏移取 `light_kit.shadow_offset(物体总高)`，透明度取 `shadow_tint.a`，
挂载档与体量门槛取 `cast_shadow`。写死就有第二套真相，而漂移的症状是"这棵树往右下倒、
那棵树往左下倒"（docs/53 §3.9），不会报错。

贴片是什么形状：**接触椭圆沿光向扫出的那一段**（Minkowski 和），于是

    贴片中心 = 接触点 + 偏移/2、  rx = 半跨 + |dx|/2、  ry = 半跨×flatten + |dy|/2

尺寸写成 `2r+1`（**恒为奇数**）就是为了让"贴片中心"有第二个意思：正中间那个 texel，
落在 `(x+rx, y+ry)`，整数、没有 0.5。于是角度这条是**恒等式而不是近似**：`sun` 是 (1,-1)
时 |dx| 与 |dy| 等量、取整也等量 ⇒ 中心相对接触点的向量是 `(h,-h)`，`atan2` 出来精确等于
光向角。验收门（`jhyt_m72_foreground_occlusion` §⑥）按同一条式子独立复算 16 件，方差必须 0。
而"接触点"是从**原图的落地行**量出来的，不是估的——见 `contact_of`。

像素契约（docs/57 §1.3）：贴片 PNG 一律 RGBA8、alpha 只有 0/255、颜色是**纯白**，
暗与透全交给 sprite tint 的 rgba（顶点色 × 贴图色）。于是"影子多暗"这件事在盘上只有
lighting.lua 一处，改 `shadow_tint.a` 三个宿主同时生效，而贴片自己永远不需要重出图。

**id 段**：`5101` 起的保留段，与 `set_foreground_occluders.py` 的 5001 同一条理由——
不保留就会让 `set_scatter_decor.py` 的 175 件装饰整体改号，它的逐字节对拍会红在跟装饰
无关的改动上。`set_scatter_decor.reserved_ids` 里必须登记 `cast_shadow` 这个 role。

依赖：Python 3 标准库 + 同目录 `pixel_kit.py`、`decor_kit.py`、`gen_manor_art.py`、
`light_kit.py`、`set_foreground_occluders.py`。
"""

import argparse
import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import decor_kit as dk  # noqa: E402  sprite_line：字段顺序与引擎 `Sprite::to_component_str` 一致
import light_kit as lk  # noqa: E402
import set_foreground_occluders as fo  # noqa: E402  LAYER_SPLITS 的拆层路径（不写第二份）
from gen_manor_art import fill_ellipse  # noqa: E402  椭圆填充（与角色/道具同一实现）
from pixel_kit import Canvas, load_canvas, transform_line  # noqa: E402

ROLE = "cast_shadow"
SORTING_LAYER = "world"
SHADOW_DIR = "assets/art/shadows"
# 保留 id 段：见模块 docstring「id 段」。
SHADOW_ID_BASE = 5101
# 贴片只挂在"有落地体量"的图上：这两目录之外是地表/内饰/UI，它们不站在地面上。
GROUND_DIRS = ("assets/art/buildings", "assets/art/props")
# 拆层件里只有**下半块**站在地面上；上半块（树冠/屋顶）在空中，不配自己的投影。
WHITE = (255, 255, 255, 255)


def posix(path):
    return path.replace("\\", "/")


def half_up(value):
    """**四舍五入（.5 一律进位）**，不是内建的 `round()`。

    Python 3 的 `round()` 是**银行家舍入**（`round(8.5)==8`、`round(522.5)==522`），而验收门
    那一侧是 Rust 的 `f64::round()`（`.round()` 对 8.5 给 9）。贴片落点里有三处会正好踩在
    `.5` 上（半偏移、接触行中心 `(x0+x1)/2`、半跨×flatten），两边各用各的就会让门红在
    "1 像素"上——而那看起来像"光向算错了"。所以这里显式写进位，门抄同一条式子。
    """
    return int(math.floor(value + 0.5))


def split_tables():
    """`{本体贴图: 拆层名}`、`{遮断块贴图}`、`{拆层名: 整图源图}`——全部转成 posix。"""
    bases, tops, sources = {}, set(), {}
    for name, group in fo.groups().items():
        bases[group["base"][0]] = name
        tops.add(group["occluder"][0])
        sources[name] = group["source"]
    return bases, tops, sources


def contact_of(canvas):
    """落地行 = 最底不透明行；落地宽 = 底部 25% 行里的**最大跨度**。

    为什么要分开两件事：正房本体图的最底一行是**台阶**（跨度 45），而它的体量是台基
    （跨度 200）——只按最底行走，正房的投影会缩成台阶底下一小坨。反过来，树那种
    "上宽下窄"的剪影（树干 21 / 图宽 64）不能往上找"够宽的行"：那里已经是树冠切面，
    拿它当落地宽就等于把树冠印在地上。所以**位置看最底行、宽度看底部一小段的最宽处**。
    """
    bottom = None
    rows = []
    for y in range(canvas.h - 1, -1, -1):
        xs = [x for x in range(canvas.w) if canvas.opaque(x, y)]
        if not xs:
            continue
        if bottom is None:
            bottom = (y, xs[0], xs[-1])
        rows.append((y, xs[0], xs[-1]))
        if len(rows) >= max(2, canvas.h // 4):
            break
    if bottom is None:
        raise SystemExit(f"[FAIL] {canvas.w}x{canvas.h} 的图整张全透明，没有落地行可取")
    widest = max(rows, key=lambda item: item[2] - item[1])
    return bottom[0], bottom[1], bottom[2], widest[2] - widest[1] + 1


def patch_canvas(rx, ry):
    """白底椭圆贴片：alpha 只有 0/255，暗与透由 sprite tint 决定（见模块 docstring）。"""
    width, height = 2 * rx + 1, 2 * ry + 1
    canvas = Canvas(width, height)
    fill_ellipse(canvas, rx, ry, rx, ry, WHITE)
    return canvas


def placements(lines, src, art):
    """盘上实体 → 贴片清单（按本体 id 排序，决定 id 段的分配顺序）。"""
    bases, tops, sources = split_tables()
    sprites, transforms, names = fo.read_entities(lines)
    own = shadow_ids(lines)
    order = int(src["cast_shadow"]["order"])
    min_reach = float(src["cast_shadow"]["min_reach_units"])
    flatten = float(src["cast_shadow"]["contact_flatten"])
    out = []
    for entity_id, payload in sprites.items():
        if entity_id in own:
            continue
        fields = fo.parse_fields(payload)
        texture = fields["texture"]
        if texture in tops:
            continue
        if posix(texture).rsplit("/", 1)[0] not in GROUND_DIRS:
            continue
        name = bases.get(texture)
        canvas = art[texture]
        declared = fields["size"].split("x")
        if (int(declared[0]), int(declared[1])) != (canvas.w, canvas.h):
            raise SystemExit(
                f"[FAIL] 实体 {entity_id} 声明 size={fields['size']}，贴图 {texture} 实为 "
                f"{canvas.w}x{canvas.h}：贴片要按物体总高算偏移，尺寸对不上就是两套真相")
        if name:
            total = art[sources[name]].h
        else:
            total = canvas.h
        dx, dy = lk.shadow_offset(total, src)
        if math.hypot(dx, dy) < min_reach:
            continue
        row, x0, x1, span = contact_of(canvas)
        x, y = fo.transform_xy(transforms[entity_id])
        half_span = span // 2
        # 偏移的一半取整：`sun` 是 (1,-1) 时 dx/dy 等量，取整也等量 ⇒ 角度**精确**等于光向。
        hx, hy = half_up(abs(dx) / 2), half_up(abs(dy) / 2)
        rx, ry = half_span + hx, half_up(half_span * flatten) + hy
        out.append({
            "base_id": entity_id,
            "name": f"{names[entity_id]}Shadow",
            "rx": rx,
            "ry": ry,
            # 接触点取整到整数格，贴片中心 = 接触点 + 取整后的半偏移 ⇒ 全整数、无 0.5 抖动。
            # 取整一律走 `half_up`（两边同式，见它的 docstring）。
            "x": half_up(x + (x0 + x1) / 2) + (hx if dx >= 0 else -hx) - rx,
            "y": half_up(y + (canvas.h - 1 - row)) - (hy if dy <= 0 else -hy) - ry,
            "order": order,
            "object_height": total,
            "reach": math.hypot(dx, dy),
            "span": span,
        })
    out.sort(key=lambda item: int(item["base_id"]))
    return out


def shadow_texture(rx, ry):
    """贴片按**尺寸**命名：同尺寸即同一张图（四棵树共用一张）。贴图条目是预算项，
    一件一张会白涨十几份，而两片椭圆只要尺寸相同就逐字节相同。"""
    return f"{SHADOW_DIR}/blob_{2 * rx + 1}x{2 * ry + 1}.png"


def shadow_lines(items):
    lines = []
    for index, item in enumerate(items):
        entity_id = SHADOW_ID_BASE + index
        lines.append(f"entity\t{entity_id}\t{item['name']}")
        lines.append(dk.sprite_line(entity_id, item["texture"], item["width"], item["height"],
                                    item["order"], item["tint"], SORTING_LAYER))
        lines.append(transform_line(entity_id, item["x"], item["y"]))
        lines.append(f"entity_meta\t{entity_id}\trole\t{ROLE}")
    return lines


def shadow_ids(lines):
    return {
        line.split("\t")[1]
        for line in lines
        if len(line.split("\t")) == 4 and line.split("\t")[0] == "entity_meta"
        and line.split("\t")[2] == "role" and line.split("\t")[3] == ROLE
    }


def strip_shadows(lines):
    ids = shadow_ids(lines)
    kept = [line for line in lines
            if not (len(line.split("\t")) > 1
                    and line.split("\t")[0] in ("entity", "component", "entity_meta")
                    and line.split("\t")[1] in ids)]
    return kept, len(ids)


def shadow_section(lines):
    ids = shadow_ids(lines)
    return [line for line in lines
            if len(line.split("\t")) > 1 and line.split("\t")[0] in ("entity", "component", "entity_meta")
            and line.split("\t")[1] in ids]


def assert_id_band_free(lines):
    """5101 以上只许是本工具的实体（5001..5100 归遮断块）：撞号 = 两边互相覆盖。"""
    for line in lines:
        fields = line.split("\t")
        if len(fields) > 2 and fields[0] in ("entity", "component") \
                and fields[1].isdigit() and int(fields[1]) >= SHADOW_ID_BASE:
            raise SystemExit(f"[FAIL] id >= {SHADOW_ID_BASE} 已被占用：{line}")


def build(lines, src, art):
    """placements → 带上贴图路径/尺寸/tint 的成品清单 + `{贴片路径: 画布}`。"""
    items = placements(lines, src, art)
    tint = "{:.3f},{:.3f},{:.3f},{:.3f}".format(
        float(src["shadow_tint"]["r"]), float(src["shadow_tint"]["g"]),
        float(src["shadow_tint"]["b"]), float(src["shadow_tint"]["a"]))
    canvases = {}
    for item in items:
        canvas = patch_canvas(item["rx"], item["ry"])
        path = posix(shadow_texture(item["rx"], item["ry"]))
        canvases[path] = canvas
        item["texture"] = path
        item["width"], item["height"] = canvas.w, canvas.h
        item["tint"] = tint
    return items, canvases


def disk_path(texture, root):
    return os.path.join(root, texture.replace("/", os.sep))


def apply_patch(lines, section):
    """与 `gen_manor_scene.py`（从零生成）共用：两条路必须产出同一个字节序列。"""
    return list(lines) + section


def write_art(canvases, root):
    """写贴片 PNG。**内容相同就不重写**：贴片是确定性的，重写会把没变的文件刷成
    modified，review 时就看不出这次到底改了什么。"""
    written, untouched = [], []
    for texture, canvas in sorted(canvases.items()):
        path = disk_path(texture, root)
        blob = canvas.to_png()
        if os.path.exists(path):
            with open(path, "rb") as handle:
                if handle.read() == blob:
                    untouched.append(texture)
                    continue
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as handle:
            handle.write(blob)
        written.append(texture)
    return written, untouched


def check_art(canvases, root):
    problems = []
    for texture, canvas in sorted(canvases.items()):
        path = disk_path(texture, root)
        if not os.path.exists(path):
            problems.append(f"贴片缺失：{texture}（先跑一次 --apply）")
            continue
        with open(path, "rb") as handle:
            disk = handle.read()
        want = canvas.to_png()
        if disk != want:
            problems.append(f"{texture} 与重算不同（盘上 {len(disk)} 字节 vs 重算 "
                            f"{len(want)} 字节）：贴片是 lighting.lua 的读数，手改盘上必被下次 --apply 抹掉")
            continue
        on_disk = load_canvas(path)
        alphas = {on_disk.get(x, y)[3] for y in range(on_disk.h) for x in range(on_disk.w)}
        if not alphas <= {0, 255}:
            problems.append(f"{texture} 有半透明 alpha {sorted(alphas - {0, 255})}"
                            "：软边门（docs/57 §1.3）只允许 tint 一处带透明度")
        whites = {on_disk.get(x, y)[:3] for y in range(on_disk.h) for x in range(on_disk.w)
                  if on_disk.get(x, y)[3] == 255}
        if whites != {WHITE[:3]}:
            problems.append(f"{texture} 的实体像素不是纯白：{sorted(whites)}"
                            "——颜色由 shadow_tint 决定，烘进 PNG 就有了第二份暗度")
    return problems


def check(lines, items, section, canvases, root, removed):
    problems = check_art(canvases, root)
    on_disk = shadow_section(lines)
    if on_disk != section:
        drift = [(a, b) for a, b in zip(on_disk, section) if a != b]
        detail = f"{len(drift)} 行不同，首条：\n  盘 {drift[0][0]}\n  算 {drift[0][1]}" if drift \
            else f"行数不同：盘上 {len(on_disk)} vs 重算 {len(section)}"
        problems.append(f"投影段与重算不一致：{detail}")
    for problem in problems:
        print(f"[FAIL] {problem}")
    print(f"check {'FAIL' if problems else 'ok'}：投影 {len(items)} 件 / 贴片 "
          f"{len(canvases)} 张、旧的 {removed} 件已清、id 保留段 {SHADOW_ID_BASE}+ 干净")
    return 1 if problems else 0


def report(items):
    for item in items:
        print(f"  {item['name']:26s} 本体={item['base_id']:>4s} 总高={item['object_height']:3d} "
              f"reach={item['reach']:5.2f} 落地跨={item['span']:3d} "
              f"贴片={item['width']}x{item['height']} @({item['x']},{item['y']}) "
              f"{item['texture']}")


def load_art(lines):
    """盘上场景引用到的 buildings/props 贴图各读一次（缓存：一棵树要问四遍）。"""
    sprites, _, _ = fo.read_entities(lines)
    art = {}
    for payload in sprites.values():
        texture = posix(fo.parse_fields(payload)["texture"])
        if texture in art or texture.startswith(SHADOW_DIR):
            continue
        if texture.rsplit("/", 1)[0] not in GROUND_DIRS:
            continue
        art[texture] = load_canvas(texture)
    for name, group in fo.groups().items():
        if group["source"] not in art:
            art[group["source"]] = load_canvas(group["source"])
    return art


def main():
    parser = argparse.ArgumentParser(description="把地面投影贴片写进 .scene（M72-06 C 段）")
    parser.add_argument("scene")
    parser.add_argument("--root", default=".", help="工程根目录（贴图路径相对它）")
    parser.add_argument("--apply", action="store_true", help="写回文件与贴片（默认只报告）")
    parser.add_argument("--check", action="store_true",
                        help="只测不改：贴片 PNG 与场景段各自和重算逐字节相同、id 保留段干净")
    args = parser.parse_args()

    src = lk.load()
    lines, newline = fo.load_lines(args.scene)
    stripped, removed = strip_shadows(lines)
    assert_id_band_free(stripped)
    art = load_art(stripped)
    items, canvases = build(stripped, src, art)
    section = shadow_lines(items)

    if args.check:
        return check(lines, items, section, canvases, args.root, removed)

    report(items)
    merged = apply_patch(stripped, section)
    if args.apply:
        written, untouched = write_art(canvases, args.root)
        with open(args.scene, "w", encoding="utf-8", newline="") as handle:
            handle.write(newline.join(merged))
        print(f"  applied：贴片 {len(items)} 件（新写 {len(written)} 张 {written}、"
              f"未变 {len(untouched)} 张）、清掉旧的 {removed} 件"
              f"  id {SHADOW_ID_BASE}..{SHADOW_ID_BASE + len(items) - 1}")
    else:
        print("  dry-run（加 --apply 才写盘）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
