-- index：入口菜单。三列卡片 + 右侧**运行期 API 索引**，演示：
--   * `ui.focus_*` 焦点环与方向键/Tab 导航；
--   * `UiEvents` 点击分发（卡片实体名就是目标场景名）；
--   * `_G` 枚举：API 表清单不再手写，运行期有多少模块表就显示多少。
--
-- 卡片是**场景实体**（`scenes/index.scene` 由 `scripts/docgen/gen-api-showcase-index.ps1`
-- 生成）。动态 UI 画在场景 Image 之上、场景 Text 之下，因此这里的页眉/API 面板不会盖住
-- 卡片文字，也不会盖住卡片图——三者各在自己的层里。

local M = {}

local entries = require("demos")
-- 卡片网格的唯一真源（生成器读同一份）：焦点环因此不可能和卡片错位。
local grid = require("menu_grid")
local COLUMNS = grid.columns

-- 右侧 API 面板的排版：42 个模块表按两列排，行高与字号一起算过（21 行 × 20 px
-- 从 y=176 铺到 576，面板底在 610）。字号再大就得减行数，减行数会让新模块没地方放。
local LIST_FONT = 14
local LIST_ROW_H = 20
local ROWS_PER_COLUMN = 21
local LIST_COLUMNS = 2

local STDLIB = {
    string = true, table = true, math = true, io = true, os = true, coroutine = true,
    debug = true, utf8 = true, package = true, _G = true, bit32 = true, arg = true,
}

local focus = 0
local opened = 0
local api_tables = {}
local api_functions = 0

--- 运行期枚举 `_G` 里的 API 表（与 `LUA_API_PROBE` 同一条判据），
--- 顺手统计函数总数——菜单不再维护任何"41 个模块"之类的手写数字。
local function scan_api_surface()
    local tables = {}
    local total = 0
    for key, value in pairs(_G) do
        if type(value) == "table" and not STDLIB[key] then
            local count = 0
            for _, inner in pairs(value) do
                if type(inner) == "function" then
                    count = count + 1
                end
            end
            if count > 0 then
                tables[#tables + 1] = { name = tostring(key), count = count }
                total = total + count
            end
        end
    end
    table.sort(tables, function(a, b)
        return a.name < b.name
    end)
    return tables, total
end

local function card_rect(index)
    local column = index % COLUMNS
    local row = math.floor(index / COLUMNS)
    return grid.left + column * grid.col_w, grid.top + row * grid.row_h
end

function M.on_create()
    focus = 0
    opened = 0
    api_tables, api_functions = scan_api_surface()

    -- 焦点环：`columns=3` 让方向键按三列网格移动（序号 0 基）。
    ui.focus_open("menu", #entries, COLUMNS)
    ui.focus_select("menu", 0)

    log.info(
        string.format(
            "index: %d demos, runtime API %d tables / %d functions",
            #entries,
            #api_tables,
            api_functions
        )
    )
end

local function open_entry(entry, how)
    if not entry then
        return false
    end
    opened = opened + 1
    scene.load(entry.scene)
    log.info(string.format("index: %s opens %s (%s)", how, entry.scene, entry.module))
    return true
end

function M.update()
    if input.action_repeat("ui_down") then
        focus = ui.focus_move("menu", "down") or focus
    elseif input.action_repeat("ui_up") then
        focus = ui.focus_move("menu", "up") or focus
    elseif input.action_repeat("ui_left") then
        focus = ui.focus_move("menu", "left") or focus
    elseif input.action_repeat("ui_right") then
        focus = ui.focus_move("menu", "right") or focus
    end
    if input.just_pressed("tab") then
        focus = ui.focus_cycle("menu", not input.is_down("shiftleft")) or focus
    end
    if input.action_pressed("confirm") then
        if open_entry(entries[focus + 1], "confirm") then
            return "index_open scene=" .. entries[focus + 1].scene
        end
    end

    -- 页眉条与右侧 API 面板：动态面板在场景图之上、场景文字之下。
    ui.panel("menu_header", 0, 0, 1280, 96, 0.070, 0.085, 0.120, 0.96)
    ui.panel("menu_header_line", 0, 94, 1280, 2, 0.30, 0.55, 0.90, 1.0)
    ui.panel("menu_api_panel", 936, grid.top, 328, 510, 0.085, 0.100, 0.140, 0.96)
    ui.panel("menu_footer", 0, 610, 1280, 110, 0.060, 0.075, 0.105, 0.96)

    local left, top = card_rect(focus)
    -- 焦点高亮：半透明覆盖在卡片图上；卡片文字是场景 Text，仍画在它上面。
    ui.panel("menu_focus", left - 4, top - 4, grid.card_w + 8, grid.card_h + 8, 0.22, 0.42, 0.72, 0.34)

    ui.text_at("menu_api_title", 952, 112, "RUNTIME API SURFACE", 1.0, 18)
    ui.text_at(
        "menu_api_count",
        952,
        142,
        string.format("%d tables / %d functions - from _G", #api_tables, api_functions),
        1.0,
        14
    )
    for i, table_info in ipairs(api_tables) do
        local column = math.floor((i - 1) / ROWS_PER_COLUMN)
        local row = (i - 1) % ROWS_PER_COLUMN
        local x = 952 + column * 156
        local y = 176 + row * LIST_ROW_H
        ui.text_at(
            "menu_api_" .. i,
            x,
            y,
            string.format("%s %d", table_info.name, table_info.count),
            1.0,
            LIST_FONT
        )
    end
    for i = #api_tables + 1, LIST_COLUMNS * ROWS_PER_COLUMN do
        ui.clear("menu_api_" .. i)
    end

    ui.text_at(
        "menu_status",
        48,
        620,
        string.format(
            "focus %d/%d - %s - opened %d - %s",
            (ui.focus_index("menu") or 0) + 1,
            ui.focus_count("menu") or 0,
            entries[focus + 1] and entries[focus + 1].scene or "?",
            opened,
            M.KEYS_BACK
        ),
        1.0,
        15
    )
    return string.format("index_ok focus=%d tables=%d", focus, #api_tables)
end

M.KEYS_BACK = "ESC / pad B returns to the menu"

function M.on_click(name)
    for index, entry in ipairs(entries) do
        if entry.scene == name then
            ui.focus_select("menu", index - 1)
            return open_entry(entry, "click")
        end
    end
    log.warn("index: unknown button " .. tostring(name))
    return false
end

function M.on_destroy()
    ui.focus_close("menu")
    for _, id in ipairs({
        "menu_header", "menu_header_line", "menu_api_panel", "menu_footer",
        "menu_focus", "menu_api_title", "menu_api_count", "menu_status",
    }) do
        ui.hide(id)
        ui.clear(id)
    end
    for i = 1, LIST_COLUMNS * ROWS_PER_COLUMN do
        ui.hide("menu_api_" .. i)
        ui.clear("menu_api_" .. i)
    end
end

return M
