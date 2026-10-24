//! 原 `renderer/mod.rs` 的 `mod tests`（WP12 阶段 4a 纯搬运）。帧统计账本（2D/3D 绘制数、几何量、矢量文本不算 sprite 实例）。

use super::*;

#[test]
fn render_metrics_count_2d_draws_and_geometry() {
    let mut metrics = RenderFrameMetrics::default();
    record_vertex_draw(&mut metrics, 60);
    record_vertex_draw(&mut metrics, 30);

    assert_eq!(metrics.draw_calls, 2);
    assert_eq!(metrics.instances, 15);
    assert_eq!(metrics.vertices, 90);
    assert_eq!(metrics.triangles, 30);
    assert_eq!(metrics.gpu_frame_us, None);
}

#[test]
fn render_metrics_do_not_treat_vector_text_as_sprite_instances() {
    let mut metrics = RenderFrameMetrics::default();
    record_triangle_draw(&mut metrics, 12);

    assert_eq!(metrics.draw_calls, 1);
    assert_eq!(metrics.instances, 0);
    assert_eq!(metrics.vertices, 12);
    assert_eq!(metrics.triangles, 4);
}

#[test]
fn render_metrics_count_3d_passes_once_per_instance() {
    let mut metrics = RenderFrameMetrics::default();
    record_indexed_draw(&mut metrics, 36, 12, 4, 2);

    assert_eq!(metrics.draw_calls, 2);
    assert_eq!(metrics.instances, 4);
    assert_eq!(metrics.vertices, 48);
    assert_eq!(metrics.triangles, 48);
}

#[test]
fn render_metrics_count_mixed_2d_and_3d_frame() {
    let mut metrics = RenderFrameMetrics::default();
    record_indexed_draw(&mut metrics, 12, 9, 3, 2);
    record_vertex_draw(&mut metrics, 18);

    assert_eq!(metrics.draw_calls, 3);
    assert_eq!(metrics.instances, 6);
    assert_eq!(metrics.vertices, 45);
    assert_eq!(metrics.triangles, 18);
}

/// SetPass 是**管线状态设置**次数：只由显式调用点记账，draw call 不会自动带出来。
#[test]
fn render_metrics_count_set_pass_calls_explicitly() {
    let mut metrics = RenderFrameMetrics::default();
    record_set_pass(&mut metrics);
    record_set_pass(&mut metrics);
    record_vertex_draw(&mut metrics, 6);
    assert_eq!(metrics.set_pass_calls, 2);
    assert_eq!(metrics.draw_calls, 1);
    assert_eq!(metrics.vertices, 6);
}
