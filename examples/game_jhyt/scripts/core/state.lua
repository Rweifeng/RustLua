local M = {
  player = nil,
  inventory = "player",
  -- 主角形象：data/appearances.lua 里的 id（默认青衫少侠）。
  -- 只存 id，形象本体（序列帧/clip）由 player.lua 从 data/characters.lua 取。
  appearance = "player_male",
  money = 120,
  energy = 100,
  inner_power = 50,
  facing = "down",
  selected_seed = "seed_lingzhi",
  selected_tool = "hoe",
  -- 选中肥料（docs/33 §4.1.3，0.4）：已垦未种格按 E 施肥；nil = 未选（E 直接播种）。
  selected_fertilizer = nil,
  -- 选中洒水器（docs/33 §4.1.4，0.5）：已垦未种格按 E 放置；nil = 未选。
  selected_sprinkler = nil,
  season = "spring",
  weather = "sunny",
  labor_minutes = 0,
  last_action = "none",
  mode = "Playing",
  return_mode = "Playing",
  -- 全局 / 剧情变量：对话与任务共用的数据容器，随存档持久化。
  vars = {},
  -- 变量副作用事件流水（有界，仅用于存档回溯与调试）。
  var_events = {}
}

-- 变量事件流水上限，避免存档无限膨胀。
local VAR_EVENT_LIMIT = 32

-- 校验变量值类型：仅接受 number / boolean / string（可 JSON 往返）。
local function var_value_ok(value)
  local t = type(value)
  return t == "number" or t == "boolean" or t == "string"
end

local function record_var_event(name, old, new, source, event)
  M.var_events[#M.var_events + 1] = {
    var = name,
    old = old == nil and "<nil>" or old,
    new = new == nil and "<nil>" or new,
    source = tostring(source or "unknown"),
    event = tostring(event or "")
  }
  while #M.var_events > VAR_EVENT_LIMIT do
    table.remove(M.var_events, 1)
  end
end

local VALID_MODES = {
  Playing = true,
  Dialogue = true,
  Inventory = true,
  Shop = true,
  Pause = true,
  -- 任务日志面板（M19-02）：只读 UI 模式，与 Inventory 同款语义
  -- （非 Playing 时 input.update 早退，UI 输入不落到世界）。
  Quest = true,
  -- 出货箱面板（docs/33 §4.1.7，0.8）：与 Shop 同款只读 UI 模式。
  Shipping = true,
  Transition = true
}

function M.get_mode()
  return M.mode
end

function M.is_mode(mode)
  return M.mode == mode
end

function M.set_mode(mode)
  if not VALID_MODES[mode] then
    return false
  end
  if M.mode ~= mode then
    if mode == "Pause" then
      M.return_mode = M.mode
    elseif M.mode == "Pause" then
      M.return_mode = "Playing"
    end
    M.mode = mode
  end
  return true
end

function M.toggle_pause()
  if M.mode == "Pause" then
    return M.set_mode(M.return_mode or "Playing")
  end
  if M.mode == "Playing" then
    return M.set_mode("Pause")
  end
  return false
end

function M.toggle_inventory()
  if M.mode == "Inventory" then
    return M.set_mode("Playing")
  end
  if M.mode == "Playing" then
    return M.set_mode("Inventory")
  end
  return false
end

-- 任务日志面板（M19-02）：与背包同款开关语义，只读。
function M.toggle_quest()
  if M.mode == "Quest" then
    return M.set_mode("Playing")
  end
  if M.mode == "Playing" then
    return M.set_mode("Quest")
  end
  return false
end

-- 读取剧情变量；不存在返回 nil。
function M.get_var(name)
  if type(name) ~= "string" then
    return nil
  end
  return M.vars[name]
end

-- 写入剧情变量（值类型非法时返回 false 且不落库）。
-- source / event 记入事件流水，保证副作用可回溯（来源 + 事件）。
function M.set_var(name, value, source, event)
  if type(name) ~= "string" or not var_value_ok(value) then
    return false
  end
  record_var_event(name, M.vars[name], value, source, event)
  M.vars[name] = value
  return true
end

-- 数值型剧情变量自增；缺失或非数值时按 0 起算。
function M.add_var(name, delta, source, event)
  if type(name) ~= "string" then
    return false
  end
  local step = tonumber(delta)
  if not step then
    return false
  end
  local base = tonumber(M.vars[name]) or 0
  return M.set_var(name, base + step, source, event)
end

-- 清空剧情变量与事件流水（新档复位 / 调试用）。
function M.clear_vars()
  M.vars = {}
  M.var_events = {}
end

function M.capture()
  local vars = {}
  for name, value in pairs(M.vars) do
    if type(name) == "string" and var_value_ok(value) then
      vars[name] = value
    end
  end
  local var_events = {}
  for i = 1, #M.var_events do
    local e = M.var_events[i]
    var_events[i] = {
      var = e.var,
      old = e.old,
      new = e.new,
      source = e.source,
      event = e.event
    }
  end
  return {
    money = M.money,
    energy = M.energy,
    inner_power = M.inner_power,
    appearance = M.appearance,
    facing = M.facing,
    selected_seed = M.selected_seed,
    selected_tool = M.selected_tool,
    season = M.season,
    weather = M.weather,
    labor_minutes = M.labor_minutes,
    last_action = M.last_action,
    mode = M.mode,
    vars = vars,
    var_events = var_events
  }
end

function M.restore(data)
  if type(data) ~= "table" then
    return
  end
  M.money = tonumber(data.money) or M.money
  M.energy = tonumber(data.energy) or M.energy
  M.inner_power = tonumber(data.inner_power) or M.inner_power
  -- 形象 id 只接受字符串；非法值保留当前（player.apply 会再校验一次）
  M.appearance = data.appearance and tostring(data.appearance) or M.appearance
  M.facing = tostring(data.facing or M.facing)
  M.selected_seed = data.selected_seed and tostring(data.selected_seed) or M.selected_seed
  M.selected_tool = data.selected_tool and tostring(data.selected_tool) or M.selected_tool
  M.season = data.season and tostring(data.season) or M.season
  M.weather = data.weather and tostring(data.weather) or M.weather
  M.labor_minutes = tonumber(data.labor_minutes) or M.labor_minutes
  M.last_action = tostring(data.last_action or M.last_action)
  M.vars = {}
  if type(data.vars) == "table" then
    for name, value in pairs(data.vars) do
      if type(name) == "string" and var_value_ok(value) then
        M.vars[name] = value
      end
    end
  end
  M.var_events = {}
  if type(data.var_events) == "table" then
    for _, e in ipairs(data.var_events) do
      if type(e) == "table" then
        M.var_events[#M.var_events + 1] = {
          var = tostring(e.var or ""),
          old = e.old,
          new = e.new,
          source = tostring(e.source or ""),
          event = tostring(e.event or "")
        }
        if #M.var_events >= VAR_EVENT_LIMIT then
          break
        end
      end
    end
  end
  if VALID_MODES[data.mode] then
    M.mode = data.mode
  else
    M.mode = "Playing"
  end
  M.return_mode = "Playing"
end

-- 新档默认值（主入口门面「开始新游戏」进入山庄前调用）。
-- player 为运行时实体 id，不入档、不在新档残留；由 player.ensure() 重建。
function M.reset()
  M.player = nil
  M.inventory = "player"
  M.money = 120
  M.energy = 100
  M.inner_power = 50
  M.appearance = "player_male"
  M.facing = "down"
  M.selected_seed = "seed_lingzhi"
  M.selected_tool = "hoe"
  M.season = "spring"
  M.weather = "sunny"
  M.labor_minutes = 0
  M.last_action = "none"
  M.mode = "Playing"
  M.return_mode = "Playing"
  M.vars = {}
  M.var_events = {}
end

return M
