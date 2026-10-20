//! 瓦片坐标映射：单元格矩形、可见格往返、越界与旋转地图的拒绝。

use super::*;

/// WP5 编辑器拾取统一：瓦片格子矩形也必须与收敛前手写的那份公式逐边一致
/// （笔刷落点与渲染的格子必须重合）。
#[test]
fn tilemap_cell_rect_matches_the_hand_written_mapping() {
    let rect = egui::Rect::from_min_size(egui::pos2(10.0, 5.0), egui::vec2(400.0, 300.0));
    let pan = egui::vec2(12.0, -9.0);
    let zoom = 2.0;
    let ppp = 1.25;
    let transform = TransformFields {
        position_x: 16.0,
        position_y: 8.0,
        rotation: 0.0,
        scale_x: 1.5,
        scale_y: 0.75,
    };
    let map = Tilemap::new(Vec2::new(32.0, 32.0), 4, 3).unwrap();
    let (x, y) = (2usize, 1usize);
    let cell_w = map.tile_size.x * transform.scale_x.abs();
    let cell_h = map.tile_size.y * transform.scale_y.abs();
    // 旧公式（收敛前 `tilemap_cell_screen_rect` 的原文）。
    let cell_w_px = cell_w * zoom / ppp;
    let cell_h_px = cell_h * zoom / ppp;
    let min_x = rect.center().x + (pan.x + (transform.position_x + x as f32 * cell_w) * zoom) / ppp;
    let max_y =
        rect.center().y + (-pan.y - (transform.position_y + y as f32 * cell_h) * zoom) / ppp;
    let expected = egui::Rect::from_min_max(
        egui::pos2(min_x, max_y - cell_h_px),
        egui::pos2(min_x + cell_w_px, max_y),
    );
    let actual = tilemap_cell_screen_rect(rect, pan, zoom, ppp, transform, &map, x, y);
    for (name, old, new) in [
        ("min.x", expected.min.x, actual.min.x),
        ("min.y", expected.min.y, actual.min.y),
        ("max.x", expected.max.x, actual.max.x),
        ("max.y", expected.max.y, actual.max.y),
    ] {
        assert!(
            (old - new).abs() < 1e-3,
            "格子矩形 {name} 与旧公式不一致：旧 {old} 新 {new}"
        );
    }

    // 行序约定：运行时把 `tiles` 的第 0 行画在地图**最下方**（world y 向上，见
    // `append_tilemap_layer` 与 2D golden 的行序签名）。编辑器面板 y 向下，因此
    // 第 0 行的格子必须出现在面板里**更大的 y** 上——笔刷格与渲染图必须同一朝向。
    let row_zero = tilemap_cell_screen_rect(rect, pan, zoom, ppp, transform, &map, 0, 0);
    let row_one = tilemap_cell_screen_rect(rect, pan, zoom, ppp, transform, &map, 0, 1);
    assert!(
        row_zero.center().y > row_one.center().y,
        "第 0 行应画在最下方：row0.y={} row1.y={}",
        row_zero.center().y,
        row_one.center().y
    );
    // 同一格：世界中心经统一映射落在格矩形中心（笔刷格与视图投影同源）。
    let world_center = Vec2::new(
        transform.position_x + (x as f32 + 0.5) * cell_w,
        transform.position_y + (y as f32 + 0.5) * cell_h,
    );
    let mapped = scene_view_world_to_panel(rect, pan, zoom, ppp, world_center);
    assert!((mapped.x - actual.center().x).abs() < 1e-3);
    assert!((mapped.y - actual.center().y).abs() < 1e-3);
}

#[test]
fn tilemap_pointer_mapping_round_trips_visible_cells() {
    let rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 300.0));
    let pan = egui::vec2(10.0, -6.0);
    let zoom = 2.0;
    let transform = TransformFields {
        position_x: 16.0,
        position_y: 8.0,
        rotation: 0.0,
        scale_x: 1.0,
        scale_y: 1.0,
    };
    let map = Tilemap::new(Vec2::new(32.0, 32.0), 4, 3).unwrap();
    let cell_rect = tilemap_cell_screen_rect(rect, pan, zoom, 1.0, transform, &map, 2, 1);
    let pointer = cell_rect.center();
    assert_eq!(
        tilemap_cell_from_pointer(rect, pan, zoom, 1.0, pointer, transform, &map),
        Some((2, 1))
    );
}

#[test]
fn tilemap_pointer_mapping_rejects_outside_and_rotated_maps() {
    let rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 300.0));
    let map = Tilemap::new(Vec2::new(32.0, 32.0), 2, 2).unwrap();
    let transform = TransformFields::default();
    assert_eq!(
        tilemap_cell_from_pointer(
            rect,
            egui::Vec2::ZERO,
            1.0,
            1.0,
            egui::pos2(-20.0, -20.0),
            transform,
            &map,
        ),
        None
    );
    let rotated = TransformFields {
        rotation: 15.0,
        ..transform
    };
    assert_eq!(
        tilemap_cell_from_pointer(
            rect,
            egui::Vec2::ZERO,
            1.0,
            1.0,
            egui::pos2(200.0, 150.0),
            rotated,
            &map,
        ),
        None
    );
}
