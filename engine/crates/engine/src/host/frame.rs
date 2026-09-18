//! Frame-level data structures shared between the scene layer (which produces
//! them each frame) and the renderer (which consumes them).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::core::{
    camera::Camera3D,
    font::FontAtlas,
    layer::RenderLayer,
    light::Light,
    math3d::{Aabb3, Mat4},
    sprite_batch::SpriteDrawBatch,
};
use bytemuck::{Pod, Zeroable};

/// Vertex format for the 2D textured overlay pass.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct OverlayVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    /// `1` = signed-distance-field glyph (text); shader applies smoothstep.
    pub sdf: u32,
}

/// WP6 单趟化：让 2D 批处理直接把顶点写成宿主布局，省掉中间那份 `SpriteVertex`。
/// 核心只提供 NDC 位置/UV/颜色，`sdf` 是宿主自己的字段，因此由这里补 0。
impl crate::core::sprite_batch::OverlayVertexSink for OverlayVertex {
    fn placeholder() -> Self {
        Self {
            position: [0.0; 2],
            uv: [0.0; 2],
            color: [0.0; 4],
            sdf: 0,
        }
    }

    fn from_ndc(position: [f32; 2], uv: [f32; 2], color: [f32; 4]) -> Self {
        Self {
            position,
            uv,
            color,
            sdf: 0,
        }
    }
}

/// WP6 世界 Sprite 顶点：位置是**世界坐标** `(x, y, z)`，投影由顶点着色器用
/// `Camera2D::view_projection` 完成。`z` 来自 `Sprite::z`，深度通道据此做深度测试。
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct WorldSpriteVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

impl crate::core::sprite_batch::WorldSpriteVertexSink for WorldSpriteVertex {
    fn placeholder() -> Self {
        Self {
            position: [0.0; 3],
            uv: [0.0; 2],
            color: [0.0; 4],
        }
    }

    fn from_world(position: [f32; 3], uv: [f32; 2], color: [f32; 4]) -> Self {
        Self {
            position,
            uv,
            color,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct VectorTextVertex {
    pub position: [f32; 2],
    pub local: [f32; 2],
    pub glyph_id: u32,
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct VectorGlyphSnapshot {
    pub segment_start: u32,
    pub segment_count: u32,
    pub _padding: [u32; 2],
    pub bounds: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct VectorSegmentSnapshot {
    pub start: [f32; 2],
    pub end: [f32; 2],
    pub bounds: [f32; 4],
}

/// One draw range over the scene vertex buffer, bound to a texture.
/// `texture == 0` selects the built-in white texture; `texture == i > 0`
/// selects `paths[i - 1]`. `sdf == true` draws through the Linear SDF pipeline.
pub struct SceneDraw {
    pub texture: u32,
    pub start: u32,
    pub count: u32,
    pub sdf: bool,
    pub vector: bool,
    /// WP6：这段几何属于哪个渲染层。世界内容与屏幕内容因此可以被区分开——
    /// 世界 Sprite 深度通道只吃世界层，屏幕 UI 永远留在屏幕空间。
    pub render_layer: RenderLayer,
}

impl SceneDraw {
    /// 世界内容（精灵 / 瓦片 / 世界文字）。
    #[must_use]
    pub const fn world(texture: u32, start: u32, count: u32) -> Self {
        Self {
            texture,
            start,
            count,
            sdf: false,
            vector: false,
            render_layer: RenderLayer::World2D,
        }
    }

    /// 屏幕空间 UI 文本/图片（按设计分辨率布局，不受世界相机影响）。
    #[must_use]
    pub const fn screen_ui(texture: u32, start: u32, count: u32) -> Self {
        Self {
            render_layer: RenderLayer::ScreenUI,
            ..Self::world(texture, start, count)
        }
    }

    /// 世界之上的叠加内容（HUD、调试绘制、性能浮层）。
    #[must_use]
    pub const fn overlay(texture: u32, start: u32, count: u32) -> Self {
        Self {
            render_layer: RenderLayer::Overlay2D,
            ..Self::world(texture, start, count)
        }
    }

    /// 标记 SDF 文本（字体距离场走 Linear 采样管线）。
    #[must_use]
    pub const fn with_sdf(mut self, sdf: bool) -> Self {
        self.sdf = sdf;
        self
    }
}

/// Sentinel paths resolved by the renderer to built-in textures instead of
/// files: the white 1x1 (bars/particles) and the built-in 5x7 glyph atlas.
pub const WHITE_TEXTURE_PATH: &str = "__white__";
pub const ATLAS_TEXTURE_PATH: &str = "__builtin_text_atlas__";
pub const FONT_ATLAS_TEXTURE_PATH: &str = "__font_atlas__";

/// Namespace for per-Text custom font texture ids so they can never collide
/// with ordinary png/jpg texture paths in `SceneMesh.paths`.
pub const FONT_TEXTURE_PREFIX: &str = "font://";

/// The namespaced texture path identifying a custom font atlas in `paths`
/// (e.g. `font://assets/fonts/x.ttf`).
#[must_use]
pub fn font_texture_path(asset: &str) -> String {
    format!("{FONT_TEXTURE_PREFIX}{asset}")
}

/// Is this a namespaced custom-font texture path?
#[must_use]
pub fn is_font_texture_path(path: &str) -> bool {
    path.starts_with(FONT_TEXTURE_PREFIX)
}

/// Strips the namespace, returning the font asset path.
#[must_use]
pub fn font_asset_from_texture_path(path: &str) -> &str {
    path.strip_prefix(FONT_TEXTURE_PREFIX).unwrap_or(path)
}

/// Scene geometry plus the texture paths its draw ranges reference.
pub struct SceneMesh {
    pub vertices: Vec<OverlayVertex>,
    pub draws: Vec<SceneDraw>,
    /// WP6 世界 Sprite（世界坐标 + z）：开关打开时精灵走这条路径，用顶点着色器投影并参与
    /// 深度测试；关闭时为空，精灵仍走上面的 NDC overlay 路径。
    pub world_sprites: Vec<WorldSpriteVertex>,
    /// `world_sprites` 的绘制分段（纹理下标与既有 `SpriteDrawBatch` 同一语义）。
    pub world_sprite_draws: Vec<SpriteDrawBatch>,
    pub paths: Vec<String>,
    /// Current CJK font atlas pixels; `pixels` empty means unchanged since last upload.
    pub font_atlas: Option<FontAtlasPixels>,
    /// Per-Text custom font atlases touched this frame. `pixels` empty means
    /// unchanged since the last upload to this renderer.
    pub custom_font_atlases: Vec<CustomFontAtlasPixels>,
    pub vector_vertices: Vec<VectorTextVertex>,
    pub vector_glyphs: Vec<VectorGlyphSnapshot>,
    pub vector_segments: Vec<VectorSegmentSnapshot>,
}

/// RGBA8 pixels of the CJK glyph atlas, with a monotonic revision for uploads.
pub struct FontAtlasPixels {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub version: u64,
}

/// Pixels of one custom (per-Text) font atlas. `key` is the namespaced id used
/// both in `SceneMesh.paths` for SDF runs and by the renderer to key its upload.
pub struct CustomFontAtlasPixels {
    pub key: String,
    pub pixels: FontAtlasPixels,
}

/// A handle to one font atlas, shared across `RuntimeScene` rebuilds so MSDF
/// glyphs are rasterized once (editor Scene View / Play reuse the same atlas).
pub type FontHandle = Arc<Mutex<FontAtlas>>;

/// Editor-injected shared cache of per-Text custom font atlases, keyed by the
/// namespaced font id (`font://...`). Standalone runtimes use their own fresh map.
pub type FontAtlasMap = Arc<Mutex<HashMap<String, FontHandle>>>;

/// A handle to one vector-outline font atlas, shared across `RuntimeScene`
/// rebuilds so outlines are parsed/extracted once.
pub type VectorFontHandle = Arc<Mutex<crate::core::font::FontVectorAtlas>>;

/// Editor-injected shared cache of per-Text vector outline atlases, keyed by
/// the namespaced font id (`font://...`).
pub type VectorAtlasMap = Arc<Mutex<HashMap<String, VectorFontHandle>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundsSource {
    Mesh,
    Fallback,
}

/// One draw entity's local-space bounds used by renderer and editor picking.
pub struct Scene3dBounds {
    pub local_aabb: Aabb3,
    pub source: BoundsSource,
}

/// marks a skinned drawable whose joint matrices are uploaded per frame.
pub struct Scene3dDrawable {
    pub entity: crate::core::scene::SceneEntityId,
    pub name: String,
    pub mesh_path: String,
    pub model: Mat4,
    pub palette: Vec<Mat4>,
    pub bounds: Scene3dBounds,
}

/// Per-frame 3D render state gathered from the runtime world: the active
/// camera (aspect already applied), the active directional light, and the
/// drawables (entities with `Transform3D` + `MeshRenderer`).
pub struct Scene3d {
    pub camera: Camera3D,
    pub light: Light,
    pub drawables: Vec<Scene3dDrawable>,
}
