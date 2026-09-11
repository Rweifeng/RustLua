//! Frame-level data structures shared between the scene layer (which produces
//! them each frame) and the renderer (which consumes them).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::core::{
    camera3d::Camera3D,
    font::FontAtlas,
    light::Light,
    math3d::{Aabb3, Mat4},
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
