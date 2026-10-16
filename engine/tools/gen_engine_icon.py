#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Deterministic, stdlib-only RustLua icons and startup splash.

Midnight tile, copper six-tooth gear, ivory crescent and orbiting dot.
Analytic shapes are supersampled; lettering uses bundled vector glyphs.
Run normally to generate all four assets, or --check to compare every byte.
ICO sizes 16/24/32/48 use BGRA DIB + zero AND masks for resource compiler
compatibility; 256 uses PNG. Preview columns: 256/128/64/32/16 on light/dark.
"""

import argparse
import hashlib
import math
import os
import struct
import sys
import zlib

# Normalized design coordinates; broad strokes retain contrast at 16px.
MASTER = 1536
OUT_SIZE = 256
PREVIEW_SIZES = (256, 128, 64, 32, 16)
ICO_SIZES = (16, 24, 32, 48, 256)
ICO_PNG_SIZES = (256,)

PALETTE = {
    "tile": (15, 25, 43),
    "rim": (39, 54, 75),
    "gear": (218, 132, 79),
    "ivory": (247, 239, 217),
    "muted": (150, 166, 184),
}
TILE_HALF = 0.485
TILE_RADIUS = 0.150
TILE_RIM = 0.009
GEAR_HOLE = 0.225
GEAR_RING = 0.315
GEAR_TIP = 0.405
GEAR_TEETH = 6
GEAR_BASE_HW = 0.58
GEAR_TIP_HW = 0.34
GEAR_PHASE = math.pi / 2
MOON = (0.475, 0.515, 0.163)
MOON_CUT = (0.545, 0.465, 0.145)
ORBIT = (0.605, 0.395, 0.043)
SPLASH_SIZE = (1280, 720)
SPLASH_MARK_SIZE = 180
DARK_BG = PALETTE["tile"]
LIGHT_BG = (242, 242, 242)

HERE = os.path.dirname(os.path.abspath(__file__))
ASSETS_DIR = os.path.join(os.path.dirname(HERE), "assets")
ICON_PATH = os.path.join(ASSETS_DIR, "engine_icon.png")
ICO_PATH = os.path.join(ASSETS_DIR, "engine_icon.ico")
PREVIEW_PATH = os.path.join(ASSETS_DIR, "engine_icon_preview.png")
SPLASH_PATH = os.path.join(ASSETS_DIR, "engine_splash.png")


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


def sd_circle(x, y, circle):
    cx, cy, radius = circle
    return math.hypot(x - cx, y - cy) - radius


def sd_moon(x, y):
    return min(max(sd_circle(x, y, MOON), -sd_circle(x, y, MOON_CUT)),
               sd_circle(x, y, ORBIT))


def paint(x, y):
    """Straight RGBA at normalized coordinates; None outside the tile."""
    tile = sd_round_rect(x, y, TILE_HALF, TILE_RADIUS)
    if tile > 0.0:
        return None
    if sd_moon(x, y) <= 0.0:
        return PALETTE["ivory"] + (255,)
    if sd_gear(x, y) <= 0.0:
        return PALETTE["gear"] + (255,)
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


# Monoline vector capitals on a 4 x 6 grid, with clipped geometric corners.
LETTER_PATHS = {
    "R": (((0, 6), (0, 0), (3, 0), (4, 1), (4, 2), (3, 3), (0, 3)),
          ((2, 3), (4, 6))),
    "U": (((0, 0), (0, 5), (1, 6), (3, 6), (4, 5), (4, 0)),),
    "S": (((4, 0), (1, 0), (0, 1), (0, 2), (1, 3), (3, 3),
           (4, 4), (4, 5), (3, 6), (0, 6)),),
    "T": (((0, 0), (4, 0)), ((2, 0), (2, 6))),
    "L": (((0, 0), (0, 6), (4, 6)),),
    "A": (((0, 6), (0, 2), (2, 0), (4, 2), (4, 6)), ((0, 3.5), (4, 3.5))),
    "G": (((4, 0), (1, 0), (0, 1), (0, 5), (1, 6), (4, 6), (4, 3), (2, 3)),),
    "M": (((0, 6), (0, 0), (2, 3), (4, 0), (4, 6)),),
    "E": (((4, 0), (0, 0), (0, 6), (4, 6)), ((0, 3), (3, 3))),
    "N": (((0, 6), (0, 0), (4, 6), (4, 0)),),
    "I": (((2, 0), (2, 6)),),
}


def draw_lettering(canvas, text, top, scale, tracking, stroke, color):
    """Centered vector text, antialiased by analytic stroke coverage."""
    width, height = SPLASH_SIZE
    advance = 4 * scale + tracking
    left = (width - (len(text) * advance - tracking)) / 2
    for index, letter in enumerate(text):
        if letter == " ":
            continue
        origin = left + index * advance
        for path in LETTER_PATHS[letter]:
            for (ax, ay), (bx, by) in zip(path, path[1:]):
                ax, ay = origin + ax * scale, top + ay * scale
                bx, by = origin + bx * scale, top + by * scale
                dx, dy = bx - ax, by - ay
                radius = stroke / 2
                for y in range(max(0, int(min(ay, by) - radius - 1)),
                               min(height, int(max(ay, by) + radius + 2))):
                    for x in range(max(0, int(min(ax, bx) - radius - 1)),
                                   min(width, int(max(ax, bx) + radius + 2))):
                        t = max(0, min(1, ((x + .5 - ax) * dx + (y + .5 - ay) * dy)
                                       / (dx * dx + dy * dy)))
                        distance = math.hypot(x + .5 - ax - t * dx, y + .5 - ay - t * dy)
                        coverage = max(0, min(1, radius + .5 - distance))
                        offset = (y * width + x) * 4
                        for channel in range(3):
                            # Max coverage at joins avoids dark seams or double blending.
                            value = round(DARK_BG[channel] * (1 - coverage) + color[channel] * coverage)
                            canvas[offset + channel] = max(canvas[offset + channel], value)


def build_splash():
    """Opaque 1280x720 navy splash; centered 180px mark and custom lettering."""
    width, height = SPLASH_SIZE
    canvas = bytearray(bytes(DARK_BG + (255,)) * (width * height))
    size = SPLASH_MARK_SIZE
    left, top = (width - size) // 2, 218
    samples = 4
    for y in range(size):
        for x in range(size):
            total = [0, 0, 0]
            for sy in range(samples):
                for sx in range(samples):
                    color = paint((x + (sx + .5) / samples) / size,
                                  (y + (sy + .5) / samples) / size)
                    for channel in range(3):
                        total[channel] += color[channel] if color else DARK_BG[channel]
            offset = ((top + y) * width + left + x) * 4
            canvas[offset:offset + 3] = bytes(round(c / (samples * samples)) for c in total)
    draw_lettering(canvas, "RUSTLUA", 440, 6, 16, 3.2, PALETTE["ivory"])
    for y in range(501, 503):
        for x in range(609, 671):
            offset = (y * width + x) * 4
            canvas[offset:offset + 3] = bytes(PALETTE["gear"])
    draw_lettering(canvas, "GAME ENGINE", 528, 2, 6, 1.3, PALETTE["muted"])
    return encode_png(width, height, canvas)


def spec_digest():
    """Fingerprint the full generator, including layout and vector glyphs."""
    with open(os.path.abspath(__file__), "rb") as handle:
        source = handle.read().replace(b"\r\n", b"\n")
    return hashlib.sha256(source).hexdigest()[:12]


def main():
    parser = argparse.ArgumentParser(description="生成 RustLua 引擎图标（PNG + ICO + 目验表）")
    parser.add_argument("--check", action="store_true", help="只复算并校验入库产物是否一致")
    parser.add_argument("--digest", action="store_true", help="只打印规格指纹，不写文件")
    arguments = parser.parse_args()

    if arguments.digest:
        print(spec_digest())
        return 0

    extent = max(math.hypot(cx - .5, cy - .5) + radius
                 for cx, cy, radius in (MOON, ORBIT))
    if extent >= GEAR_HOLE:
        print("FAIL crescent or orbit overlaps the gear")
        return 1

    rows = render_master()
    artifacts = (
        (SPLASH_PATH, build_splash()),
        (ICON_PATH, encode_png(OUT_SIZE, OUT_SIZE, downsample(rows, OUT_SIZE))),
        (ICO_PATH, encode_ico(rows)),
        (PREVIEW_PATH, build_preview(rows)),
    )

    for path, body in artifacts:
        if arguments.check:
            if not os.path.exists(path):
                print(f"FAIL engine/assets/{os.path.basename(path)} missing; run generator")
                return 1
            with open(path, "rb") as handle:
                actual = handle.read()
            if actual != body:
                print(f"FAIL engine/assets/{os.path.basename(path)} differs from generator")
                return 1
            print(f"OK   engine/assets/{os.path.basename(path)} ({len(actual)} bytes)")
        else:
            os.makedirs(ASSETS_DIR, exist_ok=True)
            with open(path, "wb") as handle:
                handle.write(body)
            print(f"wrote engine/assets/{os.path.basename(path)} ({len(body)} bytes)")
    if arguments.check:
        print(f"OK   generator {spec_digest()}")
        return 0
    print(f"wrote assets, generator {spec_digest()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
