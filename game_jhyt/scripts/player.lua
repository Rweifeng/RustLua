local state = require("state")

local M = {}

local PERSIST_ID = "player"

function M.ensure()
  if state.player and entity.is_alive(state.player) then
    return state.player
  end
  -- B-1 PersistId：先按稳定 ID 恢复，不存在才创建。
  state.player = entity.find_id(PERSIST_ID)
  if not state.player then
    state.player = entity.spawn("JianghuPlayer", PERSIST_ID)
  end
  transform.set(state.player, 480, 260)
  health.set(state.player, 100)
  return state.player
end

function M.move(dx, dy, direction)
  transform.move(M.ensure(), dx, dy)
  state.facing = direction
  state.last_action = "move"
end

return M
