-- api_gameplay：玩法层 API 的分步 Lab —— clock / calendar / day / item / inv /
-- container / shop / interact / map / tag 各主张一件事。
--
-- 一句话主线：这些模块是**同一份运行期状态的几个视角**，不是各自独立的仪表。
--   * clock / calendar / day 是同一台游戏时钟：`clock.now()` 给「第几天 + 当天分钟」，
--     `calendar.now()` 把它换算成季节/年，`day.now()` 是日循环钩子的入口读数。
--   * `item.define` 注册的定义是 `inv` 堆叠上限的唯一来源；`inv.add` 的返回值是塞不进去的余数。
--   * `container.open(实体, 容量)` 把场景实体变成第二个背包，返回的 `container:<id>` 就是
--     `inv.move` 的目标 id —— 玩家背包与箱子走同一套接口。
--   * `map.state` / `map.schema` 是分地图持久状态；`map.goto/go/enter` 只排队切图，
--     所以那一组只放在按键分支里（自检不按任何键）。
--   * `interact.query` 的候选集按实体 tag 筛（`interact` 或 `kind`），`interact.prompt` 写的是
--     引擎侧提示文本；`day.sleep` 依次触发 `on_day_end` → 次日 06:00 → `on_day_start`，
--     本模块的两个钩子把触发次数记在 `hooks` 里，read/check 都能读回。
--
-- 交互键（Lab 保留 space/方向键/1-9/r/f/f5，不用它们）：
--   clock_views      t=+60m  p=pause  n=advance
--   clock_advance    t=重置  n=+60m   s=换 rate  p=pause
--   day_sleep        n=day.sleep()  m=clock.sleep_to_morning()
--   calendar_config  y=calendar.set(12,3,2,400)  u=7x3  i=28x4
--   inventory        i=+5 种子  k=-2 种子  v=切 slot5
--   bag_slots        b=bag1→hotbar1  g=bag2↔hotbar2  v=set slot5
--   equipment        e=切 tool  q=切 trinket
--   containers       o=open  c=close  u=container.move→BAG5  l=bag→chest
--   shop             x=全价/半价目录
--   interact         c=半径 120/24  q=切农夫 role 标签
--   map_state        s=set_spawn  w=写 plots  d=清本图  a=清全部
--   map_switch       g=map.goto  e=map.enter  j=map.go（都自跳本场景）
--
-- 硬规则（README §2）：顶层只定义常量与函数；`run()` 在 headless 下只做内存读写；
-- 每个 `enter` 都重建该步依赖的时钟、日历、背包、容器、商店与地图状态。
local lab = require("lab")

local M = {}

-- ── 常量 ─────────────────────────────────────────────────────────────────────
local BAG = "lab_bag"
local HOTBAR = "lab_hotbar"
local BAG_CAPACITY = 12
local HOTBAR_SIZE = 9
local CHEST_CAPACITY = 8
local SHOP = "lab_stand"
local MAP_STATE = "lab_gameplay_farm"
local MAP_HERE = "api_gameplay"
local RADII = { 120.0, 24.0 }
local RATES = { 0.0, 60.0, 240.0 }

local FULL_CATALOG = {
    turnip_seed = { buy = 20, sell = 10 },
    watering_can = { buy = 120, sell = 60 },
    coin = { buy = 1, sell = 1 },
}
local SALE_CATALOG = {
    turnip_seed = { buy = 10, sell = 5 },
    watering_can = { buy = 60, sell = 30 },
    coin = { buy = 1, sell = 1 },
}

-- 分地图状态的轻量校验器（与 map.state 的写入形状一致）。
local MAP_SCHEMA = {
    ["$kind"] = "object",
    fields = {
        version = "integer",
        plots = "integer",
        sky = "string",
        notes = { ["$kind"] = "map", values = "any" },
    },
    required = { "version", "plots", "sky" },
}

-- 舞台里的世界布景：场景实体原本在左栏下面，这里把它们摆进舞台（x=296..936）。
local WORLD = {
    -- 菜地也搬进舞台：原场景把它摆在 x=80（左栏后面），只剩一条边可见。
    { name = "FarmPlot", x = 340.0, y = 150.0 },
    { name = "Farmer", x = 560.0, y = 360.0 },
    { name = "Chest", x = 640.0, y = 382.0 },
    { name = "Shopkeeper", x = 660.0, y = 340.0 },
}

-- ── 运行期状态（普通 Lua 局部变量，不是引擎状态）─────────────────────────────
local chest_id = nil
local farmer_id = nil
local radius_index = 1
local rate_index = 1
local advance = 0
local action = "ready"
local last_step = ""
local catalog_sale = false
local hooks = { day_end = 0, day_start = 0 }

-- ── 小工具（纯 Lua 或只读 API）───────────────────────────────────────────────

local function dash(value)
    if value == nil then
        return "-"
    end
    return tostring(value)
end

local function hhmm(now)
    if now == nil then
        return "--:--"
    end
    return string.format("%02d:%02d", now.hour or 0, (now.minute_of_day or 0) % 60)
end

--- 时钟三件套的复位：rate=0 冻结宿主推进，pause=false，再写死到指定时刻。
--- 每个依赖时钟的 `enter` 都先走这里，F5 才能从同一状态出发。
local function reset_clock(day, minute)
    clock.set_rate(0.0)
    clock.pause(false)
    clock.set(day, minute)
end

--- 日历体裁参数复位（默认 28 天/季、4 季/年）。
local function reset_calendar()
    calendar.configure({ days_per_season = 28, seasons_per_year = 4 })
end

--- 注册三个演示物品；`stack` 是 inv 堆叠上限的唯一来源。
local function define_items()
    item.define("turnip_seed", {
        name = "Turnip Seed",
        description = "Sow it in a tilled cell; one watered day per stage.",
        stack = 99,
        buy = 20,
        sell = 10,
        texture = "assets/textures/tileset.png",
        kind = "seed",
    })
    item.define("watering_can", {
        name = "Watering Can",
        description = "Equip it in the tool slot before watering.",
        stack = 1,
        buy = 120,
        sell = 60,
        texture = "assets/textures/player.png",
        kind = "tool",
    })
    item.define("coin", {
        name = "Coin",
        description = "Currency. The engine only prices items.",
        stack = 999,
        buy = 1,
        sell = 1,
        texture = "assets/textures/coin.png",
        kind = "currency",
    })
end

--- 固定初始背包：create 会重建整份背包（含装备槽），所以它同时是背包重置。
local function reset_bag()
    inv.create(BAG, BAG_CAPACITY)
    inv.hotbar(HOTBAR, HOTBAR_SIZE)
    inv.add(BAG, "turnip_seed", 5)
    inv.add(BAG, "watering_can", 1)
    inv.add(BAG, "coin", 12)
    inv.equip(BAG, "tool", "hoe")
end

local function reset_shop()
    shop.set_catalog(SHOP, FULL_CATALOG)
    catalog_sale = false
end

--- 打开箱子背包并放 10 枚金币；`container.open` 会重建，等价于容器重置。
local function open_chest()
    chest_id = entity.find("Chest")
    if chest_id == nil then
        return nil
    end
    local key = container.open(chest_id, CHEST_CAPACITY)
    inv.add(key, "coin", 10)
    return key
end

--- 世界布景：把三个场景实体摆进舞台、写好 interact 候选标签、清掉旧提示。
--- 位置每步都从同一组常数写回，读者按过任何键之后 F5 仍可复现。
local function reset_world()
    for _, spot in ipairs(WORLD) do
        local id = entity.find(spot.name)
        if id ~= nil and entity.is_alive(id) then
            transform.set(id, spot.x, spot.y)
        end
    end
    chest_id = entity.find("Chest")
    farmer_id = entity.find("Farmer")
    local shopkeeper_id = entity.find("Shopkeeper")
    if chest_id ~= nil then
        tag.set(chest_id, "kind", "chest")
        tag.set(chest_id, "interact", "open")
        tag.set(chest_id, "priority", "5")
    end
    if shopkeeper_id ~= nil then
        tag.set(shopkeeper_id, "kind", "shop")
        tag.set(shopkeeper_id, "interact", "trade")
        tag.set(shopkeeper_id, "priority", "1")
    end
    if farmer_id ~= nil then
        tag.set(farmer_id, "role", "player")
    end
    interact.clear_prompt()
end

--- 分地图状态复位：先注册 schema，再写一份合法状态并记住出生点。
local function reset_map_state()
    map.schema(MAP_STATE, MAP_SCHEMA)
    map.state(MAP_STATE, {
        version = 1,
        plots = 3,
        sky = "Sunny",
        notes = { owner = "Farmer", season = 1 },
    })
    map.set_spawn(144.0, 176.0)
end

--- 以农夫为原点查一次交互候选，并把最近的一个写进引擎提示。
local function refresh_prompt()
    local x, y = 144.0, 176.0
    if farmer_id ~= nil and entity.is_alive(farmer_id) then
        local here = transform.get(farmer_id)
        x, y = here.x, here.y
    end
    local radius = RADII[radius_index]
    local hits = interact.query(x, y, radius)
    local nearest = hits[1]
    local prompt = nil
    if nearest ~= nil then
        prompt = string.format("E - %s #%s", tostring(nearest.kind), tostring(nearest.id))
        interact.prompt(prompt)
    else
        interact.clear_prompt()
    end
    return hits, prompt
end

--- 背包占用概览；背包不存在时返回 0/0（read 不抛错）。
local function bag_usage(inventory)
    if not inv.exists(inventory) then
        return 0, 0
    end
    local data = inv.get(inventory)
    return data.occupied, data.capacity
end

--- 单格文本（空格返回 "-"）。
local function slot_text(inventory, index)
    if not inv.exists(inventory) then
        return "-"
    end
    local slot = inv.slot(inventory, index)
    if slot == nil then
        return "-"
    end
    return string.format("%s x%d", slot.item, slot.count)
end

local steps = {
    {
        id = "clock_views",
        title = "Clock views",
        claim = "clock / calendar / day 是同一台时钟的三个视角；rate=0 时它冻结，读数可复现",
        api = { "clock.now", "clock.set", "clock.set_rate", "clock.pause", "calendar.now", "day.now" },
        code = "clock.now(); calendar.now(); day.now(); clock.set(3, 480)",
        enter = function()
            reset_calendar()
            reset_clock(3, 480)
            hooks.day_end, hooks.day_start = 0, 0
            lab.note("t sets day 5 / n adds 60m / p pauses")
        end,
        run = function()
            local now = clock.now()
            local cal = calendar.now()
            local today = day.now()
            lab.note(string.format(
                "%s day=%d | calendar S%d d%d y%d | day.now=%d",
                hhmm(now),
                now.day,
                cal.season,
                cal.day,
                cal.year,
                today.day
            ))
        end,
        read = function()
            local now = clock.now()
            local cal = calendar.now()
            return {
                { label = "clock", value = hhmm(now) },
                { label = "day", value = tostring(now.day) },
                { label = "minute_of_day", value = tostring(now.minute_of_day) },
                { label = "rate", value = string.format("%.0f", now.rate) },
                { label = "paused", value = tostring(now.paused) },
                { label = "calendar", value = string.format("S%d d%d y%d", cal.season, cal.day, cal.year) },
                { label = "absolute_day", value = tostring(cal.absolute_day) },
            }
        end,
        check = function(t)
            local now = clock.now()
            t:eq(now.day, 3, "clock.set writes the day")
            t:eq(now.minute_of_day, 480, "clock.set writes the minute of day")
            t:eq(now.hour, 8, "clock.now derives hour from minute_of_day")
            t:near(now.rate, 0.0, 0.001, "rate 0 freezes the host advance")
            t:eq(now.paused, false, "the clock is running (rate 0 is not pause)")
            local cal = calendar.now()
            t:eq(cal.absolute_day, 3, "calendar.now reads the same clock")
            t:eq(cal.season, 1, "day 3 is still season 1")
            t:eq(cal.day, 3, "day of season matches")
            t:eq(cal.year, 1, "day 3 is still year 1")
            local today = day.now()
            t:eq(today.day, 3, "day.now agrees on the day")
            t:eq(today.minute, 480, "day.now agrees on the minute")
        end,
    },
    {
        id = "clock_advance",
        title = "Advance clock",
        claim = "clock.add 直接改写同一台时钟；set_rate / pause 是宿主推进它的两个开关",
        api = { "clock.add", "clock.now", "clock.set", "clock.set_rate", "clock.pause" },
        code = "clock.add(2); clock.set_rate(60); clock.pause(true)",
        enter = function()
            reset_clock(4, 600)
            advance = 0
            lab.note("every frame adds 2 game minutes; s cycles the host rate")
        end,
        run = function()
            clock.add(2.0)
            advance = advance + 2
            local now = clock.now()
            lab.note(string.format("%s day=%d (+%d min by run)", hhmm(now), now.day, advance))
        end,
        read = function()
            local now = clock.now()
            return {
                { label = "clock", value = hhmm(now) },
                { label = "day", value = tostring(now.day) },
                { label = "added", value = tostring(advance) },
                { label = "rate", value = string.format("%.0f", now.rate) },
                { label = "paused", value = tostring(now.paused) },
            }
        end,
        check = function(t)
            local now = clock.now()
            t:eq(now.minute_of_day, 600 + advance, "clock.add advanced the same clock")
            t:eq(now.day, 4, "small additions stay inside day 4")
            t:ok(advance > 0, "run() added at least one frame worth of minutes")
            clock.set_rate(60.0)
            t:near(clock.now().rate, 60.0, 0.001, "set_rate writes the host advance rate")
            clock.pause(true)
            t:eq(clock.now().paused, true, "pause(true) freezes the host advance")
            clock.pause(false)
            clock.set_rate(0.0)
            t:eq(clock.now().paused, false, "pause(false) resumes")
            t:near(clock.now().rate, 0.0, 0.001, "rate 0 freezes the clock again")
        end,
    },
    {
        id = "day_sleep",
        title = "Day sleep",
        claim = "day.sleep 先 on_day_end、再睡到次日 06:00、最后 on_day_start；clock.sleep_to_morning 不碰钩子",
        api = { "day.sleep", "day.now", "clock.sleep_to_morning", "clock.now", "clock.set", "clock.set_rate" },
        code = "day.sleep()  -- on_day_end -> next 06:00 -> on_day_start",
        enter = function()
            reset_calendar()
            reset_clock(6, 1200)
            hooks.day_end, hooks.day_start = 0, 0
            lab.note("n = day.sleep() (hooks fire) / m = clock.sleep_to_morning()")
        end,
        run = function()
            local now = clock.now()
            local today = day.now()
            lab.note(string.format(
                "day %d %s | on_day_end=%d on_day_start=%d",
                today.day,
                hhmm(now),
                hooks.day_end,
                hooks.day_start
            ))
        end,
        read = function()
            local now = clock.now()
            return {
                { label = "day", value = tostring(now.day) },
                { label = "clock", value = hhmm(now) },
                { label = "on_day_end", value = tostring(hooks.day_end) },
                { label = "on_day_start", value = tostring(hooks.day_start) },
                { label = "wake", value = "06:00" },
            }
        end,
        check = function(t)
            local before = clock.now().day
            local skipped = clock.sleep_to_morning()
            t:eq(skipped, before + 1, "sleep_to_morning advances the day")
            t:eq(clock.now().minute_of_day, 360, "sleep_to_morning wakes at 06:00")
            t:eq(hooks.day_end, 0, "sleep_to_morning fires no on_day_end")
            t:eq(hooks.day_start, 0, "sleep_to_morning fires no on_day_start")
            clock.set(before + 1, 1200)
            local slept = day.sleep()
            t:eq(slept, before + 2, "day.sleep advances the day")
            t:eq(hooks.day_end, 1, "day.sleep fires on_day_end once")
            t:eq(hooks.day_start, 1, "day.sleep fires on_day_start once")
            t:eq(day.now().day, before + 2, "day.now reads the new day")
        end,
    },
    {
        id = "calendar_config",
        title = "Calendar config",
        claim = "calendar.configure 改的是季长/年季数；越界日期在 calendar.set 写入处就被拒绝",
        api = { "calendar.configure", "calendar.set", "calendar.now", "clock.set", "clock.set_rate" },
        code = "calendar.configure({days_per_season=7,seasons_per_year=3}); calendar.set(7,3,1,120)",
        enter = function()
            reset_calendar()
            reset_clock(1, 360)
            lab.note("u=7x3 i=28x4 y=calendar.set(12,3,2,400)")
        end,
        run = function()
            local cal = calendar.now()
            lab.note(string.format(
                "absolute_day=%d -> S%d d%d y%d (default 28x4)",
                cal.absolute_day,
                cal.season,
                cal.day,
                cal.year
            ))
        end,
        read = function()
            local cal = calendar.now()
            return {
                { label = "absolute_day", value = tostring(cal.absolute_day) },
                { label = "season", value = tostring(cal.season) },
                { label = "day", value = tostring(cal.day) },
                { label = "year", value = tostring(cal.year) },
                { label = "check_wrote", value = "7x3 then restored" },
            }
        end,
        check = function(t)
            local configured = pcall(function()
                calendar.configure({ days_per_season = 7, seasons_per_year = 3 })
            end)
            t:ok(configured, "calendar.configure accepts 7 days x 3 seasons")
            local rejected = pcall(calendar.set, 8, 1, 1, 0)
            t:ok(not rejected, "calendar.set rejects day 8 under a 7-day season")
            calendar.set(7, 3, 1, 120)
            local cal = calendar.now()
            t:eq(cal.absolute_day, 21, "7x3 calendar maps S3 d7 to absolute day 21")
            t:eq(cal.season, 3, "season derives from the absolute day")
            t:eq(cal.day, 7, "day of season derives from the absolute day")
            t:eq(cal.year, 1, "absolute day 21 is still year 1")
            calendar.set(1, 1, 2, 0)
            local next_year = calendar.now()
            t:eq(next_year.absolute_day, 22, "year 2 starts at absolute day 22")
            t:eq(next_year.year, 2, "calendar.now reports year 2")
            calendar.configure({ days_per_season = 28, seasons_per_year = 4 })
            clock.set(1, 360)
            t:eq(calendar.now().absolute_day, 1, "restoring 28x4 puts the clock back on day 1")
        end,
    },
    {
        id = "inventory",
        title = "Inventory",
        claim = "item.define 的 stack 是堆叠上限；inv.add 返回塞不进去的余数，其余接口读写同一份背包",
        api = { "item.define", "item.def", "inv.create", "inv.hotbar", "inv.add", "inv.count", "inv.remove", "inv.slot" },
        code = "inv.add('lab_bag','turnip_seed',3) -> leftover; inv.count(...); inv.remove(...)",
        enter = function()
            define_items()
            reset_bag()
            lab.note("i adds 5 seeds / k removes 2 / v toggles slot 5")
        end,
        run = function()
            local used, capacity = bag_usage(BAG)
            lab.note(string.format(
                "bag %d/%d seeds=%d coins=%d",
                used,
                capacity,
                inv.count(BAG, "turnip_seed"),
                inv.count(BAG, "coin")
            ))
        end,
        read = function()
            local used, capacity = bag_usage(BAG)
            return {
                { label = "bag", value = string.format("%d/%d", used, capacity) },
                { label = "seeds", value = tostring(inv.count(BAG, "turnip_seed")) },
                { label = "cans", value = tostring(inv.count(BAG, "watering_can")) },
                { label = "coins", value = tostring(inv.count(BAG, "coin")) },
                { label = "slot1", value = slot_text(BAG, 1) },
                { label = "slot5", value = slot_text(BAG, 5) },
            }
        end,
        check = function(t)
            t:eq(inv.exists(BAG), true, "inv.create registered the bag")
            t:eq(inv.capacity(BAG), BAG_CAPACITY, "inv.capacity reads the create capacity")
            t:eq(inv.count(BAG, "turnip_seed"), 5, "inv.add seeded five seeds")
            t:eq(inv.add(BAG, "turnip_seed", 3), 0, "inv.add returns the leftover (none)")
            t:eq(inv.count(BAG, "turnip_seed"), 8, "stacking respects the item definition")
            t:eq(inv.remove(BAG, "turnip_seed", 2), 2, "inv.remove takes the requested count")
            t:eq(inv.count(BAG, "turnip_seed"), 6, "inv.count reads the remainder")
            local slot = inv.slot(BAG, 1)
            t:ok(slot ~= nil, "slot 1 holds the seed stack")
            if slot ~= nil then
                t:eq(slot.item, "turnip_seed", "slot 1 is the seed")
                t:eq(slot.count, 6, "slot 1 count matches inv.count")
            end
            local def = item.def("turnip_seed")
            t:ok(def ~= nil, "item.def returns the registered definition")
            if def ~= nil then
                t:eq(def.stack, 99, "item.def exposes the stack limit")
                t:eq(def.name, "Turnip Seed", "item.def exposes the display name")
            end
            t:eq(inv.get(BAG).capacity, BAG_CAPACITY, "inv.get reports the capacity")
        end,
    },
    {
        id = "bag_slots",
        title = "Bag slots",
        claim = "inv.get/slot 读整包与单格；inv.move/swap/set 是同一套搬运接口",
        api = { "inv.get", "inv.slot", "inv.set", "inv.swap", "inv.move", "inv.capacity", "inv.exists", "inv.count" },
        code = "inv.move(BAG,1,HOTBAR,1,nil); inv.swap(BAG,2,HOTBAR,2); inv.set(BAG,5,'coin',7)",
        enter = function()
            define_items()
            reset_bag()
            lab.note("b moves bag1 -> hotbar1 / g swaps bag2 <-> hotbar2 / v writes slot 5")
        end,
        run = function()
            local used = bag_usage(BAG)
            local hot = bag_usage(HOTBAR)
            lab.note(string.format("bag %d/%d | hotbar %d/%d", used, BAG_CAPACITY, hot, HOTBAR_SIZE))
        end,
        read = function()
            local used, capacity = bag_usage(BAG)
            local hot_used, hot_capacity = bag_usage(HOTBAR)
            return {
                { label = "bag", value = string.format("%d/%d", used, capacity) },
                { label = "hotbar", value = string.format("%d/%d", hot_used, hot_capacity) },
                { label = "bag1", value = slot_text(BAG, 1) },
                { label = "bag2", value = slot_text(BAG, 2) },
                { label = "bag5", value = slot_text(BAG, 5) },
                { label = "hotbar1", value = slot_text(HOTBAR, 1) },
                { label = "hotbar2", value = slot_text(HOTBAR, 2) },
            }
        end,
        check = function(t)
            local data = inv.get(BAG)
            t:eq(data.occupied, 3, "the seeded bag occupies three slots")
            t:eq(inv.capacity(HOTBAR), HOTBAR_SIZE, "inv.hotbar built a 9-slot hotbar")
            t:eq(inv.move(BAG, 1, HOTBAR, 1, nil), 5, "inv.move moves the whole stack")
            t:eq(inv.count(BAG, "turnip_seed"), 0, "the source stack is gone")
            t:eq(inv.count(HOTBAR, "turnip_seed"), 5, "the target stack arrived")
            inv.swap(BAG, 2, HOTBAR, 2)
            local swapped = inv.slot(HOTBAR, 2)
            t:ok(swapped ~= nil and swapped.item == "watering_can", "inv.swap moved the can to the hotbar")
            t:eq(inv.slot(BAG, 2), nil, "the bag slot is now empty")
            inv.set(BAG, 5, "coin", 7)
            local placed = inv.slot(BAG, 5)
            t:ok(placed ~= nil and placed.count == 7, "inv.set writes a stack")
            inv.set(BAG, 5, nil, nil)
            t:eq(inv.slot(BAG, 5), nil, "inv.set with nil clears the slot")
        end,
    },
    {
        id = "equipment",
        title = "Equipment",
        claim = "装备槽只是「槽名 → 物品 id」的账本：equip 写、equip_get 读单槽、equip_list 读整表",
        api = { "inv.create", "inv.add", "inv.equip", "inv.equip_get", "inv.equip_list", "inv.exists" },
        code = "inv.equip(BAG,'tool','hoe'); inv.equip_get(BAG,'tool'); inv.equip_list(BAG)",
        enter = function()
            define_items()
            reset_bag()
            lab.note("e toggles the tool slot / q toggles a trinket")
        end,
        run = function()
            local parts = {}
            for slot, item_id in pairs(inv.equip_list(BAG)) do
                parts[#parts + 1] = slot .. "=" .. item_id
            end
            lab.note(#parts .. " equipped: " .. table.concat(parts, " "))
        end,
        read = function()
            local rows = {
                { label = "tool", value = "-" },
                { label = "trinket", value = "-" },
            }
            if not inv.exists(BAG) then
                return rows
            end
            rows[1].value = dash(inv.equip_get(BAG, "tool"))
            rows[2].value = dash(inv.equip_get(BAG, "trinket"))
            local count = 0
            for _ in pairs(inv.equip_list(BAG)) do
                count = count + 1
            end
            rows[#rows + 1] = { label = "slots", value = tostring(count) }
            return rows
        end,
        check = function(t)
            t:eq(inv.equip_get(BAG, "tool"), "hoe", "reset_bag equipped the hoe")
            local list = inv.equip_list(BAG)
            t:eq(list.tool, "hoe", "equip_list exposes the tool slot")
            inv.equip(BAG, "tool", nil)
            t:eq(inv.equip_get(BAG, "tool"), nil, "inv.equip with nil clears the slot")
            inv.equip(BAG, "trinket", "charm")
            t:eq(inv.equip_get(BAG, "trinket"), "charm", "inv.equip writes an arbitrary slot")
            local count = 0
            for _ in pairs(inv.equip_list(BAG)) do
                count = count + 1
            end
            t:eq(count, 1, "equip_list lists exactly the filled slot")
            inv.equip(BAG, "tool", "hoe")
            t:eq(inv.equip_get(BAG, "tool"), "hoe", "the tool slot can be restored")
        end,
    },
    {
        id = "containers",
        title = "Containers",
        claim = "container.open 把实体变成第二个背包；返回的 container:<id> 就是 inv.move 的目标 id",
        api = { "container.open", "container.get", "container.close", "container.move", "inv.add", "inv.count", "inv.exists" },
        code = "key = container.open(chest, 8); container.move(chest,1,BAG,5); container.close(chest)",
        enter = function()
            define_items()
            reset_bag()
            open_chest()
            lab.note("o reopens the chest / c closes it / u chest->bag5 / l bag->chest")
        end,
        run = function()
            local key = chest_id ~= nil and container.get(chest_id) or nil
            lab.note(string.format(
                "chest=%s coins=%d | bag coins=%d",
                dash(key),
                key ~= nil and inv.count(key, "coin") or 0,
                inv.count(BAG, "coin")
            ))
        end,
        read = function()
            local key = chest_id ~= nil and container.get(chest_id) or nil
            local capacity = "-"
            if key ~= nil and inv.exists(key) then
                capacity = tostring(inv.capacity(key))
            end
            return {
                { label = "chest", value = dash(chest_id) },
                { label = "key", value = dash(key) },
                { label = "capacity", value = capacity },
                { label = "chest_coins", value = tostring(key ~= nil and inv.count(key, "coin") or 0) },
                { label = "bag_coins", value = tostring(inv.count(BAG, "coin")) },
            }
        end,
        check = function(t)
            t:ok(chest_id ~= nil, "the scene has a Chest entity")
            if chest_id == nil then
                return
            end
            local key = container.get(chest_id)
            t:eq(key, "container:" .. tostring(chest_id), "container.get returns the entity key")
            t:eq(inv.count(key, "coin"), 10, "the chest backpack holds the seeded coins")
            local bag_coins = inv.count(BAG, "coin")
            t:eq(inv.slot(BAG, 5), nil, "bag slot 5 starts empty")
            t:eq(container.move(chest_id, 1, BAG, 5), 10, "container.move moves the chest stack into bag slot 5")
            t:eq(inv.count(key, "coin"), 0, "the chest slot is now empty")
            t:eq(inv.count(BAG, "coin"), bag_coins + 10, "the bag received the coins")
            t:eq(container.close(chest_id), true, "container.close removes the backpack")
            t:eq(container.get(chest_id), nil, "container.get returns nil after close")
            t:ok(not inv.exists(key), "the container inventory id is gone")
            local reopened = container.open(chest_id, CHEST_CAPACITY)
            local reopened_check = container.get(chest_id)
            t:eq(reopened_check, reopened, "container.open rebuilds the backpack")
            inv.add(reopened, "coin", 10)
        end,
    },
    {
        id = "shop",
        title = "Shop",
        claim = "shop.set_catalog 是幂等覆盖；shop.price 立刻给新价，目录外的条目返回 nil",
        api = { "item.define", "item.def", "shop.set_catalog", "shop.price" },
        code = "shop.set_catalog(SHOP, catalog); shop.price(SHOP, 'turnip_seed')",
        enter = function()
            define_items()
            reset_shop()
            lab.note("x toggles full / half price catalog")
        end,
        run = function()
            local price = shop.price(SHOP, "turnip_seed")
            lab.note(string.format(
                "turnip_seed buy=%s sell=%s",
                price ~= nil and tostring(price.buy) or "nil",
                price ~= nil and tostring(price.sell) or "nil"
            ))
        end,
        read = function()
            local def = item.def("turnip_seed")
            local price = shop.price(SHOP, "turnip_seed")
            return {
                { label = "item", value = def ~= nil and def.name or "-" },
                { label = "stack", value = def ~= nil and tostring(def.stack) or "-" },
                { label = "buy", value = price ~= nil and tostring(price.buy) or "-" },
                { label = "sell", value = price ~= nil and tostring(price.sell) or "-" },
                { label = "catalog", value = catalog_sale and "sale" or "full" },
            }
        end,
        check = function(t)
            local def = item.def("turnip_seed")
            t:ok(def ~= nil, "item.def returns the registered definition")
            if def ~= nil then
                t:eq(def.buy, 20, "item.def exposes the catalog price")
                t:eq(def.stack, 99, "item.def exposes the stack limit")
            end
            local price = shop.price(SHOP, "turnip_seed")
            t:ok(price ~= nil, "shop.price returns the catalog entry")
            if price ~= nil then
                t:eq(price.buy, 20, "shop.price reads the full catalog")
                t:eq(price.sell, 10, "shop.price reads the sell price")
            end
            shop.set_catalog(SHOP, SALE_CATALOG)
            t:eq(shop.price(SHOP, "turnip_seed").buy, 10, "set_catalog overwrites the catalog")
            t:eq(shop.price(SHOP, "missing_item"), nil, "unknown items have no price")
            shop.set_catalog(SHOP, FULL_CATALOG)
            t:eq(shop.price(SHOP, "turnip_seed").buy, 20, "the full catalog can be restored")
        end,
    },
    {
        id = "interact",
        title = "Interact",
        claim = "interact.query 只挑带 kind/interact 标签的实体；prompt/clear_prompt/get_prompt 是引擎提示",
        api = { "interact.query", "interact.prompt", "interact.get_prompt", "interact.clear_prompt", "tag.set", "tag.get", "transform.get" },
        code = "hits = interact.query(x, y, 120); interact.prompt('E - chest'); interact.get_prompt()",
        enter = function()
            reset_world()
            radius_index = 1
            lab.note("c cycles radius 120/24 / q toggles the farmer role tag")
        end,
        run = function()
            local hits, prompt = refresh_prompt()
            lab.note(string.format("r=%.0f hits=%d prompt=%s", RADII[radius_index], #hits, dash(prompt)))
        end,
        read = function()
            local fx, fy = 144.0, 176.0
            local alive = farmer_id ~= nil and entity.is_alive(farmer_id)
            if alive then
                local here = transform.get(farmer_id)
                fx, fy = here.x, here.y
            end
            local hits = interact.query(fx, fy, RADII[radius_index])
            local role = nil
            if alive then
                role = tag.get(farmer_id, "role")
            end
            return {
                { label = "radius", value = string.format("%.0f", RADII[radius_index]) },
                { label = "hits", value = tostring(#hits) },
                { label = "prompt", value = dash(interact.get_prompt()) },
                { label = "farmer", value = string.format("%.0f,%.0f", fx, fy) },
                { label = "role", value = dash(role) },
            }
        end,
        check = function(t)
            t:ok(chest_id ~= nil and farmer_id ~= nil, "the scene has a Chest and a Farmer")
            if chest_id == nil or farmer_id == nil then
                return
            end
            local here = transform.get(farmer_id)
            local wide = interact.query(here.x, here.y, 120.0)
            t:ok(#wide >= 2, "chest and shopkeeper are candidates within 120 px")
            local narrow = interact.query(here.x, here.y, 24.0)
            t:eq(#narrow, 0, "a 24 px radius finds nobody")
            tag.set(chest_id, "kind", "")
            t:eq(tag.get(chest_id, "kind"), nil, "empty tag value removes the tag")
            tag.set(chest_id, "kind", "chest")
            t:eq(tag.get(chest_id, "kind"), "chest", "tag.set writes the tag back")
            interact.prompt("E - chest")
            t:eq(interact.get_prompt(), "E - chest", "interact.prompt is readable back")
            interact.clear_prompt()
            t:eq(interact.get_prompt(), nil, "interact.clear_prompt removes the prompt")
        end,
    },
    {
        id = "map_state",
        title = "Map state",
        claim = "map.schema 在 map.state 写入时校验；状态按地图名隔离，clear 只动指定名字",
        api = { "map.schema", "map.state", "map.state_clear", "map.set_spawn", "map.spawn", "map.current" },
        code = "map.schema(name, spec); map.state(name, {plots=3}); map.state_clear(name)",
        enter = function()
            reset_map_state()
            lab.note("s set_spawn / w write plots / d clear this map / a clear all")
        end,
        run = function()
            local state = map.state(MAP_STATE)
            local spawn = map.spawn()
            lab.note(string.format(
                "map=%s plots=%s spawn=%s,%s",
                dash(map.current()),
                state ~= nil and tostring(state.plots) or "nil",
                spawn ~= nil and string.format("%.0f", spawn.x) or "-",
                spawn ~= nil and string.format("%.0f", spawn.y) or "-"
            ))
        end,
        read = function()
            local state = map.state(MAP_STATE)
            local spawn = map.spawn()
            return {
                { label = "current", value = dash(map.current()) },
                { label = "plots", value = state ~= nil and tostring(state.plots) or "-" },
                { label = "sky", value = state ~= nil and tostring(state.sky) or "-" },
                { label = "spawn_x", value = spawn ~= nil and string.format("%.0f", spawn.x) or "-" },
                { label = "spawn_y", value = spawn ~= nil and string.format("%.0f", spawn.y) or "-" },
            }
        end,
        check = function(t)
            map.schema(MAP_STATE, MAP_SCHEMA)
            map.state(MAP_STATE, {
                version = 1,
                plots = 3,
                sky = "Sunny",
                notes = { owner = "Farmer" },
            })
            local state = map.state(MAP_STATE)
            t:eq(state.plots, 3, "map.state round-trips a declared integer")
            t:eq(state.sky, "Sunny", "map.state round-trips a declared string")
            local rejected = pcall(function()
                map.state(MAP_STATE, { version = "not-an-integer", plots = 1, sky = "Sunny" })
            end)
            t:ok(not rejected, "map.schema rejects an invalid write")
            t:eq(map.state_clear(MAP_STATE), 1, "map.state_clear(name) removes the named state")
            t:eq(map.state(MAP_STATE), nil, "the named state is gone")
            map.state(MAP_STATE, { version = 1, plots = 3, sky = "Sunny" })
            map.set_spawn(144.0, 176.0)
            local spawn = map.spawn()
            t:ok(spawn ~= nil, "map.spawn returns the stored spawn point")
            if spawn ~= nil then
                t:near(spawn.x, 144.0, 0.01, "spawn x round-trips")
                t:near(spawn.y, 176.0, 0.01, "spawn y round-trips")
            end
            t:ok(type(map.current()) == "string", "map.current reports a map id")
        end,
    },
    {
        id = "map_switch",
        title = "Map switch",
        claim = "map.goto / map.enter / map.go 只排队切图，不立即换场景；三个键都自跳回本场景",
        api = { "map.current", "map.spawn", 'map["goto"]', "map.enter", "map.go", "map.set_spawn" },
        code = "map[\"goto\"]('api_gameplay',144,176); map.enter('api_gameplay',176,176)",
        enter = function()
            reset_map_state()
            lab.note("g = map.goto / e = map.enter / j = map.go (all self-reload, on_create reruns)")
        end,
        run = function()
            local spawn = map.spawn()
            lab.note(string.format(
                "map=%s spawn=%s,%s | g / e / j queue a real switch",
                dash(map.current()),
                spawn ~= nil and string.format("%.0f", spawn.x) or "-",
                spawn ~= nil and string.format("%.0f", spawn.y) or "-"
            ))
        end,
        read = function()
            local spawn = map.spawn()
            return {
                { label = "current", value = dash(map.current()) },
                { label = "scene", value = dash(scene.reload()) },
                { label = "spawn_x", value = spawn ~= nil and string.format("%.0f", spawn.x) or "-" },
                { label = "spawn_y", value = spawn ~= nil and string.format("%.0f", spawn.y) or "-" },
                { label = "keys", value = "g / e / j" },
            }
        end,
        check = function(t)
            local current = map.current()
            t:ok(type(current) == "string" and #current > 0, "map.current reports a map id")
            t:eq(scene.reload(), MAP_HERE, "scene.reload returns the loaded scene name")
            local spawn = map.spawn()
            t:ok(spawn ~= nil, "map.spawn is readable before a queued switch")
            if spawn ~= nil then
                t:near(spawn.x, 144.0, 0.01, "the spawn point survives until a switch is queued")
                t:near(spawn.y, 176.0, 0.01, "the spawn point keeps its y")
            end
        end,
    },
}

-- ── 日循环钩子：day.sleep() 与自然跨午夜都会走到这里 ─────────────────────────
function M.on_day_end()
    hooks.day_end = hooks.day_end + 1
    log.info(string.format("api_gameplay on_day_end #%d day=%d", hooks.day_end, clock.now().day))
end

function M.on_day_start()
    hooks.day_start = hooks.day_start + 1
    log.info(string.format("api_gameplay on_day_start #%d day=%d", hooks.day_start, clock.now().day))
end

-- ── 生命周期 ────────────────────────────────────────────────────────────────
function M.on_create()
    -- 场景实体原本落在左栏下面：先摆进舞台，再开始分步演示。
    reset_world()
    define_items()
    reset_shop()
    lab.start({
        scene = "api_gameplay",
        title = "Gameplay",
        category = "Gameplay",
        steps = steps,
        -- SceneHud 在页眉位置（y=52），会压住 Lab 的 shell，隐藏它（stop 时恢复）。
        hide_scene_text = { "SceneHud" },
    })
    log.info(string.format("api_gameplay lab: %d steps, chest=%s", #steps, dash(chest_id)))
end

function M.update()
    local step = lab.step()
    local id = step and step.id or ""
    if id ~= last_step then
        last_step = id
        action = "-"
    end

    if id == "clock_views" then
        if input.just_pressed("t") then
            clock.set(5, 480)
            action = "clock.set(5, 480) -> day 5, 08:00"
        elseif input.just_pressed("n") then
            clock.add(60)
            action = "clock.add(60) -> " .. hhmm(clock.now())
        elseif input.just_pressed("p") then
            local paused = clock.now().paused
            clock.pause(not paused)
            action = string.format("clock.pause(%s)", tostring(not paused))
        end
    elseif id == "clock_advance" then
        if input.just_pressed("t") then
            clock.set(1, 0)
            advance = 0
            action = "clock.set(1, 0)"
        elseif input.just_pressed("n") then
            clock.add(60)
            advance = advance + 60
            action = "clock.add(60) -> " .. hhmm(clock.now())
        elseif input.just_pressed("s") then
            rate_index = rate_index % #RATES + 1
            clock.set_rate(RATES[rate_index])
            action = string.format("clock.set_rate(%.0f min/s)", RATES[rate_index])
        elseif input.just_pressed("p") then
            local paused = clock.now().paused
            clock.pause(not paused)
            action = string.format("clock.pause(%s)", tostring(not paused))
        end
    elseif id == "day_sleep" then
        if input.just_pressed("n") then
            local next_day = day.sleep()
            action = string.format(
                "day.sleep -> day %d (on_day_end=%d on_day_start=%d)",
                next_day,
                hooks.day_end,
                hooks.day_start
            )
        elseif input.just_pressed("m") then
            local next_day = clock.sleep_to_morning()
            action = string.format("clock.sleep_to_morning -> day %d (no hooks)", next_day)
        end
    elseif id == "calendar_config" then
        if input.just_pressed("y") then
            local ok = pcall(calendar.set, 12, 3, 2, 400)
            action = "calendar.set(12,3,2,400) ok=" .. tostring(ok)
        elseif input.just_pressed("u") then
            calendar.configure({ days_per_season = 7, seasons_per_year = 3 })
            action = "calendar.configure 7x3"
        elseif input.just_pressed("i") then
            calendar.configure({ days_per_season = 28, seasons_per_year = 4 })
            action = "calendar.configure 28x4"
        end
    elseif id == "inventory" then
        if input.just_pressed("i") then
            local leftover = inv.add(BAG, "turnip_seed", 5)
            action = string.format(
                "inv.add x5 leftover=%d count=%d",
                leftover,
                inv.count(BAG, "turnip_seed")
            )
        elseif input.just_pressed("k") then
            local removed = inv.remove(BAG, "turnip_seed", 2)
            action = string.format(
                "inv.remove x2 removed=%d left=%d",
                removed,
                inv.count(BAG, "turnip_seed")
            )
        elseif input.just_pressed("v") then
            if inv.slot(BAG, 5) ~= nil then
                inv.set(BAG, 5, nil, nil)
                action = "inv.set slot5 cleared"
            else
                inv.set(BAG, 5, "coin", 7)
                action = "inv.set slot5 coin x7"
            end
        end
    elseif id == "bag_slots" then
        if input.just_pressed("b") then
            local moved = inv.move(BAG, 1, HOTBAR, 1, nil)
            action = string.format("inv.move bag1 -> hotbar1 moved=%d", moved)
        elseif input.just_pressed("g") then
            inv.swap(BAG, 2, HOTBAR, 2)
            action = "inv.swap bag2 <-> hotbar2"
        elseif input.just_pressed("v") then
            inv.set(BAG, 5, "coin", 7)
            action = "inv.set bag slot5 coin x7"
        end
    elseif id == "equipment" then
        if input.just_pressed("e") then
            if inv.equip_get(BAG, "tool") ~= nil then
                inv.equip(BAG, "tool", nil)
                action = "inv.equip(tool, nil)"
            else
                inv.equip(BAG, "tool", "hoe")
                action = "inv.equip(tool, hoe)"
            end
        elseif input.just_pressed("q") then
            if inv.equip_get(BAG, "trinket") ~= nil then
                inv.equip(BAG, "trinket", nil)
                action = "inv.equip(trinket, nil)"
            else
                inv.equip(BAG, "trinket", "charm")
                action = "inv.equip(trinket, charm)"
            end
        end
    elseif id == "containers" then
        if input.just_pressed("o") then
            action = "container.open -> " .. dash(open_chest())
        elseif input.just_pressed("c") then
            if chest_id ~= nil then
                local removed = container.close(chest_id)
                action = "container.close removed=" .. tostring(removed)
            end
        elseif input.just_pressed("u") then
            if chest_id ~= nil then
                local moved = container.move(chest_id, 1, BAG, 5)
                action = string.format("container.move chest1 -> bag5 moved=%d", moved)
            end
        elseif input.just_pressed("l") then
            local key = chest_id ~= nil and container.get(chest_id) or nil
            if key ~= nil then
                local moved = inv.move(BAG, 1, key, 1, nil)
                action = string.format("inv.move bag1 -> %s moved=%d", key, moved)
            else
                action = "container.get -> nil (press o)"
            end
        end
    elseif id == "shop" then
        if input.just_pressed("x") then
            catalog_sale = not catalog_sale
            shop.set_catalog(SHOP, catalog_sale and SALE_CATALOG or FULL_CATALOG)
            local price = shop.price(SHOP, "turnip_seed")
            action = string.format(
                "shop.set_catalog(%s) buy=%s",
                catalog_sale and "sale" or "full",
                price ~= nil and tostring(price.buy) or "nil"
            )
        end
    elseif id == "interact" then
        if input.just_pressed("c") then
            radius_index = radius_index % #RADII + 1
            action = string.format("interact.query radius -> %.0f", RADII[radius_index])
        elseif input.just_pressed("q") then
            if farmer_id ~= nil and tag.get(farmer_id, "role") ~= nil then
                tag.set(farmer_id, "role", "")
                action = "tag.set(farmer role, '') -> removed"
            elseif farmer_id ~= nil then
                tag.set(farmer_id, "role", "player")
                action = "tag.set(farmer role, player)"
            end
        end
    elseif id == "map_state" then
        if input.just_pressed("s") then
            map.set_spawn(160, 200)
            action = "map.set_spawn(160, 200)"
        elseif input.just_pressed("w") then
            local state = map.state(MAP_STATE)
            local plots = (state ~= nil and tonumber(state.plots) or 0) + 1
            map.state(MAP_STATE, {
                version = 1,
                plots = plots,
                sky = "Rainy",
                notes = { owner = "Farmer" },
            })
            action = string.format("map.state plots=%d", plots)
        elseif input.just_pressed("d") then
            local removed = map.state_clear(MAP_STATE)
            action = string.format("map.state_clear(name) removed=%d", removed)
        elseif input.just_pressed("a") then
            local removed = map.state_clear()
            action = string.format("map.state_clear() removed=%d", removed)
        end
    elseif id == "map_switch" then
        if input.just_pressed("g") then
            local queued = map["goto"](MAP_HERE, 144, 176)
            action = "map.goto(self,144,176) queued=" .. tostring(queued)
        elseif input.just_pressed("e") then
            local queued = map.enter(MAP_HERE, 176, 176)
            action = "map.enter(self,176,176) queued=" .. tostring(queued)
        elseif input.just_pressed("j") then
            local queued = map.go(MAP_HERE)
            action = "map.go(self) queued=" .. tostring(queued)
        end
    end

    if action ~= "-" then
        lab.status(action)
    end
    return lab.tick()
end

function M.self_test()
    return lab.self_test()
end

function M.on_destroy()
    -- 本模块自己开过箱子背包、写过引擎交互提示：离开场景前收回。
    if chest_id ~= nil and entity.is_alive(chest_id) then
        container.close(chest_id)
    end
    interact.clear_prompt()
    lab.stop()
end

return M
