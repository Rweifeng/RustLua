//! Unified coarse-grained resource families shared by the editor, assetc, and engine.
//!
//! This is the single source of truth for how file extensions map to resource
//! families. Importers register themselves per [`ResourceKind`]; the editor and
//! assetc both report these families to users and persist them via
//! [`ResourceDatabase`](crate::core::resource_db::ResourceDatabase).

use std::path::Path;

/// Coarse-grained resource family. Kept deliberately small: format-specific
/// handling lives in the importer, not in the kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ResourceKind {
    /// Rasterized images (PNG/JPG/JPEG).
    Texture,
    /// Audio samples (WAV/OGG).
    Audio,
    /// Lua runtime scripts.
    LuaScript,
    /// Scenes and scene-like serialized data (.scene/.ron/.json).
    Scene,
    /// Prefab assets.
    Prefab,
    /// Texture atlas definitions.
    Atlas,
    /// 3D models (glTF/GLB).
    Model,
    /// Vector fonts (TTF/OTF).
    Font,
    /// 场景内声明的动画片段：没有文件路径，身份来自内容签名
    /// （`AnimationClip::resource_key`，docs/54 §4.4）。
    Animation,
    /// Anything the pipeline does not yet understand.
    Other,
}

impl ResourceKind {
    /// Maps a path to its resource family; unknown extensions become [`Self::Other`]
    /// and are never an error, so the editor can always render a row for them.
    pub fn from_path(path: impl AsRef<Path>) -> Self {
        match path
            .as_ref()
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "png" | "jpg" | "jpeg" => Self::Texture,
            "wav" | "ogg" => Self::Audio,
            "lua" => Self::LuaScript,
            "scene" | "ron" | "json" => Self::Scene,
            "prefab" => Self::Prefab,
            "atlas" => Self::Atlas,
            "glb" | "gltf" => Self::Model,
            "ttf" | "otf" => Self::Font,
            _ => Self::Other,
        }
    }

    /// User-facing family name (Chinese, matching the editor's existing UI).
    pub fn label(self) -> &'static str {
        match self {
            Self::Texture => "纹理",
            Self::Audio => "音频",
            Self::LuaScript => "Lua",
            Self::Scene => "场景",
            Self::Prefab => "预制体",
            Self::Atlas => "图集",
            Self::Model => "模型",
            Self::Font => "字体",
            Self::Animation => "动画",
            Self::Other => "其他",
        }
    }

    /// Stable persistence token used by the resource database and import cache.
    pub fn token(self) -> &'static str {
        match self {
            Self::Texture => "texture",
            Self::Audio => "audio",
            Self::LuaScript => "script",
            Self::Scene => "scene",
            Self::Prefab => "prefab",
            Self::Atlas => "atlas",
            Self::Model => "model",
            Self::Font => "font",
            Self::Animation => "animation",
            Self::Other => "other",
        }
    }

    /// Inverse of [`Self::token`]; returns `None` for unknown tokens.
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "texture" => Self::Texture,
            "audio" => Self::Audio,
            "script" => Self::LuaScript,
            "scene" => Self::Scene,
            "prefab" => Self::Prefab,
            "atlas" => Self::Atlas,
            "model" => Self::Model,
            "font" => Self::Font,
            "animation" => Self::Animation,
            "other" => Self::Other,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_full_extension_table() {
        assert_eq!(ResourceKind::from_path("a.png"), ResourceKind::Texture);
        assert_eq!(ResourceKind::from_path("a.jpg"), ResourceKind::Texture);
        assert_eq!(ResourceKind::from_path("a.jpeg"), ResourceKind::Texture);
        assert_eq!(ResourceKind::from_path("a.wav"), ResourceKind::Audio);
        assert_eq!(ResourceKind::from_path("a.ogg"), ResourceKind::Audio);
        assert_eq!(ResourceKind::from_path("a.lua"), ResourceKind::LuaScript);
        assert_eq!(ResourceKind::from_path("a.scene"), ResourceKind::Scene);
        assert_eq!(ResourceKind::from_path("a.ron"), ResourceKind::Scene);
        assert_eq!(ResourceKind::from_path("a.json"), ResourceKind::Scene);
        assert_eq!(ResourceKind::from_path("a.prefab"), ResourceKind::Prefab);
        assert_eq!(ResourceKind::from_path("a.atlas"), ResourceKind::Atlas);
        assert_eq!(ResourceKind::from_path("a.glb"), ResourceKind::Model);
        assert_eq!(ResourceKind::from_path("a.gltf"), ResourceKind::Model);
        assert_eq!(ResourceKind::from_path("a.ttf"), ResourceKind::Font);
        assert_eq!(ResourceKind::from_path("a.otf"), ResourceKind::Font);
    }

    #[test]
    fn unknown_and_unsupported_extensions_fall_to_other() {
        assert_eq!(ResourceKind::from_path("a.txt"), ResourceKind::Other);
        assert_eq!(ResourceKind::from_path("a.webp"), ResourceKind::Other);
        assert_eq!(ResourceKind::from_path("a.mp3"), ResourceKind::Other);
        assert_eq!(ResourceKind::from_path("no_extension"), ResourceKind::Other);
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        assert_eq!(
            ResourceKind::from_path("assets/UI/Button.PNG"),
            ResourceKind::Texture
        );
        assert_eq!(
            ResourceKind::from_path("assets/sound/Attack.OGG"),
            ResourceKind::Audio
        );
    }

    #[test]
    fn token_round_trips() {
        for kind in [
            ResourceKind::Texture,
            ResourceKind::Audio,
            ResourceKind::LuaScript,
            ResourceKind::Scene,
            ResourceKind::Prefab,
            ResourceKind::Atlas,
            ResourceKind::Model,
            ResourceKind::Font,
            ResourceKind::Other,
        ] {
            assert_eq!(ResourceKind::parse(kind.token()), Some(kind));
        }
        assert_eq!(ResourceKind::parse("unknown"), None);
    }

    #[test]
    fn labels_are_non_empty() {
        for kind in [
            ResourceKind::Texture,
            ResourceKind::Audio,
            ResourceKind::LuaScript,
            ResourceKind::Scene,
            ResourceKind::Prefab,
            ResourceKind::Atlas,
            ResourceKind::Model,
            ResourceKind::Font,
            ResourceKind::Other,
        ] {
            assert!(!kind.label().is_empty());
        }
    }
}
