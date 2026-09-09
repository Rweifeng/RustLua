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

-- 查询位置（farm.act / 面向格计算用）
function M.position()
  local pid = M.ensure()
  if not pid then
    return nil
  end
  return transform.get(pid)
end

return M
