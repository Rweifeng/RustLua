//! 原 `mod frame_schedule_tests`（WP12 阶段 1 纯搬运；源行段 10027–10407）。
//! M18-03 共享帧调度管线测试：内置阶段顺序、注册 API、依赖/循环/panic 诊断
//! 与调度性能统计出口。这些测试是 headless 的：直接驱动 RuntimeScene。

use super::*;
use rustlua_audio::spatial::Listener;
use rustlua_camera::camera::CameraBounds;
use std::cell::RefCell;
use std::rc::Rc;

fn sample_root() -> std::path::PathBuf {
    rustlua_infra::games_dir().join("examples").join("game_api")
}

fn sample_runtime() -> RuntimeScene {
    let root = sample_root();
    RuntimeScene::new(Some(&root)).expect("runtime for game_api sample project")
}

/// 带真实 `Camera3D` 的运行时：`game_api` 的 `demo3d.scene`。
///
/// 不能拿 `sample_runtime()`（工程的 `main.scene`）当 3D 用例——它是 2D 场景，没有
/// `Camera3D`，`active_camera3d()` 返回的是 (0,0,5) 的回退相机；M74-01 的 2D 分界
/// 会把听者切到 `Camera2D` 中心。
fn demo3d_runtime() -> RuntimeScene {
    let root = sample_root();
    let scene = rustlua_scene::scene::Scene::load(root.join("scenes/demo3d.scene"))
        .expect("load demo3d.scene");
    RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime demo3d")
}

/// M74-01：听者位姿必须由宿主**每帧**灌进音频系统。
///
/// 守的是"接线在不在"：`play_at` 在没有听者时直接拒绝（`SpatialError::NoListener`），
/// 所以这一处一旦被删掉，游戏里所有空间音效会**整个不响**——而纯音频单测、`cargo check`、
/// 示例都发现不了（示例只测纯模型，宿主不在它的路径上）。
///
/// 口径：听者 = **本帧渲染所用的那台相机**。渲染路径已经有定帧视图时宿主复用它（不额外解析
/// 一次，见 `tests/frame_view.rs` 的解析次数账本）；headless（本例就是）没有视图，于是解析一次
/// ——所以这里的期望值取 `active_camera3d()`。
#[test]
fn listener_follows_the_active_camera_each_frame() {
    let mut runtime = demo3d_runtime();
    // 构造期不该凭空有听者：`None` 就是"宿主还没接线"的判据。
    assert!(
        runtime.audio_listener().is_none(),
        "构造期听者必须为空（否则分不清接线前的状态）"
    );
    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("跑一帧");
    let listener = runtime.audio_listener().expect("跑过一帧后听者必须就位");
    let camera = runtime.active_camera3d();
    let expected = Listener::facing(
        [camera.position.x, camera.position.y, camera.position.z],
        [camera.target.x, camera.target.y, camera.target.z],
    );
    assert_eq!(listener, expected, "听者必须就是本帧那台相机");
    // 第二帧照样重灌（不是"只在第一帧灌一次"：相机随玩法移动，听者必须跟着走）。
    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("再跑一帧");
    assert_eq!(
        runtime.audio_listener(),
        Some(expected),
        "每帧都要重灌（相机不动时必须还是同一份）"
    );
}

/// M74-01：**纯 2D 场景**（没有任何透视 `Camera3D`）的听者取 `Camera2D` 中心。
///
/// 守的是真回归：`active_camera3d()` 在 2D 工程里返回 (0,0,5) 的回退相机，直接拿它当听者，
/// 2D 相机一平移，声音仍按世界原点衰减/定声像——相机在 x=120 时，世界 0 点的声音会被当成
/// "很近"，而相机附近的声音反而被衰减/定到一侧。`forward` 取 `-Z`：声像右轴是
/// `forward × up`，`+Z × +Y = -X` 会让屏幕右变成耳朵左（任务书旧稿写反过）。
#[test]
fn two_d_listener_follows_camera2d_center() {
    let mut runtime =
        RuntimeScene::from_scene_edit(None, rustlua_scene::scene::Scene::new("2d-listener"))
            .expect("runtime scene");
    runtime.set_camera_2d(
        rustlua_camera::camera::Camera2D::new(rustlua_math::math2d::Vec2::new(120.0, 80.0), 1.0)
            .expect("camera2d"),
    );
    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("跑一帧");
    let listener = runtime.audio_listener().expect("跑过一帧后听者必须就位");
    assert_eq!(
        listener.position,
        [120.0, 80.0, 0.0],
        "2D 听者必须跟随相机中心，不是 3D 回退相机的 (0,0,5)"
    );
    assert_eq!(
        listener.forward,
        [0.0, 0.0, -1.0],
        "2D 听者朝 -Z，屏幕 +X 才是耳朵右"
    );
    let params = rustlua_audio::spatial::evaluate(
        [120.5, 80.0, 0.0],
        &listener,
        rustlua_audio::spatial::SpatialModel::DEFAULT,
    )
    .expect("evaluate");
    assert!(
        (params.pan - 1.0).abs() < 1e-6,
        "相机右侧 0.5 m 的声音应 pan=+1：{}",
        params.pan
    );
    assert_eq!(params.gain, 1.0, "0.5 m 在 min_distance 内，不应衰减");

    // 相机平移后听者必须跟着走（不是只在第一帧灌一次）。
    runtime.set_camera_2d(
        rustlua_camera::camera::Camera2D::new(rustlua_math::math2d::Vec2::new(300.0, 80.0), 1.0)
            .expect("camera2d"),
    );
    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("再跑一帧");
    assert_eq!(
        runtime.audio_listener().map(|listener| listener.position),
        Some([300.0, 80.0, 0.0]),
        "2D 相机平移后听者必须跟到新中心"
    );
}

/// **回归**：换图后必须重读新场景声明的 `Camera`，否则场景声明的 bounds 只在启动场景生效。
///
/// 怎么坏的：`process_scene_transition` 重建整个世界（`World::new()` 后重新实例化），
/// 换图后那台 `Camera` 是**新的一份**；宿主原先只在 `init`（启动场景）读过一次，
/// 于是 `self.camera.bounds` 永远停在开工那张图的值上——每张地图各声明一份 bounds 的
/// 工程（`examples/game_jhyt`）里，之后所有地图的相机都不受约束，而且**没有任何报错**。
///
/// 断言四件事：① 启动场景（门面，无相机）用历史默认、无 bounds；② 换到声明了相机的
/// 场景后 bounds 变成新场景那一份；③ **center 不被重置**（姿态的权威仍是每帧的
/// `camera2d.*`，重置回作者中心会在换图那一帧闪一下）；④ **zoom 跟新场景走**（门面
/// 无声明 ⇒ 起步 1；进入声明 zoom=2 的地图后必须是 2——zoom 没有每帧属主，场景声明
/// 是唯一真源，换图不读它则门面形态的工程永远停在 1）。
#[test]
fn scene_declared_camera_bounds_refresh_after_scene_switch() {
    let root = rustlua_infra::games_dir()
        .join("examples")
        .join("game_jhyt");
    if !root.join("scenes/manor.scene").is_file() {
        eprintln!("跳过：checkout 内没有 examples/game_jhyt");
        return;
    }
    let mut runtime = RuntimeScene::new(Some(&root)).expect("runtime for game_jhyt");
    let idle = InputSnapshot::default();
    runtime.run_script_steps(1, 1.0 / 60.0, &idle).unwrap();
    assert_eq!(runtime.script.scene_name().as_deref(), Some("main"));
    assert_eq!(
        runtime.camera_2d().bounds,
        None,
        "门面 main.scene 没有声明相机 ⇒ 不该有 bounds"
    );
    assert_eq!(
        runtime.camera_2d().zoom,
        1.0,
        "门面 main.scene 没有声明相机 ⇒ zoom 保持历史默认 1"
    );

    // 进入山庄：bounds 换成地面瓦片图的矩形（40×23 格 × 32 = 1280×736）。
    runtime
        .with_script(|script| script.execute_console("on_click('BtnStart')"))
        .unwrap();
    runtime.run_script_steps(1, 1.0 / 60.0, &idle).unwrap();
    assert_eq!(runtime.script.scene_name().as_deref(), Some("manor"));
    let manor = runtime.camera_2d();
    let expected =
        CameraBounds::new(Vec2::new(0.0, 0.0), Vec2::new(1280.0, 736.0)).expect("manor bounds");
    assert_eq!(
        manor.bounds,
        Some(expected),
        "换图后应读入新场景声明的 bounds"
    );
    assert_eq!(
        manor.zoom, 2.0,
        "换图后应读入新场景声明的 zoom（门面没声明，只能在这里生效）"
    );
    // ③ 姿态不被重置：`enter_playable_map` 先落点再 follow，下一帧相机中心 = 玩家坐标
    // （624,112，#28 起山庄出生点是格 (19,3)）。若 bounds 同步顺手把 center 重置成场景作者
    // 值，这里会变成 (640,368)——
    // 注意 y 最终仍会被夹到 bounds 内（地图 736 ≥ 视口高），所以只断言 x 未被重置。
    runtime.vertices(720, 1280).expect("render viewport");
    runtime.run_script_steps(1, 1.0 / 60.0, &idle).unwrap();
    let followed = runtime.camera_2d();
    assert!(
        (followed.center.x - 624.0).abs() < 1.0,
        "换图后 center.x 应由跟随决定（624），实际 {}",
        followed.center.x
    );

    // 换到室内（20×12 = 640×384）：bounds 必须跟着地图换，而不是留在山庄那份。
    runtime
        .with_script(|script| script.execute_console("map[\"goto\"]('interior_herb')"))
        .unwrap();
    runtime.run_script_steps(1, 1.0 / 60.0, &idle).unwrap();
    assert_eq!(
        runtime.script.scene_name().as_deref(),
        Some("interior_herb")
    );
    let small =
        CameraBounds::new(Vec2::new(0.0, 0.0), Vec2::new(640.0, 384.0)).expect("interior bounds");
    assert_eq!(
        runtime.camera_2d().bounds,
        Some(small),
        "换到更小的地图后 bounds 应同步缩小"
    );
}

/// 固定步顺序必须与原手工编排一致：
/// input_apply → lua_update → ui_click_dispatch → physics2d → physics3d →
/// particles → clock → scene_transition；帧级系统每调用一次：
/// integrate_velocity → animation_state_advance → animation3d_advance →
/// animation_player_advance。
#[test]
fn builtin_pipeline_preserves_legacy_stage_order() {
    let mut runtime = sample_runtime();
    let order: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let recorder = |name: &'static str, order: Rc<RefCell<Vec<String>>>| {
        move |_scene: &mut RuntimeScene, _dt: f32| {
            order.borrow_mut().push(name.to_owned());
            Ok(())
        }
    };
    runtime
        .add_step_system(System::new(
            "user_script_probe",
            ScheduleStage::Script,
            ["lua_update"],
            recorder("user_script_probe", Rc::clone(&order)),
        ))
        .unwrap();
    runtime
        .add_step_system(System::new(
            "user_physics_probe",
            ScheduleStage::Physics,
            [] as [&str; 0],
            recorder("user_physics_probe", Rc::clone(&order)),
        ))
        .unwrap();
    runtime
        .run_script_steps(2, 1.0 / 60.0, &InputSnapshot::default())
        .expect("scheduled steps");
    // 两个用户探针各自每步执行一次，且 Script 阶段探针先于 Physics 探针。
    assert_eq!(
        order.borrow().as_slice(),
        [
            "user_script_probe",
            "user_physics_probe",
            "user_script_probe",
            "user_physics_probe",
        ]
    );
    // 调度统计的 timings 按「首次执行顺序」记录，完整反映共享管线的阶段顺序。
    // WP7 收敛后的统一序（负责人裁定）：脚本 → 静态/运动学同步 → 物理 → 动态回写 →
    // （模拟/动画）→ 世界变换传播；`spatial_propagate` 在动画之后，因为动画也会改局部变换，
    // 传播必须是**最后**一步，提取才能看到本帧的最终世界位置。
    let report = runtime.last_schedule_report().expect("schedule report");
    let names: Vec<&str> = report.timings.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "input_apply",
            "lua_update",
            "ui_click_dispatch",
            // docs/54 §4.1：每实体脚本紧跟工程级脚本（`ui_click_dispatch` 之后、用户系统之前）
            "entity_scripts",
            // UI 绑定解析：实体脚本把绑定的值写入后、用户系统读之前解析（M7x 新增 stage）
            "ui_bindings_resolve",
            "user_script_probe",
            "physics_sync",
            "physics2d_step",
            "physics3d_step",
            "user_physics_probe",
            "physics_writeback",
            "scene_emitters_step",
            "particles_step",
            "clock_step",
            "scene_transition",
            // 换图后重读新场景声明的 `Camera`（bounds 必须跟着地图走），排在换图之后。
            "scene_camera_sync",
            "integrate_velocity",
            "camera2d_controller",
            "animation_state_advance",
            "animation3d_advance",
            "animation_player_advance",
            "spatial_propagate",
        ]
    );
}

#[test]
fn schedule_report_collects_steps_and_timings() {
    let mut runtime = sample_runtime();
    runtime
        .run_script_steps(3, 1.0 / 60.0, &InputSnapshot::default())
        .expect("scheduled steps");
    let report = runtime
        .last_schedule_report()
        .expect("schedule report is recorded");
    assert_eq!(report.steps, 3);
    let names: Vec<&str> = report.timings.iter().map(|t| t.name.as_str()).collect();
    for expected in [
        "input_apply",
        "lua_update",
        "ui_click_dispatch",
        "physics2d_step",
        "physics3d_step",
        "particles_step",
        "clock_step",
        "scene_transition",
        "integrate_velocity",
        "animation_state_advance",
        "animation3d_advance",
        "animation_player_advance",
    ] {
        assert!(names.contains(&expected), "missing timing: {expected}");
    }
    assert!(report.total_elapsed >= report.timings.iter().map(|t| t.elapsed).sum::<Duration>());
    // 帧级系统只执行一次：其累计耗时应远小于每步系统（同量级下不可靠，
    // 仅验证 steps 计数与 timings 覆盖）。
    assert!(report.total_elapsed.as_nanos() > 0);
}

#[test]
fn duplicate_system_names_are_rejected_at_registration() {
    let mut runtime = sample_runtime();
    let error = runtime
        .add_step_system(System::new(
            "lua_update",
            ScheduleStage::Script,
            [] as [&str; 0],
            |_: &mut RuntimeScene, _| Ok(()),
        ))
        .expect_err("duplicate builtin name must be rejected");
    assert!(matches!(error, SchedulerError::DuplicateSystem { .. }));
    runtime
        .add_frame_system(System::new(
            "frame_probe",
            ScheduleStage::Animation,
            [] as [&str; 0],
            |_: &mut RuntimeScene, _| Ok(()),
        ))
        .unwrap();
    let error = runtime
        .add_step_system(System::new(
            "frame_probe",
            ScheduleStage::Transform,
            [] as [&str; 0],
            |_: &mut RuntimeScene, _| Ok(()),
        ))
        .expect_err("cross-pipeline duplicate name must be rejected");
    assert!(matches!(error, SchedulerError::DuplicateSystem { .. }));
}

/// 回归：注册失败绝不能"吃掉"已经注册的系统。
///
/// 旧实现每帧把系统 `drain` 进新调度器、遇到 `add_system` 失败就 `break`：
/// `Drain` 在提前退出时会连同**尚未搬迁**的系统一起丢弃，一次失败的注册就能
/// 让后续的内建系统（物理、时钟、动画）静默消失。现在系统所有权始终在调度器
/// 里，被拒绝的注册只影响它自己。
#[test]
fn rejected_registration_never_drops_registered_systems() {
    let mut runtime = sample_runtime();
    let (step_names, frame_names) = runtime.registered_system_names();
    let builtin_steps = step_names.len();
    assert!(builtin_steps >= 8, "builtin step systems: {step_names:?}");
    assert!(!frame_names.is_empty());

    // 重名的注册必须被拒绝。
    let error = runtime
        .add_step_system(System::new(
            "lua_update",
            ScheduleStage::Script,
            [] as [&str; 0],
            |_: &mut RuntimeScene, _| Ok(()),
        ))
        .expect_err("duplicate name must be rejected");
    assert!(matches!(error, SchedulerError::DuplicateSystem { .. }));

    // 被拒绝之后：注册表一个不少，调度仍然完整跑通。
    let (step_names_after, frame_names_after) = runtime.registered_system_names();
    assert_eq!(step_names_after, step_names);
    assert_eq!(frame_names_after, frame_names);
    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("schedule must still run after a rejected registration");
    let names: Vec<&str> = runtime
        .last_schedule_report()
        .expect("schedule report is recorded")
        .timings
        .iter()
        .map(|timing| timing.name.as_str())
        .collect();
    for expected in [
        "input_apply",
        "lua_update",
        "physics2d_step",
        "clock_step",
        "integrate_velocity",
    ] {
        assert!(names.contains(&expected), "dropped system: {expected}");
    }
}

#[test]
fn missing_dependency_is_diagnosed_with_system_names() {
    let mut runtime = sample_runtime();
    runtime
        .add_step_system(System::new(
            "orphan_probe",
            ScheduleStage::Script,
            ["no_such_system"],
            |_: &mut RuntimeScene, _| Ok(()),
        ))
        .unwrap();
    let error = runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect_err("missing dependency must fail the run");
    let message = error.to_string();
    assert!(
        message.contains("orphan_probe") && message.contains("no_such_system"),
        "diagnostic must name system and dependency: {error}"
    );
    // 失败后注册仍在（系统已放回），修复依赖后可重新执行。
    let (step_names, _) = runtime.registered_system_names();
    assert!(step_names.contains(&"orphan_probe".to_owned()));
}

#[test]
fn dependency_cycle_is_diagnosed_with_the_cycle_members() {
    let mut runtime = sample_runtime();
    runtime
        .add_step_system(System::new(
            "cycle_a",
            ScheduleStage::Physics,
            ["cycle_b"],
            |_: &mut RuntimeScene, _| Ok(()),
        ))
        .unwrap();
    runtime
        .add_step_system(System::new(
            "cycle_b",
            ScheduleStage::Physics,
            ["cycle_a"],
            |_: &mut RuntimeScene, _| Ok(()),
        ))
        .unwrap();
    let error = runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect_err("dependency cycle must fail the run");
    let message = error.to_string();
    assert!(
        message.contains("cycle_a") && message.contains("cycle_b"),
        "cycle diagnostic must name the members: {error}"
    );
}

#[test]
fn system_panic_is_diagnosed_and_registration_survives() {
    let mut runtime = sample_runtime();
    runtime
        .add_step_system(System::new(
            "panic_probe",
            ScheduleStage::Transform,
            [] as [&str; 0],
            |_: &mut RuntimeScene, _| panic!("probe-boom"),
        ))
        .unwrap();
    let error = runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect_err("panicking system must fail the run");
    let message = error.to_string();
    assert!(
        message.contains("panic_probe") && message.contains("probe-boom"),
        "panic diagnostic must name system and payload: {error}"
    );
    let (step_names, _) = runtime.registered_system_names();
    assert!(step_names.contains(&"panic_probe".to_owned()));
}

#[test]
fn systems_owning_script_state_run_inside_the_shared_schedule() {
    // 通过 with_script 在注册系统内驱动引擎脚本 API（Rust 侧玩法系统
    // 的官方接入路径），并验证 frame 系统每次调用只执行一次。
    let mut runtime = sample_runtime();
    let runs = Rc::new(RefCell::new(0u32));
    let counter = Rc::clone(&runs);
    runtime
        .add_frame_system(System::new(
            "frame_counter_probe",
            ScheduleStage::Transform,
            [] as [&str; 0],
            move |scene: &mut RuntimeScene, _dt: f32| {
                *counter.borrow_mut() += 1;
                scene.with_script(|script| {
                    script.set_delta(0.0);
                });
                Ok(())
            },
        ))
        .unwrap();
    runtime
        .run_script_steps(4, 1.0 / 60.0, &InputSnapshot::default())
        .expect("scheduled steps");
    assert_eq!(*runs.borrow(), 1, "frame systems run once per call");
    runtime
        .run_script_steps(0, 1.0 / 60.0, &InputSnapshot::default())
        .expect("zero-step call still runs the frame pipeline");
    assert_eq!(*runs.borrow(), 2);
}
