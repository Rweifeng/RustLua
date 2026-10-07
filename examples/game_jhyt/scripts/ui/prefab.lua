-- prefab.lua：UI 预设（prefab）的加载与实例化（M59，对标 Unity 的 Prefab 工作流）。
--
-- 里程碑号为什么跳 M59：`docs/54` 的路线图把 **M41–M58** 预留给阶段 2–5 的内容里程碑
-- （畜牧/钓鱼/矿脉…到 Beta 首发），UI 预设化不在那张表里；这里取 M59 起，避免与它抢号。
--
-- 为什么要有这一层：改造前每个界面的**版面**与**内容**混在同一段 Lua 里
-- （`world/dialogue.lua` 的 `M.render` 一边算 `y + OPTION_DY + (i-1) * OPTION_ROW_H`
-- 一边拼 `"> " .. text`），于是"调 UI 表现"必然要动玩法代码：
--   ① 改一个行距要先读懂整段绘制流程，改错了只以"某行压住下一行"这种没有报错的形态坏；
--   ② 同一族控件（面板框 + 标题 + 角标 + 关闭 + 列表行）在四个面板里各写一遍，
--      改一处样式要改四处；
--   ③ 版面没有单一可信来源：`ui/layout.lua` 只收得了**矩形**，收不了行距、字号、
--      贴图键、图标尺寸，那些散在各文件顶部的 `local ROW_H = 40` 里
--      （`inventory_ui.lua:70-74`、`shop.lua:52`、`quest_ui.lua:55-56`…）。
--
-- 预设把**版面**变成数据文件（`assets/ui/prefabs/<name>.ui.json`）：一棵节点树，每个节点
-- 说清自己的类型、相对父节点的锚点/偏移、样式与内容槽位。Lua 只喂内容：
--     prefab.render("dialogue", {
--       speaker = "顾长风", body = 台词, hint = 提示串,
--       portrait_tex = 立绘路径, portrait_w = 96, portrait_h = 96,
--       options = { { text = "> 1. …", selected = true }, … },
--     })
--
-- 为什么载体是 JSON 而不是 Lua 表模块（三条都是可查的宿主事实）：
--   * `data.load` 是**已有的**运行期数据通道（VFS 读 → `pack::decode` → serde_json → Lua 表，
--     带 schema 校验与缓存），三个宿主同源，不需要新引擎 API；
--   * `json` 在微信代码包后缀白名单内**且不会被编译**（`scripts/code-package-rules.js` 的
--     `EXTENSION_WHITELIST` 与 `isCompiledName`：被编译的只有 `*.js` 与文件名恰为 `game.json`），
--     而 `*.lua` 在白名单外、要改名成 `.lua.txt` 才传得上去——数据文件用 .json 反而少一层特例；
--   * 编辑器与外部工具能直接改 JSON，这是"制作和调整 UI 表现"的正路。
--   命名成 `<name>.ui.json`：`assetc::is_dev_artifact` 只把**文件名恰为** `manifest.json` 的
--     当开发产物裁掉，带名字的 `.ui.json` 照常进发行包。
--
-- 几何口径与引擎同源，不另造一套：`layout` 用的就是引擎 `UiRectLayout`
-- （`rustlua-ui/src/ui_layout.rs`）的 `anchors/offsets/min_size/max_size/pivot`，解析式也是
-- 同一条 `edge = parent_size × anchor + offset`，锚点预设名沿用 `UiLayoutPreset::ALL` 的
-- 小写下划线写法。同一份版面数据因此能被这里和编辑器/场景工具按同一语义读，
-- 不会"两边各解释一遍"。
--
-- 单位 = 设计像素（1280×720），且**不缩放**：`layout.scale()`/`font()` 目前没有任何界面在调
-- （全仓只有 `ui/layout.lua` 里的定义处），预设解析出的矩形就是屏幕像素。黄金对账门
-- `jhyt_m59_ui_prefab` 按"逐 id 矩形与迁移前一致"守这条；将来真要缩放，改一处
-- （本文件的画布矩形来源）而不是改八个界面。
local layout = require("ui.layout")
local skin = require("ui.skin")
local ui_text = require("ui.ui_text")

local M = {}

M.DIR = "assets/ui/prefabs/"
M.VERSION = 1

-- 工程内的预设清单。**必须显式登记**（与玩法用例进 `verify.ps1` 同一套纪律）：
-- 启动门 `validate_all()` 遍历这张表逐个 load+校验，漏登记的预设就是一份"从没被任何
-- 检查读过的数据文件"——改坏了要到玩家点开那个面板才炸。
-- M71：JSON 预设层**整体退役**——唯一登记 `dialogue`（dialogue.ui.json）已删除，
-- 对话面板改走运行期 `.prefab`（assets/prefabs/dialogue.prefab，对账门
-- jhyt_m71_dialogue_prefab）；本表清空，audit 遍历空表恒绿。
M.NAMES = {}

-- 锚点预设表（与引擎 `UiLayoutPreset::anchors()` 逐值一致）。漂移的表现是"编辑器画一套、
-- 运行时画另一套"，所以这张表由 `jhyt_m59_ui_prefab` 与引擎源码逐值对账。
M.PRESETS = {
  top_left = { 0.0, 0.0, 0.0, 0.0 },
  top_center = { 0.5, 0.0, 0.5, 0.0 },
  top_right = { 1.0, 0.0, 1.0, 0.0 },
  center_left = { 0.0, 0.5, 0.0, 0.5 },
  center = { 0.5, 0.5, 0.5, 0.5 },
  center_right = { 1.0, 0.5, 1.0, 0.5 },
  bottom_left = { 0.0, 1.0, 0.0, 1.0 },
  bottom_center = { 0.5, 1.0, 0.5, 1.0 },
  bottom_right = { 1.0, 1.0, 1.0, 1.0 },
  stretch_horizontal = { 0.0, 0.0, 1.0, 0.0 },
  stretch_vertical = { 0.0, 0.0, 0.0, 1.0 },
  full_rect = { 0.0, 0.0, 1.0, 1.0 },
}

-- 节点类型闭集：每个都必须是**现有引擎能力**，预设层不开新的绘制原语。
M.TYPES = {
  rect = true, -- 纯布局容器（不产生绘制记录，只给孩子当坐标系）
  panel = true, -- ui.panel 矩形
  circle = true, -- ui.circle 圆盘（触屏控件底衬）
  bar = true, -- ui.bar 比例条
  image = true, -- ui.image 贴图（skin 键或运行期路径，9-slice 取自 skin 注册表）
  text = true, -- ui_text.draw*（必须经这层，M30 的溢出登记才不会漏）
  list = true, -- 模板行 × 动态条数（Unity 的"预制体套预制体"）
  instance = true, -- 引用另一个预设（共用面板框 / 行样式）
}

-- 有绘制记录的节点类型（`rect`/`list` 只是容器，不占 id）。
local DRAWN = { panel = true, circle = true, bar = true, image = true, text = true }

local cache = {} -- 预设名 → 已校验的树（只读，运行期不改写它）
local last_render = {} -- 预设名 → { owner = , prefix = }

-- ---------- 结构校验 ----------
--
-- 引擎的 `data.schema` 只支持 array/object/map 与基础标量（`rustlua-ecs/src/data_schema.rs`），
-- 表达不了"节点树 + 递归 children + type 决定哪些字段必填"。所以外壳交给 schema，
-- 树内逐节点校验在这里做——报错必须能指到**节点路径**，那才是调 UI 时有用的信息。
local SCHEMA = {
  prefab_version = "integer",
  name = "string",
  owner = "string",
  layout_source = "string",
  root = "any",
}

local function node_path(trail)
  return table.concat(trail, "/")
end

local function check_spec(spec, where, field, issues)
  if spec == nil then
    issues[#issues + 1] = where .. "." .. field .. " 缺失"
  elseif type(spec) == "number" then
    return
  elseif type(spec) == "table" then
    if spec.slot ~= nil and type(spec.slot) ~= "string" then
      issues[#issues + 1] = where .. "." .. field .. ".slot 必须是字符串"
    end
    if spec.slot == nil and spec.value == nil then
      issues[#issues + 1] = where .. "." .. field .. " 必须是数值或 {slot,scale,add} / {value} 表"
    end
    for _, key in ipairs({ "scale", "add", "value" }) do
      if spec[key] ~= nil and type(spec[key]) ~= "number" then
        issues[#issues + 1] = where .. "." .. field .. "." .. key .. " 必须是数值"
      end
    end
  else
    issues[#issues + 1] = where .. "." .. field .. " 必须是数值或 {slot,scale,add} 表，实为 " .. type(spec)
  end
end

local function check_layout(node, where, issues)
  local box = node.layout
  if type(box) ~= "table" then
    issues[#issues + 1] = where .. ".layout 必须是表"
    return
  end
  if box.preset and not M.PRESETS[box.preset] then
    issues[#issues + 1] = where .. ".layout.preset 不是引擎锚点预设名：" .. tostring(box.preset)
  end
  if box.anchors then
    if type(box.anchors) ~= "table" or #box.anchors ~= 4 then
      issues[#issues + 1] = where .. ".layout.anchors 必须是 4 个分数 [l,t,r,b]"
    else
      for _, edge in ipairs(box.anchors) do
        if type(edge) ~= "number" or edge < 0 or edge > 1 then
          issues[#issues + 1] = where .. ".layout.anchors 每一项必须是 0..1 的分数"
        end
      end
    end
  elseif not box.preset then
    issues[#issues + 1] = where .. ".layout 必须给 preset 或 anchors 之一"
  end
  if type(box.offsets) ~= "table" then
    issues[#issues + 1] = where .. ".layout.offsets 必须是 {left,top,right,bottom}"
    return
  end
  for _, key in ipairs({ "left", "top", "right", "bottom" }) do
    check_spec(box.offsets[key], where, "layout.offsets." .. key, issues)
  end
end

-- 列表子树里的 id 必须含 `$i`：不带的话 N 行全写进同一条记录，画面只剩最后一行。
local function check_list_ids(node, where, issues)
  if type(node) ~= "table" then
    return
  end
  if type(node.id) == "string" and not node.id:find("%$i") then
    issues[#issues + 1] = where .. ".id 必须含 $i（列表里的每一行要写进自己的记录）：" .. node.id
  end
  if type(node.children) == "table" then
    for _, child in ipairs(node.children) do
      check_list_ids(child, where .. "/" .. tostring(child.name or "child"), issues)
    end
  end
end

local check_node

check_node = function(node, trail, seen, issues, depth, inside_list)
  local where = node_path(trail)
  if type(node) ~= "table" then
    issues[#issues + 1] = where .. " 必须是表"
    return
  end
  if depth > 8 then
    issues[#issues + 1] = where .. " 节点树深度超过 8：预设里出现了环或过度嵌套"
    return
  end
  if type(node.type) ~= "string" or not M.TYPES[node.type] then
    issues[#issues + 1] = where .. ".type 必须是 rect|panel|circle|bar|image|text|list|instance，实为 "
      .. tostring(node.type)
    return
  end
  check_layout(node, where, issues)

  if DRAWN[node.type] then
    if type(node.id) ~= "string" or node.id == "" then
      issues[#issues + 1] = where .. ".id 缺失（" .. node.type .. " 节点必须给引擎记录 id）"
    elseif inside_list and not node.id:find("%$i") then
      issues[#issues + 1] = where .. ".id 必须含 $i（列表里的每一行要写进自己的记录）：" .. node.id
    else
      -- 同一棵树的静态 id 不许撞车：`UiState` 按 id 覆盖写入，撞车的表现是"两块面板只画出来
      -- 一块"，而且两边都不报错。带 `$i` 的模板行按展开前的形态登记，避免互相误判。
      local key = node.id:gsub("%$i", "*")
      if seen[key] then
        issues[#issues + 1] = where .. ".id 与另一节点重复：" .. node.id
      end
      seen[key] = true
    end
  end

  if node.type == "panel" or node.type == "circle" or node.type == "bar" then
    if node.color and (type(node.color) ~= "table" or #node.color < 3 or #node.color > 4) then
      issues[#issues + 1] = where .. ".color 必须是 [r,g,b] 或 [r,g,b,a]"
    end
  end
  if node.type == "circle" and node.radius ~= nil then
    check_spec(node.radius, where, "radius", issues)
  end
  if node.type == "bar" then
    check_spec(node.ratio, where, "ratio", issues)
  end
  if node.type == "image" then
    if not node.texture and not node.path_slot then
      issues[#issues + 1] = where .. " image 节点必须给 texture（ui/skin 键）或 path_slot（运行期贴图路径槽位）"
    end
    -- `skin.icon` 对未知键是**静默 no-op**（它只能什么都不画），所以拼错一个字母的表现是
    -- "屏上少一张图、日志一行没有"。这一条把它挡在加载期。
    if node.texture and not skin.tex[node.texture] then
      issues[#issues + 1] = where .. ".texture 未在 ui/skin.lua 注册：" .. tostring(node.texture)
    end
  end
  if node.type == "text" then
    if type(node.font) ~= "number" then
      issues[#issues + 1] = where .. ".font 必须是数值（像素字号）"
    elseif node.font < 11 then
      -- 与 `ui_text.audit()` ② 同一条可读性下限（docs/42 §1）。在这里拦是因为报错能指到节点，
      -- 而不是等渲染完才量出一串"某条文字超宽"。
      issues[#issues + 1] = where .. ".font 低于 11px 可读性下限：" .. tostring(node.font)
    end
    if node.mode and node.mode ~= "clip" and node.mode ~= "exact" and node.mode ~= "wrap" then
      issues[#issues + 1] = where .. ".mode 必须是 clip|exact|wrap，实为 " .. tostring(node.mode)
    end
    if node.mode == "wrap" and type(node.lines) ~= "number" then
      issues[#issues + 1] = where .. ".mode=wrap 必须给 lines（最多行数）"
    end
    if not node.slot and type(node.text) ~= "string" then
      issues[#issues + 1] = where .. " 文字节点要么给 slot（运行期内容），要么给 text（静态文案）"
    end
  end
  if node.type == "list" then
    if type(node.slot) ~= "string" then
      issues[#issues + 1] = where .. ".slot 必须是条目数组的槽位名"
    end
    if type(node.max) ~= "number" or node.max <= 0 then
      issues[#issues + 1] = where .. ".max 必须是正数（槽位上限）"
    end
    if type(node.spacing) ~= "number" then
      issues[#issues + 1] = where .. ".spacing 必须是数值（行距，沿 axis 递进）"
    end
    if node.axis and node.axis ~= "x" and node.axis ~= "y" then
      issues[#issues + 1] = where .. ".axis 必须是 x|y，实为 " .. tostring(node.axis)
    end
    if type(node.item) ~= "table" then
      issues[#issues + 1] = where .. ".item 必须是模板节点"
    else
      -- 模板单独一套 seen：两行确实会用两个 id。
      check_node(node.item, { where .. "/item" }, {}, issues, depth + 1, true)
      check_list_ids(node.item, where .. "/item", issues)
    end
  end
  if node.type == "instance" then
    if type(node.prefab) ~= "string" or node.prefab == "" then
      issues[#issues + 1] = where .. ".prefab 必须是被引用预设的名字"
    end
    if node.bind and type(node.bind) ~= "table" then
      issues[#issues + 1] = where .. ".bind 必须是 { 子槽位 = 父槽位 } 表"
    elseif type(node.bind) == "table" then
      for child_slot, parent_slot in pairs(node.bind) do
        if type(child_slot) ~= "string" or type(parent_slot) ~= "string" then
          issues[#issues + 1] = where .. ".bind 的键与值都必须是字符串"
        end
      end
    end
  end

  if type(node.children) == "table" then
    for index, child in ipairs(node.children) do
      local child_name = type(child) == "table" and child.name or nil
      check_node(child,
        { trail[#trail] or "node", child_name or ("children[" .. index .. "]") },
        seen, issues, depth + 1, inside_list)
    end
  end
end

--- 校验一棵预设树；返回 (ok, issues)。
function M.validate(name, tree)
  local issues = {}
  if type(tree) ~= "table" then
    return false, { name .. "：预设根必须是表" }
  end
  if tonumber(tree.prefab_version) ~= M.VERSION then
    issues[#issues + 1] = name .. "：prefab_version 应为 " .. M.VERSION .. "，实为 "
      .. tostring(tree.prefab_version)
  end
  if tree.name ~= name then
    -- 名字与文件名分叉（改命名时只改了一边）的表现：`data.load` 按路径缓存，
    -- 于是"加载 dialogue 拿到自称 panel_frame 的树"。这条最难查，所以必守。
    issues[#issues + 1] = name .. "：预设内 name 字段必须与文件名一致，实为 " .. tostring(tree.name)
  end
  if type(tree.root) ~= "table" then
    issues[#issues + 1] = name .. "：root 必须是节点表"
    return false, issues
  end
  check_node(tree.root, { tree.root.name or "root" }, {}, issues, 1, false)
  return #issues == 0, issues
end

-- ---------- 加载 ----------

--- 加载（并缓存）一个预设；结构非法直接抛错，错误里带节点路径。
function M.load(name)
  if cache[name] then
    return cache[name]
  end
  local file = M.DIR .. name .. ".ui.json"
  if type(data) ~= "table" or type(data.load) ~= "function" then
    error("ui.prefab: 宿主没有 data.load，无法加载预设 " .. file, 0)
  end
  if type(data.schema) == "function" then
    data.schema(file, SCHEMA)
  end
  local ok, tree = pcall(data.load, file)
  if not ok then
    error("ui.prefab: 加载 " .. file .. " 失败：" .. tostring(tree), 0)
  end
  if type(tree) ~= "table" then
    error("ui.prefab: " .. file .. " 解析结果不是表", 0)
  end
  local valid, issues = M.validate(name, tree)
  if not valid then
    local lines = {}
    for _, issue in ipairs(issues) do
      lines[#lines + 1] = "  - " .. issue
    end
    error("ui.prefab: " .. file .. " 结构非法：\n" .. table.concat(lines, "\n"), 0)
  end
  cache[name] = tree
  return tree
end

--- 重新读盘：改完 JSON 不必重启（开发期调 UI 的通道）。
function M.reload(name)
  if name then
    cache[name] = nil
    last_render[name] = nil
    local file = M.DIR .. name .. ".ui.json"
    if type(data) == "table" and type(data.reload) == "function" then
      pcall(data.reload, file)
    end
    return M.load(name)
  end
  for _, key in ipairs(M.NAMES) do
    M.reload(key)
  end
  return true
end

-- ---------- 几何解析 ----------

-- 数值规格：常量 | {slot=名, scale=k, add=d} | {value=v}。
-- 动态尺寸靠它撑住：对话立绘的实际显示尺寸来自 `portrait.display_size()`，
-- 预设里写 `{"slot":"portrait_h","scale":-1,"add":-6}`（立绘底边贴面板上沿、留 6px 缝）。
local function number_spec(spec, values, where)
  if type(spec) == "number" then
    return spec
  end
  if type(spec) == "table" then
    if spec.value ~= nil then
      local base = tonumber(spec.value)
      if not base then
        error("ui.prefab: " .. where .. ".value 不是数值", 0)
      end
      return base
    end
    local raw = spec.slot and values[spec.slot]
    if raw == nil then
      -- 静默当 0 的表现是"控件叠在父节点原点上"，正是要消掉的那类零报错坏形态。
      error("ui.prefab: " .. where .. ".slot 引用了没有取值的槽位：" .. tostring(spec.slot), 0)
    end
    local source = tonumber(raw)
    if not source then
      error("ui.prefab: " .. where .. ".slot=" .. tostring(spec.slot) .. " 不是数值", 0)
    end
    return source * (tonumber(spec.scale) or 1) + (tonumber(spec.add) or 0)
  end
  error("ui.prefab: " .. where .. " 必须是数值或 {slot,value} 表，实为 " .. type(spec), 0)
end

-- 节点矩形：`edge = parent.w|h × anchor + offset`，再加父原点。
-- 式子与引擎 `UiRectLayout::resolve` 同一条；引擎把结果留在父空间，这里加回父原点，
-- 因为 Lua 侧的 `ui.*` 记录一律是屏幕绝对坐标（`core/script.rs` 里 space=Screen）。
local function resolve_rect(node, parent, values, where)
  local box = node.layout or {}
  local anchors = box.anchors or M.PRESETS[box.preset] or M.PRESETS.top_left
  local offsets = box.offsets or {}
  local at = where .. ".layout"
  local left = parent.w * anchors[1] + number_spec(offsets.left, values, at .. ".offsets.left")
  local top = parent.h * anchors[2] + number_spec(offsets.top, values, at .. ".offsets.top")
  local right = parent.w * anchors[3] + number_spec(offsets.right, values, at .. ".offsets.right")
  local bottom = parent.h * anchors[4] + number_spec(offsets.bottom, values, at .. ".offsets.bottom")
  local width = math.max(0, right - left)
  local height = math.max(0, bottom - top)
  local pivot = box.pivot or { 0.0, 0.0 }
  if type(box.min_size) == "table" then
    width = math.max(width, tonumber(box.min_size[1]) or 0)
    height = math.max(height, tonumber(box.min_size[2]) or 0)
  end
  if type(box.max_size) == "table" then
    local max_w = tonumber(box.max_size[1]) or 0
    local max_h = tonumber(box.max_size[2]) or 0
    -- 与引擎同口径：被夹紧时**支点不动**（右上角锚定的控件不会因为设了 max 而整体左移）。
    if max_w > 0 and max_w < width then
      left = left + (tonumber(pivot[1]) or 0) * (width - max_w)
      width = max_w
    end
    if max_h > 0 and max_h < height then
      top = top + (tonumber(pivot[2]) or 0) * (height - max_h)
      height = max_h
    end
  end
  return { x = parent.x + left, y = parent.y + top, w = width, h = height }
end

-- 文字预算（"这条文字允许占多宽"）：默认取节点自身宽度；也可写常量或
-- `{from="parent", minus=N}`。口径与 `layout.list_width()`/`top_row_width()` 一致，
-- 真实字宽由 `ui_text` 那侧按 `ui.measure_text` 量。
local function resolve_budget(node, rect, parent)
  local spec = node.budget
  if spec == nil then
    return rect.w
  end
  if type(spec) == "number" then
    return spec
  end
  if type(spec) == "table" then
    local base = spec.from == "parent" and parent.w or rect.w
    return base - (tonumber(spec.minus) or 0)
  end
  return rect.w
end

local function expand_id(id, index, prefix)
  local head = prefix or ""
  if index then
    return head .. (id:gsub("%$i", tostring(index)))
  end
  return head .. id
end

-- ---------- 记录收发 ----------
--
-- 为什么"收口"必须由这层做：引擎的 `UiState` 是 **retained** 的（只有切场景才清空），
-- "这一帧没画的那条记录"会一直留在屏上。现状是每个界面手写一串 `ui.clear`/`ui_text.hide`
-- （`dialogue.lua` 的 `clear_ui` 就是 14 行），漏一条就是"上一段对话的选项还挂在屏幕上"。
-- 预设知道**整棵树的 id 全集**，所以收口是遍历出来的，而不是靠人记。
local function hide_record(node, id, owner)
  if node.type == "text" then
    ui_text.hide(owner, id)
  elseif DRAWN[node.type] then
    ui.clear(id)
  end
end

local function emit(node, id, rect, values, parent, where, owner)
  if node.type == "panel" then
    local color = node.color or { 0, 0, 0, 0.5 }
    ui.panel(id, rect.x, rect.y, rect.w, rect.h,
      color[1] or 0, color[2] or 0, color[3] or 0, color[4] or 1)
    return
  end
  if node.type == "circle" then
    local color = node.color or { 0, 0, 0, 0.5 }
    local radius = node.radius and number_spec(node.radius, values, where .. ".radius") or rect.h / 2
    ui.circle(id, rect.x + rect.w / 2, rect.y + rect.h / 2, radius,
      color[1] or 0, color[2] or 0, color[3] or 0, color[4] or 1)
    return
  end
  if node.type == "bar" then
    local color = node.color or { 1, 1, 1 }
    ui.bar(id, rect.x, rect.y, rect.w, rect.h,
      number_spec(node.ratio, values, where .. ".ratio"),
      color[1] or 0, color[2] or 0, color[3] or 0)
    return
  end
  if node.type == "image" then
    local path = node.path
    local slice
    if node.texture then
      -- 贴图键也可以由运行期决定（页签 on/off、品质框）：槽位给了字符串就覆盖静态键。
      local key = node.texture
      local bound = node.slot and values[node.slot]
      if type(bound) == "string" then
        key = bound
      end
      local entry = skin.tex[key]
      if not entry then
        error("ui.prefab: " .. where .. ".texture 未在 ui/skin.lua 注册：" .. tostring(key), 0)
      end
      path = entry.path
      slice = entry.slice
    elseif node.path_slot then
      path = values[node.path_slot]
      if type(path) ~= "string" then
        error("ui.prefab: " .. where .. ".path_slot=" .. tostring(node.path_slot)
          .. " 必须是贴图路径字符串", 0)
      end
    end
    if slice then
      ui.image(id, rect.x, rect.y, rect.w, rect.h, path, 1.0, 1.0, 1.0, slice)
    else
      ui.image(id, rect.x, rect.y, rect.w, rect.h, path, 1.0, 1.0, 1.0)
    end
    return
  end
  if node.type == "text" then
    local raw = node.text
    if node.slot then
      local bound = values[node.slot]
      raw = type(bound) == "table" and bound.text or bound
    end
    local budget = resolve_budget(node, rect, parent)
    if node.mode == "wrap" then
      ui_text.draw_wrapped(owner, id, rect.x, rect.y, node.font, budget, node.lines, raw)
    elseif node.mode == "exact" then
      ui_text.draw_exact(owner, id, rect.x, rect.y, node.font, budget, raw)
    else
      ui_text.draw(owner, id, rect.x, rect.y, node.font, budget, raw)
    end
    return
  end
end

-- 该节点这一帧是否要画。三种情况都算"不画"，而**每一种都必须收口**：
--   ① 整屏隐藏（`render(name, values, {hidden=true})`，等价 SetActive(false)）；
--   ② `when` 指向的取值不为真（选中行才有标记、有立绘才画立绘框）；
--   ③ 内容槽位为空（空台词、越界的选项行）。
local function should_draw(node, values, hidden)
  if hidden then
    return false
  end
  if node.when and not values[node.when] then
    return false
  end
  if node.type == "text" and node.slot then
    local bound = values[node.slot]
    if bound == nil or bound == false then
      return false
    end
    if type(bound) == "table" and (bound.text == nil or bound.text == false) then
      return false
    end
  end
  if node.type == "image" and node.path_slot and not values[node.path_slot] then
    return false
  end
  return true
end

-- 条目取值与整屏取值的合并：条目里没有的槽位回落到整屏（行文案来自条目，
-- 行颜色/立绘尺寸可以来自整屏）。
local function merge_values(values, entry)
  local out = {}
  if type(entry) == "table" then
    for key, value in pairs(entry) do
      out[key] = value
    end
  elseif entry ~= nil then
    out.text = entry
  end
  for key, value in pairs(values) do
    if out[key] == nil then
      out[key] = value
    end
  end
  return out
end

-- 深度优先：先处理自己（画或收口），再走孩子。列表按模板展开 `max` 行，
-- 第 `#entries + 1 .. max` 行也遍历（内容为空 ⇒ 走收口），这是"关掉的行"真正的清屏路径。
local function walk(node, rect, values, owner, prefix, index, hidden, trail, parent_rect)
  local where = node_path(trail)
  if node.type == "instance" then
    local child = M.load(node.prefab)
    local bound = {}
    for key, value in pairs(values) do
      bound[key] = value
    end
    if node.bind then
      -- 子预设的槽位由父取值喂：`bind = { 子槽位 = 父槽位 }`。
      for child_slot, parent_slot in pairs(node.bind) do
        bound[child_slot] = values[parent_slot]
      end
    end
    walk(child.root, rect, bound, owner, node.prefix or prefix or "", index, hidden,
      { child.root.name or node.prefab }, parent_rect)
    return
  end

  local id = node.id and expand_id(node.id, index, prefix) or nil
  if id and DRAWN[node.type] then
    if should_draw(node, values, hidden) then
      emit(node, id, rect, values, parent_rect or rect, where, owner)
    else
      hide_record(node, id, owner)
    end
  end

  if node.type == "list" then
    local entries = values[node.slot] or {}
    local max = math.floor(node.max)
    if #entries > max then
      -- 条目多于槽位 = 静默丢最后几行（正是触屏动作条那条门要消掉的状态）。
      error("ui.prefab: " .. where .. " 槽位上限 " .. max .. " 装不下 " .. #entries
        .. " 条：加大 max 或开滚动", 0)
    end
    local axis = node.axis or "y"
    local spacing = tonumber(node.spacing) or 0
    for row = 1, max do
      local item_values = merge_values(values, entries[row])
      local item_rect = resolve_rect(node.item, rect, item_values, where .. "/item.layout")
      local step = (row - 1) * spacing
      if axis == "x" then
        item_rect.x = item_rect.x + step
      else
        item_rect.y = item_rect.y + step
      end
      walk(node.item, item_rect, item_values, owner, prefix, row, hidden,
        { where, node.item.name or "item" }, rect)
    end
    return
  end

  if type(node.children) == "table" then
    for child_index, child in ipairs(node.children) do
      local child_name = (type(child) == "table" and child.name) or ("children[" .. child_index .. "]")
      local child_trail = { trail[#trail] or "node", child_name }
      local child_rect = resolve_rect(child, rect, values, node_path(child_trail) .. ".layout")
      walk(child, child_rect, values, owner, prefix, index, hidden, child_trail, rect)
    end
  end
end

-- 画布矩形（根节点的父）：设计分辨率，1280×720。
local function canvas_rect()
  return { x = 0, y = 0, w = layout.DESIGN_W, h = layout.DESIGN_H }
end

-- 整屏取值 + 预设自带的 `probe`（校验用替身取值）合并：`probe` 只在缺该项时补，
-- 所以 `dump(name)` / `audit()` 不带取值也能把几何算完。真正"少了取值就抛错"的是
-- `number_spec`：数值槽（动态尺寸/比例）引用了没有取值的 slot 时报错，而不是静默当 0。
local function merged_values(tree, values, with_probe)
  local out = {}
  for key, value in pairs(values or {}) do
    out[key] = value
  end
  if with_probe then
    for key, value in pairs(tree.probe or {}) do
      if out[key] == nil then
        out[key] = value
      end
    end
  end
  return out
end

-- ---------- 对外入口 ----------

-- 遍历整棵树收口（隐藏 + 撤销登记），**不解析几何**、不读内容槽位。
-- 为什么不复用 `walk`：收口只需要"id 的全集"，而 `walk` 会先把每个节点的矩形算出来。
-- 于是"整屏隐藏"也被迫要求调用方把动态尺寸槽位喂齐（`portrait_h` 没给就抛错），
-- 而隐藏态根本没有东西要画——隐藏一个不该操作画面控件的面板，不该先备好它的内容。
-- `limit` 是列表展开的行数上限（模板行的 id 带 `$i`，必须逐行收）。
local function hide_tree(node, owner, prefix, limit)
  if node.type == "instance" then
    hide_tree(M.load(node.prefab).root, owner, node.prefix or prefix, limit)
    return
  end
  local id = node.id and expand_id(node.id, nil, prefix) or nil
  if id and DRAWN[node.type] then
    local rows = node.id:find("%$i") and limit or 1
    for row = 1, rows do
      hide_record(node, (id:gsub("%$i", tostring(row))), owner)
    end
  end
  if node.type == "list" and type(node.item) == "table" then
    hide_tree(node.item, owner, prefix, math.floor(node.max))
  end
  if type(node.children) == "table" then
    for _, child in ipairs(node.children) do
      hide_tree(child, owner, prefix, limit)
    end
  end
end

--- 绘制一个预设。
---   name   预设名（= 文件名去掉 `.ui.json`）
---   values 槽位取值表：文本槽给字符串，贴图槽给 skin 键或路径，列表槽给条目数组，
---          数值槽（动态尺寸/比例/`when` 判据）给数字或真值
---   opts   `{ owner = 覆盖 ui_text 的 owner（共用子预设时按宿主面板命名）,
---             hidden = 整屏隐藏, prefix = id 前缀 }`
function M.render(name, values, opts)
  local tree = M.load(name)
  opts = opts or {}
  local owner = opts.owner or tree.owner or name
  local prefix = opts.prefix
  if opts.hidden == true then
    -- 等价 Unity 的 `SetActive(false)`：整棵树收口，几何与内容槽位一概不解析。
    ui_text.reset(owner)
    hide_tree(tree.root, owner, prefix, 1)
    last_render[name] = { owner = owner, prefix = prefix }
    return true
  end
  local bound = merged_values(tree, values, false)
  local root
  if tree.layout_source then
    -- 迁移期：面板矩形的唯一定义处还在 `layout.PANEL.*`（`layout.audit()` 与
    -- `TOUCH_KEEP_CLEAR` 都指它）。这里以它为准，而**预设自己另写了一份**，
    -- 两份必须相等由 `M.audit()` ② 守——不一致就报错，不静默跟着漂。
    local panel = layout.PANEL[tree.layout_source]
    if not panel then
      error("ui.prefab: " .. name .. ".layout_source 不在 layout.PANEL 里："
        .. tostring(tree.layout_source), 0)
    end
    root = { x = panel.x, y = panel.y, w = panel.w, h = panel.h }
  else
    root = resolve_rect(tree.root, canvas_rect(), bound, name .. "/root.layout")
  end
  ui_text.begin(owner)
  last_render[name] = { owner = owner, prefix = opts.prefix }
  walk(tree.root, root, bound, owner, opts.prefix, nil, opts.hidden == true,
    { tree.root.name or "root" }, canvas_rect())
  return true
end

--- 整屏收口（关面板）。取代各处手写的 `clear_ui()`：漏一条就是一条留在屏上的旧文字。
function M.clear(name)
  local tree = cache[name]
  local entry = last_render[name]
  if not tree then
    return false
  end
  local owner = (entry and entry.owner) or tree.owner or name
  ui_text.reset(owner)
  hide_tree(tree.root, owner, entry and entry.prefix, 1)
  last_render[name] = nil
  return true
end

--- 只切可见性（内容取值不变）。
function M.set_visible(name, visible, values)
  return M.render(name, values or {}, { hidden = not visible })
end

-- ---------- 开发通道 ----------
--
-- Unity 的"场景视图"在这个仓库里对位的是这三样：`dump`（解析后的真值表）、
-- `audit`（版面不变量）、`reload`（改完 JSON 立刻看效果）。调 UI 看的是这张表，
-- 而不是猜——预设改了偏移，这里就是改完的结果。

--- 展开后的节点清单：`{ { node=路径, type=, id=, x=, y=, w=, h=, slot=, font=, text=, texture= } }`。
--- 缺槽位时按预设的 `probe`（校验用替身取值）补，所以 `dump(name)` 不带取值也能跑。
function M.dump(name, values)
  local tree = M.load(name)
  local bound = merged_values(tree, values, true)
  local rows = {}
  local root
  if tree.layout_source and layout.PANEL[tree.layout_source] then
    local panel = layout.PANEL[tree.layout_source]
    root = { x = panel.x, y = panel.y, w = panel.w, h = panel.h }
  else
    root = resolve_rect(tree.root, canvas_rect(), bound, name .. "/root.layout")
  end

  local function visit(node, rect, trail, index, parent_rect)
    rows[#rows + 1] = {
      node = node_path(trail),
      type = node.type,
      id = node.id and expand_id(node.id, index, nil) or nil,
      x = rect.x,
      y = rect.y,
      w = rect.w,
      h = rect.h,
      slot = node.slot,
      font = node.font,
      text = node.text,
      texture = node.texture,
    }
    if node.type == "list" and type(node.item) == "table" then
      local entries = bound[node.slot] or {}
      local spacing = tonumber(node.spacing) or 0
      local axis = node.axis or "y"
      for row = 1, math.floor(node.max) do
        local item_values = merge_values(bound, entries[row])
        local item_rect = resolve_rect(node.item, rect, item_values, node_path(trail) .. "/item.layout")
        local step = (row - 1) * spacing
        if axis == "x" then
          item_rect.x = item_rect.x + step
        else
          item_rect.y = item_rect.y + step
        end
        visit(node.item, item_rect, { node_path(trail), node.item.name or "item" }, row, rect)
      end
    end
    if type(node.children) == "table" then
      for child_index, child in ipairs(node.children) do
        local child_name = (type(child) == "table" and child.name) or ("children[" .. child_index .. "]")
        local child_trail = { trail[#trail] or "node", child_name }
        visit(child, resolve_rect(child, rect, bound, node_path(child_trail) .. ".layout"),
          child_trail, index, rect)
      end
    end
  end

  visit(tree.root, root, { tree.root.name or "root" }, nil, canvas_rect())
  return rows, tree.owner or name
end

--- 文本形式（门与日志读这个；字段顺序固定，可 diff）。
function M.dump_text(name, values)
  local rows = M.dump(name, values)
  local lines = {}
  for _, row in ipairs(rows) do
    if row.id then
      lines[#lines + 1] = string.format("%s\t%s\t%s\t%.2f\t%.2f\t%.2f\t%.2f",
        row.node, row.type, row.id, row.x or 0, row.y or 0, row.w or 0, row.h or 0)
    end
  end
  return table.concat(lines, "\n")
end

--- 版面不变量（与 `layout.audit()` 同一口径，登记进 `validate_data` 的启动门）：
---   ① 每个预设都能加载并通过结构校验（`load` 内部会抛，这里兜住成 issue）；
---   ② 声明了 `layout_source` 的预设，root 上写的那份 offsets 必须与
---      `layout.PANEL.*` 一致——迁移期矩形唯一定义处还在 `layout.lua`，这一条防两边各改一处
---      （`render` 用 PANEL 的值，所以预设里写错不会立刻看出来，只会悄悄分叉）；
---   ③ 解析后不越出设计画布（越界 = 被窗口切掉，而且不报错）；
---   ④ 列表模板行不越出列表矩形之外（行距 × 条数超出面板 = 最后一行画到面板外面）。
function M.audit()
  local issues = {}
  for _, name in ipairs(M.NAMES) do
    local ok, tree = pcall(M.load, name)
    if not ok then
      -- `load` 的 error 文本已经带节点路径，整条收下。
      issues[#issues + 1] = tostring(tree):gsub("^ui%.prefab: ", "")
    else
      local rows = M.dump(name, {})
      local canvas_w, canvas_h = layout.DESIGN_W, layout.DESIGN_H
      for _, row in ipairs(rows) do
        if row.id and (row.x < 0 or row.y < 0 or row.x + row.w > canvas_w or row.y + row.h > canvas_h) then
          issues[#issues + 1] = string.format(
            "%s：%s 越出设计画布 %dx%d（%.1f,%.1f %.1fx%.1f）",
            name, row.node, canvas_w, canvas_h, row.x, row.y, row.w, row.h)
        end
      end
      if tree.layout_source then
        local panel = layout.PANEL[tree.layout_source]
        if not panel then
          issues[#issues + 1] = name .. ".layout_source 不在 layout.PANEL 里："
            .. tostring(tree.layout_source)
        else
          local authored = resolve_rect(tree.root, canvas_rect(), {}, name .. "/root.layout")
          if math.abs(authored.x - panel.x) > 0.01 or math.abs(authored.y - panel.y) > 0.01
              or math.abs(authored.w - panel.w) > 0.01 or math.abs(authored.h - panel.h) > 0.01 then
            issues[#issues + 1] = string.format(
              "%s：预设 root 写的矩形 %.1f,%.1f %.1fx%.1f 与 layout.PANEL.%s (%.1f,%.1f %.1fx%.1f) 不一致",
              name, authored.x, authored.y, authored.w, authored.h,
              tree.layout_source, panel.x, panel.y, panel.w, panel.h)
          end
        end
      end
    end
  end
  return #issues == 0, issues
end

--- `validate_data` 入口（启动、`game --check`、`editor --check` 三条路径都会跑）。
function M.validate_all()
  return M.audit()
end

return M
