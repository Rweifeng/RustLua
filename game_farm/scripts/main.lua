local state = {
    player_name = "FieldFarmer",
    speed = 150,
    day = 1,
    clock = 6 * 60,
    energy = 100,
    money = 100,
    seeds = 9,
    crops = {},
    tilled = {},
    watered = {},
    crop_entities = {},
    message = "欢迎来到你的农场！",
    message_time = 4,
}

local player_id
local soil_positions = {
    { x = 480, y = 260 }, { x = 520, y = 260 }, { x = 560, y = 260 },
    { x = 480, y = 300 }, { x = 520, y = 300 }, { x = 560, y = 300 },
    { x = 480, y = 340 }, { x = 520, y = 340 }, { x = 560, y = 340 },
}

local function key(x, y)
    return tostring(x) .. ":" .. tostring(y)
end

local function say(text)
    state.message = text
    state.message_time = 4
end

local function nearest_soil()
    local p = transform.get(player_id)
    local best, distance = nil, 99999
    for _, spot in ipairs(soil_positions) do
        local dx, dy = spot.x - p.x, spot.y - p.y
        local d = math.sqrt(dx * dx + dy * dy)
        if d < distance and d <= 58 then
            best, distance = spot, d
        end
    end
    return best
end

local function spawn_crop(spot)
    local crop = entity.find("Crop_" .. key(spot.x, spot.y))
    if not crop then
        crop = entity.spawn("Crop_" .. key(spot.x, spot.y))
    end
    transform.set(crop, spot.x, spot.y)
    state.crop_entities[key(spot.x, spot.y)] = crop
    return crop
end

local function update_crop_visual(spot, stage)
    local crop = state.crop_entities[key(spot.x, spot.y)] or spawn_crop(spot)
    local size = 30 + stage * 8
    sprite.set(crop, "texture=assets/textures/crops/vegetables_sheet_scratchio.png;size=" .. size)
    transform.set(crop, spot.x, spot.y, 0, 1, 1)
end

local function spend_energy(amount)
    if state.energy < amount then
        say("体力不足，回家睡觉吧。")
        return false
    end
    state.energy = state.energy - amount
    state.clock = state.clock + 10
    return true
end

local function work()
    local spot = nearest_soil()
    if not spot then
        return
    end
    local id = key(spot.x, spot.y)
    if input.just_pressed("q") then
        if state.tilled[id] == true then
            say("这里已经翻过土了。")
        elseif spend_energy(5) then
            state.tilled[id] = true
            say("土地已翻好。按 E 播种。")
        end
    elseif input.just_pressed("e") then
        if not state.tilled[id] then
            say("先按 Q 翻土。")
        elseif state.crops[id] then
            say("这里已经种了作物。")
        elseif inv.count("player", "seed_tomato") <= 0 then
            say("没有种子了。")
        elseif spend_energy(3) then
            if inv.remove("player", "seed_tomato", 1) ~= 1 then
                say("没有种子了。")
                return
            end
            state.seeds = inv.count("player", "seed_tomato")
            state.crops[id] = { stage = 0, watered = false }
            update_crop_visual(spot, 0)
            say("播种成功。按 R 浇水。")
        end
    elseif input.just_pressed("r") then
        if not state.crops[id] then
            say("这里没有作物。")
        elseif state.watered[id] then
            say("今天已经浇过水了。")
        elseif spend_energy(2) then
            state.watered[id] = true
            state.crops[id].watered = true
            say("浇水成功。按 T 睡觉推进一天。")
        end
    elseif input.just_pressed("f") then
        local crop = state.crops[id]
        if crop and crop.stage >= 2 then
            state.crops[id] = nil
            state.watered[id] = nil
            state.money = state.money + 60
            inv.add("player", "tomato", 1)
            local crop_entity = state.crop_entities[id]
            if crop_entity and entity.is_alive(crop_entity) then
                entity.destroy(crop_entity)
            end
            state.crop_entities[id] = nil
            say("收获成功！获得 1 个番茄。")
        else
            say("作物还没有成熟。")
        end
    end
end

local function sleep()
    if not input.just_pressed("t") then
        return
    end
    state.day = clock.sleep_to_morning()
    state.clock = clock.now().minute_of_day
    state.energy = 100
    for _, spot in ipairs(soil_positions) do
        local id = key(spot.x, spot.y)
        local crop = state.crops[id]
        state.watered[id] = nil
        if crop then
            local was_watered = crop.watered
            crop.watered = nil
            if was_watered then
                crop.stage = math.min(crop.stage + 1, 2)
                update_crop_visual(spot, crop.stage)
            end
        end
    end
    say("新的一天开始了！浇水的作物成长了一阶段。")
end

local function visit_market()
    if not input.just_pressed("g") then
        return
    end
    local p = transform.get(player_id)
    if p.x >= 600 and p.x <= 760 and p.y >= 140 and p.y <= 240 then
        local sold = inv.remove("player", "tomato", inv.count("player", "tomato"))
        if sold > 0 then
            state.money = state.money + sold * 60
            say("在右侧集市卖出 " .. sold .. " 个番茄，获得 " .. sold * 60 .. " 金币。")
        else
            say("背包里没有番茄可卖。")
        end
    else
        say("请到右侧集市摊位附近按 G。")
    end
end

local function save_game()
    save.blob_write("farm", {
        day = state.day,
        clock = state.clock,
        energy = state.energy,
        money = state.money,
        seeds = inv.count("player", "seed_tomato"),
        tilled = state.tilled,
        watered = state.watered,
        crops = state.crops,
    })
    save.write("day", tostring(state.day))
    save.write("clock", tostring(state.clock))
    save.write("energy", tostring(state.energy))
    save.write("money", tostring(state.money))
    save.write("seeds", tostring(state.seeds))
    save.commit()
    say("已保存游戏。")
end

local function load_game()
    if not save.load() then
        say("没有找到存档。")
        return
    end
    local farm = save.blob_read("farm")
    if farm then
        state.day = farm.day or 1
        state.clock = farm.clock or (6 * 60)
        state.energy = farm.energy or 100
        state.money = farm.money or 0
        state.tilled = farm.tilled or {}
        state.watered = farm.watered or {}
        state.crops = farm.crops or {}
        state.seeds = farm.seeds or 0
    end
    state.day = math.max(1, tonumber(save.read("day")) or state.day)
    state.clock = tonumber(save.read("clock")) or state.clock
    state.energy = tonumber(save.read("energy")) or state.energy
    state.money = tonumber(save.read("money")) or state.money
    state.seeds = tonumber(save.read("seeds")) or state.seeds
    clock.set(state.day, state.clock)
    if not inv.exists("player") then
        inv.create("player", 24)
    end
    local current = inv.count("player", "seed_tomato")
    if state.seeds > current then
        inv.add("player", "seed_tomato", state.seeds - current)
    elseif state.seeds < current then
        inv.remove("player", "seed_tomato", current - state.seeds)
    end
    say("已读取存档。")
end

local function format_clock(minutes)
    return string.format("%02d:%02d", math.floor(minutes / 60), minutes % 60)
end

function on_create()
    player_id = entity.find(state.player_name)
    if not player_id then
        player_id = entity.spawn(state.player_name)
        transform.set(player_id, 316, 90)
    end
    item.define("seed_tomato", { name = "番茄种子", stack = 99, buy = 10, sell = 2, kind = "seed" })
    item.define("tomato", { name = "番茄", stack = 16, buy = 0, sell = 60, kind = "crop" })
    if not inv.exists("player") then
        inv.create("player", 24)
        inv.add("player", "seed_tomato", state.seeds)
    end
    local market = entity.find("MarketStand")
    if market then
        tag.set(market, "kind", "market")
    end
    local now = clock.now()
    state.day = now.day
    state.clock = now.minute_of_day
    say("欢迎来到你的农场！")
end

function update()
    if not player_id or not entity.is_alive(player_id) then
        return "player_missing"
    end

    local horizontal = input.axis("horizontal")
    local vertical = input.axis("vertical")
    if horizontal ~= 0 or vertical ~= 0 then
        local length = math.sqrt(horizontal * horizontal + vertical * vertical)
        transform.move(player_id, horizontal * 150 * time.delta() / length, vertical * 150 * time.delta() / length)
    end

    if input.just_pressed("f1") then
        save_game()
    elseif input.just_pressed("f2") then
        load_game()
    end

    work()
    sleep()
    visit_market()

    local p = transform.get(player_id)
    local now = clock.now()
    state.day = now.day
    state.clock = now.minute_of_day
    local hits = interact.query(p.x, p.y, 90)
    local near_market = false
    for _, hit in ipairs(hits) do
        if hit.kind == "market" then
            near_market = true
        end
    end
    if near_market then
        interact.prompt("按 G 出售背包里的番茄")
    else
        interact.clear_prompt()
    end
    ui.text_at("title", 24, 20, "GREENHOLLOW FARM", 2.0)
    ui.text_at("status", 24, 52, string.format("第 %d 天  %s  体力 %d/100  金币 %d  种子 %d", state.day, format_clock(state.clock), state.energy, state.money, inv.count("player", "seed_tomato")), 1.4)
    ui.text_at("help", 24, 82, "WASD 移动 | Q 翻土 | E 播种 | R 浇水 | F 收获 | T 睡觉 | G 集市 | F1/F2 存档", 1.05)
    ui.text_at("goal", 24, 112, "目标：种出成熟作物并在右侧集市出售。", 1.2)
    if state.message_time > 0 then
        ui.text_at("message", 24, 690, state.message, 1.45)
        state.message_time = math.max(0, state.message_time - time.delta())
    end
    ui.text_at("player_pos", 24, 640, string.format("位置 %.0f, %.0f", p.x, p.y), 1.0)
    ui.grid("hotbar", 24, 600, 8, 1, 34, 4)
    for i = 1, 8 do
        local slot = inv.slot("player", i)
        local def = slot and item.def(slot.item)
        if def and def.texture and def.texture ~= "" then
            ui.grid_cell("hotbar", i, def.texture, 1, 1, 1, 1)
        else
            ui.grid_cell("hotbar", i, nil, 0.18, 0.20, 0.24, 1)
        end
    end
    ui.text_at("hotbar_hint", 24, 638, "快捷栏（种子 / 番茄）", 0.95)
    return "farm_day_" .. tostring(state.day)
end
