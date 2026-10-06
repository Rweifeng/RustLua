-- systems/endings.lua：《江湖有田》多结局判定与解锁记录（docs/54 §7.6，2026-10-06 2.4）。
--
-- 5 条结局，条件走"可查询谓词"闭集（data/endings.lua 的 predicate_types 白名单）。
-- 本模块是**查询面声明层**，不接管 quest 五态机、不改写既有 ending_*.lua 任务：
-- 只把"玩家是否达成某条结局的量化门槛"求值成 once 解锁记录。
--
-- 职责：
--   * unlocked 表：已解锁结局 id → true（once，随存档持久）；
--   * check_all()：遍历 5 条对谓词求值 → 新达成的记进 unlocked、写
--     last_action = "ending_<id>"，返回本次新达成的 id 列表（可能为空）；
--   * 谓词求值器：读 player.relationship.reputation / systems.mastery.level /
--     clock.now().day——**全部 pcall 容错**：依赖模块未落地时该谓词按 false 处理，
--     避免启动崩；
--   * capture/restore/reset（随 save_manager 持久；新档 reset 清空）；
--     validate_all 返回 "" = 通过。
--
-- hooks：main.lua on_day_start 调用 check_all（补丁文本）；验收样例显式调用做边界/幂等。
-- 结局演出 / 插图 / 立绘 / 结局后自由模式面板属阶段 4 美术与 UI 管线，本模块只做
-- 数据判定与解锁记录（docs/54 §7.6 的纯数据 + 查询面）。
local state = require("core.state")
local data = require("data.endings")

local M = {}

-- 运行时状态（capture/restore 随存档持久；reset 清空）
local unlocked = {} -- ending_id → true

-- ---------- 谓词求值器（全部 pcall 容错） ----------

-- 阵营声望（player.relationship.reputation(faction)，默认 0）。模块缺失 → false。
local function reputation_value(faction_id)
  local ok, rel = pcall(require, "player.relationship")
  if not ok or type(rel) ~= "table" or type(rel.reputation) ~= "function" then
    return nil
  end
  local ok2, value = pcall(rel.reputation, faction_id)
  if not ok2 or value == nil then
    return nil
  end
  return tonumber(value)
end

-- 谓词分派表：cond → boolean。每个求值器内部 pcall 容错。
local PREDICATES = {}

function PREDICATES.reputation(cond)
  local value = reputation_value(cond.faction)
  if value == nil then
    return false
  end
  return value >= (tonumber(cond.min) or math.huge)
end

function PREDICATES.mastery_level(cond)
  local ok, mastery = pcall(require, "systems.mastery")
  if not ok or type(mastery) ~= "table" or type(mastery.level) ~= "function" then
    return false
  end
  local ok2, level = pcall(mastery.level, cond.line)
  if not ok2 then
    return false
  end
  return (tonumber(level) or 0) >= (tonumber(cond.min) or math.huge)
end

function PREDICATES.day(_cond)
  local ok, now = pcall(function()
    return clock.now().day
  end)
  if not ok then
    return false
  end
  return (tonumber(now) or 1) >= (tonumber(_cond.min) or math.huge)
end

-- ---------- 公开接口 ----------

-- 遍历全部结局，对未解锁者求谓词；新达成的记进 unlocked、写 last_action。
-- 返回本次新达成的 id 列表（可能为空）。已解锁的不再重复求值/重复记录（幂等）。
function M.check_all()
  local newly = {}
  for _, entry in ipairs(data.endings) do
    if not unlocked[entry.id] then
      local fn = PREDICATES[entry.cond and entry.cond.type]
      local met = false
      if fn then
        local ok, result = pcall(fn, entry.cond)
        met = ok and result == true
      end
      if met then
        unlocked[entry.id] = true
        state.last_action = "ending_" .. entry.id
        newly[#newly + 1] = entry.id
      end
    end
  end
  return newly
end

-- 是否已达成某结局（只读）。
function M.is_unlocked(id)
  return unlocked[id] == true
end

-- 已解锁结局 id → true（只读视图）。
function M.unlocked()
  return unlocked
end

-- 已达成结局数量。
function M.unlocked_count()
  local n = 0
  for _ in pairs(unlocked) do
    n = n + 1
  end
  return n
end

-- 某结局的玩家可见名 / 描述（数据层纯声明，无隐藏结局）。
function M.name(id)
  local entry = data.get(id)
  return entry and entry.name or ""
end

function M.description(id)
  local entry = data.get(id)
  return entry and entry.description or ""
end

-- ---------- 存档 ----------

function M.capture()
  local unlocked_data = {}
  for id, flag in pairs(unlocked) do
    if type(id) == "string" and flag == true then
      unlocked_data[id] = true
    end
  end
  return { unlocked = unlocked_data }
end

function M.restore(data_in)
  unlocked = {}
  if type(data_in) ~= "table" then
    return
  end
  if type(data_in.unlocked) == "table" then
    for id, flag in pairs(data_in.unlocked) do
      -- 只恢复当前数据表仍登记的结局 id（删结局后旧档里的孤儿记录直接丢弃）。
      if type(id) == "string" and flag == true and data.get(id) then
        unlocked[id] = true
      end
    end
  end
end

function M.reset()
  unlocked = {}
end

-- ---------- 加载期校验 ----------

-- 纯系统模块：数据校验委托 data.endings.validate_all；本模块只自检
-- "谓词白名单与数据谓词类型一一对应"。返回 "" = 通过。
function M.validate_all()
  local report = {}
  local data_report = data.validate_all()
  if data_report ~= "" then
    report[#report + 1] = "endings: data 校验失败:\n" .. data_report
  end
  -- 谓词求值表必须覆盖白名单全部类型
  for type_name in pairs(data.predicate_types) do
    if type(PREDICATES[type_name]) ~= "function" then
      report[#report + 1] = "endings: 谓词 " .. type_name .. " 缺求值器"
    end
  end
  return table.concat(report, "\n")
end

return M
