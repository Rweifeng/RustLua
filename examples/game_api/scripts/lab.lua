-- Lab：game_api 2.0 的演示框架（分栏 shell / 步骤引擎 / 读回卡片 / 行为自检）。
--
-- 一个演示 = 一组**有序步骤**。每步声明四件事：
--   claim  一句话主张（页脚显示，也是读者应验证的东西）
--   api    这步真实调用的 API 名（右侧卡片的清单）
--   run    每帧执行的真实调用（headless 下必须安全：不写盘、不无条件切场景）
--   check  自检断言（`t:eq/near/ok`），由 `__api_demo_self_test()` 统一执行
--
-- 设计约束（来自引擎本体）：
--   * 动态 UI 的绘制顺序是 面板 → 图片 → 网格 → 条 → 文本；本框架的面板底与圆角图片
--     分别用 image（`lab_01_header` 等）与 text（`lab_row_*`）两层，避免互相盖住。
--   * `ui.text_at` 没有颜色参数，所以层级靠字号、缩进、分隔线和底色表达。
--   * 场景 `Text` 永远画在动态 UI **之上**：需要让位的场景文字由 `hide_scene_text` 隐藏，
--     隐藏清单在 `stop()` 时恢复。
--
-- 顶层只定义函数与常量（README §2 硬规则 1）。

local M = {}

M.WIDTH = 1280
M.HEIGHT = 720
M.KEYS_BACK = "ESC / pad B returns to the menu"

-- 版面（1280×720 设计基准，docs/74 §3.1）。
local HEADER_H = 64
local FOOTER_Y = 656
local RAIL_X, RAIL_Y, RAIL_W, RAIL_H = 16, 76, 264, 568
local CARD_X, CARD_Y, CARD_W, CARD_H = 952, 76, 312, 568
local STAGE_X, STAGE_Y, STAGE_W, STAGE_H = 296, 76, 640, 568

local ROW_TOP = 112
local ROW_STEP = 26
local MAX_STEPS = 32
local MAX_API_LINES = 8
local MAX_CODE_LINES = 6
local MAX_READ_ROWS = 7

-- 主题：底色都很暗，强调色只用于进度、选中行与结论。
local SURFACE = { 0.105, 0.125, 0.175 }
local SURFACE_DEEP = { 0.075, 0.090, 0.130 }
local ACCENT = { 0.32, 0.62, 1.00 }
local ACCENT_WARM = { 0.96, 0.66, 0.28 }
local GOOD = { 0.34, 0.80, 0.50 }
local BAD = { 0.92, 0.36, 0.36 }

local PANEL_TEX = "assets/textures/ui/lab_panel.png"
local WHITE_TEX = "assets/textures/ui/lab_white.png"
local SLICE = "14,14,14,14"

-- 模块状态：一次只服务一个演示（`router` 每场景只加载一个模块）。
local S = nil

-- ---------------------------------------------------------------------------
-- 文本度量的小工具
-- ---------------------------------------------------------------------------

--- UTF-8 拆字符（`#text` 是字节数，中文直接切会切坏）。
local function chars(text)
    local out = {}
    for ch in tostring(text):gmatch("[%z\1-\127\194-\244][\128-\191]*") do
        out[#out + 1] = ch
    end
    return out
end

--- 粗估一个字符占多少个"字号单位"：CJK 记 1.0，ASCII 记 0.56。
local function char_units(ch)
    return ch:byte(1) >= 128 and 1.0 or 0.56
end

--- 按宽度折行（近似度量，不依赖渲染帧；`size` 是像素字号）。
local function wrap(text, size, max_width)
    local limit = max_width / size
    local lines = {}
    for paragraph in (tostring(text) .. "\n"):gmatch("(.-)\n") do
        local line, width = "", 0
        for _, ch in ipairs(chars(paragraph)) do
            local unit = char_units(ch)
            if width + unit > limit and #line > 0 then
                lines[#lines + 1] = line
                line, width = "", 0
            end
            line = line .. ch
            width = width + unit
        end
        lines[#lines + 1] = line
    end
    return lines
end

--- 截断成一行（超出宽度补 ".."）。
local function fit(text, size, max_width)
    local limit = max_width / size
    local line, width = "", 0
    for _, ch in ipairs(chars(text)) do
        local unit = char_units(ch)
        if width + unit > limit then
            return line .. ".."
        end
        line = line .. ch
        width = width + unit
    end
    return line
end

local function dash(value)
    if value == nil then
        return "-"
    end
    return tostring(value)
end

-- ---------------------------------------------------------------------------
-- 绘制原语
-- ---------------------------------------------------------------------------

local function surface(id, x, y, width, height, tint)
    if width > 0 and height > 0 then
        ui.image(id, x, y, width, height, PANEL_TEX, tint[1], tint[2], tint[3], SLICE)
    end
end

local function fill(id, x, y, width, height, tint)
    if width > 0 and height > 0 then
        ui.image(id, x, y, width, height, WHITE_TEX, tint[1], tint[2], tint[3])
    end
end

local function label(id, x, y, text, size)
    ui.text_at(id, x, y, tostring(text), 1.0, size or 13)
end

-- ---------------------------------------------------------------------------
-- 步骤与断言
-- ---------------------------------------------------------------------------

local Tester = {}
Tester.__index = Tester

local function tester_new()
    return setmetatable({ checks = 0, failures = {}, step = nil }, Tester)
end

local function describe(value)
    if type(value) == "string" then
        return string.format("%q", value)
    end
    return tostring(value)
end

function Tester:fail(message)
    self.failures[#self.failures + 1] = string.format("%s: %s", self.step or "?", message)
end

function Tester:ok(condition, message)
    self.checks = self.checks + 1
    if not condition then
        self:fail(message or "expected truthy")
    end
    return condition and true or false
end

function Tester:eq(actual, expected, message)
    self.checks = self.checks + 1
    if actual ~= expected then
        self:fail(
            string.format(
                "%s (got %s, want %s)",
                message or "values differ",
                describe(actual),
                describe(expected)
            )
        )
        return false
    end
    return true
end

function Tester:near(actual, expected, epsilon, message)
    self.checks = self.checks + 1
    local ok = type(actual) == "number" and math.abs(actual - expected) <= epsilon
    if not ok then
        self:fail(
            string.format(
                "%s (got %s, want %s +/- %s)",
                message or "values differ",
                describe(actual),
                describe(expected),
                describe(epsilon)
            )
        )
        return false
    end
    return true
end

--- 受控调用：宿主帧依赖 / 允许失败的接口用它，返回 `值, 错误`。
function M.try(fn, ...)
    local ok, value, extra = pcall(fn, ...)
    if ok then
        return value, extra
    end
    return nil, tostring(value)
end

-- ---------------------------------------------------------------------------
-- 生命周期
-- ---------------------------------------------------------------------------

local function clear_shell()
    -- 图片 / 面板 / 网格由 `ui.clear` 删除；动态文本要先用 `ui.hide` 标记不可见。
    local ids = {
        "lab_01_header", "lab_03_rail", "lab_04_card", "lab_05_footer",
        "lab_06_frame_t", "lab_06_frame_b", "lab_06_frame_l", "lab_06_frame_r",
        "lab_07_track", "lab_08_fill", "lab_09_header_line", "lab_11_rule_a",
        "lab_12_rule_b", "lab_13_footer_line",
        "lab_20_sel", "lab_21_note_bg", "lab_22_focus",
        "lab_title", "lab_meta", "lab_rail_title", "lab_selftest_title", "lab_selftest",
        "lab_keys", "lab_claim", "lab_note", "lab_api_title", "lab_call_title",
        "lab_code_title", "lab_read_title", "lab_verdict",
        "lab_focus_title", "lab_focus_sub",
    }
    for i = 1, MAX_STEPS do
        ids[#ids + 1] = "lab_row_" .. i
    end
    for i = 1, MAX_API_LINES + 2 do
        ids[#ids + 1] = "lab_api_" .. i
        ids[#ids + 1] = "lab_code_" .. i
    end
    for i = 1, MAX_READ_ROWS do
        ids[#ids + 1] = "lab_read_l_" .. i
        ids[#ids + 1] = "lab_read_v_" .. i
    end
    for _, id in ipairs(ids) do
        ui.hide(id)
        ui.clear(id)
        ui.clear(id .. ":tooltip")
        ui.clear(id .. ":title")
    end
end

local function enter_current()
    if not S then
        return
    end
    local step = S.steps[S.index]
    S.elapsed = 0
    S.note = nil
    S.cache = { api = {}, code = {}, claim = {} }

    -- API 清单与代码块在进入步骤时折行一次（避免每帧量字）。
    for _, name in ipairs(step.api or {}) do
        local lines = wrap(name, 13, CARD_W - 24)
        for _, line in ipairs(lines) do
            S.cache.api[#S.cache.api + 1] = line
        end
    end
    for _, line in ipairs(wrap(step.code or "", 12, CARD_W - 24)) do
        S.cache.code[#S.cache.code + 1] = line
    end
    S.cache.claim = wrap(step.claim or step.id or "", 15, 1180)

    if step.enter then
        step.enter()
    end
end

--- 开始一个演示：`config` 见 docs/74 §3.2。
function M.start(config)
    M.stop()
    assert(type(config) == "table", "lab.start(config) 需要表参数")
    assert(type(config.steps) == "table" and #config.steps > 0, "lab.start 需要至少一个步骤")

    S = {
        config = config,
        steps = config.steps,
        index = 1,
        elapsed = 0,
        focus = false,
        note = nil,
        selftest = nil,
        results = {},
        hidden = {},
        cache = { api = {}, code = {}, claim = {} },
    }

    -- 场景文字画在动态 UI 之上：把会压到 shell 的标题/提示行让出来。
    for _, name in ipairs(config.hide_scene_text or {}) do
        ui.scene_text_visible(name, false)
        S.hidden[#S.hidden + 1] = name
    end

    enter_current()
    log.info(string.format("lab: scene=%s steps=%d", dash(config.scene), #config.steps))
end

--- 结束演示：清掉全部 shell 元素并恢复场景文字。
function M.stop()
    if not S then
        return
    end
    clear_shell()
    for _, name in ipairs(S.hidden) do
        ui.scene_text_visible(name, true)
    end
    S = nil
end

function M.active()
    return S ~= nil
end

function M.index()
    return S and S.index or 0
end

function M.step()
    return S and S.steps[S.index] or nil
end

--- 页脚/舞台的实时说明（每帧由步骤设置）。
function M.note(text)
    if S then
        S.note = tostring(text)
    end
end

--- 在舞台底部画一行状态（步骤的 `run` 里调用）。
function M.status(text)
    M.note(text)
end

function M.go(index)
    if not S then
        return
    end
    local next_index = math.max(1, math.min(#S.steps, index))
    if next_index == S.index then
        enter_current()
        return
    end
    S.index = next_index
    enter_current()
end

function M.next()
    M.go((S and S.index or 1) + 1)
end

function M.prev()
    M.go((S and S.index or 1) - 1)
end

--- 复位当前步骤（重新执行 `enter`，用于把被交互改乱的世界恢复）。
function M.restart()
    enter_current()
end

-- ---------------------------------------------------------------------------
-- 行为自检
-- ---------------------------------------------------------------------------

--- 依次执行每个步骤的 enter → run × 3 → check，返回一行结论。
---
--- 这个函数必须在 headless 下可用：它只做内存读写（步骤的 `run` 遵守 README 硬规则 2）。
function M.self_test()
    if not S then
        return "FAIL no active lab"
    end
    local tester = tester_new()
    local saved = S.index
    local results = {}

    for i, step in ipairs(S.steps) do
        S.index = i
        tester.step = step.id or ("step" .. i)
        local before = #tester.failures
        local ok, err = pcall(function()
            enter_current()
            for _ = 1, 3 do
                if step.run then
                    step.run(1.0 / 60.0)
                end
            end
            if step.check then
                local passed, reason = step.check(tester)
                if passed == false then
                    tester:fail(reason or "check() returned false")
                end
            else
                tester:fail("step has no check()")
            end
        end)
        if not ok then
            tester:fail("raised error: " .. tostring(err))
        end
        results[i] = (#tester.failures == before) and "ok" or "fail"
    end

    S.index = saved
    S.results = results
    enter_current()

    local summary
    if #tester.failures == 0 then
        summary = string.format("ok steps=%d checks=%d failed=0", #S.steps, tester.checks)
    else
        local shown = {}
        for i = 1, math.min(#tester.failures, 6) do
            shown[#shown + 1] = tester.failures[i]
        end
        summary = string.format(
            "FAIL steps=%d checks=%d failed=%d | %s",
            #S.steps,
            tester.checks,
            #tester.failures,
            table.concat(shown, " | ")
        )
    end
    S.selftest = summary
    return summary
end

-- ---------------------------------------------------------------------------
-- 每帧：导航 → 步骤推进 → 绘制
-- ---------------------------------------------------------------------------

local function handle_keys()
    if input.just_pressed("f") then
        S.focus = not S.focus
        return
    end
    if input.just_pressed("f5") then
        M.self_test()
    end
    if input.just_pressed("r") then
        M.restart()
        return
    end
    if input.just_pressed("space") or input.just_pressed("right") or input.just_pressed("pagedown") then
        M.next()
        return
    end
    if input.just_pressed("left") or input.just_pressed("backspace") or input.just_pressed("pageup") then
        M.prev()
        return
    end
    if input.just_pressed("home") then
        M.go(1)
        return
    end
    if input.just_pressed("end") then
        M.go(#S.steps)
        return
    end
    for i = 1, math.min(9, #S.steps) do
        if input.just_pressed(tostring(i)) then
            M.go(i)
            return
        end
    end
end

local function read_rows(step)
    local rows = {}
    if not step.read then
        return rows
    end
    local value = step.read()
    if type(value) ~= "table" then
        return rows
    end
    if #value > 0 then
        for _, row in ipairs(value) do
            if type(row) == "table" then
                rows[#rows + 1] = { dash(row.label or row[1]), dash(row.value or row[2]) }
            end
        end
    else
        local keys = {}
        for key in pairs(value) do
            keys[#keys + 1] = key
        end
        table.sort(keys, function(a, b)
            return tostring(a) < tostring(b)
        end)
        for _, key in ipairs(keys) do
            rows[#rows + 1] = { dash(key), dash(value[key]) }
        end
    end
    return rows
end

local function draw_header()
    surface("lab_01_header", 0, 0, M.WIDTH, HEADER_H, { 0.115, 0.140, 0.195 })
    fill("lab_09_header_line", 0, HEADER_H - 2, M.WIDTH, 2, ACCENT)
    label("lab_title", 24, 16, S.config.title or S.config.scene or "api lab", 24)

    local counter = string.format(
        "%s   %02d / %02d",
        S.config.category or "api",
        S.index,
        #S.steps
    )
    local width = 0
    for _, ch in ipairs(chars(counter)) do
        width = width + char_units(ch) * 14
    end
    label("lab_meta", M.WIDTH - 24 - width, 24, counter, 14)

    local ratio = S.index / #S.steps
    fill("lab_07_track", 24, 52, 300, 4, { 0.22, 0.26, 0.34 })
    if ratio > 0 then
        fill("lab_08_fill", 24, 52, math.max(2, 300 * ratio), 4, ACCENT)
    end
end

local function draw_rail()
    surface("lab_03_rail", RAIL_X, RAIL_Y, RAIL_W, RAIL_H, SURFACE)
    label("lab_rail_title", RAIL_X + 16, RAIL_Y + 14, "STEPS", 11)

    for i, step in ipairs(S.steps) do
        local y = ROW_TOP + (i - 1) * ROW_STEP
        if i == S.index then
            surface("lab_20_sel", RAIL_X + 8, y - 5, RAIL_W - 16, 24, { 0.18, 0.34, 0.58 })
        end
        local title = fit(step.title or step.id or ("step " .. i), 13, 200)
        local marker = ""
        if S.results[i] == "ok" then
            marker = "  ok"
        elseif S.results[i] == "fail" then
            marker = "  !! "
        end
        label(
            "lab_row_" .. i,
            RAIL_X + 18,
            y,
            string.format("%02d  %s%s", i, title, marker),
            13
        )
    end

    label("lab_selftest_title", RAIL_X + 16, RAIL_Y + RAIL_H - 74, "SELF-TEST", 11)
    local text = S.selftest or "F5 runs every step's check()"
    local lines = wrap(text, 11, RAIL_W - 32)
    for i = 1, math.min(#lines, 3) do
        label("lab_selftest", RAIL_X + 16, RAIL_Y + RAIL_H - 54 + (i - 1) * 14, lines[i], 11)
    end
end

local function draw_card()
    surface("lab_04_card", CARD_X, CARD_Y, CARD_W, CARD_H, SURFACE)
    local x = CARD_X + 16
    local y = CARD_Y + 16
    label("lab_api_title", x, y, "API", 11)
    y = y + 22

    for i = 1, math.min(#S.cache.api, MAX_API_LINES) do
        label("lab_api_" .. i, x, y, S.cache.api[i], 13)
        y = y + 18
    end
    y = y + 6
    fill("lab_11_rule_a", x, y, CARD_W - 32, 1, { 0.24, 0.28, 0.36 })
    y = y + 12

    label("lab_call_title", x, y, "CALL", 11)
    y = y + 20
    for i = 1, math.min(#S.cache.code, MAX_CODE_LINES) do
        label("lab_code_" .. i, x, y, S.cache.code[i], 12)
        y = y + 16
    end
    y = y + 8
    fill("lab_12_rule_b", x, y, CARD_W - 32, 1, { 0.24, 0.28, 0.36 })
    y = y + 12

    label("lab_read_title", x, y, "READ", 11)
    y = y + 22
    local rows = read_rows(S.steps[S.index])
    for i = 1, math.min(#rows, MAX_READ_ROWS) do
        local row = rows[i]
        label("lab_read_l_" .. i, x, y, fit(row[1], 12, 108), 12)
        label("lab_read_v_" .. i, x + 112, y, fit(row[2], 12, CARD_W - 144), 12)
        y = y + 19
    end

    local verdict = S.results[S.index]
    if verdict == "ok" then
        label("lab_verdict", x, CARD_Y + CARD_H - 28, "CHECK  ok", 12)
    elseif verdict == "fail" then
        label("lab_verdict", x, CARD_Y + CARD_H - 28, "CHECK  failed", 12)
    else
        label("lab_verdict", x, CARD_Y + CARD_H - 28, "F5  verify all steps", 12)
    end
end

local function draw_footer()
    surface("lab_05_footer", 0, FOOTER_Y, M.WIDTH, M.HEIGHT - FOOTER_Y, { 0.090, 0.110, 0.155 })
    fill("lab_13_footer_line", 0, FOOTER_Y, M.WIDTH, 2, { 0.20, 0.32, 0.52 })
    label(
        "lab_keys",
        24,
        FOOTER_Y + 8,
        "SPACE / -> next   <- previous   R restart   F focus   F5 self-test   ESC menu",
        12
    )

    local claim = S.cache.claim or wrap(S.steps[S.index].claim or "", 15, 1180)
    local text = claim[1] or ""
    if S.note then
        text = text .. "   |   " .. S.note
    end
    label("lab_claim", 24, FOOTER_Y + 28, fit(text, 15, 1180), 15)
end

local function draw_focus()
    surface("lab_22_focus", 16, M.HEIGHT - 128, 560, 112, { 0.075, 0.090, 0.130 })
    label("lab_focus_title", 32, M.HEIGHT - 112, fit(S.config.title or "", 18, 520), 18)
    label(
        "lab_focus_sub",
        32,
        M.HEIGHT - 84,
        fit(string.format("%02d/%02d  %s", S.index, #S.steps, S.steps[S.index].claim or ""), 13, 520),
        13
    )
    label("lab_verdict", 32, M.HEIGHT - 56, "F returns to the lab shell", 12)
end

local function draw()
    if S.focus then
        draw_focus()
        return
    end
    draw_header()
    draw_rail()
    draw_card()
    draw_footer()

    -- 舞台取景框（细边，不铺满，世界仍然可见）。
    ui.panel("lab_06_frame_t", STAGE_X, STAGE_Y, STAGE_W, 2, 0.16, 0.20, 0.28, 1.0)
    ui.panel("lab_06_frame_b", STAGE_X, STAGE_Y + STAGE_H - 2, STAGE_W, 2, 0.16, 0.20, 0.28, 1.0)
    ui.panel("lab_06_frame_l", STAGE_X, STAGE_Y, 2, STAGE_H, 0.16, 0.20, 0.28, 1.0)
    ui.panel("lab_06_frame_r", STAGE_X + STAGE_W - 2, STAGE_Y, 2, STAGE_H, 0.16, 0.20, 0.28, 1.0)

    if S.note then
        local width = math.min(600, #S.note * 7 + 24)
        surface("lab_21_note_bg", STAGE_X + 12, STAGE_Y + STAGE_H - 40, width, 28, SURFACE_DEEP)
        label("lab_note", STAGE_X + 24, STAGE_Y + STAGE_H - 34, fit(S.note, 13, width - 24), 13)
    end
end

--- 每帧入口：导航键 → 当前步骤的 run → 重绘 shell。返回进日志的一行。
function M.tick()
    if not S then
        return "lab_inactive"
    end
    handle_keys()
    local step = S.steps[S.index]
    S.elapsed = S.elapsed + time.delta()
    if step.run then
        step.run(time.delta())
    end
    draw()
    return string.format("lab scene=%s step=%s", dash(S.config.scene), dash(step.id))
end

-- 供调试/自检读回的只读状态。
function M.state()
    if not S then
        return nil
    end
    return {
        scene = S.config.scene,
        index = S.index,
        steps = #S.steps,
        elapsed = S.elapsed,
        selftest = S.selftest,
        focus = S.focus,
    }
end

-- 主题常量给入口菜单复用（同一套配色）。
M.THEME = {
    surface = SURFACE,
    surface_deep = SURFACE_DEEP,
    accent = ACCENT,
    accent_warm = ACCENT_WARM,
    good = GOOD,
    bad = BAD,
}

return M
