-- api_core：核心运行时的七个步骤 —— 实体身份 / 代际 / 生命值与标签 / 事件总线 /
-- 数据 schema / 资源与平台 / 存档。
--
-- 每个步骤只主张一件事，右侧 API 卡列出这一步**真实调用**的函数，页脚写着按键与结论。
-- 所有读数都来自 API 读回（脚本不编造演示数字），因此读回值对了就等于接口语义还在。
--
-- 硬规则（README §2）：
--   * 顶层只定义常量与函数；
--   * `run()` 在 headless 下安全：只做内存读写，不落盘、不切场景、不退出、不模拟输入；
--   * `save.commit` / `save.load` / `save.clear` / `scene.load` / `scene.reload` /
--     `data.reload` / `app.quit` 全锁在按键分支里（F5 的自检碰不到它们）；
--   * 自检按 enter → run×3 → check 执行，所以每个 `enter` 都把本步依赖的状态写回初始值
--     （幽灵实体重生、血量/标签复位、订阅表重建、缓存与存档键重写）。
--
-- 场景里的 Hero / Dummy / Pickup 是 `entity.find` 的目标，本演示只给 Hero 上色，
-- 不移动任何场景实体（移动/物理/动画各有别的 Lab）。

local lab = require("lab")

local M = {}

local HERO_NAME = "Hero"
local DUMMY_NAME = "Dummy"
local PICKUP_NAME = "Pickup"
local COIN_TEXTURE = "assets/textures/coin.png"
local DATA_PATH = "data/cache_demo.json"
local EVENT_HIT = "api_core.hit"
local GHOST_NAME = "GhostProbe"
local GHOST_PERSIST = "demo.ghost"
local TAG_KEY = "mood"
local SAVE_KEY = "api_core.frame"
local BLOB_KEY = "api_core.snapshot"
local SAVE_SLOT = "demo"
local HERO_MAX_HP = 100
local HIT_DAMAGE = 7
-- 实体句柄 = index << 32 | generation；低 32 位是代际，销毁后重生会 +1。
local GENERATION_MOD = 4294967296

-- 模块状态（普通 Lua 局部变量，不是引擎状态）。
local frame = 0
local hero = nil
local ghost = nil
local scene_name = "?"
local action = "-"
local hits_by_on = 0
local hits_by_sub = 0
local last_payload = "-"
local ghost_alive_now = false
local ghost_pid = nil
local ghost_by_id = nil
local ghost_by_name = nil
local hero_hp = 0
local hero_tag = nil
local save_readback = nil
local save_slot_now = nil
local blob_hp = nil
local asset_token = nil
local platform = "-"
local project_root = ""
local data_version = nil
local data_entries = 0
local data_source = "-"

-- `data.schema` 的校验器：与 data/cache_demo.json 的形状一致。
local CACHE_SPEC = {
    ["$kind"] = "object",
    fields = {
        version = "integer",
        source = "string",
        entries = {
            ["$kind"] = "map",
            values = {
                ["$kind"] = "object",
                fields = { name = "string", value = "integer", stack = "integer" },
                required = { "name", "value", "stack" },
            },
        },
    },
    required = { "version", "entries" },
}

local function dash(value)
    if value == nil then
        return "-"
    end
    return tostring(value)
end

local function count_keys(value)
    if type(value) ~= "table" then
        return 0
    end
    local count = 0
    for _ in pairs(value) do
        count = count + 1
    end
    return count
end

local function is_ghost_alive()
    return ghost ~= nil and entity.is_alive(ghost)
end

local function drop_ghost()
    if is_ghost_alive() then
        entity.destroy(ghost)
    end
    ghost = nil
end

--- 重生幽灵：先销毁上一代再生成，保证每次 enter 都从同一状态出发。
--- `entity.spawn` 对仍被活实体占用的稳定 ID 会报错，所以必须先判活。
local function ensure_ghost()
    drop_ghost()
    ghost = entity.spawn(GHOST_NAME, GHOST_PERSIST)
    return ghost
end

--- 事件订阅表按事件名整体重建：`event.on` / `event.subscribe` 不做去重，
--- 不清掉旧回调的话 F5 会把同一个函数再压一遍、计数翻倍。
local function reset_events()
    event.off(EVENT_HIT)
    event.unsubscribe(EVENT_HIT)
    hits_by_on = 0
    hits_by_sub = 0
    last_payload = "-"
    event.on(EVENT_HIT, on_hit)
    event.subscribe(EVENT_HIT, on_sub)
end

--- 用 Sprite 的 tint 把血量画在 Hero 身上（读回 + 写回同一份组件串）。
local function tint_hero(r, g, b)
    if hero == nil or not entity.is_alive(hero) then
        return
    end
    local current = lab.try(sprite.get, hero)
    if current == nil or current.texture == nil then
        return
    end
    sprite.set(
        hero,
        string.format(
            "texture=%s;size=%dx%d;pivot=%.2f,%.2f;tint=%.3f,%.3f,%.3f,%.3f;sorting_layer=%s;order=%d",
            current.texture,
            current.size_x,
            current.size_y,
            current.pivot_x,
            current.pivot_y,
            r,
            g,
            b,
            current.tint_a,
            current.sorting_layer or "default",
            current.order or 0
        )
    )
end

local function paint_by_hp(hp)
    local ratio = math.max(0.0, math.min(1.0, hp / HERO_MAX_HP))
    tint_hero(0.45 + 0.55 * (1.0 - ratio), 0.35 + 0.65 * ratio, 0.35 + 0.20 * ratio)
end

local function reset_hero()
    hero = entity.find(HERO_NAME)
    if hero == nil then
        return
    end
    health.set(hero, HERO_MAX_HP)
    tag.set(hero, TAG_KEY, "calm")
    tint_hero(1.0, 1.0, 1.0)
end

-- 事件回调：`event.on` 与 `event.subscribe` 是同一总线的两个入口，
-- 这里用两个不同的函数订阅，一次 emit 应该让两个计数器同时 +1。
function on_hit(data)
    hits_by_on = hits_by_on + 1
    if type(data) == "table" then
        last_payload = string.format("amount=%s frame=%s", tostring(data.amount), tostring(data.frame))
    end
end

function on_sub()
    hits_by_sub = hits_by_sub + 1
end

local steps = {
    {
        id = "entity_ids",
        title = "Entity IDs",
        claim = "名字、稳定 ID 与实体句柄是三条查询路径，指向同一个实体",
        api = { "entity.spawn", "entity.find", "entity.find_id", "entity.persist_id", "entity.is_alive" },
        code = "id = entity.spawn(name, persist); entity.find(name); entity.find_id(persist)",
        enter = function()
            ensure_ghost()
            action = "spawned " .. dash(ghost)
        end,
        run = function()
            ghost_alive_now = is_ghost_alive()
            ghost_pid = ghost_alive_now and entity.persist_id(ghost) or nil
            ghost_by_id = entity.find_id(GHOST_PERSIST)
            ghost_by_name = entity.find(GHOST_NAME)
            lab.note("g destroy / h respawn; find / find_id should both follow")
        end,
        read = function()
            return {
                handle = dash(ghost),
                generation = dash(ghost_alive_now and (ghost % GENERATION_MOD) or nil),
                persist = dash(ghost_pid),
                find_id = dash(ghost_by_id),
                find_name = dash(ghost_by_name),
                alive = tostring(ghost_alive_now),
            }
        end,
        check = function(t)
            t:ok(ghost ~= nil, "entity.spawn returned a handle")
            t:eq(entity.find(GHOST_NAME), ghost, "entity.find resolves the runtime name")
            t:eq(entity.find_id(GHOST_PERSIST), ghost, "entity.find_id resolves the stable id")
            t:eq(entity.persist_id(ghost), GHOST_PERSIST, "entity.persist_id reads the stable id back")
            t:ok(entity.is_alive(ghost), "entity.is_alive sees the spawned entity")
        end,
    },
    {
        id = "entity_generation",
        title = "Generation",
        claim = "销毁会注销名字与稳定 ID；重生拿到的是换代的新句柄",
        api = { "entity.destroy", "entity.is_alive", "entity.spawn", "entity.find", "entity.find_id" },
        code = "entity.destroy(id); entity.is_alive(id) -> false; id2 = entity.spawn(name, persist)",
        enter = function()
            ensure_ghost()
            action = "generation " .. dash(ghost % GENERATION_MOD)
        end,
        run = function()
            ghost_alive_now = is_ghost_alive()
            ghost_by_id = entity.find_id(GHOST_PERSIST)
            ghost_by_name = entity.find(GHOST_NAME)
            lab.note("g destroys the handle; x repeats destroy to show the error contract")
        end,
        read = function()
            return {
                handle = dash(ghost),
                generation = dash(ghost_alive_now and (ghost % GENERATION_MOD) or nil),
                alive = tostring(ghost_alive_now),
                find_name = dash(ghost_by_name),
                find_id = dash(ghost_by_id),
            }
        end,
        check = function(t)
            local before = ghost
            local before_generation = before % GENERATION_MOD
            entity.destroy(before)
            t:ok(not entity.is_alive(before), "destroyed handle is no longer alive")
            t:eq(entity.find(GHOST_NAME), nil, "destroy unregisters the name")
            t:eq(entity.find_id(GHOST_PERSIST), nil, "destroy unregisters the stable id")
            ghost = entity.spawn(GHOST_NAME, GHOST_PERSIST)
            t:ok(entity.is_alive(ghost), "respawn with the same stable id succeeds")
            t:ok(ghost ~= before, "respawn gets a new generation")
            t:eq(entity.find_id(GHOST_PERSIST), ghost, "stable id now resolves to the new handle")
            t:ok((ghost % GENERATION_MOD) ~= before_generation, "generation part changed")
        end,
    },
    {
        id = "health_tag",
        title = "Health & Tag",
        claim = "血量是数值账本（damage 读改写），标签是键值袋（空串即删除）",
        api = { "health.set", "health.get", "health.damage", "tag.set", "tag.get" },
        code = "health.damage(id, 7); health.get(id); tag.set(id, 'mood', 'calm'); tag.get(id, 'mood')",
        enter = function()
            reset_hero()
            action = "hero reset"
        end,
        run = function()
            if hero ~= nil and entity.is_alive(hero) then
                hero_hp = health.get(hero)
                hero_tag = tag.get(hero, TAG_KEY)
                paint_by_hp(hero_hp)
            end
            lab.note("k damage / y heal / t toggle tag; Hero tint follows hp")
        end,
        read = function()
            return {
                hp = string.format("%.0f", hero_hp),
                max = tostring(HERO_MAX_HP),
                tag = dash(hero_tag),
                tag_key = TAG_KEY,
                damage = tostring(HIT_DAMAGE),
            }
        end,
        check = function(t)
            if hero == nil or not entity.is_alive(hero) then
                t:ok(false, "scene Hero is missing")
                return
            end
            health.set(hero, HERO_MAX_HP)
            t:near(health.get(hero), HERO_MAX_HP, 0.001, "health.set writes the value")
            local left = health.damage(hero, HIT_DAMAGE)
            t:near(left, HERO_MAX_HP - HIT_DAMAGE, 0.001, "health.damage returns remaining hp")
            t:near(health.get(hero), HERO_MAX_HP - HIT_DAMAGE, 0.001, "damage is readable back")
            t:near(health.damage(hero, 1000), 0.0, 0.001, "damage clamps at zero")
            health.set(hero, HERO_MAX_HP)
            t:eq(tag.get(hero, TAG_KEY), "calm", "tag.get reads the seeded tag")
            tag.set(hero, TAG_KEY, "")
            t:eq(tag.get(hero, TAG_KEY), nil, "empty value removes the tag")
            tag.set(hero, TAG_KEY, "calm")
            t:eq(tag.get(hero, TAG_KEY), "calm", "tag.set writes the tag back")
        end,
    },
    {
        id = "event_bus",
        title = "Event bus",
        claim = "on 与 subscribe 是同一总线的两个名字；off/unsubscribe 只摘各自那个回调",
        api = { "event.on", "event.subscribe", "event.emit", "event.off", "event.unsubscribe" },
        code = "event.on(name, cb); event.subscribe(name, cb2); event.emit(name, data)",
        enter = function()
            reset_events()
            action = "subscriptions rebuilt"
        end,
        run = function()
            event.emit(EVENT_HIT, { amount = HIT_DAMAGE, frame = frame })
            lab.note(string.format("emit every frame: on=%d sub=%d", hits_by_on, hits_by_sub))
        end,
        read = function()
            return {
                on_count = tostring(hits_by_on),
                sub_count = tostring(hits_by_sub),
                payload = last_payload,
                key_off = "o",
                key_unsub = "u",
                key_resub = "i",
            }
        end,
        check = function(t)
            local before_on, before_sub = hits_by_on, hits_by_sub
            event.emit(EVENT_HIT, { amount = 1, frame = 0 })
            t:eq(hits_by_on, before_on + 1, "emit reaches the on() subscriber")
            t:eq(hits_by_sub, before_sub + 1, "emit reaches the subscribe() subscriber")
            t:ok(last_payload:find("amount=1", 1, true) ~= nil, "payload arrives unchanged")
            event.off(EVENT_HIT, on_hit)
            event.emit(EVENT_HIT, { amount = 2 })
            t:eq(hits_by_on, before_on + 1, "off removes only the on() callback")
            t:eq(hits_by_sub, before_sub + 2, "subscribe() keeps receiving")
            event.on(EVENT_HIT, on_hit)
            event.unsubscribe(EVENT_HIT, on_sub)
            event.emit(EVENT_HIT, { amount = 3 })
            t:eq(hits_by_on, before_on + 2, "on() restores the callback")
            t:eq(hits_by_sub, before_sub + 2, "unsubscribe removes only the subscribe() callback")
            event.subscribe(EVENT_HIT, on_sub)
            event.emit(EVENT_HIT, { amount = 4 })
            t:eq(hits_by_on, before_on + 3, "on() still subscribed")
            t:eq(hits_by_sub, before_sub + 3, "subscribe() restored")
        end,
    },
    {
        id = "data_schema",
        title = "Data schema",
        claim = "schema 注册后真的在 data.load 上校验；cache_clear 只清缓存不改文件",
        api = { "data.schema", "data.load", "data.reload", "data.cache_clear" },
        code = "data.schema(path, spec); data.load(path); data.reload(path); data.cache_clear()",
        enter = function()
            data.schema(DATA_PATH, CACHE_SPEC)
            data.cache_clear()
            local loaded = lab.try(data.load, DATA_PATH)
            data_version = loaded and loaded.version or nil
            data_entries = loaded and count_keys(loaded.entries) or 0
            data_source = loaded and dash(loaded.source) or "-"
            action = "schema + cache reset"
        end,
        run = function()
            local loaded = lab.try(data.load, DATA_PATH)
            if loaded ~= nil then
                data_version = loaded.version
                data_entries = count_keys(loaded.entries)
                data_source = dash(loaded.source)
            end
            lab.note("d data.reload / c cache_clear then load; READ shows the parsed table")
        end,
        read = function()
            return {
                path = DATA_PATH,
                version = dash(data_version),
                entries = tostring(data_entries),
                source = data_source,
                schema = "registered",
            }
        end,
        check = function(t)
            local loaded = lab.try(data.load, DATA_PATH)
            t:ok(loaded ~= nil, "data.load parses data/cache_demo.json")
            if loaded == nil then
                return
            end
            t:eq(loaded.version, 1, "version field round-trips as an integer")
            t:eq(count_keys(loaded.entries), 2, "entries map has two keys")
            t:eq(loaded.entries.coin.value, 5, "nested integer reads back")
            -- 反向验证：换一份不匹配的 schema，下一次 load 必须被拒绝。
            data.schema(DATA_PATH, { ["$kind"] = "object", fields = { version = "string" }, required = { "version" } })
            data.cache_clear()
            local rejected = lab.try(data.load, DATA_PATH)
            t:eq(rejected, nil, "schema rejects a table that misses a required field")
            data.schema(DATA_PATH, CACHE_SPEC)
            data.cache_clear()
            t:ok(lab.try(data.load, DATA_PATH) ~= nil, "restored schema loads again")
            t:eq(count_keys(loaded.entries), data_entries, "cache_clear does not change content")
        end,
    },
    {
        id = "asset_app",
        title = "Asset & app",
        claim = "asset.load 返回可归还的句柄字符串；app 给平台与工程根两条只读事实",
        api = { "asset.load", "asset.release", "app.platform", "app.project_root", "scene.load", "scene.reload", "app.quit" },
        code = "token = asset.load(path); asset.release(token); app.platform(); scene.reload()",
        enter = function()
            local token = asset.load(COIN_TEXTURE)
            asset.release(token)
            action = "handle acquired + released"
        end,
        run = function()
            asset_token = asset.load(COIN_TEXTURE)
            asset.release(asset_token)
            platform = app.platform()
            project_root = app.project_root()
            lab.note("n scene.reload / v scene.load(index) / q app.quit (keys only)")
        end,
        read = function()
            return {
                token = dash(asset_token),
                platform = platform,
                scene = scene_name,
                project_root = project_root ~= "" and project_root or "(unset)",
                bind = "n / v / q",
            }
        end,
        check = function(t)
            local token = asset.load(COIN_TEXTURE)
            t:eq(token, "asset:" .. COIN_TEXTURE, "asset.load returns the handle string")
            asset.release(token)
            t:ok(app.platform() == "desktop" or app.platform() == "webgl", "app.platform is a host id")
            local root = app.project_root()
            t:ok(root == "" or root:find("game_api", 1, true) ~= nil, "app.project_root points at the project")
            t:eq(scene_name, "api_core", "scene.reload reported the current scene")
        end,
    },
    {
        id = "save_slots",
        title = "Save slots",
        claim = "键值、任意表与槽位都往返一次；落盘的 commit 只放在按键里",
        api = { "save.write", "save.read", "save.blob_write", "save.blob_read", "save.slot", "save.slot_get", "save.commit", "save.load", "save.clear" },
        code = "save.write(k, v); save.read(k); save.blob_write(k, t); save.slot('demo')",
        enter = function()
            save.slot(nil)
            save.write(SAVE_KEY, "enter")
            save.blob_write(BLOB_KEY, { hp = HERO_MAX_HP, frame = 0 })
            save_readback = save.read(SAVE_KEY)
            save_slot_now = save.slot_get()
            action = "in-memory save keys seeded"
        end,
        run = function()
            local hp = 0
            if hero ~= nil and entity.is_alive(hero) then
                hp = health.get(hero)
            end
            save.write(SAVE_KEY, string.format("frame=%d hp=%.0f", frame, hp))
            save_readback = save.read(SAVE_KEY)
            save.blob_write(BLOB_KEY, { hp = hp, frame = frame, ghost = is_ghost_alive() })
            local blob = save.blob_read(BLOB_KEY)
            blob_hp = type(blob) == "table" and blob.hp or nil
            save_slot_now = save.slot_get()
            lab.note("s commit / l load / c clear; write & read stay in memory")
        end,
        read = function()
            return {
                key = SAVE_KEY,
                value = dash(save_readback),
                slot = dash(save_slot_now),
                blob_hp = dash(blob_hp),
                bind_commit = "s",
                bind_load = "l",
                bind_clear = "c",
                scope = "in-memory",
            }
        end,
        check = function(t)
            save.write(SAVE_KEY, "probe")
            t:eq(save.read(SAVE_KEY), "probe", "save.write round-trips through save.read")
            save.blob_write(BLOB_KEY, { hp = 42, tags = { "a", "b" } })
            local blob = save.blob_read(BLOB_KEY)
            t:ok(type(blob) == "table", "blob_read returns a table")
            t:eq(blob.hp, 42, "blob numbers round-trip")
            t:eq(blob.tags[2], "b", "blob arrays round-trip")
            save.slot(SAVE_SLOT)
            t:eq(save.slot_get(), SAVE_SLOT, "save.slot switches the active slot")
            save.slot(nil)
            t:eq(save.slot_get(), nil, "save.slot(nil) restores the default slot")
            t:eq(save.read("api_core.missing"), nil, "unknown key reads nil")
        end,
    },
}

function M.on_create()
    scene_name = scene.reload() or "api_core"
    hero = entity.find(HERO_NAME)
    lab.start({
        scene = "api_core",
        title = "Core Runtime",
        category = "Core",
        steps = steps,
        hide_scene_text = {},
    })
    log.info(string.format("api_core lab: %d steps, scene=%s", #steps, scene_name))
end

function M.update()
    frame = frame + 1
    local step = lab.step()
    local id = step and step.id or ""

    if id == "entity_ids" or id == "entity_generation" then
        local managed = id == "entity_ids" or id == "entity_generation"
        if managed and input.just_pressed("g") then
            if is_ghost_alive() then
                entity.destroy(ghost)
                action = "g destroy -> dead"
                log.info("api_core ghost destroyed")
            else
                action = "g ghost already dead"
            end
        elseif managed and input.just_pressed("h") then
            if is_ghost_alive() then
                action = "h ghost still alive"
            else
                ensure_ghost()
                action = "h respawn id=" .. dash(ghost)
            end
        elseif id == "entity_generation" and input.just_pressed("x") then
            -- 预期失败：对已销毁的句柄再 destroy，引擎报 Lua 错误而不是静默返回 false。
            if is_ghost_alive() then
                entity.destroy(ghost)
            end
            local ok, err = pcall(function()
                entity.destroy(ghost)
            end)
            if ok then
                action = "x unexpected: second destroy succeeded"
            else
                log.error("api_core expected failure: " .. tostring(err))
                action = "x second destroy rejected"
            end
        end
    elseif id == "health_tag" then
        if hero == nil or not entity.is_alive(hero) then
            hero = entity.find(HERO_NAME)
        end
        if hero ~= nil and entity.is_alive(hero) then
            if input.just_pressed("k") then
                local left = health.damage(hero, HIT_DAMAGE)
                if left <= 0.0 then
                    health.set(hero, HERO_MAX_HP)
                end
                action = string.format("k damage -%d left=%.0f", HIT_DAMAGE, left)
                log.warn("api_core hero hit: " .. action)
            elseif input.just_pressed("y") then
                health.set(hero, HERO_MAX_HP)
                action = string.format("y health.set(%d)", HERO_MAX_HP)
            elseif input.just_pressed("t") then
                if tag.get(hero, TAG_KEY) ~= nil then
                    tag.set(hero, TAG_KEY, "")
                    action = "t tag removed"
                else
                    tag.set(hero, TAG_KEY, "calm")
                    action = "t tag=calm"
                end
            end
        end
    elseif id == "event_bus" then
        if input.just_pressed("o") then
            event.off(EVENT_HIT, on_hit)
            action = "o off(on)"
        elseif input.just_pressed("u") then
            event.unsubscribe(EVENT_HIT, on_sub)
            action = "u unsubscribe(sub)"
        elseif input.just_pressed("i") then
            reset_events()
            action = "i resubscribed both"
        elseif input.just_pressed("e") then
            event.emit(EVENT_HIT, { amount = 1, frame = frame, manual = true })
            action = "e manual emit"
        end
    elseif id == "data_schema" then
        if input.just_pressed("d") then
            local value = lab.try(data.reload, DATA_PATH)
            action = value ~= nil and "d data.reload ok" or "d data.reload failed"
        elseif input.just_pressed("c") then
            data.cache_clear()
            lab.try(data.load, DATA_PATH)
            action = "c cache_clear + load"
        end
    elseif id == "asset_app" then
        if input.just_pressed("n") then
            scene_name = scene.reload() or scene_name
            action = "n scene.reload -> " .. scene_name
        elseif input.just_pressed("v") then
            local exists = scene.load("index")
            action = "v scene.load(index)=" .. tostring(exists)
        elseif input.just_pressed("q") then
            app.quit()
            action = "q app.quit requested"
        end
    elseif id == "save_slots" then
        if input.just_pressed("s") then
            save.slot(SAVE_SLOT)
            local committed = save.commit()
            local slot_now = save.slot_get()
            save.slot(nil)
            action = string.format("s slot=%s commit=%s", dash(slot_now), tostring(committed))
            log.info("api_core save.commit -> " .. tostring(committed))
        elseif input.just_pressed("l") then
            save.slot(SAVE_SLOT)
            local loaded_ok, reason = save.load()
            local restored = save.read(SAVE_KEY)
            save.slot(nil)
            if loaded_ok then
                action = "l loaded " .. dash(restored)
            else
                action = "l load refused: " .. dash(reason)
            end
        elseif input.just_pressed("c") then
            action = "c save.clear removed=" .. tostring(save.clear())
        end
    end

    -- 页脚右侧那一小段：按键刚做了什么（claim 在左边，两者互不遮挡）。
    if action ~= "-" then
        lab.status(action)
    end
    return lab.tick()
end

function M.self_test()
    return lab.self_test()
end

function M.on_destroy()
    drop_ghost()
    event.off(EVENT_HIT)
    event.unsubscribe(EVENT_HIT)
    if hero ~= nil and entity.is_alive(hero) then
        health.set(hero, HERO_MAX_HP)
        tag.set(hero, TAG_KEY, "calm")
        tint_hero(1.0, 1.0, 1.0)
    end
    lab.stop()
end

return M
