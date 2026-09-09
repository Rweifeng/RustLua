# -*- coding: utf-8 -*-
"""《江湖有田》山庄灰盒 tileset 生成（地表 + 耕地 overlay）

一次性产物：
  assets/art/greybox/manor_ground.png   （地表 6 列）
  assets/art/greybox/manor_farm.png     （耕地/作物 6 列）

引擎 Tilemap 采样约定（见 engine core/tilemap.rs tile_uv）：
  tile 值 0 = 空；1..6 = tileset 第 N 列；UV-U 由 (N-1)/6..N/6 归一化截取；
  V 方向整幅 0..1 采样。因此纹理宽度=列数*像素宽(6*32=192)、高度任意
  （用两格高度 64 保证 V 采样在两格高下不把行间差异放大太多）。
  单格世界单位 32（scene meta tile_size=32），灰盒阶段用 32px 像素化
  tileset 直接 1:1 映射，观感朴素、清晰可辨。

列映射（game_jhyt 侧约定，farm.lua / main.scene 共用）：
  manor_ground:  1=草地  2=夯土  3=石板路  4=院墙  5=青瓦屋檐  6=水(池塘)
  manor_farm:    1=已耕地(翻土)  2=已耕地+浇水(深色)  3=苗期(stage1)
                 4=生长期(stage2)  5=成熟(stage3)  6=保留
  耕地 overlay 只在地块格上画 tile（其余为 0），配合 solid 阻挡即可。

依赖：Python 3 + Pillow。运行：工程根目录  python tools/gen_greybox_tiles.py
"""
import os

from PIL import Image, ImageDraw

OUT_DIR = os.path.join("assets", "art", "greybox")
TILE = 32
ROWS = 2  # 高度任意；留两格高度，避免单行在 V 采样时边界糊

# ---- 单列基色（稍带灰度渐变，便于辨认同属地表） ----
GROUND_COLORS = [
    (92, 128, 84),     # 1 草地 green
    (124, 92, 58),     # 2 夯土 tan
    (146, 140, 128),   # 3 石板路 grey
    (110, 84, 62),     # 4 院墙 brown
    (96, 66, 54),      # 5 青瓦 deep brown
    (66, 108, 142),    # 6 水 blue
]
FARM_COLORS = [
    (106, 76, 48),     # 1 已耕地（翻土深棕）
    (86, 64, 44),      # 2 已耕地+浇水（更深、带湿感）
    (108, 134, 74),    # 3 苗期（小嫩芽，绿）
    (74, 116, 62),     # 4 生长期（更大更绿）
    (150, 128, 52),    # 5 成熟（金黄稻穗/灵芝意象金黄褐）
    (170, 160, 150),   # 6 保留
]


def checker(draw, x0, y0, w, h, color, step=8):
    """简单棋盘噪点，让灰盒有像素质感而不刺眼。"""
    for y in range(0, h, step):
        for x in range(0, w, step):
            if (x // step + y // step) % 2 == 0:
                draw.rectangle([x0 + x, y0 + y, x0 + x + step - 1, y0 + y + step - 1], fill=color)


def speckle(img, base, color, count, spread=2):
    """叠加散布噪点，进一步打破平板色块。"""
    from random import Random

    rng = Random(hash((base, color)))
    px = img.load()
    w, h = img.size
    for _ in range(count):
        x = rng.randrange(w)
        y = rng.randrange(h)
        dx = rng.randint(-spread, spread)
        dy = rng.randint(-spread, spread)
        if 0 <= x + dx < w and 0 <= y + dy < h:
            px[x, y] = color


def make_strip(colors, speckles, name):
    img = Image.new("RGB", (TILE * 6, TILE * ROWS))
    dr = ImageDraw.Draw(img)
    for col, color in enumerate(colors):
        x0 = col * TILE
        for r in range(ROWS):
            y0 = r * TILE
            shade = 1.0 - r * 0.06
            base = tuple(int(c * shade) for c in color)
            dr.rectangle([x0, y0, x0 + TILE - 1, y0 + TILE - 1], fill=base)
        checker(dr, x0, 0, TILE, TILE * ROWS, tuple(int(c * 0.82) for c in color), step=16)
    for (bx, by), color in speckles:
        # 撒点集中在少数列内，避免整幅过花
        speckle(img, None, color, 14, 2)
    os.makedirs(OUT_DIR, exist_ok=True)
    out = os.path.join(OUT_DIR, name)
    img.save(out)
    print("wrote", out, img.size)


def build():
    make_strip(
        GROUND_COLORS,
        [((2, 0), (40, 52, 40)), ((5, 1), (34, 58, 78))],
        "manor_ground.png",
    )
    make_strip(
        FARM_COLORS,
        [((2, 0), (30, 42, 28)), ((5, 1), (80, 72, 30))],
        "manor_farm.png",
    )


if __name__ == "__main__":
    build()
