-- api_audio：演示 audio 模块的**全部** 13 个函数（播放 / 音量分组（总线）/ 设置文件往返）。
--
-- 这个演示想证明什么：
--   ① 播放接口只提交"播放请求"，返回值比想象中少：`audio.play` / `audio.music` /
--      `audio.stop_music` **没有返回值**，只有 `audio.play_on` 返回 `ok, reason`。
--      没有输出设备时引擎**静默降级**（只记一行日志就跳过，M19-02 策略），所以
--      `ok=true` 的准确含义是"这个请求被接受了（文件读到了、能解码）"，而不是
--      "你一定听得见声音"。状态行把返回值原样贴出来，就是为了让这条语义可见。
--   ② 音量是**两级乘法**：`bus_volume(bus)` 是分组自己的音量（不含主音量），
--      `effective_volume(bus)` = 主音量 × 分组音量 —— 后者才是真正灌进音频 sink 的值。
--      `audio.volume(v)` 改的是主音量，`set_bus_volume(bus, v)` 改的是某一路；
--      master 那一路上两者等价（`bus_volume("master")` 读回来就是主音量）。
--   ③ 设置能往返、也能落盘：`settings_text()` 给出四路音量的**版本化文本**
--      （首行版本头 `rustlua-audio-v1`，之后每行 `bus=0.000`），`apply_settings(text)`
--      把它灌回引擎；`settings_path()` / `load_settings()` / `save_settings()` 读写工程
--      设置文件 `settings.toml` 的 `[audio]` 段。设置文件是**可选**的：缺失时
--      `load_settings()` 返回 `false, "missing"`，这是"第一次运行"的正常路径，不是错误。
--
-- 看什么：
--   * 三个精灵 = 三张音量表：Speaker（主音量 master）/ Bgm（bgm 总线）/ Sfx（sfx 与 ui 总线）。
--     **亮度 = 该通道的实际生效音量**（把主音量或该路分组音量调小，它就暗下去 ——
--     "主 × 分组"这层乘法因此看得见）；**往上跳一下 = 这一路刚发声**。
--     Bgm 金色 = 音乐在循环、灰色 = 已停；Sfx 白闪 = sfx 总线（按 2）、蓝闪 = ui 总线
--     （按 3；ui 没有专属精灵，所以借 Sfx 精灵用颜色区分，数值见左下状态行的 ui 一路）。
--   * 精灵下面五行是**每帧读回来的事实**：`buses()` 的固定顺序、四路的 独立/生效 音量、
--     `settings_text()` 的版本化文本、`settings_path()` 的路径、三个设置函数的返回值。
--   * 左下状态行：主音量、当前选中的总线、BGM 状态、三条通道的播放次数、最近一次操作。
--
-- 素材名（最容易踩的坑）：`audio.play/play_on/music` 的 `name` **不是工程相对路径**，
-- 而是**相对于 `assets/audio/` 的裸文件名** —— 宿主用
-- `set_audio_asset_dir(<project>/assets/audio)` 配好目录，引擎再依次试 `<name>.wav` 与
-- `<name>.ogg`（见 `core/audio.rs` 的 `sound_bytes`）。所以这里写 `"pickup"`；
-- 写成 `"assets/audio/pickup.ogg"` 会去找 `assets/audio/assets/audio/pickup.ogg.ogg` 而失败。
--
-- 硬规则（README §2）：顶层只定义函数与常量（不调用任何引擎 API）；`update()` 在 headless
-- （无窗口 / 无音频设备 / 无按键）下跑通 —— 它每帧只**读**引擎状态（含读设置文件），
-- 唯一会写盘的 `audio.save_settings()` 只待在按键分支里。
--
-- 为什么屏幕上的文案全是 ASCII：本工程 `assets/fonts/NotoSansSC-VF.ttf` 是被裁剪过的
-- **子集字体**（242KB，完整字体在 `engine/crates/editor/fonts/` 里是 17.7MB），
-- 它缺很多汉字字形（`ui.measure_text` 能直接量出来：`missing_count > 0`，渲染成占位方框）。
-- 所以"看得见的字"只用 ASCII，中文解释全部放在注释与日志里 —— 日志用编辑器自己的字体，
-- 不受这个子集影响。
local api = require("api_demo")

local M = {}

-- ---------------------------------------------------------------------------
-- 常量
-- ---------------------------------------------------------------------------

-- 素材名：相对 `assets/audio/` 的裸文件名（见文件头"素材名"一段）。四个文件都已存在：
-- pickup.ogg / hit.ogg / attack.ogg / bgm.ogg。
local CLIP_PICKUP = "pickup"
local CLIP_HIT = "hit"
local CLIP_ATTACK = "attack"
local CLIP_BGM = "bgm"

-- 每次按键的音量步长（超界由引擎钳制到 0..1，本文件不自己夹）。
local STEP = 0.1

-- 三个可视化通道：`entity_name` 是场景里的实体名（运行期用 `entity.find` 换 id），
-- `base` 是基色（只负责"看出这是哪一路"），亮度才是数据。
local CHANNELS = {
    {
        key = "speaker",
        entity_name = "Speaker",
        base = { 0.60, 0.82, 1.00 },
        flash_color = { 0.92, 0.96, 1.00 },
    },
    {
        key = "bgm",
        entity_name = "Bgm",
        base = { 1.00, 0.86, 0.38 },
        flash_color = { 1.00, 1.00, 0.80 },
    },
    {
        key = "sfx",
        entity_name = "Sfx",
        base = { 1.00, 0.62, 0.78 },
        flash_color = { 1.00, 1.00, 1.00 },
    },
}

-- 音乐停掉时 Bgm 精灵的底色（灰）：与播放中的金色对比，"BGM 在不在响"一眼可见。
local BGM_STOPPED_COLOR = { 0.42, 0.44, 0.52 }
-- ui 总线的一次性音效用**蓝色**闪光（ui 没有专属精灵，借 Sfx 精灵 + 颜色区分）。
local UI_FLASH_COLOR = { 0.45, 0.85, 1.00 }

-- 总线名 → 闪光通道：master 由 Speaker 精灵表达，bgm / sfx 各有自己的精灵，
-- ui 借 Sfx 精灵（蓝色闪光）。
local FLASH_CHANNEL = { master = "speaker", bgm = "bgm", sfx = "sfx", ui = "ui" }

-- 闪光与抬升：按键发声那一帧置满，之后线性衰减 —— 视觉上的"这一路刚响过"。
local FLASH_SECONDS = 0.45
local LIFT_PX = 16
-- 音乐循环播放时 Bgm 精灵的常驻抬升（"在响"的静态标记，不依赖闪光）。
local BGM_LIFT_PX = 10

-- 按键：全部是 **v2 物理键名**（`input.just_pressed` 的严格解析会拒绝拼错的键，
-- 大写/`key:` 前缀也接受，但这里统一小写形式，和 `input.rebind` 的绑定文本同源）。
local KEY_PLAY = "digit1"
local KEY_HIT = "digit2"
local KEY_UI = "digit3"
local KEY_MUSIC = "digit4"
local KEY_STOP = "digit5"
local KEY_BUS_NEXT = "tab"
local KEY_BUS_DOWN = "comma"
local KEY_BUS_UP = "period"
local KEY_MASTER_DOWN = "bracketleft"
local KEY_MASTER_UP = "bracketright"
local KEY_APPLY = "keyp"
local KEY_LOAD = "keyl"
local KEY_SAVE = "keys"

-- 本模块自己的动态 UI 行 id（`api_demo` 负责 api_title / api_status / api_hint 三行）。
local ROW_BUSES = "api_audio_buses"
local ROW_TABLE = "api_audio_table"
local ROW_TEXT = "api_audio_text"
local ROW_PATH = "api_audio_path"
local ROW_OPS = "api_audio_ops"

-- ---------------------------------------------------------------------------
-- 模块内状态（脚本侧的观察值，不是引擎状态）
-- ---------------------------------------------------------------------------

-- 每个通道的剩余闪光秒数（按键那一帧置 FLASH_SECONDS，之后每帧衰减）。
local flash = { speaker = 0, bgm = 0, sfx = 0, ui = 0 }
-- 1..3 → { id, x, y, head }：on_create 里从场景读回来的**基座标**与"除 tint 以外的
-- Sprite 组件串"。必须缓存：本演示每帧都会写回位置与 tint，不缓存就会把上一次的抬升
-- 当成新基座（位置一路飘走），或把上一次的 tint 当成原色（越调越暗）。
local actor = {}
-- BGM 状态：引擎没有"正在播什么 / 有没有在播"的查询接口，所以这里存的是
-- `audio.music()` / `audio.stop_music()` 的**最后一次调用意图**（不假装知道引擎内部状态）。
local music_playing = false
-- 当前选中的总线下标（1..4，顺序同 `buses()` 的返回值）：`,` / `.` 改的就是这一路。
local selected = 1

local S = {
    apply = "apply_settings(text) -> press P",
    load = "load_settings() -> see on_create result",
    save = "save_settings() -> press S to write",
    last = "no key pressed yet",
    settings_path = "(read in on_create)",
    sfx_plays = 0,
    bgm_plays = 0,
    ui_plays = 0,
}

-- ---------------------------------------------------------------------------
-- 纯值工具（不碰引擎 API）
-- ---------------------------------------------------------------------------

--- 浮点短文本：`short(56.0)` → `"56"`，`short(0.5)` → `"0.50"`。
local function short(value)
    if math.abs(value - math.floor(value + 0.5)) < 0.001 then
        return string.format("%d", math.floor(value + 0.5))
    end
    return string.format("%.2f", value)
end

--- 夹到 0..1（只用于颜色通道，音量本身交给引擎钳制）。
local function clamp01(value)
    if value < 0 then
        return 0
    end
    if value > 1 then
        return 1
    end
    return value
end

--- 亮度系数：0 音量 → 0.30（暗但还看得见），满音量 → 1.0（原色）。
--- 这就是 `effective_volume` 的可视化：主 × 分组之后剩多少，精灵就亮多少。
local function brightness(effective)
    return 0.30 + 0.70 * clamp01(tonumber(effective) or 0)
end

--- 颜色 = 基色 × 亮度，再朝 `flash_color` 混合 `t`（0..1，闪光强度）。
local function shade(base, effective, flash_color, t)
    local level = brightness(effective)
    local out = { 0, 0, 0 }
    for index = 1, 3 do
        local value = base[index] * level
        if t > 0 then
            value = value + (flash_color[index] - value) * t
        end
        out[index] = clamp01(value)
    end
    return out
end

--- 把一路通道画出来：**位置** = 基座标 - 抬升量，**颜色** = tint。
--- `transform.set(id, x, y)` 只改位置；`sprite.set(id, 组件串)` 重写整条 Sprite 组件串 ——
--- 除 tint 以外的部分用 on_create 从 `sprite.get` 读回的原值拼回去（贴图/尺寸/枢轴不改），
--- 这样"亮度 = 生效音量"这条映射不会顺手动到别的字段。
local function paint(index, color, lift)
    local state = actor[index]
    if state == nil then
        return
    end
    transform.set(state.id, state.x, state.y - lift)
    sprite.set(
        state.id,
        string.format("%s;tint=%.3f,%.3f,%.3f,1.000", state.head, color[1], color[2], color[3])
    )
end

-- ---------------------------------------------------------------------------
-- 场景生命周期
-- ---------------------------------------------------------------------------

--- 进入本场景一次：取实体 id、读回基座标与 Sprite 组件串，并做一次"开机应用玩家设置"。
function M.on_create()
    api.title("api_audio - play / buses / music / settings.toml")
    api.hint(
        "1 pickup  2 hit-sfx  3 attack-ui  4 music  5 stop_music | TAB bus  , . bus vol  "
            .. "[ ] master | P apply text  L load  S save | "
            .. api.KEYS_BACK
    )

    -- 三个精灵：id 用 `entity.find`（按实体名查），基座标与"除 tint 以外的 Sprite 串"
    -- 都从**场景里读回来**（`transform.get` / `sprite.get`），不在这里硬编码，
    -- 这样改了场景文件也不会与本文件漂移。查不到实体时该通道只是没有可视化，不影响其余演示。
    for index = 1, #CHANNELS do
        local channel = CHANNELS[index]
        local id = entity.find(channel.entity_name)
        if id == nil then
            actor[index] = nil
            log.warn("api_audio 场景里找不到实体 " .. channel.entity_name .. "（该通道没有可视化）")
        else
            local position = transform.get(id) -- 2D 变换：x / y（单位 = 屏幕像素）
            local info = sprite.get(id) -- 贴图 / 尺寸 / 枢轴 / 当前 tint
            if info.texture == nil then
                -- 场景里的 Sprite 组件一定会同时建出 SpriteAsset，所以这里理论上读得到贴图；
                -- 真读不到就让这一路没有可视化，而不是让 string.format 卡住整个 update。
                actor[index] = nil
                log.warn("api_audio 实体 " .. channel.entity_name .. " 读不回贴图，跳过可视化")
            else
                actor[index] = {
                    id = id,
                    x = position.x,
                    y = position.y,
                    -- `sprite.set` 要一条完整的 Sprite 组件串（`texture=` 必须在第一段），
                    -- 这里只把 tint 留出来由每帧填，其余字段原样保留。
                    head = string.format(
                        "texture=%s;size=%sx%s;pivot=%.3f,%.3f;sorting_layer=default;order=1",
                        info.texture,
                        short(info.size_x),
                        short(info.size_y),
                        info.pivot_x,
                        info.pivot_y
                    ),
                }
            end
        end
    end

    -- `audio.settings_path()`：设置文件（`settings.toml`）的绝对路径；无工程根返回 nil，
    -- 发布包里由宿主指向**用户数据目录**（`%APPDATA%\<name>\settings.toml`）。
    local settings_path = audio.settings_path()
    if settings_path == nil then
        settings_path = "(no project root / settings_root unset)"
    end
    S.settings_path = settings_path

    -- `audio.load_settings()`：读 `settings.toml` 的 `[audio]` 段并**应用**四路音量。
    -- 在 on_create 调一次是安全的：它是**读**，不写盘（headless 门允许）。
    -- 文件缺失 → `false, "missing"`：设置文件本来就是可选的，这条路径不是错误。
    local ok, detail = audio.load_settings()
    S.load = string.format('load_settings() -> %s, "%s"', tostring(ok), tostring(detail))
    log.info("api_audio on_create " .. S.load .. " path=" .. tostring(settings_path))
end

--- 每个固定步一次。顺序是刻意的：先按键（只改世界），**之后**才读回事实并画出来 ——
--- 因此屏幕上的每个数字都是"这一帧刚改完的值"，不会慢一帧。
function M.update()
    local dt = time.delta()

    -- ① 事实源：`buses()` 返回**固定顺序**的四个总线名（master / bgm / sfx / ui）——
    --    顺序本身就是这个接口的契约（设置面板、本演示的 TAB 循环选中都按它来）。
    local buses = audio.buses()
    if #buses == 0 then
        -- 引擎保证四个名字；真出现空表就只报告，不让 update 报错（headless 门要能跑通）。
        api.status("audio.buses() returned an empty table (should not happen)")
        return "api_audio_ok buses=0"
    end
    if selected > #buses then
        selected = 1
    end

    -- ② 按键分支：所有播放 / 停止 / 改音量 / 读写设置都只在这里发生。
    --    headless 步进没有输入，因此这一段在测试里永不执行（写盘的 save_settings 就在其中）。
    if input.just_pressed(KEY_PLAY) then
        -- `audio.play(name)`：等价于 `play_on("sfx", name)`，**没有返回值**。
        -- 成功与否只能看引擎日志（失败会记一行 `[audio] play failed`）；
        -- 没有音频设备时它同样安静地返回 —— 这就是"静默降级"。
        audio.play(CLIP_PICKUP)
        S.sfx_plays = S.sfx_plays + 1
        flash.sfx = FLASH_SECONDS
        S.last = string.format('play("%s") -> no return value (= play_on("sfx", ...))', CLIP_PICKUP)
        log.info("api_audio " .. S.last)
    end

    if input.just_pressed(KEY_HIT) then
        -- `audio.play_on(bus, name)`：唯一有返回值的播放接口，`ok, reason`。
        -- `ok=true` 只说明**请求被接受**（找到文件、能解码）；无输出设备时引擎静默降级，
        -- 仍然返回 true 并且只记一行日志 —— 所以 true 不等于"你听到了"。
        local ok, reason = audio.play_on("sfx", CLIP_HIT)
        S.sfx_plays = S.sfx_plays + 1
        flash.sfx = FLASH_SECONDS
        S.last = string.format(
            'play_on("sfx", "%s") -> ok=%s reason="%s"',
            CLIP_HIT,
            tostring(ok),
            tostring(reason)
        )
        log.info("api_audio " .. S.last)
    end

    if input.just_pressed(KEY_UI) then
        -- 同一条音效走**另一条总线**：ui（界面音）。总线不同 → 生效音量不同
        -- （主 × ui），精灵闪光颜色也不同（蓝），这就是"分组"的意义。
        local ok, reason = audio.play_on("ui", CLIP_ATTACK)
        S.ui_plays = S.ui_plays + 1
        flash.ui = FLASH_SECONDS
        S.last = string.format(
            'play_on("ui", "%s") -> ok=%s reason="%s" (ui bus %.2f)',
            CLIP_ATTACK,
            tostring(ok),
            tostring(reason),
            audio.bus_volume("ui")
        )
        log.info("api_audio " .. S.last)
    end

    if input.just_pressed(KEY_MUSIC) then
        -- `audio.music(name)`：在 **bgm 总线**上循环播放（重复调用会先停掉上一首），
        -- 同样**没有返回值**。引擎不回报"音乐在不在播"，所以下面的 music_playing
        -- 记的是本脚本的调用意图，只用来画精灵与状态行。
        audio.music(CLIP_BGM)
        music_playing = true
        S.bgm_plays = S.bgm_plays + 1
        flash.bgm = FLASH_SECONDS
        S.last = string.format('music("%s") -> no return value (loops on the bgm bus)', CLIP_BGM)
        log.info("api_audio " .. S.last)
    end

    if input.just_pressed(KEY_STOP) then
        -- `audio.stop_music()`：停掉常驻音乐 sink；本来就没在播时是**空操作**（不报错）。
        audio.stop_music()
        music_playing = false
        S.last = "stop_music() -> no return value"
        log.info("api_audio " .. S.last)
    end

    if input.just_pressed(KEY_MASTER_UP) or input.just_pressed(KEY_MASTER_DOWN) then
        local up = input.just_pressed(KEY_MASTER_UP)
        local before = audio.bus_volume("master")
        local target = before + (up and STEP or -STEP)
        -- `audio.volume(v)`：主音量，超界由引擎钳制到 0..1（`AudioSystem::set_volume`）。
        -- 紧接着读回来：`bus_volume("master")` 与 `audio.volume` 是同一份值。
        audio.volume(target)
        local after = audio.bus_volume("master")
        flash.speaker = FLASH_SECONDS
        S.last = string.format(
            'volume(%.3f) -> bus_volume("master")=%.3f (clamped by the engine)',
            target,
            after
        )
        log.info("api_audio " .. S.last)
    end

    if input.just_pressed(KEY_BUS_NEXT) then
        -- 在 `buses()` 的固定顺序里循环选中（顺序本身就是这个接口的契约）。
        selected = (selected % #buses) + 1
        S.last = string.format("selected bus = %s (%d / %d)", tostring(buses[selected]), selected, #buses)
        log.info("api_audio " .. S.last)
    end

    if input.just_pressed(KEY_BUS_UP) or input.just_pressed(KEY_BUS_DOWN) then
        local up = input.just_pressed(KEY_BUS_UP)
        local bus = buses[selected]
        local before = audio.bus_volume(bus)
        local target = before + (up and STEP or -STEP)
        -- `audio.set_bus_volume(bus, v)`：改**某一路**的音量（超界钳制），返回 true。
        -- 总线名写错会报 Lua 错误，所以这里用 `buses()` 给的名字（永远合法）。
        -- 选中的是 master 时这一条与 `audio.volume` 等价（引擎里 Master 走同一个字段）。
        local ok = audio.set_bus_volume(bus, target)
        local after = audio.bus_volume(bus)
        -- 改哪一路就闪哪个精灵（master -> Speaker，bgm -> Bgm，sfx / ui -> Sfx）。
        flash[FLASH_CHANNEL[bus] or "sfx"] = FLASH_SECONDS
        S.last = string.format(
            'set_bus_volume("%s", %.3f) -> %s; bus=%.2f effective=%.2f',
            bus,
            target,
            tostring(ok),
            after,
            audio.effective_volume(bus)
        )
        log.info("api_audio " .. S.last)
    end

    if input.just_pressed(KEY_APPLY) then
        -- `settings_text()` → `apply_settings(text)` 往返。为了证明"文本真的带着值"
        -- （而不是一个恒返回 true 的空操作），先把主音量砸到 0.05，再灌回**刚读出的文本**，
        -- 然后读回主音量：它必须回到文本里的那个数。
        local text = audio.settings_text()
        local before = audio.bus_volume("master")
        audio.volume(0.05)
        local ok, reason = audio.apply_settings(text)
        local after = audio.bus_volume("master")
        local from_text = tonumber(string.match(text, "master=([%d%.]+)"))
        -- 成功的 `reason` 是空串（失败才是版本头不符之类的原因），所以状态行只贴 ok 与两个数值。
        local _ = reason
        S.apply = string.format(
            'apply_settings(text) -> ok=%s; master %.2f -> 0.05 -> %.2f (text says %s)',
            tostring(ok),
            before,
            after,
            from_text and string.format("%.3f", from_text) or "?"
        )
        S.last = S.apply
        flash.speaker = FLASH_SECONDS
        log.info("api_audio " .. S.apply)
    end

    if input.just_pressed(KEY_LOAD) then
        -- `audio.load_settings()`：读 `settings.toml` 的 `[audio]` 段并应用到内存。
        -- 文件缺失（可选文件）→ `false, "missing"`；没有工程根 → `false, "no_project_root"`。
        local ok, detail = audio.load_settings()
        S.load = string.format('load_settings() -> %s, "%s"', tostring(ok), tostring(detail))
        S.last = S.load
        log.info("api_audio " .. S.last)
    end

    if input.just_pressed(KEY_SAVE) then
        -- `audio.save_settings()`：把当前四路音量**保序**写回 `settings.toml`（保留注释与其他段）。
        -- 这是本演示里**唯一会写盘**的调用，所以它只待在按键分支里：headless 门会每帧步进，
        -- 放在 `update()` 顶层就会变成"每帧写一次盘"。
        local ok, detail = audio.save_settings()
        S.save = string.format('save_settings() -> %s, "%s"', tostring(ok), tostring(detail))
        -- 写完立刻读回来一次：`load_settings` 是这条链路的下半截，一起看才知道真的落盘了。
        local read_ok, read_detail = audio.load_settings()
        S.load = string.format('load_settings() -> %s, "%s"', tostring(read_ok), tostring(read_detail))
        S.last = S.save .. " ; then " .. S.load
        log.info("api_audio " .. S.last)
    end

    -- ③ 读回"这一帧的事实"。刻意放在按键分支**之后**：本演示的卖点就是"屏幕上的数字就是你
    --    刚改完的值"，若在按键前读，按一下 `[` 要等下一帧才看到主音量变化。
    --    对每个总线名各读两次：`bus_volume`（分组自己的音量，不含主音量）与
    --    `effective_volume`（主 × 分组，真正灌进 sink 的值）。喂进去的名字都来自 `buses()`，
    --    所以永远是合法总线名 —— 拼错名字会直接报 Lua 错误（`unknown audio bus: ...`），
    --    这是引擎刻意的设计，本演示用"只用引擎给的名字"来规避。
    local own = {}
    local effective = {}
    local bus_parts = {}
    for index = 1, #buses do
        local bus = buses[index]
        own[bus] = audio.bus_volume(bus)
        effective[bus] = audio.effective_volume(bus)
        bus_parts[index] = string.format("%s %.2f->%.2f", bus, own[bus], effective[bus])
    end

    -- 主音量：`bus_volume("master")` 读回来的就是 `audio.volume(v)` 写进去的那个值
    -- （`AudioSystem::bus_volume(Master)` 直接返回 `volume`，`effective_volume("master")` 同理）。
    local master = audio.bus_volume("master")
    local master_effective = audio.effective_volume("master")

    -- `settings_text()`：四路音量的版本化文本（首行 `rustlua-audio-v1`，之后每行 `bus=0.000`）。
    -- 每帧读一次是纯内存操作（不写盘）；显示时把换行折成 " | "，好让人看到行的格式。
    local settings_text = audio.settings_text()
    local settings_oneline = string.gsub(settings_text, "\n", " | ")

    -- ④ 闪光衰减（按键那一帧置满，之后每帧线性落下）。
    for index = 1, #CHANNELS do
        local key = CHANNELS[index].key
        if flash[key] > 0 then
            flash[key] = math.max(0, flash[key] - dt)
        end
    end

    -- ⑤ 三张"音量表"：亮度 = 实际生效音量，位置 = 这一路刚发声就往上跳。
    local speaker_flash = flash.speaker / FLASH_SECONDS
    paint(
        1,
        shade(CHANNELS[1].base, master_effective, CHANNELS[1].flash_color, speaker_flash),
        LIFT_PX * speaker_flash
    )

    -- Bgm：在循环就金色 + 常驻抬升（"在响"），停了就灰色 + 落回基座（"没响"）。
    local bgm_flash = flash.bgm / FLASH_SECONDS
    local bgm_color = music_playing and CHANNELS[2].base or BGM_STOPPED_COLOR
    paint(
        2,
        shade(bgm_color, effective["bgm"] or 0, CHANNELS[2].flash_color, bgm_flash),
        (music_playing and BGM_LIFT_PX or 0) + LIFT_PX * bgm_flash
    )

    -- Sfx：白闪 = sfx 总线（按 2），蓝闪 = ui 总线（按 3）；两者同帧时用更强的那个。
    local sfx_flash = math.max(flash.sfx, flash.ui) / FLASH_SECONDS
    local sfx_flash_color = (flash.ui > flash.sfx) and UI_FLASH_COLOR or CHANNELS[3].flash_color
    paint(
        3,
        shade(CHANNELS[3].base, effective["sfx"] or 0, sfx_flash_color, sfx_flash),
        LIFT_PX * sfx_flash
    )

    -- ⑥ 五行"每帧读回来的事实"（`ui.text_at` 是屏幕空间的运行期文字，位置固定）。
    --    每行都刻意压在一屏宽度内（用 `ui.measure_text` 量过），不让内容被右边缘切掉。
    ui.text_at(
        ROW_BUSES,
        24,
        348,
        string.format(
            "buses() -> [%s]  (fixed order; selected = %d)",
            table.concat(buses, ", "),
            selected
        ),
        1.0
    )
    ui.text_at(
        ROW_TABLE,
        24,
        372,
        "bus_volume -> effective_volume (master x bus): " .. table.concat(bus_parts, " | "),
        1.0
    )
    ui.text_at(ROW_TEXT, 24, 396, "settings_text(): " .. settings_oneline, 1.0)
    ui.text_at(ROW_PATH, 24, 420, "settings_path(): " .. tostring(S.settings_path), 1.0)
    ui.text_at(ROW_OPS, 24, 444, S.apply .. " | " .. S.load .. " | " .. S.save, 1.0)

    -- ⑦ 本帧的事实（左下状态行）。
    api.status(
        string.format(
            "master=%.2f | selected=%s bus=%.2f eff=%.2f | BGM=%s | plays sfx=%d bgm=%d ui=%d | last: %s",
            master,
            tostring(buses[selected]),
            own[buses[selected]] or -1,
            effective[buses[selected]] or -1,
            music_playing and "looping" or "stopped",
            S.sfx_plays,
            S.bgm_plays,
            S.ui_plays,
            S.last
        )
    )

    return string.format(
        "api_audio_ok master=%.2f buses=%s selected=%s music=%s sfx=%d bgm=%d ui=%d",
        master,
        table.concat(buses, ","),
        tostring(buses[selected]),
        music_playing and "on" or "off",
        S.sfx_plays,
        S.bgm_plays,
        S.ui_plays
    )
end

--- 离开场景：注销本模块的动态 UI。`ui.clear(id)` 负责该 id 下的面板/图片/网格；
--- 运行期**文字**（`ui.text_at` 写进 `state.ui`）由宿主在切场景时整体清掉
--- （`script.rs` 的 `state.ui.clear()`），所以这里和 `api.clear()` 一样只是把
--- "这个 id 归我管"这件事写清楚，不留悬空的图片/面板。
function M.on_destroy()
    ui.clear(ROW_BUSES)
    ui.clear(ROW_TABLE)
    ui.clear(ROW_TEXT)
    ui.clear(ROW_PATH)
    ui.clear(ROW_OPS)
    api.clear()
end

return M
