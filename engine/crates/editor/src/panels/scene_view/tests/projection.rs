//! 投影与缩放：设计分辨率框、贴合缩放的滑动条范围、缩放时的中心保持、
//! 3D 缓存尺寸，以及手写平移缩放映射的等价性。

use super::*;

/// R-03：3D 缓存 key 记录实际渲染尺寸。拖拽期渲染半分辨率并写入缓存；
/// 拖拽结束后视口恢复全分辨率，缓存不匹配 → 必然重渲染，不会沿用模糊帧。
#[test]
fn scene_view_3d_cache_records_actual_render_size() {
    let camera = EditorCamera::default();
    let revision = Some(7u64);
    let view_w = 1280u32;
    let view_h = 720u32;
    let half = ((view_w / 2).max(1), (view_h / 2).max(1));
    // 拖拽中写入的半分辨率缓存。
    let cache_half = SceneViewCache3d {
        size: half,
        revision,
        camera,
    };
    // 拖拽中（preview_active=true）：即使尺寸匹配也需每帧重渲染 → 不命中。
    assert!(!scene_view_3d_cache_hit(
        cache_half, half.0, half.1, revision, camera, true,
    ));
    // 拖拽结束：preview_active=false，但视口已恢复全分辨率，与半分辨率缓存尺寸不匹配 → 重渲染。
    assert!(!scene_view_3d_cache_hit(
        cache_half, view_w, view_h, revision, camera, false,
    ));
    // 空闲且全分辨率缓存与当前视口/修订/相机匹配 → 命中，可跳过重渲染。
    let cache_full = SceneViewCache3d {
        size: (view_w, view_h),
        revision,
        camera,
    };
    assert!(scene_view_3d_cache_hit(
        cache_full, view_w, view_h, revision, camera, false,
    ));
    // 相机或修订变化 → 不命中。
    let mut moved = EditorCamera::default();
    moved.dist += 1.0;
    assert!(!scene_view_3d_cache_hit(
        cache_full, view_w, view_h, revision, moved, false,
    ));
    assert!(!scene_view_3d_cache_hit(
        cache_full,
        view_w,
        view_h,
        Some(8u64),
        camera,
        false,
    ));
}

/// E18：Safe Area 由 `CanvasConfig.safe_area` 驱动（不再用固定 5%）：
/// insets 按设计分辨率钳制后走设计框同一映射；全 0 不画；极端 insets 退化为零尺寸。
#[test]
fn safe_area_frame_uses_canvas_insets_and_clamps() {
    let panel = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));
    let config = rustlua_ui::ui_layout::CanvasConfig {
        safe_area: [40.0, 20.0, 40.0, 20.0],
        ..Default::default()
    };
    let frame = design_safe_frame(
        panel,
        egui::vec2(0.0, 0.0),
        1.0,
        1.0,
        egui::Vec2::ZERO,
        (1280, 720),
        &config,
    )
    .expect("insets 非零时应有安全区框");
    // 设计框 1280×720 以面板中心 (400,300) 为中心 → 左上 (-240,-60)；
    // insets (40,20) 后左上 (-200,-40)，尺寸 1200×680。
    assert_eq!(frame.min, egui::pos2(-200.0, -40.0));
    assert_eq!(frame.size(), egui::vec2(1200.0, 680.0));

    let zero = rustlua_ui::ui_layout::CanvasConfig::default();
    assert!(
        design_safe_frame(
            panel,
            egui::vec2(0.0, 0.0),
            1.0,
            1.0,
            egui::Vec2::ZERO,
            (1280, 720),
            &zero
        )
        .is_none(),
        "insets 全 0 时安全区就是整块画布，不重复画框"
    );

    let huge = rustlua_ui::ui_layout::CanvasConfig {
        safe_area: [2000.0, 2000.0, 2000.0, 2000.0],
        ..Default::default()
    };
    let clamped = design_safe_frame(
        panel,
        egui::vec2(0.0, 0.0),
        1.0,
        1.0,
        egui::Vec2::ZERO,
        (1280, 720),
        &huge,
    )
    .unwrap();
    assert_eq!(clamped.size(), egui::vec2(0.0, 0.0));
}

/// E18：裁剪读数只统计设计框被面板裁掉的设计像素；框完整可见时四边为 0。
#[test]
fn design_frame_crop_reports_only_clipped_design_pixels() {
    let panel = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));
    let frame = design_resolution_frame(
        panel,
        egui::vec2(0.0, 0.0),
        1.0,
        1.0,
        egui::Vec2::ZERO,
        (1280, 720),
    );
    assert_eq!(
        design_frame_crop(panel, frame, 1.0, 1.0),
        [240.0, 60.0, 240.0, 60.0]
    );
    let larger_panel =
        egui::Rect::from_min_max(egui::pos2(-400.0, -100.0), egui::pos2(1200.0, 800.0));
    assert_eq!(design_frame_crop(larger_panel, frame, 1.0, 1.0), [0.0; 4]);
}

/// 设计分辨率包围框映射：以 `center`（游戏相机中心）为世界中心，尺寸按
/// zoom/ppp 缩放、按 pan 平移，与 `entity_2d_screen_box` 共用同一世界→面板变换。
#[test]
fn design_resolution_frame_maps_resolution_to_panel() {
    let panel = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));
    // center 0、pan 0、zoom 1、ppp 1：包围框居中于面板，尺寸即设计分辨率。
    let frame = design_resolution_frame(
        panel,
        egui::vec2(0.0, 0.0),
        1.0,
        1.0,
        egui::Vec2::ZERO,
        (1280, 720),
    );
    assert_eq!(frame.center(), egui::pos2(400.0, 300.0));
    assert_eq!(frame.size(), egui::vec2(1280.0, 720.0));
    // zoom 0.5 → 尺寸等比减半。
    let half = design_resolution_frame(
        panel,
        egui::vec2(0.0, 0.0),
        0.5,
        1.0,
        egui::Vec2::ZERO,
        (1280, 720),
    );
    assert_eq!(half.size(), egui::vec2(640.0, 360.0));
    // pan 平移：X 加、Y 减（屏幕 Y 翻转），中心随之移动。
    let panned = design_resolution_frame(
        panel,
        egui::vec2(100.0, -50.0),
        1.0,
        1.0,
        egui::Vec2::ZERO,
        (1280, 720),
    );
    assert_eq!(panned.center(), egui::pos2(500.0, 350.0));
    // 游戏相机中心（640,360）：包围框中心移到世界 (640,360) 映射的面板位置。
    let cam = design_resolution_frame(
        panel,
        egui::vec2(0.0, 0.0),
        1.0,
        1.0,
        egui::vec2(640.0, 360.0),
        (1280, 720),
    );
    assert_eq!(cam.center(), egui::pos2(400.0 + 640.0, 300.0 - 360.0));
}

/// 回归：窄面板 fit 出的 zoom 低于旧 Slider 最小值（0.5）时，Slider 每帧
/// 会把 zoom clamp 回 0.5，而 pan 仍按更低 zoom 计算，导致包围框偏移。
/// 现在 fit / 滚轮 / Slider 共用 SCENE_ZOOM_MIN/MAX，fit 结果必须始终
/// 落在 Slider 范围内，且包围框中心与面板中心重合。
#[test]
fn fit_zoom_stays_within_slider_range_and_centers_frame() {
    // 模拟窄面板（约 520×442 逻辑点，对应 1280×720 设计分辨率的常见布局）。
    let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(520.0, 442.0));
    let margin = 48.0;
    let avail_w = (rect.width() - margin * 2.0).max(50.0);
    let avail_h = (rect.height() - margin * 2.0).max(50.0);
    let zoom = (avail_w / 1280.0_f32)
        .min(avail_h / 720.0_f32)
        .clamp(SCENE_ZOOM_MIN, SCENE_ZOOM_MAX);
    assert!(
        (SCENE_ZOOM_MIN..=SCENE_ZOOM_MAX).contains(&zoom),
        "fit zoom {zoom} 超出 Slider 范围 [{SCENE_ZOOM_MIN}, {SCENE_ZOOM_MAX}]"
    );
    // fit 的 pan 公式：pan = -center * zoom。
    let center = egui::vec2(640.0, 360.0);
    let pan = egui::vec2(-center.x * zoom, -center.y * zoom);
    let frame = design_resolution_frame(rect, pan, zoom, 1.0, center, (1280, 720));
    let frame_center = frame.center();
    let rect_center = rect.center();
    assert!(
        (frame_center.x - rect_center.x).abs() < 0.01,
        "包围框 X 中心 {} 偏离面板 X 中心 {}",
        frame_center.x,
        rect_center.x
    );
    assert!(
        (frame_center.y - rect_center.y).abs() < 0.01,
        "包围框 Y 中心 {} 偏离面板 Y 中心 {}",
        frame_center.y,
        rect_center.y
    );
}

/// 回归：滚轮/Slider 缩放必须以视口中心为锚点。fit 后放大 2 倍，
/// 设计分辨率包围框中心必须仍落在面板中心（旧实现只改 zoom 不调 pan，
/// 锚点落在世界原点 = 包围框左下角，导致框飞出视口）。
#[test]
fn zoom_change_keeps_design_frame_centered() {
    let root = copy_sample_project("zoom-anchor");
    let project = open_project(&root).unwrap();
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
    // fit 状态：pan = -center * zoom，包围框中心在面板中心。
    app.fit_scene_view_to_design(rect);
    let center = egui::vec2(app.game_camera_2d_center.x, app.game_camera_2d_center.y);
    let frame_of = |app: &EditorApp| {
        design_resolution_frame(
            rect,
            app.scene_state.pan,
            app.scene_zoom,
            1.0,
            center,
            app.preview_resolution,
        )
    };
    assert!((frame_of(&app).center() - rect.center()).length() < 0.01);
    // 以中心为锚点放大 2 倍：包围框中心必须不动。
    let old_zoom = app.scene_zoom;
    app.set_scene_zoom_centered(old_zoom, old_zoom * 2.0);
    let frame_center = frame_of(&app).center();
    assert!(
        (frame_center - rect.center()).length() < 0.01,
        "zoom 后包围框中心 {frame_center:?} 偏离面板中心 {:?}",
        rect.center()
    );
}

/// WP5 编辑器拾取统一（旧新对拍）：收敛后的统一映射必须与收敛前**各处手写**的
/// `pan + world*zoom`（再除 `ppp`）公式逐点一致——否则选中轮廓、命中测试、框选、
/// 瓦片笔刷与设计分辨率包围框会与渲染内容错位。
///
/// 同时钉住三件事：面板 → 世界与 世界 → 面板互为逆、世界 → 面板与旧公式一致、
/// 设计分辨率包围框的中心就是游戏相机中心的世界坐标映射。
#[test]
fn scene_view_projection_matches_the_hand_written_pan_zoom_mapping() {
    let rect = egui::Rect::from_min_size(egui::pos2(40.0, 20.0), egui::vec2(800.0, 500.0));
    let pointers = [
        egui::pos2(40.0, 20.0),
        egui::pos2(440.0, 270.0),
        egui::pos2(700.0, 480.0),
        egui::pos2(123.5, 456.25),
    ];
    for ppp in [1.0_f32, 1.25, 2.0] {
        for zoom in [0.5_f32, 1.0, 2.5, 8.0] {
            for pan in [egui::vec2(0.0, 0.0), egui::vec2(120.0, -75.0)] {
                for pointer in pointers {
                    // 旧公式（收敛前 `screen_to_world_2d` 的原文）。
                    let old = Vec2::new(
                        ((pointer.x - rect.center().x) * ppp - pan.x) / zoom,
                        -((pointer.y - rect.center().y) * ppp + pan.y) / zoom,
                    );
                    let new = screen_to_world_2d(rect, pan, zoom, ppp, pointer);
                    assert!(
                        (old.x - new.x).abs() < 1e-3 && (old.y - new.y).abs() < 1e-3,
                        "面板→世界 与旧公式不一致 ppp={ppp} zoom={zoom} pan={pan:?} \
                             pointer={pointer:?}: 旧 {old:?} 新 {new:?}"
                    );

                    // 旧公式的逆（收敛前 `entity_2d_screen_box` 的中心映射）。
                    let old_panel = egui::pos2(
                        rect.center().x + (pan.x + old.x * zoom) / ppp,
                        rect.center().y + (-pan.y - old.y * zoom) / ppp,
                    );
                    let new_panel = scene_view_world_to_panel(rect, pan, zoom, ppp, new);
                    assert!(
                        (old_panel.x - new_panel.x).abs() < 1e-3
                            && (old_panel.y - new_panel.y).abs() < 1e-3,
                        "世界→面板 与旧公式不一致: 旧 {old_panel:?} 新 {new_panel:?}"
                    );
                    // 逆映射必须回到原面板点。
                    assert!(
                        (new_panel.x - pointer.x).abs() < 1e-3
                            && (new_panel.y - pointer.y).abs() < 1e-3,
                        "世界→面板 应是 面板→世界 的逆：{new_panel:?} vs {pointer:?}"
                    );
                }
            }
        }
    }

    // 设计分辨率包围框的中心 = 游戏相机中心的世界坐标映射（与运行时相机同一投影）。
    let center = egui::vec2(640.0, 360.0);
    let frame =
        design_resolution_frame(rect, egui::vec2(30.0, -12.0), 1.5, 2.0, center, (1280, 720));
    let expected = scene_view_world_to_panel(
        rect,
        egui::vec2(30.0, -12.0),
        1.5,
        2.0,
        Vec2::new(center.x, center.y),
    );
    assert!((frame.center().x - expected.x).abs() < 1e-3);
    assert!((frame.center().y - expected.y).abs() < 1e-3);
    assert!((frame.width() - 1280.0 * 1.5 / 2.0).abs() < 1e-3);
    assert!((frame.height() - 720.0 * 1.5 / 2.0).abs() < 1e-3);
}
