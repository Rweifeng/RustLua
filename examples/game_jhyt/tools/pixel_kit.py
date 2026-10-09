# -*- coding: utf-8 -*-
"""《江湖有田》像素美术公共工具：画布 / PNG 编解码 / 预览文字。

被 tools/gen_manor_art.py、tools/preview_scene.py 使用；纯 Python 标准库实现
（zlib/struct），不需要 Pillow。

坐标约定（与引擎一致）：
  * 画布 (0,0) 在左上、y 向下，图像行序 = 引擎 UV 的 v（v=0 在图像顶部）；
  * 世界坐标 y 向上，tile 32 单位，cell (cx,cy) 覆盖世界 x∈[cx*32,(cx+1)*32]、y∈[cy*32,(cy+1)*32]。
"""

import struct
import zlib

RGBA = tuple


# ---------------------------------------------------------------------------
# 画布
# ---------------------------------------------------------------------------


class Canvas:
    """RGBA8 画布：像素级绘制 + 描边 + 贴图。"""

    __slots__ = ("w", "h", "px")

    def __init__(self, w, h, fill=(0, 0, 0, 0)):
        self.w = w
        self.h = h
        self.px = bytearray(bytes(fill) * (w * h))

    def set(self, x, y, color):
        x = int(x)
        y = int(y)
        if 0 <= x < self.w and 0 <= y < self.h:
            i = (y * self.w + x) * 4
            self.px[i:i + 4] = bytes(color)

    def get(self, x, y):
        if 0 <= x < self.w and 0 <= y < self.h:
            i = (y * self.w + x) * 4
            return tuple(self.px[i:i + 4])
        return (0, 0, 0, 0)

    def opaque(self, x, y):
        return self.get(x, y)[3] > 0

    def rect(self, x0, y0, x1, y1, color):
        for y in range(int(y0), int(y1) + 1):
            for x in range(int(x0), int(x1) + 1):
                self.set(x, y, color)

    def hline(self, x0, x1, y, color):
        self.rect(x0, y, x1, y, color)

    def vline(self, x, y0, y1, color):
        self.rect(x, y0, x, y1, color)

    def blit(self, other, dx=0, dy=0):
        for y in range(other.h):
            for x in range(other.w):
                color = other.get(x, y)
                if color[3] > 0:
                    self.set(dx + x, dy + y, color)

    def outline(self, color=(0x1F, 0x23, 0x28, 255)):
        """给不透明区域外描一圈 1px 轮廓（4 邻域）——全项目统一的像素画描边。"""
        edges = []
        for y in range(self.h):
            for x in range(self.w):
                if self.get(x, y)[3] > 0:
                    continue
                for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    if self.opaque(x + dx, y + dy):
                        edges.append((x, y))
                        break
        for x, y in edges:
            self.set(x, y, color)

    def scaled(self, factor):
        out = Canvas(self.w * factor, self.h * factor)
        for y in range(self.h):
            for x in range(self.w):
                color = self.get(x, y)
                if color[3] == 0:
                    continue
                for sy in range(factor):
                    for sx in range(factor):
                        out.set(x * factor + sx, y * factor + sy, color)
        return out

    def mirrored(self):
        """水平镜像（w 朝向由 e 镜像生成，避免左右手画两遍）。"""
        out = Canvas(self.w, self.h)
        for y in range(self.h):
            for x in range(self.w):
                out.set(self.w - 1 - x, y, self.get(x, y))
        return out

    def sliced(self, x0, y0, x1, y1):
        """按闭区间裁一块。**宽度/x 原点必须留住**：M72-06 把树拆成树干+树冠两块时，
        两块共用同一个 x 原点才能靠 Transform 差一个 y 严丝合缝对回去。"""
        out = Canvas(x1 - x0 + 1, y1 - y0 + 1)
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                out.set(x - x0, y - y0, self.get(x, y))
        return out

    def to_png(self):
        raw = bytearray()
        stride = self.w * 4
        for y in range(self.h):
            raw.append(0)
            raw += self.px[y * stride:(y + 1) * stride]
        return encode_png(self.w, self.h, bytes(raw))


def encode_png(width, height, raw_rgba):
    """RGBA8（color type 6）PNG。"""
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + _chunk(b"IHDR", ihdr)
        + _chunk(b"IDAT", zlib.compress(raw_rgba, 9))
        + _chunk(b"IEND", b"")
    )


def _chunk(tag, body):
    return (
        struct.pack(">I", len(body))
        + tag
        + body
        + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)
    )


def _paeth(a, b, c):
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    if pb <= pc:
        return b
    return c


def decode_png(path):
    """返回 (width, height, channels, bytes)；支持 8bit 非隔行 RGB/RGBA。"""
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
        raise ValueError(f"unsupported color type {color_type}")
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
            raise ValueError(f"unknown filter {filter_type}")
        out[y * stride:(y + 1) * stride] = line
        prev = line
    return width, height, channels, bytes(out)


def load_canvas(path):
    """把 PNG 读成 Canvas（忽略 alpha 之外的通道差异）。"""
    width, height, channels, pixels = decode_png(path)
    canvas = Canvas(width, height)
    for y in range(height):
        for x in range(width):
            i = (y * width + x) * channels
            if channels == 4:
                canvas.set(x, y, (pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]))
            else:
                canvas.set(x, y, (pixels[i], pixels[i + 1], pixels[i + 2], 255))
    return canvas


# ---------------------------------------------------------------------------
# 颜色
# ---------------------------------------------------------------------------


def rgb(value):
    value = value.lstrip("#")
    return (int(value[0:2], 16), int(value[2:4], 16), int(value[4:6], 16), 255)


def shade(color, factor):
    return (
        max(0, min(255, int(color[0] * factor))),
        max(0, min(255, int(color[1] * factor))),
        max(0, min(255, int(color[2] * factor))),
        255,
    )


def mix(a, b, t):
    return (
        int(a[0] + (b[0] - a[0]) * t),
        int(a[1] + (b[1] - a[1]) * t),
        int(a[2] + (b[2] - a[2]) * t),
        255,
    )


def hash_stream(seed):
    """确定性伪随机（跨平台稳定，不用 random 模块以保证可复现）。"""
    state = (seed * 2654435761) & 0xFFFFFFFF

    def next_int(bound):
        nonlocal state
        state = (state * 1103515245 + 12345) & 0x7FFFFFFF
        return (state >> 7) % bound

    return next_int


# ---------------------------------------------------------------------------
# 贴图合成（UI 预览用，规则与引擎一致）
# ---------------------------------------------------------------------------


def blit_scaled(dst, src, sx, sy, sw, sh, dx, dy, dw, dh):
    """把 src 的 (sx,sy,sw,sh) 区域最近邻缩放到 (dx,dy,dw,dh)。"""
    if sw <= 0 or sh <= 0 or dw <= 0 or dh <= 0:
        return
    for oy in range(int(dh)):
        ysrc = int(sy + min(sh - 1, int(oy * sh / dh)))
        for ox in range(int(dw)):
            xsrc = int(sx + min(sw - 1, int(ox * sw / dw)))
            color = src.get(xsrc, ysrc)
            if color[3] > 0:
                dst.set(int(dx) + ox, int(dy) + oy, color)


def draw_nine_slice(dst, texture, dx, dy, dw, dh, border):
    """九宫格贴图：border = [left, right, top, bottom]（纹理像素，与引擎同序）。

    四角 1:1 不缩放，四边单轴拉伸，中心双轴拉伸；越界/退化情况直接跳过，
    保证与 core/ui.rs 的 nine_slice_cells 行为一致。
    """
    left, right, top, bottom = (int(value) for value in border)
    src_w, src_h = texture.w, texture.h
    if min(dw, dh) <= 0 or src_w <= 0 or src_h <= 0:
        return
    # 目标尺寸小于角块之和时，按比例压缩角块（引擎同样做 clamp）
    scale_x = min(1.0, dw / max(1, left + right))
    scale_y = min(1.0, dh / max(1, top + bottom))
    cl = int(left * scale_x)
    cr = int(right * scale_x)
    ct = int(top * scale_y)
    cb = int(bottom * scale_y)
    columns = [
        (0, left, dx, cl),
        (left, src_w - right, dx + cl, dw - cl - cr),
        (src_w - right, src_w, dx + dw - cr, cr),
    ]
    rows = [
        (0, top, dy, ct),
        (top, src_h - bottom, dy + ct, dh - ct - cb),
        (src_h - bottom, src_h, dy + dh - cb, cb),
    ]
    for sy0, sy1, ty, th in rows:
        for sx0, sx1, tx, tw in columns:
            blit_scaled(dst, texture, sx0, sy0, sx1 - sx0, sy1 - sy0, tx, ty, tw, th)


# ---------------------------------------------------------------------------
# 预览用 3×5 点阵字
# ---------------------------------------------------------------------------

FONT = {
    "A": ["010", "101", "111", "101", "101"],
    "B": ["110", "101", "110", "101", "110"],
    "C": ["011", "100", "100", "100", "011"],
    "D": ["110", "101", "101", "101", "110"],
    "E": ["111", "100", "110", "100", "111"],
    "F": ["111", "100", "110", "100", "100"],
    "G": ["011", "100", "101", "101", "011"],
    "H": ["101", "101", "111", "101", "101"],
    "I": ["111", "010", "010", "010", "111"],
    "J": ["001", "001", "001", "101", "010"],
    "K": ["101", "101", "110", "101", "101"],
    "L": ["100", "100", "100", "100", "111"],
    "M": ["101", "111", "111", "101", "101"],
    "N": ["101", "111", "111", "111", "101"],
    "O": ["010", "101", "101", "101", "010"],
    "P": ["110", "101", "110", "100", "100"],
    "Q": ["010", "101", "101", "111", "011"],
    "R": ["110", "101", "110", "101", "101"],
    "S": ["011", "100", "010", "001", "110"],
    "T": ["111", "010", "010", "010", "010"],
    "U": ["101", "101", "101", "101", "011"],
    "V": ["101", "101", "101", "101", "010"],
    "W": ["101", "101", "111", "111", "101"],
    "X": ["101", "101", "010", "101", "101"],
    "Y": ["101", "101", "010", "010", "010"],
    "Z": ["111", "001", "010", "100", "111"],
    "0": ["111", "101", "101", "101", "111"],
    "1": ["010", "110", "010", "010", "111"],
    "2": ["111", "001", "111", "100", "111"],
    "3": ["111", "001", "111", "001", "111"],
    "4": ["101", "101", "111", "001", "001"],
    "5": ["111", "100", "111", "001", "111"],
    "6": ["111", "100", "111", "101", "111"],
    "7": ["111", "001", "001", "001", "001"],
    "8": ["111", "101", "111", "101", "111"],
    "9": ["111", "101", "111", "001", "111"],
    "_": ["000", "000", "000", "000", "111"],
    "-": ["000", "000", "111", "000", "000"],
    ".": ["000", "000", "000", "000", "010"],
    " ": ["000", "000", "000", "000", "000"],
}


def text_width(text, scale=1):
    return max(0, len(text) * (3 * scale + scale) - scale)


# ---------------------------------------------------------------------------
# 场景 Transform 组件（统一形态）
# ---------------------------------------------------------------------------


def transform_line(entity_id, x=0.0, y=0.0, rotation=0.0, scale_x=1.0, scale_y=1.0):
    """一行**统一形态**的 `Transform` 组件（`position=..;rotation=..;scale=..`）。

    为什么必须用统一形态：引擎的 `Transform` 读侧只认这一种写法
    （`core/spatial_transform.rs` 的 `LocalTransform::from_unified_component_str`），
    2D 旧形态（`x=..;y=..;rotation=..;scale_x=..;scale_y=..`）**已随旧组件名一起删除**。
    仓库里还有一条数据门测试 `shipped_data_uses_only_unified_components`：场景里只要写回
    旧形态就立刻判红。此前 `tools/gen_*_scene.py` 一直写旧形态——旧场景文件是当时用
    一次性工具（`editor --example migrate_project`）迁过的，所以生成器与盘上场景**早就
    不一致**：重跑一次生成器就会把旧形态写回去，然后启动即失败。

    形态换算（2D 视图 → 统一 3D 形态，与迁移工具同一套口径）：
      * `z = 0`；
      * 绕 Z 轴旋转 `r` 的四元数 = `(0, 0, sin(r/2), cos(r/2))`；
      * `scale_z = 1`。
    数值一律 9 位小数、负零保留符号（`-0.000000000`）——与引擎 `to_component_str` 逐字节一致，
    否则"生成器产物 vs 引擎回写产物"对拍会出现假差异。
    """
    import math

    half = rotation / 2.0
    rz = math.sin(half)
    rw = math.cos(half)
    return (
        f"component\t{entity_id}\tTransform\t"
        f"position={x:.9f},{y:.9f},0.000000000;"
        f"rotation=0.000000000,0.000000000,{rz:.9f},{rw:.9f};"
        f"scale={scale_x:.9f},{scale_y:.9f},1.000000000"
    )

# ---------------------------------------------------------------------------
# 场景 2D 相机组件（统一 `Camera`，`projection=orthographic`）
# ---------------------------------------------------------------------------


def camera_line(entity_id, map_width, map_height, tile=32, zoom=2.0):
    """一行**正交 2D 相机**组件，把取景范围锁在地图矩形内。

    为什么每一张玩法地图都要有它：横版设计分辨率 1280×720，室外地图 40×23 格 =
    1280×736 世界单位（室内/洞穴 20×12 = 640×384，两轴都比视口小）。没有相机组件时
    运行期用默认中心 `(640,360)`/`zoom=1`/无 bounds：角色走到哪儿都看不出跟随，
    多出来的视口范围会把地图外的空白画出来。

    `zoom=2.0` 是对标星露谷的视觉口径（docs/51 §2.2 + docs/52 §4 修订）：
      * 屏幕 tile = 32 × 2 = 64px，与星露谷 16px tile 在 4× 缩放下同规格；
      * 可见域 = 1280×720 / 2 = 640×360 世界单位 = 20×11.25 格，与星露谷一屏一致；
      * 角色帧 64×64 在 zoom=2 下 = 128px 高，等于星露谷角色的屏幕尺寸；
      * 室外 40×23 格因此两轴都可被相机跟随（不再是整图摊一屏）。

    引擎口径（`core/camera.rs`）：
      * `bounds=min_x,min_y,max_x,max_y` 让 `Camera2D::constrained_center` 把**相机中心**
        夹到 `[min+半视口, max-半视口]`（半视口 = 视口/2/zoom，随 zoom 缩放）；
      * 地图在某轴上比视口还小时该轴上下界反转，引擎按"两条界的**中点**"取值
        （`clamp_center`），于是那根轴**钉在地图中心**——室内图 640×384 的 y 轴
        （384 < 360×2 全屏时 y 可夹取，x 轴 640 ≈ 视口 640 钉中心）。
      * 夹取每帧在渲染前应用（`Camera2dController::resolve_camera`），因此 Lua 的
        `camera2d.follow` 负责"想跟谁"，场景的 bounds 负责"最多跟到哪儿"。
    """
    # 世界坐标覆盖 [0, map_width*tile] × [0, map_height*tile]；中心给地图中心，
    # 第一帧（脚本还没 follow 之前）就不会先跳到默认的 (640,360) 再弹回来。
    max_x = (map_width * tile) / 2.0
    max_y = (map_height * tile) / 2.0
    return (
        f"component\t{entity_id}\tCamera\tprojection=orthographic"
        f";center={max_x:.4f},{max_y:.4f}"
        f";zoom={zoom:.6f}"
        f";bounds=0.0000,0.0000,{map_width * tile:.4f},{map_height * tile:.4f}"
    )


def draw_text(canvas, x, y, text, color=(0xE4, 0xC0, 0x78, 255), scale=1):
    cursor = x
    for char in text.upper():
        glyph = FONT.get(char, FONT[" "])
        for gy, row in enumerate(glyph):
            for gx, bit in enumerate(row):
                if bit == "1":
                    for sy in range(scale):
                        for sx in range(scale):
                            canvas.set(cursor + gx * scale + sx, y + gy * scale + sy, color)
        cursor += 3 * scale + scale
    return cursor
