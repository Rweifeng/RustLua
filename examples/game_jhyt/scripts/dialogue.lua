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
local state = require("state")
local quest = require("quest")
local npcs = require("data.npcs")
local items = require("data.items")
local tools = require("data.tools")
local interaction = require("interaction")
local player = require("player")
local npc = require("npc")

local M = {}

local TILE = 32
local INTERACT_RANGE = 56 -- 世界单位，约 1.75 格
local OPTION_LIMIT = 5   -- 与 UI 展示槽位、数字键 1..5 一致

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
  truthy = true, falsy = true, item = true, count = true
}
local VALID_EFFECT = {
  accept_quest = true, deliver_quest = true, fail_quest = true,
  set_var = true, add_var = true, add_item = true, money = true, inner_power = true
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

  return true
end

local function clear_ui()
  ui.clear("dlg:box")
  ui.hide("dlg:speaker")
  ui.hide("dlg:text")
  ui.hide("dlg:hint")
  for i = 1, OPTION_LIMIT do
    ui.hide("dlg:opt" .. i)
  end
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

-- 绘制对话面板（场景切换会清空动态 UI，故每次换节点/开始都整体重绘）。
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
  local x, y, w, h = 140, 440, 1000, 248
  ui.panel("dlg:box", x, y, w, h, 0.10, 0.11, 0.15, 0.92)
  ui.text_at("dlg:speaker", x + 24, y + 16, speaker and speaker.name or "", 1.0, 20.0)
  ui.text_at("dlg:text", x + 24, y + 50, node.text, 1.0, 18.0)
  local options = M.options()
  for i = 1, OPTION_LIMIT do
    local entry = options[i]
    if entry then
      ui.text_at("dlg:opt" .. i, x + 24, y + 96 + (i - 1) * 30, tostring(i) .. ". " .. entry.text, 1.0, 18.0)
    else
      ui.hide("dlg:opt" .. i)
    end
  end
  ui.text_at("dlg:hint", x + w - 230, y + h - 28, "数字键选择 · Esc 结束", 1.0, 14.0)
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
  state.set_mode("Dialogue")
  M.render()
  return true
end

function M.close()
  if not active then
    return false
  end
  active = nil
  clear_ui()
  if state.is_mode("Dialogue") then
    state.set_mode("Playing")
  end
  return true
end

-- 选择第 index 个可选项：应用副作用 → 跳转 next 或结束。
function M.choose(index)
  if not active then
    return false, "not_open"
  end
  local entry = M.options()[index]
  if not entry then
    return false, "invalid_option"
  end
  local option = entry.option
  quest.apply_effects(option.effects, "dialogue:" .. active.id, option.text)
  if option.finish then
    M.close()
    return true
  end
  local def = definition(active.id)
  if type(option.next) ~= "string" or not def or type(def.nodes[option.next]) ~= "table" then
    M.close()
    return false, "invalid_next"
  end
  active.node = option.next
  M.render()
  return true
end

-- 场景切换 / 新档：丢弃对话态并清掉面板（模式由调用方复位）。
function M.reset()
  active = nil
  clear_ui()
end

-- 与 NPC 对话的交互目标（interaction 框架），由 main 在进入地图时注册一次。
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
  for _, key in ipairs({ "eq", "gte", "lte", "lt", "gt" }) do
    local value = cond[key]
    if value ~= nil and type(value) ~= "boolean" and not tonumber(value) then
      issues[#issues + 1] = where .. " 的 cond." .. key .. " 必须是数值"
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
