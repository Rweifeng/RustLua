-- systems/mine.lua：《江湖有田》青云矿脉系统（docs/33 §4.6 / docs/54 §4.5，
-- 2026-10-06 1.2）。
--
-- 阶段定位：本阶段矿脉是**纯数据 + 系统**（headless 直接驱动）。矿洞场景与入口地图接线
-- 属改造计划阶段4美术管线——本系统不注册 interaction 目标、不切场景，只暴露
-- descend/mine/elevator 给驱动方（headless 样例与将来的矿洞入口交互）。
--
-- 运行期状态（capture/restore 随 save_manager 持久；reset 新档清空）：
--   current   当前所在层（1..30）；reset 后站在 1 层（矿脉入口）。
--   unlocked  已解锁层集合 { [floor]=true }；1 层恒解锁，逐层下潜与电梯直达会扩展它。
--
-- 动作语义（与 systems/shipping.lua 同口径：**失败不写 last_action**，成功才写）：
--   * descend(target)：逐层下到 target（必须 == current+1）。
--       体力判定：消耗 target 层 difficulty 点体力；不足返回 (false,"tired")。
--       未解锁层拒绝：target 不是下一层（跳层/上楼）返回 (false,"not_unlocked")。
--       成功：current=target、unlocked[target]=true、last_action="mine_descend_<target>"。
--   * mine()：在 current 层挖一镐。
--       体力判定：消耗 current 层 energy_cost 点体力；不足返回 (false,"tired")（不写动作）。
--       背包满返回 (false,"full")（不扣体力）。
--       矿石按 ore_pool 权重加权出 1 块；宝石按 gem_pool 独立掷骰命中即额外入包。
--       成功：inv.add 入包、last_action="mine_<ore_item>"，返回 (true, ore_item)。
--   * elevator(target)：电梯直达。target 必须是已解锁电梯层（5/10/.../30）或入口 1 层；
--       不耗体力。成功：current=target、last_action="mine_elevator_<target>"。
local state = require("core.state")
local mine_data = require("data.mine")

local inv = inv

local M = {}

-- 运行期状态
local current = 1
local unlocked = { [1] = true }

-- 当前层定义（越界 nil）。
local function current_def()
  return mine_data.get(current)
end

-- 矿石池加权抽取：总权重为分母，均匀落点按权重比例选中其一；恒出 1 块。
local function pick_ore(pool)
  local total = 0
  for _, entry in ipairs(pool or {}) do
    total = total + (tonumber(entry.chance) or 0)
  end
  if total <= 0 then
    return nil
  end
  local roll = math.random() * total
  local acc = 0
  for _, entry in ipairs(pool) do
    acc = acc + (tonumber(entry.chance) or 0)
    if roll <= acc then
      return entry.item
    end
  end
  return pool[#pool].item
end

-- 宝石池独立掷骰：每条按 chance（0<chance<=1）单独命中，命中即额外入包。
local function roll_gems(pool)
  local hits = {}
  for _, entry in ipairs(pool or {}) do
    local chance = tonumber(entry.chance) or 0
    if chance > 0 and math.random() <= chance then
      hits[#hits + 1] = entry.item
    end
  end
  return hits
end

-- ---------- 只读访问器（headless 断言/诊断用） ----------

function M.current()
  return current
end

function M.floor(n)
  return mine_data.get(n)
end

-- 已解锁层集合（只读镜像）。
function M.unlocked()
  local out = {}
  for f, ok in pairs(unlocked) do
    if ok then
      out[f] = true
    end
  end
  return out
end

-- 已解锁层升序列表（诊断/存档序列化用）。
function M.unlocked_list()
  local out = {}
  for f, ok in pairs(unlocked) do
    if ok then
      out[#out + 1] = f
    end
  end
  table.sort(out)
  return out
end

-- ---------- 动作 ----------

-- 逐层下潜到 target（须 == current+1）。体力判定 + 未解锁层拒绝。
function M.descend(target)
  target = tonumber(target)
  local def = mine_data.get(target)
  if not def then
    return false, "bad_floor"
  end
  if target ~= current + 1 then
    -- 跳层或上楼：未解锁（下一层之外一律拒绝，电梯层走 elevator()）。
    return false, "not_unlocked"
  end
  local cost = tonumber(def.difficulty) or 0
  if (tonumber(state.energy) or 0) < cost then
    return false, "tired"
  end
  state.energy = (tonumber(state.energy) or 0) - cost
  current = target
  unlocked[target] = true
  state.last_action = "mine_descend_" .. tostring(target)
  return true, "ok"
end

-- 在 current 层挖一镐：体力判定 → 矿石入包（背包满拒扣）→ 宝石加成掷骰。
function M.mine()
  local def = current_def()
  if not def then
    return false, "not_in_mine"
  end
  local cost = tonumber(def.energy_cost) or 0
  if (tonumber(state.energy) or 0) < cost then
    return false, "tired" -- 失败不写 last_action
  end
  local ore = pick_ore(def.ore_pool)
  if not ore then
    return false, "no_ore"
  end
  -- 先试装矿石：一件也装不下（背包满）则不扣体力、不掷宝石。
  local leftover = tonumber(inv.add(state.inventory, ore, 1)) or 0
  if leftover >= 1 then
    return false, "full"
  end
  state.energy = (tonumber(state.energy) or 0) - cost
  -- 宝石加成：独立掷骰命中即额外入包（装不下的宝石静默跳过，不影响矿石结算）。
  for _, gem in ipairs(roll_gems(def.gem_pool)) do
    local gleftover = tonumber(inv.add(state.inventory, gem, 1)) or 0
    if gleftover >= 1 then
      break -- 背包已满，后续宝石都装不下
    end
  end
  state.last_action = "mine_" .. ore
  return true, ore
end

-- 电梯直达：已解锁电梯层（5/10/.../30）或入口 1 层；不耗体力。
function M.elevator(target)
  target = tonumber(target)
  if not mine_data.get(target) then
    return false, "bad_floor"
  end
  local is_entrance = target == 1
  local is_open_elevator = mine_data.is_elevator(target) and unlocked[target] == true
  if not (is_entrance or is_open_elevator) then
    return false, "not_unlocked"
  end
  current = target
  state.last_action = "mine_elevator_" .. tostring(target)
  return true, "ok"
end

-- ---------- 存档 ----------

function M.capture()
  return {
    current = current,
    unlocked = M.unlocked_list()
  }
end

function M.restore(data_in)
  current = 1
  unlocked = { [1] = true }
  if type(data_in) ~= "table" then
    return
  end
  local cur = tonumber(data_in.current)
  if cur and mine_data.get(cur) then
    current = cur
  end
  if type(data_in.unlocked) == "table" then
    for _, f in ipairs(data_in.unlocked) do
      local fn = tonumber(f)
      if fn and mine_data.get(fn) then
        unlocked[fn] = true
      end
    end
  end
end

function M.reset()
  current = 1
  unlocked = { [1] = true }
end

-- ---------- 加载期校验 ----------

-- 返回 ""（通过）或换行分隔的问题串（与 systems/shipping/mastery 同约定）。
function M.validate_all()
  local report = {}
  local data_report = mine_data.validate_all()
  if type(data_report) == "string" and data_report ~= "" then
    report[#report + 1] = data_report
  end
  -- 运行期不变量：current 合法、unlocked[1] 恒在、current 必须已解锁。
  if not mine_data.get(current) then
    report[#report + 1] = "mine: current 层非法 " .. tostring(current)
  end
  if not unlocked[1] then
    report[#report + 1] = "mine: 入口层 1 必须恒解锁"
  end
  if mine_data.get(current) and not unlocked[current] then
    report[#report + 1] = "mine: current 层未解锁 " .. tostring(current)
  end
  return table.concat(report, "\n")
end

return M
