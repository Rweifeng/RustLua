local state = require("state")

local M = {}

function M.ensure()
  if state.player and entity.is_alive(state.player) then
    return state.player
  end
  state.player = entity.spawn("JianghuPlayer")
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
