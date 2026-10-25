-- api_tilemap：2.0 分步 Lab。主题是"格子数据是可读可写的一等公民"。
--
-- 这个演示只做内存里的读写，所以 headless 自检与交互演示走同一条路径：
--   1. 场景里的 GroundMap 组件串被引擎解析成 10x10 的格子，`tilemap.get` 读回的
--      width/height/tile_size/texture 与场景字段一致；`grid.*` 负责世界坐标 ↔ 格子换算；
--   2. `tilemap.create` 在运行期长出一张 4x3 菜地（返回实体 id，名字可被 `entity.find`），
--      图集布局、kind 解析、A* 寻路与场景地图完全同一套；
--   3. `set_cell` 画什么与 `set_solid` 挡不挡人是**两层**，可以一格空着但仍然实心；
--   4. `set_tint`/`clear_tint` 是逐格顶点色，`set_meta` 的 key 是地图自带的轻量注释；
--   5. `find_path` 失败用 `nil, reason` 返回（不是抛错），对角寻路不会切实心角；
--   6. `transform.get/set` 与 `world_text.attach` 把"格子中心"落实成可见的实体位置。
--
-- 为什么把场景地图挪到 MAP_POS：2.0 shell 的左栏/右卡会盖住世界左下角；把 GroundMap
-- 与三个场景实体一起平移到舞台内，格子数学仍然按"地图原点"计算（查询前减去 MAP_POS），
-- 这样 `grid.world_to_cell(80,80,32) == (2,2)` 这类断言与视觉位置互不干扰。
--
-- 顶层只定义常量与函数（README 硬规则 1）。

local lab = require("lab")

local M = {}

-- 场景 GroundMap 的格子边长；与 scenes/api_tilemap.scene 的 tile_size 一致。
local TILE = 32
-- 运行期把地图放进舞台：舞台屏幕区 296..936 / 76..644，默认 2D 相机把世界 y=0 画在
-- 屏幕底部（screen_y = 720 - world_y），所以 (320, 96) 正好把 10x10 地图放到舞台左半。
local MAP_POS = { x = 320, y = 96 }
-- 场景里 Walker / Target 的初始格（0 基、y 朝上）。
local WALK = { x = 2, y = 2 }
local GOAL = { x = 7, y = 7 }
-- 内墙 (5,4) 是 solid：用它证明"终点实心"是返回值而不是报错。
local SOLID_ENDPOINT = { x = 5, y = 4 }
-- 运行期菜地：4x3，和场景地图一样用 32px 格子。
local FARM_W, FARM_H = 4, 3
local FARM_POS = { x = 680, y = 120 }
local FARM_NAME = "LabTilemapFarm"
local FARM_DRY, FARM_WET = 1, 2
-- 占位图集是 6 列 1 行；显式声明布局后，格子值不会再随贴图尺寸漂移。
local ATLAS_COLS, ATLAS_ROWS = 6, 1
-- 浇水后的冷色乘法染色。
local TINT_WET = { r = 0.72, g = 0.84, b = 1.0 }

local ground_id = nil
local walker_id = nil
local target_id = nil
local sign_id = nil
local farm_id = nil
local ground_w, ground_h = 10, 10
local ground_tile = TILE

-- 缓存每一步 run 里算出来的只读结果（check 再断言，避免在 check 里制造副作用）。
local straight_path, diagonal_path, same_path, farm_path = nil, nil, nil, nil
local straight_reason, diagonal_reason = "-", "-"
local same_reason, farm_reason = "-", "-"
local solid_reason, oob_reason = "-", "-"
local hover = { cx = WALK.x, cy = WALK.y, wx = 0, wy = 0, inside = true }
local last_action = "ready"
local last_sign_text = nil

--- 实体 id 是否"存在且活着"：tilemap.* 对没有 Tilemap 组件的实体会直接报 Lua 错误，
--- 所以每次查询前都要先问清楚（实体被销毁或名字拼错时返回 nil，而不是抛错）。
local function live(id)
    return id ~= nil and entity.is_alive(id)
end

--- 绑定场景实体。entity.find 找不到返回 nil；tilemap.get 对"没有 Tilemap 组件的实体"
--- 也会报错，因此两个条件都要过。
local function bind_scene()
    ground_id = entity.find("GroundMap")
    if not live(ground_id) then
        ground_id = nil
    end
    walker_id = entity.find("Walker")
    if not live(walker_id) then
        walker_id = nil
    end
    target_id = entity.find("Target")
    if not live(target_id) then
        target_id = nil
    end
    sign_id = entity.find("Sign")
    if not live(sign_id) then
        sign_id = nil
    end
    if ground_id ~= nil then
        local info = tilemap.get(ground_id)
        if info ~= nil then
            ground_w, ground_h = info.width, info.height
            ground_tile = info.tile_size
        end
    end
end

--- 场景地图的格子边长为 TILE，但地图被整体挪到 MAP_POS；下面两个换算把"地图局部坐标"
--- 与"实体世界坐标"分开，格子数学只认局部坐标。
local function cell_center(cx, cy)
    local local_pos = grid.cell_to_world(cx, cy, TILE)
    return {
        x = MAP_POS.x + local_pos[1],
        y = MAP_POS.y + local_pos[2],
    }
end

local function cell_of_world(wx, wy)
    local cell = grid.world_to_cell(wx - MAP_POS.x, wy - MAP_POS.y, TILE)
    return cell[1], cell[2]
end

--- 把场景地图与三个场景实体放到舞台内；rotation/scale 用 transform.get 读回再原样传回，
--- 因为 transform.set 省略的参数表示"保持不变"，这里显式传入是为了证明这一点。
local function place_scene()
    if live(ground_id) then
        local pos = transform.get(ground_id)
        transform.set(ground_id, MAP_POS.x, MAP_POS.y, pos.rotation, pos.scale_x, pos.scale_y)
    end
    if live(walker_id) then
        local p = cell_center(WALK.x, WALK.y)
        transform.set(walker_id, p.x, p.y)
    end
    if live(target_id) then
        local p = cell_center(GOAL.x, GOAL.y)
        transform.set(target_id, p.x, p.y)
    end
    if live(sign_id) then
        -- 告示牌放在地图上方（世界 y 更大 = 屏幕更靠上）。
        local pos = transform.get(sign_id)
        transform.set(sign_id, MAP_POS.x + 112, MAP_POS.y + 352, pos.rotation, pos.scale_x, pos.scale_y)
    end
end

--- 把场景地图恢复到本演示依赖的基线：只有 (8,8) 会被"格子编辑"步骤改写；
--- (5,4) 必须实心，起点/终点必须可走。用 grid.in_bounds 先挡越界，避免错误路径。
local function reset_ground()
    if not live(ground_id) then
        return
    end
    if grid.in_bounds(8, 8, ground_w, ground_h) then
        tilemap.set_cell(ground_id, 8, 8, 3)
        tilemap.set_solid(ground_id, 8, 8, false)
    end
    if grid.in_bounds(SOLID_ENDPOINT.x, SOLID_ENDPOINT.y, ground_w, ground_h) then
        tilemap.set_solid(ground_id, SOLID_ENDPOINT.x, SOLID_ENDPOINT.y, true)
    end
    if grid.in_bounds(WALK.x, WALK.y, ground_w, ground_h) then
        tilemap.set_solid(ground_id, WALK.x, WALK.y, false)
    end
    if grid.in_bounds(GOAL.x, GOAL.y, ground_w, ground_h) then
        tilemap.set_solid(ground_id, GOAL.x, GOAL.y, false)
    end
end

--- 运行期菜地的所有配置：贴图、图集布局、kind 语义、位置。id 已存在时也要重新声明，
--- 因为用户可能在别的步骤里换过贴图/布局（本演示只用内存状态，重声明是幂等的）。
local function configure_farm(id)
    farm_id = id
    tilemap.set_texture(farm_id, "assets/textures/tileset.png")
    tilemap.set_atlas(farm_id, ATLAS_COLS, ATLAS_ROWS)
    -- (kinds, variants, autotile) = (6, 1, false)：格子值就是图集条目号（恒等档）。
    tilemap.set_kinds(farm_id, ATLAS_COLS, 1, false)
    local pos = transform.get(farm_id)
    transform.set(farm_id, FARM_POS.x, FARM_POS.y, pos.rotation, pos.scale_x, pos.scale_y)
end

--- 优先复用同名实体：`tilemap.create` 会给实体登记名字（entity.find 同源），
--- 这样反复进入步骤不会长出一堆菜地。尺寸不符时才重新 create。
local function ensure_farm()
    if live(farm_id) then
        return farm_id
    end
    local found = entity.find(FARM_NAME)
    if live(found) then
        local info = tilemap.get(found)
        if info ~= nil and info.width == FARM_W and info.height == FARM_H then
            configure_farm(found)
            return farm_id
        end
    end
    local id = tilemap.create(FARM_W, FARM_H, TILE, "assets/textures/tileset.png", FARM_NAME)
    if id ~= nil then
        configure_farm(id)
    end
    return farm_id
end

--- 菜地基线：全部干地、不实心、无 meta、无染色。每一步都能从这里重新开始，
--- 所以用户在任意步骤里浇水/收获后按 F5，自检仍然可重复。
local function reset_farm()
    ensure_farm()
    if not live(farm_id) then
        return
    end
    for cy = 0, FARM_H - 1 do
        for cx = 0, FARM_W - 1 do
            if tilemap.get_cell(farm_id, cx, cy) ~= FARM_DRY then
                tilemap.set_cell(farm_id, cx, cy, FARM_DRY)
            end
            if tilemap.is_solid(farm_id, cx, cy) then
                tilemap.set_solid(farm_id, cx, cy, false)
            end
            if tilemap.get_meta(farm_id, cx, cy, "crop") ~= nil then
                tilemap.set_meta(farm_id, cx, cy, "crop", "")
            end
            if tilemap.get_meta(farm_id, cx, cy, "watered") ~= nil then
                tilemap.set_meta(farm_id, cx, cy, "watered", "")
            end
            local wash = tilemap.tint(farm_id, cx, cy)
            if wash.r ~= 1 or wash.g ~= 1 or wash.b ~= 1 or wash.a ~= 1 then
                tilemap.clear_tint(farm_id, cx, cy)
            end
        end
    end
end

--- `world_text.attach` 是整串组件描述（与场景文件同语法）。同一段文本不重复写，
--- 免得每帧都改一次 WorldText 组件。它必须先于 reset_world 定义：reset_world 会调用它。
local function attach_sign(text)
    if not live(sign_id) then
        return
    end
    if text == last_sign_text then
        return
    end
    last_sign_text = text
    world_text.attach(
        sign_id,
        "text=" .. text .. ";font_size=14;color=0.900,1.000,0.700;offset=0,0;centered=true"
    )
end

--- 每一步 enter 的统一复位：重新绑定实体（场景可能刚加载）、放回舞台、恢复地图/菜地、
--- 重置 Walker 与所有路径缓存。这样"任何用户交互之后按 F5"都能从同一基线开始。
local function reset_world()
    bind_scene()
    place_scene()
    reset_ground()
    reset_farm()
    if live(walker_id) then
        local p = cell_center(WALK.x, WALK.y)
        transform.set(walker_id, p.x, p.y)
    end
    straight_path, diagonal_path, same_path, farm_path = nil, nil, nil, nil
    straight_reason, diagonal_reason = "-", "-"
    same_reason, farm_reason = "-", "-"
    solid_reason, oob_reason = "-", "-"
    last_sign_text = nil
    attach_sign("tilemap lab")
    hover = { cx = WALK.x, cy = WALK.y, wx = 0, wy = 0, inside = true }
    last_action = "ready"
end

--- `camera2d.mouse_world()` 需要宿主回填的相机快照；headless 下会明确报错。
--- 用 lab.try 的闭包形式包起来：既保留真实调用文本，又把"没有渲染帧"变成可回退的 nil。
local function read_mouse_world()
    return lab.try(function()
        return camera2d.mouse_world()
    end)
end

--- 一次算齐四种寻路结果。失败分支返回 `nil, reason`，所以 reason 要单独保存，
--- 不能在 read 里重新调用（read 必须无副作用）。
local function compute_paths()
    ensure_farm()
    if live(ground_id) then
        straight_path, straight_reason =
            tilemap.find_path(ground_id, WALK.x, WALK.y, GOAL.x, GOAL.y, false)
        diagonal_path, diagonal_reason =
            tilemap.find_path(ground_id, WALK.x, WALK.y, GOAL.x, GOAL.y, true)
        same_path, same_reason =
            tilemap.find_path(ground_id, WALK.x, WALK.y, WALK.x, WALK.y, false)
        local _, blocked = tilemap.find_path(
            ground_id,
            WALK.x,
            WALK.y,
            SOLID_ENDPOINT.x,
            SOLID_ENDPOINT.y,
            false
        )
        local _, outside = tilemap.find_path(ground_id, WALK.x, WALK.y, 99, 99, false)
        solid_reason = blocked or "-"
        oob_reason = outside or "-"
    end
    straight_reason = straight_reason or "-"
    diagonal_reason = diagonal_reason or "-"
    same_reason = same_reason or "-"
    if live(farm_id) then
        farm_path, farm_reason = tilemap.find_path(farm_id, 0, 0, FARM_W - 1, FARM_H - 1, false)
        farm_reason = farm_reason or "-"
    end
end

--- 浇水：整块菜地换成湿地列、写 crop/watered、上冷色 tint。写进去马上能读回来，
--- 这是运行期地图"可读可写"的直接证据。
local function water_farm()
    ensure_farm()
    if not live(farm_id) then
        return
    end
    for cy = 0, FARM_H - 1 do
        for cx = 0, FARM_W - 1 do
            tilemap.set_cell(farm_id, cx, cy, FARM_WET)
            tilemap.set_tint(farm_id, cx, cy, TINT_WET.r, TINT_WET.g, TINT_WET.b)
            tilemap.set_meta(farm_id, cx, cy, "crop", "wheat")
            tilemap.set_meta(farm_id, cx, cy, "watered", "1")
        end
    end
    last_action = "farm watered: 12 cells wet, crop=wheat"
end

--- 收获：格子换回干地，meta 用空串删键（get_meta 读回 nil），染色撤销回白色。
local function harvest_farm()
    ensure_farm()
    if not live(farm_id) then
        return
    end
    for cy = 0, FARM_H - 1 do
        for cx = 0, FARM_W - 1 do
            tilemap.set_cell(farm_id, cx, cy, FARM_DRY)
            tilemap.set_meta(farm_id, cx, cy, "crop", "")
            tilemap.set_meta(farm_id, cx, cy, "watered", "")
            tilemap.clear_tint(farm_id, cx, cy)
        end
    end
    last_action = "farm harvested: meta keys removed, tint cleared"
end

--- 格子编辑：把 (8,8) 的画（tile 值）与阻挡（solid）分开翻转，证明两层互不绑定。
local function toggle_ground_cell()
    if not live(ground_id) then
        return
    end
    if not grid.in_bounds(8, 8, ground_w, ground_h) then
        return
    end
    local value = tilemap.get_cell(ground_id, 8, 8)
    local solid = tilemap.is_solid(ground_id, 8, 8)
    tilemap.set_cell(ground_id, 8, 8, value == 0 and 3 or 0)
    tilemap.set_solid(ground_id, 8, 8, not solid)
    last_action = string.format("cell(8,8) art=%d solid=%s", value == 0 and 3 or 0, tostring(not solid))
end

local steps = {
    {
        id = "scene_grid",
        title = "Scene grid",
        claim = "场景 Tilemap 就是 10x10 格子数据；鼠标悬停读格，T/D/S/O/C/J 切换寻路分支",
        api = {
            "tilemap.get",
            "tilemap.get_cell",
            "tilemap.is_solid",
            "grid.world_to_cell",
            "grid.cell_to_world",
            "grid.in_bounds",
            "camera2d.mouse_world",
            "entity.find",
        },
        code = "tilemap.get(id) / grid.world_to_cell(80,80,32)",
        enter = function()
            reset_world()
        end,
        run = function(dt)
            local world = read_mouse_world()
            if world ~= nil and world.x ~= nil then
                hover.wx, hover.wy = world.x, world.y
            else
                local p = cell_center(WALK.x, WALK.y)
                hover.wx, hover.wy = p.x, p.y
            end
            hover.cx, hover.cy = cell_of_world(hover.wx, hover.wy)
            hover.inside = grid.in_bounds(hover.cx, hover.cy, ground_w, ground_h)
            last_action = string.format(
                "GroundMap %dx%d tile=%.0f  hover cell (%d,%d) inside=%s",
                ground_w,
                ground_h,
                ground_tile,
                hover.cx,
                hover.cy,
                tostring(hover.inside)
            )
            lab.note(last_action)
        end,
        read = function()
            local info = live(ground_id) and tilemap.get(ground_id) or nil
            local center = grid.cell_to_world(WALK.x, WALK.y, TILE)
            return {
                { "size", info and string.format("%dx%d", info.width, info.height) or "-" },
                { "tile", info and string.format("%.0f", info.tile_size) or "-" },
                { "texture", info and info.texture or "-" },
                { "cell(2,2)", live(ground_id) and tilemap.get_cell(ground_id, WALK.x, WALK.y) or "-" },
                { "solid(2,2)", live(ground_id) and tostring(tilemap.is_solid(ground_id, WALK.x, WALK.y)) or "-" },
                { "center", string.format("%.0f,%.0f", center[1], center[2]) },
                { "hover", string.format("(%d,%d)", hover.cx, hover.cy) },
            }
        end,
        check = function(t)
            t:ok(live(ground_id), "GroundMap entity exists")
            if not live(ground_id) then
                return
            end
            local info = tilemap.get(ground_id)
            t:ok(info ~= nil, "GroundMap has a Tilemap component")
            if info == nil then
                return
            end
            t:eq(info.width, 10, "map width comes from the scene")
            t:eq(info.height, 10, "map height comes from the scene")
            t:eq(info.tile_size, TILE, "tile size comes from the scene")
            t:ok(tostring(info.texture):find("tileset.png") ~= nil, "map samples the tileset")
            t:eq(tilemap.get_cell(ground_id, WALK.x, WALK.y), 3, "cell(2,2) is ground art")
            t:eq(tilemap.is_solid(ground_id, WALK.x, WALK.y), false, "start cell is walkable")
            t:eq(
                tilemap.is_solid(ground_id, SOLID_ENDPOINT.x, SOLID_ENDPOINT.y),
                true,
                "interior wall cell is solid"
            )
            local cell = grid.world_to_cell(80, 80, TILE)
            t:eq(cell[1], 2, "world_to_cell x")
            t:eq(cell[2], 2, "world_to_cell y")
            local center = grid.cell_to_world(2, 2, TILE)
            t:near(center[1], 80, 0.001, "cell_to_world x")
            t:near(center[2], 80, 0.001, "cell_to_world y")
            t:eq(grid.in_bounds(9, 9, ground_w, ground_h), true, "last cell is in bounds")
            t:eq(grid.in_bounds(10, 0, ground_w, ground_h), false, "x=10 is out of bounds")
            t:eq(entity.find("GroundMap"), ground_id, "entity.find resolves GroundMap")
            if walker_id ~= nil then
                t:eq(entity.is_alive(walker_id), true, "Walker stays alive")
            end
        end,
    },
    {
        id = "runtime_farm",
        title = "Runtime farm",
        claim = "tilemap.create 在运行期长出一张 4x3 菜地：布局声明与 kind 解析立刻生效（W/H 改菜地）",
        api = {
            "tilemap.create",
            "tilemap.set_texture",
            "tilemap.set_atlas",
            "tilemap.set_kinds",
            "tilemap.get",
            "tilemap.get_cell",
            "tilemap.atlas_index",
            "tilemap.autotile_mask",
        },
        code = "tilemap.create(4,3,32) / set_atlas(6,1) / set_kinds(6,1,false)",
        enter = function()
            reset_world()
            ensure_farm()
        end,
        run = function(dt)
            ensure_farm()
            if live(farm_id) then
                local info = tilemap.get(farm_id)
                local mask = tilemap.autotile_mask(farm_id, 1, 1)
                last_action = string.format(
                    "farm %dx%d atlas=%dx%d mask(1,1)=%d",
                    info.width,
                    info.height,
                    info.atlas_cols,
                    info.atlas_rows,
                    mask
                )
            else
                last_action = "farm missing"
            end
            lab.note(last_action)
        end,
        read = function()
            local info = live(farm_id) and tilemap.get(farm_id) or nil
            return {
                { "size", info and string.format("%dx%d", info.width, info.height) or "-" },
                { "tile", info and string.format("%.0f", info.tile_size) or "-" },
                { "atlas", info and string.format("%dx%d", info.atlas_cols, info.atlas_rows) or "-" },
                { "texture", info and info.texture or "-" },
                { "cell(1,1)", live(farm_id) and tilemap.get_cell(farm_id, 1, 1) or "-" },
                { "atlas_index", live(farm_id) and tilemap.atlas_index(farm_id, 1, 1) or "-" },
                { "mask(1,1)", live(farm_id) and tilemap.autotile_mask(farm_id, 1, 1) or "-" },
            }
        end,
        check = function(t)
            t:ok(live(farm_id), "runtime farm entity exists")
            if not live(farm_id) then
                return
            end
            local info = tilemap.get(farm_id)
            t:ok(info ~= nil, "created entity really has a Tilemap")
            if info == nil then
                return
            end
            t:eq(info.width, FARM_W, "runtime map width")
            t:eq(info.height, FARM_H, "runtime map height")
            t:eq(info.tile_size, TILE, "runtime map tile size")
            t:eq(info.atlas_cols, ATLAS_COLS, "declared atlas columns")
            t:eq(info.atlas_rows, ATLAS_ROWS, "declared atlas rows")
            t:ok(tostring(info.texture):find("tileset.png") ~= nil, "texture swap is readable")
            t:eq(tilemap.get_cell(farm_id, 1, 1), FARM_DRY, "cell reset to dry ground")
            t:eq(tilemap.is_solid(farm_id, 1, 1), false, "farm cells are walkable")
            -- 恒等档下，格子值就是图集条目号；四个邻居同 id -> mask = 0b1111 = 15。
            t:eq(tilemap.atlas_index(farm_id, 1, 1), FARM_DRY, "identity atlas lookup")
            t:eq(tilemap.autotile_mask(farm_id, 1, 1), 15, "interior cell sees four same neighbours")
            t:ok(entity.find(FARM_NAME) ~= nil, "tilemap.create registered a findable name")
        end,
    },
    {
        id = "cell_editing",
        title = "Cell editing",
        claim = "set_cell 改的是画、set_solid 改的是阻挡，两层可以独立翻转（E 翻转 (8,8)）",
        api = {
            "tilemap.set_cell",
            "tilemap.set_solid",
            "tilemap.get_cell",
            "tilemap.is_solid",
            "tilemap.get",
            "grid.in_bounds",
        },
        code = "set_cell(id,8,8,0); set_solid(id,8,8,true)",
        enter = function()
            reset_world()
        end,
        run = function(dt)
            if live(ground_id) and grid.in_bounds(8, 8, ground_w, ground_h) then
                tilemap.set_cell(ground_id, 8, 8, 0)
                tilemap.set_solid(ground_id, 8, 8, true)
            end
            last_action = "cell(8,8) art=0 but solid=true"
            lab.note(last_action)
        end,
        read = function()
            if not live(ground_id) then
                return { { "map", "missing" } }
            end
            return {
                { "art", tilemap.get_cell(ground_id, 8, 8) },
                { "solid", tostring(tilemap.is_solid(ground_id, 8, 8)) },
                { "tile", string.format("%.0f", ground_tile) },
            }
        end,
        check = function(t)
            t:ok(live(ground_id), "GroundMap exists for editing")
            if not live(ground_id) then
                return
            end
            t:eq(tilemap.get_cell(ground_id, 8, 8), 0, "run cleared the tile art")
            t:eq(tilemap.is_solid(ground_id, 8, 8), true, "run made the same cell solid")
            tilemap.set_cell(ground_id, 8, 8, 4)
            t:eq(tilemap.get_cell(ground_id, 8, 8), 4, "set_cell writes any in-range id")
            tilemap.set_solid(ground_id, 8, 8, false)
            t:eq(tilemap.is_solid(ground_id, 8, 8), false, "set_solid clears independently")
            tilemap.set_cell(ground_id, 8, 8, 3)
            t:eq(tilemap.get_cell(ground_id, 8, 8), 3, "cell restored for the next step")
        end,
    },
    {
        id = "tint_meta",
        title = "Tint and meta",
        claim = "逐格 tint 是顶点色、meta 是地图自带的注释；空串会删除 meta 键（W 浇水 / H 收获）",
        api = {
            "tilemap.set_tint",
            "tilemap.tint",
            "tilemap.clear_tint",
            "tilemap.set_meta",
            "tilemap.get_meta",
            "tilemap.get_cell",
            "tilemap.atlas_index",
            "tilemap.autotile_mask",
        },
        code = "set_tint(id,0,0,.72,.84,1) / set_meta(id,0,0,'crop','wheat')",
        enter = function()
            reset_world()
        end,
        run = function(dt)
            ensure_farm()
            if live(farm_id) then
                -- 一格湿地夹在干地里：atlas_index 跟着格子值，mask 只数同 id 邻居。
                tilemap.set_cell(farm_id, 0, 0, FARM_WET)
                tilemap.set_tint(farm_id, 0, 0, TINT_WET.r, TINT_WET.g, TINT_WET.b)
                tilemap.set_meta(farm_id, 0, 0, "crop", "wheat")
                tilemap.set_meta(farm_id, 0, 0, "watered", "1")
            end
            last_action = "cell(0,0): wet art, cold tint, crop=wheat"
            lab.note(last_action)
        end,
        read = function()
            if not live(farm_id) then
                return { { "farm", "missing" } }
            end
            local wash = tilemap.tint(farm_id, 0, 0)
            return {
                { "cell(0,0)", tilemap.get_cell(farm_id, 0, 0) },
                { "atlas", tilemap.atlas_index(farm_id, 0, 0) },
                { "mask", tilemap.autotile_mask(farm_id, 0, 0) },
                { "crop", tilemap.get_meta(farm_id, 0, 0, "crop") },
                { "watered", tilemap.get_meta(farm_id, 0, 0, "watered") },
                { "tint", string.format("%.2f,%.2f,%.2f", wash.r, wash.g, wash.b) },
            }
        end,
        check = function(t)
            t:ok(live(farm_id), "farm exists for tint and meta")
            if not live(farm_id) then
                return
            end
            t:eq(tilemap.get_cell(farm_id, 0, 0), FARM_WET, "wet art is readable")
            t:eq(tilemap.atlas_index(farm_id, 0, 0), FARM_WET, "atlas index follows the cell id")
            -- 四邻都是干地 1，与当前格 2 不同 -> mask 0。
            t:eq(tilemap.autotile_mask(farm_id, 0, 0), 0, "mask counts same-id neighbours only")
            local wash = tilemap.tint(farm_id, 0, 0)
            t:near(wash.r, TINT_WET.r, 0.001, "tint red")
            t:near(wash.g, TINT_WET.g, 0.001, "tint green")
            t:near(wash.b, TINT_WET.b, 0.001, "tint blue")
            t:near(wash.a, 1.0, 0.001, "tint alpha")
            t:eq(tilemap.get_meta(farm_id, 0, 0, "crop"), "wheat", "crop meta round-trips")
            t:eq(tilemap.get_meta(farm_id, 0, 0, "watered"), "1", "watered meta round-trips")
            tilemap.set_meta(farm_id, 0, 0, "watered", "")
            t:eq(tilemap.get_meta(farm_id, 0, 0, "watered"), nil, "empty string removes the key")
            t:eq(tilemap.get_meta(farm_id, 0, 0, "crop"), "wheat", "other meta keys survive")
            tilemap.clear_tint(farm_id, 0, 0)
            local plain = tilemap.tint(farm_id, 0, 0)
            t:near(plain.r, 1.0, 0.001, "clear_tint returns to white r")
            t:near(plain.g, 1.0, 0.001, "clear_tint returns to white g")
            t:near(plain.b, 1.0, 0.001, "clear_tint returns to white b")
        end,
    },
    {
        id = "pathfinding",
        title = "A* paths",
        claim = "find_path 返回含两端的格子表或 nil,reason；对角更短（T/D/S/O/C/J 切换分支）",
        api = {
            "tilemap.find_path",
            "tilemap.is_solid",
            "tilemap.get",
            "entity.find",
        },
        code = "tilemap.find_path(id,2,2,7,7,false)",
        enter = function()
            reset_world()
        end,
        run = function(dt)
            compute_paths()
            last_action = string.format(
                "4-dir=%s diag=%s solid='%s' oob='%s'",
                straight_path and tostring(#straight_path) or "nil",
                diagonal_path and tostring(#diagonal_path) or "nil",
                solid_reason,
                oob_reason
            )
            lab.note(last_action)
        end,
        read = function()
            return {
                { "4-dir", straight_path and #straight_path or "nil" },
                { "diagonal", diagonal_path and #diagonal_path or "nil" },
                { "solid end", solid_reason },
                { "oob end", oob_reason },
                { "same cell", same_path and #same_path or "nil" },
                { "farm", farm_path and #farm_path or "nil" },
                { "start", string.format("%d,%d", WALK.x, WALK.y) },
            }
        end,
        check = function(t)
            t:ok(live(ground_id), "GroundMap exists for pathfinding")
            if not live(ground_id) then
                return
            end
            t:ok(straight_path ~= nil, "4-directional path is found")
            if straight_path ~= nil then
                -- 绕墙的唯一最短路线：先沿 y=2 往右 5 格，再沿 x=7 往上 5 格 -> 11 个格心。
                t:eq(#straight_path, 11, "4-dir shortest length")
                t:eq(straight_path[1].x, WALK.x, "path includes the start x")
                t:eq(straight_path[1].y, WALK.y, "path includes the start y")
                t:eq(straight_path[#straight_path].x, GOAL.x, "path includes the goal x")
                t:eq(straight_path[#straight_path].y, GOAL.y, "path includes the goal y")
                local clear = true
                for _, cell in ipairs(straight_path) do
                    if tilemap.is_solid(ground_id, cell.x, cell.y) then
                        clear = false
                    end
                end
                t:ok(clear, "every path cell is non-solid")
            end
            t:ok(diagonal_path ~= nil, "diagonal path is found")
            if diagonal_path ~= nil and straight_path ~= nil then
                t:ok(#diagonal_path <= #straight_path, "diagonal path is not longer")
            end
            t:ok(solid_reason:find("solid") ~= nil, "solid endpoint returns a reason")
            t:ok(oob_reason:find("out of bounds") ~= nil, "out-of-bounds endpoint returns a reason")
            t:ok(same_path ~= nil and #same_path == 1, "start == goal returns one cell")
            if farm_path ~= nil then
                t:eq(#farm_path, FARM_W + FARM_H - 1, "4x3 farm Manhattan path length")
            else
                t:ok(false, "farm path is found")
            end
        end,
    },
    {
        id = "world_sign",
        title = "Cells and world",
        claim = "格子中心经 transform.set 落到实体身上；V 把悬停格贴到 WorldText 告示牌",
        api = {
            "transform.get",
            "transform.set",
            "grid.world_to_cell",
            "grid.cell_to_world",
            "grid.facing_cell",
            "grid.in_bounds",
            "world_text.attach",
            "entity.is_alive",
        },
        code = "transform.set(walker, cell_to_world(2,2)) / world_text.attach(sign, ...)",
        enter = function()
            reset_world()
        end,
        run = function(dt)
            if live(walker_id) then
                -- 只传 x/y：rotation 与 scale 由 transform.set 保留，这也是"省略即不改"的实证。
                local p = cell_center(2, 2)
                local pos = transform.get(walker_id)
                transform.set(walker_id, p.x, p.y, pos.rotation, pos.scale_x, pos.scale_y)
            end
            attach_sign("next cell (3,3)")
            last_action = "walker on cell(2,2); sign says next cell (3,3)"
            lab.note(last_action)
        end,
        read = function()
            if not live(walker_id) then
                return { { "walker", "missing" } }
            end
            local pos = transform.get(walker_id)
            local cx, cy = cell_of_world(pos.x, pos.y)
            return {
                { "world", string.format("%.0f,%.0f", pos.x, pos.y) },
                { "cell", string.format("%d,%d", cx, cy) },
                { "scale", string.format("%.0f,%.0f", pos.scale_x, pos.scale_y) },
                { "sign", live(sign_id) and "alive" or "missing" },
                { "sign text", tostring(last_sign_text) },
            }
        end,
        check = function(t)
            t:ok(live(walker_id), "Walker exists")
            if not live(walker_id) then
                return
            end
            local pos = transform.get(walker_id)
            t:near(pos.x, MAP_POS.x + 80, 0.001, "Walker world x is the cell center")
            t:near(pos.y, MAP_POS.y + 80, 0.001, "Walker world y is the cell center")
            t:near(pos.scale_x, 1.0, 0.001, "transform.set preserved scale")
            local cx, cy = cell_of_world(pos.x, pos.y)
            t:eq(cx, 2, "world_to_cell recovers x")
            t:eq(cy, 2, "world_to_cell recovers y")
            local center = grid.cell_to_world(2, 2, TILE)
            t:near(center[1], 80, 0.001, "cell_to_world local x")
            t:near(center[2], 80, 0.001, "cell_to_world local y")
            local front = grid.facing_cell(80, 80, "up", TILE)
            t:eq(front[1], 2, "facing up keeps x")
            t:eq(front[2], 3, "facing up steps one row")
            t:eq(grid.in_bounds(2, 2, ground_w, ground_h), true, "Walker cell is in bounds")
            t:ok(live(sign_id), "Sign entity exists")
            if sign_id ~= nil then
                t:eq(entity.is_alive(sign_id), true, "Sign stays alive after attach")
            end
            t:eq(entity.find("Sign"), sign_id, "entity.find resolves Sign")
        end,
    },
}

--- 每帧鼠标悬停读数：只画自己的一块面板（tm_*），不动 lab_* 元素；
--- 面板固定在舞台右上角，永远落在 296..936 / 76..644 的取景框内。
local function draw_hover()
    if not live(ground_id) then
        ui.hide("tm_hover_text")
        ui.clear("tm_hover_text")
        ui.clear("tm_hover")
        return
    end
    local world = read_mouse_world()
    if world == nil or world.x == nil then
        ui.hide("tm_hover_text")
        ui.clear("tm_hover_text")
        ui.clear("tm_hover")
        return
    end
    local cx, cy = cell_of_world(world.x, world.y)
    if not grid.in_bounds(cx, cy, ground_w, ground_h) then
        ui.hide("tm_hover_text")
        ui.clear("tm_hover_text")
        ui.clear("tm_hover")
        return
    end
    local value = tilemap.get_cell(ground_id, cx, cy)
    local solid = tilemap.is_solid(ground_id, cx, cy)
    ui.panel("tm_hover", 664, 96, 256, 46, 0.075, 0.090, 0.130, 1.0)
    ui.text_at(
        "tm_hover_text",
        676,
        104,
        string.format("cell (%d,%d)  art %d  solid %s", cx, cy, value, tostring(solid)),
        1.0,
        12
    )
    lab.note(string.format("hover cell (%d,%d) art=%d solid=%s", cx, cy, value, tostring(solid)))
end

function M.on_create()
    -- 先绑定/摆放/建菜地，再 start：lab.start 会立刻执行第一步的 enter，那里要能读到资源。
    bind_scene()
    place_scene()
    ensure_farm()
    reset_world()
    lab.start({
        scene = "api_tilemap",
        title = "Tilemap",
        category = "2D",
        steps = steps,
        hide_scene_text = { "Hud" },
    })
    log.info(string.format("api_tilemap: tread %dx%d, runtime farm 4x3", ground_w, ground_h))
end

function M.update()
    if not lab.active() then
        return lab.tick()
    end

    -- 键位避开 lab 保留键（space/方向键/数字/r/f/f5 等），只做本模块的交互：
    -- T/D/S/O/C/J 寻路分支，E 编辑格子，W/H 浇水/收获，V 换告示牌。
    if input.just_pressed("t") then
        compute_paths()
        last_action = "4-dir A*: len=" .. tostring(straight_path and #straight_path or "nil")
    elseif input.just_pressed("d") then
        compute_paths()
        last_action = "diagonal A*: len=" .. tostring(diagonal_path and #diagonal_path or "nil")
    elseif input.just_pressed("s") then
        compute_paths()
        last_action = "solid endpoint: " .. solid_reason
    elseif input.just_pressed("o") then
        compute_paths()
        last_action = "out-of-bounds endpoint: " .. oob_reason
    elseif input.just_pressed("c") then
        compute_paths()
        last_action = "same cell path: " .. tostring(same_path and #same_path or "nil")
    elseif input.just_pressed("j") then
        compute_paths()
        last_action = "farm A*: len=" .. tostring(farm_path and #farm_path or "nil")
    elseif input.just_pressed("e") then
        toggle_ground_cell()
    elseif input.just_pressed("w") then
        water_farm()
    elseif input.just_pressed("h") then
        harvest_farm()
    elseif input.just_pressed("v") then
        attach_sign(string.format("next cell (%d,%d)", hover.cx, hover.cy))
        last_action = "WorldText attached to Sign"
    end

    draw_hover()
    if last_action ~= "ready" then
        lab.note(last_action)
    end
    return lab.tick()
end

function M.self_test()
    return lab.self_test()
end

function M.on_destroy()
    -- 只清自己的动态 UI：lab_* 归 lab.stop() 管，不能在这里 ui.clear。
    ui.hide("tm_hover_text")
    ui.clear("tm_hover_text")
    ui.clear("tm_hover")
    ground_id = nil
    walker_id = nil
    target_id = nil
    sign_id = nil
    farm_id = nil
    last_sign_text = nil
    lab.stop()
end

return M
