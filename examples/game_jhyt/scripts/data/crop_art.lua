-- 由 tools/gen_crop_art.py 生成，勿手改：作物 → CropGrid 图集行号。
--
-- 条目算术与 `tools/crop_kit.entry_for`、引擎 `Tilemap::atlas_uv` 同构
-- （1 基、row-major）：`entry = 1 + (row-1)*cols + slot`。
-- slot 0..4 是阶段 1..N，slot 5..7 是收成抖动三帧。
-- 行数/顺序变了必须同时重出 `assets/art/crops/manor_crops.png` 与
-- `scenes/manor.scene` 的 CropGrid（atlas_cols/atlas_rows），三者由
-- `jhyt_m72_crop_stages` 门对拍。
local M = {}

M.atlas = "assets/art/crops/manor_crops.png"
M.cols = 8
M.rows = 29
M.stage_slots = 5
M.shake_slots = 3

M.crops = {
  lingzhi = { row = 1, stages = 5 },
  rice = { row = 2, stages = 4 },
  radish = { row = 3, stages = 4 },
  herb = { row = 4, stages = 5 },
  polygonum = { row = 5, stages = 5 },
  ginseng = { row = 6, stages = 5 },
  mint = { row = 7, stages = 5 },
  mulberry = { row = 8, stages = 4 },
  pea = { row = 9, stages = 4 },
  cucumber = { row = 10, stages = 4 },
  mung_bean = { row = 11, stages = 4 },
  prunella = { row = 12, stages = 5 },
  eggplant = { row = 13, stages = 4 },
  pumpkin = { row = 14, stages = 4 },
  gouqi = { row = 15, stages = 5 },
  cabbage = { row = 16, stages = 4 },
  chrysanthemum = { row = 17, stages = 5 },
  leek = { row = 18, stages = 4 },
  winter_wheat = { row = 19, stages = 4 },
  xueli = { row = 20, stages = 4 },
  winter_shen = { row = 21, stages = 5 },
  plum = { row = 22, stages = 5 },
  winter_cabbage = { row = 23, stages = 4 },
  evergreen = { row = 24, stages = 5 },
  peach = { row = 25, stages = 3 },
  mume = { row = 26, stages = 3 },
  loquat = { row = 27, stages = 3 },
  jujube = { row = 28, stages = 3 },
  ginkgo = { row = 29, stages = 3 },
}

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
