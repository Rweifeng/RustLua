-- panel_nav.lua：《江湖有田》面板列表导航（M20-02）。
--
-- 引擎事实（core/script.rs 的 `ui` 表 + core/ui_navigation.rs）：
--   * `ui.focus_open(id, count[, columns])`  建立/重建焦点环（0 基序号）
--   * `ui.focus_select(id, index)`           直接定位
--   * `ui.focus_index(id)`                   当前序号（无焦点返回 nil）
--   * `ui.focus_move(id, "up|down|left|right")` 方向移动（到边界环绕，返回新序号）
--   * `ui.focus_cycle(id, forward)`          Tab 式循环
--   * `ui.scroll_extents/ scroll_to_visible/ scroll_offset` 列表滚动
--
-- 本模块把上面的 0 基 API 封装成游戏面板用的 1 基调用，并在引擎未提供这些 API
-- （旧引擎）时静默降级：调用方拿不到新序号时会退回自己的钳制逻辑。
--
-- 输入侧：方向键/DPad 的按下边沿与模拟摇杆的「首次立即 + 按住重复」都在
-- input.lua 的 `ui_step` 里统一成方向字符串，再交给 `M.move`。M20-08 起菜单
-- 导航走 `ui_up/ui_down/ui_left/ui_right` 命名动作（玩家可改绑），摇杆读设备轴
-- （`input.pad_axis`，设备层已过死区）并带按住重复，避免每帧滚动。
local M = {}

local function api(name)
  if type(ui) ~= "table" then
    return nil
  end
  local fn = ui[name]
  return type(fn) == "function" and fn or nil
end

local function number(value, fallback)
  local result = tonumber(value)
  if not result then
    return fallback
  end
  return math.floor(result)
end

-- 打开/重建焦点环：`count` 项、`columns` 列（网格面板用）。返回最终项数。
function M.open(id, count, columns)
  local total = math.max(0, number(count, 0))
  local fn = api("focus_open")
  if not fn then
    return total
  end
  local ok, opened = pcall(fn, id, total, math.max(1, number(columns, 1)))
  if not ok then
    return total
  end
  return math.max(0, number(opened, total))
end

function M.close(id)
  local fn = api("focus_close")
  if fn then
    pcall(fn, id)
  end
end

function M.count(id)
  local fn = api("focus_count")
  if not fn then
    return 0
  end
  local ok, count = pcall(fn, id)
  if not ok then
    return 0
  end
  return math.max(0, number(count, 0))
end

-- 按 1 基序号设置焦点；返回是否成功。
function M.select(id, index)
  local fn = api("focus_select")
  if not fn then
    return false
  end
  local i = number(index, 0)
  if i < 1 then
    return false
  end
  local ok, done = pcall(fn, id, i - 1)
  return ok and done == true
end

-- 当前焦点的 1 基序号；无焦点/无 API 返回 nil。
function M.index(id)
  local fn = api("focus_index")
  if not fn then
    return nil
  end
  local ok, index = pcall(fn, id)
  if not ok or type(index) ~= "number" then
    return nil
  end
  return math.floor(index) + 1
end

local DIRECTIONS = { up = "up", down = "down", left = "left", right = "right" }

-- 方向移动焦点（到边界环绕）；返回新的 1 基序号或 nil。
function M.move(id, direction)
  local fn = api("focus_move")
  local dir = DIRECTIONS[tostring(direction)]
  if not fn or not dir then
    return nil
  end
  local ok, index = pcall(fn, id, dir)
  if not ok or type(index) ~= "number" then
    return nil
  end
  return math.floor(index) + 1
end

-- Tab 式循环；`forward == false` 反向。返回新的 1 基序号或 nil。
function M.cycle(id, forward)
  local fn = api("focus_cycle")
  if not fn then
    return nil
  end
  local ok, index = pcall(fn, id, forward ~= false)
  if not ok or type(index) ~= "number" then
    return nil
  end
  return math.floor(index) + 1
end

-- 把 1 基的选中行滚入视口，返回首个可见行的 1 基序号（滚动窗口起点）。
-- 无引擎滚动 API 时恒返回 1（不偏移，等价于旧行为）。
function M.window(id, count, visible, row_height, index)
  local total = math.max(0, number(count, 0))
  local rows = math.max(1, number(visible, 1))
  local step = tonumber(row_height) or 1
  local selected = math.max(1, number(index, 1))
  if step <= 0 or total == 0 then
    return 1
  end
  local extents = api("scroll_extents")
  local reveal = api("scroll_to_visible")
  local offset_fn = api("scroll_offset")
  if not (extents and reveal and offset_fn) then
    return 1
  end
  if not pcall(extents, id, rows * step, total * step) then
    return 1
  end
  if not pcall(reveal, id, (selected - 1) * step, step) then
    return 1
  end
  local ok, offset = pcall(offset_fn, id)
  if not ok or type(offset) ~= "number" then
    return 1
  end
  local first = math.floor(offset / step + 1e-4) + 1
  -- 窗口至少覆盖选中行（浮点误差兜底）。
  if first > selected then
    first = selected
  end
  if first > total then
    first = total
  end
  return math.max(1, first)
end

function M.scroll_close(id)
  local fn = api("scroll_close")
  if fn then
    pcall(fn, id)
  end
end

return M
