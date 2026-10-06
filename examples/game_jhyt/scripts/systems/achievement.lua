-- systems/achievement.lua：《江湖有田》成就系统（docs/54 §4.12/§9，2026-10-06 1.8）。
--
-- 30 个成就（含 5 隐藏），条件走"可查询谓词"闭集（data/achievements.lua 的
-- predicate_types 白名单）。本模块负责：
--   * unlocked 表：已解锁成就 id → true（随存档持久）；
--   * check_all()：遍历 30 条对谓词求值 → 新解锁的记入并发奖励 +
--     last_event = "achieve_<id>"（**不写 state.last_action**：那是 HUD 动作反馈
--     通道，跨天结算/入睡等既有契约依赖它不被旁路事件覆写——成就走自己的
--     通知字段，成就 UI 读 M.last_event()）；
--   * 谓词求值器：读 shipping.stats / state.money / mastery.level /
--     relationship.points / combat.stats.kills / clock.now().day /
--     本模块自记的 attended 节气集——**全部 pcall 容错**：依赖模块未落地
--     时该谓词按 false 处理，避免启动崩；
--   * 隐藏成就：未解锁时 description() 返回 "???"；
--   * capture/restore/reset（随 save_manager 持久）；validate_all 返回空。
--
-- hooks：main.lua on_day_start 调用 check_all（补丁文本）；验收样例显式调用。
-- 成就 UI 面板属阶段 4 美术/UI 管线，本模块只做数据与解锁逻辑。
local state = require("core.state")
local data = require("data.achievements")
-- inv 是引擎注入的全局背包 API（与 shipping.lua / goals.lua 同一用法）。
local inv = inv

local M = {}

-- 运行时状态（capture/restore 随存档持久；reset 清空）
local unlocked = {} -- achievement_id → true
local attended = {} -- festival_id → true（亲历过的不同节气）
local last_event = nil -- 最近一次解锁通知（"achieve_<id>"），不写 state.last_action

-- ---------- 谓词求值器（全部 pcall 容错） ----------

-- 出货累计量：shipping.stats()[item]。模块缺失/未注册 → 0。
local function shipping_total(pred)
  local ok, shipping = pcall(require, "systems.shipping")
  if not ok or type(shipping) ~= "table" or type(shipping.stats) ~= "function" then
    return 0
  end
  local ok2, stats = pcall(shipping.stats)
  if not ok2 or type(stats) ~= "table" then
    return 0
  end
  return tonumber(stats[pred.item]) or 0
end

-- 图鉴进度（百分比 0..100）：已出货的不同物品种类 / 全部可入箱物品种数。
-- 可入箱物品 = data.items + data.tools + 阶段2扩展物品模块里 sell > 0 的条目。
-- 分母在求值时动态算（不缓存，避免物品增删后口径漂移）；模块缺失 → 0。
local function codex_pct()
  local ok, shipping = pcall(require, "systems.shipping")
  if not ok or type(shipping) ~= "table" or type(shipping.stats) ~= "function" then
    return 0
  end
  local ok2, shipped = pcall(shipping.stats)
  if not ok2 or type(shipped) ~= "table" then
    return 0
  end
  local numerator = 0
  for _, count in pairs(shipped) do
    if (tonumber(count) or 0) > 0 then
      numerator = numerator + 1
    end
  end
  -- 分母：扫描 data.items / data.tools / 扩展模块里 sell > 0 的条目数
  local denom = 0
  local function count_sellable(tbl)
    if type(tbl) ~= "table" then
      return
    end
    for _, def in pairs(tbl) do
      if type(def) == "table" and (tonumber(def.sell) or 0) > 0 then
        denom = denom + 1
      end
    end
  end
  local ok3, items = pcall(require, "data.items")
  count_sellable(ok3 and items or nil)
  local ok4, tools = pcall(require, "data.tools")
  count_sellable(ok4 and tools or nil)
  for _, mod_name in ipairs({ "data.items_animal", "data.items_fish", "data.items_mine", "data.items_cooking", "data.items_stage3" }) do
    local ok5, ext = pcall(require, mod_name)
    if ok5 and type(ext) == "table" and type(ext.items) == "table" then
      count_sellable(ext.items)
    end
  end
  if denom <= 0 then
    return 0
  end
  return math.floor(numerator / denom * 1000) / 10 -- 保留一位小数
end

-- 谓词分派表：condition → boolean。每个求值器内部 pcall 容错。
local PREDICATES = {}

function PREDICATES.shipping_total(pred)
  return shipping_total(pred) >= (tonumber(pred.min) or math.huge)
end

function PREDICATES.money(pred)
  return (tonumber(state.money) or 0) >= (tonumber(pred.min) or math.huge)
end

function PREDICATES.mastery_level(pred)
  local ok, mastery = pcall(require, "systems.mastery")
  if not ok or type(mastery) ~= "table" or type(mastery.level) ~= "function" then
    return false
  end
  local ok2, level = pcall(mastery.level, pred.line)
  if not ok2 then
    return false
  end
  return (tonumber(level) or 0) >= (tonumber(pred.min) or math.huge)
end

function PREDICATES.heart(pred)
  local ok, rel = pcall(require, "player.relationship")
  if not ok or type(rel) ~= "table" or type(rel.points) ~= "function" then
    return false
  end
  local ok2, points = pcall(rel.points, pred.npc)
  if not ok2 or points == nil then
    return false
  end
  return (tonumber(points) or 0) >= (tonumber(pred.min) or math.huge)
end

function PREDICATES.codex_progress(pred)
  return codex_pct() >= (tonumber(pred.min_pct) or math.huge)
end

function PREDICATES.day(_pred)
  local ok, now = pcall(function()
    return clock.now().day
  end)
  if not ok then
    return false
  end
  return (tonumber(now) or 1) >= (tonumber(_pred.min) or math.huge)
end

function PREDICATES.festivals_attended(pred)
  local n = 0
  for _ in pairs(attended) do
    n = n + 1
  end
  return n >= (tonumber(pred.min) or math.huge)
end

function PREDICATES.kills(_pred)
  local ok, combat = pcall(require, "systems.combat")
  if not ok or type(combat) ~= "table" or type(combat.stats) ~= "function" then
    return false
  end
  local ok2, stats = pcall(combat.stats)
  if not ok2 or type(stats) ~= "table" then
    return false
  end
  return (tonumber(stats.kills) or 0) >= (tonumber(_pred.min) or math.huge)
end

-- ---------- 奖励发放 ----------

local function grant_reward(reward, reason)
  if type(reward) ~= "table" then
    return false
  end
  if type(reward.item) == "string" then
    local count = math.max(1, math.floor(tonumber(reward.count) or 1))
    local ok = pcall(inv.add, state.inventory, reward.item, count)
    if not ok then
      return false
    end
    return true
  end
  if tonumber(reward.money) then
    state.money = (tonumber(state.money) or 0) + math.floor(tonumber(reward.money))
    return true
  end
  return false
end

-- 记录今天是否落在某个节气窗口里（check_all 每次调用时补记）。
-- festival 是 day 的纯映射（world/festival.lua），不查随机数、不入存档；
-- 本模块自己把"亲历过的节气 id"记进 attended（随存档持久）。
local function record_today_festival()
  local ok, festival = pcall(require, "world.festival")
  if not ok or type(festival) ~= "table" or type(festival.current) ~= "function" then
    return
  end
  local ok2, entry = pcall(festival.current)
  if ok2 and type(entry) == "table" and type(entry.id) == "string" then
    attended[entry.id] = true
  end
end

-- ---------- 公开接口 ----------

-- 遍历全部成就，对未解锁者求谓词；新解锁的发奖 + 写 last_action。
-- 返回本次新解锁的 id 列表（可能为空）。
function M.check_all()
  record_today_festival()
  local newly = {}
  for _, entry in ipairs(data.achievements) do
    if not unlocked[entry.id] then
      local fn = PREDICATES[entry.condition and entry.condition.type]
      local met = false
      if fn then
        local ok, result = pcall(fn, entry.condition)
        met = ok and result == true
      end
      if met then
        unlocked[entry.id] = true
        grant_reward(entry.reward, "成就 " .. entry.name)
        last_event = "achieve_" .. entry.id
        newly[#newly + 1] = entry.id
      end
    end
  end
  return newly
end

-- 已解锁成就 id → true（只读）。
function M.unlocked()
  return unlocked
end

function M.is_unlocked(id)
  return unlocked[id] == true
end

-- 最近一次解锁通知（"achieve_<id>"，不写 state.last_action、不随存档；
-- 成就 UI/通知面板读取用）。
function M.last_event()
  return last_event
end

-- 已解锁数量。
function M.unlocked_count()
  local n = 0
  for _ in pairs(unlocked) do
    n = n + 1
  end
  return n
end

-- 某成就的玩家可见描述：隐藏成就未解锁时返回 "???"。
function M.description(id)
  local entry = data.get(id)
  if not entry then
    return ""
  end
  if entry.hidden and not unlocked[id] then
    return "???"
  end
  return entry.description or ""
end

-- 某成就的玩家可见称号：隐藏成就未解锁时返回 "???"。
function M.name(id)
  local entry = data.get(id)
  if not entry then
    return ""
  end
  if entry.hidden and not unlocked[id] then
    return "???"
  end
  return entry.name or ""
end

-- 亲历过的节气 id 数量（诊断/验收用）。
function M.festivals_attended_count()
  local n = 0
  for _ in pairs(attended) do
    n = n + 1
  end
  return n
end

-- ---------- 存档 ----------

function M.capture()
  local unlocked_data = {}
  for id, flag in pairs(unlocked) do
    if type(id) == "string" and flag == true then
      unlocked_data[id] = true
    end
  end
  local attended_data = {}
  for id, flag in pairs(attended) do
    if type(id) == "string" and flag == true then
      attended_data[id] = true
    end
  end
  return { unlocked = unlocked_data, attended = attended_data }
end

function M.restore(data_in)
  unlocked, attended = {}, {}
  if type(data_in) ~= "table" then
    return
  end
  if type(data_in.unlocked) == "table" then
    for id, flag in pairs(data_in.unlocked) do
      if type(id) == "string" and flag == true and data.get(id) then
        unlocked[id] = true
      end
    end
  end
  if type(data_in.attended) == "table" then
    for id, flag in pairs(data_in.attended) do
      if type(id) == "string" and flag == true then
        attended[id] = true
      end
    end
  end
end

function M.reset()
  unlocked, attended = {}, {}
end

-- ---------- 加载期校验 ----------

-- 纯系统模块：数据校验委托 data.achievements.validate_all；本模块只自检
-- "谓词白名单与数据谓词类型一一对应"。返回 "" = 通过。
function M.validate_all()
  local report = {}
  -- 数据层校验
  local data_report = data.validate_all()
  if data_report ~= "" then
    report[#report + 1] = "achievement: data 校验失败:\n" .. data_report
  end
  -- 谓词求值表必须覆盖白名单全部类型
  for type_name in pairs(data.predicate_types) do
    if type(PREDICATES[type_name]) ~= "function" then
      report[#report + 1] = "achievement: 谓词 " .. type_name .. " 缺求值器"
    end
  end
  return table.concat(report, "\n")
end

return M
