-- ui_test：**场景 UI 组件**与 `UiEvents` 点击事件的演示（运行时动态 `ui.*` 那一组在 api_ui 场景）。
--
-- 看什么（屏幕上从左到右）：
--   · 左侧按钮列由 scenes/ui_test.scene 的 `Container kind=vbox` 自动排列：它们的位置不是手写坐标，
--     而是容器按每个子实体 Image 的 UiLayout 尺寸累加出来的。
--   · 左键点任意按钮 → 引擎 `process_ui_clicks` 命中带 `UiEvents event=click` 的实体 → 调用
--     **工程级全局** `on_click(实体名)`（scripts/main.lua 转交 router → 本模块）。
--   · 点下去看得见三件事：
--     ① 屏幕左下状态行写出"最近一次收到的事件 + 实体名 + 计数"（`api.status`）；
--     ② 右侧 InfoPanel 里的 clicks / submits 两个数值由本脚本每帧用 `ui.text_at` 重画
--        （场景里的 Text 组件是静态的：Lua 没有"写场景实体 Text"的 API，`ui.*` 只管动态 UI）；
--     ③ 被点中的按钮外面出现一圈 `ui.panel` 高亮框。
--   · CloseButton 用 `visibility.set(id, 0)` 把 InfoPanel 那一组**场景实体**藏起来，再点一次恢复，
--     状态行里的 InfoPanel=hidden/visible 与返回串里的 panel_mask 都是**引擎侧读回**的事实
--     （`visibility.get`），不是脚本自己的布尔值。
--   · StartButton 计数 +1、ResetButton 清零、SubmitButton 提交次数 +1。
--
-- 两个"运行时不消费"的组件（因此**不写进场景文件**，否则会被 typed 组件的覆盖门判红）：
--   `UiBindings`（property=text.text;target=..;source=..）：只有 schema，只有编辑器 UI Designer
--   可编辑，运行时没有消费方 —— 本场景的"绑定"由脚本每帧重画文本代替。
--   `UiAnimation`（track.<属性>=<时间>:<值>:<缓动>）：同上 —— 右下角那条提示条的位移由脚本用
--   `time.delta()` 累计相位 + `math.sin` 驱动，效果与"一条往返位移轨道"一致。
--   对照：`ButtonStates` 同样只在编辑器侧消费（运行时渲染读 Image.texture），但它在运行时组件
--   集合里有分支（typed_scene），所以它**可以**写在场景里 —— 这是两者的区别。
--
-- 硬规则：update() 必须在 headless（无输入、无鼠标）下跑通：本模块只读实体状态 + 画动态 UI，
-- 不切场景、不写盘；顶层只定义常量与函数（不调用引擎 API）。
local api = require("api_demo")

local M = {}

-- ---------------------------------------------------------------------------
-- 常量：场景布局的镜像 + 本模块拥有的动态 UI id
-- ---------------------------------------------------------------------------

-- 五个**可点**按钮在屏幕上的矩形（1280×720）。
-- 它们由场景里的 ButtonColumn（Container kind=vbox、padding 16、spacing 12、align=center，
-- 容器矩形 (60,120)-(340,560)）算出来：每个按钮 240×48，首个孩子落在 (80,136)，逐个下移 60。
-- 为什么脚本里还要抄一份：Lua 侧没有"读场景实体解析后的 UI 矩形"的 API（命中测试在引擎内部
-- `compute_click_targets` 里做），而高亮框必须自己给坐标。改了场景里 ButtonColumn 的矩形 /
-- padding / spacing 或那个 -120,-24,120,24 的 UiLayout，这里要同步改。
-- （场景里的 DisabledButton 故意没有 UiEvents：它点不动，所以也不会进这张表。）
local BUTTON_RECTS = {
    StartButton = { 80, 136, 240, 48 },
    ResetButton = { 80, 196, 240, 48 },
    CloseButton = { 80, 256, 240, 48 },
    PointerButton = { 80, 316, 240, 48 },
    SubmitButton = { 80, 376, 240, 48 },
}

-- CloseButton 负责隐藏/恢复的**场景**实体：右侧那块九宫格 InfoPanel 与其 6 个子实体。
-- `Visibility` 是每实体一份的层遮罩，所以"整块 UI"要逐个点名；顺序与场景里的 id 一致。
local INFO_PANEL_ENTITIES = {
    "InfoPanel",
    "InfoPanelTitle",
    "InfoClicksLabel",
    "InfoSubmitsLabel",
    "InfoBindNote",
    "InfoPreviewIcon",
    "InfoPanelHint",
}

-- 动态 UI 的 id：本场景里 `ui.panel` / `ui.text_at` 建的东西全部由本模块拥有。
local ID_CLICKS = "ui_events_clicks"
local ID_SUBMITS = "ui_events_submits"
local ID_HIGHLIGHT = "ui_events_highlight"
local ID_STRIP = "ui_events_strip"
local ID_STRIP_LABEL = "ui_events_strip_label"

-- 提示条（脚本驱动的位移动画）所在的"跑道"：矩形 (620,570)-(1000,606) 是场景 UI 的空隙
-- （中间那排 Container 到 y=552 为止，右侧 InfoPanel 到 y=560 为止，左下角留给状态行）。
local STRIP_Y = 570
local STRIP_WIDTH = 200
local STRIP_HEIGHT = 36
local STRIP_CENTER_X = 800
local STRIP_SWING = 80

local clicks = 0
local submits = 0
local last_event = "none"
local last_target = "-"
local panel_visible = true
local highlighted = nil
local strip_phase = 0

-- ---------------------------------------------------------------------------
-- 内部工具
-- ---------------------------------------------------------------------------

--- 把 InfoPanel 那一组场景实体的可见层遮罩设成"全可见"或 0。
---
--- 场景 UI 没有"隐藏某个实体"的专用 API：渲染时每个实体都要过 `Visibility` 层遮罩
--- （`visibility::entity_visible_in`），因此 `visibility.set(id, 0)` 就是"藏起来"，
--- 再写回 `visibility.all()` 就是"恢复"。实体名找不到时跳过（不猜、不报错）。
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

--- 读回第一块面板实体的遮罩（0 = 藏起来了，4294967295 = 全可见）。
--- 用它而不是脚本里的布尔值，是为了让"藏起来了"这件事在 headless 日志里也可核对。
local function panel_mask()
    local id = entity.find(INFO_PANEL_ENTITIES[1])
    if not id then
        return nil
    end
    return visibility.get(id)
end

-- ---------------------------------------------------------------------------
-- 生命周期（router 分发）
-- ---------------------------------------------------------------------------

--- 进入本场景时调用一次：重置演示状态、把标题/按键提示交给 api_demo 的状态行。
function M.on_create()
    api.title("ui_test - scene UI components and UiEvents clicks")
    api.hint("LMB click a button: on_click(name) receives the entity name - " .. api.KEYS_BACK)
    clicks = 0
    submits = 0
    last_event = "none"
    last_target = "-"
    highlighted = nil
    strip_phase = 0
    -- 场景刚实例化时实体本来就是全可见的；这里显式写一次，让"进入场景 = 面板可见"成为契约
    -- （脚本状态与引擎状态因此一定一致，不会出现"变量说可见、引擎说不可见"）。
    set_panel_visible(true)
    log.info("ui_events: scene UI ready - 5 clickable buttons + 1 drag declaration, 5 container kinds")
end

--- 每个固定步一次；返回的字符串进引擎日志（`run_script_steps` / 编辑器 Play 都能看到）。
function M.update()
    -- ① 提示条位移：`UiAnimation` 组件运行时不消费（见文件头），所以这条位移动画由脚本驱动，
    --    时间仍然来自引擎：`time.delta()`（固定步长）→ 相位 → `math.sin` 做往返缓动。
    strip_phase = strip_phase + time.delta() * 1.2
    local strip_x = STRIP_CENTER_X + math.sin(strip_phase) * STRIP_SWING
    ui.panel(ID_STRIP, strip_x, STRIP_Y, STRIP_WIDTH, STRIP_HEIGHT, 0.16, 0.34, 0.52, 1.0)
    ui.text_at(ID_STRIP_LABEL, strip_x + 12, STRIP_Y + 10, "script motion", 1.0)

    -- ② 绑定文本：场景里那两个标签是静态 `Text`，数值由脚本每帧画在标签右边。
    --    `UiBindings` 运行时不消费（见文件头），所以"绑定"在这里由脚本代替。
    if panel_visible then
        ui.text_at(ID_CLICKS, 1000, 184, tostring(clicks), 1.3)
        ui.text_at(ID_SUBMITS, 1000, 220, tostring(submits), 1.3)
    else
        -- InfoPanel 整块被 visibility.set 藏起来了，脚本画的数值也必须跟着消失，
        -- 否则会留下一串浮在空处的数字。
        ui.clear(ID_CLICKS)
        ui.clear(ID_SUBMITS)
    end

    -- ③ 高亮框：动态 UI 画在**场景 UI 之下**（`RuntimeScene::vertices` 的顺序是
    --    动态面板 → 动态文本 → 场景 Text/Image），所以框画在按钮外圈 8px —— 压住按钮的
    --    那一部分会被按钮自己的 Image 盖掉，露在外面的这一圈才是看得见的高亮。
    if highlighted and BUTTON_RECTS[highlighted] then
        local rect = BUTTON_RECTS[highlighted]
        ui.panel(
            ID_HIGHLIGHT,
            rect[1] - 8,
            rect[2] - 8,
            rect[3] + 16,
            rect[4] + 16,
            1.00,
            0.82,
            0.22,
            1.0
        )
    end

    -- ④ 本帧的事实：最近一次收到的事件 + 实体名、两个计数，以及"这一帧真的画出来的那串字"
    --    （`ui.text_get` 读回动态文本：headless 下也能断言渲染口径的值）。
    local drawn = ui.text_get(ID_CLICKS)
    local drawn_text = drawn and drawn.text or "(hidden)"
    api.status(
        string.format(
            "event=%s target=%s | clicks=%d submits=%d | InfoPanel=%s | drawn=%s",
            last_event,
            last_target,
            clicks,
            submits,
            panel_visible and "visible" or "hidden",
            drawn_text
        )
    )
    return string.format(
        "ui_events_ok event=%s target=%s clicks=%d submits=%d panel_mask=%s",
        last_event,
        last_target,
        clicks,
        submits,
        tostring(panel_mask())
    )
end

--- `UiEvents event=click` 的全局回调：参数就是**实体名**（引擎 `process_ui_clicks` 的契约）。
--- 场景里 5 个按钮把 click 都绑到这个函数名上，所以这里按名字分支。
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
        -- 这个实体声明的是 pointer_enter / pointer_exit / pointer_down / pointer_up 那组事件；
        -- 运行时只分发 click（`UiEvents::click_functions`），所以这里只当"被点了"记录。
        highlighted = name
    else
        -- 别的实体名（别处动态创建的、或以后新增的按钮）：不猜语义，只上报。
        highlighted = nil
    end
    log.info(
        "ui_events: on_click "
            .. tostring(name)
            .. " clicks="
            .. tostring(clicks)
            .. " submits="
            .. tostring(submits)
    )
    return true
end

--- `ui.drag_end` 触发的全局拖拽回调（动态集合的拖拽落点）。
---
--- 本场景没有 `ui.grid`（动态网格在 api_ui 场景演示），所以它只在"别处建过动态网格"时才会被调用。
--- 场景里 DragHandle 上那条 `event=drag;function=on_drag` 是**声明**：运行时不分发场景 UiEvents
--- 的 drag 绑定（`process_ui_clicks` 只认 click），拖拽这条通路的事实源是 `ui.drag_begin` /
--- `ui.drag_end` 这对 API —— 函数名写 `on_drag` 是为了让声明指向真实存在的工程级回调。
function M.on_drag(from_id, from_index, to_id, to_index)
    last_event = "drag"
    last_target = string.format(
        "%s#%d -> %s#%d",
        tostring(from_id),
        from_index or 0,
        tostring(to_id),
        to_index or 0
    )
    log.info("ui_events: on_drag " .. last_target)
    return true
end

--- 离开场景时释放本模块建的动态 UI（`api.clear()` 由 router 负责标题/状态行/提示行）。
function M.on_destroy()
    ui.clear(ID_CLICKS)
    ui.clear(ID_SUBMITS)
    ui.clear(ID_HIGHLIGHT)
    ui.clear(ID_STRIP)
    ui.clear(ID_STRIP_LABEL)
end

return M
