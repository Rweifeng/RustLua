local state = require("production_state")

local M = {}

function M.find_all()
    state.coins = state.find_all(state.coin_prefix)
    state.coin_total = #state.coins
    return state.coins
end

function M.pick(coin_id)
    if coin_id and entity.is_alive(coin_id) then
        entity.destroy(coin_id)
        state.score = state.score + 1
        audio.play("pickup")
        log.info("coin collected " .. state.score .. "/" .. state.coin_total)
    end
end

return M
