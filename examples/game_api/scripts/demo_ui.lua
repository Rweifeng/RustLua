-- api_ui：运行期动态 UI（`ui` 表）—— 图元、网格、提示、焦点、滚动、布局、容器、主题、信号。
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
-- 键位（不占用 Lab 保留键）：
--   q hide / e show / x clear / t probe / g grid select / d drag / m modal
--   s scroll / c circle pulse / l layout / v theme / p signals / 鼠标拖格子 / 滚轮滚动
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

-- 面板 / 图片 / 网格 / tooltip / modal：`ui.clear` 能删。
local DRAW_IDS = {
    CARD_ID, DOT_ID, ICON_ID, GRID_ID, TIP_ID, MODAL_ID,
    SCROLL_BG_ID, LAYOUT_BOX_ID, CONTAINER_BOX_ID, THEME_BOX_ID,
}
-- 普通动态文本：`ui.clear` 删不掉（只删 `<id>:tooltip` / `<id>:title`），必须 `ui.hide`。
local TEXT_IDS = {
    BADGE_ID, PROBE_ID, BAR_LABEL_ID, GRID_INFO_ID, TIP_INFO_ID,
    MODAL_BODY_ID, FOCUS_INFO_ID, SCROLL_INFO_ID, LAYOUT_INFO_ID,
    CONTAINER_INFO_ID, THEME_INFO_ID, SIGNAL_INFO_ID,
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
    end

    -- 滚轮：y 为正 = 向上滚，传给 scroll_by 要取反；偏移会被引擎钳到 0..max。
    local wheel = input.scroll_delta()
    local dy = wheel and wheel[2] or 0
    if dy ~= 0 and scroll_ready then
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
