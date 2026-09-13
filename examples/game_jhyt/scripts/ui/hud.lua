-- hud.lua：《江湖有田》HUD 与提示反馈（M19-02）。
--
-- 职责（验收项①：HUD 显示生命、体力、内力、时间、天气、金钱和快捷栏）：
--   * 右上状态面板：生命(气血)/体力/内力/时间(天·季·时:分)/天气/金钱/地图/快捷栏
--   * 左下提示区：state.last_action → 可读中文反馈 + 新手引导当前步骤
--   * 顶部键位提示
--
-- 绘制用 `ui.*`（text_at/panel/hide，与 shop.lua 同一套坐标与用法）；每帧重绘，
-- 同一 id 覆盖写入，未使用的 id 显式 hide。
--
-- 文本宽度：中文字形宽度约为 ASCII 的 2 倍，本模块用「显示宽度单位」做截断
-- （clip），保证中文不溢出面板。截断只是显示层处理，不改变数据。
--
-- input.lua 每帧调用 draw() 与 text()；本模块不 require input（避免 require 环）。
local state = require("core.state")
local combat = require("systems.combat")
local weather = require("world.weather")
local maps = require("data.maps")
local tutorial = require("ui.tutorial")

local M = {}

-- 状态面板几何（1280×720 视口；右列不与对话/商店/背包/暂停面板重叠）。
local PANEL = { x = 996, y = 8, w = 276, h = 300 }
local LINE_H = 30
local LINE_FONT = 14
local PANEL_LINES = 10

local WEATHER_TEXT = { sunny = "晴", rainy = "雨" }
local SEASON_TEXT = { spring = "春", summer = "夏", autumn = "秋", winter = "冬" }

-- last_action 精确值 → 中文反馈。
local ACTION_TEXT = {
  none = "待机",
  move = "移动",
  till = "已翻土",
  rain_watered = "雨水浇灌",
  already_watered = "今日已浇水",
  tired = "体力不足",
  wrong_season = "季节不符",
  wrong_land = "土地不符",
  no_seed = "没有种子",
  not_farmland = "不是药田",
  out_of_bounds = "超出地图范围",
  no_target = "面前没有可交互目标",
  no_player = "玩家实体不存在",
  need_hoe = "需要装备锄头",
  need_watering_can = "需要装备水壶",
  not_enough = "材料/数量不足",
  full = "背包已满",
  crafting = "制作中",
  crafting_pending = "制作中不可拆除",
  poor = "金币不足",
  limit = "已达每日限购",
  not_sold = "本店不出售该物品",
  not_bought = "本店不收购该物品",
  no_effect = "该物品没有使用效果",
  unknown_item = "未知物品",
  bad_count = "数量无效",
  build_failed = "建造失败",
  no_selection = "未选建筑",
  max_level = "已达最高等级",
  no_building = "此处没有建筑",
  sleep = "睡下了，新的一天",
  practice_breath = "打坐练气，内力 +5",
  dialogue = "与 NPC 交谈中",
  shop = "商店营业中",
  dialogue_open = "对话开始",
  gift = "送礼成功",
  no_npc_near = "面前没有 NPC",
  not_giftable = "对方不收此物",
  unknown_quest = "未知任务",
  already_in_progress = "任务已在进行中",
  already_completed = "任务已完成",
  not_accepted = "尚未接取该任务",
  objectives_unmet = "任务目标未达成",
  quest_failed = "任务已失败"
}

-- 「前缀 → 文案」规则（顺序敏感：更具体的前缀放前面）。
local ACTION_PREFIX = {
  { "build_blocked", "建造受阻" },
  { "build_mode_on", "进入建造模式" },
  { "build_mode_off", "退出建造模式" },
  { "build_", "建造成功：" },
  { "demolish_", "已拆除：" },
  { "upgrade_", "升级成功：" },
  { "select_", "已选建筑：" },
  { "plant_", "已播种：" },
  { "water_", "已浇水：" },
  { "harvest_", "已收获：" },
  { "craft_start_", "开始制作：" },
  { "craft_collect_", "收取产出：" },
  { "buy_", "购买成功：" },
  { "sell_", "出售成功：" },
  { "use_", "使用成功：" },
  { "gift_", "送礼成功：" }
}

-- 建造冲突原因（building.lua 的逐格冲突枚举）→ 中文。
local BUILD_REASON = {
  out_of_bounds = "超出地图范围",
  ground = "地表不允许建造",
  solid = "已有阻挡",
  exit = "挡住出口",
  road = "压在道路上",
  farm_plot = "已有农田",
  building = "已有建筑",
  unknown_building = "未知建筑"
}

-- 按显示宽度截断，超出加省略号（CJK/全角记 2 单位，其余记 1）。
-- 只用 Lua 5.4 标准 utf8 库；非法字节序列不会被喂进来（文案均为常量/数据）。
function M.clip(text, max_units)
  local s = tostring(text or "")
  local limit = tonumber(max_units) or 0
  if limit <= 0 then
    return ""
  end
  local used = 0
  local out = {}
  for _, code in utf8.codes(s) do
    local units = code >= 0x2E80 and 2 or 1
    if used + units > limit - 1 then
      return table.concat(out) .. "…"
    end
    out[#out + 1] = utf8.char(code)
    used = used + units
  end
  return table.concat(out)
end

local function item_name(item_id)
  if type(item_id) ~= "string" or item_id == "" then
    return nil
  end
  local ok, def = pcall(item.def, item_id)
  if ok and type(def) == "table" and def.name then
    return tostring(def.name)
  end
  local items = require("data.items")
  local tools = require("data.tools")
  local fallback = items[item_id] or tools[item_id]
  if type(fallback) == "table" and fallback.name then
    return tostring(fallback.name)
  end
  return item_id
end

local function equipment()
  local ok, list = pcall(inv.equip_list, state.inventory)
  if ok and type(list) == "table" then
    return list
  end
  return {}
end

-- 时:分（引擎 clock.now().minute_of_day 是当天已过分钟数）。
local function clock_text()
  local now = clock.now()
  local day = tonumber(now.day) or 1
  local minute = tonumber(now.minute_of_day) or tonumber(now.minute) or 0
  local hour = math.floor(minute / 60)
  local rest = minute % 60
  return string.format("第%d天 %s %02d:%02d", day, SEASON_TEXT[state.season] or "?", hour, rest)
end

local function weather_text()
  return WEATHER_TEXT[weather.current()] or tostring(weather.current())
end

local function hp_text()
  if combat.is_player_dead() then
    return "生命(气血) 0/… 战败"
  end
  return "生命(气血) " .. tostring(combat.player_hp()) .. "/" .. tostring(combat.player_max_hp())
end

-- 快捷栏：当前武器 / 选中工具 / 选中种子 + 数量。
local function hotbar_lines()
  local eq = equipment()
  local weapon = item_name(eq.weapon) or "空手"
  local tool = item_name(eq.tool or state.selected_tool) or "无"
  local seed = item_name(state.selected_seed) or "无"
  local seed_count = 0
  if state.selected_seed then
    local ok, count = pcall(inv.count, state.inventory, state.selected_seed)
    if ok then
      seed_count = tonumber(count) or 0
    end
  end
  return {
    "快捷栏",
    "武器 " .. weapon .. " · 工具 " .. tool,
    "种子 " .. seed .. " x" .. tostring(seed_count)
  }
end

-- 状态面板逐行文案（headless 验收直接读这里，保证与绘制同源）。
function M.status_lines()
  local lines = {
    hp_text(),
    "体力 " .. tostring(state.energy) .. " · 内力 " .. tostring(state.inner_power),
    "时间 " .. clock_text(),
    "天气 " .. weather_text(),
    "金钱 " .. tostring(state.money) .. " 文",
    "地图 " .. tostring((maps.get(map.current() or scene.reload()) or {}).display or "未知")
  }
  for _, line in ipairs(hotbar_lines()) do
    lines[#lines + 1] = line
  end
  return lines
end

-- 单行 HUD 文案（update() 返回值 / headless 断言用）。建造模式下追加
-- footprint 预览：可放置 / 逐格冲突的首个原因（M18-02 验收项①的既有断言
-- `input.update():match('building')` 依赖这里的 `building` 字面量）。
function M.text()
  local lines = M.status_lines()
  local text = table.concat({
    lines[1],
    "体力 " .. tostring(state.energy),
    "内力 " .. tostring(state.inner_power),
    "时间 " .. clock_text(),
    "天气 " .. weather_text(),
    "金钱 " .. tostring(state.money) .. " 文",
    lines[6]
  }, " · ")
  text = text .. " · " .. tostring(lines[7]) .. "：" .. tostring(lines[8]) .. " · " .. tostring(lines[9])
  local building = require("systems.building")
  if building.build_mode() then
    local preview = building.preview(state.facing)
    local def = preview.building and require("data.buildings").get(preview.building)
    local cells = #preview.cells
    local verdict
    if not preview.building then
      verdict = "未选建筑"
    elseif preview.ok then
      verdict = "可放置 " .. tostring(cells) .. " 格"
    else
      local first = preview.conflicts[1]
      verdict = "占用冲突 " .. tostring(first and first.reason or "?")
        .. (first and ("@" .. tostring(first.x) .. "," .. tostring(first.y)) or "")
    end
    text = text .. " · 建造[" .. tostring(def and def.name or "?") .. "] " .. verdict
  end
  return text
end

-- ---------- 提示与错误反馈 ----------

-- `build_blocked_<reason>@x,y` → 中文原因 + 格坐标。
local function build_blocked_text(action)
  local reason, cell = action:match("^build_blocked_([^@]+)@(.+)$")
  if not reason then
    reason = action:match("^build_blocked_?(.*)$")
    cell = nil
  end
  local label = BUILD_REASON[reason] or (reason ~= "" and reason or "未知原因")
  local text = "建造受阻：" .. label
  if cell and cell ~= "" then
    text = text .. " @ " .. cell
  end
  return text
end

-- last_action → 可读中文；未知取值原样返回（便于诊断）。
function M.feedback(action)
  local value = tostring(action or "")
  if value == "" then
    return ""
  end
  if value:sub(1, 14) == "build_blocked_" or value == "build_blocked" then
    return build_blocked_text(value)
  end
  local exact = ACTION_TEXT[value]
  if exact then
    return exact
  end
  for _, rule in ipairs(ACTION_PREFIX) do
    local prefix, label = rule[1], rule[2]
    if value:sub(1, #prefix) == prefix then
      local rest = value:sub(#prefix + 1)
      if rest == "" or prefix == "build_mode_on" or prefix == "build_mode_off" then
        return label
      end
      return label .. item_name(rest)
    end
  end
  return value
end

-- 提示区文案（左下一行）。
function M.feedback_line()
  local text = M.feedback(state.last_action)
  if text == "" or text == "待机" or text == "移动" then
    return ""
  end
  return "› " .. text
end

-- ---------- 绘制 ----------

local function clear_dynamic()
  ui.hide("hud:keys")
  ui.hide("hud:feedback")
  ui.hide("hud:tutorial")
  for i = 1, PANEL_LINES do
    ui.hide("hud:line" .. i)
  end
end

-- 键位提示兜底：调用方（input.lua）传入**当前设备**的那一套说明
-- （`input.key_hints()`，M20-09：键鼠/手柄二选一）；缺省时用键鼠静态文案。
local DEFAULT_KEYS =
    "WASD 移动 · E 交互 · J 攻击 · K 武学 · G 送礼 · T 睡觉 · Q 修炼 · B 建造 · I 背包 · L 任务 · P 暂停"

-- 每帧重绘：状态面板 + 键位提示 + 提示区（对话态隐藏左下两行，避免与
-- 对话面板重叠——对话自己带提示）。`key_hints` 为空时用静态兜底。
function M.draw(key_hints)
  local x, y, w, h = PANEL.x, PANEL.y, PANEL.w, PANEL.h
  ui.panel("hud:box", x, y, w, h, 0.08, 0.09, 0.12, 0.62)
  local lines = M.status_lines()
  for i = 1, PANEL_LINES do
    local id = "hud:line" .. i
    local line = lines[i]
    if line then
      ui.text_at(id, x + 12, y + 10 + (i - 1) * LINE_H, M.clip(line, 34), 1.0, LINE_FONT)
    else
      ui.hide(id)
    end
  end

  local hint = key_hints
  if type(hint) ~= "string" or hint == "" then
    hint = DEFAULT_KEYS
  end
  ui.text_at("hud:keys", 16, 8, hint, 1.0, 13.0)

  if state.is_mode("Dialogue") then
    ui.hide("hud:feedback")
    ui.hide("hud:tutorial")
  else
    local feedback = M.feedback_line()
    if feedback ~= "" then
      ui.text_at("hud:feedback", 16, 660, M.clip(feedback, 60), 1.0, 16.0)
    else
      ui.hide("hud:feedback")
    end
    local prompt = tutorial.prompt()
    if prompt then
      ui.text_at("hud:tutorial", 16, 686, M.clip(prompt, 62), 1.0, 15.0)
    else
      ui.hide("hud:tutorial")
    end
  end
  return true
end

-- 场景切换 / 新档：清掉 HUD 动态元素（模式由调用方复位）。
function M.reset()
  clear_dynamic()
  return true
end

return M
