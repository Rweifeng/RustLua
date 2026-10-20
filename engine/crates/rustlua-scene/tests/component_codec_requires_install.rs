//! 未装表时的行为门：**硬错误**，不是静默跳过（`docs/62` P1 第 4 条）。
//!
//! 为什么单独一个进程：装表是进程级的 `OnceLock`，而 `rustlua-scene` 的单元测试会自己装一份
//! （见 `typed_scene::install_codecs_for_tests`）。要观察"没装表"的语义，必须在一个**从未装过表**
//! 的进程里跑——集成测试是独立可执行文件，天然满足。
//!
//! 这条门挡的回归：把"已知名字但缺 codec"改成 `None`（未知组件）或直接跳过。那样工程里的
//! `Sprite` 会在无提示的情况下消失，而所有既有测试仍然全绿。
//!
//! 装表之后的行为在 `component_codec_install_enables.rs` 里（另一个进程：装表不可撤销）。

use rustlua_scene::component_codec;
use rustlua_scene::scene::{Scene, SceneEntityId};
use rustlua_scene::typed_scene::{TypedScene, validate_component_payload};

fn scene_with(components: &[(&str, &str)]) -> Scene {
    let mut scene = Scene::new("codec-gate");
    let id = SceneEntityId(1);
    scene.add_entity(id, "hero", None).unwrap();
    let entity = scene.entities.get_mut(&id).unwrap();
    for (name, value) in components {
        entity
            .components
            .insert((*name).to_owned(), (*value).to_owned());
    }
    scene
}

/// 未装表 → 严格编译失败，且诊断里**有组件名和装表指引**。
#[test]
fn compiling_a_render_component_without_a_codec_is_a_hard_error() {
    assert!(!component_codec::is_installed(), "本测试进程不该装过表");

    let scene = scene_with(&[("Sprite", "texture=hero.png;w=8;h=8")]);
    let errors = TypedScene::compile_v2(&scene).expect_err("缺 codec 必须判红");
    assert_eq!(errors.len(), 1, "只应有一个组件诊断：{errors:?}");
    let message = &errors[0].message;
    assert_eq!(errors[0].component, "Sprite");
    assert!(
        message.contains("未安装"),
        "诊断要说清是缺 codec，而不是'未知组件'：{message}"
    );
    assert!(
        message.contains("scene_codecs"),
        "诊断要给出装表指引（谁提供 codec）：{message}"
    );
}

/// 宽容编译同样不能静默：组件被丢弃，但必须留下一条诊断。
#[test]
fn lenient_compile_reports_the_missing_codec_instead_of_dropping_silently() {
    let scene = scene_with(&[("Text", "text=Hello;font_size=24")]);
    let (typed, diagnostics) = TypedScene::compile_v2_lenient(&scene);
    assert!(
        typed.entities[0].components.is_empty(),
        "缺 codec 时不能编出组件"
    );
    assert_eq!(diagnostics.len(), 1, "必须留诊断：{diagnostics:?}");
    assert!(diagnostics[0].message.contains("未安装"));
}

/// 编辑器走的校验入口三态不变：已知名字 + 缺 codec = `Some(Err)`（不是 `None`）。
#[test]
fn payload_validation_reports_a_missing_codec_as_an_error() {
    let result = validate_component_payload("Sprite", "texture=a.png;w=8;h=8");
    let error = result
        .expect("已知组件名必须返回 Some(..)，None 是'引擎不认识它'")
        .expect_err("缺 codec 不能当作合法");
    assert!(error.contains("未安装"), "{error}");
}

/// 场景层自己的组件不受影响：它们的文法就在本 crate，永远不需要装表。
#[test]
fn scene_owned_components_still_compile_without_any_codec() {
    let scene = scene_with(&[("Transform", "position=1,2,3;rotation=0,0,0,1;scale=1,1,1")]);
    let typed = TypedScene::compile_v2(&scene).expect("Transform 不依赖表现层 codec");
    assert_eq!(typed.entities.len(), 1);
}
