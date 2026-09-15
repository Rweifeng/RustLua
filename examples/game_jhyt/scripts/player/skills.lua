-- skills.lua：武学运行时（M18-01，M18-04 接入三层属性模型）。
--
-- 职责：
--   * 读取 data/skills.lua 纯数据，提供招式查询与校验入口。
--   * 伤害计算：basic 招式 = 招式基础伤害 + 武器加成；martial 招式 = 固定伤害。
--     M18-04 起武器加成走 stats 三层属性模型：weapon_bonus() =
--     stats.query("player","attack") - stats.base("player").attack，即
--     「装备层 + 临时 Buff 层」之和，**不含基础 attack**——基础层已含在
--     招式 damage 的数值约定里（strike 基础 2，练习剑装备层 8 → 2+8=10），
--     若 query 整值叠加会把基础 2 重复算成 12，故必须减去 base。
--     数值不变性：无 Buff 时与 M18-01 的 weapon_damage() 结果完全一致；
--     临时 Buff（如行气散 attack+3）经同一入口真实生效。
--   * 装备/卸装即时生效，无缓存；冷却/前摇/判定窗的推进与结算在 combat.lua
--     （战斗底座），本模块不持状态。
--
-- 引擎事实（core/script.rs）：
--   * inv.equip_get(id, slot) 返回槽内物品 id 或 nil；inv.equip(id, slot, "")
--     清空槽位（卸装）。装备槽是任意字符串键，属性消费方是 stats.lua
--     （M18-04 三层模型）。
local state = require("core.state")
local data = require("data.skills")
local stats = require("player.stats")

local M = {}

function M.get(id)
  return data.get(id)
end

function M.ids()
  return data.ids()
end

-- 已装备武器 id（weapon 槽为空/不存在时返回 nil）。
function M.weapon_id()
  if not inv.exists(state.inventory) then
    return nil
  end
  return inv.equip_get(state.inventory, "weapon")
end

-- 武器伤害加成（M18-01 遗留直读接口，保留兼容）：在 data.tools /
-- data.items 的 definition 里找 damage 字段。没有装备或武器无 damage 字段 → 0。
-- M18-04 后 player_damage 改走 weapon_bonus()（含 Buff 层），本函数仅供
-- 旧验收（jhyt_m18_combat）与调试对照。
function M.weapon_damage()
  local item_id = M.weapon_id()
  if not item_id then
    return 0
  end
  local tools = require("data.tools")
  local items = require("data.items")
  local definition = tools[item_id] or items[item_id]
  return tonumber(definition and definition.damage) or 0
end

-- 武器加成（M18-04）：query - base = 装备层 + 临时 Buff 层之和。
-- 练习剑 damage=8 经 stats 的旧字段兼容视为 stats.attack=8 → 本函数返回 8，
-- 与 M18-01 数值一致；行气散 Buff（attack+3）叠加后返回 11。
function M.weapon_bonus()
  local base = stats.base("player")
  return stats.query("player", "attack") - (tonumber(base and base.attack) or 0)
end

-- 招式对玩家的最终伤害（攻击者是玩家时使用；敌人自带 damage，不走这里）。
-- basic 招式 = 招式基础伤害 + weapon_bonus()（装备+Buff 层，见文件头说明）。
function M.player_damage(id)
  local skill = data.get(id)
  if not skill then
    return 0
  end
  local base = tonumber(skill.damage) or 0
  if skill.uses_weapon then
    return base + M.weapon_bonus()
  end
  return base
end

-- 招式的内力消耗（basic 恒 0）。
function M.inner_cost(id)
  local skill = data.get(id)
  return tonumber(skill and skill.inner_cost) or 0
end

-- 这里**故意不**再暴露 `M.validate_all` 直通 `data.skills`：真正的校验入口只有
-- 一个（`data/skills.lua`，已登记进 `scripts/validate_data.lua`）。多一个同名
-- 直通函数会让"带 validate_all 的模块都要登记"这条不变量出现例外，也会让同一份
-- 数据被校验两次、诊断重复（M23）。

return M
