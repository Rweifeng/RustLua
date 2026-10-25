-- api_input：input 模块的 2.0 分步 Lab（物理键 / 命名动作 / 绑定 / 指针与手柄 / 设置）。
--
-- 一条主线：**物理键层只认键，命名动作层只认绑定，绑定文本可往返，设置与诊断是旁证**。
-- 本文件的每一步都能在右侧卡片读到"本帧刚读到的真实值"；按 F5 会依次跑
-- enter → run × 3 → check，把"读回语义已经错了"这类漂移当场暴露出来。
--
-- 为什么每条 `enter` 都要复位：读者可能随手按过 Y（rebind）/ U（unbind）/ I（reset），
-- 若自检前不复位，F5 的结论就依赖"上一次操作"，不再可重复。
--
-- 交互键（Lab 保留 space/方向键/1..9/R/F/F5，这里只用字母）：
--   V 注入/释放虚拟键（触屏摇杆语义）  Y rebind  U unbind  I reset  O 映射往返
--   K save_settings  J load_settings  P 导航阈值 +0.1  N 震动请求
--
-- 硬规则（README §2）：顶层只定义常量与函数；`run` 在 headless 下安全；
-- 唯一写盘的 `input.save_settings()` 只在按键分支里。
local lab = require("lab")

local M = {}

-- 场景实体名（scenes/api_input.scene）。
local PROBE, KEY, MOUSE, PAD = "Probe", "Key", "Mouse", "Pad"

-- 舞台是 x 296..936、y 76..644；仪表全部留在这个框里，底部 604 起留给 lab.note。
local KEY_BOX = { x0 = 340, y0 = 120, x1 = 640, y1 = 230 }
local MOUSE_BOX = { x0 = 330, y0 = 300, x1 = 900, y1 = 400 }
local PAD_BOX = { x0 = 340, y0 = 450, x1 = 900, y1 = 550 }
local PROBE_HOME = { x = 700, y = 300 }
local SQUARE = 40

-- 物理键清单（全部是合法 v2 键名；拼错的键名会直接报 Lua 错误，所以不拼名）。
local HELD_KEYS = { "keyw", "keys", "keya", "keyd", "arrowleft", "arrowright", "arrowup", "arrowdown" }
local EDGE_KEYS = {
    { "space", "space" },
    { "keye", "E" },
    { "keyq", "Q" },
    { "arrowup", "up" },
    { "arrowdown", "down" },
    { "arrowleft", "left" },
    { "arrowright", "right" },
}
-- 状态卡片里逐名查询的动作（动作表有几十项，只挑能说明"动作层可枚举"的几条）。
local ACTION_NAMES = { "move_up", "move_down", "move_left", "move_right", "interact", "confirm" }

-- 导航手感默认值（与 engine 的 NavSettings::DEFAULT 一致）。
local NAV_DEFAULT = { threshold = 0.5, delay_ms = 350, repeat_ms = 90 }

local ids = {}
local virtual_held = false

-- 脚本侧观察值（每帧由 run 重新算；不做业务累积，避免读数漂移）。
local obs = {
    axis_x = 0,
    axis_y = 0,
    held = "-",
    pressed = "-",
    released = "-",
    action_count = 0,
    interact = false,
    action_pressed = false,
    action_axis = 0,
    device = "keyboard",
    cursor = { 0, 0 },
    delta = { 0, 0 },
    scroll = { 0, 0 },
    press = nil,
    pointers = 0,
    pad_a = false,
    stick = { 0, 0 },
    rumbles = 0,
    nav = { threshold = NAV_DEFAULT.threshold, delay_ms = NAV_DEFAULT.delay_ms, repeat_ms = NAV_DEFAULT.repeat_ms },
    eff = { threshold = NAV_DEFAULT.threshold, delay_ms = NAV_DEFAULT.delay_ms, repeat_ms = NAV_DEFAULT.repeat_ms },
    settings_path = "",
    diagnostics = 0,
    notes = 0,
    map_lines = 0,
    map_version = "?",
    bindings = "-",
    op = "ready",
}

-- ---------------------------------------------------------------------------
-- 纯值工具（不碰引擎 API）
-- ---------------------------------------------------------------------------

local function clamp(value, low, high)
    if value < low then
        return low
    end
    if value > high then
        return high
    end
    return value
end

local function lerp(low, high, t)
    return low + (high - low) * t
end

--- 把 -1..1 映射成 0..1（输入模拟量 → 仪表位置/颜色强度）。
local function unit(value)
    return clamp((value + 1.0) * 0.5, 0.0, 1.0)
end

--- 把输入量夹到仪表框里（返回方块中心点，保证动态内容不出舞台）。
local function place(box, x, y)
    return lerp(box.x0, box.x1, unit(x)), lerp(box.y0, box.y1, unit(y))
end

--- 浮点短文本：整数值不显示小数点，便于一眼读。
local function short(value)
    if math.abs(value - math.floor(value + 0.5)) < 0.001 then
        return string.format("%d", math.floor(value + 0.5))
    end
    return string.format("%.2f", value)
end

--- 拼接字符串数组（空表返回 "-"）。
local function join_lines(list)
    if list == nil or #list == 0 then
        return "-"
    end
    local parts = {}
    for index = 1, #list do
        parts[index] = tostring(list[index])
    end
    return table.concat(parts, " / ")
end

-- ---------------------------------------------------------------------------
-- 场景 / 状态工具
-- ---------------------------------------------------------------------------

--- 按名字取实体；场景被改名/删实体时返回 nil，调用点必须先判空（不硬崩）。
local function find_ids()
    ids.probe = entity.find(PROBE)
    ids.key = entity.find(KEY)
    ids.mouse = entity.find(MOUSE)
    ids.pad = entity.find(PAD)
end

--- 释放本模块注入的虚拟键。虚拟键是**跨帧保持**的按住态（触屏摇杆语义），
--- 所以每一步进入时都要显式抬起，否则上一步的注入会污染这一帧的动作读数。
local function release_virtual()
    input.virtual_key("keye", false)
    input.virtual_key("keyv", false)
    input.virtual_key("keyp", false)
    virtual_held = false
end

--- 本帧按住的物理键（只显示按住的，空 = "-"）。
local function held_text()
    local parts = {}
    for _, key in ipairs(HELD_KEYS) do
        if input.is_down(key) then
            parts[#parts + 1] = (string.gsub(string.gsub(key, "^key", ""), "^arrow", ""))
        end
    end
    if #parts == 0 then
        return "-"
    end
    return table.concat(parts, ",")
end

--- 本帧的边沿：`just_pressed` 与别名 `pressed`（`just_released` / `released`）同义，
--- 各查一次是为了证明两个名字给的是同一份边沿集合。
local function edge_text(pressed_side)
    local parts = {}
    for _, entry in ipairs(EDGE_KEYS) do
        local hit
        if pressed_side then
            hit = input.just_pressed(entry[1]) or input.pressed(entry[1])
        else
            hit = input.just_released(entry[1]) or input.released(entry[1])
        end
        if hit then
            parts[#parts + 1] = entry[2]
        end
    end
    if #parts == 0 then
        return "-"
    end
    return table.concat(parts, ",")
end

--- `input.action(name)` 逐名查询：列出此刻按下的命名动作（动作层与物理键层互不影响）。
local function action_text()
    local parts = {}
    for _, name in ipairs(ACTION_NAMES) do
        if input.action(name) then
            parts[#parts + 1] = name
        end
    end
    if #parts == 0 then
        return "-"
    end
    return table.concat(parts, ",")
end

--- `input.bindings(name)` → "动作=key:xxx|pad:n"（空绑定也写出来，因为"为空"本身是事实）。
local function bindings_text(name)
    local list = input.bindings(name)
    if list == nil or #list == 0 then
        return name .. "=(empty)"
    end
    local parts = {}
    for index = 1, #list do
        parts[index] = tostring(list[index])
    end
    return name .. "=" .. table.concat(parts, "|")
end

--- `action_map_text()` 的行数与版本头（映射文本的契约就写在第一行）。
local function map_head(text)
    local lines = 0
    for _ in string.gmatch(text, "[^\n]+") do
        lines = lines + 1
    end
    return lines, string.gsub(text, "\n.*", "")
end

--- 设备指示灯：画在舞台右上角，位置留在框内（x 880..906、y 100..126）。
local function draw_device_lamp(device)
    local r, g, b = 0.32, 0.62, 1.00
    if device == "gamepad" then
        r, g, b = 0.34, 0.90, 0.45
    end
    ui.image("input_device_lamp", 880, 100, 26, 26, "__white__", r, g, b)
end

-- ---------------------------------------------------------------------------
-- 步骤
-- ---------------------------------------------------------------------------

local steps = {
    {
        id = "physical_layer",
        title = "Physical keys",
        claim = "物理键层只认键位：WASD/方向键的 axis 与按键边沿都与绑定无关",
        api = { "input.is_down", "input.just_pressed", "input.just_released", "input.pressed", "input.released", "input.axis" },
        code = 'input.axis("horizontal"); input.is_down("keyw"); input.pressed("space")',
        enter = function()
            release_virtual()
            find_ids()
            input.reset_bindings()
        end,
        run = function()
            obs.axis_x = input.axis("horizontal")
            obs.axis_y = input.axis("vertical")
            obs.held = held_text()
            obs.pressed = edge_text(true)
            obs.released = edge_text(false)

            -- Key 金方块：横向跟 horizontal、纵向跟 vertical；颜色强度跟"本帧是否有边沿"。
            local kx = lerp(KEY_BOX.x0, KEY_BOX.x1, unit(obs.axis_x))
            local ky = lerp(KEY_BOX.y0, KEY_BOX.y1, unit(-obs.axis_y))
            if ids.key then
                transform.set(ids.key, kx, ky)
            end

            -- Probe 蓝方块：同一份 axis 再驱动一次，说明"读同一个量不会改状态"。
            if ids.probe then
                local px, py = place(MOUSE_BOX, obs.axis_x, -obs.axis_y)
                transform.set(ids.probe, px, math.max(KEY_BOX.y0, py - 120))
            end

            obs.op = string.format("axis h=%s v=%s held=%s", short(obs.axis_x), short(obs.axis_y), obs.held)
            lab.note(obs.op .. " | press W/A/S/D or arrows to move the squares")
        end,
        read = function()
            return {
                { label = "axis h", value = short(obs.axis_x) },
                { label = "axis v", value = short(obs.axis_y) },
                { label = "held", value = obs.held },
                { label = "pressed", value = obs.pressed },
                { label = "released", value = obs.released },
            }
        end,
        check = function(t)
            t:eq(type(obs.axis_x), "number", "axis() returns a number")
            t:near(input.axis("no_such_axis"), 0, 0.0001, "an unknown axis name reads as 0")
            t:eq(type(input.is_down("keyw")), "boolean", "is_down returns a boolean")
        end,
    },
    {
        id = "named_actions",
        title = "Named actions",
        claim = "虚拟键也能驱动命名动作：keye 按住 = interact 按下，action_pressed 看到边沿",
        api = { "input.action", "input.action_pressed", "input.action_released", "input.action_axis", "input.action_repeat", "input.actions", "input.last_device", "input.virtual_key", "input.virtual_pressed" },
        code = 'input.virtual_key("keye", true); input.action("interact")',
        enter = function()
            release_virtual()
            find_ids()
            input.reset_bindings()
            -- 触屏摇杆的写法：注入按住态 + 一帧按下边沿。两者都**只走动作层**，
            -- 因此 `input.is_down("keye")` 仍为 false（物理键层没被伪造）。
            input.virtual_key("keye", true)
            input.virtual_pressed("keye")
            virtual_held = true
        end,
        run = function()
            local names = input.actions()
            obs.action_count = #names
            obs.interact = input.action("interact")
            obs.action_pressed = input.action_pressed("interact")
            obs.action_axis = input.action_axis("ui_left", "ui_right")
            obs.device = input.last_device()

            if ids.probe then
                local px = PROBE_HOME.x + (obs.interact and 40 or 0)
                local py = PROBE_HOME.y - (input.action("move_up") and 30 or 0)
                transform.set(ids.probe, px, py)
            end

            obs.op = string.format(
                "actions=%d interact=%s axis_ui=%s repeat=%s",
                obs.action_count,
                tostring(obs.interact),
                short(obs.action_axis),
                tostring(input.action_repeat("ui_right"))
            )
            lab.note(obs.op .. " | V toggles the injected virtual key")
        end,
        read = function()
            return {
                { label = "actions", value = obs.action_count },
                { label = "interact", value = tostring(obs.interact) },
                { label = "pressed", value = tostring(obs.action_pressed) },
                { label = "ui axis", value = short(obs.action_axis) },
                { label = "device", value = obs.device },
            }
        end,
        check = function(t)
            t:ok(#input.actions() >= 20, "actions() lists the default action table")
            t:eq(input.action("interact"), true, "a held virtual keye keeps interact down")
            t:eq(input.action_pressed("interact"), true, "virtual_pressed produces a press edge")
            t:eq(type(input.action_released("interact")), "boolean", "action_released returns a boolean")
            t:eq(type(input.action_axis("ui_left", "ui_right")), "number", "action_axis returns a number")
            t:eq(type(input.action_repeat("ui_right")), "boolean", "action_repeat returns a boolean")
            local device = input.last_device()
            t:ok(device == "keyboard" or device == "gamepad", "last_device is one of the two device classes")
        end,
    },
    {
        id = "bindings",
        title = "Bindings",
        claim = "binding 文本可往返：rebind → bindings 读回新键 → action_map_text 无损回灌",
        api = { "input.bindings", "input.rebind", "input.unbind", "input.reset_bindings", "input.action_map_text", "input.set_action_map" },
        code = 'input.rebind("interact", "key:digit7"); input.set_action_map(input.action_map_text())',
        enter = function()
            release_virtual()
            find_ids()
            input.reset_bindings()
            obs.op = "defaults restored"
        end,
        run = function()
            obs.bindings = bindings_text("interact")
            obs.map_lines, obs.map_version = map_head(input.action_map_text())
            obs.op = string.format("%s | map=%s(%d lines)", obs.bindings, obs.map_version, obs.map_lines)
            lab.note(obs.op .. " | Y rebind digit7  U unbind  I reset  O round-trip")
        end,
        read = function()
            return {
                { label = "interact", value = obs.bindings },
                { label = "map head", value = obs.map_version },
                { label = "map lines", value = obs.map_lines },
                { label = "last op", value = obs.op },
            }
        end,
        check = function(t)
            input.reset_bindings()
            local ok = input.rebind("interact", "key:digit7")
            t:eq(ok, true, "rebind accepts a physical key name")
            local list = input.bindings("interact")
            t:ok(#list == 1 and list[1] == "key:digit7", "bindings() reports the new key")

            local text = input.action_map_text()
            t:ok(#text > 0, "action_map_text() is non-empty")
            local roundtrip = input.set_action_map(text)
            t:eq(roundtrip, true, "set_action_map accepts the text it just produced")
            t:eq(input.action_map_text(), text, "text round-trip is lossless")

            local bad = input.set_action_map("rustlua-actions-v9\ninteract=key:keye")
            t:eq(bad, false, "a bad version header is rejected")
            t:eq(input.action_map_text(), text, "a rejected map leaves the current one untouched")

            t:eq(input.unbind("interact", "key:digit7"), true, "unbind removes one binding")
            input.reset_bindings()
            local defaults = input.bindings("interact")
            t:eq(defaults[1], "key:keye", "reset_bindings restores the default key")
        end,
    },
    {
        id = "pointers_pad",
        title = "Pointer & pad",
        claim = "光标/滚轮返回 [x,y] 数组，按下点随多指索引，手柄按钮与轴形状固定",
        api = { "input.cursor_position", "input.cursor_delta", "input.scroll_delta", "input.press_position", "input.pointer_count", "input.mouse_is_down", "input.mouse_just_pressed", "input.mouse_just_released", "input.pad_button", "input.pad_axis", "input.rumble" },
        code = 'input.press_position(); input.pad_button(0); input.pad_axis(0); input.rumble(0.6, 0.3, 200)',
        enter = function()
            release_virtual()
            find_ids()
            input.reset_bindings()
            obs.rumbles = 0
        end,
        run = function()
            obs.cursor = input.cursor_position()
            obs.delta = input.cursor_delta()
            obs.scroll = input.scroll_delta()
            obs.press = input.press_position()
            obs.pointers = input.pointer_count()
            obs.pad_a = input.pad_button(0)
            obs.stick = { input.pad_axis(0), input.pad_axis(1) }
            -- 鼠标边沿只在这里读一次（check 不碰宿主输入；headless 下恒为 false）。
            local mouse_down = input.mouse_is_down("left")
            local mouse_pressed = input.mouse_just_pressed("left")
            local mouse_released = input.mouse_just_released("left")

            -- Mouse 绿方块：中心跟光标，pressed 边沿的动画由 lab.note 读出来。
            if ids.mouse then
                local mx, my = place(MOUSE_BOX, obs.cursor[1] / 640.0 - 1.0, 1.0 - obs.cursor[2] / 360.0)
                transform.set(ids.mouse, mx, my)
            end
            -- Pad 粉方块：左摇杆的模拟量（无手柄时安静停在框中心）。
            if ids.pad then
                local pad_x, pad_y = place(PAD_BOX, obs.stick[1], obs.stick[2])
                transform.set(ids.pad, pad_x, pad_y)
            end

            obs.op = string.format(
                "cursor=%s,%s pointers=%d press=%s pad A=%s stick=%s,%s mouse=%s/%s/%s rumble=%d",
                short(obs.cursor[1]),
                short(obs.cursor[2]),
                obs.pointers,
                obs.press and string.format("%s,%s", short(obs.press[1]), short(obs.press[2])) or "nil",
                tostring(obs.pad_a),
                short(obs.stick[1]),
                short(obs.stick[2]),
                tostring(mouse_down),
                tostring(mouse_pressed),
                tostring(mouse_released),
                obs.rumbles
            )
            lab.note(obs.op .. " | N sends one rumble request (silent without a pad)")
        end,
        read = function()
            return {
                { label = "cursor", value = string.format("%s,%s", short(obs.cursor[1]), short(obs.cursor[2])) },
                { label = "delta", value = string.format("%s,%s", short(obs.delta[1]), short(obs.delta[2])) },
                { label = "scroll", value = string.format("%s,%s", short(obs.scroll[1]), short(obs.scroll[2])) },
                { label = "pointers", value = obs.pointers },
                { label = "pad A", value = tostring(obs.pad_a) },
                { label = "stick", value = string.format("%s,%s", short(obs.stick[1]), short(obs.stick[2])) },
                { label = "rumbles", value = obs.rumbles },
            }
        end,
        check = function(t)
            local cursor = input.cursor_position()
            t:eq(#cursor, 2, "cursor_position is a two-element [x, y]")
            t:eq(type(cursor[1]), "number", "cursor x is a number")
            t:eq(#input.cursor_delta(), 2, "cursor_delta is a two-element [x, y]")
            t:eq(#input.scroll_delta(), 2, "scroll_delta is a two-element [x, y]")
            local press = input.press_position()
            t:ok(press == nil or #press == 2, "press_position is nil or a two-element [x, y]")
            t:eq(type(input.pointer_count()), "number", "pointer_count is an integer")
            t:eq(type(input.mouse_is_down("left")), "boolean", "mouse_is_down returns a boolean")
            t:eq(type(input.pad_button(0)), "boolean", "pad_button returns a boolean")
            t:eq(type(input.pad_axis(0)), "number", "pad_axis returns a number")
        end,
    },
    {
        id = "settings",
        title = "Settings & nav",
        claim = "设置路径可读、导航手感被钳制后回传；诊断与迁移说明是两份独立的数组",
        api = { "input.settings_path", "input.load_settings", "input.save_settings", "input.nav_settings", "input.set_nav_settings", "input.diagnostics", "input.migration_notes" },
        code = 'input.settings_path(); input.set_nav_settings(0.5, 350, 90); input.save_settings()',
        enter = function()
            release_virtual()
            find_ids()
            input.reset_bindings()
            -- load_settings 是**读**（文件缺失返回 false,"missing" 是正常路径），用 lab.try 容错；
            -- 随后把导航手感复位到默认值，保证自检与用户先前操作无关。
            lab.try(function()
                return input.load_settings()
            end)
            local nav = lab.try(function()
                return input.nav_settings()
            end)
            if type(nav) == "table" and nav.threshold ~= nil then
                obs.nav.threshold = nav.threshold
                obs.nav.delay_ms = nav.delay_ms
                obs.nav.repeat_ms = nav.repeat_ms
            else
                obs.nav.threshold = NAV_DEFAULT.threshold
                obs.nav.delay_ms = NAV_DEFAULT.delay_ms
                obs.nav.repeat_ms = NAV_DEFAULT.repeat_ms
            end
            -- 统一推回默认手感：这一步的断言只依赖"钳制函数的返回值"，不依赖设置文件内容。
            obs.nav.threshold = NAV_DEFAULT.threshold
            obs.nav.delay_ms = NAV_DEFAULT.delay_ms
            obs.nav.repeat_ms = NAV_DEFAULT.repeat_ms
            local threshold, delay_ms, repeat_ms = input.set_nav_settings(
                obs.nav.threshold,
                obs.nav.delay_ms,
                obs.nav.repeat_ms
            )
            obs.eff.threshold = threshold
            obs.eff.delay_ms = delay_ms
            obs.eff.repeat_ms = repeat_ms
        end,
        run = function()
            obs.settings_path = input.settings_path() or ""
            obs.diagnostics = #input.diagnostics()
            obs.notes = #input.migration_notes()
            local threshold, delay_ms, repeat_ms = input.set_nav_settings(
                obs.nav.threshold,
                obs.nav.delay_ms,
                obs.nav.repeat_ms
            )
            obs.eff.threshold = threshold
            obs.eff.delay_ms = delay_ms
            obs.eff.repeat_ms = repeat_ms
            obs.op = string.format(
                "nav=%.2f/%d/%d diagnostics=%d migration=%d",
                obs.eff.threshold,
                obs.eff.delay_ms,
                obs.eff.repeat_ms,
                obs.diagnostics,
                obs.notes
            )
            lab.note(obs.op .. " | K save  J load  P threshold +0.1")
        end,
        read = function()
            return {
                { label = "path", value = obs.settings_path == "" and "(none)" or obs.settings_path },
                { label = "nav threshold", value = string.format("%.2f", obs.eff.threshold) },
                { label = "nav delay", value = obs.eff.delay_ms },
                { label = "nav repeat", value = obs.eff.repeat_ms },
                { label = "diagnostics", value = obs.diagnostics },
                { label = "migration", value = obs.notes },
            }
        end,
        check = function(t)
            local path = input.settings_path()
            t:ok(path ~= nil and #path > 0, "settings_path() points at the settings file")

            local threshold, delay_ms, repeat_ms = input.set_nav_settings(0.5, 350, 90)
            t:near(threshold, 0.5, 0.001, "set_nav_settings returns the effective threshold")
            t:eq(delay_ms, 350, "delay_ms passes through")
            t:eq(repeat_ms, 90, "repeat_ms passes through")

            local clamped_threshold, clamped_delay, clamped_repeat = input.set_nav_settings(9, 99999, 0)
            t:near(clamped_threshold, 1.0, 0.001, "threshold is clamped to 1")
            t:eq(clamped_delay, 5000, "delay is clamped to MAX_MS")
            t:eq(clamped_repeat, 16, "repeat is clamped to MIN_REPEAT_MS")

            t:eq(type(input.diagnostics()), "table", "diagnostics() is an array")
            t:eq(type(input.migration_notes()), "table", "migration_notes() is an array")

            -- 复位：下一步（或读者继续操作）看到的是默认手感。
            input.set_nav_settings(0.5, 350, 90)
            obs.nav.threshold = 0.5
            obs.nav.delay_ms = 350
            obs.nav.repeat_ms = 90
        end,
    },
}

-- ---------------------------------------------------------------------------
-- 生命周期
-- ---------------------------------------------------------------------------

function M.on_create()
    find_ids()
    lab.start({
        scene = "api_input",
        title = "Input API",
        category = "Input",
        steps = steps,
        -- 场景 Text 永远画在动态 UI 之上：标题与四行仪表标签都会压住 shell，全部让位。
        hide_scene_text = { "Hint", "ProbeLabel", "KeyLabel", "MouseLabel", "PadLabel" },
    })
    log.info("api_input: lab ready (physical keys / actions / bindings / pointer+pad / settings)")
end

-- 按步骤分支的字母键交互（Lab 保留 space/方向键/1..9/R/F/F5，这里不碰）。
function M.update()
    local step = lab.step()
    if step then
        if step.id == "named_actions" then
            if input.just_pressed("keyv") then
                if virtual_held then
                    release_virtual()
                    obs.op = "virtual keye released"
                else
                    input.virtual_key("keye", true)
                    input.virtual_pressed("keye")
                    virtual_held = true
                    obs.op = "virtual keye held (+ one press edge)"
                end
            end
        elseif step.id == "bindings" then
            if input.just_pressed("keyy") then
                local ok, err = input.rebind("interact", "key:digit7")
                obs.op = "rebind(interact,key:digit7)=" .. tostring(ok)
                if not ok then
                    obs.op = obs.op .. " err=" .. tostring(err)
                end
            elseif input.just_pressed("keyu") then
                local removed = input.unbind("interact", "key:digit7")
                obs.op = "unbind(interact,key:digit7)=" .. tostring(removed)
            elseif input.just_pressed("keyi") then
                input.reset_bindings()
                obs.op = "reset_bindings() -> " .. bindings_text("interact")
            elseif input.just_pressed("keyo") then
                local text = input.action_map_text()
                local ok = input.set_action_map(text)
                obs.op = "action_map_text -> set_action_map ok=" .. tostring(ok)
            end
        elseif step.id == "pointers_pad" then
            if input.just_pressed("keyn") then
                -- 只登记一次震动请求；无手柄时宿主静默丢弃（headless 也安全）。
                input.rumble(0.6, 0.3, 200)
                obs.rumbles = obs.rumbles + 1
                obs.op = "rumble(0.6, 0.3, 200) requested"
            end
        elseif step.id == "settings" then
            if input.just_pressed("keyk") then
                -- 唯一写盘调用：只放在按键分支里（headless 门不允许每帧写盘）。
                local ok, detail = input.save_settings()
                obs.op = string.format("save_settings=%s/%s", tostring(ok), tostring(detail))
            elseif input.just_pressed("keyj") then
                local ok, detail = lab.try(function()
                    return input.load_settings()
                end)
                obs.op = string.format("load_settings=%s/%s", tostring(ok), tostring(detail))
            elseif input.just_pressed("keyp") then
                obs.nav.threshold = clamp(obs.nav.threshold + 0.1, 0.0, 1.0)
                local threshold, delay_ms, repeat_ms = input.set_nav_settings(
                    obs.nav.threshold,
                    obs.nav.delay_ms,
                    obs.nav.repeat_ms
                )
                obs.eff.threshold = threshold
                obs.eff.delay_ms = delay_ms
                obs.eff.repeat_ms = repeat_ms
                obs.op = string.format("set_nav_settings -> %.2f/%d/%d", threshold, delay_ms, repeat_ms)
            end
        end
    end
    return lab.tick()
end

function M.self_test()
    return lab.self_test()
end

function M.on_destroy()
    -- 本模块只创建一个动态图片（设备灯）；订阅/监听器一个都没有。
    ui.clear("input_device_lamp")
    release_virtual()
    lab.stop()
end

return M
