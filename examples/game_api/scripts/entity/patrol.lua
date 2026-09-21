-- 每实体脚本：`Script  file=scripts/entity/patrol.lua` 的演示（docs/54 §4.1、docs/18「每实体脚本的钩子」）。
--
-- 这个演示想证明什么：
--   ① **脚本归属从工程级下放到实体**：巡逻不再是 `main.lua` 里的一个分派表，而是实体自己声明的
--      组件。场景里 `PatrolA` / `PatrolB` / `PatrolC` 三个实体写的是**同一个文件**。
--   ② **一个文件一份 Lua 状态、实体是实例**：顶层局部变量（`PATROL_MIN_X`/`SPEED`/`WALKERS`）是
--      按**文件**共享的，三只实体共用这一份；因此每实体状态必须写进下面这张以 `id` 为键的表里。
--      上位机的一万个实体是"一万个实例 + 一份 Lua 状态"，不是一万份 Lua 状态。
--   ③ 钩子名与工程级脚本**不重名**：这里只实现 `on_spawn(id)` / `on_update(id, dt)`；工程级仍然是
--      `on_create()` / `update()` / `on_destroy()`（写在 `scripts/demo_script.lua` 里）。两组名字
--      不重叠，所以同一个文件被两边加载也不会互相踩。
--
-- 看什么（运行 `scenes/api_script.scene`）：
--   * 三只方块各自左右往返，方向在 `[PATROL_MIN_X, PATROL_MAX_X]` 之间翻转，**相位互不相同**——
--     位置由这个文件算出来，不是工程级脚本摆出来的。
--   * 每只实体的头顶文字 `pt dir=.. x=.. t=..` 由 `world_text.attach` 每步刷新：
--     `t` 三者独立递增——同一份代码、三份实例状态，这就是"实体是实例"的可见证据。
--     attach 的组件串必须**逐字**合法（`text=..;font_size=..;color=..;offset=..;centered=true`）：
--     写引擎不认识的字段名会在这一步报 `malformed WorldText field`，不会静默丢弃。
--   * `t=` 是该实体的 `on_update` 累计调用次数；把 `engine/examples/lua_hot_reload` 那类重载流程
--     跑一遍会看到状态被重建（热重载清空该文件的 Lua 状态），已生成过的实体不会重复 `on_spawn`。
--
-- 边界（写在注释里，避免下一个人踩）：
--   * 这个文件的 `on_update` 每步对每只实体各调一次，**必须自己判活**：实体被 `entity.destroy`
--     之后钩子不会再被调用（脚本请求是按世界里的 `Script` 组件派发的），但重建/热重载期间
--     `WALKERS[id]` 可能是空的——所以下面用了兜底初始化，而不是相信"on_spawn 一定跑过"。
--   * 报错不静默：钩子里的 Lua 错误会带上脚本路径与钩子名中止当步（docs/18）。因此这里不写
--     任何可能失败的无保护调用。

-- 工程根内的相对路径由场景组件串给出（绝对路径与含 `..` 的路径在解析期就报错）；
-- 这里的常量全是纯数字，顶层不调用任何引擎 API。
local PATROL_MIN_X = 420.0 -- 巡逻左端（世界坐标，2D 相机的可见范围之内）
local PATROL_MAX_X = 860.0 -- 巡逻右端
local SPEED = 96.0 -- 像素/秒
local HUD_OFFSET = "0,-32" -- 头顶文字相对实体原点的偏移

-- **按文件共享**的每实体状态表：键是引擎给的实体 id（与 transform/quad/physics 收的是同一套编码）。
local WALKERS = {}

local function fmt(value)
    return string.format("%.1f", value)
end

--- 实体（或它所属的场景）生成时调用一次。
--- 注意 `WALKERS` 是**共享**的：这里写入的每一项各自属于一个实体实例。
function on_spawn(id)
    WALKERS[id] = {
        dir = 1,
        x = 0.0,
        ticks = 0,
    }
end

--- 每个固定步一次（在工程级 `update()` **之后**，按实体 id 升序）。
--- `id` 就是"我自己"：可以直接喂给 `transform.*` / `world_text.*`。
function on_update(id, dt)
    local self_state = WALKERS[id]
    if self_state == nil then
        -- 热重载清空了本文件的 Lua 状态，或钩子被以一个还没 on_spawn 过的 id 调到时，重建而不是崩。
        self_state = { dir = 1, x = 0.0, ticks = 0 }
        WALKERS[id] = self_state
    end

    -- ① 用 transform.get 读回"我现在在哪"。`space == "2d"` 说明实体挂的是 2D 变换。
    local position = transform.get(id)
    local x = position.x
    local y = position.y

    -- ② 推进这一步的巡逻。相位差异来自场景里三只实体**不同的初始 x**，不需要在文件里写特例。
    x = x + self_state.dir * SPEED * dt
    if x >= PATROL_MAX_X then
        x = PATROL_MAX_X
        self_state.dir = -1
    elseif x <= PATROL_MIN_X then
        x = PATROL_MIN_X
        self_state.dir = 1
    end
    transform.set(id, x, y)
    self_state.x = x
    self_state.ticks = self_state.ticks + 1

    -- ③ 报告"本帧的事实"：把每个实例的状态画在它自己头上。`world_text.attach` 收的是与场景
    --    组件串**同一套** `WorldText` 语法（`键=值;键=值`，docs/18），因此这里的调用与在场景里
    --    写一条 `WorldText` 组件等价——字段名只能是 text / font_size / color / font / offset /
    --    centered，写引擎不认识的字段名会**报错**而不是被静默丢掉。
    world_text.attach(
        id,
        string.format(
            "text=pt dir=%d x=%s t=%d;font_size=12;color=1.000,0.900,0.400;offset=%s;centered=true",
            self_state.dir,
            fmt(x),
            self_state.ticks,
            HUD_OFFSET
        )
    )
end
