-- quest_ui.lua：《江湖有田》任务日志面板（M19-02；导航/滚动 M20-02）。
--
-- 只读面板：列出全部已注册任务的五态（not_started / in_progress /
-- deliverable / completed / failed）与当前选中任务的目标进度。
-- **不在此交付任务**：交付仍走对话/交互（M17-03 的单一入口），避免出现
-- 第二条交付路径导致奖励重复或状态漂移。
--
-- 键位（与 shop.lua 同款纯键盘范式）：
--   W/S 或上下方向键 上/下选择（走引擎焦点环，到边界环绕；方向键为 M20-02）
--   A/D 翻页（每页 7 条），1-9 直接定位
--   L / Q / Esc 关闭
-- 任务超过一页时列表按选中行滚动（引擎 `ui.scroll_*`，M20-02）。
--
-- 数据来源：quest.ids() / quest.state(id) / quest.progress(id)（只读接口），
-- 本模块不写任何任务状态。
local state = require("core.state")
local quest = require("world.quest")
local npcs = require("data.npcs")
-- M19-02：面板开闭音效（只读面板无确认/取消操作）；M20-02 走 UI 总线。
local audio = require("core.audio")
-- M20-02：列表焦点/滚动导航（引擎 ui_navigation）。
local nav = require("ui.panel_nav")
-- M20-09：键鼠/手柄两套操作说明（本模块不 require ui.input，避免 require 环）。
local hints = require("ui.hints")
-- M30：任务行/目标行按真实字宽裁剪（长任务名或长目标不再画到面板外）。
local ui_text = require("ui.ui_text")
-- M32 竖版：面板矩形与行偏移的唯一来源（`ui/layout.lua`）。
local layout = require("ui.layout")
-- M68/B5：面板静态骨架搬进 `assets/prefabs/quest.prefab`，不再 require ui.skin。

-- 数字直选 1..9 的按键名（默认只有键盘数字键；未绑定该设备的动作自动跳过）。
local function digits_label()
  return hints.binding_labels(
    { "slot_1", "slot_2", "slot_3", "slot_4", "slot_5", "slot_6", "slot_7", "slot_8", "slot_9" },
    nil,
    "/"
  )
end

-- 底部提示：按当前设备二选一，按键名从**当前绑定**推导（改键后自动同步）。
local function hint_line()
  local close_label = hints.binding_labels({ "quest", "cancel", "practice" })
  return hints.panel_line({
    { keyboard = "W/S", alt_keyboard = "上下方向键", gamepad = "左摇杆/DPad", label = "选择" },
    { keyboard = "1-9", gamepad = digits_label(), label = "定位" },
    { keyboard = close_label, gamepad = close_label, label = "关闭" },
  })
end

local M = {}

-- 竖版版面（M32）：面板与行偏移取自 `layout.lua`；7 行列表 + 发布者 + 目标列表。
local PANEL = layout.PANEL.quest
local ROW_LIMIT = 7
local ROW_H = 38

local FOCUS = "qst:focus"
local SCROLL = "qst:scroll"

local STATE_TEXT = {
  not_started = "未接取",
  in_progress = "进行中",
  deliverable = "可交付",
  completed = "已完成",
  failed = "已失败"
}

local ui_state = nil

-- 前向声明（定义在 select/move 之后；Lua 词法作用域只认已声明的 local）。
local sync_focus

function M.state_text(value)
  return STATE_TEXT[value] or tostring(value)
end

function M.row_limit()
  return ROW_LIMIT
end

function M.ui_active()
  return ui_state ~= nil
end

function M.index()
  return ui_state and ui_state.index or 0
end

-- 任务行：{ id, name, state, giver_name }。id 排序沿用 quest.ids()（注册表顺序）。
function M.rows()
  local out = {}
  for _, id in ipairs(quest.ids()) do
    local def = quest.get(id) or {}
    local giver = npcs.get(def.giver)
    out[#out + 1] = {
      id = id,
      name = tostring(def.name or id),
      state = quest.state(id),
      giver_name = giver and giver.name or nil
    }
  end
  return out
end

-- 选中任务的目标进度（只读）。
function M.objectives()
  if not ui_state then
    return {}
  end
  local row = M.rows()[ui_state.index + 1]
  if not row then
    return {}
  end
  return quest.progress(row.id)
end

local function clear_ui()
  ui.clear("qst:box")
  ui.clear("qst:skin_bg")
  ui.clear("qst:rule")
  ui.clear("qst:sel")
  -- 换肤贴图走 `ui.image`（跨帧持久）：不清就会在面板关闭后留残影。
  for _, id in ipairs({ "qst:divider", "qst:seal", "qst:key_hint" }) do
    ui.clear(id)
  end
  for i = 1, ROW_LIMIT do
    ui.clear("qst:bulletqst:row" .. i)
  end
  for _, id in ipairs({ "qst:title", "qst:state", "qst:giver", "qst:objtitle", "qst:hint", "qst:empty", "qst:msg" }) do
    ui_text.hide("qst", id)
  end
  for i = 1, ROW_LIMIT do
    ui_text.hide("qst", "qst:row" .. i)
    ui_text.hide("qst", "qst:obj" .. i)
  end
  ui_text.reset("qst")
end

-- ---------- 预设（M68/B5） ----------
-- 静态骨架搬进 `assets/prefabs/quest.prefab`（对账门 = jhyt_m68_quest_prefab，黄金表 70 行
-- 钉死迁移前画面），render 只写会变的部分：选中行矩形、任务行文本、状态/发布者/目标进度、
-- 提示、空列表与滚动窗口外行的显隐、bullet todo/done 二选一。title 与 msg 固定写死在预设。
-- 开/关屏 = 根节点显隐（`prefab.set_visible` 子树语义，藏父带子、藏节点不建布局项），
-- 不撤重铺，避免槽位 LIFO 复用打乱绘制序（M63c 教训）。
local PRESET_PATH = "assets/prefabs/quest.prefab"
local preset_handle = nil
local preset_spawned = false

-- M71 修复：场景切换重建 ECS 世界后旧句柄失效，纯布尔标志漏判 → set_text 报「没有 handle N」；
-- 改成对句柄真实探活（`prefab.root` 失效句柄抛错，pcall 兜底）。
local function preset_live()
  if not preset_handle then
    return false
  end
  local ok, alive = pcall(function() return prefab.root(preset_handle) ~= nil end)
  return ok and alive == true
end

local function preset_spawn()
  if preset_handle and preset_live() then
    return true
  end
  preset_handle = prefab.spawn(PRESET_PATH)
  preset_spawned = true
  return true
end

local function preset_show()
  preset_spawn()
  prefab.set_visible(preset_handle, "QuestRoot", true)
  return true
end

local function preset_hide()
  if preset_live() then
    prefab.set_visible(preset_handle, "QuestRoot", false)
  end
  return true
end

-- 验收（jhyt_m19_ui 面板断言）取 spawn 句柄；open 前为 nil。
function M.preset_ref()
  return preset_handle
end

function M.select(index)
  if not ui_state then
    return false
  end
  local rows = M.rows()
  local i = math.floor(tonumber(index) or 0)
  if i < 0 or i >= #rows then
    return false
  end
  ui_state.index = i
  sync_focus()
  return true
end

-- 焦点环与当前序号同步（M20-02）。
function sync_focus()
  if not ui_state then
    return
  end
  nav.open(FOCUS, #M.rows(), 1)
  nav.select(FOCUS, ui_state.index + 1)
end

-- 焦点环方向移动（`step` 行，正数向下）；引擎 API 不可用返回 nil。
local function ring_step(step)
  if not ui_state then
    return nil
  end
  local direction = step > 0 and "down" or "up"
  local index = ui_state.index + 1
  local moved = false
  for _ = 1, math.abs(step) do
    local next_index = nav.move(FOCUS, direction)
    if not next_index then
      break
    end
    index = next_index
    moved = true
  end
  if not moved then
    return nil
  end
  return index - 1
end

-- 相对移动：焦点环可用时按方向逐行移动（到边界环绕），否则退回钳制语义。
function M.move(delta)
  if not ui_state then
    return false
  end
  local rows = M.rows()
  if #rows == 0 then
    return false
  end
  local step = math.floor(tonumber(delta) or 0)
  if step == 0 then
    return false
  end
  sync_focus()
  local target = ring_step(step)
  if target == nil then
    target = math.max(0, math.min(#rows - 1, ui_state.index + step))
  end
  if target == ui_state.index then
    return false
  end
  ui_state.index = target
  return true
end

-- 方向键移动（M20-02）：直接交给引擎焦点环（到边界环绕）。
function M.move_dir(direction)
  if not ui_state or #M.rows() == 0 then
    return false
  end
  sync_focus()
  local index = nav.move(FOCUS, direction)
  if not index then
    return false
  end
  ui_state.index = index - 1
  return true
end

function M.render()
  if not ui_state then
    return false
  end
  local x, y, w, h = PANEL.x, PANEL.y, PANEL.w, PANEL.h
  local BOX = layout.BOX
  local rows = M.rows()
  -- 排版预算（M32）：列表/说明/提示同宽（面板宽 - 2×24）；右上角「状态」独占右半区。
  local ROW_W = layout.list_width(PANEL)
  local STATE_W = 240
  local OBJ_W = ROW_W - 32
  -- 目标列表首行（列表区最后一行之下，留一行间距）。
  local OBJ_Y = y + BOX.list_dy + ROW_LIMIT * ROW_H + 46
  sync_focus()
  preset_spawn()
  local P = preset_handle
  -- 选中行底衬：y 随选中行（index*38）移动；矩形 = 迁移前 `ui.panel("qst:sel", …)` 的
  -- 几何（left, top, right, bottom 四边，`prefab.set_rect` 签名）。
  local sel_top = y + BOX.list_dy + ui_state.index * ROW_H
  prefab.set_rect(P, "QuestSel", x + BOX.list_dx, sel_top, x + w - BOX.list_dx, sel_top + ROW_H)

  -- 空列表互斥：有任务时隐藏"暂无任务"。
  prefab.set_visible(P, "QuestEmpty", #rows == 0)

  -- 任务超过一页时按选中行滚动窗口（引擎 ScrollState，M20-02）。
  local first = nav.window(SCROLL, #rows, ROW_LIMIT, ROW_H, ui_state.index + 1)
  for slot = 0, ROW_LIMIT - 1 do
    local row_name = "QuestRow" .. (slot + 1)
    local todo_name = "QuestBullet" .. (slot + 1) .. "Todo"
    local done_name = "QuestBullet" .. (slot + 1) .. "Done"
    local index = first + slot
    local row = rows[index]
    if row then
      local label = string.format("%d. %s【%s】", index, row.name, M.state_text(row.state))
      label = (index == ui_state.index + 1) and ("> " .. label) or ("  " .. label)
      prefab.set_text(P, row_name, label)
      prefab.set_visible(P, row_name, true)
      -- 任务状态子弹（完成 done / 其他 todo，12×12 画在行首左侧）：同一矩形叠两张图互斥。
      local done = row.state == "completed"
      prefab.set_visible(P, done_name, done)
      prefab.set_visible(P, todo_name, not done)
    else
      prefab.set_visible(P, row_name, false)
      prefab.set_visible(P, done_name, false)
      prefab.set_visible(P, todo_name, false)
    end
  end

  -- 选中任务详情：发布者 + 目标进度（只读）。
  local row = rows[ui_state.index + 1]
  if row then
    prefab.set_text(P, "QuestState", "状态 " .. M.state_text(row.state))
    prefab.set_text(P, "QuestGiver", "发布者 " .. tostring(row.giver_name or "未知"))
    prefab.set_text(P, "QuestObjTitle", "目标：")
    prefab.set_visible(P, "QuestState", true)
    prefab.set_visible(P, "QuestGiver", true)
    prefab.set_visible(P, "QuestObjTitle", true)
    local objectives = M.objectives()
    for i = 1, ROW_LIMIT do
      local objective = objectives[i]
      local id = "QuestObj" .. i
      if objective then
        local mark = objective.done and "✓" or "…"
        local text = string.format("%s %s %d/%d", mark, tostring(objective.label), objective.current, objective.target)
        prefab.set_text(P, id, text)
        prefab.set_visible(P, id, true)
      else
        prefab.set_visible(P, id, false)
      end
    end
  else
    prefab.set_visible(P, "QuestState", false)
    prefab.set_visible(P, "QuestGiver", false)
    prefab.set_visible(P, "QuestObjTitle", false)
    for i = 1, ROW_LIMIT do
      prefab.set_visible(P, "QuestObj" .. i, false)
    end
  end

  prefab.set_text(P, "QuestHint", hint_line())
  return true
end

function M.open()
  ui_state = { index = 0 }
  state.set_mode("Quest")
  audio.ui("ui_click")
  preset_show()
  M.render()
  return true
end

function M.close()
  if not ui_state then
    return false
  end
  ui_state = nil
  clear_ui()
  preset_hide()
  nav.close(FOCUS)
  nav.scroll_close(SCROLL)
  if state.is_mode("Quest") then
    state.set_mode("Playing")
  end
  audio.ui("ui_cancel")
  return true
end

function M.reset()
  ui_state = nil
  clear_ui()
  preset_hide()
  nav.close(FOCUS)
  nav.scroll_close(SCROLL)
  return true
end

return M
