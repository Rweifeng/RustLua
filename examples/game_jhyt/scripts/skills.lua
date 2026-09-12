-- skills.lua：武学运行时（M18-01）。
--
-- 职责：
--   * 读取 data/skills.lua 纯数据，提供招式查询与校验入口。
--   * 伤害计算：basic 招式 = 招式基础伤害 + 武器加成（武器槽 inv.equip
--     的 "weapon" 槽，武器 damage 字段来自 data/tools.lua / data/items.lua）；
--     martial 招式 = 固定伤害。装备/卸装即时生效，无缓存。
--   * 冷却/前摇/判定窗的推进与结算在 combat.lua（战斗底座），本模块不持状态。
--
-- 引擎事实（core/script.rs）：
--   * inv.equip_get(id, slot) 返回槽内物品 id 或 nil；装备槽是任意字符串键，
--     引擎只存骨架（M16-07 注释），属性消费方就是本模块（M18-04 再下沉完整
--     属性模型，本期只保证「装备练习剑后伤害真实生效」）。
local state = require("state")
local data = require("data.skills")

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

-- 武器伤害加成：在 data.tools / data.items 的 definition 里找 damage 字段。
-- 没有装备或武器无 damage 字段 → 0。
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

-- 招式对玩家的最终伤害（攻击者是玩家时使用；敌人自带 damage，不走这里）。
function M.player_damage(id)
  local skill = data.get(id)
  if not skill then
    return 0
  end
  local base = tonumber(skill.damage) or 0
  if skill.uses_weapon then
    return base + M.weapon_damage()
  end
  return base
end

-- 招式的内力消耗（basic 恒 0）。
function M.inner_cost(id)
  local skill = data.get(id)
  return tonumber(skill and skill.inner_cost) or 0
end

function M.validate_all()
  return data.validate_all()
end

return M
