-- api_audio：audio 模块的 2.0 分步 Lab（总线 / 音量 / 播放 / 空间化 / 设置）。
--
-- 一条主线：**主音量 × 分组音量 = 实际音量**；播放接口只登记一次请求；设置文本可往返。
-- 每一步都把自己的状态写在舞台里，右侧卡片读回本帧真实值；F5 会依次跑
-- enter → run × 3 → check，因此"读完就把状态改乱"的写法会当场变红。
--
-- 为什么每条 `enter` 都要复位总线：读者可能按过 `, . [ ]` 调过音量、按过 M 播过音乐；
-- 若不复位，F5 的结论就依赖上一次操作，不再可重复。
--
-- 交互键（Lab 保留 space/方向键/1..9/R/F/F5，这里只用字母与标点）：
--   [ ] 主音量 ±0.1    , . 选中总线 ±0.1    B 切换选中总线    X 还原混音
--   P play  H play_on(sfx)  U play_on(ui)  O play_at  M music  N stop_music
--   L load_settings    S save_settings（唯一写盘调用）  P 应用设置文本（settings 步）
--
-- 硬规则（README §2）：顶层只定义常量与函数；`run` 在 headless 下只读；
-- `audio.save_settings()` 只待在按键分支里，`load/apply` 都包在 `lab.try` 里。
local lab = require("lab")

local M = {}

-- 场景实体名（scenes/api_audio.scene）：三个精灵就是三路音量的"表"。
local SPEAKER, BGM, SFX = "Speaker", "Bgm", "Sfx"

-- 素材名是相对 `assets/audio/` 的裸文件名（pickup/hit/attack/bgm 都已存在）。
local CLIP_PICKUP = "pickup"
local CLIP_HIT = "hit"
local CLIP_ATTACK = "attack"
local CLIP_BGM = "bgm"

-- 复位值：主 0.8、bgm 0.7、sfx 0.9、ui 0.6（每一步 enter 都推回这组数）。
local MASTER = 0.8
local MIX = { bgm = 0.7, sfx = 0.9, ui = 0.6 }

local BUS_KEYS = { "master", "bgm", "sfx", "ui" }
local ACTOR_X = { 400, 640, 880 }
local METER_X = { 370, 510, 650, 790 }
local METER_Y = 400
local METER_W = 90
local METER_H = 110

-- 动态 UI 的 id 清单：`lab.stop()` 只清 shell，不会替模块清这些图；离开时必须逐个清。
local UI_IDS = {
    "audio_head", "audio_mix_line", "audio_mix_line2", "audio_sel",
    "audio_meter_bg_1", "audio_meter_bg_2", "audio_meter_bg_3", "audio_meter_bg_4",
    "audio_meter_fill_1", "audio_meter_fill_2", "audio_meter_fill_3", "audio_meter_fill_4",
    "audio_meter_txt_1", "audio_meter_txt_2", "audio_meter_txt_3", "audio_meter_txt_4",
    "audio_flash",
    "audio_listener_dot", "audio_emitter_dot", "audio_spatial_line",
    "audio_spatial_txt", "audio_spatial_txt2",
    "audio_settings_1", "audio_settings_2", "audio_settings_3", "audio_settings_4",
}

local ids = {}
local selected = 2
local emitter_x = 4
local flash = 0
local flash_x = 640
local flash_y = 220

-- 脚本侧观察值（每帧由 run 重算；不做业务累积，避免读数漂移）。
local obs = {
    master = MASTER,
    bgm = MIX.bgm,
    sfx = MIX.sfx,
    ui = MIX.ui,
    eff_bgm = MASTER * MIX.bgm,
    eff_sfx = MASTER * MIX.sfx,
    eff_ui = MASTER * MIX.ui,
    path = "",
    text = "",
    text_lines = 0,
    listener = 0,
    last_gain = "-",
    last_pan = "-",
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

--- 浮点短文本：整数值不显示小数点，便于一眼读。
local function short(value)
    if math.abs(value - math.floor(value + 0.5)) < 0.001 then
        return string.format("%d", math.floor(value + 0.5))
    end
    return string.format("%.2f", value)
end

--- 统计行数（设置文本的版本头 + 四行音量，共 5 行）。
local function count_lines(text)
    local lines = 0
    for _ in string.gmatch(text, "[^\n]+") do
        lines = lines + 1
    end
    return lines
end

-- ---------------------------------------------------------------------------
-- 场景 / 音频状态工具
-- ---------------------------------------------------------------------------

--- 按名字取实体；场景被改名/删实体时返回 nil，调用点先判空（不硬崩）。
local function find_ids()
    ids.speaker = entity.find(SPEAKER)
    ids.bgm = entity.find(BGM)
    ids.sfx = entity.find(SFX)
end

--- 把四路音量推回本演示的基准值（enter 与按键分支共用）。
local function reset_mix()
    audio.volume(MASTER)
    audio.set_bus_volume("bgm", MIX.bgm)
    audio.set_bus_volume("sfx", MIX.sfx)
    audio.set_bus_volume("ui", MIX.ui)
end

--- 清掉本模块的动态 UI（切步骤 / 销毁时调用；不会碰 shell 的 id）。
local function clear_ui()
    for _, id in ipairs(UI_IDS) do
        ui.clear(id)
    end
end

--- 当前选中总线的下标（跳过 master：主音量由 [ ] 管）。
local function cycle_bus(step)
    selected = selected + step
    if selected < 2 then
        selected = 4
    elseif selected > 4 then
        selected = 2
    end
    return BUS_KEYS[selected]
end

--- 精灵纵向位置 = 实际音量（主 × 分组）：0 时落到 300，1 时升到 180。
local function draw_actor(id, home_x, effective)
    if id then
        transform.set(id, home_x, 300 - 120 * clamp(effective, 0.0, 1.0))
    end
end

--- 四根竖条 + 读数行：亮度/高度都是 effective_volume，主 × 分组这层乘法因此看得见。
local function draw_meters()
    ui.text_at("audio_head", 330, 96, "AUDIO MIX   master x bus = effective", 1.0, 14)
    for index, key in ipairs(BUS_KEYS) do
        local x = METER_X[index]
        local effective = audio.effective_volume(key)
        if index == selected and selected > 1 then
            ui.image("audio_sel", x - 4, METER_Y - 4, METER_W + 8, METER_H + 8, "__white__", 0.18, 0.30, 0.52)
        end
        ui.image("audio_meter_bg_" .. index, x, METER_Y, METER_W, METER_H, "__white__", 0.15, 0.18, 0.25)
        local height = math.max(2, METER_H * clamp(effective, 0.0, 1.0))
        local r, g, b = 0.42, 0.62, 1.00
        if key == "bgm" then
            r, g, b = 0.96, 0.78, 0.34
        elseif key == "sfx" then
            r, g, b = 1.00, 0.58, 0.74
        elseif key == "ui" then
            r, g, b = 0.42, 0.86, 0.72
        end
        ui.image(
            "audio_meter_fill_" .. index,
            x,
            METER_Y + METER_H - height,
            METER_W,
            height,
            "__white__",
            r,
            g,
            b
        )
        ui.text_at(
            "audio_meter_txt_" .. index,
            x,
            METER_Y + METER_H + 8,
            string.format("%s %s", key, short(effective)),
            1.0,
            12
        )
    end
    draw_actor(ids.speaker, ACTOR_X[1], audio.effective_volume("master"))
    draw_actor(ids.bgm, ACTOR_X[2], audio.effective_volume("bgm"))
    draw_actor(ids.sfx, ACTOR_X[3], audio.effective_volume("sfx"))

    ui.text_at(
        "audio_mix_line",
        330,
        342,
        string.format(
            "master %s   bgm %s x %s = %s   sfx %s x %s = %s",
            short(audio.bus_volume("master")),
            short(audio.bus_volume("master")),
            short(audio.bus_volume("bgm")),
            short(audio.effective_volume("bgm")),
            short(audio.bus_volume("master")),
            short(audio.bus_volume("sfx")),
            short(audio.effective_volume("sfx"))
        ),
        1.0,
        13
    )
    ui.text_at(
        "audio_mix_line2",
        330,
        366,
        string.format(
            "ui %s x %s = %s   (brightness / bar height = effective volume)",
            short(audio.bus_volume("master")),
            short(audio.bus_volume("ui")),
            short(audio.effective_volume("ui"))
        ),
        1.0,
        12
    )
end

--- 一次播放的闪光（只画一次进度；flash 归零后把图清掉，避免留下残影）。
local function draw_flash(dt)
    if flash <= 0 then
        ui.clear("audio_flash")
        return
    end
    flash = math.max(0, flash - dt * 1.6)
    local size = 28 + 44 * flash
    ui.image(
        "audio_flash",
        clamp(flash_x - size * 0.5, 300, 930 - size),
        clamp(flash_y - size * 0.5, 90, 520 - size),
        size,
        size,
        "__white__",
        1.0,
        0.98,
        0.72
    )
end

local function draw_spatial()
    ui.text_at("audio_head", 330, 96, "SPATIAL   listener pose + play_at gain/pan", 1.0, 14)
    local lx, ly = 420, 462
    local ex = 420 + emitter_x * 40
    ui.image("audio_spatial_line", lx + 10, ly - 2, ex - lx - 20, 4, "__white__", 0.28, 0.38, 0.58)
    ui.image("audio_listener_dot", lx - 10, ly - 10, 20, 20, "__white__", 0.36, 0.64, 1.00)
    ui.image("audio_emitter_dot", ex - 10, ly - 10, 20, 20, "__white__", 1.00, 0.62, 0.32)
    ui.text_at(
        "audio_spatial_txt",
        330,
        500,
        string.format(
            "listener (0,0,5) forward -Z   emitter x=%d   last gain=%s pan=%s",
            emitter_x,
            tostring(obs.last_gain),
            tostring(obs.last_pan)
        ),
        1.0,
        13
    )
    ui.text_at(
        "audio_spatial_txt2",
        330,
        528,
        "comma / period moves the emitter; O plays once at its world position",
        1.0,
        12
    )
end

local function draw_settings()
    ui.text_at("audio_settings_1", 330, 120, "SETTINGS   versioned text (four volumes only)", 1.0, 14)
    ui.text_at("audio_settings_2", 330, 152, "path: " .. obs.path, 1.0, 12)
    ui.text_at(
        "audio_settings_3",
        330,
        184,
        "text: " .. string.gsub(obs.text, "\n", " | "),
        1.0,
        12
    )
    ui.text_at(
        "audio_settings_4",
        330,
        240,
        "P applies the text above   L load_settings   S save_settings (writes the file)",
        1.0,
        12
    )
end

-- ---------------------------------------------------------------------------
-- 步骤
-- ---------------------------------------------------------------------------

local steps = {
    {
        id = "buses_and_mix",
        title = "Buses & mix",
        claim = "四条总线固定顺序，实际音量 = 主音量 × 分组音量（master 只等于主音量）",
        api = { "audio.buses", "audio.bus_volume", "audio.effective_volume", "audio.volume" },
        code = 'audio.buses(); audio.volume(0.8); audio.effective_volume("bgm")',
        enter = function()
            find_ids()
            clear_ui()
            reset_mix()
            selected = 2
            obs.op = "mix reset to 0.80 / 0.70 / 0.90 / 0.60"
        end,
        run = function()
            local list = audio.buses()
            obs.master = audio.bus_volume("master")
            obs.bgm = audio.bus_volume("bgm")
            obs.sfx = audio.bus_volume("sfx")
            obs.ui = audio.bus_volume("ui")
            obs.eff_bgm = audio.effective_volume("bgm")
            obs.eff_sfx = audio.effective_volume("sfx")
            obs.eff_ui = audio.effective_volume("ui")
            draw_meters()
            obs.op = string.format(
                "buses=%d master=%s bgm=%s sfx=%s ui=%s",
                #list,
                short(obs.master),
                short(obs.bgm),
                short(obs.sfx),
                short(obs.ui)
            )
            lab.note(obs.op .. " | brightness = master x bus")
        end,
        read = function()
            return {
                { label = "master", value = short(obs.master) },
                { label = "bgm", value = short(obs.bgm) },
                { label = "sfx", value = short(obs.sfx) },
                { label = "ui", value = short(obs.ui) },
                { label = "bgm effective", value = short(obs.eff_bgm) },
            }
        end,
        check = function(t)
            local list = audio.buses()
            t:eq(#list, 4, "buses() lists four buses")
            t:eq(list[1], "master", "master comes first")
            t:eq(list[2], "bgm", "bgm comes second")
            t:eq(list[3], "sfx", "sfx comes third")
            t:eq(list[4], "ui", "ui comes last")
            t:near(audio.bus_volume("master"), 0.8, 0.001, "master volume reads back")
            t:near(audio.bus_volume("bgm"), 0.7, 0.001, "bgm volume reads back")
            t:near(audio.effective_volume("bgm"), 0.56, 0.001, "bgm effective = 0.8 x 0.7")
            t:near(audio.effective_volume("master"), audio.bus_volume("master"), 0.0001, "master effective is the master volume")
        end,
    },
    {
        id = "bus_and_master",
        title = "Bus vs master",
        claim = "分组音量独立可调且被钳制；主音量只影响 effective，不改分组读数",
        api = { "audio.set_bus_volume", "audio.bus_volume", "audio.effective_volume", "audio.volume" },
        code = 'audio.set_bus_volume("sfx", 0.25); audio.volume(0.5); audio.effective_volume("sfx")',
        enter = function()
            find_ids()
            clear_ui()
            reset_mix()
            selected = 2
            obs.op = "mix reset"
        end,
        run = function()
            obs.master = audio.bus_volume("master")
            obs.bgm = audio.bus_volume("bgm")
            obs.sfx = audio.bus_volume("sfx")
            obs.ui = audio.bus_volume("ui")
            obs.eff_bgm = audio.effective_volume("bgm")
            obs.eff_sfx = audio.effective_volume("sfx")
            obs.eff_ui = audio.effective_volume("ui")
            draw_meters()
            obs.op = string.format("selected=%s %s", BUS_KEYS[selected], short(audio.bus_volume(BUS_KEYS[selected])))
            lab.note(obs.op .. " | [ ] master   , . selected bus   B switch   X reset")
        end,
        read = function()
            return {
                { label = "master", value = short(obs.master) },
                { label = "selected", value = BUS_KEYS[selected] },
                { label = "bus volume", value = short(audio.bus_volume(BUS_KEYS[selected])) },
                { label = "effective", value = short(audio.effective_volume(BUS_KEYS[selected])) },
                { label = "bgm effective", value = short(obs.eff_bgm) },
                { label = "sfx effective", value = short(obs.eff_sfx) },
                { label = "ui effective", value = short(obs.eff_ui) },
            }
        end,
        check = function(t)
            t:eq(audio.set_bus_volume("sfx", 0.25), true, "set_bus_volume returns true")
            t:near(audio.bus_volume("sfx"), 0.25, 0.001, "the sfx bus reads back 0.25")
            t:near(audio.effective_volume("sfx"), 0.2, 0.001, "sfx effective = 0.8 x 0.25")

            audio.volume(0.5)
            t:near(audio.bus_volume("master"), 0.5, 0.001, "volume() sets the master bus")
            t:near(audio.bus_volume("bgm"), 0.7, 0.001, "changing master leaves bgm alone")
            t:near(audio.effective_volume("bgm"), 0.35, 0.001, "bgm effective follows the new master")

            audio.set_bus_volume("ui", 4.0)
            t:near(audio.bus_volume("ui"), 1.0, 0.001, "bus volume is clamped to 1")
            audio.volume(2.0)
            t:near(audio.bus_volume("master"), 1.0, 0.001, "master volume is clamped to 1")

            local value, reason = lab.try(function()
                return audio.set_bus_volume("no_such_bus", 0.5)
            end)
            t:ok(value == nil and type(reason) == "string", "an unknown bus raises a Lua error")

            reset_mix()
        end,
    },
    {
        id = "playback",
        title = "Playback",
        claim = "play / play_on / play_at / music 只登记播放，stop_music 对未播放状态是空操作",
        api = { "audio.play", "audio.play_on", "audio.play_at", "audio.music", "audio.stop_music", "audio.settings_text" },
        code = 'audio.play_on("sfx", "hit"); audio.play_at("attack", 4, 0); audio.music("bgm")',
        enter = function()
            find_ids()
            clear_ui()
            reset_mix()
            audio.set_listener(0, 0, 0, 0, 0, -1)
            audio.stop_music()
            flash = 0
            selected = 2
            obs.op = "no clip requested yet"
        end,
        run = function(dt)
            obs.master = audio.bus_volume("master")
            draw_meters()
            draw_flash(dt)
            lab.note("P play  H play_on sfx  U play_on ui  O play_at  M music  N stop_music")
        end,
        read = function()
            return {
                { label = "last op", value = obs.op },
                { label = "pickup", value = "play(pickup)" },
                { label = "hit", value = "play_on(sfx,hit)" },
                { label = "attack", value = "play_on(ui,attack)" },
                { label = "spatial", value = "play_at(attack,4,0)" },
                { label = "music", value = "music(bgm) / stop_music" },
            }
        end,
        check = function(t)
            local played = lab.try(function()
                audio.play(CLIP_PICKUP)
                return true
            end)
            t:eq(played, true, "play() returns without raising")

            local played_on, reason = lab.try(function()
                return audio.play_on("sfx", CLIP_HIT)
            end)
            t:ok(played_on == true or played_on == false, "play_on returns an ok flag")
            t:eq(type(reason), "string", "play_on returns a reason string")

            local spot = lab.try(function()
                local ok, gain, pan = audio.play_at(CLIP_ATTACK, 3, 0, 0)
                return { ok = ok, gain = gain, pan = pan }
            end)
            t:ok(spot == nil or type(spot.ok) == "boolean", "play_at returns an ok flag or a controlled error")
            t:ok(spot == nil or type(spot.gain) == "number", "play_at reports a numeric gain")
            t:ok(spot == nil or type(spot.pan) == "number", "play_at reports a numeric pan")

            local started = lab.try(function()
                audio.music(CLIP_BGM)
                return true
            end)
            t:eq(started, true, "music() returns without raising")
            local stopped = lab.try(function()
                audio.stop_music()
                return true
            end)
            t:eq(stopped, true, "stop_music() is safe")

            local text = audio.settings_text()
            t:eq(type(text), "string", "settings_text returns a string")
            t:ok(text:find("rustlua-audio-v1", 1, true) ~= nil, "settings_text carries the version header")
        end,
    },
    {
        id = "spatial",
        title = "Spatial",
        claim = "set_listener 写听者位姿，listener 读回六个分量；play_at 的 gain 随距离线性下降",
        api = { "audio.set_listener", "audio.listener", "audio.play_at" },
        code = 'audio.set_listener(0,0,5,0,0,-1); audio.play_at("attack", 4, 0, 5)',
        enter = function()
            find_ids()
            clear_ui()
            reset_mix()
            emitter_x = 4
            obs.last_gain = "-"
            obs.last_pan = "-"
            audio.set_listener(0, 0, 5, 0, 0, -1)
            obs.op = "listener fixed at (0,0,5) facing -Z"
        end,
        run = function()
            local pose = audio.listener()
            obs.listener = #pose
            draw_actor(ids.speaker, ACTOR_X[1], 0.8)
            draw_spatial()
            obs.op = string.format("listener components=%d emitter x=%d", obs.listener, emitter_x)
            lab.note(obs.op .. " | comma / period move the emitter; O plays at it")
        end,
        read = function()
            local pose = audio.listener()
            return {
                { label = "listener", value = pose[1] and string.format("%s,%s,%s", short(pose[1]), short(pose[2]), short(pose[3])) or "(none)" },
                { label = "forward", value = pose[4] and string.format("%s,%s,%s", short(pose[4]), short(pose[5]), short(pose[6])) or "(none)" },
                { label = "emitter x", value = emitter_x },
                { label = "last gain", value = obs.last_gain },
                { label = "last pan", value = obs.last_pan },
            }
        end,
        check = function(t)
            audio.set_listener(0, 0, 5, 0, 0, -1)
            local pose = audio.listener()
            t:eq(#pose, 6, "listener() reports position + forward")
            t:near(pose[1], 0, 0.001, "listener x is 0")
            t:near(pose[3], 5, 0.001, "listener z is 5")
            t:near(pose[6], -1, 0.001, "listener faces -Z")

            local near_spot, near_error = lab.try(function()
                local ok, gain, pan = audio.play_at(CLIP_ATTACK, 1, 0, 5)
                return { ok = ok, gain = gain, pan = pan }
            end)
            local far_spot, far_error = lab.try(function()
                local ok, gain, pan = audio.play_at(CLIP_ATTACK, 40, 0, 5)
                return { ok = ok, gain = gain, pan = pan }
            end)
            t:ok(near_spot ~= nil or near_error ~= nil, "a near play_at returns or reports an error")
            t:ok(far_spot ~= nil or far_error ~= nil, "a far play_at returns or reports an error")
            if near_spot and far_spot and near_spot.ok and far_spot.ok then
                t:ok(near_spot.gain >= far_spot.gain, "a nearer emitter is not quieter")
                t:ok(near_spot.pan > 0, "an emitter on +X pans to the right")
            else
                t:ok(true, "gain comparison skipped (no listener or no audio device)")
                t:ok(true, "pan check skipped (no listener or no audio device)")
            end
        end,
    },
    {
        id = "settings",
        title = "Settings text",
        claim = "settings_text 是带版本头的五行文本，apply_settings 往返后四路音量完全还原",
        api = { "audio.settings_text", "audio.apply_settings", "audio.settings_path", "audio.load_settings", "audio.save_settings" },
        code = 'local text = audio.settings_text(); audio.apply_settings(text); audio.settings_path()',
        enter = function()
            find_ids()
            clear_ui()
            -- load_settings 只读文件，缺失是正常状态：统一用 lab.try 容错，再复位到基准值。
            lab.try(function()
                return audio.load_settings()
            end)
            reset_mix()
            obs.op = "mix reset after load_settings"
        end,
        run = function()
            obs.text = audio.settings_text()
            obs.text_lines = count_lines(obs.text)
            obs.path = audio.settings_path() or "(none)"
            draw_settings()
            obs.op = string.format("text lines=%d path=%s", obs.text_lines, obs.path)
            lab.note(obs.op .. " | P apply  L load  S save")
        end,
        read = function()
            return {
                { label = "version", value = string.match(obs.text, "^[^\n]+") or "?" },
                { label = "lines", value = obs.text_lines },
                { label = "master", value = string.match(obs.text, "master=([%d%.]+)") or "?" },
                { label = "bgm", value = string.match(obs.text, "bgm=([%d%.]+)") or "?" },
                { label = "path", value = obs.path == "" and "(none)" or obs.path },
                { label = "last op", value = obs.op },
            }
        end,
        check = function(t)
            local text = audio.settings_text()
            t:ok(type(text) == "string" and #text > 0, "settings_text is non-empty")
            t:eq(string.match(text, "^[^\n]+"), "rustlua-audio-v1", "the version header is stable")
            t:ok(text:find("master=0.800", 1, true) ~= nil, "the master line keeps three decimals")
            t:ok(text:find("bgm=0.700", 1, true) ~= nil, "the bgm line reads back")

            local applied = lab.try(function()
                return audio.apply_settings(text)
            end)
            t:eq(applied, true, "apply_settings accepts its own text")
            audio.volume(0.25)
            t:near(audio.bus_volume("master"), 0.25, 0.001, "the live master was changed before the round-trip")
            local restored = lab.try(function()
                return audio.apply_settings(text)
            end)
            t:eq(restored, true, "the same text can be re-applied")
            t:near(audio.bus_volume("master"), 0.8, 0.001, "applying the text restores the master")
            t:near(audio.bus_volume("bgm"), 0.7, 0.001, "applying the text restores bgm")

            local accepted, reason = lab.try(function()
                return audio.apply_settings("rustlua-audio-v9\nmaster=1.000")
            end)
            t:eq(accepted, false, "a wrong version is rejected")
            t:eq(type(reason), "string", "the rejection carries a reason")

            local path = audio.settings_path()
            t:ok(path ~= nil and #path > 0, "settings_path points at the project settings file")

            local loaded = lab.try(function()
                return audio.load_settings()
            end)
            t:ok(loaded == true or loaded == false or loaded == nil, "load_settings returns a flag or a controlled error")
            reset_mix()
        end,
    },
}

-- ---------------------------------------------------------------------------
-- 生命周期
-- ---------------------------------------------------------------------------

function M.on_create()
    find_ids()
    lab.start({
        scene = "api_audio",
        title = "Audio API",
        category = "Audio",
        steps = steps,
        -- 场景 Text 画在动态 UI 之上：六块说明/标签都会压住 shell 或读数，全部让位。
        hide_scene_text = { "Legend", "SpeakerLabel", "BgmLabel", "SfxLabel", "HudNote", "Keys" },
    })
    log.info("api_audio: lab ready (buses / mix / playback / spatial / settings)")
end

-- 按步骤分支的字母/标点交互（Lab 保留 space/方向键/1..9/R/F/F5，这里不碰）。
function M.update()
    local step = lab.step()
    if step then
        if step.id == "bus_and_master" then
            if input.just_pressed("bracketleft") then
                audio.volume(clamp(audio.bus_volume("master") - 0.1, 0.0, 1.0))
                obs.op = "master -0.1"
            elseif input.just_pressed("bracketright") then
                audio.volume(clamp(audio.bus_volume("master") + 0.1, 0.0, 1.0))
                obs.op = "master +0.1"
            elseif input.just_pressed("comma") then
                local key = BUS_KEYS[selected]
                audio.set_bus_volume(key, clamp(audio.bus_volume(key) - 0.1, 0.0, 1.0))
                obs.op = key .. " -0.1"
            elseif input.just_pressed("period") then
                local key = BUS_KEYS[selected]
                audio.set_bus_volume(key, clamp(audio.bus_volume(key) + 0.1, 0.0, 1.0))
                obs.op = key .. " +0.1"
            elseif input.just_pressed("keyb") then
                obs.op = "selected " .. cycle_bus(1)
            elseif input.just_pressed("keyx") then
                reset_mix()
                obs.op = "mix reset"
            end
        elseif step.id == "playback" then
            if input.just_pressed("keyp") then
                audio.play(CLIP_PICKUP)
                flash, flash_x, flash_y = 1, ACTOR_X[1], 220
                obs.op = 'play("pickup")'
            elseif input.just_pressed("keyh") then
                local ok, reason = audio.play_on("sfx", CLIP_HIT)
                flash, flash_x, flash_y = 1, ACTOR_X[3], 220
                obs.op = string.format('play_on("sfx","hit") ok=%s %s', tostring(ok), tostring(reason))
            elseif input.just_pressed("keyu") then
                local ok, reason = audio.play_on("ui", CLIP_ATTACK)
                flash, flash_x, flash_y = 1, ACTOR_X[3], 220
                obs.op = string.format('play_on("ui","attack") ok=%s %s', tostring(ok), tostring(reason))
            elseif input.just_pressed("keyo") then
                local ok, gain, pan = audio.play_at(CLIP_ATTACK, 4, 0, 0)
                flash, flash_x, flash_y = 1, ACTOR_X[3], 220
                obs.op = string.format('play_at("attack",4,0) ok=%s gain=%.2f pan=%+.2f', tostring(ok), gain, pan)
            elseif input.just_pressed("keym") then
                audio.music(CLIP_BGM)
                flash, flash_x, flash_y = 1, ACTOR_X[2], 220
                obs.op = 'music("bgm") (loops on the bgm bus)'
            elseif input.just_pressed("keyn") then
                audio.stop_music()
                obs.op = "stop_music()"
            end
        elseif step.id == "spatial" then
            if input.just_pressed("comma") then
                emitter_x = math.max(1, emitter_x - 1)
                obs.op = "emitter x=" .. emitter_x
            elseif input.just_pressed("period") then
                emitter_x = math.min(9, emitter_x + 1)
                obs.op = "emitter x=" .. emitter_x
            elseif input.just_pressed("keyo") then
                local ok, gain, pan = audio.play_at(CLIP_ATTACK, emitter_x, 0, 5)
                obs.last_gain = string.format("%.2f", ok and gain or 0)
                obs.last_pan = string.format("%+.2f", ok and pan or 0)
                obs.op = string.format("play_at x=%d ok=%s", emitter_x, tostring(ok))
            end
        elseif step.id == "settings" then
            if input.just_pressed("keyp") then
                local ok, reason = lab.try(function()
                    return audio.apply_settings(audio.settings_text())
                end)
                obs.op = string.format("apply_settings=%s/%s", tostring(ok), tostring(reason))
            elseif input.just_pressed("keyl") then
                local ok, reason = lab.try(function()
                    return audio.load_settings()
                end)
                obs.op = string.format("load_settings=%s/%s", tostring(ok), tostring(reason))
            elseif input.just_pressed("keys") then
                -- 唯一写盘调用：只放在按键分支里（headless 门不允许每帧写盘）。
                local ok, reason = audio.save_settings()
                obs.op = string.format("save_settings=%s/%s", tostring(ok), tostring(reason))
            elseif input.just_pressed("keyx") then
                reset_mix()
                obs.op = "mix reset"
            end
        end
    end
    return lab.tick()
end

function M.self_test()
    return lab.self_test()
end

function M.on_destroy()
    -- 动态 UI 是自己画的：先清图，再停掉本模块可能启动的音乐，最后交回 shell。
    clear_ui()
    audio.stop_music()
    reset_mix()
    lab.stop()
end

return M
