-- map_manager.lua：多地图生命周期与出口传送（M17-01）。
--
-- 引擎事实（core/script.rs）：
--   * map.goto(map, x?, y?) 校验 scenes/<map>.scene 存在后，把 map 记入
--     save_data["__map"]，可选的 x,y 记入 "__map:spawn:x/y"（世界坐标），
--     并排队 scene_pending_load；真正切图在 process_scene_transition（每帧
--     physics 后应用），随后全局 on_create 在新场景上重跑。
--   * scene 实例化时 JianghuPlayer 的 Transform 来自 scene 文件（每图各自
--     的出生格）；__map:spawn 只在 Lua 侧可读（map.spawn()），引擎不会自动
--     落位 —— 因此切图落点必须由 on_create 内读 map.spawn() 并 teleport。
--   * 2D 物理无重力；玩家是动态刚体。physics.set_position(id,x,y)（M17-01
--     新增）同步刚体与 ECS Transform，供切图落点/读档恢复使用。
--
-- 模块职责：
--   * 统一入口 on_enter(map)：把玩家放到本图出生点（读 __map:spawn 优先，
--     其次 data/maps 注册表），并把该图注册为"当前图"（出口判定依赖）。
--   * 出口交互 target 经 interaction.register 注册，query 用玩家所在 cell
--     与出口格比对（走到格子上按 E 触发），interact 调 map.goto 切换。
local state = require("core.state")
local player = require("player.player")
local interaction = require("systems.interaction")
local maps = require("data.maps")
-- M34-02 冬至封山：出口是否放行读今日节气的效果表（只读查询；封哪些方向在数据里）。
local festival = require("world.festival")

local M = {}
local current_map = nil -- 最近一次 on_enter 的地图 id（门面/未进入时为 nil）
local exits_registered_for = nil -- 已注册出口的地图 id（模块级缓存）

-- 玩家所在 cell（0-index，y 朝上；与 tilemap/farm 同约定）
function M.player_cell()
  local pid = player.ensure()
  if not pid then
    return nil
  end
  local pos = transform.get(pid)
  if not pos then
    return nil
  end
  local cell = grid.world_to_cell(pos.x, pos.y, maps.TILE)
  return cell[1], cell[2]
end

-- 玩家面前的 cell（与 farm/building 同一约定：`grid.facing_cell`）。
function M.facing_cell(facing)
  local pid = player.ensure()
  if not pid then
    return nil
  end
  local pos = transform.get(pid)
  if not pos then
    return nil
  end
  local cell = grid.facing_cell(pos.x, pos.y, facing or state.facing or "down", maps.TILE)
  return cell[1], cell[2]
end

-- 引擎当前激活地图：__map 未设置时退回 scene.reload()（门面无 __map）。
function M.current()
  return current_map or map.current() or scene.reload()
end

function M.set_current(map_id)
  current_map = map_id
end

-- 统一进入处理：落点 + 当前图标记 + 入口校验日志。
-- 由 main.lua 的 on_create 在每张玩法地图进入时调用（主入口门面不调用）。
function M.on_enter(map_id)
  local definition = maps.get(map_id)
  if not definition then
    log.warn("[map] 未注册的地图: " .. tostring(map_id))
    return false
  end
  M.set_current(map_id)
  -- 出生点：map.goto 携带的入口 spawn 优先；否则用地图注册表默认出生格。
  local wx, wy = nil, nil
  local spawn = map.spawn()
  if spawn then
    wx, wy = tonumber(spawn.x), tonumber(spawn.y)
  else
    wx, wy = maps.to_world({ to_cell = definition.spawn })
  end
  local ok, reason = M.place_player(wx, wy)
  if not ok then
    log.warn("[map] " .. map_id .. " 落点失败: " .. tostring(reason))
  end
  log.info("[map] 进入 " .. tostring(map_id) .. " @(" .. tostring(wx) .. "," .. tostring(wy) .. ")")
  return true
end

-- 把玩家刚体与 ECS Transform 一起放到世界坐标 (wx, wy)，并清零速度。
function M.place_player(wx, wy)
  if type(wx) ~= "number" or type(wy) ~= "number" then
    return false, "invalid spawn"
  end
  local pid = player.ensure()
  if not pid then
    return false, "JianghuPlayer is not instantiated"
  end
  player.teleport(wx, wy)
  return true
end

-- 出口 target（interaction 协议）：candidate 携带 id/label/exit。
-- priority=20：**必须高于所有"按距离命中"的玩法交互（NPC 对话/商店/设施，均为 10）**。
-- 出口 query 只看玩家站立格；而 dialogue/shop 的 query 是 56 单位（≈1.75 格）距离圈、
-- 不看玩家朝向——若出口优先级低，NPC 站在出口格旁时（如玄鹤 06:00-08:00 在山庄门廊
-- (16,2)，紧邻出口格 (16,1)），玩家站在出口格按 E 会被对话抢占，永远无法离庄
-- （实机复现：2026-10-03）。站在出口格就是要离开，出口必须无条件优先。
--
-- 触发格判定（两条，任一成立即"在门口"）：
--   ① 脚下就是出口格（maps.lua 的 `exit.cell`，如山庄 (16,1)）；
--   ② 脚下是出口格的**正前方一格且面朝出口**（如站在山庄门廊 (16,2) 朝下）。
-- 只认 ①（原实现）时，玩家最常见的站位——门洞外那一格——按 E 得到的不是出口，
-- 而是 farm 目标兜底的"不是药田"（实机 2026-10-03："走到传送点按 E 不能传送"；
-- 门洞只有一格宽，(16,1) 南北两侧分别是墙和门廊，玩家很难恰好停在那一格）。
-- ② 只在面朝出口时成立：站在门廊背对门洞按 E 仍然是"与门口那个人交谈"，
-- NPC 对话不会被出口永久吃掉（玄鹤 06:00-08:00 就站在 (16,2)）。
local function register_exit_target(exit)
  interaction.register({
    id = exit.id,
    priority = 20, -- 高于对话/商店/设施（10）与农田（0）：在门口按 E 必能离开
    query = function(facing)
      local cx, cy = M.player_cell()
      if not cx then
        return nil, "no_player"
      end
      if M.current() ~= exit.from then
        return nil, "wrong_map"
      end
      if cx == exit.cell.x and cy == exit.cell.y then
        return { id = exit.id, exit = exit, distance = 0.5, facing = 1 }
      end
      local fx, fy = M.facing_cell(facing)
      if fx == exit.cell.x and fy == exit.cell.y then
        return { id = exit.id, exit = exit, distance = 1.0, facing = 1 }
      end
      return nil, "not_at_exit"
    end,
    prompt = function(candidate)
      if candidate and candidate.exit then
        return "E: " .. candidate.exit.label
      end
      return "E: 出口"
    end,
    interact = function(candidate)
      local target = candidate and candidate.exit
      if not target then
        return "no_exit"
      end
      -- 冬至封山（M34-02）：**命中测试一行不动**（站在门口照样能按到 E），拒绝发生在要走之前。
      -- 于是"封"只作用在数据点名的方向（山道 → 寒泉洞），回村那一格永远可用——
      -- 这与 42a565c（门口按 E 必须能离庄）是同一处代码的两个约束：一个不许把出口判没，
      -- 一个不许把玩家关在山上。
      if festival.exit_closed(target.id) then
        state.last_action = "closed_festival"
        return state.last_action
      end
      if not maps.get(target.to) then
        log.warn("[map] 出口目标地图未注册: " .. tostring(target.to))
        state.last_action = "missing_map"
        return state.last_action
      end
      local wx, wy = maps.to_world(target)
      state.set_mode("Transition") -- 切图期间锁定输入（M17-01 风险项）
      state.last_action = "exit_" .. target.to
      local ok = map["goto"](target.to, wx, wy)
      if not ok then
        log.warn("[map] map.goto 失败: " .. tostring(target.to))
        state.set_mode("Playing")
        state.last_action = "map_goto_failed"
        return state.last_action
      end
      log.info("[map] 传送 " .. tostring(target.from) .. " -> " .. tostring(target.to) .. " @(" .. tostring(wx) .. "," .. tostring(wy) .. ")")
      return state.last_action
    end
  })
end

-- 注册某地图的出口（幂等：只注册一次；地图间 on_enter 切换不重复注册）。
function M.ensure_exits(map_id)
  if exits_registered_for == map_id then
    return
  end
  local definition = maps.get(map_id)
  if definition then
    for _, exit in ipairs(definition.exits or {}) do
      register_exit_target(exit)
    end
  end
  exits_registered_for = map_id
end

return M
