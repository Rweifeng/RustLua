//! M98 软粒子像素门：GPU 粒子片元着色器按「粒子自身深度 vs 场景深度」做软边。
//!
//! 场景：z = -1 的地面平面（覆盖全屏），相机在 z = 4 看向 -Z。
//! - 粒子放在 z = 0（明显在平面前面）：`delta` 大 ⇒ 不淡出，中心像素接近粒子色。
//! - 粒子放在 z = -1（与平面重合）：`delta ≈ 0` ⇒ 软边淡出/裁剪，中心像素接近清屏色。
//! - 再对比两种软边宽度：宽度越大，同样位置下淡出越少（像素更亮）⇒ 证明配置真的进了 shader。
//!
//! 没有 compute 能力时按本目录口径 `gpu_skipped`（`RUSTLUA_REQUIRE_GPU=1` 下跳过即失败）。

use super::*;
use crate::GpuParticleConfig3D;
use rustlua_ecs::particle::{ParticleEmitter, ParticleMode};
use rustlua_render::mesh3d::GROUND_PLANE_SENTINEL;

const SIZE: u32 = 64;

fn soft_edge_renderer() -> Option<(wgpu::Device, wgpu::Queue, Renderer)> {
    let (device, queue) = super::compute::headless()?;
    let renderer = Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
        .expect("offscreen renderer");
    renderer
        .compute_enabled()
        .then_some((device, queue, renderer))
}

fn empty_mesh() -> SceneMesh {
    SceneMesh {
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
    }
}

/// 一块覆盖全屏的地面平面（z = -1，法线 +Z，尺寸 ±4），与 `world_sprite` 夹具同一写法。
fn ground_scene(camera: Camera3D, viewport: Viewport) -> Scene3d {
    let facing =
        Mat4::from_quat(Quat::from_axis_angle(Vec3::X, std::f32::consts::FRAC_PI_2).unwrap());
    let model = Mat4::translation(Vec3::new(0.0, 0.0, -1.0)) * facing;
    Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light::default(),
        point_lights: Vec::new(),
        drawables: vec![rustlua_render::frame::Scene3dDrawable {
            entity: rustlua_scene::scene::SceneEntityId(1),
            name: "Ground".to_owned(),
            mesh_path: GROUND_PLANE_SENTINEL.to_owned(),
            material: None,
            mesh_handle: Some(AssetHandle::new(0, 1)),
            model,
            palette: Vec::new(),
            instances: Vec::new(),
            bounds: rustlua_render::frame::Scene3dBounds {
                local_aabb: rustlua_math::math3d::Aabb3 {
                    min: Vec3::new(-4.0, -4.0, -0.1),
                    max: Vec3::new(4.0, 4.0, 0.1),
                },
                source: rustlua_render::frame::BoundsSource::Fallback,
            },
        }],
    }
}

fn particle_burst(origin_z: f32) -> GpuParticleConfig3D {
    GpuParticleConfig3D {
        origin: [0.0, 0.0, origin_z],
        emitter: ParticleEmitter {
            count: 64,
            speed: 0.0,
            life: 10.0,
            size_start: 2.0,
            size_end: 2.0,
            color_start: [1.0, 1.0, 1.0, 1.0],
            color_end: [1.0, 1.0, 1.0, 1.0],
            mode: ParticleMode::Gpu,
            seed: Some(7),
            ..ParticleEmitter::default()
        },
        capacity: 64,
    }
}

fn render_particles(renderer: &mut Renderer, device: &wgpu::Device, origin_z: f32) -> Vec<u8> {
    renderer
        .push_gpu_particle_burst_3d(particle_burst(origin_z))
        .expect("push gpu particle burst");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).unwrap();
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 0.0, 4.0),
        Vec3::ZERO,
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("perspective camera");
    renderer.set_frame_views(FrameViews {
        main: Some(RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR)),
        overlay: None,
    });
    let scene = ground_scene(camera, viewport);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-particle-soft-edge-target"),
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
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let outcome =
        renderer.render_into_texture(&empty_mesh(), Some(&scene), &view, SIZE, SIZE, true);
    assert!(matches!(outcome, RenderOutcome::Presented));
    let pixels = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback");
    // 这一批粒子已经画过，清空避免下一轮重复叠加。
    renderer.clear_gpu_particles();
    pixels
}

fn center_luma(pixels: &[u8]) -> u32 {
    let index = (((SIZE / 2) * SIZE + SIZE / 2) * 4) as usize;
    u32::from(pixels[index]) + u32::from(pixels[index + 1]) + u32::from(pixels[index + 2])
}

#[test]
fn gpu_particles_soft_edge_fades_when_touching_scene_depth() {
    let Some((device, _queue, mut renderer)) = soft_edge_renderer() else {
        return gpu_skipped("device has no compute");
    };
    // 平面前方：不淡出（对照）。
    let front = center_luma(&render_particles(&mut renderer, &device, 0.0));
    // 与平面重合：软边应该把它淡掉。
    let touching = center_luma(&render_particles(&mut renderer, &device, -1.0));
    assert!(
        front > touching + 20,
        "贴住场景深度时粒子应被软边淡出：front={front} touching={touching}"
    );

    // 软边宽度真的进 shader：在不同「粒子-平面」间距下扫一遍，只要有一档
    // 窄/宽软边的像素不同，就证明宽度进了管线（不知道 NDC 深度的绝对量级，
    // 所以不赌单点，扫一段距离找证据）。
    let mut differed = Vec::new();
    for z in [-0.3_f32, -0.5, -0.7, -0.85] {
        renderer.set_particle_softness(0.02);
        let narrow = center_luma(&render_particles(&mut renderer, &device, z));
        renderer.set_particle_softness(0.4);
        let wide = center_luma(&render_particles(&mut renderer, &device, z));
        differed.push((z, narrow, wide));
    }
    assert!(
        differed
            .iter()
            .any(|(_, narrow, wide)| narrow.abs_diff(*wide) > 8),
        "窄/宽软边在所有间距下像素都一样，宽度没进管线：{differed:?}"
    );
}
