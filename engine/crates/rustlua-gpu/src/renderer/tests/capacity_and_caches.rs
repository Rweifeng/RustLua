//! 原 `renderer/mod.rs` 的 `mod tests`（WP12 阶段 4a 纯搬运）。顶点缓冲扩容（2 的幂）与纹理/网格身份缓存。

use super::*;
// WP12 阶段 4b：这个名字原先由 `renderer/mod.rs` 顶部的 use 块带进来（测试靠 glob 取用）。
use rustlua_render::mesh3d::GROUND_PLANE_SENTINEL;

/// 2D 顶点缓冲容量策略：不小于初始容量、按 2 的幂增长、且总是能容纳请求。
/// 回归背景：此前容量固定为 1 MiB 且超限时**没有 else 分支**，超出的帧会
/// 静默丢掉整层 2D 绘制（含 UI 与文字），日志里没有任何线索。
#[test]
fn two_d_vertex_capacity_grows_to_powers_of_two() {
    assert_eq!(
        next_2d_vertex_capacity(0),
        INITIAL_2D_VERTEX_BUFFER_BYTES,
        "空帧不应缩小缓冲"
    );
    assert_eq!(
        next_2d_vertex_capacity(INITIAL_2D_VERTEX_BUFFER_BYTES),
        INITIAL_2D_VERTEX_BUFFER_BYTES,
        "正好装满不扩容"
    );
    // 超出一点：直接跳到 2 MiB（而不是 1 MiB + 若干字节），避免逐帧重建。
    assert_eq!(
        next_2d_vertex_capacity(INITIAL_2D_VERTEX_BUFFER_BYTES + 36),
        2 * 1024 * 1024
    );
    assert_eq!(next_2d_vertex_capacity(5 * 1024 * 1024), 8 * 1024 * 1024);
    // 任何请求都必须被容纳（扩容策略的正确性下限）。
    for required in [
        1_u64,
        1024,
        1024 * 1024 - 1,
        1024 * 1024 + 1,
        3 * 1024 * 1024 + 7,
        17 * 1024 * 1024,
    ] {
        let capacity = next_2d_vertex_capacity(required);
        assert!(
            capacity >= required,
            "capacity {capacity} must hold {required} bytes"
        );
        assert!(
            capacity.is_power_of_two(),
            "{capacity} must be a power of two"
        );
    }
}

/// docs/54 §4.4：贴图解析走**资源身份**（句柄）而不是每 draw 一次线性路径扫描。
///
/// 判据是三条可观测事实：第一帧按句柄未命中（首次加载）、第二帧同句柄命中（O(1) 复用）、
/// 同一资源**只加载一次**。宿主手写 `SceneMesh`（没有身份）时仍然按路径解析，行为不变。
#[test]
fn texture_identity_cache_is_hit_on_the_second_frame() {
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
        label: Some("rustlua-test-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let root = rustlua_infra::games_dir().join("examples").join("game_api");
    let texture_path = "assets/textures/player.png";
    if !root.join(texture_path).is_file() {
        return; // 没有示例资源就跳过（与"没有 GPU 就跳过"同一策略）
    }
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue.clone(), Some(root), false, false)
            .expect("offscreen renderer");
    let size = 32u32;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-test-identity"),
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

    // 宿主给出身份（运行时提取就是这条路径）。
    let quad = |handle: AssetHandle| SceneMesh {
        vertices: vec![
            OverlayVertex {
                position: [-1.0, -1.0],
                uv: [0.0, 1.0],
                color: [1.0; 4],
                sdf: 0,
            },
            OverlayVertex {
                position: [1.0, -1.0],
                uv: [1.0, 0.0],
                color: [1.0; 4],
                sdf: 0,
            },
            OverlayVertex {
                position: [-1.0, 1.0],
                uv: [0.0, 0.0],
                color: [1.0; 4],
                sdf: 0,
            },
            OverlayVertex {
                position: [1.0, -1.0],
                uv: [1.0, 0.0],
                color: [1.0; 4],
                sdf: 0,
            },
            OverlayVertex {
                position: [1.0, 1.0],
                uv: [1.0, 0.0],
                color: [1.0; 4],
                sdf: 0,
            },
            OverlayVertex {
                position: [-1.0, 1.0],
                uv: [0.0, 0.0],
                color: [1.0; 4],
                sdf: 0,
            },
        ],
        draws: vec![rustlua_render::frame::SceneDraw::world(1, 0, 6)],
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths: vec![texture_path.to_owned()],
        path_handles: vec![handle],
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    };

    let handle = AssetHandle::new(0, 1);
    renderer.render_into_texture(&quad(handle), None, &view, size, size);
    let first_metrics = renderer.last_frame_metrics();
    assert_eq!(
        first_metrics.draw_calls, 1,
        "一个 2D 批次就是一次 draw call"
    );
    assert_eq!(first_metrics.vertices, 6, "一个 quad 是 6 个顶点");
    assert_eq!(first_metrics.triangles, 2, "一个 quad 是两个三角形");
    assert_eq!(
        first_metrics.set_pass_calls, 1,
        "2D overlay 通道设置一次普通管线"
    );
    assert_eq!(
        renderer.texture_identity_stats(),
        (0, 1),
        "第一帧句柄还没进缓存：应记一次未命中"
    );
    assert_eq!(renderer.loaded_texture_count(), 1, "同一资源只应加载一次");
    let resources = renderer.resource_stats();
    assert_eq!(resources.loaded_textures, 1);
    assert!(
        resources.texture_bytes > 0,
        "贴图上传后应记录显存字节：{resources:?}"
    );
    assert_eq!(resources.texture_identity_misses, 1);
    renderer.render_into_texture(&quad(handle), None, &view, size, size);
    assert_eq!(
        renderer.texture_identity_stats(),
        (1, 1),
        "第二帧同句柄必须命中缓存（O(1) 复用，不再扫路径）"
    );
    assert_eq!(
        renderer.loaded_texture_count(),
        1,
        "命中缓存不该重复加载同一资源"
    );

    // 没有身份（宿主手写网格）时走旧路径：仍能画，但句柄统计不动。
    let mut legacy = quad(handle);
    legacy.path_handles.clear();
    renderer.render_into_texture(&legacy, None, &view, size, size);
    assert_eq!(
        renderer.texture_identity_stats(),
        (1, 1),
        "没有身份时按路径解析，身份统计不应变化"
    );
    assert_eq!(renderer.loaded_texture_count(), 1);
}

/// docs/54 §4.4：网格解析同样走**资源身份**——同一句柄第二帧命中缓存（O(1)），同一网格只
/// 解析一次；宿主没给身份（编辑器预览/测试夹具）时按路径解析，行为与收敛前一致。
#[test]
fn mesh_identity_cache_is_hit_on_the_second_frame() {
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
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 0.0, 4.0),
        Vec3::ZERO,
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("perspective camera");
    // 夹具自己拼 `Scene3d`，因此也要自己声明视口——视图与相机在这儿成帧一次。
    let viewport = Viewport::new(size as f32, size as f32).expect("viewport");
    let handle = AssetHandle::new(7, 3);
    let scene = |handle: Option<AssetHandle>| Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light::default(),
        drawables: vec![rustlua_render::frame::Scene3dDrawable {
            entity: rustlua_scene::scene::SceneEntityId(1),
            name: "Ground".to_owned(),
            mesh_path: GROUND_PLANE_SENTINEL.to_owned(),
            material: None,
            mesh_handle: handle,
            model: Mat4::translation(Vec3::new(0.0, 0.0, -1.0)),
            palette: Vec::new(),
            bounds: rustlua_render::frame::Scene3dBounds {
                local_aabb: rustlua_math::math3d::Aabb3 {
                    min: Vec3::new(-4.0, -4.0, -0.1),
                    max: Vec3::new(4.0, 4.0, 0.1),
                },
                source: rustlua_render::frame::BoundsSource::Fallback,
            },
        }],
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-test-mesh-identity"),
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
    let mesh = rustlua_render::frame::SceneMesh {
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

    renderer.render_into_texture(&mesh, Some(&scene(Some(handle))), &view, size, size);
    assert_eq!(
        renderer.mesh_identity_stats(),
        (0, 1),
        "第一帧句柄还没进缓存：记一次未命中"
    );
    assert_eq!(renderer.cached_mesh_count(), 1, "同一网格只解析一次");
    let resources = renderer.resource_stats();
    assert_eq!(resources.cached_meshes, 1);
    assert!(
        resources.mesh_bytes > 0,
        "网格上传后应记录顶点/索引缓冲字节：{resources:?}"
    );
    assert!(
        renderer.last_frame_metrics().vertices > 0,
        "3D 网格帧必须统计顶点数"
    );
    assert!(
        renderer.last_frame_metrics().set_pass_calls >= 2,
        "阴影 + 主网格两条通道各设置一次管线"
    );
    renderer.render_into_texture(&mesh, Some(&scene(Some(handle))), &view, size, size);
    assert_eq!(
        renderer.mesh_identity_stats(),
        (1, 1),
        "第二帧同句柄必须命中网格缓存"
    );
    assert_eq!(renderer.cached_mesh_count(), 1, "命中不该重复解析");

    // 没有身份：按路径解析（旧路径），身份统计不应变化。
    renderer.render_into_texture(&mesh, Some(&scene(None)), &view, size, size);
    assert_eq!(
        renderer.mesh_identity_stats(),
        (1, 1),
        "宿主没给身份时不该动身份统计"
    );
    assert_eq!(renderer.cached_mesh_count(), 1);
}

/// 回归：句柄只在**同一个** `RuntimeScene` 的 `AssetStore` 里唯一。编辑器重建运行时会从
/// (0, 1) 重新发号，而渲染器寿命更长；命中缓存必须核对映射路径，否则重建后会把旧网格
/// 当成新网格画出来（P3-R02 调试退化 UV 夹具时实际踩到）。
#[test]
fn mesh_identity_rejects_reused_handle_from_another_runtime() {
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
        label: Some("rustlua-test-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let root = rustlua_infra::games_dir().join("examples").join("game_api");
    let triangle = "assets/models/m8-test-triangle.glb";
    let quad = "assets/models/p3-r02-quad.gltf";
    if !root.join(triangle).is_file() || !root.join(quad).is_file() {
        return;
    }
    // 离屏渲染器是 Sync 加载模式：`cached_mesh` 返回前资源已解码上传。
    let mut renderer = Renderer::new_offscreen(device, queue, Some(root), false, false)
        .expect("offscreen renderer");
    let reused = AssetHandle::new(0, 1);
    let vertex_count = |renderer: &mut Renderer, path: &str| {
        renderer
            .cached_mesh(Some(reused), path)
            .map(|mesh| mesh.primitives[0].geometry.vertex_count)
            .unwrap_or(0)
    };
    assert_eq!(
        vertex_count(&mut renderer, triangle),
        3,
        "三角形夹具三个顶点"
    );
    // 重建后的新运行时用同一个 (0, 1) 指向另一条路径——不能复用旧网格。
    assert_eq!(
        vertex_count(&mut renderer, quad),
        4,
        "句柄重号但路径不同：必须按新路径重新解析"
    );
    // 回到第一条路径仍按路径命中缓存，而不是又解析一次。
    assert_eq!(vertex_count(&mut renderer, triangle), 3);
    assert_eq!(
        renderer.cached_mesh_count(),
        2,
        "两条路径各缓存一份，重号句柄不产生额外副本"
    );
}
