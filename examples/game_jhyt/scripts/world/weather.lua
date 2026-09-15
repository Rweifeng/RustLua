-- world/weather.lua：天气（晴/雨）与按日抽签（M29 / 阶段五 P1）。
--
-- 语义：
--   * `state.weather` 是"今天"的天气（随存档），取值闭集 `sunny`/`rainy`。
--   * 每天开始时按季节概率**抽一次签**（`M.sync()`），抽签结果当天不再变化。
--   * 抽签是**确定性的**：由 `weather_seed`（存档标量）+ 当天的绝对天数哈希得到，
--     同一存档同一天永远同一结果。理由与 M28 掉落一致——headless 要断言具体天气，
--     而且读档 / 重进地图 / 反复 `current()` 不能改变天气（用 `math.random` 就会漂）。
--   * `M.set()` 是显式覆盖（编辑器、调试、验收用来构造晴天/雨天），覆盖后当天不再重抽。
--
-- 为什么要"抽签"：此前 `weather` 只会被显式 `set`，实际游玩永远晴天 ⇒
-- `farm` 的 `rain_watered` 分支（雨天免浇水）与 NPC 的 `rain_place` 日程
-- 在真实玩法里**永远走不到**，等于两条已实现的机制没有入口。
--
-- 状态只有两个标量（`weather_seed` / `weather_day`，都随既有 state provider 进出存档），
-- 因此读档后不需要额外迁移：`weather_day` 不等于今天就会重新抽当天的签。
local state = require("core.state")

local M = {}

local SEED_VAR = "weather_seed"
local DAY_VAR = "weather_day"
local DEFAULT_SEED = 20260915
local KINDS = { sunny = true, rainy = true }

-- 季节降雨概率（docs/33 §7「天气」）：春多雨、夏雷阵雨、秋适中、冬少雪雨。
-- 数值只影响"多常见"，不影响正确性；改这里必须同步 docs/33 的表。
M.RAIN_CHANCE = {
  spring = 0.34,
  summer = 0.22,
  autumn = 0.30,
  winter = 0.16,
}

-- LCG（与 systems/loot.lua 同一套常数）：只用来把 (seed, day) 打散成 [0,1)。
local RNG_MOD = 2147483648
local RNG_MUL = 1103515245
local RNG_ADD = 12345

local function today()
  return math.floor(clock.now().day)
end

--- 复位（新档）：种子回默认值、清掉"今天已抽签"标记；天气回晴天。
function M.reset()
  state.set_var(SEED_VAR, DEFAULT_SEED, "weather", "reset")
  state.set_var(DAY_VAR, 0, "weather", "reset")
  state.weather = "sunny"
  return true
end

--- 固定抽签序列（headless / 调试）：只影响之后的按日抽签。
function M.set_seed(seed)
  state.set_var(SEED_VAR, math.floor(tonumber(seed) or DEFAULT_SEED), "weather", "set_seed")
  state.set_var(DAY_VAR, 0, "weather", "set_seed")
  return M.seed()
end

function M.seed()
  -- 未设置过种子（新档且未 reset）时用模块默认值；这是**声明的默认值**而非兼容分支。
  return tonumber(state.get_var(SEED_VAR)) or DEFAULT_SEED
end

-- [0, 1)：`(seed, day)` 的确定性哈希（不写回状态，纯函数）。
local function unit_for(seed, day)
  local value = (math.floor(seed) % RNG_MOD) * RNG_MUL + day * RNG_ADD
  value = (value * RNG_MUL + RNG_ADD) % RNG_MOD
  return value / RNG_MOD
end

--- 当天抽签结果（只读，不改状态）：给验收/调试看"第 N 天会不会下雨"。
--- 季节必须显式给出（`"spring"`/`"summer"`/`"autumn"`/`"winter"`）——写错季节就是数据错误，
--- 直接报错，不做"猜一个默认季节"的兜底（否则概率表配错也看不出来）。
function M.roll_for(day, season)
  local chance = M.RAIN_CHANCE[season]
  if type(chance) ~= "number" then
    error("weather.roll_for: unknown season " .. tostring(season))
  end
  return unit_for(M.seed(), math.floor(tonumber(day) or 1)) < chance and "rainy" or "sunny"
end

--- 抽今天的签（幂等：同一天重复调用不会重新抽）。
function M.sync()
  local day = today()
  if (tonumber(state.get_var(DAY_VAR)) or 0) == day then
    return state.weather
  end
  state.weather = M.roll_for(day, require("world.calendar").current())
  state.set_var(DAY_VAR, day, "weather", "roll")
  return state.weather
end

--- 今天的天气：只有"是今天的签"才算数，否则（新档、跨天）抽一次。
function M.current()
  if (tonumber(state.get_var(DAY_VAR)) or 0) == today() then
    return state.weather
  end
  return M.sync()
end

function M.is_rainy()
  return M.current() == "rainy"
end

--- 显式覆盖（编辑器/调试/验收）：覆盖后当天不再重抽，跨天才恢复抽签。
function M.set(value)
  if not KINDS[value] then
    return false
  end
  state.weather = value
  state.set_var(DAY_VAR, today(), "weather", "set")
  return true
end

--- 跨天钩子：重抽并返回新天气（`main.on_day_start` 调用）。
function M.on_day_start()
  state.set_var(DAY_VAR, 0, "weather", "day_start")
  return M.sync()
end

return M
