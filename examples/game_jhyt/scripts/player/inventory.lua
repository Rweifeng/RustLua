local state = require("core.state")
local items = require("data.items")
local stats = require("player.stats")
-- 当日社交饮品的锚点在 relationship 那一侧（M34-05）。这里**延迟 require**：
-- relationship → world.npc → … 的加载链会绕回背包，模块加载期取到的可能是半张表，
-- 而 use 发生在所有模块就绪之后（与本文件 heal_motes 那处同一处理）。
local relationship_cache = nil
local function relationship()
  if not relationship_cache then
    relationship_cache = require("player.relationship")
  end
  return relationship_cache
end

local M = {}

local MAX_ENERGY = 100
local MAX_INNER_POWER = 100

-- 基础上限只读口：经脉系统在此基础上聚合加成；勿改成动态值以免循环求值。
-- 消耗品与跨天恢复读 systems.meridian.max_*；战败恢复暂仍读这里的基础值。
function M.max_energy()
  return MAX_ENERGY
end

function M.max_inner_power()
  return MAX_INNER_POWER
end

-- 物品定义注册（`data/items.lua` + `data/tools.lua` → 引擎注册表）。
-- **在模块加载时就登记**，而不是等到开新档：引擎 `item.def` 是唯一的显示名查询口
-- （HUD 的物品名、快捷栏、装备槽都走它），登记必须早于任何显示行为。
-- `item.define` 幂等（同 id 覆盖），因此热重载后重复登记也无副作用。
-- 分系统物品扩展注册表（阶段 2 改造）：data/items_<system>.lua 是**可选**模块
-- （{ items = {...}, tools = {...} }），缺失即跳过、落地后自动纳入——
-- 各新系统（畜牧/钓鱼/矿脉/料理/创新系统）在并行开发时拥有自己的物品文件，
-- 避免多人同时改 data/items.lua 造成合并冲突；校验见 data/registry_check.lua
-- 的 validate_item_extensions（同一套 check_item/check_tool 规则）。
local ITEM_EXTENSION_MODULES = {
  "data.items_animal",
  "data.items_fish",
  "data.items_mine",
  "data.items_cooking",
  "data.items_stage3"
}

local function register_items()
  for id, definition in pairs(items) do
    item.define(id, definition)
  end
  for id, definition in pairs(require("data.tools")) do
    item.define(id, definition)
  end
  for _, module_name in ipairs(ITEM_EXTENSION_MODULES) do
    local ok, ext = pcall(require, module_name)
    if ok and type(ext) == "table" then
      for id, definition in pairs(ext.items or {}) do
        item.define(id, definition)
      end
      for id, definition in pairs(ext.tools or {}) do
        item.define(id, definition)
      end
    end
  end
end

register_items()

-- 初始工具（非消耗品）：发放（`setup`）与补发（`restore`）共用这一张表，
-- 免得加一件新工具只改 setup 又踩同一个坑。它们**没有第二条获取途径**——
-- 商店不售工具、`data/tools.lua` 里没有 `sell` 字段可卖、也没有任务奖励，
-- 所以缺件即死档（这正是补发成立的前提）。
local STARTER_TOOLS = {
  -- 锄头：farm.act 翻土要求 tool 槽装备它。此前新档只给槽位打标签、不发放堆叠，
  -- 而背包的行只来自槽位 ⇒ 一旦把 tool 槽换成别的工具就再也选不回来。
  "hoe",
  -- 水壶（M19-02 补齐 MVP 闭环）：farm.act 浇水要求 tool 槽装备 watering_can，
  -- 缺它作物永远无法成长，「播种→浇水→睡觉→收获→交付→制作」剧本不可完成。
  -- 由玩家在背包面板（I → 选中 → E 装备）切换，同时验证背包/装备交互。
  "watering_can",
  -- 伐木斧/矿镐（2026-09-27，docs/48 接线）：青云山道松木桩与矿石点 requires
  -- 引用（`data/gather.lua` wild_timber_1 / wild_ore_1），背包面板装备即可采集。
  "axe",
  "pick",
}

-- 开新档：背包与初始物资（物品定义已在模块加载时登记，这里不再重复）。
function M.setup()
  inv.create(state.inventory, 20)
  -- 灵芝菌种（M19-01 调整）：5→8。**每块田一茬收 1 灵芝**（`data/crops.lua`
  -- `yield = 1`，与 docs/33 §4.1 一致；M23 之前这句注释按"收 2"写、而数据里根本没有
  -- `yield` 字段，实际恒收 1）。MVP 剧本里灵芝要分四处用：
  -- 医馆委托 3（可选支线）+ 药庐订单 2 + 主线二段交药材 2 + 炼回复药 2 = 9，
  -- 山道当日采集（3）+ 次日刷新的两处采集点（`respawn_days = 1`）是主要来源，
  -- 8 粒菌种（4 块田 × 1 = 4 灵芝/茬）是多日路线的补足与教学，并留「误种一次
  -- 仍可完成」的空间（数值校核见 docs/46 §10）。
  inv.add(state.inventory, "seed_lingzhi", 8)
  -- 稻种（M19-01 调整）：3→4。稻谷要同时满足「市集订单 2 斗」与「主线二段的一斗口粮」
  -- 两处消耗，**每块田一茬收 2**（`yield = 2`），4 粒留出一次重播余量，
  -- 也避免一次误种把 MVP 剧本走死。
  inv.add(state.inventory, "seed_rice", 4)
  inv.add(state.inventory, "rice", 2)
  for _, tool in ipairs(STARTER_TOOLS) do
    inv.add(state.inventory, tool, 1)
  end
  inv.equip(state.inventory, "tool", "hoe")
end

-- 使用效果只存在于 Lua 侧数据（data.items[id].use）：引擎 item.define 会忽略
-- 未知字段，故不能从 item.def 回读。无 use 字段即不可使用（种子/收获物/工具）。
local function use_effect(item_id)
  local def = items[item_id]
  if def and type(def.use) == "table" then
    return def.use
  end
  return nil
end

-- 使用物品（M17-04 硬风险③）：先确认确实能产生效果，再应用效果，最后消耗 1 个。
-- 无效果或用满上限时拒绝并保留物品，绝不出现「只删物品不生效」的半完成状态；
-- 效果结果写入 state，随 state.capture 存档快照一致恢复。
-- M18-04 扩展：use.buff = { stat=, amount=, days= } → stats.add_buff（临时
-- Buff 按游戏日过期）；属性模型当前只挂玩家（"player"）。Buff 非法时按
-- add_buff 的 reason 拒绝且不消耗（与「先应用后消耗」语义一致）。
function M.use(item_id)
  if type(item_id) ~= "string" or item_id == "" then
    return false, "unknown_item"
  end
  if inv.count(state.inventory, item_id) <= 0 then
    return false, "not_enough"
  end
  local effect = use_effect(item_id)
  if not effect then
    return false, "no_effect"
  end
  -- 当日社交饮品（M34-05，use.social_day）：今天已经喝过就不给第二杯——
  -- 与"同一 NPC 每日一次礼物"同族语义，所以在**扣物品之前**挡（不扣物不加分）。
  if effect.social_day == true and relationship().tea_today() then
    state.last_action = "tea_today"
    return false, "tea_today"
  end

  local applied = false
  local energy = tonumber(effect.energy)
  if energy and energy ~= 0 then
    local before = tonumber(state.energy) or 0
    -- 经脉可把体力上限提高到 100 以上；恢复物品必须走同一动态上限，
    -- 否则任脉后服药会把 110 错误压回 100。
    local energy_max = require("systems.meridian").max_energy()
    local after = math.max(0, math.min(energy_max, before + energy))
    if after ~= before then
      state.energy = after
      applied = true
    end
  end
  local inner_power = tonumber(effect.inner_power)
  if inner_power and inner_power ~= 0 then
    local before = tonumber(state.inner_power) or 0
    -- 心经/三焦经等提高内力上限后，丹药/茶饮也应能恢复到动态上限。
    local inner_max = require("systems.meridian").max_inner_power()
    local after = math.max(0, math.min(inner_max, before + inner_power))
    if after ~= before then
      state.inner_power = after
      applied = true
    end
  end
  if effect.buff ~= nil then
    if type(effect.buff) ~= "table" then
      return false, "bad_buff"
    end
    local ok, reason = stats.add_buff("player", effect.buff)
    if not ok then
      return false, reason or "bad_buff"
    end
    applied = true
  end
  -- 内力已经满的人喝一杯茶仍然"有效果"（当天社交 +10%），否则这杯茶会退回
  -- "no_effect" 而原地不消耗——满内力是常态，不是异常态。
  if effect.social_day == true then
    applied = true
  end
  if not applied then
    return false, "no_effect"
  end

  inv.remove(state.inventory, item_id, 1)
  if effect.social_day == true then
    relationship().mark_tea()
  end
  state.last_action = "use_" .. item_id
  -- docs/48 effects：回复类物品（use.energy）使用成功 → 治疗提示条带。
  -- 延迟 require（avoid 模块加载期循环依赖）：use 时所有模块已就绪。
  local energy = tonumber(effect.energy) or 0
  if energy ~= 0 then
    pcall(function()
      local p = require("player.player")
      local id = p.ensure()
      if id then
        local pos = transform.get(id)
        if type(pos) == "table" then
          require("systems.effects").spawn("heal_motes", pos.x, pos.y)
        end
      end
    end)
  end
  return true, "ok"
end

-- 存档快照（save_manager 调用）：槽位 + 命名装备槽整体镜像。
-- 注意：槽位必须稠密输出（空槽 = {item="",count=0}）。引擎的 Lua↔JSON blob
-- 序列化里，稀疏数字 key 表会退化为 object、回读后 key 变字符串，导致
-- inv.set 的 index 参数类型错；1..capacity 全填充则保持 JSON array。
function M.capture()
  local data = inv.get(state.inventory)
  local capacity = data.capacity or 20
  local slots = {}
  for index = 1, capacity do
    local stack = data.slots and data.slots[index]
    if stack then
      slots[index] = { item = stack.item, count = stack.count }
    else
      slots[index] = { item = "", count = 0 }
    end
  end
  return {
    capacity = capacity,
    slots = slots,
    equipment = inv.equip_list(state.inventory)
  }
end

function M.restore(data)
  if type(data) ~= "table" then
    return
  end
  local capacity = tonumber(data.capacity) or 20
  if not inv.exists(state.inventory) then
    inv.create(state.inventory, capacity)
  end
  for index = 1, inv.capacity(state.inventory) do
    inv.set(state.inventory, index, nil, nil)
  end
  for index = 1, capacity do
    local slot = data.slots and data.slots[index]
    if slot and slot.item and slot.item ~= "" then
      inv.set(state.inventory, index, slot.item, tonumber(slot.count) or 1)
    end
  end
  for slot, item_id in pairs(data.equipment or {}) do
    inv.equip(state.inventory, slot, item_id)
  end
  -- 补发缺失的初始工具（表与理由见 `STARTER_TOOLS`）。老档的锄头只有 tool 槽标签、
  -- 没有堆叠，而背包的行只来自槽位 ⇒ 玩家既看不见它也换不回它；连槽引用都被换装
  -- 覆盖掉的那些档（tool 已是水壶），也只有在这里能救回来。
  for _, tool in ipairs(STARTER_TOOLS) do
    if (tonumber(inv.count(state.inventory, tool)) or 0) <= 0 then
      inv.add(state.inventory, tool, 1)
    end
  end
end

return M
