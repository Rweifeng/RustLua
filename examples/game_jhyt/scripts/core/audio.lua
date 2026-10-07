-- audio.lua：《江湖有田》音频运行时（M19-02 建立，M20-02 接入音量分组）。
--
-- 引擎事实（core/audio.rs + core/script.rs）：
--   * `audio.play(name)`        播放一次音效（Sfx 总线）
--   * `audio.play_on(bus,name)` 在指定总线播放一次音效（bus: master/bgm/sfx/ui）
--   * `audio.music(name)`       循环播放 BGM（Bgm 总线，改音量即时生效）
--   * `audio.stop_music()`      停止 BGM
--   * `audio.volume(v)`         主音量（0..1）
--   * `audio.set_bus_volume(bus,v)` / `audio.bus_volume(bus)` / `audio.effective_volume(bus)`
--     分组音量；实际音量 = 主音量 × 分组音量（M20-02）
--   * `audio.settings_text()` / `audio.apply_settings(text)` 音量设置文本往返
--   * `audio.load_settings()` / `audio.save_settings()` 读写
--     `<project>/project.toml` 的 `[audio]` 段（M20-02；段可选，缺失即默认 1.0）
--   * 音频目录固定为 `<project>/assets/audio/`，依次尝试 `<name>.mp3`、`<name>.wav`
--     与 `<name>.ogg`（本工程只用 .mp3）；无输出设备时引擎只 warn 并静默降级，不影响玩法。
--
-- 本模块职责：
--   * 按地图选曲（同一曲目不重启，切图才换曲）
--   * 包装 sfx/ui 调用：pcall 静默降级（缺文件 / 无设备 / 无 asset_dir 都
--     不得中断玩法逻辑）
--   * 音效走 SFX 总线、界面音走 UI 总线、BGM 走 BGM 总线（M20-02）
--   * 主音量随存档持久化（provider "audio" → { volume = 0..1 }）；分组音量以
--     project.toml 为准（`load_settings`/`save_settings`）
--
-- 只在「成功路径」播放音效的调用点在各玩法模块里（farm/shop/dialogue/...），
-- 本模块不反过来 require 它们（避免 require 环）。
local save_manager = require("core.save_manager")

local M = {}

-- 默认音量：0.7 比 1.0 更适合长时间游玩。
local DEFAULT_VOLUME = 0.7
local VOLUME_STEP = 0.1

-- 音量总线（顺序即暂停设置页展示顺序）；BUS_SET 用于按名字做成员判定
-- （数组表不能直接当集合用）。
local BUSES = { "master", "bgm", "sfx", "ui" }
local BUS_SET = { master = true, bgm = true, sfx = true, ui = true }

local TITLE_BGM = "bgm_title"
-- 地图 → BGM。未登记的地图回落到 bgm_wilds（新增地图不会静音）。
local MAP_BGM = {
  manor = "bgm_manor",
  village = "bgm_village",
  interior_herb = "bgm_village",
  interior_forge = "bgm_village",
  mountain_path = "bgm_wilds",
  cold_spring_cave = "bgm_wilds",
  combat_arena = "bgm_wilds"
}
local FALLBACK_BGM = "bgm_wilds"

-- 主音量本地缓存：引擎 bus API 不可用（旧引擎）时仍可工作。
local volume = DEFAULT_VOLUME
local current_bgm = nil -- 正在播放的曲目名；nil = 无 BGM
-- pcall 失败过的调用键（bus:name）：只记录一次，避免每帧刷日志。
local failed = {}

-- 最近播放过的音效（环形，最多 32 条）：给 `M.played()` 用。
--
-- 为什么需要：引擎的音频 API 没有「正在播放/播放次数」查询，headless 验收无法直接断言
-- 「选中移动时确实响了」。这里只在实际调用成功时记一行字符串，成本可忽略（音效调用本身
-- 就很少），也让玩家反馈「某个界面没声音」时能一眼看出是没调用还是被静音。
local played_log = {}
local PLAYED_LOG_MAX = 32

-- 最近播放的音效名（新→旧；`count` 缺省 = 全部缓存）。
function M.played(count)
  local total = #played_log
  local n = math.floor(tonumber(count) or total)
  if n < 0 then
    n = 0
  end
  if n > total then
    n = total
  end
  local out = {}
  for i = 1, n do
    out[i] = played_log[total - n + i]
  end
  return out
end

-- 是否刚播放过某音效（`within` = 最近几条内命中；缺省全部缓存）。
function M.played_recently(name, within)
  local wanted = tostring(name or "")
  if wanted == "" then
    return false
  end
  local recent = M.played(within or PLAYED_LOG_MAX)
  for _, entry in ipairs(recent) do
    if entry == wanted then
      return true
    end
  end
  return false
end

local function record_played(name)
  played_log[#played_log + 1] = name
  if #played_log > PLAYED_LOG_MAX then
    table.remove(played_log, 1)
  end
end

-- 当前引擎只暴露 log.info/warn/error（无 log.debug）；优先用 log.debug，
-- 未来引擎补齐后自动升级，仍缺失则回落到 log.info。
local function debug_log(message)
  local fn = log and (log.debug or log.info)
  if fn then
    pcall(fn, message)
  end
end

-- 静默调用引擎音频 API：任何 Lua 侧异常都不向上传播。
local function call(fn, ...)
  if type(fn) ~= "function" then
    return false
  end
  local ok, err = pcall(fn, ...)
  if not ok then
    debug_log("[audio] " .. tostring(err))
    return false
  end
  return true
end

local function clamp01(value)
  local v = tonumber(value)
  if not v then
    return nil
  end
  if v < 0 then
    return 0
  end
  if v > 1 then
    return 1
  end
  return v
end

-- 引擎侧分组音量；不支持/失败返回 nil（调用方回落到本地缓存/默认）。
local function engine_bus_volume(bus)
  if type(audio) ~= "table" or type(audio.bus_volume) ~= "function" then
    return nil
  end
  local ok, value = pcall(audio.bus_volume, bus)
  if not ok or type(value) ~= "number" then
    return nil
  end
  return clamp01(value)
end

local function engine_set_bus_volume(bus, value)
  if type(audio) ~= "table" or type(audio.set_bus_volume) ~= "function" then
    return false
  end
  return call(audio.set_bus_volume, bus, value)
end

-- 初始化（一次性引导 / 新档）：应用默认主音量与 project.toml 中的分组音量，
-- 并允许重新选曲。[audio] 段缺失时保持默认（1.0），不报错。
function M.init()
  volume = clamp01(volume) or DEFAULT_VOLUME
  engine_set_bus_volume("master", volume)
  call(audio.volume, volume)
  M.load_settings()
  current_bgm = nil
  failed = {}
  return true
end

-- 主音量：模块本地缓存是**读取来源**（与 M19-02 语义一致——模块是主音量的唯一
-- 事实来源，引擎只作为写入目标）。这样 `set_volume(0.45)` 读回仍是 0.45，而不是
-- f32 往返后的 0.44999998807907（既有验收断言依赖该语义）。
function M.volume()
  return volume
end

function M.set_volume(value)
  local v = clamp01(value)
  if not v then
    return false
  end
  volume = v
  engine_set_bus_volume("master", v)
  call(audio.volume, v)
  return true
end

-- 主音量 ±（暂停菜单主页面）：返回调整后的音量。
function M.adjust_volume(delta)
  local step = tonumber(delta) or 0
  M.set_volume(M.volume() + step)
  return M.volume()
end

-- ---------- 音量分组（M20-02） ----------

function M.buses()
  return BUSES
end

-- 分组独立音量（master 即本地主音量缓存）；未知名返回 nil。
function M.bus_volume(bus)
  if not BUS_SET[bus] then
    return nil
  end
  if bus == "master" then
    return volume
  end
  local engine = engine_bus_volume(bus)
  if engine ~= nil then
    return engine
  end
  return 1.0
end

-- 实际生效音量 = 主音量 × 分组音量；未知名返回 nil。
function M.effective_volume(bus)
  if not BUS_SET[bus] then
    return nil
  end
  if type(audio) == "table" and type(audio.effective_volume) == "function" then
    local ok, value = pcall(audio.effective_volume, bus)
    if ok and type(value) == "number" then
      return value
    end
  end
  local group = M.bus_volume(bus)
  if group == nil then
    return nil
  end
  if bus == "master" then
    return group
  end
  return (M.bus_volume("master") or volume) * group
end

function M.set_bus_volume(bus, value)
  if not BUS_SET[bus] then
    return false
  end
  local v = clamp01(value)
  if not v then
    return false
  end
  if bus == "master" then
    return M.set_volume(v)
  end
  if not engine_set_bus_volume(bus, v) then
    return false
  end
  return true
end

-- 分组音量 ±（暂停设置页）：返回调整后的分组音量（bus 非法返回 nil）。
function M.adjust_bus_volume(bus, delta)
  local current = M.bus_volume(bus)
  if current == nil then
    return nil
  end
  local step = tonumber(delta) or 0
  if not M.set_bus_volume(bus, current + step) then
    return current
  end
  return M.bus_volume(bus)
end

-- ---------- 设置文件（project.toml 的 [audio] 段，M20-02） ----------

function M.settings_path()
  if type(audio) == "table" and type(audio.settings_path) == "function" then
    local ok, path = pcall(audio.settings_path)
    if ok and type(path) == "string" then
      return path
    end
  end
  return nil
end

-- 读取工程设置并应用四路音量；返回 (ok, detail)。
-- 文件缺失（可选文件）返回 false, "missing" —— 不视为错误。
function M.load_settings()
  if type(audio) ~= "table" or type(audio.load_settings) ~= "function" then
    return false, "unsupported"
  end
  local ok, result, detail = pcall(audio.load_settings)
  if not ok then
    debug_log("[audio] load_settings: " .. tostring(result))
    return false, "error"
  end
  if result ~= true then
    return false, tostring(detail or "missing")
  end
  volume = engine_bus_volume("master") or volume
  return true, tostring(detail or "loaded")
end

-- 把当前四路音量写回工程设置；返回 (ok, detail)。
function M.save_settings()
  if type(audio) ~= "table" or type(audio.save_settings) ~= "function" then
    return false, "unsupported"
  end
  local ok, result, detail = pcall(audio.save_settings)
  if not ok then
    debug_log("[audio] save_settings: " .. tostring(result))
    return false, "error"
  end
  return result == true, tostring(detail or (result and "saved" or "error"))
end

-- 音量设置文本往返（不含 project.toml 的其余段）。
function M.settings_text()
  if type(audio) == "table" and type(audio.settings_text) == "function" then
    local ok, text = pcall(audio.settings_text)
    if ok and type(text) == "string" then
      return text
    end
  end
  return nil
end

function M.apply_settings(text)
  if type(text) ~= "string" or text == "" then
    return false, "no_text"
  end
  if type(audio) ~= "table" or type(audio.apply_settings) ~= "function" then
    return false, "unsupported"
  end
  local ok, result, detail = pcall(audio.apply_settings, text)
  if not ok then
    return false, "error"
  end
  volume = engine_bus_volume("master") or volume
  return result == true, tostring(detail or "")
end

-- ---------- BGM ----------

-- BGM 选曲：同一曲目不重启（返回 false, "same"），切图才换曲。
function M.play_bgm_name(name)
  if type(name) ~= "string" or name == "" then
    return false, "no_name"
  end
  if current_bgm == name then
    return false, "same"
  end
  current_bgm = name
  call(audio.music, name)
  debug_log("[audio] bgm=" .. name)
  return true
end

-- 按地图键播放 BGM（enter_playable_map / 标题）。
function M.play_bgm(map_id)
  if type(map_id) ~= "string" or map_id == "" then
    return false, "unknown_map"
  end
  return M.play_bgm_name(MAP_BGM[map_id] or FALLBACK_BGM)
end

function M.play_title()
  return M.play_bgm_name(TITLE_BGM)
end

function M.stop_bgm()
  current_bgm = nil
  call(audio.stop_music)
  return true
end

-- 当前曲目名（nil = 无 BGM）；headless 验收用。
function M.current_bgm()
  return current_bgm
end

-- ---------- 音效 ----------

-- 在指定总线上播放一次性音效；失败静默降级（同一 bus:name 只尝试一次）。
function M.play_on(bus, name)
  if type(name) ~= "string" or name == "" or not BUS_SET[bus] then
    return false
  end
  local key = tostring(bus) .. ":" .. name
  if failed[key] then
    return false
  end
  local fn = type(audio) == "table" and audio.play_on or nil
  local ok
  if type(fn) == "function" then
    local called, result = pcall(fn, bus, name)
    ok = called and result == true
  else
    -- 旧引擎：play 只有 SFX 总线，BGM/UI 需要时回落。
    ok = bus ~= "bgm" and call(audio.play, name)
  end
  if not ok then
    failed[key] = true
    return false
  end
  record_played(name)
  return true
end

-- 玩法音效（SFX 总线）。
function M.sfx(name)
  return M.play_on("sfx", name)
end

-- 界面音效（UI 总线；面板开闭/确认/取消）。
function M.ui(name)
  return M.play_on("ui", name)
end

-- ---------- 存档 / 复位 ----------

-- 新档复位：主音量沿用（属于玩家偏好），清曲目缓存以便重新起播。
function M.reset()
  current_bgm = nil
  failed = {}
  engine_set_bus_volume("master", volume)
  call(audio.volume, volume)
  return true
end

function M.capture()
  return { volume = M.volume() }
end

function M.restore(data)
  if type(data) ~= "table" then
    return
  end
  local v = clamp01(data.volume)
  if v then
    volume = v
  end
  engine_set_bus_volume("master", volume)
  call(audio.volume, volume)
end

-- 注册存档 provider（main.lua 的 register_save_providers 调用）。
function M.register_save()
  save_manager.register("audio", M.capture, M.restore)
  return true
end

M.VOLUME_STEP = VOLUME_STEP
M.DEFAULT_VOLUME = DEFAULT_VOLUME
M.MAP_BGM = MAP_BGM

return M
