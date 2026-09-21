-- api_script：演示"每实体 `Script` 组件 + 实体生命周期 API"这一组。
--
-- 这个演示想证明什么（分成两半，一半在场景里、一半在这里）：
--   A. **脚本归属下放到实体**：`scenes/api_script.scene` 里 `PatrolA` / `PatrolB` / `PatrolC`
--      三个实体各写了一条 `Script  file=scripts/entity/patrol.lua` —— **同一个文件、三个实体**。
--      它们的左右往返完全由 `scripts/entity/patrol.lua` 的 `on_spawn(id)` / `on_update(id, dt)`
--      驱动，本文件一行都没碰它们的位置；而且那个文件的顶层表是**按文件共享**的，每实体状态
--      写在以 `id` 为键的表里（"一个文件一份 Lua 状态、实体是实例"，docs/54 §4.1）。
--   B. **实体生命周期 API**：`entity.spawn` / `entity.find` / `entity.find_id` /
--      `entity.persist_id` / `entity.is_alive` / `entity.destroy` 六个函数各被真的调用一次，
--      并且串成一条能看懂的链路：生成 → 按名字查 → 按稳定 ID 反查 → 读回注册的稳定 ID →
--      判活 → 销毁 → 再判活（false）。
--
-- 看什么（按 `S` 生成并销毁一次之后）：
--   * 状态行里 `patrol{A,B,C}` 的 x 各不相同且都在变 —— 位置来自每实体脚本，不是这里算的。
--   * `last=` 是本次 `entity.spawn("Summoned", "demo.summoned")` 返回的实体 id。
--   * `find("Summoned")=` 是 `entity.find` 按名字查回来的 id，应与 `last=` 相同。
--   * `find_id("demo.summoned")=` 是 `entity.find_id` 按**稳定 ID** 反查回来的 id，也应相同 ——
--     这条链路的意义是：名字会重名、id 会被销毁后换代，而稳定 ID 是存档/跨场景能拿来定位的键。
--   * `readback=` 是 `entity.persist_id(id)` 从实体反查出的稳定 ID（等于 `demo.summoned`）。
--   * `alive=true → destroy → alive=false` 是 `entity.is_alive` / `entity.destroy` 的结果。
--   * 三只巡逻实体头顶的 `pt:id=.. x=.. t=..`：`t` 是各自 `on_update` 的累计调用次数，
--     三者独立递增 —— 同一份脚本代码、三份实例状态。
--
-- 硬规则（README §2）：顶层只定义函数与常量；`update()` 在 headless 下跑通（无按键时只读不写）；
-- 破坏性的 `entity.destroy` 只在按键分支里调用，且调用前先 `entity.is_alive` 判活——销毁后再对
-- 旧 id 调用大多数 API 会报 Lua 错误（docs/18「安全与错误策略」），所以本文件只在判活为 true
-- 时才继续调用其余接口。

local api = require("api_demo")

local M = {}

-- 三个巡逻实体的**名字**（场景里的实体名）。它们的 id 由 `entity.find` 在运行期查回来，
-- 因此这里不写死任何一个数字 id。
local PATROL_NAMES = { "PatrolA", "PatrolB", "PatrolC" }

-- 演示自己用的记账（模块顶层局部变量，不是引擎状态）。
local patrol_ids = {} -- 名字 → 实体 id（`entity.find` 的结果，可能为 nil）
local alive_patrols = 0 -- 本帧 `entity.is_alive` 判活的巡逻实体数
local calls_in_frame = 0 -- 本帧对巡逻实体做的 API 探测次数（"每实体脚本被驱动的旁证"）
local frame = 0 -- 本帧序号
local last_spawned = nil -- 最近一次 `entity.spawn` 返回的 id
local last_found = nil -- 最近一次 `entity.find("Summoned")` 的结果
local last_stable = nil -- 最近一次 `entity.find_id("demo.summoned")` 的结果
local last_readback = nil -- 最近一次 `entity.persist_id(id)` 读回的稳定 ID
local last_alive_before = nil -- 销毁之前 `entity.is_alive(id)` 的判定
local last_alive_after = nil -- 销毁之后 `entity.is_alive(id)` 的判定
local last_event = "no summon yet" -- 状态行里最值得看的那句话
local last_click = "no click yet" -- `on_click` 回调收到过什么

local function fmt(value)
    return string.format("%.1f", value)
end

local function text_or(value, fallback)
    if value == nil then
        return fallback
    end
    return tostring(value)
end

--- 只在进入本场景时调用一次。这里只做"查实体"，不做生成/销毁——`update()` 每帧都会跑，
--- 把破坏性操作放进来会让 headless 步进门看到完全不同的世界。
function M.on_create()
    api.title("api_script - per-entity Script hooks / entity lifecycle")
    api.hint("S summon+find+find_id+persist_id+is_alive+destroy - ESC / pad B returns to the menu")

    -- `entity.find` 按**实体名**查 id：三只巡逻实体不写死数字 id，名字是场景里的唯一真源。
    for _, name in ipairs(PATROL_NAMES) do
        patrol_ids[name] = entity.find(name)
    end
end

--- 每个固定步一次：读出"本帧的事实"，只有在按键时光改变世界。
function M.update()
    frame = frame + 1
    calls_in_frame = 0
    alive_patrols = 0

    -- ① 探测三只巡逻实体的存活与位置。先判活再读位置：这是"每实体脚本还在驱动谁"的可见证据。
    local report = {}
    for _, name in ipairs(PATROL_NAMES) do
        local id = patrol_ids[name]
        calls_in_frame = calls_in_frame + 1
        -- `entity.is_alive` 对未知/已销毁 id 返回 false（不报错），因此它是这条链上的第一道判活。
        if id ~= nil and entity.is_alive(id) then
            alive_patrols = alive_patrols + 1
            local position = transform.get(id)
            report[#report + 1] = string.format("%s=%s", name:sub(-1), fmt(position.x))
        else
            report[#report + 1] = string.format("%s=gone", name:sub(-1))
        end
    end

    -- ② 按键演示实体生命周期：生成 → 查名字 → 查稳定 ID → 读回稳定 ID → 判活 → 销毁 → 再判活。
    --    破坏性操作只在这里发生（headless 步进门没有输入，因此这一步在测试里永不触发）。
    if input.just_pressed("s") then
        -- 上一轮的召唤物还活着就先清掉：`entity.spawn` 对"仍被活实体占用的名字/稳定 ID"会报错
        -- （`persist_id already in use`），重复按 S 才不会第二次就炸。
        if last_spawned ~= nil and entity.is_alive(last_spawned) then
            entity.destroy(last_spawned)
        end

        -- 生成：第一个参数是名字（`entity.find` 查它），第二个是**稳定 ID**（`entity.find_id` 查它）。
        local id = entity.spawn("Summoned", "demo.summoned")
        last_spawned = id
        last_found = entity.find("Summoned")
        last_stable = entity.find_id("demo.summoned")

        -- 判活为 true 才继续问其余接口：销毁之后对旧 id 调用它们会报 Lua 错误。
        last_alive_before = entity.is_alive(id)
        if last_alive_before then
            -- `entity.persist_id(id)` 是反方向的查询：从实体读回它注册过的稳定 ID。
            last_readback = entity.persist_id(id)
        else
            last_readback = nil
        end

        entity.destroy(id)
        last_alive_after = entity.is_alive(id)
        last_event = string.format(
            "spawn id=%s find=%s find_id=%s readback=%s alive %s->%s",
            text_or(last_spawned, "nil"),
            text_or(last_found, "nil"),
            text_or(last_stable, "nil"),
            text_or(last_readback, "nil"),
            text_or(last_alive_before, "nil"),
            text_or(last_alive_after, "nil")
        )
        log.info("api_script " .. last_event)
    end

    -- ③ 本帧的事实。注意这里读的是**每实体脚本刚写过**的位置：工程级 `update()` 在每实体
    --    `on_update` 之前还是之后不会被本文件假设，状态行只报"读到什么"。
    local live_spawn = "none"
    if last_spawned ~= nil then
        live_spawn = string.format(
            "id=%s alive=%s",
            tostring(last_spawned),
            tostring(entity.is_alive(last_spawned))
        )
    end

    api.status(
        string.format(
            "f%d patrols(%s) alive=%d probes=%d | spawn %s | find_id=%s readback=%s | %s | click=%s",
            frame,
            table.concat(report, " "),
            alive_patrols,
            calls_in_frame,
            live_spawn,
            text_or(last_stable, "-"),
            text_or(last_readback, "-"),
            last_event,
            last_click
        )
    )
    return string.format(
        "api_script_ok patrols_alive=%d last_spawn=%s",
        alive_patrols,
        text_or(last_spawned, "none")
    )
end

--- 场景 `UiEvents` 的点击回调（实体名就是参数）。这里顺带演示"点击到的实体确实能被 API 找到"：
--- 只有 `entity.find` 查得到、且 `entity.is_alive` 判活的那个名字才会被读位置。
function M.on_click(name)
    last_click = tostring(name)
    -- 点击到的是巡逻实体时，用它的 id 直接问一次位置（"名字 → id → 组件"是这个演示的主线）。
    local id = entity.find(name)
    if id ~= nil and entity.is_alive(id) then
        local position = transform.get(id)
        last_click = string.format("%s@%s", tostring(name), fmt(position.x))
        return true
    end
    return false
end

--- 离开场景时释放本模块的动态 UI（标题/状态行/提示行由 `api_demo` 统一登记）。
function M.on_destroy()
    api.clear()
end

return M
