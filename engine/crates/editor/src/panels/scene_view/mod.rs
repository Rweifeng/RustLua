//! Scene 视图（2D/3D）：渲染、选择、Gizmo、框选、拖放生成实体。
//!
//! 第 41 轮从单文件（6,369 行）拆成目录：主绘制 `draw`、指针状态机 `pointer`、
//! 2D 命中与包围盒 `scene_geometry`、视图状态 `view_state`、3D `view_3d`、
//! 工具面板 `tool_panel`、瓦片 `tilemap`、渲染管线 `render`、投影换算 `projection`。
//! 拆分是纯搬运：只切行 + 补模块头，跨子模块用到的条目从"文件内私有"抬到 `pub(crate)`。

use crate::EditorApp;
use crate::geometry::{
    parse_transform, polygon_intersects_rect, rotated_contains, rotated_corners,
};
use crate::panels::tilemap_tools::{
    CellRect, TileFill, clamp_brush, copy_region, fill_rect, paste_region,
};
use crate::play_sync::{game_screen_texture, panel_scene_document};
use crate::preview_session::PreviewSession;
use crate::scene_interaction::{InteractionEvent, ToolPhase};
use crate::scene_resize;
use crate::util::icon_toggle;
use crate::util::strip_verbatim_prefix;
use crate::{
    GizmoAxis, SceneDrag, SceneTool, SceneView3dTypedEntity, SceneViewCache2d, SceneViewCache3d,
    SceneViewMode, TileEditMode, UiDragInitial,
};
use editor::{SelectionSource, TransformFields};
use eframe::egui;
use engine::host::{
    BoundsSource, RuntimeScene, Scene3d, Scene3dBounds, Scene3dDrawable, SceneMesh,
    apply_scene_environment, load_project_font_atlas, load_project_vector_atlas,
};
use rustlua_camera::camera::Camera2D;
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, FrameViews, Ray3, RenderView, Viewport};
use rustlua_ecs::tilemap::Tilemap;
use rustlua_ecs::transform::Transform3D;
use rustlua_ecs::transform::Vec2;
use rustlua_gpu::{ReadbackStatus, Renderer};
use rustlua_math::math3d::{Quat, Vec3};
use rustlua_physics::physics::ColliderDesc;
use rustlua_render::light::Light;
use rustlua_render::sprite::SpriteComponent;
use rustlua_scene::scene::{SceneEntity, SceneEntityId};
use rustlua_scene::typed_scene::{ComponentId, TypedComponent, TypedScene};
use rustlua_ui::ui_layout::{UiRect, UiRectLayout};
use rustlua_ui::ui_scene_layout::{UiSceneNode, resolve_scene_ui_layout};

/// Scene 视图（2D/3D）显示用的纹理来源。
///
/// 有窗口（egui-wgpu 渲染状态）时用 [`SceneViewTexture::Native`]：离屏纹理直接交给
/// egui 采样，省掉每帧「整帧 GPU→CPU 回读 + CPU→GPU 重传」，画面也不再滞后一帧。
/// 无窗口（单元测试等没有原生纹理可注册的环境）只能沿用回读像素，用
/// [`SceneViewTexture::Uploaded`] 承接。
pub(crate) enum SceneViewTexture {
    Native(egui::TextureId),
    Uploaded(egui::TextureHandle),
}

impl SceneViewTexture {
    /// 对上层绘制统一接口：`painter.image` 只关心纹理 ID。
    pub(crate) fn id(&self) -> egui::TextureId {
        match self {
            Self::Native(id) => *id,
            Self::Uploaded(handle) => handle.id(),
        }
    }
}

/// 注册/刷新单个原生纹理槽位（`scene_texture` 重建时上游已先释放旧 ID）。
pub(crate) fn sync_native_scene_texture(
    renderer: &mut eframe::egui_wgpu::Renderer,
    slot: &mut Option<SceneViewTexture>,
    device: &wgpu::Device,
    view: &wgpu::TextureView,
    filter: wgpu::FilterMode,
) {
    match slot {
        Some(SceneViewTexture::Native(id)) => {
            renderer.update_egui_texture_from_wgpu_texture(device, view, filter, *id);
        }
        _ => {
            *slot = Some(SceneViewTexture::Native(
                renderer.register_native_texture(device, view, filter),
            ));
        }
    }
}

mod draw;
mod pointer;
mod projection;
mod render;
mod scene_geometry;
#[cfg(test)]
mod tests;
mod tilemap;
mod tool_panel;
mod view_3d;
mod view_state;

// 面板之外的调用点（play_sync / runtime_debug 等）继续走这两个名字。
pub(crate) use view_state::{SCENE_ZOOM_MAX, SCENE_ZOOM_MIN};

// 子模块的纯函数在本面板内共享（`tests` 也经 `use super::*` 取用）。
pub(crate) use projection::*;
pub(crate) use scene_geometry::*;
pub(crate) use tilemap::*;
pub(crate) use view_state::*;
