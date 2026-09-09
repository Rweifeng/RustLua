# -*- coding: utf-8 -*-
"""《江湖有田》山庄灰盒主场景生成（scenes/main.scene）

用法：工程根目录 python tools/gen_manor_scene.py

引擎 Tilemap 组件约定（scene_version=2）：
  tile_size=WxH;width=W;height=H;texture=PATH;tiles=<row-major 0..6>;solid=<row-major 0/1>
  单格 32 世界单位，y 朝上；索引 index = y*width+x（row-major，y 为行）。
tile 列映射（与本工具常量一致，farm.lua 侧同名约定）：
  manor_ground：1=草地 2=夯土 3=石板路 4=院墙 5=青瓦(农舍) 6=水(池塘)
  manor_farm：  1=已耕 2=已耕+已浇水 3=苗期 4=生长期 5=成熟（0=空）

布局：农舍/晒谷场/药田埂/池塘全画进 GroundMap；FarmGrid 覆盖同一坐标系的
农田格区域，scene 里初始全 0，运行期由 farm.lua 按地块逻辑写 tile/meta。
Farm 区域（x0..x1,y0..y1，含）也在本工具写死一份供 farm.lua 引用。
"""
import os

# 地图尺寸
W, H = 40, 22

# 列映射（写 scene tile 用）
GRASS, DIRT, STONE, WALL, ROOF, WATER = 1, 2, 3, 4, 5, 6

# ---- 分区（世界坐标 cell，y 朝上：row 0 在最底/屏幕下方，y 增大朝上）----
# 农舍块（内部不可走，画青瓦）  行 3..11（下方留 1..2 做屋前院）
HOUSE = {"x0": 2, "x1": 8, "y0": 3, "y1": 11}
# 屋前石板院（农舍南侧，可走）
YARD = {"x0": 2, "x1": 10, "y0": 1, "y1": 2}
# 药田（FarmGrid 区域，玩家播种核心区）
FARM = {"x0": 14, "x1": 33, "y0": 4, "y1": 13}
# 池塘（水，不可走）右下角
POND = {"x0": 34, "x1": 38, "y0": 4, "y1": 9}
# 石板路：从院东侧横向通到田南侧（y=3 一行为“田前路”）
PATH_Y = 3
PATH = {"x0": 11, "x1": 33}
# 玩家出生格（田南侧田前路上，朝北即面对药田）
SPAWN = {"x": 16, "y": PATH_Y}


def inside(c, x0, x1, y0, y1):
    return x0 <= c[0] <= x1 and y0 <= c[1] <= y1


def build_tiles():
    """地表 tile 值矩阵 tiles[y][x]"""
    ground = [[GRASS] * W for _ in range(H)]
    # 农舍本体（先画，下面路/院不会覆盖它：分区互不重叠）
    for y in range(HOUSE["y0"], HOUSE["y1"] + 1):
        for x in range(HOUSE["x0"], HOUSE["x1"] + 1):
            ground[y][x] = ROOF
    # 屋前石板院
    for y in range(YARD["y0"], YARD["y1"] + 1):
        for x in range(YARD["x0"], YARD["x1"] + 1):
            ground[y][x] = STONE
    # 药田整块夯土底（可走；FarmGrid 层会再画耕地/作物）
    for y in range(FARM["y0"], FARM["y1"] + 1):
        for x in range(FARM["x0"], FARM["x1"] + 1):
            ground[y][x] = DIRT
    # 池塘
    for y in range(POND["y0"], POND["y1"] + 1):
        for x in range(POND["x0"], POND["x1"] + 1):
            ground[y][x] = WATER
    # 田前石板路（从院东侧横向直通田南侧）
    for x in range(PATH["x0"], PATH["x1"] + 1):
        ground[PATH_Y][x] = STONE
    # 全图围墙：底边 row0、顶边 row21、左右边缘
    for x in range(W):
        ground[0][x] = WALL
        ground[H - 1][x] = WALL
    for y in range(H):
        ground[y][0] = WALL
        ground[y][W - 1] = WALL
    return ground


def build_solid(ground):
    """solid 布尔矩阵 solid[y][x]：阻挡行走。
    规则：瓦/水/墙阻挡；草地/夯土/石板可走。"""
    sol = []
    for y in range(H):
        row = []
        for x in range(W):
            v = ground[y][x]
            row.append(1 if v in (ROOF, WATER, WALL) else 0)
        sol.append(row)
    return sol


def cell_to_world(cx, cy):
    """cell 中心世界坐标（y 朝上，行 0 在最底）"""
    return (cx + 0.5) * 32.0, (cy + 0.5) * 32.0


def row_major(grid):
    out = []
    for y in range(H):
        for x in range(W):
            out.append(str(grid[y][x]))
    return "".join(out)


def build_scene(ground, solid):
    tile_str = row_major(ground)
    solid_str = row_major(solid)
    lines = []
    lines.append("scene_version=2")
    lines.append("name=main")
    lines.append("meta\tdisplay_name\t江湖有田·山庄")
    lines.append("meta\tgame\tjianghu_youtian")
    lines.append("meta\ttile_size\t32")
    lines.append("meta\tui_reference_height\t720")
    lines.append("meta\tui_reference_width\t1280")
    # 1 地表
    lines.append("entity\t1\tGroundMap")
    lines.append("component\t1\tTransform\tx=0.000000000;y=0.000000000;rotation=0.000000000;scale_x=1.000000000;scale_y=1.000000000")
    lines.append(
        f"component\t1\tTilemap\ttile_size=32x32;width={W};height={H};texture=assets/art/greybox/manor_ground.png;tiles={tile_str};solid={solid_str}"
    )
    lines.append("entity_meta\t1\trole\tground_tilemap")
    # 2 FarmGrid 覆盖耕地 overlay（初始全空，运行期 farm.lua 写 tile/meta）
    empty = "0" * (W * H)
    empty_solid = "0" * (W * H)
    lines.append("entity\t2\tFarmGrid")
    lines.append("component\t2\tTransform\tx=0.000000000;y=0.000000000;rotation=0.000000000;scale_x=1.000000000;scale_y=1.000000000")
    lines.append(
        f"component\t2\tTilemap\ttile_size=32x32;width={W};height={H};texture=assets/art/greybox/manor_farm.png;tiles={empty};solid={empty_solid}"
    )
    lines.append("entity_meta\t2\trole\tfarm_tilemap")
    # 3 玩家本体：场景内可碰撞动态刚体（脚本经 entity.find("JianghuPlayer") 使用，
    # 用 physics.set_velocity 移动）。Transform 定在出生格中心，Collider 自动建刚体。
    spawn_wx, spawn_wy = cell_to_world(SPAWN["x"], SPAWN["y"])
    lines.append("entity\t3\tJianghuPlayer")
    lines.append(
        f"component\t3\tTransform\tx={spawn_wx:.6f};y={spawn_wy:.6f};rotation=0.000000000;scale_x=1.000000000;scale_y=1.000000000"
    )
    lines.append("component\t3\tSprite\ttexture=__white__;size=28x28;tint=0.949,0.918,0.847,1.000")
    lines.append("component\t3\tCollider\tshape=ball;radius=12")
    lines.append("entity_meta\t3\trole\tplayer_spawn")
    lines.append("entity_meta\t3\tpersistent\ttrue")
    # HUD 文本（脚本动态刷新用 ui.text_at；场景内置两个静态说明/标题）
    lines.append("entity\t4\tHudTitle")
    lines.append("component\t4\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;offsets=24.00,20.00,640.00,44.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000")
    lines.append("component\t4\tText\ttext=山庄 · 药田垂直切片;font_size=18.00;color=0.949,0.918,0.847")
    lines.append("component\t4\tTransform\tx=0.000000000;y=0.000000000;rotation=0.000000000;scale_x=1.000000000;scale_y=1.000000000")
    lines.append("entity_meta\t4\trole\thud_title")
    lines.append("entity\t5\tHudHint")
    lines.append("component\t5\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;offsets=24.00,686.00,1000.00,708.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000")
    lines.append("component\t5\tText\ttext=WASD 移动 · E 朝面前格劳作 · T 睡觉到清晨 · Q 吐纳练功;font_size=14.00;color=0.850,0.900,1.000")
    lines.append("component\t5\tTransform\tx=0.000000000;y=0.000000000;rotation=0.000000000;scale_x=1.000000000;scale_y=1.000000000")
    lines.append("entity_meta\t5\trole\thud_hint")
    return "\n".join(lines) + "\n"


def main():
    ground = build_tiles()
    solid = build_solid(ground)
    text = build_scene(ground, solid)
    out = os.path.join("scenes", "main.scene")
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print("wrote", out, f"({W}x{H})")


if __name__ == "__main__":
    main()
