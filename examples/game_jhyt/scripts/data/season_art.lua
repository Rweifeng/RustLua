-- data/season_art.lua：四季地表贴图（M29 / 阶段五 P1「资源接线或下架」）。
--
-- 背景：`assets/art/tilesets/<map>/<map>_ground_{spring,summer,autumn,winter}.png`
-- 这套四季地表**在仓库里躺了很久但从未被引用**——场景只写死了春/夏共用的基础图
-- `manor_ground.png` / `village_ground.png`，于是 `calendar` 算出来的季节只影响
-- 作物（`farm`）与 HUD 文字，画面上毫无变化。本表把"哪张图对应哪个季节"变成数据，
-- 由 `world/season_visual.lua` 在进图与跨天时应用到场景的地表 Tilemap 上。
--
-- 约定（由 `validate_all` 强制）：
--   * `entity` 必须是场景里**地表 Tilemap 实体的名字**（不是地图 id）；
--   * `pattern` 必须恰好含一个 `%s`（季节占位符），便于 `string.format`；
--   * `seasons` 非空且取值来自闭集；缺哪个季节就明说，不要"猜一个兜底图"。
--   * 文件是否真的存在由验收样例（`jhyt_m29_season_weather`）逐张查文件系统——
--     Lua 侧没有列目录能力，能校验的只有结构与闭集。
local M = {}

M.version = 1

M.ENTRY_KEYS = { entity = true, pattern = true, seasons = true }

M.SEASONS = { "spring", "summer", "autumn", "winter" }

-- 只给"有四套地表"的地图登记；没有登记的地图保持场景里的基础贴图（例如山道/洞穴）。
M.ground = {
  manor = {
    entity = "GroundMap",
    pattern = "assets/art/tilesets/manor/manor_ground_%s.png",
    seasons = { "spring", "summer", "autumn", "winter" },
  },
  village = {
    entity = "GroundMap",
    pattern = "assets/art/tilesets/village/village_ground_%s.png",
    seasons = { "spring", "summer", "autumn", "winter" },
  },
}

--- 某张地图某季节的地表贴图路径；未登记/季节不支持返回 nil（调用方保持原图）。
function M.texture_for(map_id, season)
  local entry = M.ground[tostring(map_id or "")]
  if type(entry) ~= "table" or type(season) ~= "string" then
    return nil
  end
  for _, allowed in ipairs(entry.seasons or {}) do
    if allowed == season then
      return string.format(entry.pattern, season)
    end
  end
  return nil
end

--- 已登记的地图 id 数组（顺序稳定，便于验收与文档同步）。
function M.maps()
  local ids = {}
  for id in pairs(M.ground) do
    ids[#ids + 1] = id
  end
  table.sort(ids)
  return ids
end

--- 所有"地图 × 季节"的贴图路径（顺序稳定）：验收逐张查存在性用。
function M.textures()
  local paths = {}
  for _, map_id in ipairs(M.maps()) do
    local entry = M.ground[map_id]
    for _, season in ipairs(entry.seasons or {}) do
      paths[#paths + 1] = string.format(entry.pattern, season)
    end
  end
  return paths
end

local function is_season(value)
  for _, name in ipairs(M.SEASONS) do
    if name == value then
      return true
    end
  end
  return false
end

function M.validate_all()
  local issues = {}
  local function fail(path, message)
    issues[#issues + 1] = path .. ": " .. message
  end
  if M.version ~= 1 then
    fail("data.season_art.version", "expected 1, got " .. tostring(M.version))
  end
  if type(M.ground) ~= "table" or next(M.ground) == nil then
    fail("data.season_art.ground", "expected non-empty table")
    return #issues == 0, issues
  end
  for map_id, entry in pairs(M.ground) do
    local path = "data.season_art.ground." .. tostring(map_id)
    if type(entry) ~= "table" then
      fail(path, "expected table")
    else
      for key in pairs(entry) do
        if not M.ENTRY_KEYS[key] then
          fail(path .. "." .. tostring(key), "unknown key (expected entity/pattern/seasons)")
        end
      end
      if type(entry.entity) ~= "string" or entry.entity == "" then
        fail(path .. ".entity", "expected non-empty tilemap entity name")
      end
      if type(entry.pattern) ~= "string" then
        fail(path .. ".pattern", "expected string")
      else
        local _, count = string.gsub(entry.pattern, "%%s", "")
        if count ~= 1 then
          fail(path .. ".pattern", "expected exactly one %s placeholder, got " .. tostring(count))
        end
        if not string.find(entry.pattern, "assets/", 1, true) then
          fail(path .. ".pattern", "expected project-relative assets/ path")
        end
      end
      if type(entry.seasons) ~= "table" or #entry.seasons == 0 then
        fail(path .. ".seasons", "expected non-empty array")
      else
        local seen = {}
        for index, season in ipairs(entry.seasons) do
          if not is_season(season) then
            fail(
              path .. ".seasons[" .. tostring(index) .. "]",
              "unknown season " .. tostring(season)
            )
          elseif seen[season] then
            fail(path .. ".seasons[" .. tostring(index) .. "]", "duplicate season " .. season)
          end
          seen[season] = true
        end
      end
    end
  end
  return #issues == 0, issues
end

return M
