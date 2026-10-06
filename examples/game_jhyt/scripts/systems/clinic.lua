-- systems/clinic.lua：《江湖有田》医馆经营与疫情事件运行时
-- （docs/54 阶段3「医馆经营与疫情事件」，2026-10-06 2.0）。
--
-- 职责：
--   * 接诊（treat）：按病种处方扣药材 → 诊费入账 + 医馆声望 +，写
--     last_action="clinic_treat_<disease>"；药材不足/未建馆/闭馆拒绝，且**不写
--     last_action**（与商店 sell / 出货箱 not_enough 同一语义：失败动作不留痕）。
--   * 疫情状态机（on_day_start 挂点，由 main.lua 补丁接线；样例内显式调用）：
--       季节到点激活（一次性扣村民好感、诊费乘子上浮）→ 每日递减持续天数 →
--       玩家三选一决策（治疫/囤药/闭馆），决策写 last_action。
--   * 收益统计：累计接诊人数 / 诊费收入 / 分病种计数（图鉴/成就数据源）。
--   * 持久化：capture/restore/reset（save_manager 模式），version 门。
--
-- 声望/好感是**医馆自管**的运行期计数器（不写 player/relationship.lua 的阵营
-- 声望——那是云泽村/门派的既有锚点，本系统只读不扰）。两者随本 provider 入档。
local state = require("core.state")
local calendar = require("world.calendar")
local clinic_data = require("data.clinic")
-- inv 是引擎注入全局（与 systems/shipping.lua 同一用法）。
local inv = inv

local M = {}

local SAVE_VERSION = 1

-- 运行时状态（capture/restore 随存档；reset 清空）
local reputation = 0 -- 医馆声望（接诊累计）
local favor = 0      -- 村民好感（疫情被动扣减 / 治疫回补 / 囤药小罚）
local stats = {
  treated = 0,          -- 累计接诊人数
  income = 0,           -- 累计诊费收入（文）
  by_disease = {}       -- disease_id → 接诊次数
}
-- 疫情状态机：
--   active     是否正在疫情中
--   id         疫情条目 id（data.clinic.epidemics）
--   days_left  剩余天数（on_day_start 递减，归 0 自然消退）
--   decision   nil | "treat" | "stockpile" | "close"（一次疫情只许决策一次）
--   closed     闭馆中（treat 拒 clinic_closed；疫情消退自动复馆）
local epidemic = { active = false, id = nil, days_left = 0, decision = nil, closed = false }

-- systems.building 惰性引入（避免加载期循环依赖；运行期才查医馆是否建成）。
local function building_mod()
  local ok, mod = pcall(require, "systems.building")
  return ok and mod or nil
end

-- 医馆是否已建成（任一等级）；返回等级或 nil。纯读 systems.building.list()。
function M.built()
  local b = building_mod()
  if not b then
    return nil
  end
  for _, rec in ipairs(b.list() or {}) do
    if rec.building == "clinic" then
      return tonumber(rec.level) or 1
    end
  end
  return nil
end

-- 当前疫情的诊费乘子（无疫情 = 1）。
function M.fee_multiplier()
  if not epidemic.active then
    return 1.0
  end
  local def = epidemic.id and clinic_data.epidemic(epidemic.id) or nil
  return def and tonumber(def.fee_mult) or 1.0
end

-- 当前疫情是否闭馆。
function M.is_closed()
  return epidemic.closed == true
end

-- 接诊：按病种扣药材 → 收费 + 声望。
-- 返回 (true, {fee=, rep=}) 或 (false, reason)。
-- reason ∈ unknown_disease / no_clinic / clinic_closed / not_enough。
-- 失败一律不写 state.last_action。
function M.treat(disease_id)
  local def = clinic_data.disease(disease_id)
  if not def then
    return false, "unknown_disease"
  end
  if not M.built() then
    return false, "no_clinic"
  end
  if epidemic.closed then
    return false, "clinic_closed"
  end
  -- 先验药材（逐项），不足即拒——不扣任何一味。
  for _, cost in ipairs(def.recipe) do
    if (tonumber(inv.count(state.inventory, cost.item)) or 0) < cost.count then
      return false, "not_enough"
    end
  end
  -- 扣药材。
  for _, cost in ipairs(def.recipe) do
    inv.remove(state.inventory, cost.item, cost.count)
  end
  -- 诊费：基础 × 疫情乘子（四舍五入到整文）。
  local fee = math.floor((tonumber(def.fee) or 0) * M.fee_multiplier() + 0.5)
  local rep_gain = tonumber(def.rep) or 0
  state.money = (tonumber(state.money) or 0) + fee
  reputation = reputation + rep_gain
  stats.treated = stats.treated + 1
  stats.income = stats.income + fee
  stats.by_disease[disease_id] = (stats.by_disease[disease_id] or 0) + 1
  state.last_action = "clinic_treat_" .. tostring(disease_id)
  return true, { fee = fee, rep = rep_gain }
end

-- ---------- 疫情状态机 ----------

local function clear_epidemic()
  epidemic.active = false
  epidemic.id = nil
  epidemic.days_left = 0
  epidemic.decision = nil
  epidemic.closed = false
end

local function activate(id, def)
  epidemic.active = true
  epidemic.id = id
  epidemic.days_left = tonumber(def.duration) or 1
  epidemic.decision = nil
  epidemic.closed = false
  -- 被动惩罚：疫情爆发当日一次性扣村民好感（药材需求暴涨、村民惶恐）。
  favor = favor - (tonumber(def.favor_penalty) or 0)
end

-- 日挂点（main.lua on_day_start 补丁接线；验收样例显式调用）：
--   * 疫情进行中 → 递减剩余天数，归 0 自然消退（复馆）；
--   * 无疫情 → 按「季节 + 季内第几天」查 data.clinic.trigger_for，到点激活。
function M.on_day_start()
  if epidemic.active then
    epidemic.days_left = epidemic.days_left - 1
    if epidemic.days_left <= 0 then
      clear_epidemic()
    end
    return
  end
  local season = calendar.current()
  local dis = calendar.day_in_season(tonumber(clock.now().day) or 1)
  local id, def = clinic_data.trigger_for(season, dis)
  if id then
    activate(id, def)
  end
end

-- 测试/调试入口：立即激活指定疫情（不看季节/日期，不写 last_action）。
-- 验收样例用它在一个进程内反复走三决策分支；已在疫情中拒绝。
function M.force_trigger(id)
  if epidemic.active then
    return false, "already_active"
  end
  local def = clinic_data.epidemic(id)
  if not def then
    return false, "unknown_epidemic"
  end
  activate(id, def)
  return true
end

-- 疫情决策（一次疫情只许决策一次）。choice ∈ treat/stockpile/close。
-- 返回 (true) 或 (false, reason)；成功写 last_action。
--   treat     治疫：消耗疫情药材 → 声望大涨/好感回补，疫情提前平息；药材不足拒绝。
--   stockpile 囤药：立即兑现现金收益，好感小罚，疫情继续按日消退。
--   close     闭馆：期间停诊（treat 拒 clinic_closed），无收益无额外损失。
function M.decide(choice)
  if not epidemic.active then
    return false, "no_epidemic"
  end
  if epidemic.decision then
    return false, "already_decided"
  end
  local def = clinic_data.epidemic(epidemic.id)
  if choice == "treat" then
    for _, cost in ipairs(def.herbs or {}) do
      if (tonumber(inv.count(state.inventory, cost.item)) or 0) < cost.count then
        return false, "not_enough"
      end
    end
    for _, cost in ipairs(def.herbs or {}) do
      inv.remove(state.inventory, cost.item, cost.count)
    end
    reputation = reputation + (tonumber(def.treat and def.treat.rep) or 0)
    favor = favor + (tonumber(def.treat and def.treat.favor) or 0)
    clear_epidemic()
    state.last_action = "clinic_decision_treat"
    return true
  elseif choice == "stockpile" then
    local income = tonumber(def.stockpile and def.stockpile.income) or 0
    state.money = (tonumber(state.money) or 0) + income
    favor = favor + (tonumber(def.stockpile and def.stockpile.favor) or 0)
    epidemic.decision = "stockpile"
    state.last_action = "clinic_decision_stockpile"
    return true
  elseif choice == "close" then
    epidemic.closed = true
    epidemic.decision = "close"
    state.last_action = "clinic_decision_close"
    return true
  end
  return false, "bad_choice"
end

-- ---------- 只读访问器（验收/诊断/图鉴） ----------

function M.reputation()
  return reputation
end

function M.favor()
  return favor
end

function M.stats()
  return {
    treated = stats.treated,
    income = stats.income,
    by_disease = stats.by_disease
  }
end

-- 疫情状态快照（调用方只读）。
function M.status()
  local def = epidemic.id and clinic_data.epidemic(epidemic.id) or nil
  return {
    active = epidemic.active,
    id = epidemic.id,
    name = def and def.name or nil,
    days_left = epidemic.days_left,
    decision = epidemic.decision,
    closed = epidemic.closed,
    symptom = def and def.symptom or nil,
    fee_mult = def and def.fee_mult or 1.0
  }
end

-- ---------- 存档（save_manager 模式） ----------

function M.capture()
  local by_disease = {}
  for id, count in pairs(stats.by_disease) do
    if type(id) == "string" and count > 0 then
      by_disease[id] = math.floor(count)
    end
  end
  return {
    version = SAVE_VERSION,
    reputation = math.floor(reputation),
    favor = math.floor(favor),
    stats = {
      treated = math.floor(stats.treated),
      income = math.floor(stats.income),
      by_disease = by_disease
    },
    epidemic = {
      active = epidemic.active == true,
      id = epidemic.id,
      days_left = math.floor(epidemic.days_left),
      decision = epidemic.decision,
      closed = epidemic.closed == true
    }
  }
end

function M.restore(data_in)
  reputation = 0
  favor = 0
  stats = { treated = 0, income = 0, by_disease = {} }
  clear_epidemic()
  if type(data_in) ~= "table" then
    return
  end
  if tonumber(data_in.version) ~= SAVE_VERSION then
    return
  end
  reputation = math.floor(tonumber(data_in.reputation) or 0)
  favor = math.floor(tonumber(data_in.favor) or 0)
  if type(data_in.stats) == "table" then
    stats.treated = math.floor(tonumber(data_in.stats.treated) or 0)
    stats.income = math.floor(tonumber(data_in.stats.income) or 0)
    if type(data_in.stats.by_disease) == "table" then
      for id, count in pairs(data_in.stats.by_disease) do
        if type(id) == "string" and tonumber(count) and count > 0 then
          stats.by_disease[id] = math.floor(count)
        end
      end
    end
  end
  if type(data_in.epidemic) == "table" then
    local e = data_in.epidemic
    -- 恢复的疫情必须仍是数据表里存在的条目（旧档/脏数据丢弃）。
    if e.active == true and e.id and clinic_data.epidemic(e.id) then
      epidemic.active = true
      epidemic.id = e.id
      epidemic.days_left = math.floor(tonumber(e.days_left) or 1)
      epidemic.decision = e.decision
      epidemic.closed = e.closed == true
    end
  end
end

function M.reset()
  reputation = 0
  favor = 0
  stats = { treated = 0, income = 0, by_disease = {} }
  clear_epidemic()
end

-- ---------- 加载期自检 ----------
-- 纯系统模块：数据表在 data/clinic.lua（已登记 validate_data）。这里只守运行期
-- 不变量——没有需要自检的硬编码常量；恒返回通过。
function M.validate_all()
  return true, {}
end

return M
