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
-- 最近一次读档的结果（`{ ok, status, reason }`，status ∈ loaded/missing/corrupt/error）：
-- headless 验收、暂停菜单与诊断面板据此区分"没有存档"与"存档已损坏"。
local last_load = nil

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
-- `validate(data)` 可选：restore 前的**纯只读**预校验（不写任何状态），
-- 返回 `ok, reason`（ok 为 false 即判定该 provider 的数据不可用）。
function M.register(name, capture, restore, validate)
  if type(name) ~= "string" or name == "" or type(capture) ~= "function" then
    return false, "invalid provider registration"
  end
  if validate ~= nil and type(validate) ~= "function" then
    return false, "provider validate must be a function"
  end
  local previous = providers[name]
  if previous and previous.capture == capture and previous.restore == restore and previous.validate == validate then
    return true
  end
  if not previous then
    registration_order[#registration_order + 1] = name
  end
  providers[name] = { capture = capture, restore = restore, validate = validate }
  return true
end

-- 取某个 provider 在快照里的数据：优先 providers[name]，回退顶层同名字段
-- （v1 时代顶层直写），两者都没有返回 nil（表示"该 provider 不入档"）。
local function provider_data(snapshot, name)
  if type(snapshot.providers) == "table" and snapshot.providers[name] ~= nil then
    return snapshot.providers[name]
  end
  return snapshot[name]
end

-- v1 → v2 迁移：v1 快照没有 providers 包装，各 provider 数据直接写在顶层。
-- 迁移只搬运**已注册 provider** 的字段，不做猜测；不做迁移而直接拒绝会让每个
-- 发布版本都作废玩家存档。
local function migrate(snapshot)
  local migrated = {
    version = SNAPSHOT_VERSION,
    format = snapshot.format or "jhyt.snapshot",
    providers = {}
  }
  local moved = 0
  for _, name in ipairs(registration_order) do
    local data = provider_data(snapshot, name)
    if data ~= nil then
      migrated.providers[name] = clone(data)
      migrated[name] = migrated.providers[name]
      moved = moved + 1
    end
  end
  log.info("[save] migrated snapshot v" .. tostring(snapshot.version) .. " -> v" .. tostring(SNAPSHOT_VERSION)
    .. " (" .. tostring(moved) .. " provider(s))")
  return migrated
end

-- 逐 provider 应用快照，**收集**全部失败而不是遇到第一个就返回：中途 abort
-- 会让内存停在半恢复状态（前面的 provider 已写入、后面的完全没跑）。
local function apply_providers(snapshot, failures)
  for _, name in ipairs(registration_order) do
    local provider = providers[name]
    if provider and provider.restore then
      local data = provider_data(snapshot, name)
      if data ~= nil then
        local ok, reason, detail = pcall(provider.restore, clone(data))
        if not ok then
          failures[#failures + 1] = name .. ": " .. tostring(reason)
        elseif reason == false then
          -- provider 显式返回 `false, reason`（如 player.restore 找不到实体）
          -- 也算失败：静默吞掉它会让玩家坐标/装备无声错位。
          failures[#failures + 1] = name .. ": " .. tostring(detail or "restore rejected")
        end
      end
    end
  end
  return failures
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
  if version == 0 then
    return false, "snapshot has no version"
  end
  if version > SNAPSHOT_VERSION then
    -- 更高版本 = 更新引擎写的档：明确拒绝，绝不用旧解析器猜新格式。
    return false, "unsupported snapshot version: " .. tostring(version)
  end
  if version < SNAPSHOT_VERSION then
    snapshot = migrate(snapshot)
  end
  local scene_data = snapshot.scene
  if type(scene_data) ~= "table" or type(scene_data.name) ~= "string" then
    return false, "snapshot scene summary is missing"
  end
  if scene.reload() ~= scene_data.name then
    return false, "saved scene is not the instantiated scene: " .. scene_data.name
  end

  -- ① 预校验：只读检查（provider 可选提供 validate），任何一条不通过就整体不恢复。
  local rejected = {}
  for _, name in ipairs(registration_order) do
    local provider = providers[name]
    if provider and provider.validate then
      local data = provider_data(snapshot, name)
      if data ~= nil then
        local ok, reason = pcall(provider.validate, data)
        if not ok then
          rejected[#rejected + 1] = name .. ": " .. tostring(reason)
        elseif reason == false then
          rejected[#rejected + 1] = name .. ": " .. "data rejected by validate"
        end
      end
    end
  end
  if #rejected > 0 then
    return false, "snapshot rejected before restore: " .. table.concat(rejected, "; ")
  end

  -- ② 记录当前状态，作为应用失败时的回滚点。捕获失败则放弃读档（宁可保持
  -- 现状也不要赌一个无法回滚的部分恢复）。
  local rollback, rollback_reason = M.capture()
  if not rollback then
    return false, "cannot snapshot current state before restore: " .. tostring(rollback_reason)
  end

  -- ③ 全部应用；失败则回滚到 ② 的快照，绝不留下半恢复的内存状态。
  local failures = apply_providers(snapshot, {})
  if #failures > 0 then
    local rollback_failures = apply_providers(rollback, {})
    if #rollback_failures > 0 then
      log_failure("restore", "rollback incomplete: " .. table.concat(rollback_failures, "; "))
    end
    return false, "restore failed (" .. table.concat(failures, "; ") .. ")"
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
-- 的 snapshot；不做任何 restore。门面 BtnContinue 用它决定读档后切到哪张地图
-- （M17-01 多地图继续）；随后场景 on_create 的 boot 阶段会再调 M.load 完成恢复。
function M.load_raw()
  -- save.load 返回 (ok, reason)：reason ∈ {"missing", "no_save_path", "corrupt: ..."}。
  -- 旧代码只把 pcall 的第一个返回值当结果，"存档不存在"和"存档已损坏"因此无法
  -- 区分——玩家进度丢了和从没存过，在 UI 与诊断里必须给出不同说法。
  local ok, loaded, reason = pcall(save.load)
  if not ok then
    log_failure("load", loaded)
    last_load = { ok = false, status = "error", reason = tostring(loaded) }
    return nil, "load error: " .. tostring(loaded)
  end
  if not loaded then
    local text = tostring(reason)
    if text == "missing" then
      log.info("[save] 没有存档文件（按首次进入处理）")
      last_load = { ok = false, status = "missing", reason = text }
    elseif text:sub(1, 8) == "corrupt:" then
      log.error("[save] 存档已损坏，原文件保留不覆盖：" .. text)
      last_load = { ok = false, status = "corrupt", reason = text }
    else
      log.warn("[save] 读档失败：" .. text)
      last_load = { ok = false, status = "error", reason = text }
    end
    return nil, reason
  end
  local read_ok, snapshot = pcall(save.blob_read, SAVE_KEY)
  if not read_ok then
    log_failure("read", snapshot)
    last_load = { ok = false, status = "error", reason = tostring(snapshot) }
    return nil, snapshot
  end
  if type(snapshot) ~= "table" then
    last_load = { ok = false, status = "error", reason = "snapshot is missing" }
    return nil, "snapshot is missing"
  end
  last_load = { ok = true, status = "loaded", reason = nil }
  return snapshot, nil
end

-- 存档所在的地图 id（M17-01）：门面点击「继续」时调用，据此把场景切到
-- 存档记录的地图而非固定山庄。无存档/坏档返回 nil（调用方降级到 manor）。
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

-- 最近一次读档状态（只读）；`status` 用于把「没有存档」与「存档损坏」分开显示。
function M.last_load_status()
  return clone(last_load)
end

-- 已注册 provider 名单（只读，按注册顺序）：诊断面板与验收用。
function M.provider_names()
  local names = {}
  for index, name in ipairs(registration_order) do
    names[index] = name
  end
  return names
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
