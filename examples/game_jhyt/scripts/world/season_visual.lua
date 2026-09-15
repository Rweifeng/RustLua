-- world/season_visual.lua：季节可视化（M29 / 阶段五 P1）。
--
-- 做什么：把 `data.season_art` 里登记的"地图 × 季节 → 地表贴图"应用到**当前场景**的
-- 地表 Tilemap 组件上，让季节变化真的看得见（此前季节只影响作物与 HUD 文字）。
--
-- 依赖的引擎能力是 M29 新增的 `tilemap.set_texture(id, path)`：渲染器每帧重读
-- `Tilemap::texture`，组件变化时自动重建该层网格，因此换图在下一帧生效，
-- 并且**不动**格子/碰撞层（`tiles`/`solid`/每格 meta 全保留）。
--
-- 调用点（两处，缺一不可）：
--   * 进图后（`main.enter_playable_map`）：切到另一张地图时按当前季节换图；
--   * 跨天时（`main.on_day_start`）：第 29/57/85 天季节跳变时立即换图。
--
-- 失败语义：地图**未登记**四季地表、或场景里找不到该地表实体 —— 返回 `"skipped"` 并保持原贴图
-- （"这张图没有四季美术"是正常情况，不是错误）。除此之外一律直调引擎 API、不吞异常：
-- `tilemap.get`/`tilemap.set_texture` 报错说明数据写错了（实体名拼错、拿精灵当 Tilemap），
-- 那种错误必须当场炸出来，不能被 `pcall` 变成"悄悄没换图"。
local state = require("core.state")
local calendar = require("world.calendar")
local season_art = require("data.season_art")

local M = {}

local LOG_PREFIX = "[season] "

--- 应用某地图某季节的地表贴图。
--- 返回 `"applied"` / `"skipped"`（未登记或场景无该实体），便于验收与调试。
function M.apply(map_id, season)
  local want = season_art.texture_for(map_id, season)
  if not want then
    return "skipped"
  end
  local entity_id = entity.find(season_art.ground[tostring(map_id)].entity)
  if not entity_id then
    log.warn(LOG_PREFIX .. "no ground tilemap for map " .. tostring(map_id))
    return "skipped"
  end
  if tilemap.get(entity_id).texture == want then
    -- 已经是目标贴图（同季节重复进图/跨天）：不重复写组件，避免无谓的网格重建。
    return "applied"
  end
  tilemap.set_texture(entity_id, want)
  log.info(LOG_PREFIX .. tostring(map_id) .. " -> " .. tostring(season) .. " (" .. want .. ")")
  state.set_var("season_swaps", M.swap_count() + 1, "season", "swap")
  return "applied"
end

--- 按当前季节刷新（进图 / 跨天调用）。返回 `apply` 的结果字符串。
function M.refresh(map_id)
  if type(map_id) ~= "string" or map_id == "" then
    return "skipped"
  end
  return M.apply(map_id, calendar.current())
end

--- 当前场景地表贴图（只读，供 HUD/调试/验收查询）；场景无地表实体返回 nil。
function M.current_texture(map_id)
  local entry = season_art.ground[tostring(map_id or "")]
  if type(entry) ~= "table" then
    return nil
  end
  local entity_id = entity.find(entry.entity)
  if not entity_id then
    return nil
  end
  return tilemap.get(entity_id).texture
end

--- 本局换过几次图（存档标量，只为验收/调试可见，不影响玩法）。
function M.swap_count()
  return tonumber(state.get_var("season_swaps")) or 0
end

--- 复位（新档）：计数清零。贴图由下一次进图/跨天重新应用。
function M.reset()
  state.set_var("season_swaps", 0, "season", "reset")
  return true
end

return M
