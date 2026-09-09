-- 金币职责：收集金币列表、拾取与计数。
local state = require("state")

local M = {}

function M.find_all()
    state.coins = state.find_all("Coin")
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
