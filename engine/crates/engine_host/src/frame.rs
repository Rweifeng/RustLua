//! Frame-level data structures shared between the scene layer (which produces
//! them each frame) and the renderer (which consumes them).

use bytemuck::{Pod, Zeroable};
use engine_core::{
    camera3d::Camera3D,
    light::Light,
    math3d::{Aabb3, Mat4},
};

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

/// One draw range over the scene vertex buffer, bound to a texture.
/// `texture == 0` selects the built-in white texture; `texture == i > 0`
/// selects `paths[i - 1]`. `sdf == true` draws through the Linear SDF pipeline.
pub struct SceneDraw {
    pub texture: u32,
    pub start: u32,
    pub count: u32,
    pub sdf: bool,
}

/// Sentinel paths resolved by the renderer to built-in textures instead of
/// files: the white 1x1 (bars/particles) and the built-in 5x7 glyph atlas.
pub const WHITE_TEXTURE_PATH: &str = "__white__";
pub const ATLAS_TEXTURE_PATH: &str = "__builtin_text_atlas__";
pub const FONT_ATLAS_TEXTURE_PATH: &str = "__font_atlas__";

/// Scene geometry plus the texture paths its draw ranges reference.
pub struct SceneMesh {
    pub vertices: Vec<OverlayVertex>,
    pub draws: Vec<SceneDraw>,
    pub paths: Vec<String>,
    /// Current CJK font atlas pixels; `pixels` empty means unchanged since last upload.
    pub font_atlas: Option<FontAtlasPixels>,
}

/// RGBA8 pixels of the CJK glyph atlas, with a monotonic revision for uploads.
pub struct FontAtlasPixels {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub version: u64,
}

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
    pub entity: engine_core::scene::SceneEntityId,
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
