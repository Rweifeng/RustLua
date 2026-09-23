//! Headless acceptance drive for M32 相机收口：**相机跟随玩家 + 取景不出图**。
//!
//! 需求（本轮）：`game_jhyt` 的相机跟随角色，角色始终保持在视野中心，相机视野不能
//! 超出地图边缘。
//!
//! 改造前的事实：工程里**没有任何场景声明相机**，Lua 侧也没有一处 `camera2d.follow`
//! ⇒ 运行时一直是默认相机（中心 `(640,360)`、`zoom=1`、无 bounds）：整幅世界摊在屏幕上、
//! 角色走到哪儿都不跟随，竖屏多出来的高度还会把地图外的空白画出来
//! （`examples/game_jhyt/docs/52` §4 把它记为有意留下的偏差）。
//!
//! 现在的实现分三处，本样例逐条钉住：
//!   * **场景**：每张玩法地图一个 `GameCamera` 实体，`Camera` 组件声明
//!     `projection=orthographic;center=<地图中心>;zoom=1;bounds=0,0,W*32,H*32`；
//!   * **脚本**：`scripts/world/camera_view.lua` 在每次进图后 `camera2d.follow(player)`
//!     （offset/死区/平滑全 0 ⇒ 玩家始终在视野正中）；
//!   * **引擎**：每帧渲染前用 `Camera2D::constrained_center` 把相机中心夹到地图内。
//!
//! 为什么用 `RuntimeScene` 而不是裸 `LuaRuntime`：`camera2d.get()` 读的是**宿主每帧
//! 回填的真实相机**，而"处理请求 → 跟随 → 夹取"这条链在 `RuntimeScene::step_camera2d`
//! 里（裸 `LuaRuntime` 永远不会跑它，读数会一直停在 0）。视口同样只在**真实渲染路径**
//! 上有值，因此每一步都先 `vertices(720,1280)`（= 发布窗口尺寸）再推进相机。
//!
//! 章节：
//!   ① 门面：玩法地图之外不跟随；
//!   ② 进图第一帧：相机中心 = 玩家坐标（场景声明的中心只是**初始**值，不是第二份真源）；
//!   ③ 跟随：玩家在地图中段的每个 x 上，相机中心 x 逐点相等；
//!   ④ 不出图（贴东/西墙）：相机中心被夹到 `[360, 920]`，且**由四个屏幕角反投影回来的
//!      可见矩形仍在 [0,1280]×[0,704] 内**（这是"视野不超出地图边缘"的直接证据）；
//!   ⑤ 竖屏 y 轴事实：地图(704) 比视口(1280) 矮 ⇒ 该轴不可夹取，引擎取边界中点，
//!      相机被钉在地图纵向中心 352（有意的退化，见 `camera_view.lua` 头注释）；
//!   ⑥ 换图：青云山道重绑跟随目标、bounds 换成新地图矩形；20×12 的室内图两轴都退化；
//!   ⑦ 回到门面：取消跟随（不留指向已销毁实体的目标）。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m32_camera [-- <game_jhyt-root>]

use engine::core::input::InputSnapshot;
use engine::host::RuntimeScene;
use std::{env, path::PathBuf};

/// 竖版设计分辨率 = 发布窗口尺寸 = 相机视口（`zoom=1` 时世界单位与像素 1:1）。
const VIEW_W: f32 = 720.0;
const VIEW_H: f32 = 1280.0;
/// 室外地图 40×22 格 × 32 世界单位（`pixel_kit.camera_line` 与场景 bounds 同源）。
const MAP_W: f32 = 1280.0;
const MAP_H: f32 = 704.0;
/// x 轴可用的相机中心区间 = `[bounds.min.x + 半视口, bounds.max.x - 半视口]`，
/// 即引擎 `Camera2D::constrained_center` 的夹取口径（bounds.min.x = 0）。
const CENTER_MIN: f32 = VIEW_W / 2.0; // 360
const CENTER_MAX: f32 = MAP_W - VIEW_W / 2.0; // 920
/// 夹取/投影容差：f32 投影链路有 ~1e-4 量级误差，用 1e-2（远小于一个瓦片 32）。
const EPS: f32 = 1e-2;
/// "玩家在视野正中"的容差：相机控制器每步用**上一步**的玩家位置推进，玩家自己在
/// 碰撞推挤下每帧还会挪零点几个单位，因此这个判定留 0.6 世界单位（约 2% 瓦片）——
/// 比"每帧精确相等"诚实，又远小于"跟随失效"时的偏差（可达数百单位）。
const FOLLOW_EPS: f32 = 0.6;
const DT: f32 = 1.0 / 60.0;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| engine::core::games_dir().join("examples").join("game_jhyt"));
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    let mut runtime = RuntimeScene::new(Some(project.as_path()))?;
    frame(&mut runtime)?;
    assert_scene(&mut runtime, "main")?;

    banner("① 门面：玩法地图之外不跟随（`camera2d.unfollow`）");
    let title = camera_status(&mut runtime)?;
    if title.following {
        return Err("门面不应处于跟随状态".into());
    }
    println!("  跟随状态 = {}", title.following);

    banner("② 进入山庄：进图后相机中心 x = 玩家 x；y 因地图比视口矮而钉在地图中心");
    lua_run(&mut runtime, "on_click('BtnStart')")?;
    frame(&mut runtime)?;
    assert_scene(&mut runtime, "manor")?;
    let first = camera_status(&mut runtime)?;
    if !first.following {
        return Err(format!("进图后应处于跟随状态：{first:?}").into());
    }
    expect_centered_x(&first, "进图第一帧")?;
    if (first.center.1 - MAP_H / 2.0).abs() > EPS {
        return Err(format!(
            "进图第一帧：相机中心 y={:.3}，应为地图纵向中心 {}（视口 1280 > 地图 704）",
            first.center.1,
            MAP_H / 2.0
        )
        .into());
    }
    println!(
        "  player=({:.3},{:.3}) center=({:.3},{:.3}) zoom={:?}",
        first.player.0, first.player.1, first.center.0, first.center.1, first.zoom
    );

    banner("③ 跟随：玩家 x 落在夹取区间内时，相机中心 x 逐点等于玩家 x（玩家在视野正中）");
    // 可跟随区间 = [CENTER_MIN, CENTER_MAX]：玩家靠近地图两侧时相机先被夹住，因此
    // 那一段（④ 章）玩家必然偏离正中——这是"视野不出图"的代价，不是跟随失效。
    //
    // 断言口径：与**玩家实际位置**比，不与"我想瞬移到哪"比。`teleport` 落点在实心格
    // 附近时物理会把刚体推开零点几个世界单位（半径 12 的球），那是物理的正确行为。
    for probe in [420.0_f32, 520.0, 640.0, 760.0, 860.0] {
        teleport_player(&mut runtime, probe, 336.0)?;
        let status = step_until_settled(&mut runtime)?;
        let delta = (status.center.0 - status.player.0).abs();
        if delta > FOLLOW_EPS {
            return Err(format!(
                "瞬移到 x={probe} 后玩家在 x={:.3}：相机中心 x={:.3}，偏差 {delta:.3} > {FOLLOW_EPS}",
                status.player.0, status.center.0
            )
            .into());
        }
        expect_view_inside(&status, MAP_W, MAP_H, &format!("x={probe}"))?;
        println!(
            "  玩家 x={:>7.3} → center=({:>7.3},{:>7.3}) 偏差 {:.3}",
            status.player.0, status.center.0, status.center.1, delta
        );
    }

    banner("④ 不出图：贴西/东墙时相机被夹到 [360, 920]，可见矩形仍在地图内");
    for (label, x, expect) in [
        ("贴西墙", 16.0_f32, CENTER_MIN),
        ("贴东墙", 1264.0, CENTER_MAX),
    ] {
        teleport_player(&mut runtime, x, 336.0)?;
        let status = step_until_settled(&mut runtime)?;
        if (status.center.0 - expect).abs() > EPS {
            return Err(format!(
                "{label}(x={x})：相机中心 x={:.3}，应被夹到 {expect}",
                status.center.0
            )
            .into());
        }
        expect_view_inside(&status, MAP_W, MAP_H, label)?;
        // 夹住时玩家**不可能**仍在屏幕正中——这正是"视野不出图"的代价。断言必须显式
        // 承认它，否则将来有人把 bounds 删掉，只断言"居中"的测试照样全绿。
        let centered = (status.player.0 - status.center.0).abs() <= EPS;
        if centered {
            return Err(format!("{label}：相机被夹住时玩家不可能仍在正中，断言口径有误").into());
        }
        // 直接证据：把四个屏幕角反投影回世界，可见矩形必须落在地图矩形内。
        let corners = visible_world_rect(&mut runtime)?;
        if corners.0 < -EPS || corners.1 > MAP_W + EPS {
            return Err(format!(
                "{label}：可见世界矩形 x[{:.1},{:.1}] 超出地图 [0,{MAP_W}]",
                corners.0, corners.1
            )
            .into());
        }
        println!(
            "  {label}(x={x:>6.1}) → center=({:>7.3},{:>7.3}) 可见世界矩形 x[{:.1},{:.1}] y[{:.1},{:.1}]（地图 x[0,1280] y[0,704]）",
            status.center.0, status.center.1, corners.0, corners.1, corners.2, corners.3
        );
    }

    banner("⑤ 竖屏 y 轴事实：地图比视口矮 ⇒ y 被钉在地图中心（有意的退化）");
    let status = camera_status(&mut runtime)?;
    if (status.center.1 - MAP_H / 2.0).abs() > EPS {
        return Err(format!(
            "y 轴应钉在地图纵向中心 {}：实际 {}",
            MAP_H / 2.0,
            status.center.1
        )
        .into());
    }
    println!(
        "  视口高 {VIEW_H} > 地图高 {MAP_H} ⇒ 上界({}) < 下界({})，引擎按中点取 {}",
        MAP_H / 2.0 - VIEW_H / 2.0,
        MAP_H / 2.0 + VIEW_H / 2.0,
        status.center.1
    );

    banner("⑥ 换图：青云山道重绑跟随目标、bounds 换新；室内图(20×12) 两轴都退化");
    goto_map(&mut runtime, "mountain_path")?;
    let mountain = step_until_settled(&mut runtime)?;
    if !mountain.following {
        return Err("换图后应仍处于跟随状态".into());
    }
    teleport_player(&mut runtime, 1200.0, 336.0)?;
    let mountain = step_until_settled(&mut runtime)?;
    if (mountain.center.0 - CENTER_MAX).abs() > EPS {
        return Err(format!(
            "青云山道贴东墙：相机中心 x={:.3}，应被夹到 {CENTER_MAX}",
            mountain.center.0
        )
        .into());
    }
    expect_view_inside(&mountain, MAP_W, MAP_H, "mountain_path")?;
    println!(
        "  mountain_path center=({:.3},{:.3}) entity={:?}",
        mountain.center.0, mountain.center.1, mountain.entity
    );

    goto_map(&mut runtime, "interior_herb")?;
    let small = step_until_settled(&mut runtime)?;
    if !small.following {
        return Err("室内图应仍处于跟随状态".into());
    }
    if (small.center.0 - 320.0).abs() > EPS || (small.center.1 - 192.0).abs() > EPS {
        return Err(format!(
            "室内图(20×12=640×384) 相机中心应钉在地图中心 (320,192)：实际 ({:.3},{:.3})",
            small.center.0, small.center.1
        )
        .into());
    }
    expect_view_inside(&small, 640.0, 384.0, "interior_herb")?;
    println!(
        "  药庐 20×12 center=({:.3},{:.3})",
        small.center.0, small.center.1
    );

    banner("⑦ 回到门面：取消跟随");
    lua_run(&mut runtime, "jianghu_return_to_title()")?;
    frame(&mut runtime)?;
    assert_scene(&mut runtime, "main")?;
    let back = camera_status(&mut runtime)?;
    if back.following {
        return Err("回到门面后应取消跟随".into());
    }
    println!("  跟随状态 = {}", back.following);

    println!(
        "\njhyt_m32_camera_ok map={}x{} view={}x{} clamp_x=[{}, {}]",
        MAP_W as i64,
        MAP_H as i64,
        VIEW_W as i64,
        VIEW_H as i64,
        CENTER_MIN as i64,
        CENTER_MAX as i64
    );
    Ok(())
}

// ---------- 断言 ----------

/// 玩家 x == 相机中心 x（"角色始终保持在视野中心"在**可跟随轴**上的判定）。
///
/// y 轴上地图(704)比视口(1280)矮，夹取必然把相机钉在地图中心、玩家因此不可能同时居中
/// ——那是几何事实，所以这里只判 x，y 由 `expect_view_inside` 判"停在地图中心"。
fn expect_centered_x(status: &CameraStatus, label: &str) -> Result<(), Box<dyn std::error::Error>> {
    let dx = status.player.0 - status.center.0;
    if dx.abs() > EPS {
        return Err(format!(
            "{label}：玩家 x={:.3} 不在视野中心 x={:.3}，偏差 {dx:.3}",
            status.player.0, status.center.0
        )
        .into());
    }
    Ok(())
}

/// 可见矩形是否在地图内。某轴地图比视口小时该轴**不可能**夹住，引擎按边界中点取值，
/// 那一轴只断言"相机停在边界中点"——这是该轴上"视野不出图"能给出的最强保证。
fn expect_view_inside(
    status: &CameraStatus,
    map_w: f32,
    map_h: f32,
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    for (axis, center, span, map) in [
        ("x", status.center.0, VIEW_W, map_w),
        ("y", status.center.1, VIEW_H, map_h),
    ] {
        let half = span / 2.0;
        if map >= span {
            if center - half < -EPS || center + half > map + EPS {
                return Err(format!(
                    "{label}：{axis} 轴可见区间 [{:.3},{:.3}] 超出地图 [0,{map:.3}]",
                    center - half,
                    center + half
                )
                .into());
            }
        } else if (center - map / 2.0).abs() > EPS {
            return Err(format!(
                "{label}：{axis} 轴地图({map:.3})比视口({span})小，相机应钉在地图中心 {:.3}，实际 {center:.3}",
                map / 2.0
            )
            .into());
        }
    }
    Ok(())
}

// ---------- 驱动 ----------

#[derive(Debug)]
struct CameraStatus {
    following: bool,
    entity: Option<i64>,
    center: (f32, f32),
    zoom: Option<f32>,
    player: (f32, f32),
}

fn camera_status(runtime: &mut RuntimeScene) -> Result<CameraStatus, Box<dyn std::error::Error>> {
    let following = lua_bool(runtime, "require('world.camera_view').status().following")?;
    let entity = lua_num_opt(runtime, "require('world.camera_view').status().entity")
        .map(|value| value as i64);
    let center = camera_center(runtime)?;
    let zoom = lua_num_opt(runtime, "require('world.camera_view').status().zoom");
    // 门面（没有玩家实体）上 `player` 为 nil，取到 (0,0) 占位；跟随态下的断言才有意义。
    let player = match (
        lua_num_opt(runtime, "require('world.camera_view').status().player.x"),
        lua_num_opt(runtime, "require('world.camera_view').status().player.y"),
    ) {
        (Some(x), Some(y)) => (x, y),
        _ => (0.0, 0.0),
    };
    Ok(CameraStatus {
        following,
        entity,
        center,
        zoom,
        player,
    })
}

/// 宿主回填的真实相机中心（`camera2d.get` 读的是引擎在用的值，不是脚本自述）。
fn camera_center(runtime: &mut RuntimeScene) -> Result<(f32, f32), Box<dyn std::error::Error>> {
    let cx = lua_num(runtime, "camera2d.get().x")?;
    let cy = lua_num(runtime, "camera2d.get().y")?;
    Ok((cx, cy))
}

/// 四个屏幕角反投影回世界的包围盒 `(min_x, max_x, min_y, max_y)`。
///
/// 口径注意：`camera2d.screen_to_world` / `world_to_screen` 返回的是**数组表**
/// `{x, y}`（`script.rs` 的 `table_vec`，下标 1/2），而 `camera2d.get` 返回**具名表**
/// `{x=, y=}`——两套表不是同一种，取字段时要按各自的口径。
fn visible_world_rect(
    runtime: &mut RuntimeScene,
) -> Result<(f32, f32, f32, f32), Box<dyn std::error::Error>> {
    let script = format!(
        "local a = camera2d.screen_to_world(0, 0)\n\
         local b = camera2d.screen_to_world({}, {})\n\
         return table.concat({{math.min(a[1], b[1]), math.max(a[1], b[1]), math.min(a[2], b[2]), math.max(a[2], b[2])}}, ',')",
        VIEW_W as i64, VIEW_H as i64
    );
    let text = lua_str(runtime, &format!("(function() {script} end)()"))?;
    let values: Vec<f32> = text
        .split(',')
        .map(|part| part.trim().parse::<f32>())
        .collect::<Result<_, _>>()?;
    if values.len() != 4 {
        return Err(format!("可见矩形解析失败：{text:?}").into());
    }
    Ok((values[0], values[1], values[2], values[3]))
}

/// 推进一帧并返回相机状态。**每帧都先走真实渲染路径**（`vertices`）——视口与
/// `camera2d.screen_to_world` 的回填只在那条路径上有值（headless 不调它就会一直用
/// 默认的 1280×720，夹取结果与发布窗口不一致）。
fn frame(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    runtime.vertices(VIEW_W as u32, VIEW_H as u32)?;
    runtime
        .run_script_steps(1, DT, &InputSnapshot::default())
        .map_err(|error| error.into())
}

/// 玩家瞬移后推进到相机收敛（无平滑 ⇒ 一帧到位；多跑两帧让物理与动画跟上）。
fn step_until_settled(
    runtime: &mut RuntimeScene,
) -> Result<CameraStatus, Box<dyn std::error::Error>> {
    for _ in 0..3 {
        frame(runtime)?;
    }
    camera_status(runtime)
}

fn teleport_player(
    runtime: &mut RuntimeScene,
    x: f32,
    y: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    lua_run(
        runtime,
        &format!("require('player.player').teleport({x:.4}, {y:.4})"),
    )
}

fn goto_map(runtime: &mut RuntimeScene, map_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    // `goto` 是 Lua 保留字，必须用 `map["goto"]` 取（与 `world/map_manager.lua` 同一写法）。
    lua_run(runtime, &format!("map[\"goto\"]('{map_id}')"))?;
    frame(runtime)
}

fn assert_scene(
    runtime: &mut RuntimeScene,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let scene = lua_str(runtime, "scene.reload()")?;
    if scene != expected {
        return Err(format!("scene = {scene:?}, want {expected:?}").into());
    }
    Ok(())
}

// ---------- Lua 取值 ----------

fn lua_run(runtime: &mut RuntimeScene, expr: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.with_script(|script| script.execute_console(expr))?;
    Ok(())
}

fn lua_str(runtime: &mut RuntimeScene, expr: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(runtime
        .with_script(|script| script.execute_console(&format!("return tostring({expr})")))?
        .trim()
        .to_owned())
}

fn lua_bool(runtime: &mut RuntimeScene, expr: &str) -> Result<bool, Box<dyn std::error::Error>> {
    Ok(lua_str(runtime, expr)? == "true")
}

fn lua_num(runtime: &mut RuntimeScene, expr: &str) -> Result<f32, Box<dyn std::error::Error>> {
    lua_num_opt(runtime, expr).ok_or_else(|| format!("{expr} 不是数字").into())
}

fn lua_num_opt(runtime: &mut RuntimeScene, expr: &str) -> Option<f32> {
    let text = runtime
        .with_script(|script| {
            script.execute_console(&format!(
                "local v = {expr}\n\
                 if type(v) == 'number' then return tostring(v) end\n\
                 return 'nil'"
            ))
        })
        .ok()?;
    text.trim().parse::<f32>().ok()
}

fn banner(title: &str) {
    println!("\n=== {title} ===");
}
