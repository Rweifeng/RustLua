//! 编辑器 2D 场景视图几何工具：Transform 序列化、旋转命中测试、框选相交检测。

use editor::TransformFields;
use eframe::egui;

/// 把 2D Transform 字段序列化为**唯一形态**的组件字符串。
///
/// `Transform` 只有一个表示（`position/rotation/scale`，3D 形态），2D 只是
/// "z=0、绕 Z 旋转、scale_z=1"的特例；数字格式交给引擎的编码器，编辑器不自己拼字符串。
pub(crate) fn encode_transform(transform: TransformFields) -> String {
    let local = engine::core::spatial_transform::LocalTransform::from_2d(
        engine::core::transform::Transform {
            position: engine::core::transform::Vec2::new(
                transform.position_x,
                transform.position_y,
            ),
            rotation: transform.rotation,
            scale: engine::core::transform::Vec2::new(transform.scale_x, transform.scale_y),
        },
    )
    .expect("2D 字段一定是有限数（编辑器内部数据）");
    local.to_unified_component_str()
}

/// 解析 `Transform` 组件串为 2D 字段：**唯一形态**（统一 3D 写法）经 `LocalTransform::to_2d`
/// 投影出平面 TRS；畸形串返回 `None`（旧 2D 写法已随统一删除）。
pub(crate) fn parse_transform(value: &str) -> Option<TransformFields> {
    let transform =
        engine::core::spatial_transform::LocalTransform::transform_2d_from_component_str(value)?;
    Some(TransformFields {
        position_x: transform.position.x,
        position_y: transform.position.y,
        rotation: transform.rotation,
        scale_x: transform.scale.x,
        scale_y: transform.scale.y,
    })
}

pub(crate) fn rotated_corners(
    center: egui::Pos2,
    size: egui::Vec2,
    rotation_degrees: f32,
) -> [egui::Pos2; 4] {
    let angle = rotation_degrees.to_radians();
    let corners = [
        egui::vec2(-size.x * 0.5, -size.y * 0.5),
        egui::vec2(size.x * 0.5, -size.y * 0.5),
        egui::vec2(size.x * 0.5, size.y * 0.5),
        egui::vec2(-size.x * 0.5, size.y * 0.5),
    ];
    corners.map(|p| {
        center
            + egui::vec2(
                p.x * angle.cos() - p.y * angle.sin(),
                p.x * angle.sin() + p.y * angle.cos(),
            )
    })
}

pub(crate) fn rotated_contains(
    center: egui::Pos2,
    size: egui::Vec2,
    rotation_degrees: f32,
    point: egui::Pos2,
) -> bool {
    let angle = -rotation_degrees.to_radians();
    let delta = point - center;
    let local = egui::vec2(
        delta.x * angle.cos() - delta.y * angle.sin(),
        delta.x * angle.sin() + delta.y * angle.cos(),
    );
    local.x.abs() <= size.x * 0.5 && local.y.abs() <= size.y * 0.5
}

fn point_in_polygon(point: egui::Pos2, polygon: &[egui::Pos2]) -> bool {
    let mut inside = false;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        let crosses = (a.y > point.y) != (b.y > point.y)
            && point.x < (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x;
        if crosses {
            inside = !inside;
        }
    }
    inside
}

fn orientation(a: egui::Pos2, b: egui::Pos2, c: egui::Pos2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn segments_intersect(a: egui::Pos2, b: egui::Pos2, c: egui::Pos2, d: egui::Pos2) -> bool {
    let eps = 1e-5;
    let o1 = orientation(a, b, c);
    let o2 = orientation(a, b, d);
    let o3 = orientation(c, d, a);
    let o4 = orientation(c, d, b);
    let on = |p: egui::Pos2, q: egui::Pos2, r: egui::Pos2| {
        orientation(p, q, r).abs() <= eps
            && r.x >= p.x.min(q.x) - eps
            && r.x <= p.x.max(q.x) + eps
            && r.y >= p.y.min(q.y) - eps
            && r.y <= p.y.max(q.y) + eps
    };
    (o1.signum() != o2.signum() && o3.signum() != o4.signum())
        || on(a, b, c)
        || on(a, b, d)
        || on(c, d, a)
        || on(c, d, b)
}

pub(crate) fn polygon_intersects_rect(polygon: &[egui::Pos2], rect: egui::Rect) -> bool {
    let rect_corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    polygon.iter().any(|p| rect.contains(*p))
        || rect_corners.iter().any(|p| point_in_polygon(*p, polygon))
        || (0..polygon.len()).any(|i| {
            let a = polygon[i];
            let b = polygon[(i + 1) % polygon.len()];
            (0..4).any(|j| segments_intersect(a, b, rect_corners[j], rect_corners[(j + 1) % 4]))
        })
}

#[cfg(test)]
mod geometry_tests {
    use super::*;

    #[test]
    fn rotated_hit_test_respects_rotation() {
        let center = egui::pos2(0.0, 0.0);
        let size = egui::vec2(100.0, 20.0);
        assert!(rotated_contains(center, size, 45.0, egui::pos2(0.0, 0.0)));
        assert!(!rotated_contains(center, size, 45.0, egui::pos2(45.0, 0.0)));
    }

    #[test]
    fn box_intersection_detects_edge_crossing_without_corners_inside() {
        let polygon = rotated_corners(egui::pos2(0.0, 0.0), egui::vec2(100.0, 20.0), 45.0);
        let selection = egui::Rect::from_min_max(egui::pos2(-10.0, -10.0), egui::pos2(10.0, 10.0));
        assert!(polygon_intersects_rect(&polygon, selection));
    }

    #[test]
    fn left_to_right_containment_differs_from_intersection() {
        let polygon = rotated_corners(egui::pos2(40.0, 0.0), egui::vec2(40.0, 20.0), 0.0);
        let selection = egui::Rect::from_min_max(egui::pos2(0.0, -20.0), egui::pos2(30.0, 20.0));
        assert!(!polygon.iter().all(|p| selection.contains(*p)));
        assert!(polygon_intersects_rect(&polygon, selection));
    }
}
