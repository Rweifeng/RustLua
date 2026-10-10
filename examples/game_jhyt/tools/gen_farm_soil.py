# -*- coding: utf-8 -*-
"""农田三态的出图侧（M72-08）：犁地 / 干涸 两张 FarmGrid 瓦 + 一张湿润叠痕。

用法（工程根目录 `examples/game_jhyt` 下）：

    python tools/gen_farm_soil.py            # 出图 + 出 farm_soil.lua
    python tools/gen_farm_soil.py --check    # 只复核盘上一致性（不写文件）

产出：

    assets/art/tilesets/manor/manor_farm.png        2 列 × 32px（FarmGrid overlay）
    assets/art/tilesets/manor/manor_farm_moist.png  1 列 × 32px（FarmMoist 湿痕）
    assets/art/tilesets/manor/_farm_preview.png     四格目验：犁地/干涸 各叠不叠湿痕
    scripts/data/farm_soil.lua                      运行期寻址与湿度 tint 表

## 为什么这一批从 `gen_manor_art.py` 搬出来单独成文

M72-07 的在制品压在 `gen_manor_art.py` 的建筑/屋顶区域上，而本包动的是农田区。两条流水
同文件并行 = 提交时无法只挑自己的 hunk。农田三态与作物阶段图是同一个工作包的两半
（`33` §3.1 的地块状态机），所以它跟着作物走，`gen_manor_art.py` 里的 `farm_*` 一并删除。

## 三态怎么落在两张瓦 + 一层叠痕上

`53` §3.1 的三态是**犁地 / 湿润 / 干涸**。湿润不画第三张全幅瓦（白花一张图的体积，
而且"湿"是连续量、离散步不进 tile 值里），而是：

    犁地 = FarmGrid 第 1 列
    干涸 = FarmGrid 第 2 列（浅底 + 深色裂缝）
    湿润 = 上面任一张 + FarmMoist 第 1 列的半透明湿痕 + per-tile tint 分档

tint 走 M72-02 的 `tilemap.set_tint`（运行期直接给 RGBA，不占 `ground_tints` 的编号，
所以不会动地表图集的 `tints_rgb` 对拍口径）。三档深浅由 `farm_soil.lua` 的表给，
`systems/farm.lua` 按 `watered_days` 取档——刚浇与浇过几天的地在同一张湿痕上读出差别。

## 颜色口径

不透明像素一律等于 `terrain_ramp.json` 表内某色（`dirt` 四档 + `water` 四档 + 通用墨），
出图后过 `quantize_to_ramp`，越表数必须为 0。**FarmMoist 例外**：它是半透明叠层，
`quantize_to_ramp` 会把 alpha>127 二值化成不透明，正好毁掉"透出下面土壤"这件事，
所以湿痕层只校验色相在表内、alpha 原样保留（见 `_moist_off_ramp_pixels`）。

依赖：Python 3 标准库 + 同目录 `pixel_kit.py`、`terrain_kit.py`。
"""

import argparse
import hashlib
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import terrain_kit as tk  # noqa: E402
from pixel_kit import Canvas, hash_stream, rgb  # noqa: E402

GAME_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TILE_DIR = os.path.join(GAME_ROOT, "assets", "art", "tilesets", "manor")
FARM_OUT = os.path.join(TILE_DIR, "manor_farm.png")
MOIST_OUT = os.path.join(TILE_DIR, "manor_farm_moist.png")
PREVIEW_OUT = os.path.join(TILE_DIR, "_farm_preview.png")
LUA_OUT = os.path.join(GAME_ROOT, "scripts", "data", "farm_soil.lua")

TILE = 32
FARM_COLUMNS = 2
MOIST_COLUMNS = 1

FARM_TEXTURE = "assets/art/tilesets/manor/manor_farm.png"
MOIST_TEXTURE = "assets/art/tilesets/manor/manor_farm_moist.png"

# 垄沟的行位：与作物贴图（`crop_kit` 的接触影在 y=30/31）错开，苗根不会坐在沟线上。
FURROWS = (5, 13, 21)

# 湿痕三档 tint（RGBA 浮点，乘法）：越靠后越淡，读作"浇过的水在往外散"。
MOIST_TINTS = (
    (0.80, 0.88, 1.12, 1.0),
    (0.88, 0.93, 1.06, 1.0),
    (0.96, 0.98, 1.01, 1.0),
)


def _ramp():
    return tk.load_ramp()


def _dirt(table, index):
    return rgb(tk.family_hexes(table, "dirt")[index])


def _water(table, index):
    return rgb(tk.family_hexes(table, "water")[index])


def tilled_tile(table):
    """犁地：翻起的深色松土 + 三道垄，垄脊上沿打亮一档。

    Speckle 用 `hash_stream` 而不是 `random`：同一 seed 必须每次出同一张图，
    否则 `--check` 与 Rust 门的字节对拍直接失效。
    """
    base, dark, light, deepest = (_dirt(table, i) for i in (2, 1, 3, 0))
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, base)
    for y in FURROWS:
        cv.rect(0, y, TILE - 1, y + 2, dark)
        cv.hline(0, TILE - 1, y - 1, light)
        cv.hline(0, TILE - 1, y + 3, deepest)
    rnd = hash_stream(77)
    for _ in range(26):
        cv.set(rnd(TILE), rnd(TILE), dark)
    rnd = hash_stream(78)
    for _ in range(16):
        cv.set(rnd(TILE), rnd(TILE), light)
    # 四边压暗一档：overlay 与四周夯土之间要有一条"边界"，否则新垦的地读不出是一块方块。
    cv.hline(0, TILE - 1, 0, dark)
    cv.hline(0, TILE - 1, TILE - 1, deepest)
    return cv


def dry_tile(table):
    """干涸：浅底 + 深色裂缝。与犁地的区分轴是**裂缝**和整体抬亮，不是"少几道垄"。

    为什么干涸要单独一张而不是给犁地压一个 tint：tint 是整格均匀乘法，压不出"裂开"这个
    形状信息，而玩家判断"该浇水了"靠的就是裂缝。

    缝必须是 1px 的**折线**、不许横贯整格：第一版把缝画成 `y` 与 `y+1` 两条满宽横线，
    在 32px 上与犁地的三道垄几乎同形，四格预览里"干涸"读不出是干了、只读出"另一张耕地"。
    """
    base, crust, crack = (_dirt(table, i) for i in (3, 2, 0))
    cv = Canvas(TILE, TILE)
    cv.rect(0, 0, TILE - 1, TILE - 1, base)
    # 干裂的土面是不平的：几块浅色硬壳 + 壳之间的暗缝，先铺出"板结"的底。
    for x0, y0, x1, y1 in ((2, 3, 13, 9), (17, 2, 28, 8), (5, 18, 15, 26), (19, 15, 29, 22)):
        cv.rect(x0, y0, x1, y1, base)
        cv.hline(x0, x1, y1, crust)
        cv.vline(x1, y0, y1, crust)
    rnd = hash_stream(131)
    for _ in range(26):
        cv.set(rnd(TILE), rnd(TILE), crust)
    for x, y, horizontal in ((0, 11, True), (0, 27, True), (11, 0, False), (22, 3, False)):
        _crack(cv, x, y, horizontal, crack, rnd)
    cv.hline(0, TILE - 1, TILE - 1, crust)
    return cv


def _crack(cv, x, y, horizontal, color, rnd):
    """一条从边界出发的 1px 裂缝：每走 2~4 px 折一格，中途拐出一条短支缝。"""
    branch_at = 8 + rnd(10)
    for step in range(TILE):
        if not (0 <= x < TILE and 0 <= y < TILE):
            break
        cv.set(x, y, color)
        if step == branch_at:
            bx, by = x, y
            for _ in range(5 + rnd(5)):
                bx += 1 if horizontal else (1 if rnd(2) else -1)
                by += 1 if rnd(2) else -1 if horizontal else 0
                if 0 <= bx < TILE and 0 <= by < TILE:
                    cv.set(bx, by, color)
        if step and step % (2 + rnd(3)) == 0:
            if horizontal:
                y += 1 if rnd(2) else -1
            else:
                x += 1 if rnd(2) else -1
        x += 1 if horizontal else 0
        y += 0 if horizontal else 1


def moist_mark(table):
    """湿润叠痕：整格一层淡冷水洗 + 四边湿边 + 垄沟水光 + 几颗水滴。

    半透明是这张图的全部意义：它压 FarmGrid 与 CropGrid 之上（见 `gen_manor_scene.py`
    里 FarmMoist 的 id 排序注释），不透明就等于把作物糊掉。

    为什么要那层 alpha≈46 的整格水洗：只画边与水滴时，"今天浇过"在实机一屏的尺度上读不出来
    （四格预览里第 2 格与第 1 格几乎同脸）。水洗负责"整块地变冷变深"这个一眼信号，
    边与水滴负责"湿在沟里积着"的方向感，tint 三档负责"浇了几天"的时间感。
    """
    deep = _water(table, 0)
    mid = _water(table, 1)
    bright = _water(table, 3)
    cv = Canvas(TILE, TILE)
    wash = (deep[0], deep[1], deep[2], 46)
    rim = (deep[0], deep[1], deep[2], 150)
    corner = (deep[0], deep[1], deep[2], 182)
    dash = (mid[0], mid[1], mid[2], 104)
    cv.rect(0, 0, TILE - 1, TILE - 1, wash)
    for x in range(TILE):
        for y in (0, 1, TILE - 2, TILE - 1):
            cv.set(x, y, rim)
    for y in range(2, TILE - 2):
        for x in (0, 1, TILE - 2, TILE - 1):
            cv.set(x, y, rim)
    # 四角加深：让湿痕有"积在角落"的体积感。
    for cx, cy in ((0, 0), (TILE - 4, 0), (0, TILE - 4), (TILE - 4, TILE - 4)):
        cv.rect(cx, cy, cx + 3, cy + 3, corner)
    # 水光只落在垄沟行上：湿在沟里积着，垄脊是干的。
    for y in FURROWS:
        for x in range(2, TILE - 4, 5):
            cv.rect(x, y + 1, x + 2, y + 2, dash)
    rnd = hash_stream(133)
    for _ in range(5):
        x = 3 + rnd(TILE - 8)
        y = 3 + rnd(TILE - 10)
        cv.set(x, y, bright)
        cv.rect(x, y + 1, x + 1, y + 3, (mid[0], mid[1], mid[2], 210))
    return cv


def _over(dst, src, dx=0, dy=0):
    """alpha-over 合成（只给预览图用）。

    `Canvas.blit` 是引擎口径：只搬 `alpha>0` 的像素、不混合。预览要回答的问题恰恰是
    "湿痕叠在犁地/干涸上读不读得出湿"，所以这里必须真混一次——别把它当成运行期行为。
    """
    for y in range(src.h):
        for x in range(src.w):
            top = src.get(x, y)
            if not top[3]:
                continue
            a = top[3] / 255
            back = dst.get(dx + x, dy + y)
            dst.set(dx + x, dy + y, tuple(
                round(top[i] * a + back[i] * (1 - a)) for i in range(3)) + (255,))


def preview_sheet(table, scale=4):
    """四格并排：犁地 / 犁地+湿痕 / 干涸 / 干涸+湿痕（目验三态读不读得出来）。"""
    tilled, dry, moist = tilled_tile(table), dry_tile(table), moist_mark(table)
    cells = [(tilled, None), (tilled, moist), (dry, None), (dry, moist)]
    sheet = Canvas(TILE * len(cells) * scale, TILE * scale)
    for index, (base, overlay) in enumerate(cells):
        patch = Canvas(TILE, TILE)
        patch.blit(base)
        if overlay:
            _over(patch, overlay)
        sheet.blit(patch.scaled(scale), index * TILE * scale, 0)
    return sheet


def pack(tiles, columns):
    assert len(tiles) == columns, f"tileset 必须正好 {columns} 列，实到 {len(tiles)}"
    sheet = Canvas(TILE * columns, TILE)
    for index, tile in enumerate(tiles):
        sheet.blit(tile, index * TILE, 0)
    return sheet


def farm_sheet(table):
    sheet = pack([tilled_tile(table), dry_tile(table)], FARM_COLUMNS)
    out, offenders = tk.quantize_to_ramp(sheet, tk.allowed_colors(table))
    if offenders:
        raise SystemExit(f"manor_farm.png 色阶表外像素 {offenders} 个")
    return out


def moist_sheet(table):
    return pack([moist_mark(table)], MOIST_COLUMNS)


def _moist_off_ramp_pixels(table, sheet):
    """湿痕层只校验**色相**在表内（切掉 alpha 比对），alpha 本身不参与。"""
    allowed = tk.allowed_colors(table)
    offenders = 0
    for y in range(sheet.h):
        for x in range(sheet.w):
            color = sheet.get(x, y)
            if color[3] and color[:3] not in allowed:
                offenders += 1
    return offenders


def lua_table():
    tints = "|".join(",".join(f"{value:g}" for value in rgba) for rgba in MOIST_TINTS)
    rows = "\n".join(
        f"  {{ {', '.join(f'{value:g}' for value in rgba)} }}, -- 第 {index} 档"
        for index, rgba in enumerate(MOIST_TINTS, 1)
    )
    return f"""-- 由 tools/gen_farm_soil.py 生成，勿手改：农田三态的图集寻址与湿度 tint 表。
--
-- FarmGrid 只有两列（作物贴图在 M72-08 搬去 CropGrid，见 scripts/data/crop_art.lua）：
-- 湿润不是第三列，而是 FarmMoist 第 1 列的半透明叠痕 + 下面这份 tint 分档。
local M = {{}}

M.farm_texture = "{FARM_TEXTURE}"
M.farm_cols = {FARM_COLUMNS}
M.moist_texture = "{MOIST_TEXTURE}"
M.moist_cols = {MOIST_COLUMNS}

M.tile = {{
  tilled = 1,  -- 犁地：翻起的松土，三道垄
  dry = 2,     -- 干涸：浅底 + 裂缝，该浇水了
}}

-- 湿痕 tint（1 基）：按 watered_days 取档，越浇越淡。引擎 `tilemap.set_tint` 直接吃这四个数。
M.moist_tints = {{
{rows},
}}
M.moist_tints_rgb = "{tints}"

--- 已浇水天数 → tint 档号（超出表长取最后一档）。
function M.moist_tint_id(watered_days)
  local n = {len(MOIST_TINTS)}
  if watered_days <= 0 then
    return 1
  end
  return watered_days < n and watered_days or n
end

return M
"""


def digest(data):
    return hashlib.sha256(data).hexdigest()


def _read(path):
    if not os.path.isfile(path):
        return None
    with open(path, "rb") as handle:
        return handle.read()


def write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(data)


def artifacts():
    table = _ramp()
    return table, farm_sheet(table), moist_sheet(table), lua_table().encode("utf-8")


def generate():
    table, farm, moist, lua = artifacts()
    offenders = _moist_off_ramp_pixels(table, moist)
    if offenders:
        raise SystemExit(f"manor_farm_moist.png 色阶表外色相 {offenders} 个")
    write(FARM_OUT, farm.to_png())
    write(MOIST_OUT, moist.to_png())
    write(PREVIEW_OUT, preview_sheet(table).to_png())
    write(LUA_OUT, lua)
    print(f"gen_farm_soil_ok farm={farm.w}x{farm.h}({FARM_COLUMNS}列) "
          f"moist={moist.w}x{moist.h}({MOIST_COLUMNS}列) tints={len(MOIST_TINTS)} off_ramp=0")
    return 0


def _read_digest(path):
    data = _read(path)
    return digest(data) if data else None


def check():
    table, farm, moist, lua = artifacts()
    problems = []
    if _moist_off_ramp_pixels(table, moist):
        problems.append("湿痕层有表外色相")
    if digest(farm.to_png()) != _read_digest(FARM_OUT):
        problems.append(f"盘上 {FARM_TEXTURE} 与重新生成不一致")
    if digest(moist.to_png()) != _read_digest(MOIST_OUT):
        problems.append(f"盘上 {MOIST_TEXTURE} 与重新生成不一致")
    if digest(lua) != _read_digest(LUA_OUT):
        problems.append("scripts/data/farm_soil.lua 与生成器不同步")
    farm_bytes = _read(FARM_OUT)
    moist_bytes = _read(MOIST_OUT)
    if farm_bytes and moist_bytes and digest(farm_bytes) == digest(moist_bytes):
        problems.append("犁地与湿痕是同一张图")
    if problems:
        raise SystemExit("gen_farm_soil 复核失败：\n  " + "\n  ".join(problems))
    print(f"gen_farm_soil_check_ok farm={FARM_COLUMNS}列 moist={MOIST_COLUMNS}列 "
          f"tints={len(MOIST_TINTS)} off_ramp=0")
    return 0


def main(argv):
    parser = argparse.ArgumentParser(description="生成农田三态贴图与湿度 tint 表")
    parser.add_argument("--check", action="store_true", help="只复核盘上一致性，不写文件")
    args = parser.parse_args(argv[1:])
    return check() if args.check else generate()


if __name__ == "__main__":
    sys.exit(main(sys.argv))
