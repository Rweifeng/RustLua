//! 表现层组件的**载荷契约**（`docs/62` P1 第 4 条）。
//!
//! 场景文本里这些组件的文法属于本 crate：字段语义、跨字段校验、转义规则都只有这一份
//! 实现（各类型的 `from_component_str`）。`rustlua-scene` 不再 `use` 本 crate 的类型，
//! 而是通过 `rustlua_component_codec` 注册表拿到这里的**校验函数**，把原文原样带进
//! `TypedScene`；类型化消费方（渲染器、编辑器）拿原文回来自己解析。
//!
//! **装表**：`engine` / `editor` 启动时把 `scene_codecs()` 与
//! `rustlua_ui::scene_codecs()` 一起装进 `rustlua_component_codec::install(...)`。
//! 未装表时 scene 对 `Sprite` 这类组件报**硬错误**（不是静默跳过）。
//!
//! **不许**在本 crate 里反向依赖 `rustlua-scene`：那会把渲染层拖进场景层，环就换了个方向。

use rustlua_component_codec::ComponentCodec;

use crate::light::Light;
use crate::mesh3d::MeshRenderer;
use crate::quad::Quad;
use crate::render_mode::RenderMode;
use crate::sprite::SpriteComponent;

/// `Light`：颜色/强度/类型/范围。不引用资源。
fn validate_light(value: &str) -> Result<(), String> {
    Light::from_component_str(value)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// `MeshRenderer`：网格路径 + 材质。资源路径 = `mesh`。
fn validate_mesh(value: &str) -> Result<(), String> {
    MeshRenderer::from_component_str(value)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn mesh_assets(value: &str) -> Vec<String> {
    // 解析失败时返回空：资源缺失的可见性归资源库依赖图，场景编译不重复报同一件事。
    MeshRenderer::from_component_str(value)
        .map(|renderer| vec![renderer.mesh])
        .unwrap_or_default()
}

/// `Quad`：矩形几何 + 可选贴图。资源路径 = `texture`（可选）。
fn validate_quad(value: &str) -> Result<(), String> {
    Quad::from_component_str(value)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn quad_assets(value: &str) -> Vec<String> {
    Quad::from_component_str(value)
        .ok()
        .and_then(|quad| quad.texture)
        .into_iter()
        .collect()
}

/// `Sprite`：贴图 + 尺寸/锚点/着色/排序/翻转/九宫格。资源路径 = `texture`。
fn validate_sprite(value: &str) -> Result<(), String> {
    SpriteComponent::from_component_str(value)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn sprite_assets(value: &str) -> Vec<String> {
    SpriteComponent::from_component_str(value)
        .map(|sprite| vec![sprite.texture])
        .unwrap_or_default()
}

/// `RenderMode`：显式 2D 显示模式（world / overlay / screen）。不引用资源。
fn validate_render_mode(value: &str) -> Result<(), String> {
    RenderMode::from_component_str(value)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

static LIGHT: ComponentCodec = ComponentCodec::new("Light", validate_light);
static MESH: ComponentCodec =
    ComponentCodec::new("MeshRenderer", validate_mesh).with_assets(mesh_assets);
static QUAD: ComponentCodec = ComponentCodec::new("Quad", validate_quad).with_assets(quad_assets);
static SPRITE: ComponentCodec =
    ComponentCodec::new("Sprite", validate_sprite).with_assets(sprite_assets);
static RENDER_MODE: ComponentCodec = ComponentCodec::new("RenderMode", validate_render_mode);

/// 本 crate 负责的五个组件 codec。组合根把它们与 `rustlua_ui::scene_codecs()` 一起装表。
#[must_use]
pub fn scene_codecs() -> &'static [&'static ComponentCodec] {
    static CODECS: [&ComponentCodec; 5] = [&LIGHT, &MESH, &QUAD, &SPRITE, &RENDER_MODE];
    &CODECS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 五个名字都是场景文件里真实在用的写法（`docs/54` §4.3、26 个源场景普查）。
    #[test]
    fn codecs_cover_every_render_component_name() {
        let names: Vec<&str> = scene_codecs().iter().map(|codec| codec.name()).collect();
        assert_eq!(
            names,
            vec!["Light", "MeshRenderer", "Quad", "Sprite", "RenderMode"]
        );
    }

    /// 校验必须**与归属 crate 的解析器同源**：这里用几组真实载荷，合法就过、坏的就拒。
    #[test]
    fn validation_matches_the_owner_crate_parsers() {
        assert!(SPRITE.validate("texture=a.png;size=16x16").is_ok());
        assert!(SPRITE.validate("texture=a.png;size=abc").is_err());

        assert!(MESH.validate("mesh=assets/models/hero.glb").is_ok());
        assert!(MESH.validate("mesh").is_err());

        assert!(RENDER_MODE.validate("mode=world").is_ok());
        assert!(RENDER_MODE.validate("mode=nonsense").is_err());

        assert!(QUAD.validate("size=8,8;color=1,1,1,1").is_ok());
    }

    /// 资源路径要能被场景层拿去问资源库；解析不了时返回空而不是报错。
    #[test]
    fn asset_paths_are_reported_or_empty_never_error() {
        assert_eq!(
            SPRITE.asset_paths("texture=assets/a.png;size=8x8"),
            vec!["assets/a.png".to_owned()]
        );
        assert_eq!(
            MESH.asset_paths("mesh=assets/hero.glb"),
            vec!["assets/hero.glb".to_owned()]
        );
        assert!(QUAD.asset_paths("size=8,8;color=1,1,1,1").is_empty());
        assert!(MESH.asset_paths("not a payload").is_empty());
        assert!(LIGHT.asset_paths("type=directional").is_empty());
    }
}
