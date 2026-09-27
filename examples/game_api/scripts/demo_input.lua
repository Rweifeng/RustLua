-- api_input：演示 input 模块的**全部** 32 个函数。
--
-- 这个演示想证明什么：
--   ① 三层模型是**真的分开**的——物理键（`is_down`/`just_pressed`/`just_released`/
--      `pressed`/`released`/`axis`）与命名动作（`action*`）各自独立，同一个按键在两层里
--      能各自被读到；改绑定（`rebind`/`unbind`）只动动作层，物理键层不变。
--   ② 鼠标与手柄的返回值形状是**确定的**：`cursor_position`/`cursor_delta`/`scroll_delta`
--      返回 `[1]=x, [2]=y` 的数组（不是命名表），`pad_axis` 是模拟量、`pad_button` 是布尔。
--   ③ 绑定与设置是**可持久化且可回灌**的：`action_map_text()` 读出来的文本再喂给
--      `set_action_map()` 必须无损（版本头 `rustlua-actions-v2`）；版本头不符要**报原因
--      且不改动当前映射**。
--   ④ 导航手感（`nav_settings`/`set_nav_settings`）与动作连发（`action_repeat`）同源，
--      改一下阈值/节奏就能在状态行看到生效值被钳制后的结果。
--
-- 四个"仪表"实体（场景 `scenes/api_input.scene`）：
--   Probe 蓝方块 = 物理键 + 命名动作，按键就看得见地移动；
--   Key   金方块 = 物理键边沿与 `input.axis("horizontal"/"vertical")`；
--   Mouse 绿方块 = 中心跟着光标走，滚轮推着它抖一下；
--   Pad   粉方块 = 手柄按钮/轴，没手柄时安静停在原点（headless 也如此）。
--
-- 本文件严格遵守工程硬规则：顶层只有常量与函数定义（不调用任何引擎 API），
-- `update()` 在 headless（无窗口、无设备、无按键）下跑通且不报错，
-- 唯一会写盘的 `input.save_settings()` 只出现在按键分支里。
local api = require("api_demo")

local M = {}

-- 场景里的实体名（与 scenes/api_input.scene 一一对应）。
local PROBE = "Probe"
local KEY = "Key"
local MOUSE = "Mouse"
local PAD = "Pad"

-- 三个仪表的活动范围（世界坐标 = 屏幕像素，相机是 1280x720 的正交相机）。
local KEY_BOX = { x0 = 340, y0 = 150, x1 = 440, y1 = 230, pad_x = 24, pad_y = 18 }
local MOUSE_BOX = { x0 = 120, y0 = 320, x1 = 760, y1 = 420, pad_x = 24, pad_y = 22 }
local PAD_BOX = { x0 = 640, y0 = 470, x1 = 1020, y1 = 530, pad_x = 22, pad_y = 16 }

-- 仪表方形尺寸（与场景 Sprite 的 size 一致，用来按中心点摆放）。
local SQUARE = 40

-- 圆点颜色：键盘 = 蓝，手柄 = 绿（`input.last_device()` 驱动的视觉反馈）。
local DOT_KEYBOARD = { 0.25, 0.50, 0.95 }
local DOT_GAMEPAD = { 0.25, 0.90, 0.45 }

-- 供状态行展示/演示用的物理键清单（**全部是合法键名**；拼错的键名会直接报 Lua 错误，
-- 所以这里不用变量拼名，也不写错名）。
local KEYS_HELD = { "keyw", "keys", "keya", "keyd", "arrowleft", "arrowright", "arrowup", "arrowdown" }
local KEY_EVENTS = {
    { "space", "空格" },
    { "keye", "E" },
    { "keyq", "Q" },
    { "keyr", "R" },
    { "digit1", "1(slot_1)" },
    { "digit2", "2(slot_2)" },
    { "digit3", "3(slot_3)" },
}
-- `input.action(name)` 逐名查询的清单：状态行②里逐个显示 true/false，
-- 让人能对照"哪些命名动作此刻真的按下了"。
local ACTION_NAMES = { "move_up", "move_down", "move_left", "move_right", "interact", "confirm" }

-- 一次性动作的触发键（1..6 = 主键区 digit1..digit6）。
-- 为什么借用 1..6 而不是找六个"没被绑定的键"：默认动作表有 36 项，把主键区几乎占满了
-- （`volume_*` 在 `-`/`=`、`slot_1..9` 在 1..9），找六个空闲键反而更不可读。借用它们
-- 是安全的，因为**每个分支都以 `reset_bindings()` 收尾**：默认绑定在下一帧就复原。
local KEY_REBIND = "digit1"
local KEY_UNBIND = "digit2"
local KEY_RESET = "digit3"
local KEY_MAP_ROUNDTRIP = "digit4"
local KEY_SAVE = "digit5"
local KEY_LOAD = "digit6"

-- 本帧状态（模块内共享的"仪表读数"；`update()` 每帧重新算一遍，不做业务累积）。
local S = {
    probe = nil,
    key = nil,
    mouse = nil,
    pad = nil,
    nav = { threshold = 0.5, delay_ms = 350, repeat_ms = 90 },
    key_events = 0,
    last_event = "-",
    rumbles = 0,
    map_lines = 0,
    map_version = "?",
    roundtrip = "-",
    op = "-",
    status = "",
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

--- 把 `-1..1` 映射成 `0..1`（用于颜色强度）。
local function unit(value)
    return clamp((value + 1.0) * 0.5, 0.0, 1.0)
end

--- 把光标/摇杆位置夹到仪表的活动框里（返回方块中心点）。
local function place(box, x, y)
    return lerp(box.x0, box.x1, unit(x)), lerp(box.y0, box.y1, unit(y))
end

--- 浮点文本：`short(1.0)` → `"1"`，`short(0.25)` → `"0.25"`。
local function short(value)
    if math.abs(value - math.floor(value + 0.5)) < 0.001 then
        return string.format("%d", math.floor(value + 0.5))
    end
    return string.format("%.2f", value)
end

--- 某个物理键当前是否按住（顺序与 `KEYS_HELD` 一致；只显示按住的，便于一眼读到状态）。
local function held_text()
    local parts = {}
    for _, key in ipairs(KEYS_HELD) do
        if input.is_down(key) then
            local label = string.gsub(key, "^key", "")
            label = string.gsub(label, "^arrow", "")
            parts[#parts + 1] = label
        end
    end
    if #parts == 0 then
        return "-"
    end
    return table.concat(parts, ",")
end

--- 本帧作为**按下边沿**出现的键（`just_pressed` 与别名 `pressed` 各查一次，两者同义）。
local function pressed_text()
    local parts = {}
    for _, entry in ipairs(KEY_EVENTS) do
        if input.just_pressed(entry[1]) or input.pressed(entry[1]) then
            parts[#parts + 1] = entry[2]
        end
    end
    return table.concat(parts, ",")
end

--- 本帧作为**松开边沿**出现的键（`just_released` 与别名 `released`）。
local function released_text()
    local parts = {}
    for _, entry in ipairs(KEY_EVENTS) do
        if input.just_released(entry[1]) or input.released(entry[1]) then
            parts[#parts + 1] = entry[2]
        end
    end
    return table.concat(parts, ",")
end

--- 鼠标三个按钮的本帧状态（`左/中/右` 各一段，用 `-` 表示无事件）。
local function mouse_text()
    local left = "L" .. (input.mouse_is_down("left") and "1" or "0")
        .. (input.mouse_just_pressed("left") and "^" or "")
        .. (input.mouse_just_released("left") and "v" or "")
    local middle = "M" .. (input.mouse_is_down("middle") and "1" or "0")
        .. (input.mouse_just_pressed("middle") and "^" or "")
        .. (input.mouse_just_released("middle") and "v" or "")
    local right = "R" .. (input.mouse_is_down("right") and "1" or "0")
        .. (input.mouse_just_pressed("right") and "^" or "")
        .. (input.mouse_just_released("right") and "v" or "")
    return string.format("%s %s %s", left, middle, right)
end

--- 打印用的边沿计数：`按下`与`松开`各自累加一次（`just_*` 只在边沿帧为真）。
local function count_edges(pressed, released)
    if pressed ~= "" then
        S.key_events = S.key_events + 1
        S.last_event = "按下 " .. pressed
    end
    if released ~= "" then
        S.key_events = S.key_events + 1
        S.last_event = "松开 " .. released
    end
end

--- `input.action(name)` 逐名查询：把清单里**当前按下**的动作列出来（空了写 "-"）。
--- 这比"只查一个动作"更能说明"命名动作层是一张可枚举的表"。
local function action_states_text()
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

--- `input.bindings(name)` → "动作=绑定|绑定"（绑定文本是 v2 物理键名）。
local function bindings_text(name)
    local list = input.bindings(name)
    local parts = {}
    for index = 1, #list do
        parts[index] = tostring(list[index])
    end
    if #parts == 0 then
        return name .. "=(空)"
    end
    return name .. "=" .. table.concat(parts, "|")
end

--- 把字符串数组拼成一行（数组元素一律 `tostring`，空表返回 "-"）。
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
-- 场景生命周期
-- ---------------------------------------------------------------------------

--- 进入本场景一次：取实体 id、读设置路径与导航手感、打印动作名/诊断，
--- 再画两条"轴标尺"并用 `api.title`/`api.hint` 贴出标题与按键表。
function M.on_create()
    S.probe = entity.find(PROBE)
    S.key = entity.find(KEY)
    S.mouse = entity.find(MOUSE)
    S.pad = entity.find(PAD)

    api.title("api_input - input.* 物理键 / 命名动作 / 鼠标 / 手柄 / 绑定持久化")
    api.hint(
        "WASD/方向键+J+空格+右键 = 四个仪表 | 1=rebind 2=unbind 3=reset 4=映射往返 5=save 6=load | "
            .. api.KEYS_BACK
    )

    -- `input.settings_path()`：设置文件（`[input]` 段）的实际路径；无工程根时返回 nil。
    S.status = "settings_path=" .. tostring(input.settings_path())

    -- `input.load_settings()`：文件缺失返回 `false, "missing"`（**不是错误**，绑定保持不动）。
    -- 这里在 on_create 调一次是安全的：它是**读**，不写盘。
    local ok, detail = input.load_settings()
    log.info(string.format("api_input load_settings ok=%s detail=%s", tostring(ok), tostring(detail)))

    -- `input.nav_settings()`：读 `[input]` 段的菜单导航手感；文件缺失/无工程根返回 nil。
    local nav = input.nav_settings()
    if nav then
        S.nav.threshold = nav.threshold
        S.nav.delay_ms = nav.delay_ms
        S.nav.repeat_ms = nav.repeat_ms
    else
        -- 文件不存在是首次运行的正常路径：保留引擎默认手感（0.5 / 350ms / 90ms）。
        log.info("api_input nav_settings=nil（无设置文件），保留默认 0.5 / 350ms / 90ms")
    end

    -- `input.actions()` 在日志里打印一次全量动作名（状态行只放前四个，日志里能看全）。
    local names = input.actions()
    local head = {}
    for index = 1, math.min(8, #names) do
        head[index] = tostring(names[index])
    end
    log.info(string.format("api_input actions=%d head=%s", #names, table.concat(head, ",")))

    -- `input.migration_notes()` / `input.diagnostics()`：v1→v2 的**有损换算**说明与设置读写诊断。
    -- v2 文件上两者都是空表，这里把内容打到日志里，状态行只显示条目数。
    local notes = input.migration_notes()
    local diags = input.diagnostics()
    log.info(string.format("api_input migration_notes=%d diagnostics=%d", #notes, #diags))
    for index = 1, #notes do
        log.info("api_input migration_note " .. tostring(notes[index]))
    end
    for index = 1, #diags do
        log.info("api_input diagnostic " .. tostring(diags[index]))
    end

    -- 轴标尺：Key / Pad 两根横线画出 `input.axis("horizontal")` 与手柄 X 轴的行程中点，
    -- 让人一眼看出"方块偏离中线多少 = 模拟量多少"。
    ui.image("ruler_key", KEY_BOX.x0, 199, KEY_BOX.x1 - KEY_BOX.x0, 2, "__white__", 0.20, 0.26, 0.38)
    ui.image("ruler_pad", PAD_BOX.x0, 499, PAD_BOX.x1 - PAD_BOX.x0, 2, "__white__", 0.20, 0.26, 0.38)
end

--- 每个固定步一次。headless（无设备、无按键）下也必须跑通。
function M.update()
    -- ⓪ 触屏虚拟键（`input.virtual_key` / `input.virtual_pressed`）：这两条是给**触屏
    -- 虚拟摇杆/按钮**用的——脚本注入按住态，引擎把它与物理键**合并**后再算
    -- `is_down` / `axis` / `action_axis`（`InputSnapshot::virtual_keys`）。
    --
    -- 为什么按下文这么写：
    --   * `virtual_key(name, down)` 的按住态是**跨帧保持**的：注入 true 之后每帧不重发，
    --     只在抬起时注入 false。所以这里每帧都把 `keyv` 的物理按住态原样喂给它，
    --     抬起那一帧自然发 false——注入结果直接体现在下面第①行的 axis 上。
    --   * `virtual_pressed(name)` 只造**一次按下边沿**，是"这一帧点了一下"的语义，
    --     所以它放在 `just_pressed("keyp")` 分支里（每按一次 P 触发一帧）。
    --   * 挑 `keyv` / `keyp` 是因为它们**没有**绑到任何默认动作，注入不会污染
    --     `move_*` / `ui_*` 等动作的读数。
    local vk_down = input.is_down("keyv")
    input.virtual_key("keyv", vk_down)
    if input.just_pressed("keyp") then
        input.virtual_pressed("keyp")
    end

    -- ① 物理键层：边沿、按住与两个内置轴。
    local pressed = pressed_text()
    local released = released_text()
    count_edges(pressed, released)
    local axis_x = input.axis("horizontal")
    local axis_y = input.axis("vertical")
    local in_left = input.is_down("arrowleft")
    local in_right = input.is_down("arrowright")

    -- ② 命名动作层：与物理键层互不影响（动作可以绑在手柄/鼠标上，物理键查询看不到它）。
    local act_right = input.action("move_right")
    local act_left = input.action("move_left")
    local act_up = input.action("move_up")
    local act_down = input.action("move_down")
    local act_interact = input.action("interact")
    local act_confirm = input.action("confirm")
    local interact_down = input.action_pressed("interact")
    local interact_up = input.action_released("interact")
    local ui_left_up = input.action_released("ui_left")
    local step_x = input.action_axis("ui_left", "ui_right")
    local step_y = input.action_axis("ui_down", "ui_up")

    -- 菜单连发：按下边沿立刻一步；模拟量（摇杆）按住超过 nav.delay_ms 后每 nav.repeat_ms 一步；
    -- 数字键/DPad 按住不连发。
    local repeat_up = false
    local repeat_down = false
    if input.action_repeat("ui_up") then
        repeat_up = true
    end
    if input.action_repeat("ui_down") then
        repeat_down = true
    end

    -- ③ 鼠标：位置/增量/滚轮都是 `[1]=x, [2]=y` 的**数组**，按钮名是字符串。
    local cursor = input.cursor_position()
    local delta = input.cursor_delta()
    local scroll = input.scroll_delta()
    local mouse = mouse_text()
    local moving_dot = input.mouse_is_down("left") or input.mouse_is_down("middle") or input.mouse_is_down("right")
    local stuck = input.mouse_just_pressed("left") or input.mouse_just_pressed("middle")
        or input.mouse_just_pressed("right")
    local released_click = input.mouse_just_released("left") or input.mouse_just_released("middle")
        or input.mouse_just_released("right")

    -- ④ 手柄：按钮 0..15（布尔）、轴 0..5（模拟量，左摇杆 X/Y 上为正）。
    local pad_a = input.pad_button(0)
    local pad_b = input.pad_button(1)
    local pad_x = input.pad_button(2)
    local pad_y = input.pad_button(3)
    local pad_dpad_up = input.pad_button(12)
    local pad_dpad_down = input.pad_button(13)
    local pad_dpad_left = input.pad_button(14)
    local pad_dpad_right = input.pad_button(15)
    local stick_x = input.pad_axis(0)
    local stick_y = input.pad_axis(1)
    local trigger_lt = input.pad_axis(4)
    local trigger_rt = input.pad_axis(5)

    -- `input.rumble(low, high, ms)`：只在按键分支里登记（headless 下宿主直接丢弃，静默无效）。
    local rumble_fired = false
    if interact_down then
        input.rumble(0.6, 0.3, 200)
        S.rumbles = S.rumbles + 1
        rumble_fired = true
    end

    -- `input.last_device()`：最近一次**真实按下边沿**的设备（空闲帧不改变）。
    local device = input.last_device()

    -- -----------------------------------------------------------------------
    -- 一次性动作的实验键（1..6，且**每个分支都以 `reset_bindings()` 收尾**，
    -- 避免改键结果泄漏到后续帧/后续演示）。
    --
    -- 顺带演示了一件事：1..6 同时也是默认动作 `slot_1..slot_6` 的键位，而物理键层
    -- （`just_pressed("digit1")`）与动作层（`action_pressed("slot_1")`）**各自独立**，
    -- 所以同一个按键既能驱动实验，又能被 `slot_*` 查到——两层不会互相吞掉。
    -- -----------------------------------------------------------------------
    if input.just_pressed(KEY_REBIND) then
        -- `input.rebind(action, binding)`：清空并重设该动作的**全部**绑定，返回 `ok, err`。
        -- 这里故意改绑到 key:digit7（不是 1..6），好让"解绑"那一档能真的命中一条**非默认**绑定。
        local ok, err = input.rebind("interact", "key:digit7")
        local text = "rebind(interact,key:digit7)=" .. tostring(ok)
        if not ok then
            text = text .. " err=" .. tostring(err)
        end
        log.info("api_input " .. text)
        input.reset_bindings()
        S.op = text
    elseif input.just_pressed(KEY_UNBIND) then
        -- `input.unbind(action, binding)`：移除**单个**绑定，返回是否确实移除。
        local removed = input.unbind("interact", "key:digit7")
        local plain = input.unbind("interact", "key:keye")
        local text = string.format("unbind digit7=%s keye=%s", tostring(removed), tostring(plain))
        if plain then
            -- 解绑真的生效了：此刻 `interact` 没有键盘绑定（只剩手柄按钮），会让人以为"E 坏了"。
            -- 演示的价值就在这里——**立刻复原**，让下一次按键行为可预期。
            text = text .. "(E 已解绑)"
        end
        input.reset_bindings()
        log.info("api_input " .. text .. " → reset_bindings")
        S.op = text .. "→reset"
    elseif input.just_pressed(KEY_RESET) then
        -- `input.reset_bindings()`：恢复引擎默认动作表——绑定是玩家偏好，这是**唯一**重置入口。
        input.reset_bindings()
        input.reset_bindings()
        log.info("api_input reset_bindings x2 → defaults")
        S.op = "reset_bindings→" .. bindings_text("interact")
    elseif input.just_pressed(KEY_MAP_ROUNDTRIP) then
        -- `input.action_map_text()` / `input.set_action_map(text)`：把刚读出来的文本原样回灌。
        -- 期望无损：回灌结果与回灌前逐字相同；版本头不符则返回 `false, 原因` 且**不改动**映射。
        local text = input.action_map_text()
        local lines = 0
        for _ in string.gmatch(text, "[^\n]+") do
            lines = lines + 1
        end
        local header = string.gsub(text, "\n.*", "")
        local function restore(blob)
            local ok, err = input.set_action_map(blob)
            if not ok then
                -- 引擎的原因文本是英文长句（`unsupported action map version: <头>`）；
                -- 状态行只有一行的宽度，所以这里**只留版本头**，完整原因照旧写进日志。
                log.info("api_input set_action_map 拒绝原因: " .. tostring(err))
                return "拒绝:" .. string.gsub(tostring(err), "^.*version: ", "")
            end
            return "接受"
        end
        local restored = restore(text)
        local same = (input.action_map_text() == text)
        local bad = restore("rustlua-actions-v9\ninteract=key:keye")
        local text_now = input.action_map_text()
        S.map_lines = lines
        S.map_version = header
        S.roundtrip = string.format(
            "%d行 回灌=%s 一致=%s 坏头=%s",
            lines,
            restored,
            tostring(same),
            bad
        )
        log.info(string.format("api_input action_map roundtrip: %s", S.roundtrip))
        log.info("api_input 坏版本头后 map 未变: " .. tostring(text_now == text))
        S.op = "action_map_text→set_action_map"
    elseif input.just_pressed(KEY_SAVE) then
        -- `input.save_settings()`：**只在这里**写盘（`settings.toml` 的 `[input]` 段，原子写、
        -- 保留 `[audio]` 段与注释）。headless 门不允许每帧写盘，所以它必须待在按键分支里。
        local ok, detail = input.save_settings()
        log.info(string.format("api_input save_settings ok=%s detail=%s", tostring(ok), tostring(detail)))
        local read_ok, read_detail = input.load_settings()
        S.op = string.format(
            "save=%s/%s 再load=%s/%s",
            tostring(ok),
            tostring(detail),
            tostring(read_ok),
            tostring(read_detail)
        )
    elseif input.just_pressed(KEY_LOAD) then
        -- `input.load_settings()`：读回 `[input].map` 并应用（并同时应用导航手感）；
        -- 文件缺失返回 `false, "missing"`——这是**正常路径**，不是错误。
        local ok, detail = input.load_settings()
        log.info(string.format("api_input load_settings ok=%s detail=%s", tostring(ok), tostring(detail)))
        S.op = string.format("load=%s/%s", tostring(ok), tostring(detail))
    end

    -- `input.set_nav_settings(threshold, delay_ms, repeat_ms)`：把导航手感推给引擎（阈值同时是
    -- 设备层轴判定阈值，两个时长供 `action_repeat` 用），返回**钳制后**的生效值。
    -- 这里每帧用上一步读到的值推一次：演示参数校验/钳制，同时不改变用户设置。
    local eff_threshold, eff_delay, eff_repeat = input.set_nav_settings(
        S.nav.threshold,
        S.nav.delay_ms,
        S.nav.repeat_ms
    )

    -- -----------------------------------------------------------------------
    -- 仪表：位置 = 输入，颜色 = 输入。所有 `ui.image` 都是"本帧的事实"的可视化。
    -- -----------------------------------------------------------------------

    -- Probe：物理键轴 + 命名动作轴 + 鼠标按键，一起推动蓝色方块。
    local probe_x = 450
        + 70 * (axis_x + (in_right and 1 or 0) - (in_left and 1 or 0))
        + 90 * ((act_right and 1 or 0) - (act_left and 1 or 0))
    local probe_y = 190
        - 55 * (axis_y + (act_up and 1 or 0) - (act_down and 1 or 0))
        + (moving_dot and 22 or 0)
        - (stuck and 10 or 0)
        + (act_interact and 8 or 0)
        + (act_confirm and -16 or 0)
    if S.probe then
        transform.set(S.probe, probe_x, probe_y)
    end

    -- Key：金方块 = 物理键边沿 + `input.axis`。水平位置跟 `horizontal`，垂直位置跟 `vertical`，
    -- 颜色强度跟"本帧是否有边沿"（按下更亮、松开更暗）。
    local key_x = lerp(KEY_BOX.x0, KEY_BOX.x1, unit(axis_x + (in_right and 1 or 0) - (in_left and 1 or 0)))
    local key_y = lerp(KEY_BOX.y0, KEY_BOX.y1, unit(-axis_y))
    if S.key then
        transform.set(S.key, key_x, key_y)
    end
    local edge_active = (pressed ~= "") or (released ~= "")
    local key_glow = edge_active and 1.0 or (0.45 + 0.25 * math.abs(axis_x))
    ui.image("dot_key", key_x - SQUARE * 0.5, key_y - SQUARE * 0.5 - 30, SQUARE, 8, "__white__", key_glow, key_glow * 0.8, 0.25)

    -- Mouse：绿方块中心 = `cursor_position()`；`cursor_delta` 推它一点，`scroll_delta` 推它更多
    --（滚轮通常只有 ±1，放大 4 倍才看得见）。按住左键变亮，松开那一帧变暗。
    local extra_x = delta[1] * 4 + scroll[1] * 4
    local extra_y = delta[2] * 4 + scroll[2] * 4
    local mouse_x, mouse_y = place(MOUSE_BOX, cursor[1] + extra_x, cursor[2] + extra_y)
    if S.mouse then
        transform.set(S.mouse, mouse_x, mouse_y)
    end
    local mouse_glow = 0.55
    if moving_dot then
        mouse_glow = 1.0
    elseif released_click then
        mouse_glow = 0.3
    end
    ui.image("dot_mouse", mouse_x - SQUARE * 0.5, mouse_y - SQUARE * 0.5 - 28, SQUARE, 6, "__white__", 0.4, mouse_glow, 0.35)

    -- Pad：粉方块 = 左摇杆；按住手柄按钮（A/B/X/Y、DPad）会在上面亮 4 个点；扳机 LT/RT 决定亮度。
    local pad_x, pad_y = place(PAD_BOX, stick_x, stick_y)
    if S.pad then
        transform.set(S.pad, pad_x, pad_y)
    end
    local face = { pad_y, pad_b, pad_x, pad_a }
    for index = 1, 4 do
        local on = face[index]
        ui.image(
            "face_" .. index,
            pad_x - 34 + (index - 1) * 18,
            pad_y - 34,
            14,
            on and 14 or 5,
            "__white__",
            on and 1.0 or 0.18,
            on and 1.0 or 0.20,
            on and 1.0 or 0.28
        )
    end
    local dpad = { pad_dpad_right, pad_dpad_down, pad_dpad_left, pad_dpad_up }
    for index = 1, 4 do
        local on = dpad[index]
        ui.image(
            "dpad_" .. index,
            pad_x - 34 + (index - 1) * 18,
            pad_y + 22,
            14,
            on and 14 or 5,
            "__white__",
            on and 1.0 or 0.18,
            on and 0.55 or 0.20,
            on and 1.0 or 0.28
        )
    end
    local trigger = math.max(trigger_lt, trigger_rt)
    ui.image("trigger", pad_x - 34, pad_y + 40, 70, 6, "__white__", 0.35 + 0.65 * trigger, 0.30, 0.45 + 0.55 * trigger)

    -- 设备指示灯：只在**真实按下边沿**时切换的设备类别（keyboard / gamepad）。
    local dot = DOT_KEYBOARD
    if device == "gamepad" then
        dot = DOT_GAMEPAD
    end
    ui.image("device_dot", 950, 60, 26, 26, "__white__", dot[1], dot[2], dot[3])

    -- -----------------------------------------------------------------------
    -- 状态行：把"本帧读到的数值"写出来。
    --
    -- 布局约束（真的量过，不是估的）：状态行从 y=636 起，字号 12×1.1px、行高
    -- 1.2em + 2px 间距 ⇒ **每行约 17.8px**；api_demo 在 y=662 固定渲染按键提示，
    -- 因此 4 行（到 y≈707）是上限，第 5 行会被提示压住。宽度同理：视口 1280px，
    -- 汉字 advance ≈ 1em、ASCII ≈ 0.55em，所以每行不超过约 100 个"当量字符"
    -- （下面的 `fits` 就是这个自检：超宽时把整串打进日志，屏幕上看个短提示，
    -- 而不是让半句话被裁在屏幕外面）。
    -- -----------------------------------------------------------------------
    local hover = (input.is_down("keye") or input.is_down("keyq")) and "E/Q" or "-"

    -- 第①行：物理键层（`is_down` / `just_pressed` / `pressed` / `just_released` /
    -- `released` / `axis`）——这一层只认物理键，与绑定无关。
    -- 行尾的 `虚拟keyv=` 是 `input.virtual_key` 注入后的**读回**：按 V 时它必须跟着变 true，
    -- 同时 `axis h` 也会 +1（虚拟键与物理键在 `is_down`/`axis` 里是同一份合并结果）。
    S.status = string.format(
        "① 物理键 按住[%s] 按下[%s] 松开[%s] 事件=%d 最近=%s axis h=%s v=%s 虚拟keyv=%s",
        held_text(),
        pressed ~= "" and pressed or "-",
        released ~= "" and released or "-",
        S.key_events,
        S.last_event,
        short(axis_x),
        short(axis_y),
        tostring(input.is_down("keyv"))
    )
    -- 第②行：命名动作层（`action` / `action_pressed` / `action_released` /
    -- `action_repeat` / `action_axis` / `last_device`）。`ui_x`/`ui_y` 是方向动作合成的轴，
    -- `ui_left松` 演示"任意动作"的松开边沿（与 `interact` 的边沿是同一套语义）。
    S.status = S.status
        .. "\n"
        .. string.format(
            "② 动作 按下[%s] 交互=%s(按下=%s 松开=%s) ui_left松=%s ui_x=%s ui_y=%s 连发[%s/%s]",
            action_states_text(),
            tostring(act_interact),
            tostring(interact_down),
            tostring(interact_up),
            tostring(ui_left_up),
            short(step_x),
            short(step_y),
            tostring(repeat_up),
            tostring(repeat_down)
        )
    -- 第③行：鼠标三个"数组返回"接口 + 三个 `mouse_*` 按钮接口 + 手柄原始查询
    --（`pad_button` / `pad_axis` / `rumble`）。`L1^v` = 左键按住/本帧按下/本帧松开。
    S.status = S.status
        .. "\n"
        .. string.format(
            "③ 鼠标 光标[%s,%s] 增量[%s,%s] 滚轮[%s,%s] 按钮[%s] 手柄 A=%s B=%s X=%s Y=%s 轴[%s,%s] LT%s RT%s 震动%d%s",
            short(cursor[1]),
            short(cursor[2]),
            short(delta[1]),
            short(delta[2]),
            short(scroll[1]),
            short(scroll[2]),
            mouse,
            tostring(pad_a),
            tostring(pad_b),
            tostring(pad_x),
            tostring(pad_y),
            short(stick_x),
            short(stick_y),
            short(trigger_lt),
            short(trigger_rt),
            S.rumbles,
            rumble_fired and "!" or ""
        )
    -- 第④行：绑定与设置（`bindings` / `action_map_text` / `nav_settings` /
    -- `set_nav_settings` / `diagnostics` / `migration_notes`）+ 上一次实验键的结果。
    -- 空数组（没有诊断/迁移说明）也照样显示条目数，因为"0 条"本身就是要看到的事实；
    -- 完整内容（动作全表、绑定全表、诊断原文）在 `on_create` 与各分支里进日志。
    S.status = S.status
        .. "\n"
        .. string.format(
            "④ 绑定 %s %s 映射 %s(%d行) 设备=%s 导航生效[%s/%d/%d] 实验 %s 诊断%d(%s) 迁移%d(%s)",
            bindings_text("interact"),
            bindings_text("step_x"),
            tostring(S.map_version),
            S.map_lines,
            tostring(device),
            short(eff_threshold),
            eff_delay,
            eff_repeat,
            S.op,
            #input.diagnostics(),
            join_lines(input.diagnostics()),
            #input.migration_notes(),
            join_lines(input.migration_notes())
        )

    -- 宽度自检：把每行折算成"当量字符数"（汉字 1、ASCII 0.55），上限 95 当量
    -- ≈ 1250px（视口 1280px、状态行从 x=24 起、字号 12×1.1px）。超了就把整串打进
    -- 日志，免得以为"数值没显示"其实是被裁在屏幕外——屏幕上装不下的，日志里必须有。
    local widest = 0
    for line in string.gmatch(S.status .. "\n", "([^\n]*)\n") do
        local total = utf8.len(line) or #line
        local cjk = 0
        for _ in string.gmatch(line, "[%z\1-\127\194-\244][\128-\191]*") do
            cjk = cjk + 1
        end
        local ascii = total - cjk
        local width = cjk + (ascii * 0.55)
        if width > widest then
            widest = width
        end
    end
    if widest > 95 then
        log.info(string.format("api_input status 过宽(%.0f 当量字符)，原文=%s", widest, S.status))
    end
    api.status(S.status)

    return string.format(
        "api_input_ok keys=%d act=L%sR%s interact=%s mouse=%s dev=%s pad=%s rebind=%s nav=%s",
        S.key_events,
        tostring(act_left),
        tostring(act_right),
        tostring(act_interact),
        mouse,
        tostring(device),
        tostring(pad_a),
        S.roundtrip,
        short(eff_threshold)
    )
end

--- 离开场景：删掉本模块注册的全部动态 UI（`ui.clear` 同时删文本与图片）。
function M.on_destroy()
    for _, id in ipairs({
        "ruler_key",
        "ruler_pad",
        "dot_key",
        "dot_mouse",
        "face_1",
        "face_2",
        "face_3",
        "face_4",
        "dpad_1",
        "dpad_2",
        "dpad_3",
        "dpad_4",
        "trigger",
        "device_dot",
    }) do
        ui.clear(id)
    end
end

--- 场景里的 `UiEvents` 点击回调（本场景没有可点实体，保留接口以免路由分派报错）。
function M.on_click(name)
    log.info("api_input click " .. tostring(name))
    return false
end

return M
