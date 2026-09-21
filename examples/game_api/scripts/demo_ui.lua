-- ============================================================================
-- api_ui：**运行时动态 UI**（`ui` 表，43 个函数）—— 面板 / 图片 / 网格 / tooltip /
-- 模态 / 拖拽 / 焦点环 / 滚动 / 读回 / 文本度量。
--
-- 这一组的共同前提（每一条都能在引擎代码里核实，不是猜的）：
--
--   1. 脚本用 `ui.*` 画的东西全部是 **Screen 空间**：Lua 侧固定
--      `UiLayoutSpace::Screen` + anchor [0,0]（`core/script.rs` 的 `ui.panel/image/grid`），
--      坐标就是屏幕像素 —— 1280x720 里 (0,0) 是左上角、y 向下。所以本文件的坐标
--      可以照着屏幕量；换窗口大小时它不会跟着缩放（Screen 空间恒 1:1）。
--   2. 宿主在脚本 `update()` **返回之后**才取本帧顶点（`RuntimeScene::vertices`）：
--      "先改状态、再画"才看得见；同一个 id 每帧重复 `ui.text_at` 是正常用法
--      （路由里的 `api_status` / `api_hint` 就是这么画的）。
--   3. 绘制顺序固定为 **面板 → 图片 → 网格 → 进度条 → 文本 → 场景实体的 Text/Image**
--      （`push_ui_panels → push_ui_images → push_ui_grids → push_ui_bars → push_ui_text
--        → push_scene_ui`）。三个后果，本演示刻意都用上：
--        * 压在网格上的 `ui.panel` 会被格子盖住 ⇒ 格子高亮只能用 `ui.grid_cell`
--          重写这一格的颜色/贴图（`sync_grid` 里最后写选中格）；
--        * 跟随光标的拖拽提示用 `ui.text_at` 而不是 `ui.image`（文本排在最后，永远看得见）；
--        * 场景实体（`scenes/api_ui.scene` 的 Toolbar / Hud）永远压在运行时 UI **之上**
--          ⇒ 铺满屏幕的 Image 实体会把整个演示盖掉，所以"背景"只能由脚本自己
--          用 `ui.panel` 画（`_bg`）。
--   4. 同一族内部按 **id 的字典序**绘制：`ScriptState` 里 `ui_panels` / `ui` /
--      `ui_images` / `ui_grids` 都是 `BTreeMap<String, _>`，而渲染器只在**层与层之间**
--      重排、桶内保持原序。所以面板叠放是可控的：全屏背景取 `_bg`（`'_' < 'a'`）
--      排在所有面板之前，模态取 `confirm`（`'c'`）排在 `bag_group`/`bag_tip` 之后。
--   5. 可见性三态（本场景最值得亲手试的一段）：
--        * `ui.hide(id)` 只覆盖**文本与面板**（`core/script.rs` 的 `ui.hide`）：
--          `bag_group` 这个 id 同时是外框面板和它的标题文本 —— 按 q 两个一起消失，
--          而同一区域的网格格子、HP 图标**不受影响**；
--        * `ui.clear(id)` 删**面板 / 图片 / 网格**（外加 `<id>:tooltip` / `<id>:title`
--          两个派生文本），但**不删同 id 的普通动态文本** —— 按 x 之后外框没了、
--          标题文本还在（这就是"clear 到不了普通文本"的直接证据）；
--        * 要让图片消失只能 `ui.clear`（或删掉重建）；按 r 走一遍"先 clear 再重画"。
--   6. `ui.clear` / `ui.focus_*` / `ui.scroll_*` 互不相干：焦点环与滚动状态是**逻辑状态**
--      不是绘制元素，`ui.clear` 不会碰它们，释放要显式 `ui.focus_close` / `ui.scroll_close`
--      （`M.on_destroy` 里做了）。
--
-- 键位分工写在 `api.hint`（第一行）和 `api_ui_keys2`（第二行）里，不读代码也能玩：
--   1 ui.grid_cell 单项高亮   2 ui.grid_clear 清空   3 ui.grid_hit 命中测试
--   4 ui.tooltip / tooltip_hide   5 ui.modal_show / modal_hide   6 ui.bar 数值
--   7/8/9 ui.drag_begin / drag_end / drag_cancel
--   q ui.hide   e ui.show   x ui.clear   r 重建   t ui.text   v measure_text(0) 报错
--   方向键 ui.focus_move   tab ui.focus_cycle   home/end ui.focus_select
--   w/s ui.scroll_lines   pgup/pgdn ui.scroll_page   滚轮 ui.scroll_by
--   鼠标左键在格子上按下/松开 = ui.drag_begin / ui.drag_end（真正的拖放，走 on_drag）
-- ============================================================================
local api = require("api_demo")

local M = {}

-- ---------------------------------------------------------------------------
-- 布局常量（屏幕 1280x720，单位像素，原点左上）
-- ---------------------------------------------------------------------------
local SCREEN_W, SCREEN_H = 1280, 720
local SMALL = 12 -- `ui.text_at` 不写 font_size 时渲染口径是 scale × 12

-- 全屏背景：id 取 `_bg`，靠 id 字典序排在所有面板之前（见文件头第 4 条）。
local BG_ID = "_bg"

-- 背包：`ui.grid(id, x, y, columns, rows, cell_size, gap)` —— 注意 cell_size 是**正方形
-- 边长**、gap 是**唯一**的间距字段，没有 cell_w/cell_h/spacing（以 script.rs 为准）。
local PANEL_ID = "bag_group" -- 同一个 id 既是外框面板，也是它的标题文本
local GRID_ID = "bag"
local GRID_X, GRID_Y = 76, 120
local GRID_COLUMNS, GRID_ROWS, GRID_CELL, GRID_GAP = 4, 3, 64, 10
local GRID_COUNT = GRID_COLUMNS * GRID_ROWS

local HP_ICON_ID = "hp_icon"
local TIP_ID = "bag_tip"
local MODAL_ID = "confirm"
local MODAL_BODY_ID = "confirm_body"

-- 焦点环：`ui.focus_open(id, count[, columns])`，序号 **0 基**；4 列布局让左右键也有意义。
local MENU_BG_ID = "menu_bg"
local MENU_PICK_ID = "menu_pick"
local FOCUS_ID = "menu"
local FOCUS_ITEMS, FOCUS_COLUMNS = 8, 4
local MENU_X, MENU_Y, MENU_W, MENU_H, MENU_GAP = 716, 84, 120, 48, 12

-- 滚动列表：视口 276px < 内容 24 行 × 24px = 576px ⇒ 最大偏移 300px（真的能滚）。
local LOG_BG_ID = "log_bg"
local SCROLL_ID = "log"
local LOG_X, LOG_Y = 712, 250
local LOG_ROWS, LOG_ROW_H = 24, 24
local LOG_VIEW_H = 276

-- 中间竖条（x 412..672）：tooltip / 拖拽状态 / 度量结果；y ≥ 300 留给模态，别放文本，
-- 因为文本永远排在面板之后 —— 压在模态上的文本会把模态"戳穿"。
local STRIP_X = 412

-- ---------------------------------------------------------------------------
-- 模块状态（顶层只定义函数与常量，引擎 API 一律在 on_create / update 里调）
-- ---------------------------------------------------------------------------
local ready = false
local hp = 0.75
local selection = 1 -- 网格序号是 1 基
local items = {} -- items[i] = {tex=, name=, r=, g=, b=}；nil = 空格子
local grid_dirty = true -- 需要按模型重写格子
local grid_ready = false -- `ui.grid` 还在吗（grid_cell/grid_clear/grid_hit 对不存在的 id 会报错）
local tooltip_on = false
local modal_on = false
local probe_on = false
local note = "" -- 最近一次按键的"事实"
local error_note = "" -- 受控失败的原文（`ui.measure_text` 的错误契约）
local error_hold = 0 -- `error_note` 还能在状态行停留几帧（0 = 让位给 note）
local last_hit = nil -- 最近一次 `ui.grid_hit` 的结果
local drag_calls = 0 -- `on_drag` 被调用了几次
local log_first, log_last = 0, -1 -- 上一帧画过的日志行（用来 hide 掉离开窗口的行）

-- ---------------------------------------------------------------------------
-- 工具
-- ---------------------------------------------------------------------------

--- 与渲染同口径的文本度量，受控调用：`ui.measure_text` 要读工程根下的字体配置
--- （`project.toml` 的 `font`，缺省 `assets/fonts/NotoSansSC-VF.ttf`），没有工程根的宿主
--- 里它会明确报错；headless 不能让这一帧因此炸掉，所以按工程约定用 `api.try` 包一层。
--- 返回 `{width,height,lines,glyphs,missing,missing_count}, err`。
local function measure(text, size)
    return api.try(function()
        return ui.measure_text(text, size)
    end)
end

--- 网格第 `index` 格（1 基）的屏幕矩形。
--- 与 `UiGrid::cell_rect` 在 Screen 空间同算法：列优先、步长 = 边长 + 间距
--- （`col = (i-1) % columns`、`line = (i-1) / columns`）。命中测试因此和画出来的格子对得上。
local function cell_rect(index)
    local zero = index - 1
    local column = zero % GRID_COLUMNS
    local line = math.floor(zero / GRID_COLUMNS)
    return GRID_X + column * (GRID_CELL + GRID_GAP), GRID_Y + line * (GRID_CELL + GRID_GAP)
end

--- 焦点项在屏幕上的矩形：与 `ui.focus_open(id, count, columns)` 的列布局一一对应。
--- 引擎内部给每项一个 `[column, line, 1, 1]` 的单元矩形（只用于方向导航），
--- "画在哪里、多大"由游戏决定 —— 本演示把它画成 4x2 的按钮格。
local function focus_rect(index)
    local column = index % FOCUS_COLUMNS
    local line = math.floor(index / FOCUS_COLUMNS)
    return MENU_X + column * (MENU_W + MENU_GAP), MENU_Y + line * (MENU_H + MENU_GAP)
end

--- 当前焦点序号对应的 1 基日志行号（焦点 0 基、行 1 基，转换在这里收口 —— 与
--- `examples/game_jhyt/scripts/ui/panel_nav.lua` 的做法一致）；无焦点返回 -1。
local function focus_row()
    local index = ui.focus_index(FOCUS_ID)
    if type(index) ~= "number" then
        return -1
    end
    return index + 1
end

--- 焦点变化后的收尾：把对应日志行滚进视口，并把这次移动记进状态行。
--- `ui.scroll_to_visible(id, item_top, item_height)` 只在**不完全可见**时滚动（最小滚动
--- 语义），所以方向键连按不会抖动；返回值就是"有没有真的滚"。
local function after_focus(index, label)
    if type(index) ~= "number" then
        return
    end
    local moved = ui.scroll_to_visible(SCROLL_ID, index * LOG_ROW_H, LOG_ROW_H)
    note = string.format("%s -> focus %d（scroll_to_visible=%s）", label, index + 1, tostring(moved))
end

--- `ui.focus_move(id, "up|down|left|right")`：到边界会**环绕**，未知方向是 Lua 错误
--- （所以方向只用字面量传，不做字符串拼接）。
local function move_focus(direction, label)
    local index = ui.focus_move(FOCUS_ID, direction)
    if type(index) == "number" then
        after_focus(index, label)
    end
end

-- ---------------------------------------------------------------------------
-- 建立"只建一次"的元素
-- ---------------------------------------------------------------------------

--- 物品模型（Lua 侧真源）：`ui.grid_cell` 只负责把它画出来。
local function reset_items()
    items = {}
    items[1] = { tex = "assets/textures/coin.png", name = "Coin", r = 1.00, g = 0.86, b = 0.35 }
    items[2] = { tex = "assets/textures/coin.png", name = "Coin", r = 1.00, g = 0.86, b = 0.35 }
    items[5] = { tex = "assets/textures/slime.png", name = "Slime", r = 0.65, g = 1.00, b = 0.65 }
    items[7] = { tex = "assets/textures/slime.png", name = "Slime", r = 1.00, g = 0.55, b = 0.55 }
    selection = 1
    grid_dirty = true
end

--- 建**不是每帧重画**的元素：背景面板、外框面板、网格、HP 图标、焦点环、滚动状态。
---
--- 为什么不每帧重画：`ui.panel` / `ui.image` / `ui.grid` 每次调用都会把可见性重置为 true、
--- 并覆盖上一次的内容。于是"每帧重建"的元素根本 hide 不掉（引擎注释里说的"先 clear 再重画
--- 的面板类 UI 隐藏不掉"就是这个机制）。本演示因此把静态元素建一次，
--- hide/show/clear（q/e/x）才看得到效果，只有"位置会变"的高亮框才每帧重画。
local function build_static()
    -- 背景：铺满屏幕的面板。用面板而不是场景实体的 Image —— 实体的 Image/Text 画在
    -- 所有运行时 UI **之上**，铺满屏幕会把整个演示盖掉（见文件头第 3 条）。
    ui.panel(BG_ID, 0, 0, SCREEN_W, SCREEN_H, 0.045, 0.055, 0.075, 1.0)

    -- 背包外框：`ui.panel(id, x, y, width, height, r, g, b, a)`（9 个参数，最后一个是 alpha）。
    ui.panel(PANEL_ID, 40, 64, 360, 340, 0.130, 0.160, 0.210, 1.0)
    -- 外框的标题文本与面板**同 id**：只有这样 `hide` / `clear` 的差异才看得见 ——
    -- q 之后面板与文本一起消失（hide 覆盖文本+面板），x 之后面板没了、文本还在
    -- （clear 删面板/图片/网格，却到不了同 id 的普通文本）。
    --
    -- 关键细节：这行**只在建的时候写一次**，不能每帧 `ui.text_at` ——
    -- `ui.text_at` 每次都把 `visible` 置回 true，于是"每帧重画"的文本永远 hide 不掉
    -- （引擎注释里"先 clear 再重画的面板类 UI 隐藏不掉"说的就是这个机制）。
    -- 反过来，`bag_info` 那行内容每帧都在变，就必须每帧重写。
    ui.text_at(
        PANEL_ID,
        56,
        72,
        string.format(
            "ui.grid(%dx%d, cell %d, gap %d) - %d cells, 1-based",
            GRID_COLUMNS,
            GRID_ROWS,
            GRID_CELL,
            GRID_GAP,
            GRID_COUNT
        ),
        1.0,
        SMALL
    )

    -- 焦点菜单与日志列表的底板（同族内按 id 排序：`menu_bg` 在 `menu_pick` 之前 ⇒ 高亮画得上去）。
    ui.panel(MENU_BG_ID, 700, 64, 544, 148, 0.100, 0.125, 0.170, 1.0)
    ui.panel(LOG_BG_ID, 700, 240, 544, 300, 0.085, 0.105, 0.145, 1.0)

    -- `ui.grid(id, x, y, columns, rows, cell_size, gap)`：格子序号 1 基、行优先。
    ui.grid(GRID_ID, GRID_X, GRID_Y, GRID_COLUMNS, GRID_ROWS, GRID_CELL, GRID_GAP)
    grid_ready = true
    grid_dirty = true

    -- `ui.image(id, x, y, width, height, texture, r, g, b)`：注意是 (x,y,w,h,texture,r,g,b)，
    -- 没有 alpha —— 前 5 个参数与 `ui.panel` 的位置一致，之后一个贴图路径 + 3 个颜色分量。
    ui.image(HP_ICON_ID, 40, 422, 26, 26, "assets/textures/coin.png", 1.0, 0.86, 0.35)

    -- 焦点环：8 项、4 列；已有环保留当前焦点（重建不会丢选择）。
    ui.focus_open(FOCUS_ID, FOCUS_ITEMS, FOCUS_COLUMNS)
    -- 刚建好的环"当前焦点"是**空的**（引擎语义：无焦点时方向键/Tab 落到第一项），
    -- 这里显式选中第 0 项，让高亮框一开始就看得见；重建时若已有焦点就不动它。
    if ui.focus_index(FOCUS_ID) == nil then
        ui.focus_select(FOCUS_ID, 0)
    end

    -- 滚动状态：视口 276px、内容 576px ⇒ max_offset = 300px。`scroll_open` 会把偏移清零。
    ui.scroll_open(SCROLL_ID, LOG_VIEW_H, LOG_ROWS * LOG_ROW_H)
    log_first, log_last = 0, -1
end

--- 用 Lua 侧模型重写整张网格：先 `ui.grid_clear` 复位，再逐格 `ui.grid_cell`。
--- 只在模型变了（`grid_dirty`）时做 —— 因为 `grid_clear` 会覆盖上一步的结果，
--- 按 2 清空之后不再重写，清空才看得见。
local function sync_grid()
    if not (grid_ready and grid_dirty) then
        return
    end
    ui.grid_clear(GRID_ID)
    for index = 1, GRID_COUNT do
        local item = items[index]
        if item then
            -- `ui.grid_cell(id, index, texture, r, g, b, a)`：贴图可以是 nil（空格子）。
            ui.grid_cell(GRID_ID, index, item.tex, item.r, item.g, item.b, 1.0)
        end
    end
    -- 选中格**最后**写：grid_cell 是"写这一格"，后写的赢 ⇒ 高亮成立。
    -- 高亮只能用格子自己的颜色（压在网格上的 ui.panel 会被格子盖住）。
    local item = items[selection]
    ui.grid_cell(GRID_ID, selection, item and item.tex or nil, 1.0, 0.82, 0.25, 1.0)
    grid_dirty = false
end

-- ---------------------------------------------------------------------------
-- 输入：数字键各演示一族 API，字母键演示可见性三态
-- ---------------------------------------------------------------------------
local function handle_keys()
    local cursor = input.cursor_position()
    local cx, cy = cursor[1] or 0, cursor[2] or 0

    if input.just_pressed("1") then
        -- 单项高亮：重写整张网格并让选中格变成琥珀色。
        grid_dirty = true
        note = "1: ui.grid_cell 重写格子 + 高亮选中格"
    elseif input.just_pressed("2") then
        if grid_ready then
            -- `ui.grid_clear(id)`：把每格复位成默认灰、贴图清空（不删网格本身）。
            ui.grid_clear(GRID_ID)
            grid_dirty = false
            note = "2: ui.grid_clear -> 格子全空（按 1 会照模型重画）"
        end
    elseif input.just_pressed("3") then
        if grid_ready then
            -- `ui.grid_hit(id, x, y)`：命中返回 **1 基** 序号，没命中返回 nil；id 不存在会报错。
            last_hit = ui.grid_hit(GRID_ID, cx, cy)
            if type(last_hit) == "number" then
                selection = last_hit
                grid_dirty = true
                note = string.format("3: ui.grid_hit(%.0f,%.0f)=%d 命中并选中", cx, cy, last_hit)
            else
                note = string.format("3: ui.grid_hit(%.0f,%.0f)=nil（光标不在格子上）", cx, cy)
            end
        end
    elseif input.just_pressed("4") then
        if tooltip_on then
            -- `ui.tooltip_hide(id)`：面板与 `<id>:tooltip` 文本一起隐藏（两者同生同灭）。
            ui.tooltip_hide(TIP_ID)
            tooltip_on = false
            note = "4: ui.tooltip_hide -> tooltip 面板与 :tooltip 文本一起消失"
        else
            local item = items[selection]
            local label = item and item.name or "(empty)"
            -- 宽度按**量出来的**真实字宽给（+ 左右内边距），而不是按字数估 —— 中文排版
            -- 不能用 `string.len` 数字节：CJK 满宽、ASCII 半宽。
            local measured = measure(label .. " x1", 14)
            local width = 168
            if measured and measured.width then
                width = math.max(140, math.min(240, measured.width + 26))
            end
            -- `ui.tooltip(id, x, y, text, width, height)`：贴图文本由引擎按 `x+8, y+4`、
            -- font_size=14 画在面板里；调用它就是"建/覆盖"，不是"追加"。
            ui.tooltip(
                TIP_ID,
                STRIP_X,
                120,
                string.format("%s x1 - cell %d (%.0fpx)", label, selection, width),
                width,
                40
            )
            tooltip_on = true
            note = string.format("4: ui.tooltip 宽 %.0fpx（= ui.measure_text 的字宽 + 26）", width)
        end
    elseif input.just_pressed("5") then
        if modal_on then
            -- `ui.modal_hide(id)` 只关掉**这个面板**与 `<id>:title` 文本；正文是另一个 id
            -- 的普通文本，得自己 hide —— 这正是"一个模态由几种元素拼成"的现实。
            ui.modal_hide(MODAL_ID)
            ui.hide(MODAL_BODY_ID)
            modal_on = false
            note = "5: ui.modal_hide + ui.hide(正文) -> 模态关闭"
        else
            ui.modal_show(MODAL_ID)
            ui.show(MODAL_BODY_ID)
            modal_on = true
            note = "5: ui.modal_show -> 面板与 :title 一起出现"
        end
    elseif input.just_pressed("6") then
        -- `ui.bar(id, x, y, width, height, ratio, r, g, b)`：ratio 由引擎钳到 0..1，
        -- 画的是"底色 + 按比例填充"两根矩形，所以数值变了条就变。
        hp = hp - 0.25
        if hp < 0.0 then
            hp = 1.0
        end
        note = string.format("6: ui.bar ratio=%.2f", hp)
    elseif input.just_pressed("7") then
        if grid_ready then
            -- `ui.drag_begin(id, index)`：登记"正在搬哪一格"；id 不是网格或 index ≤ 0 会报错。
            ui.drag_begin(GRID_ID, selection)
            note = string.format("7: ui.drag_begin(bag,%d) -> ui.drag_active() 有值", selection)
        end
    elseif input.just_pressed("8") then
        local active = ui.drag_active()
        if active and grid_ready then
            local target = selection % GRID_COUNT + 1
            -- `ui.drag_end(to_id, to_index)`：清掉拖拽状态并调用全局 `on_drag(from..., to...)`
            -- （本文件末尾的 `M.on_drag`），返回 true；没有活动拖拽时返回 false 且不回调。
            local ended = ui.drag_end(GRID_ID, target)
            note = string.format("8: ui.drag_end(bag,%d)=%s on_drag=%d", target, tostring(ended), drag_calls)
        else
            local ended = ui.drag_end(GRID_ID, selection)
            note = string.format("8: 没有活动拖拽 -> ui.drag_end 返回 %s", tostring(ended))
        end
    elseif input.just_pressed("9") then
        -- `ui.drag_cancel()`：丢掉拖拽状态，不发 on_drag（"放弃这次拖放"）。
        ui.drag_cancel()
        note = "9: ui.drag_cancel -> drag_active() 变回 nil"
    elseif input.just_pressed("q") then
        -- `ui.hide(id)` 覆盖**同 id 的面板与文本**：外框和它的标题文本一起隐藏，
        -- 同一区域的网格格子 / HP 图标（图片）不受影响 —— 图片只有 clear 能删。
        ui.hide(PANEL_ID)
        note = "q: ui.hide(外框) -> 面板+同 id 文本隐藏；网格与图片还在"
    elseif input.just_pressed("e") then
        ui.show(PANEL_ID)
        note = "e: ui.show(外框) -> 面板与文本一起回来"
    elseif input.just_pressed("x") then
        -- `ui.clear(id)` 删**面板 / 图片 / 网格**（外加 `<id>:tooltip` / `<id>:title` 文本），
        -- 但**不删同 id 的普通动态文本** ⇒ 外框、网格、HP 图标都没了，
        -- 而 `bag_group` 那行标题仍然画着（"clear 到不了普通文本"的直接证据）。
        ui.clear(PANEL_ID)
        ui.clear(GRID_ID)
        ui.clear(HP_ICON_ID)
        ui.clear(TIP_ID)
        ui.clear(MENU_BG_ID)
        ui.clear(LOG_BG_ID)
        grid_ready = false
        tooltip_on = false
        note = "x: ui.clear -> 面板/图片/网格消失，同 id 的文本留着（按 r 重建）"
    elseif input.just_pressed("r") then
        -- "先 clear 再重画"：面板类 UI 的常规重建路径。重建后面板重新可见（ui.panel 置 visible=true）。
        build_static()
        note = "r: 重新 ui.panel/ui.grid/ui.image"
    elseif input.just_pressed("t") then
        if probe_on then
            ui.hide("ui_text_probe")
            probe_on = false
            note = "t: ui.hide(ui_text_probe) -> ui.get 仍是表、visible=false"
        else
            -- `ui.text(id, text)` 是最小的文本 API：位置固定 (10,10)、scale 1，因此会压到
            -- 左上角标题上，默认隐藏，按 t 就地看它出现。
            ui.text("ui_text_probe", "ui.text(id, text) -> 固定 (10,10) scale 1")
            probe_on = true
            note = "t: ui.text(ui_text_probe) 出现在左上角 (10,10)"
        end
    elseif input.just_pressed("v") then
        -- `ui.measure_text(text, size)` 的契约：**size 必须为正数**，否则报 Lua 错误
        -- （不会悄悄退回默认字号）。把错误原文抓进状态行，让"错误也是 API 的一部分"可见。
        local value, err = api.try(function()
            return ui.measure_text("size=0 是非法参数", 0)
        end)
        if value then
            error_note = "v: (没有报错，与文档不符)"
        else
            -- 引擎的错误文本带换行与调用栈：压成一行再截断，否则状态行会被撑成多行。
            local flat = tostring(err):gsub("%s+", " ")
            error_note = "v: " .. string.sub(flat, 1, 72)
        end
        error_hold = 120 -- 错误原文在状态行停留约 2 秒（120 帧 @60fps），之后自动让位给 note
        note = "v: ui.measure_text(size=0) 被引擎拒绝"
    elseif input.just_pressed("up") then
        move_focus("up", "up: ui.focus_move")
    elseif input.just_pressed("down") then
        move_focus("down", "down: ui.focus_move")
    elseif input.just_pressed("left") then
        move_focus("left", "left: ui.focus_move")
    elseif input.just_pressed("right") then
        move_focus("right", "right: ui.focus_move")
    elseif input.just_pressed("tab") then
        -- Shift+Tab = 反向：`ui.focus_cycle(id[, forward])`，默认正向。
        local forward = not input.is_down("shiftleft")
        local index = ui.focus_cycle(FOCUS_ID, forward)
        after_focus(index, forward and "tab: ui.focus_cycle(forward)" or "shift+tab: ui.focus_cycle(false)")
    elseif input.just_pressed("home") then
        -- `ui.focus_select(id, index)`：按 0 基序号直接定位（越界返回 false）。
        if ui.focus_select(FOCUS_ID, 0) then
            after_focus(0, "home: ui.focus_select(0)")
        end
    elseif input.just_pressed("end") then
        if ui.focus_select(FOCUS_ID, FOCUS_ITEMS - 1) then
            after_focus(FOCUS_ITEMS - 1, string.format("end: ui.focus_select(%d)", FOCUS_ITEMS - 1))
        end
    elseif input.just_pressed("w") then
        -- `ui.scroll_lines(id, lines, line_height)`：返回**实际生效**的增量（已钳制）。
        local applied = ui.scroll_lines(SCROLL_ID, -1, LOG_ROW_H)
        note = string.format("w: ui.scroll_lines(-1,24) 生效 %.0f", applied)
    elseif input.just_pressed("s") then
        local applied = ui.scroll_lines(SCROLL_ID, 1, LOG_ROW_H)
        note = string.format("s: ui.scroll_lines(+1,24) 生效 %.0f", applied)
    elseif input.just_pressed("pageup") then
        note = string.format("pgup: ui.scroll_page(-1) 生效 %.0f", ui.scroll_page(SCROLL_ID, -1))
    elseif input.just_pressed("pagedown") then
        note = string.format("pgdn: ui.scroll_page(+1) 生效 %.0f", ui.scroll_page(SCROLL_ID, 1))
    end

    -- 滚轮：`input.scroll_delta()` 返回的是**数组**（[1]=x、[2]=y），不是数字 ——
    -- 拿整张表去跟 0 比较永远为真，必须先取 [2]。y 为正 = 向上滚 ⇒ 偏移减小；
    -- 所以传给 `ui.scroll_by` 的要取反，`-dy * 一行高` 就是"滚一行"。
    local wheel = input.scroll_delta()
    local dy = wheel and wheel[2] or 0
    if dy ~= 0 then
        -- `ui.scroll_by(id, delta)`：按像素滚动，返回**实际生效**的增量（已钳到 0..max）。
        local applied = ui.scroll_by(SCROLL_ID, -dy * LOG_ROW_H)
        note = string.format("wheel %.1f -> ui.scroll_by(%.1f) 生效 %.0f", dy, -dy * LOG_ROW_H, applied)
    end

    -- 鼠标：左键在格子上按下 = ui.drag_begin，松开 = ui.drag_end（真正的拖放，走 on_drag）。
    -- headless 下这两个 `mouse_*` 恒为 false，因此这段不会在无输入时触发。
    if grid_ready then
        if input.mouse_just_pressed("left") then
            local hit = ui.grid_hit(GRID_ID, cx, cy)
            if type(hit) == "number" then
                ui.drag_begin(GRID_ID, hit)
                selection = hit
                grid_dirty = true
                note = string.format("mouse down cell %d -> ui.drag_begin", hit)
            end
        elseif input.mouse_just_released("left") then
            if ui.drag_active() then
                local hit = ui.grid_hit(GRID_ID, cx, cy)
                local target = hit or selection
                ui.drag_end(GRID_ID, target)
                note = string.format("mouse up -> ui.drag_end(bag,%d)", target)
            end
        elseif input.mouse_is_down("left") and ui.drag_active() then
            -- 拖拽中：高亮跟着光标下的格子走（每次都是一次真实命中测试）。
            local hit = ui.grid_hit(GRID_ID, cx, cy)
            if type(hit) == "number" and hit ~= selection then
                selection = hit
                grid_dirty = true
            end
        end
    end
end

-- ---------------------------------------------------------------------------
-- 绘制（都在 update 返回前完成：宿主之后才取本帧顶点）
-- ---------------------------------------------------------------------------

local function draw_inventory()
    -- 外框面板与它的标题文本都在 `build_static` 里建（同 id，见那里的注释），
    -- 这里只更新每帧都会变的 `bag_info`。
    local item = items[selection]
    ui.text_at(
        "bag_info",
        56,
        344,
        string.format(
            "sel=%d/%d %s hit=%s drag=%s",
            selection,
            GRID_COUNT,
            item and item.name or "(empty)",
            last_hit and tostring(last_hit) or "nil",
            ui.drag_active() and "active" or "nil"
        ),
        1.0,
        SMALL
    )
end

local function draw_menu()
    local index = ui.focus_index(FOCUS_ID)
    -- 高亮框每帧重画（位置会变）：id `menu_pick` 排在 `menu_bg` 之后 ⇒ 画在底板之上；
    -- 标签文本排在所有面板之后 ⇒ 压不住。这两条都是 id 字典序 + 分族顺序的直接结果。
    if type(index) == "number" then
        local x, y = focus_rect(index)
        ui.panel(MENU_PICK_ID, x - 5, y - 5, MENU_W + 10, MENU_H + 10, 0.95, 0.72, 0.25, 0.85)
    end
    for i = 0, FOCUS_ITEMS - 1 do
        local x, y = focus_rect(i)
        ui.text_at("menu_item_" .. i, x + 10, y + 14, string.format("item %d", i + 1), 1.0, 13)
    end
    ui.text_at(
        "menu_info",
        700,
        220,
        string.format(
            "ui.focus_*: open(%d, %d cols) index=%s count=%d - arrows / tab / home / end",
            FOCUS_ITEMS,
            FOCUS_COLUMNS,
            index and tostring(index) or "nil",
            ui.focus_count(FOCUS_ID)
        ),
        1.0,
        SMALL
    )
end

--- 日志行文本（列表内容）。
local function row_text(row)
    return string.format("row %02d  ui.scroll_lines / scroll_page / scroll_by", row)
end

local function draw_log()
    -- 每帧同步视口/内容高度：窗口或内容变化后偏移会被重新钳制；返回值就是当前偏移。
    -- （headless 里窗口大小不参与 Screen 空间布局，这条调用依然是"更新 + 读回"两件事。）
    ui.scroll_extents(SCROLL_ID, LOG_VIEW_H, LOG_ROWS * LOG_ROW_H)

    -- 只画**可见区间**：`ui.scroll_visible(id)` 给的是内容像素区间 `{start=, stop=}`
    -- （字段名刻意避开 Lua 关键字 `end`），换算成 1 基行号即可。
    local offset = ui.scroll_offset(SCROLL_ID) or 0
    local visible = ui.scroll_visible(SCROLL_ID)
    local start_px = visible and visible.start or 0
    local stop_px = visible and visible.stop or 0
    local first = math.max(1, math.floor(start_px / LOG_ROW_H) + 1)
    local last = math.min(LOG_ROWS, math.ceil(stop_px / LOG_ROW_H))
    if last < first then
        last = first - 1
    end

    local marked = focus_row()
    for row = first, last do
        local y = LOG_Y + (row - 1) * LOG_ROW_H - offset
        local marker = (row == marked) and "> " or "  "
        ui.text_at("log_row_" .. row, LOG_X, y, marker .. row_text(row), 1.0, SMALL)
    end
    -- 上一帧画过、这一帧已经离开窗口的行：收掉它们的 id。
    -- 这里用 `ui.hide` 而不是 `ui.clear` —— `ui.clear` 删不掉普通动态文本（见文件头第 5 条）。
    for row = log_first, log_last do
        if row < first or row > last then
            ui.hide("log_row_" .. row)
        end
    end
    log_first, log_last = first, last

    ui.text_at(
        "log_info",
        700,
        546,
        string.format(
            "ui.scroll_*: offset=%.0f max=%.0f visible=%.0f..%.0f - %d rows x %dpx",
            offset,
            ui.scroll_max(SCROLL_ID) or 0,
            start_px,
            stop_px,
            LOG_ROWS,
            LOG_ROW_H
        ),
        1.0,
        SMALL
    )
end

local function draw_hud()
    -- `ui.bar(id, x, y, width, height, ratio, r, g, b)`
    ui.bar("hp", 72, 424, 268, 22, hp, 0.86, 0.30, 0.26)
    ui.text_at("hp_label", 40, 458, string.format("ui.bar ratio=%.2f (key 6)", hp), 1.0, SMALL)

    -- 读回族：把"画出来的东西"变成可读回、可断言的事实（headless 验收就靠这几行）。
    local panel = ui.panel_get(PANEL_ID)
    local image = ui.image_get(HP_ICON_ID)
    local drawn = ui.text_get("bag_info")
    local probe = ui.get("ui_text_probe")
    local cjk = measure("背包 - 金币 x12", 14)

    ui.text_at(
        "rb_panel",
        40,
        486,
        string.format(
            "ui.panel_get(%s) = %s",
            PANEL_ID,
            panel and string.format("%.0fx%.0f at (%.0f,%.0f) visible=%s", panel.width, panel.height, panel.x, panel.y, tostring(panel.visible)) or "nil"
        ),
        1.0,
        SMALL
    )
    ui.text_at(
        "rb_image",
        40,
        502,
        string.format(
            "ui.image_get(%s) = %s",
            HP_ICON_ID,
            image and string.format("%.0fx%.0f at (%.0f,%.0f) %s", image.width, image.height, image.x, image.y, image.texture) or "nil"
        ),
        1.0,
        SMALL
    )
    ui.text_at(
        "rb_text",
        40,
        518,
        string.format(
            "ui.text_get(bag_info) = %s",
            drawn and string.format("(%.0f,%.0f) scale=%.1f size=%.0fpx visible=%s", drawn.x, drawn.y, drawn.scale, drawn.size, tostring(drawn.visible)) or "nil"
        ),
        1.0,
        SMALL
    )
    ui.text_at(
        "rb_get",
        40,
        534,
        string.format(
            "ui.get(ui_text_probe) = %s   (t: ui.text 之后变成表)",
            probe and string.format("'%s' visible=%s", probe.text, tostring(probe.visible)) or "nil"
        ),
        1.0,
        SMALL
    )
    ui.text_at(
        "rb_measure",
        40,
        550,
        string.format(
            "ui.measure_text('背包 - 金币 x12', 14) = %s",
            cjk and string.format("%.0fx%.0f lines=%.0f glyphs=%.0f missing=%s", cjk.width, cjk.height, cjk.lines, cjk.glyphs, tostring(cjk.missing_count)) or "(unavailable)"
        ),
        1.0,
        SMALL
    )
end

local function draw_strip()
    -- tooltip 的状态（面板本身只在按键 4 时建/删）。
    ui.text_at(
        "tip_info",
        STRIP_X,
        168,
        string.format("ui.tooltip / tooltip_hide: %s (key 4)", tooltip_on and "shown" or "hidden"),
        1.0,
        SMALL
    )

    -- 拖拽：`ui.drag_active()` 返回 `{id=, index=}` 或 nil。
    local active = ui.drag_active()
    ui.text_at(
        "drag_info",
        STRIP_X,
        188,
        string.format(
            "ui.drag_active() = %s on_drag 回调 %d 次",
            active and string.format("%s:%s", tostring(active.id), tostring(active.index)) or "nil",
            drag_calls
        ),
        1.0,
        SMALL
    )
    if active then
        -- 跟随光标的提示用**文本**：动态图片排在网格之前绘制，压在背包上的图片会被格子盖住；
        -- 文本排在最后，永远看得见。
        local cursor = input.cursor_position()
        ui.text_at(
            "drag_ghost",
            (cursor[1] or 0) + 18,
            (cursor[2] or 0) - 8,
            string.format("drag %s:%s", tostring(active.id), tostring(active.index)),
            1.1,
            14
        )
    else
        ui.hide("drag_ghost")
    end

    -- 度量的用法：面板/文本宽度按真实字宽给，而不是按字数估（中文排版的老坑）。
    local measured = measure("背包 - 金币 x12", 14)
    ui.text_at(
        "measure_info",
        STRIP_X,
        216,
        string.format(
            "ui.measure_text -> tooltip 宽度 = 字宽(%.0f) + 26，按真实字宽裁剪/居中",
            measured and measured.width or 0
        ),
        1.0,
        SMALL
    )
end

--- 状态行：本帧的"事实"（读回值、计数、当前状态），不是自述。
local function publish_status()
    local focus = ui.focus_index(FOCUS_ID)
    local active = ui.drag_active()
    local cjk = measure("背包 - 金币 x12", 14)
    -- 受控失败优先显示一小会儿：它演示的是"API 的契约"，按下一次操作后自动让位给 note。
    local tail = note
    if error_hold > 0 then
        error_hold = error_hold - 1
        tail = error_note
    end
    api.status(
        string.format(
            "grid=%s sel=%d/%d hit=%s | hp=%.2f | focus=%s/%d | scroll=%.0f/%.0f | drag=%s | CJK=%.0fpx miss=%s | %s",
            grid_ready and "on" or "off",
            selection,
            GRID_COUNT,
            last_hit and tostring(last_hit) or "nil",
            hp,
            focus and tostring(focus + 1) or "-",
            ui.focus_count(FOCUS_ID),
            ui.scroll_offset(SCROLL_ID) or 0,
            ui.scroll_max(SCROLL_ID) or 0,
            active and "active" or "nil",
            cjk and cjk.width or 0,
            cjk and tostring(cjk.missing_count) or "-",
            tail
        )
    )
end

local function draw()
    draw_inventory()
    draw_menu()
    draw_log()
    draw_hud()
    draw_strip()
    -- 第二行键位提示：`api.hint` 只有一行，焦点/滚动那半截放这里。
    ui.text_at(
        "api_ui_keys2",
        24,
        682,
        "focus: arrows=ui.focus_move tab=ui.focus_cycle home/end=ui.focus_select | scroll: w/s=ui.scroll_lines pgup/pgdn=ui.scroll_page wheel=ui.scroll_by",
        1.0,
        11
    )
    publish_status()
end

-- ---------------------------------------------------------------------------
-- 生命周期
-- ---------------------------------------------------------------------------

--- 只在进入本场景时调用一次。
function M.on_create()
    api.title("api_ui - runtime ui: panel / image / grid / tooltip / modal / drag / focus / scroll")
    api.hint(
        "1 grid_cell  2 grid_clear  3 grid_hit  4 tooltip  5 modal  6 bar  7/8/9 drag  "
            .. "q hide  e show  x clear  r rebuild  t ui.text  v measure_text(0)  "
            .. api.KEYS_BACK
    )

    reset_items()
    build_static()

    -- `ui.text(id, text)`：最小文本 API，位置固定 (10,10)。它会压到左上角标题上，
    -- 所以建好立刻 hide —— 按 t 就地显示，顺便让 `ui.get` 从 nil 变成一张表。
    ui.text("ui_text_probe", "ui.text(id, text) -> 固定 (10,10) scale 1")
    ui.hide("ui_text_probe")

    -- 模态：`ui.modal(id, x, y, width, height, title)` 建面板 + `<id>:title` 文本；
    -- 一开始收起来，按 5 才 `ui.modal_show`。
    ui.modal(MODAL_ID, STRIP_X, 300, 260, 150, "ui.modal - Discard item?")
    ui.text_at(
        MODAL_BODY_ID,
        STRIP_X + 12,
        340,
        "ui.modal_show / ui.modal_hide 只覆盖面板与 <id>:title，正文是另一个 id 的文本。",
        1.0,
        SMALL
    )
    ui.modal_hide(MODAL_ID)
    ui.hide(MODAL_BODY_ID)
    modal_on = false

    tooltip_on = false
    probe_on = false
    error_note = ""
    error_hold = 0
    drag_calls = 0
    last_hit = nil
    note = "on_create: grid 4x3 / focus 8 in 4 cols / scroll 576 over 276"
    ready = true
    log.info("api_ui: runtime ui demo ready")
end

--- 每个固定步一次；返回值进引擎日志（`run_script_steps` / 编辑器 Play 都能看到）。
function M.update()
    -- 宿主可能只步进帧而不调 `on_create`（headless 门就可能是这样）：这里补一次初始化，
    -- 让 `update()` 在任何调用顺序下都能跑通（不切场景、不写盘）。
    if not ready then
        M.on_create()
    end

    handle_keys()
    sync_grid()
    draw()

    local focus = ui.focus_index(FOCUS_ID)
    return string.format(
        "api_ui_ok grid=%s sel=%d focus=%s scroll=%.0f drag=%s",
        grid_ready and "on" or "off",
        selection,
        focus and tostring(focus) or "-",
        ui.scroll_offset(SCROLL_ID) or 0,
        ui.drag_active() and "active" or "nil"
    )
end

--- `ui.drag_end(to_id, to_index)` 触发的全局回调（`main.lua` → `router.lua` → 这里）。
--- 这里把"拖放"落成**模型变更**：起点格的物品与目标格互换，再靠 `grid_dirty` 让
--- `ui.grid_cell` 把新布局画出来 —— 拖拽 API 本身只负责"谁拖到哪儿"，不搬数据。
function M.on_drag(from_id, from_index, to_id, to_index)
    drag_calls = drag_calls + 1
    local target = tonumber(to_index) or 0
    if
        from_id == GRID_ID
        and to_id == GRID_ID
        and target >= 1
        and target <= GRID_COUNT
        and tonumber(from_index)
    then
        local from = tonumber(from_index)
        if from >= 1 and from <= GRID_COUNT then
            items[from], items[target] = items[target], items[from]
            selection = target
            grid_dirty = true
            note = string.format("on_drag %d->%d 交换完成", from, target)
        end
    else
        note = string.format("on_drag %s:%s -> %s:%s（没有可交换的格子）", tostring(from_id), tostring(from_index), tostring(to_id), tostring(to_index))
    end
    return true
end

--- 离开场景时释放：焦点环与滚动状态是**逻辑状态**，`ui.clear` 到不了，必须显式关闭；
--- 绘制元素用 `ui.clear`（`ui.hide` 删不掉图片与网格）。
function M.on_destroy()
    ui.drag_cancel()
    ui.focus_close(FOCUS_ID)
    ui.scroll_close(SCROLL_ID)
    ui.clear(BG_ID)
    ui.clear(PANEL_ID)
    ui.clear(GRID_ID)
    ui.clear(HP_ICON_ID)
    ui.clear(TIP_ID)
    ui.clear(MENU_BG_ID)
    ui.clear(LOG_BG_ID)
    ui.clear(MODAL_ID)
    ui.clear(MODAL_BODY_ID)
    ui.clear("ui_text_probe")
    ready = false
end

return M
