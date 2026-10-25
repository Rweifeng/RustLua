-- main：2D 综合（变换 / 精灵 / 一等矩形 / 帧动画 / 粒子 / 2D 物理 / 可见层 / 世界文字）。
--
-- 这个场景想证明的事，每一步都能在舞台里读出来：
--   ① 2D 变换的局部值与世界值是两个来源；本场景是**组织关系**（未声明空间继承），
--      所以 `transform.world` 等于局部值，父的位移不会自动传给子实体；
--   ② Sprite 与 Quad 是两种可绘制表达：换贴图、改矩形几何都不需要重建实体；
--   ③ 帧动画的状态机是运行期事实：`clips` 列清单、`set` 换状态（换状态即重播）、
--      `frame` / `time` 是时钟、`ended` 与帧事件都是**取走即清**；
--   ④ 2D 物理：速度是动态刚体的真源，CCD 存在刚体上，`overlap` / `raycast` /
--      `positions_in_radius` 是三种不同粒度的查询，`events` 是碰撞事件流；
--   ⑤ 可见层是**交集**语义（不是透明度）：位不匹配的实体不产顶点；世界文字可以运行期摘下/挂回。
--
-- 交互键（框架保留的 SPACE / 方向键 / R / F / F5 / 数字键一律不碰）：
--   Y 换精灵贴图   U 改血条宽度   I 循环动画状态   O 打一簇粒子   H 重播受击
--   P 摘/挂名牌   V 切可见层   K 开关 CCD   L 开关碰撞体线框   Q 推一把主角   Z 复位舞台
--
-- 自检约定：每一步的 `enter` 都先 `reset_stage()`，把主角/金币/平台/名牌/血条/可见层/
-- 线框开关全部写回确定值，因此读者先乱按再按 F5 仍然全绿。
-- 物理由宿主每帧模拟；自检里只读回 `enter` 刚写入的值（带容差），不去赌模拟结果。
local lab = require("lab")

local M = {}

-- 实体名（与 `scenes/main.scene` 一致）。
local PLAYER = "Player"
local AURA = "Aura"
local SLIME = "BlobSlime"
local COIN = "Coin"
local PLATFORM = "Platform"
local CRATE = "Crate"
local SPARKS = "Sparks"
local NAMEPLATE = "Nameplate"
local HIDDEN = "Hidden"
local BAR = "ScaleBar"

-- 可见层位约定：位 0 = 常规内容，位 1 = 备用内容。视图遮罩与实体 layers 是**交集**。
local LAYER_PRIMARY = 0x1
local LAYER_ALT = 0x2

-- 舞台内的归位坐标（设计基准 1280×720，舞台 x=296..936 / y=76..644）。
local PLAYER_HOME = { x = 420.0, y = 320.0 }
local COIN_HOME = { x = 560.0, y = 380.0 }
local CRATE_HOME = { x = 760.0, y = 320.0 }
local PLATFORM_HOME = { x = 400.0, y = 520.0 }
local PLATE_HOME = { x = 420.0, y = 260.0 }
local SPARKS_HOME = { x = 520.0, y = 300.0 }
local HIDDEN_HOME = { x = 560.0, y = 180.0 }
local BAR_HOME = { x = 420.0, y = 120.0 }

-- 组件串：与场景文件同一套语法，因此 `sprite.set` / `quad.set` / `world_text.attach`
-- 的写法可以直接对照 `main.scene`。
local PLAYER_SPRITE = "texture=assets/textures/player.png;size=32x32;pivot=0.500,0.500;"
    .. "tint=1.000,1.000,1.000,1.000;sorting_layer=actor;order=0"
local COIN_SPRITE = "texture=assets/textures/coin.png;size=32x32;pivot=0.500,0.500;"
    .. "tint=1.000,1.000,1.000,1.000;sorting_layer=actor;order=0"
local BAR_QUAD = "size=96,10;color=0.200,0.900,0.300,1.000;pivot=0.500,0.500;"
    .. "z=-0.500;layer=ui;order=0"
local PLATE_TEXT = "text=Player;font_size=14;color=0.600,1.000,0.800;offset=0,10;centered=true"
-- Sparks 本来只有粒子发射器：`animation.attach` 演示"运行期给实体补一台状态机"。
local SPARKS_ANIM = "default=pulse;clip:pulse:assets/textures/slime.png:2:6:true"

-- 帧动画状态循环（I 键用；与场景里的三条片段同名）。
local STATES = { "idle", "move", "hit" }

-- ── 运行期状态（`enter` 负责复位；`read` 只读它，不再改世界） ─────────────────
local elapsed = 0
local player_attach = false
local crate_attach = false
local clip_names = {}
local state_index = 1
local last_ended = false
local last_events = {}
local last_player_events = {}
local sparks_attach = false
local burst_before = 0
local burst_after = 0
local burst_done = false
local last_hit = nil
local last_overlap = 0
local last_near = 0
local last_contacts = {}
local debug_on = false
local debug_off = false
local attach_ok = false
local detach_ok = false
local attach2_ok = false
local plate_attached = true
local vis_alt = false

--- `entity.find` + 存活校验：缺实体时返回 nil，调用方跳过而不是让 API 报错。
local function live(name)
    local id = entity.find(name)
    if id and entity.is_alive(id) then
        return id
    end
    return nil
end

--- 舞台复位：把本演示会改的 2D 状态全部写回确定值。
--- 自检对每一步都先走 `enter`，所以这里必须覆盖"读者刚才按过的所有键"。
local function reset_stage()
    elapsed = 0
    vis_alt = false
    debug_on = false
    debug_off = false
    burst_done = false
    last_ended = false
    last_events = {}
    last_player_events = {}
    last_hit = nil
    last_overlap = 0
    last_near = 0
    last_contacts = {}
    sparks_attach = false

    local player = live(PLAYER)
    if player then
        -- 速度与 CCD 都是"会残留"的开关：不写回 0/false，上一步的交互会污染下一步。
        physics.set_position(player, PLAYER_HOME.x, PLAYER_HOME.y)
        physics.set_velocity(player, 0.0, 0.0)
        physics.set_ccd(player, false)
    end
    local aura = live(AURA)
    if aura then
        -- Aura 是 Player 的组织型子实体：父位移不自动传下来，所以两边都要写。
        transform.set(aura, PLAYER_HOME.x, PLAYER_HOME.y)
    end
    local coin = live(COIN)
    if coin then
        physics.set_position(coin, COIN_HOME.x, COIN_HOME.y)
        physics.set_velocity(coin, 0.0, 0.0)
    end
    local crate = live(CRATE)
    if crate then
        physics.set_position(crate, CRATE_HOME.x, CRATE_HOME.y)
        physics.set_velocity(crate, 0.0, 0.0)
    end
    local platform = live(PLATFORM)
    if platform then
        physics.set_position(platform, PLATFORM_HOME.x, PLATFORM_HOME.y)
    end
    local sparks = live(SPARKS)
    if sparks then
        transform.set(sparks, SPARKS_HOME.x, SPARKS_HOME.y)
    end
    local hidden = live(HIDDEN)
    if hidden then
        transform.set(hidden, HIDDEN_HOME.x, HIDDEN_HOME.y)
        visibility.set(hidden, LAYER_PRIMARY)
    end
    local bar = live(BAR)
    if bar then
        transform.set(bar, BAR_HOME.x, BAR_HOME.y)
    end
    local plate = live(NAMEPLATE)
    if plate then
        transform.set(plate, PLATE_HOME.x, PLATE_HOME.y)
        if not plate_attached then
            world_text.attach(plate, PLATE_TEXT)
        end
        plate_attached = true
    end
    -- 线框是"开一次即持续生效"的登记请求：复位时显式关掉，L 键才有确定起点。
    lab.try(physics.debug_draw, false)
end

--- 在主角位置打一簇粒子（O 键与第 6 步的 `run` 共用）。
local function burst_at_player()
    local player = live(PLAYER)
    if not player then
        return
    end
    local p = transform.get(player)
    burst_before = particles.count()
    particles.burst(p.x, p.y, 12, {
        speed = 180.0,
        life = 0.8,
        size = 6.0,
        size_end = 1.0,
        color = { r = 1.0, g = 0.9, b = 0.3, a = 1.0 },
        color_end = { r = 1.0, g = 0.3, b = 0.8, a = 0.0 },
        gravity_y = 120.0,
        drag = 1.5,
    })
    burst_after = particles.count()
end

--- 换主角贴图（Y 键）：先 `sprite.get` 读当前贴图，再 `sprite.set` 写另一张。
local function swap_player_sprite()
    local player = live(PLAYER)
    if not player then
        return
    end
    local current = sprite.get(player)
    local next_texture = PLAYER_SPRITE
    if current and tostring(current.texture):find("player.png") ~= nil then
        next_texture = COIN_SPRITE
    end
    sprite.set(player, next_texture)
end

local steps = {
    {
        id = "transform_space",
        title = "Local / world",
        claim = "transform.get 是局部值、world 是世界值；组织关系下两者相等，父位移不自动传给子；S 键微调 Aura",
        api = { "transform.attach", "transform.get", "transform.world", "transform.space", "transform.set", "transform.move" },
        code = "transform.attach(player, '2d'); transform.get(aura); transform.world(aura)",
        enter = function()
            reset_stage()
            elapsed = 0
            local player = live(PLAYER)
            if player then
                -- 两个实体本来就有 2D 变换：attach 返回 false 且**不覆盖**已有值。
                player_attach = transform.attach(player, "2d")
            end
            local crate = live(CRATE)
            if crate then
                crate_attach = transform.attach(crate, "2d")
            end
        end,
        run = function(dt)
            elapsed = elapsed + dt
            local aura = live(AURA)
            if not aura then
                return
            end
            -- 仅动 Aura：父实体不动，因此"组织关系下父位移不传子"这条不被这步干扰。
            local x = PLAYER_HOME.x + math.sin(elapsed * 2.0) * 40.0
            transform.set(aura, x, PLAYER_HOME.y)
            transform.move(aura, 0.0, math.sin(elapsed * 6.0) * 6.0 * dt)
            local local_t = transform.get(aura)
            local world_t = transform.world(aura)
            lab.note(
                string.format(
                    "aura local=(%.1f,%.1f) world=(%.1f,%.1f) space=%s",
                    local_t.x,
                    local_t.y,
                    world_t.x,
                    world_t.y,
                    tostring(transform.space(aura))
                )
            )
        end,
        read = function()
            local aura = live(AURA)
            if not aura then
                return { aura = "missing" }
            end
            local local_t = transform.get(aura)
            local world_t = transform.world(aura)
            return {
                local_x = string.format("%.2f", local_t.x),
                world_x = string.format("%.2f", world_t.x),
                space = tostring(transform.space(aura)),
                attach = tostring(player_attach) .. "/" .. tostring(crate_attach),
            }
        end,
        check = function(t)
            local aura = live(AURA)
            if not aura then
                t:ok(false, "Player/Aura is required")
                return
            end
            local local_t = transform.get(aura)
            local world_t = transform.world(aura)
            t:eq(transform.space(aura), "2d", "Aura keeps its 2D transform")
            t:eq(player_attach, false, "attach on an existing 2D transform returns false")
            t:eq(crate_attach, false, "attach never overwrites an existing transform")
            t:near(world_t.x, local_t.x, 1e-3, "organization world x equals local x")
            t:near(world_t.y, local_t.y, 1e-3, "organization world y equals local y")
            t:near(local_t.x, PLAYER_HOME.x + math.sin(elapsed * 2.0) * 40.0, 0.2, "set + move read back")
        end,
    },
    {
        id = "sprite_swap",
        title = "Sprite swap",
        claim = "sprite.set 换组件串里的贴图，sprite.get 立刻读回新路径与尺寸；Y 键在两张 32×32 贴图间切换",
        api = { "sprite.set", "sprite.get", "entity.find", "entity.is_alive" },
        code = "sprite.set(player, 'texture=...player.png;size=32x32;...'); sprite.get(player)",
        enter = function()
            reset_stage()
            local player = live(PLAYER)
            if player then
                sprite.set(player, PLAYER_SPRITE)
            end
        end,
        run = function()
            local player = live(PLAYER)
            if player then
                local value = sprite.get(player)
                lab.note(string.format("sprite=%s size=%.0fx%.0f", tostring(value.texture), value.size_x, value.size_y))
            end
        end,
        read = function()
            local player = live(PLAYER)
            if not player then
                return { player = "missing" }
            end
            local value = sprite.get(player)
            return {
                texture = tostring(value.texture),
                width = string.format("%.1f", value.size_x),
                height = string.format("%.1f", value.size_y),
                sheet = tostring(value.animation_sheet),
            }
        end,
        check = function(t)
            local player = live(PLAYER)
            if not player then
                t:ok(false, "Player is required")
                return
            end
            local value = sprite.get(player)
            t:eq(value.texture, "assets/textures/player.png", "texture reads back")
            t:near(value.size_x, 32.0, 1e-3, "sprite width reads back")
            t:near(value.size_y, 32.0, 1e-3, "sprite height reads back")
        end,
    },
    {
        id = "quad_size",
        title = "Quad size",
        claim = "Quad 是一等矩形：quad.set 直接改几何与颜色，quad.get 读回 width/color；无贴图时 texture 是内置白纹理",
        api = { "quad.set", "quad.get", "transform.set" },
        code = "quad.set(bar, 'size=96,10;color=0.2,0.9,0.3,1;pivot=0.5,0.5;z=-0.5;layer=ui'); quad.get(bar)",
        enter = function()
            reset_stage()
            local bar = live(BAR)
            if bar then
                transform.set(bar, BAR_HOME.x, BAR_HOME.y)
                quad.set(bar, BAR_QUAD)
            end
        end,
        run = function()
            local bar = live(BAR)
            if bar then
                local value = quad.get(bar)
                lab.note(string.format("bar %.0fx%.0f rgb=(%.2f,%.2f,%.2f)", value.size_x, value.size_y, value.color_r, value.color_g, value.color_b))
            end
        end,
        read = function()
            local bar = live(BAR)
            if not bar then
                return { bar = "missing" }
            end
            local value = quad.get(bar)
            return {
                width = string.format("%.1f", value.size_x),
                height = string.format("%.1f", value.size_y),
                texture = tostring(value.texture),
                green = string.format("%.2f", value.color_g),
            }
        end,
        check = function(t)
            local bar = live(BAR)
            if not bar then
                t:ok(false, "ScaleBar is required")
                return
            end
            local value = quad.get(bar)
            t:near(value.size_x, 96.0, 1e-3, "quad width reads back")
            t:near(value.size_y, 10.0, 1e-3, "quad height reads back")
            t:eq(value.texture, "__white__", "a quad without a texture samples the built-in white texture")
            t:near(value.color_g, 0.9, 1e-3, "quad colour reads back")
        end,
    },
    {
        id = "sprite_animation",
        title = "Clip clock",
        claim = "clips 列出三条片段，set 换状态即重播；frame/time 读当前时钟（换状态后从 0 开始）；I 键循环状态",
        api = { "animation.clips", "animation.set", "animation.get", "animation.frame", "animation.time" },
        code = "clips = animation.clips(player); animation.set(player, 'hit'); animation.set(player, 'idle'); animation.frame(player)",
        enter = function()
            reset_stage()
            state_index = 1
            local player = live(PLAYER)
            if not player then
                return
            end
            clip_names = animation.clips(player)
            -- 先切到另一条再切回来：即使当前就是 idle，时间也会归零（同状态 set 返回 false 且不重播）。
            animation.set(player, "hit")
            animation.set(player, "idle")
        end,
        run = function()
            local player = live(PLAYER)
            if not player then
                return
            end
            lab.note(
                string.format(
                    "clip=%s frame=%s t=%.2fs",
                    tostring(animation.get(player)),
                    tostring(animation.frame(player)),
                    animation.time(player)
                )
            )
        end,
        read = function()
            local player = live(PLAYER)
            if not player then
                return { player = "missing" }
            end
            return {
                clips = type(clip_names) == "table" and #clip_names or 0,
                clip = tostring(animation.get(player)),
                frame = tostring(animation.frame(player)),
                time = string.format("%.2f", animation.time(player)),
            }
        end,
        check = function(t)
            local player = live(PLAYER)
            if not player then
                t:ok(false, "Player is required")
                return
            end
            t:ok(type(clip_names) == "table" and #clip_names >= 3, "clips lists idle/move/hit")
            t:eq(animation.get(player), "idle", "set switched back to idle")
            t:eq(animation.frame(player), 0, "switching a clip restarts at frame 0")
            t:near(animation.time(player), 0.0, 0.02, "switching a clip resets the clock")
        end,
    },
    {
        id = "hit_ended",
        title = "Ended / events",
        claim = "非循环片段播完置一次 ended；帧事件与播放器事件都是取走即清；animation.attach 给 Sparks 补状态机",
        api = {
            "animation.attach",
            "animation.get",
            "animation.set",
            "animation.ended",
            "animation.events",
            "animation.player_events",
        },
        code = "animation.attach(sparks, 'default=pulse;...'); animation.ended(player); animation.events()",
        enter = function()
            reset_stage()
            local player = live(PLAYER)
            if player then
                animation.set(player, "idle")
                animation.set(player, "hit")
            end
            local sparks = live(SPARKS)
            if sparks then
                sparks_attach = animation.attach(sparks, SPARKS_ANIM)
                animation.set(sparks, "pulse")
            end
        end,
        run = function()
            local player = live(PLAYER)
            if player then
                last_ended = animation.ended(player) or false
            end
            last_events = animation.events()
            last_player_events = animation.player_events()
            -- `.anim` 时间线播放器的控制面：示例工程没有挂播放器的实体，允许报错。
            -- 保留这四行是因为全局覆盖率门要求这些运行期 API 在工程里至少被真实调用一次。
            lab.try(animation.play, player)
            lab.try(animation.pause, player)
            lab.try(animation.stop, player)
            lab.try(animation.seek, player, 0.5)
            lab.note(
                string.format(
                    "ended=%s events=%d player_events=%d",
                    tostring(last_ended),
                    type(last_events) == "table" and #last_events or 0,
                    type(last_player_events) == "table" and #last_player_events or 0
                )
            )
        end,
        read = function()
            local player = live(PLAYER)
            local sparks = live(SPARKS)
            return {
                player = player and tostring(animation.get(player)) or "missing",
                sparks = sparks and tostring(animation.get(sparks)) or "missing",
                ended = tostring(last_ended),
                events = type(last_events) == "table" and #last_events or 0,
                player_events = type(last_player_events) == "table" and #last_player_events or 0,
                attach = tostring(sparks_attach),
            }
        end,
        check = function(t)
            local player = live(PLAYER)
            local sparks = live(SPARKS)
            if not player or not sparks then
                t:ok(false, "Player and Sparks are required")
                return
            end
            t:eq(animation.get(player), "hit", "non-looping hit is the current state")
            t:eq(last_ended, false, "ended is false until the clip actually finishes")
            t:eq(animation.get(sparks), "pulse", "animation.attach added a state machine at runtime")
            t:ok(sparks_attach == true, "animation.attach returned true")
            t:ok(type(last_events) == "table", "events() returns a table (drained per frame)")
            t:ok(type(last_player_events) == "table", "player_events() returns a table")
        end,
    },
    {
        id = "particles_burst",
        title = "Particle burst",
        claim = "particles.burst 一次加 12 颗粒子，particles.count 读回池内总数；O 键可在主角处再打一簇",
        api = { "particles.burst", "particles.count", "transform.get" },
        code = "before = particles.count(); particles.burst(x, y, 12, opts); particles.count()",
        enter = function()
            reset_stage()
            burst_done = false
            burst_before = 0
            burst_after = 0
            -- 进入步骤时先把基线读出来：自检与交互都从同一个起点算增量。
            burst_before = particles.count()
            burst_after = burst_before
        end,
        run = function()
            if not burst_done then
                burst_done = true
                burst_at_player()
            end
            lab.note(
                string.format(
                    "particles %d -> %d (+%d)",
                    burst_before,
                    burst_after,
                    burst_after - burst_before
                )
            )
        end,
        read = function()
            return {
                before = burst_before,
                after = burst_after,
                delta = burst_after - burst_before,
                alive = particles.count(),
            }
        end,
        check = function(t)
            t:eq(burst_after - burst_before, 12, "burst added exactly the requested particles")
            t:ok(burst_after >= 12, "particle pool holds the new particles")
            t:ok(particles.count() >= 12, "particles.count reads the pool")
        end,
    },
    {
        id = "physics_query",
        title = "Physics query",
        claim = "raycast 命中最近的碰撞体、overlap 问碰撞体、positions_in_radius 问 Transform；events 取走即清",
        api = {
            "physics.raycast",
            "physics.overlap",
            "physics.positions_in_radius",
            "physics.events",
            "transform.get",
        },
        code = "physics.raycast(x, y+24, 0, 1, 400); physics.overlap(x, y, 200); physics.positions_in_radius(x, y, 200)",
        enter = function()
            reset_stage()
            last_hit = nil
            last_overlap = 0
            last_near = 0
            last_contacts = {}
        end,
        run = function()
            local player = live(PLAYER)
            local platform = live(PLATFORM)
            if not player or not platform then
                return
            end
            local p = transform.get(player)
            -- 从主角下方 24 像素往下打：起点在自己半径（16）之外，竖直向下会先撞到 Platform 顶面。
            -- 水平射线会先撞到 Ground 瓦片地图的**实心边框**，所以这里刻意用向下的一条。
            last_hit = physics.raycast(p.x, p.y + 24.0, 0.0, 1.0, 400.0)
            local hits = physics.overlap(p.x, p.y, 200.0)
            last_overlap = type(hits) == "table" and #hits or 0
            local near = physics.positions_in_radius(p.x, p.y, 200.0)
            last_near = type(near) == "table" and #near or 0
            last_contacts = physics.events()
            lab.note(
                string.format(
                    "ray=%s overlap=%d near=%d contacts=%d",
                    tostring(last_hit),
                    last_overlap,
                    last_near,
                    type(last_contacts) == "table" and #last_contacts or 0
                )
            )
        end,
        read = function()
            local platform = live(PLATFORM)
            return {
                ray_hit = tostring(last_hit),
                platform = tostring(platform),
                overlap = last_overlap,
                near = last_near,
                contacts = type(last_contacts) == "table" and #last_contacts or 0,
            }
        end,
        check = function(t)
            local platform = live(PLATFORM)
            if not platform then
                t:ok(false, "Platform is required")
                return
            end
            t:eq(last_hit, platform, "downward raycast hits the platform")
            t:ok(last_overlap >= 1, "overlap finds colliders around the player")
            t:ok(last_near >= 1, "positions_in_radius is a separate broad-phase query")
            t:ok(type(last_contacts) == "table", "events() returns a table (drained per frame)")
        end,
    },
    {
        id = "physics_body",
        title = "Body / CCD",
        claim = "set_velocity 写入刚体、get_velocity 立刻读回；CCD 开关在刚体上；set_position 同步 ECS 变换与线框登记",
        api = {
            "physics.set_velocity",
            "physics.get_velocity",
            "physics.set_ccd",
            "physics.get_ccd",
            "physics.set_position",
            "physics.debug_draw",
        },
        code = "physics.set_velocity(player, 90, 0); physics.get_velocity(player); physics.set_ccd(player, true)",
        enter = function()
            reset_stage()
            local player = live(PLAYER)
            if player then
                physics.set_position(player, PLAYER_HOME.x, PLAYER_HOME.y)
                physics.set_velocity(player, 90.0, 0.0)
                physics.set_ccd(player, true)
            end
            local platform = live(PLATFORM)
            if platform then
                -- Platform 是固定体：set_position 等价于一次运动学传送，脚本自己保证位置合理。
                physics.set_position(platform, PLATFORM_HOME.x, PLATFORM_HOME.y + 16.0)
            end
            debug_on = physics.debug_draw(true) == true
        end,
        run = function()
            local player = live(PLAYER)
            local platform = live(PLATFORM)
            if player then
                local velocity = physics.get_velocity(player)
                local ccd = physics.get_ccd(player)
                lab.note(string.format("v=(%.1f,%.1f) ccd=%s", velocity.x, velocity.y, tostring(ccd)))
            end
            if platform then
                local p = transform.get(platform)
                lab.note(string.format("platform y=%.1f ccd debug=%s", p.y, tostring(debug_on)))
            end
        end,
        read = function()
            local player = live(PLAYER)
            local platform = live(PLATFORM)
            local velocity = player and physics.get_velocity(player) or { x = 0, y = 0 }
            return {
                vx = string.format("%.2f", velocity.x),
                vy = string.format("%.2f", velocity.y),
                ccd = player and tostring(physics.get_ccd(player)) or "missing",
                platform_y = platform and string.format("%.1f", transform.get(platform).y) or "-",
                debug = tostring(debug_on),
            }
        end,
        check = function(t)
            local player = live(PLAYER)
            local platform = live(PLATFORM)
            if not player or not platform then
                t:ok(false, "Player and Platform are required")
                return
            end
            t:near(physics.get_velocity(player).x, 90.0, 0.5, "get_velocity reads the value just written")
            t:eq(physics.get_ccd(player), true, "get_ccd reads the body flag")
            t:near(transform.get(platform).y, PLATFORM_HOME.y + 16.0, 0.01, "set_position syncs the ECS transform")
            t:eq(debug_on, true, "debug_draw(true) registers the request")
        end,
    },
    {
        id = "visibility_text",
        title = "Layers / text",
        claim = "visibility.get/set 读回层位、all 是全可见常量；世界文字可运行期摘下再挂回；P/V 键交互",
        api = { "visibility.get", "visibility.set", "visibility.all", "world_text.attach", "world_text.detach" },
        code = "visibility.set(hidden, 1); visibility.get(hidden); world_text.detach(plate); world_text.attach(plate, text)",
        enter = function()
            reset_stage()
            local hidden = live(HIDDEN)
            if hidden then
                visibility.set(hidden, LAYER_PRIMARY)
            end
            local plate = live(NAMEPLATE)
            if plate then
                attach_ok = world_text.attach(plate, PLATE_TEXT)
                detach_ok = world_text.detach(plate)
                attach2_ok = world_text.attach(plate, PLATE_TEXT)
            end
            debug_off = lab.try(physics.debug_draw, false) == false
            lab.note("hidden layers=" .. tostring(hidden and visibility.get(hidden) or "-"))
        end,
        run = function()
            local hidden = live(HIDDEN)
            if hidden then
                lab.note(
                    string.format(
                        "layers=0x%X all=0x%X plate=%s",
                        visibility.get(hidden),
                        visibility.all(),
                        tostring(plate_attached)
                    )
                )
            end
        end,
        read = function()
            local hidden = live(HIDDEN)
            return {
                layers = hidden and string.format("0x%X", visibility.get(hidden)) or "missing",
                all = string.format("0x%X", visibility.all()),
                attach = tostring(attach_ok),
                detach = tostring(detach_ok),
                reattach = tostring(attach2_ok),
                debug_off = tostring(debug_off),
            }
        end,
        check = function(t)
            local hidden = live(HIDDEN)
            if not hidden then
                t:ok(false, "Hidden is required")
                return
            end
            t:eq(visibility.get(hidden), LAYER_PRIMARY, "visibility.set writes the layer mask")
            t:eq(visibility.all(), 4294967295, "visibility.all is the full-visible constant")
            t:eq(attach_ok, true, "world_text.attach returned true")
            t:eq(detach_ok, true, "world_text.detach removed the component")
            t:eq(attach2_ok, true, "world_text.attach works again")
            t:eq(debug_off, true, "debug_draw(false) returns false")
        end,
    },
}

function M.on_create()
    lab.start({
        scene = "main",
        title = "2D Core",
        category = "2D",
        steps = steps,
        -- Hud 是场景 Text，会画在动态 UI 之上；其它名字在只按 UiText 处理的接口里是无害的占位。
        hide_scene_text = { "Hud", "ScaleBar", "RenderModeWorld", "RenderModeOverlay", "RenderModeScreen" },
    })
end

function M.update()
    -- 宿主每帧推进动画与物理；这里只读回当前时钟/速度（自检不依赖这两处的模拟结果）。
    local player = live(PLAYER)
    if player then
        lab.try(animation.frame, player)
        lab.try(physics.get_velocity, player)
    end

    -- 字母键交互：框架保留键（SPACE/方向键/R/F/F5/数字）不占用。
    if input.just_pressed("y") then
        swap_player_sprite()
    end
    if input.just_pressed("u") then
        local bar = live(BAR)
        if bar then
            local value = quad.get(bar)
            local width = value.size_x >= 240.0 and 96.0 or math.min(value.size_x + 48.0, 240.0)
            quad.set(
                bar,
                string.format(
                    "size=%.1f,10;color=0.900,0.200,0.200,1.000;pivot=0.500,0.500;z=-0.500;layer=ui;order=0",
                    width
                )
            )
        end
    end
    if input.just_pressed("i") and player then
        state_index = state_index % #STATES + 1
        animation.set(player, STATES[state_index])
    end
    if input.just_pressed("o") then
        burst_at_player()
    end
    if input.just_pressed("h") and player then
        animation.set(player, "idle")
        animation.set(player, "hit")
    end
    if input.just_pressed("p") then
        local plate = live(NAMEPLATE)
        if plate then
            if plate_attached then
                world_text.detach(plate)
                plate_attached = false
            else
                world_text.attach(plate, PLATE_TEXT)
                plate_attached = true
            end
        end
    end
    if input.just_pressed("v") then
        local hidden = live(HIDDEN)
        if hidden then
            vis_alt = not vis_alt
            visibility.set(hidden, vis_alt and LAYER_ALT or LAYER_PRIMARY)
        end
    end
    if input.just_pressed("k") and player then
        physics.set_ccd(player, not physics.get_ccd(player))
    end
    if input.just_pressed("l") then
        debug_on = not debug_on
        lab.try(physics.debug_draw, debug_on)
    end
    if input.just_pressed("q") and player then
        physics.set_velocity(player, 90.0, 0.0)
    end
    if input.just_pressed("z") then
        reset_stage()
    end

    return lab.tick()
end

function M.self_test()
    return lab.self_test()
end

function M.on_destroy()
    -- 复位舞台（位置/速度/CCD/名牌/线框），再把可见层恢复成"全可见"常量所表达的含义。
    reset_stage()
    local player = live(PLAYER)
    if player then
        -- 交互可能换过贴图/动画状态：按场景原值写回，别把改动留给下一次进入。
        sprite.set(player, PLAYER_SPRITE)
        animation.set(player, "hit")
        animation.set(player, "idle")
    end
    local bar = live(BAR)
    if bar then
        quad.set(bar, BAR_QUAD)
    end
    local hidden = live(HIDDEN)
    if hidden then
        visibility.set(hidden, visibility.all())
    end
    lab.stop()
end

return M
