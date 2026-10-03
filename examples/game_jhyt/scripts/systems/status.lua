-- status.lua：按**固定步**结算的瞬时状态账本（M34-03）。
--
-- 为什么要单独一个模块：`data/enemies.lua` 里毒蝎的注释写着"带中毒"，但全工程没有任何
-- 地方结算它；HUD 的八张状态图标里 `poison` 那张一直"已就绪但没人驱动"
-- （`ui/hud.lua` 的 `active_status_keys` 自认过这件事）。本模块把"命中之后还在走的东西"
-- 做成一个有名字、有数据口径、可 headless 断言的账本，让那句注释变成真话。
--
-- 计时单位只有固定步（与硬直 `hitstun`、击退、玩家无敌帧 48 步同一口径）：
--   * 本文件**不读** `time.delta()`、不读 `clock.now()`、不用 `math.random`；
--     推进只发生在 `M.tick(actor)` 被调用时，而调用方是 `systems/combat.lua` 的固定步循环
--     （`main.lua` 每步一次）。于是 30/60/120 FPS 下同输入序列的步数完全一致（docs/34 C-1）。
--   * 秒 ↔ 步的换算只在 `data/combat.lua` 的 `status` 表里发生一次，数值口径是
--     docs/33 §10 状态效果表（中毒：每 2 秒损失 3 生命 / 10 秒 → 120 步一跳 / 600 步）。
--
-- 与 `player/stats.lua` 的分工（那边的文件头有镜像说明）：
--   * **属性类增益**（+攻击/+防御，按**游戏日**过期）→ `stats.add_buff`；
--   * **按步结算的状态**（中毒掉血；将来的减速/沉默同形状）→ 本模块。
--   两层的过期锚点不同（日 vs 步），混用必然出错：把 DOT 塞进 buff 就等于按日结算掉血。
--
-- 不入存档（沿用 docs/33 既有裁定："硬直/无敌帧是瞬时状态：不进存档，读档后玩家不会继承
-- 上一局的僵直"）：本模块没有 capture/restore，清空发生在 `combat.reset()` / `combat.restore()`
-- 里（与无敌帧、硬直、击退同一批被清的东西）。中毒跨睡眠存活是另一个产品决定，要改 docs/33 再说。
--
-- 本模块**不碰气血**：结算只产出事件（`M.tick` 的返回值），扣血与战败流程归 `systems/combat.lua`。
-- 这样"毒可以毒死人，但走的是同一套死亡/回庄链路"是结构性的，不是一个约定。
local combat_data = require("data.combat")
local state = require("core.state")

local M = {}

-- list：数组，顺序 = 施加顺序（HUD 图标的排布因此确定，不受 pairs() 顺序影响）。
-- index[actor][kind] → 同一条目的引用：(actor, kind) 至多一条，就是 docs/33 §10 那句
-- "同类持续状态默认不可叠加，只刷新持续时间"的实现位置。
local list = {}
local index = {}

local function actor_bucket(actor)
  local key = tostring(actor)
  local bucket = index[key]
  if not bucket then
    bucket = {}
    index[key] = bucket
  end
  return bucket
end

local function entry_of(actor, kind)
  return index[tostring(actor)] and index[tostring(actor)][kind] or nil
end

-- HUD 的动作反馈行是**玩家**的通道（`state.last_action`），所以只有玩家侧才写它。
-- 已知边界：这一行是瞬时的——`ui/input.lua` 每步都会按当前动作改写，因此中毒当刻若还按着
-- 方向键，文字会被"移动"盖掉。持续状态的主反馈是图标（本模块的 `M.list` 驱动），不是这一行。
local PLAYER_ACTOR = "player"

local function write_feedback(def, actor)
  if actor ~= PLAYER_ACTOR then
    return
  end
  if type(def.feedback) ~= "string" or def.feedback == "" then
    return
  end
  state.last_action = def.feedback
end

--- 给某个 actor 挂一个状态种类。已存在同类 → **只刷新持续时间**（不叠加、不重置跳表）。
--- 返回 `true, "applied"|"refreshed"` 或 `false, "unknown_kind"|"bad_actor"`。
--- `source` 是诊断/日志用的来源名（如"毒蝎"），不参与规则判定。
function M.apply(kind, actor, source)
  local def = combat_data.status_entry(kind)
  if not def then
    return false, "unknown_kind"
  end
  if type(actor) ~= "string" or actor == "" then
    return false, "bad_actor"
  end
  local existing = entry_of(actor, kind)
  if existing then
    -- 只刷新持续时间：下一次结算的倒计时继续走（"再被咬一口就立刻掉一次血"是叠伤害，
    -- docs/33 §10 明说默认不可叠加）。
    existing.steps_left = def.steps
    write_feedback(def, actor)
    return true, "refreshed"
  end
  local entry = {
    kind = kind,
    actor = actor,
    steps_left = def.steps,
    next_in = def.tick_steps,
    source = tostring(source or kind)
  }
  list[#list + 1] = entry
  actor_bucket(actor)[kind] = entry
  write_feedback(def, actor)
  return true, "applied"
end

--- 该 actor 身上是否挂着某个种类（HUD 与验收用）。
function M.has(kind, actor)
  return entry_of(actor, kind) ~= nil
end

--- 剩余步数与下一次结算的倒计时；未挂该种类返回 nil。
function M.remaining(kind, actor)
  local entry = entry_of(actor, kind)
  if not entry then
    return nil
  end
  return { steps_left = entry.steps_left, next_in = entry.next_in, source = entry.source }
end

--- 只读快照（HUD 图标、headless 断言用）；缺省 actor = 全部，按施加顺序返回。
function M.list(actor)
  local out = {}
  for _, entry in ipairs(list) do
    if actor == nil or entry.actor == actor then
      out[#out + 1] = {
        kind = entry.kind,
        actor = entry.actor,
        steps_left = entry.steps_left,
        next_in = entry.next_in,
        source = entry.source
      }
    end
  end
  return out
end

--- 推进一个固定步，返回本步要结算的事件列表（当前只有 `effect = "damage"`）。
--- 到期即结算并把倒计时重置为 `tick_steps`；持续时间走完就摘除条目。
--- **谁调用**：`systems/combat.lua` 的 `tick()`（玩家侧）；将来别的 actor 由各自的持有者调用。
function M.tick(actor)
  local events = {}
  local keep = {}
  for _, entry in ipairs(list) do
    if entry.actor ~= actor then
      keep[#keep + 1] = entry
    else
      local def = combat_data.status_entry(entry.kind)
      entry.steps_left = entry.steps_left - 1
      entry.next_in = entry.next_in - 1
      if entry.next_in <= 0 and def and (tonumber(def.damage) or 0) > 0 then
        events[#events + 1] = {
          kind = entry.kind,
          actor = entry.actor,
          source = entry.source,
          effect = "damage",
          amount = def.damage
        }
        entry.next_in = def.tick_steps
      end
      if entry.steps_left > 0 then
        keep[#keep + 1] = entry
      else
        local bucket = index[tostring(entry.actor)]
        if bucket then
          bucket[entry.kind] = nil
        end
      end
    end
  end
  list = keep
  return events
end

--- 摘掉某个 actor 的一个种类（将来的解毒剂入口；今天由战败与读档路径调用的是 clear）。
function M.remove(kind, actor)
  local entry = entry_of(actor, kind)
  if not entry then
    return false
  end
  local bucket = index[tostring(actor)]
  bucket[kind] = nil
  local keep = {}
  for _, candidate in ipairs(list) do
    if candidate ~= entry then
      keep[#keep + 1] = candidate
    end
  end
  list = keep
  return true
end

--- 清掉某个 actor 的全部状态：战败（不能让死人继续掉血到复活之后）与读档（瞬时状态不入档）。
function M.clear(actor)
  local bucket = index[tostring(actor)]
  if bucket then
    for kind in pairs(bucket) do
      bucket[kind] = nil
    end
  end
  local keep = {}
  for _, entry in ipairs(list) do
    if entry.actor ~= tostring(actor) then
      keep[#keep + 1] = entry
    end
  end
  list = keep
end

--- 新档复位：整张账本清空。
function M.reset()
  list = {}
  index = {}
end

return M
