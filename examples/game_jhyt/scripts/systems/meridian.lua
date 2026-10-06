-- systems/meridian.lua：经脉修炼系统（docs/33 §7.3，2026-10-06 1.9；
-- 改造计划 docs/54 §7.1）。
--
-- 玩法闭环（docs/54 §7.1：冲穴成功大进、失败反噬小扣）：
--   * try_open(id)：前置经脉已通 + 武学熟练度达标 → 消耗内力 + 冲穴材料 → 按成功率掷骰。
--     成功：经脉永久冲通（opened 进存档），被动加成实时生效；
--     失败：内力与材料照耗、进度不涨（反噬小扣），last_action = meridian_fail_<id>。
--   * 拒绝路径（未知/已通/前置未达/熟练度不足/内力不足/材料不足）一律不扣内力、
--     不扣材料、不写 last_action（与商店 sell / shipping.add 的失败语义一致）。
--
-- 被动加成「读时聚合、写时应用」：
--   * bonus() 聚合已通经脉的 passive → { attack, defense, energy_max, inner_power_max }；
--   * attack/defense：经 player.stats 生效——apply_passive() 把聚合值写成 days=1 的
--     永久 Buff（stats.query("player", key) 即战斗读数）；每日 on_day_start 在
--     stats.on_day_start() 衰减之后重挂一次，等价于常驻；
--   * energy_max/inner_power_max：经 state 生效——max_energy()/max_inner_power() =
--     inventory 基础上限 + 聚合加成；跨天恢复池（main.on_day_start）读这两个查询。
--
-- 随机数：内部 LCG（meridian_rng 存 state.vars，随存档持久），同一存档同一冲穴顺序
-- → 同一成败（headless 可断言）；set_seed 供验收固定序列（同 loot.lua 范式）。
local state = require("core.state")
local data = require("data.meridian")
local stats = require("player.stats")
local mastery = require("systems.mastery")

local M = {}

local opened = {} -- [meridian_id] = true

-- 内力每日自然恢复量（on_day_start 挂点；补丁见交付说明）。
local DAILY_REGEN = 30

-- LCG（numerical recipes 32 位线性同余，与 systems/loot.lua 同一套）。
local RNG_VAR = "meridian_rng"
local DEFAULT_SEED = 20261006
local RNG_MOD = 2147483648
local RNG_MUL = 1103515245
local RNG_ADD = 12345

local PASSIVE_KEYS = { attack = true, defense = true, energy_max = true, inner_power_max = true }

local function meridians()
  return data.meridians or {}
end

local function by_id(id)
  if type(id) ~= "string" then
    return nil
  end
  return meridians()[id]
end

-- 派生口径（单一事实来源，勿散到调用方）：
local function inner_cost(def)
  return 10 + ((tonumber(def.difficulty) or 1) - 1) * 5
end

local function success_rate(def)
  local rate = 0.90 - ((tonumber(def.difficulty) or 1) - 1) * 0.05
  if rate < 0.35 then
    rate = 0.35
  end
  return rate
end

local function req_combat(def)
  return (tonumber(def.difficulty) or 1) - 1
end

-- ---------- 确定性 RNG ----------

local function next_unit()
  local seed = tonumber(state.get_var(RNG_VAR)) or DEFAULT_SEED
  seed = (seed * RNG_MUL + RNG_ADD) % RNG_MOD
  state.set_var(RNG_VAR, seed, "meridian", "roll")
  return seed / RNG_MOD
end

--- 固定随机序列（headless / 调试）：只影响之后的冲穴掷骰。
function M.set_seed(seed)
  state.set_var(RNG_VAR, math.floor(tonumber(seed) or DEFAULT_SEED), "meridian", "set_seed")
  return M.seed()
end

function M.seed()
  return tonumber(state.get_var(RNG_VAR)) or DEFAULT_SEED
end

-- ---------- 查询 ----------

function M.is_open(id)
  return opened[id] == true
end

--- 已通经脉 id 数组（按 difficulty 升序，只读快照）。
function M.opened_list()
  local out = {}
  for id in pairs(opened) do
    out[#out + 1] = id
  end
  table.sort(out, function(a, b)
    return (tonumber(by_id(a).difficulty) or 99) < (tonumber(by_id(b).difficulty) or 99)
  end)
  return out
end

--- 聚合被动加成：{ attack=, defense=, energy_max=, inner_power_max= }。
function M.bonus()
  local out = { attack = 0, defense = 0, energy_max = 0, inner_power_max = 0 }
  for id, def in pairs(meridians()) do
    if opened[id] and type(def.passive) == "table" then
      for key, amount in pairs(def.passive) do
        if PASSIVE_KEYS[key] then
          out[key] = (tonumber(out[key]) or 0) + (tonumber(amount) or 0)
        end
      end
    end
  end
  return out
end

local function inventory()
  return require("player.inventory")
end

--- 体力/内力上限 = inventory 基础上限 + 经脉聚合加成（跨天恢复池唯一读数口）。
function M.max_energy()
  return inventory().max_energy() + (tonumber(M.bonus().energy_max) or 0)
end

function M.max_inner_power()
  return inventory().max_inner_power() + (tonumber(M.bonus().inner_power_max) or 0)
end

--- 冲穴消耗/成功率/门槛（只读，UI 与验收展示用）。
function M.info(id)
  local def = by_id(id)
  if not def then
    return nil
  end
  return {
    id = def.id,
    name = def.name,
    difficulty = tonumber(def.difficulty) or 1,
    unlock_condition = def.unlock_condition,
    passive = def.passive,
    mat = def.mat,
    inner_cost = inner_cost(def),
    success_rate = success_rate(def),
    req_combat = req_combat(def)
  }
end

-- ---------- 被动应用 ----------

-- 把 attack/defense 聚合值写进 stats（days=1，每日 on_day_start 重挂）。
-- 幂等：stats.add_buff 对同 (stat, amount) 只续期不叠加。
function M.apply_passive()
  local b = M.bonus()
  for _, stat in ipairs({ "attack", "defense" }) do
    local amount = tonumber(b[stat]) or 0
    if amount > 0 then
      stats.add_buff("player", { stat = stat, amount = amount, days = 1 })
    end
  end
end

-- ---------- 冲穴 ----------

-- 前置检查（不产生任何副作用）：返回 (ok, reason)；
-- reason ∈ unknown_meridian/already_opened/prereq/req_mastery/not_enough_inner/not_enough_material。
function M.can_try(id)
  local def = by_id(id)
  if not def then
    return false, "unknown_meridian"
  end
  if opened[id] then
    return false, "already_opened"
  end
  local prereq = def.unlock_condition
  if prereq and not opened[prereq] then
    return false, "prereq"
  end
  if mastery.level("combat") < req_combat(def) then
    return false, "req_mastery"
  end
  if (tonumber(state.inner_power) or 0) < inner_cost(def) then
    return false, "not_enough_inner"
  end
  for item_id, count in pairs(def.mat or {}) do
    if (tonumber(inv.count(state.inventory, item_id)) or 0) < (tonumber(count) or 0) then
      return false, "not_enough_material"
    end
  end
  return true, "ok"
end

--- 冲穴一次。
-- 拒绝路径（can_try 不过）：返回 false,reason，**不扣内力/材料、不写 last_action**。
-- 掷骰路径：先扣内力与材料（成败皆耗），再按成功率掷骰：
--   成功 → (true, "opened")，经脉冲通并即时 apply_passive，last_action = meridian_<id>；
--   失败 → (true, "failed")，进度不涨，last_action = meridian_fail_<id>。
function M.try_open(id)
  local ok, reason = M.can_try(id)
  if not ok then
    return false, reason
  end
  local def = by_id(id)
  state.inner_power = (tonumber(state.inner_power) or 0) - inner_cost(def)
  for item_id, count in pairs(def.mat or {}) do
    inv.remove(state.inventory, item_id, tonumber(count) or 1)
  end
  if next_unit() < success_rate(def) then
    opened[id] = true
    M.apply_passive()
    state.last_action = "meridian_" .. id
    return true, "opened"
  end
  state.last_action = "meridian_fail_" .. id
  return true, "failed"
end

-- ---------- 每日挂点 ----------

-- 内力每日自然恢复（挂 main.on_day_start，须在 stats.on_day_start() 之后调用：
-- 先让旧 Buff 衰减，再由 apply_passive 重挂经脉常驻加成）。返回本次恢复量。
function M.on_day_start()
  local maxip = M.max_inner_power()
  local before = tonumber(state.inner_power) or 0
  local after = math.min(maxip, before + DAILY_REGEN)
  state.inner_power = after
  M.apply_passive()
  return after - before
end

-- ---------- 存档 ----------

function M.capture()
  local list = {}
  for _, id in ipairs(M.opened_list()) do
    list[#list + 1] = id
  end
  return { opened = list }
end

function M.restore(data_in)
  opened = {}
  if type(data_in) ~= "table" then
    return
  end
  if type(data_in.opened) == "table" then
    for _, id in ipairs(data_in.opened) do
      if by_id(id) then
        opened[id] = true
      end
    end
  end
  M.apply_passive()
end

function M.reset()
  opened = {}
end

-- ---------- 加载期校验 ----------

function M.validate_all()
  local report = {}
  local table_items = require("data.items")
  local list = meridians()
  local count = 0
  local root_count = 0
  local difficulties = {}
  for id, def in pairs(list) do
    count = count + 1
    if type(id) ~= "string" or type(def) ~= "table" then
      report[#report + 1] = "meridian: 条目标识非法"
    end
    if type(def.name) ~= "string" or def.name == "" then
      report[#report + 1] = "meridian: " .. tostring(id) .. " 缺 name"
    end
    local diff = tonumber(def.difficulty)
    if not diff or diff < 1 or diff > 12 or math.floor(diff) ~= diff then
      report[#report + 1] = "meridian: " .. tostring(id) .. " difficulty 须为 1..12 整数"
    else
      difficulties[diff] = (difficulties[diff] or 0) + 1
    end
    if def.unlock_condition == nil then
      root_count = root_count + 1
    elseif not by_id(def.unlock_condition) then
      report[#report + 1] = "meridian: " .. tostring(id) .. " 前置不存在 " .. tostring(def.unlock_condition)
    end
    if type(def.passive) ~= "table" then
      report[#report + 1] = "meridian: " .. tostring(id) .. " 缺 passive"
    else
      for key, amount in pairs(def.passive) do
        if not PASSIVE_KEYS[key] then
          report[#report + 1] = "meridian: " .. tostring(id) .. " 非法被动键 " .. tostring(key)
        end
        if not tonumber(amount) or tonumber(amount) <= 0 then
          report[#report + 1] = "meridian: " .. tostring(id) .. " 被动值须为正数"
        end
      end
    end
    if type(def.mat) == "table" then
      for item_id, item_count in pairs(def.mat) do
        if not table_items[item_id] then
          report[#report + 1] = "meridian: " .. tostring(id) .. " 材料未注册 " .. tostring(item_id)
        end
        if not tonumber(item_count) or tonumber(item_count) <= 0 then
          report[#report + 1] = "meridian: " .. tostring(id) .. " 材料份数非法 " .. tostring(item_id)
        end
      end
    end
  end
  if count ~= 12 then
    report[#report + 1] = "meridian: 须恰 12 条，现有 " .. tostring(count)
  end
  if root_count ~= 1 then
    report[#report + 1] = "meridian: 恰需 1 条根经脉（无前置），现有 " .. tostring(root_count)
  end
  for diff, n in pairs(difficulties) do
    if n ~= 1 then
      report[#report + 1] = "meridian: difficulty " .. tostring(diff) .. " 重复 " .. tostring(n) .. " 次"
    end
  end
  -- 前置链无环（拓扑：从所有无根前置的根节点出发传播，最后应覆盖全部）。
  local reachable = {}
  for _, def in pairs(list) do
    if def.unlock_condition == nil then
      reachable[def.id] = true
    end
  end
  local changed = true
  while changed do
    changed = false
    for _, def in pairs(list) do
      local prereq = def.unlock_condition
      if prereq and reachable[prereq] and not reachable[def.id] then
        reachable[def.id] = true
        changed = true
      end
    end
  end
  for id in pairs(list) do
    if not reachable[id] then
      report[#report + 1] = "meridian: 前置链不可达根（有环或悬空）" .. tostring(id)
      break
    end
  end
  return table.concat(report, "\n")
end

return M
