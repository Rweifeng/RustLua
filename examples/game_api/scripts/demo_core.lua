-- api_core：把 `log` / `time` / `entity` / `event` / `tag` / `health` / `asset` /
-- `scene` / `save` / `data` 这十个"核心模块"的每一个函数都真的调用一次。
--
-- 这个演示想证明什么
-- ------------------
-- ① **状态行里的每个值都来自一次 API 读回**：`health.get` / `tag.get` / `save.read` /
--    `save.blob_read` / `data.load` / `asset.load` / `scene.reload` … 脚本自己不编造任何
--    "演示数字"。于是它同时证明了两件事：接口存在，而且读回来的东西是对的。
-- ② **每帧只做内存内的读写**。`save.write` / `save.blob_write` / `save.read` 在
--    `engine/crates/engine/src/core/script.rs` 里只碰 `ScriptState::save_data`（纯内存哈希表），
--    **不碰磁盘**；真正碰磁盘 / 切场景 / 退出请求的五个调用
--    （`save.commit` / `save.load` / `save.clear` / `scene.load` / `app.quit`）
--    **全部锁在按键分支里** —— 引擎的 headless 门会**无输入**地步进 `update()`，
--    它们一旦每帧执行，测试就会偷偷写盘、换场景、甚至直接退出。
-- ③ **半数 API 只有按键才看得见**（`log.warn` / `health.damage` / `event.off` /
--    `save.slot` / `app.quit` …），提示行逐条列出键位，按完键看状态行的 `key=` 段。
-- ④ **读回值一律取在按键之后**：`key=` 说"刚做了什么"，同一行后面的数字说"做完之后世界
--    是什么样"。两者必须自洽（先读后做会让它们差一帧，看起来像互相矛盾）。
--
-- 屏幕上能看到的
-- --------------
--   * 场景中间三只实体：`Hero`（Sprite + 圆形碰撞体）、`Dummy`（Sprite + 传感器碰撞体）、
--     `Pickup`（金币）。它们是 `entity.find` 的查询目标；本演示**不移动**它们
--     （移动、物理、动画各有别的演示模块）。
--   * 左上角标题、左下角状态行（= 本帧的事实）与提示行（= 按键）。
--   * 状态行里唯一"会变形状"的一段是 `ghost`：按 G 销毁、按 H 重生之后 `gen=` 会 +1。
--     引擎把实体 id 编码成 `index << 32 | generation`（`script.rs::entity_lua_id`），
--     销毁**不会**让旧 id 指向新实体 —— 换代靠的就是 generation 这一段。
--
-- 硬规则（README §2）：顶层只定义函数与常量；`update()` 在 headless 下跑通（无按键时只读）；
-- `entity.destroy` 这类破坏性调用只在按键分支里发生。

local api = require("api_demo")

local M = {}

-- ---------------------------------------------------------------------------
-- 常量：实体名 / 资源路径 / 事件名 / 键名
-- 模块顶层**只放常量与函数定义**（README §2 硬规则 1）：顶层任何一句引擎调用都会在
-- "只是加载这个模块"时执行，而那一刻实体还没生成、宿主也还没回填相机。
-- ---------------------------------------------------------------------------
local HERO_NAME = "Hero" -- 场景里 `entity.find` 的查询目标
local DUMMY_NAME = "Dummy"
local PICKUP_NAME = "Pickup"
local COIN_TEXTURE = "assets/textures/coin.png" -- `asset.load` 的入参（工程里真实存在）
local DATA_PATH = "data/cache_demo.json" -- `data.*` 的演示数据（与 `CACHE_SPEC` 匹配）
local EVENT_HIT = "api_core.hit" -- `event.*` 的事件名
local GHOST_NAME = "GhostProbe" -- 运行期生成的临时实体名
local GHOST_PERSIST = "demo.ghost" -- 它的**稳定 ID**（`entity.find_id` 查它）
local TAG_KEY = "mood" -- `tag.get` / `tag.set` 的键
local SAVE_KEY = "api_core.frame" -- `save.write` / `save.read` 的键（值必须是字符串）
local BLOB_KEY = "api_core.snapshot" -- `save.blob_write` / `save.blob_read` 的键（任意 Lua 表）
local SAVE_SLOT = "demo" -- `save.slot` 的槽名
local HERO_MAX_HP = 100
local HIT_DAMAGE = 7
-- 实体 id 的低 32 位是 generation（代际），高 32 位是 index。
local GENERATION_MOD = 4294967296 -- 2^32

local TITLE =
    "api_core - log / time / entity / event / tag / health / asset / scene / save / data"
-- 提示行（scale 1.0）在 1280px 下大约装得下 190 个字符：这里按"一组一个 `|`"排，长度约 145。
local HINT = "SPACE hit | R heal | T tag | 1/2/3 unsub(on/sub)/resub | G/H ghost -/+ | "
    .. "K bad-destroy | S commit | L load | C clear | V data-clear | F scene | Q quit | ESC menu"

-- `api_demo` 只提供三行（标题 / 状态行 / 提示行），而状态行（scale 1.1）在 1280px 下大约
-- 只装得下 170 个字符 —— 本演示要同时显示四十个 API 的读回值，一行装不完。因此在状态行
-- **上方**（y=610，不压住任何东西）自画一行"只放读回值"的私有行。
-- 离开场景时要自己 `ui.clear`：`api_demo.clear` 只清它自己登记的那三行。
local FACTS_ID = "api_core_facts"
local FACTS_Y = 610

-- ---------------------------------------------------------------------------
-- 演示自己的记账。都是普通 Lua 局部变量（模块状态），不是引擎状态。
-- ---------------------------------------------------------------------------
local frame = 0 -- 本帧序号（`time.delta` 之外的另一个"事实"）
local hits_by_on = 0 -- 被 `event.on` 订阅的那个回调收到的次数
local hits_by_subscribe = 0 -- 被 `event.subscribe` 订阅的那个回调收到的次数
local last_hit_payload = "-" -- 最近一次 `event.emit` 带过去的数据
local last_key = "-" -- 最近一次按键分支做了什么（状态行的 `key=` 段）
local ghost = nil -- `entity.spawn` / `entity.find_id` 拿到的临时实体 id
local hero_hp = 0 -- 本帧 `health.get(hero)` 的读回值
local hero_tag = nil -- 本帧 `tag.get(hero, TAG_KEY)` 的读回值
local save_readback = nil -- 本帧 `save.read(SAVE_KEY)` 的读回值
local save_slot_now = nil -- 本帧 `save.slot_get()` 的读回值
local blob_hp = nil -- 本帧 `save.blob_read(BLOB_KEY).hp` 的读回值
local asset_token = nil -- 本帧 `asset.load` 的返回值（字符串 `asset:<路径>`）
local data_counts = "0/0/0" -- 本帧三次 data 读取各自看到的条目数
local data_version = nil -- 本帧 `data.load` 读回的 version 字段
local scene_name = "?"

-- `event.on` 与 `event.subscribe` 是**同一总线的两个名字**：`script.rs` 里两个函数往同一张
-- `subscribers` 表里塞回调。这里故意用两个不同的回调分别订阅，于是状态行 `ev on=/sub=`
-- 两个计数器一起涨 —— "一次 emit 打到两个名字上"是**看得见**的，而不是文档里的承诺。
local function on_hit_by_on(data)
    hits_by_on = hits_by_on + 1
    if type(data) == "table" then
        -- 回调收到的就是 `event.emit` 传的那个表（引擎不做 JSON 往返）。
        last_hit_payload = string.format("amount=%s frame=%s", tostring(data.amount), tostring(data.frame))
    end
end

local function on_hit_by_subscribe()
    hits_by_subscribe = hits_by_subscribe + 1
end

-- `data.schema(path, spec)` 的校验器：一份"物品表"的形状声明。
-- 语法见 `engine/crates/engine/src/core/data_schema.rs`：
--   * 字符串 = 基本类型名（`string` / `number` / `integer` / `boolean` / `any`）；
--   * `{ "$kind" = "map", values = <schema> }` = 字符串键字典，逐值校验；
--   * `{ "$kind" = "object", fields = {...}, required = {...} }` = 字段表。
-- 判别字段刻意叫 `$kind` 而不是 `kind`：游戏数据里本来就常有真实的 `kind` 字段
--（`entries` 里每一项就是），叫 `kind` 会撞名。
local CACHE_SPEC = {
    ["$kind"] = "object",
    fields = {
        version = "integer",
        source = "string",
        entries = {
            ["$kind"] = "map",
            values = {
                ["$kind"] = "object",
                fields = {
                    name = "string",
                    value = "integer",
                    stack = "integer",
                },
                required = { "name", "value", "stack" },
            },
        },
    },
    required = { "version", "entries" },
}

--- nil -> "-"（状态行里"没有值"和"值是 nil"必须一眼分得开）。
local function dash(value)
    if value == nil then
        return "-"
    end
    return tostring(value)
end

--- 折成一行并截断：引擎的 Lua 错误信息**带多行栈回溯**，直接塞进状态行会把画面顶掉。
local function one_line(value, limit)
    local text = tostring(value):gsub("%s+", " ")
    if #text > limit then
        return text:sub(1, limit) .. "…"
    end
    return text
end

--- 数一张表里有多少个键（`data.load` 读回的是**字符串键字典**，`#` 对它没有意义）。
local function key_count(value)
    if type(value) ~= "table" then
        return 0
    end
    local count = 0
    for _ in pairs(value) do
        count = count + 1
    end
    return count
end

--- 只在进入本场景时调用一次：登记初始状态。
--- 这里**不做**每帧该做的事，也**不做**破坏性操作（销毁/写盘/切场景）。
function M.on_create()
    api.title(TITLE)
    api.hint(HINT)

    -- ① entity：生成一个**只活在数据里**的临时实体（没有 Sprite，因此不占画面）。
    --    它的存在意义是把 entity 的六个函数串成一条链：
    --    生成 → 按名字查 → 按稳定 ID 反查 → 读回稳定 ID → 判活 → 销毁 → 再判活。
    --    先 `find_id` 再 `spawn` 是为了**幂等**：热重载与 `scene.load` 都会在**同一个世界**
    --    上重跑 `on_create`，而 `entity.spawn` 对"仍被活实体占用着的稳定 ID"会明确报错
    --    `persist_id already in use` —— 幂等才不会一换场景就炸。
    ghost = entity.find_id(GHOST_PERSIST)
    if ghost == nil or not entity.is_alive(ghost) then
        ghost = entity.spawn(GHOST_NAME, GHOST_PERSIST)
    end

    -- ② health / tag：给 Hero 一份初始血量与一个初始标签，之后每帧读回。
    local hero = entity.find(HERO_NAME)
    if hero ~= nil then
        health.set(hero, HERO_MAX_HP)
        tag.set(hero, TAG_KEY, "calm")
    end

    -- ③ event：两个名字各订阅一次（同一总线，因此一次 emit 会打到两个回调）。
    event.on(EVENT_HIT, on_hit_by_on)
    event.subscribe(EVENT_HIT, on_hit_by_subscribe)

    -- ④ data.schema：注册校验器。注册本身就会让该路径的旧缓存失效
    --    （见 script.rs 的 `data.schema`），因此下一次 `data.load` 一定会按这份 spec 校验。
    data.schema(DATA_PATH, CACHE_SPEC)

    -- `scene.reload()` 返回的是**当前场景名**（不是"重新加载"）：它是宿主真源。
    scene_name = dash(scene.reload())
    log.info("api_core demo ready: scene=" .. scene_name .. " ghost=" .. dash(ghost))
end

--- 每个固定步一次。返回的字符串会进引擎日志（`run_script_steps` / 编辑器 Play 都能看到）。
---
--- 顺序刻意是「先按键、后读回」：按键分支里做动作，读回段统一在最后取值，
--- 这样状态行里 `key=` 描述的动作与同一行的数值永远指同一帧的世界。
function M.update()
    frame = frame + 1

    -- ① time.delta：宿主喂进来的步长（headless 下是测试传的 dt）。
    --    本演示不假设它是 1/60 —— 状态行直接把它显示出来。
    local dt = time.delta()

    -- ② entity.find：按名字查三只实体。查不到返回 nil（**不报错**），
    --    因此后面每个用到 id 的地方都必须先判 nil / 判活。
    local hero = entity.find(HERO_NAME)
    local dummy = entity.find(DUMMY_NAME)
    local pickup = entity.find(PICKUP_NAME)
    local found = 0
    if hero ~= nil then
        found = found + 1
    end
    if dummy ~= nil then
        found = found + 1
    end
    if pickup ~= nil then
        found = found + 1
    end

    -- -----------------------------------------------------------------------
    -- ③ 按键分支：破坏性的 / 碰磁盘的 / 换场景的调用只能出现在这里。
    --    headless 步进门没有按键，因此这些分支在测试里永不触发。
    -- -----------------------------------------------------------------------
    if input.just_pressed("space") then
        if hero ~= nil and entity.is_alive(hero) then
            -- `health.damage(id, n)` 返回扣血**之后**的血量，并把它钳在 0（不会变负）。
            local left = health.damage(hero, HIT_DAMAGE)
            local readback = health.get(hero) -- 紧接着读回：返回值与读回值必须一致
            last_key = string.format("SPACE damage(-%d) left=%.0f get=%.0f", HIT_DAMAGE, left, readback)
            -- `log.warn`：走引擎的 WARN 级日志（target: "script"），控制台里能看到。
            log.warn("api_core hero hit: " .. last_key)
            if readback <= 0.0 then
                -- 归零了就复位：`health.set` 是"直接写值"，与 damage 的"读改写"是两条路。
                health.set(hero, HERO_MAX_HP)
                last_key = last_key .. " -> refilled " .. tostring(HERO_MAX_HP)
                log.info("api_core hero refilled by health.set")
            end
        end
    end

    if input.just_pressed("r") then
        if hero ~= nil and entity.is_alive(hero) then
            health.set(hero, HERO_MAX_HP)
            last_key = string.format("R health.set=%d", HERO_MAX_HP)
        end
    end

    if input.just_pressed("t") then
        if hero ~= nil and entity.is_alive(hero) then
            -- `tag.set` 的**空字符串 = 删除该标签**（见 script.rs 的 tag.set）。
            -- 于是"打标签 / 删标签"是同一个函数的两半：状态行 `tag=` 在值 / `-` 之间切换。
            local current = tag.get(hero, TAG_KEY)
            local next_value = "calm"
            local action = "set"
            if current ~= nil then
                next_value = ""
                action = "remove"
            end
            tag.set(hero, TAG_KEY, next_value)
            last_key = string.format("T tag.%s(%s)", action, TAG_KEY)
        end
    end

    if input.just_pressed("1") then
        -- `event.off(name, callback)`：只摘掉这一个回调（不传 callback 才是清掉整个事件名）。
        event.off(EVENT_HIT, on_hit_by_on)
        last_key = "1 event.off(on)"
    end

    if input.just_pressed("2") then
        event.unsubscribe(EVENT_HIT, on_hit_by_subscribe)
        last_key = "2 event.unsubscribe(subscribe)"
    end

    if input.just_pressed("3") then
        event.on(EVENT_HIT, on_hit_by_on)
        event.subscribe(EVENT_HIT, on_hit_by_subscribe)
        last_key = "3 resubscribed both names"
    end

    if input.just_pressed("g") then
        if ghost ~= nil and entity.is_alive(ghost) then
            -- `entity.destroy` 返回 true；它同时注销名字、稳定 ID、血量与物理刚体。
            entity.destroy(ghost)
            last_key = string.format(
                "G destroy id=%s alive=%s",
                dash(ghost),
                tostring(entity.is_alive(ghost))
            )
            log.info("api_core ghost destroyed: " .. last_key)
        else
            last_key = "G ghost already dead"
        end
    end

    if input.just_pressed("h") then
        if ghost ~= nil and entity.is_alive(ghost) then
            last_key = "H ghost still alive (press G first)"
        else
            -- 重生拿到的是**新一代**的 id：index 段通常被复用，generation 段 +1。
            local previous_gen = ghost ~= nil and ghost % GENERATION_MOD or nil
            ghost = entity.spawn(GHOST_NAME, GHOST_PERSIST)
            last_key = string.format(
                "H respawn gen %s->%s id=%s",
                dash(previous_gen),
                dash(ghost % GENERATION_MOD),
                dash(ghost)
            )
        end
    end

    if input.just_pressed("k") then
        -- 想让人看到**错误契约**：对已销毁的 id 再调用 `entity.destroy`，
        -- 引擎明确报 Lua 错误（`require_alive`），而不是静默返回 false。
        if ghost ~= nil and entity.is_alive(ghost) then
            entity.destroy(ghost) -- 先把它变成"已销毁的 id"
        end
        if ghost ~= nil then
            local ok, err = pcall(function()
                entity.destroy(ghost)
            end)
            if ok then
                last_key = "K unexpected: second destroy succeeded"
                log.warn("api_core " .. last_key)
            else
                -- `log.error` 的用法：把"被预期拒绝的调用"的原因打进日志（**原文**，含栈回溯）。
                log.error("api_core expected failure: " .. tostring(err))
                -- 状态行只有一行，所以这里先折成一行、掐掉栈回溯与 `runtime error:` 前缀。
                local reason = tostring(err):gsub("%s+", " ")
                reason = reason:match("^(.-) stack traceback") or reason
                reason = reason:gsub("^runtime error: ", "")
                last_key = "K destroy#2 -> " .. one_line(reason, 44)
            end
        end
    end

    if input.just_pressed("s") then
        -- 写盘只在按键分支里（README 硬规则 2）。槽名先切到 "demo" 再切回默认槽，
        -- 免得本演示把别的存档写到 demo 槽去（`save.slot(nil)` = 回到默认槽）。
        save.slot(SAVE_SLOT)
        local slot_while_set = save.slot_get()
        local committed = save.commit()
        save.slot(nil)
        last_key = string.format("S slot=%s commit=%s", dash(slot_while_set), tostring(committed))
        log.info("api_core save.commit -> " .. tostring(committed))
    end

    if input.just_pressed("l") then
        -- 读盘同样只在按键分支里。`save.load()` 返回 `(是否成功, 原因)`：
        -- 还没存过档是 `false, "missing"`，与"存档损坏"（`false, "corrupt: ..."`）分得开。
        save.slot(SAVE_SLOT) -- 与 S 键同一个槽，因此 S 之后按 L 应该真的读回来
        local loaded_ok, reason = save.load()
        local restored = save.read(SAVE_KEY) -- 读档之后同一个键拿到的是**存档里的**值
        save.slot(nil)
        if loaded_ok then
            -- 注意：读回来的值是**提交那一刻**内存里的那份，因此会比当前帧旧一点。
            last_key = string.format("L loaded ok '%s'", dash(restored))
        else
            last_key = string.format("L load refused: %s", dash(reason))
        end
    end

    if input.just_pressed("c") then
        -- `save.clear()` 清空**内存**里的全部键并返回清掉的条数
        --（磁盘文件要等下一次 `save.commit` 才被整体替换）。
        local removed = save.clear()
        last_key = string.format("C save.clear removed=%s", dash(removed))
    end

    if input.just_pressed("v") then
        data.cache_clear()
        data.schema(DATA_PATH, CACHE_SPEC) -- 重新登记校验器（顺带让该路径的缓存失效）
        last_key = "V data.cache_clear + schema re-registered"
    end

    if input.just_pressed("f") then
        -- `scene.load(name)` 返回"场景文件是否存在"；真正的切换由宿主在**步与步之间**执行
        -- （`RuntimeScene` 的 `scene_transition` 系统 → `process_scene_transition`）。
        local exists = scene.load("index")
        last_key = string.format("F scene.load(index)=%s", tostring(exists))
        log.info("api_core scene.load(index) -> " .. tostring(exists))
    end

    if input.just_pressed("q") then
        app.quit() -- 只置引擎的退出请求标记，桌面运行器在帧末真正退出
        last_key = "Q app.quit requested"
        log.info("api_core app.quit requested")
    end

    -- -----------------------------------------------------------------------
    -- ④ 本帧的读回：全部发生在按键之后，因此这一行是**按键之后**的世界。
    -- -----------------------------------------------------------------------

    -- entity 生命周期：`is_alive` 对未知/已销毁 id 返回 false（不报错），所以它是这条链的
    -- 第一道判活；`persist_id` / `find_id` 在销毁后都返回 nil —— 稳定 ID 随实体一起注销。
    local ghost_alive = ghost ~= nil and entity.is_alive(ghost)
    local ghost_gen = nil
    local ghost_pid = nil
    if ghost_alive then
        ghost_gen = ghost % GENERATION_MOD -- 低 32 位 = generation（代际）
        ghost_pid = entity.persist_id(ghost) -- 反方向查询：从实体读回它注册过的稳定 ID
    end
    local ghost_lookup = entity.find_id(GHOST_PERSIST) -- 按稳定 ID 反查实体
    local ghost_by_name = entity.find(GHOST_NAME) -- 按名字查（销毁后名字也被注销）

    -- health / tag：读之前判活 —— 对已销毁的 id 调用它们会报 Lua 错误（docs/18）。
    if hero ~= nil and entity.is_alive(hero) then
        hero_hp = health.get(hero)
        hero_tag = tag.get(hero, TAG_KEY)
    end

    -- event：每帧发一次事件。放在按键之后，所以按 1 / 2 的那一帧就能看到对应计数器停住。
    event.emit(EVENT_HIT, { amount = HIT_DAMAGE, frame = frame })

    -- asset：取句柄 → 归还。句柄是**字符串** `asset:<路径>`（不是对象，也没有引用计数），
    -- 这里取完立刻还，因此状态行里那一串就是 `asset.load` 的真实返回值。
    asset_token = asset.load(COIN_TEXTURE)
    asset.release(asset_token)

    -- scene：`scene.reload()` 是"当前场景名"的读法，直接显示；切场景在 F 键分支里。
    scene_name = dash(scene.reload())

    -- save：键值 + 任意表 + 槽。这三条读/写路径**都只写内存**（`ScriptState::save_data`）。
    save.write(SAVE_KEY, string.format("frame=%d hp=%.0f", frame, hero_hp))
    save_readback = save.read(SAVE_KEY)
    save.blob_write(BLOB_KEY, { hp = hero_hp, frame = frame, ghost_alive = ghost_alive })
    local blob = save.blob_read(BLOB_KEY)
    if type(blob) == "table" then
        blob_hp = blob.hp -- 任意 Lua 表往返一趟之后读回来（blob_read 返回 nil 表示没有这个键）
    end
    save_slot_now = save.slot_get()

    -- data：注册过校验器（on_create）之后，三条读路径必须给出同一份内容：
    --   load = 命中缓存（第一次真读盘）、reload = 无条件绕过缓存重读、
    --   cache_clear 之后 load = 缓存已空所以又读了一次盘。
    -- 三条一致才说明"缓存没有把数据变旧"。这里一次都不写盘。
    local loaded = data.load(DATA_PATH)
    local reloaded = data.reload(DATA_PATH)
    data.cache_clear() -- 注意：清的是**全部** data 缓存，不是只清这一个路径
    local after_clear = data.load(DATA_PATH)
    data_counts = string.format(
        "%d/%d/%d",
        key_count(loaded.entries),
        key_count(reloaded.entries),
        key_count(after_clear.entries)
    )
    data_version = loaded.version

    -- -----------------------------------------------------------------------
    -- ⑤ 本帧的事实：全部来自上面那些 API 的读回值，分两行画。
    --    状态行（`api.status`）= 会随按键变化的交互事实；私有行（`ui.text_at`）= IO 读回值。
    --    `ev on=/sub=` 是最直观的一处：按 1、2 之后其中一个会当场停住。
    -- -----------------------------------------------------------------------
    api.status(
        string.format(
            "f%d dt=%.1fms | key=%s | ghost g=%s alive=%s pid=%s fid=%s | hp=%.0f tag=%s "
                .. "| ev on=%d sub=%d | find %d/3",
            frame,
            dt * 1000.0,
            one_line(last_key, 44),
            dash(ghost_gen),
            tostring(ghost_alive),
            dash(ghost_pid),
            dash(ghost_lookup),
            hero_hp,
            dash(hero_tag),
            hits_by_on,
            hits_by_subscribe,
            found
        )
    )
    -- 第二行只放"从引擎读回来的存储/资源事实"，每一个都对应一次 API 调用：
    --   scene.reload()、save.read()、save.slot_get()、save.blob_read()、
    --   data.load()（版本与条目数）、asset.load()（句柄原文）、event 载荷。
    ui.text_at(
        FACTS_ID,
        24,
        FACTS_Y,
        string.format(
            "scene=%s | save.read='%s' slot=%s | blob.hp=%s | data v=%s e=%s | asset=%s | "
                .. "find(name)=%s | emit[%s]",
            scene_name,
            dash(save_readback),
            dash(save_slot_now),
            dash(blob_hp),
            dash(data_version),
            data_counts,
            dash(asset_token),
            dash(ghost_by_name),
            last_hit_payload
        ),
        1.0
    )
    return string.format(
        "api_core_ok frame=%d scene=%s hp=%.0f tag=%s ghost_alive=%s events=%d/%d",
        frame,
        scene_name,
        hero_hp,
        dash(hero_tag),
        tostring(ghost_alive),
        hits_by_on,
        hits_by_subscribe
    )
end

--- 离开场景时释放动态 UI：私有读回行 + `api_demo` 登记的三行。
---
--- 这里两个调用都要写：`script.rs` 的 `ui.clear(id)` 只删**面板/图片/网格**（以及
--- `id:title` / `id:tooltip` 两个变体），**不碰** `ui.text_at` 建的那条普通命名文本
--- （它在 `ScriptState::ui` 里）。所以真正让这一行从画面上消失的是 `ui.hide`。
function M.on_destroy()
    ui.hide(FACTS_ID)
    ui.clear(FACTS_ID)
    api.clear()
end

return M
