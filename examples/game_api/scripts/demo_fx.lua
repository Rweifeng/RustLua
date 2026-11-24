-- api_fx：特效预设的 3D 舞台——怪物循环释放刀光弧（M96 的运行期用法）。
--
-- 这个场景只证明一件事，但要把它的每一环都读得出来：
--   **特效是资产（.prefab），不是代码。** 同一张 `.sgraph` 包成两份预设（暖色刀光 / 冷蓝刀光），
--   逻辑里用 `render.play_effect` 按需加载，用 `render.set_effect_param` 逐帧微调，
--   `render.stop_effect` 收招。换风格不改代码、不改配置、不重启。
--
-- 循环的编排（全部由脚本计时，不依赖交互）：
--   stance 蓄力 → swing 出刀（挂预设 + 切 clip）→ follow 收势 → 回 stance
-- 每次出刀在两份预设之间**交替**，读者隔一次就能看出"同一张图、两套参数"的差别。
--
-- 硬规则（README §2）：顶层只定义常量与函数；`run` 在 headless 下只做内存写，
-- 不写盘、不切场景、不模拟输入。特效层是否真的挂上由宿主消费，自检只断言意图与形状。

local lab = require("lab")

local M = {}

-- 实体名（与 `scenes/api_fx.scene` 一致）。
local CAMERA = "Camera3d"
local MONSTER = "BladeMonster"
local TOTEM = "Totem"
local TOTEM_CONE = "TotemCone"

-- 两份预设：同一张 `flame_blade_slash.sgraph`，只换参数。
local PRESETS = {
    { path = "assets/fx/flame_blade_slash.prefab", name = "warm" },
    { path = "assets/fx/frost_blade_slash.prefab", name = "frost" },
}

-- 循环时序（秒）。数字直接写在 claim/read 里太散，集中在这里。
local STANCE = 0.9
local SWING = 0.55
local FOLLOW = 0.5
local CYCLE = STANCE + SWING + FOLLOW

-- 出刀时的转向角（绕 Y 轴）：开局朝镜头偏左，出刀扫向右侧。
local FACING = { stance = 0.0, swing = 0.85 }

-- 运行期状态（`enter` 负责复位）。
local elapsed = 0
local phase = "stance"
local phase_time = 0
local swings = 0
local active_preset = 1
local last_swing_preset = nil
local layer_alive = false
local swing_energy = 0.0

--- `entity.find` + 存活校验：缺实体时返回 nil，调用方据此跳过而不是让 API 报错。
local function live(name)
    local id = entity.find(name)
    if id and entity.is_alive(id) then
        return id
    end
    return nil
end

local function quat_y(angle)
    local half = angle * 0.5
    return 0.0, math.sin(half), 0.0, math.cos(half)
end

local function preset(index)
    return PRESETS[((index - 1) % #PRESETS) + 1]
end

--- 把怪物放回开场姿势：位置、朝向、剪辑。
local function stand_still()
    local monster = live(MONSTER)
    if monster then
        local _, ry, _, rw = quat_y(FACING.stance)
        transform3d.set(monster, 0, 0, 0, 0, ry, 0, rw, 1, 1, 1)
        animation3d.set(monster, "Idle")
    end
end

--- 收掉特效层并回到蓄力态。`enter` 与销毁都走它，保证"按 R/F5 之后全绿"。
local function reset_arena()
    elapsed = 0
    phase = "stance"
    phase_time = 0
    swings = 0
    active_preset = 1
    last_swing_preset = nil
    layer_alive = false
    swing_energy = 0.0
    render.stop_effect()
    stand_still()
end

--- 出刀：挂预设 + 切朝向 + 给一点刀光参数。
---
--- 每次出刀都 `play_effect` 而不是"先判断再挂"：同名重播是**替换**，所以这里天然幂等，
--- 也顺带演示了"重播即重新计时"（刀光弧从起手扫一遍，而不是接着上一刀的相位）。
--- `fixed` 不为空时锁定单个预设（`preset_swap` 之外的步骤用它把变量收敛掉）。
local function swing(fixed)
    local entry = fixed and PRESETS[fixed] or preset(active_preset)
    render.play_effect(entry.path, entry.name)
    layer_alive = true
    last_swing_preset = entry.name
    if not fixed then
        active_preset = active_preset + 1
    end
    swings = swings + 1

    local monster = live(MONSTER)
    if monster then
        local _, ry, _, rw = quat_y(FACING.swing)
        transform3d.set(monster, 0, 0, 0, 0, ry, 0, rw, 1, 1, 1)
        -- 招式剪辑名来自骨骼清单；清单里没有就保持 Idle，不赌名字。
        if animation3d.list(monster) and #animation3d.list(monster) > 0 then
            local wanted = "Attack"
            for _, clip in ipairs(animation3d.list(monster)) do
                if clip == wanted then
                    animation3d.set(monster, wanted)
                    break
                end
            end
        end
    end
    log.info(string.format("demo_fx: swing #%d preset=%s", swings, entry.name))
end

--- 每帧推进循环。`dt` 来自 lab（宿主固定步长），headless 下同样安全。
---
--- `mode` 有三档，对应三个步骤，共用同一套时序：
---   `"loop"`   冷暖交替（默认，`blade_loop` 步）
---   `"warm"`   锁定暖色预设（`tune_params` 步：变量只剩参数，读者能盯住弧形的变化）
---   `"frost"`  锁定冷蓝预设（`preset_swap` 步：和上一步对照）
local function advance(dt, mode)
    elapsed = elapsed + dt
    phase_time = phase_time + dt
    local fixed = nil
    if mode == "warm" then
        fixed = 1
    elseif mode == "frost" then
        fixed = 2
    end
    if phase == "stance" then
        -- 蓄力：刀光先收掉，朝向回正，参数回中性。
        swing_energy = math.max(0.0, swing_energy - dt * 2.2)
        if phase_time >= STANCE then
            phase, phase_time = "swing", 0.0
            swing(fixed)
        end
    elseif phase == "swing" then
        -- 出刀：亮度从 0 冲到 1，弧宽随之张开——这就是"逐帧调参"的形状。
        swing_energy = math.min(1.0, swing_energy + dt / SWING)
        render.set_effect_param("spin", 1.4 + 3.2 * swing_energy)
        render.set_effect_param("speed", 0.7 + 1.1 * swing_energy)
        if phase_time >= SWING then
            phase, phase_time = "follow", 0.0
        end
    else
        -- 收势：参数回落，层在 follow 结束时收掉（读者能看到弧线自己淡出）。
        swing_energy = math.max(0.0, swing_energy - dt / FOLLOW)
        render.set_effect_param("spin", 4.6 * swing_energy)
        if phase_time >= FOLLOW then
            phase, phase_time = "stance", 0.0
            layer_alive = false
            render.stop_effect()
            stand_still()
        end
    end
end

local function phase_zh()
    if phase == "swing" then
        return "出刀"
    elseif phase == "follow" then
        return "收势"
    end
    return "蓄力"
end

local function rebuild_totems()
    -- 两座图腾只是"给摄像机一个纵深参照"：没有它们，刀光是纯平面叠加，看不出舞台深度。
    local totem = live(TOTEM)
    if totem then
        mesh.set(totem, "__primitive__?kind=sphere&r=0.45&seg=24")
        mesh.set_material(totem, "assets/materials/p3-r02-emissive.material")
    end
    local cone = live(TOTEM_CONE)
    if cone then
        mesh.set(cone, "__primitive__?kind=cone&r=0.4&h=1.4&seg=20")
        mesh.set_material(cone, "assets/materials/p3-e02-m1-r09.material")
    end
end

--- 三步共用的开场：建图腾、选中相机、复位循环。
local function enter_arena()
    rebuild_totems()
    -- 场景里只有一台透视相机，但它不会自己变成活动相机：由脚本选中。
    local cam = live(CAMERA)
    if cam then
        camera.set_active(cam)
        -- 镜头把怪物放在画面正中：叠加层的圆心就是屏幕中心，只有这样刀光才绕着它张
                camera.set(cam, 0.9, 1.9, 6.0, 0.9, 1.3, 0.0, 50.0)
    end
    reset_arena()
end

--- 三步共用的 `read`：卡片上永远能看到同一组读数，读者不用记"上一步显示了什么"。
local function read_loop()
    return {
        { "phase", phase_zh() },
        { "swing", tostring(swings) },
        { "preset", last_swing_preset or "-" },
        { "energy", string.format("%.2f", swing_energy) },
        { "next", preset(active_preset).name },
    }
end

local function note_loop()
    lab.note(string.format(
        "phase=%s  第 %d 刀  预设=%s  能量=%.2f",
        phase,
        swings,
        last_swing_preset or "-",
        swing_energy
    ))
end

--- 预设路径的形状校验三步共用：真正的加载在宿主，headless 里没有渲染器。
local function check_preset_paths(t)
    t:ok(not render.play_effect(""), "empty preset path is rejected")
    t:ok(
        not render.play_effect("assets/fx/flame_blade_slash.sgraph"),
        "a .sgraph is not a preset; only .prefab is accepted"
    )
    t:ok(
        render.play_effect("assets/fx/flame_blade_slash.prefab"),
        "a .prefab preset path is accepted"
    )
    t:ok(render.play_effect("assets/fx/frost_blade_slash.prefab", "frost"), "named instance is accepted")
    t:eq(type(render.stop_effect("frost")), "boolean", "stop returns a boolean")
    t:ok(render.stop_effect(), "stop without a name clears every script-driven layer")
end

local steps = {
    {
        id = "blade_loop",
        title = "刀光循环 / Blade loop",
        claim = "怪物每 1.95 秒出一刀：出刀瞬间按 .prefab 预设挂一层刀光，随后逐帧改 spin/speed，收势时 stop_effect 撤掉；冷暖两份预设交替，同一张图换参数即换风格",
        api = {
            "render.play_effect",
            "render.stop_effect",
            "render.set_effect_param",
        },
        code = "render.play_effect('assets/fx/flame_blade_slash.prefab', 'warm'); render.set_effect_param('spin', 2.4); render.stop_effect()",
        enter = enter_arena,
        run = function(dt)
            advance(dt or time.delta(), "loop")
            note_loop()
        end,
        read = read_loop,
        check = function(t)
            check_preset_paths(t)

            -- 两份预设都必须是 `.prefab`、名字不同、路径不同：这就是"同一张图换个参数就是另一种
            -- 招式"的可验证部分。至于它们是否真的指向同一张图，`EffectPrefab::load` 的自测
            -- （`two_presets_on_one_graph_differ_only_in_values`）已经在宿主侧钉死了；
            -- 这里不再用 Lua 去读文件——工作目录在 headless 与真机之间并不可靠。
            for _, entry in ipairs(PRESETS) do
                t:ok(
                    entry.path:sub(-7) == ".prefab",
                    "preset " .. entry.name .. " points at a .prefab"
                )
            end
            t:ok(PRESETS[1].name ~= PRESETS[2].name, "the two presets have different instance names")
            t:ok(PRESETS[1].path ~= PRESETS[2].path, "the two presets live in different files")

            -- 循环时序：一轮必须能容纳三次相位，否则"循环释放"无从谈起。
            t:near(STANCE + SWING + FOLLOW, CYCLE, 1e-6, "cycle length adds up")
            t:ok(SWING > 0 and FOLLOW > 0, "swing and follow phases have positive length")

            reset_arena()
        end,
    },
    {
        id = "tune_params",
        title = "逐帧调参 / Tuning",
        claim = "刀光挂上后仍可逐帧改参数：spin 随时间从 1.4 涨到 4.6、speed 从 0.7 到 1.8，收势回落到 0——这就是改参数不重编管线的形状",
        api = { "render.set_effect_param", "render.play_effect" },
        code = "render.set_effect_param('spin', 1.4 + 3.2 * energy); render.set_effect_param('speed', 0.7 + 1.1 * energy)",
        enter = function()
            enter_arena()
            -- 锁定暖色预设：这一步只让参数变化，读者能盯住"同一条弧在张缩"。
            swing(1)
        end,
        run = function(dt)
            advance(dt or time.delta(), "warm")
            lab.note(string.format(
                "暖色预设 · spin=%.2f  speed=%.2f  能量=%.2f",
                1.4 + 3.2 * swing_energy,
                0.7 + 1.1 * swing_energy,
                swing_energy
            ))
        end,
        read = read_loop,
        check = function(t)
            -- 按名字改参数需要宿主先把 `.sgraph` 的名字表推下来：headless 自检没有渲染器，
            -- 函数会如实返回 (false, reason)——这本身是契约的一部分，不能在这里断言"一定成功"。
            local ok, reason = render.set_effect_param("spin", 2.4)
            if ok then
                t:ok(true, "spin accepted (host mounted the overlay)")
            else
                t:ok(
                    type(reason) == "string" and #reason > 0,
                    "named param reports why it is unavailable"
                )
            end
            t:ok(not render.set_effect_param("spin", 0 / 0), "non-finite param value is rejected")
            t:ok(not render.set_effect_param("not_a_param", 1.0), "unknown param name is rejected")
            -- 能量映射：0 与 1 两端必须落在声明的区间上。
            t:near(1.4 + 3.2 * 0.0, 1.4, 1e-6, "spin starts at 1.4")
            t:near(1.4 + 3.2 * 1.0, 4.6, 1e-6, "spin peaks at 4.6")
            t:near(0.7 + 1.1 * 0.0, 0.7, 1e-6, "speed starts at 0.7")
            t:near(0.7 + 1.1 * 1.0, 1.8, 1e-6, "speed peaks at 1.8")
            reset_arena()
        end,
    },
    {
        id = "preset_swap",
        title = "换预设 / Swap",
        claim = "同一次出刀换成冷蓝预设：不改代码、不改 project.toml、不重启——预设是资产，`play_effect` 换的名字就是另一套参数",
        api = { "render.play_effect", "render.stop_effect" },
        code = "render.stop_effect('warm'); render.play_effect('assets/fx/frost_blade_slash.prefab', 'frost')",
        enter = function()
            enter_arena()
            swing(2)
        end,
        run = function(dt)
            advance(dt or time.delta(), "frost")
            lab.note(string.format(
                "冷蓝预设 · 第 %d 刀  preset=%s",
                swings,
                last_swing_preset or "-"
            ))
        end,
        read = read_loop,
        check = function(t)
            check_preset_paths(t)
            -- 换预设的两种写法都必须成立：按层名撤、按新名字挂。
            t:ok(render.play_effect(PRESETS[1].path, "warm"), "warm layer can be mounted")
            t:eq(render.stop_effect("warm"), true, "stop by name is accepted")
            t:ok(render.play_effect(PRESETS[2].path, "frost"), "frost layer can be mounted")
            t:ok(render.stop_effect(), "stop-all clears both layers")
            reset_arena()
        end,
    },
}

function M.on_create()
    lab.start({
        scene = "api_fx",
        title = "Effect Presets",
        category = "3D",
        steps = steps,
        hide_scene_text = {},
    })
end

function M.update()
    return lab.tick()
end

function M.self_test()
    return lab.self_test()
end

function M.on_destroy()
    -- 特效层是宿主侧的全局状态：离开场景必须收掉，否则下一个场景会莫名其妙带着刀光。
    render.stop_effect()
    reset_arena()
    lab.stop()
end

-- 能力图谱门（`sample_project_capability_catalog_is_complete`）靠它核对步骤 id。
M.steps = steps

return M
