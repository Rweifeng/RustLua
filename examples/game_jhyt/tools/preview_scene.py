# -*- coding: utf-8 -*-
"""把 .scene 合成成一张 PNG 预览（地编验收用，不需要开窗口/GPU）。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/preview_scene.py scenes/manor.scene
    python tools/preview_scene.py scenes/manor.scene --out .temp/main.png --scale 1 --grid

渲染规则（严格照引擎实现，用来发现"数据和美术对不上"的问题）：

  * Tilemap 有两种编码，都认：
      - `tiles_format=0`（旧）：一格一个 ASCII 数字，图集固定 6 列 1 行、V 方向整幅拉伸；
      - `tiles_format=1`（M72-01）：逗号分隔十进制图集索引，列数/行数由
        `atlas_cols` / `atlas_rows` 显式声明（引擎不从贴图尺寸反推，这里也不反推）。
    cell (cx,cy) 覆盖世界 x∈[cx*32,(cx+1)*32]、y∈[cy*32,(cy+1)*32]，row 0 在最底；
    屏幕 y 向下，所以屏幕行 = (height-1-cy)。图集条目 n 取第
    `col=(n-1)%cols`、`row=(n-1)//cols` 个子矩形（行 0 在**贴图顶部**，与地图行 0 在底部
    是两回事——这正是引擎 autotile 位序按 index 命名而不是按方位命名的原因）。
  * 逻辑瓦模式（M72-02，`autotile=1` 或 `variant_count>1`）：格子里存的是 kind id，
    渲染时由「邻居掩码 + 确定性 FNV-1a 哈希（再做 splitmix64 雪崩收尾再取模）」解析成图集条目。
    这段解算与 `engine/crates/rustlua-ecs/src/tilemap.rs` 的 `resolve_atlas_index` **逐位一致**，
    改任何一边都必须同步另一边（另两份镜像在 `plugins/webgl/web-runtime.js` 的 JS 与 Lua 垫片里），
    否则预览图与实机不一致（本文件的存在意义就是发现这种不一致）。
  * Sprite：pivot=(0,0) 时 transform 即左下角；图像顶行对应 quad 顶部。
    `tint=r,g,b,a` 与引擎同样按**乘法 + src-over** 作用在贴图像素上（见 `blit_region`），
    纯白贴片（M72-06 的地面投影）靠它才看得见暗与透。`__white__` 内建贴图按 tint 画成
    半透明占位块，便于看玩家/NPC 站位。
  * per-tile tint（M72-02）：`tints=<cellIndex>:<tintId>,...`（稀疏，只记非白格），
    `tints_rgb=r,g,b,a|r,g,b,a|...` 是调色表；顶点色是**乘法**染色，这里用
    `tint_rect` 逐像素相乘复刻同一个语义（不做半透明叠加近似）。
  * 排序：tilemap 先画，sprite 后画；sprite 之间按 sorting_layer 档位 ×1000 + order 排序。

依赖：Python 3 标准库 + 同目录 pixel_kit.py。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import Canvas, load_canvas, rgb, shade  # noqa: E402

LAYER_RANK = {
    "background": -2,
    "world": -1,
    "default": 0,
    "actor": 1,
    "foreground": 2,
    "ui": 3,
}

TILE = 32
# 与 Rust 侧同名常量必须一致（`tilemap.rs` 的 AUTOTILE_SLOTS / FNV 参数 / 两个盐）。
AUTOTILE_SLOTS = 16
VARIANT_HASH_OFFSET = 0xcbf29ce484222325
VARIANT_HASH_PRIME = 0x00000100000001B3
MIX_AXIS_X = 0x9E3779B97F4A7C15
MIX_AXIS_Y = 0xBF58476D1CE4E5B9
U64_MASK = (1 << 64) - 1
# avalanche（splitmix64 收尾混洗）的两个乘子；0xBF58…与 MIX_AXIS_Y 同值是 splitmix 的巧合。
AVALANCHE_MUL_A = 0xBF58476D1CE4E5B9
AVALANCHE_MUL_B = 0x94D049BB133111EB
# 旧编码的"没有图集信息"时的默认布局（DEFAULT_ATLAS_COLS/ROWS）。
DEFAULT_ATLAS_COLS = 6
DEFAULT_ATLAS_ROWS = 1
LEGACY_KIND_LIMIT = 6


def parse_fields(value):
    fields = {}
    for part in value.split(";"):
        if not part:
            continue
        if "=" not in part:
            fields[part] = ""
            continue
        key, raw = part.split("=", 1)
        fields[key] = raw
    return fields


def parse_scene(path):
    """返回 (header, entities)；entities = [ {id,name,components,meta} ]，保持文件顺序。"""
    header = {}
    entities = []
    by_id = {}
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            line = line.rstrip("\n")
            if not line:
                continue
            fields = line.split("\t")
            kind = fields[0]
            if kind == "entity":
                entity = {"id": fields[1], "name": fields[2] if len(fields) > 2 else "",
                          "components": {}, "meta": {}}
                entities.append(entity)
                by_id[entity["id"]] = entity
            elif kind == "component":
                entity = by_id.get(fields[1])
                if entity is not None:
                    entity["components"][fields[2]] = fields[3] if len(fields) > 3 else ""
            elif kind == "entity_meta":
                entity = by_id.get(fields[1])
                if entity is not None:
                    entity["meta"][fields[2]] = fields[3] if len(fields) > 3 else ""
            elif kind == "meta":
                header[fields[1]] = fields[2] if len(fields) > 2 else ""
            elif "=" in line and "\t" not in line:
                key, raw = line.split("=", 1)
                header[key] = raw
    return header, entities


def blit_region(dst, src, sx, sy, sw, sh, dx, dy, dw, dh, tint=None):
    """把 src 的 (sx,sy,sw,sh) 区域按最近邻缩放到 (dx,dy,dw,dh)。

    `tint` = sprite 的顶点色（r,g,b,a 各 0..1）：引擎语义是**贴图色 × 顶点色**再 src-over
    混合。不传就是直接覆盖（既有预览逐字节不变）。少了这一步，投影贴片在预览里会画成一
    块白——它的 PNG 是**纯白**的，暗与透全在 `shadow_tint` 里（docs/57 §1.3 的软边门），
    而"影子朝右下、没盖住角色"这类只能靠眼睛验收的判断，正是要在验收图里看的东西。
    """
    for oy in range(dh):
        ysrc = sy + min(sh - 1, int(oy * sh / dh))
        for ox in range(dw):
            xsrc = sx + min(sw - 1, int(ox * sw / dw))
            color = src.get(xsrc, ysrc)
            if tint is None:
                if color[3] > 0:
                    dst.set(dx + ox, dy + oy, color)
                continue
            alpha = color[3] * tint[3]
            if alpha <= 0:
                continue
            # 夹到 255：装饰的 tint 有 1.030 这种**大于 1** 的档（顶点色可以提亮），
            # 而 `Canvas.set` 写的是 bytes —— 越界直接 ValueError。引擎侧是 u8 饱和转换。
            shaded = tuple(min(255, int(color[i] * tint[i])) for i in range(3))
            if alpha >= 255:
                dst.set(dx + ox, dy + oy, shaded + (255,))
                continue
            below = dst.get(dx + ox, dy + oy)
            f = alpha / 255.0
            mixed = tuple(int(shaded[i] * f + below[i] * (1.0 - f)) for i in range(3))
            dst.set(dx + ox, dy + oy, mixed + (255,))


class TextureCache:
    def __init__(self, root):
        self.root = root
        self.cache = {}

    def get(self, path):
        if path in self.cache:
            return self.cache[path]
        canvas = None
        if path != "__white__":
            full = os.path.join(self.root, path.replace("/", os.sep))
            if os.path.exists(full):
                canvas = load_canvas(full)
        self.cache[path] = canvas
        return canvas


def _mix_u64(state, value):
    """一次 FNV-1a 混合（把 value 按**小端 8 字节**逐字节喂进去）。"""
    mixed = state
    for index in range(8):
        byte = (value >> (8 * index)) & 0xFF
        mixed = ((mixed ^ byte) * VARIANT_HASH_PRIME) & U64_MASK
    return mixed


def _avalanche(value):
    """splitmix64 的雪崩收尾：原始 FNV 的低比特周期等于模数，直接取模会变体排成点阵。"""
    stage = (value ^ (value >> 30)) & U64_MASK
    stage = (stage * AVALANCHE_MUL_A) & U64_MASK
    stage = (stage ^ (stage >> 27)) & U64_MASK
    stage = (stage * AVALANCHE_MUL_B) & U64_MASK
    return (stage ^ (stage >> 31)) & U64_MASK


def variant_for(cells, width, height, x, y, seed, variant_count):
    """确定性变体序号：与 Rust `Tilemap::variant_for` 同一份哈希、盐顺序与收尾混洗。"""
    if variant_count < 2:
        return 0
    hash_state = VARIANT_HASH_OFFSET
    for value in (x, y, seed, MIX_AXIS_X, MIX_AXIS_Y):
        hash_state = _mix_u64(hash_state, value)
    return _avalanche(hash_state) % variant_count


def autotile_mask(cells, width, height, x, y):
    """4-bit 邻居掩码：位 0 = y-1、位 1 = x+1、位 2 = y+1、位 3 = x-1。

    越界当作"没有邻居"（地图边就是暴露的边界，不是内部瓦），与 Rust 侧一致。
    """
    own = cells[y * width + x]
    mask = 0
    for bit, (dx, dy) in enumerate(((0, -1), (1, 0), (0, 1), (-1, 0))):
        nx, ny = x + dx, y + dy
        if nx < 0 or ny < 0 or nx >= width or ny >= height:
            continue
        neighbor = cells[ny * width + nx]
        if neighbor != 0 and neighbor == own:
            mask |= 1 << bit
    return mask


def resolve_atlas_index(layout, cells, x, y):
    """格子值 → 图集条目号（0 = 什么都不画）。逐分支对照 Rust 的 resolve_atlas_index。"""
    cell = cells[y * layout["width"] + x]
    if cell == 0:
        return 0
    autotile = layout["autotile"]
    variants = layout["variant_count"]
    if not autotile and variants <= 1:
        return cell
    kind = cell
    if kind > layout["tile_kinds"]:
        return 0
    span = max(1, variants)
    base_span = layout["tile_kinds"] * span
    capacity = layout["atlas_cols"] * layout["atlas_rows"]
    if autotile:
        index = 1 + base_span + (kind - 1) * AUTOTILE_SLOTS + \
            autotile_mask(cells, layout["width"], layout["height"], x, y) % AUTOTILE_SLOTS
    else:
        index = 1 + (kind - 1) * span + variant_for(
            cells, layout["width"], layout["height"],
            x, y, layout["variant_seed"], span)
    return index if index <= capacity else 0


def tile_layout(fields, errors, name):
    """把 Tilemap 组件串里的布局信息读成一份渲染用的 layout dict（含两种编码的差异）。"""
    width = int(fields["width"])
    height = int(fields["height"])
    fmt = int(fields.get("tiles_format", "0") or 0)
    raw = fields.get("tiles", "")
    if fmt == 0:
        if not all(ch.isdigit() for ch in raw):
            errors.append(f"{name}: 旧编码 tiles 含非数字字符 {raw[:12]!r}")
        cells = [int(ch) for ch in raw]
        cols = int(fields.get("atlas_cols", DEFAULT_ATLAS_COLS) or DEFAULT_ATLAS_COLS)
        rows = int(fields.get("atlas_rows", DEFAULT_ATLAS_ROWS) or DEFAULT_ATLAS_ROWS)
    elif fmt == 1:
        cells = [int(part) for part in raw.split(",") if part.strip() != ""]
        cols = int(fields["atlas_cols"])
        rows = int(fields["atlas_rows"])
    else:
        errors.append(f"{name}: 不支持的 tiles_format={fmt}（引擎只认 0/1）")
        cells, cols, rows = [], DEFAULT_ATLAS_COLS, DEFAULT_ATLAS_ROWS
    kinds = int(fields.get("tile_kinds", "0") or 0) or LEGACY_KIND_LIMIT
    variants = int(fields.get("variant_count", "1") or 1)
    autotile = fields.get("autotile", "") in ("1", "true")
    layout = {
        "width": width,
        "height": height,
        "format": fmt,
        "cells": cells,
        "atlas_cols": cols,
        "atlas_rows": rows,
        "tile_kinds": kinds,
        "variant_count": variants,
        "autotile": autotile,
        "variant_seed": int(fields.get("variant_seed", "0") or 0),
        # 逻辑瓦模式下格子的上界是 kind 数，直接寻址模式下是图集条目数。
        "max_id": kinds if (autotile or variants > 1) else cols * rows,
        "capacity": cols * rows,
    }
    tints = [0] * (width * height)
    for entry in fields.get("tints", "").split(","):
        if not entry:
            continue
        index, tint_id = entry.split(":")
        tints[int(index)] = int(tint_id)
    layout["tints"] = tints
    table = []
    for entry in fields.get("tints_rgb", "").split("|"):
        if not entry:
            continue
        parts = [float(p) for p in entry.split(",")]
        if len(parts) != 4:
            errors.append(f"{name}: tints_rgb 条目必须是 r,g,b,a：{entry!r}")
            continue
        table.append(parts)
    layout["tint_table"] = table
    return layout


def tint_color(layout, index):
    tint_id = layout["tints"][index] if index < len(layout["tints"]) else 0
    if tint_id == 0:
        return None
    if tint_id > len(layout["tint_table"]):
        return None
    return layout["tint_table"][tint_id - 1]


def tint_rect(canvas, x, y, w, h, tint):
    """逐像素乘法染色：与引擎「顶点色 × 贴图色」一致，不是半透明叠加。

    `pixel_kit.Canvas.rect` 是裸覆盖（15 个生成器脚本依赖这个语义），所以染色
    在这个文件里自己做，不去改公共 rect。
    """
    r, g, b, a = tint
    for dy in range(h):
        for dx in range(w):
            color = canvas.get(x + dx, y + dy)
            if color[3] == 0:
                continue
            canvas.set(x + dx, y + dy, (
                int(color[0] * r), int(color[1] * g),
                int(color[2] * b), max(0, min(255, int(color[3] * a))),
            ))


def render(scene_path, root, scale=1, grid=False, only_layers=None):
    header, entities = parse_scene(scene_path)
    tilemaps = [e for e in entities if "Tilemap" in e["components"]]
    if not tilemaps:
        # 纯 UI 场景（如主入口门面 main.scene）：没有地编可预览，属于"跳过"而不是"失败"，
        # 因此返回 None 由调用方打印 skip（退出码 0），避免验证脚本把它当校验失败。
        print(f"skip {scene_path}：没有 Tilemap 实体（纯 UI 场景，不需要地编预览）")
        return None, header, [], []
    first = parse_fields(tilemaps[0]["components"]["Tilemap"])
    width = int(first["width"])
    height = int(first["height"])
    tile_w = int(float(first["tile_size"].split("x")[0]))
    tile_h = int(float(first["tile_size"].split("x")[1]))
    map_w = width * tile_w
    map_h = height * tile_h

    canvas = Canvas(map_w, map_h, fill=rgb("#101418"))
    textures = TextureCache(root)
    notes = []
    errors = []

    # 1) Tilemaps（文件顺序）
    for entity in tilemaps:
        fields = parse_fields(entity["components"]["Tilemap"])
        name = entity["name"]
        texture_path = fields.get("texture", "")
        layout = tile_layout(fields, errors, name)
        tw, th = layout["width"], layout["height"]
        cells = layout["cells"]
        solid = fields.get("solid", "")
        if len(cells) != tw * th:
            errors.append(f"{name}: tiles 数量 {len(cells)} != {tw}x{th}")
            continue
        if len(solid) != tw * th:
            errors.append(f"{name}: solid 长度 {len(solid)} != {tw}x{th}")
        for index, value in enumerate(cells):
            if value > layout["max_id"]:
                errors.append(
                    f"{name}: 格 {index} 的 tile 值 {value} 越界"
                    f"（{'逻辑瓦 kind' if layout['autotile'] or layout['variant_count'] > 1 else '图集条目'}"
                    f"上界 {layout['max_id']}）"
                )
                break
            if value != 0 and resolve_atlas_index(layout, cells, index % tw, index // tw) == 0:
                errors.append(
                    f"{name}: 格 {index} 解析不到图集条目（atlas {layout['atlas_cols']}x"
                    f"{layout['atlas_rows']} 装不下这套布局）"
                )
                break
        texture = textures.get(texture_path)
        if texture is None:
            errors.append(f"{name}: 贴图缺失 {texture_path}")
            continue
        if texture.w != layout["atlas_cols"] * tile_w:
            errors.append(
                f"{name}: tileset 宽 {texture.w} != atlas_cols({layout['atlas_cols']})*{tile_w}"
                "（列数由组件串显式声明，与贴图对不上就是错位）"
            )
        if texture.h != layout["atlas_rows"] * tile_h:
            if layout["format"] == 0 and layout["atlas_rows"] == 1:
                notes.append(
                    f"[note] {name}: 旧编码 tileset 高 {texture.h} != {tile_h}，"
                    "按引擎规则整幅拉伸到格子高度"
                )
            else:
                errors.append(
                    f"{name}: tileset 高 {texture.h} != atlas_rows({layout['atlas_rows']})*{tile_h}"
                    "（新编码不再拉伸 V，行数必须与贴图行数一致）"
                )
                continue
        sub_w = texture.w // layout["atlas_cols"]
        sub_h = max(1, texture.h // layout["atlas_rows"]) if layout["atlas_rows"] else texture.h
        drawn = 0
        for cy in range(th):
            for cx in range(tw):
                entry = resolve_atlas_index(layout, cells, cx, cy)
                if entry <= 0:
                    continue
                drawn += 1
                position = entry - 1
                col = position % layout["atlas_cols"]
                row = position // layout["atlas_cols"]
                dst_x = cx * tile_w
                dst_y = (height - 1 - cy) * tile_h
                if layout["format"] == 0 and layout["atlas_rows"] == 1:
                    # 旧引擎的 tile_uv 只按 6 列切 U，V 恒为整幅 → 复刻"拉伸"行为。
                    blit_region(canvas, texture, col * tile_w, 0, tile_w, texture.h,
                                dst_x, dst_y, tile_w, tile_h)
                else:
                    blit_region(canvas, texture, col * sub_w, row * sub_h, sub_w, sub_h,
                                dst_x, dst_y, tile_w, tile_h)
                tint = tint_color(layout, cy * tw + cx)
                if tint is not None:
                    tint_rect(canvas, dst_x, dst_y, tile_w, tile_h, tint)
        notes.append(
            f"{name}: tilemap {tw}x{th} fmt={layout['format']} "
            f"atlas={layout['atlas_cols']}x{layout['atlas_rows']}"
            f"{' autotile' if layout['autotile'] else ''}"
            f"{' variants=' + str(layout['variant_count']) if layout['variant_count'] > 1 else ''}"
            f" 贴图 {texture_path} 画出 {drawn} 格"
        )

    # 2) Sprites（按 layer/order 排序，稳定）
    sprites = []
    for index, entity in enumerate(entities):
        if "Sprite" not in entity["components"] or "Transform" not in entity["components"]:
            continue
        fields = parse_fields(entity["components"]["Sprite"])
        transform = parse_fields(entity["components"]["Transform"])
        # `.scene` 的 Transform 是规范串 `position=x,y,z;rotation=...;scale=...`（9 位小数，
        # 见 pixel_kit.transform_line 与引擎的 to_component_str），**没有** `x=`/`y=` 两个字段。
        # 早先这里读 `transform.get("x")` 恒取默认 0，于是所有 sprite 都画在 (0,0)——
        # 症状是"道具全堆在地图左下角"，而地表照常，所以只看过地表的预览不会发现。
        px, py = (transform.get("position", "0,0,0").split(",") + ["0", "0"])[:2]
        layer = fields.get("sorting_layer", "default")
        order = int(float(fields.get("order", "0") or 0))
        sprites.append({
            "entity": entity,
            "fields": fields,
            "x": float(px),
            "y": float(py),
            "layer": layer,
            "order": order,
            "index": index,
            "key": (LAYER_RANK.get(layer.strip().lower(), 0) * 1000 + order, index),
        })
    sprites.sort(key=lambda item: item["key"])

    if only_layers:
        sprites = [s for s in sprites if s["layer"] in only_layers]

    for sprite in sprites:
        fields = sprite["fields"]
        texture_path = fields.get("texture", "")
        size = fields.get("size", f"{TILE}x{TILE}")
        size_x, size_y = (float(part) for part in size.split("x"))
        pivot = fields.get("pivot", "0.5,0.5")
        pivot_x, pivot_y = (float(part) for part in pivot.split(","))
        left = sprite["x"] - pivot_x * size_x
        bottom = sprite["y"] - pivot_y * size_y
        top_screen = map_h - (bottom + size_y)
        if texture_path == "__white__":
            tint = fields.get("tint", "1,1,1,1")
            parts = [float(part) for part in tint.split(",")]
            color = (int(parts[0] * 255), int(parts[1] * 255), int(parts[2] * 255), 120)
            canvas.rect(int(left), int(top_screen), int(left + size_x) - 1,
                        int(top_screen + size_y) - 1, color)
            notes.append(
                f"{sprite['entity']['name']}: __white__ 占位 {int(size_x)}x{int(size_y)} "
                f"cell=({left / tile_w:.1f},{bottom / tile_h:.1f})"
            )
            continue
        texture = textures.get(texture_path)
        if texture is None:
            errors.append(f"{sprite['entity']['name']}: 贴图缺失 {texture_path}")
            continue
        parts = [float(part) for part in fields.get("tint", "1,1,1,1").split(",")]
        tint = tuple(parts) if any(abs(part - 1.0) > 1e-6 for part in parts) else None
        blit_region(canvas, texture, 0, 0, texture.w, texture.h,
                    int(left), int(top_screen), int(size_x), int(size_y), tint)
        notes.append(
            f"{sprite['entity']['name']}: {os.path.basename(texture_path)} "
            f"cell=({left / tile_w:.2f},{bottom / tile_h:.2f}) "
            f"size={int(size_x)}x{int(size_y)} layer={sprite['layer']} order={sprite['order']}"
        )

    if grid:
        for cx in range(width + 1):
            for y in range(map_h):
                canvas.set(cx * tile_w, y, (255, 255, 255, 28))
        for cy in range(height + 1):
            for x in range(map_w):
                canvas.set(x, cy * tile_h, (255, 255, 255, 28))

    if scale != 1:
        canvas = canvas.scaled(scale)
    return canvas, header, notes, errors


def main():
    parser = argparse.ArgumentParser(description="把 .scene 渲染成 PNG 预览")
    parser.add_argument("scene")
    parser.add_argument("--root", default=".", help="工程根目录（解析资源相对路径）")
    parser.add_argument("--out", default="", help="输出 PNG（默认 .temp/scene_preview_<name>.png）")
    parser.add_argument("--scale", type=int, default=1)
    parser.add_argument("--grid", action="store_true", help="叠加格线")
    args = parser.parse_args()

    scene_name = os.path.splitext(os.path.basename(args.scene))[0]
    out = args.out or os.path.join(".temp", f"scene_preview_{scene_name}.png")
    canvas, header, notes, errors = render(args.scene, args.root, args.scale, args.grid)
    if canvas is None:
        return  # 纯 UI 场景：已打印 skip，退出码 0
    os.makedirs(os.path.dirname(out) or ".", exist_ok=True)
    with open(out, "wb") as handle:
        handle.write(canvas.to_png())
    print(f"scene    {args.scene} ({header.get('display_name', scene_name)})")
    for note in notes:
        print("  " + note)
    for error in errors:
        print("  [FAIL] " + error)
    print(f"wrote    {out} ({canvas.w}x{canvas.h})")
    if errors:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
