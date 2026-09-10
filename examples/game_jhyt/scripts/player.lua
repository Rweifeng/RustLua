local state = require("state")

local M = {}

-- 场景原生玩家实体（JianghuPlayer）：由 main.scene 用
-- Transform + Sprite + Collider(ball;radius=12) 声明，引擎实例化时自动生成
-- 动态刚体。跨场景 persistent=true，同名实体会在每次进入山庄时被场景本体
-- 覆盖重建（引擎 process_scene_transition 语义），因此这里始终用 entity.find
-- 重新定位，不缓存运行时 id。
function M.ensure()
  local pid = entity.find("JianghuPlayer")
  if not pid then
    return nil
  end
  state.player = pid
  return pid
end

-- 直接更新刚体速度（世界单位/秒）：引擎每帧 step_physics 推进位移，
-- 与 GroundMap solid 瓦片碰撞；0 表示停住。
function M.set_velocity(dx, dy)
  local pid = M.ensure()
  if not pid then
    return
  end
  physics.set_velocity(pid, dx, dy)
end

-- 停止移动（松开方向键时调用，避免惯性滑行）
function M.stop()
  local pid = M.ensure()
  if pid then
    physics.set_velocity(pid, 0, 0)
  end
end

-- 传送：把动态刚体与 ECS Transform 同步移到世界坐标 (x,y) 并清零速度。
-- 切图落点 / 读档恢复必须用它（transform.set 只改 ECS，下一帧 step_physics
-- 会把刚体旧位置写回覆盖）；physics.set_position 为 M17-01 新增的 2D 传送。
function M.teleport(x, y)
  local pid = M.ensure()
  if not pid then
    return false
  end
  if type(x) ~= "number" or type(y) ~= "number" then
    return false
  end
  physics.set_position(pid, x, y)
  physics.set_velocity(pid, 0, 0)
  return true
end

-- 查询位置（farm.act / 面向格计算用）
function M.position()
  local pid = M.ensure()
  if not pid then
    return nil
  end
  return transform.get(pid)
end

-- 位置快照使用场景中的稳定实体名 JianghuPlayer 重新绑定；引擎当前未暴露
-- PersistId 查询/设置 API，因此这是最小安全坐标快照：记录所在地图 id 与
-- 世界坐标，跨地图读档/继续时可定位目标场景。
function M.capture()
  local pos = M.position()
  if not pos then
    return { mode = "unavailable", entity = "JianghuPlayer" }
  end
  return {
    mode = "entity_name_position",
    entity = "JianghuPlayer",
    map = (map.current() or scene.reload()),
    x = tonumber(pos.x),
    y = tonumber(pos.y)
  }
end

function M.restore(data)
  if type(data) ~= "table" or data.mode ~= "entity_name_position" then
    return false, "player position snapshot unavailable"
  end
  local pid = M.ensure()
  if not pid then
    return false, "JianghuPlayer is not instantiated"
  end
  if type(data.x) ~= "number" or type(data.y) ~= "number" then
    return false, "player position is invalid"
  end
  -- 读档恢复发生在场景重建之后：用 2D 传送保证刚体与 ECS 同步，
  -- 避免 transform.set 被下一帧 physics 回写覆盖导致"回到旧位置"。
  M.teleport(data.x, data.y)
  return true
end

return M
