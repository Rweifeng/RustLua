# -*- coding: utf-8 -*-
"""《江湖有田》标题背景像素化（把 assets/art/title/_title_bg_source.png 转成像素画风）

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/pixelate_title_bg.py                     # 默认 4× 像素化
    python tools/pixelate_title_bg.py --scale 2           # 与游戏内世界 2× 缩放同像素密度
    python tools/pixelate_title_bg.py --dither 28         # 抖动强度（天空渐变过渡带宽度）
    python tools/pixelate_title_bg.py --out <路径>

产出：

    assets/art/title/title_bg_pixel.png          像素化结果（默认 1280×720，RGB）
    assets/art/title/title_bg_pixel_preview.png  原图 vs 像素化 对比预览（半尺寸并排）

做法（对应美术资源方案 §3.4/§6 与 UI 资源设计 §2/§3）：

  1. **整数倍降采样**：1280×720 → (1280/scale)×(720/scale)。帧格对齐世界 tile 的像素密度，
     不做任意比例缩放，避免半像素。
  2. **限定色板 + 有序抖动**：色板由"夜空→天光→月"的 10 个锚点插值成 ~28 色渐变梯
     （锚点取自原图夜色 + UI 色板 indigo/paper token），天空/月晕用 4×4 Bayer 抖动
     做出像素画特有的分层过渡。
  3. **剪影保持锐利**：块内色差大的格子（山脊、月缘）取**主色**且不抖动，
     保证山脊是 1px 硬边而不是棋盘格。
  4. **月亮分阶**：按块内高亮像素占比把月心/月缘归到 paper/50 与 paper/200 两阶，出硬边圆。
  5. **星点重打**：原图 1–2px 星点在降采样后会消失，按亮度阈值在原位置重新点成单像素星。

依赖：Python 3 标准库（zlib/struct），不需要 Pillow。
"""

import argparse
import os
import struct
import zlib

# ---------------------------------------------------------------------------
# PNG 解码 / 编码（RGBA/RGB 8bit，非隔行）
# ---------------------------------------------------------------------------


def _paeth(a, b, c):
    p = a + b - c
    pa = abs(p - a)
    pb = abs(p - b)
    pc = abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    if pb <= pc:
        return b
    return c


def decode_png(path):
    """返回 (width, height, channels, rgb_bytes)。仅支持 8bit 非隔行 PNG。"""
    with open(path, "rb") as handle:
        data = handle.read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("not a PNG: " + path)
    pos = 8
    idat = bytearray()
    width = height = depth = color_type = interlace = None
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos:pos + 4])
        tag = data[pos + 4:pos + 8]
        body = data[pos + 8:pos + 8 + length]
        pos += 12 + length
        if tag == b"IHDR":
            width, height, depth, color_type, _, _, interlace = struct.unpack(">IIBBBBB", body)
        elif tag == b"IDAT":
            idat += body
        elif tag == b"IEND":
            break
    if depth != 8 or interlace:
        raise ValueError(f"unsupported PNG (depth={depth}, interlace={interlace})")
    channels = {2: 3, 6: 4}.get(color_type)
    if channels is None:
        raise ValueError(f"unsupported PNG color type {color_type} (need RGB/RGBA)")
    raw = zlib.decompress(bytes(idat))
    stride = width * channels
    out = bytearray(width * height * channels)
    prev = bytearray(stride)
    offset = 0
    for y in range(height):
        filter_type = raw[offset]
        offset += 1
        line = bytearray(raw[offset:offset + stride])
        offset += stride
        if filter_type == 1:
            for i in range(channels, stride):
                line[i] = (line[i] + line[i - channels]) & 0xFF
        elif filter_type == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif filter_type == 3:
            for i in range(stride):
                left = line[i - channels] if i >= channels else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif filter_type == 4:
            for i in range(stride):
                left = line[i - channels] if i >= channels else 0
                up = prev[i]
                upleft = prev[i - channels] if i >= channels else 0
                line[i] = (line[i] + _paeth(left, up, upleft)) & 0xFF
        elif filter_type != 0:
            raise ValueError(f"unknown PNG filter {filter_type}")
        out[y * stride:(y + 1) * stride] = line
        prev = line
    return width, height, channels, bytes(out)


def _chunk(tag, body):
    return (
        struct.pack(">I", len(body))
        + tag
        + body
        + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)
    )


def encode_png_rgb(width, height, raw_rgb):
    """RGB8（color type 2）PNG 编码；背景图不透明，不需要 alpha 通道。"""
    stride = width * 3
    filtered = bytearray()
    for y in range(height):
        filtered.append(0)  # filter: none（像素画用无损整数像素，压缩交给 zlib）
        filtered += raw_rgb[y * stride:(y + 1) * stride]
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + _chunk(b"IHDR", ihdr)
        + _chunk(b"IDAT", zlib.compress(bytes(filtered), 9))
        + _chunk(b"IEND", b"")
    )


def write_png_rgb(path, width, height, raw_rgb):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(encode_png_rgb(width, height, raw_rgb))


# ---------------------------------------------------------------------------
# 色板：夜空 → 天光 → 月，锚点对齐美术色板 token
# ---------------------------------------------------------------------------

# 锚点顺序 = 从最暗到最亮，取自原图夜空/天光的实际色标（tools/gen_title_bg.py 的 SKY_STOPS），
# 两端补近山墨色与美术色板的 paper 月心/月缘；这样"像素化"既保留原图夜色，
# 又让月亮/星点落在 UI 色板 token 上。
PALETTE_ANCHORS = [
    (0x09, 0x0C, 0x15),  # 近山/画面底：原图 (9,12,21)
    (0x0E, 0x14, 0x2A),  # 夜空顶：原图 (14,20,42)
    (0x1E, 0x28, 0x42),  # y=210 (30,40,66) 与顶之间的过渡
    (0x3A, 0x48, 0x68),  # y=360 (58,72,104)
    (0x5C, 0x6C, 0x8C),  # y=470 (92,108,142)
    (0x6C, 0x7A, 0x9C),  # y=560 天光最亮 (108,122,156)
    (0x8E, 0x96, 0xB0),  # 月晕过渡
    (0xB4, 0xAD, 0xA0),  # 月晕 → 月缘
    (0xD8, 0xCD, 0xB2),  # paper/200：月缘
    (0xF2, 0xEA, 0xD8),  # paper/50：月心 / 星
]

INK = (0x09, 0x0C, 0x15)
MOON_CORE = (0xF2, 0xEA, 0xD8)   # paper/50
MOON_RIM = (0xD8, 0xCD, 0xB2)    # paper/200
STARS = [(0xF2, 0xEA, 0xD8), (0xD8, 0xCD, 0xB2), (0x7F, 0xA3, 0xD4), (0xA8, 0x9F, 0x8A)]

# 4×4 Bayer 有序抖动矩阵（0..15）
BAYER4 = [
    [0, 8, 2, 10],
    [12, 4, 14, 6],
    [3, 11, 1, 9],
    [15, 7, 13, 5],
]

# 月亮几何（与 tools/gen_title_bg.py 的 MOON 常量一致）
MOON_CENTER = (1004, 214)
MOON_RADIUS = 92
MOON_HALO = 205

# 权重化 RGB 距离（绿色权重更高，避免把天色错判成月晕）
COLOR_WEIGHTS = (2.0, 4.0, 3.0)


def build_palette(steps_per_segment=3):
    """锚点之间做线性插值，得到一条连续渐变梯（像素画的标准做法：ramp palette）。"""
    palette = []
    for index in range(len(PALETTE_ANCHORS) - 1):
        start = PALETTE_ANCHORS[index]
        end = PALETTE_ANCHORS[index + 1]
        for step in range(steps_per_segment):
            t = step / float(steps_per_segment)
            color = tuple(
                int(round(start[c] + (end[c] - start[c]) * t)) for c in range(3)
            )
            if color not in palette:
                palette.append(color)
    if PALETTE_ANCHORS[-1] not in palette:
        palette.append(PALETTE_ANCHORS[-1])
    return palette


def nearest_color(color, palette):
    best = None
    best_distance = None
    for candidate in palette:
        distance = (
            (color[0] - candidate[0]) ** 2 * COLOR_WEIGHTS[0]
            + (color[1] - candidate[1]) ** 2 * COLOR_WEIGHTS[1]
            + (color[2] - candidate[2]) ** 2 * COLOR_WEIGHTS[2]
        )
        if best_distance is None or distance < best_distance:
            best_distance = distance
            best = candidate
    return best


# ---------------------------------------------------------------------------
# 像素化
# ---------------------------------------------------------------------------


def bayer_offset(x, y):
    """返回 -0.5..0.5 的抖动偏移。"""
    return (BAYER4[y % 4][x % 4] + 0.5) / 16.0 - 0.5


def moon_class_grid(low_w, low_h, scale):
    """按像素网格判定每格是否落在月心/月缘：几何光栅化，得到干净的像素圆。

    用几何而不是"块内高亮占比"，是因为占比法会让紧贴月缘的格子偶尔越界，
    在圆边上留下 1 像素凸起；几何法直接得到对称的阶梯圆边。
    """
    core_radius = MOON_RADIUS - scale * 0.75
    rim_radius = MOON_RADIUS + scale * 0.75
    grid = bytearray(low_w * low_h)
    moon_x, moon_y = MOON_CENTER
    for ly in range(low_h):
        center_y = ly * scale + scale * 0.5
        for lx in range(low_w):
            center_x = lx * scale + scale * 0.5
            distance = ((center_x - moon_x) ** 2 + (center_y - moon_y) ** 2) ** 0.5
            if distance <= core_radius:
                grid[ly * low_w + lx] = 2
            elif distance <= rim_radius:
                grid[ly * low_w + lx] = 1
    return grid


def pixelate(width, height, channels, pixels, scale, dither, palette):
    low_w = width // scale
    low_h = height // scale
    out = bytearray(low_w * low_h * 3)
    star_lum = [-1] * (low_w * low_h)
    star_color = [None] * (low_w * low_h)
    moon_grid = moon_class_grid(low_w, low_h, scale)
    stride = width * channels
    moon_x, moon_y = MOON_CENTER
    star_radius = MOON_HALO + 25  # 原图星点也避开了这个范围（gen_title_bg.py 同规则）

    for ly in range(low_h):
        for lx in range(low_w):
            cell = ly * low_w + lx
            out_index = cell * 3

            # 1) 月亮：几何分阶（paper/50 月心 + paper/200 月缘）
            if moon_grid[cell] == 2:
                out[out_index:out_index + 3] = bytes(MOON_CORE)
                continue
            if moon_grid[cell] == 1:
                out[out_index:out_index + 3] = bytes(MOON_RIM)
                continue

            sum_r = sum_g = sum_b = 0
            star_pixels = []      # (亮度, 颜色) 候选星点
            histogram = {}
            min_ch = [255, 255, 255]
            max_ch = [0, 0, 0]
            for sy in range(scale):
                row = (ly * scale + sy) * stride
                for sx in range(scale):
                    index = row + (lx * scale + sx) * channels
                    r = pixels[index]
                    g = pixels[index + 1]
                    b = pixels[index + 2]
                    sum_r += r
                    sum_g += g
                    sum_b += b
                    for channel, value in enumerate((r, g, b)):
                        if value < min_ch[channel]:
                            min_ch[channel] = value
                        if value > max_ch[channel]:
                            max_ch[channel] = value
                    key = (r, g, b)
                    histogram[key] = histogram.get(key, 0) + 1
                    luminance = (r * 30 + g * 59 + b * 11) // 100
                    if luminance > 145:
                        # 只收集远离月亮的亮点，当作星点
                        px = lx * scale + sx
                        py = ly * scale + sy
                        if (px - moon_x) ** 2 + (py - moon_y) ** 2 > star_radius ** 2:
                            star_pixels.append((luminance, (r, g, b)))

            count = scale * scale

            # 2) 星点：降采样会吃掉 1–2px 星，这里在原位置重打单像素（后面统一去重）
            if star_pixels:
                star_pixels.sort(key=lambda item: item[0], reverse=True)
                luminance, color = star_pixels[0]
                star_lum[cell] = luminance
                star_color[cell] = nearest_color(color, palette + STARS)
                continue

            avg = (sum_r / count, sum_g / count, sum_b / count)

            # 3) 剪影/边缘格子：取主色且不抖动，保持山脊 1px 硬边
            spread = max(max_ch[c] - min_ch[c] for c in range(3))
            if spread > 18:
                mode = max(histogram.items(), key=lambda item: item[1])[0]
                out[out_index:out_index + 3] = bytes(nearest_color(mode, palette))
                continue

            # 4) 平滑区：抖动后量化，得到像素画的分层过渡
            offset = bayer_offset(lx, ly) * dither
            target = (
                avg[0] + offset,
                avg[1] + offset * 0.6,
                avg[2] + offset * 0.4,
            )
            out[out_index:out_index + 3] = bytes(nearest_color(target, palette))

    # 星点去重：相邻格子里较暗的一颗丢掉，避免一颗 2×2px 的星被放大成 2×2 个方块
    for ly in range(low_h):
        for lx in range(low_w):
            cell = ly * low_w + lx
            if star_lum[cell] < 0:
                continue
            keep = True
            for dy in (-1, 0, 1):
                for dx in (-1, 0, 1):
                    if dx == 0 and dy == 0:
                        continue
                    nx = lx + dx
                    ny = ly + dy
                    if 0 <= nx < low_w and 0 <= ny < low_h:
                        other = ny * low_w + nx
                        if star_lum[other] > star_lum[cell]:
                            keep = False
            if not keep:
                continue
            out_index = cell * 3
            out[out_index:out_index + 3] = bytes(star_color[cell])

    return low_w, low_h, bytes(out)


def upscale(low_w, low_h, low_rgb, scale):
    """最近邻整数放大：每个低分辨率像素变成 scale×scale 的方块。"""
    row_cache = {}
    out = bytearray(low_w * scale * low_h * scale * 3)
    out_stride = low_w * scale * 3
    for ly in range(low_h):
        base = ly * low_w * 3
        row = row_cache.get(ly)
        if row is None:
            parts = []
            for lx in range(low_w):
                parts.append(bytes(low_rgb[base + lx * 3:base + lx * 3 + 3]) * scale)
            row = b"".join(parts)
            row_cache[ly] = row
        for sy in range(scale):
            start = (ly * scale + sy) * out_stride
            out[start:start + out_stride] = row
    return bytes(out)


# ---------------------------------------------------------------------------
# 对比预览
# ---------------------------------------------------------------------------


def box_downscale(width, height, channels, pixels, factor):
    out_w = width // factor
    out_h = height // factor
    out = bytearray(out_w * out_h * 3)
    stride = width * channels
    for oy in range(out_h):
        for ox in range(out_w):
            sum_r = sum_g = sum_b = 0
            for sy in range(factor):
                row = (oy * factor + sy) * stride
                for sx in range(factor):
                    index = row + (ox * factor + sx) * channels
                    sum_r += pixels[index]
                    sum_g += pixels[index + 1]
                    sum_b += pixels[index + 2]
            count = factor * factor
            index = (oy * out_w + ox) * 3
            out[index] = sum_r // count
            out[index + 1] = sum_g // count
            out[index + 2] = sum_b // count
    return out_w, out_h, bytes(out)


def build_preview(src_w, src_h, src_channels, src_pixels, pixel_path, preview_path):
    """上半：原图；下半：像素化结果。均按 2× 缩小以便一屏对比。"""
    left_w, left_h, left = box_downscale(src_w, src_h, src_channels, src_pixels, 2)
    pw, ph, pch, ppix = decode_png(pixel_path)
    right_w, right_h, right = box_downscale(pw, ph, pch, ppix, 2)
    if (left_w, left_h) != (right_w, right_h):
        raise ValueError("preview panels differ in size")
    gap = 8
    out_w = left_w
    out_h = left_h * 2 + gap
    out = bytearray(b"\x1f\x23\x28" * (out_w * out_h))
    for y in range(left_h):
        out[y * out_w * 3:(y + 1) * out_w * 3] = left[y * left_w * 3:(y + 1) * left_w * 3]
        target = (left_h + gap + y) * out_w * 3
        out[target:target + out_w * 3] = right[y * right_w * 3:(y + 1) * right_w * 3]
    write_png_rgb(preview_path, out_w, out_h, bytes(out))
    return out_w, out_h


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

DEFAULT_SOURCE = os.path.join("assets", "art", "title", "_title_bg_source.png")
DEFAULT_OUT = os.path.join("assets", "art", "title", "title_bg_pixel.png")
DEFAULT_PREVIEW = os.path.join("assets", "art", "title", "title_bg_pixel_preview.png")


def color_report(low_w, low_h, low_rgb):
    palette_used = {}
    for index in range(0, len(low_rgb), 3):
        key = (low_rgb[index], low_rgb[index + 1], low_rgb[index + 2])
        palette_used[key] = palette_used.get(key, 0) + 1
    return palette_used


def main():
    parser = argparse.ArgumentParser(description="把标题背景像素化（限定色板 + 有序抖动）")
    parser.add_argument("--source", default=DEFAULT_SOURCE)
    parser.add_argument("--out", default=DEFAULT_OUT)
    parser.add_argument("--preview", default=DEFAULT_PREVIEW)
    parser.add_argument("--scale", type=int, default=4, help="整数倍降采样/放大倍数（默认 4）")
    parser.add_argument("--dither", type=float, default=22.0, help="抖动强度，0 关闭（默认 22）")
    parser.add_argument("--palette-steps", type=int, default=3, help="锚点之间的插值步数")
    args = parser.parse_args()

    width, height, channels, pixels = decode_png(args.source)
    if width % args.scale or height % args.scale:
        raise SystemExit(f"source {width}x{height} is not divisible by scale {args.scale}")
    palette = build_palette(args.palette_steps)
    low_w, low_h, low_rgb = pixelate(
        width, height, channels, pixels, args.scale, args.dither, palette
    )
    final = upscale(low_w, low_h, low_rgb, args.scale)
    write_png_rgb(args.out, width, height, final)
    used = color_report(low_w, low_h, low_rgb)
    preview_w, preview_h = build_preview(
        width, height, channels, pixels, args.out, args.preview
    )
    print(f"source      {width}x{height} ({channels}ch)")
    print(f"pixel art   {low_w}x{low_h} → ×{args.scale} → {width}x{height}")
    print(f"palette     {len(palette)} 色（锚点 {len(PALETTE_ANCHORS)} 段插值）")
    print(f"colors used {len(used)} / {len(palette)}")
    print(f"wrote       {args.out}")
    print(f"wrote       {args.preview} ({preview_w}x{preview_h}，上=原图 下=像素化)")


if __name__ == "__main__":
    main()
