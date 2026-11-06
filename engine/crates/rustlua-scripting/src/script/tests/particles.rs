use super::*;

#[test]
fn ui_particles_and_audio_lua_api() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
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
fn particles_wind_api_round_trips_and_rejects_non_finite() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let path = script(
        "local ok, reason = particles.set_wind(12, -3)\n\
         assert(ok, reason)\n\
         local wx, wy = particles.wind()\n\
         assert(math.abs(wx - 12) < 1e-6 and math.abs(wy + 3) < 1e-6, 'wind round trip')\n\
         local bad = particles.set_wind(0/0, 0)\n\
         assert(bad == false, 'NaN wind must be rejected')\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}

#[test]
fn particles_wind_profile_round_trips_and_rejects_bad_ranges() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let path = script(
        "local ok, reason = particles.set_wind_profile(0.1, 0.5, 0.3)\n\
         assert(ok, reason)\n\
         local shear, gust, hz = particles.wind_profile()\n\
         assert(math.abs(shear - 0.1) < 1e-6, 'shear round trip')\n\
         assert(math.abs(gust - 0.5) < 1e-6, 'gust round trip')\n\
         assert(math.abs(hz - 0.3) < 1e-6, 'gust hz round trip')\n\
         assert(particles.set_wind_profile(2.0, 0.5, 0.3) == false, 'shear range')\n\
         assert(particles.set_wind_profile(0.1, 5.0, 0.3) == false, 'gust range')\n\
         assert(particles.set_wind_profile(0.1, 0.5, 30.0) == false, 'hz range')\n\
         assert(particles.set_wind_profile(0/0, 0.5, 0.3) == false, 'NaN')\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}

#[test]
fn particles_wind_sources_validate_and_reach_the_host_bridge() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let path = script(
        "local ok, reason = particles.set_wind_source(0, 10, 2, -5, 12, 0.6, 2.5)\n\
         assert(ok, reason)\n\
         assert(particles.set_wind_source(4, 0, 0, 0, 1, 0, 0) == false, 'index range')\n\
         assert(particles.set_wind_source(1, 0, 0, 0, 600, 0, 0) == false, 'radius range')\n\
         assert(particles.set_wind_source(1, 0, 0, 0, 1, 0/0, 0) == false, 'NaN')\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let sources = runtime.particle_wind_sources();
    assert_eq!(sources[0], [10.0, 2.0, -5.0, 12.0, 0.6, 2.5]);
    assert_eq!(sources[1], [0.0; 6]);
    // 清空走 Lua 入口；宿主桥读回全零。
    runtime
        .execute_project_script(&script(
            "particles.clear_wind_sources()\nfunction update() end",
        ))
        .unwrap();
    assert_eq!(runtime.particle_wind_sources()[0], [0.0; 6]);
    let _ = fs::remove_file(path);
}

#[test]
fn particles_burst3d_and_wind3d_round_trip() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let path = script(
        "particles.burst3d(1, 2, 7, 3, {speed=0, life=2, speed_z=4, gravity_z=-9, spread_z=1})\n\
         local ok, reason = particles.set_wind3d(1, 2, 3)\n\
         assert(ok, reason)\n\
         local wx, wy, wz = particles.wind3d()\n\
         assert(wx == 1 and wy == 2 and wz == 3, 'wind3d round trip')\n\
         assert(particles.set_wind3d(0, 0, 0/0) == false, 'NaN z wind must be rejected')\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let (count, zs) = runtime.with_particles(|particles| {
        (
            particles.len(),
            particles
                .iter()
                .map(|particle| particle.position_z)
                .collect::<Vec<_>>(),
        )
    });
    assert_eq!(count, 3);
    assert!(
        zs.iter().all(|z| (6.0..=8.0).contains(z)),
        "spread_z must stay around the 3D origin: {zs:?}"
    );
    runtime.step_particles(0.25);
    let moved = runtime
        .with_particles(|particles| particles.iter().all(|particle| particle.position_z != 7.0));
    assert!(moved, "3D particles must integrate z");
    let _ = fs::remove_file(path);
}

#[test]
fn particles_stream3d_upserts_and_stops_by_key() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let path = script(
        "particles.stream3d('rain', 1, 2, 3, 120, {life=1.2, gravity_z=-8, size=0.2, color={r=0.6,g=0.7,b=0.9,a=0.4}})\n\
         particles.stream3d('rain', 4, 5, 6, 180, {life=1.2, gravity_z=-8, size=0.2})\n\
         particles.stream3d('mist', 0, 1, 0, 30, {life=2, size=0.5})\n\
         local streams = assert(particles.stream_count())\n\
         assert(streams == 2, 'same key must update in place')\n\
         assert(particles.stop_stream3d('rain'))\n\
         assert(not particles.stop_stream3d('rain'))\n\
         assert(not pcall(function() particles.stream3d('bad', 0, 0, 0, 0) end), 'rate 0 must fail')\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let streams = runtime.particle_streams();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].key, "mist");
    assert_eq!(streams[0].position, [0.0, 1.0, 0.0]);
    assert!((streams[0].emitter.rate - 30.0).abs() < 1e-6);
    runtime.clear_particle_streams();
    assert!(runtime.particle_streams().is_empty());
    let _ = fs::remove_file(path);
}

#[test]
fn particles_burst_area_spawns_inside_a_box() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let path = script(
        "particles.burst3d(10, 0, 5, 48, {speed=0, life=1, area_x=2, area_y=1, area_z=3, seed=4})\n\
         assert(not pcall(function() particles.burst3d(0, 0, 0, 1, {area_x=-1}) end), 'negative area must fail')\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    assert_eq!(runtime.with_particles(|particles| particles.len()), 48);
    runtime.with_particles(|particles| {
        for particle in particles {
            assert!(
                (8.0..=12.0).contains(&particle.position.x)
                    && (-1.0..=1.0).contains(&particle.position.y)
                    && (2.0..=8.0).contains(&particle.position_z),
                "particle outside Lua area box: {particle:?}"
            );
        }
    });
    let _ = fs::remove_file(path);
}

#[test]
fn particles_burst_options_control_look_and_validate() {
    // 此前速度/寿命/颜色写死在引擎里，Lua 无法区分尘土、水花、落叶。
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
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

/// P3-R07-1：`mode`/`seed` 从 Lua 也能给，且与场景组件串同一份校验。
///
/// R07-2 之后 `mode='gpu'` 的 burst 进的是**派发队列**（宿主取走后才变成 GPU 系统），
/// 因此这里显式走一次"宿主没来接"的保底：`flush_gpu_bursts` 把它倒回 CPU 池。
/// 钉两件事——① 要了 GPU 也不许少发一个粒子；② 补发出来的与同一 seed 的 CPU 版逐位相同
/// （派发队列存的仍是发射器参数本身，而不是另一套生成逻辑）。
#[test]
fn particles_burst_accepts_mode_and_seed() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let path = script(
        "particles.burst(0, 0, 4, {mode='gpu', seed=7})\n\
         particles.burst(0, 0, 4, {mode='cpu', seed=7})\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    runtime.call_update().unwrap();
    // CPU 那一条当场进池；GPU 那一条还只在队列里（`with_particles` 读不到 = 视觉层语义）。
    assert_eq!(runtime.with_particles(|particles| particles.len()), 4);
    assert_eq!(runtime.gpu_burst_count(), 1, "gpu 请求要进派发队列");
    assert_eq!(runtime.flush_gpu_bursts(), 1, "保底要还回一条 burst");
    let all = runtime.with_particles(|particles| particles.to_vec());
    assert_eq!(all.len(), 8, "gpu 请求不丢粒子");
    for (first, second) in all[..4].iter().zip(&all[4..]) {
        assert_eq!(
            first.velocity, second.velocity,
            "同一 seed 的两次 burst 必须逐位相同"
        );
        assert_eq!(first.max_life, second.max_life);
        assert_eq!(first.size_start, second.size_start);
    }

    // 拼错的 mode 与非正 seed 都要报错，且不生成粒子（不静默退回默认值）。
    let bad = script(
        "function update()\n\
         assert(not pcall(particles.burst, 0, 0, 3, {mode='warm'}), 'mode must be cpu|gpu')\n\
         assert(not pcall(particles.burst, 0, 0, 3, {seed=-1}), 'seed must be >= 0')\n\
         return 'guarded' end",
    );
    runtime.execute_project_script(&bad).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "guarded");
    assert_eq!(runtime.with_particles(|particles| particles.len()), 8);
    assert_eq!(runtime.gpu_burst_count(), 0, "非法参数不许留下队列残骸");
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(bad);
}

/// P3-R07-2：派发队列的容量与溢出语义。
///
/// 队列只是**一帧的**派发窗口，不是无上限的暂存：写第 33 条时前 32 条还占着队列，
/// 那一条必须**当场**进 CPU 池（+`gpu_burst_overflow` 记账），而不是把已排队的爆发挤掉
/// 或悄悄丢弃。被宿主取走（`take_gpu_bursts`）后队列清空，溢出也就停止。
#[test]
fn gpu_burst_queue_overflows_to_cpu_without_loss() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let bursts = MAX_GPU_BURSTS_PER_FRAME + 8;
    let path = script(&format!(
        "function update()\n\
         for i = 1, {bursts} do\n\
         \x20 particles.burst(i, 0, 1, {{mode='gpu', seed=i}})\n\
         end\n\
         return 'ok' end"
    ));
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    assert_eq!(runtime.gpu_burst_count(), MAX_GPU_BURSTS_PER_FRAME);
    // 溢出的 8 条已经在 CPU 池里，一条不少。
    assert_eq!(runtime.with_particles(|particles| particles.len()), 8);
    assert_eq!(runtime.take_gpu_burst_overflow(), 8, "溢出要能被观测");
    assert_eq!(runtime.take_gpu_burst_overflow(), 0, "溢出计数取走即清");
    // 宿主取走队列后：这些粒子**不**出现在 CPU 池（GPU 侧由渲染器自己生成初始爆发）。
    let taken = runtime.take_gpu_bursts();
    assert_eq!(taken.len(), MAX_GPU_BURSTS_PER_FRAME);
    assert_eq!(runtime.gpu_burst_count(), 0);
    assert_eq!(runtime.with_particles(|particles| particles.len()), 8);
    // 取走的确实是入队的那份参数（位置 + 数量），宿主据此建 GPU 系统。
    for (index, (position, emitter)) in taken.iter().enumerate() {
        assert_eq!(position[0], index as f32 + 1.0);
        assert_eq!(position[2], 0.0);
        assert_eq!(emitter.count, 1);
    }
    let _ = fs::remove_file(path);
}

/// P3-R07-2：入队的 burst 在宿主取走之前**不进 CPU 池**，取走方式只有两种
/// （`take_gpu_bursts` = 派发到 GPU；`flush_gpu_bursts` = 回 CPU），且都取走即清空。
#[test]
fn gpu_burst_take_and_flush_are_exclusive() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let path = script("particles.burst(2, 3, 5, {mode='gpu'})\nfunction update() return 'ok' end");
    runtime.execute_project_script(&path).unwrap();
    runtime.call_update().unwrap();
    assert_eq!(runtime.flush_gpu_bursts(), 1);
    assert_eq!(runtime.gpu_burst_count(), 0);
    assert_eq!(runtime.with_particles(|particles| particles.len()), 5);
    // 再取一次既没有队列残骸，也不会重复生成粒子。
    assert_eq!(runtime.flush_gpu_bursts(), 0);
    assert!(runtime.take_gpu_bursts().is_empty());
    assert_eq!(runtime.with_particles(|particles| particles.len()), 5);
    let _ = fs::remove_file(path);
}
