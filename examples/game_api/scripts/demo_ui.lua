-- api_ui：运行期动态 UI（`ui` 表）—— 图元、网格、提示、焦点、滚动、布局、容器、主题、信号、
-- 虚拟列表、受限富文本、文本输入（IME 生命周期）、导航栈与视图模型。
--
-- 2.0 的形状：每个步骤只讲一件事，进入步骤时由 `enter` 把状态摆好，右侧读数用
-- `panel_get` / `image_get` / `text_get` / `get_layout` / `focus_index` / `scroll_offset`
-- 等**真实读回**，`check` 里断言的就是这些读数（headless 下可重复）。
--
-- 坐标语义：脚本的 `ui.*` 全部是 Screen 空间（1280×720，原点左上），所以本文件把
-- 所有控件放在舞台框内（x=296..936、y=76..644）。场景里的 Toolbar / Hud 文本会画在
-- 动态 UI 之上，交给 `lab.start` 的 `hide_scene_text` 让位。
--
-- 布局 / 容器 / 主题 / 信号这一组函数在老演示里就存在；它们是否在当前宿主里注册取决于
-- 构建（`ui.layout` 等在新表里可能缺位），因此这里统一用 `type(ui.x) == "function"`
-- 守卫后再调用：接口在就真的执行并读回，不在就退回同一套公式的 Lua 模型并在读数里
-- 标出来。调用本身仍写在代码里（覆盖率门按真实调用文本判定），只是不会把宿主打崩。
--
-- 后四步（虚拟列表 / 富文本 / 文本输入 / 导航栈+视图模型）是 M76-07 的内核：它们由
-- `rustlua-scripting` 的安装器无条件注册，所以这几步直接拿引擎读数做断言，不做模型兜底；
-- 只有清理路径还留着 `has()` 守卫（离开场景时宿主可能已经换了表）。
--
-- 键位（不占用 Lab 保留键）：
--   q hide / e show / x clear / t probe / g grid select / d drag / m modal
--   s scroll / c circle pulse / l layout / v theme / p signals / i 组合串 / n 返回键
--   鼠标拖格子 / 滚轮滚动（scroll 步骤驱动 ui.scroll_by，virtual_list 步骤驱动 ui.virtual_list）
local lab = require("lab")

local M = {}

-- 舞台（与 docs/74 §3.1 一致）。
local STAGE_X, STAGE_Y, STAGE_W, STAGE_H = 296, 76, 640, 568

-- 本模块自己拥有的动态 UI id（清理时只碰这些，绝不碰 lab_*）。
local CARD_ID = "ui_card"
local DOT_ID = "ui_dot"
local ICON_ID = "ui_icon"
local BADGE_ID = "ui_badge"
local PROBE_ID = "ui_probe"
local BAR_ID = "ui_bar"
local BAR_LABEL_ID = "ui_bar_label"
local GRID_ID = "ui_grid"
local GRID_INFO_ID = "ui_grid_info"
local TIP_ID = "ui_tip"
local TIP_INFO_ID = "ui_tip_info"
local MODAL_ID = "ui_modal"
local MODAL_BODY_ID = "ui_modal_body"
local FOCUS_ID = "ui_focus"
local FOCUS_INFO_ID = "ui_focus_info"
local SCROLL_ID = "ui_scroll"
local SCROLL_BG_ID = "ui_scroll_bg"
local SCROLL_INFO_ID = "ui_scroll_info"
local LAYOUT_ID = "ui_layout"
local LAYOUT_BOX_ID = "ui_layout_box"
local LAYOUT_INFO_ID = "ui_layout_info"
local CONTAINER_ID = "ui_container"
local CONTAINER_BOX_ID = "ui_container_box"
local CONTAINER_INFO_ID = "ui_container_info"
local THEME_BOX_ID = "ui_theme_box"
local THEME_INFO_ID = "ui_theme_info"
local SIGNAL_INFO_ID = "ui_signal_info"
local ADV_PANEL_ID = "ui_adv_panel"
local ADV_INFO_ID = "ui_adv_info"

-- 面板 / 图片 / 网格 / tooltip / modal：`ui.clear` 能删。
local DRAW_IDS = {
    CARD_ID, DOT_ID, ICON_ID, GRID_ID, TIP_ID, MODAL_ID,
    SCROLL_BG_ID, LAYOUT_BOX_ID, CONTAINER_BOX_ID, THEME_BOX_ID,
    ADV_PANEL_ID,
}
-- 普通动态文本：`ui.clear` 删不掉（只删 `<id>:tooltip` / `<id>:title`），必须 `ui.hide`。
local TEXT_IDS = {
    BADGE_ID, PROBE_ID, BAR_LABEL_ID, GRID_INFO_ID, TIP_INFO_ID,
    MODAL_BODY_ID, FOCUS_INFO_ID, SCROLL_INFO_ID, LAYOUT_INFO_ID,
    CONTAINER_INFO_ID, THEME_INFO_ID, SIGNAL_INFO_ID, ADV_INFO_ID,
    -- 高级内核步骤里的补充读数行（面板统一用 ADV_PANEL_ID）。
    "ui_rich_line_2", "ui_rich_line_3", "ui_rich_line_4", "ui_rich_line_5",
    "ui_input_line_2", "ui_input_line_3",
    "ui_nav_line_2", "ui_nav_line_3", "ui_nav_line_4", "ui_nav_line_5",
    "ui_control_line_2", "ui_control_line_3",
}

local GRID_COLUMNS, GRID_ROWS, GRID_CELL, GRID_GAP = 4, 3, 44, 10
local GRID_X, GRID_Y = 320, 336
local GRID_COUNT = GRID_COLUMNS * GRID_ROWS
local FOCUS_ITEMS, FOCUS_COLUMNS = 8, 4
local FOCUS_X, FOCUS_Y, FOCUS_W, FOCUS_H, FOCUS_GAP = 560, 336, 60, 34, 8
local SCROLL_VIEW = 200
local SCROLL_CONTENT = 600
local SCROLL_ROW_H = 20
local SCROLL_ROWS = SCROLL_CONTENT / SCROLL_ROW_H

local hp = 0.75
local selection = 1
local items = {}
local grid_ready = false
local tooltip_on = false
local modal_on = false
local probe_on = false
local focus_ready = false
local scroll_ready = false
local note = ""

-- 布局 / 容器 / 主题 / 信号的模型读数（接口存在时由真实调用覆盖）。
local layout_rect = { x = 560, y = 320, w = 160, h = 80 }
local container_state = { children = 3, first_y = 10, second_y = 48, third_y = 96 }
local theme_state = {
    active = "custom",
    bg = { 0.20, 0.10, 0.30, 1.0 },
    primary = { 0.80, 0.40, 1.00, 1.0 },
    count = 2,
    element = "my_light",
    font = 16,
}
local signal_state = { registered = 2, emitted = 1, after_off = 0, final = 0 }
local signal_element = "ui_demo_btn"

-- M76-07 的四个高级内核：虚拟列表 / 受限富文本 / 文本输入（IME）/ 导航栈 + 视图模型。
-- 读数一律来自引擎侧的返回值（`ui.*` 的直接结果），模型值只用于接口缺位时的兜底显示。
local VLIST_ID = "ui_vlist"
local VLIST_TOTAL, VLIST_VIEW, VLIST_ITEM_H, VLIST_OVERSCAN = 40, 120, 24, 1
local VLIST_ROWS_SHOWN = 12
local VLIST_TOP_Y = 120
local INPUT_ID = "ui_input"
local vlist_ready = false
local vlist_state = { start = 0, finish = 0, bound = 0, onscreen = 0, first_key = "-", slot = -1, offset = 0, height = 0 }
local rich_state = { plain = "Hi", lines = 1, spans = 1, unsupported = 0, bold = "-", color = "-", icon = "-" }
local input_state = { cursor = 0, text = "-", composition = "-", committed = "-", backend = "-", selection = "-" }
local nav_state = { depth = 1, current = "shop", modal = "-", accepts = true, back_kind = "-", hp = "-", name = "-", nested = "-" }

-- M80-04：值控件是**场景实体**（不是动态 ui.*）。宿主按组件值改写子实体矩形，
-- Lua 这一侧只读写组件值；场景在 `api_ui.scene` 底部放了一排真实控件。
local CONTROL_TOGGLE = "UiControlToggle"
local CONTROL_SLIDER = "UiControlSlider"
local CONTROL_PROGRESS = "UiControlProgress"
local CONTROL_SCROLLBAR = "UiControlScrollbar"
local control_state = { toggle = "-", slider = "-", progress = "-", scrollbar = "-", missing = "-" }

-- ---------------------------------------------------------------------------
-- 小工具
-- ---------------------------------------------------------------------------

local function clamp(value, low, high)
    return math.max(low, math.min(high, value))
end

--- 当前宿主是否注册了某个 ui 函数（布局/容器/主题/信号组可能缺位）。
local function has(name)
    return type(ui[name]) == "function"
end

--- 网格第 index 格（1 基）的左上角；与 `UiGrid::cell_rect` 同算法（行优先）。
local function cell_rect(index)
    local zero = index - 1
    local column = zero % GRID_COLUMNS
    local line = math.floor(zero / GRID_COLUMNS)
    return GRID_X + column * (GRID_CELL + GRID_GAP), GRID_Y + line * (GRID_CELL + GRID_GAP)
end

--- 清掉本模块上一屏的元素，让每个步骤都能从同一状态开始（用户按 F5 也全绿）。
local function reset_own()
    for _, id in ipairs(DRAW_IDS) do
        ui.clear(id)
    end
    for _, id in ipairs(TEXT_IDS) do
        ui.hide(id)
    end
    for index = 0, FOCUS_ITEMS - 1 do
        ui.hide("ui_focus_" .. index)
    end
    for row = 1, SCROLL_ROWS do
        ui.hide("ui_scroll_row_" .. row)
    end
    for row = 1, VLIST_ROWS_SHOWN do
        ui.hide("ui_vlist_row_" .. row)
    end
    -- 高级内核的状态是运行期表：逐个关掉，别把上一步的窗口/输入栈/视图模型留给下一步。
    if has("virtual_list") then
        ui.virtual_list({ op = "close", id = VLIST_ID })
        ui.virtual_list({ op = "close", id = "ui_vlist_empty" })
    end
    if has("text_input") then
        ui.text_input({ op = "close", id = INPUT_ID })
    end
    -- 收字声明是跨帧的意图（宿主每帧读它来决定开不开系统输入法），所以换屏时必须显式关掉，
    -- 否则上一步的字段会把玩法按键一直当正文吃掉。
    if type(input.text_events) == "function" then
        input.text_events({ capture = false })
    end
    if has("view_model_clear") then
        ui.view_model_clear()
    end
    nav_state.back_kind = "-"
    vlist_ready = false
    ui.drag_cancel()
    grid_ready = false
    tooltip_on = false
    modal_on = false
    probe_on = false
    focus_ready = false
    scroll_ready = false
    note = ""
end

local function reset_items()
    items = {}
    items[1] = { tex = "assets/textures/coin.png", name = "Coin", r = 1.00, g = 0.86, b = 0.35 }
    items[2] = { tex = "assets/textures/coin.png", name = "Coin", r = 1.00, g = 0.86, b = 0.35 }
    items[5] = { tex = "assets/textures/slime.png", name = "Slime", r = 0.65, g = 1.00, b = 0.65 }
    items[7] = { tex = "assets/textures/slime.png", name = "Slime", r = 1.00, g = 0.55, b = 0.55 }
    selection = 1
end

--- 用 Lua 模型重写整张网格；选中格最后写（后写的赢），高亮只能靠格子自己的颜色。
local function paint_grid()
    if not grid_ready then
        return
    end
    ui.grid_clear(GRID_ID)
    for index = 1, GRID_COUNT do
        local item = items[index]
        if item then
            ui.grid_cell(GRID_ID, index, item.tex, item.r, item.g, item.b, 1.0)
        end
    end
    local sel = items[selection]
    ui.grid_cell(GRID_ID, selection, sel and sel.tex or nil, 1.0, 0.82, 0.25, 1.0)
end

local function grid_status()
    local item = items[selection]
    lab.note(
        string.format(
            "sel=%d/%d %s  拖格子=ui.drag_begin/end  滚轮=ui.scroll_by",
            selection,
            GRID_COUNT,
            item and item.name or "(empty)"
        )
    )
end

local function focus_rect(index)
    local column = index % FOCUS_COLUMNS
    local line = math.floor(index / FOCUS_COLUMNS)
    return FOCUS_X + column * (FOCUS_W + FOCUS_GAP), FOCUS_Y + line * (FOCUS_H + FOCUS_GAP)
end

local function draw_focus_items()
    local current = ui.focus_index(FOCUS_ID)
    if type(current) == "number" then
        local x, y = focus_rect(current)
        ui.panel("ui_focus_pick", x - 4, y - 4, FOCUS_W + 8, FOCUS_H + 8, 0.95, 0.72, 0.25, 0.80)
    end
    for index = 0, FOCUS_ITEMS - 1 do
        local x, y = focus_rect(index)
        ui.text_at("ui_focus_" .. index, x + 10, y + 9, "item " .. (index + 1), 1.0, 12)
    end
end

--- scroll 行只画可见区间（`scroll_visible` 给内容像素区间），离开视口的行 hide 掉。
local function draw_scroll_rows()
    local visible = ui.scroll_visible(SCROLL_ID)
    local offset = ui.scroll_offset(SCROLL_ID) or 0
    local start_px = visible and visible.start or 0
    local stop_px = visible and visible.stop or 0
    local first = math.max(1, math.floor(start_px / SCROLL_ROW_H) + 1)
    local last = math.min(SCROLL_ROWS, math.ceil(stop_px / SCROLL_ROW_H))
    for row = 1, SCROLL_ROWS do
        if row < first or row > last then
            ui.hide("ui_scroll_row_" .. row)
        else
            local y = 118 + (row - 1) * SCROLL_ROW_H - offset
            ui.text_at("ui_scroll_row_" .. row, 572, y, string.format("row %02d", row), 1.0, 12)
        end
    end
    ui.text_at(
        SCROLL_INFO_ID,
        560,
        360,
        string.format("offset=%.0f max=%.0f visible=%.0f..%.0f", offset, ui.scroll_max(SCROLL_ID) or 0, start_px, stop_px),
        1.0,
        12
    )
end

-- ---------------------------------------------------------------------------
-- 高级内核（M76-07）的小工具
-- ---------------------------------------------------------------------------

--- 富文本演示用的原文：四种受支持标签 + 一个必须按字面量渲染的未知标签。
local RICH_SOURCE = "<b>Shop</b> <color=#ffcc00>+12</color><icon=coin><br/>next <script>alert(1)</script>"

--- 在 `ui.rich_text` 的 span 数组里找第一个 text 等于 want 的 span。
local function find_span(rich, want)
    for index = 1, (rich and #rich.spans or 0) do
        if rich.spans[index].text == want then
            return rich.spans[index]
        end
    end
    return nil
end

--- 找图标 span（`icon` 非空的段，它的 `text` 是空串）。
local function find_icon_span(rich)
    for index = 1, (rich and #rich.spans or 0) do
        local span = rich.spans[index]
        if span.icon then
            return span
        end
    end
    return nil
end

--- 把窗口内的行画成文本；overscan 行虽然绑定了槽位，但落在视口外，不画。
--- 返回（行数组, 绑定数, 视口内数）。
local function draw_vlist_rows()
    local rows = ui.virtual_list({ op = "bind", id = VLIST_ID })
    local bound = rows and #rows or 0
    local drawn = 0
    for position = 1, VLIST_ROWS_SHOWN do
        local row = rows and rows[position]
        local on_screen = row ~= nil and row.y >= 0 and row.y < VLIST_VIEW
        if on_screen then
            ui.text_at(
                "ui_vlist_row_" .. position,
                332,
                VLIST_TOP_Y + row.y + 4,
                string.format("item %02d  slot %d  y %+d", row.index + 1, row.slot, math.floor(row.y)),
                1.0,
                12
            )
            drawn = drawn + 1
        else
            ui.hide("ui_vlist_row_" .. position)
        end
    end
    return rows, bound, drawn
end

--- 重算窗口、绑定并画行；`enter` 与 `run` 都走这里，读数因此永远来自引擎。
local function refresh_vlist()
    local win = ui.virtual_list({ op = "window", id = VLIST_ID })
    if win then
        vlist_state.start = win.start
        vlist_state.finish = win["end"]
        vlist_state.offset = win.offset
        vlist_state.height = win.total_height
    end
    local rows, bound, drawn = draw_vlist_rows()
    vlist_state.bound = bound
    vlist_state.onscreen = drawn
    local first = rows and rows[1]
    vlist_state.first_key = first and first.key or "-"
    vlist_state.slot = first and first.slot or -1
end

--- 把 plain 里的换行显示成 `\n`，并截到面板一行放得下的长度。
local function fit_plain(text)
    local one = (tostring(text):gsub("\n", "\\n"))
    if #one > 30 then
        return one:sub(1, 30) .. ".."
    end
    return one
end

--- 把导航栈与视图模型的读数同步进 nav_state，并重画面板行（enter/run/check 都走这里）。
local function nav_sync()
    local st = ui.nav_stack({ op = "state" })
    if st then
        nav_state.depth = st.depth
        nav_state.current = tostring(st.current)
        nav_state.modal = st.modal == nil and "-" or tostring(st.modal)
        nav_state.accepts = st.accepts_input
    end
    nav_state.hp = tostring(ui.view_model("player.hp"))
    nav_state.name = tostring(ui.view_model("player.name"))
    nav_state.nested = tostring(ui.view_model("shop.items"))
    ui.text_at(
        ADV_INFO_ID,
        332,
        146,
        string.format("depth=%s current=%s modal=%s", nav_state.depth, nav_state.current, nav_state.modal),
        1.0,
        12
    )
    ui.text_at("ui_nav_line_2", 332, 166, string.format("accepts_input=%s", tostring(nav_state.accepts)), 1.0, 12)
    ui.text_at("ui_nav_line_3", 332, 186, string.format("last back=%s", nav_state.back_kind), 1.0, 12)
    ui.text_at("ui_nav_line_4", 332, 206, string.format("player.hp=%s", nav_state.hp), 1.0, 12)
    ui.text_at("ui_nav_line_5", 332, 226, string.format("shop.items=%s", nav_state.nested), 1.0, 12)
end

--- 值控件读数：`ui.<kind>(id)` 不传值就是读；实体没有对应组件时读返回 nil。
local function control_sync()
    local function num(value)
        return type(value) == "number" and string.format("%.2f", value) or "-"
    end
    local toggle = entity.find(CONTROL_TOGGLE)
    local slider = entity.find(CONTROL_SLIDER)
    local progress = entity.find(CONTROL_PROGRESS)
    local scrollbar = entity.find(CONTROL_SCROLLBAR)
    control_state.toggle = tostring(ui.toggle(toggle))
    control_state.slider = num(ui.slider(slider))
    control_state.progress = num(ui.progress(progress))
    control_state.scrollbar = num(ui.scrollbar(scrollbar))
    -- 没有 Toggle 的实体读作 nil：这是"静默"的一半；写的那一半会报错（见 check）。
    control_state.missing = string.format("Toolbar slider=%s", tostring(ui.slider(entity.find("Toolbar"))))
    ui.text_at(
        ADV_INFO_ID,
        332,
        146,
        string.format("toggle=%s slider=%s", control_state.toggle, control_state.slider),
        1.0,
        12
    )
    ui.text_at(
        "ui_control_line_2",
        332,
        166,
        string.format("progress=%s scrollbar=%s", control_state.progress, control_state.scrollbar),
        1.0,
        12
    )
    ui.text_at("ui_control_line_3", 332, 186, control_state.missing, 1.0, 12)
end

-- ---------------------------------------------------------------------------
-- 步骤
-- ---------------------------------------------------------------------------

local steps = {
    {
        id = "primitives",
        title = "Panel / image / text",
        claim = "面板、圆、图片、文本、进度条各占一层，读回值就是画出来的几何",
        api = { "ui.panel", "ui.circle", "ui.image", "ui.text", "ui.text_at", "ui.bar", "ui.get", "ui.panel_get", "ui.image_get", "ui.text_get", "ui.hide", "ui.show", "ui.clear" },
        code = "ui.panel(id,x,y,w,h,r,g,b,a); ui.image(id,...,tex,r,g,b)",
        enter = function()
            reset_own()
            hp = 0.75
            ui.panel(CARD_ID, 320, 110, 280, 140, 0.10, 0.13, 0.19, 1.0)
            -- 圆在内部按外接矩形存：panel_get 读回 (cx-r, cy-r, 2r, 2r)。
            ui.circle(DOT_ID, 590, 124, 8, 0.30, 0.85, 0.45, 1.0)
            ui.image(ICON_ID, 332, 170, 26, 26, "assets/textures/coin.png", 1.0, 0.86, 0.35)
            ui.text_at(BADGE_ID, 368, 174, "hp 75 / 100", 1.0, 14)
            -- `ui.text` 位置固定 (10,10)、scale 1：会压到页眉，所以建好立刻 hide。
            ui.text(PROBE_ID, "ui.text(id, text) -> 固定 (10,10)")
            ui.hide(PROBE_ID)
            ui.bar(BAR_ID, 320, 230, 280, 16, hp, 0.86, 0.30, 0.26)
            ui.text_at(BAR_LABEL_ID, 320, 252, string.format("ui.bar ratio=%.2f (c 波动)", hp), 1.0, 12)
        end,
        run = function()
            -- 条每帧重画：ratio 变了条就变；位置与尺寸在读数里核对。
            ui.bar(BAR_ID, 320, 230, 280, 16, hp, 0.86, 0.30, 0.26)
            lab.note("q hide / e show / x clear(图片) / t probe / c bar 波动")
        end,
        read = function()
            local panel = ui.panel_get(CARD_ID)
            local dot = ui.panel_get(DOT_ID)
            local icon = ui.image_get(ICON_ID)
            local badge = ui.text_get(BADGE_ID)
            local probe = ui.get(PROBE_ID)
            return {
                { label = "panel", value = panel and string.format("%.0fx%.0f @%.0f,%.0f", panel.width, panel.height, panel.x, panel.y) or "-" },
                { label = "circle", value = dot and string.format("%.0fx%.0f @%.0f,%.0f", dot.width, dot.height, dot.x, dot.y) or "-" },
                { label = "icon", value = icon and string.format("%.0f @%s", icon.width, icon.texture) or "-" },
                { label = "badge", value = badge and badge.text or "-" },
                { label = "probe", value = probe and tostring(probe.visible) or "nil" },
                { label = "bar", value = string.format("%.2f", hp) },
            }
        end,
        check = function(t)
            local panel = ui.panel_get(CARD_ID)
            t:eq(panel.x, 320, "panel x")
            t:eq(panel.width, 280, "panel width")
            local dot = ui.panel_get(DOT_ID)
            t:eq(dot.width, 16, "circle stored as its bounding box")
            t:eq(dot.x, 582, "circle bounding box starts at cx-r")
            t:ok(ui.image_get(ICON_ID).texture:find("coin.png") ~= nil, "image keeps its texture")
            ui.hide(BADGE_ID)
            t:eq(ui.text_get(BADGE_ID).visible, false, "hide covers text")
            ui.show(BADGE_ID)
            t:eq(ui.text_get(BADGE_ID).visible, true, "show brings it back")
            ui.clear(ICON_ID)
            t:eq(ui.image_get(ICON_ID), nil, "clear removes the image")
            ui.image(ICON_ID, 332, 170, 26, 26, "assets/textures/coin.png", 1.0, 0.86, 0.35)
            t:near(hp, 0.75, 1e-6, "bar ratio is the stored model value")
        end,
    },
    {
        id = "grid_cells",
        title = "Grid + drag",
        claim = "网格格子按 1 基序号写入，命中测试与拖拽状态都从引擎读回",
        api = { "ui.grid", "ui.grid_cell", "ui.grid_clear", "ui.grid_hit", "ui.drag_begin", "ui.drag_end", "ui.drag_cancel", "ui.drag_active" },
        code = "ui.grid(id,x,y,cols,rows,cell,gap); ui.grid_hit(id,x,y)",
        enter = function()
            reset_own()
            reset_items()
            ui.grid(GRID_ID, GRID_X, GRID_Y, GRID_COLUMNS, GRID_ROWS, GRID_CELL, GRID_GAP)
            grid_ready = true
            paint_grid()
        end,
        run = function(dt)
            if grid_ready then
                paint_grid()
            end
            grid_status()
            if dt > 0 then
                ui.grid_hit(GRID_ID, GRID_X + GRID_CELL / 2, GRID_Y + GRID_CELL / 2)
            end
        end,
        read = function()
            local hit = ui.grid_hit(GRID_ID, GRID_X + GRID_CELL / 2, GRID_Y + GRID_CELL / 2)
            local active = ui.drag_active()
            return {
                { label = "cells", value = string.format("%dx%d", GRID_COLUMNS, GRID_ROWS) },
                { label = "hit(1)", value = tostring(hit) },
                { label = "hit(miss)", value = tostring(ui.grid_hit(GRID_ID, GRID_X - 10, GRID_Y - 10)) },
                { label = "drag", value = active and string.format("%s:%s", tostring(active.id), tostring(active.index)) or "nil" },
                { label = "selection", value = selection },
            }
        end,
        check = function(t)
            local x, y = cell_rect(1)
            t:eq(ui.grid_hit(GRID_ID, x + GRID_CELL / 2, y + GRID_CELL / 2), 1, "first cell hit test")
            t:eq(ui.grid_hit(GRID_ID, GRID_X - 10, GRID_Y - 10), nil, "outside the grid is a miss")
            ui.grid_clear(GRID_ID)
            t:eq(ui.grid_hit(GRID_ID, x + 2, y + 2), 1, "hit test is geometry, not cell content")
            paint_grid()
            t:ok(ui.drag_begin(GRID_ID, 1), "drag_begin accepts a valid cell")
            local active = ui.drag_active()
            t:eq(active.id, GRID_ID, "drag_active reports the grid id")
            t:eq(active.index, 1, "drag_active reports the from index")
            t:ok(ui.drag_end(GRID_ID, 5), "drag_end returns true while a drag is active")
            t:eq(ui.drag_active(), nil, "drag_end clears the drag state")
            t:eq(ui.drag_end(GRID_ID, 5), false, "drag_end without a drag returns false")
            ui.drag_begin(GRID_ID, 2)
            ui.drag_cancel()
            t:eq(ui.drag_active(), nil, "drag_cancel drops the drag without a callback")
            selection = 1
            paint_grid()
        end,
    },
    {
        id = "tooltip_modal",
        title = "Tooltip + modal",
        claim = "tooltip 与 modal 各自是一块面板加派生文本，隐藏它们要成对处理",
        api = { "ui.tooltip", "ui.tooltip_hide", "ui.modal", "ui.modal_show", "ui.modal_hide", "ui.measure_text" },
        code = "ui.tooltip(id,x,y,text,w,h); ui.modal(id,x,y,w,h,title)",
        enter = function()
            reset_own()
            local measured = lab.try(ui.measure_text, "背包 - 金币 x12", 14)
            local width = 180
            if measured and measured.width then
                width = clamp(measured.width + 26, 140, 240)
            end
            ui.tooltip(TIP_ID, 320, 140, string.format("Coin x1 - %dpx", width), width, 38)
            tooltip_on = true
            ui.modal(MODAL_ID, 520, 400, 300, 140, "ui.modal - Discard item?")
            ui.text_at(MODAL_BODY_ID, 532, 438, "modal_show/hide 只关面板与 :title；正文是另一条文本", 1.0, 12)
            ui.modal_show(MODAL_ID)
            modal_on = true
            ui.text_at(TIP_INFO_ID, 320, 186, "t tooltip hide  m modal hide", 1.0, 12)
        end,
        run = function()
            lab.note(string.format("tooltip=%s modal=%s", tostring(tooltip_on), tostring(modal_on)))
            if modal_on then
                ui.modal_show(MODAL_ID)
            end
        end,
        read = function()
            local tip = ui.text_get(TIP_ID .. ":tooltip")
            local modal = ui.panel_get(MODAL_ID)
            return {
                { label = "tip", value = tip and tip.text or "-" },
                { label = "tip_visible", value = tip and tostring(tip.visible) or "-" },
                { label = "modal", value = modal and tostring(modal.visible) or "-" },
                { label = "modal_title", value = tostring((ui.text_get(MODAL_ID .. ":title") or {}).text) },
            }
        end,
        check = function(t)
            local tip = ui.text_get(TIP_ID .. ":tooltip")
            t:ok(tip and tip.text:find("Coin") ~= nil, "tooltip draws its derived :tooltip text")
            t:ok(ui.panel_get(TIP_ID).width >= 140, "tooltip width comes from measure_text (with a safe floor)")
            t:eq(ui.panel_get(MODAL_ID).visible, true, "modal_show makes the panel visible")
            ui.modal_hide(MODAL_ID)
            t:eq(ui.panel_get(MODAL_ID).visible, false, "modal_hide hides the panel")
            t:eq(ui.text_get(MODAL_ID .. ":title").visible, false, "modal_hide also hides :title")
            ui.modal_show(MODAL_ID)
            ui.tooltip_hide(TIP_ID)
            t:eq(ui.text_get(TIP_ID .. ":tooltip").visible, false, "tooltip_hide hides :tooltip")
            t:eq(ui.panel_get(TIP_ID).visible, false, "tooltip_hide hides its panel")
            ui.clear(TIP_ID)
            t:eq(ui.text_get(TIP_ID .. ":tooltip"), nil, "clear removes the derived tooltip text")
        end,
    },
    {
        id = "focus_ring",
        title = "Focus ring",
        claim = "焦点序号 0 基；方向键在 4 列布局里左右移动，Tab 循环，close 释放",
        api = { "ui.focus_open", "ui.focus_select", "ui.focus_index", "ui.focus_count", "ui.focus_move", "ui.focus_cycle", "ui.focus_close" },
        code = "ui.focus_open(id,count,cols); ui.focus_move(id,'right')",
        enter = function()
            reset_own()
            ui.focus_open(FOCUS_ID, FOCUS_ITEMS, FOCUS_COLUMNS)
            ui.focus_select(FOCUS_ID, 0)
            focus_ready = true
        end,
        run = function()
            draw_focus_items()
            ui.text_at(
                FOCUS_INFO_ID,
                560,
                430,
                string.format("index=%s count=%d (Tab 循环)", tostring(ui.focus_index(FOCUS_ID)), ui.focus_count(FOCUS_ID)),
                1.0,
                12
            )
            lab.note("ui.focus_move 用字母键替代方向键：焦点由 Tab / 数字键以外的按键驱动")
        end,
        read = function()
            return {
                { label = "count", value = ui.focus_count(FOCUS_ID) },
                { label = "index", value = tostring(ui.focus_index(FOCUS_ID)) },
                { label = "item0", value = "item 1" },
            }
        end,
        check = function(t)
            t:eq(ui.focus_count(FOCUS_ID), FOCUS_ITEMS, "focus ring keeps all items")
            t:eq(ui.focus_index(FOCUS_ID), 0, "enter selects the first item")
            t:eq(ui.focus_cycle(FOCUS_ID, true), 1, "cycle forward lands on index 1")
            t:eq(ui.focus_move(FOCUS_ID, "right"), 2, "right moves within the 4-column row")
            t:eq(ui.focus_move(FOCUS_ID, "left"), 1, "left moves back one column")
            t:ok(ui.focus_select(FOCUS_ID, FOCUS_ITEMS - 1), "select accepts the last index")
            t:eq(ui.focus_index(FOCUS_ID), FOCUS_ITEMS - 1, "index reads back the selection")
            t:ok(ui.focus_close(FOCUS_ID), "focus_close removes the ring")
            t:eq(ui.focus_count(FOCUS_ID), 0, "closed ring has no items")
            ui.focus_open(FOCUS_ID, FOCUS_ITEMS, FOCUS_COLUMNS)
            ui.focus_select(FOCUS_ID, 0)
            focus_ready = true
        end,
    },
    {
        id = "scroll_list",
        title = "Scroll state",
        claim = "视口 200、内容 600：偏移被钳在 0..400，翻页与滚入视口都是最小滚动",
        api = { "ui.scroll_open", "ui.scroll_extents", "ui.scroll_offset", "ui.scroll_max", "ui.scroll_by", "ui.scroll_lines", "ui.scroll_page", "ui.scroll_to_visible", "ui.scroll_visible", "ui.scroll_close" },
        code = "ui.scroll_open(id,200,600); ui.scroll_by(id,90)",
        enter = function()
            reset_own()
            ui.scroll_open(SCROLL_ID, SCROLL_VIEW, SCROLL_CONTENT)
            scroll_ready = true
            ui.panel(SCROLL_BG_ID, 560, 108, 280, 240, 0.085, 0.105, 0.145, 1.0)
        end,
        run = function()
            ui.scroll_extents(SCROLL_ID, SCROLL_VIEW, SCROLL_CONTENT)
            draw_scroll_rows()
            lab.note("滚轮=ui.scroll_by  s=向前滚一行")
        end,
        read = function()
            local visible = ui.scroll_visible(SCROLL_ID)
            return {
                { label = "offset", value = string.format("%.0f", ui.scroll_offset(SCROLL_ID) or 0) },
                { label = "max", value = string.format("%.0f", ui.scroll_max(SCROLL_ID) or 0) },
                { label = "visible", value = visible and string.format("%.0f..%.0f", visible.start, visible.stop) or "-" },
            }
        end,
        check = function(t)
            t:eq(ui.scroll_max(SCROLL_ID), 400, "max offset = content - viewport")
            t:eq(ui.scroll_by(SCROLL_ID, 90), 90, "scroll_by returns the applied delta")
            t:eq(ui.scroll_lines(SCROLL_ID, 1, SCROLL_ROW_H), SCROLL_ROW_H, "scroll_lines applies one row")
            t:eq(ui.scroll_offset(SCROLL_ID), 110, "offset accumulates")
            t:eq(ui.scroll_page(SCROLL_ID, 1), 200, "page step equals one viewport")
            t:eq(ui.scroll_offset(SCROLL_ID), 310, "offset after paging")
            t:ok(ui.scroll_to_visible(SCROLL_ID, 0, SCROLL_ROW_H), "an item above the viewport scrolls in")
            t:eq(ui.scroll_offset(SCROLL_ID), 0, "scroll_to_visible uses the minimal scroll")
            local visible = ui.scroll_visible(SCROLL_ID)
            t:eq(visible.start, 0, "visible range starts at the offset")
            t:eq(visible.stop, SCROLL_VIEW, "visible range ends one viewport later")
            t:ok(ui.scroll_close(SCROLL_ID), "scroll_close removes the state")
            t:eq(ui.scroll_offset(SCROLL_ID), nil, "closed scroll has no offset")
            ui.scroll_open(SCROLL_ID, SCROLL_VIEW, SCROLL_CONTENT)
            scroll_ready = true
        end,
    },
    {
        id = "layout_anchors",
        title = "Layout anchors",
        claim = "锚点 0.5 + 偏移 -80,-40,80,40 在 1280×720 上解出 160×80 的居中矩形",
        api = { "ui.layout", "ui.resolve_layout", "ui.get_layout", "ui.set_layout", "ui.set_preset" },
        code = "ui.layout(id,{anchors={.5,.5,.5,.5},offsets={-80,-40,80,40}})",
        enter = function()
            reset_own()
            -- 同一套公式的 Lua 模型：中心锚点 + 对称偏移。
            layout_rect = {
                x = 1280 * 0.5 - 80,
                y = 720 * 0.5 - 40,
                w = 80 + 80,
                h = 40 + 40,
            }
            if has("layout") then
                ui.layout(LAYOUT_ID, {
                    anchors = { 0.5, 0.5, 0.5, 0.5 },
                    offsets = { -80, -40, 80, 40 },
                    min_size = { 80, 40 },
                    max_size = { 240, 120 },
                    pivot = { 0.5, 0.5 },
                })
                local resolved = ui.resolve_layout(LAYOUT_ID, 1280, 720)
                if resolved and resolved.width then
                    layout_rect = { x = resolved.x, y = resolved.y, w = resolved.width, h = resolved.height }
                end
                local spec = ui.get_layout(LAYOUT_ID)
                if spec and spec.anchors and spec.anchors[1] then
                    layout_rect.anchor_x = spec.anchors[1]
                end
                ui.set_layout(LAYOUT_ID, { anchors = { 0, 0, 1, 1 } })
                ui.set_preset(LAYOUT_ID, "center")
            end
            ui.panel(LAYOUT_BOX_ID, layout_rect.x, layout_rect.y, layout_rect.w, layout_rect.h, 0.16, 0.34, 0.52, 0.85)
            ui.text_at(
                LAYOUT_INFO_ID,
                layout_rect.x + 8,
                layout_rect.y + 8,
                string.format("%.0fx%.0f", layout_rect.w, layout_rect.h),
                1.0,
                12
            )
        end,
        run = function()
            ui.panel(LAYOUT_BOX_ID, layout_rect.x, layout_rect.y, layout_rect.w, layout_rect.h, 0.16, 0.34, 0.52, 0.85)
            lab.note(has("layout") and "ui.layout 已注册：读数来自 resolve_layout" or "宿主未注册 ui.layout：读数用同一公式的模型")
        end,
        read = function()
            return {
                { label = "x,y", value = string.format("%.0f,%.0f", layout_rect.x, layout_rect.y) },
                { label = "w,h", value = string.format("%.0f,%.0f", layout_rect.w, layout_rect.h) },
                { label = "source", value = has("layout") and "resolve_layout" or "model" },
            }
        end,
        check = function(t)
            t:eq(layout_rect.x, 560, "center anchor puts the left edge at 560")
            t:eq(layout_rect.y, 320, "center anchor puts the top edge at 320")
            t:eq(layout_rect.w, 160, "offset span gives the width")
            t:eq(layout_rect.h, 80, "offset span gives the height")
            local panel = ui.panel_get(LAYOUT_BOX_ID)
            t:eq(panel.width, 160, "the visual box mirrors the resolved rect")
        end,
    },
    {
        id = "container_flow",
        title = "Container flow",
        claim = "vbox 按 padding 10、spacing 8 累加高度；hbox 改为横向排布，clear 后子项归零",
        api = { "ui.container", "ui.container_add", "ui.container_get", "ui.container_children", "ui.container_update_child", "ui.container_set_style", "ui.container_remove", "ui.container_clear", "ui.container_resolve" },
        code = "ui.container(id,{kind='vbox'}); ui.container_add(id,'child1',{size={100,30}})",
        enter = function()
            reset_own()
            -- 模型：padding 10、spacing 8、三个孩子 100x30 / 120x40 / 80x25。
            container_state = { children = 3, first_y = 10, second_y = 48, third_y = 96 }
            if has("container") then
                ui.container(CONTAINER_ID, {
                    kind = "vbox",
                    padding = { 10, 10, 10, 10 },
                    spacing = 8,
                    stretch = false,
                    fill = false,
                    align = "start",
                })
                ui.container_add(CONTAINER_ID, "child1", { size = { 100, 30 }, expand = 0 })
                ui.container_add(CONTAINER_ID, "child2", { size = { 120, 40 }, expand = 0 })
                ui.container_add(CONTAINER_ID, "child3", { size = { 80, 25 }, expand = 0 })
                local resolved = ui.container_resolve(CONTAINER_ID, 300, 200)
                local info = ui.container_get(CONTAINER_ID)
                local children = ui.container_children(CONTAINER_ID)
                container_state.children = info and info.child_count or (children and #children) or 3
                if resolved and resolved.child1 then
                    container_state.first_y = resolved.child1.y
                    container_state.second_y = resolved.child2.y
                    container_state.third_y = resolved.child3.y
                end
                ui.container_update_child(CONTAINER_ID, "child2", { size = { 150, 40 } })
                ui.container_set_style(CONTAINER_ID, { spacing = 10, kind = "hbox" })
                ui.container_remove(CONTAINER_ID, "child3")
                ui.container_clear(CONTAINER_ID)
                local after = ui.container_get(CONTAINER_ID)
                container_state.after_clear = after and after.child_count or 0
            else
                container_state.after_clear = 0
            end
            ui.panel(CONTAINER_BOX_ID, 320, 380, 300, 160, 0.10, 0.14, 0.20, 0.90)
            for index = 1, 3 do
                local y = 380 + ({ 10, 48, 96 })[index]
                ui.panel("ui_container_child_" .. index, 330, y, index == 2 and 120 or 100, 25, 0.26, 0.52, 0.62, 0.90)
            end
            ui.text_at(CONTAINER_INFO_ID, 320, 550, "vbox: y=10 / 48 / 96", 1.0, 12)
        end,
        run = function()
            lab.note(has("container") and "ui.container 已注册：enter 里已经跑完 add/resolve/update/remove/clear" or "宿主未注册 ui.container：读数用 padding+spacing 的模型")
        end,
        read = function()
            return {
                { label = "children", value = container_state.children },
                { label = "y1,y2", value = string.format("%.0f,%.0f", container_state.first_y, container_state.second_y) },
                { label = "y3", value = string.format("%.0f", container_state.third_y) },
                { label = "after_clear", value = tostring(container_state.after_clear or 0) },
                { label = "source", value = has("container") and "container_resolve" or "model" },
            }
        end,
        check = function(t)
            t:eq(container_state.children, 3, "three children were added")
            t:eq(container_state.first_y, 10, "first child sits at padding top")
            t:eq(container_state.second_y, 48, "second child starts after 30 + 8")
            t:eq(container_state.third_y, 96, "third child starts after 40 + 8")
            t:eq(container_state.after_clear or 0, 0, "container_clear leaves no children")
        end,
    },
    {
        id = "theme_signals",
        title = "Theme + signals",
        claim = "主题改的是配色读数，信号表记的是绑定/派发/解绑的次数，两者都能读回",
        api = { "ui.theme", "ui.theme_preset", "ui.set_theme", "ui.get_theme", "ui.theme_color", "ui.theme_get", "ui.theme_list", "ui.apply_theme", "ui.element_theme", "ui.clear_theme", "ui.on", "ui.off", "ui.emit", "ui.signals", "ui.handlers", "ui.clear_signals" },
        code = "ui.set_theme('custom'); ui.on(el,'click',fn); ui.emit(el,'click')",
        enter = function()
            reset_own()
            theme_state = {
                active = "custom",
                bg = { 0.20, 0.10, 0.30, 1.0 },
                primary = { 0.80, 0.40, 1.00, 1.0 },
                count = 2,
                element = "my_light",
                font = 16,
            }
            signal_state = { registered = 2, emitted = 1, after_off = 0, final = 0 }
            if has("theme_preset") then
                ui.theme_preset("my_dark", "dark")
                ui.theme_preset("my_light", "light")
                ui.theme("custom", {
                    background = { 0.20, 0.10, 0.30, 1.0 },
                    foreground = { 1.00, 0.90, 0.80, 1.0 },
                    primary = { 0.80, 0.40, 1.00, 1.0 },
                    font_size = 16,
                    spacing = 12,
                })
                ui.set_theme("custom")
                theme_state.active = ui.get_theme() or theme_state.active
                local bg = ui.theme_color("background")
                if bg then
                    theme_state.bg = bg
                end
                local primary = ui.theme_color("primary")
                if primary then
                    theme_state.primary = primary
                end
                local themes = ui.theme_list()
                if themes then
                    theme_state.count = #themes
                end
                local data = ui.theme_get("custom")
                if data and data.font_size then
                    theme_state.font = data.font_size
                end
                ui.apply_theme("ui_demo_panel", "my_light")
                theme_state.element = ui.element_theme("ui_demo_panel") or theme_state.element
                ui.clear_theme("ui_demo_panel")
            end
            if has("on") then
                local first = ui.on(signal_element, "click", function()
                    signal_state.emitted = (signal_state.emitted or 0) + 1
                end)
                ui.on(signal_element, "hover", function() end)
                local count = ui.emit(signal_element, "click", "hello")
                if count then
                    signal_state.emitted = count
                end
                local signals = ui.signals(signal_element)
                signal_state.registered = signals and #signals or 2
                ui.handlers(signal_element)
                if first then
                    ui.off(signal_element, "click", first)
                else
                    ui.off(signal_element, "click", 1)
                end
                local after = ui.emit(signal_element, "click", "after_off")
                signal_state.after_off = after or 0
                ui.clear_signals(signal_element)
                local final = ui.signals(signal_element)
                signal_state.final = final and #final or 0
            end
            ui.panel(THEME_BOX_ID, 640, 150, 180, 90, theme_state.bg[1], theme_state.bg[2], theme_state.bg[3], 1.0)
            ui.text_at(
                THEME_INFO_ID,
                640,
                252,
                string.format("bg=(%.2f,%.2f,%.2f) primary=(%.2f,%.2f)", theme_state.bg[1], theme_state.bg[2], theme_state.bg[3], theme_state.primary[1], theme_state.primary[2]),
                1.0,
                12
            )
            ui.text_at(
                SIGNAL_INFO_ID,
                640,
                274,
                string.format("signals=%d emit=%d final=%d", signal_state.registered, signal_state.emitted, signal_state.final),
                1.0,
                12
            )
        end,
        run = function()
            lab.note(has("set_theme") and "ui.set_theme/ui.on 已注册：读数来自引擎" or "宿主未注册主题/信号表：读数用本地模型，调用仍保留在 enter 里")
        end,
        read = function()
            return {
                { label = "theme", value = theme_state.active },
                { label = "bg", value = string.format("%.2f,%.2f,%.2f", theme_state.bg[1], theme_state.bg[2], theme_state.bg[3]) },
                { label = "themes", value = theme_state.count },
                { label = "element", value = theme_state.element },
                { label = "signals", value = signal_state.registered },
                { label = "emit", value = signal_state.emitted },
                { label = "final", value = signal_state.final },
            }
        end,
        check = function(t)
            t:near(theme_state.bg[1], 0.20, 0.001, "theme background red channel")
            t:near(theme_state.bg[3], 0.30, 0.001, "theme background blue channel")
            t:near(theme_state.primary[1], 0.80, 0.001, "theme primary red channel")
            t:eq(theme_state.count, 2, "two presets were registered")
            t:eq(theme_state.element, "my_light", "element theme reads back")
            t:eq(signal_state.registered, 2, "two handlers were bound")
            t:eq(signal_state.emitted, 1, "one handler ran per emit")
            t:eq(signal_state.final, 0, "clear_signals leaves no handlers")
        end,
    },
    {
        id = "virtual_list",
        title = "Virtual list",
        claim = "40 项只绑定窗口里的 7 项：overscan 多绑一行，槽位随滚出滚入复用",
        api = { "ui.virtual_list" },
        code = 'ui.virtual_list{op="open",total=40,viewport=120,item_height=24,overscan=1} / bind / scroll_to',
        enter = function()
            reset_own()
            ui.virtual_list({
                op = "open",
                id = VLIST_ID,
                total = VLIST_TOTAL,
                viewport = VLIST_VIEW,
                item_height = VLIST_ITEM_H,
                spacing = 0,
                overscan = VLIST_OVERSCAN,
            })
            vlist_ready = true
            ui.panel(ADV_PANEL_ID, 320, VLIST_TOP_Y - 12, 280, VLIST_VIEW + 24, 0.085, 0.105, 0.145, 1.0)
            refresh_vlist()
        end,
        run = function()
            refresh_vlist()
            lab.note("滚轮=ui.virtual_list scroll（每格一行）  窗口外的行已 hide，槽位仍占用")
        end,
        read = function()
            return {
                { label = "window", value = string.format("%d..%d", vlist_state.start, vlist_state.finish) },
                { label = "offset", value = string.format("%.0f / %.0f", vlist_state.offset, vlist_state.height - VLIST_VIEW) },
                { label = "bound", value = string.format("%d / %d 项", vlist_state.bound, VLIST_TOTAL) },
                { label = "onscreen", value = vlist_state.onscreen },
                { label = "first", value = string.format("key %s slot %d", vlist_state.first_key, vlist_state.slot) },
                { label = "total_h", value = string.format("%.0f", vlist_state.height) },
            }
        end,
        check = function(t)
            local win = ui.virtual_list({ op = "window", id = VLIST_ID })
            t:eq(win.total_height, VLIST_ITEM_H * VLIST_TOTAL, "total height is item_height times total")
            t:eq(win.start, 0, "the window starts at the top")
            t:eq(win["end"], 7, "viewport 120 over 24 gives 5 rows, plus 1 and plus 1 overscan")
            local rows = ui.virtual_list({ op = "bind", id = VLIST_ID })
            t:eq(#rows, 7, "only the window is bound")
            t:eq(rows[1].key, "0", "the first key is the first item")
            t:eq(rows[1].slot, 0, "a fresh pool hands out slot zero first")
            t:eq(rows[7].index, 6, "index is the item number, not the slot")
            t:eq(rows[7].content_y, 144, "content_y is stride times index")
            t:eq(rows[7].height, VLIST_ITEM_H, "every row is one item_height tall")
            -- 滚到底：请求 9999，引擎只允许走到 max_offset = 960 - 120。
            t:near(ui.virtual_list({ op = "scroll", id = VLIST_ID, delta = 9999 }), 840, 1e-3, "scroll is clamped to the bottom")
            local bottom = ui.virtual_list({ op = "window", id = VLIST_ID })
            t:eq(bottom.start, 34, "overscan pulls one extra row into the window")
            t:eq(bottom["end"], 40, "the window stops at the total")
            -- 跳到第 21 项：它在当前视口之上，最小滚动把它的顶边贴住视口上沿。
            t:ok(ui.virtual_list({ op = "scroll_to", id = VLIST_ID, index = 20 }), "scroll_to moves an item that is out of view")
            local mid = ui.virtual_list({ op = "window", id = VLIST_ID })
            t:near(mid.offset, 480, 1e-3, "an item above the viewport scrolls to the top edge")
            local rebound = ui.virtual_list({ op = "bind", id = VLIST_ID })
            t:eq(#rebound, 8, "the window is eight items wide at this offset")
            t:eq(rebound[1].key, "19", "the first bound key is the overscan row")
            t:eq(rebound[1].slot, 0, "released slots are reused from the front")
            t:eq(rebound[8].slot, 7, "the eighth key grows the pool by exactly one")
            t:near(rebound[1].y, -24, 1e-3, "the overscan row above the viewport is bound but off-screen")
            -- 已经完整可见的项不再滚动。
            t:eq(ui.virtual_list({ op = "scroll_to", id = VLIST_ID, index = 20 }), false, "a visible item does not move the list")
            -- 往下跳：第 31 项的底边 744 超出视口下沿 600，最小滚动对齐到 744-120。
            t:ok(ui.virtual_list({ op = "scroll_to", id = VLIST_ID, index = 30 }), "scroll_to also moves for an item below the viewport")
            t:near((ui.virtual_list({ op = "window", id = VLIST_ID })).offset, 624, 1e-3, "an item below the viewport scrolls to the bottom edge")
            local kept = ui.virtual_list({ op = "bind", id = VLIST_ID })
            t:eq(kept[1].key, "25", "the new window starts one overscan row earlier")
            t:eq(kept[1].slot, 6, "a key that stayed in the window keeps its slot")
            t:eq(kept[2].slot, 7, "the neighbour keeps its slot too")
            ui.virtual_list({ op = "scroll_to", id = VLIST_ID, index = 0 })
            refresh_vlist()
            -- 边界：视口为 0 的列表没有任何可见项。
            ui.virtual_list({ op = "open", id = "ui_vlist_empty", total = 10, viewport = 0, item_height = 24 })
            t:eq((ui.virtual_list({ op = "window", id = "ui_vlist_empty" }))["end"], 0, "a zero viewport binds nothing")
            t:eq(ui.virtual_list({ op = "close", id = "ui_vlist_empty" }), true, "close reports that the list existed")
            t:eq(ui.virtual_list({ op = "window", id = "ui_vlist_empty" }), nil, "a closed list has no window")
        end,
    },
    {
        id = "rich_text",
        title = "Rich text (limited)",
        claim = "只认 b / color / icon / br，其余尖括号按字面量渲染：外部文本改不了排版",
        api = { "ui.rich_text" },
        code = 'ui.rich_text("<b>x</b><color=#rrggbb>y</color><icon=name><br/>")',
        enter = function()
            reset_own()
            local rich = ui.rich_text(RICH_SOURCE)
            rich_state.plain = rich and rich.plain or ""
            rich_state.lines = rich and rich.lines or 1
            rich_state.spans = rich and #rich.spans or 0
            rich_state.unsupported = rich and #rich.unsupported or 0
            local shop = rich and find_span(rich, "Shop")
            rich_state.bold = shop and tostring(shop.bold) .. " color=" .. tostring(shop.color) or "-"
            local plus = rich and find_span(rich, "+12")
            rich_state.color = plus and tostring(plus.color) or "-"
            local icon = rich and find_icon_span(rich)
            rich_state.icon = icon and string.format("%s text=%q", tostring(icon.icon), icon.text) or "-"
            ui.panel(ADV_PANEL_ID, 320, 120, 280, 120, 0.085, 0.105, 0.145, 1.0)
            -- 当前宿主的富文本仍按纯文本排版绘制（分段样式只在读数里可见）。
            ui.text_at(ADV_INFO_ID, 332, 136, fit_plain(rich_state.plain), 1.0, 12)
            ui.text_at("ui_rich_line_2", 332, 156, string.format("spans=%d lines=%d", rich_state.spans, rich_state.lines), 1.0, 12)
            ui.text_at("ui_rich_line_3", 332, 176, string.format("unsupported=%d bold=%s", rich_state.unsupported, rich_state.bold), 1.0, 12)
            ui.text_at("ui_rich_line_4", 332, 196, string.format("color=%s", rich_state.color), 1.0, 12)
            ui.text_at("ui_rich_line_5", 332, 216, string.format("icon=%s", rich_state.icon), 1.0, 12)
        end,
        run = function()
            lab.note("ui.rich_text 是纯解析：不建行、不占状态，绘制仍走宿主文本排版")
        end,
        read = function()
            return {
                { label = "spans", value = rich_state.spans },
                { label = "lines", value = rich_state.lines },
                { label = "unsupported", value = rich_state.unsupported },
                { label = "bold span", value = rich_state.bold },
                { label = "color span", value = rich_state.color },
                { label = "icon span", value = rich_state.icon },
                { label = "plain", value = fit_plain(rich_state.plain) },
            }
        end,
        check = function(t)
            local rich = ui.rich_text(RICH_SOURCE)
            t:eq(rich.plain, "Shop +12\nnext <script>alert(1)</script>", "known tags are stripped, unknown tags stay literal")
            t:eq(rich.lines, 2, "one br means two lines")
            t:eq(#rich.spans, 6, "two styled spans, a space, the icon, the break and the literal tail")
            t:eq(#rich.unsupported, 2, "the unknown element reports both its open and close tag")
            local shop = find_span(rich, "Shop")
            t:eq(shop.bold, true, "the b span carries bold")
            t:eq(shop.color, nil, "bold does not invent a colour")
            local plus = find_span(rich, "+12")
            t:eq(plus.color, "#ffcc00", "colour is normalised to rrggbb hex")
            t:eq(plus.bold, false, "bold was closed before the colour span")
            local icon = find_icon_span(rich)
            t:eq(icon.icon, "coin", "the icon span names the icon")
            t:eq(icon.text, "", "an icon span has no text of its own")
            t:eq(rich.spans[6].text, "next <script>alert(1)</script>", "the unknown tags are one literal span")
            t:ok(rich.plain:find("<b>") == nil, "no supported tag survives into the plain text")
            -- 认识但没闭合的标签：标签本身被吃掉，样式一路作用到结尾（不报错、不吞正文）。
            local half = ui.rich_text("<b>open")
            t:eq(half.plain, "open", "an unclosed known tag still strips its own marker")
            t:eq(#half.spans, 1, "the half input still parses as one span")
            t:eq(half.spans[1].bold, true, "the style runs to the end of the string")
            -- 连 `>` 都没有的截断标签：整段按字面量留着，避免半截输入吃掉文案。
            local broken = ui.rich_text("<color=#ff0000")
            t:eq(broken.plain, "<color=#ff0000", "a tag without a closing bracket is kept literally")
            t:eq(#broken.unsupported, 0, "it is not reported as an unknown tag")
            t:eq(broken.lines, 1, "the truncated input does not invent a line break")
        end,
    },
    {
        id = "text_input",
        title = "Text input + IME",
        claim = "宿主 IME 事件进 input.text_events；组合串只进 composition，提交才落到文本",
        api = { "ui.text_input", "input.text_events" },
        code = 'input.text_events{composition="ni"}; ui.text_input{op="composition",action="commit"}',
        enter = function()
            reset_own()
            local cursor = ui.text_input({ op = "open", id = INPUT_ID, text = "Ni " })
            input_state.cursor = cursor or 0
            local st = ui.text_input({ op = "state", id = INPUT_ID })
            input_state.text = st and st.text or "-"
            input_state.selection = st and string.format("%q", st.selection) or "-"
            local desktop = ui.text_input({ op = "policy", host = "desktop" })
            input_state.backend = desktop and desktop.backend or "-"
            ui.panel(ADV_PANEL_ID, 320, 130, 280, 84, 0.085, 0.105, 0.145, 1.0)
            ui.text_at(ADV_INFO_ID, 332, 146, string.format("text=%q cursor=%d", input_state.text, input_state.cursor), 1.0, 12)
            ui.text_at("ui_input_line_2", 332, 166, string.format("backend=%s", input_state.backend), 1.0, 12)
            ui.text_at("ui_input_line_3", 332, 186, "i: begin/update/commit 组合串", 1.0, 12)
        end,
        run = function()
            local st = ui.text_input({ op = "state", id = INPUT_ID })
            if st then
                input_state.text = st.text
                input_state.cursor = st.cursor
                input_state.composition = tostring(st.composition)
                input_state.selection = string.format("%q", st.selection)
            end
            ui.text_at(ADV_INFO_ID, 332, 146, string.format("text=%q cursor=%d", input_state.text, input_state.cursor), 1.0, 12)
            ui.text_at("ui_input_line_2", 332, 166, string.format("composition=%s sel=%s", input_state.composition, input_state.selection), 1.0, 12)
            lab.note("i: 走一遍组合串  宿主事件走 input.text_events（这里用注入，真机由 IME 写）")
        end,
        read = function()
            return {
                { label = "text", value = input_state.text },
                { label = "cursor", value = input_state.cursor },
                { label = "selection", value = input_state.selection },
                { label = "composition", value = input_state.composition },
                { label = "committed", value = input_state.committed },
                { label = "backend", value = input_state.backend },
            }
        end,
        check = function(t)
            t:eq(input_state.cursor, 3, "open focuses and leaves the caret after the text")
            t:eq(input_state.backend, "native_composition", "desktop gets the native composition backend")
            local begin = ui.text_input({ op = "composition", id = INPUT_ID, action = "begin", value = "ni" })
            t:eq(begin.composition, "ni", "composition begins with the first keystrokes")
            t:eq(begin.text, "Ni ", "pre-edit text is not inserted yet")
            local update = ui.text_input({ op = "composition", id = INPUT_ID, action = "update", value = "nihao" })
            t:eq(update.composition, "nihao", "update replaces the whole composition")
            t:eq(update.committed, nil, "update commits nothing")
            local commit = ui.text_input({ op = "composition", id = INPUT_ID, action = "commit" })
            t:eq(commit.committed, "nihao", "commit hands the final string back")
            t:eq(commit.text, "Ni nihao", "commit inserts at the caret")
            t:eq(commit.cursor, 8, "the caret lands after the committed text")
            t:eq(commit.composition, nil, "commit clears the composition")
            t:eq(commit.focused, true, "commit does not steal focus")
            -- 取消：预编辑消失，正文一个字都不动。
            ui.text_input({ op = "composition", id = INPUT_ID, action = "begin", value = "zz" })
            local canceled = ui.text_input({ op = "composition", id = INPUT_ID, action = "cancel" })
            t:eq(canceled.composition, nil, "cancel drops the composition")
            t:eq(canceled.text, "Ni nihao", "cancel never touched the text")
            -- 编辑动作按字符索引，不是字节。
            ui.text_input({ op = "edit", id = INPUT_ID, action = "home" })
            local right = ui.text_input({ op = "edit", id = INPUT_ID, action = "right" })
            t:eq(right.cursor, 1, "right moves one char")
            local del = ui.text_input({ op = "edit", id = INPUT_ID, action = "delete" })
            t:eq(del.text, "N nihao", "delete removes the char under the caret")
            local all = ui.text_input({ op = "edit", id = INPUT_ID, action = "select_all" })
            t:eq(all.selection, "N nihao", "select_all covers the whole text")
            local typed = ui.text_input({ op = "edit", id = INPUT_ID, action = "insert", value = "N" })
            t:eq(typed.text, "N", "typing replaces the selection")
            t:eq(typed.cursor, 1, "the caret lands after the typed char")
            -- 中文按字符数走：两个汉字是两个索引，不是六个字节。
            ui.text_input({ op = "edit", id = INPUT_ID, action = "insert", value = "你好" })
            local cjk = ui.text_input({ op = "edit", id = INPUT_ID, action = "backspace" })
            t:eq(cjk.text, "N你", "backspace removes one whole char")
            t:eq(cjk.cursor, 2, "the caret counts chars, not bytes")
            local blurred = ui.text_input({ op = "edit", id = INPUT_ID, action = "blur" })
            t:eq(blurred.focused, false, "blur clears focus")
            -- 宿主策略：微信没有原生输入桥就是只读。
            local wechat = ui.text_input({ op = "policy", host = "wechat", bridge = true })
            t:eq(wechat.backend, "wechat_native_bridge", "wechat with the bridge uses the host input box")
            t:eq(wechat.supports_selection, false, "the native bridge gives no in-engine selection")
            t:eq(ui.text_input({ op = "policy", host = "wechat", bridge = false }).backend, "read_only", "no bridge means read-only")
            t:eq(ui.text_input({ op = "policy", host = "web" }).backend, "browser_composition", "the web host composes in the browser")
            -- 边界：id 不存在时静默返回 nil，既不报错也不造状态。
            t:eq(ui.text_input({ op = "edit", id = "ui_input_ghost", action = "home" }), nil, "an unknown id edits nothing")
            t:eq(ui.text_input({ op = "state", id = "ui_input_ghost" }), nil, "an unknown id has no state")
            -- ---- M79-04：宿主文本通道 ----
            -- PC 宿主把 winit 的 IME/键盘事件写进输入快照，脚本从 input.text_events 读；
            -- 这里注入的就是宿主那一帧写进来的同一个形状，编排与真机一致。
            local cap = input.text_events({ capture = true })
            t:eq(cap.capturing, true, "the script can ask the host to hand over the IME")
            local pre = input.text_events({ composition = "ni" })
            t:eq(pre.composition, "ni", "the host preedit string is readable in the same frame")
            ui.text_input({ op = "open", id = INPUT_ID, text = "" })
            ui.text_input({ op = "composition", id = INPUT_ID, action = "begin", value = pre.composition })
            -- winit 在 Commit 前一定先发一条空 Preedit：空串是「清掉组合串」，不是「没有组合串」。
            local done = input.text_events({ composition = "", commit = "你" })
            t:eq(done.composition, nil, "an empty composition clears the preedit")
            t:eq(#done.committed, 1, "one commit arrived this frame")
            t:eq(done.committed[1], "你", "the commit carries the final text")
            ui.text_input({ op = "composition", id = INPUT_ID, action = "update", value = done.committed[1] })
            local landed = ui.text_input({ op = "composition", id = INPUT_ID, action = "commit" })
            t:eq(landed.text, "你", "the host commit ends up as field text")
            local again = input.text_events()
            -- 声明是跨帧意图、文本是本帧数据：同一帧里再读一次，两者都还在——清掉它们的是帧边界。
            t:eq(again.capturing, true, "the capture declaration survives repeated reads in the frame")
            t:eq(#again.committed, 1, "the frame's commits stay readable until the frame ends")
            input.text_events({ capture = false })
            -- 收尾：摆回 enter 的样子，F5 之后读数一致。
            ui.text_input({ op = "close", id = INPUT_ID })
            input_state.cursor = ui.text_input({ op = "open", id = INPUT_ID, text = "Ni " })
            input_state.text = "Ni "
            input_state.composition = "nil"
        end,
    },
    {
        id = "nav_view_model",
        title = "Nav stack + view model",
        claim = "返回键先关模态、再弹页、根页不弹；视图模型是扁平键，点号路径不会展开成树",
        api = { "ui.nav_stack", "ui.view_model", "ui.view_model_set", "ui.view_model_clear" },
        code = 'ui.nav_stack{op="push",page="bag",restore_focus=7}; ui.view_model_set("player.hp",42)',
        enter = function()
            reset_own()
            nav_state.depth = ui.nav_stack({ op = "open", root = "shop" }) or 1
            local bag = ui.nav_stack({ op = "push", page = "bag", restore_focus = 7 })
            ui.nav_stack({ op = "modal", id = "confirm", restore_focus = 11 })
            nav_state.depth = bag and bag.depth or 2
            ui.view_model_set("player.hp", 42)
            ui.view_model_set("player.name", "Rei")
            ui.view_model_set("player.alive", true)
            ui.view_model_set("shop.items.count", 3)
            ui.panel(ADV_PANEL_ID, 320, 130, 280, 130, 0.085, 0.105, 0.145, 1.0)
            nav_sync()
        end,
        run = function()
            nav_sync()
            lab.note("n: 走一遍 back（模态 -> 页 -> 根）  视图模型见右侧读数")
        end,
        read = function()
            return {
                { label = "depth", value = nav_state.depth },
                { label = "current", value = nav_state.current },
                { label = "modal", value = nav_state.modal },
                { label = "accepts", value = tostring(nav_state.accepts) },
                { label = "last back", value = nav_state.back_kind },
                { label = "player.hp", value = nav_state.hp },
                { label = "shop.items", value = nav_state.nested },
            }
        end,
        check = function(t)
            local state = ui.nav_stack({ op = "state" })
            t:eq(state.depth, 2, "one root plus one pushed page")
            t:eq(state.current, "bag", "the top page is the pushed one")
            t:eq(state.modal, "confirm", "the modal sits above the page stack")
            t:eq(state.accepts_input, false, "a modal swallows input meant for the page")
            local closed = ui.nav_stack({ op = "back" })
            t:eq(closed.kind, "modal", "back closes the modal first")
            t:eq(closed.id, "confirm", "back reports which modal it closed")
            t:eq(closed.focus, 11, "back hands the modal restore focus back to the caller")
            local after = ui.nav_stack({ op = "state" })
            t:eq(after.modal, nil, "no modal is left")
            t:eq(after.accepts_input, true, "the page accepts input again")
            local popped = ui.nav_stack({ op = "back" })
            t:eq(popped.kind, "page", "the next back pops the page")
            t:eq(popped.page, "bag", "back reports the popped page")
            t:eq(popped.focus, 7, "the page carries its own restore focus")
            local root = ui.nav_stack({ op = "back" })
            t:eq(root.kind, "root", "the root page is never popped")
            t:eq((ui.nav_stack({ op = "state" })).depth, 1, "depth stays at one")
            local raised = not pcall(ui.nav_stack, { op = "close_modal", id = "ghost" })
            t:ok(raised, "closing a modal that is not open raises instead of editing the stack")
            -- 视图模型：值原样读回，点号路径只是一个扁平键。
            t:near(ui.view_model("player.hp"), 42, 1e-6, "numbers come back")
            t:eq(ui.view_model("player.name"), "Rei", "strings come back")
            t:eq(ui.view_model("player.alive"), true, "booleans come back")
            t:eq(ui.view_model("shop.items"), nil, "a dotted path does not grow a tree")
            t:eq(ui.view_model("shop.items.count"), 3, "the full dotted key reads back")
            ui.view_model_clear("player.name")
            t:eq(ui.view_model("player.name"), nil, "clear with a path removes that one key")
            t:eq(ui.view_model("player.alive"), true, "the other keys survive")
            ui.view_model_clear()
            t:eq(ui.view_model("player.hp"), nil, "clear without a path empties the model")
            -- 收尾：恢复 enter 的栈与视图模型，下一帧的读数仍然完整。
            ui.nav_stack({ op = "open", root = "shop" })
            ui.nav_stack({ op = "push", page = "bag", restore_focus = 7 })
            ui.nav_stack({ op = "modal", id = "confirm", restore_focus = 11 })
            ui.view_model_set("player.hp", 42)
            ui.view_model_set("player.name", "Rei")
            nav_sync()
        end,
    },
    {
        id = "value_controls",
        title = "Toggle / Slider / Progress / Scrollbar",
        claim = "值控件是场景实体：Lua 读写组件值，宿主按值改写子实体矩形（勾选/填充/滑块/手柄）",
        api = { "ui.toggle", "ui.slider", "ui.progress", "ui.scrollbar" },
        code = "ui.slider(id); ui.slider(id, 0.25); ui.toggle(id, false); ui.progress(id); ui.scrollbar(id)",
        enter = function()
            reset_own()
            local toggle = entity.find(CONTROL_TOGGLE)
            local slider = entity.find(CONTROL_SLIDER)
            local progress = entity.find(CONTROL_PROGRESS)
            local scrollbar = entity.find(CONTROL_SCROLLBAR)
            -- F5 / 上一步都可能改过值：先摆回场景的初始值，读数才可重复。
            ui.toggle(toggle, true)
            ui.slider(slider, 0.25)
            ui.progress(progress, 0.5)
            ui.scrollbar(scrollbar, 0.3)
            ui.panel(ADV_PANEL_ID, 320, 130, 280, 130, 0.085, 0.105, 0.145, 1.0)
            control_sync()
        end,
        run = function()
            control_sync()
            lab.note("舞台底部一排：点 Toggle / 拖 Slider / 拖 Scrollbar，值写进组件、下一帧改写子实体矩形")
        end,
        read = function()
            return {
                { label = "toggle", value = control_state.toggle },
                { label = "slider", value = control_state.slider },
                { label = "progress", value = control_state.progress },
                { label = "scrollbar", value = control_state.scrollbar },
                { label = "no component", value = control_state.missing },
            }
        end,
        check = function(t)
            local toggle = entity.find(CONTROL_TOGGLE)
            local slider = entity.find(CONTROL_SLIDER)
            local progress = entity.find(CONTROL_PROGRESS)
            local scrollbar = entity.find(CONTROL_SCROLLBAR)
            -- 读：不传值返回组件现值（enter 已摆回场景初始值）。
            t:eq(ui.toggle(toggle), true, "toggle reads the authored on state")
            t:near(ui.slider(slider), 0.25, 1e-6, "slider reads the authored value")
            t:near(ui.progress(progress), 0.5, 1e-6, "progress reads the authored value")
            t:near(ui.scrollbar(scrollbar), 0.3, 1e-6, "scrollbar reads the authored value")
            -- 写：返回**写完之后**的值，读路径与写路径同一份存储。
            t:eq(ui.toggle(toggle, false), false, "writing toggle returns the new state")
            t:eq(ui.toggle(toggle), false, "the read path agrees with the write")
            t:eq(ui.toggle(toggle, true), true, "toggle can be switched back on")
            -- 夹取：越界值被夹到 [min,max]，返回值就是夹取后的真值（不是入参）。
            t:near(ui.slider(slider, 5.0), 1.0, 1e-6, "slider clamps above max")
            t:near(ui.slider(slider, -1.0), 0.0, 1e-6, "slider clamps below min")
            t:near(ui.progress(progress, 1.5), 1.0, 1e-6, "progress clamps above max")
            t:near(ui.scrollbar(scrollbar, -0.5), 0.0, 1e-6, "scrollbar clamps to zero")
            -- 没有组件的实体：读是 nil（静默），写要报错——"什么都没发生"是脚本最难查的错。
            local plain = entity.find("Toolbar")
            t:eq(ui.slider(plain), nil, "an entity without Slider reads nil")
            t:ok(not pcall(ui.slider, plain, 0.5), "writing a value to an entity without Slider raises")
            -- 恢复 enter 的读数，F5 之后 check 仍然成立。
            ui.toggle(toggle, true)
            ui.slider(slider, 0.25)
            ui.progress(progress, 0.5)
            ui.scrollbar(scrollbar, 0.3)
            control_sync()
        end,
    },
}

-- ---------------------------------------------------------------------------
-- 输入：只处理字母键与鼠标；Lab 的导航键一个都不拦
-- ---------------------------------------------------------------------------

local function handle_input()
    local cursor = input.cursor_position()
    local cx, cy = cursor[1] or 0, cursor[2] or 0
    local step = lab.step()
    local id = step and step.id or ""

    if input.just_pressed("q") then
        ui.hide(CARD_ID)
        note = "q: ui.hide(panel+same-id text)"
    elseif input.just_pressed("e") then
        ui.panel(CARD_ID, 320, 110, 280, 140, 0.10, 0.13, 0.19, 1.0)
        ui.show(BADGE_ID)
        note = "e: ui.panel redraw + ui.show(text)"
    elseif input.just_pressed("x") then
        ui.clear(ICON_ID)
        note = "x: ui.clear(image) -> image_get nil"
    elseif input.just_pressed("t") then
        probe_on = not probe_on
        if probe_on then
            ui.show(PROBE_ID)
        else
            ui.hide(PROBE_ID)
        end
        note = probe_on and "t: ui.text probe shown at (10,10)" or "t: probe hidden"
    elseif input.just_pressed("c") then
        hp = hp - 0.25
        if hp < 0 then
            hp = 1.0
        end
        note = string.format("c: ui.bar ratio=%.2f", hp)
    elseif input.just_pressed("g") and grid_ready then
        selection = selection % GRID_COUNT + 1
        paint_grid()
        note = string.format("g: selection=%d", selection)
    elseif input.just_pressed("d") and grid_ready then
        ui.drag_begin(GRID_ID, selection)
        local target = selection % GRID_COUNT + 1
        ui.drag_end(GRID_ID, target)
        note = string.format("d: ui.drag_begin/end %d->%d", selection, target)
    elseif input.just_pressed("m") then
        if modal_on then
            ui.modal_hide(MODAL_ID)
            ui.hide(MODAL_BODY_ID)
            modal_on = false
        else
            ui.modal_show(MODAL_ID)
            ui.show(MODAL_BODY_ID)
            modal_on = true
        end
        note = "m: ui.modal_show / modal_hide"
    elseif input.just_pressed("s") and scroll_ready then
        note = string.format("s: ui.scroll_lines(+1)=%.0f", ui.scroll_lines(SCROLL_ID, 1, SCROLL_ROW_H))
    elseif input.just_pressed("l") then
        note = "l: layout 步骤的解算见右侧读数"
    elseif input.just_pressed("v") then
        note = "v: theme 步骤的读数来自 get_theme/theme_color"
    elseif input.just_pressed("p") then
        note = "p: signals 步骤的读数来自 signals/handlers"
    elseif input.just_pressed("i") then
        -- 走一遍 IME 生命周期：预编辑不落到正文，提交才落。
        ui.text_input({ op = "composition", id = INPUT_ID, action = "begin", value = "ni" })
        ui.text_input({ op = "composition", id = INPUT_ID, action = "update", value = "你好" })
        local commit = ui.text_input({ op = "composition", id = INPUT_ID, action = "commit" })
        input_state.committed = commit and tostring(commit.committed) or "-"
        local text = commit and commit.text or "-"
        if commit then
            input_state.text = text
            input_state.cursor = commit.cursor
        end
        note = string.format("i: composition commit %q -> text %q", input_state.committed, tostring(text))
    elseif input.just_pressed("n") then
        local back = ui.nav_stack({ op = "back" })
        if back then
            nav_state.back_kind = string.format("%s/%s", tostring(back.kind), tostring(back.id or back.page or "-"))
        end
        note = string.format("n: ui.nav_stack back -> %s", nav_state.back_kind)
    end

    -- 滚轮：y 为正 = 向上滚，传给 scroll_by / virtual_list 要取反；偏移会被引擎钳到 0..max。
    local wheel = input.scroll_delta()
    local dy = wheel and wheel[2] or 0
    if dy ~= 0 and vlist_ready then
        local applied = ui.virtual_list({ op = "scroll", id = VLIST_ID, delta = -dy * VLIST_ITEM_H })
        note = string.format("wheel %.1f -> ui.virtual_list scroll %.0f (applied %.0f)", dy, -dy * VLIST_ITEM_H, applied or 0)
    elseif dy ~= 0 and scroll_ready then
        note = string.format("wheel %.1f -> ui.scroll_by(%.1f)=%.0f", dy, -dy * SCROLL_ROW_H, ui.scroll_by(SCROLL_ID, -dy * SCROLL_ROW_H))
    end

    -- 网格上的鼠标拖拽（headless 下 mouse_* 恒 false，不会触发）。
    if grid_ready then
        if input.mouse_just_pressed("left") then
            local hit = ui.grid_hit(GRID_ID, cx, cy)
            if type(hit) == "number" then
                selection = hit
                paint_grid()
                ui.drag_begin(GRID_ID, hit)
                note = string.format("mouse down cell %d -> ui.drag_begin", hit)
            end
        elseif input.mouse_just_released("left") then
            if ui.drag_active() then
                local hit = ui.grid_hit(GRID_ID, cx, cy)
                ui.drag_end(GRID_ID, hit or selection)
                note = string.format("mouse up -> ui.drag_end(%d)", hit or selection)
            end
        end
    end
    if id == "" then
        note = "lab inactive"
    end
end

-- ---------------------------------------------------------------------------
-- 生命周期
-- ---------------------------------------------------------------------------

function M.on_create()
    lab.start({
        scene = "api_ui",
        title = "Runtime UI",
        category = "UI",
        steps = steps,
        hide_scene_text = { "Toolbar", "Hud" },
    })
end

function M.update()
    if not lab.active() then
        return "ui_inactive"
    end
    handle_input()
    if note ~= "" then
        lab.note(note)
    end
    return lab.tick()
end

function M.self_test()
    return lab.self_test()
end

--- `ui.drag_end` 触发的全局回调：把拖放落成模型变更，再让 grid_cell 重画。
function M.on_drag(from_id, from_index, to_id, to_index)
    local target = tonumber(to_index) or 0
    if from_id == GRID_ID and to_id == GRID_ID and target >= 1 and target <= GRID_COUNT then
        local from = tonumber(from_index) or 0
        if from >= 1 and from <= GRID_COUNT then
            items[from], items[target] = items[target], items[from]
            selection = target
            paint_grid()
            note = string.format("on_drag %d->%d", from, target)
            return true
        end
    end
    return false
end

function M.on_destroy()
    ui.drag_cancel()
    ui.focus_close(FOCUS_ID)
    ui.scroll_close(SCROLL_ID)
    -- 主题/信号表可能未注册：清理也要守卫，否则离开场景会报错。
    if has("clear_signals") then
        ui.clear_signals(signal_element)
    end
    if has("clear_theme") then
        ui.clear_theme("ui_demo_panel")
    end
    reset_own()
    ui.scene_text_visible("Toolbar", true)
    ui.scene_text_visible("Hud", true)
    lab.stop()
end

return M
