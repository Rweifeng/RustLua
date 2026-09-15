-- systems/death.lua：战败结算（M28 / 计划 G3，落地 docs/33 §6.3）。
--
-- 设计案口径（§6.3）与实现对应：
--   生命归零后回到山庄      → `map.goto('manor', 出生点)`
--   保留作物/建筑/任务/已入库物品/关系 → 这些都不在惩罚路径里被触碰
--   损失当天未入库掉落的 50% → `systems/loot.lua` 的待入库袋减半（向下取整）
--   额外损失当前持有铜钱的 5% → `math.floor(money * 0.05)`（最低 0）
--   不损失关键剧情物品       → 惩罚只动待入库袋与铜钱，背包里的任务物品不动
--   回庄后生命/体力/内力回满 → `combat.respawn()` + 体力内力设为上限
--
-- 为什么单独一个模块：`systems/combat.lua` 只管战斗判定（命中/硬直/死亡标志），
-- 经济与传送属于游戏侧规则；`ui/input.lua` 的战败分支只调用本模块一个入口。
local state = require("core.state")
local combat = require("systems.combat")
local loot = require("systems.loot")
local maps = require("data.maps")
local inventory = require("player.inventory")

local M = {}

-- 死亡时扣除的铜钱比例（docs/33 §6.3：当前持有铜钱的 5%）。
M.MONEY_LOSS_RATE = 0.05
-- 回庄落点（山庄注册表的 spawn：与 manor.scene 的玩家 Transform 一致）。
M.RETURN_MAP = "manor"

local last_penalty = nil

--- 只读预算：这次战败会损失什么（战败界面/headless 断言用，不改任何状态）。
function M.penalty()
  local money = tonumber(state.money) or 0
  local items = {}
  for _, entry in ipairs(loot.pending_list()) do
    local lose = math.floor(entry.count * 0.5)
    if lose > 0 then
      items[#items + 1] = { item = entry.item, count = lose }
    end
  end
  return {
    money = math.floor(money * M.MONEY_LOSS_RATE),
    items = items,
    pending = loot.pending_list()
  }
end

--- 上次战败的结算明细（HUD/日志/headless 用；新档或未战败时为 nil）。
function M.last_penalty()
  if not last_penalty then
    return nil
  end
  return {
    money = last_penalty.money,
    items = last_penalty.items,
    text = last_penalty.text
  }
end

-- 明细文案：没钱没物时给一句"身无长物"，避免 HUD 出现"损失："后面空着。
local function penalty_text(money_lost, lost_items)
  local parts = {}
  if money_lost > 0 then
    parts[#parts + 1] = "铜钱 " .. tostring(money_lost) .. " 文"
  end
  for _, entry in ipairs(lost_items) do
    parts[#parts + 1] = tostring(entry.item) .. " x" .. tostring(entry.count)
  end
  if #parts == 0 then
    return "身无长物"
  end
  return table.concat(parts, "、")
end

--- 战败结算 + 回庄。返回 `true, 结算明细`；未处于战败态返回 `false, "not_dead"`。
function M.apply()
  if not combat.is_player_dead() then
    return false, "not_dead"
  end
  -- ① 惩罚：先算明细再落账，保证 HUD/日志里报的数字与实际扣除一致。
  local lost_items = loot.lose_on_death()
  local money_lost = math.floor((tonumber(state.money) or 0) * M.MONEY_LOSS_RATE)
  if money_lost > 0 then
    state.money = (tonumber(state.money) or 0) - money_lost
  end
  local text = penalty_text(money_lost, lost_items)
  -- ② 复活：气血回满 + 敌人重置（回到山庄前先归位，避免在别的图上留下半死的敌人）。
  local ok, reason = combat.respawn()
  if not ok then
    return false, reason
  end
  -- ③ 状态恢复（§6.3：生命/体力/内力回满）。
  state.energy = inventory.max_energy()
  state.inner_power = inventory.max_inner_power()
  -- ④ 回庄：走与出口一样的地图跳转入口（校验场景存在、切图后重放本图 overlay）。
  local definition = maps.get(M.RETURN_MAP)
  local wx, wy = nil, nil
  if definition and definition.spawn then
    wx, wy = maps.to_world({ to_cell = definition.spawn })
  end
  local moved
  if wx then
    moved = map["goto"](M.RETURN_MAP, wx, wy)
  else
    moved = map["goto"](M.RETURN_MAP)
  end
  if not moved then
    state.last_action = "map_goto_failed"
    return false, "goto_failed"
  end
  -- ⑤ 反馈：HUD 一行放明细（前缀 death_penalty_ 按原样拼接）。
  last_penalty = { money = money_lost, items = lost_items, text = text }
  state.last_action = "death_penalty_" .. text
  log.warn(string.format("[death] 战败回庄：损失 %s（返回 %s）", text, M.RETURN_MAP))
  return true, last_penalty
end

--- 新档复位：清掉上一局的战败明细。
function M.reset()
  last_penalty = nil
  return true
end

return M
