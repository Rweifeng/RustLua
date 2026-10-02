use super::*;

#[test]
fn ui_particles_and_audio_lua_api() {
    let mut runtime = LuaRuntime::new().unwrap();
    let path = script(
        "particles.burst(5, -3, 10)\nui.bar('hp', 20, 30, 100, 10, 0.5, 1, 0, 0)\nui.text_at('objective', 20, 20, 'Reach the door')\naudio.volume(0.5)\nfunction update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    runtime.call_update().unwrap();
    runtime.step_particles(0.1);

    let count = runtime.with_particles(|particles| particles.len());
    assert!(count >= 10, "particles.burst should spawn at least 10");
    let (texts, bars) = runtime.with_ui(|texts, bars| (texts.clone(), bars.clone()));
    let text = texts.get("objective").expect("objective text stored");
    assert_eq!(text.text, "Reach the door");
    let bar = bars.get("hp").expect("hp bar stored");
    assert_eq!(bar.ratio, 0.5);
    let _ = fs::remove_file(path);
}
#[test]
fn particles_burst_options_control_look_and_validate() {
    // 此前速度/寿命/颜色写死在引擎里，Lua 无法区分尘土、水花、落叶。
    let mut runtime = LuaRuntime::new().unwrap();
    let path = script(
        "particles.burst(1, 2, 4, {\n\
         speed=20, life=2, size=8, size_end=0,\n\
         color={r=0.2,g=0.4,b=1.0,a=0.8}, color_end={r=1,g=1,b=1,a=0},\n\
         gravity_x=0, gravity_y=-60, drag=1.5, rotation=0.25, rotation_speed=3 })\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    runtime.call_update().unwrap();
    assert_eq!(runtime.with_particles(|particles| particles.len()), 4);
    let (speed_max, size_start, gravity, color, rotation_speed, drag) =
        runtime.with_particles(|particles| {
            let particle = particles[0];
            (
                particle.velocity.length(),
                particle.size_start,
                particle.gravity,
                particle.color_start,
                particle.rotation_speed,
                particle.drag,
            )
        });
    // 初速在 25%–100% 之间：上界 20。
    assert!(speed_max <= 20.0 + 1e-3, "speed={speed_max}");
    // 尺寸抖动 [0.75,1.25] × 8。
    assert!((6.0..=10.0).contains(&size_start), "size={size_start}");
    assert_eq!(gravity, Vec2::new(0.0, -60.0));
    assert_eq!(color, [0.2, 0.4, 1.0, 0.8]);
    assert!((rotation_speed - 3.0).abs() < 1e-6);
    assert!((drag - 1.5).abs() < 1e-6);
    // 退化参数必须报错，且不生成粒子。
    let bad = script(
        "function update()\n\
         assert(not pcall(particles.burst, 0, 0, 3, {life=0}), 'life must be > 0')\n\
         assert(not pcall(particles.burst, 0, 0, 3, {drag=-1}), 'drag >= 0')\n\
         assert(not pcall(particles.burst, 0/0, 0, 3), 'position must be finite')\n\
         return 'guarded' end",
    );
    runtime.execute_project_script(&bad).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "guarded");
    assert_eq!(runtime.with_particles(|particles| particles.len()), 4);
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(bad);
}
