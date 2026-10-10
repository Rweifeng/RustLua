# -*- coding: utf-8 -*-
"""作物阶段图生成器（M72-08）：29 种作物 × 4/5/3 阶段 + 收成抖动帧。

用法（工程根目录 `examples/game_jhyt` 下）：

    python tools/gen_crop_art.py            # 出图 + 出 crop_art.lua
    python tools/gen_crop_art.py --check    # 只复核盘上一致性（不写文件）

产出：

    assets/art/crops/<crop_id>/stage_<n>.png   逐张阶段图（docs/58 台账与
                                               tools/audit_art_assets.py 的契约路径）
    assets/art/crops/manor_crops.png           8 列 × 29 行的打包图集（CropGrid 采样）
    scripts/data/crop_art.lua                  运行期寻址表（crop → 图集行号）
    assets/art/crops/_preview.png              29 种成熟态并排（目验"一眼分辨"用）
    assets/art/crops/_preview_stages.png       每种作物整行阶段（目验生长节奏用）

三条硬契约：

1. **单源**：先画 32×32 单体图、描边，再 blit 进图集，再从**同一张量化后的图集**里裁回
   逐张 stage PNG。逐张图与图集条目因此不可能分叉（`--check` 逐条比对字节）。
2. **不许现写 hex**：颜色只从 `terrain_ramp.json`（地表六族 + `crops` 的 16 个作物身份族
   + 通用四色）取；出图前过一次 `quantize_to_ramp`，越表像素数必须为 0。
3. **两两不重复**：形态签名（不含颜色）两两不同 + 全部阶段图 PNG 字节哈希两两不同。
   反面对照由 `jhyt_m72_crop_stages` 门负责跑红，这里只在生成端守住签名这一半。

依赖：Python 3 标准库 + 同目录 `pixel_kit.py`、`terrain_kit.py`、`crop_kit.py`。
"""

import argparse
import hashlib
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import crop_kit as ck  # noqa: E402
import terrain_kit as tk  # noqa: E402
from pixel_kit import Canvas, draw_text, text_width  # noqa: E402

GAME_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CROPS_OUT = os.path.join(GAME_ROOT, "assets", "art", "crops")
ATLAS_OUT = os.path.join(CROPS_OUT, "manor_crops.png")
PREVIEW_RIPE = os.path.join(CROPS_OUT, "_preview.png")
PREVIEW_STAGES = os.path.join(CROPS_OUT, "_preview_stages.png")
LUA_OUT = os.path.join(GAME_ROOT, "scripts", "data", "crop_art.lua")


def render_tiles():
    """(crop_id, slot) → 32×32 单体图。slot 0..4 阶段，5..7 收成抖动；超出的阶段槽全透明。"""
    tiles = {}
    for crop_id in ck.ORDER:
        stages = ck.stages_of(crop_id)
        for slot in range(ck.COLS):
            if slot < stages:
                tiles[(crop_id, slot)] = ck.draw_stage(crop_id, slot + 1)
            elif slot < ck.STAGE_SLOTS:
                tiles[(crop_id, slot)] = Canvas(ck.TILE, ck.TILE)
            else:
                tiles[(crop_id, slot)] = ck.draw_shake(crop_id, slot - ck.STAGE_SLOTS)
    return tiles


def pack_atlas(tiles, quantize=True):
    sheet = Canvas(ck.TILE * ck.COLS, ck.TILE * ck.ROWS)
    for crop_id in ck.ORDER:
        row = ck.row_of(crop_id) - 1
        for slot in range(ck.COLS):
            sheet.blit(tiles[(crop_id, slot)], slot * ck.TILE, row * ck.TILE)
    if not quantize:
        return sheet, 0
    out, offenders = tk.quantize_to_ramp(sheet, ck.crop_allowed_colors())
    return out, offenders


def crop_entry(sheet, slot, row):
    return sheet.sliced(slot * ck.TILE, row * ck.TILE, slot * ck.TILE + ck.TILE - 1,
                        row * ck.TILE + ck.TILE - 1)


def stage_files_from_atlas(sheet):
    """逐张阶段图 = 从量化后的图集里裁出来的条目（不是再画一遍）。"""
    files = {}
    for crop_id in ck.ORDER:
        row = ck.row_of(crop_id) - 1
        for index in range(ck.stages_of(crop_id)):
            files[(crop_id, index + 1)] = crop_entry(sheet, index, row)
    return files


def lua_table():
    rows = ",\n".join(
        f'  {crop_id} = {{ row = {ck.row_of(crop_id)}, stages = {ck.stages_of(crop_id)} }}'
        for crop_id in ck.ORDER
    )
    return f"""-- 由 tools/gen_crop_art.py 生成，勿手改：作物 → CropGrid 图集行号。
--
-- 条目算术与 `tools/crop_kit.entry_for`、引擎 `Tilemap::atlas_uv` 同构
-- （1 基、row-major）：`entry = 1 + (row-1)*cols + slot`。
-- slot 0..{ck.STAGE_SLOTS - 1} 是阶段 1..N，slot {ck.STAGE_SLOTS}..{ck.COLS - 1} 是收成抖动三帧。
-- 行数/顺序变了必须同时重出 `assets/art/crops/manor_crops.png` 与
-- `scenes/manor.scene` 的 CropGrid（atlas_cols/atlas_rows），三者由
-- `jhyt_m72_crop_stages` 门对拍。
local M = {{}}

M.atlas = "{ck.ATLAS_TEXTURE}"
M.cols = {ck.COLS}
M.rows = {ck.ROWS}
M.stage_slots = {ck.STAGE_SLOTS}
M.shake_slots = {ck.SHAKE_SLOTS}

M.crops = {{
{rows},
}}

--- 某作物某阶段（1 基）在 CropGrid 里的瓦值（= 图集条目号，1 基；0 才是空格）。
function M.entry(crop_id, stage)
  local rec = M.crops[crop_id]
  if not rec or stage < 1 or stage > rec.stages then
    return 0
  end
  return 1 + (rec.row - 1) * M.cols + (stage - 1)
end

--- 收成抖动第 frame 帧（0 基）的瓦值。
function M.shake_entry(crop_id, frame)
  local rec = M.crops[crop_id]
  if not rec or frame < 0 or frame >= M.shake_slots then
    return 0
  end
  return 1 + (rec.row - 1) * M.cols + M.stage_slots + frame
end

return M
"""


def preview_ripe(tiles, scale=3):
    """29 种的成熟态并排（6 列）：这张图回答"稻田与灵芝田肉眼能不能分辨"。"""
    columns = 6
    rows = (len(ck.ORDER) + columns - 1) // columns
    sheet = Canvas(columns * ck.TILE * scale, rows * ck.TILE * scale)
    for index, crop_id in enumerate(ck.ORDER):
        stages = ck.stages_of(crop_id)
        tile = tiles[(crop_id, stages - 1)].scaled(scale)
        col, row = index % columns, index // columns
        sheet.blit(tile, col * ck.TILE * scale, row * ck.TILE * scale)
        draw_text(sheet, col * ck.TILE * scale + 2, row * ck.TILE * scale + 2, crop_id,
                  (242, 234, 216, 255))
    return sheet


def preview_stages(tiles, scale=2):
    """每种作物一整行（5 个阶段槽 + 3 个抖动槽）：目验生长节奏与抖动帧。"""
    sheet = Canvas(ck.COLS * ck.TILE * scale, len(ck.ORDER) * ck.TILE * scale)
    for row, crop_id in enumerate(ck.ORDER):
        for slot in range(ck.COLS):
            sheet.blit(tiles[(crop_id, slot)].scaled(scale), slot * ck.TILE * scale,
                       row * ck.TILE * scale)
        draw_text(sheet, 2, row * ck.TILE * scale + 2,
                  f"{crop_id} x{ck.stages_of(crop_id)}", (242, 234, 216, 255))
    return sheet


def write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(data)


def generate():
    ck.validate_order(os.path.join(GAME_ROOT, "scripts", "data", "crops.lua"))
    tiles = render_tiles()
    sheet, offenders = pack_atlas(tiles)
    if offenders:
        raise SystemExit(f"色阶表外像素 {offenders} 个：作物图集必须只用 terrain_ramp.json 的颜色")
    files = stage_files_from_atlas(sheet)
    # 唯一性判在**量化后的图集条目**上、且在写盘之前：判原始单体图会漏掉"两张不同的图被
    # 色阶表吸附成同一张"这种真事故；写盘之后再红则把重复资产分发到了池子里。
    problems = check_uniqueness(files)
    if problems:
        raise SystemExit("gen_crop_art 生成中止：\n  " + "\n  ".join(problems))
    for (crop_id, stage), tile in sorted(files.items()):
        write(os.path.join(CROPS_OUT, crop_id, f"stage_{stage}.png"), tile.to_png())
    write(ATLAS_OUT, sheet.to_png())
    write(PREVIEW_RIPE, preview_ripe(tiles).to_png())
    write(PREVIEW_STAGES, preview_stages(tiles).to_png())
    write(LUA_OUT, lua_table().encode("utf-8"))
    print(f"gen_crop_art_ok crops={len(ck.ORDER)} stage_files={len(files)} "
          f"atlas={ck.COLS}x{ck.ROWS} entries={ck.COLS * ck.ROWS} off_ramp=0")
    return 0


# ---------------------------------------------------------------------------
# --check：不写文件，只复核"盘上 == 重新生成"
# ---------------------------------------------------------------------------


def digest(data):
    return hashlib.sha256(data).hexdigest()


def check():
    ck.validate_order(os.path.join(GAME_ROOT, "scripts", "data", "crops.lua"))
    problems = []
    tiles = render_tiles()
    sheet, offenders = pack_atlas(tiles)
    if offenders:
        problems.append(f"越表像素 {offenders}")
    if digest(sheet.to_png()) != _read_digest(ATLAS_OUT):
        problems.append(f"图集与重新生成不一致：{ck.ATLAS_TEXTURE}")
    files = stage_files_from_atlas(sheet)
    for (crop_id, stage), tile in sorted(files.items()):
        path = os.path.join(CROPS_OUT, crop_id, f"stage_{stage}.png")
        if not os.path.isfile(path):
            problems.append(f"缺文件 {os.path.join(ck.CROPS_DIR, crop_id, f'stage_{stage}.png')}")
        elif digest(tile.to_png()) != _read_digest(path):
            problems.append(f"逐张图与图集条目不一致 {crop_id} stage {stage}")
    missing_extra = _unexpected_stage_files(files)
    if missing_extra:
        problems.append(f"盘上有 CATALOG 之外的阶段图：{missing_extra}")
    if digest(lua_table().encode("utf-8")) != _read_digest(LUA_OUT):
        problems.append("scripts/data/crop_art.lua 与 CATALOG 不同步")
    problems.extend(check_uniqueness(files))
    if problems:
        raise SystemExit("gen_crop_art 复核失败：\n  " + "\n  ".join(problems))
    print(f"gen_crop_art_check_ok stage_files={sum(ck.stages_of(c) for c in ck.ORDER)} "
          f"signatures={len(set(ck.signature(c) for c in ck.ORDER))} off_ramp=0")
    return 0


def _read_digest(path):
    if not os.path.isfile(path):
        return None
    with open(path, "rb") as handle:
        return digest(handle.read())


def _unexpected_stage_files(files):
    """盘上多出来的 stage_*.png（CATALOG 改小时留下的旧图）也算不一致，别让门读到幽灵。"""
    wanted = {(crop_id, stage) for (crop_id, stage) in files}
    extra = []
    if not os.path.isdir(CROPS_OUT):
        return extra
    for crop_id in sorted(os.listdir(CROPS_OUT)):
        folder = os.path.join(CROPS_OUT, crop_id)
        if not os.path.isdir(folder):
            continue
        for name in sorted(os.listdir(folder)):
            if not (name.startswith("stage_") and name.endswith(".png")):
                continue
            try:
                stage = int(name[len("stage_"):-len(".png")])
            except ValueError:
                stage = -1
            if (crop_id, stage) not in wanted:
                extra.append(os.path.join(crop_id, name))
    return extra


def check_uniqueness(files):
    """两两不重复的两条判据：形态签名（不含颜色）+ 量化后阶段图字节哈希。

    比对的是**图集里裁出来的条目**（`files`），也就是实际写盘、实际被引擎采样的那批字节：
    判未量化的单体图会漏掉"两张画法不同、吸附到同一色阶后完全相同"的事故。

    收成抖动的三个槽位不参与本判据：第三帧是"茬口"，它只由 height/spacing 决定，
    两株作物的茬口同形是正常的，把它算进"另一种作物长得不一样"是假判据。
    """
    problems = []
    seen_forms = {}
    for crop_id in ck.ORDER:
        form = ck.signature(crop_id)
        if form in seen_forms:
            problems.append(f"形态签名重复：{crop_id} 与 {seen_forms[form]}（只换色不算两种作物）")
        seen_forms[form] = crop_id
    images = {}
    for (crop_id, stage), tile in sorted(files.items()):
        if not _has_pixels(tile):
            problems.append(f"阶段图是空槽：{crop_id} stage {stage}（空图不能充当一种作物）")
            continue
        key = digest(tile.to_png())
        if key in images:
            problems.append(f"阶段图字节重复：{crop_id} stage {stage} == {images[key]}")
        images[key] = f"{crop_id} stage {stage}"
    return problems


def _has_pixels(tile):
    for y in range(tile.h):
        for x in range(tile.w):
            if tile.get(x, y)[3]:
                return True
    return False


def main(argv):
    parser = argparse.ArgumentParser(description="生成作物阶段图与 CropGrid 图集")
    parser.add_argument("--check", action="store_true", help="只复核盘上一致性，不写文件")
    args = parser.parse_args(argv[1:])
    return check() if args.check else generate()


if __name__ == "__main__":
    sys.exit(main(sys.argv))
