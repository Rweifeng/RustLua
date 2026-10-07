-- hud.lua：《江湖有田》HUD 与提示反馈（M19-02）。
--
-- 职责（验收项①：HUD 显示生命、体力、内力、时间、天气、金钱和快捷栏）：
--   * 右上状态面板（6 行）：① 生命(气血) ② 体力 + 内力 ③ 时间(天·季·时:分) + 天气
--     ④ 金钱 ⑤ 武器 + 工具 ⑥ 种子 + 数量
--     （地图名不再进面板：场景自带的 `HudTitle` 已在左上角显示同一份名字）
--   * 左上角两行：七日目标行（贴顶）+ 动作反馈行（state.last_action → 可读中文）
--   * 底部通栏一行：新手引导当前步骤（只在引导十步内出现；文案最长，只有通栏装得下）
--
-- 这里**不画操作说明**：常驻的键位通栏被撤下（画面下半部留给世界），完整键位表在
-- 暂停菜单的「帮助」页（`ui.pause_menu`，数据源 `ui.hints.guide_rows`）。
--
-- 绘制统一走 `ui/ui_text.lua`（M30）：它在绘制的同时登记"文字 + 字号 + 可用宽度"，
-- 于是"有没有溢出面板"变成可机器验收的不变量（`ui_text.audit()`）；本文件因此不再
-- 直接调 `ui.text_at`/`ui.hide`。每帧重绘，同一 id 覆盖写入，未使用的 id 显式 hide。
--
-- 文本宽度：**按引擎真实字宽排**（`ui.measure_text`）。此前是"中文字形宽度约为 ASCII
-- 的 2 倍"的启发式截断（`M.clip`），遇到长数字、宽拉丁字母或中英混排仍会溢出面板。
-- 截断只是显示层处理，不改变数据。
--
-- input.lua 每帧调用 draw() 与 text()；本模块不 require input（避免 require 环）。
local state = require("core.state")
local combat = require("systems.combat")
-- 招式数据与玩家按键绑定（快捷栏卡序 = `data.skills.binding` 的顺序；与触屏命中测试同源）。
local skills_data = require("data.skills")
-- M34-03：状态图标的第一次真实驱动（账本本身不依赖 UI，方向是 HUD → status）。
local status = require("systems.status")
local weather = require("world.weather")
local maps = require("data.maps")
local goals = require("systems.goals")
local tutorial = require("ui.tutorial")
local ui_text = require("ui.ui_text")
-- M32 竖版：面板几何与行宽预算的唯一来源（`ui/layout.lua`）。
local layout = require("ui.layout")
-- docs/48 ui 换肤接线：面板背景/行图标/状态图标/条图标贴图。
local skin = require("ui.skin")
-- 主角头像（docs/45 欠账 ②）：贴图与对话立绘同源（`ui/portrait.lua`，同一批生成器出的
-- 128×128 立绘），按 `state.appearance` 取那张脸。
local portrait = require("ui.portrait")

local M = {}

-- 状态面板几何（横屏：右侧信息栏顶部，与对话面板同栏、上下排布）。
-- 竖屏曾改到**底部通栏**（竖屏宽度只有 720，右上角放不下"面板 + 资源条 + 反馈行"）；
-- 2026-09-27 转横屏：信息栏右置（980..1280），面板与对话/反馈行共用一栏。
local PANEL = layout.PANEL.hud_info
-- 状态面板行距/字号：**6 行 × 28px + 顶部 10px = 178 ≤ 面板高 180**，字号 18。
-- 拆到 6 行的目的是"每行都装得下原始文案"：18px 下六行里最长的一行实测 241.1px
-- （`第999天 冬 12:00 · 天气 晴`），预算 262px（`PANEL_TEXT_W`）留 21px 余量。
-- 行数/字号是 `jhyt_m19_ui`/`jhyt_m30_cjk_text` 的验收面（`status_lines()` 恰好 6 条、
-- 逐条按 `PANEL_TEXT_W` 量原始文案），改动要同步那两处断言。
local LINE_H = 28
local LINE_FONT = 18
local PANEL_LINES = 6
-- 每行左侧的"分类色块"（引擎文字没有着色通道，用色块做视觉分组）：
-- ① 生命=绿 ② 体力/内力=青 ③ 时间/天气=金 ④ 金钱=蓝 ⑤ 武器=紫 ⑥ 种子=棕。
local CHIP = { w = 6, h = 16, gap = 4 }
local CHIP_COLORS = {
  { r = 0.36, g = 0.82, b = 0.45 },
  { r = 0.42, g = 0.78, b = 0.72 },
  { r = 0.95, g = 0.76, b = 0.30 },
  { r = 0.38, g = 0.66, b = 0.96 },
  { r = 0.78, g = 0.56, b = 0.96 },
  { r = 0.82, g = 0.62, b = 0.42 },
}
-- 面板顶部的金色细条（与色块同色系，让"底栏"有明确的收口）。
local ACCENT = { h = 3, r = 0.93, g = 0.75, b = 0.35, a = 0.9 }
-- docs/48 ui 换肤接线：状态行图标（行号 → skin key；无图标行为 nil）。
local LINE_ICONS = { [1] = "hud_icon_hp", [2] = "hud_icon_stamina", [4] = "hud_icon_coin" }
-- 状态图标注册表（8 张全列出；绘制按真实状态是否存在）。
local STATUS_ICONS = {
  poison = "hud_status_poison",
  buff = "hud_status_buff",
  bleed = "hud_status_bleed",
  debuff = "hud_status_debuff",
  sleep = "hud_status_sleep",
  stealth = "hud_status_stealth",
  weather = "hud_status_weather",
  festival = "hud_status_festival"
}
-- 主角头像（docs/45 欠账 ②：右上角那个 48×48 框一直是空的）。
--
-- 矩形 = 预设 `HudAvatarFrame`（offsets 924,16,972,64）**内缩 4px**：框的木边 + 金线正好占掉
-- 外圈 4px，脸落在金线以内 ⇒ 两边互不遮挡，谁压谁都不会咬掉对方的像素（这条是"为什么不进
-- 预设"之外还要钉的一维：改框的几何就得同步改这里）。
-- 为什么留动态层：贴图是**运行期取值**（`jianghu_set_appearance` 能换形象、随存档持久化），
-- 引擎没有运行期换贴图 API —— 与对话立绘同一个理由（见 `world/dialogue.lua` 的 `dlg:portrait`）。
local AVATAR = { id = "hud:avatar_face", x = 928, y = 20, size = 40 }
-- 当前真实激活的状态键（有玩法来源的才返回；其余状态机制未实现 → 不画但已就绪）。
local function active_status_keys()
  local out = {}
  -- 中毒等按固定步结算的瞬时状态（M34-03）：账本里有就画，**种类名即图标键**
  -- （不写 `if kind == "poison"`；将来加减速/流血的形状是"数据 + 一张图"，这里不用改）。
  -- 顺序 = 施加顺序（`systems/status.lua` 的 `list` 是数组），因此图标排布是确定的。
  for _, entry in ipairs(status.list("player")) do
    if STATUS_ICONS[entry.kind] then
      out[#out + 1] = entry.kind
    end
  end
  if require("player.stats").buff_count("player") > 0 then
    out[#out + 1] = "buff"
  end
  if weather.is_rainy() then
    out[#out + 1] = "weather"
  end
  return out
end
-- 可用文字宽度：面板内左右各留 12，再扣除色块与间距（整行行宽为设计宽两侧各留 24）。
local PANEL_TEXT_W = PANEL.w - 24 - CHIP.w - CHIP.gap - 4

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
  -- 当日社交饮品（M34-05）：同一日第二杯不再加社交，物品也不扣。
  tea_today = "今日已饮过茶",
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
  quest_failed = "任务已失败",
  -- M25 受击反馈与攻击失败原因（此前攻击失败返回值被丢弃，按下去没有任何反馈）。
  hurt = "受击僵直",
  stunned = "僵直中无法出手",
  dead = "已经倒下了",
  mode = "当前状态无法出手",
  player_dead = "对方已经收手",
  cooldown = "招式还在冷却",
  inner_power = "内力不足",
  unknown_skill = "未知招式",
  -- 招式扩充（M34-04）的两个新原因。`dash_blocked` 是"贴墙按轻功"（一格都走不了，
  -- 所以内力与冷却一样不收）；`not_hitbox` 玩家按不出来（input.lua 按 kind 分派，
  -- 追风步永远不会走 J/K），但底座确实会返回它——而 jhyt_m26_hud_feedback 扫的是
  -- 源码里的 `return false, "…"`，只要那个字面量存在，这里就得有中文。
  dash_blocked = "身前没有落脚点",
  not_hitbox = "这一招不出手",
  -- 瞬时状态（M34-03）：毒蝎咬中之后挂在身上的中毒。键来自 `data/combat.lua` 的
  -- `status.poison.feedback`，由 jhyt_m34_status 逐条比对（数据里加了新状态却没在这儿配文案 → 门红）。
  poisoned = "中毒了，气血在流失",
  -- M26 补齐（docs/37 4-13）：以下取值此前在表里没有条目，HUD 直接把英文键显示给玩家。
  -- 取值来源是各模块的 `return false, "<reason>"`，由 jhyt_m26_hud_feedback 扫描源码守住。
  gathered = "已采集",
  gather_blocked = "采集受阻",
  no_node = "这里没有可采集的东西",
  no_map = "当前地图不可采集",
  no_farm = "这里没有农田",
  unknown_recipe = "未知配方",
  no_station = "没有对应设施",
  station_level = "设施等级不足",
  busy = "设施正在忙",
  no_job = "没有可收取的产出",
  unknown_shop = "未知店铺",
  closed = "店铺已打烊",
  -- 冬至封山（M34-02）：山道往洞里去的那个方向今日不走；回村方向不受影响。
  closed_festival = "山道封了，进不得",
  no_shop = "附近没有店铺",
  missing_map = "目标地图不存在",
  map_goto_failed = "切换地图失败",
  -- 建造冲突原因（BUILD_REASON 的键）：正常走 `build_blocked_<reason>@x,y` 分支，
  -- 但单独出现时也要有中文，否则诊断输出里会露出英文键。
  farm_plot = "已有农田",
  building = "已有建筑",
  ground = "地表不允许建造",
  solid = "已有阻挡",
  exit = "挡住出口",
  road = "压在道路上",
  unknown_building = "未知建筑",
  -- 掉落与战败（M28 / 计划 G3）：`loot_<item>` 走前缀按物品名解析；
  -- 只掉铜钱时没有物品可解析，故单列一条。
  loot_money = "战利品：铜钱"
}

-- 「前缀 → 文案」规则（顺序敏感：更具体的前缀放前面）。
-- 第三个字段可选：缺省按物品名解析剩余部分（`item_name`），
--   "map"   → 按地图名解析（`data.maps.get(id).name`）
--   "plain" → 剩余部分原样拼接
local ACTION_PREFIX = {
  { "build_blocked", "建造受阻" },
  { "build_mode_on", "进入建造模式" },
  { "build_mode_off", "退出建造模式" },
  { "build_", "建造成功：" },
  { "demolish_", "已拆除：" },
  { "upgrade_", "升级成功：" },
  -- 必须排在 "select_" 之前：否则 "select_seed_lingzhi" 会命中 "select_"
  -- 被显示成「已选建筑：灵芝」（docs/37 4-13 记录的前缀误命中）。
  { "select_seed_", "已选种子：" },
  { "select_", "已选建筑：" },
  { "equip_", "已装备：" },
  { "tutorial_reward_", "引导奖励：" },
  { "exit_", "前往：", "map" },
  { "need_", "需要：" },
  { "plant_", "已播种：" },
  { "water_", "已浇水：" },
  { "harvest_", "已收获：" },
  { "gather_", "已采集：" },
  { "craft_start_", "开始制作：" },
  { "craft_collect_", "收取产出：" },
  { "buy_", "购买成功：" },
  { "sell_", "出售成功：" },
  { "use_", "使用成功：" },
  { "gift_", "送礼成功：" },
  -- 掉落（M28）：`loot_wolf_pelt` →「战利品：狼皮」（缺省按物品名解析）。
  { "loot_", "战利品：" },
  -- 战败结算（M28）：明细是拼好的中文（"铜钱 6 文、狼皮 x1"），不能再按物品名解析。
  { "death_penalty_", "战败回庄，损失 ", "plain" }
}

-- 未知取值的兜底前缀：不再把英文键直接摆给玩家，但保留原值便于诊断。
local UNKNOWN_LABEL = "未知反馈（"
local UNKNOWN_SUFFIX = "）"

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

-- 文字裁剪与换行都在 `ui/ui_text.lua` + `ui/text_fit.lua`（M30）：
--   `ui_text.draw(id, x, y, size, budget, text)` 按真实字宽裁剪并登记；
--   `ui_text.draw_wrapped(...)` 按宽度断行（标点禁则：不落行首）。
-- 历史上那个"CJK 记 2 单位"的 `hud.clip` 已删除：它遇到长数字与宽拉丁字母会算错宽度，
-- 属于"看起来在截断、实际会溢出"的坏行为。

local function item_name(item_id)
  if type(item_id) ~= "string" or item_id == "" then
    return nil
  end
  -- 直调引擎注册表：`item.def` 未注册的 id 返回 nil。
  -- 此前这里还套了 `pcall` 并兜底去翻 `data.items`/`data.tools`——那会让"引擎注册表没生效"
  -- （`inventory.ensure` 没跑）静默地看起来一切正常，属于不该留的兼容写法，M30 删掉。
  local def = item.def(item_id)
  if type(def) == "table" and def.name then
    return tostring(def.name)
  end
  return item_id
end

local function equipment()
  local list = inv.equip_list(state.inventory)
  if type(list) == "table" then
    return list
  end
  return {}
end

-- 时:分（引擎 `clock.now()` 给当天已过的分钟数 `minute_of_day`）。
local function clock_text()
  local now = clock.now()
  local day = tonumber(now.day) or 1
  local minute = tonumber(now.minute_of_day) or 0
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

-- 快捷栏拆成两行（武器+工具 / 种子+数量）：合起来最长可达 296px（四个 4 字物品名），
-- 面板预算只有 262px；拆开后两行各自最坏 247 / 167，都留得住余量。
local function hotbar_lines()
  local eq = equipment()
  local weapon = item_name(eq.weapon) or "空手"
  local tool = item_name(eq.tool or state.selected_tool) or "无"
  local seed = item_name(state.selected_seed) or "无"
  local seed_count = 0
  if state.selected_seed then
    seed_count = tonumber(inv.count(state.inventory, state.selected_seed)) or 0
  end
  return {
    "武器 " .. weapon .. " · 工具 " .. tool,
    "种子 " .. seed .. " x" .. tostring(seed_count),
  }
end

-- 前向声明：这两个绘制函数在后面定义（`M.draw` 在文件中部），Lua 的局部变量
-- 只在其声明之后可见——写成 `local function` 会让 `M.draw` 里的调用落到 nil 上，
-- `pcall` 又会把错误吞掉（画面上什么都不出现，却没有任何报错）。M29 实测踩过，
-- 因此这里显式前向声明，并在验收样例里直接断言元素真的画出来了。
local draw_bars
local draw_weather_visual
local BAR

-- 状态面板逐行文案（headless 验收直接读这里，保证与绘制同源）。
--
-- 6 行、每行最多两个字段（M33 的"零截断"改版：原先 4 行里两行实测超预算，
-- 玩家看到的省略号就是这么来的）：
--   ① 生命(气血)          ② 体力 + 内力        ③ 时间 + 天气
--   ④ 金钱                ⑤ 武器 + 工具        ⑥ 种子 + 数量
-- 地图名不在这里（场景自带的 `HudTitle` 已显示），"快捷栏"这一标签也不再出现——
-- 它只是给下面两行起头，占了 43px 预算却不带任何数值。
-- 每行的**原始**文案都要 ≤ `PANEL_TEXT_W`，由 `ui_text.audit()` 逐条量真实字宽。
function M.status_lines()
  local hotbar = hotbar_lines()
  return {
    hp_text(),
    "体力 " .. tostring(state.energy) .. " · 内力 " .. tostring(state.inner_power),
    clock_text() .. " · 天气 " .. weather_text(),
    "金钱 " .. tostring(state.money) .. " 文",
    tostring(hotbar[1] or "武器 空手 · 工具 无"),
    tostring(hotbar[2] or "种子 无 x0"),
  }
end

-- 单行 HUD 文案（update() 返回值 / headless 断言用）。建造模式下追加
-- footprint 预览：可放置 / 逐格冲突的首个原因（M18-02 验收项①的既有断言
-- `input.update():match('building')` 依赖这里的 `building` 字面量）。
function M.text()
  local lines = M.status_lines()
  -- 单行摘要由这 6 行拼接（`input.update()` 的返回值、headless 断言读的是它）。
  local text = table.concat(lines, " · ")
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
      -- M26：冲突原因用中文（此前直接把 `solid`/`farm_plot` 这类英文键拼给玩家看）。
      verdict = "占用冲突 " .. (first and (BUILD_REASON[first.reason] or first.reason) or "?")
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

-- 地图 id → 中文名（`exit_<map>` 用；缺名字时退回 id，至少带上「前往：」标签）。
-- `data/maps.lua` 的地图显示名是 `title`（如「山庄 · 药田垂直切片」），
-- 取 `·` 之前的一段作为短名（HUD 一行只有 60 显示宽度，整串标题会挤掉别的信息）。
local function map_name(map_id)
  local def = maps.get(map_id)
  if type(def) == "table" then
    local title = def.short_name or def.title
    if type(title) == "string" and title ~= "" then
      local short = title:match("^([^·]+)") or title
      return (short:gsub("%s+$", ""))
    end
  end
  return item_name(map_id)
end

-- last_action → 可读中文。
-- 未知取值不再原样露出英文键（M26 / docs/37 4-13）：返回带中文标签的兜底文案，
-- 原值仍保留在括号里供诊断，避免"玩家看到一个陌生英文单词、开发者也定位不到"。
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
    local prefix, label, resolver = rule[1], rule[2], rule[3]
    if value:sub(1, #prefix) == prefix then
      local rest = value:sub(#prefix + 1)
      if rest == "" or prefix == "build_mode_on" or prefix == "build_mode_off" then
        return label
      end
      if resolver == "map" then
        return label .. map_name(rest)
      end
      if resolver == "plain" then
        return label .. rest
      end
      return label .. item_name(rest)
    end
  end
  return UNKNOWN_LABEL .. value .. UNKNOWN_SUFFIX
end

-- 提示区文案（左下一行）。
function M.feedback_line()
  local text = M.feedback(state.last_action)
  if text == "" or text == "待机" or text == "移动" then
    return ""
  end
  return "› " .. text
end

-- 七日目标行（左上角第一行；M27）。目标线走完（`goals.prompt()` 返回空串）时这一行不画，
-- 由 `ui_text` 的登记表决定"这一帧没有 hud:goals"。
function M.goals_line()
  local text = goals.prompt()
  if type(text) ~= "string" or text == "" then
    return ""
  end
  return "目标 " .. text
end

-- ---------- 绘制 ----------

-- 每帧重绘：状态面板 + 资源条 + 提示区。
--
-- 两条位置规则（都有机器检查，不靠自觉）：
--   * 状态面板在右侧信息栏顶部，对话面板在它下方 —— `layout.CONTEXT.gameplay` 把两者
--     放进同一同屏组，`layout.audit()` 断言不重叠，因此说话时状态照样可见。
--   * 屏上不常驻操作说明（见文件头）：原本的世界底部键位通栏已收进暂停菜单的「帮助」页，
--     底部一行让回给世界与触屏按钮簇。
-- docs/48 ui 换肤接线：建造预览图标（ghost_ok 可放置 / ghost_bad 冲突）。
-- building.preview 是逻辑 footprint（headless 验收用），这里的图标是它的可视反馈，
-- 画在资源条右侧（状态图标竖排的第三行位）。
local function draw_build_ghost()
  local ok, building_mod = pcall(require, "systems.building")
  if not ok or type(building_mod) ~= "table" or not building_mod.build_mode() then
    return
  end
  local preview = building_mod.preview(state.facing)
  local ghost_key = preview.ok and "build_ghost_ok" or "build_ghost_bad"
  skin.icon("hud:build_ghost", BAR.x + BAR.w + 10, BAR.y0 + 2 * 20, 24, 24, ghost_key)
end

-- docs/48 ui 换肤接线：招式快捷栏（skill_card 卡底 + 招式图标 + 招式名）。
-- 招式取 `data.skills.binding`（cast → dash → guard），**不是** `M.skills` 的字典序：
-- 字典序会把「普攻」排到第 8 张，还会把敌人专属的「山魈冲撞」放上玩家的栏。
-- 画在资源条下方（y296 起，不压引导行）。
--
-- **按列数换行，不是无限往右排**：这里曾经写死"一行排到底"，招式从 5 张扩到 8 张之后
-- 整行宽 404px，右缘到了设计 x=1404，而 `layout.DESIGN_W` 是 1280 ⇒ 第 6 张只有左半张在
-- 屏内、7/8 完全看不见，**而且没有一门门会红**：`layout.audit()` 只查 `M.PANEL` 注册过的
-- 面板，招式卡是 `ui.image`。所以每行列数由画布宽度推导（`skill_cols()`），新增招式只会
-- 往下多一行；真实几何由 `jhyt_m19_ui` 用 `ui.image_get` 读回逐张断言（越界即红）。
-- 往下加行的余量很薄：第二行下缘 396 距攻击键包围盒上缘 411 只有 15px（`audit_touch` ③
-- 会红在 `压住 UI 元素 hud_skill_bar`），所以超过 10 招要连同 `layout.TOUCH` 一起复核。
local SKILL_ICON_KEYS = {
  strike = "skill_basic_blade",
  palm_gust = "skill_palm_gust",
  crescent_slash = "skill_crescent_slash",
  iron_palm = "skill_iron_palm",
  leopard_step = "skill_leopard_step",
  chase_wind = "skill_chase_wind",
  guard_heart = "skill_guard_heart",
}
-- 卡片几何与 `layout.TOUCH_KEEP_CLEAR.hud_skill_bar` 的包围盒同源（那边是刻意抄的副本，
-- 挪位置要让校验器报警，所以不 require 回来）。
local SKILL_SLOT = 48
local SKILL_GAP = 4
-- 招式名：12px（`ui_text.audit` 的下限是 11px；3 个汉字量出来 36px，卡宽 48 装得下）。
local SKILL_NAME_FONT = 12.0
-- 已画出去多少张：`M.reset()` 与"招式变少"时按这个数把多余的 id 显式收掉。
-- 贴图不进每帧登记表，画过就一直留在屏上——少这一格就是"砍掉一招、它的卡还在"。
local skill_slots_drawn = 0

-- M70/B7 预设化：常驻 HUD 的静态骨架（面板/色块/头像框/行图标/资源条框+图标/招式卡底/
-- 天气双图/雨罩/结局横幅/文字槽位）搬进 `assets/prefabs/hud.prefab`，运行期元素
-- （资源条 fill 宽度、状态图标、招式图标/名、建造幽灵）留动态层。HUD 常驻不关屏，
-- 预设 spawn 一次、根常 shown；每帧只写内容（set_text）与显隐（set_visible）/宽度（set_rect）。
local PRESET_PATH = "assets/prefabs/hud.prefab"
local preset_handle = nil
local preset_spawned = false

-- M71 修复：场景切换重建 ECS 世界后旧句柄失效，纯布尔标志会漏判 →
-- `prefab.set_text` 报「没有 handle N」并被 lua-guard 停用回调（HUD 每帧 draw 首当其冲）；
-- 改成对句柄做真实探活（`prefab.root` 失效句柄抛错，pcall 兜底）。
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

local function preset_visible(name, visible)
  if not preset_handle then
    return false
  end
  local ok = pcall(prefab.set_visible, preset_handle, name, visible)
  if not ok then
    preset_handle = nil
    preset_spawned = false
  end
  return ok
end

local function preset_text(name, text)
  local ok, err = pcall(prefab.set_text, preset_handle, name, tostring(text or ""))
  if not ok then
    -- 实例失效（场景切换重建 ECS 后旧句柄的实体 id 可能被新世界复用，`prefab.root`
    -- 探活会误判活着——只有真正写 `Text` 组件时才发现组件不对）：置空句柄，
    -- 下一帧 `preset_spawn` 重新实例化。常驻 HUD 不 despawn，靠这条自愈。
    preset_handle = nil
    preset_spawned = false
  end
  return ok
end

local function preset_rect(name, x, y, w, h)
  if not preset_handle then
    return false
  end
  local ok = pcall(prefab.set_rect, preset_handle, name, x, y, x + w, y + h)
  if not ok then
    preset_handle = nil
    preset_spawned = false
  end
  return ok
end

-- 验收（jhyt_m19_ui 招式卡实体存在性断言）取 spawn 句柄；draw 前为 nil。
function M.preset_ref()
  return preset_handle
end

-- 一行放得下几张：从 `BAR.x` 到设计画布右缘，按"卡宽 + 间隔"取整（最后一张不吃间隔）。
local function skill_cols()
  local room = layout.DESIGN_W - BAR.x + SKILL_GAP
  return math.max(1, math.floor(room / (SKILL_SLOT + SKILL_GAP)))
end

--- 招式卡几何：**绘制与触屏命中测试的唯一出口**。
--- `ui/virtual_pad.lua` 直接 require 本模块问"第 i 张卡在哪、点它出哪一招"，所以卡片换行、
--- 挪位置、加招式都不需要同步改触屏那一份（曾经就是两份各写各的，扩招后卡片漂出了画布）。
--- HUD 不 require virtual_pad / input，没有 require 环。
--- 字段：`index`（1 基，也是 id 后缀）、`skill`/`action`（来自 `data.skills.binding`）、
--- `name`、`hold`（true = 按住才生效的格挡）、`x/y/w/h`（设计像素，左上原点）。
function M.skill_slots()
  local slots = {}
  local cols = skill_cols()
  local x0 = BAR.x
  local y0 = BAR.y0 + BAR.row * 3 + 30
  for index, entry in ipairs(skills_data.player_slots()) do
    local def = skills_data.get(entry.skill)
    slots[#slots + 1] = {
      index = index,
      skill = entry.skill,
      action = entry.action,
      name = def.name or entry.skill,
      hold = def.kind == "guard",
      x = x0 + ((index - 1) % cols) * (SKILL_SLOT + SKILL_GAP),
      y = y0 + math.floor((index - 1) / cols) * (SKILL_SLOT + SKILL_GAP),
      w = SKILL_SLOT,
      h = SKILL_SLOT,
    }
  end
  return slots
end

local function draw_skill_bar()
  local slots = M.skill_slots()
  for index = #slots + 1, skill_slots_drawn do
    -- 卡底在预设（HudSkillCardN），这里只收动态部分：图标与招式名。
    ui.clear("hud:skill_icon" .. index)
    ui_text.hide("hud", "hud:skill_name" .. index)
  end
  -- M70/B7：卡底（`HudSkillCard1..8`）在预设，按实际槽数显隐；图标/名留动态层。
  for index = 1, 8 do
    preset_visible("HudSkillCard" .. index, index <= #slots)
  end
  skill_slots_drawn = #slots
  for _, slot in ipairs(slots) do
    local id = slot.index
    local icon_key = SKILL_ICON_KEYS[slot.skill]
    if icon_key then
      -- 图标让位给招式名：24×24 落在卡上半（4..28），名字画在 y+30 起的那一行里。
      skin.icon("hud:skill_icon" .. id, slot.x + (slot.w - 24) / 2, slot.y + 4, 24, 24, icon_key)
    end
    local label = slot.name
    local width = ui.measure_text(label, SKILL_NAME_FONT).width
    ui_text.draw("hud", "hud:skill_name" .. id,
      slot.x + (slot.w - width) / 2, slot.y + 30, SKILL_NAME_FONT, slot.w, label)
  end
end

local function draw_avatar_face()
  local tex = portrait.texture_for(state.appearance, nil)
  if tex then
    ui.image(AVATAR.id, AVATAR.x, AVATAR.y, AVATAR.size, AVATAR.size, tex, 1.0, 1.0, 1.0)
    return true
  end
  -- 换到没有立绘的形象时必须显式收掉：`ui.image` 是跨帧持久的，上一帧的脸会一直留在框里
  -- （对话立绘同坑，见 `world/dialogue.lua` 的 `ui.clear("dlg:portrait")`）。
  ui.clear(AVATAR.id)
  return false
end

function M.draw()
  local x, y, w, h = PANEL.x, PANEL.y, PANEL.w, PANEL.h
  local in_dialogue = state.is_mode("Dialogue")
  preset_spawn()
  -- 主角头像：脸随 `state.appearance` 取（详见 `AVATAR` 常量注释）。
  draw_avatar_face()
  -- 动态层的招式名仍走 `ui_text` 登记（M30 排版检查的剩余条目）：重开自己那一轮。
  ui_text.begin("hud")
  -- M70/B7：静态骨架（头像框/面板底/收口条/分类色块/行图标/资源条框/条图标/招式卡底/
  -- 天气双图/雨罩/结局横幅）在预设里，不再逐帧画；每帧只写内容与显隐/宽度。
  -- 状态图标（贴图种类随激活状态）留动态层：激活画、未激活显式收。
  local statuses = active_status_keys()
  local drawn = {}
  for index, key in ipairs(statuses) do
    local tex_key = STATUS_ICONS[key]
    if tex_key then
      drawn[key] = true
      skin.icon("hud:status_" .. key, BAR.x + BAR.w + 10, BAR.y0 + (index - 1) * 20, 16, 16, tex_key)
    end
  end
  -- 未激活的一定要显式收掉：贴图不在 `ui_text` 的每帧登记表里，上一帧画过的 id 会一直留在
  -- 屏上——"毒已经过了 600 步、图标还亮着"就是这么来的（buff/weather 同样受益）。
  for key in pairs(STATUS_ICONS) do
    if not drawn[key] then
      ui.clear("hud:status_" .. key)
    end
  end
  local lines = M.status_lines()
  for i = 1, PANEL_LINES do
    preset_text("HudLine" .. i, lines[i] or "")
  end

  if in_dialogue then
    preset_visible("HudFeedback", false)
    preset_visible("HudTutorial", false)
    preset_visible("HudGoals", false)
  else
    local feedback = M.feedback_line()
    if feedback ~= "" then
      preset_text("HudFeedback", feedback)
      preset_visible("HudFeedback", true)
    else
      preset_visible("HudFeedback", false)
    end
    -- 七日目标（M27）：放在反馈行上方，不占引导行（两者可同时可见）。字号 15→17。
    local goals = M.goals_line()
    if goals ~= "" then
      preset_text("HudGoals", goals)
      preset_visible("HudGoals", true)
    else
      preset_visible("HudGoals", false)
    end
    -- 新手引导行走**底部通栏**（`ROW.tutorial_y`），不放右栏：右栏预算 276px 而
    -- 最长一条引导实测 648px，画出来只剩半句指令（省略号把"按 E 装备"裁掉了）。
    local prompt = tutorial.prompt()
    if prompt then
      preset_text("HudTutorial", prompt)
      preset_visible("HudTutorial", true)
    else
      preset_visible("HudTutorial", false)
    end
  end
  -- 天气可见变化 + 三条资源条 + 建造预览 + 招式栏 + 结局横幅：静态骨架在预设，
  -- 这里只写宽度（set_rect）、互斥显隐（set_visible）与动态元素（图标/名/幽灵）。
  draw_bars()
  -- docs/48 ui 换肤接线：建造预览图标 + 招式快捷栏（都画在资源条下方/右侧）。
  draw_build_ghost()
  draw_skill_bar()
  draw_weather_visual()
  -- v0.2 结局横幅：state.vars.ending 一旦写入即锁定，画在屏幕正中偏上，覆盖 HUD 之上。
  draw_ending_banner()
  return true
end

-- 结局文本表（与 data/quests/ending_*.lua 中 set_var={ending = "..."} 的取值一一对应）。
-- 这里刻意把「四种结局」集中在 UI 层给出人类可读的收束语，quest 数据层只写变量；
-- 增加/改名将导致横幅与任务脱钩，因此这份表中每一项都必须能在 quests/ending_*.lua 找到。
local ENDING_BANNERS = {
  field     = { title = "结局·归田",   line = "你放下刀，回到药田。江湖恩怨从此与你无涉。" },
  sect      = { title = "结局·承业",   line = "你接过青云门信物，从此山庄属你。" },
  renounce  = { title = "结局·断念",   line = "你把五份行气散留在剑坪，自废武功。" },
  vengeance = { title = "结局·雪恨",   line = "山门前，你与司徒魁了结这段因果。" }
}
local ENDING_BANNER_BOX = { w = 520, h = 96 }

function draw_ending_banner()
  local key = state.get_var("ending")
  local banner = (type(key) == "string") and ENDING_BANNERS[key] or nil
  if not banner then
    preset_visible("HudEndingBox", false)
    preset_visible("HudEndingTitle", false)
    preset_visible("HudEndingLine", false)
    return
  end
  -- M70/B7：横幅底/标题/正文在预设（几何写死：bx=(1280-520)/2、by=floor(720*0.34)）。
  preset_text("HudEndingTitle", banner.title)
  preset_text("HudEndingLine", banner.line)
  preset_visible("HudEndingBox", true)
  preset_visible("HudEndingTitle", true)
  preset_visible("HudEndingLine", true)
end

-- 资源条（M29 / 阶段五 P1）：生命 / 体力 / 内力三条 9-slice 条。
--
-- 为什么：`assets/art/ui/hud/` 这套条（`ui_bar_frame_9s` + 三种 fill）此前**从未被引用**
-- ——状态面板只有数字文本，玩家看不出"血条见底"。这里按**真实数值的比例**画填充宽度：
-- 生命取 `combat.player_hp()/player_max_hp()`，体力/内力取 `state.energy`、
-- `state.inner_power`（上限来自 `player/inventory.lua` 的 `max_energy/max_inner_power`，
-- 与 M28 起唯一上限来源一致）。宽度变化因此是"真的反映数值"，不是装饰。
BAR = {
  x = PANEL.x + 12,
  -- 2026-09-27 横屏：三条资源条在**面板正下方**（右栏顶部已被面板占满，
  -- 竖屏曾画在面板正上方）。条高三行（18px 间隔 24px）落在 138..192，贴着面板下沿；
  -- 条宽 160、条高 18。
  y0 = PANEL.y + PANEL.h + 6,
  row = 24,
  w = 160,
  h = 18,
  frame = "assets/art/ui/hud/ui_bar_frame_9s.png",
}
local BARS = {
  { id = "hud:bar_hp", fill = "assets/art/ui/hud/ui_bar_fill_hp.png" },
  { id = "hud:bar_stamina", fill = "assets/art/ui/hud/ui_bar_fill_stamina.png" },
  { id = "hud:bar_neili", fill = "assets/art/ui/hud/ui_bar_fill_neili.png" },
}

-- 三条资源条的当前比例（只读；headless 验收直接断言这里，与绘制同源）。
-- 上限一律取 `player/inventory.lua` 的 `max_energy()`/`max_inner_power()`
-- （M28 起唯一上限来源，`main.on_day_start` 与 `systems/death.lua` 用的是同一个）。
function M.bar_ratios()
  local inventory = require("player.inventory")
  local function ratio(value, max)
    local r = (tonumber(value) or 0) / max
    if r < 0 then
      return 0
    elseif r > 1 then
      return 1
    end
    return r
  end
  return {
    hp = ratio(combat.player_hp(), combat.player_max_hp()),
    stamina = ratio(state.energy, inventory.max_energy()),
    neili = ratio(state.inner_power, inventory.max_inner_power()),
  }
end

function draw_bars()
  local ratios = M.bar_ratios()
  local values = { ratios.hp, ratios.stamina, ratios.neili }
  -- M70/B7：条框（HudBarFrame1..3）与行首图标（HudBarIcon1..3）在预设，这里只按真实
  -- 比值写填充宽度（HudBarFill1..3，`set_rect` 四边签名）。宽度变化仍是"真的反映数值"。
  for index, bar in ipairs(BARS) do
    local y = BAR.y0 + (index - 1) * BAR.row
    local fill = math.max(1, math.floor(BAR.w * values[index] + 0.5))
    preset_rect("HudBarFill" .. index, BAR.x, y, fill, BAR.h)
  end
  return true
end

-- 雨天可见变化（M29 / 阶段五 P1）：全屏冷色薄雾 + 右上角天气图标。
--
-- 为什么这么做：此前"天气"在画面上只有状态面板里的一个字（晴/雨），玩家看不出
-- 下雨；而 `farm` 的雨天免浇水、NPC 的 `rain_place` 日程都以天气为条件——没有可见
-- 反馈时这些机制像是"没生效"。这里只做**表现**：不改玩法状态，晴天把两者清掉。
--
-- 全屏遮罩用 `ui.panel`（竖版 720×1280 设计视口，M32），图标用 `ui.image`（M29 起可读回，
-- 见 `ui.image_get`）。每帧重画，晴天 `ui.clear` 收干净。
local RAIN_OVERLAY = {
  id = "hud:rain",
  w = layout.DESIGN_W,
  h = layout.DESIGN_H,
  r = 0.16,
  g = 0.22,
  b = 0.34,
  a = 0.22,
}
-- 天气取值闭集是 `sunny|rainy`（`world/weather.lua` 的 KINDS），所以只有两张图被接线；
-- `ui_weather_cloud.png` 属于"没有对应天气取值"的资源，登记在 docs/48 的计划行里。
local WEATHER_ICONS = {
  sunny = "assets/art/ui/hud/ui_weather_sun.png",
  rainy = "assets/art/ui/hud/ui_weather_rain.png",
}
-- 图标落在**屏幕右上角**（竖屏顶部右侧没有别的元素；横屏时它贴在状态面板右上角）。
local RAIN_ICON = { id = "hud:weather_icon", size = 20, x = layout.DESIGN_W - 28, y = 6 }

function draw_weather_visual()
  local rainy = weather.is_rainy()
  -- M70/B7：全屏雨罩（HudRain）与天气图标（HudWeatherSun/Rain 同一矩形叠图、互斥显隐）
  -- 在预设；每帧按天气取值切换，晴天把雨罩与雨图标收掉。
  preset_visible("HudRain", rainy)
  preset_visible("HudWeatherSun", not rainy)
  preset_visible("HudWeatherRain", rainy)
  return true
end

-- 场景切换 / 新档：清掉 HUD 动态元素（模式由调用方复位）。静态骨架在预设、由每帧
-- draw 重写内容，reset 只收动态层（建造幽灵/招式图标/招式名登记）。
function M.reset()
  ui.clear("hud:build_ghost")
  ui.clear(AVATAR.id)
  -- 招式卡底在预设（set_visible 由 draw 管）；图标与名按**实际画过的张数**收，不写死
  -- 1..5：写死会让扩招之后的图标/名永远留在屏上。
  for i = 1, skill_slots_drawn do
    ui.clear("hud:skill_icon" .. i)
    ui_text.hide("hud", "hud:skill_name" .. i)
  end
  skill_slots_drawn = 0
  return true
end

return M
