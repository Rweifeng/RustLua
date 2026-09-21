-- api_tilemap：演示 `tilemap` 与 `grid` 两个模块，共 14 个函数。
--
-- tilemap：create / get / is_solid / get_cell / set_cell / set_solid / set_texture /
--          get_meta / set_meta / find_path
-- grid：   world_to_cell / cell_to_world / facing_cell / in_bounds
--
-- 这个演示想证明四件事（每一条都对应下面的某一段代码）：
--   1. 场景里写死的 `Tilemap` 组件串（`tiles`/`solid` 逐格字符）就是引擎解析出来的网格，
--      Lua 读到的 `width/height/tile_size/texture` 与场景文件里的字段一一对应；
--   2. 运行期用 `tilemap.create(width, height, tile_size[, texture][, name])` 造出来的
--      第二张地图（一块 4x3 菜地）**不需要预先写进场景**，返回的是实体 id，
--      `get` 能把它（含 M29 的 `texture`）读回来，`set_cell/set_solid/set_texture/
--      get_cell/is_solid/set_meta/get_meta` 都能在这张"数据驱动的网格"上原地改；
--   3. `find_path(id, fx, fy, tx, ty[, diagonal])` 以 solid 为阻挡做 A*：成功返回含两端的
--      `{x,y}` 路径表，失败返回 `nil, reason`（是返回值，**不是报错**），Walker 会沿它走；
--   4. `grid` 的四个换算把"世界坐标 ↔ 格子坐标"讲清楚：鼠标指向哪一格、那一格的中心在
--      哪里、朝前走一格是哪一格、以及越界判定——全部用鼠标位置实时演示。
--
-- 引擎坐标约定（在下面各处都会用到）：
--   * 格子是 0-index、**y 朝上**（row 0 在地图最底，见 docs/16 与 core/tilemap.rs）；
--   * 格子 (x, y) 的中心在世界坐标 ((x+0.5)*tile, (y+0.5)*tile)（Tilemap 实体 Transform
--     在原点时成立，本场景的 GroundMap 就是 position=0,0）；
--   * 格子值是"贴图集第几列"：0=空、1=墙、3=地面（本场景只用到这三列；菜地还会用 2=湿地）；
--   * `solid` 是**独立的一层**（1=阻挡，与"画了什么"无关）——寻路只看它，不看 tiles；
--   * `grid.world_to_cell` / `grid.facing_cell` 返回的是 **1 基数组**：`[1]`=x、`[2]`=y；
--     `grid.cell_to_world` 返回的也是数组 `[1]`=x、`[2]`=y（core/script.rs 的 table_i32_pair）。
local api = require("api_demo")

local M = {}

-- 场景里 GroundMap 的格子边长（与组件串 `tile_size=32x32` 必须一致：格子换算只有这一处真源）。
local TILE = 32
-- 场景文件里 Walker 的初始格 / Target 精灵标记的终点格（两格都是 non-solid，中间那堵墙
-- 让路径必须绕行；`GOAL` 与 scenes/api_tilemap.scene 里 Target 的 position 是同一个格子）。
local WALK = { x = 2, y = 2 }
local GOAL = { x = 7, y = 7 }
-- 菜地（运行期 tilemap.create 出来的第二张地图）。
local FARM_W, FARM_H, FARM_TILE = 4, 3, 32
local FARM_POS = { x = 384, y = 32 }
-- 菜地视觉：1..6 = tileset 的列（见 assets/textures/tileset.png）。
local TILE_DRY, TILE_WET = 1, 2
-- 沿路径推进的节拍：每 6 个固定步走一格（约 0.1s/格 @60Hz），看得清但不会一晃而过。
local STEP_FRAMES = 6
-- `grid.facing_cell` 的 facing 参数：四个方向词 + n/s/e 三个短码，每帧轮换一个。
-- （`w` 与 `left` 同义，这里不重复列；`n`/`s`/`e` 用来证明短码也被解析。）
local FACINGS = { "up", "down", "left", "right", "n", "s", "e" }

local ground_id = nil       -- 场景声明的 GroundMap
local walker_id = nil       -- 场景声明的 Walker（Sprite + Transform）
local sign_id = nil         -- 场景声明的 Sign（WorldText）
local farm_id = nil         -- 运行期 tilemap.create 出来的菜地
local ground_w, ground_h = 10, 10

local facing_index = 0
local line = ""             -- 本帧拼好的状态行（update 末尾一次性 api.status）
local last_path, last_reason, last_terrain = nil, "-", "-"
local last_diagonal = false  -- 上一次 find_path 是否允许走对角线（T=四向 / N=八向）
local last_farm_path, last_farm_reason = nil, "-"  -- 菜地自己的 find_path 结果
local path_active = false
local path_step = 0         -- 已走到的路径下标（1 = 起点）
local walk_tick = 0
local watered = false
local last_sign = ""

--- 实体 id 是否"存在且活着"：`tilemap.*` 对没有 Tilemap 组件的实体会直接报 Lua 错误，
--- 所以调用前必须先问清楚（实体被销毁/名字拼错时返回 nil，而不是抛错）。
local function live(id)
    return id ~= nil and entity.is_alive(id)
end

--- 当前 Walker 所在格（1 基数组 → 具名字段；y 朝上）。
local function walker_cell()
    if not live(walker_id) then
        return { x = WALK.x, y = WALK.y }
    end
    local pos = transform.get(walker_id)
    local cell = grid.world_to_cell(pos.x, pos.y, TILE)
    return { x = cell[1], y = cell[2] }
end

local function walker_world()
    if not live(walker_id) then
        local p = grid.cell_to_world(WALK.x, WALK.y, TILE)
        return { x = p[1], y = p[2] }
    end
    local pos = transform.get(walker_id)
    return { x = pos.x, y = pos.y }
end

--- 把 (cx, cy) 格刷成 `value`，并同步 solid —— `set_solid` 会立刻重建这张地图的固定物理
--- 碰撞体（core/script.rs 的 refresh_tilemap_physics），因此改完就能被寻路与物理同时看到。
--- 越界已经先用 `grid.in_bounds` 挡掉：两条 API 的"越界"报错路径也就不必靠报错来演示。
local function carve(cx, cy, value, blocked)
    tilemap.set_cell(farm_id, cx, cy, value)
    tilemap.set_solid(farm_id, cx, cy, blocked)
end

--- 空 value = 删除键（core/tilemap.rs 的 Tilemap::set_meta）。key 为空才是报错。
local function mark(cx, cy, key, value)
    tilemap.set_meta(farm_id, cx, cy, key, value)
end

--- 每格 meta 的两个键：`crop`（种的是什么）与 `watered`（浇没浇水）。
local function crop_tag(cx, cy)
    return string.format("%d,%d:%s/%s", cx, cy,
        tostring(tilemap.get_meta(farm_id, cx, cy, "crop")),
        tostring(tilemap.get_meta(farm_id, cx, cy, "watered")))
end

--- 把菜地整块重画：浇过水就换成"湿地"列（2），否则"干地"列（1）。
--- 这一段只用 set_cell / set_solid / get_cell / is_solid，证明运行期地图是**可读可写**的。
--- `tilemap.*` 对不存在的实体 id 会报 Lua 错误（`bad argument #1`），所以先确认菜地在。
local function paint_farm()
    if not live(farm_id) then
        return
    end
    local want = watered and TILE_WET or TILE_DRY
    for cy = 0, FARM_H - 1 do
        for cx = 0, FARM_W - 1 do
            if tilemap.get_cell(farm_id, cx, cy) ~= want then
                carve(cx, cy, want, false)
            end
        end
    end
end

--- 种一茬麦子并浇水：每格写 crop=wheat / watered=1，再**读回来**放进状态行。
--- 读回来的值就是"真的写进去了"的证据，而不是把刚才写的变量再打印一遍。
local function plant()
    if not live(farm_id) then
        return
    end
    watered = true
    paint_farm()
    for cy = 0, FARM_H - 1 do
        for cx = 0, FARM_W - 1 do
            mark(cx, cy, "crop", "wheat")
            mark(cx, cy, "watered", "1")
        end
    end
end

--- 收获：空 value 删掉 `watered` 键，`get_meta` 读回就是 nil（区分"键不存在"与"值是空串"）。
local function harvest()
    if not live(farm_id) then
        return
    end
    watered = false
    paint_farm()
    for cy = 0, FARM_H - 1 do
        for cx = 0, FARM_W - 1 do
            mark(cx, cy, "crop", "")
            mark(cx, cy, "watered", "")
        end
    end
end

--- `find_path` 的失败是**返回值**（nil, reason），不是 Lua 错误：
---   * 起点/终点越界 → "path endpoint (x, y) is out of bounds"
---   * 起点/终点是 solid → "path endpoint (x, y) is solid"
---   * 绕不过去 → "no path from (x, y) to (x, y)"
--- 因此这里只按返回值分支，不需要 pcall。`map_id` 为 nil（地图不存在）时直接记原因，
--- 因为 `tilemap.*` 收到不存在的实体 id 会报 Lua 错误而不是返回 nil。
local function set_path(map_id, from, to, diagonal)
    path_step = 0
    walk_tick = 0
    if not live(map_id) then
        last_path, last_reason, path_active = nil, "no tilemap entity", false
        return
    end
    local path, reason = tilemap.find_path(map_id, from.x, from.y, to.x, to.y, diagonal)
    last_diagonal = diagonal and true or false
    last_reason = reason or "-"
    if path then
        last_path = path
        path_active = true
        path_step = 1
    else
        -- 失败（nil, reason）：不留上一次的路径，step 也归零，免得状态行显示过期的进度。
        last_path = nil
        path_active = false
    end
end

--- 运行期菜地也能寻路：4x3 里从 (0,0) 走到 (3,2)，证明 `create` 出来的地图和场景里声明的地图
--- 走的是同一套 A*（返回含两端的路径表）。这一段只读，不驱动 Walker。
local function farm_path()
    local path, reason = tilemap.find_path(farm_id, 0, 0, FARM_W - 1, FARM_H - 1, false)
    last_farm_path, last_farm_reason = path, reason or "-"
end

--- 沿路径格心逐格推进（不是瞬移）：先把目标点换算成世界坐标（`grid.cell_to_world`），
--- 再用 `transform.get/set` 把 Walker 挪过去。路径走完就停下。
local function advance_walker()
    if not path_active or not live(walker_id) or not last_path then
        return
    end
    walk_tick = walk_tick + 1
    local goal_index = #last_path
    if walk_tick >= STEP_FRAMES and path_step < goal_index then
        walk_tick = 0
        path_step = path_step + 1
    end
    local cell = last_path[path_step]
    local pos = grid.cell_to_world(cell.x, cell.y, TILE)
    transform.set(walker_id, pos[1], pos[2])
    if path_step >= goal_index then
        path_active = false
    end
end

--- WorldText 运行期挂载：`Sign` 场景里带的是占位文字，这里把"下一个格子"贴到它头上。
local function set_sign(text)
    if not live(sign_id) or text == last_sign then
        return
    end
    last_sign = text
    world_text.attach(sign_id, "text=" .. text .. ";font_size=14;color=0.900,1.000,0.700;offset=0,0;centered=true")
end

--- 本帧的格子换算（这一整段就是 `grid` 的四个函数）：
---   ① world_to_cell：光标世界坐标 → 它指着哪一格
---   ② cell_to_world：那一格的**中心**在世界哪里（= 把格子换算回去）
---   ③ in_bounds：那一格在这张地图里吗（越界的光标格不该被当成合法农田）
---   ④ facing_cell：从光标格朝某个方向走一格是哪一格
local function grid_state()
    local cursor = input.cursor_position()
    local sx, sy = cursor[1], cursor[2]
    -- `camera2d.mouse_world()` 把宿主回填的相机快照与光标屏幕像素一次换算成世界坐标，
    -- 返回 `{x, y}`（core/script.rs：`table_vec`），所以不是多返回值——要取字段。
    -- 没有渲染帧时（headless / 编辑器未 Play）它会明确报错，`api.try` 把它变成
    -- `nil + 原因`，这时退回"Walker 所在的格子"，演示照样可读。
    local wx, wy = nil, nil
    local world = api.try(camera2d.mouse_world)
    if world ~= nil and world.x ~= nil then
        wx, wy = world.x, world.y
    else
        local pos = walker_world()
        wx, wy = pos.x, pos.y
    end
    local cell = grid.world_to_cell(wx, wy, TILE)
    local cx, cy = cell[1], cell[2]
    local center = grid.cell_to_world(cx, cy, TILE)
    local inside = grid.in_bounds(cx, cy, ground_w, ground_h)

    facing_index = facing_index % #FACINGS + 1
    local facing = FACINGS[facing_index]
    local front = grid.facing_cell(wx, wy, facing, TILE)

    return {
        screen_x = sx,
        screen_y = sy,
        world_x = wx,
        world_y = wy,
        cx = cx,
        cy = cy,
        center_x = center[1],
        center_y = center[2],
        inside = inside,
        facing = facing,
        front_x = front[1],
        front_y = front[2],
    }
end

--- 绑定场景里已经声明好的实体。`entity.find` 找不到返回 nil，`tilemap.get` 对"没有 Tilemap
--- 组件的实体"会报 Lua 错误，所以两个条件都要过。
local function bind_maps()
    ground_id = entity.find("GroundMap")
    if not live(ground_id) or tilemap.get(ground_id) == nil then
        ground_id = nil
        api.status("api_tilemap: scene has no GroundMap Tilemap entity")
        return
    end
    walker_id = entity.find("Walker")
    sign_id = entity.find("Sign")
    local info = tilemap.get(ground_id)
    ground_w, ground_h = info.width, info.height
end

--- 用 `tilemap.create` 造第二张地图（4x3 菜地）。这是"数据驱动的网格不需要预先写进场景"：
--- 场景里只有 GroundMap，菜地完全是运行期长出来的。返回的是实体 id，`get` 再读回字段。
local function seed_farm()
    farm_id = tilemap.create(FARM_W, FARM_H, FARM_TILE, "assets/textures/tileset.png", "TilemapFarmDemo")
    tilemap.set_texture(farm_id, "assets/textures/tileset.png")
    local pos = transform.get(farm_id)
    transform.set(farm_id, FARM_POS.x, FARM_POS.y, pos.rotation, pos.scale_x, pos.scale_y)
    paint_farm()
    mark(0, 0, "crop", "wheat")
    mark(0, 0, "watered", "1")
    mark(1, 0, "crop", "carrot")
    mark(0, 1, "tilled", "1")
end

function M.on_create()
    api.title("api_tilemap - scene Tilemap + runtime tilemap.create + A* + grid 换算")
    api.hint("T path  N diagonal  P solid-end  I oob-end  Y same-cell  J farm-A*  M plant  R harvest  - mouse over a cell to see it - " .. api.KEYS_BACK)
    bind_maps()
    if ground_id ~= nil then
        seed_farm()
    end
    log.info("api_tilemap: demo ready (scene Tilemap + runtime farm + grid)")
end

function M.update()
    line = ""
    local from = walker_cell()

    -- 键盘（键名用 v1 可读名：`t`/`n`/`p`/`i`/`y`/`j`/`m`/`r`，`input.just_pressed` 也认 `key:t`）。
    if input.just_pressed("t") then
        -- 主线：从 Walker 当前格寻到终点格（终点在场景声明的 GroundMap 上，四向）。
        set_path(ground_id, from, GOAL, false)
    elseif input.just_pressed("n") then
        -- 同一个起终点、允许对角线：路径更短（状态行的 diagonal / path_len 会变）。
        set_path(ground_id, from, GOAL, true)
    elseif input.just_pressed("p") then
        -- 终点是 solid 的那一格（中间那堵墙）：find_path 返回 nil, reason。
        set_path(ground_id, from, { x = 5, y = 4 }, false)
    elseif input.just_pressed("i") then
        -- 终点越界：同样返回 nil, reason，而不是报错。
        set_path(ground_id, from, { x = 99, y = 99 }, false)
    elseif input.just_pressed("y") then
        -- 起点 = 终点：返回只含一个格子的路径（也是成功返回，不是报错）。
        set_path(ground_id, from, from, false)
    elseif input.just_pressed("j") then
        -- 运行期菜地（4x3）自己的 A*：证明 create 出来的地图走同一套寻路。
        farm_path()
    elseif input.just_pressed("m") then
        -- 种 + 浇：给整块菜地写 crop=wheat / watered=1。
        plant()
    elseif input.just_pressed("r") then
        -- 收获：空 value 删掉两个键，`get_meta` 随即读回 nil。
        harvest()
    end

    advance_walker()

    -- 场景声明的 Tilemap：把游标指着的那一格的事实读出来。
    if ground_id == nil then
        api.status("GroundMap missing: tilemap.* needs a Tilemap entity")
        return "api_tilemap_ok ground=none"
    end
    local g = grid_state()
    -- `tilemap.get(id)` 读回的是组件字段：tile_size / width / height / texture（texture 是
    -- M29 新增字段——运行期换过的贴图在这里就能看见）。
    local ground = tilemap.get(ground_id)
    local cell_value = tilemap.get_cell(ground_id, g.cx, g.cy)
    -- `is_solid` 对越界格返回 false（core/script.rs：`is_solid(x, y).unwrap_or(false)`），
    -- 所以"越界"与"实心"要靠 in_bounds 分开判断，这里两个都打出来。
    local solid = tilemap.is_solid(ground_id, g.cx, g.cy)
    local path_len = last_path and #last_path or 0

    -- 运行期菜地：只认"光标格确实落在菜地范围内"的查询，否则 tilemap.* 会因越界报错。
    local terrain = last_terrain
    if live(farm_id) and grid.in_bounds(g.cx, g.cy, FARM_W, FARM_H) then
        local info = tilemap.get(farm_id)
        terrain = string.format("cell(%d,%d)=%d solid=%s %s", g.cx, g.cy,
            tilemap.get_cell(farm_id, g.cx, g.cy),
            tostring(tilemap.is_solid(farm_id, g.cx, g.cy)),
            crop_tag(g.cx, g.cy))
        last_terrain = terrain
        if info.texture ~= nil then
            terrain = terrain .. " tex=" .. tostring(info.texture)
        end
    end

    -- 状态行拆三行：`api.status` 只是一串文本，渲染器会按 `\n` 换行（text.rs 的换行布局），
    -- 三行 × ~80 字符在 1280 宽的取景框里放得下，不会被裁掉。
    line = string.format(
        "GROUND %dx%d tile32 | cursor=(%.0f,%.0f)screen -> world(%.1f,%.1f) -> cell(%d,%d) center=(%.1f,%.1f) in_bounds=%s solid=%s tile=%s",
        ground.width, ground.height, g.screen_x, g.screen_y, g.world_x, g.world_y,
        g.cx, g.cy, g.center_x, g.center_y, tostring(g.inside), tostring(solid), tostring(cell_value))
    line = line .. string.format(
        "\nFACING %s from (%.0f,%.0f) -> cell(%d,%d) | FARM %s | farm .tex=%s path=%s reason=%s",
        g.facing, g.world_x, g.world_y, g.front_x, g.front_y, terrain,
        live(farm_id) and tostring(tilemap.get(farm_id).texture) or "-",
        last_farm_path and tostring(#last_farm_path) or "nil", tostring(last_farm_reason))
    line = line .. string.format(
        "\nPATH walker=cell(%d,%d) goal=cell(%d,%d) len=%d step=%d diagonal=%s reason=%s",
        from.x, from.y, GOAL.x, GOAL.y, path_len, path_step, tostring(last_diagonal), tostring(last_reason))

    if last_path then
        local head = last_path[1]
        local mid = last_path[math.min(2, path_len)]
        local tail = last_path[path_len]
        line = line .. string.format(" first=(%d,%d) next=(%d,%d) last=(%d,%d)",
            head.x, head.y, mid.x, mid.y, tail.x, tail.y)
    else
        line = line .. " (nil: T=try again, P/I=fail cases, J=farm route)"
    end

    api.status(line)
    set_sign(string.format("next cell (%d,%d)", g.front_x, g.front_y))

    -- 返回值进引擎日志：headless 也能看到这一帧的路径长度 / 失败原因 / 元数据。
    return string.format(
        "api_tilemap_ok cursor_cell=(%d,%d) in_bounds=%s solid=%s tile=%s walker=(%d,%d) path_len=%d step=%d diag=%s reason=%s farm_path=%s",
        g.cx, g.cy, tostring(g.inside), tostring(solid), tostring(cell_value),
        from.x, from.y, path_len, path_step, tostring(last_diagonal), tostring(last_reason),
        last_farm_path and tostring(#last_farm_path) or "nil")
end

function M.on_destroy()
    ground_id = nil
    walker_id = nil
    sign_id = nil
    farm_id = nil
    last_path = nil
    last_farm_path = nil
    path_active = false
    last_sign = ""
end

return M
