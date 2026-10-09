#!/usr/bin/env python3
"""无 GPU 的 `.prefab` 实体层合成器：把预设里的静态骨架按**槽位序**印成一张 PNG。

为什么要有它：验收门（`jhyt_m70_hud_prefab`）量的是读数（矩形/贴图名/边距/颜色），
换切图这种"字节对但样子不对"的毛病它看不见——九宫格边距选大了会把整块塌成边框、
选小了角花会被拉糊，两类在黄金表里都是同一串合法数字。这里用与引擎同一条九宫格规则
（`pixel_kit.draw_nine_slice`，对齐 `core/ui.rs` 的 `nine_slice_cells`，含"边距之和超过
目标就按比例缩边距"的 clamp）真画一遍，并顺手打印**边距契约**读数：每块的目标尺寸、
边距、是否触发 clamp、可拉伸区还剩几像素。

只看实体层（预设），不看动态层：运行期元素（招式图标/状态图标/头像脸/文字）由脚本每帧
写，不在预设文件里；本工具的目的是判"切图铺在这个尺寸上走样没有"，不是复现整屏。

Usage:
    python tools/preview_prefab.py assets/prefabs/hud.prefab --out .temp/hud_entity.png
    python tools/preview_prefab.py <prefab> --zoom 3          # 顺带出一张 3× 放大图
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pixel_kit import Canvas, draw_nine_slice, load_canvas  # noqa: E402

DESIGN_W, DESIGN_H = 1280.0, 720.0
# 预设里初始 shown、但 A 态（山庄开局、晴天、无状态、无结局）被脚本 `set_visible(false)`
# 收掉的实体——依据是黄金表 `jhyt_m70_hud_golden.tsv` 的 `hidden` 行。全屏雨罩和结局横幅
# 若按初始状态画，会把整张预览糊成一层蓝、正中间盖一块黑，目验就白做了。
A_STATE_HIDDEN = "HudRain,HudEndingBox,HudEndingTitle,HudEndingLine,HudFeedback,HudSkillCard8"


def load_texture(path, root):
    """按工程根解析贴图；内置白图 `__white__` 用 1×1 纯白（与引擎同语义）。"""
    if path == "__white__":
        return Canvas(1, 1, (255, 255, 255, 255))
    return load_canvas(os.path.join(root, path.replace("/", os.sep)))


def project_root(prefab):
    """贴图路径是**工程根**相对的（`assets/art/…`），所以从预设往上找到 `project.toml`。"""
    current = os.path.dirname(os.path.abspath(prefab))
    while True:
        if os.path.isfile(os.path.join(current, "project.toml")):
            return current
        parent = os.path.dirname(current)
        if parent == current:
            raise SystemExit(f"{prefab} 往上找不到 project.toml，解不了贴图路径")
        current = parent


def parse_prefab(path):
    """返回按槽位升序的实体列表：{slot,name,texture,color,slice,rect}。

    只认 hud 这类"根 + 绝对版面"的写法（anchors 全 0、offsets = left,top,right,bottom）；
    锚点非 0 说明这块跟着父节点变，静态合成器解不了，直接报错而不是猜。
    """
    entities = {}
    order = []
    with open(path, encoding="utf-8") as handle:
        for raw in handle:
            line = raw.rstrip("\n").rstrip("\r")
            if not line or line.startswith("#"):
                continue
            cols = line.split("\t")
            if cols[0] == "entity" and len(cols) >= 2:
                slot = int(cols[1])
                entities[slot] = {"slot": slot, "name": cols[2], "texture": None}
                order.append(slot)
                continue
            if cols[0] != "component" or len(cols) < 3:
                continue
            slot, kind = int(cols[1]), cols[2]
            body = cols[3] if len(cols) > 3 else ""
            node = entities.get(slot)
            if node is None:
                continue
            fields = {}
            for piece in body.split(";"):
                if "=" in piece:
                    key, value = piece.split("=", 1)
                    fields[key] = value
            if kind == "Image":
                color = [float(v) for v in fields.get("color", "1,1,1").split(",")]
                color = (color + [1.0])[:4]
                slice_value = fields.get("nine_slice")
                node["texture"] = fields.get("texture", "")
                node["color"] = color
                node["slice"] = None if slice_value in (None, "", "-") else [
                    float(v) for v in slice_value.split(",")
                ]
            elif kind == "UiLayout":
                anchors = [float(v) for v in fields.get("anchors", "0,0,0,0").split(",")]
                offsets = [float(v) for v in fields.get("offsets", "0,0,0,0").split(",")]
                if any(abs(value) > 1e-4 for value in anchors):
                    raise SystemExit(
                        f"{node['name']}（slot {slot}）用了非零锚点 {anchors}，"
                        "静态合成器只解 anchors=0 的绝对版面"
                    )
                x1, y1, x2, y2 = offsets
                node["rect"] = (x1, y1, x2 - x1, y2 - y1)
    return [entities[slot] for slot in sorted(entities)]


def composite(nodes, scale=1):
    cv = Canvas(int(DESIGN_W * scale), int(DESIGN_H * scale), (16, 18, 22, 255))
    cache = {}
    report = []
    for node in nodes:
        if node.get("texture") is None or node.get("rect") is None:
            continue
        rect = node["rect"]
        w, h = rect[2] * scale, rect[3] * scale
        if w <= 0 or h <= 0:
            report.append(f"  {node['name']:<18} 0 尺寸，跳过")
            continue
        path = node["texture"]
        if path not in cache:
            cache[path] = load_texture(path, node["_root"])
        texture = cache[path]
        border = node["slice"] or [0, 0, 0, 0]
        block = Canvas(int(w), int(h))
        scaled_border = [value * scale for value in border]
        if all(value == 0 for value in scaled_border):
            # 没有九宫格 = 整张拉伸（引擎对 `nine_slice=None` 就是铺满一个四边形）。
            from pixel_kit import blit_scaled

            blit_scaled(block, texture, 0, 0, texture.w, texture.h, 0, 0, w, h)
        else:
            draw_nine_slice(block, texture, 0, 0, w, h, scaled_border)
        blend(cv, block, rect[0] * scale, rect[1] * scale, node["color"])
        report.append(margin_line(node, texture, rect, border, scale))
    return cv, report


def margin_line(node, texture, rect, border, scale):
    """边距契约读数：目标 vs 四条边距，clamp 有没有发生、可拉伸区还剩几像素。"""
    x, y, w, h = rect
    left, right, top, bottom = border
    clamp_x = (left + right) > w
    clamp_y = (top + bottom) > h
    mid_w = w - min(w, left + right)
    mid_h = h - min(h, top + bottom)
    flag = "  ← clamp!" if (clamp_x or clamp_y) else ""
    return (
        f"  {node['name']:<18} {texture.w}x{texture.h} 目标 {w:.0f}x{h:.0f} "
        f"边距 {','.join(str(int(v)) for v in border) or '-'} "
        f"可拉伸区 {mid_w:.0f}x{mid_h:.0f}"
        f"{'' if border != [0, 0, 0, 0] else '（整张拉伸）'}{flag}"
    )


def blend(dst, src, dx, dy, color):
    """source-over：先把 src 乘上 Image 的染色（含 alpha），再叠到 dst 上。"""
    r, g, b, a = color
    for y in range(src.h):
        for x in range(src.w):
            pr, pg, pb, pa = src.get(x, y)
            alpha = pa / 255.0 * a
            if alpha <= 0.0:
                continue
            ox, oy = int(dx) + x, int(dy) + y
            if not (0 <= ox < dst.w and 0 <= oy < dst.h):
                continue
            dr, dg, db, da = dst.get(ox, oy)
            out_a = alpha + da / 255.0 * (1.0 - alpha)
            mix = lambda d, s: (s * alpha + d * (da / 255.0) * (1.0 - alpha)) / out_a  # noqa: E731
            dst.set(ox, oy, (
                int(round(mix(dr, pr * r))),
                int(round(mix(dg, pg * g))),
                int(round(mix(db, pb * b))),
                int(round(out_a * 255)),
            ))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("prefab")
    parser.add_argument("--out", default=".temp/prefab_entity.png")
    parser.add_argument("--zoom", type=int, default=0, help="额外输出一张 Nx 放大图（看角花/接缝）")
    parser.add_argument("--crop", default="", help="放大图只裁这块矩形：x,y,w,h（设计像素）")
    parser.add_argument("--hide", default=A_STATE_HIDDEN, help="逗号分隔的实体名，按 A 态收掉（默认见 A_STATE_HIDDEN）")
    args = parser.parse_args()

    root = project_root(args.prefab)
    hidden = {name.strip() for name in args.hide.split(",") if name.strip()}
    nodes = [node for node in parse_prefab(args.prefab) if node["name"] not in hidden]
    for node in nodes:
        node["_root"] = root
    cv, report = composite(nodes)
    os.makedirs(os.path.dirname(args.out) or ".", exist_ok=True)
    with open(args.out, "wb") as handle:
        handle.write(cv.to_png())
    print(f"{args.prefab}：实体层 {len(nodes)} 个实体，合成 → {args.out}")
    print("\n".join(report))
    if args.zoom:
        x, y, w, h = (
            [float(v) for v in args.crop.split(",")] if args.crop else (0, 0, DESIGN_W, DESIGN_H)
        )
        crop = Canvas(int(w), int(h))
        for cy in range(int(h)):
            for cx in range(int(w)):
                crop.set(cx, cy, cv.get(int(x) + cx, int(y) + cy))
        big = crop.scaled(args.zoom)
        path = args.out.replace(".png", f"_zoom{args.zoom}.png")
        with open(path, "wb") as handle:
            handle.write(big.to_png())
        print(f"放大图（{args.zoom}×，裁 {args.crop or '整屏'}）→ {path}")


if __name__ == "__main__":
    main()
