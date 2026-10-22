-- api_gameplay：玩法层 API 一个场景看完 —— clock / calendar / day / item / inv /
-- container / shop / interact / map（外加 tag.get / tag.set：interact.query 的候选集就是
-- 按实体的 tag 筛出来的，所以这两个顺手一起演示）。
--
-- 这个演示想证明的四件事（读代码时按这个顺序看，屏幕上也是按这个顺序分行显示的）：
--
-- ① clock / calendar / day 是**同一份游戏时钟**的三个视角，不是三份状态：
--    clock.now() 给「第几天 + 当天已过的分钟数」，calendar.now() 把它换算成
--    「季节 / 季节内第几天 / 年」，day.now() 是日循环钩子的入口读数。按 SPACE
--    （clock.add）或 R（clock.set_rate）之后**后续帧的读数会继续变**——这就是
--    「时钟真的在被宿主推进」的证据。只打印一次常量的话，时钟不走的 bug 是看不出来的。
-- ② clock.sleep_to_morning() 与 day.sleep() 只差一件事：后者按顺序触发全局
--    on_day_end → on_day_start（本模块把两个回调的次数打在屏幕上，见 M.on_day_end /
--    M.on_day_start）。所以这两个键只放在按键分支里：headless 的自动步进没有输入，
--    不该被日循环回调打断。
-- ③ item.define 注册的定义是 inv 堆叠上限的**唯一来源**；inv.add 的返回值是「塞不进去的
--    余数」。container.open(实体 id, 容量) 把实体关联成第二个背包，键是
--    "container:<实体 id>"，而这个键就是 inv.move / inv.swap 的 inventory id ——
--    玩家背包与箱子背包走的是同一套接口，不需要第二套搬运代码。
-- ④ map 把「分地图持久状态」与「切场景」分成两件事：map.state / map.schema 随时可读写，
--    map.go / map.enter / map.goto 会真的换场景，所以只在按键分支里调用。
--
-- 硬规则：本文件顶层只定义常量与函数，不调用任何引擎 API（模块会被单独 load 做语法检查）。
local api = require("api_demo")

local M = {}

-- ── 常量 ───────────────────────────────────────────────────────────────────────
local BAG = "bag"                      -- inv.create 建的固定容量背包
local HOTBAR = "hotbar"                -- inv.hotbar 建的快捷栏（同一套 inv.* 接口）
local BAG_CAPACITY = 12
local HOTBAR_SIZE = 9
local CHEST_CAPACITY = 8
local SHOP = "farm_stand"              -- shop.set_catalog / shop.price 的目录名
local MAP_STATE = "api_gameplay_farm"  -- map.schema / map.state 的地图名
local MAP_HERE = "api_gameplay"        -- 切图目标之一：本场景自身（自跳，安全）
local MAP_OTHER = "main"               -- 切图目标之二：工程里另一个真实存在的场景
local SEASONS = { "Spring", "Summer", "Autumn", "Winter" }
local RATES = { 0.0, 2.0, 60.0, 240.0 }   -- clock.set_rate 的档位（每秒推进多少游戏分钟）
local RADII = { 120.0, 24.0 }             -- interact.query 的半径档位
local EQUIP_SLOTS = { "tool", "trinket" }
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

-- ── 运行期状态（on_create 里填）────────────────────────────────────────────────
local chest_id = nil            -- Chest 实体 id（container.* 的关联键）
local farmer_id = nil           -- Farmer 实体 id（interact 查询原点 + tag 演示用）
local radius_index = 1          -- RADII 的下标
local rate_index = 2            -- RATES 的下标（默认 2 游戏分钟/秒）
local hooks = { day_end = 0, day_start = 0 }   -- on_day_end / on_day_start 的触发次数
local last = "on_create"        -- 最近一次操作的结果（打在状态行的最后一段）
local schema_probe = "(not probed)"            -- map.schema 的拒绝实验结论
local catalog_sale = false      -- 当前用的是原价目录还是半价目录

-- ── 小工具（都是纯 Lua，不碰引擎 API）──────────────────────────────────────────

--- 时钟读数 → "HH:MM"（clock.now() 的 minute 是「当天已过的分钟数」，hour 已经除好了）
local function hhmm(now)
    if not now then
        return "--:--"
    end
    return string.format("%02d:%02d", now.hour or 0, (now.minute or 0) % 60)
end

local function season_name(index)
    return SEASONS[index] or ("S" .. tostring(index))
end

--- 截断过长的文本（引擎的错误信息带完整字段路径，屏幕上放不下）
local function clip(text, limit)
    local value = tostring(text)
    if #value <= limit then
        return value
    end
    return value:sub(1, limit) .. "..."
end

--- 背包概览，返回 `文本, 已占格数, 总格数`。
--- inv.get 给 {id, capacity, slots, occupied}；inv.slot 按 **1 基**槽位读单格，空格返回 nil。
local function bag_text(id, limit)
    if not inv.exists(id) then
        return id .. " (absent)", 0, 0
    end
    local data = inv.get(id)
    local parts = {}
    for index = 1, math.min(limit, data.capacity) do
        local slot = inv.slot(id, index)
        if slot then
            parts[#parts + 1] = string.format("[%d]%s x%d", index, slot.item, slot.count)
        else
            parts[#parts + 1] = string.format("[%d]-", index)
        end
    end
    local text = string.format("%s %d/%d %s", id, data.occupied, data.capacity, table.concat(parts, " "))
    return text, data.occupied, data.capacity
end

--- 装备槽概览：equip_list 一次读回「槽 → 物品 id」整表，equip_get 读单槽（没装备返回 nil）。
local function equip_text(id)
    if not inv.exists(id) then
        return "(absent)"
    end
    local parts = {}
    local count = 0
    for slot, item_id in pairs(inv.equip_list(id)) do
        count = count + 1
        parts[#parts + 1] = string.format("%s=%s", slot, item_id)
    end
    for _, name in ipairs(EQUIP_SLOTS) do
        if inv.equip_get(id, name) == nil then
            parts[#parts + 1] = name .. "=-"
        end
    end
    return string.format("%s (%d slot)", table.concat(parts, " "), count)
end

-- ── 进入场景：注册定义 / 建背包 / 关联箱子 / 打标签 / 注册地图状态校验器 ────────
function M.on_create()
    api.title("api_gameplay - clock / calendar / day / item / inv / container / shop / interact / map")
    api.hint(
        "CLOCK SPACE=clock.add(30m) R=clock.set_rate(0/2/60/240) P=clock.pause T=clock.set(5,480) "
            .. "M=clock.sleep_to_morning N=day.sleep Y=calendar.set(12,3,2,400)\n"
            .. "BAG I=inv.add K=inv.remove F=inv.move G=inv.swap V=inv.set(slot5) J=inv.equip Q=tag.set/get "
            .. "CHEST U=container.move L=inv.move(to chest) Z=container.close/open SHOP X=shop.price\n"
            .. "MAP 1=map.set_spawn 2=map.state(write) 3=map.state_clear(name) 4=map.state_clear() "
            .. "5=map.goto(here) 6=map.enter(here) 7=map.go(main) C=interact r=120/24 - " .. api.KEYS_BACK
    )

    -- ⓪ calendar.configure：季节长度与年季数是体裁参数，不是写死的 28×4。这里收紧成
    --    7 天/季、3 季/年，calendar.set 的第 8 天必须被拒绝；随后恢复默认值——下面 Y 键的
    --    calendar.set(12,3,2,400) 依赖 28 天/季。
    calendar.configure({ days_per_season = 7, seasons_per_year = 3 })
    local day8_rejected = pcall(calendar.set, 8, 1, 1, 0)
    calendar.configure({ days_per_season = 28, seasons_per_year = 4 })
    last = string.format(
        "calendar.configure 7x3: calendar.set(8,1,1,0) rejected=%s, restored 28x4",
        tostring(not day8_rejected)
    )

    -- ① item.define：注册物品定义。`stack` 是 inv.add 堆叠上限的唯一来源；
    --    没定义过的物品 id 也能 add，只是按引擎默认上限 99 堆叠。
    --    item.def 是这份注册表的读口（下面每帧都会读一次 turnip_seed 打到屏幕上）。
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
        description = "Currency. The wallet itself is Lua-side, the engine only prices items.",
        stack = 999,
        buy = 1,
        sell = 1,
        texture = "assets/textures/coin.png",
        kind = "currency",
    })

    -- ② shop.set_catalog：目录只是 {item_id = {buy, sell}}；买卖交易与钱包留在 Lua 玩法层，
    --    引擎侧只有这一份价格服务（shop.price 是它的读口）。
    shop.set_catalog(SHOP, FULL_CATALOG)
    catalog_sale = false

    -- ③ inv.create / inv.hotbar：两个独立命名的背包，接口完全一样（hotbar 只是默认 9 格的 create）。
    inv.create(BAG, BAG_CAPACITY)
    inv.hotbar(HOTBAR, HOTBAR_SIZE)
    inv.add(BAG, "turnip_seed", 5)   -- 返回值 = 没塞进去的余数（这里 stack=99，必然是 0）
    inv.add(BAG, "watering_can", 1)
    inv.add(BAG, "coin", 12)
    inv.equip(BAG, "tool", "hoe")    -- 装备槽骨架：只记物品 id，属性效果留给玩法层

    -- ④ container.open：把 Chest 实体关联成第二个背包，并把 container.get 读回的键
    --    直接当 inv 的 id 用一次 —— 证明两种背包是同一套接口。
    chest_id = entity.find("Chest")
    farmer_id = entity.find("Farmer")
    if chest_id then
        container.open(chest_id, CHEST_CAPACITY)
        local chest_key = container.get(chest_id)
        if chest_key then
            inv.add(chest_key, "coin", 10)
        end
        -- tag：interact.query 只挑带 "interact" 或 "kind" 标签的实体，并按 priority 参与排序
        tag.set(chest_id, "kind", "chest")
        tag.set(chest_id, "interact", "open")
        tag.set(chest_id, "priority", "5")
    end
    local shopkeeper_id = entity.find("Shopkeeper")
    if shopkeeper_id then
        tag.set(shopkeeper_id, "kind", "shop")
        tag.set(shopkeeper_id, "interact", "trade")
        tag.set(shopkeeper_id, "priority", "1")
    end
    if farmer_id then
        -- role 不是 interact 认的两个键之一，所以农夫自己不会出现在交互候选里
        tag.set(farmer_id, "role", "player")
    end

    -- ⑤ map.schema + map.state：注册分地图状态的轻量校验器。
    --    spec 形态见 docs/18 与 core/data_schema.rs：$kind = object/array/map，
    --    叶子是 "integer"/"number"/"string"/"boolean"/"any"。
    --    注册之后 map.state 的**写入**先校验，错误定位到地图名与字段 —— 所以下面故意
    --    写一份非法状态并用 pcall 断言「确实被拒绝」，把结论打到屏幕上（校验器不是摆设）。
    map.schema(MAP_STATE, {
        ["$kind"] = "object",
        fields = {
            version = "integer",
            plots = "integer",
            sky = "string",
            notes = { ["$kind"] = "map", values = "any" },
        },
        required = { "version", "plots", "sky" },
    })
    map.state(MAP_STATE, {
        version = 1,
        plots = 3,
        sky = "Sunny",
        notes = { owner = "Farmer", season = 1 },
    })
    local accepted, reason = pcall(function()
        map.state(MAP_STATE, { version = "not-an-integer", plots = 1, sky = "Sunny" })
    end)
    if accepted then
        schema_probe = "ACCEPTED (bug!)"
    else
        schema_probe = "rejected"
        log.info("map.schema rejected the invalid write: " .. clip(reason, 160))
    end

    -- ⑥ map.set_spawn：map.spawn() 的读口；map.goto / map.enter 带坐标时会覆盖它。
    map.set_spawn(144, 176)

    -- ⑦ 时钟：引擎默认 rate = 0（时钟是**冻结**的，每一步都由 set_rate 决定），
    --    所以先显式设一次，让「时间自己在走」这件事在本场景里直接可见；
    --    之后按 R 就在 0 / 2 / 60 / 240（游戏分钟每秒）之间切换。
    clock.set_rate(RATES[rate_index])

    last = "on_create: items/bag/hotbar/chest/tags/map-state/clock ready"
    log.info(string.format(
        "api_gameplay: bag=%d hotbar=%d chest=%s map=%s",
        BAG_CAPACITY, HOTBAR_SIZE, tostring(chest_id), tostring(map.current())
    ))
end

-- ── 每帧 ───────────────────────────────────────────────────────────────────────
function M.update()
    -- ══ ① 输入 → 改状态（所有会改变后续帧读数的东西都在这里）══════════════════

    -- 时钟三件套：add / set_rate / pause 的效果**看得见地延续到后续帧**，这是本演示的重点。
    if input.just_pressed("space") then
        clock.add(30)
        last = string.format("clock.add(30) -> %s (day %s)", hhmm(clock.now()), tostring(clock.now().day))
    elseif input.just_pressed("keyr") then
        rate_index = rate_index % #RATES + 1
        clock.set_rate(RATES[rate_index])
        last = string.format("clock.set_rate(%.0f min/s)", RATES[rate_index])
    elseif input.just_pressed("keyp") then
        local paused = clock.now().paused
        clock.pause(not paused)
        last = string.format("clock.pause(%s)", tostring(not paused))
    elseif input.just_pressed("keyt") then
        clock.set(5, 480)
        last = "clock.set(5,480) -> day 5, 08:00"
    elseif input.just_pressed("keym") then
        -- sleep_to_morning **不**触发日循环钩子（对比下面的 day.sleep）
        local day_number = clock.sleep_to_morning()
        last = string.format("clock.sleep_to_morning -> day %d (no hooks fired)", day_number)
    elseif input.just_pressed("keyn") then
        -- day.sleep() = on_day_end → 次日 06:00 → on_day_start，所以只允许按键触发
        local day_number = day.sleep()
        last = string.format(
            "day.sleep -> day %d, on_day_end=%d on_day_start=%d",
            day_number, hooks.day_end, hooks.day_start
        )
    elseif input.just_pressed("keyy") then
        calendar.set(12, 3, 2, 400)   -- 第 2 年 秋（第 3 季）第 12 天，当天第 400 分钟
        last = "calendar.set(day 12, season 3, year 2, minute 400)"
    end

    -- 背包：每条 inv 接口都真的被调用一次，返回值本身就是「发生了多少」。
    if input.just_pressed("keyi") then
        local leftover = inv.add(BAG, "turnip_seed", 5)
        last = string.format("inv.add(turnip_seed x5) leftover=%d count=%d", leftover, inv.count(BAG, "turnip_seed"))
    elseif input.just_pressed("keyk") then
        local removed = inv.remove(BAG, "turnip_seed", 2)
        last = string.format("inv.remove(turnip_seed x2) removed=%d left=%d", removed, inv.count(BAG, "turnip_seed"))
    elseif input.just_pressed("keyf") then
        -- inv.move(from_id, from_index, to_id, to_index, count)；count=nil = 全搬
        local moved = inv.move(BAG, 1, HOTBAR, 1, nil)
        last = string.format("inv.move(bag1 -> hotbar1) moved=%d", moved)
    elseif input.just_pressed("keyg") then
        -- inv.swap 是**整格对调**：目标格里的东西会回到来源格，不会丢
        inv.swap(BAG, 2, HOTBAR, 2)
        last = "inv.swap(bag2 <-> hotbar2)"
    elseif input.just_pressed("keyv") then
        -- inv.set(id, index, item_id, count)：数量为 nil（或 <=0）等价于「清空这一格」
        if inv.slot(BAG, 5) then
            inv.set(BAG, 5, nil, nil)
            last = "inv.set(bag slot5, nil, nil) -> cleared"
        else
            inv.set(BAG, 5, "watering_can", 1)
            last = "inv.set(bag slot5, watering_can x1)"
        end
    elseif input.just_pressed("keyj") then
        -- 装备槽骨架：inv.equip(id, slot, item_id)，item_id=nil 清空该槽
        if inv.equip_get(BAG, "tool") then
            inv.equip(BAG, "tool", nil)
            last = "inv.equip(bag, tool, nil) -> slot cleared"
        else
            inv.equip(BAG, "tool", "hoe")
            last = "inv.equip(bag, tool, hoe)"
        end
    elseif input.just_pressed("keyq") then
        -- tag.set 的**空 value = 删除标签**，所以这个键是「有/无」的切换
        if farmer_id and tag.get(farmer_id, "role") then
            tag.set(farmer_id, "role", "")
            last = "tag.set(farmer role, '') -> tag removed"
        elseif farmer_id then
            tag.set(farmer_id, "role", "player")
            last = "tag.set(farmer role, player)"
        end
    end

    -- 箱子与商店
    if input.just_pressed("keyu") then
        if chest_id then
            -- container.move(entity_id, from_index, to_id, to_index)：来源永远是箱子
            local moved = container.move(chest_id, 1, BAG, 1)
            last = string.format("container.move(chest1 -> bag1) moved=%d", moved)
        end
    elseif input.just_pressed("keyl") then
        local chest_key = chest_id and container.get(chest_id) or nil
        if chest_key then
            -- container.get 返回的 "container:<id>" 就是 inv.move 的目标 id
            local moved = inv.move(BAG, 1, chest_key, 1, nil)
            last = string.format("inv.move(bag1 -> %s) moved=%d", chest_key, moved)
        else
            last = "inv.move skipped: container.get -> nil (press Z to open)"
        end
    elseif input.just_pressed("keyz") then
        if chest_id and container.get(chest_id) then
            -- close 把 "container:<id>" 整个背包从注册表里删掉（里面的东西一起没了）
            container.close(chest_id)
            last = "container.close(chest) -> chest backpack dropped"
        elseif chest_id then
            container.open(chest_id, CHEST_CAPACITY)
            last = string.format("container.open(chest, %d) -> new empty backpack", CHEST_CAPACITY)
        end
    elseif input.just_pressed("keyx") then
        -- set_catalog 是幂等覆盖：换一份目录，shop.price 立刻给新价
        catalog_sale = not catalog_sale
        shop.set_catalog(SHOP, catalog_sale and SALE_CATALOG or FULL_CATALOG)
        local price = shop.price(SHOP, "turnip_seed")
        last = string.format(
            "shop.set_catalog(%s) -> turnip_seed buy=%s",
            catalog_sale and "sale" or "full", price and tostring(price.buy) or "nil"
        )
    end

    -- 地图状态与切图。**切场景只允许出现在这一块**：headless 的自动步进没有输入。
    if input.just_pressed("digit1") then
        map.set_spawn(160, 200)
        last = "map.set_spawn(160,200)"
    elseif input.just_pressed("digit2") then
        local saved = map.state(MAP_STATE)
        local plots = (saved and tonumber(saved.plots) or 0) + 1
        local sky = (plots % 2 == 0) and "Rainy" or "Sunny"
        local ok, reason = pcall(function()
            map.state(MAP_STATE, {
                version = 1,
                plots = plots,
                sky = sky,
                notes = { owner = "Farmer", last_action = "till" },
            })
        end)
        if ok then
            last = string.format("map.state(write) plots=%d sky=%s", plots, sky)
        else
            last = "map.state(write) rejected: " .. clip(reason, 72)
        end
    elseif input.just_pressed("digit3") then
        local removed = map.state_clear(MAP_STATE)   -- 只清这一张地图，返回删除条数
        last = string.format("map.state_clear(%s) removed=%d", MAP_STATE, removed)
    elseif input.just_pressed("digit4") then
        local removed = map.state_clear()            -- 无参 = 清掉全部 __mapstate:*（开新游戏用）
        last = string.format("map.state_clear() removed=%d", removed)
    elseif input.just_pressed("digit5") then
        -- 注意：`goto` 是 **Lua 保留字**，`map.goto(...)` 这种写法直接编译不过，
        -- 只能写成 map["goto"](...)（`examples/game_jhyt/scripts/world/map_manager.lua`
        -- 同样是这么调的）。引擎侧的 API 名字仍然是 map.goto，语义见 core/script.rs。
        local queued = map["goto"](MAP_HERE, 144, 176)   -- 自跳：本场景重载，on_create 会重跑
        last = string.format("map.goto(%s,144,176) queued=%s", MAP_HERE, tostring(queued))
    elseif input.just_pressed("digit6") then
        local queued = map.enter(MAP_HERE, 176, 176)
        last = string.format("map.enter(%s,176,176) queued=%s", MAP_HERE, tostring(queued))
    elseif input.just_pressed("digit7") then
        local queued = map.go(MAP_OTHER)
        last = string.format("map.go(%s) queued=%s", MAP_OTHER, tostring(queued))
    elseif input.just_pressed("keyc") then
        radius_index = radius_index % #RADII + 1
        last = string.format("interact.query radius -> %.0f", RADII[radius_index])
    end

    -- ══ ② 读回本帧事实（全部现场读，不缓存 —— 上面改过的东西必须立刻看见）══════
    local now = clock.now()
    local cal = calendar.now()
    local today = day.now()
    local bag, bag_used, bag_cap = bag_text(BAG, 6)
    local hotbar = bag_text(HOTBAR, 3)
    local equip = equip_text(BAG)
    -- inv.capacity 是「固定容量背包」的容量读口（create / hotbar 之后就不再变）；
    -- 它和 inv.get 里的 capacity 是同一个值，这里故意两条路都走一遍给读者看。
    local bag_capacity = inv.capacity(BAG)
    local hotbar_capacity = inv.capacity(HOTBAR)

    -- interact.query(x, y, radius)：按距离升序（同距离按 priority）给出候选实体。
    -- 查询原点用农夫自己的 Transform，这样「谁在附近」是真的按世界坐标算的。
    local here = farmer_id and transform.get(farmer_id) or nil
    local radius = RADII[radius_index]
    local hits = interact.query(here and here.x or 144.0, here and here.y or 176.0, radius)
    local nearest = hits[1]
    if nearest then
        -- interact.prompt 写的是**引擎侧**的交互提示文本（左上角 y=48），不是我们自己画的
        interact.prompt(string.format("E - %s #%s", nearest.kind, tostring(nearest.id)))
    else
        -- 半径 24 时没人进圈 → 走 clear_prompt 分支（按 C 切换半径就能看到提示被清掉）
        interact.clear_prompt()
    end
    local prompt = interact.get_prompt()

    local turnip = item.def("turnip_seed")        -- 定义注册表是显示名/价格的唯一真源
    local price = shop.price(SHOP, "turnip_seed") -- {buy, sell} 或 nil（目录/条目不存在）
    local current_map = map.current()
    local spawn = map.spawn()
    local farm_state = map.state(MAP_STATE)
    local chest_key = chest_id and container.get(chest_id) or nil

    -- ══ ③ 上屏：api.status 是头条事实（含背包每一格），下面按模块分列细节 ══
    api.status(string.format(
        "T=%s day=%s rate=%.0f paused=%s | %s | last: %s",
        hhmm(now), tostring(now.day), now.rate, tostring(now.paused), bag, last
    ))
    ui.text_at("gameplay_clock", 24, 508, string.format(
        "CLOCK  hour=%s minute_of_day=%s rate=%.0f paused=%s  ||  DAY  day=%s minute=%s  ||  calendar.now absolute_day=%s season=%s(%s) day=%s year=%s",
        tostring(now.hour), tostring(now.minute_of_day), now.rate, tostring(now.paused),
        tostring(today.day), tostring(today.minute),
        tostring(cal.absolute_day), tostring(cal.season), season_name(cal.season),
        tostring(cal.day), tostring(cal.year)
    ), 1.0)
    ui.text_at("gameplay_item", 24, 526, string.format(
        "ITEM  item.def('turnip_seed') -> name=%s stack=%s buy=%s sell=%s kind=%s  ||  SHOP  %s turnip_seed buy=%s sell=%s catalog=%s",
        turnip and turnip.name or "?", tostring(turnip and turnip.stack),
        tostring(turnip and turnip.buy), tostring(turnip and turnip.sell), tostring(turnip and turnip.kind),
        SHOP, price and tostring(price.buy) or "nil", price and tostring(price.sell) or "nil",
        catalog_sale and "sale" or "full"
    ), 1.0)
    ui.text_at("gameplay_inv", 24, 544, string.format(
        "INV  %s inv.capacity=%s  ||  HOTBAR %s inv.capacity=%s",
        bag, tostring(bag_capacity), hotbar, tostring(hotbar_capacity)
    ), 1.0)
    ui.text_at("gameplay_equip", 24, 562, string.format(
        "EQUIP(inv.equip_list / inv.equip_get)  %s  ||  TAG role=%s  ||  day hooks on_day_end=%d on_day_start=%d  ||  map.schema on an invalid write: %s",
        equip, tostring(farmer_id and tag.get(farmer_id, "role") or "nil"),
        hooks.day_end, hooks.day_start, schema_probe
    ), 1.0)
    ui.text_at("gameplay_map", 24, 580, string.format(
        "MAP  map.current=%s map.spawn=(%s,%s)  ||  map.state('%s') plots=%s sky=%s (写入前经 map.schema 校验)",
        tostring(current_map),
        spawn and string.format("%.0f", spawn.x) or "-", spawn and string.format("%.0f", spawn.y) or "-",
        MAP_STATE, farm_state and tostring(farm_state.plots) or "nil", farm_state and tostring(farm_state.sky) or "nil"
    ), 1.0)
    ui.text_at("gameplay_interact", 24, 598, string.format(
        "CHEST  %s  ||  INTERACT  interact.query(r=%.0f) hits=%d  interact.get_prompt()=%s",
        chest_key and bag_text(chest_key, 3) or "closed (container.get -> nil)",
        radius, #hits, prompt and ("'" .. prompt .. "'") or "nil"
    ), 1.0)

    -- 返回值进引擎日志（`run_script_steps` / 编辑器 Play 的日志页都能看到）
    return string.format(
        "api_gameplay_ok t=%s day=%s bag=%d/%d chest=%s map=%s",
        hhmm(now), tostring(now.day), bag_used, bag_cap,
        chest_key and "open" or "closed", tostring(current_map)
    )
end

-- ── 日循环钩子：day.sleep() 与「时钟自然跨午夜」都会走到这里（这就是 day 模块的演示）──
function M.on_day_end()
    hooks.day_end = hooks.day_end + 1
    last = string.format("on_day_end #%d fired", hooks.day_end)
    log.info(string.format("api_gameplay: on_day_end #%d (day %s)", hooks.day_end, tostring(clock.now().day)))
end

function M.on_day_start()
    hooks.day_start = hooks.day_start + 1
    last = string.format("on_day_start #%d fired (new day)", hooks.day_start)
    log.info(string.format("api_gameplay: on_day_start #%d (day %s)", hooks.day_start, tostring(clock.now().day)))
end

-- ── 离开场景 ───────────────────────────────────────────────────────────────────
function M.on_destroy()
    -- 只收起本模块自己画的那六行细节；api_title / api_status / api_hint 由 api.clear() 统一处理。
    -- （`ui.hide` 对文本与同 id 面板都生效；`ui.clear` 只删面板/图片/网格。）
    ui.hide("gameplay_clock")
    ui.hide("gameplay_item")
    ui.hide("gameplay_inv")
    ui.hide("gameplay_equip")
    ui.hide("gameplay_map")
    ui.hide("gameplay_interact")
end

return M
