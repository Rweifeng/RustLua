# -*- coding: utf-8 -*-
"""《江湖有田》开始画面背景生成（月夜 · 水墨远山）

一次性产物：assets/art/title/title_bg.png（1280×720，RGB 不透明，不含任何文字）。
可重跑微调参数；工程原创美术（保留脚本便于复现），登记见
licenses/ASSET_REGISTER.tsv（asset_id=title_bg_png）。

构图（克制、分区清晰，避免大面积雾/云强横带）：
  上部夜空(顶深靛 → 向下渐亮，承接月光) + 星 + 右上柔月(带淡晕)
  中段为天光过渡(按钮区背景：柔和偏亮、干净)
  下部整层远山(深蓝灰、锯齿起伏，只占画面下部)
  最前景近山(最暗、贴左右收角，仅在边缘点两个峰头，不与中央 UI 冲突)
中央 y≈110-560 干净，作为运行时 UI（标题大字/按钮/印章）落点。

依赖：Python 3 + Pillow。运行：工程根目录  python tools/gen_title_bg.py
"""
import math
import os
import random

from PIL import Image, ImageDraw

W, H = 1280, 720
SEED = 7

# ---- 夜空（y, rgb）：顶深靛 -> 地平天光（按钮区落在 y≈360-560 天光上）----
SKY_STOPS = [
    (0, (14, 20, 42)),
    (210, (30, 40, 66)),
    (360, (58, 72, 104)),
    (470, (92, 108, 142)),
    (560, (108, 122, 156)),
    (600, (60, 72, 104)),
    (720, (12, 15, 24)),
]

# ---- 圆月（右上，柔晕克制）----
MOON = {"center": (1004, 214), "radius": 92, "core": (240, 234, 216),
        "halo_radius": 205, "halo_power": 0.42}

# ---- 远山整层：每座山高斯起伏成脊，向下填到画面底；中央压低于按钮下方，两侧起峰 ----
FAR_COLOR = (22, 30, 50)
# (cx, sigma, 峰顶y)
FAR_PEAKS = [
    (0, 170, 600), (120, 150, 540), (260, 175, 570), (400, 160, 610),
    (560, 175, 630), (720, 185, 620), (860, 160, 600), (1000, 155, 545),
    (1130, 150, 560), (1290, 175, 615),
]

# ---- 近山（最暗，左右收角，仅边缘，不与中央 UI 冲突）----
NEAR_COLOR = (9, 12, 21)
NEAR_PEAKS = [
    (70, 180, 520), (1180, 190, 530),
]

# ---- 底部微收暗 ----
BOTTOM_DARK = {"y0": 688, "target": (6, 8, 14)}

OUT = os.path.join("assets", "art", "title", "title_bg.png")


def lerp(a, b, t):
    t = min(max(t, 0.0), 1.0)
    return tuple(int(round(a[i] + (b[i] - a[i]) * t)) for i in range(3))


def mountain_silhouette(draw, color, peaks):
    """一堆高斯山丘的并集剪影：逐列取最靠上(小y)的峰，向下填到画面底。"""
    pts = []
    step = 4
    for x in range(0, W + 1, step):
        top = None
        for (cx, sigma, peak_y) in peaks:
            d = (x - cx) / sigma
            if d * d > 9.0:
                continue
            # 高斯脊：峰顶向下弯曲成圆润山脊
            y = peak_y + (H - peak_y) * (1.0 - math.exp(-0.5 * d * d))
            y += 3.0 * math.sin(x / 87.0 + cx * 0.013)
            top = y if top is None else min(top, y)
        if top is None:
            continue
        pts.append((x, top))
    draw.polygon(pts + [(W, H), (0, H)], fill=color)


def build():
    random.seed(SEED)
    img = Image.new("RGB", (W, H))
    dr = ImageDraw.Draw(img)

    # 1) 夜空渐变
    for y in range(H):
        color = SKY_STOPS[0][1]
        for i in range(len(SKY_STOPS) - 1):
            y0, c0 = SKY_STOPS[i]
            y1, c1 = SKY_STOPS[i + 1]
            if y <= y1:
                color = lerp(c0, c1, (y - y0) / (y1 - y0) if y1 > y0 else 0.0)
                break
        dr.line([(0, y), (W, y)], fill=color)

    # 2) 星点（上部，避开月晕）
    for _ in range(130):
        x = random.randrange(0, W)
        y = random.randrange(0, 320)
        if math.hypot(x - MOON["center"][0], y - MOON["center"][1]) < MOON["halo_radius"] + 16:
            continue
        s = random.choice([1, 1, 1, 2])
        shade = random.choice([(170, 184, 210), (222, 230, 246), (130, 144, 172)])
        dr.rectangle([x, y, x + s - 1, y + s - 1], fill=shade)

    # 3) 圆月 + 淡外晕（柔和、不刺眼）
    cx, cy = MOON["center"]
    rc, rh, power, core = MOON["radius"], MOON["halo_radius"], MOON["halo_power"], MOON["core"]
    for y in range(cy - rh, cy + rh + 1):
        for x in range(cx - rh, cx + rh + 1):
            d = math.hypot(x - cx, y - cy)
            if d > rh:
                continue
            base = img.getpixel((x, y))
            if d <= rc:
                if d < rc - 6:
                    col = core
                else:
                    col = lerp(core, base, (rc - d) / 6.0)
            else:
                col = lerp(base, core, (1.0 - (d - rc) / (rh - rc)) * power)
            img.putpixel((x, y), col)

    # 4) 远山整层（占下部）
    mountain_silhouette(dr, FAR_COLOR, FAR_PEAKS)

    # 5) 近山（最暗，左右收角）
    mountain_silhouette(dr, NEAR_COLOR, NEAR_PEAKS)

    # 6) 底部微收暗
    y0d = BOTTOM_DARK["y0"]
    for y in range(y0d, H):
        t = (y - y0d) / (H - y0d)
        dr.line([(0, y), (W, y)], fill=lerp(img.getpixel((0, y)), BOTTOM_DARK["target"], t))

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    img.convert("RGB").save(OUT)
    print("wrote", OUT, img.size)


if __name__ == "__main__":
    build()
