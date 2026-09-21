-- demo_fixture：`demo3d.scene` 与 `mixed.scene` 是**渲染 golden 的固定夹具**，不是演示模块。
--
-- 为什么它们还留在这个工程里：`engine/examples/wp0_3d_golden.rs` 逐项断言 `demo3d.scene`
-- 的 3 个 drawable（Ground / ShadowTriangle / SkeletonWarrior）与相机位姿，
-- `wp0_mixed_golden.rs` 逐像素断言 `mixed.scene` 里"网格与精灵在同一深度缓冲上互相遮挡"。
-- 这两条 golden 的价值恰恰在于**内容不变**，因此它们按原样保留（见
-- `examples/game_api/README.md` 与 `docs/59`）。这个模块只负责把"它是什么"显示出来，
-- 顺便演示最小的一帧脚本形态。
local api = require("api_demo")

local M = {}

function M.on_create()
    local current = scene.reload() or "?"
    api.title("fixture - " .. current)
    api.hint("This scene is a pinned rendering fixture, not an API demo - " .. api.KEYS_BACK)
    log.info("fixture scene loaded: " .. current)
end

function M.update()
    api.status(
        string.format(
            "scene=%s - content is pinned by the wp0 rendering goldens; see docs/59 for the mapping",
            scene.reload() or "?"
        )
    )
    return string.format("fixture_ok scene=%s", scene.reload() or "?")
end

return M
