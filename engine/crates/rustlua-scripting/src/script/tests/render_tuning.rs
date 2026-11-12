use super::*;

#[test]
fn render_fog_round_trips_and_rejects_invalid_density() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let path = script(
        "local ok, reason = render.set_fog(0.003)\n\
         assert(ok, reason)\n\
         assert(render.set_fog(0.2) == false, 'upper bound')\n\
         assert(render.set_fog(0/0) == false, 'NaN')\n\
         local cok, creason = render.set_fog_color(0.52, 0.60, 0.70)\n\
         assert(cok, creason)\n\
         assert(render.set_fog_color(1.2, 0, 0) == false, 'color range')\n\
         local cok, creason = render.set_cloud_coverage(0.8)\n\
         assert(cok, creason)\n\
         assert(render.set_cloud_coverage(1.5) == false, 'coverage range')\n\
         local sok, sreason = render.set_cloud_speed(1.5)\n\
         assert(sok, sreason)\n\
         assert(render.set_cloud_speed(-0.1) == false, 'speed range')\n\
         local bok, breason = render.set_cloud_base(90)\n\
         assert(bok, breason)\n\
         assert(render.set_cloud_base(10) == false, 'base range')\n\
         local shok, shreason = render.set_cloud_shadow_strength(0.35)\n\
         assert(shok, shreason)\n\
         assert(render.set_cloud_shadow_strength(0.9) == false, 'shadow range')\n\
         local fok, freason = render.set_flash(0.6)\n\
         assert(fok, freason)\n\
         assert(render.set_flash(1.2) == false, 'flash range')\n\
         local wok, wreason = render.set_water_tint(0.8, 0.9, 0.85, 0.3)\n\
         assert(wok, wreason)\n\
         assert(render.set_water_tint(0.8, 1.2, 0.5, 0.3) == false, 'water range')\n\
         local qok, qreason = render.set_quality(2)\n\
         assert(qok, qreason)\n\
         assert(render.set_quality(4) == false, 'quality range')\n\
         local gok, greason = render.set_surface_cover(0.6, 0.4)\n\
         assert(gok, greason)\n\
         assert(render.set_surface_cover(0.6, 1.2) == false, 'surface range')\n\
         local eok, ereason = render.set_exposure(-0.3)\n\
         assert(eok, ereason)\n\
         assert(render.set_exposure(5) == false, 'exposure range')\n\
         local rok, rreason = render.set_ripple(1.5, -2.0)\n\
         assert(rok, rreason)\n\
         assert(render.set_ripple(0/0, 0) == false, 'ripple finite')\n\
         local fok, freason = render.set_post_fx(0.3, 0.2)\n\
         assert(fok, freason)\n\
         assert(render.set_post_fx(1.2, 0.2) == false, 'post fx range')\n\
         assert(render.set_clip_cap(129) == false, 'clip cap range')\n\
         assert(render.set_clip_cap(0/0) == false, 'clip cap finite')\n\
         assert(render.set_clip_cap(-1) == false, 'clip cap negative')\n\
         local zok, zreason = render.set_clip_cap(0)\n\
         assert(zok, zreason)\n\
         local kop, kreason = render.set_clip_cap(2.5)\n\
         assert(kop, kreason)\n\
         assert(render.set_clip_cap_both_sides(1) == false, 'both sides must be boolean')\n\
         assert(render.set_clip_cap_both_sides(nil) == false, 'both sides nil rejected')\n\
         assert(render.set_clip_cap_both_sides('on') == false, 'both sides string rejected')\n\
         local ook, oreason = render.set_clip_cap_both_sides(false)\n\
         assert(ook, oreason)\n\
         assert(render.set_water_refract_steps(33) == false, 'refract steps range')\n\
         assert(render.set_water_refract_steps(-1) == false, 'refract steps negative')\n\
         assert(render.set_water_refract_steps(16.5) == false, 'refract steps integer')\n\
         assert(render.set_water_refract_steps(0/0) == false, 'refract steps finite')\n\
         assert(render.set_water_refract_steps('16') == false, 'refract steps string rejected')\n\
         local rsok, rsreason = render.set_water_refract_steps(16)\n\
         assert(rsok, rsreason)\n\
         assert(render.set_shadow_caster_distance(1001) == false, 'caster distance range')\n\
         assert(render.set_shadow_caster_distance(-1) == false, 'caster distance negative')\n\
         assert(render.set_shadow_caster_distance(20.5) == false, 'caster distance integer')\n\
         assert(render.set_shadow_caster_distance(0/0) == false, 'caster distance finite')\n\
         assert(render.set_shadow_caster_distance('20') == false, 'caster distance string rejected')\n\
         local cdok, cdreason = render.set_shadow_caster_distance(20)\n\
         assert(cdok, cdreason)\n\
         local stats = render.stats()\n\
         assert(stats.draw_calls == 0 and stats.vertices == 0 and stats.triangles == 0, 'stats defaults')\n\
         function update() end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.render_fog_density(), Some(0.003));
    assert_eq!(runtime.render_fog_color(), Some([0.52, 0.60, 0.70]));
    assert_eq!(runtime.render_cloud_coverage(), Some(0.8));
    assert_eq!(runtime.render_cloud_speed(), Some(1.5));
    assert_eq!(runtime.render_cloud_base(), Some(90.0));
    assert_eq!(runtime.render_cloud_shadow_strength(), Some(0.35));
    assert_eq!(runtime.render_flash(), Some(0.6));
    assert_eq!(runtime.render_water_tint(), Some([0.8, 0.9, 0.85, 0.3]));
    assert_eq!(runtime.render_quality(), Some(2));
    assert_eq!(runtime.render_surface_cover(), Some([0.6, 0.4]));
    assert_eq!(runtime.render_exposure(), Some(-0.3));
    assert_eq!(runtime.take_render_ripple(), Some([1.5, -2.0]));
    assert_eq!(runtime.render_post_fx(), Some([0.3, 0.2]));
    assert_eq!(runtime.render_clip_cap(), Some(2.5));
    // 显式关也必须留痕（Some(false)）：宿主靠它把启动期的 `--clip-cap-both-sides` 覆盖掉，
    // None 才是「脚本没碰过、保持宿主档」。
    assert_eq!(runtime.render_clip_cap_both_sides(), Some(false));
    assert_eq!(runtime.render_water_refract_steps(), Some(16));
    assert_eq!(runtime.render_shadow_caster_distance(), Some(20));
    let _ = fs::remove_file(path);
}

/// P3-R283：距离预算的「没碰过 = None」与「显式关 = Some(0)」必须分得开——
/// 宿主每帧只在 `Some` 时写入渲染器，若 0 被当成 None，脚本就永远关不掉
/// 启动期的 `--shadow-caster-distance`。抓的假绿：把 0 归一成 None。
#[test]
fn shadow_caster_distance_keeps_explicit_off_distinct_from_untouched() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    assert_eq!(runtime.render_shadow_caster_distance(), None, "脚本没碰过");
    let path = script(
        "local ok, reason = render.set_shadow_caster_distance(40)\n\
         assert(ok, reason)\n\
         function update() end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.render_shadow_caster_distance(), Some(40));
    let _ = fs::remove_file(path);
    let off = script(
        "local ok, reason = render.set_shadow_caster_distance(0)\n\
         assert(ok, reason)\n\
         function update() end",
    );
    runtime.execute_project_script(&off).unwrap();
    assert_eq!(
        runtime.render_shadow_caster_distance(),
        Some(0),
        "显式关必须留痕，才能覆盖宿主启动档"
    );
    let _ = fs::remove_file(off);
}
