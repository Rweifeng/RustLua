-- dialogue.lua：数据驱动对话运行时（M17-03）。
--
-- 数据定义见 data/dialogues/*.lua；新增一段对话只改数据，不改本文件与 Rust 核心
-- （验收项①）。节点/选项/条件/副作用与任务状态机（quest.lua）协作，形成
-- 「NPC 对话 → 接取任务 → 完成交付」的最小闭环。
--
-- 输入在 input.lua 的 Dialogue 分支里读取后调用本模块的 choose/close，
-- 本模块不 require input（否则与 input → dialogue 形成 require 环）。
--
-- 引用完整性由 validate_all 在加载期校验（硬风险①），运行时不做静默兜底。
local state = require("core.state")
local quest = require("world.quest")
local npcs = require("data.npcs")
local items = require("data.items")
local tools = require("data.tools")
local relationships_data = require("data.relationships")
-- M34-01：档位条件读关系分（只读；relationship 不 require dialogue，无环）。
local relationship = require("player.relationship")
-- M34-02：节气条件读"今天是不是某个节气"（festival 是 day 的纯映射，不 require dialogue，无环）。
local festival = require("world.festival")
local festivals_data = require("data.festivals")
local interaction = require("systems.interaction")
local player = require("player.player")
local npc = require("world.npc")
-- M19-02：对话打开/选择/取消音效（缺失文件或无设备时静默降级）；M20-02 走 UI 总线。
local audio = require("core.audio")
-- M20-02：选项焦点环（引擎 ui_navigation），方向键到边界环绕。
local nav = require("ui.panel_nav")
-- M20-09：键鼠/手柄两套操作说明（本模块不 require ui.input，避免 require 环；
-- 设备判定与绑定命名都在 ui.hints 里，面板自己画底部提示时才不会写死键盘文案）。
local hints = require("ui.hints")
-- M71：对话面板的**版面**（矩形、锚点、行距、字号、贴图键）搬进运行期预设
-- `assets/prefabs/dialogue.prefab`（对账门 = jhyt_m71_dialogue_prefab，黄金表 34 行
-- 钉死迁移前画面），本文件只喂内容取值（说话人、台词、选项、提示、立绘）。
-- 开/关屏 = 根节点显隐（`prefab.set_visible` 子树语义），不撤重铺（M63c 教训）。
-- 立绘本体留动态层 `ui.image`：贴图随 NPC/mood 运行期变化，引擎无运行期换贴图 API
-- （A6 裁定叠图互斥不适用，立绘是整图路径不是二选一）。
-- `prefab.*` 是引擎注入的全局运行期预设 API（spawn/set_visible/set_text），
-- 不再 require `ui.prefab`（JSON 预设层，M71 退役）。
-- docs/48 portraits 接线：对话立绘（speaker + 节点 mood → 立绘贴图）。
local portrait = require("ui.portrait")

local M = {}

local TILE = 32
local INTERACT_RANGE = 56 -- 世界单位，约 1.75 格
-- 选项**槽位上限**：与数字直选 1..5 同源，也必须与预设里 `options.max` 一致
-- （`jhyt_m71_dialogue_prefab` 逐节点对账守住这一条）。
local OPTION_LIMIT = 5

-- 对话面板的版面（矩形/锚点/行距/字号/贴图键）在 `assets/prefabs/dialogue.prefab`：
-- 调 UI 表现改那份数据，不改本文件（M71）。

local FOCUS = "dlg:focus"

-- 当前高亮选项（1 基；M19-02：W/S 上下选择；M20-02：方向键经焦点环选择；数字键仍可直接选择）。
-- 必须在焦点辅助函数之前声明，否则那些函数里的 `selected` 会退化成全局 nil。
local selected = 1

-- 焦点环与当前高亮同步（选项数量随节点/条件变化；空集合时清除焦点环）。
local function sync_focus()
  local count = #M.options()
  if count == 0 then
    nav.close(FOCUS)
    return
  end
  nav.open(FOCUS, count, 1)
  nav.select(FOCUS, selected)
end

-- 焦点环方向移动（`step` 项，正数向下）；引擎 API 不可用返回 nil。
local function ring_step(step)
  local direction = step > 0 and "down" or "up"
  local index = selected
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
  return index
end

local VALID_QUEST_STATE = {
  not_started = true,
  in_progress = true,
  deliverable = true,
  completed = true,
  failed = true
}
local VALID_COND = {
  quest = true, state = true, var = true,
  eq = true, gte = true, lte = true, lt = true, gt = true,
  truthy = true, falsy = true, item = true, count = true,
  -- M34-01：关系档位条件（成对出现）——tier_npc 是谁、tier_gte 要到 data.tiers 的
  -- 哪一档（陌生/相识/友善/亲厚/莫逆）。档位是派生值，这里只做只读查询。
  tier_npc = true, tier_gte = true,
  -- M34-02：节气条件——只有今天落在 data/festivals.lua 的某个窗口里才出现的分支
  -- （春分茶会才提茶叶单子，过了那三天这句话就收回去）。
  festival = true
}
local VALID_EFFECT = {
  accept_quest = true, deliver_quest = true, fail_quest = true,
  set_var = true, add_var = true, add_item = true, money = true, inner_power = true,
  relationship = true, reputation = true
}

-- 当前对话：{ id = 对话 id, node = 当前节点 id }；nil 表示未开启。
local active = nil

local defs = {}

local function definition(id)
  if type(id) ~= "string" then
    return nil
  end
  if defs[id] == nil then
    local ok, mod = pcall(require, "data.dialogues." .. id)
    defs[id] = (ok and type(mod) == "table") and mod or false
  end
  return defs[id] or nil
end

local function item_exists(id)
  return type(id) == "string" and (items[id] ~= nil or tools[id] ~= nil)
end

local function positive(value)
  local n = tonumber(value)
  return n ~= nil and n > 0
end

function M.get(id)
  return definition(id)
end

-- 注册表（data/dialogues/index.lua）顺序遍历。
function M.ids()
  local ok, list = pcall(require, "data.dialogues.index")
  if not ok or type(list) ~= "table" or type(list.ids) ~= "table" or list.version ~= 1 then
    return {}
  end
  local out = {}
  for _, id in ipairs(list.ids) do
    if type(id) == "string" then
      out[#out + 1] = id
    end
  end
  return out
end

-- NPC persist_id → 对话 id（首个匹配；重复由 validate_all 报告）。
local by_speaker = nil
function M.find_for_npc(persist_id)
  if type(persist_id) ~= "string" then
    return nil
  end
  if not by_speaker then
    by_speaker = {}
    for _, id in ipairs(M.ids()) do
      local def = definition(id)
      if def and type(def.speaker) == "string" and by_speaker[def.speaker] == nil then
        by_speaker[def.speaker] = id
      end
    end
  end
  return by_speaker[persist_id]
end

-- 选项显示条件：缺省即无条件；多字段为「与」关系。
local function cond_ok(cond)
  if cond == nil then
    return true
  end
  if type(cond) ~= "table" then
    return true
  end

  if cond.quest ~= nil then
    local have = quest.state(cond.quest)
    if cond.state ~= nil then
      if have ~= cond.state then
        return false
      end
    elseif have == nil then
      return false
    end
  end

  if cond.var ~= nil then
    local value = state.get_var(cond.var)
    if cond.truthy and (value == nil or value == false) then
      return false
    end
    if cond.falsy and value ~= nil and value ~= false then
      return false
    end
    local numeric = tonumber(value)
    if cond.eq ~= nil then
      if type(cond.eq) == "boolean" then
        if (value == true) ~= cond.eq then
          return false
        end
      elseif numeric ~= tonumber(cond.eq) then
        return false
      end
    end
    if cond.gte ~= nil and (numeric == nil or numeric < tonumber(cond.gte)) then
      return false
    end
    if cond.lte ~= nil and (numeric == nil or numeric > tonumber(cond.lte)) then
      return false
    end
    if cond.lt ~= nil and (numeric == nil or numeric >= tonumber(cond.lt)) then
      return false
    end
    if cond.gt ~= nil and (numeric == nil or numeric <= tonumber(cond.gt)) then
      return false
    end
  end

  if cond.item ~= nil and inv.count(state.inventory, cond.item) < (tonumber(cond.count) or 1) then
    return false
  end

  -- 关系档位条件（M34-01）：与某人的关系到档才出现的分支。
  if (cond.tier_npc ~= nil or cond.tier_gte ~= nil)
    and not relationship.affinity_met(cond.tier_npc, cond.tier_gte) then
    return false
  end

  -- 节气条件（M34-02）：今天落在该节气的窗口里；M34-05 起还过一道设施门
  -- （春分茶会要茶棚建到 3 级，门槛写在 data/festivals.lua 的 unlock 上）。
  -- 这里必须问 available() 而不是 id()：分成两处判就会各判一半。
  if cond.festival ~= nil and not festival.available(cond.festival) then
    return false
  end

  return true
end

-- M71：静态版式搬进 `assets/prefabs/dialogue.prefab`（对账门 = jhyt_m71_dialogue_prefab，
-- 黄金表 34 行钉死迁移前画面），render 只写会变的部分：说话人/台词/选项/提示文本、
-- 选中行 marker 互斥、推进箭头显隐；立绘本体留动态层（贴图随 NPC/mood 变化）。
-- 开/关屏 = 根节点显隐（`prefab.set_visible` 子树语义，藏父带子、藏节点不建布局项），
-- 不撤重铺，避免槽位 LIFO 复用打乱绘制序（M63c 教训）。
local PRESET_PATH = "assets/prefabs/dialogue.prefab"
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
  prefab.set_visible(preset_handle, "DlgRoot", true)
  return true
end

local function preset_hide()
  if preset_live() then
    prefab.set_visible(preset_handle, "DlgRoot", false)
  end
  return true
end

-- 验收（jhyt_m19_ui 对话断言）取 spawn 句柄；start 前为 nil。
function M.preset_ref()
  return preset_handle
end

-- 关面板的收口交给预设：藏根 = 整面板连同所有 id 一起收掉，不会再出现"漏清一条 =
-- 上一段对话的选项还挂在屏幕上"（引擎的 UI 记录是 retained 的，只有切场景才清空）。
-- 立绘是跨帧持久的 `ui.image`，也要显式收掉。
local function clear_ui()
  if preset_live() then
    prefab.set_visible(preset_handle, "DlgRoot", false)
  end
  ui.clear("dlg:portrait")
end

-- 当前节点可选项（已按 cond 过滤），保持数据顺序。
function M.options()
  local out = {}
  local def = active and definition(active.id)
  local node = def and def.nodes and def.nodes[active.node]
  if type(node) ~= "table" or type(node.options) ~= "table" then
    return out
  end
  for _, option in ipairs(node.options) do
    if type(option) == "table" and cond_ok(option.cond) then
      out[#out + 1] = { option = option, text = option.text }
    end
  end
  return out
end

function M.is_open()
  return active ~= nil
end

-- 可选项槽位数（与 UI、数字键 1..N 一致）。
function M.option_limit()
  return OPTION_LIMIT
end

-- 当前高亮选项（1 基）；M19-02：对话面板上下键选择。
function M.selected()
  return selected
end

function M.select(index)
  local i = math.floor(tonumber(index) or 0)
  if i < 1 or i > #M.options() then
    return false
  end
  selected = i
  sync_focus()
  return true
end

-- 相对移动高亮：焦点环可用时按方向逐行移动（到边界环绕，M20-02），
-- 否则退回既有的「不环绕、越界返回 false」语义（M19-02）。
function M.move(delta)
  local step = math.floor(tonumber(delta) or 0)
  if step == 0 or #M.options() == 0 then
    return false
  end
  sync_focus()
  local target = ring_step(step)
  if target == nil then
    target = math.max(1, math.min(#M.options(), selected + step))
  end
  if target == selected then
    return false
  end
  selected = target
  return true
end

-- 方向键移动（M20-02）：直接交给引擎焦点环（到边界环绕）。
function M.move_dir(direction)
  if #M.options() == 0 then
    return false
  end
  sync_focus()
  local index = nav.move(FOCUS, direction)
  if not index then
    return false
  end
  selected = index
  return true
end

function M.dialogue_id()
  return active and active.id or nil
end

function M.node()
  return active and active.node or nil
end

-- 只读快照（UI 与 headless 验收用）。
function M.info()
  if not active then
    return nil
  end
  local def = definition(active.id)
  local node = def and def.nodes and def.nodes[active.node]
  local speaker = def and npcs.get(def.speaker)
  local texts = {}
  for i, entry in ipairs(M.options()) do
    texts[i] = entry.text
  end
  return {
    dialogue = active.id,
    node = active.node,
    speaker = def and def.speaker or nil,
    speaker_name = speaker and speaker.name or nil,
    text = node and node.text or nil,
    options = texts
  }
end

-- M20-09：底部提示按当前设备二选一（键鼠给键位、手柄给按钮名）。片段从**当前绑定**
-- 推导（改键后自动同步）：数字直选用本模块自带的 1..OPTION_LIMIT，方向用 move_*。
-- 提示行的**宽度预算**现在写在预设里（`hint` 节点的 rect），不再由本文件猜。

local function digits_label()
  return hints.binding_labels({ "slot_1", "slot_2", "slot_3", "slot_4", "slot_5" }, nil, "/")
end

local function hint_line()
  -- 移动/选择：键盘给「W/S 或 方向键」，手柄给「左摇杆/DPad」。
  return hints.panel_line({
    { keyboard = "W/S", alt_keyboard = "方向键", gamepad = "左摇杆/DPad", label = "选择" },
    { keyboard = digits_label(), gamepad = digits_label(), label = "直选" },
    { keyboard = hints.binding_labels({ "confirm" }), gamepad = hints.binding_labels({ "confirm" }), label = "确认" },
    { keyboard = hints.binding_labels({ "cancel" }), gamepad = hints.binding_labels({ "cancel" }), label = "结束" },
  })
end

-- 选项行的显示文案：编号 + 选中箭头（与迁移前逐字符一致，黄金对账门按 id 比文本）。
local function option_label(index, entry)
  local mark = index == selected and "> " or "  "
  return mark .. tostring(index) .. ". " .. entry.text
end

-- 绘制对话面板（场景切换会清空动态 UI，故每次换节点/开始都整体重绘）。
-- 静态版式取自预设（`assets/prefabs/dialogue.prefab`），这里只喂内容取值：
-- 说话人 / 台词 / 选项 / 提示 / 立绘（立绘是动态层 `ui.image`，贴图随 NPC/mood 变化）。
function M.render()
  if not active then
    return false
  end
  local def = definition(active.id)
  local node = def and def.nodes and def.nodes[active.node]
  if type(node) ~= "table" then
    return false
  end
  local speaker = npcs.get(def.speaker)
  -- 选项数量随节点/条件变化：重绘时同步焦点环（M20-02）。
  sync_focus()
  local options = M.options()
  -- 静态版式在预设里，开屏/重绘只喂会变的部分。
  preset_show()
  local P = preset_handle
  prefab.set_text(P, "DlgSpeaker", speaker and speaker.name or "")
  prefab.set_text(P, "DlgBody", node.text or "")
  prefab.set_text(P, "DlgHint", hint_line())
  -- 选项行 + 选中箭头互斥（同一矩形叠 marker 图，选中行显、其余藏；行数不足的槽位
  -- 整行隐藏，选项 label 里的 `> ` 前缀与迁移前逐字符一致）。
  local last = 0
  for index = 1, OPTION_LIMIT do
    local opt_name = "DlgOpt" .. index
    local mark_name = "DlgMarker" .. index
    local entry = options[index]
    if entry then
      last = index
      prefab.set_text(P, opt_name, option_label(index, entry))
      prefab.set_visible(P, opt_name, true)
    else
      prefab.set_visible(P, opt_name, false)
    end
    prefab.set_visible(P, mark_name, entry ~= nil and index == selected)
  end
  -- 推进箭头：还有可选项才显（终节点无选项、结束收口）；A 态开场有 4 选项常显。
  prefab.set_visible(P, "DlgContinue", last > 0)
  -- 立绘：表情随节点 mood；无立绘的角色 `texture_for` 返回 nil，整帧收掉。
  -- `display_size()` 恒 96×96（预设的立绘矩形按它算，黄金表钉死 592,348,96×96），
  -- 照旧无条件取。`ui.image` 跨帧持久，无立绘时显式 `ui.clear` 收干净。
  local pw, ph = portrait.display_size()
  local tex = portrait.texture_for(def.speaker, node)
  if tex then
    ui.image("dlg:portrait", 592, 348, pw, ph, tex, 1.0, 1.0, 1.0)
  else
    ui.clear("dlg:portrait")
  end
  return true
end

function M.start(dialogue_id)
  local def = definition(dialogue_id)
  if not def or active then
    return false
  end
  if type(def.nodes) ~= "table" or type(def.nodes[def.start]) ~= "table" then
    return false
  end
  active = { id = dialogue_id, node = def.start }
  selected = 1
  state.set_mode("Dialogue")
  -- NPC 立绘：切 talk_<dir> 手势；close 时复位。
  npc.set_talking(def.speaker, true)
  audio.ui("ui_click")
  M.render()
  return true
end

-- silent = true 时不播放取消音（内部跳转/结束路径由 choose 播确认音）。
function M.close(silent)
  if not active then
    return false
  end
  local def = definition(active.id)
  active = nil
  clear_ui()
  nav.close(FOCUS)
  if def and def.speaker then
    npc.set_talking(def.speaker, false)
  end
  if state.is_mode("Dialogue") then
    state.set_mode("Playing")
  end
  if not silent then
    audio.ui("ui_cancel")
  end
  return true
end

-- 选择第 index 个可选项（缺省＝当前高亮项）：应用副作用 → 跳转 next 或结束。
function M.choose(index)
  if not active then
    return false, "not_open"
  end
  local target = index
  if target == nil then
    target = selected
  end
  local entry = M.options()[target]
  if not entry then
    return false, "invalid_option"
  end
  selected = target
  local option = entry.option
  quest.apply_effects(option.effects, "dialogue:" .. active.id, option.text)
  if option.finish then
    M.close(true)
    audio.ui("ui_confirm")
    return true
  end
  local def = definition(active.id)
  if type(option.next) ~= "string" or not def or type(def.nodes[option.next]) ~= "table" then
    M.close(true)
    audio.ui("ui_cancel")
    return false, "invalid_next"
  end
  active.node = option.next
  selected = 1
  audio.ui("ui_confirm")
  M.render()
  return true
end

-- 场景切换 / 新档：丢弃对话态并清掉面板（模式由调用方复位）。
function M.reset()
  active = nil
  selected = 1
  clear_ui()
  nav.close(FOCUS)
end

-- 与 NPC 对话的交互目标（interaction 框架），由 main.lua 在进入地图时注册一次。
local function player_world()
  local pos = player.position()
  if type(pos) ~= "table" then
    return nil
  end
  return tonumber(pos.x), tonumber(pos.y)
end

local registered = false
function M.ensure_target()
  if registered then
    return
  end
  registered = true
  interaction.register({
    id = "dialogue_npc",
    priority = 10,
    query = function()
      local map_id = map.current() or scene.reload()
      if not map_id then
        return nil
      end
      local px, py = player_world()
      if not px or not py then
        return nil
      end
      local best, best_distance = nil, nil
      for _, entry in ipairs(npcs.list()) do
        if M.find_for_npc(entry.persist_id) then
          local info = npc.query(entry.persist_id)
          if info and info.map == map_id and info.live and info.cell_x and info.cell_y then
            local nx = (info.cell_x + 0.5) * TILE
            local ny = (info.cell_y + 0.5) * TILE
            local dx, dy = nx - px, ny - py
            local distance = math.sqrt(dx * dx + dy * dy)
            if distance <= INTERACT_RANGE and (best_distance == nil or distance < best_distance) then
              best, best_distance = info, distance
            end
          end
        end
      end
      if not best then
        return nil
      end
      return { id = best.persist_id, npc = best, distance = best_distance, facing = 1 }
    end,
    prompt = function(candidate)
      local info = candidate.npc
      return "E: 与" .. tostring(info and info.name or "NPC") .. "交谈"
    end,
    interact = function(candidate)
      local dialogue_id = M.find_for_npc(candidate.id)
      if not dialogue_id or not M.start(dialogue_id) then
        return "no_dialogue"
      end
      return "dialogue"
    end
  })
end

-- 加载期校验辅助：选项条件（cond）白名单与引用完整性。
local function validate_cond(issues, where, cond)
  if cond == nil then
    return issues
  end
  if type(cond) ~= "table" then
    issues[#issues + 1] = where .. " 的 cond 必须是表"
    return issues
  end
  for key, value in pairs(cond) do
    if not VALID_COND[key] then
      issues[#issues + 1] = where .. " 的 cond 含非法字段：" .. tostring(key)
    end
  end
  if cond.quest ~= nil and (type(cond.quest) ~= "string" or not quest.get(cond.quest)) then
    issues[#issues + 1] = where .. " 的 cond.quest 引用未知任务：" .. tostring(cond.quest)
  end
  if cond.state ~= nil and not VALID_QUEST_STATE[cond.state] then
    issues[#issues + 1] = where .. " 的 cond.state 非法：" .. tostring(cond.state)
  end
  if cond.var ~= nil and (type(cond.var) ~= "string" or cond.var == "") then
    issues[#issues + 1] = where .. " 的 cond.var 必须是非空字符串"
  end
  if cond.item ~= nil and not item_exists(cond.item) then
    issues[#issues + 1] = where .. " 的 cond.item 引用未知物品：" .. tostring(cond.item)
  end
  if cond.count ~= nil and not positive(cond.count) then
    issues[#issues + 1] = where .. " 的 cond.count 必须为正数"
  end
  -- 关系档位条件：两个键必须成对出现，且档位名要在 data/relationships.lua 查得到。
  if cond.tier_npc ~= nil or cond.tier_gte ~= nil then
    if cond.tier_npc == nil or cond.tier_gte == nil then
      issues[#issues + 1] = where .. " 的 cond.tier_npc/cond.tier_gte 必须成对出现"
    end
    if type(cond.tier_npc) ~= "string" or not relationships_data.npcs[cond.tier_npc] then
      issues[#issues + 1] = where .. " 的 cond.tier_npc 引用未登记关系的 NPC：" .. tostring(cond.tier_npc)
    end
    if type(cond.tier_gte) ~= "string" or not relationships_data.tier_by_key(cond.tier_gte) then
      issues[#issues + 1] = where .. " 的 cond.tier_gte 引用未知关系档位：" .. tostring(cond.tier_gte)
        .. "（已知：" .. table.concat(relationships_data.tier_keys(), "/") .. "）"
    end
  end
  for _, key in ipairs({ "eq", "gte", "lte", "lt", "gt" }) do
    local value = cond[key]
    if value ~= nil and type(value) ~= "boolean" and not tonumber(value) then
      issues[#issues + 1] = where .. " 的 cond." .. key .. " 必须是数值"
    end
  end
  -- 节气条件：写的 id 必须是 data/festivals.lua 里真有的那七个之一。
  -- 打错字的分支永远不出现，而对话本身照常跑——这是最安静的一类内容失效。
  if cond.festival ~= nil then
    if type(cond.festival) ~= "string" or not festivals_data.get(cond.festival) then
      issues[#issues + 1] = where .. " 的 cond.festival 引用未知节气：" .. tostring(cond.festival)
        .. "（已知：" .. table.concat(festivals_data.id_keys(), "/") .. "）"
    end
  end
  return issues
end

-- 加载期校验辅助：选项副作用（effects）白名单与引用完整性。
local function validate_effects(issues, where, effects)
  if effects == nil then
    return issues
  end
  if type(effects) ~= "table" then
    issues[#issues + 1] = where .. " 的 effects 必须是表"
    return issues
  end
  for key in pairs(effects) do
    if not VALID_EFFECT[key] then
      issues[#issues + 1] = where .. " 的 effects 含非法字段：" .. tostring(key)
    end
  end
  for _, key in ipairs({ "accept_quest", "deliver_quest", "fail_quest" }) do
    local value = effects[key]
    if value ~= nil and (type(value) ~= "string" or not quest.get(value)) then
      issues[#issues + 1] = where .. " 的 effects." .. key .. " 引用未知任务：" .. tostring(value)
    end
  end
  for _, key in ipairs({ "set_var", "add_var" }) do
    local value = effects[key]
    if value ~= nil then
      if type(value) ~= "table" then
        issues[#issues + 1] = where .. " 的 effects." .. key .. " 必须是表"
      else
        for name in pairs(value) do
          if type(name) ~= "string" or name == "" then
            issues[#issues + 1] = where .. " 的 effects." .. key .. " 含非法变量名"
          end
        end
      end
    end
  end
  if effects.add_item ~= nil then
    if type(effects.add_item) ~= "table" then
      issues[#issues + 1] = where .. " 的 effects.add_item 必须是表"
    else
      for _, entry in ipairs(effects.add_item) do
        if type(entry) ~= "table" or not item_exists(entry.item) then
          issues[#issues + 1] = where .. " 的 effects.add_item 引用未知物品：" .. tostring(entry and entry.item)
        elseif not positive(entry.count) then
          issues[#issues + 1] = where .. " 的 effects.add_item.count 必须为正数"
        end
      end
    end
  end
  for _, key in ipairs({ "money", "inner_power" }) do
    local value = effects[key]
    if value ~= nil and not tonumber(value) then
      issues[#issues + 1] = where .. " 的 effects." .. key .. " 必须是数值"
    end
  end
  -- 关系/声望副作用（M18-04）：{ npc_id = delta } / { faction = delta }，
  -- 键必须为已知 npc/faction，值必须为数值。
  for _, key in ipairs({ "relationship", "reputation" }) do
    local value = effects[key]
    if value ~= nil then
      if type(value) ~= "table" then
        issues[#issues + 1] = where .. " 的 effects." .. key .. " 必须是表"
      else
        for id, delta in pairs(value) do
          local known = key == "relationship"
            and (type(id) == "string" and relationships_data.npcs[id] ~= nil)
            or (type(id) == "string" and relationships_data.faction(id) ~= nil)
          if not known then
            issues[#issues + 1] = where .. " 的 effects." .. key .. " 引用未知对象：" .. tostring(id)
          end
          if not tonumber(delta) then
            issues[#issues + 1] = where .. " 的 effects." .. key .. "." .. tostring(id) .. " 必须是数值"
          end
        end
      end
    end
  end
  return issues
end

-- 加载期校验：返回 ok, issues（issues 为字符串列表，空表即通过）。
function M.validate_all()
  local issues = {}
  local seen = {}
  local ok, index = pcall(require, "data.dialogues.index")
  if not ok or type(index) ~= "table" then
    issues[#issues + 1] = "data/dialogues/index.lua: unable to load registry"
    return false, issues
  end
  if index.version ~= 1 then
    issues[#issues + 1] = "data/dialogues/index.lua:version: unsupported version " .. tostring(index.version)
  end
  if type(index.ids) ~= "table" then
    issues[#issues + 1] = "data/dialogues/index.lua:ids: expected array"
    return false, issues
  end
  for position, value in ipairs(index.ids) do
    if type(value) ~= "string" or value == "" then
      issues[#issues + 1] = "data/dialogues/index.lua:ids[" .. tostring(position) .. "]: required non-empty string"
    end
  end
  for _, id in ipairs(M.ids()) do
    if seen[id] then
      issues[#issues + 1] = "对话注册表重复 id：" .. id
    end
    seen[id] = true

    local def = definition(id)
    if not def then
      issues[#issues + 1] = "对话定义缺失或加载失败：data.dialogues." .. id
    else
      if def.version ~= 1 then
        issues[#issues + 1] = "data/dialogues/" .. id .. ".lua:version: unsupported version " .. tostring(def.version)
      end
      if def.id ~= id then
        issues[#issues + 1] = "对话 " .. id .. " 的 id 字段不匹配：" .. tostring(def.id)
      end
      if type(def.speaker) ~= "string" or not npcs.get(def.speaker) then
        issues[#issues + 1] = "对话 " .. id .. " 的 speaker 引用未知 NPC：" .. tostring(def.speaker)
      end
      if type(def.nodes) ~= "table" then
        issues[#issues + 1] = "对话 " .. id .. " 缺少 nodes"
      else
        if type(def.start) ~= "string" or type(def.nodes[def.start]) ~= "table" then
          issues[#issues + 1] = "对话 " .. id .. " 的 start 节点不存在：" .. tostring(def.start)
        end
        local count = 0
        for node_id, node in pairs(def.nodes) do
          count = count + 1
          local where = "对话 " .. id .. " 节点 " .. tostring(node_id)
          if type(node) ~= "table" then
            issues[#issues + 1] = where .. " 必须是表"
          else
            if type(node.text) ~= "string" or node.text == "" then
              issues[#issues + 1] = where .. " 缺少 text"
            end
            if type(node.options) ~= "table" or #node.options == 0 then
              issues[#issues + 1] = where .. " 缺少 options"
            else
              for i, option in ipairs(node.options) do
                local ow = where .. " 选项 " .. tostring(i)
                if type(option) ~= "table" then
                  issues[#issues + 1] = ow .. " 必须是表"
                else
                  if type(option.text) ~= "string" or option.text == "" then
                    issues[#issues + 1] = ow .. " 缺少 text"
                  end
                  if option.next == nil and not option.finish then
                    issues[#issues + 1] = ow .. " 必须指定 next 或 finish"
                  elseif option.next ~= nil and (type(option.next) ~= "string" or type(def.nodes[option.next]) ~= "table") then
                    issues[#issues + 1] = ow .. " 的 next 节点不存在：" .. tostring(option.next)
                  end
                  issues = validate_cond(issues, ow, option.cond)
                  issues = validate_effects(issues, ow, option.effects)
                end
              end
            end
          end
        end
        if count == 0 then
          issues[#issues + 1] = "对话 " .. id .. " 的 nodes 为空"
        end
      end
    end
  end

  -- 交叉校验：每条任务的发布者至少要有一段对话，否则任务无法在游戏内接取。
  for _, quest_id in ipairs(quest.ids()) do
    local qdef = quest.get(quest_id)
    if qdef and type(qdef.giver) == "string" then
      local found = false
      for _, id in ipairs(M.ids()) do
        local ddef = definition(id)
        if ddef and ddef.speaker == qdef.giver then
          found = true
          break
        end
      end
      if not found then
        issues[#issues + 1] = "任务 " .. quest_id .. " 的 giver 没有对应对话：" .. qdef.giver
      end
    end
  end

  return #issues == 0, issues
end

return M
