-- tools/text_audit.lua：运行时文案审计（M30 / 阶段五 P2）。
--
-- 三件事，都是"可机器验收"的：
--   ① 语料（corpus）：把**所有可能被画到屏幕上**的字符串收集起来（数据表 + 运行时生成的
--      HUD/反馈/目标/引导/对话/任务/商店文案），逐条问引擎"字体缺字吗"；
--   ② 首包覆盖：首包只带 `char_collect` 收集到的字符子集（`assetc::package` 会写
--      `chars.txt` 并裁字体），所以"运行时文案 ⊆ 收集集合"是**发布正确性**契约，
--      少一个字符在正式包里就是豆腐块（本模块只返回字符集，比对交给验收样例）；
--   ③ 排版：遍历 `ui.ui_text` 的登记表，逐条按真实字宽量宽，检查
--      ① 缺字 ② 字号下限（`docs/42` §1：字形 11px 起可读）③ 是否超出容器可用宽度。
--
-- 为什么语料要从**运行时**取：手写一份文案清单一定会漂（改了文案忘了改清单）。
-- 这里能从公共 API 拿到的就拿（`hud.status_lines()`/`feedback_line()`/`goals_line()`、
-- `tutorial.prompt()`、`quest.rows()`…），拿不到的结构（对话节点正文、物品描述）按
-- 数据表字段递归收集——递归规则见 `M.corpus()`。
local M = {}

local ui_text = require("ui.ui_text")

-- 递归收集时认哪些字段名（其余字段一律不看，避免把 id/路径当成文案）。
local TEXT_FIELDS = {
  name = true,
  title = true,
  label = true,
  text = true,
  description = true,
  desc = true,
  prompt = true,
  hint = true,
  msg = true,
  message = true,
  reason = true,
  summary = true,
  note = true
}

-- 不参与收集的模块（引擎/工具模块，不是玩家可见文案）。
local SKIP_MODULES = {
  ["validate_data"] = true,
  ["data.registry_check"] = true,
  ["tools.text_audit"] = true,
  ["ui.ui_text"] = true,
  ["ui.text_fit"] = true
}

local function is_text(value)
  return type(value) == "string" and value ~= ""
end

local function collect_value(value, out, seen, depth)
  if depth > 6 or type(value) ~= "table" then
    return
  end
  if seen[value] then
    return
  end
  seen[value] = true
  for key, item in pairs(value) do
    if is_text(item) and TEXT_FIELDS[key] then
      out[#out + 1] = item
    elseif type(item) == "table" then
      -- 数组元素（选项列表、目标列表）与嵌套定义都递归；键名不参与判定。
      collect_value(item, out, seen, depth + 1)
    end
  end
end

--- 收集一条文案（内部用；空串/nil 直接忽略）。
local function add(out, value)
  if is_text(value) then
    out[#out + 1] = value
  end
  return out
end

--- 全部玩家可见文案。返回数组（可能有重复，去重交给字符集）。
function M.corpus()
  local out = {}
  local seen = {}

  -- ① 数据表：物品/工具/作物/建筑/配方/敌人/NPC/地图/地点/商店/任务/成就…
  for _, module_name in ipairs({
    "data.items",
    "data.tools",
    "data.crops",
    "data.buildings",
    "data.recipes",
    "data.enemies",
    "data.npcs",
    "data.maps",
    "data.places",
    "data.shops",
    "data.quests",
    "data.goals",
    "data.dialogues.index",
    "data.season_art",
    "data.appearances"
  }) do
    if not SKIP_MODULES[module_name] then
      local ok, module = pcall(require, module_name)
      if ok and type(module) == "table" then
        collect_value(module, out, seen, 0)
      end
    end
  end

  -- ② 对话节点正文与选项：`data/dialogues/index.lua` 只登记 id（`ids` 字段），
  --    正文由 `world.dialogue.get(id)` 取（引擎/脚本同一份缓存）。
  local ok_dialogues, dialogues = pcall(require, "data.dialogues.index")
  if ok_dialogues and type(dialogues) == "table" then
    local ok_api, api = pcall(require, "world.dialogue")
    if ok_api and type(api.get) == "function" then
      for _, dialogue_id in ipairs(dialogues.ids or dialogues) do
        local def = api.get(dialogue_id)
        if type(def) == "table" and type(def.nodes) == "table" then
          for _, node in pairs(def.nodes) do
            if type(node) == "table" then
              add(out, node.text)
              for _, option in ipairs(node.options or {}) do
                if type(option) == "table" then
                  add(out, option.text)
                end
              end
            end
          end
        end
      end
    end
  end

  -- ③ 字典式文案：反馈表、建造冲突原因、任务状态、物品类别、季节/天气…
  local ok_hud, hud = pcall(require, "ui.hud")
  if ok_hud and type(hud) == "table" then
    for _, field in ipairs({ "ACTION_TEXT", "BUILD_REASON", "WEATHER_TEXT", "SEASON_TEXT" }) do
      collect_value(hud[field], out, {}, 0)
    end
    if type(hud.status_lines) == "function" then
      for _, line in ipairs(hud.status_lines()) do
        add(out, line)
      end
    end
    if type(hud.feedback_line) == "function" then
      add(out, hud.feedback_line())
    end
    if type(hud.goals_line) == "function" then
      add(out, hud.goals_line())
    end
  end

  -- ③b 物品/工具名的**运行时**拼串（背包行、商店行、合成需求）：名称与价格一起出现时
  --     最长的是"金币 99999 文 / 限购剩余 N"这类行，这里按最坏数字拼一遍。
  local ok_items, items = pcall(require, "data.items")
  if ok_items and type(items) == "table" then
    for item_id, def in pairs(items) do
      if type(def) == "table" then
        add(out, def.name)
        add(out, def.description)
        add(out, string.format("%s x%d  [%s]", tostring(def.name or item_id), 999, "物品"))
        add(out, tostring(def.name or item_id) .. "  999999 文  限购剩余 999")
      end
    end
  end

  -- ④ last_action 全枚举：反馈文案是"按状态拼出来的"，只有逐个枚举才算真覆盖。
  local ok_state, state = pcall(require, "core.state")
  if ok_state and type(state) == "table" and type(state.set) == "function" then
    local ok_hud2, hud2 = pcall(require, "ui.hud")
    if ok_hud2 and type(hud2.feedback_line) == "function" then
      local actions = {}
      local ok_feed, feed = pcall(require, "systems.farm")
      if ok_feed and type(feed) == "table" then
        for key in pairs(feed.ENERGY_COST or {}) do
          actions[#actions + 1] = "farm_" .. key
        end
      end
      for _, module_name in ipairs({ "systems.combat", "systems.build", "systems.gather" }) do
        local ok, module = pcall(require, module_name)
        if ok and type(module) == "table" then
          for key, value in pairs(module) do
            if type(value) == "string" and key:find("ACTION") then
              actions[#actions + 1] = value
            end
          end
        end
      end
      -- 已知前缀族（与 `docs/18-lua-api.md` 的 `state.last_action` 约定一致）。
      for _, prefix in ipairs({
        "farm_till",
        "farm_water",
        "farm_plant",
        "farm_harvest",
        "combat_hit",
        "combat_kill",
        "combat_death",
        "build_ok",
        "build_fail",
        "gather_herb"
      }) do
        actions[#actions + 1] = prefix
        actions[#actions + 1] = prefix .. "_success"
        actions[#actions + 1] = prefix .. "_unknown_reason"
      end
      local saved = state.last_action
      for _, action in ipairs(actions) do
        state.set("last_action", action)
        add(out, hud2.feedback_line())
      end
      state.set("last_action", saved)
    end
  end

  -- ⑤ 新手引导十步文案（`data/tutorial.lua` 的 `prompt` 字段；HUD 只显示 prompt）。
  local ok_tutorial, tutorial = pcall(require, "ui.tutorial")
  if ok_tutorial and type(tutorial) == "table" and type(tutorial.steps) == "function" then
    for _, step in ipairs(tutorial.steps()) do
      if type(step) == "table" then
        add(out, step.prompt)
      end
    end
  end

  local ok_hints, hints = pcall(require, "ui.hints")
  if ok_hints and type(hints) == "table" then
    for _, field in ipairs({ "LABELS", "BUTTONS", "TEXT" }) do
      collect_value(hints[field], out, {}, 0)
    end
  end

  -- ⑥ 运行时行：任务日志行、商店行、背包行、目标行。
  local ok_quest, quest_ui = pcall(require, "ui.quest_ui")
  if ok_quest and type(quest_ui) == "table" and type(quest_ui.rows) == "function" then
    for _, row in ipairs(quest_ui.rows()) do
      if type(row) == "table" then
        add(out, row.name)
        add(out, row.giver_name)
        add(out, quest_ui.state_text and quest_ui.state_text(row.state) or nil)
      end
    end
  end

  local ok_goals, goals = pcall(require, "systems.goals")
  if ok_goals and type(goals) == "table" and type(goals.rows) == "function" then
    for _, row in ipairs(goals.rows()) do
      if type(row) == "table" then
        add(out, row.title)
        add(out, row.text)
        add(out, row.label)
      end
    end
  end

  return out
end

--- 语料用到的**字符集合**（去重、排序）；验收拿它与首包 `char_collect` 的集合比对。
function M.unique_chars()
  local set = {}
  for _, text in ipairs(M.corpus()) do
    for _, code in utf8.codes(text) do
      set[utf8.char(code)] = true
    end
  end
  local chars = {}
  for char in pairs(set) do
    if char ~= "\n" then
      chars[#chars + 1] = char
    end
  end
  table.sort(chars)
  return table.concat(chars)
end

--- 语料里字体缺的字符（返回 `{ texts = {...}, chars = "…" }`）。
function M.missing()
  local texts = {}
  local chars = {}
  for _, text in ipairs(M.corpus()) do
    local measured = ui.measure_text(text, 15.0)
    if measured.missing ~= "" then
      texts[#texts + 1] = text
      chars[measured.missing] = true
    end
  end
  local unique = {}
  for char in pairs(chars) do
    unique[#unique + 1] = char
  end
  table.sort(unique)
  return { texts = texts, chars = table.concat(unique) }
end

--- 当前画面上所有登记文字的排版审计（缺字 / 字号下限 / 溢出）。
function M.audit()
  return ui_text.audit()
end

--- 逐条明细（诊断用：`{ owner, id, text, size, budget, width, missing }`）。
function M.elements()
  local out = {}
  for owner, owned in pairs(ui_text.recorded()) do
    for id, rec in pairs(owned) do
      for line in tostring(rec.text):gmatch("[^\n]+") do
        local measured = ui.measure_text(line, rec.size)
        out[#out + 1] = {
          owner = owner,
          id = id,
          text = line,
          size = rec.size,
          budget = rec.budget,
          width = measured.width,
          missing = measured.missing
        }
      end
    end
  end
  table.sort(out, function(a, b)
    if a.owner ~= b.owner then
      return a.owner < b.owner
    end
    return a.id < b.id
  end)
  return out
end

--- 语料条数与字符数（验收打印用）。
function M.stats()
  local corpus = M.corpus()
  return {
    texts = #corpus,
    chars = utf8.len(M.unique_chars()),
    missing = #M.missing().texts
  }
end

return M
