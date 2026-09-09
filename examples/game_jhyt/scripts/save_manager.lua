-- save_manager.lua：阶段 B 存档注册中心（B-2）。
-- 聚合各模块注册的快照提供器：保存时收集成一张表写入 blob，读档后按注册表整体恢复。
-- 引擎侧存档是严格 save_version=1 版本门（on_migrate 迁移钩子已被引擎移除），
-- 因此快照自带 version 字段，版本迁移由本模块在 restore 前处理。
local M = {}

local SAVE_KEY = "jianghu_youtian.state"
local SNAPSHOT_VERSION = 1

local providers = {}

-- register(name, capture, restore)：capture() 返回可 JSON 序列化的快照，
-- restore(data) 在读档后接收该快照；data 为 nil/不合法时 restore 不被调用。
function M.register(name, capture, restore)
  providers[name] = { capture = capture, restore = restore }
end

function M.capture()
  local snapshot = { version = SNAPSHOT_VERSION }
  for name, provider in pairs(providers) do
    snapshot[name] = provider.capture()
  end
  return snapshot
end

function M.restore(snapshot)
  if type(snapshot) ~= "table" then
    return false, "snapshot is not a table"
  end
  local version = tonumber(snapshot.version) or 0
  if version ~= SNAPSHOT_VERSION then
    return false, "unsupported snapshot version: " .. tostring(version)
  end
  for name, provider in pairs(providers) do
    if snapshot[name] ~= nil and provider.restore then
      provider.restore(snapshot[name])
    end
  end
  return true
end

-- 落盘：blob 只写内存，持久化必须显式 save.commit()（原子写）。
function M.save()
  save.blob_write(SAVE_KEY, M.capture())
  local ok = save.commit()
  if not ok then
    log.warn("[save] commit 失败或未配置存档路径")
  end
  return ok
end

function M.load()
  if not save.load() then
    return false
  end
  local snapshot = save.blob_read(SAVE_KEY)
  local ok, reason = M.restore(snapshot)
  if not ok then
    log.warn("[save] " .. tostring(reason))
  end
  return ok
end

return M
