-- index：入口菜单。演示 `ui` 的**焦点环**（`ui.focus_*`）与**滚动**（`ui.scroll_*`）、
-- 读回接口（`ui.panel_get` / `ui.text_get` / `ui.measure_text`）以及命名动作导航。
--
-- 场景侧的按钮（`scenes/index.scene` 的 13 个实体）演示 `UiEvents` 点击分发：
-- 实体名就是目标场景名，点击回调因此可以直接 `scene.load(name)`。
local api = require("api_demo")

local M = {}

local entries = require("demos")
local COLUMNS = 2
-- 与 `scripts/gen-api-showcase-index.ps1` 生成的卡片网格一致（卡片 560×50、行距 58、首行 y=148）。
local ROW_H = 58
local LIST_TOP = 148
local CARD_W = 560
local CARD_H = 50
local VIEWPORT_H = 330

local focus = 0
local opened = 0
local last_scroll = 0

-- 41 个 API 表名（`ui.scroll_*` 演示用的"内容比视口高"的虚拟列表）。
local api_tables = {
    "animation", "animation3d", "app", "asset", "audio", "calendar", "camera",
    "camera2d", "camera3d", "clock", "container", "data", "day", "entity",
    "event", "grid", "health", "input", "interact", "inv", "item", "light",
    "log", "map", "mesh", "particles", "physics", "physics3d", "quad", "save",
    "scene", "shop", "sprite", "tag", "tilemap", "time", "transform",
    "transform3d", "ui", "visibility", "world_text",
}

local function entry_at(index)
    return entries[index + 1]
end

--- 焦点项的屏幕位置：2 列网格，与 `scenes/index.scene` 的按钮布局一致。
local function focus_rect(index)
    local column = index % COLUMNS
    local row = math.floor(index / COLUMNS)
    local left = 120 + column * 580
    local top = LIST_TOP + row * ROW_H
    return left, top
end

function M.on_create()
    api.title("RustLua API Showcase")
    api.hint(
        "Arrows/Tab move focus - Enter/Space or click opens - wheel scrolls the API list - "
            .. api.KEYS_BACK
    )
    -- 背景用**动态面板**而不是场景实体：场景里的 Image/Text 永远画在动态 UI 之上，
    -- 铺满屏幕的场景背景会把下面的取景框与卡片高亮整片盖住（面板在同批里按 id 字典序排，
    -- 所以 "menu_backdrop" 一定排在 "menu_focus" 之前）。
    ui.panel("menu_backdrop", 0, 0, 1280, 720, 0.070, 0.085, 0.120, 1.0)
    -- 焦点环：15 项、2 列布局（序号 0 基）。
    ui.focus_open("menu", #entries, COLUMNS)
    ui.focus_select("menu", 0)
    -- 滚动状态：内容（41 个 API 表 × 22px）比视口高，因此会出现真实的最大偏移。
    ui.scroll_open("api_list", VIEWPORT_H, #api_tables * 22)
    log.info("index: menu ready, use arrows or the mouse, ESC returns here")
end

function M.update()
    -- 菜单导航：`action_repeat` 是"菜单导航专用"的边沿+连发语义，
    -- 键盘方向键与手柄 DPad/摇杆共用同一条动作名。
    if input.action_repeat("ui_down") then
        focus = ui.focus_move("menu", "down") or focus
    elseif input.action_repeat("ui_up") then
        focus = ui.focus_move("menu", "up") or focus
    elseif input.action_repeat("ui_left") then
        focus = ui.focus_move("menu", "left") or focus
    elseif input.action_repeat("ui_right") then
        focus = ui.focus_move("menu", "right") or focus
    end
    -- TAB 循环（Shift+TAB 反向）：`focus_cycle` 用**物理键**查询，命名动作只覆盖方向键。
    if input.just_pressed("tab") then
        focus = ui.focus_cycle("menu", not input.is_down("shiftleft")) or focus
    end

    if input.action_pressed("confirm") then
        local entry = entry_at(focus)
        if entry then
            opened = opened + 1
            scene.load(entry.scene)
            return string.format("index_open scene=%s", entry.scene)
        end
    end

    -- 滚轮滚动虚拟 API 列表：`input.scroll_delta()` 返回数组（`[1]`=x、`[2]`=y），
    -- `scroll_by` 返回**实际生效**的增量（已按内容高度钳制，不会滚出空白）。
    local wheel = input.scroll_delta()
    local wheel_y = wheel and wheel[2] or 0
    if wheel_y ~= 0 then
        local applied = ui.scroll_by("api_list", wheel_y * 40)
        last_scroll = applied or 0
    end
    ui.scroll_extents("api_list", VIEWPORT_H, #api_tables * 22)

    -- 当前焦点项的取景框：位置由焦点序号算出来，尺寸与卡片一致。
    local left, top = focus_rect(focus)
    ui.panel("menu_focus", left - 6, top - 6, CARD_W + 12, CARD_H + 12, 0.16, 0.30, 0.46, 0.80)
    ui.image("menu_icon", left + 522, top + 12, 24, 24, "assets/textures/coin.png", 1, 1, 1)

    -- 读回：面板与文本的真实矩形（`panel_get` / `text_get` / `image_get`）。
    local panel = ui.panel_get("menu_focus")
    local entry = entry_at(focus)
    local label = entry and entry.label or "(no entry)"
    local measured = ui.measure_text(label, 14)
    local drawn = ui.text_get("api_status")

    -- 虚拟列表：只画可见区间（`scroll_visible` 给出 `{start, stop}`，字段名避开 `end`）。
    local offset = ui.scroll_offset("api_list") or 0
    local visible = ui.scroll_visible("api_list")
    local first = math.floor(offset / 22) + 1
    local last = math.min(#api_tables, first + math.floor(VIEWPORT_H / 22) - 1)
    -- 上一帧画过的条目先清掉，否则滚出去的旧 id 会留在屏幕上（`ui.clear` 同时删面板/图片/网格）。
    for i = 1, #api_tables do
        ui.clear("api_table_" .. i)
    end
    ui.panel("api_list_panel", 820, LIST_TOP, 380, VIEWPORT_H, 0.10, 0.12, 0.16, 0.92)
    for i = first, last do
        local y = LIST_TOP + 6 + (i - 1) * 22 - offset
        ui.text_at("api_table_" .. i, 834, y, api_tables[i], 1.0)
    end
    ui.text_at(
        "api_list_info",
        820,
        LIST_TOP + VIEWPORT_H + 6,
        string.format(
            "visible %s..%s of %d - offset %.0f / max %.0f - wheel %.0f",
            tostring(visible and visible.start or 0),
            tostring(visible and visible.stop or 0),
            #api_tables,
            offset,
            ui.scroll_max("api_list") or 0,
            wheel_y
        ),
        1.0
    )

    -- 焦点序号同时用 `focus_count` / `focus_index` 读回（0 基）。
    api.status(
        string.format(
            "focus %d/%d - %s - label %.0fx%.0f px - card %sx%s - last wheel %.0f - opened %d - status_text %s",
            (ui.focus_index("menu") or 0) + 1,
            ui.focus_count("menu") or 0,
            entry and entry.scene or "?",
            measured and measured.width or 0,
            measured and measured.height or 0,
            panel and string.format("%.0f", panel.width) or "?",
            panel and string.format("%.0f", panel.height) or "?",
            last_scroll,
            opened,
            drawn and drawn.text or "(none)"
        )
    )
    return string.format("index_ok focus=%d", focus)
end

function M.on_click(name)
    for index, entry in ipairs(entries) do
        if entry.scene == name then
            opened = opened + 1
            ui.focus_select("menu", index - 1)
            scene.load(entry.scene)
            log.info("index: click opens " .. entry.scene)
            return true
        end
    end
    log.warn("index: unknown button " .. tostring(name))
    return false
end

function M.on_destroy()
    ui.focus_close("menu")
    ui.scroll_close("api_list")
end

return M
