//! 原 `renderer/mod.rs` 的 `mod tests`（WP12 阶段 4a 纯搬运）。深度策略、视口裁剪与声明式帧视图（clear 颜色 / overlay 视口）。

use super::*;

/// docs/54 §10 第 ⑩ 行登记过的那条符号缺陷：阴影相机必须放在**光源那一侧**（`-direction`），
/// 与着色器 `l = normalize(-light.direction)` 同源。
///
/// 判据用"深度"而不是相机位置本身：阴影相机在光源一侧时，**离光源更近**的点（也就是更高、
/// 更靠上的表面）在光空间里深度更小。光传播方向朝下时，位于地面上方的点必须比地面更接近光源。
#[test]
fn shadow_camera_sits_on_the_light_side_of_the_documented_direction() {
    // 天空里的太阳：光传播方向朝下（y < 0）。
    let sun = Light {
        direction: Vec3::new(0.4, -0.8, 0.5),
        color: [1.0, 1.0, 1.0],
        intensity: 1.0,
        ..Light::default()
    };
    let matrix = light_view_projection(sun);
    let depth = |point: Vec3| -> f32 {
        let clip = matrix
            .transform_point(point)
            .expect("light-space projection of a finite point");
        clip.z
    };
    let ground = depth(Vec3::ZERO);
    let above = depth(Vec3::new(0.0, 1.0, 0.0));
    assert!(
        above < ground,
        "光朝下时，地面上方的点必须更接近阴影相机：above={above} ground={ground}"
    );
    // 反例（曾经的写法）：把相机放到 `+direction` 一侧会让上面的关系反过来——把光方向
    // 反号就应当得到相反的结论，这条同时钉住"符号真的起作用"。
    let flipped = Light {
        direction: -sun.direction,
        ..sun
    };
    let flipped_matrix = light_view_projection(flipped);
    let flipped_clip = flipped_matrix
        .transform_point(Vec3::new(0.0, 1.0, 0.0))
        .expect("clip");
    let flipped_ground = flipped_matrix.transform_point(Vec3::ZERO).expect("clip");
    assert!(
        flipped_clip.z > flipped_ground.z,
        "光方向取反后，上方的点必须变得**更远**：{} vs {}",
        flipped_clip.z,
        flipped_ground.z
    );
}

/// 方向光阴影必须跟着相机焦点走：焦点在 40,-20 时仍落在阴影图中心，且覆盖半径
/// 对应半宽 30 m；旧的固定原点 ±10 m 会把它裁掉。
#[test]
fn focused_shadow_camera_centers_on_the_camera_target() {
    let sun = Light {
        direction: Vec3::new(0.4, -0.8, 0.5),
        color: [1.0, 1.0, 1.0],
        intensity: 1.0,
        ..Light::default()
    };
    let focus = Vec3::new(40.0, 0.0, -20.0);
    let matrix = light_view_projection_focused(sun, focus, 30.0, DEFAULT_SHADOW_RESOLUTION);
    let centre = matrix.transform_point(focus).expect("focus clip");
    assert!(
        centre.x.abs() < 0.01 && centre.y.abs() < 0.01,
        "焦点必须落在阴影图中心：{centre:?}"
    );
    // 半宽 30 m：焦点 + 30 m 应落在右边界附近（方向取阴影相机局部 x 轴较难，
    // 这里只验证“同一偏移 30 m 的深度点仍在裁剪体内”，防止半径退化成旧的 10 m）。
    let edge = Vec3::new(focus.x + 25.0, focus.y, focus.z);
    let edge_clip = matrix.transform_point(edge).expect("edge clip");
    assert!(
        edge_clip.x.abs() <= 1.0 && edge_clip.y.abs() <= 1.0,
        "25 m 偏移必须仍在 30 m 半宽阴影图内：{edge_clip:?}"
    );
}

/// 小于一个纹素的焦点平移不应改变四级阴影矩阵：斜射光下按光源平面 snap 后，
/// 相机平移不会让阴影边缘逐帧抖动。
#[test]
fn shadow_projection_is_stable_under_sub_texel_focus_motion() {
    let sun = Light {
        direction: Vec3::new(0.4, -0.8, 0.5),
        color: [1.0, 1.0, 1.0],
        intensity: 1.0,
        ..Light::default()
    };
    let a = shadow_view_projections(
        sun,
        Vec3::new(40.0, 0.0, -20.0),
        270.0,
        DEFAULT_SHADOW_RESOLUTION,
    );
    let b = shadow_view_projections(
        sun,
        Vec3::new(40.0005, 0.0, -20.0005),
        270.0,
        DEFAULT_SHADOW_RESOLUTION,
    );
    for (index, (left, right)) in a.iter().zip(b.iter()).enumerate() {
        assert_eq!(
            left.columns, right.columns,
            "第 {index} 级在亚纹素平移下必须保持同一矩阵"
        );
    }
}

/// 阴影半宽随相机距离伸缩：近景收紧、远景覆盖更大，且有上下限防止退化。
#[test]
fn shadow_extent_tracks_camera_distance_with_clamps() {
    assert_eq!(shadow_half_extents_for(0.0), [20.0, 45.0, 85.0, 160.0]);
    assert_eq!(shadow_half_extents_for(40.0), [20.0, 45.0, 85.0, 160.0]);
    assert_eq!(shadow_half_extents_for(120.0), [20.0, 45.0, 85.0, 160.0]);
    assert_eq!(shadow_half_extents_for(270.0), [40.0, 85.0, 150.0, 244.0]);
    assert_eq!(shadow_half_extents_for(1000.0), [40.0, 85.0, 150.0, 260.0]);
}

/// P3-R17：半宽按 2 m 一档量化，相机距离轻微变化时级联尺寸不逐帧变；缩放量化不
/// 改变上下限，也不让覆盖范围掉到 clamp 之外。
#[test]
fn shadow_extents_quantize_scale_steps() {
    let a = shadow_half_extents_for(270.0);
    let b = shadow_half_extents_for(270.4);
    assert_eq!(a, b, "0.4 m 距离变化应落在同一档：{a:?} vs {b:?}");
    for (distance, extents) in [
        (150.0, shadow_half_extents_for(150.0)),
        (210.0, shadow_half_extents_for(210.0)),
        (300.0, shadow_half_extents_for(300.0)),
    ] {
        for (i, value) in extents.into_iter().enumerate() {
            let endpoint = matches!(value, 40.0 | 85.0 | 150.0 | 260.0);
            assert!(
                endpoint || value % 2.0 == 0.0,
                "distance={distance} cascade={i} 半宽 {value} 未量化到 2 m"
            );
        }
    }
    assert_eq!(shadow_half_extents_for(1000.0), [40.0, 85.0, 150.0, 260.0]);
}

/// 四级阴影的近/中/远必须覆盖近级之外：120 m 偏移在近/中级裁剪体外、远级内。
#[test]
fn far_shadow_cascade_covers_beyond_the_near_cascade() {
    let sun = Light {
        direction: Vec3::new(0.4, -0.8, 0.5),
        color: [1.0, 1.0, 1.0],
        intensity: 1.0,
        ..Light::default()
    };
    let focus = Vec3::new(40.0, 0.0, -20.0);
    let [near, mid, far, _ultra] =
        shadow_view_projections(sun, focus, 270.0, DEFAULT_SHADOW_RESOLUTION);
    let point = Vec3::new(focus.x + 120.0, focus.y, focus.z);
    let near_clip = near.transform_point(point).expect("near clip");
    let mid_clip = mid.transform_point(point).expect("mid clip");
    let far_clip = far.transform_point(point).expect("far clip");
    assert!(
        near_clip.x.abs() > 1.0 || near_clip.y.abs() > 1.0,
        "120 m 偏移应落在近级 40 m 半宽之外：{near_clip:?}"
    );
    assert!(
        mid_clip.x.abs() > 1.0 || mid_clip.y.abs() > 1.0,
        "120 m 偏移应落在中级 85 m 半宽之外：{mid_clip:?}"
    );
    assert!(
        far_clip.x.abs() <= 1.0 && far_clip.y.abs() <= 1.0,
        "120 m 偏移必须落在远级 150 m 半宽之内：{far_clip:?}"
    );
}

/// 第四级只兜远端：200 m 偏移在前三级裁剪体外，但必须在超远级 243 m 半宽内。
#[test]
fn ultra_shadow_cascade_covers_beyond_the_far_cascade() {
    let sun = Light {
        direction: Vec3::new(0.4, -0.8, 0.5),
        color: [1.0, 1.0, 1.0],
        intensity: 1.0,
        ..Light::default()
    };
    let focus = Vec3::new(40.0, 0.0, -20.0);
    let [near, mid, far, ultra] =
        shadow_view_projections(sun, focus, 270.0, DEFAULT_SHADOW_RESOLUTION);
    let point = Vec3::new(focus.x + 200.0, focus.y, focus.z);
    for (name, clip) in [
        ("near", near.transform_point(point).expect("near clip")),
        ("mid", mid.transform_point(point).expect("mid clip")),
        ("far", far.transform_point(point).expect("far clip")),
    ] {
        assert!(
            clip.x.abs() > 1.0 || clip.y.abs() > 1.0,
            "{name} 级 150 m 半宽不应覆盖 200 m 偏移：{clip:?}"
        );
    }
    let ultra_clip = ultra.transform_point(point).expect("ultra clip");
    assert!(
        ultra_clip.x.abs() <= 1.0 && ultra_clip.y.abs() <= 1.0,
        "200 m 偏移必须落在超远级 243 m 半宽之内：{ultra_clip:?}"
    );
}

/// 软阴影开关必须写进 `Light.params.y`：着色器用这一位在单次比较与 3×3 PCF 之间分叉。
#[test]
fn soft_shadow_flag_is_packed_into_light_params_y() {
    let sun = Light::default();
    let hard = light_uniform_bytes(sun, &[], false, false, None);
    let soft = light_uniform_bytes(sun, &[], true, false, None);
    let hard_words: &[f32] = bytemuck::cast_slice(&hard);
    let soft_words: &[f32] = bytemuck::cast_slice(&soft);
    assert_eq!(hard_words[9], 0.0);
    assert_eq!(soft_words[9], 1.0);
}

/// WP6：三条管线的深度策略逐条钉住。
///
/// 回归背景：这三个设置曾是散落的字段字面量，一次脚本化编辑把 **mesh3d 与 shadow** 的
/// `depth_write_enabled` 误改成 `false`——3D 深度静默失效，而当时的 golden 只断言可绘制
/// 对象、实例/三角形与非背景像素，**断言不了深度顺序**，因此全门绿灯。现在策略收在
/// `DepthPass` 里并由本条测试锁死；要改必须先改这里，改错立刻判红。
#[test]
fn depth_policies_are_pinned_per_pass() {
    assert!(
        DepthPass::Mesh3d.write_depth(),
        "3D 网格必须写深度：否则前后遮挡不再生效（这正是被误改过的那一处）"
    );
    assert!(
        DepthPass::Shadow.write_depth(),
        "阴影贴图必须写深度：它本身就是不透明物体到光源的距离场"
    );
    assert!(
        !DepthPass::WorldSprite.write_depth(),
        "半透明世界 Sprite 不写深度：顺序由宿主按 z 排成画家顺序，写深度会让半透明透不出来"
    );
    assert!(
        DepthPass::WorldSpriteOpaque.write_depth(),
        "不透明世界 Sprite 必须写深度：否则它无法与 3D 网格在同一世界视图里互相遮挡"
    );
    for pass in [
        DepthPass::Mesh3d,
        DepthPass::Shadow,
        DepthPass::WorldSprite,
        DepthPass::WorldSpriteOpaque,
    ] {
        assert_eq!(
            depth_stencil_state(pass, wgpu::DepthBiasState::default()).depth_compare,
            wgpu::CompareFunction::LessEqual,
            "{pass:?} 的深度比较应为 LessEqual"
        );
    }
}

/// WP5 + 任务 #20：声明的视口换算成**等比居中矩形**——必须落在目标内、至少 1×1，
/// 且"整目标视口"与"未声明"等价。
///
/// 旧口径是 `(0, 0, min(声明, 目标))`：只在声明盒与附件同比例时才对，否则内容挤在左上角。
/// 上面那条"1 px 容差 ⇒ 贴满"的护栏就是为它设的：浏览器 fit 后的 GL 画布与设计盒只差取整，
/// 若这里返回小一号的矩形，画面就会多出一条底色缝，命令流也不再是"不调用 `set_viewport`"。
#[test]
fn fit_viewport_rect_contains_and_centers_the_declared_box() {
    let full = Viewport::new(64.0, 64.0).unwrap();
    assert_eq!(fit_viewport_rect(full, (64, 64)), (0, 0, 64, 64));
    // 超出目标：等比缩进目标并**居中**（旧行为"夹到目标尺寸"= 左上角一块）。
    assert_eq!(fit_viewport_rect(full, (32, 16)), (8, 0, 16, 16));
    // 细长盒进方目标：长边贴满、短边居中留边（10.667 → 11，取整口径可见）。
    let wide = Viewport::new(60.0, 10.0).unwrap();
    assert_eq!(fit_viewport_rect(wide, (64, 64)), (0, 26, 64, 11));
    // 空/非法尺寸（构造器会拒绝，这里兜住手写的 0）：至少 1×1 且不溢出。
    let zero = Viewport::new(1.0, 1.0).unwrap();
    assert_eq!(fit_viewport_rect(zero, (0, 0)), (0, 0, 1, 1));
}

/// WP5：渲染器消费宿主声明的 [`rustlua_camera::camera::FrameViews`]——清屏色来自声明，且 2D overlay 的视口
/// 真的只画在声明的那块区域内。
///
/// 同时这是**收敛的对拍**：不声明任何视图时读回的第一像素，必须与"声明历史默认
/// 清屏色"的帧逐字节相同（回滚档与接线后行为一致）。
#[test]
fn declared_frame_views_own_clear_color_and_overlay_viewport() {
    const SIZE: u32 = 64;
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        return gpu_skipped("request_adapter");
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-view-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let empty = rustlua_render::frame::SceneMesh {
        vertices: Vec::new(),
        draws: Vec::new(),
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths: Vec::new(),
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    };
    let render_and_read = |renderer: &mut Renderer, target: &wgpu::Texture| {
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        renderer.render_into_texture(&empty, None, &view, SIZE, SIZE, true);
        renderer
            .copy_texture_to_cpu(target, SIZE, SIZE)
            .expect("readback")
    };
    let new_target = |label: &str| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    };
    let pixel = |pixels: &[u8], x: u32, y: u32| -> [u8; 4] {
        let offset = ((y * SIZE + x) * 4) as usize;
        [
            pixels[offset],
            pixels[offset + 1],
            pixels[offset + 2],
            pixels[offset + 3],
        ]
    };
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).unwrap();

    // 1) 不声明任何视图（回滚档）：读回历史默认清屏色。
    let baseline = render_and_read(&mut renderer, &new_target("rustlua-view-baseline"));
    let clear = pixel(&baseline, 0, 0);
    assert!(
        clear.iter().any(|&byte| byte > 0),
        "默认清屏应产生非零像素：{clear:?}"
    );

    // 2) 声明主视图但用**历史默认色**：必须与回滚档逐字节相同。
    let declared_default = FrameViews {
        main: Some(RenderView::degenerate(
            Projection::Perspective {
                fov_y: 1.0,
                near: 0.1,
                far: 100.0,
            },
            viewport,
            ClearPolicy::Color(DEFAULT_3D_CLEAR),
            0,
            u32::MAX,
        )),
        overlay: None,
    };
    renderer.set_frame_views(declared_default);
    let unplugged = render_and_read(&mut renderer, &new_target("rustlua-view-declared-default"));
    assert_eq!(
        unplugged, baseline,
        "声明历史默认色与不声明必须是同一帧（回滚档逐字节等价）"
    );

    // 3) 声明一个不同的清屏色：整帧都必须是该颜色 ⇒ 清屏确实由声明驱动。
    renderer.set_frame_views(FrameViews {
        main: Some(
            declared_default
                .main
                .expect("main view")
                .with_clear(ClearPolicy::Color([1.0, 0.0, 0.0, 1.0])),
        ),
        overlay: None,
    });
    let red_target = new_target("rustlua-view-red");
    let red = render_and_read(&mut renderer, &red_target);
    assert_eq!(
        pixel(&red, 0, 0),
        [255, 0, 0, 255],
        "主视图声明的清屏色必须生效"
    );
    assert_eq!(pixel(&red, SIZE - 1, SIZE - 1), [255, 0, 0, 255]);

    // 4) 声明"不清屏"：**同一张附件**上再画一帧空场景，红色必须原样留着
    //    （说明这一帧真的没有清屏，而不是又清成了别的颜色）。
    renderer.set_frame_views(FrameViews {
        main: Some(
            declared_default
                .main
                .expect("main view")
                .with_clear(ClearPolicy::None),
        ),
        overlay: None,
    });
    let kept = render_and_read(&mut renderer, &red_target);
    assert_eq!(
        pixel(&kept, 0, 0),
        [255, 0, 0, 255],
        "声明 ClearPolicy::None 时不应清屏"
    );

    // 5) 2D overlay 视口：声明半宽盒 ⇒ 渲染器把它**等比居中**摆进目标（32×64 进 64×64
    //    就是正中那条竖带），两侧留底色。任务 #20 之前这里是"左上角一块"（`(0,0,32,64)`），
    //    而同一条换算在浏览器被 fit 层掩盖了——微信接管形态没有 fit 层，缺陷才现形。
    let half = Viewport::new((SIZE / 2) as f32, SIZE as f32).unwrap();
    let quad = rustlua_render::frame::SceneMesh {
        vertices: vec![
            OverlayVertex {
                position: [-1.0, -1.0],
                uv: [0.0, 1.0],
                color: [1.0, 1.0, 1.0, 1.0],
                sdf: 0,
            },
            OverlayVertex {
                position: [1.0, -1.0],
                uv: [1.0, 1.0],
                color: [1.0, 1.0, 1.0, 1.0],
                sdf: 0,
            },
            OverlayVertex {
                position: [-1.0, 1.0],
                uv: [0.0, 0.0],
                color: [1.0, 1.0, 1.0, 1.0],
                sdf: 0,
            },
            OverlayVertex {
                position: [1.0, -1.0],
                uv: [1.0, 1.0],
                color: [1.0, 1.0, 1.0, 1.0],
                sdf: 0,
            },
            OverlayVertex {
                position: [1.0, 1.0],
                uv: [1.0, 0.0],
                color: [1.0, 1.0, 1.0, 1.0],
                sdf: 0,
            },
            OverlayVertex {
                position: [-1.0, 1.0],
                uv: [0.0, 0.0],
                color: [1.0, 1.0, 1.0, 1.0],
                sdf: 0,
            },
        ],
        draws: vec![rustlua_render::frame::SceneDraw {
            texture: 0,
            start: 0,
            count: 6,
            sdf: false,
            bitmap: false,
            vector: false,
            render_layer: rustlua_render::layer::RenderLayer::World2D,
        }],
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths: Vec::new(),
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    };
    renderer.set_frame_views(FrameViews {
        main: Some(
            declared_default
                .main
                .expect("main view")
                .with_clear(ClearPolicy::Color(DEFAULT_3D_CLEAR)),
        ),
        overlay: Some(RenderView::degenerate(
            Projection::Orthographic { zoom: 1.0 },
            half,
            ClearPolicy::None,
            1,
            u32::MAX,
        )),
    });
    let clipped = {
        let target = new_target("rustlua-view-overlay");
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        renderer.render_into_texture(&quad, None, &view, SIZE, SIZE, true);
        renderer
            .copy_texture_to_cpu(&target, SIZE, SIZE)
            .expect("readback")
    };
    assert_eq!(
        pixel(&clipped, SIZE / 2, SIZE / 2),
        [255, 255, 255, 255],
        "等比居中的 overlay 矩形正中应画出 quad"
    );
    assert_eq!(
        pixel(&clipped, 2, SIZE / 2),
        clear,
        "矩形之外的左条必须保持清屏色（视口确实生效）"
    );
    assert_eq!(
        pixel(&clipped, SIZE - 3, SIZE / 2),
        clear,
        "矩形之外的右条同样保持清屏色（居中不是左上贴齐）"
    );
}

/// P3-R198：分辨率档必须真的进入纹素对齐。焦点取光源空间原点，沿光源平面偏移
/// 1/4 个 512 纹素：512 档量化回原点（矩阵不变），2048 档正好跨一个纹素（矩阵必须变）。
#[test]
fn shadow_texel_snap_follows_the_configured_resolution() {
    let sun = Light {
        direction: Vec3::new(0.4, -0.8, 0.5),
        color: [1.0, 1.0, 1.0],
        intensity: 1.0,
        ..Light::default()
    };
    let direction = sun.direction.normalized().expect("sun direction");
    let right = direction
        .cross(Vec3::new(0.0, 1.0, 0.0))
        .normalized()
        .expect("light-plane right axis");
    let half = 30.0;
    let shift = right * (2.0 * half / 512.0 * 0.25);
    let focus = Vec3::ZERO;
    let low_a = light_view_projection_focused(sun, focus, half, 512);
    let low_b = light_view_projection_focused(sun, focus + shift, half, 512);
    let high_a = light_view_projection_focused(sun, focus, half, 2048);
    let high_b = light_view_projection_focused(sun, focus + shift, half, 2048);
    assert_eq!(
        low_a.columns, low_b.columns,
        "512 档：1/4 纹素的焦点抖动必须被量化掉"
    );
    assert_ne!(
        high_a.columns, high_b.columns,
        "2048 档：同样的抖动跨了一个纹素，矩阵必须变化（否则分辨率没进纹素对齐）"
    );
}
