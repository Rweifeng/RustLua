-- systems/fishing.lua：《江湖有田》钓鱼系统（docs/33 §4.5，2026-10-06 1.1）。
--
-- 数据口径 = docs/33 §4.5 == data/fish.lua（水域/鱼种）== data/items_fish.lua
-- （鱼物品 + 钓竿工具）== 本模块（下竿判定/体力时间消耗/存档/校验）。
--
-- 设计要点：
--   * 钓竿是"手工具"，与锄/壶/斧/镐共用同一个 hand-tool（tool）槽——装备钓竿会
--     顶掉锄头，收竿后切回（与既有工具同槽切换，不另设"钓竿"槽）。判定走
--     inv.equip_list(state.inventory).tool 的 family == "fishing_rod"。
--   * cast(water_id, opts)：读装备竿阶 → 钓点季节/天气闸 → 目标鱼季节/时段闸 →
--     竿阶难度闸 → 体力/时间消耗 → 入包。成功写 last_action=catch_<id>；
--     失败写 last_action=none 并返回原因（unknown_water/season_closed/weather_closed/
--     no_rod/time_closed/rod_too_low/tired/no_bite）。
--   * 竿阶门槛：竿阶 t 可钓 difficulty <= t*2 的鱼（t=1→≤2、t=2→≤4、t=3→≤5）。
--     低阶竿钓高阶鱼直接失败（rod_too_low）。
--   * 临时钓点 flood_pool（雨季涨水）：仅夏季 + 雨天开放，on_day_start 重算开放表。
--   * headless 可测性：opts.force="<fish_id>" 直接指定目标鱼（仍过全部闸），
--     让"固定钓某鱼 / 负例"断言不依赖随机分布；不传 force 时从当前可钓鱼池取第一条。
local state = require("core.state")
local inv = inv
local fish_data = require("data.fish")
local tools_base = require("data.tools")
local base_items = require("data.items")
local ext = require("data.items_fish")
local calendar = require("world.calendar")
local weather = require("world.weather")

local M = {}

local CAST_MINUTES = 10 -- 每次抛竿推进的当日分钟数（消耗时间）

-- 运行时状态（capture/restore 随存档；reset 清空）。
local stats = {} -- fish_id → 累计钓获条数（图鉴/成就数据源）

-- ---------- 数据访问 ----------

function M.waters()
  return fish_data.waters
end

function M.fish()
  return fish_data.fish
end

-- 工具定义合并：data/tools.lua 主表 + data/items_fish.lua ext.tools（钓竿在扩展里）。
local function tool_def(id)
  if type(id) ~= "string" or id == "" then
    return nil
  end
  return tools_base[id] or (ext.tools or {})[id]
end

-- 当前装备的钓竿：返回 (tier, def)；未装/装的不是钓竿 → nil。
function M.rod()
  local equipped = inv.equip_list(state.inventory) or {}
  local rod_id = equipped.tool
  local def = rod_id and tool_def(rod_id)
  if not def or def.family ~= "fishing_rod" then
    return nil
  end
  return tonumber(def.tier) or 1, def
end

local function in_list(list, value)
  if type(list) ~= "table" then
    return true -- nil/空 = 不限
  end
  for _, v in ipairs(list) do
    if v == value then
      return true
    end
  end
  return false
end

local function in_window(hours, minute)
  if type(hours) ~= "table" then
    return true -- nil = 全天
  end
  local m = tonumber(minute) or 0
  local from = tonumber(hours.from) or 0
  local to = tonumber(hours.to) or 1440
  return m >= from and m < to
end

-- 钓点当前是否开放（季节闸 + 天气闸）。返回 (open, reason)。
function M.water_open(water_id)
  local water = fish_data.waters[water_id]
  if not water then
    return false, "unknown_water"
  end
  local season = calendar.current()
  if not in_list(water.season, season) then
    return false, "season_closed"
  end
  if water.weather == "rainy" and not weather.is_rainy() then
    return false, "weather_closed"
  end
  return true, "ok"
end

-- 某鱼在当前时刻是否可钓（归属钓点 + 季节 + 时段）。不查竿阶（那是 cast 的闸）。
function M.fish_open(fish_id)
  local f = fish_data.fish[fish_id]
  if not f then
    return false, "unknown_fish"
  end
  local open, reason = M.water_open(f.water)
  if not open then
    return false, reason
  end
  local season = calendar.current()
  if not in_list(f.season, season) then
    return false, "season_closed"
  end
  if not in_window(f.hours, clock.now().minute) then
    return false, "time_closed"
  end
  return true, "ok"
end

-- 当前钓点在竿阶下可钓的鱼列表（季节/时段/天气/竿阶难度全过滤，按 id 排序稳定）。
function M.catchable(water_id)
  local open = M.water_open(water_id)
  if not open then
    return {}
  end
  local tier = (M.rod()) or 0
  local season = calendar.current()
  local minute = clock.now().minute
  local out = {}
  for id, f in pairs(fish_data.fish) do
    if f.water == water_id
      and in_list(f.season, season)
      and in_window(f.hours, minute)
      and f.difficulty <= tier * 2
    then
      out[#out + 1] = f
    end
  end
  table.sort(out, function(a, b)
    return a.id < b.id
  end)
  return out
end

-- ---------- 抛竿 ----------

-- cast(water_id, opts)：opts.force = 强制目标鱼 id（headless/调试，仍过全部闸）。
-- 返回 (true, fish_id) 或 (false, reason)。成功消耗体力与时间、入包、写 last_action。
function M.cast(water_id, opts)
  opts = opts or {}
  local water = fish_data.waters[water_id]
  if not water then
    state.last_action = "none"
    return false, "unknown_water"
  end
  -- 钓点闸：季节 / 天气（临时钓点）
  local open, reason = M.water_open(water_id)
  if not open then
    state.last_action = "none"
    return false, reason
  end
  -- 装备竿闸
  local tier, rod_def = M.rod()
  if not tier then
    state.last_action = "none"
    return false, "no_rod"
  end
  -- 确定目标鱼
  local target
  if type(opts.force) == "string" and opts.force ~= "" then
    local f = fish_data.fish[opts.force]
    if not f or f.water ~= water_id then
      state.last_action = "none"
      return false, "unknown_fish"
    end
    -- 强制目标仍过季节/时段闸（负例：非当季鱼出不来）
    local fok, freason = M.fish_open(opts.force)
    if not fok then
      state.last_action = "none"
      return false, freason
    end
    -- 竿阶难度闸（负例：低阶竿钓高阶鱼失败）
    if f.difficulty > tier * 2 then
      state.last_action = "none"
      return false, "rod_too_low"
    end
    target = f
  else
    local pool = M.catchable(water_id)
    if #pool == 0 then
      state.last_action = "none"
      return false, "no_bite"
    end
    target = pool[1]
  end
  -- 体力消耗（确定有鱼之后才扣，空竿不耗体力）
  local cost = tonumber(rod_def.energy) or 0
  if (tonumber(state.energy) or 0) < cost then
    state.last_action = "none"
    return false, "tired"
  end
  state.energy = (tonumber(state.energy) or 0) - cost
  -- 时间消耗
  local now = clock.now()
  clock.set(tonumber(now.day) or 1, (tonumber(now.minute) or 0) + CAST_MINUTES)
  -- 入包 + 累计
  inv.add(state.inventory, target.id, 1)
  stats[target.id] = (stats[target.id] or 0) + 1
  state.last_action = "catch_" .. target.id
  return true, target.id
end

-- 累计钓获：fish_id → 条数（图鉴/成就数据源）。
function M.stats()
  return stats
end

-- 当日开放钓点表（on_day_start 挂点 / UI 用）：按当前季节+天气重算。
function M.active_waters()
  local out = {}
  for id, water in pairs(fish_data.waters) do
    local open = M.water_open(id)
    if open then
      out[#out + 1] = id
    end
  end
  table.sort(out)
  return out
end

-- 跨天钩子（main.on_day_start 挂点）：雨季涨水临时钓点随天气刷新。
-- 纯派生（读 weather/season，不落额外状态），这里显式调用一次以便挂点闭环。
function M.on_day_start()
  return M.active_waters()
end

-- ---------- 存档 ----------

function M.capture()
  local stats_data = {}
  for id, count in pairs(stats) do
    if type(id) == "string" and count > 0 and fish_data.fish[id] then
      stats_data[id] = math.floor(count)
    end
  end
  return { stats = stats_data }
end

function M.restore(data_in)
  stats = {}
  if type(data_in) ~= "table" then
    return
  end
  if type(data_in.stats) == "table" then
    for id, count in pairs(data_in.stats) do
      if type(id) == "string" and tonumber(count) and count > 0 and fish_data.fish[id] then
        stats[id] = math.floor(count)
      end
    end
  end
end

function M.reset()
  stats = {}
end

-- ---------- 加载期校验 ----------

local SEASONS = { spring = true, summer = true, autumn = true, winter = true }

function M.validate_all()
  local report = {}
  local waters = fish_data.waters or {}
  local fish = fish_data.fish or {}
  local ext_items = (ext.items or {})
  local ext_tools = (ext.tools or {})

  -- 水域：4 处、id 自洽
  local water_count = 0
  for id, w in pairs(waters) do
    water_count = water_count + 1
    if id ~= w.id then
      report[#report + 1] = "fishing: 水域 id 不自洽 " .. tostring(id)
    end
    if type(w.name) ~= "string" or w.name == "" then
      report[#report + 1] = "fishing: 水域 " .. tostring(id) .. " 缺 name"
    end
    if w.season then
      for _, s in ipairs(w.season) do
        if not SEASONS[s] then
          report[#report + 1] = "fishing: 水域 " .. tostring(id) .. " 未知季节 " .. tostring(s)
        end
      end
    end
  end
  if water_count ~= 4 then
    report[#report + 1] = "fishing: 水域应 4 处，现有 " .. tostring(water_count)
  end

  -- 鱼种：20+、字段合法、归属水域存在、sell 与 ext 物品一致、覆盖四季
  local fish_count = 0
  local season_seen = {}
  for id, f in pairs(fish) do
    fish_count = fish_count + 1
    if id ~= f.id then
      report[#report + 1] = "fishing: 鱼种 id 不自洽 " .. tostring(id)
    end
    if not waters[f.water] then
      report[#report + 1] = "fishing: " .. tostring(id) .. " 归属未知水域 " .. tostring(f.water)
    end
    if type(f.difficulty) ~= "number" or f.difficulty < 1 or f.difficulty > 5 then
      report[#report + 1] = "fishing: " .. tostring(id) .. " difficulty 须 1..5"
    end
    if type(f.sell) ~= "number" or f.sell <= 0 then
      report[#report + 1] = "fishing: " .. tostring(id) .. " sell 须为正数"
    end
    -- sell 双写漂移：data/fish.lua 与 data/items_fish.lua 同名物品必须一致。
    -- 撞名守护例外：鱼 id 与既有物品撞名（典型：桃花鱼 fish.lua id="peach" 与果树
    -- 桃子 items.lua:peach）时，items_fish 生成物品表会跳过（保留既有物品），
    -- 鱼仍可钓（判定走 fish_data），但渔获物品解析回主表既有定义——此时不判
    -- "缺物品定义"，也不做 sell/kind 双写比对（该鱼的实际入包物由主表定义决定）。
    local item_def = ext_items[id]
    local collided = item_def == nil and base_items[id] ~= nil
    if not item_def and not collided then
      report[#report + 1] = "fishing: 鱼 " .. tostring(id) .. " 在 items_fish 缺物品定义"
    elseif not collided then
      if item_def.sell ~= f.sell then
        report[#report + 1] = "fishing: " .. tostring(id) .. " sell 双写漂移 fish="
          .. tostring(f.sell) .. " item=" .. tostring(item_def.sell)
      end
      if item_def.kind ~= "fish" then
        report[#report + 1] = "fishing: " .. tostring(id) .. " 物品 kind 应为 fish"
      end
    end
    if f.season then
      for _, s in ipairs(f.season) do
        if not SEASONS[s] then
          report[#report + 1] = "fishing: " .. tostring(id) .. " 未知季节 " .. tostring(s)
        end
        season_seen[s] = true
      end
    else
      season_seen["__all_year"] = true
    end
  end
  if fish_count < 20 then
    report[#report + 1] = "fishing: 鱼种须 20+，现有 " .. tostring(fish_count)
  end

  -- 钓竿：3 根、family fishing_rod、tier 1..3、energy 整数 >= 0
  local rod_count = 0
  for id, t in pairs(ext_tools) do
    if t.family == "fishing_rod" then
      rod_count = rod_count + 1
      if type(t.tier) ~= "number" or t.tier < 1 or t.tier > 3 then
        report[#report + 1] = "fishing: 钓竿 " .. tostring(id) .. " tier 须 1..3"
      end
      if type(t.energy) ~= "number" or t.energy < 0 or t.energy ~= math.floor(t.energy) then
        report[#report + 1] = "fishing: 钓竿 " .. tostring(id) .. " energy 须整数 >= 0"
      end
    end
  end
  if rod_count ~= 3 then
    report[#report + 1] = "fishing: 钓竿应 3 根（tier 1..3），现有 " .. tostring(rod_count)
  end

  return table.concat(report, "\n")
end

return M
