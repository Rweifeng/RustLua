-- 3D 推箱子：关卡逻辑 + 引擎 3D 能力探针。
--
-- 两件事在同一份脚本里：
--   ① 玩法：w/a/s/d 走一格、推箱子，箱子全上目标点即胜；x 重开；c 开关 3D 碰撞体线框。
--   ② 探针：`on_create` 里把 3D 侧的 Lua API 逐条真调一遍，逐条记录"通/不通/报错原文"，
--      再探一批**期望存在但引擎没有**的 3D 接口（材质、环境、图元、点光、雾）。
--      结论既走 `log.info` 进控制台（编辑器 MCP 的 read_console 能读到），
--      也能被 `eval_lua_readonly` 用 `require("sokoban").report()` 直接取回。
--
-- 关卡表与 scenes/main.scene 是同一份数据的两种形态（后者由 tools/gen_level_scene.mjs 生成），
-- 每个格子按 `Wall_c_r` / `Goal_c_r` / `Crate_c_r` 的实体名对账，加载不上一律记进探针。
local M = {}

local LEVEL = { "#######", "# ... #", "# $$$ #", "#  @  #", "#######" }
local COLS, ROWS = 7, 5
local HALF_X, HALF_Z = (COLS + 1) / 2, (ROWS + 1) / 2
local CRATE_Y, PLAYER_Y = 0.5, 0.6

local function key(c, r) return r * 1000 + c end
local function world(c, r) return c - HALF_X, r - HALF_Z end
local function cell_name(kind, c, r) return string.format("%s_%d_%d", kind, c - 1, r - 1) end

-- ── 探针记录 ────────────────────────────────────────────────────────────────────
M.probe = {}
M.absent = {}

local function note(kind, label, detail)
    M.probe[#M.probe + 1] = { kind = kind, label = label, detail = detail or "" }
    log.info(string.format("PROBE %s %s %s", kind, label, detail or ""))
end

--- 真调一次 API：成功记 ok + 返回值摘要，失败记 err + 报错原文。
local function try(label, fn)
    local ok, value = pcall(fn)
    if ok then
        note("ok", label, tostring(value))
    else
        note("err", label, tostring(value))
    end
    return ok, value
end

--- 探测一个不存在的接口：不报错，只是把它列为缺口（结论要可复现，不能靠人眼判断）。
local function missing(label)
    if rawget(_G, label) == nil then
        M.absent[#M.absent + 1] = label
        note("absent", label, "no global table")
    else
        note("present", label, "exists")
    end
end

-- ── 关卡状态 ────────────────────────────────────────────────────────────────────
M.walls = {}
M.goals = {}
M.boxes = {}
M.entities = {}
M.player = { c = 0, r = 0, id = nil }
M.moves = 0
M.pushes = 0
M.won = false

local function place(id, x, y, z)
    local t = transform3d.get(id)
    transform3d.move(id, x - t.x, y - t.y, z - t.z)
    pcall(function() physics3d.set_position(id, x, y, z) end)
end

local function lookup(name)
    local id = entity.find(name)
    if id and entity.is_alive(id) then
        return id
    end
    return nil
end

function M.load()
    M.walls, M.goals, M.boxes = {}, {}, {}
    local missing_cells = 0
    for r = 1, ROWS do
        for c = 1, COLS do
            local ch = string.sub(LEVEL[r], c, c)
            if ch == "#" then
                M.walls[key(c, r)] = true
            elseif ch == "." then
                local id = lookup(cell_name("Goal", c, r))
                if id then M.goals[key(c, r)] = id else missing_cells = missing_cells + 1 end
            elseif ch == "$" then
                local id = lookup(cell_name("Crate", c, r))
                if id then
                    M.boxes[key(c, r)] = id
                    M.entities[id] = { c = c, r = r, y = CRATE_Y }
                else
                    missing_cells = missing_cells + 1
                end
            elseif ch == "@" then
                local id = lookup("Player")
                if id then
                    M.player = { c = c, r = r, id = id }
                    M.entities[id] = { c = c, r = r, y = PLAYER_Y }
                else
                    missing_cells = missing_cells + 1
                end
            end
        end
    end
    M.moves, M.pushes, M.won = 0, 0, false
    return missing_cells
end

function M.solved()
    for k in pairs(M.goals) do
        if not M.boxes[k] then
            return false
        end
    end
    return true
end

local function step(dc, dr)
    if M.won then
        return false
    end
    local tc, tr = M.player.c + dc, M.player.r + dr
    if M.walls[key(tc, tr)] then
        return false
    end
    local tk = key(tc, tr)
    local box = M.boxes[tk]
    if box then
        local bc, br = tc + dc, tr + dr
        local bk = key(bc, br)
        if M.walls[bk] or M.boxes[bk] then
            return false
        end
        M.boxes[tk], M.boxes[bk] = nil, box
        M.entities[box].c, M.entities[box].r = bc, br
        local bx, bz = world(bc, br)
        place(box, bx, CRATE_Y, bz)
        M.pushes = M.pushes + 1
        event.emit("sokoban.push", { from = { c = tc, r = tr }, to = { c = bc, r = br } })
    end
    M.player.c, M.player.r = tc, tr
    local px, pz = world(tc, tr)
    place(M.player.id, px, PLAYER_Y, pz)
    M.moves = M.moves + 1
    if M.solved() then
        M.won = true
        event.emit("sokoban.win", { moves = M.moves, pushes = M.pushes })
    end
    return true
end

function M.reset()
    for k, id in pairs(M.boxes) do
        local e = M.entities[id]
        -- 箱子回到关卡表里的初始格：从场景实体名反推（Crate_c_r）。
        for r = 1, ROWS do
            for c = 1, COLS do
                if string.sub(LEVEL[r], c, c) == "$" and lookup(cell_name("Crate", c, r)) == id then
                    M.boxes[k] = nil
                    M.boxes[key(c, r)] = id
                    e.c, e.r = c, r
                    local x, z = world(c, r)
                    place(id, x, CRATE_Y, z)
                end
            end
        end
    end
    M.load()
    note("ok", "reset", "moves=0")
end

-- ── 能力探针 ────────────────────────────────────────────────────────────────────
--- 相机侧的探针必须晚于第一帧：投影链要用宿主回填的 viewport，
--- `on_create` 时还没有渲染过任何一帧，`camera3d.*` 会直接报
--- "needs one rendered frame first"（这条本身就是被测出来的时序约束）。
function M.probe_camera()
    try("camera3d.get", function()
        local c = camera3d.get()
        return string.format("pos=%.2f,%.2f,%.2f fov=%.1f vp=%dx%d", c.px, c.py, c.pz, c.fov, c.width, c.height)
    end)
    try("camera3d.ray", function()
        local r = camera3d.ray(640, 360)
        return string.format("o=%.2f,%.2f,%.2f d=%.2f,%.2f,%.2f", r.ox, r.oy, r.oz, r.dx, r.dy, r.dz)
    end)
    try("camera3d.mouse_ray", function()
        local r = camera3d.mouse_ray()
        return string.format("o=%.2f d=%.2f,%.2f,%.2f", r.oy, r.dx, r.dy, r.dz)
    end)
    try("camera3d.pick_plane", function()
        local p = camera3d.pick_plane(640, 360, 0)
        if not p then return "nil" end
        return string.format("hit=%.2f,%.2f,%.2f", p.x, p.y, p.z)
    end)
    try("camera3d.world_to_screen", function()
        local s = camera3d.world_to_screen(0, 0.6, 0)
        if not s then return "nil" end
        return string.format("%.1f,%.1f visible=%s", s.x, s.y, tostring(s.visible))
    end)
    try("camera3d.mouse_ray+physics3d.raycast", function()
        local r = camera3d.mouse_ray()
        local hit = physics3d.raycast(r.ox, r.oy, r.oz, r.dx, r.dy, r.dz, 60)
        M.last_pick = hit and string.format("ray-pick hit entity %s", tostring(hit)) or "ray hit nothing"
        return M.last_pick
    end)
    M.summarize()
end

function M.probe3d()
    try("app.platform", function() return app.platform() end)

    local pid = M.player.id
    try("transform3d.get", function()
        local t = transform3d.get(pid)
        return string.format("%.2f,%.2f,%.2f q=%.3f s=%.2f", t.x, t.y, t.z, t.rw, t.sx)
    end)
    try("transform3d.world", function()
        local t = transform3d.world(pid)
        return string.format("%.2f,%.2f,%.2f", t.x, t.y, t.z)
    end)
    try("transform3d.move", function()
        transform3d.move(pid, 0, 0.001, 0)
        transform3d.move(pid, 0, -0.001, 0)
        return "reverted"
    end)
    try("mesh.get(player)", function() return mesh.get(pid) end)

    local sun = lookup("Sun")
    try("light.get", function()
        if not sun then return "no Sun entity" end
        local l = light.get(sun)
        return string.format("dir=%.2f,%.2f,%.2f i=%.2f", l.dx, l.dy, l.dz, l.intensity)
    end)
    try("light.set_active", function()
        if not sun then return "no Sun entity" end
        light.set_active(sun, false)
        light.set_active(sun, true)
        return "toggled"
    end)

    try("physics3d.raycast", function()
        local x, z = world(M.player.c, M.player.r)
        local hit = physics3d.raycast(x, 5, z, 0, -1, 0, 12)
        if not hit then return "miss" end
        return string.format("hit entity=%s", tostring(hit))
    end)
    try("physics3d.overlap", function()
        local x, z = world(M.player.c, M.player.r)
        local ids = physics3d.overlap(x, 0.6, z, 1.2)
        return string.format("count=%d", #ids)
    end)
    try("physics3d.events", function()
        local e = physics3d.events()
        return string.format("count=%d", #e)
    end)
    try("physics3d.get_velocity", function() return physics3d.get_velocity(pid) and "table" or "nil" end)
    try("physics3d.set_ccd/get_ccd", function()
        physics3d.set_ccd(pid, true)
        return "ccd=" .. tostring(physics3d.get_ccd(pid))
    end)
    try("physics3d.debug_draw", function()
        physics3d.debug_draw(true)
        physics3d.debug_draw(false)
        return "toggled"
    end)

    local warrior = lookup("Warrior")
    try("animation3d.list", function()
        if not warrior then return "no Warrior entity" end
        local clips = animation3d.list(warrior)
        return table.concat(clips, ",")
    end)
    try("animation3d.get", function()
        if not warrior then return "no Warrior entity" end
        -- `animation3d.get` 返回**裸字符串**（当前剪辑名），而同族的 transform3d/camera3d/light
        -- 返回的是表——形状不统一，按表读会得到静默 nil 而不是报错。
        local name = animation3d.get(warrior)
        return string.format("current=%s", tostring(name))
    end)
    try("animation3d.set(切换剪辑)", function()
        if not warrior then return "no Warrior entity" end
        local clips = animation3d.list(warrior)
        animation3d.set(warrior, clips[1])
        return string.format("now=%s", tostring(animation3d.get(warrior)))
    end)
    try("animation3d.set_paused/time", function()
        if not warrior then return "no Warrior entity" end
        animation3d.set_paused(warrior, true)
        local p = animation3d.paused(warrior)
        animation3d.set_paused(warrior, false)
        return string.format("paused=%s time=%.3f", tostring(p), animation3d.time(warrior))
    end)

    try("entity.spawn+transform3d.attach+mesh.set", function()
        local id = entity.spawn("ProbeCube3d")
        transform3d.attach(id)
        mesh.set(id, "assets/models/sokoban-cube.glb")
        transform3d.move(id, 3.4, 0.5, -2.6)
        local got = mesh.get(id)
        entity.destroy(id)
        return "spawned mesh=" .. tostring(got)
    end)

    try("ui.text_at/text_get", function()
        ui.text_at("probe_line", 24, 690, "probe ok", 1)
        local t = ui.text_get("probe_line")
        ui.clear("probe_line")
        return t and t.text or "nil"
    end)

    -- 期望有、但没有的 3D 接口：这一批不是报错，是缺口的直接测量。
    missing("material")
    missing("environment")
    missing("model")
    missing("mesh3d")
    missing("primitive")
    missing("ray")
    missing("light3d")
    missing("fog")
    missing("postprocess")
    missing("sky")
    missing("nav")
    missing("character")

    return M.ok_count(), M.err_count()
end

function M.ok_count()
    local n = 0
    for _, p in ipairs(M.probe) do
        if p.kind == "ok" then n = n + 1 end
    end
    return n
end

function M.err_count()
    local n = 0
    for _, p in ipairs(M.probe) do
        if p.kind == "err" then n = n + 1 end
    end
    return n
end

function M.summarize()
    note("summary", "probe", string.format("ok=%d err=%d absent=%d", M.ok_count(), M.err_count(), #M.absent))
end

function M.report()
    local lines = {}
    for _, p in ipairs(M.probe) do
        lines[#lines + 1] = string.format("%s|%s|%s", p.kind, p.label, p.detail)
    end
    return table.concat(lines, "\n")
end

-- ── 生命周期 ────────────────────────────────────────────────────────────────────
function M.on_create()
    local orphan = M.load()
    note("ok", "scene.bind", string.format("walls=%d goals=%d boxes=%d orphan=%d",
        (function()
            local n = 0
            for _ in pairs(M.walls) do n = n + 1 end
            return n
        end)(),
        (function()
            local n = 0
            for _ in pairs(M.goals) do n = n + 1 end
            return n
        end)(),
        (function()
            local n = 0
            for _ in pairs(M.boxes) do n = n + 1 end
            return n
        end)(),
        orphan))
    M.probe3d()
end

local function pressed(name)
    local ok, value = pcall(function() return input.just_pressed(name) end)
    return ok and value == true
end

function M.update()
    M.frames = (M.frames or 0) + 1
    if M.frames == 3 and not M.camera_probed then
        M.camera_probed = true
        M.probe_camera()
    end
    if pressed("w") then
        step(0, -1)
    elseif pressed("s") then
        step(0, 1)
    elseif pressed("a") then
        step(-1, 0)
    elseif pressed("d") then
        step(1, 0)
    elseif pressed("x") then
        M.reset()
    elseif pressed("c") then
        pcall(function() physics3d.debug_draw(true) end)
    elseif pressed("v") then
        pcall(function()
            local r = camera3d.mouse_ray()
            local hit = physics3d.raycast(r.ox, r.oy, r.oz, r.dx, r.dy, r.dz, 60)
            M.last_pick = hit and string.format("picked entity %s", tostring(hit)) or "ray hit nothing"
        end)
    end

    local status
    if M.won then
        status = string.format("SOLVED in %d moves / %d pushes  (x = restart)", M.moves, M.pushes)
    else
        local goals, on_goal = 0, 0
        for k in pairs(M.goals) do
            goals = goals + 1
            if M.boxes[k] then
                on_goal = on_goal + 1
            end
        end
        status = string.format("moves %d  pushes %d  crates on goal %d/%d", M.moves, M.pushes, on_goal, goals)
    end
    ui.text_at("status", 24, 660, status, 1.4)
    ui.text_at("hint", 24, 634, "wasd move  x restart  c collider wire  v ray-pick at cursor", 1)
    ui.text_at("pick", 24, 608, M.last_pick or "", 1)
    ui.text_at("probe", 24, 582, string.format("3D probe: %d ok / %d err / %d absent api",
        (function()
            local n = 0
            for _, p in ipairs(M.probe) do if p.kind == "ok" then n = n + 1 end end
            return n
        end)(),
        (function()
            local n = 0
            for _, p in ipairs(M.probe) do if p.kind == "err" then n = n + 1 end end
            return n
        end)(),
        #M.absent), 1)
    -- `update()` 的返回值是宿主的 statusText 通道（bundle-summary 的首帧读数就是它）。
    return status
end

function M.on_destroy()
    ui.clear("status")
    ui.clear("hint")
    ui.clear("pick")
    ui.clear("probe")
end

return M
