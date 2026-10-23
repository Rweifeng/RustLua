//! 原 `renderer/mod.rs` 的 `mod tests`（WP12 阶段 4a 纯搬运）。世界 Sprite：画家顺序不写深度、与 3D 网格互相遮挡、视图解析回退。

use super::*;
// WP12 阶段 4b：这个名字原先由 `renderer/mod.rs` 顶部的 use 块带进来（测试靠 glob 取用）。
use crate::host::frame::WHITE_TEXTURE_PATH;
use rustlua_render::mesh3d::GROUND_PLANE_SENTINEL;
// WP12 阶段 4c：视图解析搬进 `renderer::world_sprite` 后不再由 glob 带下来，显式指名。
use crate::host::renderer::world_sprite::resolve_world_sprite_view;

/// WP6 世界 Sprite 通道的遮挡契约：**测深度但不写深度**，因此顺序即一切。
/// ① 按 z 排成画家顺序（远→近）时近处胜出；② 顺序反过来时远处胜出——两条一起说明
/// 深度不会兜底，排序必须由宿主保证（`SpriteBatch::build_world_into` + `sort_world_ranges`）。
#[test]
fn world_sprite_pass_is_painter_ordered_without_depth_writes() {
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
        label: Some("rustlua-world-sprite-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).unwrap();
    // 相机中心 (0,0)、zoom = SIZE（1 世界单位 = SIZE 像素）⇒ 世界 ±0.5 覆盖整个目标。
    let camera = rustlua_camera::camera::Camera2D::new(
        rustlua_ecs::transform::Vec2::new(0.0, 0.0),
        SIZE as f32,
    )
    .expect("camera");
    renderer.set_world_sprite_camera(camera.view_projection(viewport, -1.0, 1.0).unwrap());

    let quad = |z: f32, color: [f32; 4]| {
        let mut vertices = Vec::new();
        for (x, y) in [
            (-0.5_f32, -0.5_f32),
            (0.5, -0.5),
            (-0.5, 0.5),
            (0.5, -0.5),
            (0.5, 0.5),
            (-0.5, 0.5),
        ] {
            vertices.push(WorldSpriteVertex {
                position: [x, y, z],
                uv: [0.5, 0.5],
                color,
            });
        }
        vertices
    };
    let mut mesh = SceneMesh {
        vertices: Vec::new(),
        draws: Vec::new(),
        // 画家顺序：先远（红，z=+0.5）、后近（绿，z=-0.5）。
        world_sprites: quad(0.5, [1.0, 0.0, 0.0, 1.0]),
        world_sprite_draws: Vec::new(),
        paths: vec![WHITE_TEXTURE_PATH.to_owned()],
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    };
    mesh.world_sprites.extend(quad(-0.5, [0.0, 1.0, 0.0, 1.0]));
    // 纯 2D 回滚档（宿主没有声明任何视图）：世界 Sprite 只有画家顺序，深度策略不参与，
    // 因此这里按"不写深度"构造，与本条测试的断言一一对应。
    mesh.world_sprite_draws = vec![
        rustlua_render::sprite_batch::SpriteDrawBatch::translucent(
            0,
            0,
            rustlua_render::layer::RenderLayer::World2D,
            0,
            6,
        ),
        rustlua_render::sprite_batch::SpriteDrawBatch::translucent(
            0,
            1,
            rustlua_render::layer::RenderLayer::World2D,
            6,
            6,
        ),
    ];

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-world-sprite-target"),
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
    });
    let center_of = |renderer: &mut Renderer, mesh: &SceneMesh, texture: &wgpu::Texture| {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        renderer.render_into_texture(mesh, None, &view, SIZE, SIZE);
        let pixels = renderer
            .copy_texture_to_cpu(texture, SIZE, SIZE)
            .expect("readback");
        let index = (((SIZE / 2) * SIZE + SIZE / 2) * 4) as usize;
        [
            pixels[index],
            pixels[index + 1],
            pixels[index + 2],
            pixels[index + 3],
        ]
    };

    // ① 画家顺序（远 → 近，宿主排好）：近处的绿色**后画**并盖住红色。这正是
    //    `sort_world_ranges` 保证的顺序，半透明因此可以正常叠加。
    let painted = center_of(&mut renderer, &mesh, &texture);
    assert_eq!(
        painted,
        [0, 255, 0, 255],
        "按 z 排成远→近后，近处的绿色应盖住远处的红色"
    );

    // ② 顺序反过来（近的先画、远的后画）：远处红色**确实**盖住绿色——这条通道
    //    **不写深度**，顺序就是一切。这条把"排序是宿主的责任"钉成契约：哪天有人
    //    以为深度会兜底，这里立刻判红。
    let mut reversed = SceneMesh {
        vertices: Vec::new(),
        draws: Vec::new(),
        // 反过来的顺序：先近（绿，z=-0.5）、后远（红，z=+0.5）。
        world_sprites: quad(-0.5, [0.0, 1.0, 0.0, 1.0]),
        world_sprite_draws: Vec::new(),
        paths: vec![WHITE_TEXTURE_PATH.to_owned()],
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    };
    reversed
        .world_sprites
        .extend(quad(0.5, [1.0, 0.0, 0.0, 1.0]));
    reversed.world_sprite_draws = mesh.world_sprite_draws.clone();
    let unpainted = center_of(&mut renderer, &reversed, &texture);
    assert_eq!(
        unpainted,
        [255, 0, 0, 255],
        "不写深度时后画的远处红色会盖住绿色——排序必须由宿主保证"
    );

    // ③ 一条**贴图 id 越界**的世界精灵 draw：绑定组拿不到 → 必须**记账为丢弃**，而不是裸 `continue`。
    //    这是本轮补的不对称——2D 叠加通道那处丢弃早有正面用例（`passes.rs`），世界精灵这处此前
    //    只有"不回归"。越界正好也走 `bind_group_for` 的 `paths.get(...)?`（修好前的直接下标会 panic）。
    mesh.world_sprite_draws
        .push(rustlua_render::sprite_batch::SpriteDrawBatch::translucent(
            99,
            2,
            rustlua_render::layer::RenderLayer::World2D,
            12,
            6,
        ));
    let still_painted = center_of(&mut renderer, &mesh, &texture);
    assert_eq!(
        still_painted,
        [0, 255, 0, 255],
        "丢一条越界的 draw 不该影响其余世界精灵的像素"
    );
    let metrics = renderer.last_frame_metrics();
    assert_eq!(
        metrics.dropped_draws, 1,
        "越界的 world sprite 贴图必须被记账（此前是裸 continue：世界里少一个 sprite、日志无字）"
    );
    assert_eq!(
        metrics.draw_calls, 2,
        "丢弃不计入 draw_calls——两条正常 draw 照画"
    );
}

/// docs/54 §8-④ 的核心证明：**同一世界视图**里 Sprite 与 Mesh 按深度策略正确互相遮挡。
///
/// 场景只用内建几何（`__ground_plane__` 哨兵），因此不依赖任何示例工程资源：
/// - 主视图 = 透视相机（`RenderView::main_3d` 携带视图投影矩阵），世界 Sprite 与网格共用它；
/// - 网格是那块地面四边形，绕 X 转 90° 后正对相机、放在 z = -1；
/// - 精灵是不透明四边形（alpha = 1 ⇒ 写深度）。
///
/// 两个方向各断言一次：精灵在网格**前面**时精灵胜出；精灵在网格**后面**时网格胜出。
/// 收敛前这两条都不可能同时成立——精灵有自己的深度附件、且永远在网格之后画。
#[test]
fn world_sprites_and_meshes_occlude_each_other_in_one_view() {
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
        label: Some("rustlua-mixed-occlusion-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).unwrap();
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 0.0, 4.0),
        Vec3::ZERO,
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("perspective camera");
    // 宿主声明主视图：世界 Sprite 通道因此与网格共用相机、视口与深度附件。
    renderer.set_frame_views(FrameViews {
        main: Some(RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR)),
        overlay: None,
    });

    // 地面四边形：绕 X 转 90° 后法线朝 +Z，落在 z = -1 的平面上，尺寸 ±4。
    let facing =
        Mat4::from_quat(Quat::from_axis_angle(Vec3::X, std::f32::consts::FRAC_PI_2).unwrap());
    let mesh_model = Mat4::translation(Vec3::new(0.0, 0.0, -1.0)) * facing;
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light::default(),
        drawables: vec![crate::host::frame::Scene3dDrawable {
            entity: rustlua_scene::scene::SceneEntityId(1),
            name: "Ground".to_owned(),
            mesh_path: GROUND_PLANE_SENTINEL.to_owned(),
            material: None,
            // 测试手写场景：给一个本地句柄即可（渲染器只把它当缓存键）。
            mesh_handle: Some(AssetHandle::new(0, 1)),
            model: mesh_model,
            palette: Vec::new(),
            bounds: crate::host::frame::Scene3dBounds {
                local_aabb: rustlua_math::math3d::Aabb3 {
                    min: Vec3::new(-4.0, -4.0, -0.1),
                    max: Vec3::new(4.0, 4.0, 0.1),
                },
                source: crate::host::frame::BoundsSource::Fallback,
            },
        }],
    };

    let quad = |z: f32| {
        let mut vertices = Vec::new();
        for (x, y) in [
            (-0.5_f32, -0.5_f32),
            (0.5, -0.5),
            (-0.5, 0.5),
            (0.5, -0.5),
            (0.5, 0.5),
            (-0.5, 0.5),
        ] {
            vertices.push(WorldSpriteVertex {
                position: [x, y, z],
                uv: [0.5, 0.5],
                color: [1.0, 0.0, 0.0, 1.0],
            });
        }
        vertices
    };
    let mesh_with_sprite = |z: f32| SceneMesh {
        vertices: Vec::new(),
        draws: Vec::new(),
        world_sprites: quad(z),
        // 不透明（alpha = 1）⇒ 这一段落写深度，因此能与网格互相遮挡。
        world_sprite_draws: vec![rustlua_render::sprite_batch::SpriteDrawBatch::opaque(
            0,
            0,
            rustlua_render::layer::RenderLayer::World2D,
            0,
            6,
        )],
        paths: vec![WHITE_TEXTURE_PATH.to_owned()],
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    };

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-mixed-occlusion-target"),
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
    });
    let center_of = |renderer: &mut Renderer, mesh: &SceneMesh| {
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        renderer.render_into_texture(mesh, Some(&scene), &view, SIZE, SIZE);
        let pixels = renderer
            .copy_texture_to_cpu(&target, SIZE, SIZE)
            .expect("readback");
        let index = (((SIZE / 2) * SIZE + SIZE / 2) * 4) as usize;
        [
            pixels[index],
            pixels[index + 1],
            pixels[index + 2],
            pixels[index + 3],
        ]
    };
    let is_sprite_red = |pixel: [u8; 4]| pixel[0] > 128 && pixel[1] < 96 && pixel[2] < 96;

    // ① 精灵在网格前面（z = 0 > -1）：精灵胜出。
    let in_front = center_of(&mut renderer, &mesh_with_sprite(0.0));
    assert!(
        is_sprite_red(in_front),
        "网格前面的不透明精灵必须可见，实际 {in_front:?}"
    );

    // ② 精灵在网格后面（z = -2 < -1）：网格胜出 ⇒ 网格**能遮挡**精灵。
    let behind = center_of(&mut renderer, &mesh_with_sprite(-2.0));
    assert!(
        !is_sprite_red(behind),
        "网格后面的精灵必须被网格遮住，实际 {behind:?}"
    );

    // ③ 半透明精灵即使在前也不写深度：它不会反过来遮住网格的深度（策略按分段下发）。
    let mut translucent = mesh_with_sprite(0.0);
    translucent.world_sprite_draws =
        vec![rustlua_render::sprite_batch::SpriteDrawBatch::translucent(
            0,
            0,
            rustlua_render::layer::RenderLayer::World2D,
            0,
            6,
        )];
    for vertex in &mut translucent.world_sprites {
        vertex.color = [1.0, 0.0, 0.0, 0.5];
    }
    let blended = center_of(&mut renderer, &translucent);
    assert!(
        blended != behind,
        "半透明精灵应与'被遮挡'是两种不同的画面（说明它确实画上去了）"
    );
}

/// docs/54 §8-④：世界 Sprite 到底用哪个相机、哪个深度附件，必须由**声明的视图**决定，
/// 而不是由"谁最后调了哪个 setter"决定。这条把三种情形逐条钉住（无需 GPU）。
#[test]
fn world_sprite_view_is_resolved_from_the_declared_views() {
    let viewport = Viewport::new(640.0, 360.0).unwrap();
    let half = Viewport::new(640.0, 180.0).unwrap();
    let camera3d = Camera3D::perspective(
        Vec3::new(0.0, 0.0, 6.0),
        Vec3::ZERO,
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .unwrap();
    let camera2d =
        rustlua_camera::camera::Camera2D::new(rustlua_ecs::transform::Vec2::zero(), 1.0).unwrap();
    let main = RenderView::main_3d(camera3d, viewport, DEFAULT_3D_CLEAR);
    let overlay = RenderView::overlay_2d(camera2d, half);
    let explicit = Mat4::identity();

    // ① 声明了主视图：与主视图共用相机、视口与深度（这才是"同一世界视图"）。
    let shared =
        resolve_world_sprite_view(Some(main), Some(overlay), main.matrix(), explicit, None);
    assert!(
        shared.shared_depth,
        "声明主视图时世界 Sprite 必须共用深度附件"
    );
    assert_eq!(
        shared.viewport,
        Some(viewport),
        "世界 Sprite 用主视图的视口"
    );
    assert_eq!(
        shared.matrix,
        main.matrix().expect("主 3D 视图应带视图投影矩阵"),
        "世界 Sprite 与网格必须是同一份投影矩阵"
    );

    // ② 只有 overlay（纯 2D 画面）：用 overlay 的世界相机，但深度附件独立。
    let isolated = resolve_world_sprite_view(None, Some(overlay), None, explicit, Some(viewport));
    assert!(!isolated.shared_depth, "纯 2D 画面没有网格可共享深度");
    assert_eq!(isolated.viewport, Some(half));
    assert_eq!(isolated.matrix, overlay.matrix().unwrap());

    // ③ 什么都没声明（回滚档）：用宿主显式设置的矩阵与传入的默认视口。
    let legacy = resolve_world_sprite_view(None, None, None, explicit, Some(viewport));
    assert!(!legacy.shared_depth);
    assert_eq!(legacy.matrix, explicit);
    assert_eq!(legacy.viewport, Some(viewport));
}

/// 主视图**没有**矩阵（退化相机 / 手工构造的字面量）时，世界 Sprite 退回显式矩阵而不是
/// 悄悄用一份单位阵——错的是取景，不该表现为"精灵全在屏幕中央"。
#[test]
fn world_sprite_view_falls_back_to_the_explicit_matrix_without_a_view_matrix() {
    let viewport = Viewport::new(320.0, 240.0).unwrap();
    let matrixless = RenderView::degenerate(
        Projection::Orthographic { zoom: 2.0 },
        viewport,
        ClearPolicy::None,
        0,
        u32::MAX,
    );
    let explicit = Mat4::translation(Vec3::new(3.0, 4.0, 5.0));
    let resolved = resolve_world_sprite_view(Some(matrixless), None, None, explicit, None);
    assert_eq!(resolved.matrix, explicit);
    assert!(resolved.shared_depth);
}
