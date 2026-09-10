-- save_manager.lua：游戏级 Lua 存档注册中心。
-- save.blob_* 格式保持不变；快照版本、摘要和验证入口均在 blob 内完成。
--
-- 快照结构（SNAPSHOT_VERSION=2）：
--   * version/format：版本门与格式标记，restore 先校验再恢复。
--   * providers[name]：各注册 provider 的捕获数据（事实来源）。
--   * 顶层 name 字段：与 providers[name] 同一对象的别名，仅为兼容早期
--     blob（v1 时代顶层直写）的读取路径；恢复时优先读 providers，
--     缺省回退到顶层。保持双写是刻意取舍，勿在重构时移除顶层。
local M = {}

local SAVE_KEY = "jianghu_youtian.state"
local SNAPSHOT_VERSION = 2
local providers = {}
local registration_order = {}
local last_summary = nil

local function clone(value, seen)
  if type(value) ~= "table" then
    return value
  end
  seen = seen or {}
  if seen[value] then
    return seen[value]
  end
  local copy = {}
  seen[value] = copy
  for key, item in pairs(value) do
    copy[clone(key, seen)] = clone(item, seen)
  end
  return copy
end

local function log_failure(operation, reason)
  log.warn("[save][" .. operation .. "] " .. tostring(reason))
end

-- 同名重复注册是幂等的；只有提供器实现发生变化时才替换。
function M.register(name, capture, restore)
  if type(name) ~= "string" or name == "" or type(capture) ~= "function" then
    return false, "invalid provider registration"
  end
  local previous = providers[name]
  if previous and previous.capture == capture and previous.restore == restore then
    return true
  end
  if not previous then
    registration_order[#registration_order + 1] = name
  end
  providers[name] = { capture = capture, restore = restore }
  return true
end

function M.capture()
  local snapshot = { version = SNAPSHOT_VERSION, format = "jhyt.snapshot", providers = {} }
  for _, name in ipairs(registration_order) do
    local provider = providers[name]
    local ok, data = pcall(provider.capture)
    if not ok then
      return nil, "capture failed for provider " .. name .. ": " .. tostring(data)
    end
    snapshot.providers[name] = clone(data)
    snapshot[name] = snapshot.providers[name]
  end
  return snapshot
end

function M.summary(snapshot)
  snapshot = snapshot or select(1, M.capture())
  if type(snapshot) ~= "table" then
    return nil, "snapshot is not a table"
  end
  local scene_data = snapshot.scene or {}
  local clock_data = snapshot.clock or {}
  local farm_data = snapshot.farm or {}
  local plots = farm_data.plots or {}
  local plot_count = 0
  for _ in pairs(plots) do
    plot_count = plot_count + 1
  end
  local summary = {
    version = tonumber(snapshot.version) or 0,
    scene = tostring(scene_data.name or "unknown"),
    map = tostring(scene_data.map or "unknown"),
    day = tonumber(clock_data.day) or 0,
    minute = tonumber(clock_data.minute) or 0,
    farm_plots = plot_count,
    position_mode = tostring((snapshot.player or {}).mode or "unavailable")
  }
  return summary
end

function M.restore(snapshot)
  if type(snapshot) ~= "table" then
    return false, "snapshot is not a table"
  end
  local version = tonumber(snapshot.version) or 0
  if version == 1 then
    return false, "legacy snapshot version 1 is explicitly rejected; start a new game"
  end
  if version ~= SNAPSHOT_VERSION then
    return false, "unsupported snapshot version: " .. tostring(version)
  end
  local scene_data = snapshot.scene
  if type(scene_data) ~= "table" or type(scene_data.name) ~= "string" then
    return false, "snapshot scene summary is missing"
  end
  if scene.reload() ~= scene_data.name then
    return false, "saved scene is not the instantiated scene: " .. scene_data.name
  end
  for _, name in ipairs(registration_order) do
    local provider = providers[name]
    local data = snapshot.providers and snapshot.providers[name] or snapshot[name]
    if data ~= nil and provider.restore then
      local ok, reason = pcall(provider.restore, clone(data))
      if not ok then
        return false, "restore failed for provider " .. name .. ": " .. tostring(reason)
      end
    end
  end
  last_summary = M.summary(snapshot)
  return true
end

-- 落盘失败只放弃本次提交；save.commit 的原子语义保证已有有效文件不被覆盖。
function M.save()
  local snapshot, reason = M.capture()
  if not snapshot then
    log_failure("capture", reason)
    return false
  end
  local ok, write_reason = pcall(save.blob_write, SAVE_KEY, snapshot)
  if not ok then
    log_failure("write", write_reason)
    return false
  end
  local committed, commit_reason = pcall(save.commit)
  if not committed or not commit_reason then
    log_failure("commit", committed and "commit returned false" or commit_reason)
    return false
  end
  last_summary = M.summary(snapshot)
  log.info("[save] committed " .. tostring(last_summary.scene) .. " day=" .. tostring(last_summary.day))
  return true
end

-- 只读取盘：把磁盘 save_data 载入内存（含 __map/__mapstate:* 键），返回解码后
-- 的 snapshot；不做任何 restore。标题页 BtnContinue 用它决定读档后切到哪张地图
-- （M17-01 多地图继续）；随后场景 on_create 的 boot 阶段会再调 M.load 完成恢复。
function M.load_raw()
  local loaded, load_reason = pcall(save.load)
  if not loaded or not load_reason then
    log_failure("load", loaded and "load returned false" or load_reason)
    return nil, load_reason
  end
  local ok, snapshot = pcall(save.blob_read, SAVE_KEY)
  if not ok then
    log_failure("read", snapshot)
    return nil, snapshot
  end
  if type(snapshot) ~= "table" then
    return nil, "snapshot is missing"
  end
  return snapshot, nil
end

-- 存档所在的地图 id（M17-01）：标题页点击「继续」时调用，据此把场景切到
-- 存档记录的地图而非固定山庄。无存档/坏档返回 nil（调用方降级到 main）。
function M.peek_map()
  local snapshot, reason = M.load_raw()
  if not snapshot then
    return nil, reason
  end
  local scene_data = snapshot.scene
  if type(scene_data) ~= "table" then
    return nil, "snapshot has no scene summary"
  end
  local map_id = scene_data.map or scene_data.name
  if type(map_id) ~= "string" then
    return nil, "snapshot scene has no map"
  end
  return map_id, nil
end

function M.load()
  local snapshot, reason = M.load_raw()
  if not snapshot then
    return false, reason
  end
  local restored, restore_reason = M.restore(snapshot)
  if not restored then
    log_failure("restore", restore_reason)
  end
  return restored, restore_reason
end

function M.last_summary()
  return clone(last_summary)
end

-- 纯 Lua 长跑入口：不伪称替代真实引擎读档，仅验证快照在 30 天/100 次恢复中的稳定性。
function M.validate_long_run(days, loads)
  days = tonumber(days) or 30
  loads = tonumber(loads) or 100
  if days < 1 or loads < 1 then
    return false, "days and loads must be positive"
  end
  local snapshot, reason = M.capture()
  if not snapshot then
    log_failure("validate", reason)
    return false, reason
  end
  local count = 0
  for _ = 1, days do
    for _ = 1, loads do
      local ok, restore_reason = M.restore(clone(snapshot))
      if not ok then
        log_failure("validate", "load " .. tostring(count + 1) .. ": " .. tostring(restore_reason))
        return false, restore_reason
      end
      count = count + 1
    end
  end
  local report = { days = days, loads_per_day = loads, total_loads = count, summary = M.summary(snapshot) }
  log.info("[save][validate] completed snapshot-only run: " .. tostring(count) .. " loads")
  return true, report
end

return M
