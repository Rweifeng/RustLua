-- 工程入口：把引擎的工程级回调原样转交给路由（`scripts/router.lua`）。
--
-- 入口只做转发，演示逻辑全部在 `scripts/demo_*.lua` 里，按场景分模块。
-- 回调契约（引擎侧）：`on_create()` / `update()` / `on_destroy()` /
-- `on_click(name)` / `on_drag(from_id, from_index, to_id, to_index)` /
-- `on_day_start()` / `on_day_end()`。模块没实现的回调由路由跳过。

local route = require("router")

function on_create()
    route.on_create()
end

function update()
    return route.update()
end

function on_destroy()
    route.on_destroy()
end

function on_click(name)
    return route.on_click(name)
end

function on_drag(from_id, from_index, to_id, to_index)
    return route.on_drag(from_id, from_index, to_id, to_index)
end

function on_day_start()
    return route.on_day_start()
end

function on_day_end()
    return route.on_day_end()
end

--- 自检契约：新门 `sample_project_demos_self_test` 逐场景调用它。
---
--- 返回 `"ok steps=<N> checks=<M> failed=0"` 或 `"FAIL <scene>: <step>: <message>; ..."`。
--- 由 `router` 转给当前演示模块的 `self_test()`；fixture 场景返回 ok 0/0。
function __api_demo_self_test()
    return route.self_test()
end
