-- systems/mastery.lua：《江湖有田》熟练度系统（docs/33 §7.1，2026-10-06 0.9）。
--
-- 5 线 0–10 级 + 专精（docs/54 §4.9/§5.4）：
--   * xp：line → 累计 XP（内存 + save_manager 持久，随存档恢复）；
--   * level(line)：按 data/mastery.lua 的累计阈值表查 0–10 级；
--   * 来源接线：务农（翻土/播种/收获 +5，farm.lua）、采药（采集 +5，gather.lua）、
--     制药（制作完成 +8，crafting.lua）、江湖（卖出 +10，shop.lua）；
--     武学线 XP 来源（命中 +2/训练任务）留 Alpha（数据已注册）；
--   * 专精：每线 5 级与 10 级各 2 选 1（choose/has），选择进存档；
--     MVP 只落地务农四专精效果（farming_bonus 汇总：精耕/药性/深耕/富饶），
--     其余线专精为描述占位（Alpha 接线）。
local state = require("core.state")
local mastery_data = require("data.mastery")

local M = {}

local xp = {} -- line → 累计 XP
local chosen = {} -- line → { [slot] = specialization_id }

local function lines()
  return mastery_data.lines or {}
end

local function thresholds()
  return mastery_data.thresholds or {}
end

-- 由累计 XP 查等级（0–10）：遍历阈值表，超过第 n 个阈值即达到 n+1 级。
function M.level(line_id)
  local value = math.floor(tonumber(xp[line_id]) or 0)
  local level = 0
  for index, need in ipairs(thresholds()) do
    if value >= need then
      level = index
    else
      break
    end
  end
  return level
end

function M.xp_of(line_id)
  return math.floor(tonumber(xp[line_id]) or 0)
end

-- 累计 XP 增加（来源接线调用）。返回 (new_level, leveled_up)。
-- 不写 last_action（熟练度是后台成长，不干扰耕作/采集等动作断言）。
function M.add_xp(line_id, amount)
  local n = math.floor(tonumber(amount) or 0)
  if n <= 0 or not lines()[line_id] then
    return M.level(line_id), false
  end
  local before = M.level(line_id)
  xp[line_id] = (xp[line_id] or 0) + n
  local after = M.level(line_id)
  return after, after > before
end

local function specialization_def(line_id, spec_id)
  local line = lines()[line_id]
  if not line then
    return nil
  end
  for _, spec in ipairs(line.specializations or {}) do
    if spec.id == spec_id then
      return spec
    end
  end
  return nil
end

-- 选择专精：等级满足（level >= slot）且该槽位未选 → 存入。
-- 返回 (true,"ok") 或 (false,reason)；reason ∈ unknown_line/unknown_spec/not_unlocked/already。
function M.choose(line_id, spec_id)
  local spec = specialization_def(line_id, spec_id)
  if not spec then
    return false, "unknown_spec"
  end
  local level = M.level(line_id)
  if level < (spec.slot or 99) then
    return false, "not_unlocked"
  end
  if chosen[line_id] and chosen[line_id][spec.slot] then
    return false, "already"
  end
  chosen[line_id] = chosen[line_id] or {}
  chosen[line_id][spec.slot] = spec.id
  state.last_action = "mastery_" .. spec.id
  return true, "ok"
end

function M.has(line_id, spec_id)
  local spec = specialization_def(line_id, spec_id)
  if not spec then
    return false
  end
  return chosen[line_id] and chosen[line_id][spec.slot] == spec.id or false
end

-- 已选专精：line → { [slot] = spec_id }（只读访问器）。
function M.chosen()
  return chosen
end

-- 务农效果汇总（farm.lua 品质/产量/再生钩子的唯一来源）：
--   * 品质概率随务农等级成长：良好 25+2L（封顶 45）、上品 5+0.5L（封顶 10）；
--   * 专精精耕：产量 +10%；富饶：再 +5%；
--   * 专精深耕：再生作物再熟周期 -1 天（下限 2）；
--   * 专精药性：品质概率额外 +10% 良好 / +5% 上品。
function M.farming_bonus()
  local L = M.level("farming")
  local good = math.min(45, 25 + 2 * L)
  local super = math.min(10, math.floor(5 + 0.5 * L))
  local yield_pct = 0
  if M.has("farming", "jinggeng") then
    yield_pct = yield_pct + 10
  end
  if M.has("farming", "furao") then
    yield_pct = yield_pct + 5
  end
  local regrow_cut = M.has("farming", "gengshen") and 1 or 0
  local herb_quality = M.has("farming", "yaoxing")
  if herb_quality then
    good = math.min(55, good + 10)
    super = math.min(15, super + 5)
  end
  return {
    level = L,
    good = good,
    super = super,
    yield_pct = yield_pct,
    regrow_cut = regrow_cut,
    herb_quality = herb_quality,
  }
end

-- ---------- 存档 ----------

function M.capture()
  local xp_data, chosen_data = {}, {}
  for line_id, value in pairs(xp) do
    if lines()[line_id] and value > 0 then
      xp_data[line_id] = math.floor(value)
    end
  end
  for line_id, slots in pairs(chosen) do
    if lines()[line_id] and type(slots) == "table" then
      local safe = {}
      for slot, spec_id in pairs(slots) do
        if specialization_def(line_id, spec_id) then
          safe[slot] = spec_id
        end
      end
      chosen_data[line_id] = safe
    end
  end
  return { xp = xp_data, chosen = chosen_data }
end

function M.restore(data_in)
  xp, chosen = {}, {}
  if type(data_in) ~= "table" then
    return
  end
  if type(data_in.xp) == "table" then
    for line_id, value in pairs(data_in.xp) do
      if lines()[line_id] and tonumber(value) and value > 0 then
        xp[line_id] = math.floor(value)
      end
    end
  end
  if type(data_in.chosen) == "table" then
    for line_id, slots in pairs(data_in.chosen) do
      if lines()[line_id] and type(slots) == "table" then
        local safe = {}
        for slot, spec_id in pairs(slots) do
          if specialization_def(line_id, spec_id) then
            safe[slot] = spec_id
          end
        end
        chosen[line_id] = safe
      end
    end
  end
end

function M.reset()
  xp, chosen = {}, {}
end

-- ---------- 加载期校验 ----------

function M.validate_all()
  local report = {}
  local seen = {}
  for line_id, line in pairs(lines()) do
    if type(line_id) ~= "string" or type(line) ~= "table" then
      report[#report + 1] = "mastery: 线条目非法"
    end
    local slot_counts = {}
    for _, spec in ipairs(line.specializations or {}) do
      if type(spec.id) ~= "string" then
        report[#report + 1] = "mastery: " .. tostring(line_id) .. " 专精缺 id"
      end
      if seen[spec.id] then
        report[#report + 1] = "mastery: 专精 id 重复 " .. tostring(spec.id)
      end
      seen[spec.id] = true
      local slot = tonumber(spec.slot)
      if slot ~= 5 and slot ~= 10 then
        report[#report + 1] = "mastery: " .. tostring(spec.id) .. " 槽位须为 5 或 10"
      end
      slot_counts[slot] = (slot_counts[slot] or 0) + 1
    end
    for slot, count in pairs(slot_counts) do
      if count ~= 2 then
        report[#report + 1] = "mastery: " .. tostring(line_id) .. " 槽位 " .. tostring(slot) .. " 须 2 选 1，现有 " .. tostring(count)
      end
    end
  end
  local th = thresholds()
  if #th ~= 10 then
    report[#report + 1] = "mastery: 阈值表须 10 档，现有 " .. tostring(#th)
  end
  return table.concat(report, "\n")
end

return M
