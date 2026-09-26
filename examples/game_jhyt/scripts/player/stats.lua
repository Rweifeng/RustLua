-- stats.lua：属性模型（M18-04）。
--
-- 三层计算，互不重复叠加：
--   * base      角色基础值（玩家 "player" 用模块内 BASE_DEFAULTS；
--               其余 actor 若在 data.characters 有 stats 字段则取之，未知 actor 全 0）
--   * equipment 装备槽加成：遍历 inv.equip_list 各槽位物品 id，取
--               data.items / data.tools 定义的 stats 表求和；兼容旧字段——
--               定义无 stats 但有 damage 时视为 stats.attack（练习剑 damage=8）
--   * buff      临时 Buff：add_buff 写入、按游戏日过期（on_day_start 递减 days）
--
-- query(actor, key) = base[key] + equipment[key] + buff[key]（未知 key 返回 0）。
--
-- 数值锚点（与 M18-01 一致，勿破坏）：玩家 attack 基础必须为 2，练习剑装备层 8，
-- strike 基础伤害 2 → 装剑普攻 2+8=10。skills.lua 的武器加成走
-- query - base（即 equipment+buff 层），基础层已含在招式 damage 的数值约定里，
-- 不在此重复叠加。
--
-- 全部纯 Lua、可 headless；capture/restore 只存 buffs（actor→数组），
-- restore 对旧档缺字段（data 非 table / 无 buffs）静默用默认（清空）。
local state = require("core.state")

local M = {}

-- 新档基础属性（docs 数值案锚点：attack=2 保证练习剑后 strike=10）。
M.BASE_DEFAULTS = { attack = 2, defense = 1 }

-- 合法属性键：query/add_buff 只接受这些键，未知键一律 0/拒绝。
local STAT_KEYS = { "attack", "defense" }
local VALID_STATS = { attack = true, defense = true }

-- actor → { { stat=, amount=, days= }, ... }（运行时态，随存档 capture/restore）。
local buffs = {}

local function copy_table(source)
  local copy = {}
  if type(source) == "table" then
    for key, value in pairs(source) do
      copy[key] = value
    end
  end
  return copy
end

function M.reset()
  buffs = {}
end

-- 基础层：玩家用 BASE_DEFAULTS；data.characters 中带 stats 字段的角色取其值；
-- 未知 actor 返回全 0 值表。每次返回新表，调用方改动不污染模型。
function M.base(actor)
  local base = {}
  if actor == "player" then
    for key, value in pairs(M.BASE_DEFAULTS) do
      base[key] = value
    end
    return base
  end
  local characters = require("data.characters")
  local entry = type(actor) == "string" and characters.characters[actor] or nil
  for _, key in ipairs(STAT_KEYS) do
    base[key] = 0
  end
  if entry and type(entry.stats) == "table" then
    for _, key in ipairs(STAT_KEYS) do
      base[key] = tonumber(entry.stats[key]) or 0
    end
  end
  return base
end

-- 装备层：玩家背包各装备槽物品的 stats 求和；旧字段 damage → stats.attack。
-- 属性模型当前只挂玩家背包（state.inventory），其余 actor 返回空表。
function M.equipment_bonus(actor)
  local total = {}
  if actor ~= "player" or not inv.exists(state.inventory) then
    return total
  end
  local items = require("data.items")
  local tools = require("data.tools")
  for _, item_id in pairs(inv.equip_list(state.inventory)) do
    local definition = items[item_id] or tools[item_id]
    if type(definition) == "table" then
      if type(definition.stats) == "table" then
        for key, value in pairs(definition.stats) do
          total[key] = (tonumber(total[key]) or 0) + (tonumber(value) or 0)
        end
      elseif definition.damage ~= nil then
        total.attack = (tonumber(total.attack) or 0) + (tonumber(definition.damage) or 0)
      end
    end
  end
  return total
end

-- Buff 层：actor 身上 stat 键的活跃 Buff 加成之和（无则 0）。
function M.buff_bonus(actor, key)
  local total = 0
  local list = buffs[actor]
  if type(list) ~= "table" then
    return total
  end
  for _, buff in ipairs(list) do
    if buff.stat == key then
      total = total + (tonumber(buff.amount) or 0)
    end
  end
  return total
end

-- 三层合成查询：未知 key 返回 0（不查表、不抛错）。
-- 当前生效的 Buff 数量（HUD 状态图标：有 buff 才画图标）。
function M.buff_count(actor)
  local list = buffs[actor]
  if type(list) ~= "table" then
    return 0
  end
  local n = 0
  for _ in ipairs(list) do
    n = n + 1
  end
  return n
end

function M.query(actor, key)
  if not VALID_STATS[key] then
    return 0
  end
  local base = M.base(actor)
  local equipment = M.equipment_bonus(actor)
  return (tonumber(base[key]) or 0)
    + (tonumber(equipment[key]) or 0)
    + M.buff_bonus(actor, key)
end

-- 添加临时 Buff：{ stat=string, amount=number, days=正整数 }。
-- 非法参数返回 false,reason（bad_actor/bad_buff/bad_stat/bad_amount/bad_days）；
-- 同 actor 同 stat+amount 的 Buff 不叠加重复条目（幂等），仅续期到较大 days。
function M.add_buff(actor, buff)
  if type(actor) ~= "string" or actor == "" then
    return false, "bad_actor"
  end
  if type(buff) ~= "table" then
    return false, "bad_buff"
  end
  if type(buff.stat) ~= "string" or not VALID_STATS[buff.stat] then
    return false, "bad_stat"
  end
  local amount = tonumber(buff.amount)
  if not amount then
    return false, "bad_amount"
  end
  local days = tonumber(buff.days)
  if not days or days < 1 or math.floor(days) ~= days then
    return false, "bad_days"
  end
  local list = buffs[actor]
  if type(list) ~= "table" then
    list = {}
    buffs[actor] = list
  end
  for _, existing in ipairs(list) do
    if existing.stat == buff.stat and (tonumber(existing.amount) or 0) == amount then
      existing.days = math.max(tonumber(existing.days) or 0, days)
      return true
    end
  end
  list[#list + 1] = { stat = buff.stat, amount = amount, days = days }
  return true
end

-- 游戏日开始：每个 Buff days-1，归 0 移除；返回移除数。
function M.on_day_start()
  local removed = 0
  for _, list in pairs(buffs) do
    local keep = {}
    for _, buff in ipairs(list) do
      buff.days = (tonumber(buff.days) or 0) - 1
      if buff.days > 0 then
        keep[#keep + 1] = buff
      else
        removed = removed + 1
      end
    end
    for index = 1, #keep do
      list[index] = keep[index]
    end
    for index = #list, #keep + 1, -1 do
      list[index] = nil
    end
  end
  return removed
end

-- 只读快照：actor 的活跃 Buff 副本（headless 验收 / 调试 UI 用）。
function M.list(actor)
  local out = {}
  local list = buffs[actor]
  if type(list) ~= "table" then
    return out
  end
  for index, buff in ipairs(list) do
    out[index] = { stat = buff.stat, amount = tonumber(buff.amount) or 0, days = tonumber(buff.days) or 0 }
  end
  return out
end

-- 存档：只存 buffs（actor→数组），基础/装备层是派生态不入档。
function M.capture()
  local data = {}
  for actor, list in pairs(buffs) do
    if type(actor) == "string" and type(list) == "table" and #list > 0 then
      data[actor] = copy_table(list)
    end
  end
  return { buffs = data }
end

-- 读档：旧档缺字段（data 非 table / 无 buffs）静默清空；条目逐个校验，
-- 非法条目丢弃（不抛错，保证旧档/坏档可恢复）。
function M.restore(data)
  buffs = {}
  if type(data) ~= "table" or type(data.buffs) ~= "table" then
    return
  end
  for actor, list in pairs(data.buffs) do
    if type(actor) == "string" and actor ~= "" and type(list) == "table" then
      local rebuilt = {}
      for _, buff in ipairs(list) do
        local amount = type(buff) == "table" and tonumber(buff.amount) or nil
        local days = type(buff) == "table" and tonumber(buff.days) or nil
        if type(buff) == "table" and type(buff.stat) == "string"
          and VALID_STATS[buff.stat] and amount and days
          and days >= 1 and math.floor(days) == days then
          rebuilt[#rebuilt + 1] = { stat = buff.stat, amount = amount, days = days }
        end
      end
      if #rebuilt > 0 then
        buffs[actor] = rebuilt
      end
    end
  end
end

return M
