//! 表现层组件 codec 的**装表入口**（`docs/62` P1 第 4 条）。
//!
//! `rustlua-scene` 不再依赖 `rustlua-render` / `rustlua-ui`：它把 `Sprite`/`Text` 这类组件的
//! 载荷当**不透明文本**搬运，合法性交给归属 crate 的 codec 校验（见
//! `rustlua_component_codec` 的模块文档）。那些 codec 由组合根装一次表。
//!
//! 本模块就是那个"组合根的一处"：`engine` 同时看见渲染层与 UI 层，`editor` 又依赖 `engine`，
//! 所以双方都调这里，**不会各装一套**（重复装会被注册表拒绝，本函数因此做成幂等：
//! 第二次起直接返回 `false`）。
//!
//! 谁需要调它：任何会**编译场景文本**的入口——`RuntimeScene::init`（场景实例化的唯一入口，
//! 预制体套用与运行时校验都在下游）、编辑器场景视图、`validate_component_payload` 的调用方、
//! 跑场景校验的 examples/tests。不编译场景的进程（纯运行时帧循环）不调也行：
//! 它读的是 ECS 里的类型化组件，不是字符串。

/// 装一次表现层组件 codec 表（渲染层 5 个 + UI 层 7 个 = 12 个）。
///
/// 幂等：已经装过（或别的入口先装了）就返回 `false`，不报错。**不要**用它来检测"是不是我装的"
/// ——需要那个语义时用 `rustlua_scene::component_codec::is_installed()`。
pub fn install() -> bool {
    let mut all: Vec<&'static rustlua_component_codec::ComponentCodec> =
        rustlua_render::scene_codec::scene_codecs().to_vec();
    all.extend_from_slice(rustlua_ui::scene_codec::scene_codecs());
    let registry = rustlua_component_codec::ComponentCodecRegistry::from_codecs(&all);
    rustlua_component_codec::install(registry).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 装表必须**幂等**：多个入口（编辑器主窗口 + 场景视图 + 校验器）都会调，
    /// 若第二次就报错，装配顺序会变成隐性依赖。
    #[test]
    fn install_is_idempotent() {
        let first = install();
        let second = install();
        assert!(!second, "第二次装表必须返回 false（幂等）");
        let _ = first;
        assert!(rustlua_scene::component_codec::is_installed());
        assert_eq!(
            rustlua_scene::component_codec::global().len(),
            12,
            "渲染层 5 个 + UI 层 7 个 = 12"
        );
    }

    /// 装表之后，带表现层组件的场景文本必须能编译（这条是端到端的：codec 真的接上了）。
    #[test]
    fn a_scene_with_sprite_and_text_compiles_after_install() {
        install();
        let mut scene = rustlua_scene::scene::Scene::new("codec-smoke");
        let id = rustlua_scene::scene::SceneEntityId(1);
        scene.add_entity(id, "hero", None).unwrap();
        let entity = scene.entities.get_mut(&id).unwrap();
        entity
            .components
            .insert("Sprite".to_owned(), "texture=a.png;size=8x8".to_owned());
        entity
            .components
            .insert("Text".to_owned(), "text=Hi;font_size=24".to_owned());
        let typed = rustlua_scene::typed_scene::TypedScene::compile_v2(&scene)
            .expect("装表后 Sprite/Text 必须能编译");
        assert_eq!(typed.entities[0].components.len(), 2);
    }
}
