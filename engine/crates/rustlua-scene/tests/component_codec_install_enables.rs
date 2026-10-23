//! 装表之后同一个场景就能编——证明"缺表"才是上面那些失败的原因，不是别处坏了。
//!
//! 单独一个进程：`install` 是进程级 `OnceLock`，装了就不能撤销，所以"未装表"的断言
//! 在另一个测试二进制里（`component_codec_requires_install.rs`）。

use rustlua_scene::component_codec::{self, ComponentCodecRegistry};
use rustlua_scene::scene::{Scene, SceneEntityId};
use rustlua_scene::typed_scene::{ComponentId, TypedComponent, TypedScene};

fn scene_with(components: &[(&str, &str)]) -> Scene {
    let mut scene = Scene::new("codec-install");
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

#[test]
fn installing_the_render_and_ui_tables_enables_sprite_and_text() {
    assert!(!component_codec::is_installed());
    let mut all: Vec<&'static rustlua_component_codec::ComponentCodec> =
        rustlua_render::scene_codec::scene_codecs().to_vec();
    all.extend_from_slice(rustlua_ui::scene_codec::scene_codecs());
    component_codec::install(ComponentCodecRegistry::from_codecs(&all)).expect("只装一次");
    assert!(component_codec::is_installed());
    assert_eq!(
        component_codec::global().len(),
        14,
        "渲染层 6 个 + UI 层 8 个 = 14"
    );

    let scene = scene_with(&[
        ("Sprite", "texture=assets/hero.png;size=8x8"),
        ("Text", "text=Hello;font_size=24;font=assets/fonts/My.ttf"),
        ("LayoutItem", "grow=1;visible=true"),
    ]);
    let typed = TypedScene::compile_v2(&scene).expect("装表后应能编译");

    // 原文逐字节带进 `Opaque`：`.scene` 往返不会被本层改写。
    let Some(TypedComponent::Opaque { payload, assets }) =
        typed.entities[0].components.get(&ComponentId::Sprite)
    else {
        panic!("Sprite 应编译成 Opaque 组件");
    };
    assert_eq!(payload, "texture=assets/hero.png;size=8x8");
    assert_eq!(
        assets,
        &vec![("assets/hero.png".to_owned(), None)],
        "没有资源库时资源身份保持未解析，不得编造"
    );

    // `Text` 的自定义字体也要被收集成资源引用（发布包缺字会在这里露出来）。
    let Some(TypedComponent::Opaque { assets, .. }) =
        typed.entities[0].components.get(&ComponentId::Text)
    else {
        panic!("Text 应编译成 Opaque 组件");
    };
    assert_eq!(assets, &vec![("assets/fonts/My.ttf".to_owned(), None)]);

    // E05-2/E05-4：`LayoutItem` 是运行时认识的容器子项组件，不再是"未知组件"。
    let Some(TypedComponent::Opaque { payload, .. }) =
        typed.entities[0].components.get(&ComponentId::LayoutItem)
    else {
        panic!("LayoutItem 应编译成 Opaque 组件");
    };
    assert_eq!(payload, "grow=1;visible=true");
}
