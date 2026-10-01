use super::*;

#[test]
fn instantiated_animation_is_switchable_from_lua() {
    use crate::core::scene::{Scene, SceneEntityId};

    let mut scene = Scene::new("test");
    scene.add_entity(SceneEntityId(1), "Hero", None).unwrap();
    scene.entities.get_mut(&SceneEntityId(1)).unwrap().components.insert(
        "Animation".into(),
        "default=idle;clip:idle:assets/idle-sheet.png:4:4:true;clip:attack:assets/attack-sheet.png:2:2:false".into(),
    );

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.instantiate_scene(&scene).unwrap();

    let first = script(
        "function on_create()\nlocal h = entity.find('Hero')\nassert(animation.get(h) == 'idle')\nassert(animation.set(h, 'attack'))\nassert(animation.get(h) == 'attack')\nassert(not animation.set(h, 'attack'))\nreturn 'ok' end\nfunction update() return 'ok' end",
    );
    runtime.execute_project_script(&first).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");

    // advance past the 1.0s non-looping attack clip
    let machines = runtime.with_world(|world| world.entities_with::<AnimationStateMachine>());
    assert_eq!(machines.len(), 1);
    runtime.with_world_mut(|world| {
        world
            .get_mut::<AnimationStateMachine>(machines[0])
            .unwrap()
            .unwrap()
            .advance(1.1);
    });

    let second = script(
        "function update()\nlocal h = entity.find('Hero')\nassert(animation.frame(h) == 1)\nassert(animation.ended(h))\nassert(not animation.ended(h))\nreturn 'done' end",
    );
    runtime.execute_project_script(&second).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "done");
    let _ = fs::remove_file(first);
    let _ = fs::remove_file(second);
}

#[test]
fn runtime_spawned_entity_can_attach_animation() {
    // 运行期创建的实体（entity.spawn 只插入 Transform）此前无法播动画：
    // NPC/敌人/特效只能显示单帧。animation.attach 让 Lua 侧自己挂组件。
    let first = script(
        "function on_create()\n\
         local id = entity.spawn('Effect', 'fx_probe')\n\
         assert(animation.attach(id, 'default=idle_s;clip:idle_s:assets/fx.png:4:8:true'))\n\
         assert(animation.get(id) == 'idle_s')\n\
         local clips = animation.clips(id)\n\
         assert(#clips == 1 and clips[1] == 'idle_s')\n\
         assert(animation.set(id, 'idle_s') == false)\n\
         return 'ok' end\n\
         function update() return 'ok' end",
    );
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.execute_project_script(&first).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");

    // 组件确实进了 ECS，且每帧推进（4 帧 @8fps → 0.2s 后到第 2 帧）
    let machines = runtime.with_world(|world| world.entities_with::<AnimationStateMachine>());
    assert_eq!(machines.len(), 1);
    runtime.with_world_mut(|world| {
        world
            .get_mut::<AnimationStateMachine>(machines[0])
            .unwrap()
            .unwrap()
            .advance(0.26);
    });
    let second = script(
        "function update()\nlocal id = entity.find_id('fx_probe')\nassert(animation.frame(id) == 2)\nreturn 'frame2' end",
    );
    runtime.execute_project_script(&second).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "frame2");

    // 非法组件串要被拒绝，且不破坏已有组件
    let third = script(
        "function update()\n\
         local id = entity.find_id('fx_probe')\n\
         local ok = pcall(animation.attach, id, 'clip:broken')\n\
         assert(not ok, 'malformed component must be rejected')\n\
         assert(animation.get(id) == 'idle_s')\n\
         local gone = pcall(animation.attach, 999999, 'default=a;clip:a:assets/fx.png:2:8:true')\n\
         assert(not gone, 'dead entity must be rejected')\n\
         return 'guarded' end",
    );
    runtime.execute_project_script(&third).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "guarded");
    let _ = fs::remove_file(first);
    let _ = fs::remove_file(second);
    let _ = fs::remove_file(third);
}
/// WP4 ⑦：动画帧事件从宿主收集到 Lua 轮询的整条链路。
#[test]
fn lua_animation_events_drain_after_host_advances() {
    let mut runtime = LuaRuntime::new().unwrap();
    // 宿主侧推进一步动画后把事件交进来；Lua 侧取走即清空。
    let id = runtime.with_world_mut(|world| world.spawn());
    runtime.with_world_mut(|world| {
        world
            .insert(
                id,
                AnimationStateMachine::from_component_str(
                    "default=attack;clip:attack:s.png:2:2:false:events=1:hit",
                )
                .unwrap(),
            )
            .unwrap();
    });
    let path = script(
        "function update()\n\
         local events = animation.events()\n\
         assert(#events == 1, 'expected one frame event')\n\
         assert(events[1].name == 'hit')\n\
         assert(events[1].frame == 1)\n\
         assert(animation.events()[1] == nil, 'drained on read')\n\
         return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    // 模拟宿主的 `animation_state_advance`：推进并收集事件。
    let fired = runtime.with_world_mut(|world| {
        let mut fired = Vec::new();
        world.for_each_mut::<AnimationStateMachine>(|entity, machine| {
            machine.advance(0.6); // 2 fps -> 跨到帧 1
            for event in machine.take_frame_events() {
                fired.push((entity, event));
            }
        });
        fired
    });
    runtime.push_animation_events(fired);
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}

#[test]
fn lua_3d_animation_queries_and_switches_clips() {
    use crate::core::scene::{Scene, SceneEntityId};

    let root = crate::core::games_dir().join("examples").join("game_api");
    if !root.join("assets/models/m8-skin-quad.glb").is_file() {
        return;
    }
    let mut scene = Scene::new("3d-anim");
    scene.add_entity(SceneEntityId(1), "Hero", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Animation3D".into(),
            "mesh=assets/models/m8-skin-quad.glb;clip=Bend;looped=true".into(),
        );

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(root.clone());
    assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);

    let path = script(
        "function update()\n\
         local h = entity.find('Hero')\n\
         assert(animation3d.get(h) == 'Bend')\n\
         local list = animation3d.list(h)\n\
         assert(#list == 1 and list[1] == 'Bend')\n\
         assert(not animation3d.set(h, 'Bend'))\n\
         assert(animation3d.time(h) >= 0)\n\
         assert(not animation3d.paused(h))\n\
         animation3d.set_paused(h, true)\n\
         assert(animation3d.paused(h))\n\
         return 'anim3d-ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "anim3d-ok");

    // The Animation3D component is present in the world and advancing works.
    let animations = runtime.with_world(|world| world.entities_with::<Animation3D>());
    assert_eq!(animations.len(), 1);
    runtime.with_world_mut(|world| {
        world
            .get_mut::<Animation3D>(animations[0])
            .unwrap()
            .unwrap()
            .advance(0.5);
    });
    assert!(
        runtime.with_world(|world| world
            .get::<Animation3D>(animations[0])
            .unwrap()
            .unwrap()
            .time())
            == 0.0,
        "paused animation must not advance"
    );
    let _ = fs::remove_file(path);
}
