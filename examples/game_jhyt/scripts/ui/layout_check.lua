-- layout_check.lua：版面校验器（M32，2026-09-27 转横屏）。
--
-- 为什么单独一个文件，而不是把 `validate_all` 写进 `ui/layout.lua`：
--   * `validate_data.lua` 的注册表要求每个校验器是**纯数据/纯规则模块**，
--     入口统一叫 `validate_all()` → 返回 `(ok, issues)`；`ui/layout.lua` 是运行时
--     几何模块（被 hud/shop/dialogue 等 require），往里塞校验入口会让"几何定义"
--     和"几何校验"的调用者混在一起；
--   * 校验内容除几何自洽（`ui/layout.lua` 的 `audit()`）之外，还包括**设计分辨率契约**
--     ——场景文件 `meta ui_reference_width/height` 必须与 `layout.DESIGN_W/H` 一致。
--     这条契约横跨 Lua 与场景文件，不属于任何一方，正好由本模块守。
--
-- 登记在 `scripts/validate_data.lua`，因此启动、`game --check`、`editor --check`
-- 三条路径都会跑（与其它领域数据同一个门）。
local M = {}

M.version = 1

-- 设计分辨率契约：与 `ui/layout.lua` 必须一致的那些地方。
--   * `scenes/*.scene` 的 `meta ui_reference_width/height`（编辑器按它画设计框、
--     scene UI 的 Canvas 空间按它缩放）；
--   * `project.toml` 的 `window_width/height`（发布窗口尺寸，宿主读取）。
--
-- 读一个工程相对路径的文本文件；**宿主没有 `io` 时返回 nil**（= 跳过文件级检查）。
--
-- 为什么必须判 `io` 本身而不是只判返回值：WebGL / 微信小游戏的 Lua 运行时**不提供 `io`**
-- （它靠 `fetch`/宿主读取，没有文件系统 API），于是 `io.open` 在这一行直接抛
-- `attempt to index a nil value (global 'io')`。这不是理论问题——M32 首次把它登记进
-- `validate_data` 之后，两条 web 发布形态的启动**当场失败**（`domain data validation failed`
-- → `data-rustlua-boot=failed`），而原生/headless 那边一切正常（那里 `io` 存在）。
--
-- 所以这里的口径与适配层一致：**能力探测 + 静默跳过**，不把"宿主读不到文件"当成数据错误。
-- 代价是"json/scene 的 meta 契约"在 web 运行时不再被校验——但 web 产物本来就用同一批
-- 场景文件构建，而**发布前的 `game --check`（原生、有 `io`）会照常校验**，那才是守卫点。
--
-- **必须拼绝对路径**（2026-09-23 修）：`io.open` 的相对路径按**进程工作目录**解析，而
-- `game --check` 并不 chdir 到工程根——在 `engine/` 下跑 `--check --project
-- ../examples/game_jhyt` 时，`io.open("scenes/main.scene")` 与 `io.open("project.toml")`
-- 都返回 nil，于是**版权契约与设计分辨率契约从未真正跑过**（静默跳过 ⇒ 恒绿）。
-- 工程根从 `app.project_root()` 取（引擎暴露的绝对路径，正斜杠）。
local function read_file(path)
  if type(io) ~= "table" or type(io.open) ~= "function" then
    return nil
  end
  local root = ""
  if type(app) == "table" and type(app.project_root) == "function" then
    local ok, value = pcall(app.project_root)
    if ok and type(value) == "string" then
      root = value
    end
  end
  local candidates = {}
  if root ~= "" then
    candidates[#candidates + 1] = root .. "/" .. path
  end
  candidates[#candidates + 1] = path
  for _, candidate in ipairs(candidates) do
    local handle = io.open(candidate, "r")
    if handle then
      local text = handle:read("*a")
      handle:close()
      return text
    end
  end
  return nil
end

-- 收集引擎可见的脚本目录（`scene`/`io` 的可用范围由宿主决定；读不到就跳过，
-- 不把"读不到文件"当成数据错误——那是宿主能力问题，不是版面问题）。
local function scene_files()
  -- Lua 侧没有列目录能力：场景清单以 `data/maps.lua` 注册的 id 为准 + 门面 main。
  local ids = { "main" }
  local ok, maps = pcall(require, "data.maps")
  if ok and type(maps) == "table" and type(maps.maps) == "table" then
    for id in pairs(maps.maps) do
      if type(id) == "string" then
        ids[#ids + 1] = id
      end
    end
  end
  table.sort(ids)
  return ids
end

-- 场景 meta 里的一行（`meta\tui_reference_width\t720`）。
local function meta_value(text, key)
  for line in text:gmatch("[^\r\n]+") do
    local name, value = line:match("^meta%s+([%w_%.]+)%s+(.+)$")
    if name == key then
      return (value:gsub("%s+$", ""))
    end
  end
  return nil
end

-- 工程根：**不能**用 `scene.reload()`——那在引擎里是"当前场景名"，门面/玩法地图上取值不同，
-- 而这里要的是"工程目录"。用 `ui/layout_check.lua` 自己在 require 时的模块名反推：
-- `...` 是 `ui.layout_check`，相对路径 `scenes/x.scene` 本来就由宿主按工程根解析，
-- 因此直接传相对路径即可（与 `project.toml` 的读法一致）。
local function read_project_file(relative)
  return read_file(relative)
end

function M.validate_all()
  local layout = require("ui.layout")
  local issues = {}

  -- ① 几何自洽：矩形在设计画布内、面板互不重叠、设计分辨率是正整数。
  local ok, geometry_issues = layout.audit()
  if not ok then
    for _, issue in ipairs(geometry_issues or {}) do
      issues[#issues + 1] = tostring(issue)
    end
  end

  -- ② 设计分辨率契约：每个场景的 `meta ui_reference_*` 必须与 `ui/layout.lua` 一致。
  --     不一致的表现是"场景 UI 与 Lua 面板各按一套尺寸排"——画面上错位，
  --     而且两边都不会报错，所以必须在这里挡住。
  local want_w = tostring(layout.DESIGN_W)
  local want_h = tostring(layout.DESIGN_H)
  for _, id in ipairs(scene_files()) do
    local path = "scenes/" .. id .. ".scene"
    local text = read_file(path)
    if text then
      local got_w = meta_value(text, "ui_reference_width")
      local got_h = meta_value(text, "ui_reference_height")
      if got_w ~= want_w or got_h ~= want_h then
        issues[#issues + 1] = string.format(
          "%s:meta ui_reference_width/height = %s×%s，应为 %s×%s（与 scripts/ui/layout.lua 的 DESIGN_W/DESIGN_H 一致）",
          path, tostring(got_w), tostring(got_h), want_w, want_h
        )
      end
    end
  end

  -- ③ 发布窗口契约：`project.toml` 的 window_width/height 与设计分辨率一致。
  local toml = read_file("project.toml")
  if toml then
    local win_w = toml:match("window_width%s*=%s*(%d+)")
    local win_h = toml:match("window_height%s*=%s*(%d+)")
    if win_w ~= want_w or win_h ~= want_h then
      issues[#issues + 1] = string.format(
        "project.toml:window_width/height = %s×%s，应为 %s×%s（发布窗口按设计分辨率开，Lua 面板才 1:1 对齐）",
        tostring(win_w), tostring(win_h), want_w, want_h
      )
    end
  end

  -- ④ 触屏控件的平台门：桌面**不该画**摇杆与虚拟按钮，触屏宿主**必须画**。
  --     两边都守是因为这一条两侧都会静默坏：桌面画着按钮只是难看（键鼠用不到），
  --     触屏没了按钮则是**玩不了**。判定源是 `app.platform()`——不能用"宿主有没有
  --     `input.virtual_key`"，那四个 API 在所有宿主统一注册，探能力恒为真。
  --     宿主报告不了平台（旧 wasm 里还没有 `app.platform`）时**跳过**：这里返回错误
  --     会让 web 启动直接失败（`validate_data` 是启动门），而"检查不了"不是数据错误。
  if type(app) == "table" and type(rawget(app, "platform")) == "function" then
    local ok_pad, pad = pcall(require, "ui.virtual_pad")
    if ok_pad and type(pad) == "table" and type(pad.available) == "function" then
      local platform = tostring(app.platform())
      local drawn = pad.available() == true
      if platform == "desktop" and drawn then
        issues[#issues + 1] = "ui.virtual_pad 在桌面平台（app.platform()='desktop'）仍然可用/会绘制：摇杆与虚拟按钮该收掉"
      elseif platform ~= "desktop" and not drawn then
        issues[#issues + 1] = string.format(
          "ui.virtual_pad 在触屏平台（app.platform()='%s'）却判定为不可用：屏上没有摇杆与按钮，触屏无法操作",
          platform
        )
      end
    end
  end

  return #issues == 0, issues
end

return M
