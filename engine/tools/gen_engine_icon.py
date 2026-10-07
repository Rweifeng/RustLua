#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""RustLua 引擎图标生成器（纯标准库，本机没有 PIL）。

图案：深色圆角方砖 + 六齿齿轮环 + 嵌在齿轮内孔里的 `L`（Lua 的首字母，Lua 官方 logo 上那枚字）。
齿轮=引擎，L=脚本层；L 完整收在内孔内、不压环带，靠一圈深色描边与银色齿盘分开。
这套图给**所有引擎产物**共用：editor.exe / game.exe / assetc.exe 的窗口图标与 exe 文件图标。

用法（仓库根目录运行）：

    py engine/tools/gen_engine_icon.py           # 出图 + 目验表
    py engine/tools/gen_engine_icon.py --check   # 校验入库产物与规格一致

产物：

    engine/assets/engine_icon.png         256×256 RGBA，窗口图标（`include_bytes!` 那张）
    engine/assets/engine_icon.ico         七档位深的图标集，`build.rs` 编进 PE 资源当文件图标
    engine/assets/engine_icon_preview.png 目验表：五档尺寸各放大到 256 一格，
                                          左→右 256/128/64/32/16，上=浅底 下=深底

为什么自己算 SDF 而不是手画一张：
  * 标题栏尺寸是 Windows `SM_CXSMICON`（100% 缩放下 16px），eframe 用 Lanczos3 把这张 256
    直接缩到 16/32（`eframe::native::app_icon`）——16px 上糊不糊由半径、齿角宽、笔画粗细决定，
    得能量调重出，不能在大图上"感觉还行"；
  * 出图必须逐字节幂等，`--check` 才能当质量门（跟 `gen_ui_art.py --check` 一路）；
  * 圆/齿/直笔画都是解析形状，超采样后 box 平均即得反走样，无需外部光栅器。

为什么 .ico 里小尺寸要写成 DIB 而不是七档全塞 PNG：
  * ICO 允许内嵌 PNG（Vista+ 认），但 `zig rc` / 老 shell 对 <256 的 PNG 条目支持不一致，
    资源管理器一旦不认就退回通用 exe 图标——表现为"嵌了但看不到"，比编不过更难查；
  * 所以 16..128 走 32bpp BGRA DIB + 全 0 AND 掩码（透明靠 alpha 通道），只有 256 用 PNG。
"""

import argparse
import hashlib
import math
import os
import struct
import sys
import zlib

# --- 规格表（改这里就等于改设计；`--check` 按这份规格复算字节） -------------------

MASTER = 1536            # 超采样栅格边长；必须被下面每一档输出尺寸整除（1536=2^9·3）
OUT_SIZE = 256           # 入库 PNG 边长（eframe 会自己缩到 16/32/48）
PREVIEW_SIZES = (256, 128, 64, 32, 16)  # 目验表从左到右的顺序
# .ico 的档位：shell 实际只会问 16/24/32/48（100%~200% DPI 的小/中/大图标）和 256
# （400% 与"超大图标"视图）。不加 64/128 是因为 DIB 条目不压缩，128 一档就 65 KB，
# 而这两档没有任何宿主会去取——白涨 exe 体积。
ICO_SIZES = (16, 24, 32, 48, 256)
ICO_PNG_SIZES = (256,)   # 只有这些走内嵌 PNG，其余一律 DIB，见模块 docstring

PALETTE = {
    "tile": (0x22, 0x26, 0x2D),     # 方砖底色，贴编辑器深色皮肤
    "rim": (0x39, 0x41, 0x4D),      # 方砖内描边，让深色砖在深色标题栏上仍有边界
    "ink": (0x10, 0x13, 0x1A),      # 形体内描边（压在任何元素上都比白描边稳）
    "gear": (0xA9, 0xB4, 0xC2),     # 齿轮：冷银，退到第二视觉层级
    "glyph_l": (0xE9, 0xB4, 0x4C),  # L：暖金，最亮 = 主语义（Lua 的首字母）
}

# 圆角方砖：half 是半边长（单位 = 图幅），radius 是圆角半径，rim 是内描边厚。
TILE_HALF = 0.485
TILE_RADIUS = 0.150
TILE_RIM = 0.014

# 齿轮环（极坐标）：hole=内孔半径，ring=齿根半径，tip=齿顶半径。
# 内孔要给 L 留位置：hole 从 0.165 提到 0.205（字必须整个收在孔里），环带随之瘦到 0.095
# ——16px 上还剩 1.5px 银，够读出"环"，再瘦就没齿轮了。
# 齿槽要"浅而等宽"：齿顶只外伸 0.085（16px 上 1.4px），齿根角宽取半距（齿与槽各 30°）。
# 早先 base_hw=0.70 时槽只剩 18°，缩到 16px 齿槽全并掉，轮廓读成花瓣而不是齿轮。
GEAR_HOLE = 0.205
GEAR_RING = 0.300
GEAR_TIP = 0.385
GEAR_TEETH = 6
GEAR_BASE_HW = 0.52      # 齿根处角宽 = GEAR_BASE_HW * (π/齿数)，≈0.5 即齿与槽等宽
GEAR_TIP_HW = 0.30       # 齿顶处角宽 = GEAR_TIP_HW * (π/齿数)
GEAR_PHASE = math.pi / 2  # 让一齿正对上方，12 点方向不出现豁口
GEAR_HALO = 0.012        # 描边厚到 0.018 会把齿槽并掉（16px 上槽只剩 1px 都不到）

# L：两条方头笔画（矩形，不用胶囊）。竖笔 stem_top→corner、横笔 corner→foot_end。
# 两笔等长会被读成"时钟 12:15 的指针"而不是字母，所以横笔只做到竖笔的 0.63 长。
# 方头而不是圆头：圆头 + 等长 = 指针，方头才有"字"的收笔。
GLYPH_STEM_TOP = (0.435, 0.400)
GLYPH_CORNER = (0.435, 0.590)
GLYPH_FOOT_END = (0.555, 0.590)
GLYPH_HW = 0.040         # 笔画半边宽：256 上 20.5px，16px 上约 1.3px
GLYPH_HALO = 0.014

DARK_BG = (0x1E, 0x1E, 0x1E)
LIGHT_BG = (0xF2, 0xF2, 0xF2)

HERE = os.path.dirname(os.path.abspath(__file__))
ASSETS_DIR = os.path.join(os.path.dirname(HERE), "assets")
ICON_PATH = os.path.join(ASSETS_DIR, "engine_icon.png")
ICO_PATH = os.path.join(ASSETS_DIR, "engine_icon.ico")
PREVIEW_PATH = os.path.join(ASSETS_DIR, "engine_icon_preview.png")


# --- SDF 基元 -------------------------------------------------------------------

def sd_round_rect(x, y, half, radius):
    """到圆角方砖边界的最小距离，内部为负。"""
    qx = abs(x - 0.5) - (half - radius)
    qy = abs(y - 0.5) - (half - radius)
    ox = max(qx, 0.0)
    oy = max(qy, 0.0)
    return math.hypot(ox, oy) + min(max(qx, qy), 0.0) - radius


def gear_outer_radius(theta):
    """给定极角返回齿轮外边界半径（齿根↔齿顶的梯形齿）。"""
    pitch = 2.0 * math.pi / GEAR_TEETH
    half_pitch = pitch / 2.0
    # 折到最近齿中心的角距离
    a = abs(((theta - GEAR_PHASE + half_pitch) % pitch) - half_pitch)
    base_hw = GEAR_BASE_HW * half_pitch
    tip_hw = GEAR_TIP_HW * half_pitch
    if a <= tip_hw:
        return GEAR_TIP
    if a <= base_hw:
        t = (a - tip_hw) / (base_hw - tip_hw)
        return GEAR_TIP + (GEAR_RING - GEAR_TIP) * t
    return GEAR_RING


def sd_gear(x, y):
    """齿轮环的近似 SDF：内为负，孔内为正（用 max 交环带）。"""
    dx = x - 0.5
    dy = y - 0.5
    r = math.hypot(dx, dy)
    outer = r - gear_outer_radius(math.atan2(dy, dx))
    inner = GEAR_HOLE - r
    return max(outer, inner)


def sd_box(x, y, x0, y0, x1, y1):
    """到矩形边界的最小距离，内部为负。"""
    qx = max(x0 - x, x - x1)
    qy = max(y0 - y, y - y1)
    return math.hypot(max(qx, 0.0), max(qy, 0.0)) + min(max(qx, qy), 0.0)


def glyph_boxes():
    """L 的两条方头笔画，各自一个矩形（centerline 端点 + 半边宽推出四边）。"""
    stem_x, top = GLYPH_STEM_TOP
    _, bottom = GLYPH_CORNER
    foot_x = GLYPH_FOOT_END[0]
    hw = GLYPH_HW
    return [
        (stem_x - hw, top - hw, stem_x + hw, bottom + hw),
        (stem_x - hw, bottom - hw, foot_x + hw, bottom + hw),
    ]


def sd_glyph_l(x, y):
    return min(sd_box(x, y, *box) for box in glyph_boxes())


# --- 上色 -----------------------------------------------------------------------

def paint(x, y):
    """返回单位坐标处颜色 (r, g, b, a)；None 表示完全透明。

    画序即层级：L（含描边）压在齿轮上，齿轮压在方砖+内描边上。
    """
    tile = sd_round_rect(x, y, TILE_HALF, TILE_RADIUS)
    if tile > 0.0:
        return None
    glyph = sd_glyph_l(x, y)
    if glyph <= GLYPH_HALO:
        return PALETTE["glyph_l"] + (255,) if glyph <= 0.0 else PALETTE["ink"] + (255,)
    gear = sd_gear(x, y)
    if gear <= 0.0:
        return PALETTE["gear"] + (255,)
    if gear <= GEAR_HALO:
        return PALETTE["ink"] + (255,)
    if tile > -TILE_RIM:
        return PALETTE["rim"] + (255,)
    return PALETTE["tile"] + (255,)


def render_master():
    """把 MASTER×MASTER 栅格扫一遍，逐子像素上色（straight RGBA）。"""
    step = 1.0 / MASTER
    rows = []
    for py in range(MASTER):
        y = (py + 0.5) * step
        row = bytearray(MASTER * 4)
        for px in range(MASTER):
            color = paint((px + 0.5) * step, y)
            base = px * 4
            if color is None:
                continue
            row[base:base + 4] = bytes(color)
        rows.append(row)
    return rows


def downsample(rows, size):
    """box 平均到 size×size，按预乘 α 平均后再还原（否则深色砖边缘会吃进黑边）。"""
    factor = MASTER // size
    if factor * factor * size * size != MASTER * MASTER:
        raise ValueError(f"{size} 不能整除自 {MASTER}")
    out = bytearray(size * size * 4)
    for oy in range(size):
        base_row = oy * factor
        for ox in range(size):
            base_col = ox * factor
            rs = gs = bs = al = 0
            for sy in range(base_row, base_row + factor):
                row = rows[sy]
                for sx in range(base_col, base_col + factor):
                    i = sx * 4
                    a = row[i + 3]
                    al += a
                    rs += row[i] * a
                    gs += row[i + 1] * a
                    bs += row[i + 2] * a
            o = (oy * size + ox) * 4
            if al == 0:
                continue
            n = factor * factor
            # al 是 α 之和：预乘色除以它即还原 straight 色，α 本身对 n 取平均。
            out[o] = min(255, round(rs / al))
            out[o + 1] = min(255, round(gs / al))
            out[o + 2] = min(255, round(bs / al))
            out[o + 3] = min(255, round(al / n))
    return out


def encode_png(width, height, raw_rgba):
    """RGBA8（color type 6）PNG，filter 0。"""
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    scanlines = bytearray()
    stride = width * 4
    for y in range(height):
        scanlines.append(0)
        scanlines += raw_rgba[y * stride:(y + 1) * stride]
    return (
        b"\x89PNG\r\n\x1a\n"
        + _chunk(b"IHDR", ihdr)
        + _chunk(b"IDAT", zlib.compress(bytes(scanlines), 9))
        + _chunk(b"IEND", b"")
    )


def _chunk(tag, body):
    return (
        struct.pack(">I", len(body))
        + tag
        + body
        + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)
    )


def _dib_entry(raw_rgba, size):
    """32bpp BGRA 的 DIB 条目（自下而上）+ 全 0 AND 掩码。

    掩码全 0 是有意的：单色掩码只表达"完全不透明/完全透明"，会毁掉抗锯齿边缘的 α，
    透明一律交给第 4 通道。
    """
    stride = size * 4
    mask_stride = ((size + 31) // 32) * 4
    pixels = bytearray()
    for y in range(size - 1, -1, -1):
        row = raw_rgba[y * stride:(y + 1) * stride]
        for x in range(size):
            i = x * 4
            pixels += bytes((row[i + 2], row[i + 1], row[i], row[i + 3]))
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    return header + bytes(pixels) + bytes(mask_stride * size)


def encode_ico(rows):
    """图标集：ICONDIRENTRY 表 + 各档图像数据，供 build.rs 编进 PE 资源。"""
    images = []
    for size in ICO_SIZES:
        raw = downsample(rows, size)
        body = encode_png(size, size, raw) if size in ICO_PNG_SIZES else _dib_entry(raw, size)
        images.append((size, body))
    offset = 6 + 16 * len(images)
    directory = bytearray()
    data = bytearray()
    for size, body in images:
        edge = 0 if size >= 256 else size  # ICO 目录里 256 用 0 表示（u8 放不下）
        directory += struct.pack("<BBBBHHII", edge, edge, 0, 0, 1, 32, len(body), offset)
        data += body
        offset += len(body)
    return struct.pack("<HHH", 0, 1, len(images)) + bytes(directory) + bytes(data)


def build_preview(rows):
    """目验表：每格最近邻放大到 256，浅色底一行 + 深色底一行，格内留 8px 间距。"""
    cell = 256
    pad = 8
    columns = len(PREVIEW_SIZES)
    width = columns * cell + (columns + 1) * pad
    height = 2 * cell + 3 * pad
    canvas = bytearray(width * height * 4)
    for y in range(height):
        base = y * width * 4
        for x in range(width):
            i = base + x * 4
            canvas[i] = 0x80
            canvas[i + 1] = 0x82
            canvas[i + 2] = 0x86
            canvas[i + 3] = 255

    for row_index, bg in enumerate((LIGHT_BG, DARK_BG)):
        for col_index, size in enumerate(PREVIEW_SIZES):
            icon = downsample(rows, size)
            scale = cell // size
            origin_x = pad + col_index * (cell + pad)
            origin_y = pad + row_index * (cell + pad)
            for cy in range(cell):
                for cx in range(cell):
                    sx = min(size - 1, cx // scale)
                    sy = min(size - 1, cy // scale)
                    i = (sy * size + sx) * 4
                    a = icon[i + 3]
                    o = ((origin_y + cy) * width + (origin_x + cx)) * 4
                    for channel in range(3):
                        src = icon[i + channel]
                        canvas[o + channel] = (src * a + bg[channel] * (255 - a)) // 255
                    canvas[o + 3] = 255
    return encode_png(width, height, canvas)


def glyph_max_extent():
    """字（含描边）最外沿到图心的距离，必须 < GEAR_HOLE 才算整个收进内孔。

    方头笔画离图心最远的点一定是某个矩形角点，所以逐个角点量。这条是设计约束而不是
    渲染细节——调 GEAR_HOLE / GLYPH_* 任一项都可能让字悄悄爬出孔去，主程序据此判红。
    """
    corners = [(cx, cy)
               for x0, y0, x1, y1 in glyph_boxes()
               for cx in (x0, x1) for cy in (y0, y1)]
    return max(math.hypot(px - 0.5, py - 0.5) for px, py in corners) + GLYPH_HALO


def spec_digest():
    """规格表指纹：--check 时和产物一起打印，方便比对设计是否被改过。"""
    parts = [
        f"tile={TILE_HALF}/{TILE_RADIUS}/{TILE_RIM}",
        f"gear={GEAR_HOLE}/{GEAR_RING}/{GEAR_TIP}/{GEAR_TEETH}"
        f"/{GEAR_BASE_HW}/{GEAR_TIP_HW}/{GEAR_HALO}",
        f"glyph_l={GLYPH_STEM_TOP}/{GLYPH_CORNER}/{GLYPH_FOOT_END}"
        f"/{GLYPH_HW}/{GLYPH_HALO}",
        f"palette={sorted(PALETTE.items())}",
        f"sizes={MASTER}/{OUT_SIZE}/{ICO_SIZES}",
    ]
    return hashlib.sha256("\n".join(parts).encode("utf-8")).hexdigest()[:12]


def main():
    parser = argparse.ArgumentParser(description="生成 RustLua 引擎图标（PNG + ICO + 目验表）")
    parser.add_argument("--check", action="store_true", help="只复算并校验入库产物是否一致")
    parser.add_argument("--digest", action="store_true", help="只打印规格指纹，不写文件")
    arguments = parser.parse_args()

    if arguments.digest:
        print(spec_digest())
        return 0

    extent = glyph_max_extent()
    if extent >= GEAR_HOLE:
        print(f"FAIL 字压出内孔：最外沿 {extent:.3f} >= GEAR_HOLE {GEAR_HOLE:.3f}，"
              f"要么调小 GLYPH_*，要么加大 GEAR_HOLE")
        return 1

    rows = render_master()
    artifacts = (
        (ICON_PATH, encode_png(OUT_SIZE, OUT_SIZE, downsample(rows, OUT_SIZE))),
        (ICO_PATH, encode_ico(rows)),
    )

    if arguments.check:
        for path, expected in artifacts:
            name = os.path.basename(path)
            if not os.path.exists(path):
                print(f"FAIL engine/assets/{name} 不存在，先跑本工具出图")
                return 1
            with open(path, "rb") as handle:
                actual = handle.read()
            if actual != expected:
                print(f"FAIL engine/assets/{name} 与规格复算不一致"
                      f"（规格指纹 {spec_digest()}），重跑本工具出图")
                return 1
            print(f"OK   engine/assets/{name} ({len(actual)} bytes)")
        print(f"OK   规格指纹 {spec_digest()}")
        return 0

    os.makedirs(ASSETS_DIR, exist_ok=True)
    for path, body in artifacts:
        with open(path, "wb") as handle:
            handle.write(body)
        print(f"wrote engine/assets/{os.path.basename(path)} ({len(body)} bytes)")
    with open(PREVIEW_PATH, "wb") as handle:
        handle.write(build_preview(rows))
    print(f"wrote engine/assets/{os.path.basename(PREVIEW_PATH)}，规格指纹 {spec_digest()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
