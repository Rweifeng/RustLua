//! 原 `renderer/mod.rs` 的 `mod tests`（WP12 阶段 4a 纯搬运）。渲染通道计划与实际执行逐通道对齐、后处理通道对像素的作用。

use super::*;

/// `docs/60` 复核（2026-10-06）：**画不出来的 draw 必须留痕**。
///
/// 通道级早有对账（`RenderPlan::mismatch` → 告警 + `last_frame_passes` 可断言），**draw 级此前是
/// 静默 `continue`**：症状是"屏幕上少一块、日志里一个字都没有"，而黄金门比的是布局矩形，比不出像素少一块。
///
/// 造法是**真的**走那条分支：`sdf = true` 但贴图指向普通资源路径 → `sdf_bind_for` 既不是字体图集、
/// 也不是 `font://` 路径，于是返回 `None`（**它不像 `bind_group_for` 那样回退白贴图**，所以这里丢得掉）。
/// `count = 0` 就够：bind 查询发生在 `pass.draw` **之前**，因此不必造合法顶点。
#[test]
fn dropped_draws_are_counted_and_stay_out_of_the_draw_call_count() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        return; // no GPU available; skip
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-test-drop-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-test-drop"),
        size: wgpu::Extent3d {
            width: 32,
            height: 32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mesh = crate::host::frame::SceneMesh {
        vertices: Vec::new(),
        draws: vec![crate::host::frame::SceneDraw {
            texture: 1,
            start: 0,
            count: 0,
            sdf: true,
            bitmap: false,
            vector: false,
            render_layer: rustlua_render::layer::RenderLayer::ScreenUI,
        }],
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths: vec!["assets/textures/red-1x1.png".to_owned()],
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    };
    let outcome = renderer.render_into_texture(&mesh, None, &view, 32, 32);
    assert!(
        matches!(outcome, RenderOutcome::Presented),
        "丢一个 draw 不该让整帧失败"
    );
    let metrics = renderer.last_frame_metrics();
    assert_eq!(
        metrics.dropped_draws, 1,
        "拿不到 bind group 的 SDF draw 必须被计数（此前是静默 continue）"
    );
    assert_eq!(
        metrics.draw_calls, 0,
        "丢弃不计入 draw_calls——两者分账才看得出'计划里有、实际没画'"
    );
}

/// docs/54 §4.3：**计划就是实际执行的那一份**。    ///
/// 单测（`crate::core::render_plan`）只证明计划函数本身的顺序与取舍；这条测试在真实（离屏）渲染
/// 帧上证明渲染器跑的就是计划里列的通道，一条不多一条不少。此前"这一帧跑了哪些通道"只能
/// 靠读渲染循环的 `if` 猜——现在可以直接断言，而且漏跑会立刻在这里失败。
#[test]
fn render_plan_matches_the_executed_passes_on_a_real_frame() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        return; // no GPU available; skip
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-test-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
        .expect("offscreen renderer");
    let size = 64u32;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-test-plan"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let empty_mesh = || crate::host::frame::SceneMesh {
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
    // 空帧 + 未声明视图 ⇒ 只有回滚档的清屏（历史默认色）。
    renderer.render_into_texture(&empty_mesh(), None, &view, size, size);
    assert_eq!(
        renderer.last_frame_plan().passes(),
        [RenderPass::Clear],
        "空帧 + 未声明视图只应清屏"
    );
    assert_eq!(
        renderer.last_frame_passes(),
        renderer.last_frame_plan().passes(),
        "空帧：执行必须与计划逐条一致"
    );

    // 声明 `ClearPolicy::None` ⇒ 计划里连清屏都不该有（清屏也是一条被计划管着的通道）。
    let declared = RenderView::main_2d(
        Camera2D::new(Vec2::new(size as f32 / 2.0, size as f32 / 2.0), 1.0).expect("camera2d"),
        Viewport::new(size as f32, size as f32).expect("viewport"),
        [0.0, 0.0, 0.0, 1.0],
    );
    renderer.set_frame_views(FrameViews {
        main: Some(declared.with_clear(ClearPolicy::None)),
        overlay: Some(declared),
    });
    let quad = crate::host::frame::SceneMesh {
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
        draws: vec![
            crate::host::frame::SceneDraw {
                texture: 0,
                start: 0,
                count: 6,
                sdf: false,
                bitmap: false,
                vector: false,
                render_layer: RenderLayer::World2D,
            },
            crate::host::frame::SceneDraw::screen_ui(0, 0, 6),
        ],
        ..empty_mesh()
    };
    renderer.render_into_texture(&quad, None, &view, size, size);
    assert_eq!(
        renderer.last_frame_plan().passes(),
        [RenderPass::Legacy2DOverlay, RenderPass::ScreenUI],
        "世界层与屏幕 UI 是两条独立通道，且不清屏时不该有 Clear"
    );
    assert_eq!(
        renderer.last_frame_passes(),
        renderer.last_frame_plan().passes(),
        "2D 帧：执行必须与计划逐条一致"
    );
    assert_eq!(
        renderer.last_frame_metrics().draw_calls,
        2,
        "两条通道各应产出一个 draw"
    );
}

/// docs/54 §4.3：后处理通道写像素的**方向性**证据。
///
/// 恒等参数不该把画面弄花（离屏 sRGB 往返最多差几个 LSB），而曝光、色调与暗角三个参数各自
/// 必须产生**可判定方向**的变化——否则"通道跑了"与"通道真的生效"是两件事。
#[test]
fn post_process_pass_changes_pixels_in_the_documented_way() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        return; // no GPU available; skip
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-test-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
        .expect("offscreen renderer");
    let size = 32u32;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-test-post-process"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mesh = crate::host::frame::SceneMesh {
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
    let render_and_read = |renderer: &mut Renderer| -> Vec<u8> {
        renderer.render_into_texture(&mesh, None, &view, size, size);
        renderer
            .copy_texture_to_cpu(&texture, size, size)
            .expect("post-process readback")
    };
    let pixel = |pixels: &[u8], x: u32, y: u32| -> [u8; 4] {
        let offset = ((y * size + x) * 4) as usize;
        [
            pixels[offset],
            pixels[offset + 1],
            pixels[offset + 2],
            pixels[offset + 3],
        ]
    };

    // 回滚档：不开后处理时，计划里没有 PostProcess，整条离屏路径不存在。
    let baseline = render_and_read(&mut renderer);
    assert!(
        !renderer.last_frame_plan().contains(RenderPass::PostProcess),
        "没开后处理时计划里不该有 PostProcess"
    );
    assert_eq!(
        renderer.last_frame_passes(),
        renderer.last_frame_plan().passes()
    );

    // 恒等参数：画面必须基本不变（离屏 sRGB 往返允许多个 LSB 的量化差）。
    renderer.set_post_process(Some(PostProcess::default()));
    let identity = render_and_read(&mut renderer);
    assert!(
        renderer.last_frame_plan().contains(RenderPass::PostProcess),
        "开了后处理，计划必须多出 PostProcess"
    );
    assert_eq!(
        renderer.last_frame_passes(),
        renderer.last_frame_plan().passes()
    );
    assert_eq!(
        renderer.last_frame_passes().last(),
        Some(&RenderPass::PostProcess),
        "后处理永远是最后一条通道"
    );
    for (index, (before, after)) in baseline.iter().zip(&identity).enumerate() {
        assert!(
            before.abs_diff(*after) <= 2,
            "恒等参数不该改变像素（第 {index} 字节：{before} → {after}）"
        );
    }

    // 曝光 0.5：整体压暗。
    renderer.set_post_process(Some(
        PostProcess::new(0.5, [1.0; 4], 0.0).expect("exposure"),
    ));
    let darker = render_and_read(&mut renderer);
    let centre_before = pixel(&identity, size / 2, size / 2);
    let centre_darker = pixel(&darker, size / 2, size / 2);
    assert!(
        centre_darker[0] < centre_before[0] && centre_darker[2] < centre_before[2],
        "曝光 0.5 必须压暗：{centre_before:?} → {centre_darker:?}"
    );

    // 色调（只留红）：绿/蓝分量必须被压到接近 0。
    renderer.set_post_process(Some(
        PostProcess::new(1.0, [1.0, 0.0, 0.0, 1.0], 0.0).expect("tint"),
    ));
    let tinted = render_and_read(&mut renderer);
    let centre_tinted = pixel(&tinted, size / 2, size / 2);
    assert!(
        centre_tinted[1] <= 1 && centre_tinted[2] <= 1,
        "tint=[1,0,0] 必须把绿蓝压掉：{centre_tinted:?}"
    );
    assert!(centre_tinted[0] > 0, "红色分量必须保留：{centre_tinted:?}");

    // 暗角 1.0：四角全黑、中心不受影响。
    renderer.set_post_process(Some(
        PostProcess::new(1.0, [1.0; 4], 1.0).expect("vignette"),
    ));
    let vignetted = render_and_read(&mut renderer);
    let corner = pixel(&vignetted, 0, 0);
    let centre = pixel(&vignetted, size / 2, size / 2);
    assert_eq!(corner, [0, 0, 0, 255], "暗角 1.0 时角落必须是黑的");
    assert!(
        centre[0] > corner[0],
        "暗角只压边缘：centre={centre:?} corner={corner:?}"
    );
}
