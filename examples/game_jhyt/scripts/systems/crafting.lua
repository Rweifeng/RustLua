-- crafting.lua：《江湖有田》制作设施运行时（M18-02）。
--
-- 职责：把 data/recipes.lua 的纯数据配方挂到已放建筑的 station 上，实现
-- 「消耗材料 → 按游戏分钟制作 → 收取产出」闭环。
--
--   * 时间模型：clock 的游戏分钟绝对量 now_abs = (day-1)*1440 + minute_of_day；
--     job 记录 done_abs，跨天自然进位，不使用真实时间。
--   * 原子性：start 先校验配方/设施/等级/空闲/材料，任一失败即返回明确原因且
--     无副作用；collect 用 inv.add 的 leftover 判定背包容量，满了保留 job。
--   * 持久化：map.state("crafting") = { version=1, jobs={ [building_key]=job } }，
--     随 __mapstate:* 进存档；读档后按 clock 重新推导剩余时间。
local state = require("core.state")
local building = require("systems.building")
local recipe_data = require("data.recipes")
-- M19-02：开工/收取成功音效（失败路径由调用方/HUD 提示）。
local audio = require("core.audio")

local M = {}

local STATE_KEY = "crafting"
local STATE_VERSION = 1
local MINUTES_PER_DAY = 1440

local jobs = {} -- building_key → job
local loaded = false
local current_map = nil

local function now_abs()
  local now = clock.now()
  local day = tonumber(now.day) or 1
  local minute = tonumber(now.minute_of_day)
  if not minute then
    minute = tonumber(now.minute) or 0
  end
  return (day - 1) * MINUTES_PER_DAY + minute
end

-- 版本门：`version` 此前只写不校验，schema 变更后旧数据会被按新格式静默读入。
-- 无数据（nil）静默跳过；有数据但版本不认识 → warn + 拒绝（与 building.lua 同一口径）。
local function version_ok(data, where)
  if type(data) ~= "table" then
    return false
  end
  if tonumber(data.version) == STATE_VERSION then
    return true
  end
  log.warn("[crafting] " .. where .. " 版本不匹配：" .. tostring(data.version)
    .. "，当前 " .. STATE_VERSION .. "，本次不恢复")
  return false
end

local function persist()
  local saved = {}
  for key, job in pairs(jobs) do
    saved[key] = {
      recipe = job.recipe,
      building = job.building,
      start_abs = job.start_abs,
      done_abs = job.done_abs,
      output_item = job.output_item,
      output_count = job.output_count
    }
  end
  map.state(STATE_KEY, { version = STATE_VERSION, jobs = saved })
end

local function load_state()
  jobs = {}
  local saved = map.state(STATE_KEY)
  if not version_ok(saved, "map.state") then
    return
  end
  for key, job in pairs(saved.jobs or {}) do
    if type(job) == "table" and job.output_item then
      jobs[key] = {
        recipe = job.recipe,
        building = job.building,
        start_abs = tonumber(job.start_abs) or 0,
        done_abs = tonumber(job.done_abs) or 0,
        output_item = job.output_item,
        output_count = tonumber(job.output_count) or 1
      }
    end
  end
end

-- 切图/新档后按当前地图装载任务（幂等）
function M.on_enter(map_id)
  if current_map ~= map_id or not loaded then
    current_map = map_id
    load_state()
    loaded = true
  end
  return true
end

local function building_entry(key)
  for _, entry in ipairs(building.list()) do
    if entry.key == key then
      return entry
    end
  end
  return nil
end

local function recipe_station_ok(recipe_id, entry)
  local recipe = recipe_data.get(recipe_id)
  if not recipe then
    return false, "unknown_recipe"
  end
  if not entry then
    return false, "no_building"
  end
  local def = require("data.buildings").get(entry.building)
  if not def or not def.station or def.station ~= recipe.station then
    return false, "no_station"
  end
  if (entry.level or 1) < (recipe.min_level or 1) then
    return false, "station_level"
  end
  return true, recipe
end

function M.has_job(key)
  return jobs[key] ~= nil
end

-- 开始制作：失败无副作用
function M.start(recipe_id, key)
  if jobs[key] then
    return false, "busy"
  end
  local ok, recipe = recipe_station_ok(recipe_id, building_entry(key))
  if not ok then
    return false, recipe
  end
  for _, entry in ipairs(recipe.inputs or {}) do
    if inv.count(state.inventory, entry.item) < entry.count then
      return false, "not_enough"
    end
  end
  for _, entry in ipairs(recipe.inputs or {}) do
    inv.remove(state.inventory, entry.item, entry.count)
  end
  local start = now_abs()
  jobs[key] = {
    recipe = recipe_id,
    building = building_entry(key).building,
    start_abs = start,
    done_abs = start + recipe.minutes,
    output_item = recipe.output.item,
    output_count = recipe.output.count
  }
  persist()
  state.last_action = "craft_start_" .. recipe_id
  audio.sfx("sfx_craft")
  return true, recipe_id
end

-- 收取：未到点返回 crafting + 剩余分钟；背包满保留 job
function M.collect(key)
  local job = jobs[key]
  if not job then
    return false, "no_job"
  end
  local remaining = job.done_abs - now_abs()
  if remaining > 0 then
    state.last_action = "crafting"
    return false, { "crafting", remaining }
  end
  local leftover = tonumber(inv.add(state.inventory, job.output_item, job.output_count)) or 0
  if leftover > 0 then
    state.last_action = "full"
    return false, "full"
  end
  jobs[key] = nil
  persist()
  state.last_action = "craft_collect_" .. job.output_item
  -- 熟练度（docs/33 §7.1，0.9）：制作完成计制药线 XP。
  pcall(function()
    require("systems.mastery").add_xp("alchemy", 8)
  end)
  audio.sfx("sfx_craft")
  return true, job.output_item
end

-- 查询：{ has_job, recipe, remaining, output_item, output_count, station, ready }
function M.query(key)
  local entry = building_entry(key)
  local def = entry and require("data.buildings").get(entry.building) or nil
  local job = jobs[key]
  if not job then
    return {
      has_job = false,
      station = def and def.station or nil,
      level = entry and entry.level or nil
    }
  end
  local remaining = math.max(0, job.done_abs - now_abs())
  return {
    has_job = true,
    recipe = job.recipe,
    remaining = remaining,
    ready = remaining == 0,
    output_item = job.output_item,
    output_count = job.output_count,
    station = def and def.station or nil,
    level = entry and entry.level or nil
  }
end

-- 该建筑当前可开工的第一个配方（按 min_level/id 稳定排序）
function M.available_recipe(key)
  local entry = building_entry(key)
  if not entry then
    return nil, "no_building"
  end
  local def = require("data.buildings").get(entry.building)
  if not def or not def.station then
    return nil, "no_station"
  end
  for _, recipe_id in ipairs(recipe_data.for_station(def.station)) do
    local ok = recipe_station_ok(recipe_id, entry)
    if ok then
      local recipe = recipe_data.get(recipe_id)
      local affordable = true
      for _, input in ipairs(recipe.inputs or {}) do
        if inv.count(state.inventory, input.item) < input.count then
          affordable = false
          break
        end
      end
      if affordable then
        return recipe_id
      end
    end
  end
  return nil, "not_enough"
end

-- 交互目标：E 在设施建筑上开工 / 收取（priority 10，介于农田 0 与商店 20 之间）
function M.ensure_target()
  local interaction = require("systems.interaction")
  interaction.register({
    id = "building_station",
    priority = 10,
    query = function(facing)
      local rec = building.facing_building(facing)
      if not rec then
        return nil, "no_target"
      end
      return { cell = { x = rec.cx, y = rec.cy }, distance = 1, facing = 1 }
    end,
    can_interact = function()
      return true
    end,
    prompt = function(candidate, reason)
      if reason then
        return reason
      end
      local rec = candidate and candidate.cell and building.at(candidate.cell.x, candidate.cell.y)
      if not rec then
        return "E: 建筑"
      end
      local info = M.query(rec.key)
      if info.has_job then
        if info.ready then
          return "E: 收取 " .. tostring(info.output_item)
        end
        return "E: 制作中（剩余 " .. tostring(info.remaining) .. " 分钟）"
      end
      local recipe_id = M.available_recipe(rec.key)
      if recipe_id then
        return "E: 制作 " .. tostring(recipe_data.get(recipe_id).name)
      end
      return "E: 建筑"
    end,
    interact = function(candidate)
      local rec = candidate and candidate.cell and building.at(candidate.cell.x, candidate.cell.y)
      if not rec then
        return "no_building"
      end
      local info = M.query(rec.key)
      if info.has_job then
        local ok, reason = M.collect(rec.key)
        if ok then
          return state.last_action
        end
        if type(reason) == "table" then
          return reason[1]
        end
        return tostring(reason)
      end
      local recipe_id = M.available_recipe(rec.key)
      if not recipe_id then
        local _, why = M.available_recipe(rec.key)
        return tostring(why)
      end
      local ok, reason = M.start(recipe_id, rec.key)
      if ok then
        return state.last_action
      end
      return tostring(reason)
    end
  })
end

function M.reset()
  jobs = {}
  loaded = false
  current_map = nil
  map.state(STATE_KEY, { version = STATE_VERSION, jobs = {} })
end

function M.capture()
  local out = {}
  for key, job in pairs(jobs) do
    out[key] = {
      recipe = job.recipe,
      building = job.building,
      start_abs = job.start_abs,
      done_abs = job.done_abs,
      output_item = job.output_item,
      output_count = job.output_count
    }
  end
  return { version = STATE_VERSION, jobs = out }
end

function M.restore(data)
  if not version_ok(data, "snapshot") then
    return false, "bad_version"
  end
  jobs = {}
  for key, job in pairs(data.jobs or {}) do
    if type(job) == "table" and job.output_item then
      jobs[key] = {
        recipe = job.recipe,
        building = job.building,
        start_abs = tonumber(job.start_abs) or 0,
        done_abs = tonumber(job.done_abs) or 0,
        output_item = job.output_item,
        output_count = tonumber(job.output_count) or 1
      }
    end
  end
  persist()
end

return M
