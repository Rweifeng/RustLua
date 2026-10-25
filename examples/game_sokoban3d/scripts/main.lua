-- 工程入口：引擎的工程级回调原样转交给 `scripts/sokoban.lua`。
local game = require("sokoban")

function on_create()
    game.on_create()
end

function update()
    return game.update()
end

function on_destroy()
    game.on_destroy()
end
