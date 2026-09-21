-- demo_main：`scenes/main.scene` 的演示模块（2D 综合）。
--
-- 这个文件想证明什么：**2D 那一整套 Lua API 在同一个场景里可以互相配合**——变换与父子、
-- 精灵与一等矩形、状态机动画、粒子、世界文字、可见层遮罩、物理查询与调试绘制、屏幕 UI。
-- 读代码的方式：每个 `demo_*` 函数对应一个 API 模块，函数头注释写"看什么"；
-- 按键分工见 `on_create` 里的 `api.hint`。第一个固定步会跑一遍 `demo_sweep`，
-- 因此**没有输入**（headless / 没人按键）时全部 API 也都被真的调用过一次。
--
-- 硬规则：顶层只定义函数与常量，不调用任何引擎 API——模块加载时实体还没生成、宿主也还没
-- 回填相机，那些调用会直接报错。所有引擎调用都发生在 `on_create()` / `update()` 里。
local api = require("api_demo")

local M = {}

-- ── 常量：场景里的实体名（场景文件是真源，名字写错时下面的 need() 会报出来） ──────────
local PLAYER = "Player"
local AURA = "Player/Aura"
local SLIME = "BlobSlime"
local COIN = "Coin"
local PLATFORM = "Platform"
local CRATE = "Crate"
local SPARKS = "Sparks"
local NAMEPLATE = "Nameplate"
local HIDDEN = "Hidden"

-- 可见层的位约定（实体侧与视图侧用同一套位：位 0 = 常规内容，位 1 = "备用"内容）。
-- 这里刻意演示 `Visibility` 的语义是**交集**：实体 layers=0x1 且视图遮罩=0x2 时，
-- 那个实体"不存在于这个视图"，而不是"变透明"。
local LAYER_PRIMARY = 0x1
local LAYER_ALT = 0x2

-- ── 局部状态（模块级：一个场景一份，热重载会重建） ────────────────────────────────
local registered = false -- 本模块的标题/提示是否已登记（防止 on_create 被多次调用时重复登记）
local auto_done = false  -- 首帧的"全 API 走一遍"是否已跑过
local cycle_index = 0    -- 轮播（键 G）：变换与形象交替演示
local last_note = "(first frame runs every API once)" -- 最近一次按键干了什么
local vis_hidden = false -- Hidden 实体当前是否被脚本遮掉
local plate_attached = true -- Nameplate 的世界文字当前是否还挂着
local sprite_swapped = false -- Player 的贴图是否已被换成另一张
local burst_count = 0    -- 脚本累计爆发请求次数

-- ── 取实体 id 的小工具 ────────────────────────────────────────────────────────
--- id 门：拿到实体 id 或直接跳过这次演示（场景被改名时宁可少演示一步，也不要报错中断）。
local function id_of(name)
    local id = entity.find(name)
    if id == nil then
        return nil
    end
    return id
end

--- 带名字的 id 门：用于演示函数的第一行，缺实体时返回 nil 并留下可读的原因。
local function need(name)
    local id = id_of(name)
    if id == nil then
        last_note = "missing entity " .. name
    end
    return id
end

--- 安全调用（**宿主帧依赖**的接口用 api.try 包一层）。
--
-- 为什么包成匿名函数：`api.try(fn, ...)` 内部是 `pcall(fn, ...)`，而引擎的 API 表成员是
-- 无 self 的普通函数，直接传 `camera2d.get` 没有问题；包一层是为了**统一入口**——
-- 调用点一眼能看出"这一次调用允许失败，失败原因会被写进状态行"，而不是静默吞掉。
local function safe_call(fn, ...)
    local args = { ... }
    -- 先数一遍参数：`select('#', ...)` 会把 nil 也算进去，这里刻意用显式计数避免
    -- "最后一个参数是 nil 时参数被截断"这类隐蔽问题。
    local count = select("#", ...)
    if count == 0 then
        return api.try(function()
            return fn()
        end)
    end
    if count == 1 then
        local first = args[1]
        return api.try(function()
            return fn(first)
        end)
    end
    return api.try(function()
        return fn(args[1], args[2], args[3])
    end)
end

local function num(value)
    if type(value) ~= "number" then
        return "?"
    end
    return string.format("%.1f", value)
end

local function origin_of(id, name)
    if name ~= nil then
        id = id_of(name)
    end
    if id == nil then
        return 0, 0
    end
    local ok, x, y = pcall(transform.get, id)
    if not ok or type(x) ~= "table" then
        return 0, 0
    end
    return x.x or 0, x.y or 0
end

-- ── 1. 变换：get / set / move / attach / space / world ────────────────────────
-- 看什么：Player 与它的子实体 Aura 的**局部值 vs 世界值**。
-- 这一对是"组织关系"的父子（场景文件里解释了为什么这份发布场景不能写空间偏移）：
-- Aura 自己也写着与世界一致的坐标，因此 `transform.get` 与 `transform.world` 读出来相等；
-- 更值得看的是**父的位移不会自动传给子**——这正是组织关系与空间继承的区别。
-- `transform.attach(id, "2d")` 给缺 2D 变换的实体补一个（已存在的返回 false，不覆盖），
-- `transform.space` 报出实体当前按哪个空间解释。
local function demo_transform()
    local player = need(PLAYER)
    local aura = need(AURA)
    if player == nil or aura == nil then
        return
    end

    -- 6-1 get：局部值（父子关系下这是"相对父"的坐标）
    local local_aura = transform.get(aura)
    -- 6-2 world：世界值（屏幕上实际在哪）。组织关系下两者相等——差别在**空间继承**打开时才出现
    -- （`meta spatial local` 的工程里，`world` 是父链组合的结果而 `get` 只是自己的局部值）。
    local world_aura = transform.world(aura)
    -- 6-3 space：这个实体现在按哪一套空间解释（2D 的 Transform 还是 3D 的 Transform3D）
    local space_player = transform.space(player)
    local space_aura = transform.space(aura)

    -- 6-4 set：整体写位置（rotation / scale 省略 = 保持原值）。
    -- Aura 是 Player 的子实体，但它自己那份坐标就是它的世界坐标（见场景文件第 ③ 条的取舍），
    -- 所以这里**两侧都要写**：父位移不会自动传给子，这一点正是组织关系与空间继承的区别。
    local world_player = transform.world(player)
    local world_aura = transform.world(aura)
    transform.set(player, world_player.x + 24, world_player.y)
    transform.set(aura, world_aura.x + 24, world_aura.y)

    -- 6-5 move：增量移动（相对当前位置）——它只动自己，不动子实体。
    transform.move(player, 0, 4)

    -- 6-6 attach：缺 2D 变换时补一个，已存在返回 false（**不覆盖**已有值）。
    local attached_player = transform.attach(player, "2d")
    local attached_crate = transform.attach(CRATE, "2d")

    local after_local = transform.get(aura)
    local after_world = transform.world(aura)
    last_note = string.format(
        "transform local=(%s,%s) world=(%s,%s) space=%s/%s attach player=%s crate=%s",
        num(after_local.x),
        num(after_local.y),
        num(after_world.x),
        num(after_world.y),
        tostring(space_player),
        tostring(space_aura),
        tostring(attached_player),
        tostring(attached_crate)
    )
    return last_note
end

-- ── 2. 精灵：sprite.get / sprite.set ──────────────────────────────────────────
-- 看什么：同一个实体换一张 32×32 的贴图，`sprite.get` 读回的 `texture` 立刻跟着变。
-- 注意 `animation_sheet`：带 Animation 的实体渲染走的是**当前片段**的图集，
-- 所以"贴图换没换"要看 texture 与 animation_sheet 两个字段，别只看一个。
local function demo_sprite()
    local player = need(PLAYER)
    if player == nil then
        return
    end

    local before = sprite.get(player)
    sprite_swapped = not sprite_swapped
    local texture = "assets/textures/player.png"
    if sprite_swapped then
        texture = "assets/textures/coin.png"
    end
    -- sprite.set 收的是**组件串**（与场景文件里的写法同一套语法），size 用 `宽x高`。
    sprite.set(
        player,
        string.format(
            "texture=%s;size=32x32;pivot=0.500,0.500;tint=1.000,1.000,1.000,1.000;sorting_layer=actor;order=0",
            texture
        )
    )
    local after = sprite.get(player)
    last_note = string.format(
        "sprite.get texture %s -> %s size=%sx%s sheet=%s",
        tostring(before.texture),
        tostring(after.texture),
        num(after.size_x),
        num(after.size_y),
        tostring(after.animation_sheet)
    )
    return last_note
end

-- ── 3. 一等矩形：quad.get / quad.set ─────────────────────────────────────────
-- 看什么：ScaleBar 本来是一根 96×10 的绿条，被改成"血条"（每按一次长 24 像素、红底、layer=ui）。
-- 关键点是 `texture`：这个 Quad **没写贴图**，`quad.get` 给出的实际采样路径是内置白纹理
-- 哨兵 `__white__`——纯色矩形因此不需要写 `texture=__white__` 这种内部约定。
local function demo_quad()
    local bar = need("ScaleBar")
    if bar == nil then
        return
    end
    local before = quad.get(bar)
    -- 矩形尺寸每按一次长 24 像素，直到 240（演示"运行时改几何"，不是改实体的缩放）。
    local width = math.min((before.size_x or 96) + 24, 240)
    quad.set(
        bar,
        string.format(
            "size=%.1f,14;color=0.900,0.200,0.200,1.000;pivot=0.500,0.500;z=-0.500;layer=ui;order=0",
            width
        )
    )
    local after = quad.get(bar)
    last_note = string.format(
        "quad size=%sx%s color=(%s,%s,%s) texture=%s z=%s component=%s",
        num(after.size_x),
        num(after.size_y),
        num(after.color_r),
        num(after.color_g),
        num(after.color_b),
        tostring(after.texture),
        num(after.z),
        tostring(after.component)
    )
    return last_note
end

-- ── 4. 动画：attach / get / set / frame / time / clips / ended / events ───────
-- 看什么：三段事实。
--   ① Player 的片段清单（clips）与当前状态（get）、当前帧号（frame）、播放时刻（time）；
--   ② 切到非循环片段 `hit` 之后 `ended` 会**取走即清空**地返回一次 true（帧事件同一种用法）；
--   ③ `animation.attach` 给**还没有 Animation 的实体**（Sparks）补一台状态机——
--      "运行期造出来的实体也能播动画"就靠它。
local function demo_animation()
    local player = need(PLAYER)
    if player == nil then
        return
    end

    local clips = animation.clips(player)
    local current = animation.get(player)
    local frame = animation.frame(player)
    local clock = animation.time(player)

    -- 先给 Sparks 挂状态机（它本来只有粒子发射器）：组件串与场景里的 Animation 同一套语法。
    local sparks = id_of(SPARKS)
    local attached = false
    if sparks ~= nil then
        attached = animation.attach(sparks, "default=pulse;clip:pulse:assets/textures/slime.png:2:6:true")
    end

    -- 切到非循环片段：它播完会置一次 ended。再切回 idle 让画面继续有东西在动。
    local switched = animation.set(player, "hit")
    animation.set(player, "idle")

    -- events：本帧触发的帧事件（取走即清空）。片段 `hit` 声明了 events=0:swing,1:hit。
    local events = animation.events()
    local ended = animation.ended(player)
    local event_name = "none"
    if type(events) == "table" and #events > 0 then
        event_name = tostring(events[1].name) .. "@" .. tostring(events[1].frame)
    end

    local names = {}
    if type(clips) == "table" then
        for index = 1, #clips do
            names[#names + 1] = tostring(clips[index])
        end
    end
    last_note = string.format(
        "animation state=%s frame=%s time=%s clips=[%s] switch=%s ended=%s events=%s attach(sparks)=%s",
        tostring(current),
        tostring(frame),
        num(clock),
        table.concat(names, ","),
        tostring(switched),
        tostring(ended),
        event_name,
        tostring(attached)
    )
    return last_note
end

-- ── 5. 粒子：particles.burst / particles.count ───────────────────────────────
-- 看什么：在世界坐标上打一簇彩色火花（带重力、带阻力、颜色渐隐成蓝色透明），
-- 然后读回**当前存活粒子数**。Sparks 自己的 ParticleEmitter 是 `rate=24` 的连续发射，
-- 所以这个计数里既有场景声明的连续发射，也有脚本这一次爆发。
-- 注意 `gravity_y` 与场景里同一个方向的约定：世界 Y 向上、屏幕 Y 向下，所以正的 gravity_y
-- 是"往下掉"（与场景里 Sparks 的 `gravity=0,120` 一致）。
local function demo_particles()
    local x, y = origin_of(nil, SPARKS)
    burst_count = burst_count + 1
    particles.burst(x, y, 12, {
        speed = 180.0,
        life = 0.8,
        size = 6.0,
        size_end = 1.0,
        color = { r = 1.0, g = 0.9, b = 0.3, a = 1.0 },
        color_end = { r = 0.2, g = 0.4, b = 1.0, a = 0.0 },
        gravity_x = 0.0,
        gravity_y = 120.0,
        drag = 1.5,
    })
    last_note = string.format("particles.burst #%d at (%s,%s) alive=%d", burst_count, num(x), num(y), particles.count())
    return last_note
end

-- ── 6. 世界文字：world_text.detach / world_text.attach ───────────────────────
-- 看什么：名字牌在**世界位置**上（相机移动时跟着世界走），与屏幕空间的 Hud 完全是两套东西。
-- 第一次按键把 Nameplate 摘掉（detach 返回是否真的移除了），第二次用 attach 挂回来，
-- 并且顺手演示"运行期创建的实体要自己挂文字"——attach 是同一个入口。
local function demo_world_text()
    local plate = need(NAMEPLATE)
    if plate == nil then
        return
    end
    if plate_attached then
        local removed = world_text.detach(plate)
        plate_attached = false
        last_note = string.format("world_text.detach -> %s (名牌是实体的一部分，摘掉就不画了)", tostring(removed))
    else
        local attached = world_text.attach(
            plate,
            "text=Player;font_size=14;color=0.600,1.000,0.800;offset=0,10;centered=true"
        )
        plate_attached = true
        last_note = string.format("world_text.attach -> %s (文字锚在实体的世界位置上)", tostring(attached))
    end
    return last_note
end

-- ── 7. 可见层：visibility.get / visibility.set / visibility.all ───────────────
-- 看什么：Hidden 这个实体的 layers 被在 0x1 与 0x2 之间切换。视图遮罩是 0x1 时，
-- 写到 0x2 的那个实体**不存在于这个视图**——它不产顶点、不占绘制层序号，不是"变透明"。
-- `visibility.all()` 是全可见常量（u32::MAX），用来表达"恢复默认"。
-- 注意 Hidden 在场景里**没有** `Visibility` 组件：`set` 会把组件补上（这正是"缺组件 = 全可见"
-- 这条既有约定的入口），第一次 `get` 读到的因此是全可见常量而不是 0。
-- 场景文件里写了为什么刻意不写该组件（typed_scene 尚未覆盖 `Visibility`，会判红那条计数门）。
local function demo_visibility()
    local hidden = need(HIDDEN)
    if hidden == nil then
        return
    end
    local before = visibility.get(hidden)
    local all = visibility.all()
    local layers = LAYER_PRIMARY
    if vis_hidden then
        layers = LAYER_ALT
    end
    local written = visibility.set(hidden, layers)
    local after = visibility.get(hidden)
    local player_default = visibility.get(id_of(PLAYER) or hidden)
    vis_hidden = not vis_hidden
    last_note = string.format(
        "visibility %s -> %s (written=%s all=0x%X player_layers=0x%X)",
        tostring(before),
        tostring(after),
        tostring(written),
        all,
        player_default
    )
    return last_note
end

-- ── 8. 物理：overlap / raycast / positions_in_radius / events ────────────────
-- 看什么：三种查询各自的语义差别，以及本帧的碰撞事件（取走即清空）。
--   · `physics.overlap` 走 Rapier 的**碰撞体**查询：形状、offset、sensor 都参与；
--   · `physics.raycast` 命中最近的碰撞体，返回那个实体（null = 什么都没打到）；
--   · `physics.positions_in_radius` 是**宽相**位置查询（只看 Transform 的圆距离）——
--     与 overlap 是两件事，因此有两个名字。
local function demo_physics_query()
    local player = need(PLAYER)
    local coin = need(COIN)
    if player == nil or coin == nil then
        return
    end
    local x, y = origin_of(player)

    -- 8-1 overlap：以 Player 为中心 180 半径的圆去撞碰撞体（Coin 是 sensor，也会被查到）
    local hits = physics.overlap(x, y, 180.0)
    local hit_count = 0
    if type(hits) == "table" then
        hit_count = #hits
    end

    -- 8-2 raycast：从 Player 向 +X 打一条 800 长的射线，返回**最近**命中的那个碰撞体。
    -- 这条路径会先穿过瓦片地图的非实心格（没有碰撞体）与 coin/slime（在 y=180，不在 y=0 这条线上），
    -- 最后打到 Crate(ball,760,320) 的圆边缘——因此它演示的是"最近的那个"而不是"途中的所有"。
    local hit = physics.raycast(x, y, 1.0, 0.0, 800.0)

    -- 8-3 positions_in_radius：按 Transform 位置做的圆查询（不是碰撞体：没有碰撞体的实体也会被查到）
    local near = physics.positions_in_radius(x, y, 180.0)
    local near_count = 0
    if type(near) == "table" then
        near_count = #near
    end

    -- 8-4 events：本帧碰撞事件（`started` 表示这一帧刚开始接触），取走即清空
    local events = physics.events()
    local event_text = "none"
    if type(events) == "table" and #events > 0 then
        event_text = string.format("#%d started=%s", #events, tostring(events[1].started))
    end

    last_note = string.format(
        "physics overlap=%d raycast=%s positions_in_radius=%d events=%s",
        hit_count,
        tostring(hit),
        near_count,
        event_text
    )
    return last_note
end

-- ── 9. 物理：get_velocity / set_velocity / set_position / set_ccd / get_ccd ──
-- 看什么：动态刚体的**速度是真源**（`PhysicsAuthority::Physics` 默认：每步之后模拟把位置写回
-- ECS，所以"直接把动态体挪过去"会被模拟覆盖）。这里给 Player 一个速度让它自己走向 Coin，
-- 顺手开一次连续碰撞检测并读回物理世界的真实开关。
-- `set_position` 用在**非动态**的东西上（Platform 是 `dynamic=false` 的固定体）：
-- 它等价于"运动学传送"，脚本要自己保证目标位置合理。
local function demo_physics_body()
    local player = need(PLAYER)
    local platform = need(PLATFORM)
    if player == nil or platform == nil then
        return
    end

    -- 9-1 get_velocity：读回 `{x=, y=}`（数值下标 [1]/[2] 同样可用）
    local before = physics.get_velocity(player)
    -- 9-2 set_velocity：让 Player 沿 +X 慢慢走向 Coin，制造一次真实接触（events 会看到）
    physics.set_velocity(player, 90.0, 0.0)
    local after = physics.get_velocity(player)

    -- 9-3 set_position：传送一个**固定体**（Platform 抬高一格）
    local px, py = origin_of(platform)
    physics.set_position(platform, px, py + 16.0)

    -- 9-4 set_ccd / 9-5 get_ccd：连续碰撞检测（高速物不穿薄墙）。真源在物理世界里，
    -- 所以 get_ccd 读的是刚体上的真实开关，而不是引擎侧的影子记录。
    local ccd_before = physics.get_ccd(player)
    physics.set_ccd(player, true)
    local ccd_after = physics.get_ccd(player)

    last_note = string.format(
        "physics velocity (%s,%s) -> (%s,%s) ccd %s -> %s platform=(%s,%s)",
        num(before.x),
        num(before.y),
        num(after.x),
        num(after.y),
        tostring(ccd_before),
        tostring(ccd_after),
        num(px),
        num(py + 16.0)
    )
    return last_note
end

-- ── 10. 物理：debug_draw ────────────────────────────────────────────────────
-- 看什么：碰撞体轮廓（动态体绿、固定体青、sensor 黄）叠在世界内容之上、屏幕 UI 之下。
-- 它是**渲染期消费**的登记请求（开一次即持续生效，不必每帧调用），headless 下只是登记。
local function demo_physics_debug()
    -- 开一次即持续生效：宿主在渲染期取走这个请求，headless 下只是被登记。
    local enabled = physics.debug_draw(true)
    local player = id_of(PLAYER)
    local ccd = false
    if player ~= nil then
        local ok, value = safe_call(physics.get_ccd, player)
        ccd = ok and value == true
    end
    last_note = string.format(
        "physics.debug_draw(%s) 轮廓：绿=动态(player) 青=固定(Platform/Ground) 黄=sensor(Coin)；player_ccd=%s",
        tostring(enabled),
        tostring(ccd)
    )
    return last_note
end

-- ── 11. 相机与瓦片：屏幕 UI 之外的"取景"事实 ─────────────────────────────────
-- 看什么：`camera2d.get/get_zoom` 读的是**宿主每帧回填**的真实相机快照。
-- 没有渲染帧时（headless 测试、编辑器只打开场景还没进 Play）它明确报错——引擎刻意不拿一台
-- 默认相机去猜。因此这里用 api.try 包一层，把"宿主还没提供快照"这件事写进状态行而不是中断。
local function demo_camera()
    local center, error_text = safe_call(camera2d.get)
    local zoom = safe_call(camera2d.get_zoom)
    if center == nil then
        last_note = string.format("camera2d.get unavailable: %s", tostring(error_text))
        return last_note
    end
    last_note = string.format(
        "camera2d center=(%s,%s) zoom=%s | Ground 是 8x8 瓦片地图，实心格自动生成固定碰撞体",
        num(center.x),
        num(center.y),
        tostring(zoom)
    )
    return last_note
end

-- ── 12. 首帧/轮播用的组合演示：把每个模块都真的调一次 ─────────────────────────
-- 为什么要有它：自动化门（headless 步进、没有人按键）也要覆盖到全部 API；
-- 有输入时它只作为"轮播"存在，键 G 交替跑变换与形象两组。
local function demo_sweep()
    local parts = {
        demo_transform(),
        demo_sprite(),
        demo_quad(),
        demo_animation(),
        demo_particles(),
        demo_world_text(),
        demo_visibility(),
        demo_physics_query(),
        demo_physics_body(),
        demo_physics_debug(),
        demo_camera(),
    }
    local kept = {}
    for index = 1, #parts do
        if parts[index] ~= nil and parts[index] ~= true then
            kept[#kept + 1] = tostring(parts[index])
        end
    end
    return table.concat(kept, " | ")
end

-- ── 按键分发表：键名 → 演示函数 ──────────────────────────────────────────────
-- 用函数值而不是字符串间接调用，是为了让"这个键真的调了这个 API"在代码里一眼可见；
-- 自动化覆盖率门按 `模块.函数(` 的文本出现判定，因此每个 API 都有**直呼**它的那一行。
local function key_actions()
    return {
        keyt = demo_transform, -- 变换：get/set/move/attach/space/world
        keyy = demo_sprite, -- 精灵：get/set
        keyu = demo_quad, -- 一等矩形：get/set
        keyi = demo_animation, -- 动画：attach/get/set/frame/time/clips/ended/events
        keyo = demo_particles, -- 粒子：burst/count
        keyp = demo_world_text, -- 世界文字：attach/detach
        keyv = demo_visibility, -- 可见性：get/set/all
        keyj = demo_physics_query, -- 物理查询：overlap/raycast/positions_in_radius/events
        keyk = demo_physics_body, -- 物理刚体：get/set_velocity、set_position、set/ccd
        keyl = demo_physics_debug, -- 物理调试绘制：debug_draw
        keyc = demo_camera, -- 相机（宿主帧依赖，api.try 包着）：get/get_zoom
        space = demo_sweep, -- 全部走一遍
    }
end

--- `on_create`：只在进入本场景时调用一次，登记标题与按键提示。
function M.on_create()
    api.title("main - 2D core: Transform / Sprite / Quad / Animation / Tilemap / Physics")
    api.hint(
        "T transform  Y sprite  U quad  I animation  O particles  P world_text  V visibility  "
            .. "J physics query  K physics body  L debug_draw  C camera  SPACE all  "
            .. api.KEYS_BACK
    )
    registered = true
    return "demo_main_created"
end

--- 每个固定步一次。返回的字符串会进引擎日志（`run_script_steps` / 编辑器 Play 都能看到）。
function M.update()
    -- ① 首帧把每个 API 模块都真的跑一遍（headless 门因此也能覆盖全部 API）。
    --    刻意不放在顶层：模块加载时实体还不存在。
    if not auto_done then
        auto_done = true
        last_note = demo_sweep()
    end

    -- ② 按键分工。
    local actions = key_actions()
    for key, action in pairs(actions) do
        if input.pressed(key) then
            last_note = action() or last_note
        end
    end

    -- ③ 键 G：变换与形象交替轮播（不按键也会自己动，看的是"父子局部/世界值怎么变"）。
    if input.pressed("keyg") then
        cycle_index = (cycle_index + 1) % 2
        if cycle_index == 0 then
            last_note = demo_transform() or last_note
        else
            last_note = demo_sprite() or last_note
        end
    end

    -- ④ 键 Z：把整套改动复位（"恢复默认"用的就是 `visibility.all()` 那个全可见常量所表达的意图：
-- 实体侧遮罩回到位 0）。
    if input.pressed("keyz") then
        local hidden = id_of(HIDDEN)
        if hidden ~= nil then
            visibility.set(hidden, LAYER_PRIMARY)
            vis_hidden = false
        end
        local bar = id_of("ScaleBar")
        if bar ~= nil then
            quad.set(bar, "size=96,10;color=0.200,0.900,0.300,1.000;pivot=0.500,0.500;z=-0.500;layer=ui;order=0")
        end
        local plate = id_of(NAMEPLATE)
        if plate ~= nil and not plate_attached then
            world_text.attach(plate, "text=Player;font_size=14;color=0.900,0.950,1.000;offset=0,10;centered=true")
            plate_attached = true
        end
        local player = id_of(PLAYER)
        if player ~= nil then
            physics.set_velocity(player, 0.0, 0.0)
            physics.set_ccd(player, false)
        end
        last_note = "reset: visibility.all / quad / world_text / velocity / ccd restored"
    end

    -- ⑤ 状态行：本帧的**事实**（读回的值，而不是"我打算做什么"）。
    local facts = {}
    local player = id_of(PLAYER)
    if player ~= nil then
        local ok, local_player = pcall(transform.get, player)
        if ok and type(local_player) == "table" then
            facts[#facts + 1] = string.format("player=(%s,%s)", num(local_player.x), num(local_player.y))
        end
    end
    local aura = id_of(AURA)
    if aura ~= nil then
        local ok, aura_world = pcall(transform.world, aura)
        if ok and type(aura_world) == "table" then
            facts[#facts + 1] = string.format("aura_world=(%s,%s)", num(aura_world.x), num(aura_world.y))
        end
    end
    facts[#facts + 1] = string.format("particles=%d", particles.count())
    facts[#facts + 1] = string.format("bursts=%d", burst_count)
    if player ~= nil then
        local ok, velocity = pcall(physics.get_velocity, player)
        if ok and type(velocity) == "table" then
            facts[#facts + 1] = string.format("player_v=(%s,%s)", num(velocity.x), num(velocity.y))
        end
    end
    local hidden = id_of(HIDDEN)
    if hidden ~= nil then
        local ok, layers = pcall(visibility.get, hidden)
        if ok then
            facts[#facts + 1] = string.format("hidden_layers=0x%X", layers)
        end
    end
    local slime = id_of(SLIME)
    if slime ~= nil then
        local ok, frame = pcall(animation.frame, slime)
        if ok then
            facts[#facts + 1] = string.format("slime_frame=%s", tostring(frame))
        end
    end
    local ok_events, events = pcall(physics.events)
    if ok_events and type(events) == "table" then
        facts[#facts + 1] = string.format("contacts=%d", #events)
    end
    if not registered then
        facts[#facts + 1] = "hint/title not registered (on_create missing)"
    end

    api.status(table.concat(facts, " "))
    return "demo_main " .. table.concat(facts, " ") .. " | " .. tostring(last_note)
end

--- 离开场景时释放本模块登记的动态 UI（几何/文字/提示都清掉）。
function M.on_destroy()
    api.clear()
    registered = false
    return "demo_main_destroyed"
end

return M
