//! Headless acceptance drive for M32 相机收口：**相机跟随玩家 + 取景不出图**。
//!
//! 需求（本轮）：`game_jhyt` 的相机跟随角色，角色始终保持在视野中心，相机视野不能
//! 超出地图边缘。
//!
//! 改造前的事实：工程里**没有任何场景声明相机**，Lua 侧也没有一处 `camera2d.follow`
//! ⇒ 运行时一直是默认相机（中心 `(640,360)`、`zoom=1`、无 bounds）：整幅世界摊在屏幕上、
//! 角色走到哪儿都不跟随（`examples/game_jhyt/docs/52` §4 曾记为有意留下的偏差）。
//!
//! 现在的实现分三处，本样例逐条钉住：
//!   * **场景**：每张玩法地图一个 `GameCamera` 实体，`Camera` 组件声明
//!     `projection=orthographic;center=<地图中心>;zoom=2;bounds=0,0,W*32,H*32`；
//!   * **脚本**：`scripts/world/camera_view.lua` 在每次进图后 `camera2d.follow(player)`
//!     （offset/死区/平滑全 0 ⇒ 玩家始终在视野正中）；
//!   * **引擎**：每帧渲染前用 `Camera2D::constrained_center` 把相机中心夹到地图内。
//!
//! 2026-09-27 转横屏 + 2026-10 对标星露谷 **zoom=2**（docs/51 §2.2，`camera_view.lua`
//! 校验器强制每张场景 Camera zoom=2）后的几何事实（写清楚，别当成 bug）：
//!   室外地图 40×23 格 = 1280×736 世界单位，视口 1280×720、zoom=2 ⇒
//!   可见世界域 = 640×360 单位（20×11.25 格，屏幕 tile=64px）：
//!   * **x 轴**：可见宽(640) < 地图宽(1280) ⇒ 夹取区间 [320, 960]——
//!     玩家在中段时相机精确跟随，贴东西墙时夹到区间端点（约半屏跟随余量）；
//!   * **y 轴**：可见高(360) < 地图高(736) ⇒ 该轴可夹取，区间 [180, 556]——
//!     玩家贴地图上下沿时相机各让 180 单位，视野始终在图内。
//!   * 一句话：**两轴都"跟随 + 不出界"同时成立，x 轴贴墙时留出 320 单位的夹取余量。**
//!
//! 为什么用 `RuntimeScene` 而不是裸 `LuaRuntime`：`camera2d.get()` 读的是**宿主每帧
//! 回填的真实相机**，而"处理请求 → 跟随 → 夹取"这条链在 `RuntimeScene::step_camera2d`
//! 里（裸 `LuaRuntime` 永远不会跑它，读数会一直停在 0）。视口同样只在**真实渲染路径**
//! 上有值，因此每一步都先 `vertices(1280,720)`（= 发布窗口尺寸）再推进相机。
//!
//! 章节：
//!   ① 门面：玩法地图之外不跟随；
//!   ② 进图第一帧：相机 x 精确跟随出生点（528 落在中段区间 [320, 960] 内）、y 夹到区间下界 180；
//!   ③ 中段跟随：玩家 x 落在 [320, 960] 内时，相机 x == 玩家 x（可见矩形始终在图内）；
//!   ④ 不出图（贴东/西墙）：相机 x 夹到区间端点 320/960，且由四个屏幕角
//!      反投影回来的可见矩形仍在 [0,1280]×[0,736] 内（"视野不超出地图边缘"的直接证据）；
//!   ⑤ 横屏 y 轴事实：地图(736) 比可见域(360) 高 ⇒ 该轴可夹取，区间 [180, 556]，
//!      上/中/下三个探针分别钉下界、跟随、钉上界；
//!   ⑥ 换图：青云山道重绑跟随目标、bounds 换成新地图矩形（贴东墙夹到 960）；
//!      20×12 的室内图 x 轴退化钉 320、y 轴玩家出生在区间外夹到下界 180；
//!   ⑦ 回到门面：取消跟随（不留指向已销毁实体的目标）。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m32_camera [-- <game_jhyt-root>]

use engine::host::RuntimeScene;
use rustlua_input::input::InputSnapshot;
use std::{env, path::PathBuf};

/// 设计分辨率 = 发布窗口尺寸 = 相机视口（横屏 1280×720）。
const VIEW_W: f32 = 1280.0;
const VIEW_H: f32 = 720.0;
/// 相机 zoom（docs/51 §2.2 + `world/camera_view.lua` 校验器强制 zoom=2，对标星露谷）：
/// 屏幕 tile=64px、一屏 20×11.25 格。可见世界域 = 视口 / zoom。
const ZOOM: f32 = 2.0;
const VIEW_WORLD_W: f32 = VIEW_W / ZOOM; // 640
const VIEW_WORLD_H: f32 = VIEW_H / ZOOM; // 360
/// 室外地图 40×23 格 × 32 世界单位（`pixel_kit.camera_line` 与场景 bounds 同源）。
const MAP_W: f32 = 1280.0;
const MAP_H: f32 = 736.0;
/// x 轴可用的相机中心区间 = `[bounds.min.x + 半可见宽, bounds.max.x - 半可见宽]`，
/// 即引擎 `Camera2D::constrained_center` 的夹取口径（bounds.min.x = 0）。
/// **横屏 zoom=2**：可见宽 640 < 地图宽 1280 ⇒ 区间 [320, 960]，中段跟随、贴墙夹到两端。
const CENTER_MIN: f32 = VIEW_WORLD_W / 2.0; // 320
const CENTER_MAX: f32 = MAP_W - VIEW_WORLD_W / 2.0; // 960
/// y 轴同款区间 `[180, 556]`：地图高 736 比可见高 360 高出一倍多，上/下各 180 单位余量。
const CENTER_MIN_Y: f32 = VIEW_WORLD_H / 2.0; // 180
const CENTER_MAX_Y: f32 = MAP_H - VIEW_WORLD_H / 2.0; // 556
/// 夹取/投影容差：f32 投影链路有 ~1e-4 量级误差，用 1e-2（远小于一个瓦片 32）。
const EPS: f32 = 1e-2;
/// "玩家在视野正中"的容差（zoom=2 贴墙时恒不成立，仅用于贴墙章节的
/// "玩家不可能在正中"判定）：贴墙 x=16/1264 时中心夹到 320/960，偏差数百单位，
/// 用 EPS 即可区分"夹到边界"与"意外跟随到玩家"。
const FOLLOW_EPS: f32 = 0.6;
const DT: f32 = 1.0 / 60.0;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        rustlua_infra::games_dir()
            .join("examples")
            .join("game_jhyt")
    });
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

    banner(
        "② 进入山庄：进图后相机跟随出生点（x=528 落在中段夹取区间 [320,960] 内）；y 夹到区间下界",
    );
    lua_run(&mut runtime, "on_click('BtnStart')")?;
    frame(&mut runtime)?;
    assert_scene(&mut runtime, "manor")?;
    let first = step_until_settled(&mut runtime)?;
    if !first.following {
        return Err(format!("进图后应处于跟随状态：{first:?}").into());
    }
    // zoom=2 横屏：可见宽 640 < 地图宽 1280 ⇒ 出生点 x=528 在中段区间内，相机 x 精确跟随
    // 玩家（夹取区间 [320, 960] 不含 528 之外的边界）；y=112 在区间下界 180 之外 ⇒ 夹到 180。
    expect_center_x(&first, first.player.0, "进图第一帧")?;
    if (first.center.1 - CENTER_MIN_Y).abs() > EPS {
        return Err(format!(
            "进图第一帧：相机中心 y={:.3}，应夹到 y 区间下界 {}（落点玩家 y≈112，在夹取区间之外）",
            first.center.1, CENTER_MIN_Y
        )
        .into());
    }
    println!(
        "  player=({:.3},{:.3}) center=({:.3},{:.3}) zoom={:?}",
        first.player.0, first.player.1, first.center.0, first.center.1, first.zoom
    );

    banner("③ 中段跟随：玩家 x 落在夹取区间 [320, 960] 内时，相机 x 精确跟随玩家");
    // 横屏 zoom=2 几何：可见宽 640 < 地图宽 1280 ⇒ 夹取区间 [320, 960]，中段内相机中心
    // = 玩家位置（无死区/无平滑时逐帧精确跟随；传送若被碰撞推出，中心跟随的是玩家**实际**
    // 落点而非探针目标）。断言口径：**center.x == player.x** + 可见矩形仍在地图内
    // （"不出图"的直接证据）。若有人把 zoom 改回 1，区间退化为单点 640，
    // 本节会立刻红——这正是"夹取区间随 zoom 同步收窄"的回归哨。
    for probe in [420.0_f32, 520.0, 640.0, 760.0, 860.0] {
        teleport_player(&mut runtime, probe, 336.0)?;
        let status = step_until_settled(&mut runtime)?;
        expect_center_x(&status, status.player.0, &format!("x={probe}"))?;
        expect_view_inside(&status, MAP_W, MAP_H, &format!("x={probe}"))?;
        println!(
            "  玩家 x={:>7.3} → center=({:>7.3},{:>7.3}) 可见矩形 x[{:.1},{:.1}]",
            status.player.0,
            status.center.0,
            status.center.1,
            status.center.0 - VIEW_WORLD_W / 2.0,
            status.center.0 + VIEW_WORLD_W / 2.0
        );
    }

    banner("④ 不出图：贴西/东墙时相机 x 夹到区间端点 320/960，可见矩形仍在地图内");
    for (label, x, want) in [
        ("贴西墙", 16.0_f32, CENTER_MIN),
        ("贴东墙", 1264.0, CENTER_MAX),
    ] {
        teleport_player(&mut runtime, x, 336.0)?;
        let status = step_until_settled(&mut runtime)?;
        expect_center_x(&status, want, label)?;
        expect_view_inside(&status, MAP_W, MAP_H, label)?;
        // 相机 x 夹在区间端点时玩家**不可能**仍在屏幕正中（贴墙时偏差数百单位）——断言必须
        // 显式承认它，否则将来有人把 bounds 删掉、相机乱漂，只断言"不出图"的测试照样全绿。
        let centered = (status.player.0 - status.center.0).abs() <= FOLLOW_EPS;
        if centered {
            return Err(format!("{label}：相机夹到端点时玩家不可能仍在正中，断言口径有误").into());
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
            "  {label}(x={x:>6.1}) → center=({:>7.3},{:>7.3}) 可见世界矩形 x[{:.1},{:.1}] y[{:.1},{:.1}]（地图 x[0,1280] y[0,736]）",
            status.center.0, status.center.1, corners.0, corners.1, corners.2, corners.3
        );
    }

    banner(
        "⑤ 横屏 y 轴事实：地图比可见域高 ⇒ y 可夹取，区间 [180, 556]（上钉下界/中跟随/下钉上界）",
    );
    for (probe_y, want_y, label, follow) in [
        (112.0_f32, CENTER_MIN_Y, "贴地图上部", false),
        (368.0_f32, 0.0_f32, "地图纵向正中", true),
        (640.0_f32, CENTER_MAX_Y, "贴地图下部", false),
    ] {
        teleport_player(&mut runtime, 640.0, probe_y)?;
        let status = step_until_settled(&mut runtime)?;
        let expected = if follow { status.player.1 } else { want_y };
        if (status.center.1 - expected).abs() > EPS {
            return Err(format!(
                "{label}：相机中心 y={:.3}，应为 {:.3}（夹取区间 [{CENTER_MIN_Y}, {CENTER_MAX_Y}]）",
                status.center.1, expected
            )
            .into());
        }
        expect_view_inside(&status, MAP_W, MAP_H, label)?;
        println!(
            "  玩家 y={probe_y:>6.1} → center y={:>7.3} 可见区间 y[{:.1},{:.1}]",
            status.center.1,
            status.center.1 - VIEW_WORLD_H / 2.0,
            status.center.1 + VIEW_WORLD_H / 2.0
        );
    }

    banner("⑥ 换图：青云山道重绑跟随目标、bounds 换新；室内图(20×12) 两轴都退化");
    goto_map(&mut runtime, "mountain_path")?;
    let mountain = step_until_settled(&mut runtime)?;
    if !mountain.following {
        return Err("换图后应仍处于跟随状态".into());
    }
    teleport_player(&mut runtime, 1200.0, 336.0)?;
    let mountain = step_until_settled(&mut runtime)?;
    // 青云山道也是 1280×736、zoom=2 ⇒ x 夹取区间 [320, 960]，贴东墙（x=1200）夹到 960。
    expect_center_x(&mountain, CENTER_MAX, "mountain_path 贴东墙")?;
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
    // 室内 20×12=640×384、zoom=2：可见 640×360 ⇒ x 区间 [320,320] 退化为单点钉 320；
    // y 区间 [180, 204]，玩家出生 (336,112) 在区间外 ⇒ 夹到 180。
    if (small.center.0 - 320.0).abs() > EPS || (small.center.1 - 180.0).abs() > EPS {
        return Err(format!(
            "室内图(20×12=640×384) 相机中心应钉在 (320,180)（x 退化单点、y 夹下界）：实际 ({:.3},{:.3})",
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

/// 相机中心 x == 期望值（zoom=2 横屏夹取区间 [320, 960] 内的跟随/端点断言）。
///
/// y 轴单独由 `expect_view_inside` 与章节 ⑤ 的夹取区间断言负责。
fn expect_center_x(
    status: &CameraStatus,
    want: f32,
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let dx = status.center.0 - want;
    if dx.abs() > EPS {
        return Err(format!(
            "{label}：相机中心 x={:.3}，应等于 {want:.3}（zoom=2 夹取区间 [{CENTER_MIN}, {CENTER_MAX}]）",
            status.center.0
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
        ("x", status.center.0, VIEW_WORLD_W, map_w),
        ("y", status.center.1, VIEW_WORLD_H, map_h),
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
