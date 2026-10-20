//! 视图状态：缩放上下限、适配画面、状态行诊断、3D 缓存键与逐帧日志。
//! 状态的真相在 `EditorApp::scene_view` 里，这里只做读写编排。

use super::*;

impl EditorApp {
    /// 诊断提示（`pub(crate)`：拖拽/框选的提交层在 `crate::scene_drag` 里也要报诊断）。
    pub(crate) fn set_scene_view_diagnostic(&mut self, message: impl Into<String>) {
        let message = message.into();
        if self.preview_diagnostic.as_deref() == Some(message.as_str()) {
            return;
        }
        self.push_console_notice(format!("[scene view] {message}"));
        self.preview_diagnostic = Some(message);
    }

    pub(crate) fn clear_scene_view_diagnostic(&mut self) {
        self.preview_diagnostic = None;
    }

    /// 把 2D 场景视图适配到设计分辨率包围框：计算使包围框完整显示在面板内并
    /// 居中的 zoom 与 pan（世界相机中心落在面板中心）。
    pub(crate) fn fit_scene_view_to_design(&mut self, rect: egui::Rect) {
        let (w, h) = self.preview_resolution;
        if w == 0 || h == 0 {
            return;
        }
        let margin = 48.0;
        let avail_w = (rect.width() - margin * 2.0).max(50.0);
        let avail_h = (rect.height() - margin * 2.0).max(50.0);
        let zoom = (avail_w / w as f32)
            .min(avail_h / h as f32)
            .clamp(SCENE_ZOOM_MIN, SCENE_ZOOM_MAX);
        let center = egui::vec2(self.game_camera_2d_center.x, self.game_camera_2d_center.y);
        self.scene_zoom = zoom;
        self.scene_state.pan = egui::vec2(-center.x * zoom, -center.y * zoom);
    }

    /// 以视口中心为锚点调整 Scene 缩放：pan 按新旧 zoom 比例缩放，使当前
    /// 位于面板中心的世界点（fit 后即设计分辨率包围框中心）保持不动。
    /// 滚轮缩放与工具栏 Slider 必须共用此入口，否则锚点不一致。
    pub(crate) fn set_scene_zoom_centered(&mut self, old_zoom: f32, new_zoom: f32) {
        let new_zoom = new_zoom.clamp(SCENE_ZOOM_MIN, SCENE_ZOOM_MAX);
        if old_zoom > 0.0 && (new_zoom - old_zoom).abs() > f32::EPSILON {
            self.scene_state.pan *= new_zoom / old_zoom;
        }
        self.scene_zoom = new_zoom;
    }
}
/// Scene 视图 zoom 范围：fit / 滚轮 / Slider 必须共用同一范围，否则 egui
/// Slider 会在每帧把 fit 结果 clamp 到自己的范围，导致包围框偏移。
pub(crate) const SCENE_ZOOM_MIN: f32 = 0.2;
pub(crate) const SCENE_ZOOM_MAX: f32 = 4.0;

/// 判断 3D Scene 视图是否可直接命中渲染缓存（M11-03 / R-03）：
/// 无活动拖拽预览，且实际渲染尺寸 / 场景修订 / 编辑器相机都未变化时才命中。
/// `render_w/render_h` 必须是实际渲染尺寸（交互期为半分辨率）——缓存 key 记录
/// 真实渲染尺寸，拖拽结束半分辨率 → 全分辨率必然失效，不会沿用模糊帧。
pub(crate) fn scene_view_3d_cache_hit(
    cache: SceneViewCache3d,
    render_w: u32,
    render_h: u32,
    revision: Option<u64>,
    camera: crate::EditorCamera,
    preview_active: bool,
) -> bool {
    !preview_active
        && cache.size == (render_w, render_h)
        && cache.revision == revision
        && cache.camera == camera
}

/// 单帧耗时超过阈值才记录，便于对照「回读 → 原生纹理」两条路径的开销（两条路径共用）。
pub(crate) fn log_scene_view_frame(started: std::time::Instant, size: (u32, u32), view: &str) {
    let elapsed = started.elapsed();
    if elapsed >= std::time::Duration::from_millis(8) {
        tracing::info!(
            target: "editor",
            "[editor-perf] scene view {view} frame: {}ms ({}x{})",
            elapsed.as_millis(),
            size.0,
            size.1
        );
    }
}
