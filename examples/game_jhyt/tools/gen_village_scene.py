# -*- coding: utf-8 -*-
"""《江湖有田》云泽村灰盒场景生成（scenes/village.scene）

用法：工程根目录 python tools/gen_village_scene.py

地表复用 assets/art/greybox/manor_ground.png（6 列灰盒调色）：
  1=草地 2=夯土 3=石板路 4=院墙 5=青瓦(屋顶) 6=水(池塘)
布局（cell 0-index，row 0 在底、y 朝上；世界坐标 = (cx+0.5)*32）：
  南入口与主街（stone）在中下部；西侧药庐、东侧杂货铺为不可走入的 roof 块；
  北侧中央一个小池塘（water）。NPC/商店实体运行期由 Lua 数据模块生成，
  场景文件只声明地表 + 玩家 + HUD，保持纯数据驱动。
"""
import os

W, H = 40, 22
TILE = 32

GRASS, DIRT, STONE, WALL, ROOF, WATER = 1, 2, 3, 4, 5, 6


def build_tiles():
    ground = [[GRASS] * W for _ in range(H)]
    # 外圈院墙
    for x in range(W):
        ground[0][x] = WALL
        ground[H - 1][x] = WALL
    for y in range(H):
        ground[y][0] = WALL
        ground[y][W - 1] = WALL
    # 南入口 + 主街（stone），直通中央广场
    for y in range(1, 10):
        for x in range(16, 24):
            ground[y][x] = STONE
    # 中央横向广场
    for x in range(10, 30):
        ground[9][x] = STONE
    # 西侧药庐（不可走入）
    for y in range(11, 21):
        for x in range(2, 10):
            ground[y][x] = ROOF
    for x in range(2, 10):
        ground[10][x] = STONE  # 门前石阶
    # 东侧杂货铺（不可走入）
    for y in range(11, 21):
        for x in range(30, 38):
            ground[y][x] = ROOF
    for x in range(30, 38):
        ground[10][x] = STONE  # 门前石阶
    # 北侧中央池塘（不可走入）
    for y in range(16, 18):
        for x in range(18, 22):
            ground[y][x] = WATER
    return ground


def build_solid(ground):
    sol = []
    for y in range(H):
        row = []
        for x in range(W):
            v = ground[y][x]
            row.append(1 if v in (ROOF, WATER, WALL) else 0)
        sol.append(row)
    return sol


def row_major(grid):
    return "".join(str(grid[y][x]) for y in range(H) for x in range(W))


def cell_to_world(cx, cy):
    return (cx + 0.5) * TILE, (cy + 0.5) * TILE


def build_scene(ground, solid):
    # 南入口（主街起点）为村庄默认出生/返回山庄的出口点
    spawn_wx, spawn_wy = cell_to_world(19, 2)
    lines = [
        "scene_version=2",
        "name=village",
        "meta\tdisplay_name\t江湖有田·云泽村",
        "meta\tgame\tjianghu_youtian",
        "meta\ttile_size\t32",
        "meta\tui_reference_height\t720",
        "meta\tui_reference_width\t1280",
        "entity\t1\tGroundMap",
        "component\t1\tTransform\tx=0.000000000;y=0.000000000;rotation=0.000000000;scale_x=1.000000000;scale_y=1.000000000",
        "component\t1\tTilemap\ttile_size=32x32;width={0};height={1};texture=assets/art/greybox/manor_ground.png;tiles={2};solid={3}".format(
            W, H, row_major(ground), row_major(solid)
        ),
        "entity_meta\t1\trole\tground_tilemap",
        "entity\t2\tJianghuPlayer",
        "component\t2\tTransform\tx={0:.6f};y={1:.6f};rotation=0.000000000;scale_x=1.000000000;scale_y=1.000000000".format(
            spawn_wx, spawn_wy
        ),
        "component\t2\tSprite\ttexture=__white__;size=28x28;tint=0.949,0.918,0.847,1.000",
        "component\t2\tCollider\tshape=ball;radius=12",
        "entity_meta\t2\trole\tplayer_spawn",
        "entity_meta\t2\tpersistent\ttrue",
        "entity\t3\tHudTitle",
        "component\t3\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;offsets=24.00,20.00,640.00,44.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000",
        "component\t3\tText\ttext=云泽村 · 南市街;font_size=18.00;color=0.949,0.918,0.847",
        "component\t3\tTransform\tx=0.000000000;y=0.000000000;rotation=0.000000000;scale_x=1.000000000;scale_y=1.000000000",
        "entity_meta\t3\trole\thud_title",
        "entity\t4\tHudHint",
        "component\t4\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;offsets=24.00,686.00,1000.00,708.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000",
        "component\t4\tText\ttext=WASD 移动 · E 交互(对话/商店/出口) · T 睡觉 · Q 吐纳;font_size=14.00;color=0.850,0.900,1.000",
        "component\t4\tTransform\tx=0.000000000;y=0.000000000;rotation=0.000000000;scale_x=1.000000000;scale_y=1.000000000",
        "entity_meta\t4\trole\thud_hint",
    ]
    return "\n".join(lines) + "\n"


def main():
    ground = build_tiles()
    solid = build_solid(ground)
    text = build_scene(ground, solid)
    out = os.path.join("scenes", "village.scene")
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print("wrote", out, f"({W}x{H})")


if __name__ == "__main__":
    main()
