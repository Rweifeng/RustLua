-- 游戏主入口：只负责加载职责模块并转发生命周期回调。
-- 玩法逻辑按职责拆分到同目录各模块：
--   game.lua          流程编排（场景初始化、每帧更新、事件分发、胜负）
--   state.lua         共享状态与工具函数
--   player.lua        玩家（移动/攻击/血量/动画）
--   enemies.lua       史莱姆（收集/追击）
--   coins.lua         金币（收集/拾取）
--   save_manager.lua  存档（自动恢复/存/读）
--   hud.lua           HUD（血条/目标文本）
--   demo3d.lua        M8-06 3D 集成演示（移动网格/切换相机，防御式）
local game = require("game")
local demo3d = require("demo3d")

log.info("project script loaded")

function on_create()
    game.on_create()
    demo3d.on_create()
end

function update()
    local result = game.update()
    demo3d.update()
    return result
end
