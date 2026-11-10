//! P3-R185：不透明前到后排序门——近处网格先写深度，被挡像素靠 early-Z 拒绝。
//!
//! 钉住三件事：① 默认档不透明 draw 按组内最近实例的平方距离升序（声明顺序远→近时
//! 输出必须近→远）；② `--no-depth-sort` 回滚档保持声明顺序；③ 实例合批组同样参与
//! 排序（HashMap 遍历顺序任意，排序后输出确定）。不透明渲染与顺序无关，两档像素一致；
//! Blend 队列在排序之后才追加，不参与本排序。
//!
//! 网格用地面哨兵；skinned（palette 非空）drawable 走单 draw 通道，声明顺序可控。
//! 剔除关闭：x=30 的远块在 fov 90°/5 单位距离之外，开着会被视锥剔掉，判据就不成立了。

use super::*;
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, RenderView, Viewport};
use rustlua_math::math3d::{Mat4, Vec3};
use rustlua_render::mesh3d::GROUND_PLANE_SENTINEL;

const SIZE: u32 = 32;

fn test_device(tag: &str) -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some(tag),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    Some((device, queue))
}

fn drawable(entity: u64, x: f32) -> rustlua_render::frame::Scene3dDrawable {
    rustlua_render::frame::Scene3dDrawable {
        entity: rustlua_scene::scene::SceneEntityId(entity),
        name: format!("plane-{entity}"),
        mesh_path: GROUND_PLANE_SENTINEL.to_owned(),
        material: None,
        mesh_handle: None,
        model: Mat4::translation(Vec3::new(x, 0.0, -1.0)),
        palette: Vec::new(),
        instances: Vec::new(),
        bounds: rustlua_render::frame::Scene3dBounds {
            local_aabb: rustlua_math::math3d::Aabb3 {
                min: Vec3::new(-4.0, -4.0, -0.1),
                max: Vec3::new(4.0, 4.0, 0.1),
            },
            source: rustlua_render::frame::BoundsSource::Fallback,
        },
    }
}

fn skinned(
    mut drawable: rustlua_render::frame::Scene3dDrawable,
) -> rustlua_render::frame::Scene3dDrawable {
    drawable.palette.push(Mat4::identity());
    drawable
}

fn scene_with(drawables: Vec<rustlua_render::frame::Scene3dDrawable>) -> Scene3d {
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 0.0, 4.0),
        Vec3::ZERO,
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("perspective camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light::default(),
        point_lights: Vec::new(),
        drawables,
    }
}

fn first_model_x(draw: &FrameDraw) -> f32 {
    draw.models[0].columns[3][0]
}

#[test]
fn opaque_draws_sort_front_to_back_and_rollback_keeps_declaration_order() {
    let Some((device, queue)) = test_device("rustlua-depth-sort-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer =
        Renderer::new_offscreen(device, queue, None, false, true).expect("offscreen renderer");
    // 声明顺序：远（x=30）在前、近（x=0）在后。
    let scene = scene_with(vec![skinned(drawable(1, 30.0)), skinned(drawable(2, 0.0))]);

    let draws = renderer.frame_3d_draws(&scene);
    assert_eq!(draws.len(), 2);
    assert_eq!(first_model_x(&draws[0]), 0.0, "默认档近者先画（early-Z）");
    assert_eq!(first_model_x(&draws[1]), 30.0);

    // 回滚档：声明顺序原样保留。
    renderer.set_depth_sort_enabled(false);
    let draws = renderer.frame_3d_draws(&scene);
    assert_eq!(draws.len(), 2);
    assert_eq!(first_model_x(&draws[0]), 30.0, "回滚档保持声明顺序");
    assert_eq!(first_model_x(&draws[1]), 0.0);

    // 再开回来：排序恢复（开关双向都生效，不是只读一次）。
    renderer.set_depth_sort_enabled(true);
    let draws = renderer.frame_3d_draws(&scene);
    assert_eq!(first_model_x(&draws[0]), 0.0);
}

#[test]
fn instanced_groups_sort_by_nearest_instance() {
    let Some((device, queue)) = test_device("rustlua-depth-sort-groups-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer =
        Renderer::new_offscreen(device, queue, None, false, true).expect("offscreen renderer");
    // 不同 material 覆盖键 ⇒ 两个合批组（覆盖解析失败回退内嵌材质，不影响分组键）。
    // 远组两个实例 x=30/29，最近也是 29 ⇒ 键仍比近组 x=0 远，近组先画。
    let mut far_a = drawable(1, 30.0);
    far_a.material = Some("materials/far.rmat".to_owned());
    let mut far_b = drawable(2, 29.0);
    far_b.material = Some("materials/far.rmat".to_owned());
    let mut near = drawable(3, 0.0);
    near.material = Some("materials/near.rmat".to_owned());
    let scene = scene_with(vec![far_a, far_b, near]);

    let draws = renderer.frame_3d_draws(&scene);
    assert_eq!(draws.len(), 2, "两个合批组各出一个 draw");
    assert_eq!(first_model_x(&draws[0]), 0.0, "近组先画");
    assert_eq!(draws[0].models.len(), 1);
    assert_eq!(draws[1].models.len(), 2, "远组实例完整（合批不被排序拆散）");
    assert_eq!(first_model_x(&draws[1]), 30.0);
}
