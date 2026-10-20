//! UI 组件的**载荷契约**（`docs/62` P1 第 4 条）。
//!
//! 场景文本里 `UiLayout` / `Container` / `Text` / `Image` / `UiEvents` / `ButtonStates`
//! 的文法属于本 crate（字号、颜色、九宫格、事件绑定语法都只有这一份实现）。
//! `rustlua-scene` 通过 `rustlua_component_codec` 注册表拿到这里的校验函数，
//! 不再直接依赖 `rustlua-ui` 的类型；编辑器/渲染器拿原文回来自己解析。
//!
//! **装表**：`engine` / `editor` 启动时把 `scene_codecs()` 与
//! `rustlua_render::scene_codecs()` 一起装进 `rustlua_component_codec::install(...)`。

use rustlua_component_codec::ComponentCodec;

use crate::ui::UiImage;
use crate::ui::UiText;
use crate::ui_events::UiEvents;
use crate::ui_layout::{ContainerStyle, UiRectLayout};
use crate::ui_states::ButtonStateSet;
use crate::world_text::WorldText;

/// `UiLayout`：屏幕空间矩形（锚点/偏移/尺寸/层）。
fn validate_layout(value: &str) -> Result<(), String> {
    UiRectLayout::from_component_str(value).map(|_| ())
}

/// `Container`：容器样式（方向/间距/对齐/内边距）。
fn validate_container(value: &str) -> Result<(), String> {
    ContainerStyle::from_component_str(value).map(|_| ())
}

/// `Text`：文字 + 字号 + 颜色 + 可选自定义字体。资源路径 = `font`（非空时）。
fn validate_text(value: &str) -> Result<(), String> {
    UiText::from_component_str(value)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn text_assets(value: &str) -> Vec<String> {
    UiText::from_component_str(value)
        .ok()
        .filter(|text| !text.font.trim().is_empty())
        .map(|text| vec![text.font])
        .unwrap_or_default()
}

/// `Image`：贴图 + 着色 + 可选九宫格。资源路径 = `texture`。
fn validate_image(value: &str) -> Result<(), String> {
    UiImage::from_component_str(value)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn image_assets(value: &str) -> Vec<String> {
    UiImage::from_component_str(value)
        .ok()
        .map(|image| vec![image.texture])
        .unwrap_or_default()
}

/// `UiEvents`：`event=..;function=..|..` 形式的事件绑定串。
fn validate_events(value: &str) -> Result<(), String> {
    UiEvents::parse(value)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// `ButtonStates`：按钮各态的样式覆盖。
fn validate_button_states(value: &str) -> Result<(), String> {
    ButtonStateSet::from_component_str(value).map(|_| ())
}

/// `WorldText`：世界空间文字（文字字段 + 像素偏移 + 居中）。
///
/// 它从 `rustlua-scene` 搬来（P1 ④）：类型内嵌 [`UiText`]，文法本来就在本 crate。
/// 资源路径 = `font`（非空时），与屏幕 `Text` 同一口径。
fn validate_world_text(value: &str) -> Result<(), String> {
    WorldText::from_component_str(value)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn world_text_assets(value: &str) -> Vec<String> {
    WorldText::from_component_str(value)
        .ok()
        .filter(|world_text| !world_text.text.font.trim().is_empty())
        .map(|world_text| vec![world_text.text.font])
        .unwrap_or_default()
}

static LAYOUT: ComponentCodec = ComponentCodec::new("UiLayout", validate_layout);
static CONTAINER: ComponentCodec = ComponentCodec::new("Container", validate_container);
static TEXT: ComponentCodec = ComponentCodec::new("Text", validate_text).with_assets(text_assets);
static IMAGE: ComponentCodec =
    ComponentCodec::new("Image", validate_image).with_assets(image_assets);
static EVENTS: ComponentCodec = ComponentCodec::new("UiEvents", validate_events);
static BUTTON_STATES: ComponentCodec = ComponentCodec::new("ButtonStates", validate_button_states);
static WORLD_TEXT: ComponentCodec =
    ComponentCodec::new("WorldText", validate_world_text).with_assets(world_text_assets);

/// 本 crate 负责的七个组件 codec。组合根把它们与 `rustlua_render::scene_codecs()` 一起装表。
#[must_use]
pub fn scene_codecs() -> &'static [&'static ComponentCodec] {
    static CODECS: [&ComponentCodec; 7] = [
        &LAYOUT,
        &CONTAINER,
        &TEXT,
        &IMAGE,
        &EVENTS,
        &BUTTON_STATES,
        &WORLD_TEXT,
    ];
    &CODECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codecs_cover_every_ui_component_name() {
        let names: Vec<&str> = scene_codecs().iter().map(|codec| codec.name()).collect();
        assert_eq!(
            names,
            vec![
                "UiLayout",
                "Container",
                "Text",
                "Image",
                "UiEvents",
                "ButtonStates",
                "WorldText"
            ]
        );
    }

    /// 校验与归属 crate 的解析器同源：真实载荷过、坏载荷拒。
    #[test]
    fn validation_matches_the_owner_crate_parsers() {
        assert!(
            LAYOUT
                .validate("anchors=0,0,1,1;offsets=10,20,100,40")
                .is_ok()
        );
        assert!(TEXT.validate("text=Hello;font_size=24").is_ok());
        assert!(TEXT.validate("font_size=not-a-number").is_err());
        assert!(
            IMAGE
                .validate("texture=assets/ui/panel.png;color=1,1,1,1")
                .is_ok()
        );
        assert!(EVENTS.validate("event=click;function=on_click").is_ok());
        assert!(EVENTS.validate("event=bogus;function=f").is_err());
        assert!(
            BUTTON_STATES
                .validate("normal=assets/ui/btn.png;hover=assets/ui/btn_hover.png")
                .is_ok()
        );
        assert!(BUTTON_STATES.validate("bogus=a.png").is_err());
        assert!(
            WORLD_TEXT
                .validate("text=HP;font_size=16;offset=0,-24")
                .is_ok()
        );
        assert!(
            WORLD_TEXT
                .validate("text=HP;font_size=16;offset=bad")
                .is_err()
        );
    }

    /// 字体与贴图要能被场景层收集成资源引用；空字体不算引用（用项目默认字体）。
    #[test]
    fn asset_paths_cover_font_and_texture() {
        assert_eq!(
            TEXT.asset_paths("text=Hi;font_size=24;font=assets/fonts/My.ttf"),
            vec!["assets/fonts/My.ttf".to_owned()]
        );
        assert!(TEXT.asset_paths("text=Hi;font_size=24").is_empty());
        assert_eq!(
            IMAGE.asset_paths("texture=assets/ui/panel.png"),
            vec!["assets/ui/panel.png".to_owned()]
        );
        assert!(CONTAINER.asset_paths("kind=hbox;spacing=8").is_empty());
        assert_eq!(
            WORLD_TEXT.asset_paths("text=HP;font_size=16;font=assets/fonts/My.ttf"),
            vec!["assets/fonts/My.ttf".to_owned()]
        );
        assert!(WORLD_TEXT.asset_paths("text=HP;font_size=16").is_empty());
    }
}
