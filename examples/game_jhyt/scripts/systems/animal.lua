-- systems/animal.lua：《江湖有田》畜牧（药园畜养，docs/33 §4.4，2026-10-06 1.0）。
--
-- 武侠化"药园畜养"：山庄畜舍（coop 鸡舍 / barn 畜棚 / stable 马厩）领养动物，
-- 每日饲喂/抚摸攒好感（mood 0..10），清晨 on_day_start 结算产出。
--
-- 设计要点：
--   * 拥有表 owned：按畜舍容量上限领养（容量表 HOUSE_CAPACITY 按建筑等级）。
--   * adopt：需对应畜舍已建成（systems.building.list 事实来源）且未满容。
--   * feed：每日一次，消耗饲料（data.animals.<id>.feed），fed=true 且 mood+1；
--     重复喂/缺料/无此动物均拒绝且不写 last_action（与商店/出货箱同纪律）。
--   * pet：每日一次抚摸，mood+1；重复抚摸拒绝。
--   * on_day_start：每个动物 cycle 递减；cycle 归 0 且"昨天喂过 + mood≥MIN"→
--     产物进背包；mood≥SPIRIT 时产特殊产物（灵蛋）替代普通产物。随后重置当日
--     fed/petted，晴天放牧 mood+1（雨天室内不放牧）。幂等可每日调用。
--   * 好感独立于 player/relationship（不共用 100/心口径），0..10 整数分档。
--   * capture/restore/reset 随 save_manager 持久；validate_all 纯自检返回字符串。
local state = require("core.state")
-- inv / weather 是引擎注入 / 世界模块（与 systems/shipping.lua 同一用法）。
local inv = inv
local animals_data = require("data.animals")

local M = {}

local STATE_VERSION = 1
local MAX_MOOD = 10
local MIN_PRODUCE_MOOD = 1 -- 吃饱且好感达标（≥1）才产
local SPIRIT_MOOD = 8 -- 好感极高（≥8）→ 特殊产物（灵蛋）

-- 畜舍容量（按建筑等级；house id == data/buildings.lua 的 coop/barn/stable）。
local HOUSE_CAPACITY = {
  coop = { [1] = 2, [2] = 4 },
  barn = { [1] = 2, [2] = 4 },
  stable = { [1] = 1, [2] = 2 }
}

-- 运行期（capture/restore 随存档；reset 清空）。
local owned = {} -- { { uid, animal_id, mood, fed, petted, cycle }, ... }
local next_uid = 1

local function def_of(animal_id)
  return animals_data.get(animal_id)
end

local function find(animal_id)
  for _, a in ipairs(owned) do
    if a.animal_id == animal_id then
      return a
    end
  end
  return nil
end

-- 该类畜舍已建成的最高等级记录（systems.building.list 事实来源）。
local function built_house(house_id)
  local building = require("systems.building")
  local best = nil
  for _, rec in ipairs(building.list() or {}) do
    if rec.building == house_id then
      if not best or (tonumber(rec.level) or 1) > (tonumber(best.level) or 1) then
        best = rec
      end
    end
  end
  return best
end

local function count_in_house(house_id)
  local n = 0
  for _, a in ipairs(owned) do
    local d = def_of(a.animal_id)
    if d and d.house == house_id then
      n = n + 1
    end
  end
  return n
end

-- ---------- 访问器（验收/诊断只读） ----------

function M.mood(animal_id)
  local a = find(animal_id)
  return a and a.mood or 0
end

function M.count()
  return #owned
end

function M.list()
  return owned
end

-- ---------- 领养 ----------
-- 需对应畜舍已建（等级≥1）且容量未满。失败不写 last_action。
function M.adopt(animal_id)
  local def = def_of(animal_id)
  if not def then
    return false, "unknown_animal"
  end
  local house = built_house(def.house)
  if not house then
    return false, "no_house"
  end
  local level = tonumber(house.level) or 1
  local cap = (HOUSE_CAPACITY[def.house] or {})[level] or 0
  if count_in_house(def.house) >= cap then
    return false, "no_space"
  end
  local rec = {
    uid = next_uid,
    animal_id = animal_id,
    mood = 0,
    fed = false,
    petted = false,
    cycle = tonumber(def.produce_cycle) or 1
  }
  next_uid = next_uid + 1
  owned[#owned + 1] = rec
  state.last_action = "adopt_" .. animal_id
  return true, rec
end

-- ---------- 喂食 ----------
-- 每日一次；消耗饲料；fed=true、mood+1。失败不写 last_action。
function M.feed(animal_id)
  local def = def_of(animal_id)
  local a = find(animal_id)
  if not def or not a then
    return false, "not_found"
  end
  if a.fed then
    return false, "already_fed"
  end
  local need = def.feed or { item = "rice", count = 1 }
  if (tonumber(inv.count(state.inventory, need.item)) or 0) < (tonumber(need.count) or 1) then
    return false, "not_enough"
  end
  inv.remove(state.inventory, need.item, tonumber(need.count) or 1)
  a.fed = true
  a.mood = math.min(MAX_MOOD, a.mood + 1)
  state.last_action = "feed_" .. animal_id
  return true, "ok"
end

-- ---------- 抚摸 ----------
-- 每日一次；mood+1。失败不写 last_action。
function M.pet(animal_id)
  local a = find(animal_id)
  if not a then
    return false, "not_found"
  end
  if a.petted then
    return false, "already_petted"
  end
  a.petted = true
  a.mood = math.min(MAX_MOOD, a.mood + 1)
  state.last_action = "pet_" .. animal_id
  return true, "ok"
end

-- ---------- 每日产出（main.on_day_start 挂点；样例也显式调用） ----------
function M.on_day_start()
  local weather = require("world.weather")
  local graze = not weather.is_rainy() -- 雨天室内不放牧
  for _, a in ipairs(owned) do
    local def = def_of(a.animal_id)
    if def then
      a.cycle = (tonumber(a.cycle) or 1) - 1
      if a.cycle <= 0 then
        -- 吃饱（昨天喂过）+ 好感达标 → 产物进背包。
        if a.fed and a.mood >= MIN_PRODUCE_MOOD then
          local pid = (def.spirit_produce and a.mood >= SPIRIT_MOOD) and def.spirit_produce
            or def.produce
          if pid then
            inv.add(state.inventory, pid, 1)
          end
        end
        a.cycle = tonumber(def.produce_cycle) or 1
      end
      a.fed = false
      a.petted = false
      if graze then
        a.mood = math.min(MAX_MOOD, a.mood + 1)
      end
    end
  end
end

-- ---------- 存档（save_manager 模式） ----------

function M.capture()
  local list = {}
  for _, a in ipairs(owned) do
    list[#list + 1] = {
      uid = a.uid,
      animal_id = a.animal_id,
      mood = a.mood,
      fed = a.fed,
      petted = a.petted,
      cycle = a.cycle
    }
  end
  return { version = STATE_VERSION, next_uid = next_uid, list = list }
end

function M.restore(data_in)
  owned = {}
  next_uid = 1
  if type(data_in) ~= "table" then
    return
  end
  if tonumber(data_in.version) ~= STATE_VERSION then
    return
  end
  next_uid = tonumber(data_in.next_uid) or 1
  for _, e in ipairs(data_in.list or {}) do
    if type(e) == "table" and def_of(e.animal_id) then
      owned[#owned + 1] = {
        uid = tonumber(e.uid) or next_uid,
        animal_id = e.animal_id,
        mood = math.max(0, math.min(MAX_MOOD, tonumber(e.mood) or 0)),
        fed = e.fed == true,
        petted = e.petted == true,
        cycle = tonumber(e.cycle) or 1
      }
    end
  end
end

function M.reset()
  owned = {}
  next_uid = 1
end

-- ---------- 加载期自检 ----------
-- 返回 ""（通过）或换行分隔的问题串（与 systems/mastery / shipping 同契约）。
function M.validate_all()
  local report = {}
  local items = require("data.items")
  local tools = require("data.tools")
  local function known(id)
    return items[id] ~= nil or tools[id] ~= nil
  end
  for id, def in pairs(animals_data.animals or {}) do
    local p = "animal:" .. tostring(id)
    if type(def.name) ~= "string" or def.name == "" then
      report[#report + 1] = p .. ": name 缺失"
    end
    if not HOUSE_CAPACITY[def.house] then
      report[#report + 1] = p .. ": unknown house " .. tostring(def.house)
    end
    if not known(def.produce) then
      report[#report + 1] = p .. ": unknown produce " .. tostring(def.produce)
    end
    if def.spirit_produce and not known(def.spirit_produce) then
      report[#report + 1] = p .. ": unknown spirit_produce " .. tostring(def.spirit_produce)
    end
    if type(def.produce_cycle) ~= "number" or def.produce_cycle < 1
      or def.produce_cycle ~= math.floor(def.produce_cycle) then
      report[#report + 1] = p .. ": produce_cycle 须为正整数"
    end
    if type(def.feed) ~= "table" or not known(def.feed.item) then
      report[#report + 1] = p .. ": feed 引用了不存在的物品"
    end
  end
  return table.concat(report, "\n")
end

return M
