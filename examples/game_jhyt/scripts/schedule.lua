-- schedule.lua：NPC 日程解析与校验（M17-02 纯数据运行时）。
--
-- 日程定义在 data/schedules/<id>.lua（见文件内注释的时段语义）。本模块只做
-- 三件事，不碰实体/寻路：
--   1. resolve(id, minute, rainy)  把"当天分钟"解析成 {place, activity}
--      —— 支持跨午夜时段（to <= from 表示 wrap），支持雨天替代 place。
--   2. validate(id) / validate_all()  —— 时段无缝覆盖 0..1439 且互不重叠的
--      确定性诊断（缺口/重叠/空区间/未知地点）。
--   3. format_minute(minute)  —— 诊断文案用的 HH:MM。
--
-- 冲突与缺失都返回结构化 issue 字符串（含 schedule id / 时段 / 原因），
-- 由 npc.lua 写入 log.warn 与可查询的 diagnostics 列表。
local places = require("data.places")
local npcs = require("data.npcs")

local M = {}

local MINUTES_PER_DAY = 1440
local cache = {}

local function normalize_minute(minute)
  local value = tonumber(minute)
  if not value then
    return 0
  end
  value = math.floor(value) % MINUTES_PER_DAY
  if value < 0 then
    value = value + MINUTES_PER_DAY
  end
  return value
end

-- 时段 [from, to)：from < to 普通区间；from > to 跨午夜（wrap）。
local function contains(block, minute)
  local from = tonumber(block.from)
  local to = tonumber(block.to)
  if not from or not to or from == to then
    return false
  end
  if from < to then
    return minute >= from and minute < to
  end
  return minute >= from or minute < to
end

-- 懒加载日程：require("data.schedules.<id>")；失败缓存为 false 避免反复报错。
function M.get(id)
  if type(id) ~= "string" or id == "" then
    return nil, "invalid schedule id"
  end
  if cache[id] == nil then
    local ok, def = pcall(require, "data.schedules." .. id)
    if ok and type(def) == "table" then
      cache[id] = def
    else
      cache[id] = false
    end
  end
  if cache[id] == false then
    return nil, "cannot load schedule: data.schedules." .. id
  end
  return cache[id]
end

function M.format_minute(minute)
  local value = normalize_minute(minute)
  return string.format("%02d:%02d", math.floor(value / 60), value % 60)
end

-- 解析：返回 { place, activity, block, index } 或 nil, reason。
-- rainy 为真且该时段定义了 rain_place 时，用 rain_place 替换 place。
function M.resolve(id, minute, rainy)
  local schedule, reason = M.get(id)
  if not schedule then
    return nil, reason
  end
  local blocks = schedule.blocks
  if type(blocks) ~= "table" or #blocks == 0 then
    return nil, "schedule has no blocks: " .. tostring(id)
  end
  local at = normalize_minute(minute)
  for index, block in ipairs(blocks) do
    if type(block) == "table" and contains(block, at) then
      local place = block.place
      if rainy and type(block.rain_place) == "string" then
        place = block.rain_place
      end
      return {
        place = place,
        activity = block.activity,
        block = block,
        index = index,
        minute = at
      }
    end
  end
  return nil, "no block covers minute " .. tostring(at) .. " in schedule " .. tostring(id)
end

-- 校验单个日程：返回 (ok, issues)。issues 为字符串列表。
function M.validate(id)
  local issues = {}
  local schedule, reason = M.get(id)
  if not schedule then
    issues[#issues + 1] = tostring(reason)
    return false, issues
  end
  local blocks = schedule.blocks
  if type(blocks) ~= "table" or #blocks == 0 then
    issues[#issues + 1] = "schedule " .. tostring(id) .. " has no blocks"
    return false, issues
  end
  local intervals = {}
  for index, block in ipairs(blocks) do
    local from = tonumber(type(block) == "table" and block.from)
    local to = tonumber(type(block) == "table" and block.to)
    if not from or not to then
      issues[#issues + 1] = "schedule " .. tostring(id) .. " block " .. index .. " has non-numeric from/to"
    elseif from < 0 or from >= MINUTES_PER_DAY or to < 0 or to >= MINUTES_PER_DAY then
      issues[#issues + 1] = "schedule " .. tostring(id) .. " block " .. index .. " minute out of range"
    elseif from == to then
      issues[#issues + 1] = "schedule " .. tostring(id) .. " block " .. index .. " is empty (from == to)"
    else
      if not places.get(block.place) then
        issues[#issues + 1] = "schedule " .. tostring(id) .. " block " .. index .. " unknown place: " .. tostring(block.place)
      end
      if block.rain_place and not places.get(block.rain_place) then
        issues[#issues + 1] = "schedule " .. tostring(id) .. " block " .. index .. " unknown rain_place: " .. tostring(block.rain_place)
      end
      if from < to then
        intervals[#intervals + 1] = { from, to, index }
      else
        intervals[#intervals + 1] = { from, MINUTES_PER_DAY, index }
        intervals[#intervals + 1] = { 0, to, index }
      end
    end
  end
  table.sort(intervals, function(a, b)
    if a[1] == b[1] then
      return a[2] < b[2]
    end
    return a[1] < b[1]
  end)
  local cursor = 0
  for _, interval in ipairs(intervals) do
    if interval[1] > cursor then
      issues[#issues + 1] = "schedule " .. tostring(id) .. " gap " .. M.format_minute(cursor)
        .. ".." .. M.format_minute(interval[1])
    elseif interval[1] < cursor then
      issues[#issues + 1] = "schedule " .. tostring(id) .. " overlap at " .. M.format_minute(interval[1])
    end
    if interval[2] > cursor then
      cursor = interval[2]
    end
  end
  if cursor < MINUTES_PER_DAY then
    issues[#issues + 1] = "schedule " .. tostring(id) .. " gap " .. M.format_minute(cursor) .. "..24:00"
  end
  return #issues == 0, issues
end

-- 校验全部 NPC 日程（含 NPC 引用的日程不存在/为空）。
function M.validate_all()
  local issues = {}
  for _, npc in ipairs(npcs.list()) do
    local schedule_id = npc.schedule
    if type(schedule_id) ~= "string" or schedule_id == "" then
      issues[#issues + 1] = "npc " .. tostring(npc.persist_id) .. " has no schedule"
    else
      local _, npc_issues = M.validate(schedule_id)
      for _, issue in ipairs(npc_issues) do
        issues[#issues + 1] = issue
      end
    end
  end
  return #issues == 0, issues
end

return M
