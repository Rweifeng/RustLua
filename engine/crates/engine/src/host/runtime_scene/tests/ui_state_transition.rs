//! M80-07：状态底色过渡的宿主门——时长/曲线真的驱动绘制，不只是记了个状态。
//!
//! 读数一律取自 `push_scene_ui_images` 提交的顶点颜色：那是宿主自己算出来的东西，
//! 拿 `ui_visual_state` 自证只能证明状态机动了，证明不了画面跟着动。
//!
//! 三件事必须成立：
//! - 过渡第一帧显示的还是作者底色（渐变起点，不是瞬切）；
//! - 中途换态（hover→pressed）从**当前显示色**继续，既不回落到底色也不跳到新目标色；
//! - 没配 transition 的节点第一帧就到位，且不留过渡条目（逐字节回到旧行为）。

use super::*;
use rustlua_input::input::{InputSnapshot, MouseButton};
use rustlua_scene::scene::{Scene, SceneEntityId};

const VIEW_W: u32 = 1280;
const VIEW_H: u32 = 720;
const STEP: f32 = 1.0 / 60.0;
/// 夹具里过渡配的是 0.2 秒 = 12 帧，多跑几帧保证已经落定。
const SETTLE_STEPS: usize = 14;

fn add(scene: &mut Scene, id: u64, name: &str, components: &[(&str, &str)]) {
    scene.add_entity(SceneEntityId(id), name, None).unwrap();
    let entity = scene.entities.get_mut(&SceneEntityId(id)).unwrap();
    for (key, value) in components {
        entity.components.insert((*key).into(), (*value).into());
    }
}

fn tint_runtime(states: &str) -> RuntimeScene {
    let mut scene = Scene::new("ui_state_transition");
    add(
        &mut scene,
        1,
        "TintButton",
        &[
            (
                "Image",
                "texture=assets/textures/scroll_clip_probe.png;color=0,0,1,1",
            ),
            ("UiLayout", "anchors=0,0,0,0;offsets=100,100,300,200"),
            ("UiEvents", "event=click;function=on_click"),
            ("ButtonStates", states),
        ],
    );
    RuntimeScene::from_scene_edit(None, scene).unwrap()
}

fn cursor_at(x: f64, y: f64) -> InputSnapshot {
    let mut input = InputSnapshot::default();
    input.cursor_moved(x, y);
    input
}

fn press_at(x: f64, y: f64) -> InputSnapshot {
    let mut input = cursor_at(x, y);
    input.mouse_event(MouseButton::Left, true);
    input
}

fn step(runtime: &mut RuntimeScene, input: &InputSnapshot) {
    runtime.run_script_steps(1, STEP, input).unwrap();
}

/// 取 `TintButton` 那块四边形提交上来的颜色——左上角 (100,100) 的角点唯一确定它。
fn displayed_color(runtime: &mut RuntimeScene) -> [f32; 4] {
    let mut vertices = Vec::new();
    let mut draws = Vec::new();
    runtime
        .push_scene_ui_images(&mut vertices, &mut draws, VIEW_W, VIEW_H)
        .unwrap();
    let expected = [
        100.0 / VIEW_W as f32 * 2.0 - 1.0,
        1.0 - 100.0 / VIEW_H as f32 * 2.0,
    ];
    let vertex = vertices
        .iter()
        .find(|vertex| {
            (vertex.position[0] - expected[0]).abs() < 1e-3
                && (vertex.position[1] - expected[1]).abs() < 1e-3
        })
        .unwrap_or_else(|| {
            panic!(
                "提交顶点里没有 TintButton 的角点 {:?}：{:?}",
                expected,
                vertices
                    .iter()
                    .map(|vertex| vertex.position)
                    .collect::<Vec<_>>()
            )
        });
    vertex.color
}

fn assert_close(actual: f32, expected: f32, what: &str) {
    assert!(
        (actual - expected).abs() < 2e-3,
        "{what}: 期望 {expected}，实际 {actual}"
    );
}

#[test]
fn hover_tint_ramps_over_the_authored_duration() {
    let mut runtime = tint_runtime("tint.hover=1,0,0,1;transition=0.2:linear");
    // 按钮中心 (200,150)。
    step(&mut runtime, &cursor_at(200.0, 150.0));
    let first = displayed_color(&mut runtime);
    assert_close(first[0], 0.0, "过渡第一帧的红通道");
    assert_close(first[2], 1.0, "过渡第一帧仍是作者底色");

    let mut reds = vec![first[0]];
    for _ in 0..SETTLE_STEPS {
        step(&mut runtime, &cursor_at(200.0, 150.0));
        let color = displayed_color(&mut runtime);
        reds.push(color[0]);
    }
    for pair in reds.windows(2) {
        assert!(
            pair[1] >= pair[0],
            "底色只能朝目标状态单调推进，实测 {reds:?}"
        );
    }
    // 第 7 帧（含首帧共 6 次推进）恰好走完一半：0.2 秒 = 12 帧。
    assert_close(reds[6], 0.5, "走满一半时长应是中点色");
    let settled = reds[SETTLE_STEPS];
    assert_close(settled, 1.0, "时长走完后就是 hover 目标色");
    assert_close(displayed_color(&mut runtime)[2], 0.0, "落定后蓝通道归零");
}

#[test]
fn state_switch_continues_from_the_displayed_color() {
    let mut runtime = tint_runtime("tint.hover=1,0,0,1;tint.pressed=0,1,0,1;transition=0.2:linear");
    for _ in 0..3 {
        step(&mut runtime, &cursor_at(200.0, 150.0));
    }
    let hovering = displayed_color(&mut runtime);
    assert!(
        hovering[0] > 0.0 && hovering[0] < 0.5,
        "按之前应还在渐变路上: {hovering:?}"
    );

    step(&mut runtime, &press_at(200.0, 150.0));
    let pressed = displayed_color(&mut runtime);
    // 这一帧的显示色 = 被打断那一刻的色：既没回落到作者底色（红=0、蓝=1），也没跳到目标绿。
    assert_close(
        pressed[0],
        hovering[0] + STEP / 0.2,
        "换态那一帧从当前显示色继续",
    );
    assert_close(pressed[1], 0.0, "换态当帧绿通道还没起步");
    assert!(
        pressed[2] > 0.5,
        "红通道必须带着 hover 的进度，不能从底色重放: {pressed:?}"
    );

    for _ in 0..SETTLE_STEPS {
        step(&mut runtime, &press_at(200.0, 150.0));
    }
    let settled = displayed_color(&mut runtime);
    assert_close(settled[1], 1.0, "落定后是 pressed 目标色");
    assert_close(settled[0], 0.0, "落定后红通道归零");
}

#[test]
fn tint_without_transition_lands_on_the_first_frame() {
    let mut runtime = tint_runtime("tint.hover=1,0,0,1");
    step(&mut runtime, &cursor_at(200.0, 150.0));
    let color = displayed_color(&mut runtime);
    assert_close(color[0], 1.0, "没配过渡就该立刻是目标色");
    assert_close(color[2], 0.0, "没配过渡就不该留渐变尾巴");
    assert!(
        runtime.ui_state_transitions.is_empty(),
        "时长为 0 不得登记过渡条目"
    );
}
