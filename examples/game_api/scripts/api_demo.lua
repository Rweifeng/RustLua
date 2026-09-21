-- 演示模块共用的小工具：统一的标题/状态行、按键提示，以及"宿主帧依赖调用"的安全包装。
--
-- 为什么需要 `api.try`：`camera2d.get/screen_to_world/world_to_screen/mouse_world` 与
-- `camera3d.ray/mouse_ray/world_to_screen/get/pick_plane` 读的是**宿主每帧回填**的相机快照；
-- 没有渲染帧时（引擎 headless 测试、编辑器打开场景但没进 Play）直接调用会明确报错——
-- 这是引擎刻意的设计（"不拿一台默认相机去猜"）。演示因此把这些调用包一层，并把失败原因
-- 打进取景框里的状态行，让"宿主还没提供快照"这件事看得见，而不是静默吞掉。
local M = {}

local status_text = ""
local hint_text = ""
local title_text = ""

--- 受控调用：宿主帧依赖 / 允许失败的接口用它，返回 `值, 错误`。
function M.try(fn, ...)
    local ok, value, extra = pcall(fn, ...)
    if ok then
        return value, extra
    end
    return nil, tostring(value)
end

--- 场景标题（左上角）。
function M.title(text)
    title_text = text
end

--- 本帧状态行（左下角第一行）。
function M.status(text)
    status_text = tostring(text)
end

--- 按键提示行（左下角第二行）。
function M.hint(text)
    hint_text = tostring(text)
end

--- 由路由在模块 `update()` **之后**调用：本帧顶点在脚本返回后才被取走，
--- 因此"先改状态、再画"才是看得见的顺序。
---
--- 版式：状态行**贴着按键提示上方**往上排（两段都按行数算高度，因此多行状态不会压住提示），
--- 整块贴屏幕底部。演示自己画的说明行请放在 y ≤ 600，把 600..720 留给共享取景框。
function M.render()
    local LINE = 18
    local BOTTOM = 706
    local function line_count(text)
        local count = 1
        for _ in text:gmatch("\n") do
            count = count + 1
        end
        return count
    end

    if title_text ~= "" then
        ui.text_at("api_title", 24, 16, title_text, 1.5)
    end
    local hint_y = math.max(0, BOTTOM - LINE * line_count(hint_text))
    local status_y = math.max(0, hint_y - LINE * line_count(status_text))
    ui.text_at("api_status", 24, status_y, status_text, 1.1)
    ui.text_at("api_hint", 24, hint_y, hint_text, 1.0)
end

--- 清理本帧动态 UI（模块 `on_destroy` 用；`ui.clear` 同时删面板/图片/网格）。
function M.clear()
    ui.clear("api_title")
    ui.clear("api_status")
    ui.clear("api_hint")
end

--- 通用按键提示文案。
M.KEYS_BACK = "ESC / pad B returns to the menu"

return M
