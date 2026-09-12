# -*- coding: utf-8 -*-
"""把 .scene 合成成一张 PNG 预览（地编验收用，不需要开窗口/GPU）。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/preview_scene.py scenes/main.scene
    python tools/preview_scene.py scenes/main.scene --out .temp/main.png --scale 1 --grid

渲染规则（严格照引擎实现，用来发现"数据和美术对不上"的问题）：

  * Tilemap：cell (cx,cy) 覆盖世界 x∈[cx*32,(cx+1)*32]、y∈[cy*32,(cy+1)*32]，row 0 在最底；
    屏幕 y 向下，所以屏幕行 = (height-1-cy)。tile 值 n(1..6) 取 tileset 第 n 列，
    即纹理区域 x∈[(n-1)*32, n*32)、**V 方向整幅拉伸到格子高度**
    （core/tilemap.rs tile_uv 只按 6 列切 U，V 恒为 0..1）。
  * Sprite：pivot=(0,0) 时 transform 即左下角；图像顶行对应 quad 顶部。
    `__white__` 内建贴图按 tint 画成半透明占位块，便于看玩家/NPC 站位。
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


def blit_region(dst, src, sx, sy, sw, sh, dx, dy, dw, dh):
    """把 src 的 (sx,sy,sw,sh) 区域按最近邻缩放到 (dx,dy,dw,dh)。"""
    for oy in range(dh):
        ysrc = sy + min(sh - 1, int(oy * sh / dh))
        for ox in range(dw):
            xsrc = sx + min(sw - 1, int(ox * sw / dw))
            color = src.get(xsrc, ysrc)
            if color[3] > 0:
                dst.set(dx + ox, dy + oy, color)


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


def render(scene_path, root, scale=1, grid=False, only_layers=None):
    header, entities = parse_scene(scene_path)
    tilemaps = [e for e in entities if "Tilemap" in e["components"]]
    if not tilemaps:
        raise SystemExit(f"{scene_path}: 没有 Tilemap 实体（纯 UI 场景，不需要地编预览）")
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
        texture_path = fields.get("texture", "")
        tiles = fields.get("tiles", "")
        solid = fields.get("solid", "")
        tw = int(fields["width"])
        th = int(fields["height"])
        if len(tiles) != tw * th:
            errors.append(f"{entity['name']}: tiles 长度 {len(tiles)} != {tw}x{th}")
        if len(solid) != tw * th:
            errors.append(f"{entity['name']}: solid 长度 {len(solid)} != {tw}x{th}")
        for value in tiles:
            if not value.isdigit() or int(value) > 6:
                errors.append(f"{entity['name']}: tiles 含非法列号 {value!r}（引擎只支持 0..6）")
                break
        texture = textures.get(texture_path)
        if texture is None:
            errors.append(f"{entity['name']}: 贴图缺失 {texture_path}")
            continue
        if texture.w != 6 * int(float(fields["tile_size"].split("x")[0])):
            errors.append(
                f"{entity['name']}: tileset 宽 {texture.w} != 6*{tile_w}"
                "（引擎固定按 6 列切 U，多余列采不到）"
            )
        if texture.h != tile_h:
            notes.append(
                f"[note] {entity['name']}: tileset 高 {texture.h} != {tile_h}，"
                "按引擎规则整幅拉伸到格子高度"
            )
        drawn = 0
        for cy in range(th):
            for cx in range(tw):
                value = int(tiles[cy * tw + cx])
                if value <= 0:
                    continue
                drawn += 1
                blit_region(
                    canvas, texture,
                    (value - 1) * tile_w, 0, tile_w, texture.h,
                    cx * tile_w, (height - 1 - cy) * tile_h, tile_w, tile_h,
                )
        notes.append(f"{entity['name']}: tilemap {tw}x{th} 贴图 {texture_path} 画出 {drawn} 格")

    # 2) Sprites（按 layer/order 排序，稳定）
    sprites = []
    for index, entity in enumerate(entities):
        if "Sprite" not in entity["components"] or "Transform" not in entity["components"]:
            continue
        fields = parse_fields(entity["components"]["Sprite"])
        transform = parse_fields(entity["components"]["Transform"])
        layer = fields.get("sorting_layer", "default")
        order = int(float(fields.get("order", "0") or 0))
        sprites.append({
            "entity": entity,
            "fields": fields,
            "x": float(transform.get("x", 0.0)),
            "y": float(transform.get("y", 0.0)),
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
        blit_region(canvas, texture, 0, 0, texture.w, texture.h,
                    int(left), int(top_screen), int(size_x), int(size_y))
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
