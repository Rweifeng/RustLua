-- 由 tools/gen_farm_soil.py 生成，勿手改：农田三态的图集寻址与湿度 tint 表。
--
-- FarmGrid 只有两列（作物贴图在 M72-08 搬去 CropGrid，见 scripts/data/crop_art.lua）：
-- 湿润不是第三列，而是 FarmMoist 第 1 列的半透明叠痕 + 下面这份 tint 分档。
local M = {}

M.farm_texture = "assets/art/tilesets/manor/manor_farm.png"
M.farm_cols = 2
M.moist_texture = "assets/art/tilesets/manor/manor_farm_moist.png"
M.moist_cols = 1

M.tile = {
  tilled = 1,  -- 犁地：翻起的松土，三道垄
  dry = 2,     -- 干涸：浅底 + 裂缝，该浇水了
}

-- 湿痕 tint（1 基）：按 watered_days 取档，越浇越淡。引擎 `tilemap.set_tint` 直接吃这四个数。
M.moist_tints = {
  { 0.8, 0.88, 1.12, 1 }, -- 第 1 档
  { 0.88, 0.93, 1.06, 1 }, -- 第 2 档
  { 0.96, 0.98, 1.01, 1 }, -- 第 3 档,
}
M.moist_tints_rgb = "0.8,0.88,1.12,1|0.88,0.93,1.06,1|0.96,0.98,1.01,1"

--- 已浇水天数 → tint 档号（超出表长取最后一档）。
function M.moist_tint_id(watered_days)
  local n = 3
  if watered_days <= 0 then
    return 1
  end
  return watered_days < n and watered_days or n
end

return M
