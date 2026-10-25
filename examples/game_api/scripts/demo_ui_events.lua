-- ui_test：**场景 UI 组件**的运行时读回与交互（运行时动态 `ui.*` 的深入演示在 api_ui）。
--
-- 本场景的 UI 是"数据"：按钮列由 Container(kind=vbox) 摆放、信息面板用九宫格 Image、
-- 六种容器 kind 各有一份样本。Lua 侧没有"读场景实体布局矩形"的 API，所以这里做两件事：
--   1. 用 `entity.find` / `visibility.get` / `visibility.set` 读写**引擎侧**的场景实体状态；
--   2. 把 Container 的排布规则用同一套公式复算一份（脚本里的 BUTTON_RECTS / 内缩矩形），
--      自检断言就落在"引擎状态 + 同一公式"这两个可重复的事实上。
--
-- 点击走引擎 `process_ui_clicks`：命中带 `UiEvents event=click` 的实体后调用全局
-- `on_click(实体名)`（main.lua → router → 本模块），计数与高亮只在这里更新。
--
-- 场景里的 TitleText / HeaderComponentsText 在页眉上，InfoPanel 的文本压在右侧 API 卡上，
-- 它们由 `hide_scene_text` 让位；按钮与图标（演示对象）保持可见。
local lab = require("lab")

local M = {}

-- 五个可点按钮 + 一个禁用按钮的屏幕矩形（Container vbox：padding 16、spacing 12、
-- 容器 (60,120)-(340,560)、按钮 240x48 ⇒ 首个 (80,136)，逐个下移 60）。
local BUTTON_ORDER = {
    "StartButton",
    "ResetButton",
    "CloseButton",
    "PointerButton",
    "SubmitButton",
    "DisabledButton",
}
local LABEL_OF = {
    StartButton = "StartLabel",
    ResetButton = "ResetLabel",
    CloseButton = "CloseLabel",
    PointerButton = "PointerLabel",
    SubmitButton = "SubmitLabel",
    DisabledButton = "DisabledLabel",
}
local BUTTON_X, BUTTON_Y, BUTTON_W, BUTTON_H, BUTTON_STEP = 80, 136, 240, 48, 60

-- CloseButton 负责藏起来的场景实体（Visibility 是逐实体的层遮罩）。
local INFO_PANEL_ENTITIES = {
    "InfoPanel",
    "InfoPanelTitle",
    "InfoClicksLabel",
    "InfoSubmitsLabel",
    "InfoBindNote",
    "InfoPreviewIcon",
    "InfoPanelHint",
}

-- 六种容器 kind 的模型矩形（与 ui_test.scene 的 UiLayout / Container 字段一致）。
local COLUMN_RECT = { 60, 120, 340, 560 }
local LEGEND_RECT = { 380, 120, 860, 180 }
local GRID_RECT = { 380, 220, 640, 416 }
local CENTER_RECT = { 380, 456, 560, 552 }
local MARGIN_RECT = { 600, 456, 860, 552 }

-- 本模块拥有的动态 UI id。
local ID_CLICKS = "ui_events_clicks"
local ID_SUBMITS = "ui_events_submits"
local ID_INFO = "ui_events_info"
local ID_NOTE = "ui_events_note"
local ID_HIGHLIGHT = "ui_events_highlight"
local ID_SCRATCH = "ui_events_scratch"

local clicks = 0
local submits = 0
local last_event = "none"
local last_target = "-"
local panel_visible = true
local highlighted = nil
local found = {}

local function reset_dynamic()
    ui.clear(ID_CLICKS)
    ui.clear(ID_SUBMITS)
    ui.clear(ID_HIGHLIGHT)
    ui.clear(ID_SCRATCH)
    ui.hide(ID_INFO)
    ui.hide(ID_NOTE)
end

--- 把场景实体按名字抓一遍；缺实体时返回 nil，后续读回会跳过而不是崩。
local function bind_entities()
    found = {}
    for _, name in ipairs(BUTTON_ORDER) do
        found[name] = entity.find(name)
    end
    found.InfoPanel = entity.find("InfoPanel")
    found.InfoPreviewIcon = entity.find("InfoPreviewIcon")
end

local function button_rect(index)
    return BUTTON_X, BUTTON_Y + (index - 1) * BUTTON_STEP, BUTTON_W, BUTTON_H
end

--- Visibility 读写：0 = 藏起来，`visibility.all()` = 全可见。
local function set_panel_visible(visible)
    local layers = visible and visibility.all() or 0
    for _, name in ipairs(INFO_PANEL_ENTITIES) do
        local id = entity.find(name)
        if id then
            visibility.set(id, layers)
        end
    end
    panel_visible = visible
end

local function panel_mask(name)
    local id = entity.find(name)
    if not id then
        return nil
    end
    return visibility.get(id)
end

-- ---------------------------------------------------------------------------
-- 步骤
-- ---------------------------------------------------------------------------

local steps = {
    {
        id = "button_column",
        title = "Container vbox",
        claim = "按钮列的位置不是手写的：vbox 用 padding 16、spacing 12 把 6 个按钮逐个排下来",
        api = { "entity.find", "ui.text_at", "ui.text_get" },
        code = "entity.find('StartButton'); 80 + (i-1)*(48+12)",
        enter = function()
            reset_dynamic()
            bind_entities()
            highlighted = nil
            last_event = "none"
            last_target = "-"
        end,
        run = function()
            local count = 0
            for _, name in ipairs(BUTTON_ORDER) do
                if found[name] then
                    count = count + 1
                end
            end
            ui.text_at(
                ID_INFO,
                360,
                500,
                string.format("6 buttons, first (80,136), step 60, %d found", count),
                1.0,
                13
            )
            lab.note("容器按 UiLayout 尺寸摆放子实体；按钮矩形只是同一公式的镜像")
        end,
        read = function()
            local _, y6 = button_rect(6)
            return {
                { label = "buttons", value = #BUTTON_ORDER },
                { label = "first", value = string.format("%d,%d", BUTTON_X, BUTTON_Y) },
                { label = "last_y", value = y6 },
                { label = "column_h", value = COLUMN_RECT[4] - COLUMN_RECT[2] },
            }
        end,
        check = function(t)
            t:ok(found.StartButton ~= nil, "StartButton is in the scene")
            t:ok(found.SubmitButton ~= nil, "SubmitButton is in the scene")
            t:ok(found.DisabledButton ~= nil, "DisabledButton is in the scene")
            local x, y = button_rect(1)
            t:eq(x, 80, "vbox padding+alignment puts the first button at x=80")
            t:eq(y, 136, "first button starts at 120 + 16")
            local _, last_y = button_rect(6)
            t:eq(last_y, 436, "sixth button sits at the last vbox slot")
            t:eq(last_y + BUTTON_H, 484, "last button bottom stays inside the column")
        end,
    },
    {
        id = "click_counters",
        title = "Runtime bindings",
        claim = "场景 Text 是静态的：计数由脚本每帧用 ui.text_at 重画，ui.text_get 读回同一串字",
        api = { "ui.text_at", "ui.text_get", "ui.clear", "entity.find" },
        code = "ui.text_at('ui_events_clicks', x, y, tostring(clicks), 1.3)",
        enter = function()
            reset_dynamic()
            bind_entities()
            clicks = 0
            submits = 0
            last_event = "none"
            last_target = "-"
            set_panel_visible(true)
        end,
        run = function()
            ui.text_at(ID_CLICKS, 360, 300, "clicks = " .. tostring(clicks), 1.0, 16)
            ui.text_at(ID_SUBMITS, 360, 326, "submits = " .. tostring(submits), 1.0, 16)
            ui.text_at(
                ID_NOTE,
                360,
                352,
                "last event = " .. last_event .. " / " .. last_target,
                1.0,
                12
            )
            lab.note("LMB 点按钮 -> 引擎 on_click(name) -> 计数与高亮")
        end,
        read = function()
            local drawn = ui.text_get(ID_CLICKS)
            return {
                { label = "clicks", value = clicks },
                { label = "submits", value = submits },
                { label = "drawn", value = drawn and drawn.text or "-" },
                { label = "event", value = last_event },
            }
        end,
        check = function(t)
            local drawn = ui.text_get(ID_CLICKS)
            t:ok(drawn ~= nil, "the counter text was drawn")
            t:eq(drawn and drawn.text, "clicks = 0", "the drawn string matches the model")
            t:eq(ui.text_get(ID_SUBMITS).text, "submits = 0", "submits starts at zero too")
            -- `ui.clear` 删不掉同 id 的普通文本，所以这里清的是派生提示；再用 text_at 重建。
            ui.clear(ID_SCRATCH)
            ui.text_at(ID_SCRATCH, 360, 380, "scratch", 1.0, 12)
            t:eq(ui.text_get(ID_SCRATCH).text, "scratch", "text_at creates the scratch label")
            ui.clear(ID_SCRATCH)
            t:eq(ui.text_get(ID_SCRATCH).text, "scratch", "ui.clear does not remove plain dynamic text")
            ui.hide(ID_SCRATCH)
            t:eq(ui.text_get(ID_SCRATCH).visible, false, "ui.hide is the way to drop plain text")
            t:eq(clicks, 0, "counting starts from zero")
        end,
    },
    {
        id = "info_visibility",
        title = "Visibility mask",
        claim = "CloseButton 的隐藏是 visibility.set(id, 0)，读回 0 与 visibility.all() 都是引擎侧事实",
        api = { "visibility.all", "visibility.get", "visibility.set", "entity.find" },
        code = "visibility.set(entity.find('InfoPanel'), visibility.all())",
        enter = function()
            reset_dynamic()
            bind_entities()
            set_panel_visible(true)
        end,
        run = function()
            ui.text_at(
                ID_INFO,
                360,
                300,
                string.format("InfoPanel mask = %s (all = %s)", tostring(panel_mask("InfoPanel")), tostring(visibility.all())),
                1.0,
                13
            )
            lab.note("visibility.get 读的是实体层遮罩；0 = 整块隐藏")
        end,
        read = function()
            return {
                { label = "mask", value = tostring(panel_mask("InfoPanel")) },
                { label = "all", value = tostring(visibility.all()) },
                { label = "visible", value = tostring(panel_visible) },
            }
        end,
        check = function(t)
            local all = visibility.all()
            t:ok(all > 0, "visibility.all is a non-empty layer mask")
            t:eq(panel_mask("InfoPanel"), all, "the panel starts fully visible")
            set_panel_visible(false)
            t:eq(panel_mask("InfoPanel"), 0, "hiding writes the zero mask")
            t:eq(panel_mask("InfoPreviewIcon"), 0, "every child of the block is hidden")
            set_panel_visible(true)
            t:eq(panel_mask("InfoPanel"), all, "restoring writes visibility.all back")
        end,
    },
    {
        id = "container_kinds",
        title = "Five container kinds",
        claim = "vbox/hbox/grid/center/margin 的差别只在解算规则：同一组 UiLayout 得到不同矩形",
        api = { "entity.find", "ui.text_at" },
        code = "entity.find('LegendRow') / entity.find('IconGrid') / entity.find('MarginCard')",
        enter = function()
            reset_dynamic()
            bind_entities()
        end,
        run = function()
            ui.text_at(
                ID_INFO,
                360,
                430,
                "hbox chips 140x40 @10 | grid 100x80 @8 | center 120x60 | margin 234x70",
                1.0,
                12
            )
            lab.note("每个 kind 的容器都在场景里；这里是它们解算后的模型矩形")
        end,
        read = function()
            return {
                { label = "legend_inner", value = "460x40" },
                { label = "grid_cell", value = "100x80" },
                { label = "center_chip", value = "120x60" },
                { label = "margin_inner", value = "234x70" },
            }
        end,
        check = function(t)
            t:ok(entity.find("ButtonColumn") ~= nil, "vbox container exists")
            t:ok(entity.find("LegendRow") ~= nil, "hbox container exists")
            t:ok(entity.find("IconGrid") ~= nil, "grid container exists")
            t:ok(entity.find("CenterBadge") ~= nil, "center container exists")
            t:ok(entity.find("MarginCard") ~= nil, "margin container exists")
            -- 模型：hbox 内宽 = 480 - 2*10 = 460；三个 140 宽 + 两个 10 间距 = 440。
            t:eq(LEGEND_RECT[3] - LEGEND_RECT[1] - 20, 460, "hbox inner width")
            t:eq(140 * 3 + 10 * 2, 440, "hbox content width fits the inner width")
            -- margin：600..860 减左右 padding 10/16 = 234；456..552 减上下 10/16 = 70。
            t:eq(MARGIN_RECT[3] - MARGIN_RECT[1] - 26, 234, "margin inner width")
            t:eq(MARGIN_RECT[4] - MARGIN_RECT[2] - 26, 70, "margin inner height")
            -- center：120x60 在 180x96 的内区里居中。
            t:eq(CENTER_RECT[1] + (CENTER_RECT[3] - CENTER_RECT[1] - 120) / 2, 410, "center chip x")
            -- grid：2x2、每格 100x80、间距 8。
            t:eq(GRID_RECT[1] + 8, 388, "grid starts after padding")
        end,
    },
    {
        id = "highlight_overlay",
        title = "Highlight overlay",
        claim = "动态高亮画在按钮外圈 8px：面板读回矩形，文字读回内容，点击只改模型",
        api = { "ui.panel", "ui.panel_get", "ui.text_at", "ui.text_get", "ui.clear", "entity.find" },
        code = "ui.panel('ui_events_highlight', x-8, y-8, w+16, h+16, 1.0,0.82,0.22,1.0)",
        enter = function()
            reset_dynamic()
            bind_entities()
            highlighted = "StartButton"
        end,
        run = function()
            local rect = { button_rect(1) }
            if highlighted == "StartButton" then
                ui.panel(ID_HIGHLIGHT, rect[1] - 8, rect[2] - 8, rect[3] + 16, rect[4] + 16, 1.0, 0.82, 0.22, 0.85)
            end
            ui.text_at(ID_INFO, 360, 300, "highlight = " .. tostring(highlighted), 1.0, 14)
            lab.note("点击按钮会换高亮目标；框在按钮外圈，露出来的那一圈才看得见")
        end,
        read = function()
            local panel = ui.panel_get(ID_HIGHLIGHT)
            local text = ui.text_get(ID_INFO)
            return {
                { label = "target", value = tostring(highlighted) },
                { label = "frame", value = panel and string.format("%.0fx%.0f @%.0f,%.0f", panel.width, panel.height, panel.x, panel.y) or "-" },
                { label = "label", value = text and text.text or "-" },
            }
        end,
        check = function(t)
            local panel = ui.panel_get(ID_HIGHLIGHT)
            t:ok(panel ~= nil, "the highlight panel exists")
            t:eq(panel.x, 72, "frame starts 8px left of the button")
            t:eq(panel.y, 128, "frame starts 8px above the button")
            t:eq(panel.width, 256, "frame is 16px wider than the button")
            t:eq(panel.height, 64, "frame is 16px taller than the button")
            t:eq(ui.text_get(ID_INFO).text, "highlight = StartButton", "the caption reads back")
            ui.clear(ID_HIGHLIGHT)
            t:eq(ui.panel_get(ID_HIGHLIGHT), nil, "clear removes the frame")
        end,
    },
}

-- ---------------------------------------------------------------------------
-- 生命周期
-- ---------------------------------------------------------------------------

function M.on_create()
    lab.start({
        scene = "ui_test",
        title = "Scene UI",
        category = "UI",
        steps = steps,
        -- 只让页眉 / 右栏上确实压住 shell 的文本让位；按钮与图标（演示对象）保留。
        hide_scene_text = {
            "TitleText",
            "HeaderComponentsText",
            "VBoxLabel",
            "InfoPanelLabel",
            "InfoPanelTitle",
            "InfoClicksLabel",
            "InfoSubmitsLabel",
            "InfoBindNote",
            "InfoPanelHint",
        },
    })
    log.info("ui_events: 6 buttons + 5 container kinds + nine-slice InfoPanel")
end

function M.update()
    if not lab.active() then
        return "ui_events_inactive"
    end
    return lab.tick()
end

function M.self_test()
    return lab.self_test()
end

--- `UiEvents event=click` 的全局回调：参数是实体名（引擎 `process_ui_clicks` 的契约）。
function M.on_click(name)
    last_event = "click"
    last_target = tostring(name)
    if name == "StartButton" then
        clicks = clicks + 1
        highlighted = name
    elseif name == "ResetButton" then
        clicks = 0
        highlighted = name
    elseif name == "CloseButton" then
        set_panel_visible(not panel_visible)
        highlighted = name
    elseif name == "SubmitButton" then
        submits = submits + 1
        highlighted = name
    elseif name == "PointerButton" then
        highlighted = name
    else
        highlighted = nil
    end
    log.info("ui_events: on_click " .. tostring(name) .. " clicks=" .. tostring(clicks))
    return true
end

--- 场景 UiEvents 的 drag 声明在运行时不派发；真正的拖拽入口是 `ui.drag_end`（api_ui 演示）。
function M.on_drag(from_id, from_index, to_id, to_index)
    last_event = "drag"
    last_target = string.format("%s#%d -> %s#%d", tostring(from_id), from_index or 0, tostring(to_id), to_index or 0)
    return true
end

function M.on_destroy()
    ui.clear(ID_CLICKS)
    ui.clear(ID_SUBMITS)
    ui.clear(ID_HIGHLIGHT)
    ui.clear(ID_SCRATCH)
    ui.hide(ID_INFO)
    ui.hide(ID_NOTE)
    set_panel_visible(true)
    lab.stop()
end

return M
